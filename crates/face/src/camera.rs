//! The infrared camera, over V4L2 (no libv4l): found by its formats (a
//! camera that gives only grey is an infrared one: the ThinkPad's
//! "Integrated IR Camera", 640×360 GREY at 15 frames a second), opened,
//! streamed from memory-mapped buffers, and its frames copied out as
//! [`Gray`]s. Tests give a folder of pictures instead ([`FakeCamera`]).

use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::model::Gray;

const fn iowr(t: u8, nr: u8, size: usize) -> libc::c_ulong {
    ((3u64 << 30) | ((size as u64) << 16) | ((t as u64) << 8) | nr as u64) as libc::c_ulong
}
const fn iow(t: u8, nr: u8, size: usize) -> libc::c_ulong {
    ((1u64 << 30) | ((size as u64) << 16) | ((t as u64) << 8) | nr as u64) as libc::c_ulong
}

const VIDIOC_ENUM_FMT: libc::c_ulong = iowr(b'V', 2, 64);
const VIDIOC_S_FMT: libc::c_ulong = iowr(b'V', 5, 208);
const VIDIOC_REQBUFS: libc::c_ulong = iowr(b'V', 8, 20);
const VIDIOC_QUERYBUF: libc::c_ulong = iowr(b'V', 9, 88);
const VIDIOC_QBUF: libc::c_ulong = iowr(b'V', 15, 88);
const VIDIOC_DQBUF: libc::c_ulong = iowr(b'V', 17, 88);
const VIDIOC_STREAMON: libc::c_ulong = iow(b'V', 18, 4);
const VIDIOC_STREAMOFF: libc::c_ulong = iow(b'V', 19, 4);
const VIDIOC_ENUM_FRAMESIZES: libc::c_ulong = iowr(b'V', 74, 44);
const CAPTURE: u32 = 1;
const MMAP: u32 = 1;
const GREY: u32 = u32::from_le_bytes(*b"GREY");

fn ioctl(fd: &OwnedFd, req: libc::c_ulong, buf: &mut [u8]) -> io::Result<()> {
    loop {
        // SAFETY: `buf` is the size the request's number says.
        let r = unsafe { libc::ioctl(fd.as_raw_fd(), req, buf.as_mut_ptr()) };
        if r >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

fn open_node(path: &Path) -> io::Result<OwnedFd> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a NUL-terminated path; the fd owned at once.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The pixel formats a node captures in.
fn formats(fd: &OwnedFd) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..16u32 {
        let mut b = [0u8; 64];
        b[0..4].copy_from_slice(&i.to_ne_bytes());
        b[4..8].copy_from_slice(&CAPTURE.to_ne_bytes());
        if ioctl(fd, VIDIOC_ENUM_FMT, &mut b).is_err() {
            break;
        }
        out.push(u32::from_ne_bytes(b[44..48].try_into().unwrap()));
    }
    out
}

/// The infrared cameras: nodes that capture grey and nothing else.
pub fn infrared() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else { return Vec::new() };
    let mut out: Vec<PathBuf> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let dev = PathBuf::from("/dev").join(&name);
            let fd = open_node(&dev).ok()?;
            let f = formats(&fd);
            (!f.is_empty() && f.iter().all(|x| *x == GREY)).then_some(dev)
        })
        .collect();
    out.sort();
    out
}

/// The USB interface a node is (its sysfs directory: "…/3-4:1.2"), for
/// its extension units.
pub fn usb_interface(dev: &Path) -> Option<PathBuf> {
    let name = dev.file_name()?;
    std::fs::canonicalize(Path::new("/sys/class/video4linux").join(name).join("device")).ok()
}

/// Frames as they come.
pub trait Frames {
    fn frame(&mut self, timeout: Duration) -> io::Result<Gray>;
}

/// A V4L2 camera streaming.
pub struct Camera {
    fd: OwnedFd,
    w: usize,
    h: usize,
    stride: usize,
    bufs: Vec<(*mut libc::c_void, usize)>,
}

// SAFETY: the mappings belong to the camera alone.
unsafe impl Send for Camera {}

impl Camera {
    /// Open `dev` in grey at its first size and start it streaming.
    pub fn open(dev: &Path) -> io::Result<Camera> {
        let fd = open_node(dev)?;
        // Its first size for grey.
        let mut fs = [0u8; 44];
        fs[4..8].copy_from_slice(&GREY.to_ne_bytes());
        ioctl(&fd, VIDIOC_ENUM_FRAMESIZES, &mut fs)?;
        let (w, h) = (u32::from_ne_bytes(fs[12..16].try_into().unwrap()), u32::from_ne_bytes(fs[16..20].try_into().unwrap()));
        let mut f = [0u8; 208];
        f[0..4].copy_from_slice(&CAPTURE.to_ne_bytes());
        f[8..12].copy_from_slice(&w.to_ne_bytes());
        f[12..16].copy_from_slice(&h.to_ne_bytes());
        f[16..20].copy_from_slice(&GREY.to_ne_bytes());
        ioctl(&fd, VIDIOC_S_FMT, &mut f)?;
        let (w, h) = (u32::from_ne_bytes(f[8..12].try_into().unwrap()) as usize, u32::from_ne_bytes(f[12..16].try_into().unwrap()) as usize);
        let stride = (u32::from_ne_bytes(f[24..28].try_into().unwrap()) as usize).max(w);
        let mut rb = [0u8; 20];
        rb[0..4].copy_from_slice(&4u32.to_ne_bytes());
        rb[4..8].copy_from_slice(&CAPTURE.to_ne_bytes());
        rb[8..12].copy_from_slice(&MMAP.to_ne_bytes());
        ioctl(&fd, VIDIOC_REQBUFS, &mut rb)?;
        let n = u32::from_ne_bytes(rb[0..4].try_into().unwrap());
        let mut cam = Camera { fd, w, h, stride, bufs: Vec::new() };
        for i in 0..n {
            let mut b = buffer(i);
            ioctl(&cam.fd, VIDIOC_QUERYBUF, &mut b)?;
            let offset = u32::from_ne_bytes(b[64..68].try_into().unwrap()) as libc::off_t;
            let len = u32::from_ne_bytes(b[72..76].try_into().unwrap()) as usize;
            // SAFETY: the driver's buffer, at the offset it gave.
            let p = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, cam.fd.as_raw_fd(), offset) };
            if p == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            cam.bufs.push((p, len));
            ioctl(&cam.fd, VIDIOC_QBUF, &mut b)?;
        }
        let mut t = CAPTURE.to_ne_bytes();
        ioctl(&cam.fd, VIDIOC_STREAMON, &mut t)?;
        Ok(cam)
    }

    /// The node's file descriptor (for its extension units' controls).
    pub fn fd(&self) -> &OwnedFd {
        &self.fd
    }
}

fn buffer(index: u32) -> [u8; 88] {
    let mut b = [0u8; 88];
    b[0..4].copy_from_slice(&index.to_ne_bytes());
    b[4..8].copy_from_slice(&CAPTURE.to_ne_bytes());
    b[60..64].copy_from_slice(&MMAP.to_ne_bytes());
    b
}

impl Frames for Camera {
    fn frame(&mut self, timeout: Duration) -> io::Result<Gray> {
        let mut pfd = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd.
        let r = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
        if r == 0 {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut b = buffer(0);
        ioctl(&self.fd, VIDIOC_DQBUF, &mut b)?;
        let i = u32::from_ne_bytes(b[0..4].try_into().unwrap()) as usize;
        let used = u32::from_ne_bytes(b[8..12].try_into().unwrap()) as usize;
        let (p, len) = self.bufs[i];
        // SAFETY: the buffer dequeued is ours till it's queued again.
        let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, len.min(used.max(self.stride * self.h))) };
        let mut data = Vec::with_capacity(self.w * self.h);
        for y in 0..self.h {
            let row = &bytes[(y * self.stride).min(bytes.len())..((y * self.stride) + self.w).min(bytes.len())];
            data.extend_from_slice(row);
            data.resize((y + 1) * self.w, 0);
        }
        ioctl(&self.fd, VIDIOC_QBUF, &mut b)?;
        Ok(Gray { w: self.w, h: self.h, data })
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let mut t = CAPTURE.to_ne_bytes();
        let _ = ioctl(&self.fd, VIDIOC_STREAMOFF, &mut t);
        for (p, len) in self.bufs.drain(..) {
            // SAFETY: mapped in `open`.
            unsafe { libc::munmap(p, len) };
        }
    }
}

/// Frames from a folder of pictures (`OLO_FACE_CAMERA`), in name order,
/// round and round, at the camera's pace.
pub struct FakeCamera {
    frames: Vec<Gray>,
    next: usize,
}

impl FakeCamera {
    pub fn open(dir: &Path) -> io::Result<FakeCamera> {
        let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg")).collect();
        names.sort();
        let frames: Vec<Gray> = names.iter().filter_map(|p| image::open(p).ok()).map(|i| Gray::from_image(&i)).collect();
        if frames.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no pictures"));
        }
        Ok(FakeCamera { frames, next: 0 })
    }
}

impl Frames for FakeCamera {
    fn frame(&mut self, _: Duration) -> io::Result<Gray> {
        std::thread::sleep(Duration::from_millis(66));
        let f = self.frames[self.next % self.frames.len()].clone();
        self.next += 1;
        Ok(f)
    }
}

/// The camera to look through: `fake`'s pictures (tests: the service
/// is given a folder only by its command line, never its environment),
/// else the first infrared camera.
pub fn open(fake: Option<&Path>) -> io::Result<Box<dyn Frames + Send>> {
    if let Some(dir) = fake {
        return Ok(Box::new(FakeCamera::open(dir)?));
    }
    let dev = infrared().into_iter().next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no infrared camera"))?;
    Ok(Box::new(Camera::open(&dev)?))
}
