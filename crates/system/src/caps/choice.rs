//! One empty administrator-created marker per authorized account.
use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

pub const CHOICE_DIR: &str = "etc/olo/compositor-first.d";

/// The switch for the whole computer that came before the choice for each
/// account (2026-11-20, for a day): any account's Olo login was handed the
/// capability where this file was. Nothing of Olo's reads it now; a login
/// screen from that day still would, so Doctor offers to remove it.
pub const OLD_SWITCH: &str = "etc/olo/compositor-first";

fn trusted(file: &File, owner: u32, directory: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    if (if directory { meta.is_dir() } else { meta.is_file() }) && meta.uid() == owner && meta.mode() & 0o022 == 0 {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsafe compositor scheduling grant path"))
    }
}

fn open_at(dir: &File, name: &CStr, flags: libc::c_int) -> io::Result<File> {
    // SAFETY: a borrowed directory fd and NUL-terminated single component.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC | libc::O_NOFOLLOW, 0o644 as libc::mode_t) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { File::from_raw_fd(fd) }) }
}

fn grant_dir(root: &Path, owner: u32, create: bool) -> io::Result<File> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY;
    let mut parent = OpenOptions::new().read(true).custom_flags(flags | libc::O_NOFOLLOW).open(root)?;
    trusted(&parent, owner, true)?;
    for part in [c"etc", c"olo", c"compositor-first.d"] {
        let child = match open_at(&parent, part, flags) {
            Err(e) if create && part != c"etc" && e.kind() == io::ErrorKind::NotFound => {
                // SAFETY: mkdirat creates one entry relative to the checked directory.
                if unsafe { libc::mkdirat(parent.as_raw_fd(), part.as_ptr(), 0o755) } != 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::AlreadyExists {
                        return Err(e);
                    }
                } else {
                    parent.sync_all()?;
                }
                open_at(&parent, part, flags)?
            }
            other => other?,
        };
        trusted(&child, owner, true)?;
        parent = child;
    }
    Ok(parent)
}

fn marker(uid: u32) -> CString {
    CString::new(uid.to_string()).expect("decimal UID has no NUL")
}

pub fn chosen(root: &Path, owner: u32, uid: u32) -> bool {
    let Ok(dir) = grant_dir(root, owner, false) else { return false };
    open_at(&dir, &marker(uid), libc::O_PATH).and_then(|f| trusted(&f, owner, false)).is_ok()
}

/// Apply or undo the named account's grant. The file is complete at creation.
pub fn set_choice(root: &Path, owner: u32, uid: u32, enabled: bool) -> io::Result<()> {
    let dir = match grant_dir(root, owner, enabled) {
        Err(e) if !enabled && e.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => other?,
    };
    let name = marker(uid);
    if enabled {
        match open_at(&dir, &name, libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY) {
            Ok(file) => {
                trusted(&file, owner, false)?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => trusted(&open_at(&dir, &name, libc::O_PATH)?, owner, false)?,
            Err(e) => return Err(e),
        }
    } else {
        match open_at(&dir, &name, libc::O_PATH) {
            Ok(file) => trusted(&file, owner, false)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        // SAFETY: unlink one entry in the pinned, checked grant directory.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::NotFound {
                return Err(e);
            }
        }
    }
    dir.sync_all()
}
