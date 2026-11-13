//! The conversation as both frontends show it: a list of entries (what the
//! person said, what the agent said, each action it took, what happened to
//! the session) reduced from the session's events, and the rich text made
//! from them. Each entry's text is made once and kept until the entry
//! changes, so a long conversation costs only what's new.
//!
//! An action is one line ("Ran `cargo test` · exit 0 · 2 s"); what it showed
//! (its output, a diff, a file) opens under it. Short diffs and failures
//! open by themselves; anything else waits to be asked for.
use crate::rich::{self, Block, Text, Tone, ToolState};
use olo_coder_core::Event;
use serde_json::Value;
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

/// Entries kept for the view; earlier ones stay in the session's transcript.
const KEEP: usize = 1200;
/// An action's output, as shown when it's opened.
const OUTPUT_BYTES: usize = 48 * 1024;
/// Diffs up to this many lines open by themselves.
const SHORT_DIFF: usize = 40;
/// The live tail of a running action's output.
const LIVE_LINES: usize = 12;

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub item: Item,
    rendered: Option<Arc<Text>>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Welcome,
    User {
        text: String,
        from: Option<String>,
    },
    Assistant {
        text: String,
    },
    Tool(Tool),
    Note {
        tone: Tone,
        title: String,
        text: String,
    },
    /// Markdown shown as it is (a component, a plan, a command's answer).
    Markdown {
        text: String,
        block: Block,
    },
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub call: String,
    /// One line of Markdown: what was done to what.
    pub title: String,
    pub state: ToolState,
    /// After the title: an exit status, a count, how long.
    pub summary: String,
    /// What it showed: a fence's language and the text.
    pub output: Option<(String, String)>,
    /// Opens by itself (a short diff, a failure).
    pub open: bool,
    pub started: u64,
}

#[derive(Default)]
pub struct Feed {
    pub entries: VecDeque<Entry>,
    /// Entries let go of at the start.
    pub dropped: usize,
    next: u64,
    /// The agent's words as they arrive.
    pub live: String,
    /// The running action's output as it arrives.
    pub live_output: String,
    /// What the model is thinking about, while it does.
    pub reasoning: String,
    /// Entries opened (or closed) against their default.
    flipped: HashSet<u64>,
    pub seq: i64,
    /// Bumped whenever what's shown changes.
    pub generation: u64,
}

impl Feed {
    pub fn welcome() -> Feed {
        let mut f = Feed::default();
        f.push(Item::Welcome);
        f
    }

    fn push(&mut self, item: Item) -> u64 {
        // The welcome is for an empty conversation: it goes when the
        // person first says something.
        if matches!(item, Item::User { .. }) && matches!(self.entries.front().map(|e| &e.item), Some(Item::Welcome)) {
            self.entries.pop_front();
        }
        self.next += 1;
        self.entries.push_back(Entry { id: self.next, item, rendered: None });
        while self.entries.len() > KEEP {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.generation += 1;
        self.next
    }

    /// A note from the frontend itself (a command's answer, an error).
    pub fn notice(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(rest) = text.strip_prefix("Error: ") {
            self.push(Item::Note { tone: Tone::Failure, title: "That didn’t work".into(), text: rest.into() });
        } else {
            self.push(Item::Markdown { text: text.into(), block: Block::Body });
        }
    }

    /// Open or close entry `id`'s output.
    pub fn toggle(&mut self, id: u64) {
        if !self.flipped.remove(&id) {
            self.flipped.insert(id);
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.rendered = None;
        }
        self.generation += 1;
    }

    /// Open (or close) every action's output.
    pub fn open_all(&mut self, open: bool) {
        self.flipped.clear();
        for e in &mut self.entries {
            if let Item::Tool(t) = &e.item {
                if t.output.is_some() && t.open != open {
                    self.flipped.insert(e.id);
                }
                e.rendered = None;
            }
        }
        self.generation += 1;
    }

    fn tool(&mut self, call: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().take(400).find(|e| matches!(&e.item, Item::Tool(t) if t.call == call))
    }

    fn changed(&mut self, call: &str, f: impl FnOnce(&mut Tool)) -> bool {
        let Some(e) = self.tool(call) else { return false };
        if let Item::Tool(t) = &mut e.item {
            f(t);
        }
        e.rendered = None;
        self.generation += 1;
        true
    }

    pub fn event(&mut self, e: &Event) {
        let d = &e.data;
        let text = |v: &Value| v.as_str().unwrap_or("").to_owned();
        let before = self.generation;
        match e.kind.as_str() {
            "delta" => {
                self.reasoning.clear();
                self.live.push_str(d["text"].as_str().unwrap_or(""));
                if self.live.len() > 64000 {
                    self.live = olo_coder_core::tools::clip(&self.live, 64000);
                }
                self.generation += 1;
            }
            "reasoning" => {
                self.reasoning.push_str(d["text"].as_str().unwrap_or(""));
                if self.reasoning.len() > 4000 {
                    let keep = self.reasoning.char_indices().rev().nth(2000).map_or(0, |(n, _)| n);
                    self.reasoning = self.reasoning[keep..].into();
                }
                self.generation += 1;
            }
            "tool_output" => {
                self.live_output.push_str(d["text"].as_str().unwrap_or(""));
                if self.live_output.len() > 16000 {
                    let keep = self.live_output.char_indices().rev().nth(8000).map_or(0, |(n, _)| n);
                    self.live_output = self.live_output[keep..].into();
                }
                self.generation += 1;
            }
            "assistant" => {
                self.live.clear();
                self.reasoning.clear();
                if let Some(t) = d["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.push(Item::Assistant { text: t.trim().into() });
                }
                self.generation += 1;
            }
            "messages_delivered" => {
                self.live.clear();
                for m in d["messages"].as_array().into_iter().flatten() {
                    let from = m["from"]["title"].as_str().map(str::to_owned);
                    self.push(Item::User { text: text(&m["text"]), from });
                }
                let skipped = d["skipped_calls"].as_array().map_or(0, Vec::len);
                if skipped > 0 {
                    let s = if skipped == 1 { "" } else { "s" };
                    self.push(Item::Note { tone: Tone::Info, title: "Steered".into(), text: format!("{skipped} planned action{s} skipped for the new message.") });
                }
            }
            "user" => {
                self.push(Item::User { text: text(&d["text"]), from: None });
            }
            "todos_edited" => {
                let said: Vec<&str> = d["said"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if !said.is_empty() {
                    // Edits one after another make one note.
                    let said = said.join("; ");
                    if let Some(e) = self.entries.back_mut().filter(|e| matches!(&e.item, Item::Markdown { text, block: Block::Quiet } if text.starts_with(EDITED))) {
                        if let Item::Markdown { text, .. } = &mut e.item {
                            text.truncate(text.len() - 1);
                            text.push_str(&format!("; {said}."));
                        }
                        e.rendered = None;
                        self.generation += 1;
                    } else {
                        self.push(Item::Markdown { text: format!("{EDITED}{said}."), block: Block::Quiet });
                    }
                }
            }
            "approval" => {
                let call = &d["call"];
                let id = text(&call["id"]);
                let reason = text(&d["reason"]);
                if !self.changed(&id, |t| {
                    t.state = ToolState::Waiting;
                    t.summary = "needs your approval".into();
                }) {
                    let mut t = tool(call, e.time);
                    t.state = ToolState::Waiting;
                    t.summary = "needs your approval".into();
                    if !reason.is_empty() {
                        t.output = Some((String::new(), reason));
                    }
                    self.push(Item::Tool(t));
                }
            }
            "tool_started" => {
                let call = &d["call"];
                let name = call["name"].as_str().unwrap_or("");
                if olo_coder_core::tools::internal(name) {
                    return;
                }
                self.live.clear();
                self.live_output.clear();
                let id = text(&call["id"]);
                let fresh = tool(call, e.time);
                if !self.changed(&id, |t| {
                    t.state = ToolState::Running;
                    t.summary.clear();
                    t.output = None;
                    t.started = e.time;
                }) {
                    self.push(Item::Tool(fresh));
                }
            }
            "tool_denied" => {
                let call = &d["call"];
                let id = text(&call["id"]);
                if !self.changed(&id, |t| {
                    t.state = ToolState::Denied;
                    t.summary = "not allowed".into();
                }) {
                    let mut t = tool(call, e.time);
                    t.state = ToolState::Denied;
                    t.summary = "not allowed".into();
                    self.push(Item::Tool(t));
                }
            }
            "tool_result" => self.result(e),
            "error" | "paused" | "interrupted" => {
                self.live.clear();
                self.live_output.clear();
                self.reasoning.clear();
                let reason = d["message"].as_str().or(d["reason"].as_str()).unwrap_or("");
                let (tone, title, text) = match (e.kind.as_str(), reason) {
                    ("paused", "cancelled") | ("interrupted", _) => (Tone::Info, "Stopped", "Nothing more runs until you send a message or choose Resume."),
                    ("paused", _) => (Tone::Warning, "Paused", reason),
                    _ => (Tone::Failure, "Stopped by an error", reason),
                };
                self.push(Item::Note { tone, title: title.into(), text: text.into() });
            }
            "retry" => {
                let delay = d["seconds"].as_u64().map(|n| format!(" in {n} s")).unwrap_or_default();
                let reason = d["reason"].as_str().map(|s| format!(": {s}")).unwrap_or_default();
                self.push(Item::Note { tone: Tone::Info, title: "Reconnecting".into(), text: format!("Trying the model again{delay} (attempt {}){reason}", d["attempt"]) });
            }
            "compaction_applied" => {
                let id = d["checkpoint_id"].as_str().unwrap_or("");
                self.push(Item::Note {
                    tone: Tone::Info,
                    title: "Context checkpoint".into(),
                    text: format!(
                        "Earlier work is summarised; {} tokens in context now. The whole conversation is kept. [Checkpoint](olo-coder:compaction/{id}) · [Transcript](olo-coder:transcript)",
                        thousands(d["context_tokens"].as_u64().unwrap_or(0))
                    ),
                });
            }
            "compaction_restored" => self.push_note(Tone::Info, "Context restored", "The original messages are back in context."),
            "compaction_deferred" => self.push_note(Tone::Info, "Checkpoint waits", d["reason"].as_str().unwrap_or("The conversation changed while it was summarised.")),
            "compacted" => self.push_note(Tone::Info, "Context compacted", "The model’s context was summarised; the whole conversation is kept."),
            "auto_review" if d["allow"] != true => self.push_note(Tone::Warning, "Review asks for you", d["reason"].as_str().unwrap_or("The automatic review wants your decision.")),
            "supervisor_review" => {
                let action = d["action"].as_str().unwrap_or("review");
                if action != "continue_watching" && action != "none" {
                    let body = [d["reason"].as_str().unwrap_or(""), d["message"].as_str().unwrap_or("")].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
                    self.push(Item::Note { tone: Tone::Info, title: format!("Supervisor: {}", action.replace('_', " ")), text: body });
                }
            }
            "supervisor_error" => self.push_note(Tone::Warning, "Supervisor couldn’t review", d["message"].as_str().unwrap_or("")),
            "mcp_unavailable" => {
                let title = format!("{} is unavailable", d["server"].as_str().unwrap_or("A connection"));
                self.push_note(Tone::Warning, &title, d["message"].as_str().unwrap_or(""));
            }
            "integration_login" => {
                let title = format!("{}: {}", d["server"].as_str().unwrap_or("Connection"), d["status"]["state"].as_str().unwrap_or(""));
                self.push_note(Tone::Info, &title, d["status"]["message"].as_str().unwrap_or(""));
            }
            "goal" => {
                if let Some(objective) = d["goal"]["objective"].as_str().or(d["objective"].as_str()) {
                    self.push_note(Tone::Info, "Goal", objective);
                }
            }
            "spec_accepted" => self.push_note(Tone::Success, "Specification accepted", "The agent works to it now and checks each criterion."),
            "child_finished" => {
                let title = format!("Subagent finished: {}", d["title"].as_str().unwrap_or("task"));
                self.push_note(Tone::Info, &title, d["summary"].as_str().or(d["result"].as_str()).unwrap_or(""));
            }
            "undo" => self.push_note(Tone::Info, "Undone", "The file is back as it was before that edit."),
            _ => {}
        }
        if self.generation == before && !matches!(e.kind.as_str(), "delta" | "reasoning" | "tool_output") {
            // (Nothing shown changed.)
        }
    }

    fn push_note(&mut self, tone: Tone, title: &str, text: &str) {
        self.push(Item::Note { tone, title: title.into(), text: text.into() });
    }

    fn result(&mut self, e: &Event) {
        let d = &e.data;
        let name = d["name"].as_str().unwrap_or("");
        let output = &d["output"];
        let failed = d["error"] == true;
        self.live.clear();
        self.live_output.clear();
        if !failed {
            match name {
                "set_todos" => {
                    // The list itself is beside the conversation (and in the
                    // terminal's Tasks); here, only that it was made.
                    let todos = output["todos"].as_array().cloned().unwrap_or_default();
                    let done = todos.iter().filter(|t| t["status"] == "done").count();
                    self.push(Item::Markdown { text: format!("Task list · {done} of {} done", todos.len()), block: Block::Quiet });
                    return;
                }
                "update_todos" | "report_status" => return,
                "complete_goal" => {
                    self.push_note(Tone::Success, "Goal reached", output["goal"]["evidence"].as_str().unwrap_or("Verified."));
                    return;
                }
                "update_spec" => {
                    let spec = output["spec"]["text"].as_str().or(output["text"].as_str()).unwrap_or("");
                    self.push(Item::Markdown { text: format!("**Specification** · revision {}\n\n{spec}", output["spec"]["revision"]), block: Block::Body });
                    return;
                }
                "present" => {
                    self.push(Item::Markdown { text: component(&output["component"]), block: Block::Body });
                    return;
                }
                "remember" | "update_criterion" | "set_terminal_title" | "pause_work" => return,
                _ => {}
            }
        }
        let call = d["call_id"].as_str().unwrap_or("").to_owned();
        let (summary, body, open) = outcome(output, failed);
        let time = e.time;
        let state = if !failed {
            ToolState::Done
        } else if output["error"].as_str().is_some_and(|r| r.starts_with("cancelled")) {
            ToolState::Stopped
        } else {
            ToolState::Failed
        };
        let updated = self.changed(&call, |t| {
            let took = time.saturating_sub(t.started);
            t.state = state;
            t.summary = with_time(summary.clone(), took);
            t.output = body.clone();
            t.open = open;
        });
        if !updated && !olo_coder_core::tools::internal(name) {
            let mut t = tool(&serde_json::json!({"id":call,"name":name,"arguments":{}}), time);
            t.state = state;
            t.summary = summary;
            t.output = body;
            t.open = open;
            self.push(Item::Tool(t));
        }
    }

    /// The conversation as rich text, made with `r` (on a worker).
    pub fn document(&mut self, r: &mut rich::Renderer) -> Text {
        let mut doc = Text::default();
        if self.dropped > 0 {
            let earlier = r.render(&format!("*{} earlier entries are in the [transcript](olo-coder:transcript).*", thousands(self.dropped as u64)));
            doc.append(&earlier, Block::Quiet, true);
        }
        let flipped = std::mem::take(&mut self.flipped);
        let mut running = None;
        for e in &mut self.entries {
            if e.rendered.is_none() {
                let (md, _) = markdown(e.id, &e.item, &flipped);
                let mut t = r.render(&md);
                if let Item::Tool(tool) = &e.item {
                    // The line itself, then its output.
                    for p in t.paragraphs.iter_mut().skip(1) {
                        p.block = Block::Output;
                    }
                    if let Some(first) = t.paragraphs.first_mut() {
                        first.block = Block::Tool(tool.state);
                    }
                }
                e.rendered = Some(Arc::new(t));
            }
            if let Item::Tool(t) = &e.item {
                if t.state == ToolState::Running {
                    running = Some(e.id);
                }
            }
            let block = match &e.item {
                Item::User { .. } => Block::User,
                Item::Note { tone, .. } => Block::Note(*tone),
                Item::Markdown { block, .. } => *block,
                _ => Block::Body,
            };
            doc.append(e.rendered.as_ref().unwrap(), block, true);
        }
        self.flipped = flipped;
        if running.is_some() && !self.live_output.is_empty() {
            let tail: Vec<&str> = self.live_output.lines().rev().take(LIVE_LINES).collect();
            let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            doc.append(&r.render(&fenced("", &tail)), Block::Output, false);
        }
        if !self.reasoning.trim().is_empty() && self.live.is_empty() {
            let last = self.reasoning.trim().lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").trim_matches('*');
            doc.append(&r.render(&format!("*{}*", one_line(last))), Block::Quiet, true);
        }
        if !self.live.is_empty() {
            doc.append(&r.render(&self.live), Block::Body, true);
        }
        doc
    }
}

/// An entry as Markdown, and the block its paragraphs are in.
fn markdown(id: u64, item: &Item, flipped: &HashSet<u64>) -> (String, Block) {
    match item {
        Item::Welcome => (
            "## What shall we build?\n\nDescribe a task, ask about the code, or plan it first with **Agent ▸ Spec Mode**. Set a goal (**Agent ▸ Set Goal…**) for work that spans days: the session, its decisions and its checkpoints are kept.".into(),
            Block::Quiet,
        ),
        Item::User { text, from } => {
            let mut md = String::new();
            if let Some(from) = from {
                md.push_str(&format!("**From {}**\n\n", one_line(from)));
            }
            md.push_str(text);
            (md, Block::User)
        }
        Item::Assistant { text } => (text.clone(), Block::Body),
        Item::Note { title, text, tone } => {
            let md = if text.trim().is_empty() { format!("**{title}**") } else { format!("**{title}** — {text}") };
            (md, Block::Note(*tone))
        }
        Item::Markdown { text, block } => (text.clone(), *block),
        Item::Tool(t) => {
            let open = t.output.is_some() && (t.open != flipped.contains(&id));
            let mark = match (t.output.is_some(), open) {
                (false, _) => "•",
                (true, false) => "▸",
                (true, true) => "▾",
            };
            let summary = if t.summary.is_empty() { String::new() } else { format!(" · {}", t.summary) };
            let line = if t.output.is_some() { format!("[{mark} {}](olo-coder:toggle/{id}){summary}", t.title) } else { format!("{mark} {}{summary}", t.title) };
            let mut md = line;
            if open {
                if let Some((lang, text)) = &t.output {
                    md.push_str("\n\n");
                    md.push_str(&fenced(lang, &olo_coder_core::tools::clip(text, OUTPUT_BYTES)));
                }
            }
            (md, Block::Body)
        }
    }
}

/// How a note of the person's edits to the task list begins.
const EDITED: &str = "You edited the task list: ";

/// A new action's entry from its call.
fn tool(call: &Value, time: u64) -> Tool {
    let name = call["name"].as_str().unwrap_or("tool").to_owned();
    let a = &call["arguments"];
    let s = |k: &str| a[k].as_str().unwrap_or("");
    let title = match name.as_str() {
        "execute" => {
            let args: Vec<&str> = a["args"].as_array().map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let mut line = s("program").to_owned();
            for arg in args {
                line.push(' ');
                if arg.contains(' ') { line.push_str(&format!("'{arg}'")) } else { line.push_str(arg) }
            }
            format!("Ran {}", code(&line))
        }
        "read_file" => format!("Read {}", code(s("path"))),
        "write_file" => format!("Wrote {}", code(s("path"))),
        "edit_file" => format!("Edited {}", code(s("path"))),
        "delete_file" => format!("Deleted {}", code(s("path"))),
        "create_directory" => format!("Made folder {}", code(s("path"))),
        "list_files" => match s("path") {
            "" | "." | "./" => "Listed the project's files".into(),
            path => format!("Listed {}", code(path)),
        },
        "search" => format!("Searched for {}", code(s("pattern"))),
        "fetch_url" => format!("Fetched {}", code(s("url"))),
        "repository_read" | "repository_write" => format!("Git {}", code(s("operation"))),
        "github_read" | "github_write" => format!("GitHub {}", code(s("operation"))),
        "screen_capture" => "Looked at the screen".into(),
        "screen_read" => "Read the screen".into(),
        "screen_control" => format!("Used the screen: {}", code(s("action"))),
        "send_message" => "Sent a message".into(),
        "read_granted_secret" => format!("Read the shared password {}", code(s("name"))),
        "history_search" | "search_transcript" => format!("Searched the conversation for {}", code(s("query"))),
        "read_transcript" => "Read the transcript".into(),
        "read_skill" => format!("Read skill {}", code(s("path"))),
        "wait_agent" | "agent_status" => "Checked on a subagent".into(),
        n if n.starts_with("mcp__") => {
            let mut parts = n.trim_start_matches("mcp__").splitn(2, "__");
            let server = parts.next().unwrap_or("");
            let tool = parts.next().unwrap_or("");
            format!("{server}: {}", tool.replace('_', " "))
        }
        n => {
            let mut words = n.replace('_', " ");
            if let Some(c) = words.get(..1) {
                words = c.to_uppercase() + &words[1..];
            }
            words
        }
    };
    Tool { call: call["id"].as_str().unwrap_or("").to_owned(), title, state: ToolState::Running, summary: String::new(), output: None, open: false, started: time }
}

/// What an action's result says, what it shows when opened, and whether
/// that opens by itself.
fn outcome(o: &Value, failed: bool) -> (String, Option<(String, String)>, bool) {
    if failed {
        let reason = o["error"].as_str().unwrap_or("failed");
        if reason.starts_with("cancelled") {
            return ("stopped".into(), None, false);
        }
        let first = reason.lines().next().unwrap_or(reason);
        let summary = olo_coder_core::tools::clip(first, 140);
        let body = (reason.len() > first.len() || first.len() > 140).then(|| (String::new(), reason.to_owned()));
        return (format!("failed: {summary}"), body, true);
    }
    if let Some(diff) = o["diff"].as_str() {
        let (add, del) = diff.lines().fold((0, 0), |(a, d), l| {
            if l.starts_with("+++") || l.starts_with("---") {
                (a, d)
            } else if l.starts_with('+') {
                (a + 1, d)
            } else if l.starts_with('-') {
                (a, d + 1)
            } else {
                (a, d)
            }
        });
        let lines = diff.lines().count();
        return (format!("+{add} −{del}"), Some(("diff".into(), diff.into())), lines <= SHORT_DIFF);
    }
    if o.get("stdout").is_some() {
        let out = o["stdout"].as_str().unwrap_or("");
        let err = o["stderr"].as_str().unwrap_or("");
        let code = o["exit_code"].as_i64();
        let ok = o["success"] == true;
        let mut text = out.trim_end().to_owned();
        if !err.trim().is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(err.trim_end());
        }
        let summary = match (ok, code) {
            (true, _) => "done".to_owned(),
            (false, Some(c)) => format!("exit {c}"),
            (false, None) => "ended by a signal".to_owned(),
        };
        let body = (!text.is_empty()).then(|| (String::new(), text));
        return (summary, body, !ok);
    }
    if let Some(text) = o["text"].as_str() {
        let path = o["path"].as_str().unwrap_or("");
        let language = olo_kit_code_language(path, text);
        let source = if o.get("total_lines").is_some() {
            text.lines().map(|l| l.split_once(": ").filter(|(n, _)| n.bytes().all(|b| b.is_ascii_digit())).map_or(l, |(_, line)| line)).collect::<Vec<_>>().join("\n")
        } else {
            text.into()
        };
        let summary = match o["total_lines"].as_u64() {
            Some(total) => format!("{} of {} lines", source.lines().count(), thousands(total)),
            None => format!("{} lines", source.lines().count()),
        };
        return (summary, Some((language.into(), source)), false);
    }
    // Git's answers, as git would print them.
    if let (Some(files), true) = (o["files"].as_array(), o.get("branch").is_some()) {
        let more = if o["truncated"] == true { "+" } else { "" };
        let summary = match files.len() {
            0 => "clean".to_owned(),
            1 => format!("1{more} change"),
            n => format!("{n}{more} changes"),
        };
        let width = files.iter().filter_map(|f| f["state"].as_str()).map(str::len).max().unwrap_or(0);
        let text = files
            .iter()
            .map(|f| {
                let staged = if f["staged"] == true { " (staged)" } else { "" };
                format!("{:width$}  {}{staged}", f["state"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (summary, (!text.is_empty()).then(|| (String::new(), text)), false);
    }
    if let Some(commits) = o["commits"].as_array() {
        let s = if commits.len() == 1 { "" } else { "s" };
        let text = commits
            .iter()
            .map(|c| {
                let id = c["id"].as_str().unwrap_or("");
                let line = c["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
                format!("{}  {line}  ({})", id.get(..8).unwrap_or(id), c["author"].as_str().unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (format!("{} commit{s}", commits.len()), (!text.is_empty()).then(|| (String::new(), text)), false);
    }
    if let Some(branches) = o["branches"].as_array() {
        let s = if branches.len() == 1 { "" } else { "es" };
        let text = branches.iter().filter_map(|b| b["name"].as_str()).collect::<Vec<_>>().join("\n");
        return (format!("{} branch{s}", branches.len()), (!text.is_empty()).then(|| (String::new(), text)), false);
    }
    if let Some(results) = o["results"].as_array() {
        let more = if o["possibly_truncated"] == true { "+" } else { "" };
        let s = if results.len() == 1 { "" } else { "s" };
        let text = results
            .iter()
            .map(|r| match r {
                Value::String(p) => p.clone(),
                r => format!("{}:{}: {}", r["path"].as_str().unwrap_or(""), r["line"], r["text"].as_str().unwrap_or("").trim()),
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (format!("{}{more} result{s}", results.len()), (!text.is_empty()).then(|| (String::new(), text)), false);
    }
    if let Some(content) = o["content"].as_array() {
        let text = content.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
        return (String::new(), (!text.is_empty()).then(|| (String::new(), text)), false);
    }
    if o.is_null() || o.as_object().is_some_and(|m| m.is_empty()) {
        return (String::new(), None, false);
    }
    (String::new(), Some(("json".into(), serde_json::to_string_pretty(o).unwrap_or_default())), false)
}

/// The language a file's contents are coloured as.
fn olo_kit_code_language(path: &str, text: &str) -> &'static str {
    olo_syntax::detect(std::path::Path::new(path), text.lines().next().unwrap_or("")).map_or("", |l| l.id)
}

fn with_time(summary: String, seconds: u64) -> String {
    if seconds < 2 {
        return summary;
    }
    let took = if seconds < 90 { format!("{seconds} s") } else { format!("{} min", seconds.div_ceil(60)) };
    if summary.is_empty() { took } else { format!("{summary} · {took}") }
}

/// `s` as inline code, whatever backticks it has.
pub fn code(s: &str) -> String {
    let s = one_line(s);
    let s = olo_coder_core::tools::clip(&s, 160);
    let run = s.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(run + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') { " " } else { "" };
    if s.is_empty() { String::new() } else { format!("{fence}{pad}{s}{pad}{fence}") }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn fenced(language: &str, text: &str) -> String {
    let width = text.lines().map(|l| l.trim_start().chars().take_while(|c| *c == '`').count() + 1).max().unwrap_or(3).max(3);
    let language: String = language.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '#' | '.')).take(64).collect();
    let fence = "`".repeat(width);
    format!("\n{fence}{language}\n{text}\n{fence}\n")
}

pub fn component(v: &Value) -> String {
    let cell = |v: &Value| {
        let text = match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => v.to_string(),
        };
        text.replace('\n', " ").replace('|', "\\|")
    };
    let mut out = format!("**{}**\n\n", v["title"].as_str().unwrap_or("Result"));
    match v["kind"].as_str().unwrap_or("") {
        "table" => {
            if let Some(columns) = v["columns"].as_array() {
                out += &format!("| {} |\n| {} |\n", columns.iter().map(cell).collect::<Vec<_>>().join(" | "), vec!["---"; columns.len()].join(" | "));
            }
            for row in v["rows"].as_array().into_iter().flatten() {
                if let Some(cells) = row.as_array() {
                    out += &format!("| {} |\n", cells.iter().map(cell).collect::<Vec<_>>().join(" | "));
                }
            }
        }
        "checklist" => {
            for item in v["items"].as_array().into_iter().flatten() {
                out += &format!("- [{}] {}\n", if item["done"] == true { "x" } else { " " }, cell(&item["text"]));
            }
        }
        "progress" => {
            let value = v["value"].as_f64().unwrap_or(0.0);
            let total = v["total"].as_f64().unwrap_or(1.0);
            let done = ((value / total).clamp(0.0, 1.0) * 24.0).round() as usize;
            out += &format!("{}{} **{value} / {total}**\n", "━".repeat(done), "─".repeat(24 - done));
        }
        "chart" => {
            if let Some(values) = v["values"].as_array() {
                let maximum = values.iter().filter_map(Value::as_f64).map(f64::abs).fold(0.0, f64::max).max(f64::EPSILON);
                out += "| Measurement | Value | Chart |\n| --- | ---: | --- |\n";
                for (i, value) in values.iter().enumerate() {
                    let number = value.as_f64().unwrap_or(0.0);
                    let bars = (number.abs() / maximum * 24.0).round() as usize;
                    out += &format!("| {} | {number} | {} |\n", cell(&v["labels"][i]), "▰".repeat(bars));
                }
            }
        }
        "code" => out += &fenced(v["language"].as_str().unwrap_or(""), v["code"].as_str().unwrap_or("")),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn ev(seq: i64, kind: &str, data: Value) -> Event {
        Event { seq, session: "s".into(), time: 100 + seq as u64, kind: kind.into(), data }
    }
    #[test]
    fn an_action_is_one_line_that_opens() {
        let mut f = Feed::default();
        let mut r = rich::Renderer::default();
        f.event(&ev(1, "tool_started", json!({"call":{"id":"c1","name":"execute","arguments":{"program":"cargo","args":["test","--lib"]}}})));
        let doc = f.document(&mut r);
        assert!(doc.text.contains("Ran cargo test --lib"), "{}", doc.text);
        assert!(doc.paragraphs.iter().any(|p| p.block == Block::Tool(ToolState::Running)));
        f.event(&ev(5, "tool_result", json!({"call_id":"c1","name":"execute","error":false,"output":{"stdout":"running 3 tests\nok","stderr":"","exit_code":0,"success":true}})));
        let doc = f.document(&mut r);
        assert!(doc.text.contains("▸ Ran cargo test --lib · done · 4 s"), "{}", doc.text);
        assert!(!doc.text.contains("running 3 tests"), "output waits to be opened");
        let id = f.entries.back().unwrap().id;
        f.toggle(id);
        let doc = f.document(&mut r);
        assert!(doc.text.contains("running 3 tests"));
        assert!(doc.paragraphs.iter().any(|p| p.block == Block::Output));
        assert!(doc.runs.iter().any(|run| run.style.link.as_deref() == Some(&format!("olo-coder:toggle/{id}"))));
    }
    #[test]
    fn failures_and_short_diffs_open_by_themselves() {
        let mut f = Feed::default();
        let mut r = rich::Renderer::default();
        f.event(&ev(1, "tool_started", json!({"call":{"id":"c1","name":"edit_file","arguments":{"path":"src/lib.rs"}}})));
        f.event(&ev(2, "tool_result", json!({"call_id":"c1","name":"edit_file","error":false,"output":{"diff":"--- a\n+++ b\n@@\n-old\n+new\n+more"}})));
        f.event(&ev(3, "tool_started", json!({"call":{"id":"c2","name":"execute","arguments":{"program":"false"}}})));
        f.event(&ev(4, "tool_result", json!({"call_id":"c2","name":"execute","error":false,"output":{"stdout":"","stderr":"boom","exit_code":1,"success":false}})));
        let doc = f.document(&mut r);
        assert!(doc.text.contains("▾ Edited src/lib.rs · +2 −1"), "{}", doc.text);
        assert!(doc.text.contains("+more"));
        assert!(doc.text.contains("exit 1"));
        assert!(doc.text.contains("boom"));
    }
    #[test]
    fn messages_appear_when_delivered_and_notes_read_plainly() {
        let mut f = Feed::default();
        let mut r = rich::Renderer::default();
        f.event(&ev(1, "message_queued", json!({"message":{"id":"m","text":"Fix the build"}})));
        assert!(f.entries.is_empty(), "a pending message waits in the queue, not the conversation");
        f.event(&ev(2, "messages_delivered", json!({"ids":["m"],"messages":[{"id":"m","text":"Fix the build"}],"skipped_calls":[]})));
        f.event(&ev(3, "paused", json!({"message":"cancelled"})));
        let doc = f.document(&mut r);
        assert!(doc.text.contains("Fix the build"));
        assert!(doc.paragraphs.iter().any(|p| p.block == Block::User));
        assert!(doc.text.contains("Stopped"));
        assert!(!doc.text.contains("cancelled"));
    }
    #[test]
    fn internal_tools_become_content_not_actions() {
        let mut f = Feed::default();
        let mut r = rich::Renderer::default();
        f.event(&ev(1, "tool_started", json!({"call":{"id":"t","name":"set_todos","arguments":{}}})));
        f.event(&ev(2, "tool_result", json!({"call_id":"t","name":"set_todos","error":false,"output":{"todos":[{"text":"Write it","status":"done"},{"text":"Test it","status":"in_progress"}]}})));
        f.event(&ev(3, "tool_started", json!({"call":{"id":"r","name":"report_status","arguments":{}}})));
        f.event(&ev(4, "tool_result", json!({"call_id":"r","name":"report_status","error":false,"output":{"report":{"doing":["Testing"]}}})));
        f.event(&ev(5, "todos_edited", json!({"said":["marked “Test it” done"]})));
        f.event(&ev(6, "todos_edited", json!({"said":["added “Ship it”"]})));
        let doc = f.document(&mut r);
        assert!(doc.text.contains("You edited the task list: marked “Test it” done; added “Ship it”."), "edits in a row make one note: {}", doc.text);
        assert!(doc.text.contains("Task list · 1 of 2 done"), "{}", doc.text);
        assert!(!doc.text.contains("Testing"), "status reports are shown beside the conversation");
        assert!(!doc.paragraphs.iter().any(|p| matches!(p.block, Block::Tool(_))));
    }
    #[test]
    fn inline_code_survives_backticks() {
        assert_eq!(code("a`b"), "``a`b``");
        assert_eq!(code("`x`"), "`` `x` ``");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
