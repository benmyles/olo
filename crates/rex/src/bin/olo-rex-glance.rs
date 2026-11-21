//! Kit-free Deskbar Glance provider for Olo Rex (`olo-rex-glance`).
//!
//! Connects event-driven over `rex.control.v1` to the local Rex server (zero
//! polling!) and publishes the `"agents"` Deskbar Glance widget showing live
//! blocked/working agents and active sessions.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use olo_protocol::glance::{Body, Entry, Glance, Line, Request, Requests, Timeline, content_path, requests_path};
use olo_rex_client::{
    control::ControlClient,
    model::{ClientIdentity, ControlEvent, ProgramPriority, ProgramStatusRecord, SessionSummary, SessionView},
    ssh::SshSessionPool,
    transport::RexEndpoint,
};
use olo_theme::Hue;

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

fn glance_dir() -> PathBuf {
    olo_xdg::state_dir().join("glance")
}

fn read_requests() -> Vec<Request> {
    let dir = glance_dir();
    let p = requests_path(&dir, "rex");
    let Ok(bytes) = std::fs::read(&p) else { return Vec::new() };
    serde_json::from_slice::<Requests>(&bytes).map(|r| r.widgets).unwrap_or_default()
}

fn write_timelines(summary: &LiveSummary, reqs: &[Request]) {
    let dir = glance_dir();
    let now = jiff::Timestamp::now().as_second();
    for req in reqs {
        let g = summary.build_glance(req);
        let tl = Timeline { entries: vec![Entry { from: now - now.rem_euclid(60), glance: g }], made: now };
        let path: PathBuf = content_path(&dir, req.widget);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec(&tl) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

fn main() -> ExitCode {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(_) => return ExitCode::FAILURE,
    };
    rt.block_on(async move {
        let mut summary = LiveSummary::default();
        let reqs = read_requests();
        if reqs.is_empty() {
            return ExitCode::SUCCESS;
        }
        write_timelines(&summary, &reqs);

        let pool = Arc::new(SshSessionPool::default());
        let endpoint = RexEndpoint::Local { autostart: false };
        let mut backoff = Duration::from_secs(2);

        loop {
            let reqs = read_requests();
            if reqs.is_empty() {
                return ExitCode::SUCCESS;
            }
            let _ = &pool;
            match endpoint.connect_stream().await {
                Ok((stream, host_hdr)) => {
                    backoff = Duration::from_secs(2);
                    let id = ClientIdentity {
                        kind: Some("other".into()),
                        platform: Some("linux".into()),
                        name: Some("olo-rex-glance".into()),
                        version: Some(env!("CARGO_PKG_VERSION").into()),
                        label: None,
                    };
                    if let Ok((ctrl, mut ev_rx)) = ControlClient::connect(stream, &host_hdr, id, None).await {
                        summary.connected = true;
                        if let Ok(boot) = ctrl.session_bootstrap("all", None, Some(&[]), &[]).await {
                            summary.sessions = boot.payload.attach_all.sessions;
                            for s in &summary.sessions {
                                if let Ok(v) = ctrl.session_view(&s.session_id).await {
                                    for w in &v.payload.windows {
                                        for b in w.all_blocks() {
                                            if b.creator_name == "com.superlogical.terminal"
                                                && let Ok(st) = ctrl.terminal_program_status(&s.session_id, &b.block_id).await
                                            {
                                                summary.statuses.insert((s.session_id.clone(), b.block_id.clone()), st.payload);
                                            }
                                        }
                                    }
                                    summary.views.insert(s.session_id.clone(), v.payload);
                                }
                            }
                        }
                        write_timelines(&summary, &reqs);

                        let mut req_poll = tokio::time::interval(Duration::from_secs(15));
                        req_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        req_poll.tick().await;

                        loop {
                            tokio::select! {
                                _ = req_poll.tick() => {
                                    let r = read_requests();
                                    if r.is_empty() {
                                        ctrl.close();
                                        return ExitCode::SUCCESS;
                                    }
                                }
                                ev = ev_rx.recv() => {
                                    let Some(env) = ev else { break };
                                    let mut changed = false;
                                    match env.event {
                                        ControlEvent::SessionCreated(s) => {
                                            summary.sessions.push(s.clone());
                                            if let Ok(v) = ctrl.session_view(&s.session_id).await {
                                                summary.views.insert(s.session_id, v.payload);
                                            }
                                            changed = true;
                                        }
                                        ControlEvent::SessionDestroyed { session_id } => {
                                            summary.sessions.retain(|s| s.session_id != session_id);
                                            summary.views.remove(&session_id);
                                            summary.statuses.retain(|(sid, _), _| sid != &session_id);
                                            changed = true;
                                        }
                                        ControlEvent::SessionViewChanged { session_id, revision } => {
                                            let cur = summary.views.get(&session_id).map_or(0, |v| v.revision);
                                            if revision > cur
                                                && let Ok(v) = ctrl.session_view(&session_id).await
                                            {
                                                summary.views.insert(session_id, v.payload);
                                                changed = true;
                                            }
                                        }
                                        ControlEvent::BlockEvent { session_id, block_id, name, .. }
                                            if name == "program_status_changed" || name == "program_status_removed" =>
                                        {
                                            if let Ok(st) = ctrl.terminal_program_status(&session_id, &block_id).await {
                                                summary.statuses.insert((session_id, block_id), st.payload);
                                                changed = true;
                                            }
                                        }
                                        _ => {}
                                    }
                                    if changed {
                                        let r = read_requests();
                                        if r.is_empty() {
                                            ctrl.close();
                                            return ExitCode::SUCCESS;
                                        }
                                        write_timelines(&summary, &r);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(_) => {}
            }
            summary.connected = false;
            let r = read_requests();
            if r.is_empty() {
                return ExitCode::SUCCESS;
            }
            write_timelines(&summary, &r);
            let _ = BTreeMap::<String, String>::new();
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    })
}
