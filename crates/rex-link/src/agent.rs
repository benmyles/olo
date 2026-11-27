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
use crate::procs::{Proc, all_procs};

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
/// Matches by what the process actually *is* (its `argv[0]` basename,
/// `/proc/<pid>/exe` basename, `comm`, or for Node-hosted MCP/LSP
/// servers its script path) rather than substrings anywhere in its
/// arguments; Codex's helper and `$CODEX_HOME/packages/` root apply
/// only when `cli == "codex"`.
fn clis_own_process(cli: &str, argv: &[String], exe: &str, comm: &str, codex_home: &Path) -> bool {
    if comm == "sleep" && argv.len() == 2 {
        return true;
    }
    let prog = argv.first().map(String::as_str).unwrap_or("");
    let base_low = |p: &str| Path::new(p).file_name().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let (prog_base, exe_base, comm_low) = (base_low(prog), base_low(exe), comm.to_ascii_lowercase());
    let is_helper =
        |name: &str| matches!(name, "olo-rex-link" | "ripgrep" | "app-server" | "mcp-server" | "language-server" | "rust-analyzer" | "pyright" | "gopls" | "clangd" | "tsserver" | "playwright-mcp");
    if is_helper(&prog_base) || is_helper(&exe_base) || is_helper(&comm_low) {
        return true;
    }
    if matches!(prog_base.as_str(), "node" | "nodejs" | "bun" | "deno")
        && argv.get(1).is_some_and(|script| {
            let s = script.to_ascii_lowercase();
            s.contains("/@modelcontextprotocol/") || s.contains("/mcp-server") || s.contains("/playwright-mcp") || s.contains("/language-server") || s.contains("/tsserver")
        })
    {
        return true;
    }
    if cli == "codex" {
        if prog_base == "codex-code-mode-host" || exe_base == "codex-code-mode-host" || comm_low == "codex-code-mode" {
            return true;
        }
        let codex_pkgs = codex_home.join("packages");
        let under_codex_pkgs =
            |p: &str| !p.is_empty() && !codex_home.as_os_str().is_empty() && !Path::new(p).components().any(|c| matches!(c, std::path::Component::ParentDir)) && Path::new(p).starts_with(&codex_pkgs);
        if under_codex_pkgs(prog) || under_codex_pkgs(exe) {
            return true;
        }
    }
    false
}

fn find_background_shells(cli: &str, agent_pid: i32, places: &Places, by_ppid: &HashMap<i32, Vec<&Proc>>) -> Vec<BackgroundShellInfo> {
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
        let exe = read_exe(p.pid);
        if clis_own_process(cli, &argv, &exe, &p.comm, &places.codex) {
            continue;
        }
        let cmd = argv.join(" ");
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
        let mut matches: Vec<(&Proc, Vec<String>)> = Vec::new();
        for p in candidates {
            let argv = read_argv(p.pid);
            let exe = read_exe(p.pid);
            if let Some((kind, args)) = agent_of(&argv, &p.comm, &exe)
                && kind == item.cli
            {
                matches.push((p, args));
            }
        }
        if !matches.is_empty() {
            let pids: Vec<(i32, u64)> = matches.iter().map(|(p, _)| (p.pid, start_ticks(p.pid))).collect();
            // Prefer the deepest/leaf matching agent process (avoids shell wrapper scripts).
            let (leaf, args) = matches.pop().unwrap();
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
    TranscriptInspection { state, last_message, error, pending_tool, pending_tools }
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
    TranscriptInspection { state, last_message, error, pending_tool, pending_tools }
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
            }
        }
    }

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
                        state = "done";
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
    TranscriptInspection { state, last_message, error, pending_tool, pending_tools }
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
    struct Batch<'a> {
        items: &'a [InspectItemRequest],
    }
    let req = Batch { items };

    let places = Places::here();
    let procs = all_procs();
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut by_ppid: HashMap<i32, Vec<&Proc>> = HashMap::new();
    for p in &procs {
        by_ppid.entry(p.ppid).or_default().push(p);
    }
    let need_codex = req.items.iter().any(|i| i.cli == "codex");
    let rollouts = if need_codex { codex_rollouts(&places) } else { HashMap::new() };
    let mut codex_open: Option<Vec<(String, String)>> = None;
    let mut taken: HashSet<String> = HashSet::new();

    for item in req.items {
        if let Some(ref ks) = item.known_session_id
            && !ks.is_empty()
        {
            taken.insert(ks.clone());
        }
    }

    let mut results = Vec::with_capacity(req.items.len());
    for item in req.items {
        let found = find_agent_proc(item, &by_pid, &by_ppid);
        let (pid, proc_cwd, args, agent_pids) = match found {
            Some((p, args, pids)) => (Some(p.pid), read_cwd(p.pid), args, pids),
            None => (None, String::new(), Vec::new(), Vec::new()),
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

        let inspected = match transcript_path.as_deref() {
            Some(p) if item.cli == "claude" => inspect_claude_transcript(p),
            Some(p) if item.cli == "agy" => inspect_agy_transcript(p),
            Some(p) if item.cli == "codex" => inspect_codex_transcript(p),
            _ => TranscriptInspection { state: "unknown", last_message: None, error: None, pending_tool: None, pending_tools: Vec::new() },
        };

        let background_shells = pid.map(|p| find_background_shells(&item.cli, p, &item_places, &by_ppid)).unwrap_or_default();
        let has_active_task = background_shells.iter().any(|s| s.kind == "task");
        let waiting_on = if pid.is_none() {
            None
        } else if has_active_task {
            Some("own_run")
        } else if matches!(inspected.state, "done" | "waiting" | "aborted" | "idle") {
            Some("someone")
        } else {
            None
        };

        results.push(InspectItemResult {
            id: item.id.clone(),
            pid,
            alive: pid.is_some(),
            cli_session_id,
            session_confidence: (session_confidence != "none").then_some(session_confidence),
            transcript_path: transcript_path.map(|p| p.display().to_string()),
            last_message: inspected.last_message,
            transcript_state: inspected.state,
            transcript_error: inspected.error,
            pending_tool: inspected.pending_tool,
            pending_tools: inspected.pending_tools,
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
        let codex_home = Path::new("/home/u/.codex");
        let argv = |cmd: &str| cmd.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        for (cli, cmd, exe, comm, home) in [
            (
                "codex",
                "/home/u/.codex/packages/standalone/releases/0.172.0-x86_64-unknown-linux-musl/bin/codex-code-mode-host --listen unix:///run/user/1000/c.sock",
                "",
                "codex-code-mode",
                codex_home,
            ),
            ("codex", "/home/u/.codex/packages/standalone/releases/0.173.0-x86_64-unknown-linux-musl/bin/some-later-helper", "", "some-later-help", codex_home),
            ("codex", "/var/tmp/custom-codex/packages/standalone/bin/helper", "", "helper", Path::new("/var/tmp/custom-codex")),
            ("codex", "node /usr/lib/node_modules/@modelcontextprotocol/server-filesystem/dist/index.js /tmp", "/usr/bin/node", "node", codex_home),
            ("agy", "rust-analyzer", "/usr/bin/rust-analyzer", "rust-analyzer", codex_home),
            ("codex", "sleep 45", "/usr/bin/sleep", "sleep", codex_home),
        ] {
            assert!(clis_own_process(cli, &argv(cmd), exe, comm, home), "{cli}: {cmd}");
        }
        for (cli, cmd, exe, comm, home) in [
            ("codex", "cargo test -p olo-rex", "/usr/bin/cargo", "cargo", codex_home),
            ("codex", "python3 /var/tmp/x/verify.py --all", "/usr/bin/python3", "python3", codex_home),
            // (A script of the agent's that happens to be kept under a folder of that name elsewhere.)
            ("codex", "bash /srv/work/.codex/packages/run-tests.sh", "/usr/bin/bash", "bash", codex_home),
            // A Python test runner whose script name mentions Codex's helper must NOT be hidden.
            ("codex", "python3 test-codex-code-mode-host.py", "/usr/bin/python3", "python3", codex_home),
            // A directly executed test program under a project-local `.codex/packages/` must NOT be hidden.
            ("codex", "/srv/work/project/.codex/packages/run-tests", "/srv/work/project/.codex/packages/run-tests", "run-tests", codex_home),
            // Codex's helper exception applies only to Codex, never another CLI.
            ("agy", "/home/u/.codex/packages/standalone/releases/0.172.0-x86_64-unknown-linux-musl/bin/codex-code-mode-host --listen unix:///run/user/1000/c.sock", "", "codex-code-mode", codex_home),
            ("codex", "sleep 5 10", "/usr/bin/sleep", "sleep", codex_home),
        ] {
            assert!(!clis_own_process(cli, &argv(cmd), exe, comm, home), "{cli}: {cmd}");
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
}
