//! Tar, plain or in gzip, bzip2, xz or zstd: read and written (GNU tar's
//! headers, long names, permissions, times, symbolic and hard links).

use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

use tar::{Archive, Builder, EntryType, Header, HeaderMode};

use crate::{
    Codec, Control, Entry, EntryKind, Error, Result, safe_path,
    sink::{Sink, WalkKind, Walked},
    stopped_or,
};

fn bad(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::Other || e.kind() == io::ErrorKind::InvalidData || e.kind() == io::ErrorKind::UnexpectedEof { Error::Bad(e.to_string()) } else { Error::Io(e) }
}

pub fn list(path: &Path, codec: Codec, cancel: &std::sync::atomic::AtomicBool) -> Result<Vec<Entry>> {
    let mut a = Archive::new(codec.reader(File::open(path)?)?);
    let mut out = Vec::new();
    for e in a.entries().map_err(bad)? {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let e = e.map_err(bad)?;
        let h = e.header();
        let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        let link = || e.link_name_bytes().map(|l| String::from_utf8_lossy(&l).into_owned()).unwrap_or_default();
        let kind = match h.entry_type() {
            EntryType::Directory => EntryKind::Dir,
            EntryType::Symlink => EntryKind::Symlink(link()),
            EntryType::Link => EntryKind::Hardlink(link()),
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => EntryKind::File,
            _ => continue,
        };
        out.push(Entry { path: safe_path(&name), name, kind, size: h.size().unwrap_or(0), mode: h.mode().ok(), mtime: h.mtime().ok().map(|t| t as i64), encrypted: false });
    }
    Ok(out)
}

pub fn extract(path: &Path, codec: Codec, sink: &mut Sink, ctl: &Control) -> Result<()> {
    // (How far is how much of the file has been read: its size is known.)
    ctl.total(std::fs::metadata(path)?.len());
    let mut a = Archive::new(codec.reader(ctl.counted(File::open(path)?))?);
    for e in a.entries().map_err(|e| stopped_or(e, ctl))? {
        let mut e = e.map_err(|e| stopped_or(e, ctl))?;
        let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        let Some(rel) = safe_path(&name) else { continue };
        ctl.item(&name)?;
        let h = e.header();
        let (kind, mode, mtime) = (h.entry_type(), h.mode().ok(), h.mtime().ok().map(|t| t as i64));
        let link = e.link_name_bytes().map(|l| String::from_utf8_lossy(&l).into_owned());
        match kind {
            EntryType::Directory => sink.dir(&rel, mode, mtime)?,
            EntryType::Symlink => sink.symlink(&rel, &link.unwrap_or_default()),
            EntryType::Link => {
                if let Some(target) = link.as_deref().and_then(safe_path) {
                    sink.hardlink(&rel, target);
                }
            }
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => sink.file(&rel, &mut e, mode, mtime).map_err(|err| stopped_or(err, ctl))?,
            _ => {}
        }
    }
    Ok(())
}

/// A file written through a compression, finished at the end.
enum Out {
    Plain(io::BufWriter<File>),
    Gz(flate2::write::GzEncoder<io::BufWriter<File>>),
    Bz(bzip2::write::BzEncoder<io::BufWriter<File>>),
    Xz(liblzma::write::XzEncoder<io::BufWriter<File>>),
    Zst(zstd::stream::write::Encoder<'static, io::BufWriter<File>>),
}

impl Write for Out {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self {
            Out::Plain(w) => w.write(b),
            Out::Gz(w) => w.write(b),
            Out::Bz(w) => w.write(b),
            Out::Xz(w) => w.write(b),
            Out::Zst(w) => w.write(b),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Out::Plain(w) => w.flush(),
            Out::Gz(w) => w.flush(),
            Out::Bz(w) => w.flush(),
            Out::Xz(w) => w.flush(),
            Out::Zst(w) => w.flush(),
        }
    }
}

impl Out {
    fn new(f: File, codec: Codec) -> io::Result<Out> {
        let w = io::BufWriter::with_capacity(1 << 16, f);
        Ok(match codec {
            Codec::None => Out::Plain(w),
            Codec::Gzip => Out::Gz(flate2::write::GzEncoder::new(w, flate2::Compression::default())),
            Codec::Bzip2 => Out::Bz(bzip2::write::BzEncoder::new(w, bzip2::Compression::default())),
            Codec::Xz => Out::Xz(liblzma::write::XzEncoder::new(w, 6)),
            Codec::Zstd => Out::Zst(zstd::stream::write::Encoder::new(w, 3)?),
        })
    }

    fn finish(self) -> io::Result<()> {
        let mut w = match self {
            Out::Plain(w) => w,
            Out::Gz(w) => w.finish()?,
            Out::Bz(w) => w.finish()?,
            Out::Xz(w) => w.finish()?,
            Out::Zst(w) => w.finish()?,
        };
        w.flush()?;
        w.get_ref().sync_all()
    }
}

pub fn compress(walked: &[Walked], partial: &Path, codec: Codec, ctl: &Control) -> Result<()> {
    let mut b = Builder::new(Out::new(File::create_new(partial)?, codec)?);
    b.follow_symlinks(false);
    b.mode(HeaderMode::Complete);
    for w in walked {
        ctl.item(&w.name)?;
        let meta = w.path.symlink_metadata()?;
        let mut h = Header::new_gnu();
        h.set_metadata_in_mode(&meta, HeaderMode::Complete);
        match &w.kind {
            WalkKind::Dir => {
                h.set_size(0);
                b.append_data(&mut h, format!("{}/", w.name), io::empty()).map_err(|e| stopped_or(e, ctl))?;
            }
            WalkKind::Symlink(target) => {
                h.set_size(0);
                b.append_link(&mut h, &w.name, target).map_err(|e| stopped_or(e, ctl))?;
            }
            WalkKind::File => {
                let r = ctl.counted(crate::sink::open(&w.path)?);
                b.append_data(&mut h, &w.name, r).map_err(|e| stopped_or(e, ctl))?;
            }
        }
    }
    b.into_inner().map_err(|e| stopped_or(e, ctl))?.finish()?;
    Ok(())
}
