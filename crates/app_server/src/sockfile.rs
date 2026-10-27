//! Socket files we made, removed on exit only while they're still ours.
//! Removing by name alone is dangerous: if the runtime directory was
//! cleared (the user logged out) and a new session took the same name, a
//! leftover app_server exiting would delete the live session's sockets.

use std::{
    os::unix::{fs::MetadataExt, net::UnixListener},
    path::{Path, PathBuf},
};

/// A file in the file system, by identity.
#[derive(Debug, Default)]
pub struct Owned {
    path: PathBuf,
    id: Option<(u64, u64)>,
}

impl Owned {
    /// Remember what's at `path` now (just made by us).
    pub fn new(path: &Path) -> Owned {
        Owned { path: path.to_owned(), id: identity(path) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Is the file at our path still the one we made?
    pub fn still_ours(&self) -> bool {
        self.id.is_some() && identity(&self.path) == self.id
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if self.still_ours() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Our own `wayland-N`: the first name whose lock we can take (as
/// libwayland does), listening, with its files.
pub struct Wayland {
    pub name: String,
    pub listener: UnixListener,
    /// Held while we live: the name is ours.
    pub _lock: std::fs::File,
    pub _files: [Owned; 2],
}

pub fn bind_wayland() -> std::io::Result<Wayland> {
    use std::os::fd::AsRawFd;
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    for n in 1..33 {
        let name = format!("wayland-{n}");
        let lock_path = dir.join(format!("{name}.lock"));
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path)?;
        // SAFETY: flock on an fd we own.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            continue;
        }
        let path = dir.join(&name);
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        return Ok(Wayland { name, listener, _lock: lock, _files: [Owned::new(&path), Owned::new(&lock_path)] });
    }
    Err(std::io::Error::other("no free wayland-N socket name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_a_replaced_file_alone() {
        let dir = std::env::temp_dir().join(format!("olo-sockfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let first = UnixListener::bind(&path).unwrap();
        let owned = Owned::new(&path);
        assert!(owned.still_ours());
        // Someone else's now (the directory was emptied, a new session bound it).
        std::fs::remove_file(&path).unwrap();
        drop(first);
        let _theirs = UnixListener::bind(&path).unwrap();
        assert!(!owned.still_ours());
        drop(owned);
        assert!(path.exists(), "removed a socket that wasn't ours");
        let mine = Owned::new(&path);
        drop(mine);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
