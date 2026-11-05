//! What every Olo program leaves when it crashes, so the crash can be
//! looked into after: [`install`], the first thing `main` does
//! (`olo_ui::run` does it for the kit's apps; a test checks every
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

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Be ready for a crash: the report's hook and handlers, and stderr to
/// the journal when it goes nowhere. Once; the first thing `main` does.
pub fn install() {
    if SETUP.get().is_some() {
        return;
    }
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
    let socket = std::env::var_os("OLO_CRASH_JOURNAL").map_or_else(|| PathBuf::from(JOURNAL), PathBuf::from);
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
    ours.sort_by(|a, b| b.0.cmp(&a.0));
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
