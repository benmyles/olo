//! The system tree can be an ordinary user's scratch folder. Release
//! names from the command line also become file names and make values.
use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub struct System {
    pub root: PathBuf,
}
impl System {
    pub fn here() -> Result<Self> {
        let root = std::env::var_os("OLO_NVIDIA_ROOT").map(PathBuf::from).unwrap_or_else(|| "/".into()).canonicalize()?;
        Ok(Self { root })
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("var/lib/olo/nvidia")
    }
    pub fn kernel(&self, release: &str) -> PathBuf {
        self.root.join("lib/modules").join(release)
    }
    pub fn running(&self) -> Result<String> {
        let r = if self.root == Path::new("/") { olo_kmod::release() } else { fs::read_to_string(self.root.join("proc/sys/kernel/osrelease"))? };
        valid_release(r.trim())?;
        Ok(r.trim().to_owned())
    }
    pub fn headers(&self, r: &str) -> Result<PathBuf> {
        for path in [self.root.join("usr/src/kernels").join(r), self.kernel(r).join("build")] {
            if path.join("Makefile").is_file() {
                let path = path.canonicalize()?;
                ensure!(path.starts_with(&self.root), "kernel headers lie outside the selected root");
                ensure!(fs::read_to_string(path.join("include/config/kernel.release")).is_ok_and(|s| s.trim() == r), "the headers in {} do not identify kernel {r}", path.display());
                return Ok(path);
            }
        }
        anyhow::bail!("install the kernel-devel headers for {r}")
    }
    pub fn releases(&self) -> Result<Vec<String>> {
        let mut found = Vec::new();
        for entry in fs::read_dir(self.root.join("lib/modules")).context("listing installed kernels")? {
            let e = entry?;
            let name = e.file_name().to_string_lossy().into_owned();
            if valid_release(&name).is_ok() && e.path().is_dir() {
                found.push(name)
            }
        }
        found.sort_by(|a, b| version_order(a, b));
        Ok(found)
    }
    pub fn manifest(&self, r: &str) -> PathBuf {
        self.state().join(format!("{r}.json"))
    }
    pub fn outcome(&self, r: &str) -> PathBuf {
        self.state().join("outcomes").join(format!("{r}.json"))
    }
    pub fn opted_in(&self) -> Result<bool> {
        match fs::symlink_metadata(self.state().join("prevention.enabled")) {
            Ok(m) => {
                ensure!(m.is_file(), "the NVIDIA opt-in marker is not a regular file");
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("reading the NVIDIA opt-in marker"),
        }
    }
    /// RPM Fusion installs the stock set through an RPM transaction.
    /// Share RPM's fcntl lock across the short index/rename operation,
    /// never across the fetch or compilation. A watcher woken midway
    /// through an RPM transaction sees its completed module set.
    pub fn packages(&self) -> Result<Option<File>> {
        use std::os::fd::AsRawFd;
        let db = self.root.join("usr/lib/sysimage/rpm");
        if !db.try_exists()? {
            return Ok(None);
        }
        let f = File::open(db.join(".rpm.lock")).context("opening Fedora's RPM transaction lock")?;
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_RDLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        loop {
            if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_OFD_SETLKW, &lock) } == 0 {
                return Ok(Some(f));
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
    }
    /// A scratch tree must not redirect an installation to the host.
    /// Fedora's /lib -> usr/lib is the one alias in these write paths.
    /// Where privileged, the tree must belong to the administrator.
    pub fn directory(&self, path: &Path) -> Result<()> {
        let relative = path.strip_prefix(&self.root)?;
        let root_user = unsafe { libc::geteuid() } == 0;
        let mut p = self.root.clone();
        for component in std::iter::once(None).chain(relative.components().map(Some)) {
            if let Some(c) = component {
                p.push(c);
            }
            let meta = match fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
            };
            if meta.is_symlink() {
                ensure!(p == self.root.join("lib") && p.canonicalize()? == self.root.join("usr/lib"), "refusing redirected module or state folder {}", p.display());
                self.directory(&self.root.join("usr/lib"))?;
            } else {
                ensure!(meta.is_dir(), "{} is not a folder", p.display());
                ensure!(!root_user || (meta.uid() == 0 && meta.mode() & 0o022 == 0), "{} must be owned by root and writable only by root for this operation", p.display());
            }
        }
        Ok(())
    }
    pub fn destinations(&self, r: &str) -> Result<()> {
        self.directory(&self.kernel(r).join("updates/olo"))?;
        for dir in ["source", "build", "outcomes", "checks"] {
            self.directory(&self.state().join(dir))?;
        }
        self.directory(&self.state().join("build").join(r))
    }
    pub fn lock(&self) -> Result<File> {
        ensure!(self.root != Path::new("/") || unsafe { libc::geteuid() } == 0, "run this command as root to change installed modules");
        self.directory(&self.state())?;
        fs::create_dir_all(self.state())?;
        let lock = File::options().read(true).write(true).custom_flags(libc::O_NOFOLLOW).create(true).truncate(false).open(self.state().join("lock"))?;
        lock.lock().context("locking NVIDIA module changes")?;
        Ok(lock)
    }
}

pub fn valid_release(s: &str) -> Result<()> {
    ensure!(s.as_bytes().first().is_some_and(u8::is_ascii_digit) && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".+~-_".contains(&b)), "invalid kernel release {s:?}");
    Ok(())
}

/// Compare numeric components by value (7.10 follows 7.9).
fn version_order(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while let (Some(&x), Some(&y)) = (a.first(), b.first()) {
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let i = a.iter().take_while(|v| v.is_ascii_digit()).count();
            let j = b.iter().take_while(|v| v.is_ascii_digit()).count();
            let trim = |s: &[u8]| s.iter().take_while(|v| **v == b'0').count();
            let (x, y) = (&a[trim(&a[..i])..i], &b[trim(&b[..j])..j]);
            let c = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if c != Ordering::Equal {
                return c;
            }
            (a, b) = (&a[i..], &b[j..]);
        } else {
            if x != y {
                return x.cmp(&y);
            }
            (a, b) = (&a[1..], &b[1..]);
        }
    }
    a.len().cmp(&b.len())
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}
pub fn json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_reader(File::open(path)?)?)
}

pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("file has no parent")?;
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.as_file().set_permissions(fs::Permissions::from_mode(0o644))?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic(path, &serde_json::to_vec(value)?)
}
pub fn remove_file(p: &Path) -> Result<()> {
    match fs::remove_file(p) {
        Ok(()) => {
            File::open(p.parent().unwrap())?.sync_all()?;
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct Manifest {
    pub kernel: String,
    pub version: String,
    pub set: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub patches: BTreeMap<String, String>,
    pub modules: crate::modules::Set,
    pub built_at: u64,
}
#[derive(serde::Deserialize, Serialize)]
pub struct Outcome {
    pub ok: bool,
    pub at: u64,
    pub message: String,
}
