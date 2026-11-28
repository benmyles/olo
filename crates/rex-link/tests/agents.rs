//! The agents service of a host (`olo-rex-link agent`), tried as it
//! runs: a Rex server of the test's own, the helper as built, and **no
//! Olo desktop anywhere**: every `olo agent` command is asked of the
//! service by the helper's own command, as `olo agent` asks it on a
//! host whose desktop is asleep.
//!
//! The children are stand-ins (`stand-in.sh`, run under an agent's
//! name): they draw what a test tells them to and write down what they
//! are typed, so what the service reads and what it types are exact.
//! (The agents themselves, against a made-up API, are `agents_live.rs`.)

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Reads field 22 (`starttime` in clock ticks) from `/proc/{pid}/stat` so a
/// test waiting for a retired service PID to exit is immune to PID reuse.
fn proc_start_ticks(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    // Field 22 is index 19 after the closing ')' of comm (field 2).
    after_comm.split_whitespace().nth(19)?.parse::<u64>().ok()
}

/// Writes `data` to a stand-in FIFO without ever blocking past `until`: uses
/// `O_NONBLOCK` open and write retries so a dead stand-in can never hang `join`.
fn write_fifo_within(fifo: &Path, data: &[u8], until: Instant) -> bool {
    let mut file = loop {
        match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(fifo) {
            Ok(f) => break f,
            Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    };
    let mut written = 0usize;
    while written < data.len() {
        match file.write(&data[written..]) {
            Ok(0) => return false,
            Ok(n) => written += n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {
                if Instant::now() >= until {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return false,
        }
    }
    true
}

const HELPER: &str = env!("CARGO_BIN_EXE_olo-rex-link");
const REX: &str = "/usr/bin/rex";
const STAND_IN: &str = include_str!("stand-in.sh");

fn rule() -> String {
    "─".repeat(60)
}

/// Claude Code's screens, as it draws them (2.1.33x): at work, asking
/// leave to run a command with "1. Yes" marked, and at its prompt.
fn working() -> String {
    format!("❯ Do what the brief says.\n● Read(brief.md)\n✻ Thinking… (12s)\n{0}\n❯ \n{0}\n  ⏸ manual mode on · esc to interrupt · ← for agents\n", rule())
}

fn asking(command: &str, marked: usize) -> String {
    let dashed = "╌".repeat(60);
    let mark = |i: usize| if i == marked { "❯" } else { " " };
    format!(
        "● Bash({command})\n{0}\n Bash command\n Make it so\n{dashed}\n │ {command}\n{dashed}\n Do you want to proceed?\n {1} 1. Yes\n {2} 2. Yes, and don’t ask again for: {command}\n {3} 3. No\n Esc to cancel · Tab to amend\n",
        rule(),
        mark(0),
        mark(1),
        mark(2)
    )
}

fn idle(said: &str) -> String {
    format!("● {said}\n✻ Crunched for 2s\n{0}\n❯ \n{0}\n  ⏸ manual mode on · ? for shortcuts\n", rule())
}

/// Codex's screens, as it draws them (0.172): at work (its status line
/// over its field), and with words of its own on the screen and no
/// status line (at its prompt; and what stands between two of its
/// steps, a turn still on).
fn codex_working() -> String {
    "› Review it.\n\n• Explored\n  └ Read review.md\n\n• Working (4s • esc to interrupt)\n\n› Ask Codex to do anything\n\n  GPT-6.2-Astra max · /var/tmp/x · Context 52% left\n  ← for agents · ? for shortcuts\n".to_owned()
}

fn codex_idle(said: &str) -> String {
    format!("› Review it.\n\n• {said}\n\n› Ask Codex to do anything\n\n  GPT-6.2-Astra max · /var/tmp/x · Context 52% left\n  ← for agents · ? for shortcuts\n")
}

struct Rig {
    dir: PathBuf,
    sock: PathBuf,
    server: Child,
    session: String,
    lead_window: String,
    lead_block: String,
    extra_env: Vec<(String, String)>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Capture the live service's `(pid, start_ticks)` BEFORE retiring and
        // wait for that exact process to exit (with a deadline) BEFORE
        // `remove_dir_all` so a delayed background writer never recreates the
        // test folder after cleanup.
        let retired_proc = self.service_pid().and_then(|pid| proc_start_ticks(pid).map(|ticks| (pid, ticks)));
        let _ = self.helper().args(["agent", "retire"]).output();
        let _ = self.server.kill();
        let _ = self.server.wait();
        if let Some((pid, start_ticks)) = retired_proc {
            let until = Instant::now() + Duration::from_secs(5);
            while proc_start_ticks(pid) == Some(start_ticks) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(15));
            }
        } else {
            std::thread::sleep(Duration::from_millis(100));
        }
        if std::env::var_os("OLO_AGENTS_TEST_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Rig {
    /// A Rex server of this test's own with one session and one tab in
    /// it (the lead's); none where Rex isn't installed.
    fn start(tag: &str) -> Option<Self> {
        Self::start_with_env(tag, &[])
    }

    fn start_with_env(tag: &str, extra_env: &[(&str, &str)]) -> Option<Self> {
        if !Path::new(REX).is_file() {
            eprintln!("skipped: {REX} isn't installed");
            return None;
        }
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!("/var/tmp/olo-agents-test-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["home", "run", "st", "agents", "bin", "stage", "work"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let sock = dir.join("rex.sock");
        let server = Self::serve(&dir, &sock);
        let mut rig = Self {
            dir,
            sock,
            server,
            session: String::new(),
            lead_window: String::new(),
            lead_block: String::new(),
            extra_env: extra_env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        };
        rig.lead();
        Some(rig)
    }

    fn serve(dir: &Path, sock: &Path) -> Child {
        let _ = std::fs::remove_file(sock);
        let at = format!("unix://{}", sock.display());
        let mut server = Command::new(REX);
        server.args(["-S", &at, "server", "run", "--listen", &at, "--restore=false", "--state-dir"]).arg(dir.join("st")).current_dir(dir.join("work")).stdout(Stdio::null()).stderr(Stdio::null());
        Self::scratch_env(dir, sock, &mut server);
        let server = server.spawn().expect("the scratch Rex server");
        let until = Instant::now() + Duration::from_secs(8);
        while !sock.exists() {
            assert!(Instant::now() < until, "the scratch Rex server didn't begin to listen");
            std::thread::sleep(Duration::from_millis(20));
        }
        server
    }

    /// Nothing of the computer the test runs on: a home, a runtime
    /// folder and a Rex server of its own, and no pane it happens to be
    /// run from.
    fn scratch_env(dir: &Path, sock: &Path, cmd: &mut Command) {
        cmd.env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", dir.join("run"))
            .env("XDG_STATE_HOME", dir.join("st"))
            .env("XDG_CONFIG_HOME", dir.join("st"))
            .env("REX_SERVER_SOCKET", sock)
            .env("OLO_REX_AGENTS_DIR", dir.join("agents"))
            // (How long a minder has to report once its agent has ended: three seconds here, of three minutes.)
            .env("OLO_REX_AGENTS_MINDER_REPORTS_MS", "3000")
            // (How long an agent at rest has to take up a message sent to it before its rest is told all the same: 2.5 s here, of thirty.)
            .env("OLO_REX_AGENTS_TAKEN_UP_MS", "2500")
            // (How long a Codex's conversation file is believed that a turn is on while it shows no life: three seconds here, of ten.)
            .env("OLO_REX_AGENTS_TURN_ON_MS", "3000")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .env_remove("SSH_AUTH_SOCK");
        // (Run from an agent's own shell, the test carries that agent's
        // marks: none of them is the scratch server's or its children's.
        // Likewise strip any desktop-bridge dirs from the inherited PATH so
        // the scratch Rex server starts with a clean systemd-like base PATH
        // even when `jato test` runs inside a desktop-connected Rex pane.)
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|n| n.starts_with("CLAUDE") || n.starts_with("CODEX") || n == "AI_AGENT") {
                cmd.env_remove(name);
            }
        }
        if let Ok(path) = std::env::var("PATH") {
            let clean: Vec<&str> = path.split(':').filter(|d| !d.is_empty() && !d.ends_with("/rex-commands") && !d.ends_with("/olo/rex/bin")).collect();
            if !clean.is_empty() {
                cmd.env("PATH", clean.join(":"));
            }
        }
    }

    /// The session and the lead's tab, made anew (after the server was
    /// started again too).
    fn lead(&mut self) {
        let session = self.rex("session.create", json!({ "label": "lead" }));
        self.session = session["session_id"].as_str().expect("a session").to_owned();
        let window = self.rex(
            "session.new_window",
            json!({ "session_id": self.session, "window_label": "lead", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 110, "rows": 40 } } } } }),
        );
        self.lead_window = window["window_id"].as_str().expect("a window").to_owned();
        self.lead_block = window["block_ids"][0].as_str().expect("a block").to_owned();
    }

    /// The Rex server asked directly (as `rex api call` asks it).
    fn rex(&self, method: &str, payload: Value) -> Value {
        let mut cmd = Command::new(REX);
        cmd.args(["-S", &format!("unix://{}", self.sock.display()), "api", "call", method, &payload.to_string()]);
        Self::scratch_env(&self.dir, &self.sock, &mut cmd);
        // (A server just started has its socket's file before it
        // answers on it: on a busy computer the first call met "no Rex
        // server is running at …". Asked again for a few seconds.)
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            let out = cmd.output().expect("rex api call");
            if out.status.success() {
                return serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
            }
            let said = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(said.contains("no Rex server is running") && Instant::now() < until, "rex api call {method}: {said}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn helper(&self) -> Command {
        let mut cmd = Command::new(HELPER);
        cmd.current_dir(self.dir.join("work")).env("PWD", self.dir.join("work"));
        Self::scratch_env(&self.dir, &self.sock, &mut cmd);
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        cmd
    }

    /// `olo agent ARGS`, asked from a pane (`REX_BLOCK`): its status,
    /// what it printed and what it said.
    fn olo_from(&self, block: Option<&str>, args: &[&str]) -> (i32, String, String) {
        let mut cmd = self.helper();
        cmd.arg("agent").args(args);
        if let Some(block) = block {
            cmd.env("REX_BLOCK", block);
        }
        let out = cmd.output().expect("the helper");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    /// From the lead's pane.
    fn olo(&self, args: &[&str]) -> (i32, String, String) {
        self.olo_from(Some(&self.lead_block), args)
    }

    /// A command that must succeed and print one JSON object, on one line.
    fn json(&self, args: &[&str]) -> Value {
        let (code, out, err) = self.olo(args);
        assert_eq!(code, 0, "olo agent {args:?} failed\nstdout: {out}\nstderr: {err}\n{}", self.said());
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1, "olo agent {args:?} prints one JSON object on one line:\n{out}");
        serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("olo agent {args:?}: {e}\n{out}"))
    }

    /// What the service said in its log, for a failure's account.
    fn said(&self) -> String {
        let mut said = String::new();
        if let Ok(dirs) = std::fs::read_dir(self.dir.join("agents")) {
            for d in dirs.flatten() {
                said.push_str(&std::fs::read_to_string(d.path().join("service.log")).unwrap_or_default());
            }
        }
        format!("the service's log:\n{said}")
    }

    /// Puts the stand-in on the children's PATH under an agent's name.
    fn stand_in(&self, cli: &str) {
        let file = self.dir.join("bin").join(cli);
        std::fs::write(&file, STAND_IN).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn staged(&self, name: &str, what: &str) -> PathBuf {
        self.dir.join("stage").join(format!("{name}.{what}"))
    }

    /// The folder the rig's children work in, where they are told one (`--cwd`).
    fn work(&self) -> String {
        self.dir.join("work").display().to_string()
    }

    /// A stand-in Claude Code's conversation `session` (its `--session-id`), written as Claude Code keeps one, in
    /// the rig's own home: what it was asked, and what it said last, its turn over. (A stand-in draws screens and
    /// keeps no conversation; what a turn's end has to say is read from this file, as it is from the agent's own.)
    fn says(&self, session: &str, asked: &str, said: &str) {
        let folder: String = self.work().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let dir = self.dir.join("home/.claude/projects").join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [
            json!({ "type": "user", "message": { "role": "user", "content": asked } }),
            json!({ "type": "assistant", "message": { "id": "msg_1", "role": "assistant", "content": [{ "type": "text", "text": said }], "stop_reason": "end_turn" } }),
        ];
        std::fs::write(dir.join(format!("{session}.jsonl")), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }

    /// Starts a stand-in child and waits until it listens.
    fn start_child_from(&self, block: Option<&str>, cli: &str, name: &str, more: &[&str]) -> Value {
        let fifo = self.staged(name, "fifo");
        assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let (stage, path) = (format!("STAND_IN_DIR={}", self.dir.join("stage").display()), format!("PATH={}:/usr/bin:/bin", self.dir.join("bin").display()));
        let named = format!("STAND_IN_NAME={name}");
        let mut args = vec!["start", cli, "--model", "stand-in", "--effort", "none", "--name", name, "--json", "--env", &stage, "--env", &named, "--env", &path];
        args.extend_from_slice(more);
        let (code, out, err) = self.olo_from(block, &args);
        assert_eq!(code, 0, "start {name}: {err}\n{}", self.said());
        let started: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("start {name}: {e}\n{out}"));
        self.wait("the stand-in to listen", || self.staged(name, "ready").exists());
        started
    }

    fn start_child(&self, cli: &str, name: &str, more: &[&str]) -> Value {
        self.start_child_from(Some(&self.lead_block), cli, name, more)
    }

    /// Tells a stand-in one thing (bounded non-blocking FIFO write so a dead
    /// stand-in can never hang the test thread).
    fn tell(&self, name: &str, line: &str) {
        let (fifo, line) = (self.staged(name, "fifo"), format!("{line}\n"));
        let until = Instant::now() + Duration::from_secs(10);
        assert!(write_fifo_within(&fifo, line.as_bytes(), until), "the stand-in {name} didn't take a line");
    }

    /// Has a stand-in draw a screen.
    fn show(&self, name: &str, screen: &str) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let file = self.staged(name, &format!("screen{}", N.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&file, screen).unwrap();
        self.tell(name, &format!("show {}", file.display()));
    }

    /// Every byte a stand-in was typed.
    fn keys(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.staged(name, "keys")).unwrap_or_default()
    }

    /// What a stand-in was typed since `before`, which must be `keys`
    /// and no more. (Waited for a moment: what the service has written
    /// to a tab reaches the stand-in's own record of it a little after
    /// the command that typed it has answered, on a busy computer.)
    fn typed_since(&self, name: &str, before: usize, keys: &[u8], what: &str) {
        let until = Instant::now() + Duration::from_secs(5);
        while self.keys(name).len() < before + keys.len() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        // (And nothing after it.)
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(String::from_utf8_lossy(&self.keys(name)[before..]), String::from_utf8_lossy(keys), "{what}");
    }

    fn wait(&self, what: &str, mut is: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(15);
        while !is() {
            assert!(Instant::now() < until, "waited 15 s for {what}\n{}", self.said());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The test plays the agent: once the stand-in has been typed
    /// `keys` (after what it had been typed before), it draws `then`.
    fn when_typed(&self, name: &str, keys: &'static [u8], then: String) -> std::thread::JoinHandle<bool> {
        self.when_typed_after(name, keys, Duration::ZERO, then)
    }

    fn when_typed_after(&self, name: &str, keys: &'static [u8], delay: Duration, then: String) -> std::thread::JoinHandle<bool> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let (file, from) = (self.staged(name, "keys"), self.keys(name).len());
        let (fifo, stage) = (self.staged(name, "fifo"), self.staged(name, &format!("then{}-{}", from, N.fetch_add(1, Ordering::Relaxed))));
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                let typed = std::fs::read(&file).unwrap_or_default();
                if typed.len() > from && typed[from..].windows(keys.len()).any(|w| w == keys) {
                    break;
                }
                if Instant::now() >= until {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if !delay.is_zero() {
                let wake = (Instant::now() + delay).min(until);
                if wake > Instant::now() {
                    std::thread::sleep(wake.saturating_duration_since(Instant::now()));
                }
            }
            if std::fs::write(&stage, then).is_err() {
                return false;
            }
            write_fifo_within(&fifo, format!("show {}\n", stage.display()).as_bytes(), until)
        })
    }

    /// Deterministic barrier variant of [`Self::when_typed`]: once `keys`
    /// arrive in `name`, signals the returned `seen` receiver and waits on the
    /// returned `release` sender before drawing `then` via non-blocking FIFO
    /// handoff (zero scheduling sleeps).
    fn when_typed_gated(&self, name: &str, keys: &'static [u8], then: String) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<bool>) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let (file, from) = (self.staged(name, "keys"), self.keys(name).len());
        let (fifo, stage) = (self.staged(name, "fifo"), self.staged(name, &format!("gated{}-{}", from, N.fetch_add(1, Ordering::Relaxed))));
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                let typed = std::fs::read(&file).unwrap_or_default();
                if typed.len() > from && typed[from..].windows(keys.len()).any(|w| w == keys) {
                    break;
                }
                if Instant::now() >= until {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = seen_tx.send(());
            let rem = until.saturating_duration_since(Instant::now());
            if release_rx.recv_timeout(rem).is_err() {
                return false;
            }
            if std::fs::write(&stage, then).is_err() {
                return false;
            }
            write_fifo_within(&fifo, format!("show {}\n", stage.display()).as_bytes(), until)
        });
        (seen_rx, release_tx, handle)
    }

    fn status(&self, name: &str) -> Value {
        self.json(&["status", name, "--json"])
    }

    fn wait_state(&self, name: &str, state: &str) -> Value {
        let mut last = Value::Null;
        self.wait(&format!("{name} to be {state}"), || {
            last = self.status(name);
            last["state"] == state
        });
        last
    }

    fn service_pid(&self) -> Option<i32> {
        let out = self.helper().args(["agent", "ping"]).output().ok()?;
        serde_json::from_slice::<Value>(&out.stdout).ok()?["pid"].as_i64().map(|p| p as i32)
    }

    /// One raw request to the service's socket, as another build of the
    /// helper would send it.
    fn raw(&self, request: Value) -> Value {
        use std::io::BufRead as _;
        let socket = std::fs::read_dir(self.dir.join("agents")).unwrap().flatten().map(|d| d.path().join("sock")).find(|s| s.exists()).expect("the service's socket");
        let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("the service");
        stream.write_all(format!("{request}\n").as_bytes()).unwrap();
        let mut lines = std::io::BufReader::new(stream);
        let (mut taken, mut line) = (String::new(), String::new());
        lines.read_line(&mut taken).unwrap();
        assert_eq!(taken, "\n", "the service says it took a request before it does it");
        lines.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or(Value::Null)
    }
}

/// Item 19, whole: a child started, read, answered, told, stopped and
/// its events followed by cursor, with no Olo desktop connected to the
/// server at any point.
#[test]
fn every_command_works_with_no_desktop_connected() {
    let Some(rig) = Rig::start("nodesk") else { return };
    rig.stand_in("claude");
    let session_id = "11111111-2222-3333-4444-555555555555";
    let started = rig.start_child("claude", "builder", &["--session-id", session_id]);
    assert_eq!(started["ok"], true);
    assert_eq!(started["name"], "builder");
    assert_eq!(started["desktop"], "none", "a child has nothing of the desktop unless given it");
    assert_eq!(started["parent_window_id"], json!(rig.lead_window));
    assert_eq!(started["cli_session_id"], session_id);
    assert!(started["started_at"].as_i64().unwrap() > 1_700_000_000_000, "milliseconds");
    let mut cursor = started["cursor"].as_u64().unwrap();

    // Nobody but the service is the server's client: no desktop.
    let clients = rig.rex("client.list", json!({}));
    let clients: Vec<&Value> = clients["clients"].as_array().unwrap().iter().filter(|c| c["connection_state"] == "connected").collect();
    assert!(!clients.iter().any(|c| c["info"]["kind"] == "app"), "no desktop is connected: {clients:?}");
    assert!(clients.iter().any(|c| c["info"]["name"] == "Olo agents"), "the service is: {clients:?}");

    // At work.
    rig.show("builder", &working());
    let at_work = rig.wait_state("builder", "working");
    assert!(at_work["pid"].as_i64().is_some_and(|p| p > 1), "{at_work}");
    assert_eq!(at_work["running"], true);
    assert!(at_work["last_output_at"].as_u64().unwrap() > 1_700_000_000_000);

    // It asks leave: an event with the whole command and each choice by its kind.
    rig.show("builder", &asking("touch one.txt", 0));
    let asked = rig.json(&["watch", "--all", "--since", &cursor.to_string(), "--on", "approval,error,exit", "--timeout", "15", "--json"]);
    assert_eq!(asked["event"], "waiting_approval", "{asked}");
    assert_eq!(asked["name"], "builder");
    assert!(asked["approval"]["command"].as_str().unwrap().contains("touch one.txt"), "{asked}");
    let kinds: Vec<&str> = asked["approval"]["choices"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["once", "dont_ask_again", "no"], "a choice that keeps a rule is never `once`");
    assert!(asked["cursor"].as_u64().unwrap() > cursor);
    cursor = asked["cursor"].as_u64().unwrap();
    assert_eq!(rig.status("builder")["state"], "waiting_approval");

    // Text isn't typed into a question.
    let before = rig.keys("builder").len();
    let (code, _, err) = rig.olo(&["send", "builder", "go on", "--enter"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("dialog is active"), "{err}");
    assert_eq!(rig.keys("builder").len(), before, "nothing was typed");

    // Yes, once: Return alone where the mark stands on it; the question goes.
    let agent = rig.when_typed("builder", b"\r", working());
    let approved = rig.json(&["approve", "builder", "--once", "--json"]);
    assert!(agent.join().unwrap());
    assert_eq!(approved["chosen_kind"], "once");
    assert_eq!(approved["dismissed"], true, "{approved}");
    rig.typed_since("builder", before, b"\r", "Return and nothing else");

    // Its turn ends: an event after the cursor.
    rig.show("builder", &idle("Done with this part."));
    let rested = rig.json(&["watch", "--all", "--since", &cursor.to_string(), "--on", "idle,done,error,exit", "--timeout", "15", "--json"]);
    assert!(matches!(rested["event"].as_str(), Some("idle" | "done")), "{rested}");
    cursor = rested["cursor"].as_u64().unwrap();

    // A message of several lines goes as one paste, then Return by itself.
    let message = rig.dir.join("work/message.txt");
    std::fs::write(&message, "Next: the tests.\nThen say DONE.\n").unwrap();
    let before = rig.keys("builder").len();
    let agent = rig.when_typed("builder", b"\x1b[201~\r", working());
    let sent = rig.json(&["send", "builder", "--file", "message.txt", "--enter", "--json"]);
    assert!(agent.join().unwrap());
    assert_eq!(sent["delivered"], true, "{sent}");
    rig.typed_since("builder", before, b"\x1b[200~Next: the tests.\nThen say DONE.\n\x1b[201~\r", "what was typed into it");

    // What it shows is read.
    let read = rig.json(&["read", "builder", "--lines", "20", "--json"]);
    assert!(read["output"].as_str().unwrap().contains("Thinking"), "{read}");

    // A key with Alt, as tmux's `M-m`.
    let before = rig.keys("builder").len();
    rig.json(&["send", "builder", "--key", "Alt+m", "--json"]);
    rig.wait("the key", || rig.keys("builder").len() > before);
    rig.typed_since("builder", before, b"\x1bm", "what was typed into it");

    // Stopped, what it showed saved here; its conversation's id kept.
    let stopped = rig.json(&["stop", "builder", "--save", "saved/builder.txt", "--json"]);
    assert_eq!(stopped["cli_session_id"], session_id);
    assert_eq!(stopped["saved_path"], json!(rig.dir.join("work/saved/builder.txt")));
    assert!(std::fs::read_to_string(rig.dir.join("work/saved/builder.txt")).unwrap().contains("Thinking"));
    let gone = rig.json(&["watch", "--all", "--since", &cursor.to_string(), "--on", "exit", "--timeout", "10", "--json"]);
    assert_eq!(gone["event"], "exited", "{gone}");
    assert_eq!(gone["name"], "builder");
    let listed = rig.json(&["list", "--all", "--json"]);
    let kept = listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == "builder").expect("kept among the exited");
    assert_eq!(kept["state"], "exited");
    assert_eq!(kept["cli_session_id"], session_id);
    // (Its tab is closed on the server.)
    let view = rig.rex("session.view", json!({ "session_id": rig.session }));
    assert_eq!(view["windows"].as_array().unwrap().len(), 1, "only the lead's tab is left");
    assert_eq!(rig.json(&["prune", "--json"])["pruned"], 1);
}

/// Item 20: a child that is a command of the asker's own.
#[test]
fn a_command_of_ones_own_is_a_child_tab() {
    let Some(rig) = Rig::start("command") else { return };
    // It runs and ends: its end is an event with its status.
    let started = rig.json(&["start", "--command", "--name", "ticker", "--json", "--", "sh", "-c", "echo tick; sleep 0.4; exit 7"]);
    assert_eq!(started["cli"], "command");
    assert_eq!(started["command"], json!(["sh", "-c", "echo tick; sleep 0.4; exit 7"]));
    let ended = rig.json(&["watch", "ticker", "--on", "exit", "--timeout", "15", "--json"]);
    assert_eq!(ended["event"], "exited", "{ended}");
    assert_eq!(ended["summary"], "exited (code 7)");
    let status = rig.status("ticker");
    assert_eq!((&status["state"], &status["running"], &status["exit_code"]), (&json!("exited"), &json!(false), &json!(7)));

    // One that goes on is running, is read, is typed into and is stopped.
    let reader = rig.json(&["start", "--command", "--name", "reader", "--json", "--", "sh", "-c", "echo ready; while IFS= read -r line; do echo \"got:$line\"; done"]);
    rig.wait("its first line", || rig.json(&["read", "reader", "--json"])["output"].as_str().unwrap().contains("ready"));
    let status = rig.status("reader");
    assert_eq!((&status["state"], &status["running"]), (&json!("working"), &json!(true)), "{status}");
    assert_eq!(status["command"][0], "sh", "what it runs is said: {status}");
    rig.json(&["send", "reader", "a line", "--enter", "--json"]);
    rig.wait("its answer", || rig.json(&["read", "reader", "--json"])["output"].as_str().unwrap().contains("got:a line"));
    // (Nothing an agent's screen would mean is read into what it prints.)
    rig.json(&["send", "reader", "Do you want to proceed? 1. Yes 2. No", "--enter", "--json"]);
    assert_eq!(rig.status("reader")["state"], "working");
    // Listed with the lead's other children, by the lead's tab.
    let listed = rig.json(&["list", "--parent", &rig.lead_window, "--json"]);
    assert!(listed["subagents"].as_array().unwrap().iter().any(|s| s["id"] == reader["id"] && s["cli"] == "command"), "{listed}");
    let stopped = rig.json(&["stop", "reader", "--json"]);
    assert_eq!(stopped["stopped"], json!([reader["id"]]));

    // What only an agent takes is refused, and a command is asked for.
    let (code, _, err) = rig.olo(&["start", "--command", "--model", "m", "--", "true"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("agent's option"), "{err}");
    let (code, _, err) = rig.olo(&["start", "--command"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("after `--`"), "{err}");
}

/// The service can be killed at any moment: the next command starts
/// another, which knows the children (its files, and the server's tabs),
/// goes on numbering events where the last stopped, and doesn't say
/// twice what was said. And after the Rex server itself is started
/// again, every child has exited and keeps its conversation's id.
#[test]
fn a_service_killed_and_a_server_restarted_lose_nothing() {
    let Some(mut rig) = Rig::start("killed") else { return };
    rig.stand_in("claude");
    let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let started = rig.start_child("claude", "survivor", &["--session-id", session_id]);
    rig.show("survivor", &asking("make test", 0));
    let asked = rig.json(&["watch", "survivor", "--since", &started["cursor"].to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    let cursor = asked["cursor"].as_u64().unwrap();

    // Killed, with no chance to write anything.
    let first = rig.service_pid().expect("a service");
    assert!(Command::new("kill").args(["-9", &first.to_string()]).status().unwrap().success());
    rig.wait("the service to be gone", || !Path::new(&format!("/proc/{first}")).exists());

    // The next command starts another: the child is there, as it stands.
    let status = rig.status("survivor");
    assert_eq!(status["state"], "waiting_approval", "{status}");
    assert_ne!(rig.service_pid(), Some(first));
    assert!(status["cursor"].as_u64().unwrap() >= cursor, "the numbering goes on: {status}");
    // The question that was said is not said again…
    let (code, out, _) = rig.olo(&["watch", "survivor", "--since", &cursor.to_string(), "--on", "approval", "--timeout", "2", "--json"]);
    assert_eq!(code, 124, "nothing new since the cursor: {out}");
    // …and what happens next is, after it.
    rig.show("survivor", &idle("Tests pass."));
    let rested = rig.json(&["watch", "survivor", "--since", &cursor.to_string(), "--on", "idle,done", "--timeout", "15", "--json"]);
    assert!(rested["cursor"].as_u64().unwrap() > cursor, "{rested}");
    let cursor = rested["cursor"].as_u64().unwrap();

    // The Rex server itself started again: its tabs are gone with it.
    let _ = rig.server.kill();
    let _ = rig.server.wait();
    rig.wait("the service to end with its server", || rig.service_pid().is_none());
    rig.server = Rig::serve(&rig.dir, &rig.sock);
    rig.lead();
    let listed = rig.json(&["list", "--all", "--json"]);
    let kept = listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == "survivor").unwrap_or_else(|| panic!("kept: {listed}"));
    assert_eq!(kept["state"], "exited");
    assert_eq!(kept["cli_session_id"], session_id, "to be taken up again");
    assert!(listed["cursor"].as_u64().unwrap() > cursor, "its exit was written down: {listed}");
}

/// A child may act on its own descendants alone, and is known by the
/// process that asks, whatever pane that says it is in; and what a
/// child may ask of the desktop is said for a process or a pane.
#[test]
fn a_child_is_known_by_its_process_and_keeps_to_its_own() {
    let Some(rig) = Rig::start("confined") else { return };
    rig.stand_in("claude");
    let a = rig.start_child("claude", "alpha", &[]);
    let b = rig.start_child("claude", "beta", &["--desktop", "screens=qa-1"]);
    let helper = format!("OLO_REX_AGENTS_DIR={} REX_SERVER_SOCKET={} {HELPER} agent", rig.dir.join("agents").display(), rig.sock.display());
    let run_in = |name: &str, command: String| -> String {
        let _ = std::fs::remove_file(rig.staged(name, "ran"));
        rig.tell(name, &format!("run {command}"));
        rig.wait("the command in the child's tab", || rig.staged(name, "ran").exists());
        std::fs::read_to_string(rig.staged(name, "out")).unwrap()
    };

    // From inside alpha's tab: its own child is started under it…
    let inner = run_in("alpha", format!("{helper} start --command --name inner --json -- sleep 300"));
    let inner: Value = serde_json::from_str(inner.trim()).unwrap_or_else(|e| panic!("{e}: {inner}"));
    assert_eq!(inner["parent_window_id"], a["window_id"], "under the tab that asked");
    // …and it sees that child and no other.
    let seen = run_in("alpha", format!("{helper} list --all --json"));
    let seen: Value = serde_json::from_str(seen.trim()).unwrap();
    let names: Vec<&str> = seen["subagents"].as_array().unwrap().iter().filter_map(|s| s["name"].as_str()).collect();
    assert_eq!(names, ["inner"], "{seen}");
    // Its sibling, its parent's other children and itself are not its to act on,
    // even when it says it is the lead's pane.
    for refused in ["stop beta --force", "status beta", "send beta hello", "read alpha", "prune"] {
        let said = run_in("alpha", format!("REX_BLOCK={} {helper} {refused}; echo status=$?", rig.lead_block));
        assert!(!said.contains("status=0"), "alpha did `{refused}`: {said}");
    }
    assert_eq!(rig.status("beta")["running"], true, "beta wasn't stopped");
    // It can't give its own child more of the desktop than it has.
    let said = run_in("alpha", format!("{helper} start --command --desktop inherit -- true; echo status=$?"));
    assert!(said.contains("status=69"), "{said}");
    let said = run_in("beta", format!("{helper} start --command --desktop screens=qa-2 -- true; echo status=$?"));
    assert!(said.contains("status=69"), "{said}");

    // What each may ask of the desktop, by the asking process (a child's
    // own, whatever pane it names) and by the pane.
    let alpha_pid = rig.status("alpha")["pid"].as_i64().unwrap();
    let policy = |pid: Option<i64>, block: &str| rig.raw(json!({ "proto": 1, "op": "policy", "block": block, "obj": { "pid": pid } }))["data"].clone();
    assert_eq!(policy(Some(alpha_pid), &rig.lead_block)["desktop"], "none", "its process says whose it is");
    assert_eq!(policy(None, b["block_id"].as_str().unwrap())["desktop"], "screens=qa-1");
    assert_eq!(policy(None, inner["block_id"].as_str().unwrap())["desktop"], "none", "a child's child has no more than it");
    assert_eq!(policy(Some(i64::from(std::process::id())), &rig.lead_block)["desktop"], Value::Null, "the lead's pane is nobody's child");

    // The lead stops alpha and all under it.
    let stopped = rig.json(&["stop", "alpha", "--tree", "--force", "--json"]);
    assert_eq!(stopped["stopped"].as_array().unwrap().len(), 2, "{stopped}");
    rig.json(&["stop", "beta", "--force", "--json"]);
}

/// What a desktop that connects is told: everything, then each change.
#[test]
fn a_follower_is_told_everything_and_then_each_change() {
    use std::io::BufRead as _;
    let Some(rig) = Rig::start("follow") else { return };
    rig.stand_in("agy");
    // (A child from before anyone followed.)
    rig.json(&["start", "--command", "--name", "before", "--json", "--", "sleep", "300"]);
    let mut follow = rig.helper().args(["agent", "follow"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut frames = std::io::BufReader::new(follow.stdout.take().unwrap()).lines();
    let mut next = || -> Value { serde_json::from_str(&frames.next().expect("a frame").unwrap()).unwrap() };
    let first = next();
    assert_eq!(first["t"], "snapshot", "{first}");
    let known: Vec<&str> = first["agents"].as_array().unwrap().iter().filter_map(|a| a["name"].as_str()).collect();
    assert_eq!(known, ["before"], "all it knows, at once");
    assert_eq!(first["agents"][0]["parent_window_id"], json!(rig.lead_window));

    // One that starts is said, with whose child it is.
    let started = rig.start_child("agy", "later", &[]);
    // (The next it hears of the new one: the one from before may have
    // something said of it first, its first look at its screen coming
    // just then. Taking the very next frame for the new child's failed
    // three runs in four with the computer quick, and passed with it
    // busy, where the first child had settled before anyone followed.)
    let mut said = next();
    while said["agent"]["id"] != started["id"] {
        assert_eq!(said["agent"]["name"], "before", "{said}");
        said = next();
    }
    assert_eq!((&said["t"], &said["agent"]["id"], &said["agent"]["cli"]), (&json!("agent"), &started["id"], &json!("agy")), "{said}");
    assert_eq!(said["agent"]["desktop_policy"]["mode"], "none");
    // How it stands, as that changes.
    rig.show("later", "Command\n────────────────────\nRequesting permission for:\n   rm -f /var/tmp/x\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation\n  3. No, cancel\n  ↑/↓ Navigate\n");
    let mut asked = next();
    while asked["agent"]["state"] != "waiting_approval" {
        asked = next();
    }
    assert_eq!(asked["agent"]["id"], started["id"]);
    // Its end, and its record's.
    rig.json(&["stop", "later", "--force", "--json"]);
    let mut ended = next();
    while ended["agent"]["state"] != "exited" {
        ended = next();
    }
    assert!(ended["agent"]["exited_at"].as_i64().is_some(), "{ended}");
    rig.json(&["prune", "--json"]);
    let mut gone = next();
    while gone["t"] != "gone" {
        gone = next();
    }
    assert_eq!(gone["id"], started["id"]);
    // The follower's input closing ends it.
    drop(follow.stdin.take());
    assert_eq!(follow.wait().unwrap().code(), Some(0));
}

/// A following that is asked to end (its input closed) says it was
/// asked to: 0, never "the service went away, follow again" (75). The
/// closing ends its reading of the service at once, and the two raced
/// to say why it ended; with many at once the reading often won.
#[test]
fn a_following_asked_to_end_says_it_was_asked() {
    use std::io::BufRead as _;
    let Some(rig) = Rig::start("unfollow") else { return };
    rig.json(&["start", "--command", "--name", "there", "--json", "--", "sleep", "300"]);
    for round in 0..4 {
        let mut following: Vec<_> = (0..24)
            .map(|_| {
                let mut f = rig.helper().args(["agent", "follow"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
                // (Following: its first frame is here.)
                let mut frames = std::io::BufReader::new(f.stdout.take().unwrap()).lines();
                assert!(frames.next().is_some_and(|l| l.is_ok_and(|l| l.contains("snapshot"))));
                (f, frames)
            })
            .collect();
        for (f, _) in &mut following {
            drop(f.stdin.take());
        }
        let codes: Vec<Option<i32>> = following.iter_mut().map(|(f, _)| f.wait().unwrap().code()).collect();
        assert!(codes.iter().all(|c| *c == Some(0)), "round {round}: {codes:?}");
    }
    rig.json(&["stop", "there", "--force", "--json"]);
}

/// A feedback survey drawn over a child's screen is set aside by the
/// service (its own "0"), and what lies under it is what is said.
#[test]
fn a_feedback_survey_is_set_aside() {
    let Some(rig) = Rig::start("survey") else { return };
    rig.stand_in("claude");
    let started = rig.start_child("claude", "surveyed", &[]);
    rig.show("surveyed", &working());
    rig.wait_state("surveyed", "working");
    // Claude Code's survey over its prompt; the test's agent takes it away at "0".
    let survey = format!("● Done.\n{0}\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n{0}\n❯ \n{0}\n  ⏸ manual mode on · ? for shortcuts\n", rule());
    let before = rig.keys("surveyed").len();
    let agent = rig.when_typed("surveyed", b"0", idle("Done."));
    rig.show("surveyed", &survey);
    assert!(agent.join().unwrap(), "the service set the survey aside");
    let rested = rig.json(&["watch", "surveyed", "--since", &started["cursor"].to_string(), "--on", "idle,done", "--timeout", "15", "--json"]);
    assert!(matches!(rested["event"].as_str(), Some("idle" | "done")), "{rested}");
    rig.typed_since("surveyed", before, b"0", "once, and nothing else");
    assert_eq!(rig.status("surveyed")["survey"], false);
    // An agent slow to take it away (a busy host: half a second and more to draw): its key once all the
    // same. Pressed again after 300 ms, the second `0` was typed into the agent's field.
    rig.show("surveyed", &working());
    rig.wait_state("surveyed", "working");
    let before = rig.keys("surveyed").len();
    let agent = rig.when_typed_after("surveyed", b"0", Duration::from_millis(650), idle("Done again."));
    rig.show("surveyed", &survey);
    assert!(agent.join().unwrap(), "the service set the slow one's survey aside");
    rig.wait("the survey to be gone", || rig.status("surveyed")["survey"] == false);
    // (Asked of from several sides meanwhile, as a list, a watch and a status are: still once.)
    for _ in 0..4 {
        let _ = rig.status("surveyed");
    }
    rig.typed_since("surveyed", before, b"0", "once for a showing, however slow the agent is to draw");
    // One that stays (its key does nothing): pressed once, said, and left alone.
    rig.show("surveyed", &working());
    rig.wait_state("surveyed", "working");
    let before = rig.keys("surveyed").len();
    rig.show("surveyed", &survey);
    rig.wait("the survey that stays to be said", || rig.status("surveyed")["survey"] == true && rig.keys("surveyed").len() > before);
    std::thread::sleep(Duration::from_millis(1300));
    let _ = rig.status("surveyed");
    rig.typed_since("surveyed", before, b"0", "once, though it stays");
    // A repaint caught empty after its key, then the same survey once more before it goes (a slow
    // agent on a busy host): one showing, its key once. (The empty frame let the showing go, and
    // the survey drawn again was pressed for again: a second `0`, the field's if the first was taken.)
    rig.show("surveyed", &working());
    rig.wait_state("surveyed", "working");
    let before = rig.keys("surveyed").len();
    let blank = rig.when_typed("surveyed", b"0", "\n\n".to_owned());
    rig.show("surveyed", &survey);
    assert!(blank.join().unwrap(), "the service set the survey aside");
    std::thread::sleep(Duration::from_millis(400));
    rig.show("surveyed", &survey);
    std::thread::sleep(Duration::from_millis(700));
    for _ in 0..3 {
        let _ = rig.status("surveyed");
    }
    rig.show("surveyed", &idle("Done at last."));
    rig.wait_state("surveyed", "idle");
    rig.typed_since("surveyed", before, b"0", "once, though an empty frame came between two paints of it");
    rig.json(&["stop", "surveyed", "--force", "--json"]);
}

/// Claude Code's offer to send a feedback report it has drafted ("1 to
/// review · 2 to send · 0 to dismiss") is nobody's to answer but the
/// person's or the lead's: the service tells whoever looks after the
/// child, once for each showing, presses nothing itself, and types
/// nothing into the child while it shows but the box's own `0`, asked
/// for by name. (The ShiftUp lead's item 48, 2026-11-27; Ben's rule
/// there: "no feedback report is ever sent, and nothing answers that
/// box by itself".)
#[test]
fn a_drafted_feedback_report_is_told_and_never_answered() {
    let Some(rig) = Rig::start("draft") else { return };
    rig.stand_in("claude");
    let started = rig.start_child("claude", "drafted", &[]);
    rig.show("drafted", &idle("Done."));
    rig.wait_state("drafted", "idle");
    assert_eq!(rig.status("drafted")["feedback_draft"], Value::Null);
    let boxed = format!("● Done.\n\n  1 to review · 2 to send · 0 to dismiss\n{0}\n❯ \n{0}\n  ⏸ manual mode on · ? for shortcuts\n", rule());
    let before = rig.keys("drafted").len();
    rig.show("drafted", &boxed);
    // Told, once, as an escalation on the watch of whoever looks after it; and said in its status.
    let told = rig.json(&["watch", "drafted", "--since", &started["cursor"].to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!(told["event"], "escalation", "{told}");
    assert!(told["summary"].as_str().is_some_and(|s| s.starts_with("Feedback draft:") && s.contains("--key 0")), "{told}");
    assert_eq!(rig.status("drafted")["feedback_draft"], "1 to review · 2 to send · 0 to dismiss");
    let (code, out, _) = rig.olo(&["watch", "drafted", "--since", &told["cursor"].to_string(), "--on", "escalation", "--timeout", "2", "--json"]);
    assert_eq!(code, 124, "told twice of one showing: {out}");
    // Nothing is typed into it: not a message (one that begins "2" would send the report), not another key.
    for refused in [
        &["send", "drafted", "2 more things to do", "--enter", "--json"][..],
        &["send", "drafted", "--key", "2", "--json"],
        &["send", "drafted", "--key", "Enter", "--json"],
        // (`--force` is no way in, nor its 0 with another key beside it. A code review, 2026-11-27.)
        &["send", "drafted", "--force", "--key", "2", "--json"],
        &["send", "drafted", "--ctrl", "m", "--key", "0", "--json"],
        &["send", "drafted", "--key", "0", "--enter", "--json"],
    ] {
        let (code, out, err) = rig.olo(refused);
        assert!(code != 0 && err.contains("feedback draft"), "{refused:?} went through: {out}{err}");
    }
    assert_eq!(rig.keys("drafted").len(), before, "something was typed into the box: {:?}", &rig.keys("drafted")[before..]);
    // Its own 0, asked for by name: pressed, and nothing else.
    let agent = rig.when_typed("drafted", b"0", idle("Done."));
    rig.json(&["send", "drafted", "--key", "0", "--json"]);
    assert!(agent.join().unwrap(), "its 0 was pressed");
    rig.typed_since("drafted", before, b"0", "its 0, once, and nothing else");
    rig.wait("the box to be gone", || rig.status("drafted")["feedback_draft"].is_null());
    // Gone: what is sent is typed as before. And another showing is told again.
    let before = rig.keys("drafted").len();
    rig.json(&["send", "drafted", "hello", "--enter", "--json"]);
    assert!(rig.keys("drafted").len() > before);
    let since = rig.status("drafted")["cursor"].to_string();
    rig.show("drafted", &boxed);
    let again = rig.json(&["watch", "drafted", "--since", &since, "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!(again["event"], "escalation", "{again}");
    // Its command names the child by its id (a name may have a space in it).
    assert!(again["summary"].as_str().is_some_and(|s| s.contains(&format!("olo agent send {} --key 0", started["id"].as_str().unwrap()))), "{again}");
    // A survey's two lines still on the screen above the box: the service sets nothing aside while the box shows.
    let before = rig.keys("drafted").len();
    rig.show("drafted", &format!("● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n● Done.\n\n  1 to review · 2 to send · 0 to dismiss\n{0}\n❯ \n{0}\n  ⏸ manual mode on · ? for shortcuts\n", rule()));
    rig.wait("the box to be seen again", || rig.status("drafted")["feedback_draft"].is_string());
    std::thread::sleep(Duration::from_millis(900));
    let _ = rig.status("drafted");
    assert_eq!(rig.keys("drafted").len(), before, "the service pressed a key into the box by itself: {:?}", &rig.keys("drafted")[before..]);
    // The box gone and the survey still there: set aside now, as any survey (kept back, it was marked as
    // one that stays and never was. The confirming review).
    let agent = rig.when_typed("drafted", b"0", idle("Done."));
    rig.show("drafted", &format!("● Done.\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n{0}\n❯ \n{0}\n  ⏸ manual mode on · ? for shortcuts\n", rule()));
    rig.wait("the survey to be set aside", || {
        let _ = rig.status("drafted");
        rig.keys("drafted").len() > before
    });
    assert!(agent.join().unwrap(), "the survey's 0 was pressed");
    // (Its own key, once, however slow the stand-in is to draw on a busy computer. This had been
    // loosened to "one to three" when it failed under load: that was the bug, not the load.)
    std::thread::sleep(Duration::from_millis(700));
    rig.typed_since("drafted", before, b"0", "the survey's own 0, once");
    // A question still on the screen above the box: no yes is written through the box, by a list or by hand.
    let before = rig.keys("drafted").len();
    rig.show("drafted", &format!("{}{boxed}", asking("ls", 0)));
    rig.wait("the box to be seen over the question", || rig.status("drafted")["feedback_draft"].is_string());
    let under = rig.status("drafted");
    assert!(under["state"] != "waiting_approval" && under["approval"].is_null(), "a question is offered from under the box: {under}");
    let (code, out, err) = rig.olo(&["approve", "drafted", "--json"]);
    assert!(code != 0, "a yes went through the box: {out}{err}");
    assert_eq!(rig.keys("drafted").len(), before, "a yes was typed into the box: {:?}", &rig.keys("drafted")[before..]);
    // The box's words printed in what the agent wrote are not the box: a message goes through.
    rig.show("drafted", &format!("● The box says:\n● 1 to review · 2 to send · 0 to dismiss\n✻ Crunched for 2s\n{0}\n❯ \n{0}\n  ⏸ manual mode on · ? for shortcuts\n", rule()));
    rig.wait("the box to be gone", || rig.status("drafted")["feedback_draft"].is_null());
    rig.json(&["send", "drafted", "2 more things", "--enter", "--json"]);
    rig.json(&["stop", "drafted", "--force", "--json"]);
}

/// A turn that failed says why, and one command says "continue" to
/// every child of the tab that stands at an error (the ShiftUp lead's
/// item 49, 2026-11-27, after an outage ended every child's turn at
/// once: "the event saying WHY the turn ended (a provider or account
/// error, with its status), and one command that says 'continue' to
/// every child of mine that ended that way").
#[test]
fn a_failed_turn_says_why_and_all_that_failed_are_told_at_once() {
    let Some(rig) = Rig::start("failed") else { return };
    rig.stand_in("claude");
    let failed = |why: &str| format!("● Read(src/parser.rs)\n  ⎿  {why}\n{0}\n❯ \n{0}\n", rule());
    let mut cursors = Vec::new();
    for name in ["one", "two", "well"] {
        cursors.push(rig.start_child("claude", name, &[])["cursor"].clone());
        rig.show(name, &working());
        rig.wait_state(name, "working");
    }
    rig.show("one", &failed("API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}"));
    rig.show("two", &failed("API Error: 401 authentication_error: invalid x-api-key"));
    rig.show("well", &idle("All done."));
    // Each event says why: the error's own words, whose fault, and its status.
    let told = rig.json(&["watch", "one", "--since", &cursors[0].to_string(), "--on", "error", "--timeout", "15", "--json"]);
    assert_eq!((&told["state"], &told["error_kind"], &told["error_status"]), (&json!("error"), &json!("provider"), &json!(529)), "{told}");
    assert!(told["error"].as_str().is_some_and(|e| e.starts_with("API Error: 529")), "{told}");
    let told = rig.json(&["watch", "two", "--since", &cursors[1].to_string(), "--on", "error", "--timeout", "15", "--json"]);
    assert_eq!((&told["error_kind"], &told["error_status"]), (&json!("account"), &json!(401)), "{told}");
    let status = rig.status("one");
    assert_eq!((&status["error_kind"], &status["error_status"]), (&json!("provider"), &json!(529)), "{status}");
    rig.wait_state("well", "idle");
    assert_eq!(rig.status("well")["error_kind"], Value::Null);

    // One message to all that failed: the two, not the one that is well.
    let before: Vec<usize> = ["one", "two", "well"].iter().map(|n| rig.keys(n).len()).collect();
    let (code, _, err) = rig.olo(&["send", "--all-errored"]);
    assert!(code == 2 && err.contains("what to send"), "{err}");
    // (Only a message and its flags: a control or a key is no part of it.)
    for refused in [&["send", "--all-errored", "continue", "--ctrl=c"][..], &["send", "--all-errored", "continue", "--key=Enter"], &["send", "--all-errored", "continue", "--force"]] {
        let (code, _, err) = rig.olo(refused);
        assert!(code == 2 && err.contains("only a message"), "{refused:?}: {err}");
    }
    // (Nor from a pane the service doesn't know, one closed since, in a session it does: it would answer
    // for the tab in front, another lead's where several share the session.)
    let stale = rig
        .helper()
        .args(["agent", "send", "--all-errored", "continue", "--enter"])
        .env("REX_BLOCK", "block:00000000-0000-7000-8000-000000000000")
        .env("REX_SESSION", &rig.session)
        .output()
        .expect("the helper");
    assert!(stale.status.code() == Some(2) && String::from_utf8_lossy(&stale.stderr).contains("isn't certain"), "{stale:?}");
    // (No file is opened for it: a FIFO nobody writes to would hang it.)
    let (code, _, err) = rig.olo(&["send", "--all-errored", "--file", "/proc/self/fd/0", "--enter"]);
    assert!(code == 2 && err.contains("only a message"), "{err}");
    assert_eq!(["one", "two", "well"].iter().map(|n| rig.keys(n).len()).collect::<Vec<_>>(), before, "something was typed by a refused command");
    // (Each takes it up: the test plays the agents.)
    let took: Vec<_> = ["one", "two"].iter().map(|n| rig.when_typed(n, b"go on where you were.", working())).collect();
    let sent = rig.json(&["send", "--all-errored", "--enter", "--json", "--", "The outage is over: go on where you were."]);
    assert!(took.into_iter().all(|t| t.join().unwrap()), "{sent}");
    assert_eq!((&sent["ok"], &sent["count"]), (&json!(true), &json!(2)), "{sent}");
    assert!(sent["sent"].as_array().unwrap().iter().all(|s| s["delivered"] == true), "{sent}");
    assert!(sent["sent"].as_array().unwrap().iter().all(|s| s["was"].as_str().is_some_and(|w| w.starts_with("API Error"))), "{sent}");
    let names: Vec<&str> = sent["sent"].as_array().unwrap().iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(names.contains(&"one") && names.contains(&"two") && !names.contains(&"well"), "{sent}");
    for (i, name) in ["one", "two"].iter().enumerate() {
        let typed = String::from_utf8_lossy(&rig.keys(name)[before[i]..]).into_owned();
        assert!(typed.contains("The outage is over: go on where you were."), "{name} was not told: {typed:?}");
    }
    assert_eq!(rig.keys("well").len(), before[2], "the child that is well was told too");
    // None left standing at an error (they work again): nothing to send, and said so.
    rig.wait_state("one", "working");
    rig.wait_state("two", "working");
    let none = rig.json(&["send", "--all-errored", "Again.", "--enter", "--json"]);
    assert_eq!((&none["ok"], &none["count"]), (&json!(true), &json!(0)), "{none}");
    // One that is sent the message and doesn't take it up (its screen as it was, the words in its
    // field or gone nowhere) is not said to have been told (the confirming review: "bulk still reports
    // unverified submissions as taken").
    rig.show("one", &failed("API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}"));
    rig.wait_state("one", "error");
    let (code, out, _) = rig.olo(&["send", "--all-errored", "Once more.", "--enter", "--json"]);
    let not_taken: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!((code, &not_taken["ok"], &not_taken["count"]), (1, &json!(false), &json!(0)), "{not_taken}");
    assert_eq!(not_taken["sent"][0]["delivered"], false, "{not_taken}");
}

/// Forty children under one tab: started side by side, all there, and
/// listed in well under a second.
#[test]
fn forty_children_are_started_together_and_listed_quickly() {
    let Some(rig) = Rig::start("forty") else { return };
    let rig = &rig;
    std::thread::scope(|scope| {
        for n in 0..40 {
            scope.spawn(move || {
                let name = format!("c{n:02}");
                let (code, _, err) = rig.olo(&["start", "--command", "--name", &name, "--json", "--", "sleep", "300"]);
                assert_eq!(code, 0, "{name}: {err}");
            });
        }
    });
    let began = Instant::now();
    let listed = rig.json(&["list", "--json"]);
    let took = began.elapsed();
    let children = listed["subagents"].as_array().unwrap();
    assert_eq!(children.len(), 40, "none lost to starting together");
    assert!(children.iter().all(|c| c["state"] == "working"));
    eprintln!("forty children listed in {took:?}");
    assert!(took < Duration::from_secs(1), "forty children took {took:?} to list");
    // A name is one child's among the live.
    let (code, _, err) = rig.olo(&["start", "--command", "--name", "c07", "--", "true"]);
    assert_eq!(code, 2, "{err}");
    // The session's view has them all; stopped, it has the lead's alone.
    for child in children {
        rig.json(&["stop", child["id"].as_str().unwrap(), "--force", "--json"]);
    }
    assert_eq!(rig.rex("session.view", json!({ "session_id": rig.session }))["windows"].as_array().unwrap().len(), 1);
}

/// A later build's command has the running service hand over: it ends
/// having written what it knows, the later one's is started, and the
/// children are as they were. And children an earlier Olo's desktop
/// kept are taken over once.
#[test]
fn a_later_build_takes_over_and_kept_children_are_adopted() {
    let Some(rig) = Rig::start("handover") else { return };
    let started = rig.json(&["start", "--command", "--name", "steady", "--json", "--", "sleep", "300"]);
    let first = rig.service_pid().expect("a service");
    let answer = rig.raw(json!({ "proto": 999, "op": "list" }));
    assert_eq!(answer["data"]["handover"], true, "{answer}");
    rig.wait("the first service to end", || !Path::new(&format!("/proc/{first}")).exists());
    // (This build's command starts its own, and the child is there.)
    let listed = rig.json(&["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"]);
    assert_ne!(rig.service_pid(), Some(first));

    // A tab made by an earlier desktop, whose record that desktop kept.
    let window = rig.rex(
        "session.new_window",
        json!({ "session_id": rig.session, "window_label": "old", "focus": false, "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "command": ["sleep", "300"] } } } }),
    );
    let kept = json!({
        "id": "sa-0ld001", "name": "old", "host_key": "far", "session_id": rig.session, "window_id": window["window_id"], "block_id": window["block_ids"][0],
        "parent_window_id": rig.lead_window, "cli": "claude", "model": "claude-opus-5-5", "effort": "max", "title": "old", "started_at": 1_795_000_000_000i64,
        "cli_session_id": "99999999-8888-7777-6666-555555555555",
    });
    let gone = json!({ "id": "sa-0ld002", "name": "older", "host_key": "far", "session_id": "session:gone", "window_id": "window:gone", "block_id": "block:gone", "parent_window_id": "window:x", "cli": "agy", "model": "m", "effort": "high", "title": "older" });
    let mut adopt = rig.helper().args(["agent", "adopt"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    adopt.stdin.take().unwrap().write_all(json!({ "agents": [&kept, &gone] }).to_string().as_bytes()).unwrap();
    let said = adopt.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&said.stdout).contains("Took over 2"), "{said:?}");
    let listed = rig.json(&["list", "--all", "--json"]);
    let by_name = |name: &str| listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name}: {listed}")).clone();
    assert_eq!(by_name("old")["state"], "working", "its tab is there: live");
    assert_eq!(by_name("old")["cli_session_id"], "99999999-8888-7777-6666-555555555555");
    assert_eq!(by_name("older")["state"], "exited", "its tab is gone: kept as exited");
    // (Given again, they are not taken twice.)
    let mut again = rig.helper().args(["agent", "adopt"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    again.stdin.take().unwrap().write_all(json!({ "agents": [&kept, &gone] }).to_string().as_bytes()).unwrap();
    let again_out = again.wait_with_output().unwrap();
    assert!(again_out.status.success() && String::from_utf8_lossy(&again_out.stdout).contains("Took over 0"), "{again_out:?}");
    rig.json(&["stop", "old", "--force", "--json"]);
    rig.json(&["stop", "steady", "--force", "--json"]);
}

/// An upgrade that speaks the same protocol takes over too: a helper put
/// beside the running one on the host, after it, is asked through at
/// once, the children there as before. An asker from before the upgrade
/// (a watcher still waiting) never takes the service back; and a build
/// somewhere else, however new, never takes it at all: a source tree's
/// debug build asked through from a test session was handed a host's
/// live service, 2026-11-26.
#[test]
fn a_helper_put_beside_it_later_takes_over_and_no_other_does() {
    let Some(rig) = Rig::start("upgrade") else { return };
    // A host's helpers, in the folder they are kept in.
    let kept = rig.dir.join("rex-link");
    std::fs::create_dir_all(&kept).unwrap();
    let (first_build, later_build) = (kept.join("olo-rex-link-0001-x86_64"), kept.join("olo-rex-link-0002-x86_64"));
    std::fs::copy(HELPER, &first_build).unwrap();
    let ask = |helper: &Path, args: &[&str]| -> Value {
        let mut cmd = Command::new(helper);
        cmd.current_dir(rig.dir.join("work")).env("PWD", rig.dir.join("work"));
        Rig::scratch_env(&rig.dir, &rig.sock, &mut cmd);
        // (From the lead's pane: named after the scratch environment, which takes away whatever pane the test
        // itself runs in. Named before it, this asked from no pane at all, and the child went under the lead's
        // tab only because that was the tab in front in the one session there.)
        cmd.env("REX_BLOCK", &rig.lead_block);
        let out = run_just_written(cmd.arg("agent").args(args));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let pid = |helper: &Path| ask(helper, &["ping"])["pid"].as_i64().unwrap() as i32;
    let started = ask(&first_build, &["start", "--command", "--name", "steady", "--json", "--", "sleep", "300"]);
    let first = pid(&first_build);
    assert_eq!(std::fs::read_link(format!("/proc/{first}/exe")).unwrap(), first_build);
    // The same build asking again changes nothing; nor does another
    // program asking, however new its file; nor the helper from
    // somewhere else (the build this test was made with, in its tree).
    assert_eq!(pid(&first_build), first);
    let raw = rig.raw(json!({ "proto": 1, "op": "ping" }));
    assert_eq!(raw["data"]["pid"], json!(first), "answered, not handed over: {raw}");
    let elsewhere = rig.dir.join("tree").join("olo-rex-link");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::copy(HELPER, &elsewhere).unwrap();
    assert_eq!(pid(&elsewhere), first, "a build that isn't beside it is answered, and takes nothing");
    assert_eq!(rig.service_pid(), Some(first));

    // "An upgrade": the helper again, beside the first, written now.
    std::fs::copy(HELPER, &later_build).unwrap();
    // Held (the account's hold on this host's pieces, beside the
    // services' folder): the later build is answered by the service
    // that runs, which stays; and it isn't retired by one who doesn't
    // say they know of the hold.
    let hold = rig.dir.join("host-upgrade.hold");
    std::fs::write(&hold, "Held by this test:\nagents are at work here.\n").unwrap();
    let listed = ask(&later_build, &["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"], "answered while held: {listed}");
    assert_eq!(pid(&later_build), first, "held: the later build was handed the service");
    let retire = |more: &[&str]| {
        let mut cmd = Command::new(&later_build);
        Rig::scratch_env(&rig.dir, &rig.sock, &mut cmd);
        let out = run_just_written(cmd.args(["agent", "retire"]).args(more));
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (code, said) = retire(&[]);
    assert!(code == 69 && said.contains("is held at the build it runs") && said.contains("Held by this test: agents are at work here.") && said.contains("--even-if-held"), "{code}: {said}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pid(&first_build), first, "held: it was retired all the same");
    assert!(
        std::fs::read_to_string(rig.dir.join("agents").read_dir().unwrap().flatten().next().unwrap().path().join("service.log")).unwrap().contains("not handed the service: held"),
        "said once in its log"
    );
    // The hold lifted: the next asking of the later build takes over.
    std::fs::remove_file(&hold).unwrap();
    let listed = ask(&later_build, &["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"], "the child is there, asked of the later build: {listed}");
    rig.wait("the first service to end", || !Path::new(&format!("/proc/{first}")).exists());
    let second = pid(&later_build);
    assert_ne!(second, first);
    assert_eq!(std::fs::read_link(format!("/proc/{second}/exe")).unwrap(), later_build, "the service is the later build's");

    // The earlier build asking (a watcher from before): answered by the later one, which stays.
    let listed = ask(&first_build, &["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"]);
    assert_eq!(pid(&later_build), second, "never handed back to an earlier build");
    // (Held again: one who says they know of the hold retires it.)
    std::fs::write(&hold, "Held again.\n").unwrap();
    assert_eq!(retire(&["--even-if-held"]).0, 0);
    rig.wait("the service retired by one who knows of the hold to end", || !Path::new(&format!("/proc/{second}")).exists());
    std::fs::remove_file(&hold).unwrap();
    ask(&later_build, &["stop", "steady", "--force", "--json"]);
}

/// A handover is one step, whoever else asks meanwhile: the service
/// that retires starts its successor itself, its lock handed down, so
/// no asker of an older build can start theirs in the gap. On a host
/// with thirty watchers begun under earlier builds a handover took two
/// minutes and six services, of three builds in turn (2026-11-27,
/// 06:15 UTC: each time the service retired, whoever asked first
/// started a service of their own build).
#[test]
fn a_handover_is_one_step_whoever_else_asks() {
    let Some(rig) = Rig::start("onestep") else { return };
    let kept = rig.dir.join("rex-link");
    std::fs::create_dir_all(&kept).unwrap();
    let build = |n: u32| kept.join(format!("olo-rex-link-000{n}-x86_64"));
    std::fs::copy(HELPER, build(1)).unwrap();
    let asking = |helper: &Path, args: &[&str]| {
        let mut cmd = Command::new(helper);
        cmd.current_dir(rig.dir.join("work")).env("PWD", rig.dir.join("work"));
        Rig::scratch_env(&rig.dir, &rig.sock, &mut cmd);
        cmd.env("REX_BLOCK", &rig.lead_block);
        cmd.arg("agent").args(args);
        cmd
    };
    let ask = |helper: &Path, args: &[&str]| -> Value {
        let out = run_just_written(&mut asking(helper, args));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let pid = |helper: &Path| ask(helper, &["ping"])["pid"].as_i64().unwrap() as i32;
    let runs = |pid: i32| std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    let served = || rig.said().lines().filter(|l| l.contains("olo agents: serving (pid")).map(str::to_owned).collect::<Vec<_>>();
    let started = ask(&build(1), &["start", "--command", "--name", "steady", "--json", "--", "sleep", "300"]);
    let first = pid(&build(1));
    assert_eq!(runs(first), build(1));

    // The host's other askers, of the builds before: each asks without
    // pause for as long as the test goes on, and starts its own build's
    // service whenever none answers (as a watch does).
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let askers = |helper: PathBuf, n: usize| -> Vec<std::thread::JoinHandle<u32>> {
        (0..n)
            .map(|_| {
                let (stop, mut cmd) = (stop.clone(), asking(&helper, &["list", "--fast", "--json"]));
                std::thread::spawn(move || {
                    let mut answered = 0;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        answered += u32::from(cmd.output().is_ok_and(|out| out.status.success()));
                    }
                    answered
                })
            })
            .collect()
    };
    let mut old = askers(build(1), 6);

    // 1. A later build asks (nothing holds the service): its helper's
    // service is the next one, and the only next one.
    std::fs::copy(HELPER, build(2)).unwrap();
    let listed = ask(&build(2), &["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"], "the child is there, asked of the later build: {listed}");
    rig.wait("the first service to end", || !Path::new(&format!("/proc/{first}")).exists());
    let second = pid(&build(2));
    assert_eq!(runs(second), build(2), "the later build's service");
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(pid(&build(2)), second, "and it stays, the older builds still asking");
    let lines = served();
    assert!(lines.len() == 2 && lines[1].contains(&format!("pid {second}, olo-rex-link-0002")), "one service after the first, the later build's: {lines:#?}");
    assert!(rig.said().contains("its successor is started"), "{}", rig.said());

    // 2. Held, and retired by one who knows of the hold, to a helper
    // named: the same, with askers of both builds before it.
    old.extend(askers(build(2), 3));
    std::fs::copy(HELPER, build(3)).unwrap();
    let hold = rig.dir.join("host-upgrade.hold");
    std::fs::write(&hold, "Held by this test.\n").unwrap();
    assert_eq!(pid(&build(3)), second, "held: answered by the one that runs");
    let retire = |more: &[&str]| {
        let out = run_just_written(asking(&build(3), &["retire"]).args(more));
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    // (What takes over is a helper of the account's own, by its whole path: nothing else is started.)
    for not_one in ["/bin/sh", "olo-rex-link-0003-x86_64", "/nowhere/olo-rex-link-0003-x86_64"] {
        let (code, said) = retire(&["--even-if-held", "--to", not_one]);
        assert!(code == 2 && said.contains("--to"), "{not_one}: {code}: {said}");
    }
    let (code, said) = retire(&["--even-if-held", "--to"]);
    assert!(code == 2 && said.contains("--to requires"), "{code}: {said}");
    assert_eq!(pid(&build(2)), second, "refused: it stays");
    let (code, said) = retire(&["--even-if-held", "--to", build(3).to_str().unwrap()]);
    assert_eq!(code, 0, "{said}");
    rig.wait("the second service to end", || !Path::new(&format!("/proc/{second}")).exists());
    rig.wait("the successor service to listen", || rig.service_pid().is_some_and(|p| p != second));
    let third = pid(&build(3));
    assert_eq!(runs(third), build(3), "the service of the helper it was retired to");
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(pid(&build(3)), third);
    let lines = served();
    assert!(lines.len() == 3 && lines[2].contains(&format!("pid {third}, olo-rex-link-0003")), "one service after the second, the named helper's: {lines:#?}");
    std::fs::remove_file(&hold).unwrap();

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let answered: Vec<u32> = old.into_iter().map(|a| a.join().unwrap()).collect();
    assert!(answered.iter().all(|n| *n > 0), "every asker of an older build was answered meanwhile: {answered:?}");
    let listed = ask(&build(3), &["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"], "the child, through both handovers: {listed}");
    ask(&build(3), &["stop", "steady", "--force", "--json"]);
}

/// A service from before services started their successors (update 6
/// and earlier) is taken over in one step by the upgrade tool, however
/// many askers of its build ask meanwhile: a child of the tool sleeps
/// on the service's lock and becomes the new helper's service
/// (`tools/dev/host-upgrade`, `take_over`). Tried with such a helper
/// where one is named (`OLO_TEST_OLD_HELPER`: a file of a release from
/// before 2026-11-27, 06:30 UTC; a host keeps them in
/// `~/.local/share/olo/rex-link`); passed over where none is, as this
/// tree's helper is not one.
#[test]
fn a_service_from_before_is_taken_over_in_one_step() {
    let Some(old) = std::env::var_os("OLO_TEST_OLD_HELPER").map(PathBuf::from) else {
        eprintln!("passed over: no helper from before named in OLO_TEST_OLD_HELPER");
        return;
    };
    the_tool_hands_it_over("takeover", &old, true);
}

/// The upgrade tool, the whole of it as a release runs it (a release's
/// work folder, a host's own folders: the helper put, the skill, what
/// agents are told, the service, its last line), hands a service that
/// starts its own successor to the new helper's in one step (`retire
/// --to`, asked by the tool), with the same askers of the build before.
/// (The whole tool: its summary's last line failed on a variable after a
/// handover that had gone well, 2026-11-27, and only its service step
/// was tried here.)
#[test]
fn the_upgrade_tool_hands_a_service_over_in_one_step() {
    the_tool_hands_it_over("toolstep", Path::new(HELPER), false);
}

/// `tools/dev/host-upgrade` against a held service of the helper `old`,
/// twenty watches and six askers of that build going on: one service
/// after it, the new helper's. From before: the service's step alone
/// (`--service-only NEW`). Else the whole tool, a release's work folder
/// made for it (this tree's sources, this build's helper).
fn the_tool_hands_it_over(name: &str, old: &Path, from_before: bool) {
    let Some(rig) = Rig::start(name) else { return };
    // (A host's helpers where a host keeps them: the tool puts the new one beside.)
    let kept = rig.dir.join("home/.local/share/olo/rex-link");
    std::fs::create_dir_all(&kept).unwrap();
    let build = |n: u32| kept.join(format!("olo-rex-link-000{n}-x86_64"));
    std::fs::copy(old, build(1)).unwrap();
    let asking = |helper: &Path, args: &[&str]| {
        let mut cmd = Command::new(helper);
        cmd.current_dir(rig.dir.join("work")).env("PWD", rig.dir.join("work"));
        Rig::scratch_env(&rig.dir, &rig.sock, &mut cmd);
        cmd.env("REX_BLOCK", &rig.lead_block);
        cmd.arg("agent").args(args);
        cmd
    };
    let ask = |helper: &Path, args: &[&str]| -> Value {
        let out = run_just_written(&mut asking(helper, args));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let pid = |helper: &Path| ask(helper, &["ping"])["pid"].as_i64().unwrap() as i32;
    let runs = |pid: i32| std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    let started = ask(&build(1), &["start", "--command", "--name", "steady", "--json", "--", "sleep", "300"]);
    let first = pid(&build(1));
    assert_eq!(runs(first), build(1));
    assert_eq!(ask(&build(1), &["ping"])["hands_over"].is_null(), from_before, "whether the first helper is one from before");

    // The host as it was: a hold, twenty watches that go on and six askers without pause, all of the old build.
    std::fs::write(rig.dir.join("host-upgrade.hold"), "Held by this test.\n").unwrap();
    let mut watches: Vec<Child> = (0..20).map(|_| asking(&build(1), &["watch", "--all", "--follow"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()).collect();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let askers: Vec<_> = (0..6)
        .map(|_| {
            let (stop, mut cmd) = (stop.clone(), asking(&build(1), &["list", "--fast", "--json"]));
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = cmd.output();
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));

    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut tool = Command::new("/usr/bin/python3");
    tool.arg(tree.join("tools/dev/host-upgrade"));
    if from_before {
        // The new helper, put beside it; the tool, asked for the service alone.
        std::fs::copy(HELPER, build(2)).unwrap();
        tool.arg("--service-only").arg(build(2));
    } else {
        // A release's work folder: its source (this tree) and what its Rex carries (this build's helper).
        let work = rig.dir.join("release");
        let out = work.join("target/release/build/olo-rex/0000/out");
        std::fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(tree.canonicalize().unwrap(), work.join("src")).unwrap();
        std::fs::copy(HELPER, out.join("olo-rex-link-x86_64")).unwrap();
        std::fs::copy(HELPER, out.join("olo-rex-link-aarch64")).unwrap();
        // (A host that has the skill: the tool keeps it current.)
        let skill = rig.dir.join("home/.claude/skills/olo-rex-bridge");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "an older skill\n").unwrap();
        tool.arg("--work").arg(&work);
    }
    tool.arg("--even-if-held").env_remove("OLO_RELEASE_WORK").env_remove("OLO_NO_HOST_UPGRADE");
    Rig::scratch_env(&rig.dir, &rig.sock, &mut tool);
    let began = Instant::now();
    let out = tool.output().unwrap();
    let took = began.elapsed();
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    std::thread::sleep(Duration::from_millis(700));
    let served: Vec<String> = rig.said().lines().filter(|l| l.contains("olo agents: serving (pid")).map(str::to_owned).collect();
    eprintln!("the tool, {took:.1?}: {}\nserved:\n{}", said.trim(), served.join("\n"));
    assert!(out.status.success() && said.contains("the agents service (pid") && said.contains("in one step"), "{said}\n{}\n{}", String::from_utf8_lossy(&out.stderr), rig.said());
    let last = said.trim().lines().last().unwrap_or_default();
    let second = pid(&build(1));
    let new = runs(second);
    if from_before {
        assert!(last.starts_with("host-upgrade: done: the agents service (pid"), "{said}");
        assert_eq!(new, build(2), "the new helper's service");
    } else {
        // Its whole run: each thing put, said; its last line the summary, with the service that runs.
        for put in ["put the helper olo-rex-link-", "put the skill's SKILL.md", "put what agents here are told (", "put the agents service (pid"] {
            assert!(said.contains(put), "`{put}`: {said}");
        }
        assert!(
            last.starts_with("host-upgrade: done: this host has release") && last.ends_with(&format!("the agents service is pid {second}")),
            "its last line: {last}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(new.parent() == Some(kept.as_path()) && new != build(1), "the service is the helper the tool put: {}", new.display());
        assert_ne!(std::fs::read_to_string(rig.dir.join("home/.claude/skills/olo-rex-bridge/SKILL.md")).unwrap(), "an older skill\n");
        assert!(std::fs::read_to_string(rig.dir.join("home/.claude/CLAUDE.md")).unwrap().contains("olo:rex-host:begin"), "what agents are told, in the scratch home");
    }
    assert_eq!(pid(&new), second, "and it stays, the old build's askers answered by it");
    // Between the first and the new one: only what the tool says got in (none, but for a few times in a hundred).
    let between = served.iter().skip(1).filter(|l| l.contains("olo-rex-link-0001")).count();
    let told = if said.contains("but for") { said.split("but for ").nth(1).and_then(|t| t.split(' ').next()).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0) } else { 0 };
    assert_eq!(between, told, "every service in between is one the tool says it retired: {served:#?}\n{said}");
    assert!(served.last().unwrap().contains(&format!("pid {second}, {}", new.file_name().unwrap().to_str().unwrap())), "{served:#?}");

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for a in askers {
        let _ = a.join();
    }
    for w in &mut watches {
        assert!(w.try_wait().unwrap().is_none(), "a watch of the old build went on through it");
        let _ = w.kill();
        let _ = w.wait();
    }
    std::fs::remove_file(rig.dir.join("host-upgrade.hold")).unwrap();
    let listed = ask(&new, &["list", "--json"]);
    assert_eq!(listed["subagents"][0]["id"], started["id"], "the child, through it: {listed}");
    ask(&new, &["stop", "steady", "--force", "--json"]);
}

/// A signal that ends the service is in its log with its sender: which
/// signal, the sender's pid and what it is. (The log said "told to end"
/// and no more; one night the service was ended so and its sender could
/// not be found afterwards: 2026-11-27, 05:01:08 UTC.) And a service a
/// long-lived command started is collected by it when it ends: a
/// follower had two that had ended and were never collected.
#[test]
fn a_signal_that_ends_it_is_logged_with_its_sender() {
    let Some(rig) = Rig::start("signalled") else { return };
    // A follower starts the service, and goes on.
    let mut follower = rig.helper().args(["agent", "watch", "--all", "--follow"]).env("REX_BLOCK", &rig.lead_block).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    rig.wait("a service", || rig.service_pid().is_some());
    rig.json(&["start", "--command", "--name", "steady", "--json", "--", "sleep", "300"]);
    // (Only a service of this test's own is ever signalled: its command line names the rig's own server.)
    let own = |pid: i32| {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        assert!(cmdline.contains(rig.sock.to_str().unwrap()) && cmdline.trim_end().ends_with("serve"), "not this test's service: {cmdline}");
        pid
    };
    let me = std::process::id();
    for (signal, name) in [(libc::SIGTERM, "SIGTERM"), (libc::SIGINT, "SIGINT")] {
        let pid = own(rig.service_pid().unwrap());
        // SAFETY: a signal to a process just seen to be this test's own service.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
        // (Gone, not left ended and uncollected under the follower that started it.)
        rig.wait("the service to end and be collected", || !Path::new(&format!("/proc/{pid}")).exists());
        let said = rig.said();
        let line = said.lines().find(|l| l.contains(&format!("told to end by {name} from pid {me} "))).unwrap_or_else(|| panic!("{name}: {said}"));
        // SAFETY: asks who this process is.
        assert!(line.contains(&format!("(uid {})", unsafe { libc::geteuid() })), "{line}");
        let this = std::env::current_exe().unwrap();
        assert!(line.contains(this.file_name().unwrap().to_str().unwrap()), "what the sender is: {line}");
        assert!(said.contains("ending (told to end)"), "{said}");
        // The next asking starts another, the child with it.
        rig.wait("another service", || rig.service_pid().is_some_and(|next| next != pid));
        assert_eq!(rig.json(&["list", "--json"])["subagents"][0]["name"], "steady");
    }
    assert!(follower.try_wait().unwrap().is_none(), "the follower goes on through both");
    let _ = follower.kill();
    let _ = follower.wait();
    rig.json(&["stop", "steady", "--force", "--json"]);
}

/// A registry that is there and can't be read is a failure, said with
/// its file's name: never an empty list, which would say of every child
/// that it isn't. It is left as it is.
#[test]
fn a_registry_that_cant_be_read_is_said_not_emptied() {
    let Some(rig) = Rig::start("damaged") else { return };
    rig.json(&["start", "--command", "--name", "steady", "--json", "--", "sleep", "300"]);
    let registry = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path().join("registry.json")).find(|p| p.is_file()).expect("its registry");
    // The service gone, and its registry damaged (half a write, another version's).
    let pid = rig.service_pid().unwrap();
    rig.helper().args(["agent", "retire"]).output().unwrap();
    rig.wait("the service to end", || !Path::new(&format!("/proc/{pid}")).exists());
    let was = std::fs::read_to_string(&registry).unwrap();
    std::fs::write(&registry, &was[..was.len() / 2]).unwrap();
    let (code, out, err) = rig.olo(&["list", "--json"]);
    assert_eq!(code, 69, "{out}\n{err}");
    assert!(err.contains("registry") && err.contains("can't be read"), "{err}");
    assert!(!out.contains("\"subagents\":[]"), "never an empty list: {out}");
    assert_eq!(std::fs::read_to_string(&registry).unwrap(), &was[..was.len() / 2], "left as it is");
    // Put right, it is read again, the child with it.
    std::fs::write(&registry, &was).unwrap();
    let listed = rig.json(&["list", "--json"]);
    assert_eq!(listed["subagents"][0]["name"], "steady");
    rig.json(&["stop", "steady", "--force", "--json"]);
}

/// A key its bytes can't say is pressed as the keyboard would press it:
/// through the server's own encoder, by what the program asked of its
/// keys. Ctrl+Enter to a program on the kitty keyboard protocol (Claude
/// Code pushes its flags at its start) is `CSI 13;5u`; to one that
/// asked nothing, xterm's `CSI 27;5;13~`; a plain Return, Alt with a
/// letter and Escape are their bytes either way.
#[test]
fn a_key_is_pressed_as_the_keyboard_would_press_it() {
    let Some(rig) = Rig::start("keys") else { return };
    rig.stand_in("claude");
    rig.start_child("claude", "typist", &[]);
    let pressed = |key: &str| -> Vec<u8> {
        let before = rig.keys("typist").len();
        rig.json(&["send", "typist", "--key", key, "--json"]);
        rig.wait(&format!("the key {key}"), || rig.keys("typist").len() > before);
        // (All of it: a sequence comes in one write, read a moment after its first byte.)
        std::thread::sleep(Duration::from_millis(80));
        rig.keys("typist")[before..].to_vec()
    };
    // A program that asked nothing of its keys.
    assert_eq!(pressed("Enter"), b"\r");
    assert_eq!(pressed("Ctrl+Enter"), b"\x1b[27;5;13~");
    assert_eq!(pressed("Alt+m"), b"\x1bm");
    // It asks for the kitty keyboard protocol, as Claude Code does.
    rig.show("typist", "\x1b[>1u ready for keys\n");
    rig.wait("its screen", || rig.olo(&["read", "typist"]).1.contains("ready for keys"));
    assert_eq!(pressed("Ctrl+Enter"), b"\x1b[13;5u");
    assert_eq!(pressed("Shift+Enter"), b"\x1b[13;2u");
    assert_eq!(pressed("C-S-Enter"), b"\x1b[13;6u");
    assert_eq!(pressed("Ctrl+Tab"), b"\x1b[9;5u");
    assert_eq!(pressed("Enter"), b"\r");
    assert_eq!(pressed("Alt+m"), b"\x1bm");
    assert_eq!(pressed("Escape"), b"\x1b");
    assert_eq!(pressed("Ctrl+Shift+Up"), b"\x1b[1;6A");
    // A character by itself is that one key: no Return after it, and not
    // pasted (a program that takes pastes would get it between its
    // paste marks, as it gets `send`'s text).
    rig.show("typist", "\x1b[?2004h takes pastes\n");
    rig.wait("its screen", || rig.olo(&["read", "typist"]).1.contains("takes pastes"));
    assert_eq!(pressed("0"), b"0");
    assert_eq!(pressed("y"), b"y");
    assert_eq!(pressed("Shift+y"), b"Y");
    // Into a question it is an answer by its number: refused as text
    // is, with nothing reaching the program, unless it is meant.
    rig.show("typist", &asking("rm -rf build", 0));
    rig.wait("its question", || rig.json(&["status", "typist", "--json"])["state"] == "waiting_approval");
    let before = rig.keys("typist").len();
    let (code, out, err) = rig.olo(&["send", "typist", "--key", "2", "--json"]);
    assert_eq!(code, 1, "a character key into a question: {out}{err}");
    assert!(format!("{out}{err}").contains("refusing to press a character key") && format!("{out}{err}").contains("--force"), "{out}{err}");
    assert_eq!(rig.keys("typist").len(), before, "the refused key reached it");
    // (A key that types nothing still goes, as before.)
    assert_eq!(pressed("Down"), b"\x1b[B");
    let before = rig.keys("typist").len();
    rig.json(&["send", "typist", "--force", "--key", "2", "--json"]);
    rig.wait("the forced key", || rig.keys("typist").len() > before);
    assert_eq!(rig.keys("typist")[before..], *b"2");
    rig.json(&["stop", "typist", "--force", "--json"]);
}

/// What a watcher is and isn't told: two questions one after the other
/// are two events; a cursor from before the log began again is said so;
/// text that was already on the screen isn't the text waited for; a
/// grandchild's own end reaches whoever watches the tab above it.
#[test]
fn a_watcher_is_told_each_thing_once_and_nothing_that_was_there() {
    let Some(rig) = Rig::start("watch") else { return };
    rig.stand_in("claude");
    let started = rig.start_child("claude", "alpha", &[]);
    let mut cursor = started["cursor"].as_u64().unwrap();
    let watch = |since: u64, on: &str| rig.json(&["watch", "--all", "--since", &since.to_string(), "--on", on, "--timeout", "15", "--json"]);

    // A question, then another in its place with nothing between.
    rig.show("alpha", &asking("rm -rf build", 0));
    let first = watch(cursor, "approval");
    assert!(first["approval"]["command"].as_str().unwrap().contains("rm -rf build"), "{first}");
    cursor = first["cursor"].as_u64().unwrap();
    rig.show("alpha", &asking("git push --force", 0));
    let second = watch(cursor, "approval");
    assert!(second["approval"]["command"].as_str().unwrap().contains("git push --force"), "the second question is its own event: {second}");
    assert!(second["cursor"].as_u64().unwrap() > cursor);
    cursor = second["cursor"].as_u64().unwrap();
    // (Shown again as it is, it is not said again.)
    rig.show("alpha", &asking("git push --force", 0));
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &cursor.to_string(), "--on", "approval", "--timeout", "2", "--json"]);
    assert_eq!(code, 124, "{out}");
    assert_eq!(serde_json::from_str::<Value>(out.trim()).unwrap()["event"], "timeout");

    // A cursor past the last event: said, at once.
    let reset = rig.json(&["watch", "--all", "--since", "999999", "--timeout", "3", "--json"]);
    assert_eq!(reset["event"], "reset", "{reset}");
    assert_eq!(reset["cursor"], cursor);

    // Text already there isn't waited for; text that comes is.
    let (code, out, _) = rig.olo(&["watch", "alpha", "--match", "git push", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "what stood on the screen matched: {out}");
    let seen = std::thread::scope(|scope| {
        let waiting = scope.spawn(|| rig.json(&["watch", "alpha", "--match", "ALL TESTS PASS", "--timeout", "15", "--json"]));
        std::thread::sleep(Duration::from_millis(400));
        rig.show("alpha", &idle("ALL TESTS PASS (212)"));
        waiting.join().unwrap()
    });
    assert_eq!(seen["event"], "match", "{seen}");
    assert!(seen["matched_line"].as_str().unwrap().contains("ALL TESTS PASS (212)"), "{seen}");
    let (code, _, err) = rig.olo(&["watch", "alpha", "--regex", "(unclosed", "--timeout", "1"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = rig.olo(&["watch", "alpha", "--on", "nonsense"]);
    assert_eq!(code, 2, "{err}");

    // A grandchild ends by itself: the lead, watching its own tab, is told.
    let inner = rig.start_child_from(started["block_id"].as_str(), "claude", "grand", &[]);
    assert_eq!(inner["parent_window_id"], started["window_id"]);
    let cursor = rig.json(&["list", "--json"])["cursor"].as_u64().unwrap();
    rig.tell("grand", "exit 42");
    let ended = watch(cursor, "exit");
    assert_eq!((&ended["id"], &ended["event"], &ended["summary"]), (&inner["id"], &json!("exited"), &json!("exited (code 42)")), "{ended}");
    assert_eq!(ended["error"], "process exited with code 42");

    // What can't both be asked.
    let (code, _, err) = rig.olo(&["stop", "alpha", "--force", "--keep-tab"]);
    assert_eq!(code, 2, "{err}");
    // An end is said once, however it is learnt (the stop itself, the
    // server's word that the tab closed, the screen that is no more).
    let before = ended["cursor"].as_u64().unwrap();
    rig.json(&["stop", "alpha", "--tree", "--force", "--json"]);
    let mut ends: Vec<String> = Vec::new();
    let mut since = before;
    loop {
        let (code, out, _) = rig.olo(&["watch", "--all", "--since", &since.to_string(), "--on", "exit", "--timeout", "2", "--json"]);
        let said: Value = serde_json::from_str(out.trim()).unwrap();
        if code == 124 {
            break;
        }
        ends.push(said["name"].as_str().unwrap_or_default().to_owned());
        since = said["cursor"].as_u64().unwrap();
    }
    assert_eq!(ends, ["alpha"], "one end for the one child that was left");
}

/// A question is answered by what a choice means, wherever it stands:
/// Claude Code's read outside the working directories has its yes for
/// this once last and a kept yes under the mark; Antigravity's yes for
/// this conversation is asked for by its kind (`--session`); and a no
/// is never the no that is kept.
#[test]
fn a_question_is_answered_by_what_a_choice_means() {
    let Some(rig) = Rig::start("meaning") else { return };
    rig.stand_in("claude");
    rig.stand_in("agy");
    let read_outside = |marked: usize| {
        let mark = |i: usize| if i == marked { "❯" } else { " " };
        format!(
            " Read(/var/tmp/other/notes.md)\n Allow this read?\n {} 1. Yes, and keep allowing any reads outside the working directories\n {} 2. No, and block reads\n {} 3. No, and ask again next time\n {} 4. Yes, but ask again next time\n Esc to cancel\n",
            mark(0),
            mark(1),
            mark(2),
            mark(3)
        )
    };
    let reader = rig.start_child("claude", "reader", &[]);
    rig.show("reader", &read_outside(0));
    let asked = rig.json(&["watch", "reader", "--since", &reader["cursor"].to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    let kinds: Vec<&str> = asked["approval"]["choices"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["dont_ask_again", "no_always", "no", "once"], "{asked}");

    // Yes, once: down to the fourth (the mark is seen there before Return), never Return on the first.
    let before = rig.keys("reader").len();
    let moved = rig.when_typed("reader", b"\x1b[B\x1b[B\x1b[B", read_outside(3));
    let gone = rig.when_typed("reader", b"\r", working());
    let approved = rig.json(&["approve", "reader", "--once", "--json"]);
    assert!(moved.join().unwrap() && gone.join().unwrap());
    assert_eq!((&approved["chosen_label"], &approved["dismissed"]), (&json!("Yes, but ask again next time"), &json!(true)), "{approved}");
    rig.typed_since("reader", before, b"\x1b[B\x1b[B\x1b[B\r", "what was typed into it");

    // No, once: the third, not the no that blocks from now on.
    rig.show("reader", &read_outside(0));
    rig.wait_state("reader", "waiting_approval");
    let before = rig.keys("reader").len();
    let moved = rig.when_typed("reader", b"\x1b[B\x1b[B", read_outside(2));
    let gone = rig.when_typed("reader", b"\r", idle("Not read."));
    let refused = rig.json(&["approve", "reader", "--no", "--json"]);
    assert!(moved.join().unwrap() && gone.join().unwrap());
    assert_eq!(refused["chosen_label"], "No, and ask again next time", "{refused}");
    rig.typed_since("reader", before, b"\x1b[B\x1b[B\r", "what was typed into it");
    // (Where the mark doesn't come to stand on it, abort cleanly WITHOUT sending Escape or Return.)
    rig.show("reader", &read_outside(0));
    rig.wait_state("reader", "waiting_approval");
    let before = rig.keys("reader").len();
    let (code, _, err) = rig.olo(&["approve", "reader", "--once", "--json"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("without pressing Enter or Escape"), "{err}");
    rig.typed_since("reader", before, b"\x1b[B\x1b[B\x1b[B", "never Return or Escape where the mark wasn't seen to land");
    // There is no yes for the session here: said, nothing typed.
    let before = rig.keys("reader").len();
    let (code, _, err) = rig.olo(&["approve", "reader", "--session"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no safe `allow_session` choice"), "{err}");
    assert_eq!(rig.keys("reader").len(), before);

    // Antigravity's: for this conversation, by its kind; never the one kept in settings.
    let agy_asks = |marked: usize| {
        let mark = |i: usize| if i == marked { ">" } else { " " };
        format!(
            "Command\n────────────────────\nRequesting permission for:\n   cargo test -p api\nRun this command?\n{} 1. Yes, run command\n{} 2. Yes, and always allow in this conversation\n{} 3. Yes, and always allow for commands that start with 'cargo' (Persist to settings.json)\n{} 4. No, cancel\n  ↑/↓ Navigate\n",
            mark(0),
            mark(1),
            mark(2),
            mark(3)
        )
    };
    let walker = rig.start_child("agy", "walker", &[]);
    rig.show("walker", &agy_asks(0));
    let asked = rig.json(&["watch", "walker", "--since", &walker["cursor"].to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    let kinds: Vec<&str> = asked["approval"]["choices"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["once", "allow_session", "dont_ask_again", "no"], "{asked}");
    let before = rig.keys("walker").len();
    let moved = rig.when_typed("walker", b"\x1b[B", agy_asks(1));
    let gone = rig.when_typed("walker", b"\r", "● Ran (cargo test -p api)\n\n────────────────────\n> \n? for shortcuts        accept-edits · gemini-4-neon\n".to_owned());
    let allowed = rig.json(&["approve", "walker", "--session", "--json"]);
    assert!(moved.join().unwrap() && gone.join().unwrap());
    assert_eq!((&allowed["chosen_kind"], &allowed["chosen_label"]), (&json!("allow_session"), &json!("Yes, and always allow in this conversation")), "{allowed}");
    rig.typed_since("walker", before, b"\x1b[B\r", "what was typed into it");
    rig.json(&["stop", "reader", "--force", "--json"]);
    rig.json(&["stop", "walker", "--force", "--json"]);
}

/// What a child is started with reaches it: each `--env` set, each
/// `--unset-env` and the desktop's own unset, in the child's own
/// environment (ShiftUp's launcher reads it there before it trusts a
/// tab: one of its variables is the guard against an agent starting
/// agents).
#[test]
fn what_a_child_is_started_with_is_in_its_environment() {
    let Some(rig) = Rig::start("env") else { return };
    let started = rig.json(&[
        "start", "--command", "--name", "guarded", "--json", "--env", "SHIFTUP_NO_AGENTS=1", "--env", "WITH_SPACES=two words = kept", "--unset-env", "XDG_STATE_HOME", "--", "sh", "-c",
        "echo \"guard=[$SHIFTUP_NO_AGENTS] spaces=[$WITH_SPACES] state=[${XDG_STATE_HOME-unset}] session=[${REX_SESSION-unset}] display=[${WAYLAND_DISPLAY-unset}] block=[${REX_BLOCK:+set}]\"; sleep 300",
    ]);
    let line = "guard=[1] spaces=[two words = kept] state=[unset] session=[unset] display=[unset] block=[set]";
    rig.wait("its line", || rig.json(&["read", "guarded", "--json"])["output"].as_str().unwrap().contains("guard=["));
    let read = rig.json(&["read", "guarded", "--json"]);
    assert!(read["output"].as_str().unwrap().replace('\n', "").contains(line), "{read}");
    // (And as the launcher reads it: from the process itself.)
    // (Its process is the `env` it was started through for a moment,
    // then the program's own, the same number: read once it is that.)
    let pid = rig.status("guarded")["pid"].as_i64().unwrap();
    rig.wait("the child's program to be its own", || {
        let own = |p: i64| std::fs::read(format!("/proc/{p}/environ")).unwrap_or_default().split(|b| *b == 0).any(|v| v == b"SHIFTUP_NO_AGENTS=1");
        own(pid) || std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default().split_whitespace().filter_map(|c| c.parse::<i64>().ok()).any(own)
    });
    let mut seen = false;
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(child) = entry.file_name().to_str().and_then(|n| n.parse::<i64>().ok()) else { continue };
        let stat = std::fs::read_to_string(format!("/proc/{child}/stat")).unwrap_or_default();
        let parent = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)).and_then(|p| p.parse::<i64>().ok());
        if child == pid || parent == Some(pid) {
            let environ = std::fs::read(format!("/proc/{child}/environ")).unwrap_or_default();
            let vars: Vec<&[u8]> = environ.split(|b| *b == 0).collect();
            if vars.contains(&&b"SHIFTUP_NO_AGENTS=1"[..]) {
                seen = true;
                assert!(!vars.iter().any(|v| v.starts_with(b"REX_SESSION=") || v.starts_with(b"XDG_STATE_HOME=")), "what was to be unset is set");
            }
        }
    }
    assert!(seen, "the guard is in the child's own environment (pid {pid})");
    assert_eq!(started["desktop"], "none");
    // A name that isn't a variable's is refused before anything starts.
    let (code, _, err) = rig.olo(&["start", "--command", "--env", "BAD NAME=1", "--", "true"]);
    assert_eq!(code, 2, "{err}");
    rig.json(&["stop", "guarded", "--force", "--json"]);
}

/// Runs a program this test has just written (a copy of the helper).
/// Run at once it can be refused as "busy" (`ETXTBSY`): another test's
/// thread that started a program in the same instant carried this
/// file, still open for writing, into its child until that child became
/// its own program. It is free a moment later: asked again, for a
/// second at most. (It failed one run in ten, 2026-11-26.)
fn run_just_written(cmd: &mut Command) -> std::process::Output {
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        match cmd.output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
            out => return out.expect("the helper"),
        }
    }
}

/// The minder's side of a test: its folder in the service's own, its
/// command there, and a command run in its tab (as its agent would run
/// it: the service knows it by its process).
struct Minding<'a> {
    rig: &'a Rig,
    /// The stand-in's name (`STAND_IN_NAME`), which a test gives it.
    stand_in: String,
    dir: PathBuf,
}

impl<'a> Minding<'a> {
    /// The fifo a minder's stand-in will listen on, made before it starts.
    fn expect(rig: &Rig, stand_in: &str) {
        assert!(Command::new("mkfifo").arg(rig.staged(stand_in, "fifo")).status().unwrap().success());
    }

    fn of(rig: &'a Rig, stand_in: &str, dir_id: &str) -> Self {
        rig.wait("the minder's stand-in to listen", || rig.staged(stand_in, "ready").exists());
        let service = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("minders").is_dir()).expect("the service's folder has its minders'");
        Self { rig, stand_in: stand_in.to_owned(), dir: service.join("minders").join(dir_id) }
    }

    fn rules_of(&self, agent_id: &str) -> PathBuf {
        self.dir.parent().unwrap().join(agent_id).join("rules.md")
    }

    fn env(&self) -> String {
        format!("OLO_REX_AGENTS_DIR={} REX_SERVER_SOCKET={}", self.rig.dir.join("agents").display(), self.rig.sock.display())
    }

    /// A command run in the minder's tab: what it printed.
    fn run(&self, command: &str) -> String {
        let _ = std::fs::remove_file(self.rig.staged(&self.stand_in, "ran"));
        self.rig.tell(&self.stand_in, &format!("run {command}"));
        self.rig.wait("the command in the minder's tab", || self.rig.staged(&self.stand_in, "ran").exists());
        std::fs::read_to_string(self.rig.staged(&self.stand_in, "out")).unwrap()
    }

    /// Its own command (`mind …`), answered in JSON.
    fn mind(&self, args: &str) -> Value {
        let out = self.run(&format!("{} {} {args}", self.env(), self.dir.join("mind").display()));
        serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("mind {args}: {e}\n{out}\n{}", self.rig.said()))
    }

    /// The service's command asked directly from its tab (what it has no leave for): what was said, with the status.
    fn olo(&self, args: &str) -> String {
        self.run(&format!("{} {HELPER} agent {args}; echo status=$?", self.env()))
    }
}

/// A shared minder (`agents::minder`): started before its agent as a
/// child tab of the lead, with its agent nested under it; it alone is
/// told what its agent asks, and answers it; it reaches its agents and
/// nothing else; what it escalates is on the parent's own watch; and
/// its report once its last agent has ended is its last word.
#[test]
fn a_minder_answers_for_its_agent_and_tells_the_parent() {
    let Some(rig) = Rig::start("minder") else { return };
    rig.stand_in("claude");
    let other = rig.start_child("claude", "other", &[]);
    Minding::expect(&rig, "minds-worker");
    let rules = "Approve `cargo test`.\nRefuse any push, and say to open a pull request instead.";
    let started = rig.start_child("claude", "worker", &["--minder", "claude:stand-in-big:max", "--rules-text", rules, "--minder-env", "STAND_IN_NAME=minds-worker"]);
    let worker_id = started["id"].as_str().unwrap();
    let minder_id = started["minder"]["id"].as_str().unwrap();
    let minder = Minding::of(&rig, "minds-worker", minder_id);
    assert_eq!(
        (&started["minder"]["name"], &started["minder"]["cli"], &started["minder"]["model"], &started["minder"]["effort"], &started["minder"]["reused"]),
        (&json!("minder"), &json!("claude"), &json!("stand-in-big"), &json!("max"), &json!(false)),
        "{started}"
    );

    // What it is: a child tab of the lead (`role: "minder"`, `minds: null`), with `worker` nested under it (`parent_window_id == minder.window_id`, `minded_by == minder.id`); `worker`'s rules in `minders/WORKER_ID/rules.md` and the minder's command in `minders/MINDER_ID/mind`.
    let listed = rig.json(&["list", "--all", "--json"]);
    let row = |name: &str| listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name} isn't listed: {listed}")).clone();
    assert_eq!((&row("minder")["parent_window_id"], &row("minder")["minds"], &row("minder")["role"]), (&json!(rig.lead_window), &Value::Null, &json!("minder")), "{listed}");
    assert_eq!((&row("worker")["parent_window_id"], &row("worker")["minded_by"], &row("worker")["minder"]), (&started["minder"]["window_id"], &json!(minder_id), &json!(minder_id)), "{listed}");
    assert_eq!((&row("other")["parent_window_id"], &row("other")["minder"]), (&json!(rig.lead_window), &Value::Null), "{listed}");
    assert_eq!(std::fs::read_to_string(minder.rules_of(worker_id)).unwrap(), rules);
    assert_eq!(std::fs::metadata(minder.dir.join("mind")).unwrap().permissions().mode() & 0o777, 0o700);
    // What it was started with: the shell and the three tools that read; its command the one thing allowed in the shell, reading allowed, nothing asked; shared-minder instructions in its system prompt.
    let line = minder.run("tr '\\0' '\\n' < /proc/$PPID/cmdline");
    let args: Vec<&str> = line.lines().collect();
    let after = |flag: &str| args.iter().position(|a| *a == flag).and_then(|i| args.get(i + 1)).copied().unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    assert_eq!((after("--model"), after("--effort"), after("--permission-mode"), after("--tools")), ("stand-in-big", "max", "dontAsk", "Bash,Read,Grep,Glob"), "{args:?}");
    let allowed = args.iter().position(|a| *a == "--allowed-tools").map(|i| &args[i + 1..i + 5]).unwrap_or_else(|| panic!("no --allowed-tools in {args:?}"));
    assert_eq!(allowed, ["Read", "Grep", "Glob", &format!("Bash({} *)", minder.dir.join("mind").display())], "{args:?}");
    assert!(args.contains(&"--strict-mcp-config"), "{args:?}");
    let told = line.split("--append-system-prompt\n").nth(1).unwrap_or_default();
    assert!(told.starts_with("You are a minder. Your job is to watch over the agents that run as child tabs under yours.") && told.contains("list --json"), "{told}");
    assert!(args.last().unwrap().starts_with("Begin minding now. Run: ") && args.last().unwrap().contains("watch --since "), "{args:?}");

    // `mind list --json` shows strictly its own minded agents with their `what`, `cwd` and `rules` path (never `other`).
    let mlist = minder.mind("list --json");
    let msubs = mlist["subagents"].as_array().unwrap();
    assert_eq!(msubs.len(), 1, "minder sees only its minded agents: {mlist}");
    assert_eq!((&msubs[0]["name"], &msubs[0]["rules"]), (&json!("worker"), &json!(minder.rules_of(worker_id).display().to_string())), "{mlist}");
    assert!(msubs[0]["what"].as_str().is_some_and(|w| w.contains("Claude Code") && w.contains(&rig.dir.join("work").display().to_string())), "{mlist}");

    // The minder hears `minding` (`added`) for `worker` naming its rules file, and the lead's watch does NOT.
    let added = minder.mind("watch --since 0 --on minding,question,approval,idle,error,exit --timeout 15 --json");
    assert_eq!(
        (&added["event"], &added["name"], &added["minding"]["action"], &added["minding"]["rules"]),
        (&json!("minding"), &json!("worker"), &json!("added"), &json!(minder.rules_of(worker_id).display().to_string())),
        "{added}"
    );
    let mut cursor = added["cursor"].as_u64().unwrap();

    // The agent asks leave. Its minder is told; the parent's watch is not woken by it (nor by `minding`).
    rig.show("worker", &asking("cargo test", 0));
    let asked = minder.mind(&format!("watch --since {cursor} --on minding,question,approval,idle,error,exit --timeout 15 --json"));
    assert_eq!((&asked["name"], &asked["event"]), (&json!("worker"), &json!("waiting_approval")), "{asked}");
    assert!(asked["approval"]["command"].as_str().unwrap().contains("cargo test"), "{asked}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", "0", "--on", "approval,question,escalation,minding", "--timeout", "2", "--json"]);
    assert_eq!(code, 124, "the parent was woken by what is its minder's to answer or by minding: {out}");
    let (code, out, _) = rig.olo(&["watch", "worker", "--on", "approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "…nor by how it stands now: {out}");
    // (The parent can still see it, and answer it itself, if it looks.)
    assert_eq!(rig.status("worker")["state"], "waiting_approval");

    // The minder says yes, once: typed into the agent as the parent's own yes is.
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = minder.mind("approve worker --once --json");
    assert!(gone.join().unwrap(), "{approved}");
    assert_eq!((&approved["chosen_label"], &approved["dismissed"]), (&json!("Yes"), &json!(true)), "{approved}");
    rig.typed_since("worker", before, b"\r", "what was typed into it");
    cursor = asked["cursor"].as_u64().unwrap();

    // What isn't a minder's: any other agent, and anything but listing, watching, reading, answering and escalating, whatever pane it says it is.
    for refused in [
        "stop worker --force",
        "start --command -- true",
        "prune",
        "mind worker --off",
        "focus worker",
        "status other",
        "read other",
        "send other hello",
        "approve other --once",
        "watch other --timeout 1",
    ] {
        for as_lead in [false, true] {
            let said = if as_lead { minder.run(&format!("REX_BLOCK={} {} {HELPER} agent {refused}; echo status=$?", rig.lead_block, minder.env())) } else { minder.olo(refused) };
            assert!(!said.contains("status=0") && !said.contains("status=124"), "a minder did `{refused}`: {said}");
        }
    }
    assert!(minder.olo("stop worker --force").contains("status=69"));
    // (Nor a yes that is kept, even for its own agent.)
    assert!(minder.olo("approve worker --session").contains("status=69"));
    // (`list --all` from a minder even with spoofed REX_BLOCK shows only its own agents, never `other`.)
    let spoofed_list = minder.run(&format!("REX_BLOCK={} {} {HELPER} agent list --all --json", rig.lead_block, minder.env()));
    assert!(!spoofed_list.contains(other["id"].as_str().unwrap()), "minder saw another child of the lead: {spoofed_list}");
    assert_eq!((&rig.status("worker")["running"], &rig.status("other")["running"]), (&json!(true), &json!(true)));
    assert_eq!(other["name"], "other");

    // What the rules don't cover goes up: an event on the parent's own watch, about the agent, in the minder's words.
    rig.show("worker", &asking("git push origin main", 0));
    let asked = minder.mind(&format!("watch --since {cursor} --on minding,question,approval,idle,error,exit --timeout 15 --json"));
    assert!(asked["approval"]["command"].as_str().unwrap().contains("git push"), "{asked}");
    let lead_from = cursor;
    cursor = asked["cursor"].as_u64().unwrap();
    let before = rig.keys("worker").len();
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("git push origin main", 2));
    let gone = rig.when_typed("worker", b"\r", working());
    let refused = minder.mind("approve worker --no --reason 'No pushes: open a pull request instead.' --json");
    assert!(moved.join().unwrap() && gone.join().unwrap(), "{refused}");
    assert_eq!(refused["chosen_label"], "No", "{refused}");
    assert!(rig.keys("worker")[before..].starts_with(b"\x1b[B\x1b[B"), "down to No, never Return on Yes: {:?}", String::from_utf8_lossy(&rig.keys("worker")[before..]));
    // (Its words whole, as a shell gives them: nothing in them is run.)
    let said = "worker asked to push to main ($HOME, \"quoted\"). Refused by your rule; it was told to open a pull request.";
    let up = minder.mind(&format!("escalate worker '{said}' --json"));
    assert_eq!((&up["event"], &up["name"], &up["id"]), (&json!("escalation"), &json!("worker"), &json!(worker_id)), "{up}");
    let heard = rig.json(&["watch", "--all", "--since", &lead_from.to_string(), "--timeout", "15", "--json"]);
    assert_eq!(
        (&heard["event"], &heard["name"], &heard["summary"], &heard["cursor"]),
        (&json!("escalation"), &json!("worker"), &json!(said), &up["cursor"]),
        "the first thing the parent hears since before the question: {heard}"
    );
    // (And `mind escalate "TEXT"` without an agent name escalates about the minder itself.)
    let self_msg = "Minder self-check: running low on context.";
    let self_up = minder.mind(&format!("escalate '{self_msg}' --json"));
    assert_eq!((&self_up["event"], &self_up["name"], &self_up["id"]), (&json!("escalation"), &json!("minder"), &json!(minder_id)), "{self_up}");
    let self_heard = rig.json(&["watch", "--all", "--since", &heard["cursor"].to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&self_heard["event"], &self_heard["name"], &self_heard["summary"]), (&json!("escalation"), &json!("minder"), &json!(self_msg)), "{self_heard}");
    // (Once: the next watch from there has nothing; and a watch for other things alone isn't woken by it.)
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &self_heard["cursor"].to_string(), "--on", "escalation,approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &lead_from.to_string(), "--on", "approval,question", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");

    // Asked of: who minds it, by what rules, and what was answered, in order.
    let of = rig.json(&["mind", "worker", "--json"]);
    assert_eq!((&of["minder"]["id"], &of["rules"]), (&json!(minder_id), &json!(rules)), "{of}");
    let did: Vec<&str> = of["answered"].as_array().unwrap().iter().filter_map(|a| a["did"].as_str()).collect();
    assert_eq!(did, ["approve", "approve", "escalate"], "{of}");
    // (Each with the minder that gave it.)
    assert!(of["answered"].as_array().unwrap().iter().all(|a| a["by"] == minder_id), "{of}");
    assert_eq!(of["answered"][2]["said"], said);
    let (code, out, _) = rig.olo(&["mind", "worker"]);
    assert!(code == 0 && out.starts_with("worker is minded by minder (claude:stand-in-big:max)\n") && out.contains("escalate"), "{out}");

    // The agent ends (last agent of this minder). Its minder hears it and reports; that is its last word: it is stopped at once, well before its time is up.
    rig.tell("worker", "exit 0");
    let ended = minder.mind(&format!("watch --since {cursor} --on minding,question,approval,idle,error,exit --timeout 15 --json"));
    assert_eq!((&ended["name"], &ended["state"]), (&json!("worker"), &json!("exited")), "{ended}");
    let reported = Instant::now();
    let report = minder.mind("escalate worker 'worker has ended. Approved: cargo test. Refused: a push.' --json");
    let heard = rig.json(&["watch", "--all", "--since", &ended["cursor"].to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&heard["event"], &heard["state"], &heard["cursor"]), (&json!("escalation"), &json!("exited"), &report["cursor"]), "{heard}");
    rig.wait("the minder to be stopped after its report", || rig.status(minder_id)["running"] == false);
    assert!(reported.elapsed() < Duration::from_millis(2500), "stopped by its report, not by its time running out: {:?}", reported.elapsed());
    // (Its end isn't news: nothing more is said of it to the parent.)
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &heard["cursor"].to_string(), "--on", "escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
}

/// A minder for an agent that runs already (`olo agent mind`): attached
/// (reparented under the minder), its rules changed in place without
/// restarting the minder, taken off (reparented back under the lead);
/// one that ends by itself is said to the parent, whose own the agent's
/// asking is again; and an empty minder doesn't outlive its last agent.
#[test]
fn a_minder_is_attached_changed_and_taken_off() {
    let Some(rig) = Rig::start("minding") else { return };
    rig.stand_in("claude");
    let env = |name: &str| -> Vec<String> {
        vec![
            "--env".into(),
            format!("STAND_IN_DIR={}", rig.dir.join("stage").display()),
            "--env".into(),
            format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display()),
            "--env".into(),
            format!("STAND_IN_NAME={name}"),
        ]
    };
    let mind = |args: &[&str], stand_in: &str| -> Value {
        Minding::expect(&rig, stand_in);
        let env = env(stand_in);
        let all: Vec<&str> = ["mind"].into_iter().chain(args.iter().copied()).chain(env.iter().map(String::as_str)).chain(["--json"]).collect();
        rig.json(&all)
    };

    // Rules with no minder to follow them: said, and nothing started.
    let (code, _, err) = rig.olo(&["start", "claude", "--model", "m", "--effort", "e", "--name", "nobody", "--rules-text", "Approve everything."]);
    assert!(code == 2 && err.contains("--minder"), "{err}");
    let (code, _, err) = rig.olo(&["start", "claude", "--model", "m", "--effort", "e", "--name", "nobody", "--minder", "claude:m:e", "--rules", "no-such-file.md"]);
    assert!(code != 0 && err.contains("no-such-file.md"), "{err}");
    assert!(rig.json(&["list", "--all", "--json"])["subagents"].as_array().unwrap().is_empty());

    let started = rig.start_child("claude", "worker", &[]);
    let worker_id = started["id"].as_str().unwrap().to_owned();
    let (code, _, err) = rig.olo(&["mind", "worker", "--rules-text", "Approve cargo test."]);
    assert!(code == 2 && err.contains("--with"), "rules for a minder it doesn't have: {err}");
    let (code, _, err) = rig.olo(&["mind", "worker", "--off"]);
    assert!(code == 1 && err.contains("no minder"), "{err}");
    assert_eq!(rig.json(&["mind", "worker", "--json"])["minder"], Value::Null);

    // Attached to it as it runs, the rules from a file (read where it was asked): `worker` moves under `minder`.
    std::fs::write(rig.dir.join("work/rules.md"), "Approve `cargo test`.\n").unwrap();
    let first = mind(&["worker", "--with", "claude:stand-in-big:max", "--rules", "rules.md"], "m1");
    let m1_id = first["minder"]["id"].as_str().unwrap().to_owned();
    let m1 = Minding::of(&rig, "m1", &m1_id);
    assert_eq!((&first["minder"]["name"], &first["replaced"], &first["parent_window_id"]), (&json!("minder"), &Value::Null, &first["minder"]["window_id"]), "{first}");
    assert_eq!(std::fs::read_to_string(m1.rules_of(&worker_id)).unwrap(), "Approve `cargo test`.\n");
    let from = first["cursor"].as_u64().unwrap();
    let added = m1.mind(&format!("watch --since {from} --on minding,question,approval --timeout 15 --json"));
    assert_eq!((&added["event"], &added["minding"]["action"], &added["name"]), (&json!("minding"), &json!("added"), &json!("worker")), "{added}");
    rig.show("worker", &asking("cargo test", 0));
    let asked = m1.mind(&format!("watch --since {} --timeout 15 --json", added["cursor"]));
    assert_eq!(asked["event"], "waiting_approval", "{asked}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &from.to_string(), "--on", "approval,escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");

    // New rules: rewritten and sent to the SAME running minder via a `minding` (`rules`) event — NO restart!
    let second = rig.json(&["mind", "worker", "--rules-text", "Approve nothing.", "--json"]);
    assert_eq!(
        (&second["replaced"], &second["minder"]["id"], &second["minder"]["model"], &second["minder"]["effort"]),
        (&Value::Null, &json!(m1_id), &json!("stand-in-big"), &json!("max")),
        "{second}"
    );
    assert_eq!(std::fs::read_to_string(m1.rules_of(&worker_id)).unwrap(), "Approve nothing.");
    let r_ev = m1.mind(&format!("watch --since {} --on minding,question,approval --timeout 15 --json", second["cursor"]));
    assert_eq!((&r_ev["event"], &r_ev["minding"]["action"], &r_ev["minding"]["rules"]), (&json!("minding"), &json!("rules"), &json!(m1.rules_of(&worker_id).display().to_string())), "{r_ev}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &from.to_string(), "--on", "approval,escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "updating rules woke the parent: {out}");
    // (And the running minder can still inspect `worker`.)
    assert_eq!(m1.mind("status worker --json")["state"], "waiting_approval");

    // A minder that ends by itself, its agent still running: `worker` moves back under the lead, the parent is told, and what the agent asks is the parent's again.
    rig.tell("m1", "exit 3");
    let heard = rig.json(&["watch", "--all", "--since", &from.to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&heard["event"], &heard["name"]), (&json!("escalation"), &json!("worker")), "{heard}");
    // (How it ended is said as it was first heard: its program's status, or that its tab closed, whichever word came first.)
    let how = heard["summary"].as_str().unwrap();
    assert!(how.starts_with("Its minder ended (exited (code 3))") || how.starts_with("Its minder ended (its tab was closed)"), "{heard}");
    assert!(how.ends_with("what it asks comes to you again."), "{heard}");
    assert_eq!(rig.json(&["mind", "worker", "--json"])["minder"], Value::Null);
    assert_eq!(rig.status("worker")["parent_window_id"], json!(rig.lead_window));
    let now_mine = rig.json(&["watch", "--all", "--since", &from.to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    assert!(now_mine["approval"]["command"].as_str().unwrap().contains("cargo test"), "{now_mine}");

    // Taken off (`--off`): `worker` moves back under the lead, minder receives `minding` (`removed`), nothing escalated to lead, asking is the parent's again, and the empty minder ends after its timeout.
    let third = mind(&["worker", "--with", "claude:stand-in-big:high"], "m3");
    let m3_id = third["minder"]["id"].as_str().unwrap().to_owned();
    let m3 = Minding::of(&rig, "m3", &m3_id);
    assert_eq!(std::fs::read_to_string(m3.rules_of(&worker_id)).unwrap(), "Approve nothing.", "no rules given: those it had");
    let _ = m3.mind(&format!("watch --since {} --on minding --timeout 15 --json", third["cursor"]));
    let (code, out, _) = rig.olo(&["watch", "worker", "--on", "approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
    let off = rig.json(&["mind", "worker", "--off", "--json"]);
    assert_eq!((&off["detached_from"], &off["minder"], &off["parent_window_id"]), (&json!(m3_id), &Value::Null, &json!(rig.lead_window)), "{off}");
    assert_eq!(rig.status("worker")["parent_window_id"], json!(rig.lead_window));
    let rem = m3.mind("watch --since 0 --on minding --timeout 15 --json");
    assert_eq!((&rem["event"], &rem["minding"]["action"], &rem["name"]), (&json!("minding"), &json!("removed"), &json!("worker")), "{rem}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &heard["cursor"].to_string(), "--on", "escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
    assert_eq!(rig.json(&["watch", "worker", "--on", "approval", "--timeout", "15", "--json"])["state"], "waiting_approval");
    rig.wait("the empty minder taken off to end after its time", || rig.status(&m3_id)["running"] == false);

    // A minder whose last agent has ended and that says nothing: stopped when its time is up, not left minding nothing.
    let fourth = mind(&["worker", "--with", "claude:stand-in-big:max"], "m4");
    let fourth_id = fourth["minder"]["id"].as_str().unwrap().to_owned();
    let _m4 = Minding::of(&rig, "m4", &fourth_id);
    let ended = Instant::now();
    rig.tell("worker", "exit 0");
    rig.wait_state("worker", "exited");
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(rig.status(&fourth_id)["running"], true, "it has three seconds to report");
    rig.wait("the minder of an agent that ended to be stopped", || rig.status(&fourth_id)["running"] == false);
    assert!(ended.elapsed() > Duration::from_millis(2800), "{:?}", ended.elapsed());
    // (And an agent that has ended takes no minder.)
    let (code, _, err) = rig.olo(&["mind", "worker", "--with", "claude:m:e"]);
    assert!(code == 1 && err.contains("exited"), "{err}");
}

/// A minder that fell behind (a real one takes seconds to judge, an agent
/// asks again in less): it hears what stands, not each thing that passed;
/// and its yes is for the question it was told of, never for one that
/// came since.
#[test]
fn a_minder_that_fell_behind_answers_only_what_stands() {
    let Some(rig) = Rig::start("behind") else { return };
    rig.stand_in("claude");
    Minding::expect(&rig, "m");
    let started = rig.start_child("claude", "worker", &["--minder", "claude:stand-in-big:max", "--rules-text", "Approve cargo test.", "--minder-env", "STAND_IN_NAME=m"]);
    let minder = Minding::of(&rig, "m", started["minder"]["id"].as_str().unwrap());
    let from = started["cursor"].as_u64().unwrap();
    let cursor = || rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    // (A screen shown and written down as the agent's state.)
    let shown = |screen: &str| {
        let before = cursor();
        rig.show("worker", screen);
        rig.wait("the agent's new state to be written down", || cursor() > before);
        cursor()
    };
    let watch =
        |since: u64, on: &str, secs: u32| minder.run(&format!("{} {} watch --since {since} --on {on} --timeout {secs} --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    let event = |said: &str| -> Value { serde_json::from_str(said.lines().next().unwrap_or_default()).unwrap_or_else(|e| panic!("{e}: {said}")) };

    // Two questions, one after the other, while the minder looks away: it is told the one that stands.
    shown(&asking("cargo test", 0));
    let push = shown(&asking("git push origin main", 0));
    let told = event(&watch(from, "question,approval,idle,error,exit", 15));
    assert!(told["approval"]["command"].as_str().unwrap().contains("git push"), "told a question that had passed: {told}");
    assert_eq!(told["cursor"], push);

    // While it judges that one, another takes its place. Its yes is not a yes to that: nothing is typed.
    shown(&asking("rm -rf build", 0));
    let before = rig.keys("worker").len();
    let said = minder.run(&format!("{} {} approve worker --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(said.contains("isn't what you were last told of") && said.contains("status=1"), "{said}");
    assert_eq!(rig.keys("worker").len(), before, "something was typed into the agent");
    assert_eq!(rig.status("worker")["state"], "waiting_approval");
    // It looks again, is told what stands asked, and answers that.
    let stands = minder.mind("status worker --json");
    assert!(stands["approval"]["command"].as_str().unwrap().contains("rm -rf build"), "{stands}");
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("rm -rf build", 2));
    let gone = rig.when_typed("worker", b"\r", working());
    let refused = minder.mind("approve worker --no --reason 'Not in the rules.' --json");
    assert!(moved.join().unwrap() && gone.join().unwrap());
    assert_eq!((&refused["chosen_label"], &refused["dismissed"]), (&json!("No"), &json!(true)), "{refused}");

    // What stands is rest: the questions that passed are not told to one that waits for questions…
    let rested = shown(&idle("Done with the build."));
    assert!(watch(push, "approval,question", 1).contains("status=124"));
    // …and one that waits for anything hears the rest alone, then nothing more of what was before it.
    let told = event(&watch(push, "question,approval,idle,error,exit", 15));
    assert_eq!((&told["state"], &told["cursor"]), (&json!("idle"), &json!(rested)), "{told}");
    assert!(watch(rested, "question,approval,idle,error,exit", 1).contains("status=124"));

    // The parent, whose watch it also is, still hears each thing in turn once the minder is off: nothing was dropped from the log.
    rig.json(&["mind", "worker", "--off", "--json"]);
    let first = rig.json(&["watch", "worker", "--since", &from.to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    assert!(first["approval"]["command"].as_str().unwrap().contains("cargo test"), "{first}");
    let second = rig.json(&["watch", "worker", "--since", &first["cursor"].to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    assert!(second["approval"]["command"].as_str().unwrap().contains("git push"), "{second}");
}

/// Which turn a rest belongs to (the ShiftUp lead's items 27 and 34, 2026-11-26: "tell no turn's end until
/// the prompt's own turn has ended"; "A turn's end event … carries the cursor or id of the `send` it answers.
/// Then a launcher can tell a turn's reply from an earlier turn's without comparing times"). A child started
/// with nothing to do is at rest once it is at its own prompt, and not before; a child given a message is not
/// told at rest again until it has taken that up, and its turn's end says which message it answers.
/// A Codex in a turn is not at rest, whatever its screen reads: its
/// conversation's file says a task is started and not complete (the
/// ShiftUp lead's items 50 and 51, 2026-11-27: "`olo agent run codex`
/// returns at Codex's first message as if the turn had ended"; "a Codex
/// tab that is working is listed as `idle`"). And a file left saying so
/// by a turn that never ended is believed only while the agent shows
/// life.
#[test]
fn a_codex_in_a_turn_by_its_file_is_not_at_rest() {
    let Some(rig) = Rig::start("codexturn") else { return };
    rig.stand_in("codex");
    // (Codex's own record of a conversation: `sessions/Y/M/D/rollout-…-ID.jsonl` under its home.)
    let rollout = |id: &str| {
        let dir = rig.dir.join("home/.codex/sessions/2026/11/27");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("rollout-2026-11-27T10-00-00-{id}.jsonl"))
    };
    let add = |file: &Path, records: &[Value]| {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file).unwrap();
        f.write_all(records.iter().map(|r| format!("{r}\n")).collect::<String>().as_bytes()).unwrap();
    };
    let said =
        |text: &str, phase: &str| json!({ "type": "response_item", "payload": { "type": "message", "role": "assistant", "phase": phase, "content": [{ "type": "output_text", "text": text }] } });
    // (`at`: the time its file gives the turn's beginning. One long past is of the conversation's life before
    // this child; one to come stands for "since this child was made".)
    let (long_ago, in_this_life) = ("2020-01-01T00:00:00.000Z", "2099-01-01T00:00:00.000Z");
    let begun = |asked: &str, at: &str| {
        [
            json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "task_started" } }),
            json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": asked }] } }),
        ]
    };

    // 1. One Codex for one prompt (`run`). Its first words, then a step, then its last: `run` answers with the last.
    const TURN: &str = "5f0c1f0e-2222-4a22-8a22-2f8f0e0a0002";
    let file = rollout(TURN);
    add(&file, &[json!({ "type": "session_meta", "payload": { "id": TURN, "cwd": rig.work() } })]);
    assert!(Command::new("mkfifo").arg(rig.staged("reads", "fifo")).status().unwrap().success());
    let (stage, path, named) = (format!("STAND_IN_DIR={}", rig.dir.join("stage").display()), format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display()), "STAND_IN_NAME=reads".to_owned());
    let work = rig.work();
    std::thread::scope(|scope| {
        let done = scope.spawn(|| {
            rig.olo(&[
                "run",
                "codex",
                "--model",
                "stand-in",
                "--effort",
                "none",
                "--name",
                "reads",
                "--cwd",
                &work,
                "--env",
                &stage,
                "--env",
                &named,
                "--env",
                &path,
                "--timeout",
                "40",
                "--prompt",
                "Review it.",
                "--json",
                "--resume",
                TURN,
            ])
        });
        rig.wait("the stand-in to listen", || rig.staged("reads", "ready").exists());
        rig.show("reads", &codex_idle("Ready."));
        // It takes its prompt up: the file says a task is started, the screen says it works.
        add(&file, &begun("Review it.", in_this_life));
        rig.show("reads", &codex_working());
        rig.wait("the service to have seen it at work", || rig.status("reads")["answers"].is_u64());
        let since = rig.status("reads")["cursor"].as_u64().unwrap();
        // Its first words, written and on its screen, and no status line: as its screen stands between two
        // steps. For two seconds and more, more words coming all the while.
        add(&file, &[said("I will read the file first.", "commentary")]);
        for words in 1..=6 {
            rig.show("reads", &codex_idle(&format!("I will read the file first.{}", " And then.".repeat(words))));
            std::thread::sleep(Duration::from_millis(400));
            assert!(!done.is_finished(), "`run` returned at Codex's first words, its turn still on");
        }
        assert_eq!(rig.status("reads")["state"], "working", "listed as at rest within its turn: {}", rig.status("reads"));
        assert_eq!(rig.json(&["list", "--all", "--json"])["subagents"][0]["state"], "working");
        let (code, out, _) = rig.olo(&["watch", "reads", "--since", &since.to_string(), "--on", "idle,done", "--timeout", "1", "--json"]);
        assert_eq!(code, 124, "a rest was told within its turn: {out}");
        // Its turn ends: said in its file, and nothing more written to its screen. Told, with its last words.
        add(&file, &[said("It says forty-two.", "final_answer"), json!({ "type": "event_msg", "payload": { "type": "task_complete" } })]);
        let (code, out, err) = done.join().unwrap();
        assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
        let answer: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!((&answer["ended_as"], &answer["last_message"]), (&json!("done"), &json!("It says forty-two.")), "{answer}");
    });

    // 2. A file that says a task is started, and a Codex that stands still at its prompt and says nothing of
    //    itself. Where the turn is of this child's life (one that never ended): at work for as long as the file
    //    is believed (three seconds here), then at rest, and told so.
    const LEFT: &str = "5f0c1f0e-3333-4a33-8a33-2f8f0e0a0003";
    let left = rollout(LEFT);
    add(&left, &[json!({ "type": "session_meta", "payload": { "id": LEFT, "cwd": rig.work() } })]);
    add(&left, &begun("Something that never ended.", in_this_life));
    let started = rig.start_child("codex", "still", &["--resume", LEFT, "--cwd", &work]);
    let at = Instant::now();
    rig.show("still", &codex_idle("Ready."));
    let (code, out, err) = rig.olo(&["watch", "still", "--since", &started["cursor"].as_u64().unwrap().to_string(), "--on", "idle,done", "--timeout", "20", "--json"]);
    assert_eq!(code, 0, "a Codex that stands still was never told at rest: {out}{err}\n{}", rig.said());
    let told: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(told["state"], "idle", "{told}");
    assert!(at.elapsed() >= Duration::from_millis(2500), "told at rest while its file was still believed: {:?}", at.elapsed());
    assert!(at.elapsed() < Duration::from_secs(12), "{:?}", at.elapsed());
    //    Where the turn is of the conversation's life before this child (killed then; the conversation taken
    //    up again with nothing to do): at rest at once, lively as it is while it starts. (It was "at work" two
    //    and a half seconds after its start: the live suite under load, 2026-11-27.)
    const BEFORE: &str = "5f0c1f0e-6666-4a66-8a66-2f8f0e0a0006";
    let before = rollout(BEFORE);
    add(&before, &[json!({ "type": "session_meta", "payload": { "id": BEFORE, "cwd": rig.work() } })]);
    add(&before, &begun("Something, long ago.", long_ago));
    let started = rig.start_child("codex", "again", &["--resume", BEFORE, "--cwd", &work]);
    let at = Instant::now();
    for words in 1..=3 {
        rig.show("again", &codex_idle(&format!("Ready.{}", " Still drawing.".repeat(words))));
        std::thread::sleep(Duration::from_millis(250));
        assert_ne!(rig.status("again")["state"], "working", "at work by a turn of its life before: {}", rig.status("again"));
    }
    let (code, out, err) = rig.olo(&["watch", "again", "--since", &started["cursor"].as_u64().unwrap().to_string(), "--on", "idle,done", "--timeout", "20", "--json"]);
    assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
    assert!(at.elapsed() < Duration::from_millis(2500), "kept back by a turn of its life before: {:?}", at.elapsed());

    // 3. The same file, and a Codex that says what it is doing itself (OSC 7501, as 0.172 does): its own word
    //    is what is believed, and "idle" is its word too, though it says it by taking its record away. At rest
    //    at once, not when the file's word has run out. (It was "at work" for those seconds: the live suite,
    //    2026-11-27, "resumed-after-kill Codex at rest must NEVER be stuck working".)
    const SPOKE: &str = "5f0c1f0e-5555-4a55-8a55-2f8f0e0a0005";
    let spoke = rollout(SPOKE);
    add(&spoke, &[json!({ "type": "session_meta", "payload": { "id": SPOKE, "cwd": rig.work() } })]);
    add(&spoke, &begun("Something that never ended.", in_this_life));
    let started = rig.start_child("codex", "spoke", &["--resume", SPOKE, "--cwd", &work]);
    // (As Codex starts: "working" for an instant, then "idle".)
    rig.show("spoke", &format!("\x1b]7501;state=working\x07{}", codex_idle("Ready.")));
    rig.wait("the service to have its own word", || rig.status("spoke")["state"] == "working");
    let at = Instant::now();
    rig.show("spoke", &format!("\x1b]7501;state=idle\x07{}", codex_idle("Ready.")));
    let (code, out, err) = rig.olo(&["watch", "spoke", "--since", &started["cursor"].as_u64().unwrap().to_string(), "--on", "idle,done", "--timeout", "20", "--json"]);
    assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
    assert!(at.elapsed() < Duration::from_millis(2000), "a Codex that says it is idle was kept at work by its file: {:?}", at.elapsed());
}

/// A conversation taken up again with a prompt is not done by its old
/// last words: its program writes to the conversation's file as it
/// starts, and the last turn in the file is still the one before. Told
/// by when the file's last turn began (met in the live suite under
/// load, 2026-11-27: "the first rest told of a conversation taken up
/// again with a prompt isn't the prompt's turn's end (after 1.18 s)",
/// with the first conversation's reply as its answer).
#[test]
fn a_conversation_taken_up_again_is_not_done_by_its_old_last_words() {
    let Some(rig) = Rig::start("again") else { return };
    rig.stand_in("claude");
    const AGAIN: &str = "5f0c1f0e-4444-4a44-8a44-2f8f0e0a0004";
    let work = rig.work();
    let folder: String = work.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let dir = rig.dir.join("home/.claude/projects").join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{AGAIN}.jsonl"));
    let add = |records: &[Value]| {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&file).unwrap();
        f.write_all(records.iter().map(|r| format!("{r}\n")).collect::<String>().as_bytes()).unwrap();
    };
    let turn = |at: &str, asked: &str, said: &str| {
        [
            json!({ "type": "user", "timestamp": at, "message": { "role": "user", "content": asked } }),
            json!({ "type": "assistant", "timestamp": at, "message": { "id": format!("msg_{said}"), "role": "assistant", "content": [{ "type": "text", "text": said }], "stop_reason": "end_turn" } }),
        ]
    };
    // As it was left, long ago: a turn, over.
    add(&turn("2020-01-01T00:00:00.000Z", "The first thing?", "The old answer."));
    let started = rig.start_child("claude", "again", &["--resume", AGAIN, "--prompt", "And the second thing?", "--cwd", &work]);
    let from = started["cursor"].as_u64().unwrap().to_string();
    let watch = |secs: &str| rig.olo(&["watch", "again", "--since", &from, "--on", "idle,done", "--timeout", secs, "--json"]);
    rig.show("again", &idle("The old answer."));
    rig.wait("its conversation to be found", || rig.status("again")["last_message"] == "The old answer.");
    // Its program writes to the file as it starts (no turn of it yet).
    add(&[json!({ "type": "system", "subtype": "informational", "timestamp": "2099-01-01T00:00:00.000Z", "content": "Conversation resumed." })]);
    let (code, out, _) = watch("3");
    assert_eq!(code, 124, "told at rest by the old conversation's last words: {out}");
    // Its turn, quicker than any reading of its screen: its message and its answer in the file.
    add(&turn("2099-01-01T00:00:01.000Z", "And the second thing?", "The second answer."));
    rig.show("again", &idle("The second answer."));
    let (code, out, err) = watch("15");
    assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
    let told: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!((&told["last_message"], &told["answers"]), (&json!("The second answer."), &started["cursor"]), "{told}");
}

/// What a conversation's file says of a turn before the one a child was
/// given is not of this turn, and a turn's end that only its file says
/// is told without waiting for its screen. (The confirming code review
/// of the Codex fix, 2026-11-27: its 2, 4 and 6; each fails with its
/// fix taken out.)
#[test]
fn a_turn_before_is_not_this_turn_and_a_files_word_is_looked_for() {
    let Some(rig) = Rig::start("turnbefore") else { return };
    rig.stand_in("codex");
    rig.stand_in("claude");
    let work = rig.work();
    let rollout = |id: &str| {
        let dir = rig.dir.join("home/.codex/sessions/2026/11/27");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("rollout-2026-11-27T10-00-00-{id}.jsonl"))
    };
    let add = |file: &Path, records: &[Value]| {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file).unwrap();
        f.write_all(records.iter().map(|r| format!("{r}\n")).collect::<String>().as_bytes()).unwrap();
    };
    let meta = |id: &str| json!({ "type": "session_meta", "payload": { "id": id, "cwd": rig.work() } });
    let started_at = |at: &str| json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "task_started" } });
    let said = |at: &str, text: &str| json!({ "timestamp": at, "type": "response_item", "payload": { "type": "message", "role": "assistant", "phase": "final_answer", "content": [{ "type": "output_text", "text": text }] } });
    let complete = |at: &str, error: Value| json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "task_complete", "error": error } });
    // (The clock's own time, in the files' form, some milliseconds ago.)
    let ago = |ms: u64| {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64 - ms as i64;
        let (days, of_day) = (at.div_euclid(86_400_000), at.rem_euclid(86_400_000));
        // (A day's number to its date: Howard Hinnant's `civil_from_days`.)
        let z = days + 719_468;
        let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", of_day / 3_600_000, of_day / 60_000 % 60, of_day / 1000 % 60, of_day % 1000)
    };
    assert!(ago(0).starts_with("20") && ago(0).len() == 24, "{}", ago(0));
    let watch = |name: &str, since: &Value, on: &str, secs: &str| rig.olo(&["watch", name, "--since", &since.to_string(), "--on", on, "--timeout", secs, "--json"]);

    // 1. A conversation whose last turn failed, taken up again with a prompt. The old failure is on file and
    //    its prompt is clean: not this turn's error, and nothing is told of it.
    const FAILED: &str = "5f0c1f0e-7777-4a77-8a77-2f8f0e0a0007";
    let file = rollout(FAILED);
    add(&file, &[meta(FAILED), started_at("2020-01-01T00:00:00.000Z"), complete("2020-01-01T00:00:05.000Z", json!({ "message": "stream disconnected" }))]);
    let retry = rig.start_child("codex", "retry", &["--resume", FAILED, "--prompt", "Try again.", "--cwd", &work]);
    rig.show("retry", &codex_idle("Ready."));
    rig.wait("its conversation to be found", || rig.status("retry")["transcript_path"].is_string());
    let (code, out, _) = watch("retry", &retry["cursor"], "error,idle,done", "3");
    assert_eq!(code, 124, "an old turn's failure was told as the new prompt's: {out}");
    assert_ne!(rig.status("retry")["state"], "error", "{}", rig.status("retry"));
    //    Its own turn, quicker than any reading of its screen, and nothing more written to the screen after:
    //    its file says so, and that is looked for (it waited out its two minutes, or for ever).
    let at = Instant::now();
    add(&file, &[started_at(&ago(0)), said(&ago(0), "Done this time."), complete(&ago(0), Value::Null)]);
    let (code, out, err) = watch("retry", &retry["cursor"], "error,idle,done", "15");
    assert_eq!(code, 0, "a turn's end that only its file says was never told: {out}{err}\n{}", rig.said());
    let told: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!((&told["last_message"], &told["answers"]), (&json!("Done this time."), &retry["cursor"]), "{told}");
    assert!(at.elapsed() < Duration::from_secs(6), "told only after {:?}", at.elapsed());

    // 2. A conversation whose last turn began a second before this child was asked for (a quick one, stopped,
    //    and taken up again at once): the turn before, not this one's.
    const QUICK: &str = "5f0c1f0e-8888-4a88-8a88-2f8f0e0a0008";
    let file = rollout(QUICK);
    add(&file, &[meta(QUICK), started_at(&ago(1200)), said(&ago(1100), "The answer before."), complete(&ago(1000), Value::Null)]);
    let next = rig.start_child("codex", "next", &["--resume", QUICK, "--prompt", "And now?", "--cwd", &work]);
    rig.show("next", &codex_idle("The answer before."));
    rig.wait("its conversation to be found", || rig.status("next")["transcript_path"].is_string());
    let (code, out, _) = watch("next", &next["cursor"], "error,idle,done", "3");
    assert_eq!(code, 124, "the turn of a second before was told as this prompt's: {out}");

    // 3. A watch of two children with no cursor: one at rest by its screen and in a turn by its file, another
    //    with a question standing since before the watch. The question is told. (That a child *turned down*
    //    doesn't end the looking at the others needs the first to change between two readings of one asking:
    //    not staged here; the watch goes on to the others by its own loop, `'scan`.)
    const BUSY: &str = "5f0c1f0e-9999-4a99-8a99-2f8f0e0a0009";
    let file = rollout(BUSY);
    add(&file, &[meta(BUSY)]);
    rig.start_child("codex", "busy", &["--resume", BUSY, "--cwd", &work]);
    rig.show("busy", &codex_idle("I will read the file first."));
    rig.wait("its conversation to be found", || rig.status("busy")["transcript_path"].is_string());
    rig.start_child("claude", "asks", &[]);
    rig.show("asks", &asking("cargo test", 0));
    rig.wait_state("asks", "waiting_approval");
    add(&file, &[started_at(&ago(0))]);
    rig.wait("the busy one to be at work by its file", || rig.status("busy")["state"] == "working");
    let (code, out, err) = rig.olo(&["watch", "busy", "asks", "--on", "approval,idle", "--timeout", "2", "--json"]);
    assert_eq!(code, 0, "a question standing before the watch waited behind a child turned down: {out}{err}");
    let told: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!((&told["name"], &told["state"]), (&json!("asks"), &json!("waiting_approval")), "{told}");

    // 4. A Codex whose turn the person cut short: its notice reads as an error on its screen, and it is at
    //    rest by its file. A watch for errors alone is not answered with that rest (the confirming review:
    //    "The watcher returns immediately with `event=idle` and success").
    const CUT: &str = "5f0c1f0e-aaaa-4aaa-8aaa-2f8f0e0a000a";
    let file = rollout(CUT);
    add(&file, &[meta(CUT)]);
    rig.start_child("codex", "cut", &["--resume", CUT, "--cwd", &work]);
    rig.show("cut", "› Do the thing.\n\n■ Conversation interrupted - use /feedback if something went wrong\n\n› Ask Codex to do anything\n\n  GPT-6.2-Astra max · /var/tmp/x · Context 52% left\n  ← for agents · ? for shortcuts\n");
    add(&file, &[started_at(&ago(0)), json!({ "timestamp": ago(0), "type": "event_msg", "payload": { "type": "turn_aborted", "reason": "interrupted" } })]);
    rig.wait("its conversation to be found, and its rest", || {
        let status = rig.status("cut");
        status["transcript_path"].is_string() && status["state"] == "idle"
    });
    let (code, out, _) = rig.olo(&["watch", "cut", "--on", "error", "--timeout", "2", "--json"]);
    assert_eq!(code, 124, "a watch for errors was answered with a rest: {out}");
}

#[test]
fn a_rest_is_the_end_of_the_turn_it_says() {
    let Some(rig) = Rig::start("turns") else { return };
    rig.stand_in("claude");
    let started = rig.start_child("claude", "alpha", &[]);
    let mut from = started["cursor"].as_u64().unwrap();
    let watch = |since: u64, secs: &str| rig.olo(&["watch", "alpha", "--since", &since.to_string(), "--on", "idle,done,question,approval,error,exit", "--timeout", secs, "--json"]);
    let told = |since: u64| -> Value {
        let (code, out, err) = watch(since, "15");
        assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
        serde_json::from_str(out.trim()).unwrap()
    };

    // What stands in its tab before its own screen is drawn (the tab's shell, here as it stood on the day)
    // is not the agent at rest, whatever was read of the tab first.
    rig.show("alpha", "[systemd]\nFailed Units: 1\n  systemd-zram-setup@zram0.service\n");
    // (The rule holds for fifteen seconds from the child's start, `UNDRAWN_MS`. Use the
    // `started_at` from `start_child` directly; if the host was already past 12 s, advance
    // `from` past any valid expired-shell event before checking the real prompt.)
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let since_start = started["started_at"].as_u64().map(|at| Duration::from_millis(now_ms.saturating_sub(at)));
    if since_start.is_none_or(|d| d < Duration::from_secs(12)) {
        let (code, out, _) = watch(from, "2");
        assert_eq!(code, 124, "the tab's shell was told as the agent at rest: {out}");
    } else {
        eprintln!("a_rest_is_the_end_of_the_turn_it_says: {since_start:?} since the child's start: advancing past any expired-shell idle");
        let (code, out, _) = watch(from, "1");
        if code == 0
            && let Ok(ev) = serde_json::from_str::<Value>(out.trim())
            && let Some(c) = ev["cursor"].as_u64()
        {
            from = c;
        }
    }
    // At its own prompt, with nothing to do: at rest, told at once, answering nothing.
    let at = Instant::now();
    rig.show("alpha", &idle("Ready."));
    let up = told(from);
    assert_eq!((&up["state"], &up["summary"], &up["answers"]), (&json!("idle"), &json!("at prompt"), &Value::Null), "{up}");
    assert!(at.elapsed() < Duration::from_secs(5), "told only when the fifteen seconds were up: {:?}", at.elapsed());

    // A message sent to it at rest. It still stands at its prompt for a moment: that is not the message's turn's end.
    let sent = rig.json(&["send", "alpha", "Do the first thing.", "--enter", "--json"]);
    let first = sent["cursor"].as_u64().unwrap();
    rig.show("alpha", &idle("Ready."));
    let (code, out, _) = watch(first, "1");
    assert_eq!(code, 124, "the rest before the message's turn was told: {out}");
    // It takes it up, and its turn ends: that rest answers the message.
    // (Waited for until the service itself has seen it at work: `status` looks at the screen for itself, and
    // sooner than the service's own reading of it on a busy computer.)
    let at_work_on = |cursor: u64| rig.wait("the service to have seen it at work on what it was given", || rig.status("alpha")["answers"] == cursor);
    rig.show("alpha", &working());
    at_work_on(first);
    rig.show("alpha", &idle("The first thing is done."));
    let ended = told(first);
    assert_eq!((&ended["state"], &ended["answers"]), (&json!("idle"), &json!(first)), "{ended}");

    // The next message's turn answers the next message; a question in it is of that turn too.
    let sent = rig.json(&["send", "alpha", "Do the second thing.", "--enter", "--json"]);
    let second = sent["cursor"].as_u64().unwrap();
    assert!(second > first);
    rig.show("alpha", &asking("cargo test", 0));
    let asked = told(second);
    assert_eq!((&asked["state"], &asked["answers"]), (&json!("waiting_approval"), &json!(second)), "{asked}");
    // (A message sent into a turn that is going is that turn's: nothing new is expected of it.)
    rig.show("alpha", &working());
    rig.wait_state("alpha", "working");
    rig.json(&["send", "alpha", "And mind the tests.", "--enter", "--json"]);
    rig.show("alpha", &idle("The second thing is done."));
    let ended = told(asked["cursor"].as_u64().unwrap());
    assert_eq!((&ended["state"], &ended["answers"]), (&json!("idle"), &json!(second)), "{ended}");

    // A message it never takes up: its rest is told when its time is up (2.5 s here), as it stands, answering nothing new.
    let sent = rig.json(&["send", "alpha", "Do the third thing.", "--enter", "--json"]);
    let third = sent["cursor"].as_u64().unwrap();
    let at = Instant::now();
    rig.show("alpha", &idle("The second thing is done."));
    let (code, out, _) = watch(third, "1");
    assert_eq!(code, 124, "{out}");
    let stood = told(third);
    assert!(at.elapsed() >= Duration::from_millis(2300) && at.elapsed() < Duration::from_secs(8), "told when its time was up: {:?}", at.elapsed());
    assert_eq!((&stood["state"], &stood["answers"]), (&json!("idle"), &json!(second)), "it never took the third up: {stood}");
    // (Taken up late all the same, the turn is the third's.)
    rig.show("alpha", &working());
    at_work_on(third);
    rig.show("alpha", &idle("The third thing is done."));
    let ended = told(stood["cursor"].as_u64().unwrap());
    assert_eq!(ended["answers"], third, "{ended}");
}

/// `stop` answers once the tab's own program has ended, or says it has not, with its pid (the ShiftUp lead's
/// item 35, 2026-11-26: "Then a launcher doesn't have to watch the program go itself").
#[test]
fn a_stop_answers_once_the_program_has_ended_or_says_it_has_not() {
    let Some(rig) = Rig::start("stopped") else { return };
    let there = |pid: i64| Path::new(&format!("/proc/{pid}")).exists();
    // A program that goes when its tab is closed: gone by the time `stop` has answered.
    rig.json(&["start", "--command", "--name", "goes", "--json", "--", "sleep", "300"]);
    rig.wait("its process to be known", || rig.status("goes")["pid"].as_i64().is_some_and(|p| p > 1));
    let pid = rig.status("goes")["pid"].as_i64().unwrap();
    assert!(there(pid));
    let stopped = rig.json(&["stop", "goes", "--force", "--json"]);
    assert!(!there(pid), "`stop` answered while its program still ran: {stopped}");
    assert_eq!((&stopped["ended"], &stopped["programs"][0]["pid"], &stopped["programs"][0]["ended"]), (&json!(true), &json!(pid), &json!(true)), "{stopped}");

    // One that will not go (it takes no hang-up, no interrupt, no asking): said, with its pid, when its five seconds are up.
    rig.json(&["start", "--command", "--name", "stays", "--json", "--", "sh", "-c", "trap '' HUP INT TERM QUIT; while :; do sleep 1; done"]);
    rig.wait("its process to be known", || rig.status("stays")["pid"].as_i64().is_some_and(|p| p > 1));
    let pid = rig.status("stays")["pid"].as_i64().unwrap();
    let at = Instant::now();
    let (code, out, _) = rig.olo(&["stop", "stays", "--force"]);
    let took = at.elapsed();
    if there(pid) {
        assert!(code == 0 && out.contains("not ended after 5 s") && out.contains(&format!("stays (pid {pid})")), "{out}");
        assert!(took >= Duration::from_millis(4800) && took < Duration::from_secs(9), "{took:?}");
        // (Asked again in JSON of the tab that is gone: its record says how it ended; the process is this test's to end.)
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    } else {
        // (Where the Rex server ends a closed tab's program whatever it traps, it has ended: said so, at once.)
        assert!(out.starts_with("Stopped 1 subagent tab(s).") && took < Duration::from_secs(5), "{out} after {took:?}");
    }

    // Kept open, a child is only interrupted: its program goes on, and that is said, at once.
    rig.stand_in("claude");
    rig.start_child("claude", "kept", &[]);
    rig.wait("its process to be known", || rig.status("kept")["pid"].as_i64().is_some_and(|p| p > 1));
    let pid = rig.status("kept")["pid"].as_i64().unwrap();
    let at = Instant::now();
    let kept = rig.json(&["stop", "kept", "--keep-tab", "--json"]);
    assert!(at.elapsed() < Duration::from_secs(3), "{:?}", at.elapsed());
    assert_eq!((&kept["ended"], &kept["programs"][0]["pid"], &kept["programs"][0]["ended"]), (&json!(false), &json!(pid), &json!(false)), "{kept}");
    assert!(there(pid));
    rig.json(&["stop", "kept", "--force", "--json"]);
}

/// A minder that begins is told what stands (the ShiftUp lead's trial, 2026-11-27: a minder put in another's
/// place while its agent stood on a question was given the cursor of that moment and "will never be told of
/// it"; Ben, watching: "we shouldn't have to wait for the first watch to timeout it should see any outstanding
/// questions when it connects"). Their three cases: attached to an agent already asking; its rules replaced
/// while it asks; a minder killed while its agent asks.
#[test]
fn a_minder_that_begins_is_told_what_stands() {
    let Some(rig) = Rig::start("begins") else { return };
    rig.stand_in("claude");
    let env = |name: &str| -> Vec<String> {
        vec![
            "--env".into(),
            format!("STAND_IN_DIR={}", rig.dir.join("stage").display()),
            "--env".into(),
            format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display()),
            "--env".into(),
            format!("STAND_IN_NAME={name}"),
        ]
    };
    let mind = |args: &[&str], stand_in: &str| -> Value {
        Minding::expect(&rig, stand_in);
        let env = env(stand_in);
        let all: Vec<&str> = ["mind"].into_iter().chain(args.iter().copied()).chain(env.iter().map(String::as_str)).chain(["--json"]).collect();
        rig.json(&all)
    };
    let _started = rig.start_child("claude", "worker", &[]);
    let cursor = || rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let shown = |screen: &str| {
        let before = cursor();
        rig.show("worker", screen);
        rig.wait("the agent's new state to be written down", || cursor() > before);
    };
    // (A minder's first watch, from the cursor it was begun with: what it is told, and how soon.)
    let first_watch = |m: &Minding, begun: &Value, secs: u32| {
        let at = Instant::now();
        let said = m.run(&format!("{} {} watch --since {} --on question,approval,idle,error,exit --timeout {secs} --json; echo status=$?", m.env(), m.dir.join("mind").display(), begun["cursor"]));
        (said, at.elapsed())
    };
    let told = |said: &str| -> Value { serde_json::from_str(said.lines().next().unwrap_or_default()).unwrap_or_else(|e| panic!("{e}: {said}")) };

    // The agent asks, and nobody minds it.
    shown(&asking("printf hello > out.txt", 0));

    // 1. A minder attached while it asks. The cursor it begins with is after the question; it is told the question all the same, at once.
    let one = mind(&["worker", "--with", "claude:stand-in-big:max", "--rules-text", "Escalate every write."], "m1");
    let m1 = Minding::of(&rig, "m1", one["minder"]["id"].as_str().unwrap());
    let (said, took) = first_watch(&m1, &one, 20);
    assert!(told(&said)["approval"]["command"].as_str().unwrap().contains("printf hello"), "a minder attached to an agent already asking wasn't told what it asks: {said}");
    assert!(took < Duration::from_secs(4), "told only after {took:?}");
    // It passes the question up and leaves it standing: it is not told it again.
    m1.mind("escalate worker 'It asks to write out.txt.' --json");
    let (said, _) = first_watch(&m1, &told(&said), 1);
    assert!(said.contains("status=124"), "told the same standing question twice: {said}");

    // 2. Its rules replaced while the agent still asks: the SAME minder is told of the new rules and re-told the standing question at once without restarting.
    let a_ev = m1.mind("watch --since 0 --on minding --timeout 15 --json");
    assert_eq!(a_ev["minding"]["action"], "added", "{a_ev}");
    let two = rig.json(&["mind", "worker", "--rules-text", "Approve writes of out.txt.", "--json"]);
    assert_eq!(two["minder"]["id"], one["minder"]["id"], "rules updated without restarting the minder: {two}");
    let r_ev = m1.mind(&format!("watch --since {} --on minding --timeout 15 --json", two["cursor"]));
    assert_eq!(r_ev["minding"]["action"], "rules", "{r_ev}");
    let (said, took) = first_watch(&m1, &two, 20);
    assert!(told(&said)["approval"]["command"].as_str().unwrap().contains("printf hello") && took < Duration::from_secs(4), "after {took:?}: {said}");

    // 3. That minder killed while the agent asks, and another attached: told at once, and its yes is typed.
    rig.tell("m1", "exit 9");
    rig.wait("the killed minder to be gone", || rig.json(&["mind", "worker", "--json"])["minder"].is_null());
    let three = mind(&["worker", "--with", "claude:stand-in-big:max"], "m3");
    let m3 = Minding::of(&rig, "m3", three["minder"]["id"].as_str().unwrap());
    let (said, took) = first_watch(&m3, &three, 20);
    assert!(told(&said)["approval"]["command"].as_str().unwrap().contains("printf hello") && took < Duration::from_secs(4), "after {took:?}: {said}");
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = m3.mind("approve worker --once --json");
    assert!(gone.join().unwrap(), "{approved}");
    rig.typed_since("worker", before, b"\r", "what was typed into it");
    // (Each answer is kept with the minder that gave it: the first's escalation, the third's yes.)
    let of = rig.json(&["mind", "worker", "--json"]);
    let by: Vec<(&str, &Value)> = of["answered"].as_array().unwrap().iter().map(|a| (a["did"].as_str().unwrap(), &a["by"])).collect();
    assert_eq!(by, [("escalate", &one["minder"]["id"]), ("approve", &three["minder"]["id"])], "{of}");

    // An agent at work: a fresh minder that begins is told nothing (what was last written of it, its question, no longer stands).
    rig.wait_state("worker", "working");
    rig.json(&["mind", "worker", "--off", "--json"]);
    rig.json(&["stop", three["minder"]["id"].as_str().unwrap(), "--force", "--json"]);
    let four = mind(&["worker", "--with", "claude:stand-in-big:max"], "m4");
    let m4 = Minding::of(&rig, "m4", four["minder"]["id"].as_str().unwrap());
    let (said, _) = first_watch(&m4, &four, 1);
    assert!(said.contains("status=124"), "told of a question that was answered before it began: {said}");
    // …and an agent at rest: told that it rests, once.
    shown(&idle("Wrote out.txt."));
    rig.json(&["mind", "worker", "--off", "--json"]);
    rig.json(&["stop", four["minder"]["id"].as_str().unwrap(), "--force", "--json"]);
    let five = mind(&["worker", "--with", "claude:stand-in-big:max"], "m5");
    let m5 = Minding::of(&rig, "m5", five["minder"]["id"].as_str().unwrap());
    let (said, took) = first_watch(&m5, &five, 20);
    assert!(told(&said)["state"] == "idle" && took < Duration::from_secs(4), "after {took:?}: {said}");
    let (said, _) = first_watch(&m5, &told(&said), 1);
    assert!(said.contains("status=124"), "{said}");
}

/// `--trust`: the folder's trust, asked of a child's own folder at its start, is answered by the service, by
/// where the mark stands, whichever agent asks: Antigravity's "Do you trust the contents of this project?" (its
/// mark on the yes already: Return) as Claude Code's (its mark on "No, exit": Down, the mark seen on the yes,
/// then Return). Another folder's is nobody's to trust unasked: it stands, a question. (The ShiftUp lead's item
/// 42, 2026-11-27: `olo agent run agy … --trust --cwd NEW_FOLDER` stood on Antigravity's question; the screens
/// are the agents' own, read here that day.)
#[test]
fn a_childs_own_folder_is_trusted_at_its_start() {
    let Some(rig) = Rig::start("trust") else { return };
    rig.stand_in("claude");
    rig.stand_in("agy");
    let work = rig.work();
    let agys = |folder: &str| {
        format!(
            "Accessing workspace:\n{folder}\nDo you trust the contents of this project?\nAntigravity CLI requires permission to read, edit, and execute files here.\n> Yes, I trust this folder\n  No, exit\n  ↑/↓ Navigate · enter Confirm\n"
        )
    };
    let claudes = |marked: usize| {
        let mark = |i: usize| if i == marked { "❯" } else { " " };
        format!(
            "{0}\n Accessing workspace:\n\n {work}\n\n Quick safety check: Is this a project you created or one you trust?\n\n {1} No, exit\n {2} Yes, I trust this folder\n\n Enter to confirm · Esc to cancel\n",
            rule(),
            mark(0),
            mark(1)
        )
    };

    // Antigravity's, of its own folder.
    rig.start_child("agy", "reader", &["--trust", "--cwd", &work]);
    let before = rig.keys("reader").len();
    let gone = rig.when_typed("reader", b"\r", idle("Ready."));
    rig.show("reader", &agys(&work));
    assert!(gone.join().unwrap(), "Antigravity's question was left standing\n{}", rig.said());
    rig.typed_since("reader", before, b"\r", "Return, on the yes its mark stood on");

    // Claude Code's, of its own folder: never Return on "No, exit".
    rig.start_child("claude", "builder", &["--trust", "--cwd", &work]);
    let before = rig.keys("builder").len();
    let moved = rig.when_typed("builder", b"\x1b[B", claudes(1));
    let gone = rig.when_typed("builder", b"\r", idle("Ready."));
    rig.show("builder", &claudes(0));
    assert!(moved.join().unwrap() && gone.join().unwrap(), "Claude Code's question was left standing\n{}", rig.said());
    rig.typed_since("builder", before, b"\x1b[B\r", "Down to the yes, then Return");

    // Another folder than its own: left for whoever answers, nothing typed.
    rig.start_child("agy", "stray", &["--trust", "--cwd", &work]);
    let before = rig.keys("stray").len();
    rig.show("stray", &agys("/var/tmp/somebody-elses"));
    rig.wait_state("stray", "waiting_question");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(rig.keys("stray").len(), before, "a folder that isn't the child's own was answered");
}

/// `approve --box KEY`: a yes lands only on the box it was judged for (the ShiftUp lead's item 40, 2026-11-27:
/// "`approve` answers whatever stands when it arrives, so a yes judged for one box can land on the next"). A
/// box has a key, in the event that tells of it and in `status` alike; another box has another, though its
/// command differs by one digit (the screen's own key for a dialog holds those two for one), and so has the
/// very same command asked again once it was answered.
#[test]
fn a_yes_lands_only_on_the_box_it_was_judged_for() {
    let Some(rig) = Rig::start("box") else { return };
    rig.stand_in("claude");
    let worker = rig.start_child("claude", "worker", &[]);
    let standing = || rig.status("worker")["approval"]["box"].as_str().map(str::to_owned);
    rig.show("worker", &asking("sleep 5", 0));
    let asked = rig.json(&["watch", "worker", "--since", &worker["cursor"].to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    let first = asked["approval"]["box"].as_str().unwrap_or_else(|| panic!("an approval's event has its box's key: {asked}")).to_owned();
    assert_eq!(standing().as_deref(), Some(first.as_str()), "the same box read again has the same key");

    // Another box takes its place before the yes arrives.
    rig.show("worker", &asking("sleep 9", 0));
    rig.wait("the other box to be the one that stands", || standing().is_some_and(|b| b != first));
    let second = standing().unwrap();
    let before = rig.keys("worker").len();
    let (code, _, err) = rig.olo(&["approve", "worker", "--once", "--box", &first, "--json"]);
    assert!(code == 1 && err.contains("isn't the one that stands now") && err.contains(&second), "{code}: {err}");
    assert_eq!(rig.keys("worker").len(), before, "nothing was typed into it");

    // Judged again, and answered by its own key.
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = rig.json(&["approve", "worker", "--once", "--box", &second, "--json"]);
    assert!(gone.join().unwrap() && approved["dismissed"] == true, "{approved}");

    // The very same command asked again is another box: the key that was answered answers nothing.
    rig.show("worker", &asking("sleep 9", 0));
    rig.wait("its second asking to stand", || standing().is_some());
    let third = standing().unwrap();
    assert_ne!(third, second, "a box asked again after it was answered is another");
    let before = rig.keys("worker").len();
    let (code, _, err) = rig.olo(&["approve", "worker", "--once", "--box", &second, "--json"]);
    assert!(code == 1 && err.contains("isn't the one that stands now"), "{code}: {err}");
    assert_eq!(rig.keys("worker").len(), before, "nothing was typed into it");
    // (With no key said, it answers what stands, as before.)
    let gone = rig.when_typed("worker", b"\r", idle("Slept."));
    let approved = rig.json(&["approve", "worker", "--once", "--json"]);
    assert!(gone.join().unwrap() && approved["dismissed"] == true, "{approved}");
}

/// A prompt given at a start is taken up when the agent's own screen says a turn is on, and by nothing that
/// comes before: the agent loading (Rex's word for its pane is "at work" then), the folder's trust asked and
/// answered, its own empty prompt. None of those rests is told, and none answers the start. (The ShiftUp lead's
/// item 46, 2026-11-27: an agy started with a prompt stood at its own empty prompt 1.3 s later, was told at rest
/// *answering its start*, and `olo agent run` returned done and closed its tab: the agent had done nothing.)
#[test]
fn a_prompt_is_taken_up_by_a_turn_and_by_nothing_before_it() {
    let Some(rig) = Rig::start("taken") else { return };
    rig.stand_in("claude");
    let started = rig.start_child("claude", "loads", &["--prompt", "Read the file and say what it says."]);
    let cursor = started["cursor"].as_u64().unwrap_or_else(|| panic!("{started}")).to_string();
    let rests = |within: &str| rig.olo(&["watch", "loads", "--since", &cursor, "--on", "idle,done", "--timeout", within, "--json"]);
    // It loads: something on its screen, not its own prompt, no turn.
    rig.show("loads", "Claude Code\n\n  Loading your project…\n");
    std::thread::sleep(Duration::from_millis(600));
    // The folder's trust, asked before any prompt is taken up; then its own prompt, empty.
    rig.show("loads", &format!("{0}\n Accessing workspace:\n\n {1}\n\n Quick safety check: Is this a project you created or one you trust?\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel\n", rule(), rig.work()));
    rig.wait_state("loads", "waiting_question");
    rig.show("loads", &idle("Ready."));
    let (code, out, err) = rests("3");
    assert!(code == 124, "a rest before its prompt's turn was told: {code}: {out}{err}\n{}", rig.said());
    assert!(rig.status("loads")["answers"].is_null(), "nothing has answered its start yet: {}", rig.status("loads"));
    // Its turn, by its own screen; and its end: told, and it answers the start.
    rig.show("loads", &working());
    rig.wait("its turn to be the one that answers its start", || rig.status("loads")["answers"].as_u64().map(|a| a.to_string()) == Some(cursor.clone()));
    rig.show("loads", &idle("It says forty-two."));
    let (code, out, err) = rests("20");
    let told: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{code}: {e}: {out}{err}"));
    assert!(code == 0 && matches!(told["state"].as_str(), Some("idle" | "done")) && told["answers"].as_u64().map(|a| a.to_string()) == Some(cursor.clone()), "{told}");
}

/// Real `agy 1.5.18` startup race: while `ChainedAuth` resolves, `agy` draws its pre-chat
/// `"Welcome to the Antigravity CLI ... ⣷  Signing in..."` splash (with NO chat input box) and
/// emits `OSC 7501;state=blocked:kind=auth`. Once auth finishes, `agy` draws its empty prompt
/// (`──── / > / ──── / ? for shortcuts`) with `OSC 7501;state=idle` for 300-1200ms while
/// `model_discovery` runs, before finally taking up `-i PROMPT` (`state=working`, `⣷ Generating...`).
///
/// The `"Signing in..."` splash must NEITHER take up the pending prompt expectation (which would
/// let the subsequent pre-turn empty prompt emit `idle` with `answers: Some(cursor)` and fail
/// `olo agent run` as `"silent"`), NOR fail as `waiting_question` if sign-in persists >= 250ms.
/// Meanwhile, a genuine unauthenticated `"Select login method:"` chooser MUST still be reported
/// as `waiting_question`.
#[test]
fn agy_signing_in_splash_does_not_take_up_prompt_or_fail_silent() {
    let Some(rig) = Rig::start("agysign") else { return };
    rig.stand_in("agy");
    let agy_signing_in = concat!(
        "\x1b]7501;state=blocked:kind=auth:app=antigravity:msg=U2lnbiBpbiByZXF1aXJlZA==\x07",
        "\n     ▄▀▀▄\n    ▀▀▀▀▀▀\n   ▀▀▀▀▀▀▀▀\n  ▄▀▀    ▀▀▄\n ▄▀▀      ▀▀▄\n\n",
        " Welcome to the Antigravity CLI. You are currently not signed in.\n\n",
        " ⣷  Signing in...\n"
    );
    let agy_empty_prompt = format!(
        "\x1b]7501;state=idle:app=antigravity\x07\n      ▄▀▀▄        Antigravity CLI 1.5.18\n     ▀▀▀▀▀▀       Enterprise Gateway\n    ▀▀▀▀▀▀▀▀      {1}\n   ▄▀▀    ▀▀▄\n  ▄▀▀      ▀▀▄\n\n{0}\n>\n{0}\n? for shortcuts\n",
        rule(),
        rig.work()
    );
    let agy_generating =
        format!("\x1b]7501;state=working:app=antigravity\x07\n      ▄▀▀▄        Antigravity CLI 1.5.18\n\n{0}\n> Check the file.\n⣷  Generating...\n{0}\n>\n{0}\nesc to cancel\n", rule());
    let agy_done = format!(
        "\x1b]7501;state=done:app=antigravity:msg=QUxMX0dPT0Q=\x07\n      ▄▀▀▄        Antigravity CLI 1.5.18\n\n{0}\n> Check the file.\n\n  ALL_GOOD\n\n{0}\n>\n{0}\n? for shortcuts\n",
        rule()
    );
    let started = rig.start_child("agy", "signer", &["--prompt", "Check the file."]);
    let cursor = started["cursor"].as_u64().unwrap_or_else(|| panic!("{started}")).to_string();
    let rests = |within: &str| rig.olo(&["watch", "signer", "--since", &cursor, "--on", "idle,done,question", "--timeout", within, "--json"]);

    // 1. Pre-chat "Signing in..." splash held for 450ms (> CONFIRM=250ms): must neither take up
    //    `expects` (`answers` must stay null) nor trigger `waiting_question`.
    rig.show("signer", agy_signing_in);
    std::thread::sleep(Duration::from_millis(450));
    let st_signing = rig.status("signer");
    assert!(st_signing["answers"].is_null(), "Signing in... splash must NOT take up prompt expectation: {st_signing}");
    assert_eq!(st_signing["state"], "working", "Signing in... splash must NOT trip waiting_question: {st_signing}");

    // 2. Post-auth empty prompt (`state=idle`) during `model_discovery`: held for 700ms (> CONFIRM).
    //    Must NOT be told as a rest answering the prompt!
    rig.show("signer", &agy_empty_prompt);
    let (code, out, err) = rests("1");
    assert_eq!(code, 124, "pre-turn rest after Signing in... was wrongly told: {code}: {out}{err}\n{}", rig.said());
    assert!(rig.status("signer")["answers"].is_null(), "answers must still be null at pre-turn empty prompt: {}", rig.status("signer"));

    // 3. Real turn starts (`Generating...` above input box + `esc to cancel` below) -> takes up expectation.
    rig.show("signer", &agy_generating);
    rig.wait("real turn takes up prompt", || rig.status("signer")["answers"].as_u64().map(|a| a.to_string()) == Some(cursor.clone()));

    // 4. Real turn finishes -> told as done answering the start.
    rig.show("signer", &agy_done);
    let (code, out, err) = rests("15");
    let told: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{code}: {e}: {out}{err}"));
    assert!(code == 0 && matches!(told["state"].as_str(), Some("idle" | "done")) && told["answers"].as_u64().map(|a| a.to_string()) == Some(cursor.clone()), "{told}");

    // 5. Genuine unauthenticated login chooser MUST still surface as `waiting_question`.
    let agy_login_chooser = concat!(
        "\x1b]7501;state=blocked:kind=auth:app=antigravity:msg=U2lnbiBpbiByZXF1aXJlZA==\x07",
        "\n     ▄▀▀▄\n    ▀▀▀▀▀▀\n   ▀▀▀▀▀▀▀▀\n  ▄▀▀    ▀▀▄\n ▄▀▀      ▀▀▄\n\n",
        " Welcome to the Antigravity CLI. You are currently not signed in.\n\n",
        " Select login method:\n > 1. Google OAuth\n   2. Use a Google Cloud project\n\n",
        "   ↑/↓ Navigate · enter Select\n"
    );
    let unauth = rig.start_child("agy", "unauth", &["--prompt", "Check."]);
    let ucur = unauth["cursor"].as_u64().unwrap().to_string();
    rig.show("unauth", agy_login_chooser);
    let (code, out, err) = rig.olo(&["watch", "unauth", "--since", &ucur, "--on", "question", "--timeout", "10", "--json"]);
    assert_eq!(code, 0, "genuine login chooser must emit waiting_question: {out}{err}");
}

/// `olo agent run`: one agent for one prompt, start to finish (the ShiftUp lead's item 36). What comes in a
/// turn's place is said, with a status of its own, and the tab kept where somebody is to answer it. (Its
/// last message whole, and its file, are tried with Claude Code itself: `agents_live.rs`.)
#[test]
fn one_agent_for_one_prompt_from_start_to_finish() {
    let Some(rig) = Rig::start("oneshot") else { return };
    rig.stand_in("claude");
    let run = |name: &str, more: &[&str]| -> (i32, String, String) {
        assert!(Command::new("mkfifo").arg(rig.staged(name, "fifo")).status().unwrap().success());
        let (stage, path, named) = (format!("STAND_IN_DIR={}", rig.dir.join("stage").display()), format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display()), format!("STAND_IN_NAME={name}"));
        // (A time of its own for each, so that none waits for ever on a stand-in that was never told: said, where it isn't the case tried.)
        let work = rig.work();
        let mut args = vec!["run", "claude", "--model", "stand-in", "--effort", "none", "--name", name, "--cwd", &work, "--env", &stage, "--env", &named, "--env", &path];
        if !more.contains(&"--timeout") {
            args.extend(["--timeout", "40"]);
        }
        args.extend_from_slice(more);
        rig.olo(&args)
    };
    // A prompt is what it runs: none, nothing started.
    let (code, _, err) = rig.olo(&["run", "claude", "--model", "stand-in", "--effort", "none", "--name", "nothing"]);
    assert!(code == 2 && err.contains("--prompt"), "{err}");
    assert!(rig.json(&["list", "--all", "--json"])["subagents"].as_array().unwrap().is_empty());

    std::thread::scope(|scope| {
        // Its prompt's turn ends: status 0, its last message whole, its tab stopped, and what it rested on before
        // its turn told to nobody.
        const READS: &str = "5f0c1f0e-1111-4a11-8a11-2f8f0e0a0001";
        let done = scope.spawn(|| run("reads", &["--prompt", "Read the file and say what it says.", "--json", "--session-id", READS]));
        rig.wait("the stand-in to listen", || rig.staged("reads", "ready").exists());
        rig.show("reads", &idle("Ready."));
        std::thread::sleep(Duration::from_millis(700));
        assert!(!done.is_finished(), "it answered on the rest before its prompt's turn");
        rig.show("reads", &working());
        rig.wait("the service to have seen it at work", || rig.status("reads")["answers"].is_u64());
        rig.says(READS, "Read the file and say what it says.", "It says forty-two.");
        rig.show("reads", &idle("It says forty-two."));
        let (code, out, err) = done.join().unwrap();
        assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
        let said: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
        assert_eq!((&said["ok"], &said["ended_as"], &said["stopped"], &said["name"]), (&json!(true), &json!("done"), &json!(true), &json!("reads")), "{said}");
        assert_eq!(said["last_message"], "It says forty-two.", "{said}");
        assert_eq!(rig.status("reads")["running"], false, "its tab is stopped");

        // A turn that ends with no message of the agent's to give is no reading: status 1, said, never "done"
        // (the ShiftUp lead's item 46: an agent that had done nothing was reported done, exit 0, its message
        // null, and "we nearly read 'no findings' from it").
        let mute = scope.spawn(|| run("mute", &["--prompt", "Read the file and say what it says.", "--json"]));
        rig.wait("the stand-in to listen", || rig.staged("mute", "ready").exists());
        rig.show("mute", &working());
        rig.wait("the service to have seen it at work", || rig.status("mute")["answers"].is_u64());
        rig.show("mute", &idle("…"));
        let (code, out, err) = mute.join().unwrap();
        let said: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
        assert!(code == 1 && said["ok"] == false && said["ended_as"] == "silent" && said["last_message"].is_null() && err.contains("no message of its own"), "{code}: {said}\n{err}");

        // It asks leave and nobody is set to answer: status 3, said, its tab kept, and it can be answered.
        let asks = scope.spawn(|| run("asks", &["--prompt", "Push it."]));
        rig.wait("the stand-in to listen", || rig.staged("asks", "ready").exists());
        rig.show("asks", &asking("git push origin main", 0));
        let (code, out, err) = asks.join().unwrap();
        assert!(code == 3 && out.is_empty() && err.contains("`asks` asks leave") && err.contains("git push origin main") && err.contains("olo agent approve asks"), "{code}: {out}{err}");
        assert_eq!(rig.status("asks")["state"], "waiting_approval", "kept, and still asking");
        rig.json(&["stop", "asks", "--force", "--json"]);

        // It exits before its turn ends: status 1, said.
        let dies = scope.spawn(|| run("dies", &["--prompt", "Do it."]));
        rig.wait("the stand-in to listen", || rig.staged("dies", "ready").exists());
        rig.show("dies", &working());
        rig.tell("dies", "exit 7");
        let (code, _, err) = dies.join().unwrap();
        assert!(code == 1 && err.contains("`dies` exited before its turn ended"), "{code}: {err}");

        // Kept open when told to; and a time given that passes: status 124, kept.
        let slow = scope.spawn(|| run("slow", &["--prompt", "Take your time.", "--timeout", "2"]));
        rig.wait("the stand-in to listen", || rig.staged("slow", "ready").exists());
        rig.show("slow", &working());
        let (code, _, err) = slow.join().unwrap();
        assert!(code == 124 && err.contains("had not ended its turn after 2 s"), "{code}: {err}");
        assert_eq!(rig.status("slow")["running"], true);
        rig.json(&["stop", "slow", "--force", "--json"]);
    });
}

/// A child started from a shell that isn't in Rex (a cron job, a plain SSH shell: no `REX_BLOCK`) goes under the
/// tab that is named, by a pane's block, a tab's window or a child's name, in whichever session that tab is; with
/// none named, nothing is started. (The ShiftUp lead's item 45. It went under the tab in front in the first
/// session the server listed, whoever's that was.)
#[test]
fn a_child_from_outside_rex_goes_under_the_tab_that_is_named() {
    let Some(rig) = Rig::start("outside") else { return };
    rig.stand_in("claude");
    let (code, _, err) = rig.olo_from(None, &["start", "claude", "--model", "stand-in", "--effort", "none", "--name", "stray", "--json"]);
    assert!(code != 0 && err.contains("--parent BLOCK"), "{code}: {err}");
    assert!(rig.json(&["list", "--all", "--json"])["subagents"].as_array().unwrap().is_empty(), "nothing was started");

    // Under the lead's tab, by its pane's block; and it is the lead's child as any other.
    let cron = rig.start_child_from(None, "claude", "cron", &["--parent", &rig.lead_block]);
    assert_eq!(cron["parent_window_id"], rig.lead_window.as_str(), "{cron}");
    let mine = rig.json(&["list", "--json"]);
    assert!(mine["subagents"].as_array().unwrap().iter().any(|a| a["name"] == "cron"), "{mine}");
    // Under that child, by its name.
    let under = rig.start_child_from(None, "claude", "nested", &["--parent", "cron"]);
    assert_eq!(under["parent_window_id"], cron["window_id"], "{under}");

    // A tab of another session, by its block and by its window.
    let other = rig.rex("session.create", json!({ "label": "other" }));
    let tab = rig.rex(
        "session.new_window",
        json!({ "session_id": other["session_id"], "window_label": "other", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 110, "rows": 40 } } } } }),
    );
    let (window, block) = (tab["window_id"].as_str().unwrap().to_owned(), tab["block_ids"][0].as_str().unwrap().to_owned());
    let far = rig.start_child_from(None, "claude", "far", &["--parent", &block]);
    assert_eq!(far["parent_window_id"], window.as_str(), "{far}");
    // (Followed from the shell that started it, as it was started: by its name, from outside Rex.)
    let (code, out, err) = rig.olo_from(None, &["status", "far", "--json"]);
    let status: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{code}: {e}: {out}{err}"));
    assert!(code == 0 && status["parent_window_id"] == window.as_str() && status["running"] == true, "in that tab's session, not the first one listed: {status}");
    let farther = rig.start_child_from(None, "claude", "farther", &["--parent", &window]);
    assert_eq!(farther["parent_window_id"], window.as_str(), "{farther}");

    // A tab that is nowhere: said, nothing started.
    let (code, _, err) = rig.olo_from(None, &["start", "claude", "--model", "stand-in", "--effort", "none", "--name", "lost", "--parent", "block:nowhere", "--json"]);
    assert!(code != 0 && err.contains("isn't in any Rex session"), "{code}: {err}");
}

/// `olo agent resume NAME`: an ended child is started again as the same conversation, with what it was started
/// with (the ShiftUp lead's item 44: "model, effort, cwd, env, settings, the list, its minder and rules. Then our
/// records keep no conversation id at all"). Its own flags again, its environment, its folder; the conversation
/// it had in place of whatever named one at its start; what is said to it now in place of its first prompt. One
/// that runs, or that never began a conversation, is not started again, and says why.
#[test]
fn an_ended_child_is_started_again_as_it_was() {
    let Some(rig) = Rig::start("resume") else { return };
    rig.stand_in("claude");
    const SID: &str = "5f0c1f0e-3333-4a33-8a33-2f8f0e0a0003";
    let work = rig.work();
    let first = rig.start_child("claude", "builder", &["--cwd", &work, "--session-id", SID, "--env", "COLOUR=teal", "--permission-mode", "acceptEdits", "--prompt", "Build the first part."]);
    let (code, _, err) = rig.olo(&["resume", "builder"]);
    assert!(code == 1 && err.contains("`builder` is running"), "{code}: {err}");
    rig.json(&["stop", "builder", "--force", "--json"]);

    // Started again: the stand-in listens anew, under the same name.
    std::fs::remove_file(rig.staged("builder", "ready")).unwrap();
    let again = rig.json(&["resume", "builder", "--prompt", "Go on with the second part.", "--json"]);
    assert!(again["name"] == "builder" && again["id"] != first["id"], "{again}");
    rig.wait("the stand-in to listen again", || rig.staged("builder", "ready").exists());
    // What it runs as now: its own command line, its folder, what its environment has.
    rig.tell("builder", "run tr '\\0' '\\n' < /proc/$PPID/cmdline; echo; echo COLOUR=$COLOUR; pwd");
    rig.wait("it to have said what it runs as", || rig.staged("builder", "ran").exists());
    let said = std::fs::read_to_string(rig.staged("builder", "out")).unwrap();
    let args: Vec<&str> = said.lines().collect();
    let after = |flag: &str| args.iter().position(|a| *a == flag).and_then(|i| args.get(i + 1)).copied();
    assert_eq!((after("--resume"), after("--session-id"), after("--permission-mode"), after("--model")), (Some(SID), None, Some("acceptEdits"), Some("stand-in")), "{said}");
    assert!(said.contains("Go on with the second part.") && !said.contains("Build the first part."), "what is said to it now, not its first prompt again: {said}");
    assert!(args.contains(&"COLOUR=teal") && args.contains(&work.as_str()), "its environment and its folder: {said}");
    assert_eq!(rig.status("builder")["cli_session_id"], SID);

    // What it was started with is the owner's alone to read, and has no prompt in it.
    let kept = rig.dir.join("agents");
    let files: Vec<PathBuf> = std::fs::read_dir(&kept)
        .unwrap()
        .flatten()
        .map(|d| d.path().join("minders"))
        .filter(|p| p.is_dir())
        .flat_map(|p| std::fs::read_dir(p).unwrap().flatten().map(|d| d.path().join("started-with.json")))
        .filter(|p| p.exists())
        .collect();
    assert!(!files.is_empty(), "nothing kept under {}", kept.display());
    for file in &files {
        assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600, "{}", file.display());
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains("COLOUR=teal") && !text.contains("Build the first part.") && !text.contains("Go on with"), "{text}");
    }

    // One that never began a conversation: nothing to take up again.
    rig.start_child("claude", "blank", &[]);
    rig.json(&["stop", "blank", "--force", "--json"]);
    let (code, _, err) = rig.olo(&["resume", "blank"]);
    assert!(code == 1 && err.contains("has no conversation to take up again"), "{code}: {err}");
    let (code, _, err) = rig.olo(&["resume", "builder", "--model", "another"]);
    assert!(code == 2 && err.contains("unknown resume option"), "{code}: {err}");
}

/// `olo agent send NAME "…" --enter --wait`: asking a child that stands is one line that prints its answer (the
/// ShiftUp lead's item 43). Sent to a child at rest, the message begins a turn of its own, and the answer is
/// that turn's last message: not the rest it was sent from, which is still on the screen for a moment. Sent
/// into a turn already going, it is that turn's, whose end is waited for. What comes in a turn's place is said
/// as `run` says it, and the child is left as it is.
#[test]
fn a_message_is_sent_and_its_answer_waited_for() {
    let Some(rig) = Rig::start("sendwait") else { return };
    rig.stand_in("claude");
    const PARTNER: &str = "5f0c1f0e-2222-4a22-8a22-2f8f0e0a0002";
    let work = rig.work();
    rig.start_child("claude", "partner", &["--cwd", &work, "--session-id", PARTNER]);
    rig.says(PARTNER, "Hello.", "Ready.");
    rig.show("partner", &idle("Ready."));
    // (At rest: "idle", or "done" where its conversation says its last turn is over.)
    let rests = || matches!(rig.status("partner")["state"].as_str(), Some("idle" | "done"));
    rig.wait("it to be at rest", rests);
    let ask = |more: &[&str]| {
        let mut args = vec!["send", "partner", "What does the file say?", "--enter", "--wait"];
        args.extend_from_slice(more);
        rig.olo(&args)
    };

    std::thread::scope(|scope| {
        // At rest: its answer is the turn the message begins, and nothing before it.
        let before = rig.keys("partner").len();
        let asked = scope.spawn(|| ask(&["--json", "--timeout", "40"]));
        rig.wait("the message to be typed into it", || rig.keys("partner").len() > before);
        std::thread::sleep(Duration::from_millis(700));
        assert!(!asked.is_finished(), "it answered with the rest the message was sent from");
        rig.show("partner", &working());
        rig.wait("the service to have seen it at work", || rig.status("partner")["state"] == "working");
        rig.says(PARTNER, "What does the file say?", "It says forty-two.");
        rig.show("partner", &idle("It says forty-two."));
        let (code, out, err) = asked.join().unwrap();
        let said: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{code}: {e}: {out}{err}"));
        assert!(code == 0 && said["ok"] == true && said["ended_as"] == "done" && said["last_message"] == "It says forty-two.", "{code}: {said}\n{err}");
        assert_eq!(rig.status("partner")["running"], true, "the child is left as it is");

        // Its answer alone, as text, where no JSON is asked.
        let typed = rig.keys("partner").len();
        let asked = scope.spawn(|| ask(&["--timeout", "40"]));
        rig.wait("the second message to be typed into it", || rig.keys("partner").len() > typed);
        rig.show("partner", &working());
        rig.wait("the service to have seen it at work", || rig.status("partner")["state"] == "working");
        rig.says(PARTNER, "What does the file say?", "Still forty-two.");
        rig.show("partner", &idle("Still forty-two."));
        let (code, out, err) = asked.join().unwrap();
        assert!(code == 0 && out.trim() == "Still forty-two.", "{code}: {out}{err}");

        // Into a turn already going: that turn's end is its answer.
        rig.show("partner", &working());
        rig.wait_state("partner", "working");
        let asked = scope.spawn(|| ask(&["--json", "--timeout", "40", "--force"]));
        std::thread::sleep(Duration::from_millis(700));
        assert!(!asked.is_finished());
        rig.says(PARTNER, "What does the file say?", "Forty-two, as before.");
        rig.show("partner", &idle("Forty-two, as before."));
        let (code, out, err) = asked.join().unwrap();
        let said: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{code}: {e}: {out}{err}"));
        assert!(code == 0 && said["last_message"] == "Forty-two, as before.", "{code}: {said}\n{err}");

        // It asks leave in the message's turn: status 3, said, and it still asks.
        let typed = rig.keys("partner").len();
        let asked = scope.spawn(|| ask(&["--timeout", "40"]));
        rig.wait("the next message and Enter to be typed into it", || rig.keys("partner")[typed..].contains(&b'\r'));
        rig.show("partner", &asking("git push origin main", 0));
        let (code, out, err) = asked.join().unwrap();
        assert!(code == 3 && out.is_empty() && err.contains("`partner` asks leave") && err.contains("git push origin main"), "{code}: {out}{err}");
        assert_eq!(rig.status("partner")["state"], "waiting_approval");

        // Its time passes: status 124.
        rig.show("partner", &idle("Not pushed."));
        rig.wait("it to be at rest again", rests);
        let (code, _, err) = ask(&["--timeout", "2"]);
        assert!(code == 124 && err.contains("had not ended its turn after 2 s"), "{code}: {err}");
    });
    // (A message not submitted begins no turn: said, nothing sent.)
    let (code, _, err) = rig.olo(&["send", "partner", "half a thought", "--wait"]);
    assert!(code == 2 && err.contains("--enter"), "{code}: {err}");
}

/// `olo agent watch --follow`: a watch that goes on (the ShiftUp lead's item 38: "a lead must keep one wait
/// running over all its children and start it again each time it returns"). One command, each thing as it
/// comes, until it is stopped.
#[test]
fn a_watch_that_goes_on_says_each_thing_as_it_comes() {
    let Some(rig) = Rig::start("follow") else { return };
    rig.stand_in("claude");
    rig.start_child("claude", "alpha", &[]);
    rig.show("alpha", &idle("Ready."));
    rig.wait("its rest to be written down", || rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().is_some_and(|c| c >= 1));
    // (Begun now: what was before it is not said again.)
    let mut follower = rig.helper();
    let mut follower = follower
        .args(["agent", "watch", "--all", "--follow", "--on", "approval,idle,escalation,exit"])
        .env("REX_BLOCK", &rig.lead_block)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let reader = {
        let (lines, out) = (lines.clone(), follower.stdout.take().unwrap());
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(line);
            }
        })
    };
    let said = || lines.lock().unwrap().clone();
    let heard = |what: &str| rig.wait(&format!("the follower to say `{what}`"), || said().iter().any(|l| l.contains(what)));
    std::thread::sleep(Duration::from_millis(600));
    assert!(said().is_empty(), "said what was there before it began: {:?}", said());

    rig.show("alpha", &asking("cargo test", 0));
    heard("alpha: asks leave: cargo test");
    let gone = rig.when_typed("alpha", b"\r", working());
    rig.json(&["approve", "alpha", "--once", "--json"]);
    assert!(gone.join().unwrap());
    rig.wait_state("alpha", "working");
    rig.show("alpha", &idle("All tests pass."));
    heard("alpha: idle");
    // A second child started while it follows is followed too; and a child's own word to its parent.
    rig.start_child("claude", "beta", &[]);
    let helper = format!("OLO_REX_AGENTS_DIR={} REX_SERVER_SOCKET={} {HELPER} agent", rig.dir.join("agents").display(), rig.sock.display());
    // (Two lines of it: made by the shell, a stand-in being told one line at a time.)
    rig.tell("beta", &format!("run {helper} escalate \"$(printf 'beta has a word:\\nof two lines.')\""));
    heard("beta: escalation");
    heard("of two lines.");
    rig.tell("alpha", "exit 0");
    heard("alpha: exited (code 0)");
    // One command said all of it, in order, and still runs.
    let all = said();
    let at = |what: &str| all.iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what}: {all:?}"));
    assert!(at("asks leave") < at("alpha: idle") && at("alpha: idle") < at("beta: escalation") && at("beta: escalation") < at("alpha: exited"), "{all:?}");
    assert!(all[at("beta: escalation") + 1].starts_with("beta has a word:"), "its message whole, under its line: {all:?}");
    assert!(follower.try_wait().unwrap().is_none(), "it goes on until it is stopped");
    let _ = follower.kill();
    let _ = follower.wait();
    let _ = reader.join();
    rig.json(&["stop", "beta", "--force", "--json"]);
}

/// Leave given by a list, by the service itself (the ShiftUp lead's item 37, with Ben's words: "i don't care
/// much about requiring approvals from them at all only in the most extreme cases"): yes, once, at once, to
/// whatever a child asks leave for, except what a pattern of the parent's list names, which is asked as before
/// (of its minder if it has one, else on the parent's watch). Never a kept yes; and what it allowed is kept.
#[test]
fn leave_is_given_by_a_list_and_what_it_names_is_asked() {
    let Some(rig) = Rig::start("leave") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "# What is never done without asking.\n\\bgit\\s+(push|commit)\\b\n\\brm\\s+-[a-zA-Z]*r\n").unwrap();
    // A list that isn't one: said, and nothing started.
    std::fs::write(rig.dir.join("work/broken.txt"), "\\bsudo\\b\n(unclosed\n").unwrap();
    let (code, _, err) = rig.olo(&["start", "claude", "--model", "m", "--effort", "e", "--name", "nobody", "--approve-all-but", "broken.txt"]);
    assert!(code == 2 && err.contains("line 2 of the list isn't a pattern"), "{err}");
    assert!(rig.json(&["list", "--all", "--json"])["subagents"].as_array().unwrap().is_empty());

    let started = rig.start_child("claude", "worker", &["--approve-all-but", "not-these.txt"]);
    assert_eq!(started["approve_all_but"], 2, "{started}");
    let from = started["cursor"].as_u64().unwrap();
    let woken = |since: u64, secs: &str| rig.olo(&["watch", "--all", "--since", &since.to_string(), "--on", "approval,question", "--timeout", secs, "--json"]);

    // What no pattern names: the service says yes, once, itself, and nobody is told there was a question.
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    let at = Instant::now();
    rig.show("worker", &asking("cd /w && cargo test -p olo-kit > test.log", 0));
    assert!(gone.join().unwrap(), "the service didn't answer it\n{}", rig.said());
    // (At once, by its list: not after any wait for a minder or a parent, which are a minute and more. The
    // bound was four seconds, and on a machine with eighty runnable tasks for thirty-two processors the
    // service's own answer took longer once, 2026-11-27: it tells "at once" from "waited for someone", no more.)
    assert!(at.elapsed() < Duration::from_secs(12), "{:?}", at.elapsed());
    rig.typed_since("worker", before, b"\r", "Return on Yes, the first choice: once, and nothing else");
    let (code, out, _) = woken(from, "1");
    assert_eq!(code, 124, "the parent was told of a question its list answers: {out}");

    // What a pattern names (a later part of a chain): asked as before, on the parent's watch, nothing typed.
    let before = rig.keys("worker").len();
    rig.show("worker", &asking("cargo build && git push origin main", 0));
    let (code, out, err) = woken(from, "15");
    assert_eq!(code, 0, "{out}{err}");
    let asked: Value = serde_json::from_str(out.trim()).unwrap();
    assert!(asked["approval"]["command"].as_str().unwrap().contains("git push"), "{asked}");
    assert_eq!(rig.keys("worker").len(), before, "the service answered what its list names");
    let gone = rig.when_typed("worker", b"\r", working());
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("cargo build && git push origin main", 2));
    rig.json(&["approve", "worker", "--no", "--json"]);
    assert!(moved.join().unwrap() && gone.join().unwrap());

    // What it allowed is kept, with the whole of what was asked.
    let of = rig.json(&["mind", "worker", "--json"]);
    assert_eq!(of["approve_all_but"], json!(["\\bgit\\s+(push|commit)\\b", "\\brm\\s+-[a-zA-Z]*r"]), "{of}");
    let allowed: Vec<(&str, &str, &str)> = of["answered"].as_array().unwrap().iter().map(|a| (a["did"].as_str().unwrap(), a["by"].as_str().unwrap(), a["said"].as_str().unwrap())).collect();
    assert_eq!(allowed, [("allowed", "list", "Bash: cd /w && cargo test -p olo-kit > test.log")], "{of}");

    // With a minder too: what the list names goes to the minder, not to the parent; what it allows, to nobody.
    Minding::expect(&rig, "lm");
    let minded = rig.json(&[
        "mind",
        "worker",
        "--with",
        "claude:stand-in-big:max",
        "--rules-text",
        "Refuse commits.",
        "--json",
        "--env",
        &format!("STAND_IN_DIR={}", rig.dir.join("stage").display()),
        "--env",
        &format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display()),
        "--env",
        "STAND_IN_NAME=lm",
    ]);
    let lm = Minding::of(&rig, "lm", minded["minder"]["id"].as_str().unwrap());
    let c = minded["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("git commit -am wip", 0));
    let told = lm.mind(&format!("watch --since {c} --on question,approval,idle,error,exit --timeout 15 --json"));
    assert!(told["approval"]["command"].as_str().unwrap().contains("git commit"), "{told}");
    let (code, out, _) = woken(c, "1");
    assert_eq!(code, 124, "what the list names went to the parent though the child has a minder: {out}");
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("git commit -am wip", 2));
    let gone = rig.when_typed("worker", b"\r", working());
    lm.mind("approve worker --no --json");
    assert!(moved.join().unwrap() && gone.join().unwrap());
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    rig.show("worker", &asking("cargo fmt --check", 0));
    assert!(gone.join().unwrap(), "what the list allows is the service's to answer, minder or no minder");
    rig.typed_since("worker", before, b"\r", "once");
    let said = lm.run(&format!("{} {} watch --since {} --on question,approval --timeout 1 --json; echo status=$?", lm.env(), lm.dir.join("mind").display(), told["cursor"]));
    assert!(said.contains("status=124"), "the minder was told of a question the list answered: {said}");
    rig.json(&["mind", "worker", "--off", "--json"]);

    // The list changed as it runs (what was allowed is now named), and taken away (everything is asked).
    std::fs::write(rig.dir.join("work/stricter.txt"), "cargo\n").unwrap();
    let (code, said, err) = rig.olo(&["mind", "worker", "--approve-all-but", "stricter.txt"]);
    assert!(code == 0 && said.contains("except what 1 pattern names"), "{said}{err}");
    let c = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("cargo test", 0));
    assert_eq!(woken(c, "15").0, 0, "what the new list names is asked");
    let gone = rig.when_typed("worker", b"\r", working());
    rig.json(&["approve", "worker", "--once", "--json"]);
    assert!(gone.join().unwrap());
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    rig.show("worker", &asking("ls -la", 0));
    assert!(gone.join().unwrap(), "what the new list doesn't name is allowed");
    rig.typed_since("worker", before, b"\r", "once");
    let (code, said, _) = rig.olo(&["mind", "worker", "--approve-none"]);
    assert!(code == 0 && said.contains("asked again"), "{said}");
    let c = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("ls -la /w", 0));
    assert_eq!(woken(c, "15").0, 0, "with no list everything is asked");
    assert_eq!(rig.json(&["mind", "worker", "--json"])["approve_all_but"], Value::Null);
    rig.json(&["stop", "worker", "--force", "--json"]);
}

/// A child's tab is never the service's to size. The Rex server makes
/// whoever creates a block its owner if they are attached to its session
/// then, and the service is attached from its first child there on: its
/// second child and every one after was the service's own, stood at the
/// size of the pane it was asked from for as long as it ran, and a
/// desktop that showed it in a shorter view had its last rows cut off
/// (Ben's agy children with no typing line, 2026-11-27). Every child's
/// block has no owner, the first's and the later ones'; keeps the size it
/// was made with; takes the size a client that shows it asks for; and
/// has none still after the service has started again and found them.
#[test]
fn a_childs_tab_is_never_the_services_to_size() {
    let Some(rig) = Rig::start("size-owner") else { return };
    let size_of = |block: &str| {
        let said = rig.rex("com.superlogical.terminal.size", json!({ "session_id": rig.session, "block_id": block, "args": {} }));
        if said.get("columns").is_some() { said } else { said["payload"].clone() }
    };
    let lead = size_of(&rig.lead_block);
    let blocks: Vec<String> =
        (1..=3).map(|i| rig.json(&["start", "--command", "--name", &format!("child-{i}"), "--json", "--", "sleep", "300"])["block_id"].as_str().expect("its block").to_owned()).collect();
    for (i, block) in blocks.iter().enumerate() {
        let size = size_of(block);
        assert!(size["owner"].is_null(), "child {}'s block has an owner: {size}", i + 1);
        assert_eq!((&size["columns"], &size["rows"]), (&lead["columns"], &lead["rows"]), "as large as the pane it was asked from, until somebody shows it");
    }
    // A client that shows the third (as a desktop does: it asks for its view's size) is given it.
    rig.rex("com.superlogical.terminal.resize", json!({ "session_id": rig.session, "block_id": blocks[2], "args": { "columns": 91, "rows": 33, "cell_width_px": 8, "cell_height_px": 16 } }));
    let shown = size_of(&blocks[2]);
    assert_eq!((shown["columns"].as_u64(), shown["rows"].as_u64()), (Some(91), Some(33)), "{shown}");
    // The service started again finds its children, and their tabs are still nobody's.
    let (code, said, err) = rig.olo(&["retire"]);
    assert_eq!(code, 0, "{said}{err}");
    let listed = rig.json(&["list", "--fast", "--json"]);
    assert_eq!(listed["subagents"].as_array().map(Vec::len), Some(3), "{listed}");
    for block in &blocks {
        assert!(size_of(block)["owner"].is_null(), "after the service started again: {}", size_of(block));
    }
    let again = size_of(&blocks[2]);
    assert_eq!((again["columns"].as_u64(), again["rows"].as_u64()), (Some(91), Some(33)), "the size a desktop gave it holds: {again}");
}

/// A command the agent has folded on its screen (`⋯ N lines hidden`) is never
/// `--approve-all-but`'s to allow: it is asked, whatever its conversation file
/// holds. (a) with what the list excepts on a hidden line (`git push`: it was
/// allowed by the lines that showed); (b) with a harmless tail, known whole from
/// the conversation file (allowed for a day by that match: which call is this
/// box's can't be told for certain); (c) with no conversation file. (d) A
/// command shown whole is allowed as before.
#[test]
fn a_folded_command_is_never_allowed_by_a_list() {
    let Some(rig) = Rig::start("folded") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+push\\b\n").unwrap();
    const SID: &str = "5f0c1f0e-4444-4a44-8a44-2f8f0e0a0004";
    let work = rig.work();
    let started = rig.start_child("claude", "worker", &["--cwd", &work, "--session-id", SID, "--approve-all-but", "not-these.txt"]);
    let _ = started;

    let write_pending_bash = |full_cmd: Option<&str>| {
        let folder: String = work.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let dir = rig.dir.join("home/.claude/projects").join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{SID}.jsonl"));
        match full_cmd {
            Some(cmd) => {
                let line = json!({
                    "type": "assistant",
                    "message": {
                        "id": "msg_pending",
                        "role": "assistant",
                        "content": [{ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": cmd } }],
                        "stop_reason": "tool_use"
                    }
                });
                std::fs::write(&path, format!("{line}\n")).unwrap();
            }
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    };
    let folded_asking = |head: &str, hidden: usize, marked: usize| -> String {
        let dashed = "╌".repeat(60);
        let mark = |i: usize| if i == marked { "❯" } else { " " };
        format!(
            "● Bash({head})\n{0}\n Bash command\n Make it so\n{dashed}\n │ {head}\n │ ⋯ {hidden} lines hidden\n{dashed}\n Do you want to proceed?\n {1} 1. Yes\n {2} 2. Yes, and don’t ask again for: {head}\n {3} 3. No\n Esc to cancel · Tab to amend\n",
            rule(),
            mark(0),
            mark(1),
            mark(2)
        )
    };
    let woken = |since: u64, secs: &str| rig.olo(&["watch", "--all", "--since", &since.to_string(), "--on", "approval,question", "--timeout", secs, "--json"]);

    // (a) Folded command whose hidden tail holds `git push`: NO key sent, reaches parent's watch!
    write_pending_bash(Some("cargo test --workspace\necho step 2\ngit push origin main"));
    let c = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let before = rig.keys("worker").len();
    rig.show("worker", &folded_asking("cargo test --workspace", 19, 0));
    let (code, out, err) = woken(c, "15");
    assert_eq!(code, 0, "folded command with hidden git push should wake parent: {out}{err}");
    assert_eq!(rig.keys("worker").len(), before, "list must NOT send Return when hidden tail matches exception");
    let ev: Value = serde_json::from_str(out.trim()).unwrap();
    assert!(ev["approval"]["command"].as_str().is_some_and(|s| s.contains("git push")), "expected whole reconciled command: {ev}");
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", folded_asking("cargo test --workspace", 19, 2));
    let gone = rig.when_typed("worker", b"\r", working());
    rig.json(&["approve", "worker", "--no", "--json"]);
    assert!(moved.join().unwrap() && gone.join().unwrap());
    rig.wait_state("worker", "working");

    // (b) The same folded command with nothing excepted in its hidden tail, whole in the conversation
    // file: asked all the same, no key sent.
    write_pending_bash(Some("cargo test --workspace\necho all checks ok\necho done"));
    let c = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let before = rig.keys("worker").len();
    rig.show("worker", &folded_asking("cargo test --workspace", 19, 0));
    let (code, out, err) = woken(c, "15");
    assert_eq!(code, 0, "a folded command is asked, its whole text known or not: {out}{err}");
    assert_eq!(rig.keys("worker").len(), before, "and no Return is sent for it");
    let gone = rig.when_typed("worker", b"\r", working());
    rig.json(&["approve", "worker", "--once", "--json"]);
    assert!(gone.join().unwrap());
    rig.wait_state("worker", "working");

    // (c) Folded command with NO transcript to match: NO key sent, wakes parent!
    write_pending_bash(None);
    let c = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let before = rig.keys("worker").len();
    rig.show("worker", &folded_asking("cargo check --workspace", 12, 0));
    let (code, out, err) = woken(c, "15");
    assert_eq!(code, 0, "folded command without transcript match must wake parent: {out}{err}");
    assert_eq!(rig.keys("worker").len(), before, "list must NOT send Return when folded command has no transcript match");
    let gone = rig.when_typed("worker", b"\r", working());
    rig.json(&["approve", "worker", "--once", "--json"]);
    assert!(gone.join().unwrap());
    rig.wait_state("worker", "working");

    // (d) Unfolded command (not cut): allowed once as before, even without transcript.
    let c = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    rig.show("worker", &asking("cargo fmt --check", 0));
    assert!(gone.join().unwrap(), "unfolded command should still be allowed");
    rig.typed_since("worker", before, b"\r", "Return on Yes once");
    assert_eq!(woken(c, "1").0, 124, "unfolded clean command should not wake parent");
    rig.json(&["stop", "worker", "--force", "--json"]);
}

/// `prune` takes the records of the asking tab's own exited children, and nobody else's. (2026-11-27: it took
/// every exited record of the session, and two leads' tabs are one session: 124 records went with one check
/// child's, most of them another lead's.)
#[test]
fn prune_takes_the_asking_tabs_own_records_and_no_others() {
    let Some(rig) = Rig::start("prune") else { return };
    // Another lead, in a tab of its own in the same session.
    let other = rig.rex(
        "session.new_window",
        json!({ "session_id": rig.session, "window_label": "other lead", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 110, "rows": 40 } } } } }),
    );
    let other_block = other["block_ids"][0].as_str().unwrap().to_owned();
    let start = |block: &str, name: &str| {
        let (code, out, err) = rig.olo_from(Some(block), &["start", "--command", "--name", name, "--json", "--", "true"]);
        assert_eq!(code, 0, "{out}{err}");
    };
    let names = |listed: &Value| -> Vec<String> {
        let mut names: Vec<String> = listed["subagents"].as_array().unwrap().iter().map(|s| format!("{} {}", s["name"].as_str().unwrap(), s["state"].as_str().unwrap())).collect();
        names.sort();
        names
    };
    // Each lead has children that have exited; mine has one that still runs, and one's own child that exited.
    start(&rig.lead_block, "mine-1");
    start(&rig.lead_block, "mine-2");
    start(&other_block, "theirs-1");
    start(&other_block, "theirs-2");
    rig.json(&["start", "--command", "--name", "mine-runs", "--json", "--", "sleep", "300"]);
    rig.wait("the four to have exited", || {
        let all = rig.json(&["list", "--all", "--fast", "--json"]);
        all["subagents"].as_array().unwrap().iter().filter(|s| s["state"] == "exited").count() == 4
    });

    // Asked from my tab: my two, and nothing of the other lead's, nor anything that runs.
    let (code, said, _) = rig.olo(&["prune"]);
    assert!(code == 0 && said.starts_with("Pruned 2 exited subagent record(s) of this tab's own children. 2 of other tabs' are left as they are."), "{said}");
    assert_eq!(names(&rig.json(&["list", "--all", "--fast", "--json"])), ["mine-runs working", "theirs-1 exited", "theirs-2 exited"]);
    // Again: nothing more of mine to take; theirs still left.
    let pruned = rig.json(&["prune", "--json"]);
    assert_eq!((&pruned["pruned"], &pruned["left"], &pruned["scope"]), (&json!(0), &json!(2), &json!("own")), "{pruned}");
    // From a shell that is no tab: nothing is its own.
    let (code, out, _) = rig.olo_from(None, &["prune", "--json"]);
    assert!(code == 0 && out.contains("\"pruned\":0"), "{out}");
    // A word it doesn't know is refused, not taken for a yes.
    let (code, _, err) = rig.olo(&["prune", "--everything"]);
    assert!(code == 2 && err.contains("unknown option"), "{err}");
    assert_eq!(names(&rig.json(&["list", "--all", "--fast", "--json"])).len(), 3);
    // Every tab's of the session, only when asked for by that name.
    let (code, said, _) = rig.olo(&["prune", "--session"]);
    assert!(code == 0 && said.starts_with("Pruned 2 exited subagent record(s) of this session, every tab's."), "{said}");
    assert_eq!(names(&rig.json(&["list", "--all", "--fast", "--json"])), ["mine-runs working"]);
    rig.json(&["stop", "mine-runs", "--force", "--json"]);
}

/// Three agents started with `--minder` share ONE minder and nest under
/// it (`lead ▸ minder ▸ {alpha, beta, gamma}`, N + 1 = 4 child tabs);
/// two agents asking the exact same command are answered differently by
/// their own rules; a fourth agent attached by `--minder NAME` reaches
/// the running minder via a `minding` (`added`) event; one agent's exit
/// leaves the minder running for the rest; an unminded agent stays
/// directly under the lead and is unreachable to the minder.
#[test]
fn a_shared_minder_minds_multiple_agents_by_their_own_rules() {
    let Some(rig) = Rig::start("shared") else { return };
    rig.stand_in("claude");

    // An unminded agent: stays directly under the lead, no minder started.
    let solo = rig.start_child("claude", "solo", &[]);
    assert_eq!((&solo["parent_window_id"], &solo["minder"]), (&json!(rig.lead_window), &Value::Null), "{solo}");

    Minding::expect(&rig, "shared-m");
    let rules_a = "Approve `cargo test`.";
    let rules_b = "Refuse `cargo test` and tell it to run `cargo check` instead.";
    let rules_c = "Escalate everything.";
    let a = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", rules_a, "--minder-env", "STAND_IN_NAME=shared-m"]);
    let b = rig.start_child("claude", "beta", &["--minder", "claude:stand-in-big:max", "--rules-text", rules_b]);
    let c = rig.start_child("claude", "gamma", &["--minder", "claude:stand-in-big:max", "--rules-text", rules_c]);

    let minder_id = a["minder"]["id"].as_str().unwrap().to_owned();
    let minder_win = a["minder"]["window_id"].as_str().unwrap().to_owned();
    assert_eq!((&a["minder"]["reused"], &b["minder"]["reused"], &c["minder"]["reused"]), (&json!(false), &json!(true), &json!(true)), "{a}\n{b}\n{c}");
    assert_eq!((&b["minder"]["id"], &c["minder"]["id"]), (&json!(minder_id), &json!(minder_id)), "all 3 share one minder");

    // Tree shape in `list --json`: lead ▸ minder ▸ {alpha, beta, gamma}, plus solo directly under lead.
    let listed = rig.json(&["list", "--json"]);
    let subs = listed["subagents"].as_array().unwrap();
    // 1 solo + 1 minder + 3 minded agents = 5 child tabs (the 3 minded agents + 1 minder = N + 1 = 4 tabs).
    assert_eq!(subs.len(), 5, "{listed}");
    let row = |name: &str| subs.iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("missing {name}: {listed}")).clone();
    assert_eq!(
        (&row("minder")["parent_window_id"], &row("minder")["role"], &row("minder")["minds"], &row("minder")["children"]),
        (&json!(rig.lead_window), &json!("minder"), &Value::Null, &json!(3)),
        "{listed}"
    );
    for n in ["alpha", "beta", "gamma"] {
        assert_eq!((&row(n)["parent_window_id"], &row(n)["minded_by"], &row(n)["minder"]), (&json!(minder_win), &json!(minder_id), &json!(minder_id)), "{n}: {listed}");
    }
    assert_eq!((&row("solo")["parent_window_id"], &row("solo")["minded_by"], &row("solo")["minder"]), (&json!(rig.lead_window), &Value::Null, &Value::Null), "{listed}");

    let minder = Minding::of(&rig, "shared-m", &minder_id);
    // `mind list --json` returns its 3 agents (never `solo`) with each agent's own rules file path.
    let mlist = minder.mind("list --json");
    let msubs = mlist["subagents"].as_array().unwrap();
    let mut mnames: Vec<&str> = msubs.iter().filter_map(|s| s["name"].as_str()).collect();
    mnames.sort();
    assert_eq!(mnames, ["alpha", "beta", "gamma"], "{mlist}");
    for (name, id, expected_rules) in [("alpha", a["id"].as_str().unwrap(), rules_a), ("beta", b["id"].as_str().unwrap(), rules_b), ("gamma", c["id"].as_str().unwrap(), rules_c)] {
        let item = msubs.iter().find(|s| s["name"] == name).unwrap();
        let rpath = item["rules"].as_str().unwrap();
        assert_eq!(std::fs::read_to_string(rpath).unwrap(), expected_rules, "{name} rules at {rpath}");
        assert_eq!(rpath, minder.rules_of(id).display().to_string());
    }
    assert!(minder.olo("status solo").contains("status=1"), "minder cannot reach unminded sibling `solo`");

    // Minder receives `minding` (`added`) for all 3 agents in order.
    let mut cur = 0u64;
    for expected in ["alpha", "beta", "gamma"] {
        let ev = minder.mind(&format!("watch --since {cur} --on minding --timeout 15 --json"));
        assert_eq!((&ev["event"], &ev["name"], &ev["minding"]["action"]), (&json!("minding"), &json!(expected), &json!("added")), "{ev}");
        cur = ev["cursor"].as_u64().unwrap();
    }

    // Both `alpha` and `beta` ask the EXACT SAME command (`cargo test`):
    // `alpha` is approved by its rules; `beta` is refused by its rules.
    rig.show("alpha", &asking("cargo test", 0));
    rig.show("beta", &asking("cargo test", 0));
    let mut saw = std::collections::HashSet::new();
    for _ in 0..2 {
        let asked = minder.mind(&format!("watch --since {cur} --on minding,question,approval,idle,error,exit --timeout 15 --json"));
        assert_eq!(asked["event"], "waiting_approval", "{asked}");
        let who = asked["name"].as_str().unwrap().to_owned();
        cur = asked["cursor"].as_u64().unwrap();
        assert!(saw.insert(who.clone()), "duplicate event for {who}: {asked}");
        if who == "alpha" {
            let before = rig.keys("alpha").len();
            let gone = rig.when_typed("alpha", b"\r", working());
            let ok = minder.mind("approve alpha --once --json");
            assert!(gone.join().unwrap() && ok["chosen_label"] == "Yes", "{ok}");
            rig.typed_since("alpha", before, b"\r", "alpha approved once");
        } else {
            assert_eq!(who, "beta");
            let before = rig.keys("beta").len();
            let moved = rig.when_typed("beta", b"\x1b[B\x1b[B", asking("cargo test", 2));
            let gone = rig.when_typed("beta", b"\r", working());
            let no = minder.mind("approve beta --no --reason 'Run cargo check instead.' --json");
            assert!(moved.join().unwrap() && gone.join().unwrap() && no["chosen_label"] == "No", "{no}");
            assert!(rig.keys("beta")[before..].starts_with(b"\x1b[B\x1b[B"));
        }
    }
    assert_eq!(saw.len(), 2);
    // Each agent's own `answers.jsonl` recorded only its own answer.
    let of_a = rig.json(&["mind", "alpha", "--json"]);
    let of_b = rig.json(&["mind", "beta", "--json"]);
    assert_eq!(of_a["answered"].as_array().unwrap().len(), 1, "{of_a}");
    assert_eq!(of_b["answered"].as_array().unwrap().len(), 1, "{of_b}");
    assert!(of_a["answered"][0]["said"].to_string().contains("--once"), "{of_a}");
    assert!(of_b["answered"][0]["said"].to_string().contains("--no"), "{of_b}");

    // Unminded `solo` asking leave wakes the lead's watch directly.
    let solo_from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.show("solo", &asking("ls -la", 0));
    let solo_ev = rig.json(&["watch", "--all", "--since", &solo_from.to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    assert_eq!((&solo_ev["name"], &solo_ev["event"]), (&json!("solo"), &json!("waiting_approval")), "{solo_ev}");

    // 4th agent added while minder runs using `--minder NAME` (no colons): reaches minder via `minding` (`added`).
    let d = rig.start_child("claude", "delta", &["--minder", "minder", "--rules-text", "Approve `cargo fmt`."]);
    assert_eq!((&d["minder"]["id"], &d["minder"]["reused"], &d["parent_window_id"]), (&json!(minder_id), &json!(true), &json!(minder_win)), "{d}");
    let d_ev = minder.mind(&format!("watch --since {cur} --on minding --timeout 15 --json"));
    assert_eq!(
        (&d_ev["event"], &d_ev["name"], &d_ev["minding"]["action"], &d_ev["minding"]["rules"]),
        (&json!("minding"), &json!("delta"), &json!("added"), &json!(minder.rules_of(d["id"].as_str().unwrap()).display().to_string())),
        "{d_ev}"
    );
    cur = d_ev["cursor"].as_u64().unwrap();

    // One agent (`alpha`) exits: minder hears it, escalates a report for `alpha`, and STAYS RUNNING for the rest (past the 3 s timeout!).
    let lead_before_exit = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.tell("alpha", "exit 0");
    let a_exit = minder.mind(&format!("watch --since {cur} --on minding,question,approval,idle,error,exit --timeout 15 --json"));
    assert_eq!((&a_exit["name"], &a_exit["state"]), (&json!("alpha"), &json!("exited")), "{a_exit}");
    minder.mind("escalate alpha 'alpha finished cleanly (1 approval).' --json");
    let heard = rig.json(&["watch", "--all", "--since", &lead_before_exit.to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&heard["name"], &heard["event"], &heard["state"]), (&json!("alpha"), &json!("escalation"), &json!("exited")), "{heard}");
    std::thread::sleep(Duration::from_millis(3500));
    assert_eq!(rig.status(&minder_id)["running"], true, "minder must keep running while beta/gamma/delta live");
}

/// Stopping a shared minder (`stop MINDER`, `stop MINDER --tree`),
/// closing its tab directly, or an API error on the minder NEVER stops
/// its minded agents: each moves back under the minder's parent tab
/// (verified in `list`, `status`, and `agent follow`), unminded, and
/// the lead receives an escalation on each. `stop --tree` on a lead tab
/// (both a child sub-lead and a top-level lead window) STILL stops
/// everything under it.
#[test]
fn stopping_or_losing_a_shared_minder_never_stops_its_agents() {
    let Some(rig) = Rig::start("keepalive") else { return };
    rig.stand_in("claude");

    // Start `agent follow` so we verify every reparenting is pushed live to Rex!
    let mut follow = rig.helper().args(["agent", "follow"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let follow_frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let follow_reader = {
        let (frames, out) = (follow_frames.clone(), follow.stdout.take().unwrap());
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    frames.lock().unwrap().push(v);
                }
            }
        })
    };
    let frame_mark = || follow_frames.lock().unwrap().len();
    let assert_parent_after = |since_idx: usize, agent_id: &str, expected_parent: &str| {
        rig.wait(&format!("follow to report {agent_id} under {expected_parent} after frame {since_idx}"), || {
            follow_frames.lock().unwrap()[since_idx..].iter().rev().find(|f| f["t"] == "agent" && f["agent"]["id"] == agent_id).is_some_and(|f| f["agent"]["parent_window_id"] == expected_parent)
        });
        let last = follow_frames.lock().unwrap()[since_idx..].iter().rev().find(|f| f["t"] == "agent" && f["agent"]["id"] == agent_id).cloned().expect("agent frame after boundary");
        assert_eq!(last["agent"]["parent_window_id"], json!(expected_parent), "last follow frame after {since_idx} has wrong parent: {last}");
    };
    let escalations_after = |since_seq: u64| -> Vec<Value> {
        let Some(service_dir) = std::fs::read_dir(rig.dir.join("agents")).ok().into_iter().flatten().flatten().map(|d| d.path()).find(|d| d.join("events.jsonl").exists()) else {
            return Vec::new();
        };
        std::fs::read_to_string(service_dir.join("events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|ev| ev["seq"].as_u64().unwrap_or(0) > since_seq && ev["event"] == "escalation")
            .collect()
    };

    let m0 = frame_mark();
    Minding::expect(&rig, "km1");
    let a1 = rig.start_child("claude", "a1", &["--minder", "claude:stand-in-big:max", "--rules-text", "r1", "--minder-env", "STAND_IN_NAME=km1"]);
    let a2 = rig.start_child("claude", "a2", &["--minder", "claude:stand-in-big:max", "--rules-text", "r2"]);
    let (a1_id, a2_id) = (a1["id"].as_str().unwrap().to_owned(), a2["id"].as_str().unwrap().to_owned());
    let m1_id = a1["minder"]["id"].as_str().unwrap().to_owned();
    let _m1 = Minding::of(&rig, "km1", &m1_id);
    assert_parent_after(m0, &a1_id, a1["minder"]["window_id"].as_str().unwrap());

    let expect_both_escalated = |since: u64, what: &str| {
        let mut cur = since;
        let mut names = std::collections::HashSet::new();
        for _ in 0..2 {
            let ev = rig.json(&["watch", "--all", "--since", &cur.to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
            assert!(ev["summary"].as_str().is_some_and(|s| s.starts_with("Its minder ended") && s.ends_with("what it asks comes to you again.")), "{what}: {ev}");
            names.insert(ev["name"].as_str().unwrap().to_owned());
            cur = ev["cursor"].as_u64().unwrap();
        }
        assert_eq!(names, ["a1".to_owned(), "a2".to_owned()].into_iter().collect(), "{what}");
        for n in ["a1", "a2"] {
            let st = rig.status(n);
            assert_eq!((&st["running"], &st["parent_window_id"], &st["minder"]), (&json!(true), &json!(rig.lead_window), &Value::Null), "{what} left {n} broken: {st}");
        }
    };

    // 1. `olo agent stop MINDER --force`: only minder stops; a1 & a2 stay alive under lead and lead is told.
    let f1 = frame_mark();
    let from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let stopped = rig.json(&["stop", &m1_id, "--force", "--json"]);
    assert_eq!(stopped["stopped"], json!([m1_id]), "stopping minder must never stop its agents: {stopped}");
    expect_both_escalated(from, "stop MINDER");
    assert_parent_after(f1, &a1_id, &rig.lead_window);
    assert_parent_after(f1, &a2_id, &rig.lead_window);

    let reattach = |stand_in: &str| -> (String, String, Minding<'_>) {
        let fm = frame_mark();
        Minding::expect(&rig, stand_in);
        let stage = format!("STAND_IN_DIR={}", rig.dir.join("stage").display());
        let path = format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display());
        let named = format!("STAND_IN_NAME={stand_in}");
        let r1 = rig.json(&["mind", "a1", "--with", "claude:stand-in-big:max", "--env", &stage, "--env", &path, "--env", &named, "--json"]);
        let r2 = rig.json(&["mind", "a2", "--with", "claude:stand-in-big:max", "--json"]);
        assert_eq!(r1["minder"]["id"], r2["minder"]["id"], "reattached both to one shared minder");
        let mid = r1["minder"]["id"].as_str().unwrap().to_owned();
        let mwin = r1["minder"]["window_id"].as_str().unwrap().to_owned();
        assert_parent_after(fm, &a1_id, &mwin);
        assert_parent_after(fm, &a2_id, &mwin);
        let m = Minding::of(&rig, stand_in, &mid);
        (mid, mwin, m)
    };

    // 2. `olo agent stop MINDER --tree --force`: STILL stops ONLY minder, never a1 or a2!
    let (m2_id, _, _m2) = reattach("km2");
    let f2 = frame_mark();
    let from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let stopped = rig.json(&["stop", &m2_id, "--tree", "--force", "--json"]);
    assert_eq!(stopped["stopped"], json!([m2_id]), "stop MINDER --tree must never stop its agents: {stopped}");
    expect_both_escalated(from, "stop MINDER --tree");
    assert_parent_after(f2, &a1_id, &rig.lead_window);
    assert_parent_after(f2, &a2_id, &rig.lead_window);

    // 3. Minder's tab closed directly on Rex server (`session.close_window`): a1 & a2 stay alive under lead, lead is told.
    let (_, m3_win, _m3) = reattach("km3");
    let f3 = frame_mark();
    let from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.rex("session.close_window", json!({ "session_id": rig.session, "window_id": m3_win }));
    expect_both_escalated(from, "minder tab closed");
    assert_parent_after(f3, &a1_id, &rig.lead_window);
    assert_parent_after(f3, &a2_id, &rig.lead_window);

    // 4. Minder hits an API error (process stays alive at prompt in `error` state): a1 & a2 released under lead, lead told, and next `--minder` starts a fresh healthy minder.
    let (m4_id, _, _m4) = reattach("km4");
    let f4 = frame_mark();
    let from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.show("km4", &format!("● Working\n  ⎿  API Error: Connection lost mid-response\n{}\n❯ \n", rule()));
    expect_both_escalated(from, "minder API error");
    assert_parent_after(f4, &a1_id, &rig.lead_window);
    assert_parent_after(f4, &a2_id, &rig.lead_window);
    Minding::expect(&rig, "km5");
    let fresh = rig.start_child("claude", "a3", &["--minder", "claude:stand-in-big:max", "--rules-text", "r3", "--minder-env", "STAND_IN_NAME=km5"]);
    assert_ne!(fresh["minder"]["id"], json!(m4_id), "must not reuse an errored minder: {fresh}");
    let _m5 = Minding::of(&rig, "km5", fresh["minder"]["id"].as_str().unwrap());

    // 5. `stop --tree` on a child lead tab (`sublead ▸ minder ▸ {s1, s2}`) stops ALL of them quietly (zero escalations).
    let sublead = rig.start_child("claude", "sublead", &[]);
    let sub_block = sublead["block_id"].as_str().unwrap().to_owned();
    Minding::expect(&rig, "sub-m");
    let s1 = rig.start_child_from(Some(&sub_block), "claude", "s1", &["--minder", "claude:stand-in-big:max", "--rules-text", "rs1", "--minder-env", "STAND_IN_NAME=sub-m"]);
    let s2 = rig.start_child_from(Some(&sub_block), "claude", "s2", &["--minder", "claude:stand-in-big:max", "--rules-text", "rs2"]);
    let sub_minder_id = s1["minder"]["id"].as_str().unwrap().to_owned();
    assert_eq!(s2["minder"]["id"], json!(sub_minder_id));
    let _sm = Minding::of(&rig, "sub-m", &sub_minder_id);
    let sub_from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let stopped_sub = rig.json(&["stop", "sublead", "--tree", "--force", "--json"]);
    assert_eq!(stopped_sub["stopped"].as_array().unwrap().len(), 4, "sublead + minder + s1 + s2: {stopped_sub}");
    for id in [sublead["id"].as_str().unwrap(), sub_minder_id.as_str(), s1["id"].as_str().unwrap(), s2["id"].as_str().unwrap()] {
        assert_eq!(rig.status(id)["running"], false, "{id} should be stopped");
    }
    assert!(escalations_after(sub_from).is_empty(), "recursive stop sublead --tree must emit zero escalations: {:?}", escalations_after(sub_from));

    // 6. `stop <lead_window> --tree` on the top-level lead's own tab stops every descendant quietly (zero escalations).
    let top_from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let stopped_top = rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);
    assert!(stopped_top["stopped"].as_array().unwrap().len() >= 4, "all remaining children under lead stopped: {stopped_top}");
    assert!(rig.json(&["list", "--json"])["subagents"].as_array().unwrap().is_empty(), "nothing left live under lead");
    assert!(escalations_after(top_from).is_empty(), "recursive stop lead --tree must emit zero escalations: {:?}", escalations_after(top_from));

    drop(follow.stdin.take());
    let _ = follow.wait();
    let _ = follow_reader.join();
}

/// `OLO_REX_AGENTS_MINDS_MOST` caps how many live agents one shared
/// minder minds (default 12; set to 2 here): the 3rd agent spawns a 2nd
/// minder (`minder-2`); targeting a full minder by `--minder NAME` is
/// refused; and giving a new agent to an empty minder before its
/// shutdown timeout cancels its shutdown.
#[test]
fn a_shared_minder_splits_at_minds_most_and_cancels_shutdown_when_reused() {
    let Some(rig) = Rig::start_with_env("most", &[("OLO_REX_AGENTS_MINDS_MOST", "2"), ("OLO_REX_AGENTS_MINDER_REPORTS_MS", "5000")]) else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "cap-m1");
    let w1 = rig.start_child("claude", "w1", &["--minder", "claude:stand-in-big:max", "--rules-text", "r1", "--minder-env", "STAND_IN_NAME=cap-m1"]);
    let w2 = rig.start_child("claude", "w2", &["--minder", "claude:stand-in-big:max", "--rules-text", "r2"]);
    assert_eq!(w1["minder"]["id"], w2["minder"]["id"]);
    let m1_id = w1["minder"]["id"].as_str().unwrap().to_owned();
    let _m1 = Minding::of(&rig, "cap-m1", &m1_id);

    // Explicitly naming the full minder is refused cleanly.
    let (code, _, err) = rig.olo(&["start", "claude", "--model", "stand-in", "--effort", "none", "--name", "too-many", "--minder", "minder"]);
    assert!(code == 1 && err.contains("the most is 2"), "{code}: {err}");

    // The 3rd agent with `--minder claude:stand-in-big:max` spawns a 2nd minder (`minder-2`).
    Minding::expect(&rig, "cap-m2");
    let w3 = rig.start_child("claude", "w3", &["--minder", "claude:stand-in-big:max", "--rules-text", "r3", "--minder-env", "STAND_IN_NAME=cap-m2"]);
    let m2_id = w3["minder"]["id"].as_str().unwrap().to_owned();
    assert_ne!(m2_id, m1_id, "3rd agent must get a 2nd minder when MINDS_MOST=2");
    assert_eq!(w3["minder"]["name"], "minder-2", "{w3}");
    let _m2 = Minding::of(&rig, "cap-m2", &m2_id);

    // Last agent of `minder-2` exits -> 5 s countdown starts. Give it `w4` before the timer fires -> countdown cancelled!
    rig.tell("w3", "exit 0");
    rig.wait_state("w3", "exited");
    std::thread::sleep(Duration::from_millis(500));
    let w4 = rig.start_child("claude", "w4", &["--minder", "minder-2", "--rules-text", "r4"]);
    assert_eq!(w4["minder"]["id"], json!(m2_id), "w4 attached to minder-2 before timeout: {w4}");
    std::thread::sleep(Duration::from_millis(5200));
    assert_eq!(rig.status(&m2_id)["running"], true, "giving minder-2 a new agent must cancel its empty-minder shutdown");

    // Once `w4` exits too and nothing replaces it, `minder-2` ends itself after ~5 s.
    rig.tell("w4", "exit 0");
    rig.wait_state("w4", "exited");
    rig.wait("empty minder-2 to end after its time", || rig.status(&m2_id)["running"] == false);
    assert_eq!(rig.status(&m1_id)["running"], true, "minder-1 still has w1 and w2");
}

/// An older 1:1 minder (`minds: Some(agent_id)`, nested under its one
/// agent, with `minders/AGENT_ID/mind`) already present in `registry.json`
/// when the new service starts keeps minding its agent end-to-end:
/// watches, approves, hides approvals from the lead, escalates, refuses
/// `list` (old 1:1 gate), and stops cleanly on `--off`.
#[test]
fn an_old_kind_minder_in_the_registry_keeps_minding_after_takeover() {
    let Some(rig) = Rig::start("legacy") else { return };
    rig.stand_in("claude");

    // Create a worker under lead, and a child tab under worker (the old 1:1 shape).
    let worker = rig.start_child("claude", "worker", &[]);
    let worker_id = worker["id"].as_str().unwrap().to_owned();
    let worker_block = worker["block_id"].as_str().unwrap().to_owned();
    let old_m = rig.start_child_from(Some(&worker_block), "claude", "worker-minder", &[]);
    let old_m_id = old_m["id"].as_str().unwrap().to_owned();

    // Retire the service cleanly and wait for its process to exit completely (after its final `write_now`),
    // then patch `registry.json` to the exact old 1:1 minder schema (`minds: Some(worker_id)`, no `minded_by`) and write the legacy `minders/WORKER_ID/mind` script.
    let old_pid = rig.service_pid().expect("service pid before retire");
    let service_dir = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("registry.json").exists()).expect("service dir");
    let retired = rig.helper().args(["agent", "retire"]).output().unwrap();
    assert!(retired.status.success(), "{}", String::from_utf8_lossy(&retired.stderr));
    rig.wait("retired service process to exit", || !Path::new(&format!("/proc/{old_pid}")).exists());
    let reg_path = service_dir.join("registry.json");
    let mut reg: Value = serde_json::from_str(&std::fs::read_to_string(&reg_path).unwrap()).unwrap();
    for entry in reg["entries"].as_array_mut().unwrap() {
        if entry["id"] == old_m_id {
            entry["role"] = json!("minder");
            entry["minds"] = json!(worker_id);
        }
        entry.as_object_mut().unwrap().remove("minded_by");
    }
    std::fs::write(&reg_path, serde_json::to_string_pretty(&reg).unwrap()).unwrap();
    let mind_dir = service_dir.join("minders").join(&worker_id);
    std::fs::create_dir_all(&mind_dir).unwrap();
    std::fs::write(mind_dir.join("rules.md"), "Approve `cargo test`.\n").unwrap();
    let script_path = mind_dir.join("mind");
    std::fs::write(&script_path, olo_rex_link::agents::minder::script(HELPER, &worker_id)).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700)).unwrap();

    // Next `olo agent` starts the new service over the legacy registry.
    let of = rig.json(&["mind", "worker", "--json"]);
    assert_eq!(of["minder"]["id"], json!(old_m_id), "new service recognizes old 1:1 minder: {of}");

    let legacy = Minding::of(&rig, "worker-minder", &worker_id);
    // Old 1:1 gate still refuses `list` (code 69).
    assert!(legacy.olo("list --json").contains("status=69"), "old 1:1 minder must still be refused `list`");

    // Worker asks leave: hidden from lead, delivered to old-kind minder's legacy `mind watch`, approved via legacy `mind approve --once` (no agent name!).
    let from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("cargo test", 0));
    let asked = legacy.mind(&format!("watch --since {from} --on question,approval --timeout 15 --json"));
    assert_eq!((&asked["name"], &asked["event"]), (&json!("worker"), &json!("waiting_approval")), "{asked}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &from.to_string(), "--on", "approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "lead should not be woken while old-kind minder runs: {out}");

    let gone = rig.when_typed("worker", b"\r", working());
    let ap = legacy.mind("approve --once --json");
    assert!(gone.join().unwrap() && ap["dismissed"] == true, "{ap}");

    // Legacy `mind escalate "TEXT"` (no agent name) escalates about `worker`.
    let up = legacy.mind("escalate 'Legacy minder note.' --json");
    assert_eq!((&up["event"], &up["name"]), (&json!("escalation"), &json!("worker")), "{up}");
    let heard = rig.json(&["watch", "--all", "--since", &from.to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&heard["name"], &heard["summary"]), (&json!("worker"), &json!("Legacy minder note.")), "{heard}");

    // `--off` stops the old-kind minder directly, as before.
    let off = rig.json(&["mind", "worker", "--off", "--json"]);
    assert_eq!((&off["stopped"], &off["minder"]), (&json!(old_m_id), &Value::Null), "{off}");
    rig.wait("old-kind minder to stop on --off", || rig.status(&old_m_id)["running"] == false);
}

/// Measures the service at rest for 10 seconds with 1 shared minder
/// (actively holding `mind watch`) and 3 live agents, verifying both
/// tab count (`N + 1 = 4` child tabs; 5 Rex windows including lead)
/// and idle efficiency (near-zero CPU and wakeups/s).
#[test]
fn a_shared_minder_and_three_agents_cost_four_tabs_and_rest_quietly() {
    let Some(rig) = Rig::start("idlecost") else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "idle-m");
    let a = rig.start_child("claude", "a", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=idle-m"]);
    let b = rig.start_child("claude", "b", &["--minder", "claude:stand-in-big:max", "--rules-text", "r"]);
    let c = rig.start_child("claude", "c", &["--minder", "claude:stand-in-big:max", "--rules-text", "r"]);
    let minder_id = a["minder"]["id"].as_str().unwrap().to_owned();
    assert_eq!((&b["minder"]["id"], &c["minder"]["id"]), (&json!(minder_id), &json!(minder_id)));
    let minder = Minding::of(&rig, "idle-m", &minder_id);

    // Tab count: N = 3 agents + 1 minder = N + 1 = 4 child tabs (5 Rex windows including the lead tab).
    let listed = rig.json(&["list", "--json"]);
    assert_eq!(listed["subagents"].as_array().unwrap().len(), 4, "3 agents + 1 minder = 4 child tabs: {listed}");
    let view = rig.rex("session.view", json!({ "session_id": rig.session }));
    assert_eq!(view["windows"].as_array().unwrap().len(), 5, "1 lead + 1 minder + 3 agents = 5 windows: {view}");

    // Drain the 3 initial `minding` events so the minder's watch sits genuinely blocked at rest.
    let mut cur = 0u64;
    for _ in 0..3 {
        let ev = minder.mind(&format!("watch --since {cur} --on minding --timeout 15 --json"));
        cur = ev["cursor"].as_u64().unwrap();
    }
    // Let all 3 agents rest at prompt and settle startup tasks before measuring.
    for n in ["a", "b", "c"] {
        rig.show(n, &idle("Ready."));
        rig.wait_state(n, "idle");
    }
    // Spawn a real blocking `mind watch` as the minder and verify it is
    // genuinely connected and blocked in the service BEFORE sampling 10 s at rest.
    let minder_block = rig.status(&minder_id)["block_id"].as_str().unwrap().to_owned();
    let mut watch_cmd = Command::new(minder.dir.join("mind"));
    Rig::scratch_env(&rig.dir, &rig.sock, &mut watch_cmd);
    let mut watch_child = watch_cmd
        .env("REX_SESSION", &rig.session)
        .env("REX_BLOCK", &minder_block)
        .args(["watch", "--since", &cur.to_string(), "--on", "minding,question,approval,error,exit", "--timeout", "25", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn blocking mind watch");
    let pid = rig.service_pid().expect("service pid");
    // Wait until the watch process has connected and is sitting idle (still running after startup settles).
    let watch_pid = watch_child.id();
    rig.wait("mind watch to block on service socket", || {
        let sleeping = std::fs::read_to_string(format!("/proc/{watch_pid}/stat")).ok().and_then(|s| s.rsplit_once(") ").map(|(_, r)| r.starts_with("S "))).unwrap_or(false);
        sleeping && watch_child.try_wait().unwrap().is_none()
    });
    std::thread::sleep(Duration::from_millis(800));
    assert!(watch_child.try_wait().unwrap().is_none(), "mind watch exited before idle window began");

    let sample = |pid: i32| -> (u64, u64, u64) {
        let (mut ticks, mut vol, mut inv) = (0u64, 0u64, 0u64);
        for tid in std::fs::read_dir(format!("/proc/{pid}/task")).unwrap().flatten() {
            if let Ok(st) = std::fs::read_to_string(tid.path().join("stat"))
                && let Some((_, rest)) = st.rsplit_once(") ")
            {
                let f: Vec<&str> = rest.split_whitespace().collect();
                if f.len() > 12 {
                    ticks += f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0);
                }
            }
            if let Ok(status) = std::fs::read_to_string(tid.path().join("status")) {
                for line in status.lines() {
                    if let Some(v) = line.strip_prefix("voluntary_ctxt_switches:") {
                        vol += v.trim().parse::<u64>().unwrap_or(0);
                    } else if let Some(v) = line.strip_prefix("nonvoluntary_ctxt_switches:") {
                        inv += v.trim().parse::<u64>().unwrap_or(0);
                    }
                }
            }
        }
        (ticks, vol, inv)
    };
    let (t0, v0, i0) = sample(pid);
    let wall = Instant::now();
    std::thread::sleep(Duration::from_secs(10));
    let secs = wall.elapsed().as_secs_f64();
    let (t1, v1, i1) = sample(pid);
    // Confirm the watch stayed genuinely blocked for the entire 10 s window,
    // then trigger an approval on `a` to prove the watch was live on the service.
    assert!(watch_child.try_wait().unwrap().is_none(), "mind watch exited during idle window");
    rig.show("a", &asking("cargo test", 0));
    let woke = watch_child.wait_with_output().expect("mind watch output");
    assert!(woke.status.success(), "mind watch failed to wake on approval: {}", String::from_utf8_lossy(&woke.stderr));
    let woke_ev: Value = serde_json::from_slice(&woke.stdout).expect("watch json");
    assert_eq!((&woke_ev["name"], &woke_ev["state"]), (&json!("a"), &json!("waiting_approval")), "watch woke on wrong event: {woke_ev}");

    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let cpu_pct = (t1.saturating_sub(t0) as f64) / hz / secs * 100.0;
    let vol_delta = v1.saturating_sub(v0);
    let inv_delta = i1.saturating_sub(i0);
    let wakeups_per_s = (vol_delta as f64) / secs;
    eprintln!("IDLE_MEASUREMENT: pid={pid} over {secs:.2}s: cpu={cpu_pct:.2}% wakeups={vol_delta} ({wakeups_per_s:.2}/s) preempted={inv_delta} child_tabs=4 session_windows=5");
    assert!(cpu_pct < 1.0, "service used {cpu_pct:.2}% CPU at rest over {secs:.2}s");
    assert!(wakeups_per_s < 0.5, "service woke {vol_delta} times ({wakeups_per_s:.2}/s) at rest over {secs:.2}s (rules out any 1 Hz poll)");
}

/// Round 2 (#1, #8, #10, #29):
/// - #1: `--role minder` (CLI and JSON) refused; forged `role: "minder"` is not reused as a minder.
/// - #8: exited record cannot act as caller via its closed `REX_BLOCK`; minder `status`/`read` on an exited ward serves frozen exit info without rereading a resumed transcript.
/// - #10: `--minder NAME` / `--with NAME` resolve strictly among the asking tab's own minders; moving a sub-lead under a minder below it is refused (no cycle).
/// - #29: automatic minder naming skips names colliding with the worker being started or workers nested under existing minders; moving a duplicate-named worker under one minder is refused.
#[test]
fn round2_1_8_10_29_authority_exited_caller_named_scope_cycles_and_names() {
    let Some(rig) = Rig::start("r2-auth") else { return };
    rig.stand_in("claude");

    // #1: CLI and structured JSON start both refuse `--role minder`.
    let (code, _, err) = rig.olo(&["start", "claude", "--model", "stand-in", "--effort", "none", "--name", "spoof", "--role", "minder"]);
    assert!(code == 2 && err.to_ascii_lowercase().contains("minder"), "CLI --role minder must be refused: {code}: {err}");
    let mut req_cmd = rig.helper().args(["agent", "request"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    req_cmd
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({
                "proto": 1, "op": "start", "args": [], "cwd": rig.dir.join("work"),
                "session": rig.session, "block": rig.lead_block,
                "obj": { "cli": "claude", "model": "stand-in", "effort": "none", "name": "spoof2", "role": "minder" }
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let req_out = req_cmd.wait_with_output().unwrap();
    let req_json: Value = serde_json::from_slice(&req_out.stdout).unwrap();
    assert!(req_json["ok"] == false && req_json["code"] == 2, "JSON role=minder must be refused: {req_json}");

    // #8a: an exited child's closed `REX_BLOCK` confers zero live authority.
    let short = rig.start_child("claude", "short", &[]);
    let short_block = short["block_id"].as_str().unwrap().to_owned();
    rig.json(&["stop", "short", "--force", "--json"]);
    let (code, _, err) = rig.olo_from(Some(&short_block), &["list", "--json"]);
    assert_eq!(code, 69, "exited caller must be refused (69): {code}: {err}");

    // #8b: minder calling `status` on an exited ward whose transcript later grows (resumed outside roster) must NOT read the new run's message.
    let sid = "11111111-2222-3333-4444-555555555555";
    rig.says(sid, "do work", "OLD_WARD_REPLY");
    Minding::expect(&rig, "m-exit");
    let ward = rig.start_child("claude", "ward", &["--session-id", sid, "--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=m-exit"]);
    let ward_id = ward["id"].as_str().unwrap().to_owned();
    let m_exit_id = ward["minder"]["id"].as_str().unwrap().to_owned();
    let m_exit = Minding::of(&rig, "m-exit", &m_exit_id);
    // Keep a second live worker so `m-exit` stays alive while we inspect exited `ward`.
    let _keep = rig.start_child("claude", "keep", &["--minder", "claude:stand-in-big:max", "--rules-text", "r"]);
    rig.show("ward", &idle("OLD_WARD_REPLY"));
    rig.wait_state("ward", "done");
    rig.tell("ward", "exit 0");
    rig.wait_state("ward", "exited");
    // Simulate lead resuming that conversation outside the roster and writing a secret new reply to the shared transcript.
    rig.says(sid, "secret resumed prompt", "SECRET_RESUMED_OUTSIDE_ROSTER");
    let st_ward = m_exit.mind(&format!("status {ward_id} --json"));
    assert_eq!(st_ward["state"], "exited", "{st_ward}");
    assert!(!st_ward["last_message"].as_str().unwrap_or("").contains("SECRET_RESUMED_OUTSIDE_ROSTER"), "minder must not reread resumed transcript of exited ward: {st_ward}");
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);

    // #10a: two top-level leads in one session — Lead 1 cannot attach to Lead 2's minder by `--minder minder`.
    let lead2_win_json = rig.rex(
        "session.new_window",
        json!({ "session_id": rig.session, "window_label": "lead2", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 110, "rows": 40 } } } } }),
    );
    let lead2_block = lead2_win_json["block_ids"][0].as_str().unwrap().to_owned();
    Minding::expect(&rig, "m-lead2");
    let _l2w = rig.start_child_from(Some(&lead2_block), "claude", "l2w", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=m-lead2"]);
    // Lead 1 has NO minder of its own; naming `--minder minder` must NOT steal Lead 2's minder!
    let (code, _, err) = rig.olo(&["start", "claude", "--model", "stand-in", "--effort", "none", "--name", "l1w", "--minder", "minder"]);
    assert!(code != 0, "Lead 1 must not resolve Lead 2's minder by name: {code}: {err}");
    // Cleanup lead2's tree from lead2's own block.
    rig.olo_from(Some(&lead2_block), &["stop", lead2_win_json["window_id"].as_str().unwrap(), "--tree", "--force", "--json"]);

    // #10b: moving a sub-lead under a minder below that sub-lead must be refused (no cycle!).
    let sublead = rig.start_child("claude", "sublead", &[]);
    let sub_block = sublead["block_id"].as_str().unwrap().to_owned();
    Minding::expect(&rig, "sub-m");
    let _sw = rig.start_child_from(Some(&sub_block), "claude", "sw", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=sub-m"]);
    let sub_m_id = _sw["minder"]["id"].as_str().unwrap().to_owned();
    let (code, _, err) = rig.olo(&["mind", "sublead", "--with", &sub_m_id]);
    assert!(code != 0, "moving sublead under its own descendant minder must be refused: {code}: {err}");
    rig.json(&["stop", "sublead", "--tree", "--force", "--json"]);

    // #29: worker named `minder` with auto-created minder must not collide with the minder's name (`minder-2`).
    Minding::expect(&rig, "auto-m1");
    let w_minder = rig.start_child("claude", "minder", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=auto-m1"]);
    assert_ne!(w_minder["minder"]["name"], json!("minder"), "auto minder name collided with worker named `minder`: {w_minder}");
    // Start a worker named `minder-3` under `auto-m1` and then a second minder of another effort -> must skip `minder-3`!
    let _w_m3 = rig.start_child("claude", "minder-3", &["--minder", "claude:stand-in-big:max", "--rules-text", "r"]);
    Minding::expect(&rig, "auto-m2");
    let w_other = rig.start_child("claude", "w-other", &["--minder", "claude:stand-in-big:high", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=auto-m2"]);
    assert_ne!(w_other["minder"]["name"], json!("minder-3"), "auto minder name collided with nested worker `minder-3`: {w_other}");
    assert_ne!(w_other["minder"]["name"], json!("minder"), "auto minder name collided with nested worker `minder`: {w_other}");
}

/// Round 2 (#2, #3, #4, #5):
/// - #2: minder `watch --all --match` ignores unminded grandchildren; named `mind watch alpha` ends with words when `alpha` stops being minded by it.
/// - #3: minder process calling `follow`, `adopt`, `retire`, `ping` directly is refused (code 69).
/// - #4: `mind send` and minder `send --wait --out` / `--file` are refused and write no host file.
/// - #5: minder `send` on an active approval box (bare `--enter`, `--key Down`, `--ctrl c`, `--force`) is refused with zero input typed; minder `send NAME "answer" --enter` on a recognised non-approval `WaitingQuestion` succeeds once told.
#[test]
fn round2_2_3_4_5_minder_gates_watches_send_and_non_approval_questions() {
    let Some(rig) = Rig::start("r2-gates") else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "gm");
    let alpha =
        rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "Approve cargo test. Answer database question with postgres.", "--minder-env", "STAND_IN_NAME=gm"]);
    let alpha_block = alpha["block_id"].as_str().unwrap().to_owned();
    let minder_id = alpha["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "gm", &minder_id);
    // Drain initial `added` event.
    let added = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    assert_eq!(added["minding"]["action"], "added");
    let mut cur = added["cursor"].as_u64().unwrap();

    // #2a: unminded grandchild started by `alpha` must NEVER leak into minder's `watch --all --match`.
    let _grand = rig.start_child_from(Some(&alpha_block), "claude", "grand", &[]);
    let watch_thread = {
        let env = minder.env();
        let mind_bin = minder.dir.join("mind").display().to_string();
        let stage = rig.dir.join("stage");
        let stand_in = minder.stand_in.clone();
        std::thread::spawn(move || {
            let out_file = stage.join("grand-watch.out");
            let fifo = stage.join(format!("{stand_in}.fifo"));
            let cmd =
                format!("run sh -c '{env} {mind_bin} watch --since {cur} --match GRAND_SECRET_TOKEN --timeout 2 --json > {} 2>&1; echo status=$? >> {}'\n", out_file.display(), out_file.display());
            std::fs::OpenOptions::new().write(true).open(&fifo).unwrap().write_all(cmd.as_bytes()).unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(s) = std::fs::read_to_string(&out_file)
                    && s.contains("status=")
                {
                    return s;
                }
                assert!(Instant::now() < until, "grandchild watch did not finish");
                std::thread::sleep(Duration::from_millis(30));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    rig.show("grand", &idle("GRAND_SECRET_TOKEN appeared in grandchild"));
    let grand_watch_out = watch_thread.join().unwrap();
    assert!(grand_watch_out.contains("status=124") && !grand_watch_out.contains("GRAND_SECRET_TOKEN"), "minder text watch leaked grandchild output: {grand_watch_out}");

    // #3: minder process directly requesting `follow`, `adopt`, `retire`, `ping` must be refused (69).
    let pid_before = rig.service_pid().expect("service pid");
    for forbidden in ["retire", "ping", "follow"] {
        let out = minder.olo(forbidden);
        assert!(out.contains("status=69"), "minder must be refused `{forbidden}` with 69: {out}");
    }
    let adopt_out = minder.run(&format!("printf '{{\"agents\":[]}}' | {} {HELPER} agent adopt; echo status=$?", minder.env()));
    assert!(adopt_out.contains("status=69"), "minder must be refused `adopt` with 69: {adopt_out}");
    assert_eq!(rig.service_pid(), Some(pid_before), "minder must not have retired the service");

    // #4: `mind send` and direct `send` with `--wait --out` or `--file` must be refused and write NO file.
    let evil_out = rig.dir.join("minder-wrote-host-file.txt");
    let secret_src = rig.dir.join("host-secret.txt");
    std::fs::write(&secret_src, "HOST_SECRET_42").unwrap();
    rig.show("alpha", &idle("Ready."));
    rig.wait_state("alpha", "idle");
    for bad_send in [
        format!("{} {} send alpha 'hi' --enter --wait --out {} --timeout 2; echo status=$?", minder.env(), minder.dir.join("mind").display(), evil_out.display()),
        format!("{} {} send alpha --file {} --enter; echo status=$?", minder.env(), minder.dir.join("mind").display(), secret_src.display()),
        format!("{} {HELPER} agent send alpha 'hi' --enter --wait --out {} --timeout 2; echo status=$?", minder.env(), evil_out.display()),
    ] {
        let out = minder.run(&bad_send);
        assert!(!out.contains("status=0"), "minder send option must be refused: {bad_send} -> {out}");
    }
    assert!(!evil_out.exists(), "minder must never create host file via --wait --out");

    // #5a: while an approval box is active on `alpha`, minder `send` (bare `--enter`, `--key Down`, `--ctrl c`, `--force`) is refused with ZERO bytes typed.
    rig.show("alpha", &asking("rm -rf /", 0));
    let ap_ev = minder.mind(&format!("watch --since {cur} --on approval --timeout 15 --json"));
    assert_eq!(ap_ev["event"], "waiting_approval", "{ap_ev}");
    cur = ap_ev["cursor"].as_u64().unwrap();
    let keys_before = rig.keys("alpha").len();
    for bypass in [
        format!("{} {} send alpha --enter; echo status=$?", minder.env(), minder.dir.join("mind").display()),
        format!("{} {} send alpha --key Down; echo status=$?", minder.env(), minder.dir.join("mind").display()),
        format!("{} {} send alpha --ctrl c; echo status=$?", minder.env(), minder.dir.join("mind").display()),
        format!("{} {HELPER} agent send alpha '1' --force --enter; echo status=$?", minder.env()),
        format!("{} {HELPER} agent send alpha --key Enter; echo status=$?", minder.env()),
        format!("{} {HELPER} agent send alpha --ctrl c; echo status=$?", minder.env()),
    ] {
        let out = minder.run(&bypass);
        assert!(!out.contains("status=0"), "minder send must not bypass approval dialog: {bypass} -> {out}");
    }
    assert_eq!(rig.keys("alpha").len(), keys_before, "no keys may be typed into alpha during refused minder sends");

    // #5b: recognised non-approval `WaitingQuestion` — `mind send alpha "postgres" --enter` succeeds once told!
    let question_screen = format!("Which database should I configure?\n ❯ 1. Postgres\n   2. SQLite\n Enter to select · ↑/↓ navigate · Esc to cancel\n{}\n", rule());
    rig.show("alpha", &question_screen);
    let q_ev = minder.mind(&format!("watch --since {cur} --on question --timeout 15 --json"));
    assert_eq!(q_ev["event"], "waiting_question", "{q_ev}");
    cur = q_ev["cursor"].as_u64().unwrap();
    let before_q = rig.keys("alpha").len();
    let q_answered = rig.when_typed("alpha", b"postgres", working());
    let sent_q = minder.mind("send alpha 'postgres' --enter --json");
    assert_eq!(sent_q["ok"], json!(true), "minder must be able to answer non-approval WaitingQuestion with text+--enter: {sent_q}");
    assert!(q_answered.join().unwrap(), "alpha did not receive postgres answer");
    assert!(rig.keys("alpha")[before_q..].windows(b"postgres".len()).any(|w| w == b"postgres"));

    // #2b: named `mind watch alpha` ends with words when lead runs `mind alpha --off` (never returns alpha's next question).
    let named_watch = {
        let env = minder.env();
        let mind_bin = minder.dir.join("mind").display().to_string();
        let stage = rig.dir.join("stage");
        let stand_in = minder.stand_in.clone();
        std::thread::spawn(move || {
            let out_file = stage.join("named-watch.out");
            let fifo = stage.join(format!("{stand_in}.fifo"));
            let cmd =
                format!("run sh -c '{env} {mind_bin} watch alpha --since {cur} --on approval,question --timeout 15 --json > {} 2>&1; echo status=$? >> {}'\n", out_file.display(), out_file.display());
            std::fs::OpenOptions::new().write(true).open(&fifo).unwrap().write_all(cmd.as_bytes()).unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(s) = std::fs::read_to_string(&out_file)
                    && s.contains("status=")
                {
                    return s;
                }
                assert!(Instant::now() < until, "named watch did not exit after --off");
                std::thread::sleep(Duration::from_millis(30));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    rig.json(&["mind", "alpha", "--off", "--json"]);
    rig.show("alpha", &asking("echo after-detach", 0));
    let nw_out = named_watch.join().unwrap();
    assert!(!nw_out.contains("status=0") && !nw_out.contains("after-detach"), "named watch must end on detach and never deliver post-detach questions: {nw_out}");
}

/// Round 2 (#6, #7, #9):
/// - #6: updating an agent's rules voids what the minder was told (`told` cleared), refusing an approve decided under old rules with words to re-read rules.
/// - #7: two askings of the exact same command are two box instances (`b1` vs `b2`): a minder grant from `b1` is refused for `b2`.
/// - #9: `stop <OTHER_LEAD_WINDOW> --tree` from top-level Lead 1 cannot stop Lead 2's subtree.
#[test]
fn round2_6_7_9_rules_invalidation_box_instance_and_top_level_stop() {
    let Some(rig) = Rig::start("r2-rules-box") else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "rbm");
    let _a = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "Approve cargo test.", "--minder-env", "STAND_IN_NAME=rbm"]);
    let minder_id = _a["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "rbm", &minder_id);
    let added = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    let cur = added["cursor"].as_u64().unwrap();

    // #6: minder sees approval under old rules -> lead changes rules -> minder tries to approve under old rules -> REFUSED!
    rig.show("alpha", &asking("cargo test", 0));
    let ev1 = minder.mind(&format!("watch --since {cur} --on approval --timeout 15 --json"));
    assert_eq!(ev1["event"], "waiting_approval", "{ev1}");
    rig.json(&["mind", "alpha", "--rules-text", "Refuse cargo test.", "--json"]);
    let keys_before = rig.keys("alpha").len();
    let stale_ap = minder.run(&format!("{} {} approve alpha --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(stale_ap.contains("status=1") && stale_ap.to_ascii_lowercase().contains("rules"), "approve under stale rules must be refused with words about rules: {stale_ap}");
    assert_eq!(rig.keys("alpha").len(), keys_before, "no Enter may be sent under stale rules");

    // #7: deliver the `minding` ("rules") revision first, then re-read status (now told of box b1),
    // then LEAD directly answers b1, then alpha asks the EXACT SAME command (`cargo test`) again (b2) -> minder's grant from b1 must be REFUSED for b2!
    let rules_ev = minder.mind("watch --since 0 --on minding --timeout 10 --json");
    assert_eq!(rules_ev["minding"]["action"], "rules", "{rules_ev}");
    let st1 = minder.mind("status alpha --json");
    let box1 = st1["approval"]["box"].as_str().unwrap().to_owned();
    let gone1 = rig.when_typed("alpha", b"\r", working());
    let lead_ap = rig.json(&["approve", "alpha", "--once", "--json"]);
    assert!(gone1.join().unwrap() && lead_ap["dismissed"] == true, "{lead_ap}");
    rig.wait_state("alpha", "working");
    // Exact same command asked a second time!
    rig.show("alpha", &asking("cargo test", 0));
    rig.wait_state("alpha", "waiting_approval");
    let box2 = rig.status("alpha")["approval"]["box"].as_str().unwrap().to_owned();
    assert_ne!(box1, box2, "second asking of identical command must have a new box instance");
    let keys_before2 = rig.keys("alpha").len();
    let reuse_ap = minder.run(&format!("{} {} approve alpha --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(reuse_ap.contains("status=1"), "minder grant for b1 must NOT authorize identical b2: {reuse_ap}");
    assert_eq!(rig.keys("alpha").len(), keys_before2, "no Enter may be sent to b2 using b1 grant");

    // #9: two top-level leads in same session — Lead 1 calling `stop <lead2_window> --tree` must be refused!
    let lead2_win_json = rig.rex(
        "session.new_window",
        json!({ "session_id": rig.session, "window_label": "lead2", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 110, "rows": 40 } } } } }),
    );
    let lead2_win = lead2_win_json["window_id"].as_str().unwrap().to_owned();
    let lead2_block = lead2_win_json["block_ids"][0].as_str().unwrap().to_owned();
    let b_worker = rig.start_child_from(Some(&lead2_block), "claude", "b-worker", &[]);
    let (code, _, err) = rig.olo(&["stop", &lead2_win, "--tree", "--force"]);
    assert_eq!(code, 69, "Lead 1 must not recursively stop Lead 2's top-level tab: {code}: {err}");
    assert_eq!(rig.status(b_worker["id"].as_str().unwrap())["running"], json!(true), "Lead 2's worker must still be running");
    rig.olo_from(Some(&lead2_block), &["stop", &lead2_win, "--tree", "--force", "--json"]);
}

/// Round 2 (#24, #25, #26):
/// - #24: a standing question whose transcript says `waiting` while the screen looks `idle` is delivered to the minder (never hidden from both lead and minder); a standing question evicted past the 2,000-event log ring is still seen by a newly attached minder.
/// - #25: a targeted `mind watch B --on minding` does not swallow an earlier unread `rules` event for `A`; delivery state persists across service handover (`retire` + restart) without replaying already-delivered roster events.
/// - #26: detaching (`mind --off`) a waiting worker wakes the lead's ongoing watch whose cursor had already advanced past the suppressed approval.
#[test]
fn round2_24_25_26_standing_questions_per_agent_cursors_and_detach_handoff() {
    let Some(rig) = Rig::start("r2-events") else { return };
    rig.stand_in("claude");

    // #24a: worker whose transcript says `waiting` (AskUserQuestion) while screen looks `idle`.
    let sid_q = "22222222-3333-4444-5555-666666666666";
    {
        let folder: String = rig.work().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let dir = rig.dir.join("home/.claude/projects").join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [
            json!({ "type": "user", "message": { "role": "user", "content": "start task" } }),
            json!({
                "type": "assistant",
                "message": {
                    "id": "msg_q", "role": "assistant", "stop_reason": "tool_use",
                    "content": [{ "type": "tool_use", "id": "tu_1", "name": "AskUserQuestion", "input": { "question": "Which branch?" } }]
                }
            }),
        ];
        std::fs::write(dir.join(format!("{sid_q}.jsonl")), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }
    Minding::expect(&rig, "em1");
    let alpha = rig.start_child("claude", "alpha", &["--session-id", sid_q, "--minder", "claude:stand-in-big:max", "--rules-text", "r-alpha-1", "--minder-env", "STAND_IN_NAME=em1"]);
    let minder_id = alpha["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "em1", &minder_id);
    let ev_add = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    assert_eq!((&ev_add["name"], &ev_add["minding"]["action"]), (&json!("alpha"), &json!("added")), "{ev_add}");

    // Show idle prompt on screen while transcript has pending AskUserQuestion -> service records `waiting_question`.
    rig.show("alpha", &idle("Waiting for your choice."));
    rig.wait_state("alpha", "waiting_question");
    // Minder's watch MUST receive `waiting_question` (not discard it because raw screen looks `idle`!).
    let q_ev = minder.mind("watch --since 0 --on question,approval --timeout 10 --json");
    assert_eq!((&q_ev["name"], &q_ev["state"]), (&json!("alpha"), &json!("waiting_question")), "transcript-waiting question was hidden from minder: {q_ev}");

    // #25a: unread `rules` event for `alpha` (seq N) and `added` for `beta` (seq N+1):
    // targeted `mind watch beta --on minding` must NOT swallow `alpha`'s unread `rules` event!
    rig.json(&["mind", "alpha", "--rules-text", "r-alpha-2", "--json"]);
    let _beta = rig.start_child("claude", "beta", &["--minder", "claude:stand-in-big:max", "--rules-text", "r-beta-1"]);
    let b_added = minder.mind("watch beta --since 0 --on minding --timeout 15 --json");
    assert_eq!((&b_added["name"], &b_added["minding"]["action"]), (&json!("beta"), &json!("added")), "{b_added}");
    let a_rules = minder.mind("watch --since 0 --on minding --timeout 5 --json");
    assert_eq!((&a_rules["name"], &a_rules["minding"]["action"]), (&json!("alpha"), &json!("rules")), "targeted watch on beta swallowed alpha's unread rules event: {a_rules}");

    // #25b: service handover (`retire` + restart) preserves minder delivery state so already-delivered `minding` events are NOT replayed.
    let old_pid = rig.service_pid().expect("service pid");
    let ret = rig.helper().args(["agent", "retire"]).output().unwrap();
    assert!(ret.status.success(), "{}", String::from_utf8_lossy(&ret.stderr));
    rig.wait("retired service to exit", || !Path::new(&format!("/proc/{old_pid}")).exists());
    // Trigger new service startup and verify minder does not receive replayed `added`/`rules` events.
    let after_handover = minder.run(&format!("{} {} watch --since 0 --on minding --timeout 2 --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(after_handover.contains("status=124"), "handover replayed already-delivered minding events: {after_handover}");

    // #26: `beta` asks leave while minded -> lead's cursor advances past that approval -> lead runs `mind beta --off` -> lead's ongoing watch wakes with `beta`'s standing approval!
    rig.show("beta", &asking("cargo test --release", 0));
    let b_ap = minder.mind("watch beta --since 0 --on approval --timeout 15 --json");
    assert_eq!((&b_ap["name"], &b_ap["event"]), (&json!("beta"), &json!("waiting_approval")), "{b_ap}");
    let lead_cursor = b_ap["cursor"].as_u64().unwrap();
    let other = rig.start_child("claude", "other", &[]);
    let _ = other;
    rig.show("other", &idle("hello"));
    rig.wait_state("other", "idle");
    rig.wait("other idle event recorded in log", || rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap() > lead_cursor);
    let advanced = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    assert!(advanced > lead_cursor, "lead cursor should be past beta's suppressed approval");
    let lead_watch = {
        let dir = rig.dir.clone();
        let sock = rig.sock.clone();
        let session = rig.session.clone();
        let block = rig.lead_block.clone();
        std::thread::spawn(move || {
            let mut cmd = Command::new(HELPER);
            Rig::scratch_env(&dir, &sock, &mut cmd);
            let out = cmd
                .env("REX_SESSION", &session)
                .env("REX_BLOCK", &block)
                .current_dir(dir.join("work"))
                .args(["agent", "watch", "--all", "--since", &advanced.to_string(), "--on", "approval", "--timeout", "10", "--json"])
                .output()
                .unwrap();
            (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
        })
    };
    std::thread::sleep(Duration::from_millis(250));
    rig.json(&["mind", "beta", "--off", "--json"]);
    let (wcode, wout, werr) = lead_watch.join().unwrap();
    assert_eq!(wcode, 0, "lead ongoing watch stayed silent after --off on waiting beta: {wcode} {wout} {werr}");
    let wv: Value = serde_json::from_str(wout.trim()).unwrap();
    assert_eq!((&wv["name"], &wv["event"]), (&json!("beta"), &json!("waiting_approval")), "{wv}");

    // #24b: standing approval survives >2,000 newer events evicting the event log ring.
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);
    Minding::expect(&rig, "evict-m");
    let ex_h = rig.start_child("claude", "ex-h", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=evict-m"]);
    let ev_mid = ex_h["minder"]["id"].as_str().unwrap().to_owned();
    let ev_m_block = rig.status(&ev_mid)["block_id"].as_str().unwrap().to_owned();
    let ev_m = Minding::of(&rig, "evict-m", &ev_mid);
    let _stand = rig.start_child("claude", "stand-w", &["--minder", &ev_mid, "--rules-text", "r"]);
    rig.tell("ex-h", "exit 0");
    rig.wait_state("ex-h", "exited");
    let before_ap = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    rig.show("stand-w", &asking("cargo build", 0));
    rig.wait_state("stand-w", "waiting_approval");
    rig.wait("stand-w approval recorded", || rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap() > before_ap);
    let ap_seq = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    // Flood >2,050 events over the service socket (about exited `ex-h`, so zero Rex RPCs) to evict `ap_seq` from the 2,000-entry ring!
    let sock_path = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path().join("sock")).find(|p| p.exists()).expect("service sock");
    let req_line = format!(
        "{}\n",
        json!({
            "proto": 1, "op": "escalate", "args": ["ex-h", "ring filler"], "cwd": rig.work(),
            "session": rig.session, "block": ev_m_block, "inherit": [], "from": "test"
        })
    );
    for _ in 0..2050 {
        use std::io::BufRead as _;
        let mut s = std::os::unix::net::UnixStream::connect(&sock_path).unwrap();
        s.write_all(req_line.as_bytes()).unwrap();
        let mut r = std::io::BufReader::new(s);
        let mut line = String::new();
        let _ = r.read_line(&mut line);
        if line.trim().is_empty() {
            line.clear();
            let _ = r.read_line(&mut line);
        }
    }
    let after_flood = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    assert!(after_flood >= ap_seq + 2050, "expected >2000 events past ap_seq={ap_seq}, got {after_flood}");
    // Despite `ap_seq` being evicted from the 2,000-event log ring, `mind watch` MUST still deliver `stand-w`'s standing approval via `live.latest_event`!
    let ev_ap = ev_m.mind("watch stand-w --since 0 --on approval --timeout 10 --json");
    assert_eq!((&ev_ap["name"], &ev_ap["state"]), (&json!("stand-w"), &json!("waiting_approval")), "ring eviction lost stand-w's standing approval: {ev_ap}");
}

/// Round 2 (#27, #28):
/// - #27: when two agents under one minder exit while the minder is busy, reporting the first exit does NOT end the minder after 500 ms before it reports the second exit.
/// - #28: an exited agent's report by immutable ID targets the exited record even after a new live agent reuses the same name; a 2-argument `escalate <unknown> "TEXT"` returns an error instead of silently escalating about the minder itself.
#[test]
fn round2_27_28_multi_exit_reports_and_target_attribution() {
    let Some(rig) = Rig::start_with_env("r2-exits", &[("OLO_REX_AGENTS_MINDER_REPORTS_MS", "10000")]) else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "xm");
    let a1 = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=xm"]);
    let a1_id = a1["id"].as_str().unwrap().to_owned();
    let minder_id = a1["minder"]["id"].as_str().unwrap().to_owned();
    let b1 = rig.start_child("claude", "beta", &["--minder", "claude:stand-in-big:max", "--rules-text", "r"]);
    let b1_id = b1["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "xm", &minder_id);
    for _ in 0..2 {
        let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    }

    // #28b: 2-arg escalate with unknown target MUST return an error, never silently escalate about the minder itself!
    let bad_esc = minder.run(&format!("{} {} escalate no-such-worker 'oops' --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(!bad_esc.contains("status=0"), "2-arg escalate with unknown target must fail instead of escalating about minder: {bad_esc}");

    // #28a: `alpha` exits, then a NEW live `alpha` starts under the same minder -> escalating by `a1_id` attributes to `a1_id`, NOT the new live `alpha`!
    rig.tell("alpha", "exit 0");
    rig.wait_state(&a1_id, "exited");
    let _ = std::fs::remove_file(rig.dir.join("stage/alpha.fifo"));
    let _ = std::fs::remove_file(rig.dir.join("stage/alpha.ready"));
    let a2 = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "r2"]);
    let a2_id = a2["id"].as_str().unwrap().to_owned();
    assert_ne!(a1_id, a2_id);
    let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    let esc_old = minder.mind(&format!("escalate {a1_id} 'old alpha summary' --json"));
    assert_eq!(esc_old["id"], json!(a1_id), "escalation by immutable ID must attribute to exited a1_id: {esc_old}");
    assert_eq!(rig.json(&["mind", &a2_id, "--json"])["answered"].as_array().unwrap().len(), 0, "new live alpha must not receive old alpha's audit entry");
    assert_eq!(rig.json(&["mind", &a1_id, "--json"])["answered"].as_array().unwrap().len(), 1);

    // #27: BOTH remaining live agents (`beta` and new `alpha`) exit while minder is busy ->
    // escalating `beta` first must NOT end the minder 500 ms later before `alpha` (`a2_id`) is reported!
    rig.tell("beta", "exit 0");
    rig.tell("alpha", "exit 0");
    rig.wait_state(&b1_id, "exited");
    rig.wait_state(&a2_id, "exited");
    let _ = minder.mind(&format!("escalate {b1_id} 'beta done' --json"));
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(rig.status(&minder_id)["running"], json!(true), "minder ended after first of two exit reports before reporting second exit");
    let reported_all = Instant::now();
    let _ = minder.mind(&format!("escalate {a2_id} 'new alpha done' --json"));
    rig.wait("minder to stop once all exit reports are sent", || rig.status(&minder_id)["running"] == false);
    assert!(reported_all.elapsed() < Duration::from_millis(5000), "minder should stop promptly once final exit report is sent (grace is 10s): {:?}", reported_all.elapsed());
}

/// Round 2 (#31, #34):
/// - #34: a slow `follow` consumer that falls behind the 1,024-frame broadcast buffer does NOT deadlock the service on `RecvError::Lagged` and recovers with a fresh `Frame::Snapshot`.
/// - #31: a late state frame computed before a move cannot overwrite the moved worker's new `parent_window_id` on `follow`.
#[test]
fn round2_31_34_frame_order_after_move_and_slow_follow_lag() {
    let Some(rig) = Rig::start("r2-follow") else { return };
    rig.stand_in("claude");

    // #34: start a slow `follow` consumer, read its initial snapshot, pause draining, then push >1,050 frames via one `adopt` RPC.
    let mut follow = rig.helper().args(["agent", "follow"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut reader = std::io::BufReader::new(follow.stdout.take().unwrap());
    let mut first_line = String::new();
    use std::io::BufRead as _;
    reader.read_line(&mut first_line).unwrap();
    let first: Value = serde_json::from_str(first_line.trim()).unwrap();
    assert_eq!(first["t"], "snapshot", "{first}");

    // Send 1,100 already-exited dummy entries via `adopt` to overflow the 1,024-slot broadcast channel while `follow` is not reading.
    let dummies: Vec<Value> = (0..1100)
        .map(|i| {
            json!({
                "id": format!("sa-lag{i:04}"), "name": format!("lag-{i}"), "host_key": "",
                "session_id": rig.session, "window_id": format!("w-lag-{i}"), "block_id": format!("b-lag-{i}"),
                "parent_window_id": rig.lead_window, "cli": "claude", "model": "m", "effort": "e",
                "title": format!("lag-{i}"), "started_at": 1000, "exited_at": 2000, "seen_live": true
            })
        })
        .collect();
    let mut adopt = rig.helper().args(["agent", "adopt"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    adopt.stdin.take().unwrap().write_all(json!({ "agents": dummies }).to_string().as_bytes()).unwrap();
    let adopt_out = adopt.wait_with_output().unwrap();
    assert!(adopt_out.status.success(), "{}", String::from_utf8_lossy(&adopt_out.stderr));

    // Read frames on a background thread (this triggers `RecvError::Lagged` in the service!) and simultaneously check `ping` does NOT deadlock!
    let (resync_tx, resync_rx) = std::sync::mpsc::channel();
    let got_resync = std::thread::spawn(move || {
        let mut saw_snapshot = false;
        let mut line = String::new();
        for _ in 0..1200 {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                        && v["t"] == "snapshot"
                    {
                        saw_snapshot = true;
                        break;
                    }
                }
            }
        }
        let _ = resync_tx.send(saw_snapshot);
        (saw_snapshot, reader)
    });
    // `ping` must answer quickly (< 4 s), proving the service did NOT deadlock on the lagged branch!
    let ping_at = Instant::now();
    let mut ping_child = rig.helper().args(["agent", "ping"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let ping_ok = loop {
        if let Ok(Some(st)) = ping_child.try_wait() {
            break st.success();
        }
        if ping_at.elapsed() >= Duration::from_secs(4) {
            let _ = ping_child.kill();
            let _ = ping_child.wait();
            break false;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let saw_snapshot = if ping_ok { resync_rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false) } else { false };
    // Always close/kill `follow` BEFORE joining the reader thread so a missed snapshot can never hang the test.
    drop(follow.stdin.take());
    let _ = follow.kill();
    let _ = follow.wait();
    let _ = got_resync.join();
    assert!(ping_ok, "service deadlocked or stalled on lagged follow after {:?}", ping_at.elapsed());
    assert!(saw_snapshot, "lagged follow consumer must receive a fresh resync snapshot");

    // #31: late state frame after a move must carry the NEW `parent_window_id` and never revert to old parent.
    let mut f2 = rig.helper().args(["agent", "follow"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let f2_reader = {
        let (frames, out) = (frames.clone(), f2.stdout.take().unwrap());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    frames.lock().unwrap().push(v);
                }
            }
        })
    };
    let w = rig.start_child("claude", "mover", &[]);
    let w_id = w["id"].as_str().unwrap().to_owned();
    Minding::expect(&rig, "fm1");
    let stage_env = format!("STAND_IN_DIR={}", rig.dir.join("stage").display());
    let path_env = format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display());
    // Trigger a state change on `mover` AND immediately move `mover` under a shared minder while `transition` is awaiting `look`/`CONFIRM`/`inspect`!
    rig.show("mover", &asking("cargo check", 0));
    let moved = rig.json(&["mind", "mover", "--with", "claude:stand-in-big:max", "--rules-text", "r", "--env", &stage_env, "--env", &path_env, "--env", "STAND_IN_NAME=fm1", "--json"]);
    let m_win = moved["minder"]["window_id"].as_str().unwrap().to_owned();
    let m_id = moved["minder"]["id"].as_str().unwrap().to_owned();
    // Wait for the confirmed `waiting_approval` state frame to arrive on `follow`, then verify the LAST frame for `mover` STILL has `parent_window_id == m_win`!
    rig.wait("waiting_approval frame for mover", || frames.lock().unwrap().iter().any(|f| f["t"] == "agent" && f["agent"]["id"] == w_id && f["agent"]["state"] == "waiting_approval"));
    std::thread::sleep(Duration::from_millis(400));
    let last_frame = frames.lock().unwrap().iter().rev().find(|f| f["t"] == "agent" && f["agent"]["id"] == w_id).cloned().expect("frame for mover");
    assert_eq!((&last_frame["agent"]["parent_window_id"], &last_frame["agent"]["minded_by"]), (&json!(m_win), &json!(m_id)), "late state frame reverted mover to old parent: {last_frame}");
    drop(f2.stdin.take());
    let _ = f2.wait();
    let _ = f2_reader.join();
}

/// Round 2 (#12, #13, #14, #15, #16, #17):
/// - #13: an earlier empty-minder timer does NOT shorten a subsequent empty grace period after a worker came and went.
/// - #14: an unwritable rules directory leaves the started worker unminded under the lead with `minder_error` (never minded with no rules); a failed rules rewrite preserves the existing `rules.md` intact.
/// - #16: an errored minder arms an empty timer instead of occupying a tab forever; a service handover during an empty minder's grace period re-arms its empty timer on `reconcile`.
/// - #17: concurrent `start --minder` bursts with `MINDS_MOST=2` convert reservations atomically so both workers share one minder instead of double-counting load=2.
/// - #12 & #15: concurrent moves stay consistent, and a worker exiting during attach or a minder dying during worker launch never commits a broken assignment.
#[test]
fn round2_12_13_14_15_16_17_launch_move_timer_and_rules_races() {
    let Some(rig) = Rig::start_with_env("r2-races", &[("OLO_REX_AGENTS_MINDS_MOST", "2"), ("OLO_REX_AGENTS_MINDER_REPORTS_MS", "8000")]) else { return };
    rig.stand_in("claude");

    // #13: empty timer T1 (8.0 s) must NOT shorten the second empty period when a worker arrives at ~2.5 s and exits shortly after.
    Minding::expect(&rig, "tm1");
    let w1 = rig.start_child("claude", "w1", &["--minder", "claude:stand-in-big:max", "--rules-text", "r1", "--minder-env", "STAND_IN_NAME=tm1"]);
    let tm1_id = w1["minder"]["id"].as_str().unwrap().to_owned();
    let _tm1 = Minding::of(&rig, "tm1", &tm1_id);
    let cur_w1 = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let first_empty = Instant::now();
    rig.tell("w1", "exit 0");
    let _ = rig.json(&["watch", "w1", "--since", &cur_w1.to_string(), "--on", "exit", "--timeout", "15", "--json"]);
    // T1 is armed for 8.0 s from w1's exit. Wait until ~2.5 s after first_empty, attach w2, then exit w2 -> T2 should run a FULL 8.0 s (~t+11.0 s), NOT fire at t+8.0 s!
    if let Some(rem) = (first_empty + Duration::from_millis(2500)).checked_duration_since(Instant::now()) {
        std::thread::sleep(rem);
    }
    let w2 = rig.start_child("claude", "w2", &["--minder", &tm1_id, "--rules-text", "r2"]);
    assert_eq!(w2["minder"]["id"], json!(tm1_id), "{w2}");
    std::thread::sleep(Duration::from_millis(200));
    let cur_w2 = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    let second_empty = Instant::now();
    rig.tell("w2", "exit 0");
    let _ = rig.json(&["watch", "w2", "--since", &cur_w2.to_string(), "--on", "exit", "--timeout", "15", "--json"]);
    // Wait until ~8.6 s after first_empty (0.6 s PAST T1's 8.0 s, yet ~2.5 s BEFORE T2's 8.0 s!).
    if let Some(rem) = (first_empty + Duration::from_millis(8600)).checked_duration_since(Instant::now()) {
        std::thread::sleep(rem);
    }
    let fast_list = rig.json(&["list", "--fast", "--json"]);
    let tm1_live = fast_list["subagents"].as_array().unwrap().iter().any(|s| s["id"] == tm1_id);
    assert!(tm1_live, "first empty timer T1 shortened second grace period (first_empty={:?}, second_empty={:?})", first_empty.elapsed(), second_empty.elapsed());
    // After the full second grace period (~8.0 s), tm1 ends cleanly.
    let _ = rig.json(&["watch", &tm1_id, "--since", &cur_w2.to_string(), "--on", "exit", "--timeout", "15", "--json"]);
    assert_eq!(rig.status(&tm1_id)["running"], json!(false), "tm1 must end after full second grace period");

    // #14: rules write failure must leave started worker unminded under lead with `minder_error` (never minded with no rules),
    // and failed rules rewrite must leave existing `rules.md` untouched.
    Minding::expect(&rig, "rm1");
    let rw1 = rig.start_child("claude", "rw1", &["--minder", "claude:stand-in-big:max", "--rules-text", "KEEP_ME_INTACT", "--minder-env", "STAND_IN_NAME=rm1"]);
    let rw1_id = rw1["id"].as_str().unwrap().to_owned();
    let _rm1_id = rw1["minder"]["id"].as_str().unwrap().to_owned();
    let service_dir = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("registry.json").exists()).expect("service dir");
    let minders_dir = service_dir.join("minders");
    let rw1_dir = minders_dir.join(&rw1_id);
    // Make `rw1_dir` read-only (0500) so atomic temp-file creation in `rw1_dir` fails, then verify original `rules.md` stays intact!
    std::fs::set_permissions(&rw1_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (code, _, _) = rig.olo(&["mind", "rw1", "--rules-text", "CORRUPTED_PARTIAL"]);
    std::fs::set_permissions(&rw1_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_ne!(code, 0, "rules update in read-only dir must fail");
    assert_eq!(std::fs::read_to_string(rw1_dir.join("rules.md")).unwrap(), "KEEP_ME_INTACT", "failed rules rewrite must leave existing rules intact");

    // Make `minders_dir` read-only (0500) so creating a new agent's rules directory fails while reusing `rm1`:
    // `rw2` must start UNMINDED under lead with `minder_error`, never registered under `rm1` with no rules!
    std::fs::set_permissions(&minders_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let rw2 = rig.start_child("claude", "rw2", &["--minder", "claude:stand-in-big:max", "--rules-text", "r2"]);
    std::fs::set_permissions(&minders_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(rw2["minder_error"].is_string(), "expected minder_error on rules write failure: {rw2}");
    let rw2_st = rig.status("rw2");
    assert_eq!(
        (&rw2_st["parent_window_id"], &rw2_st["minded_by"], &rw2_st["minder"]),
        (&json!(rig.lead_window), &Value::Null, &Value::Null),
        "worker whose rules couldn't be written must not be minded: {rw2_st}"
    );
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);

    // #16a: an errored shared minder releases its roster AND arms its empty timer so it doesn't sit forever.
    Minding::expect(&rig, "em-err");
    let ew = rig.start_child("claude", "ew", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=em-err"]);
    let em_id = ew["minder"]["id"].as_str().unwrap().to_owned();
    let _em = Minding::of(&rig, "em-err", &em_id);
    rig.show("em-err", &format!("● Working\n  ⎿  API Error: Rate limit exceeded\n{}\n❯ \n", rule()));
    rig.wait("ew released back to lead on minder error", || rig.status("ew")["minder"].is_null());
    rig.wait("errored empty minder to end after its timer instead of sitting forever", || rig.status(&em_id)["running"] == false);
    rig.json(&["stop", "ew", "--force", "--json"]);

    // #16b: handover (`retire` + restart) while a minder is empty still re-arms its empty timer on `reconcile`.
    Minding::expect(&rig, "em-hand");
    let hw = rig.start_child("claude", "hw", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=em-hand"]);
    let hm_id = hw["minder"]["id"].as_str().unwrap().to_owned();
    rig.tell("hw", "exit 0");
    rig.wait_state("hw", "exited");
    let old_pid = rig.service_pid().expect("service pid");
    let ret = rig.helper().args(["agent", "retire"]).output().unwrap();
    assert!(ret.status.success());
    rig.wait("retired service exit", || !Path::new(&format!("/proc/{old_pid}")).exists());
    // Trigger new service via `list`; reconcile must re-arm the empty timer for `hm_id`.
    let _ = rig.json(&["list", "--fast", "--json"]);
    rig.wait("empty minder to end after handover reconcile", || rig.status(&hm_id)["running"] == false);

    // #17: concurrent starts with `MINDS_MOST=2` must convert reservation to live assignment atomically so BOTH share 1 minder (not 2!).
    Minding::expect(&rig, "conc-m1");
    Minding::expect(&rig, "conc-m2");
    for n in ["c1", "c2"] {
        let fifo = rig.dir.join(format!("stage/{n}.fifo"));
        let _ = std::fs::remove_file(&fifo);
        assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    }
    let spawn_start = |name: &'static str, minder_standin: &'static str| {
        let dir = rig.dir.clone();
        let sock = rig.sock.clone();
        let session = rig.session.clone();
        let block = rig.lead_block.clone();
        let work = rig.work();
        std::thread::spawn(move || {
            let mut cmd = Command::new(HELPER);
            Rig::scratch_env(&dir, &sock, &mut cmd);
            let out = cmd
                .env("OLO_REX_AGENTS_MINDS_MOST", "2")
                .env("OLO_REX_AGENTS_MINDER_REPORTS_MS", "2600")
                .env("REX_SESSION", &session)
                .env("REX_BLOCK", &block)
                .current_dir(&work)
                .args([
                    "agent",
                    "start",
                    "claude",
                    "--model",
                    "stand-in",
                    "--effort",
                    "none",
                    "--name",
                    name,
                    "--cwd",
                    &work,
                    "--env",
                    &format!("STAND_IN_DIR={}", dir.join("stage").display()),
                    "--env",
                    &format!("STAND_IN_NAME={name}"),
                    "--env",
                    &format!("PATH={}:/usr/bin:/bin", dir.join("bin").display()),
                    "--minder",
                    "claude:stand-in-big:max",
                    "--rules-text",
                    "r",
                    "--minder-env",
                    &format!("STAND_IN_NAME={minder_standin}"),
                    "--json",
                ])
                .output()
                .unwrap();
            assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
            serde_json::from_slice::<Value>(&out.stdout).unwrap()
        })
    };
    let t1 = spawn_start("c1", "conc-m1");
    let t2 = spawn_start("c2", "conc-m2");
    let (r1, r2) = (t1.join().unwrap(), t2.join().unwrap());
    assert_eq!(r1["minder"]["id"], r2["minder"]["id"], "concurrent starts with MINDS_MOST=2 double-counted reservation and started 2 minders: {r1} vs {r2}");
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);

    // #15: two concurrent `mind --with` moves on the same worker stay consistent (exactly one destination owns it, no duplicate removal).
    Minding::expect(&rig, "mv-a");
    Minding::expect(&rig, "mv-b");
    let ma_seed = rig.start_child("claude", "ma-seed", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=mv-a"]);
    let ma_id = ma_seed["minder"]["id"].as_str().unwrap().to_owned();
    let ma_win = ma_seed["minder"]["window_id"].as_str().unwrap().to_owned();
    let mb_seed = rig.start_child("claude", "mb-seed", &["--minder", "claude:stand-in-small:low", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=mv-b"]);
    let mb_id = mb_seed["minder"]["id"].as_str().unwrap().to_owned();
    let mb_win = mb_seed["minder"]["window_id"].as_str().unwrap().to_owned();
    let mv_worker = rig.start_child("claude", "mv-w", &[]);
    let mv_id = mv_worker["id"].as_str().unwrap().to_owned();
    let spawn_move = |dest: String| {
        let dir = rig.dir.clone();
        let sock = rig.sock.clone();
        let session = rig.session.clone();
        let block = rig.lead_block.clone();
        let work = rig.work();
        std::thread::spawn(move || {
            let mut cmd = Command::new(HELPER);
            Rig::scratch_env(&dir, &sock, &mut cmd);
            let out = cmd
                .env("OLO_REX_AGENTS_MINDS_MOST", "2")
                .env("OLO_REX_AGENTS_MINDER_REPORTS_MS", "2600")
                .env("REX_SESSION", &session)
                .env("REX_BLOCK", &block)
                .current_dir(&work)
                .args(["agent", "mind", "mv-w", "--with", &dest, "--rules-text", "r", "--json"])
                .output()
                .unwrap();
            assert!(out.status.success(), "move to {dest}: {}", String::from_utf8_lossy(&out.stderr));
        })
    };
    let j1 = spawn_move(ma_id.clone());
    let j2 = spawn_move(mb_id.clone());
    j1.join().unwrap();
    j2.join().unwrap();
    let final_st = rig.status(&mv_id);
    let final_m = final_st["minded_by"].as_str().expect("mv-w must be minded by one destination");
    let expected_win = if final_m == ma_id {
        &ma_win
    } else {
        assert_eq!(final_m, mb_id);
        &mb_win
    };
    assert_eq!(final_st["parent_window_id"], json!(expected_win), "parent_window_id diverged from minded_by: {final_st}");
    let ma_list = Minding::of(&rig, "mv-a", &ma_id).mind("list --fast --json");
    let mb_list = Minding::of(&rig, "mv-b", &mb_id).mind("list --fast --json");
    let in_a = ma_list["subagents"].as_array().unwrap().iter().any(|s| s["id"] == mv_id);
    let in_b = mb_list["subagents"].as_array().unwrap().iter().any(|s| s["id"] == mv_id);
    assert_ne!(in_a, in_b, "mv-w must belong to exactly one minder: in_a={in_a} in_b={in_b}");
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);

    // #16c: transcript-only API error on a shared minder (screen stays at idle prompt) marks it unhealthy,
    // releases its worker, prevents reuse, and arms its empty timer.
    Minding::expect(&rig, "tx-err-m");
    let tx_sid = "11111111-2222-3333-4444-555555555555";
    let txw = rig.start_child("claude", "txw", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=tx-err-m"]);
    let txm_id = txw["minder"]["id"].as_str().unwrap().to_owned();
    let _txm = Minding::of(&rig, "tx-err-m", &txm_id);
    // Bind `tx_sid` to `tx-err-m`'s PID via Claude's per-PID session record so `inspect` reads its transcript!
    let txm_pid = rig.status(&txm_id)["pid"].as_i64().expect("tx-err-m pid") as i32;
    let sess_dir = rig.dir.join("home/.claude/sessions");
    std::fs::create_dir_all(&sess_dir).unwrap();
    let mut pids = vec![txm_pid];
    if let Ok(kids) = std::fs::read_to_string(format!("/proc/{txm_pid}/task/{txm_pid}/children")) {
        pids.extend(kids.split_whitespace().filter_map(|s| s.parse::<i32>().ok()));
    }
    for p in pids {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat"))
            && let Some((_, rest)) = stat.rsplit_once(") ")
            && let Some(ticks) = rest.split_whitespace().nth(19).and_then(|s| s.parse::<u64>().ok())
        {
            std::fs::write(sess_dir.join(format!("{p}.json")), json!({ "pid": p, "procStart": ticks, "sessionId": tx_sid }).to_string()).unwrap();
        }
    }
    let txm_cwd = service_dir.join("minders").display().to_string();
    let txm_slug: String = txm_cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let txm_proj = rig.dir.join("home/.claude/projects").join(txm_slug);
    std::fs::create_dir_all(&txm_proj).unwrap();
    // Write a Claude transcript with `isApiErrorMessage: true` and show working -> idle on screen
    // so `effective_state` detects transcript-only `Error` even while the screen stays at idle prompt.
    // (Its turn's own time is after the minder was started, as a real one's is: what a file says of a turn
    // from *before* a child was given its work is not that work's, `state::of_the_turn_given`. Written
    // with a time of that morning, this passed or failed by whether a reading of the "working" screen had
    // taken the minder's prompt up first: found when it failed five times of five, 2026-11-27.)
    let tx_file = txm_proj.join(format!("{tx_sid}.jsonl"));
    std::fs::write(
        &tx_file,
        format!(
            "{}\n{}\n",
            json!({ "type": "user", "message": { "role": "user", "content": "Begin" }, "timestamp": "2099-01-01T00:00:00Z" }),
            json!({ "type": "assistant", "isApiErrorMessage": true, "message": { "role": "assistant", "content": [{ "type": "text", "text": "API Error: 529 Overloaded" }] }, "timestamp": "2026-11-27T00:00:01Z" }),
        ),
    )
    .unwrap();
    rig.show("tx-err-m", &working());
    rig.wait_state(&txm_id, "working");
    rig.show("tx-err-m", &idle("Waiting."));
    rig.wait("txw released on transcript-only minder error", || rig.status("txw")["minder"].is_null());
    rig.wait("transcript-errored minder ended by empty timer", || rig.status(&txm_id)["running"] == false);
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);

    // #12: deterministic mid-launch minder exit (`start --minder`) & mid-attach worker exit (`mind --with`).
    let Some(rig12) = Rig::start_with_env("r2-12", &[("OLO_REX_AGENTS_MINDER_REPORTS_MS", "2600"), ("OLO_REX_AGENTS_TEST_START_HOLD_MS", "1100"), ("OLO_REX_AGENTS_TEST_MIND_HOLD_MS", "1100")]) else {
        return;
    };
    rig12.stand_in("claude");
    Minding::expect(&rig12, "die-m");
    let seed = rig12.start_child("claude", "seed", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=die-m"]);
    let die_m_id = seed["minder"]["id"].as_str().unwrap().to_owned();
    let _dm = Minding::of(&rig12, "die-m", &die_m_id);
    // Start `w-race` under `die-m` (which holds 1100 ms before `begin_attached`), and kill `die-m` mid-launch.
    let race_fifo = rig12.dir.join("stage/w-race.fifo");
    assert!(Command::new("mkfifo").arg(&race_fifo).status().unwrap().success());
    let start_handle = {
        let (dir, sock, session, block, work, target_m) = (rig12.dir.clone(), rig12.sock.clone(), rig12.session.clone(), rig12.lead_block.clone(), rig12.work(), die_m_id.clone());
        std::thread::spawn(move || {
            let mut cmd = Command::new(HELPER);
            Rig::scratch_env(&dir, &sock, &mut cmd);
            let out = cmd
                .env("OLO_REX_AGENTS_TEST_START_HOLD_MS", "1100")
                .env("OLO_REX_AGENTS_TEST_MIND_HOLD_MS", "1100")
                .env("REX_SESSION", &session)
                .env("REX_BLOCK", &block)
                .current_dir(&work)
                .args([
                    "agent",
                    "start",
                    "claude",
                    "--model",
                    "stand-in",
                    "--effort",
                    "none",
                    "--name",
                    "w-race",
                    "--cwd",
                    &work,
                    "--env",
                    &format!("STAND_IN_DIR={}", dir.join("stage").display()),
                    "--env",
                    "STAND_IN_NAME=w-race",
                    "--env",
                    &format!("PATH={}:/usr/bin:/bin", dir.join("bin").display()),
                    "--minder",
                    &target_m,
                    "--rules-text",
                    "r",
                    "--json",
                ])
                .output()
                .unwrap();
            assert!(out.status.success(), "w-race start should fall back cleanly: {}", String::from_utf8_lossy(&out.stderr));
            serde_json::from_slice::<Value>(&out.stdout).unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(400));
    rig12.json(&["stop", &die_m_id, "--force", "--json"]);
    let raced = start_handle.join().unwrap();
    assert!(raced["minder_error"].is_string() && raced["minder"].is_null(), "w-race must report minder_error when minder died mid-launch: {raced}");
    let raced_st = rig12.status("w-race");
    assert_eq!((&raced_st["parent_window_id"], &raced_st["minded_by"]), (&json!(rig12.lead_window), &Value::Null), "w-race must land unminded under lead: {raced_st}");

    // Now test worker exiting while `mind --with` is in flight: must fail, emit no `added`, and clean up the fresh minder!
    let dying = rig12.start_child("claude", "dying-w", &[]);
    let dying_id = dying["id"].as_str().unwrap().to_owned();
    Minding::expect(&rig12, "fresh-m12");
    let mind_handle = {
        let (dir, sock, session, block, work) = (rig12.dir.clone(), rig12.sock.clone(), rig12.session.clone(), rig12.lead_block.clone(), rig12.work());
        std::thread::spawn(move || {
            let mut cmd = Command::new(HELPER);
            Rig::scratch_env(&dir, &sock, &mut cmd);
            cmd.env("OLO_REX_AGENTS_TEST_START_HOLD_MS", "1100")
                .env("OLO_REX_AGENTS_TEST_MIND_HOLD_MS", "1100")
                .env("REX_SESSION", &session)
                .env("REX_BLOCK", &block)
                .current_dir(&work)
                .args([
                    "agent",
                    "mind",
                    "dying-w",
                    "--with",
                    "claude:stand-in-big:max",
                    "--rules-text",
                    "r",
                    "--env",
                    &format!("STAND_IN_DIR={}", dir.join("stage").display()),
                    "--env",
                    &format!("PATH={}:/usr/bin:/bin", dir.join("bin").display()),
                    "--env",
                    "STAND_IN_NAME=fresh-m12",
                    "--json",
                ])
                .output()
                .unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    rig12.tell("dying-w", "exit 0");
    rig12.wait_state(&dying_id, "exited");
    let mind_out = mind_handle.join().unwrap();
    assert!(!mind_out.status.success(), "mind --with on exiting worker must fail: {}", String::from_utf8_lossy(&mind_out.stdout));
    // Fresh minder started for `dying-w` must be cleaned up (not left running empty).
    let live_minders: Vec<Value> = rig12.json(&["list", "--fast", "--json"])["subagents"].as_array().unwrap().iter().filter(|s| s["role"] == "minder").cloned().collect();
    assert!(live_minders.is_empty(), "fresh minder for exited worker was left running: {live_minders:?}");
}

/// Round 2 (#18, #19, and legacy 1:1 rules-edit migration):
/// - #18: `stop sublead --tree --keep-tab` must NOT leave a stale `let_go` mark that silently swallows a later independent minder exit.
/// - #19: top-level `stop <lead_window> --tree --keep-tab` reports surviving running PIDs in its text reply instead of saying "Stopped N subagent tab(s)."
/// - Legacy 1:1 rules edit: `mind worker --rules-text ...` on a legacy 1:1 minder migrates cleanly to a shared minder, stopping the old 1:1 minder and moving `worker` under the shared minder.
#[test]
fn round2_18_19_quiet_stop_survivor_report_and_legacy_rules_migration() {
    let Some(rig) = Rig::start("r2-stop") else { return };
    rig.stand_in("claude");

    // #18: `stop sublead --tree --keep-tab` keeps tabs alive -> later independent exit of `sub-m` MUST still escalate to lead!
    let sublead = rig.start_child("claude", "sublead", &[]);
    let sub_block = sublead["block_id"].as_str().unwrap().to_owned();
    Minding::expect(&rig, "sub-m18");
    let sw = rig.start_child_from(Some(&sub_block), "claude", "sw18", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=sub-m18"]);
    let _sm = Minding::of(&rig, "sub-m18", sw["minder"]["id"].as_str().unwrap());
    let kept = rig.json(&["stop", "sublead", "--tree", "--keep-tab", "--json"]);
    assert_eq!(kept["ok"], json!(true), "{kept}");
    let from = rig.json(&["list", "--fast", "--json"])["cursor"].as_u64().unwrap();
    // Now `sub-m18` exits independently while `sw18` is still alive: lead MUST receive the escalation!
    rig.tell("sub-m18", "exit 2");
    let esc = rig.json(&["watch", "--all", "--since", &from.to_string(), "--on", "escalation", "--timeout", "10", "--json"]);
    assert_eq!((&esc["event"], &esc["name"]), (&json!("escalation"), &json!("sw18")), "stale let_go swallowed independent minder exit: {esc}");

    // #19: top-level `stop <lead_window> --tree --keep-tab` text reply MUST say it interrupted / still running (not "Stopped N subagent tab(s).").
    let (code, out, err) = rig.olo(&["stop", &rig.lead_window, "--tree", "--keep-tab"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("still running") || out.contains("Interrupted"), "top-level --keep-tab reply hid surviving programs: {out}");
    rig.json(&["stop", &rig.lead_window, "--tree", "--force", "--json"]);

    // Legacy 1:1 rules edit migration: `mind worker --rules-text` migrates a legacy 1:1 minder to a shared minder.
    let worker = rig.start_child("claude", "legw", &[]);
    let worker_id = worker["id"].as_str().unwrap().to_owned();
    let worker_block = worker["block_id"].as_str().unwrap().to_owned();
    let old_m = rig.start_child_from(Some(&worker_block), "claude", "legm", &[]);
    let old_m_id = old_m["id"].as_str().unwrap().to_owned();
    let old_pid = rig.service_pid().expect("service pid");
    let service_dir = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("registry.json").exists()).expect("service dir");
    assert!(rig.helper().args(["agent", "retire"]).output().unwrap().status.success());
    rig.wait("retired service exit", || !Path::new(&format!("/proc/{old_pid}")).exists());
    let reg_path = service_dir.join("registry.json");
    let mut reg: Value = serde_json::from_str(&std::fs::read_to_string(&reg_path).unwrap()).unwrap();
    for e in reg["entries"].as_array_mut().unwrap() {
        if e["id"] == old_m_id {
            e["role"] = json!("minder");
            e["minds"] = json!(worker_id);
        }
        e.as_object_mut().unwrap().remove("minded_by");
        e.as_object_mut().unwrap().remove("shared_minder");
    }
    std::fs::write(&reg_path, serde_json::to_string_pretty(&reg).unwrap()).unwrap();
    Minding::expect(&rig, "leg-new");
    let stage = format!("STAND_IN_DIR={}", rig.dir.join("stage").display());
    let path = format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display());
    let migrated = rig.json(&["mind", "legw", "--rules-text", "Approve cargo clippy.", "--env", &stage, "--env", &path, "--env", "STAND_IN_NAME=leg-new", "--json"]);
    let new_m_id = migrated["minder"]["id"].as_str().unwrap().to_owned();
    let new_m_win = migrated["minder"]["window_id"].as_str().unwrap().to_owned();
    assert_ne!(new_m_id, old_m_id, "legacy rules edit should migrate to a new shared minder: {migrated}");
    assert_eq!(migrated["replaced"], json!(old_m_id), "{migrated}");
    rig.wait("old 1:1 minder stopped after migration", || rig.status(&old_m_id)["running"] == false);
    let wst = rig.status("legw");
    assert_eq!((&wst["parent_window_id"], &wst["minded_by"]), (&json!(new_m_win), &json!(new_m_id)), "{wst}");
    assert_eq!(std::fs::read_to_string(service_dir.join("minders").join(&worker_id).join("rules.md")).unwrap(), "Approve cargo clippy.");
}

/// Round 3 Item 1 (#6, #25):
/// After rules change, neither `status` nor an approval-only `watch` (without `minding`)
/// may clear the pending rules revision or mint a fresh approval grant. The minder MUST
/// receive the `minding` (`rules`) revision first — even if >2,000 newer host events have
/// rolled the `minding` event off the log ring!
#[test]
fn round3_1_rules_revision_must_be_delivered_before_grant_renews() {
    let Some(rig) = Rig::start("r3-rules-rev") else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "r3m1");
    let a = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "Approve cargo test.", "--minder-env", "STAND_IN_NAME=r3m1"]);
    let minder_id = a["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "r3m1", &minder_id);
    let added = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    assert_eq!(added["minding"]["action"], "added", "{added}");
    let cur = added["cursor"].as_u64().unwrap();

    // Deliver initial approval under old rules.
    rig.show("alpha", &asking("cargo test", 0));
    let ev1 = minder.mind(&format!("watch --since {cur} --on approval --timeout 15 --json"));
    assert_eq!(ev1["event"], "waiting_approval", "{ev1}");

    // Lead updates rules.
    rig.json(&["mind", "alpha", "--rules-text", "Refuse cargo test.", "--json"]);

    // (a) Calling `status` without receiving the `minding` ("rules") event must NOT renew the grant!
    let _st = minder.mind("status alpha --json");
    let keys_before = rig.keys("alpha").len();
    let ap_after_status = minder.run(&format!("{} {} approve alpha --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(
        ap_after_status.contains("status=1") && ap_after_status.to_ascii_lowercase().contains("rules"),
        "ordinary `status` must NOT clear pending rules revision or renew grant: {ap_after_status}"
    );
    assert_eq!(rig.keys("alpha").len(), keys_before, "no keys may be typed before rules revision is delivered");

    // (b) Calling approval-only `watch --on approval` (no `minding`) must NOT clear pending rules revision or renew grant!
    let _aw = minder.run(&format!("{} {} watch alpha --since 0 --on approval --timeout 2 --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    let ap_after_watch = minder.run(&format!("{} {} approve alpha --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(
        ap_after_watch.contains("status=1") && ap_after_watch.to_ascii_lowercase().contains("rules"),
        "approval-only `watch` must NOT clear pending rules revision or renew grant: {ap_after_watch}"
    );
    assert_eq!(rig.keys("alpha").len(), keys_before, "no keys may be typed after approval-only watch");

    // (c) Evict the 2,000-event log ring with 2,050 unrelated escalations from another child.
    let filler = rig.start_child("claude", "ring-filler", &[]);
    let filler_block = filler["block_id"].as_str().unwrap().to_owned();
    for i in 0..2_050 {
        let resp = rig.raw(json!({
            "proto": 1, "op": "escalate", "args": [format!("filler-{i}")], "cwd": rig.work(),
            "block": filler_block, "session": rig.session,
        }));
        assert_eq!(resp["ok"], json!(true), "ring filler {i} failed: {resp}");
    }

    // (d) Even after the ring evicted the original `minding` event, `watch --on minding` MUST still deliver the rules revision!
    let rules_ev = minder.mind("watch --since 0 --on minding --timeout 10 --json");
    assert_eq!(
        (&rules_ev["event"], &rules_ev["minding"]["action"], &rules_ev["name"]),
        (&json!("minding"), &json!("rules"), &json!("alpha")),
        "evicted rules revision must still be delivered on minding watch: {rules_ev}"
    );

    // (e) Now that the rules revision HAS been delivered, `status` renews the grant and `approve` succeeds!
    let _st2 = minder.mind("status alpha --json");
    let gone = rig.when_typed("alpha", b"\r", working());
    let ap_ok = minder.mind("approve alpha --once --json");
    assert_eq!(ap_ok["ok"], json!(true), "approve must succeed after rules revision was delivered: {ap_ok}");
    assert!(gone.join().unwrap(), "alpha did not receive Enter after valid approve");
}

/// Round 3 Item 2 (#6, #7):
/// - Revocation during arrow navigation / verify loop must stop immediately and send ZERO Escape or Enter keys.
/// - Two identical approval boxes back-to-back with no blank screen between must NEVER get a second automatic retry Enter.
/// - Revocation while waiting to paste a refusal `--reason` must send ZERO reason text and ZERO Enter.
#[test]
fn round3_2_approve_no_keys_after_revocation_and_no_retry_into_second_box() {
    let Some(rig) = Rig::start("r3-approve") else { return };
    rig.stand_in("claude");

    Minding::expect(&rig, "r3m2");
    let a = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "Allow all.", "--minder-env", "STAND_IN_NAME=r3m2"]);
    let minder_id = a["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "r3m2", &minder_id);
    let added = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    let mut cur = added["cursor"].as_u64().unwrap();

    // (a) Revocation during arrow navigation / verify loop: `--no` moves 2 arrows down then waits 1.5s
    // for cursor to land (stand-in doesn't redraw). Revoking rules while in verify loop must NOT write Escape (`\x1b` alone) or Enter (`\r`)!
    rig.show("alpha", &asking("cargo test", 0));
    let ev1 = minder.mind(&format!("watch --since {cur} --on approval --timeout 15 --json"));
    assert_eq!(ev1["event"], "waiting_approval", "{ev1}");
    cur = ev1["cursor"].as_u64().unwrap();
    let keys_start = rig.keys("alpha").len();
    let nav_thread = {
        let env = minder.env();
        let mind_bin = minder.dir.join("mind").display().to_string();
        let stage = rig.dir.join("stage");
        let stand_in = minder.stand_in.clone();
        std::thread::spawn(move || {
            let out_file = stage.join("r3-nav.out");
            let fifo = stage.join(format!("{stand_in}.fifo"));
            let cmd = format!("run sh -c '{env} {mind_bin} approve alpha --no --json > {} 2>&1; echo status=$? >> {}'\n", out_file.display(), out_file.display());
            std::fs::OpenOptions::new().write(true).open(&fifo).unwrap().write_all(cmd.as_bytes()).unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(s) = std::fs::read_to_string(&out_file)
                    && s.contains("status=")
                {
                    return s;
                }
                assert!(Instant::now() < until, "nav approve did not finish");
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    // Wait until the first Down arrow (`\x1b[B`) hits `alpha`, then revoke rules immediately!
    rig.wait("down arrow sent during approve --no", || rig.keys("alpha").len() >= keys_start + 3);
    rig.json(&["mind", "alpha", "--rules-text", "Revoked mid-navigation.", "--json"]);
    let nav_out = nav_thread.join().unwrap();
    assert!(!nav_out.contains("status=0"), "revoked mid-navigation approve must fail: {nav_out}");
    std::thread::sleep(Duration::from_millis(120));
    let typed = &rig.keys("alpha")[keys_start..];
    // Strip legitimate arrow sequences `\x1b[B` / `\x1b[A`: ZERO stray `\x1b` (Escape) and ZERO `\r` may remain!
    let mut i = 0;
    let mut leftover = Vec::new();
    while i < typed.len() {
        if i + 2 < typed.len() && typed[i] == 0x1b && typed[i + 1] == b'[' && matches!(typed[i + 2], b'A' | b'B') {
            i += 3;
        } else {
            leftover.push(typed[i]);
            i += 1;
        }
    }
    assert!(leftover.is_empty(), "revoked approve must send ZERO Escape or Enter after revocation, got leftover bytes {leftover:?} (full: {typed:?})");

    // Consume rules revision and refresh grant for part (b).
    let r_ev = minder.mind("watch --since 0 --on minding --timeout 10 --json");
    assert_eq!(r_ev["minding"]["action"], "rules", "{r_ev}");
    let _ = minder.mind("status alpha --json");

    // (b) Identical second box with no blank screen between: stand-in keeps showing the exact same box
    // when `\r` arrives -> `approve --once` must send AT MOST ONE `\r` (never an automatic 700ms retry Enter!).
    let keys_b = rig.keys("alpha").len();
    let _ap_b = minder.run(&format!("{} {} approve alpha --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    std::thread::sleep(Duration::from_millis(100));
    let enters = rig.keys("alpha")[keys_b..].iter().filter(|&&b| b == b'\r').count();
    assert_eq!(enters, 1, "approve must send exactly ONE Enter (no automatic retry that could confirm a back-to-back identical box), sent {enters}");

    // (c) Revocation during refusal `--no --reason`: box already on choice 2 ("3. No", delta=0).
    // On Enter, stand-in draws a neutral screen (not at prompt) so approve waits up to 1.2s before pasting reason.
    // Lead detaches (`--off`) right after Enter -> ZERO reason text may be pasted!
    rig.show("alpha", &working());
    rig.wait_state("alpha", "working");
    rig.show("alpha", &asking("git push origin main", 2));
    let ev2 = minder.mind(&format!("watch --since {cur} --on approval --timeout 15 --json"));
    assert_eq!(ev2["event"], "waiting_approval", "{ev2}");
    let keys_c = rig.keys("alpha").len();
    let neutral_screen = format!("Refused.\nStill settling output...\n{}\n", rule());
    let on_enter = rig.when_typed("alpha", b"\r", neutral_screen);
    let reason_thread = {
        let env = minder.env();
        let mind_bin = minder.dir.join("mind").display().to_string();
        let stage = rig.dir.join("stage");
        let stand_in = minder.stand_in.clone();
        std::thread::spawn(move || {
            let out_file = stage.join("r3-reason.out");
            let fifo = stage.join(format!("{stand_in}.fifo"));
            let cmd = format!("run sh -c '{env} {mind_bin} approve alpha --no --reason NO_PASTE_AFTER_REVOKE --json > {} 2>&1; echo status=$? >> {}'\n", out_file.display(), out_file.display());
            std::fs::OpenOptions::new().write(true).open(&fifo).unwrap().write_all(cmd.as_bytes()).unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(s) = std::fs::read_to_string(&out_file)
                    && s.contains("status=")
                {
                    return s;
                }
                assert!(Instant::now() < until, "reason approve did not finish");
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    assert!(on_enter.join().unwrap(), "alpha did not receive Enter for refusal");
    // Immediately detach minder while approve is in the 1.2s wait loop before pasting the reason!
    rig.json(&["mind", "alpha", "--off", "--json"]);
    let _ = reason_thread.join().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let typed_c = String::from_utf8_lossy(&rig.keys("alpha")[keys_c..]).into_owned();
    assert!(!typed_c.contains("NO_PASTE_AFTER_REVOKE"), "revoked minder must NOT paste refusal reason after detach/revocation: {typed_c:?}");
}

/// Round 3 Items 3, 4, 5 (#1, #3):
/// - Item 3a: `adopt` strips `shared_minder` and `minded_by` from imported records.
/// - Item 3b: an ordinary (non-minder) child tab calling `adopt` is refused with code 69.
/// - Item 4: a restricted caller (minder or exited child) sending `proto > PROTO` is refused (69) BEFORE handover and never retires the service.
/// - Item 5: a `policy` query from a non-child peer (e.g. Rex) asking about a minder's pane succeeds with `desktop: "none"` instead of refusing with 69.
#[test]
fn round3_3_4_5_adopt_handover_and_policy_gates() {
    let Some(rig) = Rig::start("r3-gates") else { return };
    rig.stand_in("claude");

    // Start an ordinary child `ord`, plus a worker `alpha` under a shared minder `r3m3`.
    let ord = rig.start_child("claude", "ord", &[]);
    let ord_block = ord["block_id"].as_str().unwrap().to_owned();
    Minding::expect(&rig, "r3m3");
    let a = rig.start_child("claude", "alpha", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=r3m3"]);
    let minder_id = a["minder"]["id"].as_str().unwrap().to_owned();
    let minder_block = rig.status(&minder_id)["block_id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "r3m3", &minder_id);

    // Item 3a: `adopt` from lead with forged `shared_minder: true` and `minded_by` MUST clear both fields!
    let forged_m = json!({
        "id": "sa-forged-m", "name": "forged-m", "host_key": "",
        "session_id": rig.session, "window_id": "w-forged-m", "block_id": "b-forged-m",
        "parent_window_id": rig.lead_window, "cli": "claude", "model": "m", "effort": "e",
        "title": "forged-m", "started_at": 1000, "exited_at": 2000, "seen_live": true,
        "shared_minder": true, "minded_by": "sa-other"
    });
    let mut adopt = rig.helper().args(["agent", "adopt"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    adopt.stdin.take().unwrap().write_all(json!({ "agents": [forged_m] }).to_string().as_bytes()).unwrap();
    let adopt_out = adopt.wait_with_output().unwrap();
    assert!(adopt_out.status.success(), "{}", String::from_utf8_lossy(&adopt_out.stderr));
    let listed = rig.json(&["list", "--all", "--fast", "--json"]);
    let rec = listed["subagents"].as_array().unwrap().iter().find(|e| e["id"] == "sa-forged-m").expect("adopted entry");
    assert_eq!(rec["shared_minder"], json!(false), "adopt must strip shared_minder: {rec}");
    assert_eq!(rec["minded_by"], Value::Null, "adopt must strip minded_by: {rec}");

    // Item 3b: ordinary non-minder child calling `adopt` (both via REX_BLOCK and from inside its own tab) MUST be refused with code 69!
    let mut child_adopt = rig.helper().env("REX_BLOCK", &ord_block).args(["agent", "adopt"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child_adopt.stdin.take().unwrap().write_all(b"{\"agents\":[]}").unwrap();
    let child_adopt_out = child_adopt.wait_with_output().unwrap();
    assert_eq!(child_adopt_out.status.code(), Some(69), "child caller (REX_BLOCK) must be refused `adopt` with 69: {:?}", child_adopt_out);
    let _ = std::fs::remove_file(rig.staged("ord", "ran"));
    rig.tell("ord", &format!("run printf '{{\"agents\":[]}}' | {} {HELPER} agent adopt; echo status=$?", minder.env()));
    rig.wait("ord adopt ran", || rig.staged("ord", "ran").exists());
    let ord_adopt_out = std::fs::read_to_string(rig.staged("ord", "out")).unwrap();
    assert!(ord_adopt_out.contains("status=69"), "child caller (process ancestry) must be refused `adopt` with 69: {ord_adopt_out}");

    // Item 4: restricted caller (minder via block or exited child) sending `proto: 99` must receive refusal 69 and NEVER retire the service!
    let pid_before = rig.service_pid().expect("service pid");
    let raw_minder_proto = rig.raw(json!({ "proto": 99, "op": "ping", "block": minder_block, "session": rig.session }));
    assert_eq!((&raw_minder_proto["ok"], &raw_minder_proto["code"]), (&json!(false), &json!(69)), "minder proto>PROTO must be refused with 69, not trigger handover: {raw_minder_proto}");
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(rig.service_pid(), Some(pid_before), "minder proto>PROTO retired the service!");

    // Also test exited child sending `proto: 99`:
    rig.tell("ord", "exit 0");
    rig.wait_state("ord", "exited");
    let raw_exited_proto = rig.raw(json!({ "proto": 99, "op": "ping", "block": ord_block, "session": rig.session }));
    assert_eq!((&raw_exited_proto["ok"], &raw_exited_proto["code"]), (&json!(false), &json!(69)), "exited child proto>PROTO must be refused with 69: {raw_exited_proto}");
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(rig.service_pid(), Some(pid_before), "exited child proto>PROTO retired the service!");

    // Item 5: non-child peer (Rex / test process) querying `policy` for the minder's pane MUST succeed with `desktop: "none"`!
    let pol = rig.raw(json!({ "proto": 1, "op": "policy", "block": minder_block, "session": rig.session }));
    assert_eq!(pol["ok"], json!(true), "policy query for minder's pane must NOT be refused as if the minder asked: {pol}");
    assert_eq!(pol["data"]["child"], json!(minder_id), "{pol}");
    assert_eq!(pol["data"]["desktop"], json!("none"), "{pol}");
    // Keep minder alive until after policy check.
    let _ = minder.mind("list --json");
}

/// Round 3 Items 6, 7, 8, 12, 13, 14:
/// - Item 6 (#5, #24): minder `send` refuses unrecognised `Blocked` dialog (69); enforces grant + rules_gen on transcript-only question; refuses text + `--key` outside recognised question (69).
/// - Item 7 (#9): top-level `stop <LEAD_WIN> --tree` with missing or unmapped `REX_BLOCK` is refused (69), never falling back to `sess.active`.
/// - Item 8 (#10): `mind NAME --with ...` resolves bare source strictly inside asker's own subtree (never takes another lead's worker).
/// - Item 12 (#7, #24): identical second approval box after direct terminal answer + brief working is NOT deduplicated in `SubagentEventLog`.
/// - Item 13 (#26): `mind NAME --off` returns a cursor strictly BEFORE any republished standing question so `watch --since <cursor>` catches it.
/// - Item 14 (#28): `mind escalate "Help"` with a ward named `help` is always a self-escalation.
#[test]
fn round3_6_7_8_12_13_14_send_stop_mind_box_detach_escalate() {
    let Some(rig) = Rig::start("r3-misc") else { return };
    rig.stand_in("claude");

    // Start `alpha` (with known session ID for transcript-only question test) and `help` under shared minder `r3m6`.
    let q_sid = "66666666-7777-8888-9999-aaaaaaaaaaaa";
    Minding::expect(&rig, "r3m6");
    let a = rig.start_child("claude", "alpha", &["--session-id", q_sid, "--minder", "claude:stand-in-big:max", "--rules-text", "Answer questions.", "--minder-env", "STAND_IN_NAME=r3m6"]);
    let minder_id = a["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "r3m6", &minder_id);
    let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");
    let _help_w = rig.start_child("claude", "help", &["--minder", "claude:stand-in-big:max", "--rules-text", "r"]);
    let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");

    // Item 14 (#28): 1-arg `mind escalate Help` when a ward is named `help` MUST succeed as a self-escalation!
    let esc_self = minder.mind("escalate Help --json");
    assert_eq!(esc_self["ok"], json!(true), "1-arg escalate matching a ward name must be self-escalation: {esc_self}");
    assert_eq!(esc_self["id"], json!(minder_id), "1-arg escalate must target the minder itself: {esc_self}");

    // Item 6a (#5): unrecognised `Blocked` dialog (`Activity::Blocked` -> "needs input (unrecognised dialog)") MUST be refused by minder `send` (code 69)!
    rig.show("alpha", "\x1b]7501;state=blocked\x07Unrecognised modal dialog\nPress something to continue\n");
    rig.wait_state("alpha", "waiting_question");
    let _ = minder.mind("status alpha --json");
    let k_blk = rig.keys("alpha").len();
    let blk_send = minder.run(&format!("{} {} send alpha 'yes' --enter --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(blk_send.contains("status=69"), "minder send into unrecognised Blocked dialog must be refused with 69: {blk_send}");
    assert_eq!(rig.keys("alpha").len(), k_blk, "zero keys may be typed into unrecognised dialog");

    // Item 6c (#5): body + `--key` on a `working` screen must be refused for a minder (code 69)!
    rig.show("alpha", &format!("\x1b]7501;state=working\x07{}", working()));
    rig.wait_state("alpha", "working");
    let k_wk = rig.keys("alpha").len();
    let combo_send = minder.run(&format!("{} {HELPER} agent send alpha 'hello' --key Escape --enter --json; echo status=$?", minder.env()));
    assert!(combo_send.contains("status=69"), "minder send with text + --key on working screen must be refused with 69: {combo_send}");
    assert_eq!(rig.keys("alpha").len(), k_wk, "zero keys may be typed on refused text+--key send");

    // Item 6b (#24): transcript-only `WaitingQuestion` (`AskUserQuestion` in transcript while screen looks `idle`) MUST enforce question grant + `rules_gen`!
    {
        let folder: String = rig.work().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let tdir = rig.dir.join("home/.claude/projects").join(folder);
        std::fs::create_dir_all(&tdir).unwrap();
        let lines = [
            json!({ "type": "user", "message": { "role": "user", "content": "ask me" } }),
            json!({ "type": "assistant", "message": { "id": "msg_q", "role": "assistant", "content": [{ "type": "tool_use", "id": "tu_q", "name": "AskUserQuestion", "input": { "question": "Proceed?" } }], "stop_reason": "tool_use" } }),
        ];
        std::fs::write(tdir.join(format!("{q_sid}.jsonl")), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }
    rig.show("alpha", &format!("\x1b]7501;state=idle\x07{}", idle("Standing by.")));
    rig.wait_state("alpha", "waiting_question");
    // Update rules so minder has NO valid grant for this transcript-only question!
    rig.json(&["mind", "alpha", "--rules-text", "Escalate questions.", "--json"]);
    let k_tx = rig.keys("alpha").len();
    let tx_send = minder.run(&format!("{} {} send alpha 'yes' --enter --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(tx_send.contains("status=1"), "transcript-only WaitingQuestion must enforce grant/rules_gen on minder send: {tx_send}");
    assert_eq!(rig.keys("alpha").len(), k_tx, "zero keys may be typed into transcript-only question without valid grant");
    // Clear the synthetic AskUserQuestion transcript before testing Item 12's Working -> WaitingApproval transition.
    rig.says(q_sid, "ask me", "answered");

    // Item 13 (#26): `mind alpha --off` while approval stands returns a cursor BEFORE the republished question so `watch --since <cursor>` catches it!
    rig.show("alpha", &asking("cargo check", 0));
    rig.wait_state("alpha", "waiting_approval");
    let off = rig.json(&["mind", "alpha", "--off", "--json"]);
    let off_cursor = off["cursor"].as_u64().unwrap();
    let lead_ev = rig.json(&["watch", "alpha", "--since", &off_cursor.to_string(), "--on", "approval", "--timeout", "8", "--json"]);
    assert_eq!((&lead_ev["event"], &lead_ev["name"]), (&json!("waiting_approval"), &json!("alpha")), "watch --since <detach_cursor> must catch republished standing question: {lead_ev}");
    assert!(lead_ev["cursor"].as_u64().unwrap() > off_cursor, "republished question must be strictly after detach cursor");

    // Item 12 (#7, #24): identical second approval box after direct terminal handling + brief working MUST NOT be deduplicated in event log!
    let box1 = lead_ev["approval"]["box"].as_str().unwrap().to_owned();
    let c1 = lead_ev["cursor"].as_u64().unwrap();
    // Simulate a person answering directly in the terminal (not through `approve`): screen goes to working, then draws the EXACT SAME box again!
    rig.show("alpha", &working());
    rig.wait_state("alpha", "working");
    rig.show("alpha", &asking("cargo check", 0));
    let lead_ev2 = rig.json(&["watch", "alpha", "--since", &c1.to_string(), "--on", "approval", "--timeout", "8", "--json"]);
    assert_eq!(lead_ev2["event"], "waiting_approval", "identical second box must not be deduplicated away: {lead_ev2}");
    let box2 = lead_ev2["approval"]["box"].as_str().unwrap().to_owned();
    assert_ne!(box1, box2, "identical second box must carry a fresh box key");
    assert!(lead_ev2["cursor"].as_u64().unwrap() > c1, "identical second box must advance cursor");

    // Items 7 & 8 (#9, #10): second lead tab in the same session (which becomes `sess.active`!).
    let lead2 = rig.rex(
        "session.new_window",
        json!({ "session_id": rig.session, "window_label": "lead2", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 110, "rows": 40 } } } } }),
    );
    let lead2_win = lead2["window_id"].as_str().unwrap().to_owned();
    let lead2_block = lead2["block_ids"][0].as_str().unwrap().to_owned();
    let only_b = rig.start_child_from(Some(&lead2_block), "claude", "only-b", &[]);
    let only_b_id = only_b["id"].as_str().unwrap().to_owned();

    // Item 8: Lead 1 calling `mind only-b --with ...` MUST NOT steal Lead 2's worker by bare name!
    let (m_code, m_out, m_err) = rig.olo(&["mind", "only-b", "--with", "claude:stand-in-big:max", "--rules-text", "stolen"]);
    assert_ne!(m_code, 0, "Lead 1 must NOT move Lead 2's worker by bare name: {m_out}{m_err}");
    assert_eq!(rig.status(&only_b_id)["parent_window_id"], json!(lead2_win), "Lead 2's worker must stay under Lead 2");

    // Item 7: `stop <lead2_win> --tree` with missing `REX_BLOCK` or unmapped `REX_BLOCK` (even though `lead2_win` is `sess.active`!) MUST be refused with 69!
    let out_no_block = rig.helper().env("REX_SESSION", &rig.session).args(["agent", "stop", &lead2_win, "--tree", "--force"]).output().unwrap();
    assert_eq!(out_no_block.status.code(), Some(69), "stop --tree with no REX_BLOCK must be refused (69): {:?}", out_no_block);
    let out_bad_block = rig.helper().env("REX_SESSION", &rig.session).env("REX_BLOCK", "block:does-not-exist").args(["agent", "stop", &lead2_win, "--tree", "--force"]).output().unwrap();
    assert_eq!(out_bad_block.status.code(), Some(69), "stop --tree with unmapped REX_BLOCK must be refused (69): {:?}", out_bad_block);
    assert_eq!(rig.status(&only_b_id)["running"], json!(true), "Lead 2's worker must still be running after refused stop --tree");
    rig.olo_from(Some(&lead2_block), &["stop", &lead2_win, "--tree", "--force", "--json"]);
}

/// Spawns a local WebSocket-over-Unix-socket proxy in front of `real_sock` that
/// forwards all Rex control & data traffic transparently, except when `fail_close_and_view`
/// is armed it returns a non-`gone` Rex API error (`code: "busy"`) for `session.close_window`
/// and `session.view` (optionally filtered to a target `window_id` for `close_window`).
struct FaultyRexProxy {
    sock: PathBuf,
    fail_all: std::sync::Arc<std::sync::atomic::AtomicBool>,
    fail_window: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FaultyRexProxy {
    #[allow(clippy::result_large_err)]
    fn start(real_sock: PathBuf, proxy_sock: PathBuf) -> Self {
        use futures_util::{SinkExt as _, StreamExt as _};
        use tokio_tungstenite::tungstenite::{Message, handshake::server, http};
        let _ = std::fs::remove_file(&proxy_sock);
        let fail_all = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fail_window = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (p_sock, f_all, f_win, st_flag) = (proxy_sock.clone(), fail_all.clone(), fail_window.clone(), stop.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::UnixListener::bind(&p_sock).unwrap();
                let _ = ready_tx.send(());
                loop {
                    tokio::select! {
                        res = listener.accept() => {
                            let Ok((client_stream, _)) = res else { break };
                            let (r_sock, f_all, f_win) = (real_sock.clone(), f_all.clone(), f_win.clone());
                            tokio::spawn(async move {
                                let mut path = String::from("/api/control");
                                let mut proto = String::from("rex.control.v1");
                                let cb = |req: &server::Request, mut resp: server::Response| {
                                    path = req.uri().path().to_owned();
                                    if let Some(p) = req.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()) {
                                        proto = p.to_owned();
                                        resp.headers_mut().insert("sec-websocket-protocol", http::HeaderValue::from_str(&proto).unwrap());
                                    }
                                    Ok(resp)
                                };
                                let Ok(mut c_ws) = tokio_tungstenite::accept_hdr_async(client_stream, cb).await else { return };
                                let Ok(up_stream) = tokio::net::UnixStream::connect(&r_sock).await else { return };
                                let up_req = http::Request::builder()
                                    .uri(format!("ws://localhost{path}"))
                                    .header("Host", "localhost")
                                    .header("Upgrade", "websocket")
                                    .header("Connection", "Upgrade")
                                    .header("Sec-WebSocket-Version", "13")
                                    .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
                                    .header("Sec-WebSocket-Protocol", &proto)
                                    .body(())
                                    .unwrap();
                                let Ok((mut u_ws, _)) = tokio_tungstenite::client_async(up_req, up_stream).await else { return };
                                let is_ctrl = path == "/api/control";
                                loop {
                                    tokio::select! {
                                        msg = c_ws.next() => match msg {
                                            Some(Ok(Message::Text(txt))) if is_ctrl => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&txt)
                                                    && v["type"] == "request"
                                                {
                                                    let method = v["method"].as_str().unwrap_or("");
                                                    let wid = v["payload"]["window_id"].as_str();
                                                    let all_on = f_all.load(Ordering::Relaxed);
                                                    let win_hit = f_win.lock().unwrap().as_deref().is_some_and(|w| wid == Some(w));
                                                    let inject_err = (all_on && matches!(method, "session.close_window" | "session.view"))
                                                        || (win_hit && method == "session.close_window");
                                                    if inject_err {
                                                        let err_resp = json!({
                                                            "type": "response",
                                                            "id": v["id"],
                                                            "error": { "code": "busy", "message": "injected temporary RPC failure" }
                                                        });
                                                        let _ = c_ws.send(Message::Text(err_resp.to_string().into())).await;
                                                        continue;
                                                    }
                                                }
                                                if u_ws.send(Message::Text(txt)).await.is_err() { break; }
                                            }
                                            Some(Ok(m)) => if u_ws.send(m).await.is_err() { break; },
                                            _ => break,
                                        },
                                        msg = u_ws.next() => match msg {
                                            Some(Ok(m)) => if c_ws.send(m).await.is_err() { break; },
                                            _ => break,
                                        }
                                    }
                                }
                            });
                        }
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {
                            if st_flag.load(Ordering::Relaxed) { break; }
                        }
                    }
                }
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Self { sock: proxy_sock, fail_all, fail_window, stop, thread: Some(thread) }
    }
}

impl Drop for FaultyRexProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.sock);
    }
}

/// Round 3 Items 9 & 10 (#8, #19):
/// - Item 10 (#19): two non-`gone` RPC failures (`session.close_window` + `session.view`) MUST NOT count as a successful close or mark the live tab exited.
/// - Item 9 (#8, #19): migrating off a legacy 1:1 minder revokes legacy authority atomically in the same commit even if closing the old tab fails, AND failed close of a legacy minder tab is reported (never silently discarded) on both `--off` and migration.
#[test]
fn round3_9_10_legacy_revocation_and_rpc_close_failures() {
    let Some(mut rig) = Rig::start("r3-rpc-fail") else { return };
    rig.stand_in("claude");
    let real_sock = rig.sock.clone();
    let proxy_sock = rig.dir.join("rex-proxy.sock");
    let proxy = FaultyRexProxy::start(real_sock, proxy_sock.clone());
    // Route all agents-service calls through the proxy!
    rig.sock = proxy.sock.clone();

    // Item 10 (#19): start `w10`, then fail both `session.close_window` and `session.view`.
    let w10 = rig.start_child("claude", "w10", &[]);
    let w10_id = w10["id"].as_str().unwrap().to_owned();
    proxy.fail_all.store(true, Ordering::Relaxed);
    let (code, out, _err) = rig.olo(&["stop", "w10", "--force", "--json"]);
    proxy.fail_all.store(false, Ordering::Relaxed);
    let v: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("stop json: {e}\n{out}"));
    assert_ne!(code, 0, "stop with close + refresh RPC failures must fail: {v}");
    assert_eq!(v["ok"], json!(false), "{v}");
    assert!(v["stopped"].as_array().unwrap().is_empty(), "w10 must NOT be in stopped: {v}");
    assert!(!v["failed"].as_array().unwrap().is_empty(), "w10 must be in failed: {v}");
    assert_eq!(rig.status(&w10_id)["running"], json!(true), "live tab must NOT be marked exited after two RPC failures");

    // Item 9: set up two legacy 1:1 minders (`legm1` -> `legw1` for `--off`, `legm2` -> `legw2` for migration).
    let legw1 = rig.start_child("claude", "legw1", &[]);
    let legw1_id = legw1["id"].as_str().unwrap().to_owned();
    let legw1_blk = legw1["block_id"].as_str().unwrap().to_owned();
    let legm1 = rig.start_child_from(Some(&legw1_blk), "claude", "legm1", &[]);
    let legm1_id = legm1["id"].as_str().unwrap().to_owned();
    let legm1_win = legm1["window_id"].as_str().unwrap().to_owned();

    let legw2 = rig.start_child("claude", "legw2", &[]);
    let legw2_id = legw2["id"].as_str().unwrap().to_owned();
    let legw2_blk = legw2["block_id"].as_str().unwrap().to_owned();
    let legm2 = rig.start_child_from(Some(&legw2_blk), "claude", "legm2", &[]);
    let legm2_id = legm2["id"].as_str().unwrap().to_owned();
    let legm2_win = legm2["window_id"].as_str().unwrap().to_owned();
    let legm2_blk = legm2["block_id"].as_str().unwrap().to_owned();

    let old_pid = rig.service_pid().expect("service pid");
    let service_dir = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("registry.json").exists()).expect("service dir");
    assert!(rig.helper().args(["agent", "retire"]).output().unwrap().status.success());
    rig.wait("retired service exit", || !Path::new(&format!("/proc/{old_pid}")).exists());
    let reg_path = service_dir.join("registry.json");
    let mut reg: Value = serde_json::from_str(&std::fs::read_to_string(&reg_path).unwrap()).unwrap();
    for e in reg["entries"].as_array_mut().unwrap() {
        if e["id"] == legm1_id {
            e["role"] = json!("minder");
            e["minds"] = json!(legw1_id);
        } else if e["id"] == legm2_id {
            e["role"] = json!("minder");
            e["minds"] = json!(legw2_id);
        }
        e.as_object_mut().unwrap().remove("minded_by");
        e.as_object_mut().unwrap().remove("shared_minder");
    }
    std::fs::write(&reg_path, serde_json::to_string_pretty(&reg).unwrap()).unwrap();

    // Item 9a: `mind legw1 --off` when closing `legm1` fails MUST NOT silently claim ok!
    *proxy.fail_window.lock().unwrap() = Some(legm1_win.clone());
    let (off_code, off_out, off_err) = rig.olo(&["mind", "legw1", "--off", "--json"]);
    *proxy.fail_window.lock().unwrap() = None;
    assert_ne!(off_code, 0, "legacy --off must report failure when old legacy minder tab fails to close: {off_out}{off_err}");

    // Item 9b: migrating `legw2` off legacy `legm2` when closing `legm2` fails:
    // (1) legacy `legm2`'s authority MUST end in the same commit (cannot reach `legw2` even while tab survives!),
    // (2) failed close of `legm2` MUST be surfaced in the reply (not silently discarded!).
    Minding::expect(&rig, "r3-mig-m");
    let stage = format!("STAND_IN_DIR={}", rig.dir.join("stage").display());
    let path = format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display());
    *proxy.fail_window.lock().unwrap() = Some(legm2_win.clone());
    let (mig_code, mig_out, mig_err) = rig.olo(&["mind", "legw2", "--rules-text", "New rules.", "--env", &stage, "--env", &path, "--env", "STAND_IN_NAME=r3-mig-m", "--json"]);
    *proxy.fail_window.lock().unwrap() = None;
    let mig_json: Value = serde_json::from_str(mig_out.trim()).unwrap_or(Value::Null);
    assert!(
        mig_code != 0 || mig_json["old_minder_close_failed"].is_array() || !mig_err.is_empty(),
        "failed close of legacy minder tab during migration must be reported, not silently discarded: code={mig_code} out={mig_out} err={mig_err}"
    );
    // Even though `legm2`'s tab is still open, `legm2` MUST immediately lose authority over `legw2`!
    let (st_code, st_out, st_err) = rig.olo_from(Some(&legm2_blk), &["status", &legw2_id, "--json"]);
    assert_ne!(st_code, 0, "revoked legacy minder whose tab survived close failure must NOT still reach legw2: {st_out}{st_err}");

    // Restore real socket before Rig::drop retires the service cleanly.
    drop(proxy);
}

/// Round 3 Item 11 (#4):
/// Our new helper's `send --wait` must stay wire-compatible against a held older
/// service (`PROTO == 1`, pre-round-2 helper in `/var/tmp/olo-hs/takeover/olo-rex-link-before`)
/// that does NOT recognize `--wait` in `req.args`!
#[test]
fn round3_11_send_wait_against_held_older_service() {
    let old_helper = Path::new("/var/tmp/olo-hs/takeover/olo-rex-link-before");
    if !old_helper.is_file() {
        eprintln!("skipped: {} not present", old_helper.display());
        return;
    }
    let Some(rig) = Rig::start("r3-held-wait") else { return };
    rig.stand_in("claude");

    // Place a hold file where the service looks for `host-upgrade.hold` (`dir/host-upgrade.hold` for `OLO_REX_AGENTS_DIR=dir/agents`).
    std::fs::write(rig.dir.join("host-upgrade.hold"), "round3 compat test hold\n").unwrap();

    // Start the OLDER service using `olo-rex-link-before`!
    let mut old_cmd = Command::new(old_helper);
    old_cmd.current_dir(rig.dir.join("work")).env("PWD", rig.dir.join("work")).env("REX_BLOCK", &rig.lead_block).args(["agent", "list", "--fast", "--json"]);
    Rig::scratch_env(&rig.dir, &rig.sock, &mut old_cmd);
    old_cmd.env("REX_BLOCK", &rig.lead_block);
    let old_out = old_cmd.output().expect("old helper list");
    assert!(old_out.status.success(), "older service failed to start: {}", String::from_utf8_lossy(&old_out.stderr));
    let old_pid = rig.service_pid().expect("older service pid");

    // Start a stand-in worker `alpha` with a known session ID.
    let sid = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
    let _a = rig.start_child("claude", "alpha", &["--session-id", sid]);
    rig.show("alpha", &idle("Ready."));
    rig.wait_state("alpha", "idle");

    // Have the stand-in react to the incoming message by showing working, writing transcript, then idle!
    let keys_before = rig.keys("alpha").len();
    let work_dir = rig.work();
    let home_dir = rig.dir.join("home");
    let stage = rig.dir.join("stage");
    let react = std::thread::spawn(move || {
        let keys_file = stage.join("alpha.keys");
        let fifo = stage.join("alpha.fifo");
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            let typed = std::fs::read(&keys_file).unwrap_or_default();
            if typed.len() > keys_before && typed[keys_before..].contains(&b'\r') {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let s1 = stage.join("r3-w.screen");
        std::fs::write(&s1, working()).unwrap();
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo).and_then(|mut f| f.write_all(format!("show {}\n", s1.display()).as_bytes()));
        std::thread::sleep(Duration::from_millis(350));
        let folder: String = work_dir.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let tdir = home_dir.join(".claude/projects").join(folder);
        std::fs::create_dir_all(&tdir).unwrap();
        let lines = [
            json!({ "type": "user", "message": { "role": "user", "content": "ping older service" } }),
            json!({ "type": "assistant", "message": { "id": "msg_old", "role": "assistant", "content": [{ "type": "text", "text": "OLDER_SERVICE_OK" }], "stop_reason": "end_turn" } }),
        ];
        std::fs::write(tdir.join(format!("{sid}.jsonl")), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let s2 = stage.join("r3-i.screen");
        std::fs::write(&s2, idle("OLDER_SERVICE_OK")).unwrap();
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo).and_then(|mut f| f.write_all(format!("show {}\n", s2.display()).as_bytes()));
    });

    // Run OUR NEW helper's `send --enter --wait` against the HELD OLDER service!
    let (code, out, err) = rig.olo(&["send", "alpha", "ping older service", "--enter", "--wait", "--timeout", "10", "--json"]);
    react.join().unwrap();
    assert_eq!(rig.service_pid(), Some(old_pid), "held older service must not have handed over");
    assert_eq!(code, 0, "new helper's `send --wait` broke against held older service: stdout={out} stderr={err}");
    let v: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("send --wait json: {e}\n{out}"));
    assert_eq!(v["last_message"], json!("OLDER_SERVICE_OK"), "{v}");

    // Lift hold so Rig::drop can retire the older service cleanly.
    let _ = std::fs::remove_file(rig.dir.join("host-upgrade.hold"));
}

/// A question is answered once whatever its screen does meanwhile, and
/// only by who saw it: four cases the code review of the first fix
/// found (Astra 6.2, 2026-11-27), each failing without its fix.
///
/// A repaint caught half-way after the Return (a frame with nothing on
/// it, then the same box for a while yet: a slow agent) is not the
/// box's going: the list answers it once.
#[test]
fn a_repaints_empty_frame_is_not_a_box_gone() {
    let Some(rig) = Rig::start("once-repaint") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+(push|commit)\\b\n").unwrap();
    rig.start_child("claude", "slow", &["--approve-all-but", "not-these.txt"]);
    let before = rig.keys("slow").len();
    let blank = rig.when_typed("slow", b"\r", "\n\n".to_owned());
    rig.show("slow", &asking("cargo build -p olo-kit", 0));
    assert!(blank.join().unwrap(), "the list didn't answer\n{}", rig.said());
    std::thread::sleep(Duration::from_millis(120));
    rig.show("slow", &asking("cargo build -p olo-kit", 0));
    std::thread::sleep(Duration::from_millis(900));
    rig.show("slow", &working());
    rig.wait_state("slow", "working");
    std::thread::sleep(Duration::from_millis(400));
    let allowed = rig.json(&["mind", "slow", "--json"])["answered"].as_array().map_or(0, Vec::len);
    assert_eq!((String::from_utf8_lossy(&rig.keys("slow")[before..]).as_ref(), allowed), ("\r", 1), "a box seen again after an empty frame was answered again\n{}", rig.said());
    rig.json(&["stop", "slow", "--force", "--json"]);
}

/// …and where the screen stays empty for longer (a repaint that is slow
/// on a busy host, or the box gone and the very same asked again: the
/// screen can't say which), the same box seen next is a person's, not
/// the list's: one Return. With the agent seen at work between them,
/// the same asked again is another box, and the list's. (The test
/// above failed one whole run in two under a load of 95, its 120 ms of
/// nothing grown past the service's two readings: "\r\r".)
#[test]
fn the_same_box_after_only_an_empty_screen_is_a_persons() {
    let Some(rig) = Rig::start("once-blank") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+(push|commit)\\b\n").unwrap();
    let started = rig.start_child("claude", "blank", &["--approve-all-but", "not-these.txt"]);
    let before = rig.keys("blank").len();
    let blank = rig.when_typed("blank", b"\r", "\n\n".to_owned());
    rig.show("blank", &asking("cargo build -p olo-kit", 0));
    assert!(blank.join().unwrap(), "the list didn't answer\n{}", rig.said());
    // Wait deterministically until the service has finished the blank dismissal
    // (list logged its one allowed entry in `mind blank --json`).
    rig.wait("list answer recorded after blank dismissal", || rig.json(&["mind", "blank", "--json"])["answered"].as_array().is_some_and(|a| !a.is_empty()));
    rig.show("blank", &asking("cargo build -p olo-kit", 0));
    let asked = rig.json(&["watch", "blank", "--since", &started["cursor"].to_string(), "--on", "approval", "--timeout", "10", "--json"]);
    assert_eq!(asked["state"], "waiting_approval", "the same box after an empty screen wasn't told to anyone: {asked}");
    assert_eq!(String::from_utf8_lossy(&rig.keys("blank")[before..]), "\r", "the same box after only an empty screen was answered again by the list\n{}", rig.said());
    // (At work, then the very same asked: another box, the list's.)
    rig.show("blank", &working());
    rig.wait_state("blank", "working");
    let again = rig.when_typed("blank", b"\r", working());
    rig.show("blank", &asking("cargo build -p olo-kit", 0));
    assert!(again.join().unwrap(), "the same asked again after work wasn't the list's\n{}", rig.said());
    assert_eq!(String::from_utf8_lossy(&rig.keys("blank")[before..]), "\r\r");
    rig.json(&["stop", "blank", "--force", "--json"]);
}

/// Two askings with no box named, one behind the other: the second is
/// told an answer is on its way, and the box that comes next, which its
/// asker never saw, is not answered by it. One that names its box waits
/// its turn behind the active answer, and is refused for the box that
/// stands by then. Uses deterministic barriers (zero scheduling sleeps).
#[test]
fn an_asking_behind_another_answers_nothing_it_never_saw() {
    let Some(rig) = Rig::start("once-queued") else { return };
    rig.stand_in("claude");
    rig.start_child("claude", "pair", &[]);
    rig.show("pair", &asking("make one", 0));
    let box1_key = rig.wait_state("pair", "waiting_approval")["approval"]["box"].as_str().unwrap().to_owned();
    let before = rig.keys("pair").len();
    // Gate box 1's dismissal deterministically so `first` holds `answer_lock` while
    // BOTH an unkeyed request (`second`) AND a concurrent keyed request (`third`) arrive!
    let (seen_ret, release_next, next) = rig.when_typed_gated("pair", b"\r", asking("rm -rf build", 0));
    let (first, second, third) = std::thread::scope(|scope| {
        let first = scope.spawn(|| rig.olo(&["approve", "pair", "--once", "--json"]));
        // Wait deterministically until `first` has written `\r` and is inside dismissal wait!
        seen_ret.recv_timeout(Duration::from_secs(10)).expect("first approve did not write Enter");
        // Unkeyed concurrent caller is refused immediately without blocking.
        let second = rig.olo(&["approve", "pair", "--once", "--json"]);
        // Keyed concurrent caller (`--box box1_key`) waits behind `first`, then sees box 2 and is refused!
        let third = scope.spawn(|| rig.olo(&["approve", "pair", "--once", "--box", &box1_key, "--json"]));
        // Release box 2 now that `second` finished and `third` is queued behind `first`.
        let _ = release_next.send(());
        (first.join().unwrap(), second, third.join().unwrap())
    });
    assert!(next.join().unwrap());
    assert_eq!(first.0, 0, "{first:?}");
    rig.wait("the next box to stand", || rig.status("pair")["approval"]["command"].as_str().is_some_and(|c| c.contains("rm -rf build")));
    assert_eq!(String::from_utf8_lossy(&rig.keys("pair")[before..]), "\r", "the box that came next was answered by an asking that never saw it: second={second:?} third={third:?}");
    assert!(second.0 != 0 && second.2.contains("being answered"), "{second:?}");
    assert!(third.0 != 0 && (third.2.contains("isn't the one that stands now") || third.2.contains("already answered")), "concurrent keyed approve must refuse box 2: {third:?}");
    let named = rig.status("pair")["approval"]["box"].as_str().unwrap().to_owned();
    rig.show("pair", &asking("make two", 0));
    rig.wait("the box after", || rig.status("pair")["approval"]["command"].as_str().is_some_and(|c| c.contains("make two")));
    let (code, _, err) = rig.olo(&["approve", "pair", "--once", "--box", &named, "--json"]);
    assert!(code != 0 && err.contains("isn't the one that stands now"), "{err}");
    rig.json(&["stop", "pair", "--force", "--json"]);
}

/// A list taken away while the mark is walked to the yes allows
/// nothing: the arrow, and no Return or Escape. Deterministic barrier.
#[test]
fn a_list_taken_away_mid_answer_allows_nothing() {
    let Some(rig) = Rig::start("once-revoked") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+(push|commit)\\b\n").unwrap();
    let started = rig.start_child("claude", "revoked", &["--approve-all-but", "not-these.txt"]);
    let before = rig.keys("revoked").len();
    let (seen_up, release_redraw, moved) = rig.when_typed_gated("revoked", b"\x1b[A", asking("cargo fmt", 0));
    rig.show("revoked", &asking("cargo fmt", 1));
    seen_up.recv_timeout(Duration::from_secs(10)).expect("list did not send Up arrow");
    // Deterministically revoke the list BEFORE releasing the cursor-moved redraw!
    rig.json(&["mind", "revoked", "--approve-none", "--json"]);
    let _ = release_redraw.send(());
    assert!(moved.join().unwrap());
    // Wait until the revoked box is reported to the parent instead of auto-answered.
    let ev = rig.json(&["watch", "revoked", "--since", &started["cursor"].to_string(), "--on", "approval", "--timeout", "10", "--json"]);
    assert_eq!(ev["state"], "waiting_approval", "{ev}");
    let allowed = rig.json(&["mind", "revoked", "--json"])["answered"].as_array().map_or(0, Vec::len);
    assert_eq!((String::from_utf8_lossy(&rig.keys("revoked")[before..]).as_ref(), allowed), ("\x1b[A", 0), "a list taken away answered all the same\n{}", rig.said());
    rig.json(&["stop", "revoked", "--force", "--json"]);
}

/// A box that stood when the service was replaced is not the list's to
/// answer again: the one before may have. It is told to a person, strictly
/// beyond the predecessor's last cursor. Owns the fixture service `Child`
/// handle directly (NEVER `kill -9 <bare PID>` or `/proc/PID` wait).
#[test]
fn a_box_standing_at_a_takeover_is_a_persons() {
    let Some(rig) = Rig::start("once-takeover") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+(push|commit)\\b\n").unwrap();

    // Start the predecessor service as an owned `Child` handle before any client auto-spawns one.
    let mut svc_child = rig.helper().args(["agent", "--server", &rig.sock.display().to_string(), "serve"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    rig.wait("owned fixture service answers ping", || {
        rig.helper().args(["agent", "ping"]).output().ok().and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()).is_some_and(|v| v["pid"].as_u64() == Some(svc_child.id() as u64))
    });

    rig.start_child("claude", "kept", &["--approve-all-but", "not-these.txt"]);
    let before = rig.keys("kept").len();
    rig.show("kept", &asking("cargo check", 0));
    rig.wait("the list's one Return", || rig.keys("kept").len() > before);
    // Wait until the service has registered the standing approval and capture predecessor's last cursor.
    let pred_cursor = rig.helper().args(["agent", "ping"]).output().ok().and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()).and_then(|v| v["cursor"].as_u64()).unwrap_or(0);

    // Kill and reap OUR owned `Child` handle directly (immune to PID reuse!).
    svc_child.kill().unwrap();
    let _ = svc_child.wait();

    // Next client auto-starts the replacement service; watch strictly beyond `pred_cursor`.
    let asked = rig.json(&["watch", "kept", "--since", &pred_cursor.to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    assert_eq!(asked["state"], "waiting_approval", "{asked}");
    assert!(asked["cursor"].as_u64().unwrap() > pred_cursor, "event must be strictly beyond predecessor cursor {pred_cursor}: {asked}");
    assert_eq!(String::from_utf8_lossy(&rig.keys("kept")[before..]), "\r", "the service that took the child over answered its standing box again");
    rig.json(&["stop", "kept", "--force", "--json"]);
}

/// A folder's trust at a child's start: one question whether or not its
/// path is drawn yet, and not told to anyone while its answer is on
/// its way. Uses deterministic barrier on `\r`.
#[test]
fn a_folders_trust_is_one_question_and_not_told_while_answered() {
    let Some(rig) = Rig::start("trust-once") else { return };
    rig.stand_in("claude");
    let work = rig.work();
    let other = rig.dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let asks = |folder: &str| {
        format!(
            "{0}\n Accessing workspace:\n\n {folder}\n\n Quick safety check: Is this a project you created or one you trust?\n\n ❯ Yes, I trust this folder\n   No, exit\n\n Enter to confirm · Esc to cancel\n",
            rule()
        )
    };
    let started = rig.start_child("claude", "builder", &["--trust", "--cwd", &work, "--add-dir", &other.display().to_string()]);
    let before = rig.keys("builder").len();
    let (seen_ret, release_whole, whole) = rig.when_typed_gated("builder", b"\r", asks(&work));
    rig.show("builder", &asks(""));
    seen_ret.recv_timeout(Duration::from_secs(10)).expect("the trust wasn't answered");
    let _ = release_whole.send(());
    assert!(whole.join().unwrap(), "the trust redraw didn't land\n{}", rig.said());
    // Not told as a question while its answer is on its way…
    let (code, out, _) = rig.olo(&["watch", "builder", "--since", &started["cursor"].to_string(), "--on", "question,approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "a trust question whose answer was on its way was told: {out}");
    rig.show("builder", &idle("Ready."));
    rig.wait_state("builder", "idle");
    // …and answered once, though it was read two ways.
    assert_eq!(String::from_utf8_lossy(&rig.keys("builder")[before..]), "\r", "one question, read with and without its path, was answered twice");
    rig.json(&["stop", "builder", "--force", "--json"]);
}

/// A `--trust` child's own folder is the service's to answer from the
/// child's start, not from the moment the answering comes to it: its
/// question, standing with its mark not drawn yet for longer than a
/// confirmation, is not told as one.
#[test]
fn a_trust_at_a_start_is_the_services_before_it_is_answered() {
    let Some(rig) = Rig::start("trust-held") else { return };
    rig.stand_in("claude");
    let work = rig.work();
    let asks = |mark: &str| {
        format!(
            "{0}\n Accessing workspace:\n\n {work}\n\n Quick safety check: Is this a project you created or one you trust?\n\n {mark} Yes, I trust this folder\n   No, exit\n\n Enter to confirm · Esc to cancel\n",
            rule()
        )
    };
    let started = rig.start_child("claude", "builder", &["--trust", "--cwd", &work]);
    let before = rig.keys("builder").len();
    rig.show("builder", &asks(" "));
    // Unmarked trust prompt held > CONFIRM must NOT be reported as a question.
    let (code, out, _) = rig.olo(&["watch", "builder", "--since", &started["cursor"].to_string(), "--on", "question,approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "unmarked startup trust prompt was told: {out}");
    let gone = rig.when_typed("builder", b"\r", idle("Ready."));
    rig.show("builder", &asks("❯"));
    assert!(gone.join().unwrap(), "the trust wasn't answered\n{}", rig.said());
    rig.wait_state("builder", "idle");
    let (code, out, _) = rig.olo(&["watch", "builder", "--since", &started["cursor"].to_string(), "--on", "question,approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "a trust question the service was about to answer was told: {out}");
    assert_eq!(String::from_utf8_lossy(&rig.keys("builder")[before..]), "\r");
    // (One that isn't the child's own is told at once, as ever.)
    rig.show("builder", &asks("❯").replace(&work, "/somewhere/else"));
    let asked = rig.json(&["watch", "builder", "--since", &started["cursor"].to_string(), "--on", "question", "--timeout", "10", "--json"]);
    assert_eq!(asked["state"], "waiting_question", "{asked}");
    assert_eq!(String::from_utf8_lossy(&rig.keys("builder")[before..]), "\r", "a folder that isn't the child's was answered");
    rig.json(&["stop", "builder", "--force", "--json"]);
}

/// Regression test for Case 1: `--approve-all-but` must answer a single approval box
/// exactly once even when the child takes > `CONFIRM` (250 ms) to redraw after Enter,
/// and must still answer a genuine second identical box that appears afterwards.
#[test]
fn repro_case1_list_answers_twice_on_slow_redraw() {
    let Some(rig) = Rig::start("repro-case1") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+(push|commit)\\b\n").unwrap();
    let started = rig.start_child("claude", "worker", &["--approve-all-but", "not-these.txt"]);
    let from = started["cursor"].as_u64().unwrap();

    // Box 1: stand-in takes 450 ms (> CONFIRM 250 ms, < approve 2 s dismiss wait) to redraw after Enter.
    let before = rig.keys("worker").len();
    let gone = rig.when_typed_after("worker", b"\r", Duration::from_millis(450), working());
    rig.show("worker", &asking("cd /w && cargo test -p olo-kit > test.log", 0));
    assert!(gone.join().unwrap(), "the service didn't answer box 1\n{}", rig.said());
    rig.wait_state("worker", "working");
    let keys = rig.keys("worker");
    let typed = String::from_utf8_lossy(&keys[before..]).into_owned();
    let of = rig.json(&["mind", "worker", "--json"]);
    let allowed: Vec<(&str, &str, &str)> = of["answered"].as_array().unwrap().iter().map(|a| (a["did"].as_str().unwrap(), a["by"].as_str().unwrap(), a["said"].as_str().unwrap())).collect();
    assert_eq!(typed, "\r", "expected 1 Enter for box 1, got {typed:?}\n{of}");
    assert_eq!(allowed, [("allowed", "list", "Bash: cd /w && cargo test -p olo-kit > test.log")], "{of}");

    // Box 2: the exact same command asked a second time after `working()` must STILL be answered once!
    let before2 = rig.keys("worker").len();
    let gone2 = rig.when_typed_after("worker", b"\r", Duration::from_millis(450), working());
    rig.show("worker", &asking("cd /w && cargo test -p olo-kit > test.log", 0));
    assert!(gone2.join().unwrap(), "the service didn't answer identical box 2\n{}", rig.said());
    rig.wait_state("worker", "working");
    let keys2 = rig.keys("worker");
    let typed2 = String::from_utf8_lossy(&keys2[before2..]).into_owned();
    let of2 = rig.json(&["mind", "worker", "--json"]);
    let allowed2: Vec<(&str, &str, &str)> = of2["answered"].as_array().unwrap().iter().map(|a| (a["did"].as_str().unwrap(), a["by"].as_str().unwrap(), a["said"].as_str().unwrap())).collect();
    assert_eq!(typed2, "\r", "expected 1 Enter for identical box 2, got {typed2:?}\n{of2}");
    assert_eq!(allowed2, [("allowed", "list", "Bash: cd /w && cargo test -p olo-kit > test.log"), ("allowed", "list", "Bash: cd /w && cargo test -p olo-kit > test.log")], "{of2}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &from.to_string(), "--on", "approval,question", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "parent was woken for list-answered boxes: {out}");

    // Also verify manual `approve` on a slow-redrawing box does not emit a duplicate `waiting_approval` event
    // AND concurrent `approve` calls only send ONE Enter.
    let m = rig.start_child("claude", "manual", &[]);
    let m_cur = m["cursor"].as_u64().unwrap();
    rig.show("manual", &asking("git push origin main", 0));
    let ev1 = rig.json(&["watch", "manual", "--since", &m_cur.to_string(), "--on", "approval", "--timeout", "10", "--json"]);
    let after_ev1 = ev1["cursor"].as_u64().unwrap();
    let m_before = rig.keys("manual").len();
    let m_gone = rig.when_typed_after("manual", b"\r", Duration::from_millis(450), working());
    let (r1, r2) = std::thread::scope(|s| {
        let t1 = s.spawn(|| rig.olo(&["approve", "manual", "--once", "--json"]));
        let t2 = s.spawn(|| rig.olo(&["approve", "manual", "--once", "--json"]));
        (t1.join().unwrap(), t2.join().unwrap())
    });
    assert!(m_gone.join().unwrap(), "manual approve did not send Enter");
    assert_eq!(usize::from(r1.0 == 0) + usize::from(r2.0 == 0), 1, "exactly one concurrent approve must succeed: r1={r1:?} r2={r2:?}");
    rig.wait_state("manual", "working");
    let m_keys = rig.keys("manual");
    assert_eq!(String::from_utf8_lossy(&m_keys[m_before..]), "\r", "concurrent approves must write exactly one Enter");
    let (code, out, _) = rig.olo(&["watch", "manual", "--since", &after_ev1.to_string(), "--on", "approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "slow redraw after manual approve must not re-emit waiting_approval: {out}");
}

/// Regression test for Case 2: startup `--trust` must press Down once and Return once
/// even when the child takes > 100 ms to redraw cursor movement and > 250 ms to dismiss.
#[test]
fn repro_case2_trust_presses_down_twice_on_slow_redraw() {
    let Some(rig) = Rig::start("repro-case2") else { return };
    rig.stand_in("claude");
    rig.stand_in("agy");
    let work = rig.work();
    let agys = |folder: &str| {
        format!(
            "Accessing workspace:\n{folder}\nDo you trust the contents of this project?\nAntigravity CLI requires permission to read, edit, and execute files here.\n> Yes, I trust this folder\n  No, exit\n  ↑/↓ Navigate · enter Confirm\n"
        )
    };
    let claudes = |marked: usize| {
        let mark = |i: usize| if i == marked { "❯" } else { " " };
        format!(
            "{0}\n Accessing workspace:\n\n {work}\n\n Quick safety check: Is this a project you created or one you trust?\n\n {1} No, exit\n {2} Yes, I trust this folder\n\n Enter to confirm · Esc to cancel\n",
            rule(),
            mark(0),
            mark(1)
        )
    };

    // Claude: 180 ms redraw lag after Down (> 100 ms) and 400 ms after Return (> 250 ms).
    rig.start_child("claude", "builder", &["--trust", "--cwd", &work]);
    let before = rig.keys("builder").len();
    let moved = rig.when_typed_after("builder", b"\x1b[B", Duration::from_millis(180), claudes(1));
    let gone = rig.when_typed_after("builder", b"\r", Duration::from_millis(400), idle("Ready."));
    rig.show("builder", &claudes(0));
    assert!(moved.join().unwrap() && gone.join().unwrap(), "trust wasn't answered\n{}", rig.said());
    rig.wait_state("builder", "idle");
    let keys_c = rig.keys("builder");
    let typed_c = String::from_utf8_lossy(&keys_c[before..]).into_owned();

    // Agy: 400 ms redraw lag after Return (> 250 ms).
    rig.start_child("agy", "reader", &["--trust", "--cwd", &work]);
    let before_a = rig.keys("reader").len();
    let gone_a = rig.when_typed_after("reader", b"\r", Duration::from_millis(400), idle("Ready."));
    rig.show("reader", &agys(&work));
    assert!(gone_a.join().unwrap(), "agy trust wasn't answered\n{}", rig.said());
    rig.wait_state("reader", "idle");
    let keys_a = rig.keys("reader");
    let typed_a = String::from_utf8_lossy(&keys_a[before_a..]).into_owned();

    assert_eq!(typed_c, "\x1b[B\r", "claude trust typed: {typed_c:?}");
    assert_eq!(typed_a, "\r", "agy trust typed: {typed_a:?}");
}

/// Deterministic barrier test (Addendum Item 5) for serialized registry writes
/// AND `retire --to` waiting successor handover:
/// 1. Pins the background writer inside `write_lock` via `OLO_REX_AGENTS_TEST_WRITER_BARRIER`
///    (zero sleep races!).
/// 2. Starts a worker + minder AND calls `retire --to HELPER` while the background writer is
///    pinned -> releases the barrier -> verifies the predecessor finishes its final save,
///    drops its runtime before closing the handover pipe, and the successor takes over with a
///    new PID knowing every child and persisting the newest registry!
#[test]
fn a_slow_write_of_the_registry_never_lands_over_a_newer_one() {
    let barrier_dir = PathBuf::from(format!("/var/tmp/olo-writer-barrier-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&barrier_dir);
    std::fs::create_dir_all(&barrier_dir).unwrap();
    let enter = barrier_dir.join("entered");
    let release = barrier_dir.join("release");
    let barrier_env = format!("{}:{}", enter.display(), release.display());

    let Some(rig) = Rig::start_with_env("writer-race", &[("OLO_REX_AGENTS_TEST_WRITER_BARRIER", &barrier_env), ("OLO_REX_AGENTS_MINDER_REPORTS_MS", "8000")]) else {
        let _ = std::fs::remove_dir_all(&barrier_dir);
        return;
    };
    rig.stand_in("claude");

    // Start `prev`, then stop it so the background writer triggers and pins on the barrier!
    let prev = rig.start_child("claude", "prev", &[]);
    rig.json(&["stop", prev["id"].as_str().unwrap(), "--force", "--json"]);
    rig.wait("background writer to hit deterministic barrier", || enter.exists());

    // Start a brand-new child `fresh` on a thread while the background writer holds `write_lock`
    // (its `begin_attached::write_now` queues deterministically behind the pinned writer!),
    // wait deterministically for `fresh` to register (proving it is queued on `write_lock`),
    // verify the barrier did NOT expire on its own, then release it!
    let expired = enter.with_extension("expired");
    let fresh = std::thread::scope(|scope| {
        let start_t = scope.spawn(|| rig.start_child("claude", "fresh", &[]));
        rig.wait("fresh registered and queued on write_lock", || rig.json(&["list", "--fast", "--json"])["subagents"].as_array().is_some_and(|a| a.iter().any(|e| e["name"] == "fresh")));
        assert!(!expired.exists(), "writer barrier timed out before fresh queued!");
        std::fs::write(&release, b"go\n").unwrap();
        start_t.join().unwrap()
    });
    assert!(!expired.exists(), "writer barrier must not have expired");
    let file = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path().join("registry.json")).find(|p| p.exists()).expect("the registry's file");
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(written["entries"].as_array().unwrap().iter().any(|e| e["id"] == fresh["id"]), "a slow write landed over a newer one: the file has no `fresh`\n{written}");

    // Now exercise `retire --to <HELPER>` with an actual waiting successor!
    Minding::expect(&rig, "em-hand");
    let hw = rig.start_child("claude", "hw", &["--minder", "claude:stand-in-big:max", "--rules-text", "r", "--minder-env", "STAND_IN_NAME=em-hand"]);
    let (minder, worker) = (hw["minder"]["id"].as_str().unwrap().to_owned(), hw["id"].as_str().unwrap().to_owned());
    rig.tell("hw", "exit 0");
    rig.wait_state("hw", "exited");
    let old_pid = rig.service_pid().expect("a service");
    let old_ticks = proc_start_ticks(old_pid).expect("predecessor start_ticks");
    let out = rig.helper().args(["agent", "retire", "--to", HELPER]).output().unwrap();
    assert!(out.status.success(), "retire --to failed: {}", String::from_utf8_lossy(&out.stderr));
    // Wait for predecessor to exit (checking start_ticks against PID reuse) AND successor to answer ping with a new PID.
    rig.wait("predecessor service to exit", || proc_start_ticks(old_pid) != Some(old_ticks));
    let mut new_pid = None;
    rig.wait("successor service started via --to to answer ping", || {
        new_pid = rig.service_pid().filter(|&p| p != old_pid);
        new_pid.is_some()
    });
    let (code, out_s, said) = rig.olo(&["status", &minder, "--json"]);
    assert_eq!(code, 0, "successor doesn't know minder started before handover: {out_s} {said}\n{}", rig.said());
    assert_eq!(rig.status(&worker)["running"], json!(false));
    assert_eq!(rig.status("fresh")["running"], json!(true));
    rig.json(&["stop", &minder, "--force", "--json"]);
    rig.json(&["stop", "fresh", "--force", "--json"]);
    let _ = std::fs::remove_dir_all(&barrier_dir);
}

/// Update 11 Findings 1 & 8:
/// - Empty `--box ""` / `--box=""` is rejected with exit code 2 (no accidental unkeyed fallback).
/// - A queued minder `approve` without a pre-existing grant (or bound to box 1) can NEVER wait
///   behind another answer and spend a future grant minted for box 2!
#[test]
fn update11_queued_minder_cannot_spend_future_grant_and_empty_box_exit_2() {
    let Some(rig) = Rig::start("u11-minder-q") else { return };
    rig.stand_in("claude");
    Minding::expect(&rig, "u11m1");
    let started = rig.start_child("claude", "worker", &["--minder", "claude:stand-in-big:max", "--rules-text", "Allow tests.", "--minder-env", "STAND_IN_NAME=u11m1"]);
    let minder_id = started["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "u11m1", &minder_id);
    let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");

    rig.show("worker", &asking("cargo test -p one", 0));
    let ev1 = minder.mind("watch --since 0 --on approval --timeout 15 --json");
    assert_eq!(ev1["state"], "waiting_approval", "{ev1}");

    // Empty `--box ""` and `--box=""` MUST exit 2 and send ZERO keys!
    let before_empty = rig.keys("worker").len();
    let (c1, _, e1) = rig.olo(&["approve", "worker", "--once", "--box", "", "--json"]);
    let (c2, _, e2) = rig.olo(&["approve", "worker", "--once", "--box=", "--json"]);
    assert_eq!(c1, 2, "empty --box must exit 2: {e1}");
    assert_eq!(c2, 2, "empty --box= must exit 2: {e2}");
    assert_eq!(rig.keys("worker").len(), before_empty, "empty --box must write zero bytes");

    // Hold parent's approval of box 1 inside dismissal wait while box 2 is gated;
    // fire a concurrent minder `approve` and release box 2 without any fixed sleep
    // (deterministic poll-driven `answer_lock` contention is verified in
    // `service.rs::box_tests::round2_confirm_retry_...`), confirming zero keys land on box 2!
    let before = rig.keys("worker").len();
    let (seen_ret, release_box2, draw_box2) = rig.when_typed_gated("worker", b"\r", asking("rm -rf /danger", 0));
    let (parent_res, queued_minder_out) = std::thread::scope(|scope| {
        let p = scope.spawn(|| rig.olo(&["approve", "worker", "--once", "--json"]));
        seen_ret.recv_timeout(Duration::from_secs(10)).expect("parent approve did not send Enter");
        let qm = scope.spawn(|| minder.run(&format!("{} {} approve worker --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display())));
        let _ = release_box2.send(());
        (p.join().unwrap(), qm.join().unwrap())
    });
    assert!(draw_box2.join().unwrap());
    assert_eq!(parent_res.0, 0, "{parent_res:?}");
    assert!(queued_minder_out.contains("status=1"), "queued minder approve bound to box 1 must fail on box 2: {queued_minder_out}");
    rig.wait("box 2 stands unanswered", || rig.status("worker")["approval"]["command"].as_str().is_some_and(|c| c.contains("rm -rf /danger")));
    assert_eq!(String::from_utf8_lossy(&rig.keys("worker")[before..]), "\r", "queued minder approve must NOT confirm box 2!");
    rig.json(&["stop", "worker", "--force", "--json"]);
}

/// Update 11 Findings 2, 3, 19 + Addendum Item 4:
/// (a) Foreign folder appearing after Down on startup `--trust` aborts WITHOUT pressing Return.
/// (b) Partial `--add-dir` frame (`Yes, add it` without header/path) followed by full path gets
///     a single Return (never confirmed on pathless frame).
/// (c) Blank frame between primary trust and `--add-dir` trust does NOT end startup trust early.
/// (d) Slow queued background registry write (`OLO_REX_AGENTS_TEST_WRITER_BARRIER`) does NOT
///     eat the 15 s startup trust window: trust answers immediately while the writer is blocked!
#[test]
fn update11_startup_trust_foreign_partial_blank_and_slow_save() {
    let barrier_dir = PathBuf::from(format!("/var/tmp/olo-trust-barrier-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&barrier_dir);
    std::fs::create_dir_all(&barrier_dir).unwrap();
    let enter = barrier_dir.join("entered");
    let release = barrier_dir.join("release");
    let barrier_env = format!("{}:{}", enter.display(), release.display());

    let Some(rig) = Rig::start_with_env("u11-trust", &[("OLO_REX_AGENTS_TEST_WRITER_BARRIER", &barrier_env)]) else {
        let _ = std::fs::remove_dir_all(&barrier_dir);
        return;
    };
    rig.stand_in("claude");
    let work = rig.work();
    let extra = rig.dir.join("extra-dir");
    std::fs::create_dir_all(&extra).unwrap();
    let extra_s = extra.display().to_string();

    let primary_prompt = |folder: &str, marked: usize| {
        let m = |i: usize| if i == marked { "❯" } else { " " };
        format!(
            "{0}\n Accessing workspace:\n\n {folder}\n\n Quick safety check: Is this a project you created or one you trust?\n\n {1} No, exit\n {2} Yes, I trust this folder\n\n Enter to confirm · Esc to cancel\n",
            rule(),
            m(0),
            m(1)
        )
    };
    let partial_add_dir = format!("{0}\n Is this a directory you created or one you trust?\n\n ❯ Yes, add it\n   No, exit\n\n Enter to confirm · Esc to cancel\n", rule());
    let full_add_dir =
        format!("{0}\n Adding a new directory:\n\n {extra_s}\n\n Is this a directory you created or one you trust?\n\n ❯ Yes, add it\n   No, exit\n\n Enter to confirm · Esc to cancel\n", rule());

    // (d) First pin the background writer at the barrier, then start a `--trust` child:
    //     startup trust MUST run immediately BEFORE `write_now()` completes!
    let decoy = rig.start_child("claude", "decoy", &[]);
    rig.json(&["stop", decoy["id"].as_str().unwrap(), "--force", "--json"]);
    rig.wait("writer pinned at barrier", || enter.exists());

    // Start `trusty` in a thread (its `start` reply awaits `write_now` behind the barrier),
    // wait for its stand-in FIFO to become ready, show primary trust prompt, and verify
    // startup trust answers it BEFORE we even release the writer barrier!
    let trusty_fifo = rig.staged("trusty", "fifo");
    assert!(Command::new("mkfifo").arg(&trusty_fifo).status().unwrap().success());
    let trusty_keys_file = rig.staged("trusty", "keys");
    let trusty_ready = rig.staged("trusty", "ready");
    let stage_env = format!("STAND_IN_DIR={}", rig.dir.join("stage").display());
    let path_env = format!("PATH={}:/usr/bin:/bin", rig.dir.join("bin").display());
    let trusty_started = std::thread::scope(|scope| {
        let start_t = scope.spawn(|| {
            rig.json(&[
                "start",
                "claude",
                "--model",
                "stand-in",
                "--effort",
                "none",
                "--name",
                "trusty",
                "--trust",
                "--cwd",
                &work,
                "--add-dir",
                &extra_s,
                "--json",
                "--env",
                &stage_env,
                "--env",
                "STAND_IN_NAME=trusty",
                "--env",
                &path_env,
            ])
        });
        // Wait for stand-in tab to boot while `start` is still blocked behind the writer barrier!
        rig.wait("trusty stand-in ready while writer barrier still held", || trusty_ready.exists());
        let gone_blank = rig.when_typed("trusty", b"\r", "\n\n".to_owned());
        rig.show("trusty", &primary_prompt(&work, 1));
        assert!(gone_blank.join().unwrap(), "startup trust MUST answer primary prompt immediately even while registry write is blocked!");
        assert!(!release.exists(), "answered while barrier was still held");
        assert!(!enter.with_extension("expired").exists(), "writer barrier timed out before trust answered!");
        // Now release writer barrier so `start` finishes its durable write and returns.
        std::fs::write(&release, b"go\n").unwrap();
        start_t.join().unwrap()
    });
    assert!(!enter.with_extension("expired").exists(), "writer barrier must not have expired");
    assert_eq!(trusty_started["ok"], true);
    let keys_after_primary = std::fs::read(&trusty_keys_file).unwrap_or_default().len();
    assert_eq!(keys_after_primary, 1, "exactly one Return for primary prompt");

    // (b) & (c) After a blank frame between primary and `--add-dir`, show a partial `--add-dir`
    //     frame (`Yes, add it` without path) -> must NOT press Return yet!
    rig.show("trusty", &partial_add_dir);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(rig.keys("trusty").len(), keys_after_primary, "partial pathless add-dir frame must NOT receive Return");
    // Now draw the full `--add-dir` prompt with its path -> startup trust (still alive across the blank!) answers with ONE Return!
    let add_done = rig.when_typed("trusty", b"\r", idle("All trusted."));
    rig.show("trusty", &full_add_dir);
    assert!(add_done.join().unwrap(), "full add-dir after blank frame was not answered by startup trust\n{}", rig.said());
    rig.wait_state("trusty", "idle");
    assert_eq!(String::from_utf8_lossy(&rig.keys("trusty")), "\r\r", "expected exactly one Return for primary and one for add-dir");
    rig.json(&["stop", "trusty", "--force", "--json"]);

    // (a) Foreign folder appearing after Down on startup `--trust` MUST abort WITHOUT pressing Return!
    rig.start_child("claude", "foreign", &["--trust", "--cwd", &work]);
    let f_before = rig.keys("foreign").len();
    let swapped = rig.when_typed("foreign", b"\x1b[B", primary_prompt("/var/tmp/not-my-folder", 1));
    rig.show("foreign", &primary_prompt(&work, 0));
    assert!(swapped.join().unwrap(), "foreign trust did not send initial Down");
    // Wait for the foreign folder prompt to be escalated as a question (proving trust exited without Return).
    let q = rig.json(&["watch", "foreign", "--since", "0", "--on", "question", "--timeout", "10", "--json"]);
    assert_eq!(q["state"], "waiting_question", "{q}");
    assert_eq!(String::from_utf8_lossy(&rig.keys("foreign")[f_before..]), "\x1b[B", "foreign folder after Down must NEVER receive Return!");
    rig.json(&["stop", "foreign", "--force", "--json"]);
    let _ = std::fs::remove_dir_all(&barrier_dir);
}

/// Update 11 Findings 4, 6, 7:
/// (a) `--approve-all-but` rechecks the LATEST prompt during navigation: if `⋯ 12 lines hidden`
///     appears on the verifying frame, aborts immediately with ZERO Escape and ZERO Return.
/// (b) Incomplete 2-choice repaint (`1. Yes` + `2. Yes, always`, no refusal) or bare
///     `esc to interrupt` footer after `BoxEnd::Blank` does NOT rearm the list.
/// (c) Positive intervening `Working` during dismissal retires box 1 immediately so an
///     identical box 2 IS answered by the list.
#[test]
fn update11_list_rechecks_folded_lines_repaint_partial_and_positive_elsewhere() {
    let Some(rig) = Rig::start("u11-list") else { return };
    rig.stand_in("claude");
    std::fs::write(rig.dir.join("work/not-these.txt"), "\\bgit\\s+(push|commit)\\b\n").unwrap();
    let started = rig.start_child("claude", "listed", &["--approve-all-but", "not-these.txt"]);
    let mut cur = started["cursor"].as_u64().unwrap();

    // (a) Prompt initially starts on choice 1 (needs Up arrow) with no folded lines;
    //     when Up arrives, redraws with `⋯ 12 lines hidden` on choice 0 -> MUST NOT press Return or Escape!
    let dashed = "╌".repeat(60);
    let folded_box = format!(
        "● Bash(cargo test)\n{0}\n Bash command\n Make it so\n{dashed}\n │ cargo test\n   ⋯ 12 lines hidden\n{dashed}\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don’t ask again for: cargo test\n   3. No\n Esc to cancel · Tab to amend\n",
        rule()
    );
    let before_a = rig.keys("listed").len();
    let fold_redraw = rig.when_typed("listed", b"\x1b[A", folded_box);
    rig.show("listed", &asking("cargo test", 1));
    assert!(fold_redraw.join().unwrap(), "list did not send Up arrow");
    let ev_fold = rig.json(&["watch", "listed", "--since", &cur.to_string(), "--on", "approval", "--timeout", "10", "--json"]);
    assert_eq!(ev_fold["state"], "waiting_approval", "folded box must fall through to parent: {ev_fold}");
    cur = ev_fold["cursor"].as_u64().unwrap();
    assert_eq!(String::from_utf8_lossy(&rig.keys("listed")[before_a..]), "\x1b[A", "folded lines on verify frame must send ZERO Return and ZERO Escape");

    // Clear with positive working state.
    rig.show("listed", &working());
    rig.wait_state("listed", "working");

    // (b) Trigger `BoxEnd::Blank` on `cargo fmt`, then flash a bare `esc to interrupt` footer
    //     and an incomplete 2-choice repaint -> neither may rearm `list_skip`!
    let before_b = rig.keys("listed").len();
    let blank_draw = rig.when_typed("listed", b"\r", "\n\n".to_owned());
    rig.show("listed", &asking("cargo fmt", 0));
    assert!(blank_draw.join().unwrap(), "list did not answer cargo fmt");
    rig.wait("list recorded one answer", || rig.json(&["mind", "listed", "--json"])["answered"].as_array().is_some_and(|a| a.len() == 1));
    // Flash bare `esc to interrupt` footer (NOT genuinely working!) and incomplete 2-choice repaint.
    rig.show("listed", "  esc to interrupt\n");
    std::thread::sleep(Duration::from_millis(200));
    rig.show("listed", "Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don’t ask again for: cargo fmt\n");
    std::thread::sleep(Duration::from_millis(200));
    // Redraw identical `cargo fmt` box -> must STILL be skipped by list and told to parent!
    rig.show("listed", &asking("cargo fmt", 0));
    let ev_b = rig.json(&["watch", "listed", "--since", &cur.to_string(), "--on", "approval", "--timeout", "10", "--json"]);
    assert_eq!(ev_b["state"], "waiting_approval", "partial/bare footer must NOT rearm list: {ev_b}");
    assert_eq!(String::from_utf8_lossy(&rig.keys("listed")[before_b..]), "\r", "list must NOT answer cargo fmt a second time after bare footer");

    // (c) Positive intervening `Working` clears `list_skip` so an identical `cargo fmt` IS answered!
    rig.show("listed", &working());
    rig.wait_state("listed", "working");
    let before_c = rig.keys("listed").len();
    let ok_c = rig.when_typed("listed", b"\r", working());
    rig.show("listed", &asking("cargo fmt", 0));
    assert!(ok_c.join().unwrap(), "identical box after positive Working must be answered by list");
    assert_eq!(String::from_utf8_lossy(&rig.keys("listed")[before_c..]), "\r");
    rig.json(&["stop", "listed", "--force", "--json"]);
}

/// Update 11 Findings 9 & 12:
/// (a) Quiet ordinary chooser Q covered by startup `--trust` is confirmed and published once
///     trust finishes (suppression records a dedupe boundary).
/// (b) `approve --no --reason "FIX"` NEVER pastes `"FIX"` into a blank repaint (`BoxEnd::Blank`)
///     or an ordinary chooser Q!
#[test]
fn update11_quiet_chooser_under_trust_and_no_reason_never_pastes_into_chooser() {
    let Some(rig) = Rig::start("u11-reason") else { return };
    rig.stand_in("claude");
    let work = rig.work();
    let trust_prompt = format!(
        "{0}\n Accessing workspace:\n\n {work}\n\n Quick safety check: Is this a project you created or one you trust?\n\n ❯ Yes, I trust this folder\n   No, exit\n\n Enter to confirm · Esc to cancel\n",
        rule()
    );
    let chooser_q = "Which deployment target should I use?\n ❯ 1. Staging\n   2. Production\n Enter to select · Esc to cancel\n";

    // (a) Child starts with `--trust`: trust prompt answered -> immediately reveals quiet chooser Q ->
    //     Q must be confirmed and published!
    let started = rig.start_child("claude", "trustq", &["--trust", "--cwd", &work]);
    let to_q = rig.when_typed("trustq", b"\r", chooser_q.to_owned());
    rig.show("trustq", &trust_prompt);
    assert!(to_q.join().unwrap(), "startup trust was not answered");
    let q_ev = rig.json(&["watch", "trustq", "--since", &started["cursor"].to_string(), "--on", "question", "--timeout", "10", "--json"]);
    assert_eq!(q_ev["state"], "waiting_question", "chooser Q after startup trust must be published: {q_ev}");
    rig.json(&["stop", "trustq", "--force", "--json"]);

    // (b) `approve --no --reason`: when refusal lands on blank screen (`BoxEnd::Blank`),
    //     ZERO reason text is pasted (`dismissal == "blank"`, `reason_sent == false`)!
    rig.start_child("claude", "refuse", &[]);
    rig.show("refuse", &asking("git push origin main", 2));
    rig.wait_state("refuse", "waiting_approval");
    let b1 = rig.keys("refuse").len();
    let to_blank = rig.when_typed("refuse", b"\r", "\n\n".to_owned());
    let r_blank = rig.json(&["approve", "refuse", "--no", "--reason", "DO_NOT_PASTE_BLANK", "--json"]);
    assert!(to_blank.join().unwrap());
    assert_eq!(r_blank["dismissal"], "blank", "{r_blank}");
    assert_eq!(r_blank["reason_sent"], false, "{r_blank}");
    assert!(!String::from_utf8_lossy(&rig.keys("refuse")[b1..]).contains("DO_NOT_PASTE_BLANK"), "must NEVER paste reason on BoxEnd::Blank");

    // When refusal lands on an ordinary chooser Q, ZERO reason text is pasted!
    rig.show("refuse", &working());
    rig.wait_state("refuse", "working");
    rig.show("refuse", &asking("git push --force", 2));
    rig.wait_state("refuse", "waiting_approval");
    let b2 = rig.keys("refuse").len();
    let to_chooser = rig.when_typed("refuse", b"\r", chooser_q.to_owned());
    let r_chooser = rig.json(&["approve", "refuse", "--no", "--reason", "DO_NOT_PASTE_CHOOSER", "--json"]);
    assert!(to_chooser.join().unwrap());
    assert_eq!(r_chooser["reason_sent"], false, "{r_chooser}");
    assert!(!String::from_utf8_lossy(&rig.keys("refuse")[b2..]).contains("DO_NOT_PASTE_CHOOSER"), "must NEVER paste reason into chooser Q");
    rig.json(&["stop", "refuse", "--force", "--json"]);
}

/// Update 11 Finding 15 + Bonus Non-Bridge PATH Fix:
/// (a) Standing approval already delivered to a shared minder before `retire --to` handover
///     is redelivered ONCE by the successor service so the minder gets a fresh grant and can approve it!
/// (b) Starting a child with `--desktop none` preserves non-bridge user bin dirs from caller `PATH`
///     (even when `rex-server` has minimal `PATH`), while strictly stripping `/rex-commands` and `/olo/rex/bin`.
#[test]
fn update11_handover_redelivers_standing_minded_approval_and_preserves_non_bridge_path() {
    let Some(rig) = Rig::start("u11-handover-path") else { return };
    rig.stand_in("claude");

    // (a) Shared minder receives approval before handover (`seen` updated), service retires `--to HELPER`
    //     (`told` starts empty in successor) -> successor redelivers standing approval once so minder can approve!
    Minding::expect(&rig, "u11m2");
    let started = rig.start_child("claude", "worker", &["--minder", "claude:stand-in-big:max", "--rules-text", "Allow tests.", "--minder-env", "STAND_IN_NAME=u11m2"]);
    let minder_id = started["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "u11m2", &minder_id);
    let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");

    rig.show("worker", &asking("cargo test -p handover", 0));
    let ev1 = minder.mind("watch --since 0 --on approval --timeout 15 --json");
    assert_eq!(ev1["state"], "waiting_approval", "{ev1}");
    let old_box_key = ev1["approval"]["box"].as_str().expect("predecessor box key").to_owned();

    // Hand over via `retire --to HELPER` while the approval is still standing!
    let old_pid = rig.service_pid().expect("predecessor pid");
    let old_ticks = proc_start_ticks(old_pid).expect("predecessor start_ticks");
    assert!(rig.helper().args(["agent", "retire", "--to", HELPER]).output().unwrap().status.success());
    rig.wait("predecessor exited", || proc_start_ticks(old_pid) != Some(old_ticks));
    rig.wait("successor online", || rig.service_pid().is_some_and(|p| p != old_pid));

    // Successor must redeliver the standing approval once to the minder with a FRESH generation box key!
    let ev2 = minder.mind("watch --since 0 --on approval --timeout 15 --json");
    assert_eq!(ev2["state"], "waiting_approval", "successor must redeliver standing approval to minder after handover: {ev2}");
    let new_box_key = ev2["approval"]["box"].as_str().expect("successor box key").to_owned();
    assert_ne!(old_box_key, new_box_key, "A8: successor box key must carry a new service_gen and NEVER equal predecessor's key");
    // A delayed parent approve carrying the predecessor's old box key MUST be refused with ZERO keys written!
    let keys_before_stale = rig.keys("worker").len();
    let (stale_code, _, stale_err) = rig.olo(&["approve", "worker", "--once", "--box", &old_box_key, "--json"]);
    assert_eq!(stale_code, 1, "stale predecessor box key must be refused after handover: {stale_err}");
    assert_eq!(rig.keys("worker").len(), keys_before_stale, "stale predecessor box key must write zero bytes");
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = minder.mind("approve worker --once --json");
    assert!(gone.join().unwrap(), "minder could not approve redelivered box after handover: {approved}");
    assert_eq!(approved["ok"], true, "{approved}");
    rig.json(&["stop", "worker", "--force", "--json"]);

    // (b) Non-bridge PATH fix: create a custom user bin dir AND two fake bridge dirs (`/rex-commands`, `/olo/rex/bin`).
    //     Put a custom CLI in user bin dir and a decoy command in bridge dirs. Launch under `--desktop none`
    //     with caller `PATH` containing all three -> child MUST find user bin CLI and MUST NOT have bridge dirs in PATH!
    let user_bin = rig.dir.join("custom-user-bin");
    let bridge1 = rig.dir.join("state/rex-commands");
    let bridge2 = rig.dir.join("share/olo/rex/bin");
    for d in [&user_bin, &bridge1, &bridge2] {
        std::fs::create_dir_all(d).unwrap();
    }
    let out_file = rig.dir.join("stage/path-check.txt");
    let custom_cli = user_bin.join("my-custom-cli");
    std::fs::write(&custom_cli, format!("#!/bin/sh\nprintf '%s\\n' \"$PATH\" > {}\n", out_file.display())).unwrap();
    std::fs::set_permissions(&custom_cli, std::fs::Permissions::from_mode(0o755)).unwrap();

    let caller_path = format!("{}:{}:{}:/usr/bin:/bin", bridge1.display(), user_bin.display(), bridge2.display());
    let mut cmd = rig.helper();
    cmd.env("REX_BLOCK", &rig.lead_block).env("PATH", &caller_path).args(["agent", "start", "command", "--name", "pathcheck", "--desktop", "none", "--json", "--", "my-custom-cli"]);
    let res = cmd.output().unwrap();
    assert!(res.status.success(), "start command using non-bridge PATH failed: {}", String::from_utf8_lossy(&res.stderr));
    rig.wait("custom CLI ran and wrote PATH", || out_file.exists());
    let child_path = std::fs::read_to_string(&out_file).unwrap();
    let dirs: Vec<&str> = child_path.trim().split(':').collect();
    assert!(dirs.contains(&user_bin.to_str().unwrap()), "child PATH must preserve non-bridge user bin dir: {child_path}");
    assert!(!dirs.iter().any(|d| d.ends_with("/rex-commands") || d.ends_with("/olo/rex/bin")), "child PATH under --desktop none must strip bridge dirs: {child_path}");
}

/// Round 3 Integration Tests (A7 / N3 + Astra Test Gap #2 / N12.1):
/// (a) A standing `WaitingApproval` recorded BEFORE handover that its shared
///     minder has NOT YET READ (`seen < last_seq`) MUST be redelivered with the
///     successor's fresh box key after `retire --to` so the minder can approve it!
/// (b) Handover drain-timeout abort & socket rebind via `OLO_REX_AGENTS_TEST_DRAIN_MS`:
///     holding an admitted `start` across the shortened drain deadline causes the
///     incumbent to abort handover, rebind its socket, complete the start, and
///     continue serving under the SAME PID!
#[test]
fn round3_unread_minded_approval_survives_handover_and_drain_timeout_aborts_cleanly() {
    // (a) Unread pre-handover minded approval (`seen < last_seq`).
    let Some(rig) = Rig::start("r3-unread-ho") else { return };
    rig.stand_in("claude");
    Minding::expect(&rig, "r3m1");
    let started = rig.start_child("claude", "worker", &["--minder", "claude:stand-in-big:max", "--rules-text", "Allow tests.", "--minder-env", "STAND_IN_NAME=r3m1"]);
    let minder_id = started["minder"]["id"].as_str().unwrap().to_owned();
    let minder = Minding::of(&rig, "r3m1", &minder_id);
    let _ = minder.mind("watch --since 0 --on minding --timeout 15 --json");

    // Show approval and wait specifically for the predecessor service to journal
    // `WaitingApproval` for `worker` in `events.jsonl` WITHOUT the minder consuming
    // it (`seen < last_seq` — Astra regression check)!
    rig.show("worker", &asking("cargo test -p unread-handover", 0));
    let worker_id = started["id"].as_str().unwrap().to_owned();
    rig.wait("predecessor journals unread approval for worker", || {
        let Some(service_dir) = std::fs::read_dir(rig.dir.join("agents")).ok().into_iter().flatten().flatten().map(|d| d.path()).find(|d| d.join("events.jsonl").exists()) else {
            return false;
        };
        std::fs::read_to_string(service_dir.join("events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .any(|ev| ev["id"] == worker_id && ev["state"] == "waiting_approval")
    });

    // Retire `--to HELPER` BEFORE the minder watches!
    let old_pid = rig.service_pid().expect("predecessor pid");
    let old_ticks = proc_start_ticks(old_pid).expect("predecessor ticks");
    assert!(rig.helper().args(["agent", "retire", "--to", HELPER]).output().unwrap().status.success());
    rig.wait("predecessor exited", || proc_start_ticks(old_pid) != Some(old_ticks));
    rig.wait("successor online", || rig.service_pid().is_some_and(|p| p != old_pid));

    // Minder now watches on the successor -> MUST receive the standing approval with the successor's box key and approve it!
    let ev = minder.mind("watch --since 0 --on approval --timeout 15 --json");
    assert_eq!(ev["state"], "waiting_approval", "A7/N3: unread pre-handover approval must be delivered after handover: {ev}");
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = minder.mind("approve worker --once --json");
    assert!(gone.join().unwrap(), "minder could not approve unread pre-handover box: {approved}");
    assert_eq!(approved["ok"], true, "{approved}");
    rig.json(&["stop", "worker", "--force", "--json"]);

    // (b) Barrier-held handover drain-timeout abort + non-zero `retire` exit + socket rebind
    //     via `OLO_REX_AGENTS_TEST_DRAIN_MS` + `OLO_REX_AGENTS_TEST_WRITER_BARRIER`!
    let barrier_dir = PathBuf::from(format!("/var/tmp/olo-drain-barrier-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&barrier_dir);
    std::fs::create_dir_all(&barrier_dir).unwrap();
    let enter = barrier_dir.join("entered");
    let release = barrier_dir.join("release");
    let barrier_env = format!("{}:{}", enter.display(), release.display());
    let Some(rig_d) = Rig::start_with_env("r3-drain-abort", &[("OLO_REX_AGENTS_TEST_DRAIN_MS", "300"), ("OLO_REX_AGENTS_TEST_WRITER_BARRIER", &barrier_env)]) else {
        let _ = std::fs::remove_dir_all(&barrier_dir);
        return;
    };
    rig_d.stand_in("claude");
    // Pin the background writer at the barrier FIRST (so `slowstart`'s
    // `write_now_committing` blocks deterministically on `write_lock` with `doing == 1`).
    let decoy = rig_d.start_child("claude", "decoy", &[]);
    rig_d.json(&["stop", decoy["id"].as_str().unwrap(), "--force", "--json"]);
    rig_d.wait("writer pinned at barrier", || enter.exists());
    let pid_before = rig_d.service_pid().expect("incumbent pid");
    let ticks_before = proc_start_ticks(pid_before).expect("incumbent ticks");
    let base_wins = rig_d.rex("session.view", json!({ "session_id": rig_d.session }))["windows"].as_array().unwrap().len();

    let (start_res, retire_out) = std::thread::scope(|scope| {
        let start_t = scope.spawn(|| rig_d.start_child("claude", "slowstart", &[]));
        // Wait deterministically until `slowstart`'s tab exists in Rex
        // (so `start` is now awaiting `write_now_committing` behind the barrier
        // with `doing == 1`), then trigger `retire --to HELPER` which MUST hit
        // the 300 ms drain deadline, abort handover, rebind the socket, and
        // return NON-ZERO to the `retire` caller (Neon #1 / Astra #22)!
        rig_d.wait("slowstart tab created and blocked at writer barrier", || {
            rig_d.rex("session.view", json!({ "session_id": rig_d.session }))["windows"].as_array().is_some_and(|w| w.len() > base_wins)
        });
        let out = rig_d.helper().args(["agent", "retire", "--to", HELPER]).output().unwrap();
        // Release the barrier ONLY AFTER `retire` has returned its abort outcome!
        std::fs::write(&release, b"go\n").unwrap();
        (start_t.join().unwrap(), out)
    });
    let _ = std::fs::remove_dir_all(&barrier_dir);
    assert!(!retire_out.status.success(), "Neon #1/Astra #22: aborted retire must return non-zero, got: {retire_out:?}");
    assert_eq!(start_res["ok"], true, "admitted start must complete cleanly despite drain-timeout handover abort: {start_res}");
    // Incumbent MUST still be the running service (same PID & start_ticks) with its socket rebound and responsive!
    rig_d.wait("incumbent service rebound and answering ping", || rig_d.service_pid() == Some(pid_before));
    assert_eq!(proc_start_ticks(pid_before), Some(ticks_before), "incumbent PID must have survived handover abort");
    assert!(rig_d.said().contains("handover aborted"), "log should record handover abort: {}", rig_d.said());
    assert_eq!(rig_d.status("slowstart")["running"], json!(true));
    rig_d.json(&["stop", "slowstart", "--force", "--json"]);
}
