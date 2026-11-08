//! What a fix is made of: small steps the app runs as this user, or the
//! privileged helper (helper.rs) runs as root, each doing one thing in
//! process (a file written, a sysfs knob set, a systemd unit stopped
//! over D-Bus): nothing is shelled out to.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnitAction {
    Start,
    Stop,
    Enable,
    Disable,
    /// Have systemd re-read its unit files (after one is removed).
    Reload,
    /// Start it, or start it again (an updated program).
    Restart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Step {
    /// Write a whole file (through a sibling, then renamed into place).
    WriteFile {
        path: PathBuf,
        content: String,
        mode: u32,
    },
    /// Copy a file (a program) whole, owned by whoever runs the step.
    CopyFile {
        from: PathBuf,
        to: PathBuf,
        mode: u32,
    },
    /// Have NetworkManager read its configuration again.
    NetworkManagerReload,
    /// Have the session bus read its configuration again (a user's D-Bus
    /// activation file written: it applies at once).
    SessionBusReload,
    /// Set variables in the session's environment for what starts from
    /// now on: systemd's user manager's and D-Bus activation's
    /// (`olo_system::env::update_activation_environment`).
    SetSessionEnvironment {
        vars: Vec<(String, String)>,
    },
    /// Write a value to a sysfs attribute.
    WriteSysfs {
        path: PathBuf,
        value: String,
    },
    /// Remove a file (already gone is fine).
    RemoveFile {
        path: PathBuf,
    },
    /// Act on a system (not user) systemd unit.
    Unit {
        name: String,
        action: UnitAction,
    },
    Signal {
        pid: i32,
        signal: i32,
    },
    /// List a program in /etc/shells (a login shell must be there).
    AddShell {
        path: PathBuf,
    },
    /// Start the kernel with `arg` ("key=value") from the next boot: in
    /// each boot entry (/boot/loader/entries, the rescue one left as it
    /// is), /etc/kernel/cmdline (what new kernels' entries get) and GRUB's
    /// defaults, as grubby does; a value for the same key replaced.
    AddKernelArgument {
        arg: String,
    },
    /// Run a shell once (`-c 'exit 0'`): it starts, before it's anyone's
    /// login shell.
    TryShell {
        path: PathBuf,
    },
    /// Make `shell` `user`'s login shell (AccountsService's SetShell).
    SetLoginShell {
        user: String,
        shell: PathBuf,
    },
    /// Install packages by name, from the system's trusted repositories
    /// (through PackageKit: `olo_system::packages`), with what they need.
    InstallPackages {
        names: Vec<String>,
    },
    /// Update these packages (those of them with an update waiting).
    UpdatePackages {
        names: Vec<String>,
    },
    /// Add a package repository: its release package downloaded from
    /// the first of `urls` that has it (the maker's own server, then its
    /// mirrors' redirector: each tried up to `TRIES` times, as a
    /// redirector sends each try to another mirror and one may be out of
    /// date) and installed (it brings the repository's settings and
    /// signing keys, which every package from it is then checked
    /// against), and the package lists read again.
    InstallRepository {
        name: String,
        urls: Vec<String>,
    },
    /// Copy a folder's files into another (made if need be), each over
    /// its old self (through a sibling renamed into place), `mode` each
    /// (Olo Setup: Olo's data where its login screen reads it).
    CopyTree {
        from: PathBuf,
        to: PathBuf,
        mode: u32,
    },
    /// Make a folder (and those above it) with `mode`, owned by `owner`
    /// (a user's name, its group the user's own) if given: a tmpfiles.d
    /// `d` line's work, done now rather than at the next boot (systemd
    /// won't start its tmpfiles service again by hand).
    MakeDir {
        path: PathBuf,
        mode: u32,
        owner: Option<String>,
    },
    /// SELinux's file labels for these paths (`(regex, type)`, recorded
    /// with the system's own others: `semanage fcontext`), then put on
    /// what's there (`restorecon -R` on `restore`). Nothing without
    /// SELinux. Olo's login screen is a display manager to SELinux, as GDM
    /// is: its program `xdm_exec_t` (so it runs as `xdm_t`, from which the
    /// person's session may start), its folders `xdm_var_run_t` and
    /// `xdm_var_lib_t`.
    SelinuxLabels {
        rules: Vec<(String, String)>,
        restore: Vec<PathBuf>,
    },
    /// Make the system users and groups a sysusers.d file names (as at
    /// boot): systemd's own program for it, run on that file
    /// (systemd-sysusers.service runs only after an update, and there's
    /// no other way to have systemd make one).
    MakeSystemUsers {
        config: PathBuf,
    },
    /// Make the start-up images (initramfs) again, for every kernel
    /// installed: what's in them (Plymouth's theme and settings) changed.
    /// One of the two steps that run another program (dracut, as root):
    /// the images are the boot system's own, and there is no other way
    /// to have it make them.
    RebuildInitramfs,
    /// Install `name` from its maker's latest GitHub release, its
    /// published SHA-256 checked, to `to` (`crate::releases`): a program
    /// the system's repositories don't carry (mise, worktrunk).
    InstallRelease {
        name: String,
        release: crate::releases::Release,
        to: PathBuf,
    },
    /// Start a graphics card and its driver again without restarting the
    /// computer: `services` that keep it open stopped (NVIDIA's own,
    /// Olo's GPU follower), its driver's `modules` unloaded in order (as
    /// `modprobe -r`, but never waiting on one in use: the kernel
    /// refuses at once, `delete_module` with `O_NONBLOCK`; asked again
    /// for a few seconds, then it fails saying what has `in_use_by` open:
    /// device nodes, one ending in `*` a prefix), the card reset
    /// (function-level, else its bus), taken off the bus and found again
    /// (`remove`, its bridge's `rescan`: fresh power management, a failed
    /// one staying failed until then), udev loading and binding its
    /// driver as at start (up to 30 s), and the services started again.
    /// Failing partway, nothing is left off: the card's driver is loaded
    /// again (udev told the card was added, which loads it by its PCI
    /// alias) and the services started.
    RestartGpu {
        address: String,
        modules: Vec<String>,
        services: Vec<String>,
        in_use_by: Vec<String>,
    },
    /// Start one of Olo's own programs (`olo-…`) with these arguments,
    /// detached, as the person (never as root): its own command line
    /// does the fix (`olo-diskbackup --now`).
    StartProgram {
        argv: Vec<String>,
    },
}

impl Step {
    /// What it does, for a report ("Writing /etc/udev/rules.d/…").
    pub fn describe(&self) -> String {
        match self {
            Step::WriteFile { path, .. } => format!("Writing {}", path.display()),
            Step::CopyFile { to, .. } => format!("Installing {}", to.display()),
            Step::NetworkManagerReload => "Reloading NetworkManager's settings".into(),
            Step::SessionBusReload => "Reloading the session bus's settings".into(),
            Step::SetSessionEnvironment { vars } => format!("Setting {} for the session", vars.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ")),
            Step::WriteSysfs { path, value } => format!("Setting {} to {value}", path.display()),
            Step::RemoveFile { path } => format!("Removing {}", path.display()),
            Step::Unit { name, action } => {
                let verb = match action {
                    UnitAction::Start => "Starting",
                    UnitAction::Stop => "Stopping",
                    UnitAction::Enable => "Enabling",
                    UnitAction::Disable => "Disabling",
                    UnitAction::Reload => "Reloading systemd for",
                    UnitAction::Restart => "Restarting",
                };
                format!("{verb} {name}")
            }
            Step::Signal { pid, .. } => format!("Stopping process {pid}"),
            Step::RestartGpu { address, .. } => format!("Starting the graphics card at {address} and its driver again"),
            Step::AddShell { path } => format!("Listing {} in /etc/shells", path.display()),
            Step::AddKernelArgument { arg } => format!("Starting the kernel with {arg}"),
            Step::TryShell { path } => format!("Trying {}", path.display()),
            Step::SetLoginShell { user, shell } => format!("Making {} {user}'s login shell", shell.display()),
            Step::InstallPackages { names } => format!("Installing {}", short_list(names)),
            Step::UpdatePackages { names } => format!("Updating {}", short_list(names)),
            Step::InstallRelease { name, release, .. } => format!("Installing {name} from github.com/{}", release.repo),
            Step::InstallRepository { name, .. } => format!("Adding the {name} repository"),
            Step::CopyTree { to, .. } => format!("Installing {}", to.display()),
            Step::MakeDir { path, .. } => format!("Making {}", path.display()),
            Step::MakeSystemUsers { config } => format!("Making the users {} names", config.display()),
            Step::SelinuxLabels { restore, .. } => format!("Labelling {} for SELinux", restore.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
            Step::RebuildInitramfs => "Making the start-up images again".into(),
            Step::StartProgram { argv } => format!("Running {}", argv.join(" ")),
        }
    }

    /// Do it, telling how it goes (a package download's progress) as it
    /// goes.
    pub fn run_with(&self, progress: &mut dyn FnMut(String)) -> Result<(), String> {
        let mut said = |p: olo_system::packages::Progress| {
            use olo_system::packages::Progress as P;
            match p {
                P::Percent(n) => progress(format!("{n}%")),
                P::Status(s) => progress(s.to_owned()),
                P::Package(name) => progress(name),
            }
        };
        match self {
            Step::InstallPackages { names } => {
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                olo_system::packages::install(&names, &mut said).map_err(|e| format!("{}: {e}", self.describe()))
            }
            Step::UpdatePackages { names } => {
                let waiting = olo_system::packages::updates().map_err(|e| format!("{}: {e}", self.describe()))?;
                let ids: Vec<String> = waiting.into_iter().filter(|p| names.iter().any(|n| n == p.name())).map(|p| p.id).collect();
                olo_system::packages::update(ids, &mut said).map_err(|e| format!("{}: {e}", self.describe()))
            }
            Step::InstallRepository { urls, .. } => {
                let fail = |e: String| format!("{}: {e}", self.describe());
                said(olo_system::packages::Progress::Status("Downloading"));
                let (url, bytes) = release_package(urls).map_err(fail)?;
                // (Where PackageKit, as root, reads it; gone after.)
                let name = url.rsplit('/').next().filter(|n| n.ends_with(".rpm")).unwrap_or("release.rpm");
                let dir = std::env::temp_dir().join(format!("olo-doctor-{}", std::process::id()));
                std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
                let file = dir.join(name);
                std::fs::write(&file, &bytes).map_err(|e| fail(e.to_string()))?;
                let done = olo_system::packages::install_file(&file, &mut said).and_then(|_| olo_system::packages::refresh(&mut said));
                let _ = std::fs::remove_dir_all(&dir);
                done.map_err(|e| fail(e.to_string()))
            }
            Step::InstallRelease { release, to, .. } => {
                let mut last = 0;
                crate::releases::install(release, to, &mut |done, total| {
                    let percent = if total > 0 { done * 100 / total } else { 0 };
                    if percent != last {
                        last = percent;
                        progress(format!("{percent}%"));
                    }
                })
                .map(|_| ())
                .map_err(|e| format!("{}: {e}", self.describe()))
            }
            _ => self.run(),
        }
    }

    /// Do it (as whoever runs this).
    pub fn run(&self) -> Result<(), String> {
        let failed = |e: std::io::Error| format!("{}: {e}", self.describe());
        match self {
            Step::WriteFile { path, content, mode } => write_file(path, content, *mode).map_err(failed),
            Step::CopyFile { from, to, mode } => std::fs::read(from).and_then(|bytes| write_bytes(to, &bytes, *mode)).map_err(failed),
            Step::NetworkManagerReload => olo_system::network::reload().map_err(|e| format!("{}: {e}", self.describe())),
            Step::SessionBusReload => olo_system::env::reload_session_bus().map_err(|e| format!("{}: {e}", self.describe())),
            Step::SetSessionEnvironment { vars } => {
                let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                olo_system::env::update_activation_environment(&pairs).map_err(|e| format!("{}: {e}", self.describe()))
            }
            Step::WriteSysfs { path, value } => std::fs::write(path, value).map_err(failed),
            Step::RemoveFile { path } => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(failed(e)),
                _ => Ok(()),
            },
            Step::Unit { name, action } => unit(name, action).map_err(|e| format!("{}: {e}", self.describe())),
            Step::Signal { pid, signal } => {
                // SAFETY: kill with a pid and signal number has no memory effects.
                if unsafe { libc::kill(*pid, *signal) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(failed(std::io::Error::last_os_error())) }
            }
            Step::RestartGpu { address, modules, services, in_use_by } => restart_gpu(address, modules, services, in_use_by),
            Step::StartProgram { argv } => {
                // (Olo's own, as the person: never another program, never root.)
                // SAFETY: geteuid has no preconditions.
                if argv.first().is_none_or(|p| !p.starts_with("olo-") || p.contains('/')) || unsafe { libc::geteuid() } == 0 {
                    return Err(format!("{}: not one of Olo's programs, as you", self.describe()));
                }
                let argv: Vec<std::ffi::OsString> = argv.iter().map(Into::into).collect();
                olo_xdg::open::spawn(&argv).map_err(failed)
            }
            Step::AddShell { path } => add_shell(path).map_err(failed),
            Step::AddKernelArgument { arg } => add_kernel_argument(Path::new("/"), arg).map_err(failed),
            Step::TryShell { path } => try_shell(path),
            Step::SetLoginShell { user, shell } => set_login_shell(user, shell).map_err(|e| format!("{}: {e}", self.describe())),
            Step::InstallPackages { .. } | Step::UpdatePackages { .. } | Step::InstallRelease { .. } | Step::InstallRepository { .. } => self.run_with(&mut |_| {}),
            Step::CopyTree { from, to, mode } => copy_tree(from, to, *mode).map_err(failed),
            Step::MakeDir { path, mode, owner } => make_dir(path, *mode, owner.as_deref()).map_err(failed),
            Step::SelinuxLabels { rules, restore } => selinux_labels(rules, restore).map_err(|e| format!("{}: {e}", self.describe())),
            Step::MakeSystemUsers { config } => {
                let out = std::process::Command::new("systemd-sysusers").arg(config).stdin(std::process::Stdio::null()).output();
                match out {
                    Ok(o) if o.status.success() => Ok(()),
                    Ok(o) => Err(format!("{}: systemd-sysusers: {}", self.describe(), String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("failed"))),
                    Err(e) => Err(failed(e)),
                }
            }
            Step::RebuildInitramfs => {
                let status = std::process::Command::new("dracut")
                    .args(["--force", "--regenerate-all"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .output();
                match status {
                    Ok(o) if o.status.success() => Ok(()),
                    Ok(o) => Err(format!("{}: dracut: {}", self.describe(), String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("failed"))),
                    Err(e) => Err(failed(e)),
                }
            }
        }
    }
}

/// Record SELinux's labels for `rules` and put them on `restore` (see
/// `Step::SelinuxLabels`); nothing where SELinux is off.
fn selinux_labels(rules: &[(String, String)], restore: &[PathBuf]) -> Result<(), String> {
    if !Path::new("/sys/fs/selinux/enforce").exists() {
        return Ok(());
    }
    let run = |args: &[&str]| -> Result<String, String> {
        let o = std::process::Command::new(args[0]).args(&args[1..]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("{}: {e}", args[0]))?;
        if o.status.success() { Ok(String::new()) } else { Err(format!("{}: {}", args[0], String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("failed"))) }
    };
    for (regex, kind) in rules {
        // Added, or changed if one is there already.
        if run(&["semanage", "fcontext", "-a", "-t", kind, regex]).is_err() {
            run(&["semanage", "fcontext", "-m", "-t", kind, regex])?;
        }
    }
    for p in restore.iter().filter(|p| p.exists()) {
        run(&["restorecon", "-R", &p.display().to_string()])?;
    }
    Ok(())
}

/// `path` made with `mode`, owned by user `owner` and their group.
fn make_dir(path: &Path, mode: u32, owner: Option<&str>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    if let Some(name) = owner {
        let c = std::ffi::CString::new(name).map_err(std::io::Error::other)?;
        // SAFETY: getpwnam's static entry, read at once.
        let ids = unsafe {
            let pw = libc::getpwnam(c.as_ptr());
            (!pw.is_null()).then(|| ((*pw).pw_uid, (*pw).pw_gid))
        };
        let (uid, gid) = ids.ok_or_else(|| std::io::Error::other(format!("there's no user {name}")))?;
        std::os::unix::fs::chown(path, Some(uid), Some(gid))?;
    }
    Ok(())
}

/// Every file of `from` copied into `to`, each over its old self.
fn copy_tree(from: &Path, to: &Path, mode: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target, mode)?;
        } else {
            write_bytes(&target, &std::fs::read(e.path())?, mode)?;
        }
    }
    Ok(())
}

/// A few names, and how many more ("kernel, linux-firmware and 4 more").
/// `Step::RestartGpu`.
fn restart_gpu(address: &str, modules: &[String], services: &[String], in_use_by: &[String]) -> Result<(), String> {
    let mut said = Vec::new();
    for s in services {
        if let Err(e) = unit(s, &UnitAction::Stop) {
            said.push(format!("stopping {s}: {e}"));
        }
    }
    let done = unload_modules(modules, in_use_by).and_then(|()| restart_pci_device(address));
    if done.is_err() {
        // (What was unloaded, loaded again: the card as udev found it at
        // start, its PCI alias naming the driver.)
        let _ = std::fs::write(Path::new("/sys/bus/pci/devices").join(address).join("uevent"), "add");
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    for s in services {
        if let Err(e) = unit(s, &UnitAction::Start) {
            said.push(format!("starting {s} again: {e}"));
        }
    }
    match done {
        Err(e) => Err(if said.is_empty() { e } else { format!("{e} ({})", said.join("; ")) }),
        Ok(()) if !said.is_empty() => Err(format!("the card is back, but {}", said.join("; "))),
        Ok(()) => Ok(()),
    }
}

/// Unload `names`, in order (`Step::RestartGpu`).
fn unload_modules(names: &[String], in_use_by: &[String]) -> Result<(), String> {
    use std::time::{Duration, Instant};
    for name in names {
        if !Path::new("/sys/module").join(name).exists() {
            continue;
        }
        let c = std::ffi::CString::new(name.as_str()).map_err(|e| e.to_string())?;
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            // SAFETY: a NUL-terminated name and a flag; nothing is handed
            // to the kernel to keep.
            if unsafe { libc::syscall(libc::SYS_delete_module, c.as_ptr(), libc::O_NONBLOCK) } == 0 {
                break;
            }
            let e = std::io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::ENOENT) => break,
                Some(libc::EWOULDBLOCK | libc::EBUSY) if Instant::now() < until => std::thread::sleep(Duration::from_millis(200)),
                _ => {
                    let users = holders(in_use_by);
                    return Err(if users.is_empty() { format!("{name} couldn't be unloaded ({e})") } else { format!("{name} is still in use, by {}", users.join(", ")) });
                }
            }
        }
    }
    Ok(())
}

/// The processes with one of `nodes` open ("chrome (process 12)"), as
/// root sees them.
fn holders(nodes: &[String]) -> Vec<String> {
    let matches = |link: &str| nodes.iter().any(|n| n.strip_suffix('*').map_or(link == n, |prefix| link.starts_with(prefix)));
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let open = std::fs::read_dir(e.path().join("fd")).into_iter().flatten().flatten().filter_map(|f| std::fs::read_link(f.path()).ok()).any(|l| matches(&l.to_string_lossy()));
        if open {
            let name = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            out.push(format!("{} (process {pid})", name.trim()));
        }
    }
    out
}

/// The card at `address` reset, off the bus and found again, its driver
/// unloaded first (`Step::RestartGpu`).
fn restart_pci_device(address: &str) -> Result<(), String> {
    use std::time::{Duration, Instant};
    let dev = Path::new("/sys/bus/pci/devices").join(address);
    let real = std::fs::canonicalize(&dev).map_err(|e| format!("{address}: {e}"))?;
    let bridge = real.parent().ok_or_else(|| format!("{address}: no bridge above it"))?.to_path_buf();
    if dev.join("driver").exists() {
        return Err(format!("{address} still has its driver"));
    }
    // (A card whose firmware stopped wants a reset; its driver resets it
    // again as it starts, so one the kernel can't do isn't the end.)
    let _ = std::fs::write(dev.join("reset"), "1");
    std::fs::write(dev.join("remove"), "1").map_err(|e| format!("taking {address} off the bus: {e}"))?;
    let until = Instant::now() + Duration::from_secs(5);
    while dev.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::write(bridge.join("rescan"), "1").map_err(|e| format!("looking for {address} on the bus again: {e}"))?;
    let until = Instant::now() + Duration::from_secs(30);
    while !dev.join("driver").exists() {
        if Instant::now() >= until {
            return Err(if dev.exists() { format!("{address} is back on the bus, but its driver didn't start") } else { format!("{address} wasn't found on the bus again") });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if std::fs::read_to_string(dev.join("power/runtime_status")).is_ok_and(|s| s.trim() == "error") {
        return Err(format!("{address} is back, but its power management failed again"));
    }
    Ok(())
}

fn short_list(names: &[String]) -> String {
    match names.len() {
        0..=3 => names.join(", "),
        n => format!("{} and {} more", names[..3].join(", "), n - 3),
    }
}

/// `path` listed in /etc/shells (it is already: nothing written).
fn add_shell(path: &Path) -> std::io::Result<()> {
    add_shell_to(Path::new("/etc/shells"), path)
}

/// `arg` ("key=value") in every place the kernel's command line is kept
/// under `root` (see `Step::AddKernelArgument`).
fn add_kernel_argument(root: &Path, arg: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    let entries = root.join("boot/loader/entries");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&entries).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "conf") && !p.to_string_lossy().contains("rescue")).collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f)?;
        let new: Vec<String> = text
            .lines()
            .map(|l| match l.strip_prefix("options ") {
                Some(o) => format!("options {}", with_argument(o, arg)),
                None => l.to_owned(),
            })
            .collect();
        let new = new.join("\n") + if text.ends_with('\n') { "\n" } else { "" };
        if new != text {
            write_file(&f, &new, mode(&f))?;
        }
    }
    let cmdline = root.join("etc/kernel/cmdline");
    if let Ok(text) = std::fs::read_to_string(&cmdline) {
        let new = format!("{}\n", with_argument(text.trim(), arg));
        if new != text {
            write_file(&cmdline, &new, mode(&cmdline))?;
        }
    }
    let grub = root.join("etc/default/grub");
    if let Ok(text) = std::fs::read_to_string(&grub) {
        let new: Vec<String> = text
            .lines()
            .map(|l| match l.strip_prefix("GRUB_CMDLINE_LINUX=\"").and_then(|v| v.strip_suffix('"')) {
                Some(v) => format!("GRUB_CMDLINE_LINUX=\"{}\"", with_argument(v, arg)),
                None => l.to_owned(),
            })
            .collect();
        let new = new.join("\n") + if text.ends_with('\n') { "\n" } else { "" };
        if new != text {
            write_file(&grub, &new, mode(&grub))?;
        }
    }
    Ok(())
}

/// A command line with `arg`: its key's value replaced, else added.
fn with_argument(line: &str, arg: &str) -> String {
    let key = arg.split('=').next().unwrap_or(arg);
    let mut found = false;
    let mut out: Vec<&str> = line
        .split_whitespace()
        .map(|t| {
            if t.split('=').next() == Some(key) {
                found = true;
                arg
            } else {
                t
            }
        })
        .collect();
    if !found {
        out.push(arg);
    }
    out.join(" ")
}

/// `path` listed in the shells file `shells`.
fn add_shell_to(shells: &Path, path: &Path) -> std::io::Result<()> {
    let text = std::fs::read_to_string(shells).unwrap_or_default();
    let line = path.to_string_lossy();
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut text = text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    write_file(shells, &text, 0o644)
}

/// The shell at `path` runs a command and exits cleanly.
fn try_shell(path: &Path) -> Result<(), String> {
    // (Olo's own build of it: the one program a step runs.)
    match std::process::Command::new(path).args(["-c", "exit 0"]).stdin(std::process::Stdio::null()).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{} didn't run a command ({s})", path.display())),
        Err(e) => Err(format!("{} didn't start: {e}", path.display())),
    }
}

/// `user`'s login shell made `shell`, through AccountsService (which
/// asks polkit; root, as the helper is, may).
fn set_login_shell(user: &str, shell: &Path) -> Result<(), String> {
    olo_system::accounts::set_shell(user, &shell.to_string_lossy()).map_err(|e| e.to_string())
}

fn write_file(path: &Path, content: &str, mode: u32) -> std::io::Result<()> {
    write_bytes(path, content.as_bytes(), mode)
}

fn write_bytes(path: &Path, content: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.olo-new"));
    std::fs::write(&tmp, content)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    // The SELinux label of the file it replaces, kept: a new file takes
    // its folder's (/etc/default/grub became etc_t, not bootloader_etc_t).
    if let Some(label) = selinux_label(path) {
        set_selinux_label(&tmp, &label);
    }
    std::fs::rename(&tmp, path)
}

const SELINUX: &std::ffi::CStr = c"security.selinux";

/// A file's SELinux label (its xattr), if it has one.
fn selinux_label(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt as _;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = vec![0u8; 256];
    // SAFETY: c and buf are valid for the call; the length is buf's.
    let n = unsafe { libc::getxattr(c.as_ptr(), SELINUX.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    (n > 0).then(|| {
        buf.truncate(n as usize);
        buf
    })
}

fn set_selinux_label(path: &Path, label: &[u8]) {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
    // SAFETY: c and label are valid for the call.
    let _ = unsafe { libc::setxattr(c.as_ptr(), SELINUX.as_ptr(), label.as_ptr().cast(), label.len(), 0) };
}

pub use olo_system::systemd::UnitState;

/// What systemd says about a system unit.
pub fn unit_state(name: &str) -> Result<UnitState, String> {
    olo_system::systemd::unit_state(name).map_err(|e| e.to_string())
}

fn unit(name: &str, action: &UnitAction) -> Result<(), String> {
    use olo_system::systemd;
    match action {
        UnitAction::Start => systemd::start_stop(name, true),
        UnitAction::Stop => systemd::start_stop(name, false),
        UnitAction::Enable => systemd::enable(name, true),
        UnitAction::Disable => systemd::enable(name, false),
        UnitAction::Reload => systemd::reload(),
        UnitAction::Restart => systemd::restart(name),
    }
    .map_err(|e| e.to_string())
}

/// How many times each place a release package might be is tried.
const TRIES: usize = 3;

/// A repository's release package from the first of `urls` that has one
/// (an RPM: its lead's magic, not a mirror's error page), each tried up
/// to `TRIES` times; where it came from, and its bytes. Every place's
/// answer said when none has it.
fn release_package(urls: &[String]) -> Result<(String, Vec<u8>), String> {
    let mut why = Vec::new();
    for url in urls {
        for _ in 0..TRIES {
            match crate::releases::download(url, 16 << 20, &mut |_, _| {}) {
                Ok(bytes) if is_rpm(&bytes) => return Ok((url.clone(), bytes)),
                Ok(_) => why.push(format!("{url} sent something other than a package")),
                Err(e) => why.push(e),
            }
        }
    }
    why.dedup();
    Err(if why.is_empty() { "nowhere to download it from".into() } else { why.join("; ") })
}

/// An RPM package's lead (its first four bytes).
fn is_rpm(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xed, 0xab, 0xee, 0xdb])
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_kernel_argument_everywhere_the_command_line_is_kept() {
        let root = std::path::Path::new("/var/tmp").join(format!("olo-doctor-kargs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, t: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        w("boot/loader/entries/x-7.2.15.conf", "title Fedora\noptions root=UUID=1 ro rhgb quiet $tuned_params\ngrub_users $grub_users\n");
        w("boot/loader/entries/x-0-rescue.conf", "title Rescue\noptions root=UUID=1 ro\n");
        w("etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet\n");
        w("etc/default/grub", "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet xe.enable_dpcd_backlight=1\"\n");
        add_kernel_argument(&root, "xe.enable_dpcd_backlight=3").unwrap();
        // Twice is the same as once.
        add_kernel_argument(&root, "xe.enable_dpcd_backlight=3").unwrap();
        let r = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
        assert_eq!(r("boot/loader/entries/x-7.2.15.conf"), "title Fedora\noptions root=UUID=1 ro rhgb quiet $tuned_params xe.enable_dpcd_backlight=3\ngrub_users $grub_users\n");
        assert_eq!(r("boot/loader/entries/x-0-rescue.conf"), "title Rescue\noptions root=UUID=1 ro\n");
        assert_eq!(r("etc/kernel/cmdline"), "root=UUID=1 ro rhgb quiet xe.enable_dpcd_backlight=3\n");
        assert_eq!(r("etc/default/grub"), "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet xe.enable_dpcd_backlight=3\"\n");
        let _ = std::fs::remove_dir_all(&root);
    }
    use super::*;

    /// A release package from the first place that has one: a stale
    /// mirror's 404 and a page that isn't a package passed over, every
    /// answer said when nowhere has it (a local server for the places).
    #[test]
    fn a_release_package_comes_from_the_first_place_that_has_it() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in server.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).unwrap_or(0);
                let path = String::from_utf8_lossy(&buf[..n]).split_whitespace().nth(1).unwrap_or("").to_owned();
                let (status, body): (&str, &[u8]) = match path.as_str() {
                    "/good.rpm" => ("200 OK", &[0xed, 0xab, 0xee, 0xdb, 3, 0]),
                    "/page.rpm" => ("200 OK", b"<html>busy</html>"),
                    _ => ("404 Not Found", b""),
                };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(body);
            }
        });
        let at = |p: &str| format!("http://127.0.0.1:{port}/{p}");
        let (url, bytes) = release_package(&[at("stale.rpm"), at("page.rpm"), at("good.rpm")]).unwrap();
        assert_eq!((url, bytes.len()), (at("good.rpm"), 6));
        let why = release_package(&[at("stale.rpm"), at("page.rpm")]).unwrap_err();
        assert!(why.contains("answered 404") && why.contains("something other than a package"), "{why}");
    }

    #[test]
    fn steps_round_trip() {
        let steps = vec![
            Step::WriteFile { path: "/etc/x".into(), content: "a\n".into(), mode: 0o644 },
            Step::Unit { name: "keyd.service".into(), action: UnitAction::Disable },
            Step::Signal { pid: 4, signal: 15 },
        ];
        let back: Vec<Step> = serde_json::from_str(&serde_json::to_string(&steps).unwrap()).unwrap();
        assert_eq!(back, steps);
    }

    #[test]
    fn files_are_written_whole_and_removed() {
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-fix-test-{}", std::process::id()));
        let path = dir.join("rules.d/71.rules");
        Step::WriteFile { path: path.clone(), content: "x\n".into(), mode: 0o644 }.run().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x\n");
        assert!(std::fs::read_dir(path.parent().unwrap()).unwrap().count() == 1, "no temporary left behind");
        Step::RemoveFile { path: path.clone() }.run().unwrap();
        assert!(!path.exists());
        Step::RemoveFile { path }.run().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn signalling_a_gone_process_is_fine() {
        // No such pid (the largest allowed is far below), and a signal
        // 0 never changes anything.
        assert!(Step::Signal { pid: i32::MAX, signal: 0 }.run().is_ok());
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p olo-doctor -- --ignored rpm_fusion`: RPM Fusion's
    /// nonfree release package for Fedora 45, from where the fix looks.
    #[test]
    #[ignore = "the network"]
    fn rpm_fusion_s_release_package_is_there() {
        let file = "nonfree/fedora/rpmfusion-nonfree-release-45.noarch.rpm";
        let (url, bytes) = super::release_package(&[format!("https://download1.rpmfusion.org/{file}"), format!("https://mirrors.rpmfusion.org/{file}")]).unwrap();
        assert!(url.starts_with("https://download1.rpmfusion.org/") && bytes.len() > 4096, "{url}: {} bytes", bytes.len());
    }
}
