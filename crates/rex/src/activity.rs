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

/// Format a tool call's input into clean key/value rows (never raw JSON braces).
/// Returns an empty vector when the input merely repeats the command already
/// shown on the tool header row (e.g. `Bash` with a single `command` field).
pub fn format_tool_input(name: &str, summary: &str, raw_input: &str) -> Vec<(String, String)> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let is_shell = matches!(name.to_ascii_lowercase().as_str(), "bash" | "shell" | "run_command" | "exec");
        if is_shell
            && map.len() == 1
            && let Some((_, v)) = map.iter().next()
            && v.as_str().is_some_and(|s| s.trim() == summary.trim() || summary.trim_end_matches('…').len() >= 12 && s.trim().starts_with(summary.trim_end_matches('…')))
        {
            return Vec::new();
        }
        let mut rows = Vec::new();
        for (k, v) in map {
            let val = match v {
                serde_json::Value::String(s) => s.trim_end_matches('\n').to_owned(),
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
        Self { format: TranscriptFormat::Unknown, agent_label: agent_label.into(), truncated: false, entries: Vec::new(), next_id: 1, call_index: HashMap::new(), pending_agy_calls: Vec::new() }
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
        }
        // Plain text fallback.
        if self.format == TranscriptFormat::Unknown {
            self.format = TranscriptFormat::Plain;
        }
        // Coalesce consecutive plain log lines into one block (up to 40 lines per entry)
        // so streaming raw logs stay compact and scannable.
        if let Some(TranscriptEntry::Message { speaker: Speaker::Assistant, segments, .. }) = self.entries.last_mut()
            && self.format == TranscriptFormat::Plain
            && let Some(BodySegment::Code { code, .. }) = segments.last_mut()
            && code.lines().count() < 40
        {
            code.push('\n');
            code.push_str(trimmed);
            return;
        }
        let id = self.alloc_id();
        self.entries.push(TranscriptEntry::Message {
            id,
            speaker: Speaker::Assistant,
            label: self.agent_label.clone(),
            timestamp: String::new(),
            segments: vec![BodySegment::Code { lang: String::new(), code: trimmed.to_owned() }],
        });
    }

    fn parse_json_line(&mut self, v: &serde_json::Value) -> bool {
        let ty = v["type"].as_str().unwrap_or("");
        // 1. Antigravity CLI format (`step_index` + `USER_INPUT` / `PLANNER_RESPONSE` / `GENERIC` / `CHECKPOINT`).
        if v.get("step_index").is_some() || matches!(ty, "USER_INPUT" | "PLANNER_RESPONSE" | "GENERIC" | "CHECKPOINT") {
            self.format = TranscriptFormat::Agy;
            if self.agent_label == "Agent" {
                self.agent_label = "Antigravity".into();
            }
            self.parse_agy_step(v, ty);
            return true;
        }
        // 2. Codex rollout format (`session_meta`, `response_item`, `event_msg`).
        if matches!(ty, "session_meta" | "response_item" | "event_msg" | "turn_context") {
            self.format = TranscriptFormat::Codex;
            if self.agent_label == "Agent" {
                self.agent_label = "Codex".into();
            }
            self.parse_codex_item(v, ty);
            return true;
        }
        // 3. Claude Code format (`user`, `assistant`, `system`, `summary`, `attachment`).
        if matches!(ty, "user" | "assistant" | "system" | "summary" | "progress") || v.get("message").is_some() {
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
        if ty == "summary"
            && let Some(s) = v["summary"].as_str()
            && !s.trim().is_empty()
        {
            let id = self.alloc_id();
            self.entries.push(TranscriptEntry::Divider { id, label: s.trim().to_owned(), timestamp: ts });
            return;
        }
        if ty == "progress" || v.get("attachment").is_some() {
            return;
        }
        let Some(msg) = v.get("message") else { return };
        let role = msg["role"].as_str().unwrap_or(ty);
        match &msg["content"] {
            serde_json::Value::String(text) => {
                let cleaned = clean_user_text(text);
                if cleaned.is_empty() {
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
                            let output = extract_content_text(&block["content"]);
                            if let Some(&idx) = self.call_index.get(call_id)
                                && let Some(TranscriptEntry::ToolCall { output: slot, status, .. }) = self.entries.get_mut(idx)
                            {
                                *slot = Some(truncate_output(&output));
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
                self.flush_text(role, &ts, &mut text_buf);
            }
            _ => {}
        }
    }

    fn parse_codex_item(&mut self, v: &serde_json::Value, ty: &str) {
        let ts = short_time(v["timestamp"].as_str().unwrap_or(""));
        if ty == "session_meta" {
            if let Some(nick) = v.pointer("/payload/agent_nickname").and_then(|s| s.as_str())
                && !nick.is_empty()
            {
                self.agent_label = nick.to_owned();
            }
            return;
        }
        if ty == "event_msg" {
            let et = v.pointer("/payload/type").and_then(|s| s.as_str()).unwrap_or("");
            if et == "task_complete" {
                let id = self.alloc_id();
                self.entries.push(TranscriptEntry::Divider { id, label: "Turn complete".into(), timestamp: ts });
            }
            return;
        }
        if ty != "response_item" {
            return;
        }
        let payload = &v["payload"];
        let ptype = payload["type"].as_str().unwrap_or("");
        match ptype {
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
                let name = payload["name"].as_str().unwrap_or("Tool").to_owned();
                let call_id = payload["call_id"].as_str().map(str::to_owned);
                let raw_args = payload["arguments"].as_str().or_else(|| payload["input"].as_str()).unwrap_or("");
                let parsed_args: serde_json::Value = serde_json::from_str(raw_args).unwrap_or_else(|_| serde_json::Value::String(raw_args.to_owned()));
                let summary = summarize_tool_args(&name, &parsed_args);
                let input = pretty_json_or_str(&parsed_args);
                let id = self.alloc_id();
                let idx = self.entries.len();
                if let Some(cid) = &call_id {
                    self.call_index.insert(cid.clone(), idx);
                }
                self.entries.push(TranscriptEntry::ToolCall { id, call_id, name, summary, input, output: None, status: ToolStatus::Running, timestamp: ts });
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = payload["call_id"].as_str().unwrap_or("");
                let output = payload["output"].as_str().map(str::to_owned).unwrap_or_else(|| extract_content_text(&payload["output"]));
                if let Some(&idx) = self.call_index.get(call_id)
                    && let Some(TranscriptEntry::ToolCall { output: slot, status, .. }) = self.entries.get_mut(idx)
                {
                    *slot = Some(truncate_output(&output));
                    *status = ToolStatus::Ok;
                }
            }
            _ => {}
        }
    }

    fn parse_agy_step(&mut self, v: &serde_json::Value, ty: &str) {
        let ts = short_time(v["created_at"].as_str().unwrap_or(""));
        match ty {
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
                if let Some(thinking) = v["thinking"].as_str()
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
            "GENERIC" => {
                let content = v["content"].as_str().unwrap_or("").trim();
                let is_err = v["status"].as_str().is_some_and(|s| s.eq_ignore_ascii_case("ERROR") || s.eq_ignore_ascii_case("FAILED"))
                    || content.contains("Command exited with code 1")
                    || content.starts_with("Error:");
                if !self.pending_agy_calls.is_empty() {
                    let idx = self.pending_agy_calls.remove(0);
                    if let Some(TranscriptEntry::ToolCall { output, status, .. }) = self.entries.get_mut(idx) {
                        *output = Some(truncate_output(content));
                        *status = if is_err { ToolStatus::Error } else { ToolStatus::Ok };
                        return;
                    }
                }
                if !content.is_empty() {
                    let id = self.alloc_id();
                    self.entries.push(TranscriptEntry::ToolCall {
                        id,
                        call_id: None,
                        name: "Output".into(),
                        summary: one_line(content, 80),
                        input: String::new(),
                        output: Some(truncate_output(content)),
                        status: if is_err { ToolStatus::Error } else { ToolStatus::Ok },
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
                                let (plain, _) = parse_inline_code(t);
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
                    let (plain, _) = parse_inline_code(text);
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

fn clean_user_text(raw: &str) -> String {
    let s = raw.trim();
    if let Some(start) = s.find("<USER_REQUEST>") {
        let rest = &s[start + "<USER_REQUEST>".len()..];
        if let Some(end) = rest.find("</USER_REQUEST>") {
            return rest[..end].trim().to_owned();
        }
        return rest.trim().to_owned();
    }
    s.to_owned()
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
        codex.push_line(r#"{"type":"response_item","timestamp":"2026-11-25T11:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix bug"}]}}"#);
        codex.push_line(r#"{"type":"response_item","timestamp":"2026-11-25T11:00:02Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"cmd\":\"git diff\"}"}}"#);
        codex.push_line(r#"{"type":"response_item","timestamp":"2026-11-25T11:00:03Z","payload":{"type":"function_call_output","call_id":"c1","output":"clean"}}"#);
        assert_eq!(codex.agent_label, "Newton");
        assert_eq!(codex.entries.len(), 2);

        let mut agy = TranscriptDoc::new("Antigravity");
        agy.push_line(r#"{"step_index":0,"type":"USER_INPUT","created_at":"2026-11-25T12:00:00Z","content":"<USER_REQUEST>\nReview shots\n</USER_REQUEST>"}"#);
        agy.push_line(r#"{"step_index":1,"type":"PLANNER_RESPONSE","created_at":"2026-11-25T12:00:02Z","thinking":"Checking PNGs","content":"Looks sharp.","tool_calls":[{"name":"read_file","args":{"Path":"\"/a/b/shot.png\""}}]}"#);
        agy.push_line(r#"{"step_index":2,"type":"GENERIC","status":"DONE","created_at":"2026-11-25T12:00:03Z","content":"OK"}"#);
        assert_eq!(agy.format, TranscriptFormat::Agy);
        assert_eq!(agy.entries.len(), 4);
        assert_eq!(agy.find("Review shots").len(), 1);

        let cmd = tmux_attach_command(&TmuxTarget { socket: "/tmp/tmux-1000/default".into(), session: "agy-1".into(), target: "agy-1:0.0".into(), cols: 120, rows: 40, pty_cols: 120, pty_rows: 41 });
        assert!(cmd.contains("attach -r -f ignore-size -t"));
    }
}
