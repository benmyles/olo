//! WOFF 1.0 and WOFF2 (W3C) to the sfnt they wrap, for `@font-face` sources.
//! Everything is bounds-checked against the declared sizes; a malformed file
//! is an error, never a panic or an unbounded allocation.
use super::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
/// The largest font a page may load, decoded.
pub const MAX_SFNT: usize = 32 * 1024 * 1024;
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or(Error::Invalid)?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    /// WOFF2's variable-length unsigned integer.
    fn base128(&mut self) -> Result<u32> {
        let mut value: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            if i == 0 && b == 0x80 {
                return Err(Error::Invalid);
            }
            if value & 0xFE00_0000 != 0 {
                return Err(Error::Invalid);
            }
            value = (value << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Invalid)
    }
    /// WOFF2's 255UInt16.
    fn u255(&mut self) -> Result<u16> {
        let code = self.u8()?;
        Ok(match code {
            253 => self.u16()?,
            255 => u16::from(self.u8()?).checked_add(253).ok_or(Error::Invalid)?,
            254 => u16::from(self.u8()?).checked_add(253 * 2).ok_or(Error::Invalid)?,
            v => u16::from(v),
        })
    }
}
const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm", b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH",
    b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC", b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar", b"bdat", b"bloc",
    b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty", b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc", b"Feat", b"Sill",
];
struct Table {
    tag: [u8; 4],
    data: Vec<u8>,
}
/// Whether these bytes are a WOFF container.
pub fn is_woff(data: &[u8]) -> bool {
    data.len() >= 4 && matches!(&data[..4], b"wOFF" | b"wOF2")
}
/// Decode a WOFF or WOFF2 file to the sfnt (or TrueType collection) inside.
pub fn decode(data: &[u8], cancelled: Option<&AtomicBool>) -> Result<Vec<u8>> {
    match data.get(..4).ok_or(Error::Invalid)? {
        b"wOFF" => woff1(data, cancelled),
        b"wOF2" => woff2(data, cancelled),
        _ => Err(Error::Invalid),
    }
}
fn woff1(data: &[u8], cancelled: Option<&AtomicBool>) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    r.bytes(4)?;
    let flavor = r.u32()?;
    let _length = r.u32()?;
    let count = r.u16()? as usize;
    r.u16()?;
    let total = r.u32()? as usize;
    if total > MAX_SFNT || count > 512 {
        return Err(Error::Limit);
    }
    r.bytes(24)?; // versions, metadata and private blocks
    let mut tables = Vec::with_capacity(count);
    let mut budget = MAX_SFNT;
    for _ in 0..count {
        check(cancelled)?;
        let tag: [u8; 4] = r.bytes(4)?.try_into().unwrap();
        let offset = r.u32()? as usize;
        let compressed = r.u32()? as usize;
        let original = r.u32()? as usize;
        r.u32()?; // original checksum
        let end = offset.checked_add(compressed).filter(|&e| e <= data.len()).ok_or(Error::Invalid)?;
        budget = budget.checked_sub(original).ok_or(Error::Limit)?;
        let bytes = &data[offset..end];
        let table = if compressed < original {
            use std::io::Read;
            let mut out = Vec::with_capacity(original);
            flate2::read::ZlibDecoder::new(bytes).take(original as u64 + 1).read_to_end(&mut out).map_err(|_| Error::Invalid)?;
            if out.len() != original {
                return Err(Error::Invalid);
            }
            out
        } else {
            if compressed != original {
                return Err(Error::Invalid);
            }
            bytes.to_vec()
        };
        tables.push(Table { tag, data: table });
    }
    Ok(sfnt(flavor, tables))
}
struct Entry {
    tag: [u8; 4],
    transformed: bool,
    length: usize,
    original: usize,
}
fn woff2(data: &[u8], cancelled: Option<&AtomicBool>) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    r.bytes(4)?;
    let flavor = r.u32()?;
    let _length = r.u32()?;
    let count = r.u16()? as usize;
    r.u16()?;
    let total = r.u32()? as usize;
    let compressed_size = r.u32()? as usize;
    r.bytes(24)?;
    if total > MAX_SFNT || count > 512 {
        return Err(Error::Limit);
    }
    let mut entries = Vec::with_capacity(count);
    let mut decompressed = 0usize;
    for _ in 0..count {
        check(cancelled)?;
        let flags = r.u8()?;
        let tag: [u8; 4] = if flags & 0x3F == 0x3F { r.bytes(4)?.try_into().unwrap() } else { *KNOWN_TAGS[(flags & 0x3F) as usize] };
        let version = flags >> 6;
        let original = r.base128()? as usize;
        // glyf and loca are transformed at version 0; other tables at 1+.
        let transformed = if &tag == b"glyf" || &tag == b"loca" { version == 0 } else { version != 0 };
        let length = if transformed { r.base128()? as usize } else { original };
        decompressed = decompressed.checked_add(length).ok_or(Error::Limit)?;
        if decompressed > MAX_SFNT || original > MAX_SFNT {
            return Err(Error::Limit);
        }
        entries.push(Entry { tag, transformed, length, original });
    }
    // A collection lists which tables each font uses.
    let mut fonts: Vec<(u32, Vec<usize>)> = Vec::new();
    if flavor == 0x7474_6366 {
        let _version = r.u32()?;
        let n = r.u255()? as usize;
        if n > 256 {
            return Err(Error::Limit);
        }
        for _ in 0..n {
            let tables = r.u255()? as usize;
            let font_flavor = r.u32()?;
            let mut indices = Vec::with_capacity(tables);
            for _ in 0..tables {
                let i = r.u255()? as usize;
                if i >= entries.len() {
                    return Err(Error::Invalid);
                }
                indices.push(i);
            }
            fonts.push((font_flavor, indices));
        }
    }
    let compressed = r.bytes(compressed_size).or_else(|_| Ok::<_, Error>(&data[r.at.min(data.len())..]))?;
    check(cancelled)?;
    let mut stream = Vec::with_capacity(decompressed.min(MAX_SFNT));
    {
        use std::io::Read;
        let mut reader = brotli_decompressor::Decompressor::new(compressed, 64 * 1024);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            check(cancelled)?;
            let n = match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Invalid),
            };
            if stream.len() + n > decompressed {
                return Err(Error::Invalid);
            }
            stream.extend_from_slice(&chunk[..n]);
        }
    }
    if stream.len() != decompressed {
        return Err(Error::Invalid);
    }
    // Slice the stream into tables, reconstructing the transformed ones.
    let mut tables: Vec<Table> = Vec::with_capacity(entries.len());
    let mut at = 0usize;
    let mut glyf_index = None;
    let mut loca_index = None;
    let mut x_mins: Vec<i16> = Vec::new();
    let mut loca_format = 0u16;
    for (i, e) in entries.iter().enumerate() {
        check(cancelled)?;
        let bytes = stream.get(at..at + e.length).ok_or(Error::Invalid)?;
        at += e.length;
        let table = match (&e.tag, e.transformed) {
            (b"glyf", true) => {
                let rebuilt = reconstruct_glyf(bytes, cancelled)?;
                x_mins = rebuilt.x_mins;
                loca_format = rebuilt.long;
                glyf_index = Some(tables.len());
                tables.push(Table { tag: *b"glyf", data: rebuilt.glyf });
                loca_index = Some(tables.len());
                tables.push(Table { tag: *b"loca", data: rebuilt.loca });
                continue;
            }
            (b"loca", true) => {
                // Reconstructed with glyf; the directory still lists it.
                if loca_index.is_none() || e.length != 0 {
                    return Err(Error::Invalid);
                }
                continue;
            }
            (b"hmtx", true) => Table { tag: *b"hmtx", data: Vec::new() }, // Needs hhea/maxp: done below.
            _ => {
                if bytes.len() != e.original {
                    return Err(Error::Invalid);
                }
                Table { tag: e.tag, data: bytes.to_vec() }
            }
        };
        let _ = i;
        tables.push(table);
    }
    // hmtx transform needs the glyph count, metric count and glyph x minimums.
    if let Some(hi) = tables.iter().position(|t| &t.tag == b"hmtx" && t.data.is_empty()) {
        let entry = entries.iter().find(|e| &e.tag == b"hmtx").ok_or(Error::Invalid)?;
        let start: usize = entries.iter().take_while(|e| &e.tag != b"hmtx").map(|e| e.length).sum();
        let bytes = stream.get(start..start + entry.length).ok_or(Error::Invalid)?;
        let hhea = tables.iter().find(|t| &t.tag == b"hhea").ok_or(Error::Invalid)?;
        let maxp = tables.iter().find(|t| &t.tag == b"maxp").ok_or(Error::Invalid)?;
        let metrics = u16::from_be_bytes(hhea.data.get(34..36).ok_or(Error::Invalid)?.try_into().unwrap()) as usize;
        let glyphs = u16::from_be_bytes(maxp.data.get(4..6).ok_or(Error::Invalid)?.try_into().unwrap()) as usize;
        tables[hi].data = reconstruct_hmtx(bytes, metrics, glyphs, &x_mins)?;
    }
    if let (Some(_), Some(head)) = (glyf_index, tables.iter_mut().find(|t| &t.tag == b"head")) {
        // The loca format is the transform's, whatever head said.
        if head.data.len() >= 52 {
            head.data[50..52].copy_from_slice(&loca_format.to_be_bytes());
        }
    }
    if fonts.is_empty() {
        return Ok(sfnt(flavor, tables));
    }
    // Collection: the directory's table indices map onto the reconstructed
    // tables (a transformed glyf stands for its loca too).
    let mut map = Vec::with_capacity(entries.len());
    let mut ti = 0;
    for e in &entries {
        if &e.tag == b"loca" && e.transformed {
            map.push(loca_index.ok_or(Error::Invalid)?);
            continue;
        }
        map.push(ti);
        ti += 1;
        if &e.tag == b"glyf" && e.transformed {
            ti += 1; // the loca it produced
        }
    }
    collection(&fonts, &map, tables)
}
/// glyf and loca rebuilt from the transformed streams, with each glyph's
/// x minimum (for hmtx) and whether loca uses long offsets.
struct Glyf {
    glyf: Vec<u8>,
    loca: Vec<u8>,
    x_mins: Vec<i16>,
    long: u16,
}
/// Rebuild glyf and loca from the WOFF2 transformed glyf streams.
fn reconstruct_glyf(data: &[u8], cancelled: Option<&AtomicBool>) -> Result<Glyf> {
    let mut r = Reader::new(data);
    let _version = r.u32()?;
    let glyphs = r.u16()? as usize;
    let index_format = r.u16()?;
    let mut sizes = [0usize; 7];
    for s in &mut sizes {
        *s = r.u32()? as usize;
    }
    let mut streams: Vec<Reader<'_>> = Vec::with_capacity(7);
    for &s in &sizes {
        streams.push(Reader::new(r.bytes(s)?));
    }
    let [contours, points, flags, glyph, composite, bbox, instructions] = streams.as_mut_slice() else { unreachable!() };
    let bitmap_len = ((glyphs + 31) >> 5) << 2;
    let bitmap = bbox.bytes(bitmap_len)?;
    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(glyphs + 1);
    let mut mins = Vec::with_capacity(glyphs);
    for g in 0..glyphs {
        check(cancelled)?;
        offsets.push(glyf.len());
        let n = contours.i16()?;
        let has_bbox = bitmap[g >> 3] & (0x80 >> (g & 7)) != 0;
        if n == 0 {
            mins.push(0);
            continue;
        }
        if n > 0 {
            let n = n as usize;
            let mut ends = Vec::with_capacity(n);
            let mut total = 0usize;
            for _ in 0..n {
                total = total.checked_add(points.u255()? as usize).ok_or(Error::Invalid)?;
                if total > 65535 {
                    return Err(Error::Invalid);
                }
                ends.push(total);
            }
            let mut xs = Vec::with_capacity(total);
            let mut ys = Vec::with_capacity(total);
            let mut on = Vec::with_capacity(total);
            let (mut x, mut y) = (0i32, 0i32);
            for _ in 0..total {
                let f = flags.u8()?;
                on.push(f & 0x80 == 0);
                let (dx, dy) = triplet(f & 0x7F, glyph)?;
                x = x.checked_add(dx).ok_or(Error::Invalid)?;
                y = y.checked_add(dy).ok_or(Error::Invalid)?;
                if !(-32768..=32767).contains(&x) || !(-32768..=32767).contains(&y) {
                    return Err(Error::Invalid);
                }
                xs.push(x as i16);
                ys.push(y as i16);
            }
            let instruction_length = glyph.u255()? as usize;
            let instruction_bytes = instructions.bytes(instruction_length)?;
            let (x_min, y_min, x_max, y_max) = if has_bbox {
                (bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?)
            } else {
                (xs.iter().copied().min().unwrap_or(0), ys.iter().copied().min().unwrap_or(0), xs.iter().copied().max().unwrap_or(0), ys.iter().copied().max().unwrap_or(0))
            };
            mins.push(x_min);
            glyf.extend_from_slice(&(n as i16).to_be_bytes());
            for v in [x_min, y_min, x_max, y_max] {
                glyf.extend_from_slice(&v.to_be_bytes());
            }
            for e in &ends {
                glyf.extend_from_slice(&((e.saturating_sub(1)) as u16).to_be_bytes());
            }
            glyf.extend_from_slice(&(instruction_length as u16).to_be_bytes());
            glyf.extend_from_slice(instruction_bytes);
            // Plain flags and two-byte deltas: valid TrueType, no repeat packing.
            for &o in &on {
                glyf.push(if o { 0x01 } else { 0x00 });
            }
            let mut previous = 0i16;
            for &v in &xs {
                glyf.extend_from_slice(&v.wrapping_sub(previous).to_be_bytes());
                previous = v;
            }
            previous = 0;
            for &v in &ys {
                glyf.extend_from_slice(&v.wrapping_sub(previous).to_be_bytes());
                previous = v;
            }
        } else {
            // Composite: the component records are copied as they are.
            if !has_bbox {
                return Err(Error::Invalid);
            }
            let (x_min, y_min, x_max, y_max) = (bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?);
            mins.push(x_min);
            glyf.extend_from_slice(&n.to_be_bytes());
            for v in [x_min, y_min, x_max, y_max] {
                glyf.extend_from_slice(&v.to_be_bytes());
            }
            let mut has_instructions = false;
            loop {
                let flags = composite.u16()?;
                let glyph_index = composite.u16()?;
                let mut size = 4;
                size += if flags & 0x0001 != 0 { 4 } else { 2 };
                if flags & 0x0008 != 0 {
                    size += 2;
                } else if flags & 0x0040 != 0 {
                    size += 4;
                } else if flags & 0x0080 != 0 {
                    size += 8;
                }
                glyf.extend_from_slice(&flags.to_be_bytes());
                glyf.extend_from_slice(&glyph_index.to_be_bytes());
                glyf.extend_from_slice(composite.bytes(size - 4)?);
                has_instructions |= flags & 0x0100 != 0;
                if flags & 0x0020 == 0 {
                    break;
                }
            }
            if has_instructions {
                let length = glyph.u255()? as usize;
                glyf.extend_from_slice(&(length as u16).to_be_bytes());
                glyf.extend_from_slice(instructions.bytes(length)?);
            }
        }
        if glyf.len() % 4 != 0 {
            glyf.resize(glyf.len() + (4 - glyf.len() % 4), 0);
        }
        if glyf.len() > MAX_SFNT {
            return Err(Error::Limit);
        }
    }
    offsets.push(glyf.len());
    let long = index_format != 0 || glyf.len() > 2 * 65535;
    let mut loca = Vec::with_capacity(offsets.len() * if long { 4 } else { 2 });
    for &o in &offsets {
        if long {
            loca.extend_from_slice(&(o as u32).to_be_bytes());
        } else {
            loca.extend_from_slice(&((o / 2) as u16).to_be_bytes());
        }
    }
    Ok(Glyf { glyf, loca, x_mins: mins, long: u16::from(long) })
}
/// WOFF2's triplet coordinate encoding: one flag chooses the byte count and signs.
fn triplet(flag: u8, data: &mut Reader<'_>) -> Result<(i32, i32)> {
    let sign = |f: u8, v: i32| if f & 1 != 0 { v } else { -v };
    let f = i32::from(flag);
    Ok(if flag < 10 {
        let b = i32::from(data.u8()?);
        (0, sign(flag, ((f & 14) << 7) + b))
    } else if flag < 20 {
        let b = i32::from(data.u8()?);
        (sign(flag, (((f - 10) & 14) << 7) + b), 0)
    } else if flag < 84 {
        let b0 = f - 20;
        let b1 = i32::from(data.u8()?);
        (sign(flag, 1 + (b0 & 0x30) + (b1 >> 4)), sign(flag >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)))
    } else if flag < 120 {
        let b0 = f - 84;
        let (b1, b2) = (i32::from(data.u8()?), i32::from(data.u8()?));
        (sign(flag, 1 + ((b0 / 12) << 8) + b1), sign(flag >> 1, 1 + (((b0 % 12) >> 2) << 8) + b2))
    } else if flag < 124 {
        let (b1, b2, b3) = (i32::from(data.u8()?), i32::from(data.u8()?), i32::from(data.u8()?));
        (sign(flag, (b1 << 4) + (b2 >> 4)), sign(flag >> 1, ((b2 & 0x0f) << 8) + b3))
    } else {
        let (b1, b2, b3, b4) = (i32::from(data.u8()?), i32::from(data.u8()?), i32::from(data.u8()?), i32::from(data.u8()?));
        (sign(flag, (b1 << 8) + b2), sign(flag >> 1, (b3 << 8) + b4))
    })
}
fn reconstruct_hmtx(data: &[u8], metrics: usize, glyphs: usize, x_mins: &[i16]) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    let flags = r.u8()?;
    let proportional_omitted = flags & 1 != 0;
    let monospace_omitted = flags & 2 != 0;
    if metrics > glyphs || (proportional_omitted || monospace_omitted) && x_mins.len() < glyphs {
        return Err(Error::Invalid);
    }
    let mut advances = Vec::with_capacity(metrics);
    for _ in 0..metrics {
        advances.push(r.u16()?);
    }
    let mut out = Vec::with_capacity(metrics * 4 + (glyphs - metrics) * 2);
    for (i, a) in advances.iter().enumerate() {
        let lsb = if proportional_omitted { x_mins[i] } else { r.i16()? };
        out.extend_from_slice(&a.to_be_bytes());
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    for x_min in x_mins.iter().copied().chain(std::iter::repeat(0)).take(glyphs).skip(metrics) {
        let lsb = if monospace_omitted { x_min } else { r.i16()? };
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    Ok(out)
}
fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}
fn directory(flavor: u32, tables: &[(&[u8; 4], u32, u32)], out: &mut Vec<u8>) {
    let n = tables.len() as u16;
    let mut entry_selector = 0u16;
    while (2u16 << entry_selector) <= n {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector) * 16;
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(n * 16 - search_range).to_be_bytes());
    for (tag, offset, length) in tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&[0, 0, 0, 0]); // checksum, filled below
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
    }
}
/// Serialise one font: directory sorted by tag, tables four-byte aligned.
fn sfnt(flavor: u32, mut tables: Vec<Table>) -> Vec<u8> {
    tables.sort_by_key(|t| t.tag);
    let header = 12 + 16 * tables.len();
    let mut offset = header;
    let mut placed = Vec::with_capacity(tables.len());
    for t in &tables {
        placed.push((&t.tag, offset as u32, t.data.len() as u32));
        offset += (t.data.len() + 3) & !3;
    }
    let mut out = Vec::with_capacity(offset);
    directory(flavor, &placed, &mut out);
    for (i, t) in tables.iter().enumerate() {
        let sum = checksum(&t.data);
        out[12 + 16 * i + 4..12 + 16 * i + 8].copy_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&t.data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}
/// A TrueType collection sharing table data between its fonts.
fn collection(fonts: &[(u32, Vec<usize>)], map: &[usize], tables: Vec<Table>) -> Result<Vec<u8>> {
    let header = 12 + 4 * fonts.len();
    let mut offset = header + fonts.iter().map(|(_, t)| 12 + 16 * t.len()).sum::<usize>();
    let mut table_offsets = Vec::with_capacity(tables.len());
    for t in &tables {
        table_offsets.push(offset as u32);
        offset += (t.data.len() + 3) & !3;
    }
    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
    let mut font_offset = header;
    for (_, indices) in fonts {
        out.extend_from_slice(&(font_offset as u32).to_be_bytes());
        font_offset += 12 + 16 * indices.len();
    }
    for (flavor, indices) in fonts {
        let mut placed: Vec<(&[u8; 4], u32, u32)> = Vec::with_capacity(indices.len());
        for &i in indices {
            let ti = *map.get(i).ok_or(Error::Invalid)?;
            let t = tables.get(ti).ok_or(Error::Invalid)?;
            placed.push((&t.tag, table_offsets[ti], t.data.len() as u32));
        }
        placed.sort_by(|a, b| a.0.cmp(b.0));
        let start = out.len();
        directory(*flavor, &placed, &mut out);
        for (i, (_, off, _)) in placed.iter().enumerate() {
            let ti = table_offsets.iter().position(|o| o == off).ok_or(Error::Invalid)?;
            let sum = checksum(&tables[ti].data);
            out[start + 12 + 16 * i + 4..start + 12 + 16 * i + 8].copy_from_slice(&sum.to_be_bytes());
        }
    }
    for t in &tables {
        out.extend_from_slice(&t.data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base128_and_255() {
        let mut r = Reader::new(&[0x3F, 0x81, 0x00, 253, 0x01, 0x02, 255, 0x05, 254, 0x05, 7]);
        assert_eq!(r.base128().unwrap(), 0x3F);
        assert_eq!(r.base128().unwrap(), 128);
        assert_eq!(r.u255().unwrap(), 0x0102);
        assert_eq!(r.u255().unwrap(), 258);
        assert_eq!(r.u255().unwrap(), 511);
        assert_eq!(r.u255().unwrap(), 7);
        assert!(Reader::new(&[0x80, 0x01]).base128().is_err());
    }
    #[test]
    fn triplets_follow_the_specification_table() {
        // flag 1 with one byte 5: dy = +(0<<7)+5; flag 0: negative.
        assert_eq!(triplet(1, &mut Reader::new(&[5])).unwrap(), (0, 5));
        assert_eq!(triplet(0, &mut Reader::new(&[5])).unwrap(), (0, -5));
        assert_eq!(triplet(11, &mut Reader::new(&[5])).unwrap(), (5, 0));
        assert_eq!(triplet(127, &mut Reader::new(&[1, 2, 3, 4])).unwrap(), (0x0102, 0x0304));
    }
}
