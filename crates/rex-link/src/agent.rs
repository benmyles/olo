//! What the agents service reads of the agents on this host
//! (`agentd`), and `olo-rex-link agent <trust|inspect>`, the same asked
//! by hand.
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
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::panes::{Places, agent_of, argv_session, claude_record, codex_rollouts, codex_threads, project_name, start_ticks, uuid};
use crate::procs::{Proc, all_procs_stat, subtree_procs};

const USAGE: &str = "usage: olo-rex-link agent <trust PATH...|inspect>";

pub fn run(args: Vec<String>) -> i32 {
    let mut it = args.into_iter();
    let Some(sub) = it.next() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let rest: Vec<String> = it.collect();
    match sub.as_str() {
        "trust" => run_trust(&rest),
        "inspect" => run_inspect(),
        other => {
            eprintln!("olo-rex-link agent: unknown subcommand `{other}`\n{USAGE}");
            2
        }
    }
}

// ---------------------------------------------------------------------------
// `agent trust PATH...`: atomic pre-trust in ~/.claude.json
// ---------------------------------------------------------------------------

fn trust_claude_paths(claude_json: &Path, paths: &[String]) -> io::Result<Vec<String>> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    // Follow existing symlink so we never clobber a symlinked `~/.claude.json` with a plain file.
    let target_path = if std::fs::symlink_metadata(claude_json).is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::canonicalize(claude_json).unwrap_or_else(|_| claude_json.to_path_buf())
    } else {
        claude_json.to_path_buf()
    };
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = target_path.with_file_name(".claude.json.olo-trust.lock");
    let lock_file = OpenOptions::new().create(true).write(true).truncate(false).mode(0o600).open(&lock_path)?;
    // SAFETY: `lock_file` holds a valid file descriptor; exclusive flock serializes concurrent trust edits.
    unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX);
    }

    let mut trusted = Vec::new();
    let res = (|| -> io::Result<Vec<String>> {
        // Retry merge up to 3 times if an independent Claude process modified the file while we read it.
        for attempt in 0..3 {
            let meta_before = std::fs::metadata(&target_path).ok();
            let orig_mode = meta_before.as_ref().map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600);
            let mtime_before = meta_before.as_ref().and_then(|m| m.modified().ok());

            let mut root: Value = std::fs::read_to_string(&target_path).ok().and_then(|s| serde_json::from_str(&s).ok()).filter(|v: &Value| v.is_object()).unwrap_or_else(|| json!({}));

            let projects = root.as_object_mut().expect("object").entry("projects").or_insert_with(|| json!({}));
            if !projects.is_object() {
                *projects = json!({});
            }
            let proj_map = projects.as_object_mut().expect("projects object");

            trusted.clear();
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

            let mtime_after = std::fs::metadata(&target_path).ok().and_then(|m| m.modified().ok());
            if attempt < 2 && mtime_before.is_some() && mtime_after != mtime_before {
                continue;
            }

            let encoded = serde_json::to_string_pretty(&root).map_err(|e| io::Error::other(e.to_string()))?;
            let tmp = target_path.with_file_name(format!(".claude.json.olo-tmp.{}.{}", std::process::id(), attempt));
            {
                let mut f = OpenOptions::new().create(true).write(true).truncate(true).mode(orig_mode).open(&tmp)?;
                f.write_all(encoded.as_bytes())?;
                f.write_all(b"\n")?;
                let _ = f.sync_all();
            }
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(orig_mode));
            std::fs::rename(&tmp, &target_path)?;
            break;
        }
        Ok(trusted)
    })();

    // SAFETY: releases exclusive flock on valid fd before drop.
    unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN);
    }
    res
}

fn expand_tilde(raw: &str) -> PathBuf {
    let home = crate::procs::home();
    if raw == "~" {
        home
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    }
}

fn run_trust(args: &[String]) -> i32 {
    let mut home_override: Option<PathBuf> = None;
    let mut config_dir_override: Option<PathBuf> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--home" {
            i += 1;
            if let Some(h) = args.get(i) {
                home_override = Some(PathBuf::from(h));
            }
        } else if let Some(h) = args[i].strip_prefix("--home=") {
            home_override = Some(PathBuf::from(h));
        } else if args[i] == "--claude-config-dir" {
            i += 1;
            if let Some(d) = args.get(i) {
                config_dir_override = Some(PathBuf::from(d));
            }
        } else if let Some(d) = args[i].strip_prefix("--claude-config-dir=") {
            config_dir_override = Some(PathBuf::from(d));
        } else {
            paths.push(args[i].clone());
        }
        i += 1;
    }
    let claude_json = if let Some(cfg_dir) = config_dir_override.or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())) {
        cfg_dir.join(".claude.json")
    } else {
        let home = home_override.unwrap_or_else(|| Places::here().home);
        home.join(".claude.json")
    };
    match trust_claude_paths(&claude_json, &paths) {
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

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct InspectItemRequest {
    pub id: String,
    pub cli: String,
    #[serde(default)]
    pub shell_pid: i32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub known_session_id: Option<String>,
    #[serde(default)]
    pub known_transcript: Option<String>,
    /// (Taken from whoever still sends it, and not used: a conversation
    /// was once guessed by when it was written.)
    #[serde(default)]
    #[allow(dead_code)]
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize)]
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
    /// Its command, whole: its tail is what tells a test run from a
    /// server (it was cut at 240 characters; the ShiftUp lead's item
    /// 41, 2026-11-27). Past [`CMD_MOST`] characters it is cut all the
    /// same, and `cmd_cut` says so.
    cmd: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cmd_cut: bool,
    cwd: String,
    kind: &'static str,
    /// The file its output goes to, where it goes to one (a shell of
    /// Claude Code's own: `…/tasks/ID.output`), and when it was last
    /// written (Unix milliseconds): how long a run has said nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_wrote_ms: Option<u64>,
}

/// The command an agent's own shell runs, from the script its agent
/// gave the shell: Claude Code runs each of its commands as `bash -c
/// "source SNAPSHOT … && eval 'COMMAND' < /dev/null && pwd -P >| …"`
/// (2.1.332, read from one running). The word after the script's own
/// ` eval ` (the first that is followed by a whole quoted word and then
/// the rest of the script: one inside the command is inside its
/// quotes), as a shell reads it (quoted pieces set side by side:
/// `'it'"'"'s'`). None where the script has no such word.
fn own_shell_command(script: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = script[from..].find(" eval ") {
        let start = from + at + " eval ".len();
        if let Some((word, after)) = shell_word(&script[start..])
            && (after.is_empty() || after.starts_with(' '))
            && !word.trim().is_empty()
        {
            return Some(word);
        }
        from = start;
    }
    None
}

/// One quoted word at the start of `s` (`'…'` and `"…"` pieces side by
/// side) and what follows it; None if it doesn't begin with a quote or
/// a quote isn't closed.
fn shell_word(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = s.char_indices().peekable();
    let mut end = 0;
    loop {
        match chars.peek().copied() {
            Some((_, '\'')) => {
                chars.next();
                loop {
                    match chars.next()? {
                        (i, '\'') => {
                            end = i + 1;
                            break;
                        }
                        (_, c) => out.push(c),
                    }
                }
            }
            Some((_, '"')) => {
                chars.next();
                loop {
                    match chars.next()? {
                        (i, '"') => {
                            end = i + 1;
                            break;
                        }
                        (_, '\\') => out.push(chars.next()?.1),
                        (_, c) => out.push(c),
                    }
                }
            }
            _ => break,
        }
    }
    (end > 0).then(|| (out, &s[end..]))
}

/// Where a process's output goes, if to a file, and when that file was
/// last written.
fn output_of(pid: i32) -> (Option<String>, Option<u64>) {
    let Ok(target) = std::fs::read_link(format!("/proc/{pid}/fd/1")) else { return (None, None) };
    let Ok(meta) = std::fs::metadata(&target) else { return (None, None) };
    if !meta.is_file() {
        return (None, None);
    }
    let wrote = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
    (Some(target.display().to_string()), wrote)
}

#[derive(Debug, Serialize)]
pub(crate) struct InspectItemResult {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<i32>,
    alive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cli_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_confidence: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_message: Option<String>,
    transcript_state: &'static str,
    /// When the last turn in its conversation's file began ([`TranscriptInspection`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_started_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_tool: Option<PendingToolInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pending_tools: Vec<PendingToolInfo>,
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

/// Resolves `Places` (`HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`) from `/proc/<pid>/environ`
/// when available so subagents launched with custom `HOME` / config dirs still resolve their
/// real transcripts and session IDs.
fn places_for_pid(pid: Option<i32>, fallback: &Places) -> Places {
    let Some(p) = pid else {
        return Places { home: fallback.home.clone(), claude: fallback.claude.clone(), codex: fallback.codex.clone() };
    };
    let Ok(raw) = std::fs::read(format!("/proc/{p}/environ")) else {
        return Places { home: fallback.home.clone(), claude: fallback.claude.clone(), codex: fallback.codex.clone() };
    };
    let mut env_home: Option<PathBuf> = None;
    let mut env_claude: Option<PathBuf> = None;
    let mut env_codex: Option<PathBuf> = None;
    for entry in raw.split(|&b| b == 0) {
        if let Some(eq) = entry.iter().position(|&b| b == b'=') {
            let k = &entry[..eq];
            let v = String::from_utf8_lossy(&entry[eq + 1..]);
            if v.is_empty() {
                continue;
            }
            match k {
                b"HOME" => env_home = Some(PathBuf::from(v.as_ref())),
                b"CLAUDE_CONFIG_DIR" => env_claude = Some(PathBuf::from(v.as_ref())),
                b"CODEX_HOME" => env_codex = Some(PathBuf::from(v.as_ref())),
                _ => {}
            }
        }
    }
    let home = env_home.unwrap_or_else(|| fallback.home.clone());
    let claude = env_claude.unwrap_or_else(|| home.join(".claude"));
    let codex = env_codex.unwrap_or_else(|| home.join(".codex"));
    Places { home, claude, codex }
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

/// The most of a background shell's command that is told: a heredoc of
/// a megabyte isn't sent with every `status`.
const CMD_MOST: usize = 32_000;

/// A command as it is told: whole, or its first [`CMD_MOST`]
/// characters and that it was cut.
fn whole_cmd(cmd: &str) -> (String, bool) {
    match cmd.char_indices().nth(CMD_MOST) {
        Some((at, _)) => (cmd[..at].to_owned(), true),
        None => (cmd.to_owned(), false),
    }
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

/// Whether a process under an agent is the agent program's own doing,
/// not a run of the agent's: a server or helper its CLI keeps (an MCP
/// server, a language server, its search), or a bare `sleep`. Codex
/// keeps one of its own for as long as it runs
/// (`…/.codex/packages/…/bin/codex-code-mode-host --listen …`): told as
/// the agent's task, every Codex was "at rest on a run of its own" for
/// good, so a reviewer that had written its review and said DONE was
/// never seen to have finished, and a lead's list said `waiting_on:
/// own_run` of every one (the ShiftUp lead's item 51, and a review of
/// Olo's own lost for it, 2026-11-27). Anything run from the CLI's own
/// package is the CLI's.
fn clis_own_process(cmd: &str, comm: &str, argc: usize) -> bool {
    let low = cmd.to_ascii_lowercase();
    low.contains("olo-rex-link")
        || low.contains("ripgrep")
        || low.contains("app-server")
        || low.contains("mcp-server")
        || low.contains("modelcontextprotocol")
        || low.contains("language-server")
        || low.contains("rust-analyzer")
        || low.contains("pyright")
        || low.contains("gopls")
        || low.contains("clangd")
        || low.contains("tsserver")
        || low.contains("playwright-mcp")
        || low.contains("codex-code-mode-host")
        || low.split_whitespace().next().is_some_and(|program| program.contains("/.codex/packages/"))
        || (comm == "sleep" && argc == 2)
}

fn find_background_shells(cli: &str, agent_pid: i32, by_ppid: &HashMap<i32, Vec<&Proc>>) -> Vec<BackgroundShellInfo> {
    let mut out = Vec::new();
    // The agent's own shells first, each one whole: a shell the agent
    // started for a command of its own is what it waits on, whatever
    // runs in it at this moment. Read leaf by leaf (below), a shell
    // whose command was `sleep 45`, or a loop that sleeps between its
    // looks, had nothing to show: a bare `sleep` is skipped there, and
    // the shell above it as a wrapper (the ShiftUp lead's item 26,
    // 2026-11-26: a Claude Code agent with "1 shell" in its footer had
    // `background_shells: []` and `waiting_on: someone`).
    let mut own: HashSet<i32> = HashSet::new();
    for shell in by_ppid.get(&agent_pid).into_iter().flatten() {
        if !matches!(shell.comm.as_str(), "sh" | "bash" | "zsh" | "dash") {
            continue;
        }
        let argv = read_argv(shell.pid);
        let Some(cmd) = argv.iter().position(|a| a == "-c").and_then(|i| argv.get(i + 1..)).and_then(|rest| rest.iter().find(|a| !a.starts_with('-'))).and_then(|script| own_shell_command(script))
        else {
            continue;
        };
        let (output, last_wrote_ms) = output_of(shell.pid);
        own.insert(shell.pid);
        own.extend(collect_descendants(shell.pid, by_ppid).iter().map(|p| p.pid));
        let (told, cmd_cut) = whole_cmd(&cmd);
        out.push(BackgroundShellInfo { pid: shell.pid, kind: classify_bg_process(&cmd), cmd: told, cmd_cut, cwd: read_cwd(shell.pid), output, last_wrote_ms });
    }
    // Claude Code's shells of the agent's own are those, and all of
    // them: what else runs under it is Claude Code's own doing (as it
    // starts: `ps aux | grep -E code|cursor|…` to see which editor it is
    // in; later its searches, its servers), which read leaf by leaf was
    // said to be the agent's run, and the agent to be waiting on it.
    if cli == "claude" {
        return out;
    }
    let desc = collect_descendants(agent_pid, by_ppid);
    for p in desc {
        if own.contains(&p.pid) {
            continue;
        }
        let argv = read_argv(p.pid);
        if argv.is_empty() {
            continue;
        }
        let cmd = argv.join(" ");
        if clis_own_process(&cmd, &p.comm, argv.len()) {
            continue;
        }
        // Skip wrapper shells whose direct child is already in the list (keep the leaf command).
        let is_wrapper = matches!(p.comm.as_str(), "sh" | "bash" | "zsh" | "dash") && by_ppid.get(&p.pid).is_some_and(|k| !k.is_empty());
        if is_wrapper {
            continue;
        }
        let kind = classify_bg_process(&cmd);
        let cwd = read_cwd(p.pid);
        let (output, last_wrote_ms) = output_of(p.pid);
        let (cmd, cmd_cut) = whole_cmd(&cmd);
        out.push(BackgroundShellInfo { pid: p.pid, cmd, cmd_cut, cwd, kind, output, last_wrote_ms });
    }
    out
}

/// Finds the primary agent process (preferring the leaf agent process when behind a wrapper script)
/// AND returns all matching `(pid, start_ticks)` in the pane's process tree so Claude's PID record
/// and open FDs are checked across wrappers as well as children.
fn find_agent_proc<'a>(item: &InspectItemRequest, by_pid: &HashMap<i32, &'a Proc>, by_ppid: &HashMap<i32, Vec<&'a Proc>>) -> Option<(&'a Proc, Vec<String>, Vec<(i32, u64)>)> {
    if item.shell_pid > 0 {
        let mut candidates: Vec<&Proc> = Vec::new();
        if let Some(&root) = by_pid.get(&item.shell_pid) {
            candidates.push(root);
        }
        candidates.extend(collect_descendants(item.shell_pid, by_ppid));
        let mut matches: Vec<(&Proc, Vec<String>, String)> = Vec::new();
        for p in candidates {
            let argv = read_argv(p.pid);
            let exe = read_exe(p.pid);
            if let Some((kind, args)) = agent_of(&argv, &p.comm, &exe)
                && kind == item.cli
            {
                matches.push((p, args, exe));
            }
        }
        if !matches.is_empty() {
            let pids: Vec<(i32, u64)> = matches.iter().map(|(p, _, _)| (p.pid, start_ticks(p.pid))).collect();
            // Prefer the deepest/leaf matching agent process across wrapper
            // scripts/interpreters (e.g. bash/node wrapper -> agent binary),
            // without stepping down into a child with the same executable
            // (a fork-before-exec or sub-invocation of the agent itself).
            // A step down is taken only to a process that is still what
            // it was read as, read again whole (its name, its command
            // line, its executable): a child read between its fork and
            // its exec has the agent's command line and name for that
            // instant and, a moment later, another executable (a shell's):
            // read one after the other it passed for the agent's own
            // program behind a wrapper, and the shell it became was then
            // looked under for itself (a code review, 2026-11-27).
            let still = |i: usize| {
                let (p, _, exe) = &matches[i];
                let comm = crate::procs::proc_stat_of(p.pid).map(|now| now.comm).unwrap_or_default();
                let exe_now = read_exe(p.pid);
                exe_now == *exe && agent_of(&read_argv(p.pid), &comm, &exe_now).is_some_and(|(kind, _)| kind == item.cli)
            };
            let mut chosen = 0usize;
            for i in 1..matches.len() {
                if !matches[i].2.is_empty() && matches[i].2 != matches[chosen].2 && still(i) {
                    chosen = i;
                }
            }
            let (leaf, args, _) = matches.swap_remove(chosen);
            return Some((leaf, args, pids));
        }
    }
    // Fallback when shell_pid wasn't known: match by known_session_id in argv across all processes.
    // (Only then. Where the pane's own process is known and no agent is
    // under it, a process elsewhere with that id in its command line is
    // another agent's, however the id came to be this child's.)
    if item.shell_pid <= 0
        && let Some(ref sid) = item.known_session_id
        && !sid.is_empty()
    {
        for &p in by_pid.values() {
            let argv = read_argv(p.pid);
            if argv.iter().any(|a| a.contains(sid.as_str())) {
                let exe = read_exe(p.pid);
                if let Some((kind, args)) = agent_of(&argv, &p.comm, &exe)
                    && kind == item.cli
                {
                    let pids = vec![(p.pid, start_ticks(p.pid))];
                    return Some((p, args, pids));
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_session_and_transcript(
    places: &Places,
    cli: &str,
    agent_pids: &[(i32, u64)],
    cwd: &str,
    args: &[String],
    known_sid: Option<&str>,
    known_transcript: Option<&str>,
    rollouts: &HashMap<String, PathBuf>,
    codex_open: &mut Option<Vec<(String, String)>>,
    taken: &mut HashSet<String>,
) -> (Option<String>, Option<PathBuf>, &'static str) {
    let mut sid: Option<String> = None;
    let mut confidence: &'static str = "none";

    // 1. Surest evidence first: Claude's own per-PID session record (checked across all agent PIDs in tree).
    if cli == "claude" {
        for &(p, st) in agent_pids {
            if let Some(rec) = claude_record(places, p, st) {
                sid = Some(rec);
                confidence = "record";
                break;
            }
        }
    }

    // 2. What its own process holds open: for `agy`, the lock on the
    //    conversation that is its own (`presence/<UUID>.lock`,
    //    `Places::agy_presence`) and nothing else. Its databases and its
    //    logs are no evidence: an agy that has just started, with no
    //    conversation yet, opens the others' `conversations/<UUID>.db`,
    //    and was said to be in one of theirs. For `codex`, its rollout
    //    (`sessions/...-<UUID>.jsonl`). Across the agent's processes in
    //    the pane's tree.
    if sid.is_none() && cli == "agy" {
        let presence_dir = places.agy_presence();
        // (Two locks held, as between one conversation and the next: the one locked last.)
        let mut held: Option<(SystemTime, String)> = None;
        for &(p, _) in agent_pids {
            let Ok(rd) = std::fs::read_dir(format!("/proc/{p}/fd")) else { continue };
            for entry in rd.flatten() {
                let Ok(target) = std::fs::read_link(entry.path()) else { continue };
                if target.starts_with(&presence_dir)
                    && let Some(stem) = target.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".lock"))
                    && uuid(stem)
                {
                    let at = std::fs::metadata(&target).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    if held.as_ref().is_none_or(|(was, _)| at >= *was) {
                        held = Some((at, stem.to_owned()));
                    }
                }
            }
        }
        if let Some((_, id)) = held {
            sid = Some(id);
            confidence = "lock";
        }
    }

    if sid.is_none() && cli == "codex" {
        let codex_sessions = places.codex.join("sessions");
        for &(p, _) in agent_pids {
            let Ok(rd) = std::fs::read_dir(format!("/proc/{p}/fd")) else { continue };
            for entry in rd.flatten() {
                let Ok(t) = std::fs::read_link(entry.path()) else { continue };
                if t.extension().is_some_and(|ex| ex == "jsonl")
                    && t.starts_with(&codex_sessions)
                    && let Some(stem) = t.file_stem().and_then(|s| s.to_str())
                    && stem.len() >= 36
                {
                    let maybe = &stem[stem.len() - 36..];
                    if uuid(maybe) {
                        sid = Some(maybe.to_owned());
                        confidence = "open";
                        break;
                    }
                }
            }
            if sid.is_some() {
                break;
            }
        }
    }

    // 3. Explicit CLI argv session id (`--resume`, `--session-id`, `--conversation`), skipping if forked!
    let is_fork_argv = args.iter().any(|a| a == "--fork-session" || a == "fork");
    if sid.is_none() && !is_fork_argv {
        if let Some(a_sid) = argv_session(cli, args) {
            sid = Some(a_sid);
            confidence = "argv";
        }
    }

    // 4. Caller-known session ID (when not superseded by surer record/open evidence).
    if sid.is_none()
        && let Some(ks) = known_sid.filter(|s| !s.is_empty())
    {
        sid = Some(ks.to_owned());
        confidence = "known";
    }

    if let Some(ref s) = sid {
        taken.insert(s.clone());
    }

    if let Some(tp) = known_transcript.filter(|s| !s.is_empty()) {
        let p = PathBuf::from(tp);
        // Only trust cached transcript if it matches any newly discovered `sid`!
        let matches_sid = sid.as_deref().is_none_or(|s| tp.contains(s));
        if p.exists() && matches_sid {
            return (sid, Some(p), confidence);
        }
    }

    let transcript = match cli {
        "claude" => {
            let proj = places.claude.join("projects").join(project_name(cwd));
            // (Its own, by its record or its command line, or none: the
            // newest written in its folder since it started may be any
            // other session's there. A child with no conversation yet
            // was shown the lead's own last message as its own,
            // 2026-11-26.)
            sid.as_ref().map(|id| proj.join(format!("{id}.jsonl"))).filter(|p| p.exists())
        }
        "codex" => {
            if let Some(ref id) = sid
                && let Some(p) = rollouts.get(id)
                && p.exists()
            {
                Some(p.clone())
            } else {
                // Ask Codex's app-server daemon for open threads in `cwd` (excludes already-taken IDs!).
                let threads = codex_open.get_or_insert_with(|| codex_threads(places));
                let mut from_daemon: Option<PathBuf> = None;
                for (tid, tcwd) in threads.iter() {
                    if tcwd == cwd && !taken.contains(tid) {
                        taken.insert(tid.clone());
                        sid = Some(tid.clone());
                        confidence = "daemon";
                        from_daemon = rollouts.get(tid).filter(|p| p.exists()).cloned();
                        break;
                    }
                }
                // (No further: the newest rollout written since it
                // started may be any other Codex's.)
                from_daemon
            }
        }
        "agy" => {
            // (Its own, by the lock it holds or its command line, or
            // none until it has one: the newest written since it
            // started is, as often as not, another agy's.)
            sid.as_ref().map(|id| places.agy_brain().join(id).join(".system_generated/logs/transcript.jsonl")).filter(|p| p.exists())
        }
        _ => None,
    };

    (sid, transcript, confidence)
}

struct TranscriptInspection {
    state: &'static str,
    last_message: Option<String>,
    error: Option<String>,
    pending_tool: Option<PendingToolInfo>,
    pending_tools: Vec<PendingToolInfo>,
    /// When the last turn in the file began, by the file's own word
    /// (Claude Code: the time on the person's last message; Codex: on
    /// its last `task_started`): what tells this turn from the one
    /// before it in a conversation taken up again. None where the file
    /// doesn't say, or the turn's beginning is further back than what
    /// is read of the file.
    turn_started_ms: Option<u64>,
}

/// A time as the agents write it in their files ("2026-11-27T13:04:34.832Z",
/// in UTC), in milliseconds since 1970.
fn iso_ms(s: &str) -> Option<u64> {
    // (In UTC and said so, as both write it. A time with an offset of
    // its own, "13:00:00+01:00", read as UTC would be hours out, and a
    // turn of before would pass for one since: not read at all. A code
    // review, 2026-11-27.)
    let (date, time) = s.trim().strip_suffix('Z')?.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, sec) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || !(0..24).contains(&h) || !(0..60).contains(&min) || !(0..61).contains(&sec) || y < 1970 {
        return None;
    }
    let ms: i64 = format!("{:0<3}", frac.chars().take(3).collect::<String>()).parse().ok()?;
    // (Days from 1970 to a date: the civil-from-days rule, the other way.)
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + day - 719_469;
    u64::try_from(((days * 24 + h) * 60 + min) * 60_000 + sec * 1000 + ms).ok()
}

fn inspect_claude_transcript(path: &Path) -> TranscriptInspection {
    // Read last 512 KiB so even long final assistant messages & tool args are complete!
    let tail = read_tail_text(path, 512 * 1024);
    let mut state: &'static str = "unknown";
    let mut error: Option<String> = None;
    let mut pending_tools: Vec<PendingToolInfo> = Vec::new();
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

    // When the last turn began: the time on the person's last message
    // (not on what a tool gave back, which is written as theirs too, nor
    // on what Claude Code writes of itself in their name).
    let turn_started_ms = parsed
        .iter()
        .rev()
        .find(|v| {
            v["type"].as_str() == Some("user")
                && !v["isMeta"].as_bool().unwrap_or(false)
                && !v.pointer("/message/content").and_then(Value::as_array).is_some_and(|parts| !parts.is_empty() && parts.iter().all(|p| p["type"].as_str() == Some("tool_result")))
                // (What Claude Code writes in their name *begins* so. A
                // person's own message that speaks of such a tag,
                // "Explain <command-name> in this transcript", is theirs.)
                && !extract_claude_text(v).is_some_and(|t| t.trim_start().starts_with("<local-command-") || t.trim_start().starts_with("<command-name>"))
        })
        .and_then(|v| v["timestamp"].as_str())
        .and_then(iso_ms);

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
                let raw_txt = extract_claude_text(v).unwrap_or_default();
                let is_meta = v["isMeta"].as_bool().unwrap_or(false) || raw_txt.contains("<local-command-") || raw_txt.contains("<command-name>");
                if raw_txt.contains("[Request interrupted by user") {
                    state = "aborted";
                } else if is_meta {
                    state = "idle";
                } else {
                    state = "working";
                }
                break;
            }
            "assistant" => {
                if v["isApiErrorMessage"].as_bool().unwrap_or(false) {
                    state = "error";
                    error = extract_claude_text(v).or_else(|| Some("API Error".into()));
                    break;
                }
                if pending_tools.is_empty()
                    && let Some(arr) = v.pointer("/message/content").and_then(Value::as_array)
                {
                    for b in arr {
                        if b["type"].as_str() == Some("tool_use") {
                            let tid = b["id"].as_str().unwrap_or("");
                            if tid.is_empty() || !completed_tool_ids.contains(tid) {
                                let tool = b["name"].as_str().unwrap_or("tool").to_owned();
                                let input = b.get("input").cloned();
                                let command = input.as_ref().and_then(|i| i["command"].as_str().or_else(|| i["file_path"].as_str()).or_else(|| i["path"].as_str())).map(str::to_owned);
                                pending_tools.push(PendingToolInfo { tool, command, arguments: input });
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
                        let asks_user = pending_tools.iter().any(|t| matches!(t.tool.as_str(), "AskUserQuestion" | "ExitPlanMode"));
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

    let pending_tool = pending_tools.first().cloned();
    TranscriptInspection { state, last_message, error, pending_tool, pending_tools, turn_started_ms }
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
    let mut pending_tools: Vec<PendingToolInfo> = Vec::new();
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
                pending_tools.clear();
                error = None;
            }
            "PLANNER_RESPONSE" => {
                if let Some(txt) = v["modified_response"].as_str().or_else(|| v["response"].as_str()).or_else(|| v["content"].as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                    last_message = Some(txt.to_owned());
                }
                if let Some(calls) = v["tool_calls"].as_array()
                    && !calls.is_empty()
                {
                    pending_count = calls.len();
                    let asking = calls.iter().any(|c| c["name"].as_str() == Some("ask_question"));
                    pending_tools.clear();
                    for call in calls {
                        let tool = call["name"].as_str().unwrap_or("tool").to_owned();
                        let args = call.get("arguments").cloned().or_else(|| call.get("args").cloned());
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
                        pending_tools.push(PendingToolInfo { tool, command, arguments: args });
                    }
                    state = if asking { "waiting" } else { "working" };
                } else if st == "ERROR" || st == "FAILED" {
                    state = "error";
                    error = v["error"].as_str().map(str::to_owned).or_else(|| Some("Antigravity turn failed".into()));
                } else {
                    pending_count = 0;
                    pending_tools.clear();
                    state = "done";
                }
            }
            "GENERIC" | "TOOL_RESPONSE" => {
                if st != "RUNNING" {
                    pending_count = pending_count.saturating_sub(1);
                    if pending_count == 0 {
                        pending_tools.clear();
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

    let pending_tool = pending_tools.first().cloned();
    TranscriptInspection { state, last_message, error, pending_tool, pending_tools, turn_started_ms: None }
}

fn inspect_codex_transcript(path: &Path) -> TranscriptInspection {
    let tail = read_tail_text(path, 512 * 1024);
    let mut state: &'static str = "unknown";
    let mut last_message: Option<String> = None;
    let mut error: Option<String> = None;
    let mut pending_tools: Vec<PendingToolInfo> = Vec::new();
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
            } else if p["type"].as_str() == Some("task_complete")
                && let Some(m) = p["last_agent_message"].as_str().filter(|s| !s.trim().is_empty())
            {
                // (A turn's end names its last words itself.)
                last_message = Some(m.trim().to_owned());
            } else if p["type"].as_str() == Some("task_started") {
                // (A turn's words are its own: one that ends with none,
                // or is cut short, has none, not the turn before's. A
                // code review, 2026-11-27: "a silent turn can succeed
                // with a previous turn's text".)
                last_message = None;
            }
        }
    }

    // When the last turn began: the time on its last `task_started`.
    let turn_started_ms =
        parsed.iter().rev().find(|v| v["type"].as_str() == Some("event_msg") && v["payload"]["type"].as_str() == Some("task_started")).and_then(|v| v["timestamp"].as_str()).and_then(iso_ms);

    // Collect all uncompleted function calls in order for H14 screen-matching.
    for v in &parsed {
        if v["type"].as_str() == Some("response_item") {
            let p = &v["payload"];
            if matches!(p["type"].as_str(), Some("function_call" | "custom_tool_call")) {
                let cid = p["call_id"].as_str().unwrap_or("");
                if cid.is_empty() || !completed_calls.contains(cid) {
                    let tool = p["name"].as_str().unwrap_or("tool").to_owned();
                    let args_val = p["arguments"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()).or_else(|| p.get("arguments").cloned());
                    let command = args_val.as_ref().and_then(|a| a["cmd"].as_str().or_else(|| a["command"].as_str())).map(str::to_owned);
                    pending_tools.push(PendingToolInfo { tool, command, arguments: args_val });
                }
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
                    "task_complete" => {
                        // (Codex 0.172 ends a turn that failed, a refused
                        // request or a stream cut short past its retries,
                        // with `task_complete` too, the error in it: read
                        // as done, with no error. A second investigator,
                        // with the real Codex, 2026-11-27.)
                        if let Some(failed) = p.get("error").filter(|e| !e.is_null()) {
                            state = "error";
                            error = failed["message"].as_str().or_else(|| failed.as_str()).map(str::to_owned).or_else(|| Some("the turn failed".into()));
                        } else {
                            state = "done";
                        }
                        break;
                    }
                    "turn_aborted" => {
                        state = "aborted";
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

    let pending_tool = pending_tools.first().cloned();
    TranscriptInspection { state, last_message, error, pending_tool, pending_tools, turn_started_ms }
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
    println!("{}", json!({ "ok": true, "items": inspect_items(&req.items) }));
    0
}

/// Each child's agent as this host has it now: its process, the
/// conversation it is in and where that is written, its last message
/// whole, the tool call it waits on, the programs it runs. One pass over
/// `/proc` for all of them.
pub(crate) fn inspect_items(items: &[InspectItemRequest]) -> Vec<InspectItemResult> {
    inspect_items_with(items, |_| {})
}

fn inspect_items_with(items: &[InspectItemRequest], mut before_transcript: impl FnMut(Option<i32>)) -> Vec<InspectItemResult> {
    fn build_index(procs: &[Proc]) -> (HashMap<i32, &Proc>, HashMap<i32, Vec<&Proc>>) {
        let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let mut by_ppid: HashMap<i32, Vec<&Proc>> = HashMap::new();
        for p in procs {
            by_ppid.entry(p.ppid).or_default().push(p);
        }
        (by_pid, by_ppid)
    }

    let places = Places::here();
    let mut global_procs: Option<Vec<Proc>> = None;
    let need_codex = items.iter().any(|i| i.cli == "codex");
    let rollouts = if need_codex { codex_rollouts(&places) } else { HashMap::new() };
    let mut codex_open: Option<Vec<(String, String)>> = None;
    let mut taken: HashSet<String> = HashSet::new();

    for item in items {
        if let Some(ref ks) = item.known_session_id
            && !ks.is_empty()
        {
            taken.insert(ks.clone());
        }
    }

    // Phase 1: locate each agent process (walking only `shell_pid`'s subtree
    // when known), resolve its session/transcript, and read its transcript.
    struct Pending<'a> {
        item: &'a InspectItemRequest,
        pid: Option<i32>,
        cli_session_id: Option<String>,
        session_confidence: &'static str,
        transcript_path: Option<PathBuf>,
        inspected: TranscriptInspection,
    }
    let mut pending = Vec::with_capacity(items.len());
    for item in items {
        let (pid, proc_cwd, args, agent_pids) = {
            let local = if item.shell_pid > 0 { subtree_procs(item.shell_pid) } else { None };
            let procs_ref: &[Proc] = match &local {
                Some(v) => v.as_slice(),
                None => global_procs.get_or_insert_with(all_procs_stat).as_slice(),
            };
            let (by_pid, by_ppid) = build_index(procs_ref);
            match find_agent_proc(item, &by_pid, &by_ppid) {
                Some((p, args, pids)) => (Some(p.pid), read_cwd(p.pid), args, pids),
                None => (None, String::new(), Vec::new(), Vec::new()),
            }
        };
        let cwd = if !proc_cwd.is_empty() { proc_cwd } else { item.cwd.clone() };
        let item_places = places_for_pid(pid, &places);
        let item_rollouts = if item.cli == "codex" && item_places.codex != places.codex { Some(codex_rollouts(&item_places)) } else { None };
        let active_rollouts = item_rollouts.as_ref().unwrap_or(&rollouts);
        let mut custom_codex_open: Option<Vec<(String, String)>> = None;
        let active_codex_open = if item_places.codex != places.codex { &mut custom_codex_open } else { &mut codex_open };

        let (cli_session_id, transcript_path, session_confidence) = resolve_session_and_transcript(
            &item_places,
            &item.cli,
            &agent_pids,
            &cwd,
            &args,
            item.known_session_id.as_deref(),
            item.known_transcript.as_deref(),
            active_rollouts,
            active_codex_open,
            &mut taken,
        );

        before_transcript(pid);

        let inspected = match transcript_path.as_deref() {
            Some(p) if item.cli == "claude" => inspect_claude_transcript(p),
            Some(p) if item.cli == "agy" => inspect_agy_transcript(p),
            Some(p) if item.cli == "codex" => inspect_codex_transcript(p),
            _ => TranscriptInspection { state: "unknown", last_message: None, error: None, pending_tool: None, pending_tools: Vec::new(), turn_started_ms: None },
        };

        pending.push(Pending { item, pid, cli_session_id, session_confidence, transcript_path, inspected });
    }

    // Phase 2: read each agent's live child processes STRICTLY AFTER reading
    // its transcript so any background shell started before the turn ended is
    // never missed due to a stale pre-transcript `/proc` snapshot.
    let mut fresh_global: Option<Vec<Proc>> = None;
    let mut results = Vec::with_capacity(pending.len());
    for p_item in pending {
        let background_shells = match p_item.pid {
            Some(p) => {
                let local = subtree_procs(p);
                let procs_ref: &[Proc] = match &local {
                    Some(v) => v.as_slice(),
                    None => fresh_global.get_or_insert_with(all_procs_stat).as_slice(),
                };
                let (_, fresh_by_ppid) = build_index(procs_ref);
                find_background_shells(&p_item.item.cli, p, &fresh_by_ppid)
            }
            None => Vec::new(),
        };
        let has_active_task = background_shells.iter().any(|s| s.kind == "task");
        let waiting_on = if p_item.pid.is_none() {
            None
        } else if has_active_task {
            Some("own_run")
        } else if matches!(p_item.inspected.state, "done" | "waiting" | "aborted" | "idle") {
            Some("someone")
        } else {
            None
        };

        results.push(InspectItemResult {
            id: p_item.item.id.clone(),
            pid: p_item.pid,
            alive: p_item.pid.is_some(),
            cli_session_id: p_item.cli_session_id,
            session_confidence: (p_item.session_confidence != "none").then_some(p_item.session_confidence),
            transcript_path: p_item.transcript_path.map(|p| p.display().to_string()),
            last_message: p_item.inspected.last_message,
            transcript_state: p_item.inspected.state,
            turn_started_ms: p_item.inspected.turn_started_ms,
            transcript_error: p_item.inspected.error,
            pending_tool: p_item.inspected.pending_tool,
            pending_tools: p_item.inspected.pending_tools,
            background_shells,
            waiting_on,
        });
    }
    results
}

/// Marks folders trusted in Claude Code's own file, for the account
/// (or the home and configuration folder a child was given).
pub(crate) fn trust(home_override: Option<&str>, config_dir_override: Option<&str>, paths: &[String]) -> io::Result<Vec<String>> {
    let claude_json = match config_dir_override.map(PathBuf::from).or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())) {
        Some(cfg_dir) => cfg_dir.join(".claude.json"),
        None => home_override.map(PathBuf::from).unwrap_or_else(|| Places::here().home).join(".claude.json"),
    };
    trust_claude_paths(&claude_json, paths)
}

#[cfg(test)]
mod tests {
    /// A background shell's command is told whole (it was cut at 240 characters), up to a cap that says so.
    /// When the last turn in a conversation's file began, by the file's own word.
    #[test]
    fn a_files_last_turn_says_when_it_began() {
        assert_eq!(iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(iso_ms("2026-11-27T13:04:34.832Z"), Some(1_795_784_674_832));
        assert_eq!(iso_ms("2024-02-29T23:59:59Z"), Some(1_709_251_199_000));
        assert_eq!(iso_ms("2026-11-27 13:04"), None);
        // (With an offset of its own, or none said: not read, rather than read as UTC.)
        assert_eq!(iso_ms("2026-11-27T13:00:00.250+01:00"), None);
        assert_eq!(iso_ms("2026-11-27T13:00:00"), None);
        // (A folder of this run's own, on disk: never one left by another run, never in /tmp.)
        let dir =
            std::path::PathBuf::from(format!("/var/tmp/olo-turn-began-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
        std::fs::create_dir(&dir).unwrap();
        // Claude Code: the person's last message; not what a tool gave back, nor what Claude Code writes in
        // their name, though both are later.
        let claude = dir.join("c.jsonl");
        let lines = [
            json!({ "type": "user", "timestamp": "2026-11-27T10:00:00.000Z", "message": { "role": "user", "content": "The first thing." } }),
            json!({ "type": "assistant", "timestamp": "2026-11-27T10:00:05.000Z", "message": { "id": "m1", "role": "assistant", "content": [{ "type": "text", "text": "Done." }], "stop_reason": "end_turn" } }),
            json!({ "type": "user", "timestamp": "2026-11-27T11:00:00.250Z", "message": { "role": "user", "content": [{ "type": "text", "text": "The second thing." }] } }),
            json!({ "type": "assistant", "timestamp": "2026-11-27T11:00:01.000Z", "message": { "id": "m2", "role": "assistant", "content": [{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "true" } }], "stop_reason": "tool_use" } }),
            json!({ "type": "user", "timestamp": "2026-11-27T11:00:02.000Z", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "" }] } }),
            json!({ "type": "user", "isMeta": true, "timestamp": "2026-11-27T11:00:03.000Z", "message": { "role": "user", "content": "Caveat: what follows was written by Claude Code." } }),
            json!({ "type": "assistant", "timestamp": "2026-11-27T11:00:04.000Z", "message": { "id": "m3", "role": "assistant", "content": [{ "type": "text", "text": "Second done." }], "stop_reason": "end_turn" } }),
        ];
        std::fs::write(&claude, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let read = inspect_claude_transcript(&claude);
        assert_eq!((read.state, read.turn_started_ms), ("done", iso_ms("2026-11-27T11:00:00.250Z")));
        // A person's own message that speaks of one of Claude Code's tags is theirs; what begins with the tag is not.
        let more = [
            json!({ "type": "user", "timestamp": "2026-11-27T12:00:00.000Z", "message": { "role": "user", "content": "Explain <command-name> in this transcript" } }),
            json!({ "type": "user", "timestamp": "2026-11-27T12:00:09.000Z", "message": { "role": "user", "content": "<command-name>/effort</command-name>" } }),
        ];
        std::fs::write(&claude, lines.iter().chain(more.iter()).map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        assert_eq!(inspect_claude_transcript(&claude).turn_started_ms, iso_ms("2026-11-27T12:00:00.000Z"));
        // Codex: its last `task_started`.
        let codex = dir.join("x.jsonl");
        let lines = [
            json!({ "timestamp": "2026-11-27T10:00:00.000Z", "type": "event_msg", "payload": { "type": "task_started" } }),
            json!({ "timestamp": "2026-11-27T10:00:09.000Z", "type": "event_msg", "payload": { "type": "task_complete", "last_agent_message": "One." } }),
            json!({ "timestamp": "2026-11-27T12:30:00.500Z", "type": "event_msg", "payload": { "type": "task_started" } }),
            json!({ "timestamp": "2026-11-27T12:30:01.000Z", "type": "response_item", "payload": { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Two." }] } }),
        ];
        std::fs::write(&codex, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let read = inspect_codex_transcript(&codex);
        assert_eq!((read.state, read.turn_started_ms), ("working", iso_ms("2026-11-27T12:30:00.500Z")));
        // A turn that failed ends with `task_complete` and its error: an error, not done.
        std::fs::write(&codex, format!("{}\n{}\n", lines[2], json!({ "timestamp": "2026-11-27T12:30:02.000Z", "type": "event_msg", "payload": { "type": "task_complete", "last_agent_message": null, "error": { "message": "stream disconnected" } } }))).unwrap();
        let read = inspect_codex_transcript(&codex);
        assert_eq!((read.state, read.error.as_deref()), ("error", Some("stream disconnected")));
        // A turn that ends with no words has none: not the words of the turn before it.
        std::fs::write(
            &codex,
            format!(
                "{}\n{}\n{}\n{}\n",
                lines[0],
                lines[1],
                lines[2],
                json!({ "timestamp": "2026-11-27T12:30:02.000Z", "type": "event_msg", "payload": { "type": "task_complete", "last_agent_message": null } })
            ),
        )
        .unwrap();
        let read = inspect_codex_transcript(&codex);
        assert_eq!((read.state, read.last_message), ("done", None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shells_command_is_told_whole() {
        let long = format!("cd /a/long/worktree && jato test -p some-package -- {} --exact the_last_word", "x".repeat(600));
        assert_eq!(super::whole_cmd(&long), (long.clone(), false));
        let (told, cut) = super::whole_cmd(&"é".repeat(super::CMD_MOST + 5));
        assert!(cut && told.chars().count() == super::CMD_MOST);
        let shell = super::BackgroundShellInfo { pid: 1, cmd: long.clone(), cmd_cut: false, cwd: String::new(), kind: "task", output: None, last_wrote_ms: None };
        let told = serde_json::to_value(&shell).unwrap();
        assert!(told["cmd"] == long.as_str() && told.get("cmd_cut").is_none(), "{told}");
    }

    use super::*;

    /// What an agent's program keeps for itself is not a run of the
    /// agent's: Codex's own helper as its process list has it, a server
    /// of the CLI's, a bare sleep. A build, a test or a script of the
    /// agent's is.
    #[test]
    fn a_clis_own_helper_is_not_the_agents_run() {
        for (cmd, comm, argc) in [
            ("/home/u/.codex/packages/standalone/releases/0.172.0-x86_64-unknown-linux-musl/bin/codex-code-mode-host --listen unix:///run/user/1000/c.sock", "codex-code-mode", 3),
            ("/home/u/.codex/packages/standalone/releases/0.173.0-x86_64-unknown-linux-musl/bin/some-later-helper", "some-later-help", 1),
            ("node /usr/lib/node_modules/@modelcontextprotocol/server-filesystem/dist/index.js /tmp", "node", 3),
            ("rust-analyzer", "rust-analyzer", 1),
            ("sleep 45", "sleep", 2),
        ] {
            assert!(clis_own_process(cmd, comm, argc), "{cmd}");
        }
        for (cmd, comm, argc) in [
            ("cargo test -p olo-rex", "cargo", 4),
            ("python3 /var/tmp/x/verify.py --all", "python3", 3),
            // (A script of the agent's that happens to be kept under a folder of that name elsewhere.)
            ("bash /srv/work/.codex/packages/run-tests.sh", "bash", 2),
            ("sleep 5 10", "sleep", 3),
        ] {
            assert!(!clis_own_process(cmd, comm, argc), "{cmd}");
        }
    }

    /// The command of an agent's own shell, from the script Claude Code
    /// gives it (2.1.332, copied from two that ran): the word after
    /// `eval`, its quoted pieces put together.
    #[test]
    fn an_own_shells_command_is_read_from_its_script() {
        let wrap = |eval: &str| {
            format!(
                "source /home/f/.claude/shell-snapshots/snapshot-bash-1795681562352-golggv.sh 2>/dev/null || true && \\builtin shopt -u extglob 2>/dev/null || true && {{ \\builtin unalias -- 'which'; \\builtin unset -f -- 'which'; }} >/dev/null 2>&1 || true && eval {eval} < /dev/null && pwd -P >| /var/tmp/claude-319e-cwd"
            )
        };
        assert_eq!(own_shell_command(&wrap("'sleep 100'")).as_deref(), Some("sleep 100"));
        // (A quote inside it, as a shell writes one: closed, quoted, opened again.)
        assert_eq!(own_shell_command(&wrap(r#"'awk '"'"'{print $4}'"'"' /proc/1/stat; echo "done"'"#)).as_deref(), Some(r#"awk '{print $4}' /proc/1/stat; echo "done""#));
        // (A command with an eval of its own: the script's is the last, the one that runs it.)
        assert_eq!(own_shell_command(&wrap(r#"'while true; do eval "$next"; sleep 5; done'"#)).as_deref(), Some(r#"while true; do eval "$next"; sleep 5; done"#));
        for none in ["sleep 100", "bash -lc tail -f log", "x && eval ", "x && eval 'unclosed"] {
            assert_eq!(own_shell_command(none), None, "{none}");
        }
    }

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

        // 4. Agy prefers transcript_full.jsonl when present and reads `content` (agy 1.5.17 schema)
        let agy_compact = dir.join("transcript.jsonl");
        let agy_full = dir.join("transcript_full.jsonl");
        std::fs::write(&agy_compact, "{\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"short...\"}\n").unwrap();
        std::fs::write(&agy_full, "{\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"DONE: full untruncated antigravity reply\\nwith multiple lines\"}\n").unwrap();
        let r_agy = inspect_agy_transcript(&agy_compact);
        assert_eq!(r_agy.state, "done");
        assert_eq!(r_agy.last_message.as_deref(), Some("DONE: full untruncated antigravity reply\nwith multiple lines"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A background shell spawned by the agent after `inspect_items` begins
    /// (while `/proc` is being scanned) and before the turn's `end_turn`
    /// assistant message is read from its transcript must be reported in
    /// `background_shells` with `waiting_on: Some("own_run")`, never missed
    /// due to reusing a stale pre-transcript `/proc` snapshot.
    #[test]
    fn a_background_shell_spawned_before_transcript_end_is_never_missed() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = PathBuf::from(format!("/var/tmp/olo-inspect-bg-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake_claude = dir.join("claude");
        let trigger = dir.join("spawn-now");
        let ready = dir.join("bg-pid");
        let sid = "11111111-2222-3333-4444-555555555555";
        let tx = dir.join(format!("{sid}.jsonl"));
        std::fs::write(&tx, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Start the wait.\"}}\n").unwrap();
        // Fake `claude` process: starts with 0 children; once `trigger` appears,
        // spawns a background bash with Claude Code's wrapper shape and records its PID.
        std::fs::write(
            &fake_claude,
            format!(
                "#!/bin/bash\nwhile [ ! -f '{}' ]; do sleep 0.01; done\n/bin/bash -c \"source /dev/null 2>/dev/null || true && eval 'echo STARTED_WAITING; sleep 45' < /dev/null && pwd -P >| /dev/null\" </dev/null >/dev/null 2>&1 &\nBG=$!\necho $BG > '{}'\nwait\n",
                trigger.display(),
                ready.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake_claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = std::process::Command::new(&fake_claude).spawn().unwrap();
        let claude_pid = child.id() as i32;
        // Wait until the script has exec'd so `/proc/{pid}/cmdline` shows `claude`.
        let start_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !read_argv(claude_pid).iter().any(|a| a.ends_with("/claude")) && std::time::Instant::now() < start_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let item = InspectItemRequest {
            id: "sa-race".into(),
            cli: "claude".into(),
            shell_pid: claude_pid,
            cwd: dir.display().to_string(),
            known_session_id: Some(sid.into()),
            known_transcript: Some(tx.display().to_string()),
            started_at: 0,
        };

        let results = inspect_items_with(&[item], |_| {
            // Simulate Claude spawning its background bash AND then writing its closing
            // `end_turn` message AFTER `inspect_items`'s initial process lookup.
            std::fs::write(&trigger, "go").unwrap();
            let dl = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let bg_pid: i32 = loop {
                if let Ok(s) = std::fs::read_to_string(&ready)
                    && let Ok(p) = s.trim().parse::<i32>()
                    && read_argv(p).iter().any(|a| a.contains("STARTED_WAITING"))
                {
                    break p;
                }
                assert!(std::time::Instant::now() < dl, "background bash did not start");
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert!(bg_pid > 0);
            let mut f = OpenOptions::new().append(true).open(&tx).unwrap();
            writeln!(f, "{{\"type\":\"assistant\",\"message\":{{\"id\":\"m1\",\"stop_reason\":\"end_turn\",\"content\":[{{\"type\":\"text\",\"text\":\"OWN_SHELL_STARTED\"}}]}}}}").unwrap();
        });

        // Clean up spawned child tree before asserting.
        if let Ok(s) = std::fs::read_to_string(&ready)
            && let Ok(bg_pid) = s.trim().parse::<i32>()
            && let Some(sub) = subtree_procs(bg_pid)
        {
            for p in sub {
                // SAFETY: killing only test-spawned descendants in `bg_pid`'s subtree.
                unsafe { libc::kill(p.pid, libc::SIGKILL) };
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.transcript_state, "done");
        assert_eq!(r.last_message.as_deref(), Some("OWN_SHELL_STARTED"));
        assert_eq!(r.background_shells.len(), 1, "expected 1 background shell, got {r:?}");
        assert_eq!(r.background_shells[0].cmd, "echo STARTED_WAITING; sleep 45");
        assert_eq!(r.waiting_on, Some("own_run"));
    }
}
