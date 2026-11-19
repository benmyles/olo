//! A sleeping discrete GPU is tuned when a client opens a device node.
//! Watch opens on individual nodes, and only topology changes on directories.
use std::{
    collections::HashMap,
    ffi::CString,
    fs::File,
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq)]
enum Watch {
    Device,
    Nodes,
    Run,
    Fans,
}

#[derive(Clone, Copy)]
pub enum Event {
    Device,
    Topology,
    Cooling,
}

fn add(fd: &File, watches: &mut HashMap<i32, Watch>, path: &Path, mask: u32, kind: Watch) {
    let Ok(name) = CString::new(path.as_os_str().as_bytes()) else { return };
    // SAFETY: the file owns the inotify fd, and name is NUL terminated.
    let wd = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), name.as_ptr(), mask) };
    if wd >= 0 {
        watches.insert(wd, kind);
    }
}

fn devices(fd: &File, watches: &mut HashMap<i32, Watch>) {
    for g in olo_system::gpus::list().into_iter().filter(|g| g.vendor == 0x10de) {
        for path in [g.card, g.render].into_iter().flatten() {
            add(fd, watches, &path, libc::IN_OPEN, Watch::Device);
        }
    }
    for entry in std::fs::read_dir("/dev").into_iter().flatten().flatten() {
        if entry.file_name().to_str().and_then(|n| n.strip_prefix("nvidia")).is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())) {
            add(fd, watches, &entry.path(), libc::IN_OPEN, Watch::Device);
        }
    }
}

pub fn watch(send: impl Fn(Event) -> bool + Send + 'static) -> std::io::Result<()> {
    // SAFETY: a successful inotify_init1 returns a fresh owned fd.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut fd = unsafe { File::from_raw_fd(fd) };
    let mut watches = HashMap::new();
    let node_mask = libc::IN_CREATE | libc::IN_MOVED_TO;
    for path in ["/dev", "/dev/dri"] {
        add(&fd, &mut watches, Path::new(path), node_mask, Watch::Nodes);
    }
    add(&fd, &mut watches, Path::new("/run"), libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE, Watch::Run);
    let fans = PathBuf::from("/run/olo-fans");
    let fan_mask = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_DELETE;
    add(&fd, &mut watches, &fans, fan_mask, Watch::Fans);
    devices(&fd, &mut watches);
    std::thread::Builder::new().name("power-device-events".into()).spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let n = match fd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let mut i = 0;
            let (mut opened, mut scan, mut cooling) = (false, false, false);
            while i + size_of::<libc::inotify_event>() <= n {
                // SAFETY: the kernel supplies complete records; the byte buffer need not be aligned.
                let ev = unsafe { std::ptr::read_unaligned(buf[i..].as_ptr().cast::<libc::inotify_event>()) };
                i += size_of::<libc::inotify_event>();
                let Some(name) = buf.get(i..i + ev.len as usize) else { break };
                i += ev.len as usize;
                let name = name.split(|&b| b == 0).next().unwrap_or_default();
                if ev.mask & libc::IN_IGNORED != 0 {
                    watches.remove(&ev.wd);
                    continue;
                }
                if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                    scan = true;
                    cooling = true;
                    opened = true;
                }
                match watches.get(&ev.wd) {
                    Some(Watch::Device) => opened = true,
                    Some(Watch::Nodes) if name == b"dri" || name.starts_with(b"nvidia") || name.starts_with(b"card") || name.starts_with(b"renderD") => scan = true,
                    Some(Watch::Run) if name == b"olo-fans" => {
                        add(&fd, &mut watches, &fans, fan_mask, Watch::Fans);
                        cooling = true;
                    }
                    Some(Watch::Fans) if name == b"pressure" => cooling = true,
                    _ => {}
                }
            }
            if scan {
                // The first graphics device can arrive after the service started.
                add(&fd, &mut watches, Path::new("/dev/dri"), node_mask, Watch::Nodes);
                devices(&fd, &mut watches);
                if !send(Event::Topology) {
                    break;
                }
            }
            if opened && !send(Event::Device) || cooling && !send(Event::Cooling) {
                break;
            }
        }
    })?;
    Ok(())
}
