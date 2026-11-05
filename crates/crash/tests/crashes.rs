//! Real crashes, in a child (this test binary run again): a panic, a
//! segfault and an abort each leave their report, and stderr that went
//! nowhere reaches the journal's socket (a stand-in's here).

use std::{
    io::Read as _,
    os::unix::{net::UnixListener, process::CommandExt as _, process::ExitStatusExt as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// What the child does, when it's the child.
#[test]
fn child() {
    let Ok(how) = std::env::var("OLO_CRASH_CHILD") else { return };
    // A test's crash: no core, nor systemd-coredump's line about one.
    // SAFETY: prctl on this process.
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) };
    let how = match how.strip_suffix("-plain") {
        Some(plain) => plain.to_owned(),
        None => {
            let t = std::time::Instant::now();
            olo_crash::install();
            if how == "time" {
                println!("installed in {} ns", t.elapsed().as_nanos());
            }
            how
        }
    };
    eprintln!("said before crashing");
    match how.as_str() {
        "panic" => panic!("the child's \"panic\""),
        // SAFETY: not safe at all: the point is the fault.
        "segv" => unsafe {
            std::ptr::read_volatile(std::ptr::null::<u8>().wrapping_add(16));
        },
        "abort" => std::process::abort(),
        "wait" => {
            println!("waiting");
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        "overflow" => {
            fn deeper(n: u64) -> u64 {
                let a = std::hint::black_box([n as u8; 4096]);
                deeper(n + 1) + u64::from(a[0])
            }
            std::hint::black_box(deeper(0));
        }
        "thread" => {
            // A worker's panic: the program goes on.
            let _ = std::thread::Builder::new().name("worker".into()).spawn(|| panic!("in the worker")).unwrap().join();
            eprintln!("still here");
        }
        _ => {}
    }
}

fn run(how: &str, state: &Path, stderr: Stdio, journal: Option<&Path>) -> std::process::ExitStatus {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "child", "--nocapture", "--test-threads", "1"]).env("OLO_CRASH_CHILD", how).env("XDG_STATE_HOME", state).stdout(Stdio::null()).stderr(stderr);
    if let Some(j) = journal {
        c.env("OLO_CRASH_JOURNAL", j);
    }
    // No core for a test's crash.
    // SAFETY: setrlimit is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            libc::setrlimit(libc::RLIMIT_CORE, &none);
            Ok(())
        });
    }
    c.status().unwrap()
}

/// The reports in `state`'s crashes folder, as text.
fn reports(state: &Path) -> Vec<String> {
    let dir = state.join("olo/crashes");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    names.sort();
    names.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect()
}

/// A test's folder in /var/tmp, removed when dropped, passed or not.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let d = PathBuf::from(format!("/var/tmp/olo-crash-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

#[test]
fn a_panic_is_reported_with_its_backtrace() {
    let state = scratch("panic");
    let status = run("panic", &state, Stdio::null(), None);
    assert_eq!(status.code(), Some(101));
    let r = reports(&state);
    assert_eq!(r.len(), 1, "{r:?}");
    for want in ["\"kind\":\"panic\"", "\"thread\":\"child\"", "the child's \\\"panic\\\"", "\"place\":\"crates/crash/tests/crashes.rs:", "\"backtrace\":\"", "\"started_ms\":", "\"build\":"] {
        assert!(r[0].contains(want), "{want} in {}", r[0]);
    }
    // The backtrace names where (release builds keep line tables).
    assert!(r[0].contains("crashes.rs"), "{}", r[0]);
}

#[test]
fn a_worker_panic_is_reported_and_the_program_goes_on() {
    let state = scratch("thread");
    let status = run("thread", &state, Stdio::null(), None);
    assert!(status.success());
    let r = reports(&state);
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].contains("\"thread\":\"worker\"") && r[0].contains("in the worker"), "{}", r[0]);
}

#[test]
fn a_segfault_is_reported_and_still_ends_the_program_as_one() {
    let state = scratch("segv");
    let status = run("segv", &state, Stdio::null(), None);
    assert_eq!(status.signal(), Some(libc::SIGSEGV), "{status:?}");
    let r = reports(&state);
    assert_eq!(r.len(), 1, "{r:?}");
    for want in ["\"kind\":\"signal\"", "\"signal\":\"SIGSEGV\"", "\"address\":\"0x10\"", "\"code\":1", "\"core\":\"coredumpctl info "] {
        assert!(r[0].contains(want), "{want} in {}", r[0]);
    }
}

#[test]
fn an_abort_is_reported_once() {
    let state = scratch("abort");
    let status = run("abort", &state, Stdio::null(), None);
    assert_eq!(status.signal(), Some(libc::SIGABRT), "{status:?}");
    let r = reports(&state);
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].contains("\"signal\":\"SIGABRT\""), "{}", r[0]);
}

#[test]
fn stderr_that_went_nowhere_goes_to_the_journal() {
    let state = scratch("journal");
    let socket = state.join("journal.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let reader = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut got = String::new();
        let _ = s.read_to_string(&mut got);
        got
    });
    let status = run("panic", &state, Stdio::null(), Some(&socket));
    assert_eq!(status.code(), Some(101));
    let got = reader.join().unwrap();
    // The stream's header (named for the program: this test's binary),
    // then what it said, its panic's message and where its report is.
    let mut lines = got.lines();
    assert!(lines.next().unwrap().starts_with("crashes-"), "{got}");
    assert_eq!(lines.by_ref().take(6).collect::<Vec<_>>(), ["", "6", "1", "0", "0", "0"]);
    let rest: Vec<&str> = lines.collect();
    assert!(rest.contains(&"said before crashing"), "{got}");
    assert!(rest.iter().any(|l| l.starts_with("<3>crashes-") && l.contains("the report is")), "marked an error: {got}");
    assert!(rest.iter().any(|l| l.contains("panicked at")), "{got}");
}

#[test]
fn stderr_that_goes_somewhere_is_left_alone() {
    let state = scratch("piped");
    let socket = state.join("journal.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let out = {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "child", "--nocapture"]).env("OLO_CRASH_CHILD", "thread").env("XDG_STATE_HOME", &state).env("OLO_CRASH_JOURNAL", &socket);
        c.output().unwrap()
    };
    assert!(String::from_utf8_lossy(&out.stderr).contains("still here"));
}

#[test]
fn a_stack_overflow_is_reported_once_and_std_still_says_so() {
    let state = scratch("overflow");
    let out = {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "child", "--nocapture", "--test-threads", "1"]).env("OLO_CRASH_CHILD", "overflow").env("XDG_STATE_HOME", &state);
        // SAFETY: setrlimit is async-signal-safe.
        unsafe {
            c.pre_exec(|| {
                libc::setrlimit(libc::RLIMIT_CORE, &libc::rlimit { rlim_cur: 0, rlim_max: 0 });
                Ok(())
            });
        }
        c.output().unwrap()
    };
    assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{:?}", out.status);
    assert!(String::from_utf8_lossy(&out.stderr).contains("has overflowed its stack"), "{}", String::from_utf8_lossy(&out.stderr));
    let r = reports(&state);
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].contains("\"signal\":\"SIGSEGV\""), "{}", r[0]);
}

/// What installing costs at start (`--ignored`): a fresh process each
/// time, its stderr piped (left alone) or /dev/null (sent to a journal
/// socket, a stand-in's), the median of 30.
#[test]
#[ignore]
fn install_cost() {
    let state = scratch("time");
    let socket = state.join("journal.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let _ = std::io::copy(&mut s, &mut std::io::sink());
            });
        }
    });
    for (what, null) in [("stderr piped", false), ("stderr to the journal", true)] {
        let mut ns: Vec<u64> = (0..30)
            .map(|_| {
                let mut c = Command::new(std::env::current_exe().unwrap());
                c.args(["--exact", "child", "--nocapture", "--test-threads", "1"]).env("OLO_CRASH_CHILD", "time").env("XDG_STATE_HOME", &state).env("OLO_CRASH_JOURNAL", &socket);
                c.stderr(if null { Stdio::null() } else { Stdio::piped() });
                let out = c.output().unwrap();
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.split("installed in ").nth(1).and_then(|t| t.split(' ').next()?.parse().ok()).unwrap()
            })
            .collect();
        ns.sort();
        println!("{what}: median {} µs, max {} µs", ns[15] / 1000, ns[29] / 1000);
    }
}

#[test]
fn a_sent_segv_ends_the_program_and_names_its_sender() {
    let state = scratch("sent");
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "child", "--nocapture", "--test-threads", "1"]).env("OLO_CRASH_CHILD", "wait").env("XDG_STATE_HOME", &state).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    // Up and waiting (it says so), then sent SIGSEGV as `kill -SEGV` does.
    let mut out = child.stdout.take().unwrap();
    let mut buf = [0u8; 64];
    let mut got = Vec::new();
    while !String::from_utf8_lossy(&got).contains("waiting") {
        let n = out.read(&mut buf).unwrap();
        assert!(n > 0);
        got.extend_from_slice(&buf[..n]);
    }
    // SAFETY: kill on our own child.
    unsafe { libc::kill(child.id() as i32, libc::SIGSEGV) };
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGSEGV), "{status:?}");
    let r = reports(&state);
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].contains(&format!("\"sent_by\":{}", std::process::id())) && !r[0].contains("address"), "{}", r[0]);
}
