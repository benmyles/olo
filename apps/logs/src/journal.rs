//! systemd's journal files, read natively (the format is documented in
//! systemd's JOURNAL_FILE_FORMAT.md): the header, then entries found
//! through the chain of entry arrays, each entry's fields in data objects
//! (compressed with zstd, xz or lz4, in the regular or the compact layout).
//! A file still being written is read again from where it was left as it
//! grows; journald only appends and fills arrays in order.

use std::{
    collections::HashMap,
    fs::File,
    io,
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    sync::Arc,
};

const SIGNATURE: &[u8; 8] = b"LPKSHHRH";

const INCOMPAT_XZ: u32 = 1;
const INCOMPAT_LZ4: u32 = 2;
const INCOMPAT_KEYED_HASH: u32 = 4;
const INCOMPAT_ZSTD: u32 = 8;
const INCOMPAT_COMPACT: u32 = 16;
const INCOMPAT_KNOWN: u32 = INCOMPAT_XZ | INCOMPAT_LZ4 | INCOMPAT_KEYED_HASH | INCOMPAT_ZSTD | INCOMPAT_COMPACT;

const OBJECT_DATA: u8 = 1;
const OBJECT_ENTRY: u8 = 3;
const OBJECT_ENTRY_ARRAY: u8 = 6;

const COMPRESSED_XZ: u8 = 1;
const COMPRESSED_LZ4: u8 = 2;
const COMPRESSED_ZSTD: u8 = 4;

/// No field we show is bigger than this (journald's own default limit for
/// a field is 64 KiB; a core dump can be far larger and isn't wanted).
const MAX_FIELD: u64 = 1 << 20;

/// The fields kept of each entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Field {
    Message,
    Priority,
    Comm,
    Exe,
    Identifier,
    Pid,
    UserUnit,
    Unit,
}

impl Field {
    fn from_name(name: &[u8]) -> Option<Field> {
        Some(match name {
            b"MESSAGE" => Field::Message,
            b"PRIORITY" => Field::Priority,
            b"_COMM" => Field::Comm,
            b"_EXE" => Field::Exe,
            b"SYSLOG_IDENTIFIER" => Field::Identifier,
            b"_PID" => Field::Pid,
            b"_SYSTEMD_USER_UNIT" => Field::UserUnit,
            b"_SYSTEMD_UNIT" => Field::Unit,
            _ => return None,
        })
    }
}

/// One journal entry, with the fields we keep.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    /// Microseconds since the epoch.
    pub realtime: u64,
    pub boot: [u8; 16],
    pub fields: Vec<(Field, Arc<str>)>,
}

impl Entry {
    pub fn get(&self, field: Field) -> Option<&str> {
        self.fields.iter().find(|(f, _)| *f == field).map(|(_, v)| &**v)
    }
}

fn le64(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

struct Header {
    incompatible: u32,
    n_entries: u64,
    entry_array_offset: u64,
    head_realtime: u64,
}

fn read_header(file: &File) -> io::Result<Header> {
    let mut h = [0u8; 208];
    file.read_exact_at(&mut h, 0)?;
    if &h[..8] != SIGNATURE {
        return Err(bad("not a journal file"));
    }
    let incompatible = le32(&h, 12);
    if incompatible & !INCOMPAT_KNOWN != 0 {
        return Err(bad("a journal file feature this reader doesn't know"));
    }
    Ok(Header { incompatible, n_entries: le64(&h, 152), entry_array_offset: le64(&h, 176), head_realtime: le64(&h, 184) })
}

/// A journal file, read entry by entry and again as it grows.
pub struct Journal {
    pub path: PathBuf,
    file: File,
    compact: bool,
    /// The entry array being read and the index in it of the next entry.
    array: u64,
    index: u64,
    /// Entries read so far.
    seen: u64,
    /// Data objects already read (most are shared between entries: the
    /// program, its unit, its pid); `None` for fields we don't keep.
    cache: HashMap<u64, Option<(Field, Arc<str>)>>,
}

impl Journal {
    pub fn open(path: &Path) -> io::Result<Journal> {
        let file = File::open(path)?;
        let h = read_header(&file)?;
        Ok(Journal { path: path.to_owned(), file, compact: h.incompatible & INCOMPAT_COMPACT != 0, array: 0, index: 0, seen: 0, cache: HashMap::new() })
    }

    /// The first entry's time (to order files).
    pub fn head_realtime(&self) -> u64 {
        read_header(&self.file).map_or(0, |h| h.head_realtime)
    }

    /// Whether the file now at `path` is another one (journald rotated it).
    pub fn replaced(&self) -> bool {
        match (std::fs::metadata(&self.path), self.file.metadata()) {
            (Ok(now), Ok(ours)) => now.ino() != ours.ino() || now.dev() != ours.dev(),
            _ => true,
        }
    }

    fn read(&self, at: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.file.read_exact_at(&mut buf, at)?;
        Ok(buf)
    }

    /// An object's type, flags and size.
    fn object_header(&self, at: u64) -> io::Result<(u8, u8, u64)> {
        let h = self.read(at, 16)?;
        let size = le64(&h, 8);
        if size < 16 {
            return Err(bad("an object too small"));
        }
        Ok((h[0], h[1], size))
    }

    /// Up to `max` entries not read before; fewer when the file has no
    /// more (yet).
    pub fn read_new(&mut self, max: usize, out: &mut Vec<Entry>) -> io::Result<()> {
        let h = read_header(&self.file)?;
        if self.array == 0 {
            self.array = h.entry_array_offset;
        }
        let item = if self.compact { 4 } else { 8 };
        let mut taken = 0;
        while self.array != 0 && self.seen < h.n_entries && taken < max {
            let (kind, _, size) = self.object_header(self.array)?;
            if kind != OBJECT_ENTRY_ARRAY {
                return Err(bad("an entry array that isn't one"));
            }
            let count = (size - 24) / item;
            if self.index >= count {
                let next = le64(&self.read(self.array + 16, 8)?, 0);
                if next == 0 {
                    break;
                }
                self.array = next;
                self.index = 0;
                continue;
            }
            let raw = self.read(self.array + 24 + self.index * item, item as usize)?;
            let offset = if self.compact { le32(&raw, 0) as u64 } else { le64(&raw, 0) };
            if offset == 0 {
                // Not written yet.
                break;
            }
            self.index += 1;
            self.seen += 1;
            match self.entry(offset) {
                Ok(e) => {
                    out.push(e);
                    taken += 1;
                }
                // One damaged entry (a file journald didn't close cleanly)
                // isn't the end of the file.
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn entry(&mut self, at: u64) -> io::Result<Entry> {
        let (kind, _, size) = self.object_header(at)?;
        if kind != OBJECT_ENTRY || !(64..=1 << 24).contains(&size) {
            return Err(bad("an entry that isn't one"));
        }
        let obj = self.read(at, size as usize)?;
        let mut entry = Entry { realtime: le64(&obj, 24), ..Default::default() };
        entry.boot.copy_from_slice(&obj[40..56]);
        let item = if self.compact { 4 } else { 16 };
        let mut i = 64;
        while i + item <= obj.len() {
            let data = if self.compact { le32(&obj, i) as u64 } else { le64(&obj, i) };
            i += item;
            if data == 0 {
                continue;
            }
            let field = match self.cache.get(&data) {
                Some(f) => f.clone(),
                None => {
                    let f = self.data(data)?;
                    // A message is seldom repeated; everything else is.
                    if !matches!(f, Some((Field::Message, _))) {
                        if self.cache.len() > 50_000 {
                            self.cache.clear();
                        }
                        self.cache.insert(data, f.clone());
                    }
                    f
                }
            };
            if let Some(f) = field {
                entry.fields.push(f);
            }
        }
        Ok(entry)
    }

    /// A data object's field, if it is one we keep.
    fn data(&self, at: u64) -> io::Result<Option<(Field, Arc<str>)>> {
        let (kind, flags, size) = self.object_header(at)?;
        if kind != OBJECT_DATA {
            return Err(bad("a data object that isn't one"));
        }
        let start = if self.compact { 72 } else { 64 };
        if size < start || size - start > MAX_FIELD {
            return Ok(None);
        }
        let stored = self.read(at + start, (size - start) as usize)?;
        let payload = match flags & (COMPRESSED_XZ | COMPRESSED_LZ4 | COMPRESSED_ZSTD) {
            0 => stored,
            COMPRESSED_ZSTD => decompress_zstd(&stored)?,
            COMPRESSED_XZ => decompress_xz(&stored)?,
            COMPRESSED_LZ4 => decompress_lz4(&stored)?,
            _ => return Err(bad("data compressed two ways")),
        };
        let Some(eq) = payload.iter().position(|&b| b == b'=') else { return Ok(None) };
        let Some(field) = Field::from_name(&payload[..eq]) else { return Ok(None) };
        Ok(Some((field, Arc::from(String::from_utf8_lossy(&payload[eq + 1..]).as_ref()))))
    }
}

fn decompress_zstd(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut d = zstd::stream::read::Decoder::new(data)?;
    io::Read::read_to_end(&mut io::Read::take(&mut d, MAX_FIELD), &mut out)?;
    Ok(out)
}

fn decompress_xz(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut d = liblzma::read::XzDecoder::new(data);
    io::Read::read_to_end(&mut io::Read::take(&mut d, MAX_FIELD), &mut out)?;
    Ok(out)
}

/// journald's lz4: the uncompressed size (64-bit) and then one lz4 block.
fn decompress_lz4(data: &[u8]) -> io::Result<Vec<u8>> {
    let size = le64(data, 0);
    if data.len() < 8 || size > MAX_FIELD {
        return Err(bad("lz4 data too large"));
    }
    lz4_flex::block::decompress(&data[8..], size as usize).map_err(|e| bad(&e.to_string()))
}

/// This user's journal files, oldest first: the persistent journal in
/// /var/log/journal or, without one, the volatile one in /run/log/journal.
/// The last is the one being written (`user-UID.journal`).
pub fn user_files(uid: u32) -> Vec<PathBuf> {
    let machine = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
    let machine = machine.trim();
    let mut files = Vec::new();
    for root in ["/var/log/journal", "/run/log/journal"] {
        let dir = Path::new(root).join(machine);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let active = format!("user-{uid}.journal");
        let archived = format!("user-{uid}@");
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == active || (name.starts_with(&archived) && (name.ends_with(".journal") || name.ends_with(".journal~"))) {
                files.push(e.path());
            }
        }
        if !files.is_empty() {
            break;
        }
    }
    // Oldest first by the first entry's time; the active file last.
    let mut keyed: Vec<(u64, bool, PathBuf)> = files
        .into_iter()
        .map(|p| {
            let active = p.file_name().is_some_and(|n| !n.to_string_lossy().contains('@'));
            let head = Journal::open(&p).map_or(0, |j| j.head_realtime());
            (head, active, p)
        })
        .collect();
    keyed.sort_by_key(|(head, active, _)| (*active, *head));
    keyed.into_iter().map(|(_, _, p)| p).collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A small journal file in the regular or compact layout, uncompressed
    /// or with its messages in zstd, written the way journald lays it out
    /// (objects 8-byte aligned, entry arrays chained).
    pub fn write_journal(path: &Path, compact: bool, zstd: bool, entries: &[(u64, &[(&str, &str)])]) {
        let mut buf = vec![0u8; 256];
        buf[..8].copy_from_slice(SIGNATURE);
        let incompat = if compact { INCOMPAT_COMPACT } else { 0 } | if zstd { INCOMPAT_ZSTD } else { 0 };
        buf[12..16].copy_from_slice(&incompat.to_le_bytes());
        let align = |b: &mut Vec<u8>| {
            while !b.len().is_multiple_of(8) {
                b.push(0);
            }
        };
        let object = |b: &mut Vec<u8>, kind: u8, flags: u8, body: &[u8]| -> u64 {
            align(b);
            let at = b.len() as u64;
            b.push(kind);
            b.push(flags);
            b.extend_from_slice(&[0; 6]);
            b.extend_from_slice(&((16 + body.len()) as u64).to_le_bytes());
            b.extend_from_slice(body);
            at
        };
        let mut entry_offsets = Vec::new();
        for (i, (time, fields)) in entries.iter().enumerate() {
            let mut items = Vec::new();
            for (k, v) in *fields {
                let text = format!("{k}={v}");
                let (flags, payload) = if zstd && *k == "MESSAGE" { (COMPRESSED_ZSTD, zstd::encode_all(text.as_bytes(), 3).unwrap()) } else { (0, text.into_bytes()) };
                let mut body = vec![0u8; if compact { 56 } else { 48 }];
                body.extend_from_slice(&payload);
                items.push(object(&mut buf, OBJECT_DATA, flags, &body));
            }
            let mut body = Vec::new();
            body.extend_from_slice(&(i as u64 + 1).to_le_bytes());
            body.extend_from_slice(&time.to_le_bytes());
            body.extend_from_slice(&0u64.to_le_bytes());
            body.extend_from_slice(&[7; 16]);
            body.extend_from_slice(&0u64.to_le_bytes());
            for it in items {
                if compact {
                    body.extend_from_slice(&(it as u32).to_le_bytes());
                } else {
                    body.extend_from_slice(&it.to_le_bytes());
                    body.extend_from_slice(&0u64.to_le_bytes());
                }
            }
            entry_offsets.push(object(&mut buf, OBJECT_ENTRY, 0, &body));
        }
        // Two arrays: room for 2 in the first, the rest (and a spare slot) in the second.
        let item = |b: &mut Vec<u8>, off: u64| if compact { b.extend_from_slice(&(off as u32).to_le_bytes()) } else { b.extend_from_slice(&off.to_le_bytes()) };
        let (first, rest) = entry_offsets.split_at(entry_offsets.len().min(2));
        let mut body2 = 0u64.to_le_bytes().to_vec();
        for &o in rest {
            item(&mut body2, o);
        }
        item(&mut body2, 0);
        let second = object(&mut buf, OBJECT_ENTRY_ARRAY, 0, &body2);
        let mut body1 = (if rest.is_empty() { 0 } else { second }).to_le_bytes().to_vec();
        for &o in first {
            item(&mut body1, o);
        }
        let first_at = object(&mut buf, OBJECT_ENTRY_ARRAY, 0, &body1);
        buf[152..160].copy_from_slice(&(entries.len() as u64).to_le_bytes());
        buf[176..184].copy_from_slice(&first_at.to_le_bytes());
        buf[184..192].copy_from_slice(&entries.first().map_or(0, |e| e.0).to_le_bytes());
        std::fs::write(path, buf).unwrap();
    }

    #[test]
    fn reads_both_layouts() {
        let dir = std::env::temp_dir().join(format!("tabula-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (compact, zstd) in [(false, false), (true, false), (true, true), (false, true)] {
            let path = dir.join(format!("j-{compact}-{zstd}.journal"));
            write_journal(
                &path,
                compact,
                zstd,
                &[
                    (10, &[("MESSAGE", "first"), ("_COMM", "tabula-app-serv"), ("PRIORITY", "6"), ("_BOOT_ID", "x")]),
                    (20, &[("MESSAGE", "second"), ("_COMM", "tabula-app-serv")]),
                    (30, &[("MESSAGE", "third has = in it"), ("_COMM", "chrome"), ("_PID", "42")]),
                ],
            );
            let mut j = Journal::open(&path).unwrap();
            let mut out = Vec::new();
            j.read_new(2, &mut out).unwrap();
            assert_eq!(out.len(), 2);
            j.read_new(100, &mut out).unwrap();
            assert_eq!(out.len(), 3, "compact {compact} zstd {zstd}");
            assert_eq!(out[0].get(Field::Message), Some("first"));
            assert_eq!(out[0].get(Field::Priority), Some("6"));
            assert_eq!(out[2].get(Field::Message), Some("third has = in it"));
            assert_eq!(out[2].get(Field::Pid), Some("42"));
            assert_eq!(out[2].realtime, 30);
            assert_eq!(out[1].fields.len(), 2);
            // Nothing new.
            j.read_new(100, &mut out).unwrap();
            assert_eq!(out.len(), 3);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Append an entry with one MESSAGE as journald does: the objects at
    /// the end, then the next free slot of the last entry array, then the
    /// header's count.
    fn append(path: &Path, compact: bool, time: u64, message: &str) {
        let mut b = std::fs::read(path).unwrap();
        let obj = |b: &mut Vec<u8>, kind: u8, body: &[u8]| -> u64 {
            while !b.len().is_multiple_of(8) {
                b.push(0);
            }
            let at = b.len() as u64;
            b.extend_from_slice(&[kind, 0, 0, 0, 0, 0, 0, 0]);
            b.extend_from_slice(&((16 + body.len()) as u64).to_le_bytes());
            b.extend_from_slice(body);
            at
        };
        let mut data = vec![0u8; if compact { 56 } else { 48 }];
        data.extend_from_slice(format!("MESSAGE={message}").as_bytes());
        let d = obj(&mut b, OBJECT_DATA, &data);
        let mut entry = 99u64.to_le_bytes().to_vec();
        entry.extend_from_slice(&time.to_le_bytes());
        entry.extend_from_slice(&[0; 8 + 16 + 8]);
        if compact {
            entry.extend_from_slice(&(d as u32).to_le_bytes());
        } else {
            entry.extend_from_slice(&d.to_le_bytes());
            entry.extend_from_slice(&[0; 8]);
        }
        let e = obj(&mut b, OBJECT_ENTRY, &entry);
        // The last array, and its first empty slot.
        let item = if compact { 4 } else { 8 };
        let mut array = le64(&b, 176) as usize;
        while le64(&b, array + 16) != 0 {
            array = le64(&b, array + 16) as usize;
        }
        let size = le64(&b, array + 8) as usize;
        let slot = (array + 24..array + size).step_by(item).find(|&at| b[at..at + item].iter().all(|&x| x == 0)).expect("a free slot");
        if compact {
            b[slot..slot + 4].copy_from_slice(&(e as u32).to_le_bytes());
        } else {
            b[slot..slot + 8].copy_from_slice(&e.to_le_bytes());
        }
        let n = le64(&b, 152) + 1;
        b[152..160].copy_from_slice(&n.to_le_bytes());
        // In place, as journald writes (the reader keeps the file open).
        std::fs::OpenOptions::new().write(true).open(path).unwrap().write_all_at(&b, 0).unwrap();
    }

    #[test]
    fn follows_a_growing_file() {
        let dir = std::env::temp_dir().join(format!("tabula-journal-grow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for compact in [false, true] {
            let path = dir.join(format!("g-{compact}.journal"));
            write_journal(&path, compact, false, &[(10, &[("MESSAGE", "one")]), (20, &[("MESSAGE", "two")]), (30, &[("MESSAGE", "three")])]);
            let mut j = Journal::open(&path).unwrap();
            let mut out = Vec::new();
            j.read_new(100, &mut out).unwrap();
            assert_eq!(out.len(), 3);
            append(&path, compact, 40, "four");
            out.clear();
            j.read_new(100, &mut out).unwrap();
            assert_eq!(out.len(), 1, "compact {compact}");
            assert_eq!(out[0].get(Field::Message), Some("four"));
            assert!(!j.replaced());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "reads this machine's journal"]
    fn reads_the_real_journal() {
        // SAFETY: getuid has no preconditions.
        let files = user_files(unsafe { libc::getuid() });
        assert!(!files.is_empty());
        let mut total = 0;
        for f in files {
            let mut j = Journal::open(&f).unwrap();
            let mut out = Vec::new();
            j.read_new(usize::MAX, &mut out).unwrap();
            eprintln!("{}: {} entries", f.display(), out.len());
            total += out.len();
        }
        assert!(total > 0);
    }
}
