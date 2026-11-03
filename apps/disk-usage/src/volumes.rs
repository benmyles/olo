//! The volumes worth scanning: each mounted disk file system once (from
//! /proc/self/mountinfo), with its name, size and free space, and which
//! mounts a scan may cross into (`Mounts`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub name: String,
    pub mount: PathBuf,
    pub total: u64,
    pub free: u64,
}

#[derive(Debug, Clone)]
struct Mount {
    /// The part of the file system mounted here ("/home" for a btrfs
    /// subvolume, "/" for a whole file system).
    root: PathBuf,
    point: PathBuf,
    fstype: String,
    source: String,
}

/// mountinfo escapes spaces and the like as `\040`.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && let Some(v) = s.get(i + 1..i + 4).and_then(|o| u8::from_str_radix(o, 8).ok())
        {
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let l: Vec<&str> = left.split(' ').collect();
            let r: Vec<&str> = right.split(' ').collect();
            Some(Mount { root: unescape(l.get(3)?).into(), point: unescape(l.get(4)?).into(), fstype: r.first()?.to_string(), source: unescape(r.get(1)?) })
        })
        .collect()
}

fn mounts() -> Vec<Mount> {
    std::fs::read_to_string("/proc/self/mountinfo").map(|t| parse(&t)).unwrap_or_default()
}

/// A file system on a disk (not memory, a network or an image of the system).
fn on_disk(m: &Mount) -> bool {
    m.source.starts_with("/dev/") && !matches!(m.fstype.as_str(), "squashfs" | "overlay" | "iso9660" | "erofs")
}

/// (total, free) bytes of the file system holding `path`.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct from a valid C string.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let block = st.f_frsize as u64;
    Some((st.f_blocks as u64 * block, st.f_bavail as u64 * block))
}

/// Labels by device (/dev/disk/by-label).
fn labels() -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    for e in std::fs::read_dir("/dev/disk/by-label").into_iter().flatten().flatten() {
        if let Ok(dev) = std::fs::canonicalize(e.path()) {
            out.insert(dev, unescape(&e.file_name().to_string_lossy().replace("\\x20", "\\040")));
        }
    }
    out
}

/// The boot volume's name, as Tracker shows it.
fn system_name() -> String {
    std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty() && s != "localhost").unwrap_or_else(|| "System".into())
}

/// Each disk file system once, at its shortest mount point, the system
/// first.
pub fn volumes() -> Vec<Volume> {
    let labels = labels();
    let mut best: HashMap<String, Mount> = HashMap::new();
    for m in mounts().into_iter().filter(on_disk) {
        let shorter = best.get(&m.source).is_none_or(|b| m.point.as_os_str().len() < b.point.as_os_str().len());
        if shorter {
            best.insert(m.source.clone(), m);
        }
    }
    let mut out: Vec<Volume> = best
        .into_values()
        .filter_map(|m| {
            let (total, free) = space(&m.point)?;
            let label = std::fs::canonicalize(&m.source).ok().and_then(|d| labels.get(&d).cloned());
            let name = if m.point == Path::new("/") {
                system_name()
            } else {
                label.unwrap_or_else(|| m.point.file_name().map_or_else(|| m.point.display().to_string(), |n| n.to_string_lossy().into_owned()))
            };
            Some(Volume { name, mount: m.point, total, free })
        })
        .collect();
    out.sort_by(|a, b| (a.mount != Path::new("/")).cmp(&(b.mount != Path::new("/"))).then(a.mount.cmp(&b.mount)));
    out
}

/// Which mount points a scan of one folder goes into: those of the same
/// file system showing a part of it the scan doesn't already cover (a
/// btrfs `/home` under `/`), never other file systems (`/proc`, `/tmp`,
/// a USB disk) or the same part twice (bind mounts).
#[derive(Debug, Default)]
pub struct Mounts {
    by_point: HashMap<PathBuf, Mount>,
    source: Option<String>,
    covered: Vec<PathBuf>,
}

impl Mounts {
    pub fn for_scan(root: &Path) -> Mounts {
        Mounts::from(mounts(), root)
    }

    fn from(list: Vec<Mount>, root: &Path) -> Mounts {
        // The mount holding the scan's folder: the longest point above it.
        let holder = list.iter().filter(|m| root.starts_with(&m.point)).max_by_key(|m| m.point.as_os_str().len()).cloned();
        let covered = holder.as_ref().map(|m| vec![m.root.join(root.strip_prefix(&m.point).unwrap_or(Path::new("")))]).unwrap_or_default();
        Mounts { source: holder.map(|m| m.source), covered, by_point: list.into_iter().map(|m| (m.point.clone(), m)).collect() }
    }

    /// Whether a folder the scan reached at `path` should be scanned (it
    /// is no mount point, or one it may cross).
    pub fn enter(&mut self, path: &Path) -> bool {
        let Some(m) = self.by_point.get(path) else { return true };
        if Some(&m.source) != self.source.as_ref() || self.covered.iter().any(|c| m.root.starts_with(c)) {
            return false;
        }
        self.covered.push(m.root.clone());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
70 1 0:32 /root / rw,relatime shared:1 - btrfs /dev/nvme0n1p6 rw
71 70 0:32 /home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p6 rw
72 70 0:32 /root/srv /srv/bind rw,relatime shared:3 - btrfs /dev/nvme0n1p6 rw
73 70 0:40 / /tmp rw shared:4 - tmpfs tmpfs rw
74 70 259:5 / /boot rw shared:5 - ext4 /dev/nvme0n1p5 rw
75 70 8:1 / /run/media/ben/My\\040Disk rw shared:6 - exfat /dev/sda1 rw";

    #[test]
    fn parses_mountinfo() {
        let m = parse(INFO);
        assert_eq!(m.len(), 6);
        assert_eq!(m[1].root, Path::new("/home"));
        assert_eq!(m[5].point, Path::new("/run/media/ben/My Disk"));
        assert!(on_disk(&m[4]) && !on_disk(&m[3]));
    }

    #[test]
    fn crosses_only_new_parts_of_the_same_file_system() {
        let mut s = Mounts::from(parse(INFO), Path::new("/"));
        assert!(s.enter(Path::new("/usr")), "not a mount point");
        assert!(s.enter(Path::new("/home")), "another subvolume of the same disk");
        assert!(!s.enter(Path::new("/srv/bind")), "a bind of what / already shows");
        assert!(!s.enter(Path::new("/tmp")) && !s.enter(Path::new("/boot")));
        // A scan of /home alone covers only it.
        let mut h = Mounts::from(parse(INFO), Path::new("/home/ben"));
        assert!(h.enter(Path::new("/home/ben/x")));
        assert!(!h.enter(Path::new("/boot")));
    }
}
