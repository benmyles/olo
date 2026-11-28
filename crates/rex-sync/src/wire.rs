//! Framed wire session over any `Read + Write` stream (`UnixStream`, TCP,
//! SSH `exec_io`, QUIC stream): version handshake, tree-hash fast path,
//! compressed manifest exchange, append-probe / 2-level super-chunk
//! negotiation, pipelined streaming recipes with atomic `.olo-sync-part`
//! commits and full BLAKE3 verification, and strict path confinement to `root`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::chunk::{self, ChunkId, ChunkRef, StreamDigest};
use crate::index::{Entry, EntryKind, Filter, Index, MAX_SKIPPED_PATHS, PART_PREFIX, Skipped, SkippedKind, dir_mode, final_dir_mode, validate_rel_path};
use crate::recipe::{self, AppendProbe, DataCodec, MAX_DATA_PIECE_LEN, Piece, Plan, REF_PREFIX_MAX, RecipeKind, SuperChunk};

/// Wire protocol version exchanged in the opening handshake.
pub const WIRE_VERSION: u32 = 3;

/// Maximum allowed frame payload length (8 MiB). Any frame advertising a
/// larger payload is rejected immediately before allocating.
pub const MAX_FRAME_LEN: usize = 8 << 20;

/// Maximum number of chunks in a file for which the receiver sends leaf chunks
/// directly instead of a 2-level [`SuperChunk`] summary (~1 MiB file).
const DIRECT_LEAF_CHUNK_LIMIT: usize = 64;

/// Maximum uncompressed payload bundled into a single `Manifest` frame.
const MAX_MANIFEST_CHUNK_RAW: usize = 1 << 20;

/// Maximum length of a single path or symlink target on the wire.
const MAX_PATH_LEN: usize = 4096;

/// Maximum entries accepted in a single `Manifest` chunk or chunk-list frame.
const MAX_ITEMS_PER_FRAME: usize = 250_000;
/// The most entries a peer's list holds over all its frames (a manifest,
/// what it asks of chunks): one that never says "no more" can't grow this
/// side's memory for ever.
const MAX_ITEMS: usize = 20_000_000;
/// The most chunks an append's tail is said in (about 4 GiB of it): more,
/// and the file is built beside its place instead.
const MAX_TAIL_CHUNKS: usize = 256 << 10;

const FLAG_DELETE_EXTRA: u8 = 1 << 0;
const FLAG_KEEP_NEWER: u8 = 1 << 1;
/// The receiver looks only at what the sender names (`Options::scoped`).
const FLAG_SCOPED: u8 = 1 << 2;
/// The receiver asks the sender to confirm its skipped entries and sender-side
/// errors in a `TAG_SENDER_SUMMARY` frame before finishing.
const FLAG_SENDER_SUMMARY: u8 = 1 << 3;

// Frame tags.
const TAG_HELLO: u8 = 1;
const TAG_HELLO_ACK: u8 = 2;
const TAG_MANIFEST: u8 = 3;
const TAG_NEED_CHUNKS: u8 = 4;
const TAG_CHUNK_INFO: u8 = 5;
const TAG_NEED_LEAF_CHUNKS: u8 = 6;
const TAG_LEAF_CHUNKS: u8 = 7;
/// The sender's paths, to a scoped receiver.
const TAG_SCOPE: u8 = 8;
const TAG_MKDIR: u8 = 10;
const TAG_SYMLINK: u8 = 11;
const TAG_SET_META: u8 = 12;
const TAG_COPY_FILE: u8 = 13;
const TAG_FILE_START: u8 = 14;
const TAG_FILE_PIECE: u8 = 15;
const TAG_FILE_END: u8 = 16;
const TAG_REMOVE: u8 = 17;
const TAG_DONE: u8 = 18;
const TAG_REPORT: u8 = 19;
const TAG_HARDLINK: u8 = 20;
const TAG_FILE_ABORT: u8 = 21;
/// Sender-side completeness summary (errors + skipped entries) delivered to a
/// receiver that requested `FLAG_SENDER_SUMMARY`.
const TAG_SENDER_SUMMARY: u8 = 22;

const REASON_PROTECTED: &str = "kept: the receiver's own";
const REASON_CHANGED: &str = "changed while it was sent";

const SPARSE_BLOCK: usize = 4096;
static ZEROS: [u8; 64 << 10] = [0u8; 64 << 10];

/// How often the receiver checkpoints its index mid-session: every 30
/// seconds at most, and never sooner than twenty times its last save took
/// (an index of millions of entries takes seconds to write whole: saved by
/// files or bytes received, a fast session spent most of its time saving).
/// So saving is at most a twentieth of a session's time, whatever its size.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(30);
const CHECKPOINT_SAVES_APART: u32 = 20;
/// A checkpoint by files received (tests only: none in a real session).
const CHECKPOINT_FILES: u64 = u64::MAX;

/// What the sender can offer: an index of entries and random-access reads of
/// file slices.
pub trait Source {
    fn index(&self) -> io::Result<Index>;

    /// Like [`Source::index`], calling `on_entry(path, size)` as each regular
    /// file is indexed so progress is reported from the start of a session.
    fn index_with_progress(&self, _on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Index> {
        self.index()
    }

    /// Like [`Source::index_with_progress`], aborting promptly with
    /// [`io::ErrorKind::ConnectionAborted`] when `cancel` becomes true.
    fn index_with_progress_cancel(&self, on_entry: &mut dyn FnMut(&str, u64), _cancel: Option<&AtomicBool>) -> io::Result<Index> {
        self.index_with_progress(on_entry)
    }

    /// Optional stat-only index pass: returns `(index, unhashed_paths)` where
    /// uncached regular files in `unhashed_paths` have not been read yet.
    /// Brand-new files are then streamed once with their BLAKE3 computed on the
    /// fly and sent in `TAG_FILE_END`. Default: `Ok(None)` (eager indexing).
    fn index_lazy_with_progress(&self, _on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Option<(Index, HashSet<String>)>> {
        Ok(None)
    }

    /// Like [`Source::index_lazy_with_progress`], aborting promptly with
    /// [`io::ErrorKind::ConnectionAborted`] when `cancel` becomes true.
    fn index_lazy_with_progress_cancel(&self, on_entry: &mut dyn FnMut(&str, u64), _cancel: Option<&AtomicBool>) -> io::Result<Option<(Index, HashSet<String>)>> {
        self.index_lazy_with_progress(on_entry)
    }

    /// Hash (and chunk when `size >= chunk::CHUNK_MIN_SIZE`) a single unhashed
    /// file on demand when the receiver already has a file at this path or of
    /// this size.
    fn hash_entry(&self, _path: &str, _size: u64) -> io::Result<chunk::FileDigest> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "lazy hashing not supported by this Source"))
    }

    /// Whether [`Source::save_cache`] persists the index (used to skip
    /// computing sender-side chunk lists for brand-new files when no cache is
    /// configured).
    fn wants_cache(&self) -> bool {
        false
    }

    /// Save the completed index cache at the end of a lazy session.
    fn save_cache(&self, _index: &Index) -> io::Result<()> {
        Ok(())
    }

    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>>;

    /// Whether `entry` on the source still matches its indexed stat (size,
    /// mtime, ctime, inode). Returning `false` aborts that file alone with
    /// `"changed while it was sent"`.
    fn check_unchanged(&self, _entry: &Entry) -> bool {
        true
    }

    /// Probe `[offset, offset + max_len)` of `path` for a filesystem hole
    /// (`SEEK_DATA`/`SEEK_HOLE`): returns `(true, hole_len)` if it starts with
    /// a hole, or `(false, data_len)` for a data run.
    fn extent_at(&self, _path: &str, _offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
        Ok((false, max_len))
    }
}

/// A [`Source`] backed by a local folder, with an optional on-disk [`Index`]
/// cache so unchanged trees are scanned without reading file contents.
pub struct FolderSource {
    pub root: PathBuf,
    pub filter: Filter,
    pub cache_path: Option<PathBuf>,
}

impl FolderSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), filter: Filter::default(), cache_path: None }
    }
}

impl Source for FolderSource {
    fn index(&self) -> io::Result<Index> {
        self.index_with_progress(&mut |_, _| {})
    }

    fn index_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Index> {
        self.index_with_progress_cancel(on_entry, None)
    }

    fn index_with_progress_cancel(&self, on_entry: &mut dyn FnMut(&str, u64), cancel: Option<&AtomicBool>) -> io::Result<Index> {
        let prev = self.cache_path.as_deref().and_then(|p| Index::load(p).ok());
        let idx = Index::scan_with_progress_cancel(&self.root, &self.filter, prev.as_ref(), on_entry, cancel)?;
        if let Some(cache) = &self.cache_path {
            idx.save(cache)?;
        }
        Ok(idx)
    }

    fn index_lazy_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Option<(Index, HashSet<String>)>> {
        self.index_lazy_with_progress_cancel(on_entry, None)
    }

    fn index_lazy_with_progress_cancel(&self, on_entry: &mut dyn FnMut(&str, u64), cancel: Option<&AtomicBool>) -> io::Result<Option<(Index, HashSet<String>)>> {
        // Use stat-only lazy indexing whenever a cache is configured OR when
        // scoped via `filter.only` (single-file transfers never use the
        // tree-hash fast path, and lazy indexing avoids pre-handshake hashing
        // of large files before streaming).
        if self.cache_path.is_none() && self.filter.only.is_empty() {
            return Ok(None);
        }
        let prev = self.cache_path.as_deref().and_then(|p| Index::load(p).ok());
        let (idx, unhashed) = Index::scan_stat_with_progress_cancel(&self.root, &self.filter, prev.as_ref(), on_entry, cancel)?;
        Ok(Some((idx, unhashed)))
    }

    fn hash_entry(&self, path: &str, size: u64) -> io::Result<chunk::FileDigest> {
        validate_rel_path(path).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut f = open_no_symlink_components(&self.root, path)?;
        chunk::hash_and_chunk_file_in(&mut Vec::new(), &mut f, size)
    }

    fn wants_cache(&self) -> bool {
        self.cache_path.is_some()
    }

    fn save_cache(&self, index: &Index) -> io::Result<()> {
        if let Some(cache) = &self.cache_path {
            index.save(cache)?;
        }
        Ok(())
    }

    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        validate_rel_path(path).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut f = open_no_symlink_components(&self.root, path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        let mut filled = 0usize;
        while filled < len {
            match f.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        buf.truncate(filled);
        Ok(buf)
    }

    fn check_unchanged(&self, entry: &Entry) -> bool {
        if validate_rel_path(&entry.path).is_err() {
            return false;
        }
        let Ok(meta) = fs::symlink_metadata(self.root.join(&entry.path)) else {
            return false;
        };
        if !meta.file_type().is_file() {
            return false;
        }
        let mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
        let ctime_ns = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
        meta.len() == entry.size && meta.ino() == entry.ino && mtime_ns == entry.mtime_ns && ctime_ns == entry.ctime_ns
    }

    fn extent_at(&self, path: &str, offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
        validate_rel_path(path).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let f = open_no_symlink_components(&self.root, path)?;
        let actual_len = f.metadata()?.len();
        if actual_len < offset.saturating_add(max_len) {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file shrunk"));
        }
        Ok(chunk::probe_extent(&f, offset, max_len))
    }
}

/// A [`Source`] backed by a local folder that ALWAYS performs stat-only lazy
/// indexing even with `cache_path: None` (ideal for uploading a folder into a
/// brand-new empty remote stage without pre-hashing the whole tree upfront).
pub struct LazyFolderSource {
    inner: FolderSource,
}

impl LazyFolderSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { inner: FolderSource::new(root) }
    }
}

impl Source for LazyFolderSource {
    fn index(&self) -> io::Result<Index> {
        self.inner.index()
    }

    fn index_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Index> {
        self.inner.index_with_progress(on_entry)
    }

    fn index_with_progress_cancel(&self, on_entry: &mut dyn FnMut(&str, u64), cancel: Option<&AtomicBool>) -> io::Result<Index> {
        self.inner.index_with_progress_cancel(on_entry, cancel)
    }

    fn index_lazy_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Option<(Index, HashSet<String>)>> {
        self.index_lazy_with_progress_cancel(on_entry, None)
    }

    fn index_lazy_with_progress_cancel(&self, on_entry: &mut dyn FnMut(&str, u64), cancel: Option<&AtomicBool>) -> io::Result<Option<(Index, HashSet<String>)>> {
        let (idx, unhashed) = Index::scan_stat_with_progress_cancel(&self.inner.root, &self.inner.filter, None, on_entry, cancel)?;
        Ok(Some((idx, unhashed)))
    }

    fn hash_entry(&self, path: &str, size: u64) -> io::Result<chunk::FileDigest> {
        self.inner.hash_entry(path, size)
    }

    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.inner.read(path, offset, len)
    }

    fn check_unchanged(&self, entry: &Entry) -> bool {
        self.inner.check_unchanged(entry)
    }

    fn extent_at(&self, path: &str, offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
        self.inner.extent_at(path, offset, max_len)
    }
}

/// Metadata for one file provided by [`ListSource`].
#[derive(Debug, Clone)]
pub struct ListEntry {
    pub path: String,
    pub mode: u32,
    pub mtime_ns: i64,
}

impl ListEntry {
    pub fn new(path: impl Into<String>, mode: u32, mtime_ns: i64) -> Self {
        Self { path: path.into(), mode: mode & 0o777, mtime_ns }
    }
}

type BytesProvider = Box<dyn Fn(&str) -> io::Result<Vec<u8>> + Send>;

/// A [`Source`] backed by an explicit list of files whose bytes are supplied
/// by a caller-provided closure (e.g. files whose content is transformed
/// before syncing). Every file's bytes are held for the session: for a few
/// small files; a large set is better written out and sent as a folder.
pub struct ListSource {
    items: Vec<ListEntry>,
    provider: BytesProvider,
    cache: Mutex<HashMap<String, Vec<u8>>>,
}

impl ListSource {
    pub fn new(items: Vec<ListEntry>, provider: impl Fn(&str) -> io::Result<Vec<u8>> + Send + 'static) -> Self {
        Self { items, provider: Box::new(provider), cache: Mutex::new(HashMap::new()) }
    }
}

impl Source for ListSource {
    fn index(&self) -> io::Result<Index> {
        self.index_with_progress(&mut |_, _| {})
    }

    fn index_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Index> {
        let mut entries = BTreeMap::new();
        let mut buf = Vec::new();
        let mut cache = self.cache.lock().unwrap();
        for item in &self.items {
            let bytes = (self.provider)(&item.path)?;
            let digest = if bytes.len() as u64 >= chunk::CHUNK_MIN_SIZE { chunk::hash_and_chunk_in(&mut buf, &bytes[..])? } else { chunk::hash_only_in(&mut buf, &bytes[..])? };
            on_entry(&item.path, digest.size);
            cache.insert(item.path.clone(), bytes);
            entries.insert(
                item.path.clone(),
                Entry {
                    path: item.path.clone(),
                    kind: EntryKind::File,
                    size: digest.size,
                    mtime_ns: item.mtime_ns,
                    ctime_ns: 0,
                    ino: 0,
                    mode: item.mode & 0o777,
                    hash: digest.hash,
                    chunks: digest.chunks,
                },
            );
        }
        Ok(Index { entries, ..Index::default() })
    }

    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.cache.lock().unwrap().get(path) {
            let start = (offset as usize).min(bytes.len());
            let end = start.saturating_add(len).min(bytes.len());
            return Ok(bytes[start..end].to_vec());
        }
        let bytes = (self.provider)(path)?;
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(len).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }
}

/// Stage of a sync session reported through [`Options::progress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Walking the source tree (files and bytes looked at so far).
    Indexing,
    /// Diff against the receiver is complete; totals reflect what will be sent.
    Planned,
    /// Streaming planned files to the receiver.
    Sending,
    /// Session finished.
    Done,
}

/// Live progress snapshot delivered to [`Options::progress`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub files: u64,
    pub bytes: u64,
    pub files_total: u64,
    pub bytes_total: u64,
    pub current: String,
}

/// Callback type for [`Options::progress`].
pub type ProgressCallback = Arc<dyn Fn(&Progress) + Send + Sync>;

/// Options governing a sync session.
#[derive(Clone, Default)]
pub struct Options {
    /// Remove entries on the receiver that the sender does not have.
    /// Default: `false`. Enforced by the receiver so an untrusted sender
    /// cannot delete files unless the receiver opts in.
    pub delete_extra: bool,
    /// Leave a file on the receiver untouched if its `mtime_ns` is strictly
    /// newer than the sender's file. Default: `false`.
    pub keep_newer: bool,
    /// Path filter applied to the tree. Default: recursive, all entries.
    pub filter: Filter,
    /// Optional cache file path used by [`receive`] to persist its [`Index`]
    /// across sessions so unchanged trees are scanned with zero file reads.
    pub cache_path: Option<PathBuf>,
    /// The receiver looks only at the paths the sender names, nothing else
    /// of its root (a root that is a whole home: Agent Sessions' copy puts
    /// a set of files under it), and takes nothing away. Told to the
    /// sender in the receiver's hello; the sender's own is unused.
    pub scoped: bool,
    /// Root-relative paths the receiver keeps as its own (a file, or a folder
    /// and everything under it): every operation that would create, replace,
    /// change or remove one is refused with `"kept: the receiver's own"`.
    pub protect: Vec<String>,
    /// Optional observer called by [`send`] no more than ten times a second
    /// within a phase and once at each phase's end.
    pub progress: Option<ProgressCallback>,
    /// Receiver option: ask the sender to transmit its skipped-entry and
    /// sender-error summary on the wire (`FLAG_SENDER_SUMMARY`) so the receiver
    /// can confirm a complete copy via [`Report::sender_confirmed`]. Older
    /// senders ignore the flag and leave `sender_confirmed == false`.
    pub request_sender_summary: bool,
    /// Optional cancellation flag checked during local indexing and sending so
    /// a stopped transfer aborts promptly even before transport I/O blocks.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("delete_extra", &self.delete_extra)
            .field("keep_newer", &self.keep_newer)
            .field("filter", &self.filter)
            .field("cache_path", &self.cache_path)
            .field("scoped", &self.scoped)
            .field("protect", &self.protect)
            .field("progress", &self.progress.as_ref().map(|_| ".."))
            .field("request_sender_summary", &self.request_sender_summary)
            .field("cancel", &self.cancel.as_ref().map(|c| c.load(Ordering::Relaxed)))
            .finish()
    }
}

const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(100);

struct ProgressReporter<'a> {
    cb: Option<&'a (dyn Fn(&Progress) + Send + Sync)>,
    last_emit: Option<Instant>,
}

impl<'a> ProgressReporter<'a> {
    fn new(cb: Option<&'a ProgressCallback>) -> Self {
        Self { cb: cb.map(|f| f.as_ref()), last_emit: None }
    }

    fn maybe_emit(&mut self, p: &Progress) {
        let Some(cb) = self.cb else { return };
        if self.last_emit.is_none_or(|t| t.elapsed() >= PROGRESS_MIN_INTERVAL) {
            cb(p);
            self.last_emit = Some(Instant::now());
        }
    }

    fn emit_now(&mut self, p: &Progress) {
        let Some(cb) = self.cb else { return };
        cb(p);
        self.last_emit = Some(Instant::now());
    }
}

/// Per-path failure recorded in a [`Report`] without aborting the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError {
    pub path: String,
    pub reason: String,
}

/// Summary of a completed sync session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub same: u64,
    pub copied: u64,
    pub appended: u64,
    pub patched: u64,
    pub new: u64,
    pub removed: u64,
    pub left_newer: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub data_bytes_raw: u64,
    pub data_bytes_compressed: u64,
    pub errors: Vec<PathError>,
    /// Everything the sender's scan passed over (unread, non-UTF-8 names,
    /// special files, mounts on other filesystems), counted and named.
    pub skipped: Skipped,
    /// Whether the sender explicitly confirmed its final error/skipped summary
    /// on the wire (when [`Options::request_sender_summary`] was set). Always
    /// `true` on the sender's own returned report.
    pub sender_confirmed: bool,
}

impl Report {
    /// Total bytes transmitted across the wire in both directions.
    pub fn wire_bytes(&self) -> u64 {
        self.bytes_sent + self.bytes_received
    }
}

/// Sender entry point: sync everything from `source` to the peer on `conn`.
pub fn send<S: Read + Write>(conn: S, source: &dyn Source, options: Options) -> io::Result<Report> {
    let mut progress = ProgressReporter::new(options.progress.as_ref());
    let mut indexed_files = 0u64;
    let mut indexed_bytes = 0u64;
    let mut on_index_entry = |path: &str, size: u64| {
        indexed_files += 1;
        indexed_bytes += size;
        progress.maybe_emit(&Progress { phase: Phase::Indexing, files: indexed_files, bytes: indexed_bytes, files_total: 0, bytes_total: 0, current: path.to_owned() });
    };
    let cancel_ref = options.cancel.as_deref();
    let (mut raw_index, mut unhashed, used_lazy) = match source.index_lazy_with_progress_cancel(&mut on_index_entry, cancel_ref)? {
        Some((idx, unhashed)) => (idx, unhashed, true),
        None => (source.index_with_progress_cancel(&mut on_index_entry, cancel_ref)?, HashSet::new(), false),
    };
    if cancel_ref.is_some_and(|c| c.load(Ordering::Acquire)) {
        return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "stopped"));
    }
    filter_index(&mut raw_index, &options.filter, &mut unhashed);
    let mut sender_index = raw_index;
    let (final_indexed_files, final_indexed_bytes) = sender_index.entries.values().filter(|e| e.kind == EntryKind::File).fold((0u64, 0u64), |(f, b), e| (f + 1, b + e.size));
    progress.emit_now(&Progress { phase: Phase::Indexing, files: final_indexed_files, bytes: final_indexed_bytes, files_total: 0, bytes_total: 0, current: String::new() });
    let mut wire = Wire::new(conn);

    // 1. Handshake: receiver sends Hello, sender checks version and tree hash.
    let (tag, payload) = wire.read_frame()?;
    if tag != TAG_HELLO {
        return Err(invalid_data(format!("expected Hello, got tag {tag}")));
    }
    let hello = decode_hello(&payload)?;
    if hello.version != WIRE_VERSION {
        return Err(invalid_data(format!("unsupported wire version {}", hello.version)));
    }

    let effective_keep_newer = options.keep_newer || (hello.flags & FLAG_KEEP_NEWER != 0);
    let scoped = hello.flags & FLAG_SCOPED != 0;
    let effective_delete_extra = hello.flags & FLAG_DELETE_EXTRA != 0 && !scoped;
    let want_sender_summary = hello.flags & FLAG_SENDER_SUMMARY != 0;

    if scoped {
        send_scope(&mut wire, &sender_index)?;
        wire.flush()?;
    } else if unhashed.is_empty() && hello.tree_hash == sender_index.tree_hash() {
        let mut ack = Vec::with_capacity(5);
        ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        // 1 = legacy in_sync (no summary follows);
        // 2 = in_sync + TAG_SENDER_SUMMARY follows (only sent when receiver asked for FLAG_SENDER_SUMMARY).
        ack.push(if want_sender_summary { 2 } else { 1 });
        wire.write_frame(TAG_HELLO_ACK, &ack)?;
        let skipped = sender_index.skipped();
        if want_sender_summary {
            send_sender_summary(&mut wire, &[], &skipped)?;
        }
        wire.flush()?;
        if used_lazy {
            source.save_cache(&sender_index)?;
        }
        progress.emit_now(&Progress { phase: Phase::Planned, files: 0, bytes: 0, files_total: 0, bytes_total: 0, current: String::new() });
        progress.emit_now(&Progress { phase: Phase::Sending, files: 0, bytes: 0, files_total: 0, bytes_total: 0, current: String::new() });
        progress.emit_now(&Progress { phase: Phase::Done, files: 0, bytes: 0, files_total: 0, bytes_total: 0, current: String::new() });
        return Ok(Report { same: sender_index.file_count(), bytes_sent: wire.bytes_sent, bytes_received: wire.bytes_received, skipped, sender_confirmed: true, ..Report::default() });
    } else {
        let mut ack = Vec::with_capacity(5);
        ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        ack.push(0); // in_sync = false
        wire.write_frame(TAG_HELLO_ACK, &ack)?;
        wire.flush()?;
    }

    // 2. Receive receiver's manifest (no chunk lists).
    let mut recv_manifest: BTreeMap<String, ManifestEntry> = BTreeMap::new();
    loop {
        let (tag, payload) = wire.read_frame()?;
        if tag != TAG_MANIFEST {
            return Err(invalid_data(format!("expected Manifest, got tag {tag}")));
        }
        let more = decode_manifest_frame(&payload, &mut recv_manifest)?;
        if recv_manifest.len() > MAX_ITEMS {
            return Err(invalid_data("the receiver's manifest holds too many entries"));
        }
        if !more {
            break;
        }
    }

    // Index receiver files by BLAKE3 (for O(1) CopyFile detection) and by size
    // (so lazy-indexed new files only hash upfront when a CopyFile is possible).
    let mut recv_by_hash: HashMap<[u8; 32], String> = HashMap::new();
    let mut recv_sizes: HashSet<u64> = HashSet::new();
    for entry in recv_manifest.values() {
        if entry.kind == EntryKind::File {
            recv_by_hash.entry(entry.hash).or_insert_with(|| entry.path.clone());
            recv_sizes.insert(entry.size);
        }
    }

    // 3. Classify every sender file and gather chunk requests for differing files.
    let mut same_count = 0u64;
    let mut left_newer_count = 0u64;
    let mut set_meta_files: Vec<String> = Vec::new();
    let mut copy_files: Vec<(String, String)> = Vec::new();
    // `(path, hash_at_end)`
    let mut new_files: Vec<(String, bool)> = Vec::new();
    let mut delta_files: Vec<String> = Vec::new();
    let mut chunk_reqs: Vec<ChunkReq> = Vec::new();
    let mut updated_files: HashSet<String> = HashSet::new();
    let mut aborted_files: HashSet<String> = HashSet::new();
    let mut unread_targets: HashSet<String> = HashSet::new();
    let mut sender_errors: Vec<PathError> = Vec::new();

    let file_paths: Vec<String> = sender_index.entries.values().filter(|e| e.kind == EntryKind::File).map(|e| e.path.clone()).collect();
    for path in file_paths {
        if unhashed.contains(&path) {
            let (entry_size, entry_mtime_ns) = {
                let e = &sender_index.entries[&path];
                (e.size, e.mtime_ns)
            };
            let their_same_path = recv_manifest.get(&path).filter(|t| t.kind == EntryKind::File);
            if let Some(their) = their_same_path
                && effective_keep_newer
                && their.mtime_ns > entry_mtime_ns
            {
                left_newer_count += 1;
                continue;
            }
            let need_prehash = match their_same_path {
                Some(their) => their.size == entry_size || (their.size >= chunk::CHUNK_MIN_SIZE && entry_size >= chunk::CHUNK_MIN_SIZE) || (entry_size > 0 && recv_sizes.contains(&entry_size)),
                None => entry_size > 0 && recv_sizes.contains(&entry_size),
            };
            if !need_prehash {
                updated_files.insert(path.clone());
                new_files.push((path, true));
                continue;
            }
            match source.hash_entry(&path, entry_size) {
                Ok(digest) => {
                    if let Some(e) = sender_index.entries.get_mut(&path) {
                        e.size = digest.size;
                        e.hash = digest.hash;
                        e.chunks = digest.chunks;
                    }
                    unhashed.remove(&path);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    sender_index.entries.remove(&path);
                    unhashed.remove(&path);
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    sender_index.entries.remove(&path);
                    unhashed.remove(&path);
                    sender_index.unread.insert(path.clone(), e.to_string());
                    unread_targets.insert(path);
                    continue;
                }
                Err(_) => {
                    aborted_files.insert(path.clone());
                    sender_errors.push(PathError { path, reason: REASON_CHANGED.into() });
                    continue;
                }
            }
        }

        let entry = &sender_index.entries[&path];
        if let Some(their) = recv_manifest.get(&entry.path)
            && their.kind == EntryKind::File
        {
            if their.hash == entry.hash {
                same_count += 1;
                if their.mode != entry.mode {
                    set_meta_files.push(path);
                }
                continue;
            }
            if effective_keep_newer && their.mtime_ns > entry.mtime_ns {
                left_newer_count += 1;
                continue;
            }
        }
        if let Some(from) = recv_by_hash.get(&entry.hash) {
            updated_files.insert(entry.path.clone());
            copy_files.push((path, from.clone()));
            continue;
        }
        if let Some(their) = recv_manifest.get(&entry.path)
            && their.kind == EntryKind::File
            && their.size >= chunk::CHUNK_MIN_SIZE
            && entry.size >= chunk::CHUNK_MIN_SIZE
            && !entry.chunks.is_empty()
        {
            let probe = if entry.size > their.size {
                match recipe::append_probe_for(entry, their.size, |off, len| source.read(&entry.path, off, len)) {
                    Ok(p) => p,
                    Err(_) => {
                        aborted_files.insert(entry.path.clone());
                        sender_errors.push(PathError { path: entry.path.clone(), reason: REASON_CHANGED.into() });
                        continue;
                    }
                }
            } else {
                None
            };
            chunk_reqs.push(match probe {
                Some(p) => ChunkReq::AppendOrSuper { path: entry.path.clone(), probe: p },
                None => ChunkReq::SuperOrLeaf { path: entry.path.clone() },
            });
            updated_files.insert(entry.path.clone());
            delta_files.push(path);
            continue;
        }
        updated_files.insert(entry.path.clone());
        new_files.push((path, false));
    }

    // 4. Ask receiver for append confirmation / super-chunks / leaf chunks.
    let mut chunk_resps: HashMap<String, ChunkResp> = HashMap::new();
    let mut leaf_by_file: HashMap<String, HashMap<u32, Vec<ChunkRef>>> = HashMap::new();

    if !chunk_reqs.is_empty() {
        send_need_chunks(&mut wire, &chunk_reqs)?;
        wire.flush()?;

        loop {
            let (tag, payload) = wire.read_frame()?;
            if tag != TAG_CHUNK_INFO {
                return Err(invalid_data(format!("expected ChunkInfo, got tag {tag}")));
            }
            let more = decode_chunk_info_frame(&payload, &mut chunk_resps)?;
            if chunk_resps.len() > MAX_ITEMS {
                return Err(invalid_data("too many chunk answers"));
            }
            if !more {
                break;
            }
        }

        // Round 2: for any SuperChunks response that has unmatched super-chunks,
        // request leaf chunks ONLY for the receiver's unmatched super-chunks.
        let mut leaf_reqs: Vec<(String, Vec<u32>)> = Vec::new();
        for path in &delta_files {
            let entry = &sender_index.entries[path];
            if let Some(ChunkResp::SuperChunks(their_supers)) = chunk_resps.get(&entry.path) {
                let our_supers = recipe::super_chunks(&entry.chunks);
                let their_ids: HashSet<(ChunkId, u64)> = their_supers.iter().map(|s| (s.id, s.len)).collect();
                let any_our_unmatched = our_supers.iter().any(|s| !their_ids.contains(&(s.id, s.len)));
                if !any_our_unmatched {
                    continue;
                }
                let our_ids: HashSet<(ChunkId, u64)> = our_supers.iter().map(|s| (s.id, s.len)).collect();
                let missing: Vec<u32> = their_supers.iter().enumerate().filter(|(_, s)| !our_ids.contains(&(s.id, s.len))).map(|(idx, _)| idx as u32).collect();
                if !missing.is_empty() {
                    leaf_reqs.push((entry.path.clone(), missing));
                }
            }
        }

        if !leaf_reqs.is_empty() {
            send_need_leaf_chunks(&mut wire, &leaf_reqs)?;
            wire.flush()?;
            loop {
                let (tag, payload) = wire.read_frame()?;
                if tag != TAG_LEAF_CHUNKS {
                    return Err(invalid_data(format!("expected LeafChunks, got tag {tag}")));
                }
                let more = decode_leaf_chunks_frame(&payload, &mut leaf_by_file)?;
                if leaf_by_file.values().map(HashMap::len).sum::<usize>() > MAX_ITEMS {
                    return Err(invalid_data("too many leaf chunk answers"));
                }
                if !more {
                    break;
                }
            }
        }
    }

    // 5. Pipelined transfer phase: dirs -> symlinks -> meta -> copies -> recipes -> hardlinks -> removes -> done.
    let plan_files = (copy_files.len() + delta_files.len() + new_files.len()) as u64;
    let plan_bytes: u64 = copy_files
        .iter()
        .map(|(p, _)| sender_index.entries[p].size)
        .chain(delta_files.iter().map(|p| sender_index.entries[p].size))
        .chain(new_files.iter().map(|(p, _)| sender_index.entries[p].size))
        .sum();
    progress.emit_now(&Progress { phase: Phase::Planned, files: 0, bytes: 0, files_total: plan_files, bytes_total: plan_bytes, current: String::new() });

    for entry in sender_index.entries.values() {
        if entry.kind == EntryKind::Dir {
            let needs_dir = !matches!(
                recv_manifest.get(&entry.path),
                Some(their) if their.kind == EntryKind::Dir && final_dir_mode(their.mode) == final_dir_mode(entry.mode)
            );
            if needs_dir {
                let mut buf = Vec::with_capacity(entry.path.len() + 16);
                write_str(&mut buf, &entry.path);
                buf.extend_from_slice(&entry.mode.to_le_bytes());
                buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
                wire.write_frame(TAG_MKDIR, &buf)?;
            }
        }
    }

    for entry in sender_index.entries.values() {
        if let EntryKind::Symlink(target) = &entry.kind {
            let needs_sym = !matches!(
                recv_manifest.get(&entry.path),
                Some(their) if their.kind == entry.kind
            );
            if needs_sym {
                let mut buf = Vec::with_capacity(entry.path.len() + target.len() + 16);
                write_str(&mut buf, &entry.path);
                write_str(&mut buf, target);
                buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
                wire.write_frame(TAG_SYMLINK, &buf)?;
            }
        }
    }

    for path in set_meta_files {
        let entry = &sender_index.entries[&path];
        let mut buf = Vec::with_capacity(entry.path.len() + 16);
        write_str(&mut buf, &entry.path);
        buf.extend_from_slice(&entry.mode.to_le_bytes());
        buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
        wire.write_frame(TAG_SET_META, &buf)?;
    }

    let mut sent_files = 0u64;
    let mut sent_bytes = 0u64;

    for (path, from) in copy_files {
        let entry = &sender_index.entries[&path];
        let mut buf = Vec::with_capacity(entry.path.len() + from.len() + 64);
        write_str(&mut buf, &entry.path);
        write_str(&mut buf, &from);
        buf.extend_from_slice(&entry.mode.to_le_bytes());
        buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
        buf.extend_from_slice(&entry.size.to_le_bytes());
        buf.extend_from_slice(&entry.hash);
        wire.write_frame(TAG_COPY_FILE, &buf)?;
        sent_files += 1;
        sent_bytes += entry.size;
        progress.maybe_emit(&Progress { phase: Phase::Sending, files: sent_files, bytes: sent_bytes, files_total: plan_files, bytes_total: plan_bytes, current: path });
    }

    let mut data_bytes_raw = 0u64;
    let mut data_bytes_compressed = 0u64;
    let wants_cache = source.wants_cache();

    for path in delta_files {
        let entry = &sender_index.entries[&path];
        let entry_size = entry.size;
        let their_size = recv_manifest.get(&entry.path).map(|e| e.size).unwrap_or(0);
        let plan = match chunk_resps.get(&entry.path) {
            Some(ChunkResp::AppendConfirmed) => recipe::plan_append(&entry.path, entry.size, their_size),
            Some(ChunkResp::LeafChunks(their_chunks)) => recipe::plan_patch_chunks(&entry.path, &entry.chunks, their_chunks),
            Some(ChunkResp::SuperChunks(their_supers)) => {
                let empty = HashMap::new();
                let leaves = leaf_by_file.get(&entry.path).unwrap_or(&empty);
                recipe::plan_from_super_and_leaf(&entry.path, &entry.chunks, their_supers, leaves)
            }
            None => recipe::plan_new(entry.size),
        };
        let mut in_file = 0u64;
        let mut on_bytes = |n: u64| {
            in_file += n;
            progress.maybe_emit(&Progress {
                phase: Phase::Sending,
                files: sent_files,
                bytes: sent_bytes + in_file.min(entry_size),
                files_total: plan_files,
                bytes_total: plan_bytes,
                current: path.clone(),
            });
        };
        if let Plan::Build { kind, ops } = plan {
            match stream_file_plan(&mut wire, source, entry, kind, &ops, false, false, &mut data_bytes_raw, &mut data_bytes_compressed, &mut on_bytes)? {
                StreamOutcome::Sent { .. } => {}
                StreamOutcome::Changed => {
                    aborted_files.insert(path.clone());
                    sender_errors.push(PathError { path: path.clone(), reason: REASON_CHANGED.into() });
                }
                StreamOutcome::Unread(reason) => {
                    sender_index.entries.remove(&path);
                    sender_index.unread.insert(path.clone(), reason);
                    unread_targets.insert(path.clone());
                }
            }
        }
        sent_files += 1;
        sent_bytes += entry_size;
        progress.maybe_emit(&Progress { phase: Phase::Sending, files: sent_files, bytes: sent_bytes, files_total: plan_files, bytes_total: plan_bytes, current: path });
    }

    for (path, hash_at_end) in new_files {
        let entry_size = sender_index.entries[&path].size;
        let Plan::Build { kind, ops } = recipe::plan_new(entry_size) else {
            unreachable!();
        };
        let mut in_file = 0u64;
        let mut on_bytes = |n: u64| {
            in_file += n;
            progress.maybe_emit(&Progress {
                phase: Phase::Sending,
                files: sent_files,
                bytes: sent_bytes + in_file.min(entry_size),
                files_total: plan_files,
                bytes_total: plan_bytes,
                current: path.clone(),
            });
        };
        let compute_chunks = hash_at_end && wants_cache && entry_size >= chunk::CHUNK_MIN_SIZE;
        let outcome = stream_file_plan(&mut wire, source, &sender_index.entries[&path], kind, &ops, hash_at_end, compute_chunks, &mut data_bytes_raw, &mut data_bytes_compressed, &mut on_bytes)?;
        match outcome {
            StreamOutcome::Sent { hash, chunks } => {
                if hash_at_end {
                    if let Some(e) = sender_index.entries.get_mut(&path) {
                        e.hash = hash;
                        e.chunks = chunks;
                    }
                    unhashed.remove(&path);
                }
            }
            StreamOutcome::Changed => {
                aborted_files.insert(path.clone());
                sender_errors.push(PathError { path: path.clone(), reason: REASON_CHANGED.into() });
            }
            StreamOutcome::Unread(reason) => {
                sender_index.entries.remove(&path);
                unhashed.remove(&path);
                sender_index.unread.insert(path.clone(), reason);
                unread_targets.insert(path.clone());
            }
        }
        sent_files += 1;
        sent_bytes += entry_size;
        progress.maybe_emit(&Progress { phase: Phase::Sending, files: sent_files, bytes: sent_bytes, files_total: plan_files, bytes_total: plan_bytes, current: path });
    }

    progress.emit_now(&Progress { phase: Phase::Sending, files: sent_files, bytes: sent_bytes, files_total: plan_files, bytes_total: plan_bytes, current: String::new() });

    // Hard links are sent once all target files are in place. If the target
    // was replaced in this pass (or isn't linked to it on the receiver yet),
    // make the link (again).
    let mut unread_hardlinks: Vec<(String, String)> = Vec::new();
    for entry in sender_index.entries.values() {
        if let EntryKind::HardLink(target) = &entry.kind {
            if unread_targets.contains(target) {
                if let Some(reason) = sender_index.unread.get(target) {
                    unread_hardlinks.push((entry.path.clone(), reason.clone()));
                }
                continue;
            }
            if aborted_files.contains(target) {
                sender_errors.push(PathError { path: entry.path.clone(), reason: REASON_CHANGED.into() });
                continue;
            }
            let target_replaced = updated_files.contains(target);
            let already_linked = !target_replaced
                && matches!(
                    recv_manifest.get(&entry.path),
                    Some(their) if their.kind == entry.kind
                );
            if !already_linked {
                let mut buf = Vec::with_capacity(entry.path.len() + target.len() + 8);
                write_str(&mut buf, &entry.path);
                write_str(&mut buf, target);
                wire.write_frame(TAG_HARDLINK, &buf)?;
            }
        }
    }

    for (p, reason) in unread_hardlinks {
        sender_index.entries.remove(&p);
        sender_index.unread.insert(p, reason);
    }

    if effective_delete_extra {
        // Remove extra paths in reverse order so children are removed before parents.
        // (Nor what the sender couldn't read or didn't enter, nor anything under it:
        // what it holds isn't known. A root-level unread marker "." or "" covers
        // the entire tree so an incomplete root listing never deletes peer files.)
        let root_unread = sender_index.unread.contains_key(".") || sender_index.unread.contains_key("");
        let unknown = |p: &str| root_unread || sender_index.unread.keys().chain(sender_index.mounts.iter()).any(|u| p == u || p.strip_prefix(u.as_str()).is_some_and(|rest| rest.starts_with('/')));
        let extras: Vec<&String> = recv_manifest.keys().rev().filter(|p| !sender_index.entries.contains_key(*p) && !unknown(p)).collect();
        for path in extras {
            let mut buf = Vec::with_capacity(path.len() + 4);
            write_str(&mut buf, path);
            wire.write_frame(TAG_REMOVE, &buf)?;
        }
    }

    let skipped = sender_index.skipped();
    if want_sender_summary {
        send_sender_summary(&mut wire, &sender_errors, &skipped)?;
    }

    let mut done_buf = Vec::with_capacity(32);
    done_buf.extend_from_slice(&same_count.to_le_bytes());
    done_buf.extend_from_slice(&left_newer_count.to_le_bytes());
    done_buf.extend_from_slice(&data_bytes_raw.to_le_bytes());
    done_buf.extend_from_slice(&data_bytes_compressed.to_le_bytes());
    wire.write_frame(TAG_DONE, &done_buf)?;
    wire.flush()?;

    // 6. Read final report from receiver (may span multiple frames).
    let mut report = read_report(&mut wire)?;
    if used_lazy {
        // Persist only entries whose hashes were actually computed (never
        // aborted or unhashed keep_newer entries).
        for p in unhashed.iter().chain(aborted_files.iter()) {
            sender_index.entries.remove(p);
        }
        source.save_cache(&sender_index)?;
    }
    report.errors.extend(sender_errors);
    report.skipped = skipped;
    report.sender_confirmed = true;
    report.bytes_sent = wire.bytes_sent;
    report.bytes_received = wire.bytes_received;
    progress.emit_now(&Progress { phase: Phase::Done, files: plan_files, bytes: plan_bytes, files_total: plan_files, bytes_total: plan_bytes, current: String::new() });
    Ok(report)
}

/// Receiver entry point: accept updates from the peer on `conn` into `root`.
///
/// Nothing is ever written outside `root`: paths that are absolute, contain
/// `..`, or would traverse a symlink are refused and recorded in
/// [`Report::errors`].
pub fn receive<S: Read + Write>(conn: S, root: &Path, options: Options) -> io::Result<Report> {
    receive_inner(conn, root, options, |p| fs::File::open(p), CHECKPOINT_FILES)
}

fn receive_inner<S: Read + Write, R: Read>(conn: S, root: &Path, options: Options, open: impl FnMut(&Path) -> io::Result<R>, checkpoint_files: u64) -> io::Result<Report> {
    // (Scoped, nothing is taken away: what else the root holds isn't the
    // session's.)
    let options = Options { delete_extra: options.delete_extra && !options.scoped, ..options };
    fs::create_dir_all(root)?;
    let prev = options.cache_path.as_deref().and_then(|p| Index::load(p).ok());
    let mut wire = Wire::new(conn);
    let mut flags = 0u8;
    if options.delete_extra {
        flags |= FLAG_DELETE_EXTRA;
    }
    if options.keep_newer {
        flags |= FLAG_KEEP_NEWER;
    }
    if options.request_sender_summary {
        flags |= FLAG_SENDER_SUMMARY;
    }
    let recv_index = if options.scoped {
        // Scoped: the hello says so, the sender names its paths, and only
        // those are looked at here.
        let mut hello = Vec::with_capacity(45);
        hello.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        hello.push(flags | FLAG_SCOPED);
        hello.extend_from_slice(&[0u8; 32]);
        hello.extend_from_slice(&0u64.to_le_bytes());
        wire.write_frame(TAG_HELLO, &hello)?;
        wire.flush()?;
        let mut paths = Vec::new();
        loop {
            let (tag, payload) = wire.read_frame()?;
            if tag != TAG_SCOPE {
                return Err(invalid_data(format!("expected Scope, got tag {tag}")));
            }
            let more = decode_scope_frame(&payload, &mut paths)?;
            if paths.len() > MAX_ITEMS {
                return Err(invalid_data("the sender names too many paths"));
            }
            if !more {
                break;
            }
        }
        Index::scan_paths_with(root, &paths, prev.as_ref(), open)?
    } else {
        Index::scan_with(root, &options.filter, prev.as_ref(), open)?
    };
    if options.scoped {
        return receive_after_hello(wire, root, options, recv_index, checkpoint_files);
    }
    let recv_tree_hash = recv_index.tree_hash();

    // 1. Handshake: send Hello, read HelloAck.
    let mut hello = Vec::with_capacity(45);
    hello.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    hello.push(flags);
    hello.extend_from_slice(&recv_tree_hash);
    hello.extend_from_slice(&recv_index.file_count().to_le_bytes());
    wire.write_frame(TAG_HELLO, &hello)?;
    wire.flush()?;

    let (tag, payload) = wire.read_frame()?;
    if tag != TAG_HELLO_ACK {
        return Err(invalid_data(format!("expected HelloAck, got tag {tag}")));
    }
    if payload.len() != 5 {
        return Err(invalid_data("malformed HelloAck"));
    }
    let peer_version = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    if peer_version != WIRE_VERSION {
        return Err(invalid_data(format!("unsupported wire version {peer_version}")));
    }
    let in_sync = payload[4];
    if in_sync > 2 || (in_sync == 2 && !options.request_sender_summary) {
        return Err(invalid_data("invalid in_sync flag in HelloAck"));
    }
    if in_sync >= 1 {
        let mut report = Report { same: recv_index.file_count(), ..Report::default() };
        if in_sync == 2 {
            // Capable sender explicitly advertised TAG_SENDER_SUMMARY in HelloAck:
            // require the complete summary frame (mid-frame EOF is a hard error).
            let (tag, summary_payload) = wire.read_frame()?;
            if tag != TAG_SENDER_SUMMARY {
                return Err(invalid_data(format!("expected SenderSummary, got tag {tag}")));
            }
            let (sender_errors, skipped) = read_sender_summary_frames(&mut wire, &summary_payload)?;
            report.errors.extend(sender_errors);
            report.skipped = skipped;
            report.sender_confirmed = true;
        }
        // in_sync == 1: legacy sender that never sends TAG_SENDER_SUMMARY.
        // Do NOT read ahead (would block on a kept-open duplex stream); leave
        // `sender_confirmed = false` so destructive callers know not to delete.
        if let Some(cache) = &options.cache_path {
            recv_index.save(cache)?;
        }
        report.bytes_sent = wire.bytes_sent;
        report.bytes_received = wire.bytes_received;
        return Ok(report);
    }

    receive_after_hello(wire, root, options, recv_index, checkpoint_files)
}

/// The receiver's session once the hellos are done: its manifest, then
/// what the sender asks and sends, until it is done.
fn receive_after_hello<S: Read + Write>(mut wire: Wire<S>, root: &Path, options: Options, mut recv_index: Index, checkpoint_files: u64) -> io::Result<Report> {
    // 2. Send manifest (no chunk lists).
    send_manifest(&mut wire, &recv_index)?;
    wire.flush()?;

    // 3. Process negotiation and transfer frames until TAG_DONE.
    let mut report = Report::default();
    let mut sender_errors_from_wire: Vec<PathError> = Vec::new();
    let mut summary_received = false;
    let mut super_cache: HashMap<String, Vec<SuperChunk>> = HashMap::new();
    let mut active_file: Option<ActiveFile> = None;
    let mut copy_buf = vec![0u8; 64 << 10];
    // Folders created or temporarily made writable: `(final_mode, Option<mtime_ns>)`,
    // finalized deepest-first at the end of the session.
    let mut pending_dirs: BTreeMap<String, (u32, Option<i64>)> = BTreeMap::new();
    let mut last_checkpoint = Instant::now();
    let mut files_since_checkpoint = 0u64;
    let mut bytes_since_checkpoint = 0u64;
    let mut index_dirty = false;
    // (How long the last save took: the next is that many times later.)
    let mut save_took = Duration::ZERO;

    let mut maybe_checkpoint = |recv_index: &Index, dirty: &mut bool, files: &mut u64, bytes: &mut u64, last: &mut Instant| {
        if !*dirty {
            return;
        }
        let Some(cache) = &options.cache_path else {
            return;
        };
        let due = CHECKPOINT_INTERVAL.max(save_took * CHECKPOINT_SAVES_APART);
        if *files >= checkpoint_files || last.elapsed() >= due {
            let began = Instant::now();
            if recv_index.save(cache).is_ok() {
                save_took = began.elapsed();
                *dirty = false;
                *files = 0;
                *bytes = 0;
                *last = Instant::now();
            }
        }
    };

    loop {
        let (tag, payload) = match wire.read_frame() {
            Ok(frame) => frame,
            Err(e) => {
                if let Some(af) = active_file.take() {
                    abort_file(af);
                }
                if index_dirty && let Some(cache) = &options.cache_path {
                    let _ = recv_index.save(cache);
                }
                return Err(e);
            }
        };
        if summary_received && tag != TAG_DONE {
            return Err(invalid_data(format!("unexpected frame tag {tag} after SenderSummary")));
        }
        match tag {
            TAG_NEED_CHUNKS => {
                if active_file.is_some() {
                    return Err(invalid_data("NeedChunks while file transfer active"));
                }
                let mut reqs = Vec::new();
                let mut more = decode_need_chunks_frame(&payload, &mut reqs)?;
                while more {
                    if reqs.len() > MAX_ITEMS {
                        return Err(invalid_data("too many chunk requests"));
                    }
                    let (t, p) = wire.read_frame()?;
                    if t != TAG_NEED_CHUNKS {
                        return Err(invalid_data("expected continuation NeedChunks"));
                    }
                    more = decode_need_chunks_frame(&p, &mut reqs)?;
                }
                send_chunk_info(&mut wire, &recv_index, &reqs, &mut super_cache)?;
                wire.flush()?;
            }
            TAG_NEED_LEAF_CHUNKS => {
                if active_file.is_some() {
                    return Err(invalid_data("NeedLeafChunks while file transfer active"));
                }
                let mut reqs = Vec::new();
                let mut more = decode_need_leaf_chunks_frame(&payload, &mut reqs)?;
                while more {
                    if reqs.len() > MAX_ITEMS {
                        return Err(invalid_data("too many leaf chunk requests"));
                    }
                    let (t, p) = wire.read_frame()?;
                    if t != TAG_NEED_LEAF_CHUNKS {
                        return Err(invalid_data("expected continuation NeedLeafChunks"));
                    }
                    more = decode_need_leaf_chunks_frame(&p, &mut reqs)?;
                }
                send_leaf_chunks(&mut wire, &recv_index, &super_cache, &reqs)?;
                wire.flush()?;
            }
            TAG_MKDIR => {
                if active_file.is_some() {
                    return Err(invalid_data("Mkdir while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                let mode = cur.read_u32()?;
                let mtime_ns = cur.read_i64()?;
                cur.expect_eof()?;
                match apply_mkdir(root, &path, mode, mtime_ns, &options.protect, &mut pending_dirs, &mut recv_index) {
                    Ok(()) => index_dirty = true,
                    Err(reason) => report.errors.push(PathError { path, reason }),
                }
            }
            TAG_SYMLINK => {
                if active_file.is_some() {
                    return Err(invalid_data("Symlink while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                let target = cur.read_str(MAX_PATH_LEN)?;
                let mtime_ns = cur.read_i64()?;
                cur.expect_eof()?;
                match apply_symlink(root, &path, &target, mtime_ns, &options, &mut pending_dirs, &mut recv_index) {
                    Ok(()) => index_dirty = true,
                    Err(reason) => report.errors.push(PathError { path, reason }),
                }
            }
            TAG_HARDLINK => {
                if active_file.is_some() {
                    return Err(invalid_data("Hardlink while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                let target = cur.read_str(MAX_PATH_LEN)?;
                cur.expect_eof()?;
                match apply_hardlink(root, &path, &target, &options, &mut pending_dirs, &mut recv_index) {
                    Ok(()) => index_dirty = true,
                    Err(reason) => report.errors.push(PathError { path, reason }),
                }
            }
            TAG_SET_META => {
                if active_file.is_some() {
                    return Err(invalid_data("SetMeta while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                let mode = cur.read_u32()?;
                let mtime_ns = cur.read_i64()?;
                cur.expect_eof()?;
                match apply_set_meta(root, &path, mode, mtime_ns, &options.protect, &mut recv_index) {
                    Ok(()) => index_dirty = true,
                    Err(reason) => report.errors.push(PathError { path, reason }),
                }
            }
            TAG_COPY_FILE => {
                if active_file.is_some() {
                    return Err(invalid_data("CopyFile while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                let from = cur.read_str(MAX_PATH_LEN)?;
                let mode = cur.read_u32()?;
                let mtime_ns = cur.read_i64()?;
                let size = cur.read_u64()?;
                let hash = cur.read_array::<32>()?;
                cur.expect_eof()?;
                match apply_copy_file(root, &path, &from, mode, mtime_ns, size, hash, &options, &mut pending_dirs, &mut recv_index, &mut copy_buf) {
                    Ok(CopyOutcome::Copied) => {
                        report.copied += 1;
                        index_dirty = true;
                        files_since_checkpoint += 1;
                        bytes_since_checkpoint += size;
                        maybe_checkpoint(&recv_index, &mut index_dirty, &mut files_since_checkpoint, &mut bytes_since_checkpoint, &mut last_checkpoint);
                    }
                    Ok(CopyOutcome::LeftNewer) => report.left_newer += 1,
                    Err(reason) => report.errors.push(PathError { path, reason }),
                }
            }
            TAG_FILE_START => {
                if active_file.is_some() {
                    return Err(invalid_data("nested FileStart"));
                }
                let start = decode_file_start(&payload)?;
                active_file = Some(begin_file(root, start, &options, &mut pending_dirs, &recv_index, &mut report));
            }
            TAG_FILE_PIECE => {
                let Some(af) = active_file.as_mut() else {
                    return Err(invalid_data("FilePiece without FileStart"));
                };
                let piece = decode_file_piece(&payload, &af.basis_paths)?;
                apply_active_piece(root, af, &piece, &mut copy_buf);
            }
            TAG_FILE_END => {
                let Some(mut af) = active_file.take() else {
                    return Err(invalid_data("FileEnd without FileStart"));
                };
                if af.expected_hash.is_some() {
                    if !payload.is_empty() {
                        return Err(invalid_data("trailing bytes in FileEnd"));
                    }
                } else {
                    if payload.len() != 32 {
                        return Err(invalid_data("expected 32-byte hash in FileEnd"));
                    }
                    let hash: [u8; 32] = payload.try_into().unwrap();
                    af.expected_hash = Some(hash);
                }
                let expected_size = af.expected_size;
                let before_done = report.new + report.patched + report.appended;
                finish_file(root, af, &options.protect, &mut recv_index, &mut report);
                if report.new + report.patched + report.appended > before_done {
                    index_dirty = true;
                    files_since_checkpoint += 1;
                    bytes_since_checkpoint += expected_size;
                    maybe_checkpoint(&recv_index, &mut index_dirty, &mut files_since_checkpoint, &mut bytes_since_checkpoint, &mut last_checkpoint);
                }
            }
            TAG_FILE_ABORT => {
                if !payload.is_empty() {
                    return Err(invalid_data("trailing bytes in FileAbort"));
                }
                let Some(af) = active_file.take() else {
                    return Err(invalid_data("FileAbort without FileStart"));
                };
                abort_file(af);
            }
            TAG_REMOVE => {
                if active_file.is_some() {
                    return Err(invalid_data("Remove while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                cur.expect_eof()?;
                if is_protected(&options.protect, &path) || contains_protected(&options.protect, &path) {
                    report.errors.push(PathError { path, reason: REASON_PROTECTED.into() });
                } else if !options.delete_extra {
                    report.errors.push(PathError { path, reason: "remove refused: delete_extra is false".into() });
                } else {
                    match apply_remove(root, &path, &options.protect, &mut pending_dirs, &mut recv_index) {
                        Ok(true) => {
                            report.removed += 1;
                            index_dirty = true;
                        }
                        Ok(false) => {}
                        Err(reason) => report.errors.push(PathError { path, reason }),
                    }
                }
            }
            TAG_SENDER_SUMMARY => {
                if !options.request_sender_summary {
                    return Err(invalid_data("unexpected SenderSummary"));
                }
                if active_file.is_some() {
                    return Err(invalid_data("SenderSummary while file transfer active"));
                }
                if summary_received {
                    return Err(invalid_data("duplicate SenderSummary"));
                }
                let (sender_errors, skipped) = read_sender_summary_frames(&mut wire, &payload)?;
                sender_errors_from_wire.extend(sender_errors);
                report.skipped = skipped;
                report.sender_confirmed = true;
                summary_received = true;
            }
            TAG_DONE => {
                if active_file.is_some() {
                    return Err(invalid_data("Done while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                report.same = cur.read_u64()?;
                let sender_left_newer = cur.read_u64()?;
                report.left_newer += sender_left_newer;
                report.data_bytes_raw = cur.read_u64()?;
                report.data_bytes_compressed = cur.read_u64()?;
                cur.expect_eof()?;
                break;
            }
            other => {
                return Err(invalid_data(format!("unknown frame tag {other}")));
            }
        }
    }

    finalize_dirs(root, pending_dirs, &mut recv_index, &mut report);

    if let Some(cache) = &options.cache_path {
        recv_index.save(cache)?;
    }

    send_report(&mut wire, &report)?;
    wire.flush()?;

    // Attach sender-side errors only after sending the receiver's own report
    // back so the sender does not see its own errors duplicated.
    report.errors.extend(sender_errors_from_wire);
    report.bytes_sent = wire.bytes_sent;
    report.bytes_received = wire.bytes_received;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Active file state machine on the receiver
// ---------------------------------------------------------------------------

struct FileStartMsg {
    path: String,
    kind: RecipeKind,
    size: u64,
    mode: u32,
    mtime_ns: i64,
    /// `Some(hash)` when known up front, or `None` when the sender streams a
    /// new file once and delivers its 32-byte BLAKE3 in `TAG_FILE_END`.
    hash: Option<[u8; 32]>,
    basis_paths: Vec<String>,
    /// An append's: the size the receiver's file has, and the sender's
    /// chunks from the start of that file's last one to the end.
    tail: Option<(u64, Vec<ChunkRef>)>,
}

/// A piece decoded from a `TAG_FILE_PIECE` frame.
enum WirePiece {
    Copy { from: String, offset: u64, len: u64 },
    Data { raw_len: u32, codec: DataCodec, bytes: Vec<u8> },
    Zero { len: u64 },
}

/// Writes a brand-new file sparsely: any whole aligned 4 KiB block of zeros
/// is skipped by advancing `logical_pos` without writing, and `finish` sets
/// the final length so trailing holes stay unallocated.
struct SparseWriter {
    file: fs::File,
    logical_pos: u64,
    disk_pos: u64,
}

impl SparseWriter {
    fn new(file: fs::File) -> Self {
        Self { file, logical_pos: 0, disk_pos: 0 }
    }

    fn write_raw(&mut self, slice: &[u8]) -> io::Result<()> {
        if slice.is_empty() {
            return Ok(());
        }
        if self.disk_pos != self.logical_pos {
            self.file.seek(SeekFrom::Start(self.logical_pos))?;
            self.disk_pos = self.logical_pos;
        }
        self.file.write_all(slice)?;
        self.logical_pos += slice.len() as u64;
        self.disk_pos += slice.len() as u64;
        Ok(())
    }

    fn write_data(&mut self, mut data: &[u8]) -> io::Result<()> {
        let rem = (self.logical_pos % SPARSE_BLOCK as u64) as usize;
        if rem != 0 {
            let head = (SPARSE_BLOCK - rem).min(data.len());
            self.write_raw(&data[..head])?;
            data = &data[head..];
        }
        let mut pending_start = 0usize;
        let mut offset = 0usize;
        while offset + SPARSE_BLOCK <= data.len() {
            let block = &data[offset..offset + SPARSE_BLOCK];
            if is_all_zero(block) {
                if pending_start < offset {
                    self.write_raw(&data[pending_start..offset])?;
                }
                self.logical_pos += SPARSE_BLOCK as u64;
                offset += SPARSE_BLOCK;
                pending_start = offset;
            } else {
                offset += SPARSE_BLOCK;
            }
        }
        if pending_start < data.len() {
            self.write_raw(&data[pending_start..])?;
        }
        Ok(())
    }

    fn write_zeros(&mut self, mut len: u64) -> io::Result<()> {
        let rem = self.logical_pos % SPARSE_BLOCK as u64;
        if rem != 0 {
            let head = (SPARSE_BLOCK as u64 - rem).min(len) as usize;
            self.write_raw(&ZEROS[..head])?;
            len -= head as u64;
        }
        let whole_blocks = len / SPARSE_BLOCK as u64;
        if whole_blocks > 0 {
            let skip = whole_blocks * SPARSE_BLOCK as u64;
            self.logical_pos += skip;
            len -= skip;
        }
        if len > 0 {
            self.write_raw(&ZEROS[..len as usize])?;
        }
        Ok(())
    }

    fn finish(self) -> io::Result<fs::File> {
        if self.disk_pos != self.logical_pos {
            self.file.set_len(self.logical_pos)?;
        }
        Ok(self.file)
    }
}

fn is_all_zero(block: &[u8]) -> bool {
    let (chunks, rem) = block.as_chunks::<16>();
    chunks.iter().all(|c| *c == [0u8; 16]) && rem.iter().all(|&b| b == 0)
}

fn feed_zeros(digest: &mut StreamDigest, len: u64) {
    digest.update_zeros(len);
}

enum ActiveState {
    /// An append written where the file is (`FileStartMsg::tail`): after
    /// its old end, the old file's chunks but its last kept, the tail
    /// chunked from that last one's start (read back: 64 KiB at most) and
    /// checked against the sender's. Never the whole file copied and read
    /// again (a 200 MB transcript and a turn added to it were 478 ms of
    /// it, and 400 MB of disk, on each sync). Gone wrong, it is cut back
    /// to its old end; cut short (the session ended), it is still a prefix
    /// of the sender's, and the next sync appends the rest.
    Appending {
        file: fs::File,
        base: u64,
        kept: Vec<ChunkRef>,
        expected: Vec<ChunkRef>,
        digest: Box<StreamDigest>,
        written: u64,
        /// The base's copy, which comes first, is what is there already.
        based: bool,
    },
    Writing {
        part_path: PathBuf,
        writer: SparseWriter,
        digest: Box<StreamDigest>,
        written: u64,
    },
    /// Path was refused (or skipped because `keep_newer` protected a newer file);
    /// remaining pieces are drained cleanly until `FileEnd`.
    Skipped {
        already_recorded: bool,
        error: Option<String>,
    },
}

struct ActiveFile {
    /// Whether a folder where the file goes may be taken away for it
    /// (`Options::delete_extra`).
    replace_dirs: bool,
    path: String,
    kind: RecipeKind,
    expected_size: u64,
    mode: u32,
    mtime_ns: i64,
    expected_hash: Option<[u8; 32]>,
    basis_paths: Vec<String>,
    state: ActiveState,
}

fn begin_file(root: &Path, start: FileStartMsg, options: &Options, pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>, recv_index: &Index, report: &mut Report) -> ActiveFile {
    let make_skipped = |err: Option<String>, already_recorded: bool| ActiveFile {
        replace_dirs: options.delete_extra,
        path: start.path.clone(),
        kind: start.kind,
        expected_size: start.size,
        mode: start.mode,
        mtime_ns: start.mtime_ns,
        expected_hash: start.hash,
        basis_paths: start.basis_paths.clone(),
        state: ActiveState::Skipped { already_recorded, error: err },
    };

    if let Err(e) = validate_rel_path(&start.path) {
        report.errors.push(PathError { path: start.path.clone(), reason: format!("refused path: {e}") });
        return make_skipped(None, true);
    }
    if is_protected(&options.protect, &start.path) {
        report.errors.push(PathError { path: start.path.clone(), reason: REASON_PROTECTED.into() });
        return make_skipped(None, true);
    }
    for bp in &start.basis_paths {
        if let Err(e) = validate_rel_path(bp) {
            report.errors.push(PathError { path: start.path.clone(), reason: format!("refused basis path '{bp}': {e}") });
            return make_skipped(None, true);
        }
    }
    if let Err(reason) = ensure_safe_parent_dirs(root, &start.path, &options.protect, pending_dirs) {
        report.errors.push(PathError { path: start.path.clone(), reason });
        return make_skipped(None, true);
    }

    // Defense-in-depth: receiver enforces keep_newer locally as well.
    if options.keep_newer
        && let Some(existing) = recv_index.entries.get(&start.path)
        && existing.kind == EntryKind::File
        && existing.mtime_ns > start.mtime_ns
    {
        report.left_newer += 1;
        return make_skipped(None, true);
    }

    if fs::symlink_metadata(root.join(&start.path)).is_ok_and(|m| m.is_dir()) {
        if contains_protected(&options.protect, &start.path) {
            report.errors.push(PathError { path: start.path.clone(), reason: REASON_PROTECTED.into() });
            return make_skipped(None, true);
        }
        // (A folder where the file goes is taken away only by a mirror: by
        // `delete_extra`, which the receiver chose.)
        if !options.delete_extra {
            report.errors.push(PathError { path: start.path.clone(), reason: "a folder is there: left as it is (a mirror, delete_extra, replaces it)".into() });
            return make_skipped(None, true);
        }
    }

    if let Some(appending) = start.tail.as_ref().and_then(|(base, expected)| append_in_place(root, &start.path, *base, expected, recv_index)) {
        return ActiveFile {
            replace_dirs: options.delete_extra,
            path: start.path,
            kind: start.kind,
            expected_size: start.size,
            mode: start.mode,
            mtime_ns: start.mtime_ns,
            expected_hash: start.hash,
            basis_paths: start.basis_paths,
            state: appending,
        };
    }

    let part_path = part_path_for(root, &start.path);
    let digest = if start.size >= chunk::CHUNK_MIN_SIZE { StreamDigest::new() } else { StreamDigest::new_hash_only() };
    match create_part(&part_path) {
        Ok(file) => ActiveFile {
            replace_dirs: options.delete_extra,
            path: start.path,
            kind: start.kind,
            expected_size: start.size,
            mode: start.mode,
            mtime_ns: start.mtime_ns,
            expected_hash: start.hash,
            basis_paths: start.basis_paths,
            state: ActiveState::Writing { part_path, writer: SparseWriter::new(file), digest: Box::new(digest), written: 0 },
        },
        Err(e) => {
            report.errors.push(PathError { path: start.path.clone(), reason: format!("create temp file failed: {e}") });
            make_skipped(None, true)
        }
    }
}

/// An append to be written in place, where the receiver's file is still
/// what its index says (`base` long, unchanged since it was read) and
/// isn't a link (neither symlink nor hard-linked to another name): opened at
/// its end, its last chunk read back into the digest of the tail.
fn append_in_place(root: &Path, path: &str, base: u64, expected: &[ChunkRef], recv_index: &Index) -> Option<ActiveState> {
    let entry = recv_index.entries.get(path).filter(|e| e.kind == EntryKind::File && e.size == base && !e.chunks.is_empty())?;
    let (last, kept) = entry.chunks.split_last()?;
    let last_start: u64 = kept.iter().map(|c| c.len as u64).sum();
    if last_start + last.len as u64 != base {
        return None;
    }
    let dst = root.join(path);
    let meta = fs::symlink_metadata(&dst).ok().filter(|m| m.file_type().is_file())?;
    // Never append into a file with more than one link: it would change its twin.
    if meta.nlink() > 1 {
        return None;
    }
    let mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
    if meta.len() != base || meta.ino() != entry.ino || mtime_ns != entry.mtime_ns {
        return None;
    }
    let mut file = fs::OpenOptions::new().read(true).write(true).open(&dst).ok()?;
    let mut back = vec![0u8; last.len as usize];
    file.seek(SeekFrom::Start(last_start)).ok()?;
    file.read_exact(&mut back).ok()?;
    if chunk::chunk_id(&back) != last.id {
        return None;
    }
    let mut digest = Box::new(StreamDigest::new());
    digest.update(&back);
    file.seek(SeekFrom::Start(base)).ok()?;
    Some(ActiveState::Appending { file, base, kept: kept.to_vec(), expected: expected.to_vec(), digest, written: base, based: false })
}

fn apply_active_piece(root: &Path, af: &mut ActiveFile, piece: &WirePiece, copy_buf: &mut [u8]) {
    if let ActiveState::Appending { file, base, digest, written, based, .. } = &mut af.state {
        let res: Result<(), String> = (|| {
            match piece {
                // (What the file has already: the one copy an append starts with.)
                WirePiece::Copy { from, offset: 0, len } if !*based && *from == af.path && *len == *base => *based = true,
                WirePiece::Copy { .. } => return Err("an append copied what it isn't".into()),
                WirePiece::Data { raw_len, codec, bytes } => {
                    let raw = recipe::decode_data_piece(*raw_len, codec, bytes, |from, off, len| {
                        let mut f = open_no_symlink_components(root, from)?;
                        f.seek(SeekFrom::Start(off))?;
                        let mut buf = vec![0u8; len];
                        f.read_exact(&mut buf)?;
                        Ok(buf)
                    })
                    .map_err(|e| format!("decode piece: {e}"))?;
                    *written = written.checked_add(raw.len() as u64).ok_or_else(|| "file size overflow".to_string())?;
                    if *written > af.expected_size || !*based {
                        return Err("the append doesn't fit what was said of it".into());
                    }
                    file.write_all(&raw).map_err(|e| format!("append: {e}"))?;
                    digest.update(&raw);
                }
                WirePiece::Zero { len } => {
                    *written = written.checked_add(*len).ok_or_else(|| "file size overflow".to_string())?;
                    if *written > af.expected_size || !*based {
                        return Err("the append doesn't fit what was said of it".into());
                    }
                    let mut rem = *len;
                    while rem > 0 {
                        let take = (rem as usize).min(ZEROS.len());
                        file.write_all(&ZEROS[..take]).map_err(|e| format!("append: {e}"))?;
                        digest.update(&ZEROS[..take]);
                        rem -= take as u64;
                    }
                }
            }
            Ok(())
        })();
        if let Err(err) = res {
            let _ = file.set_len(*base);
            af.state = ActiveState::Skipped { already_recorded: false, error: Some(err) };
        }
        return;
    }
    let ActiveState::Writing { part_path, writer, digest, written } = &mut af.state else {
        return;
    };

    let res: Result<(), String> = (|| {
        match piece {
            WirePiece::Copy { from, offset, len } => {
                let mut f = open_no_symlink_components(root, from).map_err(|e| format!("open basis '{from}': {e}"))?;
                f.seek(SeekFrom::Start(*offset)).map_err(|e| format!("seek basis '{from}': {e}"))?;
                let mut rem = *len;
                while rem > 0 {
                    let take = (rem as usize).min(copy_buf.len());
                    f.read_exact(&mut copy_buf[..take]).map_err(|e| format!("read basis '{from}': {e}"))?;
                    writer.write_data(&copy_buf[..take]).map_err(|e| format!("write temp: {e}"))?;
                    digest.update(&copy_buf[..take]);
                    *written = written.checked_add(take as u64).ok_or_else(|| "file size overflow".to_string())?;
                    if *written > af.expected_size {
                        return Err("recipe exceeded expected file size".into());
                    }
                    rem -= take as u64;
                }
            }
            WirePiece::Data { raw_len, codec, bytes } => {
                let raw = recipe::decode_data_piece(*raw_len, codec, bytes, |from, off, len| {
                    let mut f = open_no_symlink_components(root, from)?;
                    f.seek(SeekFrom::Start(off))?;
                    let mut buf = vec![0u8; len];
                    f.read_exact(&mut buf)?;
                    Ok(buf)
                })
                .map_err(|e| format!("decode piece: {e}"))?;
                writer.write_data(&raw).map_err(|e| format!("write temp: {e}"))?;
                digest.update(&raw);
                *written = written.checked_add(raw.len() as u64).ok_or_else(|| "file size overflow".to_string())?;
                if *written > af.expected_size {
                    return Err("recipe exceeded expected file size".into());
                }
            }
            WirePiece::Zero { len } => {
                *written = written.checked_add(*len).ok_or_else(|| "file size overflow".to_string())?;
                if *written > af.expected_size {
                    return Err("recipe exceeded expected file size".into());
                }
                writer.write_zeros(*len).map_err(|e| format!("write sparse zeros: {e}"))?;
                feed_zeros(digest, *len);
            }
        }
        Ok(())
    })();

    if let Err(err) = res {
        let _ = fs::remove_file(&*part_path);
        af.state = ActiveState::Skipped { already_recorded: false, error: Some(err) };
    }
}

fn abort_file(af: ActiveFile) {
    match af.state {
        ActiveState::Appending { file, base, .. } => {
            let _ = file.set_len(base);
        }
        ActiveState::Writing { part_path, writer, .. } => {
            drop(writer);
            let _ = fs::remove_file(&part_path);
        }
        ActiveState::Skipped { .. } => {}
    }
}

fn finish_file(root: &Path, af: ActiveFile, protect: &[String], recv_index: &mut Index, report: &mut Report) {
    match af.state {
        ActiveState::Appending { file, base, kept, expected, digest, written, based } => {
            let tail = digest.finalize().chunks;
            let res: Result<(), String> = (|| {
                if !based || written != af.expected_size {
                    return Err(format!("size mismatch: wrote {written}, expected {}", af.expected_size));
                }
                if tail != expected {
                    return Err("the append's chunks aren't the sender's".into());
                }
                file.set_permissions(fs::Permissions::from_mode(af.mode & 0o777)).map_err(|e| format!("set mode: {e}"))?;
                if let Some(st) = ns_to_system_time(af.mtime_ns) {
                    let _ = file.set_modified(st);
                }
                Ok(())
            })();
            match res {
                Ok(()) => {
                    if let Ok(meta) = file.metadata() {
                        let mut chunks = kept;
                        chunks.extend(tail);
                        recv_index.entries.insert(
                            af.path.clone(),
                            Entry {
                                path: af.path.clone(),
                                kind: EntryKind::File,
                                size: af.expected_size,
                                mtime_ns: meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
                                ctime_ns: meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
                                ino: meta.ino(),
                                mode: af.mode & 0o777,
                                // (The sender's, of a file whose every chunk is
                                // checked: the old ones by the index, the new
                                // ones here.)
                                hash: af.expected_hash.unwrap_or([0u8; 32]),
                                chunks,
                            },
                        );
                    } else {
                        recv_index.entries.remove(&af.path);
                    }
                    report.appended += 1;
                }
                Err(reason) => {
                    let _ = file.set_len(base);
                    recv_index.entries.remove(&af.path);
                    report.errors.push(PathError { path: af.path, reason });
                }
            }
        }
        ActiveState::Skipped { already_recorded, error } => {
            if !already_recorded && let Some(reason) = error {
                report.errors.push(PathError { path: af.path, reason });
            }
        }
        ActiveState::Writing { part_path, writer, digest, written } => {
            let res: Result<(), String> = (|| {
                let file = writer.finish().map_err(|e| format!("finish temp: {e}"))?;
                let computed = digest.finalize();
                if written != af.expected_size || computed.size != af.expected_size {
                    return Err(format!("size mismatch: wrote {}, expected {}", computed.size, af.expected_size));
                }
                let Some(expected_hash) = af.expected_hash else {
                    return Err("missing BLAKE3 hash".into());
                };
                if computed.hash != expected_hash {
                    return Err("BLAKE3 verification failed".into());
                }
                file.set_permissions(fs::Permissions::from_mode(af.mode & 0o777)).map_err(|e| format!("set mode: {e}"))?;
                if let Some(st) = ns_to_system_time(af.mtime_ns) {
                    let _ = file.set_modified(st);
                }
                drop(file);

                let dst = root.join(&af.path);
                if let Ok(meta) = fs::symlink_metadata(&dst)
                    && meta.file_type().is_dir()
                {
                    if contains_protected(protect, &af.path) {
                        return Err(REASON_PROTECTED.into());
                    }
                    if !af.replace_dirs {
                        return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
                    }
                    fs::remove_dir_all(&dst).map_err(|e| format!("replace dir: {e}"))?;
                }
                fs::rename(&part_path, &dst).map_err(|e| format!("rename into place: {e}"))?;

                let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat dst: {e}"))?;
                let mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
                let ctime_ns = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
                recv_index.entries.insert(
                    af.path.clone(),
                    Entry {
                        path: af.path.clone(),
                        kind: EntryKind::File,
                        size: computed.size,
                        mtime_ns,
                        ctime_ns,
                        ino: meta.ino(),
                        mode: af.mode & 0o777,
                        hash: computed.hash,
                        chunks: computed.chunks,
                    },
                );
                Ok(())
            })();

            match res {
                Ok(()) => match af.kind {
                    RecipeKind::Append => report.appended += 1,
                    RecipeKind::Patch => report.patched += 1,
                    RecipeKind::New => report.new += 1,
                },
                Err(reason) => {
                    let _ = fs::remove_file(&part_path);
                    report.errors.push(PathError { path: af.path, reason });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Receiver filesystem helpers (strict root confinement, zero symlink follows)
// ---------------------------------------------------------------------------

fn is_protected(protect: &[String], path: &str) -> bool {
    protect.iter().any(|raw| {
        let p = raw.trim_matches('/');
        !p.is_empty() && (path == p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/')))
    })
}

fn contains_protected(protect: &[String], dir_path: &str) -> bool {
    protect.iter().any(|raw| {
        let p = raw.trim_matches('/');
        !p.is_empty() && p.strip_prefix(dir_path).is_some_and(|rest| rest.starts_with('/'))
    })
}

/// A file being received, made new beside its place: any left there before
/// (by a session that ended) taken away first, and never opened through a
/// link left at its name; its owner's alone until it is whole.
fn create_part(part: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    match fs::remove_file(part) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(part)
}

/// Fixed-length temporary file name in the same folder as `rel`
/// (`.olo-sync-part-<16 hex>`), fitting `NAME_MAX` even for 255-byte names.
fn part_path_for(root: &Path, rel: &str) -> PathBuf {
    use std::fmt::Write as _;
    let dst = root.join(rel);
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let h = blake3::hash(name.as_bytes());
    let mut part_name = String::with_capacity(PART_PREFIX.len() + 16);
    part_name.push_str(PART_PREFIX);
    for b in &h.as_bytes()[..8] {
        let _ = write!(part_name, "{b:02x}");
    }
    match dst.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(part_name),
        _ => PathBuf::from(part_name),
    }
}

fn ensure_safe_parent_dirs(root: &Path, rel: &str, protect: &[String], pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>) -> Result<(), String> {
    validate_rel_path(rel).map_err(|e| format!("refused path: {e}"))?;
    let mut cur = root.to_path_buf();
    let mut anc_rel = String::new();
    let parts: Vec<&str> = rel.split('/').collect();
    for &seg in &parts[..parts.len() - 1] {
        if !anc_rel.is_empty() {
            anc_rel.push('/');
        }
        anc_rel.push_str(seg);
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(format!("refused path: traverses symlink at '{seg}'"));
                }
                if !meta.file_type().is_dir() {
                    if is_protected(protect, &anc_rel) {
                        return Err(REASON_PROTECTED.into());
                    }
                    fs::remove_file(&cur).map_err(|e| format!("replace non-dir '{seg}': {e}"))?;
                    fs::create_dir(&cur).map_err(|e| format!("mkdir '{seg}': {e}"))?;
                } else {
                    let cur_mode = meta.permissions().mode();
                    if (cur_mode & 0o700) != 0o700 {
                        if is_protected(protect, &anc_rel) {
                            return Err(REASON_PROTECTED.into());
                        }
                        let cur_mtime = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
                        pending_dirs.entry(anc_rel.clone()).or_insert((final_dir_mode(cur_mode), Some(cur_mtime)));
                        fs::set_permissions(&cur, fs::Permissions::from_mode(dir_mode(cur_mode))).map_err(|e| format!("chmod parent '{seg}': {e}"))?;
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if is_protected(protect, &anc_rel) {
                    return Err(REASON_PROTECTED.into());
                }
                fs::create_dir(&cur).map_err(|e| format!("mkdir '{seg}': {e}"))?;
            }
            Err(e) => return Err(format!("stat '{seg}': {e}")),
        }
    }
    Ok(())
}

fn ensure_writable_existing_parents(root: &Path, rel: &str, protect: &[String], pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>) -> Result<(), String> {
    validate_rel_path(rel).map_err(|e| format!("refused path: {e}"))?;
    let mut cur = root.to_path_buf();
    let mut anc_rel = String::new();
    let parts: Vec<&str> = rel.split('/').collect();
    for &seg in &parts[..parts.len() - 1] {
        if !anc_rel.is_empty() {
            anc_rel.push('/');
        }
        anc_rel.push_str(seg);
        cur.push(seg);
        if let Ok(meta) = fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                return Err(format!("refused path: traverses symlink at '{seg}'"));
            }
            if meta.file_type().is_dir() {
                let cur_mode = meta.permissions().mode();
                if (cur_mode & 0o700) != 0o700 {
                    if is_protected(protect, &anc_rel) {
                        return Err(REASON_PROTECTED.into());
                    }
                    let cur_mtime = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
                    pending_dirs.entry(anc_rel.clone()).or_insert((final_dir_mode(cur_mode), Some(cur_mtime)));
                    fs::set_permissions(&cur, fs::Permissions::from_mode(dir_mode(cur_mode))).map_err(|e| format!("chmod parent '{seg}': {e}"))?;
                }
            }
        }
    }
    Ok(())
}

fn open_no_symlink_components(root: &Path, rel: &str) -> io::Result<fs::File> {
    validate_rel_path(rel).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut cur = root.to_path_buf();
    for seg in rel.split('/') {
        cur.push(seg);
        let meta = fs::symlink_metadata(&cur)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("path traverses symlink at '{seg}'")));
        }
    }
    fs::File::open(cur)
}

fn check_no_symlink_ancestors(root: &Path, rel: &str) -> Result<(), String> {
    validate_rel_path(rel).map_err(|e| format!("refused path: {e}"))?;
    let mut cur = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for &seg in &parts[..parts.len() - 1] {
        cur.push(seg);
        if let Ok(meta) = fs::symlink_metadata(&cur)
            && meta.file_type().is_symlink()
        {
            return Err(format!("refused path: traverses symlink at '{seg}'"));
        }
    }
    Ok(())
}

fn apply_mkdir(root: &Path, path: &str, mode: u32, mtime_ns: i64, protect: &[String], pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>, recv_index: &mut Index) -> Result<(), String> {
    validate_rel_path(path).map_err(|e| format!("refused path: {e}"))?;
    if is_protected(protect, path) {
        return Err(REASON_PROTECTED.into());
    }
    ensure_safe_parent_dirs(root, path, protect, pending_dirs)?;
    let dst = root.join(path);
    match fs::symlink_metadata(&dst) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                fs::remove_file(&dst).map_err(|e| format!("remove before mkdir: {e}"))?;
                fs::create_dir(&dst).map_err(|e| format!("mkdir: {e}"))?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&dst).map_err(|e| format!("mkdir: {e}"))?;
        }
        Err(e) => return Err(format!("stat dir: {e}")),
    }
    let work_mode = dir_mode(mode);
    let target_mode = final_dir_mode(mode);
    fs::set_permissions(&dst, fs::Permissions::from_mode(work_mode)).map_err(|e| format!("chmod dir: {e}"))?;
    pending_dirs.insert(path.to_owned(), (target_mode, Some(mtime_ns)));
    let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat dir: {e}"))?;
    recv_index.entries.insert(
        path.to_owned(),
        Entry {
            path: path.to_owned(),
            kind: EntryKind::Dir,
            size: 0,
            mtime_ns: meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
            ctime_ns: meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
            ino: meta.ino(),
            mode: target_mode,
            hash: [0u8; 32],
            chunks: Vec::new(),
        },
    );
    Ok(())
}

fn apply_symlink(root: &Path, path: &str, target: &str, _mtime_ns: i64, options: &Options, pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>, recv_index: &mut Index) -> Result<(), String> {
    validate_rel_path(path).map_err(|e| format!("refused path: {e}"))?;
    if is_protected(&options.protect, path) {
        return Err(REASON_PROTECTED.into());
    }
    ensure_safe_parent_dirs(root, path, &options.protect, pending_dirs)?;
    let dst = root.join(path);
    if fs::symlink_metadata(&dst).is_ok_and(|m| m.is_dir()) {
        if contains_protected(&options.protect, path) {
            return Err(REASON_PROTECTED.into());
        }
        if !options.delete_extra {
            return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
        }
    }
    let part = part_path_for(root, path);
    let _ = fs::remove_file(&part);
    symlink(target, &part).map_err(|e| format!("create symlink: {e}"))?;
    if let Ok(meta) = fs::symlink_metadata(&dst)
        && meta.file_type().is_dir()
    {
        fs::remove_dir_all(&dst).map_err(|e| format!("replace dir with symlink: {e}"))?;
    }
    if let Err(e) = fs::rename(&part, &dst) {
        let _ = fs::remove_file(&part);
        return Err(format!("rename symlink: {e}"));
    }
    let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat symlink: {e}"))?;
    recv_index.entries.insert(
        path.to_owned(),
        Entry {
            path: path.to_owned(),
            kind: EntryKind::Symlink(target.to_owned()),
            size: target.len() as u64,
            mtime_ns: meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
            ctime_ns: meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
            ino: meta.ino(),
            mode: meta.permissions().mode() & 0o777,
            hash: *blake3::hash(target.as_bytes()).as_bytes(),
            chunks: Vec::new(),
        },
    );
    Ok(())
}

fn apply_hardlink(root: &Path, path: &str, target: &str, options: &Options, pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>, recv_index: &mut Index) -> Result<(), String> {
    validate_rel_path(path).map_err(|e| format!("refused path: {e}"))?;
    validate_rel_path(target).map_err(|e| format!("refused hardlink target '{target}': {e}"))?;
    if is_protected(&options.protect, path) {
        return Err(REASON_PROTECTED.into());
    }
    ensure_safe_parent_dirs(root, path, &options.protect, pending_dirs)?;

    // (Never a link to what the receiver keeps as its own: a mode set on
    // the link would be set on it.)
    if is_protected(&options.protect, target) {
        return Err(REASON_PROTECTED.into());
    }
    // Verify target traverses no symlinks and is itself a regular file.
    let mut target_cur = root.to_path_buf();
    for seg in target.split('/') {
        target_cur.push(seg);
        let meta = fs::symlink_metadata(&target_cur).map_err(|e| format!("stat hardlink target '{target}': {e}"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("refused hardlink target '{target}': traverses symlink at '{seg}'"));
        }
    }
    let target_meta = fs::symlink_metadata(&target_cur).map_err(|e| format!("stat hardlink target '{target}': {e}"))?;
    if !target_meta.file_type().is_file() {
        return Err(format!("hardlink target '{target}' isn't a regular file"));
    }

    let dst = root.join(path);
    if let Ok(dst_meta) = fs::symlink_metadata(&dst) {
        if dst_meta.file_type().is_dir() {
            if contains_protected(&options.protect, path) {
                return Err(REASON_PROTECTED.into());
            }
            if !options.delete_extra {
                return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
            }
        } else if dst_meta.file_type().is_file() && dst_meta.dev() == target_meta.dev() && dst_meta.ino() == target_meta.ino() {
            // Already linked to the exact target inode.
            recv_index.entries.insert(
                path.to_owned(),
                Entry {
                    path: path.to_owned(),
                    kind: EntryKind::HardLink(target.to_owned()),
                    size: 0,
                    mtime_ns: dst_meta.mtime().saturating_mul(1_000_000_000).saturating_add(dst_meta.mtime_nsec()),
                    ctime_ns: dst_meta.ctime().saturating_mul(1_000_000_000).saturating_add(dst_meta.ctime_nsec()),
                    ino: dst_meta.ino(),
                    mode: dst_meta.permissions().mode() & 0o777,
                    hash: [0u8; 32],
                    chunks: Vec::new(),
                },
            );
            return Ok(());
        }
    }

    let part = part_path_for(root, path);
    let _ = fs::remove_file(&part);
    fs::hard_link(&target_cur, &part).map_err(|e| format!("create hardlink: {e}"))?;
    if let Ok(meta) = fs::symlink_metadata(&dst)
        && meta.file_type().is_dir()
    {
        fs::remove_dir_all(&dst).map_err(|e| format!("replace dir with hardlink: {e}"))?;
    }
    if let Err(e) = fs::rename(&part, &dst) {
        let _ = fs::remove_file(&part);
        return Err(format!("rename hardlink: {e}"));
    }
    let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat hardlink: {e}"))?;
    recv_index.entries.insert(
        path.to_owned(),
        Entry {
            path: path.to_owned(),
            kind: EntryKind::HardLink(target.to_owned()),
            size: 0,
            mtime_ns: meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
            ctime_ns: meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
            ino: meta.ino(),
            mode: meta.permissions().mode() & 0o777,
            hash: [0u8; 32],
            chunks: Vec::new(),
        },
    );
    Ok(())
}

fn apply_set_meta(root: &Path, path: &str, mode: u32, mtime_ns: i64, protect: &[String], recv_index: &mut Index) -> Result<(), String> {
    validate_rel_path(path).map_err(|e| format!("refused path: {e}"))?;
    if is_protected(protect, path) {
        return Err(REASON_PROTECTED.into());
    }
    check_no_symlink_ancestors(root, path)?;
    let dst = root.join(path);
    let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err("refused SetMeta on symlink".into());
    }
    let clean_mode = if meta.file_type().is_dir() { final_dir_mode(mode) } else { mode & 0o777 };
    fs::set_permissions(&dst, fs::Permissions::from_mode(clean_mode)).map_err(|e| format!("chmod: {e}"))?;
    if meta.file_type().is_file()
        && let Some(st) = ns_to_system_time(mtime_ns)
        && let Ok(f) = fs::File::open(&dst)
    {
        let _ = f.set_modified(st);
    }
    if let Some(entry) = recv_index.entries.get_mut(path) {
        let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat: {e}"))?;
        entry.mode = clean_mode;
        entry.mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
        entry.ctime_ns = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
    }
    Ok(())
}

enum CopyOutcome {
    Copied,
    LeftNewer,
}

#[allow(clippy::too_many_arguments)]
fn apply_copy_file(
    root: &Path,
    path: &str,
    from: &str,
    mode: u32,
    mtime_ns: i64,
    size: u64,
    expected_hash: [u8; 32],
    options: &Options,
    pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>,
    recv_index: &mut Index,
    copy_buf: &mut [u8],
) -> Result<CopyOutcome, String> {
    validate_rel_path(path).map_err(|e| format!("refused path: {e}"))?;
    validate_rel_path(from).map_err(|e| format!("refused basis path '{from}': {e}"))?;
    if is_protected(&options.protect, path) {
        return Err(REASON_PROTECTED.into());
    }
    if options.keep_newer
        && let Some(existing) = recv_index.entries.get(path)
        && existing.kind == EntryKind::File
        && existing.mtime_ns > mtime_ns
    {
        return Ok(CopyOutcome::LeftNewer);
    }
    ensure_safe_parent_dirs(root, path, &options.protect, pending_dirs)?;
    if fs::symlink_metadata(root.join(path)).is_ok_and(|m| m.is_dir()) {
        if contains_protected(&options.protect, path) {
            return Err(REASON_PROTECTED.into());
        }
        if !options.delete_extra {
            return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
        }
    }

    let mut src = open_no_symlink_components(root, from).map_err(|e| format!("open '{from}': {e}"))?;
    let part = part_path_for(root, path);
    let dst_file = create_part(&part).map_err(|e| format!("create temp: {e}"))?;
    let mut dst_writer = SparseWriter::new(dst_file);

    let reuse_chunks = recv_index.entries.get(from).filter(|e| e.kind == EntryKind::File && e.hash == expected_hash && e.size == size).map(|e| e.chunks.clone());

    let res: Result<Vec<ChunkRef>, String> = (|| {
        let mut hasher = blake3::Hasher::new();
        let mut sd = if size >= chunk::CHUNK_MIN_SIZE && reuse_chunks.as_ref().is_none_or(Vec::is_empty) { Some(StreamDigest::new()) } else { None };
        let mut copied = 0u64;
        loop {
            let n = src.read(copy_buf).map_err(|e| format!("read '{from}': {e}"))?;
            if n == 0 {
                break;
            }
            dst_writer.write_data(&copy_buf[..n]).map_err(|e| format!("write temp: {e}"))?;
            hasher.update(&copy_buf[..n]);
            if let Some(d) = sd.as_mut() {
                d.update(&copy_buf[..n]);
            }
            copied += n as u64;
        }
        let dst_file = dst_writer.finish().map_err(|e| format!("finish temp: {e}"))?;
        let computed_hash = *hasher.finalize().as_bytes();
        if copied != size || computed_hash != expected_hash {
            return Err("copied file BLAKE3 mismatch".into());
        }
        dst_file.set_permissions(fs::Permissions::from_mode(mode & 0o777)).map_err(|e| format!("chmod: {e}"))?;
        if let Some(st) = ns_to_system_time(mtime_ns) {
            let _ = dst_file.set_modified(st);
        }
        drop(dst_file);
        let dst = root.join(path);
        if let Ok(meta) = fs::symlink_metadata(&dst)
            && meta.file_type().is_dir()
        {
            fs::remove_dir_all(&dst).map_err(|e| format!("replace dir: {e}"))?;
        }
        fs::rename(&part, &dst).map_err(|e| format!("rename: {e}"))?;
        Ok(match sd {
            Some(d) => d.finalize().chunks,
            None => reuse_chunks.unwrap_or_default(),
        })
    })();

    match res {
        Ok(chunks) => {
            let dst = root.join(path);
            let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat dst: {e}"))?;
            recv_index.entries.insert(
                path.to_owned(),
                Entry {
                    path: path.to_owned(),
                    kind: EntryKind::File,
                    size,
                    mtime_ns: meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
                    ctime_ns: meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
                    ino: meta.ino(),
                    mode: mode & 0o777,
                    hash: expected_hash,
                    chunks,
                },
            );
            Ok(CopyOutcome::Copied)
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

fn apply_remove(root: &Path, path: &str, protect: &[String], pending_dirs: &mut BTreeMap<String, (u32, Option<i64>)>, recv_index: &mut Index) -> Result<bool, String> {
    if is_protected(protect, path) || contains_protected(protect, path) {
        return Err(REASON_PROTECTED.into());
    }
    ensure_writable_existing_parents(root, path, protect, pending_dirs)?;
    let dst = root.join(path);
    match fs::symlink_metadata(&dst) {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                match fs::remove_dir(&dst) {
                    Ok(()) => {
                        pending_dirs.remove(path);
                        recv_index.entries.remove(path);
                        Ok(true)
                    }
                    Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(false),
                    Err(e) => Err(format!("rmdir: {e}")),
                }
            } else {
                fs::remove_file(&dst).map_err(|e| format!("remove: {e}"))?;
                recv_index.entries.remove(path);
                Ok(true)
            }
        }
        // (Gone already, or with its folder: one replaced by a file first,
        // what was in it is taken away after.)
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {
            pending_dirs.remove(path);
            recv_index.entries.remove(path);
            Ok(false)
        }
        Err(e) => Err(format!("stat for remove: {e}")),
    }
}

/// Set each folder the receiver created or temporarily made writable to its
/// final mode (`0o3777`, keeping setgid & sticky, never setuid) and mtime,
/// deepest first so read-only parent folders don't block their children.
fn finalize_dirs(root: &Path, pending_dirs: BTreeMap<String, (u32, Option<i64>)>, recv_index: &mut Index, report: &mut Report) {
    let mut dirs: Vec<(String, u32, Option<i64>)> = pending_dirs.into_iter().map(|(p, (m, t))| (p, m, t)).collect();
    dirs.sort_by(|a, b| b.0.matches('/').count().cmp(&a.0.matches('/').count()).then_with(|| b.0.cmp(&a.0)));
    for (path, target_mode, target_mtime) in dirs {
        let dst = root.join(&path);
        match fs::symlink_metadata(&dst) {
            Ok(m) if m.file_type().is_dir() => {}
            _ => continue,
        }
        if let Some(ns) = target_mtime
            && let Some(st) = ns_to_system_time(ns)
            && let Ok(f) = fs::File::open(&dst)
        {
            let _ = f.set_modified(st);
        }
        let final_mode = final_dir_mode(target_mode);
        if let Err(e) = fs::set_permissions(&dst, fs::Permissions::from_mode(final_mode)) {
            report.errors.push(PathError { path, reason: format!("chmod dir: {e}") });
            continue;
        }
        if let Ok(meta) = fs::symlink_metadata(&dst)
            && let Some(entry) = recv_index.entries.get_mut(&path)
        {
            entry.mode = final_dir_mode(meta.permissions().mode());
            entry.mtime_ns = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
            entry.ctime_ns = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
        }
    }
}

fn ns_to_system_time(ns: i64) -> Option<SystemTime> {
    if ns >= 0 { UNIX_EPOCH.checked_add(Duration::from_nanos(ns as u64)) } else { UNIX_EPOCH.checked_sub(Duration::from_nanos(ns.unsigned_abs())) }
}

fn filter_index(index: &mut Index, filter: &Filter, unhashed: &mut HashSet<String>) {
    if filter.recursive && filter.exclude.is_empty() && filter.include.is_empty() && filter.only.is_empty() {
        return;
    }
    let keep_path = |path: &str, kind: &EntryKind| -> bool {
        if !filter.recursive && path.contains('/') {
            return false;
        }
        match kind {
            EntryKind::Dir => filter.allow_dir(path),
            EntryKind::File | EntryKind::Symlink(_) | EntryKind::HardLink(_) => {
                // Ensure no ancestor directory is excluded.
                let mut prefix = String::new();
                let segs: Vec<&str> = path.split('/').collect();
                for &seg in &segs[..segs.len() - 1] {
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(seg);
                    if !filter.allow_dir(&prefix) {
                        return false;
                    }
                }
                filter.matches_entry(path)
            }
        }
    };

    // If a hard link is kept while its target is filtered out, promote the
    // first kept twin to a full File entry (borrowing its target's content
    // metadata and chunks) and repoint any remaining twins to it.
    let mut promoted: HashMap<String, String> = HashMap::new();
    let mut updates: Vec<(String, Entry)> = Vec::new();
    for (path, entry) in &index.entries {
        if let EntryKind::HardLink(target) = &entry.kind
            && keep_path(path, &entry.kind)
        {
            let target_kept = index.entries.get(target).is_some_and(|t| keep_path(target, &t.kind));
            if !target_kept {
                if let Some(new_target) = promoted.get(target) {
                    let mut updated = entry.clone();
                    updated.kind = EntryKind::HardLink(new_target.clone());
                    updates.push((path.clone(), updated));
                } else if let Some(orig) = index.entries.get(target) {
                    let mut updated = orig.clone();
                    updated.path = path.clone();
                    if unhashed.contains(target) {
                        unhashed.insert(path.clone());
                    }
                    promoted.insert(target.clone(), path.clone());
                    updates.push((path.clone(), updated));
                }
            }
        }
    }
    for (path, entry) in updates {
        index.entries.insert(path, entry);
    }
    index.entries.retain(|path, entry| keep_path(path, &entry.kind));
    unhashed.retain(|path| index.entries.contains_key(path));
    index.mounts.retain(|path| keep_path(path, &EntryKind::Dir));
    index.special.retain(|path| keep_path(path, &EntryKind::File));
    if !filter.only.is_empty() {
        // `filter.only` names only valid UTF-8 paths; non-UTF-8 siblings are
        // outside the requested scope.
        index.non_utf8.clear();
    } else {
        index.non_utf8.retain(|path| keep_path(path, &EntryKind::File));
    }
    index.unread.retain(|path, _| path == "." || keep_path(path, &EntryKind::File) || keep_path(path, &EntryKind::Dir));
}

// ---------------------------------------------------------------------------
// Wire framing & message codecs
// ---------------------------------------------------------------------------

struct Wire<S: Read + Write> {
    inner: S,
    write_buf: Vec<u8>,
    bytes_sent: u64,
    bytes_received: u64,
}

impl<S: Read + Write> Wire<S> {
    fn new(inner: S) -> Self {
        Self { inner, write_buf: Vec::with_capacity(64 << 10), bytes_sent: 0, bytes_received: 0 }
    }

    fn write_frame(&mut self, tag: u8, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_FRAME_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("frame payload {} exceeds limit {MAX_FRAME_LEN}", payload.len())));
        }
        let header_and_body = 5 + payload.len();
        if self.write_buf.len() + header_and_body > 64 << 10 && !self.write_buf.is_empty() {
            self.flush()?;
        }
        let len = payload.len() as u32;
        self.write_buf.extend_from_slice(&len.to_le_bytes());
        self.write_buf.push(tag);
        self.write_buf.extend_from_slice(payload);
        self.bytes_sent += header_and_body as u64;
        if self.write_buf.len() >= 64 << 10 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.write_buf.is_empty() {
            self.inner.write_all(&self.write_buf)?;
            self.write_buf.clear();
        }
        self.inner.flush()
    }

    fn read_frame(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut header = [0u8; 5];
        self.inner.read_exact(&mut header)?;
        self.bytes_received += 5;
        let len = u32::from_le_bytes(header[0..4].try_into().unwrap()) as usize;
        let tag = header[4];
        if len > MAX_FRAME_LEN {
            return Err(invalid_data(format!("frame length {len} exceeds maximum {MAX_FRAME_LEN}")));
        }
        // Read in bounded chunks so a lying peer that advertises a huge length
        // and closes the stream never causes an allocation of `len` bytes.
        let mut payload = Vec::with_capacity(len.min(64 << 10));
        let mut rem = len;
        let mut tmp = [0u8; 16 << 10];
        while rem > 0 {
            let take = rem.min(tmp.len());
            self.inner.read_exact(&mut tmp[..take])?;
            payload.extend_from_slice(&tmp[..take]);
            self.bytes_received += take as u64;
            rem -= take;
        }
        Ok((tag, payload))
    }
}

struct HelloMsg {
    version: u32,
    flags: u8,
    tree_hash: [u8; 32],
    _file_count: u64,
}

fn decode_hello(payload: &[u8]) -> io::Result<HelloMsg> {
    let mut cur = Cursor::new(payload);
    let version = cur.read_u32()?;
    let flags = cur.read_u8()?;
    let tree_hash = cur.read_array::<32>()?;
    let file_count = cur.read_u64()?;
    cur.expect_eof()?;
    Ok(HelloMsg { version, flags, tree_hash, _file_count: file_count })
}

struct ManifestEntry {
    path: String,
    kind: EntryKind,
    size: u64,
    mtime_ns: i64,
    mode: u32,
    hash: [u8; 32],
}

fn send_manifest<S: Read + Write>(wire: &mut Wire<S>, index: &Index) -> io::Result<()> {
    let mut chunk_entries: Vec<&Entry> = Vec::new();
    let mut est_bytes = 4usize;

    let total = index.entries.len();
    let mut sent = 0usize;
    for entry in index.entries.values() {
        let extra = match &entry.kind {
            EntryKind::File => 32,
            EntryKind::Dir => 0,
            EntryKind::Symlink(target) | EntryKind::HardLink(target) => 4 + target.len(),
        };
        let entry_sz = 4 + entry.path.len() + 1 + 8 + 8 + 4 + extra;
        if !chunk_entries.is_empty() && (est_bytes + entry_sz > MAX_MANIFEST_CHUNK_RAW || chunk_entries.len() >= MAX_ITEMS_PER_FRAME) {
            write_manifest_batch(wire, &chunk_entries, true)?;
            chunk_entries.clear();
            est_bytes = 4;
        }
        chunk_entries.push(entry);
        est_bytes += entry_sz;
        sent += 1;
        if sent == total {
            write_manifest_batch(wire, &chunk_entries, false)?;
            return Ok(());
        }
    }
    // Empty index still sends one empty manifest frame with `more = 0`.
    write_manifest_batch(wire, &[], false)
}

/// The sender's paths, to a scoped receiver: its files first (so the
/// receiver's `scan_paths` sees the first file before any hard link to it),
/// then links and folders, in compressed batches.
fn send_scope<S: Read + Write>(wire: &mut Wire<S>, index: &Index) -> io::Result<()> {
    let mut paths: Vec<&String> = index.entries.values().filter(|e| e.kind == EntryKind::File).map(|e| &e.path).collect();
    paths.extend(index.entries.values().filter(|e| matches!(e.kind, EntryKind::HardLink(_) | EntryKind::Symlink(_) | EntryKind::Dir)).map(|e| &e.path));
    let mut start = 0;
    loop {
        let (mut raw, mut end, mut size) = (Vec::new(), start, 4usize);
        while end < paths.len() && (end == start || (size + 4 + paths[end].len() <= MAX_MANIFEST_CHUNK_RAW && end - start < MAX_ITEMS_PER_FRAME)) {
            size += 4 + paths[end].len();
            end += 1;
        }
        raw.extend_from_slice(&((end - start) as u32).to_le_bytes());
        for p in &paths[start..end] {
            write_str(&mut raw, p);
        }
        let more = end < paths.len();
        write_compressed_batch(wire, TAG_SCOPE, more, &raw)?;
        if !more {
            return Ok(());
        }
        start = end;
    }
}

fn decode_scope_frame(payload: &[u8], out: &mut Vec<String>) -> io::Result<bool> {
    let (more, raw) = read_compressed_batch(payload, MAX_MANIFEST_CHUNK_RAW + (256 << 10))?;
    let mut cur = Cursor::new(&raw);
    let count = cur.read_u32()? as usize;
    if count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many paths in a scope frame"));
    }
    for _ in 0..count {
        out.push(cur.read_str(MAX_PATH_LEN)?);
    }
    cur.expect_eof()?;
    Ok(more)
}

fn write_manifest_batch<S: Read + Write>(wire: &mut Wire<S>, entries: &[&Entry], more: bool) -> io::Result<()> {
    let mut raw = Vec::with_capacity(4 + entries.len() * 64);
    raw.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        write_str(&mut raw, &entry.path);
        match &entry.kind {
            EntryKind::File => raw.push(0),
            EntryKind::Dir => raw.push(1),
            EntryKind::Symlink(target) => {
                raw.push(2);
                write_str(&mut raw, target);
            }
            EntryKind::HardLink(target) => {
                raw.push(3);
                write_str(&mut raw, target);
            }
        }
        raw.extend_from_slice(&entry.size.to_le_bytes());
        raw.extend_from_slice(&entry.mtime_ns.to_le_bytes());
        raw.extend_from_slice(&entry.mode.to_le_bytes());
        if matches!(entry.kind, EntryKind::File) {
            raw.extend_from_slice(&entry.hash);
        }
    }
    write_compressed_batch(wire, TAG_MANIFEST, more, &raw)
}

fn decode_manifest_frame(payload: &[u8], out: &mut BTreeMap<String, ManifestEntry>) -> io::Result<bool> {
    let (more, raw) = read_compressed_batch(payload, MAX_MANIFEST_CHUNK_RAW + (256 << 10))?;
    let mut cur = Cursor::new(&raw);
    let count = cur.read_u32()? as usize;
    if count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many entries in manifest frame"));
    }
    for _ in 0..count {
        let path = cur.read_str(MAX_PATH_LEN)?;
        let kind_tag = cur.read_u8()?;
        let kind = match kind_tag {
            0 => EntryKind::File,
            1 => EntryKind::Dir,
            2 => EntryKind::Symlink(cur.read_str(MAX_PATH_LEN)?),
            3 => EntryKind::HardLink(cur.read_str(MAX_PATH_LEN)?),
            _ => return Err(invalid_data(format!("unknown manifest kind {kind_tag}"))),
        };
        let size = cur.read_u64()?;
        let mtime_ns = cur.read_i64()?;
        let mode = cur.read_u32()?;
        let hash = if matches!(kind, EntryKind::File) { cur.read_array::<32>()? } else { [0u8; 32] };
        out.insert(path.clone(), ManifestEntry { path, kind, size, mtime_ns, mode, hash });
    }
    cur.expect_eof()?;
    Ok(more)
}

enum ChunkReq {
    AppendOrSuper { path: String, probe: AppendProbe },
    SuperOrLeaf { path: String },
}

enum ChunkResp {
    AppendConfirmed,
    LeafChunks(Vec<ChunkRef>),
    SuperChunks(Vec<SuperChunk>),
}

fn send_need_chunks<S: Read + Write>(wire: &mut Wire<S>, reqs: &[ChunkReq]) -> io::Result<()> {
    let mut raw = Vec::with_capacity(4 + reqs.len().min(1024) * 64);
    let mut batch_count = 0u32;
    raw.extend_from_slice(&0u32.to_le_bytes());

    for req in reqs {
        let item_sz = match req {
            ChunkReq::AppendOrSuper { path, .. } => 1 + 4 + path.len() + 4 + 32 + 4 + 16,
            ChunkReq::SuperOrLeaf { path } => 1 + 4 + path.len(),
        };
        if batch_count > 0 && (raw.len() + item_sz > MAX_MANIFEST_CHUNK_RAW || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
            raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
            write_compressed_batch(wire, TAG_NEED_CHUNKS, true, &raw)?;
            raw.clear();
            raw.extend_from_slice(&0u32.to_le_bytes());
            batch_count = 0;
        }
        match req {
            ChunkReq::AppendOrSuper { path, probe } => {
                raw.push(1);
                write_str(&mut raw, path);
                raw.extend_from_slice(&probe.prefix_chunk_count.to_le_bytes());
                raw.extend_from_slice(&probe.prefix_chunks_hash);
                raw.extend_from_slice(&probe.last_chunk.len.to_le_bytes());
                raw.extend_from_slice(&probe.last_chunk.id);
            }
            ChunkReq::SuperOrLeaf { path } => {
                raw.push(0);
                write_str(&mut raw, path);
            }
        }
        batch_count += 1;
    }
    raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
    write_compressed_batch(wire, TAG_NEED_CHUNKS, false, &raw)
}

fn decode_need_chunks_frame(payload: &[u8], out: &mut Vec<ChunkReq>) -> io::Result<bool> {
    let (more, raw) = read_compressed_batch(payload, MAX_FRAME_LEN)?;
    let mut cur = Cursor::new(&raw);
    let count = cur.read_u32()? as usize;
    if count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many chunk requests"));
    }
    for _ in 0..count {
        let kind = cur.read_u8()?;
        let path = cur.read_str(MAX_PATH_LEN)?;
        match kind {
            0 => out.push(ChunkReq::SuperOrLeaf { path }),
            1 => {
                let prefix_chunk_count = cur.read_u32()?;
                let prefix_chunks_hash = cur.read_array::<32>()?;
                let len = cur.read_u32()?;
                let id = cur.read_array::<16>()?;
                out.push(ChunkReq::AppendOrSuper { path, probe: AppendProbe { prefix_chunk_count, prefix_chunks_hash, last_chunk: ChunkRef { len, id } } });
            }
            _ => return Err(invalid_data("unknown chunk request kind")),
        }
    }
    cur.expect_eof()?;
    Ok(more)
}

fn send_chunk_info<S: Read + Write>(wire: &mut Wire<S>, recv_index: &Index, reqs: &[ChunkReq], super_cache: &mut HashMap<String, Vec<SuperChunk>>) -> io::Result<()> {
    send_chunk_info_with_limit(wire, recv_index, reqs, super_cache, MAX_MANIFEST_CHUNK_RAW)
}

fn send_chunk_info_with_limit<S: Read + Write>(wire: &mut Wire<S>, recv_index: &Index, reqs: &[ChunkReq], super_cache: &mut HashMap<String, Vec<SuperChunk>>, max_raw: usize) -> io::Result<()> {
    let mut raw = Vec::new();
    let mut batch_count = 0u32;
    raw.extend_from_slice(&0u32.to_le_bytes());

    let flush = |wire: &mut Wire<S>, raw: &mut Vec<u8>, batch_count: &mut u32, more: bool| -> io::Result<()> {
        raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
        write_compressed_batch(wire, TAG_CHUNK_INFO, more, raw)?;
        raw.clear();
        raw.extend_from_slice(&0u32.to_le_bytes());
        *batch_count = 0;
        Ok(())
    };

    for req in reqs {
        let path = match req {
            ChunkReq::AppendOrSuper { path, .. } | ChunkReq::SuperOrLeaf { path } => path,
        };
        let Some(entry) = recv_index.entries.get(path).filter(|e| e.kind == EntryKind::File && !e.chunks.is_empty()) else {
            continue;
        };
        let confirmed = match req {
            ChunkReq::AppendOrSuper { probe, .. } => recipe::verify_append_probe(entry, probe),
            ChunkReq::SuperOrLeaf { .. } => false,
        };
        if confirmed {
            let item_sz = 1 + 4 + path.len();
            if batch_count > 0 && (raw.len() + item_sz > max_raw || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
                flush(wire, &mut raw, &mut batch_count, true)?;
            }
            raw.push(0); // AppendConfirmed
            write_str(&mut raw, path);
            batch_count += 1;
        } else if entry.chunks.len() <= DIRECT_LEAF_CHUNK_LIMIT {
            let item_sz = 1 + 4 + path.len() + 4 + entry.chunks.len() * 20;
            if batch_count > 0 && (raw.len() + item_sz > max_raw || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
                flush(wire, &mut raw, &mut batch_count, true)?;
            }
            raw.push(1); // LeafChunks
            write_str(&mut raw, path);
            raw.extend_from_slice(&(entry.chunks.len() as u32).to_le_bytes());
            for c in &entry.chunks {
                raw.extend_from_slice(&c.len.to_le_bytes());
                raw.extend_from_slice(&c.id);
            }
            batch_count += 1;
        } else {
            // A huge file's super-chunk list may exceed a single batch/frame;
            // emit it in slices that the decoder reassembles in order.
            let supers = recipe::super_chunks(&entry.chunks);
            let item_hdr = 1 + 4 + path.len() + 4;
            let max_per_slice = (max_raw.saturating_sub(4 + item_hdr) / 40).max(1);
            let mut pos = 0usize;
            while pos < supers.len() {
                let remaining_space = max_raw.saturating_sub(raw.len() + item_hdr);
                let fit = remaining_space / 40;
                if batch_count > 0 && (fit == 0 || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
                    flush(wire, &mut raw, &mut batch_count, true)?;
                }
                let room = (max_raw.saturating_sub(raw.len() + item_hdr) / 40).max(1).min(max_per_slice);
                let end = (pos + room).min(supers.len());
                let slice = &supers[pos..end];
                raw.push(2); // SuperChunks (or continuation slice)
                write_str(&mut raw, path);
                raw.extend_from_slice(&(slice.len() as u32).to_le_bytes());
                for sc in slice {
                    raw.extend_from_slice(&sc.start_chunk.to_le_bytes());
                    raw.extend_from_slice(&sc.chunk_count.to_le_bytes());
                    raw.extend_from_slice(&sc.offset.to_le_bytes());
                    raw.extend_from_slice(&sc.len.to_le_bytes());
                    raw.extend_from_slice(&sc.id);
                }
                batch_count += 1;
                pos = end;
            }
            super_cache.insert(path.clone(), supers);
        }
    }
    flush(wire, &mut raw, &mut batch_count, false)
}

fn decode_chunk_info_frame(payload: &[u8], out: &mut HashMap<String, ChunkResp>) -> io::Result<bool> {
    let (more, raw) = read_compressed_batch(payload, MAX_FRAME_LEN)?;
    let mut cur = Cursor::new(&raw);
    let count = cur.read_u32()? as usize;
    if count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many chunk info items"));
    }
    for _ in 0..count {
        let kind = cur.read_u8()?;
        let path = cur.read_str(MAX_PATH_LEN)?;
        match kind {
            0 => {
                out.insert(path, ChunkResp::AppendConfirmed);
            }
            1 => {
                let n = cur.read_u32()? as usize;
                if n.saturating_mul(20) > cur.remaining() {
                    return Err(invalid_data("truncated leaf chunk list"));
                }
                let mut chunks = Vec::with_capacity(n);
                for _ in 0..n {
                    let len = cur.read_u32()?;
                    let id = cur.read_array::<16>()?;
                    chunks.push(ChunkRef { len, id });
                }
                out.insert(path, ChunkResp::LeafChunks(chunks));
            }
            2 => {
                let n = cur.read_u32()? as usize;
                if n.saturating_mul(40) > cur.remaining() {
                    return Err(invalid_data("truncated super-chunk list"));
                }
                let mut supers = Vec::with_capacity(n);
                for _ in 0..n {
                    let start_chunk = cur.read_u32()?;
                    let chunk_count = cur.read_u32()?;
                    let offset = cur.read_u64()?;
                    let len = cur.read_u64()?;
                    let id = cur.read_array::<16>()?;
                    supers.push(SuperChunk { start_chunk, chunk_count, offset, len, id });
                }
                match out.entry(path) {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if let ChunkResp::SuperChunks(existing) = e.get_mut() {
                            if existing.len().saturating_add(supers.len()) > MAX_ITEMS {
                                return Err(invalid_data("too many super-chunks for one file"));
                            }
                            existing.extend(supers);
                        } else {
                            return Err(invalid_data("conflicting chunk info for path"));
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(ChunkResp::SuperChunks(supers));
                    }
                }
            }
            _ => return Err(invalid_data("unknown chunk info kind")),
        }
    }
    cur.expect_eof()?;
    Ok(more)
}

fn send_need_leaf_chunks<S: Read + Write>(wire: &mut Wire<S>, reqs: &[(String, Vec<u32>)]) -> io::Result<()> {
    let mut raw = Vec::new();
    let mut batch_count = 0u32;
    raw.extend_from_slice(&0u32.to_le_bytes());

    let flush = |wire: &mut Wire<S>, raw: &mut Vec<u8>, batch_count: &mut u32, more: bool| -> io::Result<()> {
        raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
        write_compressed_batch(wire, TAG_NEED_LEAF_CHUNKS, more, raw)?;
        raw.clear();
        raw.extend_from_slice(&0u32.to_le_bytes());
        *batch_count = 0;
        Ok(())
    };

    for (path, indices) in reqs {
        let item_hdr = 4 + path.len() + 4;
        let max_per_slice = (MAX_MANIFEST_CHUNK_RAW.saturating_sub(4 + item_hdr) / 4).max(1);
        let mut pos = 0usize;
        while pos < indices.len() {
            let remaining_space = MAX_MANIFEST_CHUNK_RAW.saturating_sub(raw.len() + item_hdr);
            let fit = remaining_space / 4;
            if batch_count > 0 && (fit == 0 || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
                flush(wire, &mut raw, &mut batch_count, true)?;
            }
            let room = (MAX_MANIFEST_CHUNK_RAW.saturating_sub(raw.len() + item_hdr) / 4).max(1).min(max_per_slice);
            let end = (pos + room).min(indices.len());
            let slice = &indices[pos..end];
            write_str(&mut raw, path);
            raw.extend_from_slice(&(slice.len() as u32).to_le_bytes());
            for &si in slice {
                raw.extend_from_slice(&si.to_le_bytes());
            }
            batch_count += 1;
            pos = end;
        }
    }
    flush(wire, &mut raw, &mut batch_count, false)
}

fn decode_need_leaf_chunks_frame(payload: &[u8], out: &mut Vec<(String, Vec<u32>)>) -> io::Result<bool> {
    let (more, raw) = read_compressed_batch(payload, MAX_FRAME_LEN)?;
    let mut cur = Cursor::new(&raw);
    let count = cur.read_u32()? as usize;
    if count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many leaf chunk requests"));
    }
    for _ in 0..count {
        let path = cur.read_str(MAX_PATH_LEN)?;
        let n = cur.read_u32()? as usize;
        if n.saturating_mul(4) > cur.remaining() {
            return Err(invalid_data("truncated super-chunk index list"));
        }
        let mut indices = Vec::with_capacity(n);
        for _ in 0..n {
            indices.push(cur.read_u32()?);
        }
        out.push((path, indices));
    }
    cur.expect_eof()?;
    Ok(more)
}

fn send_leaf_chunks<S: Read + Write>(wire: &mut Wire<S>, recv_index: &Index, super_cache: &HashMap<String, Vec<SuperChunk>>, reqs: &[(String, Vec<u32>)]) -> io::Result<()> {
    let mut raw = Vec::new();
    let mut batch_count = 0u32;
    raw.extend_from_slice(&0u32.to_le_bytes());

    let mut flat: Vec<(&str, u32, &[ChunkRef])> = Vec::new();
    for (path, indices) in reqs {
        let Some(entry) = recv_index.entries.get(path) else {
            continue;
        };
        let Some(supers) = super_cache.get(path) else {
            continue;
        };
        for &si in indices {
            let Some(sc) = supers.get(si as usize) else {
                continue;
            };
            let start = sc.start_chunk as usize;
            let end = start.saturating_add(sc.chunk_count as usize);
            if end <= entry.chunks.len() {
                flat.push((path.as_str(), si, &entry.chunks[start..end]));
            }
        }
    }

    if flat.is_empty() {
        write_compressed_batch(wire, TAG_LEAF_CHUNKS, false, &raw)?;
        return Ok(());
    }

    for (path, si, chunks) in flat {
        let item_sz = 4 + path.len() + 4 + 4 + chunks.len() * 20;
        if batch_count > 0 && (raw.len() + item_sz > MAX_MANIFEST_CHUNK_RAW || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
            raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
            write_compressed_batch(wire, TAG_LEAF_CHUNKS, true, &raw)?;
            raw.clear();
            raw.extend_from_slice(&0u32.to_le_bytes());
            batch_count = 0;
        }
        write_str(&mut raw, path);
        raw.extend_from_slice(&si.to_le_bytes());
        raw.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for c in chunks {
            raw.extend_from_slice(&c.len.to_le_bytes());
            raw.extend_from_slice(&c.id);
        }
        batch_count += 1;
    }
    raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
    write_compressed_batch(wire, TAG_LEAF_CHUNKS, false, &raw)
}

fn decode_leaf_chunks_frame(payload: &[u8], out: &mut HashMap<String, HashMap<u32, Vec<ChunkRef>>>) -> io::Result<bool> {
    let (more, raw) = read_compressed_batch(payload, MAX_FRAME_LEN)?;
    let mut cur = Cursor::new(&raw);
    let count = cur.read_u32()? as usize;
    if count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many leaf chunk batches"));
    }
    for _ in 0..count {
        let path = cur.read_str(MAX_PATH_LEN)?;
        let si = cur.read_u32()?;
        let n = cur.read_u32()? as usize;
        if n.saturating_mul(20) > cur.remaining() {
            return Err(invalid_data("truncated leaf chunks"));
        }
        let mut chunks = Vec::with_capacity(n);
        for _ in 0..n {
            let len = cur.read_u32()?;
            let id = cur.read_array::<16>()?;
            chunks.push(ChunkRef { len, id });
        }
        out.entry(path).or_default().insert(si, chunks);
    }
    cur.expect_eof()?;
    Ok(more)
}

fn write_data_piece_frame<S: Read + Write>(
    wire: &mut Wire<S>,
    piece_buf: &mut Vec<u8>,
    basis_index: &HashMap<String, u16>,
    piece: Piece,
    data_bytes_raw: &mut u64,
    data_bytes_compressed: &mut u64,
) -> io::Result<()> {
    piece_buf.clear();
    match piece {
        Piece::Copy { from, offset, len } => {
            let bidx = basis_index[&from];
            piece_buf.push(0); // Copy
            piece_buf.extend_from_slice(&bidx.to_le_bytes());
            piece_buf.extend_from_slice(&offset.to_le_bytes());
            piece_buf.extend_from_slice(&len.to_le_bytes());
        }
        Piece::Data { raw_len, codec, bytes } => {
            *data_bytes_raw += raw_len as u64;
            *data_bytes_compressed += bytes.len() as u64;
            piece_buf.push(1); // Data
            piece_buf.extend_from_slice(&raw_len.to_le_bytes());
            match codec {
                DataCodec::Raw => piece_buf.push(0),
                DataCodec::Zstd => piece_buf.push(1),
                DataCodec::ZstdRefPrefix { from, prefix_offset, prefix_len } => {
                    let bidx = basis_index[&from];
                    piece_buf.push(2);
                    piece_buf.extend_from_slice(&bidx.to_le_bytes());
                    piece_buf.extend_from_slice(&prefix_offset.to_le_bytes());
                    piece_buf.extend_from_slice(&prefix_len.to_le_bytes());
                }
            }
            piece_buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            piece_buf.extend_from_slice(&bytes);
        }
    }
    wire.write_frame(TAG_FILE_PIECE, piece_buf)
}

enum StreamOutcome {
    Sent { hash: [u8; 32], chunks: Vec<ChunkRef> },
    Changed,
    Unread(String),
}

/// Stream one file's plan to the receiver. When `hash_at_end` is true, the
/// file was indexed by stat alone, so its BLAKE3 (and optionally chunk list for
/// the sender's cache) is computed on the fly in a single read pass and sent in
/// `TAG_FILE_END`.
#[allow(clippy::too_many_arguments)]
fn stream_file_plan<S: Read + Write>(
    wire: &mut Wire<S>,
    source: &dyn Source,
    entry: &Entry,
    kind: RecipeKind,
    ops: &[recipe::PlanOp],
    hash_at_end: bool,
    compute_chunks: bool,
    data_bytes_raw: &mut u64,
    data_bytes_compressed: &mut u64,
    on_bytes: &mut impl FnMut(u64),
) -> io::Result<StreamOutcome> {
    if !source.check_unchanged(entry) {
        return Ok(StreamOutcome::Changed);
    }

    // (Holes looked for once a file, in one of a mebibyte or more, and piece
    // by piece only in one that has some: each look opens the file again,
    // and a home is mostly small files with none.)
    let may_have_holes = if entry.size >= 1 << 20 {
        match source.extent_at(&entry.path, 0, entry.size) {
            Ok((hole, len)) => hole || len < entry.size,
            Err(e) if hash_at_end && e.kind() == io::ErrorKind::PermissionDenied => return Ok(StreamOutcome::Unread(e.to_string())),
            Err(_) => false,
        }
    } else {
        false
    };

    // Fast single-read path for lazy-hashed files that fit in one piece
    // (<= 2 MiB, no holes): read BEFORE sending TAG_FILE_START so unreadable
    // files (PermissionDenied) are caught with zero extra open() calls and
    // zero frames written.
    let preloaded_single = if hash_at_end && !may_have_holes && ops.len() <= 1 {
        let need = entry.size as usize;
        match source.read(&entry.path, 0, need) {
            Ok(b) if b.len() == need => Some(b),
            Ok(_) => return Ok(StreamOutcome::Changed),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Ok(StreamOutcome::Unread(e.to_string())),
            Err(_) => return Ok(StreamOutcome::Changed),
        }
    } else {
        if hash_at_end && entry.size < (1 << 20) {
            match source.read(&entry.path, 0, 0) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Ok(StreamOutcome::Unread(e.to_string())),
                Err(_) => return Ok(StreamOutcome::Changed),
            }
        }
        None
    };

    // Intern all receiver basis paths referenced by ops into a small table.
    let mut basis_paths: Vec<String> = Vec::new();
    let mut basis_index: HashMap<String, u16> = HashMap::new();
    let mut intern = |p: &str| -> io::Result<u16> {
        if let Some(&idx) = basis_index.get(p) {
            return Ok(idx);
        }
        let idx = u16::try_from(basis_paths.len()).map_err(|_| invalid_data("too many basis paths"))?;
        basis_paths.push(p.to_owned());
        basis_index.insert(p.to_owned(), idx);
        Ok(idx)
    };
    for op in ops {
        match op {
            recipe::PlanOp::Copy { from, .. } => {
                intern(from)?;
            }
            recipe::PlanOp::Data { ref_prefix: Some(rp), .. } => {
                intern(&rp.from)?;
            }
            recipe::PlanOp::Data { ref_prefix: None, .. } => {}
        }
    }

    // Send FileStart.
    let mut start_buf = Vec::with_capacity(entry.path.len() + 64);
    write_str(&mut start_buf, &entry.path);
    start_buf.push(match kind {
        RecipeKind::Append => 0,
        RecipeKind::Patch => 1,
        RecipeKind::New => 2,
    });
    start_buf.extend_from_slice(&entry.size.to_le_bytes());
    start_buf.extend_from_slice(&entry.mode.to_le_bytes());
    start_buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
    if hash_at_end {
        start_buf.push(0); // hash arrives in TAG_FILE_END
    } else {
        start_buf.push(1);
        start_buf.extend_from_slice(&entry.hash);
    }
    start_buf.extend_from_slice(&(basis_paths.len() as u16).to_le_bytes());
    for bp in &basis_paths {
        write_str(&mut start_buf, bp);
    }
    // (An append: the size the receiver's file has, and this file's chunks
    // from the start of its last one, so the receiver can write only the
    // tail after its own and check it by its chunks, reading back no more
    // than that last chunk: `ActiveState::Appending`.)
    let tail = match (kind, ops.first()) {
        (RecipeKind::Append, Some(recipe::PlanOp::Copy { offset: 0, len, .. })) => recipe::append_tail(&entry.chunks, *len).map(|k| (*len, &entry.chunks[k..])),
        _ => None,
    };
    match tail.filter(|(_, chunks)| chunks.len() <= MAX_TAIL_CHUNKS) {
        Some((base, chunks)) => {
            start_buf.push(1);
            start_buf.extend_from_slice(&base.to_le_bytes());
            start_buf.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
            for c in chunks {
                start_buf.extend_from_slice(&c.len.to_le_bytes());
                start_buf.extend_from_slice(&c.id);
            }
        }
        None => start_buf.push(0),
    }
    wire.write_frame(TAG_FILE_START, &start_buf)?;

    // Stream each piece onto the wire (bounded at a few MiB in memory).
    // If the source file vanishes, shrinks or changes mid-read, abort this
    // file alone with TAG_FILE_ABORT rather than failing the whole session.
    let mut piece_buf = Vec::new();
    let mut stream_digest = (kind == RecipeKind::New).then(|| if compute_chunks { StreamDigest::new() } else { StreamDigest::new_hash_only() });
    let mut source_changed = false;

    if let Some(raw) = preloaded_single {
        if let Some(sd) = stream_digest.as_mut() {
            sd.update(&raw);
        }
        if !raw.is_empty() {
            let piece = recipe::compress_piece(&raw, None)?;
            write_data_piece_frame(wire, &mut piece_buf, &basis_index, piece, data_bytes_raw, data_bytes_compressed)?;
            on_bytes(raw.len() as u64);
        }
    } else {
        'ops: for op in ops {
            match op {
                recipe::PlanOp::Copy { from, offset, len } => {
                    write_data_piece_frame(wire, &mut piece_buf, &basis_index, Piece::Copy { from: from.clone(), offset: *offset, len: *len }, data_bytes_raw, data_bytes_compressed)?;
                    on_bytes(*len);
                }
                recipe::PlanOp::Data { offset, len, ref_prefix } => {
                    if ref_prefix.is_some() || *len < SPARSE_BLOCK as u64 || !may_have_holes {
                        let raw = match source.read(&entry.path, *offset, *len as usize) {
                            Ok(b) if b.len() as u64 == *len => b,
                            _ => {
                                source_changed = true;
                                break 'ops;
                            }
                        };
                        let prefix_buf = match ref_prefix {
                            Some(rp) if rp.len > 0 => match source.read(&entry.path, rp.sender_offset, rp.len as usize) {
                                Ok(p) if p.len() == rp.len as usize => Some((rp.from.as_str(), rp.receiver_offset, p)),
                                _ => {
                                    source_changed = true;
                                    break 'ops;
                                }
                            },
                            _ => None,
                        };
                        if let Some(sd) = stream_digest.as_mut() {
                            sd.update(&raw);
                        }
                        let piece = match &prefix_buf {
                            Some((from, recv_off, p)) => recipe::compress_piece(&raw, Some((from, *recv_off, p)))?,
                            None => recipe::compress_piece(&raw, None)?,
                        };
                        write_data_piece_frame(wire, &mut piece_buf, &basis_index, piece, data_bytes_raw, data_bytes_compressed)?;
                        on_bytes(*len);
                    } else {
                        let mut cur_off = *offset;
                        let mut rem = *len;
                        while rem > 0 {
                            let (is_hole, extent_len) = match source.extent_at(&entry.path, cur_off, rem) {
                                Ok(e) => e,
                                Err(_) => {
                                    source_changed = true;
                                    break 'ops;
                                }
                            };
                            if is_hole && extent_len >= SPARSE_BLOCK as u64 {
                                let hlen = extent_len.min(rem);
                                if let Some(sd) = stream_digest.as_mut() {
                                    sd.update_zeros(hlen);
                                }
                                piece_buf.clear();
                                piece_buf.push(2); // Zero (sparse hole)
                                piece_buf.extend_from_slice(&hlen.to_le_bytes());
                                wire.write_frame(TAG_FILE_PIECE, &piece_buf)?;
                                on_bytes(hlen);
                                cur_off += hlen;
                                rem -= hlen;
                            } else {
                                let take = if is_hole {
                                    // Tiny hole (< 4 KiB): read normally with following bytes.
                                    rem.min(MAX_DATA_PIECE_LEN as u64) as usize
                                } else {
                                    extent_len.max(1).min(rem).min(MAX_DATA_PIECE_LEN as u64) as usize
                                };
                                let raw = match source.read(&entry.path, cur_off, take) {
                                    Ok(b) if b.len() == take => b,
                                    _ => {
                                        source_changed = true;
                                        break 'ops;
                                    }
                                };
                                if let Some(sd) = stream_digest.as_mut() {
                                    sd.update(&raw);
                                }
                                let piece = recipe::compress_piece(&raw, None)?;
                                write_data_piece_frame(wire, &mut piece_buf, &basis_index, piece, data_bytes_raw, data_bytes_compressed)?;
                                on_bytes(take as u64);
                                cur_off += take as u64;
                                rem -= take as u64;
                            }
                        }
                    }
                }
            }
        }
    }

    let computed = stream_digest.map(StreamDigest::finalize);
    if !source_changed
        && !hash_at_end
        && let Some(ref d) = computed
        && d.hash != entry.hash
    {
        source_changed = true;
    }
    if !source_changed
        && hash_at_end
        && let Some(ref d) = computed
        && d.size != entry.size
    {
        source_changed = true;
    }
    if !source_changed && !source.check_unchanged(entry) {
        source_changed = true;
    }

    if source_changed {
        wire.write_frame(TAG_FILE_ABORT, &[])?;
        Ok(StreamOutcome::Changed)
    } else if hash_at_end {
        let d = computed.expect("New file must compute digest");
        wire.write_frame(TAG_FILE_END, &d.hash)?;
        Ok(StreamOutcome::Sent { hash: d.hash, chunks: d.chunks })
    } else {
        wire.write_frame(TAG_FILE_END, &[])?;
        Ok(StreamOutcome::Sent { hash: entry.hash, chunks: Vec::new() })
    }
}

fn decode_file_start(payload: &[u8]) -> io::Result<FileStartMsg> {
    let mut cur = Cursor::new(payload);
    let path = cur.read_str(MAX_PATH_LEN)?;
    let kind = match cur.read_u8()? {
        0 => RecipeKind::Append,
        1 => RecipeKind::Patch,
        2 => RecipeKind::New,
        k => return Err(invalid_data(format!("unknown recipe kind {k}"))),
    };
    let size = cur.read_u64()?;
    let mode = cur.read_u32()?;
    let mtime_ns = cur.read_i64()?;
    let hash = match cur.read_u8()? {
        0 => None,
        1 => Some(cur.read_array::<32>()?),
        f => return Err(invalid_data(format!("unknown has_hash flag {f}"))),
    };
    let basis_count = cur.read_u16()? as usize;
    if basis_count > 1024 {
        return Err(invalid_data("too many basis paths"));
    }
    let mut basis_paths = Vec::with_capacity(basis_count);
    for _ in 0..basis_count {
        basis_paths.push(cur.read_str(MAX_PATH_LEN)?);
    }
    let tail = match cur.read_u8()? {
        0 => None,
        1 => {
            let base = cur.read_u64()?;
            let count = cur.read_u32()? as usize;
            if count > MAX_TAIL_CHUNKS || count.saturating_mul(20) > cur.remaining() {
                return Err(invalid_data("an append's tail chunks don't fit its frame"));
            }
            let mut chunks = Vec::with_capacity(count);
            for _ in 0..count {
                let len = cur.read_u32()?;
                chunks.push(ChunkRef { len, id: cur.read_array::<16>()? });
            }
            Some((base, chunks))
        }
        t => return Err(invalid_data(format!("unknown tail flag {t}"))),
    };
    cur.expect_eof()?;
    Ok(FileStartMsg { path, kind, size, mode, mtime_ns, hash, basis_paths, tail })
}

fn decode_file_piece(payload: &[u8], basis_paths: &[String]) -> io::Result<WirePiece> {
    let mut cur = Cursor::new(payload);
    match cur.read_u8()? {
        0 => {
            let bidx = cur.read_u16()? as usize;
            let from = basis_paths.get(bidx).ok_or_else(|| invalid_data("invalid basis index in Copy piece"))?.clone();
            let offset = cur.read_u64()?;
            let len = cur.read_u64()?;
            cur.expect_eof()?;
            Ok(WirePiece::Copy { from, offset, len })
        }
        1 => {
            let raw_len = cur.read_u32()?;
            if raw_len as usize > MAX_DATA_PIECE_LEN {
                return Err(invalid_data("piece raw_len exceeds limit"));
            }
            let codec = match cur.read_u8()? {
                0 => DataCodec::Raw,
                1 => DataCodec::Zstd,
                2 => {
                    let bidx = cur.read_u16()? as usize;
                    let from = basis_paths.get(bidx).ok_or_else(|| invalid_data("invalid basis index in ZstdRefPrefix"))?.clone();
                    let prefix_offset = cur.read_u64()?;
                    let prefix_len = cur.read_u32()?;
                    if prefix_len as usize > REF_PREFIX_MAX {
                        return Err(invalid_data("ref_prefix length exceeds limit"));
                    }
                    DataCodec::ZstdRefPrefix { from, prefix_offset, prefix_len }
                }
                c => return Err(invalid_data(format!("unknown data codec {c}"))),
            };
            let blen = cur.read_u32()? as usize;
            if blen > MAX_FRAME_LEN {
                return Err(invalid_data("piece payload exceeds frame limit"));
            }
            let bytes = cur.take(blen)?.to_vec();
            cur.expect_eof()?;
            Ok(WirePiece::Data { raw_len, codec, bytes })
        }
        2 => {
            let len = cur.read_u64()?;
            cur.expect_eof()?;
            Ok(WirePiece::Zero { len })
        }
        t => Err(invalid_data(format!("unknown piece tag {t}"))),
    }
}

fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Send `r` in one or more compressed `TAG_REPORT` frames so thousands of
/// path errors never exceed `MAX_FRAME_LEN`.
fn send_report<S: Read + Write>(wire: &mut Wire<S>, r: &Report) -> io::Result<()> {
    let mut pos = 0usize;
    let mut first = true;
    loop {
        let mut raw = Vec::with_capacity(if first { 128 } else { 64 });
        if first {
            raw.extend_from_slice(&r.same.to_le_bytes());
            raw.extend_from_slice(&r.copied.to_le_bytes());
            raw.extend_from_slice(&r.appended.to_le_bytes());
            raw.extend_from_slice(&r.patched.to_le_bytes());
            raw.extend_from_slice(&r.new.to_le_bytes());
            raw.extend_from_slice(&r.removed.to_le_bytes());
            raw.extend_from_slice(&r.left_newer.to_le_bytes());
            raw.extend_from_slice(&r.data_bytes_raw.to_le_bytes());
            raw.extend_from_slice(&r.data_bytes_compressed.to_le_bytes());
        }
        let count_off = raw.len();
        raw.extend_from_slice(&0u32.to_le_bytes());
        let mut batch_count = 0u32;
        while pos < r.errors.len() {
            let err = &r.errors[pos];
            let p = truncate_str(&err.path, MAX_PATH_LEN);
            let reason = truncate_str(&err.reason, MAX_PATH_LEN);
            let item_sz = 4 + p.len() + 4 + reason.len();
            if batch_count > 0 && (raw.len() + item_sz > MAX_MANIFEST_CHUNK_RAW || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
                break;
            }
            write_str(&mut raw, p);
            write_str(&mut raw, reason);
            batch_count += 1;
            pos += 1;
        }
        raw[count_off..count_off + 4].copy_from_slice(&batch_count.to_le_bytes());
        let more = pos < r.errors.len();
        write_compressed_batch(wire, TAG_REPORT, more, &raw)?;
        first = false;
        if !more {
            return Ok(());
        }
    }
}

fn read_report<S: Read + Write>(wire: &mut Wire<S>) -> io::Result<Report> {
    let (tag, payload) = wire.read_frame()?;
    if tag != TAG_REPORT {
        return Err(invalid_data(format!("expected Report, got tag {tag}")));
    }
    let (mut more, raw) = read_compressed_batch(&payload, MAX_MANIFEST_CHUNK_RAW + (256 << 10))?;
    let mut cur = Cursor::new(&raw);
    let same = cur.read_u64()?;
    let copied = cur.read_u64()?;
    let appended = cur.read_u64()?;
    let patched = cur.read_u64()?;
    let new = cur.read_u64()?;
    let removed = cur.read_u64()?;
    let left_newer = cur.read_u64()?;
    let data_bytes_raw = cur.read_u64()?;
    let data_bytes_compressed = cur.read_u64()?;
    let err_count = cur.read_u32()? as usize;
    if err_count > MAX_ITEMS_PER_FRAME {
        return Err(invalid_data("too many errors in report frame"));
    }
    let mut errors = Vec::with_capacity(err_count.min(1024));
    for _ in 0..err_count {
        let path = cur.read_str(MAX_PATH_LEN)?;
        let reason = cur.read_str(MAX_PATH_LEN)?;
        errors.push(PathError { path, reason });
    }
    cur.expect_eof()?;

    while more {
        let (tag, payload) = wire.read_frame()?;
        if tag != TAG_REPORT {
            return Err(invalid_data(format!("expected continuation Report, got tag {tag}")));
        }
        let (next_more, raw) = read_compressed_batch(&payload, MAX_MANIFEST_CHUNK_RAW + (256 << 10))?;
        let mut cur = Cursor::new(&raw);
        let count = cur.read_u32()? as usize;
        if count > MAX_ITEMS_PER_FRAME || errors.len().saturating_add(count) > MAX_ITEMS {
            return Err(invalid_data("too many errors in report"));
        }
        for _ in 0..count {
            let path = cur.read_str(MAX_PATH_LEN)?;
            let reason = cur.read_str(MAX_PATH_LEN)?;
            errors.push(PathError { path, reason });
        }
        cur.expect_eof()?;
        more = next_more;
    }

    Ok(Report {
        same,
        copied,
        appended,
        patched,
        new,
        removed,
        left_newer,
        bytes_sent: 0,
        bytes_received: 0,
        data_bytes_raw,
        data_bytes_compressed,
        errors,
        skipped: Skipped::default(),
        sender_confirmed: false,
    })
}

/// Maximum encoded sample-path bytes per skipped category in frame 1 of
/// `TAG_SENDER_SUMMARY` (4 categories × 128 KiB <= 512 KiB, always fitting
/// well inside `MAX_MANIFEST_CHUNK_RAW`). The exact `count` is always sent.
const MAX_SKIPPED_SAMPLE_BYTES_PER_KIND: usize = 128 << 10;

fn write_skipped_kind(buf: &mut Vec<u8>, kind: &SkippedKind) {
    buf.extend_from_slice(&kind.count.to_le_bytes());
    let mut paths: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for p in kind.paths.iter().take(MAX_SKIPPED_PATHS) {
        let p = truncate_str(p, MAX_PATH_LEN);
        let need = 4 + p.len();
        if used + need > MAX_SKIPPED_SAMPLE_BYTES_PER_KIND {
            break;
        }
        used += need;
        paths.push(p);
    }
    buf.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    for p in paths {
        write_str(buf, p);
    }
}

fn read_skipped_kind(cur: &mut Cursor<'_>) -> io::Result<SkippedKind> {
    let count = cur.read_u64()?;
    let n = cur.read_u32()? as usize;
    if n > MAX_SKIPPED_PATHS || (n as u64) > count {
        return Err(invalid_data("too many skipped paths in sender summary"));
    }
    let mut paths = Vec::with_capacity(n);
    for _ in 0..n {
        paths.push(cur.read_str(MAX_PATH_LEN)?);
    }
    Ok(SkippedKind { count, paths })
}

/// Send the sender's final `errors` and `skipped` summary to a receiver that
/// asked for `FLAG_SENDER_SUMMARY` (may span multiple frames if many errors).
fn send_sender_summary<S: Read + Write>(wire: &mut Wire<S>, errors: &[PathError], skipped: &Skipped) -> io::Result<()> {
    let mut pos = 0usize;
    let mut first = true;
    loop {
        let mut raw = Vec::with_capacity(if first { 128 } else { 64 });
        if first {
            write_skipped_kind(&mut raw, &skipped.unread);
            write_skipped_kind(&mut raw, &skipped.non_utf8);
            write_skipped_kind(&mut raw, &skipped.special);
            write_skipped_kind(&mut raw, &skipped.mounts);
        }
        let count_off = raw.len();
        raw.extend_from_slice(&0u32.to_le_bytes());
        let mut batch_count = 0u32;
        while pos < errors.len() {
            let err = &errors[pos];
            let p = truncate_str(&err.path, MAX_PATH_LEN);
            let reason = truncate_str(&err.reason, MAX_PATH_LEN);
            let item_sz = 4 + p.len() + 4 + reason.len();
            // On frame 1, allow 0 errors if skipped samples already fill the
            // chunk so errors spill cleanly to frame 2 without exceeding the cap.
            if (batch_count > 0 || first) && (raw.len() + item_sz > MAX_MANIFEST_CHUNK_RAW || batch_count as usize >= MAX_ITEMS_PER_FRAME) {
                break;
            }
            write_str(&mut raw, p);
            write_str(&mut raw, reason);
            batch_count += 1;
            pos += 1;
        }
        raw[count_off..count_off + 4].copy_from_slice(&batch_count.to_le_bytes());
        let more = pos < errors.len();
        write_compressed_batch(wire, TAG_SENDER_SUMMARY, more, &raw)?;
        first = false;
        if !more {
            return Ok(());
        }
    }
}

/// Hard bounds on cumulative `TAG_SENDER_SUMMARY` continuation frames and
/// decoded error text so a misbehaving peer cannot loop forever or exhaust RAM.
const MAX_SENDER_SUMMARY_FRAMES: usize = 64;
const MAX_SENDER_SUMMARY_BYTES: usize = 16 << 20;

fn read_sender_summary_frames<S: Read + Write>(wire: &mut Wire<S>, first_payload: &[u8]) -> io::Result<(Vec<PathError>, Skipped)> {
    // Frame 1 may carry up to 4 × 200 × 4 KiB legacy samples + errors; cap at MAX_FRAME_LEN.
    let (mut more, raw) = read_compressed_batch(first_payload, MAX_FRAME_LEN)?;
    let mut total_decoded_bytes = raw.len();
    let mut frames_seen = 1usize;
    let mut cur = Cursor::new(&raw);
    let unread = read_skipped_kind(&mut cur)?;
    let non_utf8 = read_skipped_kind(&mut cur)?;
    let special = read_skipped_kind(&mut cur)?;
    let mounts = read_skipped_kind(&mut cur)?;
    let skipped = Skipped { unread, non_utf8, special, mounts };
    let err_count = cur.read_u32()? as usize;
    if err_count > MAX_ITEMS_PER_FRAME || err_count > MAX_ITEMS {
        return Err(invalid_data("too many errors in sender summary"));
    }
    let mut errors = Vec::with_capacity(err_count.min(1024));
    for _ in 0..err_count {
        let path = cur.read_str(MAX_PATH_LEN)?;
        let reason = cur.read_str(MAX_PATH_LEN)?;
        errors.push(PathError { path, reason });
    }
    cur.expect_eof()?;

    while more {
        frames_seen += 1;
        if frames_seen > MAX_SENDER_SUMMARY_FRAMES {
            return Err(invalid_data("too many SenderSummary continuation frames"));
        }
        let (tag, payload) = wire.read_frame()?;
        if tag != TAG_SENDER_SUMMARY {
            return Err(invalid_data(format!("expected continuation SenderSummary, got tag {tag}")));
        }
        let (next_more, raw) = read_compressed_batch(&payload, MAX_MANIFEST_CHUNK_RAW + (256 << 10))?;
        total_decoded_bytes = total_decoded_bytes.saturating_add(raw.len());
        if total_decoded_bytes > MAX_SENDER_SUMMARY_BYTES {
            return Err(invalid_data("SenderSummary decoded bytes exceed limit"));
        }
        let mut cur = Cursor::new(&raw);
        let count = cur.read_u32()? as usize;
        if count == 0 {
            return Err(invalid_data("empty SenderSummary continuation frame"));
        }
        if count > MAX_ITEMS_PER_FRAME || errors.len().saturating_add(count) > MAX_ITEMS {
            return Err(invalid_data("too many errors in sender summary"));
        }
        for _ in 0..count {
            let path = cur.read_str(MAX_PATH_LEN)?;
            let reason = cur.read_str(MAX_PATH_LEN)?;
            errors.push(PathError { path, reason });
        }
        cur.expect_eof()?;
        more = next_more;
    }

    Ok((errors, skipped))
}

fn write_compressed_batch<S: Read + Write>(wire: &mut Wire<S>, tag: u8, more: bool, raw: &[u8]) -> io::Result<()> {
    let piece = recipe::compress_piece(raw, None)?;
    let Piece::Data { raw_len, codec, bytes } = piece else {
        unreachable!();
    };
    let mut payload = Vec::with_capacity(6 + bytes.len());
    payload.push(u8::from(more));
    payload.push(match codec {
        DataCodec::Raw => 0,
        DataCodec::Zstd => 1,
        DataCodec::ZstdRefPrefix { .. } => unreachable!(),
    });
    payload.extend_from_slice(&raw_len.to_le_bytes());
    payload.extend_from_slice(&bytes);
    wire.write_frame(tag, &payload)
}

fn read_compressed_batch(payload: &[u8], max_raw: usize) -> io::Result<(bool, Vec<u8>)> {
    let mut cur = Cursor::new(payload);
    let more_u8 = cur.read_u8()?;
    if more_u8 > 1 {
        return Err(invalid_data("invalid continuation flag"));
    }
    let codec_u8 = cur.read_u8()?;
    let raw_len = cur.read_u32()? as usize;
    if raw_len > max_raw {
        return Err(invalid_data("batch raw_len exceeds limit"));
    }
    let body = cur.rest();
    let raw = match codec_u8 {
        0 => {
            if body.len() != raw_len {
                return Err(invalid_data("raw batch length mismatch"));
            }
            body.to_vec()
        }
        1 => {
            let mut out = Vec::with_capacity(raw_len);
            let mut dctx = zstd::zstd_safe::DCtx::create();
            dctx.decompress(&mut out, body).map_err(|c| invalid_data(zstd::zstd_safe::get_error_name(c)))?;
            if out.len() != raw_len {
                return Err(invalid_data("decompressed batch length mismatch"));
            }
            out
        }
        _ => return Err(invalid_data("unknown batch codec")),
    };
    Ok((more_u8 == 1, raw))
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

    fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    fn expect_eof(&self) -> io::Result<()> {
        if self.pos == self.buf.len() { Ok(()) } else { Err(invalid_data("trailing bytes in frame")) }
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| invalid_data("length overflow"))?;
        if end > self.buf.len() {
            return Err(invalid_data("truncated frame"));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn read_u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
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

    fn read_str(&mut self, max_len: usize) -> io::Result<String> {
        let len = self.read_u32()? as usize;
        if len > max_len {
            return Err(invalid_data("string exceeds maximum length"));
        }
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| invalid_data("invalid UTF-8"))?;
        Ok(s.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn scratch_dir(name: &str) -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-sync-wire-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sync_folders(src: &Path, dst: &Path, src_cache: Option<PathBuf>, dst_cache: Option<PathBuf>, opts: Options) -> (Report, Report) {
        let (a, b) = UnixStream::pair().unwrap();
        let src_dir = src.to_path_buf();
        let dst_dir = dst.to_path_buf();
        let send_opts = opts.clone();
        let mut recv_opts = opts;
        recv_opts.cache_path = dst_cache;

        let t = thread::spawn(move || {
            let source = FolderSource { root: src_dir, filter: send_opts.filter.clone(), cache_path: src_cache };
            send(a, &source, send_opts).unwrap()
        });
        let recv_rep = receive(b, &dst_dir, recv_opts).unwrap();
        let send_rep = t.join().unwrap();
        assert_eq!(send_rep.bytes_sent, recv_rep.bytes_received);
        assert_eq!(send_rep.bytes_received, recv_rep.bytes_sent);
        (send_rep, recv_rep)
    }

    fn structured_records(target_bytes: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(target_bytes + 128);
        let mut i = 0u64;
        while out.len() < target_bytes {
            let mut rec = [0u8; 64];
            rec[0..4].copy_from_slice(b"REC\0");
            rec[4..12].copy_from_slice(&i.to_le_bytes());
            let hash = blake3::hash(&i.to_le_bytes());
            rec[12..28].copy_from_slice(&hash.as_bytes()[..16]);
            rec[28..64].copy_from_slice(b"structured-payload-record-olo-sync!!");
            out.extend_from_slice(&rec);
            i += 1;
        }
        out.truncate(target_bytes);
        out
    }

    fn jsonl_data(target_bytes: usize, start_turn: usize) -> (Vec<u8>, usize) {
        let mut out = Vec::with_capacity(target_bytes + 256);
        let mut i = start_turn;
        while out.len() < target_bytes {
            out.extend_from_slice(
                format!(
                    "{{\"type\":\"message\",\"role\":\"assistant\",\"session\":\"rex-9f8e7d6c\",\"turn\":{i},\"tool\":\"read_file\",\"path\":\"/home/me/src/app/src/wire.rs\",\"summary\":\"inspecting frame {i} for delta encoding\"}}\n"
                )
                .as_bytes(),
            );
            i += 1;
        }
        (out, i)
    }

    #[test]
    fn full_sync_identical_trees_and_noop_under_2kib() {
        let src = scratch_dir("full-src");
        let dst = scratch_dir("full-dst");
        let caches = scratch_dir("full-cache");

        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("root.txt"), b"hello root").unwrap();
        fs::write(src.join("a/b/deep.txt"), b"deep file content\n".repeat(200)).unwrap();
        fs::write(src.join("empty.txt"), b"").unwrap();
        symlink("root.txt", src.join("link")).unwrap();
        fs::set_permissions(src.join("root.txt"), fs::Permissions::from_mode(0o755)).unwrap();

        let (s1, r1) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert!(r1.errors.is_empty(), "{:?}", r1.errors);
        assert_eq!(s1.new, 3);
        assert_eq!(r1.new, 3);

        let src_idx = Index::scan(&src, &Filter::all(), None).unwrap();
        let dst_idx = Index::scan(&dst, &Filter::all(), None).unwrap();
        assert_eq!(src_idx.tree_hash(), dst_idx.tree_hash());
        assert_eq!(fs::read_link(dst.join("link")).unwrap().to_str().unwrap(), "root.txt");
        assert_eq!(fs::symlink_metadata(dst.join("root.txt")).unwrap().permissions().mode() & 0o777, 0o755);

        // Second session with nothing changed: under 2 KiB on the wire.
        let (s2, r2) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert!(r2.errors.is_empty());
        assert_eq!(s2.same, 3);
        assert!(s2.wire_bytes() < 2048, "expected < 2 KiB on wire for no-op sync, got {} B", s2.wire_bytes());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
        let _ = fs::remove_dir_all(&caches);
    }

    #[test]
    fn append_4kib_to_20mb_under_8kib_on_wire() {
        let src = scratch_dir("append20-src");
        let dst = scratch_dir("append20-dst");
        let caches = scratch_dir("append20-cache");

        let (mut data, next_turn) = jsonl_data(20 << 20, 0);
        fs::write(src.join("transcript.jsonl"), &data).unwrap();

        let (_, r1) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert_eq!(r1.new, 1);

        // Append ~4 KiB of new JSONL lines.
        let (tail, _) = jsonl_data(4096, next_turn);
        data.extend_from_slice(&tail);
        fs::write(src.join("transcript.jsonl"), &data).unwrap();

        let (s2, r2) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        assert_eq!(r2.appended, 1);
        assert!(s2.wire_bytes() < 8192, "expected < 8 KiB on wire for 4 KiB append to 20 MB file, got {} B", s2.wire_bytes());
        assert_eq!(fs::read(dst.join("transcript.jsonl")).unwrap(), data);

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
        let _ = fs::remove_dir_all(&caches);
    }

    #[test]
    fn one_byte_changed_in_20mb_under_70kib_on_wire() {
        let src = scratch_dir("edit20-src");
        let dst = scratch_dir("edit20-dst");
        let caches = scratch_dir("edit20-cache");

        let mut data = structured_records(20 << 20);
        fs::write(src.join("blob.bin"), &data).unwrap();

        sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());

        let mid = data.len() / 2;
        data[mid] ^= 0xFF;
        fs::write(src.join("blob.bin"), &data).unwrap();

        let (s2, r2) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        assert_eq!(r2.patched, 1);
        assert!(s2.wire_bytes() < 70 * 1024, "expected < 70 KiB on wire for 1-byte edit in 20 MB file, got {} B", s2.wire_bytes());
        assert_eq!(fs::read(dst.join("blob.bin")).unwrap(), data);

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
        let _ = fs::remove_dir_all(&caches);
    }

    #[test]
    fn rename_5mb_file_under_1kib_on_wire() {
        let src = scratch_dir("rename5-src");
        let dst = scratch_dir("rename5-dst");
        let caches = scratch_dir("rename5-cache");

        let data = structured_records(5 << 20);
        fs::write(src.join("old_name.bin"), &data).unwrap();

        let opts = Options { delete_extra: true, ..Options::default() };
        sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), opts.clone());

        fs::rename(src.join("old_name.bin"), src.join("new_name.bin")).unwrap();

        let (s2, r2) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), opts);
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        assert_eq!(r2.copied, 1);
        assert_eq!(r2.removed, 1);
        assert!(!dst.join("old_name.bin").exists());
        assert_eq!(fs::read(dst.join("new_name.bin")).unwrap(), data);
        assert!(s2.wire_bytes() < 1024, "expected < 1 KiB on wire for 5 MB rename, got {} B", s2.wire_bytes());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
        let _ = fs::remove_dir_all(&caches);
    }

    #[test]
    fn keep_newer_leaves_newer_receiver_file_untouched() {
        let src = scratch_dir("newer-src");
        let dst = scratch_dir("newer-dst");

        let old_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let new_time = UNIX_EPOCH + Duration::from_secs(1_800_000_000);

        fs::write(src.join("session.jsonl"), b"older on sender\n").unwrap();
        fs::OpenOptions::new().write(true).open(src.join("session.jsonl")).unwrap().set_modified(old_time).unwrap();

        fs::write(dst.join("session.jsonl"), b"newer on receiver\n").unwrap();
        fs::OpenOptions::new().write(true).open(dst.join("session.jsonl")).unwrap().set_modified(new_time).unwrap();

        let opts = Options { keep_newer: true, ..Options::default() };
        let (s, r) = sync_folders(&src, &dst, None, None, opts);
        assert_eq!(s.left_newer, 1);
        assert_eq!(r.left_newer, 1);
        assert_eq!(fs::read(dst.join("session.jsonl")).unwrap(), b"newer on receiver\n");

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn list_source_transforms_and_syncs_files() {
        let dst = scratch_dir("list-src-dst");
        let (a, b) = UnixStream::pair().unwrap();

        let t = thread::spawn(move || {
            let items = vec![ListEntry::new("notes/a.txt", 0o644, 1_700_000_000_000_000_000), ListEntry::new("notes/b.txt", 0o600, 1_700_000_000_000_000_000)];
            let source = ListSource::new(items, |path| Ok(format!("transformed:{path}").into_bytes()));
            send(a, &source, Options::default()).unwrap()
        });
        let r = receive(b, &dst, Options::default()).unwrap();
        let s = t.join().unwrap();
        assert!(r.errors.is_empty());
        assert_eq!(s.new, 2);
        assert_eq!(fs::read_to_string(dst.join("notes/a.txt")).unwrap(), "transformed:notes/a.txt");
        assert_eq!(fs::read_to_string(dst.join("notes/b.txt")).unwrap(), "transformed:notes/b.txt");

        let _ = fs::remove_dir_all(&dst);
    }

    struct RawMaliciousSource {
        path: String,
        bytes: Vec<u8>,
    }

    impl Source for RawMaliciousSource {
        fn index(&self) -> io::Result<Index> {
            let d = chunk::hash_and_chunk(&self.bytes[..])?;
            let mut entries = BTreeMap::new();
            entries.insert(
                self.path.clone(),
                Entry { path: self.path.clone(), kind: EntryKind::File, size: d.size, mtime_ns: 1_700_000_000_000_000_000, ctime_ns: 0, ino: 0, mode: 0o644, hash: d.hash, chunks: d.chunks },
            );
            Ok(Index { entries, ..Index::default() })
        }

        fn read(&self, _path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let s = offset as usize;
            Ok(self.bytes[s..s + len].to_vec())
        }
    }

    #[test]
    fn dotdot_and_symlink_escape_paths_refused_and_reported() {
        let base = scratch_dir("escape-parent");
        let dst = base.join("root");
        fs::create_dir_all(&dst).unwrap();

        // 1. Path with `..` must be refused and reported, writing nothing outside root.
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let src = RawMaliciousSource { path: "../escaped.txt".into(), bytes: b"should never be written".to_vec() };
            send(a, &src, Options::default()).unwrap()
        });
        let r = receive(b, &dst, Options::default()).unwrap();
        let s = t.join().unwrap();
        assert!(!base.join("escaped.txt").exists());
        assert_eq!(r.errors.len(), 1);
        assert_eq!(s.errors.len(), 1);
        assert_eq!(r.errors[0].path, "../escaped.txt");

        // 2. Path traversing an existing symlink must also be refused.
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, dst.join("symdir")).unwrap();

        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let src = RawMaliciousSource { path: "symdir/pwn.txt".into(), bytes: b"should never traverse symlink".to_vec() };
            send(a, &src, Options::default()).unwrap()
        });
        let r2 = receive(b, &dst, Options::default()).unwrap();
        let _ = t.join().unwrap();
        assert!(!outside.join("pwn.txt").exists());
        assert_eq!(r2.errors.len(), 1);
        assert_eq!(r2.errors[0].path, "symdir/pwn.txt");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn damaged_and_oversized_frames_return_error() {
        let dst = scratch_dir("damaged-frame");

        // Oversized frame length (> MAX_FRAME_LEN) rejected immediately without allocating.
        let (mut a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut hdr = [0u8; 5];
            let _ = a.read_exact(&mut hdr); // read Hello header
            let len = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
            let mut rest = vec![0u8; len];
            let _ = a.read_exact(&mut rest);
            // Send a huge bogus length header (1 GiB).
            let huge: u32 = 1 << 30;
            let _ = a.write_all(&huge.to_le_bytes());
            let _ = a.write_all(&[TAG_HELLO_ACK]);
            let _ = a.flush();
        });
        let err = receive(b, &dst, Options::default()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        t.join().unwrap();

        // Wrong protocol version rejected.
        let (mut a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut hdr = [0u8; 5];
            let _ = a.read_exact(&mut hdr);
            let len = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
            let mut rest = vec![0u8; len];
            let _ = a.read_exact(&mut rest);
            let mut bad_ack = Vec::new();
            bad_ack.extend_from_slice(&5u32.to_le_bytes());
            bad_ack.push(TAG_HELLO_ACK);
            bad_ack.extend_from_slice(&999u32.to_le_bytes());
            bad_ack.push(0);
            let _ = a.write_all(&bad_ack);
            let _ = a.flush();
        });
        let err = receive(b, &dst, Options::default()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        t.join().unwrap();

        // Truncated frame returns UnexpectedEof.
        let (mut a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut hdr = [0u8; 5];
            let _ = a.read_exact(&mut hdr);
            let len = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
            let mut rest = vec![0u8; len];
            let _ = a.read_exact(&mut rest);
            let _ = a.write_all(&100u32.to_le_bytes());
            let _ = a.write_all(&[TAG_HELLO_ACK, 1, 2, 3]);
            drop(a);
        });
        let err = receive(b, &dst, Options::default()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        t.join().unwrap();

        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn grown_file_with_modified_prefix_falls_back_to_patch_and_absolute_path_refused() {
        let src = scratch_dir("grow-mod-src");
        let dst = scratch_dir("grow-mod-dst");
        let caches = scratch_dir("grow-mod-cache");

        let (mut data, next_turn) = jsonl_data(256 << 10, 0);
        fs::write(src.join("log.jsonl"), &data).unwrap();
        sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());

        // Mutate a byte near the beginning AND append at the end: append probe
        // must reject and fall back cleanly to Patch.
        data[100] ^= 0xFF;
        let (tail, _) = jsonl_data(4096, next_turn);
        data.extend_from_slice(&tail);
        fs::write(src.join("log.jsonl"), &data).unwrap();

        let (_, r2) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert!(r2.errors.is_empty());
        assert_eq!(r2.patched, 1);
        assert_eq!(r2.appended, 0);
        assert_eq!(fs::read(dst.join("log.jsonl")).unwrap(), data);

        // Extra receiver file stays by default (delete_extra = false).
        fs::write(dst.join("keep_me.txt"), b"local only").unwrap();
        let (_, r3) = sync_folders(&src, &dst, Some(caches.join("src.idx")), Some(caches.join("dst.idx")), Options::default());
        assert_eq!(r3.removed, 0);
        assert!(dst.join("keep_me.txt").exists());

        // Absolute path from a malicious peer is refused and reported.
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let src = RawMaliciousSource { path: "/var/tmp/olo-rex-sync-should-not-exist".into(), bytes: b"nope".to_vec() };
            send(a, &src, Options::default()).unwrap()
        });
        let r4 = receive(b, &dst, Options::default()).unwrap();
        let _ = t.join().unwrap();
        assert_eq!(r4.errors.len(), 1);
        assert!(!Path::new("/var/tmp/olo-rex-sync-should-not-exist").exists());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
        let _ = fs::remove_dir_all(&caches);
    }

    /// A sender's file where the receiver has a folder: the folder is
    /// taken away only by a mirror (`delete_extra`), never by an update;
    /// a file is replaced by one, as an update is.
    #[test]
    fn a_folder_is_taken_away_only_by_a_mirror() {
        let (src, dst) = (scratch_dir("kind-src"), scratch_dir("kind-dst"));
        fs::write(src.join("was-a-folder"), b"now a file").unwrap();
        fs::create_dir_all(dst.join("was-a-folder/inside")).unwrap();
        fs::write(dst.join("was-a-folder/inside/kept.txt"), b"the receiver's own").unwrap();
        let (_, kept) = sync_folders(&src, &dst, None, None, Options::default());
        assert_eq!(fs::read(dst.join("was-a-folder/inside/kept.txt")).unwrap(), b"the receiver's own");
        assert!(kept.errors.iter().any(|e| e.path == "was-a-folder" && e.reason.contains("a folder is there")), "{:?}", kept.errors);
        let (_, mirrored) = sync_folders(&src, &dst, None, None, Options { delete_extra: true, ..Options::default() });
        assert!(mirrored.errors.is_empty(), "{:?}", mirrored.errors);
        assert_eq!(fs::read(dst.join("was-a-folder")).unwrap(), b"now a file");
        for d in [src, dst] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// A folder read only where it comes from (Go's module cache keeps its
    /// so) stays writable while files go into it, then ends with the sender's
    /// 0555 mode; and the two are in step after, in one round trip.
    #[test]
    fn a_read_only_folder_takes_what_is_sent_into_it() {
        let (src, dst, caches) = (scratch_dir("ro-src"), scratch_dir("ro-dst"), scratch_dir("ro-cache"));
        fs::create_dir_all(src.join("module")).unwrap();
        fs::write(src.join("module/go.mod"), b"module example.com/m\n").unwrap();
        fs::set_permissions(src.join("module"), fs::Permissions::from_mode(0o555)).unwrap();
        let (_, first) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), Options::default());
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert_eq!(fs::read(dst.join("module/go.mod")).unwrap(), b"module example.com/m\n");
        assert_eq!(fs::metadata(dst.join("module")).unwrap().permissions().mode() & 0o777, 0o555);
        let (again, _) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), Options::default());
        assert!(again.wire_bytes() < 128, "{} bytes", again.wire_bytes());
        fs::set_permissions(src.join("module"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(dst.join("module"), fs::Permissions::from_mode(0o755)).unwrap();
        for d in [src, dst, caches] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// A file the sender can't read isn't sent, and a mirror doesn't take
    /// the receiver's copy of it away: what it holds isn't known.
    #[test]
    fn what_the_sender_cant_read_is_left_alone() {
        let (src, dst) = (scratch_dir("unread-src"), scratch_dir("unread-dst"));
        fs::write(src.join("secret"), b"not for this user").unwrap();
        fs::write(src.join("plain"), b"plain").unwrap();
        fs::set_permissions(src.join("secret"), fs::Permissions::from_mode(0o000)).unwrap();
        fs::write(dst.join("secret"), b"the receiver's copy").unwrap();
        let readable = fs::read(src.join("secret")).is_ok();
        let (_, report) = sync_folders(&src, &dst, None, None, Options { delete_extra: true, ..Options::default() });
        if !readable {
            assert_eq!(fs::read(dst.join("secret")).unwrap(), b"the receiver's copy");
            assert_eq!(report.removed, 0);
        }
        assert_eq!(fs::read(dst.join("plain")).unwrap(), b"plain");
        fs::set_permissions(src.join("secret"), fs::Permissions::from_mode(0o600)).unwrap();
        for d in [src, dst] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// A link left at a file's part name (a crash, or anyone) isn't written
    /// through: the file is made new beside its place, and what the link
    /// named is untouched.
    #[test]
    fn a_link_at_a_part_name_isnt_written_through() {
        let (src, dst, elsewhere) = (scratch_dir("part-src"), scratch_dir("part-dst"), scratch_dir("part-elsewhere"));
        fs::write(src.join("file.txt"), b"what was sent").unwrap();
        fs::write(elsewhere.join("victim"), b"untouched").unwrap();
        symlink(elsewhere.join("victim"), part_path_for(&dst, "file.txt")).unwrap();
        let (_, report) = sync_folders(&src, &dst, None, None, Options::default());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(fs::read(dst.join("file.txt")).unwrap(), b"what was sent");
        assert_eq!(fs::read(elsewhere.join("victim")).unwrap(), b"untouched");
        for d in [src, dst, elsewhere] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// A tree read as it changes: a file gone between its folder's listing
    /// and its reading is left out, not the whole scan failed.
    #[test]
    fn a_file_gone_while_the_tree_is_read_is_left_out() {
        let src = scratch_dir("gone-src");
        fs::write(src.join("a"), b"a").unwrap();
        fs::write(src.join("b"), b"b").unwrap();
        let index = Index::scan_with(&src, &Filter::all(), None, |p| {
            if p.ends_with("a") {
                return Err(io::Error::new(io::ErrorKind::NotFound, "gone"));
            }
            fs::File::open(p)
        })
        .unwrap();
        assert!(!index.entries.contains_key("a") && index.entries.contains_key("b") && index.unread.is_empty());
        let unreadable = Index::scan_with(&src, &Filter::all(), None, |p| {
            if p.ends_with("a") {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "no"));
            }
            fs::File::open(p)
        })
        .unwrap();
        assert!(unreadable.unread.contains_key("a") && unreadable.entries.contains_key("b"));
        assert!(Index::scan(&src.join("not-there"), &Filter::all(), None).is_err());
        fs::remove_dir_all(src).unwrap();
    }

    /// A file that only grew is appended to where it is: the same file
    /// (its inode), its tail checked by its chunks, and the receiver's
    /// index right after, so the next session is in step in one round
    /// trip. One changed since the receiver read it is built beside its
    /// place instead.
    #[test]
    fn an_append_is_written_in_place() {
        use std::os::unix::fs::MetadataExt;
        let (src, dst, caches) = (scratch_dir("inplace-src"), scratch_dir("inplace-dst"), scratch_dir("inplace-cache"));
        let (mut log, next) = jsonl_data(3 << 20, 0);
        fs::write(src.join("session.jsonl"), &log).unwrap();
        let opts = || Options::default();
        let (s, r) = (Some(caches.join("s")), Some(caches.join("r")));
        sync_folders(&src, &dst, s.clone(), r.clone(), opts());
        let ino = fs::metadata(dst.join("session.jsonl")).unwrap().ino();
        let (more, _) = jsonl_data(5000, next);
        log.extend_from_slice(&more);
        fs::write(src.join("session.jsonl"), &log).unwrap();
        let (sent, got) = sync_folders(&src, &dst, s.clone(), r.clone(), opts());
        assert_eq!((got.appended, got.errors.len()), (1, 0), "{:?}", got.errors);
        assert_eq!(fs::read(dst.join("session.jsonl")).unwrap(), log);
        assert_eq!(fs::metadata(dst.join("session.jsonl")).unwrap().ino(), ino, "appended where it is");
        assert!(sent.wire_bytes() < 4096, "{} bytes", sent.wire_bytes());
        let (again, _) = sync_folders(&src, &dst, s.clone(), r.clone(), opts());
        assert!(again.wire_bytes() < 128, "in step after: {} bytes", again.wire_bytes());
        // Changed there since (written over with the same bytes, a new file):
        // not what its index says, so built beside its place.
        let there = fs::read(dst.join("session.jsonl")).unwrap();
        fs::remove_file(dst.join("session.jsonl")).unwrap();
        fs::write(dst.join("session.jsonl"), &there).unwrap();
        let moved = fs::metadata(dst.join("session.jsonl")).unwrap().ino();
        let (more, _) = jsonl_data(5000, next + 100);
        log.extend_from_slice(&more);
        fs::write(src.join("session.jsonl"), &log).unwrap();
        let (_, got) = sync_folders(&src, &dst, s, r, opts());
        assert!(got.errors.is_empty(), "{:?}", got.errors);
        assert_eq!(fs::read(dst.join("session.jsonl")).unwrap(), log);
        let _ = moved;
        for d in [src, dst, caches] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// A tail that isn't what the sender said of it is cut off again: the
    /// file is as it was, and the error said.
    #[test]
    fn an_append_not_the_senders_is_cut_back() {
        let root = scratch_dir("inplace-bad");
        let (old, _) = jsonl_data(200 << 10, 0);
        fs::write(root.join("log"), &old).unwrap();
        let index = Index::scan(&root, &Filter::all(), None).unwrap();
        let wrong = vec![ChunkRef { len: 1, id: [7; 16] }];
        let state = append_in_place(&root, "log", old.len() as u64, &wrong, &index).expect("appended in place");
        let mut af = ActiveFile {
            replace_dirs: false,
            path: "log".into(),
            kind: RecipeKind::Append,
            expected_size: old.len() as u64 + 5,
            mode: 0o644,
            mtime_ns: 0,
            expected_hash: Some([0; 32]),
            basis_paths: vec!["log".into()],
            state,
        };
        let mut buf = vec![0u8; 64 << 10];
        apply_active_piece(&root, &mut af, &WirePiece::Copy { from: "log".into(), offset: 0, len: old.len() as u64 }, &mut buf);
        apply_active_piece(&root, &mut af, &WirePiece::Data { raw_len: 5, codec: DataCodec::Raw, bytes: b"extra".to_vec() }, &mut buf);
        let (mut index, mut report) = (index, Report::default());
        finish_file(&root, af, &[], &mut index, &mut report);
        assert!(report.errors.iter().any(|e| e.reason.contains("aren't the sender's")), "{:?}", report.errors);
        assert_eq!(fs::read(root.join("log")).unwrap(), old);
        fs::remove_dir_all(root).unwrap();
    }

    /// A scoped session: the receiver looks only at what the sender names
    /// (its root a whole home, say), takes nothing away even asked to,
    /// and is in step after with a small manifest; an append still goes
    /// in place.
    #[test]
    fn a_scoped_receiver_looks_at_what_it_is_named_alone() {
        use std::os::unix::fs::MetadataExt;
        let (src, home, caches) = (scratch_dir("scoped-src"), scratch_dir("scoped-home"), scratch_dir("scoped-cache"));
        fs::create_dir_all(src.join(".claude/projects/p")).unwrap();
        let (mut log, next) = jsonl_data(1 << 20, 0);
        fs::write(src.join(".claude/projects/p/s.jsonl"), &log).unwrap();
        fs::create_dir_all(home.join("own/deep")).unwrap();
        fs::write(home.join("own/deep/kept"), b"the host's own").unwrap();
        fs::write(home.join("unrelated"), b"also").unwrap();
        let scoped = || Options { scoped: true, delete_extra: true, ..Options::default() };
        let (_, got) = sync_folders(&src, &home, Some(caches.join("s")), Some(caches.join("r")), scoped());
        assert!(got.errors.is_empty() && got.removed == 0, "{got:?}");
        assert_eq!(fs::read(home.join(".claude/projects/p/s.jsonl")).unwrap(), log);
        assert_eq!(fs::read(home.join("own/deep/kept")).unwrap(), b"the host's own");
        assert!(home.join("unrelated").exists());
        // Nothing changed: the manifest of what was named, no more.
        let (again, _) = sync_folders(&src, &home, Some(caches.join("s")), Some(caches.join("r")), scoped());
        assert!(again.wire_bytes() < 1024, "{} bytes", again.wire_bytes());
        // A turn added: appended in place, a few KiB.
        let ino = fs::metadata(home.join(".claude/projects/p/s.jsonl")).unwrap().ino();
        let (more, _) = jsonl_data(3000, next);
        log.extend_from_slice(&more);
        fs::write(src.join(".claude/projects/p/s.jsonl"), &log).unwrap();
        let (sent, got) = sync_folders(&src, &home, Some(caches.join("s")), Some(caches.join("r")), scoped());
        assert_eq!((got.appended, got.errors.len()), (1, 0), "{:?}", got.errors);
        assert_eq!(fs::read(home.join(".claude/projects/p/s.jsonl")).unwrap(), log);
        assert_eq!(fs::metadata(home.join(".claude/projects/p/s.jsonl")).unwrap().ino(), ino);
        assert!(sent.wire_bytes() < 4096, "{} bytes", sent.wire_bytes());
        for d in [src, home, caches] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// What a scoped receiver indexes: the named files alone, not what is
    /// missing, nor what is reached through a link.
    #[test]
    fn a_scoped_index_is_of_the_named_files() {
        let root = scratch_dir("scoped-index");
        let elsewhere = scratch_dir("scoped-elsewhere");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/one"), b"1").unwrap();
        fs::write(root.join("a/two"), b"2").unwrap();
        fs::write(elsewhere.join("three"), b"3").unwrap();
        symlink(&elsewhere, root.join("link")).unwrap();
        let named = ["a/one", "a/missing", "link/three", "../x"].map(String::from);
        let index = Index::scan_paths(&root, &named, None).unwrap();
        assert_eq!(index.entries.keys().collect::<Vec<_>>(), ["a/one"]);
        // Naming folders and symlinks directly records them as scan does.
        let named_all = ["a", "a/one", "link"].map(String::from);
        let index_all = Index::scan_paths(&root, &named_all, None).unwrap();
        assert_eq!(index_all.entries["a"].kind, EntryKind::Dir);
        assert_eq!(index_all.entries["a/one"].kind, EntryKind::File);
        assert!(matches!(index_all.entries["link"].kind, EntryKind::Symlink(_)));
        for d in [root, elsewhere] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// Item 1: a scoped second pass over an unchanged tree with nested folders
    /// and symlinks sends no mkdir or symlink frames and leaves every folder's
    /// mode/mtime and every symlink's inode untouched; a folder or symlink that
    /// did change is still updated.
    #[test]
    fn scoped_second_pass_skips_unchanged_dirs_and_symlinks() {
        use std::os::unix::fs::MetadataExt as _;
        use std::sync::{Arc, Mutex as StdMutex};

        /// Transport wrapper that parses outgoing frame tags from the sender.
        struct TagRecorder {
            inner: UnixStream,
            tags: Arc<StdMutex<Vec<u8>>>,
            buf: Vec<u8>,
            skip: usize,
        }

        impl Read for TagRecorder {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.inner.read(buf)
            }
        }

        impl Write for TagRecorder {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.buf.extend_from_slice(data);
                let mut pos = 0usize;
                loop {
                    if self.skip > 0 {
                        let take = self.skip.min(self.buf.len() - pos);
                        pos += take;
                        self.skip -= take;
                        if self.skip > 0 {
                            break;
                        }
                    }
                    if self.buf.len() - pos < 5 {
                        break;
                    }
                    let len = u32::from_le_bytes(self.buf[pos..pos + 4].try_into().unwrap()) as usize;
                    let tag = self.buf[pos + 4];
                    self.tags.lock().unwrap().push(tag);
                    pos += 5;
                    self.skip = len;
                }
                if pos > 0 {
                    self.buf.drain(..pos);
                }
                self.inner.write(data)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.inner.flush()
            }
        }

        let (src, dst, caches) = (scratch_dir("scoped-ds-src"), scratch_dir("scoped-ds-dst"), scratch_dir("scoped-ds-cache"));
        fs::create_dir_all(src.join("alpha/beta")).unwrap();
        fs::create_dir_all(src.join("empty_dir")).unwrap();
        fs::write(src.join("alpha/beta/f.txt"), b"hello").unwrap();
        symlink("alpha/beta/f.txt", src.join("sym_file")).unwrap();
        symlink("alpha", src.join("sym_dir")).unwrap();
        let past = UNIX_EPOCH + Duration::from_secs(1_700_001_000);
        fs::File::open(src.join("alpha/beta")).unwrap().set_modified(past).unwrap();
        fs::File::open(src.join("alpha")).unwrap().set_modified(past).unwrap();
        fs::File::open(src.join("empty_dir")).unwrap().set_modified(past).unwrap();
        fs::set_permissions(src.join("alpha/beta"), fs::Permissions::from_mode(0o750)).unwrap();
        fs::set_permissions(src.join("empty_dir"), fs::Permissions::from_mode(0o700)).unwrap();

        let run = |tags: Arc<StdMutex<Vec<u8>>>| {
            let (a, b) = UnixStream::pair().unwrap();
            let rec = TagRecorder { inner: a, tags, buf: Vec::new(), skip: 0 };
            let src_dir = src.clone();
            let sc = caches.join("s");
            let dc = caches.join("r");
            let t = thread::spawn(move || {
                let source = FolderSource { root: src_dir, filter: Filter::default(), cache_path: Some(sc) };
                send(rec, &source, Options { scoped: true, ..Options::default() }).unwrap()
            });
            let r = receive(b, &dst, Options { scoped: true, cache_path: Some(dc), ..Options::default() }).unwrap();
            let s = t.join().unwrap();
            (s, r)
        };

        // Pass 1: everything created.
        let tags1 = Arc::new(StdMutex::new(Vec::new()));
        let (_, r1) = run(tags1.clone());
        assert!(r1.errors.is_empty(), "{:?}", r1.errors);
        assert_eq!(r1.new, 1);
        assert!(tags1.lock().unwrap().contains(&TAG_MKDIR));
        assert!(tags1.lock().unwrap().contains(&TAG_SYMLINK));

        // Record destination directory mtimes/modes and symlink inodes.
        let dir_snap = |p: &str| {
            let m = fs::symlink_metadata(dst.join(p)).unwrap();
            (m.permissions().mode() & 0o7777, m.mtime(), m.mtime_nsec())
        };
        let sym_ino = |p: &str| fs::symlink_metadata(dst.join(p)).unwrap().ino();
        let before_alpha = dir_snap("alpha");
        let before_beta = dir_snap("alpha/beta");
        let before_empty = dir_snap("empty_dir");
        let before_sym_file = sym_ino("sym_file");
        let before_sym_dir = sym_ino("sym_dir");
        assert_eq!(before_beta.0, 0o750);
        assert_eq!(before_empty.0, 0o700);

        // Pass 2: unchanged -> zero TAG_MKDIR and zero TAG_SYMLINK, mtimes and inodes preserved.
        let tags2 = Arc::new(StdMutex::new(Vec::new()));
        let (_, r2) = run(tags2.clone());
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        assert_eq!(r2.same, 1);
        let t2 = tags2.lock().unwrap().clone();
        assert!(!t2.contains(&TAG_MKDIR), "unexpected TAG_MKDIR on unchanged scoped pass: {t2:?}");
        assert!(!t2.contains(&TAG_SYMLINK), "unexpected TAG_SYMLINK on unchanged scoped pass: {t2:?}");
        assert_eq!(dir_snap("alpha"), before_alpha);
        assert_eq!(dir_snap("alpha/beta"), before_beta);
        assert_eq!(dir_snap("empty_dir"), before_empty);
        assert_eq!(sym_ino("sym_file"), before_sym_file);
        assert_eq!(sym_ino("sym_dir"), before_sym_dir);

        // Pass 3: change one folder mode and one symlink target; only those change.
        fs::set_permissions(src.join("empty_dir"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(src.join("sym_dir")).unwrap();
        symlink("alpha/beta", src.join("sym_dir")).unwrap();

        let tags3 = Arc::new(StdMutex::new(Vec::new()));
        let (_, r3) = run(tags3.clone());
        assert!(r3.errors.is_empty(), "{:?}", r3.errors);
        let t3 = tags3.lock().unwrap().clone();
        assert_eq!(t3.iter().filter(|&&t| t == TAG_MKDIR).count(), 1);
        assert_eq!(t3.iter().filter(|&&t| t == TAG_SYMLINK).count(), 1);
        assert_eq!(dir_snap("empty_dir").0, 0o755);
        assert_eq!(fs::read_link(dst.join("sym_dir")).unwrap().to_str().unwrap(), "alpha/beta");
        // Unchanged dir and symlink still untouched.
        assert_eq!(dir_snap("alpha/beta"), before_beta);
        assert_eq!(sym_ino("sym_file"), before_sym_file);

        for d in [src, dst, caches] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 2: progress observer receives phases in order (`Indexing -> Planned
    /// -> Sending -> Done`), its last `Sending` matches the plan's totals, and
    /// a second pass with one changed file plans only that one file.
    #[test]
    fn progress_reports_phases_in_order_and_plan_totals() {
        let (src, dst, caches) = (scratch_dir("prog-src"), scratch_dir("prog-dst"), scratch_dir("prog-cache"));
        fs::create_dir_all(src.join("sub")).unwrap();
        let files = [("a.txt", b"alpha payload".as_slice()), ("b.txt", b"beta payload 1234".as_slice()), ("sub/c.txt", b"gamma".as_slice()), ("sub/d.bin", &[9u8; 4096])];
        for (p, data) in &files {
            fs::write(src.join(p), data).unwrap();
        }
        let total_bytes: u64 = files.iter().map(|(_, d)| d.len() as u64).sum();

        let phase_rank = |p: Phase| match p {
            Phase::Indexing => 0u8,
            Phase::Planned => 1,
            Phase::Sending => 2,
            Phase::Done => 3,
        };

        let check_order = |events: &[Progress]| {
            assert!(!events.is_empty());
            for w in events.windows(2) {
                assert!(phase_rank(w[0].phase) <= phase_rank(w[1].phase), "phases went backwards: {:?} -> {:?}", w[0].phase, w[1].phase);
            }
            for wanted in [Phase::Indexing, Phase::Planned, Phase::Sending, Phase::Done] {
                assert!(events.iter().any(|e| e.phase == wanted), "missing phase {wanted:?} in {events:?}");
            }
        };

        // Pass 1: all 4 files planned and sent.
        let events1: Arc<Mutex<Vec<Progress>>> = Arc::new(Mutex::new(Vec::new()));
        let cb1: ProgressCallback = {
            let ev = events1.clone();
            Arc::new(move |p: &Progress| ev.lock().unwrap().push(p.clone()))
        };
        let opts1 = Options { progress: Some(cb1), ..Options::default() };
        // Ensure Options keeps Debug and Clone with a progress observer set.
        let _dbg = format!("{opts1:?}");
        let _cloned = opts1.clone();
        let (_, r1) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), opts1);
        assert!(r1.errors.is_empty(), "{:?}", r1.errors);

        let ev1 = events1.lock().unwrap().clone();
        check_order(&ev1);
        let planned1 = ev1.iter().find(|e| e.phase == Phase::Planned).unwrap();
        assert_eq!((planned1.files, planned1.bytes, planned1.files_total, planned1.bytes_total), (0, 0, 4, total_bytes));
        let last_sending1 = ev1.iter().rev().find(|e| e.phase == Phase::Sending).unwrap();
        assert_eq!((last_sending1.files, last_sending1.bytes, last_sending1.files_total, last_sending1.bytes_total), (4, total_bytes, 4, total_bytes));
        let done1 = ev1.last().unwrap();
        assert_eq!(done1.phase, Phase::Done);
        assert_eq!((done1.files, done1.bytes, done1.files_total, done1.bytes_total), (4, total_bytes, 4, total_bytes));

        // Pass 2: change 1 file -> plan has 1 file and its bytes (scoped too).
        let updated = b"beta payload updated with more content!";
        fs::write(src.join("b.txt"), updated).unwrap();
        let events2: Arc<Mutex<Vec<Progress>>> = Arc::new(Mutex::new(Vec::new()));
        let cb2: ProgressCallback = {
            let ev = events2.clone();
            Arc::new(move |p: &Progress| ev.lock().unwrap().push(p.clone()))
        };
        let (_, r2) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), Options { scoped: true, progress: Some(cb2), ..Options::default() });
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);

        let ev2 = events2.lock().unwrap().clone();
        check_order(&ev2);
        let planned2 = ev2.iter().find(|e| e.phase == Phase::Planned).unwrap();
        assert_eq!((planned2.files_total, planned2.bytes_total), (1, updated.len() as u64));
        let last_sending2 = ev2.iter().rev().find(|e| e.phase == Phase::Sending).unwrap();
        assert_eq!((last_sending2.files, last_sending2.bytes, last_sending2.files_total, last_sending2.bytes_total), (1, updated.len() as u64, 1, updated.len() as u64));

        // Pass 3: unchanged fast path -> plan has 0 files.
        let events3: Arc<Mutex<Vec<Progress>>> = Arc::new(Mutex::new(Vec::new()));
        let cb3: ProgressCallback = {
            let ev = events3.clone();
            Arc::new(move |p: &Progress| ev.lock().unwrap().push(p.clone()))
        };
        let (_, r3) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), Options { progress: Some(cb3), ..Options::default() });
        assert!(r3.errors.is_empty(), "{:?}", r3.errors);
        let ev3 = events3.lock().unwrap().clone();
        check_order(&ev3);
        let planned3 = ev3.iter().find(|e| e.phase == Phase::Planned).unwrap();
        assert_eq!((planned3.files_total, planned3.bytes_total), (0, 0));

        for d in [src, dst, caches] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 5: a receive session cut midway checkpoints its index cache (both
    /// periodically while receiving and when the connection drops), so a second
    /// session finishes without re-reading or re-hashing files from the first.
    #[test]
    fn receiver_checkpoints_index_on_interrupted_session() {
        /// Transport wrapper on the sender side that shuts down the stream
        /// cleanly after forwarding `cut_after` `TAG_FILE_END` frames.
        struct CuttingTransport {
            inner: UnixStream,
            buf: Vec<u8>,
            skip: usize,
            in_file_end: bool,
            file_ends: usize,
            cut_after: usize,
            cut: bool,
        }

        impl Read for CuttingTransport {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.cut {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "cut"));
                }
                self.inner.read(buf)
            }
        }

        impl Write for CuttingTransport {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                if self.cut {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "cut"));
                }
                self.buf.extend_from_slice(data);
                let mut pos = 0usize;
                let mut cut_at = None;
                loop {
                    if self.skip > 0 {
                        let take = self.skip.min(self.buf.len() - pos);
                        pos += take;
                        self.skip -= take;
                        if self.skip > 0 {
                            break;
                        }
                        if self.in_file_end {
                            self.in_file_end = false;
                            self.file_ends += 1;
                            if self.file_ends >= self.cut_after {
                                cut_at = Some(pos);
                                break;
                            }
                        }
                    }
                    if self.buf.len() - pos < 5 {
                        break;
                    }
                    let len = u32::from_le_bytes(self.buf[pos..pos + 4].try_into().unwrap()) as usize;
                    let tag = self.buf[pos + 4];
                    if self.buf.len() - pos < 5 + len {
                        let header_end = pos + 5;
                        let body_avail = self.buf.len() - header_end;
                        self.skip = len - body_avail;
                        self.in_file_end = tag == TAG_FILE_END;
                        break;
                    }
                    pos += 5 + len;
                    if tag == TAG_FILE_END {
                        self.file_ends += 1;
                        if self.file_ends >= self.cut_after {
                            cut_at = Some(pos);
                            break;
                        }
                    }
                }
                if let Some(end) = cut_at {
                    self.inner.write_all(&self.buf[..end])?;
                    self.inner.flush()?;
                    let _ = self.inner.shutdown(std::net::Shutdown::Both);
                    self.buf.clear();
                    self.cut = true;
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "cut"));
                }
                let to_send = self.buf.split_off(0);
                self.inner.write_all(&to_send)?;
                Ok(data.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                if self.cut {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "cut"));
                }
                self.inner.flush()
            }
        }

        for scoped in [false, true] {
            let (src, dst, caches) = (
                scratch_dir(if scoped { "ckpt-sc-src" } else { "ckpt-src" }),
                scratch_dir(if scoped { "ckpt-sc-dst" } else { "ckpt-dst" }),
                scratch_dir(if scoped { "ckpt-sc-cache" } else { "ckpt-cache" }),
            );
            for i in 0..30 {
                fs::write(src.join(format!("f_{i:02}.txt")), format!("file number {i} content\n").repeat(20)).unwrap();
            }
            let src_cache = caches.join("src.idx");
            let dst_cache = caches.join("dst.idx");

            // Session 1: cut the connection after 12 files, with periodic
            // checkpointing set to every 5 files so periodic saves run too.
            let (a, b) = UnixStream::pair().unwrap();
            let s_dir = src.clone();
            let sc = src_cache.clone();
            let dc = dst_cache.clone();
            let t = thread::spawn(move || {
                let transport = CuttingTransport { inner: a, buf: Vec::new(), skip: 0, in_file_end: false, file_ends: 0, cut_after: 12, cut: false };
                let source = FolderSource { root: s_dir, filter: Filter::default(), cache_path: Some(sc) };
                send(transport, &source, Options { scoped, ..Options::default() })
            });
            let recv_res = receive_inner(b, &dst, Options { scoped, cache_path: Some(dc), ..Options::default() }, |p| fs::File::open(p), 5);
            let send_res = t.join().unwrap();
            assert!(send_res.is_err(), "expected sender error on cut");
            assert!(recv_res.is_err(), "expected receiver error on cut");

            // Cache must exist and record the 12 files already received.
            let cached = Index::load(&dst_cache).expect("checkpoint cache must exist after cut");
            assert_eq!(cached.file_count(), 12, "expected 12 checkpointed files (scoped={scoped})");

            // Session 2: resume full sync; receiver must NOT open/hash any of
            // the 12 already-received files.
            let (a, b) = UnixStream::pair().unwrap();
            let s_dir = src.clone();
            let sc = src_cache.clone();
            let dc = dst_cache.clone();
            let t = thread::spawn(move || {
                let source = FolderSource { root: s_dir, filter: Filter::default(), cache_path: Some(sc) };
                send(a, &source, Options { scoped, ..Options::default() }).unwrap()
            });
            let receiver_opens = AtomicUsize::new(0);
            let r2 = receive_inner(
                b,
                &dst,
                Options { scoped, cache_path: Some(dc), ..Options::default() },
                |p| {
                    receiver_opens.fetch_add(1, Ordering::Relaxed);
                    fs::File::open(p)
                },
                CHECKPOINT_FILES,
            )
            .unwrap();
            let s2 = t.join().unwrap();
            assert!(r2.errors.is_empty(), "{:?}", r2.errors);
            assert_eq!(receiver_opens.load(Ordering::Relaxed), 0, "already-received files were re-hashed on resume (scoped={scoped})");
            assert_eq!(s2.same, 12, "expected 12 unchanged files from session 1 (scoped={scoped})");
            assert_eq!(r2.new, 18, "expected remaining 18 files transferred (scoped={scoped})");
            assert_eq!(Index::scan(&src, &Filter::all(), None).unwrap().tree_hash(), Index::scan(&dst, &Filter::all(), None).unwrap().tree_hash());

            for d in [src, dst, caches] {
                let _ = fs::remove_dir_all(d);
            }
        }
    }

    /// Item 7: on a first pass into an empty receiver, new files are indexed by
    /// stat alone and read from disk exactly once (streaming their BLAKE3 in
    /// `TAG_FILE_END`), and the receiver verifies that trailing hash and rejects
    /// corruption.
    #[test]
    fn new_files_read_once_on_first_pass_and_end_hash_verified() {
        use std::sync::atomic::AtomicU64;

        struct CountingFolderSource {
            inner: FolderSource,
            hash_entry_calls: Arc<AtomicUsize>,
            bytes_read: Arc<AtomicU64>,
        }

        impl Source for CountingFolderSource {
            fn index(&self) -> io::Result<Index> {
                self.inner.index()
            }

            fn index_lazy_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Option<(Index, HashSet<String>)>> {
                self.inner.index_lazy_with_progress(on_entry)
            }

            fn hash_entry(&self, path: &str, size: u64) -> io::Result<chunk::FileDigest> {
                self.hash_entry_calls.fetch_add(1, Ordering::Relaxed);
                self.inner.hash_entry(path, size)
            }

            fn wants_cache(&self) -> bool {
                self.inner.wants_cache()
            }

            fn save_cache(&self, index: &Index) -> io::Result<()> {
                self.inner.save_cache(index)
            }

            fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
                let b = self.inner.read(path, offset, len)?;
                self.bytes_read.fetch_add(b.len() as u64, Ordering::Relaxed);
                Ok(b)
            }

            fn check_unchanged(&self, entry: &Entry) -> bool {
                self.inner.check_unchanged(entry)
            }

            fn extent_at(&self, path: &str, offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
                self.inner.extent_at(path, offset, max_len)
            }
        }

        let (src, dst, caches) = (scratch_dir("once-src"), scratch_dir("once-dst"), scratch_dir("once-cache"));
        let small = b"small single-pass file".to_vec();
        let medium = structured_records(128 << 10);
        let big = structured_records(3 << 20); // > MAX_DATA_PIECE_LEN -> multi-piece stream
        fs::write(src.join("small.txt"), &small).unwrap();
        fs::write(src.join("medium.bin"), &medium).unwrap();
        fs::write(src.join("big.bin"), &big).unwrap();
        let total_len = (small.len() + medium.len() + big.len()) as u64;

        let hash_calls = Arc::new(AtomicUsize::new(0));
        let bytes_read = Arc::new(AtomicU64::new(0));
        let (sc, dc) = (caches.join("s"), caches.join("r"));

        // Pass 1: every file read once, zero pre-hashing.
        let (a, b) = UnixStream::pair().unwrap();
        let s_dir = src.clone();
        let sc1 = sc.clone();
        let hc = hash_calls.clone();
        let br = bytes_read.clone();
        let t = thread::spawn(move || {
            let source = CountingFolderSource { inner: FolderSource { root: s_dir, filter: Filter::default(), cache_path: Some(sc1) }, hash_entry_calls: hc, bytes_read: br };
            send(a, &source, Options::default()).unwrap()
        });
        let r1 = receive(b, &dst, Options { cache_path: Some(dc.clone()), ..Options::default() }).unwrap();
        let _s1 = t.join().unwrap();
        assert!(r1.errors.is_empty(), "{:?}", r1.errors);
        assert_eq!(r1.new, 3);
        assert_eq!(hash_calls.load(Ordering::Relaxed), 0, "first pass must not pre-hash new files");
        assert_eq!(bytes_read.load(Ordering::Relaxed), total_len, "each new file must be read exactly once");
        assert_eq!(fs::read(dst.join("small.txt")).unwrap(), small);
        assert_eq!(fs::read(dst.join("medium.bin")).unwrap(), medium);
        assert_eq!(fs::read(dst.join("big.bin")).unwrap(), big);

        // Pass 2: sender cache saved by pass 1 -> 0 reads, 1-RTT no-op!
        bytes_read.store(0, Ordering::Relaxed);
        let (a, b) = UnixStream::pair().unwrap();
        let s_dir = src.clone();
        let sc2 = sc.clone();
        let hc = hash_calls.clone();
        let br = bytes_read.clone();
        let t = thread::spawn(move || {
            let source = CountingFolderSource { inner: FolderSource { root: s_dir, filter: Filter::default(), cache_path: Some(sc2) }, hash_entry_calls: hc, bytes_read: br };
            send(a, &source, Options::default()).unwrap()
        });
        let r2 = receive(b, &dst, Options { cache_path: Some(dc), ..Options::default() }).unwrap();
        let s2 = t.join().unwrap();
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        assert_eq!(s2.same, 3);
        assert_eq!(bytes_read.load(Ordering::Relaxed), 0);
        assert!(s2.wire_bytes() < 256, "expected 1-RTT no-op after lazy pass 1, got {} B", s2.wire_bytes());

        // Corrupting the 32-byte trailing hash in TAG_FILE_END must fail BLAKE3 verification.
        struct CorruptFileEnd {
            inner: UnixStream,
            buf: Vec<u8>,
            skip: usize,
            corrupting_end: bool,
        }

        impl Read for CorruptFileEnd {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.inner.read(buf)
            }
        }

        impl Write for CorruptFileEnd {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.buf.extend_from_slice(data);
                let mut pos = 0usize;
                loop {
                    if self.skip > 0 {
                        let take = self.skip.min(self.buf.len() - pos);
                        if self.corrupting_end && take > 0 {
                            self.buf[pos] ^= 0xFF;
                            self.corrupting_end = false;
                        }
                        pos += take;
                        self.skip -= take;
                        if self.skip > 0 {
                            break;
                        }
                    }
                    if self.buf.len() - pos < 5 {
                        break;
                    }
                    let len = u32::from_le_bytes(self.buf[pos..pos + 4].try_into().unwrap()) as usize;
                    let tag = self.buf[pos + 4];
                    pos += 5;
                    self.skip = len;
                    self.corrupting_end = tag == TAG_FILE_END && len == 32;
                }
                let out = self.buf.split_off(0);
                self.inner.write_all(&out)?;
                Ok(data.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                self.inner.flush()
            }
        }

        let dst_bad = scratch_dir("once-dst-bad");
        let (a, b) = UnixStream::pair().unwrap();
        let s_dir = src.clone();
        // (A source keeping a cache, new to it: the lazy way, each file's
        // hash at its end.)
        let bad_cache = scratch_dir("once-bad-cache").join("idx");
        let t = thread::spawn(move || {
            let wire = CorruptFileEnd { inner: a, buf: Vec::new(), skip: 0, corrupting_end: false };
            send(wire, &FolderSource { root: s_dir, filter: Filter::default(), cache_path: Some(bad_cache) }, Options::default()).unwrap()
        });
        let r_bad = receive(b, &dst_bad, Options::default()).unwrap();
        let _ = t.join().unwrap();
        assert_eq!(r_bad.new, 0);
        assert_eq!(r_bad.errors.len(), 3);
        assert!(r_bad.errors.iter().all(|e| e.reason.contains("BLAKE3")), "{:?}", r_bad.errors);
        assert!(!dst_bad.join("small.txt").exists());

        for d in [src, dst, dst_bad, caches] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 1: protected files and folders on the receiver are refused on every
    /// operation (create, replace, chmod, symlink, hardlink, remove, dir replace)
    /// with `"kept: the receiver's own"`, while the rest of the session succeeds.
    /// A hard link the sender makes to what the receiver keeps as its own
    /// is refused: never a second name for the receiver's keys (a mode set
    /// on it would be set on them).
    #[test]
    fn a_link_to_a_protected_file_is_refused() {
        use std::os::unix::fs::MetadataExt as _;
        let (src, dst) = (scratch_dir("prot-link-src"), scratch_dir("prot-link-dst"));
        fs::create_dir_all(src.join(".ssh")).unwrap();
        fs::write(src.join(".ssh/authorized_keys"), b"sender key\n").unwrap();
        fs::hard_link(src.join(".ssh/authorized_keys"), src.join("twin")).unwrap();
        fs::create_dir_all(dst.join(".ssh")).unwrap();
        fs::write(dst.join(".ssh/authorized_keys"), b"receiver own key\n").unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let src_dir = src.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(src_dir), Options::default()).unwrap());
        let recv = receive(b, &dst, Options { protect: vec![".ssh/authorized_keys".into()], ..Options::default() }).unwrap();
        t.join().unwrap();
        assert_eq!(fs::read(dst.join(".ssh/authorized_keys")).unwrap(), b"receiver own key\n");
        assert_eq!(fs::metadata(dst.join(".ssh/authorized_keys")).unwrap().nlink(), 1, "no second name for the receiver's keys");
        assert!(recv.errors.iter().any(|e| e.path == "twin" && e.reason == REASON_PROTECTED), "{:?}", recv.errors);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn protected_paths_are_kept_as_the_receivers_own() {
        let (src, dst) = (scratch_dir("prot-src"), scratch_dir("prot-dst"));

        // Sender files: tries to overwrite .ssh/authorized_keys, create inside
        // protected folder `keep`, chmod a same-content protected file, and
        // replace a dir holding a protected file.
        fs::create_dir_all(src.join(".ssh")).unwrap();
        fs::write(src.join(".ssh/authorized_keys"), b"sender key\n").unwrap();
        fs::write(src.join(".ssh/config"), b"Host example\n").unwrap();
        fs::create_dir_all(src.join("keep/sub")).unwrap();
        fs::write(src.join("keep/sub/new.txt"), b"sender file").unwrap();
        symlink("target", src.join("keep/sym")).unwrap();
        fs::write(src.join("same_prot"), b"identical").unwrap();
        fs::set_permissions(src.join("same_prot"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(src.join("ok.txt"), b"arrives fine").unwrap();

        // Receiver state: its own authorized_keys, its own file inside `keep`,
        // extra file to test delete_extra refusal, same_prot with different mode.
        fs::create_dir_all(dst.join(".ssh")).unwrap();
        fs::write(dst.join(".ssh/authorized_keys"), b"receiver own key\n").unwrap();
        fs::create_dir_all(dst.join("keep")).unwrap();
        fs::write(dst.join("keep/mine.txt"), b"receiver only").unwrap();
        fs::write(dst.join("same_prot"), b"identical").unwrap();
        fs::set_permissions(dst.join("same_prot"), fs::Permissions::from_mode(0o600)).unwrap();

        let (a, b) = UnixStream::pair().unwrap();
        let src_dir = src.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(src_dir), Options::default()).unwrap());
        let recv_opts = Options { delete_extra: true, protect: vec![".ssh/authorized_keys".into(), "keep".into(), "same_prot".into()], ..Options::default() };
        let recv_rep = receive(b, &dst, recv_opts).unwrap();
        let send_rep = t.join().unwrap();

        assert_eq!(fs::read(dst.join(".ssh/authorized_keys")).unwrap(), b"receiver own key\n");
        assert_eq!(fs::read(dst.join(".ssh/config")).unwrap(), b"Host example\n");
        assert_eq!(fs::read(dst.join("keep/mine.txt")).unwrap(), b"receiver only");
        assert!(!dst.join("keep/sub/new.txt").exists());
        assert!(fs::symlink_metadata(dst.join("keep/sym")).is_err());
        assert_eq!(fs::metadata(dst.join("same_prot")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read(dst.join("ok.txt")).unwrap(), b"arrives fine");

        for expected in [".ssh/authorized_keys", "keep/sub/new.txt", "keep/sym", "keep/mine.txt", "same_prot"] {
            assert!(recv_rep.errors.iter().any(|e| e.path == expected && e.reason == REASON_PROTECTED), "missing protected error for {expected} in {:?}", recv_rep.errors);
            assert!(send_rep.errors.iter().any(|e| e.path == expected && e.reason == REASON_PROTECTED));
        }

        // Also refuse replacing a parent directory (`.ssh`) with a file when a
        // child (`.ssh/authorized_keys`) is protected.
        let src2 = scratch_dir("prot-src2");
        fs::write(src2.join(".ssh"), b"trying to replace dir").unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let s2 = src2.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(s2), Options::default()).unwrap());
        let r2 = receive(b, &dst, Options { delete_extra: true, protect: vec![".ssh/authorized_keys".into()], ..Options::default() }).unwrap();
        let _ = t.join().unwrap();
        assert_eq!(fs::read(dst.join(".ssh/authorized_keys")).unwrap(), b"receiver own key\n");
        assert!(r2.errors.iter().any(|e| e.path == ".ssh" && e.reason == REASON_PROTECTED), "{:?}", r2.errors);

        for d in [src, src2, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 2 (end-to-end): root-anchored exclude `/.cache` leaves out `.cache`
    /// at the root while still syncing `proj/.cache`.
    #[test]
    fn root_anchored_exclude_syncs_nested_cache_only() {
        let (src, dst) = (scratch_dir("anchor-src"), scratch_dir("anchor-dst"));
        fs::create_dir_all(src.join(".cache")).unwrap();
        fs::create_dir_all(src.join("proj/.cache")).unwrap();
        fs::write(src.join(".cache/top.bin"), b"top cache").unwrap();
        fs::write(src.join("proj/.cache/nested.bin"), b"nested cache").unwrap();
        fs::write(src.join("top.log"), b"top log").unwrap();
        fs::write(src.join("proj/nested.log"), b"nested log").unwrap();

        let opts = Options { filter: Filter { exclude: vec!["/.cache".into(), "/*.log".into()], ..Filter::default() }, ..Options::default() };
        let (_, r) = sync_folders(&src, &dst, None, None, opts);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(!dst.join(".cache").exists());
        assert!(!dst.join("top.log").exists());
        assert_eq!(fs::read(dst.join("proj/.cache/nested.bin")).unwrap(), b"nested cache");
        assert_eq!(fs::read(dst.join("proj/nested.log")).unwrap(), b"nested log");

        for d in [src, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 3: a file that vanishes, shrinks, grows or changes in place between
    /// the sender's index and its read is passed over alone (receiver drops its
    /// part file; sender's `Report.errors` gets `"changed while it was sent"`),
    /// and the rest of the session succeeds.
    #[test]
    fn file_gone_shrunk_grown_or_changed_mid_send_is_passed_over() {
        use std::sync::atomic::AtomicBool;
        let (src, dst) = (scratch_dir("chg-src"), scratch_dir("chg-dst"));

        let big = vec![b'A'; 3 << 20]; // > MAX_DATA_PIECE_LEN (2 MiB) -> 2 pieces
        fs::write(src.join("a_vanish.bin"), &big).unwrap();
        fs::write(src.join("b_shrink.bin"), &big).unwrap();
        fs::write(src.join("c_grow.bin"), &big).unwrap();
        fs::write(src.join("d_mutate.bin"), &big).unwrap();
        fs::write(src.join("e_good.txt"), b"survived cleanly").unwrap();
        // Pre-existing shorter file on dst; sender's larger version vanishes
        // right after `index()` so `append_probe_for` hits NotFound.
        fs::write(dst.join("f_probe_vanish.log"), b"older prefix\n").unwrap();
        fs::write(src.join("f_probe_vanish.log"), b"older prefix\nplus appended lines that vanish\n").unwrap();

        // A Source wrapper that mutates each victim AFTER piece 1 is read (so
        // TAG_FILE_START has already opened a part file on the receiver) and
        // also deletes `f_probe_vanish.log` right after `index()`.
        struct MutatingSource {
            inner: FolderSource,
            mutated_shrink: AtomicBool,
            mutated_vanish: AtomicBool,
            mutated_grow: AtomicBool,
            mutated_same_len: AtomicBool,
        }

        impl Source for MutatingSource {
            fn index(&self) -> io::Result<Index> {
                let idx = self.inner.index()?;
                let _ = fs::remove_file(self.inner.root.join("f_probe_vanish.log"));
                Ok(idx)
            }

            fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
                let data = self.inner.read(path, offset, len)?;
                if path == "a_vanish.bin" && !self.mutated_vanish.swap(true, Ordering::SeqCst) {
                    let _ = fs::remove_file(self.inner.root.join(path));
                } else if path == "b_shrink.bin" && !self.mutated_shrink.swap(true, Ordering::SeqCst) {
                    let _ = fs::OpenOptions::new().write(true).open(self.inner.root.join(path)).and_then(|f| f.set_len(1 << 20));
                } else if path == "c_grow.bin" && !self.mutated_grow.swap(true, Ordering::SeqCst) {
                    let _ = fs::OpenOptions::new().append(true).open(self.inner.root.join(path)).and_then(|mut f| f.write_all(b"extra tail"));
                } else if path == "d_mutate.bin" && !self.mutated_same_len.swap(true, Ordering::SeqCst) {
                    let mut mutated = data;
                    mutated[0] ^= 0xFF;
                    return Ok(mutated);
                }
                Ok(data)
            }

            fn check_unchanged(&self, entry: &Entry) -> bool {
                self.inner.check_unchanged(entry)
            }

            fn extent_at(&self, path: &str, offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
                self.inner.extent_at(path, offset, max_len)
            }
        }

        let (a, b) = UnixStream::pair().unwrap();
        let src_dir = src.clone();
        let t = thread::spawn(move || {
            let source = MutatingSource {
                inner: FolderSource::new(src_dir),
                mutated_shrink: AtomicBool::new(false),
                mutated_vanish: AtomicBool::new(false),
                mutated_grow: AtomicBool::new(false),
                mutated_same_len: AtomicBool::new(false),
            };
            send(a, &source, Options::default()).unwrap()
        });
        let recv_rep = receive(b, &dst, Options::default()).unwrap();
        let send_rep = t.join().unwrap();

        assert_eq!(recv_rep.new, 1);
        assert_eq!(fs::read(dst.join("e_good.txt")).unwrap(), b"survived cleanly");
        assert_eq!(fs::read(dst.join("f_probe_vanish.log")).unwrap(), b"older prefix\n");
        for victim in ["a_vanish.bin", "b_shrink.bin", "c_grow.bin", "d_mutate.bin", "f_probe_vanish.log"] {
            if victim != "f_probe_vanish.log" {
                assert!(!dst.join(victim).exists(), "{victim} should not be committed");
            }
            assert!(send_rep.errors.iter().any(|e| e.path == victim && e.reason == REASON_CHANGED), "expected '{REASON_CHANGED}' for {victim}, got {:?}", send_rep.errors);
        }
        // Receiver must not leave any `.olo-sync-part-*` files behind.
        let leftover_parts: Vec<_> = fs::read_dir(&dst).unwrap().filter_map(|e| e.ok()).filter(|e| crate::index::is_part_name(&e.file_name().to_string_lossy())).collect();
        assert!(leftover_parts.is_empty(), "leftover part files: {leftover_parts:?}");

        for d in [src, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 4a: `append_in_place` never appends into a file with more than one
    /// hard link (it would mutate its twin); instead the file is written anew.
    #[test]
    fn append_does_not_mutate_hardlinked_twin() {
        let (src, dst, caches) = (scratch_dir("hl-app-src"), scratch_dir("hl-app-dst"), scratch_dir("hl-app-cache"));
        let (mut log, next) = jsonl_data(1 << 20, 0);
        fs::write(src.join("log.jsonl"), &log).unwrap();

        let (s, r) = (Some(caches.join("s")), Some(caches.join("r")));
        sync_folders(&src, &dst, s.clone(), r.clone(), Options::default());

        // Create a local hard-linked twin on the receiver.
        let old_content = log.clone();
        let old_ino = fs::metadata(dst.join("log.jsonl")).unwrap().ino();
        fs::hard_link(dst.join("log.jsonl"), dst.join("twin.jsonl")).unwrap();

        let (more, _) = jsonl_data(4096, next);
        log.extend_from_slice(&more);
        fs::write(src.join("log.jsonl"), &log).unwrap();

        let (_, got) = sync_folders(&src, &dst, s, r, Options::default());
        assert!(got.errors.is_empty(), "{:?}", got.errors);
        assert_eq!(got.appended, 1);
        assert_eq!(fs::read(dst.join("log.jsonl")).unwrap(), log);
        assert_ne!(fs::metadata(dst.join("log.jsonl")).unwrap().ino(), old_ino, "must write anew rather than in-place");
        assert_eq!(fs::read(dst.join("twin.jsonl")).unwrap(), old_content, "twin must stay untouched");

        for d in [src, dst, caches] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 4b: hard links are carried among included entries, re-linked when
    /// the first file is replaced on a later pass, and sent whole when the
    /// first twin is excluded by a filter.
    #[test]
    fn hardlinks_are_carried_relinked_on_replace_and_respect_filters() {
        let (src, dst, caches) = (scratch_dir("hl-src"), scratch_dir("hl-dst"), scratch_dir("hl-cache"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"version one content").unwrap();
        fs::hard_link(src.join("a.txt"), src.join("b.txt")).unwrap();
        fs::hard_link(src.join("a.txt"), src.join("sub/c.txt")).unwrap();

        let (sc, dc) = (Some(caches.join("s")), Some(caches.join("r")));
        let (_, r1) = sync_folders(&src, &dst, sc.clone(), dc.clone(), Options::default());
        assert!(r1.errors.is_empty(), "{:?}", r1.errors);
        assert_eq!(r1.new, 1);

        let a_meta = fs::metadata(dst.join("a.txt")).unwrap();
        let b_meta = fs::metadata(dst.join("b.txt")).unwrap();
        let c_meta = fs::metadata(dst.join("sub/c.txt")).unwrap();
        assert_eq!((a_meta.dev(), a_meta.ino()), (b_meta.dev(), b_meta.ino()));
        assert_eq!((a_meta.dev(), a_meta.ino()), (c_meta.dev(), c_meta.ino()));
        assert_eq!(a_meta.nlink(), 3);

        // Pass 2 (no changes): fast path in one round trip.
        let (s2, r2) = sync_folders(&src, &dst, sc.clone(), dc.clone(), Options::default());
        assert!(r2.errors.is_empty());
        assert!(s2.wire_bytes() < 256, "expected no-op fast path, got {} B", s2.wire_bytes());

        // Pass 3: replace `a.txt` (with `b.txt` and `sub/c.txt` still linked to
        // it on the sender). Receiver replaces `a.txt` with a new inode and
        // must re-link `b.txt` and `sub/c.txt` to the new inode.
        fs::write(src.join("a.txt"), b"version two updated content").unwrap();
        let (_, r3) = sync_folders(&src, &dst, sc.clone(), dc.clone(), Options::default());
        assert!(r3.errors.is_empty(), "{:?}", r3.errors);
        let a_meta3 = fs::metadata(dst.join("a.txt")).unwrap();
        let b_meta3 = fs::metadata(dst.join("b.txt")).unwrap();
        let c_meta3 = fs::metadata(dst.join("sub/c.txt")).unwrap();
        assert_ne!(a_meta3.ino(), a_meta.ino());
        assert_eq!((a_meta3.dev(), a_meta3.ino()), (b_meta3.dev(), b_meta3.ino()));
        assert_eq!((a_meta3.dev(), a_meta3.ino()), (c_meta3.dev(), c_meta3.ino()));
        assert_eq!(fs::read(dst.join("b.txt")).unwrap(), b"version two updated content");
        assert_eq!(fs::read(dst.join("sub/c.txt")).unwrap(), b"version two updated content");

        // Also works in scoped mode!
        let dst_scoped = scratch_dir("hl-dst-scoped");
        let (_, rs1) = sync_folders(&src, &dst_scoped, None, None, Options { scoped: true, ..Options::default() });
        assert!(rs1.errors.is_empty(), "{:?}", rs1.errors);
        assert_eq!(fs::metadata(dst_scoped.join("a.txt")).unwrap().ino(), fs::metadata(dst_scoped.join("b.txt")).unwrap().ino());
        fs::write(src.join("a.txt"), b"version three scoped").unwrap();
        let (_, rs2) = sync_folders(&src, &dst_scoped, None, None, Options { scoped: true, ..Options::default() });
        assert!(rs2.errors.is_empty(), "{:?}", rs2.errors);
        assert_eq!(fs::metadata(dst_scoped.join("a.txt")).unwrap().ino(), fs::metadata(dst_scoped.join("b.txt")).unwrap().ino());
        assert_eq!(fs::read(dst_scoped.join("b.txt")).unwrap(), b"version three scoped");

        // Filter excludes `a.txt`: `b.txt` is sent whole and `sub/c.txt` links to `b.txt`.
        let dst_filt = scratch_dir("hl-dst-filt");
        let filt_opts = Options { filter: Filter { exclude: vec!["/a.txt".into()], ..Filter::default() }, ..Options::default() };
        let (_, rf) = sync_folders(&src, &dst_filt, None, None, filt_opts);
        assert!(rf.errors.is_empty(), "{:?}", rf.errors);
        assert!(!dst_filt.join("a.txt").exists());
        assert_eq!(fs::read(dst_filt.join("b.txt")).unwrap(), b"version three scoped");
        assert_eq!(fs::metadata(dst_filt.join("b.txt")).unwrap().ino(), fs::metadata(dst_filt.join("sub/c.txt")).unwrap().ino());

        for d in [src, dst, dst_scoped, dst_filt, caches] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 5: filenames up to `NAME_MAX` (255 bytes) sync cleanly because the
    /// part file uses a fixed-length hashed name instead of `<name>.olo-sync-part`.
    #[test]
    fn long_filenames_up_to_name_max_sync_cleanly() {
        let (src, dst) = (scratch_dir("long-src"), scratch_dir("long-dst"));
        let name_255 = "x".repeat(255);
        let link_250 = format!("l{}", "y".repeat(249));
        let hard_250 = format!("h{}", "z".repeat(249));
        fs::write(src.join(&name_255), b"255-byte filename works").unwrap();
        symlink(&name_255, src.join(&link_250)).unwrap();
        fs::hard_link(src.join(&name_255), src.join(&hard_250)).unwrap();

        let (_, r) = sync_folders(&src, &dst, None, None, Options::default());
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(fs::read(dst.join(&name_255)).unwrap(), b"255-byte filename works");
        assert_eq!(fs::read_link(dst.join(&link_250)).unwrap().to_str().unwrap(), name_255);
        assert_eq!(fs::metadata(dst.join(&name_255)).unwrap().ino(), fs::metadata(dst.join(&hard_250)).unwrap().ino());

        for d in [src, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 6: large manifests with many long symlinks, a huge file's
    /// super-chunk list exceeding `MAX_FRAME_LEN`, and reports with thousands
    /// of errors all split cleanly across frames without hitting `MAX_FRAME_LEN`.
    #[test]
    fn frames_over_max_frame_len_split_cleanly() {
        // 6a: Many long symlinks (> 8 MiB uncompressed manifest on receiver).
        let (src, dst) = (scratch_dir("bigsym-src"), scratch_dir("bigsym-dst"));
        let long_target = "t/".repeat(1800); // 3,600 bytes each
        for i in 0..2600 {
            let name = format!("s{i:04}");
            symlink(&long_target, dst.join(&name)).unwrap();
            symlink(&long_target, src.join(&name)).unwrap();
        }
        // One differing file so hello tree-hash mismatches and manifest is sent.
        fs::write(src.join("trigger.txt"), b"changed").unwrap();
        let (_, r) = sync_folders(&src, &dst, None, None, Options::default());
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(fs::read(dst.join("trigger.txt")).unwrap(), b"changed");

        // 6b: One huge file whose super-chunk list exceeds MAX_FRAME_LEN (8 MiB).
        // 212,000 super-chunks * 40 B = 8.48 MiB raw.
        let mut fake_chunks = Vec::with_capacity(212_000 * 8);
        for i in 0..(212_000u32 * 8) {
            let mut id = [0u8; 16];
            id[1..5].copy_from_slice(&i.to_le_bytes()); // id[0] == 0 -> cut every 8 chunks
            fake_chunks.push(ChunkRef { len: 16384, id });
        }
        let expected_supers = recipe::super_chunks(&fake_chunks);
        assert!(expected_supers.len() * 40 > MAX_FRAME_LEN, "expected > 8 MiB super-chunk list");
        let mut fake_idx = Index::default();
        fake_idx.entries.insert(
            "huge.img".into(),
            Entry { path: "huge.img".into(), kind: EntryKind::File, size: fake_chunks.len() as u64 * 16384, mtime_ns: 0, ctime_ns: 0, ino: 1, mode: 0o644, hash: [1; 32], chunks: fake_chunks },
        );
        let (a, b) = UnixStream::pair().unwrap();
        let reqs = vec![ChunkReq::SuperOrLeaf { path: "huge.img".into() }];
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            let mut scache = HashMap::new();
            send_chunk_info(&mut w, &fake_idx, &reqs, &mut scache).unwrap();
            w.flush().unwrap();
        });
        let mut w_recv = Wire::new(b);
        let mut resps = HashMap::new();
        let mut frame_count = 0usize;
        loop {
            let (tag, payload) = w_recv.read_frame().unwrap();
            assert_eq!(tag, TAG_CHUNK_INFO);
            frame_count += 1;
            if !decode_chunk_info_frame(&payload, &mut resps).unwrap() {
                break;
            }
        }
        t.join().unwrap();
        assert!(frame_count > 1, "expected super-chunk list to span multiple frames");
        match resps.get("huge.img") {
            Some(ChunkResp::SuperChunks(got)) => assert_eq!(got, &expected_supers),
            other => panic!("unexpected chunk response: {:?}", other.is_some()),
        }

        // 6c: Report with thousands of errors exceeding MAX_FRAME_LEN (raw > 9 MiB).
        let big_report = Report {
            errors: (0..12_000).map(|i| PathError { path: format!("dir/{}/file_{i:05}.dat", "p".repeat(400)), reason: format!("failed: {}", "r".repeat(400)) }).collect(),
            ..Report::default()
        };
        let (a, b) = UnixStream::pair().unwrap();
        let expected_errors = big_report.errors.clone();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            send_report(&mut w, &big_report).unwrap();
            w.flush().unwrap();
        });
        let mut w_recv = Wire::new(b);
        let got_rep = read_report(&mut w_recv).unwrap();
        t.join().unwrap();
        assert_eq!(got_rep.errors, expected_errors);

        for d in [src, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 7: sparse files stay sparse on the receiver (aligned 4 KiB zero
    /// blocks are skipped via seek + `set_len`), and the sender skips holes
    /// with `SEEK_DATA`/`SEEK_HOLE` so almost nothing crosses the wire.
    #[test]
    fn sparse_files_stay_sparse_and_skip_holes_on_wire() {
        let (src, dst) = (scratch_dir("sparse-src"), scratch_dir("sparse-dst"));
        let sparse_path = src.join("disk.img");
        {
            let mut f = fs::File::create(&sparse_path).unwrap();
            f.write_all(&[0xAB; 4096]).unwrap();
            f.seek(SeekFrom::Start(8 << 20)).unwrap();
            f.write_all(&[0xCD; 4096]).unwrap();
            // Also leave a trailing 8 MiB hole (total 16 MiB).
            f.set_len(16 << 20).unwrap();
        }

        let (s, r) = sync_folders(&src, &dst, None, None, Options::default());
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.new, 1);

        let src_meta = fs::metadata(&sparse_path).unwrap();
        let dst_meta = fs::metadata(dst.join("disk.img")).unwrap();
        assert_eq!(dst_meta.len(), 16 << 20);
        // Allocated bytes on disk must be far less than logical size (16 MiB).
        assert!(dst_meta.blocks() * 512 < 1 << 20, "expected sparse dst, allocated {} bytes (src allocated {} bytes)", dst_meta.blocks() * 512, src_meta.blocks() * 512);
        // Sender skipped holes via SEEK_DATA/SEEK_HOLE so wire bytes are tiny.
        assert!(s.wire_bytes() < 32 << 10, "expected tiny wire bytes for sparse file, got {} B", s.wire_bytes());
        assert_eq!(fs::read(&sparse_path).unwrap(), fs::read(dst.join("disk.img")).unwrap());

        // Also test receiver-side zero-block skipping when data arrives as
        // normal Piece::Data (ListSource has no filesystem holes).
        let dst2 = scratch_dir("sparse-dst2");
        let mut raw_with_zeros = vec![0u8; 4 << 20];
        raw_with_zeros[0..4096].fill(0x55);
        raw_with_zeros[(2 << 20)..(2 << 20) + 4096].fill(0x77);
        let expected = raw_with_zeros.clone();
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let src = ListSource::new(vec![ListEntry::new("zeroed.bin", 0o644, 1_700_000_000_000_000_000)], move |_| Ok(raw_with_zeros.clone()));
            send(a, &src, Options::default()).unwrap()
        });
        let r2 = receive(b, &dst2, Options::default()).unwrap();
        let _ = t.join().unwrap();
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        let m2 = fs::metadata(dst2.join("zeroed.bin")).unwrap();
        assert_eq!(m2.len(), 4 << 20);
        assert!(m2.blocks() * 512 < 512 << 10, "expected sparse dst2 from zero runs, allocated {} bytes", m2.blocks() * 512);
        assert_eq!(fs::read(dst2.join("zeroed.bin")).unwrap(), expected);

        for d in [src, dst, dst2] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 8: folders stay writable while syncing and are finalized deepest
    /// first to the sender's mode (including 0555, sticky 01755, setgid 02755,
    /// never setuid) and mtime after their contents; a second session into
    /// existing 0555 folders still succeeds.
    #[test]
    fn folder_modes_times_and_second_pass_into_readonly_dirs() {
        let (src, dst, caches) = (scratch_dir("dmode-src"), scratch_dir("dmode-dst"), scratch_dir("dmode-cache"));
        fs::create_dir_all(src.join("ro/inner")).unwrap();
        fs::write(src.join("ro/inner/go.mod"), b"v1").unwrap();
        fs::create_dir_all(src.join("sticky")).unwrap();
        fs::create_dir_all(src.join("sgid")).unwrap();

        let past = UNIX_EPOCH + Duration::from_secs(1_700_000_123);
        fs::File::open(src.join("ro/inner")).unwrap().set_modified(past).unwrap();
        fs::File::open(src.join("ro")).unwrap().set_modified(past).unwrap();
        fs::set_permissions(src.join("ro/inner"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(src.join("sticky"), fs::Permissions::from_mode(0o1755)).unwrap();
        fs::set_permissions(src.join("sgid"), fs::Permissions::from_mode(0o2755)).unwrap();

        let (sc, dc) = (Some(caches.join("s")), Some(caches.join("r")));
        let (_, r1) = sync_folders(&src, &dst, sc.clone(), dc.clone(), Options::default());
        assert!(r1.errors.is_empty(), "{:?}", r1.errors);
        assert_eq!(fs::metadata(dst.join("ro")).unwrap().permissions().mode() & 0o7777, 0o555);
        assert_eq!(fs::metadata(dst.join("ro/inner")).unwrap().permissions().mode() & 0o7777, 0o555);
        assert_eq!(fs::metadata(dst.join("sticky")).unwrap().permissions().mode() & 0o7777, 0o1755);
        assert_eq!(fs::metadata(dst.join("sgid")).unwrap().permissions().mode() & 0o7777, 0o2755);
        assert_eq!(fs::metadata(dst.join("ro/inner")).unwrap().mtime(), 1_700_000_123);
        assert_eq!(fs::metadata(dst.join("ro")).unwrap().mtime(), 1_700_000_123);
        // Setuid bit is never preserved by final_dir_mode.
        assert_eq!(final_dir_mode(0o4755), 0o0755);
        assert_eq!(final_dir_mode(0o7755), 0o3755);

        // Pass 2: add a new file inside the already-0555 nested folders.
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(src.join("ro/inner"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(src.join("ro/inner/go.sum"), b"checksum").unwrap();
        fs::set_permissions(src.join("ro/inner"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();

        let (_, r2) = sync_folders(&src, &dst, sc.clone(), dc.clone(), Options::default());
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        assert_eq!(fs::read(dst.join("ro/inner/go.sum")).unwrap(), b"checksum");
        assert_eq!(fs::metadata(dst.join("ro/inner")).unwrap().permissions().mode() & 0o7777, 0o555);
        assert_eq!(fs::metadata(dst.join("ro")).unwrap().permissions().mode() & 0o7777, 0o555);

        // Pass 3: remove a file inside the 0555 nested folders with delete_extra.
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(src.join("ro/inner"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(src.join("ro/inner/go.sum")).unwrap();
        fs::set_permissions(src.join("ro/inner"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();

        let (_, r3) = sync_folders(&src, &dst, sc, dc, Options { delete_extra: true, ..Options::default() });
        assert!(r3.errors.is_empty(), "{:?}", r3.errors);
        assert!(!dst.join("ro/inner/go.sum").exists());
        assert_eq!(fs::metadata(dst.join("ro/inner")).unwrap().permissions().mode() & 0o7777, 0o555);
        assert_eq!(fs::metadata(dst.join("ro")).unwrap().permissions().mode() & 0o7777, 0o555);

        for d in [src, dst] {
            let _ = fs::set_permissions(d.join("ro"), fs::Permissions::from_mode(0o755));
            let _ = fs::set_permissions(d.join("ro/inner"), fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(d);
        }
        let _ = fs::remove_dir_all(caches);
    }

    /// Item 9: the index does not descend into folders on another filesystem
    /// (`st_dev` differs from root's), recording them in `Index.mounts` and
    /// `Report.skipped.mounts`, and a mirror does not delete them on receiver.
    #[test]
    fn one_filesystem_skips_and_records_mounts() {
        let (src, dst) = (scratch_dir("mnt-src"), scratch_dir("mnt-dst"));
        fs::create_dir_all(src.join("other_fs")).unwrap();
        fs::write(src.join("other_fs/outside.txt"), b"different fs").unwrap();
        fs::create_dir_all(src.join("sub/nested_fs")).unwrap();
        fs::write(src.join("sub/nested_fs/deep.txt"), b"different fs 2").unwrap();
        fs::write(src.join("home.txt"), b"same fs").unwrap();

        // Pre-existing dir on receiver at the mount path must survive delete_extra.
        fs::create_dir_all(dst.join("other_fs")).unwrap();
        fs::write(dst.join("other_fs/kept.txt"), b"receiver mount").unwrap();

        struct MountSimSource {
            root: PathBuf,
        }

        impl Source for MountSimSource {
            fn index(&self) -> io::Result<Index> {
                let root_dev = fs::symlink_metadata(&self.root)?.dev();
                Index::scan_with_dev(
                    &self.root,
                    &Filter::all(),
                    None,
                    |p| fs::File::open(p),
                    |p, m| {
                        if p.ends_with("other_fs") || p.ends_with("nested_fs") { root_dev.wrapping_add(1) } else { m.dev() }
                    },
                )
            }

            fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
                FolderSource::new(&self.root).read(path, offset, len)
            }
        }

        let (a, b) = UnixStream::pair().unwrap();
        let src_dir = src.clone();
        let t = thread::spawn(move || send(a, &MountSimSource { root: src_dir }, Options { delete_extra: true, ..Options::default() }).unwrap());
        let recv_rep = receive(b, &dst, Options { delete_extra: true, ..Options::default() }).unwrap();
        let send_rep = t.join().unwrap();

        assert!(recv_rep.errors.is_empty(), "{:?}", recv_rep.errors);
        assert_eq!(fs::read(dst.join("home.txt")).unwrap(), b"same fs");
        assert!(!dst.join("other_fs/outside.txt").exists());
        assert!(!dst.join("sub/nested_fs").exists());
        assert_eq!(fs::read(dst.join("other_fs/kept.txt")).unwrap(), b"receiver mount");
        assert_eq!(send_rep.skipped.mounts.count, 2);
        assert_eq!(send_rep.skipped.mounts.paths, vec!["other_fs", "sub/nested_fs"]);

        for d in [src, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// Item 10: everything not sent (unreadable files, non-UTF-8 names,
    /// sockets/FIFOs, mounts) is counted and named (up to 200 paths each,
    /// lossily printed for non-UTF-8 names) in the sender's `Report.skipped`.
    #[test]
    fn skipped_items_are_counted_and_named_up_to_200() {
        use std::ffi::{CString, OsStr};
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::net::UnixListener;

        let (src, dst) = (scratch_dir("skip-src"), scratch_dir("skip-dst"));
        fs::write(src.join("normal.txt"), b"ok").unwrap();

        // Non-UTF-8 filename.
        let odd_path = src.join(OsStr::from_bytes(b"odd-\xff-file.txt"));
        fs::write(&odd_path, b"odd").unwrap();

        // Special files: Unix socket and FIFO.
        let _listener = UnixListener::bind(src.join("agent.sock")).unwrap();
        let fifo_c = CString::new(src.join("pipe.fifo").as_os_str().as_bytes()).unwrap();
        // SAFETY: valid NUL-terminated path string in our scratch dir.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);

        // Unreadable files: simulate 250 via scan_with + 1 mount to verify the
        // 200-path cap and exact counts across every category.
        fs::create_dir_all(src.join("mnt")).unwrap();
        for i in 0..250 {
            fs::write(src.join(format!("unreadable_{i:03}")), b"secret").unwrap();
        }

        struct SkippedSource {
            root: PathBuf,
        }

        impl Source for SkippedSource {
            fn index(&self) -> io::Result<Index> {
                let root_dev = fs::symlink_metadata(&self.root)?.dev();
                Index::scan_with_dev(
                    &self.root,
                    &Filter::all(),
                    None,
                    |p| {
                        if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("unreadable_")) {
                            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
                        }
                        fs::File::open(p)
                    },
                    |p, m| if p.ends_with("mnt") { root_dev.wrapping_add(1) } else { m.dev() },
                )
            }

            fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
                FolderSource::new(&self.root).read(path, offset, len)
            }
        }

        let run = || {
            let (a, b) = UnixStream::pair().unwrap();
            let s = src.clone();
            let t = thread::spawn(move || send(a, &SkippedSource { root: s }, Options::default()).unwrap());
            let r = receive(b, &dst, Options::default()).unwrap();
            (t.join().unwrap(), r)
        };

        // Pass 1 (normal path) and Pass 2 (tree-hash fast path) both carry `skipped`.
        for pass in 1..=2 {
            let (send_rep, recv_rep) = run();
            assert!(recv_rep.errors.is_empty(), "pass {pass}: {:?}", recv_rep.errors);
            assert_eq!(send_rep.skipped.unread.count, 250, "pass {pass}");
            assert_eq!(send_rep.skipped.unread.paths.len(), crate::index::MAX_SKIPPED_PATHS, "pass {pass}");
            assert_eq!(send_rep.skipped.non_utf8.count, 1, "pass {pass}");
            assert_eq!(send_rep.skipped.non_utf8.paths, vec!["odd-\u{fffd}-file.txt"], "pass {pass}");
            assert_eq!(send_rep.skipped.special.count, 2, "pass {pass}");
            assert_eq!(send_rep.skipped.special.paths, vec!["agent.sock", "pipe.fifo"], "pass {pass}");
            assert_eq!(send_rep.skipped.mounts.count, 1, "pass {pass}");
            assert_eq!(send_rep.skipped.mounts.paths, vec!["mnt"], "pass {pass}");
        }
        assert_eq!(fs::read(dst.join("normal.txt")).unwrap(), b"ok");

        drop(_listener);
        for d in [src, dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// What a home-like copy costs (a measurement, run by hand, release;
    /// as yourself: under `sudo perf stat` give it `sudo -u $USER`, or its
    /// tree in /var/tmp is root's): 20 000 small files, 50 of a few MiB, a
    /// sparse image; a first pass into nothing, then an unchanged second.
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn engine_bench_home() {
        let src = PathBuf::from("/var/tmp/olo-engine-bench-src");
        if !src.join("done").exists() {
            let _ = fs::remove_dir_all(&src);
            let mut x: u64 = 0x9E3779B97F4A7C15;
            let mut next = || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            };
            for d in 0..200 {
                let dir = src.join(format!("proj{}/src/mod{}", d % 20, d));
                fs::create_dir_all(&dir).unwrap();
                for f in 0..100 {
                    let len = 1000 + (next() % 19_000) as usize;
                    let bytes: Vec<u8> = (0..len).map(|i| if i % 7 == 0 { b'\n' } else { b'a' + (next() % 26) as u8 }).collect();
                    fs::write(dir.join(format!("f{f}.rs")), bytes).unwrap();
                }
            }
            fs::create_dir_all(src.join("big")).unwrap();
            for b in 0..50 {
                let len = (1 << 20) + (next() % (4 << 20)) as usize;
                let bytes: Vec<u8> = (0..len).map(|_| (next() & 0xff) as u8).collect();
                fs::write(src.join(format!("big/b{b}.bin")), bytes).unwrap();
            }
            let f = fs::File::create(src.join("disk.img")).unwrap();
            f.set_len(64 << 20).unwrap();
            use std::os::unix::fs::FileExt as _;
            let data: Vec<u8> = (0..(4 << 20)).map(|_| (next() & 0xff) as u8).collect();
            f.write_at(&data, 16 << 20).unwrap();
            fs::write(src.join("done"), b"").unwrap();
        }
        let dst = PathBuf::from("/var/tmp/olo-engine-bench-dst");
        let _ = fs::remove_dir_all(&dst);
        fs::create_dir_all(&dst).unwrap();
        for pass in 1..=2 {
            let began = std::time::Instant::now();
            let (a, b) = UnixStream::pair().unwrap();
            let s = src.clone();
            let t = thread::spawn(move || send(a, &FolderSource::new(s), Options::default()).unwrap());
            let r = receive(b, &dst, Options { scoped: true, ..Options::default() }).unwrap();
            let sr = t.join().unwrap();
            println!("pass {pass}: {:.2} s, wire {} bytes, received {} new {} copied, errors {}", began.elapsed().as_secs_f64(), sr.wire_bytes(), r.new, r.copied, r.errors.len());
        }
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn sender_summary_reports_non_utf8_and_confirms_clean_copies() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let dir = PathBuf::from(format!("/var/tmp/olo-sync-summary-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (src, dst, empty_src, empty_dst) = (dir.join("src"), dir.join("dst"), dir.join("empty-src"), dir.join("empty-dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::create_dir_all(&empty_src).unwrap();
        fs::create_dir_all(&empty_dst).unwrap();

        // 1. Non-empty folder containing both a valid UTF-8 file and a non-UTF-8 file.
        fs::write(src.join("good.txt"), b"ok").unwrap();
        let bad_name = OsStr::from_bytes(b"bad-\xff-name.txt");
        fs::write(src.join(bad_name), b"skipped").unwrap();

        let (a, b) = UnixStream::pair().unwrap();
        let src_c = src.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(src_c), Options::default()).unwrap());
        let r = receive(b, &dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        let s = t.join().unwrap();
        assert!(r.sender_confirmed, "receiver must see sender_confirmed = true");
        assert_eq!(r.skipped.non_utf8.count, 1, "receiver must receive skipped non-UTF-8 count");
        assert!(!r.skipped.is_empty());
        assert_eq!(s.skipped.non_utf8.count, 1);

        // 2. Folder containing ONLY a non-UTF-8 file hits the tree-hash in_sync fast path:
        // receiver MUST STILL learn that 1 non-UTF-8 entry was skipped!
        fs::write(empty_src.join(bad_name), b"only-non-utf8").unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let es = empty_src.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(es), Options::default()).unwrap());
        let r_fast = receive(b, &empty_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        let _ = t.join().unwrap();
        assert!(r_fast.sender_confirmed, "fast path must still confirm sender summary");
        assert_eq!(r_fast.skipped.non_utf8.count, 1, "in_sync fast path must report skipped non-UTF-8 entry");

        // 3. Clean folder with zero skipped entries confirms complete.
        fs::remove_file(src.join(bad_name)).unwrap();
        let clean_dst = dir.join("clean-dst");
        let (a, b) = UnixStream::pair().unwrap();
        let src_c = src.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(src_c), Options::default()).unwrap());
        let r_clean = receive(b, &clean_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        let _ = t.join().unwrap();
        assert!(r_clean.sender_confirmed);
        assert!(r_clean.skipped.is_empty());
        assert!(r_clean.errors.is_empty());

        // 4. Legacy sender simulation (ignores FLAG_SENDER_SUMMARY and sends TAG_DONE without TAG_SENDER_SUMMARY):
        // receiver finishes with sender_confirmed == false so callers refuse source deletion.
        let legacy_dst = dir.join("legacy-dst");
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            let (tag, _hello) = w.read_frame().unwrap();
            assert_eq!(tag, TAG_HELLO);
            let mut ack = Vec::with_capacity(5);
            ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
            ack.push(0); // not in_sync
            w.write_frame(TAG_HELLO_ACK, &ack).unwrap();
            w.flush().unwrap();
            let (mtag, _) = w.read_frame().unwrap();
            assert_eq!(mtag, TAG_MANIFEST);
            // Send TAG_DONE directly without TAG_SENDER_SUMMARY (as an older helper does).
            w.write_frame(TAG_DONE, &[0u8; 32]).unwrap();
            w.flush().unwrap();
            let _ = read_report(&mut w).unwrap();
        });
        let r_legacy = receive(b, &legacy_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        t.join().unwrap();
        assert!(!r_legacy.sender_confirmed, "legacy sender without TAG_SENDER_SUMMARY must leave sender_confirmed = false");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sender_summary_hardening_scoped_siblings_limits_duplicates_and_in_sync_compat() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let dir = PathBuf::from(format!("/var/tmp/olo-sync-summary-hard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        // 1. Scoped single-file transfer with a non-UTF-8 sibling and reserved part sibling
        //    must succeed with empty skipped summary.
        fs::write(src.join("report.pdf"), b"pdf-bytes").unwrap();
        fs::write(src.join(OsStr::from_bytes(b"legacy-\xff.txt")), b"ignore").unwrap();
        fs::write(src.join("notes.olo-sync-part"), b"part").unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let src_c = src.clone();
        let t = thread::spawn(move || {
            let filter = Filter { only: vec!["report.pdf".into()], ..Default::default() };
            let source = FolderSource { root: src_c, filter: filter.clone(), cache_path: None };
            send(a, &source, Options { scoped: true, filter, ..Options::default() }).unwrap()
        });
        let r_scoped = receive(b, &dst, Options { scoped: true, request_sender_summary: true, ..Options::default() }).unwrap();
        let s_scoped = t.join().unwrap();
        assert!(r_scoped.sender_confirmed);
        assert!(r_scoped.skipped.is_empty(), "unrelated siblings broke scoped transfer: {:?}", r_scoped.skipped);
        assert!(s_scoped.skipped.is_empty());
        assert_eq!(fs::read(dst.join("report.pdf")).unwrap(), b"pdf-bytes");

        // 2. Full-folder transfer with reserved part file MUST report it as skipped
        //    (so Move never silently deletes user files named `*.olo-sync-part`).
        let part_src = dir.join("part-src");
        let part_dst = dir.join("part-dst");
        fs::create_dir_all(&part_src).unwrap();
        fs::write(part_src.join("notes.olo-sync-part"), b"important").unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let ps = part_src.clone();
        let t = thread::spawn(move || send(a, &FolderSource::new(ps), Options::default()).unwrap());
        let r_part = receive(b, &part_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        let _ = t.join().unwrap();
        assert!(r_part.sender_confirmed);
        assert_eq!(r_part.skipped.unread.count, 1, "reserved part file must be reported in skipped");

        // 3. Large skipped-path summary (4 × 200 × 3000-byte paths + errors) round-trips cleanly.
        let big_path = "a/".repeat(1500);
        let mk_kind = || SkippedKind { count: 250, paths: vec![big_path.clone(); MAX_SKIPPED_PATHS] };
        let big_skipped = Skipped { unread: mk_kind(), non_utf8: mk_kind(), special: mk_kind(), mounts: mk_kind() };
        let errs = vec![PathError { path: "x".into(), reason: "denied".into() }; 5];
        let (a, b) = UnixStream::pair().unwrap();
        let errs_c = errs.clone();
        let skipped_c = big_skipped.clone();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            send_sender_summary(&mut w, &errs_c, &skipped_c).unwrap();
            w.flush().unwrap();
        });
        let mut rw = Wire::new(b);
        let (tag, payload) = rw.read_frame().unwrap();
        assert_eq!(tag, TAG_SENDER_SUMMARY);
        let (got_errs, got_skipped) = read_sender_summary_frames(&mut rw, &payload).unwrap();
        t.join().unwrap();
        assert_eq!(got_errs.len(), 5);
        assert_eq!(got_skipped.unread.count, 250);
        assert_eq!(got_skipped.non_utf8.count, 250);
        assert_eq!(got_skipped.special.count, 250);
        assert_eq!(got_skipped.mounts.count, 250);

        // 4. Duplicate TAG_SENDER_SUMMARY is rejected (cannot overwrite skipped counts).
        let dup_dst = dir.join("dup-dst");
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            let _ = w.read_frame().unwrap();
            let mut ack = Vec::with_capacity(5);
            ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
            ack.push(0);
            w.write_frame(TAG_HELLO_ACK, &ack).unwrap();
            w.flush().unwrap();
            let _ = w.read_frame().unwrap();
            let mut sk = Skipped::default();
            sk.non_utf8.push("bad".into());
            send_sender_summary(&mut w, &[], &sk).unwrap();
            send_sender_summary(&mut w, &[], &Skipped::default()).unwrap();
            let _ = w.flush();
        });
        let err = receive(b, &dup_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap_err();
        let _ = t.join();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("SenderSummary"), "{err}");

        // 5. Legacy equal-tree sender (`in_sync = 1`) over a KEPT-OPEN duplex stream
        //    must return immediately with `sender_confirmed = false` (never block).
        let open_dst = dir.join("open-dst");
        let (a, b) = UnixStream::pair().unwrap();
        let (hold_tx, hold_rx) = std::sync::mpsc::channel::<()>();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            let (tag, _) = w.read_frame().unwrap();
            assert_eq!(tag, TAG_HELLO);
            let mut ack = Vec::with_capacity(5);
            ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
            ack.push(1); // legacy in_sync without summary
            w.write_frame(TAG_HELLO_ACK, &ack).unwrap();
            w.flush().unwrap();
            // Keep socket open until receiver returns!
            let _ = hold_rx.recv();
        });
        let r_open = receive(b, &open_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        let _ = hold_tx.send(());
        t.join().unwrap();
        assert!(!r_open.sender_confirmed, "legacy in_sync=1 must return immediately with sender_confirmed=false");

        // 6. Capable equal-tree sender (`in_sync = 2`) that disconnects mid-summary
        //    MUST fail with UnexpectedEof instead of returning Ok.
        let trunc_dst = dir.join("trunc-dst");
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            let _ = w.read_frame().unwrap();
            let mut ack = Vec::with_capacity(5);
            ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
            ack.push(2); // promises TAG_SENDER_SUMMARY
            w.write_frame(TAG_HELLO_ACK, &ack).unwrap();
            w.flush().unwrap();
            // Write only 2 bytes of the 5-byte frame header and drop.
            let _ = w.inner.write_all(&[0x10, 0x00]);
        });
        let err = receive(b, &trunc_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap_err();
        t.join().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        // 7. Root-level unread "." suppresses ALL mirror deletions on the receiver.
        struct RootUnreadSource;
        impl Source for RootUnreadSource {
            fn index(&self) -> io::Result<Index> {
                let mut idx = Index::default();
                idx.unread.insert(".".into(), "reading folder: EIO".into());
                Ok(idx)
            }
            fn read(&self, _path: &str, _offset: u64, _len: usize) -> io::Result<Vec<u8>> {
                unreachable!()
            }
        }
        let mirror_dst = dir.join("mirror-dst");
        fs::create_dir_all(&mirror_dst).unwrap();
        fs::write(mirror_dst.join("intact.pdf"), b"keep-me").unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || send(a, &RootUnreadSource, Options::default()).unwrap());
        let r_mirror = receive(b, &mirror_dst, Options { delete_extra: true, request_sender_summary: true, ..Options::default() }).unwrap();
        let _ = t.join().unwrap();
        assert_eq!(r_mirror.removed, 0, "root unread '.' must suppress all mirror deletions");
        assert!(mirror_dst.join("intact.pdf").exists(), "intact peer file must survive root unread error");
        assert_eq!(r_mirror.skipped.unread.count, 1);

        // 8. Continuation frame with more=1 and count=0 is rejected immediately (no infinite loop).
        let loop_dst = dir.join("loop-dst");
        let (a, b) = UnixStream::pair().unwrap();
        let t = thread::spawn(move || {
            let mut w = Wire::new(a);
            let _ = w.read_frame().unwrap();
            let mut ack = Vec::with_capacity(5);
            ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
            ack.push(0);
            w.write_frame(TAG_HELLO_ACK, &ack).unwrap();
            w.flush().unwrap();
            let _ = w.read_frame().unwrap();
            // Frame 1: empty skipped + 0 errors, more = true.
            let mut f1 = Vec::new();
            for _ in 0..4 {
                write_skipped_kind(&mut f1, &SkippedKind::default());
            }
            f1.extend_from_slice(&0u32.to_le_bytes());
            write_compressed_batch(&mut w, TAG_SENDER_SUMMARY, true, &f1).unwrap();
            // Continuation frame: 0 errors, more = true -> must be rejected!
            write_compressed_batch(&mut w, TAG_SENDER_SUMMARY, true, &0u32.to_le_bytes()).unwrap();
            let _ = w.flush();
        });
        let err = receive(b, &loop_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap_err();
        let _ = t.join();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("empty SenderSummary continuation"), "{err}");

        // 9. LazyFolderSource streams brand-new files without eager pre-hashing.
        let lazy_src = dir.join("lazy-src");
        let lazy_dst = dir.join("lazy-dst");
        fs::create_dir_all(&lazy_src).unwrap();
        fs::write(lazy_src.join("big.bin"), vec![0x42u8; 64 * 1024]).unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let ls = lazy_src.clone();
        let t = thread::spawn(move || send(a, &LazyFolderSource::new(ls), Options::default()).unwrap());
        let r_lazy = receive(b, &lazy_dst, Options { request_sender_summary: true, ..Options::default() }).unwrap();
        let _ = t.join().unwrap();
        assert!(r_lazy.sender_confirmed);
        assert_eq!(fs::read(lazy_dst.join("big.bin")).unwrap().len(), 64 * 1024);

        let _ = fs::remove_dir_all(&dir);
    }
}
