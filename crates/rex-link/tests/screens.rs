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
    // No desktop to ask: said with the way that needs none (of a screen
    // only: the desktop's other commands have no other way).
    let away = host.olo(&["screen", "start", "work"], &[("DESKTOP_SAYS", "69")]);
    assert_eq!(away.status.code(), Some(69));
    assert!(err(&away).contains("olo screen --on host"), "{}", err(&away));
    let open = host.olo(&["open", "http://localhost:1"], &[("DESKTOP_SAYS", "69")]);
    assert_eq!((open.status.code(), err(&open).as_str()), (Some(69), ""));
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
