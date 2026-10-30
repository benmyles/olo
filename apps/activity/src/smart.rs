//! A disk's own account of its health: NVMe's SMART / health log (log
//! page 2), read with an admin command on its controller (`/dev/nvmeN`).
//! Only root may send one, so it comes with full access (the helper
//! samples as root); without, the disk's health is "needs full access".

use serde::{Deserialize, Serialize};

/// `struct nvme_admin_cmd` (linux/nvme_ioctl.h), 72 bytes.
#[repr(C)]
#[derive(Default)]
struct AdminCmd {
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

/// `NVME_IOCTL_ADMIN_CMD`: `_IOWR('N', 0x41, struct nvme_admin_cmd)`.
const ADMIN_CMD: libc::c_ulong = (3 << 30) | ((std::mem::size_of::<AdminCmd>() as libc::c_ulong) << 16) | ((b'N' as libc::c_ulong) << 8) | 0x41;
const GET_LOG_PAGE: u8 = 0x02;
const HEALTH_LOG: u32 = 0x02;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// Critical warning bits (spare low, temperature, reliability,
    /// read-only, volatile backup): none is good.
    pub warning: u8,
    pub temperature: f64,
    /// Spare capacity left, and the threshold below which it warns (%).
    pub spare: u8,
    pub spare_threshold: u8,
    /// How much of its rated life is used (%, may pass 100).
    pub used: u8,
    pub read_bytes: u128,
    pub written_bytes: u128,
    pub power_on_hours: u128,
    pub power_cycles: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
}

impl Health {
    /// In a word or two.
    pub fn verdict(&self) -> &'static str {
        match self.warning {
            0 if self.used < 90 => "Good",
            0 => "Worn",
            w if w & 0x08 != 0 => "Read-only",
            _ => "Failing",
        }
    }
}

fn le(b: &[u8]) -> u128 {
    b.iter().rev().fold(0u128, |acc, &x| (acc << 8) | x as u128)
}

/// The 512-byte health log, as NVMe 1.4 §5.14.1.2 lays it out.
pub fn parse(log: &[u8; 512]) -> Health {
    // Data units are thousands of 512-byte blocks.
    let units = |at: usize| le(&log[at..at + 16]) * 512_000;
    Health {
        warning: log[0],
        temperature: le(&log[1..3]) as f64 - 273.15,
        spare: log[3],
        spare_threshold: log[4],
        used: log[5],
        read_bytes: units(32),
        written_bytes: units(48),
        power_cycles: le(&log[112..128]),
        power_on_hours: le(&log[128..144]),
        unsafe_shutdowns: le(&log[144..160]),
        media_errors: le(&log[160..176]),
    }
}

/// Controller `nvme0`'s health log.
pub fn read(controller: &str) -> std::io::Result<Health> {
    use std::os::fd::AsRawFd;
    let dev = std::fs::File::open(format!("/dev/{controller}"))?;
    let mut log = [0u8; 512];
    let mut cmd = AdminCmd {
        opcode: GET_LOG_PAGE,
        nsid: 0xFFFF_FFFF,
        addr: log.as_mut_ptr() as u64,
        data_len: log.len() as u32,
        // The number of dwords, less one, and which log.
        cdw10: ((log.len() as u32 / 4 - 1) << 16) | HEALTH_LOG,
        ..Default::default()
    };
    // SAFETY: an admin command whose buffer is `log`, alive and as long as
    // it says, for the length of the call.
    if unsafe { libc::ioctl(dev.as_raw_fd(), ADMIN_CMD as _, &mut cmd) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(parse(&log))
}

/// The controller a namespace is on: `nvme0n1` is `nvme0`'s.
pub fn controller_of(disk: &str) -> Option<String> {
    let rest = disk.strip_prefix("nvme")?;
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!n.is_empty() && rest[n.len()..].starts_with('n')).then(|| format!("nvme{n}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_the_kernels() {
        assert_eq!(std::mem::size_of::<AdminCmd>(), 72);
        assert_eq!(ADMIN_CMD, 0xC048_4E41);
    }

    #[test]
    fn reads_a_health_log() {
        let mut log = [0u8; 512];
        log[1..3].copy_from_slice(&310u16.to_le_bytes());
        log[3] = 100;
        log[4] = 99;
        log[5] = 3;
        log[48..56].copy_from_slice(&1000u64.to_le_bytes());
        log[128..136].copy_from_slice(&4321u64.to_le_bytes());
        log[144] = 7;
        let h = parse(&log);
        assert!((h.temperature - 36.85).abs() < 0.01);
        assert_eq!((h.spare, h.spare_threshold, h.used), (100, 99, 3));
        assert_eq!(h.written_bytes, 512_000_000);
        assert_eq!((h.power_on_hours, h.unsafe_shutdowns), (4321, 7));
        assert_eq!(h.verdict(), "Good");
        assert_eq!(controller_of("nvme0n1").as_deref(), Some("nvme0"));
        assert_eq!(controller_of("sda"), None);
    }

    #[test]
    fn needs_root() {
        // As this user the controller can't be opened (or the command is
        // refused): an error, never a made-up health.
        if unsafe { libc::geteuid() } != 0 && std::path::Path::new("/dev/nvme0").exists() {
            assert!(read("nvme0").is_err());
        }
    }
}
