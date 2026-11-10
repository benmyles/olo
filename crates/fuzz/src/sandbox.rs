//! The fuzzers' sandbox: every run of olo-fuzz happens inside one, with
//! its own environment and nothing of the machine's files but what it
//! needs to run, read-only. The fuzzers press, drag and type into real
//! apps: a drag that lands in a Tracker window instead of on a seam moves
//! the files it shows (it once tried to move /usr/share/backgrounds), so
//! they must never see the person's.
//!
//! Three processes: this one stays outside and waits; its child makes
//! new user, mount, PID, network and IPC namespaces (no root needed);
//! the child's child is the namespace's first process (PID 1, an init):
//! it builds the sandbox's root, runs olo-fuzz again inside it and waits
//! for every process left to it. When the init ends, the kernel kills
//! whatever is still in the namespace, and each of the three dies with
//! its parent (`PR_SET_PDEATHSIG`): killed outright, a run leaves nothing
//! running.
//!
//! Inside, read-only: /usr, /etc (and the /bin, /lib links), a sysfs of
//! its own (nothing mounted under it: no cgroups), the GPU's device nodes and
//! /dev's harmless ones, fontconfig's cache, and of the repository only
//! `target/release` (and `target/debug`), `data` and `tools`; busd for
//! the sessions' buses; with `--chrome`, the installed Chrome. Its own:
//! /proc, ptys (a devpts of its own), /run with its runtime directory,
//! /dev/shm, and a home, /tmp and /var/tmp backed by a folder on disk
//! (`/var/tmp/olo-fuzz-sandbox-PID`, removed after); the one place shared
//! with the machine is /var/tmp/olo-fuzz, where results go. No network
//! (a network namespace with nothing in it). The environment is a short
//! list made afresh (no WAYLAND_DISPLAY, no D-Bus address, no agent
//! sockets of the person's).

use std::{
    ffi::{CString, OsStr},
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

use crate::session::root;

/// Set in the environment of olo-fuzz run inside the sandbox.
pub const INSIDE: &str = "OLO_FUZZ_SANDBOX";

/// Where results go, the one folder shared with the machine.
pub const OUT: &str = "/var/tmp/olo-fuzz";

pub fn inside() -> bool {
    std::env::var_os(INSIDE).is_some()
}

/// What the sandbox is made of, found outside it.
struct Plan {
    /// The folder on disk behind its home, /tmp and /var/tmp.
    backing: PathBuf,
    uid: u32,
    gid: u32,
    /// The person's home's path (the sandbox's home is at the same path,
    /// a different folder).
    home: PathBuf,
    repo: PathBuf,
    busd: Option<PathBuf>,
    /// The installed Chrome's folder (its version's), for `--chrome`.
    chrome: Option<PathBuf>,
    user: String,
}

/// Run olo-fuzz with `args` in a sandbox; its exit code. Called first
/// thing (before any thread starts: the namespaces want one thread).
pub fn run(args: &[String]) -> i32 {
    let plan = match plan(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("olo-fuzz: no sandbox: {e}");
            return 2;
        }
    };
    let me = std::process::id() as libc::pid_t;
    // SAFETY: fork with one thread; the child runs on.
    let child = unsafe { libc::fork() };
    if child < 0 {
        eprintln!("olo-fuzz: fork: {}", std::io::Error::last_os_error());
        return 2;
    }
    if child == 0 {
        // SAFETY: the child's own process group (a terminal's Ctrl-C is
        // the parent's to pass on, once), and death with the parent.
        unsafe {
            libc::setpgid(0, 0);
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            if libc::getppid() != me {
                libc::_exit(1);
            }
        }
        // The first to go if memory runs out, all of the sandbox (it's
        // inherited): never the person's programs. A sweep that took more
        // than there was had the kernel kill Ben's Slack.
        let _ = std::fs::write("/proc/self/oom_score_adj", "1000");
        let code = namespaces(&plan, args);
        // SAFETY: the child ends here.
        unsafe { libc::_exit(code) };
    }
    forward_signals(child);
    let code = wait_for(child);
    let _ = std::fs::remove_dir_all(&plan.backing);
    code
}

fn plan(args: &[String]) -> Result<Plan, String> {
    // Folders a run killed before it could remove them.
    if let Ok(entries) = std::fs::read_dir("/var/tmp") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(pid) = name.strip_prefix("olo-fuzz-sandbox-").and_then(|p| p.parse::<u32>().ok())
                && !Path::new(&format!("/proc/{pid}")).exists()
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let backing = PathBuf::from(format!("/var/tmp/olo-fuzz-sandbox-{}", std::process::id()));
    for d in ["root", "home", "tmp", "vartmp"] {
        std::fs::create_dir_all(backing.join(d)).map_err(|e| format!("{}: {e}", backing.display()))?;
    }
    std::fs::create_dir_all(OUT).map_err(|e| format!("{OUT}: {e}"))?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?);
    let busd = std::env::var_os("TSESSION_BUSD")
        .map(PathBuf::from)
        .into_iter()
        .chain(["olo-dev", "tabula-dev"].iter().map(|d| home.join(format!(".cache/{d}/busd-lenient/busd-0.5.0/target/release/busd"))))
        .chain(std::iter::once(home.join(".cargo/bin/busd")))
        .find(|p| p.is_file());
    let chrome = args.iter().any(|a| a == "--chrome").then(|| std::fs::canonicalize(home.join(".local/share/olo/chrome/current")).ok()).flatten();
    Ok(Plan {
        backing,
        // SAFETY: plain getters.
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        home,
        repo: root(),
        busd,
        chrome,
        user: std::env::var("USER").unwrap_or_else(|_| "fuzz".into()),
    })
}

/// The child: new namespaces, then the init in them. Its exit code.
fn namespaces(plan: &Plan, args: &[String]) -> i32 {
    let flags = libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWPID | libc::CLONE_NEWNET | libc::CLONE_NEWIPC;
    // SAFETY: unshare with one thread.
    if unsafe { libc::unshare(flags) } != 0 {
        eprintln!("olo-fuzz: no sandbox: unshare: {} (unprivileged user namespaces off?)", std::io::Error::last_os_error());
        return 2;
    }
    // The person's own ids inside, and no others.
    let maps = [("setgroups", "deny".to_owned()), ("uid_map", format!("{0} {0} 1", plan.uid)), ("gid_map", format!("{0} {0} 1", plan.gid))];
    for (file, text) in maps {
        if let Err(e) = std::fs::write(format!("/proc/self/{file}"), text) {
            eprintln!("olo-fuzz: no sandbox: {file}: {e}");
            return 2;
        }
    }
    // SAFETY: fork with one thread: the child is PID 1 of the new namespace.
    let init = unsafe { libc::fork() };
    if init < 0 {
        eprintln!("olo-fuzz: fork: {}", std::io::Error::last_os_error());
        return 2;
    }
    if init == 0 {
        // SAFETY: death with the parent. (Its parent is outside the PID
        // namespace: getppid() says 0, so it can't check it's still there;
        // the parent waits for it, and dies only killed, which kills this
        // once the death signal is set.)
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
        let code = match build(plan) {
            Ok(()) => init_run(plan, args),
            Err(e) => {
                eprintln!("olo-fuzz: no sandbox: {e}");
                2
            }
        };
        // SAFETY: the init ends here, and the namespace with it.
        unsafe { libc::_exit(code) };
    }
    forward_signals(init);
    wait_for(init)
}

/// The init: olo-fuzz again inside, every orphan waited for, its code.
fn init_run(plan: &Plan, args: &[String]) -> i32 {
    let runtime = format!("/run/user/{}", plan.uid);
    let mut env: Vec<(String, String)> = vec![
        (INSIDE.into(), "1".into()),
        ("HOME".into(), plan.home.display().to_string()),
        ("USER".into(), plan.user.clone()),
        ("LOGNAME".into(), plan.user.clone()),
        ("PATH".into(), format!("{}:/usr/local/bin:/usr/bin:/bin", plan.repo.join("target/release").display())),
        ("XDG_RUNTIME_DIR".into(), runtime),
    ];
    for keep in ["LANG", "LC_ALL", "LC_CTYPE", "TERM", "RUST_LOG", "RUST_BACKTRACE", "RUST_LIB_BACKTRACE", "TZ", "NO_COLOR", "WAYLAND_DEBUG"] {
        if let Ok(v) = std::env::var(keep) {
            env.push((keep.into(), v));
        }
    }
    if let Some(b) = &plan.busd {
        env.push(("TSESSION_BUSD".into(), b.display().to_string()));
    }
    // (By /proc/self/exe: the path it ran from may not be in here.)
    let exe = CString::new("/proc/self/exe").expect("no NUL");
    let argv: Vec<CString> = std::iter::once("olo-fuzz".to_owned()).chain(args.iter().cloned()).filter_map(|a| CString::new(a).ok()).collect();
    let envp: Vec<CString> = env.iter().filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok()).collect();
    let mut argv_p: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    argv_p.push(std::ptr::null());
    let mut envp_p: Vec<*const libc::c_char> = envp.iter().map(|a| a.as_ptr()).collect();
    envp_p.push(std::ptr::null());
    // SAFETY: fork with one thread; the child only execs (or exits).
    let main = unsafe { libc::fork() };
    if main < 0 {
        eprintln!("olo-fuzz: fork: {}", std::io::Error::last_os_error());
        return 2;
    }
    if main == 0 {
        // SAFETY: argv and envp NULL-terminated and alive till exec.
        unsafe {
            libc::execve(exe.as_ptr(), argv_p.as_ptr(), envp_p.as_ptr());
            libc::_exit(127);
        }
    }
    forward_signals(main);
    let mut code = 1;
    loop {
        let mut status = 0;
        // SAFETY: waits for any child of the init's (orphans come to it).
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid == main {
            code = exit_code(status);
            break;
        }
        if pid < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
    code
}

fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

fn wait_for(pid: libc::pid_t) -> i32 {
    loop {
        let mut status = 0;
        // SAFETY: waits for our child.
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return exit_code(status);
        }
        if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return 1;
        }
    }
}

/// Where Ctrl-C, SIGTERM and SIGHUP go: on to the next process in.
static FORWARD_TO: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn forward(sig: libc::c_int) {
    let to = FORWARD_TO.load(std::sync::atomic::Ordering::Relaxed);
    if to > 0 {
        // SAFETY: kill(2) is async-signal-safe.
        unsafe { libc::kill(to, sig) };
    }
}

fn forward_signals(to: libc::pid_t) {
    FORWARD_TO.store(to, std::sync::atomic::Ordering::Relaxed);
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: a handler that only forwards the signal.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = forward as *const () as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

// ---- the root -------------------------------------------------------------

fn c(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes()).expect("no NUL in a path")
}

fn err(what: &str, path: &Path) -> String {
    format!("{what} {}: {}", path.display(), std::io::Error::last_os_error())
}

fn mount(src: Option<&Path>, dst: &Path, fstype: Option<&str>, flags: libc::c_ulong, data: Option<&str>) -> Result<(), String> {
    let src = src.map(c);
    let fstype = fstype.map(|f| CString::new(f).expect("no NUL"));
    let data = data.map(|d| CString::new(d).expect("no NUL"));
    // SAFETY: NUL-terminated strings alive for the call.
    let r = unsafe {
        libc::mount(
            src.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            c(dst).as_ptr(),
            fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            flags,
            data.as_ref().map_or(std::ptr::null(), |s| s.as_ptr().cast()),
        )
    };
    if r != 0 { Err(err("mount", dst)) } else { Ok(()) }
}

/// `src` at `dst`, read-only (keeping the flags the mount had: a user
/// namespace may not drop them).
fn bind_ro(src: &Path, dst: &Path) -> Result<(), String> {
    bind(src, dst)?;
    // SAFETY: statvfs of a path, into a zeroed struct.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c(dst).as_ptr(), &mut st) } != 0 {
        return Err(err("statvfs", dst));
    }
    let f = st.f_flag;
    let mut keep: libc::c_ulong = 0;
    for (st_flag, ms) in [
        (libc::ST_NOSUID, libc::MS_NOSUID),
        (libc::ST_NODEV, libc::MS_NODEV),
        (libc::ST_NOEXEC, libc::MS_NOEXEC),
        (libc::ST_NOATIME, libc::MS_NOATIME),
        (libc::ST_NODIRATIME, libc::MS_NODIRATIME),
        (libc::ST_RELATIME, libc::MS_RELATIME),
    ] {
        if f & st_flag != 0 {
            keep |= ms;
        }
    }
    mount(None, dst, None, libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | keep, None)
}

/// `src` at `dst`, as it is (not what's mounted under it).
fn bind(src: &Path, dst: &Path) -> Result<(), String> {
    if src.is_dir() {
        std::fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    } else {
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    mount(Some(src), dst, None, libc::MS_BIND, None)
}

fn dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Build the root in `backing/root` and move into it.
fn build(plan: &Plan) -> Result<(), String> {
    let new = plan.backing.join("root");
    let at = |p: &str| new.join(p.trim_start_matches('/'));
    // Nothing done here reaches the machine's mounts.
    mount(None, Path::new("/"), None, libc::MS_REC | libc::MS_PRIVATE, None)?;
    mount(Some(Path::new("tmpfs")), &new, Some("tmpfs"), libc::MS_NOSUID | libc::MS_NODEV, Some("mode=0755,size=16m"))?;
    for system in ["/usr", "/etc"] {
        bind_ro(Path::new(system), &at(system))?;
    }
    // Fedora's /bin, /sbin, /lib, /lib64 are links into /usr.
    for top in ["/bin", "/sbin", "/lib", "/lib64"] {
        let p = Path::new(top);
        match std::fs::read_link(p) {
            Ok(to) => std::os::unix::fs::symlink(to, at(top)).map_err(|e| format!("{top}: {e}"))?,
            Err(_) if p.is_dir() => bind_ro(p, &at(top))?,
            Err(_) => {}
        }
    }
    // A sysfs of its own, read-only: the devices (the GPUs' for Vulkan),
    // nothing mounted under it (cgroups, debugfs). (A bind of the
    // machine's would have to take those too.)
    dir(&at("/sys"))?;
    mount(Some(Path::new("sysfs")), &at("/sys"), Some("sysfs"), libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None)?;
    dir(&at("/proc"))?;
    mount(Some(Path::new("proc")), &at("/proc"), Some("proc"), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None)?;
    // /dev: the harmless nodes, the GPUs', and its own ptys and shm.
    let dev = at("/dev");
    dir(&dev)?;
    mount(Some(Path::new("tmpfs")), &dev, Some("tmpfs"), libc::MS_NOSUID | libc::MS_NOEXEC, Some("mode=0755,size=1m"))?;
    for node in ["null", "zero", "full", "random", "urandom"] {
        bind(&Path::new("/dev").join(node), &dev.join(node))?;
    }
    if Path::new("/dev/dri").is_dir() {
        bind(Path::new("/dev/dri"), &dev.join("dri"))?;
    }
    for e in std::fs::read_dir("/dev").map_err(|e| format!("/dev: {e}"))?.flatten() {
        let name = e.file_name();
        if name.as_bytes().starts_with(b"nvidia") {
            bind(&e.path(), &dev.join(&name))?;
        }
    }
    dir(&dev.join("pts"))?;
    mount(Some(Path::new("devpts")), &dev.join("pts"), Some("devpts"), libc::MS_NOSUID | libc::MS_NOEXEC, Some("newinstance,ptmxmode=0666,mode=0620"))?;
    std::os::unix::fs::symlink("pts/ptmx", dev.join("ptmx")).map_err(|e| format!("/dev/ptmx: {e}"))?;
    dir(&dev.join("shm"))?;
    mount(Some(Path::new("tmpfs")), &dev.join("shm"), Some("tmpfs"), libc::MS_NOSUID | libc::MS_NODEV, Some("mode=1777"))?;
    for (name, to) in [("fd", "/proc/self/fd"), ("stdin", "/proc/self/fd/0"), ("stdout", "/proc/self/fd/1"), ("stderr", "/proc/self/fd/2")] {
        std::os::unix::fs::symlink(to, dev.join(name)).map_err(|e| format!("/dev/{name}: {e}"))?;
    }
    // /run, with its own runtime directory.
    dir(&at("/run"))?;
    mount(Some(Path::new("tmpfs")), &at("/run"), Some("tmpfs"), libc::MS_NOSUID | libc::MS_NODEV, Some("mode=0755"))?;
    let runtime = at(&format!("/run/user/{}", plan.uid));
    dir(&runtime)?;
    std::fs::set_permissions(&runtime, std::os::unix::fs::PermissionsExt::from_mode(0o700)).map_err(|e| format!("{}: {e}", runtime.display()))?;
    // Its own /tmp, /var/tmp and home, on disk; results to the machine's.
    let b = |p: &str| plan.backing.join(p);
    bind(&b("tmp"), &at("/tmp"))?;
    bind(&b("vartmp"), &at("/var/tmp"))?;
    bind(Path::new(OUT), &at(OUT))?;
    if Path::new("/var/cache/fontconfig").is_dir() {
        bind_ro(Path::new("/var/cache/fontconfig"), &at("/var/cache/fontconfig"))?;
    }
    bind(&b("home"), &at(&plan.home.display().to_string()))?;
    // Of the repository, what runs and what it reads.
    for part in ["target/release", "target/debug", "data", "tools"] {
        let p = plan.repo.join(part);
        if p.is_dir() {
            bind_ro(&p, &at(&p.display().to_string()))?;
        }
    }
    if let Some(busd) = &plan.busd {
        bind_ro(busd, &at(&busd.display().to_string()))?;
    }
    if let Some(chrome) = &plan.chrome {
        // (Its version's folder, as `current` names it: not the folder's
        // other files, the extension's signing key among them.)
        let installed = plan.home.join(".local/share/olo/chrome");
        let name = chrome.file_name().unwrap_or(OsStr::new("current"));
        bind_ro(chrome, &at(&installed.join(name).display().to_string()))?;
        let _ = std::os::unix::fs::symlink(name, at(&installed.join("current").display().to_string()));
    }
    // Into it, the machine's root gone.
    let old = new.join(".old");
    dir(&old)?;
    // SAFETY: pivot_root of a mount point we made, in our mount namespace.
    if unsafe { libc::syscall(libc::SYS_pivot_root, c(&new).as_ptr(), c(&old).as_ptr()) } != 0 {
        return Err(err("pivot_root", &new));
    }
    std::env::set_current_dir("/").map_err(|e| format!("chdir /: {e}"))?;
    // SAFETY: detaching the old root, ours to drop.
    if unsafe { libc::umount2(c(Path::new("/.old")).as_ptr(), libc::MNT_DETACH) } != 0 {
        return Err(err("umount", Path::new("/.old")));
    }
    let _ = std::fs::remove_dir("/.old");
    Ok(())
}

/// `olo-fuzz sandbox-probe`, run inside: what it can see and do, one line
/// each, for the sandbox's test. Exit code 0 if it's sealed.
pub fn probe() -> i32 {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut sealed = true;
    let mut say = |what: &str, ok: bool| {
        println!("{} {what}", if ok { "ok" } else { "LEAK" });
        sealed &= ok;
    };
    say("inside the sandbox", inside());
    say("PID 2 or so: a namespace of its own", std::process::id() < 100);
    // Nothing of the person's home but the repository's parts.
    let in_home: Vec<String> = std::fs::read_dir(&home).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    // The read-only build can live in a worktree, not only ~/src. Permit
    // its actual top-level mount ancestor; no other home entries are exposed.
    let repo_top = root().strip_prefix(&home).ok().and_then(|p| p.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned());
    say(&format!("home holds only its own ({in_home:?})"), in_home.iter().all(|n| matches!(n.as_str(), ".cache" | ".local" | "homes") || repo_top.as_ref() == Some(n)));
    let repo = root();
    let repo_top: Vec<String> = std::fs::read_dir(&repo).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    say(&format!("of the repository, only what runs ({repo_top:?})"), repo_top.iter().all(|n| matches!(n.as_str(), "target" | "data" | "tools")));
    say("the repository's build read-only", std::fs::write(repo.join("target/release/.olo-fuzz-probe"), "x").is_err());
    say("/usr read-only", std::fs::write("/usr/.olo-fuzz-probe", "x").is_err());
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let in_runtime = std::fs::read_dir(&runtime).map(|d| d.count()).unwrap_or(usize::MAX);
    say(&format!("a runtime directory of its own ({runtime}, {in_runtime} entries)"), in_runtime == 0);
    say("no WAYLAND_DISPLAY, D-Bus or agent sockets", ["WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "SSH_AUTH_SOCK", "OLO_SOCKET", "DISPLAY"].iter().all(|k| std::env::var_os(k).is_none()));
    say("no system bus", !Path::new("/run/dbus/system_bus_socket").exists());
    say("no network", std::net::TcpStream::connect_timeout(&"1.1.1.1:443".parse().expect("an address"), std::time::Duration::from_millis(500)).is_err());
    let procs = std::fs::read_dir("/proc").map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok()).count()).unwrap_or(0);
    say(&format!("only its own processes ({procs})"), procs < 10);
    let oom = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap_or_default();
    say(&format!("the first to go if memory runs out (oom_score_adj {})", oom.trim()), oom.trim() == "1000");
    say("results can be written", std::fs::write(Path::new(OUT).join(".probe"), "x").is_ok());
    let _ = std::fs::remove_file(Path::new(OUT).join(".probe"));
    i32::from(!sealed)
}
