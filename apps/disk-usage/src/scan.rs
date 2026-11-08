//! Scanning a folder on a worker thread into a tree the window draws
//! while it grows. Sizes are what files take on disk (allocated blocks,
//! so sparse files count what they use), hard links count once, and the
//! scan stays on one file system (`volumes::Mounts`).

use olo_sync::{Mutex, RwLock};
use std::{
    collections::HashSet,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::volumes::Mounts;

pub type NodeId = u32;
pub const ROOT: NodeId = 0;

#[derive(Debug)]
pub struct Node {
    pub name: Box<str>,
    /// Bytes on disk, with everything inside for a folder.
    pub size: u64,
    pub parent: NodeId,
    pub dir: bool,
    /// Files inside (a folder), or 1.
    pub files: u64,
    pub children: Vec<NodeId>,
    /// A folder the scan couldn't read (its size is a floor).
    pub unreadable: bool,
    /// Moved to the Trash or deleted since: left out everywhere.
    pub gone: bool,
}

#[derive(Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: PathBuf,
    /// Bumped by every change, so views know to sort again.
    pub generation: u64,
    /// Children are in order of size (the scan has finished).
    pub sorted: bool,
}

impl Tree {
    pub fn new(root: &Path) -> Tree {
        let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        let node = Node { name: name.into(), size: 0, parent: ROOT, dir: true, files: 0, children: Vec::new(), unreadable: false, gone: false };
        Tree { nodes: vec![node], root: root.to_path_buf(), generation: 0, sorted: false }
    }

    pub fn path(&self, mut id: NodeId) -> PathBuf {
        let mut parts = Vec::new();
        while id != ROOT {
            parts.push(&*self.nodes[id as usize].name);
            id = self.nodes[id as usize].parent;
        }
        let mut p = self.root.clone();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    /// The node at `path`, if the scan has reached it.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let rest = path.strip_prefix(&self.root).ok()?;
        let mut id = ROOT;
        for part in rest.components() {
            let name = part.as_os_str().to_string_lossy();
            id = *self.nodes[id as usize].children.iter().find(|&&c| *self.nodes[c as usize].name == *name && !self.nodes[c as usize].gone)?;
        }
        Some(id)
    }

    /// `id`'s folders from the scan's root down to it.
    pub fn ancestry(&self, mut id: NodeId) -> Vec<NodeId> {
        let mut out = vec![id];
        while id != ROOT {
            id = self.nodes[id as usize].parent;
            out.push(id);
        }
        out.reverse();
        out
    }

    pub fn is_inside(&self, id: NodeId, folder: NodeId) -> bool {
        let mut at = id;
        loop {
            if at == folder {
                return true;
            }
            if at == ROOT {
                return false;
            }
            at = self.nodes[at as usize].parent;
        }
    }

    /// What's in `id`, largest first, leaving out anything smaller than
    /// `min` bytes.
    pub fn children(&self, id: NodeId, min: u64) -> Vec<NodeId> {
        let node = &self.nodes[id as usize];
        let mut out: Vec<NodeId> = node.children.iter().copied().filter(|&c| !self.nodes[c as usize].gone && self.nodes[c as usize].size >= min).collect();
        if !self.sorted {
            out.sort_by(|&a, &b| self.nodes[b as usize].size.cmp(&self.nodes[a as usize].size));
        }
        out
    }

    /// Take `id` out: it was moved to the Trash or deleted.
    pub fn remove(&mut self, id: NodeId) {
        if id == ROOT || self.nodes[id as usize].gone {
            return;
        }
        let (size, files) = (self.nodes[id as usize].size, self.nodes[id as usize].files);
        self.nodes[id as usize].gone = true;
        let mut at = self.nodes[id as usize].parent;
        let mut above = Vec::new();
        loop {
            let n = &mut self.nodes[at as usize];
            n.size = n.size.saturating_sub(size);
            n.files = n.files.saturating_sub(files);
            above.push(at);
            if at == ROOT {
                break;
            }
            at = n.parent;
        }
        // Smaller now, so in a new place among their neighbours.
        if self.sorted {
            for a in above {
                let mut children = std::mem::take(&mut self.nodes[a as usize].children);
                children.sort_by(|&x, &y| self.nodes[y as usize].size.cmp(&self.nodes[x as usize].size));
                self.nodes[a as usize].children = children;
            }
        }
        self.generation += 1;
    }

    /// Put `id` back in (taken out by `remove`, then put back from the
    /// Trash where it was).
    pub fn put_back(&mut self, id: NodeId) {
        if id == ROOT || !self.nodes[id as usize].gone {
            return;
        }
        let (size, files) = (self.nodes[id as usize].size, self.nodes[id as usize].files);
        self.nodes[id as usize].gone = false;
        let mut at = self.nodes[id as usize].parent;
        let mut above = Vec::new();
        loop {
            let n = &mut self.nodes[at as usize];
            n.size += size;
            n.files += files;
            above.push(at);
            if at == ROOT {
                break;
            }
            at = n.parent;
        }
        if self.sorted {
            for a in above {
                let mut children = std::mem::take(&mut self.nodes[a as usize].children);
                children.sort_by(|&x, &y| self.nodes[y as usize].size.cmp(&self.nodes[x as usize].size));
                self.nodes[a as usize].children = children;
            }
        }
        self.generation += 1;
    }

    fn sort_all(&mut self) {
        for i in 0..self.nodes.len() {
            let mut children = std::mem::take(&mut self.nodes[i].children);
            children.sort_by(|&a, &b| self.nodes[b as usize].size.cmp(&self.nodes[a as usize].size));
            self.nodes[i].children = children;
        }
        self.sorted = true;
        self.generation += 1;
    }
}

/// How a scan is going, shared with the window.
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    /// Folders that couldn't be read.
    pub unreadable: AtomicU64,
    pub current: Mutex<PathBuf>,
    pub cancel: AtomicBool,
    pub done: AtomicBool,
}

pub struct Scan {
    pub tree: Arc<RwLock<Tree>>,
    pub progress: Arc<Progress>,
}

impl Scan {
    pub fn running(&self) -> bool {
        !self.progress.done.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Scan `root` on a thread; `tick` runs every tenth of a second while it
/// does, and once at the end.
pub fn start(root: PathBuf, tick: impl Fn() + Send + 'static) -> Scan {
    let tree = Arc::new(RwLock::new(Tree::new(&root)));
    let progress = Arc::new(Progress::default());
    let (t, p) = (tree.clone(), progress.clone());
    std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || {
            walk(&root, &t, &p, &tick);
            if let Ok(mut tree) = t.write() {
                tree.sort_all();
            }
            p.done.store(true, Ordering::Relaxed);
            tick();
        })
        .expect("thread");
    Scan { tree, progress }
}

/// An entry found in a folder, before it goes into the tree.
struct Found {
    name: Box<str>,
    size: u64,
    dir: bool,
}

fn walk(root: &Path, tree: &RwLock<Tree>, p: &Progress, tick: &dyn Fn()) {
    let mut mounts = Mounts::for_scan(root);
    let mut linked: HashSet<(u64, u64)> = HashSet::new();
    let mut stack: Vec<(NodeId, PathBuf)> = vec![(ROOT, root.to_path_buf())];
    if let Ok(meta) = std::fs::symlink_metadata(root) {
        add_size(&mut tree.write().unwrap(), ROOT, meta.blocks() * 512, 0);
    }
    let mut last_tick = Instant::now();
    while let Some((id, dir)) = stack.pop() {
        if p.cancel.load(Ordering::Relaxed) {
            return;
        }
        if last_tick.elapsed() >= Duration::from_millis(100) {
            *p.current.lock().unwrap() = dir.clone();
            tick();
            last_tick = Instant::now();
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                p.unreadable.fetch_add(1, Ordering::Relaxed);
                tree.write().unwrap().nodes[id as usize].unreadable = true;
                continue;
            }
        };
        let mut found = Vec::new();
        for e in entries.flatten() {
            // DirEntry::metadata doesn't follow symlinks: a link is its own
            // few bytes, never what it points to.
            let Ok(meta) = e.metadata() else { continue };
            let dir = meta.is_dir();
            if dir && !mounts.enter(&e.path()) {
                continue;
            }
            let mut size = meta.blocks() * 512;
            if !dir && meta.nlink() > 1 && !linked.insert((meta.dev(), meta.ino())) {
                size = 0;
            }
            found.push(Found { name: e.file_name().to_string_lossy().into(), size, dir });
        }
        // Into the tree in one go, so the window waits at most a folder.
        let mut t = tree.write().unwrap();
        let (mut bytes, mut files) = (0, 0);
        let mut dirs = Vec::new();
        for f in found {
            let child = t.nodes.len() as NodeId;
            bytes += f.size;
            files += u64::from(!f.dir);
            t.nodes.push(Node { name: f.name, size: f.size, parent: id, dir: f.dir, files: u64::from(!f.dir), children: Vec::new(), unreadable: false, gone: false });
            t.nodes[id as usize].children.push(child);
            if f.dir {
                dirs.push(child);
            }
        }
        add_size(&mut t, id, bytes, files);
        for child in dirs.into_iter().rev() {
            let name = t.nodes[child as usize].name.to_string();
            stack.push((child, dir.join(name)));
        }
        drop(t);
        p.files.fetch_add(files, Ordering::Relaxed);
        p.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Add to `id` and every folder above it.
fn add_size(t: &mut Tree, mut id: NodeId, bytes: u64, files: u64) {
    loop {
        let n = &mut t.nodes[id as usize];
        n.size += bytes;
        n.files += files;
        if id == ROOT {
            break;
        }
        id = n.parent;
    }
    t.generation += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(scan: &Scan) {
        while scan.running() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn sizes_add_up_and_hard_links_count_once() {
        let dir = PathBuf::from(format!("/var/tmp/olo-disk-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/big"), vec![1u8; 256 * 1024]).unwrap();
        std::fs::write(dir.join("a/small"), b"hi").unwrap();
        std::fs::hard_link(dir.join("a/b/big"), dir.join("twin")).unwrap();
        std::os::unix::fs::symlink("/usr", dir.join("link")).unwrap();
        let scan = start(dir.clone(), || {});
        wait(&scan);
        let t = scan.tree.read().unwrap();
        assert!(t.sorted);
        let a = t.find(&dir.join("a")).unwrap();
        let big = t.find(&dir.join("a/b/big")).unwrap();
        let twin = t.find(&dir.join("twin")).unwrap();
        let (b, w) = (t.nodes[big as usize].size, t.nodes[twin as usize].size);
        assert!(b.max(w) >= 256 * 1024 && b.min(w) == 0, "one of the links counts: {b} {w}");
        assert_eq!(t.nodes[ROOT as usize].files, 4);
        let sum: u64 = t.nodes[ROOT as usize].children.iter().map(|&c| t.nodes[c as usize].size).sum();
        assert!(t.nodes[ROOT as usize].size >= sum && t.nodes[ROOT as usize].size - sum <= 64 * 1024, "the root's own blocks only");
        assert_eq!(t.children(ROOT, 0)[0], if t.nodes[big as usize].size > 0 { a } else { twin });
        assert_eq!(t.path(big), dir.join("a/b/big"));
        assert!(t.is_inside(big, a) && !t.is_inside(a, big));
        drop(t);
        let mut t = scan.tree.write().unwrap();
        let before = t.nodes[ROOT as usize].size;
        let a_size = t.nodes[a as usize].size;
        t.remove(a);
        assert_eq!(t.nodes[ROOT as usize].size, before - a_size);
        assert_eq!(t.find(&dir.join("a")), None);
        assert!(t.nodes[ROOT as usize].children.windows(2).all(|w| t.nodes[w[0] as usize].size >= t.nodes[w[1] as usize].size));
        // Put back (Undo of Move to Trash): as it was.
        t.put_back(a);
        assert_eq!(t.nodes[ROOT as usize].size, before);
        assert_eq!(t.nodes[ROOT as usize].files, 4);
        assert_eq!(t.find(&dir.join("a")), Some(a));
        assert_eq!(t.children(ROOT, 0)[0], if t.nodes[big as usize].size > 0 { a } else { twin });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
