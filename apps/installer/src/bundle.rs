//! Olo itself, as the release has it (tools/release.sh): one archive,
//! `olo-VERSION-ARCH.tar.zst`, its SHA-256 beside it. Found (the latest
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

/// The SHA-256 a `.sha256` names for `name` (or its only one).
fn expected(sums: &str, name: &str) -> Option<String> {
    let lines: Vec<(&str, &str)> = sums.lines().filter_map(|l| l.split_once(char::is_whitespace)).map(|(h, n)| (h, n.trim().trim_start_matches('*'))).collect();
    lines.iter().find(|(_, n)| *n == name).or(lines.first()).map(|(h, _)| h.to_ascii_lowercase()).filter(|h| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()))
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

/// The archive, here and checked: downloaded into the cache unless a
/// checked copy is there. `progress` gets (bytes so far, of how many).
pub fn fetch(src: &Source, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let sums = match &src.sums {
        Origin::Url(u) => olo_net::across_changes(Duration::from_secs(30), || agent().get(u).call())
            .map_err(|e| format!("the checksum couldn't be downloaded ({u}): {e}"))?
            .into_body()
            .read_to_string()
            .map_err(|e| e.to_string())?,
        Origin::File(p) => std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
    };
    let want = expected(&sums, &src.name).ok_or("the checksum file names no SHA-256")?;
    let path = match &src.archive {
        Origin::File(p) => p.clone(),
        Origin::Url(url) => {
            let dir = cache_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = dir.join(&src.name);
            if path.exists() && sha256_of(&path).is_ok_and(|h| h == want) {
                return Ok(path);
            }
            let part = dir.join(format!("{}.part", src.name));
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
        if matches!(src.archive, Origin::Url(_)) {
            let _ = std::fs::remove_file(&path);
        }
        return Err(format!("the archive isn't what was published (its SHA-256 is {have}, not {want})"));
    }
    Ok(path)
}

/// Reads through, counting what's read (for the progress bar).
struct Counting<R> {
    inner: R,
    read: u64,
    total: u64,
    progress: Box<dyn FnMut(u64, u64)>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        (self.progress)(self.read, self.total);
        Ok(n)
    }
}

/// The archive unpacked into `into` (made empty first): `into/olo/…`.
pub fn unpack(archive: &Path, into: &Path, cancel: &AtomicBool, progress: Box<dyn FnMut(u64, u64)>) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let counting = Counting { inner: std::io::BufReader::new(file), read: 0, total, progress };
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
        if !name.starts_with("olo-") {
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
            ("share/olo/icons/32/doctor.png", "i"),
            ("system/session/olo.desktop", "d"),
            ("VERSION", "2026.11.07-abc\n"),
        ] {
            std::fs::create_dir_all(src.join(p).parent().unwrap()).unwrap();
            std::fs::write(src.join(p), c).unwrap();
        }
        let archive = dir.join("olo-test.tar.zst");
        {
            let out = zstd::stream::write::Encoder::new(std::fs::File::create(&archive).unwrap(), 3).unwrap().auto_finish();
            let mut t = tar::Builder::new(out);
            t.append_dir_all("olo", &src).unwrap();
            t.finish().unwrap();
        }
        let stop = AtomicBool::new(false);
        let home = dir.join("home");
        for _ in 0..2 {
            let olo = unpack(&archive, &dir.join("stage"), &stop, Box::new(|_, _| {})).unwrap();
            assert_eq!(version_of(&olo), "2026.11.07-abc");
            place(&olo, &home, &mut |_, _| {}).unwrap();
        }
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/olo-session")).unwrap(), "s");
        assert!(!home.join(".local/bin/pam_olo_keyring.so").exists(), "the PAM module isn't a program");
        assert_eq!(std::fs::read_link(home.join(".local/bin/op")).unwrap(), Path::new("olo-op"));
        assert_eq!(installed(&home).as_deref(), Some("2026.11.07-abc"));
        assert!(home.join(".local/share/olo/icons/32/doctor.png").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
