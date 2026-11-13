//! Linux Landlock write confinement for developer processes. Construct rules on
//! the worker before fork; pre_exec only performs async-signal-safe syscalls.
//! Reading the host and networking remain available. This is not a container.
use anyhow::{Result, ensure};
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
};
const READ: u64 = (1 << 0) | (1 << 2) | (1 << 3);
const ALL: u64 = (1 << 15) - 1; // ABI 3, including REFER and TRUNCATE.
#[repr(C)]
struct Ruleset {
    fs: u64,
}
#[repr(C, packed)]
struct Beneath {
    access: u64,
    fd: i32,
}
pub struct Sandbox(OwnedFd);
impl Sandbox {
    pub fn workspace(root: &Path, scratch: &Path) -> Result<Self> {
        let version = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0, 1) };
        ensure!(version >= 3, "workspace sandbox requires Landlock ABI 3; choose unrestricted on an unsupported kernel");
        let attr = Ruleset { fs: ALL };
        let fd = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, &attr, std::mem::size_of::<Ruleset>(), 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let s = Self(unsafe { OwnedFd::from_raw_fd(fd as i32) });
        s.allow(Path::new("/"), READ)?;
        s.allow(root, ALL)?;
        s.allow(scratch, ALL)?;
        s.allow(Path::new("/dev/null"), 1 << 1)?;
        Ok(s)
    }
    fn allow(&self, path: &Path, access: u64) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let attr = Beneath { access, fd: fd.as_raw_fd() };
        if unsafe { libc::syscall(libc::SYS_landlock_add_rule, self.0.as_raw_fd(), 1, &attr, 0) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    pub fn enforce(&self) -> std::io::Result<()> {
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, self.0.as_raw_fd(), 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
