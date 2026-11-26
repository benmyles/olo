//! `olo-rex-link move look-pane [--screens HEX] PID…`: what runs in each of
//! Rex's panes, for a move: each pane's whole process tree (command lines
//! whole, folders, start times, the agents' session files open), and the
//! agent there with the session it is in and how that was known (JSON).
//!
//! The session, by the surest evidence there is, and each pane its own (two
//! panes are never given one session):
//! - Claude Code's own record of each process of it that runs
//!   (`~/.claude/sessions/PID.json`: the session it is in now, after a
//!   `/resume` or a `/clear` too), checked to be that process by its start;
//! - a session file held open (a Codex without its daemon: its rollout;
//!   agy: its conversation's database);
//! - Codex's daemon (`codex app-server`, where its windows' turns run, so
//!   no window holds its rollout): the conversations it has open in the
//!   pane's folder;
//! - a Claude Code with no record (an older one), or an agy: the sessions
//!   of its folder written since it started;
//! - the command line (`--resume ID`, `codex resume … ID`,
//!   `--conversation=ID`);
//! - else the newest of its folder: a guess, said so.
//!
//! Where a pane has more than one candidate, or two panes one, each pane's
//! screen (some of its lines: `--screens`, hex-coded JSON from the app,
//! `{"PID": ["line", …]}`) is matched against the end of each candidate's
//! transcript, and the best matches go first.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::procs::{Proc, all_procs, codex_home, home, unhex};

/// Where the agents keep their sessions: the home, Claude Code's folder
/// (`CLAUDE_CONFIG_DIR`, else `~/.claude`), Codex's (`CODEX_HOME`, else
/// `~/.codex`). (A test's are its own scratch: never this computer's
/// agents'.)
pub struct Places {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub codex: PathBuf,
}

impl Places {
    pub fn here() -> Places {
        let home = home();
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
        Places { home, claude, codex: codex_home() }
    }

    pub(crate) fn agy(&self) -> PathBuf {
        self.home.join(".gemini/antigravity-cli/conversations")
    }

    pub(crate) fn agy_brain(&self) -> PathBuf {
        self.home.join(".gemini/antigravity-cli/brain")
    }

    /// Where each Antigravity holds a lock named for the conversation
    /// that is its own (`presence/ID.lock`): the one mark of whose a
    /// conversation is. Its database (`conversations/ID.db`) is no such
    /// mark: an Antigravity that has just started, with no conversation
    /// of its own, opens the others' databases to list them, and was
    /// then said to be in another agent's conversation (the ShiftUp
    /// lead's item 33, 2026-11-26: a launcher resumed what it was told,
    /// and another agent's conversation got a turn that wasn't its own).
    pub(crate) fn agy_presence(&self) -> PathBuf {
        self.home.join(".gemini/antigravity-cli/presence")
    }
}

/// The agents' own session files, as each holds them open (Codex's
/// rollouts, archived ones too; agy's lock on the conversation that is
/// its own, [`Places::agy_presence`]: never its databases, which every
/// agy opens).
fn session_file(places: &Places, target: &Path) -> bool {
    let codex = [places.codex.join("sessions"), places.codex.join("archived_sessions")];
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    (codex.iter().any(|d| target.starts_with(d)) && name.ends_with(".jsonl")) || (target.starts_with(places.agy_presence()) && name.ends_with(".lock"))
}

/// When a process started, in clock ticks after boot (`/proc/PID/stat`'s
/// field 22, counted after the name's closing bracket: a name may hold
/// spaces and brackets of its own).
pub(crate) fn start_ticks(pid: i32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(19)?.parse().ok())).unwrap_or(0)
}

/// One process for `look-pane`.
pub(crate) fn process_json(p: &Proc, places: &Places) -> serde_json::Value {
    let argv: Vec<String> =
        std::fs::read(format!("/proc/{}/cmdline", p.pid)).map(|c| c.split(|&b| b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect()).unwrap_or_default();
    let cwd = std::fs::read_link(format!("/proc/{}/cwd", p.pid)).map(|c| c.display().to_string()).unwrap_or_default();
    let exe = std::fs::read_link(format!("/proc/{}/exe", p.pid)).map(|c| c.display().to_string()).unwrap_or_default();
    let mut open: Vec<String> = std::fs::read_dir(format!("/proc/{}/fd", p.pid))
        .map(|d| d.filter_map(|e| std::fs::read_link(e.ok()?.path()).ok()).filter(|t| session_file(places, t)).map(|t| t.display().to_string()).collect())
        .unwrap_or_default();
    open.sort();
    open.dedup();
    serde_json::json!({"pid": p.pid, "ppid": p.ppid, "name": p.comm, "argv": argv, "cwd": cwd, "exe": exe, "start": start_ticks(p.pid), "open": open})
}

pub(crate) fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn named(n: &str) -> Option<&'static str> {
    match n {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "agy" | "antigravity" => Some("agy"),
        _ => None,
    }
}

/// Which agent a process is (`claude`, `codex`, `agy`), and its command
/// line from the agent's own name on (`node …/cli.js --model x` is
/// `claude --model x`): by its name, its program's, Claude Code's own build
/// (named by its version), or the script an interpreter runs. Only the
/// program itself: `vim claude`, `less codex` are no agent.
pub fn agent_of(argv: &[String], comm: &str, exe: &str) -> Option<(&'static str, Vec<String>)> {
    let rest = |from: usize, agent: &'static str| Some((agent, std::iter::once(agent.to_owned()).chain(argv.iter().skip(from).cloned()).collect()));
    if let Some(agent) = argv.first().and_then(|a| named(base(a))) {
        return rest(1, agent);
    }
    // (A script its interpreter runs: its name, or its package's.)
    let interpreter =
        argv.first().map(|a| base(a)).is_some_and(|n| matches!(n, "node" | "nodejs" | "bun" | "deno" | "env" | "sh" | "bash" | "dash" | "zsh") || n.starts_with("node") || n.starts_with("python"));
    if interpreter && let Some(at) = argv.iter().skip(1).position(|a| !a.starts_with('-')).map(|i| i + 1) {
        let script = &argv[at];
        let agent = named(base(script)).or_else(|| {
            if script.contains("@anthropic-ai/claude-code") {
                Some("claude")
            } else if script.contains("@openai/codex") {
                Some("codex")
            } else {
                None
            }
        });
        if let Some(agent) = agent {
            return rest(at + 1, agent);
        }
    }
    // (Claude Code's own build: its name is its version, "2.1.331".)
    if exe.contains("/claude/versions/") || named(comm).is_some() && argv.is_empty() {
        return rest(1, named(comm).unwrap_or("claude"));
    }
    named(comm).and_then(|agent| rest(1, agent))
}

/// Whether `w` is a session's id as the agents write them (a UUID).
pub(crate) fn uuid(w: &str) -> bool {
    let parts: Vec<&str> = w.split('-').collect();
    parts.len() == 5 && parts.iter().zip([8, 4, 4, 4, 12]).all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The session id on an agent's command line (`args` from its name on):
/// Claude Code's `--resume ID`, `-r ID`, `--session-id ID`; Codex's
/// `resume [FLAGS…] ID`; agy's `--conversation ID` (`-conversation`,
/// `--resume`, `-r`); each with `=ID` too. Only an id: `--resume TERM`
/// opens Claude Code's picker.
pub fn argv_session(agent: &str, args: &[String]) -> Option<String> {
    let flags: &[&str] = match agent {
        "claude" => &["--resume", "-r", "--session-id"],
        "codex" => &[],
        _ => &["--conversation", "-conversation", "--resume", "-r"],
    };
    let words = args.get(1..).unwrap_or_default();
    if agent == "codex" {
        let after = words.iter().position(|w| w == "resume")?;
        return words[after + 1..].iter().find(|w| uuid(w)).cloned();
    }
    words.iter().enumerate().find_map(|(i, w)| {
        let (flag, value) = match w.split_once('=') {
            Some((f, v)) => (f, Some(v.to_owned())),
            None => (w.as_str(), words.get(i + 1).cloned()),
        };
        flags.contains(&flag).then_some(value).flatten().filter(|v| uuid(v))
    })
}

/// When a process started, as the clock has it (its start in ticks after
/// boot, and boot's time).
pub(crate) fn started_at(start_ticks: u64) -> Option<SystemTime> {
    let boot: u64 = std::fs::read_to_string("/proc/stat").ok()?.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())?;
    // SAFETY: a getter.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    Some(std::time::UNIX_EPOCH + Duration::from_secs(boot) + Duration::from_millis(start_ticks * 1000 / hz))
}

/// Claude Code's name for a project's folder: its path, each byte but a
/// letter or a digit as `-`.
pub(crate) fn project_name(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The session Claude Code's own record of process `pid` says it is in,
/// if the record is that process's (by its start: a pid is used again).
pub(crate) fn claude_record(places: &Places, pid: i32, start: u64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(places.claude.join("sessions").join(format!("{pid}.json"))).ok()?).ok()?;
    let recorded = match &v["procStart"] {
        serde_json::Value::String(s) => s.parse::<u64>().ok()?,
        n => n.as_u64()?,
    };
    (recorded == start).then(|| v["sessionId"].as_str().map(str::to_owned)).flatten().filter(|s| uuid(s))
}

/// A session that may be a pane's: its id, its transcript, when it was
/// last written.
#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub(crate) id: String,
    pub(crate) file: Option<PathBuf>,
    pub(crate) when: SystemTime,
}

/// The files of `dir` named `ID` + `ext` (an id a UUID), newest first;
/// written since `since` if given.
pub(crate) fn sessions_in(dir: &Path, ext: &str, since: Option<SystemTime>) -> Vec<Candidate> {
    let mut all: Vec<Candidate> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(ext)?;
                let when = e.metadata().ok()?.modified().ok()?;
                (uuid(id) && since.is_none_or(|s| when >= s)).then(|| Candidate { id: id.to_owned(), file: Some(e.path()), when })
            })
            .collect()
        })
        .unwrap_or_default();
    all.sort_by(|a, b| b.when.cmp(&a.when));
    all
}

/// Codex's rollouts by their conversation's id (`rollout-STAMP-ID.jsonl`,
/// in folders by date, archived ones too).
pub(crate) fn codex_rollouts(places: &Places) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(places.codex.join("sessions"), 0), (places.codex.join("archived_sessions"), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let path = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 4 {
                    stack.push((path, depth + 1));
                }
            } else if let Some(stem) = e.file_name().to_str().and_then(|n| n.strip_suffix(".jsonl")).map(str::to_owned) {
                let parts: Vec<&str> = stem.split('-').collect();
                if parts.len() >= 5 {
                    let id = parts[parts.len() - 5..].join("-");
                    if uuid(&id) {
                        out.insert(id, path);
                    }
                }
            }
        }
    }
    out
}

/// What Codex's daemon has open: each conversation's id and folder (none
/// when there is no daemon). Over its control socket (a WebSocket, as the
/// Token Pool asks it), five seconds at most.
pub(crate) fn codex_threads(places: &Places) -> Vec<(String, String)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let Ok(stream) = UnixStream::connect(places.codex.join("app-server-control/app-server-control.sock")) else { return Vec::new() };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let Ok((mut ws, _)) = tungstenite::client::client("ws://localhost/", stream) else { return Vec::new() };
    let mut next = 1u64;
    let mut call = |ws: &mut tungstenite::WebSocket<UnixStream>, method: &str, params: serde_json::Value| -> Option<serde_json::Value> {
        let id = next;
        next += 1;
        ws.send(tungstenite::Message::text(serde_json::json!({"id": id, "method": method, "params": params}).to_string())).ok()?;
        while Instant::now() < deadline {
            let msg = ws.read().ok()?;
            let Ok(v) = serde_json::from_str::<serde_json::Value>(msg.to_text().unwrap_or_default()) else { continue };
            if v["id"] == id && v.get("method").is_none() {
                return v.get("error").is_none().then(|| v["result"].clone());
            }
        }
        None
    };
    let info = serde_json::json!({"clientInfo": {"name": "olo-rex-link", "title": "Olo Rex", "version": olo_rex_link::VERSION}});
    if call(&mut ws, "initialize", info).is_none() {
        return Vec::new();
    }
    let _ = ws.send(tungstenite::Message::text(serde_json::json!({"method": "initialized"}).to_string()));
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    while let Some(page) = call(&mut ws, "thread/loaded/list", serde_json::json!({"cursor": cursor})) {
        for id in page["data"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
            if let Some(read) = call(&mut ws, "thread/read", serde_json::json!({"threadId": id, "includeTurns": false}))
                && let Some(cwd) = read["thread"]["cwd"].as_str()
            {
                out.push((id.to_owned(), cwd.to_owned()));
            }
        }
        match page["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    let _ = ws.close(None);
    out
}

/// Text with every run of white space one space.
fn plain(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The end of a transcript as plain text: every string in its last half
/// megabyte of JSON lines, one after another.
fn transcript_tail(file: &Path) -> String {
    let Ok(mut f) = std::fs::File::open(file) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(512 << 10);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    let mut lines = bytes.split(|&b| b == b'\n');
    if from > 0 {
        lines.next();
    }
    fn strings(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) if s.len() >= 8 => {
                out.push_str(s);
                out.push('\n');
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    for line in lines {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) {
            strings(&v, &mut out);
        }
    }
    plain(&out)
}

/// How many of a screen's lines a transcript's end has.
fn score(lines: &[String], tail: &str) -> usize {
    lines.iter().filter(|l| tail.contains(l.as_str())).count()
}

/// A pane's agent, as found.
pub(crate) struct Found {
    pub(crate) agent: &'static str,
    pub(crate) pid: i32,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) start: u64,
    pub(crate) open: Option<String>,
    /// Its own processes (Claude Code's record may be any one's).
    pub(crate) pids: Vec<(i32, u64)>,
}

/// The agent in a pane's tree (parents first), if one runs there.
pub(crate) fn found_in(tree: &[serde_json::Value]) -> Option<Found> {
    let words = |p: &serde_json::Value, key: &str| -> Vec<String> { p[key].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default() };
    let (p, agent, args) = tree.iter().find_map(|p| {
        let (agent, args) = agent_of(&words(p, "argv"), p["name"].as_str().unwrap_or_default(), p["exe"].as_str().unwrap_or_default())?;
        Some((p, agent, args))
    })?;
    // (The session file any of the tree holds open: Codex's rollout's id,
    // `rollout-STAMP-ID.jsonl`; agy's own conversation's, by its lock,
    // `ID.lock`.)
    let open = tree.iter().flat_map(|q| words(q, "open")).find_map(|f| {
        let name = base(&f).to_owned();
        if let Some(stem) = name.strip_suffix(".lock") {
            return uuid(stem).then(|| stem.to_owned());
        }
        let parts: Vec<&str> = name.strip_suffix(".jsonl")?.split('-').collect();
        let id = (parts.len() >= 5).then(|| parts[parts.len() - 5..].join("-"))?;
        uuid(&id).then_some(id)
    });
    let pids = tree
        .iter()
        .filter(|q| agent_of(&words(q, "argv"), q["name"].as_str().unwrap_or_default(), q["exe"].as_str().unwrap_or_default()).is_some_and(|(a, _)| a == agent))
        .filter_map(|q| Some((q["pid"].as_i64()? as i32, q["start"].as_u64()?)))
        .collect();
    Some(Found { agent, pid: p["pid"].as_i64().unwrap_or(0) as i32, args, cwd: p["cwd"].as_str().unwrap_or_default().to_owned(), start: p["start"].as_u64().unwrap_or(0), open, pids })
}

/// Each pane's agent and the session it is in (`null` for a pane with
/// none), as the module's head says. `threads`: what Codex's daemon has
/// open, asked only where a Codex pane needs it.
pub(crate) fn sessions(
    found: &[Option<Found>],
    screens: &HashMap<i32, Vec<String>>,
    roots: &[i32],
    places: &Places,
    threads: &mut dyn FnMut() -> Vec<(String, String)>,
) -> Vec<Option<(String, &'static str)>> {
    let mut out: Vec<Option<(String, &'static str)>> = vec![None; found.len()];
    let mut taken: HashSet<String> = HashSet::new();
    // (Asked once, where a Codex pane needs it.)
    let mut open_threads: Option<Vec<(String, String)>> = None;
    let mut rollouts: Option<HashMap<String, PathBuf>> = None;
    // What is sure, first: a record, a file held open.
    for (i, f) in found.iter().enumerate() {
        let Some(f) = f else { continue };
        let sure = if f.agent == "claude" { f.pids.iter().find_map(|&(pid, start)| claude_record(places, pid, start)).map(|id| (id, "record")) } else { None };
        let sure = sure.or_else(|| f.open.clone().map(|id| (id, "open")));
        if let Some((id, how)) = sure
            && taken.insert(id.clone())
        {
            out[i] = Some((id, how));
        }
    }
    // Then each pane's candidates: the daemon's in its folder (Codex), its
    // folder's written since it started (Claude Code, agy).
    let mut pending: Vec<(usize, Vec<Candidate>, &'static str)> = Vec::new();
    for (i, f) in found.iter().enumerate() {
        let Some(f) = f else { continue };
        if out[i].is_some() {
            continue;
        }
        let since = started_at(f.start);
        let (candidates, how): (Vec<Candidate>, &'static str) = match f.agent {
            "codex" => {
                let threads = open_threads.get_or_insert_with(&mut *threads);
                let rollouts = rollouts.get_or_insert_with(|| codex_rollouts(places));
                let mut c: Vec<Candidate> = threads
                    .iter()
                    .filter(|(_, cwd)| *cwd == f.cwd)
                    .map(|(id, _)| {
                        let file = rollouts.get(id).cloned();
                        let when = file.as_ref().and_then(|p| std::fs::metadata(p).ok()?.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
                        Candidate { id: id.clone(), file, when }
                    })
                    .collect();
                c.sort_by(|a, b| b.when.cmp(&a.when));
                (c, "daemon")
            }
            "claude" => (sessions_in(&places.claude.join("projects").join(project_name(&f.cwd)), ".jsonl", since), "written"),
            _ => (sessions_in(&places.agy(), ".db", since), "written"),
        };
        let candidates: Vec<Candidate> = candidates.into_iter().filter(|c| !taken.contains(&c.id)).collect();
        if !candidates.is_empty() {
            pending.push((i, candidates, how));
        }
    }
    // Each candidate to one pane: the best matches of screen to transcript
    // first, then the newest.
    let shared: HashMap<&str, usize> = pending.iter().flat_map(|(_, c, _)| c.iter().map(|c| c.id.as_str())).fold(HashMap::new(), |mut m, id| {
        *m.entry(id).or_default() += 1;
        m
    });
    let mut tails: HashMap<PathBuf, String> = HashMap::new();
    let mut pairs: Vec<(usize, SystemTime, usize, String, &'static str, bool)> = Vec::new();
    for (i, candidates, how) in &pending {
        let lines = screens.get(&roots[*i]).map(Vec::as_slice).unwrap_or_default();
        let alone = candidates.len() == 1 && shared.get(candidates[0].id.as_str()) == Some(&1);
        for c in candidates {
            let s = if alone || lines.is_empty() { 0 } else { c.file.as_ref().map_or(0, |file| score(lines, tails.entry(file.clone()).or_insert_with(|| transcript_tail(file)))) };
            pairs.push((s, c.when, *i, c.id.clone(), how, alone));
        }
    }
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    for (s, _, i, id, how, alone) in pairs {
        if out[i].is_some() || taken.contains(&id) {
            continue;
        }
        let how = if alone {
            how
        } else if s > 0 {
            "screen"
        } else {
            "guess"
        };
        taken.insert(id.clone());
        out[i] = Some((id, how));
    }
    // Then the command line; then, for Claude Code, the newest of its
    // folder.
    for (i, f) in found.iter().enumerate() {
        let Some(f) = f else { continue };
        if out[i].is_some() {
            continue;
        }
        if let Some(id) = argv_session(f.agent, &f.args).filter(|id| !taken.contains(id)) {
            taken.insert(id.clone());
            out[i] = Some((id, "argv"));
        } else if f.agent == "claude"
            && let Some(c) = sessions_in(&places.claude.join("projects").join(project_name(&f.cwd)), ".jsonl", None).into_iter().find(|c| !taken.contains(&c.id))
        {
            taken.insert(c.id.clone());
            out[i] = Some((c.id, "guess"));
        }
    }
    out
}

/// `move look-pane [--screens HEX] PID…`: for each pane's process (Rex's
/// report gives its pid), it and everything under it, parents first, and
/// the agent there with its session (JSON).
#[cfg_attr(not(feature = "move"), allow(dead_code))]
pub fn look_pane(args: &[String]) -> i32 {
    let (mut screens, mut pids) = (HashMap::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--screens" {
            let parsed: Option<HashMap<String, Vec<String>>> = it.next().and_then(|h| unhex(h)).and_then(|b| serde_json::from_slice(&b).ok());
            let Some(parsed) = parsed else {
                eprintln!("olo-rex-link move look-pane: --screens takes hex-coded JSON");
                return 2;
            };
            screens = parsed.into_iter().filter_map(|(k, v)| Some((k.parse::<i32>().ok()?, v.iter().map(|l| plain(l)).filter(|l| l.len() >= 16).collect()))).collect();
        } else if let Ok(pid) = a.parse::<i32>() {
            pids.push(pid);
        } else {
            eprintln!("usage: olo-rex-link move look-pane [--screens HEX] PID...");
            return 2;
        }
    }
    let procs = all_procs();
    let places = Places::here();
    let trees: Vec<Vec<serde_json::Value>> = pids
        .iter()
        .map(|&root| {
            let mut tree = Vec::new();
            let mut at = vec![root];
            while let Some(pid) = at.pop() {
                if let Some(p) = procs.iter().find(|p| p.pid == pid) {
                    tree.push(process_json(p, &places));
                    let mut children: Vec<i32> = procs.iter().filter(|c| c.ppid == pid).map(|c| c.pid).collect();
                    children.sort_unstable_by(|a, b| b.cmp(a));
                    at.extend(children);
                }
            }
            tree
        })
        .collect();
    let found: Vec<Option<Found>> = trees.iter().map(|t| found_in(t)).collect();
    let said = sessions(&found, &screens, &pids, &places, &mut || codex_threads(&places));
    let panes: Vec<serde_json::Value> = pids
        .iter()
        .zip(trees)
        .zip(found.iter().zip(said))
        .map(|((&root, tree), (f, session))| {
            let agent = f.as_ref().map_or(serde_json::Value::Null, |f| {
                let (session, how) = session.map_or((None, "none"), |(id, how)| (Some(id), how));
                serde_json::json!({"agent": f.agent, "pid": f.pid, "argv": f.args, "cwd": f.cwd, "session": session, "how": how})
            });
            serde_json::json!({"pid": root, "tree": tree, "agent": agent})
        })
        .collect();
    println!("{}", serde_json::Value::Array(panes));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    /// An agent is the program itself, or a script its interpreter runs;
    /// its command line from its own name on. A program that names one
    /// (an editor, a pager) is none.
    #[test]
    fn an_agent_is_the_program_itself() {
        assert_eq!(agent_of(&v(&["claude", "--model", "opus"]), "claude", ""), Some(("claude", v(&["claude", "--model", "opus"]))));
        assert_eq!(agent_of(&v(&["/usr/bin/node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js", "-c"]), "node", ""), Some(("claude", v(&["claude", "-c"]))));
        assert_eq!(agent_of(&v(&["node", "/x/@openai/codex/bin/codex.js", "--yolo"]), "node", ""), Some(("codex", v(&["codex", "--yolo"]))));
        assert_eq!(agent_of(&v(&["/bin/bash", "/home/u/.local/bin/claude", "--resume", "x"]), "claude", ""), Some(("claude", v(&["claude", "--resume", "x"]))));
        assert_eq!(agent_of(&v(&["claude"]), "2.1.331", "/home/u/.local/share/claude/versions/2.1.331"), Some(("claude", v(&["claude"]))));
        assert_eq!(agent_of(&v(&["/home/u/.local/bin/agy", "--conversation", "x"]), "agy", ""), Some(("agy", v(&["agy", "--conversation", "x"]))));
        for not in [&["vim", "claude"][..], &["less", "codex"], &["man", "agy"], &["rm", "/home/u/.local/bin/claude"], &["bash", "-lc", "echo"]] {
            assert_eq!(agent_of(&v(not), not[0], ""), None, "{not:?}");
        }
    }

    /// A session id is found after its flag, `=` or not, Codex's after
    /// `resume` and its flags; never a word that isn't an id.
    #[test]
    fn a_session_is_read_from_a_command_line() {
        let id = "01a1f085-335d-7433-9592-3e263e8d6c4c";
        assert_eq!(argv_session("claude", &v(&["claude", "--model", "opus", "--resume", id])), Some(id.to_owned()));
        assert_eq!(argv_session("claude", &v(&["claude", &format!("--resume={id}")])), Some(id.to_owned()));
        assert_eq!(argv_session("claude", &v(&["claude", "--resume", "my search"])), None);
        assert_eq!(argv_session("codex", &v(&["codex", "resume", "--yolo", "-m", "o9", id])), Some(id.to_owned()));
        assert_eq!(argv_session("codex", &v(&["codex", "--yolo"])), None);
        assert_eq!(argv_session("agy", &v(&["agy", "--yolo", &format!("--conversation={id}")])), Some(id.to_owned()));
        assert_eq!(argv_session("agy", &v(&["agy", "-r", id])), Some(id.to_owned()));
        assert!(uuid(id) && !uuid("01a1f085-335d-7433-9592") && !uuid("zz1f085-335d-7433-9592-3e263e8d6c4cz"));
    }

    fn scratch(name: &str) -> Places {
        let d = PathBuf::from(format!("/var/tmp/olo-rex-link-panes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Places { claude: d.join(".claude"), codex: d.join(".codex"), home: d }
    }

    /// The agents' session files are told by where they are: Codex's
    /// rollouts (archived too), and agy's lock on its own conversation,
    /// never the conversations' databases (every agy opens those).
    #[test]
    fn session_files_are_told_by_their_place() {
        let h = Places { home: "/home/f".into(), claude: "/home/f/.claude".into(), codex: "/home/f/.codex".into() };
        assert!(session_file(&h, Path::new("/home/f/.codex/sessions/2026/11/25/rollout-x-ID.jsonl")));
        assert!(session_file(&h, Path::new("/home/f/.codex/archived_sessions/rollout-x-ID.jsonl")));
        assert!(session_file(&h, Path::new("/home/f/.gemini/antigravity-cli/presence/ID.lock")));
        for not_its_own in ["conversations/ID.db", "conversations/ID.db-wal", "conversations/ID.db-shm", "conversation_summaries.db", "brain/ID/.system_generated/logs/transcript.jsonl"] {
            assert!(!session_file(&h, &Path::new("/home/f/.gemini/antigravity-cli").join(not_its_own)), "{not_its_own}");
        }
        assert!(!session_file(&h, Path::new("/home/f/.codex/config.toml")));
        assert!(!session_file(&h, Path::new("/home/g/.codex/sessions/a.jsonl")));
    }

    /// A pane's process read whole: its command line with a newline and a
    /// space in its arguments, its start time though its name has a space
    /// and a bracket.
    #[test]
    fn a_panes_process_is_read_whole() {
        let places = scratch("whole");
        // (A process that names itself with a space and brackets, as a
        // program may: its name in /proc/PID/stat is that.)
        let mut child = std::process::Command::new("bash").args(["-c", "printf 'odd (name) x' > /proc/self/comm; sleep 30 & wait"]).spawn().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while crate::procs::proc_of(child.id() as i32).is_none_or(|p| p.comm != "odd (name) x") && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let p = crate::procs::proc_of(child.id() as i32).unwrap();
        assert_eq!(p.comm, "odd (name) x");
        let v = process_json(&p, &places);
        assert_eq!(v["argv"][1], "-c");
        assert!(v["start"].as_u64().unwrap() > 0, "{v}");
        let _ = child.kill();
        let _ = child.wait();
        let mut child = std::process::Command::new("bash").args(["-c", "sleep 30 & wait", "a word\nwith a line"]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let v = process_json(&crate::procs::proc_of(child.id() as i32).unwrap(), &places);
        assert_eq!(v["argv"][3], "a word\nwith a line");
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&places.home);
    }

    fn proc_json(pid: u32, argv: &[&str], cwd: &str, start: u64, open: &[&str]) -> serde_json::Value {
        serde_json::json!({"pid": pid, "name": base(argv[0]), "argv": argv, "cwd": cwd, "exe": "", "start": start, "open": open})
    }

    /// Each pane's session by the surest evidence, each pane its own:
    /// Claude Code's record of its process (checked by its start), a file
    /// held open, Codex's daemon's conversation in the pane's folder, the
    /// command line; a program that isn't an agent, none.
    #[test]
    fn each_pane_is_given_its_own_session() {
        let places = scratch("own");
        let (s1, s2, s3, c1, c2) = (
            "11111111-0000-4000-8000-000000000001",
            "22222222-0000-4000-8000-000000000002",
            "33333333-0000-4000-8000-000000000003",
            "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c",
        );
        // (Claude Code's records: pane 0's process is its record's; pane
        // 1's record is of another process that had its pid.)
        std::fs::create_dir_all(places.claude.join("sessions")).unwrap();
        std::fs::write(places.claude.join("sessions/101.json"), format!("{{\"pid\":101,\"sessionId\":\"{s1}\",\"procStart\":\"5000\"}}")).unwrap();
        std::fs::write(places.claude.join("sessions/201.json"), format!("{{\"pid\":201,\"sessionId\":\"{s2}\",\"procStart\":\"1\"}}")).unwrap();
        let trees = [
            vec![proc_json(100, &["bash"], "/w/a", 4000, &[]), proc_json(101, &["claude", "--resume", s3], "/w/a", 5000, &[])],
            vec![proc_json(201, &["claude", "--resume", s3], "/w/b", 6000, &[])],
            vec![proc_json(301, &["codex", "--yolo"], "/w/c", 7000, &[])],
            vec![proc_json(401, &["agy", &format!("--conversation={c2}")], "/w/d", 8000, &[])],
            vec![proc_json(501, &["tail", "-f", "todo.txt"], "/w", 9000, &[])],
        ];
        let found: Vec<Option<Found>> = trees.iter().map(|t| found_in(t)).collect();
        let mut asked = 0;
        let said = sessions(&found, &HashMap::new(), &[100, 201, 301, 401, 501], &places, &mut || {
            asked += 1;
            vec![(c1.to_owned(), "/w/c".to_owned()), (c2.to_owned(), "/w/elsewhere".to_owned())]
        });
        assert_eq!(said[0], Some((s1.to_owned(), "record")), "its record, not its command line");
        assert_eq!(said[1], Some((s3.to_owned(), "argv")), "a record of another process is no evidence");
        assert_eq!(said[2], Some((c1.to_owned(), "daemon")), "the daemon's conversation in its folder");
        assert_eq!(said[3], Some((c2.to_owned(), "argv")));
        assert_eq!(said[4], None);
        assert_eq!(asked, 1, "the daemon asked once");
        let _ = std::fs::remove_dir_all(&places.home);
    }

    /// An Antigravity's conversation is the one it holds the lock of.
    /// One that has just started holds none, whatever else it has open
    /// (as read from two of them, 2026-11-26: the second held the
    /// first's database open and no lock).
    #[test]
    fn an_agys_conversation_is_the_one_it_holds_the_lock_of() {
        let (own, other) = ("81d35bc3-2fbb-4a4d-b863-ce005a295017", "c50df071-a1bd-47f2-bb77-8eed71527b20");
        let at = "/home/f/.gemini/antigravity-cli";
        // (As `process_json` lists them: only what `session_file` takes.)
        let first = vec![proc_json(100, &["bash"], "/w", 1, &[]), proc_json(101, &["agy"], "/w", 2, &[&format!("{at}/presence/{own}.lock")])];
        assert_eq!(found_in(&first).unwrap().open.as_deref(), Some(own));
        let fresh = vec![proc_json(201, &["agy"], "/w", 3, &[])];
        assert_eq!(found_in(&fresh).unwrap().open, None, "no lock held: no conversation of its own, the other's ({other}) least of all");
    }

    /// Two Claude Code panes in one folder, two sessions written since
    /// they started: each pane is given the session its screen shows,
    /// never both the newest.
    #[test]
    fn two_panes_in_one_folder_are_told_apart_by_their_screens() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-panes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = ("aaaaaaaa-0000-4000-8000-000000000001", "bbbbbbbb-0000-4000-8000-000000000002");
        std::fs::write(dir.join(format!("{a}.jsonl")), "{\"message\":{\"content\":\"Refactor the parser so the tokens keep their spans\"}}\n").unwrap();
        std::fs::write(dir.join(format!("{b}.jsonl")), "{\"message\":{\"content\":\"Write the release notes for the new widget kit\"}}\n").unwrap();
        let cands = sessions_in(&dir, ".jsonl", None);
        assert_eq!(cands.len(), 2);
        let tails: Vec<String> = cands.iter().map(|c| transcript_tail(c.file.as_ref().unwrap())).collect();
        let screen = vec![plain("  Write the release notes for   the new widget kit")];
        let scores: Vec<usize> = tails.iter().map(|t| score(&screen, t)).collect();
        let best = cands.iter().zip(&scores).max_by_key(|(_, s)| **s).unwrap().0;
        assert_eq!(best.id, b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
