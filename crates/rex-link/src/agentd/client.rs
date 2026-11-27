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
            // (Before any thread of the runtime's is there: what tells it
            // to end is read with who sent it, `service::end_signals`.)
            let signals = super::service::end_signals();
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("olo agents: {e}");
                    return 1;
                }
            };
            runtime.block_on(super::service::serve(server, paths, signals))
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
            let env = |name: &str| std::env::var(name).unwrap_or_default();
            let req = Request { proto: PROTO, op: "adopt".into(), obj: Some(obj), block: env("REX_BLOCK"), session: env("REX_SESSION"), ..Default::default() };
            print_answer(ask(&server, &paths, &req, true))
        }
        "ping" | "retire" => {
            // (Never starts one: says whether one is there.)
            let env = |name: &str| std::env::var(name).unwrap_or_default();
            let req = Request { proto: PROTO, op: sub.clone(), args: rest.clone(), block: env("REX_BLOCK"), session: env("REX_SESSION"), ..Default::default() };
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
        "mind-send" => {
            // Restricted send entry point invoked by a minder's `mind` script:
            // never expands `--file`, never runs `--wait` / `--out`, and allows
            // only plain text, `--enter`, `--paste`, `--json`, and chooser nav keys.
            if let Err((msg, code)) = validate_mind_send_args(&rest) {
                eprintln!("olo agent send: {msg}");
                return code;
            }
            let env = |name: &str| std::env::var(name).unwrap_or_default();
            let cwd = Some(env("PWD")).filter(|p| p.starts_with('/')).or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string())).unwrap_or_default();
            let req = Request { proto: PROTO, op: "send".into(), args: rest, obj: None, cwd, block: env("REX_BLOCK"), session: env("REX_SESSION"), inherit: inherited(), wait: false };
            print_answer(ask(&server, &paths, &req, false))
        }
        _ => {
            // (`send --all-errored` takes a message and nothing made into
            // one on the way: no file is opened for it, a FIFO least of all.)
            let bulk = sub == "send" && rest.first().is_some_and(|a| a == "--all-errored");
            let args = match if bulk { Ok(rest) } else { with_files(&sub, rest) } {
                Ok(args) => args,
                Err(e) => {
                    eprintln!("olo agent {sub}: {e}");
                    return e.1;
                }
            };
            let env = |name: &str| std::env::var(name).unwrap_or_default();
            let cwd = Some(env("PWD")).filter(|p| p.starts_with('/')).or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string())).unwrap_or_default();
            let req = Request { proto: PROTO, op: sub.clone(), args, obj: None, cwd, block: env("REX_BLOCK"), session: env("REX_SESSION"), inherit: inherited(), wait: false };
            // Two commands are several askings, made here: one agent for
            // one prompt, start to finish; and a watch that goes on.
            let asked = |op: &str, args: Vec<String>| ask(&server, &paths, &Request { op: op.to_owned(), args, ..req.clone() }, retriable(op));
            let asked_send_wait = |args: Vec<String>| ask(&server, &paths, &Request { op: "send".to_owned(), args, wait: true, ..req.clone() }, false);
            let has_wait_flag = req.args.iter().take_while(|a| a.as_str() != "--").any(|a| a == "--wait");
            match sub.as_str() {
                "run" => one_shot(req.args.clone(), &asked),
                "send" if req.args.first().is_some_and(|a| a == "--all-errored") => send_to_all_errored(req.args[1..].to_vec(), &asked),
                "send" if has_wait_flag => send_and_wait(req.args.clone(), &asked_send_wait, &asked),
                "watch" | "wait" if req.args.iter().any(|a| a == "--follow") => follow_watch(req.args.clone(), &asked),
                _ => print_answer(ask(&server, &paths, &req, retriable(&sub))),
            }
        }
    }
}

/// `olo agent send --all-errored TEXT… [--enter] [--json]`: the same
/// message to every child of the asking tab whose turn ended in an error
/// and that still runs, each as `send NAME …` would send it (the ShiftUp
/// lead's item 49, 2026-11-27, after an outage ended every child's turn
/// at once: "one command that says 'continue' to every child of mine
/// that ended that way"). Says which it sent to and how each went; 0
/// where all went (or none had failed), 1 where any didn't.
fn send_to_all_errored(rest: Vec<String>, asked: &dyn Fn(&str, Vec<String>) -> Response) -> i32 {
    // What to send, and how: a message and `--enter`, `--paste`,
    // `--json`, nothing else (not `--ctrl=c`, not a file made into
    // text on the way here); after `--` everything is the message.
    let (mut text, mut flags, mut as_json, mut literal) = (Vec::new(), Vec::new(), false, false);
    for word in &rest {
        match word.as_str() {
            _ if literal => text.push(word.clone()),
            "--" => literal = true,
            "--json" => as_json = true,
            "--enter" | "--paste" => flags.push(word.clone()),
            other if other.starts_with('-') => {
                eprintln!("olo agent send --all-errored: only a message, with --enter, --paste, --json (not `{other}`): each child is watched as any other");
                return 2;
            }
            _ => text.push(word.clone()),
        }
    }
    let text = text.join(" ");
    // (As it would be typed: a message of control bytes alone is none,
    // and its `--enter` would be a bare Return to every child.)
    if olo_rex_link::agents::spec::sanitize_paste_text(&text).trim().is_empty() {
        eprintln!("usage: olo agent send --all-errored TEXT… [--enter] [--paste] [--json]: what to send is missing");
        return 2;
    }
    // (Submitted, each is seen to be taken before it is said to be.)
    let submits = flags.iter().any(|f| f == "--enter");
    if submits {
        flags.push("--verify".into());
    }
    // From a tab's own pane: whose children they are is that tab's to
    // say, never the tab that happens to be in front.
    if std::env::var("REX_BLOCK").map_or(true, |b| b.trim().is_empty()) {
        eprintln!("olo agent send --all-errored: only from a tab's own pane (REX_BLOCK): whose children they are must be certain");
        return 2;
    }
    let listed = asked("list", vec!["--json".into()]);
    if !listed.ok {
        return print_answer(listed);
    }
    // (And the pane must be one the service knows: for one it doesn't, a
    // pane closed since, it answers for the tab in front, which may be
    // another lead's. The confirming review.)
    if listed.data["parent_certain"] != true {
        eprintln!("olo agent send --all-errored: the agents service doesn't know this pane (REX_BLOCK), so whose children these are isn't certain: nothing sent");
        return 2;
    }
    // The asking tab's own children: its direct ones, and those minded
    // for it under a minder of its own; not a child's children (theirs
    // to look after), and no minder.
    let rows: Vec<&Value> = listed.data["subagents"].as_array().into_iter().flatten().collect();
    let depth = |c: &Value| c["depth"].as_u64().unwrap_or(0);
    let is_minder = |c: &Value| c["shared_minder"] == true || !c["minds"].is_null();
    let my_minders: Vec<&str> = rows.iter().filter(|c| depth(c) == 1 && is_minder(c)).filter_map(|c| c["id"].as_str()).collect();
    let mine = |c: &&&Value| !is_minder(c) && (depth(c) == 1 || (depth(c) == 2 && c["minded_by"].as_str().is_some_and(|m| my_minders.contains(&m))));
    let failed: Vec<(String, String)> =
        rows.iter().filter(mine).filter(|c| c["state"] == "error").filter_map(|c| Some((c["id"].as_str()?.to_owned(), c["name"].as_str().unwrap_or_default().to_owned()))).collect();
    let mut sent = Vec::new();
    let mut all_went = true;
    for (id, name) in failed {
        // (As it stands now: one that has gone on meanwhile, by itself or
        // by somebody's hand, is not told to go on in the middle of it.)
        let now = asked("status", vec![id.clone(), "--json".into()]);
        // (A check that failed is not a child that recovered.)
        if !now.ok {
            all_went = false;
            sent.push(json!({ "id": id, "name": name, "ok": false, "error": format!("its state couldn't be read ({}): not sent", now.stderr.trim()) }));
            continue;
        }
        if now.data["state"] != "error" {
            sent.push(json!({ "id": id, "name": name, "ok": true, "skipped": "no longer at an error", "state": now.data["state"] }));
            continue;
        }
        let was = now.data["error"].as_str().or(now.data["summary"].as_str()).map(str::to_owned);
        let mut args = vec![id.clone()];
        args.extend(flags.iter().cloned());
        args.extend(["--json".to_owned(), "--".to_owned(), text.clone()]);
        let answer = asked("send", args);
        // (Sent, and taken: text left in its field unsubmitted is not, and
        // an answer that doesn't say is not a yes.)
        let went = answer.ok && if submits { answer.data["delivered"] == true } else { answer.data["delivered"] != false };
        all_went &= went;
        sent.push(json!({
            "id": id, "name": name, "was": was, "error_kind": now.data["error_kind"], "error_status": now.data["error_status"],
            "ok": went, "delivered": answer.data["delivered"], "submitted": submits, "cursor": answer.data["cursor"],
            "error": (!went).then(|| if answer.ok { "sent, and not seen to be taken: look at it".to_owned() } else { answer.stderr.trim().to_owned() }),
        }));
    }
    let told = sent.iter().filter(|one| one["skipped"].is_null() && one["ok"] == true).count();
    if as_json {
        println!("{}", json!({ "ok": all_went, "count": told, "sent": sent }));
    } else if sent.is_empty() {
        println!("no child of this tab stands at an error");
    } else {
        for one in &sent {
            let who = one["name"].as_str().filter(|n| !n.is_empty()).or(one["id"].as_str()).unwrap_or_default();
            match (one["skipped"].as_str(), one["error"].as_str()) {
                (Some(why), _) => println!("{who}: {why}"),
                (None, Some(why)) => println!("{who}: not sent ({why})"),
                (None, None) if one["submitted"] == false => println!("{who}: typed into its field, not submitted (no --enter)"),
                (None, None) => println!("{who}: sent"),
            }
        }
    }
    let _ = std::io::stdout().flush();
    i32::from(!all_went)
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
/// Waits for the end of the turn that answers what a child was given
/// at `given` (a start's cursor, a send's), or for what comes in its
/// place: how it ended ("done", "unanswered", "question", "approval",
/// "error", "exit", "timeout"), the event that said so, and the cursor
/// to watch on from. `its_own`: the turn is one the giving began (the
/// child was at rest), so its end is the rest whose `answers` is
/// `given`, and a rest that answers anything else is the child never
/// having taken it up; otherwise (a message sent into a turn already
/// going) it is that turn's, and the next rest is its end.
fn turn_end(id: &str, given: u64, its_own: bool, until: Option<Instant>, asked: &dyn Fn(&str, Vec<String>) -> Response) -> Result<(&'static str, Value, u64), Box<Response>> {
    let mut cursor = given;
    loop {
        let wait = match until {
            Some(until) if Instant::now() >= until => return Ok(("timeout", Value::Null, cursor)),
            Some(until) => until.saturating_duration_since(Instant::now()).as_secs().clamp(1, 90),
            None => 90,
        };
        let told = asked(
            "watch",
            vec![id.to_owned(), "--since".into(), cursor.to_string(), "--on".into(), "question,approval,idle,done,error,exit".into(), "--timeout".into(), wait.to_string(), "--json".into()],
        );
        if let Some(c) = told.data["cursor"].as_u64() {
            cursor = c;
        }
        if !told.ok {
            // (Its time up with nothing to say: asked again. Anything else is said.)
            if told.code == 124 {
                continue;
            }
            return Err(Box::new(told));
        }
        let ev = told.data;
        match ev["state"].as_str().unwrap_or_default() {
            "idle" | "done" if !its_own || ev["answers"].as_u64() == Some(given) => return Ok(("done", ev, cursor)),
            // (At rest without having taken up what it was given: said as that.)
            "idle" | "done" => return Ok(("unanswered", ev, cursor)),
            "waiting_question" => return Ok(("question", ev, cursor)),
            "waiting_approval" => return Ok(("approval", ev, cursor)),
            "error" => return Ok(("error", ev, cursor)),
            "exited" => return Ok(("exit", ev, cursor)),
            _ => {}
        }
    }
}

/// `olo agent send NAME "…" --enter --wait`: a message sent to a child
/// that stands, and its answer waited for (the ShiftUp lead's item 43,
/// 2026-11-27: "returns when the turn that answers that message has
/// ended and prints its whole last message … non-zero as `run` is when
/// it stands on a question or the time passes. Then asking a standing
/// agent is one line that prints its answer"). `send`, then a `watch`
/// of that child from the send's cursor until the rest that ends the
/// message's turn; the child is left as it is. Its own options, beside
/// `send`'s: `--wait`, `--timeout SECS`, `--out FILE`, `--json`.
fn send_and_wait(args: Vec<String>, send_wait: &dyn Fn(Vec<String>) -> Response, asked: &dyn Fn(&str, Vec<String>) -> Response) -> i32 {
    let fail = |msg: &str, code: i32| {
        eprintln!("olo agent send: {msg}");
        code
    };
    let (mut out, mut as_json, mut timeout) = (None::<String>, false, None::<u64>);
    let mut send = Vec::with_capacity(args.len() + 1);
    let mut after_sep = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--" => {
                after_sep.push(arg);
                after_sep.extend(it.by_ref());
            }
            "--wait" => {}
            "--json" => as_json = true,
            "--out" => match it.next() {
                Some(path) => out = Some(path),
                None => return fail("--out requires a file path", 2),
            },
            "--timeout" => match it.next().and_then(|s| s.trim_end_matches('s').parse().ok()) {
                Some(secs) => timeout = Some(secs),
                None => return fail("--timeout requires a number of seconds", 2),
            },
            a if a.starts_with("--out=") => out = Some(a["--out=".len()..].to_owned()),
            _ => send.push(arg),
        }
    }
    // (A message that isn't submitted begins no turn to wait for.)
    if !send.iter().any(|a| a == "--enter") {
        return fail("--wait waits for the turn the message begins: send it with --enter", 2);
    }
    // Signal `--wait` via optional `Request::wait` rather than `req.args` so
    // earlier `PROTO == 1` services (which ignore unknown JSON fields) stay
    // wire-compatible while current services still refuse `--wait` from minders.
    send.push("--json".to_owned());
    send.extend(after_sep);
    let sent = send_wait(send);
    if !sent.ok {
        return print_answer(sent);
    }
    let id = sent.data["id"].as_str().unwrap_or_default().to_owned();
    let name = sent.data["name"].as_str().unwrap_or(&id).to_owned();
    let given = sent.data["cursor"].as_u64().unwrap_or(0);
    // (A service from before it said so: taken as a turn of its own.)
    let its_own = sent.data["began_turn"].as_bool().unwrap_or(true);
    let until = timeout.map(|secs| Instant::now() + Duration::from_secs(secs));
    let (how, event, cursor) = match turn_end(&id, given, its_own, until, asked) {
        Ok(ended) => ended,
        Err(failed) => return print_answer(*failed),
    };
    let status = asked("status", vec![id.clone(), "--json".into()]).data;
    let last = event["last_message"].as_str().or(status["last_message"].as_str()).map(str::to_owned);
    let how = if how == "done" && last.as_deref().is_none_or(|m| m.trim().is_empty()) { "silent" } else { how };
    if let (Some(path), Some(text), "done") = (&out, &last, how) {
        let dest = Path::new(path);
        let tmp = dest.with_extension(format!("olo-tmp-{}", std::process::id()));
        let written = dest.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, text)).and_then(|()| std::fs::rename(&tmp, dest));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("olo agent send: couldn't write {path}: {e}");
        }
    }
    let (code, said) = match how {
        "done" => (0, String::new()),
        "unanswered" => (1, format!("`{name}` came to rest without taking the message up")),
        "silent" => (1, format!("`{name}` came to rest with no message of its own to give")),
        "question" => (3, format!("`{name}` stands on a question (`olo agent send {name} …`, or look): {}", event["summary"].as_str().unwrap_or_default())),
        "approval" => (3, format!("`{name}` asks leave (`olo agent approve {name} --once|--no`): {}", event["approval"]["command"].as_str().or(event["summary"].as_str()).unwrap_or_default())),
        "error" => (1, format!("`{name}` failed: {}", event["error"].as_str().or(event["summary"].as_str()).unwrap_or_default())),
        "exit" => (1, format!("`{name}` exited before its turn ended ({})", event["summary"].as_str().unwrap_or("exited"))),
        _ => (124, format!("`{name}` had not ended its turn after {} s", timeout.unwrap_or(0))),
    };
    if as_json {
        let payload = json!({
            "ok": code == 0, "ended_as": how, "id": id, "name": name, "last_message": last, "answers": event["answers"], "delivered": sent.data["delivered"],
            "cli_session_id": status["cli_session_id"], "transcript_path": status["transcript_path"], "out": out.filter(|_| how == "done"), "cursor": cursor,
        });
        println!("{payload}");
    } else if let (Some(text), "done") = (&last, how) {
        println!("{}", text.trim_end());
    }
    let _ = std::io::stdout().flush();
    if code != 0 {
        eprintln!("olo agent send: {said}");
    }
    code
}

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
    let (how, event, cursor) = match turn_end(&id, given, true, until, asked) {
        Ok(ended) => ended,
        Err(failed) => return print_answer(*failed),
    };

    // Its whole last message (a rest's own event has it; else as it stands now).
    let status = asked("status", vec![id.clone(), "--json".into()]).data;
    let last = event["last_message"].as_str().or(status["last_message"].as_str()).map(str::to_owned);
    // A turn's end with no message of the agent's is not its prompt's
    // own turn's end, whatever it answers: said, and never a success.
    // (An agent that had done nothing was once reported done, exit 0,
    // `last_message` null: "we nearly read 'no findings' from it", the
    // ShiftUp lead, 2026-11-27, item 46.)
    let how = if how == "done" && last.as_deref().is_none_or(|m| m.trim().is_empty()) { "silent" } else { how };
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
    let stop = !keep && matches!(how, "done" | "error" | "unanswered" | "silent");
    let stopped = stop && asked("stop", vec![id.clone(), "--force".into(), "--json".into()]).ok;
    let (code, said) = match how {
        "done" => (0, String::new()),
        "unanswered" => (1, format!("`{name}` came to rest without taking its prompt up")),
        "silent" => (1, format!("`{name}` came to rest with no message of its own to give: it never took its prompt up (or its conversation couldn't be read)")),
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
                "minding" => format!("{who}: {}", ev["summary"].as_str().unwrap_or("minding")),
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

/// Validates arguments for the restricted `mind-send` entry point called by
/// a minder's `mind` script: allows only `<TARGET>`, plain text (`--text` or
/// positional), `--enter`/`--submit`, `--paste`, `--json`, `--`, and chooser
/// navigation `--key` names. Explicitly refuses `--wait`, `--out`, `--timeout`,
/// `--file`, `--force`, `--ctrl`, `--verify`, and unknown options.
fn validate_mind_send_args(args: &[String]) -> Result<(), (String, i32)> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--" => break,
            "--enter" | "--submit" | "--paste" | "--json" => {}
            "--text" => {
                i += 1;
                if i >= args.len() {
                    return Err(("--text requires text".into(), 2));
                }
            }
            s if s.starts_with("--text=") => {}
            "--key" => {
                i += 1;
                let Some(k) = args.get(i) else {
                    return Err(("--key requires a chooser key name".into(), 2));
                };
                if !olo_rex_link::agents::is_minder_chooser_key(k) {
                    return Err((format!("a minder may not send `--key {k}` (only chooser keys: Up, Down, Left, Right, Tab, Shift+Tab, Space, Backspace, Enter, Escape)"), 69));
                }
            }
            s if s.starts_with("--key=") => {
                let k = &s["--key=".len()..];
                if !olo_rex_link::agents::is_minder_chooser_key(k) {
                    return Err((format!("a minder may not send `--key {k}` (only chooser keys: Up, Down, Left, Right, Tab, Shift+Tab, Space, Backspace, Enter, Escape)"), 69));
                }
            }
            "--wait" | "--out" | "--timeout" | "--file" | "--force" | "--ctrl" | "--verify" => {
                return Err((format!("`{a}` is not allowed from a minder's `mind send`"), 69));
            }
            s if s.starts_with("--out=") || s.starts_with("--timeout=") || s.starts_with("--file=") || s.starts_with("--ctrl=") => {
                return Err((format!("`{s}` is not allowed from a minder's `mind send`"), 69));
            }
            s if s.starts_with('-') => {
                return Err((format!("unknown `mind send` option `{s}`"), 2));
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
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
        "start" | "spawn" | "run" | "resume" => [("--prompt-file", "prompt", &["--prompt"][..], &[][..])].into_iter().chain(RULES).collect(),
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
    /// The service took it and had not answered when its time was up
    /// (that many seconds): what was done isn't known.
    Late(u64),
}

/// One request sent, and what came of it: the service's mark that it
/// took it (an empty line), then its answer.
fn exchange(stream: UnixStream, req: &Request) -> Asked {
    exchange_within(stream, req, answer_within(req))
}

/// How long the service has to answer a request: a `watch` its own
/// time and half a minute (ten minutes where it names none, as the
/// service has it); anything else two minutes, which is many times what
/// the slowest of them takes (a start with its folders' trust, a stop
/// that waits for programs to end). No request waits for ever: a `run`
/// in a test here sat 3 h 45 min on a service that had taken its
/// request and never answered it, its own `--timeout 40` notwithstanding
/// (found still waiting, 2026-11-27).
fn answer_within(req: &Request) -> Duration {
    if req.op != "watch" {
        return Duration::from_secs(120);
    }
    let seconds = |v: &str| v.trim_end_matches('s').parse::<u64>().ok();
    let mut named = None;
    let mut args = req.args.iter();
    while let Some(arg) = args.next() {
        if arg == "--timeout" {
            named = args.next().and_then(|v| seconds(v)).or(named);
        } else if let Some(v) = arg.strip_prefix("--timeout=") {
            named = seconds(v).or(named);
        }
    }
    // (Or its structured form: `rex do olo.agent.watch --args '{"timeout": N}'`.)
    let named = named.or_else(|| req.obj.as_ref().and_then(|o| o["timeout"].as_u64()));
    Duration::from_secs(named.unwrap_or(600).min(3600) + 30)
}

fn exchange_within(mut stream: UnixStream, req: &Request, within: Duration) -> Asked {
    let Ok(mut line) = serde_json::to_string(req) else { return Asked::NotTaken };
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
        return Asked::NotTaken;
    }
    let _ = stream.set_read_timeout(Some(within));
    let late = |e: &std::io::Error| matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut);
    let mut lines = BufReader::new(stream);
    let mut taken = String::new();
    match lines.read_line(&mut taken) {
        Ok(0) => return Asked::NotTaken,
        Ok(_) => {}
        Err(e) if late(&e) => return Asked::Late(within.as_secs()),
        Err(_) => return Asked::NotTaken,
    }
    let mut answer = String::new();
    // (A service from before the mark answers at once.)
    let said = if taken.trim().is_empty() {
        match lines.read_line(&mut answer) {
            Ok(_) => answer.as_str(),
            Err(e) if late(&e) => return Asked::Late(within.as_secs()),
            Err(_) => "",
        }
    } else {
        taken.as_str()
    };
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
    // (Its log's last line, less when it was written and whose it is.)
    let last_word =
        || std::fs::read_to_string(&paths.log).ok().and_then(|log| log.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.split_once("olo agents: ").map_or(l, |(_, word)| word).to_owned()));
    let mut service = start_service(server, paths).map_err(|e| unavailable(format!("the agents service couldn't be started: {e}")))?;
    // (It listens once it has reached the Rex server and read what the
    // last one wrote: waited for here. One that ends without listening
    // says why in its log, unless another got there first and listens.)
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.socket) {
            // (The one it started is its child for as long as this
            // command lives, a watch that goes on for hours: waited for
            // on a thread, so that when it ends, or ended at once because
            // another was there first, it isn't left a process that has
            // ended and is never collected. A follower had two such,
            // found 2026-11-27.)
            std::thread::spawn(move || {
                let _ = service.wait();
            });
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
        "OLO_REX_AGENTS_MINDS_MOST",
        "OLO_REX_AGENTS_TAKEN_UP_MS",
        "OLO_REX_AGENTS_TEST_START_HOLD_MS",
        "OLO_REX_AGENTS_TEST_MIND_HOLD_MS",
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
            Asked::Late(secs) => {
                return Response {
                    ok: false,
                    stdout: String::new(),
                    stderr: format!(
                        "olo agent {}: the agents service took this and had not answered after {secs} s: it may be stuck (`olo agent ping` says whether it answers at all). What was done isn't known: look (`olo agent list`) before asking again\n",
                        req.op
                    ),
                    code: AGAIN,
                    data: Value::Null,
                };
            }
        }
    }
    lost()
}

/// `agent follow`: everything the service knows, then each change, a
/// line each, for as long as this command's input stays open (the
/// desktop closes it to end the following). Its status says why it
/// ended: 0 asked to, 75 the service went away (follow again).
fn follow(server: &Path, paths: &Paths) -> i32 {
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    let req = Request { proto: PROTO, op: "follow".into(), block: env("REX_BLOCK"), session: env("REX_SESSION"), ..Default::default() };
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
            Some(Ok(first)) => {
                // (A direct refusal before the stream begins — e.g. a minder calling `follow`.)
                if let Ok(resp) = serde_json::from_str::<Response>(&first)
                    && !resp.ok
                    && resp.code != 0
                {
                    return print_answer(resp);
                }
                first
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// No request waits for ever on a service that took it: its time
    /// is its own (a watch's) or two minutes, and when it passes the
    /// command says so. (Tried with a service that takes a request,
    /// says so, and never answers.)
    #[test]
    fn a_request_the_service_never_answers_ends() {
        let watch = |args: &[&str]| Request { proto: PROTO, op: "watch".into(), args: args.iter().map(|a| (*a).to_owned()).collect(), ..Default::default() };
        assert_eq!(answer_within(&watch(&["worker", "--timeout", "40", "--json"])), Duration::from_secs(70));
        assert_eq!(answer_within(&watch(&["worker", "--timeout=90s"])), Duration::from_secs(120));
        assert_eq!(answer_within(&watch(&["--all"])), Duration::from_secs(630), "a watch that names no time has the service's ten minutes");
        assert_eq!(answer_within(&Request { proto: PROTO, op: "watch".into(), obj: Some(serde_json::json!({ "timeout": 15 })), ..Default::default() }), Duration::from_secs(45));
        assert_eq!(answer_within(&Request { proto: PROTO, op: "start".into(), ..Default::default() }), Duration::from_secs(120));

        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-agent-client-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("sock");
        let _ = std::fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        // It takes the request (its mark: an empty line) and says no more, the connection kept open.
        let service = std::thread::spawn(move || {
            let (mut asked, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(asked.try_clone().unwrap()).read_line(&mut line).unwrap();
            asked.write_all(b"\n").unwrap();
            std::thread::sleep(Duration::from_millis(1500));
            line
        });
        let began = Instant::now();
        let got = exchange_within(UnixStream::connect(&sock).unwrap(), &watch(&["worker", "--timeout", "40"]), Duration::from_millis(300));
        assert!(matches!(got, Asked::Late(_)) && began.elapsed() < Duration::from_millis(1200), "after {:?}", began.elapsed());
        assert!(service.join().unwrap().contains("\"watch\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
