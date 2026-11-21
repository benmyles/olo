//! Mutations are serialized across the service, Doctor and the CLI.
//! Stock and both indexes stay available until a promotion has been
//! verified. A surviving manifest also lets check finish after a crash.
use crate::{
    boot, build,
    index::{Index, Indexer},
    modules::{self, OURS, STOCK, Set},
    patches,
    system::{self, Manifest, Outcome, System},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::PermissionsExt,
        io::{AsRawFd, FromRawFd},
    },
    path::{Path, PathBuf},
};

pub fn requirements(s: &System, r: &str) -> Result<(Set, PathBuf)> {
    let stock = modules::set(&s.kernel(r).join(STOCK), r).context("checking the distribution's five NVIDIA modules")?;
    let version = &stock["nvidia"].version;
    ensure!(version == build::VERSION, "these patches are for {}, the distribution has {version}", build::VERSION);
    let headers = s.headers(r)?;
    ensure!(!boot::secure_boot(&s.root)?, "Secure Boot is on; Olo does not yet sign these NVIDIA modules");
    boot::running_unsigned_policy(&s.root)?;
    boot::initramfs(&s.root, r)?;
    Ok((stock, headers))
}

fn compatible(stock: &Set, built: &Set) -> Result<()> {
    for name in modules::NAMES {
        let m = &built[name];
        let orig = &stock[name];
        ensure!(m.version == orig.version && m.vermagic == orig.vermagic, "{name} has a different driver version or vermagic from the distribution");
    }
    Ok(())
}

pub fn healthy(s: &System, r: &str) -> Result<Manifest> {
    let m: Manifest = system::json(&s.manifest(r)).context("cannot read the installed set's manifest")?;
    ensure!(
        m.kernel == r && m.version == build::VERSION && m.set == "prevention" && m.patches == patches::hashes()? && m.source_sha256 == build::SOURCE_SHA && m.source_commit == build::COMMIT,
        "the installed manifest does not match the reviewed source and patches"
    );
    let kernel = s.kernel(r);
    let stock = modules::set(&kernel.join(STOCK), r)?;
    let dir = kernel.join(OURS);
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().and_then(|n| n.strip_suffix(".ko"));
        ensure!(entry.file_type()?.is_file() && name.is_some_and(|n| modules::NAMES.contains(&n)), "unexpected file in the installed NVIDIA set: {}", entry.path().display());
    }
    let installed = modules::set(&dir, r)?;
    ensure!(installed["nvidia"].version == m.version && installed == m.modules, "the installed modules differ from their manifest");
    compatible(&stock, &installed)?;
    ensure!(!boot::secure_boot(&s.root)?, "Secure Boot is enabled or could not be checked");
    boot::running_unsigned_policy(&s.root)?;
    boot::initramfs(&s.root, r)?;
    modules::verify_index(&kernel, OURS)?;
    Ok(m)
}

fn forget(s: &System, r: &str) -> Result<()> {
    let target = s.kernel(r).join(OURS);
    if target.try_exists()? {
        let parent = target.parent().unwrap();
        let retired = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700)).prefix(".olo-retired-").tempdir_in(s.kernel(r).parent().unwrap())?;
        fs::rename(&target, retired.path().join("olo"))?;
        File::open(parent)?.sync_all()?;
        retired.close()?;
    }
    system::remove_file(&s.manifest(r))
}

pub fn withdraw(s: &System, r: &str, tool: &dyn Indexer) -> Result<()> {
    s.destinations(r)?;
    let _packages = s.packages()?;
    let kernel = s.kernel(r);
    let stock = Index::prepare(s, r, false, tool)?;
    stock.publish(&kernel)?;
    // Only remove Olo once the active index has been switched, even
    // if the package manager has removed some or all of the stock set.
    let selected = modules::index(&kernel)?;
    modules::verify_binary(&kernel, &selected)?;
    ensure!(selected.values().all(|p| p.parent() == Some(Path::new(STOCK))), "the index still selects an override; its files have been kept");
    forget(s, r)?;
    modules::verify_index(&kernel, STOCK).context("override withdrawn; check the distribution's NVIDIA module package")?;
    modules::set(&kernel.join(STOCK), r).context("override withdrawn; the distribution's NVIDIA set is incomplete or inconsistent")?;
    Ok(())
}

// The compiler ran without privilege. Follow no link in its outputs,
// never open a special file, and validate the root-owned staged copy.
fn stage_outputs(tree: &Path, dir: &Path) -> Result<()> {
    let parent = File::open(tree)?;
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK) as _;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    for n in modules::NAMES {
        let filename = format!("{n}.ko");
        let rel = CString::new(format!("kernel-open/{filename}"))?;
        let fd = unsafe { libc::syscall(libc::SYS_openat2, parent.as_raw_fd(), rel.as_ptr(), &how, std::mem::size_of::<libc::open_how>()) };
        if fd < 0 {
            return Err(io::Error::last_os_error()).with_context(|| format!("opening built {filename}"));
        }
        let file = unsafe { File::from_raw_fd(fd as i32) };
        let size = file.metadata()?;
        ensure!(size.is_file() && size.len() <= 256 << 20, "invalid build output {filename}");
        let mut out = File::create_new(dir.join(&filename))?;
        ensure!(io::copy(&mut file.take(size.len() + 1), &mut out)? == size.len(), "{filename} changed while it was copied");
        out.set_permissions(fs::Permissions::from_mode(0o644))?;
        out.sync_all()?;
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub fn install(s: &System, r: &str, tree: &Path, expected: &Set, tool: &dyn Indexer, enable: bool) -> Result<()> {
    let kernel = s.kernel(r);
    let updates = kernel.join("updates");
    fs::create_dir_all(&updates)?;
    // depmod scans hidden folders as well. Stage beside the release,
    // on the module filesystem but outside its scan, then rename the
    // whole directory into updates/olo.
    let staging = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700)).prefix(".olo-stage-").tempdir_in(kernel.parent().unwrap())?;
    stage_outputs(tree, staging.path())?;
    let built = modules::set(staging.path(), r)?;
    compatible(expected, &built)?;
    let manifest = Manifest {
        kernel: r.into(),
        version: build::VERSION.into(),
        set: "prevention".into(),
        source_commit: build::COMMIT.into(),
        source_sha256: build::SOURCE_SHA.into(),
        patches: patches::hashes()?,
        modules: built,
        built_at: system::now(),
    };
    let _packages = s.packages()?;
    // A package update or replacement boot image can overlap make.
    // Recheck at promotion, this time holding the package lock.
    ensure!(&requirements(s, r)?.0 == expected, "distribution modules changed during the build; try again after the package update");
    let fallback = Index::prepare(s, r, false, tool)?;
    let result = (|| -> Result<()> {
        // Record intent first. If power goes out here, check can
        // regenerate the stock index even if no folder was published.
        system::save(&s.manifest(r), &manifest)?;
        let target = kernel.join(OURS);
        if target.try_exists()? {
            // Every path referenced by the old index stays reachable
            // even if power fails at this rename. The old generation
            // is now the private staging folder, cleaned up on return.
            let a = CString::new(staging.path().as_os_str().as_bytes())?;
            let b = CString::new(target.as_os_str().as_bytes())?;
            if unsafe { libc::renameat2(libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), libc::RENAME_EXCHANGE) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
        } else {
            fs::rename(staging.path(), &target)?;
        }

        File::open(&updates)?.sync_all()?;
        let selected = Index::prepare(s, r, true, tool)?;
        selected.publish(&kernel)?;
        modules::verify_index(&kernel, OURS)?;
        ensure!(&modules::set(&kernel.join(STOCK), r)? == expected, "distribution modules changed during installation");
        if enable {
            system::atomic(&s.state().join("prevention.enabled"), b"prevention\n")?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        let rollback = (|| -> Result<()> {
            fallback.publish(&kernel)?;
            modules::verify_index(&kernel, STOCK)?;
            forget(s, r)
        })();
        match rollback {
            Ok(()) => bail!("{err:#}; all five distribution modules selected again"),
            Err(undo) => bail!("{err:#}; restoring the distribution's index also failed: {undo:#}"),
        }
    }
    Ok(())
}

pub fn build(s: &System, r: &str, fetch: bool, tool: &dyn Indexer) -> Result<()> {
    s.destinations(r)?;
    let result = (|| -> Result<()> {
        let (stock, headers) = {
            let _packages = s.packages()?;
            if (s.manifest(r).try_exists()? || s.kernel(r).join(OURS).try_exists()?) && healthy(s, r).is_err() {
                withdraw(s, r, tool).context("withdrawing the stale installed set before building")?;
            }
            requirements(s, r)?
        };
        let archive = build::source(&s.state(), fetch)?;
        let work = build::Work::from_archive(&s.state().join("build").join(r), &archive)?;
        work.compile(&headers, r)?;
        install(s, r, &work.tree(), &stock, tool, fetch)
    })();
    let message = match &result {
        Ok(()) => format!("Five prevention modules installed for {r}'s next start"),
        Err(e) => format!("{e:#}"),
    };
    let history = system::save(&s.outcome(r), &Outcome { ok: result.is_ok(), at: system::now(), message });
    match (result, history) {
        (result, Ok(())) => result,
        (Ok(()), Err(e)) => bail!("installed the modules for {r}, but could not save the build outcome: {e:#}"),
        (Err(e), Err(note)) => bail!("{e:#}; could not save the build outcome: {note:#}"),
    }
}

// One failed release must not prevent withdrawal on the others.
fn reconcile_kernel(s: &System, r: &str, opted_in: bool, tool: &dyn Indexer) -> Result<bool> {
    let _packages = s.packages()?;
    let kernel = s.kernel(r);
    if !kernel.join(OURS).try_exists()? && !s.manifest(r).try_exists()? {
        if !kernel.join("modules.dep").try_exists()? && !kernel.join("modules.dep.bin").try_exists()? && !kernel.join(STOCK).try_exists()? {
            return Ok(false);
        }
        let paths = modules::index(&kernel)?;
        modules::verify_binary(&kernel, &paths)?;
        if !paths.values().any(|p| p.parent() == Some(Path::new(OURS))) {
            return Ok(false);
        }
        withdraw(s, r, tool).context("repairing an index that points to a removed Olo set")?;
        return Ok(true);
    }
    if !opted_in {
        withdraw(s, r, tool).context("finishing withdrawal after opt-out")?;
        return Ok(true);
    }
    let Err(why) = healthy(s, r) else { return Ok(false) };
    let withdrawn = withdraw(s, r, tool);
    let note = Outcome { at: system::now(), ok: false, message: format!("Checked installed set: {why:#}") };
    let history = s.directory(&s.state().join("checks")).and_then(|()| system::save(&s.state().join("checks").join(format!("{r}.json")), &note));
    match (withdrawn, history) {
        (Ok(()), Ok(())) => eprintln!("Withdrew the override for {r}: {why:#}"),
        (Err(e), Ok(())) => bail!("{why:#}; {e:#}"),
        (Ok(()), Err(e)) => bail!("override withdrawn: {why:#}; could not record the check: {e:#}"),
        (Err(e), Err(note)) => bail!("{why:#}; {e:#}; could not record the check: {note:#}"),
    }
    Ok(true)
}

pub fn check(s: &System, tool: &dyn Indexer, mut build_next: impl FnMut(&str) -> Result<()>) -> Result<()> {
    let opted_in = s.opted_in()?;
    let mut errors = Vec::new();
    let mut newest = None;
    let mut withdrawn = Vec::new();
    for r in s.releases()? {
        if s.kernel(&r).join(STOCK).is_dir() {
            newest = Some(r.clone());
        }
        match reconcile_kernel(s, &r, opted_in, tool) {
            Ok(false) => {}
            Ok(true) => withdrawn.push(r),
            Err(e) => {
                errors.push(format!("{r}: {e:#}"));
                withdrawn.push(r);
            }
        }
    }
    if let Some(r) = newest
        && opted_in
        && !withdrawn.contains(&r)
        && !s.kernel(&r).join(OURS).try_exists()?
        && let Err(e) = build_next(&r)
    {
        errors.push(format!("{r}: {e:#}"));
    }
    ensure!(errors.is_empty(), "{}", errors.join("\n"));
    Ok(())
}
