//! Shared presentation worker. Both frontends submit the same command strings
//! and reduce the same event stream. No network or filesystem I/O on the UI thread.
use anyhow::{Context, Result};
use olo_coder_core::{
    commands::{self, Input},
    rpc, *,
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    sync::{Arc, mpsc},
};

#[derive(Clone, Debug)]
pub enum Update {
    Session(Box<Session>),
    Catalog { rows: Vec<Value>, next_offset: Option<usize> },
    InputResult { session: String, text: String, error: Option<String> },
    Repository { session: String, result: Value },
    Models { provider: Provider, ids: Vec<String> },
    Event(Event),
    Notice(String),
    Connection(Option<String>),
    Opened(String),
    Rendered(Arc<olo_ui::rich_text::Text>),
}
pub enum Action {
    Submit(String),
    Steer(String, Delivery),
    Browse(SessionFilter),
    Refresh,
    Repository,
    Models,
    Stop,
    Approve(bool),
    Quit,
}
#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Action>,
}
impl Handle {
    pub fn submit(&self, text: impl Into<String>) {
        let _ = self.tx.send(Action::Submit(text.into()));
    }
}
struct Following {
    stream: Arc<olo_sync::Mutex<Option<std::os::unix::net::UnixStream>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    wake: mpsc::Sender<()>,
}
impl Drop for Following {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.wake.send(());
        if let Some(s) = self.stream.lock().unwrap().as_ref() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}
fn follow(dir: &std::path::Path, session: &str, filter: SessionFilter, callback: Arc<dyn Fn(Update) + Send + Sync>, actions: mpsc::Sender<Action>) -> Result<Following> {
    let first = std::os::unix::net::UnixStream::connect(rpc::socket(dir)).ok();
    let stream = Arc::new(olo_sync::Mutex::new(first.as_ref().map(|s| s.try_clone()).transpose()?));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (wake, wait) = mpsc::channel();
    let held = Following { stream: stream.clone(), stop: stop.clone(), wake };
    let dir = dir.to_owned();
    let session = session.to_string();
    std::thread::Builder::new().name("coder-events".into()).spawn(move || {
        let mut next = first;
        let mut cursor = -1;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(mut connected) = next.take() {
                let mut replaying = true;
                let request = if session.is_empty() { json!({"subscribe_sessions":true,"filter":filter}) } else { json!({"subscribe":true,"session":session,"after":cursor}) };
                let _ = writeln!(connected, "{request}");
                for line in std::io::BufReader::new(connected).lines() {
                    let Ok(line) = line else { break };
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if let Some(catalog) = v.get("catalog") {
                        if let Some(rows) = catalog["sessions"].as_array() {
                            callback(Update::Catalog { rows: rows.clone(), next_offset: catalog["next_offset"].as_u64().map(|v| v as usize) });
                        }
                        continue;
                    }
                    if v["ready"] == true {
                        replaying = false;
                        callback(Update::Connection(None));
                        cursor = cursor.max(v["after"].as_i64().unwrap_or(0));
                        let _ = actions.send(Action::Refresh);
                        continue;
                    }
                    if v["reconnect"] == true {
                        break;
                    }
                    if let Ok(event) = serde_json::from_value::<Event>(v) {
                        cursor = cursor.max(event.seq);
                        let refresh = !replaying
                            && event.seq > 0
                            && matches!(
                                event.kind.as_str(),
                                "idle"
                                    | "approval"
                                    | "approval_decision"
                                    | "tool_result"
                                    | "error"
                                    | "paused"
                                    | "configured"
                                    | "spec"
                                    | "spec_mode"
                                    | "spec_accepted"
                                    | "goal"
                                    | "todos"
                                    | "goal_complete"
                                    | "compacted"
                                    | "criterion"
                                    | "running"
                                    | "memory"
                                    | "stop"
                                    | "resume"
                                    | "attachment"
                                    | "undo"
                                    | "child_finished"
                                    | "organized"
                                    | "message_queued"
                                    | "messages_delivered"
                                    | "message_cancelled"
                                    | "message_edited"
                                    | "message_moved"
                            );
                        let repository_changed = !replaying
                            && (event.kind == "undo"
                                || (event.kind == "tool_result"
                                    && matches!(event.data["name"].as_str(), Some("write_file" | "edit_file" | "delete_file" | "create_directory" | "execute" | "repository_write"))));
                        callback(Update::Event(event));
                        if repository_changed {
                            let _ = actions.send(Action::Repository);
                        }
                        if refresh {
                            let _ = actions.send(Action::Refresh);
                        }
                    }
                }
            }
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if !session.is_empty() {
                callback(Update::Connection(Some("Reconnecting to the session service…".into())));
            }
            // Offline backoff only; connected clients sleep on their socket forever.
            for attempt in 0.. {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                if super::ensure_service(&dir).is_ok() {
                    if let Ok(s) = std::os::unix::net::UnixStream::connect(rpc::socket(&dir)) {
                        *stream.lock().unwrap() = s.try_clone().ok();
                        next = Some(s);
                        break;
                    }
                }
                if wait.recv_timeout(std::time::Duration::from_secs(1u64 << attempt.min(5))).is_ok() {
                    return;
                }
            }
        }
    })?;
    Ok(held)
}
pub fn start(dir: PathBuf, workspace: PathBuf, session: Option<String>, callback: impl Fn(Update) + Send + Sync + 'static) -> Handle {
    let (tx, rx) = mpsc::channel();
    let handle = Handle { tx: tx.clone() };
    let mut inspecting = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (presentation, updates) = mpsc::channel::<Update>();
    std::thread::Builder::new()
        .name("coder-rich-text".into())
        .spawn(move || {
            let mut view = View::default();
            let mut renderer = olo_ui::rich_text::Renderer::default();
            let mut previous = String::new();
            while let Ok(first) = updates.recv() {
                for update in std::iter::once(first).chain(updates.try_iter().take(256)) {
                    view.apply(update.clone());
                    callback(update);
                }
                let source = view.markdown();
                if source != previous {
                    let rendered = renderer.render(&source);
                    previous = source;
                    callback(Update::Rendered(Arc::new(rendered)));
                }
            }
        })
        .expect("start presentation worker");
    let callback: Arc<dyn Fn(Update) + Send + Sync> = Arc::new(move |u| {
        let _ = presentation.send(u);
    });
    std::thread::Builder::new()
        .name("coder-client".into())
        .spawn(move || {
            let result = (|| -> Result<()> {
                callback(Update::Connection(Some("Opening your workspace…".into())));
                let mut wanted = session;
                let mut workspace = workspace;
                let mut last_error = String::new();
                let mut attempt = 0u32;
                let mut s: Session = loop {
                    let opened = (|| -> Result<Session> {
                        super::ensure_service(&dir)?;
                        Ok(serde_json::from_value(rpc::call(
                            &dir,
                            &match &wanted {
                                Some(session) => Command::Get { session: session.clone() },
                                None => Command::Create { workspace: workspace.clone(), config: None },
                            },
                        )?)?)
                    })();
                    match opened {
                        Ok(s) => break s,
                        Err(error) => {
                            let error = format!("{error:#}");
                            if error != last_error {
                                callback(Update::Connection(Some(format!("Waiting for the session: {error}. Retrying automatically; /open ID or /new can choose another session."))));
                                last_error = error.clone();
                            }
                            attempt = attempt.saturating_add(1);
                            match rx.recv_timeout(std::time::Duration::from_secs(1u64 << attempt.min(3))) {
                                Ok(Action::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                                Ok(Action::Submit(text) | Action::Steer(text, _)) => {
                                    if let Some(id) = text.trim().strip_prefix("/open ") {
                                        wanted = Some(id.trim().into());
                                    } else if text.trim() == "/new" {
                                        wanted = None;
                                    } else if let Some(path) = text.trim().strip_prefix("/new ") {
                                        workspace = PathBuf::from(path.trim());
                                        wanted = None;
                                    } else {
                                        callback(Update::InputResult { session: wanted.clone().unwrap_or_default(), text, error: Some(error) });
                                    }
                                }
                                Ok(Action::Browse(filter)) => {
                                    if let Ok(catalog) = rpc::call(&dir, &Command::Browse { filter }) {
                                        if let Some(rows) = catalog["sessions"].as_array() {
                                            callback(Update::Catalog { rows: rows.clone(), next_offset: catalog["next_offset"].as_u64().map(|n| n as usize) });
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                };
                callback(Update::Opened(s.id.clone()));
                let _ = tx.send(Action::Repository);
                let _ = tx.send(Action::Models);
                callback(Update::Session(Box::new(s.clone())));
                let mut following = follow(&dir, &s.id, Default::default(), callback.clone(), tx.clone())?;
                let mut catalog = follow(&dir, "", Default::default(), callback.clone(), tx.clone())?;
                while let Ok(action) = rx.recv() {
                    let mut submitted = match &action {
                        Action::Submit(text) | Action::Steer(text, _) => Some((s.id.clone(), text.clone())),
                        _ => None,
                    };
                    let result = (|| -> Result<()> {
                        let input = match action {
                            Action::Quit => return Err(anyhow::anyhow!("__quit")),
                            Action::Models => {
                                let dir = dir.clone();
                                let provider = s.config.model.provider;
                                let callback = callback.clone();
                                std::thread::spawn(move || {
                                    if let Ok(value) = rpc::call(&dir, &Command::Models { provider }) {
                                        let entries = value["data"].as_array().or_else(|| value["models"].as_array()).or_else(|| value.as_array());
                                        let ids =
                                            entries.map(|rows| rows.iter().filter_map(|r| r["id"].as_str().or(r["slug"].as_str()).or(r.as_str())).map(str::to_owned).collect()).unwrap_or_default();
                                        callback(Update::Models { provider, ids });
                                    }
                                });
                                return Ok(());
                            }
                            Action::Repository => {
                                if !inspecting.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                    let inspecting = inspecting.clone();
                                    let dir = dir.clone();
                                    let session = s.id.clone();
                                    let callback = callback.clone();
                                    std::thread::spawn(move || {
                                        let result = rpc::call(&dir, &Command::Repository { session: session.clone(), arguments: json!({"operation":"status"}) })
                                            .unwrap_or_else(|e| json!({"error":e.to_string()}));
                                        callback(Update::Repository { session, result });
                                        inspecting.store(false, std::sync::atomic::Ordering::Relaxed);
                                    });
                                }
                                return Ok(());
                            }
                            Action::Refresh => {
                                s = serde_json::from_value(rpc::call(&dir, &Command::Get { session: s.id.clone() })?)?;
                                callback(Update::Session(Box::new(s.clone())));
                                return Ok(());
                            }
                            Action::Browse(filter) => {
                                catalog = follow(&dir, "", filter, callback.clone(), tx.clone())?;
                                return Ok(());
                            }
                            Action::Steer(text, delivery) => {
                                if text.starts_with('/') {
                                    commands::parse(&text, &s)?
                                } else {
                                    Input::Command(Command::Steer { session: s.id.clone(), text, delivery, message_id: None })
                                }
                            }
                            Action::Stop => Input::Command(Command::Stop { session: s.id.clone() }),
                            Action::Approve(allow) => Input::Command(Command::Approve { session: s.id.clone(), call_id: s.pending.as_ref().context("no pending approval")?.call.id.clone(), allow }),
                            Action::Submit(text) => commands::parse(&text, &s)?,
                        };
                        match input {
                            Input::Help => callback(Update::Notice(commands::HELP.into())),
                            Input::Quit => return Err(anyhow::anyhow!("__quit")),
                            Input::Config => callback(Update::Notice(serde_json::to_string_pretty(&s.config)?)),
                            Input::Open(id) => {
                                inspecting = Arc::new(std::sync::atomic::AtomicBool::new(false));
                                s = serde_json::from_value(rpc::call(&dir, &Command::Get { session: id })?)?;
                                callback(Update::Opened(s.id.clone()));
                                following = follow(&dir, &s.id, Default::default(), callback.clone(), tx.clone())?;
                                let _ = tx.send(Action::Repository);
                                let _ = tx.send(Action::Models);
                                callback(Update::Session(Box::new(s.clone())));
                            }
                            Input::New(path) => {
                                inspecting = Arc::new(std::sync::atomic::AtomicBool::new(false));
                                let workspace = if path.is_empty() { s.workspace.clone() } else { PathBuf::from(path) };
                                s = serde_json::from_value(rpc::call(&dir, &Command::Create { workspace, config: None })?)?;
                                callback(Update::Opened(s.id.clone()));
                                following = follow(&dir, &s.id, Default::default(), callback.clone(), tx.clone())?;
                                let _ = tx.send(Action::Repository);
                                let _ = tx.send(Action::Models);
                                callback(Update::Session(Box::new(s.clone())));
                            }
                            Input::Command(cmd) => {
                                let previous = s.config.model.provider;
                                let quiet = matches!(
                                    &cmd,
                                    Command::Send { .. }
                                        | Command::Steer { .. }
                                        | Command::Organize { .. }
                                        | Command::CancelMessage { .. }
                                        | Command::EditMessage { .. }
                                        | Command::MoveMessage { .. }
                                        | Command::Tool { .. }
                                        | Command::Stop { .. }
                                        | Command::Resume { .. }
                                        | Command::Configure { .. }
                                        | Command::Approve { .. }
                                );
                                let value = rpc::call(&dir, &cmd)?;
                                // The command was accepted. A later refresh failure
                                // must never restore an already submitted message.
                                if let Some((session, text)) = submitted.take() {
                                    callback(Update::InputResult { session, text, error: None });
                                }
                                if !quiet {
                                    callback(Update::Notice(if let Some(diff) = value["diff"].as_str() { fenced("diff", diff) } else { fenced("json", &serde_json::to_string_pretty(&value)?) }));
                                }
                                s = serde_json::from_value(rpc::call(&dir, &Command::Get { session: s.id.clone() })?)?;
                                if s.config.model.provider != previous {
                                    let _ = tx.send(Action::Models);
                                }
                                callback(Update::Session(Box::new(s.clone())));
                            }
                        }
                        Ok(())
                    })();
                    if let Some((session, text)) = submitted {
                        callback(Update::InputResult { session, text, error: result.as_ref().err().map(|e| format!("{e:#}")) });
                    }
                    if let Err(e) = result {
                        if e.to_string() == "__quit" {
                            break;
                        }
                        callback(Update::Notice(format!("Error: {e:#}")));
                    }
                }
                drop(following);
                drop(catalog);
                Ok(())
            })();
            if let Err(e) = result {
                callback(Update::Notice(format!("Connection error: {e:#}")));
            }
        })
        .expect("start client worker");
    handle
}
#[derive(Default)]
pub struct View {
    pub session: Option<Session>,
    pub catalog: Arc<Vec<Value>>,
    pub next_offset: Option<usize>,
    pub rich: Arc<olo_ui::rich_text::Text>,
    pub repository: Option<Value>,
    pub models: Vec<String>,
    pub lines: std::collections::VecDeque<String>,
    pub live: String,
    pub live_code: bool,
    pub notice: String,
    pub connection: Option<String>,
    pub seq: i64,
    pub id: String,
}
impl View {
    pub fn push(&mut self, text: &str) {
        for line in text.lines() {
            self.lines.push_back(olo_coder_core::tools::clip(line, 12000));
        }
        while self.lines.len() > 2000 || self.lines.iter().map(String::len).sum::<usize>() > 256000 {
            self.lines.pop_front();
        }
    }
    pub fn apply(&mut self, u: Update) {
        match u {
            Update::Catalog { rows, next_offset } => {
                self.catalog = Arc::new(rows);
                self.next_offset = next_offset;
            }
            Update::InputResult { .. } => {}
            Update::Connection(status) => self.connection = status,
            Update::Rendered(doc) => self.rich = doc,
            Update::Models { provider, ids } => {
                if self.session.as_ref().is_some_and(|s| s.config.model.provider == provider) {
                    self.models = ids;
                }
            }
            Update::Repository { session, result } => {
                if session == self.id {
                    self.repository = Some(result);
                }
            }
            Update::Opened(id) => {
                self.id = id;
                self.seq = 0;
                self.lines.clear();
                self.live.clear();
                self.notice.clear();
                self.connection = None;
                self.rich = Arc::default();
                self.repository = None;
                self.push("# Make something remarkable\n\nA workspace for ideas, code, and the work between them.\n\n**Build** — describe what you want to create.\n\n**Explore** — ask about your codebase or review a change.\n\n**Plan together** — start with `/spec on` and shape a detailed specification.\n\nSet a `/goal` for ongoing work. Your session, decisions, and checkpoints stay with you.\n\n---\n");
            }
            Update::Session(s) => {
                if self.session.as_ref().is_some_and(|old| old.config.model.provider != s.config.model.provider) {
                    self.models.clear();
                }
                self.session = Some(*s);
            }
            Update::Notice(t) => {
                self.notice = t.clone();
                self.push(&format!("\n{t}\n"));
            }
            Update::Event(e) => {
                if e.session != self.id || (e.seq > 0 && e.seq <= self.seq) {
                    return;
                }
                self.seq = self.seq.max(e.seq);
                match e.kind.as_str() {
                    "delta" => {
                        self.live_code = false;
                        self.live.push_str(e.data["text"].as_str().unwrap_or(""));
                        if self.live.len() > 64000 {
                            self.live = olo_coder_core::tools::clip(&self.live, 64000);
                        }
                    }
                    "message_queued" => {
                        let message = &e.data["message"];
                        let from = message["from"]["title"].as_str().unwrap_or("You");
                        self.push(&format!("\n### {} · {}\n\n{}\n\n", from, message["delivery"].as_str().unwrap_or("queued"), message["text"].as_str().unwrap_or("")));
                    }
                    "messages_delivered" => {
                        self.live.clear();
                        let skipped = e.data["skipped_calls"].as_array().map_or(0, Vec::len);
                        if skipped > 0 {
                            self.push(&format!("\n> Steering applied · {skipped} earlier actions skipped before execution.\n"));
                        }
                    }
                    "user" => {
                        self.live.clear();
                        self.push(&format!("\n### You\n\n{}\n", e.data["text"].as_str().unwrap_or("")));
                    }
                    "assistant" => {
                        self.live.clear();
                        if let Some(text) = e.data["text"].as_str().filter(|t| !t.trim().is_empty()) {
                            self.push(&format!("\n### Coder\n\n{text}\n"));
                        }
                    }
                    "tool_started" => {
                        let call = &e.data["call"];
                        let name = call["name"].as_str().unwrap_or("tool");
                        if !olo_coder_core::tools::internal(name) {
                            let args = &call["arguments"];
                            let detail = if name == "execute" {
                                format!(
                                    "{} {}",
                                    args["program"].as_str().unwrap_or(""),
                                    args["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default()
                                )
                            } else {
                                args["path"].as_str().or(args["pattern"].as_str()).or(args["operation"].as_str()).unwrap_or("").to_string()
                            };
                            self.push(&format!("\n#### {}\n\n{}\n", name.replace('_', " "), if detail.is_empty() { String::new() } else { format!("`{}`", detail.replace('`', "'")) }));
                        }
                    }
                    "tool_result" => {
                        self.live.clear();
                        let output = &e.data["output"];
                        if output.get("files").is_some() && output.get("branch").is_some() {
                            self.repository = Some(output.clone());
                        }
                        let name = e.data["name"].as_str().unwrap_or("");
                        if e.data["error"] == true {
                            self.push(&format!("\n> **Action failed** — {}\n", output["error"].as_str().unwrap_or("See action details.")));
                        } else if name == "set_todos" {
                            self.push(&todo_component("Plan", &output["todos"]));
                        } else if name == "complete_goal" {
                            self.push(&format!("\n> **Goal completed**\n> {}\n", output["goal"]["evidence"].as_str().unwrap_or("Verified.")));
                        } else if name == "remember" {
                            self.push("\n> Session memory updated.\n");
                        } else if name == "update_spec" {
                            self.push(&format!("\n## Specification\n\n{}\n", output["spec"]["text"].as_str().or(output["text"].as_str()).unwrap_or("Specification updated.")));
                        } else if name == "update_criterion" {
                            self.push("\n> Acceptance criterion updated.\n");
                        } else if name == "list_files" {
                            if let Some(files) = output["results"].as_array() {
                                self.push(&fenced("", &files.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")));
                            }
                        } else if output.get("component").is_some() {
                            self.push(&component(&output["component"]));
                        } else if let Some(diff) = output["diff"].as_str() {
                            self.push(&fenced("diff", diff));
                        } else if let Some(text) = output["text"].as_str() {
                            let language = output["path"].as_str().and_then(|p| olo_ui::code::detect(std::path::Path::new(p), text)).map(|l| l.id).unwrap_or("");
                            let source = if output.get("total_lines").is_some() {
                                text.lines().map(|l| l.split_once(": ").filter(|(n, _)| n.bytes().all(|b| b.is_ascii_digit())).map_or(l, |(_, line)| line)).collect::<Vec<_>>().join("\n")
                            } else {
                                text.into()
                            };
                            self.push(&fenced(language, &source));
                        } else if let Some(text) = output["stdout"].as_str() {
                            self.push(&fenced("", text));
                            if let Some(stderr) = output["stderr"].as_str().filter(|s| !s.is_empty()) {
                                self.push(&fenced("", stderr));
                            }
                            self.push(&format!("**{}** · exit {}", if output["success"] == true { "Succeeded" } else { "Failed" }, output["exit_code"]));
                        } else if let Some(content) = output["content"].as_array() {
                            for block in content {
                                if let Some(text) = block["text"].as_str() {
                                    self.push(text);
                                }
                            }
                        } else {
                            self.push(&fenced("json", &olo_coder_core::tools::clip(&serde_json::to_string_pretty(output).unwrap_or_default(), 12000)));
                        }
                        self.push(&format!("\n[Action details · {}](olo-coder:event/{})\n", e.seq, e.seq));
                    }
                    "tool_output" => {
                        self.live_code = true;
                        self.live.push_str(e.data["text"].as_str().unwrap_or(""));
                        if self.live.len() > 16000 {
                            let keep = self.live.char_indices().rev().nth(8000).map_or(0, |(n, _)| n);
                            self.live = self.live[keep..].into();
                        }
                    }
                    "approval" => self.push(&format!("\n> **Approval needed** — {}\n> {}\n", e.data["call"]["name"].as_str().unwrap_or("Action"), e.data["reason"].as_str().unwrap_or(""))),
                    "error" | "paused" | "retry" | "compacted" | "auto_review" | "interrupted" => self.push(&format!("{} · {}", e.kind, e.data)),
                    _ => {}
                }
            }
        }
    }
    pub fn markdown(&self) -> String {
        let mut text = self.lines.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
        if !self.live.is_empty() {
            text.push_str("\n\n");
            if self.live_code {
                text.push_str(&fenced("", &self.live));
            } else {
                text.push_str(&self.live);
            }
        }
        if let Some(status) = &self.connection {
            text.push_str(&format!("\n\n> {status}\n"));
        }
        text
    }
    pub fn status(&self) -> String {
        self.session.as_ref().map_or_else(
            || "Connecting…".into(),
            |s| {
                format!(
                    "{:?} · {} · {} · {} tokens · {} compactions{}",
                    s.status,
                    s.config.model.model,
                    s.config.model.effort,
                    s.usage.input + s.usage.output,
                    s.compactions,
                    if s.spec_mode { " · Spec mode" } else { "" }
                )
            },
        )
    }
    pub fn work(&self) -> String {
        let Some(s) = &self.session else { return "Connecting…".into() };
        let mut t = format!("Workspace\n{}\n\nSession\n{}\n\n", s.workspace.display(), s.id);
        if let Some(repo) = &self.repository {
            if let Some(branch) = repo["branch"].as_str() {
                t += &format!("Branch · {branch}\n{} changed entries\n\n", repo["files"].as_array().map_or(0, Vec::len));
            }
        }
        if let Some(g) = &s.goal {
            t += &format!("Goal · {}\n{}\n{}\n\n", g.status, g.objective, g.evidence);
        }
        t += "Todos\n";
        for todo in &s.todos {
            t += &format!("[{}] {} · {}\n{}\n", todo.status, todo.id, todo.text, todo.evidence);
        }
        if !s.spec.text.is_empty() {
            t += &format!(
                "\nSpecification · revision {}{}\n{}\n",
                s.spec.revision,
                if s.spec.accepted_revision == Some(s.spec.revision) { " · accepted" } else { " · awaiting acceptance" },
                s.spec.text
            );
            for c in &s.spec.criteria {
                t += &format!("[{}] {} · {}\n{}\n", c.status, c.id, c.text, c.evidence);
            }
        }
        if let Some(a) = &s.pending {
            t += &format!("\nApproval\n{}\n{}\n{}\n", a.reason, a.call.name, serde_json::to_string_pretty(&a.call.arguments).unwrap_or_default());
        }
        t += &format!(
            "\nMemory\n{}\n\nApproval: {:?}\n{}\n\nInput: {} · Output: {} · Cached: {}\nActive context: {} tokens\n",
            s.memory, s.config.approval, s.config.approval_guidance, s.usage.input, s.usage.output, s.usage.cached, s.usage.context
        );
        t
    }
}

fn fenced(language: &str, text: &str) -> String {
    let width = text.lines().map(|l| l.trim_start().chars().take_while(|c| *c == '`').count() + 1).max().unwrap_or(3).max(3);
    let language: String = language.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '#' | '.')).take(64).collect();
    let fence = "`".repeat(width);
    format!("\n{fence}{language}\n{text}\n{fence}\n")
}

fn component(v: &Value) -> String {
    let cell = |v: &Value| {
        let text = match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => v.to_string(),
        };
        text.replace('\n', " ").replace('|', "\\|")
    };
    let mut out = format!("\n## {}\n\n", v["title"].as_str().unwrap_or("Result"));
    match v["kind"].as_str().unwrap_or("") {
        "table" => {
            if let Some(columns) = v["columns"].as_array() {
                out += &format!("| {} |\n| {} |\n", columns.iter().map(cell).collect::<Vec<_>>().join(" | "), vec!["---"; columns.len()].join(" | "));
            }
            if let Some(rows) = v["rows"].as_array() {
                for row in rows {
                    if let Some(cells) = row.as_array() {
                        out += &format!("| {} |\n", cells.iter().map(cell).collect::<Vec<_>>().join(" | "));
                    }
                }
            }
        }
        "checklist" => {
            if let Some(items) = v["items"].as_array() {
                for item in items {
                    out += &format!("- [{}] {}\n", if item["done"] == true { "x" } else { " " }, cell(&item["text"]));
                }
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

fn todo_component(title: &str, todos: &Value) -> String {
    let items = todos.as_array().map(|ts| ts.iter().map(|t| json!({"text":t["text"],"done":t["status"]=="done"})).collect::<Vec<_>>()).unwrap_or_default();
    component(&json!({"kind":"checklist","title":title,"items":items}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_status_is_transient_and_preserves_transcript() {
        let mut view = View::default();
        view.push("A durable conversation");
        let before = view.markdown();
        for _ in 0..100 {
            view.apply(Update::Connection(Some("Reconnecting…".into())));
        }
        assert_eq!(view.markdown().matches("Reconnecting…").count(), 1);
        assert_eq!(view.lines.len(), 1);
        view.apply(Update::Connection(None));
        assert_eq!(view.markdown(), before);
    }
    #[test]
    fn rich_components_preserve_numeric_cells_and_contain_fences() {
        let table = component(&json!({"kind":"table","title":"Results","columns":["Metric","Value"],"rows":[["Tests",27],["Successful",true]]}));
        assert!(table.contains("| Tests | 27 |"));
        assert!(table.contains("| Successful | true |"));
        let code = fenced("rust\n```", "fn main() {}\n   ```\nnot Markdown");
        assert!(code.starts_with("\n````rust\n"));
        let mut renderer = olo_ui::rich_text::Renderer::default();
        let doc = renderer.render(&code);
        assert!(doc.text.contains("   ```\nnot Markdown"));
    }
}
