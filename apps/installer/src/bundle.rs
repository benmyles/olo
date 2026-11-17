//! Olo itself, as the release has it (tools/release.sh): an archive,
//! `olo-VERSION-ARCH.tar.zst`, its SHA-256 beside it, and the parts
//! that file names besides (the wallpapers, an archive of their own
//! since 2026-11-17: with them Olo was more than the 2 GiB a file GitHub
//! takes). Found (the latest
//! GitHub release of the repository's, or one given), downloaded into
//! the cache (kept: a second run, or one after a failure, doesn't fetch
//! it again), checked, unpacked into a staging folder, then put in place
//! in the home folder as tools/install-session.sh does: each program
//! written beside its old self and renamed over it (a running one keeps
//! its file), the data copied over. Nothing is shelled out to.

use std::{
    io::{Read, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use sha2::Digest as _;

/// The repository whose releases hold Olo.
pub const REPO: &str = "benmyles/olo";

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
    ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(10)
        .user_agent("Olo-Setup")
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into()
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
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = olo_net::across_changes(Duration::from_secs(30), || agent().get(&url).header("Accept", "application/vnd.github+json").call()).map_err(|e| match e {
        ureq::Error::StatusCode(404) => "GitHub has no release of Olo yet".to_owned(),
        ureq::Error::StatusCode(code) => format!("GitHub answered {code} when asked for Olo's latest release"),
        e => format!("GitHub couldn't be reached: {e}"),
    })?;
    let v: serde_json::Value = body.into_body().read_json().map_err(|e| format!("GitHub's answer couldn't be read: {e}"))?;
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let ending = format!("-{}.tar.zst", arch());
    let archive =
        assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.starts_with("olo-") && n.ends_with(&ending))).ok_or_else(|| format!("Olo's latest release has nothing for {}", arch()))?;
    let name = archive["name"].as_str().unwrap_or_default().to_owned();
    let sums = assets.iter().find(|a| a["name"].as_str() == Some(&format!("{name}.sha256"))).ok_or("Olo's latest release has no checksum for its archive")?;
    Ok(Source {
        archive: Origin::Url(archive["browser_download_url"].as_str().unwrap_or_default().to_owned()),
        sums: Origin::Url(sums["browser_download_url"].as_str().unwrap_or_default().to_owned()),
        name,
    })
}

/// A `.sha256`'s lines: each SHA-256 and the file it is of.
fn sums_of(sums: &str) -> Vec<(String, &str)> {
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
fn parts(sums: &str, name: &str) -> Vec<(String, String)> {
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

fn sha256_of(path: &Path) -> std::io::Result<String> {
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
/// each downloaded into the cache unless a checked copy is there.
/// `progress` gets (which file, of how many, bytes so far, of how many).
pub fn fetch(src: &Source, cancel: &AtomicBool, progress: &mut dyn FnMut(usize, usize, u64, u64)) -> Result<Vec<PathBuf>, String> {
    let sums = match &src.sums {
        Origin::Url(u) => olo_net::across_changes(Duration::from_secs(30), || agent().get(u).call())
            .map_err(|e| format!("the checksum couldn't be downloaded ({u}): {e}"))?
            .into_body()
            .read_to_string()
            .map_err(|e| e.to_string())?,
        Origin::File(p) => std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
    };
    let want = expected(&sums, &src.name).ok_or("the checksum file names no SHA-256")?;
    let mut files = vec![(src.name.clone(), src.archive.clone(), want)];
    files.extend(parts(&sums, &src.name).into_iter().map(|(name, want)| (name.clone(), src.archive.beside(&name), want)));
    let n = files.len();
    let mut here = Vec::new();
    for (i, (name, origin, want)) in files.iter().enumerate() {
        here.push(fetch_one(name, origin, want, cancel, &mut |got, total| progress(i + 1, n, got, total))?);
    }
    Ok(here)
}

/// One file of a release, `name` from `origin`, whose SHA-256 is `want`.
fn fetch_one(name: &str, origin: &Origin, want: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let path = match origin {
        Origin::File(p) => p.clone(),
        Origin::Url(url) => {
            let dir = cache_dir();
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
pub fn unpack(archives: &[PathBuf], into: &Path, cancel: &AtomicBool, mut progress: Box<dyn FnMut(u64, u64)>) -> Result<PathBuf, String> {
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
        let archives = fetch(&source, &stop, &mut |_, _, _, _| {}).unwrap();
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
            "share/olo/icons/manifest.txt",
            "share/olo/icons/16",
            "share/olo/brand",
            "share/olo/fonts",
            "share/olo/ocr/text-detection.onnx",
            "share/olo/face/sface.onnx",
            "share/olo/themes/platinum/icons/16",
            "share/olo/wallpapers/pack.txt",
            "share/olo/wallpapers/4k",
            "share/olo/wallpapers/tiles",
            "system/session/olo.desktop",
            "LICENSE",
        ] {
            assert!(olo.join(needed).exists(), "{needed} is missing");
        }
        for sources in ["share/olo/icons/src", "share/olo/wallpapers/src", "share/olo/icons/reference"] {
            assert!(!olo.join(sources).exists(), "{sources} isn't for a release");
        }
        std::fs::remove_dir_all(&stage).unwrap();
    }

    /// An archive as tools/release.sh makes one, unpacked and put in place
    /// in a home folder, twice (a second run over the first).
    #[test]
    fn unpacked_and_placed() {
        let dir = PathBuf::from(format!("/var/tmp/olo-installer-test-{}", std::process::id()));
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
        let archives = fetch(&source, &stop, &mut |i, n, _, _| seen.push((i, n))).unwrap();
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
        assert!(fetch(&source, &stop, &mut |_, _, _, _| {}).is_err_and(|e| e.contains("olo-wallpapers-test.tar.zst isn't what was published")));
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/olo-session")).unwrap(), "s");
        assert!(!home.join(".local/bin/pam_olo_keyring.so").exists(), "the PAM module isn't a program");
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/libpam_olo_face.so")).unwrap(), "f");
        assert_eq!(std::fs::read_link(home.join(".local/bin/op")).unwrap(), Path::new("olo-op"));
        assert_eq!(installed(&home).as_deref(), Some("2026.11.07-abc"));
        assert!(home.join(".local/share/olo/icons/32/doctor.png").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
