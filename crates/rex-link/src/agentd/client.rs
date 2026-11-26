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

const USAGE: &str = "usage: olo agent <start|list|status|read|watch|send|approve|stop|prune|focus> [ARGS…]";

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
            println!("{USAGE}\n\n{}", olo_rex_link::agents::start_usage());
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
            let req = Request { proto: PROTO, op: sub.clone(), ..Default::default() };
            match UnixStream::connect(&paths.socket).map(|stream| exchange(stream, &req)) {
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
            print_answer(ask(&server, &paths, &req, retriable(&sub)))
        }
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
/// --file PATH` is its text sent whole as a paste. Nothing after `--` is
/// touched.
fn with_files(sub: &str, args: Vec<String>) -> Result<Vec<String>, Refused> {
    let (flag, what) = match sub {
        "start" | "spawn" => ("--prompt-file", "prompt"),
        "send" | "input" => ("--file", "send"),
        _ => return Ok(args),
    };
    let mut out = Vec::with_capacity(args.len() + 2);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            out.push(arg);
            out.extend(it);
            break;
        }
        let path = if arg == flag {
            Some(it.next().ok_or_else(|| Refused(format!("{flag} requires a file path"), 2))?)
        } else {
            arg.strip_prefix(flag).and_then(|rest| rest.strip_prefix('=')).map(str::to_owned)
        };
        let Some(path) = path else {
            out.push(arg);
            continue;
        };
        let body = std::fs::read(&path).map_err(|e| Refused(format!("couldn't read {what} file {path}: {e}"), 1))?;
        let body = String::from_utf8_lossy(&body).into_owned();
        if sub == "start" || sub == "spawn" {
            out.extend(["--prompt".to_owned(), body]);
        } else {
            out.extend(["--text".to_owned(), body, "--paste".to_owned()]);
        }
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
    for name in ["HOME", "USER", "LOGNAME", "PATH", "LANG", "LC_ALL", "TMPDIR", "XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "REX_SERVER_SOCKET", "OLO_REX_AGENTS_DIR"] {
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
        std::thread::spawn(move || {
            let mut sink = [0u8; 256];
            let mut stdin = std::io::stdin().lock();
            while stdin.read(&mut sink).is_ok_and(|n| n > 0) {}
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
        return AGAIN;
    }
    AGAIN
}
