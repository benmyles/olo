//! The computer's human presence sensor: whether someone is in front of
//! it, and looking at the screen. A ThinkPad P1 Gen 9's is in its camera
//! module (a HID sensor hub: the kernel's `hid_sensor_prox`, an IIO
//! device named "prox" with `proximity0`, someone there, and
//! `attention`, looking). Its buffer (`/dev/iio:deviceN`) carries a
//! sample as either changes; Olo's udev rule (`73-olo-presence.rules`,
//! Olo Doctor installs it) gives it to the session and turns the buffer
//! on. Nothing polls: `_raw` is read once at the start (each read asks
//! the sensor), then the buffer is waited on.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// What the sensor says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Presence {
    /// Someone is in front of the computer.
    pub present: bool,
    /// They're looking at the screen (None: the sensor doesn't say).
    pub attentive: Option<bool>,
}

/// The presence sensor's IIO folder, if this computer has one.
pub fn sensor() -> Option<PathBuf> {
    std::fs::read_dir("/sys/bus/iio/devices")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| std::fs::read_to_string(p.join("name")).is_ok_and(|n| n.trim() == "prox") && p.join("in_proximity0_raw").exists())
}

/// Whether the session may read it (Olo's udev rule is in).
pub fn readable(sensor: &Path) -> bool {
    let dev = Path::new("/dev").join(sensor.file_name().unwrap_or_default());
    std::fs::File::open(dev).is_ok()
}

fn read(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

/// What it says now (each `_raw` read asks the sensor: at the start only).
pub fn now(sensor: &Path) -> Presence {
    let num = |f: &str| read(sensor.join(f)).and_then(|v| v.parse::<f64>().ok());
    Presence { present: num("in_proximity0_raw").is_some_and(|v| v > 0.0), attentive: num("in_attention_input").map(|v| v > 0.0) }
}

/// One element of a buffered sample: where it is and how it's stored.
#[derive(Debug, Clone, PartialEq)]
struct Element {
    name: String,
    offset: usize,
    bytes: usize,
    signed: bool,
    bits: u32,
    shift: u32,
    big_endian: bool,
}

/// The buffer's layout, from `scan_elements` (the enabled elements, by
/// index, each aligned to its own storage size).
fn layout(sensor: &Path) -> (Vec<Element>, usize) {
    let dir = sensor.join("scan_elements");
    let mut found: Vec<(u32, String, String)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            let name = f.strip_suffix("_en")?.to_owned();
            (read(dir.join(&f))? == "1").then_some(())?;
            Some((read(dir.join(format!("{name}_index")))?.parse().ok()?, name.clone(), read(dir.join(format!("{name}_type")))?))
        })
        .collect();
    found.sort();
    let mut offset: usize = 0;
    let mut out: Vec<Element> = Vec::new();
    for (_, name, ty) in found {
        let Some(e) = parse_type(&name, &ty) else { continue };
        offset = offset.div_ceil(e.bytes) * e.bytes;
        out.push(Element { offset, ..e });
        offset += out.last().map_or(0, |e| e.bytes);
    }
    // (A sample is padded to its largest element.)
    let largest = out.iter().map(|e| e.bytes).max().unwrap_or(1);
    (out, offset.div_ceil(largest) * largest)
}

/// "le:s8/32>>0": little-endian, signed, 8 bits stored in 32, shifted 0.
fn parse_type(name: &str, ty: &str) -> Option<Element> {
    let (endian, rest) = ty.split_once(':')?;
    let signed = rest.starts_with('s');
    let rest = &rest[1..];
    let (bits, rest) = rest.split_once('/')?;
    let (storage, shift) = rest.split_once(">>").unwrap_or((rest, "0"));
    let storage: usize = storage.split('X').next()?.parse().ok()?;
    Some(Element { name: name.to_owned(), offset: 0, bytes: storage / 8, signed, bits: bits.parse().ok()?, shift: shift.parse().ok()?, big_endian: endian == "be" })
}

fn value(e: &Element, sample: &[u8]) -> Option<i64> {
    let b = sample.get(e.offset..e.offset + e.bytes)?;
    let mut raw: u64 = 0;
    for (i, byte) in b.iter().enumerate() {
        let at = if e.big_endian { e.bytes - 1 - i } else { i };
        raw |= u64::from(*byte) << (8 * at);
    }
    raw >>= e.shift;
    let mask = if e.bits >= 64 { u64::MAX } else { (1u64 << e.bits) - 1 };
    raw &= mask;
    Some(if e.signed && e.bits < 64 && raw >> (e.bits - 1) & 1 == 1 { (raw | !mask) as i64 } else { raw as i64 })
}

/// Call `changed` with what the sensor says now and as it changes, on a
/// thread of its own, until `changed` answers false. None if there's no
/// sensor the session can read.
pub fn watch(mut changed: impl FnMut(Presence) -> bool + Send + 'static) -> Option<std::thread::JoinHandle<()>> {
    let sensor = sensor()?;
    let dev = Path::new("/dev").join(sensor.file_name()?);
    let mut file = std::fs::File::open(&dev).ok()?;
    std::thread::Builder::new()
        .name("presence".into())
        .spawn(move || {
            let mut last = now(&sensor);
            if !changed(last) {
                return;
            }
            let (elements, size) = layout(&sensor);
            if size == 0 {
                tracing::warn!("presence: its buffer has nothing enabled");
                return;
            }
            let mut buf = vec![0u8; size * 16];
            loop {
                // (Blocks until the sensor says something.)
                let n = match file.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                for sample in buf[..n].chunks_exact(size) {
                    let get = |name: &str| elements.iter().find(|e| e.name == name).and_then(|e| value(e, sample));
                    let next = Presence { present: get("in_proximity0").map_or(last.present, |v| v > 0), attentive: get("in_attention").map(|v| v != 0).or(last.attentive) };
                    if next != last {
                        last = next;
                        if !changed(next) {
                            return;
                        }
                    }
                }
            }
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_read_as_the_kernel_lays_them_out() {
        let p = parse_type("in_proximity0", "le:s8/32>>0").unwrap();
        assert_eq!((p.bytes, p.bits, p.signed), (4, 8, true));
        let a = Element { offset: 4, ..parse_type("in_attention", "le:s8/32>>0").unwrap() };
        let p16 = parse_type("in_proximity1", "le:s16/32>>0").unwrap();
        assert_eq!(p16.bits, 16);
        // Someone there (1), attention 100; then -1 as s8.
        let sample = [1, 0, 0, 0, 100, 0, 0, 0];
        assert_eq!(value(&p, &sample), Some(1));
        assert_eq!(value(&a, &sample), Some(100));
        assert_eq!(value(&p, &[0xff, 0, 0, 0]), Some(-1));
    }

    /// This computer's sensor (`cargo test -p olo-system presence -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn live() {
        let s = sensor();
        eprintln!("{s:?} readable {:?} now {:?}", s.as_deref().map(readable), s.as_deref().map(now));
    }
}
