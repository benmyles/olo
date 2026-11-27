//! Agent Screens of a host's own (`olo-rex-link screen`, and `olo screen`
//! on a host, apps/rex/src/bridge_olo.sh): which of the two is asked, the
//! host's or the desktop's, and what the helper does with an Olo that is
//! here. An Olo made up for the test: a prefix whose `olo-screen` says
//! what it was run with. (A release put under a prefix from its archive
//! is olo-release's own test; a real screen started is the dev server's
//! check, HACKING, "Agent Screens of a host's own".)

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const HELPER: &str = env!("CARGO_BIN_EXE_olo-rex-link");
const OLO: &str = include_str!("../../../apps/rex/src/bridge_olo.sh");

struct Host {
    dir: PathBuf,
}

impl Host {
    fn new(name: &str) -> Host {
        let dir = std::env::temp_dir().join(format!("olo-host-screens-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let script = |path: &Path, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script(&dir.join("olo"), OLO);
        // (The desktop, as far as `olo` goes: `rex do`, which says what
        // it was asked and ends as told.)
        script(&dir.join("rex"), "#!/bin/sh\necho \"desktop: $OLO_BRIDGE_CMD $OLO_BRIDGE_ARG_1 $OLO_BRIDGE_ARG_2 ($OLO_BRIDGE_ARGC)\"\nexit ${DESKTOP_SAYS:-0}\n");
        Host { dir }
    }

    fn prefix(&self) -> PathBuf {
        self.dir.join("desktop")
    }

    /// Olo put here, as far as the helper looks: its screen tool (which
    /// says what it was run with, and lists `screens`) and its version.
    fn with_olo(&self, screens: &str) {
        let bin = self.prefix().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(self.prefix().join("share/olo")).unwrap();
        std::fs::write(self.prefix().join("share/olo/version"), "9-test\n").unwrap();
        let tool = bin.join("olo-screen");
        std::fs::write(&tool, format!("#!/bin/sh\nif [ \"$1\" = list ]; then echo '{screens}'; exit 0; fi\necho \"host: $* (first on PATH: ${{PATH%%:*}})\"\n")).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// An Olo whose screen tool starts and stops something, as the real
    /// one does a session: `start NAME` leaves a process in a group of
    /// its own and prints the screen (its name and that process), and
    /// `stop NAME` writes that it was asked and ends that process (the
    /// session alone, as the real one asks its session to end: not the
    /// group, where the helper's keeper is).
    fn with_olo_that_starts(&self) {
        self.with_olo("[]");
        let tool = self.prefix().join("bin/olo-screen");
        let dir = self.dir.display();
        std::fs::write(
            &tool,
            format!(
                "#!/bin/sh\ncase \"$1\" in\nstart)\n\tset -m\n\tsleep 300 > /dev/null 2>&1 < /dev/null &\n\techo $! > '{dir}/session-'$2\n\tprintf '{{\"name\": \"%s\", \"pid\": %s}}\\n' \"$2\" \"$!\"\n\t;;\nstop)\n\techo \"stop $2\" >> '{dir}/asked'\n\tkill -TERM \"$(cat '{dir}/session-'$2)\" 2> /dev/null\n\t;;\nesac\n"
            ),
        )
        .unwrap();
    }

    /// What the screen tool was asked to stop, so far.
    fn asked(&self) -> String {
        std::fs::read_to_string(self.dir.join("asked")).unwrap_or_default()
    }

    /// The process a made-up screen `name` is, if it still runs.
    fn session(&self, name: &str) -> Option<i32> {
        let pid: i32 = std::fs::read_to_string(self.dir.join(format!("session-{name}"))).ok()?.trim().parse().ok()?;
        // (Not one that has ended and waits to be reaped by nobody.)
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        (!stat.rsplit_once(')')?.1.trim_start().starts_with('Z')).then_some(pid)
    }

    /// A Rex tab's program, as far as the helper can tell: something
    /// started with a `REX_BLOCK` of its own, which starts screen `name`
    /// with the helper (`flags` besides) and then waits to be told to
    /// end (its standard input closed). Returned once the screen is
    /// started.
    fn tab_that_starts(&self, name: &str, flags: &str, block: Option<&str>) -> std::process::Child {
        let mut tab = self.command(Path::new("/bin/sh"));
        tab.arg("-c").arg(format!("'{HELPER}' screen start {name} {flags} > '{}/started-{name}'; read line", self.dir.display())).stdin(Stdio::piped());
        match block {
            Some(block) => tab.env("REX_BLOCK", block),
            None => tab.env_remove("REX_BLOCK"),
        };
        let tab = tab.spawn().unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(self.dir.join(format!("started-{name}"))).is_ok_and(|t| !t.contains("\"pid\"")) || !self.dir.join(format!("started-{name}")).exists() {
            assert!(Instant::now() < until, "screen {name} wasn't started within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        tab
    }

    fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        c.env_remove("OLO_SCREENS_ON")
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("home/.config"))
            .env("XDG_STATE_HOME", self.dir.join("home/.local/state"))
            .env("XDG_DATA_HOME", self.dir.join("home/.local/share"))
            .env("OLO_HOST_DESKTOP", self.prefix())
            .env("OLO_REX_LINK", HELPER)
            .env("OLO_BRIDGE_REX", self.dir.join("rex"))
            .env("XDG_RUNTIME_DIR", &self.dir);
        c
    }

    fn helper(&self, args: &[&str]) -> Output {
        self.command(Path::new(HELPER)).arg("screen").args(args).output().unwrap()
    }

    fn olo(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = self.command(&self.dir.join("olo"));
        c.args(args).envs(env.iter().copied());
        c.output().unwrap()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // (Whatever made-up screens a test left, a failed one too: each
        // a process in a group of its own.)
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if let Some(name) = entry.file_name().to_string_lossy().strip_prefix("session-")
                && let Some(session) = self.session(name)
            {
                let _ = Command::new("/bin/kill").args(["-TERM", "--", &format!("-{session}")]).status();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).trim().to_owned()
}

/// With no Olo here, the helper says so and how to put it; `status` says
/// the same as data. With one, its `olo-screen` is run with what was
/// asked, its own programs first on the path.
#[test]
fn the_helper_runs_the_olo_that_is_here() {
    let host = Host::new("helper");
    let none = host.helper(&["start", "work"]);
    assert_eq!(none.status.code(), Some(1));
    assert!(err(&none).contains("Olo isn't on this host yet") && err(&none).contains("olo screen install"), "{}", err(&none));
    let status: serde_json::Value = serde_json::from_str(&out(&host.helper(&["status"]))).unwrap();
    assert_eq!((status["installed"].clone(), status["how"].as_str()), (serde_json::Value::Null, Some("olo screen install")), "{status}");
    assert_eq!(status["default"], "desktop");

    host.with_olo("[]");
    let ran = host.helper(&["start", "work", "--size", "800x600"]);
    assert!(ran.status.success(), "{}", err(&ran));
    assert_eq!(out(&ran), format!("host: start work --size 800x600 (first on PATH: {})", host.prefix().join("bin").display()));
    let status: serde_json::Value = serde_json::from_str(&out(&host.helper(&["status"]))).unwrap();
    assert_eq!(status["installed"], "9-test", "{status}");
    assert_eq!(status["missing"], serde_json::json!([]), "a script asks its loader for nothing: {status}");
    assert!(status.get("how").is_none() || status["software_drawing"] == false, "{status}");
    // The window that shows screens is the desktop's.
    let watch = host.helper(&["watch"]);
    assert_eq!(watch.status.code(), Some(1));
    assert!(err(&watch).contains("from the Olo desktop"), "{}", err(&watch));
}

/// Olo is taken off a host again, but never from under a screen that
/// runs.
#[test]
fn olo_is_not_removed_under_a_running_screen() {
    let host = Host::new("remove");
    host.with_olo(r#"[{"name":"work","state":"running"},{"name":"old","state":"stopped"}]"#);
    let held = host.helper(&["remove"]);
    assert_eq!(held.status.code(), Some(1));
    assert!(err(&held).contains("a screen still runs here (work)") && err(&held).contains("work") && !err(&held).contains("old"), "{}", err(&held));
    assert!(host.prefix().join("bin/olo-screen").is_file());
    host.with_olo(r#"[{"name":"old","state":"stopped"}]"#);
    let gone = host.helper(&["remove"]);
    assert!(gone.status.success(), "{}", err(&gone));
    assert!(!host.prefix().exists());
    assert!(out(&host.helper(&["remove"])).contains("nothing to take off"));
}

/// `olo screen` on a host: the desktop's unless told (`--on`, then
/// `OLO_SCREENS_ON`, then the host's default), with `--on` and its word
/// taken out of what is passed on either way; what puts Olo here and
/// says what is here is the host's own whichever is asked; and where the
/// desktop can't be asked, the other way is said.
#[test]
fn olo_screen_goes_where_it_is_told() {
    let host = Host::new("olo");
    host.with_olo("[]");
    let on_host = |o: &Output| out(o).starts_with("host: ");
    let said = |o: &Output| out(o);

    // Not told: the desktop, as it always was.
    assert_eq!(said(&host.olo(&["screen", "start", "work"], &[])), "desktop: screen start work (2)");
    // Told, each way; the nearer wins.
    let flag = host.olo(&["screen", "--on", "host", "start", "work"], &[]);
    assert!(on_host(&flag) && said(&flag).contains("host: start work (first"), "{} {}", said(&flag), err(&flag));
    assert!(on_host(&host.olo(&["screen", "look", "work"], &[("OLO_SCREENS_ON", "host")])));
    assert_eq!(said(&host.olo(&["screen", "--on", "desktop", "look", "work"], &[("OLO_SCREENS_ON", "host")])), "desktop: screen look work (2)");
    assert_eq!(said(&host.olo(&["screen", "default"], &[])), "desktop");
    assert_eq!(said(&host.olo(&["screen", "default", "host"], &[])), "host");
    assert!(on_host(&host.olo(&["screen", "tree", "work"], &[])), "the host's default");
    assert_eq!(said(&host.olo(&["screen", "tree", "work"], &[("OLO_SCREENS_ON", "desktop")])), "desktop: screen tree work (2)");
    assert_eq!(said(&host.olo(&["screen", "default", "desktop"], &[])), "desktop");
    // The host's own whichever is asked.
    let status = host.olo(&["screen", "--on", "desktop", "status"], &[]);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&said(&status)).unwrap()["installed"], "9-test");
    // Neither word.
    let moon = host.olo(&["screen", "--on", "moon", "list"], &[]);
    assert_eq!((moon.status.code(), err(&moon).as_str()), (Some(2), "olo screen: --on takes host or desktop, not `moon`"));
    assert_eq!(host.olo(&["screen", "--on"], &[]).status.code(), Some(2));
    // A helper from before it had screens takes `screen` for a program
    // to run, and a host may have one of that name that answers and
    // succeeds: never run so; said, with the other way.
    let old = host.dir.join("old-helper");
    std::fs::write(&old, "#!/bin/sh\necho 'Use: screen [-opts] [cmd [args]]'\nexit 0\n").unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = host.olo(&["screen", "--on", "host", "list"], &[("OLO_REX_LINK", old.to_str().unwrap())]);
    assert_eq!(before.status.code(), Some(69), "{} {}", said(&before), err(&before));
    assert!(said(&before).is_empty() && err(&before).contains("no screens of its own yet"), "{}", err(&before));
    // No desktop to ask: said with the way that needs none (of a screen
    // only: the desktop's other commands have no other way).
    let away = host.olo(&["screen", "start", "work"], &[("DESKTOP_SAYS", "69")]);
    assert_eq!(away.status.code(), Some(69));
    assert!(err(&away).contains("olo screen --on host"), "{}", err(&away));
    let open = host.olo(&["open", "http://localhost:1"], &[("DESKTOP_SAYS", "69")]);
    assert_eq!((open.status.code(), err(&open).as_str()), (Some(69), ""));
}

/// Olo isn't changed under screens that run: `install` says which run
/// and leaves it (nothing is fetched: the archive named here isn't even
/// there), unless told to go ahead.
#[test]
fn olo_is_not_changed_under_a_running_screen() {
    let host = Host::new("held");
    host.with_olo(r#"[{"name":"work","state":"running"},{"name":"old","state":"stopped"}]"#);
    let held = host.helper(&["install", "--from", "/nowhere/olo-9-x86_64.tar.zst", "--no-chrome", "--no-emoji-font"]);
    let lines: Vec<serde_json::Value> = out(&held).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0]["held"], serde_json::json!(["work"]), "{}", out(&held));
    assert_eq!((lines[0]["installed"].as_str(), lines[0]["changed"].as_u64()), (Some("9-test"), Some(0)));
    assert!(lines[0]["how"].as_str().unwrap().contains("--under-screens"));
    // (4: left as it is for the screens; 3 where this host couldn't
    // draw one anyway, which is said first.)
    let lacking = lines.last().unwrap()["software_drawing"] == false;
    assert_eq!(held.status.code(), Some(if lacking { 3 } else { 4 }));
    // Told to go ahead, it tries (and here fails on the archive that isn't there).
    let ahead = host.helper(&["install", "--from", "/nowhere/olo-9-x86_64.tar.zst", "--under-screens", "--no-chrome", "--no-emoji-font"]);
    assert_eq!(ahead.status.code(), Some(1));
    assert!(out(&ahead).contains("\"error\""), "{}", out(&ahead));
    // With none running, the same.
    host.with_olo(r#"[{"name":"old","state":"stopped"}]"#);
    assert_eq!(host.helper(&["install", "--from", "/nowhere/olo-9-x86_64.tar.zst", "--no-chrome", "--no-emoji-font"]).status.code(), Some(1));
}

/// A screen started from a Rex tab is stopped when the tab's program
/// ends, and not before; one started with `--keep`, or from no Rex tab,
/// stays until it is stopped.
#[test]
fn a_screen_ends_with_the_tab_that_started_it() {
    let host = Host::new("tab");
    host.with_olo_that_starts();
    let block = format!("block:test-{}", std::process::id());
    let within = |seconds: u64, what: &str, done: &dyn Fn() -> bool| {
        let until = Instant::now() + Duration::from_secs(seconds);
        while !done() {
            assert!(Instant::now() < until, "{what} (not within {seconds} s)");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // (A keeper is the helper again: `screen keep-until NAME …`.)
    let keeper_of = |name: &str| {
        std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|p| {
            let line = std::fs::read(p.path().join("cmdline")).unwrap_or_default();
            let words: Vec<&[u8]> = line.split(|b| *b == 0).collect();
            words.windows(2).any(|w| w[0] == b"keep-until" && w[1] == name.as_bytes())
        })
    };
    let mut tab = host.tab_that_starts("work", "", Some(&block));
    let mut kept = host.tab_that_starts("kept", "--keep", Some(&block));
    let mut bare = host.tab_that_starts("bare", "", None);
    let mut by_hand = host.tab_that_starts("byhand", "", Some(&block));
    // While its tab's program runs, a screen runs: nothing is asked.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(host.asked(), "");
    assert!(["work", "kept", "bare", "byhand"].iter().all(|name| host.session(name).is_some()));
    assert!(keeper_of("work") && keeper_of("byhand") && !keeper_of("kept") && !keeper_of("bare"));

    // One stopped by hand while its tab's program still runs: its
    // keeper has nothing left to do, and ends.
    assert!(host.helper(&["stop", "byhand"]).status.success());
    within(10, "a screen stopped by hand takes its keeper with it", &|| !keeper_of("byhand"));
    assert_eq!(host.asked(), "stop byhand\n");

    // The tabs' programs end (told to: their input closes).
    for tab in [&mut tab, &mut kept, &mut bare, &mut by_hand] {
        drop(tab.stdin.take());
        tab.wait().unwrap();
    }
    within(10, "the screen started from the tab is stopped when the tab's program ends", &|| host.asked() == "stop byhand\nstop work\n");
    within(10, "and what ran on it has ended, its keeper too", &|| host.session("work").is_none() && !keeper_of("work"));
    // The others stay, and the one stopped by hand isn't stopped again
    // (whatever had its name by now): nothing more is asked.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(host.asked(), "stop byhand\nstop work\n");
    for name in ["kept", "bare"] {
        assert!(host.session(name).is_some(), "{name} was stopped with its tab");
    }
    // (The made-up screens it leaves are ended as the host is dropped.)
}

// ---------------------------------------------------------------------------
// Live streaming, zero-polling `follow`, and `NAME.owner.json` tests
// ---------------------------------------------------------------------------

mod live_and_follow {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use olo_rex_link::screen_wire::{
        ClientMsg, ExportMsg, FollowEvent, HistoryFrameMark, HistoryMsg, HistoryRequest, HostScreenState, OwnerPane, PROTO, ScreenOwner, ServerMsg, read_export_msg, read_follow_event,
        read_history_msg, read_server_msg, write_client_msg, write_history_request,
    };
    use olo_screen_codec::{
        Frame, Writer as OlofWriter,
        olof::{frame_at as olof_frame_at, index_dir as olof_index_dir},
        tiles::{EncodedDelta, apply_delta},
    };

    const HELPER: &str = env!("CARGO_BIN_EXE_olo-rex-link");

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-screens-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Pure-Rust `olo-session` relay invoked by `SessionStub` with
    /// `arg0("olo-session")` and `OLO_SCREEN=<name>` so `SO_PEERCRED` on every
    /// test Unix socket genuinely belongs to the screen's pinned session PID.
    #[test]
    #[ignore]
    fn fake_session_relay() {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut line = String::new();
        let mut reader = BufReader::new(stdin.lock());
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let trimmed = line.trim();
            if let Some((listen_s, backend_s)) = trimmed.split_once('\t') {
                let listen = PathBuf::from(listen_s);
                let backend = PathBuf::from(backend_s);
                let _ = std::fs::remove_file(&listen);
                let listener = UnixListener::bind(&listen).expect("relay bind");
                let _ = writeln!(stdout, "ready");
                let _ = stdout.flush();
                std::thread::spawn(move || {
                    for conn in listener.incoming().flatten() {
                        let backend_p = backend.clone();
                        std::thread::spawn(move || {
                            let Ok(upstream) = UnixStream::connect(&backend_p) else { return };
                            let (Ok(mut c_rd), Ok(mut u_wr)) = (conn.try_clone(), upstream.try_clone()) else { return };
                            let t = std::thread::spawn(move || {
                                let _ = std::io::copy(&mut c_rd, &mut u_wr);
                                let _ = u_wr.shutdown(std::net::Shutdown::Write);
                            });
                            let (mut u_rd, mut c_wr) = (upstream, conn);
                            let _ = std::io::copy(&mut u_rd, &mut c_wr);
                            let _ = c_wr.shutdown(std::net::Shutdown::Both);
                            let _ = t.join();
                        });
                    }
                });
            }
            line.clear();
        }
    }

    struct SessionStub {
        child: Child,
        stdin: Option<std::process::ChildStdin>,
        stdout: BufReader<std::process::ChildStdout>,
    }

    impl SessionStub {
        fn start(name: &str) -> Self {
            use std::os::unix::process::CommandExt as _;
            let exe = std::env::current_exe().unwrap();
            let mut child = Command::new(exe)
                .arg0("olo-session")
                .args(["--ignored", "--exact", "live_and_follow::fake_session_relay", "--nocapture"])
                .env("OLO_SCREEN", name)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let stdin = child.stdin.take();
            let stdout = BufReader::new(child.stdout.take().unwrap());
            Self { child, stdin, stdout }
        }

        fn pid(&self) -> i32 {
            self.child.id() as i32
        }

        fn bind_socket(&mut self, listen: &Path, backend: &Path) {
            let stdin = self.stdin.as_mut().expect("session stdin open");
            writeln!(stdin, "{}\t{}", listen.display(), backend.display()).unwrap();
            stdin.flush().unwrap();
            let mut line = String::new();
            loop {
                line.clear();
                let n = self.stdout.read_line(&mut line).unwrap();
                assert!(n > 0, "fake_session_relay exited before reporting ready");
                if line.trim() == "ready" {
                    break;
                }
            }
        }

        fn kill_and_wait(&mut self) {
            self.stdin.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl Drop for SessionStub {
        fn drop(&mut self) {
            self.kill_and_wait();
        }
    }

    struct Cleanup(PathBuf, Vec<SessionStub>);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.1.clear();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Stand-in for a screen's `app_server` Unix socket: supports
    /// `subscribe_to` (`painted`), `screenshot` (raw RGBA file), and
    /// `synthetic` (recording received events).
    struct FakeAppServer {
        socket: PathBuf,
        backend: PathBuf,
        frame: Arc<Mutex<(u32, u32, Vec<u8>)>>,
        subscribers: Arc<Mutex<Vec<UnixStream>>>,
        screenshots: Arc<AtomicUsize>,
        inputs: Arc<(Mutex<Vec<serde_json::Value>>, Condvar)>,
        batch_sizes: Arc<Mutex<Vec<usize>>>,
        corrupt_shots: Arc<AtomicUsize>,
        shot_error: Arc<Mutex<Option<String>>>,
        synthetic_gate: Arc<(Mutex<(bool, usize)>, Condvar)>,
        synthetic_error_count: Arc<AtomicUsize>,
        drop_after_synthetic: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    }

    impl FakeAppServer {
        fn start_in(stub: &mut SessionStub, socket: PathBuf, width: u32, height: u32, rgba: Vec<u8>) -> Self {
            Self::start_in_with_opts(stub, socket, width, height, rgba, false, 0)
        }

        fn start_in_with_opts(stub: &mut SessionStub, socket: PathBuf, width: u32, height: u32, rgba: Vec<u8>, reject_painted_flag: bool, corrupt_shot_count: usize) -> Self {
            static BACKEND_SEQ: AtomicUsize = AtomicUsize::new(0);
            let seq = BACKEND_SEQ.fetch_add(1, Ordering::Relaxed);
            let backend = socket.with_file_name(format!(".backend-{}-{seq}.sock", std::process::id()));
            let _ = std::fs::remove_file(&backend);
            let listener = UnixListener::bind(&backend).unwrap();
            listener.set_nonblocking(true).unwrap();
            stub.bind_socket(&socket, &backend);

            let frame = Arc::new(Mutex::new((width, height, rgba)));
            let subscribers: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
            let screenshots = Arc::new(AtomicUsize::new(0));
            let inputs = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
            let batch_sizes = Arc::new(Mutex::new(Vec::new()));
            let reject_painted = Arc::new(AtomicBool::new(reject_painted_flag));
            let corrupt_shots = Arc::new(AtomicUsize::new(corrupt_shot_count));
            let shot_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let synthetic_gate = Arc::new((Mutex::new((false, 0usize)), Condvar::new()));
            let synthetic_error_count = Arc::new(AtomicUsize::new(0));
            let drop_after_synthetic = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));

            let (f, subs, shots, ins, bsz, rej, cor, se, sg, sec, das, st) = (
                frame.clone(),
                subscribers.clone(),
                screenshots.clone(),
                inputs.clone(),
                batch_sizes.clone(),
                reject_painted,
                corrupt_shots.clone(),
                shot_error.clone(),
                synthetic_gate.clone(),
                synthetic_error_count.clone(),
                drop_after_synthetic.clone(),
                stop.clone(),
            );
            std::thread::spawn(move || {
                while !st.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            let (f, subs, shots, ins, bsz, rej, cor, se, sg, sec, das, st) =
                                (f.clone(), subs.clone(), shots.clone(), ins.clone(), bsz.clone(), rej.clone(), cor.clone(), se.clone(), sg.clone(), sec.clone(), das.clone(), st.clone());
                            std::thread::spawn(move || handle_client(stream, f, subs, shots, ins, bsz, rej, cor, se, sg, sec, das, st));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self { socket, backend, frame, subscribers, screenshots, inputs, batch_sizes, corrupt_shots, shot_error, synthetic_gate, synthetic_error_count, drop_after_synthetic, stop }
        }

        fn set_corrupt_shots(&self, n: usize) {
            self.corrupt_shots.store(n, Ordering::SeqCst);
        }

        fn set_shot_error(&self, msg: Option<&str>) {
            *self.shot_error.lock().unwrap() = msg.map(str::to_owned);
        }

        fn hold_synthetic(&self, hold: bool) {
            let mut g = self.synthetic_gate.0.lock().unwrap();
            g.0 = hold;
            if !hold {
                self.synthetic_gate.1.notify_all();
            }
        }

        fn wait_synthetic_blocked(&self, timeout: Duration) -> bool {
            let until = Instant::now() + timeout;
            let mut g = self.synthetic_gate.0.lock().unwrap();
            while g.1 == 0 {
                let Some(rem) = until.checked_duration_since(Instant::now()) else { return false };
                let (next, _) = self.synthetic_gate.1.wait_timeout(g, rem).unwrap();
                g = next;
            }
            true
        }

        fn disconnect_subscribers(&self) {
            let mut subs = self.subscribers.lock().unwrap();
            for s in subs.drain(..) {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }

        fn batch_sizes(&self) -> Vec<usize> {
            self.batch_sizes.lock().unwrap().clone()
        }

        fn set_synthetic_error_count(&self, n: usize) {
            self.synthetic_error_count.store(n, Ordering::SeqCst);
        }

        fn set_drop_after_synthetic(&self, on: bool) {
            self.drop_after_synthetic.store(on, Ordering::SeqCst);
        }

        fn set_frame(&self, width: u32, height: u32, rgba: Vec<u8>) {
            *self.frame.lock().unwrap() = (width, height, rgba);
        }

        fn paint(&self) {
            let mut subs = self.subscribers.lock().unwrap();
            subs.retain_mut(|s| s.write_all(b"{\"type\":\"event\",\"event\":{\"type\":\"painted\",\"output\":\"HEADLESS-1\"}}\n").is_ok());
        }

        fn wait_subscriber(&self, timeout: Duration) {
            let until = Instant::now() + timeout;
            while self.subscribers.lock().unwrap().is_empty() {
                assert!(Instant::now() < until, "stream did not subscribe to painted events");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn screenshot_count(&self) -> usize {
            self.screenshots.load(Ordering::SeqCst)
        }

        fn received_inputs(&self) -> Vec<serde_json::Value> {
            self.inputs.0.lock().unwrap().clone()
        }

        fn wait_inputs(&self, at_least: usize, timeout: Duration) -> Vec<serde_json::Value> {
            let until = Instant::now() + timeout;
            let mut g = self.inputs.0.lock().unwrap();
            while g.len() < at_least {
                let Some(rem) = until.checked_duration_since(Instant::now()) else { break };
                let (next, _) = self.inputs.1.wait_timeout(g, rem).unwrap();
                g = next;
            }
            g.clone()
        }

        fn shutdown(&self) {
            self.stop.store(true, Ordering::SeqCst);
            self.hold_synthetic(false);
            let mut subs = self.subscribers.lock().unwrap();
            for s in subs.drain(..) {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            let _ = std::fs::remove_file(&self.socket);
            let _ = std::fs::remove_file(&self.backend);
        }
    }

    impl Drop for FakeAppServer {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_client(
        mut stream: UnixStream,
        frame: Arc<Mutex<(u32, u32, Vec<u8>)>>,
        subscribers: Arc<Mutex<Vec<UnixStream>>>,
        screenshots: Arc<AtomicUsize>,
        inputs: Arc<(Mutex<Vec<serde_json::Value>>, Condvar)>,
        batch_sizes: Arc<Mutex<Vec<usize>>>,
        reject_painted: Arc<AtomicBool>,
        corrupt_shots: Arc<AtomicUsize>,
        shot_error: Arc<Mutex<Option<String>>>,
        synthetic_gate: Arc<(Mutex<(bool, usize)>, Condvar)>,
        synthetic_error_count: Arc<AtomicUsize>,
        drop_after_synthetic: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    ) {
        let Ok(reader_stream) = stream.try_clone() else { return };
        let mut reader = BufReader::new(reader_stream);
        let mut line = String::new();
        while !stop.load(Ordering::Relaxed) {
            line.clear();
            let Ok(n) = reader.read_line(&mut line) else { break };
            if n == 0 {
                break;
            }
            let Ok(req) = serde_json::from_str::<serde_json::Value>(line.trim()) else { break };
            match req["type"].as_str() {
                Some("subscribe_to") => {
                    if reject_painted.load(Ordering::SeqCst) {
                        let _ = stream.write_all(b"{\"type\":\"error\",\"message\":\"unknown event\"}\n");
                        continue;
                    }
                    if stream.write_all(b"{\"type\":\"ok\"}\n").is_err() {
                        break;
                    }
                    if let Ok(clone) = stream.try_clone() {
                        subscribers.lock().unwrap().push(clone);
                    }
                }
                Some("screenshot") => {
                    let Some(path) = req["path"].as_str() else { break };
                    screenshots.fetch_add(1, Ordering::SeqCst);
                    if let Some(msg) = shot_error.lock().unwrap().clone() {
                        let reply = serde_json::json!({"type": "error", "message": msg});
                        if stream.write_all(format!("{reply}\n").as_bytes()).is_err() {
                            break;
                        }
                        continue;
                    }
                    let remaining = corrupt_shots.try_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1));
                    if remaining.is_ok() {
                        // Write truncated RGBA bytes so read/decode fails transiently.
                        let _ = std::fs::write(path, [10u8, 0, 0, 0, 10, 0, 0, 0, 1, 2]);
                    } else {
                        let (w, h, rgba) = frame.lock().unwrap().clone();
                        let mut bytes = Vec::with_capacity(8 + rgba.len());
                        bytes.extend_from_slice(&w.to_le_bytes());
                        bytes.extend_from_slice(&h.to_le_bytes());
                        bytes.extend_from_slice(&rgba);
                        let tmp = format!("{path}.part");
                        std::fs::write(&tmp, &bytes).unwrap();
                        std::fs::rename(&tmp, path).unwrap();
                    }
                    if stream.write_all(b"{\"type\":\"ok\"}\n").is_err() {
                        break;
                    }
                }
                Some("synthetic") => {
                    {
                        let mut g = synthetic_gate.0.lock().unwrap();
                        if g.0 {
                            g.1 += 1;
                            synthetic_gate.1.notify_all();
                            while g.0 && !stop.load(Ordering::Relaxed) {
                                g = synthetic_gate.1.wait(g).unwrap();
                            }
                            g.1 = g.1.saturating_sub(1);
                        }
                    }
                    if synthetic_error_count.try_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1)).is_ok() {
                        if stream.write_all(b"{\"type\":\"error\",\"message\":\"bad request\"}\n").is_err() {
                            break;
                        }
                        continue;
                    }
                    if let Some(arr) = req["events"].as_array() {
                        batch_sizes.lock().unwrap().push(arr.len());
                        let mut g = inputs.0.lock().unwrap();
                        g.extend(arr.iter().cloned());
                        inputs.1.notify_all();
                    }
                    if drop_after_synthetic.swap(false, Ordering::SeqCst) {
                        // Simulate compositor closing socket right after applying the click.
                        break;
                    }
                    if stream.write_all(b"{\"type\":\"ok\"}\n").is_err() {
                        break;
                    }
                }
                _ => {
                    let _ = stream.write_all(b"{\"type\":\"ok\"}\n");
                }
            }
        }
    }

    fn spawn_session_stub(name: &str) -> Child {
        use std::os::unix::process::CommandExt as _;
        Command::new("/bin/sleep").arg0("olo-session").arg("60").env("OLO_SCREEN", name).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
    }

    fn write_screen_json(rt_dir: &Path, name: &str, pid: i32, socket: &Path, w: u32, h: u32) {
        let screens_dir = rt_dir.join("olo-screens");
        std::fs::create_dir_all(&screens_dir).unwrap();
        let json = serde_json::json!({
            "name": name,
            "wayland_display": format!("wayland-{name}"),
            "olo_socket": socket,
            "pid": pid,
            "width": w,
            "height": h,
            "scale": 1.0,
            "sound": false,
            "started": 1_795_716_120u64,
        });
        let tmp = screens_dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(&json).unwrap()).unwrap();
        std::fs::rename(&tmp, screens_dir.join(format!("{name}.json"))).unwrap();
    }

    fn make_rgba(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = ((x as u8).wrapping_add(seed)) & 0xfe;
                rgba[i + 1] = ((y as u8).wrapping_mul(3).wrapping_add(seed)) & 0xfe;
                rgba[i + 2] = 0x40;
                rgba[i + 3] = 0xff;
            }
        }
        rgba
    }

    fn decode_and_apply(delta_bytes: &[u8], canvas: &mut Frame) -> EncodedDelta {
        let delta = EncodedDelta::decode_bytes(delta_bytes).unwrap();
        apply_delta(&delta, canvas).unwrap();
        delta
    }

    fn spawn_stream(dir: &Path, name: &str) -> Child {
        Command::new(HELPER)
            .args(["screen", "stream", name])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", dir.join("run"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// Spawn a background reader for `ServerMsg` so tests can assert timeouts
    /// cleanly without blocking forever.
    fn server_msg_channel(mut stdout: std::process::ChildStdout) -> std::sync::mpsc::Receiver<Option<ServerMsg>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                match read_server_msg(&mut stdout) {
                    Ok(Some(msg)) => {
                        if tx.send(Some(msg)).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = tx.send(None);
                        break;
                    }
                }
            }
        });
        rx
    }

    fn follow_event_channel(stdout: std::process::ChildStdout) -> std::sync::mpsc::Receiver<Option<FollowEvent>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_follow_event(&mut reader) {
                    Ok(Some(ev)) => {
                        if tx.send(Some(ev)).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = tx.send(None);
                        break;
                    }
                }
            }
        });
        rx
    }

    #[test]
    fn stream_idle_sends_nothing_and_painted_rebuilds_picture_exactly() {
        let dir = scratch_dir("stream-basic");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (160u32, 96u32);
        let frame1 = make_rgba(w, h, 10);
        let sock = rt.join("fake-basic.sock");
        let mut stub = SessionStub::start("live");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, frame1.clone());
        write_screen_json(&rt, "live", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);

        let mut child = spawn_stream(&dir, "live");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());

        server.wait_subscriber(Duration::from_secs(5));

        // 1. Initial keyframe rebuilds frame1 exactly.
        let ServerMsg::Frame { seq, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected initial keyframe");
        };
        assert_eq!(seq, 1);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        let d1 = decode_and_apply(&delta, &mut canvas);
        assert!(d1.keyframe);
        assert_eq!((canvas.width as u32, canvas.height as u32), (w, h));
        assert_eq!(canvas.rgba, frame1);
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();

        // 2. At rest: no painted event -> 0 screenshots, 0 frames sent.
        let shots_before = server.screenshot_count();
        assert!(rx.recv_timeout(Duration::from_millis(350)).is_err(), "unexpected message at rest");
        assert_eq!(server.screenshot_count(), shots_before, "no captures at rest");

        // 3. Painted event after changing two tiles -> single delta frame rebuilding frame2 exactly.
        let mut frame2 = frame1.clone();
        // Pixel in tile (0, 0) and pixel in edge tile (128, 64).
        frame2[(10 * w as usize + 12) * 4..][..4].copy_from_slice(&[200, 10, 20, 255]);
        frame2[(80 * w as usize + 150) * 4..][..4].copy_from_slice(&[30, 210, 40, 255]);
        server.set_frame(w, h, frame2.clone());
        server.paint();

        let ServerMsg::Frame { seq, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected delta frame 2");
        };
        assert_eq!(seq, 2);
        let d2 = decode_and_apply(&delta, &mut canvas);
        assert!(!d2.keyframe);
        assert_eq!(d2.rects.len(), 2, "only the 2 changed tiles should be sent: {:?}", d2.rects);
        assert_eq!(canvas.rgba, frame2);
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 2 }).unwrap();

        // 4. Painted event with identical pixels -> 0 changed tiles, nothing sent.
        server.paint();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "identical frame should send nothing");

        // 5. Pause suppresses captures until unpaused (ordered via a Control barrier, no fixed sleep).
        let ctl_file = rt.join("olo-screens/live.control");
        write_client_msg(&mut stdin, &ClientMsg::Pause { paused: true }).unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Control { on: true }).unwrap();
        let my_pid = child.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_file).ok().as_deref() != Some(my_pid.as_str()) {
            assert!(Instant::now() < until, "barrier Control(true) after Pause(true) was not applied");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(Some(ServerMsg::Control { held: true, .. }))), "expected Control(true) ack");
        let mut frame3 = frame2.clone();
        frame3[(5 * w as usize + 5) * 4..][..4].copy_from_slice(&[99, 88, 77, 255]);
        server.set_frame(w, h, frame3.clone());
        server.paint();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "paused stream must not send frames");

        write_client_msg(&mut stdin, &ClientMsg::Pause { paused: false }).unwrap();
        let ServerMsg::Frame { seq, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected frame 3 after resume");
        };
        assert_eq!(seq, 3);
        decode_and_apply(&delta, &mut canvas);
        assert_eq!(canvas.rgba, frame3);

        // 6. Clean EOF exits promptly and removes the private 0700 capture directory.
        let stream_pid = child.id();
        drop(stdin);
        let status = wait_child_timeout(&mut child, Duration::from_secs(5));
        assert!(status.success(), "stream exit: {status}");
        let views = rt.join("olo-screens/views");
        let leftovers: Vec<_> = std::fs::read_dir(&views).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().contains(&format!("-{stream_pid}-"))).collect();
        assert!(leftovers.is_empty(), "private capture directory must be removed on exit: {leftovers:?}");
    }

    #[test]
    fn stream_caps_in_flight_at_two_until_ack() {
        let dir = scratch_dir("stream-backpressure");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let sock = rt.join("fake-bp.sock");
        let mut stub = SessionStub::start("bp");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 1));
        write_screen_json(&rt, "bp", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);

        let mut child = spawn_stream(&dir, "bp");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        server.wait_subscriber(Duration::from_secs(5));

        // Frame 1 (unacked).
        let ServerMsg::Frame { seq: s1, delta: d1 } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(s1, 1);

        // Frame 2 (unacked) -> now 2 in flight.
        server.set_frame(w, h, make_rgba(w, h, 2));
        server.paint();
        let ServerMsg::Frame { seq: s2, delta: d2 } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(s2, 2);

        // Frame 3 triggered while 2 are unacked -> MUST NOT be sent.
        let frame3 = make_rgba(w, h, 3);
        server.set_frame(w, h, frame3.clone());
        server.paint();
        std::thread::sleep(Duration::from_millis(120));
        server.paint();
        assert!(rx.recv_timeout(Duration::from_millis(350)).is_err(), "third frame must wait for an ack");

        // Ack frame 1 -> frame 3 arrives immediately with latest pixels.
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();
        let ServerMsg::Frame { seq: s3, delta: d3 } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(s3, 3);

        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        for d in [d1, d2, d3] {
            decode_and_apply(&d, &mut canvas);
        }
        assert_eq!(canvas.rgba, frame3);

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
    }

    #[test]
    fn stream_gates_input_on_control_and_cleans_control_on_eof_and_sigterm() {
        let dir = scratch_dir("stream-control");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let sock = rt.join("fake-ctl.sock");
        let mut stub = SessionStub::start("ctl");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 5));
        write_screen_json(&rt, "ctl", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_file = rt.join("olo-screens/ctl.control");

        // Part A: Input gating + cleanup on stdin EOF.
        let mut child = spawn_stream(&dir, "ctl");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();

        // Input while control is NOT held must be dropped (review1 change 10).
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"key","key":30,"pressed":true}]"#.into() }).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(server.received_inputs().is_empty(), "input must be dropped when control is not held");
        assert!(!ctl_file.exists());

        // Take control -> wait for published PID (never racing on empty file, Finding 35).
        write_client_msg(&mut stdin, &ClientMsg::Control { on: true }).unwrap();
        let pid_a_str = child.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_file).ok().as_deref() != Some(pid_a_str.as_str()) {
            assert!(Instant::now() < until, "ctl.control did not publish pid {pid_a_str}");
            std::thread::sleep(Duration::from_millis(10));
        }

        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"move","x":1.0,"y":2.0},{"type":"move","x":5.0,"y":6.0},{"type":"key","key":30,"pressed":true}]"#.into() }).unwrap();
        let got = server.wait_inputs(2, Duration::from_secs(3));
        assert_eq!(got.len(), 2, "expected coalesced move + key: {got:?}");
        assert_eq!(got[0], serde_json::json!({"type": "move", "x": 5.0, "y": 6.0}));
        assert_eq!(got[1], serde_json::json!({"type": "key", "key": 30, "pressed": true}));

        // Release control -> held key 30 is released automatically, control file removed, subsequent input dropped.
        write_client_msg(&mut stdin, &ClientMsg::Control { on: false }).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while ctl_file.exists() {
            assert!(Instant::now() < until, "ctl.control was not removed on Control(false)");
            std::thread::sleep(Duration::from_millis(10));
        }
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"key","key":31,"pressed":true}]"#.into() }).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let received = server.received_inputs();
        assert_eq!(received.len(), 3, "expected move, key 30 down, key 30 auto-release (key 31 dropped): {received:?}");
        assert_eq!(received[2], serde_json::json!({"type": "key", "key": 30, "pressed": false}));

        // Take control again, then close stdin -> ctl.control must disappear!
        write_client_msg(&mut stdin, &ClientMsg::Control { on: true }).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_file).ok().as_deref() != Some(pid_a_str.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
        assert!(!ctl_file.exists(), "ctl.control must be removed on stdin EOF");

        // Part B: SIGTERM while holding control must also remove ctl.control & temp capture dir!
        let mut child_b = spawn_stream(&dir, "ctl");
        let mut stdin_b = child_b.stdin.take().unwrap();
        let rx_b = server_msg_channel(child_b.stdout.take().unwrap());
        let _ = rx_b.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut stdin_b, &ClientMsg::Control { on: true }).unwrap();
        let pid_b = child_b.id();
        let pid_b_str = pid_b.to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_file).ok().as_deref() != Some(pid_b_str.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        // SAFETY: sending SIGTERM to the child process we spawned above.
        unsafe {
            libc::kill(pid_b as i32, libc::SIGTERM);
        }
        let status = wait_child_timeout(&mut child_b, Duration::from_secs(5));
        assert!(status.success(), "expected clean exit after SIGTERM, got {status}");
        drop(stdin_b);
        assert!(!ctl_file.exists(), "ctl.control must be removed on SIGTERM");
        let views = rt.join("olo-screens/views");
        let leftovers: Vec<_> = std::fs::read_dir(&views).into_iter().flatten().flatten().collect();
        assert!(leftovers.is_empty(), "temp capture dirs must be removed on SIGTERM: {leftovers:?}");
    }

    #[test]
    fn stream_reconnects_after_reload_and_reports_screen_gone() {
        let dir = scratch_dir("stream-reconnect");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let sock1 = rt.join("fake-rec-1.sock");
        let mut stub = SessionStub::start("recon");
        let server1 = FakeAppServer::start_in(&mut stub, sock1.clone(), w, h, make_rgba(w, h, 11));
        write_screen_json(&rt, "recon", stub.pid(), &sock1, w, h);

        let mut child = spawn_stream(&dir, "recon");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        let ServerMsg::Frame { seq, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(seq, 1);
        assert!(EncodedDelta::decode_bytes(&delta).unwrap().keyframe);
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();

        // Simulate app_server reload inside the SAME olo-session: new socket + updated NAME.json, then drop old server.
        let sock2 = rt.join("fake-rec-2.sock");
        let frame_after = make_rgba(w, h, 77);
        let server2 = FakeAppServer::start_in(&mut stub, sock2.clone(), w, h, frame_after.clone());
        write_screen_json(&rt, "recon", stub.pid(), &sock2, w, h);
        server1.shutdown();

        let mut saw_reconnecting = false;
        let mut rebuilt = None;
        let until = Instant::now() + Duration::from_secs(6);
        while Instant::now() < until {
            let msg = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
            match msg {
                ServerMsg::Status(s) if s.reconnecting => saw_reconnecting = true,
                ServerMsg::Frame { seq, delta } if seq > 1 => {
                    let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
                    let d = decode_and_apply(&delta, &mut canvas);
                    if d.keyframe {
                        rebuilt = Some(canvas.rgba);
                        write_client_msg(&mut stdin, &ClientMsg::Ack { seq }).unwrap();
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(saw_reconnecting, "expected Reconnecting status during reload");
        assert_eq!(rebuilt.as_deref(), Some(frame_after.as_slice()));

        // Stop screen completely -> expect ScreenGone and clean exit.
        stub.kill_and_wait();
        let _ = std::fs::remove_file(rt.join("olo-screens/recon.json"));
        server2.shutdown();

        let mut saw_gone = false;
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(8)).unwrap() {
                Some(ServerMsg::ScreenGone { .. }) => {
                    saw_gone = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(saw_gone, "expected ScreenGone after screen stopped");
        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn follow_watches_empty_home_folders_appearing_lifecycle_and_debounces_last_used() {
        let dir = scratch_dir("follow");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let config = home.join(".config");
        let desktop = home.join(".local/share/olo/desktop");
        // Deliberately start with ONLY empty base roots — none of the screens/olo folders exist yet!
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &config)
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let rx = follow_event_channel(child.stdout.take().unwrap());

        // 1. Initial Snapshot on empty home.
        let snap = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let FollowEvent::Snapshot { proto, status, screens } = snap else {
            panic!("expected initial snapshot, got {snap:?}");
        };
        assert_eq!(proto, PROTO);
        assert_eq!(status.installed, None);
        assert!(!status.default_on_host);
        assert!(screens.is_empty());

        // 2. Host Olo folders appear later -> Status event with version & committed timestamp.
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(desktop.join("share/olo")).unwrap();
        std::fs::write(desktop.join("share/olo/version"), "2026.11.27-abc123\n").unwrap();
        std::fs::write(desktop.join("share/olo/release.json"), r#"{"committed":1795715000}"#).unwrap();
        std::fs::write(desktop.join("bin/olo-screen"), "#!/bin/sh\n").unwrap();

        let wait_for = |pred: &dyn Fn(&FollowEvent) -> bool, label: &str| -> FollowEvent {
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
                if pred(&ev) {
                    return ev;
                }
            }
            panic!("timed out waiting for {label}");
        };

        let st = wait_for(&|ev| matches!(ev, FollowEvent::Status { status } if status.installed.as_deref() == Some("2026.11.27-abc123")), "Olo install status");
        let FollowEvent::Status { status } = st else { unreachable!() };
        assert_eq!(status.committed, Some(1_795_715_000));
        assert!(!status.default_on_host);

        // 3. Default file folder appears later -> default_on_host flips true.
        std::fs::create_dir_all(config.join("olo")).unwrap();
        std::fs::write(config.join("olo/screens-on"), "host\n").unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Status { status } if status.default_on_host), "default_on_host=true");

        // 4. Screens folders appear later + screen started with owner.json -> Screen event.
        let mut stub = spawn_session_stub("work");
        let sdir = state.join("olo/screens/work");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":1440,"height":900,"scale":1.0,"sound":false}"#).unwrap();
        let owner = ScreenOwner {
            host_key: None,
            session_id: "sess-1".into(),
            block_id: "block-2".into(),
            started_by: Some(OwnerPane { host_key: None, session_id: "sess-1".into(), block_id: "block-1".into() }),
            pid: Some(stub.id() as i32),
            began: Some(12345),
            rex_socket: None,
            at: 1_795_716_000_000,
            ..Default::default()
        };
        let rt_screens = rt.join("olo-screens");
        std::fs::create_dir_all(&rt_screens).unwrap();
        std::fs::write(rt_screens.join("work.owner.json"), serde_json::to_vec(&owner).unwrap()).unwrap();
        std::fs::write(sdir.join("owner.json"), serde_json::to_vec(&owner).unwrap()).unwrap();
        write_screen_json(&rt, "work", stub.id() as i32, &rt.join("work.sock"), 1440, 900);

        let ev = wait_for(&|ev| matches!(ev, FollowEvent::Screen { screen } if screen.name == "work" && screen.state == HostScreenState::Running && screen.owner.is_some()), "work screen running");
        let FollowEvent::Screen { screen } = ev else { unreachable!() };
        assert_eq!(screen.owner.as_ref().map(|o| o.block_id.as_str()), Some("block-2"));
        assert_eq!(screen.started_by.as_ref().map(|o| o.block_id.as_str()), Some("block-1"));

        // 5. Title changed -> Screen event with new title.
        std::fs::write(sdir.join("title"), "Checkout QA\n").unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Screen { screen } if screen.title.as_deref() == Some("Checkout QA")), "title change");

        // 6. Burst of 20 actions over 300 ms -> at most 2 LastUsed lines/s, ending with final timestamp!
        let actions_path = sdir.join("actions.jsonl");
        let burst_start = Instant::now();
        for i in 1..=20u64 {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&actions_path).unwrap();
            writeln!(f, "{{\"at\":{},\"ms\":10,\"kind\":\"click\"}}", i * 1000).unwrap();
            std::thread::sleep(Duration::from_millis(15));
        }
        let mut last_used_events = Vec::new();
        let collect_until = burst_start + Duration::from_millis(950);
        while Instant::now() < collect_until {
            let rem = collect_until.saturating_duration_since(Instant::now());
            match rx.recv_timeout(rem) {
                Ok(Some(FollowEvent::LastUsed { name, last_used, .. })) if name == "work" => last_used_events.push(last_used),
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!((1..=2).contains(&last_used_events.len()), "expected 1..=2 debounced LastUsed lines in <1s, got {last_used_events:?}");
        assert_eq!(last_used_events.last().copied(), Some(20_010), "final action timestamp must be delivered: {last_used_events:?}");

        // 7. Session killed abruptly (without removing work.json) -> pidfd wakes follow immediately!
        let _ = stub.kill();
        let _ = stub.wait();
        wait_for(&|ev| matches!(ev, FollowEvent::Screen { screen } if screen.name == "work" && screen.state == HostScreenState::Stopped), "stopped via pidfd");

        // 8. Deleted -> Gone event.
        let _ = std::fs::remove_file(rt_screens.join("work.json"));
        let _ = std::fs::remove_dir_all(&sdir);
        wait_for(&|ev| matches!(ev, FollowEvent::Gone { name } if name == "work"), "gone event");

        // 9. Stdin EOF -> exits immediately.
        let t0 = Instant::now();
        drop(stdin);
        let status = wait_child_timeout(&mut child, Duration::from_secs(2));
        assert!(status.success(), "follow exit: {status}");
        assert!(t0.elapsed() < Duration::from_secs(2), "follow took {:?} to exit on stdin EOF", t0.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn owner_file_written_on_start_and_not_rewritten_for_same_pane() {
        let dir = scratch_dir("owner");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(desktop.join("share/olo")).unwrap();
        std::fs::write(desktop.join("share/olo/version"), "9-test\n").unwrap();

        // Stand-in `olo-screen`: creates NAME.json and state/NAME/screen.json on `start`.
        let tool = desktop.join("bin/olo-screen");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = start ]; then\n\
             mkdir -p '{rt}/olo-screens' '{state}/olo/screens/'\"$2\"\n\
             printf '{{\"name\":\"%s\",\"wayland_display\":\"w\",\"olo_socket\":\"/tmp/x\",\"pid\":1,\"width\":1440,\"height\":900,\"scale\":1.0,\"sound\":false,\"started\":1}}\\n' \"$2\" > '{rt}/olo-screens/'\"$2\".json\n\
             printf '{{\"width\":1440,\"height\":900,\"scale\":1.0,\"sound\":false}}\\n' > '{state}/olo/screens/'\"$2\"/screen.json\n\
             printf '{{\"name\":\"%s\",\"pid\":1}}\\n' \"$2\"\n\
             fi\n",
            rt = rt.display(),
            state = state.display(),
        );
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run_screen = |args: &[&str], session: &str, block: &str| {
            let out = Command::new(HELPER)
                .arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("REX_SESSION", session)
                .env("REX_BLOCK", block)
                .output()
                .unwrap();
            assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        };

        // Start from pane A -> writes both runtime and state owner files.
        run_screen(&["start", "qa", "--keep"], "sess-1", "block-A");
        let rt_owner = rt.join("olo-screens/qa.owner.json");
        let st_owner = state.join("olo/screens/qa/owner.json");
        let parsed: ScreenOwner = serde_json::from_slice(&std::fs::read(&rt_owner).unwrap()).unwrap();
        assert_eq!((parsed.session_id.as_str(), parsed.block_id.as_str()), ("sess-1", "block-A"));
        assert_eq!(parsed.started_by.as_ref().map(|p| p.block_id.as_str()), Some("block-A"));
        assert_eq!(std::fs::read(&rt_owner).unwrap(), std::fs::read(&st_owner).unwrap());

        let meta_before = (
            std::fs::metadata(&rt_owner).unwrap().ino(),
            std::fs::metadata(&rt_owner).unwrap().mtime_nsec(),
            std::fs::metadata(&st_owner).unwrap().ino(),
            std::fs::metadata(&st_owner).unwrap().mtime_nsec(),
            parsed.at,
        );

        // Action from the SAME pane -> neither file is rewritten (review1 change 8).
        std::thread::sleep(Duration::from_millis(30));
        run_screen(&["click", "qa", "10", "20"], "sess-1", "block-A");
        let after_same: ScreenOwner = serde_json::from_slice(&std::fs::read(&rt_owner).unwrap()).unwrap();
        let meta_after = (
            std::fs::metadata(&rt_owner).unwrap().ino(),
            std::fs::metadata(&rt_owner).unwrap().mtime_nsec(),
            std::fs::metadata(&st_owner).unwrap().ino(),
            std::fs::metadata(&st_owner).unwrap().mtime_nsec(),
            after_same.at,
        );
        assert_eq!(meta_before, meta_after, "owner file must NOT be rewritten by a second action from the same pane");

        // Action from a DIFFERENT pane -> updates current owner while preserving started_by.
        run_screen(&["click", "qa", "30", "40"], "sess-1", "block-B");
        let after_other: ScreenOwner = serde_json::from_slice(&std::fs::read(&rt_owner).unwrap()).unwrap();
        assert_eq!((after_other.session_id.as_str(), after_other.block_id.as_str()), ("sess-1", "block-B"));
        assert_eq!(after_other.started_by.as_ref().map(|p| p.block_id.as_str()), Some("block-A"));
        let st_other: ScreenOwner = serde_json::from_slice(&std::fs::read(&st_owner).unwrap()).unwrap();
        assert_eq!(st_other.block_id, "block-B");
        assert_eq!(st_other.started_by.as_ref().map(|p| p.block_id.as_str()), Some("block-A"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn history_msg_channel(mut stdout: std::process::ChildStdout) -> std::sync::mpsc::Receiver<Option<HistoryMsg>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                match read_history_msg(&mut stdout) {
                    Ok(Some(msg)) => {
                        if tx.send(Some(msg)).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = tx.send(None);
                        break;
                    }
                }
            }
        });
        rx
    }

    fn export_msg_channel(mut stdout: std::process::ChildStdout) -> std::sync::mpsc::Receiver<Option<ExportMsg>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                match read_export_msg(&mut stdout) {
                    Ok(Some(msg)) => {
                        if tx.send(Some(msg)).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = tx.send(None);
                        break;
                    }
                }
            }
        });
        rx
    }

    /// Review 1a #1: control is taken only if free (flock before writing);
    /// a second viewer's `Control(true)` is refused with a `Status`, leaves the
    /// first's file untouched, and keeps dropping input until the first releases.
    /// A stale unlocked control file is treated as free.
    #[test]
    fn stream_control_refused_when_held_by_another_stream_and_acquired_after_release() {
        let dir = scratch_dir("ctl-lock");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let sock = rt.join("fake-lock.sock");
        let mut stub = SessionStub::start("lock");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 7));
        write_screen_json(&rt, "lock", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_path = rt.join("olo-screens/lock.control");

        // Pre-seed a stale unlocked control file: first stream must treat it as free.
        std::fs::write(&ctl_path, "999999").unwrap();

        let mut c1 = spawn_stream(&dir, "lock");
        let mut in1 = c1.stdin.take().unwrap();
        let rx1 = server_msg_channel(c1.stdout.take().unwrap());
        let _ = rx1.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in1, &ClientMsg::Ack { seq: 1 }).unwrap();

        let mut c2 = spawn_stream(&dir, "lock");
        let mut in2 = c2.stdin.take().unwrap();
        let rx2 = server_msg_channel(c2.stdout.take().unwrap());
        let _ = rx2.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in2, &ClientMsg::Ack { seq: 1 }).unwrap();

        // Stream 1 takes control over the stale unlocked file.
        write_client_msg(&mut in1, &ClientMsg::Control { on: true }).unwrap();
        let p1 = c1.id().to_string();
        let until = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p1.as_str()) {
            assert!(Instant::now() < until, "stream 1 did not acquire control");
            std::thread::sleep(Duration::from_millis(10));
        }

        // Stream 2 asks for control while stream 1 holds it -> refused with Status,
        // stream 1's control file untouched, and stream 2's input stays dropped.
        write_client_msg(&mut in2, &ClientMsg::Control { on: true }).unwrap();
        let mut saw_refusal = false;
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if let Ok(Some(ServerMsg::Control { held: false, reason: Some(msg), .. })) = rx2.recv_timeout(Duration::from_secs(2)) {
                assert!(msg.contains("Someone else is controlling"), "unexpected refusal reason: {msg:?}");
                saw_refusal = true;
                break;
            }
        }
        assert!(saw_refusal, "expected refusal Status for stream 2");
        assert_eq!(std::fs::read_to_string(&ctl_path).unwrap(), p1, "stream 1's control file must not be overwritten");

        write_client_msg(&mut in2, &ClientMsg::Input { json_events: r#"[{"type":"key","key":30,"pressed":true}]"#.into() }).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(server.received_inputs().is_empty(), "refused stream 2 input must be dropped");

        // Stream 1 releases -> stream 2 asks again and acquires control + input works!
        write_client_msg(&mut in1, &ClientMsg::Control { on: false }).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while ctl_path.exists() {
            assert!(Instant::now() < until, "stream 1 did not remove control file");
            std::thread::sleep(Duration::from_millis(10));
        }

        write_client_msg(&mut in2, &ClientMsg::Control { on: true }).unwrap();
        let p2 = c2.id().to_string();
        let until = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p2.as_str()) {
            assert!(Instant::now() < until, "stream 2 did not acquire control after stream 1 released");
            std::thread::sleep(Duration::from_millis(10));
        }
        write_client_msg(&mut in2, &ClientMsg::Input { json_events: r#"[{"type":"key","key":30,"pressed":true},{"type":"key","key":30,"pressed":false}]"#.into() }).unwrap();
        let got = server.wait_inputs(2, Duration::from_secs(3));
        assert_eq!(got.len(), 2);

        drop(in1);
        drop(in2);
        assert!(wait_child_timeout(&mut c1, Duration::from_secs(5)).success());
        assert!(wait_child_timeout(&mut c2, Duration::from_secs(5)).success());
        assert!(!ctl_path.exists());
    }

    /// Review 1a #2: keys and buttons left down are released when control ends
    /// (EOF, `Control(false)`, or app_server reconnect), and nothing extra is
    /// sent when everything was already released.
    #[test]
    fn stream_releases_held_keys_and_buttons_on_eof_control_end_and_reload() {
        let dir = scratch_dir("held-rel");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let sock = rt.join("fake-rel.sock");
        let mut stub = SessionStub::start("rel");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 3));
        write_screen_json(&rt, "rel", stub.pid(), &sock, w, h);
        let ctl_path = rt.join("olo-screens/rel.control");

        // A: press key 42 + button 272, then close stdin -> both released before exit!
        let mut child = spawn_stream(&dir, "rel");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Control { on: true }).unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"key","key":42,"pressed":true},{"type":"button","button":272,"pressed":true}]"#.into() }).unwrap();
        let got = server.wait_inputs(2, Duration::from_secs(3));
        assert_eq!(got.len(), 2);

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
        assert!(!ctl_path.exists());

        let all = server.wait_inputs(4, Duration::from_secs(3));
        assert_eq!(all.len(), 4, "expected 2 presses + 2 synthetic releases on EOF, got {all:?}");
        assert!(all[2..].contains(&serde_json::json!({"type":"key","key":42,"pressed":false})), "missing key release: {all:?}");
        assert!(all[2..].contains(&serde_json::json!({"type":"button","button":272,"pressed":false})), "missing button release: {all:?}");

        // B: press and release key 30 cleanly -> closing stdin sends NOTHING extra!
        let before_b = server.received_inputs().len();
        let mut child_b = spawn_stream(&dir, "rel");
        let mut stdin_b = child_b.stdin.take().unwrap();
        let rx_b = server_msg_channel(child_b.stdout.take().unwrap());
        let _ = rx_b.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut stdin_b, &ClientMsg::Ack { seq: 1 }).unwrap();
        write_client_msg(&mut stdin_b, &ClientMsg::Control { on: true }).unwrap();
        write_client_msg(&mut stdin_b, &ClientMsg::Input { json_events: r#"[{"type":"key","key":30,"pressed":true},{"type":"key","key":30,"pressed":false}]"#.into() }).unwrap();
        let got_b = server.wait_inputs(before_b + 2, Duration::from_secs(3));
        assert_eq!(got_b.len(), before_b + 2);
        drop(stdin_b);
        assert!(wait_child_timeout(&mut child_b, Duration::from_secs(5)).success());
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(server.received_inputs().len(), before_b + 2, "no extra releases when nothing was left down");

        // C: press key 56, then `Control(false)` -> release sent before control ends!
        let before_c = server.received_inputs().len();
        let mut child_c = spawn_stream(&dir, "rel");
        let mut stdin_c = child_c.stdin.take().unwrap();
        let rx_c = server_msg_channel(child_c.stdout.take().unwrap());
        let _ = rx_c.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut stdin_c, &ClientMsg::Ack { seq: 1 }).unwrap();
        write_client_msg(&mut stdin_c, &ClientMsg::Control { on: true }).unwrap();
        write_client_msg(&mut stdin_c, &ClientMsg::Input { json_events: r#"[{"type":"key","key":56,"pressed":true}]"#.into() }).unwrap();
        let _ = server.wait_inputs(before_c + 1, Duration::from_secs(3));
        write_client_msg(&mut stdin_c, &ClientMsg::Control { on: false }).unwrap();
        let after_c = server.wait_inputs(before_c + 2, Duration::from_secs(3));
        assert_eq!(after_c.last(), Some(&serde_json::json!({"type":"key","key":56,"pressed":false})));

        // D: press key 29, then app_server reloads -> control is revoked synchronously
        // BEFORE Reconnecting (Finding 7), dropping `.control` and never leaking stale
        // held-input state into the reloaded compositor.
        write_client_msg(&mut stdin_c, &ClientMsg::Control { on: true }).unwrap();
        write_client_msg(&mut stdin_c, &ClientMsg::Input { json_events: r#"[{"type":"key","key":29,"pressed":true}]"#.into() }).unwrap();
        let _ = server.wait_inputs(before_c + 3, Duration::from_secs(3));
        // Drain earlier Control(true)/Control(false)/Control(true) acks from Parts C & D before shutdown.
        while rx_c.try_recv().is_ok() {}
        server.shutdown();
        let ctl_file = rt.join("olo-screens/rel.control");
        let until = Instant::now() + Duration::from_secs(3);
        let mut saw_revoked = false;
        while Instant::now() < until {
            match rx_c.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(ServerMsg::Control { held: false, .. })) => {
                    saw_revoked = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_revoked, "expected Control(false) revocation when app_server disconnects");
        assert!(!ctl_file.exists(), "control lock must be dropped before Reconnecting");
        let server2 = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 9));
        server2.wait_subscriber(Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(100));
        assert!(server2.received_inputs().is_empty(), "revoked control must not inject stale inputs after reload");

        drop(stdin_c);
        assert!(wait_child_timeout(&mut child_c, Duration::from_secs(5)).success());
        drop(stub);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review 1a #3: an app_server that rejects `subscribe_to painted` is never
    /// polled — the helper sends a clear `Status` asking to update Olo, takes
    /// zero screenshots, and exits.
    #[test]
    fn stream_rejects_app_server_without_painted_without_polling() {
        let dir = scratch_dir("no-painted");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let sock = rt.join("fake-old.sock");
        let mut stub = SessionStub::start("old");
        let server = FakeAppServer::start_in_with_opts(&mut stub, sock.clone(), w, h, make_rgba(w, h, 1), true, 0);
        write_screen_json(&rt, "old", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);

        let mut child = spawn_stream(&dir, "old");
        let _stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());

        let msg = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        match msg {
            ServerMsg::Status(st) => {
                assert!(st.error, "expected error status: {st:?}");
                assert!(st.message.contains("Update Olo on this host"), "unexpected message: {st:?}");
            }
            other => panic!("expected update Status, got {other:?}"),
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None, "stream should end after status");
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
        assert_eq!(server.screenshot_count(), 0, "must never capture or poll without painted events");
    }

    /// Review 1a #4: transient screenshot read/decode failures never drop a
    /// paint — the helper retries and delivers the picture, and persistent
    /// failures report an error `Status`.
    #[test]
    fn stream_retries_lost_capture_read_and_recovers_paint() {
        let dir = scratch_dir("retry-read");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();

        let (w, h) = (128u32, 64u32);
        let frame1 = make_rgba(w, h, 11);
        let sock = rt.join("fake-retry.sock");
        // Corrupt the first 2 screenshots, succeed on the 3rd!
        let mut stub = SessionStub::start("retry");
        let server = FakeAppServer::start_in_with_opts(&mut stub, sock.clone(), w, h, frame1.clone(), false, 2);
        write_screen_json(&rt, "retry", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);

        let mut child = spawn_stream(&dir, "retry");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());

        let ServerMsg::Frame { seq, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected recovered initial frame");
        };
        assert_eq!(seq, 1);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        decode_and_apply(&delta, &mut canvas);
        assert_eq!(canvas.rgba, frame1);
        assert_eq!(server.screenshot_count(), 3, "2 failed reads + 1 successful retry");
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();

        // Persistent corruption (> 3 retries) emits a non-fatal `Warning` (Finding 7).
        server.set_corrupt_shots(10);
        server.paint();
        let msg = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        match msg {
            ServerMsg::Warning { message } => {
                assert!(message.contains("picture"), "unexpected warning: {message:?}");
            }
            other => panic!("expected read-failure Warning, got {other:?}"),
        }

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
    }

    /// Step 1b: `olo-rex-link screen history NAME` follows appends from before
    /// the screen folder exists, sends nothing at rest, reports resets, serves
    /// bit-exact `frame_at` deltas via `olo-screen-codec`, and drops overtaken
    /// requests when three arrive at once.
    #[test]
    fn history_follows_appends_idle_silence_reset_and_frame_at_latest_wins() {
        let dir = scratch_dir("history");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "history-stream", "rec"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let rx = history_msg_channel(child.stdout.take().unwrap());

        // 1. Initial empty snapshot before the screen folder exists.
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(HistoryMsg::Actions { reset: true, lines: vec![] }));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(HistoryMsg::Index { reset: true, last_size: None, frames: vec![] }));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }));

        // 2. Folder appears + 2 actions + 3 recorded frames via `olo_screen_codec::Writer`.
        let sdir = state.join("olo/screens/rec");
        std::fs::create_dir_all(&sdir).unwrap();
        let (w, h) = (160usize, 96usize);
        let f1 = Frame { width: w, height: h, rgba: make_rgba(w as u32, h as u32, 1) };
        let mut f2 = f1.clone();
        f2.rgba[(10 * w + 10) * 4..][..4].copy_from_slice(&[240, 20, 20, 255]);
        let mut f3 = f2.clone();
        f3.rgba[(70 * w + 130) * 4..][..4].copy_from_slice(&[20, 240, 20, 255]);
        let mut writer = OlofWriter::new(sdir.clone(), 64 << 20);
        writer.add(f1.clone(), 1_000).unwrap();
        writer.add(f2.clone(), 2_000).unwrap();
        writer.add(f3.clone(), 3_000).unwrap();

        let actions_path = sdir.join("actions.jsonl");
        let a1 = r#"{"at":1000,"ms":20,"kind":"start","args":[]}"#;
        let a2 = r#"{"at":2000,"ms":15,"kind":"click","args":["10","10"]}"#;
        std::fs::write(&actions_path, format!("{a1}\n{a2}\n")).unwrap();

        let mut got_actions: Vec<String> = Vec::new();
        let mut got_marks: Vec<HistoryFrameMark> = Vec::new();
        let mut got_size: Option<(u32, u32)> = None;
        let until = Instant::now() + Duration::from_secs(5);
        while (got_actions.len() < 2 || got_marks.len() < 3) && Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap() {
                HistoryMsg::Actions { reset, lines } => {
                    if reset {
                        got_actions = lines;
                    } else {
                        got_actions.extend(lines);
                    }
                }
                HistoryMsg::Index { reset, last_size, frames } => {
                    got_size = last_size;
                    if reset {
                        got_marks = frames;
                    } else {
                        got_marks.extend(frames);
                    }
                }
                HistoryMsg::SnapshotDone { .. } => {}
                other => panic!("unexpected msg during initial append: {other:?}"),
            }
        }
        // Drain any trailing SnapshotDone from folder creation before asserting idle silence.
        while matches!(rx.recv_timeout(Duration::from_millis(50)), Ok(Some(HistoryMsg::SnapshotDone { .. }))) {}
        assert_eq!(got_actions, vec![a1.to_owned(), a2.to_owned()]);
        assert_eq!(got_size, Some((w as u32, h as u32)));
        assert_eq!(got_marks, vec![HistoryFrameMark { at: 1_000, key: true }, HistoryFrameMark { at: 2_000, key: false }, HistoryFrameMark { at: 3_000, key: false }]);

        // 3. At rest: nothing sent.
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "unexpected history traffic at rest");

        // 4. Append 1 more action and 1 more frame -> only the additions arrive (`reset: false`).
        let mut f4 = f3.clone();
        f4.rgba[(20 * w + 80) * 4..][..4].copy_from_slice(&[20, 20, 240, 255]);
        writer.add(f4.clone(), 4_000).unwrap();
        let a3 = r#"{"at":4000,"ms":10,"kind":"type","args":["hi"]}"#;
        std::fs::OpenOptions::new().append(true).open(&actions_path).unwrap().write_all(format!("{a3}\n").as_bytes()).unwrap();

        let mut saw_act_add = false;
        let mut saw_idx_add = false;
        let until = Instant::now() + Duration::from_secs(5);
        while (!saw_act_add || !saw_idx_add) && Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap() {
                HistoryMsg::Actions { reset, lines } => {
                    assert!(!reset, "append must have reset=false");
                    assert_eq!(lines, vec![a3.to_owned()]);
                    saw_act_add = true;
                }
                HistoryMsg::Index { reset, last_size, frames } => {
                    assert!(!reset, "frame append must have reset=false");
                    assert_eq!(last_size, Some((w as u32, h as u32)));
                    assert_eq!(frames, vec![HistoryFrameMark { at: 4_000, key: false }]);
                    saw_idx_add = true;
                }
                other => panic!("unexpected msg during incremental append: {other:?}"),
            }
        }
        assert!(saw_act_add && saw_idx_add);

        // 5. `frame_at`: Miss before first, keyframe on first hit, tile delta on second hit,
        //    each matching `olo_screen_codec::olof::frame_at` bit-for-bit.
        let segs = olof_index_dir(&sdir);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };

        write_history_request(&mut stdin, &HistoryRequest::FrameAt { req: 1, at: 500 }).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(HistoryMsg::Miss { req: 1 }));

        write_history_request(&mut stdin, &HistoryRequest::FrameAt { req: 2, at: 1_500 }).unwrap();
        let HistoryMsg::Frame { req, frame_at, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected frame for req 2");
        };
        assert_eq!((req, frame_at), (2, 1_000));
        let d2 = decode_and_apply(&delta, &mut canvas);
        assert!(d2.keyframe, "first frame_at answer must be a keyframe");
        assert_eq!((frame_at, canvas.clone()), olof_frame_at(&segs, 1_500).unwrap());

        write_history_request(&mut stdin, &HistoryRequest::FrameAt { req: 3, at: 4_000 }).unwrap();
        let HistoryMsg::Frame { req, frame_at, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected frame for req 3");
        };
        assert_eq!((req, frame_at), (3, 4_000));
        let d3 = decode_and_apply(&delta, &mut canvas);
        assert!(!d3.keyframe, "subsequent same-size frame_at answer must be a delta");
        assert!(d3.rects.len() < d2.rects.len(), "delta should only carry changed tiles");
        assert_eq!((frame_at, canvas.clone()), olof_frame_at(&segs, 4_000).unwrap());

        // 6. Three requests sent at once -> ONLY the last (`req: 12`) is answered!
        let burst = "{\"op\":\"frame_at\",\"req\":10,\"at\":1000}\n{\"op\":\"frame_at\",\"req\":11,\"at\":3000}\n{\"op\":\"frame_at\",\"req\":12,\"at\":2500}\n";
        stdin.write_all(burst.as_bytes()).unwrap();
        stdin.flush().unwrap();
        let HistoryMsg::Frame { req, frame_at, delta } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected single frame answer for burst");
        };
        assert_eq!((req, frame_at), (12, 2_000), "only the latest request (12) must be answered");
        decode_and_apply(&delta, &mut canvas);
        assert_eq!((frame_at, canvas.clone()), olof_frame_at(&segs, 2_500).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "overtaken requests must not be answered");

        // 7. Reset: shorten `actions.jsonl` and delete `.olof` files -> both emit `reset: true`.
        let a_reset = r#"{"at":9000,"ms":5,"kind":"stop","args":[]}"#;
        std::fs::write(&actions_path, format!("{a_reset}\n")).unwrap();
        for entry in std::fs::read_dir(&sdir).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "olof") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let mut saw_act_reset = false;
        let mut saw_idx_reset = false;
        let until = Instant::now() + Duration::from_secs(5);
        while (!saw_act_reset || !saw_idx_reset) && Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap() {
                HistoryMsg::Actions { reset, lines } => {
                    if reset {
                        assert_eq!(lines, vec![a_reset.to_owned()]);
                        saw_act_reset = true;
                    }
                }
                HistoryMsg::Index { reset, last_size, frames } => {
                    if reset {
                        assert_eq!(last_size, None);
                        assert!(frames.is_empty());
                        saw_idx_reset = true;
                    }
                }
                other => panic!("unexpected msg during reset: {other:?}"),
            }
        }
        assert!(saw_act_reset && saw_idx_reset);

        // 8. Closing stdin ends `screen history-stream` immediately.
        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Step 1b: `olo-rex-link screen export NAME` streams progress + exact MP4
    /// bytes and removes its temp file; closing stdin mid-export kills the
    /// child process and leaves no `.mp4` or `.mp4.part` file behind.
    #[test]
    fn export_streams_progress_and_exact_mp4_bytes_and_cancels_cleanly_on_eof() {
        let dir = scratch_dir("export");
        let home = dir.join("home");
        let rt = dir.join("run");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();

        let args_log = dir.join("export-args.txt");
        let expected_mp4: Vec<u8> = (0..(140 * 1024usize)).map(|i| ((i * 31 + 7) & 0xff) as u8).collect();
        let fixture_mp4 = dir.join("fixture.mp4");
        std::fs::write(&fixture_mp4, &expected_mp4).unwrap();

        // 1. Normal export run: emits progress, writes MP4, outputs metadata JSON.
        let tool = desktop.join("bin/olo-screen");
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" > '{args_log}'\n\
             out=''\n\
             prev=''\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = '--export' ]; then out=\"$a\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             echo 'olo-screen: exporting… 30%' >&2\n\
             cp '{fixture}' \"$out.part\"\n\
             echo 'olo-screen: exporting… 80%' >&2\n\
             mv \"$out.part\" \"$out\"\n\
             echo 'olo-screen: exporting… 100%' >&2\n\
             printf '{{\"path\":\"%s\",\"seconds\":2.5,\"frames\":25,\"pictures_encoded\":9,\"encoder\":\"cpu\",\"size\":[1440,900]}}\\n' \"$out\"\n",
            args_log = args_log.display(),
            fixture = fixture_mp4.display(),
        );
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let export_disk = dir.join("disk-tmp");
        std::fs::create_dir_all(&export_disk).unwrap();
        let mut child = Command::new(HELPER)
            .args(["screen", "export", "qa", "--part", "1-4", "--zoom", "2"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _stdin = child.stdin.take().unwrap();
        let rx = export_msg_channel(child.stdout.take().unwrap());

        let mut progresses = Vec::new();
        let mut meta = None;
        let mut got_mp4 = Vec::new();
        let mut saw_done = false;
        while let Some(msg) = rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            match msg {
                ExportMsg::Progress { done } => progresses.push(done),
                ExportMsg::Meta(m) => meta = Some(m),
                ExportMsg::Chunk(bytes) => got_mp4.extend_from_slice(&bytes),
                ExportMsg::Done => {
                    saw_done = true;
                    break;
                }
                ExportMsg::Error { message } => panic!("unexpected export error: {message}"),
            }
        }
        assert!(wait_child_timeout(&mut child, Duration::from_secs(5)).success());
        assert!(saw_done);
        assert!(progresses.len() >= 2 && progresses.iter().any(|&p| (p - 0.3).abs() < 0.02));
        let meta = meta.expect("expected ExportMeta");
        assert_eq!((meta.bytes as usize, meta.frames, meta.encoded, meta.encoder.as_str(), meta.size), (expected_mp4.len(), 25, 9, "cpu", (1440, 900)));
        assert_eq!(got_mp4, expected_mp4, "streamed MP4 bytes must equal the file bit-for-bit");

        let logged = std::fs::read_to_string(&args_log).unwrap();
        assert!(logged.contains("--from\n2\n") && logged.contains("--to\n5\n") && logged.contains("--zoom\n2\n"), "unexpected forwarded args:\n{logged}");

        let leftovers: Vec<_> = std::fs::read_dir(&export_disk).unwrap().flatten().collect();
        assert!(leftovers.is_empty(), "temp export dirs must be cleaned up: {leftovers:?}");

        // 2. Cancelled export: stdin closed mid-run kills child process and removes `.mp4.part` & `.mp4`.
        let pid_file = dir.join("slow-export.pid");
        let slow_script = format!(
            "#!/bin/sh\n\
             echo $$ > '{pid_file}'\n\
             out=''\n\
             prev=''\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = '--export' ]; then out=\"$a\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             printf 'partial' > \"$out.part\"\n\
             echo 'olo-screen: exporting… 10%' >&2\n\
             exec sleep 30\n",
            pid_file = pid_file.display(),
        );
        std::fs::write(&tool, slow_script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut slow_child = Command::new(HELPER)
            .args(["screen", "export", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let slow_stdin = slow_child.stdin.take().unwrap();
        let slow_rx = export_msg_channel(slow_child.stdout.take().unwrap());

        // Wait until the slow export wrote its PID + emitted 10% progress.
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match slow_rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Some(ExportMsg::Progress { done }) if done >= 0.09 => break,
                Some(_) => continue,
                None => panic!("slow export ended prematurely"),
            }
        }
        let child_pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        assert!(PathBuf::from(format!("/proc/{child_pid}")).exists(), "slow export process should be running");

        // Close stdin -> helper kills export child and removes temp files immediately!
        drop(slow_stdin);
        let t_cancel = Instant::now();
        assert!(wait_child_timeout(&mut slow_child, Duration::from_secs(2)).success());
        assert!(t_cancel.elapsed() < Duration::from_secs(2), "cancel took {:?}", t_cancel.elapsed());
        while PathBuf::from(format!("/proc/{child_pid}")).exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!PathBuf::from(format!("/proc/{child_pid}")).exists(), "cancelled export child PID {child_pid} still running");
        let leftovers: Vec<_> = std::fs::read_dir(&export_disk).unwrap().flatten().collect();
        assert!(leftovers.is_empty(), "cancelled export leaked temp dirs: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_child_timeout(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child process did not exit within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    /// Finding 12: `screen history NAME` must forward to host `olo-screen history NAME`
    /// (preserving human-readable CLI), while `screen --protocol` advertises `history-stream`.
    #[test]
    fn review_12_history_cli_forwards_to_host_and_protocol_advertises_wire_verbs() {
        let dir = scratch_dir("rev12-hist");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, "#!/bin/sh\necho \"host-cli: $*\"\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "history", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Keep stdin open: if `screen history qa` enters binary follow mode, it will hang!
        let _keep_stdin_open = child.stdin.take().unwrap();
        let status = wait_child_timeout(&mut child, Duration::from_secs(2));
        assert!(status.success(), "expected host CLI forward to succeed: {status}");
        let mut out = String::new();
        std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out).unwrap();
        assert_eq!(out.trim(), "host-cli: history qa");

        let proto =
            Command::new(HELPER).args(["screen", "--protocol"]).env("HOME", &home).env("XDG_RUNTIME_DIR", &rt).env("XDG_STATE_HOME", &state).env("OLO_HOST_DESKTOP", &desktop).output().unwrap();
        assert!(proto.status.success(), "screen --protocol failed: {}", String::from_utf8_lossy(&proto.stderr));
        let v: serde_json::Value = serde_json::from_slice(&proto.stdout).unwrap();
        assert_eq!(v["proto"], PROTO);
        assert!(v["commands"].as_array().is_some_and(|a| a.iter().any(|x| x == "history-stream")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finding 27: rejected actions must not overwrite `owner.json`, `take --new`
    /// must attribute to the created screen's name (not a window ID), and
    /// diverged runtime vs state owner files must be repaired on the next action.
    #[test]
    fn review_27_rejected_action_does_not_overwrite_owner_take_new_and_repairs_divergence() {
        let dir = scratch_dir("rev27-owner");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();

        let tool = desktop.join("bin/olo-screen");
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in\n\
             start)\n\
               mkdir -p '{rt}/olo-screens' '{state}/olo/screens/'\"$2\"\n\
               printf '{{\"name\":\"%s\",\"wayland_display\":\"w\",\"olo_socket\":\"/tmp/x\",\"pid\":1,\"width\":1440,\"height\":900,\"scale\":1.0,\"sound\":false,\"started\":1}}\\n' \"$2\" > '{rt}/olo-screens/'\"$2\".json\n\
               printf '{{\"width\":1440,\"height\":900,\"scale\":1.0,\"sound\":false}}\\n' > '{state}/olo/screens/'\"$2\"/screen.json\n\
               printf '{{\"name\":\"%s\",\"pid\":1}}\\n' \"$2\"\n\
               ;;\n\
             click)\n\
               if [ \"${{FAIL_CLICK:-0}}\" = 1 ]; then echo 'controlled by person' >&2; exit 1; fi\n\
               ;;\n\
             take)\n\
               mkdir -p '{rt}/olo-screens' '{state}/olo/screens/taken1'\n\
               printf '{{\"name\":\"taken1\",\"wayland_display\":\"w\",\"olo_socket\":\"/tmp/x\",\"pid\":1,\"width\":1440,\"height\":900,\"scale\":1.0,\"sound\":false,\"started\":1}}\\n' > '{rt}/olo-screens/taken1.json\n\
               printf '{{\"width\":1440,\"height\":900,\"scale\":1.0,\"sound\":false}}\\n' > '{state}/olo/screens/taken1/screen.json\n\
               printf '{{\"name\":\"taken1\",\"went\":[42],\"stayed\":[]}}\\n'\n\
               ;;\n\
             esac\n",
            rt = rt.display(),
            state = state.display(),
        );
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run = |args: &[&str], block: &str, fail_click: bool| {
            Command::new(HELPER)
                .arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("REX_SESSION", "s1")
                .env("REX_BLOCK", block)
                .env("FAIL_CLICK", if fail_click { "1" } else { "0" })
                .output()
                .unwrap()
        };

        assert!(run(&["start", "qa", "--keep"], "block-A", false).status.success());
        let rt_owner = rt.join("olo-screens/qa.owner.json");
        let st_owner = state.join("olo/screens/qa/owner.json");

        // 1. Rejected click from block-B must NOT overwrite block-A.
        let rej = run(&["click", "qa", "10", "20"], "block-B", true);
        assert!(!rej.status.success());
        let after_rej: ScreenOwner = serde_json::from_slice(&std::fs::read(&rt_owner).unwrap()).unwrap();
        assert_eq!(after_rej.block_id, "block-A", "rejected action must not change owner");

        // 2. `take --new --window 42` attributes to created screen `taken1`, NOT `42`.
        let took = run(&["take", "--new", "--window", "42"], "block-B", false);
        assert!(took.status.success());
        assert!(!rt.join("olo-screens/42.owner.json").exists(), "window id 42 must not become owner file");
        let taken_owner: ScreenOwner = serde_json::from_slice(&std::fs::read(rt.join("olo-screens/taken1.owner.json")).unwrap()).unwrap();
        assert_eq!(taken_owner.block_id, "block-B");
        assert_eq!(taken_owner.started_by.as_ref().map(|p| p.block_id.as_str()), Some("block-B"));

        // 3. Diverged state copy (stale block-A) vs runtime copy (block-B) is repaired on next block-B action.
        assert!(run(&["click", "qa", "1", "2"], "block-B", false).status.success());
        let stale_a = ScreenOwner { session_id: "s1".into(), block_id: "block-A".into(), ..Default::default() };
        std::fs::write(&st_owner, serde_json::to_vec(&stale_a).unwrap()).unwrap();
        assert!(run(&["click", "qa", "3", "4"], "block-B", false).status.success());
        let repaired: ScreenOwner = serde_json::from_slice(&std::fs::read(&st_owner).unwrap()).unwrap();
        assert_eq!(repaired.block_id, "block-B", "diverged state owner copy must be repaired");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 9, 23, 25, 26: FIFO descriptor never hangs `follow`, bare `sleep`
    /// with `OLO_SCREEN` fails session identity, `tabula-screens` created after
    /// startup is watched, deleting `actions.jsonl` drops a legacy-only screen,
    /// and moving + recreating a screen directory keeps watching the replacement.
    #[test]
    fn review_9_23_25_26_follow_fifo_identity_tabula_actions_and_move_self() {
        let dir = scratch_dir("rev-follow");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let config = home.join(".config");
        let desktop = home.join(".local/share/olo/desktop");
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        // Finding 9: place a FIFO where a descriptor JSON would sit.
        let fifo_path = rt.join("olo-screens/evil.json");
        let c_fifo = std::ffi::CString::new(fifo_path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);

        // Finding 25: bare `/bin/sleep 60` with `OLO_SCREEN=fake` must NOT qualify as a running screen!
        let mut fake_sleep = Command::new("/bin/sleep").arg("60").env("OLO_SCREEN", "fake").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        write_screen_json(&rt, "fake", fake_sleep.id() as i32, &rt.join("fake.sock"), 800, 600);

        // Finding 26 (part B): legacy stopped screen with ONLY `actions.jsonl`.
        let legacy_dir = state.join("olo/screens/legacy");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(legacy_dir.join("actions.jsonl"), "{\"at\":1000,\"ms\":5,\"kind\":\"click\"}\n").unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &config)
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let rx = follow_event_channel(child.stdout.take().unwrap());

        // Snapshot arrives promptly despite FIFO, omits `fake`, includes `legacy`.
        let snap = rx.recv_timeout(Duration::from_secs(3)).expect("follow hung on FIFO").unwrap();
        let FollowEvent::Snapshot { screens, .. } = snap else { panic!("expected Snapshot, got {snap:?}") };
        assert!(!screens.iter().any(|s| s.name == "fake"), "bare sleep must not pass session identity check: {screens:?}");
        assert!(screens.iter().any(|s| s.name == "legacy"), "legacy actions-only screen should be listed");

        let wait_for = |pred: &dyn Fn(&FollowEvent) -> bool, label: &str| {
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until {
                let rem = until.saturating_duration_since(Instant::now());
                let ev = rx.recv_timeout(rem).unwrap_or_else(|_| panic!("timed out waiting for {label}")).unwrap();
                if pred(&ev) {
                    return;
                }
            }
            panic!("timed out waiting for {label}");
        };

        // Finding 26 (part B): deleting `actions.jsonl` removes the legacy screen immediately.
        std::fs::remove_file(legacy_dir.join("actions.jsonl")).unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Gone { name } if name == "legacy"), "Gone after actions.jsonl deleted");

        // Finding 26 (part A): `tabula-screens` created AFTER startup is still watched.
        use std::os::unix::process::CommandExt as _;
        let mut tabula_stub = Command::new("/bin/sleep").arg0("olo-session").arg("60").env("OLO_SCREEN", "oldrt").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let tabula_dir = rt.join("tabula-screens");
        std::fs::create_dir_all(&tabula_dir).unwrap();
        let tab_json = serde_json::json!({
            "name": "oldrt", "wayland_display": "w", "tabula_socket": rt.join("oldrt.sock"),
            "pid": tabula_stub.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 1
        });
        std::fs::write(tabula_dir.join("oldrt.json"), serde_json::to_vec(&tab_json).unwrap()).unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Screen { screen } if screen.name == "oldrt" && screen.state == HostScreenState::Running), "tabula-screens created after startup");

        // Finding 23: move screen dir away (`IN_MOVE_SELF`) and recreate at same name -> still watched!
        let mv_dir = state.join("olo/screens/mv");
        std::fs::create_dir_all(&mv_dir).unwrap();
        std::fs::write(mv_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Screen { screen } if screen.name == "mv"), "mv created");
        let trashed = dir.join("mv-trashed");
        std::fs::rename(&mv_dir, &trashed).unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Gone { name } if name == "mv"), "mv moved away");
        std::fs::create_dir_all(&mv_dir).unwrap();
        std::fs::write(mv_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(mv_dir.join("title"), "Reborn\n").unwrap();
        wait_for(&|ev| matches!(ev, FollowEvent::Screen { screen } if screen.name == "mv" && screen.title.as_deref() == Some("Reborn")), "recreated mv watched");

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(2)).success());
        let _ = fake_sleep.kill();
        let _ = fake_sleep.wait();
        let _ = tabula_stub.kill();
        let _ = tabula_stub.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 3 & 4: (A) restarting a screen under the same name never lets
    /// the old viewer forward input to the replacement or unlink the new
    /// viewer's `.control` lock; (B) slow synthetic replies keep `.control`
    /// held until queued input and held-key releases finish.
    #[test]
    fn review_3_4_restarted_screen_and_slow_handoff_do_not_leak_control_or_input() {
        let dir = scratch_dir("rev3-4");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);

        // Part A: Finding 3 (restarted screen under same name).
        let sock1 = rt.join("s1.sock");
        let mut stub1 = SessionStub::start("same");
        let server1 = FakeAppServer::start_in(&mut stub1, sock1.clone(), w, h, make_rgba(w, h, 1));
        write_screen_json(&rt, "same", stub1.pid(), &sock1, w, h);

        let mut c1 = spawn_stream(&dir, "same");
        let mut in1 = c1.stdin.take().unwrap();
        let rx1 = server_msg_channel(c1.stdout.take().unwrap());
        let _ = rx1.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in1, &ClientMsg::Ack { seq: 1 }).unwrap();
        write_client_msg(&mut in1, &ClientMsg::Control { on: true }).unwrap();
        let ctl_path = rt.join("olo-screens/same.control");
        let p1 = c1.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p1.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }

        // Stop original screen (unlinks `.control`), start brand-new session under same name!
        stub1.kill_and_wait();
        let _ = std::fs::remove_file(&ctl_path);
        server1.shutdown();

        let sock2 = rt.join("s2.sock");
        let mut stub2 = SessionStub::start("same");
        let server2 = FakeAppServer::start_in(&mut stub2, sock2.clone(), w, h, make_rgba(w, h, 2));
        write_screen_json(&rt, "same", stub2.pid(), &sock2, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub2]);

        // Viewer 2 connects to the new screen and takes control.
        let mut c2 = spawn_stream(&dir, "same");
        let mut in2 = c2.stdin.take().unwrap();
        let rx2 = server_msg_channel(c2.stdout.take().unwrap());
        let _ = rx2.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in2, &ClientMsg::Ack { seq: 1 }).unwrap();
        write_client_msg(&mut in2, &ClientMsg::Control { on: true }).unwrap();
        let p2 = c2.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p2.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(rx2.recv_timeout(Duration::from_secs(2)), Ok(Some(ServerMsg::Control { held: true, .. }))), "expected viewer 2 Control(true) ack");

        // Viewer 1 MUST see ScreenGone, exit promptly, NOT forward input to server2, and NOT unlink viewer 2's `.control`!
        let _ = write_client_msg(&mut in1, &ClientMsg::Input { json_events: r#"[{"type":"key","key":88,"pressed":true}]"#.into() });
        let mut saw_gone = false;
        let until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            match rx1.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(ServerMsg::ScreenGone { .. })) => {
                    saw_gone = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_gone, "old viewer must receive ScreenGone when original session exits");
        drop(in1);
        assert!(wait_child_timeout(&mut c1, Duration::from_secs(3)).success());
        assert_eq!(std::fs::read_to_string(&ctl_path).ok().as_deref(), Some(p2.as_str()), "old viewer exit must NOT unlink new viewer's .control");
        assert!(server2.received_inputs().is_empty(), "old viewer must NOT forward input to replacement screen");

        // Part B: Finding 4 & 61 (deterministic sync gate replaces sleep timing:
        // hold synthetic reply while Control(false) arrives and verify `.control`
        // remains held until the gated key-down + key-up release both finish!).
        server2.hold_synthetic(true);
        write_client_msg(&mut in2, &ClientMsg::Input { json_events: r#"[{"type":"key","key":42,"pressed":true}]"#.into() }).unwrap();
        assert!(server2.wait_synthetic_blocked(Duration::from_secs(3)), "input worker did not reach synthetic gate");
        write_client_msg(&mut in2, &ClientMsg::ControlSeq { on: false, seq: 99 }).unwrap();
        // While the synthetic handler is gated, `.control` MUST STILL BE HELD and no release ack emitted!
        assert!(rx2.recv_timeout(Duration::from_millis(80)).is_err(), "Control(false) ack arrived before gated synthetic input finished");
        assert!(ctl_path.exists(), ".control was dropped before slow synthetic input and release finished!");
        // Unblock the gate: key-down finishes, key-up unwinds, `.control` unlinks, and seq=99 ack arrives!
        server2.hold_synthetic(false);
        let ack = rx2.recv_timeout(Duration::from_secs(4)).expect("expected Control(false) release ack").unwrap();
        assert_eq!(ack, ServerMsg::Control { held: false, seq: 99, reason: None });
        assert!(!ctl_path.exists(), ".control was not removed before Control(false) ack arrived");
        let got = server2.wait_inputs(2, Duration::from_secs(2));
        assert_eq!(got.len(), 2, "expected key-down + key-up release: {got:?}");

        drop(in2);
        assert!(wait_child_timeout(&mut c2, Duration::from_secs(3)).success());
    }

    /// Findings 5, 6, 16, 19: failed input reply retains held key; dropped reply
    /// does not duplicate a click; `keysym` & `touch` are unwound; unknown events
    /// are dropped; `[Control(false), Input] + EOF` drops the trailing input;
    /// pointer-only `move` triggers capture without a `Painted` event.
    #[test]
    fn review_5_6_16_19_input_reliability_keysym_touch_eof_order_and_cursor_move() {
        let dir = scratch_dir("rev5-6-16-19");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);
        let sock = rt.join("in.sock");
        let mut stub = SessionStub::start("inp");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 3));
        write_screen_json(&rt, "inp", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_path = rt.join("olo-screens/inp.control");

        let mut child = spawn_stream(&dir, "inp");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 1 }).unwrap();
        write_client_msg(&mut stdin, &ClientMsg::Control { on: true }).unwrap();
        let p = child.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(Some(ServerMsg::Control { held: true, .. }))), "expected Control(true) ack");

        // Finding 5a: press key 42 (succeeds), then send key 42 release when server returns Reply::Error.
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"key","key":42,"pressed":true}]"#.into() }).unwrap();
        assert_eq!(server.wait_inputs(1, Duration::from_secs(3)).len(), 1);
        server.set_synthetic_error_count(1);
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"key","key":42,"pressed":false}]"#.into() }).unwrap();
        std::thread::sleep(Duration::from_millis(120));

        // Finding 6: press keysym + touch down + send unsupported touchpad_gesture (must be dropped).
        write_client_msg(
            &mut stdin,
            &ClientMsg::Input {
                json_events: r#"[{"type":"keysym","keysym":65,"pressed":true},{"type":"touch","slot":2,"phase":"down","x":12.0,"y":34.0},{"type":"touchpad_gesture","kind":"pinch"}]"#.into(),
            },
        )
        .unwrap();
        let after_press = server.wait_inputs(3, Duration::from_secs(3));
        assert!(!after_press.iter().any(|e| e["type"] == "touchpad_gesture"), "unsupported variant must be dropped: {after_press:?}");

        // Finding 5b: click whose reply is lost (server drops socket after applying) must NOT be duplicated!
        server.set_drop_after_synthetic(true);
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"button","button":272,"pressed":true},{"type":"button","button":272,"pressed":false}]"#.into() }).unwrap();
        let _ = server.wait_inputs(5, Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(150));
        let click_downs = server.received_inputs().iter().filter(|e| e["type"] == "button" && e["button"] == 272 && e["pressed"] == true).count();
        assert_eq!(click_downs, 1, "click must not be applied twice when reply is lost");

        // Finding 19: cursor-only `move` without any `Painted` event still captures updated pixels!
        let frame2 = make_rgba(w, h, 99);
        server.set_frame(w, h, frame2.clone());
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"move","x":44.0,"y":55.0}]"#.into() }).unwrap();
        let ServerMsg::Frame { seq: s2, delta: d2 } = rx.recv_timeout(Duration::from_secs(3)).expect("pointer move should trigger capture even without Painted").unwrap() else {
            panic!("expected Frame after pointer move");
        };
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: s2 }).unwrap();
        let _ = d2;

        // Finding 16 + 5a + 6: send `[Control(false), Input(key 99 down)]` + EOF in ONE write:
        // key 42, keysym 65, touch slot 2 MUST ALL be released, while key 99 MUST be dropped!
        let mut final_buf = Vec::new();
        write_client_msg(&mut final_buf, &ClientMsg::Control { on: false }).unwrap();
        write_client_msg(&mut final_buf, &ClientMsg::Input { json_events: r#"[{"type":"key","key":99,"pressed":true}]"#.into() }).unwrap();
        stdin.write_all(&final_buf).unwrap();
        stdin.flush().unwrap();
        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());

        let all = server.received_inputs();
        assert!(!all.iter().any(|e| e["type"] == "key" && e["key"] == 99), "input after Control(false) before EOF must be dropped: {all:?}");
        assert!(all.iter().any(|e| e["type"] == "key" && e["key"] == 42 && e["pressed"] == false), "key 42 must be released after earlier rejected release: {all:?}");
        assert!(all.iter().any(|e| e["type"] == "keysym" && e["keysym"] == 65 && e["pressed"] == false), "keysym 65 must be released: {all:?}");
        assert!(all.iter().any(|e| e["type"] == "touch" && e["slot"] == 2 && e["phase"] == "up"), "touch slot 2 must be released: {all:?}");
    }

    /// Findings 7, 13, 14, 15, 22: ACK ahead of `seq_sent` never breaks 2-in-flight;
    /// unchanged-pixel repaints obey pacing; screenshot `Reply::Error` does not
    /// reconnect-loop or drop control ownership; stalled stdout never defeats EOF exit.
    #[test]
    fn review_7_13_14_15_22_ack_pacing_shot_error_and_stalled_stdout_cancel() {
        let dir = scratch_dir("rev7-13-14-15-22");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);
        let sock = rt.join("flow.sock");
        let frame1 = make_rgba(w, h, 4);
        let mut stub = SessionStub::start("flow");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, frame1.clone());
        write_screen_json(&rt, "flow", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_path = rt.join("olo-screens/flow.control");

        // Part A: Finding 13 (ACK ahead of seq_sent) + Finding 22 (unchanged pixel pacing) + Finding 14/15 (shot error while controlling).
        let mut child = spawn_stream(&dir, "flow");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        server.wait_subscriber(Duration::from_secs(5));

        // Send a bogus future ACK (seq=999) right after frame 1 arrives.
        let ServerMsg::Frame { seq: s1, .. } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(s1, 1);
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 999 }).unwrap();

        // Frame 2 arrives (now 2 in flight because seq=999 was ignored!).
        server.set_frame(w, h, make_rgba(w, h, 5));
        server.paint();
        let ServerMsg::Frame { seq: s2, .. } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(s2, 2);

        // Frame 3 MUST wait because 2 frames are still unacknowledged!
        server.set_frame(w, h, make_rgba(w, h, 6));
        server.paint();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "future ACK 999 disabled MAX_IN_FLIGHT=2");

        // Valid ACK 2 releases frame 3.
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 2 }).unwrap();
        let ServerMsg::Frame { seq: s3, .. } = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else { panic!() };
        assert_eq!(s3, 3);
        write_client_msg(&mut stdin, &ClientMsg::Ack { seq: 3 }).unwrap();

        // Finding 22: after 120 ms quiet stretch, burst 12 Painted events 10 ms apart with identical pixels.
        std::thread::sleep(Duration::from_millis(120));
        let shots_before = server.screenshot_count();
        for _ in 0..12 {
            server.paint();
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(40));
        let extra_shots = server.screenshot_count() - shots_before;
        assert!(extra_shots <= 4, "unchanged-pixel repaints bypassed capture pacing: took {extra_shots} screenshots in ~120 ms");

        // Findings 14 & 15: while holding control, screenshot Reply::Error("disk full")
        // must NOT trigger Reconnecting loop and must NOT send `error: true` while still holding `.control`.
        write_client_msg(&mut stdin, &ClientMsg::Control { on: true }).unwrap();
        let p = child.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(Some(ServerMsg::Control { held: true, .. }))), "expected Control(true) ack");
        server.set_shot_error(Some("disk full"));
        server.paint();
        let until = Instant::now() + Duration::from_secs(3);
        let mut saw_warning = false;
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(ServerMsg::Control { held: true, .. })) => continue,
                Ok(Some(ServerMsg::Warning { message })) => {
                    assert!(message.contains("disk full"), "should report actual screenshot error: {message:?}");
                    saw_warning = true;
                    break;
                }
                other => panic!("expected Warning (or Control grant), got {other:?}"),
            }
        }
        assert!(saw_warning, "expected capture Warning on screenshot Reply::Error");
        assert!(ctl_path.exists(), "control lock must remain held");
        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());

        // Part B: Finding 7 (stalled stdout reader NEVER defeats EOF exit + control cleanup!).
        server.set_shot_error(None);
        // Use a larger incompressible frame (~1 MiB) so stdout pipe buffer (64 KiB) fills immediately.
        let (bw, bh) = (512u32, 512u32);
        let mut noisy = vec![0u8; (bw * bh * 4) as usize];
        let mut x: u32 = 0x1234_5678;
        for chunk in noisy.as_chunks_mut::<4>().0 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        server.set_frame(bw, bh, noisy);

        let mut stalled = spawn_stream(&dir, "flow");
        let mut stalled_in = stalled.stdin.take().unwrap();
        let _undrained_stdout = stalled.stdout.take().unwrap(); // Deliberately NEVER read!
        // Take control first so we also verify `.control` is cleaned up when stdout is blocked!
        write_client_msg(&mut stalled_in, &ClientMsg::Control { on: true }).unwrap();
        let ps = stalled.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(ps.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        // Finding 61: observe actual pipe-full backpressure via `F_GETPIPE_SZ` +
        // `FIONREAD` on the undrained stdout pipe instead of sleeping blindly!
        {
            use std::os::fd::AsRawFd as _;
            let fd = _undrained_stdout.as_raw_fd();
            let pipe_sz = unsafe { libc::fcntl(fd, libc::F_GETPIPE_SZ) }.max(4096);
            // If `Control(true)` occupied 1 page slot before the 1 MiB keyframe
            // write filled all remaining page slots, `FIONREAD` reaches at least
            // `pipe_sz - 4096` (or `pipe_sz` on a 1-page pipe) while blocked in `poll(POLLOUT)`.
            let full_threshold = if pipe_sz > 4096 { pipe_sz - 4096 } else { pipe_sz };
            let until = Instant::now() + Duration::from_secs(3);
            let mut queued: libc::c_int = 0;
            while Instant::now() < until {
                unsafe { libc::ioctl(fd, libc::FIONREAD, &mut queued) };
                if queued >= full_threshold {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(queued >= full_threshold, "expected stalled stdout pipe buffer to fill (>={full_threshold} bytes for pipe_sz={pipe_sz}), got {queued} bytes");
        }
        drop(stalled_in);
        let status = wait_child_timeout(&mut stalled, Duration::from_secs(3));
        assert!(status.success(), "stalled stream should exit cleanly on EOF: {status}");
        assert!(!ctl_path.exists(), "stalled stream must release .control on EOF");
    }

    /// Findings 10 & 25: stream capture directory is private `0700` (removed
    /// wholesale on exit), and a descriptor pointing to another screen's
    /// session socket is rejected by `SO_PEERCRED` verification.
    #[test]
    fn review_10_25_private_capture_dir_and_wrong_peer_socket_rejected() {
        let dir = scratch_dir("rev10-25");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);

        // Part A: Finding 10 (private 0700 capture dir).
        let sock_a = rt.join("a.sock");
        let mut stub_a = SessionStub::start("alpha");
        let _server_a = FakeAppServer::start_in(&mut stub_a, sock_a.clone(), w, h, make_rgba(w, h, 8));
        write_screen_json(&rt, "alpha", stub_a.pid(), &sock_a, w, h);

        let mut c_a = spawn_stream(&dir, "alpha");
        let in_a = c_a.stdin.take().unwrap();
        let rx_a = server_msg_channel(c_a.stdout.take().unwrap());
        let _ = rx_a.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let views = rt.join("olo-screens/views");
        let cap_dirs: Vec<_> = std::fs::read_dir(&views).unwrap().flatten().collect();
        assert_eq!(cap_dirs.len(), 1, "expected one private capture dir: {cap_dirs:?}");
        let mode = std::fs::symlink_metadata(cap_dirs[0].path()).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700, "capture dir must be private 0700, got {mode:o}");
        drop(in_a);
        assert!(wait_child_timeout(&mut c_a, Duration::from_secs(3)).success());
        assert!(std::fs::read_dir(&views).unwrap().next().is_none(), "private capture dir must be removed on exit");

        // Part B: Finding 25 (descriptor for `beta` pointing at `alpha`'s socket is rejected!).
        let stub_b = SessionStub::start("beta");
        write_screen_json(&rt, "beta", stub_b.pid(), &sock_a, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub_a, stub_b]);

        let mut c_b = spawn_stream(&dir, "beta");
        let _in_b = c_b.stdin.take().unwrap();
        let rx_b = server_msg_channel(c_b.stdout.take().unwrap());
        let msg = rx_b.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        match msg {
            ServerMsg::Status(st) => {
                assert!(st.error, "wrong-peer socket must produce error status: {st:?}");
                assert!(st.message.contains("untrusted screen socket"), "unexpected message: {st:?}");
            }
            other => panic!("expected untrusted socket rejection Status, got {other:?}"),
        }
        assert!(wait_child_timeout(&mut c_b, Duration::from_secs(3)).success());
    }

    /// Findings 23 & 29: replacing an `.olof` segment at the same path with an
    /// identical-length, identical-timestamp file on a new inode serves the NEW
    /// pixels (never stale cached pixels), and moving + recreating the screen
    /// state folder keeps watching the replacement without leaking watches.
    #[test]
    fn review_23_29_history_inode_replacement_and_move_self_recreate() {
        let dir = scratch_dir("rev23-29-hist");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        let sdir = state.join("olo/screens/rep");
        std::fs::create_dir_all(&sdir).unwrap();

        let (w, h) = (64usize, 64usize);
        let red_rgba: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 255], w * h).flatten().collect();
        let blue_rgba: Vec<u8> = std::iter::repeat_n([0u8, 0, 255, 255], w * h).flatten().collect();

        // Build two separate segments with identical timestamp (1000) and identical compressed length.
        let tmp_a = dir.join("seg_a");
        let tmp_b = dir.join("seg_b");
        std::fs::create_dir_all(&tmp_a).unwrap();
        std::fs::create_dir_all(&tmp_b).unwrap();
        OlofWriter::new(tmp_a.clone(), 64 << 20).add(Frame { width: w, height: h, rgba: red_rgba.clone() }, 1_000).unwrap();
        OlofWriter::new(tmp_b.clone(), 64 << 20).add(Frame { width: w, height: h, rgba: blue_rgba.clone() }, 1_000).unwrap();
        let seg_a = std::fs::read_dir(&tmp_a).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == "olof")).unwrap().path();
        let seg_b = std::fs::read_dir(&tmp_b).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == "olof")).unwrap().path();
        assert_eq!(std::fs::metadata(&seg_a).unwrap().len(), std::fs::metadata(&seg_b).unwrap().len(), "test setup: identical file lengths");
        let target_seg = sdir.join("0000000000001000.olof");
        std::fs::copy(&seg_a, &target_seg).unwrap();
        let ino_before = std::fs::metadata(&target_seg).unwrap().ino();

        let mut child = Command::new(HELPER)
            .args(["screen", "history-stream", "rep"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let rx = history_msg_channel(child.stdout.take().unwrap());

        // Drain initial Actions + Index + SnapshotDone.
        let _ = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(), HistoryMsg::SnapshotDone { .. }));

        // Decode initial red frame (req 1).
        write_history_request(&mut stdin, &HistoryRequest::FrameAt { req: 1, at: 1_000 }).unwrap();
        let HistoryMsg::Frame { req, delta, .. } = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap() else { panic!("expected red frame") };
        assert_eq!(req, 1);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        decode_and_apply(&delta, &mut canvas);
        assert_eq!(canvas.rgba, red_rgba);

        // Atomically replace target_seg with a NEW inode of the EXACT same length & timestamp containing blue!
        let staged = sdir.join(".new-seg.tmp");
        std::fs::copy(&seg_b, &staged).unwrap();
        std::fs::rename(&staged, &target_seg).unwrap();
        assert_ne!(std::fs::metadata(&target_seg).unwrap().ino(), ino_before);
        // Wait past coalesce window so inotify update flushes.
        std::thread::sleep(Duration::from_millis(80));
        while rx.try_recv().is_ok() {}

        // Request req 2 at t=1000 -> MUST decode blue from the new inode, NOT cached red!
        write_history_request(&mut stdin, &HistoryRequest::FrameAt { req: 2, at: 1_000 }).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let mut got_blue = false;
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap() {
                HistoryMsg::Frame { req: 2, delta, .. } => {
                    decode_and_apply(&delta, &mut canvas);
                    assert_eq!(canvas.rgba, blue_rgba, "replaced segment returned stale pixels from old inode!");
                    got_blue = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(got_blue, "did not receive req 2 frame");

        // Finding 23: move `sdir` away (`IN_MOVE_SELF`), recreate `sdir`, write a new action,
        // and also modify the moved-away directory to prove its old watch cannot clear the new one.
        let moved = dir.join("rep-moved");
        std::fs::rename(&sdir, &moved).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        std::fs::create_dir_all(&sdir).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        // Move the old directory again (would emit stale IN_MOVE_SELF if watch leaked!).
        let moved2 = dir.join("rep-moved-2");
        std::fs::rename(&moved, &moved2).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let fresh_action = r#"{"at":5000,"ms":4,"kind":"click","args":["1","2"]}"#;
        std::fs::write(sdir.join("actions.jsonl"), format!("{fresh_action}\n")).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let mut saw_fresh = false;
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(HistoryMsg::Actions { lines, .. })) if lines.iter().any(|l| l == fresh_action) => {
                    saw_fresh = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_fresh, "history did not observe actions in recreated folder after IN_MOVE_SELF");

        // Finding 23 & 49: verify zero leaked inotify watches in `/proc/<pid>/fdinfo/*` after rename+recreate
        // (target + up to 2 ancestor levels = <= 3 constant watches, never growing).
        let count_inotify_wds = |pid: u32| -> usize {
            let mut total = 0usize;
            for entry in std::fs::read_dir(format!("/proc/{pid}/fdinfo")).into_iter().flatten().flatten() {
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    total += text.lines().filter(|l| l.starts_with("inotify wd:")).count();
                }
            }
            total
        };
        let wds_after_first = count_inotify_wds(child.id());
        assert!((1..=3).contains(&wds_after_first), "expected 1..=3 inotify watches (target + ancestors), got {wds_after_first}");
        // Move & recreate a second time and verify watch count does NOT grow!
        let moved2 = sdir.with_file_name("rep-moved-aside-2");
        std::fs::rename(&sdir, &moved2).unwrap();
        std::fs::create_dir_all(&sdir).unwrap();
        let fresh_action2 = r#"{"at":4000,"ms":5,"kind":"click","args":["after-move-2"]}"#;
        std::fs::write(sdir.join("actions.jsonl"), format!("{fresh_action2}\n")).unwrap();
        let until2 = Instant::now() + Duration::from_secs(4);
        let mut saw_fresh2 = false;
        while Instant::now() < until2 {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(HistoryMsg::Actions { lines, .. })) if lines.iter().any(|l| l == fresh_action2) => {
                    saw_fresh2 = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_fresh2, "history did not observe actions after second rename+recreate");
        assert_eq!(count_inotify_wds(child.id()), wds_after_first, "moved directory must not leak kernel inotify watches");

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 10, 11, 30, 31, 32: option-shaped name rejected, discontiguous
    /// `--part` rejected, temp MP4 placed in private `0700` dir on disk (not
    /// `XDG_RUNTIME_DIR` tmpfs), non-ASCII progress + >64 KiB stderr handled
    /// without panic or truncation drop, and SIGTERM-ignoring grandchild killed
    /// on cancellation (with child SIGTERM mask unblocked).
    #[test]
    fn review_10_11_30_31_32_export_validation_disk_dir_diagnostics_and_grandchild_cancel() {
        let dir = scratch_dir("rev-export");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let export_disk = dir.join("disk-tmp");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&export_disk).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();

        let ran_flag = dir.join("tool-ran");
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, format!("#!/bin/sh\necho \"$*\" > '{}'\nexit 0\n", ran_flag.display())).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Finding 11: `--clear` rejected before invoking host `olo-screen`.
        let mut c_flag = Command::new(HELPER)
            .args(["screen", "export", "--clear"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let rx_flag = export_msg_channel(c_flag.stdout.take().unwrap());
        assert!(matches!(rx_flag.recv_timeout(Duration::from_secs(3)).unwrap(), Some(ExportMsg::Error { .. })));
        let _ = wait_child_timeout(&mut c_flag, Duration::from_secs(3));
        assert!(!ran_flag.exists(), "option-shaped name `--clear` must not run host olo-screen");

        // Finding 30: discontiguous `--part 0,2,5` rejected before invoking host `olo-screen`.
        let mut c_part = Command::new(HELPER)
            .args(["screen", "export", "qa", "--part", "0,2,5"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let rx_part = export_msg_channel(c_part.stdout.take().unwrap());
        let Some(ExportMsg::Error { message }) = rx_part.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("expected ExportMsg::Error for discontiguous --part");
        };
        assert!(message.contains("contiguous"), "expected clear explanation, got: {message}");
        let _ = wait_child_timeout(&mut c_part, Duration::from_secs(3));
        assert!(!ran_flag.exists(), "discontiguous --part must not run host olo-screen");

        // Finding 10 + 31: verify temp MP4 is under `export_disk` (0700, NOT `rt`),
        // non-ASCII progress `"exporting…40%"` does NOT panic, and >80 KiB stderr
        // preserves the final error diagnostic cleanly.
        let out_path_log = dir.join("out-path.txt");
        let dir_mode_log = dir.join("dir-mode.txt");
        let noisy_script = format!(
            "#!/bin/sh\n\
             out=''\n\
             prev=''\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = '--export' ]; then out=\"$a\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             printf '%s' \"$out\" > '{out_path_log}'\n\
             stat -c '%a' \"$(dirname \"$out\")\" > '{dir_mode_log}'\n\
             printf 'olo-screen: exporting…40%%\\n' >&2\n\
             i=0; while [ $i -lt 1200 ]; do printf 'noisy diagnostic line 0123456789abcdef0123456789abcdef0123456789abcdef\\n' >&2; i=$((i+1)); done\n\
             printf 'FINAL_REASON: encoder failed\\n' >&2\n\
             exit 1\n",
            out_path_log = out_path_log.display(),
            dir_mode_log = dir_mode_log.display(),
        );
        std::fs::write(&tool, noisy_script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut c_noisy = Command::new(HELPER)
            .args(["screen", "export", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let _in_noisy = c_noisy.stdin.take().unwrap();
        let rx_noisy = export_msg_channel(c_noisy.stdout.take().unwrap());
        let mut saw_40 = false;
        let mut err_msg = None;
        while let Some(msg) = rx_noisy.recv_timeout(Duration::from_secs(5)).unwrap() {
            match msg {
                ExportMsg::Progress { done } if (done - 0.4).abs() < 0.02 => saw_40 = true,
                ExportMsg::Error { message } => {
                    err_msg = Some(message);
                    break;
                }
                _ => {}
            }
        }
        assert!(wait_child_timeout(&mut c_noisy, Duration::from_secs(3)).success());
        assert!(saw_40, "expected Progress(0.4) from non-ASCII `exporting…40%` without panic");
        let err_msg = err_msg.expect("expected ExportMsg::Error after >80 KiB stderr");
        assert!(err_msg.contains("FINAL_REASON: encoder failed"), "missing tail diagnostic: {err_msg}");
        let used_out = std::fs::read_to_string(&out_path_log).unwrap();
        assert!(used_out.starts_with(&export_disk.display().to_string()), "export MP4 must be on disk ({}) not runtime tmpfs: {used_out}", export_disk.display());
        assert_eq!(std::fs::read_to_string(&dir_mode_log).unwrap().trim(), "700", "export temp dir must be mode 0700");

        // Finding 32: child spawns a SIGTERM-ignoring grandchild and leader exits
        // immediately on SIGTERM -> cancellation MUST kill the grandchild too!
        let gc_pid_file = dir.join("grandchild.pid");
        let term_seen_file = dir.join("leader-got-term");
        let gc_script = format!(
            "#!/bin/sh\n\
             trap 'echo yes > \"{term_seen}\"; exit 0' TERM\n\
             sh -c 'trap \"\" TERM; echo $$ > \"{gc_pid}\"; exec sleep 30' &\n\
             while [ ! -s '{gc_pid}' ]; do sleep 0.01; done\n\
             echo 'olo-screen: exporting… 15%' >&2\n\
             sleep 30 &\n\
             wait $!\n",
            term_seen = term_seen_file.display(),
            gc_pid = gc_pid_file.display(),
        );
        std::fs::write(&tool, gc_script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut c_gc = Command::new(HELPER)
            .args(["screen", "export", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let in_gc = c_gc.stdin.take().unwrap();
        let rx_gc = export_msg_channel(c_gc.stdout.take().unwrap());
        loop {
            match rx_gc.recv_timeout(Duration::from_secs(5)).unwrap() {
                Some(ExportMsg::Progress { done }) if done >= 0.14 => break,
                Some(_) => continue,
                None => panic!("grandchild export fixture ended early"),
            }
        }
        // Finding 60: pin the grandchild with a `pidfd` BEFORE verifying its
        // identity and use `pidfd` for both exit polling and cleanup signaling!
        let gc_pid: i32 = std::fs::read_to_string(&gc_pid_file).unwrap().trim().parse().unwrap();
        let gc_pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, gc_pid, 0 as libc::c_uint) as i32 };
        assert!(gc_pidfd >= 0, "grandchild PID {gc_pid} should be alive and pinnable before cancel");
        let gc_cmd = std::fs::read(format!("/proc/{gc_pid}/cmdline")).unwrap_or_default();
        assert!(gc_cmd.windows(5).any(|w| w == b"sleep"), "pinned grandchild pidfd should refer to our sleep grandchild: {gc_cmd:?}");

        drop(in_gc);
        assert!(wait_child_timeout(&mut c_gc, Duration::from_secs(3)).success());
        let mut pfd = libc::pollfd { fd: gc_pidfd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, 2000) };
        let gc_exited = r > 0 && pfd.revents != 0;
        if !gc_exited {
            // Clean up leaked grandchild safely via pinned pidfd (never raw PID kill)!
            unsafe {
                libc::syscall(libc::SYS_pidfd_send_signal, gc_pidfd, libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0 as libc::c_uint);
            }
        }
        unsafe { libc::close(gc_pidfd) };
        assert!(gc_exited, "SIGTERM-ignoring grandchild PID {gc_pid} survived export cancellation!");
        assert!(term_seen_file.exists(), "child inherited blocked SIGTERM and never saw SIGTERM!");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 2 & 14: (A) an existing unlocked `.control` file holding a live
    /// controller PID (older non-flock viewer) is NOT overwritten; (B) a
    /// reachable socket that accepts `subscribe_to` and never replies times out
    /// cleanly instead of stalling startup forever.
    #[test]
    fn review_2_14_non_flock_controller_respected_and_silent_subscribe_times_out() {
        let dir = scratch_dir("rev2-14");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);

        // Part A: Finding 2 (pre-existing unlocked `.control` with live `olo-rex-link` PID).
        let sock = rt.join("nf.sock");
        let mut stub = SessionStub::start("nf");
        let _server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 2));
        write_screen_json(&rt, "nf", stub.pid(), &sock, w, h);

        // Spawn a live `olo-rex-link screen follow` process whose exe is `olo-rex-link` (a valid controller exe)
        // and write its PID into `nf.control` WITHOUT taking flock!
        let mut fake_old_viewer = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let old_pid_str = fake_old_viewer.id().to_string();
        let ctl_path = rt.join("olo-screens/nf.control");
        std::fs::write(&ctl_path, &old_pid_str).unwrap();

        let mut c1 = spawn_stream(&dir, "nf");
        let mut in1 = c1.stdin.take().unwrap();
        let rx1 = server_msg_channel(c1.stdout.take().unwrap());
        let _ = rx1.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in1, &ClientMsg::Control { on: true }).unwrap();
        let msg = rx1.recv_timeout(Duration::from_secs(3)).expect("expected control refusal").unwrap();
        match msg {
            ServerMsg::Control { held: false, reason: Some(reason), .. } => assert!(reason.contains("Someone else is controlling"), "unexpected refusal: {reason:?}"),
            other => panic!("expected refusal Control, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&ctl_path).unwrap(), old_pid_str, "live non-flock controller PID must not be clobbered");
        drop(in1);
        assert!(wait_child_timeout(&mut c1, Duration::from_secs(3)).success());
        drop(fake_old_viewer.stdin.take());
        assert!(wait_child_timeout(&mut fake_old_viewer, Duration::from_secs(3)).success());

        // Part B: Finding 14 (reachable socket accepts connection and never replies to `subscribe_to`).
        let silent_sock = rt.join("silent.sock");
        let silent_backend = rt.join(".silent-backend.sock");
        let _ = std::fs::remove_file(&silent_backend);
        let listener = UnixListener::bind(&silent_backend).unwrap();
        let held_conns = Arc::new(Mutex::new(Vec::new()));
        let hc = held_conns.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                // Deliberately keep sockets open without ever writing a reply!
                hc.lock().unwrap().push(conn);
            }
        });
        let mut stub_s = SessionStub::start("silent");
        stub_s.bind_socket(&silent_sock, &silent_backend);
        write_screen_json(&rt, "silent", stub_s.pid(), &silent_sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub, stub_s]);

        let mut c_silent = Command::new(HELPER)
            .args(["screen", "stream", "silent"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1")
            .env("OLO_SCREEN_HANDSHAKE_MS", "250")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _in_s = c_silent.stdin.take().unwrap();
        let rx_s = server_msg_channel(c_silent.stdout.take().unwrap());
        let mut saw_gone = false;
        let until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            match rx_s.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(ServerMsg::ScreenGone { .. })) => {
                    saw_gone = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_gone, "silent subscribe_to socket must time out and emit ScreenGone");
        assert!(wait_child_timeout(&mut c_silent, Duration::from_secs(3)).success());
    }

    // -----------------------------------------------------------------------
    // Review 2 — Wave 1 regression tests (findings 5, 6, 7, 8, 9, 10, 11, 13,
    // 14, 15, 17, 18, 19, 21, 42).
    // -----------------------------------------------------------------------

    /// Findings 5, 8, 10, 36: Take Control holds `LOCK_EX` on stable
    /// `NAME.control.lock` (with `pid start_ticks`) + `NAME.control`, rejects
    /// when a legacy `olo-screen` binary lacks helper-control support, ignores
    /// PID-reused stale markers with mismatched start_ticks, and rejects when
    /// `NAME.control.lock` holds an active `LOCK_SH` action lease.
    #[test]
    fn review2_w1_control_lock_stable_inode_stale_reuse_and_legacy_cli() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = scratch_dir("r2-w1-ctl");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);
        let sock = rt.join("ctl.sock");
        let mut stub = SessionStub::start("ctl");
        let _server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 7));
        write_screen_json(&rt, "ctl", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_path = rt.join("olo-screens/ctl.control");
        let stable_path = rt.join("olo-screens/ctl.control.lock");

        // (A) Finding 5 & 8: normal acquisition creates `ctl.control.lock` with
        // `LOCK_EX` and `"{pid} {start_ticks}"`.
        let mut c1 = spawn_stream(&dir, "ctl");
        let mut in1 = c1.stdin.take().unwrap();
        let rx1 = server_msg_channel(c1.stdout.take().unwrap());
        let _ = rx1.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in1, &ClientMsg::Control { on: true }).unwrap();
        let p1 = c1.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p1.as_str()) {
            assert!(Instant::now() < until, "c1 did not write ctl.control");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(stable_path.exists(), "Take Control must create stable NAME.control.lock");
        let stable_text = std::fs::read_to_string(&stable_path).unwrap();
        let parts: Vec<&str> = stable_text.split_whitespace().collect();
        assert_eq!(parts.len(), 2, "NAME.control.lock must store '<pid> <start_ticks>', got {stable_text:?}");
        assert_eq!(parts[0], p1);
        assert!(parts[1].parse::<u64>().unwrap() > 0);
        // Verify LOCK_EX is actually held on NAME.control.lock.
        let probe = std::fs::OpenOptions::new().read(true).write(true).open(&stable_path).unwrap();
        let lock_res = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
        assert_ne!(lock_res, 0, "NAME.control.lock must hold LOCK_EX while controlling");
        drop(probe);

        drop(in1);
        assert!(wait_child_timeout(&mut c1, Duration::from_secs(3)).success());
        assert!(!ctl_path.exists(), "ctl.control must be removed on release");

        // (B) Finding 36: holding LOCK_SH on `ctl.control.lock` (an in-flight agent
        // CLI ActionLease) blocks Take Control cleanly!
        let lease = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&stable_path).unwrap();
        assert_eq!(unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) }, 0);
        let mut c2 = spawn_stream(&dir, "ctl");
        let mut in2 = c2.stdin.take().unwrap();
        let rx2 = server_msg_channel(c2.stdout.take().unwrap());
        let _ = rx2.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in2, &ClientMsg::Control { on: true }).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut saw_refusal = false;
        while Instant::now() < until {
            match rx2.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(ServerMsg::Status(st))) if st.error => {
                    saw_refusal = true;
                    break;
                }
                Ok(Some(other)) => {
                    if format!("{other:?}").contains("held: false") {
                        saw_refusal = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(saw_refusal, "LOCK_SH on NAME.control.lock must refuse Take Control");
        assert!(!ctl_path.exists(), "refused Take Control must not publish ctl.control");
        drop(in2);
        assert!(wait_child_timeout(&mut c2, Duration::from_secs(3)).success());
        drop(lease);

        // (C) Finding 8: PID reuse with mismatched start_ticks in `ctl.control.lock`
        // must NOT block Take Control!
        let mut live_helper = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let reused_pid = live_helper.id();
        std::fs::write(&ctl_path, reused_pid.to_string()).unwrap();
        // Record an impossible start_ticks=1 (cannot match a newly spawned process).
        std::fs::write(&stable_path, format!("{reused_pid} 1")).unwrap();

        let mut c3 = spawn_stream(&dir, "ctl");
        let mut in3 = c3.stdin.take().unwrap();
        let rx3 = server_msg_channel(c3.stdout.take().unwrap());
        let _ = rx3.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in3, &ClientMsg::Control { on: true }).unwrap();
        let p3 = c3.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p3.as_str()) {
            assert!(Instant::now() < until, "PID-reused stale .control with mismatched start_ticks blocked Take Control");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(in3);
        assert!(wait_child_timeout(&mut c3, Duration::from_secs(3)).success());
        drop(live_helper.stdin.take());
        assert!(wait_child_timeout(&mut live_helper, Duration::from_secs(3)).success());

        // (D) Finding 10: a legacy host `olo-screen` lacking helper `.control` support
        // must cause Take Control to be refused cleanly!
        let legacy_bin = dir.join("desktop/bin/olo-screen");
        std::fs::create_dir_all(legacy_bin.parent().unwrap()).unwrap();
        std::fs::write(&legacy_bin, "#!/bin/sh\necho 'legacy olo-screen'\n").unwrap();
        std::fs::set_permissions(&legacy_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut c4 = spawn_stream(&dir, "ctl");
        let mut in4 = c4.stdin.take().unwrap();
        let rx4 = server_msg_channel(c4.stdout.take().unwrap());
        let _ = rx4.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in4, &ClientMsg::Control { on: true }).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut legacy_refused = false;
        while Instant::now() < until {
            match rx4.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(ServerMsg::Status(st))) if st.error => {
                    legacy_refused = true;
                    break;
                }
                Ok(Some(other)) => {
                    if format!("{other:?}").contains("held: false") {
                        legacy_refused = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(legacy_refused, "legacy host olo-screen must refuse Take Control");
        assert!(!ctl_path.exists(), "legacy refusal must not create ctl.control");
        drop(in4);
        assert!(wait_child_timeout(&mut c4, Duration::from_secs(3)).success());
    }

    /// Findings 6 & 42: symlink screen directory under `$XDG_STATE_HOME/olo/screens/alias`
    /// is never listed by `follow` and never read by `history-stream`; and
    /// `read_follow_event` rejects invalid/option/path screen names on decode.
    #[test]
    fn review2_w1_symlink_screen_dir_and_follow_name_validation() {
        use std::os::unix::fs::symlink;

        let dir = scratch_dir("r2-w1-sym");
        let rt = dir.join("run");
        let state = dir.join("state");
        let real_dir = state.join("olo/screens/real");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(real_dir.join("actions.jsonl"), "{\"at\":1000,\"ms\":10,\"kind\":\"click\",\"args\":[]}\n").unwrap();
        std::fs::write(real_dir.join("title"), "Secret Real\n").unwrap();
        symlink(&real_dir, state.join("olo/screens/alias")).unwrap();

        // 1. `follow` must list `real` and MUST NOT list symlink `alias`!
        let mut follow = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let fin = follow.stdin.take().unwrap();
        let mut freader = BufReader::new(follow.stdout.take().unwrap());
        let ev = read_follow_event(&mut freader).unwrap().unwrap();
        let FollowEvent::Snapshot { screens, .. } = ev else { panic!("expected Snapshot, got {ev:?}") };
        assert!(screens.iter().any(|s| s.name == "real"), "expected real screen: {screens:?}");
        assert!(!screens.iter().any(|s| s.name == "alias"), "symlink screen dir must NOT be listed: {screens:?}");
        drop(fin);
        assert!(wait_child_timeout(&mut follow, Duration::from_secs(3)).success());

        // 2. `history-stream alias` must NOT read `real`'s actions through symlink!
        let mut hist = Command::new(HELPER)
            .args(["screen", "history-stream", "alias"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let hin = hist.stdin.take().unwrap();
        let mut hreader = BufReader::new(hist.stdout.take().unwrap());
        let first = read_history_msg(&mut hreader).unwrap().unwrap();
        match first {
            HistoryMsg::Actions { lines, .. } => assert!(lines.is_empty(), "history-stream followed symlink screen dir and leaked actions: {lines:?}"),
            HistoryMsg::Error { .. } => {}
            other => panic!("unexpected first history msg for symlink dir: {other:?}"),
        }
        drop(hin);
        let _ = wait_child_timeout(&mut hist, Duration::from_secs(3));

        // 3. Finding 42: `read_follow_event` rejects invalid screen names in every variant.
        for bad in [
            "{\"t\":\"gone\",\"name\":\"../escape\"}\n",
            "{\"t\":\"gone\",\"name\":\"--clear\"}\n",
            "{\"t\":\"last_used\",\"name\":\"../escape\",\"last_used\":1000}\n",
            "{\"t\":\"screen\",\"screen\":{\"name\":\"--bad\",\"state\":\"stopped\",\"width\":100,\"height\":100,\"scale\":1.0,\"sound\":false}}\n",
            "{\"t\":\"snapshot\",\"proto\":1,\"status\":{\"installed\":null,\"default_on_host\":false},\"screens\":[{\"name\":\"a/b\",\"state\":\"stopped\",\"width\":100,\"height\":100,\"scale\":1.0,\"sound\":false}]}\n",
        ] {
            let mut cur = std::io::Cursor::new(bad.as_bytes());
            let res = read_follow_event(&mut cur);
            assert!(res.is_err(), "read_follow_event accepted invalid screen name in {bad}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 7, 9, 13:
    /// (A) Compositor disconnect/reconnect revokes `.control` on disk immediately;
    /// (B) `[release(K), press(K)]` in ONE batch keeps `K` tracked and releases it on EOF;
    /// (C) Single >64-event batch is split to <=64 per IPC request, and >512 release
    ///     flooding enforces queue bounds and revokes control instead of unbounded growth.
    #[test]
    fn review2_w1_reconnect_revokes_control_wire_order_held_and_input_bounds() {
        let dir = scratch_dir("r2-w1-inp");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);
        let sock = rt.join("inp2.sock");
        let mut stub = SessionStub::start("inp2");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 9));
        write_screen_json(&rt, "inp2", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_path = rt.join("olo-screens/inp2.control");

        // (A) Finding 7: compositor drops subscriber socket (in-place restart) while controlling ->
        // helper MUST drop `.control` on disk as it transitions to Reconnecting!
        let mut c1 = spawn_stream(&dir, "inp2");
        let mut in1 = c1.stdin.take().unwrap();
        let rx1 = server_msg_channel(c1.stdout.take().unwrap());
        server.wait_subscriber(Duration::from_secs(5));
        let _ = rx1.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in1, &ClientMsg::Control { on: true }).unwrap();
        let p1 = c1.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p1.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        server.disconnect_subscribers();
        // Wait for Reconnecting status.
        let until = Instant::now() + Duration::from_secs(5);
        let mut saw_reconn = false;
        while Instant::now() < until {
            match rx1.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(ServerMsg::Status(st))) if st.reconnecting => {
                    saw_reconn = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_reconn, "expected Reconnecting status after compositor drop");
        assert!(!ctl_path.exists(), "Finding 7: .control MUST be released before/on Reconnecting!");
        drop(in1);
        assert!(wait_child_timeout(&mut c1, Duration::from_secs(3)).success());

        // (B) Finding 9 + 13a: `[release(30), press(30)]` in one batch keeps key 30 down
        // so disconnect synthesizes key 30 release; AND a single 90-event batch splits <= 64!
        let mut c2 = spawn_stream(&dir, "inp2");
        let mut in2 = c2.stdin.take().unwrap();
        let rx2 = server_msg_channel(c2.stdout.take().unwrap());
        let _ = rx2.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in2, &ClientMsg::Control { on: true }).unwrap();
        let p2 = c2.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p2.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(rx2.recv_timeout(Duration::from_secs(2)), Ok(Some(ServerMsg::Control { held: true, .. }))), "expected viewer 2 Control(true) ack");
        let base_inputs = server.received_inputs().len();
        write_client_msg(&mut in2, &ClientMsg::Input { json_events: r#"[{"type":"key","key":30,"pressed":true}]"#.into() }).unwrap();
        server.wait_inputs(base_inputs + 1, Duration::from_secs(3));
        // Now release AND re-press key 30 in the SAME batch!
        write_client_msg(&mut in2, &ClientMsg::Input { json_events: r#"[{"type":"key","key":30,"pressed":false},{"type":"key","key":30,"pressed":true}]"#.into() }).unwrap();
        server.wait_inputs(base_inputs + 3, Duration::from_secs(3));

        // Send a single 90-event batch (scroll events that don't coalesce) -> each IPC call <= 64!
        let big: Vec<String> = (0..90).map(|i| format!(r#"{{"type":"scroll","dx":0.0,"dy":{}.0}}"#, i + 1)).collect();
        let big_json = format!("[{}]", big.join(","));
        write_client_msg(&mut in2, &ClientMsg::Input { json_events: big_json }).unwrap();
        server.wait_inputs(base_inputs + 93, Duration::from_secs(3));
        let max_batch = server.batch_sizes().into_iter().max().unwrap_or(0);
        assert!(max_batch <= 64, "Finding 13: IPC synthetic batch exceeded 64 events (got {max_batch})");

        // Close stdin -> MUST synthesize final release for key 30!
        drop(in2);
        assert!(wait_child_timeout(&mut c2, Duration::from_secs(3)).success());
        let all = server.received_inputs();
        let final_30_up = all[base_inputs + 3..].iter().any(|e| e["type"] == "key" && e["key"] == 30 && e["pressed"] == false);
        assert!(final_30_up, "Finding 9: key 30 from [release, press] batch was NOT released on disconnect!");

        // (C) Finding 13b: flooding >512 release events while compositor is held MUST NOT
        // accumulate unboundedly; it must overflow-revoke control and drop `.control`.
        let mut c3 = spawn_stream(&dir, "inp2");
        let mut in3 = c3.stdin.take().unwrap();
        let rx3 = server_msg_channel(c3.stdout.take().unwrap());
        let _ = rx3.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        write_client_msg(&mut in3, &ClientMsg::Control { on: true }).unwrap();
        let p3 = c3.id().to_string();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::read_to_string(&ctl_path).ok().as_deref() != Some(p3.as_str()) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        server.hold_synthetic(true);
        // Send one key press to block the input worker on the gated mock server.
        write_client_msg(&mut in3, &ClientMsg::Input { json_events: r#"[{"type":"key","key":50,"pressed":true}]"#.into() }).unwrap();
        assert!(server.wait_synthetic_blocked(Duration::from_secs(3)), "input worker did not block on gate");
        // Now flood 20 batches of 40 release events (800 releases > 512 cap)!
        let rel_batch: String = format!("[{}]", (0..40).map(|i| format!(r#"{{"type":"key","key":{},"pressed":false}}"#, 100 + i)).collect::<Vec<_>>().join(","));
        for _ in 0..20 {
            let _ = write_client_msg(&mut in3, &ClientMsg::Input { json_events: rel_batch.clone() });
        }
        // Release the gate: worker must observe overflow, unwind key 50, and drop `.control`!
        server.hold_synthetic(false);
        let until = Instant::now() + Duration::from_secs(3);
        while ctl_path.exists() {
            assert!(Instant::now() < until, "Finding 13: release flood did not trigger overflow control revocation");
            std::thread::sleep(Duration::from_millis(15));
        }
        drop(in3);
        assert!(wait_child_timeout(&mut c3, Duration::from_secs(3)).success());
    }

    /// Findings 11, 14, 18:
    /// (A) Host export enforces `OLO_SCREEN_MAX_EXPORT_BYTES` during encoding and aborts;
    /// (B) `SIGKILL` of export helper kills child encoder via `PR_SET_PDEATHSIG` and
    ///     next export reclaims the crashed helper's unlocked scratch directory.
    #[test]
    fn review2_w1_export_budget_pdeathsig_and_crash_reclaim() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = scratch_dir("r2-w1-exp");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        let export_disk = dir.join("export-disk");
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&export_disk).unwrap();

        // (A) Finding 11: fake exporter writes 64 KiB while budget is 4096 bytes.
        let tool = desktop.join("bin/olo-screen");
        let big_script = "#!/bin/sh\n\
            out=''\n\
            while [ $# -gt 0 ]; do\n\
              if [ \"$1\" = '--export' ]; then out=\"$2\"; shift 2; continue; fi\n\
              shift\n\
            done\n\
            dd if=/dev/zero of=\"$out\" bs=1024 count=64 2>/dev/null\n\
            echo 'olo-screen: exporting… 50%' >&2\n\
            sleep 5\n";
        std::fs::write(&tool, big_script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut c1 = Command::new(HELPER)
            .args(["screen", "export", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .env("OLO_SCREEN_MAX_EXPORT_BYTES", "4096")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let _in1 = c1.stdin.take().unwrap();
        let rx1 = export_msg_channel(c1.stdout.take().unwrap());
        let mut saw_err = false;
        let until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            match rx1.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(ExportMsg::Error { message })) => {
                    assert!(message.to_lowercase().contains("limit") || message.to_lowercase().contains("size") || message.to_lowercase().contains("budget"), "unexpected error: {message}");
                    saw_err = true;
                    break;
                }
                Ok(Some(ExportMsg::Meta(_))) => panic!("over-budget export must not emit Meta"),
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_err, "Finding 11: over-budget export did not abort with ExportMsg::Error");
        assert!(wait_child_timeout(&mut c1, Duration::from_secs(3)).success());

        // (B) Finding 18: SIGKILLing the export helper must kill the child encoder via
        // PR_SET_PDEATHSIG AND leave a lockable orphan dir that the next export reclaims!
        let enc_pid_file = dir.join("enc.pid");
        let slow_script = format!(
            "#!/bin/sh\n\
             echo $$ > '{}'\n\
             echo 'olo-screen: exporting… 10%' >&2\n\
             exec sleep 30\n",
            enc_pid_file.display()
        );
        std::fs::write(&tool, slow_script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut c2 = Command::new(HELPER)
            .args(["screen", "export", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let _in2 = c2.stdin.take().unwrap();
        let rx2 = export_msg_channel(c2.stdout.take().unwrap());
        loop {
            match rx2.recv_timeout(Duration::from_secs(5)).unwrap() {
                Some(ExportMsg::Progress { done }) if done > 0.0 => break,
                Some(_) => continue,
                None => panic!("slow exporter ended early"),
            }
        }
        let enc_pid: i32 = std::fs::read_to_string(&enc_pid_file).unwrap().trim().parse().unwrap();
        let enc_pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, enc_pid, 0 as libc::c_uint) as i32 };
        assert!(enc_pidfd >= 0, "failed to open pidfd for encoder {enc_pid}");

        // Hard SIGKILL the export helper (no drop / no cleanup handler can run!).
        c2.kill().unwrap();
        let _ = c2.wait();

        // Encoder child MUST die promptly from PR_SET_PDEATHSIG(SIGKILL)!
        let mut pfd = libc::pollfd { fd: enc_pidfd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, 2000) };
        if r <= 0 {
            unsafe { libc::syscall(libc::SYS_pidfd_send_signal, enc_pidfd, libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0 as libc::c_uint) };
        }
        unsafe { libc::close(enc_pidfd) };
        assert!(r > 0 && pfd.revents != 0, "Finding 18: encoder child survived SIGKILL of export helper!");

        // Orphaned scratch dir should still exist right after SIGKILL, and the next export must reclaim it!
        let orphans_before: Vec<_> = std::fs::read_dir(&export_disk).unwrap().flatten().collect();
        assert!(!orphans_before.is_empty(), "expected orphaned scratch dir after SIGKILL");

        let ok_script = "#!/bin/sh\n\
            out=''\n\
            while [ $# -gt 0 ]; do\n\
              if [ \"$1\" = '--export' ]; then out=\"$2\"; shift 2; continue; fi\n\
              shift\n\
            done\n\
            printf 'okmp4' > \"$out\"\n\
            echo '{\"frames\":1,\"pictures_encoded\":1,\"encoder\":\"cpu\",\"size\":[64,64]}'\n";
        std::fs::write(&tool, ok_script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut c3 = Command::new(HELPER)
            .args(["screen", "export", "qa"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let _in3 = c3.stdin.take().unwrap();
        let rx3 = export_msg_channel(c3.stdout.take().unwrap());
        while let Ok(Some(msg)) = rx3.recv_timeout(Duration::from_secs(5)) {
            if matches!(msg, ExportMsg::Done) {
                break;
            }
        }
        assert!(wait_child_timeout(&mut c3, Duration::from_secs(3)).success());
        let remaining: Vec<_> = std::fs::read_dir(&export_disk).unwrap().flatten().collect();
        assert!(remaining.is_empty(), "Finding 18: crashed helper's orphaned scratch dir was NOT reclaimed: {remaining:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finding 15: Live refuses an oversized screen (7680x4320@3x = 23040x12960)
    /// BEFORE requesting `Request::Screenshot` (zero screenshots taken on mock
    /// server, zero tmpfs bloat!).
    #[test]
    fn review2_w1_pre_capture_oversized_screen_never_requests_screenshot() {
        let dir = scratch_dir("r2-w1-oversize-live");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let sock = rt.join("huge.sock");
        let mut stub = SessionStub::start("huge");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), 128, 64, make_rgba(128, 64, 1));
        // Write a legal 7680x4320 @ 3.0x runtime descriptor (23040x12960 physical pixels > 16384/256MiB).
        let rt_dir = rt.join("olo-screens");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::write(
            rt_dir.join("huge.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "huge",
                "wayland_display": "wayland-huge",
                "olo_socket": sock,
                "pid": stub.pid(),
                "width": 7680,
                "height": 4320,
                "scale": 3.0,
                "sound": false,
                "started": 1000
            }))
            .unwrap(),
        )
        .unwrap();
        let _cleanup = Cleanup(dir.clone(), vec![stub]);

        let mut child = spawn_stream(&dir, "huge");
        let stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        let msg = rx.recv_timeout(Duration::from_secs(4)).expect("expected size warning/status").unwrap();
        assert!(
            matches!(&msg, ServerMsg::Status(st) if st.message.contains("23040") || st.message.contains("limit")) || format!("{msg:?}").contains("23040") || format!("{msg:?}").contains("limit"),
            "expected size limit warning, got {msg:?}"
        );
        assert_eq!(server.screenshot_count(), 0, "Finding 15: helper requested Screenshot on 23040x12960 screen BEFORE checking dimensions!");
        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());
    }

    /// Finding 19: A FIFO named `frames-0000000000000001.olof` in a screen's
    /// recording folder and a FIFO at `$XDG_CONFIG_HOME/olo/screens-on` MUST NOT
    /// hang `history-stream` or `screen default`!
    #[test]
    fn review2_w1_fifo_in_recordings_and_screens_on_never_hang() {
        let dir = scratch_dir("r2-w1-fifo");
        let rt = dir.join("run");
        let state = dir.join("state");
        let cfg = dir.join("cfg");
        let sdir = state.join("olo/screens/fifosc");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::create_dir_all(cfg.join("olo")).unwrap();

        let fifo_olof = sdir.join("frames-0000000000000001.olof");
        let c_olof = std::ffi::CString::new(fifo_olof.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_olof.as_ptr(), 0o600) }, 0);

        let fifo_cfg = cfg.join("olo/screens-on");
        let c_cfg = std::ffi::CString::new(fifo_cfg.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_cfg.as_ptr(), 0o600) }, 0);

        // 1. `screen default` must NOT block on FIFO `screens-on`.
        let mut def = Command::new(HELPER)
            .args(["screen", "default"])
            .env("HOME", dir.join("home"))
            .env("XDG_CONFIG_HOME", &cfg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(wait_child_timeout(&mut def, Duration::from_secs(2)).success(), "Finding 19: screen default hung on FIFO screens-on");

        // 2. `history-stream fifosc` must NOT block on FIFO `.olof`!
        let mut hist = Command::new(HELPER)
            .args(["screen", "history-stream", "fifosc"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &cfg)
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let hin = hist.stdin.take().unwrap();
        let mut hreader = BufReader::new(hist.stdout.take().unwrap());
        let first = read_history_msg(&mut hreader).unwrap().unwrap();
        assert!(matches!(first, HistoryMsg::Actions { .. }), "expected initial Actions without hanging on FIFO: {first:?}");
        drop(hin);
        assert!(wait_child_timeout(&mut hist, Duration::from_secs(3)).success());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finding 21: `read_server_msg` and `read_history_msg` must reject a
    /// bogus 200 MiB `MSG_FRAME` / `MSG_HISTORY_FRAME` header from its fixed
    /// prefix with `InvalidData` (not `UnexpectedEof` after allocating 200 MiB!).
    #[test]
    fn review2_w1_streaming_wire_prefix_validation_before_allocation() {
        use olo_rex_link::screen_wire::{MSG_FRAME, MSG_HISTORY_FRAME};

        // Live MSG_FRAME claiming 200 MiB payload, followed by only 8-byte seq + 20-byte bogus magic.
        let mut buf = Vec::new();
        buf.push(MSG_FRAME);
        buf.extend_from_slice(&200_000_000u32.to_le_bytes());
        buf.extend_from_slice(&1u64.to_le_bytes()); // seq
        buf.extend_from_slice(b"NOPE"); // invalid OLOT magic!
        buf.extend_from_slice(&[0u8; 16]);
        let mut cur = std::io::Cursor::new(&buf);
        let err = read_server_msg(&mut cur).expect_err("bogus OLOT prefix must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "Finding 21: expected immediate InvalidData from prefix check (not UnexpectedEof after 200 MiB alloc): {err}");

        // History MSG_HISTORY_FRAME claiming 200 MiB payload, followed by req + frame_at + 20-byte bogus magic.
        let mut hbuf = Vec::new();
        hbuf.push(MSG_HISTORY_FRAME);
        hbuf.extend_from_slice(&200_000_000u32.to_le_bytes());
        hbuf.extend_from_slice(&1u64.to_le_bytes()); // req
        hbuf.extend_from_slice(&1000u64.to_le_bytes()); // frame_at
        hbuf.extend_from_slice(b"NOPE");
        hbuf.extend_from_slice(&[0u8; 16]);
        let mut hcur = std::io::Cursor::new(&hbuf);
        let herr = read_history_msg(&mut hcur).expect_err("bogus history OLOT prefix must fail");
        assert_eq!(herr.kind(), std::io::ErrorKind::InvalidData, "Finding 21: expected immediate InvalidData from history prefix check: {herr}");
    }

    // -----------------------------------------------------------------------
    // Review 2 — Wave 2 & Wave 3 regression tests (findings 3, 25, 26, 27, 33,
    // 34, 35, 36, 37, 38, 40, 43, 44, 45, 48, 49, 50, 54).
    // -----------------------------------------------------------------------

    /// Findings 26 & 35:
    /// (A) A paused/idle live stream wakes immediately on `session_pidfd` exit
    ///     even when the compositor socket stays open and silent (emits ScreenGone);
    /// (B) Idle `follow`, `history-stream`, and `stream` all exit promptly when
    ///     stdout closes even while stdin remains open!
    #[test]
    fn review2_w2_idle_paused_stream_session_death_and_stdout_close_exits() {
        use std::os::unix::process::CommandExt as _;

        let dir = scratch_dir("r2-w2-idle");
        let rt = dir.join("run");
        let state = dir.join("state");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(state.join("olo/screens/idle")).unwrap();
        let (w, h) = (128u32, 64u32);

        // (A) Finding 26: spawn a parent `olo-session` wrapper whose child is
        // `fake_session_relay` so killing ONLY the parent leaves the compositor
        // socket open and connected while `session_pid` is dead!
        let sock = rt.join("idle.sock");
        let backend = rt.join(".idle-backend.sock");
        let _ = std::fs::remove_file(&backend);
        let listener = UnixListener::bind(&backend).unwrap();
        let held = Arc::new(Mutex::new(Vec::new()));
        let held_bg = held.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let Ok(mut r_stream) = conn.try_clone() else { continue };
                let mut w_stream = conn;
                let frame = make_rgba(w, h, 11);
                let held_c = held_bg.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(&mut r_stream);
                    let mut line = String::new();
                    while reader.read_line(&mut line).unwrap_or(0) > 0 {
                        if let Ok(req) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                            match req["type"].as_str() {
                                Some("subscribe_to") => {
                                    let _ = w_stream.write_all(b"{\"type\":\"ok\"}\n");
                                    let _ = w_stream.flush();
                                }
                                Some("screenshot") => {
                                    if let Some(p) = req["path"].as_str() {
                                        let mut out = Vec::with_capacity(8 + frame.len());
                                        out.extend_from_slice(&w.to_le_bytes());
                                        out.extend_from_slice(&h.to_le_bytes());
                                        out.extend_from_slice(&frame);
                                        let _ = std::fs::write(p, &out);
                                    }
                                    let _ = w_stream.write_all(b"{\"type\":\"ok\"}\n");
                                    let _ = w_stream.flush();
                                }
                                _ => {}
                            }
                        }
                        line.clear();
                    }
                    held_c.lock().unwrap().push(w_stream);
                });
            }
        });

        let exe = std::env::current_exe().unwrap();
        let script = format!("'{}' --ignored --exact live_and_follow::fake_session_relay --nocapture <&0 & echo $!; wait", exe.display());
        let mut parent =
            Command::new("/bin/sh").arg0("olo-session").args(["-c", &script]).env("OLO_SCREEN", "idle").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut p_in = parent.stdin.take().unwrap();
        let mut p_out = BufReader::new(parent.stdout.take().unwrap());
        let mut relay_pid_line = String::new();
        p_out.read_line(&mut relay_pid_line).unwrap();
        let relay_pid: i32 = relay_pid_line.trim().parse().expect("expected relay child pid");
        let relay_pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, relay_pid, 0 as libc::c_uint) as i32 };
        assert!(relay_pidfd >= 0);

        writeln!(p_in, "{}\t{}", sock.display(), backend.display()).unwrap();
        p_in.flush().unwrap();
        let mut ready_line = String::new();
        loop {
            ready_line.clear();
            assert!(p_out.read_line(&mut ready_line).unwrap() > 0);
            if ready_line.trim() == "ready" {
                break;
            }
        }
        write_screen_json(&rt, "idle", parent.id() as i32, &sock, w, h);

        let mut c_paused = spawn_stream(&dir, "idle");
        let mut in_paused = c_paused.stdin.take().unwrap();
        let rx_paused = server_msg_channel(c_paused.stdout.take().unwrap());
        let ServerMsg::Frame { seq: s1, .. } = rx_paused.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() else {
            panic!("expected initial frame");
        };
        write_client_msg(&mut in_paused, &ClientMsg::Ack { seq: s1 }).unwrap();
        // Pause the stream so `stream_loop` sleeps indefinitely in `cvar.wait(s)`!
        write_client_msg(&mut in_paused, &ClientMsg::Pause { paused: true }).unwrap();
        std::thread::sleep(Duration::from_millis(80));

        // Kill ONLY the `olo-session` parent PID while the relay child keeps the socket OPEN!
        parent.kill().unwrap();
        let _ = parent.wait();

        let gone_msg = rx_paused.recv_timeout(Duration::from_secs(2));
        let exited = wait_child_timeout(&mut c_paused, Duration::from_secs(2));
        // Clean up the orphaned relay child via pidfd before asserting!
        drop(p_in);
        unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, relay_pidfd, libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0 as libc::c_uint);
            libc::close(relay_pidfd);
        }
        assert!(matches!(gone_msg, Ok(Some(ServerMsg::ScreenGone { .. }))), "Finding 26: paused stream did not wake on session_pidfd death while socket stayed open: {gone_msg:?}");
        assert!(exited.success(), "paused stream should exit cleanly after session death");

        // (B) Finding 35: idle `follow`, `history-stream`, and `stream` MUST exit
        // promptly when stdout closes while stdin stays OPEN!
        let mut stub2 = SessionStub::start("idle2");
        let sock2 = rt.join("idle2.sock");
        let _srv2 = FakeAppServer::start_in(&mut stub2, sock2.clone(), w, h, make_rgba(w, h, 12));
        write_screen_json(&rt, "idle2", stub2.pid(), &sock2, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub2]);

        // 1. `follow`
        let mut c_fol = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _keep_fol_in = c_fol.stdin.take().unwrap();
        let mut fol_out = BufReader::new(c_fol.stdout.take().unwrap());
        let _ = read_follow_event(&mut fol_out).unwrap().unwrap();
        drop(fol_out); // Close stdout while stdin stays OPEN!
        assert!(wait_child_timeout(&mut c_fol, Duration::from_secs(2)).success(), "Finding 35: idle follow hung after stdout closed with stdin open");

        // 2. `history-stream`
        let mut c_hist = Command::new(HELPER)
            .args(["screen", "history-stream", "idle"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _keep_hist_in = c_hist.stdin.take().unwrap();
        let mut hist_out = BufReader::new(c_hist.stdout.take().unwrap());
        let _ = read_history_msg(&mut hist_out).unwrap().unwrap();
        let _ = read_history_msg(&mut hist_out).unwrap().unwrap();
        let _ = read_history_msg(&mut hist_out).unwrap().unwrap();
        drop(hist_out); // Close stdout while stdin stays OPEN!
        assert!(wait_child_timeout(&mut c_hist, Duration::from_secs(2)).success(), "Finding 35: idle history-stream hung after stdout closed with stdin open");

        // 3. `stream` (holding `.control` too -> must release `.control` on stdout close!)
        let ctl_path2 = rt.join("olo-screens/idle2.control");
        let mut c_str = spawn_stream(&dir, "idle2");
        let mut keep_str_in = c_str.stdin.take().unwrap();
        let mut str_out = BufReader::new(c_str.stdout.take().unwrap());
        let _ = read_server_msg(&mut str_out).unwrap().unwrap();
        write_client_msg(&mut keep_str_in, &ClientMsg::Control { on: true }).unwrap();
        let _ = read_server_msg(&mut str_out).unwrap().unwrap();
        assert!(ctl_path2.exists());
        drop(str_out); // Close stdout while stdin stays OPEN!
        assert!(wait_child_timeout(&mut c_str, Duration::from_secs(2)).success(), "Finding 35: idle stream hung after stdout closed with stdin open");
        assert!(!ctl_path2.exists(), "Finding 35: stream must release .control when stdout closes");
    }

    /// Findings 33 & 34: oversized `.olof` recording header (>16384 side) emits
    /// `Index { last_size: None }` without killing history-stream, returns
    /// `HistoryMsg::FrameError` on `FrameAt` for the oversized timestamp, keeps
    /// answering normal `FrameAt` requests afterwards, and emits only incremental
    /// marks (`reset: false`) when a new frame is appended.
    #[test]
    fn review2_w2_history_oversized_frame_error_and_incremental_append() {
        let dir = scratch_dir("r2-w2-hist-ov");
        let rt = dir.join("run");
        let state = dir.join("state");
        let sdir = state.join("olo/screens/ov");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&sdir).unwrap();

        // Segment 1: valid 64x64 frame at t=1000.
        let (w, h) = (64usize, 64usize);
        let rgba: Vec<u8> = std::iter::repeat_n([10u8, 200u8, 30u8, 255u8], w * h).flatten().collect();
        let mut writer1 = OlofWriter::new(sdir.clone(), 64 << 20);
        writer1.add(Frame { width: w, height: h, rgba: rgba.clone() }, 1_000).unwrap();
        drop(writer1);

        // Segment 2: raw `.olof` header claiming 20000x12000 (>MAX_NET_DIM / >256MiB,
        // still within local MAX_LOCAL_DIM=24576) at t=2000 with tile_count=0.
        let mut raw_ov = Vec::new();
        raw_ov.extend_from_slice(b"OLOFRM1\n");
        let rec_len: u32 = 21; // at(8) + w(4) + h(4) + key(1) + tile_count(4)
        raw_ov.extend_from_slice(&rec_len.to_le_bytes());
        raw_ov.extend_from_slice(&2000u64.to_le_bytes());
        raw_ov.extend_from_slice(&20_000u32.to_le_bytes());
        raw_ov.extend_from_slice(&12_000u32.to_le_bytes());
        raw_ov.push(1u8); // keyframe
        raw_ov.extend_from_slice(&0u32.to_le_bytes()); // tile_count = 0
        std::fs::write(sdir.join("zzzz-oversize-2000.olof"), &raw_ov).unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "history-stream", "ov"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let rx = history_msg_channel(child.stdout.take().unwrap());

        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), Some(HistoryMsg::Actions { reset: true, .. })));
        let Some(HistoryMsg::Index { reset: true, last_size, frames }) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("expected initial Index");
        };
        assert_eq!(last_size, None, "Finding 33: oversized last frame must report last_size: None");
        assert_eq!(frames.len(), 2, "expected both timestamps indexed: {frames:?}");
        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), Some(HistoryMsg::SnapshotDone { .. })));

        // Ask for oversized frame at t=2000 -> must emit FrameError and stay alive!
        writeln!(stdin, r#"{{"op":"frame_at","req":1,"at":2000}}"#).unwrap();
        stdin.flush().unwrap();
        let Some(HistoryMsg::FrameError { req: 1, message }) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("Finding 33: expected FrameError for oversized frame_at");
        };
        assert!(message.contains("20000") || message.contains("limit"), "unexpected FrameError message: {message}");

        // Stream is STILL alive: ask for valid frame at t=1000 -> succeeds!
        writeln!(stdin, r#"{{"op":"frame_at","req":2,"at":1000}}"#).unwrap();
        stdin.flush().unwrap();
        let Some(HistoryMsg::Frame { req: 2, frame_at: 1000, .. }) = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
            panic!("stream should keep serving valid frames after FrameError");
        };

        // Finding 34: remove oversized segment, create a fresh segment, then append
        // one more frame to it and assert incremental `reset: false` with only 1 mark!
        let _ = std::fs::remove_file(sdir.join("zzzz-oversize-2000.olof"));
        let mut writer3 = OlofWriter::new(sdir.clone(), 64 << 20);
        writer3.add(Frame { width: w, height: h, rgba: rgba.clone() }, 3_000).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Some(HistoryMsg::Index { frames, .. }) if frames.iter().any(|m| m.at == 3000) => break,
                Some(_) if Instant::now() < until => continue,
                other => panic!("did not receive mark 3000: {other:?}"),
            }
        }
        // Append a genuinely changed frame so `OlofWriter::add` writes a new record!
        let rgba2: Vec<u8> = std::iter::repeat_n([200u8, 30u8, 10u8, 255u8], w * h).flatten().collect();
        writer3.add(Frame { width: w, height: h, rgba: rgba2 }, 3_100).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let mut saw_incremental = false;
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Some(HistoryMsg::Index { reset: false, frames, .. }) if frames.len() == 1 && frames[0].at == 3100 => {
                    saw_incremental = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(saw_incremental, "Finding 34: appending one frame must emit incremental reset=false with 1 mark");

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 36, 37, 38, 48: `ClientMsg::ControlSeq` produces seq-matched
    /// `ServerMsg::Control` grant/refusal/release-barrier acks, respects
    /// `LOCK_SH` action lease on `NAME.control.lock`, and forwards
    /// `SyntheticEvent::Wheel` to `app_server`.
    #[test]
    fn review2_w3_control_seq_action_lease_and_wheel_input() {
        use std::os::fd::AsRawFd as _;

        let dir = scratch_dir("r2-w3-ctlseq");
        let rt = dir.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        let (w, h) = (128u32, 64u32);
        let sock = rt.join("seq.sock");
        let mut stub = SessionStub::start("seq");
        let server = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 21));
        write_screen_json(&rt, "seq", stub.pid(), &sock, w, h);
        let _cleanup = Cleanup(dir.clone(), vec![stub]);
        let ctl_path = rt.join("olo-screens/seq.control");
        let lock_path = rt.join("olo-screens/seq.control.lock");

        let mut child = spawn_stream(&dir, "seq");
        let mut stdin = child.stdin.take().unwrap();
        let rx = server_msg_channel(child.stdout.take().unwrap());
        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();

        // 1. Grant with seq=7.
        write_client_msg(&mut stdin, &ClientMsg::ControlSeq { on: true, seq: 7 }).unwrap();
        let grant = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert_eq!(grant, ServerMsg::Control { held: true, seq: 7, reason: None });
        assert!(ctl_path.exists());

        // 2. Finding 48: wheel synthetic event accepted and forwarded to FakeAppServer!
        write_client_msg(&mut stdin, &ClientMsg::Input { json_events: r#"[{"type":"wheel","dx":0,"dy":-1}]"#.into() }).unwrap();
        let got = server.wait_inputs(1, Duration::from_secs(2));
        assert!(got.iter().any(|e| e["type"] == "wheel" && e["dx"] == 0 && e["dy"] == -1), "Finding 48: SyntheticEvent::Wheel was dropped instead of forwarded: {got:?}");

        // 3. Release barrier with seq=9: `.control` MUST already be gone when ack arrives!
        write_client_msg(&mut stdin, &ClientMsg::ControlSeq { on: false, seq: 9 }).unwrap();
        let rel = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert_eq!(rel, ServerMsg::Control { held: false, seq: 9, reason: None });
        assert!(!ctl_path.exists(), "Finding 38: .control still existed when Control(false) ack arrived");

        // 4. Finding 36: LOCK_SH lease causes seq-matched refusal!
        let lease = std::fs::OpenOptions::new().read(true).write(true).open(&lock_path).unwrap();
        assert_eq!(unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) }, 0);
        write_client_msg(&mut stdin, &ClientMsg::ControlSeq { on: true, seq: 11 }).unwrap();
        let refused = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert!(matches!(refused, ServerMsg::Control { held: false, seq: 11, reason: Some(_) }), "expected seq=11 refusal, got {refused:?}");
        drop(lease);

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());
    }

    /// Findings 40, 45, 49, 50:
    /// (A) A 2.4 MiB single valid JSON action line (>2 MiB batch size) is
    ///     delivered intact without dropping or shifting subsequent indices;
    /// (B) >10,050 action lines stream in multiple chunks followed by a single
    ///     `SnapshotDone` barrier;
    /// (C) Renaming the ancestor `$XDG_STATE_HOME/olo/screens` directory and
    ///     recreating `screens/anc` still delivers new actions without watch leaks!
    #[test]
    fn review2_w3_history_long_action_line_ancestor_replace_and_snapshot_done() {
        let dir = scratch_dir("r2-w3-hist-long");
        let rt = dir.join("run");
        let state = dir.join("state");
        let sdir = state.join("olo/screens/anc");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&sdir).unwrap();

        // (A) Finding 45: small line 0, 2.4 MiB valid JSON line 1, small line 2.
        let big_payload = "x".repeat(2_400_000);
        let line0 = r#"{"at":1000,"ms":5,"kind":"click","args":["first"]}"#.to_owned();
        let line1 = format!(r#"{{"at":1010,"ms":5,"kind":"type","args":["{big_payload}"]}}"#);
        let line2 = r#"{"at":1020,"ms":5,"kind":"click","args":["third"]}"#.to_owned();
        std::fs::write(sdir.join("actions.jsonl"), format!("{line0}\n{line1}\n{line2}\n")).unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "history-stream", "anc"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let rx = history_msg_channel(child.stdout.take().unwrap());

        let mut got_lines: Vec<String> = Vec::new();
        let mut saw_snap_done = false;
        let until = Instant::now() + Duration::from_secs(6);
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(3)).unwrap() {
                Some(HistoryMsg::Actions { lines, .. }) => got_lines.extend(lines),
                Some(HistoryMsg::Index { .. }) => {}
                Some(HistoryMsg::SnapshotDone { actions_gen, index_gen }) => {
                    assert_eq!(actions_gen, 1);
                    assert_eq!(index_gen, 1);
                    saw_snap_done = true;
                    break;
                }
                other => panic!("unexpected msg during initial long-line snapshot: {other:?}"),
            }
        }
        assert!(saw_snap_done, "expected SnapshotDone after multi-chunk actions");
        assert_eq!(got_lines.len(), 3, "Finding 45: 2.4 MiB action line was dropped (got {} lines instead of 3)!", got_lines.len());
        assert_eq!(got_lines[0], line0);
        assert_eq!(got_lines[1].len(), line1.len());
        assert_eq!(got_lines[2], line2);

        // (B) Finding 49: move the ANCESTOR `state/olo/screens` aside and recreate
        // `state/olo/screens/anc/actions.jsonl` -> history-stream MUST notice!
        let screens_root = state.join("olo/screens");
        let moved_root = state.join("olo/screens-moved-aside");
        std::fs::rename(&screens_root, &moved_root).unwrap();
        std::fs::create_dir_all(&sdir).unwrap();
        let after_move = r#"{"at":5000,"ms":5,"kind":"click","args":["after-ancestor-move"]}"#;
        std::fs::write(sdir.join("actions.jsonl"), format!("{after_move}\n")).unwrap();

        let mut saw_after_move = false;
        let until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Some(HistoryMsg::Actions { lines, .. })) if lines.iter().any(|l| l == after_move) => {
                    saw_after_move = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(saw_after_move, "Finding 49: history-stream was stranded when ancestor `screens/` directory was replaced!");

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(3)).success());

        // (C) Finding 40 & 50: >10,050 lines emit multiple Actions chunks followed by ONE SnapshotDone.
        let mut many = String::with_capacity(10_050 * 48);
        for i in 0..10_050usize {
            many.push_str(&format!("{{\"at\":{},\"ms\":1,\"kind\":\"click\",\"args\":[]}}\n", 1000 + i));
        }
        std::fs::write(sdir.join("actions.jsonl"), &many).unwrap();
        let mut child2 = Command::new(HELPER)
            .args(["screen", "history-stream", "anc"])
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", dir.join("home/.config"))
            .env("OLO_HOST_DESKTOP", dir.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin2 = child2.stdin.take().unwrap();
        let rx2 = history_msg_channel(child2.stdout.take().unwrap());
        let mut action_msgs = 0usize;
        let mut total_lines = 0usize;
        let mut done_msgs = 0usize;
        while let Ok(Some(msg)) = rx2.recv_timeout(Duration::from_secs(4)) {
            match msg {
                HistoryMsg::Actions { reset, lines } => {
                    assert_eq!(reset, action_msgs == 0, "only first chunk should set reset=true");
                    action_msgs += 1;
                    total_lines += lines.len();
                }
                HistoryMsg::Index { .. } => {}
                HistoryMsg::SnapshotDone { .. } => {
                    done_msgs += 1;
                    break;
                }
                other => panic!("unexpected msg: {other:?}"),
            }
        }
        assert!(action_msgs >= 2, "expected chunked Actions (>10000 lines), got {action_msgs}");
        assert_eq!(total_lines, 10_050);
        assert_eq!(done_msgs, 1);
        drop(stdin2);
        assert!(wait_child_timeout(&mut child2, Duration::from_secs(3)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Findings 3, 44, 54:
    /// (A) Host `olo-screen` supporting `--part` receives `--part 0,2-3` for
    ///     discontiguous exports AND `--protocol` advertises `"discontiguous_part"`;
    /// (B) `record_owner` binds `(screen_pid, screen_started)` so `follow`
    ///     discards stale owner records when a replacement session runs, and
    ///     `stop` unlinks `NAME.owner.json`;
    /// (C) `olo-rex-link screen stop NAME --expected-started N` refuses mismatched
    ///     generations without invoking host `olo-screen`, and succeeds when matched!
    #[test]
    fn review2_w3_export_discontiguous_part_stop_generation_and_owner_binding() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = scratch_dir("r2-w3-exp-own");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        let export_disk = dir.join("export-disk");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(&export_disk).unwrap();

        // (A) Finding 44: fake host `olo-screen` that understands `--part`!
        let part_log = dir.join("part-args.txt");
        let stop_log = dir.join("stop-args.txt");
        let tool = desktop.join("bin/olo-screen");
        let script = format!(
            "#!/bin/sh\n\
             OLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\n\
             if [ \"$1\" = 'history' ]; then\n\
               printf '%s\\n' \"$*\" > '{part_log}'\n\
               out=''\n\
               while [ $# -gt 0 ]; do\n\
                 if [ \"$1\" = '--export' ]; then out=\"$2\"; shift 2; continue; fi\n\
                 if [ \"$1\" = '--part' ]; then shift 2; continue; fi\n\
                 shift\n\
               done\n\
               printf 'mp4bytes' > \"$out\"\n\
               echo '{{\"frames\":4,\"pictures_encoded\":2,\"encoder\":\"cpu\",\"size\":[64,64]}}'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = 'stop' ]; then\n\
               printf '%s\\n' \"$*\" > '{stop_log}'\n\
               exit 0\n\
             fi\n\
             exit 0\n",
            part_log = part_log.display(),
            stop_log = stop_log.display(),
        );
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Check `--protocol` advertises `discontiguous_part`.
        let proto_out = Command::new(HELPER).args(["screen", "--protocol"]).env("HOME", &home).env("OLO_HOST_DESKTOP", &desktop).env("OLO_ALLOW_TEST_SCREEN_CAPS", "1").output().unwrap();
        assert!(proto_out.status.success());
        let proto_json: serde_json::Value = serde_json::from_slice(&proto_out.stdout).unwrap();
        let caps = proto_json["capabilities"].as_array().expect("capabilities array");
        assert!(caps.iter().any(|c| c == "discontiguous_part"), "Finding 44: --protocol missing discontiguous_part: {proto_json}");

        // Export discontiguous `--part 0,2-3` -> MUST forward `--part 0,2-3` and succeed!
        let mut c_exp = Command::new(HELPER)
            .args(["screen", "export", "qa", "--part", "0,2-3"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1")
            .env("OLO_SCREEN_EXPORT_TMPDIR", &export_disk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let _in_exp = c_exp.stdin.take().unwrap();
        let rx_exp = export_msg_channel(c_exp.stdout.take().unwrap());
        let mut saw_done = false;
        while let Ok(Some(msg)) = rx_exp.recv_timeout(Duration::from_secs(4)) {
            match msg {
                ExportMsg::Done => {
                    saw_done = true;
                    break;
                }
                ExportMsg::Error { message } => panic!("Finding 44: discontiguous --part failed against capable host olo-screen: {message}"),
                _ => {}
            }
        }
        assert!(saw_done, "Finding 44: expected ExportMsg::Done");
        assert!(wait_child_timeout(&mut c_exp, Duration::from_secs(3)).success());
        let passed = std::fs::read_to_string(&part_log).unwrap();
        assert!(passed.contains("--part 0,2-3"), "Finding 44: expected `--part 0,2-3` forwarded, got: {passed}");

        // (B) & (C): Findings 3 & 54: owner binding + `--expected-started` stop check.
        let mut stub = SessionStub::start("gen");
        let sock = rt.join("gen.sock");
        let _srv = FakeAppServer::start_in(&mut stub, sock.clone(), 128, 64, make_rgba(128, 64, 1));
        let rt_dir = rt.join("olo-screens");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::write(
            rt_dir.join("gen.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "gen",
                "wayland_display": "wayland-gen",
                "olo_socket": sock,
                "pid": stub.pid(),
                "width": 128,
                "height": 64,
                "scale": 1.0,
                "sound": false,
                "started": 2000
            }))
            .unwrap(),
        )
        .unwrap();

        // Trigger owner recording via an OWNER_ACTION (`click`) from REX_BLOCK=blk-old!
        let st_click = Command::new(HELPER)
            .args(["screen", "click", "gen", "10", "10"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("REX_SESSION", "ses-1")
            .env("REX_BLOCK", "blk-old")
            .status()
            .unwrap();
        assert!(st_click.success());
        let rt_owner_path = rt_dir.join("gen.owner.json");
        let owner_val: serde_json::Value = serde_json::from_slice(&std::fs::read(&rt_owner_path).unwrap()).unwrap();
        assert_eq!(owner_val["screen_pid"].as_i64(), Some(stub.pid() as i64), "Finding 54: owner.json missing screen_pid: {owner_val}");
        assert_eq!(owner_val["screen_started"].as_u64(), Some(2000), "Finding 54: owner.json missing screen_started: {owner_val}");

        // Now simulate a replacement session (`started: 3000`, new stub2) WITHOUT rewriting owner.json!
        let mut stub2 = SessionStub::start("gen");
        let sock2 = rt.join("gen2.sock");
        let _srv2 = FakeAppServer::start_in(&mut stub2, sock2.clone(), 128, 64, make_rgba(128, 64, 2));
        std::fs::write(
            rt_dir.join("gen.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "gen",
                "wayland_display": "wayland-gen",
                "olo_socket": sock2,
                "pid": stub2.pid(),
                "width": 128,
                "height": 64,
                "scale": 1.0,
                "sound": false,
                "started": 3000
            }))
            .unwrap(),
        )
        .unwrap();
        let _cleanup = Cleanup(dir.clone(), vec![stub, stub2]);

        // `follow` MUST discard stale owner (bound to started=2000) for running started=3000!
        let mut fol = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let fin = fol.stdin.take().unwrap();
        let mut freader = BufReader::new(fol.stdout.take().unwrap());
        let FollowEvent::Snapshot { screens, .. } = read_follow_event(&mut freader).unwrap().unwrap() else {
            panic!("expected Snapshot");
        };
        let gen_s = screens.iter().find(|s| s.name == "gen").expect("gen listed");
        assert_eq!(gen_s.owner, None, "Finding 54: follow reported stale owner for replacement screen session: {gen_s:?}");
        drop(fin);
        assert!(wait_child_timeout(&mut fol, Duration::from_secs(3)).success());

        // Finding 3 + Round 3: `screen stop gen --expected-started 2000` MUST fail with exit 75 and leave `gen` running!
        let mut cmd_bad = Command::new(HELPER);
        cmd_bad
            .args(["screen", "stop", "gen", "--expected-started", "2000"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (bad_code, _, bad_err) = run_cmd_bounded(cmd_bad, Duration::from_secs(5));
        assert_eq!(bad_code, Some(75), "Finding 3 / Round 3: mismatched --expected-started should exit 75: {bad_err}");
        assert!(!stop_log.exists(), "Finding 3: mismatched --expected-started must NOT invoke host olo-screen stop");

        // Matching `--expected-started 3000` stops the session directly in-process under `gen.lifecycle.lock` and unlinks `gen.json` + `gen.owner.json`!
        let mut cmd_ok = Command::new(HELPER);
        cmd_ok
            .args(["screen", "stop", "gen", "--expected-started", "3000"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (ok_code, _, ok_err) = run_cmd_bounded(cmd_ok, Duration::from_secs(5));
        assert_eq!(ok_code, Some(0), "Finding 3: matching --expected-started should succeed: {ok_err}");
        assert!(!rt_dir.join("gen.json").exists(), "Round 3: in-process guarded stop must remove gen.json");
        assert!(!rt_owner_path.exists(), "Finding 54: stop must unlink stale runtime gen.owner.json");
    }

    /// Round 3 Req 1: `stop`, `delete`, and `history NAME --clear` accept
    /// `--expected-started` and `--expected-last-used`, check under
    /// `NAME.lifecycle.lock`, exit 75 with plain words on mismatch, and act
    /// in-process under the same lock on match (no `bin/olo-screen` needed).
    #[test]
    fn round3_req1_guarded_stop_delete_clear_exit_75_and_in_process_act() {
        let dir = scratch_dir("r3-req1");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        // Finding 6: guarded acts require an installed `olo-screen` that takes `NAME.lifecycle.lock`.
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, "#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run_helper = |args: &[&str]| -> (Option<i32>, String) {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            let (code, _, stderr) = run_cmd_bounded(cmd, Duration::from_secs(5));
            (code, stderr)
        };

        // --- A: Guarded STOP (use unique screen name `r3_work` to avoid any live collision) ---
        let mut stub = SessionStub::start("r3_work");
        let sock = rt.join("r3_work.sock");
        let _srv = FakeAppServer::start_in(&mut stub, sock.clone(), 128, 64, make_rgba(128, 64, 1));
        let rt_dir = rt.join("olo-screens");
        std::fs::write(
            rt_dir.join("r3_work.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "r3_work",
                "wayland_display": "w-r3_work",
                "olo_socket": sock,
                "pid": stub.pid(),
                "width": 128,
                "height": 64,
                "scale": 1.0,
                "sound": false,
                "started": 5000
            }))
            .unwrap(),
        )
        .unwrap();
        let sdir = state.join("olo/screens/r3_work");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":128,"height":64,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(sdir.join("title"), "Work\n").unwrap();
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":9000,\"ms\":50,\"kind\":\"click\"}\n").unwrap();
        std::fs::write(sdir.join("frames-0000000000009000.olof"), b"OLOF").unwrap();

        // Wrong `--expected-started`: exit 75 + plain words, process stays alive.
        let (c1, e1) = run_helper(&["stop", "r3_work", "--expected-started", "4000"]);
        assert_eq!(c1, Some(75), "expected exit 75 on started mismatch, got {c1:?}, stderr={e1}");
        assert!(e1.contains("r3_work was started again since you asked"), "stderr={e1}");
        assert!(rt_dir.join("r3_work.json").exists(), "mismatched stop must not touch r3_work.json");

        // Wrong `--expected-last-used` (actual is 9050, expected 8000): exit 75 + plain words.
        let (c2, e2) = run_helper(&["stop", "r3_work", "--expected-started", "5000", "--expected-last-used", "8000"]);
        assert_eq!(c2, Some(75), "expected exit 75 on last_used mismatch, got {c2:?}, stderr={e2}");
        assert!(e2.contains("r3_work has newer activity since you asked"), "stderr={e2}");

        // Guarded `delete` and `history --clear` while running (expecting stopped) -> exit 75!
        let (c_del_run, e_del_run) = run_helper(&["delete", "r3_work", "--expected-last-used", "9050"]);
        assert_eq!(c_del_run, Some(75), "delete while running must exit 75: {e_del_run}");
        assert!(e_del_run.contains("r3_work was started again since you asked"));

        let (c_clr_run, e_clr_run) = run_helper(&["history", "r3_work", "--clear", "--expected-last-used", "9050"]);
        assert_eq!(c_clr_run, Some(75), "clear while running must exit 75: {e_clr_run}");
        assert!(e_clr_run.contains("r3_work was started again since you asked"));

        // Matching `--expected-started 5000 --expected-last-used 9050`: stops in-process and records `"stop"` in actions.jsonl (Finding 18)!
        let (c_ok_stop, e_ok_stop) = run_helper(&["stop", "r3_work", "--expected-started", "5000", "--expected-last-used", "9050"]);
        assert_eq!(c_ok_stop, Some(0), "matching guarded stop failed ({c_ok_stop:?}): {e_ok_stop}");
        assert!(!rt_dir.join("r3_work.json").exists(), "guarded stop must remove r3_work.json");
        let _ = wait_child_timeout(&mut stub.child, Duration::from_secs(3));

        // Read the recorded `"stop"` action's timestamp for the next Clear!
        let after_stop_lines = std::fs::read_to_string(sdir.join("actions.jsonl")).unwrap();
        let stop_row: serde_json::Value = serde_json::from_str(after_stop_lines.lines().last().unwrap()).unwrap();
        assert_eq!(stop_row["kind"], "stop", "Finding 18: guarded stop must record a stop row in actions.jsonl");
        let stop_lu = stop_row["at"].as_u64().unwrap() + stop_row["ms"].as_u64().unwrap();

        // --- B: Guarded HISTORY --CLEAR ---
        // `--expected-last-used` without `--clear` must fail with exit 2.
        let (c_bad_hist, _) = run_helper(&["history", "r3_work", "--expected-last-used", &stop_lu.to_string()]);
        assert_eq!(c_bad_hist, Some(2), "history without --clear must reject --expected-* with exit 2");

        // Stale `--expected-last-used 9050` (actual is `stop_lu > 9050`) -> exit 75, files untouched.
        let (c_clr_stale, e_clr_stale) = run_helper(&["history", "r3_work", "--clear", "--expected-started", "0", "--expected-last-used", "9050"]);
        assert_eq!(c_clr_stale, Some(75), "stale clear must exit 75: {e_clr_stale}");
        assert!(e_clr_stale.contains("r3_work has newer activity since you asked"));
        assert!(sdir.join("actions.jsonl").exists());
        assert!(sdir.join("frames-0000000000009000.olof").exists());

        // Matching `--expected-started 0 --expected-last-used <stop_lu>` -> clears actions + frames, keeps screen.json & title.
        let (c_clr_ok, e_clr_ok) = run_helper(&["history", "r3_work", "--clear", "--expected-started", "0", "--expected-last-used", &stop_lu.to_string()]);
        assert_eq!(c_clr_ok, Some(0), "matching guarded clear failed: {e_clr_ok}");
        assert!(!sdir.join("actions.jsonl").exists(), "clear must remove actions.jsonl");
        assert!(!sdir.join("frames-0000000000009000.olof").exists(), "clear must remove frames-*.olof");
        assert!(sdir.join("screen.json").exists(), "clear must keep screen.json");
        assert!(sdir.join("title").exists(), "clear must keep title");

        // --- C: Guarded DELETE ---
        // Write a new action (`at=12000, ms=20`) -> `--expected-last-used 0` must exit 75!
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":12000,\"ms\":20,\"kind\":\"type\"}\n").unwrap();
        let (c_del_stale, e_del_stale) = run_helper(&["delete", "r3_work", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_del_stale, Some(75), "stale delete must exit 75: {e_del_stale}");
        assert!(e_del_stale.contains("r3_work has newer activity since you asked"));
        assert!(sdir.exists(), "refused delete must not remove state dir");

        // Matching `--expected-started 0 --expected-last-used 12020` -> moves to XDG Trash!
        let (c_del_ok, e_del_ok) = run_helper(&["delete", "r3_work", "--expected-started", "0", "--expected-last-used", "12020"]);
        assert_eq!(c_del_ok, Some(0), "matching guarded delete failed: {e_del_ok}");
        assert!(!sdir.exists(), "guarded delete must remove screen state dir");
        assert!(data.join("Trash/files/r3_work").is_dir(), "guarded delete must move screen to XDG Trash/files");
        assert!(data.join("Trash/info/r3_work.trashinfo").is_file(), "guarded delete must write .trashinfo");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round 3 Req 2 (updated for Findings 6, 7 & 26): `screen --protocol`
    /// advertises `stop-expected`, `delete-expected`, `clear-expected`, and
    /// `control-lock` ONLY when the installed host `olo-screen` positively
    /// supports them (never for a legacy or comment-only stub).
    #[test]
    fn round3_req2_protocol_capabilities_delete_clear_and_control_lock() {
        let dir = scratch_dir("r3-req2");
        let home = dir.join("home");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();

        let probe = || -> Vec<String> {
            let mut cmd = Command::new(HELPER);
            cmd.args(["screen", "--protocol"]).env("HOME", &home).env("OLO_HOST_DESKTOP", &desktop).env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            let (code, stdout, stderr) = run_cmd_bounded(cmd, Duration::from_secs(4));
            assert_eq!(code, Some(0), "protocol failed: {stderr}");
            let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
            v["capabilities"].as_array().unwrap().iter().filter_map(|c| c.as_str().map(str::to_owned)).collect()
        };

        // 1. Legacy host `olo-screen` (no lifecycle lock or control lock):
        //    NONE of `stop-expected`, `delete-expected`, `clear-expected`, `control-lock` are advertised (Findings 6 & 26).
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let legacy_caps = probe();
        for cap in ["stop-expected", "delete-expected", "clear-expected", "control-lock"] {
            assert!(!legacy_caps.iter().any(|c| c == cap), "legacy olo-screen must NOT advertise {cap}: {legacy_caps:?}");
        }

        // 2. Modern host `olo-screen` declaring lifecycle lock + control lock + export part:
        //    ALL four capabilities ARE advertised.
        std::fs::write(&tool, "#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let modern_caps = probe();
        assert!(modern_caps.iter().any(|c| c == "control-lock"), "modern olo-screen MUST advertise control-lock: {modern_caps:?}");
        assert!(modern_caps.iter().any(|c| c == "stop-expected"));
        assert!(modern_caps.iter().any(|c| c == "stop-expected-started"));
        assert!(modern_caps.iter().any(|c| c == "delete-expected"));
        assert!(modern_caps.iter().any(|c| c == "clear-expected"));
        assert!(modern_caps.iter().any(|c| c == "discontiguous_part"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round 3 Req 3: `screen follow` includes `status.not_set_up_reason` and
    /// live `actions` count (zero polling, updated on append and clear), and
    /// older readers without those fields parse every emitted line cleanly.
    #[test]
    fn round3_req3_follow_not_set_up_reason_live_actions_and_older_reader() {
        use std::sync::mpsc;

        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct LegacyStatus {
            #[serde(default)]
            installed: Option<String>,
            #[serde(default)]
            committed: Option<u64>,
            #[serde(default)]
            default_on_host: bool,
        }
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct LegacyScreen {
            name: String,
            state: String,
            width: u32,
            height: u32,
            scale: f64,
            #[serde(default)]
            last_used: Option<u64>,
        }
        #[derive(serde::Deserialize, Debug)]
        #[serde(tag = "t", rename_all = "snake_case")]
        #[allow(dead_code)]
        enum LegacyEvent {
            Snapshot { proto: u32, status: LegacyStatus, screens: Vec<LegacyScreen> },
            Screen { screen: LegacyScreen },
            LastUsed { name: String, last_used: u64 },
            Gone { name: String },
            Status { status: LegacyStatus },
        }

        let dir = scratch_dir("r3-req3");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&state).unwrap();

        let mut child = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        // Older reader MUST parse every single emitted line without error!
                        let _: LegacyEvent = serde_json::from_str(trimmed).unwrap_or_else(|e| panic!("older reader failed on line {trimmed}: {e}"));
                        let v: serde_json::Value = serde_json::from_str(trimmed).unwrap();
                        if tx.send(v).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // 1. Uninstalled host -> exact initial `status.not_set_up_reason` (T3).
        let snap = rx.recv_timeout(Duration::from_secs(3)).expect("snapshot");
        assert_eq!(snap["t"], "snapshot");
        assert_eq!(
            snap["status"]["not_set_up_reason"].as_str(),
            Some("Olo isn't installed on this host yet (`olo screen install`)."),
            "T3: uninstalled host must carry exact not_set_up_reason: {snap}"
        );

        let wait_json = |pred: &dyn Fn(&serde_json::Value) -> bool, label: &str| -> serde_json::Value {
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until {
                let rem = until.saturating_duration_since(Instant::now());
                let v = rx.recv_timeout(rem).unwrap_or_else(|_| panic!("timed out waiting for {label}"));
                if pred(&v) {
                    return v;
                }
            }
            panic!("timed out waiting for {label}");
        };

        // 2. Install fake `olo-screen` + version -> `Status` event transitions to exact `screen status` readiness (T3).
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(desktop.join("share/olo")).unwrap();
        std::fs::write(desktop.join("bin/olo-screen"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(desktop.join("bin/olo-screen"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(desktop.join("share/olo/version"), "2026.11.27-test\n").unwrap();
        let st_ev = wait_json(&|v| v["t"] == "status" && v["status"]["installed"] == "2026.11.27-test", "installed status");
        // Verify the exact transition matches `olo-rex-link screen status` (`how` field when unready, None when ready).
        let mut st_cmd = Command::new(HELPER);
        st_cmd.args(["screen", "status"]).env("HOME", &home).env("OLO_HOST_DESKTOP", &desktop);
        let (st_c, st_out, _) = run_cmd_bounded(st_cmd, Duration::from_secs(3));
        assert_eq!(st_c, Some(0));
        let st_json: serde_json::Value = serde_json::from_str(&st_out).unwrap();
        let expected_reason = st_json.get("how").and_then(|v| v.as_str());
        assert_eq!(st_ev["status"].get("not_set_up_reason").and_then(|v| v.as_str()), expected_reason, "T3: post-install not_set_up_reason must match exact host readiness");

        // 3. Stopped screen with no actions -> `actions == 0`.
        let sdir = state.join("olo/screens/act");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let sc_ev = wait_json(&|v| v["t"] == "screen" && v["screen"]["name"] == "act", "new screen act");
        assert_eq!(sc_ev["screen"]["actions"].as_u64(), Some(0), "new screen must report actions=0: {sc_ev}");

        // 4. Append 3 actions -> `last_used` event reports `actions == 3` and `last_used == 3010`.
        let actions_path = sdir.join("actions.jsonl");
        std::fs::write(&actions_path, "{\"at\":1000,\"ms\":10,\"kind\":\"click\"}\n{\"at\":2000,\"ms\":10,\"kind\":\"type\"}\n{\"at\":3000,\"ms\":10,\"kind\":\"key\"}\n").unwrap();
        let lu3 =
            wait_json(&|v| (v["t"] == "last_used" && v["name"] == "act" && v["actions"] == 3) || (v["t"] == "screen" && v["screen"]["name"] == "act" && v["screen"]["actions"] == 3), "actions=3");
        if lu3["t"] == "last_used" {
            assert_eq!(lu3["last_used"].as_u64(), Some(3010));
        }

        // 5. Append 2 more actions at the SAME timestamp -> `actions` still advances to 5!
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&actions_path).unwrap();
            writeln!(f, "{{\"at\":3000,\"ms\":10,\"kind\":\"click\"}}").unwrap();
            writeln!(f, "{{\"at\":3000,\"ms\":10,\"kind\":\"click\"}}").unwrap();
        }
        wait_json(&|v| (v["t"] == "last_used" && v["name"] == "act" && v["actions"] == 5) || (v["t"] == "screen" && v["screen"]["name"] == "act" && v["screen"]["actions"] == 5), "actions=5");

        // 6. Remove `actions.jsonl` -> `Screen` event resets `actions == 0` and clears `last_used`.
        std::fs::remove_file(&actions_path).unwrap();
        let cleared = wait_json(&|v| v["t"] == "screen" && v["screen"]["name"] == "act" && v["screen"]["actions"] == 0, "actions reset to 0");
        assert!(cleared["screen"].get("last_used").is_none(), "cleared screen must omit last_used: {cleared}");

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(2)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round 3b Findings 1, 11, 12, 13, 18, 19, 25, 27:
    /// - F1/F11: Guarded Stop signals ONLY the pinned target session (never a same-name session in another runtime).
    /// - F12: Guarded Delete refuses any running screen with exit 75 regardless of `--expected-started`.
    /// - F13: Guarded Stop refuses when Take Control is held, and notifies live `olo-wlproxy` sockets (`send_home`).
    /// - F18: Guarded Stop records a `"stop"` entry in `actions.jsonl`.
    /// - F19: Guarded action respects `OLO_SCREEN_LOCK_TIMEOUT_MS` and returns busy instead of hanging forever.
    /// - F25: Legacy descriptor missing `"started"` never matches `--expected-started 0`; Clear checks `--expected-last-used` (exit 75) before running-screen exit 1.
    /// - F27: Guarded Delete writes local time (not UTC) in `.trashinfo`'s `DeletionDate=`.
    #[test]
    fn round3b_f1_f11_f12_f13_f18_f19_f25_f27_guarded_stop_and_delete() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::net::UnixListener;

        let dir = scratch_dir("r3b-stop-del");
        let home = dir.join("home");
        let rt_a = dir.join("run-a");
        let rt_b = dir.join("run-b");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt_a.join("olo-screens")).unwrap();
        std::fs::create_dir_all(rt_b.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        // Modern test stand-in declaring explicit lifecycle + control lock support.
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, "#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run_helper_bounded = |rt: &Path, args: &[&str], extra_env: &[(&str, &str)]| -> (Option<i32>, String) {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            for (k, v) in extra_env {
                cmd.env(k, v);
            }
            let (code, _, err) = run_cmd_bounded(cmd, Duration::from_secs(4));
            (code, err)
        };

        let sname = "r3b_same";
        let mut stub_a = SessionStub::start(sname);
        let mut stub_b = SessionStub::start(sname);
        let sock_a = rt_a.join("a.sock");
        let sock_b = rt_b.join("b.sock");
        let _srv_a = FakeAppServer::start_in(&mut stub_a, sock_a.clone(), 128, 64, make_rgba(128, 64, 1));
        let _srv_b = FakeAppServer::start_in(&mut stub_b, sock_b.clone(), 128, 64, make_rgba(128, 64, 2));
        let write_desc = |rt: &Path, sock: &Path, pid: i32, started: Option<u64>| {
            let mut v = serde_json::json!({
                "name": sname,
                "wayland_display": format!("wayland-{sname}"),
                "olo_socket": sock,
                "pid": pid,
                "width": 128,
                "height": 64,
                "scale": 1.0,
                "sound": false,
            });
            if let Some(st) = started {
                v["started"] = serde_json::json!(st);
            }
            std::fs::write(rt.join(format!("olo-screens/{sname}.json")), serde_json::to_vec(&v).unwrap()).unwrap();
        };
        write_desc(&rt_a, &sock_a, stub_a.pid(), Some(5000));
        write_desc(&rt_b, &sock_b, stub_b.pid(), Some(6000));

        let sdir = state.join(format!("olo/screens/{sname}"));
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":128,"height":64,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":9000,\"ms\":50,\"kind\":\"click\"}\n").unwrap();

        // F12: Guarded Delete with MATCHING positive `--expected-started 5000` MUST STILL refuse running screen with exit 75!
        let (code, err) = run_helper_bounded(&rt_a, &["delete", sname, "--expected-started", "5000"], &[]);
        assert_eq!(code, Some(75), "F12: guarded delete on running screen must exit 75 even with matching started: {err}");
        assert!(err.contains("was started again since you asked"), "F12 stderr={err}");
        assert!(rt_a.join(format!("olo-screens/{sname}.json")).exists(), "F12: running screen must not be stopped by delete");

        // F25b: Clear on running screen with matching started=5000 and STALE last_used=8000 must exit 75 (not 1)!
        let (code, err) = run_helper_bounded(&rt_a, &["history", sname, "--clear", "--expected-started", "5000", "--expected-last-used", "8000"], &[]);
        assert_eq!(code, Some(75), "F25: clear must check last_used (exit 75) before running-screen exit 1: {err}");
        assert!(err.contains("has newer activity since you asked"), "F25 stderr={err}");

        // F25a: Legacy descriptor missing `"started"` MUST NOT match `--expected-started 0`!
        write_desc(&rt_a, &sock_a, stub_a.pid(), None);
        let (code, err) = run_helper_bounded(&rt_a, &["stop", sname, "--expected-started", "0"], &[]);
        assert_eq!(code, Some(75), "F25: legacy missing started must not match --expected-started 0: {err}");
        assert!(rt_a.join(format!("olo-screens/{sname}.json")).exists());
        write_desc(&rt_a, &sock_a, stub_a.pid(), Some(5000));

        // F19: Holding `NAME.lifecycle.lock` with bounded lock timeout returns busy promptly (no hang).
        {
            let lf = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).open(rt_a.join(format!("olo-screens/{sname}.lifecycle.lock"))).unwrap();
            assert_eq!(unsafe { libc::flock(lf.as_raw_fd(), libc::LOCK_EX) }, 0);
            let t0 = Instant::now();
            let (code, err) = run_helper_bounded(&rt_a, &["stop", sname, "--expected-started", "5000"], &[("OLO_SCREEN_LOCK_TIMEOUT_MS", "150")]);
            assert_eq!(code, Some(1), "F19: contended lock should exit 1 (busy), got {code:?}: {err}");
            assert!(err.to_lowercase().contains("busy"), "F19: expected busy error, got: {err}");
            assert!(t0.elapsed() < Duration::from_secs(2), "F19: lock timeout took too long: {:?}", t0.elapsed());
        }

        // F13a: Guarded Stop refuses while a viewer holds Take Control (`NAME.control` + `NAME.control.lock`).
        {
            let mut viewer = Command::new(HELPER)
                .args(["screen", "stream", sname])
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt_a)
                .env("XDG_STATE_HOME", &state)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut vin = viewer.stdin.take().unwrap();
            let vrx = server_msg_channel(viewer.stdout.take().unwrap());
            let _ = vrx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
            write_client_msg(&mut vin, &ClientMsg::ControlSeq { seq: 1, on: true }).unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until {
                if let Ok(Some(ServerMsg::Control { seq: 1, held: true, .. })) = vrx.recv_timeout(Duration::from_millis(200)) {
                    break;
                }
            }
            let (code, err) = run_helper_bounded(&rt_a, &["stop", sname, "--expected-started", "5000"], &[]);
            assert_eq!(code, Some(1), "F13: stop while controlled must be refused: {err}");
            assert!(err.to_lowercase().contains("control"), "F13: expected control error, got: {err}");
            assert!(rt_a.join(format!("olo-screens/{sname}.json")).exists());
            drop(vin);
            let _ = wait_child_timeout(&mut viewer, Duration::from_secs(2));
        }

        // F13b: Fake `olo-wlproxy-<our_pid>.sock` receives `home ...` during guarded stop!
        let proxy_sock = rt_a.join(format!("olo-wlproxy-{}.sock", std::process::id()));
        let proxy_listener = UnixListener::bind(&proxy_sock).unwrap();
        let (proxy_tx, proxy_rx) = std::sync::mpsc::channel::<String>();
        let proxy_thread = std::thread::spawn(move || {
            if let Ok((mut s, _)) = proxy_listener.accept() {
                let mut line = String::new();
                let mut r = BufReader::new(&mut s);
                let _ = r.read_line(&mut line);
                let _ = s.write_all(b"ok\n");
                let _ = proxy_tx.send(line);
            }
        });

        // F1 + F11 + F13b + F18 + T1: Guarded Stop on `rt_a` stops ONLY `stub_a`, leaves `stub_b` in `rt_b` ALIVE
        // (verified via `try_wait().is_none()`, not `/proc/PID`!), sends `home` to proxy, and records `"kind":"stop"`!
        let (code, err) = run_helper_bounded(&rt_a, &["stop", sname, "--expected-started", "5000", "--expected-last-used", "9050"], &[]);
        assert_eq!(code, Some(0), "guarded stop failed: {err}");
        assert!(!rt_a.join(format!("olo-screens/{sname}.json")).exists(), "rt_a descriptor must be removed");
        assert!(stub_b.child.try_wait().unwrap().is_none(), "F1/T1: guarded stop on rt_a killed unrelated same-name session stub_b in rt_b!");
        assert!(rt_b.join(format!("olo-screens/{sname}.json")).exists(), "F1: rt_b descriptor must be untouched");
        let proxy_msg = proxy_rx.recv_timeout(Duration::from_secs(2)).expect("F13: guarded stop did not send home to olo-wlproxy");
        assert!(proxy_msg.starts_with("home "), "F13: expected `home ...`, got {proxy_msg:?}");
        let _ = proxy_thread.join();
        let _ = std::fs::remove_file(&proxy_sock);

        // End `stub_b` cleanly before deleting `state/r3b_same` so Delete acts on a fully stopped profile.
        let _ = stub_b.child.kill();
        let _ = stub_b.child.wait();
        let _ = std::fs::remove_file(rt_b.join(format!("olo-screens/{sname}.json")));

        // F18: Verify `"kind":"stop"` was appended to `actions.jsonl`!
        let actions_text = std::fs::read_to_string(sdir.join("actions.jsonl")).unwrap();
        let last_line = actions_text.lines().last().expect("actions.jsonl must have stop entry");
        let stop_ev: serde_json::Value = serde_json::from_str(last_line).unwrap();
        assert_eq!(stop_ev["kind"], "stop", "F18: guarded stop must record a stop action in actions.jsonl: {actions_text}");
        let stop_lu = stop_ev["at"].as_u64().unwrap() + stop_ev["ms"].as_u64().unwrap();
        assert!(stop_lu > 9050, "F18: stop action must advance last_used past 9050");

        // F27: Guarded Delete with `TZ=UTC-2` (UTC+2) writes local time (+2h ahead of UTC) in `.trashinfo`!
        let utc_hour = {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            (secs % 86_400) / 3_600
        };
        let (code, err) = run_helper_bounded(&rt_a, &["delete", sname, "--expected-started", "0", "--expected-last-used", &stop_lu.to_string()], &[("TZ", "UTC-2")]);
        assert_eq!(code, Some(0), "F27 guarded delete failed: {err}");
        let info_text = std::fs::read_to_string(data.join(format!("Trash/info/{sname}.trashinfo"))).unwrap();
        let date_str = info_text.lines().find_map(|l| l.strip_prefix("DeletionDate=")).expect("DeletionDate line");
        let hour: u64 = date_str.split('T').nth(1).unwrap().split(':').next().unwrap().parse().unwrap();
        let expected_local_hour = (utc_hour + 2) % 24;
        let expected_local_hour_next = (utc_hour + 3) % 24; // handle minute 59->00 boundary
        assert!(hour == expected_local_hour || hour == expected_local_hour_next, "F27: expected UTC+2 local hour ({expected_local_hour}), got DeletionDate={date_str:?} (utc_hour={utc_hour})");

        let _ = stub_a.child.kill();
        let _ = stub_a.child.wait();
        let _ = stub_b.child.kill();
        let _ = stub_b.child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round 3b Findings 5, 6, 7, 14, 15, 16, 17, 26:
    /// - F5: Corrupt descriptor or unreadable `actions.jsonl` refuses (exit 1), never treated as stopped/empty.
    /// - F6/F7/F26: `--protocol` and guarded execution gate strictly on installed readable `olo-screen` with positive markers (never missing/unreadable/comment-spoofed), and advertise `stop-expected`.
    /// - F14: Guarded Delete refuses an ordinary non-screen directory lacking `screen.json`/`actions.jsonl`.
    /// - F15: History > 17 MiB reads latest action at EOF (never truncated at 16 MiB prefix).
    /// - F16: Malformed `--expected-*` forms exit 2 and never fall through to unguarded `olo-screen`.
    /// - F17: Clear removes `frames-*.olof` BEFORE `actions.jsonl` so a frame unlink error keeps `actions.jsonl` retryable with the original `--expected-last-used`.
    #[test]
    fn round3b_f5_f6_f7_f14_f15_f16_f17_f26_safety_capabilities_large_log_and_partial_clear() {
        let dir = scratch_dir("r3b-safety");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();

        let run = |args: &[&str]| -> (Option<i32>, String, String) {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            run_cmd_bounded(cmd, Duration::from_secs(5))
        };
        let probe_caps = || -> Vec<String> {
            let (code, out, err) = run(&["--protocol"]);
            assert_eq!(code, Some(0), "protocol failed: {err}");
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            v["capabilities"].as_array().unwrap().iter().filter_map(|c| c.as_str().map(str::to_owned)).collect()
        };

        // F6/F26: Empty prefix (no `bin/olo-screen`) MUST NOT advertise control-lock or guarded capabilities!
        let empty_caps = probe_caps();
        for cap in ["control-lock", "stop-expected", "delete-expected", "clear-expected"] {
            assert!(!empty_caps.iter().any(|c| c == cap), "F6/F26: empty prefix must not advertise {cap}: {empty_caps:?}");
        }

        // F26: Unreadable `bin/olo-screen` (`chmod 000`) MUST NOT advertise control-lock or guarded capabilities!
        let tool = desktop.join("bin/olo-screen");
        let ran_log = dir.join("legacy-ran.log");
        std::fs::write(&tool, format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 0\n", ran_log.display())).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable_caps = probe_caps();
        for cap in ["control-lock", "stop-expected", "delete-expected", "clear-expected"] {
            assert!(!unreadable_caps.iter().any(|c| c == cap), "F26: unreadable bin must not advertise {cap}: {unreadable_caps:?}");
        }

        // F26: Comment-spoofed wrapper MUST NOT advertise control-lock or guarded capabilities!
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&tool, format!("#!/bin/sh\n# olo-rex-link .control.lock .lifecycle.lock\necho \"$*\" >> '{}'\nexit 0\n", ran_log.display())).unwrap();
        let spoof_caps = probe_caps();
        for cap in ["control-lock", "stop-expected", "delete-expected", "clear-expected"] {
            assert!(!spoof_caps.iter().any(|c| c == cap), "F26: comment-only stub must not advertise {cap}: {spoof_caps:?}");
        }

        // F6: Executing a guarded act against legacy `olo-screen` (no lifecycle lock) MUST refuse with upgrade words!
        let sdir = state.join("olo/screens/s_safe");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let (leg_c, _, leg_err) = run(&["delete", "s_safe", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(leg_c, Some(1), "F6: guarded delete against legacy olo-screen must refuse: {leg_err}");
        assert!(leg_err.contains("olo screen install"), "F6: expected upgrade guidance, got: {leg_err}");
        assert!(sdir.exists(), "F6: refused delete must leave screen intact");

        // F16: Malformed `--expected-*` flags MUST exit 2 and NEVER invoke host `olo-screen`!
        for bad in [
            &["delete", "s_safe", "--expected-started=0"][..],
            &["stop", "s_safe", "--expected-last-used=0"][..],
            &["delete", "s_safe", "--expected-bogus", "1"][..],
            &["history", "s_safe", "--expected-started", "--clear", "0"][..],
            &["history", "s_safe", "--clear", "--expected-last-used=0"][..],
        ] {
            let (c, _, err) = run(bad);
            assert_eq!(c, Some(2), "F16: malformed {bad:?} must exit 2, got {c:?}: {err}");
            assert!(!ran_log.exists(), "F16: malformed {bad:?} fell through to host olo-screen!");
        }

        // Install modern test stand-in -> F7 `stop-expected` + `delete-expected` + `clear-expected` + `control-lock` advertised!
        std::fs::write(&tool, "#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let modern_caps = probe_caps();
        for cap in ["control-lock", "stop-expected", "stop-expected-started", "delete-expected", "clear-expected", "discontiguous_part"] {
            assert!(modern_caps.iter().any(|c| c == cap), "F6/F7/F26: modern stub missing {cap}: {modern_caps:?}");
        }

        // F5a: Corrupt runtime descriptor MUST fail with exit 1 (never treated as stopped!).
        let bad_desc = rt.join("olo-screens/s_safe.json");
        std::fs::write(&bad_desc, b"{corrupt json").unwrap();
        let (c_bad, _, err_bad) = run(&["delete", "s_safe", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_bad, Some(1), "F5: corrupt descriptor must refuse (exit 1), got {c_bad:?}: {err_bad}");
        assert!(sdir.exists());
        std::fs::remove_file(&bad_desc).unwrap();

        // F5b + T3: Genuinely corrupt JSON `actions.jsonl` AND unreadable `actions.jsonl` (`chmod 000`)
        // MUST both fail with exit 1 (never treated as empty, even if run as root!).
        let act_path = sdir.join("actions.jsonl");
        std::fs::write(&act_path, "{not valid json at all\n").unwrap();
        let (c_corrupt_clr, _, _) = run(&["history", "s_safe", "--clear", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_corrupt_clr, Some(1), "F5/T3: genuinely corrupt JSON actions.jsonl must refuse clear (exit 1)");
        let (c_corrupt_del, _, _) = run(&["delete", "s_safe", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_corrupt_del, Some(1), "F5/T3: genuinely corrupt JSON actions.jsonl must refuse delete (exit 1)");
        assert!(sdir.exists());

        std::fs::write(&act_path, "{\"at\":5000,\"ms\":10,\"kind\":\"click\"}\n").unwrap();
        std::fs::set_permissions(&act_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (c_clr, _, _) = run(&["history", "s_safe", "--clear", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_clr, Some(1), "F5: unreadable actions.jsonl must refuse clear (exit 1), got {c_clr:?}");
        assert!(act_path.exists());
        let (c_del, _, _) = run(&["delete", "s_safe", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_del, Some(1), "F5: unreadable actions.jsonl must refuse delete (exit 1), got {c_del:?}");
        assert!(sdir.exists());
        std::fs::set_permissions(&act_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        // F14: Ordinary non-screen directory lacking `screen.json` and `actions.jsonl` MUST NOT be trashed!
        let plain_dir = state.join("olo/screens/not_a_screen");
        std::fs::create_dir_all(&plain_dir).unwrap();
        std::fs::write(plain_dir.join("notes.txt"), "keep me").unwrap();
        let (c_plain, _, _) = run(&["delete", "not_a_screen", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c_plain, Some(1), "F14: non-screen dir must be refused");
        assert!(plain_dir.exists());

        // F17: Frame unlink failure (`EISDIR` on `frames-0000000000000001.olof` subdir) MUST keep `actions.jsonl` intact
        // so retrying with the original `--expected-last-used 5010` succeeds once the blocker is cleared!
        let blocker = sdir.join("frames-0000000000000001.olof");
        std::fs::create_dir_all(&blocker).unwrap();
        let (c_part, _, _) = run(&["history", "s_safe", "--clear", "--expected-started", "0", "--expected-last-used", "5010"]);
        assert_eq!(c_part, Some(1), "F17: failing frame unlink must return error");
        assert!(act_path.exists(), "F17: actions.jsonl must NOT be unlinked before frames succeed!");
        std::fs::remove_dir(&blocker).unwrap();
        let (c_retry, _, err_retry) = run(&["history", "s_safe", "--clear", "--expected-started", "0", "--expected-last-used", "5010"]);
        assert_eq!(c_retry, Some(0), "F17: retry with original --expected-last-used must succeed, got {c_retry:?}: {err_retry}");
        assert!(!act_path.exists());

        // F15: History > 17 MiB (well past old 16 MiB cap!): guard MUST read final action at EOF!
        {
            let mut f = std::fs::File::create(&act_path).unwrap();
            let line = "{\"at\":1000,\"ms\":10,\"kind\":\"click\",\"args\":[\"padding-0123456789abcdef0123456789abcdef\"]}\n";
            let chunk = line.repeat(1024); // ~94 KiB
            for _ in 0..190 {
                f.write_all(chunk.as_bytes()).unwrap(); // ~17.9 MiB
            }
            f.write_all(b"{\"at\":999999,\"ms\":10,\"kind\":\"click\"}\n").unwrap();
        }
        let (c_old, _, _) = run(&["history", "s_safe", "--clear", "--expected-started", "0", "--expected-last-used", "1010"]);
        assert_eq!(c_old, Some(75), "F15: 17+ MiB log must NOT pass on early timestamp 1010!");
        assert!(act_path.exists());
        let (c_tail, _, err_tail) = run(&["history", "s_safe", "--clear", "--expected-started", "0", "--expected-last-used", "1000009"]);
        assert_eq!(c_tail, Some(0), "F15: 17+ MiB log must match real EOF timestamp 1000009: {err_tail}");
        assert!(!act_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round 3b Findings 20, 21, 23, 24:
    /// - F20: 2.5 MiB single action record with `"at"` after 2.2 MiB payload parses accurately.
    /// - F21: Same-inode in-place truncate + same-size rewrite is detected (never stale).
    /// - F23: FIFO in `prefix/bin` never hangs `screen follow`.
    /// - F24: Large (>17 MiB) pre-existing history emits Snapshot promptly and delivers accurate count past 16 MiB.
    #[test]
    fn round3b_f20_f21_f23_f24_follow_long_record_rewrite_fifo_and_large_history() {
        use std::sync::mpsc;

        let dir = scratch_dir("r3b-follow");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        std::fs::create_dir_all(desktop.join("share/olo")).unwrap();
        std::fs::write(desktop.join("share/olo/version"), "2026.11.27-test\n").unwrap();
        std::fs::write(desktop.join("bin/olo-screen"), "#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock\nexit 0\n").unwrap();
        std::fs::set_permissions(desktop.join("bin/olo-screen"), std::fs::Permissions::from_mode(0o755)).unwrap();

        // F23: Place a FIFO inside `prefix/bin`!
        let fifo_bin = desktop.join("bin/evil-fifo");
        let c_fifo = std::ffi::CString::new(fifo_bin.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);

        // F20: Screen with a 2.5 MiB single JSON record whose `"at"` sits AFTER 2.2 MiB of `"args"`.
        let long_dir = state.join("olo/screens/longrec");
        std::fs::create_dir_all(&long_dir).unwrap();
        std::fs::write(long_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        {
            let mut f = std::fs::File::create(long_dir.join("actions.jsonl")).unwrap();
            f.write_all(b"{\"args\":[\"").unwrap();
            let block = vec![b'A'; 64 * 1024];
            for _ in 0..36 {
                f.write_all(&block).unwrap(); // 2.25 MiB
            }
            f.write_all(b"\"],\"at\":77777,\"ms\":23,\"kind\":\"type\"}\n").unwrap();
        }

        // F21: Screen for same-inode same-size in-place rewrite test.
        let rw_dir = state.join("olo/screens/rewrite");
        std::fs::create_dir_all(&rw_dir).unwrap();
        std::fs::write(rw_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let two_lines = "{\"at\":1000,\"ms\":10,\"kind\":\"click\"}\n{\"at\":2000,\"ms\":10,\"kind\":\"click\"}\n";
        std::fs::write(rw_dir.join("actions.jsonl"), two_lines).unwrap();

        // F24: Screen with > 17 MiB history (200,000 + 1 lines) to verify fast Snapshot + full count past 16 MiB!
        let big_dir = state.join("olo/screens/bighist");
        std::fs::create_dir_all(&big_dir).unwrap();
        std::fs::write(big_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let big_expected_lines: u64 = 200_001;
        {
            let mut f = std::fs::File::create(big_dir.join("actions.jsonl")).unwrap();
            let line = "{\"at\":1000,\"ms\":10,\"kind\":\"click\",\"args\":[\"padding-0123456789abcdef0123456789abcdef\"]}\n";
            let chunk = line.repeat(1000);
            for _ in 0..200 {
                f.write_all(chunk.as_bytes()).unwrap(); // 200,000 lines (~18.4 MiB)
            }
            f.write_all(b"{\"at\":888888,\"ms\":12,\"kind\":\"click\"}\n").unwrap();
        }

        let t_launch = Instant::now();
        let mut child = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                            && tx.send(v).is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        // F23 + F24: Initial Snapshot MUST arrive promptly without hanging on FIFO!
        let snap = rx.recv_timeout(Duration::from_secs(3)).expect("F23: follow hung on FIFO in prefix/bin");
        assert_eq!(snap["t"], "snapshot");
        assert!(t_launch.elapsed() < Duration::from_secs(3), "F24: initial Snapshot took {:?}", t_launch.elapsed());
        let screens = snap["screens"].as_array().unwrap();

        let mut long_count = screens.iter().find(|s| s["name"] == "longrec").and_then(|s| s["actions"].as_u64());
        let mut long_lu = screens.iter().find(|s| s["name"] == "longrec").and_then(|s| s["last_used"].as_u64());
        let mut big_count = screens.iter().find(|s| s["name"] == "bighist").and_then(|s| s["actions"].as_u64());
        let big_lu = screens.iter().find(|s| s["name"] == "bighist").and_then(|s| s["last_used"].as_u64());
        assert_eq!(big_lu, Some(888_900), "F15/F24: bighist initial Snapshot must have accurate EOF last_used");
        assert_eq!(screens.iter().find(|s| s["name"] == "rewrite").and_then(|s| s["actions"].as_u64()), Some(2));

        // Drain deferred count completions for `longrec` and `bighist`.
        let until = Instant::now() + Duration::from_secs(5);
        while (long_count != Some(1) || big_count != Some(big_expected_lines)) && Instant::now() < until {
            let rem = until.saturating_duration_since(Instant::now());
            let Ok(ev) = rx.recv_timeout(rem) else { break };
            if ev["t"] == "last_used" {
                if ev["name"] == "longrec" {
                    long_count = ev["actions"].as_u64();
                    long_lu = ev["last_used"].as_u64();
                } else if ev["name"] == "bighist" {
                    big_count = ev["actions"].as_u64();
                }
            } else if ev["t"] == "screen" {
                if ev["screen"]["name"] == "longrec" {
                    long_count = ev["screen"]["actions"].as_u64();
                    long_lu = ev["screen"]["last_used"].as_u64();
                } else if ev["screen"]["name"] == "bighist" {
                    big_count = ev["screen"]["actions"].as_u64();
                }
            }
        }
        assert_eq!(long_count, Some(1), "F20: 2.5 MiB single record must be counted as 1");
        assert_eq!(long_lu, Some(77_800), "F20: 2.5 MiB single record must extract at+ms=77800");
        assert_eq!(big_count, Some(big_expected_lines), "F15/F24: 18+ MiB history must report full {big_expected_lines} actions (not 16 MiB prefix!)");

        // F21: Pure in-place overwrite on the EXACT same inode with the EXACT same byte length
        // (no O_TRUNC, so file length never drops to 0 and even same-tick mtime must be caught!).
        let prefix = "{\"at\":9000,\"ms\":5,\"kind\":\"click\",\"args\":[\"";
        let suffix = "\"]}\n";
        assert!(two_lines.len() >= prefix.len() + suffix.len());
        let pad = "x".repeat(two_lines.len() - prefix.len() - suffix.len());
        let one_line_same_len = format!("{prefix}{pad}{suffix}");
        assert_eq!(one_line_same_len.len(), two_lines.len());
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(rw_dir.join("actions.jsonl")).unwrap();
            f.write_all(one_line_same_len.as_bytes()).unwrap();
            f.flush().unwrap();
        }
        let until_rw = Instant::now() + Duration::from_secs(3);
        let mut saw_rewritten = false;
        while Instant::now() < until_rw {
            let rem = until_rw.saturating_duration_since(Instant::now());
            let Ok(ev) = rx.recv_timeout(rem) else { break };
            if ev["t"] == "screen" && ev["screen"]["name"] == "rewrite" && ev["screen"]["actions"] == 1 && ev["screen"]["last_used"] == 9005 {
                saw_rewritten = true;
                break;
            }
        }
        assert!(saw_rewritten, "F21: same-inode same-size in-place rewrite must update follow to actions=1, last_used=9005");

        drop(stdin);
        assert!(wait_child_timeout(&mut child, Duration::from_secs(2)).success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bounded helper invocation that drains stdout/stderr on background threads
    /// and enforces a hard timeout via `wait_child_timeout` (T1: never blocks in `Command::output`).
    fn run_cmd_bounded(mut cmd: Command, timeout: Duration) -> (Option<i32>, String, String) {
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let mut out_pipe = child.stdout.take().unwrap();
        let mut err_pipe = child.stderr.take().unwrap();
        let t_out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut out_pipe, &mut s);
            s
        });
        let t_err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut err_pipe, &mut s);
            s
        });
        let status = wait_child_timeout(&mut child, timeout);
        let stdout = t_out.join().unwrap_or_default();
        let stderr = t_err.join().unwrap_or_default();
        (status.code(), stdout, stderr)
    }

    fn write_modern_stub(desktop: &Path) {
        std::fs::create_dir_all(desktop.join("bin")).unwrap();
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, "#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Final Item 1 (5, 9, 11, 12): Symlinked runtime path (`/var/run` -> `/run`)
    /// must match via `(dev, ino)` so Delete never trashes a live screen; invalid
    /// PID (`<= 0`) or unexplained settings-dir mismatch must fail closed (exit 1).
    #[test]
    fn final_item1_symlink_runtime_and_invalid_pid_fail_closed() {
        use std::os::unix::fs::symlink;
        use std::os::unix::process::CommandExt as _;

        let dir = scratch_dir("final-item1");
        let home = dir.join("home");
        let rt_real = dir.join("run-real");
        let rt_link = dir.join("run-link");
        let rt_other = dir.join("run-other");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt_real.join("olo-screens")).unwrap();
        std::fs::create_dir_all(rt_other.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        symlink(&rt_real, &rt_link).unwrap();
        write_modern_stub(&desktop);

        let helper = |rt: &Path, args: &[&str]| {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            run_cmd_bounded(cmd, Duration::from_secs(4))
        };

        // (A) Live session launched with symlink spelling `rt_link/olo-screens/sym_s-settings.json`,
        // while helper inspects via real path `rt_real`: Delete --expected-started 0 MUST exit 75!
        let mut live = Command::new("/bin/sleep")
            .arg0("olo-session")
            .arg("60")
            .env("OLO_SCREEN", "sym_s")
            .env("OLO_APP_SERVER_SETTINGS", rt_link.join("olo-screens/sym_s-settings.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let sdir = state.join("olo/screens/sym_s");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(
            rt_real.join("olo-screens/sym_s.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "sym_s", "wayland_display": "w-sym", "olo_socket": rt_real.join("sym.sock"),
                "pid": live.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 5000
            }))
            .unwrap(),
        )
        .unwrap();

        let (code, _, err) = helper(&rt_real, &["delete", "sym_s", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(code, Some(75), "Item 1: symlink runtime path must recognize live session (exit 75), got {code:?}: {err}");
        assert!(sdir.exists(), "Item 1: live screen with symlink runtime path was trashed!");

        let _ = live.kill();
        let _ = live.wait();

        // (B) Invalid PID (0 and -5) in descriptor MUST fail closed with exit 1 (NEVER classify as stopped!).
        for bad_pid in [0i32, -5i32] {
            std::fs::write(
                rt_real.join("olo-screens/sym_s.json"),
                serde_json::to_vec(&serde_json::json!({
                    "name": "sym_s", "wayland_display": "w-sym", "olo_socket": rt_real.join("sym.sock"),
                    "pid": bad_pid, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 5000
                }))
                .unwrap(),
            )
            .unwrap();
            let (code, _, err) = helper(&rt_real, &["delete", "sym_s", "--expected-started", "0", "--expected-last-used", "0"]);
            assert_eq!(code, Some(1), "Item 1: invalid pid {bad_pid} must fail closed (exit 1), got {code:?}: {err}");
            assert!(sdir.exists(), "Item 1: invalid pid {bad_pid} allowed Delete to trash screen!");
        }

        // (C) Unexplained identity mismatch: live `olo-session` for `sym_s` whose settings dir is a
        // DIFFERENT directory (`rt_other`) MUST fail closed (exit 1), NEVER classify as stopped!
        let mut other_live = Command::new("/bin/sleep")
            .arg0("olo-session")
            .arg("60")
            .env("OLO_SCREEN", "sym_s")
            .env("OLO_APP_SERVER_SETTINGS", rt_other.join("olo-screens/sym_s-settings.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(
            rt_real.join("olo-screens/sym_s.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "sym_s", "wayland_display": "w-sym", "olo_socket": rt_real.join("sym.sock"),
                "pid": other_live.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 5000
            }))
            .unwrap(),
        )
        .unwrap();
        let (code, _, err) = helper(&rt_real, &["delete", "sym_s", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(code, Some(1), "Item 1: unexplained runtime identity mismatch must fail closed (exit 1), got {code:?}: {err}");
        assert!(sdir.exists(), "Item 1: identity mismatch allowed Delete to trash screen!");
        assert!(other_live.try_wait().unwrap().is_none(), "Item 1: other session must remain alive");

        let _ = other_live.kill();
        let _ = other_live.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Item 2: Guarded Stop of `qa` must NOT unlink `qa-settings.json` when
    /// it is another screen's descriptor (carrying `olo_socket`/`pid`), while still
    /// unlinking a genuine settings file.
    #[test]
    fn final_item2_stop_preserves_other_screen_settings_named_descriptor() {
        let dir = scratch_dir("final-item2");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        let rt_dir = rt.join("olo-screens");
        let mut qa_proc = spawn_session_stub("qa");
        let mut qa_set_proc = spawn_session_stub("qa-settings");
        std::fs::write(
            rt_dir.join("qa.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "qa", "wayland_display": "w-qa", "olo_socket": rt.join("qa.sock"),
                "pid": qa_proc.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 5000
            }))
            .unwrap(),
        )
        .unwrap();
        let other_desc = serde_json::to_vec(&serde_json::json!({
            "name": "qa-settings", "wayland_display": "w-qaset", "olo_socket": rt.join("qaset.sock"),
            "pid": qa_set_proc.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 6000
        }))
        .unwrap();
        std::fs::write(rt_dir.join("qa-settings.json"), &other_desc).unwrap();

        let mut cmd = Command::new(HELPER);
        cmd.args(["screen", "stop", "qa", "--expected-started", "5000"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (code, _, err) = run_cmd_bounded(cmd, Duration::from_secs(5));
        assert_eq!(code, Some(0), "guarded stop qa failed: {err}");
        assert!(!rt_dir.join("qa.json").exists(), "qa.json must be removed");
        assert!(rt_dir.join("qa-settings.json").exists(), "Item 2: qa-settings.json (another screen's descriptor!) was deleted by stop qa!");
        assert_eq!(std::fs::read(rt_dir.join("qa-settings.json")).unwrap(), other_desc);
        assert!(qa_set_proc.try_wait().unwrap().is_none(), "qa-settings session must still be running");

        // Real settings file (no descriptor fields) for `qa2` SHOULD still be unlinked.
        let mut qa2_proc = spawn_session_stub("qa2");
        std::fs::write(
            rt_dir.join("qa2.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "qa2", "wayland_display": "w-qa2", "olo_socket": rt.join("qa2.sock"),
                "pid": qa2_proc.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 5100
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(rt_dir.join("qa2-settings.json"), br#"{"appearance":{"look":"light"}}"#).unwrap();
        let mut cmd2 = Command::new(HELPER);
        cmd2.args(["screen", "stop", "qa2", "--expected-started", "5100"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (code2, _, err2) = run_cmd_bounded(cmd2, Duration::from_secs(5));
        assert_eq!(code2, Some(0), "guarded stop qa2 failed: {err2}");
        assert!(!rt_dir.join("qa2-settings.json").exists(), "Item 2: genuine qa2-settings.json should be cleaned up");

        let _ = qa_proc.kill();
        let _ = qa_proc.wait();
        let _ = qa_set_proc.kill();
        let _ = qa_set_proc.wait();
        let _ = qa2_proc.kill();
        let _ = qa2_proc.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Item 3 (13): Guarded Stop must speak the REAL compositor NDJSON
    /// `{"type":"list_windows"}\n` -> top-level `"windows"` array and REAL kit
    /// `olo-menus-PID.sock` `{"move":...}\n` protocol alongside `olo-wlproxy`.
    #[test]
    fn final_item3_send_home_real_compositor_and_kit_move_protocol() {
        use std::sync::mpsc;

        let dir = scratch_dir("final-item3");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        let mut sess = spawn_session_stub("kit_home");
        // Spawn a real helper child carrying `OLO_HOME` in its `/proc/<pid>/environ`!
        let home_env = serde_json::json!({
            "surface": null,
            "set": [["WAYLAND_DISPLAY", "wayland-person"], ["OLO_SOCKET", "/run/person.sock"]],
            "unset": ["OLO_SCREEN", "OLO_HOME"]
        });
        let mut kit_app = Command::new("/bin/sleep").arg("60").env("OLO_HOME", home_env.to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let kit_pid = kit_app.id();

        // 1. wlproxy socket
        let proxy_sock = rt.join(format!("olo-wlproxy-{}.sock", std::process::id()));
        let proxy_listener = UnixListener::bind(&proxy_sock).unwrap();
        let (proxy_tx, proxy_rx) = mpsc::channel::<String>();
        let t_proxy = std::thread::spawn(move || {
            if let Ok((mut s, _)) = proxy_listener.accept() {
                let mut line = String::new();
                let _ = BufReader::new(&mut s).read_line(&mut line);
                let _ = s.write_all(b"ok\n");
                let _ = proxy_tx.send(line);
            }
        });

        // 2. Real kit `olo-menus-<kit_pid>.sock` expecting `{"move": <MoveTo>}\n`
        let menus_sock = rt.join(format!("olo-menus-{kit_pid}.sock"));
        let menus_listener = UnixListener::bind(&menus_sock).unwrap();
        let (move_tx, move_rx) = mpsc::channel::<serde_json::Value>();
        let moved_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let moved_done_c = moved_done.clone();
        let t_menus = std::thread::spawn(move || {
            if let Ok((mut s, _)) = menus_listener.accept() {
                let mut line = String::new();
                let _ = BufReader::new(&mut s).read_line(&mut line);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    let _ = move_tx.send(v);
                }
                moved_done_c.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = s.write_all(b"{\"ok\":true}\n");
            }
        });

        // 3. Real compositor `olo_socket` speaking NDJSON `{"type":"list_windows"}\n` -> `{"type":"windows","windows":[...]}\n`
        let comp_sock = rt.join("kit_home.sock");
        let comp_listener = UnixListener::bind(&comp_sock).unwrap();
        let (comp_tx, comp_rx) = mpsc::channel::<serde_json::Value>();
        let moved_done_comp = moved_done.clone();
        let comp_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let comp_stop_c = comp_stop.clone();
        let t_comp = std::thread::spawn(move || {
            let _ = comp_listener.set_nonblocking(true);
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until && !comp_stop_c.load(std::sync::atomic::Ordering::SeqCst) {
                match comp_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut line = String::new();
                        if BufReader::new(&mut s).read_line(&mut line).is_ok()
                            && let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                        {
                            let _ = comp_tx.send(v);
                            let windows = if moved_done_comp.load(std::sync::atomic::Ordering::SeqCst) {
                                serde_json::json!([])
                            } else {
                                serde_json::json!([{"id": 1, "app_id": "tracker", "title": "Home", "pid": kit_pid, "workspace": 0, "minimized": false, "focused": true, "kept": true}])
                            };
                            let reply = serde_json::json!({"type": "windows", "windows": windows});
                            let _ = writeln!(s, "{reply}");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        });

        std::fs::write(
            rt.join("olo-screens/kit_home.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "kit_home", "wayland_display": "wayland-kithome", "olo_socket": comp_sock,
                "pid": sess.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 7000
            }))
            .unwrap(),
        )
        .unwrap();

        let mut cmd = Command::new(HELPER);
        cmd.args(["screen", "stop", "kit_home", "--expected-started", "7000"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (code, _, err) = run_cmd_bounded(cmd, Duration::from_secs(5));
        assert_eq!(code, Some(0), "guarded stop kit_home failed: {err}");

        let proxy_msg = proxy_rx.recv_timeout(Duration::from_secs(1)).expect("Item 3: wlproxy did not receive home");
        assert!(proxy_msg.starts_with("home "), "proxy_msg={proxy_msg:?}");

        let comp_req = comp_rx.recv_timeout(Duration::from_secs(1)).expect("Item 3: compositor did not receive NDJSON list_windows");
        assert_eq!(comp_req["type"], "list_windows", "Item 3: wrong compositor request: {comp_req}");

        let move_req = move_rx.recv_timeout(Duration::from_secs(1)).expect("Item 3: olo-menus-PID.sock did not receive move JSON");
        assert_eq!(move_req["move"], home_env, "Item 3: wrong kit move payload: {move_req}");

        comp_stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = t_comp.join();
        let _ = t_proxy.join();
        let _ = t_menus.join();
        let _ = sess.kill();
        let _ = sess.wait();
        let _ = kit_app.kill();
        let _ = kit_app.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Item 4 (Regression from 1 & 11): If the session supervisor is SIGSTOP'd
    /// while its child compositor runs, escalated SIGKILL Stop must NOT remove the
    /// descriptor while leaving the child compositor alive.
    #[test]
    fn final_item4_forced_stop_does_not_leave_live_child_behind() {
        use std::os::unix::process::CommandExt as _;

        let dir = scratch_dir("final-item4");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        // Spawn a parent `olo-session` (`sh` with arg0 `olo-session`) that spawns a long-lived child
        // (`sleep 60`), prints the child PID, and waits. Then SIGSTOP the parent!
        let child_pid_file = dir.join("child.pid");
        let script = format!("sleep 60 & echo $! > '{}'; wait", child_pid_file.display());
        let mut parent = Command::new("/bin/sh")
            .arg0("olo-session")
            .args(["-c", &script])
            .env("OLO_SCREEN", "susp")
            .env("OLO_APP_SERVER_SETTINGS", rt.join("olo-screens/susp-settings.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let child_pid: i32 = loop {
            if let Ok(s) = std::fs::read_to_string(&child_pid_file)
                && let Ok(p) = s.trim().parse::<i32>()
                && p > 0
            {
                break p;
            }
            assert!(Instant::now() < until, "child pid never appeared");
            std::thread::sleep(Duration::from_millis(10));
        };
        // Pin the child via pidfd BEFORE SIGSTOPing parent so we can check reliably (even if reparented to init).
        let child_pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, child_pid, 0 as libc::c_uint) as i32 };
        assert!(child_pidfd >= 0, "pidfd_open child failed");
        let child_exited = |fd: i32| -> bool {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            unsafe { libc::poll(&mut pfd, 1, 0) > 0 && pfd.revents != 0 }
        };
        assert!(!child_exited(child_pidfd));

        // SIGSTOP the supervisor so SIGTERM cannot run its shutdown handler!
        assert_eq!(unsafe { libc::kill(parent.id() as i32, libc::SIGSTOP) }, 0);
        // Wait until `/proc/<parent>/stat` shows stopped ('T').
        let t_stop = Instant::now() + Duration::from_secs(2);
        while Instant::now() < t_stop {
            if let Ok(st) = std::fs::read_to_string(format!("/proc/{}/stat", parent.id()))
                && st.rsplit_once(')').and_then(|(_, r)| r.trim_start().as_bytes().first().copied()) == Some(b'T')
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        std::fs::write(
            rt.join("olo-screens/susp.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "susp", "wayland_display": "w-susp", "olo_socket": rt.join("susp.sock"),
                "pid": parent.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 8000
            }))
            .unwrap(),
        )
        .unwrap();

        let mut cmd = Command::new(HELPER);
        cmd.args(["screen", "stop", "susp", "--expected-started", "8000"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1")
            .env("OLO_SCREEN_STOP_TERM_MS", "200");
        let (code, _, err) = run_cmd_bounded(cmd, Duration::from_secs(10));
        let desc_gone = !rt.join("olo-screens/susp.json").exists();
        let child_is_dead = child_exited(child_pidfd);

        // Clean up child & parent before asserting so nothing leaks on failure.
        if !child_is_dead {
            unsafe {
                libc::syscall(libc::SYS_pidfd_send_signal, child_pidfd, libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0 as libc::c_uint);
            }
        }
        unsafe { libc::close(child_pidfd) };
        let _ = parent.kill();
        let _ = parent.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!(desc_gone && !child_is_dead), "Item 4: forced Stop removed susp.json (code={code:?}, err={err:?}) while session child pid {child_pid} was STILL ALIVE!");
        assert_eq!(code, Some(0), "Item 4: forced Stop should cleanly terminate suspended supervisor + verified child via pidfd: {err}");
        assert!(child_is_dead, "Item 4: verified session child must have exited");
    }

    /// Final Item 5 (4, 13, 17): Fail closed on bad control.lock, and stage Clear so
    /// frame failures NEVER leave earlier frames deleted.
    #[test]
    fn final_item5_control_lock_fail_closed_and_clear_staging_rollback() {
        use std::os::unix::fs::symlink;

        let dir = scratch_dir("final-item5");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        let helper = |args: &[&str]| {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            run_cmd_bounded(cmd, Duration::from_secs(4))
        };

        // (A) Symlink at `ctl_bad.control.lock` MUST fail closed (exit 1), NEVER proceed!
        let sdir = state.join("olo/screens/ctl_bad");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let target = dir.join("other_lock");
        std::fs::write(&target, b"").unwrap();
        symlink(&target, rt.join("olo-screens/ctl_bad.control.lock")).unwrap();
        let (code, _, err) = helper(&["delete", "ctl_bad", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(code, Some(1), "Item 5: symlink control.lock must fail closed (exit 1), got {code:?}: {err}");
        assert!(sdir.exists(), "Item 5: symlink control.lock allowed Delete to proceed!");
        std::fs::remove_file(rt.join("olo-screens/ctl_bad.control.lock")).unwrap();

        // (B) Clear staging rollback: 3 valid frames + 1 directory `frames-0000000000009999.olof`
        // causing failure MUST restore ALL 3 valid frames AND `actions.jsonl` intact!
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":5000,\"ms\":10,\"kind\":\"click\"}\n").unwrap();
        for f in ["frames-0000000000000001.olof", "frames-0000000000000002.olof", "frames-0000000000000003.olof"] {
            std::fs::write(sdir.join(f), b"OLOF-FRAME").unwrap();
        }
        std::fs::create_dir_all(sdir.join("frames-0000000000009999.olof")).unwrap();
        let (code, _, err) = helper(&["history", "ctl_bad", "--clear", "--expected-started", "0", "--expected-last-used", "5010"]);
        assert_eq!(code, Some(1), "Item 5: failed clear must exit 1: {err}");
        assert!(sdir.join("actions.jsonl").exists(), "Item 5: actions.jsonl must survive rollback");
        for f in ["frames-0000000000000001.olof", "frames-0000000000000002.olof", "frames-0000000000000003.olof"] {
            assert_eq!(std::fs::read(sdir.join(f)).ok().as_deref(), Some(b"OLOF-FRAME".as_slice()), "Item 5: frame {f} was deleted instead of rolled back on failed Clear!");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Items 6 & 7 (13, 14, 18, 19): FIFO at `actions.jsonl` must NOT hang
    /// guarded Stop and must surface recording error separately; unterminated
    /// `actions.jsonl` must get a leading `\n` before `"stop"` so `}{` never happens.
    #[test]
    fn final_item6_item7_fifo_history_nonblocking_and_unterminated_jsonl_newline() {
        let dir = scratch_dir("final-item6-7");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        // (A) Item 6: Started-only Stop with a FIFO at `actions.jsonl` (no reader!) must NOT hang!
        let mut s_fifo = spawn_session_stub("fifo_stop");
        let fdir = state.join("olo/screens/fifo_stop");
        std::fs::create_dir_all(&fdir).unwrap();
        std::fs::write(fdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let fifo = fdir.join("actions.jsonl");
        let cfifo = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cfifo.as_ptr(), 0o600) }, 0);
        std::fs::write(
            rt.join("olo-screens/fifo_stop.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "fifo_stop", "wayland_display": "w-fs", "olo_socket": rt.join("fs.sock"),
                "pid": s_fifo.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 9100
            }))
            .unwrap(),
        )
        .unwrap();

        let t0 = Instant::now();
        let mut cmd = Command::new(HELPER);
        cmd.args(["screen", "stop", "fifo_stop", "--expected-started", "9100"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (code, _, err) = run_cmd_bounded(cmd, Duration::from_secs(3));
        assert!(t0.elapsed() < Duration::from_secs(3), "Item 6: Stop hung on FIFO actions.jsonl!");
        assert_eq!(code, Some(0), "Item 6: completed Stop should exit 0 even when history append fails: {err}");
        assert!(!rt.join("olo-screens/fifo_stop.json").exists(), "Item 6: descriptor must be removed");
        assert!(!err.trim().is_empty(), "Item 6: recording failure on FIFO must be surfaced on stderr!");

        // (B) Item 7: Unterminated final JSON line in `actions.jsonl` must NOT become `}{` after Stop!
        let mut s_nl = spawn_session_stub("no_nl");
        let ndir = state.join("olo/screens/no_nl");
        std::fs::create_dir_all(&ndir).unwrap();
        std::fs::write(ndir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(ndir.join("actions.jsonl"), r#"{"at":1000,"ms":5,"kind":"click"}"#).unwrap(); // NO trailing \n!
        std::fs::write(
            rt.join("olo-screens/no_nl.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "no_nl", "wayland_display": "w-nl", "olo_socket": rt.join("nl.sock"),
                "pid": s_nl.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 9200
            }))
            .unwrap(),
        )
        .unwrap();
        let mut cmd2 = Command::new(HELPER);
        cmd2.args(["screen", "stop", "no_nl", "--expected-started", "9200", "--expected-last-used", "1005"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (code2, _, err2) = run_cmd_bounded(cmd2, Duration::from_secs(3));
        assert_eq!(code2, Some(0), "Item 7: stop no_nl failed: {err2}");
        let text = std::fs::read_to_string(ndir.join("actions.jsonl")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "Item 7: unterminated actions.jsonl corrupted into `}}{{`! content={text:?}");
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("line 0 must remain valid JSON");
        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("line 1 must be valid stop JSON");
        assert_eq!(first["kind"], "click");
        assert_eq!(second["kind"], "stop");

        let _ = s_fifo.kill();
        let _ = s_fifo.wait();
        let _ = s_nl.kill();
        let _ = s_nl.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Item 8 (9, 14): Delete AND Clear must require a regular-file screen
    /// marker (rejecting directories and dangling symlinks named `screen.json`).
    #[test]
    fn final_item8_regular_file_marker_for_delete_and_clear() {
        use std::os::unix::fs::symlink;

        let dir = scratch_dir("final-item8");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        let helper = |args: &[&str]| {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            run_cmd_bounded(cmd, Duration::from_secs(4))
        };

        // (A) Directory named `screen.json`: rejected by BOTH Delete and Clear!
        let dir_marker = state.join("olo/screens/dir_m");
        std::fs::create_dir_all(dir_marker.join("screen.json")).unwrap();
        std::fs::write(dir_marker.join("frames-0000000000000001.olof"), b"KEEP").unwrap();
        let (c1, _, e1) = helper(&["delete", "dir_m", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c1, Some(1), "Item 8: directory screen.json must be rejected by Delete: {e1}");
        assert!(dir_marker.exists());
        let (c2, _, e2) = helper(&["history", "dir_m", "--clear", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c2, Some(1), "Item 8: directory screen.json must be rejected by Clear: {e2}");
        assert!(dir_marker.join("frames-0000000000000001.olof").exists(), "Item 8: Clear touched non-screen dir!");

        // (B) Dangling symlink named `screen.json`: rejected by BOTH Delete and Clear!
        let sym_marker = state.join("olo/screens/sym_m");
        std::fs::create_dir_all(&sym_marker).unwrap();
        symlink("/nonexistent/nowhere", sym_marker.join("screen.json")).unwrap();
        let (c3, _, e3) = helper(&["delete", "sym_m", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c3, Some(1), "Item 8: dangling symlink screen.json must be rejected by Delete: {e3}");
        assert!(sym_marker.exists());
        let (c4, _, e4) = helper(&["history", "sym_m", "--clear", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c4, Some(1), "Item 8: dangling symlink screen.json must be rejected by Clear: {e4}");

        // (C) Ordinary unmarked directory: rejected by Clear too!
        let plain = state.join("olo/screens/plain_m");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("frames-0000000000000001.olof"), b"KEEP").unwrap();
        let (c5, _, e5) = helper(&["history", "plain_m", "--clear", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c5, Some(1), "Item 8: unmarked dir must be rejected by Clear: {e5}");
        assert!(plain.join("frames-0000000000000001.olof").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Item 9 (16, 25): Strict CLI parsing (`--expectd-started` exit 2,
    /// `delete qa --help` local help, `stop qa --quick` preserved, `history qa
    /// --search --expected-started` preserved) and stale-before-error exit code order.
    #[test]
    fn final_item9_strict_cli_parsing_help_and_stale_before_error() {
        let dir = scratch_dir("final-item9");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(desktop.join("bin")).unwrap();

        let forwarded_log = dir.join("forwarded.log");
        let tool = desktop.join("bin/olo-screen");
        std::fs::write(&tool, format!("#!/bin/sh\nOLO_TEST_SCREEN_CAPS=control-lock,lifecycle-lock,discontiguous_part\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n", forwarded_log.display())).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let helper = |args: &[&str]| {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen")
                .args(args)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &rt)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_DATA_HOME", &data)
                .env("OLO_HOST_DESKTOP", &desktop)
                .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            run_cmd_bounded(cmd, Duration::from_secs(4))
        };

        let sdir = state.join("olo/screens/qa");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();

        // 1. Typo `history qa --clear --expectd-started 10` MUST exit 2 and NOT invoke `olo-screen`!
        let (c, _, err) = helper(&["history", "qa", "--clear", "--expectd-started", "10"]);
        assert_eq!(c, Some(2), "Item 9: typo --expectd-started must exit 2, got {c:?}: {err}");
        assert!(!forwarded_log.exists(), "Item 9: typo --expectd-started was forwarded to olo-screen!");

        // 2. `delete qa --help` MUST exit 0 as local help and NOT invoke `olo-screen` or delete `qa`!
        let (c, out, err) = helper(&["delete", "qa", "--help"]);
        assert_eq!(c, Some(0), "Item 9: delete qa --help must exit 0: {err}");
        assert!(out.contains("usage:") || out.contains("delete"), "Item 9: expected local help text, got {out:?}");
        assert!(!forwarded_log.exists(), "Item 9: delete qa --help was forwarded to olo-screen (which would delete qa!)");
        assert!(sdir.exists());

        // 3. Unguarded `stop qa --quick` MUST be preserved and forwarded to `olo-screen`!
        let (c, _, err) = helper(&["stop", "qa", "--quick"]);
        assert_eq!(c, Some(0), "Item 9: unguarded `stop qa --quick` must succeed, got {c:?}: {err}");
        let log1 = std::fs::read_to_string(&forwarded_log).unwrap_or_default();
        assert!(log1.contains("stop qa --quick"), "Item 9: expected `stop qa --quick` forwarded, got {log1:?}");
        let _ = std::fs::remove_file(&forwarded_log);

        // 4. Literal `--expected-started` as a `--search` value in non-clear `history` MUST be forwarded!
        let (c, _, err) = helper(&["history", "qa", "--search", "--expected-started"]);
        assert_eq!(c, Some(0), "Item 9: `history qa --search --expected-started` must be forwarded, got {c:?}: {err}");
        let log2 = std::fs::read_to_string(&forwarded_log).unwrap_or_default();
        assert!(log2.contains("history qa --search --expected-started"), "Item 9: got {log2:?}");

        // 5. Stale-before-error (F25): live `qa` + unreadable `actions.jsonl` (`chmod 000`) +
        //    `delete qa --expected-started 0 --expected-last-used 0` MUST exit 75 (liveness already proves stale!).
        let mut live = spawn_session_stub("qa");
        std::fs::write(
            rt.join("olo-screens/qa.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "qa", "wayland_display": "w-qa", "olo_socket": rt.join("qa.sock"),
                "pid": live.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 5000
            }))
            .unwrap(),
        )
        .unwrap();
        let act = sdir.join("actions.jsonl");
        std::fs::write(&act, "{\"at\":1000,\"ms\":5,\"kind\":\"click\"}\n").unwrap();
        std::fs::set_permissions(&act, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (c, _, err) = helper(&["delete", "qa", "--expected-started", "0", "--expected-last-used", "0"]);
        std::fs::set_permissions(&act, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = live.kill();
        let _ = live.wait();
        let _ = std::fs::remove_file(rt.join("olo-screens/qa.json"));
        assert_eq!(c, Some(75), "Item 9: live screen vs expected_started=0 must exit 75 even when actions.jsonl is unreadable, got {c:?}: {err}");

        // 6. Vanished confirmed target: stopped screen deleted before confirmation finishes -> exit 75!
        let (c, _, err) = helper(&["delete", "vanished_screen", "--expected-started", "0", "--expected-last-used", "0"]);
        assert_eq!(c, Some(75), "Item 9: vanished confirmed target must exit 75, got {c:?}: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Final Item 10 (5, 6, 8, 20, 21, 23, 24, 26): Schema-correct JSON decoder
    /// (`\u0061t`, invalid JSON, torn final row), `follow` open error -> `actions: None`,
    /// backward timestamp on deferred path -> `Screen`, FIFO at `share/olo/version`,
    /// and production capability gating (no `OLO_ALLOW_TEST_SCREEN_CAPS`, executable bit,
    /// `screen stream` fail-closed when `olo-screen` absent).
    #[test]
    fn final_item10_json_decoder_follow_errors_backward_ts_fifo_version_and_cap_gating() {
        use std::sync::mpsc;

        let dir = scratch_dir("final-item10");
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        write_modern_stub(&desktop);

        let helper = |args: &[&str], allow_test_caps: bool| {
            let mut cmd = Command::new(HELPER);
            cmd.arg("screen").args(args).env("HOME", &home).env("XDG_RUNTIME_DIR", &rt).env("XDG_STATE_HOME", &state).env("XDG_DATA_HOME", &data).env("OLO_HOST_DESKTOP", &desktop);
            if allow_test_caps {
                cmd.env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
            } else {
                cmd.env_remove("OLO_ALLOW_TEST_SCREEN_CAPS");
            }
            run_cmd_bounded(cmd, Duration::from_secs(4))
        };

        // (A) Escaped key `\u0061t`, invalid JSON `{"at":100,"ms":0,"kind":}`, and torn final row!
        let sdir = state.join("olo/screens/jdec");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();

        // A1: Escaped key `\u0061t`: second row is at=200 -> Clear expecting 105 MUST exit 75!
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":100,\"ms\":5,\"kind\":\"click\"}\n{\"\\u0061t\":200,\"ms\":5,\"kind\":\"click\"}\n").unwrap();
        let (c, _, err) = helper(&["history", "jdec", "--clear", "--expected-started", "0", "--expected-last-used", "105"], true);
        assert_eq!(c, Some(75), "Item 10: escaped key \\u0061t=200 must defeat expected_last_used=105 (exit 75), got {c:?}: {err}");
        assert!(sdir.join("actions.jsonl").exists());

        // A2: Invalid JSON `{"at":100,"ms":0,"kind":}` MUST fail closed (exit 1)!
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":100,\"ms\":0,\"kind\":}\n").unwrap();
        let (c, _, err) = helper(&["history", "jdec", "--clear", "--expected-started", "0", "--expected-last-used", "100"], true);
        assert_eq!(c, Some(1), "Item 10: invalid JSON must fail closed (exit 1), got {c:?}: {err}");
        assert!(sdir.join("actions.jsonl").exists());

        // A3: Torn final row after valid row MUST fail closed (exit 1), NEVER accept row 1!
        std::fs::write(sdir.join("actions.jsonl"), "{\"at\":100,\"ms\":0,\"kind\":\"click\"}\n{\"at\":200,\"ms\":").unwrap();
        let (c, _, err) = helper(&["history", "jdec", "--clear", "--expected-started", "0", "--expected-last-used", "100"], true);
        assert_eq!(c, Some(1), "Item 10: torn final row must fail closed (exit 1), got {c:?}: {err}");
        assert!(sdir.join("actions.jsonl").exists());

        // (B) Capability gating (6, 26) WITHOUT `OLO_ALLOW_TEST_SCREEN_CAPS=1`:
        // B1: Non-ELF script with `OLO_TEST_SCREEN_CAPS=...` MUST NOT advertise caps in production!
        let (c, out, err) = helper(&["--protocol"], false);
        assert_eq!(c, Some(0), "protocol failed: {err}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let prod_caps: Vec<String> = v["capabilities"].as_array().unwrap().iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        for cap in ["control-lock", "stop-expected", "delete-expected", "clear-expected"] {
            assert!(!prod_caps.iter().any(|x| x == cap), "Item 10: production mode honored OLO_TEST_SCREEN_CAPS script for {cap}: {prod_caps:?}");
        }

        // B2: Non-executable (`0644`) `bin/olo-screen` MUST NOT advertise caps even with `OLO_ALLOW_TEST_SCREEN_CAPS=1`!
        let tool = desktop.join("bin/olo-screen");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (c, out, _) = helper(&["--protocol"], true);
        assert_eq!(c, Some(0));
        let v2: serde_json::Value = serde_json::from_str(&out).unwrap();
        let noexec_caps: Vec<String> = v2["capabilities"].as_array().unwrap().iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        for cap in ["control-lock", "stop-expected", "delete-expected", "clear-expected"] {
            assert!(!noexec_caps.iter().any(|x| x == cap), "Item 10: non-executable 0644 bin/olo-screen advertised {cap}: {noexec_caps:?}");
        }
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        // B3: `screen stream` Take Control with absent `bin/olo-screen` MUST fail closed in production (no OLO_ALLOW_TEST_SCREEN_CAPS)!
        let empty_desktop = dir.join("empty-desktop");
        std::fs::create_dir_all(&empty_desktop).unwrap();
        let mut s_ctl = SessionStub::start("prod_ctl");
        let ctl_sock = rt.join("prod_ctl.sock");
        let _srv = FakeAppServer::start_in(&mut s_ctl, ctl_sock.clone(), 128, 64, make_rgba(128, 64, 1));
        write_screen_json(&rt, "prod_ctl", s_ctl.pid(), &ctl_sock, 128, 64);
        let mut viewer = Command::new(HELPER)
            .args(["screen", "stream", "prod_ctl"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HOST_DESKTOP", &empty_desktop)
            .env_remove("OLO_ALLOW_TEST_SCREEN_CAPS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut vin = viewer.stdin.take().unwrap();
        let vrx = server_msg_channel(viewer.stdout.take().unwrap());
        let _ = vrx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        write_client_msg(&mut vin, &ClientMsg::ControlSeq { seq: 1, on: true }).unwrap();
        let mut refused = false;
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            match vrx.recv_timeout(Duration::from_millis(300)) {
                Ok(Some(ServerMsg::Control { seq: 1, held: false, .. })) => {
                    refused = true;
                    break;
                }
                Ok(Some(ServerMsg::Control { seq: 1, held: true, .. })) => break,
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        drop(vin);
        let _ = wait_child_timeout(&mut viewer, Duration::from_secs(2));
        s_ctl.kill_and_wait();
        let _ = std::fs::remove_file(rt.join("olo-screens/prod_ctl.json"));
        assert!(refused, "Item 10: screen stream granted Take Control in production with absent olo-screen!");

        // (C) FIFO at `share/olo/version` MUST NOT block `screen status`!
        let fifo_desktop = dir.join("fifo-desktop");
        std::fs::create_dir_all(fifo_desktop.join("bin")).unwrap();
        std::fs::create_dir_all(fifo_desktop.join("share/olo")).unwrap();
        std::fs::write(fifo_desktop.join("bin/olo-screen"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(fifo_desktop.join("bin/olo-screen"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let vfifo = fifo_desktop.join("share/olo/version");
        let cvfifo = std::ffi::CString::new(vfifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cvfifo.as_ptr(), 0o600) }, 0);
        let t_st = Instant::now();
        let mut st_cmd = Command::new(HELPER);
        st_cmd.args(["screen", "status"]).env("HOME", &home).env("XDG_RUNTIME_DIR", &rt).env("XDG_STATE_HOME", &state).env("OLO_HOST_DESKTOP", &fifo_desktop);
        let (st_code, _, _) = run_cmd_bounded(st_cmd, Duration::from_secs(3));
        assert_eq!(st_code, Some(0), "Item 10: screen status hung/failed on FIFO share/olo/version");
        assert!(t_st.elapsed() < Duration::from_secs(3));

        // (D) `screen follow`: unreadable `actions.jsonl` publishes `actions: None` (never `0`),
        //     and backward timestamp on deferred (> 2 MiB) append emits `Screen` (not `LastUsed`)!
        let udir = state.join("olo/screens/unr");
        std::fs::create_dir_all(&udir).unwrap();
        std::fs::write(udir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        // Use a FIFO at `actions.jsonl` (unreadable as regular file even under root!).
        let u_act = udir.join("actions.jsonl");
        let cu_act = std::ffi::CString::new(u_act.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cu_act.as_ptr(), 0o600) }, 0);

        let bdir = state.join("olo/screens/backts");
        std::fs::create_dir_all(&bdir).unwrap();
        std::fs::write(bdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(bdir.join("actions.jsonl"), "{\"at\":9000,\"ms\":0,\"kind\":\"click\"}\n").unwrap();

        let mut fol = Command::new(HELPER)
            .args(["screen", "follow"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("OLO_HOST_DESKTOP", &desktop)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let fin = fol.stdin.take().unwrap();
        let fout = fol.stdout.take().unwrap();
        let (ftx, frx) = mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            let mut r = BufReader::new(fout);
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 0 {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                    && ftx.send(v).is_err()
                {
                    break;
                }
                line.clear();
            }
        });

        let snap = frx.recv_timeout(Duration::from_secs(3)).expect("snapshot");
        let unr_s = snap["screens"].as_array().unwrap().iter().find(|s| s["name"] == "unr").expect("unr listed");
        assert!(unr_s.get("actions").is_none(), "Item 10: unreadable actions.jsonl must publish actions: None (omitted), got {unr_s}");

        // Track displayed state with Rex's exact rules (screens_sock.rs:2542):
        // `LastUsed` only advances `last_used`, never lowers it; only `Screen` can lower `last_used`
        // or reset `actions` to `None` while rescanning.
        let back_init = snap["screens"].as_array().unwrap().iter().find(|s| s["name"] == "backts").expect("backts listed");
        let mut displayed_lu = back_init["last_used"].as_u64();
        let mut displayed_actions = back_init["actions"].as_u64();
        assert_eq!(displayed_lu, Some(9000));
        assert_eq!(displayed_actions, Some(1));

        // Append > 2.5 MiB (more than one scan slice!) ending at backward timestamp `at=4000`!
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(bdir.join("actions.jsonl")).unwrap();
            let pad = "p".repeat(64 * 1024);
            for _ in 0..42 {
                writeln!(f, "{{\"at\":4000,\"ms\":0,\"kind\":\"click\",\"args\":[\"{pad}\"]}}").unwrap();
            }
        }
        let until_b = Instant::now() + Duration::from_secs(4);
        let mut saw_lowering_screen = false;
        let mut saw_bad_backward_last_used = false;
        let mut saw_stale_old_count_on_4000 = false;
        while Instant::now() < until_b {
            let rem = until_b.saturating_duration_since(Instant::now());
            let Ok(ev) = frx.recv_timeout(rem) else { break };
            if ev["t"] == "last_used" && ev["name"] == "backts" {
                let ts = ev["last_used"].as_u64().unwrap();
                let acts = ev.get("actions").and_then(|v| v.as_u64());
                if displayed_lu.is_some_and(|prev| ts < prev) {
                    saw_bad_backward_last_used = true;
                }
                if ts == 4000 && acts == Some(1) {
                    saw_stale_old_count_on_4000 = true;
                }
                if displayed_lu.is_none_or(|prev| ts > prev) {
                    displayed_lu = Some(ts);
                }
                if let Some(a) = acts {
                    displayed_actions = Some(a);
                }
            } else if ev["t"] == "screen" && ev["screen"]["name"] == "backts" {
                let ts = ev["screen"]["last_used"].as_u64();
                let acts = ev["screen"].get("actions").and_then(|v| v.as_u64());
                if ts == Some(4000) && acts == Some(1) {
                    saw_stale_old_count_on_4000 = true;
                }
                if ts == Some(4000) {
                    saw_lowering_screen = true;
                }
                displayed_lu = ts;
                displayed_actions = acts;
            }
            if displayed_lu == Some(4000) && displayed_actions == Some(43) {
                break;
            }
        }
        drop(fin);
        let _ = wait_child_timeout(&mut fol, Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!saw_bad_backward_last_used, "Item 10: backward timestamp emitted LastUsed while displayed_lu was still higher!");
        assert!(!saw_stale_old_count_on_4000, "Item 10: emitted stale old actions=1 paired with backward timestamp 4000!");
        assert!(saw_lowering_screen, "Item 10: expected full Screen event to lower last_used 9000 -> 4000");
        assert_eq!((displayed_lu, displayed_actions), (Some(4000), Some(43)), "Item 10: Rex state did not converge to (4000, 43)!");
    }

    /// Round 1 agy-review Finding 1 (integration): on a standard Linux UPG
    /// (`umask 0002`) host where `$XDG_RUNTIME_DIR/olo-screens`,
    /// `$XDG_STATE_HOME/olo/screens`, and `$XDG_DATA_HOME/Trash` are `0775`
    /// user-owned directories, BOTH `screen stream` AND guarded `screen delete`
    /// Mirror of `screens::is_private_user_group` so integration tests verify
    /// UPG vs non-UPG behavior accurately on every host/CI account.
    fn is_private_user_group(uid: libc::uid_t, gid: libc::gid_t) -> bool {
        use std::ffi::CStr;

        let mut pw_buf: Vec<libc::c_char> = vec![0; 1024];
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut pw_res: *mut libc::passwd = std::ptr::null_mut();
        loop {
            let rc = unsafe { libc::getpwuid_r(uid, &mut pw, pw_buf.as_mut_ptr(), pw_buf.len(), &mut pw_res) };
            if rc == libc::ERANGE && pw_buf.len() < 64 * 1024 {
                pw_buf.resize(pw_buf.len() * 2, 0);
                continue;
            }
            if rc != 0 || pw_res.is_null() {
                return false;
            }
            break;
        }
        if pw.pw_gid != gid || pw.pw_name.is_null() {
            return false;
        }
        let pw_name = unsafe { CStr::from_ptr(pw.pw_name) }.to_bytes().to_vec();

        let mut gr_buf: Vec<libc::c_char> = vec![0; 1024];
        let mut gr: libc::group = unsafe { std::mem::zeroed() };
        let mut gr_res: *mut libc::group = std::ptr::null_mut();
        loop {
            let rc = unsafe { libc::getgrgid_r(gid, &mut gr, gr_buf.as_mut_ptr(), gr_buf.len(), &mut gr_res) };
            if rc == libc::ERANGE && gr_buf.len() < 64 * 1024 {
                gr_buf.resize(gr_buf.len() * 2, 0);
                continue;
            }
            if rc != 0 || gr_res.is_null() {
                return false;
            }
            break;
        }
        if gr.gr_name.is_null() {
            return false;
        }
        let gr_name = unsafe { CStr::from_ptr(gr.gr_name) }.to_bytes();
        if gr_name != pw_name.as_slice() {
            return false;
        }
        if !gr.gr_mem.is_null() {
            let mut p = gr.gr_mem;
            loop {
                let mem = unsafe { *p };
                if mem.is_null() {
                    break;
                }
                let mem_bytes = unsafe { CStr::from_ptr(mem) }.to_bytes();
                if mem_bytes != pw_name.as_slice() {
                    return false;
                }
                p = unsafe { p.add(1) };
            }
        }
        true
    }

    /// MUST succeed on both UPG and non-UPG accounts (Round 1 F1 + Round 2 F3).
    #[test]
    fn agy_r1_f1_upg_0775_stream_and_guarded_delete_succeed() {
        let dir = scratch_dir("agy-r1-f1-int");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.join("home");
        let rt = dir.join("run");
        let state = dir.join("state");
        let data = dir.join("data");
        let desktop = dir.join("desktop");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(rt.join("olo-screens")).unwrap();
        std::fs::create_dir_all(state.join("olo/screens/upg_del")).unwrap();
        std::fs::create_dir_all(data.join("Trash/files")).unwrap();
        std::fs::create_dir_all(data.join("Trash/info")).unwrap();
        write_modern_stub(&desktop);

        // Keep `rt` at `0700` (matching real `$XDG_RUNTIME_DIR`) so the `0775`
        // `olo-screens` stream test verifies private-ancestor enclosure on ALL accounts.
        std::fs::set_permissions(&rt, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(rt.join("olo-screens"), std::fs::Permissions::from_mode(0o775)).unwrap();

        // Simulate `umask 0002` `0775` permissions across state and Trash trees outside a `0700` ancestor.
        for p in [state.clone(), state.join("olo"), state.join("olo/screens"), data.clone(), data.join("Trash"), data.join("Trash/files"), data.join("Trash/info")] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o775)).unwrap();
        }

        // 1. Live `screen stream` on a running screen inside `0700` `rt` -> `0775` `olo-screens`
        //    MUST deliver Frame unconditionally on all accounts!
        let (w, h) = (64, 48);
        let mut stub = SessionStub::start("upg_live");
        let sock = rt.join("upg_live.sock");
        let _srv = FakeAppServer::start_in(&mut stub, sock.clone(), w, h, make_rgba(w, h, 3));
        write_screen_json(&rt, "upg_live", stub.pid(), &sock, w, h);
        let mut stream_c = Command::new(HELPER)
            .args(["screen", "stream", "upg_live"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let sin = stream_c.stdin.take().unwrap();
        let srx = server_msg_channel(stream_c.stdout.take().unwrap());
        let first = srx.recv_timeout(Duration::from_secs(4)).expect("stream reply").expect("stream msg");
        drop(sin);
        let _ = wait_child_timeout(&mut stream_c, Duration::from_secs(2));
        assert!(matches!(first, ServerMsg::Frame { .. }), "agy-r1 F1: screen stream failed on 0700 rt -> 0775 olo-screens dir, got {first:?}");

        // 2. Guarded `screen delete` on stopped screen inside `0775` state & Trash trees
        //    outside a `0700` ancestor succeeds iff both directories belong to `uid`'s UPG!
        let uid = unsafe { libc::geteuid() };
        let state_gid = std::fs::symlink_metadata(&state).unwrap().gid();
        let data_gid = std::fs::symlink_metadata(&data).unwrap().gid();
        let is_upg = is_private_user_group(uid, state_gid) && is_private_user_group(uid, data_gid);

        let sdir = state.join("olo/screens/upg_del");
        std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let mut del_cmd = Command::new(HELPER);
        del_cmd
            .args(["screen", "delete", "upg_del", "--expected-started", "0", "--expected-last-used", "0"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", &data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (code, _, err) = run_cmd_bounded(del_cmd, Duration::from_secs(4));
        if is_upg {
            assert_eq!(code, Some(0), "agy-r1 F1: guarded delete failed on UPG 0775 state/Trash dirs: {err}");
            assert!(!sdir.exists(), "agy-r1 F1: UPG guarded delete must move screen dir to Trash");
        } else {
            assert_eq!(code, Some(1), "agy-r2 F3: non-UPG 0775 state/Trash dirs outside 0700 ancestor must fail closed: {err}");
            assert!(sdir.exists(), "agy-r2 F3: refused non-UPG delete must leave screen dir intact");
        }

        // 3. Unconditionally verify `0700` private-ancestor enclosure for `0775` state & Trash
        //    trees so guarded `screen delete` is proven end-to-end on non-UPG accounts too!
        let priv_root = dir.join("priv-0700");
        let enc_state = priv_root.join("state");
        let enc_data = priv_root.join("data");
        let enc_sdir = enc_state.join("olo/screens/enc_del");
        std::fs::create_dir_all(&enc_sdir).unwrap();
        std::fs::create_dir_all(enc_data.join("Trash/files")).unwrap();
        std::fs::create_dir_all(enc_data.join("Trash/info")).unwrap();
        std::fs::set_permissions(&priv_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        for p in [enc_state.clone(), enc_state.join("olo"), enc_state.join("olo/screens"), enc_data.clone(), enc_data.join("Trash"), enc_data.join("Trash/files"), enc_data.join("Trash/info")] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o775)).unwrap();
        }
        std::fs::write(enc_sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let mut enc_del_cmd = Command::new(HELPER);
        enc_del_cmd
            .args(["screen", "delete", "enc_del", "--expected-started", "0", "--expected-last-used", "0"])
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("XDG_STATE_HOME", &enc_state)
            .env("XDG_DATA_HOME", &enc_data)
            .env("OLO_HOST_DESKTOP", &desktop)
            .env("OLO_ALLOW_TEST_SCREEN_CAPS", "1");
        let (enc_code, _, enc_err) = run_cmd_bounded(enc_del_cmd, Duration::from_secs(4));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(enc_code, Some(0), "agy-r2 F3: guarded delete inside 0700 ancestor -> 0775 state/Trash must succeed unconditionally: {enc_err}");
    }
}
