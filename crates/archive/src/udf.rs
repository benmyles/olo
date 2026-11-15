//! UDF disc images (ECMA-167, OSTA's UDF 1.02 to 2.60), read: the file
//! system of DVDs, Blu-ray discs and Windows' install images, whose
//! ISO 9660 tree, where there is one, holds no more than a note saying
//! to read this one. Read from the image as it's needed (a directory at
//! a time in memory, a file streamed); UDF 2.50's metadata partition, a
//! write-once disc's virtual allocation table and strategy 4096's chains
//! of entries followed; a sparable partition read as it was written.

use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read},
    os::unix::fs::FileExt,
    path::Path,
};

use crate::{Control, Entry, EntryKind, Result, safe_path, sink::Sink, stopped_or};

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("UDF: {why}"))
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64le(b: &[u8], at: usize) -> u64 {
    u32le(b, at) as u64 | (u32le(b, at + 4) as u64) << 32
}

/// A descriptor's tag identifier, if its checksum adds up.
fn tag(b: &[u8]) -> Option<u16> {
    if b.len() < 16 {
        return None;
    }
    let sum = b[..16].iter().enumerate().filter(|(i, _)| *i != 4).fold(0u8, |s, (_, v)| s.wrapping_add(*v));
    (sum == b[4]).then(|| u16le(b, 0))
}

/// Whether the volume recognition sequence (from byte 32768) names a UDF
/// volume: an NSR descriptor before its end.
pub fn is_udf(f: &File) -> bool {
    let mut b = [0u8; 6];
    for k in 0..64u64 {
        if f.read_exact_at(&mut b, 32768 + k * 2048).is_err() {
            return false;
        }
        match &b[1..6] {
            b"NSR02" | b"NSR03" => return true,
            b"TEA01" => return false,
            _ => {}
        }
    }
    false
}

/// OSTA compressed Unicode (d-characters): 8 bits a character, or 16.
fn cs0(b: &[u8]) -> String {
    match b.split_first() {
        Some((8 | 254, rest)) => rest.iter().map(|&c| c as char).collect(),
        Some((16 | 255, rest)) => String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect::<Vec<_>>()),
        _ => String::new(),
    }
}

/// A 12-byte timestamp as Unix time.
fn time(b: &[u8]) -> Option<i64> {
    let tz = u16le(b, 0);
    let year = u16le(b, 2) as i64;
    if year == 0 {
        return None;
    }
    // Minutes from UTC, a signed 12-bit number, when the type is local time.
    let off = (tz & 0x0fff) as i64;
    let off = if off & 0x800 != 0 { off - 0x1000 } else { off };
    let off = if tz >> 12 == 1 && off != -2047 { off } else { 0 };
    let days = crate::zipfmt::days_from_civil(year, b[4].clamp(1, 12) as u32, b[5].clamp(1, 31) as u32);
    Some(days * 86_400 + b[6] as i64 * 3600 + b[7] as i64 * 60 + b[8] as i64 - off * 60)
}

/// A logical block: its number in a partition (by reference number).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Lb {
    part: u16,
    lbn: u32,
}

fn long_ad(b: &[u8], at: usize) -> Lb {
    Lb { lbn: u32le(b, at + 4), part: u16le(b, at + 8) }
}

enum Map {
    /// A partition at this block of the image.
    Physical { num: u16, start: u64 },
    /// A write-once disc's: a block's place in the physical partition
    /// (at `start`) looked up in its allocation table.
    Virtual { start: u64, table: Vec<u32> },
    /// UDF 2.50's metadata partition, the blocks of a file: runs of
    /// (first block, blocks, the image's block).
    Metadata(Vec<(u64, u64, u64)>),
    /// Not read (a type this doesn't know).
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    File,
    Dir,
    Symlink,
    /// Devices, pipes, sockets, the volume's own files: left out.
    Other,
}

/// An extent of a file's data.
#[derive(Clone, Copy)]
struct Ext {
    at: Lb,
    len: u64,
    /// False for an extent allocated but not written (read as zeros).
    recorded: bool,
}

#[derive(Clone)]
enum Data {
    Inline(Vec<u8>),
    Extents(Vec<Ext>),
}

/// A file entry.
#[derive(Clone)]
struct Node {
    kind: Kind,
    mode: Option<u32>,
    size: u64,
    mtime: Option<i64>,
    data: Data,
}

pub struct Udf {
    f: File,
    bs: u64,
    maps: Vec<Map>,
    root: Lb,
}

/// The volume descriptor sequence at `(length, block)`: its partitions
/// (number, first block, length) and its logical volume descriptor.
#[allow(clippy::type_complexity)]
fn sequence(f: &File, bs: u64, (len, loc): (u32, u32)) -> io::Result<(Vec<(u16, u64, u64)>, Vec<u8>)> {
    let (mut len, mut loc) = (len as u64, loc as u64);
    // (number, start, length, sequence number); (sequence number, descriptor)
    let mut parts: Vec<(u16, u64, u64, u32)> = Vec::new();
    let mut lvd: Option<(u32, Vec<u8>)> = None;
    for _ in 0..8 {
        let mut buf = vec![0u8; len.min(1 << 20) as usize];
        f.read_exact_at(&mut buf, loc * bs)?;
        let mut next = None;
        let mut i = 0;
        while i + 16 <= buf.len() {
            let b = &buf[i..];
            let mut step = bs as usize;
            match tag(b) {
                // A partition descriptor (the latest of each number).
                Some(5) if b.len() >= 196 => {
                    let (seq, num, start, plen) = (u32le(b, 16), u16le(b, 22), u32le(b, 188) as u64, u32le(b, 192) as u64);
                    match parts.iter_mut().find(|p| p.0 == num) {
                        Some(p) if p.3 <= seq => *p = (num, start, plen, seq),
                        Some(_) => {}
                        None => parts.push((num, start, plen, seq)),
                    }
                }
                // The logical volume descriptor, its partition maps after it.
                Some(6) if b.len() >= 440 => {
                    let seq = u32le(b, 16);
                    let n = (u32le(b, 264) as usize).min(b.len() - 440);
                    if lvd.as_ref().is_none_or(|(s, _)| *s <= seq) {
                        lvd = Some((seq, b[..440 + n].to_vec()));
                    }
                    step = (440 + n).div_ceil(bs as usize) * bs as usize;
                }
                // A pointer to the sequence's next extent.
                Some(3) if b.len() >= 28 => {
                    next = Some((u32le(b, 20) as u64, u32le(b, 24) as u64));
                    break;
                }
                Some(8) => break,
                _ => {}
            }
            i += step;
        }
        match next {
            Some((l, at)) => (len, loc) = (l, at),
            None => break,
        }
    }
    let lvd = lvd.ok_or_else(|| bad("no logical volume"))?.1;
    if parts.is_empty() {
        return Err(bad("no partition"));
    }
    Ok((parts.into_iter().map(|(n, s, l, _)| (n, s, l)).collect(), lvd))
}

impl Udf {
    pub fn open(path: &Path) -> io::Result<Udf> {
        let f = File::open(path)?;
        if !is_udf(&f) {
            return Err(bad("no UDF volume"));
        }
        let size = f.metadata()?.len();
        // The anchor: at block 256, else the last block or 256 before it;
        // where it is gives the block size.
        let mut anchor = None;
        'sizes: for bs in [2048u64, 512, 4096, 1024, 8192, 16384, 32768] {
            let n = size / bs;
            for at in [256, n.saturating_sub(1), n.saturating_sub(257)] {
                let mut b = [0u8; 32];
                if at > 0 && f.read_exact_at(&mut b, at * bs).is_ok() && tag(&b) == Some(2) && u32le(&b, 12) as u64 == at {
                    anchor = Some((bs, (u32le(&b, 16), u32le(&b, 20)), (u32le(&b, 24), u32le(&b, 28))));
                    break 'sizes;
                }
            }
        }
        let (bs, main, reserve) = anchor.ok_or_else(|| bad("no anchor"))?;
        let (parts, lvd) = sequence(&f, bs, main).or_else(|_| sequence(&f, bs, reserve))?;
        if u32le(&lvd, 212) as u64 != bs {
            return Err(bad("blocks of two sizes"));
        }
        let start_of = |num: u16| parts.iter().find(|p| p.0 == num).map(|p| (p.1, p.2)).ok_or_else(|| bad("a map to no partition"));
        // The partition maps: physical ones first, the others once they're
        // there to be read through.
        let table = &lvd[440..];
        let mut u = Udf { f, bs, maps: Vec::new(), root: Lb { part: 0, lbn: 0 } };
        let mut later = Vec::new();
        let mut i = 0;
        for _ in 0..u32le(&lvd, 268).min(64) {
            let Some(&[ty, ml]) = table.get(i..i + 2) else { break };
            let (ty, ml) = (ty, ml as usize);
            let m = table.get(i..i + ml).filter(|_| ml >= 6).ok_or_else(|| bad("a partition map runs over"))?;
            match ty {
                1 => {
                    let num = u16le(m, 4);
                    u.maps.push(Map::Physical { num, start: start_of(num)?.0 });
                }
                2 if ml >= 64 => {
                    let num = u16le(m, 38);
                    let id = &m[5..28];
                    if id.starts_with(b"*UDF Sparable Partition") {
                        u.maps.push(Map::Physical { num, start: start_of(num)?.0 });
                    } else {
                        later.push((u.maps.len(), id.to_vec(), num, m.to_vec()));
                        u.maps.push(Map::Unknown);
                    }
                }
                _ => u.maps.push(Map::Unknown),
            }
            i += ml;
        }
        let physical = |maps: &[Map], num: u16| maps.iter().position(|m| matches!(m, Map::Physical { num: n, .. } if *n == num));
        for (at, id, num, m) in later {
            // The physical partition this one is laid on, by its number (a
            // map of its own added when no map names it).
            let (start, plen) = start_of(num)?;
            let pref = physical(&u.maps, num).unwrap_or_else(|| {
                u.maps.push(Map::Physical { num, start });
                u.maps.len() - 1
            });
            if id.starts_with(b"*UDF Virtual Partition") {
                let table = u.vat(pref, start, plen)?;
                u.maps[at] = Map::Virtual { start, table };
            } else if id.starts_with(b"*UDF Metadata Partition") {
                let runs = [u32le(&m, 40), u32le(&m, 44)].into_iter().find_map(|lbn| u.metadata_runs(Lb { part: pref as u16, lbn }, start).ok());
                u.maps[at] = Map::Metadata(runs.ok_or_else(|| bad("its metadata file can't be read"))?);
            }
        }
        // The file set descriptor, and in it the root directory's entry.
        let fsd = u.block(long_ad(&lvd, 248))?;
        if tag(&fsd) != Some(256) {
            return Err(bad("no file set"));
        }
        u.root = long_ad(&fsd, 400);
        Ok(u)
    }

    /// A virtual partition's allocation table: in the last file entry
    /// written (the image's last blocks).
    fn vat(&self, pref: usize, start: u64, plen: u64) -> io::Result<Vec<u32>> {
        let end = (self.f.metadata()?.len() / self.bs).min(start + plen.max(1));
        for back in 1..=32u64 {
            let Some(blk) = end.checked_sub(back).filter(|b| *b >= start) else { break };
            let at = Lb { part: pref as u16, lbn: (blk - start) as u32 };
            let Ok(b) = self.block(at) else { continue };
            if !matches!(tag(&b), Some(261 | 266)) {
                continue;
            }
            let ty = b[16 + 11];
            let Ok(n) = self.node(at, true) else { continue };
            let mut d = Vec::new();
            let Ok(r) = self.reader(&n) else { continue };
            r.take(64 << 20).read_to_end(&mut d)?;
            let entries = |b: &[u8]| b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect::<Vec<u32>>();
            if ty == 248 && d.len() >= 152 {
                // UDF 2.00 on: a header, then the table.
                let hl = (u16le(&d, 0) as usize).min(d.len());
                return Ok(entries(&d[hl..]));
            }
            if ty == 0 && d.len() >= 36 && d[d.len() - 35..].starts_with(b"*UDF Virtual Alloc Tbl") {
                // UDF 1.50: the table, then an identifier.
                return Ok(entries(&d[..d.len() - 36]));
            }
        }
        Err(bad("no virtual allocation table"))
    }

    /// The metadata partition's runs, from its file's entry.
    fn metadata_runs(&self, at: Lb, start: u64) -> io::Result<Vec<(u64, u64, u64)>> {
        let n = self.node(at, true)?;
        let Data::Extents(exts) = n.data else { return Err(bad("metadata held in its entry")) };
        let mut runs = Vec::new();
        let mut first = 0u64;
        for e in exts {
            let blocks = e.len.div_ceil(self.bs);
            if e.recorded {
                runs.push((first, blocks, start + e.at.lbn as u64));
            }
            first += blocks;
        }
        Ok(runs)
    }

    /// Where logical block `at` is in the image, and how many blocks
    /// after it (up to `want`) follow it there.
    fn place(&self, at: Lb, want: u64) -> io::Result<(u64, u64)> {
        let lbn = at.lbn as u64;
        match self.maps.get(at.part as usize) {
            Some(Map::Physical { start, .. }) => Ok((start + lbn, want)),
            Some(Map::Virtual { start, table }) => match table.get(lbn as usize) {
                Some(&p) if p != u32::MAX => Ok((start + p as u64, 1)),
                _ => Err(bad("a block the allocation table hasn't")),
            },
            Some(Map::Metadata(runs)) => runs.iter().find(|r| (r.0..r.0 + r.1).contains(&lbn)).map(|r| (r.2 + lbn - r.0, want.min(r.0 + r.1 - lbn))).ok_or_else(|| bad("a block outside the metadata")),
            _ => Err(bad("a partition this doesn't read")),
        }
    }

    fn block(&self, at: Lb) -> io::Result<Vec<u8>> {
        let (blk, _) = self.place(at, 1)?;
        let mut b = vec![0u8; self.bs as usize];
        self.f.read_exact_at(&mut b, blk * self.bs)?;
        Ok(b)
    }

    /// The file entry at `at` (the latest of a chain of them); `any`
    /// keeps the volume's own files.
    fn node(&self, mut at: Lb, any: bool) -> io::Result<Node> {
        for _ in 0..16 {
            let b = self.block(at)?;
            match tag(&b) {
                // An indirect entry: the entry is where it points.
                Some(259) => at = long_ad(&b, 36),
                Some(id @ (261 | 266)) => {
                    // Strategy 4096: a newer entry may follow, pointed to
                    // from the next block.
                    if u16le(&b, 16 + 4) == 4096 {
                        let next = Lb { part: at.part, lbn: at.lbn.wrapping_add(1) };
                        if let Ok(nb) = self.block(next)
                            && tag(&nb) == Some(259)
                            && u32le(&nb, 36) & 0x3fff_ffff != 0
                            && long_ad(&nb, 36) != at
                        {
                            at = long_ad(&nb, 36);
                            continue;
                        }
                    }
                    return self.entry_of(&b, id == 266, at, any);
                }
                _ => return Err(bad("not a file entry")),
            }
        }
        Err(bad("a chain of entries too long"))
    }

    fn entry_of(&self, b: &[u8], extended: bool, at: Lb, any: bool) -> io::Result<Node> {
        let (ty, flags) = (b[16 + 11], u16le(b, 16 + 18));
        let kind = match ty {
            4 => Kind::Dir,
            0 | 5 | 249 => Kind::File,
            12 => Kind::Symlink,
            _ if any => Kind::File,
            _ => Kind::Other,
        };
        let (mtime_at, ea_at) = if extended { (92, 208) } else { (84, 168) };
        let size = u64le(b, 56);
        let (lea, lad) = (u32le(b, ea_at) as usize, u32le(b, ea_at + 4) as usize);
        let ads_at = ea_at + 8 + lea;
        let ads = ads_at.checked_add(lad).and_then(|end| b.get(ads_at..end)).ok_or_else(|| bad("an entry runs over"))?;
        let data = match flags & 7 {
            3 => Data::Inline(ads[..(size as usize).min(ads.len())].to_vec()),
            t => {
                let mut exts = Vec::new();
                self.extents(ads, t, at.part, 0, &mut exts)?;
                Data::Extents(exts)
            }
        };
        // Read, write and execute for owner, group and others, from the
        // five bits each that UDF keeps; none when the owner couldn't read.
        let p = u32le(b, 44);
        let mut mode = ((p >> 10) & 7) << 6 | ((p >> 5) & 7) << 3 | (p & 7);
        for (bit, m) in [(0x40, 0o4000), (0x80, 0o2000), (0x100, 0o1000)] {
            if flags & bit != 0 {
                mode |= m;
            }
        }
        let mode = (mode & 0o400 != 0).then_some(mode);
        Ok(Node { kind, mode, size, mtime: time(&b[mtime_at..mtime_at + 12]), data })
    }

    /// Allocation descriptors (short, long or extended) as extents,
    /// following their continuations.
    fn extents(&self, mut ads: &[u8], ty: u16, part: u16, depth: u32, out: &mut Vec<Ext>) -> io::Result<()> {
        let size = match ty {
            0 => 8,
            1 => 16,
            2 => 20,
            _ => return Err(bad("an unknown allocation descriptor")),
        };
        while ads.len() >= size {
            let raw = u32le(ads, 0);
            let (kind, len) = (raw >> 30, (raw & 0x3fff_ffff) as u64);
            if len == 0 {
                break;
            }
            let at = match ty {
                0 => Lb { lbn: u32le(ads, 4), part },
                1 => long_ad(ads, 0),
                _ => Lb { lbn: u32le(ads, 12), part: u16le(ads, 16) },
            };
            if kind == 3 {
                if depth >= 64 {
                    return Err(bad("allocation descriptors chained too far"));
                }
                let b = self.block(at)?;
                if tag(&b) != Some(258) {
                    return Err(bad("a lost allocation extent"));
                }
                let n = (u32le(&b, 20) as usize).min(b.len() - 24);
                return self.extents(&b[24..24 + n], ty, part, depth + 1, out);
            }
            if out.len() >= 1 << 20 {
                return Err(bad("too many extents"));
            }
            out.push(Ext { at, len, recorded: kind == 0 });
            ads = &ads[size..];
        }
        Ok(())
    }

    /// A file's data, `size` bytes of it.
    fn reader(&self, n: &Node) -> io::Result<Box<dyn Read + '_>> {
        let exts = match &n.data {
            Data::Inline(d) => return Ok(Box::new(io::Cursor::new(d.clone()))),
            Data::Extents(e) => e,
        };
        // Runs of the image (or of zeros), in order.
        let mut runs = Vec::new();
        let mut left = n.size;
        for e in exts {
            let len = e.len.min(left);
            if len == 0 {
                break;
            }
            left -= len;
            if !e.recorded {
                runs.push((None, len));
                continue;
            }
            let mut done = 0;
            while done < len {
                let skip = done / self.bs;
                let (blk, count) = self.place(Lb { part: e.at.part, lbn: e.at.lbn.wrapping_add(skip as u32) }, (len - done).div_ceil(self.bs))?;
                let take = (count * self.bs).min(len - done);
                runs.push((Some(blk * self.bs), take));
                done += take;
                if runs.len() > 1 << 22 {
                    return Err(bad("a file in too many pieces"));
                }
            }
        }
        Ok(Box::new(Runs { f: &self.f, runs, i: 0, at: 0 }))
    }

    /// A directory's names: (name, entry), not the parent, nothing deleted.
    fn children(&self, dir: &Node) -> io::Result<Vec<(String, Lb)>> {
        let mut d = Vec::new();
        self.reader(dir)?.take(64 << 20).read_to_end(&mut d)?;
        let mut out = Vec::new();
        let mut i = 0;
        while i + 38 <= d.len() {
            let b = &d[i..];
            if tag(b) != Some(257) {
                break;
            }
            let (chars, lfi, liu) = (b[18], b[19] as usize, u16le(b, 36) as usize);
            let Some(raw) = b.get(38 + liu..38 + liu + lfi) else { break };
            if chars & 0x0c == 0 && lfi > 0 {
                let name = cs0(raw).replace(['/', '\0'], "_");
                if !name.is_empty() && name != "." && name != ".." {
                    out.push((name, long_ad(b, 20)));
                }
            }
            i += (38 + liu + lfi).div_ceil(4) * 4;
        }
        Ok(out)
    }

    /// A symbolic link's target, from its path components.
    fn link(&self, n: &Node) -> io::Result<String> {
        let mut d = Vec::new();
        self.reader(n)?.take(64 << 10).read_to_end(&mut d)?;
        let (mut parts, mut absolute) = (Vec::new(), false);
        let mut i = 0;
        while i + 4 <= d.len() {
            let (ty, l) = (d[i], d[i + 1] as usize);
            let id = d.get(i + 4..i + 4 + l).unwrap_or_default();
            match ty {
                1 | 2 => {
                    absolute = true;
                    parts.clear();
                }
                3 => parts.push("..".to_owned()),
                4 => parts.push(".".to_owned()),
                5 => parts.push(cs0(id)),
                _ => {}
            }
            i += 4 + l;
        }
        Ok(format!("{}{}", if absolute { "/" } else { "" }, parts.join("/")))
    }

    /// Every file and folder, with its path, depth first.
    fn walk(&self) -> io::Result<Vec<(String, Node)>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(String::new(), self.root, self.node(self.root, false)?, 0u32)];
        while let Some((prefix, at, dir, depth)) = stack.pop() {
            if !seen.insert(at) || depth > 64 {
                continue;
            }
            let mut kids = self.children(&dir)?;
            kids.sort_by(|a, b| b.0.cmp(&a.0));
            for (name, at) in kids {
                // (An entry that can't be read is left out, not the rest.)
                let Ok(n) = self.node(at, false) else { continue };
                if n.kind == Kind::Other {
                    continue;
                }
                let p = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                if n.kind == Kind::Dir {
                    stack.push((p.clone(), at, n.clone(), depth + 1));
                }
                out.push((p, n));
                if out.len() > 4 << 20 {
                    return Err(bad("too many files"));
                }
            }
        }
        Ok(out)
    }

    /// Its partition maps' kinds, for the tests.
    #[cfg(test)]
    pub(crate) fn map_kinds(&self) -> Vec<&'static str> {
        self.maps
            .iter()
            .map(|m| match m {
                Map::Physical { .. } => "physical",
                Map::Virtual { .. } => "virtual",
                Map::Metadata(_) => "metadata",
                Map::Unknown => "unknown",
            })
            .collect()
    }

    fn entry(&self, name: String, n: &Node) -> Entry {
        let kind = match n.kind {
            Kind::Dir => EntryKind::Dir,
            Kind::Symlink => EntryKind::Symlink(self.link(n).unwrap_or_default()),
            _ => EntryKind::File,
        };
        let size = if n.kind == Kind::File { n.size } else { 0 };
        Entry { path: safe_path(&name), name, kind, size, mode: n.mode.map(|m| m & 0o7777), mtime: n.mtime, encrypted: false }
    }
}

/// Runs of the image, or of zeros, read one after another.
struct Runs<'a> {
    f: &'a File,
    runs: Vec<(Option<u64>, u64)>,
    i: usize,
    at: u64,
}

impl Read for Runs<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while let Some(&(off, len)) = self.runs.get(self.i) {
            if self.at >= len {
                (self.i, self.at) = (self.i + 1, 0);
                continue;
            }
            let n = buf.len().min((len - self.at) as usize);
            let k = match off {
                Some(o) => self.f.read_at(&mut buf[..n], o + self.at)?,
                None => {
                    buf[..n].fill(0);
                    n
                }
            };
            if k == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.at += k as u64;
            return Ok(k);
        }
        Ok(0)
    }
}

pub fn list(u: &Udf) -> io::Result<Vec<Entry>> {
    Ok(u.walk()?.into_iter().map(|(name, n)| u.entry(name, &n)).collect())
}

pub fn extract(u: &Udf, sink: &mut Sink, ctl: &Control) -> Result<()> {
    let nodes = u.walk()?;
    ctl.total(nodes.iter().filter(|(_, n)| n.kind == Kind::File).map(|(_, n)| n.size).sum());
    for (name, n) in nodes {
        let Some(rel) = safe_path(&name).and_then(|r| sink.map(r)) else { continue };
        ctl.item(&name)?;
        let mode = n.mode.map(|m| m & 0o7777);
        match n.kind {
            Kind::Symlink => {
                if let Ok(l) = u.link(&n) {
                    sink.symlink(&rel, &l);
                }
            }
            Kind::Dir => sink.dir(&rel, mode, n.mtime)?,
            _ => {
                let mut data = ctl.counted(u.reader(&n)?);
                sink.file(&rel, &mut data, mode, n.mtime).map_err(|e| stopped_or(e, ctl))?;
            }
        }
    }
    Ok(())
}
