//! Installing Google Chrome for this user, natively: Google's rpm
//! repository is read for the newest `google-chrome-stable` for this CPU,
//! the package is downloaded (its sha256 checked against the repository's)
//! and unpacked here in Rust (rpm headers, then the compressed cpio
//! payload). Chrome goes into `~/.local/share/olo/chrome/<version>`, with
//! `current` pointing at it, its icons into the hicolor theme and a desktop
//! entry that starts it through `olo-chrome`. Nothing needs root, and
//! nothing outside the user's home is touched.

use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

/// Google's repository (the one its own Fedora package adds).
const REPO: &str = "https://dl.google.com/linux/chrome/rpm/stable";
const PACKAGE: &str = "google-chrome-stable";
/// Where in the package Chrome lives.
const PREFIX: &str = "./opt/google/chrome/";

/// How far an install has got.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Looking,
    Downloading { version: String, got: u64, total: u64 },
    Unpacking { version: String },
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

/// The newest package in the repository.
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub version: String,
    pub href: String,
    pub sha256: String,
    pub size: u64,
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
}

/// Where Chrome is kept: a folder per version and `current`.
pub fn root() -> PathBuf {
    data_home().join("olo/chrome")
}

/// Chrome's own launcher script, if Chrome is installed.
pub fn installed() -> Option<PathBuf> {
    let p = root().join("current/google-chrome");
    fs::metadata(&p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).then_some(p)
}

/// The repository's name for this computer's CPU.
fn arch() -> Result<&'static str, String> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("aarch64"),
        "x86_64" => Ok("x86_64"),
        other => Err(format!("Google doesn't make Chrome for Linux on {other}.")),
    }
}

fn agent() -> ureq::Agent {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder().tls_config(tls).max_redirects(10).user_agent("Olo").timeout_connect(Some(Duration::from_secs(30))).timeout_per_read(Some(Duration::from_secs(60))).build().into()
}

/// `url`'s answer (asked again across a network change: `olo_net`).
fn get(agent: &ureq::Agent, url: &str) -> Result<ureq::Body, String> {
    olo_net::across_changes(Duration::from_secs(30), || agent.get(url).call()).map(|r| r.into_body()).map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("Google's server answered {code} for {url}."),
        e => format!("Couldn't reach Google's server: {e}."),
    })
}

fn read_all(body: ureq::Body, limit: u64) -> Result<Vec<u8>, String> {
    body.into_with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())
}

/// `data` unpacked by its file name's extension.
fn decompress(name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let r = if name.ends_with(".gz") {
        flate2::read::GzDecoder::new(data).read_to_end(&mut out)
    } else if name.ends_with(".zst") {
        zstd::stream::read::Decoder::new(data).and_then(|mut d| d.read_to_end(&mut out))
    } else if name.ends_with(".xz") {
        liblzma::read::XzDecoder::new(data).read_to_end(&mut out)
    } else {
        out.extend_from_slice(data);
        Ok(0)
    };
    r.map(|_| out).map_err(|e| format!("The repository's index is damaged: {e}."))
}

/// Elements and text, in order (what reading a repository's XML needs).
enum X {
    Start(String, Vec<(String, String)>),
    End(String),
    Text(String),
}

fn xml(text: &str) -> Vec<X> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(text);
    let attrs = |e: &quick_xml::events::BytesStart| {
        e.attributes()
            .flatten()
            .map(|a| {
                let k = a.key.local_name().as_ref().to_owned();
                let v = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default();
                (k, v)
            })
            .collect::<Vec<_>>()
    };
    let name = |n: quick_xml::name::LocalName| n.as_ref().to_owned();
    let mut out = Vec::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => out.push(X::Start(name(e.local_name()), attrs(&e))),
            Ok(Event::Empty(e)) => {
                out.push(X::Start(name(e.local_name()), attrs(&e)));
                out.push(X::End(name(e.local_name())));
            }
            Ok(Event::End(e)) => out.push(X::End(name(e.local_name()))),
            Ok(Event::Text(t)) => out.push(X::Text(t.xml10_content().into_owned())),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Where repomd.xml says the primary index is.
fn primary_href(repomd: &str) -> Option<String> {
    let mut in_primary = false;
    for x in xml(repomd) {
        match x {
            X::Start(n, a) if n == "data" => in_primary = attr(&a, "type") == Some("primary"),
            X::Start(n, a) if n == "location" && in_primary => return attr(&a, "href").map(str::to_owned),
            _ => {}
        }
    }
    None
}

/// The newest `google-chrome-stable` in a primary index.
fn newest(primary: &str) -> Option<Package> {
    let mut best: Option<(Vec<u64>, Package)> = None;
    let (mut name, mut version, mut href, mut sha256, mut size) = (String::new(), String::new(), String::new(), String::new(), 0u64);
    let mut field = String::new();
    let mut sha = false;
    for x in xml(primary) {
        match x {
            X::Start(n, _) if n == "package" => {
                (name, version, href, sha256, size) = Default::default();
            }
            X::Start(n, a) => {
                match n.as_str() {
                    "version" => version = attr(&a, "ver").unwrap_or_default().to_owned(),
                    "location" => href = attr(&a, "href").unwrap_or_default().to_owned(),
                    "size" => size = attr(&a, "package").and_then(|s| s.parse().ok()).unwrap_or(0),
                    "checksum" => sha = attr(&a, "type") == Some("sha256"),
                    _ => {}
                }
                field = n;
            }
            X::Text(t) => match field.as_str() {
                "name" => name.push_str(t.trim()),
                "checksum" if sha => sha256.push_str(t.trim()),
                _ => {}
            },
            X::End(n) if n == "package" => {
                if name == PACKAGE && !href.is_empty() && sha256.len() == 64 {
                    let key: Vec<u64> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
                    if best.as_ref().is_none_or(|(k, _)| key > *k) {
                        best = Some((key, Package { version: version.clone(), href: href.clone(), sha256: sha256.clone(), size }));
                    }
                }
            }
            X::End(_) => field.clear(),
        }
    }
    best.map(|(_, p)| p)
}

/// Ask the repository for the newest Chrome.
pub fn latest() -> Result<Package, String> {
    let agent = agent();
    let base = format!("{REPO}/{}", arch()?);
    let repomd = read_all(get(&agent, &format!("{base}/repodata/repomd.xml"))?, 1 << 20)?;
    let href = primary_href(&String::from_utf8_lossy(&repomd)).ok_or("The repository has no package index.")?;
    let packed = read_all(get(&agent, &format!("{base}/{href}"))?, 64 << 20)?;
    let primary = decompress(&href, &packed)?;
    newest(&String::from_utf8_lossy(&primary)).ok_or_else(|| format!("The repository has no {PACKAGE}."))
}

/// Download, check, unpack and set up Chrome; `tell` hears how it goes
/// (from this thread) and `cancel` stops it between reads.
pub fn install(cancel: &AtomicBool, tell: &dyn Fn(Progress)) {
    tell(match run(cancel, tell) {
        Ok(Some(path)) => Progress::Done(path),
        Ok(None) => Progress::Cancelled,
        Err(e) => Progress::Failed(e),
    });
}

fn run(cancel: &AtomicBool, tell: &dyn Fn(Progress)) -> Result<Option<PathBuf>, String> {
    tell(Progress::Looking);
    let package = latest()?;
    put_in(package, cancel, tell)
}

/// The version `current` points at.
pub fn current_version() -> Option<String> {
    fs::read_link(root().join("current")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// How often to look for a newer Chrome.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether it's time to look for a newer Chrome (not looked for a day).
pub fn update_due() -> bool {
    let stamp = root().join(".checked");
    fs::metadata(&stamp).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_none_or(|age| age >= CHECK_EVERY)
}

/// Look for a newer Chrome and put it in beside the one in use (which
/// keeps running; the next start is the new one). Quiet: what happened
/// goes to the log. One at a time.
pub fn update_quietly() {
    let root = root();
    let _ = fs::write(root.join(".checked"), b"");
    let lock = root.join(".updating");
    // A lock left by an update that died an hour ago doesn't count.
    if fs::metadata(&lock).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(3600)) {
        let _ = fs::remove_file(&lock);
    }
    if fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        return;
    }
    let outcome = latest().and_then(|package| {
        if current_version().as_deref() == Some(package.version.as_str()) {
            return Ok(None);
        }
        eprintln!("olo-chrome: updating Chrome to {}", package.version);
        put_in(package, &AtomicBool::new(false), &|_| {})
    });
    match outcome {
        Ok(Some(p)) => eprintln!("olo-chrome: Chrome updated: {}", p.display()),
        Ok(None) => {}
        Err(e) => eprintln!("olo-chrome: couldn't update Chrome: {e}"),
    }
    let _ = fs::remove_file(&lock);
}

/// Download, check and unpack `package`, and make it the current one.
fn put_in(package: Package, cancel: &AtomicBool, tell: &dyn Fn(Progress)) -> Result<Option<PathBuf>, String> {
    let root = root();
    fs::create_dir_all(&root).map_err(|e| format!("Couldn't make {}: {e}.", root.display()))?;
    let rpm = root.join(format!(".{PACKAGE}-{}.rpm.part", package.version));
    let Some(()) = download(&package, &rpm, cancel, tell)? else {
        let _ = fs::remove_file(&rpm);
        return Ok(None);
    };
    tell(Progress::Unpacking { version: package.version.clone() });
    let dest = root.join(&package.version);
    let staging = root.join(format!(".{}.new", package.version));
    let _ = fs::remove_dir_all(&staging);
    let unpacked = fs::File::open(&rpm).map_err(|e| e.to_string()).and_then(|f| unpack_rpm(io::BufReader::new(f), &staging, cancel));
    let _ = fs::remove_file(&rpm);
    match unpacked {
        Ok(true) => {}
        Ok(false) => {
            let _ = fs::remove_dir_all(&staging);
            return Ok(None);
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    if !staging.join("google-chrome").is_file() {
        let _ = fs::remove_dir_all(&staging);
        return Err("The package didn't hold Chrome.".into());
    }
    let before = current_version();
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&staging, &dest).map_err(|e| e.to_string())?;
    // `current` switches over in one step.
    let link = root.join(".current.new");
    let _ = fs::remove_file(&link);
    symlink(&package.version, &link).and_then(|_| fs::rename(&link, root.join("current"))).map_err(|e| e.to_string())?;
    // Older versions go, but not the one just replaced: it may still be
    // running, and Chrome reads its files as it goes.
    for e in fs::read_dir(&root).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let keep = n == package.version || n == "current" || n.starts_with('.') || before.as_deref() == Some(n.as_str());
        if !keep && e.path().is_dir() {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    set_up(&dest)?;
    Ok(Some(dest.join("google-chrome")))
}

/// Into `to`, checking the sha256; None if cancelled. Across a network
/// change (a cable pulled, Wi-Fi left) it goes on where it broke off, on
/// the new one (`olo_net::http::Download`; Cancel ends a wait for one).
fn download(package: &Package, to: &Path, cancel: &AtomicBool, tell: &dyn Fn(Progress)) -> Result<Option<()>, String> {
    let agent = agent();
    let url = format!("{REPO}/{}/{}", arch()?, package.href);
    let mut body = olo_net::http::Download::start(&agent, &url, None, Some(cancel)).map_err(|e| format!("Couldn't download Chrome: {e}."))?;
    let total = body.total().unwrap_or(package.size);
    let mut file = fs::File::create(to).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut got, mut told) = (0u64, Instant::now() - Duration::from_secs(1));
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let n = body.read(&mut buf).map_err(|e| format!("The download stopped: {e}."))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        if told.elapsed() >= Duration::from_millis(100) {
            tell(Progress::Downloading { version: package.version.clone(), got, total });
            told = Instant::now();
        }
    }
    tell(Progress::Downloading { version: package.version.clone(), got, total });
    let sum: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !sum.eq_ignore_ascii_case(&package.sha256) {
        return Err("The download doesn't match the repository's checksum, so it wasn't installed.".into());
    }
    Ok(Some(()))
}

/// An rpm header: its strings by tag (the payload's compressor), and
/// the rest as it is, for the files' arrays ([`Header::files`]).
#[derive(Default)]
struct Header {
    strings: HashMap<u32, String>,
    /// Each entry: its tag, type, offset in `data` and count.
    index: Vec<(u32, u32, usize, usize)>,
    data: Vec<u8>,
}

/// A file of the package as its header says it: what the payload's
/// newer form leaves out of the archive (rpm 6's "stripped" cpio, whose
/// entries are only a number: the file's place in these).
#[derive(Debug, Clone, PartialEq)]
struct PackageFile {
    /// As an archive names it: `./opt/google/chrome/chrome`.
    name: String,
    mode: u32,
    size: u64,
    inode: u64,
    /// How many names the file has (hard links: its data comes with the
    /// last of them).
    links: u64,
}

impl Header {
    fn get(&self, tag: u32) -> Option<(u32, usize, usize)> {
        self.index.iter().find(|e| e.0 == tag).map(|e| (e.1, e.2, e.3))
    }

    /// An array of numbers (16, 32 or 64 bits each, big-endian).
    fn numbers(&self, tag: u32) -> Vec<u64> {
        let Some((kind, offset, count)) = self.get(tag) else { return Vec::new() };
        let width = match kind {
            3 => 2,
            4 => 4,
            5 => 8,
            _ => return Vec::new(),
        };
        (0..count).map_while(|i| self.data.get(offset + i * width..offset + (i + 1) * width)).map(|b| b.iter().fold(0u64, |n, byte| n << 8 | u64::from(*byte))).collect()
    }

    /// An array of strings (each ended by a zero).
    fn texts(&self, tag: u32) -> Vec<String> {
        let Some((kind, mut offset, count)) = self.get(tag) else { return Vec::new() };
        if !matches!(kind, 6 | 8 | 9) {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count.min(100_000));
        for _ in 0..count {
            let Some(rest) = self.data.get(offset..) else { break };
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            out.push(String::from_utf8_lossy(&rest[..end]).into_owned());
            offset += end + 1;
        }
        out
    }

    /// The package's files, in the header's order: names from the
    /// folders' and the files' own (DIRNAMES, DIRINDEXES, BASENAMES),
    /// sizes (LONGFILESIZES, else FILESIZES), modes, and inodes (and
    /// link counts, where given) for hard links.
    fn files(&self) -> Vec<PackageFile> {
        const FILESIZES: u32 = 1028;
        const FILEMODES: u32 = 1030;
        const FILEINODES: u32 = 1096;
        const DIRINDEXES: u32 = 1116;
        const BASENAMES: u32 = 1117;
        const DIRNAMES: u32 = 1118;
        const LONGFILESIZES: u32 = 5008;
        const FILENLINKS: u32 = 5045;
        let (bases, dirs, at) = (self.texts(BASENAMES), self.texts(DIRNAMES), self.numbers(DIRINDEXES));
        let sizes = Some(self.numbers(LONGFILESIZES)).filter(|s| !s.is_empty()).unwrap_or_else(|| self.numbers(FILESIZES));
        let (modes, inodes, links) = (self.numbers(FILEMODES), self.numbers(FILEINODES), self.numbers(FILENLINKS));
        // (A package that doesn't say how many names a file has: as many
        // files as have its inode.)
        let sharing = |i: usize| match (links.get(i), inodes.get(i)) {
            (Some(n), _) => *n,
            (None, Some(inode)) if modes.get(i).is_some_and(|m| m & 0o170000 == 0o100000) => inodes.iter().zip(&modes).filter(|(other, m)| *other == inode && *m & 0o170000 == 0o100000).count() as u64,
            _ => 1,
        };
        bases
            .iter()
            .enumerate()
            .map(|(i, base)| PackageFile {
                name: format!(".{}{base}", at.get(i).and_then(|d| dirs.get(*d as usize)).map_or("/", String::as_str)),
                mode: modes.get(i).copied().unwrap_or(0) as u32,
                size: sizes.get(i).copied().unwrap_or(0),
                inode: inodes.get(i).copied().unwrap_or(0),
                links: sharing(i),
            })
            .collect()
    }
}

fn read_header(r: &mut impl Read, pad: bool) -> Result<Header, String> {
    let mut head = [0u8; 16];
    r.read_exact(&mut head).map_err(|_| "The package is cut short.")?;
    if head[..3] != [0x8e, 0xad, 0xe8] {
        return Err("This isn't an rpm package.".into());
    }
    let count = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize;
    let size = u32::from_be_bytes(head[12..16].try_into().unwrap()) as usize;
    if count > 100_000 || size > 64 << 20 {
        return Err("The package's header is damaged.".into());
    }
    let mut index = vec![0u8; count * 16];
    let mut data = vec![0u8; size];
    r.read_exact(&mut index).and_then(|_| r.read_exact(&mut data)).map_err(|_| "The package is cut short.")?;
    if pad && !size.is_multiple_of(8) {
        let mut skip = vec![0u8; 8 - size % 8];
        r.read_exact(&mut skip).map_err(|_| "The package is cut short.")?;
    }
    let mut out = Header::default();
    for e in index.chunks(16) {
        let word = |i: usize| u32::from_be_bytes(e[i..i + 4].try_into().unwrap());
        let (tag, kind, offset, count) = (word(0), word(4), word(8) as usize, word(12) as usize);
        out.index.push((tag, kind, offset, count));
        if kind == 6 && offset < data.len() {
            let end = data[offset..].iter().position(|&b| b == 0).map_or(data.len(), |p| offset + p);
            out.strings.insert(tag, String::from_utf8_lossy(&data[offset..end]).into_owned());
        }
    }
    out.data = data;
    Ok(out)
}

const PAYLOAD_COMPRESSOR: u32 = 1125;

/// Unpack Chrome's files from an rpm into `dest`; false if cancelled.
fn unpack_rpm(mut r: impl Read, dest: &Path, cancel: &AtomicBool) -> Result<bool, String> {
    let mut lead = [0u8; 96];
    r.read_exact(&mut lead).map_err(|_| "The package is cut short.")?;
    if lead[..4] != [0xed, 0xab, 0xee, 0xdb] {
        return Err("This isn't an rpm package.".into());
    }
    read_header(&mut r, true)?; // the signature
    let header = read_header(&mut r, false)?;
    let payload: Box<dyn Read> = match header.strings.get(&PAYLOAD_COMPRESSOR).map(String::as_str).unwrap_or("gzip") {
        "xz" | "lzma" => Box::new(liblzma::read::XzDecoder::new(r)),
        "zstd" => Box::new(zstd::stream::read::Decoder::new(r).map_err(|e| e.to_string())?),
        "gzip" => Box::new(flate2::read::GzDecoder::new(r)),
        other => return Err(format!("The package is compressed with {other}, which Olo can't unpack.")),
    };
    unpack_cpio(payload, dest, cancel, &header.files())
}

/// The files under [`PREFIX`] in a cpio archive, into `dest`: "newc"
/// (each entry says its name, mode and size), or rpm 6's stripped form
/// (`07070X` and a number: the file's place in `files`, the package
/// header's list, which says the rest; Chrome's packages since
/// 157.0.8103.78, built with rpm 6: the updater said "in a form Olo
/// can't unpack" and Chrome stayed as it was, 2026-11-17). An archive
/// may hold both, and ends as newc's do.
fn unpack_cpio(mut r: impl Read, dest: &Path, cancel: &AtomicBool, files: &[PackageFile]) -> Result<bool, String> {
    let damaged = |e: io::Error| format!("The package's contents are damaged: {e}.");
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    // Hard links: names waiting for the data that comes with the last one.
    let mut links: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    // (Stripped: how many of an inode's names have come; its data is
    // with the last.)
    let mut seen: HashMap<u64, u64> = HashMap::new();
    let mut at = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let mut magic = [0u8; 6];
        r.read_exact(&mut magic).map_err(damaged)?;
        // What the entry is, and how much of the archive is its data.
        let (name, ino, mode, nlink, size) = match &magic {
            b"070701" | b"070702" => {
                let mut head = [0u8; 104];
                r.read_exact(&mut head).map_err(damaged)?;
                let field = |i: usize| u64::from_str_radix(std::str::from_utf8(&head[i * 8..8 + i * 8]).unwrap_or("0"), 16).unwrap_or(0);
                let (ino, mode, nlink, size, namesize) = (field(0), field(1) as u32, field(4), field(6), field(11) as usize);
                let mut name = vec![0u8; namesize];
                r.read_exact(&mut name).map_err(damaged)?;
                at += 110 + namesize as u64;
                (String::from_utf8_lossy(&name[..namesize.saturating_sub(1)]).into_owned(), ino, mode, nlink, size)
            }
            b"07070X" => {
                let mut number = [0u8; 8];
                r.read_exact(&mut number).map_err(damaged)?;
                at += 14;
                let file = std::str::from_utf8(&number).ok().and_then(|n| usize::from_str_radix(n, 16).ok()).and_then(|i| files.get(i));
                let Some(f) = file else { return Err("The package's contents name a file its header doesn't have.".into()) };
                let kind = f.mode & 0o170000;
                // Data for a file, with the last of its names, and for a
                // link (where it goes, as in newc); none for a folder.
                let last = f.links <= 1 || {
                    let n = seen.entry(f.inode).or_insert(0);
                    *n += 1;
                    *n == f.links
                };
                let size = match kind {
                    0o100000 if last => f.size,
                    0o120000 => f.size,
                    _ => 0,
                };
                (f.name.clone(), f.inode, f.mode, f.links, size)
            }
            _ => return Err("The package's contents are in a form Olo can't unpack.".into()),
        };
        skip(&mut r, pad4(at))?;
        at += pad4(at);
        if name == "TRAILER!!!" {
            return Ok(true);
        }
        let rel = name.strip_prefix(PREFIX).filter(|r| !r.is_empty() && !r.split('/').any(|c| c == ".." || c.is_empty()));
        let path = rel.map(|r| dest.join(r));
        match (mode & 0o170000, &path) {
            (0o040000, Some(p)) => fs::create_dir_all(p).map_err(|e| e.to_string())?,
            (0o120000, Some(p)) => {
                let mut target = vec![0u8; size as usize];
                r.read_exact(&mut target).map_err(damaged)?;
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let _ = fs::remove_file(p);
                symlink(String::from_utf8_lossy(&target).as_ref(), p).map_err(|e| e.to_string())?;
                at += size;
                skip(&mut r, pad4(at))?;
                at += pad4(at);
                continue;
            }
            (0o100000, Some(p)) if size == 0 && nlink > 1 => links.entry(ino).or_default().push(p.clone()),
            (0o100000, Some(p)) => {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut f = fs::File::create(p).map_err(|e| e.to_string())?;
                io::copy(&mut (&mut r).take(size), &mut f).map_err(damaged)?;
                // No setuid or setgid bits: this is the user's copy.
                fs::set_permissions(p, fs::Permissions::from_mode(mode & 0o777)).map_err(|e| e.to_string())?;
                for other in links.remove(&ino).unwrap_or_default() {
                    fs::copy(p, &other).map_err(|e| e.to_string())?;
                }
                at += size;
                skip(&mut r, pad4(at))?;
                at += pad4(at);
                continue;
            }
            _ => {}
        }
        skip(&mut r, size)?;
        at += size;
        skip(&mut r, pad4(at))?;
        at += pad4(at);
    }
}

fn pad4(at: u64) -> u64 {
    (4 - at % 4) % 4
}

fn skip(r: &mut impl Read, n: u64) -> Result<(), String> {
    let copied = io::copy(&mut r.take(n), &mut io::sink()).map_err(|e| e.to_string())?;
    if copied == n { Ok(()) } else { Err("The package is cut short.".into()) }
}

/// Chrome's icons into the hicolor theme and a desktop entry that starts
/// it through this program (so it can be set up and updated here).
pub fn set_up(dir: &Path) -> Result<(), String> {
    let icons = data_home().join("icons/hicolor");
    for px in [16, 24, 32, 48, 64, 128, 256] {
        let from = dir.join(format!("product_logo_{px}.png"));
        if from.is_file() {
            let to = icons.join(format!("{px}x{px}/apps"));
            fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            fs::copy(&from, to.join("google-chrome.png")).map_err(|e| e.to_string())?;
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let apps = data_home().join("applications");
    fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
    let tmp = apps.join(".google-chrome.desktop.new");
    fs::write(&tmp, desktop_entry(&exe)).and_then(|_| fs::rename(&tmp, apps.join(olo_xdg::open::CHROME_ENTRY))).map_err(|e| e.to_string())?;
    // The default browser, unless the person chose another.
    olo_xdg::open::offer_browser(&apps).map(drop).map_err(|e| e.to_string())
}

/// Whether the desktop entry is ours (starts Chrome through `exe`).
pub fn entry_is_current(exe: &Path) -> bool {
    fs::read_to_string(data_home().join("applications/google-chrome.desktop")).is_ok_and(|t| t == desktop_entry(exe))
}

fn desktop_entry(exe: &Path) -> String {
    let exe = exe.display();
    format!(
        "[Desktop Entry]
Version=1.0
Name=Google Chrome
GenericName=Web Browser
Comment=Access the Internet
Exec={exe} %U
StartupNotify=true
Terminal=false
Icon=google-chrome
Type=Application
Categories=Network;WebBrowser;
MimeType=application/pdf;application/rdf+xml;application/rss+xml;application/xhtml+xml;application/xhtml_xml;application/xml;image/gif;image/jpeg;image/png;image/webp;text/html;text/xml;x-scheme-handler/http;x-scheme-handler/https;
StartupWMClass=google-chrome
# Olo lists Google Chrome among its own applications.
NoDisplay=true
Actions=new-window;new-private-window;

[Desktop Action new-window]
Name=New Window
Exec={exe} --new-window

[Desktop Action new-private-window]
Name=New Incognito Window
Exec={exe} --incognito
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_newest_package() {
        let repomd = r#"<repomd><data type="other"><location href="repodata/o.xml.gz"/></data>
            <data type="primary"><location href="repodata/abc-primary.xml.gz"/></data></repomd>"#;
        assert_eq!(primary_href(repomd).as_deref(), Some("repodata/abc-primary.xml.gz"));
        let sum = "a".repeat(64);
        let primary = format!(
            r#"<metadata>
            <package type="rpm"><name>google-chrome-stable</name><version epoch="0" ver="9.0.1.2" rel="1"/>
              <checksum type="sha256" pkgid="YES">{sum}</checksum><size package="10"/><location href="x/old.rpm"/></package>
            <package type="rpm"><name>google-chrome-stable</name><version epoch="0" ver="10.0.0.1" rel="1"/>
              <checksum type="sha256" pkgid="YES">{sum}</checksum><size package="20"/><location href="x/new.rpm"/></package>
            <package type="rpm"><name>google-chrome-beta</name><version epoch="0" ver="11.0.0.0" rel="1"/>
              <checksum type="sha256" pkgid="YES">{sum}</checksum><location href="x/beta.rpm"/></package>
            </metadata>"#
        );
        let p = newest(&primary).unwrap();
        assert_eq!((p.version.as_str(), p.href.as_str(), p.size), ("10.0.0.1", "x/new.rpm", 20));
    }

    /// A newc archive entry.
    fn entry(out: &mut Vec<u8>, name: &str, mode: u32, data: &[u8]) {
        let fields = [1, mode, 0, 0, 1, 0, data.len() as u32, 0, 0, 0, 0, name.len() as u32 + 1, 0];
        out.extend_from_slice(b"070701");
        for f in fields {
            out.extend_from_slice(format!("{f:08X}").as_bytes());
        }
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(data);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }

    #[test]
    fn unpacks_only_chrome() {
        let mut a = Vec::new();
        entry(&mut a, "./opt/google/chrome", 0o040755, b"");
        entry(&mut a, "./opt/google/chrome/google-chrome", 0o104755, b"#!/bin/sh\n");
        entry(&mut a, "./opt/google/chrome/lib/x.so", 0o100644, b"elf");
        entry(&mut a, "./opt/google/chrome/link", 0o120777, b"lib/x.so");
        entry(&mut a, "./usr/bin/google-chrome", 0o100755, b"no");
        entry(&mut a, "./opt/google/chrome/../../etc/evil", 0o100644, b"no");
        entry(&mut a, "TRAILER!!!", 0, b"");
        let dest = std::env::temp_dir().join(format!("olo-chrome-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        assert!(unpack_cpio(a.as_slice(), &dest, &AtomicBool::new(false), &[]).unwrap());
        assert_eq!(fs::read(dest.join("google-chrome")).unwrap(), b"#!/bin/sh\n");
        // The setuid bit isn't kept.
        assert_eq!(fs::metadata(dest.join("google-chrome")).unwrap().permissions().mode() & 0o7777, 0o755);
        assert_eq!(fs::read(dest.join("link")).unwrap(), b"elf");
        assert!(!dest.join("usr").exists() && !dest.parent().unwrap().join("etc/evil").exists());
        let _ = fs::remove_dir_all(&dest);
    }

    /// rpm 6's stripped payload: each entry a number, the file's place in
    /// the header's list, which says its name, mode and size (and that
    /// two names are one file, whose data comes with the last); a
    /// link's target is its data, as in newc.
    #[test]
    fn unpacks_a_stripped_archive_by_the_header() {
        let file = |name: &str, mode: u32, size: u64, _link: &str, inode: u64, links: u64| PackageFile { name: name.into(), mode, size, inode, links };
        let files = [
            file("./opt/google/chrome", 0o040755, 0, "", 1, 2),
            file("./opt/google/chrome/google-chrome", 0o104755, 10, "", 2, 1),
            file("./opt/google/chrome/lib/x.so", 0o100644, 3, "", 3, 1),
            file("./opt/google/chrome/link", 0o120777, 8, "lib/x.so", 4, 1),
            file("./usr/bin/google-chrome", 0o100755, 2, "", 5, 1),
            file("./opt/google/chrome/one", 0o100644, 4, "", 6, 2),
            file("./opt/google/chrome/two", 0o100644, 4, "", 6, 2),
            file("./opt/google/chrome/../../etc/evil", 0o100644, 2, "", 7, 1),
        ];
        let mut a = Vec::new();
        let mut put = |i: usize, data: &[u8]| {
            a.extend_from_slice(format!("07070X{i:08x}").as_bytes());
            while a.len() % 4 != 0 {
                a.push(0);
            }
            a.extend_from_slice(data);
            while a.len() % 4 != 0 {
                a.push(0);
            }
        };
        put(0, b"");
        put(1, b"#!/bin/sh\n");
        put(2, b"elf");
        put(3, b"lib/x.so");
        put(4, b"no");
        put(5, b"");
        put(6, b"same");
        put(7, b"no");
        entry(&mut a, "TRAILER!!!", 0, b"");
        // (On disk: /tmp is memory here.)
        let dest = PathBuf::from("/var/tmp").join(format!("olo-chrome-stripped-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        assert!(unpack_cpio(a.as_slice(), &dest, &AtomicBool::new(false), &files).unwrap());
        assert_eq!(fs::read(dest.join("google-chrome")).unwrap(), b"#!/bin/sh\n");
        assert_eq!(fs::metadata(dest.join("google-chrome")).unwrap().permissions().mode() & 0o7777, 0o755);
        assert_eq!(fs::read_link(dest.join("link")).unwrap(), Path::new("lib/x.so"));
        assert_eq!(fs::read(dest.join("link")).unwrap(), b"elf");
        assert_eq!((fs::read(dest.join("one")).unwrap(), fs::read(dest.join("two")).unwrap()), (b"same".to_vec(), b"same".to_vec()));
        assert!(!dest.join("usr").exists() && !dest.parent().unwrap().join("etc/evil").exists());
        // A number the header hasn't: said, not guessed at.
        let mut odd = Vec::new();
        odd.extend_from_slice(b"07070X00000063\0\0");
        assert!(unpack_cpio(odd.as_slice(), &dest, &AtomicBool::new(false), &files).unwrap_err().contains("header doesn't have"));
        let _ = fs::remove_dir_all(&dest);
    }

    /// A package as Google ships it (`OLO_CHROME_RPM`, a file fetched by
    /// hand; unpacked under `OLO_CHROME_UNPACK_TO`): Chrome's own files
    /// come out, its program whole. `cargo test --release -p olo-chrome
    /// unpacks_a_real_package -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn unpacks_a_real_package() {
        let (Some(rpm), Some(dest)) = (std::env::var_os("OLO_CHROME_RPM"), std::env::var_os("OLO_CHROME_UNPACK_TO")) else { return };
        let dest = PathBuf::from(dest);
        let _ = fs::remove_dir_all(&dest);
        let t = std::time::Instant::now();
        assert!(unpack_rpm(io::BufReader::new(fs::File::open(rpm).unwrap()), &dest, &AtomicBool::new(false)).unwrap());
        let chrome = fs::metadata(dest.join("chrome")).unwrap();
        let count = walk(&dest);
        println!("unpacked {count} files in {:.1} s; chrome is {} MB, mode {:o}", t.elapsed().as_secs_f32(), chrome.len() >> 20, chrome.permissions().mode() & 0o7777);
        assert!(chrome.len() > 100 << 20 && chrome.permissions().mode() & 0o111 != 0);
        assert!(dest.join("google-chrome").exists() && dest.join("product_logo_48.png").exists() && count > 100);
        fn walk(dir: &Path) -> usize {
            fs::read_dir(dir).map(|d| d.flatten().map(|e| if e.path().is_dir() && !e.path().is_symlink() { walk(&e.path()) } else { 1 }).sum()).unwrap_or(0)
        }
    }

    /// A header's arrays as rpm writes them: the files named from their
    /// folders, sizes of 64 bits before those of 32.
    #[test]
    fn a_headers_files() {
        let mut h = Header::default();
        let mut add = |tag: u32, kind: u32, count: usize, bytes: &[u8]| {
            h.index.push((tag, kind, h.data.len(), count));
            h.data.extend_from_slice(bytes);
        };
        add(1117, 8, 2, b"chrome\0icon.png\0");
        add(1118, 8, 2, b"/opt/google/chrome/\0/usr/share/icons/\0");
        add(1116, 4, 2, &[0, 0, 0, 0, 0, 0, 0, 1]);
        add(1030, 3, 2, &[0x81, 0xed, 0x81, 0xa4]);
        add(5008, 5, 2, &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9]);
        add(1028, 4, 2, &[0, 0, 0, 7, 0, 0, 0, 7]);
        add(1096, 4, 2, &[0, 0, 0, 5, 0, 0, 0, 6]);
        let files = h.files();
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].name.as_str(), files[0].mode, files[0].size, files[0].links), ("./opt/google/chrome/chrome", 0o100755, 1 << 32, 1));
        assert_eq!((files[1].name.as_str(), files[1].mode, files[1].size), ("./usr/share/icons/icon.png", 0o100644, 9));
    }
}
