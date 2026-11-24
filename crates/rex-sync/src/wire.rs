//! Framed wire session over any `Read + Write` stream (`UnixStream`, TCP,
//! SSH `exec_io`, QUIC stream): version handshake, tree-hash fast path,
//! compressed manifest exchange, append-probe / 2-level super-chunk
//! negotiation, pipelined streaming recipes with atomic `.olo-sync-part`
//! commits and full BLAKE3 verification, and strict path confinement to `root`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::chunk::{self, ChunkId, ChunkRef, StreamDigest};
use crate::index::{Entry, EntryKind, Filter, Index, PART_SUFFIX, dir_mode, validate_rel_path};
use crate::recipe::{self, AppendProbe, DataCodec, MAX_DATA_PIECE_LEN, Piece, Plan, REF_PREFIX_MAX, RecipeKind, SuperChunk};

/// Wire protocol version exchanged in the opening handshake.
pub const WIRE_VERSION: u32 = 1;

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
const MAX_ITEMS: usize = 4_000_000;
/// The most chunks an append's tail is said in (about 4 GiB of it): more,
/// and the file is built beside its place instead.
const MAX_TAIL_CHUNKS: usize = 256 << 10;

const FLAG_DELETE_EXTRA: u8 = 1 << 0;
const FLAG_KEEP_NEWER: u8 = 1 << 1;

// Frame tags.
const TAG_HELLO: u8 = 1;
const TAG_HELLO_ACK: u8 = 2;
const TAG_MANIFEST: u8 = 3;
const TAG_NEED_CHUNKS: u8 = 4;
const TAG_CHUNK_INFO: u8 = 5;
const TAG_NEED_LEAF_CHUNKS: u8 = 6;
const TAG_LEAF_CHUNKS: u8 = 7;
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

/// What the sender can offer: an index of entries and random-access reads of
/// file slices.
pub trait Source {
    fn index(&self) -> io::Result<Index>;
    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>>;
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
        let prev = self.cache_path.as_deref().and_then(|p| Index::load(p).ok());
        let idx = Index::scan(&self.root, &self.filter, prev.as_ref())?;
        if let Some(cache) = &self.cache_path {
            idx.save(cache)?;
        }
        Ok(idx)
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
        let mut entries = BTreeMap::new();
        let mut buf = Vec::new();
        let mut cache = self.cache.lock().unwrap();
        for item in &self.items {
            let bytes = (self.provider)(&item.path)?;
            let digest = chunk::hash_and_chunk_in(&mut buf, &bytes[..])?;
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
        Ok(Index { entries, unread: BTreeMap::new() })
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

/// Options governing a sync session.
#[derive(Debug, Clone, Default)]
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
}

impl Report {
    /// Total bytes transmitted across the wire in both directions.
    pub fn wire_bytes(&self) -> u64 {
        self.bytes_sent + self.bytes_received
    }
}

/// Sender entry point: sync everything from `source` to the peer on `conn`.
pub fn send<S: Read + Write>(conn: S, source: &dyn Source, options: Options) -> io::Result<Report> {
    let mut raw_index = source.index()?;
    filter_index(&mut raw_index, &options.filter);
    let sender_index = raw_index;
    let sender_tree_hash = sender_index.tree_hash();
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
    let effective_delete_extra = hello.flags & FLAG_DELETE_EXTRA != 0;

    if hello.tree_hash == sender_tree_hash {
        let mut ack = Vec::with_capacity(5);
        ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        ack.push(1); // in_sync = true
        wire.write_frame(TAG_HELLO_ACK, &ack)?;
        wire.flush()?;
        return Ok(Report { same: sender_index.file_count(), bytes_sent: wire.bytes_sent, bytes_received: wire.bytes_received, ..Report::default() });
    }

    let mut ack = Vec::with_capacity(5);
    ack.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    ack.push(0); // in_sync = false
    wire.write_frame(TAG_HELLO_ACK, &ack)?;
    wire.flush()?;

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

    // Index receiver files by BLAKE3 for O(1) CopyFile detection.
    let mut recv_by_hash: HashMap<[u8; 32], String> = HashMap::new();
    for entry in recv_manifest.values() {
        if entry.kind == EntryKind::File {
            recv_by_hash.entry(entry.hash).or_insert_with(|| entry.path.clone());
        }
    }

    // 3. Classify every sender file and gather chunk requests for differing files.
    let mut same_count = 0u64;
    let mut left_newer_count = 0u64;
    let mut set_meta_files: Vec<&Entry> = Vec::new();
    let mut copy_files: Vec<(&Entry, String)> = Vec::new();
    let mut new_files: Vec<&Entry> = Vec::new();
    let mut delta_files: Vec<&Entry> = Vec::new();
    let mut chunk_reqs: Vec<ChunkReq> = Vec::new();

    for entry in sender_index.entries.values() {
        if entry.kind != EntryKind::File {
            continue;
        }
        if let Some(their) = recv_manifest.get(&entry.path)
            && their.kind == EntryKind::File
        {
            if their.hash == entry.hash {
                same_count += 1;
                if their.mode != entry.mode {
                    set_meta_files.push(entry);
                }
                continue;
            }
            if effective_keep_newer && their.mtime_ns > entry.mtime_ns {
                left_newer_count += 1;
                continue;
            }
        }
        if let Some(from) = recv_by_hash.get(&entry.hash) {
            copy_files.push((entry, from.clone()));
            continue;
        }
        if let Some(their) = recv_manifest.get(&entry.path)
            && their.kind == EntryKind::File
            && their.size > 0
            && entry.size > 0
        {
            let probe = if entry.size > their.size { recipe::append_probe_for(entry, their.size, |off, len| source.read(&entry.path, off, len))? } else { None };
            chunk_reqs.push(match probe {
                Some(p) => ChunkReq::AppendOrSuper { path: entry.path.clone(), probe: p },
                None => ChunkReq::SuperOrLeaf { path: entry.path.clone() },
            });
            delta_files.push(entry);
            continue;
        }
        new_files.push(entry);
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
        for entry in &delta_files {
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

    // 5. Pipelined transfer phase: dirs -> symlinks -> meta -> copies -> recipes -> removes -> done.
    for entry in sender_index.entries.values() {
        if entry.kind == EntryKind::Dir {
            let needs_dir = !matches!(
                recv_manifest.get(&entry.path),
                Some(their) if their.kind == EntryKind::Dir && dir_mode(their.mode) == dir_mode(entry.mode)
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

    for entry in set_meta_files {
        let mut buf = Vec::with_capacity(entry.path.len() + 16);
        write_str(&mut buf, &entry.path);
        buf.extend_from_slice(&entry.mode.to_le_bytes());
        buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
        wire.write_frame(TAG_SET_META, &buf)?;
    }

    for (entry, from) in copy_files {
        let mut buf = Vec::with_capacity(entry.path.len() + from.len() + 64);
        write_str(&mut buf, &entry.path);
        write_str(&mut buf, &from);
        buf.extend_from_slice(&entry.mode.to_le_bytes());
        buf.extend_from_slice(&entry.mtime_ns.to_le_bytes());
        buf.extend_from_slice(&entry.size.to_le_bytes());
        buf.extend_from_slice(&entry.hash);
        wire.write_frame(TAG_COPY_FILE, &buf)?;
    }

    let mut data_bytes_raw = 0u64;
    let mut data_bytes_compressed = 0u64;

    for entry in delta_files {
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
        if let Plan::Build { kind, ops } = plan {
            stream_file_plan(&mut wire, source, entry, kind, &ops, &mut data_bytes_raw, &mut data_bytes_compressed)?;
        }
    }

    for entry in new_files {
        let Plan::Build { kind, ops } = recipe::plan_new(entry.size) else {
            unreachable!();
        };
        stream_file_plan(&mut wire, source, entry, kind, &ops, &mut data_bytes_raw, &mut data_bytes_compressed)?;
    }

    if effective_delete_extra {
        // Remove extra paths in reverse order so children are removed before parents.
        // (Nor what the sender couldn't read, nor anything in a folder it
        // couldn't: what it holds isn't known.)
        let unknown = |p: &str| sender_index.unread.keys().any(|u| p == u || p.strip_prefix(u.as_str()).is_some_and(|rest| rest.starts_with('/')));
        let extras: Vec<&String> = recv_manifest.keys().rev().filter(|p| !sender_index.entries.contains_key(*p) && !unknown(p)).collect();
        for path in extras {
            let mut buf = Vec::with_capacity(path.len() + 4);
            write_str(&mut buf, path);
            wire.write_frame(TAG_REMOVE, &buf)?;
        }
    }

    let mut done_buf = Vec::with_capacity(32);
    done_buf.extend_from_slice(&same_count.to_le_bytes());
    done_buf.extend_from_slice(&left_newer_count.to_le_bytes());
    done_buf.extend_from_slice(&data_bytes_raw.to_le_bytes());
    done_buf.extend_from_slice(&data_bytes_compressed.to_le_bytes());
    wire.write_frame(TAG_DONE, &done_buf)?;
    wire.flush()?;

    // 6. Read final report from receiver.
    let (tag, payload) = wire.read_frame()?;
    if tag != TAG_REPORT {
        return Err(invalid_data(format!("expected Report, got tag {tag}")));
    }
    let mut report = decode_report(&payload)?;
    report.bytes_sent = wire.bytes_sent;
    report.bytes_received = wire.bytes_received;
    Ok(report)
}

/// Receiver entry point: accept updates from the peer on `conn` into `root`.
///
/// Nothing is ever written outside `root`: paths that are absolute, contain
/// `..`, or would traverse a symlink are refused and recorded in
/// [`Report::errors`].
pub fn receive<S: Read + Write>(conn: S, root: &Path, options: Options) -> io::Result<Report> {
    fs::create_dir_all(root)?;
    let prev = options.cache_path.as_deref().and_then(|p| Index::load(p).ok());
    let mut recv_index = Index::scan(root, &options.filter, prev.as_ref())?;
    let recv_tree_hash = recv_index.tree_hash();
    let mut wire = Wire::new(conn);

    // 1. Handshake: send Hello, read HelloAck.
    let mut flags = 0u8;
    if options.delete_extra {
        flags |= FLAG_DELETE_EXTRA;
    }
    if options.keep_newer {
        flags |= FLAG_KEEP_NEWER;
    }
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
    if in_sync > 1 {
        return Err(invalid_data("invalid in_sync flag in HelloAck"));
    }
    if in_sync == 1 {
        if let Some(cache) = &options.cache_path {
            recv_index.save(cache)?;
        }
        return Ok(Report { same: recv_index.file_count(), bytes_sent: wire.bytes_sent, bytes_received: wire.bytes_received, ..Report::default() });
    }

    // 2. Send manifest (no chunk lists).
    send_manifest(&mut wire, &recv_index)?;
    wire.flush()?;

    // 3. Process negotiation and transfer frames until TAG_DONE.
    let mut report = Report::default();
    let mut super_cache: HashMap<String, Vec<SuperChunk>> = HashMap::new();
    let mut active_file: Option<ActiveFile> = None;
    let mut copy_buf = vec![0u8; 64 << 10];

    loop {
        let (tag, payload) = wire.read_frame()?;
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
                if let Err(reason) = apply_mkdir(root, &path, mode, mtime_ns, &mut recv_index) {
                    report.errors.push(PathError { path, reason });
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
                if let Err(reason) = apply_symlink(root, &path, &target, mtime_ns, options.delete_extra, &mut recv_index) {
                    report.errors.push(PathError { path, reason });
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
                if let Err(reason) = apply_set_meta(root, &path, mode, mtime_ns, &mut recv_index) {
                    report.errors.push(PathError { path, reason });
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
                match apply_copy_file(root, &path, &from, mode, mtime_ns, size, hash, &options, &mut recv_index, &mut copy_buf) {
                    Ok(CopyOutcome::Copied) => report.copied += 1,
                    Ok(CopyOutcome::LeftNewer) => report.left_newer += 1,
                    Err(reason) => report.errors.push(PathError { path, reason }),
                }
            }
            TAG_FILE_START => {
                if active_file.is_some() {
                    return Err(invalid_data("nested FileStart"));
                }
                let start = decode_file_start(&payload)?;
                active_file = Some(begin_file(root, start, &options, &recv_index, &mut report));
            }
            TAG_FILE_PIECE => {
                let Some(af) = active_file.as_mut() else {
                    return Err(invalid_data("FilePiece without FileStart"));
                };
                let piece = decode_file_piece(&payload, &af.basis_paths)?;
                apply_active_piece(root, af, &piece, &mut copy_buf);
            }
            TAG_FILE_END => {
                if !payload.is_empty() {
                    return Err(invalid_data("trailing bytes in FileEnd"));
                }
                let Some(af) = active_file.take() else {
                    return Err(invalid_data("FileEnd without FileStart"));
                };
                finish_file(root, af, &mut recv_index, &mut report);
            }
            TAG_REMOVE => {
                if active_file.is_some() {
                    return Err(invalid_data("Remove while file transfer active"));
                }
                let mut cur = Cursor::new(&payload);
                let path = cur.read_str(MAX_PATH_LEN)?;
                cur.expect_eof()?;
                if !options.delete_extra {
                    report.errors.push(PathError { path, reason: "remove refused: delete_extra is false".into() });
                } else {
                    match apply_remove(root, &path, &mut recv_index) {
                        Ok(true) => report.removed += 1,
                        Ok(false) => {}
                        Err(reason) => report.errors.push(PathError { path, reason }),
                    }
                }
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

    if let Some(cache) = &options.cache_path {
        recv_index.save(cache)?;
    }

    let report_payload = encode_report(&report);
    wire.write_frame(TAG_REPORT, &report_payload)?;
    wire.flush()?;

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
    hash: [u8; 32],
    basis_paths: Vec<String>,
    /// An append's: the size the receiver's file has, and the sender's
    /// chunks from the start of that file's last one to the end.
    tail: Option<(u64, Vec<ChunkRef>)>,
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
        writer: BufWriter<fs::File>,
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
    expected_hash: [u8; 32],
    basis_paths: Vec<String>,
    state: ActiveState,
}

fn begin_file(root: &Path, start: FileStartMsg, options: &Options, recv_index: &Index, report: &mut Report) -> ActiveFile {
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
    for bp in &start.basis_paths {
        if let Err(e) = validate_rel_path(bp) {
            report.errors.push(PathError { path: start.path.clone(), reason: format!("refused basis path '{bp}': {e}") });
            return make_skipped(None, true);
        }
    }
    if let Err(reason) = ensure_safe_parent_dirs(root, &start.path) {
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

    // (A folder where the file goes is taken away only by a mirror: by
    // `delete_extra`, which the receiver chose.)
    if !options.delete_extra && fs::symlink_metadata(root.join(&start.path)).is_ok_and(|m| m.is_dir()) {
        report.errors.push(PathError { path: start.path.clone(), reason: "a folder is there: left as it is (a mirror, delete_extra, replaces it)".into() });
        return make_skipped(None, true);
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
            state: ActiveState::Writing { part_path, writer: BufWriter::with_capacity(64 << 10, file), digest: Box::new(StreamDigest::new()), written: 0 },
        },
        Err(e) => {
            report.errors.push(PathError { path: start.path.clone(), reason: format!("create temp file failed: {e}") });
            make_skipped(None, true)
        }
    }
}

/// An append to be written in place, where the receiver's file is still
/// what its index says (`base` long, unchanged since it was read) and
/// isn't a link: opened at its end, its last chunk read back into the
/// digest of the tail.
fn append_in_place(root: &Path, path: &str, base: u64, expected: &[ChunkRef], recv_index: &Index) -> Option<ActiveState> {
    let entry = recv_index.entries.get(path).filter(|e| e.kind == EntryKind::File && e.size == base && !e.chunks.is_empty())?;
    let (last, kept) = entry.chunks.split_last()?;
    let last_start: u64 = kept.iter().map(|c| c.len as u64).sum();
    if last_start + last.len as u64 != base {
        return None;
    }
    use std::os::unix::fs::MetadataExt;
    let dst = root.join(path);
    let meta = fs::symlink_metadata(&dst).ok().filter(|m| m.file_type().is_file())?;
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

fn apply_active_piece(root: &Path, af: &mut ActiveFile, piece: &Piece, copy_buf: &mut [u8]) {
    if let ActiveState::Appending { file, base, digest, written, based, .. } = &mut af.state {
        let res: Result<(), String> = (|| {
            match piece {
                // (What the file has already: the one copy an append starts with.)
                Piece::Copy { from, offset: 0, len } if !*based && *from == af.path && *len == *base => *based = true,
                Piece::Copy { .. } => return Err("an append copied what it isn't".into()),
                Piece::Data { raw_len, codec, bytes } => {
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
            Piece::Copy { from, offset, len } => {
                let mut f = open_no_symlink_components(root, from).map_err(|e| format!("open basis '{from}': {e}"))?;
                f.seek(SeekFrom::Start(*offset)).map_err(|e| format!("seek basis '{from}': {e}"))?;
                let mut rem = *len;
                while rem > 0 {
                    let take = (rem as usize).min(copy_buf.len());
                    f.read_exact(&mut copy_buf[..take]).map_err(|e| format!("read basis '{from}': {e}"))?;
                    writer.write_all(&copy_buf[..take]).map_err(|e| format!("write temp: {e}"))?;
                    digest.update(&copy_buf[..take]);
                    *written = written.checked_add(take as u64).ok_or_else(|| "file size overflow".to_string())?;
                    if *written > af.expected_size {
                        return Err("recipe exceeded expected file size".into());
                    }
                    rem -= take as u64;
                }
            }
            Piece::Data { raw_len, codec, bytes } => {
                let raw = recipe::decode_data_piece(*raw_len, codec, bytes, |from, off, len| {
                    let mut f = open_no_symlink_components(root, from)?;
                    f.seek(SeekFrom::Start(off))?;
                    let mut buf = vec![0u8; len];
                    f.read_exact(&mut buf)?;
                    Ok(buf)
                })
                .map_err(|e| format!("decode piece: {e}"))?;
                writer.write_all(&raw).map_err(|e| format!("write temp: {e}"))?;
                digest.update(&raw);
                *written = written.checked_add(raw.len() as u64).ok_or_else(|| "file size overflow".to_string())?;
                if *written > af.expected_size {
                    return Err("recipe exceeded expected file size".into());
                }
            }
        }
        Ok(())
    })();

    if let Err(err) = res {
        let _ = fs::remove_file(&*part_path);
        af.state = ActiveState::Skipped { already_recorded: false, error: Some(err) };
    }
}

fn finish_file(root: &Path, af: ActiveFile, recv_index: &mut Index, report: &mut Report) {
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
                    use std::os::unix::fs::MetadataExt;
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
                                hash: af.expected_hash,
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
        ActiveState::Writing { part_path, mut writer, digest, written } => {
            let res: Result<(), String> = (|| {
                writer.flush().map_err(|e| format!("flush temp: {e}"))?;
                let file = writer.into_inner().map_err(|e| format!("flush temp: {}", e.error()))?;
                let computed = digest.finalize();
                if written != af.expected_size || computed.size != af.expected_size {
                    return Err(format!("size mismatch: wrote {}, expected {}", computed.size, af.expected_size));
                }
                if computed.hash != af.expected_hash {
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
                    if !af.replace_dirs {
                        return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
                    }
                    fs::remove_dir_all(&dst).map_err(|e| format!("replace dir: {e}"))?;
                }
                fs::rename(&part_path, &dst).map_err(|e| format!("rename into place: {e}"))?;

                let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat dst: {e}"))?;
                use std::os::unix::fs::MetadataExt;
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

fn part_path_for(root: &Path, rel: &str) -> PathBuf {
    let dst = root.join(rel);
    let mut os = dst.into_os_string();
    os.push(PART_SUFFIX);
    PathBuf::from(os)
}

fn ensure_safe_parent_dirs(root: &Path, rel: &str) -> Result<(), String> {
    validate_rel_path(rel).map_err(|e| format!("refused path: {e}"))?;
    let mut cur = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for &seg in &parts[..parts.len() - 1] {
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(format!("refused path: traverses symlink at '{seg}'"));
                }
                if !meta.file_type().is_dir() {
                    fs::remove_file(&cur).map_err(|e| format!("replace non-dir '{seg}': {e}"))?;
                    fs::create_dir(&cur).map_err(|e| format!("mkdir '{seg}': {e}"))?;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&cur).map_err(|e| format!("mkdir '{seg}': {e}"))?;
            }
            Err(e) => return Err(format!("stat '{seg}': {e}")),
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

fn apply_mkdir(root: &Path, path: &str, mode: u32, _mtime_ns: i64, recv_index: &mut Index) -> Result<(), String> {
    ensure_safe_parent_dirs(root, path)?;
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
    let clean_mode = dir_mode(mode);
    fs::set_permissions(&dst, fs::Permissions::from_mode(clean_mode)).map_err(|e| format!("chmod dir: {e}"))?;
    let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat dir: {e}"))?;
    use std::os::unix::fs::MetadataExt;
    recv_index.entries.insert(
        path.to_owned(),
        Entry {
            path: path.to_owned(),
            kind: EntryKind::Dir,
            size: 0,
            mtime_ns: meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec()),
            ctime_ns: meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()),
            ino: meta.ino(),
            mode: clean_mode,
            hash: [0u8; 32],
            chunks: Vec::new(),
        },
    );
    Ok(())
}

fn apply_symlink(root: &Path, path: &str, target: &str, _mtime_ns: i64, replace_dirs: bool, recv_index: &mut Index) -> Result<(), String> {
    ensure_safe_parent_dirs(root, path)?;
    let dst = root.join(path);
    if !replace_dirs && fs::symlink_metadata(&dst).is_ok_and(|m| m.is_dir()) {
        return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
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
    use std::os::unix::fs::MetadataExt;
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

fn apply_set_meta(root: &Path, path: &str, mode: u32, mtime_ns: i64, recv_index: &mut Index) -> Result<(), String> {
    check_no_symlink_ancestors(root, path)?;
    let dst = root.join(path);
    let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err("refused SetMeta on symlink".into());
    }
    let clean_mode = mode & 0o777;
    fs::set_permissions(&dst, fs::Permissions::from_mode(clean_mode)).map_err(|e| format!("chmod: {e}"))?;
    if meta.file_type().is_file()
        && let Some(st) = ns_to_system_time(mtime_ns)
        && let Ok(f) = fs::File::open(&dst)
    {
        let _ = f.set_modified(st);
    }
    if let Some(entry) = recv_index.entries.get_mut(path) {
        let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat: {e}"))?;
        use std::os::unix::fs::MetadataExt;
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
    recv_index: &mut Index,
    copy_buf: &mut [u8],
) -> Result<CopyOutcome, String> {
    validate_rel_path(path).map_err(|e| format!("refused path: {e}"))?;
    validate_rel_path(from).map_err(|e| format!("refused basis path '{from}': {e}"))?;
    if options.keep_newer
        && let Some(existing) = recv_index.entries.get(path)
        && existing.kind == EntryKind::File
        && existing.mtime_ns > mtime_ns
    {
        return Ok(CopyOutcome::LeftNewer);
    }
    ensure_safe_parent_dirs(root, path)?;
    if !options.delete_extra && fs::symlink_metadata(root.join(path)).is_ok_and(|m| m.is_dir()) {
        return Err("a folder is there: left as it is (a mirror, delete_extra, replaces it)".into());
    }

    let mut src = open_no_symlink_components(root, from).map_err(|e| format!("open '{from}': {e}"))?;
    let part = part_path_for(root, path);
    let mut dst_file = create_part(&part).map_err(|e| format!("create temp: {e}"))?;

    let reuse_chunks = recv_index.entries.get(from).filter(|e| e.kind == EntryKind::File && e.hash == expected_hash && e.size == size).map(|e| e.chunks.clone());

    let res: Result<Vec<ChunkRef>, String> = (|| {
        let mut hasher = blake3::Hasher::new();
        let mut sd = if reuse_chunks.is_none() { Some(StreamDigest::new()) } else { None };
        let mut copied = 0u64;
        loop {
            let n = src.read(copy_buf).map_err(|e| format!("read '{from}': {e}"))?;
            if n == 0 {
                break;
            }
            dst_file.write_all(&copy_buf[..n]).map_err(|e| format!("write temp: {e}"))?;
            hasher.update(&copy_buf[..n]);
            if let Some(d) = sd.as_mut() {
                d.update(&copy_buf[..n]);
            }
            copied += n as u64;
        }
        dst_file.flush().map_err(|e| format!("flush temp: {e}"))?;
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
        Ok(match reuse_chunks {
            Some(c) => c,
            None => sd.unwrap().finalize().chunks,
        })
    })();

    match res {
        Ok(chunks) => {
            let dst = root.join(path);
            let meta = fs::symlink_metadata(&dst).map_err(|e| format!("stat dst: {e}"))?;
            use std::os::unix::fs::MetadataExt;
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

fn apply_remove(root: &Path, path: &str, recv_index: &mut Index) -> Result<bool, String> {
    check_no_symlink_ancestors(root, path)?;
    let dst = root.join(path);
    match fs::symlink_metadata(&dst) {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                match fs::remove_dir(&dst) {
                    Ok(()) => {
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
            recv_index.entries.remove(path);
            Ok(false)
        }
        Err(e) => Err(format!("stat for remove: {e}")),
    }
}

fn ns_to_system_time(ns: i64) -> Option<SystemTime> {
    if ns >= 0 { UNIX_EPOCH.checked_add(Duration::from_nanos(ns as u64)) } else { UNIX_EPOCH.checked_sub(Duration::from_nanos(ns.unsigned_abs())) }
}

fn filter_index(index: &mut Index, filter: &Filter) {
    if filter.recursive && filter.exclude.is_empty() && filter.include.is_empty() {
        return;
    }
    index.entries.retain(|path, entry| {
        if !filter.recursive && path.contains('/') {
            return false;
        }
        match entry.kind {
            EntryKind::Dir => filter.allow_dir(path),
            EntryKind::File | EntryKind::Symlink(_) => {
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
    });
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
        let entry_sz = 4 + entry.path.len() + 1 + 8 + 8 + 4 + 32;
        if !chunk_entries.is_empty() && est_bytes + entry_sz > MAX_MANIFEST_CHUNK_RAW {
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
    let mut raw = Vec::with_capacity(4 + reqs.len() * 64);
    let mut batch_count = 0u32;
    raw.extend_from_slice(&0u32.to_le_bytes());

    for (idx, req) in reqs.iter().enumerate() {
        let is_last = idx + 1 == reqs.len();
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
        if raw.len() >= MAX_MANIFEST_CHUNK_RAW || is_last {
            raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
            write_compressed_batch(wire, TAG_NEED_CHUNKS, !is_last, &raw)?;
            raw.clear();
            raw.extend_from_slice(&0u32.to_le_bytes());
            batch_count = 0;
        }
    }
    Ok(())
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
    let mut raw = Vec::new();
    let mut batch_count = 0u32;
    raw.extend_from_slice(&0u32.to_le_bytes());

    for (idx, req) in reqs.iter().enumerate() {
        let is_last = idx + 1 == reqs.len();
        let path = match req {
            ChunkReq::AppendOrSuper { path, .. } | ChunkReq::SuperOrLeaf { path } => path,
        };
        if let Some(entry) = recv_index.entries.get(path)
            && entry.kind == EntryKind::File
        {
            let confirmed = match req {
                ChunkReq::AppendOrSuper { probe, .. } => recipe::verify_append_probe(entry, probe),
                ChunkReq::SuperOrLeaf { .. } => false,
            };
            if confirmed {
                raw.push(0); // AppendConfirmed
                write_str(&mut raw, path);
            } else if entry.chunks.len() <= DIRECT_LEAF_CHUNK_LIMIT {
                raw.push(1); // LeafChunks
                write_str(&mut raw, path);
                raw.extend_from_slice(&(entry.chunks.len() as u32).to_le_bytes());
                for c in &entry.chunks {
                    raw.extend_from_slice(&c.len.to_le_bytes());
                    raw.extend_from_slice(&c.id);
                }
            } else {
                let supers = recipe::super_chunks(&entry.chunks);
                raw.push(2); // SuperChunks
                write_str(&mut raw, path);
                raw.extend_from_slice(&(supers.len() as u32).to_le_bytes());
                for sc in &supers {
                    raw.extend_from_slice(&sc.start_chunk.to_le_bytes());
                    raw.extend_from_slice(&sc.chunk_count.to_le_bytes());
                    raw.extend_from_slice(&sc.offset.to_le_bytes());
                    raw.extend_from_slice(&sc.len.to_le_bytes());
                    raw.extend_from_slice(&sc.id);
                }
                super_cache.insert(path.clone(), supers);
            }
            batch_count += 1;
        }

        if raw.len() >= MAX_MANIFEST_CHUNK_RAW || is_last {
            raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
            write_compressed_batch(wire, TAG_CHUNK_INFO, !is_last, &raw)?;
            raw.clear();
            raw.extend_from_slice(&0u32.to_le_bytes());
            batch_count = 0;
        }
    }
    Ok(())
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
                out.insert(path, ChunkResp::SuperChunks(supers));
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

    for (idx, (path, indices)) in reqs.iter().enumerate() {
        let is_last = idx + 1 == reqs.len();
        write_str(&mut raw, path);
        raw.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        for &si in indices {
            raw.extend_from_slice(&si.to_le_bytes());
        }
        batch_count += 1;
        if raw.len() >= MAX_MANIFEST_CHUNK_RAW || is_last {
            raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
            write_compressed_batch(wire, TAG_NEED_LEAF_CHUNKS, !is_last, &raw)?;
            raw.clear();
            raw.extend_from_slice(&0u32.to_le_bytes());
            batch_count = 0;
        }
    }
    Ok(())
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

    for (idx, (path, si, chunks)) in flat.iter().enumerate() {
        let is_last = idx + 1 == flat.len();
        write_str(&mut raw, path);
        raw.extend_from_slice(&si.to_le_bytes());
        raw.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for c in *chunks {
            raw.extend_from_slice(&c.len.to_le_bytes());
            raw.extend_from_slice(&c.id);
        }
        batch_count += 1;
        if raw.len() >= MAX_MANIFEST_CHUNK_RAW || is_last {
            raw[0..4].copy_from_slice(&batch_count.to_le_bytes());
            write_compressed_batch(wire, TAG_LEAF_CHUNKS, !is_last, &raw)?;
            raw.clear();
            raw.extend_from_slice(&0u32.to_le_bytes());
            batch_count = 0;
        }
    }
    Ok(())
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

fn stream_file_plan<S: Read + Write>(
    wire: &mut Wire<S>,
    source: &dyn Source,
    entry: &Entry,
    kind: RecipeKind,
    ops: &[recipe::PlanOp],
    data_bytes_raw: &mut u64,
    data_bytes_compressed: &mut u64,
) -> io::Result<()> {
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
    start_buf.extend_from_slice(&entry.hash);
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

    // Stream each Piece directly onto the wire (bounded at a few MiB in memory).
    let mut piece_buf = Vec::new();
    recipe::emit_pieces(ops, &mut |off, len| source.read(&entry.path, off, len), &mut |piece| {
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
        wire.write_frame(TAG_FILE_PIECE, &piece_buf)
    })?;

    wire.write_frame(TAG_FILE_END, &[])
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
    let hash = cur.read_array::<32>()?;
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

fn decode_file_piece(payload: &[u8], basis_paths: &[String]) -> io::Result<Piece> {
    let mut cur = Cursor::new(payload);
    match cur.read_u8()? {
        0 => {
            let bidx = cur.read_u16()? as usize;
            let from = basis_paths.get(bidx).ok_or_else(|| invalid_data("invalid basis index in Copy piece"))?.clone();
            let offset = cur.read_u64()?;
            let len = cur.read_u64()?;
            cur.expect_eof()?;
            Ok(Piece::Copy { from, offset, len })
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
            Ok(Piece::Data { raw_len, codec, bytes })
        }
        t => Err(invalid_data(format!("unknown piece tag {t}"))),
    }
}

fn encode_report(r: &Report) -> Vec<u8> {
    let mut out = Vec::with_capacity(80 + r.errors.len() * 32);
    out.extend_from_slice(&r.same.to_le_bytes());
    out.extend_from_slice(&r.copied.to_le_bytes());
    out.extend_from_slice(&r.appended.to_le_bytes());
    out.extend_from_slice(&r.patched.to_le_bytes());
    out.extend_from_slice(&r.new.to_le_bytes());
    out.extend_from_slice(&r.removed.to_le_bytes());
    out.extend_from_slice(&r.left_newer.to_le_bytes());
    out.extend_from_slice(&r.data_bytes_raw.to_le_bytes());
    out.extend_from_slice(&r.data_bytes_compressed.to_le_bytes());
    out.extend_from_slice(&(r.errors.len() as u32).to_le_bytes());
    for err in &r.errors {
        write_str(&mut out, &err.path);
        write_str(&mut out, &err.reason);
    }
    out
}

fn decode_report(payload: &[u8]) -> io::Result<Report> {
    let mut cur = Cursor::new(payload);
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
    if err_count > 100_000 {
        return Err(invalid_data("too many errors in report"));
    }
    let mut errors = Vec::with_capacity(err_count.min(1024));
    for _ in 0..err_count {
        let path = cur.read_str(MAX_PATH_LEN)?;
        let reason = cur.read_str(MAX_PATH_LEN)?;
        errors.push(PathError { path, reason });
    }
    cur.expect_eof()?;
    Ok(Report { same, copied, appended, patched, new, removed, left_newer, bytes_sent: 0, bytes_received: 0, data_bytes_raw, data_bytes_compressed, errors })
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
            Ok(Index { entries, unread: BTreeMap::new() })
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
    /// so) is its owner's to write where it goes, so what is sent into it
    /// arrives; and the two are in step after, in one round trip.
    #[test]
    fn a_read_only_folder_takes_what_is_sent_into_it() {
        let (src, dst, caches) = (scratch_dir("ro-src"), scratch_dir("ro-dst"), scratch_dir("ro-cache"));
        fs::create_dir_all(src.join("module")).unwrap();
        fs::write(src.join("module/go.mod"), b"module example.com/m\n").unwrap();
        fs::set_permissions(src.join("module"), fs::Permissions::from_mode(0o555)).unwrap();
        let (_, first) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), Options::default());
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert_eq!(fs::read(dst.join("module/go.mod")).unwrap(), b"module example.com/m\n");
        assert_eq!(fs::metadata(dst.join("module")).unwrap().permissions().mode() & 0o777, 0o755);
        let (again, _) = sync_folders(&src, &dst, Some(caches.join("s")), Some(caches.join("r")), Options::default());
        assert!(again.wire_bytes() < 128, "{} bytes", again.wire_bytes());
        fs::set_permissions(src.join("module"), fs::Permissions::from_mode(0o755)).unwrap();
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
        symlink(elsewhere.join("victim"), dst.join(format!("file.txt{PART_SUFFIX}"))).unwrap();
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
            expected_hash: [0; 32],
            basis_paths: vec!["log".into()],
            state,
        };
        let mut buf = vec![0u8; 64 << 10];
        apply_active_piece(&root, &mut af, &Piece::Copy { from: "log".into(), offset: 0, len: old.len() as u64 }, &mut buf);
        apply_active_piece(&root, &mut af, &Piece::Data { raw_len: 5, codec: DataCodec::Raw, bytes: b"extra".to_vec() }, &mut buf);
        let (mut index, mut report) = (index, Report::default());
        finish_file(&root, af, &mut index, &mut report);
        assert!(report.errors.iter().any(|e| e.reason.contains("aren't the sender's")), "{:?}", report.errors);
        assert_eq!(fs::read(root.join("log")).unwrap(), old);
        fs::remove_dir_all(root).unwrap();
    }
}
