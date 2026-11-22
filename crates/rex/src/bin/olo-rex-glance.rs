//! Kit-free Deskbar Glance provider for Olo Rex (`olo-rex-glance`).
//!
//! Connects event-driven over `rex.control.v1` to the local Rex server (zero
//! polling!) and publishes the `"agents"` Deskbar Glance widget showing live
//! blocked/working agents and active sessions.

#[path = "glance/watch.rs"]
mod watch;

use std::{
    collections::HashMap,
    io::{self, Write},
    os::fd::AsRawFd,
    path::Path,
    process::ExitCode,
};

use olo_protocol::glance::{self, Body, Entry, Glance, Line, Request, Requests, Timeline, content_path, requests_path};
use olo_rex_client::{
    RexError,
    control::ControlClient,
    model::{ClientIdentity, ControlEvent, ProgramPriority, ProgramStatusRecord, SessionSummary, SessionView},
    transport::{discover_or_start_local_socket, local_socket_candidates},
};
use olo_theme::Hue;
use tokio::sync::mpsc;

#[derive(Default)]
struct LiveSummary {
    connected: bool,
    sessions: Vec<SessionSummary>,
    views: HashMap<String, SessionView>,
    statuses: HashMap<(String, String), Vec<ProgramStatusRecord>>,
}

impl LiveSummary {
    fn build_glance(&self, req: &Request) -> Glance {
        let count: usize = req.options.get("count").and_then(|s| s.parse().ok()).unwrap_or(5).clamp(1, 10);

        if !self.connected {
            return Glance {
                icon: Some("rex".into()),
                plain: true,
                title: "Rex Server Offline".into(),
                detail: "Click to open Rex".into(),
                tip: "Rex: local server is not running.".into(),
                ..Glance::default()
            };
        }

        struct AgentItem {
            priority: ProgramPriority,
            state: String,
            session_id: String,
            block_id: String,
            session_label: String,
            title: String,
            hue: Option<Hue>,
        }

        let mut items = Vec::new();
        let mut blocked = 0usize;
        let mut working = 0usize;
        let mut total_panes = 0usize;

        for sess in &self.sessions {
            let Some(view) = self.views.get(&sess.session_id) else { continue };
            for win in &view.windows {
                for blk in win.all_blocks() {
                    total_panes += 1;
                    let recs = self.statuses.get(&(sess.session_id.clone(), blk.block_id.clone())).map(Vec::as_slice).unwrap_or(&[]);
                    if let Some(top) = recs.iter().max_by_key(|r| r.priority()) {
                        let prio = top.priority();
                        match prio {
                            ProgramPriority::Blocked => blocked += 1,
                            ProgramPriority::Working => working += 1,
                            _ => {}
                        }
                        if prio != ProgramPriority::Idle {
                            let label = top
                                .title
                                .as_deref()
                                .or(top.effective_app.as_deref())
                                .or(top.msg.as_deref())
                                .filter(|s| !s.is_empty())
                                .unwrap_or(if !blk.label.is_empty() { &blk.label } else { &win.label })
                                .to_owned();
                            let hue = match prio {
                                ProgramPriority::Blocked | ProgramPriority::Error => Some(Hue::Red),
                                ProgramPriority::Working => Some(Hue::Blue),
                                ProgramPriority::Done => Some(Hue::Green),
                                ProgramPriority::Idle => None,
                            };
                            items.push(AgentItem {
                                priority: prio,
                                state: top.state.clone(),
                                session_id: sess.session_id.clone(),
                                block_id: blk.block_id.clone(),
                                session_label: if sess.label.is_empty() { "Session".into() } else { sess.label.clone() },
                                title: label,
                                hue,
                            });
                        }
                    }
                }
            }
        }

        items.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.session_label.cmp(&b.session_label)));

        let title = if blocked > 0 {
            format!("{blocked} waiting")
        } else if working > 0 {
            format!("{working} working")
        } else if self.sessions.is_empty() {
            "No active sessions".into()
        } else if self.sessions.len() == 1 {
            "1 session".into()
        } else {
            format!("{} sessions", self.sessions.len())
        };

        let detail = if total_panes > 0 { format!("{} pane{}", total_panes, if total_panes == 1 { "" } else { "s" }) } else { String::new() };

        let mut lines = Vec::new();
        let mut open = Vec::new();
        if let Some(first) = items.first() {
            open = vec!["--focus-block".into(), "local".into(), first.session_id.clone(), first.block_id.clone()];
        }
        for it in items.iter().take(count) {
            let tag = match it.state.as_str() {
                "blocked" => "Needs input",
                "error" => "Error",
                "working" => "Working",
                "done" => "Done",
                other => other,
            };
            lines.push(Line { label: tag.into(), text: format!("{}: {}", it.session_label, it.title), hue: it.hue, dim: false, ring: it.priority != ProgramPriority::Blocked });
        }
        if lines.is_empty() {
            for s in self.sessions.iter().take(count) {
                lines.push(Line { label: "Session".into(), text: if s.label.is_empty() { s.session_id.clone() } else { s.label.clone() }, hue: Some(Hue::Blue), dim: false, ring: true });
            }
        }

        let tip = format!("Rex: {title}{}", if detail.is_empty() { String::new() } else { format!(" ({detail})") });
        Glance { icon: Some("rex".into()), value: String::new(), title, detail, note: None, body: if lines.is_empty() { None } else { Some(Body::Lines { lines }) }, tip, plain: true, open }
    }
}

fn read_requests(path: &Path) -> io::Result<Vec<Request>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<Requests>(&bytes)?.widgets),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn write_timelines(dir: &Path, summary: &LiveSummary, reqs: &[Request]) -> io::Result<()> {
    let now = jiff::Timestamp::now().as_second();
    for req in reqs {
        let entry = Entry { from: now, glance: summary.build_glance(req) };
        glance::write_json(&content_path(dir, req.widget), &Timeline { entries: vec![entry], made: now })?;
    }
    Ok(())
}

impl LiveSummary {
    async fn refresh_view(&mut self, ctrl: &ControlClient, sid: &str) -> Result<(), RexError> {
        let view = ctrl.session_view(sid).await?.payload;
        self.statuses.retain(|(s, b), _| s != sid || view.find_block(b).is_some());
        for b in view.windows.iter().flat_map(|w| w.all_blocks()).filter(|b| b.creator_name == "com.superlogical.terminal") {
            let key = (sid.to_owned(), b.block_id.clone());
            if !self.statuses.contains_key(&key)
                && let Ok(records) = ctrl.terminal_program_status(sid, &b.block_id).await
            {
                self.statuses.insert(key, records.payload);
            }
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.session_id == sid) {
            s.label.clone_from(&view.label);
        }
        self.views.insert(sid.to_owned(), view);
        Ok(())
    }

    async fn bootstrap(&mut self, ctrl: &ControlClient) -> Result<u64, RexError> {
        let boot = ctrl.session_bootstrap("all", None, Some(&[]), &[]).await?;
        *self = Self { connected: true, sessions: boot.payload.attach_all.sessions, ..Default::default() };
        for s in self.sessions.clone() {
            // A session may have closed between bootstrap and its view; the
            // corresponding event is already queued on this connection.
            let _ = self.refresh_view(ctrl, &s.session_id).await;
        }
        Ok(boot.seq)
    }

    async fn apply(&mut self, ctrl: &ControlClient, event: ControlEvent) -> bool {
        match event {
            ControlEvent::SessionCreated(s) => {
                if !self.sessions.iter().any(|existing| existing.session_id == s.session_id) {
                    self.sessions.push(s.clone());
                }
                let _ = self.refresh_view(ctrl, &s.session_id).await;
            }
            ControlEvent::SessionDestroyed { session_id } => {
                self.sessions.retain(|s| s.session_id != session_id);
                self.views.remove(&session_id);
                self.statuses.retain(|(sid, _), _| sid != &session_id);
            }
            ControlEvent::SessionMoved { session_id, before_session_id } => {
                let Some(i) = self.sessions.iter().position(|s| s.session_id == session_id) else { return false };
                let s = self.sessions.remove(i);
                let before = self.sessions.iter().position(|s| Some(&s.session_id) == before_session_id.as_ref()).unwrap_or(self.sessions.len());
                self.sessions.insert(before, s);
            }
            ControlEvent::SessionViewChanged { session_id, revision } => {
                if self.views.get(&session_id).is_some_and(|v| v.revision >= revision) {
                    return false;
                }
                return self.refresh_view(ctrl, &session_id).await.is_ok();
            }
            ControlEvent::BlockEvent { session_id, block_id, name, .. } if name == "program_status_changed" || name == "program_status_removed" => {
                let Ok(records) = ctrl.terminal_program_status(&session_id, &block_id).await else { return false };
                self.statuses.insert((session_id, block_id), records.payload);
            }
            _ => return false,
        }
        true
    }
}

async fn local_connection() -> Result<(ControlClient, mpsc::UnboundedReceiver<olo_rex_client::ControlEventEnvelope>), RexError> {
    let path = discover_or_start_local_socket(false).await?;
    let stream = tokio::net::UnixStream::connect(path).await?;
    let identity = ClientIdentity { kind: Some("other".into()), platform: Some("linux".into()), name: Some("olo-rex-glance".into()), ..Default::default() };
    ControlClient::connect_unix(stream, identity).await
}

async fn serve() -> io::Result<()> {
    let dir = glance::dir();
    let request_path = requests_path(&dir, "rex");
    let mut requests_watch = watch::Paths::new(std::slice::from_ref(&request_path))?;
    let mut reqs = read_requests(&request_path)?;
    if reqs.is_empty() {
        return Ok(());
    }
    let mut lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(glance::lock_path(&dir, "rex"))?;
    // SAFETY: Deskbar and other providers use this advisory ownership lock.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    lock.set_len(0)?;
    write!(lock, "{}", std::process::id())?;
    loop {
        // Arm before connecting so a socket appearing during discovery is seen.
        let sockets = watch::Paths::new(&local_socket_candidates())?;
        let mut summary = LiveSummary::default();
        if let Ok((ctrl, mut events)) = local_connection().await
            && let Ok(mut seq) = summary.bootstrap(&ctrl).await
        {
            drop(sockets);
            write_timelines(&dir, &summary, &reqs)?;
            loop {
                tokio::select! {
                    ev = events.recv() => {
                        let Some(ev) = ev else { break };
                        if ev.seq <= seq { continue; }
                        if matches!(ev.event, ControlEvent::EventDropped { .. }) {
                            let Ok(watermark) = summary.bootstrap(&ctrl).await else { break };
                            seq = watermark;
                        } else if !summary.apply(&ctrl, ev.event).await {
                            continue;
                        }
                        write_timelines(&dir, &summary, &reqs)?;
                    }
                    changed = requests_watch.changed() => {
                        changed?;
                        requests_watch = watch::Paths::new(std::slice::from_ref(&request_path))?;
                        reqs = read_requests(&request_path)?;
                        if reqs.is_empty() { ctrl.close(); return Ok(()); }
                        write_timelines(&dir, &summary, &reqs)?;
                    }
                }
            }
            continue;
        }
        write_timelines(&dir, &LiveSummary::default(), &reqs)?;
        tokio::select! {
            changed = sockets.changed() => changed?,
            changed = requests_watch.changed() => {
                changed?;
                requests_watch = watch::Paths::new(std::slice::from_ref(&request_path))?;
                reqs = read_requests(&request_path)?;
                if reqs.is_empty() { return Ok(()); }
            }
        }
    }
}

fn main() -> ExitCode {
    let result = tokio::runtime::Builder::new_current_thread().enable_all().build().and_then(|rt| rt.block_on(serve()));
    if let Err(e) = result {
        eprintln!("olo-rex-glance: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
