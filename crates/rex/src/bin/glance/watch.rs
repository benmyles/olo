//! Wait for a file to change or a missing socket to appear. A rename over a
//! watched file removes its old inode. The caller rearms before reading again.

use std::{
    collections::HashMap,
    ffi::{CString, OsString},
    io::{self, Read},
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, FromRawFd},
    },
    path::PathBuf,
};
use tokio::io::unix::AsyncFd;

pub struct Paths {
    fd: AsyncFd<std::fs::File>,
    names: HashMap<i32, Vec<Option<OsString>>>,
}

impl Paths {
    pub fn new(paths: &[PathBuf]) -> io::Result<Self> {
        // SAFETY: success returns a new owned file descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut names = HashMap::<_, Vec<_>>::new();
        for path in paths {
            let mut target = path.as_path();
            loop {
                let name = path.strip_prefix(target).unwrap().components().next().map(|c| c.as_os_str().to_owned());
                let mask = libc::IN_DELETE_SELF
                    | libc::IN_MOVE_SELF
                    | libc::IN_MASK_ADD
                    | if name.is_some() { libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_ATTRIB } else { libc::IN_ATTRIB | libc::IN_CLOSE_WRITE };
                let c_path = CString::new(target.as_os_str().as_bytes())?;
                // SAFETY: the descriptor and C string remain valid for the call.
                let wd = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), c_path.as_ptr(), mask) };
                if wd >= 0 {
                    // A missing component may have appeared while we walked up.
                    if let Some(child) = &name
                        && target.join(child).exists()
                    {
                        target = path;
                        continue;
                    }
                    names.entry(wd).or_default().push(name);
                    break;
                }
                let e = io::Error::last_os_error();
                if !matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) {
                    return Err(e);
                }
                let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) else { return Err(e) };
                target = parent;
            }
        }
        // SAFETY: File owns this descriptor and it stays open until AsyncFd drops it.
        let fd = unsafe { AsyncFd::register_with_interest(fd, tokio::io::Interest::READABLE) }?;
        Ok(Self { fd, names })
    }

    pub async fn changed(&self) -> io::Result<()> {
        let mut buf = [0u8; 8192];
        loop {
            let mut ready = self.fd.readable().await?;
            let Ok(n) = ready.try_io(|fd| fd.get_ref().read(&mut buf)) else { continue };
            let mut bytes = &buf[..n?];
            while bytes.len() >= size_of::<libc::inotify_event>() {
                // SAFETY: we checked the size; a byte buffer need not be aligned.
                let ev = unsafe { bytes.as_ptr().cast::<libc::inotify_event>().read_unaligned() };
                let header = size_of::<libc::inotify_event>();
                let name = &bytes[header..header + ev.len as usize];
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                if ev.mask & (libc::IN_Q_OVERFLOW | libc::IN_IGNORED | libc::IN_MOVE_SELF | libc::IN_DELETE_SELF) != 0
                    || self.names.get(&ev.wd).is_some_and(|names| names.iter().any(|n| n.as_ref().is_none_or(|n| n.as_bytes() == &name[..end])))
                {
                    return Ok(());
                }
                bytes = &bytes[header + ev.len as usize..];
            }
        }
    }
}
