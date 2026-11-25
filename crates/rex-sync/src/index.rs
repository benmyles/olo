//! What one side has: an `Index` of files, directories and symlinks under a
//! root, with stat-based caching so re-scanning an unchanged tree reads zero
//! file bytes, and a compact binary encoding for the on-disk cache.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::chunk::{self, ChunkId, ChunkRef};

/// Legacy temporary suffix once used by `wire::receive` while building a file
/// beside its destination; still recognized by [`is_part_name`] and skipped by
/// `Index::scan`.
pub const PART_SUFFIX: &str = ".olo-sync-part";

/// Fixed-length temporary prefix used by `wire::receive` while building a file
/// beside its destination (`.olo-sync-part-<16 hex>`), fitting `NAME_MAX` even
/// for 255-byte filenames; skipped by `Index::scan`.
pub const PART_PREFIX: &str = ".olo-sync-part-";

/// Whether a single filename (not a path) is a receiver partial-transfer file.
pub fn is_part_name(name: &str) -> bool {
    name.starts_with(PART_PREFIX) || name.ends_with(PART_SUFFIX)
}

/// Maximum number of example paths recorded per skipped category in [`Skipped`].
pub const MAX_SKIPPED_PATHS: usize = 200;

const MAGIC: &[u8; 8] = b"OLOSYNC\0";
const FORMAT_VERSION: u32 = 1;
const TRAILER_LEN: usize = 16;

/// What kind of filesystem entry an [`Entry`] describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// Symlink with its raw link target (recorded, never followed).
    Symlink(String),
    /// Hard link to an earlier included regular file in the same index
    /// (its root-relative path).
    HardLink(String),
}

/// One entry in an [`Index`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative path using `/` separators (never leading `/`, never `.` or `..`).
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub ino: u64,
    /// Unix permission bits (`mode & 0o777` for files; `mode & 0o3777` for
    /// directories, keeping setgid and sticky but never setuid).
    pub mode: u32,
    /// Whole-file BLAKE3 (zeros for directories and hard links; BLAKE3 of target for symlinks).
    pub hash: [u8; 32],
    /// Content-defined chunks (empty for directories, symlinks and hard links).
    pub chunks: Vec<ChunkRef>,
}

/// Which paths to include when walking a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Whether to descend into subdirectories. Default: `true`.
    pub recursive: bool,
    /// Glob patterns to exclude (e.g. `".git"`, `"*.tmp"`, `"build/out"`,
    /// `"/.cache"`). A pattern starting with `/` matches only from the root;
    /// a pattern without `/` matches any path component; a pattern with `/`
    /// inside matches the whole relative path.
    pub exclude: Vec<String>,
    /// If non-empty, only non-directory entries matching at least one pattern
    /// are included (directories are still traversed when `recursive` is true,
    /// unless excluded).
    pub include: Vec<String>,
    /// If non-empty, these files alone, by their whole relative path, and
    /// the folders on their way: no patterns (a name with `*` in it is
    /// itself). A single file sent: its folder, and it.
    pub only: Vec<String>,
}

impl Default for Filter {
    fn default() -> Self {
        Self { recursive: true, exclude: Vec::new(), include: Vec::new(), only: Vec::new() }
    }
}

impl Filter {
    pub fn all() -> Self {
        Self::default()
    }

    pub fn shallow() -> Self {
        Self { recursive: false, ..Self::default() }
    }

    /// Whether a directory at `rel_path` should be entered (and kept).
    pub fn allow_dir(&self, rel_path: &str) -> bool {
        if !self.only.is_empty() {
            return self.only.iter().any(|p| p.strip_prefix(rel_path).is_some_and(|rest| rest.starts_with('/')));
        }
        if !self.recursive {
            return false;
        }
        !self.is_excluded(rel_path)
    }

    /// Whether a file or symlink at `rel_path` passes the filter.
    pub fn matches_entry(&self, rel_path: &str) -> bool {
        if !self.only.is_empty() {
            return self.only.iter().any(|p| p == rel_path);
        }
        if self.is_excluded(rel_path) {
            return false;
        }
        if self.include.is_empty() {
            return true;
        }
        self.include.iter().any(|p| pattern_matches(p, rel_path))
    }

    fn is_excluded(&self, rel_path: &str) -> bool {
        if self.exclude.iter().any(|p| pattern_matches(p, rel_path)) {
            return true;
        }
        // An excluded ancestor folder excludes everything under it (so a
        // root-anchored pattern like `/.cache` also excludes `.cache/foo`).
        let mut end = rel_path.len();
        while let Some(slash) = rel_path[..end].rfind('/') {
            let ancestor = &rel_path[..slash];
            if self.exclude.iter().any(|p| pattern_matches(p, ancestor)) {
                return true;
            }
            end = slash;
        }
        false
    }
}

fn pattern_matches(pattern: &str, rel_path: &str) -> bool {
    if let Some(anchored) = pattern.strip_prefix('/') {
        let anchored = anchored.trim_end_matches('/');
        if anchored.is_empty() {
            return false;
        }
        // Root-anchored: a single-segment pattern like `/.cache` or `/*.tmp`
        // matches only at the root, never deeper in a subfolder.
        if !anchored.contains('/') && rel_path.contains('/') {
            return false;
        }
        glob_match(anchored.as_bytes(), rel_path.as_bytes())
    } else if pattern.contains('/') {
        glob_match(pattern.as_bytes(), rel_path.as_bytes())
    } else {
        rel_path.split('/').any(|seg| glob_match(pattern.as_bytes(), seg.as_bytes()))
    }
}

/// Simple `*` / `?` glob matcher (non-backtracking DP / two-pointer).
fn glob_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = Some(p);
            p += 1;
            star_t = t;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

/// Validate that a relative path has no leading/trailing `/`, no empty
/// segments, no `.` or `..`, no NUL bytes, and no reserved partial-transfer
/// filename (`.olo-sync-part-...` or `*.olo-sync-part`).
pub fn validate_rel_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("empty path");
    }
    if path.starts_with('/') {
        return Err("absolute path");
    }
    if path.ends_with('/') {
        return Err("trailing slash");
    }
    if path.as_bytes().contains(&0) {
        return Err("nul byte in path");
    }
    for seg in path.split('/') {
        if seg.is_empty() {
            return Err("empty path segment");
        }
        if seg == "." || seg == ".." {
            return Err("dot segment in path");
        }
        if is_part_name(seg) {
            return Err("reserved partial-transfer name");
        }
    }
    Ok(())
}

/// One category of entries passed over during a scan: exact total count and
/// up to [`MAX_SKIPPED_PATHS`] example paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkippedKind {
    pub count: u64,
    pub paths: Vec<String>,
}

impl SkippedKind {
    pub fn push(&mut self, path: String) {
        self.count += 1;
        if self.paths.len() < MAX_SKIPPED_PATHS {
            self.paths.push(path);
        }
    }
}

/// Everything a scan saw and passed over rather than sending, grouped by why:
/// counted and named, never silently dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skipped {
    /// Files, folders or symlinks that couldn't be read or stat'd.
    pub unread: SkippedKind,
    /// Entries whose name wasn't valid UTF-8 (lossily printed).
    pub non_utf8: SkippedKind,
    /// Special files (sockets, FIFOs, devices).
    pub special: SkippedKind,
    /// Folders on another filesystem below the root (`st_dev` differs).
    pub mounts: SkippedKind,
}

impl Skipped {
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    pub fn total(&self) -> u64 {
        self.unread.count + self.non_utf8.count + self.special.count + self.mounts.count
    }
}

/// Snapshot of every synced entry under a root, ordered by relative path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    pub entries: BTreeMap<String, Entry>,
    /// What a scan saw and couldn't read (no leave to), each with why:
    /// neither sent nor, by a mirror, taken away on the other side, since
    /// what it holds isn't known. Not kept in the encoding.
    pub unread: BTreeMap<String, String>,
    /// Folders on another filesystem below the root (`st_dev` differs from
    /// root's), passed over. Not kept in the encoding.
    pub mounts: Vec<String>,
    /// Names that weren't valid UTF-8, lossily printed. Not kept in the encoding.
    pub non_utf8: Vec<String>,
    /// Special files (sockets, FIFOs, devices), passed over. Not kept in the encoding.
    pub special: Vec<String>,
}

impl Index {
    /// Walk `root` with `filter`, reusing `hash` and `chunks` from `previous`
    /// whenever a file's size, mtime, ctime and inode are unchanged.
    ///
    /// Symlinks are recorded with their target and never followed. Special
    /// files (sockets, FIFOs, device nodes) and `.olo-sync-part` temp files
    /// are skipped.
    pub fn scan(root: &Path, filter: &Filter, previous: Option<&Index>) -> io::Result<Self> {
        Self::scan_with(root, filter, previous, |p| fs::File::open(p))
    }

    /// Like [`Index::scan`], opening files through `open` (useful in tests to
    /// verify that re-scanning an unchanged tree opens and reads zero files).
    pub fn scan_with<R: io::Read>(root: &Path, filter: &Filter, previous: Option<&Index>, open: impl FnMut(&Path) -> io::Result<R>) -> io::Result<Self> {
        Self::scan_with_dev(root, filter, previous, open, |_, m| m.dev())
    }

    pub(crate) fn scan_with_dev<R: io::Read>(
        root: &Path,
        filter: &Filter,
        previous: Option<&Index>,
        mut open: impl FnMut(&Path) -> io::Result<R>,
        mut dev_of: impl FnMut(&Path, &fs::Metadata) -> u64,
    ) -> io::Result<Self> {
        let mut index = Self::default();
        let mut chunk_buf = Vec::new();
        let mut seen_inodes: HashMap<(u64, u64), String> = HashMap::new();
        // (The root itself must be there: a folder gone isn't a folder
        // emptied, which a mirror would make the other side.)
        let root_meta = fs::symlink_metadata(root)?;
        if !root_meta.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("{} isn't a folder", root.display())));
        }
        let root_dev = dev_of(root, &root_meta);
        scan_dir(root, "", root_dev, filter, previous, &mut open, &mut dev_of, &mut chunk_buf, &mut seen_inodes, &mut index)?;
        Ok(index)
    }

    /// Summary of everything this scan passed over (counted, and up to
    /// [`MAX_SKIPPED_PATHS`] example paths per kind).
    pub fn skipped(&self) -> Skipped {
        let mut s = Skipped::default();
        for p in self.unread.keys() {
            s.unread.push(p.clone());
        }
        for p in &self.non_utf8 {
            s.non_utf8.push(p.clone());
        }
        for p in &self.special {
            s.special.push(p.clone());
        }
        for p in &self.mounts {
            s.mounts.push(p.clone());
        }
        s
    }

    /// What `root` has at `paths` alone (a scoped session's receiver: the
    /// files a sender names, nothing else of a root that may be a whole
    /// home looked at), each as `scan` would have it: a file unchanged
    /// since `previous` not read again, one that isn't there or is reached
    /// through a symlink left out (the receiver refuses to write there
    /// anyway), one that can't be read in `unread`.
    pub fn scan_paths(root: &Path, paths: &[String], previous: Option<&Index>) -> io::Result<Self> {
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("{} isn't a folder", root.display())));
        }
        let mut index = Self::default();
        let mut chunk_buf = Vec::new();
        let mut seen_inodes: HashMap<(u64, u64), String> = HashMap::new();
        'paths: for rel in paths {
            if validate_rel_path(rel).is_err() {
                continue;
            }
            let mut at = root.to_path_buf();
            let parts: Vec<&str> = rel.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                at.push(part);
                match fs::symlink_metadata(&at) {
                    Ok(m) if m.is_dir() => {}
                    _ => continue 'paths,
                }
            }
            at.push(parts[parts.len() - 1]);
            let meta = match fs::symlink_metadata(&at) {
                Ok(meta) if meta.is_file() => meta,
                _ => continue,
            };
            let mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
            let ctime_ns = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
            let (dev, ino, mode, size) = (meta.dev(), meta.ino(), meta.permissions().mode() & 0o777, meta.len());
            if meta.nlink() > 1
                && let Some(first) = seen_inodes.get(&(dev, ino))
            {
                index.entries.insert(rel.clone(), Entry { path: rel.clone(), kind: EntryKind::HardLink(first.clone()), size: 0, mtime_ns, ctime_ns, ino, mode, hash: [0u8; 32], chunks: Vec::new() });
                continue;
            }
            if let Some(prev) = previous.and_then(|idx| idx.entries.get(rel))
                && prev.kind == EntryKind::File
                && (prev.size, prev.mtime_ns, prev.ctime_ns, prev.ino) == (size, mtime_ns, ctime_ns, ino)
            {
                if meta.nlink() > 1 {
                    seen_inodes.insert((dev, ino), rel.clone());
                }
                index.entries.insert(rel.clone(), Entry { mode, ..prev.clone() });
                continue;
            }
            match fs::File::open(&at).and_then(|f| chunk::hash_and_chunk_in(&mut chunk_buf, f)) {
                Ok(d) => {
                    if meta.nlink() > 1 {
                        seen_inodes.insert((dev, ino), rel.clone());
                    }
                    index.entries.insert(rel.clone(), Entry { path: rel.clone(), kind: EntryKind::File, size: d.size, mtime_ns, ctime_ns, ino, mode, hash: d.hash, chunks: d.chunks });
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    index.unread.insert(rel.clone(), e.to_string());
                }
            }
        }
        Ok(index)
    }

    /// Deterministic 32-byte digest of the whole tree's synced state (paths,
    /// kinds, sizes, modes, and content hashes). Two indexes with the same
    /// `tree_hash` have identical trees, so a sync can finish in one round
    /// trip without exchanging a manifest.
    pub fn tree_hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"olo-rex-sync-tree-v1");
        h.update(&(self.entries.len() as u64).to_le_bytes());
        for entry in self.entries.values() {
            h.update(&(entry.path.len() as u32).to_le_bytes());
            h.update(entry.path.as_bytes());
            match &entry.kind {
                EntryKind::File => {
                    h.update(&[0u8]);
                    h.update(&entry.size.to_le_bytes());
                    h.update(&entry.mode.to_le_bytes());
                    h.update(&entry.hash);
                }
                EntryKind::Dir => {
                    h.update(&[1u8]);
                    h.update(&final_dir_mode(entry.mode).to_le_bytes());
                }
                EntryKind::Symlink(target) => {
                    h.update(&[2u8]);
                    h.update(&(target.len() as u32).to_le_bytes());
                    h.update(target.as_bytes());
                }
                EntryKind::HardLink(target) => {
                    h.update(&[3u8]);
                    h.update(&(target.len() as u32).to_le_bytes());
                    h.update(target.as_bytes());
                }
            }
        }
        *h.finalize().as_bytes()
    }

    /// Count of regular-file entries.
    pub fn file_count(&self) -> u64 {
        self.entries.values().filter(|e| matches!(e.kind, EntryKind::File)).count() as u64
    }

    /// Compact binary encoding with magic, version, explicit lengths, and a
    /// trailing 16-byte BLAKE3 integrity tag.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.entries.len() * 64);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

        for entry in self.entries.values() {
            write_str(&mut out, &entry.path);
            match &entry.kind {
                EntryKind::File => out.push(0),
                EntryKind::Dir => out.push(1),
                EntryKind::Symlink(target) => {
                    out.push(2);
                    write_str(&mut out, target);
                }
                EntryKind::HardLink(target) => {
                    out.push(3);
                    write_str(&mut out, target);
                }
            }
            out.extend_from_slice(&entry.size.to_le_bytes());
            out.extend_from_slice(&entry.mtime_ns.to_le_bytes());
            out.extend_from_slice(&entry.ctime_ns.to_le_bytes());
            out.extend_from_slice(&entry.ino.to_le_bytes());
            out.extend_from_slice(&entry.mode.to_le_bytes());
            if matches!(entry.kind, EntryKind::File) {
                out.extend_from_slice(&entry.hash);
                out.extend_from_slice(&(entry.chunks.len() as u32).to_le_bytes());
                for c in &entry.chunks {
                    out.extend_from_slice(&c.len.to_le_bytes());
                    out.extend_from_slice(&c.id);
                }
            }
        }

        let tag = blake3::hash(&out);
        out.extend_from_slice(&tag.as_bytes()[..TRAILER_LEN]);
        out
    }

    /// Decode an index produced by [`Index::encode`]. Any bad magic, unknown
    /// version, truncated buffer, bad checksum or malformed path returns an
    /// `io::Error` rather than panicking or over-allocating.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() < MAGIC.len() + 4 + 4 + TRAILER_LEN {
            return Err(invalid_data("index too short"));
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(invalid_data("bad index magic"));
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(invalid_data(format!("unsupported index version {version}")));
        }
        let body_len = bytes.len() - TRAILER_LEN;
        let expected = blake3::hash(&bytes[..body_len]);
        if expected.as_bytes()[..TRAILER_LEN] != bytes[body_len..] {
            return Err(invalid_data("index checksum mismatch"));
        }

        let mut cur = Cursor::new(&bytes[12..body_len]);
        let count = cur.read_u32()? as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let path = cur.read_str()?;
            validate_rel_path(&path).map_err(|e| invalid_data(format!("invalid path: {e}")))?;
            let kind_tag = cur.read_u8()?;
            let kind = match kind_tag {
                0 => EntryKind::File,
                1 => EntryKind::Dir,
                2 => EntryKind::Symlink(cur.read_str()?),
                3 => {
                    let target = cur.read_str()?;
                    validate_rel_path(&target).map_err(|e| invalid_data(format!("invalid hardlink target: {e}")))?;
                    EntryKind::HardLink(target)
                }
                _ => return Err(invalid_data(format!("unknown entry kind {kind_tag}"))),
            };
            let size = cur.read_u64()?;
            let mtime_ns = cur.read_i64()?;
            let ctime_ns = cur.read_i64()?;
            let ino = cur.read_u64()?;
            let mode = cur.read_u32()?;
            let (hash, chunks) = match &kind {
                EntryKind::File => {
                    let hash = cur.read_array::<32>()?;
                    let chunk_count = cur.read_u32()? as usize;
                    // Bound allocation by remaining bytes (each ChunkRef is 20 bytes).
                    if chunk_count.saturating_mul(20) > cur.remaining() {
                        return Err(invalid_data("truncated chunk list in index"));
                    }
                    let mut chunks = Vec::with_capacity(chunk_count);
                    for _ in 0..chunk_count {
                        let len = cur.read_u32()?;
                        let id: ChunkId = cur.read_array::<16>()?;
                        chunks.push(ChunkRef { len, id });
                    }
                    (hash, chunks)
                }
                EntryKind::Dir | EntryKind::HardLink(_) => ([0u8; 32], Vec::new()),
                EntryKind::Symlink(target) => (*blake3::hash(target.as_bytes()).as_bytes(), Vec::new()),
            };
            entries.insert(path.clone(), Entry { path, kind, size, mtime_ns, ctime_ns, ino, mode, hash, chunks });
        }
        if cur.remaining() != 0 {
            return Err(invalid_data("trailing bytes in index"));
        }
        Ok(Self { entries, ..Self::default() })
    }

    /// Write the encoded index atomically to `path` (temporary sibling file
    /// renamed into place).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let data = self.encode();
        let pid = std::process::id();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("index");
        let tmp_name = format!(".{file_name}.tmp.{pid}.{seq}");
        let tmp_path = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(tmp_name),
            _ => Path::new(&tmp_name).to_path_buf(),
        };
        let res = (|| -> io::Result<()> {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp_path)?;
            f.write_all(&data)?;
            f.flush()?;
            drop(f);
            fs::rename(&tmp_path, path)?;
            Ok(())
        })();
        if res.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        res
    }

    /// Load and decode an index previously saved with [`Index::save`].
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        Self::decode(&bytes)
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_dir<R: io::Read>(
    root: &Path,
    rel_prefix: &str,
    root_dev: u64,
    filter: &Filter,
    previous: Option<&Index>,
    open: &mut impl FnMut(&Path) -> io::Result<R>,
    dev_of: &mut impl FnMut(&Path, &fs::Metadata) -> u64,
    chunk_buf: &mut Vec<u8>,
    seen_inodes: &mut HashMap<(u64, u64), String>,
    out: &mut Index,
) -> io::Result<()> {
    let dir_path = if rel_prefix.is_empty() { root.to_path_buf() } else { root.join(rel_prefix) };
    // (A tree is read while it changes: what goes as it is read was never
    // there to send; what can't be read is said, and the rest goes on.)
    let listing = match fs::read_dir(&dir_path) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound && !rel_prefix.is_empty() => return Ok(()),
        Err(e) if !rel_prefix.is_empty() => {
            out.unread.insert(rel_prefix.to_owned(), e.to_string());
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let mut names: Vec<String> = Vec::new();
    let mut non_utf8_here: Vec<String> = Vec::new();
    for entry in listing {
        let Ok(entry) = entry else { continue };
        match entry.file_name().into_string() {
            Ok(name) => {
                if is_part_name(&name) {
                    continue;
                }
                names.push(name);
            }
            Err(os_name) => {
                let lossy = os_name.to_string_lossy();
                let rel = if rel_prefix.is_empty() { lossy.into_owned() } else { format!("{rel_prefix}/{lossy}") };
                non_utf8_here.push(rel);
            }
        }
    }
    if !non_utf8_here.is_empty() {
        non_utf8_here.sort();
        out.non_utf8.extend(non_utf8_here);
    }
    names.sort();

    for name in names {
        let rel = if rel_prefix.is_empty() { name } else { format!("{rel_prefix}/{name}") };
        let full = root.join(&rel);
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                out.unread.insert(rel, e.to_string());
                continue;
            }
        };
        let ft = meta.file_type();
        let mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
        let ctime_ns = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
        let dev = dev_of(&full, &meta);
        let ino = meta.ino();

        if ft.is_dir() {
            if !filter.allow_dir(&rel) {
                continue;
            }
            // One filesystem: never descend into a mount on another device.
            if dev != root_dev {
                out.mounts.push(rel);
                continue;
            }
            let mode = final_dir_mode(meta.permissions().mode());
            out.entries.insert(rel.clone(), Entry { path: rel.clone(), kind: EntryKind::Dir, size: 0, mtime_ns, ctime_ns, ino, mode, hash: [0u8; 32], chunks: Vec::new() });
            scan_dir(root, &rel, root_dev, filter, previous, open, dev_of, chunk_buf, seen_inodes, out)?;
        } else if ft.is_symlink() {
            if !filter.matches_entry(&rel) {
                continue;
            }
            let mode = meta.permissions().mode() & 0o777;
            let target = match fs::read_link(&full).map(|t| t.into_os_string().into_string()) {
                Ok(Ok(target)) => target,
                Ok(Err(_)) => {
                    out.unread.insert(rel, "its target isn't UTF-8".into());
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    out.unread.insert(rel, e.to_string());
                    continue;
                }
            };
            let hash = *blake3::hash(target.as_bytes()).as_bytes();
            let size = target.len() as u64;
            out.entries.insert(rel.clone(), Entry { path: rel, kind: EntryKind::Symlink(target), size, mtime_ns, ctime_ns, ino, mode, hash, chunks: Vec::new() });
        } else if ft.is_file() {
            if !filter.matches_entry(&rel) {
                continue;
            }
            let mode = meta.permissions().mode() & 0o777;
            if meta.nlink() > 1
                && let Some(first) = seen_inodes.get(&(dev, ino))
            {
                out.entries.insert(rel.clone(), Entry { path: rel, kind: EntryKind::HardLink(first.clone()), size: 0, mtime_ns, ctime_ns, ino, mode, hash: [0u8; 32], chunks: Vec::new() });
                continue;
            }
            let size = meta.len();
            if let Some(prev) = previous.and_then(|idx| idx.entries.get(&rel))
                && prev.kind == EntryKind::File
                && prev.size == size
                && prev.mtime_ns == mtime_ns
                && prev.ctime_ns == ctime_ns
                && prev.ino == ino
            {
                if meta.nlink() > 1 {
                    seen_inodes.insert((dev, ino), rel.clone());
                }
                out.entries.insert(rel.clone(), Entry { path: rel, kind: EntryKind::File, size, mtime_ns, ctime_ns, ino, mode, hash: prev.hash, chunks: prev.chunks.clone() });
                continue;
            }
            let digest = match open(&full).and_then(|reader| chunk::hash_and_chunk_in(chunk_buf, reader)) {
                Ok(digest) => digest,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    out.unread.insert(rel, e.to_string());
                    continue;
                }
            };
            if meta.nlink() > 1 {
                seen_inodes.insert((dev, ino), rel.clone());
            }
            out.entries.insert(rel.clone(), Entry { path: rel, kind: EntryKind::File, size: digest.size, mtime_ns, ctime_ns, ino, mode, hash: digest.hash, chunks: digest.chunks });
        } else if filter.matches_entry(&rel) {
            // Special files (sockets, FIFOs, devices): recorded, never silently dropped.
            out.special.push(rel);
        }
    }
    Ok(())
}

/// A folder's working mode while the receiver is writing into it: its owner
/// can always read, write and enter it (`| 0o700`), before the end of the
/// session sets it back to [`final_dir_mode`].
pub fn dir_mode(mode: u32) -> u32 {
    (mode & 0o3777) | 0o700
}

/// A folder's final mode: keeps setgid (`0o2000`), sticky (`0o1000`) and
/// `rwx` bits (`0o777`), never setuid (`0o4000`).
pub fn final_dir_mode(mode: u32) -> u32 {
    mode & 0o3777
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn invalid_data(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| invalid_data("length overflow"))?;
        if end > self.buf.len() {
            return Err(invalid_data("unexpected end of index"));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn read_u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn read_i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn read_array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn read_str(&mut self) -> io::Result<String> {
        let len = self.read_u32()? as usize;
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| invalid_data("invalid UTF-8 in index"))?;
        Ok(s.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch_dir(name: &str) -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-sync-test-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Exact names: those files and the folders on their way, nothing a
    /// pattern would have taken with them.
    #[test]
    fn only_takes_the_named_files() {
        let f = Filter { only: vec!["a*b".into(), "deep/in/it.txt".into()], ..Filter::default() };
        assert!(f.matches_entry("a*b") && !f.matches_entry("axb") && f.matches_entry("deep/in/it.txt") && !f.matches_entry("deep/in/other.txt"));
        assert!(f.allow_dir("deep") && f.allow_dir("deep/in") && !f.allow_dir("de") && !f.allow_dir("other"));
    }

    #[test]
    fn unchanged_tree_reads_zero_files() {
        let root = scratch_dir("zero-reads");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello world").unwrap();
        fs::write(root.join("sub/b.bin"), vec![42u8; 100_000]).unwrap();
        symlink("a.txt", root.join("link")).unwrap();

        let first_reads = AtomicUsize::new(0);
        let idx1 = Index::scan_with(&root, &Filter::all(), None, |p| {
            first_reads.fetch_add(1, Ordering::Relaxed);
            fs::File::open(p)
        })
        .unwrap();
        assert_eq!(first_reads.load(Ordering::Relaxed), 2);
        assert_eq!(idx1.entries.len(), 4); // a.txt, link, sub, sub/b.bin
        assert_eq!(idx1.entries["link"].kind, EntryKind::Symlink("a.txt".into()));

        let second_reads = AtomicUsize::new(0);
        let idx2 = Index::scan_with(&root, &Filter::all(), Some(&idx1), |p| {
            second_reads.fetch_add(1, Ordering::Relaxed);
            fs::File::open(p)
        })
        .unwrap();
        assert_eq!(second_reads.load(Ordering::Relaxed), 0);
        assert_eq!(idx1, idx2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn symlinks_are_never_followed() {
        let root = scratch_dir("no-follow");
        let outside = scratch_dir("outside");
        fs::write(outside.join("secret.txt"), b"do not read").unwrap();
        symlink(&outside, root.join("escape_dir")).unwrap();
        symlink(outside.join("secret.txt"), root.join("escape_file")).unwrap();

        let idx = Index::scan(&root, &Filter::all(), None).unwrap();
        assert_eq!(idx.entries.len(), 2);
        assert!(matches!(idx.entries["escape_dir"].kind, EntryKind::Symlink(_)));
        assert!(matches!(idx.entries["escape_file"].kind, EntryKind::Symlink(_)));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn encode_decode_round_trip_and_damage_refused() {
        let root = scratch_dir("codec");
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join("dir/file.txt"), b"some data here").unwrap();
        symlink("dir/file.txt", root.join("sym")).unwrap();

        let idx = Index::scan(&root, &Filter::all(), None).unwrap();
        let encoded = idx.encode();
        let decoded = Index::decode(&encoded).unwrap();
        assert_eq!(idx, decoded);

        // Save/load atomic round trip.
        let cache = root.join("cache/index.bin");
        idx.save(&cache).unwrap();
        assert_eq!(Index::load(&cache).unwrap(), idx);

        // Damaged magic.
        let mut bad = encoded.clone();
        bad[0] ^= 0xff;
        assert!(Index::decode(&bad).is_err());

        // Old/wrong version.
        let mut bad = encoded.clone();
        bad[8] = 99;
        assert!(Index::decode(&bad).is_err());

        // Truncated.
        assert!(Index::decode(&encoded[..encoded.len() - 1]).is_err());
        assert!(Index::decode(&encoded[..10]).is_err());

        // Flipped bit in body.
        let mut bad = encoded.clone();
        bad[20] ^= 1;
        assert!(Index::decode(&bad).is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn filter_exclude_include_and_shallow() {
        let f = Filter { recursive: true, exclude: vec![".git".into(), "*.tmp".into(), "sub/skip.txt".into()], include: vec!["*.rs".into(), "*.txt".into()], only: vec![] };
        assert!(!f.allow_dir(".git"));
        assert!(!f.allow_dir("a/.git"));
        assert!(f.allow_dir("src"));
        assert!(f.matches_entry("src/main.rs"));
        assert!(!f.matches_entry("src/main.tmp"));
        assert!(!f.matches_entry("sub/skip.txt"));
        assert!(f.matches_entry("sub/keep.txt"));
        assert!(!f.matches_entry("bin/app"));

        let s = Filter::shallow();
        assert!(!s.allow_dir("sub"));
        assert!(s.matches_entry("top.txt"));
    }

    /// Item 2: root-anchored patterns starting with `/` match only from the
    /// root, while patterns without `/` match any component and patterns with
    /// `/` inside match the whole relative path.
    #[test]
    fn root_anchored_excludes_match_only_from_root() {
        let f = Filter { exclude: vec!["/.cache".into(), "/*.log".into(), "build/out".into(), "*.tmp".into()], ..Filter::default() };
        assert!(!f.allow_dir(".cache"));
        assert!(!f.matches_entry(".cache/data"));
        assert!(!f.matches_entry(".cache/sub/data"));
        assert!(f.allow_dir("proj/.cache"));
        assert!(f.matches_entry("proj/.cache/data"));
        assert!(!f.matches_entry("root.log"));
        assert!(f.matches_entry("proj/app.log"));
        assert!(!f.allow_dir("build/out"));
        assert!(f.allow_dir("nested/build/out"));
        assert!(!f.matches_entry("a.tmp"));
        assert!(!f.matches_entry("proj/b.tmp"));

        let root = scratch_dir("anchored-exclude");
        fs::create_dir_all(root.join(".cache/sub")).unwrap();
        fs::create_dir_all(root.join("proj/.cache")).unwrap();
        fs::write(root.join(".cache/sub/x"), b"skip").unwrap();
        fs::write(root.join("proj/.cache/keep"), b"keep").unwrap();
        fs::write(root.join("root.log"), b"skip").unwrap();
        fs::write(root.join("proj/app.log"), b"keep").unwrap();

        let idx = Index::scan(&root, &f, None).unwrap();
        assert!(!idx.entries.contains_key(".cache"));
        assert!(!idx.entries.contains_key(".cache/sub/x"));
        assert!(!idx.entries.contains_key("root.log"));
        assert!(idx.entries.contains_key("proj/.cache/keep"));
        assert!(idx.entries.contains_key("proj/app.log"));

        let _ = fs::remove_dir_all(&root);
    }

    /// Item 4b + Item 5: hard links encode/decode cleanly and fixed-length
    /// `.olo-sync-part-<hash>` files are skipped by `Index::scan` and refused
    /// by `validate_rel_path`.
    #[test]
    fn hardlinks_roundtrip_in_index_and_part_files_skipped() {
        let root = scratch_dir("hl-part-idx");
        fs::write(root.join("a.txt"), b"shared bytes").unwrap();
        fs::hard_link(root.join("a.txt"), root.join("b.txt")).unwrap();
        fs::write(root.join(format!("{PART_PREFIX}0123456789abcdef")), b"temp").unwrap();
        fs::write(root.join(format!("legacy{PART_SUFFIX}")), b"temp").unwrap();

        let idx = Index::scan(&root, &Filter::all(), None).unwrap();
        assert_eq!(idx.entries.len(), 2);
        assert_eq!(idx.entries["a.txt"].kind, EntryKind::File);
        assert_eq!(idx.entries["b.txt"].kind, EntryKind::HardLink("a.txt".into()));

        let decoded = Index::decode(&idx.encode()).unwrap();
        assert_eq!(idx, decoded);
        assert!(validate_rel_path(&format!("{PART_PREFIX}0123456789abcdef")).is_err());
        assert!(validate_rel_path(&format!("sub/{PART_PREFIX}deadbeef")).is_err());

        let _ = fs::remove_dir_all(&root);
    }
}
