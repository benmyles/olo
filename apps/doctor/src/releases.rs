//! A program from its maker's latest GitHub release, installed natively
//! (Olo Suggestions' mise and worktrunk, which Fedora doesn't carry): the
//! release asked of GitHub's API, the asset and its published SHA-256
//! downloaded, checked, the program taken out of a `.tar.xz` or `.tar.gz`
//! (or the asset itself, a bare binary), and put in place whole (beside
//! it, then renamed). Nothing is shelled out to. On a worker (it waits on
//! the network), telling how far it has got.

use std::{io::Read as _, path::Path, time::Duration};

use sha2::Digest as _;

/// Where a program's releases are, and how its asset is named.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Release {
    /// "owner/name" on GitHub.
    pub repo: String,
    /// The asset's name, `{tag}` the release's tag ("mise-{tag}-linux-x64").
    pub asset: String,
    /// The file with its SHA-256: a list of "HASH  name" lines
    /// ("SHASUMS256.txt") or the hash alone (`{asset}.sha256`), `{asset}`
    /// and `{tag}` filled in.
    pub sums: String,
    /// The program's name in the archive (None: the asset is the program).
    pub member: Option<String>,
}

fn agent() -> ureq::Agent {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder().tls_config(tls).max_redirects(10).user_agent("Olo").timeout_connect(Some(Duration::from_secs(30))).timeout_per_read(Some(Duration::from_secs(60))).build().into()
}

fn fetch(agent: &ureq::Agent, url: &str, accept: &str, limit: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    let host = url.split('/').nth(2).unwrap_or(url);
    let response = agent.get(url).header("Accept", accept).call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("{host} answered {code} for {url}"),
        e => format!("{host} couldn't be reached: {e}"),
    })?;
    let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
    let mut body = response.into_body();
    let mut reader = body.as_reader();
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("The download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() as u64 > limit {
            return Err(format!("{url} is bigger than a program should be"));
        }
        progress(out.len() as u64, total);
    }
    Ok(out)
}

/// A file at `url` (a repository's release package), at most `limit`
/// bytes.
pub fn download(url: &str, limit: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    fetch(&agent(), url, "*/*", limit, progress)
}

/// The latest release's tag.
fn latest_tag(agent: &ureq::Agent, repo: &str) -> Result<String, String> {
    let text = fetch(agent, &format!("https://api.github.com/repos/{repo}/releases/latest"), "application/vnd.github+json", 1 << 20, &mut |_, _| {})?;
    let v: serde_json::Value = serde_json::from_slice(&text).map_err(|_| "GitHub's answer couldn't be read".to_string())?;
    v.get("tag_name").and_then(|t| t.as_str()).map(str::to_owned).ok_or_else(|| format!("{repo} has no release"))
}

/// `asset`'s SHA-256 in a sums file: its line, or the file's only hash.
pub fn hash_of(sums: &str, asset: &str) -> Option<String> {
    let lines: Vec<&str> = sums.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let named = lines.iter().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset || name.ends_with(&format!("/{asset}"))).then(|| hash.to_owned())
    });
    let alone = || (lines.len() == 1).then(|| lines[0].split_whitespace().next().map(str::to_owned)).flatten();
    named.or_else(alone).map(|h| h.to_lowercase()).filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The file named `member` (at any depth) of a tar archive.
fn from_tar(reader: impl std::io::Read, member: &str) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if entry.header().entry_type().is_file() && path.file_name().is_some_and(|n| n == member) {
            let mut out = Vec::new();
            entry.read_to_end(&mut out).map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err(format!("the release holds no {member}"))
}

/// The program in `bytes` (the asset as downloaded).
pub fn program_of(bytes: &[u8], asset: &str, member: Option<&str>) -> Result<Vec<u8>, String> {
    match member {
        None => Ok(bytes.to_vec()),
        Some(m) if asset.ends_with(".tar.xz") => from_tar(liblzma::read::XzDecoder::new(bytes), m),
        Some(m) if asset.ends_with(".tar.gz") || asset.ends_with(".tgz") => from_tar(flate2::read::GzDecoder::new(bytes), m),
        Some(_) => Err(format!("{asset} isn't an archive Olo opens")),
    }
}

/// Install the latest release's program to `to` (made executable),
/// `progress` told the download's bytes so far and in all. Returns the
/// release's tag.
pub fn install(r: &Release, to: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<String, String> {
    let agent = agent();
    let tag = latest_tag(&agent, &r.repo)?;
    let asset = r.asset.replace("{tag}", &tag);
    let base = format!("https://github.com/{}/releases/download/{tag}", r.repo);
    let sums_name = r.sums.replace("{asset}", &asset).replace("{tag}", &tag);
    let sums = fetch(&agent, &format!("{base}/{sums_name}"), "application/octet-stream", 1 << 20, &mut |_, _| {})?;
    let want = hash_of(&String::from_utf8_lossy(&sums), &asset).ok_or_else(|| format!("{sums_name} says nothing of {asset}"))?;
    let bytes = fetch(&agent, &format!("{base}/{asset}"), "application/octet-stream", 512 << 20, progress)?;
    let got: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err(format!("{asset} isn't what its maker published (its SHA-256 doesn't match)"));
    }
    let program = program_of(&bytes, &asset, r.member.as_deref())?;
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let partial = to.with_extension("partial");
    use std::os::unix::fs::OpenOptionsExt as _;
    let write = || -> std::io::Result<()> {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o755).open(&partial)?;
        f.write_all(&program)?;
        f.sync_all()?;
        std::fs::rename(&partial, to)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("{}: {e}", to.display())
    })?;
    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_from_either_kind_of_sums_file() {
        let list = "aaaa  other\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF  mise-v1-linux-x64\n";
        assert_eq!(hash_of(list, "mise-v1-linux-x64").as_deref(), Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
        let alone = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *worktrunk-x86_64-unknown-linux-musl.tar.xz\n";
        assert!(hash_of(alone, "worktrunk-x86_64-unknown-linux-musl.tar.xz").is_some());
        assert_eq!(hash_of("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210\n", "x").as_deref(), Some("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"));
        assert_eq!(hash_of(list, "missing"), None);
        assert_eq!(hash_of("nothex  x\n", "x"), None);
    }

    #[test]
    fn the_program_out_of_a_tar_xz() {
        let mut tar = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(5);
        h.set_mode(0o755);
        h.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut h, "worktrunk-x86_64/wt", &b"#!wt\n"[..]).unwrap();
        let plain = tar.into_inner().unwrap();
        let mut xz = liblzma::write::XzEncoder::new(Vec::new(), 6);
        std::io::Write::write_all(&mut xz, &plain).unwrap();
        let packed = xz.finish().unwrap();
        assert_eq!(program_of(&packed, "w.tar.xz", Some("wt")).unwrap(), b"#!wt\n");
        assert!(program_of(&packed, "w.tar.xz", Some("nope")).is_err());
        assert_eq!(program_of(b"ELF", "mise-v1-linux-x64", None).unwrap(), b"ELF");
    }

    /// The real thing, from GitHub (`--ignored`): worktrunk's latest
    /// release (a `.tar.xz` beside its own `.sha256`) and mise's (a bare
    /// binary in a SHASUMS256.txt) into a scratch folder, checked, each an
    /// executable program.
    #[test]
    #[ignore]
    fn installs_from_github() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::path::PathBuf::from("/var/tmp").join(format!("olo-release-{}", std::process::id()));
        let wt = Release { repo: "max-sixty/worktrunk".into(), asset: "worktrunk-x86_64-unknown-linux-musl.tar.xz".into(), sums: "{asset}.sha256".into(), member: Some("wt".into()) };
        let mise = Release { repo: "jdx/mise".into(), asset: "mise-{tag}-linux-x64".into(), sums: "SHASUMS256.txt".into(), member: None };
        for (r, name) in [(wt, "wt"), (mise, "mise")] {
            let to = dir.join(name);
            let mut last = (0, 0);
            let tag = install(&r, &to, &mut |got, of| last = (got, of)).unwrap();
            assert!(tag.starts_with('v') || tag.chars().next().is_some_and(|c| c.is_ascii_digit()), "{tag}");
            let bytes = std::fs::read(&to).unwrap();
            assert!(bytes.len() > 1_000_000 && bytes.starts_with(b"\x7fELF"), "{name}");
            assert_eq!(std::fs::metadata(&to).unwrap().permissions().mode() & 0o777, 0o755);
            assert!(last.0 > 0 && (last.1 == 0 || last.0 == last.1), "{name}: {last:?}");
            assert!(!to.with_extension("partial").exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
