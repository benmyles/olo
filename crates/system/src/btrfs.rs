//! Btrfs file attributes (`FS_NOCOW_FL`, `FS_IOC_FIEMAP`) read and
//! changed natively in Rust, without shelling out to `chattr` or
//! `filefrag`. Used by Olo's Kache app and Olo Doctor to keep kache's
//! SQLite `index.db` from fragmenting on copy-on-write Btrfs while
//! leaving `store/` copy-on-write so zero-copy reflinks keep working.

use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{self, Read as _, Write as _},
    os::{
        fd::AsRawFd as _,
        unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Linux `BTRFS_SUPER_MAGIC` (`/usr/include/linux/magic.h`).
const BTRFS_SUPER_MAGIC: libc::c_long = 0x9123_683E;

/// Linux `FS_NOCOW_FL` (`/usr/include/linux/fs.h`): disable copy-on-write
/// (and compression) for new extents on Btrfs. Must be set on an empty
/// file before data is written.
const FS_NOCOW_FL: libc::c_int = 0x0080_0000;

/// `_IOR('f', 1, long)` and `_IOW('f', 2, long)`: the ioctl numbers
/// encode `sizeof(long)`, though the kernel reads/writes an `int`.
const FS_IOC_GETFLAGS: libc::Ioctl = ((2usize << 30) | (std::mem::size_of::<libc::c_long>() << 16) | ((b'f' as usize) << 8) | 1) as libc::Ioctl;
const FS_IOC_SETFLAGS: libc::Ioctl = ((1usize << 30) | (std::mem::size_of::<libc::c_long>() << 16) | ((b'f' as usize) << 8) | 2) as libc::Ioctl;

/// `_IOWR('f', 11, struct fiemap)` (`struct fiemap` is 32 bytes).
const FS_IOC_FIEMAP: libc::Ioctl = ((3usize << 30) | (std::mem::size_of::<Fiemap>() << 16) | ((b'f' as usize) << 8) | 11) as libc::Ioctl;
const FIEMAP_FLAG_SYNC: u32 = 0x0000_0001;

#[repr(C)]
struct Fiemap {
    fm_start: u64,
    fm_length: u64,
    fm_flags: u32,
    fm_mapped_extents: u32,
    fm_extent_count: u32,
    fm_reserved: u32,
}

/// Minimum extra free space required beside the copied file so rewriting
/// a large database never fills the volume.
const HEADROOM_BYTES: u64 = 64 << 20;

/// Whether `path` resides on a Btrfs filesystem (`statfs` magic).
pub fn is_btrfs(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `st` is valid writable storage.
    unsafe { libc::statfs(c.as_ptr(), &mut st) == 0 && st.f_type == BTRFS_SUPER_MAGIC }
}

/// Whether `path` has the `FS_NOCOW_FL` (`+C`) inode attribute set.
pub fn is_nocow(path: &Path) -> io::Result<bool> {
    let f = File::open(path)?;
    let mut flags: libc::c_int = 0;
    // SAFETY: `FS_IOC_GETFLAGS` writes a `c_int` into `&mut flags`.
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((flags & FS_NOCOW_FL) != 0)
}

/// Number of physical extents `path` is split across (`FS_IOC_FIEMAP`
/// with `fm_extent_count = 0` asks the kernel to count every extent).
pub fn extents(path: &Path) -> io::Result<u64> {
    let f = File::open(path)?;
    let mut map = Fiemap { fm_start: 0, fm_length: u64::MAX, fm_flags: FIEMAP_FLAG_SYNC, fm_mapped_extents: 0, fm_extent_count: 0, fm_reserved: 0 };
    // SAFETY: `FS_IOC_FIEMAP` with `fm_extent_count = 0` only reads and
    // updates the 32-byte `Fiemap` header without accessing a trailing array.
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_FIEMAP, &mut map) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(map.fm_mapped_extents))
}

fn free_bytes(dir: &Path) -> io::Result<u64> {
    let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `st` is valid writable storage.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

fn set_nocow_flag(f: &File, nocow: bool) -> io::Result<()> {
    let mut flags: libc::c_int = 0;
    // SAFETY: `FS_IOC_GETFLAGS` writes a `c_int` into `&mut flags`.
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if nocow {
        flags |= FS_NOCOW_FL;
    } else {
        flags &= !FS_NOCOW_FL;
    }
    // SAFETY: `FS_IOC_SETFLAGS` reads a `c_int` from `&flags`.
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_SETFLAGS, &flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Verify the filesystem actually kept the requested flag (Btrfs
    // requires the file to be empty when toggling `+C`).
    let mut after: libc::c_int = 0;
    // SAFETY: `FS_IOC_GETFLAGS` writes a `c_int` into `&mut after`.
    if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_GETFLAGS, &mut after) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if ((after & FS_NOCOW_FL) != 0) != nocow {
        return Err(io::Error::other(if nocow { "filesystem did not keep the NOCOW attribute" } else { "filesystem did not clear the NOCOW attribute" }));
    }
    Ok(())
}

/// Removes the temporary file if rewriting fails or unwinds before rename.
struct TempGuard(Option<PathBuf>);

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Rewrite a regular file on Btrfs with `FS_NOCOW_FL` enabled (`nocow =
/// true`) or cleared (`nocow = false`), preserving its contents and mode.
///
/// The caller must ensure no process is writing to `path` meanwhile
/// (e.g. stop the owning service first). Copies via plain `read`/`write`
/// rather than `std::fs::copy` or `std::io::copy`, both of which use
/// `copy_file_range` / `FICLONE` and would fail or preserve CoW extents.
pub fn rewrite_nocow(path: &Path, nocow: bool, progress: &mut dyn FnMut(u64, u64)) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    if !is_btrfs(path) {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "not on a Btrfs filesystem"));
    }
    if is_nocow(path)? == nocow {
        return Ok(());
    }
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent folder"))?;
    let total = meta.len();
    let avail = free_bytes(dir)?;
    if avail < total.saturating_add(HEADROOM_BYTES) {
        return Err(io::Error::other(format!("not enough free space to rewrite {} (need about {} MB free)", path.display(), (total + HEADROOM_BYTES) >> 20)));
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mode = meta.permissions().mode() & 0o7777;
    // Pick a fresh sibling name (never clobbering an existing file).
    let (tmp_path, mut dst) = {
        let mut chosen = None;
        let pid = std::process::id();
        for n in 0u32..32 {
            let candidate = dir.join(format!(".{name}.olo-nocow-{pid}-{n}"));
            match OpenOptions::new().write(true).create_new(true).open(&candidate) {
                Ok(f) => {
                    chosen = Some((candidate, f));
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        chosen.ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "could not create temporary file"))?
    };
    let mut guard = TempGuard(Some(tmp_path.clone()));
    std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode))?;
    // Set or clear FS_NOCOW_FL on the empty file BEFORE writing a byte.
    set_nocow_flag(&dst, nocow)?;
    let mut src = File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut last_report = Instant::now();
    progress(0, total);
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        done += n as u64;
        if last_report.elapsed() >= Duration::from_millis(100) {
            progress(done, total);
            last_report = Instant::now();
        }
    }
    progress(done, total);
    dst.sync_all()?;
    drop(dst);
    drop(src);
    std::fs::rename(&tmp_path, path)?;
    guard.0 = None;
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Hold `dir/daemon.run.lock` exclusively if present (up to 5 s) so no
/// kache daemon can start while `index.db` is being rewritten.
fn lock_daemon_if_present(dir: &Path) -> io::Result<Option<File>> {
    let lock_path = dir.join("daemon.run.lock");
    if !lock_path.is_file() {
        return Ok(None);
    }
    let f = OpenOptions::new().read(true).write(true).open(&lock_path)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: `flock` on a valid open descriptor we own.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(f));
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) && err.raw_os_error() != Some(libc::EAGAIN) {
            return Err(err);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "kache's daemon is still holding its lock"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stops `user_service` (if given and currently running), rewrites `path`
/// with `FS_NOCOW_FL` set or cleared, and always starts `user_service`
/// again afterwards if it was running.
pub fn rewrite_file_nocow_with_service(path: &Path, nocow: bool, user_service: Option<&str>, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    if !is_btrfs(path) {
        return Err(format!("{} is not on a Btrfs filesystem", path.display()));
    }
    if is_nocow(path).map_err(|e| e.to_string())? == nocow {
        return Ok(());
    }
    let was_running = match user_service {
        Some(unit) => match crate::systemd::user_unit(unit) {
            Ok(u) if u.loaded && u.running() => {
                crate::systemd::user_job(unit, "StopUnit").map_err(|e| format!("could not stop {unit}: {e}"))?;
                true
            }
            _ => false,
        },
        None => false,
    };
    let result = (|| -> Result<(), String> {
        let _lock = path.parent().map(lock_daemon_if_present).transpose().map_err(|e| e.to_string())?.flatten();
        rewrite_nocow(path, nocow, progress).map_err(|e| e.to_string())
    })();
    if was_running && let Some(unit) = user_service {
        if let Err(e) = crate::systemd::user_job(unit, "StartUnit") {
            return match result {
                Ok(()) => Err(format!("rewrote {}, but could not restart {unit}: {e}", path.display())),
                Err(orig) => Err(format!("{orig} (and could not restart {unit}: {e})")),
            };
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nocow_round_trip_on_btrfs_or_graceful_skip() {
        let dir = PathBuf::from(format!("/var/tmp/olo-btrfs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("index.db");
        let payload: Vec<u8> = (0..(256 * 1024)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &payload).unwrap();
        if !is_btrfs(&file) {
            // Non-Btrfs test host: verify unsupported is returned cleanly.
            assert_eq!(rewrite_nocow(&file, true, &mut |_, _| {}).unwrap_err().kind(), io::ErrorKind::Unsupported);
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        assert!(!is_nocow(&file).unwrap(), "fresh file starts copy-on-write");
        let mut saw_progress = false;
        rewrite_nocow(&file, true, &mut |done, total| {
            assert!(done <= total);
            saw_progress = true;
        })
        .unwrap();
        assert!(saw_progress);
        assert!(is_nocow(&file).unwrap(), "FS_NOCOW_FL set after rewrite");
        assert_eq!(std::fs::read(&file).unwrap(), payload, "contents preserved byte for byte");
        assert!(extents(&file).unwrap() >= 1, "FIEMAP reports extent count");
        // Idempotent when already NOCOW.
        rewrite_nocow(&file, true, &mut |_, _| {}).unwrap();
        assert!(is_nocow(&file).unwrap());
        // Restoring copy-on-write clears FS_NOCOW_FL and keeps contents.
        rewrite_nocow(&file, false, &mut |_, _| {}).unwrap();
        assert!(!is_nocow(&file).unwrap());
        assert_eq!(std::fs::read(&file).unwrap(), payload);
        // Symlinks are refused.
        let link = dir.join("link.db");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(rewrite_nocow(&link, true, &mut |_, _| {}).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
