//! Where a repository lives: any place that keeps named objects whole (a
//! folder, Google Drive, …). Objects are written once and never changed:
//! a new name for new content, so a slow or eventually consistent store
//! never shows half of one. Names are paths with `/` ("data/ab/abcd…").

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

pub trait Store: Send + Sync {
    /// Keep `data` as `name` (whole, or not at all).
    fn put(&self, name: &str, data: &[u8]) -> io::Result<()>;
    fn get(&self, name: &str) -> io::Result<Vec<u8>>;
    /// `len` bytes of `name` from `offset` (a chunk out of a pack).
    fn get_range(&self, name: &str, offset: u64, len: u64) -> io::Result<Vec<u8>>;
    /// The names in folder `dir` and those under it ("index" → "index/…";
    /// "" everything), sorted, each with its size; none if there's no such
    /// folder.
    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>>;
    fn delete(&self, name: &str) -> io::Result<()>;
    /// Where it is, to say ("~/Backups/DiskBackup", "Google Drive").
    fn describe(&self) -> String;
    /// It keeps a checksum of each object (an MD5 of its bytes, as
    /// Google Drive does), so what's sent is worth summing first.
    fn keeps_checksums(&self) -> bool {
        false
    }
    /// The store's own checksum of each object in `dir` (as `list`), by
    /// name, lowercase hex: compared with what was sent, it shows each
    /// arrived and stayed as sent, without fetching it. None: it keeps
    /// none.
    fn checksums(&self, _dir: &str) -> io::Result<Option<HashMap<String, String>>> {
        Ok(None)
    }
}

/// A store in a folder (a disk of its own, a mounted share; and the tests).
pub struct LocalStore {
    root: PathBuf,
}

impl LocalStore {
    pub fn new(root: impl Into<PathBuf>) -> LocalStore {
        LocalStore { root: root.into() }
    }

    fn path(&self, name: &str) -> io::Result<PathBuf> {
        // (Names are the repository's own; never out of the folder.)
        if name.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad name {name}")));
        }
        Ok(self.root.join(name))
    }
}

impl Store for LocalStore {
    fn put(&self, name: &str, data: &[u8]) -> io::Result<()> {
        let path = self.path(name)?;
        let dir = path.parent().unwrap_or(Path::new("/"));
        std::fs::create_dir_all(dir)?;
        // Whole or not at all: written aside, synced, then named.
        let tmp = dir.join(format!(".{}.part", path.file_name().unwrap_or_default().to_string_lossy()));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)
    }

    fn get(&self, name: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.path(name)?)
    }

    fn get_range(&self, name: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let f = std::fs::File::open(self.path(name)?)?;
        let mut out = vec![0u8; len as usize];
        f.read_exact_at(&mut out, offset)?;
        Ok(out)
    }

    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>> {
        fn walk(root: &Path, at: &Path, out: &mut Vec<(String, u64)>) -> io::Result<()> {
            let entries = match std::fs::read_dir(at) {
                Ok(e) => e,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            };
            for e in entries {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if e.file_type()?.is_dir() {
                    walk(root, &e.path(), out)?;
                } else if let Ok(rel) = e.path().strip_prefix(root) {
                    out.push((rel.to_string_lossy().into_owned(), e.metadata()?.len()));
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        // ("" is everything.)
        let at = if dir.is_empty() { self.root.clone() } else { self.path(dir)? };
        walk(&self.root, &at, &mut out)?;
        out.sort();
        Ok(out)
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(name)?) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }
}

/// A speed limit for one direction (Ben, 2026-11-07: "make sure that i
/// can also set a max upload/download speed for the backups too"): bytes a
/// second, 0 for none, changed at any time (a backup running takes the new
/// one at its next piece). A store paces what it sends or fetches with it
/// a piece at a time (`take` before each), so the network never sees more
/// than the limit for longer than a moment: up to a quarter second's worth
/// goes at once, then it waits.
#[derive(Debug, Default)]
pub struct Limit {
    bytes_per_second: std::sync::atomic::AtomicU64,
    /// When what was let through so far is paid off.
    paid: std::sync::Mutex<Option<std::time::Instant>>,
}

impl Limit {
    pub fn new(bytes_per_second: u64) -> Limit {
        let l = Limit::default();
        l.set(bytes_per_second);
        l
    }

    pub fn set(&self, bytes_per_second: u64) {
        self.bytes_per_second.store(bytes_per_second, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.bytes_per_second.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `n` bytes are about to go: wait as long as the limit asks.
    pub fn take(&self, n: u64) {
        let rate = self.get();
        if rate == 0 || n == 0 {
            return;
        }
        let wait = {
            let Ok(mut paid) = self.paid.lock() else { return };
            let now = std::time::Instant::now();
            let from = paid.map_or(now, |p| p.max(now));
            let until = from + std::time::Duration::from_secs_f64(n as f64 / rate as f64);
            *paid = Some(until);
            // (A quarter second's worth may go at once.)
            until.saturating_duration_since(now).saturating_sub(std::time::Duration::from_millis(250))
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

/// A store with speed limits, for one that doesn't pace itself (a folder):
/// each object paced a piece at a time as it's written or read.
pub struct Throttled<S> {
    pub inner: S,
    pub up: std::sync::Arc<Limit>,
    pub down: std::sync::Arc<Limit>,
}

/// The pieces a transfer is paced by.
pub const PIECE: usize = 256 << 10;

impl<S: Store> Store for Throttled<S> {
    fn put(&self, name: &str, data: &[u8]) -> io::Result<()> {
        for piece in data.chunks(PIECE) {
            self.up.take(piece.len() as u64);
        }
        self.inner.put(name, data)
    }

    fn get(&self, name: &str) -> io::Result<Vec<u8>> {
        let data = self.inner.get(name)?;
        for piece in data.chunks(PIECE) {
            self.down.take(piece.len() as u64);
        }
        Ok(data)
    }

    fn get_range(&self, name: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        self.down.take(len);
        self.inner.get_range(name, offset, len)
    }

    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>> {
        self.inner.list(dir)
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        self.inner.delete(name)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn keeps_checksums(&self) -> bool {
        self.inner.keeps_checksums()
    }

    fn checksums(&self, dir: &str) -> io::Result<Option<HashMap<String, String>>> {
        self.inner.checksums(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At a limit, what goes takes as long as it says (less the moment's
    /// allowance); none, at once; changed while it runs, taken at once.
    #[test]
    fn limits_pace() {
        let l = Limit::new(4 << 20);
        let t = std::time::Instant::now();
        for _ in 0..8 {
            l.take(256 << 10);
        }
        // 2 MiB at 4 MiB/s: half a second, less a quarter's allowance.
        let took = t.elapsed().as_secs_f64();
        assert!((0.2..0.4).contains(&took), "{took}");
        l.set(0);
        let t = std::time::Instant::now();
        l.take(100 << 20);
        assert!(t.elapsed().as_millis() < 5);
    }
}
