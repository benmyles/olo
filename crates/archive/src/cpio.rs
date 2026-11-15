//! cpio (read): the "new" ASCII headers (070701, 070702: initramfs,
//! RPM's payload) and the portable old ones (070707), plain or in a
//! compression.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{Codec, Control, Entry, EntryKind, Error, Result, safe_path, sink::Sink, stopped_or};

pub fn is_cpio(h: &[u8]) -> bool {
    h.starts_with(b"070701") || h.starts_with(b"070702") || h.starts_with(b"070707")
}

struct Header {
    name: String,
    mode: u32,
    mtime: i64,
    size: u64,
    /// Bytes of padding after the data.
    pad: u64,
}

fn field(b: &[u8], radix: u32) -> io::Result<u64> {
    let s = std::str::from_utf8(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a cpio header that isn't text"))?;
    u64::from_str_radix(s, radix).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("“{s}” isn't a number")))
}

fn skip(r: &mut impl Read, n: u64) -> io::Result<()> {
    io::copy(&mut r.take(n), &mut io::sink())?;
    Ok(())
}

/// The next header, or None after the trailer.
fn next(r: &mut impl Read) -> io::Result<Option<Header>> {
    let mut magic = [0u8; 6];
    if crate::read_up_to(r, &mut magic)? < 6 {
        return Ok(None);
    }
    let (namesize, mode, mtime, size, newc) = match &magic {
        b"070701" | b"070702" => {
            let mut h = [0u8; 104];
            r.read_exact(&mut h)?;
            let f = |i: usize| field(&h[i * 8..i * 8 + 8], 16);
            (f(11)?, f(1)? as u32, f(5)? as i64, f(6)?, true)
        }
        b"070707" => {
            let mut h = [0u8; 70];
            r.read_exact(&mut h)?;
            // dev ino mode uid gid nlink rdev (6 each), mtime 11, namesize 6, filesize 11
            (field(&h[53..59], 8)?, field(&h[12..18], 8)? as u32, field(&h[42..53], 8)? as i64, field(&h[59..70], 8)?, false)
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "not a cpio header")),
    };
    if namesize == 0 || namesize > 64 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a cpio name of no length"));
    }
    let mut name = vec![0u8; namesize as usize];
    r.read_exact(&mut name)?;
    if newc {
        let used = 110 + namesize;
        skip(r, (4 - used % 4) % 4)?;
    }
    let name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())]).into_owned();
    if name == "TRAILER!!!" {
        return Ok(None);
    }
    let pad = if newc { (4 - size % 4) % 4 } else { 0 };
    Ok(Some(Header { name, mode, mtime, size, pad }))
}

fn kind_of(mode: u32) -> Option<EntryKind> {
    match mode & 0o170000 {
        0o040000 => Some(EntryKind::Dir),
        0o100000 => Some(EntryKind::File),
        0o120000 => Some(EntryKind::Symlink(String::new())),
        _ => None,
    }
}

pub fn list(path: &Path, codec: Codec, cancel: &AtomicBool) -> Result<Vec<Entry>> {
    let mut r = codec.reader(File::open(path)?)?;
    let mut out = Vec::new();
    while let Some(h) = next(&mut r).map_err(|e| Error::Bad(e.to_string()))? {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut kind = kind_of(h.mode);
        if let Some(EntryKind::Symlink(_)) = kind {
            let mut t = vec![0u8; h.size.min(4096) as usize];
            r.read_exact(&mut t)?;
            skip(&mut r, h.size - t.len() as u64 + h.pad)?;
            kind = Some(EntryKind::Symlink(String::from_utf8_lossy(&t).into_owned()));
        } else {
            skip(&mut r, h.size + h.pad)?;
        }
        let Some(kind) = kind else { continue };
        let name = h.name.trim_start_matches("./").to_owned();
        if name.is_empty() || name == "." {
            continue;
        }
        out.push(Entry { path: safe_path(&name), name, kind, size: h.size, mode: Some(h.mode & 0o7777), mtime: Some(h.mtime), encrypted: false });
    }
    Ok(out)
}

pub fn extract(path: &Path, codec: Codec, sink: &mut Sink, ctl: &Control) -> Result<()> {
    ctl.total(std::fs::metadata(path)?.len());
    let mut r = codec.reader(ctl.counted(File::open(path)?))?;
    while let Some(h) = next(&mut r).map_err(|e| stopped_or(e, ctl))? {
        let name = h.name.trim_start_matches("./").to_owned();
        let rel = safe_path(&name).and_then(|r| sink.map(r));
        let (mode, mtime) = (Some(h.mode & 0o7777), Some(h.mtime));
        match (kind_of(h.mode), rel) {
            (Some(EntryKind::Dir), Some(rel)) => {
                ctl.item(&name)?;
                sink.dir(&rel, mode, mtime)?;
                skip(&mut r, h.size + h.pad)?;
            }
            (Some(EntryKind::File), Some(rel)) => {
                ctl.item(&name)?;
                let mut data = (&mut r).take(h.size);
                sink.file(&rel, &mut data, mode, mtime).map_err(|e| stopped_or(e, ctl))?;
                skip(&mut r, h.pad)?;
            }
            (Some(EntryKind::Symlink(_)), Some(rel)) => {
                let mut t = vec![0u8; h.size.min(4096) as usize];
                r.read_exact(&mut t)?;
                skip(&mut r, h.size - t.len() as u64 + h.pad)?;
                sink.symlink(&rel, &String::from_utf8_lossy(&t));
            }
            _ => skip(&mut r, h.size + h.pad)?,
        }
    }
    Ok(())
}
