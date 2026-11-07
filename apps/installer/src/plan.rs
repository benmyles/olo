//! What Setup does, in order, on a worker thread (it waits on the
//! network, PackageKit and disks): each stage told to the window as it
//! goes (`Event`), each one doing only what isn't done (a second run, or
//! one after a failure, picks up where the first stopped). What needs
//! root goes to Olo Doctor's privileged helper, this program run again
//! through pkexec (`--helper`), asked for once: the password is asked by
//! the desktop's own polkit agent (GNOME's, on a stock Fedora).

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use olo_doctor::{
    fix::Step,
    helper::{FromHelper, Helper, ToHelper, Way},
};

use crate::bundle;

/// What Olo needs to run, from Fedora's own repositories (what its
/// programs link, the services a session talks to, and the fonts it
/// doesn't carry): those not installed are, through PackageKit.
pub const PACKAGES: &[&str] = &[
    "mesa-dri-drivers",
    "mesa-vulkan-drivers",
    "vulkan-loader",
    "mesa-libEGL",
    "mesa-libgbm",
    "libseat",
    "libdisplay-info",
    "libinput",
    "libwacom",
    "pixman",
    "lua-libs",
    "libgudev",
    "pipewire",
    "pipewire-libs",
    "wireplumber",
    "xdg-desktop-portal",
    "polkit",
    "NetworkManager",
    "bluez",
    "upower",
    "accountsservice",
    "google-noto-sans-cjk-vf-fonts",
];

/// What the person chose (the Choose page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choices {
    /// Olo's own login screen in GDM's place.
    pub login_screen: bool,
    /// Olo's boot splash.
    pub boot_splash: bool,
    /// RPM Fusion's repositories (NVIDIA's driver, more codecs).
    pub rpm_fusion: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Choices { login_screen: false, boot_splash: false, rpm_fusion: false }
    }
}

impl Choices {
    /// These, and whatever `had` has: what Setup installed before is
    /// chosen again, so running it again updates it (the login screen
    /// runs copies of its own outside the home folder, which an update
    /// leaving it out kept old).
    pub fn and(self, had: Choices) -> Choices {
        Choices { login_screen: self.login_screen || had.login_screen, boot_splash: self.boot_splash || had.boot_splash, rpm_fusion: self.rpm_fusion || had.rpm_fusion }
    }

    /// Which of them this computer has, under `root` ("/"): Olo's login
    /// screen as its display manager, Olo's boot splash as Plymouth's
    /// theme, RPM Fusion's repositories.
    pub fn installed_under(root: &Path) -> Choices {
        let at = |p: &str| root.join(p.trim_start_matches('/'));
        let dm = std::fs::read_link(at("/etc/systemd/system/display-manager.service")).ok();
        let theme = std::fs::read_to_string(at("/etc/plymouth/plymouthd.conf")).unwrap_or_default();
        Choices {
            login_screen: dm.as_deref().and_then(Path::file_name).is_some_and(|n| n == "olo-login.service"),
            boot_splash: theme.lines().any(|l| l.trim() == "Theme=olo"),
            rpm_fusion: at("/etc/yum.repos.d/rpmfusion-free.repo").exists(),
        }
    }
}

/// What the window is told.
#[derive(Debug, Clone)]
pub enum Event {
    /// Stage `n` of `of` begins.
    Stage {
        n: usize,
        of: usize,
        title: String,
    },
    /// How the stage is going: its share done (0 to 1, if known) and what
    /// it's doing now.
    Progress {
        share: Option<f32>,
        text: String,
    },
    /// A line for the details.
    Note(String),
    /// Waiting for the administrator's password.
    Password,
    /// All done; `restart`: the login screen or boot splash changed, so a
    /// restart shows them.
    Done {
        version: String,
        restart: bool,
    },
    /// It stopped: what failed, and what to do about it.
    Failed {
        what: String,
        why: String,
        advice: String,
    },
    Stopped,
}

/// The computer as Setup found it.
#[derive(Debug, Clone, Default)]
pub struct Computer {
    pub fedora: Option<u32>,
    pub variant: String,
    pub atomic: bool,
    pub user: String,
    pub home: PathBuf,
    pub nvidia: bool,
    /// Free space in the home folder, in bytes.
    pub free: u64,
    /// The version of Olo installed already, if any.
    pub installed: Option<String>,
    /// What of the Choose page's this computer has already.
    pub had: Choices,
}

/// What this computer is.
pub fn look() -> Computer {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |k: &str| os.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|v| v.trim_matches('"').to_owned());
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let fedora = (field("ID").as_deref() == Some("fedora")).then(|| field("VERSION_ID").and_then(|v| v.parse().ok())).flatten();
    // SAFETY: statvfs into a zeroed struct.
    let free = unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        let p = std::ffi::CString::new(home.as_os_str().as_encoded_bytes()).unwrap_or_default();
        if libc::statvfs(p.as_ptr(), &mut s) == 0 { s.f_bavail as u64 * s.f_frsize } else { 0 }
    };
    let nvidia = std::fs::read_dir("/sys/bus/pci/devices").into_iter().flatten().flatten().any(|d| std::fs::read_to_string(d.path().join("vendor")).is_ok_and(|v| v.trim() == "0x10de"));
    Computer {
        fedora,
        variant: field("VARIANT").or_else(|| field("NAME")).unwrap_or_default(),
        atomic: Path::new("/run/ostree-booted").exists(),
        user: std::env::var("USER").unwrap_or_default(),
        installed: bundle::installed(&home),
        had: Choices::installed_under(Path::new("/")),
        home,
        nvidia,
        free,
    }
}

/// Why this computer can't have Olo installed by Setup, if it can't.
pub fn unsuitable(c: &Computer) -> Option<String> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return Some("Setup runs as you, not as root: it asks for the administrator's password when it needs it.".into());
    }
    match c.fedora {
        None => return Some("Olo installs on Fedora (44 or 45); this isn't Fedora.".into()),
        Some(v) if v < 44 => return Some(format!("Olo needs Fedora 44 or newer; this is Fedora {v}. Upgrade Fedora first.")),
        _ => {}
    }
    if c.atomic {
        return Some("This is an atomic Fedora (Silverblue, Kinoite): Setup installs on Fedora Workstation. tools/setup.sh in Olo's repository sets up an atomic one.".into());
    }
    if !matches!(bundle::arch(), "x86_64" | "aarch64") {
        return Some(format!("Olo is made for x86_64 and aarch64 computers; this is {}.", bundle::arch()));
    }
    // (The archive, unpacked, and its copy: about 6 GB at most.)
    if c.free < 6 << 30 {
        return Some(format!("Olo needs about 6 GB free in your home folder; there are {:.1} GB.", c.free as f64 / (1u64 << 30) as f64));
    }
    None
}

/// Where the login screen looks for sessions (an atomic system's /usr
/// can't be written; never used, Setup refusing those).
fn sessions_dir(c: &Computer) -> &'static str {
    if c.atomic { "/usr/local/share/wayland-sessions" } else { "/usr/share/wayland-sessions" }
}

/// The login screen's entry for Olo, pointing at the session in `bin`.
fn session_entry(template: &str, bin: &Path) -> String {
    template.lines().filter(|l| !l.starts_with("TryExec=")).map(|l| if l.starts_with("Exec=") { format!("Exec={}/olo-session", bin.display()) } else { l.to_owned() }).collect::<Vec<_>>().join("\n")
        + "\n"
}

/// Devices already plugged in that Olo's udev rules are for, told to be
/// looked at again (as `udevadm trigger` would, by their `uevent`).
fn triggers() -> Vec<Step> {
    let mut steps = Vec::new();
    for class in ["/sys/class/hidraw", "/sys/bus/usb/devices", "/sys/bus/iio/devices"] {
        for d in std::fs::read_dir(class).into_iter().flatten().flatten() {
            let uevent = d.path().join("uevent");
            if uevent.exists() {
                steps.push(Step::WriteSysfs { path: uevent, value: "change".into() });
            }
        }
    }
    steps
}

/// The root steps that put Olo on the login screen's menu and install its
/// udev rules (as tools/install-session.sh does), from the unpacked `olo`.
fn session_steps(c: &Computer, olo: &Path) -> Result<Vec<Step>, String> {
    let read = |p: PathBuf| std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()));
    let bin = c.home.join(".local/bin");
    let entry = session_entry(&read(olo.join("system/session/olo.desktop"))?, &bin);
    let here = sessions_dir(c);
    let mut steps = vec![Step::WriteFile { path: PathBuf::from(here).join("olo.desktop"), content: entry, mode: 0o644 }];
    for other in ["/usr/local/share/wayland-sessions", "/usr/share/wayland-sessions"] {
        if other != here && Path::new(other).join("olo.desktop").exists() {
            steps.push(Step::RemoveFile { path: PathBuf::from(other).join("olo.desktop") });
        }
    }
    for e in std::fs::read_dir(olo.join("system/udev")).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".rules") {
            steps.push(Step::WriteFile { path: PathBuf::from("/etc/udev/rules.d").join(&name), content: read(e.path())?, mode: 0o644 });
        }
    }
    // (Errors here only mean a device went meanwhile.)
    Ok(steps)
}

/// The root steps for Olo's boot splash, the theme made (as the person,
/// at their screen's scale) into `theme`.
fn boot_splash_steps(theme: &Path) -> Result<Vec<Step>, String> {
    let to = PathBuf::from("/usr/share/plymouth/themes/olo");
    let mut steps = vec![Step::CopyTree { from: theme.to_owned(), to, mode: 0o644 }];
    let conf = PathBuf::from("/etc/plymouth/plymouthd.conf");
    if conf.exists() && !conf.with_extension("conf.before-olo").exists() {
        steps.push(Step::CopyFile { from: conf.clone(), to: conf.with_extension("conf.before-olo"), mode: 0o644 });
    }
    steps.push(Step::WriteFile { path: conf, content: "# Olo's boot splash (Olo Setup).\n[Daemon]\nTheme=olo\nDeviceScale=1\n".into(), mode: 0o644 });
    steps.push(Step::RebuildInitramfs);
    Ok(steps)
}

/// The root helper, started the first time it's needed.
struct Root {
    helper: Option<(Helper, mpsc::Receiver<Option<FromHelper>>)>,
    next: u64,
}

impl Root {
    /// Run `steps` as root, telling how they go.
    fn run(&mut self, steps: Vec<Step>, tell: &dyn Fn(Event)) -> Result<(), String> {
        if steps.is_empty() {
            return Ok(());
        }
        if self.helper.is_none() {
            let way = match std::env::var("OLO_SETUP_HELPER").as_deref() {
                Ok("direct") => Way::Direct,
                _ => Way::Pkexec,
            };
            tell(Event::Password);
            let (tx, rx) = mpsc::channel();
            let h = Helper::start(way, move |m| _ = tx.send(m)).map_err(|e| format!("the administrator's helper couldn't start: {e}"))?;
            match rx.recv() {
                Ok(Some(FromHelper::Ready)) => {}
                _ => return Err("the administrator's password wasn't given".into()),
            }
            self.helper = Some((h, rx));
        }
        let (h, rx) = self.helper.as_ref().expect("started");
        self.next += 1;
        let id = self.next;
        let n = steps.len();
        h.send(&ToHelper::Run { id, steps });
        let mut begun = 0usize;
        loop {
            match rx.recv() {
                Ok(Some(FromHelper::Progress { id: i, text })) if i == id => {
                    // (Each step said as it begins; then what it says as it goes.)
                    if !text.contains(": ") {
                        begun += 1;
                    }
                    tell(Event::Progress { share: Some(begun.saturating_sub(1) as f32 / n as f32), text });
                }
                Ok(Some(FromHelper::Done { id: i, results })) if i == id => {
                    return match results.into_iter().find_map(Result::err) {
                        Some(e) => Err(e),
                        None => Ok(()),
                    };
                }
                Ok(Some(_)) => continue,
                _ => return Err("the administrator's helper stopped".into()),
            }
        }
    }
}

/// Install Olo: `choices`, the archive `given` (else the latest
/// release), telling `tell` how it goes; stopped when `stop` is set.
pub fn install(choices: Choices, given: Option<String>, stop: Arc<AtomicBool>, tell: impl Fn(Event) + Send + Sync + 'static) {
    let tell: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(tell);
    let c = look();
    let fail = |what: &str, why: String, advice: &str| Event::Failed { what: what.to_owned(), why, advice: advice.to_owned() };
    let mut stages = vec!["Checking this computer", "Getting what Olo needs", "Downloading Olo", "Unpacking Olo", "Copying files", "Adding Olo to the login screen"];
    if choices.rpm_fusion {
        stages.insert(2, "Adding RPM Fusion");
    }
    if choices.login_screen {
        stages.push("Installing Olo's login screen");
    }
    if choices.boot_splash {
        stages.push("Installing Olo's boot splash");
    }
    let of = stages.len();
    let mut n = 0;
    let mut stage = |title: &str| {
        n += 1;
        tell(Event::Stage { n, of, title: title.to_owned() });
    };
    let stopped = || stop.load(Ordering::Relaxed);
    let mut root = Root { helper: None, next: 0 };
    let say = |e: Event| tell(e);

    stage(stages[0]);
    if let Some(why) = unsuitable(&c) {
        return say(fail("This computer", why, "Olo Setup installs on Fedora Workstation 44 or newer."));
    }
    say(Event::Note(format!("Fedora {} {}, {}; {:.0} GB free.", c.fedora.unwrap_or(0), c.variant, bundle::arch(), c.free as f64 / (1u64 << 30) as f64)));

    stage("Getting what Olo needs");
    let mut packages: Vec<String> = PACKAGES.iter().map(|s| (*s).to_owned()).collect();
    if choices.boot_splash {
        packages.push("plymouth-plugin-two-step".into());
    }
    if let Err(e) = root.run(vec![Step::InstallPackages { names: packages }], &say) {
        return say(fail("Getting what Olo needs", e, "Check the network connection, then try again. If Fedora's software was updating, let it finish first."));
    }
    if stopped() {
        return say(Event::Stopped);
    }

    if choices.rpm_fusion {
        stage("Adding RPM Fusion");
        let v = c.fedora.unwrap_or(0);
        let repo = |kind: &str| {
            let file = format!("{kind}/fedora/rpmfusion-{kind}-release-{v}.noarch.rpm");
            Step::InstallRepository { name: format!("RPM Fusion ({kind})"), urls: vec![format!("https://download1.rpmfusion.org/{file}"), format!("https://mirrors.rpmfusion.org/{file}")] }
        };
        if let Err(e) = root.run(vec![repo("free"), repo("nonfree")], &say) {
            return say(fail("Adding RPM Fusion", e, "RPM Fusion's servers may be busy: try again in a while, or go on without it (Olo works without)."));
        }
    }

    stage("Downloading Olo");
    let source = match bundle::find(given.as_deref()) {
        Ok(s) => s,
        Err(e) => return say(fail("Finding Olo", e, "Check the network connection, then try again.")),
    };
    say(Event::Note(format!("Olo: {}", source.name)));
    let mut last = -1i64;
    let archive = bundle::fetch(&source, &stop, &mut |got, total| {
        let mb = (got >> 20) as i64;
        if mb != last {
            last = mb;
            let share = (total > 0).then(|| got as f32 / total as f32);
            let text = if total > 0 { format!("{} MB of {} MB", got >> 20, total >> 20) } else { format!("{} MB", got >> 20) };
            say(Event::Progress { share, text });
        }
    });
    let archive = match archive {
        Ok(a) => a,
        Err(_) if stopped() => return say(Event::Stopped),
        Err(e) => return say(fail("Downloading Olo", e, "Check the network connection, then try again: what has come so far is kept.")),
    };

    stage("Unpacking Olo");
    let staging = bundle::cache_dir().join("unpacked");
    let tell_unpack = {
        let mut last = -1i64;
        let tell = tell.clone();
        move |done: u64, total: u64| {
            let percent = if total > 0 { (done * 100 / total) as i64 } else { 0 };
            if percent != last {
                last = percent;
                tell(Event::Progress { share: (total > 0).then(|| done as f32 / total as f32), text: format!("{percent}% unpacked") });
            }
        }
    };
    let olo = match bundle::unpack(&archive, &staging, &stop, Box::new(tell_unpack)) {
        Ok(o) => o,
        Err(_) if stopped() => return say(Event::Stopped),
        Err(e) => return say(fail("Unpacking Olo", e, "Try again: if the archive was damaged, it's downloaded again.")),
    };
    let version = bundle::version_of(&olo);

    stage("Copying files");
    let mut last = 0u64;
    if let Err(e) = bundle::place(&olo, &c.home, &mut |done, total| {
        if done == total || done >= last + 25 {
            last = done;
            say(Event::Progress { share: Some(done as f32 / total.max(1) as f32), text: format!("{done} of {total} files") });
        }
    }) {
        return say(fail("Copying files", e, "Make some room in your home folder, then try again."));
    }

    stage("Adding Olo to the login screen");
    let steps = match session_steps(&c, &olo) {
        Ok(mut s) => {
            s.extend(triggers());
            s
        }
        Err(e) => return say(fail("Adding Olo to the login screen", e, "The archive is missing a part: try again.")),
    };
    if let Err(e) = root.run(steps, &say) {
        // (A device unplugged meanwhile is no failure.)
        if !e.contains("uevent") {
            return say(fail("Adding Olo to the login screen", e, "Try again."));
        }
    }

    let mut restart = false;
    if choices.login_screen {
        stage("Installing Olo's login screen");
        // (Steps shared with Olo Doctor's "Use Olo's Login Screen", in
        // place of whichever display manager the system starts.)
        let instead = olo_doctor::login_screen::display_manager(Path::new("/")).or_else(|| Some("gdm.service".into()));
        let steps = olo_doctor::login_screen::steps(&olo.join("bin"), &olo.join("share/olo"), instead.as_deref());
        match root.run(steps, &say) {
            Ok(()) => restart = true,
            Err(e) => return say(fail("Installing Olo's login screen", e, "Try again, or leave it out: Olo is on GDM's session menu either way.")),
        }
    }
    if choices.boot_splash {
        stage("Installing Olo's boot splash");
        let theme = bundle::cache_dir().join("plymouth-theme");
        let _ = std::fs::remove_dir_all(&theme);
        let made = std::process::Command::new(c.home.join(".local/bin/olo-app-server")).arg("--plymouth-theme").arg(&theme).status();
        match made {
            Ok(s) if s.success() => {}
            _ => return say(fail("Installing Olo's boot splash", "Olo's boot splash couldn't be drawn".into(), "Try again, or leave it out.")),
        }
        match boot_splash_steps(&theme).and_then(|s| root.run(s, &say)) {
            Ok(()) => restart = true,
            Err(e) => return say(fail("Installing Olo's boot splash", e, "Try again, or leave it out.")),
        }
    }
    // The next login Olo's (GDM chooses the session the person last
    // chose; its gear still offers GNOME).
    match default_session("olo") {
        Ok(()) => say(Event::Note("Olo is your session from your next login.".into())),
        Err(e) => say(Event::Note(format!("Olo couldn't be made your session ({e}): choose it from the login screen's gear."))),
    }
    let _ = std::fs::remove_dir_all(&staging);
    say(Event::Done { version, restart });
}

/// Make `session` the person's session at the next login, as the login
/// screen's own menu does (AccountsService's `SetSession`).
fn default_session(session: &str) -> Result<(), String> {
    // (Never in a test run on someone's own computer.)
    if std::env::var_os("OLO_SETUP_HELPER").is_some() {
        return Err("a test run".into());
    }
    let conn = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
    let user = std::env::var("USER").map_err(|_| "no user name".to_owned())?;
    let path: zbus::zvariant::OwnedObjectPath = conn
        .call_method(Some("org.freedesktop.Accounts"), "/org/freedesktop/Accounts", Some("org.freedesktop.Accounts"), "FindUserByName", &(user,))
        .and_then(|r| r.body().deserialize())
        .map_err(|e| e.to_string())?;
    conn.call_method(Some("org.freedesktop.Accounts"), path.as_str(), Some("org.freedesktop.Accounts.User"), "SetSession", &(session,)).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run again, Setup chooses what it installed before (and updates it).
    #[test]
    fn what_was_installed_is_chosen_again() {
        let root = std::env::temp_dir().join(format!("olo-setup-had-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(Choices::installed_under(&root), Choices::default());
        std::fs::create_dir_all(root.join("etc/systemd/system")).unwrap();
        std::fs::create_dir_all(root.join("etc/plymouth")).unwrap();
        std::fs::create_dir_all(root.join("etc/yum.repos.d")).unwrap();
        // GDM's: not Olo's login screen.
        std::os::unix::fs::symlink("/usr/lib/systemd/system/gdm.service", root.join("etc/systemd/system/display-manager.service")).unwrap();
        std::fs::write(root.join("etc/plymouth/plymouthd.conf"), "[Daemon]\nTheme=bgrt\n").unwrap();
        assert_eq!(Choices::installed_under(&root), Choices::default());
        std::fs::remove_file(root.join("etc/systemd/system/display-manager.service")).unwrap();
        std::os::unix::fs::symlink("/etc/systemd/system/olo-login.service", root.join("etc/systemd/system/display-manager.service")).unwrap();
        std::fs::write(root.join("etc/plymouth/plymouthd.conf"), "# Olo's boot splash (Olo Setup).\n[Daemon]\nTheme=olo\nDeviceScale=1\n").unwrap();
        std::fs::write(root.join("etc/yum.repos.d/rpmfusion-free.repo"), "").unwrap();
        let had = Choices::installed_under(&root);
        assert_eq!(had, Choices { login_screen: true, boot_splash: true, rpm_fusion: true });
        assert_eq!(Choices::default().and(had), had);
        assert_eq!(Choices { login_screen: true, ..Choices::default() }.and(Choices::default()).login_screen, true);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tmpfiles_file_s_folders() {
        let steps = olo_doctor::login_screen::folders("# a comment\nd /var/lib/olo-greeter 0750 olo-greeter olo-greeter -\nd /run/olo-login 0755 root root -\nL /x - - - - /y\n");
        assert_eq!(
            steps,
            [Step::MakeDir { path: "/var/lib/olo-greeter".into(), mode: 0o750, owner: Some("olo-greeter".into()) }, Step::MakeDir { path: "/run/olo-login".into(), mode: 0o755, owner: None },]
        );
    }

    #[test]
    fn the_session_entry_names_the_person_s_session() {
        let e = session_entry("[Desktop Entry]\nName=Olo Desktop\nExec=olo-session\nTryExec=olo-session\n", Path::new("/home/u/.local/bin"));
        assert_eq!(e, "[Desktop Entry]\nName=Olo Desktop\nExec=/home/u/.local/bin/olo-session\n");
    }
}
