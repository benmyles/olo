//! A systemd path watch is not recursive. Follow the stock folders
//! themselves (including a new kernel whose extra/ does not exist
//! yet), asleep in inotify between changes. Ignore our index writes.
use crate::{
    index::Depmod,
    manage,
    system::{self, System},
};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fs::File,
    io::{self, Read},
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, FromRawFd},
    },
    path::{Path, PathBuf},
};

pub const SERVICE: &str = include_str!("../olo-nvidia.service");
pub const WATCH_SERVICE: &str = include_str!("../olo-nvidia-watch.service");

struct Watches {
    fd: File,
    paths: BTreeMap<i32, PathBuf>,
    base: PathBuf,
}
impl Watches {
    fn new(base: PathBuf) -> Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self { fd: unsafe { File::from_raw_fd(fd) }, paths: BTreeMap::new(), base })
    }
    fn add(&mut self, path: &Path) -> Result<()> {
        let c = CString::new(path.as_os_str().as_bytes())?;
        let mask = libc::IN_ONLYDIR | libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_CLOSE_WRITE;
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), mask) };
        if wd < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err(e.into());
        }
        self.paths.insert(wd, path.to_owned());
        Ok(())
    }
    fn refresh(&mut self, s: &System) -> Result<()> {
        let mut want = BTreeSet::from([self.base.clone()]);
        self.add(&self.base.clone())?;
        for folder in ["boot", "usr/src/kernels"] {
            let path = s.root.join(folder);
            self.add(&path)?;
            want.insert(path);
        }
        for r in s.releases()? {
            let k = s.kernel(&r);
            for path in [&k, k.join("extra").as_path(), k.join("extra/nvidia").as_path()] {
                self.add(path)?;
                want.insert(path.to_owned());
            }
        }
        self.paths.retain(|wd, p| {
            if want.contains(p) {
                true
            } else {
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), *wd) };
                false
            }
        });
        Ok(())
    }
    fn relevant(&mut self, bytes: &[u8]) -> bool {
        let mut offset = 0;
        let mut changed = false;
        while offset + 16 <= bytes.len() {
            let event = unsafe { bytes.as_ptr().add(offset).cast::<libc::inotify_event>().read_unaligned() };
            let Some(raw) = bytes.get(offset + 16..offset + 16 + event.len as usize) else { return true };
            let name = std::str::from_utf8(raw.split(|b| *b == 0).next().unwrap()).unwrap_or("");
            if event.mask & libc::IN_Q_OVERFLOW != 0 {
                changed = true
            }
            if event.mask & libc::IN_IGNORED != 0 {
                self.paths.remove(&event.wd);
            } else if let Some(path) = self.paths.get(&event.wd) {
                changed |= if name.is_empty() {
                    true
                } else if path.ends_with("boot") {
                    name.starts_with("initramfs-") && name.ends_with(".img")
                } else if path == &self.base || path.ends_with("usr/src/kernels") {
                    system::valid_release(name).is_ok()
                } else if path.ends_with("extra/nvidia") {
                    crate::modules::module_name(Path::new(name)).is_some()
                } else if path.ends_with("extra") {
                    name == "nvidia"
                } else {
                    name == "extra" || name == "build"
                };
            }
            offset += 16 + event.len as usize;
        }
        changed
    }
}

pub fn watch(s: &System) -> Result<()> {
    let mut w = Watches::new(s.root.join("lib/modules"))?;
    let mut buf = [0; 65536];
    loop {
        w.refresh(s)?;
        {
            let _guard = s.lock()?;
            if let Err(e) = manage::check(s, &Depmod, |r| manage::build(s, r, false, &Depmod)) {
                eprintln!("NVIDIA check: {e:#}");
            }
        }
        // Events that arrived during a build remain queued.
        loop {
            let n = w.fd.read(&mut buf).context("waiting for a change to installed NVIDIA modules")?;
            if w.relevant(&buf[..n]) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn event(w: &mut Watches) -> bool {
        let mut p = libc::pollfd { fd: w.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut p, 1, 1000) }, 1, "timed out waiting for inotify");
        let mut buf = [0; 8192];
        let n = w.fd.read(&mut buf).unwrap();
        w.relevant(&buf[..n])
    }
    #[test]
    fn follows_new_stock_folders_and_ignores_index_writes() {
        let tmp = crate::real_build::scratch();
        let s = System { root: tmp.path().into() };
        let base = s.root.join("lib/modules");
        fs::create_dir_all(&base).unwrap();
        let mut w = Watches::new(base).unwrap();
        w.refresh(&s).unwrap();
        let kernel = s.kernel("7.10");
        fs::create_dir(&kernel).unwrap();
        assert!(event(&mut w));
        w.refresh(&s).unwrap();
        fs::write(kernel.join("modules.dep"), "").unwrap();
        assert!(!event(&mut w));
        let stock = kernel.join(crate::modules::STOCK);
        fs::create_dir_all(&stock).unwrap();
        assert!(event(&mut w));
        w.refresh(&s).unwrap();
        fs::write(stock.join("nvidia.ko.xz"), "").unwrap();
        assert!(event(&mut w));
        fs::remove_file(stock.join("nvidia.ko.xz")).unwrap();
        assert!(event(&mut w));
    }
}
