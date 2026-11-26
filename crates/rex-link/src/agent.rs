//! `olo-rex-link agent <trust|inspect>`: host-side helpers for native Rex
//! subagent tabs (`olo agent`).
//!
//! - `agent trust PATH...`: atomically marks directories as trusted in Claude
//!   Code's `~/.claude.json` (`projects[path].hasTrustDialogAccepted = true`)
//!   under an advisory `flock` lock so concurrent launches never corrupt it.
//! - `agent inspect`: reads a JSON batch `{ "items": [...] }` on stdin and
//!   returns per-child CLI PID, real CLI session ID, transcript path, full
//!   final assistant message, pending tool call arguments (unfolded),
//!   transcript state/API error, and live descendant background shells in a
//!   single `/proc` pass (fast even at 40+ concurrent children).

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::moving::{Proc, all_procs};
use crate::panes::{Places, agent_of, argv_session, claude_record, codex_rollouts, project_name, sessions_in, start_ticks, started_at, uuid};

const USAGE: &str = "usage: olo-rex-link agent <trust PATH...|inspect>";

pub fn run(args: Vec<String>) -> ! {
    let mut it = args.into_iter();
    let Some(sub) = it.next() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let rest: Vec<String> = it.collect();
    let code = match sub.as_str() {
        "trust" => run_trust(&rest),
        "inspect" => run_inspect(),
        other => {
            eprintln!("olo-rex-link agent: unknown subcommand `{other}`\n{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// `agent trust PATH...`: atomic pre-trust in ~/.claude.json
// ---------------------------------------------------------------------------

fn trust_claude_paths(claude_json: &Path, paths: &[String]) -> io::Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(parent) = claude_json.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = claude_json.with_file_name(".claude.json.olo-trust.lock");
    let lock_file = OpenOptions::new().create(true).write(true).truncate(false).open(&lock_path)?;
    // SAFETY: `lock_file` holds a valid file descriptor; exclusive flock serializes concurrent trust edits.
    unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX);
    }

    let mut root: Value = std::fs::read_to_string(claude_json).ok().and_then(|s| serde_json::from_str(&s).ok()).filter(|v: &Value| v.is_object()).unwrap_or_else(|| json!({}));

    let projects = root.as_object_mut().expect("object").entry("projects").or_insert_with(|| json!({}));
    if !projects.is_object() {
        *projects = json!({});
    }
    let proj_map = projects.as_object_mut().expect("projects object");

    let mut trusted = Vec::new();
    for raw in paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let expanded = expand_tilde(trimmed);
        let canonical = std::fs::canonicalize(&expanded).unwrap_or_else(|_| expanded.clone());
        for candidate in [expanded.display().to_string(), canonical.display().to_string()] {
            if candidate.is_empty() {
                continue;
            }
            let entry = proj_map.entry(candidate.clone()).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            entry.as_object_mut().expect("project entry").insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            if !trusted.contains(&candidate) {
                trusted.push(candidate);
            }
        }
    }

    let encoded = serde_json::to_string_pretty(&root).map_err(|e| io::Error::other(e.to_string()))?;
    let tmp = claude_json.with_file_name(format!(".claude.json.olo-tmp.{}", std::process::id()));
    {
        let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(&tmp)?;
        f.write_all(encoded.as_bytes())?;
        f.write_all(b"\n")?;
        let _ = f.sync_all();
    }
    std::fs::rename(&tmp, claude_json)?;
    // SAFETY: releases exclusive flock on valid fd before drop.
    unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN);
    }
    Ok(trusted)
}

fn expand_tilde(raw: &str) -> PathBuf {
    let home = crate::moving::home();
    if raw == "~" {
        home
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    }
}

fn run_trust(paths: &[String]) -> i32 {
    let places = Places::here();
    let claude_json = places.home.join(".claude.json");
    match trust_claude_paths(&claude_json, paths) {
        Ok(trusted) => {
            println!("{}", json!({ "ok": true, "trusted": trusted }));
            0
        }
        Err(e) => {
            println!("{}", json!({ "ok": false, "error": e.to_string() }));
            1
        }
    }
}

// ---------------------------------------------------------------------------
// `agent inspect`: single-pass batch inspection for up to 40+ child tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InspectBatchRequest {
    #[serde(default)]
    items: Vec<InspectItemRequest>,
}

#[derive(Debug, Deserialize)]
struct InspectItemRequest {
    id: String,
    cli: String,
    #[serde(default)]
    shell_pid: i32,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    known_session_id: Option<String>,
    #[serde(default)]
    known_transcript: Option<String>,
    #[serde(default)]
    started_at: i64,
}

#[derive(Debug, Serialize)]
struct PendingToolInfo {
    tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<Value>,
}

#[derive(Debug, Serialize)]
struct BackgroundShellInfo {
    pid: i32,
    cmd: String,
    cwd: String,
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct InspectItemResult {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<i32>,
    alive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cli_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_message: Option<String>,
    transcript_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_tool: Option<PendingToolInfo>,
    background_shells: Vec<BackgroundShellInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting_on: Option<&'static str>,
}

fn read_argv(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline")).map(|c| c.split(|&b| b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect()).unwrap_or_default()
}

fn read_cwd(pid: i32) -> String {
    std::fs::read_link(format!("/proc/{pid}/cwd")).map(|p| p.display().to_string()).unwrap_or_default()
}

fn read_exe(pid: i32) -> String {
    std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.display().to_string()).unwrap_or_default()
}

fn read_tail_text(path: &Path, max_bytes: u64) -> String {
    let Ok(mut f) = File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut raw = Vec::new();
    let _ = f.read_to_end(&mut raw);
    let slice = if from > 0 {
        match raw.iter().position(|&b| b == b'\n') {
            Some(nl) => &raw[nl + 1..],
            None => &raw[..],
        }
    } else {
        &raw[..]
    };
    String::from_utf8_lossy(slice).into_owned()
}

fn collect_descendants<'a>(root_pid: i32, by_ppid: &HashMap<i32, Vec<&'a Proc>>) -> Vec<&'a Proc> {
    let mut out = Vec::new();
    let mut stack = vec![root_pid];
    let mut seen = HashSet::new();
    seen.insert(root_pid);
    while let Some(p) = stack.pop() {
        if let Some(kids) = by_ppid.get(&p) {
            for &kid in kids {
                if seen.insert(kid.pid) {
                    out.push(kid);
                    stack.push(kid.pid);
                }
            }
        }
    }
    out
}

fn classify_bg_process(cmd: &str) -> &'static str {
    let low = cmd.to_ascii_lowercase();
    let base = low.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("");
    if matches!(base, "sh" | "bash" | "zsh" | "fish" | "dash") && !low.contains(" -c ") {
        return "shell";
    }
    if low.contains("vite")
        || low.contains("next dev")
        || low.contains("webpack")
        || low.contains("uvicorn")
        || low.contains("gunicorn")
        || low.contains("flask run")
        || low.contains("rails server")
        || low.contains("manage.py runserver")
        || low.contains("cargo watch")
        || low.contains("trunk serve")
        || low.contains("http.server")
        || low.contains("dev-server")
        || low.contains("tail -f")
        || low.contains("nodemon")
        || low.contains("postgres")
        || low.contains("redis-server")
        || low.contains("serve ")
    {
        return "dev_server";
    }
    "task"
}

fn find_background_shells(agent_pid: i32, by_ppid: &HashMap<i32, Vec<&Proc>>) -> Vec<BackgroundShellInfo> {
    let desc = collect_descendants(agent_pid, by_ppid);
    let mut out = Vec::new();
    for p in desc {
        let argv = read_argv(p.pid);
        if argv.is_empty() {
            continue;
        }
        let cmd = argv.join(" ");
        // Skip internal MCP/language/helper daemons that belong to the CLI itself.
        let low = cmd.to_ascii_lowercase();
        if low.contains("olo-rex-link") || low.contains("ripgrep") || low.contains("app-server") || (p.comm == "sleep" && argv.len() == 2) {
            continue;
        }
        // Skip wrapper shells whose direct child is already in the list (keep the leaf command).
        let is_wrapper = matches!(p.comm.as_str(), "sh" | "bash" | "zsh" | "dash") && by_ppid.get(&p.pid).is_some_and(|k| !k.is_empty());
        if is_wrapper {
            continue;
        }
        let kind = classify_bg_process(&cmd);
        let cwd = read_cwd(p.pid);
        out.push(BackgroundShellInfo { pid: p.pid, cmd: cmd.chars().take(240).collect(), cwd, kind });
    }
    out
}

fn find_agent_proc<'a>(item: &InspectItemRequest, by_pid: &HashMap<i32, &'a Proc>, by_ppid: &HashMap<i32, Vec<&'a Proc>>) -> Option<(&'a Proc, Vec<String>)> {
    if item.shell_pid > 0 {
        let mut candidates: Vec<&Proc> = Vec::new();
        if let Some(&root) = by_pid.get(&item.shell_pid) {
            candidates.push(root);
        }
        candidates.extend(collect_descendants(item.shell_pid, by_ppid));
        for p in candidates {
            let argv = read_argv(p.pid);
            let exe = read_exe(p.pid);
            if let Some((kind, args)) = agent_of(&argv, &p.comm, &exe)
                && kind == item.cli
            {
                return Some((p, args));
            }
        }
    }
    // Fallback when shell_pid wasn't known: match by known_session_id in argv across all processes.
    if let Some(ref sid) = item.known_session_id
        && !sid.is_empty()
    {
        for &p in by_pid.values() {
            let argv = read_argv(p.pid);
            if argv.iter().any(|a| a.contains(sid.as_str())) {
                let exe = read_exe(p.pid);
                if let Some((kind, args)) = agent_of(&argv, &p.comm, &exe)
                    && kind == item.cli
                {
                    return Some((p, args));
                }
            }
        }
    }
    None
}

fn resolve_session_and_transcript(
    places: &Places,
    cli: &str,
    pid: Option<i32>,
    start: u64,
    cwd: &str,
    args: &[String],
    known_sid: Option<&str>,
    known_transcript: Option<&str>,
    started_at_unix: i64,
    rollouts: &HashMap<String, PathBuf>,
) -> (Option<String>, Option<PathBuf>) {
    let mut sid = known_sid.filter(|s| !s.is_empty()).map(str::to_owned);
    if sid.is_none()
        && cli == "claude"
        && let Some(p) = pid
    {
        sid = claude_record(places, p, start);
    }
    if sid.is_none() {
        sid = argv_session(cli, args);
    }
    // Agy open .db file descriptor reveals exact conversation UUID immediately!
    if sid.is_none()
        && cli == "agy"
        && let Some(p) = pid
        && let Ok(rd) = std::fs::read_dir(format!("/proc/{p}/fd"))
    {
        for entry in rd.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path())
                && target.starts_with(places.agy())
                && let Some(name) = target.file_name().and_then(|n| n.to_str())
            {
                for suf in [".db", ".db-wal", ".db-shm"] {
                    if let Some(stem) = name.strip_suffix(suf)
                        && uuid(stem)
                    {
                        sid = Some(stem.to_owned());
                        break;
                    }
                }
            }
            if sid.is_some() {
                break;
            }
        }
    }

    if let Some(tp) = known_transcript.filter(|s| !s.is_empty()) {
        let p = PathBuf::from(tp);
        if p.exists() {
            return (sid, Some(p));
        }
    }

    let since = if start > 0 {
        started_at(start).map(|t| t.checked_sub(Duration::from_secs(3)).unwrap_or(t))
    } else if started_at_unix > 0 {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(started_at_unix.saturating_sub(3) as u64))
    } else {
        None
    };

    let transcript = match cli {
        "claude" => {
            let proj = places.claude.join("projects").join(project_name(cwd));
            if let Some(ref id) = sid {
                let p = proj.join(format!("{id}.jsonl"));
                if p.exists() { Some(p) } else { None }
            } else {
                sessions_in(&proj, ".jsonl", since).into_iter().find_map(|c| {
                    if sid.is_none() {
                        sid = Some(c.id);
                    }
                    c.file
                })
            }
        }
        "codex" => {
            if let Some(ref id) = sid
                && let Some(p) = rollouts.get(id)
                && p.exists()
            {
                Some(p.clone())
            } else if let Some(p) = pid
                && let Ok(rd) = std::fs::read_dir(format!("/proc/{p}/fd"))
            {
                rd.flatten().find_map(|e| {
                    let t = std::fs::read_link(e.path()).ok()?;
                    if t.extension().is_some_and(|ex| ex == "jsonl") && t.starts_with(places.codex.join("sessions")) {
                        if sid.is_none() {
                            // rollout-YYYY-MM-DDTHH-MM-SS-<uuid>.jsonl
                            if let Some(stem) = t.file_stem().and_then(|s| s.to_str())
                                && stem.len() >= 36
                            {
                                let maybe = &stem[stem.len() - 36..];
                                if uuid(maybe) {
                                    sid = Some(maybe.to_owned());
                                }
                            }
                        }
                        Some(t)
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        }
        "agy" => {
            if let Some(ref id) = sid {
                let p = places.agy_brain().join(id).join(".system_generated/logs/transcript.jsonl");
                if p.exists() { Some(p) } else { None }
            } else if let Some(p) = pid {
                // First check `/proc/<pid>/fd` for an open file under `~/.gemini/antigravity-cli/brain/<UUID>/`.
                let from_fd = std::fs::read_dir(format!("/proc/{p}/fd")).ok().and_then(|rd| {
                    let brain = places.agy_brain();
                    rd.flatten().find_map(|e| {
                        let t = std::fs::read_link(e.path()).ok()?;
                        let rel = t.strip_prefix(&brain).ok()?;
                        let first = rel.components().next()?.as_os_str().to_str()?;
                        if uuid(first) {
                            let tr = brain.join(first).join(".system_generated/logs/transcript.jsonl");
                            if tr.exists() {
                                sid = Some(first.to_owned());
                                return Some(tr);
                            }
                        }
                        None
                    })
                });
                from_fd.or_else(|| {
                    since.and_then(|s| newest_agy_transcript(&places.agy_brain(), Some(s))).map(|(id, p)| {
                        sid = Some(id);
                        p
                    })
                })
            } else {
                None
            }
        }
        _ => None,
    };

    (sid, transcript)
}

fn newest_agy_transcript(brain: &Path, since: Option<SystemTime>) -> Option<(String, PathBuf)> {
    let rd = std::fs::read_dir(brain).ok()?;
    let mut best: Option<(SystemTime, String, PathBuf)> = None;
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str() else { continue };
        if !uuid(id) {
            continue;
        }
        let p = entry.path().join(".system_generated/logs/transcript.jsonl");
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        if since.is_some_and(|s| mtime < s) {
            continue;
        }
        if best.as_ref().is_none_or(|(b, ..)| mtime > *b) {
            best = Some((mtime, id.to_owned(), p));
        }
    }
    best.map(|(_, id, p)| (id, p))
}

struct TranscriptInspection {
    state: &'static str,
    last_message: Option<String>,
    error: Option<String>,
    pending_tool: Option<PendingToolInfo>,
}

fn inspect_claude_transcript(path: &Path) -> TranscriptInspection {
    // Read last 512 KiB so even long final assistant messages & tool args are complete!
    let tail = read_tail_text(path, 512 * 1024);
    let mut state: &'static str = "unknown";
    let mut error: Option<String> = None;
    let mut pending_tool: Option<PendingToolInfo> = None;
    let mut completed_tool_ids: HashSet<String> = HashSet::new();

    // Collect entries in order so we can join multi-line chunks of the same assistant message.id.
    let mut parsed: Vec<Value> = Vec::new();
    for line in tail.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            parsed.push(v);
        }
    }

    // Track completed tool_use_ids from user tool_result entries.
    for v in &parsed {
        if v["type"].as_str() == Some("user")
            && let Some(arr) = v.pointer("/message/content").and_then(Value::as_array)
        {
            for item in arr {
                if item["type"].as_str() == Some("tool_result")
                    && let Some(tid) = item["tool_use_id"].as_str()
                {
                    completed_tool_ids.insert(tid.to_owned());
                }
            }
        }
    }

    // Determine tail state, error, and any uncompleted pending tool_use by scanning backwards.
    for v in parsed.iter().rev() {
        let ty = v["type"].as_str().unwrap_or("");
        match ty {
            "api-request-blob"
            | "api-request-shape"
            | "attachment"
            | "data-flow-records"
            | "data-flow-incomplete"
            | "progress"
            | "last-prompt"
            | "custom-title"
            | "agent-name"
            | "mode"
            | "permission-mode"
            | "session-owner"
            | "atis-latch"
            | "file-history-snapshot"
            | "file-history-delta"
            | "tools-used"
            | "derived-from"
            | "cost-state"
            | "queue-operation" => continue,
            "api-request" => {
                if state == "unknown" {
                    state = "working";
                }
                break;
            }
            "system" => {
                let sub = v["subtype"].as_str().unwrap_or("");
                if sub == "api_error" || v.get("error").is_some_and(|e| !e.is_null()) {
                    state = "error";
                    error =
                        v["error"].as_str().or_else(|| v.pointer("/error/message").and_then(Value::as_str)).or_else(|| v["content"].as_str()).map(str::to_owned).or_else(|| Some("API Error".into()));
                    break;
                }
            }
            "user" => {
                state = "working";
                break;
            }
            "assistant" => {
                if v["isApiErrorMessage"].as_bool().unwrap_or(false) {
                    state = "error";
                    error = extract_claude_text(v).or_else(|| Some("API Error".into()));
                    break;
                }
                if pending_tool.is_none()
                    && let Some(arr) = v.pointer("/message/content").and_then(Value::as_array)
                {
                    for b in arr.iter().rev() {
                        if b["type"].as_str() == Some("tool_use") {
                            let tid = b["id"].as_str().unwrap_or("");
                            if tid.is_empty() || !completed_tool_ids.contains(tid) {
                                let tool = b["name"].as_str().unwrap_or("tool").to_owned();
                                let input = b.get("input").cloned();
                                let command = input.as_ref().and_then(|i| i["command"].as_str().or_else(|| i["file_path"].as_str()).or_else(|| i["path"].as_str())).map(str::to_owned);
                                pending_tool = Some(PendingToolInfo { tool, command, arguments: input });
                                break;
                            }
                        }
                    }
                }
                match v.pointer("/message/stop_reason").and_then(Value::as_str) {
                    Some("end_turn" | "stop_sequence" | "max_tokens") => {
                        state = "done";
                        break;
                    }
                    Some("tool_use") => {
                        let asks_user = pending_tool.as_ref().is_some_and(|t| matches!(t.tool.as_str(), "AskUserQuestion" | "ExitPlanMode"));
                        state = if asks_user { "waiting" } else { "working" };
                        break;
                    }
                    _ => {
                        state = "working";
                        break;
                    }
                }
            }
            _ => continue,
        }
    }

    // Extract complete last assistant text message (combining chunks that share the final message.id).
    let mut last_msg_id: Option<String> = None;
    let mut parts: Vec<String> = Vec::new();
    for v in parsed.iter().rev() {
        if v["type"].as_str() != Some("assistant") || v["isApiErrorMessage"].as_bool().unwrap_or(false) {
            if !parts.is_empty() {
                break;
            }
            continue;
        }
        let mid = v.pointer("/message/id").and_then(Value::as_str).map(str::to_owned);
        if let Some(ref want) = last_msg_id
            && mid.as_deref() != Some(want.as_str())
        {
            break;
        }
        if let Some(txt) = extract_claude_text(v) {
            if last_msg_id.is_none() {
                last_msg_id = mid;
            }
            parts.push(txt);
            if last_msg_id.is_none() {
                break;
            }
        }
    }
    parts.reverse();
    let last_message = (!parts.is_empty()).then(|| parts.join("\n"));

    TranscriptInspection { state, last_message, error, pending_tool }
}

fn extract_claude_text(v: &Value) -> Option<String> {
    if let Some(s) = v.pointer("/message/content").and_then(Value::as_str) {
        let t = s.trim();
        return (!t.is_empty()).then(|| t.to_owned());
    }
    let arr = v.pointer("/message/content").and_then(Value::as_array)?;
    let mut chunks = Vec::new();
    for b in arr {
        if b["type"].as_str() == Some("text")
            && let Some(s) = b["text"].as_str()
            && !s.trim().is_empty()
        {
            chunks.push(s.trim().to_owned());
        }
    }
    (!chunks.is_empty()).then(|| chunks.join("\n"))
}

fn inspect_agy_transcript(path: &Path) -> TranscriptInspection {
    // Prefer `transcript_full.jsonl` beside `transcript.jsonl` if available so long messages/commands are NEVER truncated!
    let full_path = path.with_file_name("transcript_full.jsonl");
    let read_path = if full_path.exists() { full_path.as_path() } else { path };
    let tail = read_tail_text(read_path, 512 * 1024);

    let mut state: &'static str = "unknown";
    let mut last_message: Option<String> = None;
    let mut error: Option<String> = None;
    let mut pending_tool: Option<PendingToolInfo> = None;
    let mut pending_count = 0usize;

    for line in tail.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
        let ty = v["type"].as_str().unwrap_or("");
        if matches!(ty, "" | "SYSTEM_MESSAGE" | "CHECKPOINT") {
            continue;
        }
        let st = v["status"].as_str().unwrap_or("").to_ascii_uppercase();
        match ty {
            "USER_INPUT" => {
                state = "working";
                pending_count = 0;
                pending_tool = None;
                error = None;
            }
            "PLANNER_RESPONSE" => {
                if let Some(txt) = v["modified_response"].as_str().or_else(|| v["response"].as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                    last_message = Some(txt.to_owned());
                }
                if let Some(calls) = v["tool_calls"].as_array()
                    && !calls.is_empty()
                {
                    pending_count = calls.len();
                    let asking = calls.iter().any(|c| c["name"].as_str() == Some("ask_question"));
                    if let Some(last_call) = calls.last() {
                        let tool = last_call["name"].as_str().unwrap_or("tool").to_owned();
                        let args = last_call.get("arguments").cloned().or_else(|| last_call.get("args").cloned());
                        let command = args.as_ref().and_then(|a| {
                            a.as_str()
                                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                                .unwrap_or_else(|| a.clone())
                                .get("CommandLine")
                                .or_else(|| a.get("command"))
                                .or_else(|| a.get("Path"))
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        });
                        pending_tool = Some(PendingToolInfo { tool, command, arguments: args });
                    }
                    state = if asking { "waiting" } else { "working" };
                } else if st == "ERROR" || st == "FAILED" {
                    state = "error";
                    error = v["error"].as_str().map(str::to_owned).or_else(|| Some("Antigravity turn failed".into()));
                } else {
                    pending_count = 0;
                    pending_tool = None;
                    state = "done";
                }
            }
            "GENERIC" | "TOOL_RESPONSE" => {
                if st != "RUNNING" {
                    pending_count = pending_count.saturating_sub(1);
                    if pending_count == 0 {
                        pending_tool = None;
                    }
                }
                state = "working";
            }
            "ERROR" | "API_ERROR" => {
                state = "error";
                error = v["message"].as_str().or_else(|| v["error"].as_str()).map(str::to_owned).or_else(|| Some("API Error".into()));
            }
            _ => {}
        }
    }

    TranscriptInspection { state, last_message, error, pending_tool }
}

fn inspect_codex_transcript(path: &Path) -> TranscriptInspection {
    let tail = read_tail_text(path, 512 * 1024);
    let mut state: &'static str = "unknown";
    let mut last_message: Option<String> = None;
    let mut error: Option<String> = None;
    let mut pending_tool: Option<PendingToolInfo> = None;
    let mut completed_calls: HashSet<String> = HashSet::new();

    let mut parsed: Vec<Value> = Vec::new();
    for line in tail.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            parsed.push(v);
        }
    }

    for v in &parsed {
        if v["type"].as_str() == Some("response_item") {
            let p = &v["payload"];
            if matches!(p["type"].as_str(), Some("function_call_output" | "custom_tool_call_output"))
                && let Some(cid) = p["call_id"].as_str()
            {
                completed_calls.insert(cid.to_owned());
            }
            if p["type"].as_str() == Some("message") && p["role"].as_str() == Some("assistant") {
                if let Some(arr) = p["content"].as_array() {
                    let texts: Vec<&str> = arr.iter().filter_map(|c| c["text"].as_str()).collect();
                    if !texts.is_empty() {
                        last_message = Some(texts.join("\n"));
                    }
                }
            }
        } else if v["type"].as_str() == Some("event_msg") {
            let p = &v["payload"];
            if p["type"].as_str() == Some("agent_message")
                && let Some(m) = p["message"].as_str().filter(|s| !s.trim().is_empty())
            {
                last_message = Some(m.trim().to_owned());
            }
        }
    }

    for v in parsed.iter().rev() {
        match v["type"].as_str().unwrap_or("") {
            "event_msg" => {
                let p = &v["payload"];
                match p["type"].as_str().unwrap_or("") {
                    "task_failed" | "error" => {
                        state = "error";
                        error = p["message"].as_str().or_else(|| p["error"].as_str()).map(str::to_owned);
                        break;
                    }
                    "exec_approval_request" | "apply_patch_approval_request" | "request_user_input" => {
                        state = "waiting";
                        break;
                    }
                    "task_complete" | "turn_aborted" => {
                        state = "done";
                        break;
                    }
                    "task_started" => {
                        state = "working";
                        break;
                    }
                    _ => {}
                }
            }
            "response_item" => {
                let p = &v["payload"];
                match p["type"].as_str().unwrap_or("") {
                    "function_call" | "custom_tool_call" => {
                        let cid = p["call_id"].as_str().unwrap_or("");
                        if cid.is_empty() || !completed_calls.contains(cid) {
                            let tool = p["name"].as_str().unwrap_or("tool").to_owned();
                            let args_val = p["arguments"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()).or_else(|| p.get("arguments").cloned());
                            let command = args_val.as_ref().and_then(|a| a["cmd"].as_str().or_else(|| a["command"].as_str())).map(str::to_owned);
                            pending_tool = Some(PendingToolInfo { tool, command, arguments: args_val });
                            state = "working";
                            break;
                        }
                    }
                    "message" if p["role"].as_str() == Some("user") => {
                        state = "working";
                        break;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    TranscriptInspection { state, last_message, error, pending_tool }
}

fn run_inspect() -> i32 {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() {
        println!("{}", json!({ "ok": false, "error": "failed to read stdin" }));
        return 1;
    }
    let req: InspectBatchRequest = match serde_json::from_str(input.trim()) {
        Ok(r) => r,
        Err(e) => {
            println!("{}", json!({ "ok": false, "error": format!("invalid json: {e}") }));
            return 2;
        }
    };

    let places = Places::here();
    let procs = all_procs();
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut by_ppid: HashMap<i32, Vec<&Proc>> = HashMap::new();
    for p in &procs {
        by_ppid.entry(p.ppid).or_default().push(p);
    }
    let need_codex = req.items.iter().any(|i| i.cli == "codex");
    let rollouts = if need_codex { codex_rollouts(&places) } else { HashMap::new() };

    let mut results = Vec::with_capacity(req.items.len());
    for item in &req.items {
        let found = find_agent_proc(item, &by_pid, &by_ppid);
        let (pid, start, proc_cwd, args) = match found {
            Some((p, args)) => (Some(p.pid), start_ticks(p.pid), read_cwd(p.pid), args),
            None => (None, 0, String::new(), Vec::new()),
        };
        let cwd = if !proc_cwd.is_empty() { proc_cwd } else { item.cwd.clone() };
        let (cli_session_id, transcript_path) =
            resolve_session_and_transcript(&places, &item.cli, pid, start, &cwd, &args, item.known_session_id.as_deref(), item.known_transcript.as_deref(), item.started_at, &rollouts);

        let inspected = match transcript_path.as_deref() {
            Some(p) if item.cli == "claude" => inspect_claude_transcript(p),
            Some(p) if item.cli == "agy" => inspect_agy_transcript(p),
            Some(p) if item.cli == "codex" => inspect_codex_transcript(p),
            _ => TranscriptInspection { state: "unknown", last_message: None, error: None, pending_tool: None },
        };

        let background_shells = pid.map(|p| find_background_shells(p, &by_ppid)).unwrap_or_default();
        let has_active_task = background_shells.iter().any(|s| s.kind == "task");
        let waiting_on = if pid.is_none() {
            None
        } else if has_active_task {
            Some("own_run")
        } else if matches!(inspected.state, "done" | "waiting") {
            Some("someone")
        } else {
            None
        };

        results.push(InspectItemResult {
            id: item.id.clone(),
            pid,
            alive: pid.is_some(),
            cli_session_id,
            transcript_path: transcript_path.map(|p| p.display().to_string()),
            last_message: inspected.last_message,
            transcript_state: inspected.state,
            transcript_error: inspected.error,
            pending_tool: inspected.pending_tool,
            background_shells,
            waiting_on,
        });
    }

    println!("{}", json!({ "ok": true, "items": results }));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_claude_paths_is_atomic_and_preserves_existing_keys() {
        let dir = PathBuf::from(format!("/var/tmp/olo-trust-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        let cfg = dir.join(".claude.json");
        std::fs::write(&cfg, r#"{"numStartups":42,"projects":{"/existing":{"hasTrustDialogAccepted":true,"allowedTools":["Read"]}}}"#).unwrap();
        let repo = dir.join("repo").display().to_string();
        let trusted = trust_claude_paths(&cfg, &[repo.clone()]).unwrap();
        assert!(trusted.contains(&repo));
        let parsed: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(parsed["numStartups"], 42);
        assert_eq!(parsed["projects"]["/existing"]["allowedTools"][0], "Read");
        assert_eq!(parsed["projects"][&repo]["hasTrustDialogAccepted"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspects_claude_and_agy_transcripts_for_full_message_pending_tool_and_api_error() {
        let dir = PathBuf::from(format!("/var/tmp/olo-inspect-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. Claude multi-chunk end_turn message
        let claude = dir.join("c1.jsonl");
        std::fs::write(
            &claude,
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Fix it\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"stop_reason\":null,\"content\":[{\"type\":\"text\",\"text\":\"Line 1\"}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"stop_reason\":\"end_turn\",\"content\":[{\"type\":\"text\",\"text\":\"DONE: fixed all 4 tests\"}]}}\n",
            ),
        )
        .unwrap();
        let r = inspect_claude_transcript(&claude);
        assert_eq!(r.state, "done");
        assert_eq!(r.last_message.as_deref(), Some("Line 1\nDONE: fixed all 4 tests"));

        // 2. Claude API error mid-response
        let claude_err = dir.join("c2.jsonl");
        std::fs::write(&claude_err, "{\"type\":\"assistant\",\"isApiErrorMessage\":true,\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"API Error: Connection lost mid-response\"}]}}\n")
            .unwrap();
        let r_err = inspect_claude_transcript(&claude_err);
        assert_eq!(r_err.state, "error");
        assert!(r_err.error.as_deref().unwrap_or("").contains("Connection lost mid-response"));

        // 3. Claude pending tool_use carries full untruncated command
        let claude_tool = dir.join("c3.jsonl");
        std::fs::write(
            &claude_tool,
            "{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"stop_reason\":\"tool_use\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test --workspace -- --nocapture\"}}]}}\n",
        )
        .unwrap();
        let r_tool = inspect_claude_transcript(&claude_tool);
        assert_eq!(r_tool.state, "working");
        assert_eq!(r_tool.pending_tool.as_ref().unwrap().tool, "Bash");
        assert_eq!(r_tool.pending_tool.as_ref().unwrap().command.as_deref(), Some("cargo test --workspace -- --nocapture"));

        // 4. Agy prefers transcript_full.jsonl when present
        let agy_compact = dir.join("transcript.jsonl");
        let agy_full = dir.join("transcript_full.jsonl");
        std::fs::write(&agy_compact, "{\"type\":\"PLANNER_RESPONSE\",\"response\":\"short...\"}\n").unwrap();
        std::fs::write(&agy_full, "{\"type\":\"PLANNER_RESPONSE\",\"response\":\"DONE: full untruncated antigravity reply\\nwith multiple lines\"}\n").unwrap();
        let r_agy = inspect_agy_transcript(&agy_compact);
        assert_eq!(r_agy.state, "done");
        assert_eq!(r_agy.last_message.as_deref(), Some("DONE: full untruncated antigravity reply\nwith multiple lines"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
