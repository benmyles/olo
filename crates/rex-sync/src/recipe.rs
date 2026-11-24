//! Cheapest recipe per file: `Same`, `CopyFile` (moves/copies), `Append`
//! (verified from chunk lists + at most 64 KiB tail check, compressed against
//! the preceding up-to-64 KiB via zstd's `ref_prefix`), `Patch` (chunk reuse
//! across the old file with adjacent copies merged), and `New` (streamed in
//! bounded pieces). Uncompressible data goes raw, flagged.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use zstd::zstd_safe::{self, CParameter};

use crate::chunk::{self, ChunkId, ChunkRef};
use crate::index::{Entry, EntryKind, Index, validate_rel_path};

/// Maximum uncompressed size of a single `Data` piece (2 MiB), keeping both
/// sender and receiver memory bounded at a few MiB for arbitrarily large files.
pub const MAX_DATA_PIECE_LEN: usize = 2 << 20;

/// Maximum reference prefix passed to zstd's `ref_prefix` (64 KiB).
pub const REF_PREFIX_MAX: usize = 64 << 10;

/// Default zstd compression level for literal data pieces.
pub const ZSTD_LEVEL: i32 = 3;

/// Maximum literal piece size for which `Patch` attaches a preceding-copy
/// `ref_prefix` (for huge pieces zstd already has a full window of its own).
const PATCH_REF_PREFIX_MAX_DATA: u64 = 256 << 10;

const SUPER_MIN_CHUNKS: usize = 8;
const SUPER_MAX_CHUNKS: usize = 128;
const SUPER_CUT_MASK: u8 = 0x1F; // ~32 chunks (~512 KiB) average per super-chunk

/// Content-defined group of consecutive leaf chunks, used by the wire protocol
/// to narrow down edits in large files without transferring every leaf chunk id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperChunk {
    pub start_chunk: u32,
    pub chunk_count: u32,
    pub offset: u64,
    pub len: u64,
    pub id: ChunkId,
}

/// Compact proof that the sender's first `their_size` bytes match the
/// receiver's file, computed without reading the sender's prefix before the
/// receiver's last chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendProbe {
    pub prefix_chunk_count: u32,
    pub prefix_chunks_hash: [u8; 32],
    pub last_chunk: ChunkRef,
}

/// Which strategy produced a file's recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeKind {
    Append,
    Patch,
    New,
}

/// Reference prefix shared by both sides for `ZstdRefPrefix` compression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPrefix {
    /// Basis file path on the receiver.
    pub from: String,
    /// Byte offset of the prefix in the sender's file.
    pub sender_offset: u64,
    /// Byte offset of the same prefix in the receiver's `from` file.
    pub receiver_offset: u64,
    /// Length of the prefix (`<= REF_PREFIX_MAX`).
    pub len: u32,
}

/// Lightweight, payload-free operation in a [`Plan`], allowing large files to
/// be planned upfront and streamed piece by piece without holding the file in
/// memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOp {
    Copy { from: String, offset: u64, len: u64 },
    Data { offset: u64, len: u64, ref_prefix: Option<RefPrefix> },
}

/// Cheapest plan for bringing a single file on the receiver up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Receiver already has this exact file at this path.
    Same,
    /// Receiver already has this exact file under another path (rename/copy).
    CopyFile { from: String },
    /// Receiver reconstructs the file from a sequence of copy and data ops.
    Build { kind: RecipeKind, ops: Vec<PlanOp> },
}

/// How the payload bytes of a [`Piece::Data`] are encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataCodec {
    /// Raw uncompressed bytes (chosen when zstd does not shrink the payload).
    Raw,
    /// Standard zstd (level 3).
    Zstd,
    /// Zstd (level 3) compressed using bytes `[prefix_offset .. +prefix_len]`
    /// from the receiver's `from` file as `ref_prefix`.
    ZstdRefPrefix { from: String, prefix_offset: u64, prefix_len: u32 },
}

/// One concrete piece of a materialized [`Recipe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Copy `len` bytes starting at `offset` from the receiver's `from` file.
    Copy { from: String, offset: u64, len: u64 },
    /// Literal data piece producing `raw_len` uncompressed bytes.
    Data { raw_len: u32, codec: DataCodec, bytes: Vec<u8> },
}

/// Materialized recipe for a file (primarily for in-memory tests and small
/// files; `wire::send` streams [`Piece`]s directly from [`PlanOp`]s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub kind: RecipeKind,
    pub pieces: Vec<Piece>,
}

/// Deterministic 32-byte BLAKE3 over a slice of [`ChunkRef`]s.
pub fn hash_chunk_refs(chunks: &[ChunkRef]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"olo-rex-sync-chunks-v1");
    h.update(&(chunks.len() as u64).to_le_bytes());
    for c in chunks {
        h.update(&c.len.to_le_bytes());
        h.update(&c.id);
    }
    *h.finalize().as_bytes()
}

/// Group `chunks` into content-defined [`SuperChunk`]s so a mid-file edit or
/// insertion invalidates at most 1–2 super-chunks.
pub fn super_chunks(chunks: &[ChunkRef]) -> Vec<SuperChunk> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut offset = 0u64;
    while i < chunks.len() {
        let start = i;
        let start_off = offset;
        let mut h = blake3::Hasher::new();
        let mut count = 0usize;
        let mut len = 0u64;
        while i < chunks.len() {
            let c = chunks[i];
            h.update(&c.len.to_le_bytes());
            h.update(&c.id);
            len += c.len as u64;
            offset += c.len as u64;
            count += 1;
            i += 1;
            if count >= SUPER_MAX_CHUNKS || (count >= SUPER_MIN_CHUNKS && (c.id[0] & SUPER_CUT_MASK) == 0) {
                break;
            }
        }
        let digest = h.finalize();
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest.as_bytes()[..16]);
        out.push(SuperChunk { start_chunk: start as u32, chunk_count: count as u32, offset: start_off, len, id });
    }
    out
}

/// Build an [`AppendProbe`] for `ours` against a shorter receiver file of
/// `their_size` bytes. Reads only `[last_start .. their_size)` (at most 64 KiB)
/// from `ours` — never touches the prefix before `last_start`.
pub fn append_probe_for(ours: &Entry, their_size: u64, mut read: impl FnMut(u64, usize) -> io::Result<Vec<u8>>) -> io::Result<Option<AppendProbe>> {
    if ours.size <= their_size || their_size == 0 || ours.chunks.is_empty() {
        return Ok(None);
    }
    let mut off = 0u64;
    let mut k = 0usize;
    while k < ours.chunks.len() {
        let next = off + ours.chunks[k].len as u64;
        if next >= their_size {
            break;
        }
        off = next;
        k += 1;
    }
    if k >= ours.chunks.len() {
        return Ok(None);
    }
    let last_len = (their_size - off) as usize;
    if last_len == 0 || last_len > chunk::MAX {
        return Ok(None);
    }
    let slice = read(off, last_len)?;
    if slice.len() != last_len {
        return Ok(None);
    }
    let last_chunk = ChunkRef { len: last_len as u32, id: chunk::chunk_id(&slice) };
    let prefix_chunks_hash = hash_chunk_refs(&ours.chunks[..k]);
    Ok(Some(AppendProbe { prefix_chunk_count: k as u32, prefix_chunks_hash, last_chunk }))
}

/// Where an append's tail starts in `ours`: the index of our chunk that
/// holds the receiver's last byte (`base` long), which the receiver's last
/// chunk begins as. `None` where `base` isn't within `ours`.
pub fn append_tail(ours: &[ChunkRef], base: u64) -> Option<usize> {
    let mut off = 0u64;
    for (k, c) in ours.iter().enumerate() {
        let next = off + c.len as u64;
        if next >= base {
            return (base > off).then_some(k);
        }
        off = next;
    }
    None
}

/// Verify on the receiver whether `theirs` matches `probe` (i.e. `theirs` is an
/// exact prefix of the sender's file).
pub fn verify_append_probe(theirs: &Entry, probe: &AppendProbe) -> bool {
    let k = probe.prefix_chunk_count as usize;
    if theirs.chunks.len() != k.saturating_add(1) {
        return false;
    }
    if theirs.chunks[k] != probe.last_chunk {
        return false;
    }
    let total: u64 = theirs.chunks.iter().map(|c| c.len as u64).sum();
    if total != theirs.size {
        return false;
    }
    hash_chunk_refs(&theirs.chunks[..k]) == probe.prefix_chunks_hash
}

/// Check directly whether `ours` is a pure append on top of `theirs`, using
/// chunk lists first and reading at most 64 KiB (from `theirs`'s last chunk
/// start to `theirs.size`) only if every preceding chunk matches.
pub fn is_append(ours: &Entry, theirs: &Entry, mut read: impl FnMut(u64, usize) -> io::Result<Vec<u8>>) -> io::Result<bool> {
    if ours.size <= theirs.size || theirs.size == 0 || theirs.chunks.is_empty() {
        return Ok(false);
    }
    let m = theirs.chunks.len();
    if ours.chunks.len() < m {
        return Ok(false);
    }
    if ours.chunks[..m - 1] != theirs.chunks[..m - 1] {
        return Ok(false);
    }
    let last = theirs.chunks[m - 1];
    let last_start: u64 = theirs.chunks[..m - 1].iter().map(|c| c.len as u64).sum();
    if last_start.checked_add(last.len as u64) != Some(theirs.size) {
        return Ok(false);
    }
    if last.len == 0 || last.len as usize > chunk::MAX {
        return Ok(false);
    }
    let slice = read(last_start, last.len as usize)?;
    if slice.len() != last.len as usize {
        return Ok(false);
    }
    Ok(chunk::chunk_id(&slice) == last.id)
}

/// Build an append [`Plan`]: copy `[0, their_size)` from `path`, then send
/// `[their_size, our_size)` with the last up-to-64 KiB of `[0, their_size)` as
/// `ref_prefix` on the first tail piece.
pub fn plan_append(path: &str, our_size: u64, their_size: u64) -> Plan {
    let mut ops = Vec::new();
    ops.push(PlanOp::Copy { from: path.to_owned(), offset: 0, len: their_size });
    let prefix_len = their_size.min(REF_PREFIX_MAX as u64) as u32;
    let prefix_off = their_size - prefix_len as u64;
    let mut first = true;
    let mut pos = their_size;
    while pos < our_size {
        let len = (our_size - pos).min(MAX_DATA_PIECE_LEN as u64);
        let ref_prefix = if first && prefix_len > 0 {
            first = false;
            Some(RefPrefix { from: path.to_owned(), sender_offset: prefix_off, receiver_offset: prefix_off, len: prefix_len })
        } else {
            None
        };
        ops.push(PlanOp::Data { offset: pos, len, ref_prefix });
        pos += len;
    }
    Plan::Build { kind: RecipeKind::Append, ops }
}

/// Build a [`Plan`] for a brand-new file (streamed in `<= MAX_DATA_PIECE_LEN`
/// pieces).
pub fn plan_new(our_size: u64) -> Plan {
    let mut ops = Vec::new();
    let mut pos = 0u64;
    while pos < our_size {
        let len = (our_size - pos).min(MAX_DATA_PIECE_LEN as u64);
        ops.push(PlanOp::Data { offset: pos, len, ref_prefix: None });
        pos += len;
    }
    Plan::Build { kind: RecipeKind::New, ops }
}

/// Plan a chunk-level patch from `ours` against `theirs` (both at `path`).
pub fn plan_patch_chunks(path: &str, ours: &[ChunkRef], theirs: &[ChunkRef]) -> Plan {
    let mut their_map: HashMap<ChunkId, (u64, u32)> = HashMap::with_capacity(theirs.len());
    let mut off = 0u64;
    for c in theirs {
        their_map.entry(c.id).or_insert((off, c.len));
        off += c.len as u64;
    }

    let mut ops: Vec<PlanOp> = Vec::new();
    let mut our_off = 0u64;
    for c in ours {
        if let Some(&(their_off, their_len)) = their_map.get(&c.id)
            && their_len == c.len
        {
            push_copy(&mut ops, path, their_off, c.len as u64);
        } else {
            push_data(&mut ops, our_off, c.len as u64);
        }
        our_off += c.len as u64;
    }
    attach_patch_ref_prefixes(&mut ops);
    Plan::Build { kind: RecipeKind::Patch, ops }
}

/// Plan a chunk-level patch using 2-level [`SuperChunk`] matches plus leaf
/// chunks only for the receiver's unmatched super-chunks.
pub fn plan_from_super_and_leaf(path: &str, our_chunks: &[ChunkRef], their_supers: &[SuperChunk], leaf_chunks_by_super: &HashMap<u32, Vec<ChunkRef>>) -> Plan {
    let mut super_map: HashMap<ChunkId, &SuperChunk> = HashMap::with_capacity(their_supers.len());
    for sc in their_supers {
        super_map.entry(sc.id).or_insert(sc);
    }

    let mut leaf_map: HashMap<ChunkId, (u64, u32)> = HashMap::new();
    for (&super_idx, chunks) in leaf_chunks_by_super {
        let Some(sc) = their_supers.get(super_idx as usize) else {
            continue;
        };
        let mut off = sc.offset;
        for c in chunks {
            leaf_map.entry(c.id).or_insert((off, c.len));
            off += c.len as u64;
        }
    }

    let our_supers = super_chunks(our_chunks);
    let mut ops: Vec<PlanOp> = Vec::new();
    for os in &our_supers {
        if let Some(ts) = super_map.get(&os.id)
            && ts.len == os.len
        {
            push_copy(&mut ops, path, ts.offset, os.len);
        } else {
            let start = os.start_chunk as usize;
            let end = start + os.chunk_count as usize;
            let mut our_off = os.offset;
            for c in &our_chunks[start..end] {
                if let Some(&(their_off, their_len)) = leaf_map.get(&c.id)
                    && their_len == c.len
                {
                    push_copy(&mut ops, path, their_off, c.len as u64);
                } else {
                    push_data(&mut ops, our_off, c.len as u64);
                }
                our_off += c.len as u64;
            }
        }
    }
    attach_patch_ref_prefixes(&mut ops);
    Plan::Build { kind: RecipeKind::Patch, ops }
}

/// Choose the cheapest [`Plan`] for `ours` given the receiver's full [`Index`].
pub fn plan_file(ours: &Entry, receiver: &Index, read: impl FnMut(u64, usize) -> io::Result<Vec<u8>>) -> io::Result<Plan> {
    if let Some(theirs) = receiver.entries.get(&ours.path)
        && theirs.kind == EntryKind::File
        && theirs.hash == ours.hash
    {
        return Ok(Plan::Same);
    }
    if let Some(from) = receiver.entries.values().find(|e| e.kind == EntryKind::File && e.hash == ours.hash).map(|e| e.path.clone()) {
        return Ok(Plan::CopyFile { from });
    }
    if let Some(theirs) = receiver.entries.get(&ours.path)
        && theirs.kind == EntryKind::File
        && theirs.size > 0
    {
        if is_append(ours, theirs, read)? {
            return Ok(plan_append(&ours.path, ours.size, theirs.size));
        }
        return Ok(plan_patch_chunks(&ours.path, &ours.chunks, &theirs.chunks));
    }
    Ok(plan_new(ours.size))
}

fn push_copy(ops: &mut Vec<PlanOp>, from: &str, offset: u64, len: u64) {
    if len == 0 {
        return;
    }
    if let Some(PlanOp::Copy { from: prev_from, offset: prev_off, len: prev_len }) = ops.last_mut()
        && prev_from == from
        && *prev_off + *prev_len == offset
    {
        *prev_len += len;
        return;
    }
    ops.push(PlanOp::Copy { from: from.to_owned(), offset, len });
}

fn push_data(ops: &mut Vec<PlanOp>, offset: u64, len: u64) {
    if len == 0 {
        return;
    }
    let mut cur_off = offset;
    let mut rem = len;
    while rem > 0 {
        if let Some(PlanOp::Data { offset: prev_off, len: prev_len, ref_prefix: None }) = ops.last_mut()
            && *prev_off + *prev_len == cur_off
            && *prev_len < MAX_DATA_PIECE_LEN as u64
        {
            let room = MAX_DATA_PIECE_LEN as u64 - *prev_len;
            let take = rem.min(room);
            *prev_len += take;
            cur_off += take;
            rem -= take;
            continue;
        }
        let take = rem.min(MAX_DATA_PIECE_LEN as u64);
        ops.push(PlanOp::Data { offset: cur_off, len: take, ref_prefix: None });
        cur_off += take;
        rem -= take;
    }
}

/// When a small `Data` op immediately follows a `Copy` op in a patch, attach
/// the last up-to-64 KiB of that `Copy` as a `RefPrefix`: both sides hold
/// those exact bytes, so zstd compresses structured binary/text edits against
/// their immediate context.
fn attach_patch_ref_prefixes(ops: &mut [PlanOp]) {
    let mut out_pos = 0u64;
    let mut prev_copy: Option<(String, u64, u64, u64)> = None; // (from, sender_end, recv_end, len)
    for op in ops.iter_mut() {
        match op {
            PlanOp::Copy { from, offset, len } => {
                out_pos += *len;
                prev_copy = Some((from.clone(), out_pos, *offset + *len, *len));
            }
            PlanOp::Data { offset, len, ref_prefix } => {
                if ref_prefix.is_none()
                    && *len <= PATCH_REF_PREFIX_MAX_DATA
                    && let Some((from, sender_end, recv_end, copy_len)) = &prev_copy
                    && *sender_end == *offset
                    && *copy_len > 0
                {
                    let plen = (*copy_len).min(REF_PREFIX_MAX as u64) as u32;
                    *ref_prefix = Some(RefPrefix { from: from.clone(), sender_offset: *sender_end - plen as u64, receiver_offset: *recv_end - plen as u64, len: plen });
                }
                out_pos += *len;
                prev_copy = None;
            }
        }
    }
}

/// Compress `raw` with zstd level 3 (and optional `ref_prefix`), falling back
/// to `DataCodec::Raw` if compression does not make the payload smaller.
pub fn compress_piece(raw: &[u8], ref_prefix: Option<(&str, u64, &[u8])>) -> io::Result<Piece> {
    let raw_len = u32::try_from(raw.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "piece too large"))?;
    if raw.is_empty() {
        return Ok(Piece::Data { raw_len: 0, codec: DataCodec::Raw, bytes: Vec::new() });
    }

    let bound = zstd_safe::compress_bound(raw.len());
    let mut out = Vec::with_capacity(bound);
    let mut cctx = zstd_safe::CCtx::create();
    cctx.set_parameter(CParameter::CompressionLevel(ZSTD_LEVEL)).map_err(zstd_err)?;

    let codec = if let Some((from, prefix_offset, prefix)) = ref_prefix
        && !prefix.is_empty()
    {
        cctx.ref_prefix(prefix).map_err(zstd_err)?;
        DataCodec::ZstdRefPrefix { from: from.to_owned(), prefix_offset, prefix_len: prefix.len() as u32 }
    } else {
        DataCodec::Zstd
    };

    cctx.compress2(&mut out, raw).map_err(zstd_err)?;
    if out.len() < raw.len() { Ok(Piece::Data { raw_len, codec, bytes: out }) } else { Ok(Piece::Data { raw_len, codec: DataCodec::Raw, bytes: raw.to_vec() }) }
}

/// Decompress (or validate raw) bytes of a [`Piece::Data`], enforcing
/// `raw_len <= MAX_DATA_PIECE_LEN` before allocating.
pub fn decode_data_piece(raw_len: u32, codec: &DataCodec, bytes: &[u8], mut read_prefix: impl FnMut(&str, u64, usize) -> io::Result<Vec<u8>>) -> io::Result<Vec<u8>> {
    let raw_usize = raw_len as usize;
    if raw_usize > MAX_DATA_PIECE_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("piece raw_len {raw_usize} exceeds limit {MAX_DATA_PIECE_LEN}")));
    }
    match codec {
        DataCodec::Raw => {
            if bytes.len() != raw_usize {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "raw piece length mismatch"));
            }
            Ok(bytes.to_vec())
        }
        DataCodec::Zstd => {
            let mut out = Vec::with_capacity(raw_usize);
            let mut dctx = zstd_safe::DCtx::create();
            dctx.decompress(&mut out, bytes).map_err(zstd_err)?;
            if out.len() != raw_usize {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "decompressed piece length mismatch"));
            }
            Ok(out)
        }
        DataCodec::ZstdRefPrefix { from, prefix_offset, prefix_len } => {
            let plen = *prefix_len as usize;
            if plen > REF_PREFIX_MAX {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "ref_prefix exceeds limit"));
            }
            let prefix = read_prefix(from, *prefix_offset, plen)?;
            if prefix.len() != plen {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read for ref_prefix"));
            }
            let mut out = Vec::with_capacity(raw_usize);
            let mut dctx = zstd_safe::DCtx::create();
            dctx.ref_prefix(&prefix).map_err(zstd_err)?;
            dctx.decompress(&mut out, bytes).map_err(zstd_err)?;
            if out.len() != raw_usize {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "decompressed ref_prefix piece length mismatch"));
            }
            Ok(out)
        }
    }
}

/// Stream [`Piece`]s from `ops`, reading at most `MAX_DATA_PIECE_LEN` +
/// `REF_PREFIX_MAX` bytes at a time.
pub fn emit_pieces(ops: &[PlanOp], read: &mut impl FnMut(u64, usize) -> io::Result<Vec<u8>>, emit: &mut impl FnMut(Piece) -> io::Result<()>) -> io::Result<()> {
    for op in ops {
        match op {
            PlanOp::Copy { from, offset, len } => {
                emit(Piece::Copy { from: from.clone(), offset: *offset, len: *len })?;
            }
            PlanOp::Data { offset, len, ref_prefix } => {
                let raw = read(*offset, *len as usize)?;
                if raw.len() as u64 != *len {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read while emitting data piece"));
                }
                let prefix_buf = match ref_prefix {
                    Some(rp) if rp.len > 0 => {
                        let p = read(rp.sender_offset, rp.len as usize)?;
                        if p.len() != rp.len as usize {
                            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read for ref_prefix"));
                        }
                        Some((rp.from.as_str(), rp.receiver_offset, p))
                    }
                    _ => None,
                };
                let piece = match &prefix_buf {
                    Some((from, recv_off, p)) => compress_piece(&raw, Some((from, *recv_off, p)))?,
                    None => compress_piece(&raw, None)?,
                };
                emit(piece)?;
            }
        }
    }
    Ok(())
}

/// Materialize all [`PlanOp`]s into a [`Recipe`].
pub fn materialize(kind: RecipeKind, ops: &[PlanOp], read: &mut impl FnMut(u64, usize) -> io::Result<Vec<u8>>) -> io::Result<Recipe> {
    let mut pieces = Vec::with_capacity(ops.len());
    emit_pieces(ops, read, &mut |p| {
        pieces.push(p);
        Ok(())
    })?;
    Ok(Recipe { kind, pieces })
}

/// Apply a [`Recipe`] against existing files under `root`, writing the
/// reconstructed bytes to `out` and returning their whole-file BLAKE3.
pub fn apply_recipe(recipe: &Recipe, root: &Path, out: &mut impl Write) -> io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut copy_buf = vec![0u8; 64 << 10];
    for piece in &recipe.pieces {
        match piece {
            Piece::Copy { from, offset, len } => {
                let mut f = open_safe_basis(root, from)?;
                f.seek(SeekFrom::Start(*offset))?;
                let mut rem = *len;
                while rem > 0 {
                    let take = (rem as usize).min(copy_buf.len());
                    f.read_exact(&mut copy_buf[..take])?;
                    out.write_all(&copy_buf[..take])?;
                    hasher.update(&copy_buf[..take]);
                    rem -= take as u64;
                }
            }
            Piece::Data { raw_len, codec, bytes } => {
                let raw = decode_data_piece(*raw_len, codec, bytes, |from, off, len| {
                    let mut f = open_safe_basis(root, from)?;
                    f.seek(SeekFrom::Start(off))?;
                    let mut buf = vec![0u8; len];
                    f.read_exact(&mut buf)?;
                    Ok(buf)
                })?;
                out.write_all(&raw)?;
                hasher.update(&raw);
            }
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

fn open_safe_basis(root: &Path, rel: &str) -> io::Result<fs::File> {
    validate_rel_path(rel).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut cur = root.to_path_buf();
    for seg in rel.split('/') {
        cur.push(seg);
        let meta = fs::symlink_metadata(&cur)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "basis path traverses symlink"));
        }
    }
    fs::File::open(cur)
}

fn zstd_err(code: usize) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, zstd_safe::get_error_name(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Filter;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch_dir(name: &str) -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-sync-recipe-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn jsonl_lines(start: usize, count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in start..start + count {
            out.extend_from_slice(
                format!(
                    "{{\"type\":\"assistant\",\"session\":\"sess-01928374\",\"turn\":{i},\"model\":\"gemini-4-neon\",\"content\":\"thinking carefully about step {i} in /home/me/src/app/src/recipe.rs\"}}\n"
                )
                .as_bytes(),
            );
        }
        out
    }

    fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    fn read_slice(data: &[u8]) -> impl FnMut(u64, usize) -> io::Result<Vec<u8>> + '_ {
        move |off, len| {
            let start = off as usize;
            let end = (start + len).min(data.len());
            Ok(data[start..end].to_vec())
        }
    }

    #[test]
    fn selects_same_and_copy_file() {
        let recv_dir = scratch_dir("same-copy");
        fs::write(recv_dir.join("a.txt"), b"hello world content").unwrap();
        let recv_idx = Index::scan(&recv_dir, &Filter::all(), None).unwrap();

        let our_same = recv_idx.entries["a.txt"].clone();
        assert_eq!(plan_file(&our_same, &recv_idx, |_, _| unreachable!()).unwrap(), Plan::Same);

        let mut our_moved = our_same;
        our_moved.path = "renamed.txt".into();
        assert_eq!(plan_file(&our_moved, &recv_idx, |_, _| unreachable!()).unwrap(), Plan::CopyFile { from: "a.txt".into() });

        let _ = fs::remove_dir_all(&recv_dir);
    }

    #[test]
    fn selects_append_uses_ref_prefix_and_rebuilds_exact() {
        let recv_dir = scratch_dir("append");
        let send_dir = scratch_dir("append-send");
        let base = jsonl_lines(0, 1000);
        let mut grown = base.clone();
        grown.extend_from_slice(&jsonl_lines(1000, 25));

        fs::write(recv_dir.join("log.jsonl"), &base).unwrap();
        fs::write(send_dir.join("log.jsonl"), &grown).unwrap();

        let recv_idx = Index::scan(&recv_dir, &Filter::all(), None).unwrap();
        let send_idx = Index::scan(&send_dir, &Filter::all(), None).unwrap();
        let ours = &send_idx.entries["log.jsonl"];

        // Verify append check reads at most 64 KiB of the prefix.
        let mut bytes_read_for_check = 0usize;
        let plan = plan_file(ours, &recv_idx, |off, len| {
            bytes_read_for_check += len;
            Ok(grown[off as usize..off as usize + len].to_vec())
        })
        .unwrap();
        assert!(bytes_read_for_check <= chunk::MAX);

        let Plan::Build { kind, ops } = plan else {
            panic!("expected Build(Append), got {plan:?}");
        };
        assert_eq!(kind, RecipeKind::Append);

        // Verify AppendProbe also confirms on the receiver.
        let probe = append_probe_for(ours, base.len() as u64, read_slice(&grown)).unwrap().unwrap();
        assert!(verify_append_probe(&recv_idx.entries["log.jsonl"], &probe));

        let recipe = materialize(kind, &ops, &mut read_slice(&grown)).unwrap();
        // Should have 1 Copy of the full old prefix and 1 ZstdRefPrefix Data piece.
        assert_eq!(recipe.pieces.len(), 2);
        assert!(matches!(
            recipe.pieces[0],
            Piece::Copy {
                offset: 0,
                len,
                ..
            } if len == base.len() as u64
        ));
        assert!(matches!(recipe.pieces[1], Piece::Data { codec: DataCodec::ZstdRefPrefix { .. }, .. }));

        let mut rebuilt = Vec::new();
        let hash = apply_recipe(&recipe, &recv_dir, &mut rebuilt).unwrap();
        assert_eq!(rebuilt, grown);
        assert_eq!(hash, ours.hash);

        let _ = fs::remove_dir_all(&recv_dir);
        let _ = fs::remove_dir_all(&send_dir);
    }

    #[test]
    fn selects_patch_for_mid_file_edit_and_rebuilds_exact() {
        let recv_dir = scratch_dir("patch");
        let send_dir = scratch_dir("patch-send");
        let old = jsonl_lines(0, 1500);
        let mut edited = old.clone();
        let mid = edited.len() / 2;
        edited[mid] ^= 0x55;

        fs::write(recv_dir.join("data.bin"), &old).unwrap();
        fs::write(send_dir.join("data.bin"), &edited).unwrap();

        let recv_idx = Index::scan(&recv_dir, &Filter::all(), None).unwrap();
        let send_idx = Index::scan(&send_dir, &Filter::all(), None).unwrap();
        let ours = &send_idx.entries["data.bin"];

        let plan = plan_file(ours, &recv_idx, read_slice(&edited)).unwrap();
        let Plan::Build { kind, ops } = plan else {
            panic!("expected Build(Patch), got {plan:?}");
        };
        assert_eq!(kind, RecipeKind::Patch);
        // Adjacent copies should be merged: Copy, Data, Copy.
        assert!(ops.len() <= 4, "expected merged copies, got {} ops", ops.len());

        let recipe = materialize(kind, &ops, &mut read_slice(&edited)).unwrap();
        let mut rebuilt = Vec::new();
        let hash = apply_recipe(&recipe, &recv_dir, &mut rebuilt).unwrap();
        assert_eq!(rebuilt, edited);
        assert_eq!(hash, ours.hash);

        let _ = fs::remove_dir_all(&recv_dir);
        let _ = fs::remove_dir_all(&send_dir);
    }

    #[test]
    fn incompressible_data_falls_back_to_raw() {
        let random = pseudo_random(32 << 10, 999);
        let piece = compress_piece(&random, None).unwrap();
        match &piece {
            Piece::Data { raw_len, codec, bytes } => {
                assert_eq!(*raw_len as usize, random.len());
                assert_eq!(*codec, DataCodec::Raw);
                assert_eq!(bytes, &random);
            }
            other => panic!("expected Raw Data piece, got {other:?}"),
        }
    }

    #[test]
    fn new_file_splits_into_bounded_pieces() {
        let recv_dir = scratch_dir("new-file");
        let send_dir = scratch_dir("new-file-send");
        let big = vec![b'A'; MAX_DATA_PIECE_LEN + 50_000];
        fs::write(send_dir.join("big.txt"), &big).unwrap();

        let recv_idx = Index::scan(&recv_dir, &Filter::all(), None).unwrap();
        let send_idx = Index::scan(&send_dir, &Filter::all(), None).unwrap();
        let ours = &send_idx.entries["big.txt"];

        let plan = plan_file(ours, &recv_idx, read_slice(&big)).unwrap();
        let Plan::Build { kind, ops } = plan else {
            panic!("expected Build(New)");
        };
        assert_eq!(kind, RecipeKind::New);
        assert_eq!(ops.len(), 2);

        let recipe = materialize(kind, &ops, &mut read_slice(&big)).unwrap();
        let mut rebuilt = Vec::new();
        let hash = apply_recipe(&recipe, &recv_dir, &mut rebuilt).unwrap();
        assert_eq!(rebuilt, big);
        assert_eq!(hash, ours.hash);

        let _ = fs::remove_dir_all(&recv_dir);
        let _ = fs::remove_dir_all(&send_dir);
    }
}
