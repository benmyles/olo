//! The infrared emitter: the flood light the camera's infrared frames
//! need. Some cameras light it whenever their infrared node streams;
//! others (Chicony's, in ThinkPads among them) wait to be told through a
//! vendor control in one of their UVC extension units, which Windows
//! Hello's driver knows and Linux doesn't. Olo finds it as Windows' own
//! setup would: for each control of the infrared interface's extension
//! units that can be set, each value its range suggests, set while the
//! frames are watched for the flood (the face's region, or the whole
//! frame, a good deal brighter); the one that lights it is kept (per
//! camera, by its USB id) and set each time the camera opens. Whatever
//! didn't is put back as it was.

use std::{
    io,
    os::fd::{AsRawFd, OwnedFd},
    path::Path,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{camera::Frames, model::Gray};

const UVCIOC_CTRL_QUERY: libc::c_ulong = ((3u64 << 30) | (16u64 << 16) | ((b'u' as u64) << 8) | 0x21) as libc::c_ulong;
const SET_CUR: u8 = 0x01;
const GET_CUR: u8 = 0x81;
const GET_MAX: u8 = 0x83;
const GET_LEN: u8 = 0x85;
const GET_INFO: u8 = 0x86;
const GET_DEF: u8 = 0x87;

/// How a camera's emitter is lit: its extension unit, the control, the
/// value to set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Emitter {
    /// The camera's USB vendor and product ("04f2:b8b3").
    pub camera: String,
    pub unit: u8,
    pub selector: u8,
    pub value: Vec<u8>,
}

fn query(fd: &OwnedFd, unit: u8, selector: u8, q: u8, data: &mut [u8]) -> io::Result<()> {
    #[repr(C)]
    struct Query {
        unit: u8,
        selector: u8,
        query: u8,
        size: u16,
        data: *mut u8,
    }
    let mut x = Query { unit, selector, query: q, size: data.len() as u16, data: data.as_mut_ptr() };
    // SAFETY: the struct the request names, its data `size` long.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), UVCIOC_CTRL_QUERY, &mut x as *mut Query) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// A control's length (bytes), if it has one.
fn len(fd: &OwnedFd, unit: u8, selector: u8) -> Option<usize> {
    let mut b = [0u8; 2];
    query(fd, unit, selector, GET_LEN, &mut b).ok()?;
    Some(u16::from_le_bytes(b) as usize).filter(|n| (1..=64).contains(n))
}

fn get(fd: &OwnedFd, unit: u8, selector: u8, q: u8, n: usize) -> Option<Vec<u8>> {
    let mut b = vec![0u8; n];
    query(fd, unit, selector, q, &mut b).ok().map(|_| b)
}

/// Set a control (`value` its whole length).
pub fn set(fd: &OwnedFd, unit: u8, selector: u8, value: &[u8]) -> io::Result<()> {
    let mut b = value.to_vec();
    query(fd, unit, selector, SET_CUR, &mut b)
}

/// The extension units of a camera's interface (its sysfs directory,
/// "…/3-4:1.2"): each unit's id and the selectors its bitmap says it has.
pub fn units(interface: &Path) -> Vec<(u8, Vec<u8>)> {
    let Some(dev) = interface.parent() else { return Vec::new() };
    let Ok(b) = std::fs::read(dev.join("descriptors")) else { return Vec::new() };
    let number: u8 = std::fs::read_to_string(interface.join("bInterfaceNumber")).ok().and_then(|s| u8::from_str_radix(s.trim(), 16).ok()).unwrap_or(0);
    let (mut out, mut i, mut iface) = (Vec::new(), 0usize, None::<(u8, u8, u8)>);
    while i + 2 <= b.len() {
        let len = b[i] as usize;
        if len < 2 || i + len > b.len() {
            break;
        }
        let kind = b[i + 1];
        if kind == 4 && len >= 7 {
            iface = Some((b[i + 2], b[i + 5], b[i + 6]));
        }
        // A video control interface's extension unit.
        if kind == 0x24 && len >= 24 && b[i + 2] == 6 && iface.is_some_and(|f| f.0 == number && f.1 == 0x0e && f.2 == 1) {
            let unit = b[i + 3];
            let pins = b[i + 21] as usize;
            let size = b.get(i + 22 + pins).copied().unwrap_or(0) as usize;
            let bitmap = b.get(i + 23 + pins..i + 23 + pins + size).unwrap_or(&[]);
            let selectors = (0..bitmap.len() * 8).filter(|k| bitmap[k / 8] & (1 << (k % 8)) != 0).map(|k| (k + 1) as u8).collect();
            out.push((unit, selectors));
        }
        i += len;
    }
    out
}

/// A frame's brightness: the mean of its middle half (where a face is).
fn brightness(g: &Gray) -> f32 {
    g.mean_in([g.w as f32 / 4.0, g.h as f32 / 4.0, g.w as f32 / 2.0, g.h as f32 / 2.0])
}

/// The brightest of the next few frames (an emitter that flashes every
/// other frame lights one of them).
fn lit(frames: &mut dyn Frames, n: usize) -> f32 {
    (0..n).filter_map(|_| frames.frame(Duration::from_millis(500)).ok()).map(|f| brightness(&f)).fold(0.0, f32::max)
}

/// Values to try for a control `n` long whose current value is `cur`
/// and range tops at `max`: its maximum, and the current with each byte
/// in turn raised to 1, 2 and 3 (as vendor controls take a mode and an
/// intensity).
fn candidates(cur: &[u8], max: Option<&[u8]>, def: Option<&[u8]>) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut add = |v: Vec<u8>| {
        if v != cur && !out.contains(&v) {
            out.push(v);
        }
    };
    if let Some(m) = max {
        add(m.to_vec());
    }
    if let Some(d) = def {
        add(d.to_vec());
    }
    for i in 0..cur.len().min(9) {
        for v in 1..=3u8 {
            let mut c = cur.to_vec();
            c[i] = v;
            add(c);
        }
    }
    out
}

/// Find the control that lights the emitter of the camera streaming in
/// `frames` (its node `fd`, its interface `interface`): `None` if it's
/// lit already or nothing tried lit it. Each control tried is put back.
/// `say` hears what's being tried.
pub fn find(fd: &OwnedFd, frames: &mut dyn Frames, interface: &Path, camera: &str, say: &dyn Fn(&str)) -> Option<Emitter> {
    // Let the exposure settle, then what dark is.
    for _ in 0..8 {
        let _ = frames.frame(Duration::from_millis(500));
    }
    let base = lit(frames, 4);
    say(&format!("dark at {base:.0}"));
    for (unit, selectors) in units(interface) {
        for selector in selectors {
            let Some(n) = len(fd, unit, selector) else { continue };
            // Only what can be both read and set.
            let info = get(fd, unit, selector, GET_INFO, 1).map(|b| b[0]).unwrap_or(0);
            if info & 0x03 != 0x03 {
                continue;
            }
            let Some(cur) = get(fd, unit, selector, GET_CUR, n) else { continue };
            let (max, def) = (get(fd, unit, selector, GET_MAX, n), get(fd, unit, selector, GET_DEF, n));
            for value in candidates(&cur, max.as_deref(), def.as_deref()) {
                if set(fd, unit, selector, &value).is_err() {
                    continue;
                }
                let now = lit(frames, 6);
                say(&format!("unit {unit} control {selector} {value:02x?}: {now:.0}"));
                // The flood: well over the dark frames.
                if now > base + 25.0 && now > base * 1.5 {
                    return Some(Emitter { camera: camera.to_owned(), unit, selector, value });
                }
            }
            let _ = set(fd, unit, selector, &cur);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_to_try() {
        let c = candidates(&[0, 0, 0], Some(&[3, 3, 3]), None);
        assert_eq!(c[0], vec![3, 3, 3]);
        assert!(c.contains(&vec![1, 0, 0]) && c.contains(&vec![0, 0, 2]));
        assert!(!c.contains(&vec![0, 0, 0]));
    }

    /// The ThinkPad's camera's units, from its descriptors (read-only):
    /// `cargo test -p olo-face units -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn this_machines_units() {
        for dev in crate::camera::infrared() {
            let iface = crate::camera::usb_interface(&dev).unwrap();
            println!("{} {}: {:?}", dev.display(), iface.display(), units(&iface));
        }
    }
}
