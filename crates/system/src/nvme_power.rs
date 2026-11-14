//! NVMe drives' power: how much each may draw while it works, so a drive
//! can be kept cooler (Preferences ▸ Power ▸ Power Modes; Ben,
//! 2026-11-13: "add the ability to adjust/customize nvme power for each
//! profile for when a user wants to get their nvme drive to run a bit
//! cooler (but default should be whatever it is now / by default)").
//!
//! A drive lists its power states (Identify Controller's power state
//! descriptors): operational ones it works in, each with the most it
//! draws (PS0 the most), and non-operational ones it rests in. The host
//! picks the operational state it works in (Set Features, Power
//! Management); resting (APST, which the kernel sets up) is the drive's
//! own, and it comes back to the state the host picked. A lower state
//! caps the drive's power, so its heat and its peak speed: in the
//! drive's own words, nothing guessed.
//!
//! Natively, through the kernel's admin passthrough on the controller's
//! character device (`NVME_IOCTL_ADMIN_CMD` on /dev/nvmeN), which is
//! root's: read by `olo-gpu-tune` (the follower), which says what it
//! found in `gpu_tuning::Followed`. The drives themselves (name, model,
//! serial) are read from /sys without root ([`drives`]). A controller
//! reset (some resumes) puts it back on PS0: the follower sets it again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// An NVMe drive (its controller), as /sys says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drive {
    /// "nvme0": its character device is /dev/nvme0.
    pub name: String,
    pub model: String,
    /// What it's known by in the plan (names can change between starts).
    pub serial: String,
}

impl Drive {
    /// Its model as said to a person: "SKHynix_HFS002TFM9X179N" →
    /// "SKHynix HFS002TFM9X179N".
    pub fn label(&self) -> String {
        self.model.replace('_', " ")
    }
}

/// One of a drive's power states.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerState {
    pub index: u8,
    /// The most it draws in it, in watts.
    pub max_watts: f32,
    /// A state it works in (else one it rests in).
    pub operational: bool,
}

/// What a power mode puts a drive on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NvmePreset {
    /// Left as it is: the drive's own (PS0), untouched.
    #[default]
    AsIs,
    /// Its second working state: a little less power and heat.
    Cooler,
    /// Its lowest working state.
    Coolest,
}

impl NvmePreset {
    pub const ALL: [NvmePreset; 3] = [NvmePreset::AsIs, NvmePreset::Cooler, NvmePreset::Coolest];

    pub fn name(self) -> &'static str {
        match self {
            NvmePreset::AsIs => "As It Is",
            NvmePreset::Cooler => "Cooler",
            NvmePreset::Coolest => "Coolest",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            NvmePreset::AsIs => "as-is",
            NvmePreset::Cooler => "cooler",
            NvmePreset::Coolest => "coolest",
        }
    }

    pub fn parse(s: &str) -> Option<NvmePreset> {
        NvmePreset::ALL.into_iter().find(|p| p.id() == s)
    }

    /// The working state it is among `states` (None: as it is, or the
    /// drive has no state for it).
    pub fn state(self, states: &[PowerState]) -> Option<u8> {
        let working: Vec<u8> = states.iter().filter(|s| s.operational).map(|s| s.index).collect();
        match self {
            NvmePreset::AsIs => None,
            NvmePreset::Cooler => working.get(1).copied(),
            NvmePreset::Coolest => working.last().copied().filter(|&i| i != 0),
        }
    }
}

/// Every NVMe drive here, by name (sysfs's; no root needed).
pub fn drives() -> Vec<Drive> {
    drives_under(Path::new("/sys/class/nvme"))
}

pub fn drives_under(class: &Path) -> Vec<Drive> {
    let read = |p: PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_owned()).unwrap_or_default();
    let mut v: Vec<Drive> = std::fs::read_dir(class)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            // (Only controllers on PCIe: not fabrics' or a namespace's
            // generic node.)
            (name.starts_with("nvme") && read(p.join("transport")) == "pcie").then(|| Drive { name, model: read(p.join("model")), serial: read(p.join("serial")) })
        })
        .collect();
    v.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then(a.name.cmp(&b.name)));
    v
}

/// The kernel's admin passthrough command (`struct nvme_passthru_cmd`).
#[repr(C)]
#[derive(Default)]
struct Passthru {
    opcode: u8,
    flags: u8,
    rsvd1: u16,
    nsid: u32,
    cdw2: u32,
    cdw3: u32,
    metadata: u64,
    addr: u64,
    metadata_len: u32,
    data_len: u32,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
    timeout_ms: u32,
    result: u32,
}

/// `_IOWR('N', 0x41, struct nvme_passthru_cmd)`.
const NVME_IOCTL_ADMIN_CMD: libc::c_ulong = (3 << 30) | ((std::mem::size_of::<Passthru>() as libc::c_ulong) << 16) | ((b'N' as libc::c_ulong) << 8) | 0x41;
const IDENTIFY: u8 = 0x06;
const SET_FEATURES: u8 = 0x09;
const GET_FEATURES: u8 = 0x0a;
/// The Power Management feature.
const POWER_MANAGEMENT: u32 = 0x02;

fn admin(dev: &std::fs::File, cmd: &mut Passthru) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    cmd.timeout_ms = 5_000;
    // SAFETY: `cmd` is a valid nvme_passthru_cmd for the call's length,
    // and `addr`, when set, points at a buffer of `data_len` bytes that
    // outlives the call.
    let r = unsafe { libc::ioctl(dev.as_raw_fd(), NVME_IOCTL_ADMIN_CMD, cmd as *mut Passthru) };
    match r {
        0 => Ok(cmd.result),
        r if r < 0 => Err(std::io::Error::last_os_error()),
        // (A positive answer is the drive's status: it refused.)
        status => Err(std::io::Error::other(format!("the drive refused (status {status:#x})"))),
    }
}

fn open(drive: &str) -> std::io::Result<std::fs::File> {
    std::fs::File::open(Path::new("/dev").join(drive))
}

/// Its power states, from its Identify Controller data (root).
pub fn power_states(drive: &str) -> std::io::Result<Vec<PowerState>> {
    let dev = open(drive)?;
    let mut data = vec![0u8; 4096];
    let mut cmd = Passthru { opcode: IDENTIFY, cdw10: 1, addr: data.as_mut_ptr() as u64, data_len: data.len() as u32, ..Default::default() };
    admin(&dev, &mut cmd)?;
    Ok(parse_states(&data))
}

/// The power state descriptors in Identify Controller data: NPSS (byte
/// 263, zero-based) and 32 bytes a state from byte 2048 (the most it
/// draws in bytes 0–1, in centiwatts, or 0.1 mW when MXPS, byte 3 bit
/// 0, is set; non-operational: byte 3 bit 1).
pub fn parse_states(id: &[u8]) -> Vec<PowerState> {
    let Some(&npss) = id.get(263) else { return Vec::new() };
    (0..=npss as usize)
        .filter_map(|i| {
            let d = id.get(2048 + i * 32..2048 + i * 32 + 32)?;
            let mp = u16::from_le_bytes([d[0], d[1]]) as f32;
            let scale = if d[3] & 1 != 0 { 0.0001 } else { 0.01 };
            Some(PowerState { index: i as u8, max_watts: mp * scale, operational: d[3] & 2 == 0 })
        })
        .collect()
}

/// The working state it's on now (root).
pub fn current(drive: &str) -> std::io::Result<u8> {
    let dev = open(drive)?;
    let mut cmd = Passthru { opcode: GET_FEATURES, cdw10: POWER_MANAGEMENT, ..Default::default() };
    Ok((admin(&dev, &mut cmd)? & 0x1f) as u8)
}

/// Put it on working state `state` (root): not saved on the drive, so
/// it lasts until it's reset or the computer restarts.
pub fn set(drive: &str, state: u8) -> std::io::Result<()> {
    let dev = std::fs::OpenOptions::new().read(true).write(true).open(Path::new("/dev").join(drive))?;
    let mut cmd = Passthru { opcode: SET_FEATURES, cdw10: POWER_MANAGEMENT, cdw11: u32::from(state & 0x1f), ..Default::default() };
    admin(&dev, &mut cmd).map(|_| ())
}

/// A drive as the follower found and left it: its power states and the
/// one it's on.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveState {
    pub serial: String,
    pub model: String,
    pub name: String,
    pub states: Vec<PowerState>,
    /// The working state it's on (None: it couldn't be asked).
    pub now: Option<u8>,
}

/// What a preset is on a drive in words, with what it draws at most:
/// "Cooler (6.4 W)".
pub fn choice_name(p: NvmePreset, states: &[PowerState]) -> String {
    match (p, p.state(states).and_then(|i| states.iter().find(|s| s.index == i))) {
        (NvmePreset::AsIs, _) => p.name().into(),
        (_, Some(s)) => format!("{} ({:.1} W)", p.name(), s.max_watts),
        (_, None) => p.name().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drive's states read from its Identify data: three it works in
    /// (8.25, 6.3 and 5.4 W), two it rests in.
    #[test]
    fn power_states_from_identify_data() {
        let mut id = vec![0u8; 4096];
        id[263] = 4;
        for (i, (centiwatts, rest)) in [(825u16, false), (630, false), (540, false), (5, true), (1, true)].into_iter().enumerate() {
            let d = &mut id[2048 + i * 32..];
            d[..2].copy_from_slice(&centiwatts.to_le_bytes());
            d[3] = if rest { 2 } else { 0 };
        }
        let states = parse_states(&id);
        assert_eq!(states.len(), 5);
        assert!((states[1].max_watts - 6.3).abs() < 1e-4 && states[1].operational && !states[3].operational);
        assert_eq!((NvmePreset::AsIs.state(&states), NvmePreset::Cooler.state(&states), NvmePreset::Coolest.state(&states)), (None, Some(1), Some(2)));
        assert_eq!(choice_name(NvmePreset::Coolest, &states), "Coolest (5.4 W)");
        // One working state: nothing cooler to be had.
        assert_eq!((NvmePreset::Cooler.state(&states[..1]), NvmePreset::Coolest.state(&states[..1])), (None, None));
    }

    /// The ioctl's number, as the kernel's header makes it.
    #[test]
    fn the_admin_ioctl() {
        assert_eq!(std::mem::size_of::<Passthru>(), 72);
        assert_eq!(NVME_IOCTL_ADMIN_CMD, 0xC048_4E41);
    }

    #[test]
    fn drives_from_sys() {
        let root = Path::new("/var/tmp").join(format!("olo-nvme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, t: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        for (n, model) in [("nvme1", "Second"), ("nvme0", "SKHynix_HFS002TFM9X179N")] {
            w(&format!("{n}/transport"), "pcie\n");
            w(&format!("{n}/model"), &format!("{model}   \n"));
            w(&format!("{n}/serial"), &format!("S{n}\n"));
        }
        w("nvme-fabrics/transport", "");
        let d = drives_under(&root);
        assert_eq!(d.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["nvme0", "nvme1"]);
        assert_eq!((d[0].label(), d[0].serial.as_str()), ("SKHynix HFS002TFM9X179N".to_owned(), "Snvme0"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
