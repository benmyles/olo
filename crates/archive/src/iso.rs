//! ISO 9660 disc images (read): the primary volume's tree, with Rock
//! Ridge's names, permissions and symbolic links where it has them,
//! else Joliet's long names, else the plain 8.3 ones (";1" taken off).

use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::FileExt,
    path::Path,
};

use crate::{Control, Entry, EntryKind, Result, safe_path, sink::Sink, stopped_or};

const SECTOR: u64 = 2048;

pub fn is_iso(path: &Path) -> bool {
    let Ok(f) = File::open(path) else { return false };
    let mut b = [0u8; 6];
    f.read_exact_at(&mut b, 16 * SECTOR).is_ok() && b[0] == 1 && &b[1..6] == b"CD001"
}

#[derive(Clone)]
struct Rec {
    lba: u64,
    len: u64,
    dir: bool,
    name: String,
    mode: Option<u32>,
    link: Option<String>,
    mtime: Option<i64>,
}

/// A directory record's 7-byte date as Unix time.
fn date(b: &[u8]) -> Option<i64> {
    if b.len() < 7 || b[0] == 0 {
        return None;
    }
    let days = crate::zipfmt::days_from_civil(1900 + b[0] as i64, b[1].max(1) as u32, b[2].max(1) as u32);
    let tz = (b[6] as i8) as i64 * 15 * 60;
    Some(days * 86_400 + b[3] as i64 * 3600 + b[4] as i64 * 60 + b[5] as i64 - tz)
}

/// Rock Ridge's entries in a system use area: (name, mode, link).
fn rock_ridge(f: &File, mut su: Vec<u8>, depth: u32) -> (Option<String>, Option<u32>, Option<String>) {
    let (mut name, mut mode, mut link) = (None::<String>, None, None::<String>);
    let mut i = 0;
    while i + 4 <= su.len() {
        let (sig, len) = (&su[i..i + 2], su[i + 2] as usize);
        if len < 4 || i + len > su.len() {
            break;
        }
        let data = &su[i + 4..i + len];
        match sig {
            b"NM" if !data.is_empty() => {
                let part = String::from_utf8_lossy(&data[1..]).into_owned();
                name = Some(name.unwrap_or_default() + &part);
            }
            b"PX" if data.len() >= 4 => mode = Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
            b"SL" if !data.is_empty() => {
                // Its components: the root (an absolute link), ".", "..",
                // or a name; several SL entries continue one link.
                let (mut parts, mut absolute) = (Vec::new(), false);
                let mut j = 1;
                while j + 2 <= data.len() {
                    let (cf, cl) = (data[j], data[j + 1] as usize);
                    if cf & 0x08 != 0 {
                        absolute = true;
                    } else if cf & 0x02 != 0 {
                        parts.push(".".to_owned());
                    } else if cf & 0x04 != 0 {
                        parts.push("..".to_owned());
                    } else {
                        parts.push(String::from_utf8_lossy(data.get(j + 2..j + 2 + cl).unwrap_or_default()).into_owned());
                    }
                    j += 2 + cl;
                }
                let s = format!("{}{}", if absolute { "/" } else { "" }, parts.join("/"));
                link = Some(match link {
                    Some(before) if !before.is_empty() => format!("{before}/{s}"),
                    _ => s,
                });
            }
            b"CE" if data.len() >= 24 && depth < 4 => {
                let block = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as u64;
                let off = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as u64;
                let n = (u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize).min(64 * 1024);
                let mut more = vec![0u8; n];
                if f.read_exact_at(&mut more, block * SECTOR + off).is_ok() {
                    let (n2, m2, l2) = rock_ridge(f, more, depth + 1);
                    name = name.or(n2);
                    mode = mode.or(m2);
                    link = link.or(l2);
                }
            }
            b"ST" => break,
            _ => {}
        }
        i += len;
    }
    su.clear();
    (name, mode, link)
}

/// The records in a directory's extent.
fn records(f: &File, lba: u64, len: u64, joliet: bool) -> io::Result<Vec<Rec>> {
    let len = len.min(16 << 20);
    let mut buf = vec![0u8; len as usize];
    f.read_exact_at(&mut buf, lba * SECTOR)?;
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let rl = buf[i] as usize;
        if rl == 0 {
            // The rest of this sector is padding.
            i = (i / SECTOR as usize + 1) * SECTOR as usize;
            continue;
        }
        if i + rl > buf.len() || rl < 34 {
            break;
        }
        let r = &buf[i..i + rl];
        let nl = r[32] as usize;
        if 33 + nl > rl {
            break;
        }
        let raw = &r[33..33 + nl];
        if !(nl == 1 && (raw[0] == 0 || raw[0] == 1)) {
            let dir = r[25] & 0x02 != 0;
            let mut name = if joliet {
                let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(raw).into_owned()
            };
            if let Some(i) = name.rfind(';') {
                name.truncate(i);
            }
            if !dir && name.ends_with('.') {
                name.pop();
            }
            let su_at = 33 + nl + (1 - nl % 2);
            let (rr_name, mode, link) = if su_at < rl && !joliet { rock_ridge(f, r[su_at..].to_vec(), 0) } else { (None, None, None) };
            out.push(Rec {
                lba: u32::from_le_bytes([r[2], r[3], r[4], r[5]]) as u64,
                len: u32::from_le_bytes([r[10], r[11], r[12], r[13]]) as u64,
                dir,
                name: rr_name.unwrap_or(name),
                mode,
                link,
                mtime: date(&r[18..25]),
            });
        }
        i += rl;
    }
    Ok(out)
}

/// Every record in the tree, with its path, depth first.
fn walk(path: &Path) -> io::Result<(File, Vec<(String, Rec)>)> {
    let f = File::open(path)?;
    let mut pvd = [0u8; 2048];
    f.read_exact_at(&mut pvd, 16 * SECTOR)?;
    if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an ISO 9660 image"));
    }
    let root_of = |vd: &[u8]| (u32::from_le_bytes([vd[158], vd[159], vd[160], vd[161]]) as u64, u32::from_le_bytes([vd[166], vd[167], vd[168], vd[169]]) as u64);
    let (mut lba, mut len) = root_of(&pvd);
    // Rock Ridge if the primary tree has its entries; else Joliet's tree if there is one.
    let rr = records(&f, lba, len, false)?.iter().any(|r| r.mode.is_some());
    let mut joliet = false;
    if !rr {
        for s in 17..32u64 {
            let mut vd = [0u8; 2048];
            if f.read_exact_at(&mut vd, s * SECTOR).is_err() || vd[0] == 255 {
                break;
            }
            if vd[0] == 2 && &vd[1..6] == b"CD001" && vd[88] == b'%' && vd[89] == b'/' && matches!(vd[90], b'@' | b'C' | b'E') {
                (lba, len) = root_of(&vd);
                joliet = true;
                break;
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![(String::new(), lba, len, 0u32)];
    while let Some((prefix, lba, len, depth)) = stack.pop() {
        if !seen.insert(lba) || depth > 64 {
            continue;
        }
        let mut recs = records(&f, lba, len, joliet)?;
        recs.sort_by(|a, b| b.name.cmp(&a.name));
        for r in recs {
            let p = if prefix.is_empty() { r.name.clone() } else { format!("{prefix}/{}", r.name) };
            if r.dir {
                stack.push((p.clone(), r.lba, r.len, depth + 1));
            }
            out.push((p, r));
        }
    }
    Ok((f, out))
}

fn entry(name: String, r: &Rec) -> Entry {
    let kind = match (&r.link, r.dir) {
        (Some(l), _) => EntryKind::Symlink(l.clone()),
        (None, true) => EntryKind::Dir,
        (None, false) => EntryKind::File,
    };
    Entry { path: safe_path(&name), name, kind, size: if r.dir { 0 } else { r.len }, mode: r.mode.map(|m| m & 0o7777), mtime: r.mtime, encrypted: false }
}

pub fn list(path: &Path) -> Result<Vec<Entry>> {
    let (_, recs) = walk(path).map_err(|e| crate::Error::Bad(e.to_string()))?;
    Ok(recs.into_iter().map(|(n, r)| entry(n, &r)).collect())
}

pub fn extract(path: &Path, sink: &mut Sink, ctl: &Control) -> Result<()> {
    let (mut f, recs) = walk(path).map_err(|e| crate::Error::Bad(e.to_string()))?;
    ctl.total(recs.iter().filter(|(_, r)| !r.dir && r.link.is_none()).map(|(_, r)| r.len).sum());
    for (name, r) in recs {
        let Some(rel) = crate::safe_path(&name).and_then(|r| sink.map(r)) else { continue };
        ctl.item(&name)?;
        let mode = r.mode.map(|m| m & 0o7777);
        if let Some(l) = &r.link {
            sink.symlink(&rel, l);
        } else if r.dir {
            sink.dir(&rel, mode, r.mtime)?;
        } else {
            f.seek(SeekFrom::Start(r.lba * SECTOR))?;
            let mut data = ctl.counted((&mut f).take(r.len));
            sink.file(&rel, &mut data, mode, r.mtime).map_err(|e| stopped_or(e, ctl))?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn read_all(f: &mut File) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}
