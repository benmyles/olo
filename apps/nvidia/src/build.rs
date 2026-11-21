//! Fetch once, verify on every build, and give make a fresh worktree.
//! The root process owns the cache and the installation. Only the
//! compiler's private copy is writable by its unprivileged user.
use crate::patches;
use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::{
    ffi::OsStrExt,
    fs::{PermissionsExt, chown},
    net::UnixStream,
    process::CommandExt,
};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::{NamedTempFile, TempDir};

struct Series {
    version: &'static str,
    commit: &'static str,
    source_sha256: &'static str,
}
// SHA-256 of the compressed bytes served by codeload for COMMIT on
// 2026-11-20. All 4,005 regular files compared byte for byte with the
// pinned pristine checkout. Git's own uncompressed archive is different.
const SERIES: &[Series] = &[Series { version: "615.71.09", commit: "61dcc93722ecb418bb5f2e00923f05b4b8051dd1", source_sha256: "7c6346ba45bef0ab2b7553735e93cfa758d74c55d94fc339139c9473457bffa3" }];
pub const VERSION: &str = SERIES[0].version;
pub const COMMIT: &str = SERIES[0].commit;
pub const SOURCE_SHA: &str = SERIES[0].source_sha256;
const ARCHIVE_LIMIT: u64 = 64 << 20;

fn verify(bytes: Vec<u8>) -> Result<Vec<u8>> {
    ensure!(hex::encode(Sha256::digest(&bytes)) == SOURCE_SHA, "NVIDIA's source archive differs from the pinned SHA-256 for {VERSION}");
    Ok(bytes)
}

pub fn source(state: &Path, fetch: bool) -> Result<Vec<u8>> {
    let dir = state.join("source");
    let file = dir.join(format!("{COMMIT}.tar.gz"));
    match File::open(&file) {
        Ok(f) => {
            let mut bytes = Vec::new();
            f.take(ARCHIVE_LIMIT + 1).read_to_end(&mut bytes)?;
            return verify(bytes);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    }
    ensure!(fetch, "no verified source is cached for {VERSION}; run olo-nvidia build to fetch it");
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder().tls_config(tls).max_redirects(0).timeout_global(Some(Duration::from_secs(180))).build().into();
    let url = format!("https://codeload.github.com/NVIDIA/open-gpu-kernel-modules/tar.gz/{COMMIT}");
    let mut response = agent.get(url.as_str()).header("Accept-Encoding", "identity").call().context("fetching NVIDIA's pinned release")?;
    let bytes = verify(response.body_mut().with_config().limit(ARCHIVE_LIMIT).read_to_vec()?)?;
    fs::create_dir_all(&dir)?;
    let mut tmp = NamedTempFile::new_in(&dir)?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&file)?;
    File::open(dir)?.sync_all()?;
    Ok(bytes)
}

fn extract(bytes: &[u8], dest: &Path) -> Result<()> {
    let top = format!("open-gpu-kernel-modules-{COMMIT}");
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_pax_global_extensions() {
            // git archive writes the commit ID before the top folder.
            let mut header = String::new();
            entry.read_to_string(&mut header)?;
            ensure!(header == format!("52 comment={COMMIT}\n"), "source archive names a different commit");
            continue;
        }
        let path = entry.path()?;
        let child = path.strip_prefix(&top).context("unrecognized source archive prefix")?;
        ensure!(child.components().all(|c| matches!(c, Component::Normal(_))), "unsafe name in source archive");
        let target = dest.join(child);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(target)?;
        } else {
            ensure!(entry.header().entry_type().is_file(), "unexpected link or special file in source archive");
            let mode = 0o644 | (entry.header().mode()? & 0o111);
            let mut f = File::create_new(target)?;
            io::copy(&mut entry, &mut f)?;
            f.set_permissions(Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

pub struct Work {
    // The parent remains private to root, including throughout the build.
    pub private: TempDir,
    parent: PathBuf,
}

impl Work {
    pub fn new(parent: &Path) -> Result<Self> {
        fs::create_dir_all(parent)?;
        Ok(Self { private: tempfile::Builder::new().permissions(Permissions::from_mode(0o700)).prefix(".work-").tempdir_in(parent)?, parent: parent.into() })
    }
    pub fn tree(&self) -> PathBuf {
        self.private.path().join("tree")
    }

    pub fn from_archive(parent: &Path, bytes: &[u8]) -> Result<Self> {
        let work = Self::new(parent)?;
        extract(bytes, &work.tree())?;
        patches::prepare(&work.tree())?;
        Ok(work)
    }

    pub fn compile(&self, headers: &Path, release: &str) -> Result<()> {
        let tree = self.tree();
        fs::create_dir(tree.join(".compiler-tmp"))?;
        let root = unsafe { libc::geteuid() } == 0;
        // nobody is the shared overflow identity on Linux. Another
        // process with that uid must not be able to edit a Makefile or
        // a .ko in the tree. Make it reachable only inside the child's
        // private mount namespace. The host mountpoint stays empty.
        let mountpoint = tempfile::Builder::new().prefix(".compile-").tempdir_in(&self.parent)?;
        let cwd = if root {
            give_tree(&tree)?;
            mountpoint.path()
        } else {
            tree.as_path()
        };
        let src = CString::new(tree.as_os_str().as_bytes())?;
        let dst = CString::new(cwd.as_os_str().as_bytes())?;
        let log = self.private.path().join("make.log");
        let out = File::create_new(&log)?;
        let jobs = std::thread::available_parallelism()?.get().min(16);
        let mut make = Command::new("/usr/bin/make");
        make.env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C")
            .env("TMPDIR", cwd.join(".compiler-tmp"))
            .arg("-O")
            .arg(format!("-j{jobs}"))
            .args(["V=1", "VERBOSE=1"])
            .arg(format!("KERNEL_UNAME={release}"))
            .arg(format!("SYSSRC={}", headers.display()))
            .args(["IGNORE_CC_MISMATCH=1", "IGNORE_XEN_PRESENCE=1", "IGNORE_PREEMPT_RT_PRESENCE=1", "modules"])
            .stdin(Stdio::null())
            .stderr(out.try_clone()?)
            .stdout(out);
        // The parent is privileged in the original user namespace; it
        // writes the ID maps after the child has entered its own. The
        // child never gets an opportunity to run make as root.
        let (mut child_socket, mapper) = if root {
            let (parent, child) = UnixStream::pair()?;
            child.set_read_timeout(Some(Duration::from_secs(30)))?;
            parent.set_read_timeout(Some(Duration::from_secs(30)))?;
            (Some(child), Some(std::thread::spawn(move || map_compiler(parent))))
        } else {
            (None, None)
        };
        // Only Linux syscalls between fork and exec. All strings are
        // allocated above. A failed isolation or priority step aborts.
        unsafe {
            make.pre_exec(move || {
                let checked = |r: i32| if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) };
                if root {
                    checked(libc::unshare(libc::CLONE_NEWNS))?;
                    checked(libc::mount(std::ptr::null(), c"/".as_ptr(), std::ptr::null(), libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()))?;
                    checked(libc::mount(src.as_ptr(), dst.as_ptr(), std::ptr::null(), libc::MS_BIND, std::ptr::null()))?;
                    checked(libc::setgroups(0, std::ptr::null()))?;
                    // A different nobody process on the host must also be unable
                    // to reach the private mount through /proc/PID/root or ptrace.
                    // Root owns this user namespace; nobody in the parent has
                    // no tracing capability here. Preserve identities across
                    // the boundary, then permanently drop root before exec.
                    checked(libc::unshare(libc::CLONE_NEWUSER))?;
                    let socket = child_socket.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
                    socket.write_all(&libc::getpid().to_ne_bytes())?;
                    let mut ack = [0];
                    socket.read_exact(&mut ack)?;
                    if ack != [1] {
                        return Err(io::ErrorKind::PermissionDenied.into());
                    }
                    checked(libc::setresgid(65534, 65534, 65534))?;
                    checked(libc::setresuid(65534, 65534, 65534))?;
                }
                // Keep the exec-error pipe usable until exec; close every
                // non-standard descriptor in the compiler image.
                checked(libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) as i32)?;
                checked(libc::chdir(dst.as_ptr()))?;
                checked(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0))?;
                checked(libc::setpriority(libc::PRIO_PROCESS, 0, 19))?;
                checked(libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13) as i32)?;
                Ok(())
            });
        }
        eprintln!("Building five NVIDIA modules for {release} with {jobs} jobs.");
        let started = make.spawn();
        drop(make); // close the parent's copy of the child's socket
        if let Some(thread) = mapper {
            thread.join().map_err(|_| anyhow::anyhow!("compiler identity setup stopped"))??;
        }
        let result = started.context("starting make with unprivileged build settings")?.wait()?;
        if !result.success() {
            let details = fs::read_to_string(log)?;
            let errors: Vec<_> = details.lines().filter(|l| l.contains("error:") || l.contains("Error ") || l.contains("Permission denied")).take(6).collect();
            bail!("make failed ({result}): {}", errors.join(" | "));
        }
        Ok(())
    }
}

fn give_tree(path: &Path) -> Result<()> {
    // Only the just-extracted, hash-checked files, behind the private
    // ancestor. The compiler has not been started yet.
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            give_tree(&entry?.path())?;
        }
    }
    chown(path, Some(65534), Some(65534))?;
    Ok(())
}

/// A private socket created before fork. Only that child can send its
/// PID, and it waits for the reply before executing any build input.
fn map_compiler(mut socket: UnixStream) -> Result<()> {
    let mut pid = [0; 4];
    match socket.read_exact(&mut pid) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()), // spawn failed earlier
        Err(e) => return Err(e.into()),
    }
    let proc = Path::new("/proc").join(i32::from_ne_bytes(pid).to_string());
    let result = (|| -> Result<()> {
        for name in ["uid_map", "gid_map"] {
            fs::write(proc.join(name), b"0 0 1\n65534 65534 1\n").with_context(|| format!("setting the compiler's {name}"))?;
        }
        Ok(())
    })();
    socket.write_all(&[u8::from(result.is_ok())])?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_git_archive_with_its_global_commit_header() {
        let tmp = crate::real_build::scratch();
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gzip);
        let top = format!("open-gpu-kernel-modules-{COMMIT}");
        for (path, kind, contents) in [
            ("pax_global_header".into(), tar::EntryType::XGlobalHeader, format!("52 comment={COMMIT}\n")),
            (top.clone(), tar::EntryType::Directory, String::new()),
            (format!("{top}/Makefile"), tar::EntryType::Regular, "modules:\n".into()),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(kind);
            h.set_size(contents.len() as u64);
            h.set_mode(0o644);
            tar.append_data(&mut h, path, contents.as_bytes()).unwrap();
        }
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        extract(&bytes, tmp.path()).unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join("Makefile")).unwrap(), "modules:\n");
    }
}
