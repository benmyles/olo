//! A touchpad's contacts as the pad reports them (each one's place, size
//! and pressure), for whoever asks (`EventKind::TouchContacts`:
//! Preferences' palm trainer, which shows them live and learns from
//! them).
//!
//! libinput doesn't give them out (only pointer motion and gestures), and
//! a resting palm, which the touch filter hides, gives nothing at all.
//! So libinput's way to the devices ([`Interface`], through the session
//! as Smithay's own) keeps a copy of each filtered pad's descriptor
//! (`touch_filter::DEVICES`) for app_server's loop. The loop wakes on that
//! copy when the pad reports, as it does for libinput anyway (no wakeup
//! of its own). Only while someone asks, it reads every slot's state
//! with `EVIOCGMTSLOTS`, the kernel's latest, which leaves the events to
//! libinput, and says it on when it changed.

use std::{
    collections::HashMap,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    path::{Path, PathBuf},
};

use olo_protocol::touchpad::{PadAxes, TouchContact};
#[cfg(feature = "udev")]
use smithay::backend::session::{AsErrno, Session, libseat::LibSeatSession};

/// A pad opened (its path and a copy of libinput's descriptor), or closed.
pub enum Pad {
    Opened(PathBuf, OwnedFd),
    Closed(PathBuf),
}

/// libinput's way to the devices: the session's, telling app_server's
/// loop of each filtered pad.
#[cfg(feature = "udev")]
pub struct Interface {
    session: LibSeatSession,
    pads: smithay::reexports::calloop::channel::Sender<Pad>,
    /// libinput's descriptors of the pads, by number, to know which one
    /// closes.
    open: HashMap<RawFd, PathBuf>,
}

#[cfg(feature = "udev")]
impl Interface {
    pub fn new(session: LibSeatSession, pads: smithay::reexports::calloop::channel::Sender<Pad>) -> Interface {
        Interface { session, pads, open: HashMap::new() }
    }
}

#[cfg(feature = "udev")]
impl smithay::reexports::input::LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        use rustix::fs::OFlags;
        let fd = self.session.open(path, OFlags::from_bits_truncate(flags as u32)).map_err(|e| e.as_errno().unwrap_or(1))?;
        if name(&fd).is_some_and(|n| crate::touch_filter::DEVICES.iter().any(|d| n.contains(d)))
            && let Ok(copy) = fd.try_clone()
        {
            self.open.insert(fd.as_raw_fd(), path.to_owned());
            let _ = self.pads.send(Pad::Opened(path.to_owned(), copy));
        }
        Ok(fd)
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        if let Some(path) = self.open.remove(&fd.as_raw_fd()) {
            let _ = self.pads.send(Pad::Closed(path));
        }
        let _ = self.session.close(fd);
    }
}

const ABS_MT_SLOT: u32 = 0x2f;
const ABS_MT_TOUCH_MAJOR: u32 = 0x30;
const ABS_MT_TOUCH_MINOR: u32 = 0x31;
const ABS_MT_POSITION_X: u32 = 0x35;
const ABS_MT_POSITION_Y: u32 = 0x36;
const ABS_MT_TRACKING_ID: u32 = 0x39;
const ABS_MT_PRESSURE: u32 = 0x3a;

/// `_IOC(_IOC_READ, 'E', nr, size)`.
const fn eviocg(nr: u32, size: usize) -> libc::c_ulong {
    ((2 << 30) | ((size as u32) << 16) | ((b'E' as u32) << 8) | nr) as libc::c_ulong
}

/// The device's name (`EVIOCGNAME`).
fn name(fd: &OwnedFd) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the kernel writes at most `buf.len()` bytes.
    let n = unsafe { libc::ioctl(fd.as_raw_fd(), eviocg(0x06, buf.len()), buf.as_mut_ptr()) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..(n as usize).saturating_sub(1)]).into_owned())
}

/// An axis's minimum, maximum and resolution (`EVIOCGABS`).
fn abs(fd: &OwnedFd, code: u32) -> Option<(i32, i32, i32)> {
    let mut info = [0i32; 6];
    // SAFETY: `input_absinfo` is six i32s.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), eviocg(0x40 + code, std::mem::size_of_val(&info)), info.as_mut_ptr()) };
    (r >= 0).then_some((info[1], info[2], info[5]))
}

/// A pad being watched: its axes, how many slots it has, and what was
/// last said.
pub struct Watch {
    pub axes: PadAxes,
    slots: usize,
    last: Option<Vec<TouchContact>>,
}

impl Watch {
    pub fn new(fd: &OwnedFd) -> Option<Watch> {
        let (x_min, x_max, x_res) = abs(fd, ABS_MT_POSITION_X)?;
        let (y_min, y_max, y_res) = abs(fd, ABS_MT_POSITION_Y)?;
        let (_, size_max, _) = abs(fd, ABS_MT_TOUCH_MAJOR)?;
        let (_, pressure_max, _) = abs(fd, ABS_MT_PRESSURE).unwrap_or((0, 0, 0));
        let (_, slot_max, _) = abs(fd, ABS_MT_SLOT)?;
        let axes = PadAxes { name: name(fd).unwrap_or_default(), x_min, x_max, y_min, y_max, x_res, y_res, size_max, pressure_max };
        Some(Watch { axes, slots: (slot_max + 1).clamp(1, 32) as usize, last: None })
    }

    /// What's on the pad now, if it changed since last said.
    pub fn changed(&mut self, fd: &OwnedFd) -> Option<Vec<TouchContact>> {
        let now = self.read(fd)?;
        if self.last.as_ref() == Some(&now) {
            return None;
        }
        self.last = Some(now.clone());
        Some(now)
    }

    /// Every slot's values for `code` (`EVIOCGMTSLOTS`).
    fn slots(&self, fd: &OwnedFd, code: u32) -> Option<Vec<i32>> {
        let mut buf = vec![0i32; 1 + self.slots];
        buf[0] = code as i32;
        // SAFETY: the kernel reads the code and writes at most `slots`
        // values after it, within the buffer's size given.
        let r = unsafe { libc::ioctl(fd.as_raw_fd(), eviocg(0x0a, buf.len() * 4), buf.as_mut_ptr()) };
        (r >= 0).then(|| buf.split_off(1))
    }

    fn read(&self, fd: &OwnedFd) -> Option<Vec<TouchContact>> {
        let ids = self.slots(fd, ABS_MT_TRACKING_ID)?;
        if ids.iter().all(|id| *id < 0) {
            return Some(Vec::new());
        }
        let [x, y, major, minor, pressure] = [ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TOUCH_MAJOR, ABS_MT_TOUCH_MINOR, ABS_MT_PRESSURE].map(|c| self.slots(fd, c).unwrap_or_else(|| vec![0; self.slots]));
        Some(
            (0..self.slots)
                .filter(|&i| ids[i] >= 0)
                .map(|i| TouchContact { id: ids[i], x: x[i], y: y[i], major: major[i], minor: minor[i], pressure: pressure[i] })
                .collect(),
        )
    }
}

/// Microseconds on the monotonic clock (as evdev's and libinput's).
pub fn now_us() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: plain clock_gettime into a timespec of ours.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1000
}
