//! Three-way reconciliation and conflict preservation for two-way folder sync.
//!
//! Given the last agreed `baseline` index, the current `local` (desktop) index,
//! and the current `remote` (host) index, [`plan`] produces a deterministic
//! [`TwoWayPlan`] that never loses data:
//!
//! - Changes on one side only propagate to the other (adds, edits, mode
//!   changes, deletes).
//! - Edit-vs-delete conflicts keep the edited file (never silently drop edits).
//! - Edit-vs-edit and add-vs-add conflicts keep the newer `mtime_ns` in place
//!   (tie-break: desktop wins) and preserve the other side's version beside it
//!   as `<stem>.conflict-<side>-<YYYYMMDD-HHMMSS>[.<ext>]` on BOTH sides.
//! - File-vs-directory conflicts keep the directory in place so its children
//!   are undisturbed and preserve the conflicting file as a conflict copy.
//! - Unreadable entries (`Index::unread`) are never propagated as deletions.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::index::{Entry, EntryKind, Filter, Index, PART_PREFIX, final_dir_mode, validate_rel_path};

const INDEX_FRAME_MAGIC: &[u8; 4] = b"OLIX";

/// Encode an [`Index`] (stripping per-file chunk lists) plus its `unread` map
/// as a compact zstd-compressed frame for `olo-rex-link sync index`.
pub fn encode_index_frame(index: &Index, mut out: impl Write) -> io::Result<()> {
    let mut stripped = index.clone();
    for entry in stripped.entries.values_mut() {
        entry.chunks.clear();
    }
    out.write_all(INDEX_FRAME_MAGIC)?;
    let unread_count = u32::try_from(stripped.unread.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "too many unread paths"))?;
    out.write_all(&unread_count.to_le_bytes())?;
    for (path, reason) in &stripped.unread {
        let pb = path.as_bytes();
        let rb = reason.as_bytes();
        out.write_all(&(pb.len() as u32).to_le_bytes())?;
        out.write_all(pb)?;
        out.write_all(&(rb.len() as u32).to_le_bytes())?;
        out.write_all(rb)?;
    }
    let mut enc = zstd::stream::write::Encoder::new(&mut out, 3)?;
    stripped.encode_to(&mut enc)?;
    enc.finish()?;
    out.flush()
}

/// Decode a frame produced by [`encode_index_frame`].
pub fn decode_index_frame(mut input: impl Read) -> io::Result<Index> {
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic)?;
    if &magic != INDEX_FRAME_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad index frame header"));
    }
    let mut u32_buf = [0u8; 4];
    input.read_exact(&mut u32_buf)?;
    let unread_count = u32::from_le_bytes(u32_buf) as usize;
    if unread_count > 1_000_000 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "too many unread entries"));
    }
    let mut unread = BTreeMap::new();
    for _ in 0..unread_count {
        input.read_exact(&mut u32_buf)?;
        let plen = u32::from_le_bytes(u32_buf) as usize;
        if plen > 65536 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unread path too long"));
        }
        let mut pbuf = vec![0u8; plen];
        input.read_exact(&mut pbuf)?;
        let path = String::from_utf8(pbuf).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 unread path"))?;

        input.read_exact(&mut u32_buf)?;
        let rlen = u32::from_le_bytes(u32_buf) as usize;
        if rlen > 65536 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unread reason too long"));
        }
        let mut rbuf = vec![0u8; rlen];
        input.read_exact(&mut rbuf)?;
        let reason = String::from_utf8(rbuf).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 unread reason"))?;
        unread.insert(path, reason);
    }
    let mut dec = zstd::stream::read::Decoder::new(input)?;
    let mut idx = Index::decode_from(&mut dec)?;
    idx.unread = unread;
    Ok(idx)
}

/// Which side kept the canonical path in a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    Desktop,
    Host,
}

/// One conflict resolved without data loss by keeping a side-by-side copy of
/// the losing version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The original path kept by `winner`.
    pub path: String,
    /// The conflict copy path holding the losing side's contents.
    pub copy: String,
    /// Which side kept `path`.
    pub winner: Winner,
}

/// Complete reconciliation plan produced by [`plan`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TwoWayPlan {
    /// Local (desktop) `(src, dst)` copies to preserve losing desktop files
    /// before pulls overwrite `src`.
    pub local_copy: Vec<(String, String)>,
    /// Remote (host) `(src, dst)` copies to preserve losing host files before
    /// pushes overwrite `src`.
    pub remote_copy: Vec<(String, String)>,
    /// Paths to remove on desktop (sorted deepest-first).
    pub local_remove: Vec<String>,
    /// Paths to remove on host (sorted deepest-first).
    pub remote_remove: Vec<String>,
    /// Directories to create or chmod on desktop `(path, mode)` (shallowest-first).
    pub local_mkdir: Vec<(String, u32)>,
    /// Directories to create or chmod on host `(path, mode)` (shallowest-first).
    pub remote_mkdir: Vec<(String, u32)>,
    /// Non-directory paths to pull from host to desktop via scoped `receive`.
    pub pull_paths: Vec<String>,
    /// Non-directory paths to push from desktop to host via scoped `send`.
    pub push_paths: Vec<String>,
    /// Conflicts encountered and preserved in this plan.
    pub conflicts: Vec<Conflict>,
}

impl TwoWayPlan {
    /// True when both sides already match and no local/remote action is needed.
    pub fn is_empty(&self) -> bool {
        self.local_copy.is_empty()
            && self.remote_copy.is_empty()
            && self.local_remove.is_empty()
            && self.remote_remove.is_empty()
            && self.local_mkdir.is_empty()
            && self.remote_mkdir.is_empty()
            && self.pull_paths.is_empty()
            && self.push_paths.is_empty()
    }
}

/// Sanitize a host label into a short filename-safe tag for conflict copies
/// (e.g. `"My Build Box"` -> `"my-build-box"`).
pub fn sanitize_tag(label: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(ch.to_ascii_lowercase());
            if out.len() >= 24 {
                break;
            }
        } else {
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() { "host".to_owned() } else { trimmed.to_owned() }
}

/// Build a conflict copy path beside `rel_path` (e.g.
/// `dir/notes.md` -> `dir/notes.conflict-host-20261125-095500.md`),
/// staying within Linux's 255-byte `NAME_MAX`.
pub fn conflict_name(rel_path: &str, tag: &str, stamp: &str, suffix_num: Option<u32>) -> String {
    let (parent, name) = match rel_path.rsplit_once('/') {
        Some((p, n)) => (Some(p), n),
        None => (None, rel_path),
    };
    let tag = sanitize_tag(tag);
    let extra = match suffix_num {
        Some(n) => format!(".conflict-{tag}-{stamp}-{n}"),
        None => format!(".conflict-{tag}-{stamp}"),
    };
    // Split extension only when there is a non-empty stem before the last dot
    // (dotfiles like `.gitignore` have no extension).
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    let ext_part = ext.map(|e| format!(".{e}")).unwrap_or_default();
    let overhead = extra.len() + ext_part.len();
    let max_stem = 255usize.saturating_sub(overhead);
    let mut stem_end = stem.len().min(max_stem);
    while stem_end > 0 && !stem.is_char_boundary(stem_end) {
        stem_end -= 1;
    }
    let fname = format!("{}{}{}", &stem[..stem_end], extra, ext_part);
    match parent {
        Some(p) => format!("{p}/{fname}"),
        None => fname,
    }
}

/// True if `rel_path`'s filename looks like a conflict copy generated by
/// [`conflict_name`] (`*.conflict-<tag>-<YYYYMMDD>-<HHMMSS>*`).
pub fn is_conflict_path(rel_path: &str) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let Some((_, rest)) = name.split_once(".conflict-") else { return false };
    // Expect `<tag>-<8 digits>-<6 digits>` somewhere in `rest`.
    let bytes = rest.as_bytes();
    if bytes.len() < 16 {
        return false;
    }
    for i in 1..bytes.len().saturating_sub(15) {
        if bytes[i] == b'-' && bytes[i + 1..i + 9].iter().all(u8::is_ascii_digit) && bytes[i + 9] == b'-' && bytes[i + 10..i + 16].iter().all(u8::is_ascii_digit) {
            return true;
        }
    }
    false
}

/// List all conflict copy relative paths currently present under `root`
/// matching `filter`.
pub fn list_conflicts(root: &Path, filter: &Filter) -> io::Result<Vec<String>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let idx = Index::scan(root, filter, None)?;
    Ok(idx.entries.keys().filter(|p| is_conflict_path(p)).cloned().collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SemanticKind<'a> {
    File { size: u64, mode: u32, hash: [u8; 32] },
    Dir { mode: u32 },
    Symlink(&'a str),
}

fn resolve_entry<'a>(index: &'a Index, entry: &'a Entry) -> SemanticKind<'a> {
    match &entry.kind {
        EntryKind::File => SemanticKind::File { size: entry.size, mode: entry.mode & 0o777, hash: entry.hash },
        EntryKind::Dir => SemanticKind::Dir { mode: final_dir_mode(entry.mode) },
        EntryKind::Symlink(target) => SemanticKind::Symlink(target.as_str()),
        EntryKind::HardLink(target) => match index.entries.get(target) {
            Some(t) if matches!(t.kind, EntryKind::File) => SemanticKind::File { size: t.size, mode: t.mode & 0o777, hash: t.hash },
            _ => SemanticKind::File { size: entry.size, mode: entry.mode & 0o777, hash: entry.hash },
        },
    }
}

fn same_entry(a_idx: &Index, a: &Entry, b_idx: &Index, b: &Entry) -> bool {
    resolve_entry(a_idx, a) == resolve_entry(b_idx, b)
}

fn same_file_content(a_idx: &Index, a: &Entry, b_idx: &Index, b: &Entry) -> bool {
    matches!(
        (resolve_entry(a_idx, a), resolve_entry(b_idx, b)),
        (SemanticKind::File { size: sa, hash: ha, .. }, SemanticKind::File { size: sb, hash: hb, .. }) if sa == sb && ha == hb
    )
}

fn is_dir_entry(e: &Entry) -> bool {
    matches!(e.kind, EntryKind::Dir)
}

fn covered_by_unread(unread: &BTreeMap<String, String>, path: &str) -> bool {
    if unread.contains_key(path) {
        return true;
    }
    let mut end = path.len();
    while let Some(slash) = path[..end].rfind('/') {
        if unread.contains_key(&path[..slash]) {
            return true;
        }
        end = slash;
    }
    false
}

fn pick_unique_conflict_path(rel_path: &str, tag: &str, stamp: &str, reserved: &mut HashSet<String>) -> String {
    let first = conflict_name(rel_path, tag, stamp, None);
    if reserved.insert(first.clone()) {
        return first;
    }
    for n in 2..=10_000u32 {
        let cand = conflict_name(rel_path, tag, stamp, Some(n));
        if reserved.insert(cand.clone()) {
            return cand;
        }
    }
    let fallback = conflict_name(rel_path, tag, stamp, Some(u32::MAX));
    reserved.insert(fallback.clone());
    fallback
}

/// Compute a 3-way reconciliation plan between `local` (desktop) and `remote`
/// (host) relative to the last synced `baseline` (if any).
pub fn plan(baseline: Option<&Index>, local: &Index, remote: &Index, host_tag: &str, stamp: &str) -> TwoWayPlan {
    let empty_idx = Index::default();
    let base = baseline.unwrap_or(&empty_idx);

    let mut all_paths: BTreeSet<&str> = BTreeSet::new();
    for k in base.entries.keys() {
        all_paths.insert(k.as_str());
    }
    for k in local.entries.keys() {
        all_paths.insert(k.as_str());
    }
    for k in remote.entries.keys() {
        all_paths.insert(k.as_str());
    }

    // Paths already taken on either side so conflict copy names never collide.
    let mut reserved: HashSet<String> = HashSet::with_capacity(local.entries.len() + remote.entries.len());
    for k in local.entries.keys() {
        reserved.insert(k.clone());
    }
    for k in remote.entries.keys() {
        reserved.insert(k.clone());
    }

    // Track paths that will survive (must have their ancestor dirs preserved).
    let mut surviving_local: HashSet<String> = HashSet::new();
    let mut surviving_remote: HashSet<String> = HashSet::new();

    // First pass: decide non-directory entries and straightforward changes,
    // leaving dir-vs-non-dir collisions to check whether any children survive.
    #[derive(Debug)]
    enum PendingDirConflict<'a> {
        /// Local is non-dir, remote is dir (desktop tried to replace dir with file, or both added).
        LocalFileRemoteDir { path: &'a str, loc_changed: bool, rem_changed: bool },
        /// Remote is non-dir, local is dir.
        RemoteFileLocalDir { path: &'a str, loc_changed: bool, rem_changed: bool },
    }

    let mut out = TwoWayPlan::default();
    let mut pending_type_conflicts: Vec<PendingDirConflict<'_>> = Vec::new();

    for &path in &all_paths {
        if covered_by_unread(&local.unread, path) || covered_by_unread(&remote.unread, path) {
            continue;
        }
        let b = base.entries.get(path);
        let l = local.entries.get(path);
        let r = remote.entries.get(path);

        match (l, r) {
            (None, None) => {}
            (Some(le), Some(re)) if same_entry(local, le, remote, re) => {
                surviving_local.insert(path.to_owned());
                surviving_remote.insert(path.to_owned());
            }
            (Some(le), None) => {
                let loc_unchanged = b.is_some_and(|be| same_entry(base, be, local, le));
                if loc_unchanged {
                    // Deleted on host, untouched on desktop -> delete on desktop.
                    out.local_remove.push(path.to_owned());
                } else {
                    // Added or modified on desktop (even if deleted on host) -> push to host.
                    if is_dir_entry(le) {
                        out.remote_mkdir.push((path.to_owned(), final_dir_mode(le.mode)));
                    } else {
                        out.push_paths.push(path.to_owned());
                    }
                    surviving_local.insert(path.to_owned());
                    surviving_remote.insert(path.to_owned());
                }
            }
            (None, Some(re)) => {
                let rem_unchanged = b.is_some_and(|be| same_entry(base, be, remote, re));
                if rem_unchanged {
                    // Deleted on desktop, untouched on host -> delete on host.
                    out.remote_remove.push(path.to_owned());
                } else {
                    // Added or modified on host (even if deleted on desktop) -> pull to desktop.
                    if is_dir_entry(re) {
                        out.local_mkdir.push((path.to_owned(), final_dir_mode(re.mode)));
                    } else {
                        out.pull_paths.push(path.to_owned());
                    }
                    surviving_local.insert(path.to_owned());
                    surviving_remote.insert(path.to_owned());
                }
            }
            (Some(le), Some(re)) => {
                let loc_changed = b.is_none_or(|be| !same_entry(base, be, local, le));
                let rem_changed = b.is_none_or(|be| !same_entry(base, be, remote, re));

                // Dir vs non-dir: defer until we know whether any descendant survives.
                if !is_dir_entry(le) && is_dir_entry(re) {
                    pending_type_conflicts.push(PendingDirConflict::LocalFileRemoteDir { path, loc_changed, rem_changed });
                    continue;
                }
                if is_dir_entry(le) && !is_dir_entry(re) {
                    pending_type_conflicts.push(PendingDirConflict::RemoteFileLocalDir { path, loc_changed, rem_changed });
                    continue;
                }

                // Both are dirs (mode differs).
                if is_dir_entry(le) && is_dir_entry(re) {
                    let push = match (loc_changed, rem_changed) {
                        (true, false) => true,
                        (false, true) => false,
                        _ => le.mtime_ns >= re.mtime_ns,
                    };
                    if push {
                        out.remote_mkdir.push((path.to_owned(), final_dir_mode(le.mode)));
                    } else {
                        out.local_mkdir.push((path.to_owned(), final_dir_mode(re.mode)));
                    }
                    surviving_local.insert(path.to_owned());
                    surviving_remote.insert(path.to_owned());
                    continue;
                }

                // Both are non-dirs (File / Symlink).
                match (loc_changed, rem_changed) {
                    (true, false) => {
                        out.push_paths.push(path.to_owned());
                    }
                    (false, true) => {
                        out.pull_paths.push(path.to_owned());
                    }
                    _ => {
                        // Both changed (or no baseline and content differs).
                        if same_file_content(local, le, remote, re) {
                            // Identical file content, only permissions differ: newer mtime wins, no duplicate copy.
                            if le.mtime_ns >= re.mtime_ns {
                                out.push_paths.push(path.to_owned());
                            } else {
                                out.pull_paths.push(path.to_owned());
                            }
                        } else if le.mtime_ns >= re.mtime_ns {
                            // Desktop wins in place; preserve host's version as conflict copy.
                            let copy = pick_unique_conflict_path(path, host_tag, stamp, &mut reserved);
                            out.remote_copy.push((path.to_owned(), copy.clone()));
                            out.pull_paths.push(copy.clone());
                            out.push_paths.push(path.to_owned());
                            out.conflicts.push(Conflict { path: path.to_owned(), copy: copy.clone(), winner: Winner::Desktop });
                            surviving_local.insert(copy.clone());
                            surviving_remote.insert(copy);
                        } else {
                            // Host wins in place; preserve desktop's version as conflict copy.
                            let copy = pick_unique_conflict_path(path, "desktop", stamp, &mut reserved);
                            out.local_copy.push((path.to_owned(), copy.clone()));
                            out.pull_paths.push(path.to_owned());
                            out.push_paths.push(copy.clone());
                            out.conflicts.push(Conflict { path: path.to_owned(), copy: copy.clone(), winner: Winner::Host });
                            surviving_local.insert(copy.clone());
                            surviving_remote.insert(copy);
                        }
                    }
                }
                surviving_local.insert(path.to_owned());
                surviving_remote.insert(path.to_owned());
            }
        }
    }

    // Resolve deferred Dir-vs-File collisions now that surviving descendants are known.
    let has_surviving_child = |dir: &str, set: &HashSet<String>| {
        let prefix = format!("{dir}/");
        set.iter().any(|p| p.starts_with(&prefix))
    };

    for tc in pending_type_conflicts {
        match tc {
            PendingDirConflict::LocalFileRemoteDir { path, loc_changed, rem_changed } => {
                let child_survives = has_surviving_child(path, &surviving_remote);
                if loc_changed && !rem_changed && !child_survives {
                    // Clean replacement of empty/deleted dir on host with desktop's file.
                    out.remote_remove.push(path.to_owned());
                    out.push_paths.push(path.to_owned());
                } else {
                    // Type conflict: keep directory at `path`, move desktop's file to conflict copy.
                    let copy = pick_unique_conflict_path(path, "desktop", stamp, &mut reserved);
                    let mode = remote.entries.get(path).map(|e| final_dir_mode(e.mode)).unwrap_or(0o755);
                    out.local_copy.push((path.to_owned(), copy.clone()));
                    out.local_remove.push(path.to_owned());
                    out.local_mkdir.push((path.to_owned(), mode));
                    out.push_paths.push(copy.clone());
                    out.conflicts.push(Conflict { path: path.to_owned(), copy: copy.clone(), winner: Winner::Host });
                    surviving_local.insert(copy.clone());
                    surviving_remote.insert(copy);
                }
                surviving_local.insert(path.to_owned());
                surviving_remote.insert(path.to_owned());
            }
            PendingDirConflict::RemoteFileLocalDir { path, loc_changed, rem_changed } => {
                let child_survives = has_surviving_child(path, &surviving_local);
                if !loc_changed && rem_changed && !child_survives {
                    // Clean replacement of empty/deleted dir on desktop with host's file.
                    out.local_remove.push(path.to_owned());
                    out.pull_paths.push(path.to_owned());
                } else {
                    // Type conflict: keep directory at `path`, move host's file to conflict copy.
                    let copy = pick_unique_conflict_path(path, host_tag, stamp, &mut reserved);
                    let mode = local.entries.get(path).map(|e| final_dir_mode(e.mode)).unwrap_or(0o755);
                    out.remote_copy.push((path.to_owned(), copy.clone()));
                    out.remote_remove.push(path.to_owned());
                    out.remote_mkdir.push((path.to_owned(), mode));
                    out.pull_paths.push(copy.clone());
                    out.conflicts.push(Conflict { path: path.to_owned(), copy: copy.clone(), winner: Winner::Desktop });
                    surviving_local.insert(copy.clone());
                    surviving_remote.insert(copy);
                }
                surviving_local.insert(path.to_owned());
                surviving_remote.insert(path.to_owned());
            }
        }
    }

    // Never remove a directory that is an ancestor of a surviving path on that side
    // (e.g. a directory deleted on one side while a new/edited file lives inside it on the other).
    let ancestors_of = |set: &HashSet<String>| -> HashSet<String> {
        let mut anc = HashSet::new();
        for p in set {
            let mut end = p.len();
            while let Some(slash) = p[..end].rfind('/') {
                anc.insert(p[..slash].to_owned());
                end = slash;
            }
        }
        anc
    };
    let local_anc = ancestors_of(&surviving_local);
    let remote_anc = ancestors_of(&surviving_remote);

    out.local_remove.retain(|p| {
        if local_anc.contains(p) && local.entries.get(p).is_some_and(is_dir_entry) {
            // Keep the directory on desktop and ensure it exists on host too.
            let mode = local.entries.get(p).map(|e| final_dir_mode(e.mode)).unwrap_or(0o755);
            out.remote_mkdir.push((p.clone(), mode));
            false
        } else {
            true
        }
    });
    out.remote_remove.retain(|p| {
        if remote_anc.contains(p) && remote.entries.get(p).is_some_and(is_dir_entry) {
            let mode = remote.entries.get(p).map(|e| final_dir_mode(e.mode)).unwrap_or(0o755);
            out.local_mkdir.push((p.clone(), mode));
            false
        } else {
            true
        }
    });

    // Sort removes deepest-first, mkdirs shallowest-first, transfers deterministically.
    let depth_desc = |a: &String, b: &String| b.matches('/').count().cmp(&a.matches('/').count()).then_with(|| b.cmp(a));
    let depth_asc = |a: &(String, u32), b: &(String, u32)| a.0.matches('/').count().cmp(&b.0.matches('/').count()).then_with(|| a.0.cmp(&b.0));

    out.local_remove.sort_by(depth_desc);
    out.local_remove.dedup();
    out.remote_remove.sort_by(depth_desc);
    out.remote_remove.dedup();
    out.local_mkdir.sort_by(depth_asc);
    out.local_mkdir.dedup_by(|a, b| a.0 == b.0);
    out.remote_mkdir.sort_by(depth_asc);
    out.remote_mkdir.dedup_by(|a, b| a.0 == b.0);
    out.pull_paths.sort();
    out.pull_paths.dedup();
    out.push_paths.sort();
    out.push_paths.dedup();
    out.conflicts.sort_by(|a, b| a.path.cmp(&b.path));

    out
}

/// Normalize an [`Entry`] for storage in a baseline index: hard links are
/// resolved to standalone regular files so removing the original link target
/// never leaves a dangling reference in the baseline.
fn normalize_entry(index: &Index, entry: &Entry, new_path: Option<&str>) -> Entry {
    let path = new_path.map(str::to_owned).unwrap_or_else(|| entry.path.clone());
    match &entry.kind {
        EntryKind::HardLink(target) => {
            if let Some(t) = index.entries.get(target)
                && matches!(t.kind, EntryKind::File)
            {
                return Entry { path, kind: EntryKind::File, size: t.size, mtime_ns: entry.mtime_ns, ctime_ns: 0, ino: 0, mode: t.mode & 0o777, hash: t.hash, chunks: Vec::new() };
            }
            Entry { path, kind: EntryKind::File, size: entry.size, mtime_ns: entry.mtime_ns, ctime_ns: 0, ino: 0, mode: entry.mode & 0o777, hash: entry.hash, chunks: Vec::new() }
        }
        EntryKind::File => Entry { path, kind: EntryKind::File, size: entry.size, mtime_ns: entry.mtime_ns, ctime_ns: 0, ino: 0, mode: entry.mode & 0o777, hash: entry.hash, chunks: Vec::new() },
        EntryKind::Dir => Entry { path, kind: EntryKind::Dir, size: 0, mtime_ns: entry.mtime_ns, ctime_ns: 0, ino: 0, mode: final_dir_mode(entry.mode), hash: [0u8; 32], chunks: Vec::new() },
        EntryKind::Symlink(t) => {
            Entry { path, kind: EntryKind::Symlink(t.clone()), size: entry.size, mtime_ns: entry.mtime_ns, ctime_ns: 0, ino: 0, mode: entry.mode & 0o777, hash: entry.hash, chunks: Vec::new() }
        }
    }
}

/// Compute the exact post-sync baseline index after `plan` has succeeded with
/// zero errors. Built deterministically from the pre-sync snapshots so any edit
/// written to disk *during* the transfer is NOT falsely absorbed into the
/// baseline and will be synced on the next pass.
pub fn post_sync_baseline(local: &Index, remote: &Index, plan: &TwoWayPlan) -> Index {
    let mut entries = BTreeMap::new();
    for (k, v) in &local.entries {
        entries.insert(k.clone(), normalize_entry(local, v, None));
    }

    // 1. Local copies (desktop losers preserved).
    for (src, dst) in &plan.local_copy {
        if let Some(orig) = local.entries.get(src) {
            entries.insert(dst.clone(), normalize_entry(local, orig, Some(dst)));
        }
    }

    // 2. Local removals (except paths immediately recreated by local_mkdir or pull_paths).
    for p in &plan.local_remove {
        entries.remove(p);
    }

    // 3. Local mkdirs (and remote mkdirs that created dirs on host — already in local or added here).
    for (p, mode) in &plan.local_mkdir {
        let mtime = remote.entries.get(p).map(|e| e.mtime_ns).unwrap_or(0);
        entries.insert(p.clone(), Entry { path: p.clone(), kind: EntryKind::Dir, size: 0, mtime_ns: mtime, ctime_ns: 0, ino: 0, mode: final_dir_mode(*mode), hash: [0u8; 32], chunks: Vec::new() });
    }

    // 4. Pulled files/symlinks (including conflict copies created on host by `remote_copy`).
    let remote_copy_src: BTreeMap<&str, &str> = plan.remote_copy.iter().map(|(s, d)| (d.as_str(), s.as_str())).collect();
    for p in &plan.pull_paths {
        let src_key = remote_copy_src.get(p.as_str()).copied().unwrap_or(p.as_str());
        if let Some(re) = remote.entries.get(src_key) {
            entries.insert(p.clone(), normalize_entry(remote, re, Some(p)));
        }
        // Ensure ancestor dirs created implicitly by receive are present in baseline too.
        let mut end = p.len();
        while let Some(slash) = p[..end].rfind('/') {
            let anc = &p[..slash];
            if !entries.contains_key(anc) {
                let mode = remote.entries.get(anc).map(|e| final_dir_mode(e.mode)).unwrap_or(0o755);
                entries.insert(anc.to_owned(), Entry { path: anc.to_owned(), kind: EntryKind::Dir, size: 0, mtime_ns: 0, ctime_ns: 0, ino: 0, mode, hash: [0u8; 32], chunks: Vec::new() });
            }
            end = slash;
        }
    }

    Index { entries, ..Index::default() }
}

/// Ensure no component along `rel`'s parent directories under `root` is a symlink.
fn check_no_symlink_ancestors(root: &Path, rel: &str) -> io::Result<()> {
    let mut cur = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for seg in &parts[..parts.len().saturating_sub(1)] {
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("symlink ancestor refused: {}", cur.display())));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("not a directory: {}", cur.display())));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn set_mtime_nofollow(path: &Path, mtime_ns: i64) {
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else { return };
    let secs = mtime_ns.div_euclid(1_000_000_000);
    let nsecs = mtime_ns.rem_euclid(1_000_000_000);
    let times = [libc::timespec { tv_sec: secs, tv_nsec: nsecs }, libc::timespec { tv_sec: secs, tv_nsec: nsecs }];
    // SAFETY: `cpath` is NUL-terminated and `times` is a valid 2-element array.
    unsafe {
        libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW);
    }
}

fn copy_entry_confined(root: &Path, src: &str, dst: &str) -> io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    validate_rel_path(src).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{src}: {e}")))?;
    validate_rel_path(dst).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{dst}: {e}")))?;
    check_no_symlink_ancestors(root, src)?;
    check_no_symlink_ancestors(root, dst)?;

    let src_path = root.join(src);
    let dst_path = root.join(dst);
    let meta = fs::symlink_metadata(&src_path)?;
    if let Some(parent) = dst_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!("{PART_PREFIX}{:08x}{:08x}", std::process::id(), seq);
    let tmp_path = dst_path.parent().unwrap_or(root).join(tmp_name);

    let res = (|| -> io::Result<()> {
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&src_path)?;
            symlink(&target, &tmp_path)?;
            set_mtime_nofollow(&tmp_path, mtime_ns);
            fs::rename(&tmp_path, &dst_path)?;
            return Ok(());
        }
        if !meta.file_type().is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{src}: not a regular file or symlink")));
        }
        let mut reader = fs::File::open(&src_path)?;
        let mut writer = fs::OpenOptions::new().write(true).create_new(true).open(&tmp_path)?;
        io::copy(&mut reader, &mut writer)?;
        drop(writer);
        let mode = meta.permissions().mode() & 0o777;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode));
        set_mtime_nofollow(&tmp_path, mtime_ns);
        fs::rename(&tmp_path, &dst_path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    res
}

/// Apply local mutations (copies first, then deepest-first removes, then
/// shallowest-first mkdirs) confined strictly under `root`.
/// Used identically on desktop and inside `olo-rex-link sync mutate` on a host.
pub fn apply_mutations(root: &Path, copies: &[(String, String)], removes: &[String], mkdirs: &[(String, u32)]) -> io::Result<()> {
    fs::create_dir_all(root)?;

    // 1. Copies (preserve losing files before anything is removed or overwritten).
    for (src, dst) in copies {
        copy_entry_confined(root, src, dst)?;
    }

    // 2. Removals, deepest-first (never removes non-empty dirs holding excluded files).
    let mut rem_sorted = removes.to_vec();
    rem_sorted.sort_by(|a, b| b.matches('/').count().cmp(&a.matches('/').count()).then_with(|| b.cmp(a)));
    rem_sorted.dedup();
    for rel in &rem_sorted {
        validate_rel_path(rel).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{rel}: {e}")))?;
        check_no_symlink_ancestors(root, rel)?;
        let full = root.join(rel);
        let meta = match fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if meta.file_type().is_dir() {
            match fs::remove_dir(&full) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::DirectoryNotEmpty => {}
                Err(e) => return Err(e),
            }
        } else {
            match fs::remove_file(&full) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }

    // 3. Directory creations / chmods, shallowest-first.
    let mut mk_sorted = mkdirs.to_vec();
    mk_sorted.sort_by(|a, b| a.0.matches('/').count().cmp(&b.0.matches('/').count()).then_with(|| a.0.cmp(&b.0)));
    mk_sorted.dedup_by(|a, b| a.0 == b.0);
    for (rel, mode) in &mk_sorted {
        validate_rel_path(rel).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{rel}: {e}")))?;
        check_no_symlink_ancestors(root, rel)?;
        let full = root.join(rel);
        fs::create_dir_all(&full)?;
        if *mode != 0 {
            let _ = fs::set_permissions(&full, fs::Permissions::from_mode(final_dir_mode(*mode)));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file_entry(path: &str, content: &[u8], mtime_ns: i64, mode: u32) -> Entry {
        Entry {
            path: path.to_owned(),
            kind: EntryKind::File,
            size: content.len() as u64,
            mtime_ns,
            ctime_ns: mtime_ns,
            ino: 1,
            mode: mode & 0o777,
            hash: *blake3::hash(content).as_bytes(),
            chunks: Vec::new(),
        }
    }

    fn dir_entry(path: &str, mtime_ns: i64, mode: u32) -> Entry {
        Entry { path: path.to_owned(), kind: EntryKind::Dir, size: 0, mtime_ns, ctime_ns: mtime_ns, ino: 2, mode: final_dir_mode(mode), hash: [0u8; 32], chunks: Vec::new() }
    }

    fn idx(entries: Vec<Entry>) -> Index {
        let mut map = BTreeMap::new();
        for e in entries {
            map.insert(e.path.clone(), e);
        }
        Index { entries: map, ..Index::default() }
    }

    #[test]
    fn conflict_naming_and_detection() {
        assert_eq!(conflict_name("notes.md", "My Build Box", "20261125-095500", None), "notes.conflict-my-build-box-20261125-095500.md");
        assert_eq!(conflict_name("dir/Makefile", "host", "20261125-095500", None), "dir/Makefile.conflict-host-20261125-095500");
        assert_eq!(conflict_name(".gitignore", "desktop", "20261125-095500", Some(2)), ".gitignore.conflict-desktop-20261125-095500-2");
        assert!(is_conflict_path("dir/notes.conflict-host-20261125-095500.md"));
        assert!(is_conflict_path(".gitignore.conflict-desktop-20261125-095500-2"));
        assert!(!is_conflict_path("dir/notes.md"));
        assert!(!is_conflict_path("conflict-notes.txt"));

        // Long filename stays <= 255 bytes.
        let long = format!("{}.txt", "a".repeat(250));
        let cname = conflict_name(&long, "host", "20261125-095500", None);
        assert!(cname.len() <= 255, "len={}", cname.len());
        assert!(cname.ends_with(".txt"));
    }

    #[test]
    fn one_sided_adds_edits_and_deletes_propagate() {
        let base = idx(vec![
            file_entry("keep.txt", b"same", 10, 0o644),
            file_entry("edit_loc.txt", b"v1", 10, 0o644),
            file_entry("edit_rem.txt", b"v1", 10, 0o644),
            file_entry("del_loc.txt", b"gone", 10, 0o644),
            file_entry("del_rem.txt", b"gone", 10, 0o644),
        ]);
        let local = idx(vec![
            file_entry("keep.txt", b"same", 10, 0o644),
            file_entry("edit_loc.txt", b"v2-local", 20, 0o644),
            file_entry("edit_rem.txt", b"v1", 10, 0o644),
            file_entry("del_rem.txt", b"gone", 10, 0o644),
            file_entry("add_loc.txt", b"new-local", 20, 0o644),
        ]);
        let remote = idx(vec![
            file_entry("keep.txt", b"same", 10, 0o644),
            file_entry("edit_loc.txt", b"v1", 10, 0o644),
            file_entry("edit_rem.txt", b"v2-remote", 20, 0o644),
            file_entry("del_loc.txt", b"gone", 10, 0o644),
            file_entry("add_rem.txt", b"new-remote", 20, 0o644),
        ]);

        let p = plan(Some(&base), &local, &remote, "far", "20261125-100000");
        assert_eq!(p.push_paths, vec!["add_loc.txt", "edit_loc.txt"]);
        assert_eq!(p.pull_paths, vec!["add_rem.txt", "edit_rem.txt"]);
        assert_eq!(p.local_remove, vec!["del_rem.txt"]);
        assert_eq!(p.remote_remove, vec!["del_loc.txt"]);
        assert!(p.conflicts.is_empty());

        // Post-sync baseline equals the converged tree.
        let next = post_sync_baseline(&local, &remote, &p);
        assert!(next.entries.contains_key("add_loc.txt"));
        assert!(next.entries.contains_key("add_rem.txt"));
        assert!(!next.entries.contains_key("del_loc.txt"));
        assert!(!next.entries.contains_key("del_rem.txt"));
        assert_eq!(next.entries["edit_loc.txt"].hash, *blake3::hash(b"v2-local").as_bytes());
        assert_eq!(next.entries["edit_rem.txt"].hash, *blake3::hash(b"v2-remote").as_bytes());
    }

    #[test]
    fn edit_vs_edit_preserves_both_versions_with_newer_in_place() {
        let base = idx(vec![file_entry("a.md", b"base", 10, 0o644), file_entry("b.md", b"base", 10, 0o644)]);
        let local = idx(vec![file_entry("a.md", b"desktop-newer", 30, 0o644), file_entry("b.md", b"desktop-older", 15, 0o644)]);
        let remote = idx(vec![file_entry("a.md", b"host-older", 20, 0o644), file_entry("b.md", b"host-newer", 25, 0o644)]);

        let p = plan(Some(&base), &local, &remote, "build", "20261125-100000");
        assert_eq!(p.remote_copy, vec![("a.md".into(), "a.conflict-build-20261125-100000.md".into())]);
        assert_eq!(p.local_copy, vec![("b.md".into(), "b.conflict-desktop-20261125-100000.md".into())]);
        assert_eq!(p.push_paths, vec!["a.md", "b.conflict-desktop-20261125-100000.md"]);
        assert_eq!(p.pull_paths, vec!["a.conflict-build-20261125-100000.md", "b.md"]);
        assert_eq!(p.conflicts.len(), 2);
        assert_eq!(p.conflicts[0].winner, Winner::Desktop);
        assert_eq!(p.conflicts[1].winner, Winner::Host);

        let next = post_sync_baseline(&local, &remote, &p);
        assert_eq!(next.entries["a.md"].hash, *blake3::hash(b"desktop-newer").as_bytes());
        assert_eq!(next.entries["a.conflict-build-20261125-100000.md"].hash, *blake3::hash(b"host-older").as_bytes());
        assert_eq!(next.entries["b.md"].hash, *blake3::hash(b"host-newer").as_bytes());
        assert_eq!(next.entries["b.conflict-desktop-20261125-100000.md"].hash, *blake3::hash(b"desktop-older").as_bytes());
    }

    #[test]
    fn edit_vs_delete_keeps_edited_file_and_identical_edits_do_nothing() {
        let base = idx(vec![file_entry("ed_loc.txt", b"v1", 10, 0o644), file_entry("ed_rem.txt", b"v1", 10, 0o644), file_entry("both_same.txt", b"v1", 10, 0o644)]);
        let local = idx(vec![file_entry("ed_loc.txt", b"saved-edit", 20, 0o644), file_entry("both_same.txt", b"v2", 20, 0o644)]);
        let remote = idx(vec![file_entry("ed_rem.txt", b"saved-remote", 20, 0o644), file_entry("both_same.txt", b"v2", 25, 0o644)]);

        let p = plan(Some(&base), &local, &remote, "far", "20261125-100000");
        assert_eq!(p.push_paths, vec!["ed_loc.txt"]);
        assert_eq!(p.pull_paths, vec!["ed_rem.txt"]);
        assert!(p.local_remove.is_empty() && p.remote_remove.is_empty() && p.conflicts.is_empty());
    }

    #[test]
    fn dir_vs_file_conflict_keeps_directory_and_preserves_file() {
        let local = idx(vec![file_entry("box", b"i am a file", 20, 0o644)]);
        let remote = idx(vec![dir_entry("box", 10, 0o755), file_entry("box/inside.txt", b"child", 10, 0o644)]);

        let p = plan(None, &local, &remote, "far", "20261125-100000");
        assert_eq!(p.local_copy, vec![("box".into(), "box.conflict-desktop-20261125-100000".into())]);
        assert_eq!(p.local_remove, vec!["box"]);
        assert_eq!(p.local_mkdir, vec![("box".into(), 0o755)]);
        assert_eq!(p.pull_paths, vec!["box/inside.txt"]);
        assert_eq!(p.push_paths, vec!["box.conflict-desktop-20261125-100000"]);
        assert_eq!(p.conflicts.len(), 1);
    }

    #[test]
    fn deleted_dir_on_one_side_preserved_when_child_added_on_other() {
        let base = idx(vec![dir_entry("d", 10, 0o755), file_entry("d/old.txt", b"old", 10, 0o644)]);
        let local = idx(vec![dir_entry("d", 10, 0o755), file_entry("d/old.txt", b"old", 10, 0o644), file_entry("d/new.txt", b"new", 20, 0o644)]);
        let remote = idx(vec![]);

        let p = plan(Some(&base), &local, &remote, "far", "20261125-100000");
        assert_eq!(p.local_remove, vec!["d/old.txt"]);
        assert_eq!(p.remote_mkdir, vec![("d".into(), 0o755)]);
        assert_eq!(p.push_paths, vec!["d/new.txt"]);
    }

    #[test]
    fn unread_entries_are_never_deleted() {
        let base = idx(vec![file_entry("secret.txt", b"x", 10, 0o600)]);
        let local = idx(vec![file_entry("secret.txt", b"x", 10, 0o600)]);
        let mut remote = idx(vec![]);
        remote.unread.insert("secret.txt".into(), "permission denied".into());

        let p = plan(Some(&base), &local, &remote, "far", "20261125-100000");
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn apply_mutations_copies_removes_and_mkdirs_safely() {
        let root = PathBuf::from(format!("/var/tmp/olo-rex-sync-twoway-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/a.txt"), b"original").unwrap();
        fs::write(root.join("sub/keep.tmp"), b"excluded").unwrap();

        apply_mutations(&root, &[("sub/a.txt".into(), "sub/a.conflict-desktop-20261125-100000.txt".into())], &["sub/a.txt".into(), "sub".into()], &[("fresh/nested".into(), 0o755)]).unwrap();

        assert!(!root.join("sub/a.txt").exists());
        assert_eq!(fs::read(root.join("sub/a.conflict-desktop-20261125-100000.txt")).unwrap(), b"original");
        // Non-empty dir kept safely.
        assert_eq!(fs::read(root.join("sub/keep.tmp")).unwrap(), b"excluded");
        assert!(root.join("fresh/nested").is_dir());
        fs::remove_dir_all(root).unwrap();
    }
}
