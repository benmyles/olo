//! Host Activity discovery types and incremental streaming transcript parser
//! shared by Rex's Activity sidebar, live transcript viewer, and automated tests.
//!
//! Parses real on-disk transcript streams for:
//! - **Claude Code** (`~/.claude/projects/<proj>/<id>.jsonl` and `subagents/agent-*.jsonl`)
//! - **Codex** (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`)
//! - **Antigravity CLI** (`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`)
//! - **Plain text / generic JSONL** fallback for stand-in or custom agents.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::provision::shell_word;

/// One Rex pane sent to `olo-rex-link activity watch` on stdin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneQuery {
    pub block_id: String,
    pub pid: i32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub screens: Vec<String>,
}

/// Safe read-only tmux attach target for a tmux-launched subagent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxTarget {
    pub socket: String,
    pub session: String,
    pub target: String,
    pub cols: u16,
    pub rows: u16,
    /// Client PTY geometry (`window_width × (window_height + status_rows)`) so
    /// attaching to a detached tmux session with `-r -f ignore-size` keeps the
    /// subagent's pane geometry identical down to the single row.
    #[serde(default)]
    pub pty_cols: u16,
    #[serde(default)]
    pub pty_rows: u16,
}

impl TmuxTarget {
    pub fn attach_size(&self) -> (u16, u16) {
        let cols = if self.pty_cols > 0 { self.pty_cols } else { self.cols }.clamp(20, 400);
        let rows = if self.pty_rows > 0 { self.pty_rows } else { self.rows.saturating_add(1) }.clamp(5, 200);
        (cols, rows)
    }
}

/// How a nested subagent was launched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentOrigin {
    Native,
    Shell,
    Tmux,
}

/// Live activity state of a nested subagent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentState {
    Working,
    Waiting,
    Done,
    Failed,
}

/// One nested subagent discovered under a top-level Rex pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentInfo {
    pub id: String,
    pub kind: String,
    pub origin: SubagentOrigin,
    pub name: String,
    pub description: String,
    pub state: SubagentState,
    pub pid: Option<i32>,
    pub transcript: Option<String>,
    pub tmux: Option<TmuxTarget>,
    #[serde(default)]
    pub started_ms: u64,
}

/// Discovered top-level agent metadata and nested subagents for one Rex pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneActivity {
    pub block_id: String,
    pub agent: Option<String>,
    pub pid: Option<i32>,
    pub session_id: Option<String>,
    pub transcript: Option<String>,
    #[serde(default)]
    pub subagents: Vec<SubagentInfo>,
}

/// Snapshot emitted as NDJSON by `olo-rex-link activity watch`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub panes: Vec<PaneActivity>,
}

/// Frame emitted as NDJSON by `olo-rex-link activity tail`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TailFrame {
    Meta {
        path: String,
        #[serde(default)]
        truncated: bool,
        #[serde(default)]
        offset: u64,
        #[serde(default)]
        waiting: bool,
    },
    Lines {
        lines: Vec<String>,
    },
    Reset,
    Error {
        error: String,
    },
}

/// Remote shell command to start the zero-polling Activity watcher helper.
pub fn watch_command(helper_id: &str) -> String {
    format!("{} activity watch", crate::helper::file(helper_id))
}

/// Remote shell command to stream a transcript file live via inotify.
pub fn tail_command(helper_id: &str, path: &str) -> String {
    format!("{} activity tail {}", crate::helper::file(helper_id), shell_word(path))
}

/// Safe read-only tmux attach command that never resizes the subagent's window
/// (`-r`: read-only, `-f ignore-size`: this viewer's size never counts).
pub fn tmux_attach_command(target: &TmuxTarget) -> String {
    format!("tmux -S {} attach -r -f ignore-size -t {}", shell_word(&target.socket), shell_word(&target.target))
}

// ---------------------------------------------------------------------------
// Incremental streaming transcript parser & search engine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptFormat {
    Unknown,
    Claude,
    Codex,
    Agy,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    User,
    Assistant,
    System,
}

/// One visual segment of a message body: normal prose or a fenced code block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum BodySegment {
    Text(String),
    Code { lang: String, code: String },
}

/// Outcome of a tool call once its paired result arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
}

/// One structured item in the rendered conversation stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum TranscriptEntry {
    Message { id: usize, speaker: Speaker, label: String, timestamp: String, segments: Vec<BodySegment> },
    Thinking { id: usize, summary: String, text: String, timestamp: String },
    ToolCall { id: usize, call_id: Option<String>, name: String, summary: String, input: String, output: Option<String>, status: ToolStatus, timestamp: String },
    Divider { id: usize, label: String, timestamp: String },
}

impl TranscriptEntry {
    pub fn id(&self) -> usize {
        match self {
            Self::Message { id, .. } | Self::Thinking { id, .. } | Self::ToolCall { id, .. } | Self::Divider { id, .. } => *id,
        }
    }

    pub fn is_collapsible(&self) -> bool {
        matches!(self, Self::Thinking { .. } | Self::ToolCall { .. })
    }

    /// Plain searchable text across every field of this entry.
    pub fn searchable_text(&self) -> String {
        match self {
            Self::Message { label, segments, .. } => {
                let mut out = label.clone();
                for s in segments {
                    out.push('\n');
                    match s {
                        BodySegment::Text(t) => out.push_str(t),
                        BodySegment::Code { lang, code } => {
                            out.push_str(lang);
                            out.push('\n');
                            out.push_str(code);
                        }
                    }
                }
                out
            }
            Self::Thinking { summary, text, .. } => format!("Thinking {summary}\n{text}"),
            Self::ToolCall { name, summary, input, output, .. } => {
                let mut out = format!("{name} {summary}\n{input}");
                if let Some(o) = output {
                    out.push('\n');
                    out.push_str(o);
                }
                out
            }
            Self::Divider { label, .. } => label.clone(),
        }
    }
}

/// Which sub-field of a [`TranscriptEntry`] a [`SearchMatch`] sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchField {
    MessageSegment(usize),
    ThinkingText,
    ToolHeader,
    ToolInput,
    ToolOutput,
    Divider,
}

/// One exact substring occurrence inside a [`TranscriptDoc`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub entry_index: usize,
    pub entry_id: usize,
    pub field: MatchField,
    /// Byte span in the rendered plain string for `field` (with inline code
    /// backticks already stripped for prose & thinking).
    pub start: usize,
    pub end: usize,
    pub in_collapsed_body: bool,
}

/// Semantic category for color-coding tool badges consistently across Claude,
/// Codex, and Antigravity harnesses using the active terminal's ANSI palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    Shell,
    Read,
    Search,
    Edit,
    Agent,
    Web,
    Other,
}

/// Classify a tool name from Claude Code, Codex, or Antigravity CLI into a
/// semantic display category.
pub fn classify_tool(name: &str) -> ToolCategory {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "bash" | "shell" | "exec" | "exec_command" | "run_command" | "terminal" => ToolCategory::Shell,
        "read" | "read_file" | "view_file" | "list_dir" | "ls" | "notebookread" => ToolCategory::Read,
        "grep" | "glob" | "find" | "find_by_name" | "grep_search" | "search" | "ripgrep" => ToolCategory::Search,
        "edit" | "write" | "edit_file" | "write_file" | "apply_patch" | "patch" | "multiedit" | "notebookedit" | "replace_file_content" => ToolCategory::Edit,
        "agent" | "task" | "invoke_subagent" | "spawn_agent" | "send_message" | "manage_subagents" | "swarm" | "wait" | "close_agent" => ToolCategory::Agent,
        "webfetch" | "websearch" | "search_web" | "read_url_content" | "fetch" => ToolCategory::Web,
        _ => {
            if lower.contains("bash") || lower.contains("shell") || lower.contains("command") || lower.contains("exec") {
                ToolCategory::Shell
            } else if lower.contains("edit") || lower.contains("write") || lower.contains("patch") {
                ToolCategory::Edit
            } else if lower.contains("search") || lower.contains("grep") || lower.contains("glob") || lower.contains("find") {
                ToolCategory::Search
            } else if lower.contains("read") || lower.contains("file") || lower.contains("view") {
                ToolCategory::Read
            } else if lower.contains("agent") || lower.contains("subagent") {
                ToolCategory::Agent
            } else {
                ToolCategory::Other
            }
        }
    }
}

/// Friendly compact display name for tool calls across harnesses.
pub fn friendly_tool_name(name: &str) -> String {
    match name {
        "run_command" | "exec_command" => "Shell".into(),
        "read_file" => "Read".into(),
        "edit_file" | "replace_file_content" => "Edit".into(),
        "write_file" => "Write".into(),
        "apply_patch" => "Patch".into(),
        "search_web" => "Web Search".into(),
        "invoke_subagent" => "Subagent".into(),
        "manage_subagents" => "Subagents".into(),
        "send_message" => "Message".into(),
        "manage_task" => "Task".into(),
        "ask_question" | "AskUserQuestion" => "Ask".into(),
        "update_steps" => "Steps".into(),
        other => other.to_owned(),
    }
}

/// One line of a structured edit/patch diff rendered with ANSI green/red styling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Hunk(String),
    Remove(String),
    Add(String),
    Context(String),
}

/// Extract structured diff lines from Edit/Patch/Write tool inputs across
/// Claude (`old_string`/`new_string`), Antigravity (`Target`/`Replacement`),
/// and Codex (`apply_patch`).
pub fn extract_tool_diff(name: &str, raw_input: &str) -> Option<Vec<DiffLine>> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    // 1. Codex `apply_patch` unified patch text.
    if lower.contains("patch") || trimmed.contains("*** Begin Patch") || trimmed.contains("*** Update File:") {
        let patch_text = if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            v.get("patch").or_else(|| v.get("input")).and_then(unquote_arg).unwrap_or_else(|| trimmed.to_owned())
        } else {
            trimmed.to_owned()
        };
        let mut lines = Vec::new();
        for l in patch_text.lines() {
            if l.starts_with("*** Begin Patch") || l.starts_with("*** End Patch") {
                continue;
            }
            if l.starts_with("*** ") || l.starts_with("@@") {
                lines.push(DiffLine::Hunk(l.to_owned()));
            } else if let Some(rest) = l.strip_prefix('+') {
                lines.push(DiffLine::Add(rest.to_owned()));
            } else if let Some(rest) = l.strip_prefix('-') {
                lines.push(DiffLine::Remove(rest.to_owned()));
            } else if let Some(rest) = l.strip_prefix(' ') {
                lines.push(DiffLine::Context(rest.to_owned()));
            } else {
                lines.push(DiffLine::Context(l.to_owned()));
            }
        }
        if !lines.is_empty() {
            return Some(lines);
        }
    }
    // 2. Claude `Edit` (`old_string` / `new_string`) or AGY `edit_file` (`Target` / `Replacement`).
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let old = map.get("old_string").or_else(|| map.get("Target")).or_else(|| map.get("target")).and_then(unquote_arg);
        let new = map.get("new_string").or_else(|| map.get("Replacement")).or_else(|| map.get("replacement")).and_then(unquote_arg);
        if old.is_some() || new.is_some() {
            let mut lines = Vec::new();
            if let Some(p) = map.get("file_path").or_else(|| map.get("Path")).or_else(|| map.get("path")).and_then(unquote_arg)
                && !p.is_empty()
            {
                lines.push(DiffLine::Hunk(p));
            }
            if let Some(o) = old {
                for l in o.lines().take(60) {
                    lines.push(DiffLine::Remove(l.to_owned()));
                }
            }
            if let Some(n) = new {
                for l in n.lines().take(60) {
                    lines.push(DiffLine::Add(l.to_owned()));
                }
            }
            if !lines.is_empty() {
                return Some(lines);
            }
        }
    }
    None
}

/// Strip ANSI escape sequences (CSI/OSC/2-byte ESC) so terminal output in tool
/// results renders cleanly without raw `\x1b[32m` codes.
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_owned();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    // CSI: skip until final byte 0x40..=0x7e
                    i += 1;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                b']' => {
                    // OSC: skip until BEL or ST (ESC \)
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Inline span kind extracted by [`parse_inline_markdown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineKind {
    Code,
    Bold,
    Heading,
    Quote,
    Bullet,
}

/// One inline formatting span inside plain rendered text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InlineSpan {
    pub start: usize,
    pub end: usize,
    pub kind: InlineKind,
}

/// Strip markdown markers (headings `#`/`##`/`###`, leading `- ` bullets -> `• `,
/// `` `code` ``, and `**bold**`) into clean plain text and exact byte ranges so
/// rendered prose and Find highlights share 1:1 identical byte offsets.
pub fn parse_inline_markdown(raw: &str) -> (String, Vec<InlineSpan>) {
    let mut out = String::with_capacity(raw.len());
    let mut spans = Vec::new();
    for (line_idx, line) in raw.split('\n').enumerate() {
        if line_idx > 0 {
            out.push('\n');
        }
        let line_out_start = out.len();
        let trimmed = line.trim_start();
        let lead_ws = &line[..line.len() - trimmed.len()];

        let mut is_heading = false;
        let mut is_quote = false;
        let body: &str = if let Some(rest) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
            is_heading = true;
            rest.trim_start()
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            is_quote = true;
            out.push_str(lead_ws);
            rest
        } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            out.push_str(lead_ws);
            let b_start = out.len();
            out.push_str("• ");
            spans.push(InlineSpan { start: b_start, end: b_start + "•".len(), kind: InlineKind::Bullet });
            rest
        } else {
            line
        };

        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                if let Some(rel_end) = body[i + 1..].find('`')
                    && rel_end > 0
                    && !body[i + 1..i + 1 + rel_end].contains('\n')
                {
                    let start = out.len();
                    out.push_str(&body[i + 1..i + 1 + rel_end]);
                    spans.push(InlineSpan { start, end: out.len(), kind: InlineKind::Code });
                    i += rel_end + 2;
                    continue;
                }
            } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'*') {
                if let Some(rel_end) = body[i + 2..].find("**")
                    && rel_end > 0
                    && !body[i + 2..i + 2 + rel_end].contains('\n')
                {
                    let inner = &body[i + 2..i + 2 + rel_end];
                    if !inner.starts_with(' ') && !inner.ends_with(' ') {
                        let start = out.len();
                        out.push_str(inner);
                        spans.push(InlineSpan { start, end: out.len(), kind: InlineKind::Bold });
                        i += rel_end + 4;
                        continue;
                    }
                }
            }
            let ch = body[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        let line_out_end = out.len();
        if line_out_end > line_out_start {
            if is_heading {
                spans.push(InlineSpan { start: line_out_start, end: line_out_end, kind: InlineKind::Heading });
            } else if is_quote {
                spans.push(InlineSpan { start: line_out_start, end: line_out_end, kind: InlineKind::Quote });
            }
        }
    }
    (out, spans)
}

/// Strip inline markdown backticks (`` `code` ``) from prose/thinking text and
/// return the rendered plain text along with `(byte_start, byte_end)` code spans.
pub fn parse_inline_code(raw: &str) -> (String, Vec<(usize, usize)>) {
    let mut out = String::with_capacity(raw.len());
    let mut spans = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(rel_end) = raw[i + 1..].find('`')
                && rel_end > 0
                && !raw[i + 1..i + 1 + rel_end].contains('\n')
            {
                let start = out.len();
                out.push_str(&raw[i + 1..i + 1 + rel_end]);
                spans.push((start, out.len()));
                i += rel_end + 2;
                continue;
            }
        }
        let ch = raw[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, spans)
}

/// Format a tool call's input into clean key/value rows (never raw JSON braces
/// and never AGY double-encoded string quotes). Returns empty when the input
/// merely repeats the command already shown on the tool header row.
pub fn format_tool_input(name: &str, summary: &str, raw_input: &str) -> Vec<(String, String)> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let is_shell = classify_tool(name) == ToolCategory::Shell;
        if is_shell
            && map.len() == 1
            && let Some((_, v)) = map.iter().next()
            && unquote_arg(v).is_some_and(|s| s.trim() == summary.trim() || summary.trim_end_matches('…').len() >= 12 && s.trim().starts_with(summary.trim_end_matches('…')))
        {
            return Vec::new();
        }
        let mut rows = Vec::new();
        for (k, v) in map {
            // Skip noisy internal AGY metadata parameters when formatting display args.
            if matches!(k.as_str(), "WaitMsBeforeAsync" | "NotificationTimeoutSeconds" | "IsDaemon") {
                continue;
            }
            let val = match &v {
                serde_json::Value::String(_) => unquote_arg(&v).unwrap_or_default().trim_end_matches('\n').to_owned(),
                serde_json::Value::Null => continue,
                other => other.to_string(),
            };
            if !val.is_empty() {
                rows.push((k, val));
            }
        }
        return rows;
    }
    if trimmed == summary.trim() {
        return Vec::new();
    }
    vec![(String::new(), trimmed.to_owned())]
}

/// Flatten formatted tool input rows into one searchable/displayable string.
pub fn tool_input_display_text(name: &str, summary: &str, raw_input: &str) -> String {
    let rows = format_tool_input(name, summary, raw_input);
    rows.into_iter().map(|(k, v)| if k.is_empty() { v } else { format!("{k}: {v}") }).collect::<Vec<_>>().join("\n")
}

/// Capitalise common fenced code block language tags (`"rust"` -> `"Rust"`).
pub fn format_code_lang(lang: &str) -> String {
    let t = lang.trim();
    match t.to_ascii_lowercase().as_str() {
        "" => String::new(),
        "rust" | "rs" => "Rust".into(),
        "json" => "JSON".into(),
        "toml" => "TOML".into(),
        "yaml" | "yml" => "YAML".into(),
        "html" => "HTML".into(),
        "css" => "CSS".into(),
        "sql" => "SQL".into(),
        "sh" | "bash" | "zsh" | "shell" => "Shell".into(),
        "js" | "javascript" => "JavaScript".into(),
        "ts" | "typescript" => "TypeScript".into(),
        "py" | "python" => "Python".into(),
        "c" => "C".into(),
        "cpp" | "c++" => "C++".into(),
        "go" => "Go".into(),
        "md" | "markdown" => "Markdown".into(),
        _ => {
            let mut chars = t.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

fn find_occurrences_ci(text: &str, needle_lower: &str) -> Vec<(usize, usize)> {
    if needle_lower.is_empty() || text.is_empty() {
        return Vec::new();
    }
    // Map lowercase byte offsets back safely for ASCII/UTF-8 by walking char indices.
    let mut lower = String::with_capacity(text.len());
    let mut map: Vec<usize> = Vec::with_capacity(text.len() + 1);
    for (orig_idx, ch) in text.char_indices() {
        for lc in ch.to_lowercase() {
            for _ in 0..lc.len_utf8() {
                map.push(orig_idx);
            }
            lower.push(lc);
        }
    }
    map.push(text.len());

    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(needle_lower) {
        let l_start = from + pos;
        let l_end = l_start + needle_lower.len();
        let orig_start = map[l_start];
        let orig_end = map[l_end];
        if orig_end > orig_start {
            out.push((orig_start, orig_end));
        }
        from = l_end.max(from + 1);
    }
    out
}

/// Incrementally populated conversation transcript.
#[derive(Clone, Debug)]
pub struct TranscriptDoc {
    pub format: TranscriptFormat,
    pub agent_label: String,
    pub model: String,
    pub truncated: bool,
    pub entries: Vec<TranscriptEntry>,
    next_id: usize,
    /// Map from tool_use/call id -> index in `entries` so arriving results pair cleanly.
    call_index: HashMap<String, usize>,
    /// Index of the most recent unpaired AGY tool call(s) waiting for `GENERIC` output.
    pending_agy_calls: Vec<usize>,
}

impl Default for TranscriptDoc {
    fn default() -> Self {
        Self::new("Agent")
    }
}

impl TranscriptDoc {
    pub fn new(agent_label: impl Into<String>) -> Self {
        Self {
            format: TranscriptFormat::Unknown,
            agent_label: agent_label.into(),
            model: String::new(),
            truncated: false,
            entries: Vec::new(),
            next_id: 1,
            call_index: HashMap::new(),
            pending_agy_calls: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.call_index.clear();
        self.pending_agy_calls.clear();
        self.next_id = 1;
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Append one raw line from the transcript file (JSONL or plain text).
    pub fn push_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
            && v.is_object()
        {
            if self.parse_json_line(&v) {
                return;
            }
            // Never dump unknown internal JSONL telemetry records as raw JSON walls
            // once the stream has already been identified as a structured harness.
            if matches!(self.format, TranscriptFormat::Claude | TranscriptFormat::Codex | TranscriptFormat::Agy) {
                return;
            }
        }
        // Plain text fallback (strip raw ANSI so plain logs still read cleanly).
        if self.format == TranscriptFormat::Unknown {
            self.format = TranscriptFormat::Plain;
        }
        let clean_line = strip_ansi(trimmed);
        // Coalesce consecutive plain log lines into one block (up to 40 lines per entry)
        // so streaming raw logs stay compact and scannable.
        if let Some(TranscriptEntry::Message { speaker: Speaker::Assistant, segments, .. }) = self.entries.last_mut()
            && self.format == TranscriptFormat::Plain
            && let Some(BodySegment::Code { code, .. }) = segments.last_mut()
            && code.lines().count() < 40
        {
            code.push('\n');
            code.push_str(&clean_line);
            return;
        }
        let id = self.alloc_id();
        self.entries.push(TranscriptEntry::Message {
            id,
            speaker: Speaker::Assistant,
            label: self.agent_label.clone(),
            timestamp: String::new(),
            segments: vec![BodySegment::Code { lang: String::new(), code: clean_line }],
        });
    }

    fn parse_json_line(&mut self, v: &serde_json::Value) -> bool {
        let ty = v["type"].as_str().unwrap_or("");
        // 1. Antigravity CLI format (`step_index` + AGY step types).
        if v.get("step_index").is_some() || matches!(ty, "USER_INPUT" | "PLANNER_RESPONSE" | "GENERIC" | "TOOL_RESPONSE" | "CHECKPOINT" | "SYSTEM_MESSAGE") {
            self.format = TranscriptFormat::Agy;
            if self.agent_label == "Agent" {
                self.agent_label = "Antigravity".into();
            }
            self.parse_agy_step(v, ty);
            return true;
        }
        // 2. Codex rollout format (including `token_usage_record`, `world_state`, `compacted`).
        if matches!(ty, "session_meta" | "response_item" | "event_msg" | "turn_context" | "token_usage_record" | "world_state" | "compacted") || v.get("ordinal").is_some() {
            self.format = TranscriptFormat::Codex;
            if self.agent_label == "Agent" {
                self.agent_label = "Codex".into();
            }
            self.parse_codex_item(v, ty);
            return true;
        }
        // 3. Claude Code format — match ALL internal & conversation record types
        //    BEFORE checking `v.get("message")` so `api-request-blob` and other
        //    telemetry records are cleanly swallowed instead of dumped as raw JSON!
        if is_claude_record_type(ty) || v.get("sessionId").is_some() || v.get("agentId").is_some() || v.get("parentUuid").is_some() || v.get("message").is_some() {
            self.format = TranscriptFormat::Claude;
            if self.agent_label == "Agent" {
                self.agent_label = "Claude".into();
            }
            self.parse_claude_line(v, ty);
            return true;
        }
        false
    }

    fn parse_claude_line(&mut self, v: &serde_json::Value, ty: &str) {
        let ts = short_time(v["timestamp"].as_str().unwrap_or(""));
        // Drop internal Claude Code telemetry, API request blobs, snapshots, and plumbing.
        if matches!(
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
        ) || v.get("attachment").is_some()
        {
            return;
        }
        if ty == "summary"
            && let Some(s) = v["summary"].as_str()
            && !s.trim().is_empty()
        {
            let id = self.alloc_id();
            self.entries.push(TranscriptEntry::Divider { id, label: s.trim().to_owned(), timestamp: ts });
            return;
        }
        if ty == "system" {
            let sub = v["subtype"].as_str().unwrap_or("");
            if sub == "compact_boundary" {
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Divider { id, label: "Context compacted".into(), timestamp: ts });
            } else if sub == "api_error"
                && let Some(err) = v["error"].as_str().or_else(|| v.pointer("/error/message").and_then(|s| s.as_str()))
            {
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::ToolCall {
                    id,
                    call_id: None,
                    name: "API Error".into(),
                    summary: one_line(err, 80),
                    input: String::new(),
                    output: Some(truncate_output(err)),
                    status: ToolStatus::Error,
                    timestamp: ts,
                });
            }
            return;
        }
        if ty != "user" && ty != "assistant" {
            return;
        }
        let Some(msg) = v.get("message") else { return };
        if self.model.is_empty()
            && let Some(m) = msg["model"].as_str()
            && !m.is_empty()
        {
            self.model = short_model_name(m);
        }
        let role = msg["role"].as_str().unwrap_or(ty);
        if role == "system" {
            return;
        }
        let is_api_err = v["isApiErrorMessage"].as_bool().unwrap_or(false);
        match &msg["content"] {
            serde_json::Value::String(text) => {
                if let Some(note) = summarize_task_notification(text) {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::Divider { id, label: note, timestamp: ts });
                    return;
                }
                let cleaned = clean_user_text(text);
                if cleaned.is_empty() {
                    return;
                }
                if is_api_err {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::ToolCall {
                        id,
                        call_id: None,
                        name: "API Error".into(),
                        summary: one_line(&cleaned, 80),
                        input: String::new(),
                        output: Some(truncate_output(&cleaned)),
                        status: ToolStatus::Error,
                        timestamp: ts,
                    });
                    return;
                }
                let speaker = if role == "user" { Speaker::User } else { Speaker::Assistant };
                let label = if speaker == Speaker::User { "You".into() } else { self.agent_label.clone() };
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Message { id, speaker, label, timestamp: ts, segments: split_code_segments(&cleaned) });
            }
            serde_json::Value::Array(blocks) => {
                let mut text_buf = String::new();
                for block in blocks {
                    let btype = block["type"].as_str().unwrap_or("");
                    match btype {
                        "text" => {
                            if let Some(t) = block["text"].as_str() {
                                if let Some(note) = summarize_task_notification(t) {
                                    self.flush_text(role, &ts, &mut text_buf);
                                    let id = self.alloc_id();
                                    self.entries.push(TranscriptEntry::Divider { id, label: note, timestamp: ts.clone() });
                                    continue;
                                }
                                if !text_buf.is_empty() {
                                    text_buf.push('\n');
                                }
                                text_buf.push_str(t);
                            }
                        }
                        "thinking" => {
                            if let Some(t) = block["thinking"].as_str()
                                && !t.trim().is_empty()
                            {
                                self.flush_text(role, &ts, &mut text_buf);
                                let id = self.alloc_id();
                                self.entries.push(TranscriptEntry::Thinking { id, summary: one_line(t, 88), text: t.trim().to_owned(), timestamp: ts.clone() });
                            }
                        }
                        "tool_use" => {
                            self.flush_text(role, &ts, &mut text_buf);
                            let name = block["name"].as_str().unwrap_or("Tool").to_owned();
                            let call_id = block["id"].as_str().map(str::to_owned);
                            let input_val = &block["input"];
                            let summary = summarize_tool_args(&name, input_val);
                            let input = pretty_json_or_str(input_val);
                            let id = self.alloc_id();
                            let entry_idx = self.entries.len();
                            if let Some(cid) = &call_id {
                                self.call_index.insert(cid.clone(), entry_idx);
                            }
                            self.entries.push(TranscriptEntry::ToolCall { id, call_id, name, summary, input, output: None, status: ToolStatus::Running, timestamp: ts.clone() });
                        }
                        "tool_result" => {
                            let call_id = block["tool_use_id"].as_str().unwrap_or("");
                            let is_err = block["is_error"].as_bool().unwrap_or(false);
                            let raw_out = clean_tool_output(&extract_content_text(&block["content"]));
                            let output = if raw_out.is_empty() {
                                v.pointer("/toolUseResult/stdout")
                                    .and_then(|s| s.as_str())
                                    .or_else(|| v.pointer("/toolUseResult/stderr").and_then(|s| s.as_str()))
                                    .map(clean_tool_output)
                                    .unwrap_or_default()
                            } else {
                                raw_out
                            };
                            if let Some(&idx) = self.call_index.get(call_id)
                                && let Some(TranscriptEntry::ToolCall { output: slot, status, .. }) = self.entries.get_mut(idx)
                            {
                                *slot = (!output.is_empty()).then(|| truncate_output(&output));
                                *status = if is_err { ToolStatus::Error } else { ToolStatus::Ok };
                            } else if !output.trim().is_empty() {
                                let id = self.alloc_id();
                                self.entries.push(TranscriptEntry::ToolCall {
                                    id,
                                    call_id: (!call_id.is_empty()).then(|| call_id.to_owned()),
                                    name: "Tool Result".into(),
                                    summary: one_line(&output, 80),
                                    input: String::new(),
                                    output: Some(truncate_output(&output)),
                                    status: if is_err { ToolStatus::Error } else { ToolStatus::Ok },
                                    timestamp: ts.clone(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if is_api_err && !text_buf.trim().is_empty() {
                    let err = std::mem::take(&mut text_buf);
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::ToolCall {
                        id,
                        call_id: None,
                        name: "API Error".into(),
                        summary: one_line(&err, 80),
                        input: String::new(),
                        output: Some(truncate_output(&err)),
                        status: ToolStatus::Error,
                        timestamp: ts,
                    });
                } else {
                    self.flush_text(role, &ts, &mut text_buf);
                }
            }
            _ => {}
        }
    }

    fn parse_codex_item(&mut self, v: &serde_json::Value, ty: &str) {
        let ts = short_time(v["timestamp"].as_str().unwrap_or(""));
        match ty {
            "session_meta" => {
                if let Some(nick) = v.pointer("/payload/agent_nickname").or_else(|| v.pointer("/payload/source/subagent/thread_spawn/agent_nickname")).and_then(|s| s.as_str())
                    && !nick.is_empty()
                {
                    self.agent_label = nick.to_owned();
                }
                return;
            }
            "turn_context" => {
                if self.model.is_empty()
                    && let Some(m) = v.pointer("/payload/model").and_then(|s| s.as_str())
                    && !m.is_empty()
                {
                    self.model = short_model_name(m);
                }
                return;
            }
            "token_usage_record" | "world_state" => return,
            "compacted" => {
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Divider { id, label: "Context compacted".into(), timestamp: ts });
                return;
            }
            "event_msg" => {
                let et = v.pointer("/payload/type").and_then(|s| s.as_str()).unwrap_or("");
                match et {
                    "task_complete" => {
                        let already_div = matches!(self.entries.last(), Some(TranscriptEntry::Divider { label, .. }) if label == "Turn complete");
                        if !already_div {
                            let id = self.alloc_id();
                            self.entries.push(TranscriptEntry::Divider { id, label: "Turn complete".into(), timestamp: ts });
                        }
                    }
                    "turn_aborted" => {
                        let id = self.alloc_id();
                        self.entries.push(TranscriptEntry::Divider { id, label: "Turn interrupted".into(), timestamp: ts });
                    }
                    "task_failed" | "error" => {
                        let msg = v.pointer("/payload/message").or_else(|| v.pointer("/payload/error")).and_then(|s| s.as_str()).unwrap_or("Turn failed");
                        let id = self.alloc_id();
                        self.entries.push(TranscriptEntry::ToolCall {
                            id,
                            call_id: None,
                            name: "Error".into(),
                            summary: one_line(msg, 80),
                            input: String::new(),
                            output: Some(truncate_output(msg)),
                            status: ToolStatus::Error,
                            timestamp: ts,
                        });
                    }
                    "exec_approval_request" | "apply_patch_approval_request" => {
                        let id = self.alloc_id();
                        self.entries.push(TranscriptEntry::Divider { id, label: "Waiting for approval".into(), timestamp: ts });
                    }
                    _ => {}
                }
                return;
            }
            "response_item" => {}
            _ => return,
        }

        let payload = &v["payload"];
        let ptype = payload["type"].as_str().unwrap_or("");
        match ptype {
            "additional_tools" => {}
            "compaction" => {
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Divider { id, label: "Context compacted".into(), timestamp: ts });
            }
            "message" => {
                let role = payload["role"].as_str().unwrap_or("assistant");
                // Skip huge boilerplate developer/system prompt blocks.
                if role == "developer" || role == "system" {
                    return;
                }
                let text = extract_content_text(&payload["content"]);
                let cleaned = clean_user_text(&text);
                if cleaned.is_empty() {
                    return;
                }
                let speaker = if role == "user" { Speaker::User } else { Speaker::Assistant };
                let label = if speaker == Speaker::User { "You".into() } else { self.agent_label.clone() };
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Message { id, speaker, label, timestamp: ts, segments: split_code_segments(&cleaned) });
            }
            "reasoning" => {
                let text = payload["summary"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"))
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| payload["content"].as_str().map(str::to_owned));
                if let Some(t) = text
                    && !t.trim().is_empty()
                {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::Thinking { id, summary: one_line(&t, 88), text: t.trim().to_owned(), timestamp: ts });
                }
            }
            "function_call" | "custom_tool_call" => {
                let raw_name = payload["name"].as_str().unwrap_or("Tool");
                let call_id = payload["call_id"].as_str().map(str::to_owned);
                let raw_args = payload["arguments"].as_str().or_else(|| payload["input"].as_str()).unwrap_or("");
                let (name, summary, input) = parse_codex_tool_call(raw_name, raw_args);
                let id = self.alloc_id();
                let idx = self.entries.len();
                if let Some(cid) = &call_id {
                    self.call_index.insert(cid.clone(), idx);
                }
                self.entries.push(TranscriptEntry::ToolCall { id, call_id, name, summary, input, output: None, status: ToolStatus::Running, timestamp: ts });
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = payload["call_id"].as_str().unwrap_or("");
                let raw_out = payload["output"].as_str().map(str::to_owned).unwrap_or_else(|| extract_content_text(&payload["output"]));
                let (cleaned_out, failed) = parse_codex_tool_output(&raw_out);
                if let Some(&idx) = self.call_index.get(call_id)
                    && let Some(TranscriptEntry::ToolCall { output: slot, status, .. }) = self.entries.get_mut(idx)
                {
                    *slot = (!cleaned_out.is_empty()).then(|| truncate_output(&cleaned_out));
                    *status = if failed { ToolStatus::Error } else { ToolStatus::Ok };
                }
            }
            _ => {}
        }
    }

    fn parse_agy_step(&mut self, v: &serde_json::Value, ty: &str) {
        let ts = short_time(v["created_at"].as_str().unwrap_or(""));
        if self.model.is_empty()
            && let Some(m) = v["model"].as_str()
            && !m.is_empty()
        {
            self.model = short_model_name(m);
        }
        match ty {
            "SYSTEM_MESSAGE" => {
                // Skip internal harness reminders (`work efficiently`, etc.), keep real notices.
                let content = v["content"].as_str().unwrap_or("");
                if content.contains("Reminder: work efficiently") || content.contains("not actually sent by the user") {
                    return;
                }
                let cleaned = clean_user_text(content);
                if !cleaned.is_empty() {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::Divider { id, label: one_line(&cleaned, 96), timestamp: ts });
                }
            }
            "USER_INPUT" => {
                let raw = v["content"].as_str().unwrap_or("");
                let cleaned = clean_user_text(raw);
                if !cleaned.is_empty() {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::Message { id, speaker: Speaker::User, label: "You".into(), timestamp: ts, segments: split_code_segments(&cleaned) });
                }
            }
            "PLANNER_RESPONSE" => {
                self.pending_agy_calls.clear();
                if let Some(thinking) = v["thinking"].as_str().or_else(|| v["thoughts"].as_str())
                    && !thinking.trim().is_empty()
                {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::Thinking { id, summary: one_line(thinking, 88), text: thinking.trim().to_owned(), timestamp: ts.clone() });
                }
                if let Some(content) = v["content"].as_str()
                    && !content.trim().is_empty()
                {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::Message {
                        id,
                        speaker: Speaker::Assistant,
                        label: self.agent_label.clone(),
                        timestamp: ts.clone(),
                        segments: split_code_segments(content.trim()),
                    });
                }
                if let Some(calls) = v["tool_calls"].as_array() {
                    for call in calls {
                        let name = call["name"].as_str().unwrap_or("tool").to_owned();
                        let args = &call["args"];
                        let summary = summarize_tool_args(&name, args);
                        let input = pretty_json_or_str(args);
                        let id = self.alloc_id();
                        let idx = self.entries.len();
                        self.pending_agy_calls.push(idx);
                        self.entries.push(TranscriptEntry::ToolCall { id, call_id: None, name, summary, input, output: None, status: ToolStatus::Running, timestamp: ts.clone() });
                    }
                }
            }
            "GENERIC" | "TOOL_RESPONSE" => {
                let raw_content = v["content"].as_str().unwrap_or("");
                let err_field = v["error"].as_str().unwrap_or("").trim();
                let mut content = strip_agy_tool_headers(&clean_tool_output(raw_content));
                if !err_field.is_empty() {
                    if content.is_empty() {
                        content = err_field.to_owned();
                    } else if !content.contains(err_field) {
                        content = format!("{err_field}\n{content}");
                    }
                }
                let st_str = v["status"].as_str().unwrap_or("");
                let is_running = st_str.eq_ignore_ascii_case("RUNNING");
                let is_err = st_str.eq_ignore_ascii_case("ERROR") || st_str.eq_ignore_ascii_case("FAILED") || !err_field.is_empty() || has_nonzero_exit_code(&content) || content.starts_with("Error:");
                let status = if is_err {
                    ToolStatus::Error
                } else if is_running {
                    ToolStatus::Running
                } else {
                    ToolStatus::Ok
                };
                if !self.pending_agy_calls.is_empty() {
                    let idx = self.pending_agy_calls.remove(0);
                    if let Some(TranscriptEntry::ToolCall { output, status: slot_st, .. }) = self.entries.get_mut(idx) {
                        *output = (!content.is_empty()).then(|| truncate_output(&content));
                        *slot_st = status;
                        return;
                    }
                }
                if !content.is_empty() {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::ToolCall {
                        id,
                        call_id: None,
                        name: if is_err { "Error".into() } else { "Output".into() },
                        summary: one_line(&content, 80),
                        input: String::new(),
                        output: Some(truncate_output(&content)),
                        status,
                        timestamp: ts,
                    });
                }
            }
            "CHECKPOINT" => {
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Divider { id, label: "Context compacted".into(), timestamp: ts });
            }
            _ => {}
        }
    }

    fn flush_text(&mut self, role: &str, ts: &str, buf: &mut String) {
        let cleaned = clean_user_text(buf);
        buf.clear();
        if cleaned.is_empty() {
            return;
        }
        let speaker = if role == "user" { Speaker::User } else { Speaker::Assistant };
        let label = if speaker == Speaker::User { "You".into() } else { self.agent_label.clone() };
        let id = self.alloc_id();
        self.entries.push(TranscriptEntry::Message { id, speaker, label, timestamp: ts.to_owned(), segments: split_code_segments(&cleaned) });
    }

    /// Case-insensitive per-occurrence search across every rendered text field
    /// (prose, inline code, fenced code blocks, thinking body, tool headers,
    /// inputs, and outputs). Records whether each match sits inside a
    /// collapsible body so Find can auto-expand only the active match's item.
    pub fn find(&self, query: &str) -> Vec<SearchMatch> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            let eid = entry.id();
            match entry {
                TranscriptEntry::Message { segments, .. } => {
                    for (seg_idx, seg) in segments.iter().enumerate() {
                        match seg {
                            BodySegment::Text(t) => {
                                let (plain, _) = parse_inline_markdown(t.trim());
                                for (start, end) in find_occurrences_ci(&plain, &q) {
                                    out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::MessageSegment(seg_idx), start, end, in_collapsed_body: false });
                                }
                            }
                            BodySegment::Code { code, .. } => {
                                let cleaned = code.trim_end_matches('\n');
                                for (start, end) in find_occurrences_ci(cleaned, &q) {
                                    out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::MessageSegment(seg_idx), start, end, in_collapsed_body: false });
                                }
                            }
                        }
                    }
                }
                TranscriptEntry::Thinking { text, .. } => {
                    let (plain, _) = parse_inline_markdown(text.trim());
                    for (start, end) in find_occurrences_ci(&plain, &q) {
                        out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::ThinkingText, start, end, in_collapsed_body: true });
                    }
                }
                TranscriptEntry::ToolCall { name, summary, input, output, .. } => {
                    let head = format!("{name} {summary}");
                    for (start, end) in find_occurrences_ci(&head, &q) {
                        out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::ToolHeader, start, end, in_collapsed_body: false });
                    }
                    let formatted_in = tool_input_display_text(name, summary, input);
                    for (start, end) in find_occurrences_ci(&formatted_in, &q) {
                        out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::ToolInput, start, end, in_collapsed_body: true });
                    }
                    if let Some(o) = output {
                        let cleaned = o.trim_end_matches('\n');
                        for (start, end) in find_occurrences_ci(cleaned, &q) {
                            out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::ToolOutput, start, end, in_collapsed_body: true });
                        }
                    }
                }
                TranscriptEntry::Divider { label, .. } => {
                    for (start, end) in find_occurrences_ci(label, &q) {
                        out.push(SearchMatch { entry_index: i, entry_id: eid, field: MatchField::Divider, start, end, in_collapsed_body: false });
                    }
                }
            }
        }
        out
    }
}

/// Split markdown text into prose and fenced code blocks (` ```lang ... ``` `).
pub fn split_code_segments(text: &str) -> Vec<BodySegment> {
    let mut out = Vec::new();
    let mut prose = String::new();
    let mut code = String::new();
    let mut code_lang = String::new();
    let mut in_code = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_code {
                out.push(BodySegment::Code { lang: std::mem::take(&mut code_lang), code: std::mem::take(&mut code).trim_end_matches('\n').to_owned() });
                in_code = false;
            } else {
                if !prose.trim().is_empty() {
                    out.push(BodySegment::Text(prose.trim_end().to_owned()));
                }
                prose.clear();
                code_lang = rest.trim().to_owned();
                in_code = true;
            }
            continue;
        }
        if in_code {
            if !code.is_empty() {
                code.push('\n');
            }
            code.push_str(line);
        } else {
            if !prose.is_empty() {
                prose.push('\n');
            }
            prose.push_str(line);
        }
    }
    if in_code && !code.is_empty() {
        out.push(BodySegment::Code { lang: code_lang, code });
    } else if !prose.trim().is_empty() {
        out.push(BodySegment::Text(prose.trim_end().to_owned()));
    }
    if out.is_empty() && !text.trim().is_empty() {
        out.push(BodySegment::Text(text.trim().to_owned()));
    }
    out
}

fn is_claude_record_type(ty: &str) -> bool {
    matches!(
        ty,
        "user"
            | "assistant"
            | "system"
            | "summary"
            | "progress"
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
    )
}

fn short_model_name(m: &str) -> String {
    m.trim().trim_start_matches("claude-").to_owned()
}

fn strip_xml_block(mut text: String, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    while let Some(start) = text.find(&open) {
        if let Some(rel_end) = text[start + open.len()..].find(&close) {
            let end = start + open.len() + rel_end + close.len();
            text.replace_range(start..end, "");
        } else {
            break;
        }
    }
    text
}

fn summarize_task_notification(raw: &str) -> Option<String> {
    if !raw.contains("<task-notification>") {
        return None;
    }
    let tag_val = |t: &str| -> Option<String> {
        let open = format!("<{t}>");
        let close = format!("</{t}>");
        let s = raw.find(&open)? + open.len();
        let e = raw[s..].find(&close)?;
        Some(raw[s..s + e].trim().to_owned())
    };
    if let Some(sum) = tag_val("summary")
        && !sum.is_empty()
    {
        return Some(sum);
    }
    let status = tag_val("status").unwrap_or_else(|| "done".into());
    let tid = tag_val("task-id").unwrap_or_default();
    Some(if tid.is_empty() { format!("Subagent {status}") } else { format!("Subagent {tid}: {status}") })
}

fn clean_user_text(raw: &str) -> String {
    let s = raw.trim();
    if let Some(start) = s.find("<USER_REQUEST>") {
        let rest = &s[start + "<USER_REQUEST>".len()..];
        if let Some(end) = rest.find("</USER_REQUEST>") {
            return rest[..end].trim().to_owned();
        }
        return rest.trim().to_owned();
    }
    let mut text = strip_ansi(s);
    for tag in ["system-reminder", "local-command-caveat", "ADDITIONAL_METADATA", "USER_SETTINGS_CHANGE", "environment_context", "INSTRUCTIONS"] {
        if text.contains(&format!("<{tag}>")) {
            text = strip_xml_block(text, tag);
        }
    }
    text.trim().to_owned()
}

fn clean_tool_output(raw: &str) -> String {
    let mut text = strip_ansi(raw);
    if text.contains("<tool_use_error>") {
        text = text.replace("<tool_use_error>", "").replace("</tool_use_error>", "");
    }
    if text.contains("<system-reminder>") {
        text = strip_xml_block(text, "system-reminder");
    }
    text.trim().to_owned()
}

fn strip_agy_tool_headers(text: &str) -> String {
    let mut lines = text.lines().peekable();
    while let Some(&l) = lines.peek() {
        let t = l.trim();
        if t.starts_with("Created At:") || t.starts_with("Completed At:") {
            lines.next();
        } else {
            break;
        }
    }
    lines.collect::<Vec<_>>().join("\n").trim().to_owned()
}

fn has_nonzero_exit_code(text: &str) -> bool {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        for prefix in ["command exited with code ", "exit status ", "exited with code ", "exit code "] {
            if let Some(pos) = lower.find(prefix) {
                let digits: String = lower[pos + prefix.len()..].trim().chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
                if let Ok(code) = digits.parse::<i32>()
                    && code != 0
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Unwrap Codex `custom_tool_call` / `function_call` wrappers into clean tool
/// name, summary, and input text.
fn parse_codex_tool_call(raw_name: &str, raw_args: &str) -> (String, String, String) {
    let trimmed = raw_args.trim();
    // Codex JS `exec` wrapper: `text(await tools.exec_command({cmd:"cat brief.md"}))`
    if raw_name == "exec" && trimmed.contains("exec_command") {
        if let Some(pos) = trimmed.find("cmd:") {
            let after = trimmed[pos + 4..].trim_start();
            if let Some(quote) = after.chars().next().filter(|&c| c == '"' || c == '\'') {
                let mut cmd = String::new();
                let mut esc = false;
                for ch in after[1..].chars() {
                    if esc {
                        match ch {
                            'n' => cmd.push('\n'),
                            't' => cmd.push('\t'),
                            _ => cmd.push(ch),
                        }
                        esc = false;
                    } else if ch == '\\' {
                        esc = true;
                    } else if ch == quote {
                        break;
                    } else {
                        cmd.push(ch);
                    }
                }
                if !cmd.is_empty() {
                    return ("Shell".into(), one_line(&cmd, 72), cmd);
                }
            }
        }
    }
    if raw_name == "apply_patch" || trimmed.starts_with("*** Begin Patch") {
        let target_file = trimmed
            .lines()
            .find_map(|l| l.strip_prefix("*** Update File:").or_else(|| l.strip_prefix("*** Add File:")).or_else(|| l.strip_prefix("*** Delete File:")).map(str::trim))
            .unwrap_or("patch");
        let short = target_file.rsplit('/').next().unwrap_or(target_file);
        return ("Patch".into(), one_line(short, 72), trimmed.to_owned());
    }
    let parsed_args: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::Value::String(trimmed.to_owned()));
    let summary = summarize_tool_args(raw_name, &parsed_args);
    let input = pretty_json_or_str(&parsed_args);
    (raw_name.to_owned(), summary, input)
}

/// Unwrap Codex `custom_tool_call_output` / `function_call_output` JSON wrappers
/// (`{"output":"...","metadata":{"exit_code":N}}`) and strip ANSI codes.
fn parse_codex_tool_output(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && v.is_object()
    {
        let exit_code = v.pointer("/metadata/exit_code").or_else(|| v.get("exit_code")).and_then(|x| x.as_i64());
        let failed = exit_code.is_some_and(|c| c != 0) || v["status"].as_str().is_some_and(|s| s.eq_ignore_ascii_case("error") || s.eq_ignore_ascii_case("failed"));
        let mut out = v["output"].as_str().or_else(|| v["stdout"].as_str()).map(clean_tool_output).unwrap_or_else(|| clean_tool_output(trimmed));
        if let Some(err) = v["stderr"].as_str().filter(|s| !s.trim().is_empty()) {
            let e = clean_tool_output(err);
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&e);
        }
        if let Some(code) = exit_code
            && code != 0
            && !out.to_ascii_lowercase().contains("exit")
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("exit status {code}"));
        }
        return (out, failed);
    }
    let cleaned = clean_tool_output(trimmed);
    let failed = has_nonzero_exit_code(&cleaned);
    (cleaned, failed)
}

fn short_time(ts: &str) -> String {
    // Extract HH:MM:SS from ISO-8601 `2026-11-25T14:52:34Z`
    if let Some((_, after_t)) = ts.split_once('T') {
        let core = after_t.split(['.', 'Z', '+', '-']).next().unwrap_or(after_t);
        if core.len() >= 5 {
            return core[..core.len().min(8)].to_owned();
        }
    }
    String::new()
}

fn one_line(s: &str, max_chars: usize) -> String {
    let first = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut out: String = first.chars().take(max_chars).collect();
    if first.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn truncate_output(s: &str) -> String {
    const LIMIT: usize = 16 * 1024;
    if s.len() <= LIMIT {
        s.trim_end().to_owned()
    } else {
        let cut = s.floor_char_boundary(LIMIT);
        format!("{}\n… ({} bytes truncated)", s[..cut].trim_end(), s.len() - cut)
    }
}

fn extract_content_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            arr.iter().filter_map(|x| x["text"].as_str().or_else(|| x["input_text"].as_str()).or_else(|| x["output_text"].as_str()).or_else(|| x.as_str())).collect::<Vec<_>>().join("\n")
        }
        other if other.is_null() => String::new(),
        other => other.to_string(),
    }
}

fn unquote_arg(v: &serde_json::Value) -> Option<String> {
    let s = v.as_str()?.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 { serde_json::from_str::<String>(s).ok().or_else(|| Some(s[1..s.len() - 1].to_owned())) } else { Some(s.to_owned()) }
}

fn summarize_tool_args(name: &str, args: &serde_json::Value) -> String {
    if let Some(obj) = args.as_object() {
        for key in ["command", "CommandLine", "cmd", "file_path", "Path", "path", "pattern", "query", "description", "toolSummary", "Role"] {
            if let Some(v) = obj.get(key).and_then(unquote_arg)
                && !v.is_empty()
            {
                let short = if matches!(key, "file_path" | "Path" | "path") { v.rsplit('/').next().unwrap_or(&v).to_owned() } else { v };
                return one_line(&short, 72);
            }
        }
        if let Some((k, v)) = obj.iter().next() {
            let val = unquote_arg(v).unwrap_or_else(|| v.to_string());
            return one_line(&format!("{k}: {val}"), 72);
        }
    }
    if let Some(s) = args.as_str() {
        return one_line(s, 72);
    }
    let _ = name;
    String::new()
}

fn pretty_json_or_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_turns_code_blocks_and_paired_tools() {
        let mut doc = TranscriptDoc::new("Claude");
        doc.push_line(r#"{"type":"user","timestamp":"2026-11-25T10:00:00Z","message":{"role":"user","content":"Check build status"}}"#);
        doc.push_line(r#"{"type":"assistant","timestamp":"2026-11-25T10:00:02Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Let me run jato test."},{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"jato test -p olo-rex"}}]}}"#);
        doc.push_line(
            r#"{"type":"user","timestamp":"2026-11-25T10:00:05Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"test result: ok. 149 passed"}]}}"#,
        );
        doc.push_line(r#"{"type":"assistant","timestamp":"2026-11-25T10:00:06Z","message":{"role":"assistant","content":[{"type":"text","text":"All green:\n```rust\nassert_eq!(149, 149);\n```\nReady to ship."}]}}"#);

        assert_eq!(doc.format, TranscriptFormat::Claude);
        assert_eq!(doc.entries.len(), 4);
        match &doc.entries[2] {
            TranscriptEntry::ToolCall { name, summary, output, status, .. } => {
                assert_eq!(name, "Bash");
                assert_eq!(summary, "jato test -p olo-rex");
                assert_eq!(output.as_deref(), Some("test result: ok. 149 passed"));
                assert_eq!(*status, ToolStatus::Ok);
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &doc.entries[3] {
            TranscriptEntry::Message { segments, .. } => {
                assert_eq!(segments.len(), 3);
                assert_eq!(segments[1], BodySegment::Code { lang: "rust".into(), code: "assert_eq!(149, 149);".into() });
            }
            other => panic!("expected Message, got {other:?}"),
        }
        let hits = doc.find("149");
        // Matches once in Bash tool output ("149 passed") and twice in the Rust code block ("149, 149").
        assert_eq!(hits.len(), 3);
        assert!(hits[0].in_collapsed_body);
        assert!(!hits[1].in_collapsed_body);
        assert!(!hits[2].in_collapsed_body);

        let (plain, spans) = parse_inline_code("Uses `ignore-size` with `tmux`");
        assert_eq!(plain, "Uses ignore-size with tmux");
        assert_eq!(spans, vec![(5, 16), (22, 26)]);
        assert!(format_tool_input("Bash", "jato test -p olo-rex", r#"{"command":"jato test -p olo-rex"}"#).is_empty());
        assert_eq!(format_tool_input("Read", "panes.rs", r#"{"path":"crates/rex-link/src/panes.rs"}"#), vec![("path".into(), "crates/rex-link/src/panes.rs".into())]);
        assert_eq!(format_code_lang("rust"), "Rust");
    }

    #[test]
    fn parses_codex_and_agy_transcripts_and_tmux_cmd() {
        let mut codex = TranscriptDoc::new("Codex");
        codex.push_line(r#"{"type":"session_meta","timestamp":"2026-11-25T11:00:00Z","payload":{"agent_nickname":"Newton"}}"#);
        codex.push_line(r#"{"type":"turn_context","timestamp":"2026-11-25T11:00:00Z","payload":{"model":"o9-codex"}}"#);
        // Real Codex noise that must NEVER leak as raw JSON blocks:
        codex.push_line(r#"{"type":"token_usage_record","timestamp":"2026-11-25T11:00:00Z","payload":{"total_tokens":5000}}"#);
        codex.push_line(r#"{"type":"world_state","timestamp":"2026-11-25T11:00:00Z","payload":{"full":true,"state":{"agents_md":{"text":"huge"}}}}"#);
        codex.push_line(r#"{"type":"response_item","timestamp":"2026-11-25T11:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix bug"}]}}"#);
        codex.push_line(r#"{"type":"response_item","timestamp":"2026-11-25T11:00:02Z","payload":{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"text(await tools.exec_command({cmd:\"git diff\"}))\n"}}"#);
        codex.push_line(r#"{"type":"response_item","timestamp":"2026-11-25T11:00:03Z","payload":{"type":"custom_tool_call_output","call_id":"c1","output":"{\"output\":\"\\u001b[31merror\\u001b[0m\",\"metadata\":{\"exit_code\":1}}"}}"#);
        assert_eq!(codex.agent_label, "Newton");
        assert_eq!(codex.model, "o9-codex");
        assert_eq!(codex.entries.len(), 2);
        match &codex.entries[1] {
            TranscriptEntry::ToolCall { name, summary, output, status, .. } => {
                assert_eq!(name, "Shell");
                assert_eq!(summary, "git diff");
                assert_eq!(*status, ToolStatus::Error);
                assert!(output.as_deref().unwrap().contains("error"));
                assert!(!output.as_deref().unwrap().contains("\x1b"));
            }
            other => panic!("expected Shell ToolCall, got {other:?}"),
        }

        let mut agy = TranscriptDoc::new("Antigravity");
        agy.push_line(
            r#"{"step_index":0,"type":"USER_INPUT","created_at":"2026-11-25T12:00:00Z","content":"<USER_REQUEST>\nReview shots\n</USER_REQUEST>\n<ADDITIONAL_METADATA>ignored</ADDITIONAL_METADATA>"}"#,
        );
        agy.push_line(
            r#"{"step_index":1,"type":"SYSTEM_MESSAGE","source":"SYSTEM","status":"DONE","content":"The following is a <SYSTEM_MESSAGE> not actually sent by the user. Reminder: work efficiently."}"#,
        );
        agy.push_line(r#"{"step_index":2,"type":"PLANNER_RESPONSE","model":"gemini-4-neon-xhigh","created_at":"2026-11-25T12:00:02Z","thinking":"Checking PNGs","content":"Looks sharp.","tool_calls":[{"name":"read_file","args":{"Path":"\"/a/b/shot.png\""}},{"name":"run_command","args":{"CommandLine":"\"false\""}}]}"#);
        agy.push_line(r#"{"step_index":3,"type":"GENERIC","status":"DONE","created_at":"2026-11-25T12:00:03Z","content":"Created At: 2026-11-25T12:00:03Z\nCompleted At: 2026-11-25T12:00:03Z\nOK"}"#);
        agy.push_line(r#"{"step_index":4,"type":"GENERIC","status":"ERROR","created_at":"2026-11-25T12:00:04Z","error":"permission check failed","content":""}"#);
        assert_eq!(agy.format, TranscriptFormat::Agy);
        assert_eq!(agy.model, "gemini-4-neon-xhigh");
        assert_eq!(agy.entries.len(), 5);
        match &agy.entries[3] {
            TranscriptEntry::ToolCall { output, status, .. } => {
                assert_eq!(output.as_deref(), Some("OK"));
                assert_eq!(*status, ToolStatus::Ok);
            }
            other => panic!("expected read_file ToolCall, got {other:?}"),
        }
        match &agy.entries[4] {
            TranscriptEntry::ToolCall { output, status, .. } => {
                assert_eq!(output.as_deref(), Some("permission check failed"));
                assert_eq!(*status, ToolStatus::Error);
            }
            other => panic!("expected failed run_command ToolCall, got {other:?}"),
        }
        assert_eq!(agy.find("Review shots").len(), 1);

        let cmd = tmux_attach_command(&TmuxTarget { socket: "/tmp/tmux-1000/default".into(), session: "agy-1".into(), target: "agy-1:0.0".into(), cols: 120, rows: 40, pty_cols: 120, pty_rows: 41 });
        assert!(cmd.contains("attach -r -f ignore-size -t"));
    }

    #[test]
    fn skips_claude_internal_telemetry_and_extracts_diffs() {
        let mut claude = TranscriptDoc::new("Claude");
        claude.push_line(r#"{"type":"user","timestamp":"2026-11-25T10:00:00Z","message":{"role":"user","content":"Fix parser"}}"#);
        // Real Claude Code 2.1 internal records that previously leaked raw JSON blobs!
        claude.push_line(r#"{"type":"attachment","attachment":{"type":"deferred_tools_record","entries":[]}}"#);
        claude.push_line(r#"{"type":"data-flow-records","records":[]}"#);
        claude.push_line(r#"{"type":"api-request-shape","shape":{}}"#);
        claude.push_line(r#"{"type":"api-request-blob","message":{"role":"system","content":[{"type":"text","text":"SECRET SYSTEM PROMPT"}]}}"#);
        claude.push_line(r#"{"type":"api-request","id":"req_1","timestamp":"2026-11-25T10:00:01Z"}"#);
        claude.push_line(r#"{"type":"assistant","timestamp":"2026-11-25T10:00:02Z","message":{"model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"let a = 1;","new_string":"let a = 2;"}}]}}"#);
        assert_eq!(claude.format, TranscriptFormat::Claude);
        assert_eq!(claude.model, "opus-5-5");
        assert_eq!(claude.entries.len(), 2, "must not leak any telemetry or api-request-blob lines: {:?}", claude.entries);
        if let TranscriptEntry::ToolCall { name, input, .. } = &claude.entries[1] {
            let diff = extract_tool_diff(name, input).expect("Edit should produce diff");
            assert_eq!(diff, vec![DiffLine::Hunk("src/lib.rs".into()), DiffLine::Remove("let a = 1;".into()), DiffLine::Add("let a = 2;".into())]);
        } else {
            panic!("expected Edit tool call");
        }
    }
}
