//! What every Olo program leaves when it crashes, so the crash can be
//! looked into after: [`install`], the first thing `main` does
//! (`olo_kit::run` does it for the kit's apps; a test checks every
//! program does, tests/every_program.rs).
//!
//! - **A report**, one JSON file a crash in `~/.local/state/olo/crashes/`
//!   ([`dir`]), `PROGRAM-PID-MILLIS.json`, the newest [`KEPT`] of each
//!   program kept:
//!   - a panic, any thread's, whether the program goes on or not: its
//!     message, where, the thread, and the backtrace (release builds keep
//!     line tables, so it names files and lines);
//!   - a fatal signal (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT): which,
//!     its code and address, the thread. Written by the handler with
//!     system calls alone (async-signal-safe: nothing allocated, no lock
//!     taken), which then hands the signal on to what had it before
//!     (std's stack-overflow check, else the default action), so a core
//!     is still made: `coredumpctl info PID` has the rest.
//!
//!   Each says the program, its pid, its executable and build (the
//!   file's time), when it started and when it crashed.
//! - **Its log in the journal**: a program whose stderr goes nowhere
//!   (/dev/null: one started by Tracker, Deskbar or another app) sends
//!   it to the journal (its stream socket, named for the program), so
//!   Logs shows what it said, and its panic's message.
//!
//! Nothing is done while the program runs: a panic hook and five signal
//! handlers set at start, a few system calls (under 100 µs), and nothing
//! after until a crash.

use std::{
    ffi::CString,
    io::Write as _,
    os::unix::{ffi::OsStrExt as _, io::AsRawFd as _, net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Reports kept for each program, the newest.
pub const KEPT: usize = 10;
/// Reports one process writes at most (a panic over and over in a
/// pool's workers).
const MOST: u32 = 4;
const SIGNALS: [libc::c_int; 5] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT];
const JOURNAL: &str = "/run/systemd/journal/stdout";

/// What the handlers need, made at start (nothing is made in them).
struct Setup {
    program: String,
    dir: PathBuf,
    /// `dir`, NUL-ended, for the signal handler.
    dir_c: CString,
    exe: String,
    /// The executable's time (seconds): which build it was.
    build: u64,
    started_ms: u64,
}

static SETUP: OnceLock<Setup> = OnceLock::new();
/// What had each of `SIGNALS` before, to hand it on.
static BEFORE: OnceLock<[libc::sigaction; 5]> = OnceLock::new();
/// A panic was reported: the abort that may follow is the same crash.
static PANICKED: AtomicBool = AtomicBool::new(false);
/// A signal was reported: what it's handed on to isn't reported again.
static SIGNALLED: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicU32 = AtomicU32::new(0);
/// stderr is the journal's stream (`journal_stderr`): a crash's line is
/// marked an error there ("<3>").
static JOURNALED: AtomicBool = AtomicBool::new(false);

/// What a crash's line starts with: the journal's mark for an error, if
/// it goes there.
fn error_mark() -> &'static [u8] {
    if JOURNALED.load(Ordering::Relaxed) { b"<3>" } else { b"" }
}

/// Where reports go: `~/.local/state/olo/crashes` (`$XDG_STATE_HOME`'s
/// if set); a service with no home's (the Wi-Fi healer, as root) in
/// /var/lib/olo/crashes.
pub fn dir() -> PathBuf {
    let absolute = |v: &str| std::env::var_os(v).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(state) = absolute("XDG_STATE_HOME") {
        return state.join("olo/crashes");
    }
    match absolute("HOME") {
        Some(home) if home != Path::new("/") => home.join(".local/state/olo/crashes"),
        // SAFETY: getuid has no preconditions.
        _ if unsafe { libc::getuid() } == 0 => PathBuf::from("/var/lib/olo/crashes"),
        _ => PathBuf::from(format!("/var/tmp/olo-crashes-{}", unsafe { libc::getuid() })),
    }
}

/// The program's name: its executable's (an upgrade's old one has
/// " (deleted)" after it).
fn program_name(exe: &Path) -> String {
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "olo".into());
    let name = name.trim_end_matches(" (deleted)");
    // Safe in a file name and in JSON as it is.
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

/// Whether a panic's message is `print!`'s or `eprint!`'s on a pipe its
/// reader closed.
fn output_cut_off(message: Option<&str>) -> bool {
    message.is_some_and(|m| (m.starts_with("failed printing to stdout") || m.starts_with("failed printing to stderr")) && m.contains("Broken pipe"))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// How many files a program may keep open below which it is held too
/// tight (Olo Doctor says so): a login's 1,024 is, 65,536 isn't.
pub const OPEN_FILES_ENOUGH: u64 = 65_536;

/// This program may keep as many files open as its account is allowed:
/// the limit it starts with (RLIMIT_NOFILE's soft limit, 1,024 from a
/// login, kept so low for programs that still use `select`) raised to
/// the most it may be (the hard limit: 524,288 on Fedora). Its old and
/// new value; None where it can't be read.
///
/// Every Olo program does this as it starts (`install`), and what it
/// starts inherits it. A window server's every client, buffer and
/// sync object is a file; app_server with three displays' windows had
/// 696 open of its 1,024 when Deskbar's folding made every window draw
/// anew, and past the limit the kernel drops the descriptors programs
/// send: Terminal and Tracker lost their windows ("file descriptor
/// expected", 2026-11-18). A terminal's shells and a file manager's
/// folders are files too.
pub fn raise_open_files() -> Option<(u64, u64)> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes `limit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    let was = limit.rlim_cur;
    // ("Unlimited" isn't given for files: the kernel's own ceiling then.)
    let most = if limit.rlim_max == libc::RLIM_INFINITY { std::fs::read_to_string("/proc/sys/fs/nr_open").ok().and_then(|n| n.trim().parse().ok()).unwrap_or(1_048_576) } else { limit.rlim_max };
    if was >= most {
        return Some((was, was));
    }
    limit.rlim_cur = most;
    // SAFETY: setrlimit reads `limit`; a soft limit up to the hard one needs no privilege.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Some((was, was));
    }
    Some((was, most))
}

/// A block of this size or more is mapped by itself and unmapped the
/// moment it is freed, whichever thread asked (where the C library
/// starts: `give_memory_back` holds it there).
const MAPPED: libc::c_int = 128 << 10;
/// What may stay free at the end of a thread's share of the heap before
/// it goes back to the system (where the C library starts, too).
const KEPT_FREE: libc::c_int = 128 << 10;

/// Memory this program has finished with goes back to the system: the C
/// library's allocator held to the two thresholds it starts with.
/// Whether it took them (false where the C library isn't glibc, or with
/// `OLO_MALLOC_AS_IT_COMES` set: to measure one against the other). For
/// a program to call as it starts, after [`install`]: the compositor
/// does.
///
/// Left alone, glibc raises both as a program runs. Once a block of up
/// to 32 MiB has been mapped and freed, blocks up to that size are no
/// longer mapped by themselves but cut from the asking thread's arena;
/// and from then on an arena's free end is given back only once it is
/// twice that large, which an arena of 64 MiB never is. `malloc_trim`
/// doesn't take it either: of a thread's arena it returns the holes,
/// never the end. So a thread that needed much for a moment keeps it for
/// the life of the program, written to and unused. The compositor's
/// wallpaper thread decodes a picture of 35 MiB to show 5: its arena
/// stood at 37 MiB, 36.8 of it dirty, on every screen, an agent's too
/// (2026-11-21).
///
/// Measured in turn (release builds; a test desktop of 1440 x 900 at 1x
/// drawing on the GPU, at rest): the compositor's own memory 54.1 MiB to
/// 22.6, twice each.
/// Over a run of the speed benchmark its time on the processors was 3.6 s
/// as the allocator comes and 3.2 and 3.4 s so held: three fifths more
/// page faults, which cost less than they save.
///
/// Not for every program, yet. The kit's apps were tried with the same
/// (through `GLIBC_TUNABLES`, without a rebuild). At rest they held the
/// same either way, but for what a first run's compiling of shaders
/// leaves, which no threshold gives back (it is in use). And StyledEdit
/// scrolling and typing took a fifth more of a processor, Viewer
/// resizing a sixth: what a frame frees went back to the system and was
/// faulted in again with every frame. With the thresholds higher (32
/// MiB mapped, 4 or 16 kept free) that cost went, and with it the gain
/// after a first run. docs/TODO.md has what to try there.
pub fn give_memory_back() -> bool {
    if std::env::var_os("OLO_MALLOC_AS_IT_COMES").is_some() {
        return false;
    }
    #[cfg(target_env = "gnu")]
    // SAFETY: two settings of the C library's allocator, plain numbers.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, MAPPED) == 1 && libc::mallopt(libc::M_TRIM_THRESHOLD, KEPT_FREE) == 1
    }
    #[cfg(not(target_env = "gnu"))]
    false
}

static OPEN_FILES: OnceLock<Option<(u64, u64)>> = OnceLock::new();

/// What `install` found and made of this program's limit on open files
/// (`raise_open_files`: what it was, what it is), for a program that
/// says so in its log.
pub fn open_files() -> Option<(u64, u64)> {
    OPEN_FILES.get().copied().flatten()
}

/// What every Olo program does first: it may keep the files it needs
/// open (`raise_open_files`), and is ready for a crash: the report's
/// hook and handlers, and stderr to the journal when it goes nowhere.
/// Once; the first thing `main` does (`tests/every_program.rs` holds
/// every program to it).
pub fn install() {
    if SETUP.get().is_some() {
        return;
    }
    let _ = OPEN_FILES.set(raise_open_files());
    let exe = std::env::current_exe().unwrap_or_default();
    let program = program_name(&exe);
    let exe = exe.to_string_lossy().trim_end_matches(" (deleted)").to_owned();
    let build = std::fs::metadata(&exe).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let dir = dir();
    let Ok(dir_c) = CString::new(dir.as_os_str().as_bytes()) else { return };
    journal_stderr(&program);
    if SETUP.set(Setup { program, dir, dir_c, exe, build, started_ms: now_ms() }).is_err() {
        return;
    }
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Output a reader stopped reading (`olo-screen --help | head`):
        // not a crash, the end a shell expects of a program whose output
        // was cut off, as SIGPIPE's (128 + 13), with no report. (Rust
        // ignores SIGPIPE, so the write fails and `println!` panics: on
        // 2026-11-15 that read as Agent Screens crashing.)
        if output_cut_off(info.payload_as_str()) {
            std::process::exit(141);
        }
        PANICKED.store(true, Ordering::SeqCst);
        if REPORTED.fetch_add(1, Ordering::SeqCst) < MOST {
            panic_report(info);
        }
        before(info);
    }));
    // SAFETY: sigaction with zeroed (valid, empty) structs, the handler
    // async-signal-safe; what was there is kept first, to hand on to.
    unsafe {
        let mut before: [libc::sigaction; 5] = std::mem::zeroed();
        for (i, s) in SIGNALS.iter().enumerate() {
            libc::sigaction(*s, std::ptr::null(), &mut before[i]);
        }
        let _ = BEFORE.set(before);
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut sa.sa_mask);
        for s in SIGNALS {
            libc::sigaction(s, &sa, std::ptr::null_mut());
        }
    }
}

/// stderr to the journal, if it goes nowhere: /dev/null, or closed.
/// Non-blocking, so a journal that can't keep up loses a line rather
/// than holding the program up.
fn journal_stderr(program: &str) {
    // SAFETY: fstat fills `st`, zeroed first.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let nowhere = unsafe { libc::fstat(2, &mut st) } != 0 || ((st.st_mode & libc::S_IFMT) == libc::S_IFCHR && st.st_rdev == libc::makedev(1, 3));
    if !nowhere {
        return;
    }
    // (`OLO_CRASH_JOURNAL`: another socket, for the tests.)
    let test_socket = std::env::var_os("OLO_CRASH_JOURNAL");
    // A program a test started (cargo's environment, which a test's
    // children have) keeps out of the person's journal: there its lines
    // read as the real program's (olo-logs, Olo Doctor; 2026-11-17, a
    // test's phone service, its stderr discarded, said "can't register
    // with BlueZ" and named the stand-in's phone among the real one's).
    if test_socket.is_none() && std::env::var_os("CARGO_MANIFEST_DIR").is_some() {
        return;
    }
    let socket = test_socket.map_or_else(|| PathBuf::from(JOURNAL), PathBuf::from);
    let Ok(mut s) = UnixStream::connect(socket) else { return };
    // The stream's header: its name, no unit, info unless a line starts
    // "<N>" (a crash's: error), and not on to syslog, the kernel's log or
    // the console.
    if s.write_all(format!("{program}\n\n6\n1\n0\n0\n0\n").as_bytes()).is_err() || s.set_nonblocking(true).is_err() {
        return;
    }
    let _ = s.shutdown(std::net::Shutdown::Read);
    // SAFETY: dup2 onto stderr; `s` closes its own descriptor after.
    if unsafe { libc::dup2(s.as_raw_fd(), 2) } == 2 {
        JOURNALED.store(true, Ordering::Relaxed);
    }
}

/// `s` as a JSON string's insides.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn panic_report(info: &std::panic::PanicHookInfo) {
    let Some(setup) = SETUP.get() else { return };
    let thread = std::thread::current();
    let message = info.payload_as_str().unwrap_or("(not a message)");
    let place = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let at = now_ms();
    let json = format!(
        "{{\"program\":\"{}\",\"pid\":{},\"exe\":\"{}\",\"build\":{},\"started_ms\":{},\"at_ms\":{},\"kind\":\"panic\",\"thread\":\"{}\",\"message\":\"{}\",\"place\":\"{}\",\"backtrace\":\"{}\"}}\n",
        setup.program,
        std::process::id(),
        escape(&setup.exe),
        setup.build,
        setup.started_ms,
        at,
        escape(thread.name().unwrap_or("unnamed")),
        escape(message),
        escape(&place),
        escape(&backtrace),
    );
    let name = format!("{}-{}-{at}.json", setup.program, std::process::id());
    let _ = std::fs::create_dir_all(&setup.dir);
    let part = setup.dir.join(format!(".{name}"));
    let to = setup.dir.join(&name);
    if std::fs::write(&part, json).and_then(|()| std::fs::rename(&part, &to)).is_ok() {
        eprintln!("{}{}: crashed; the report is {}", String::from_utf8_lossy(error_mark()), setup.program, to.display());
        prune(&setup.dir, &setup.program);
    }
}

/// The program's reports past the newest [`KEPT`] go.
fn prune(dir: &Path, program: &str) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let prefix = format!("{program}-");
    let mut ours: Vec<(u64, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix(&prefix)?.strip_suffix(".json")?;
            // PID-MILLIS: the time is the last part.
            let at = rest.rsplit('-').next()?.parse().ok()?;
            Some((at, e.path()))
        })
        .collect();
    ours.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, path) in ours.into_iter().skip(KEPT) {
        let _ = std::fs::remove_file(path);
    }
}

/// Bytes gathered in place, for the signal handler (nothing allocated).
struct Buf<const N: usize> {
    b: [u8; N],
    n: usize,
}

impl<const N: usize> Buf<N> {
    fn new() -> Self {
        Buf { b: [0; N], n: 0 }
    }

    fn put(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            if self.n < N {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    /// As a JSON string's insides.
    fn esc(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            match c {
                b'"' | b'\\' => self.put(&[b'\\', c]),
                c if c < 0x20 => self.put(b"?"),
                c => self.put(&[c]),
            };
        }
        self
    }

    fn num(&mut self, mut v: u64) -> &mut Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..])
    }

    fn int(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.put(b"-");
        }
        self.num(v.unsigned_abs())
    }

    fn hex(&mut self, v: u64) -> &mut Self {
        self.put(b"0x");
        let mut d = [0u8; 16];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b"0123456789abcdef"[(v & 15) as usize];
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..])
    }

    fn bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }
}

fn signal_name(sig: libc::c_int) -> &'static [u8] {
    match sig {
        libc::SIGSEGV => b"SIGSEGV",
        libc::SIGBUS => b"SIGBUS",
        libc::SIGILL => b"SIGILL",
        libc::SIGFPE => b"SIGFPE",
        libc::SIGABRT => b"SIGABRT",
        _ => b"signal",
    }
}

extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let first = !SIGNALLED.swap(true, Ordering::SeqCst);
    // An abort after a panic (one while unwinding) is that panic's crash.
    if first
        && !(sig == libc::SIGABRT && PANICKED.load(Ordering::SeqCst))
        && REPORTED.fetch_add(1, Ordering::SeqCst) < MOST
        && let Some(setup) = SETUP.get()
    {
        // SAFETY: `info` is the kernel's, for this signal.
        unsafe { signal_report(setup, sig, &*info) };
    }
    // SAFETY: sigaction, signal and raise are async-signal-safe.
    unsafe {
        if info.is_null() || (*info).si_code <= 0 {
            // Sent (kill, abort's raise): its default action, as its
            // sender meant (std's handler would take a sent SIGSEGV for
            // no overflow and let the program go on), as this returns.
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        } else {
            // A fault: what had it before has it again, and it comes
            // back as its instruction runs again (std's overflow check,
            // then the default).
            let i = SIGNALS.iter().position(|s| *s == sig).unwrap_or(0);
            match BEFORE.get() {
                Some(before) => libc::sigaction(sig, &before[i], std::ptr::null_mut()),
                None => {
                    libc::signal(sig, libc::SIG_DFL);
                    0
                }
            };
        }
    }
}

/// The signal's report, with system calls alone. Out of line, so the
/// handler's own frame stays small: a signal that comes while another's
/// handler runs (std's stack-overflow abort) lands on what's left of the
/// thread's alternate stack.
///
/// # Safety
/// In a signal handler, `setup` made before it was installed.
#[inline(never)]
unsafe fn signal_report(setup: &Setup, sig: libc::c_int, info: &libc::siginfo_t) {
    // SAFETY: each a system call on buffers of ours (async-signal-safe).
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
        let at = ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000;
        let pid = libc::getpid() as u64;
        let tid = libc::syscall(libc::SYS_gettid) as u64;
        let mut thread = [0u8; 17];
        libc::prctl(libc::PR_GET_NAME, thread.as_mut_ptr());
        let thread = &thread[..thread.iter().position(|c| *c == 0).unwrap_or(16)];
        // A fault's address; a sent signal's sender.
        let sent = info.si_code <= 0;
        let address = if sent { 0 } else { info.si_addr() as u64 };
        let sender = if sent { info.si_pid() as u64 } else { 0 };
        let mut name = Buf::<96>::new();
        name.put(setup.program.as_bytes()).put(b"-").num(pid).put(b"-").num(at).put(b".json");
        let mut part = Buf::<320>::new();
        part.put(setup.dir_c.as_bytes()).put(b"/.").put(name.bytes()).put(b"\0");
        let mut to = Buf::<320>::new();
        to.put(setup.dir_c.as_bytes()).put(b"/").put(name.bytes()).put(b"\0");
        // (Kept small: it may run on a thread's alternate signal stack,
        // a few kilobytes.)
        let mut j = Buf::<1280>::new();
        j.put(b"{\"program\":\"").put(setup.program.as_bytes());
        j.put(b"\",\"pid\":").num(pid);
        j.put(b",\"exe\":\"").esc(setup.exe.as_bytes());
        j.put(b"\",\"build\":").num(setup.build);
        j.put(b",\"started_ms\":").num(setup.started_ms);
        j.put(b",\"at_ms\":").num(at);
        j.put(b",\"kind\":\"signal\",\"thread\":\"").esc(thread);
        j.put(b"\",\"tid\":").num(tid);
        j.put(b",\"signal\":\"").put(signal_name(sig));
        j.put(b"\",\"code\":").int(info.si_code as i64);
        if sent {
            j.put(b",\"sent_by\":").num(sender);
        } else {
            j.put(b",\"address\":\"").hex(address).put(b"\"");
        }
        j.put(b",\"core\":\"coredumpctl info ").num(pid).put(b"\"}\n");
        // Each folder of the path, as create_dir_all would.
        let path = setup.dir_c.as_bytes();
        let mut each = Buf::<320>::new();
        for (i, &c) in path.iter().enumerate() {
            if c == b'/' && i > 0 {
                each.put(b"\0");
                libc::mkdir(each.bytes().as_ptr().cast(), 0o700);
                each.n -= 1;
            }
            each.put(&[c]);
        }
        libc::mkdir(setup.dir_c.as_ptr(), 0o700);
        let fd = libc::open(part.bytes().as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o600);
        if fd < 0 {
            return;
        }
        let mut done = 0;
        let bytes = j.bytes();
        while done < bytes.len() {
            let n = libc::write(fd, bytes[done..].as_ptr().cast(), bytes.len() - done);
            if n <= 0 {
                break;
            }
            done += n as usize;
        }
        libc::close(fd);
        libc::rename(part.bytes().as_ptr().cast(), to.bytes().as_ptr().cast());
        for piece in [error_mark(), setup.program.as_bytes(), b": ", signal_name(sig), b"; the report is ", &to.bytes()[..to.n - 1], b"\n"] {
            libc::write(2, piece.as_ptr().cast(), piece.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_cut_off_is_told_apart() {
        assert!(output_cut_off(Some("failed printing to stdout: Broken pipe (os error 32)")));
        assert!(output_cut_off(Some("failed printing to stderr: Broken pipe (os error 32)")));
        assert!(!output_cut_off(Some("failed printing to stdout: No space left on device (os error 28)")));
        assert!(!output_cut_off(Some("Broken pipe")) && !output_cut_off(None));
    }

    #[test]
    fn names_and_numbers_as_written() {
        assert_eq!(program_name(Path::new("/home/me/.local/bin/olo-viewer (deleted)")), "olo-viewer");
        assert_eq!(program_name(Path::new("/x/we\"ird name")), "we_ird_name");
        let mut b = Buf::<64>::new();
        b.num(0).put(b" ").num(18446744073709551615).put(b" ").int(-6).put(b" ").hex(0x7f92_d002_28f7).put(b" ").esc(b"a\"b\\c\n");
        assert_eq!(std::str::from_utf8(b.bytes()).unwrap(), "0 18446744073709551615 -6 0x7f92d00228f7 a\\\"b\\\\c?");
        let mut small = Buf::<4>::new();
        small.put(b"toolong");
        assert_eq!(small.bytes(), b"tool");
        assert_eq!(escape("a\"\n\u{1}"), "a\\\"\\n\\u0001");
    }

    #[test]
    fn only_the_newest_reports_are_kept() {
        let dir = PathBuf::from(format!("/var/tmp/olo-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for at in 0..(KEPT as u64 + 3) {
            std::fs::write(dir.join(format!("olo-viewer-77-{}.json", 1000 + at)), "{}").unwrap();
        }
        std::fs::write(dir.join("olo-tracker-5-1.json"), "{}").unwrap();
        prune(&dir, "olo-viewer");
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left.len(), KEPT + 1, "{left:?}");
        assert!(left.contains(&"olo-tracker-5-1.json".to_owned()));
        assert!(!left.contains(&"olo-viewer-77-1000.json".to_owned()), "the oldest went");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
