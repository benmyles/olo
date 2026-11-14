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
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

#[derive(Clone, Debug)]
pub enum Update {
    SettingsSaved {
        key: String,
        result: Result<(), String>,
    },
    Peers(Vec<Peer>),
    SettingsResult {
        key: String,
        result: Result<Value, String>,
    },
    Session(Box<Session>),
    Profiles(Vec<olo_coder_core::profiles::ModelProfile>),
    ProfileModels {
        key: String,
        profile: Box<olo_coder_core::profiles::ModelProfile>,
        models: Result<Vec<String>, String>,
    },
    Catalog {
        rows: Vec<Value>,
        next_offset: Option<usize>,
    },
    InputResult {
        session: String,
        text: String,
        error: Option<String>,
    },
    Repository {
        session: String,
        result: Value,
    },
    Models {
        session: String,
        model: Box<olo_coder_core::ModelConfig>,
        ids: Vec<olo_coder_core::providers::Listed>,
    },
    Event(Event),
    Notice(String),
    Connection(Option<String>),
    Opened(String),
    Rendered(Arc<crate::rich::Text>),
    /// Open or close an action's output in the conversation.
    Toggle(u64),
    /// Open or close every action's output.
    OpenAll(bool),
}
pub enum Action {
    SubmitSettings {
        text: String,
        key: String,
    },
    SubmitForm {
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SettingsRequest {
        key: String,
        command: Box<Command>,
    },
    Submit(String),
    Steer(String, Delivery),
    Prompt {
        text: String,
        delivery: Option<Delivery>,
        session: String,
        generation: u64,
    },
    Browse(SessionFilter),
    Refresh,
    Repository,
    Models,
    Profiles,
    DiscoverProfile {
        key: String,
        profile: Box<olo_coder_core::profiles::ModelProfile>,
    },
    /// A key pasted for `model`'s service, to keep in Passwords
    /// (`models::keep_key`); answered as `SettingsSaved`.
    KeepKey {
        key: String,
        model: Box<olo_coder_core::ModelConfig>,
        secret: String,
    },
    Stop,
    Approve(bool),
    /// Shown differently, nothing sent: an action's output opened or closed.
    Show(Update),
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
        let action = match (action, &self.control) {
            (Action::Show(update), Some(control)) => {
                (control.callback)(update);
                return Ok(());
            }
            (action, _) => action,
        };
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
                    if let Some(peers) = v.get("peers") {
                        if let Ok(peers) = serde_json::from_value(peers.clone()) {
                            callback(Update::Peers(peers));
                        }
                    }
                    if let Some(profiles) = v.get("profiles") {
                        if let Ok(profiles) = serde_json::from_value(profiles.clone()) {
                            callback(Update::Profiles(profiles));
                        }
                    }
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
                                    | "supervisor_configured"
                                    | "supervisor_review"
                                    | "supervisor_usage"
                                    | "integration_configured"
                                    | "integration_removed"
                                    | "integration_login"
                                    | "olo_configured"
                                    | "password_grant"
                                    | "spec"
                                    | "spec_mode"
                                    | "spec_accepted"
                                    | "goal"
                                    | "todos"
                                    | "todos_edited"
                                    | "status_report"
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
/// The newest session in `workspace` nobody has used yet (no turns, no
/// context, no goal, nothing waiting): opening the app again opens that
/// one rather than leaving another empty session behind.
fn untouched(dir: &Path, workspace: &Path) -> Result<Option<String>> {
    let want = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_owned());
    let list = rpc::call(dir, &Command::List)?;
    Ok(list.as_array().into_iter().flatten().find_map(|s| {
        let fresh = s["fresh"].as_bool() == Some(true);
        let same = s["workspace"].as_str().is_some_and(|w| std::fs::canonicalize(w).unwrap_or_else(|_| w.into()) == want);
        (fresh && same).then(|| s["id"].as_str().map(str::to_owned)).flatten()
    }))
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
            let mut renderer = crate::rich::Renderer::default();
            let mut shown = u64::MAX;
            while let Ok(first) = updates.recv() {
                for update in std::iter::once(first).chain(updates.try_iter().take(256)) {
                    let quiet = matches!(update, Update::Toggle(_) | Update::OpenAll(_));
                    view.apply(update.clone());
                    if !quiet {
                        callback(update);
                    }
                }
                if view.feed.generation != shown {
                    shown = view.feed.generation;
                    callback(Update::Rendered(Arc::new(view.feed.document(&mut renderer))));
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
                                None => match untouched(&dir, &workspace)? {
                                    Some(session) => Command::Get { session },
                                    None => Command::Create { workspace: workspace.clone(), config: None },
                                },
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
                                Ok(Action::SubmitSettings { key, .. }) => callback(Update::SettingsSaved { key, result: Err(error) }),
                                Ok(Action::SettingsRequest { key, .. }) => callback(Update::SettingsResult { key, result: Err(error) }),
                                Ok(Action::DiscoverProfile { key, profile }) => callback(Update::ProfileModels { key, profile, models: Err(error) }),
                                // (Passwords, not the service: kept all the same.)
                                Ok(Action::KeepKey { key, model, secret }) => {
                                    let callback = callback.clone();
                                    std::thread::spawn(move || callback(Update::SettingsSaved { key, result: crate::models::keep_key(&model, &secret) }));
                                }
                                Ok(Action::SubmitForm { reply, .. }) => {
                                    let _ = reply.send(Err(error));
                                }
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
                    let (action, mut form_reply, mut form_key) = match action {
                        Action::SubmitForm { text, reply } => (Action::Submit(text), Some(reply), None),
                        Action::SubmitSettings { text, key } => (Action::Submit(text), None, Some(key)),
                        action => (action, None, None),
                    };
                    let mut submitted = match &action {
                        Action::Submit(text) | Action::Steer(text, _) if form_reply.is_none() && form_key.is_none() => Some((s.id.clone(), text.clone())),
                        _ => None,
                    };
                    let result = (|| -> Result<()> {
                        let input = match action {
                            Action::SubmitSettings { .. } | Action::SubmitForm { .. } | Action::Prompt { .. } => unreachable!("prompt admission unwraps queued input"),
                            Action::Quit => return Err(anyhow::anyhow!("__quit")),
                            Action::Show(update) => {
                                callback(update);
                                return Ok(());
                            }
                            Action::SettingsRequest { key, command } => {
                                let dir = dir.clone();
                                let callback = callback.clone();
                                std::thread::spawn(move || callback(Update::SettingsResult { key, result: rpc::call(&dir, &command).map_err(|e| e.to_string()) }));
                                return Ok(());
                            }
                            Action::KeepKey { key, model, secret } => {
                                let callback = callback.clone();
                                std::thread::spawn(move || callback(Update::SettingsSaved { key, result: crate::models::keep_key(&model, &secret) }));
                                return Ok(());
                            }
                            Action::DiscoverProfile { key, profile } => {
                                let dir = dir.clone();
                                let callback = callback.clone();
                                std::thread::spawn(move || {
                                    let command = Command::ModelProfiles { action: "discover".into(), profile: Some((*profile).clone()), id: None, session: None, role: String::new() };
                                    let models = rpc::call(&dir, &command).and_then(|v| Ok(serde_json::from_value(v["models"].clone())?)).map_err(|e| e.to_string());
                                    callback(Update::ProfileModels { key, profile, models });
                                });
                                return Ok(());
                            }
                            Action::Profiles => {
                                let value = rpc::call(&dir, &Command::ModelProfiles { action: "list".into(), profile: None, id: None, session: None, role: String::new() })?;
                                callback(Update::Profiles(serde_json::from_value(value["profiles"].clone())?));
                                return Ok(());
                            }
                            Action::Models => {
                                let dir = dir.clone();
                                let provider = s.config.model.provider;
                                let session = s.id.clone();
                                let model = Box::new(s.config.model.clone());
                                let callback = callback.clone();
                                std::thread::spawn(move || {
                                    if let Ok(value) = rpc::call(&dir, &Command::Models { provider, session: Some(session.clone()) }) {
                                        let ids = olo_coder_core::providers::listed(&model, &value);
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
                                        let mut result = rpc::call(&dir, &Command::Repository { session: session.clone(), arguments: json!({"operation":"status"}) })
                                            .unwrap_or_else(|e| json!({"error":e.to_string()}));
                                        if let Ok(branches) = rpc::call(&dir, &Command::Repository { session: session.clone(), arguments: json!({"operation":"branches"}) }) {
                                            result["branches"] = branches["branches"].clone();
                                        }
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
                                        | Command::ConfigureFields { .. }
                                        | Command::Configure { .. }
                                        | Command::Approve { .. }
                                );
                                let value = rpc::call(&dir, &cmd)?;
                                if let Some(key) = form_key.take() {
                                    callback(Update::SettingsSaved { key, result: Ok(()) });
                                }
                                if let Some(reply) = form_reply.take() {
                                    let _ = reply.send(Ok(()));
                                }
                                if matches!(&cmd, Command::Integrations { .. }) {
                                    if let Some(url) = value["authorization_url"].as_str() {
                                        let url = url.to_owned();
                                        let callback = callback.clone();
                                        std::thread::spawn(move || {
                                            if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::parse(&url)) {
                                                callback(Update::Notice(format!("Open the sign-in link to continue: {e}")));
                                            }
                                        });
                                    }
                                }
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
                    if let Some(key) = form_key.take() {
                        callback(Update::SettingsSaved { key, result: result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")) });
                    }
                    if let Some(reply) = form_reply.take() {
                        let _ = reply.send(result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")));
                    }
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
pub struct View {
    pub peers: Vec<Peer>,
    pub session: Option<Session>,
    pub catalog: Arc<Vec<Value>>,
    pub next_offset: Option<usize>,
    pub rich: Arc<crate::rich::Text>,
    pub repository: Option<Value>,
    /// The models the session's service offers, by name, in its order.
    pub models: Vec<olo_coder_core::providers::Listed>,
    pub profiles: Vec<olo_coder_core::profiles::ModelProfile>,
    pub feed: crate::feed::Feed,
    pub notice: String,
    pub compaction_progress: Option<String>,
    pub connection: Option<String>,
    pub id: String,
    /// The model is writing its reply, or thinking, now.
    pub writing: bool,
    pub thinking: bool,
    /// Events go into the conversation (the presentation worker's view;
    /// a frontend's copy keeps only what it shows around it).
    pub feed_events: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            peers: vec![],
            session: None,
            catalog: Arc::default(),
            next_offset: None,
            rich: Arc::default(),
            repository: None,
            models: vec![],
            profiles: vec![],
            feed: crate::feed::Feed::welcome(),
            notice: String::new(),
            compaction_progress: None,
            connection: None,
            id: String::new(),
            writing: false,
            thinking: false,
            feed_events: true,
        }
    }
}
impl View {
    pub fn apply(&mut self, u: Update) {
        match u {
            Update::Peers(peers) => self.peers = peers,
            Update::ProfileModels { .. } | Update::SettingsResult { .. } | Update::SettingsSaved { .. } => {}
            Update::Profiles(profiles) => self.profiles = profiles,
            Update::Catalog { rows, next_offset } => {
                self.catalog = Arc::new(rows);
                self.next_offset = next_offset;
            }
            Update::InputResult { .. } => {}
            Update::Connection(status) => self.connection = status,
            Update::Rendered(doc) => self.rich = doc,
            Update::Toggle(id) => self.feed.toggle(id),
            Update::OpenAll(open) => self.feed.open_all(open),
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
                self.feed = crate::feed::Feed::welcome();
                self.notice.clear();
                self.compaction_progress = None;
                self.connection = None;
                self.rich = Arc::default();
                self.repository = None;
            }
            Update::Session(s) => {
                if self.session.as_ref().is_some_and(|old| old.id != s.id || old.config.model != s.config.model) {
                    self.models.clear();
                }
                self.session = Some(*s);
            }
            Update::Notice(t) => {
                self.notice = t.clone();
                self.feed.notice(&t);
            }
            Update::Event(e) => {
                if e.session != self.id || (e.seq > 0 && e.seq <= self.feed.seq) {
                    return;
                }
                self.feed.seq = self.feed.seq.max(e.seq);
                if matches!(e.kind.as_str(), "idle" | "error" | "paused" | "interrupted" | "compaction_applied" | "compaction_deferred") {
                    self.compaction_progress = None;
                }
                match e.kind.as_str() {
                    "delta" => self.writing = true,
                    "reasoning" => self.thinking = true,
                    "assistant" | "tool_started" | "approval" | "idle" | "error" | "paused" | "interrupted" | "stop" => {
                        self.writing = false;
                        self.thinking = false;
                    }
                    _ => {}
                }
                match e.kind.as_str() {
                    "compaction_progress" => self.compaction_progress = Some(format!("Summarising {}/{}", e.data["completed"], e.data["total"])),
                    "compaction_ready" => self.compaction_progress = Some("Checkpoint ready".into()),
                    "tool_result" => {
                        let output = &e.data["output"];
                        if output.get("files").is_some() && output.get("branch").is_some() {
                            self.repository = Some(output.clone());
                        }
                    }
                    _ => {}
                }
                if self.feed_events {
                    self.feed.event(&e);
                }
            }
        }
    }
    /// What the session is doing, in a few words.
    pub fn activity(&self) -> String {
        let Some(s) = &self.session else { return self.connection.clone().unwrap_or_else(|| "Connecting…".into()) };
        if let Some(c) = &self.connection {
            return c.clone();
        }
        if let Some(p) = &self.compaction_progress {
            return p.clone();
        }
        match s.status {
            Status::Running if self.writing => "Writing…".into(),
            Status::Running if self.thinking => "Thinking…".into(),
            Status::Running => s.in_flight.as_ref().map(|c| format!("Running {}…", c.name.replace('_', " "))).unwrap_or_else(|| "Working…".into()),
            Status::Approval => "Waiting for your approval".into(),
            Status::Paused => "Paused".into(),
            Status::Interrupted => "Stopped".into(),
            Status::Failed => "Stopped by an error".into(),
            Status::Complete => "Goal reached".into(),
            Status::Idle => "Ready".into(),
        }
    }
    /// The status line under the window, which has the model in its own
    /// menu: what's in context, what's been spent.
    pub fn usage_line(&self) -> String {
        let line = self.status();
        match self.session.as_ref() {
            Some(s) => line.split_once(" · ").filter(|(model, _)| model.starts_with(s.config.model.model.as_str())).map_or(line.clone(), |(_, rest)| rest.to_owned()),
            None => line,
        }
    }
    pub fn status(&self) -> String {
        self.session.as_ref().map_or_else(
            || "Connecting…".into(),
            |s| {
                let window = s.config.model.context_window;
                let short = crate::feed::short;
                let context = if window > 0 { format!("{} of {} in context", short(s.usage.context), short(window)) } else { format!("{} in context", short(s.usage.context)) };
                format!(
                    "{}{} · {} · {} tokens used{}{}",
                    s.config.model.model,
                    if s.config.model.effort.is_empty() { String::new() } else { format!(" ({})", s.config.model.effort) },
                    context,
                    short(s.usage.input + s.usage.output),
                    if s.compactions > 0 { format!(" · {} checkpoints", s.compactions) } else { String::new() },
                    if s.spec_mode { " · Spec mode" } else { "" },
                )
            },
        )
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
/// A command's answer as the conversation shows it: what a person reads
/// (a list, a table, a line saying what changed), never the API's JSON,
/// except for `/api` and commands with nothing better to say.
fn command_result(command: &Command, value: &Value) -> Result<String> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let on = |v: &Value| if v == &Value::Bool(true) { "on" } else { "off" };
    let mut out = String::new();
    match command {
        // Shown where they're made (the task list), and noted in the
        // conversation by their event.
        Command::EditTodos { .. } | Command::Todos { .. } => return Ok(String::new()),
        Command::Integrations { action, .. } => {
            if let Some(url) = value["authorization_url"].as_str() {
                out.push_str(&format!("Sign-in started: [open the sign-in page]({url}).\n"));
            }
            match action.as_str() {
                "" | "list" => {
                    out.push_str("### Connections\n\n");
                    let olo = &value["olo"];
                    out.push_str(&format!("- **Olo Terminal**: {} (names its tab after the work)\n", on(&olo["terminal"])));
                    out.push_str(&format!("- **Agent Screens**: {} (a screen of its own for apps and web pages)\n", on(&olo["screens"])));
                    for server in value["servers"].as_array().into_iter().flatten() {
                        let place = server["url"].as_str().or(server["program"].as_str()).unwrap_or("");
                        out.push_str(&format!("- **{}**: {place}\n", text(&server["name"])));
                    }
                    let shared: Vec<String> = olo["secret_grants"].as_object().into_iter().flatten().map(|(name, _)| format!("“{name}”")).collect();
                    out.push_str(&format!("- **Passwords**: {}\n", if shared.is_empty() { "nothing shared".to_owned() } else { format!("shares {}", shared.join(", ")) }));
                    out.push_str("\nThe Connections menu adds Slack, Logfire or any MCP server.\n");
                }
                "olo" => {
                    let olo = &value["olo"];
                    out.push_str(&format!("Agent Screens {}, terminal titles {}.\n", on(&olo["screens"]), on(&olo["terminal"])));
                }
                "install" | "configure" => out.push_str(&format!("Connected **{}**.\n", text(&value["server"]["name"]))),
                "remove" => out.push_str(&format!("Removed **{}**.\n", text(&value["removed"]))),
                "grant_secret" | "revoke_secret" => out.push_str(&format!("{} “{}” with the agent.\n", if value["shared"] == true { "Shared" } else { "Stopped sharing" }, text(&value["name"]))),
                "test" | "inspect" if value["connected"] == true => {
                    let tools: Vec<String> = value["tools"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().or(t.as_str()).map(str::to_owned)).collect();
                    out.push_str(&format!("Connected: {} tools{}.\n", tools.len(), if tools.is_empty() { String::new() } else { format!(" ({})", tools.join(", ")) }));
                }
                _ if value["authorization_url"].is_string() => {}
                _ => out.push_str(&fenced("json", &serde_json::to_string_pretty(value)?)),
            }
        }
        Command::Supervisor { config, .. } => {
            let c = &value["config"];
            let st = &value["state"];
            if config.is_some() {
                out.push_str(&if c["enabled"] == true { format!("The supervisor is on: it looks over the work every {} turns.\n", c["every_turns"]) } else { "The supervisor is off.\n".to_owned() });
            } else {
                out.push_str(&format!(
                    "### Supervisor · {}\n\nLooked {} times, stepped in {} times.{}\n",
                    if c["enabled"] == true { "On" } else { "Off" },
                    st["reviews"],
                    st["interventions"],
                    st["last_reason"].as_str().filter(|r| !r.is_empty()).map(|r| format!("\n\nLast: {r}")).unwrap_or_default()
                ));
            }
        }
        Command::Compactions { .. } => {
            let rows = value["checkpoints"].as_array().cloned().unwrap_or_default();
            if rows.is_empty() && value["checkpoints"].is_array() {
                out.push_str("Nothing has been summarised yet: the whole conversation is in the context.\n");
            } else if !rows.is_empty() {
                out.push_str("### Summaries of earlier work\n\n| Summary | State | Records | Tokens |\n| --- | --- | ---: | ---: |\n");
                for row in &rows {
                    let id = row["id"].as_str().unwrap_or("");
                    out.push_str(&format!(
                        "| [{}](olo-coder:compaction/{id}) | {} | {} | {} |\n",
                        when(row["created"].as_i64()),
                        text(&row["status"]),
                        row["source_count"].as_i64().unwrap_or(0),
                        row["summary_tokens"].as_i64().map(|t| crate::models::tokens(t as u64)).unwrap_or_default()
                    ));
                }
            } else {
                // One summary: its text, and what it came from.
                if let Some(summary) = value["summary"].as_str() {
                    out.push_str(&format!("### Summary\n\n{summary}\n\n"));
                }
                if let Some(id) = value["parent"].as_str() {
                    out.push_str(&format!("[The summary before it](olo-coder:compaction/{id})\n\n"));
                }
                let sources: Vec<String> = value["prefix_refs"].as_array().into_iter().flatten().filter_map(Value::as_i64).take(100).map(|id| format!("[{id}](olo-coder:transcript/{id})")).collect();
                if !sources.is_empty() {
                    out.push_str(&format!("From records {}\n", sources.join(" · ")));
                }
                if out.is_empty() {
                    out.push_str(&fenced("json", &serde_json::to_string_pretty(value)?));
                }
            }
        }
        Command::Transcript { .. } => {
            if let Some(body) = value["text"].as_str() {
                out.push_str(&format!("### Record {}\n\n{}", value["record"], fenced("json", body)));
                if let Some(next) = value["next_offset"].as_u64() {
                    out.push_str(&format!("\n{} of {} bytes shown.\n", next, value["total_bytes"]));
                }
            } else {
                out.push_str("### The whole conversation, as kept\n\n");
                for row in value["records"].as_array().into_iter().flatten() {
                    if let Some(id) = row["id"].as_i64() {
                        let preview: String = text(&row["preview"]).chars().take(90).collect::<String>().replace(['\n', '|'], " ");
                        out.push_str(&format!("- [{id}](olo-coder:transcript/{id}) {}\n", preview.trim()));
                    }
                }
            }
        }
        Command::TranscriptSearch { .. } => {
            let rows = value.as_array().cloned().unwrap_or_default();
            out.push_str(&format!("### {} found\n\n", if rows.len() == 1 { "One record".to_owned() } else { format!("{} records", rows.len()) }));
            for row in &rows {
                if let Some(id) = row["id"].as_i64() {
                    let preview: String = text(&row["preview"]).chars().take(90).collect::<String>().replace('\n', " ");
                    out.push_str(&format!("- [{id}](olo-coder:transcript/{id}) {}\n", preview.trim()));
                }
            }
        }
        Command::Repository { arguments, .. } => match arguments["operation"].as_str().unwrap_or("status") {
            "log" => {
                out.push_str("### Recent commits\n\n");
                for c in value["commits"].as_array().into_iter().flatten() {
                    let id: String = text(&c["id"]).chars().take(8).collect();
                    let subject = text(&c["message"]).lines().next().unwrap_or("").to_owned();
                    out.push_str(&format!("- `{id}` {subject} · {}\n", text(&c["author"])));
                }
            }
            "branches" => {
                out.push_str("### Branches\n\n");
                for b in value["branches"].as_array().into_iter().flatten() {
                    let at: String = text(&b["target"]).chars().take(8).collect();
                    out.push_str(&format!("- **{}**{}\n", text(&b["name"]), if at.is_empty() { String::new() } else { format!(" at `{at}`") }));
                }
            }
            "worktrees" => {
                out.push_str("### Worktrees\n\n");
                for w in value["worktrees"].as_array().into_iter().flatten() {
                    let branch = w["branch"].as_str().map(|b| format!("**{b}**")).unwrap_or_else(|| "no branch".into());
                    out.push_str(&format!("- {branch} in `{}`{}\n", text(&w["path"]), if w["current"] == true { " (this one)" } else { "" }));
                }
            }
            "status" => {
                let files = value["files"].as_array().cloned().unwrap_or_default();
                out.push_str(&format!(
                    "On **{}**: {}.\n",
                    value["branch"].as_str().unwrap_or("no branch"),
                    if files.is_empty() { "nothing changed".to_owned() } else { format!("{} changed", files.len()) }
                ));
                for f in &files {
                    out.push_str(&format!("- {} `{}`{}\n", text(&f["state"]), text(&f["path"]), if f["staged"] == true { " (staged)" } else { "" }));
                }
            }
            _ => out.push_str(&if let Some(diff) = value["diff"].as_str() { fenced("diff", diff) } else { fenced("json", &serde_json::to_string_pretty(value)?) }),
        },
        Command::Checkpoints { .. } => {
            let rows = value.as_array().or_else(|| value["checkpoints"].as_array()).cloned().unwrap_or_default();
            out.push_str(&format!(
                "### Checkpoints\n\n{}\n",
                if rows.is_empty() { "None yet: the agent makes one before it changes files.".to_owned() } else { format!("{} checkpoints; Edit ▸ Undo goes back to one.", rows.len()) }
            ));
        }
        _ => out.push_str(&if let Some(diff) = value["diff"].as_str() { fenced("diff", diff) } else { fenced("json", &serde_json::to_string_pretty(value)?) }),
    }
    Ok(out)
}

/// When something was made: "2:05", or with its date further back.
fn when(t: Option<i64>) -> String {
    use olo_kit_base::clock;
    let Some(t) = t.filter(|t| *t > 0) else { return "—".into() };
    let (at, today) = (clock::local(t), clock::now());
    if (at.year, at.month, at.day) == (today.year, today.month, today.day) { at.time() } else { format!("{}, {}", at.date(), at.time()) }
}

use crate::feed::fenced;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogs_are_bound_to_the_session_and_endpoint_that_requested_them() {
        let mut session = Session::new("/var/tmp".into(), Config { model: ModelConfig::for_provider(Provider::Openai), ..Config::default() });
        let old = session.config.model.clone();
        let mut view = View { id: session.id.clone(), session: Some(session.clone()), models: listed(&["old-model"]), ..Default::default() };
        session.config.model.endpoint.as_mut().unwrap().base_url = "http://127.0.0.1:9999/v1".into();
        view.apply(Update::Session(Box::new(session.clone())));
        assert!(view.models.is_empty());
        view.apply(Update::Models { session: session.id.clone(), model: Box::new(old), ids: listed(&["stale"]) });
        assert!(view.models.is_empty());
        view.apply(Update::Models { session: session.id, model: Box::new(session.config.model), ids: listed(&["fresh"]) });
        assert_eq!(view.models, listed(&["fresh"]));
    }
    fn listed(ids: &[&str]) -> Vec<olo_coder_core::providers::Listed> {
        ids.iter().map(|id| olo_coder_core::providers::Listed { id: (*id).into(), name: (*id).into(), efforts: vec![], context_window: None }).collect()
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
        let doc = view.feed.document(&mut crate::rich::Renderer::default());
        assert!(doc.text.contains("stopped"), "{}", doc.text);
        assert!(doc.text.contains("Need the missing fixture"));
        assert!(doc.text.contains("Context compacted"));
        assert!(!doc.text.contains("opaque-id") && !doc.text.contains("{\"message\""));
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
        view.feed.notice("A durable conversation");
        let mut r = crate::rich::Renderer::default();
        let before = view.feed.document(&mut r).text;
        for _ in 0..100 {
            view.apply(Update::Connection(Some("Reconnecting…".into())));
        }
        assert_eq!(view.activity(), "Reconnecting…");
        view.apply(Update::Connection(None));
        assert_eq!(view.feed.document(&mut r).text, before);
    }
    #[test]
    fn rich_components_preserve_numeric_cells_and_contain_fences() {
        let table = crate::feed::component(&json!({"kind":"table","title":"Results","columns":["Metric","Value"],"rows":[["Tests",27],["Successful",true]]}));
        assert!(table.contains("| Tests | 27 |"));
        assert!(table.contains("| Successful | true |"));
        let code = fenced("rust\n```", "fn main() {}\n   ```\nnot Markdown");
        assert!(code.starts_with("\n````rust\n"));
        let mut renderer = crate::rich::Renderer::default();
        let doc = renderer.render(&code);
        assert!(doc.text.contains("   ```\nnot Markdown"));
    }
}
