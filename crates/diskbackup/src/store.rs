//! Where a repository lives: any place that keeps named objects whole (a
//! folder, Google Drive, …). Objects are written once and never changed:
//! a new name for new content, so a slow or eventually consistent store
//! never shows half of one. Names are paths with `/` ("data/ab/abcd…").

use std::{
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
    /// "" everything), none if there's no such folder.
    fn list(&self, dir: &str) -> io::Result<Vec<String>>;
    fn delete(&self, name: &str) -> io::Result<()>;
    /// Where it is, to say ("~/Backups/DiskBackup", "Google Drive").
    fn describe(&self) -> String;
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

    fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        fn walk(root: &Path, at: &Path, out: &mut Vec<String>) -> io::Result<()> {
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
                    out.push(rel.to_string_lossy().into_owned());
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
