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
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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

struct Rig {
    dir: PathBuf,
    sock: PathBuf,
    server: Child,
    session: String,
    lead_window: String,
    lead_block: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        // (The service ends with its server; asked to first, so what it
        // writes on its way out is written before the folder goes.)
        let _ = self.helper().args(["agent", "retire"]).output();
        let _ = self.server.kill();
        let _ = self.server.wait();
        std::thread::sleep(Duration::from_millis(100));
        if std::env::var_os("OLO_AGENTS_TEST_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Rig {
    /// A Rex server of this test's own with one session and one tab in
    /// it (the lead's); none where Rex isn't installed.
    fn start(tag: &str) -> Option<Self> {
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
        let mut rig = Self { dir, sock, server, session: String::new(), lead_window: String::new(), lead_block: String::new() };
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
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .env_remove("SSH_AUTH_SOCK");
        // (Run from an agent's own shell, the test carries that agent's
        // marks: none of them is the scratch server's or its children's.)
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|n| n.starts_with("CLAUDE") || n.starts_with("CODEX") || n == "AI_AGENT") {
                cmd.env_remove(name);
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
        let out = cmd.output().expect("rex api call");
        assert!(out.status.success(), "rex api call {method}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn helper(&self) -> Command {
        let mut cmd = Command::new(HELPER);
        cmd.current_dir(self.dir.join("work")).env("PWD", self.dir.join("work"));
        Self::scratch_env(&self.dir, &self.sock, &mut cmd);
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

    /// Tells a stand-in one thing (waited for: it takes a line when it
    /// has done with the last).
    fn tell(&self, name: &str, line: &str) {
        let (fifo, line) = (self.staged(name, "fifo"), format!("{line}\n"));
        let (done, told) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let sent = std::fs::OpenOptions::new().write(true).open(&fifo).and_then(|mut f| f.write_all(line.as_bytes()));
            let _ = done.send(sent.is_ok());
        });
        assert_eq!(told.recv_timeout(Duration::from_secs(10)), Ok(true), "the stand-in {name} didn't take a line");
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
        let (file, from) = (self.staged(name, "keys"), self.keys(name).len());
        let (fifo, stage) = (self.staged(name, "fifo"), self.staged(name, &format!("then{from}")));
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
            std::fs::write(&stage, then).unwrap();
            std::fs::OpenOptions::new().write(true).open(&fifo).and_then(|mut f| f.write_all(format!("show {}\n", stage.display()).as_bytes())).is_ok()
        })
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
    assert_eq!(&rig.keys("builder")[before..], b"\r", "Return and nothing else");

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
    assert_eq!(&rig.keys("builder")[before..], b"\x1b[200~Next: the tests.\nThen say DONE.\n\x1b[201~\r");

    // What it shows is read.
    let read = rig.json(&["read", "builder", "--lines", "20", "--json"]);
    assert!(read["output"].as_str().unwrap().contains("Thinking"), "{read}");

    // A key with Alt, as tmux's `M-m`.
    let before = rig.keys("builder").len();
    rig.json(&["send", "builder", "--key", "Alt+m", "--json"]);
    rig.wait("the key", || rig.keys("builder").len() > before);
    assert_eq!(&rig.keys("builder")[before..], b"\x1bm");

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
    let said = next();
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
    assert_eq!(&rig.keys("surveyed")[before..], b"0", "once, and nothing else");
    assert_eq!(rig.status("surveyed")["survey"], false);
    rig.json(&["stop", "surveyed", "--force", "--json"]);
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
    adopt.stdin.take().unwrap().write_all(json!({ "agents": [kept, gone] }).to_string().as_bytes()).unwrap();
    let said = adopt.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&said.stdout).contains("Took over 2"), "{said:?}");
    let listed = rig.json(&["list", "--all", "--json"]);
    let by_name = |name: &str| listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name}: {listed}")).clone();
    assert_eq!(by_name("old")["state"], "working", "its tab is there: live");
    assert_eq!(by_name("old")["cli_session_id"], "99999999-8888-7777-6666-555555555555");
    assert_eq!(by_name("older")["state"], "exited", "its tab is gone: kept as exited");
    // (Given again, they are not taken twice.)
    let mut again = rig.helper().args(["agent", "adopt"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    again.stdin.take().unwrap().write_all(json!({ "agents": [by_name("old")] }).to_string().as_bytes()).unwrap();
    assert!(String::from_utf8_lossy(&again.wait_with_output().unwrap().stdout).contains("Took over 0"));
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
        cmd.current_dir(rig.dir.join("work")).env("PWD", rig.dir.join("work")).env("REX_BLOCK", &rig.lead_block);
        Rig::scratch_env(&rig.dir, &rig.sock, &mut cmd);
        let out = cmd.arg("agent").args(args).output().expect("the helper");
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
    ask(&later_build, &["stop", "steady", "--force", "--json"]);
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
    assert_eq!(&rig.keys("reader")[before..], b"\x1b[B\x1b[B\x1b[B\r");

    // No, once: the third, not the no that blocks from now on.
    rig.show("reader", &read_outside(0));
    rig.wait_state("reader", "waiting_approval");
    let before = rig.keys("reader").len();
    let moved = rig.when_typed("reader", b"\x1b[B\x1b[B", read_outside(2));
    let gone = rig.when_typed("reader", b"\r", idle("Not read."));
    let refused = rig.json(&["approve", "reader", "--no", "--json"]);
    assert!(moved.join().unwrap() && gone.join().unwrap());
    assert_eq!(refused["chosen_label"], "No, and ask again next time", "{refused}");
    assert_eq!(&rig.keys("reader")[before..], b"\x1b[B\x1b[B\r");
    // (Where the mark doesn't come to stand on it, Escape and not Return.)
    rig.show("reader", &read_outside(0));
    rig.wait_state("reader", "waiting_approval");
    let before = rig.keys("reader").len();
    let (code, _, err) = rig.olo(&["approve", "reader", "--once", "--json"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("Escape instead of Enter"), "{err}");
    assert_eq!(&rig.keys("reader")[before..], b"\x1b[B\x1b[B\x1b[B\x1b", "never Return where the mark wasn't seen to land");
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
    assert_eq!(&rig.keys("walker")[before..], b"\x1b[B\r");
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
    let pid = rig.status("guarded")["pid"].as_i64().unwrap();
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
