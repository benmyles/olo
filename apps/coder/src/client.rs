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
    Models { session: String, model: Box<olo_coder_core::ModelConfig>, ids: Vec<String> },
    Event(Event),
    Notice(String),
    Connection(Option<String>),
    Opened(String),
    Rendered(Arc<olo_ui::rich_text::Text>),
}
pub enum Action {
    Submit(String),
    Steer(String, Delivery),
    Prompt { text: String, delivery: Option<Delivery>, session: String, generation: u64 },
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
    pub tx: ActionSender,
}
#[derive(Clone)]
pub struct ActionSender {
    queue: mpsc::Sender<Action>,
    control: Option<Arc<StopControl>>,
}
struct StopControl {
    dir: PathBuf,
    session: Arc<olo_sync::Mutex<String>>,
    pending: olo_sync::Mutex<std::collections::HashSet<String>>,
    callback: Arc<dyn Fn(Update) + Send + Sync>,
    generation: std::sync::atomic::AtomicU64,
}
impl ActionSender {
    pub fn send(&self, action: Action) -> Result<(), mpsc::SendError<Action>> {
        let stop = matches!(&action, Action::Stop) || matches!(&action, Action::Submit(text) | Action::Steer(text, _) if text.trim() == "/stop");
        if stop {
            if let Some(control) = &self.control {
                let session = control.session.lock().unwrap().clone();
                if !session.is_empty() {
                    control.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if !control.pending.lock().unwrap().insert(session.clone()) {
                        return Ok(());
                    }
                    let control = control.clone();
                    let submitted = match &action {
                        Action::Submit(text) | Action::Steer(text, _) => Some(text.clone()),
                        _ => None,
                    };
                    let worker = control.clone();
                    let target = session.clone();
                    let spawned = std::thread::Builder::new().name("coder-stop".into()).spawn(move || {
                        let result = rpc::call(&worker.dir, &Command::Stop { session: target.clone() });
                        if let Some(text) = submitted {
                            (worker.callback)(Update::InputResult { session: target.clone(), text, error: result.as_ref().err().map(|e| format!("{e:#}")) });
                        }
                        if let Err(error) = result {
                            (worker.callback)(Update::Notice(format!("Stop request needs attention for session {target}: {error:#}")));
                        }
                        worker.pending.lock().unwrap().remove(&target);
                    });
                    if let Err(error) = spawned {
                        control.pending.lock().unwrap().remove(&session);
                        (control.callback)(Update::Notice(format!("Could not start cancellation worker: {error}")));
                        return Err(mpsc::SendError(action));
                    }
                    return Ok(());
                }
            }
        }
        let action = match (action, &self.control) {
            (Action::Submit(text), Some(control)) if prompt_input(&text) => control.prompt(text, None),
            (Action::Steer(text, delivery), Some(control)) if prompt_input(&text) => control.prompt(text, Some(delivery)),
            (action, _) => action,
        };
        self.queue.send(action)
    }
}
fn prompt_input(text: &str) -> bool {
    !text.starts_with('/') || ["/queue ", "/after-tool ", "/interrupt "].iter().any(|prefix| text.starts_with(prefix))
}
impl StopControl {
    fn prompt(&self, text: String, delivery: Option<Delivery>) -> Action {
        Action::Prompt { text, delivery, session: self.session.lock().unwrap().clone(), generation: self.generation.load(std::sync::atomic::Ordering::SeqCst) }
    }
    fn admit(&self, action: Action, current: &str) -> Option<Action> {
        match action {
            Action::Prompt { text, delivery, session, generation } => {
                if generation != self.generation.load(std::sync::atomic::Ordering::SeqCst) || (!session.is_empty() && session != current) {
                    (self.callback)(Update::InputResult { session, text, error: Some("Message was held before submission because work was stopped or the attached session changed.".into()) });
                    None
                } else {
                    Some(match delivery {
                        Some(delivery) => Action::Steer(text, delivery),
                        None => Action::Submit(text),
                    })
                }
            }
            action => Some(action),
        }
    }
}
impl From<mpsc::Sender<Action>> for ActionSender {
    fn from(queue: mpsc::Sender<Action>) -> Self {
        Self { queue, control: None }
    }
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
                                    | "compaction_applied"
                                    | "compaction_restored"
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
    let current = Arc::new(olo_sync::Mutex::new(session.clone().unwrap_or_default()));
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
    let attachment = current.clone();
    let callback: Arc<dyn Fn(Update) + Send + Sync> = Arc::new(move |u| {
        if let Update::Opened(session) = &u {
            *attachment.lock().unwrap() = session.clone();
        }
        let _ = presentation.send(u);
    });
    let control = Arc::new(StopControl { dir: dir.clone(), session: current, pending: Default::default(), callback: callback.clone(), generation: Default::default() });
    let handle = Handle { tx: ActionSender { queue: tx.clone(), control: Some(control.clone()) } };
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
                                Ok(Action::Prompt { text, session, .. }) => {
                                    callback(Update::InputResult { session, text, error: Some(error) });
                                }
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
                    let Some(action) = control.admit(action, &s.id) else { continue };
                    let mut submitted = match &action {
                        Action::Submit(text) | Action::Steer(text, _) => Some((s.id.clone(), text.clone())),
                        _ => None,
                    };
                    let result = (|| -> Result<()> {
                        let input = match action {
                            Action::Prompt { .. } => unreachable!("prompt admission unwraps queued input"),
                            Action::Quit => return Err(anyhow::anyhow!("__quit")),
                            Action::Models => {
                                let dir = dir.clone();
                                let provider = s.config.model.provider;
                                let session = s.id.clone();
                                let model = Box::new(s.config.model.clone());
                                let callback = callback.clone();
                                std::thread::spawn(move || {
                                    if let Ok(value) = rpc::call(&dir, &Command::Models { provider, session: Some(session.clone()) }) {
                                        let entries = value["data"].as_array().or_else(|| value["models"].as_array()).or_else(|| value.as_array());
                                        let ids =
                                            entries.map(|rows| rows.iter().filter_map(|r| r["id"].as_str().or(r["slug"].as_str()).or(r.as_str())).map(str::to_owned).collect()).unwrap_or_default();
                                        callback(Update::Models { session, model, ids });
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
                                let previous = s.config.model.clone();
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
                                    callback(Update::Notice(command_result(&cmd, &value)?));
                                }
                                s = serde_json::from_value(rpc::call(&dir, &Command::Get { session: s.id.clone() })?)?;
                                if s.config.model != previous {
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
    pub compaction_progress: Option<String>,
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
            Update::Models { session, model, ids } => {
                if self.session.as_ref().is_some_and(|s| s.id == session && s.config.model == *model) {
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
                self.compaction_progress = None;
                self.connection = None;
                self.rich = Arc::default();
                self.repository = None;
                self.push("# Make something remarkable\n\nA workspace for ideas, code, and the work between them.\n\n**Build** — describe what you want to create.\n\n**Explore** — ask about your codebase or review a change.\n\n**Plan together** — start with `/spec on` and shape a detailed specification.\n\nSet a `/goal` for ongoing work. Your session, decisions, and checkpoints stay with you.\n\n---\n");
            }
            Update::Session(s) => {
                if self.session.as_ref().is_some_and(|old| old.id != s.id || old.config.model != s.config.model) {
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
                if matches!(e.kind.as_str(), "idle" | "error" | "paused" | "interrupted" | "compaction_applied" | "compaction_deferred") {
                    self.compaction_progress = None;
                }
                match e.kind.as_str() {
                    "compaction_progress" => self.compaction_progress = Some(format!("Compacting {}/{}", e.data["completed"], e.data["total"])),
                    "compaction_ready" => self.compaction_progress = Some("Checkpoint ready".into()),
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
                            let reason = output["error"].as_str().unwrap_or("See action details.");
                            if reason.starts_with("cancelled") {
                                let details =
                                    if reason.contains("partial output artifact ") { "Partial output is available in the action details." } else { "See action details for the recorded outcome." };
                                self.push(&format!("\n> **Action stopped** — Review any changes before retrying. {details}\n"));
                            } else {
                                self.push(&format!("\n> **Action failed** — {reason}\n"));
                            }
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
                    "error" | "paused" | "interrupted" => {
                        self.live.clear();
                        let reason = e.data["message"].as_str().or(e.data["reason"].as_str()).unwrap_or("See session history for details.");
                        let label = match e.kind.as_str() {
                            "paused" => "Paused",
                            "interrupted" => "Session interrupted",
                            _ => "Session needs attention",
                        };
                        let reason = if reason == "cancelled" { "Work has stopped. Pending messages stay available for Resume." } else { reason };
                        self.push(&format!("\n\n> **{label}** — {reason}\n\n"));
                    }
                    "retry" => {
                        let delay = e.data["seconds"].as_u64().map(|n| format!(" in {n} seconds")).unwrap_or_default();
                        let reason = e.data["reason"].as_str().map(|s| format!(" · {s}")).unwrap_or_default();
                        self.push(&format!("\n\n> **Reconnecting to the model**{delay} · attempt {}{reason}\n\n", e.data["attempt"]));
                    }
                    "compaction_applied" => self.push(&format!("\n\n> **Context checkpoint applied** — {} tokens in the working set. Original transcript preserved.\n\n[Inspect checkpoint](olo-coder:compaction/{}) · [Original transcript](olo-coder:transcript)\n\n", e.data["context_tokens"], e.data["checkpoint_id"].as_str().unwrap_or(""))),
                    "compaction_restored" => self.push("\n\n> **Context restored** — Original messages are back in the working set.\n\n"),
                    "compaction_deferred" => self.push(&format!("\n\n> **Compaction deferred** — {}\n\n", e.data["reason"].as_str().unwrap_or("Source context changed"))),
                    "compacted" => self.push("\n\n> **Context compacted** — Provider context was saved. The full session history remains available.\n\n"),
                    "auto_review" => {
                        let decision = if e.data["allow"] == true { "Action approved" } else { "Approval needed" };
                        self.push(&format!("\n\n> **{decision}** — {}\n\n", e.data["reason"].as_str().unwrap_or("Automatic review finished.")));
                    }
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
                    "{:?} · {}{} · {} tokens · {} compactions{}{}",
                    s.status,
                    s.config.model.model,
                    if s.config.model.effort.is_empty() { String::new() } else { format!(" · {}", s.config.model.effort) },
                    s.usage.input + s.usage.output,
                    s.compactions,
                    if s.spec_mode { " · Spec mode" } else { "" },
                    self.compaction_progress.as_ref().map(|v| format!(" · {v}")).unwrap_or_default()
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

/// Internal transcript links only inspect data; untrusted model text cannot
/// turn a link click into an approval, restore, write or model invocation.
pub fn read_link(link: &str) -> Option<String> {
    if link == "olo-coder:transcript" {
        return Some("/transcript".into());
    }
    for (prefix, command) in [("olo-coder:event/", "event"), ("olo-coder:transcript/", "transcript")] {
        if let Some(id) = link.strip_prefix(prefix).and_then(|v| v.parse::<i64>().ok()).filter(|id| *id > 0) {
            return Some(format!("/{command} {id}"));
        }
    }
    if let Some(id) = link.strip_prefix("olo-coder:compaction/").filter(|v| v.len() == 36 && v.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')) {
        return Some(format!("/compactions {id}"));
    }
    None
}
fn command_result(command: &Command, value: &Value) -> Result<String> {
    let mut links = String::new();
    match command {
        Command::Compactions { .. } => {
            if let Some(rows) = value["checkpoints"].as_array() {
                links.push_str("### Compaction checkpoints\n\n| Checkpoint | State | Sources | Summary tokens |\n| --- | --- | ---: | ---: |\n");
                for row in rows {
                    let id = row["id"].as_str().unwrap_or("");
                    links.push_str(&format!(
                        "| [{}](olo-coder:compaction/{id}) | {} | {} | {} |\n",
                        id.chars().take(8).collect::<String>(),
                        row["status"].as_str().unwrap_or(""),
                        row["source_count"],
                        row["summary_tokens"]
                    ));
                }
            }
            if let Some(id) = value["parent"].as_str() {
                links.push_str(&format!("\n[Previous checkpoint](olo-coder:compaction/{id})\n"));
            }
            for id in value["prefix_refs"].as_array().into_iter().flatten().filter_map(Value::as_i64).take(100) {
                links.push_str(&format!("[Source {id}](olo-coder:transcript/{id}) · "));
            }
        }
        Command::Transcript { .. } => {
            for row in value["records"].as_array().into_iter().flatten() {
                if let Some(id) = row["id"].as_i64() {
                    links.push_str(&format!("[Record {id}](olo-coder:transcript/{id}) · "));
                }
            }
        }
        Command::TranscriptSearch { .. } => {
            for row in value.as_array().into_iter().flatten() {
                if let Some(id) = row["id"].as_i64() {
                    links.push_str(&format!("[Match {id}](olo-coder:transcript/{id}) · "));
                }
            }
        }
        _ => {}
    }
    links.push_str(&if let Some(diff) = value["diff"].as_str() { fenced("diff", diff) } else { fenced("json", &serde_json::to_string_pretty(value)?) });
    Ok(links)
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
    fn catalogs_are_bound_to_the_session_and_endpoint_that_requested_them() {
        let mut session = Session::new("/var/tmp".into(), Config { model: ModelConfig::for_provider(Provider::Openai), ..Config::default() });
        let old = session.config.model.clone();
        let mut view = View { id: session.id.clone(), session: Some(session.clone()), models: vec!["old-model".into()], ..Default::default() };
        session.config.model.endpoint.as_mut().unwrap().base_url = "http://127.0.0.1:9999/v1".into();
        view.apply(Update::Session(Box::new(session.clone())));
        assert!(view.models.is_empty());
        view.apply(Update::Models { session: session.id.clone(), model: Box::new(old), ids: vec!["stale".into()] });
        assert!(view.models.is_empty());
        view.apply(Update::Models { session: session.id, model: Box::new(session.config.model), ids: vec!["fresh".into()] });
        assert_eq!(view.models, vec!["fresh"]);
    }
    #[test]
    fn source_links_cannot_perform_writes_or_accept_embedded_commands() {
        assert_eq!(read_link("olo-coder:transcript/47"), Some("/transcript 47".into()));
        assert_eq!(read_link("olo-coder:compaction/11111111-1111-1111-1111-111111111111"), Some("/compactions 11111111-1111-1111-1111-111111111111".into()));
        for link in ["olo-coder:restore-context/47", "olo-coder:event/-1", "olo-coder:event/1\n/approve", "olo-coder:compaction/../../", "olo-coder:api/{}"] {
            assert!(read_link(link).is_none());
        }
    }
    #[test]
    fn stopped_actions_and_session_status_are_readable_separate_blocks() {
        let mut view = View { id: "session".into(), ..Default::default() };
        for (seq, kind, data) in [
            (1, "tool_result", json!({"name":"execute","error":true,"output":{"error":"cancelled; process group terminated; partial output artifact opaque-id"}})),
            (2, "paused", json!({"message":"cancelled"})),
            (3, "paused", json!({"message":"Need the missing fixture"})),
            (4, "compacted", json!({"compactions":2})),
        ] {
            view.apply(Update::Event(Event { seq, session: "session".into(), time: 0, kind: kind.into(), data }));
        }
        let source = view.markdown();
        assert!(source.contains("**Action stopped**"));
        assert!(source.contains("olo-coder:event/1)\n\n\n> **Paused**"));
        assert!(source.contains("Need the missing fixture"));
        assert!(source.contains("**Context compacted**"));
        assert!(!source.contains("opaque-id") && !source.contains("{\"message\""));
    }
    #[test]
    fn stop_bypasses_busy_requests_and_holds_older_prompts() {
        use std::time::{Duration, Instant};
        let dir = std::path::PathBuf::from("/var/tmp").join(format!("olo-coder-control-{}", olo_coder_core::id()));
        std::fs::create_dir(&dir).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(rpc::socket(&dir)).unwrap();
        let (accepted, acceptance) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(socket.try_clone().unwrap()).read_line(&mut line).unwrap();
            let value: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["command"], json!({"op":"stop","session":"original"}));
            writeln!(socket, "{}", json!({"result":{"stopped":true}})).unwrap();
            accepted.send(()).unwrap();
        });
        let (updates, observed) = mpsc::channel();
        let control = Arc::new(StopControl {
            dir: dir.clone(),
            session: Arc::new(olo_sync::Mutex::new("original".into())),
            pending: Default::default(),
            generation: Default::default(),
            callback: Arc::new(move |u| {
                let _ = updates.send(u);
            }),
        });
        // No consumer on the normal queue: it represents a request waiting on
        // an unresponsive peer. Stop must take another connection immediately.
        let (queue, waiting) = mpsc::channel();
        let sender = ActionSender { queue, control: Some(control.clone()) };
        assert!(sender.send(Action::Steer("Older unsent prompt".into(), Delivery::Queue)).is_ok());
        let began = Instant::now();
        assert!(sender.send(Action::Submit("/stop".into())).is_ok());
        acceptance.recv_timeout(Duration::from_secs(1)).unwrap();
        println!("Stop bypassed stalled request in {:?}", began.elapsed());
        assert!(control.admit(waiting.recv().unwrap(), "original").is_none());
        let mut held = false;
        for _ in 0..2 {
            if let Update::InputResult { text, session, error: Some(_) } = observed.recv_timeout(Duration::from_secs(1)).unwrap() {
                assert_eq!(text, "Older unsent prompt");
                assert_eq!(session, "original");
                held = true;
            }
        }
        assert!(held);
        assert!(sender.send(Action::Steer("New explicit prompt".into(), Delivery::AfterTool)).is_ok());
        assert!(matches!(control.admit(waiting.recv().unwrap(), "original"), Some(Action::Steer(_, _))));
        assert!(sender.send(Action::Submit("Stay with original session".into())).is_ok());
        assert!(control.admit(waiting.recv().unwrap(), "another").is_none());
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
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
