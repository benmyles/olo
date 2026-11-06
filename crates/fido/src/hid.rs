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

/// Call `changed` whenever a HID device node comes or goes (a key
/// plugged in or out), from udev's own announcements (netlink, group 2:
/// sent once udev has set the node up, its permissions too), on a thread
/// of its own that sleeps till then. Errors if the socket can't be had.
pub fn watch(changed: impl Fn() + Send + 'static) -> io::Result<()> {
    // SAFETY: plain socket calls on a socket of our own, with structs
    // sized as the kernel expects.
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    // udev's group: after its rules have run.
    addr.nl_groups = 2;
    let bound = unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) };
    if bound < 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    std::thread::Builder::new().name("fido-plugging".into()).spawn(move || {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            let msg = &buf[..n as usize];
            if is_hidraw_change(msg) {
                changed();
            }
        }
        unsafe { libc::close(fd) };
    })?;
    Ok(())
}

/// Whether a udev announcement is a hidraw node added or removed.
fn is_hidraw_change(msg: &[u8]) -> bool {
    // libudev's: a header ("libudev", a magic number, its size, then where
    // the properties are and how long), the properties after it.
    let props = if msg.starts_with(b"libudev\0") && msg.len() >= 24 {
        let at = u32::from_ne_bytes([msg[16], msg[17], msg[18], msg[19]]) as usize;
        let len = u32::from_ne_bytes([msg[20], msg[21], msg[22], msg[23]]) as usize;
        msg.get(at..at.saturating_add(len)).unwrap_or(&[])
    } else {
        msg
    };
    let has = |needle: &[u8]| props.split(|&b| b == 0).any(|p| p == needle);
    has(b"SUBSYSTEM=hidraw") && (has(b"ACTION=add") || has(b"ACTION=remove"))
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
    fn hidraw_announcements() {
        // As udev sends them: its header, then the properties.
        let udev = |props: &[u8]| {
            let mut m = b"libudev\0".to_vec();
            m.extend_from_slice(&0xfeedcafeu32.to_be_bytes());
            m.extend_from_slice(&40u32.to_ne_bytes());
            m.extend_from_slice(&40u32.to_ne_bytes());
            m.extend_from_slice(&(props.len() as u32).to_ne_bytes());
            m.resize(40, 0x55);
            m.extend_from_slice(props);
            m
        };
        assert!(is_hidraw_change(&udev(b"ACTION=add\0DEVPATH=/devices/x/hidraw/hidraw4\0SUBSYSTEM=hidraw\0DEVNAME=/dev/hidraw4\0")));
        assert!(is_hidraw_change(&udev(b"ACTION=remove\0SUBSYSTEM=hidraw\0")));
        assert!(!is_hidraw_change(&udev(b"ACTION=add\0SUBSYSTEM=input\0")));
        assert!(!is_hidraw_change(&udev(b"ACTION=change\0SUBSYSTEM=hidraw\0")));
        // The kernel's own form.
        assert!(is_hidraw_change(b"add@/devices/x/hidraw/hidraw4\0ACTION=add\0SUBSYSTEM=hidraw\0"));
    }

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
