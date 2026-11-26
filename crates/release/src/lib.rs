//! Olo's releases, as tools/release.sh makes them and GitHub keeps them:
//! an archive, `olo-VERSION-ARCH.tar.zst`, its SHA-256 beside it, and
//! the parts that file names besides (the wallpapers, an archive of their
//! own since 2026-11-17: with them Olo was more than the 2 GiB a file
//! GitHub takes). Found (the latest GitHub release of the repository's,
//! or one given), downloaded into the cache (kept: a second run, or one
//! after a failure, doesn't fetch it again), checked, unpacked into a
//! staging folder, then put in place as tools/install-session.sh does:
//! each program written beside its old self and renamed over it (a
//! running one keeps its file), the data copied over. Nothing is shelled
//! out to.
//!
//! Setup (apps/installer) installs Olo from here, all of it into a home
//! folder (`place`). An installed Olo upgrades from here too: its session
//! asks `olo-release` (main.rs) now and then whether there's a newer
//! release than what it has (`latest`, `installed_in`, `is_newer`), and
//! has it install one when the person chooses it in Deskbar
//! (`place_prefix`: only the programs that differ, what was installed
//! recorded, a part it already has not fetched again).

use std::{
    io::{Read, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use std::os::unix::fs::PermissionsExt as _;

use sha2::Digest as _;

/// The repository whose releases hold Olo.
pub const REPO: &str = "benmyles/olo";

/// GitHub's API for the repository (`OLO_RELEASES`: another, a test's
/// own server).
fn api() -> String {
    std::env::var("OLO_RELEASES").ok().filter(|v| !v.is_empty()).map(|v| v.trim_end_matches('/').to_owned()).unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}"))
}

/// Where an archive comes from.
#[derive(Debug, Clone)]
pub enum Origin {
    Url(String),
    File(PathBuf),
}

/// The archive to install.
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub archive: Origin,
    /// Its `.sha256` (as `sha256sum` writes it).
    pub sums: Origin,
}

fn agent() -> ureq::Agent {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder().tls_config(tls).max_redirects(10).user_agent("Olo").timeout_connect(Some(Duration::from_secs(30))).timeout_per_read(Some(Duration::from_secs(60))).build().into()
}

/// This computer's architecture, as release files are named.
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// The archive to install: `given` (a URL or a file, its `.sha256`
/// beside it), else the newest release's for this architecture.
pub fn find(given: Option<&str>) -> Result<Source, String> {
    if let Some(g) = given {
        let name = g.rsplit('/').next().unwrap_or(g).to_owned();
        return Ok(if g.starts_with("http://") || g.starts_with("https://") {
            Source { name, archive: Origin::Url(g.to_owned()), sums: Origin::Url(format!("{g}.sha256")) }
        } else {
            Source { name, archive: Origin::File(PathBuf::from(g)), sums: Origin::File(PathBuf::from(format!("{g}.sha256"))) }
        });
    }
    latest().map(|r| r.source)
}

/// A release, as GitHub describes it.
#[derive(Debug, Clone)]
pub struct Release {
    /// "2026.11.17-4e8d72a5", as its archive is named (and its VERSION says).
    pub version: String,
    /// Its tag ("v2026.11.17") and title ("Olo 2026.11.17").
    pub tag: String,
    pub title: String,
    /// When it was published, and when the commit it was made from was
    /// (seconds since the epoch; the commit's where GitHub says).
    pub published: u64,
    pub committed: Option<u64>,
    pub source: Source,
    /// Its checksum file: the archive's SHA-256 and its parts'.
    pub sums: String,
    /// Each of its files' sizes, by name.
    pub sizes: Vec<(String, u64)>,
}

impl Release {
    /// When what it holds was made: its commit's time, else when it was
    /// published.
    pub fn made(&self) -> u64 {
        self.committed.unwrap_or(self.published)
    }

    /// What installing it over `installed` downloads: the archive, and
    /// each part not installed already (by its SHA-256), in bytes.
    pub fn download_bytes(&self, installed: &Installed) -> u64 {
        let size = |name: &str| self.sizes.iter().find(|(n, _)| n == name).map_or(0, |(_, s)| *s);
        size(&self.source.name) + parts(&self.sums, &self.source.name).iter().filter(|(_, h)| !installed.sums.contains(h)).map(|(n, _)| size(n)).sum::<u64>()
    }
}

/// GitHub's answer to `url` (its API's JSON).
fn api_json(url: &str, what: &str) -> Result<serde_json::Value, String> {
    let body = olo_net::across_changes(Duration::from_secs(30), || agent().get(url).header("Accept", "application/vnd.github+json").call()).map_err(|e| match e {
        ureq::Error::StatusCode(404) => format!("GitHub has no {what}"),
        ureq::Error::StatusCode(code) => format!("GitHub answered {code} when asked for {what}"),
        e => format!("GitHub couldn't be reached: {e}"),
    })?;
    body.into_body().read_json().map_err(|e| format!("GitHub's answer couldn't be read: {e}"))
}

/// The newest release of Olo, with its checksum file, for this
/// architecture.
pub fn latest() -> Result<Release, String> {
    latest_from(&api())
}

/// [`latest`], asking the API at `api`.
pub fn latest_from(api: &str) -> Result<Release, String> {
    let v = api_json(&format!("{api}/releases/latest"), "release of Olo yet")?;
    release_of(api, &v)
}

/// The release of Olo tagged `tag` ("v2026.11.17"), with its checksum
/// file: one to install that isn't the newest (About Olo's other
/// releases).
pub fn by_tag(tag: &str) -> Result<Release, String> {
    by_tag_from(&api(), tag)
}

/// [`by_tag`], asking the API at `api`.
pub fn by_tag_from(api: &str, tag: &str) -> Result<Release, String> {
    if tag.is_empty() || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
        return Err(format!("“{tag}” isn't a release's tag"));
    }
    let v = api_json(&format!("{api}/releases/tags/{tag}"), "such release")?;
    release_of(api, &v)
}

/// A release as a list of them says it: no more than its names and when
/// it was published (what installing it needs is asked for then).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Listed {
    pub tag: String,
    pub title: String,
    /// As its archive is named ("2026.11.17-4e8d72a5"); empty where it has
    /// none for this architecture.
    pub version: String,
    pub published: u64,
    /// Its archive's size, in bytes (the wallpapers' is fetched only
    /// where they changed).
    #[serde(default)]
    pub size: u64,
}

/// Olo's releases, the newest first (GitHub's last 30; drafts and
/// pre-releases left out, and those with nothing for this architecture).
pub fn list() -> Result<Vec<Listed>, String> {
    list_from(&api())
}

/// [`list`], asking the API at `api`.
pub fn list_from(api: &str) -> Result<Vec<Listed>, String> {
    let v = api_json(&format!("{api}/releases?per_page=30"), "releases of Olo yet")?;
    let ending = format!("-{}.tar.zst", arch());
    let mut out: Vec<Listed> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false) && !r["prerelease"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let asset = r["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.starts_with("olo-") && n.ends_with(&ending) && !n.contains("wallpapers")))?;
            let archive = asset["name"].as_str()?;
            Some(Listed {
                size: asset["size"].as_u64().unwrap_or(0),
                tag: r["tag_name"].as_str()?.to_owned(),
                title: r["name"].as_str().filter(|t| !t.is_empty()).or(r["tag_name"].as_str())?.to_owned(),
                version: version_in(archive).to_owned(),
                published: r["published_at"].as_str().and_then(parse_time).unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.published.cmp(&a.published));
    Ok(out)
}

/// A release from GitHub's description of it (`v`), its checksum file
/// and its commit's time asked for from the API at `api`.
fn release_of(api: &str, v: &serde_json::Value) -> Result<Release, String> {
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let ending = format!("-{}.tar.zst", arch());
    let archive = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.starts_with("olo-") && n.ends_with(&ending)))
        .ok_or_else(|| format!("{} has nothing for {}", v["name"].as_str().or(v["tag_name"].as_str()).unwrap_or("This release of Olo"), arch()))?;
    let name = archive["name"].as_str().unwrap_or_default().to_owned();
    let sums = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&format!("{name}.sha256")))
        .ok_or_else(|| format!("{} has no checksum for its archive", v["name"].as_str().or(v["tag_name"].as_str()).unwrap_or("This release of Olo")))?;
    let source = Source {
        archive: Origin::Url(archive["browser_download_url"].as_str().unwrap_or_default().to_owned()),
        sums: Origin::Url(sums["browser_download_url"].as_str().unwrap_or_default().to_owned()),
        name: name.clone(),
    };
    let sums = read_sums(&source.sums)?;
    // When its commit was made, where the release names one (a branch's
    // name would be its newest commit, not the release's).
    let commit = v["target_commitish"].as_str().filter(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()));
    let committed = commit.and_then(|c| api_json(&format!("{api}/commits/{c}"), "such commit").ok()).and_then(|c| c["commit"]["committer"]["date"].as_str().and_then(parse_time));
    Ok(Release {
        version: version_in(&name).to_owned(),
        tag: v["tag_name"].as_str().unwrap_or_default().to_owned(),
        title: v["name"].as_str().filter(|t| !t.is_empty()).or(v["tag_name"].as_str()).unwrap_or_default().to_owned(),
        published: v["published_at"].as_str().and_then(parse_time).unwrap_or_default(),
        committed,
        sizes: assets.iter().filter_map(|a| Some((a["name"].as_str()?.to_owned(), a["size"].as_u64().unwrap_or(0)))).collect(),
        source,
        sums,
    })
}

/// The version an archive's name says: `olo-2026.11.17-4e8d72a5-x86_64.tar.zst`
/// is 2026.11.17-4e8d72a5.
pub fn version_in(name: &str) -> &str {
    let v = name.strip_prefix("olo-").unwrap_or(name).strip_suffix(".tar.zst").unwrap_or(name);
    v.strip_suffix(&format!("-{}", arch())).unwrap_or(v)
}

/// "2026-11-17T20:13:39Z" (GitHub's times) in seconds since the epoch.
pub fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim().strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let d: Vec<i64> = date.split('-').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let t: Vec<i64> = time.split(':').map(|p| p.split('.').next().unwrap_or(p).parse().ok()).collect::<Option<_>>()?;
    let ([y, m, day], [h, mi, sec]) = (<[i64; 3]>::try_from(d).ok()?, <[i64; 3]>::try_from(t).ok()?);
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + sec).ok()
}

/// A checksum file's text, from where it is.
fn read_sums(sums: &Origin) -> Result<String, String> {
    match sums {
        Origin::Url(u) => olo_net::across_changes(Duration::from_secs(30), || agent().get(u).call())
            .map_err(|e| format!("the checksum couldn't be downloaded ({u}): {e}"))?
            .into_body()
            .read_to_string()
            .map_err(|e| e.to_string()),
        Origin::File(p) => std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display())),
    }
}

/// A `.sha256`'s lines: each SHA-256 and the file it is of.
pub fn sums_of(sums: &str) -> Vec<(String, &str)> {
    sums.lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .map(|(h, n)| (h.to_ascii_lowercase(), n.trim().trim_start_matches('*')))
        .filter(|(h, _)| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()))
        .collect()
}

/// The SHA-256 a `.sha256` names for `name` (or its first).
fn expected(sums: &str, name: &str) -> Option<String> {
    let lines = sums_of(sums);
    lines.iter().find(|(_, n)| *n == name).or(lines.first()).map(|(h, _)| h.clone())
}

/// The archive's parts, as its `.sha256` names them: every other
/// archive listed there (a plain file name ending `.tar.zst`), with its
/// SHA-256. They are where the archive is, and unpacked over it.
pub fn parts(sums: &str, name: &str) -> Vec<(String, String)> {
    sums_of(sums).into_iter().filter(|(_, n)| *n != name && n.ends_with(".tar.zst") && !n.contains(['/', '\\']) && !n.starts_with('.')).map(|(h, n)| (n.to_owned(), h)).collect()
}

impl Origin {
    /// The file `name` beside this one.
    fn beside(&self, name: &str) -> Origin {
        match self {
            Origin::Url(u) => Origin::Url(format!("{}/{name}", u.rsplit_once('/').map_or("", |(dir, _)| dir))),
            Origin::File(p) => Origin::File(p.with_file_name(name)),
        }
    }
}

pub fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Where downloads are kept between runs.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")).join("olo-installer")
}

/// The archive and its parts (the archive first), here and checked:
/// each downloaded into the cache unless a checked copy is there; a part
/// whose SHA-256 is in `have` (installed already: `Installed::sums`)
/// left out. `progress` gets (which file, of how many, bytes so far, of
/// how many).
pub fn fetch(src: &Source, have: &[String], cancel: &AtomicBool, progress: &mut dyn FnMut(usize, usize, u64, u64)) -> Result<Vec<PathBuf>, String> {
    fetch_into(src, have, &cache_dir(), cancel, progress)
}

/// [`fetch`], downloading into `cache`.
pub fn fetch_into(src: &Source, have: &[String], cache: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(usize, usize, u64, u64)) -> Result<Vec<PathBuf>, String> {
    let sums = read_sums(&src.sums)?;
    let want = expected(&sums, &src.name).ok_or("the checksum file names no SHA-256")?;
    let mut files = vec![(src.name.clone(), src.archive.clone(), want)];
    files.extend(parts(&sums, &src.name).into_iter().filter(|(_, h)| !have.contains(h)).map(|(name, want)| (name.clone(), src.archive.beside(&name), want)));
    let n = files.len();
    let mut here = Vec::new();
    for (i, (name, origin, want)) in files.iter().enumerate() {
        here.push(fetch_one(name, origin, want, cache, cancel, &mut |got, total| progress(i + 1, n, got, total))?);
    }
    Ok(here)
}

/// One file of a release, `name` from `origin`, whose SHA-256 is `want`.
fn fetch_one(name: &str, origin: &Origin, want: &str, cache: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let path = match origin {
        Origin::File(p) => p.clone(),
        Origin::Url(url) => {
            let dir = cache.to_path_buf();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = dir.join(name);
            if path.exists() && sha256_of(&path).is_ok_and(|h| h == want) {
                return Ok(path);
            }
            let part = dir.join(format!("{name}.part"));
            // (Across a network change, a cable pulled or Wi-Fi left, it
            // goes on where it broke off, on the new one.)
            let mut body = olo_net::http::Download::start(&agent(), url, None, Some(cancel)).map_err(|e| match e {
                ureq::Error::StatusCode(code) => format!("the download answered {code} ({url})"),
                e => format!("the download couldn't start: {e}"),
            })?;
            let total = body.total().unwrap_or(0);
            let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            let mut buf = vec![0u8; 1 << 20];
            let mut got = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("stopped".into());
                }
                let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
                got += n as u64;
                progress(got, total);
            }
            out.sync_all().map_err(|e| e.to_string())?;
            std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
            path
        }
    };
    let have = sha256_of(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if have != want {
        if matches!(origin, Origin::Url(_)) {
            let _ = std::fs::remove_file(&path);
        }
        return Err(format!("{name} isn't what was published (its SHA-256 is {have}, not {want})"));
    }
    Ok(path)
}

/// Reads through, counting what's read (for the progress bar): `before`
/// is what the archives before this one came to.
struct Counting<'a, R> {
    inner: R,
    read: u64,
    before: u64,
    total: u64,
    progress: &'a mut dyn FnMut(u64, u64),
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        (self.progress)(self.before + self.read, self.total);
        Ok(n)
    }
}

/// The archives (Olo's, then its parts) unpacked into `into` (made empty
/// first), one over another: `into/olo/…`.
pub fn unpack(archives: &[PathBuf], into: &Path, cancel: &AtomicBool, progress: Box<dyn FnMut(u64, u64)>) -> Result<PathBuf, String> {
    unpack_telling(archives, into, cancel, progress)
}

/// [`unpack`], told through something that needn't outlive the call.
fn unpack_telling<'a>(archives: &[PathBuf], into: &Path, cancel: &AtomicBool, mut progress: Box<dyn FnMut(u64, u64) + 'a>) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let sizes: Vec<u64> = archives.iter().map(|a| std::fs::metadata(a).map(|m| m.len()).unwrap_or(0)).collect();
    let (total, mut before) = (sizes.iter().sum(), 0);
    for (archive, size) in archives.iter().zip(&sizes) {
        let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
        let counting = Counting { inner: std::io::BufReader::new(file), read: 0, before, total, progress: &mut *progress };
        let decoder = zstd::stream::read::Decoder::new(counting).map_err(|e| format!("the archive couldn't be opened: {e}"))?;
        let mut tar = tar::Archive::new(decoder);
        tar.set_preserve_permissions(true);
        for entry in tar.entries().map_err(|e| format!("the archive couldn't be read: {e}"))? {
            if cancel.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            let mut entry = entry.map_err(|e| format!("the archive is damaged: {e}"))?;
            // (Only what's under olo/, nothing outside the folder.)
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            if !path.starts_with("olo") || path.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir)) {
                return Err(format!("the archive has something it shouldn't: {}", path.display()));
            }
            entry.unpack_in(into).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        before += size;
    }
    let olo = into.join("olo");
    if !olo.join("bin/olo-session").is_file() {
        return Err("the archive has no Olo in it".into());
    }
    Ok(olo)
}

/// The version an unpacked Olo is.
pub fn version_of(olo: &Path) -> String {
    std::fs::read_to_string(olo.join("VERSION")).unwrap_or_default().trim().to_owned()
}

/// Copy `from` (a file) to `to`, through a sibling renamed into place.
fn put_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = to.with_file_name(format!(".{}.new", to.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(from, &tmp)?;
    std::fs::rename(&tmp, to)
}

/// Copy the tree `from` into `to`, every file over its old self.
fn put_tree(from: &Path, to: &Path, done: &mut dyn FnMut()) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            put_tree(&e.path(), &target, done)?;
        } else {
            put_file(&e.path(), &target)?;
            done();
        }
    }
    Ok(())
}

fn count_files(dir: &Path) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { count_files(&e.path()) } else { 1 }).sum()
}

/// What goes in the home folder, from an unpacked Olo `olo`: its programs
/// in `home/.local/bin`, its data in `home/.local/share/olo`, and `op`,
/// `pbcopy` and `pbpaste` beside the programs (never over the person's
/// own). `progress` gets (files so far, of how many).
pub fn place(olo: &Path, home: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    let (bin, share) = (home.join(".local/bin"), home.join(".local/share/olo"));
    let err = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(&bin).map_err(err)?;
    let total = count_files(&olo.join("bin")) + count_files(&olo.join("share/olo"));
    let mut n = 0u64;
    for e in std::fs::read_dir(olo.join("bin")).map_err(err)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        n += 1;
        progress(n, total);
        // (Face unlock's PAM module goes with the programs: Preferences
        // installs it from beside them. The keyring's is the login
        // screen's step, from the unpacked Olo.)
        if !name.starts_with("olo-") && name != "libpam_olo_face.so" {
            continue;
        }
        put_file(&e.path(), &bin.join(&name)).map_err(|e| format!("{}: {e}", bin.join(&name).display()))?;
    }
    put_tree(&olo.join("share/olo"), &share, &mut || {
        n += 1;
        progress(n, total);
    })
    .map_err(|e| format!("{}: {e}", share.display()))?;
    std::fs::write(share.join("version"), version_of(olo) + "\n").map_err(err)?;
    for (name, ours) in [("op", "olo-op"), ("pbcopy", "olo-pbcopy"), ("pbpaste", "olo-pbpaste")] {
        let link = bin.join(name);
        if bin.join(ours).is_file() && std::fs::symlink_metadata(&link).is_err() {
            let _ = std::os::unix::fs::symlink(ours, &link);
        }
    }
    Ok(())
}

/// The version installed in `home`, if any.
pub fn installed(home: &Path) -> Option<String> {
    let v = std::fs::read_to_string(home.join(".local/share/olo/version")).ok()?;
    let v = v.trim();
    (!v.is_empty() && home.join(".local/bin/olo-session").is_file()).then(|| v.to_owned())
}

/// What an installed Olo under `prefix` (`~/.local`: its programs in
/// `bin`, its data in `share/olo`) is, as far as its releases go.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Installed {
    /// The release's version it was installed from (Setup's, or an
    /// upgrade's): `share/olo/version`.
    pub version: Option<String>,
    /// When that release's commit was made (an upgrade records it:
    /// `share/olo/release.json`).
    pub committed: Option<u64>,
    /// When its programs were last put there (the newest of them), in
    /// seconds since the epoch: what a build tree's install leaves to go
    /// by.
    pub at: Option<u64>,
    /// The SHA-256 of each archive of the release installed
    /// (`share/olo/release.sha256`): a part it already has isn't
    /// fetched again (the wallpapers, 1.6 GB, change rarely).
    pub sums: Vec<String>,
}

/// What's installed under `prefix`.
pub fn installed_in(prefix: &Path) -> Installed {
    let share = prefix.join("share/olo");
    let version = std::fs::read_to_string(share.join("version")).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let record: serde_json::Value = std::fs::read(share.join("release.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let at = std::fs::read_dir(prefix.join("bin"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("olo-"))
        .filter_map(|e| e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .max();
    let sums = std::fs::read_to_string(share.join("release.sha256")).map(|t| sums_of(&t).into_iter().map(|(h, _)| h).collect()).unwrap_or_default();
    Installed { version, committed: record["committed"].as_u64(), at, sums }
}

/// Whether `release` is newer than what's `installed`: not the version
/// it is; made after the release it came from was, where that's known,
/// else after its programs were last put there (a build tree's install:
/// a release made from an older commit than the build installed isn't
/// offered over it).
pub fn is_newer(release: &Release, installed: &Installed) -> bool {
    if installed.version.as_deref() == Some(release.version.as_str()) {
        return false;
    }
    match (installed.committed, installed.at) {
        (Some(c), _) => release.made() > c,
        // (A minute's grace: a build installed as the release was made.)
        (None, Some(at)) => release.made() > at + 60,
        (None, None) => true,
    }
}

/// Whether the files at `a` and `b` hold the same.
fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let (Ok(na), Ok(nb)) = (read_full(&mut fa, &mut ba), read_full(&mut fb, &mut bb)) else { return false };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// Reads until `buf` is full or the file ends; how much.
fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// An unpacked Olo `olo` put in place under `prefix` (`~/.local`), as an
/// upgrade does: each program that differs from the one there, written
/// beside it and renamed over it (a running one keeps its file); the
/// keyring's PAM module in `lib/olo` (Olo Doctor puts it where the login
/// screen loads it); the data copied over; `retire`'s programs (ones Olo
/// no longer has) taken away; and what was installed recorded (its
/// version, and, from `release`, its commit's time and checksum file).
/// The programs that changed, by name. `progress` gets (files so far,
/// of how many).
pub fn place_prefix(olo: &Path, prefix: &Path, retire: &[String], release: Option<&Release>, progress: &mut dyn FnMut(u64, u64)) -> Result<Vec<String>, String> {
    let (bin, share) = (prefix.join("bin"), prefix.join("share/olo"));
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    std::fs::create_dir_all(&bin).map_err(|e| err(&bin, e))?;
    let total = count_files(&olo.join("bin")) + count_files(&olo.join("share/olo"));
    let mut n = 0u64;
    let mut changed = Vec::new();
    for e in std::fs::read_dir(olo.join("bin")).map_err(|e| err(&olo.join("bin"), e))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        n += 1;
        progress(n, total);
        let target = if name == "pam_olo_keyring.so" {
            let lib = prefix.join("lib/olo");
            std::fs::create_dir_all(&lib).map_err(|e| err(&lib, e))?;
            lib.join(&name)
        } else if (name.starts_with("olo-") && !name.contains('.')) || name == "libpam_olo_face.so" {
            bin.join(&name)
        } else {
            continue;
        };
        if same_file(&e.path(), &target) {
            continue;
        }
        put_file(&e.path(), &target).map_err(|e| err(&target, e))?;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).map_err(|e| err(&target, e))?;
        if target.parent() == Some(bin.as_path()) {
            changed.push(name);
        }
    }
    changed.sort();
    if olo.join("share/olo").is_dir() {
        put_tree(&olo.join("share/olo"), &share, &mut || {
            n += 1;
            progress(n, total);
        })
        .map_err(|e| err(&share, e))?;
    }
    for name in retire {
        let gone = bin.join(name);
        match std::fs::remove_file(&gone) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(&gone, e)),
        }
    }
    std::fs::create_dir_all(&share).map_err(|e| err(&share, e))?;
    std::fs::write(share.join("version"), version_of(olo) + "\n").map_err(|e| err(&share, e))?;
    if let Some(r) = release {
        std::fs::write(share.join("release.sha256"), &r.sums).map_err(|e| err(&share, e))?;
        let record = serde_json::json!({ "version": r.version, "tag": r.tag, "committed": r.committed, "published": r.published });
        std::fs::write(share.join("release.json"), record.to_string()).map_err(|e| err(&share, e))?;
    }
    for (name, ours) in [("op", "olo-op"), ("pbcopy", "olo-pbcopy"), ("pbpaste", "olo-pbpaste")] {
        let link = bin.join(name);
        if bin.join(ours).is_file() && std::fs::symlink_metadata(&link).is_err() {
            let _ = std::os::unix::fs::symlink(ours, &link);
        }
    }
    Ok(changed)
}

/// What [`install`] is asked.
#[derive(Debug, Clone, Default)]
pub struct Install {
    /// An archive (a file or a URL, its `.sha256` beside it), not one of
    /// GitHub's releases.
    pub from: Option<String>,
    /// The release tagged so (an older one too), not the newest.
    pub tag: Option<String>,
    /// Programs Olo no longer has, taken away.
    pub retire: Vec<String>,
    /// Parts left out, each by how its file's name begins
    /// (`olo-wallpapers-`: a Rex host's screens do without them, 1.6 GB).
    pub without: Vec<String>,
}

/// A release downloaded, checked, unpacked and put in place under
/// `prefix` (`olo-release install`, and Olo put on a Rex host for its
/// agents' screens: `olo-rex-link screen install`): the newest, or the
/// one `ask` names. `tell` hears how far it is ("downloading",
/// "unpacking", "installing": done, of how much), as often as it moves.
/// The programs that changed, and the version now there.
pub fn install(prefix: &Path, ask: &Install, cancel: &AtomicBool, tell: &mut dyn FnMut(&'static str, u64, u64)) -> Result<(Vec<String>, String), String> {
    install_with(prefix, ask, &cache_dir(), cancel, tell)
}

/// [`install`], with its downloads and what it unpacks in `cache`.
pub fn install_with(prefix: &Path, ask: &Install, cache: &Path, cancel: &AtomicBool, tell: &mut dyn FnMut(&'static str, u64, u64)) -> Result<(Vec<String>, String), String> {
    let mut installed = installed_in(prefix);
    // The newest release (its checksum file and commit known: recorded
    // as it's installed), or the archive given.
    let (source, release) = match (&ask.from, &ask.tag) {
        (Some(given), _) => (find(Some(given))?, None),
        (None, Some(tag)) => {
            let r = by_tag(tag)?;
            (r.source.clone(), Some(r))
        }
        (None, None) => {
            let r = latest()?;
            (r.source.clone(), Some(r))
        }
    };
    // (A part left out is one it "has": never fetched.)
    if !ask.without.is_empty() {
        let sums = match &release {
            Some(r) => r.sums.clone(),
            None => read_sums(&source.sums)?,
        };
        installed.sums.extend(parts(&sums, &source.name).into_iter().filter(|(name, _)| ask.without.iter().any(|w| name.starts_with(w.as_str()))).map(|(_, hash)| hash));
    }
    // (Bytes across all the files: the archive, and the parts it hasn't.)
    let total: u64 = release.as_ref().map_or(0, |r| r.download_bytes(&installed));
    let mut before = 0u64;
    let mut current = (0usize, 0u64);
    let archives = fetch_into(&source, &installed.sums, cache, cancel, &mut |file, _, got, of| {
        if file != current.0 {
            before += current.1;
            current = (file, 0);
        }
        current.1 = of;
        let (done, all) = if total > 0 { (before + got, total) } else { (got, of) };
        tell("downloading", done, all);
    })?;
    let stage = cache.join("upgrade");
    let olo = unpack_telling(&archives, &stage, cancel, Box::new(|done, all| tell("unpacking", done, all)))?;
    let version = version_of(&olo);
    let changed = place_prefix(&olo, prefix, &ask.retire, release.as_ref(), &mut |done, all| tell("installing", done, all))?;
    // What was downloaded, only kept until it's in place: 2 GB in the
    // cache otherwise.
    let _ = std::fs::remove_dir_all(&stage);
    for a in archives.iter().filter(|a| a.starts_with(cache)) {
        let _ = std::fs::remove_file(a);
    }
    Ok((changed, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checksum_file_names_the_hash() {
        let h = "a".repeat(64);
        assert_eq!(expected(&format!("{h}  olo-1-x86_64.tar.zst\n"), "olo-1-x86_64.tar.zst"), Some(h.clone()));
        assert_eq!(expected(&format!("{h} *olo-1-x86_64.tar.zst\n"), "olo-1-x86_64.tar.zst"), Some(h.clone()));
        assert_eq!(expected("nonsense", "x"), None);
    }

    /// A release installed from its files under a prefix: its programs
    /// and data there, what it is recorded, the archive given left where
    /// it was; and without a part (a Rex host's screens do without the
    /// wallpapers), that part's files not there, nor asked for (its file
    /// isn't even beside the archive).
    #[test]
    fn a_release_is_installed_under_a_prefix_without_a_part_left_out() {
        let dir = std::env::temp_dir().join(format!("olo-release-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = |name: &str, files: &[(&str, &str, u32)]| {
            let path = dir.join(name);
            let mut tar = tar::Builder::new(zstd::stream::write::Encoder::new(std::fs::File::create(&path).unwrap(), 1).unwrap());
            for (file, text, mode) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(text.len() as u64);
                header.set_mode(*mode);
                header.set_cksum();
                tar.append_data(&mut header, file, text.as_bytes()).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
            sha256_of(&path).unwrap()
        };
        let olo = archive(
            "olo-9-x86_64.tar.zst",
            &[("olo/VERSION", "9\n", 0o644), ("olo/bin/olo-session", "#!/bin/sh\n", 0o755), ("olo/bin/olo-screen", "#!/bin/sh\n", 0o755), ("olo/share/olo/icons/a.svg", "<svg/>", 0o644)],
        );
        let walls = archive("olo-wallpapers-9.tar.zst", &[("olo/share/olo/wallpapers/hill.webp", "a hill", 0o644)]);
        std::fs::write(dir.join("olo-9-x86_64.tar.zst.sha256"), format!("{olo}  olo-9-x86_64.tar.zst\n{walls}  olo-wallpapers-9.tar.zst\n")).unwrap();
        let from = Some(dir.join("olo-9-x86_64.tar.zst").to_string_lossy().into_owned());
        let stop = AtomicBool::new(false);

        // Without the wallpapers, whose file is gone meanwhile.
        std::fs::rename(dir.join("olo-wallpapers-9.tar.zst"), dir.join("kept")).unwrap();
        let (host, cache) = (dir.join("host"), dir.join("cache"));
        let mut said = Vec::new();
        let ask = Install { from: from.clone(), without: vec!["olo-wallpapers-".into()], ..Default::default() };
        let (changed, version) = install_with(&host, &ask, &cache, &stop, &mut |doing, _, _| {
            if said.last() != Some(&doing) {
                said.push(doing);
            }
        })
        .unwrap();
        assert_eq!((changed, version.as_str()), (vec!["olo-screen".to_owned(), "olo-session".to_owned()], "9"));
        assert_eq!(said, ["unpacking", "installing"], "nothing is downloaded from a file");
        assert!(host.join("bin/olo-session").is_file() && host.join("share/olo/icons/a.svg").is_file());
        assert!(!host.join("share/olo/wallpapers").exists(), "the part left out");
        assert_eq!(installed_in(&host).version.as_deref(), Some("9"));
        assert!(dir.join("olo-9-x86_64.tar.zst").is_file(), "an archive given is the giver's to keep");
        assert!(!cache.join("upgrade").exists(), "what was unpacked is cleared away");
        // Again: nothing differs, nothing is put.
        assert_eq!(install_with(&host, &ask, &cache, &stop, &mut |_, _, _| {}).unwrap().0, Vec::<String>::new());

        // With every part: the wallpapers too (and asked for: without
        // their file it fails, saying which).
        let all = Install { from, ..Default::default() };
        let whole = dir.join("whole");
        let missing = install_with(&whole, &all, &cache, &stop, &mut |_, _, _| {}).unwrap_err();
        assert!(missing.contains("olo-wallpapers-9.tar.zst"), "{missing}");
        std::fs::rename(dir.join("kept"), dir.join("olo-wallpapers-9.tar.zst")).unwrap();
        install_with(&whole, &all, &cache, &stop, &mut |_, _, _| {}).unwrap();
        assert_eq!(std::fs::read_to_string(whole.join("share/olo/wallpapers/hill.webp")).unwrap(), "a hill");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The archive's parts are the other archives its checksum file
    /// names, each beside it; nothing else there is one.
    #[test]
    fn the_checksum_file_names_the_parts() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let sums = format!("{a}  olo-1-x86_64.tar.zst\n{b}  olo-wallpapers-1.tar.zst\n{b}  ../evil.tar.zst\n{b}  notes.txt\nshort  x.tar.zst\n");
        assert_eq!(expected(&sums, "olo-1-x86_64.tar.zst"), Some(a.clone()));
        assert_eq!(parts(&sums, "olo-1-x86_64.tar.zst"), vec![("olo-wallpapers-1.tar.zst".to_owned(), b.clone())]);
        assert_eq!(parts(&format!("{a}  olo-1-x86_64.tar.zst\n"), "olo-1-x86_64.tar.zst"), vec![]);
        let beside = |o: Origin| match o.beside("w.tar.zst") {
            Origin::Url(u) => u,
            Origin::File(p) => p.to_string_lossy().into_owned(),
        };
        assert_eq!(beside(Origin::Url("https://example.org/releases/download/v1/olo-1-x86_64.tar.zst".into())), "https://example.org/releases/download/v1/w.tar.zst");
        assert_eq!(beside(Origin::File("/var/tmp/dist/olo-1-x86_64.tar.zst".into())), "/var/tmp/dist/w.tar.zst");
    }

    /// A release as tools/release.sh made it (`OLO_RELEASE=…/olo-VERSION-ARCH.tar.zst`),
    /// found, checked and unpacked as Setup does, into a folder beside
    /// it, removed after: it has what an installed Olo needs. `cargo test
    /// --release -p olo-installer a_real_release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_real_release_unpacks() {
        let Some(given) = std::env::var_os("OLO_RELEASE") else { return };
        let given = PathBuf::from(given);
        let stop = AtomicBool::new(false);
        let source = find(given.to_str()).unwrap();
        let archives = fetch(&source, &[], &stop, &mut |_, _, _, _| {}).unwrap();
        println!("{} archives: {:?}", archives.len(), archives.iter().map(|a| a.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>());
        assert!(archives.len() >= 2, "Olo and its wallpapers");
        let stage = given.with_file_name("unpack-test");
        let t = std::time::Instant::now();
        let olo = unpack(&archives, &stage, &stop, Box::new(|_, _| {})).unwrap();
        println!("version {}, unpacked in {:.1} s: {} files", version_of(&olo), t.elapsed().as_secs_f64(), count_files(&olo));
        for needed in [
            "bin/olo-session",
            "bin/olo-app-server",
            "bin/olo-faced",
            "bin/libpam_olo_face.so",
            "bin/pam_olo_keyring.so",
            "share/olo/icons/svg/olo.svg",
            "share/olo/brand",
            "share/olo/fonts",
            "share/olo/ocr/text-detection.onnx",
            "share/olo/face/sface.onnx",
            "share/olo/themes/platinum/icons/recolour.txt",
            "share/olo/wallpapers/pack.txt",
            "share/olo/wallpapers/4k",
            "share/olo/wallpapers/tiles",
            "system/session/olo.desktop",
            "LICENSE",
        ] {
            assert!(olo.join(needed).exists(), "{needed} is missing");
        }
        for sources in ["share/olo/icons/src", "share/olo/wallpapers/src", "share/olo/icons/reference", "share/olo/icons/16", "share/olo/themes/platinum/icons/16"] {
            assert!(!olo.join(sources).exists(), "{sources} isn't for a release");
        }
        std::fs::remove_dir_all(&stage).unwrap();
    }

    /// A server for a test: answers each GET with `answer(path)` (none:
    /// 404), until the test ends. Its address.
    fn serve(answer: impl Fn(&str) -> Option<(String, Vec<u8>)> + Send + 'static) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&req).lines().next().unwrap_or_default().to_owned();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, kind, body) = match answer(&path) {
                    Some((kind, body)) => ("200 OK", kind, body),
                    None => ("404 Not Found", "text/plain".to_owned(), b"no".to_vec()),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        at
    }

    /// The upgrader's way through a release, against a server of the
    /// test's own as GitHub: the newest found with its commit's time,
    /// offered over an older install, its files fetched and checked,
    /// only the programs that differ put in place, a retired one taken
    /// away, what was installed recorded; then not offered again, and a
    /// part already installed (the wallpapers) not fetched again.
    #[test]
    fn a_release_found_and_installed_as_an_upgrade() {
        let dir = PathBuf::from(format!("/var/tmp/olo-release-upgrade-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |root: &Path, files: &[(&str, &str)]| {
            for (p, c) in files {
                std::fs::create_dir_all(root.join(p).parent().unwrap()).unwrap();
                std::fs::write(root.join(p), c).unwrap();
            }
        };
        let main = format!("olo-2026.11.17-abcdef12-{}.tar.zst", arch());
        let wallpapers = "olo-wallpapers-2026.11.17-abcdef12.tar.zst".to_owned();
        write(
            &dir.join("src/olo"),
            &[
                ("bin/olo-session", "session, new"),
                ("bin/olo-deskbar", "deskbar, the same"),
                ("bin/olo-op", "op"),
                ("bin/pam_olo_keyring.so", "pam"),
                ("bin/README.txt", "not a program"),
                ("share/olo/icons/32/doctor.png", "icon"),
                ("VERSION", "2026.11.17-abcdef12\n"),
            ],
        );
        write(&dir.join("part/olo"), &[("share/olo/wallpapers/4k/dunes.webp", "dunes")]);
        let pack = |name: &str, from: &Path| {
            let archive = dir.join("dl").join(name);
            std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
            let out = zstd::stream::write::Encoder::new(std::fs::File::create(&archive).unwrap(), 3).unwrap().auto_finish();
            let mut t = tar::Builder::new(out);
            t.append_dir_all("olo", from).unwrap();
            t.finish().unwrap();
            archive
        };
        let (a, w) = (pack(&main, &dir.join("src/olo")), pack(&wallpapers, &dir.join("part/olo")));
        let sums = format!("{}  {main}\n{}  {wallpapers}\n", sha256_of(&a).unwrap(), sha256_of(&w).unwrap());
        std::fs::write(dir.join("dl").join(format!("{main}.sha256")), &sums).unwrap();
        // GitHub, as far as the upgrader asks it: the release, its
        // commit, its files. The commit an hour from now: newer than
        // anything installed here.
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let soon = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600;
        let (y, m, d, hh, mm, ss) = civil(soon);
        let when = format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z");
        let served = dir.join("dl");
        let (main_s, sums_name) = (main.clone(), format!("{main}.sha256"));
        let at = std::sync::Arc::new(std::sync::OnceLock::<String>::new());
        let at_inside = at.clone();
        let api = serve(move |path| {
            let base = at_inside.get().cloned().unwrap_or_default();
            if path == "/releases/latest" {
                let asset = |n: &str| serde_json::json!({ "name": n, "size": std::fs::metadata(served.join(n)).map(|m| m.len()).unwrap_or(0), "browser_download_url": format!("{base}/dl/{n}") });
                let v = serde_json::json!({
                    "tag_name": "v2026.11.17", "name": "Olo 2026.11.17", "target_commitish": commit, "published_at": "2026-11-17T20:13:39Z",
                    "assets": [asset(&main_s), asset(&sums_name), asset("olo-wallpapers-2026.11.17-abcdef12.tar.zst")],
                });
                return Some(("application/json".into(), v.to_string().into_bytes()));
            }
            if path == format!("/commits/{commit}") {
                return Some(("application/json".into(), serde_json::json!({ "commit": { "committer": { "date": when } } }).to_string().into_bytes()));
            }
            let name = path.strip_prefix("/dl/")?;
            Some(("application/octet-stream".into(), std::fs::read(served.join(name)).ok()?))
        });
        at.set(api.clone()).unwrap();
        // Installed: an older Olo, one program Olo no longer has.
        let prefix = dir.join("prefix");
        write(&prefix, &[("bin/olo-session", "session, old"), ("bin/olo-deskbar", "deskbar, the same"), ("bin/olo-smart-tile", "retired"), ("share/olo/icons/32/doctor.png", "old icon")]);
        let r = latest_from(&api).unwrap();
        assert_eq!((r.version.as_str(), r.tag.as_str(), r.title.as_str()), ("2026.11.17-abcdef12", "v2026.11.17", "Olo 2026.11.17"));
        assert_eq!((r.published, r.committed), (parse_time("2026-11-17T20:13:39Z").unwrap(), Some(soon)));
        let installed = installed_in(&prefix);
        assert!(installed.version.is_none() && installed.at.is_some());
        assert!(is_newer(&r, &installed), "made after the programs here were put here");
        let (a_size, w_size) = (std::fs::metadata(&a).unwrap().len(), std::fs::metadata(&w).unwrap().len());
        assert_eq!(r.download_bytes(&installed), a_size + w_size);
        // Fetched, unpacked, put in place.
        let stop = AtomicBool::new(false);
        let archives = fetch_into(&r.source, &installed.sums, &dir.join("cache"), &stop, &mut |_, _, _, _| {}).unwrap();
        assert_eq!(archives.len(), 2);
        let olo = unpack(&archives, &dir.join("stage"), &stop, Box::new(|_, _| {})).unwrap();
        let changed = place_prefix(&olo, &prefix, &["olo-smart-tile".to_owned()], Some(&r), &mut |_, _| {}).unwrap();
        assert_eq!(changed, ["olo-op", "olo-session"], "the one the same isn't put again");
        assert_eq!(std::fs::read_to_string(prefix.join("bin/olo-session")).unwrap(), "session, new");
        assert!(!prefix.join("bin/olo-smart-tile").exists() && !prefix.join("bin/README.txt").exists() && !prefix.join("bin/pam_olo_keyring.so").exists());
        assert_eq!(std::fs::read_to_string(prefix.join("lib/olo/pam_olo_keyring.so")).unwrap(), "pam");
        assert_eq!(std::fs::read_to_string(prefix.join("share/olo/icons/32/doctor.png")).unwrap(), "icon");
        assert_eq!(std::fs::read_to_string(prefix.join("share/olo/wallpapers/4k/dunes.webp")).unwrap(), "dunes");
        assert_eq!(std::fs::read_link(prefix.join("bin/op")).unwrap(), Path::new("olo-op"));
        // Recorded: not offered again; the wallpapers not fetched again.
        let now = installed_in(&prefix);
        assert_eq!((now.version.as_deref(), now.committed), (Some("2026.11.17-abcdef12"), Some(soon)));
        assert_eq!(now.sums.len(), 2);
        assert!(!is_newer(&r, &now));
        assert_eq!(r.download_bytes(&now), a_size);
        let again = fetch_into(&r.source, &now.sums, &dir.join("cache"), &stop, &mut |_, _, _, _| {}).unwrap();
        assert_eq!(again.len(), 1, "the archive alone");
        // A release made before what's installed isn't newer.
        let older = Release { version: "2026.11.16-00000000".into(), committed: Some(soon - 7200), ..r.clone() };
        assert!(!is_newer(&older, &now));
        assert!(!is_newer(&Release { committed: Some(1), ..r.clone() }, &Installed { at: Some(1000), ..Default::default() }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Seconds since the epoch as a date and time (the test's own way
    /// back from `parse_time`).
    fn civil(t: u64) -> (i64, i64, i64, i64, i64, i64) {
        let (days, secs) = ((t / 86400) as i64, (t % 86400) as i64);
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
        (y, m, d, secs / 3600, secs % 3600 / 60, secs % 60)
    }

    #[test]
    fn times_as_github_writes_them() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-17T20:13:39Z"), Some(1_794_946_419));
        assert_eq!(parse_time("2000-02-29T12:00:00Z"), Some(951_825_600));
        assert_eq!(parse_time("2026-11-17"), None);
        assert_eq!(version_in(&format!("olo-2026.11.17-4e8d72a5-{}.tar.zst", arch())), "2026.11.17-4e8d72a5");
    }

    /// An archive as tools/release.sh makes one, unpacked and put in place
    /// in a home folder, twice (a second run over the first).
    #[test]
    fn unpacked_and_placed() {
        let dir = PathBuf::from(format!("/var/tmp/olo-release-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src/olo");
        for (p, c) in [
            ("bin/olo-session", "s"),
            ("bin/olo-op", "o"),
            ("bin/pam_olo_keyring.so", "p"),
            ("bin/libpam_olo_face.so", "f"),
            ("share/olo/icons/32/doctor.png", "i"),
            ("system/session/olo.desktop", "d"),
            ("VERSION", "2026.11.07-abc\n"),
        ] {
            std::fs::create_dir_all(src.join(p).parent().unwrap()).unwrap();
            std::fs::write(src.join(p), c).unwrap();
        }
        // (Its wallpapers, a part of their own.)
        let part = dir.join("part/olo");
        std::fs::create_dir_all(part.join("share/olo/wallpapers/4k")).unwrap();
        std::fs::write(part.join("share/olo/wallpapers/4k/dunes.webp"), "w").unwrap();
        let pack = |name: &str, from: &Path| {
            let archive = dir.join(name);
            let out = zstd::stream::write::Encoder::new(std::fs::File::create(&archive).unwrap(), 3).unwrap().auto_finish();
            let mut t = tar::Builder::new(out);
            t.append_dir_all("olo", from).unwrap();
            t.finish().unwrap();
            archive
        };
        let (archive, wallpapers) = (pack("olo-test-x86_64.tar.zst", &src), pack("olo-wallpapers-test.tar.zst", &part));
        // Found as a release's files are: the archive given, its parts
        // named by its checksum file, each checked.
        let sum = |p: &Path| sha256_of(p).unwrap();
        std::fs::write(dir.join("olo-test-x86_64.tar.zst.sha256"), format!("{}  olo-test-x86_64.tar.zst\n{}  olo-wallpapers-test.tar.zst\n", sum(&archive), sum(&wallpapers))).unwrap();
        let stop = AtomicBool::new(false);
        let source = find(Some(archive.to_str().unwrap())).unwrap();
        // (Files on disk aren't downloaded: no progress is told.)
        let mut seen: Vec<(usize, usize)> = Vec::new();
        let archives = fetch(&source, &[], &stop, &mut |i, n, _, _| seen.push((i, n))).unwrap();
        assert_eq!(archives, vec![archive.clone(), wallpapers.clone()]);
        let home = dir.join("home");
        assert_eq!(seen, vec![]);
        for _ in 0..2 {
            let last = std::sync::Arc::new(std::sync::Mutex::new((0u64, 0u64)));
            let told = last.clone();
            let olo = unpack(&archives, &dir.join("stage"), &stop, Box::new(move |done, total| *told.lock().unwrap() = (done, total))).unwrap();
            let (done, total) = *last.lock().unwrap();
            assert_eq!(done, total, "all of both archives read");
            assert_eq!(version_of(&olo), "2026.11.07-abc");
            place(&olo, &home, &mut |_, _| {}).unwrap();
        }
        assert_eq!(std::fs::read_to_string(home.join(".local/share/olo/wallpapers/4k/dunes.webp")).unwrap(), "w");
        // A part that isn't what the checksum file says is refused.
        std::fs::write(&wallpapers, "other").unwrap();
        assert!(fetch(&source, &[], &stop, &mut |_, _, _, _| {}).is_err_and(|e| e.contains("olo-wallpapers-test.tar.zst isn't what was published")));
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/olo-session")).unwrap(), "s");
        assert!(!home.join(".local/bin/pam_olo_keyring.so").exists(), "the PAM module isn't a program");
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/libpam_olo_face.so")).unwrap(), "f");
        assert_eq!(std::fs::read_link(home.join(".local/bin/op")).unwrap(), Path::new("olo-op"));
        assert_eq!(installed(&home).as_deref(), Some("2026.11.07-abc"));
        assert!(home.join(".local/share/olo/icons/32/doctor.png").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
