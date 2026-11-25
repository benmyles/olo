//! `olo-rex-link activity watch|once|tail`: zero-polling host-side discovery
//! of nested subagents (native Claude Code, Codex, and Antigravity CLI
//! subagents plus shell- and tmux-launched child agents) and live streaming
//! transcript tailing over `inotify` + `pidfd` + `poll`.
//!
//! Sleeps indefinitely in `poll(..., -1)` whenever nothing changes (zero CPU
//! at rest), and exits immediately when stdin closes (when Rex hides Activity
//! or closes a transcript view).

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs::{File, Metadata};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::moving::{Proc, all_procs};
use crate::panes::{Places, agent_of, argv_session, claude_record, codex_rollouts, codex_threads, found_in, process_json, project_name, sessions, sessions_in, start_ticks, started_at, uuid};

const USAGE: &str = "usage: olo-rex-link activity <watch|once|tail PATH [--max-initial BYTES]>";

/// Coalesce filesystem/process bursts over this window before rescanning.
const SETTLE_MS: u64 = 150;
/// Default maximum tail read on initial open (1 MiB, snapped to newline boundary).
const DEFAULT_MAX_INITIAL: u64 = 1024 * 1024;
/// Maximum lines per NDJSON batch frame from `activity tail`.
const TAIL_BATCH_LINES: usize = 256;

const INOTIFY_DIR_MASK: u32 = libc::IN_CREATE | libc::IN_DELETE | libc::IN_MODIFY | libc::IN_CLOSE_WRITE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF;

const INOTIFY_FILE_MASK: u32 = libc::IN_MODIFY | libc::IN_CLOSE_WRITE | libc::IN_ATTRIB | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF;

struct OwnedFd(i32);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // SAFETY: `self.0` is an exclusively owned file descriptor.
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PaneQuery {
    block_id: String,
    pid: i32,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    screens: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct WatchInput {
    #[serde(default)]
    panes: Option<Vec<PaneQuery>>,
    #[serde(default)]
    refresh: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct TmuxTarget {
    socket: String,
    session: String,
    target: String,
    cols: u16,
    rows: u16,
    pty_cols: u16,
    pty_rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct SubagentInfo {
    id: String,
    kind: String,
    origin: String,
    name: String,
    description: String,
    state: String,
    pid: Option<i32>,
    transcript: Option<String>,
    tmux: Option<TmuxTarget>,
    started_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PaneActivity {
    block_id: String,
    agent: Option<String>,
    pid: Option<i32>,
    session_id: Option<String>,
    transcript: Option<String>,
    subagents: Vec<SubagentInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ActivitySnapshot {
    ok: bool,
    panes: Vec<PaneActivity>,
}

pub fn run(args: Vec<String>) -> ! {
    let mut it = args.into_iter();
    let Some(sub) = it.next() else {
        eprintln!("olo-rex-link activity: missing subcommand\n{USAGE}");
        std::process::exit(2);
    };
    let rest: Vec<String> = it.collect();
    let code = match sub.as_str() {
        "watch" => run_watch(false),
        "once" => run_watch(true),
        "tail" => run_tail(&rest),
        other => {
            eprintln!("olo-rex-link activity: unknown subcommand {other}\n{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// Process environment, tmux discovery, and transcript resolution
// ---------------------------------------------------------------------------

fn read_environ(pid: i32) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/environ")) else { return out };
    for entry in bytes.split(|&b| b == 0) {
        if let Some(pos) = entry.iter().position(|&b| b == b'=') {
            let k = String::from_utf8_lossy(&entry[..pos]).into_owned();
            let v = String::from_utf8_lossy(&entry[pos + 1..]).into_owned();
            out.insert(k, v);
        }
    }
    out
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

fn system_time_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn file_mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).map_or(0, system_time_ms)
}

fn modified_within(p: &Path, secs: u64) -> bool {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|d| d.as_secs() <= secs)
}

fn file_modified_since(p: &Path, since: Option<SystemTime>) -> bool {
    let Some(s) = since else { return false };
    let Ok(mtime) = std::fs::metadata(p).and_then(|m| m.modified()) else { return false };
    let grace = s.checked_sub(Duration::from_secs(5)).unwrap_or(s);
    mtime >= grace
}

/// Read the trailing `max_bytes` of a file (aligned to the next newline when
/// starting mid-file) for fast O(1) tail status inspection.
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

/// Parse an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff]Z`) into epoch ms.
fn parse_iso_ms(ts: &str) -> Option<u64> {
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: i64 = ts.get(0..4)?.parse().ok()?;
    let mo: i64 = ts.get(5..7)?.parse().ok()?;
    let d: i64 = ts.get(8..10)?.parse().ok()?;
    let hh: i64 = ts.get(11..13)?.parse().ok()?;
    let mm: i64 = ts.get(14..16)?.parse().ok()?;
    let ss: i64 = ts.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Howard Hinnant's civil-to-days algorithm (UTC).
    let y_adj = y - i64::from(mo <= 2);
    let era = y_adj.div_euclid(400);
    let yoe = y_adj.rem_euclid(400);
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    if secs < 0 {
        return None;
    }
    let mut ms = (secs as u64) * 1000;
    if b.get(19) == Some(&b'.') {
        let frac: String = ts[20..].chars().take_while(|c| c.is_ascii_digit()).take(3).collect();
        if let Ok(mut f) = frac.parse::<u64>() {
            match frac.len() {
                1 => f *= 100,
                2 => f *= 10,
                _ => {}
            }
            ms += f;
        }
    }
    Some(ms)
}

/// Extract the earliest timestamp from the first few JSONL lines of a transcript.
fn first_timestamp_ms(path: &Path) -> Option<u64> {
    let f = File::open(path).ok()?;
    let mut reader = io::BufReader::new(f);
    let mut line = String::new();
    for _ in 0..8 {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        for key in ["timestamp", "created_at"] {
            if let Some(ts) = v[key].as_str()
                && let Some(ms) = parse_iso_ms(ts)
            {
                return Some(ms);
            }
        }
        if let Some(ts) = v.pointer("/payload/timestamp").and_then(|s| s.as_str())
            && let Some(ms) = parse_iso_ms(ts)
        {
            return Some(ms);
        }
    }
    None
}

/// Detect an agent process from argv/comm/exe OR explicit `OLO_ACTIVITY_AGENT`
/// env var (used by stand-ins in automated rig tests alongside real agents).
fn detect_agent(pid: i32, comm: &str, env: &HashMap<String, String>) -> Option<(String, Vec<String>)> {
    let argv = read_argv(pid);
    let exe = read_exe(pid);
    if let Some((kind, args)) = agent_of(&argv, comm, &exe) {
        return Some((kind.to_owned(), args));
    }
    if let Some(kind) = env.get("OLO_ACTIVITY_AGENT").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return Some((kind.to_owned(), argv));
    }
    None
}

/// Resolve an agent process's transcript file path on disk, if available.
fn resolve_transcript(
    places: &Places,
    kind: &str,
    pid: i32,
    start: u64,
    cwd: &str,
    args: &[String],
    session_id: Option<&str>,
    env: &HashMap<String, String>,
    rollouts: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    if let Some(explicit) = env.get("OLO_ACTIVITY_TRANSCRIPT").filter(|s| !s.is_empty()) {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return Some(p);
        }
    }
    let sid = session_id.map(str::to_owned).or_else(|| if kind == "claude" { claude_record(places, pid, start) } else { None }).or_else(|| argv_session(kind, args));
    match kind {
        "claude" => {
            let proj = places.claude.join("projects").join(project_name(cwd));
            if let Some(id) = &sid {
                let p = proj.join(format!("{id}.jsonl"));
                if p.exists() {
                    return Some(p);
                }
            }
            let since = started_at(start).map(|t| t.checked_sub(Duration::from_secs(2)).unwrap_or(t));
            sessions_in(&proj, ".jsonl", since).into_iter().find_map(|c| c.file)
        }
        "codex" => {
            if let Some(id) = &sid
                && let Some(p) = rollouts.get(id)
                && p.exists()
            {
                return Some(p.clone());
            }
            // Check open fds for a rollout jsonl.
            if let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
                for entry in rd.flatten() {
                    if let Ok(target) = std::fs::read_link(entry.path())
                        && target.extension().is_some_and(|e| e == "jsonl")
                        && target.starts_with(places.codex.join("sessions"))
                    {
                        return Some(target);
                    }
                }
            }
            None
        }
        "agy" => {
            if let Some(id) = &sid {
                let p = places.agy_brain().join(id).join(".system_generated/logs/transcript.jsonl");
                if p.exists() {
                    return Some(p);
                }
            }
            // Check open conversation .db fd -> brain/<id>/.system_generated/logs/transcript.jsonl.
            if let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
                for entry in rd.flatten() {
                    if let Ok(target) = std::fs::read_link(entry.path())
                        && target.starts_with(places.agy())
                        && let Some(name) = target.file_name().and_then(|n| n.to_str())
                    {
                        for suf in [".db", ".db-wal", ".db-shm"] {
                            if let Some(stem) = name.strip_suffix(suf)
                                && uuid(stem)
                            {
                                let p = places.agy_brain().join(stem).join(".system_generated/logs/transcript.jsonl");
                                if p.exists() {
                                    return Some(p);
                                }
                            }
                        }
                    }
                }
            }
            // Fallback: newest brain transcript modified since process started.
            let since = started_at(start).map(|t| t.checked_sub(Duration::from_secs(2)).unwrap_or(t));
            newest_agy_transcript(&places.agy_brain(), since)
        }
        _ => None,
    }
}

fn newest_agy_transcript(brain: &Path, since: Option<SystemTime>) -> Option<PathBuf> {
    let rd = std::fs::read_dir(brain).ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
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
        if best.as_ref().is_none_or(|(b, _)| mtime > *b) {
            best = Some((mtime, p));
        }
    }
    best.map(|(_, p)| p)
}

/// First short snippet of a user prompt in a JSONL file (for fallback descriptions).
fn first_prompt_snippet(path: &Path) -> String {
    let Ok(f) = File::open(path) else { return String::new() };
    let mut reader = io::BufReader::new(f);
    let mut line = String::new();
    for _ in 0..8 {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let candidate =
            v.pointer("/message/content").and_then(|c| c.as_str()).or_else(|| v.pointer("/content").and_then(|c| c.as_str())).or_else(|| v.pointer("/payload/content/0/text").and_then(|c| c.as_str()));
        if let Some(s) = candidate {
            let trimmed = strip_xml_request(s);
            let one: String = trimmed.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(96).collect();
            if !one.is_empty() {
                return one;
            }
        }
    }
    String::new()
}

fn strip_xml_request(s: &str) -> &str {
    if let Some(start) = s.find("<USER_REQUEST>") {
        let rest = &s[start + "<USER_REQUEST>".len()..];
        if let Some(end) = rest.find("</USER_REQUEST>") {
            return rest[..end].trim();
        }
        return rest.trim();
    }
    s.trim()
}

// ---------------------------------------------------------------------------
// Native subagent discovery: Claude Code, Codex, Antigravity CLI
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailState {
    Active,
    Waiting,
    Done,
    Failed,
    Unknown,
}

/// Extract XML-like tag text (`<tag>VALUE</tag>`) without allocations beyond the match.
fn extract_xml_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim())
}

/// Scan the parent Claude Code transcript for `<task-notification>` completion
/// notices (`<task-id>AGENT_ID</task-id>` + `<status>completed|failed|killed</status>`).
fn claude_parent_task_states(parent_jsonl: &Path) -> HashMap<String, &'static str> {
    let mut out = HashMap::new();
    let Ok(f) = File::open(parent_jsonl) else { return out };
    let reader = io::BufReader::new(f);
    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("<task-notification>") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let text = v["content"].as_str().or_else(|| v.pointer("/attachment/prompt").and_then(|s| s.as_str())).or_else(|| v.pointer("/message/content").and_then(|s| s.as_str()));
        let Some(t) = text else { continue };
        if let Some(tid) = extract_xml_tag(t, "task-id")
            && !tid.is_empty()
        {
            let status = extract_xml_tag(t, "status").unwrap_or("completed").to_ascii_lowercase();
            let mapped = if matches!(status.as_str(), "failed" | "error") { "failed" } else { "done" };
            out.insert(tid.to_owned(), mapped);
        }
    }
    out
}

/// Inspect the tail of a Claude Code JSONL transcript to determine whether a
/// subagent is currently executing a tool/API turn, waiting for user input,
/// finished (`end_turn`), or errored — instead of guessing from file mtime.
fn claude_tail_state(jsonl: &Path) -> TailState {
    let tail = read_tail_text(jsonl, 64 * 1024);
    for line in tail.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else { continue };
        let ty = v["type"].as_str().unwrap_or("");
        match ty {
            // Skip internal telemetry & request blobs that don't reflect turn completion.
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
            "api-request" => return TailState::Active,
            "system" => {
                let sub = v["subtype"].as_str().unwrap_or("");
                if sub == "api_error" || v.get("error").is_some_and(|e| !e.is_null()) {
                    return TailState::Failed;
                }
                continue;
            }
            "user" => {
                // A user turn (initial prompt or tool_result) at the very tail means
                // the model is currently thinking / generating its next response.
                return TailState::Active;
            }
            "assistant" => {
                if v["isApiErrorMessage"].as_bool().unwrap_or(false) {
                    return TailState::Failed;
                }
                let stop = v.pointer("/message/stop_reason").and_then(|s| s.as_str());
                match stop {
                    Some("end_turn" | "stop_sequence" | "max_tokens") => return TailState::Done,
                    Some("tool_use") => {
                        let asks_user = v
                            .pointer("/message/content")
                            .and_then(|c| c.as_array())
                            .is_some_and(|arr| arr.iter().any(|b| b["type"].as_str() == Some("tool_use") && matches!(b["name"].as_str(), Some("AskUserQuestion" | "ExitPlanMode"))));
                        return if asks_user { TailState::Waiting } else { TailState::Active };
                    }
                    None => {
                        // Streaming chunk or mid-turn assistant entry without stop_reason yet.
                        let has_tool = v.pointer("/message/content").and_then(|c| c.as_array()).is_some_and(|arr| arr.iter().any(|b| b["type"].as_str() == Some("tool_use")));
                        return if has_tool { TailState::Active } else { TailState::Active };
                    }
                    Some(_) => return TailState::Active,
                }
            }
            _ => continue,
        }
    }
    TailState::Unknown
}

fn resolve_claude_state(jsonl: &Path, meta_finished: Option<bool>, parent_status: Option<&'static str>, parent_started: Option<SystemTime>) -> &'static str {
    if let Some(st) = parent_status {
        return st;
    }
    let tail = claude_tail_state(jsonl);
    if let Some(fin) = meta_finished {
        if fin {
            return if tail == TailState::Failed { "failed" } else { "done" };
        }
        return match tail {
            TailState::Waiting => "waiting",
            TailState::Failed => "failed",
            _ => "working",
        };
    }
    // Real Claude Code omits `"finished"` while a subagent is running and writes
    // `"finished": true` once it completes. Guard against ancient orphaned files
    // from an earlier crashed session by checking whether the file was touched
    // during the live parent process's run (or recently).
    let live_window = file_modified_since(jsonl, parent_started) || modified_within(jsonl, 1800);
    match tail {
        TailState::Failed => "failed",
        TailState::Done => "done",
        TailState::Waiting => {
            if live_window {
                "waiting"
            } else {
                "done"
            }
        }
        TailState::Active => {
            if live_window {
                "working"
            } else {
                "done"
            }
        }
        TailState::Unknown => {
            if modified_within(jsonl, 120) {
                "working"
            } else {
                "done"
            }
        }
    }
}

fn discover_claude_subagents(places: &Places, cwd: &str, session_id: &str, parent_started: Option<SystemTime>, watch_dirs: &mut Vec<PathBuf>) -> Vec<SubagentInfo> {
    let proj_dir = places.claude.join("projects").join(project_name(cwd));
    let sub_dir = proj_dir.join(session_id).join("subagents");
    if proj_dir.is_dir() {
        watch_dirs.push(proj_dir.clone());
    }
    let sess_dir = proj_dir.join(session_id);
    if sess_dir.is_dir() {
        watch_dirs.push(sess_dir);
    }
    if !sub_dir.is_dir() {
        return Vec::new();
    }
    watch_dirs.push(sub_dir.clone());
    let parent_jsonl = proj_dir.join(format!("{session_id}.jsonl"));
    let parent_states = claude_parent_task_states(&parent_jsonl);

    let Ok(rd) = std::fs::read_dir(&sub_dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(fname) = name.to_str() else { continue };
        let Some(agent_id) = fname.strip_prefix("agent-").and_then(|s| s.strip_suffix(".jsonl")) else { continue };
        let jsonl = entry.path();
        let meta_path = sub_dir.join(format!("agent-{agent_id}.meta.json"));
        let meta: Option<serde_json::Value> = std::fs::read(&meta_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let agent_type = meta.as_ref().and_then(|m| m["agentType"].as_str()).unwrap_or("Subagent");
        let desc = meta.as_ref().and_then(|m| m["description"].as_str()).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_else(|| first_prompt_snippet(&jsonl));
        let finished = meta.as_ref().and_then(|m| m["finished"].as_bool());
        let state = resolve_claude_state(&jsonl, finished, parent_states.get(agent_id).copied(), parent_started);
        let started_ms = first_timestamp_ms(&jsonl).or_else(|| std::fs::metadata(&jsonl).ok().and_then(|m| m.created().ok().or_else(|| m.modified().ok())).map(system_time_ms)).unwrap_or(0);
        out.push(SubagentInfo {
            id: format!("claude-sub:{agent_id}"),
            kind: "claude".into(),
            origin: "native".into(),
            name: agent_type.to_owned(),
            description: desc,
            state: state.into(),
            pid: None,
            transcript: Some(jsonl.display().to_string()),
            tmux: None,
            started_ms,
        });
    }
    out.sort_by(|a, b| a.started_ms.cmp(&b.started_ms).then_with(|| a.id.cmp(&b.id)));
    out
}

fn discover_codex_subagents(parent_session_id: &str, parent_started: Option<SystemTime>, rollouts: &HashMap<String, PathBuf>, watch_dirs: &mut Vec<PathBuf>) -> Vec<SubagentInfo> {
    let mut out = Vec::new();
    for (tid, path) in rollouts {
        if tid == parent_session_id {
            continue;
        }
        let Ok(f) = File::open(path) else { continue };
        let mut reader = io::BufReader::new(f);
        let mut first = String::new();
        if reader.read_line(&mut first).unwrap_or(0) == 0 {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) else { continue };
        if v["type"].as_str() != Some("session_meta") {
            continue;
        }
        let payload = &v["payload"];
        let parent = payload["parent_thread_id"].as_str().or_else(|| payload.pointer("/source/subagent/thread_spawn/parent_thread_id").and_then(|x| x.as_str())).or_else(|| {
            let s = payload["session_id"].as_str()?;
            (s != tid).then_some(s)
        });
        if parent != Some(parent_session_id) {
            continue;
        }
        if let Some(dir) = path.parent() {
            watch_dirs.push(dir.to_path_buf());
        }
        let nickname = payload["agent_nickname"]
            .as_str()
            .or_else(|| payload.pointer("/source/subagent/thread_spawn/agent_nickname").and_then(|x| x.as_str()))
            .or_else(|| payload.pointer("/source/subagent/thread_spawn/agent_role").and_then(|x| x.as_str()))
            .or_else(|| payload.pointer("/source/subagent/other").and_then(|x| x.as_str()))
            .or_else(|| payload["thread_source"].as_str())
            .unwrap_or("Codex subagent");
        let desc = payload
            .pointer("/source/subagent/thread_spawn/agent_path")
            .and_then(|x| x.as_str())
            .map(|p| p.trim_start_matches("/root/").to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| first_prompt_snippet(path));
        let state = codex_rollout_state(path, parent_started);
        let started_ms = first_timestamp_ms(path).unwrap_or_else(|| file_mtime_ms(path));
        out.push(SubagentInfo {
            id: format!("codex-sub:{tid}"),
            kind: "codex".into(),
            origin: "native".into(),
            name: nickname.to_owned(),
            description: desc,
            state: state.into(),
            pid: None,
            transcript: Some(path.display().to_string()),
            tmux: None,
            started_ms,
        });
    }
    out.sort_by(|a, b| a.started_ms.cmp(&b.started_ms).then_with(|| a.id.cmp(&b.id)));
    out
}

fn codex_rollout_state(path: &Path, parent_started: Option<SystemTime>) -> &'static str {
    let tail = read_tail_text(path, 64 * 1024);
    let live_window = file_modified_since(path, parent_started) || modified_within(path, 1800);
    let mut completed_calls: HashSet<String> = HashSet::new();
    let mut saw_any = false;
    for line in tail.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else { continue };
        saw_any = true;
        match v["type"].as_str().unwrap_or("") {
            "event_msg" => {
                let et = v.pointer("/payload/type").and_then(|s| s.as_str()).unwrap_or("");
                match et {
                    "task_failed" | "error" => return "failed",
                    "exec_approval_request" | "apply_patch_approval_request" | "request_user_input" => {
                        return if live_window { "waiting" } else { "done" };
                    }
                    "task_complete" | "turn_aborted" => return "done",
                    "task_started" => return if live_window { "working" } else { "done" },
                    _ => {}
                }
            }
            "response_item" => {
                let payload = &v["payload"];
                match payload["type"].as_str().unwrap_or("") {
                    "function_call_output" | "custom_tool_call_output" => {
                        if let Some(cid) = payload["call_id"].as_str() {
                            completed_calls.insert(cid.to_owned());
                        }
                    }
                    "function_call" | "custom_tool_call" => {
                        let cid = payload["call_id"].as_str().unwrap_or("");
                        if cid.is_empty() || !completed_calls.contains(cid) {
                            return if live_window { "working" } else { "done" };
                        }
                    }
                    "message" if payload["role"].as_str() == Some("user") => {
                        return if live_window { "working" } else { "done" };
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if saw_any && (file_modified_since(path, parent_started) || modified_within(path, 120)) { "working" } else { "done" }
}

fn agy_transcript_state(path: &Path, parent_started: Option<SystemTime>, killed_by_parent: bool) -> &'static str {
    if killed_by_parent {
        return "done";
    }
    let tail = read_tail_text(path, 64 * 1024);
    let live_window = file_modified_since(path, parent_started) || modified_within(path, 1800);
    let mut pending_tools: usize = 0;
    let mut asking = false;
    let mut last_type = String::new();
    let mut last_status = String::new();
    for line in tail.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else { continue };
        let ty = v["type"].as_str().unwrap_or("");
        if matches!(ty, "" | "SYSTEM_MESSAGE" | "CHECKPOINT") {
            continue;
        }
        let st = v["status"].as_str().unwrap_or("").to_ascii_uppercase();
        last_type = ty.to_owned();
        last_status = st.clone();
        match ty {
            "USER_INPUT" => {
                pending_tools = 0;
                asking = false;
            }
            "PLANNER_RESPONSE" => {
                if let Some(calls) = v["tool_calls"].as_array()
                    && !calls.is_empty()
                {
                    pending_tools = calls.len();
                    asking = calls.iter().any(|c| c["name"].as_str() == Some("ask_question"));
                } else {
                    pending_tools = 0;
                    asking = false;
                }
            }
            "GENERIC" | "TOOL_RESPONSE" => {
                if st != "RUNNING" {
                    pending_tools = pending_tools.saturating_sub(1);
                }
            }
            _ => {}
        }
    }
    if asking && live_window {
        return "waiting";
    }
    match last_type.as_str() {
        "PLANNER_RESPONSE" => {
            if matches!(last_status.as_str(), "ERROR" | "FAILED") {
                "failed"
            } else if pending_tools > 0 {
                if live_window { "working" } else { "done" }
            } else if last_status == "DONE" {
                "done"
            } else if live_window {
                "working"
            } else {
                "done"
            }
        }
        "GENERIC" | "TOOL_RESPONSE" => {
            if last_status == "RUNNING" {
                if live_window { "working" } else { "done" }
            } else if matches!(last_status.as_str(), "ERROR" | "FAILED") {
                if modified_within(path, 120) { "working" } else { "failed" }
            } else if live_window {
                // Tool result just returned; planner is generating the next step.
                "working"
            } else {
                "done"
            }
        }
        "USER_INPUT" => {
            if live_window {
                "working"
            } else {
                "done"
            }
        }
        _ => {
            if modified_within(path, 60) {
                "working"
            } else {
                "done"
            }
        }
    }
}

/// Infer live state for a running tmux- or shell-launched child agent from its
/// transcript when available; defaults to `"working"` because the PID is alive.
fn infer_live_agent_state(kind: &str, transcript: Option<&Path>, started: Option<SystemTime>) -> &'static str {
    let Some(path) = transcript else { return "working" };
    match kind {
        "claude" => resolve_claude_state(path, None, None, started),
        "codex" => codex_rollout_state(path, started),
        "agy" => agy_transcript_state(path, started, false),
        _ => "working",
    }
}

fn discover_agy_subagents(places: &Places, parent_transcript: &Path, parent_started: Option<SystemTime>, watch_dirs: &mut Vec<PathBuf>) -> Vec<SubagentInfo> {
    let Ok(f) = File::open(parent_transcript) else { return Vec::new() };
    let reader = io::BufReader::new(f);
    let mut pending_roles: Vec<(String, String)> = Vec::new();
    let mut found: Vec<(String, String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut killed: HashSet<String> = HashSet::new();

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("invoke_subagent") && !line.contains("manage_subagents") && !line.contains("conversationId") && !line.contains("conversation_id") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if let Some(calls) = v["tool_calls"].as_array() {
            for call in calls {
                match call["name"].as_str() {
                    Some("invoke_subagent") => {
                        let role = clean_agy_arg(call.pointer("/args/Role").and_then(|s| s.as_str()))
                            .or_else(|| clean_agy_arg(call.pointer("/args/TypeName").and_then(|s| s.as_str())))
                            .unwrap_or_else(|| "Subagent".into());
                        let prompt = clean_agy_arg(call.pointer("/args/Prompt").and_then(|s| s.as_str())).unwrap_or_default();
                        let desc: String = prompt.lines().next().unwrap_or("").chars().take(96).collect();
                        pending_roles.push((role, desc));
                    }
                    Some("manage_subagents") => {
                        let act = clean_agy_arg(call.pointer("/args/Action").and_then(|s| s.as_str())).unwrap_or_default();
                        if act == "kill_all" {
                            for (id, _, _) in &found {
                                killed.insert(id.clone());
                            }
                        } else if act == "kill" {
                            if let Some(ids_raw) = call.pointer("/args/ConversationIds") {
                                for id in extract_uuids(&ids_raw.to_string()) {
                                    killed.insert(id);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(content) = v["content"].as_str() {
            for id in extract_uuids(content) {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let child_path = places.agy_brain().join(&id).join(".system_generated/logs/transcript.jsonl");
                if child_path.exists() {
                    let (role, desc) = if !pending_roles.is_empty() { pending_roles.remove(0) } else { ("Subagent".into(), first_prompt_snippet(&child_path)) };
                    found.push((id, role, desc));
                }
            }
        }
    }

    let mut out = Vec::new();
    for (id, role, desc) in found {
        let log_dir = places.agy_brain().join(&id).join(".system_generated/logs");
        let child_path = log_dir.join("transcript.jsonl");
        if log_dir.is_dir() {
            watch_dirs.push(log_dir);
        }
        let state = agy_transcript_state(&child_path, parent_started, killed.contains(&id));
        let started_ms = first_timestamp_ms(&child_path).unwrap_or_else(|| file_mtime_ms(&child_path));
        out.push(SubagentInfo {
            id: format!("agy-sub:{id}"),
            kind: "agy".into(),
            origin: "native".into(),
            name: role,
            description: desc,
            state: state.into(),
            pid: None,
            transcript: Some(child_path.display().to_string()),
            tmux: None,
            started_ms,
        });
    }
    out
}

fn clean_agy_arg(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 { serde_json::from_str::<String>(s).ok().or_else(|| Some(s[1..s.len() - 1].to_owned())) } else { Some(s.to_owned()) }
}

fn extract_uuids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    if bytes.len() < 36 {
        return out;
    }
    let mut i = 0;
    while i + 36 <= bytes.len() {
        if bytes[i + 8] == b'-' && bytes[i + 13] == b'-' && bytes[i + 18] == b'-' && bytes[i + 23] == b'-' {
            if let Ok(cand) = std::str::from_utf8(&bytes[i..i + 36])
                && uuid(cand)
            {
                out.push(cand.to_owned());
                i += 36;
                continue;
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Shell-launched and tmux-launched child agents
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct TmuxPaneEntry {
    socket: String,
    session: String,
    target: String,
    pane_id: String,
    pane_pid: i32,
    cols: u16,
    rows: u16,
    pty_cols: u16,
    pty_rows: u16,
    cwd: String,
    title: String,
    server_pid: i32,
}

fn my_uid() -> u32 {
    // SAFETY: pure getter.
    unsafe { libc::geteuid() }
}

/// Discover all live tmux sockets belonging to this user (standard `/tmp/tmux-<uid>/*`,
/// `$TMUX_TMPDIR/tmux-<uid>/*`, plus any `-S PATH` or `TMUX=PATH,...` in process envs/argvs).
fn discover_tmux_sockets(procs: &[Proc], watch_dirs: &mut Vec<PathBuf>) -> Vec<PathBuf> {
    let uid = my_uid();
    let mut dirs = vec![PathBuf::from(format!("/tmp/tmux-{uid}"))];
    if let Ok(td) = std::env::var("TMUX_TMPDIR")
        && !td.is_empty()
    {
        dirs.push(PathBuf::from(format!("{td}/tmux-{uid}")));
    }
    let mut sockets: HashSet<PathBuf> = HashSet::new();

    for p in procs {
        let env = read_environ(p.pid);
        if let Some(t) = env.get("TMUX")
            && let Some(sock) = t.split(',').next()
            && !sock.is_empty()
        {
            let sp = PathBuf::from(sock);
            if sp.exists() {
                if let Some(parent) = sp.parent() {
                    dirs.push(parent.to_path_buf());
                }
                sockets.insert(sp);
            }
        }
        if p.comm == "tmux" || p.comm.starts_with("tmux:") {
            let argv = read_argv(p.pid);
            let mut i = 0;
            while i < argv.len() {
                if argv[i] == "-S"
                    && let Some(sock) = argv.get(i + 1)
                {
                    let sp = PathBuf::from(sock);
                    if sp.exists() {
                        if let Some(parent) = sp.parent() {
                            dirs.push(parent.to_path_buf());
                        }
                        sockets.insert(sp);
                    }
                }
                i += 1;
            }
        }
    }

    for dir in dirs {
        if dir.is_dir() {
            watch_dirs.push(dir.clone());
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if std::fs::metadata(&p).is_ok_and(|m| m.uid() == uid) {
                        sockets.insert(p);
                    }
                }
            }
        }
    }
    let mut list: Vec<PathBuf> = sockets.into_iter().collect();
    list.sort();
    list
}

fn list_tmux_panes(sockets: &[PathBuf]) -> Vec<TmuxPaneEntry> {
    let mut out = Vec::new();
    for sock in sockets {
        let sock_str = sock.display().to_string();
        let Ok(res) = Command::new("tmux")
            .args([
                "-S",
                &sock_str,
                "list-panes",
                "-a",
                "-F",
                "#{session_name}\t#{window_index}\t#{pane_index}\t#{pane_id}\t#{pane_pid}\t#{pane_width}\t#{pane_height}\t#{pane_current_path}\t#{pane_title}\t#{pid}\t#{window_width}\t#{window_height}\t#{status}",
            ])
            .output()
        else {
            continue;
        };
        if !res.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&res.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 13 {
                continue;
            }
            let Ok(pane_pid) = parts[4].parse::<i32>() else { continue };
            let cols = parts[5].parse::<u16>().unwrap_or(80);
            let rows = parts[6].parse::<u16>().unwrap_or(24);
            let server_pid = parts[9].parse::<i32>().unwrap_or(0);
            let win_w = parts[10].parse::<u16>().unwrap_or(cols);
            let win_h = parts[11].parse::<u16>().unwrap_or(rows);
            let status_rows: u16 = match parts[12].trim() {
                "off" | "0" => 0,
                "on" => 1,
                other => other.parse().unwrap_or(1),
            };
            out.push(TmuxPaneEntry {
                socket: sock_str.clone(),
                session: parts[0].to_owned(),
                target: format!("{}:{}.{}", parts[0], parts[1], parts[2]),
                pane_id: parts[3].to_owned(),
                pane_pid,
                cols,
                rows,
                pty_cols: win_w,
                pty_rows: win_h.saturating_add(status_rows),
                cwd: parts[7].to_owned(),
                title: parts[8].to_owned(),
                server_pid,
            });
        }
    }
    out
}

fn subtree_pids(procs: &[Proc], root: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        for p in procs {
            if p.ppid == pid {
                stack.push(p.pid);
            }
        }
    }
    out
}

fn friendly_folder(cwd: &str, home: &Path) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    if Path::new(cwd) == home {
        return "Home".into();
    }
    cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd).to_owned()
}

fn nice_agent_label(kind: &str, argv: &[String], env: &HashMap<String, String>) -> String {
    if let Some(name) = env.get("OLO_ACTIVITY_NAME").filter(|s| !s.trim().is_empty()) {
        return name.trim().to_owned();
    }
    // Check for `--role` or Olo agent folder name in argv (e.g. /var/tmp/olo-agents/<name>/brief.md).
    for (i, a) in argv.iter().enumerate() {
        if (a == "--role" || a == "-role")
            && let Some(v) = argv.get(i + 1)
            && !v.trim().is_empty()
        {
            return v.trim().to_owned();
        }
        if let Some(rest) = a.split("/olo-agents/").nth(1)
            && let Some(name) = rest.split('/').next()
            && !name.is_empty()
        {
            return name.to_owned();
        }
    }
    match kind {
        "claude" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "agy" => "Antigravity".into(),
        other => other.to_owned(),
    }
}

/// Perform one full discovery pass for the given Rex panes.
/// Also returns all directories to watch with inotify and all child PIDs to watch with pidfd.
fn scan_once(panes: &[PaneQuery], places: &Places) -> (ActivitySnapshot, Vec<PathBuf>, Vec<i32>) {
    let procs = all_procs();
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut watch_dirs: Vec<PathBuf> = Vec::new();
    let mut child_pids: Vec<i32> = Vec::new();

    // Base agent metadata directories to watch via inotify.
    for base_dir in [places.claude.join("sessions"), places.codex.join("sessions"), places.agy(), places.agy_brain(), PathBuf::from("/var/tmp/olo-agents")] {
        if base_dir.is_dir() {
            watch_dirs.push(base_dir);
        }
    }
    // Also watch newest Codex date directory (YYYY/MM/DD) so new subagent rollouts wake immediately.
    if let Some(latest_codex_dir) = newest_leaf_dir(&places.codex.join("sessions"), 3) {
        watch_dirs.push(latest_codex_dir);
    }

    // Build process trees & identify each pane's top-level agent and session.
    let roots: Vec<i32> = panes.iter().map(|p| p.pid).collect();
    let trees: Vec<Vec<serde_json::Value>> = roots
        .iter()
        .map(|&root| {
            let mut tree = Vec::new();
            let mut at = vec![root];
            let mut seen = HashSet::new();
            while let Some(pid) = at.pop() {
                if !seen.insert(pid) {
                    continue;
                }
                if let Some(p) = by_pid.get(&pid) {
                    tree.push(process_json(p, places));
                    let mut children: Vec<i32> = procs.iter().filter(|c| c.ppid == pid).map(|c| c.pid).collect();
                    children.sort_unstable_by(|a, b| b.cmp(a));
                    at.extend(children);
                }
            }
            tree
        })
        .collect();
    let found: Vec<Option<crate::panes::Found>> = trees.iter().map(|t| found_in(t)).collect();
    let screens: HashMap<i32, Vec<String>> = panes.iter().map(|p| (p.pid, p.screens.clone())).collect();
    let has_codex = found.iter().any(|f| f.as_ref().is_some_and(|x| x.agent == "codex"));
    let rollouts = codex_rollouts(places);
    let said = sessions(&found, &screens, &roots, places, &mut || if has_codex { codex_threads(places) } else { Vec::new() });

    // Discover tmux sockets & panes once across the host.
    let tmux_sockets = discover_tmux_sockets(&procs, &mut watch_dirs);
    let tmux_panes = list_tmux_panes(&tmux_sockets);

    // Precompute subtree pid sets and block ownership maps for tmux matching.
    let mut pane_subtrees: Vec<HashSet<i32>> = Vec::with_capacity(panes.len());
    for &root in &roots {
        pane_subtrees.push(subtree_pids(&procs, root).into_iter().collect());
    }
    let block_index: HashMap<&str, usize> = panes.iter().enumerate().map(|(i, p)| (p.block_id.as_str(), i)).collect();

    // Map each tmux pane to a parent Rex pane index (if owned by one).
    let mut tmux_for_pane: Vec<Vec<&TmuxPaneEntry>> = vec![Vec::new(); panes.len()];
    for tp in &tmux_panes {
        let mut owner: Option<usize> = None;
        // 1. Direct REX_BLOCK / OLO_PARENT_BLOCK in tmux pane shell or any child process in that tmux pane.
        let t_pids = subtree_pids(&procs, tp.pane_pid);
        for &pid in &t_pids {
            let env = read_environ(pid);
            if let Some(b) = env.get("OLO_PARENT_BLOCK").or_else(|| env.get("REX_BLOCK"))
                && let Some(&idx) = block_index.get(b.as_str())
            {
                owner = Some(idx);
                break;
            }
        }
        // 2. REX_BLOCK in the tmux server's own environment.
        if owner.is_none() && tp.server_pid > 0 {
            let env = read_environ(tp.server_pid);
            if let Some(b) = env.get("OLO_PARENT_BLOCK").or_else(|| env.get("REX_BLOCK"))
                && let Some(&idx) = block_index.get(b.as_str())
            {
                owner = Some(idx);
            }
        }
        // 3. Tmux server PID is a descendant of one of our Rex panes.
        if owner.is_none() && tp.server_pid > 0 {
            for (i, set) in pane_subtrees.iter().enumerate() {
                if set.contains(&tp.server_pid) {
                    owner = Some(i);
                    break;
                }
            }
        }
        if let Some(idx) = owner {
            tmux_for_pane[idx].push(tp);
        }
    }

    let mut out_panes = Vec::with_capacity(panes.len());
    for (idx, q) in panes.iter().enumerate() {
        let f = found[idx].as_ref();
        let top_session = said[idx].as_ref().map(|(id, _)| id.clone());

        // Top-level agent (either standard `found_in` or a stand-in with `OLO_ACTIVITY_AGENT`).
        let (top_kind, top_pid, top_start, top_cwd, top_args, top_env) = if let Some(f) = f {
            let env = read_environ(f.pid);
            (Some(f.agent.to_owned()), Some(f.pid), f.start, f.cwd.clone(), f.args.clone(), env)
        } else {
            // Check pane subtree for an OLO_ACTIVITY_AGENT stand-in.
            let mut standin = None;
            for pid in subtree_pids(&procs, q.pid) {
                let env = read_environ(pid);
                if let Some(k) = env.get("OLO_ACTIVITY_AGENT").filter(|s| !s.trim().is_empty()) {
                    let cwd = read_cwd(pid);
                    standin = Some((Some(k.trim().to_owned()), Some(pid), start_ticks(pid), if cwd.is_empty() { q.cwd.clone() } else { cwd }, read_argv(pid), env));
                    break;
                }
            }
            standin.unwrap_or_else(|| (None, None, 0, q.cwd.clone(), Vec::new(), HashMap::new()))
        };

        let top_transcript =
            top_kind.as_deref().zip(top_pid).and_then(|(kind, pid)| resolve_transcript(places, kind, pid, top_start, &top_cwd, &top_args, top_session.as_deref(), &top_env, &rollouts));
        if let Some(tp) = &top_transcript
            && let Some(parent) = tp.parent()
        {
            watch_dirs.push(parent.to_path_buf());
        }

        let mut subs: Vec<SubagentInfo> = Vec::new();
        let mut claimed_pids: HashSet<i32> = HashSet::new();
        if let Some(pid) = top_pid {
            claimed_pids.insert(pid);
        }

        // 1. Native subagents (Claude Code, Codex, Antigravity CLI).
        let parent_started = (top_start > 0).then(|| started_at(top_start)).flatten();
        let effective_sid = top_session
            .clone()
            .or_else(|| if top_kind.as_deref() == Some("claude") { top_transcript.as_ref().and_then(|p| p.file_stem()?.to_str().map(str::to_owned)).filter(|s| uuid(s)) } else { None });
        if let (Some(kind), Some(sid)) = (top_kind.as_deref(), effective_sid.as_deref()) {
            match kind {
                "claude" => subs.extend(discover_claude_subagents(places, &top_cwd, sid, parent_started, &mut watch_dirs)),
                "codex" => subs.extend(discover_codex_subagents(sid, parent_started, &rollouts, &mut watch_dirs)),
                "agy" => {
                    if let Some(tp) = &top_transcript {
                        subs.extend(discover_agy_subagents(places, tp, parent_started, &mut watch_dirs));
                    }
                }
                _ => {}
            }
        }

        // 2. Tmux-launched child agents owned by this Rex pane.
        let mut tmux_pids_seen: HashSet<i32> = HashSet::new();
        for tp in &tmux_for_pane[idx] {
            let sub_pids = subtree_pids(&procs, tp.pane_pid);
            let mut found_tmux_agent = None;
            for &pid in &sub_pids {
                if claimed_pids.contains(&pid) {
                    continue;
                }
                let Some(proc) = by_pid.get(&pid) else { continue };
                let env = read_environ(pid);
                if let Some((kind, args)) = detect_agent(pid, &proc.comm, &env) {
                    found_tmux_agent = Some((pid, kind, args, env));
                    break;
                }
            }
            let Some((apid, kind, args, env)) = found_tmux_agent else { continue };
            for &p in &sub_pids {
                tmux_pids_seen.insert(p);
            }
            claimed_pids.insert(apid);
            child_pids.push(apid);
            let st = start_ticks(apid);
            let cwd = {
                let c = read_cwd(apid);
                if c.is_empty() { tp.cwd.clone() } else { c }
            };
            let transcript = resolve_transcript(places, &kind, apid, st, &cwd, &args, None, &env, &rollouts);
            if let Some(tr) = &transcript
                && let Some(parent) = tr.parent()
            {
                watch_dirs.push(parent.to_path_buf());
            }
            let name = if !tp.session.is_empty() { tp.session.clone() } else { nice_agent_label(&kind, &args, &env) };
            let description = env
                .get("OLO_ACTIVITY_DESC")
                .cloned()
                .filter(|s| !s.is_empty())
                .or_else(|| (!tp.title.is_empty() && tp.title != tp.session).then(|| tp.title.clone()))
                .unwrap_or_else(|| friendly_folder(&cwd, &places.home));
            let child_started = started_at(st);
            let started_ms = child_started.map_or(0, system_time_ms);
            let state = infer_live_agent_state(&kind, transcript.as_deref(), child_started);
            subs.push(SubagentInfo {
                id: format!("tmux:{}:{}:{}", tp.socket, tp.session, tp.pane_id),
                kind,
                origin: "tmux".into(),
                name,
                description,
                state: state.into(),
                pid: Some(apid),
                transcript: transcript.map(|p| p.display().to_string()),
                tmux: Some(TmuxTarget {
                    socket: tp.socket.clone(),
                    session: tp.session.clone(),
                    target: tp.target.clone(),
                    cols: tp.cols,
                    rows: tp.rows,
                    pty_cols: tp.pty_cols,
                    pty_rows: tp.pty_rows,
                }),
                started_ms,
            });
        }

        // 3. Direct shell-launched child agents in the pane's process subtree (not in tmux).
        let search_root = top_pid.unwrap_or(q.pid);
        let desc_pids = subtree_pids(&procs, search_root);
        let top_same_pids: HashSet<i32> = f.map(|x| x.pids.iter().map(|&(p, _)| p).collect()).unwrap_or_default();
        for &pid in &desc_pids {
            if pid == q.pid || claimed_pids.contains(&pid) || tmux_pids_seen.contains(&pid) {
                continue;
            }
            let Some(proc) = by_pid.get(&pid) else { continue };
            let env = read_environ(pid);
            let Some((kind, args)) = detect_agent(pid, &proc.comm, &env) else { continue };
            // Skip internal wrapper/worker processes of the same top-level agent instance
            // unless it has a distinct session/transcript or is a child of a shell tool.
            let is_direct_top_child = top_pid.is_some_and(|tp| proc.ppid == tp);
            let has_explicit_sub = env.contains_key("OLO_ACTIVITY_AGENT") || env.contains_key("OLO_ACTIVITY_TRANSCRIPT");
            if top_same_pids.contains(&pid) && is_direct_top_child && !has_explicit_sub && top_kind.as_deref() == Some(kind.as_str()) {
                continue;
            }
            // Don't double-count descendants of an already-discovered shell subagent.
            if claimed_pids.contains(&proc.ppid) && !top_pid.is_some_and(|tp| tp == proc.ppid) {
                claimed_pids.insert(pid);
                continue;
            }
            claimed_pids.insert(pid);
            child_pids.push(pid);
            let st = start_ticks(pid);
            let cwd = read_cwd(pid);
            let transcript = resolve_transcript(places, &kind, pid, st, &cwd, &args, None, &env, &rollouts);
            if let Some(tr) = &transcript
                && let Some(parent) = tr.parent()
            {
                watch_dirs.push(parent.to_path_buf());
            }
            let name = nice_agent_label(&kind, &args, &env);
            let description = env.get("OLO_ACTIVITY_DESC").cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| friendly_folder(&cwd, &places.home));
            let child_started = started_at(st);
            let started_ms = child_started.map_or(0, system_time_ms);
            let state = infer_live_agent_state(&kind, transcript.as_deref(), child_started);
            subs.push(SubagentInfo {
                id: format!("proc:{pid}:{st}"),
                kind,
                origin: "shell".into(),
                name,
                description,
                state: state.into(),
                pid: Some(pid),
                transcript: transcript.map(|p| p.display().to_string()),
                tmux: None,
                started_ms,
            });
        }

        out_panes.push(PaneActivity {
            block_id: q.block_id.clone(),
            agent: top_kind,
            pid: top_pid,
            session_id: top_session,
            transcript: top_transcript.map(|p| p.display().to_string()),
            subagents: subs,
        });
    }

    (ActivitySnapshot { ok: true, panes: out_panes }, watch_dirs, child_pids)
}

fn newest_leaf_dir(root: &Path, max_depth: usize) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    let mut cur = root.to_path_buf();
    for _ in 0..max_depth {
        let mut best: Option<(String, PathBuf)> = None;
        let Ok(rd) = std::fs::read_dir(&cur) else { break };
        for entry in rd.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && let Ok(name) = entry.file_name().into_string()
                && best.as_ref().is_none_or(|(b, _)| name > *b)
            {
                best = Some((name, entry.path()));
            }
        }
        match best {
            Some((_, next)) => cur = next,
            None => break,
        }
    }
    Some(cur)
}

// ---------------------------------------------------------------------------
// Zero-polling `activity watch` (inotify + pidfd + poll)
// ---------------------------------------------------------------------------

fn pidfd_open(pid: i32) -> Option<OwnedFd> {
    // SAFETY: Linux syscall `pidfd_open(pid, 0)` returns a new fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) as i32 };
    (fd >= 0).then_some(OwnedFd(fd))
}

fn add_inotify_watch(ifd: i32, dir: &Path, mask: u32, watched: &mut HashMap<PathBuf, i32>) {
    if watched.contains_key(dir) || !dir.exists() {
        return;
    }
    let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else { return };
    // SAFETY: `ifd` is valid and `c` is NUL-terminated.
    let wd = unsafe { libc::inotify_add_watch(ifd, c.as_ptr(), mask) };
    if wd >= 0 {
        watched.insert(dir.to_path_buf(), wd);
    }
}

fn set_nonblocking(fd: i32) {
    // SAFETY: standard fcntl on valid fd.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

fn run_watch(once: bool) -> i32 {
    let places = Places::here();
    if once {
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            return 0;
        }
        let input: WatchInput = serde_json::from_str(line.trim()).unwrap_or_default();
        let panes = input.panes.unwrap_or_default();
        let (snap, _, _) = scan_once(&panes, &places);
        let mut out = io::stdout().lock();
        let _ = serde_json::to_writer(&mut out, &snap);
        let _ = writeln!(out);
        let _ = out.flush();
        return 0;
    }

    // SAFETY: creates non-blocking close-on-exec inotify descriptor.
    let raw_ifd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if raw_ifd < 0 {
        eprintln!("olo-rex-link activity watch: inotify_init1 failed: {}", io::Error::last_os_error());
        return 1;
    }
    let ifd = OwnedFd(raw_ifd);
    set_nonblocking(0);

    let mut watched_dirs: HashMap<PathBuf, i32> = HashMap::new();
    let mut pidfds: HashMap<i32, OwnedFd> = HashMap::new();
    let mut current_panes: Vec<PaneQuery> = Vec::new();
    let mut last_snapshot: Option<ActivitySnapshot> = None;
    let mut stdin_buf = Vec::new();
    let mut dirty = false;
    let mut dirty_since: Option<Instant> = None;
    let mut force_emit = false;
    let mut inotify_buf = vec![0u8; 32 * 1024];

    // Emit initial ready banner so Rex knows the watcher is up.
    {
        let mut out = io::stdout().lock();
        if writeln!(out, "{{\"ready\":true}}").and_then(|_| out.flush()).is_err() {
            return 0;
        }
    }

    loop {
        if dirty && dirty_since.is_none() {
            dirty_since = Some(Instant::now());
        }
        let timeout_ms: i32 = if force_emit {
            0
        } else if let Some(since) = dirty_since {
            let elapsed = since.elapsed().as_millis() as u64;
            SETTLE_MS.saturating_sub(elapsed).min(i32::MAX as u64) as i32
        } else {
            -1
        };

        let mut pfds = Vec::with_capacity(2 + pidfds.len());
        pfds.push(libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 });
        pfds.push(libc::pollfd { fd: ifd.0, events: libc::POLLIN, revents: 0 });
        let tracked_pids: Vec<i32> = pidfds.keys().copied().collect();
        for pid in &tracked_pids {
            if let Some(fd) = pidfds.get(pid) {
                pfds.push(libc::pollfd { fd: fd.0, events: libc::POLLIN, revents: 0 });
            }
        }

        // SAFETY: `pfds` holds valid descriptors.
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return 1;
        }

        // 1. Read stdin commands (pane updates / refresh / EOF to stop).
        if pfds[0].revents != 0 {
            let mut chunk = [0u8; 8192];
            loop {
                // SAFETY: non-blocking read from stdin.
                let got = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
                if got > 0 {
                    stdin_buf.extend_from_slice(&chunk[..got as usize]);
                } else if got == 0 {
                    return 0;
                } else {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::Interrupted {
                        break;
                    }
                    return 0;
                }
            }
            while let Some(nl) = stdin_buf.iter().position(|&b| b == b'\n') {
                let line = std::mem::take(&mut stdin_buf);
                let (head, tail) = line.split_at(nl + 1);
                stdin_buf = tail.to_vec();
                if let Ok(text) = std::str::from_utf8(head)
                    && let Ok(msg) = serde_json::from_str::<WatchInput>(text.trim())
                {
                    if let Some(panes) = msg.panes {
                        current_panes = panes;
                        dirty = true;
                        force_emit = true;
                    }
                    if msg.refresh {
                        dirty = true;
                    }
                }
            }
            if (pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                return 0;
            }
        }

        // 2. Drain inotify events.
        if pfds[1].revents != 0 {
            loop {
                // SAFETY: non-blocking read from inotify fd.
                let got = unsafe { libc::read(ifd.0, inotify_buf.as_mut_ptr().cast(), inotify_buf.len()) };
                if got <= 0 {
                    break;
                }
                dirty = true;
            }
        }

        // 3. Check exited child pidfds (instant wake on subagent exit!).
        let mut exited = Vec::new();
        for (i, pid) in tracked_pids.iter().enumerate() {
            if pfds[2 + i].revents != 0 {
                exited.push(*pid);
                dirty = true;
            }
        }
        for pid in exited {
            pidfds.remove(&pid);
        }

        // 4. If settled (or forced by stdin pane update), scan & emit if changed.
        let ready_to_scan = force_emit || (dirty && dirty_since.is_some_and(|s| s.elapsed().as_millis() as u64 >= SETTLE_MS));
        if ready_to_scan {
            dirty = false;
            dirty_since = None;
            let forced = force_emit;
            force_emit = false;

            let (snap, dirs, cpids) = scan_once(&current_panes, &places);
            for dir in dirs {
                add_inotify_watch(ifd.0, &dir, INOTIFY_DIR_MASK, &mut watched_dirs);
            }
            // Refresh pidfds for current live child agents.
            let cpid_set: HashSet<i32> = cpids.iter().copied().collect();
            pidfds.retain(|pid, _| cpid_set.contains(pid));
            for pid in cpids {
                if !pidfds.contains_key(&pid)
                    && let Some(pfd) = pidfd_open(pid)
                {
                    pidfds.insert(pid, pfd);
                }
            }

            if forced || last_snapshot.as_ref() != Some(&snap) {
                let mut out = io::stdout().lock();
                if serde_json::to_writer(&mut out, &snap).is_err() || writeln!(out).is_err() || out.flush().is_err() {
                    return 0;
                }
                last_snapshot = Some(snap);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Zero-polling `activity tail <PATH>` (inotify + poll)
// ---------------------------------------------------------------------------

fn emit_json(val: &serde_json::Value) -> bool {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, val).is_ok() && writeln!(out).is_ok() && out.flush().is_ok()
}

fn run_tail(args: &[String]) -> i32 {
    let mut path_arg: Option<String> = None;
    let mut max_initial: u64 = DEFAULT_MAX_INITIAL;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--max-initial" => {
                max_initial = it.next().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MAX_INITIAL);
            }
            other if path_arg.is_none() => path_arg = Some(other.to_owned()),
            _ => {}
        }
    }
    let Some(path_str) = path_arg else {
        eprintln!("olo-rex-link activity tail: missing PATH\n{USAGE}");
        return 2;
    };
    let path = PathBuf::from(&path_str);

    // SAFETY: non-blocking inotify fd.
    let raw_ifd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if raw_ifd < 0 {
        let _ = emit_json(&serde_json::json!({"type": "error", "error": format!("inotify: {}", io::Error::last_os_error())}));
        return 1;
    }
    let ifd = OwnedFd(raw_ifd);
    set_nonblocking(0);

    let mut watches: HashMap<PathBuf, i32> = HashMap::new();
    if let Some(parent) = path.parent() {
        add_inotify_watch(ifd.0, parent, INOTIFY_DIR_MASK, &mut watches);
    }
    add_inotify_watch(ifd.0, &path, INOTIFY_FILE_MASK, &mut watches);

    let mut cur_ino: u64 = 0;
    let mut offset: u64 = 0;
    let mut partial = Vec::<u8>::new();

    if let Some((ino, end_off, truncated, lines, rem)) = read_initial_tail(&path, max_initial) {
        cur_ino = ino;
        offset = end_off;
        partial = rem;
        if !emit_json(&serde_json::json!({"type": "meta", "path": path_str, "truncated": truncated, "offset": offset})) {
            return 0;
        }
        for chunk in lines.chunks(TAIL_BATCH_LINES) {
            if !emit_json(&serde_json::json!({"type": "lines", "lines": chunk})) {
                return 0;
            }
        }
    } else if !emit_json(&serde_json::json!({"type": "meta", "path": path_str, "truncated": false, "offset": 0, "waiting": true})) {
        return 0;
    }

    let mut inotify_buf = vec![0u8; 16 * 1024];
    loop {
        let mut pfds = [libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: ifd.0, events: libc::POLLIN, revents: 0 }];
        // SAFETY: valid pollfds.
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return 1;
        }
        if pfds[0].revents != 0 {
            let mut sink = [0u8; 256];
            // SAFETY: non-blocking read from stdin.
            let got = unsafe { libc::read(0, sink.as_mut_ptr().cast(), sink.len()) };
            if got <= 0 || (pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                return 0;
            }
        }
        if pfds[1].revents != 0 {
            loop {
                // SAFETY: drain inotify buffer.
                let got = unsafe { libc::read(ifd.0, inotify_buf.as_mut_ptr().cast(), inotify_buf.len()) };
                if got <= 0 {
                    break;
                }
            }
            // Re-ensure watch on file (in case file was just created or atomically replaced).
            if path.exists() {
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                let ino = meta.ino();
                let len = meta.len();
                if ino != cur_ino || len < offset {
                    // File created, replaced, or truncated: re-watch and reset.
                    watches.remove(&path);
                    add_inotify_watch(ifd.0, &path, INOTIFY_FILE_MASK, &mut watches);
                    cur_ino = ino;
                    offset = 0;
                    partial.clear();
                    if !emit_json(&serde_json::json!({"type": "reset"})) {
                        return 0;
                    }
                }
                if len > offset
                    && let Ok(mut f) = File::open(&path)
                    && f.seek(SeekFrom::Start(offset)).is_ok()
                {
                    let mut appended = Vec::new();
                    if f.read_to_end(&mut appended).is_ok() && !appended.is_empty() {
                        offset += appended.len() as u64;
                        partial.extend_from_slice(&appended);
                        let (lines, rem) = split_complete_lines(&partial);
                        partial = rem;
                        let kept: Vec<String> = lines.into_iter().filter(|l| !is_tail_noise_line(l.as_bytes())).collect();
                        for chunk in kept.chunks(TAIL_BATCH_LINES) {
                            if !emit_json(&serde_json::json!({"type": "lines", "lines": chunk})) {
                                return 0;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Top-level JSONL record types dropped by `TranscriptDoc` that carry zero visible
/// conversation content (e.g., Claude 2.1 `api-request-blob`, `attachment`, `progress`).
fn is_tail_noise_type(ty: &str) -> bool {
    matches!(
        ty,
        "progress"
            | "attachment"
            | "api-request"
            | "api-request-blob"
            | "api-request-shape"
            | "data-flow-records"
            | "data-flow-incomplete"
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
            | "queue-operation"
            | "token_usage_record"
            | "world_state"
    )
}

#[derive(Deserialize)]
struct TailLineProbe<'a> {
    #[serde(rename = "type", default)]
    ty: Option<&'a str>,
    #[serde(default)]
    attachment: Option<serde::de::IgnoredAny>,
}

/// Fast check for telemetry/noise JSONL records so 300 KB+ `api-request-blob` lines
/// don't crowd out real conversation turns in `read_initial_tail` or flood SSH on live tail.
fn is_tail_noise_line(bytes: &[u8]) -> bool {
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() || trimmed[0] != b'{' {
        return false;
    }
    // Fast path: check top-level `"type"` in the leading 512 bytes when it appears before
    // any nested `"message"`/`"content"`/`"payload"` block (skips multi-hundred-KB blobs in O(1)).
    let head = &trimmed[..trimmed.len().min(512)];
    let find_sub = |hay: &[u8], pat: &[u8]| hay.windows(pat.len()).position(|w| w == pat);
    let nested_at = [b"\"message\"".as_slice(), b"\"content\"", b"\"payload\""].into_iter().filter_map(|p| find_sub(head, p)).min().unwrap_or(head.len());
    if let Some(pos) = find_sub(&head[..nested_at], b"\"type\"") {
        let mut i = pos + 6;
        while i < head.len() && matches!(head[i], b' ' | b'\t' | b':') {
            i += 1;
        }
        if i < head.len() && head[i] == b'"' {
            i += 1;
            let start = i;
            while i < head.len() && head[i] != b'"' && head[i] != b'\\' {
                i += 1;
            }
            if i < head.len()
                && head[i] == b'"'
                && let Ok(ty) = std::str::from_utf8(&head[start..i])
            {
                if is_tail_noise_type(ty) {
                    return true;
                }
                if matches!(
                    ty,
                    "user"
                        | "assistant"
                        | "system"
                        | "summary"
                        | "response_item"
                        | "event_msg"
                        | "session_meta"
                        | "turn_context"
                        | "compacted"
                        | "USER_INPUT"
                        | "PLANNER_RESPONSE"
                        | "GENERIC"
                        | "TOOL_RESPONSE"
                        | "CHECKPOINT"
                        | "SYSTEM_MESSAGE"
                ) {
                    return false;
                }
            }
        }
    }
    if let Ok(probe) = serde_json::from_slice::<TailLineProbe<'_>>(trimmed) {
        if probe.attachment.is_some() {
            return true;
        }
        if let Some(ty) = probe.ty {
            return is_tail_noise_type(ty);
        }
    }
    false
}

/// Read the latest `max_init` bytes of *substantive* (non-telemetry) lines from `p`
/// by scanning backwards in chunks (up to 32 MiB raw) so huge Claude 2.1 `api-request-blob`
/// records cannot squeeze a long conversation down to 0–1 visible turns.
fn read_initial_tail(p: &Path, max_init: u64) -> Option<(u64, u64, bool, Vec<String>, Vec<u8>)> {
    let meta: Metadata = std::fs::metadata(p).ok()?;
    let ino = meta.ino();
    let len = meta.len();
    let mut f = File::open(p).ok()?;

    const CHUNK: u64 = 256 * 1024;
    let raw_cap: u64 = max_init.saturating_mul(32).clamp(8 * 1024 * 1024, 32 * 1024 * 1024);
    let min_pos = len.saturating_sub(raw_cap);

    let mut pos = len;
    let mut carry_tail = Vec::<u8>::new(); // bytes belonging to the start of the line continuing into the next (later) chunk
    let mut rem_at_eof: Option<Vec<u8>> = None;
    let mut kept_rev: Vec<String> = Vec::new();
    let mut kept_bytes: u64 = 0;
    let mut truncated = false;

    while pos > min_pos {
        let next_pos = pos.saturating_sub(CHUNK).max(min_pos);
        let to_read = (pos - next_pos) as usize;
        f.seek(SeekFrom::Start(next_pos)).ok()?;
        let mut buf = vec![0u8; to_read];
        f.read_exact(&mut buf).ok()?;
        if !carry_tail.is_empty() {
            buf.extend_from_slice(&carry_tail);
            carry_tail.clear();
        }
        let mut end = buf.len();
        // On the very first (EOF) chunk, anything after the last `\n` is an incomplete line still being written.
        if rem_at_eof.is_none() {
            match buf.iter().rposition(|&b| b == b'\n') {
                Some(nl) => {
                    rem_at_eof = Some(buf[nl + 1..].to_vec());
                    end = nl;
                }
                None => {
                    if next_pos == 0 {
                        rem_at_eof = Some(buf);
                        pos = 0;
                        break;
                    }
                    carry_tail = buf;
                    pos = next_pos;
                    continue;
                }
            }
        }

        // Walk complete lines backwards inside `buf[..end]`.
        let mut stop = false;
        loop {
            let line_start = match buf[..end].iter().rposition(|&b| b == b'\n') {
                Some(nl) => nl + 1,
                None => {
                    if next_pos == 0 {
                        0
                    } else {
                        carry_tail = buf[..end].to_vec();
                        break;
                    }
                }
            };
            let mut line_end = end;
            if line_end > line_start && buf[line_end - 1] == b'\r' {
                line_end -= 1;
            }
            let line_slice = &buf[line_start..line_end];
            if !line_slice.is_empty() && !is_tail_noise_line(line_slice) {
                let sz = line_slice.len() as u64 + 1;
                if kept_bytes > 0 && kept_bytes + sz > max_init {
                    truncated = true;
                    stop = true;
                    break;
                }
                kept_bytes += sz;
                kept_rev.push(String::from_utf8_lossy(line_slice).into_owned());
            }
            if line_start == 0 {
                break;
            }
            end = line_start - 1;
        }
        pos = next_pos;
        if stop {
            break;
        }
    }

    if !truncated && pos > 0 {
        truncated = true;
    }

    kept_rev.reverse();
    Some((ino, len, truncated, kept_rev, rem_at_eof.unwrap_or_default()))
}

fn split_complete_lines(bytes: &[u8]) -> (Vec<String>, Vec<u8>) {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let mut end = i;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            lines.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
            start = i + 1;
        }
    }
    (lines, bytes[start..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Places {
        let d = PathBuf::from(format!("/var/tmp/olo-rex-link-act-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Places { claude: d.join(".claude"), codex: d.join(".codex"), home: d }
    }

    #[test]
    fn discovers_native_claude_codex_and_agy_subagents() {
        let places = scratch("native");
        let now = Some(SystemTime::now());
        // 1. Claude native subagents: running (missing `finished` + tool_use), waiting (AskUserQuestion), failed (parent task-notification), done (end_turn + finished)
        let c_sid = "11111111-0000-4000-8000-000000000010";
        let proj = places.claude.join("projects").join(project_name("/w/app"));
        let sub_dir = proj.join(c_sid).join("subagents");
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::write(
            proj.join(format!("{c_sid}.jsonl")),
            "{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"<task-notification>\\n<task-id>fail99</task-id>\\n<status>failed</status>\\n</task-notification>\"}\n",
        )
        .unwrap();
        // Running Claude subagent (real Claude Code omits `finished` while running, and appends telemetry after tool_use)
        std::fs::write(sub_dir.join("agent-abc123.meta.json"), r#"{"agentType":"Explore","description":"Find parser bugs"}"#).unwrap();
        std::fs::write(
            sub_dir.join("agent-abc123.jsonl"),
            "{\"type\":\"user\",\"timestamp\":\"2026-11-25T10:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"Find parser bugs\"}}\n\
             {\"type\":\"assistant\",\"timestamp\":\"2026-11-25T10:00:02Z\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"tool_use\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"sleep 60\"}}]}}\n\
             {\"type\":\"attachment\",\"attachment\":{\"type\":\"deferred_tools_record\"}}\n",
        )
        .unwrap();
        // Waiting Claude subagent (AskUserQuestion)
        std::fs::write(sub_dir.join("agent-wait01.meta.json"), r#"{"agentType":"Plan","description":"Clarify layout"}"#).unwrap();
        std::fs::write(
            sub_dir.join("agent-wait01.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-11-25T10:00:05Z\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"tool_use\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"AskUserQuestion\",\"input\":{}}]}}\n",
        )
        .unwrap();
        // Failed via parent task-notification
        std::fs::write(sub_dir.join("agent-fail99.meta.json"), r#"{"agentType":"general-purpose","description":"Build release"}"#).unwrap();
        std::fs::write(sub_dir.join("agent-fail99.jsonl"), "{\"type\":\"user\",\"timestamp\":\"2026-11-25T10:00:06Z\",\"message\":{\"role\":\"user\",\"content\":\"Build\"}}\n").unwrap();
        // Completed via meta finished=true + end_turn
        std::fs::write(sub_dir.join("agent-done02.meta.json"), r#"{"agentType":"Explore","description":"Audit icons","finished":true}"#).unwrap();
        std::fs::write(
            sub_dir.join("agent-done02.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-11-25T10:00:08Z\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"end_turn\",\"content\":[{\"type\":\"text\",\"text\":\"All good.\"}]}}\n",
        )
        .unwrap();

        let mut wd = Vec::new();
        let c_subs = discover_claude_subagents(&places, "/w/app", c_sid, now, &mut wd);
        assert_eq!(c_subs.len(), 4);
        let state_of = |id: &str| c_subs.iter().find(|s| s.id == id).unwrap().state.as_str();
        assert_eq!(state_of("claude-sub:abc123"), "working");
        assert_eq!(state_of("claude-sub:wait01"), "waiting");
        assert_eq!(state_of("claude-sub:fail99"), "failed");
        assert_eq!(state_of("claude-sub:done02"), "done");
        assert!(c_subs[0].tmux.is_none());

        // 2. Codex native subagents (working on task_started vs done on task_complete vs failed on task_failed)
        let p_tid = "01a1fd2c-8322-7a50-8bcb-babba41cf8b0";
        let s_work = "01a20010-44cc-7e41-840d-7c28016d197d";
        let s_done = "01a20010-44cc-7e41-840d-7c28016d197e";
        let s_fail = "01a20010-44cc-7e41-840d-7c28016d197f";
        let rdir = places.codex.join("sessions/2026/11/25");
        std::fs::create_dir_all(&rdir).unwrap();
        let write_codex = |tid: &str, nick: &str, tail: &str| {
            let f = rdir.join(format!("rollout-2026-11-25T10-00-00-{tid}.jsonl"));
            std::fs::write(
                &f,
                format!(
                    "{{\"type\":\"session_meta\",\"timestamp\":\"2026-11-25T10:00:00Z\",\"payload\":{{\"id\":\"{tid}\",\"session_id\":\"{p_tid}\",\"parent_thread_id\":\"{p_tid}\",\"agent_nickname\":\"{nick}\",\"source\":{{\"subagent\":{{\"thread_spawn\":{{\"parent_thread_id\":\"{p_tid}\",\"agent_nickname\":\"{nick}\",\"agent_path\":\"/root/fast_build\"}}}}}}}}}}\n{tail}\n"
                ),
            )
            .unwrap();
        };
        write_codex(s_work, "Newton", r#"{"type":"event_msg","payload":{"type":"task_started"}}"#);
        write_codex(s_done, "Curie", r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#);
        write_codex(s_fail, "Euler", r#"{"type":"event_msg","payload":{"type":"task_failed"}}"#);
        let rollouts = codex_rollouts(&places);
        let x_subs = discover_codex_subagents(p_tid, now, &rollouts, &mut wd);
        assert_eq!(x_subs.len(), 3);
        let xstate = |tid: &str| x_subs.iter().find(|s| s.id == format!("codex-sub:{tid}")).unwrap().state.as_str();
        assert_eq!(xstate(s_work), "working");
        assert_eq!(xstate(s_done), "done");
        assert_eq!(xstate(s_fail), "failed");

        // 3. AGY native subagents (working on RUNNING tool, waiting on ask_question, done on final PLANNER_RESPONSE without tool_calls)
        let a_parent = "22222222-0000-4000-8000-000000000001";
        let a_work = "22222222-0000-4000-8000-000000000002";
        let a_done = "22222222-0000-4000-8000-000000000003";
        let a_wait = "22222222-0000-4000-8000-000000000004";
        let p_logs = places.agy_brain().join(a_parent).join(".system_generated/logs");
        for id in [a_work, a_done, a_wait] {
            std::fs::create_dir_all(places.agy_brain().join(id).join(".system_generated/logs")).unwrap();
        }
        std::fs::create_dir_all(&p_logs).unwrap();
        std::fs::write(
            p_logs.join("transcript.jsonl"),
            format!(
                "{{\"type\":\"PLANNER_RESPONSE\",\"tool_calls\":[{{\"name\":\"invoke_subagent\",\"args\":{{\"Role\":\"\\\"Code Reviewer\\\"\",\"Prompt\":\"\\\"Inspect diff\\\"\"}}}},{{\"name\":\"invoke_subagent\",\"args\":{{\"Role\":\"\\\"Tester\\\"\",\"Prompt\":\"\\\"Run suite\\\"\"}}}},{{\"name\":\"invoke_subagent\",\"args\":{{\"Role\":\"\\\"Designer\\\"\",\"Prompt\":\"\\\"Ask user\\\"\"}}}}]}}\n\
                 {{\"type\":\"GENERIC\",\"content\":\"Launched conversationId: {a_work}\"}}\n\
                 {{\"type\":\"GENERIC\",\"content\":\"Launched conversationId: {a_done}\"}}\n\
                 {{\"type\":\"GENERIC\",\"content\":\"Launched conversationId: {a_wait}\"}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            places.agy_brain().join(a_work).join(".system_generated/logs/transcript.jsonl"),
            "{\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-11-25T10:00:00Z\",\"content\":\"Inspect diff\"}\n\
             {\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"tool_calls\":[{\"name\":\"run_command\",\"args\":{}}]}\n\
             {\"type\":\"GENERIC\",\"status\":\"RUNNING\",\"content\":\"Running build...\"}\n",
        )
        .unwrap();
        std::fs::write(
            places.agy_brain().join(a_done).join(".system_generated/logs/transcript.jsonl"),
            "{\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-11-25T10:00:00Z\",\"content\":\"Run suite\"}\n\
             {\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"All tests passed.\"}\n",
        )
        .unwrap();
        std::fs::write(
            places.agy_brain().join(a_wait).join(".system_generated/logs/transcript.jsonl"),
            "{\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-11-25T10:00:00Z\",\"content\":\"Ask user\"}\n\
             {\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"tool_calls\":[{\"name\":\"ask_question\",\"args\":{}}]}\n",
        )
        .unwrap();

        let a_subs = discover_agy_subagents(&places, &p_logs.join("transcript.jsonl"), now, &mut wd);
        assert_eq!(a_subs.len(), 3);
        let astate = |id: &str| a_subs.iter().find(|s| s.id == format!("agy-sub:{id}")).unwrap().state.as_str();
        assert_eq!(astate(a_work), "working");
        assert_eq!(astate(a_done), "done");
        assert_eq!(astate(a_wait), "waiting");

        let _ = std::fs::remove_dir_all(&places.home);
    }

    #[test]
    fn splits_complete_and_partial_tail_lines() {
        let (lines, rem) = split_complete_lines(b"one\ntwo\r\nthree");
        assert_eq!(lines, vec!["one", "two"]);
        assert_eq!(rem, b"three");
    }

    #[test]
    fn initial_tail_skips_claude_telemetry_blobs_to_preserve_conversation() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sub.jsonl");
        let mut raw = String::new();
        for i in 0..40 {
            raw.push_str(&format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"Turn {i} question\"}}}}\n"));
            raw.push_str(&format!("{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Turn {i} answer with details\"}}]}}}}\n"));
        }
        // Append >1.4 MiB of Claude 2.1 `api-request-blob` telemetry followed by one final line.
        let blob_payload = "x".repeat(350 * 1024);
        for _ in 0..4 {
            raw.push_str(&format!("{{\"type\":\"api-request-blob\",\"blob\":\"{blob_payload}\"}}\n"));
        }
        raw.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"All 40 turns complete!\"}]}}\n");
        std::fs::write(&path, &raw).unwrap();

        let (_, end_off, truncated, lines, rem) = read_initial_tail(&path, DEFAULT_MAX_INITIAL).unwrap();
        assert_eq!(end_off, raw.len() as u64);
        assert!(rem.is_empty());
        assert!(!truncated, "substantive bytes are far below 1 MiB once telemetry blobs are filtered");
        assert_eq!(lines.len(), 81, "all 80 prior turns + final turn preserved despite 1.4 MiB of telemetry blobs");
        assert!(lines.first().unwrap().contains("Turn 0 question"));
        assert!(lines.last().unwrap().contains("All 40 turns complete!"));

        // Ensure a user message mentioning `"type":"progress"` in prose is NOT dropped.
        let prose_line = br#"{"type":"user","message":{"role":"user","content":"Check {\"type\":\"progress\"} handling"}}"#;
        assert!(!is_tail_noise_line(prose_line));
        assert!(is_tail_noise_line(br#"{"type":"api-request-blob","data":"..."}"#));
        assert!(is_tail_noise_line(br#"{"type":"attachment","attachment":{"type":"deferred"}}"#));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
