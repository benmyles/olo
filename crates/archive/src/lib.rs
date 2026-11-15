//! Archives, read and written in Rust, in process (Ben, 2026-11-14:
//! "olo's tracker should have native support for compression/archives …
//! support all the common formats including zip, tar, gz, tar.gz, bzip,
//! 7z, rar, iso and so on"): zip (and its AES), tar plain or with gzip,
//! bzip2, xz or zstd, 7-Zip (and its AES), RAR (read), disc images (read:
//! ISO 9660 with Joliet and Rock Ridge names, and UDF), cpio (read), and
//! a single file compressed with gzip, bzip2, xz or zstd.
//!
//! - [`kind`]: what a file is, by its first bytes (then its name).
//! - [`list`]: what's in one.
//! - [`extract`]: everything out of one, as Finder's Archive Utility
//!   does it: one thing at its top comes out beside the archive, several
//!   come out in a folder named for it; nothing is ever written outside
//!   (`..` and absolute paths refused, every file made new, links made
//!   last and only when what they point to stays inside); permissions
//!   kept (but setuid and setgid), times kept.
//! - [`compress`]: files and folders into a new archive, symbolic links
//!   kept as links where the format can.
//!
//! Each takes a [`Control`]: how far it has got (bytes of how many, and
//! the item), and a flag that stops it (what was half made is removed).
//! A password-protected archive says so ([`Error::Password`]) and is
//! read again with one.

mod cpio;
mod iso;
mod rar;
mod sevenz;
mod sink;
mod tarfmt;
mod udf;
mod zipfmt;

use std::{
    fs::File,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub use sink::Sink;

/// A compression around a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    None,
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

impl Codec {
    /// The stream's name's ending (".gz").
    pub fn suffix(self) -> &'static str {
        match self {
            Codec::None => "",
            Codec::Gzip => ".gz",
            Codec::Bzip2 => ".bz2",
            Codec::Xz => ".xz",
            Codec::Zstd => ".zst",
        }
    }

    /// `r` read through this codec.
    pub fn reader<'a, R: Read + 'a>(self, r: R) -> io::Result<Box<dyn Read + 'a>> {
        let r = io::BufReader::with_capacity(1 << 16, r);
        Ok(match self {
            Codec::None => Box::new(r),
            Codec::Gzip => Box::new(flate2::read::MultiGzDecoder::new(r)),
            Codec::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(r)),
            Codec::Xz => Box::new(liblzma::read::XzDecoder::new_multi_decoder(r)),
            Codec::Zstd => Box::new(zstd::stream::read::Decoder::new(r)?),
        })
    }

    fn of_magic(m: &[u8]) -> Option<Codec> {
        if m.starts_with(&[0x1f, 0x8b]) {
            Some(Codec::Gzip)
        } else if m.starts_with(b"BZh") {
            Some(Codec::Bzip2)
        } else if m.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
            Some(Codec::Xz)
        } else if m.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Some(Codec::Zstd)
        } else {
            None
        }
    }
}

/// What kind of archive a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZip,
    Rar,
    Iso,
    Tar(Codec),
    Cpio(Codec),
    /// One file, compressed (foo.txt.gz).
    Single(Codec),
}

impl Kind {
    /// In words: "Zip archive", "Tar archive (xz)".
    pub fn name(self) -> String {
        let codec = |c: Codec| match c {
            Codec::None => String::new(),
            Codec::Gzip => " (gzip)".into(),
            Codec::Bzip2 => " (bzip2)".into(),
            Codec::Xz => " (xz)".into(),
            Codec::Zstd => " (zstd)".into(),
        };
        match self {
            Kind::Zip => "Zip archive".into(),
            Kind::SevenZip => "7-Zip archive".into(),
            Kind::Rar => "RAR archive".into(),
            Kind::Iso => "Disc image".into(),
            Kind::Tar(c) => format!("Tar archive{}", codec(c)),
            Kind::Cpio(c) => format!("cpio archive{}", codec(c)),
            Kind::Single(c) => format!("Compressed file{}", codec(c)),
        }
    }
}

/// What a file is, by its first bytes (and inside a compression, the
/// first bytes of what it holds), then its name; None for anything else.
pub fn kind(path: &Path) -> Option<Kind> {
    let mut f = File::open(path).ok()?;
    let mut m = [0u8; 8];
    let n = read_up_to(&mut f, &mut m).ok()?;
    let m = &m[..n];
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if m.starts_with(b"PK\x03\x04") || m.starts_with(b"PK\x05\x06") {
        return Some(Kind::Zip);
    }
    if m.starts_with(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) {
        return Some(Kind::SevenZip);
    }
    if m.starts_with(b"Rar!\x1a\x07") {
        return Some(Kind::Rar);
    }
    if let Some(c) = Codec::of_magic(m) {
        // What's inside the compression: a tar, a cpio, or one file.
        let mut head = [0u8; 512];
        let inner = File::open(path).ok().and_then(|f| c.reader(f).ok()).and_then(|mut r| read_up_to(&mut r, &mut head).ok()).unwrap_or(0);
        return Some(inner_kind(&head[..inner], c, &name));
    }
    let mut head = [0u8; 512];
    let n = File::open(path).ok().and_then(|mut f| read_up_to(&mut f, &mut head).ok()).unwrap_or(0);
    if is_tar(&head[..n]) {
        return Some(Kind::Tar(Codec::None));
    }
    if cpio::is_cpio(&head[..n]) {
        return Some(Kind::Cpio(Codec::None));
    }
    if iso::is_iso(path) {
        return Some(Kind::Iso);
    }
    None
}

fn inner_kind(head: &[u8], c: Codec, name: &str) -> Kind {
    if is_tar(head) || name.ends_with(".tgz") || name.ends_with(".tbz2") || name.ends_with(".txz") || name.contains(".tar.") {
        Kind::Tar(c)
    } else if cpio::is_cpio(head) {
        Kind::Cpio(c)
    } else {
        Kind::Single(c)
    }
}

/// Whether `h` starts a tar: "ustar" where it should be, or a header
/// whose checksum adds up.
fn is_tar(h: &[u8]) -> bool {
    if h.len() < 512 {
        return false;
    }
    if &h[257..262] == b"ustar" {
        return true;
    }
    let stored = std::str::from_utf8(&h[148..156]).ok().map(|s| s.trim_matches(|c: char| c == '\0' || c == ' ')).and_then(|s| u32::from_str_radix(s, 8).ok());
    let sum: u32 = h[..512].iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u32 }).sum();
    stored == Some(sum) && h[0] != 0
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Whether a file is an archive this crate reads.
pub fn is_archive(path: &Path) -> bool {
    kind(path).is_some()
}

/// What an entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// A symbolic link to this target.
    Symlink(String),
    /// A hard link to this other entry.
    Hardlink(String),
}

/// One thing in an archive.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its name as the archive has it.
    pub name: String,
    /// Where it would go, relative: None when it would be outside (`..`,
    /// absolute), which is never extracted.
    pub path: Option<PathBuf>,
    pub kind: EntryKind,
    pub size: u64,
    pub mode: Option<u32>,
    /// When it was last changed (Unix time).
    pub mtime: Option<i64>,
    pub encrypted: bool,
}

/// What went wrong.
#[derive(Debug)]
pub enum Error {
    /// It needs a password: none was given (false) or it was wrong (true).
    Password(bool),
    /// Stopped by the person.
    Cancelled,
    /// Not something this reads or writes.
    Unsupported(String),
    /// Broken, or not what it says it is.
    Bad(String),
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Password(false) => write!(f, "It's protected by a password."),
            Error::Password(true) => write!(f, "The password isn't right."),
            Error::Cancelled => write!(f, "Stopped."),
            Error::Unsupported(s) => write!(f, "{s}"),
            Error::Bad(s) => write!(f, "It's damaged or isn't what it says it is: {s}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// How far a job has got.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    /// The item being read or written.
    pub item: String,
}

/// What a job reports to, and what stops it (used through `&self`, so
/// a reader can count while the job names items).
pub struct Control<'a> {
    pub cancel: &'a AtomicBool,
    inner: std::cell::RefCell<Inner<'a>>,
}

struct Inner<'a> {
    progress: &'a mut dyn FnMut(&Progress),
    at: Progress,
    last: std::time::Instant,
}

impl<'a> Control<'a> {
    pub fn new(cancel: &'a AtomicBool, progress: &'a mut dyn FnMut(&Progress)) -> Control<'a> {
        Control { cancel, inner: std::cell::RefCell::new(Inner { progress, at: Progress::default(), last: std::time::Instant::now() - std::time::Duration::from_secs(1) }) }
    }

    fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
    }

    fn total(&self, total: u64) {
        self.inner.borrow_mut().at.total = total;
    }

    fn item(&self, item: &str) -> Result<()> {
        self.inner.borrow_mut().at.item = item.to_owned();
        self.report(true);
        self.check()
    }

    fn advance(&self, bytes: u64) -> Result<()> {
        self.inner.borrow_mut().at.done += bytes;
        self.report(false);
        self.check()
    }

    /// Said at most twenty times a second (always when `now`).
    fn report(&self, now: bool) {
        let mut i = self.inner.borrow_mut();
        if now || i.last.elapsed() >= std::time::Duration::from_millis(50) {
            i.last = std::time::Instant::now();
            let at = i.at.clone();
            (i.progress)(&at);
        }
    }

    /// Said once at the end, whatever the time.
    fn done(&self) {
        self.report(true);
    }

    /// `r`, counted as it's read, stopping when told.
    fn counted<R: Read>(&self, r: R) -> Counted<'_, 'a, R> {
        Counted { r, ctl: self }
    }
}

/// A reader that tells its `Control` how far it has got.
pub(crate) struct Counted<'b, 'a, R> {
    r: R,
    ctl: &'b Control<'a>,
}

impl<R: Read> Read for Counted<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.r.read(buf)?;
        // (Not Interrupted, which io::copy would try again.)
        self.ctl.advance(n as u64).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(n)
    }
}

/// An error a counted read ended with, as the job's: stopped, or not.
pub(crate) fn stopped_or(e: io::Error, ctl: &Control) -> Error {
    if ctl.cancel.load(Ordering::Relaxed) { Error::Cancelled } else { Error::Io(e) }
}

/// An entry's name as a path inside the folder it's extracted into, or
/// None when it would be outside it (absolute, `..`, a drive letter) or
/// is nothing (the archive's own root).
pub fn safe_path(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in Path::new(&name).components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// What's in the archive at `path`.
pub fn list(path: &Path, password: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Entry>> {
    let kind = kind(path).ok_or_else(|| Error::Unsupported("This isn't an archive Olo can read.".into()))?;
    match kind {
        Kind::Zip => zipfmt::list(path),
        Kind::SevenZip => sevenz::list(path, password),
        Kind::Rar => rar::list(path, password),
        Kind::Iso => iso::list(path),
        Kind::Tar(c) => tarfmt::list(path, c, cancel),
        Kind::Cpio(c) => cpio::list(path, c, cancel),
        Kind::Single(c) => Ok(vec![single_entry(path, c)]),
    }
}

fn single_entry(path: &Path, c: Codec) -> Entry {
    let name = base_name(path, Kind::Single(c));
    Entry { path: safe_path(&name), name, kind: EntryKind::File, size: 0, mode: None, mtime: None, encrypted: false }
}

/// The archive's name without its endings: "photos" for photos.tar.gz.
pub fn base_name(path: &Path, kind: Kind) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    let cut = |ends: &[&str]| ends.iter().find(|e| lower.ends_with(*e) && lower.len() > e.len()).map(|e| name[..name.len() - e.len()].to_owned());
    let base = match kind {
        Kind::Zip => cut(&[".zip", ".jar", ".apk", ".epub", ".xpi", ".whl", ".cbz"]),
        Kind::SevenZip => cut(&[".7z", ".cb7"]),
        Kind::Rar => cut(&[".rar", ".cbr"]),
        Kind::Iso => cut(&[".iso", ".img"]),
        Kind::Tar(_) => cut(&[".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.zstd", ".tgz", ".tbz2", ".tbz", ".txz", ".tzst", ".tar"]),
        Kind::Cpio(_) => cut(&[".cpio.gz", ".cpio.xz", ".cpio.zst", ".cpio.bz2", ".cpio"]),
        Kind::Single(_) => cut(&[".gz", ".bz2", ".xz", ".zst", ".zstd", ".z"]),
    };
    base.unwrap_or_else(|| format!("{name} contents"))
}

/// `path`, or "name 2.ext", "name 3.ext", … if it's taken.
pub fn unique(path: &Path) -> PathBuf {
    if path.symlink_metadata().is_err() {
        return path.to_owned();
    }
    let parent = path.parent().unwrap_or(Path::new(""));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // (The endings stay: "photos 2.tar.gz".)
    let lower = name.to_lowercase();
    let ext_at = [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"].iter().find(|e| lower.ends_with(*e)).map(|e| name.len() - e.len()).or_else(|| name.rfind('.').filter(|&i| i > 0));
    let (stem, ext) = match ext_at {
        Some(i) => (&name[..i], &name[i..]),
        None => (name.as_str(), ""),
    };
    (2..).map(|n| parent.join(format!("{stem} {n}{ext}"))).find(|p| p.symlink_metadata().is_err()).expect("a free name")
}

/// Extract everything in the archive at `path` into the folder `into`
/// (Finder's way: one thing at its top comes out as it is, several come
/// out in a folder named for the archive; a name taken gets a number).
/// What it made at `into`'s top, for undo.
pub fn extract(path: &Path, into: &Path, password: Option<&str>, ctl: &Control) -> Result<Vec<PathBuf>> {
    extract_with(path, into, password, ctl, None)
}

/// Extract only `names` (entries' paths as `list` gives them; a folder
/// with what's in it) into `into`, each without `strip` at the front of
/// its path (the folder being looked at in the archive): each thing at
/// the top as it is, a taken name numbered. What it made.
pub fn extract_some(path: &Path, names: &[String], strip: &str, into: &Path, password: Option<&str>, ctl: &Control) -> Result<Vec<PathBuf>> {
    let names: Vec<PathBuf> = names.iter().filter_map(|n| safe_path(n)).collect();
    let strip = safe_path(strip).unwrap_or_default();
    extract_with(path, into, password, ctl, Some((names, strip)))
}

fn extract_with(path: &Path, into: &Path, password: Option<&str>, ctl: &Control, only: Option<(Vec<PathBuf>, PathBuf)>) -> Result<Vec<PathBuf>> {
    let kind = kind(path).ok_or_else(|| Error::Unsupported("This isn't an archive Olo can read.".into()))?;
    // Into a hidden folder of its own beside where it goes, so nothing
    // half made is ever seen, and nothing there is overwritten.
    let stage = sink::stage_dir(into)?;
    let some = only.is_some();
    let made = (|| {
        let mut sink = Sink::new(&stage);
        if let Some((names, strip)) = only {
            sink = sink.only(names, strip);
        }
        match kind {
            Kind::Zip => zipfmt::extract(path, &mut sink, password, ctl)?,
            Kind::SevenZip => sevenz::extract(path, &mut sink, password, ctl)?,
            Kind::Rar => rar::extract(path, &mut sink, password, ctl)?,
            Kind::Iso => iso::extract(path, &mut sink, ctl)?,
            Kind::Tar(c) => tarfmt::extract(path, c, &mut sink, ctl)?,
            Kind::Cpio(c) => cpio::extract(path, c, &mut sink, ctl)?,
            Kind::Single(c) => {
                let name = base_name(path, kind);
                // (Counted in what's read of the compressed file: its size is known.)
                ctl.total(std::fs::metadata(path)?.len());
                ctl.item(&name)?;
                let rel = safe_path(&name).ok_or_else(|| Error::Bad("it has no name".into()))?;
                let mut src = c.reader(ctl.counted(File::open(path)?))?;
                sink.file(&rel, &mut src, None, None).map_err(|e| stopped_or(e, ctl))?;
            }
        }
        sink.finish()?;
        ctl.done();
        if some { sink::settle_each(&stage, into) } else { sink::settle(&stage, into, &base_name(path, kind)) }
    })();
    let _ = std::fs::remove_dir_all(&stage);
    made
}

/// What an archive is made as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    SevenZip,
}

impl Format {
    pub const ALL: [Format; 7] = [Format::Zip, Format::TarGz, Format::TarXz, Format::TarZst, Format::TarBz2, Format::Tar, Format::SevenZip];

    /// The ending its file gets.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Zip => ".zip",
            Format::Tar => ".tar",
            Format::TarGz => ".tar.gz",
            Format::TarBz2 => ".tar.bz2",
            Format::TarXz => ".tar.xz",
            Format::TarZst => ".tar.zst",
            Format::SevenZip => ".7z",
        }
    }

    /// As a menu says it.
    pub fn label(self) -> &'static str {
        match self {
            Format::Zip => "Zip",
            Format::Tar => "Tar",
            Format::TarGz => "Tar, gzip (.tar.gz)",
            Format::TarBz2 => "Tar, bzip2 (.tar.bz2)",
            Format::TarXz => "Tar, xz (.tar.xz)",
            Format::TarZst => "Tar, zstd (.tar.zst)",
            Format::SevenZip => "7-Zip",
        }
    }

    /// Whether it can be protected by a password.
    pub fn takes_password(self) -> bool {
        matches!(self, Format::Zip | Format::SevenZip)
    }

    /// "zip", "tar.gz": the format by its ending, for commands.
    pub fn of_word(w: &str) -> Option<Format> {
        let w = w.trim_start_matches('.');
        Format::ALL.into_iter().find(|f| f.extension().trim_start_matches('.') == w)
    }
}

/// The name a new archive of `sources` gets: "name.zip" for one thing,
/// "Archive.zip" for several, as Finder's.
pub fn archive_name(sources: &[PathBuf], format: Format) -> String {
    match sources {
        [one] => format!("{}{}", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Archive".into()), format.extension()),
        _ => format!("Archive{}", format.extension()),
    }
}

/// Make the archive `dest` (a new file: written beside it under a hidden
/// name, then put in place) of `sources`, each under its own name with
/// what's in it.
pub fn compress(sources: &[PathBuf], format: Format, dest: &Path, password: Option<&str>, ctl: &Control) -> Result<()> {
    let walked = sink::walk(sources, ctl.cancel)?;
    ctl.total(walked.iter().map(|w| w.size).sum());
    let parent = dest.parent().unwrap_or(Path::new("."));
    let partial = parent.join(format!(".{}.olo-partial", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let made = match format {
        Format::Zip => zipfmt::compress(&walked, &partial, password, ctl),
        Format::SevenZip => sevenz::compress(&walked, &partial, password, ctl),
        Format::Tar => tarfmt::compress(&walked, &partial, Codec::None, ctl),
        Format::TarGz => tarfmt::compress(&walked, &partial, Codec::Gzip, ctl),
        Format::TarBz2 => tarfmt::compress(&walked, &partial, Codec::Bzip2, ctl),
        Format::TarXz => tarfmt::compress(&walked, &partial, Codec::Xz, ctl),
        Format::TarZst => tarfmt::compress(&walked, &partial, Codec::Zstd, ctl),
    };
    ctl.done();
    match made {
        Ok(()) => {
            if dest.symlink_metadata().is_ok() {
                let _ = std::fs::remove_file(&partial);
                return Err(Error::Io(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is there already", dest.display()))));
            }
            std::fs::rename(&partial, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests;
