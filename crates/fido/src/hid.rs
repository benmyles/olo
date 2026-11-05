//! A FIDO key's HID interface: finding it (`devices`, by its report
//! descriptor's usage page, 0xF1D0) and its 64-byte reports
//! ([`Transport`], [`Hidraw`]).

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    time::Duration,
};

/// A CTAPHID report's size.
pub const REPORT: usize = 64;

/// A FIDO interface found: its hidraw node and what its driver calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub path: PathBuf,
    /// "Yubico YubiKey OTP+FIDO+CCID".
    pub name: String,
    pub vendor: u32,
    pub product: u32,
}

/// Every FIDO (CTAPHID) interface plugged in, in hidraw order. Reads
/// /sys only: nothing is opened.
pub fn devices() -> Vec<Device> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/hidraw") else { return out };
    for e in dir.flatten() {
        let dev = e.path().join("device");
        let Ok(rd) = std::fs::read(dev.join("report_descriptor")) else { continue };
        if !is_fido(&rd) {
            continue;
        }
        let uevent = std::fs::read_to_string(dev.join("uevent")).unwrap_or_default();
        let field = |k: &str| uevent.lines().find_map(|l| l.strip_prefix(k)).map(str::to_owned);
        // HID_ID=0003:00001050:00000407 (bus, vendor, product).
        let ids = field("HID_ID=").and_then(|id| {
            let mut p = id.split(':').skip(1);
            Some((u32::from_str_radix(p.next()?, 16).ok()?, u32::from_str_radix(p.next()?, 16).ok()?))
        });
        let (vendor, product) = ids.unwrap_or((0, 0));
        out.push(Device { path: PathBuf::from("/dev").join(e.file_name()), name: field("HID_NAME=").unwrap_or_default(), vendor, product });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether a HID report descriptor is a FIDO interface's: a Usage Page
/// of 0xF1D0 with a Usage of 1 (CTAPHID).
pub fn is_fido(rd: &[u8]) -> bool {
    let mut i = 0;
    let mut page = 0u32;
    while i < rd.len() {
        let b = rd[i];
        if b == 0xFE {
            // A long item: its size, its tag, its data.
            i += 3 + *rd.get(i + 1).unwrap_or(&0) as usize;
            continue;
        }
        let size = match b & 3 {
            3 => 4,
            s => s as usize,
        };
        let v = (0..size).fold(0u32, |v, k| v | (*rd.get(i + 1 + k).unwrap_or(&0) as u32) << (8 * k));
        match b & 0xFC {
            0x04 => page = v,
            // (A four-byte usage carries its page in its top half.)
            0x08 if (size == 4 && v == 0xF1D0_0001) || (page == 0xF1D0 && v == 1) => return true,
            _ => {}
        }
        i += 1 + size;
    }
    false
}

/// Reports to and from a key.
pub trait Transport: Send {
    fn send(&mut self, report: &[u8; REPORT]) -> io::Result<()>;
    /// The next report, or `None` once `wait` has gone by without one.
    fn recv(&mut self, wait: Duration) -> io::Result<Option<[u8; REPORT]>>;
}

/// A key's hidraw node.
pub struct Hidraw(File);

impl Hidraw {
    pub fn open(d: &Device) -> io::Result<Self> {
        Ok(Hidraw(OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(&d.path)?))
    }
}

impl Transport for Hidraw {
    fn send(&mut self, report: &[u8; REPORT]) -> io::Result<()> {
        // The report number first: 0, as FIDO interfaces have none.
        let mut out = [0u8; REPORT + 1];
        out[1..].copy_from_slice(report);
        self.0.write_all(&out)
    }

    fn recv(&mut self, wait: Duration) -> io::Result<Option<[u8; REPORT]>> {
        let mut fd = libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = wait.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one valid pollfd, for the length given.
        let n = unsafe { libc::poll(&mut fd, 1, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(None) } else { Err(e) };
        }
        if n == 0 {
            return Ok(None);
        }
        if fd.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "the key was unplugged"));
        }
        let mut r = [0u8; REPORT];
        let got = self.0.read(&mut r)?;
        if got == 0 {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "the key was unplugged"));
        }
        Ok(Some(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fido_interface_by_its_descriptor() {
        // A YubiKey 5's FIDO interface, as its hidraw node gives it (the
        // start: usage page 0xF1D0, usage 1, a collection).
        assert!(is_fido(&[0x06, 0xD0, 0xF1, 0x09, 0x01, 0xA1, 0x01, 0x09, 0x20, 0x15, 0x00]));
        // Its OTP interface: a keyboard (generic desktop, usage 6).
        assert!(!is_fido(&[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07]));
        // A vendor page with usage 1 elsewhere isn't.
        assert!(!is_fido(&[0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01]));
        // An extended usage.
        assert!(is_fido(&[0x0B, 0x01, 0x00, 0xD0, 0xF1, 0xA1, 0x01]));
    }
}
