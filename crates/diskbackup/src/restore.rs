//! Putting back: a file or folder of a snapshot, as it was (contents,
//! permissions, modified times, links), wherever asked.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};

use crate::{
    Error, Result,
    backup::Progress,
    crypto::Id,
    repo::Repo,
    tree::{Kind, Node},
};

/// The node at `path` (absolute, as it was backed up) in the snapshot whose
/// tree is `root`; `/` is the root's own.
pub fn find(repo: &Repo, root: &Id, path: &Path) -> Result<Option<Node>> {
    let mut node = Node { name: b"/".to_vec(), kind: Kind::Dir, mode: 0o755, mtime: 0, ctime: 0, ino: 0, size: 0, content: Vec::new(), subtree: Some(*root), target: None };
    for c in path.components() {
        let Component::Normal(name) = c else { continue };
        let Some(sub) = node.subtree else { return Ok(None) };
        let tree = repo.tree(&sub)?;
        match tree.get(name.as_encoded_bytes()) {
            Some(n) => node = n.clone(),
            None => return Ok(None),
        }
    }
    Ok(Some(node))
}

/// Put `path` of the snapshot whose tree is `root` back as `target` (which
/// is replaced if it's a file; a folder's contents are put in it).
pub fn restore(repo: &Repo, root: &Id, path: &Path, target: &Path, cancel: &AtomicBool, progress: &Progress) -> Result<()> {
    let node = find(repo, root, path)?.ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} isn't in this snapshot", path.display()))))?;
    put(repo, &node, target, cancel, progress)
}

fn put(repo: &Repo, node: &Node, target: &Path, cancel: &AtomicBool, progress: &Progress) -> Result<()> {
    if cancel.load(Relaxed) {
        return Err(Error::Cancelled);
    }
    match node.kind {
        Kind::Dir => {
            std::fs::create_dir_all(target)?;
            if let Some(id) = node.subtree {
                for child in repo.tree(&id)?.nodes {
                    put(repo, &child, &target.join(std::ffi::OsStr::from_encoded_bytes_checked(&child.name)), cancel, progress)?;
                }
            }
            // (Times after what's in it, which changes them.)
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(node.mode))?;
            set_mtime(target, node.mtime);
        }
        Kind::File => {
            use std::io::Write;
            if let Ok(mut c) = progress.current.lock() {
                *c = target.display().to_string();
            }
            let tmp = target.with_file_name(format!(".{}.restoring", target.file_name().unwrap_or_default().to_string_lossy()));
            {
                let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
                for id in &node.content {
                    if cancel.load(Relaxed) {
                        drop(f);
                        let _ = std::fs::remove_file(&tmp);
                        return Err(Error::Cancelled);
                    }
                    let data = repo.get(id)?;
                    f.write_all(&data)?;
                    progress.bytes.fetch_add(data.len() as u64, Relaxed);
                }
                f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            }
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(node.mode))?;
            set_mtime(&tmp, node.mtime);
            std::fs::rename(&tmp, target)?;
            progress.files.fetch_add(1, Relaxed);
        }
        Kind::Symlink => {
            let to = std::ffi::OsString::from_encoded_bytes_checked(node.target.clone().unwrap_or_default());
            let _ = std::fs::remove_file(target);
            std::os::unix::fs::symlink(to, target)?;
        }
    }
    Ok(())
}

fn set_mtime(p: &Path, ns: i64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns.max(0) as u64);
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(t));
    }
}

/// The bytes of `OsStr`'s and `OsString`'s, back (they came from one).
trait FromBytes {
    fn from_encoded_bytes_checked(b: &[u8]) -> &Self;
}

impl FromBytes for std::ffi::OsStr {
    fn from_encoded_bytes_checked(b: &[u8]) -> &std::ffi::OsStr {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b)
    }
}

trait FromBytesOwned {
    fn from_encoded_bytes_checked(b: Vec<u8>) -> Self;
}

impl FromBytesOwned for std::ffi::OsString {
    fn from_encoded_bytes_checked(b: Vec<u8>) -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b)
    }
}
