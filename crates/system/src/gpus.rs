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
pub fn pci_name(vendor: u16, device: u16) -> Option<String> {
    let db = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"].iter().map(read).find(|t| !t.is_empty())?;
    model_in(&db, vendor, device)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
