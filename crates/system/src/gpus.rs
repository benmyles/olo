//! The computer's GPUs, as the kernel's DRM devices show them: each one's
//! PCI address, its maker and model (named from the PCI ID database,
//! hwdata's pci.ids), its driver, whether the computer started on it,
//! its card and render nodes, and the display connectors wired to it.
//! Read from sysfs only: a GPU asleep stays asleep.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Gpu {
    /// Its PCI address ("0000:01:00.0"): the same from one start to the
    /// next, how a choice of GPU is kept.
    pub pci: String,
    pub vendor: u16,
    pub device: u16,
    pub driver: String,
    /// "Intel Graphics", "NVIDIA RTX PRO 2000 Blackwell Generation Laptop GPU".
    pub name: String,
    /// The one the computer started on (`boot_vga`).
    pub boot: bool,
    /// Its nodes: "/dev/dri/card1", "/dev/dri/renderD129".
    pub card: Option<PathBuf>,
    pub render: Option<PathBuf>,
    /// The display connectors wired to it ("eDP-1", "HDMI-A-2").
    pub connectors: Vec<String>,
    /// Its folder in sysfs.
    pub sysfs: PathBuf,
}

impl Gpu {
    pub fn maker(&self) -> &'static str {
        maker(self.vendor)
    }
}

fn maker(vendor: u16) -> &'static str {
    match vendor {
        0x10de => "NVIDIA",
        0x8086 => "Intel",
        0x1002 => "AMD",
        0x106b => "Apple",
        _ => "GPU",
    }
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn hex(p: impl AsRef<Path>) -> u16 {
    u16::from_str_radix(read(p).trim().trim_start_matches("0x"), 16).unwrap_or(0)
}

/// Every GPU with a DRM card, the one the computer started on first.
pub fn list() -> Vec<Gpu> {
    list_in(Path::new("/sys/class/drm"))
}

fn list_in(drm: &Path) -> Vec<Gpu> {
    let mut out: Vec<Gpu> = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(drm).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for card in entries.iter().filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-'))) {
        let Ok(device) = card.join("device").canonicalize() else { continue };
        let Some(pci) = device.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if out.iter().any(|g| g.pci == pci) {
            continue;
        }
        let (vendor, product) = (hex(device.join("vendor")), hex(device.join("device")));
        let driver = std::fs::read_link(device.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        let card_name = card.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let connectors = entries.iter().filter_map(|p| p.file_name()?.to_str()?.strip_prefix(&format!("{card_name}-")).map(str::to_owned)).collect();
        let render = std::fs::read_dir(device.join("drm")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("renderD"));
        let model = pci_name(vendor, product);
        let m = maker(vendor);
        let name = match model {
            Some(model) if model.starts_with(m) => model,
            Some(model) => format!("{m} {model}"),
            None => format!("{m} GPU"),
        };
        out.push(Gpu {
            pci,
            vendor,
            device: product,
            driver,
            name,
            boot: read(device.join("boot_vga")).trim() == "1",
            card: Some(PathBuf::from("/dev/dri").join(&card_name)),
            render: render.map(|r| PathBuf::from("/dev/dri").join(r)),
            connectors,
            sysfs: device,
        });
    }
    out.sort_by_key(|g| !g.boot);
    out
}

/// A PCI device's model as the PCI ID database names it: the marketing
/// name in its brackets where it has one ("GB206GLM [RTX PRO 2000
/// Blackwell Generation Laptop GPU]": the latter).
/// Looked up once for each device a program asks about and kept: the
/// database is a megabyte and a half, read and gone through line by line,
/// and app_server answers `Request::Gpus` on its own loop (2.7 ms a list
/// of two GPUs before this, 2026-11-21; see `tests::live`).
pub fn pci_name(vendor: u16, device: u16) -> Option<String> {
    static NAMES: std::sync::Mutex<Vec<((u16, u16), Option<String>)>> = std::sync::Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, name)) = names.iter().find(|(id, _)| *id == (vendor, device)) {
        return name.clone();
    }
    let name = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"].iter().map(read).find(|t| !t.is_empty()).and_then(|db| model_in(&db, vendor, device));
    names.push(((vendor, device), name.clone()));
    name
}

fn model_in(db: &str, vendor: u16, device: u16) -> Option<String> {
    let (vendor, device) = (format!("{vendor:04x}"), format!("{device:04x}"));
    let mut in_vendor = false;
    for line in db.lines() {
        if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            in_vendor = line.starts_with(&vendor);
        } else if in_vendor
            && let Some(rest) = line.strip_prefix('\t')
            && !rest.starts_with('\t')
            && let Some(model) = rest.strip_prefix(&device)
        {
            let model = model.trim();
            let model = match (model.find('['), model.rfind(']')) {
                (Some(a), Some(b)) if b > a => &model[a + 1..b],
                _ => model,
            };
            return Some(model.to_owned());
        }
    }
    None
}

/// The GPU at PCI address `pci`, if there is one.
pub fn by_pci(pci: &str) -> Option<Gpu> {
    list().into_iter().find(|g| g.pci == pci)
}

/// xe's queue priorities, as its config query says them.
pub const XE_NORMAL: u8 = 1;
pub const XE_HIGH: u8 = 2;

/// `struct drm_xe_device_query` (the kernel's xe_drm.h).
#[repr(C)]
#[derive(Default)]
struct XeQuery {
    extensions: u64,
    query: u32,
    size: u32,
    data: u64,
    reserved: [u64; 2],
}

/// `DRM_IOCTL_XE_DEVICE_QUERY`: `_IOWR('d', DRM_COMMAND_BASE + 0, struct
/// drm_xe_device_query)`.
const XE_DEVICE_QUERY: libc::c_ulong = (3 << 30) | ((std::mem::size_of::<XeQuery>() as libc::c_ulong) << 16) | ((b'd' as libc::c_ulong) << 8) | 0x40;
/// `DRM_XE_DEVICE_QUERY_CONFIG`, and in its answer
/// `DRM_XE_QUERY_CONFIG_MAX_EXEC_QUEUE_PRIORITY`.
const XE_CONFIG: u32 = 2;
const XE_CONFIG_MAX_PRIORITY: usize = 4;

/// The highest priority Intel's xe would give work of the calling
/// thread's on the GPU `device` is an open node of (0 low, [`XE_NORMAL`],
/// [`XE_HIGH`]): the driver's own word, by its config query, and the
/// test it makes whenever a queue is made
/// (`xe_exec_queue_device_get_max_priority`: high for a thread that
/// may set priorities, CAP_SYS_NICE, as the kernel and its security
/// module see it). What EGL says of a context's priority isn't this:
/// Mesa asks the kernel for no more than the driver allows and reports
/// what the program asked (HACKING, "The GPU has priorities too").
/// None: not xe's device (the number means something else to another
/// driver, so it is never sent to one), or it wouldn't say. Two system
/// calls on a node the caller holds open; it opens nothing, so a GPU
/// asleep stays asleep.
pub fn xe_highest_priority(device: std::os::fd::BorrowedFd<'_>) -> Option<u8> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::MetadataExt as _;
    let node = std::fs::File::from(device.try_clone_to_owned().ok()?).metadata().ok()?.rdev();
    let (major, minor) = (libc::major(node), libc::minor(node));
    let driver = std::fs::read_link(format!("/sys/dev/char/{major}:{minor}/device/driver")).ok()?;
    if driver.file_name()? != "xe" {
        return None;
    }
    let mut query = XeQuery { query: XE_CONFIG, ..Default::default() };
    // SAFETY: the kernel's structure, ours; with no room given it says
    // how much the answer takes.
    if unsafe { libc::ioctl(device.as_raw_fd(), XE_DEVICE_QUERY, &mut query as *mut XeQuery) } != 0 {
        return None;
    }
    let mut answer = vec![0u64; (query.size as usize).div_ceil(8)];
    query.data = answer.as_mut_ptr() as u64;
    // SAFETY: `answer` is at least the `size` bytes the kernel asked for.
    if unsafe { libc::ioctl(device.as_raw_fd(), XE_DEVICE_QUERY, &mut query as *mut XeQuery) } != 0 {
        return None;
    }
    xe_max_priority_in(&answer)
}

/// From `struct drm_xe_query_config` as 64-bit words: the count of its
/// values (32 bits, then padding), then the values.
fn xe_max_priority_in(answer: &[u64]) -> Option<u8> {
    let values = (*answer.first()? & 0xffff_ffff) as usize;
    if values <= XE_CONFIG_MAX_PRIORITY {
        return None;
    }
    u8::try_from(*answer.get(1 + XE_CONFIG_MAX_PRIORITY)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drivers_word_on_priority() {
        // Five values, the fifth the highest priority; an older driver's
        // four have none; a short answer has none.
        assert_eq!(xe_max_priority_in(&[5, 0xb0a0, 0x12, 4096, 48, 2]), Some(2));
        assert_eq!(xe_max_priority_in(&[5 | 7 << 32, 0, 0, 0, 0, 1]), Some(1));
        assert_eq!(xe_max_priority_in(&[4, 0, 0, 0, 0]), None);
        assert_eq!(xe_max_priority_in(&[5, 0, 0]), None);
        assert_eq!(xe_max_priority_in(&[]), None);
        assert_eq!(std::mem::size_of::<XeQuery>(), 40);
        assert_eq!(XE_DEVICE_QUERY, 0xc028_6440);
        // A file that is no DRM node is asked nothing.
        let file = std::fs::File::open("/proc/self/status").unwrap();
        assert_eq!(xe_highest_priority(std::os::fd::AsFd::as_fd(&file)), None);
    }

    /// This computer's Intel card, asked for this (ordinary) process:
    /// `cargo test -p olo-system the_drivers_word_here -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "asks this computer's own driver"]
    fn the_drivers_word_here() {
        for g in list().into_iter().filter(|g| g.driver == "xe") {
            let node = std::fs::File::open(g.render.as_ref().unwrap()).unwrap();
            let highest = xe_highest_priority(std::os::fd::AsFd::as_fd(&node));
            println!("{}: the highest priority this process may have: {highest:?}; may set priorities: {}", g.pci, crate::caps::held(crate::caps::SYS_NICE));
            assert_eq!(highest, Some(if crate::caps::held(crate::caps::SYS_NICE) { XE_HIGH } else { XE_NORMAL }));
            // What an asking costs (app_server asks once as it takes a GPU
            // up): on a release build.
            let started = std::time::Instant::now();
            const TIMES: u32 = 2000;
            for _ in 0..TIMES {
                std::hint::black_box(xe_highest_priority(std::os::fd::AsFd::as_fd(&node)));
            }
            println!("an asking: {:.1} µs", started.elapsed().as_secs_f64() * 1e6 / f64::from(TIMES));
        }
    }

    #[test]
    fn names_from_the_database() {
        let db = "# comment\n10de  NVIDIA Corporation\n\t2d39  GB206GLM [RTX PRO 2000 Blackwell Generation Laptop GPU]\n\t\t17aa 235c  ThinkPad\n8086  Intel Corporation\n\tb0a0  Panther Lake [Intel Graphics]\n\t1234  Plain Name\n";
        assert_eq!(model_in(db, 0x10de, 0x2d39).as_deref(), Some("RTX PRO 2000 Blackwell Generation Laptop GPU"));
        assert_eq!(model_in(db, 0x8086, 0xb0a0).as_deref(), Some("Intel Graphics"));
        assert_eq!(model_in(db, 0x8086, 0x1234).as_deref(), Some("Plain Name"));
        assert_eq!(model_in(db, 0x8086, 0x9999), None);
    }

    /// This computer's (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn live() {
        for g in list() {
            eprintln!("{g:?}");
        }
        // What a list costs whoever asks for one (app_server answers
        // `Request::Gpus` with it on its own loop): on a release build
        // (`cargo test --release -p olo-system gpus::tests::live -- --ignored --nocapture`).
        let started = std::time::Instant::now();
        const TIMES: u32 = 200;
        for _ in 0..TIMES {
            std::hint::black_box(list());
        }
        eprintln!("a list: {:.0} µs", started.elapsed().as_secs_f64() * 1e6 / f64::from(TIMES));
    }
}
