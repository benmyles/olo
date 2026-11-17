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
    /// Make this target what the computer starts to (a minimal Fedora
    /// starts to its console, `multi-user.target`, and a login screen
    /// needs `graphical.target`).
    MakeDefault,
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
    /// Write a value to a sysfs attribute. It lasts until the next
    /// restart (or the device's next plugging in): alone, only for what
    /// is meant to (applying now what a rule or a file written beside
    /// it does from then on).
    WriteSysfs {
        path: PathBuf,
        value: String,
    },
    /// Let a Thunderbolt device in, for good: boltd told to remember it
    /// (`olo_system::bolt::enroll`, by its unique id), which lets it in
    /// now and whenever it's plugged in; without boltd, or if it says
    /// no, the kernel's `authorized` for this once (`device`: its name
    /// in /sys/bus/thunderbolt/devices).
    AllowThunderbolt {
        device: String,
        uid: String,
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
    /// `root` is the file system's ("/"; a test's tree otherwise).
    AddKernelArgument {
        arg: String,
        root: PathBuf,
    },
    /// Start the kernel without `key` ("preempt": the argument of that
    /// name, whatever its value) from the next boot: taken out of every
    /// place `AddKernelArgument` puts one.
    RemoveKernelArgument {
        key: String,
        root: PathBuf,
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
    /// Remove installed packages by name (through PackageKit), with what
    /// was installed only for them.
    RemovePackages {
        names: Vec<String>,
    },
    /// Put PackageKit and dnf's service in place where they aren't (a
    /// minimal Fedora), with Fedora's own dnf: `packages::bootstrap`.
    InstallPackageServices,
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
    /// Start a Bluetooth controller's hardware again: `device` (its sysfs
    /// device) let go of by its `driver` (the driver's sysfs folder), reset
    /// if it can be (a PCI function reset), and taken up again.
    RestartBluetooth {
        device: PathBuf,
        driver: PathBuf,
    },
    /// Every running program's threads kept off processors `kept` (`on`;
    /// the kept core's, `olo_system::cores`), or let back on: as root,
    /// every process's (the kernel's own and realtime threads aside).
    KeepOffCores {
        kept: Vec<usize>,
        on: bool,
    },
    /// Start one of Olo's own programs (`olo-…`) with these arguments,
    /// detached, as the person (never as root): its own command line
    /// does the fix (`olo-diskbackup --now`).
    StartProgram {
        argv: Vec<String>,
    },
    /// Remove a build of Olo kept to fall back to (`tools/keep-build.sh`):
    /// its folder, `home`/.local/olo-kept/`name`, and its entry on the
    /// login screen, as `keep-build.sh --remove` does. Nothing else:
    /// `name` is one plain word and the folder not a link.
    RemoveKeptBuild {
        home: PathBuf,
        name: String,
    },
    /// Turn swap off: every swap area in use (`/proc/swaps`), each with
    /// swapoff(2), which brings what's in it back into memory first (and
    /// fails, the area left on, where there isn't the memory). For now
    /// only: what keeps it off from the next start goes beside it
    /// (zram-generator's file written, `FstabSwapOff`).
    SwapOff,
    /// Take the swap lines out of /etc/fstab (each commented, saying who
    /// did), so none is turned on at the next start.
    FstabSwapOff,
    /// Make a zram swap device again at the size its settings now give
    /// (zram-generator's, read at systemd's reload before this step):
    /// the swap on it turned off (`swapoff(2)`: what's in it comes back
    /// into memory first), the device reset, and its swap unit started,
    /// which sets the device up and turns the swap on; checked, the swap
    /// is on at the end. The kernel refuses a reset while anything has
    /// the device open, as udev does for a moment after its swap goes,
    /// so it's asked again for up to ten seconds: zram-generator's own
    /// reset (its unit's stop) asks once, and so failed on Ben's
    /// computer, 2026-11-16, leaving the device at its old size and no
    /// swap, its units failed.
    RemakeZramSwap {
        device: String,
    },
    /// Let every running program of account `uid` lock memory without
    /// limit (RLIMIT_MEMLOCK, `prlimit(2)` on each): what the login's
    /// limits file gives from the next login, given now, so a program
    /// started (or started again) from one of them has it without one.
    /// Until the account logs out.
    LetSessionLockMemory {
        uid: u32,
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
            Step::AllowThunderbolt { device, .. } => format!("Letting Thunderbolt device {device} in, now and from now on"),
            Step::RemoveFile { path } => format!("Removing {}", path.display()),
            Step::Unit { name, action } => {
                let verb = match action {
                    UnitAction::Start => "Starting",
                    UnitAction::Stop => "Stopping",
                    UnitAction::Enable => "Enabling",
                    UnitAction::Disable => "Disabling",
                    UnitAction::Reload => "Reloading systemd for",
                    UnitAction::Restart => "Restarting",
                    UnitAction::MakeDefault => "Starting the computer to",
                };
                format!("{verb} {name}")
            }
            Step::Signal { pid, .. } => format!("Stopping process {pid}"),
            Step::RestartGpu { address, .. } => format!("Starting the graphics card at {address} and its driver again"),
            Step::RestartBluetooth { device, .. } => {
                format!("Starting Bluetooth's hardware ({}) and its driver again", device.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
            }
            Step::AddShell { path } => format!("Listing {} in /etc/shells", path.display()),
            Step::AddKernelArgument { arg, .. } => format!("Starting the kernel with {arg}"),
            Step::RemoveKernelArgument { key, .. } => format!("Starting the kernel without {key}"),
            Step::TryShell { path } => format!("Trying {}", path.display()),
            Step::SetLoginShell { user, shell } => format!("Making {} {user}'s login shell", shell.display()),
            Step::InstallPackages { names } => format!("Installing {}", short_list(names)),
            Step::RemovePackages { names } => format!("Removing {}", short_list(names)),
            Step::InstallPackageServices => "Installing PackageKit and dnf's service".into(),
            Step::UpdatePackages { names } => format!("Updating {}", short_list(names)),
            Step::InstallRelease { name, release, .. } => format!("Installing {name} from github.com/{}", release.repo),
            Step::InstallRepository { name, .. } => format!("Adding the {name} repository"),
            Step::CopyTree { to, .. } => format!("Installing {}", to.display()),
            Step::MakeDir { path, .. } => format!("Making {}", path.display()),
            Step::MakeSystemUsers { config } => format!("Making the users {} names", config.display()),
            Step::SelinuxLabels { restore, .. } => format!("Labelling {} for SELinux", restore.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
            Step::RebuildInitramfs => "Making the start-up images again".into(),
            Step::StartProgram { argv } => format!("Running {}", argv.join(" ")),
            Step::RemoveKeptBuild { home, name } => format!("Removing the kept build {} and its login screen entry", home.join(KEPT_BUILDS).join(name).display()),
            Step::SwapOff => "Turning swap off".into(),
            Step::FstabSwapOff => "Taking swap out of /etc/fstab".into(),
            Step::LetSessionLockMemory { .. } => "Letting the programs running now keep their memory in RAM".into(),
            Step::RemakeZramSwap { device } => format!("Making the swap on /dev/{device} again at its new size"),
            Step::KeepOffCores { kept, on } => {
                let list = olo_system::cores::format_list(kept);
                if *on { format!("Moving the programs running on processor {list} to the others") } else { format!("Letting every program run on processor {list} again") }
            }
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
        // (A system with no PackageKit yet, a minimal Fedora: put in place
        // first, so a package step works there too.)
        if matches!(self, Step::InstallPackages { .. } | Step::RemovePackages { .. } | Step::UpdatePackages { .. } | Step::InstallRepository { .. } | Step::InstallPackageServices)
            && !olo_system::packages::missing_services().is_empty()
        {
            olo_system::packages::bootstrap(&mut said).map_err(|e| format!("{}: {e}", Step::InstallPackageServices.describe()))?;
        }
        match self {
            Step::InstallPackageServices => Ok(()),
            Step::InstallPackages { names } => {
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                olo_system::packages::install(&names, &mut said).map_err(|e| format!("{}: {e}", self.describe()))
            }
            Step::RemovePackages { names } => {
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                olo_system::packages::remove(&names, &mut said).map_err(|e| format!("{}: {e}", self.describe()))
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
            Step::RebuildInitramfs => rebuild_initramfs(progress).map_err(|e| format!("{}: {e}", self.describe())),
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
            Step::AllowThunderbolt { device, uid } => {
                let remembered = if uid.is_empty() { Err("it has no unique id".to_owned()) } else { olo_system::bolt::enroll(uid).map_err(|e| e.to_string()) };
                match remembered {
                    Ok(()) => Ok(()),
                    // (Not remembered: in for this once, and said.)
                    Err(why) => {
                        tracing::warn!("doctor: boltd didn't remember Thunderbolt device {device} ({why}): allowed until it's unplugged");
                        std::fs::write(Path::new("/sys/bus/thunderbolt/devices").join(device).join("authorized"), "1").map_err(failed)
                    }
                }
            }
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
            Step::RestartBluetooth { device, driver } => restart_bluetooth(device, driver),
            Step::KeepOffCores { kept, on } => {
                let swept = crate::kept_core::sweep_all(kept, *on);
                tracing::info!(processes = swept.processes, threads = swept.threads, failed = swept.failed, "{}", self.describe());
                Ok(())
            }
            Step::StartProgram { argv } => {
                // (Olo's own, as the person: never another program, never root.)
                // SAFETY: geteuid has no preconditions.
                if argv.first().is_none_or(|p| !p.starts_with("olo-") || p.contains('/')) || unsafe { libc::geteuid() } == 0 {
                    return Err(format!("{}: not one of Olo's programs, as you", self.describe()));
                }
                let argv: Vec<std::ffi::OsString> = argv.iter().map(Into::into).collect();
                olo_xdg::open::spawn(&argv).map_err(failed)
            }
            Step::RemoveKeptBuild { home, name } => remove_kept_build(home, name, SESSION_ENTRIES).map_err(failed),
            Step::SwapOff => swap_off(Path::new("/proc/swaps")).map_err(|e| format!("{}: {e}", self.describe())),
            Step::FstabSwapOff => fstab_swap_off(Path::new("/etc/fstab")).map_err(failed),
            Step::LetSessionLockMemory { uid } => let_lock_memory(Path::new("/proc"), *uid).map_err(|e| format!("{}: {e}", self.describe())),
            Step::RemakeZramSwap { device } => {
                let start = |unit: &str| olo_system::systemd::start_afresh(unit, std::time::Duration::from_secs(30)).map_err(|e| e.to_string());
                remake_zram_swap(Path::new("/"), device, std::time::Duration::from_secs(10), &start).map_err(|e| format!("{}: {e}", self.describe()))
            }
            Step::AddShell { path } => add_shell(path).map_err(failed),
            Step::AddKernelArgument { arg, root } => add_kernel_argument(root, arg).map_err(failed),
            Step::RemoveKernelArgument { key, root } => remove_kernel_argument(root, key).map_err(failed),
            Step::TryShell { path } => try_shell(path),
            Step::SetLoginShell { user, shell } => set_login_shell(user, shell).map_err(|e| format!("{}: {e}", self.describe())),
            Step::InstallPackages { .. } | Step::RemovePackages { .. } | Step::UpdatePackages { .. } | Step::InstallRelease { .. } | Step::InstallRepository { .. } | Step::InstallPackageServices => {
                self.run_with(&mut |_| {})
            }
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
            Step::RebuildInitramfs => rebuild_initramfs(&mut |_| {}).map_err(|e| format!("{}: {e}", self.describe())),
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
/// Where builds kept to fall back to are, in a home (`tools/keep-build.sh`).
pub const KEPT_BUILDS: &str = ".local/olo-kept";

/// Where a kept build's login screen entry may be (an image-based system's
/// /usr is read-only: /usr/local's).
const SESSION_ENTRIES: &[&str] = &["/usr/share/wayland-sessions", "/usr/local/share/wayland-sessions"];

/// `Step::RemoveKeptBuild`: `home`'s kept build `name` and its entry in
/// any of `sessions`, then the kept builds' folder if that was the last.
fn remove_kept_build(home: &Path, name: &str, sessions: &[&str]) -> std::io::Result<()> {
    let plain = !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain || !home.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("not a kept build: {name}")));
    }
    let kept = home.join(KEPT_BUILDS);
    let dir = kept.join(name);
    match std::fs::symlink_metadata(&dir) {
        // (`remove_dir_all` follows no link inside it.)
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&dir)?,
        Ok(_) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} isn't a folder", dir.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    for folder in sessions {
        match std::fs::remove_file(Path::new(folder).join(format!("olo-kept-{name}.desktop"))) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    // (The folder of them, if that was the last: as keep-build.sh leaves it.)
    let _ = std::fs::remove_dir(&kept);
    Ok(())
}

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
/// Every installed kernel's start-up image made again (dracut, the one
/// program this shells out to: the image is the distribution's to make),
/// telling how it goes from what dracut says as it goes (`--verbose`:
/// Fedora's dracut says only warnings without it): each kernel's image
/// as it begins and each module it takes in ("6.17.7: plymouth"), each
/// image done. Its last words say why, when it fails.
fn rebuild_initramfs(progress: &mut dyn FnMut(String)) -> Result<(), String> {
    use std::io::BufRead as _;
    let mut child = std::process::Command::new("dracut")
        .args(["--force", "--regenerate-all", "--verbose"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("dracut: {e}"))?;
    let mut last = String::new();
    let mut kernel = String::new();
    if let Some(err) = child.stderr.take() {
        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            if let Some(said) = initramfs_progress(&line, &mut kernel) {
                progress(said);
            }
            if !line.trim().is_empty() {
                last = line;
            }
        }
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("dracut: {}", last.trim().trim_start_matches("dracut").trim_start_matches(['[', 'F', 'E', ']', ':', ' ']))),
        Err(e) => Err(format!("dracut: {e}")),
    }
}

/// What a line of dracut's says of how it goes (`kernel`: the image
/// being made now, from its file's name).
fn initramfs_progress(line: &str, kernel: &mut String) -> Option<String> {
    // (Each kernel's run begins "Executing: /usr/bin/dracut --kver=…".)
    if line.contains("Executing:")
        && let Some(k) = line.split_whitespace().find_map(|w| w.strip_prefix("--kver="))
    {
        *kernel = k.to_owned();
        return Some(format!("{kernel}: starting"));
    }
    let said = line.split_once("*** ")?.1.trim_end().trim_end_matches(" ***");
    let of = |what: String| if kernel.is_empty() { what } else { format!("{kernel}: {what}") };
    if let Some(module) = said.strip_prefix("Including module: ") {
        return Some(of(format!("including {module}")));
    }
    if said.starts_with("Creating initramfs image file '") && said.ends_with(" done") {
        return Some(of("made".into()));
    }
    if said.starts_with("Creating image file '") {
        return Some(of("writing the image".into()));
    }
    None
}

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
/// As Olo's healer does it (apps/wifi-heal/src/bluetooth.rs, `reset`):
/// unbound, reset where it can be, bound again; BlueZ takes it up.
fn restart_bluetooth(device: &Path, driver: &Path) -> Result<(), String> {
    let id = device.file_name().ok_or("no device name")?.to_string_lossy().into_owned();
    std::fs::write(driver.join("unbind"), &id).map_err(|e| format!("letting go of {id}: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    if device.join("reset").exists()
        && let Err(e) = std::fs::write(device.join("reset"), "1")
    {
        tracing::info!("doctor: {id}'s reset: {e} (taken up again as it is)");
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::fs::write(driver.join("bind"), &id).map_err(|e| format!("taking up {id} again: {e}"))
}

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
/// The swap areas the kernel lists (`/proc/swaps`' text): each line's
/// device or file, after the heading's, and its size in bytes. (A space
/// in a name comes as the kernel writes it, `\040`: undone.)
pub fn swap_areas(swaps: &str) -> Vec<(String, u64)> {
    let unescaped = |name: &str| {
        let (mut out, mut rest) = (Vec::new(), name.as_bytes());
        while let Some((&b, tail)) = rest.split_first() {
            let octal = tail.get(..3).filter(|d| b == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)));
            match octal {
                Some(d) => {
                    out.push(d.iter().fold(0u32, |n, c| n * 8 + (c - b'0') as u32) as u8);
                    rest = &tail[3..];
                }
                None => {
                    out.push(b);
                    rest = tail;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    swaps
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let (name, _kind, kib) = (words.next()?, words.next()?, words.next()?.parse::<u64>().ok()?);
            Some((unescaped(name), kib * 1024))
        })
        .collect()
}

/// Every swap area in `swaps` (/proc/swaps) turned off. All are tried;
/// the first that couldn't be is the error.
fn swap_off(swaps: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(swaps).map_err(|e| e.to_string())?;
    let mut failed = None;
    for (area, _) in swap_areas(&text) {
        let Ok(path) = std::ffi::CString::new(area.as_bytes()) else { continue };
        // SAFETY: swapoff(2) with a NUL-terminated path.
        if unsafe { libc::swapoff(path.as_ptr()) } != 0 {
            let e = std::io::Error::last_os_error();
            failed.get_or_insert(format!("{area}: {e}"));
        }
    }
    failed.map_or(Ok(()), Err)
}

/// See `Step::RemakeZramSwap`. `root` is the file system's ("/"; a
/// test's tree), `patience` how long the device is asked to reset and
/// the swap waited for, `start` starts a system unit and waits for it.
fn remake_zram_swap(root: &Path, device: &str, patience: std::time::Duration, start: &dyn Fn(&str) -> Result<(), String>) -> Result<(), String> {
    if !device.starts_with("zram") || !device["zram".len()..].bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{device} isn't a zram device"));
    }
    let node = format!("/dev/{device}");
    let on = |root: &Path| std::fs::read_to_string(root.join("proc/swaps")).is_ok_and(|t| swap_areas(&t).iter().any(|(name, _)| *name == node));
    if on(root) {
        let path = std::ffi::CString::new(node.as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: swapoff(2) with a NUL-terminated path.
        if unsafe { libc::swapoff(path.as_ptr()) } != 0 {
            return Err(format!("its swap couldn't be turned off: {}", std::io::Error::last_os_error()));
        }
    }
    // Reset, once nothing has it open.
    let block = root.join("sys/block").join(device);
    let empty = |block: &Path| std::fs::read_to_string(block.join("disksize")).is_ok_and(|s| s.trim() == "0");
    let until = std::time::Instant::now() + patience;
    while !empty(&block) {
        match std::fs::write(block.join("reset"), "1") {
            Ok(()) => break,
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) && std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(format!("the device couldn't be reset: {e}")),
        }
    }
    // Its unit sets it up at the size its settings give, and turns it on.
    start(&format!("dev-{device}.swap"))?;
    let until = std::time::Instant::now() + patience;
    while !on(root) {
        if std::time::Instant::now() >= until {
            return Err(format!("the swap on {node} didn't come back (systemctl status dev-{device}.swap says why)"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

/// The processes of account `uid` under `proc` (/proc), by their ids.
pub fn processes_of(proc: &Path, uid: u32) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt as _;
    let mut pids: Vec<i32> =
        std::fs::read_dir(proc).into_iter().flatten().flatten().filter(|e| e.metadata().is_ok_and(|m| m.uid() == uid)).filter_map(|e| e.file_name().to_string_lossy().parse::<i32>().ok()).collect();
    pids.sort_unstable();
    pids
}

/// Every running process of `uid` let lock memory without limit. Those
/// that have ended meanwhile are passed over; the error is the first
/// other refusal, when none could be changed.
fn let_lock_memory(proc: &Path, uid: u32) -> Result<(), String> {
    let all = libc::rlimit64 { rlim_cur: libc::RLIM64_INFINITY, rlim_max: libc::RLIM64_INFINITY };
    let (mut changed, mut refused) = (0, None);
    for pid in processes_of(proc, uid) {
        // SAFETY: prlimit64(2) reads `all`; no old limit is asked for.
        if unsafe { libc::prlimit64(pid, libc::RLIMIT_MEMLOCK, &all, std::ptr::null_mut()) } == 0 {
            changed += 1;
        } else {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                refused.get_or_insert(e.to_string());
            }
        }
    }
    match refused {
        Some(e) if changed == 0 => Err(e),
        _ => Ok(()),
    }
}

/// `fstab`'s text with its swap lines commented out (None: it has none).
pub fn fstab_without_swap(fstab: &str) -> Option<String> {
    let is_swap = |l: &str| !l.trim_start().starts_with('#') && l.split_whitespace().nth(2) == Some("swap");
    if !fstab.lines().any(is_swap) {
        return None;
    }
    let mut out = String::with_capacity(fstab.len() + 64);
    for l in fstab.lines() {
        if is_swap(l) {
            out.push_str("# Olo Doctor turned swap off: ");
        }
        out.push_str(l);
        out.push('\n');
    }
    Some(out)
}

fn fstab_swap_off(fstab: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let text = std::fs::read_to_string(fstab)?;
    let Some(new) = fstab_without_swap(&text) else { return Ok(()) };
    let mode = std::fs::metadata(fstab).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    write_file(fstab, &new, mode)
}

fn add_shell(path: &Path) -> std::io::Result<()> {
    add_shell_to(Path::new("/etc/shells"), path)
}

/// `arg` ("key=value") in every place the kernel's command line is kept
/// under `root` (see `Step::AddKernelArgument`).
fn add_kernel_argument(root: &Path, arg: &str) -> std::io::Result<()> {
    edit_kernel_arguments(root, &|line| with_argument(line, arg))
}

/// `key`'s argument out of every place the kernel's command line is kept
/// under `root` (see `Step::RemoveKernelArgument`).
fn remove_kernel_argument(root: &Path, key: &str) -> std::io::Result<()> {
    edit_kernel_arguments(root, &|line| without_argument(line, key))
}

/// Every place the kernel's command line is kept under `root`, each as
/// `edit` makes it.
fn edit_kernel_arguments(root: &Path, edit: &dyn Fn(&str) -> String) -> std::io::Result<()> {
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
                Some(o) => format!("options {}", edit(o)),
                None => l.to_owned(),
            })
            .collect();
        let new = new.join("\n") + if text.ends_with('\n') { "\n" } else { "" };
        if new != text {
            write_file(&f, &new, mode(&f))?;
        }
    }
    // GRUB's defaults, then /etc/kernel/cmdline, in that order: Fedora's
    // kernel-install (20-grub.install) makes a new kernel's entry from
    // /etc/kernel/cmdline, unless that file is older than GRUB's
    // defaults, in which case it has grub2-mkconfig write it again from
    // them first. Both get the change, and the command line is left no
    // older than the defaults (unless it was before: someone's own edit
    // of the defaults, still to be taken up), so what a kernel update's
    // entry is made from is the file as written here.
    let (cmdline, grub) = (root.join("etc/kernel/cmdline"), root.join("etc/default/grub"));
    let written = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let older = |a: &Path, b: &Path| matches!((written(a), written(b)), (Some(a), Some(b)) if a < b);
    let was_older = older(&cmdline, &grub);
    if let Ok(text) = std::fs::read_to_string(&grub) {
        let new: Vec<String> = text
            .lines()
            .map(|l| match l.strip_prefix("GRUB_CMDLINE_LINUX=\"").and_then(|v| v.strip_suffix('"')) {
                Some(v) => format!("GRUB_CMDLINE_LINUX=\"{}\"", edit(v)),
                None => l.to_owned(),
            })
            .collect();
        let new = new.join("\n") + if text.ends_with('\n') { "\n" } else { "" };
        if new != text {
            write_file(&grub, &new, mode(&grub))?;
        }
    }
    if let Ok(text) = std::fs::read_to_string(&cmdline) {
        let new = format!("{}\n", edit(text.trim()));
        if new != text {
            write_file(&cmdline, &new, mode(&cmdline))?;
        }
        if !was_older
            && older(&cmdline, &grub)
            && let Some(at) = written(&grub)
        {
            std::fs::File::options().write(true).open(&cmdline)?.set_modified(at)?;
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

/// A command line without `key`'s argument (bare, or with any value).
fn without_argument(line: &str, key: &str) -> String {
    line.split_whitespace().filter(|t| t.split('=').next() != Some(key)).collect::<Vec<_>>().join(" ")
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
        UnitAction::MakeDefault => systemd::set_default(name),
    }
    .map_err(|e| e.to_string())?;
    // A unit that stands failed after its start didn't start, whatever
    // systemd said of being asked: said, so the fix isn't called done
    // (a swap was "made again" whose two units had both failed,
    // 2026-11-16).
    if matches!(action, UnitAction::Start | UnitAction::Restart) && systemd::failed(name) {
        return Err(format!("{name} failed to start (journalctl -u {name} says why)"));
    }
    Ok(())
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

    /// dracut's own words (Fedora 45's, `--verbose`) as the progress
    /// said under the finding.
    #[test]
    fn dracut_says_how_it_goes() {
        let mut kernel = String::new();
        let lines = [
            "dracut[I]: Executing: /usr/bin/dracut --kver=7.2.15-300.fc45.x86_64 --force --verbose",
            "dracut[I]: *** Including module: plymouth ***",
            "dracut[W]: Possible missing firmware for module",
            "dracut[I]: *** Creating image file '/boot/initramfs-7.2.15-300.fc45.x86_64.img' ***",
            "dracut[I]: *** Creating initramfs image file '/boot/initramfs-7.2.15-300.fc45.x86_64.img' done ***",
        ];
        let said: Vec<String> = lines.iter().filter_map(|l| super::initramfs_progress(l, &mut kernel)).collect();
        assert_eq!(said, ["7.2.15-300.fc45.x86_64: starting", "7.2.15-300.fc45.x86_64: including plymouth", "7.2.15-300.fc45.x86_64: writing the image", "7.2.15-300.fc45.x86_64: made",]);
    }

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
        // A kernel an update brings gets its entry from /etc/kernel/cmdline
        // unless that is older than GRUB's defaults: it isn't.
        let written = |p: &str| std::fs::metadata(root.join(p)).unwrap().modified().unwrap();
        assert!(written("etc/kernel/cmdline") >= written("etc/default/grub"));
        // (Only the defaults lacking it, the command line already right:
        // still not left older.)
        w("etc/default/grub", "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet\"\n");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(root.join("etc/kernel/cmdline")).unwrap().set_modified(past).unwrap();
        std::fs::File::options().write(true).open(root.join("etc/default/grub")).unwrap().set_modified(past).unwrap();
        add_kernel_argument(&root, "xe.enable_dpcd_backlight=3").unwrap();
        assert_eq!(r("etc/default/grub"), "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet xe.enable_dpcd_backlight=3\"\n");
        assert!(written("etc/kernel/cmdline") >= written("etc/default/grub"));
        // Someone's own edit of the defaults, not yet taken up (the
        // command line older than them), is left to be: still older.
        std::fs::File::options().write(true).open(root.join("etc/kernel/cmdline")).unwrap().set_modified(past).unwrap();
        add_kernel_argument(&root, "quiet").unwrap();
        assert!(written("etc/kernel/cmdline") < written("etc/default/grub"));
        // And out again, everywhere it was put; what's beside it stays.
        remove_kernel_argument(&root, "xe.enable_dpcd_backlight").unwrap();
        remove_kernel_argument(&root, "xe.enable_dpcd_backlight").unwrap();
        assert_eq!(r("boot/loader/entries/x-7.2.15.conf"), "title Fedora\noptions root=UUID=1 ro rhgb quiet $tuned_params\ngrub_users $grub_users\n");
        assert_eq!(r("boot/loader/entries/x-0-rescue.conf"), "title Rescue\noptions root=UUID=1 ro\n");
        assert_eq!(r("etc/kernel/cmdline"), "root=UUID=1 ro rhgb quiet\n");
        assert_eq!(r("etc/default/grub"), "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet\"\n");
        let _ = std::fs::remove_dir_all(&root);
    }
    use super::*;

    /// The kernel's list of swap areas read: each one's name (a space in
    /// it as the kernel writes it) and size; none in an empty list.
    #[test]
    fn swap_areas_read() {
        let swaps = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n/dev/zram0                              partition\t8388604\t\t6403060\t\t100\n/var/swap\\040file                          file\t\t1048572\t\t0\t\t-2\n";
        assert_eq!(swap_areas(swaps), [("/dev/zram0".to_owned(), 8388604 * 1024), ("/var/swap file".to_owned(), 1048572 * 1024)]);
        assert!(swap_areas("Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n").is_empty());
        assert!(swap_areas("").is_empty());
    }

    /// /etc/fstab's swap lines commented out, the rest as it was; one
    /// with none (or only ones already commented) left alone.
    #[test]
    fn fstab_loses_its_swap() {
        let fstab = "# /etc/fstab\nUUID=aa-bb / btrfs subvol=root,compress=zstd:1 0 0\nUUID=cc-dd none swap defaults 0 0\n/swapfile swap swap defaults,pri=5 0 0\n#UUID=ee none swap defaults 0 0\n";
        let new = fstab_without_swap(fstab).expect("changed");
        assert_eq!(
            new,
            "# /etc/fstab\nUUID=aa-bb / btrfs subvol=root,compress=zstd:1 0 0\n# Olo Doctor turned swap off: UUID=cc-dd none swap defaults 0 0\n# Olo Doctor turned swap off: /swapfile swap swap defaults,pri=5 0 0\n#UUID=ee none swap defaults 0 0\n"
        );
        assert_eq!(fstab_without_swap(&new), None);
        assert_eq!(fstab_without_swap("UUID=aa-bb / btrfs defaults 0 0\n"), None);
        // In a file: written back whole, with the mode it had.
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-fstab-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("fstab");
        std::fs::write(&file, fstab).unwrap();
        fstab_swap_off(&file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), new);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A zram swap made again: a device already reset is only started
    /// (its unit turns the swap on); one whose swap doesn't come back is
    /// said; and nothing but a zram device is touched.
    #[test]
    fn a_zram_swap_is_made_again() {
        let root = PathBuf::from(format!("/var/tmp/olo-doctor-remake-zram-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/block/zram0")).unwrap();
        std::fs::create_dir_all(root.join("proc")).unwrap();
        let heading = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";
        std::fs::write(root.join("proc/swaps"), heading).unwrap();
        std::fs::write(root.join("sys/block/zram0/disksize"), "0\n").unwrap();
        let soon = std::time::Duration::from_millis(300);
        // Its unit started, the swap there after: done.
        let started = std::cell::RefCell::new(Vec::new());
        let turns_on = |unit: &str| {
            started.borrow_mut().push(unit.to_owned());
            std::fs::write(root.join("proc/swaps"), format!("{heading}/dev/zram0 partition 65263080 0 100\n")).map_err(|e| e.to_string())
        };
        assert_eq!(remake_zram_swap(&root, "zram0", soon, &turns_on), Ok(()));
        assert_eq!(*started.borrow(), ["dev-zram0.swap"]);
        // A device still set up (and nothing of the test's to turn off):
        // reset first (the file written), then started.
        std::fs::write(root.join("proc/swaps"), heading).unwrap();
        std::fs::write(root.join("sys/block/zram0/disksize"), "8589934592\n").unwrap();
        assert_eq!(remake_zram_swap(&root, "zram0", soon, &turns_on), Ok(()));
        assert_eq!(std::fs::read_to_string(root.join("sys/block/zram0/reset")).unwrap(), "1");
        // Its unit started but no swap comes: said.
        std::fs::write(root.join("proc/swaps"), heading).unwrap();
        let e = remake_zram_swap(&root, "zram0", soon, &|_| Ok(())).unwrap_err();
        assert_eq!(e, "the swap on /dev/zram0 didn't come back (systemctl status dev-zram0.swap says why)");
        // Its unit can't be started: said as systemd says it.
        assert_eq!(remake_zram_swap(&root, "zram0", soon, &|_| Err("no such unit".into())), Err("no such unit".into()));
        // Not a zram device: refused.
        assert_eq!(remake_zram_swap(&root, "nvme0n1p3", soon, &turns_on), Err("nvme0n1p3 isn't a zram device".into()));
        assert_eq!(Step::RemakeZramSwap { device: "zram0".into() }.describe(), "Making the swap on /dev/zram0 again at its new size");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The account's processes are found by who owns them; and letting
    /// them lock memory is root's to do (this test isn't: refused, said).
    #[test]
    fn an_accounts_processes_let_lock_memory() {
        use std::os::unix::fs::MetadataExt as _;
        let me = std::fs::metadata("/proc/self").unwrap().uid();
        let mine = processes_of(Path::new("/proc"), me);
        assert!(mine.contains(&(std::process::id() as i32)));
        assert!(mine.windows(2).all(|w| w[0] < w[1]));
        // (pid 1 is root's.)
        assert_eq!(mine.contains(&1), me == 0);
        if me != 0 {
            let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: getrlimit writes `limit`.
            unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut limit) };
            // (Already without limit, as in a session it was given to:
            // nothing to refuse.)
            if limit.rlim_max != libc::RLIM_INFINITY {
                assert_eq!(let_lock_memory(Path::new("/proc"), me), Err("Operation not permitted (os error 1)".into()));
            }
        }
        assert_eq!(Step::LetSessionLockMemory { uid: 1000 }.describe(), "Letting the programs running now keep their memory in RAM");
    }

    /// Turning swap off where none is on does nothing, and says so
    /// (the kernel's list as a file: no real swap is touched).
    #[test]
    fn no_swap_to_turn_off() {
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-no-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let swaps = dir.join("swaps");
        std::fs::write(&swaps, "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n").unwrap();
        assert_eq!(swap_off(&swaps), Ok(()));
        // One that isn't a swap area (nor ours to turn off: not root):
        // refused by the kernel, and said.
        std::fs::write(&swaps, format!("Filename Type Size Used Priority\n{} file 4 0 -2\n", dir.join("none").display())).unwrap();
        assert!(swap_off(&swaps).is_err_and(|e| e.contains("none")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A kept build goes with its login entry, and only it: an odd name
    /// or a link is refused, a link's target left as it is.
    #[test]
    fn a_kept_build_and_its_entry_are_removed_and_nothing_else() {
        let root = PathBuf::from(format!("/var/tmp/olo-doctor-remove-kept-{}", std::process::id()));
        let (home, sessions) = (root.join("home"), root.join("sessions"));
        let kept = home.join(KEPT_BUILDS);
        std::fs::create_dir_all(kept.join("old/bin")).unwrap();
        std::fs::write(kept.join("old/bin/olo-session"), "x").unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("olo-kept-old.desktop"), "[Desktop Entry]").unwrap();
        std::fs::write(sessions.join("olo.desktop"), "[Desktop Entry]").unwrap();
        let s = sessions.to_str().unwrap();
        remove_kept_build(&home, "old", &[s]).unwrap();
        assert!(!kept.join("old").exists() && !sessions.join("olo-kept-old.desktop").exists());
        assert!(sessions.join("olo.desktop").exists(), "another entry was touched");
        assert!(!kept.exists(), "the kept builds' folder, empty, stays");
        // (Already gone: fine.)
        remove_kept_build(&home, "old", &[s]).unwrap();
        for bad in ["", "..", ".hidden", "a/b", "a b", "../home"] {
            assert!(remove_kept_build(&home, bad, &[s]).is_err(), "{bad:?} was taken");
        }
        // A link where a build would be: refused, its target kept.
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("precious"), "x").unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        std::os::unix::fs::symlink(&elsewhere, kept.join("link")).unwrap();
        assert!(remove_kept_build(&home, "link", &[s]).is_err());
        assert!(elsewhere.join("precious").exists());
        assert!(remove_kept_build(Path::new("relative"), "old", &[s]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

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
