//! depmod chooses modules using the distributor's rules and writes
//! several binary indexes as well as modules.dep. Prepare a complete
//! stock index *while the override is still reachable*: even a failed
//! scan or an interrupted withdrawal must leave resolvable file paths.
use crate::{modules, system::System};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, c_char, c_void},
    fs::{self, File},
    io::{self, Read, Seek},
    os::fd::AsRawFd,
    os::unix::ffi::OsStrExt,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    process::Command,
};
use tempfile::TempDir;

pub trait Indexer {
    fn generate(&self, system: &System, release: &str, include_olo: bool, output: &Path) -> Result<()>;
}
pub struct Depmod;

fn links(from: &Path, to: &Path, include: impl Fn(&OsStr) -> bool) -> Result<()> {
    for item in fs::read_dir(from)? {
        let item = item?;
        if include(&item.file_name()) {
            symlink(item.path(), to.join(item.file_name()))?;
        }
    }
    Ok(())
}

impl Indexer for Depmod {
    fn generate(&self, s: &System, r: &str, include_olo: bool, output: &Path) -> Result<()> {
        let real = s.kernel(r);
        let view = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700)).tempdir_in(output)?;
        // For promotion depmod reads the actual selected root (-b),
        // and writes its results separately (-o). While withdrawing,
        // omit updates/olo in a private view so old indexes still
        // resolve until the verified replacement is durable.
        let basedir = if include_olo || !real.join(modules::OURS).try_exists()? {
            s.root.as_path()
        } else {
            let kernel = view.path().join("lib/modules").join(r);
            fs::create_dir_all(kernel.join("updates"))?;
            // Only inputs. An index in the real tree must never be opened
            // for writing through a symlink from the preview tree.
            for entry in fs::read_dir(&real)? {
                let e = entry?;
                let name = e.file_name();
                if matches!(name.to_str(), Some("build" | "source" | "updates")) {
                    continue;
                }
                if !name.as_encoded_bytes().starts_with(b"modules.") || e.path().is_dir() || matches!(name.to_str(), Some("modules.order" | "modules.builtin" | "modules.builtin.modinfo")) {
                    symlink(e.path(), kernel.join(name))?;
                }
            }
            if real.join("updates").is_dir() {
                links(&real.join("updates"), &kernel.join("updates"), |name| name != "olo")?;
            }
            view.path()
        };
        let mut cmd = Command::new("/usr/sbin/depmod");
        cmd.env_clear().env("PATH", "/usr/sbin:/usr/bin").env("LC_ALL", "C").arg("-b").arg(basedir).args(["-m", "/lib/modules", "-o"]).arg(output);
        if s.root != Path::new("/") {
            let empty = view.path().join("empty-config");
            fs::create_dir(&empty)?;
            for dir in ["etc/depmod.d", "run/depmod.d", "usr/local/lib/depmod.d", "usr/lib/depmod.d"] {
                let dir = s.root.join(dir);
                if dir.is_dir() {
                    cmd.arg("-C").arg(dir);
                }
            }
            cmd.arg("-C").arg(empty);
        }
        let out = cmd.arg(r).output().context("running the kernel module indexer")?;
        if !out.status.success() || !out.stderr.is_empty() {
            bail!("depmod did not produce a clean index ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}

// The first path in a row is the selected module. The rest are its
// dependencies, in depmod order. Restrict this writer to release-relative
// Fedora module paths. Refuse an unfamiliar layout before publication.
type Graph = BTreeMap<String, Vec<String>>;

fn module_key(path: &str) -> Result<String> {
    let p = Path::new(path);
    ensure!(p.components().all(|c| matches!(c, std::path::Component::Normal(_))) && !path.bytes().any(|b| b.is_ascii_whitespace() || b == b':'), "unsupported module path {path:?}");
    let file = p.file_name().and_then(OsStr::to_str).context("missing module filename")?;
    let stem = [".ko", ".ko.xz", ".ko.zst", ".ko.gz"].iter().find_map(|suffix| file.strip_suffix(suffix)).filter(|s| !s.is_empty()).with_context(|| format!("unsupported module filename {file}"))?;
    Ok(stem.replace('-', "_"))
}

fn graph(rows: &str, dump: bool) -> Result<Graph> {
    let mut result = Graph::new();
    for row in rows.lines() {
        let (name, row) = if dump {
            let (name, row) = row.split_once(' ').context("invalid libkmod dependency record")?;
            (Some(name), row)
        } else {
            (None, row)
        };
        let (path, deps) = row.split_once(':').context("missing dependency separator")?;
        let key = module_key(path)?;
        ensure!(name.is_none_or(|n| n == key), "wrong binary lookup key for {path}");
        let paths = std::iter::once(path).chain(deps.split_ascii_whitespace()).map(str::to_owned).collect();
        ensure!(result.insert(key, paths).is_none(), "duplicate module in dependency index: {path}");
    }
    Ok(result)
}

fn checked_graph(dir: &Path, scratch: &Path) -> Result<Graph> {
    #[link(name = "libkmod.so.2", kind = "dylib", modifiers = "+verbatim")]
    unsafe extern "C" {
        fn kmod_new(dir: *const c_char, config: *const *const c_char) -> *mut c_void;
        fn kmod_unref(ctx: *mut c_void) -> *mut c_void;
        fn kmod_dump_index(ctx: *mut c_void, index: libc::c_int, fd: libc::c_int) -> libc::c_int;
    }
    let text = graph(&fs::read_to_string(dir.join("modules.dep"))?, false)?;
    let path = CString::new(dir.as_os_str().as_bytes())?;
    let config = [std::ptr::null()];
    let mut dump = tempfile::tempfile_in(scratch)?;
    let ctx = unsafe { kmod_new(path.as_ptr(), config.as_ptr()) };
    ensure!(!ctx.is_null(), "cannot open kernel module indexes at {}", dir.display());
    // KMOD_INDEX_MODULES_DEP is zero in libkmod's public ABI.
    let ret = unsafe { kmod_dump_index(ctx, 0, dump.as_raw_fd()) };
    unsafe { kmod_unref(ctx) };
    ensure!(ret == 0, "cannot read the binary dependency index at {}", dir.display());
    dump.rewind()?;
    let mut records = String::new();
    dump.read_to_string(&mut records)?;
    ensure!(text == graph(&records, true)?, "binary and text dependency graphs disagree at {}", dir.display());
    Ok(text)
}

fn managed_name(path: &str) -> Option<&str> {
    let path = Path::new(path);
    if matches!(path.parent()?.to_str()?, modules::STOCK | modules::OURS) { modules::module_name(path) } else { None }
}

fn verify_graphs(active: &Path, candidate: &Path, scratch: &Path) -> Result<()> {
    let old = checked_graph(active, scratch).context("checking the existing distribution index")?;
    let new = checked_graph(candidate, scratch).context("checking the proposed index")?;
    for (key, old_row) in &old {
        let Some(new_row) = new.get(key) else {
            // A withdrawal may have to retire a stale NVIDIA set after
            // the distribution removed it. Never lose an unrelated row.
            ensure!(managed_name(&old_row[0]).is_some(), "depmod omitted {}", old_row[0]);
            continue;
        };
        ensure!(
            old_row.len() == new_row.len() && old_row.iter().zip(new_row).all(|(a, b)| { a == b || managed_name(a).is_some_and(|n| managed_name(b) == Some(n)) }),
            "depmod changed the provider or dependencies of {key}"
        );
    }
    for row in new.values() {
        ensure!(fs::metadata(active.join(&row[0]))?.is_file(), "indexed module is missing: {}", row[0]);
        for dep in &row[1..] {
            ensure!(new.get(&module_key(dep)?).is_some_and(|r| &r[0] == dep), "missing dependency {dep} of {}", row[0]);
        }
    }
    Ok(())
}

pub struct Index {
    files: TempDir,
    names: Vec<std::ffi::OsString>,
}
impl Index {
    pub fn prepare(s: &System, r: &str, ours: bool, tool: &dyn Indexer) -> Result<Self> {
        let work = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700)).prefix(".index-").tempdir_in(s.state())?;
        let dir = work.path().join("lib/modules").join(r);
        fs::create_dir_all(&dir)?;
        tool.generate(s, r, ours, work.path())?;
        for name in [
            "modules.dep",
            "modules.dep.bin",
            "modules.alias",
            "modules.alias.bin",
            "modules.softdep",
            "modules.weakdep",
            "modules.symbols",
            "modules.symbols.bin",
            "modules.builtin.bin",
            "modules.builtin.alias.bin",
            "modules.devname",
        ] {
            ensure!(fs::symlink_metadata(dir.join(name)).with_context(|| format!("missing depmod output {name}"))?.is_file(), "invalid depmod output {name}");
        }
        verify_graphs(&s.kernel(r), &dir, work.path())?;
        let selected = modules::index(&dir)?;
        let verify = work.path().join("verify");
        fs::create_dir(&verify)?;
        links(&dir, &verify, |_| true)?;
        for entry in fs::read_dir(s.kernel(r))? {
            let e = entry?;
            let name = e.file_name();
            if e.path().is_dir() && !matches!(name.to_str(), Some("build" | "source")) {
                symlink(e.path(), verify.join(name))?;
            }
        }
        modules::verify_binary(&verify, &selected)?;
        // A missing or partial stock set must still lose a stale
        // override. Verify the remaining stock files, and report the
        // incomplete stock set to the caller after the withdrawal.
        let expected = if ours { modules::OURS } else { modules::STOCK };
        for name in modules::NAMES {
            if let Some(path) = selected.get(name) {
                ensure!(path.parent() == Some(Path::new(expected)) && s.kernel(r).join(path).is_file(), "depmod did not select {expected}/{name} for {r}");
            } else {
                ensure!(!ours && !["ko", "ko.xz", "ko.zst"].iter().any(|ext| s.kernel(r).join(expected).join(format!("{name}.{ext}")).exists()), "depmod did not select {name} for {r}");
            }
        }
        ensure!(dir.join("modules.dep.bin").metadata()?.len() > 0, "depmod did not write its binary dependency index");
        // Reserve the entire rollback on the module filesystem, before
        // promoting anything. Reverting a full disk then needs only
        // renames, never a second depmod run or another copy of an index.
        let files = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700)).prefix(".olo-index-").tempdir_in(s.kernel(r))?;
        let mut names = Vec::new();
        for e in fs::read_dir(dir)? {
            let e = e?;
            ensure!(e.file_type()?.is_file() && e.file_name().as_encoded_bytes().starts_with(b"modules."), "unexpected file in depmod's output");
            let mut src = File::open(e.path())?;
            let mut dest = File::create_new(files.path().join(e.file_name()))?;
            io::copy(&mut src, &mut dest)?;
            dest.set_permissions(fs::Permissions::from_mode(0o644))?;
            dest.sync_all()?;
            names.push(e.file_name());
        }
        names.sort_by_key(|n| n == "modules.dep.bin");
        Ok(Self { files, names })
    }
    pub fn publish(self, kernel: &Path) -> Result<()> {
        for name in self.names {
            fs::rename(self.files.path().join(&name), kernel.join(name))?;
        }
        File::open(kernel)?.sync_all()?;
        Ok(())
    }
}
