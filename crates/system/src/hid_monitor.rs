//! Monitors whose brightness is set over USB rather than a backlight or
//! DDC/CI: Apple's Studio Display and Pro Display XDR, LG's UltraFine.
//! They follow the USB Monitor Control class: a HID feature report with
//! the VESA VCP Brightness usage. We find it in the device's report
//! descriptor and read and write it through `/dev/hidrawN` (a udev
//! `uaccess` rule gives the session's user the device; see
//! `data/udev/70-olo-displays.rules`).

use std::{
    fs::File,
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

use crate::backlight::{to_level, to_raw};

/// The brightness field of a feature report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    report: u8,
    /// Bit offset and size within the report (after its id byte).
    offset: usize,
    bits: usize,
    min: u32,
    max: u32,
    /// The whole report's length in bytes, id included.
    len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidMonitor {
    /// `/dev/hidrawN`.
    pub path: PathBuf,
    pub vendor: u16,
    pub product: u16,
    field: Field,
}

const MONITOR_VCP_PAGE: u32 = 0x82;
const VCP_BRIGHTNESS: u32 = 0x10;

/// The brightness field in a HID report descriptor, if it has one.
fn parse(desc: &[u8]) -> Option<Field> {
    let mut page = 0u32;
    let (mut min, mut max, mut size, mut count, mut report) = (0u32, 0u32, 0usize, 0usize, 0u8);
    let mut usages: Vec<u32> = Vec::new();
    // Bits so far in each feature report.
    let mut lengths = std::collections::HashMap::<u8, usize>::new();
    let mut found = None;
    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xfe {
            // A long item: skip it.
            let n = *desc.get(i + 1)? as usize;
            i += 3 + n;
            continue;
        }
        let n = [0, 1, 2, 4][(prefix & 3) as usize];
        let data = desc.get(i + 1..i + 1 + n)?;
        let value = data.iter().rev().fold(0u32, |v, &b| (v << 8) | b as u32);
        i += 1 + n;
        match prefix & 0xfc {
            0x04 => page = value,
            0x14 => min = value,
            0x24 => max = value,
            0x74 => size = value as usize,
            0x84 => report = value as u8,
            0x94 => count = value as usize,
            // A usage; four bytes carry their own page.
            0x08 => usages.push(if n == 4 { value } else { (page << 16) | value }),
            0xb0 => {
                let at = lengths.entry(report).or_default();
                if found.is_none() && usages.contains(&((MONITOR_VCP_PAGE << 16) | VCP_BRIGHTNESS)) && size > 0 && size <= 32 {
                    found = Some(Field { report, offset: *at, bits: size, min, max, len: 0 });
                }
                *at += size * count;
                usages.clear();
            }
            // Other main items end the local state too.
            0x80 | 0x90 | 0xa0 | 0xc0 => usages.clear(),
            _ => {}
        }
    }
    let mut f = found?;
    f.len = 1 + lengths.get(&f.report)?.div_ceil(8);
    (f.max > f.min).then_some(f)
}

/// `_IOC(_IOC_READ | _IOC_WRITE, 'H', nr, len)`.
fn hidioc(nr: u64, len: usize) -> u64 {
    (3 << 30) | ((len as u64) << 16) | ((b'H' as u64) << 8) | nr
}

fn feature(file: &File, buf: &mut [u8], set: bool) -> io::Result<()> {
    let nr = if set { 0x06 } else { 0x07 };
    // SAFETY: `buf` is valid for `buf.len()` bytes, as the request says.
    let r = unsafe { libc::ioctl(file.as_raw_fd(), hidioc(nr, buf.len()) as _, buf.as_mut_ptr()) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn ids(dir: &Path) -> Option<(u16, u16)> {
    let uevent = std::fs::read_to_string(dir.join("device/uevent")).ok()?;
    let id = uevent.lines().find_map(|l| l.strip_prefix("HID_ID="))?;
    let mut parts = id.split(':').skip(1);
    let vendor = u32::from_str_radix(parts.next()?, 16).ok()?;
    let product = u32::from_str_radix(parts.next()?, 16).ok()?;
    Some((vendor as u16, product as u16))
}

/// Every connected monitor with a USB brightness control.
pub fn all() -> Vec<HidMonitor> {
    let Ok(dir) = std::fs::read_dir("/sys/class/hidraw") else { return Vec::new() };
    let mut found: Vec<HidMonitor> = dir
        .flatten()
        .filter_map(|e| {
            let sys = e.path();
            let desc = std::fs::read(sys.join("device/report_descriptor")).ok()?;
            let field = parse(&desc)?;
            let (vendor, product) = ids(&sys)?;
            Some(HidMonitor { path: Path::new("/dev").join(e.file_name()), vendor, product, field })
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

impl HidMonitor {
    /// The maker's EDID (PNP) id, to tell which display this is.
    pub fn edid_vendor(&self) -> Option<&'static str> {
        match self.vendor {
            0x05ac => Some("APP"),
            0x043e => Some("GSM"),
            _ => None,
        }
    }

    fn read(&self, file: &File) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; self.field.len];
        buf[0] = self.field.report;
        feature(file, &mut buf, false)?;
        Ok(buf)
    }

    pub fn raw(&self) -> io::Result<u32> {
        let buf = self.read(&File::open(&self.path)?)?;
        Ok(get_bits(&buf[1..], self.field.offset, self.field.bits))
    }

    pub fn set_raw(&self, raw: u32) -> io::Result<()> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&self.path)?;
        // The report's other fields stay as they are.
        let mut buf = self.read(&file)?;
        let raw = raw.clamp(self.field.min, self.field.max);
        set_bits(&mut buf[1..], self.field.offset, self.field.bits, raw);
        feature(&file, &mut buf, true)
    }

    /// Perceived brightness, 0–1 (as the built-in display's).
    pub fn level(&self) -> io::Result<f32> {
        let f = self.field;
        Ok(to_level(self.raw()?.saturating_sub(f.min), f.max - f.min))
    }

    pub fn set_level(&self, level: f32) -> io::Result<()> {
        let f = self.field;
        // Never off: the lowest is the display's own minimum.
        self.set_raw(f.min + to_raw(level, f.max - f.min))
    }
}

fn get_bits(bytes: &[u8], offset: usize, bits: usize) -> u32 {
    (0..bits).fold(0u32, |v, i| {
        let b = offset + i;
        let bit = bytes.get(b / 8).map_or(0, |byte| (byte >> (b % 8)) & 1);
        v | ((bit as u32) << i)
    })
}

fn set_bits(bytes: &mut [u8], offset: usize, bits: usize, value: u32) {
    for i in 0..bits {
        let b = offset + i;
        if let Some(byte) = bytes.get_mut(b / 8) {
            let mask = 1 << (b % 8);
            if value >> i & 1 == 1 { *byte |= mask } else { *byte &= !mask }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Apple Studio Display's interface 7.
    const STUDIO: &str = "05800901a101850106820009101690012760ea000067e1000001550e75209501b142050f0950150026204e661001550d7510b14206820009101690012760ea000067e1000001550e752095018102c0";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn finds_the_studio_display_brightness() {
        let f = parse(&hex(STUDIO)).unwrap();
        assert_eq!(f, Field { report: 1, offset: 0, bits: 32, min: 400, max: 60000, len: 7 });
        // What the display answered: 20000.
        let report = hex("01204e00000000");
        assert_eq!(get_bits(&report[1..], f.offset, f.bits), 20000);
        let mut r = report.clone();
        set_bits(&mut r[1..], f.offset, f.bits, 60000);
        assert_eq!(r, hex("0160ea00000000"));
    }

    #[test]
    fn ignores_other_devices() {
        // A keyboard's interface: no monitor usages.
        assert_eq!(parse(&hex("0600ff0953a101850115002501750895190600ff09538102c0")), None);
    }

    /// Sets every USB-controlled monitor to full brightness and reads it
    /// back (on real hardware: `cargo test -p olo-system live_full --
    /// --ignored`).
    #[test]
    #[ignore]
    fn live_full() {
        for m in all() {
            m.set_level(1.0).unwrap();
            assert_eq!(m.raw().unwrap(), m.field.max, "{}", m.path.display());
            eprintln!("{}: full ({})", m.path.display(), m.field.max);
        }
    }

    #[test]
    #[ignore]
    fn live_read() {
        for m in all() {
            eprintln!("{} {:04x}:{:04x} raw {:?} level {:?}", m.path.display(), m.vendor, m.product, m.raw(), m.level());
        }
    }
}
