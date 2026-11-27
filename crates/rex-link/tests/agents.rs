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
            // (How long a minder has to report once its agent has ended: three seconds here, of three minutes.)
            .env("OLO_REX_AGENTS_MINDER_REPORTS_MS", "3000")
            // (How long an agent at rest has to take up a message sent to it before its rest is told all the same: 2.5 s here, of thirty.)
            .env("OLO_REX_AGENTS_TAKEN_UP_MS", "2500")
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
    // (Where the mark doesn't come to stand on it, Escape and not Return.)
    rig.show("reader", &read_outside(0));
    rig.wait_state("reader", "waiting_approval");
    let before = rig.keys("reader").len();
    let (code, _, err) = rig.olo(&["approve", "reader", "--once", "--json"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("Escape instead of Enter"), "{err}");
    rig.typed_since("reader", before, b"\x1b[B\x1b[B\x1b[B\x1b", "never Return where the mark wasn't seen to land");
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

    fn of(rig: &'a Rig, stand_in: &str, minded_id: &str) -> Self {
        rig.wait("the minder's stand-in to listen", || rig.staged(stand_in, "ready").exists());
        let service = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("minders").is_dir()).expect("the service's folder has its minders'");
        Self { rig, stand_in: stand_in.to_owned(), dir: service.join("minders").join(minded_id) }
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

/// A minder (`agents::minder`): started with its agent, it alone is told
/// what the agent asks, and answers it; it reaches that agent and nothing
/// else; what it escalates is on the parent's own watch; and its report
/// once the agent has ended is its last word.
#[test]
fn a_minder_answers_for_its_agent_and_tells_the_parent() {
    let Some(rig) = Rig::start("minder") else { return };
    rig.stand_in("claude");
    let other = rig.start_child("claude", "other", &[]);
    Minding::expect(&rig, "minds-worker");
    let rules = "Approve `cargo test`.\nRefuse any push, and say to open a pull request instead.";
    let started = rig.start_child("claude", "worker", &["--minder", "claude:stand-in-big:max", "--rules-text", rules, "--minder-env", "STAND_IN_NAME=minds-worker"]);
    let worker_id = started["id"].as_str().unwrap();
    let minder = Minding::of(&rig, "minds-worker", worker_id);
    assert_eq!(
        (&started["minder"]["name"], &started["minder"]["cli"], &started["minder"]["model"], &started["minder"]["effort"]),
        (&json!("worker-minder"), &json!("claude"), &json!("stand-in-big"), &json!("max")),
        "{started}"
    );

    // What it is: a child tab of the agent it minds, said so wherever either is listed; its rules and its one command in its folder.
    let listed = rig.json(&["list", "--all", "--json"]);
    let row = |name: &str| listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name} isn't listed: {listed}")).clone();
    assert_eq!((&row("worker-minder")["parent_window_id"], &row("worker-minder")["minds"], &row("worker-minder")["role"]), (&started["window_id"], &started["id"], &json!("minder")), "{listed}");
    assert_eq!((&row("worker")["minder"], &row("other")["minder"]), (&started["minder"]["id"], &Value::Null), "{listed}");
    assert_eq!(std::fs::read_to_string(minder.dir.join("rules.md")).unwrap(), rules);
    assert_eq!(std::fs::metadata(minder.dir.join("mind")).unwrap().permissions().mode() & 0o777, 0o700);
    // What it was started with: the shell and the three tools that read; its command the one thing allowed in the shell, reading allowed, nothing asked; the parent's rules in its standing instructions.
    let line = minder.run("tr '\\0' '\\n' < /proc/$PPID/cmdline");
    let args: Vec<&str> = line.lines().collect();
    let after = |flag: &str| args.iter().position(|a| *a == flag).and_then(|i| args.get(i + 1)).copied().unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    assert_eq!((after("--model"), after("--effort"), after("--permission-mode"), after("--tools")), ("stand-in-big", "max", "dontAsk", "Bash,Read,Grep,Glob"), "{args:?}");
    let allowed = args.iter().position(|a| *a == "--allowed-tools").map(|i| &args[i + 1..i + 5]).unwrap_or_else(|| panic!("no --allowed-tools in {args:?}"));
    assert_eq!(allowed, ["Read", "Grep", "Glob", &format!("Bash({} *)", minder.dir.join("mind").display())], "{args:?}");
    assert!(args.contains(&"--strict-mcp-config"), "{args:?}");
    let told = line.split("--append-system-prompt\n").nth(1).unwrap_or_default();
    // (With where its agent works: rules that speak of a folder are read against that.)
    assert!(
        told.starts_with(&format!("You are a minder. Your whole job is one other agent, \"worker\" (Claude Code, stand-in, working in {})", rig.dir.join("work").display()))
            && told.contains(&format!("----\n{rules}\n----")),
        "{told}"
    );
    assert!(args.last().unwrap().starts_with("Begin minding \"worker\" now. Run: ") && args.last().unwrap().contains(&format!("watch --since {}", started["cursor"])), "{args:?}");

    // The agent asks leave. Its minder is told; the parent's watch is not woken by it.
    let mut cursor = started["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("cargo test", 0));
    let asked = minder.mind(&format!("watch --since {cursor} --on question,approval,idle,error,exit --timeout 15 --json"));
    assert_eq!((&asked["name"], &asked["event"]), (&json!("worker"), &json!("waiting_approval")), "{asked}");
    assert!(asked["approval"]["command"].as_str().unwrap().contains("cargo test"), "{asked}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &cursor.to_string(), "--on", "approval,question,escalation", "--timeout", "2", "--json"]);
    assert_eq!(code, 124, "the parent was woken by what is its minder's to answer: {out}");
    let (code, out, _) = rig.olo(&["watch", "worker", "--on", "approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "…nor by how it stands now: {out}");
    // (The parent can still see it, and answer it itself, if it looks.)
    assert_eq!(rig.status("worker")["state"], "waiting_approval");

    // The minder says yes, once: typed into the agent as the parent's own yes is.
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = minder.mind("approve --once --json");
    assert!(gone.join().unwrap(), "{approved}");
    assert_eq!((&approved["chosen_label"], &approved["dismissed"]), (&json!("Yes"), &json!(true)), "{approved}");
    rig.typed_since("worker", before, b"\r", "what was typed into it");
    cursor = asked["cursor"].as_u64().unwrap();

    // What isn't a minder's: any other agent, and anything but watching, reading, answering and escalating, whatever pane it says it is.
    for refused in [
        "stop worker --force",
        "list --all --json",
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
    assert_eq!((&rig.status("worker")["running"], &rig.status("other")["running"]), (&json!(true), &json!(true)));
    assert_eq!(other["name"], "other");

    // What the rules don't cover goes up: an event on the parent's own watch, about the agent, in the minder's words.
    rig.show("worker", &asking("git push origin main", 0));
    let asked = minder.mind(&format!("watch --since {cursor} --on question,approval,idle,error,exit --timeout 15 --json"));
    assert!(asked["approval"]["command"].as_str().unwrap().contains("git push"), "{asked}");
    let lead_from = cursor;
    cursor = asked["cursor"].as_u64().unwrap();
    let before = rig.keys("worker").len();
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("git push origin main", 2));
    let gone = rig.when_typed("worker", b"\r", working());
    let refused = minder.mind("approve --no --reason 'No pushes: open a pull request instead.' --json");
    assert!(moved.join().unwrap() && gone.join().unwrap(), "{refused}");
    assert_eq!(refused["chosen_label"], "No", "{refused}");
    assert!(rig.keys("worker")[before..].starts_with(b"\x1b[B\x1b[B"), "down to No, never Return on Yes: {:?}", String::from_utf8_lossy(&rig.keys("worker")[before..]));
    // (Its words whole, as a shell gives them: nothing in them is run.)
    let said = "worker asked to push to main ($HOME, \"quoted\"). Refused by your rule; it was told to open a pull request.";
    let up = minder.mind(&format!("escalate '{said}' --json"));
    assert_eq!((&up["event"], &up["name"]), (&json!("escalation"), &json!("worker")), "{up}");
    let heard = rig.json(&["watch", "--all", "--since", &lead_from.to_string(), "--timeout", "15", "--json"]);
    assert_eq!(
        (&heard["event"], &heard["name"], &heard["summary"], &heard["cursor"]),
        (&json!("escalation"), &json!("worker"), &json!(said), &up["cursor"]),
        "the first thing the parent hears since before the question: {heard}"
    );
    // (Once: the next watch from there has nothing; and a watch for other things alone isn't woken by it.)
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &heard["cursor"].to_string(), "--on", "escalation,approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &lead_from.to_string(), "--on", "approval,question", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");

    // Asked of: who minds it, by what rules, and what was answered, in order.
    let of = rig.json(&["mind", "worker", "--json"]);
    assert_eq!((&of["minder"]["id"], &of["rules"]), (&started["minder"]["id"], &json!(rules)), "{of}");
    let did: Vec<&str> = of["answered"].as_array().unwrap().iter().filter_map(|a| a["did"].as_str()).collect();
    assert_eq!(did, ["approve", "approve", "escalate"], "{of}");
    // (Each with the minder that gave it.)
    assert!(of["answered"].as_array().unwrap().iter().all(|a| a["by"] == started["minder"]["id"]), "{of}");
    assert_eq!(of["answered"][2]["said"], said);
    let (code, out, _) = rig.olo(&["mind", "worker"]);
    assert!(code == 0 && out.starts_with("worker is minded by worker-minder (claude:stand-in-big:max)\n") && out.contains("escalate"), "{out}");

    // The agent ends. Its minder hears it and reports; that is its last word: it is stopped at once, well before its time is up.
    rig.tell("worker", "exit 0");
    let ended = minder.mind(&format!("watch --since {cursor} --on question,approval,idle,error,exit --timeout 15 --json"));
    assert_eq!((&ended["name"], &ended["state"]), (&json!("worker"), &json!("exited")), "{ended}");
    let reported = Instant::now();
    let report = minder.mind("escalate 'worker has ended. Approved: cargo test. Refused: a push.' --json");
    let heard = rig.json(&["watch", "--all", "--since", &ended["cursor"].to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&heard["event"], &heard["state"], &heard["cursor"]), (&json!("escalation"), &json!("exited"), &report["cursor"]), "{heard}");
    let minder_id = started["minder"]["id"].as_str().unwrap();
    rig.wait("the minder to be stopped after its report", || rig.status(minder_id)["running"] == false);
    assert!(reported.elapsed() < Duration::from_millis(2500), "stopped by its report, not by its time running out: {:?}", reported.elapsed());
    // (Its end isn't news: nothing more is said of it to the parent.)
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &heard["cursor"].to_string(), "--on", "escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
}

/// A minder for an agent that runs already (`olo agent mind`): attached,
/// its rules changed, taken off; one that ends by itself is said to the
/// parent, whose own the agent's asking is again; and one whose agent has
/// ended doesn't outlive it.
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

    // Attached to it as it runs, the rules from a file (read where it was asked).
    std::fs::write(rig.dir.join("work/rules.md"), "Approve `cargo test`.\n").unwrap();
    let first = mind(&["worker", "--with", "claude:stand-in-big:max", "--rules", "rules.md"], "m1");
    let m1 = Minding::of(&rig, "m1", &worker_id);
    assert_eq!((&first["minder"]["name"], &first["replaced"]), (&json!("worker-minder"), &Value::Null), "{first}");
    assert_eq!(std::fs::read_to_string(m1.dir.join("rules.md")).unwrap(), "Approve `cargo test`.\n");
    let from = first["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("cargo test", 0));
    let asked = m1.mind(&format!("watch --since {from} --timeout 15 --json"));
    assert_eq!(asked["event"], "waiting_approval", "{asked}");
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &from.to_string(), "--on", "approval,escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");

    // New rules: the same minder begun anew with them (the one before stopped, and its end not said: the parent asked for it).
    let second = mind(&["worker", "--rules-text", "Approve nothing."], "m2");
    let m2 = Minding::of(&rig, "m2", &worker_id);
    assert_eq!((&second["replaced"], &second["minder"]["model"], &second["minder"]["effort"]), (&first["minder"]["id"], &json!("stand-in-big"), &json!("max")), "{second}");
    assert_ne!(second["minder"]["id"], first["minder"]["id"]);
    assert_eq!(std::fs::read_to_string(m2.dir.join("rules.md")).unwrap(), "Approve nothing.");
    assert!(m2.run("tr '\\0' '\\n' < /proc/$PPID/cmdline").contains("----\nApprove nothing.\n----"));
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &from.to_string(), "--on", "approval,escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "a minder put in another's place was said to have ended, or the question came to the parent between them: {out}");
    // (What stands asked is the new minder's to answer: it is told how the agent stands.)
    assert_eq!(m2.mind("status --json")["state"], "waiting_approval");

    // A minder that ends by itself, its agent still running: the parent is told, and what the agent asks is the parent's again, from where it was.
    rig.tell("m2", "exit 3");
    let heard = rig.json(&["watch", "--all", "--since", &from.to_string(), "--on", "escalation", "--timeout", "15", "--json"]);
    assert_eq!((&heard["event"], &heard["name"]), (&json!("escalation"), &json!("worker")), "{heard}");
    // (How it ended is said as it was first heard: its program's status, or that its tab closed, whichever word came first.)
    let how = heard["summary"].as_str().unwrap();
    assert!(how.starts_with("Its minder ended (exited (code 3))") || how.starts_with("Its minder ended (its tab was closed)"), "{heard}");
    assert!(how.ends_with("what it asks comes to you again."), "{heard}");
    assert_eq!(rig.json(&["mind", "worker", "--json"])["minder"], Value::Null);
    let now_mine = rig.json(&["watch", "--all", "--since", &from.to_string(), "--on", "approval", "--timeout", "15", "--json"]);
    assert!(now_mine["approval"]["command"].as_str().unwrap().contains("cargo test"), "{now_mine}");

    // Taken off: stopped, nothing said of its end, and the asking the parent's again.
    let third = mind(&["worker", "--with", "claude:stand-in-big:high"], "m3");
    let _m3 = Minding::of(&rig, "m3", &worker_id);
    assert_eq!(std::fs::read_to_string(m1.dir.join("rules.md")).unwrap(), "Approve nothing.", "no rules given: those it had");
    let (code, out, _) = rig.olo(&["watch", "worker", "--on", "approval", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
    let off = rig.json(&["mind", "worker", "--off", "--json"]);
    assert_eq!((&off["stopped"], &off["minder"]), (&third["minder"]["id"], &Value::Null), "{off}");
    rig.wait("the minder taken off to be gone", || rig.status(third["minder"]["id"].as_str().unwrap())["running"] == false);
    let (code, out, _) = rig.olo(&["watch", "--all", "--since", &heard["cursor"].to_string(), "--on", "escalation", "--timeout", "1", "--json"]);
    assert_eq!(code, 124, "{out}");
    assert_eq!(rig.json(&["watch", "worker", "--on", "approval", "--timeout", "15", "--json"])["state"], "waiting_approval");

    // A minder whose agent has ended and that says nothing: stopped when its time is up, not left minding nothing.
    let fourth = mind(&["worker", "--with", "claude:stand-in-big:max"], "m4");
    let fourth = fourth["minder"]["id"].as_str().unwrap();
    let _m4 = Minding::of(&rig, "m4", &worker_id);
    let ended = Instant::now();
    rig.tell("worker", "exit 0");
    rig.wait_state("worker", "exited");
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(rig.status(fourth)["running"], true, "it has three seconds to report");
    rig.wait("the minder of an agent that ended to be stopped", || rig.status(fourth)["running"] == false);
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
    let minder = Minding::of(&rig, "m", started["id"].as_str().unwrap());
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
    let said = minder.run(&format!("{} {} approve --once --json; echo status=$?", minder.env(), minder.dir.join("mind").display()));
    assert!(said.contains("isn't what you were last told of") && said.contains("status=1"), "{said}");
    assert_eq!(rig.keys("worker").len(), before, "something was typed into the agent");
    assert_eq!(rig.status("worker")["state"], "waiting_approval");
    // It looks again, is told what stands asked, and answers that.
    let stands = minder.mind("status --json");
    assert!(stands["approval"]["command"].as_str().unwrap().contains("rm -rf build"), "{stands}");
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("rm -rf build", 2));
    let gone = rig.when_typed("worker", b"\r", working());
    let refused = minder.mind("approve --no --reason 'Not in the rules.' --json");
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
#[test]
fn a_rest_is_the_end_of_the_turn_it_says() {
    let Some(rig) = Rig::start("turns") else { return };
    rig.stand_in("claude");
    let started = rig.start_child("claude", "alpha", &[]);
    let from = started["cursor"].as_u64().unwrap();
    let watch = |since: u64, secs: &str| rig.olo(&["watch", "alpha", "--since", &since.to_string(), "--on", "idle,done,question,approval,error,exit", "--timeout", secs, "--json"]);
    let told = |since: u64| -> Value {
        let (code, out, err) = watch(since, "15");
        assert_eq!(code, 0, "{out}{err}\n{}", rig.said());
        serde_json::from_str(out.trim()).unwrap()
    };

    // What stands in its tab before its own screen is drawn (the tab's shell, here as it stood on the day)
    // is not the agent at rest, whatever was read of the tab first.
    rig.show("alpha", "[systemd]\nFailed Units: 1\n  systemd-zram-setup@zram0.service\n");
    let (code, out, _) = watch(from, "2");
    assert_eq!(code, 124, "the tab's shell was told as the agent at rest: {out}");
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
    let started = rig.start_child("claude", "worker", &[]);
    let worker_id = started["id"].as_str().unwrap().to_owned();
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
    let m1 = Minding::of(&rig, "m1", &worker_id);
    let (said, took) = first_watch(&m1, &one, 20);
    assert!(told(&said)["approval"]["command"].as_str().unwrap().contains("printf hello"), "a minder attached to an agent already asking wasn't told what it asks: {said}");
    assert!(took < Duration::from_secs(4), "told only after {took:?}");
    // It passes the question up and leaves it standing: it is not told it again.
    m1.mind("escalate 'It asks to write out.txt.' --json");
    let (said, _) = first_watch(&m1, &told(&said), 1);
    assert!(said.contains("status=124"), "told the same standing question twice: {said}");

    // 2. Its rules replaced while the agent still asks: the minder begun anew is told at once.
    let two = mind(&["worker", "--rules-text", "Approve writes of out.txt."], "m2");
    let m2 = Minding::of(&rig, "m2", &worker_id);
    let (said, took) = first_watch(&m2, &two, 20);
    assert!(told(&said)["approval"]["command"].as_str().unwrap().contains("printf hello") && took < Duration::from_secs(4), "after {took:?}: {said}");

    // 3. That minder killed while the agent asks, and another attached: told at once, and its yes is typed.
    rig.tell("m2", "exit 9");
    rig.wait("the killed minder to be gone", || rig.json(&["mind", "worker", "--json"])["minder"].is_null());
    let three = mind(&["worker", "--with", "claude:stand-in-big:max"], "m3");
    let m3 = Minding::of(&rig, "m3", &worker_id);
    let (said, took) = first_watch(&m3, &three, 20);
    assert!(told(&said)["approval"]["command"].as_str().unwrap().contains("printf hello") && took < Duration::from_secs(4), "after {took:?}: {said}");
    let before = rig.keys("worker").len();
    let gone = rig.when_typed("worker", b"\r", working());
    let approved = m3.mind("approve --once --json");
    assert!(gone.join().unwrap(), "{approved}");
    rig.typed_since("worker", before, b"\r", "what was typed into it");
    // (Each answer is kept with the minder that gave it: the first's escalation, the third's yes.)
    let of = rig.json(&["mind", "worker", "--json"]);
    let by: Vec<(&str, &Value)> = of["answered"].as_array().unwrap().iter().map(|a| (a["did"].as_str().unwrap(), &a["by"])).collect();
    assert_eq!(by, [("escalate", &one["minder"]["id"]), ("approve", &three["minder"]["id"])], "{of}");

    // An agent at work: a minder that begins is told nothing (what was last written of it, its question, no longer stands).
    rig.wait_state("worker", "working");
    rig.json(&["mind", "worker", "--off", "--json"]);
    let four = mind(&["worker", "--with", "claude:stand-in-big:max"], "m4");
    let m4 = Minding::of(&rig, "m4", &worker_id);
    let (said, _) = first_watch(&m4, &four, 1);
    assert!(said.contains("status=124"), "told of a question that was answered before it began: {said}");
    // …and an agent at rest: told that it rests, once.
    shown(&idle("Wrote out.txt."));
    rig.json(&["mind", "worker", "--off", "--json"]);
    let five = mind(&["worker", "--with", "claude:stand-in-big:max"], "m5");
    let m5 = Minding::of(&rig, "m5", &worker_id);
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
        rig.wait("the next message to be typed into it", || rig.keys("partner").len() > typed);
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
    assert!(at.elapsed() < Duration::from_secs(4), "{:?}", at.elapsed());
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
    let lm = Minding::of(&rig, "lm", started["id"].as_str().unwrap());
    let c = minded["cursor"].as_u64().unwrap();
    rig.show("worker", &asking("git commit -am wip", 0));
    let told = lm.mind(&format!("watch --since {c} --on question,approval,idle,error,exit --timeout 15 --json"));
    assert!(told["approval"]["command"].as_str().unwrap().contains("git commit"), "{told}");
    let (code, out, _) = woken(c, "1");
    assert_eq!(code, 124, "what the list names went to the parent though the child has a minder: {out}");
    let moved = rig.when_typed("worker", b"\x1b[B\x1b[B", asking("git commit -am wip", 2));
    let gone = rig.when_typed("worker", b"\r", working());
    lm.mind("approve --no --json");
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
