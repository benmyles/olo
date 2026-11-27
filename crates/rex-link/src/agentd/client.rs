//! The asking side of the agents service: what `olo agent …` runs on a
//! host, and what the Olo desktop runs there to follow it. It finds
//! the service's socket (starting the service where none answers),
//! sends one request and prints the answer as the command's own output
//! and status. No Rex client, no desktop: a socket of this account's.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use olo_rex_link::agents::wire::{PROTO, Paths, Request, Response, server_socket};
use serde_json::{Value, json};

const USAGE: &str = "usage: olo agent <start|run|list|status|read|watch|send|approve|mind|escalate|stop|prune|focus> [ARGS…]";

/// 69: what can't be done from here (no Rex server, no service); 75:
/// try again (the service ended while it answered).
const UNAVAILABLE: i32 = 69;
const AGAIN: i32 = 75;

fn uid() -> u32 {
    // SAFETY: no arguments, no failure.
    unsafe { libc::geteuid() }
}

pub fn run(mut args: Vec<String>) -> i32 {
    // (`--server PATH`: another Rex server than the account's own; Olo's tests.)
    let mut server: Option<PathBuf> = None;
    if args.first().is_some_and(|a| a == "--server") && args.len() >= 2 {
        server = Some(PathBuf::from(args[1].strip_prefix("unix://").unwrap_or(&args[1])));
        args.drain(..2);
    }
    let Some(sub) = args.first().cloned() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let rest = args.split_off(1);
    match sub.as_str() {
        // (What this build's service speaks: asked by `olo` before it relies on one.)
        "--protocol" | "protocol" => {
            println!("{PROTO}");
            return 0;
        }
        "-h" | "--help" | "help" => {
            println!(
                "{USAGE}\n\n{}\n\n{RUN_USAGE}\n\nolo agent watch … --follow: a watch that goes on: each thing as it comes, a line each (its message under it), until it is stopped.",
                olo_rex_link::agents::start_usage()
            );
            return 0;
        }
        // (The same asked by hand, as before there was a service.)
        "inspect" | "trust" => return crate::agent::run(std::iter::once(sub).chain(rest).collect()),
        _ => {}
    }
    let home = crate::procs::home();
    let Some(server) = server.or_else(|| server_socket(&home, uid())) else {
        eprintln!("olo agent: no Rex server is running for this account here");
        return UNAVAILABLE;
    };
    let paths = Paths::of(&server, &home, uid());
    match sub.as_str() {
        "serve" => {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("olo agents: {e}");
                    return 1;
                }
            };
            runtime.block_on(super::service::serve(server, paths))
        }
        "follow" => follow(&server, &paths),
        "request" => {
            // The desktop's: one request as JSON on the input, its answer as JSON.
            let mut text = String::new();
            let _ = std::io::stdin().read_to_string(&mut text);
            let Ok(mut req) = serde_json::from_str::<Request>(text.trim()) else {
                println!("{}", json!({ "ok": false, "stderr": "olo agent: a request that isn't one\n", "code": 2 }));
                return 2;
            };
            req.proto = PROTO;
            let answer = ask(&server, &paths, &req, retriable(&req.op));
            println!("{}", serde_json::to_string(&answer).unwrap_or_default());
            0
        }
        "adopt" => {
            let mut text = String::new();
            let _ = std::io::stdin().read_to_string(&mut text);
            let obj: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
            let req = Request { proto: PROTO, op: "adopt".into(), obj: Some(obj), ..Default::default() };
            print_answer(ask(&server, &paths, &req, true))
        }
        "ping" | "retire" => {
            // (Never starts one: says whether one is there.)
            let req = Request { proto: PROTO, op: sub.clone(), args: rest.clone(), ..Default::default() };
            match UnixStream::connect(&paths.socket).map(|stream| exchange(stream, &req)) {
                // (A service that is held says why it stays.)
                Ok(Asked::Answer(answer)) if !answer.ok => print_answer(answer),
                Ok(Asked::Answer(answer)) => {
                    println!("{}", answer.data);
                    0
                }
                _ => {
                    eprintln!("olo agent: no agents service is running");
                    1
                }
            }
        }
        _ => {
            let args = match with_files(&sub, rest) {
                Ok(args) => args,
                Err(e) => {
                    eprintln!("olo agent {sub}: {e}");
                    return e.1;
                }
            };
            let env = |name: &str| std::env::var(name).unwrap_or_default();
            let cwd = Some(env("PWD")).filter(|p| p.starts_with('/')).or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string())).unwrap_or_default();
            let req = Request { proto: PROTO, op: sub.clone(), args, obj: None, cwd, block: env("REX_BLOCK"), session: env("REX_SESSION"), inherit: inherited() };
            // Two commands are several askings, made here: one agent for
            // one prompt, start to finish; and a watch that goes on.
            let asked = |op: &str, args: Vec<String>| ask(&server, &paths, &Request { op: op.to_owned(), args, ..req.clone() }, retriable(op));
            match sub.as_str() {
                "run" => one_shot(req.args.clone(), &asked),
                "watch" | "wait" if req.args.iter().any(|a| a == "--follow") => follow_watch(req.args.clone(), &asked),
                _ => print_answer(ask(&server, &paths, &req, retriable(&sub))),
            }
        }
    }
}

pub const RUN_USAGE: &str = "usage: olo agent run <claude|codex|agy> --model MODEL --effort EFFORT (--prompt TEXT | --prompt-file FILE) [--name NAME] [--out FILE] [--keep-tab] [--timeout SECS] [--json] [what `olo agent start` takes: --cwd, --trust, --env, --minder … --rules …]\n\nOne agent for one prompt: starts it, waits for the prompt's own turn to end, prints its whole last message (and writes it to --out), and stops its tab (unless --keep-tab).\nStatus: 0 its turn ended; 1 it failed or exited before that; 3 it stands on a question or asks leave (its tab is kept: answer it, or look); 124 --timeout passed (its tab is kept).";

/// `olo agent run …`: one agent for one prompt, start to finish (the
/// ShiftUp lead's item 36, 2026-11-27: "One command for a one-shot
/// agent … starts it, returns when the prompt's own turn has ended (or
/// it exited or failed), prints its whole last message (and writes it
/// to `--out`), and stops the tab unless told to keep it; non-zero when
/// the turn did not end as a turn (error, exit, a question left
/// standing: said which)". Every reading of theirs is an agent started
/// for it: a start, a wait loop, a read and a stop of their own.)
///
/// Several askings of the service, made by this command: `start`, a
/// `watch` of that one child from the start's cursor until a rest that
/// answers the start (`answers`: the prompt's own turn's end and no
/// rest before it), then `stop`. From a parent's shell it is a
/// background command that ends when the reading is in.
fn one_shot(args: Vec<String>, asked: &dyn Fn(&str, Vec<String>) -> Response) -> i32 {
    let fail = |msg: &str, code: i32| {
        eprintln!("olo agent run: {msg}");
        code
    };
    // What is this command's own, taken out; the rest is `start`'s.
    let (mut out, mut keep, mut as_json, mut timeout) = (None::<String>, false, false, None::<u64>);
    let mut start = Vec::with_capacity(args.len() + 1);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--" => {
                start.push(arg);
                start.extend(it.by_ref());
            }
            "--out" => match it.next() {
                Some(path) => out = Some(path),
                None => return fail("--out requires a file path", 2),
            },
            "--timeout" => match it.next().and_then(|s| s.parse().ok()) {
                Some(secs) => timeout = Some(secs),
                None => return fail("--timeout requires a number of seconds", 2),
            },
            "--keep-tab" => keep = true,
            "--json" => as_json = true,
            "-h" | "--help" => {
                println!("{RUN_USAGE}");
                return 0;
            }
            a if a.starts_with("--out=") => out = Some(a["--out=".len()..].to_owned()),
            _ => start.push(arg),
        }
    }
    if !start.iter().any(|a| a == "--prompt" || a == "-p") {
        return fail(&format!("a prompt is what it runs: --prompt TEXT or --prompt-file FILE\n{RUN_USAGE}"), 2);
    }
    start.push("--json".to_owned());
    let started = asked("start", start);
    if !started.ok {
        return print_answer(started);
    }
    let id = started.data["id"].as_str().unwrap_or_default().to_owned();
    let name = started.data["name"].as_str().unwrap_or(&id).to_owned();
    let given = started.data["cursor"].as_u64().unwrap_or(0);
    let until = timeout.map(|secs| Instant::now() + Duration::from_secs(secs));

    // Its prompt's own turn's end, or what came in its place.
    let mut cursor = given;
    let (how, event) = loop {
        let wait = match until {
            Some(until) if Instant::now() >= until => break ("timeout", Value::Null),
            Some(until) => until.saturating_duration_since(Instant::now()).as_secs().clamp(1, 90),
            None => 90,
        };
        let told = asked(
            "watch",
            vec![id.clone(), "--since".into(), cursor.to_string(), "--on".into(), "question,approval,idle,done,error,exit".into(), "--timeout".into(), wait.to_string(), "--json".into()],
        );
        if let Some(c) = told.data["cursor"].as_u64() {
            cursor = c;
        }
        if !told.ok {
            // (Its time up with nothing to say: asked again. Anything else is said.)
            if told.code == 124 {
                continue;
            }
            return print_answer(told);
        }
        let ev = told.data;
        match ev["state"].as_str().unwrap_or_default() {
            "idle" | "done" if ev["answers"].as_u64() == Some(given) => break ("done", ev),
            // (At rest without having taken its prompt up: said as that.)
            "idle" | "done" => break ("unanswered", ev),
            "waiting_question" => break ("question", ev),
            "waiting_approval" => break ("approval", ev),
            "error" => break ("error", ev),
            "exited" => break ("exit", ev),
            _ => {}
        }
    };

    // Its whole last message (a rest's own event has it; else as it stands now).
    let status = asked("status", vec![id.clone(), "--json".into()]).data;
    let last = event["last_message"].as_str().or(status["last_message"].as_str()).map(str::to_owned);
    if let (Some(path), Some(text)) = (&out, &last) {
        let dest = Path::new(path);
        let tmp = dest.with_extension(format!("olo-tmp-{}", std::process::id()));
        let written = dest.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, text)).and_then(|()| std::fs::rename(&tmp, dest));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            // (Its message is still printed, and its tab kept: nothing is lost with the file.)
            eprintln!("olo agent run: couldn't write {path}: {e}");
            keep = true;
        }
    }
    // Stopped when its turn is over or it can do no more; kept where
    // somebody is to answer it or look.
    let stop = !keep && matches!(how, "done" | "error" | "unanswered");
    let stopped = stop && asked("stop", vec![id.clone(), "--force".into(), "--json".into()]).ok;
    let (code, said) = match how {
        "done" => (0, String::new()),
        "unanswered" => (1, format!("`{name}` came to rest without taking its prompt up")),
        "question" => (3, format!("`{name}` stands on a question (its tab is kept: `olo agent send {name} …`, or look): {}", event["summary"].as_str().unwrap_or_default())),
        "approval" => {
            (3, format!("`{name}` asks leave (its tab is kept: `olo agent approve {name} --once|--no`): {}", event["approval"]["command"].as_str().or(event["summary"].as_str()).unwrap_or_default()))
        }
        "error" => (1, format!("`{name}` failed: {}", event["error"].as_str().or(event["summary"].as_str()).unwrap_or_default())),
        "exit" => (1, format!("`{name}` exited before its turn ended ({})", event["summary"].as_str().unwrap_or("exited"))),
        _ => (124, format!("`{name}` had not ended its turn after {} s (its tab is kept)", timeout.unwrap_or(0))),
    };
    if as_json {
        let payload = json!({
            "ok": code == 0, "ended_as": how, "id": id, "name": name, "last_message": last, "answers": event["answers"],
            "cli_session_id": status["cli_session_id"], "transcript_path": status["transcript_path"], "out": out.filter(|_| last.is_some()),
            "stopped": stopped, "cursor": cursor, "minder": started.data["minder"],
        });
        println!("{payload}");
    } else if let Some(text) = &last {
        println!("{}", text.trim_end());
    }
    let _ = std::io::stdout().flush();
    if code != 0 {
        eprintln!("olo agent run: {said}");
    }
    code
}

/// `olo agent watch … --follow`: a watch that goes on. Each thing
/// waited for is printed as it comes and the watch goes on from it,
/// until it is stopped (the ShiftUp lead's item 38, 2026-11-27: "a lead
/// must keep one wait running over all its children and start it again
/// each time it returns, and a child nobody waits on waits unseen").
/// Run by a harness that gives each line of a running command to its
/// agent as it is printed (Claude Code's Monitor), it is a word to the
/// parent with no wait of its own to keep: an escalation, a turn's end
/// with its message whole, a failure, an end, each once. Without
/// `--since` it begins from now (nothing that was is said again).
fn follow_watch(args: Vec<String>, asked: &dyn Fn(&str, Vec<String>) -> Response) -> i32 {
    let (mut rest, mut since, mut as_json) = (Vec::with_capacity(args.len()), None::<u64>, false);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--follow" => {}
            "--json" => as_json = true,
            // (Each asking's own: this command's time is until it is stopped.)
            "--timeout" => {
                it.next();
            }
            "--since" => since = it.next().and_then(|s| s.parse().ok()),
            a if a.starts_with("--since=") => since = a["--since=".len()..].parse().ok(),
            a if a.starts_with("--timeout=") => {}
            _ => rest.push(arg),
        }
    }
    let mut cursor = match since {
        Some(cursor) => cursor,
        None => {
            let listed = asked("list", vec!["--fast".into(), "--json".into()]);
            match listed.data["cursor"].as_u64() {
                Some(cursor) => cursor,
                None => return print_answer(listed),
            }
        }
    };
    loop {
        let mut args = rest.clone();
        args.extend(["--since".to_owned(), cursor.to_string(), "--timeout".to_owned(), "90".to_owned(), "--json".to_owned()]);
        let told = asked("watch", args);
        if let Some(c) = told.data["cursor"].as_u64() {
            cursor = c;
        }
        if !told.ok {
            if told.code == 124 {
                continue;
            }
            return print_answer(told);
        }
        let ev = &told.data;
        if as_json {
            println!("{ev}");
        } else {
            // (One line that says who and what, then its message whole:
            // lines printed together reach an agent together.)
            let who = ev["name"].as_str().or(ev["id"].as_str()).unwrap_or_default();
            let what = ev["event"].as_str().unwrap_or_default();
            let head = match what {
                "reset" => "the cursor was past the log's end: watching from now".to_owned(),
                "escalation" => format!("{who}: escalation"),
                "waiting_approval" => format!("{who}: asks leave: {}", ev["approval"]["command"].as_str().or(ev["summary"].as_str()).unwrap_or_default()),
                "waiting_question" => format!("{who}: asks: {}", ev["summary"].as_str().unwrap_or_default()),
                "exited" => format!("{who}: {}", ev["summary"].as_str().unwrap_or("exited")),
                "error" => format!("{who}: failed: {}", ev["error"].as_str().or(ev["summary"].as_str()).unwrap_or_default()),
                other => format!("{who}: {other}"),
            };
            let body = match what {
                "escalation" => ev["summary"].as_str(),
                "idle" | "done" => ev["last_message"].as_str(),
                _ => None,
            };
            match body.map(str::trim_end).filter(|b| !b.is_empty()) {
                Some(body) => println!("{head} (cursor {cursor})\n{body}"),
                None => println!("{head} (cursor {cursor})"),
            }
        }
        let _ = std::io::stdout().flush();
    }
}

/// What a child with the desktop's leave (`--desktop inherit`) takes of
/// this shell's environment: what the desktop gave the pane it is asked
/// from.
fn inherited() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in ["SSH_AUTH_SOCK", "BROWSER"] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            out.push((name.to_owned(), value));
        }
    }
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        if dir.ends_with("/rex-commands") || dir.ends_with("/olo/rex/bin") {
            out.push(("PATH_PREPEND".to_owned(), dir.to_owned()));
        }
    }
    out
}

struct Refused(String, i32);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The files a command names are this side's to read: `start
/// --prompt-file PATH` is `--prompt` with what the file says, `send
/// --file PATH` is its text sent whole as a paste, and a minder's rules
/// (`start … --rules PATH`, `mind … --rules PATH`) are `--rules-text`
/// with what the file says. Nothing after `--` is touched.
fn with_files(sub: &str, args: Vec<String>) -> Result<Vec<String>, Refused> {
    // (The flag, what it is in a refusal, and what it becomes: the text
    // after these, and these after the text.)
    type Named = (&'static str, &'static str, &'static [&'static str], &'static [&'static str]);
    const RULES: [Named; 3] = [("--rules", "rules", &["--rules-text"], &[]), ("--rules-file", "rules", &["--rules-text"], &[]), ("--approve-all-but", "list", &["--approve-all-but-text"], &[])];
    let named: Vec<Named> = match sub {
        "start" | "spawn" | "run" => [("--prompt-file", "prompt", &["--prompt"][..], &[][..])].into_iter().chain(RULES).collect(),
        "mind" | "minder" => RULES.to_vec(),
        "send" | "input" => vec![("--file", "send", &["--text"], &["--paste"])],
        _ => return Ok(args),
    };
    let mut out = Vec::with_capacity(args.len() + 2);
    let mut it = args.into_iter();
    'args: while let Some(arg) = it.next() {
        if arg == "--" {
            out.push(arg);
            out.extend(it);
            break;
        }
        for (flag, what, before, after) in &named {
            let path = if arg == *flag {
                Some(it.next().ok_or_else(|| Refused(format!("{flag} requires a file path"), 2))?)
            } else {
                arg.strip_prefix(flag).and_then(|rest| rest.strip_prefix('=')).map(str::to_owned)
            };
            let Some(path) = path else { continue };
            let body = std::fs::read(&path).map_err(|e| Refused(format!("couldn't read {what} file {path}: {e}"), 1))?;
            out.extend(before.iter().map(|s| (*s).to_owned()));
            out.push(String::from_utf8_lossy(&body).into_owned());
            out.extend(after.iter().map(|s| (*s).to_owned()));
            continue 'args;
        }
        out.push(arg);
    }
    Ok(out)
}

/// Whether asking twice does nothing twice (so it may be asked again of
/// a service started in place of one that ended).
fn retriable(op: &str) -> bool {
    matches!(op, "list" | "ls" | "status" | "read" | "tail" | "watch" | "wait" | "focus" | "prune")
}

fn print_answer(answer: Response) -> i32 {
    let _ = std::io::stdout().write_all(answer.stdout.as_bytes());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(answer.stderr.as_bytes());
    if answer.ok {
        0
    } else if answer.code != 0 {
        answer.code
    } else {
        1
    }
}

/// How asking went.
enum Asked {
    Answer(Response),
    /// The connection ended before the service took the request:
    /// nothing was done.
    NotTaken,
    /// It ended after: what was done isn't known.
    Lost,
}

/// One request sent, and what came of it: the service's mark that it
/// took it (an empty line), then its answer.
fn exchange(mut stream: UnixStream, req: &Request) -> Asked {
    let Ok(mut line) = serde_json::to_string(req) else { return Asked::NotTaken };
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
        return Asked::NotTaken;
    }
    let mut lines = BufReader::new(stream);
    let mut taken = String::new();
    if lines.read_line(&mut taken).unwrap_or(0) == 0 {
        return Asked::NotTaken;
    }
    let mut answer = String::new();
    // (A service from before the mark answers at once.)
    let said = if taken.trim().is_empty() { lines.read_line(&mut answer).map(|_| answer.as_str()).unwrap_or_default() } else { taken.as_str() };
    match serde_json::from_str(said.trim()) {
        Ok(answer) => Asked::Answer(answer),
        Err(_) => Asked::Lost,
    }
}

/// The service's socket, the service started where none answers.
fn connected(server: &Path, paths: &Paths) -> Result<UnixStream, Box<Response>> {
    if let Ok(stream) = UnixStream::connect(&paths.socket) {
        return Ok(stream);
    }
    let unavailable = |why: String| Box::new(Response { ok: false, stdout: String::new(), stderr: format!("olo agent: {why}\n"), code: UNAVAILABLE, data: Value::Null });
    let last_word = || std::fs::read_to_string(&paths.log).ok().and_then(|log| log.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim_start_matches("olo agents: ").to_owned()));
    let mut service = start_service(server, paths).map_err(|e| unavailable(format!("the agents service couldn't be started: {e}")))?;
    // (It listens once it has reached the Rex server and read what the
    // last one wrote: waited for here. One that ends without listening
    // says why in its log, unless another got there first and listens.)
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.socket) {
            return Ok(stream);
        }
        if let Ok(Some(status)) = service.try_wait()
            && !status.success()
        {
            return Err(unavailable(last_word().unwrap_or_else(|| "the agents service couldn't start".to_owned())));
        }
        if Instant::now() >= until {
            return Err(unavailable(format!("the agents service isn't answering: {}", last_word().unwrap_or_else(|| "it didn't begin to listen".to_owned()))));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Starts this build's service, apart from this command (a session and
/// a process group of its own: it outlives the pane it was started
/// from), saying what it has to say in its log.
fn start_service(server: &Path, paths: &Paths) -> std::io::Result<std::process::Child> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    use std::os::unix::process::CommandExt as _;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&paths.dir)?;
    // (Its log begun again where it has grown: the last service's words are enough.)
    let grown = std::fs::metadata(&paths.log).is_ok_and(|m| m.len() > 1 << 20);
    let log = std::fs::OpenOptions::new().create(true).append(!grown).write(true).truncate(grown).mode(0o600).open(&paths.log)?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.args(["agent", "--server"]).arg(server).arg("serve").stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log).current_dir("/");
    // The same service whoever starts it (a pane's shell, an agent in a
    // child tab, the desktop over SSH): of the asking shell's
    // environment only the account's own places. Nothing an agent set
    // for itself or its children reaches it (where an agent's files are
    // is read from that agent's own process, `agent::places_for_pid`).
    command.env_clear();
    for name in [
        "HOME",
        "USER",
        "LOGNAME",
        "PATH",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "REX_SERVER_SOCKET",
        "OLO_REX_AGENTS_DIR",
        "OLO_REX_AGENTS_MINDER_REPORTS_MS",
        "OLO_REX_AGENTS_TAKEN_UP_MS",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    // SAFETY: between fork and exec, one call that is safe there.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    // (Its own from here: this command looks at it only until it
    // listens, and its end is the system's to take up after.)
    command.spawn()
}

/// Asks the service, starting one where none is. A service of an
/// earlier build hands over to this one's (it ends, having finished
/// what it was doing and written what it knows; this build's is started
/// and asked). A request the service never took is asked of the next
/// service whatever it is; one it took and didn't answer (it was killed)
/// is asked again only where asking twice does nothing twice.
fn ask(server: &Path, paths: &Paths, req: &Request, again: bool) -> Response {
    let lost = || Response {
        ok: false,
        stdout: String::new(),
        stderr: format!("olo agent {}: the agents service ended while it did this; look (`olo agent list`) before asking again\n", req.op),
        code: AGAIN,
        data: Value::Null,
    };
    for _ in 0..6 {
        let stream = match connected(server, paths) {
            Ok(stream) => stream,
            Err(unavailable) => return *unavailable,
        };
        match exchange(stream, req) {
            // (Its handing over: gone when its socket is.)
            Asked::Answer(answer) if !answer.ok && answer.data["handover"] == true => {
                let until = Instant::now() + Duration::from_secs(25);
                while paths.socket.exists() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Asked::Answer(answer) => return answer,
            Asked::NotTaken => std::thread::sleep(Duration::from_millis(30)),
            Asked::Lost if again => std::thread::sleep(Duration::from_millis(30)),
            Asked::Lost => return lost(),
        }
    }
    lost()
}

/// `agent follow`: everything the service knows, then each change, a
/// line each, for as long as this command's input stays open (the
/// desktop closes it to end the following). Its status says why it
/// ended: 0 asked to, 75 the service went away (follow again).
fn follow(server: &Path, paths: &Paths) -> i32 {
    let req = Request { proto: PROTO, op: "follow".into(), ..Default::default() };
    let Ok(mut line) = serde_json::to_string(&req) else { return 1 };
    line.push('\n');
    for _ in 0..4 {
        let stream = match connected(server, paths) {
            Ok(stream) => stream,
            Err(e) => {
                let _ = std::io::stderr().write_all(e.stderr.as_bytes());
                return e.code;
            }
        };
        let Ok(mut to_service) = stream.try_clone() else { return 1 };
        if to_service.write_all(line.as_bytes()).is_err() {
            continue;
        }
        let mut frames = BufReader::new(stream).lines();
        // (A service of an earlier build answers by handing over: an
        // empty line and its word, then its end. This build's follows.)
        let first = match frames.next() {
            Some(Ok(first)) if first.trim().is_empty() => {
                let until = Instant::now() + Duration::from_secs(25);
                while paths.socket.exists() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                continue;
            }
            Some(Ok(first)) => first,
            _ => continue,
        };
        // The input closing ends it: the service is told by its side closing.
        // (And this side is told it was asked to, before the closing:
        // the closing ends the reading below at once, and whichever of
        // the two threads came first said why the command ended. With
        // the computer busy the reading did, and a following that was
        // asked to end said "the service went away, follow again": 75
        // for 0, one run in a few of the tests under load, 2026-11-26.)
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let told = asked.clone();
        std::thread::spawn(move || {
            let mut sink = [0u8; 256];
            let mut stdin = std::io::stdin().lock();
            while stdin.read(&mut sink).is_ok_and(|n| n > 0) {}
            told.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = to_service.shutdown(std::net::Shutdown::Both);
            std::process::exit(0);
        });
        let mut stdout = std::io::stdout().lock();
        for frame in std::iter::once(Ok(first)).chain(frames) {
            let Ok(frame) = frame else { break };
            if writeln!(stdout, "{frame}").and_then(|()| stdout.flush()).is_err() {
                return 0;
            }
        }
        return if asked.load(std::sync::atomic::Ordering::SeqCst) { 0 } else { AGAIN };
    }
    AGAIN
}
