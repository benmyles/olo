//! Keeping pages from hurting each other or NetPositive. Servo runs each
//! site's pages in a content process of their own (engine.rs), and this
//! keeps each one in its own box:
//!
//! - **A unit each.** A content process hands itself to the user's
//!   systemd as a transient service (`tabula-browser-<ui pid>-<n>`), so
//!   the kernel gives it its own memory limit (`MemoryMax`, see
//!   [`page_memory`]; no swap: going over kills that page alone), a cap
//!   on threads, and a CPU weight we can move both ways as tabs come and
//!   go. No privileges are needed: the user manager owns its cgroups.
//!   Without systemd, the page runs where Servo started it, niced, with
//!   an `RLIMIT_DATA` and an RSS watch.
//! - **Which process is which tab.** Every couple of seconds each tab is
//!   asked, in a script, for its process id: the media backend in the
//!   content process answers a keyed `canPlayType` a bit at a time
//!   ([`pid_bit`]). A page that hasn't answered in ten seconds is stuck
//!   in a script; its tab offers to stop it, and it drops to the lowest
//!   weight meanwhile.
//! - **Who gets the CPU.** Pages you can see outweigh hidden tabs five to
//!   one, and a stuck page gets almost nothing.
//!
//! The watching runs on a thread of its own ([`watch`]); the tab side is
//! [`Pings`].

use std::{
    cell::RefCell,
    collections::HashMap,
    os::fd::AsFd,
    rc::Rc,
    sync::{
        OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

use servo::{JSValue, WebView, WebViewId};
use tabula_kit::Proxy;
use zbus::blocking::Connection;
use zvariant::{Fd, OwnedObjectPath, OwnedValue, Value};

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const SERVICE: &str = "org.freedesktop.systemd1.Service";
/// Set in a page's unit, so it doesn't hand itself off again.
const IN_UNIT: &str = "TABULA_BROWSER_PAGE_UNIT";
/// NetPositive's process id, for a page in a unit of its own to follow.
const UI_PID: &str = "TABULA_BROWSER_UI_PID";
/// The key `pid_bit` wants, made at start and inherited by every page.
const KEY: &str = "TABULA_BROWSER_KEY";
/// How often tabs are asked, and how long one may take to answer.
const PING_EVERY: Duration = Duration::from_secs(2);
const HUNG_AFTER: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_secs(1);

/// Make the key pages answer `pid_bit` with. Call first thing in `main`,
/// before there are threads: it goes in the environment, for the content
/// processes to inherit.
pub fn init() {
    if std::env::var_os(KEY).is_none() {
        // SAFETY: called at the top of `main`, before any other thread.
        unsafe { std::env::set_var(KEY, crate::logins::token()) };
    }
    servo_constellation::set_pid_alias(handed_off_pid);
}

/// Where a content process that handed itself off (`hand_off`) notes the
/// pid it went on to run as, named by its own: NetPositive's process
/// manager knows it by the first, Servo's memory profiler by the second.
fn pids_dir(parent: u32) -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("tabula-browser-{parent}.pids"))
}

/// The pid content process `pid` became (itself if it didn't hand off).
fn handed_off_pid(pid: u32) -> u32 {
    let note = pids_dir(std::process::id()).join(pid.to_string());
    let real = std::fs::read_to_string(&note).ok().and_then(|s| s.trim().parse().ok());
    let _ = std::fs::remove_file(note);
    real.unwrap_or(pid)
}

fn key() -> &'static str {
    static KEY_VALUE: OnceLock<String> = OnceLock::new();
    KEY_VALUE.get_or_init(|| std::env::var(KEY).unwrap_or_default())
}

/// `canPlayType("video/x-tabula-pid; key=…; bit=N")`, answered in a
/// content process: bit N of its process id, or None if this isn't our
/// question (or the key is wrong: pages can't ask).
pub fn pid_bit(media_type: &str) -> Option<bool> {
    let rest = media_type.strip_prefix("video/x-tabula-pid;")?;
    let (mut key_ok, mut bit) = (false, None);
    for p in rest.split(';') {
        match p.trim().split_once('=') {
            Some(("key", k)) => key_ok = !key().is_empty() && k == key(),
            Some(("bit", b)) => bit = b.parse::<u32>().ok().filter(|b| *b < 32),
            _ => {}
        }
    }
    let bit = bit.filter(|_| key_ok)?;
    Some(std::process::id() >> bit & 1 == 1)
}

/// The script that reads a page's process id, a bit at a time.
fn pid_script() -> String {
    format!(
        "(function() {{ var v = document.createElement('video'), n = 0; \
         for (var i = 0; i < 32; i++) if (v.canPlayType('video/x-tabula-pid; key={}; bit=' + i)) n += Math.pow(2, i); \
         return n; }})()",
        key()
    )
}

fn total_memory() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("MemTotal:")?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()))
        .map_or(8 << 30, |kb| kb << 10)
}

/// The most memory one content process may have: a quarter of the
/// machine's, between 1.5 and 4 GiB, which the heaviest web apps fit in
/// and leaves the rest of the desktop room (or
/// `TABULA_BROWSER_PAGE_MEMORY` MiB, for tests).
pub fn page_memory() -> u64 {
    if let Some(mib) = std::env::var("TABULA_BROWSER_PAGE_MEMORY").ok().and_then(|v| v.parse::<u64>().ok()) {
        return mib << 20;
    }
    (total_memory() / 4).clamp(3 << 29, 4 << 30)
}

/// A content process starts here. It hands itself to systemd if it can
/// (then the unit's process runs the pages, and this one exits), and
/// otherwise limits itself and runs them.
pub fn content_process(token: String) {
    if let Some(ui) = std::env::var(UI_PID).ok().and_then(|p| p.parse().ok()) {
        end_with(ui);
    } else if std::env::var_os(IN_UNIT).is_none() {
        match hand_off(&token) {
            Ok(()) => return,
            Err(e) => {
                tracing::debug!("page stays in process {}: {e}", std::process::id());
                limit_self();
            }
        }
    }
    crate::media::install();
    servo::run_content_process(token);
}

/// Run this content process again as a transient systemd service, with
/// our stdout and stderr, and wait until it's started.
fn hand_off(token: &str) -> Result<(), String> {
    // This very binary through /proc, even once an upgrade has replaced
    // the file; its usual name for argv[0].
    let name = std::env::current_exe().ok().and_then(|p| p.to_str().map(|s| s.trim_end_matches(" (deleted)").to_string()));
    let exe = format!("/proc/{}/exe", std::process::id());
    let argv0 = name.unwrap_or_else(|| exe.clone());
    let bus = Connection::session().map_err(|e| e.to_string())?;
    // Only a running systemd: asking for one that isn't there could start
    // a wait on bus activation that never ends.
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus).map_err(|e| e.to_string())?;
    let systemd = zbus::names::BusName::try_from(SYSTEMD).map_err(|e| e.to_string())?;
    let running = dbus.name_has_owner(systemd).map_err(|e| e.to_string())?;
    if !running {
        return Err("no systemd on the session bus".into());
    }
    let name = format!("tabula-browser-{}-{}.service", std::os::unix::process::parent_id(), std::process::id());
    let mut env: Vec<String> =
        std::env::vars_os().filter_map(|(k, v)| Some(format!("{}={}", k.to_str()?, v.to_str()?))).filter(|kv| !kv.starts_with('=')).collect();
    env.push(format!("{IN_UNIT}={name}"));
    env.push(format!("{UI_PID}={}", std::os::unix::process::parent_id()));
    let cwd = std::env::current_dir().ok().and_then(|d| d.to_str().map(String::from)).unwrap_or_else(|| "/".into());
    let max = page_memory();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let properties: Vec<(&str, Value)> = vec![
        ("Description", Value::from("NetPositive page")),
        ("Type", Value::from("exec")),
        ("ExecStart", Value::from(vec![(exe, vec![argv0, "--content-process".to_string(), token.to_string()], false)])),
        ("Environment", Value::from(env)),
        ("WorkingDirectory", Value::from(cwd)),
        ("StandardOutputFileDescriptor", Value::from(Fd::from(stdout.as_fd()))),
        ("StandardErrorFileDescriptor", Value::from(Fd::from(stderr.as_fd()))),
        ("CPUWeight", Value::from(Priority::Visible.weight())),
        ("MemoryMax", Value::from(max)),
        // No swap: a page running away with memory meets its limit (and
        // ends) rather than grinding the disk.
        ("MemorySwapMax", Value::from(0u64)),
        ("TasksMax", Value::from(4096u64)),
        ("KillMode", Value::from("control-group")),
        ("SendSIGHUP", Value::from(false)),
    ];
    let aux: Vec<(&str, Vec<(&str, Value)>)> = Vec::new();
    bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "StartTransientUnit", &(name.as_str(), "fail", properties, aux))
        .map_err(|e| e.to_string())?;
    // Wait for it to be running (or not): the constellation waits for one
    // of us to connect, so only give up if it never started.
    let start = Instant::now();
    loop {
        let state = unit_path(&bus, &name).and_then(|p| get::<String>(&bus, &p, UNIT, "ActiveState"));
        match state.as_deref() {
            Some("active") => {
                if let Some(pid) = unit_path(&bus, &name).and_then(|p| get::<u32>(&bus, &p, SERVICE, "MainPID")).filter(|p| *p > 0) {
                    let dir = pids_dir(std::os::unix::process::parent_id());
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join(std::process::id().to_string()), pid.to_string());
                }
                return Ok(());
            }
            Some("failed") => {
                let _ = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "ResetFailedUnit", &(name.as_str(),));
                return Err("its unit failed to start".into());
            }
            _ if start.elapsed() > Duration::from_secs(5) => {
                let _ = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "StopUnit", &(name.as_str(), "replace"));
                return Err("its unit didn't start".into());
            }
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Without systemd: a lower priority than NetPositive's own, and a limit
/// on memory the kernel enforces on this process.
/// End when NetPositive (`ui`) does, however it ends: a page in a unit of
/// its own isn't its child, so nothing else would stop it.
fn end_with(ui: i32) {
    // SAFETY: pidfd_open on a process id; the descriptor is ours.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, ui, 0) } as i32;
    if fd < 0 {
        tracing::debug!("page: NetPositive (pid {ui}) is gone");
        std::process::exit(0);
    }
    std::thread::Builder::new()
        .name("ui-watch".into())
        .spawn(move || {
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd we own; readable once the process exits.
            while unsafe { libc::poll(&mut p, 1, -1) } < 0 {}
            std::process::exit(0);
        })
        .expect("thread");
}

fn limit_self() {
    // SAFETY: plain syscalls on this process.
    unsafe {
        // A child of NetPositive here: end with it.
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
        libc::setpriority(libc::PRIO_PROCESS, 0, 5);
        let max = page_memory() as libc::rlim_t;
        libc::setrlimit(libc::RLIMIT_DATA, &libc::rlimit { rlim_cur: max, rlim_max: max });
    }
}

fn unit_path(bus: &Connection, name: &str) -> Option<OwnedObjectPath> {
    let reply = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "GetUnit", &(name,)).ok()?;
    reply.body().deserialize().ok()
}

fn get<T: TryFrom<OwnedValue>>(bus: &Connection, path: &OwnedObjectPath, iface: &str, prop: &str) -> Option<T> {
    let reply = bus.call_method(Some(SYSTEMD), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(iface, prop)).ok()?;
    let v: OwnedValue = reply.body().deserialize().ok()?;
    T::try_from(v).ok()
}

/// How much CPU a page gets against the others.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Priority {
    Hung,
    Hidden,
    Visible,
}

impl Priority {
    fn weight(self) -> u64 {
        match self {
            Priority::Visible => 100,
            Priority::Hidden => 20,
            Priority::Hung => 1,
        }
    }

    /// Without systemd only nice is left, and it only goes up.
    fn nice(self) -> i32 {
        match self {
            Priority::Visible | Priority::Hidden => 5,
            Priority::Hung => 19,
        }
    }
}

enum Cmd {
    Priority(u32, Priority),
    Kill(u32),
}

/// From the watching thread, as an app message.
pub enum Report {
    /// A second has passed: time to look at the tabs.
    Tick,
    /// A page's process is gone, killed for using too much memory (or by
    /// us, asked to stop).
    Stopped { pid: u32, memory: bool },
}

pub struct Watch {
    tx: Sender<Cmd>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(pids_dir(std::process::id()));
    }
}

impl Watch {
    pub fn set_priority(&self, pid: u32, p: Priority) {
        let _ = self.tx.send(Cmd::Priority(pid, p));
    }

    pub fn kill(&self, pid: u32) {
        let _ = self.tx.send(Cmd::Kill(pid));
    }
}

/// Start watching our content processes; reports come to the app as
/// [`Report`] messages.
pub fn watch(proxy: Proxy) -> Watch {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("page-guard".into()).spawn(move || Watcher::new(proxy).run(rx)).expect("thread");
    Watch { tx }
}

struct Unit {
    name: String,
    path: OwnedObjectPath,
    weight: u64,
}

struct Watcher {
    proxy: Proxy,
    bus: Option<Connection>,
    me: u32,
    /// Units by their process.
    units: HashMap<u32, Unit>,
    /// Processes we were told to stop, so their end isn't blamed on memory.
    killed: Vec<u32>,
    /// Without systemd: the nice each process is at.
    niced: HashMap<u32, i32>,
}

impl Watcher {
    fn new(proxy: Proxy) -> Watcher {
        let bus = Connection::session().ok();
        Watcher { proxy, bus, me: std::process::id(), units: HashMap::new(), killed: Vec::new(), niced: HashMap::new() }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        self.collect_leftovers();
        let mut next = Instant::now() + TICK;
        loop {
            match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(Cmd::Priority(pid, p)) => self.set_priority(pid, p),
                Ok(Cmd::Kill(pid)) => self.kill(pid),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.tick();
                    next = Instant::now() + TICK;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Units a NetPositive that crashed left failed.
    fn collect_leftovers(&self) {
        let Some(bus) = &self.bus else { return };
        let Ok(reply) =
            bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "ListUnitsByPatterns", &(vec!["failed"], vec!["tabula-browser-*.service"]))
        else {
            return;
        };
        type Row = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);
        let rows: Vec<Row> = reply.body().deserialize().unwrap_or_default();
        for r in rows {
            let _ = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "ResetFailedUnit", &(r.0.as_str(),));
        }
    }

    /// The unit `pid` runs in, if it's one of our pages'.
    fn unit(&mut self, pid: u32) -> Option<&mut Unit> {
        if !self.units.contains_key(&pid) {
            let bus = self.bus.as_ref()?;
            let reply = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "GetUnitByPID", &(pid,)).ok()?;
            let path: OwnedObjectPath = reply.body().deserialize().ok()?;
            let name: String = get(bus, &path, UNIT, "Id")?;
            if !name.starts_with(&format!("tabula-browser-{}-", self.me)) {
                return None;
            }
            self.units.insert(pid, Unit { name, path, weight: Priority::Visible.weight() });
        }
        self.units.get_mut(&pid)
    }

    fn set_priority(&mut self, pid: u32, p: Priority) {
        if pid == self.me {
            return;
        }
        let weight = p.weight();
        if let Some(u) = self.unit(pid) {
            if u.weight == weight {
                return;
            }
            u.weight = weight;
            let name = u.name.clone();
            if let Some(bus) = &self.bus {
                let props: Vec<(&str, Value)> = vec![("CPUWeight", Value::from(weight))];
                let r = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "SetUnitProperties", &(name.as_str(), true, props));
                if let Err(e) = r {
                    tracing::debug!("{name}: CPUWeight {weight}: {e}");
                }
            }
            return;
        }
        // No unit: nice every thread, and only ever further.
        let nice = p.nice();
        if self.niced.get(&pid).is_some_and(|n| *n >= nice) {
            return;
        }
        self.niced.insert(pid, nice);
        for tid in threads(pid) {
            // SAFETY: a plain syscall; a thread that just went is harmless.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, nice) };
        }
    }

    fn kill(&mut self, pid: u32) {
        if pid == self.me || pid <= 1 {
            return;
        }
        self.killed.push(pid);
        // SAFETY: a plain syscall, at a content process of ours.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }

    /// Learn the process of each of our units while it runs, so that when
    /// one ends we can say which; units that ended before we saw them are
    /// just collected.
    fn discover(&mut self) {
        let Some(bus) = &self.bus else { return };
        let pattern = format!("tabula-browser-{}-*.service", self.me);
        let Ok(reply) =
            bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "ListUnitsByPatterns", &(Vec::<&str>::new(), vec![pattern.as_str()]))
        else {
            return;
        };
        type Row = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);
        let rows: Vec<Row> = reply.body().deserialize().unwrap_or_default();
        for (name, _, _, active, _, _, path, ..) in rows {
            if self.units.values().any(|u| u.name == name) {
                continue;
            }
            match active.as_str() {
                "active" => {
                    if let Some(pid) = get::<u32>(bus, &path, SERVICE, "MainPID").filter(|p| *p > 0) {
                        self.units.insert(pid, Unit { name, path, weight: Priority::Visible.weight() });
                    }
                }
                "failed" => {
                    let _ = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "ResetFailedUnit", &(name.as_str(),));
                }
                _ => {}
            }
        }
    }

    /// Find pages that ended, and why.
    fn tick(&mut self) {
        self.discover();
        let mut gone = Vec::new();
        for (&pid, u) in &self.units {
            let Some(bus) = &self.bus else { break };
            let state: Option<String> = get(bus, &u.path, UNIT, "ActiveState");
            if matches!(state.as_deref(), Some("active" | "activating" | "deactivating" | "reloading")) {
                continue;
            }
            let result: Option<String> = get(bus, &u.path, SERVICE, "Result");
            tracing::debug!("page process {pid} ({}) ended: {result:?}", u.name);
            let _ = bus.call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "ResetFailedUnit", &(u.name.as_str(),));
            gone.push((pid, result.as_deref() == Some("oom-kill")));
        }
        // Without systemd, an RSS watch stands in for MemoryMax.
        let max = page_memory();
        for &pid in self.niced.keys() {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                gone.push((pid, false));
            } else if rss(pid).is_some_and(|r| r > max) && !self.killed.contains(&pid) {
                // SAFETY: a plain syscall, at a content process of ours.
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                gone.push((pid, true));
            }
        }
        self.proxy.send(Report::Tick);
        for (pid, memory) in gone {
            self.units.remove(&pid);
            self.niced.remove(&pid);
            let asked = self.killed.contains(&pid);
            self.killed.retain(|k| *k != pid);
            if memory || asked {
                self.proxy.send(Report::Stopped { pid, memory: memory && !asked });
            }
        }
    }
}

fn threads(pid: u32) -> Vec<i32> {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default()
}

fn rss(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    Some(pages * page)
}

/// What a tab's page has said about itself.
#[derive(Default)]
struct Ping {
    /// When we asked, if it hasn't answered yet.
    asked: Option<Instant>,
    /// When it last answered.
    answered: Option<Instant>,
    pid: Option<u32>,
    /// "Wait" was pressed for the current wait.
    waived: bool,
}

/// The tabs' side: asks each page for its process, and notices those that
/// stop answering.
#[derive(Default)]
pub struct Pings {
    pings: Rc<RefCell<HashMap<WebViewId, Ping>>>,
}

impl Pings {
    /// Ask `view` again, if it's time.
    pub fn ask(&self, view: &WebView) {
        let id = view.id();
        {
            let mut pings = self.pings.borrow_mut();
            let p = pings.entry(id).or_default();
            if p.asked.is_some() || p.answered.is_some_and(|a| a.elapsed() < PING_EVERY) {
                return;
            }
            p.asked = Some(Instant::now());
        }
        let pings = Rc::downgrade(&self.pings);
        view.evaluate_javascript(pid_script(), move |result| {
            let Some(pings) = pings.upgrade() else { return };
            let mut pings = pings.borrow_mut();
            let Some(p) = pings.get_mut(&id) else { return };
            p.asked = None;
            p.answered = Some(Instant::now());
            p.waived = false;
            if let Ok(JSValue::Number(n)) = result
                && n > 0.0
            {
                p.pid = Some(n as u32);
            }
        });
    }

    /// The process `view`'s page last said it's in.
    pub fn pid(&self, view: WebViewId) -> Option<u32> {
        self.pings.borrow().get(&view)?.pid
    }

    /// Whether `view`'s page has been stuck long enough to say so.
    pub fn hung(&self, view: WebViewId) -> bool {
        self.pings.borrow().get(&view).is_some_and(|p| !p.waived && p.asked.is_some_and(|a| a.elapsed() >= HUNG_AFTER))
    }

    /// Stop saying it's stuck until it answers and gets stuck again.
    pub fn wait(&self, view: WebViewId) {
        if let Some(p) = self.pings.borrow_mut().get_mut(&view) {
            p.waived = true;
        }
    }

    /// The page in `view` was replaced (stopped, or a new one loaded).
    pub fn forget(&self, view: WebViewId) {
        self.pings.borrow_mut().remove(&view);
    }

    /// Tabs whose page is in `pid`.
    pub fn views_in(&self, pid: u32) -> Vec<WebViewId> {
        self.pings.borrow().iter().filter(|(_, p)| p.pid == Some(pid)).map(|(id, _)| *id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_only_with_the_key() {
        // SAFETY: tests that read the key run in this one.
        unsafe { std::env::set_var(KEY, "k3y") };
        let me = std::process::id();
        let mut n = 0u32;
        for bit in 0..32 {
            if pid_bit(&format!("video/x-tabula-pid; key=k3y; bit={bit}")).unwrap() {
                n |= 1 << bit;
            }
        }
        assert_eq!(n, me);
        assert_eq!(pid_bit("video/x-tabula-pid; key=nope; bit=0"), None);
        assert_eq!(pid_bit("video/x-tabula-pid; bit=0"), None);
        assert_eq!(pid_bit("video/mp4"), None);
    }
}
