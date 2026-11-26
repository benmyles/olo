//! The agents service of a host (`olo-rex-link agent serve`): one
//! program for the account's Rex server, started by the first `olo
//! agent` command (or by an Olo desktop as it connects) and ended when
//! nothing is left for it to do.
//!
//! It owns the children of the server's tabs. It starts each as a tab
//! of the server's (`session.new_window`), writes down whose child it
//! is (the registry, a file of the account's), reads what it shows from
//! the server itself, types into it, and closes it; and it keeps the
//! numbered log of what happens to each (`watch --since`). Nothing of
//! it is the Olo desktop's: with no desktop connected every command
//! works as with one, and a desktop that connects is told everything
//! (`follow`) and shows it.
//!
//! **At rest it does nothing.** It waits on its socket, on the server's
//! connection and on each live child's stream, and on no clock. A
//! child's output is read as a sign only (that something was written,
//! and how much): after a pause in it ([`QUIET`]; [`AT_MOST`] where it
//! never pauses) the child's screen is asked of the server once and
//! read (`olo_agent_screen`), and what changed of its state is written
//! down once it has held a quarter of a second ([`CONFIRM`]).
//!
//! **It can be killed at any moment.** The registry and the log are
//! files, each written whole or line by line; a new service reads them,
//! asks the server which tabs are still there, and goes on: a child
//! whose tab is gone has exited, one that is there is watched again. A
//! command whose service went away starts another and asks again.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use olo_rex_link::agents::wire::{AgentView, Frame, PROTO, Paths, Request, Response};
use olo_rex_link::agents::{Activity, NativeSubagent, ScreenClassification, SubagentEvent, SubagentEventLog, SubagentLiveState, SubagentRegistry, classify_screen_rich, now_unix_ms, to_unix_ms};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc};

use super::rex::{Event, Rex, RexErr, Said};

/// The registry's name for this host: the service knows one.
pub const HOST: &str = "";
/// How long a child's output must pause before its screen is read.
pub const QUIET: Duration = Duration::from_millis(120);
/// And the longest that is waited for such a pause while it goes on
/// writing (a spinner beside a question never pauses).
pub const AT_MOST: Duration = Duration::from_millis(1000);
/// How long a state must hold before it is an event.
pub const CONFIRM: Duration = Duration::from_millis(250);
/// With no child alive and nobody asking, the service ends after this.
const IDLE_EXIT: Duration = Duration::from_secs(600);
/// The longest request taken (a prompt or a file sent whole).
const REQUEST_MOST: usize = 8 << 20;

/// A session of the server, as last seen.
#[derive(Default)]
pub struct Sess {
    pub windows: HashSet<String>,
    /// Each block's window.
    pub blocks: HashMap<String, String>,
    pub active: Option<String>,
    /// Whether nothing has been said to change since it was read.
    fresh: bool,
    /// Whether this connection follows it (its blocks' events, their
    /// streams).
    attached: bool,
}

/// A reading of a child's screen: what it showed, and what that means.
#[derive(Clone)]
pub struct Reading {
    out_seq: u64,
    activity: Activity,
    pub cls: ScreenClassification,
    pub screen: String,
}

/// What is kept of a child while it lives (nothing of it is written
/// down: a new service finds it out again).
pub struct Live {
    /// Counts what its program wrote: a reading is of one count.
    out_seq: u64,
    pub last_output_at: u64,
    pub last_state_change_at: u64,
    read: Option<Reading>,
    /// What its program says of itself (OSC 7501), by the record's name.
    statuses: Vec<(String, Activity, Option<String>)>,
    pub exit_code: Option<i32>,
    /// The terminal's own process (the shell the server started).
    pub pid: Option<i32>,
    rows: u16,
    last_state: Option<SubagentLiveState>,
    last_key: Option<String>,
    check_gen: u64,
    seen_working: bool,
    /// Asks its watcher to read the screen now.
    poke: Arc<Notify>,
    task: Option<tokio::task::AbortHandle>,
    /// What followers were last told of it.
    told: Option<(SubagentLiveState, Option<String>)>,
    /// A feedback survey on its screen wouldn't go when set aside: left
    /// alone until it has gone by itself.
    survey_stays: bool,
}

impl Live {
    fn new() -> Self {
        let now = now_unix_ms();
        Self {
            out_seq: 0,
            last_output_at: now,
            last_state_change_at: now,
            read: None,
            statuses: Vec::new(),
            exit_code: None,
            pid: None,
            rows: 0,
            last_state: None,
            last_key: None,
            check_gen: 0,
            seen_working: false,
            poke: Arc::new(Notify::new()),
            task: None,
            told: None,
            survey_stays: false,
        }
    }

    fn activity(&self) -> (Activity, Option<String>) {
        self.statuses.iter().max_by_key(|(_, a, _)| a.rank()).map(|(_, a, msg)| (*a, msg.clone())).unwrap_or_default()
    }
}

pub struct State {
    pub reg: SubagentRegistry,
    pub log: SubagentEventLog,
    pub live: HashMap<String, Live>,
    pub sessions: HashMap<String, Sess>,
    /// Connections being answered now.
    clients: usize,
    /// Of them, those asking something that is done once and isn't a
    /// wait (not a `watch`, not a `follow`): what the service finishes
    /// before it ends.
    doing: usize,
    /// Since when nothing has been asked and no child lives.
    idle_since: Option<Instant>,
}

/// A child as it stands, read now or as last read.
pub struct Looked {
    pub cls: ScreenClassification,
    pub screen: String,
    pub last_output_at: u64,
    pub last_state_change_at: u64,
    pub pid: Option<i32>,
    /// A feedback survey is drawn over it and wouldn't be set aside:
    /// what is read may be covered by it.
    pub survey: bool,
}

pub struct Service {
    pub rex: Arc<Rex>,
    pub paths: Paths,
    st: Mutex<State>,
    /// Something a waiter may be waiting for happened: an event written,
    /// a question come or gone, an exit.
    pub changed: Notify,
    frames: broadcast::Sender<Frame>,
    /// `watch --match` and `--regex` in progress: only then is every
    /// reading of a screen a reason to wake the watchers.
    pub text_watchers: AtomicUsize,
    /// The registry or the log has something to write.
    save: Notify,
    /// The service has become idle, or stopped being so.
    idle: Notify,
    /// Set to end the service once what it has is written.
    retire: Notify,
}

fn exited(summary: String) -> ScreenClassification {
    ScreenClassification { state: SubagentLiveState::Exited, summary: Some(summary), approval: None, folder_trust: None, api_error: None, usage: None }
}

fn starting() -> ScreenClassification {
    ScreenClassification { state: SubagentLiveState::Working, summary: Some("starting".into()), approval: None, folder_trust: None, api_error: None, usage: None }
}

/// What tells two readings of one state apart: the question asked, the
/// error said.
pub fn subkey(cls: &ScreenClassification) -> String {
    if let Some(ap) = &cls.approval {
        format!("ap:{}", ap.key)
    } else if let Some(tr) = &cls.folder_trust {
        format!("trust:{}:{}", tr.folder.as_deref().unwrap_or(""), tr.selected_index)
    } else if let Some(err) = &cls.api_error {
        format!("err:{err}")
    } else if cls.state == SubagentLiveState::WaitingQuestion {
        format!("q:{}", cls.summary.as_deref().unwrap_or(""))
    } else {
        String::new()
    }
}

pub fn tail_of(screen: &str, lines: usize) -> Vec<String> {
    screen.lines().rev().filter(|l| !l.trim().is_empty()).take(lines).map(str::to_owned).collect::<Vec<_>>().into_iter().rev().collect()
}

/// Whether the process that asks runs another file of the helper than
/// this service does, written later (a build put on the host since this
/// one started). The same file, or an earlier one (a watcher from before
/// an upgrade, still waiting), is not: the service is never handed back.
fn asker_is_a_later_build(pid: i32) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    // (Through /proc's own links: a file taken off the disk since, as an
    // upgrade's tidying does, is still read.)
    let (Ok(mine), Ok(theirs)) = (std::fs::metadata("/proc/self/exe"), std::fs::metadata(format!("/proc/{pid}/exe"))) else { return false };
    (mine.dev(), mine.ino()) != (theirs.dev(), theirs.ino()) && (theirs.mtime(), theirs.mtime_nsec()) > (mine.mtime(), mine.mtime_nsec())
}

impl Service {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- The server's sessions ----

    /// Reads a session's windows and blocks from the server.
    pub async fn refresh(&self, session: &str) -> Result<(), RexErr> {
        let view = self.rex.call("session.view", json!({ "session_id": session })).await?;
        let mut st = self.lock();
        let sess = st.sessions.entry(session.to_owned()).or_default();
        sess.windows.clear();
        sess.blocks.clear();
        sess.active = view["active_window_id"].as_str().map(str::to_owned);
        for w in view["windows"].as_array().into_iter().flatten() {
            let Some(wid) = w["window_id"].as_str() else { continue };
            sess.windows.insert(wid.to_owned());
            for layer in w["layers"].as_array().into_iter().flatten() {
                for b in layer["blocks"].as_array().into_iter().flatten() {
                    if let Some(bid) = b["block_id"].as_str() {
                        sess.blocks.insert(bid.to_owned(), wid.to_owned());
                    }
                }
            }
        }
        sess.fresh = true;
        Ok(())
    }

    /// The session as the server has it now (read again only where
    /// something was said to change).
    pub async fn seen(&self, session: &str) -> Result<(), RexErr> {
        if self.lock().sessions.get(session).is_some_and(|s| s.fresh) {
            return Ok(());
        }
        self.refresh(session).await
    }

    pub async fn session_ids(&self) -> Result<Vec<String>, RexErr> {
        let list = self.rex.call("session.list", Value::Null).await?;
        Ok(list["sessions"].as_array().into_iter().flatten().filter_map(|s| s["session_id"].as_str().map(str::to_owned)).collect())
    }

    /// The session a block is in, looked for in every session.
    pub async fn session_of_block(&self, block: &str) -> Option<String> {
        let found = |st: &State| st.sessions.iter().find(|(_, s)| s.fresh && s.blocks.contains_key(block)).map(|(id, _)| id.clone());
        if let Some(id) = found(&self.lock()) {
            return Some(id);
        }
        for id in self.session_ids().await.ok()? {
            let _ = self.seen(&id).await;
        }
        found(&self.lock())
    }

    /// Follows a session (its blocks' events, and leave to open their
    /// streams), once.
    async fn attach(&self, session: &str) -> Result<(), RexErr> {
        if self.lock().sessions.get(session).is_some_and(|s| s.attached) {
            return Ok(());
        }
        self.rex.call("session.attach", json!({ "session_id": session })).await?;
        self.lock().sessions.entry(session.to_owned()).or_default().attached = true;
        Ok(())
    }

    // ---- A child's screen ----

    /// How a child stands. `fresh`: asked of the server now, whatever
    /// was read before; otherwise the last reading where nothing was
    /// written since.
    pub async fn look(&self, entry: &NativeSubagent, fresh: bool) -> Looked {
        let (reading, out_seq, activity, status_msg, rows) = {
            let st = self.lock();
            let alive = entry.exited_at.is_none() && st.reg.by_window(HOST, &entry.session_id, &entry.window_id).is_some();
            let Some(live) = st.live.get(&entry.id).filter(|_| alive) else {
                // (Its program ended and its tab stays, or the tab is gone.)
                let code = st.live.get(&entry.id).and_then(|l| l.exit_code).or(entry.exit_code);
                let cls = match code {
                    Some(code) => exited(format!("exited (code {code})")),
                    None => exited("tab closed".into()),
                };
                let (out, chg) = st.live.get(&entry.id).map(|l| (l.last_output_at, l.last_state_change_at)).unwrap_or((0, 0));
                return Looked { cls, screen: String::new(), last_output_at: out, last_state_change_at: chg, pid: entry.last_pid, survey: false };
            };
            if let Some(code) = live.exit_code {
                return Looked {
                    cls: exited(format!("exited (code {code})")),
                    screen: String::new(),
                    last_output_at: live.last_output_at,
                    last_state_change_at: live.last_state_change_at,
                    pid: live.pid.or(entry.last_pid),
                    survey: false,
                };
            }
            let (activity, status_msg) = live.activity();
            if !fresh && let Some(read) = live.read.as_ref().filter(|r| r.out_seq == live.out_seq && r.activity == activity) {
                return Looked {
                    cls: read.cls.clone(),
                    screen: read.screen.clone(),
                    last_output_at: live.last_output_at,
                    last_state_change_at: live.last_state_change_at,
                    pid: live.pid.or(entry.last_pid),
                    survey: live.survey_stays,
                };
            }
            (live.read.clone(), live.out_seq, activity, status_msg, live.rows)
        };
        let mut survey = false;
        let (cls, screen) = match self.screen_under_survey(entry, usize::from(rows), &mut survey).await {
            Ok(screen) => {
                let mut cls = classify_screen_rich(entry.cli, activity, &screen, true);
                if cls.summary.is_none() {
                    cls.summary = status_msg;
                }
                (cls, screen)
            }
            // (Its tab is gone: the event that says so is on its way.)
            Err(e) if e.gone() => (exited("tab closed".into()), String::new()),
            // (The server didn't say: as it last stood, or starting.)
            Err(_) => reading.map(|r| (r.cls, r.screen)).unwrap_or_else(|| (starting(), String::new())),
        };
        let mut st = self.lock();
        let (mut out, mut chg, mut pid) = (0, 0, entry.last_pid);
        if let Some(live) = st.live.get_mut(&entry.id) {
            if cls.state != SubagentLiveState::Exited {
                live.read = Some(Reading { out_seq, activity, cls: cls.clone(), screen: screen.clone() });
            }
            live.survey_stays = survey;
            (out, chg, pid) = (live.last_output_at, live.last_state_change_at, live.pid.or(entry.last_pid));
        }
        Looked { cls, screen, last_output_at: out, last_state_change_at: chg, pid, survey }
    }

    /// A child's screen as the server has it, with a feedback survey
    /// drawn over it set aside first (its own "0: Dismiss", "[0] Skip":
    /// never a question for whoever looks after the agent, and a
    /// permission prompt may lie under it). Three times at most; one
    /// that stays is said (`survey`) and left alone until it has gone.
    async fn screen_under_survey(&self, entry: &NativeSubagent, rows: usize, survey: &mut bool) -> Result<String, RexErr> {
        let mut screen = self.rex.screen(&entry.session_id, &entry.block_id, rows).await?;
        let Some(agent) = entry.cli.screen_agent() else { return Ok(screen) };
        let stays = self.lock().live.get(&entry.id).is_some_and(|l| l.survey_stays);
        for _ in 0..3 {
            let Some(found) = olo_agent_screen::parse_survey(agent, &screen) else {
                *survey = false;
                return Ok(screen);
            };
            *survey = true;
            if stays {
                break;
            }
            self.rex.write(&entry.session_id, &entry.block_id, found.dismiss).await?;
            tokio::time::sleep(Duration::from_millis(300)).await;
            screen = self.rex.screen(&entry.session_id, &entry.block_id, rows).await?;
        }
        *survey = olo_agent_screen::parse_survey(agent, &screen).is_some();
        Ok(screen)
    }

    /// Forgets what was last made of a child (after typing into it: the
    /// next thing it shows is an event even where it looks the same).
    pub fn reset_edge(&self, entry: &NativeSubagent, clear_seen_working: bool) {
        let mut st = self.lock();
        if let Some(live) = st.live.get_mut(&entry.id) {
            live.last_state = None;
            live.last_key = None;
            live.read = None;
            if clear_seen_working {
                live.seen_working = false;
            }
        }
        st.log.reset_child_dedupe(&entry.id);
    }

    /// Has a child's watcher read its screen now.
    pub fn poke(&self, id: &str) {
        if let Some(live) = self.lock().live.get(id) {
            live.poke.notify_one();
        }
    }

    // ---- What is read of the agents themselves (their transcripts, their processes) ----

    /// Each child's agent as this host has it: the conversation it is
    /// in, its last message whole, the tool call it waits on, what it
    /// runs. What was found of a conversation is kept.
    pub async fn inspect(self: &Arc<Self>, entries: &[NativeSubagent]) -> HashMap<String, Value> {
        let entries: Vec<&NativeSubagent> = entries.iter().filter(|e| e.cli.is_agent()).collect();
        if entries.is_empty() {
            return HashMap::new();
        }
        let items: Vec<crate::agent::InspectItemRequest> = {
            let st = self.lock();
            entries
                .iter()
                .map(|e| crate::agent::InspectItemRequest {
                    id: e.id.clone(),
                    cli: e.cli.binary().to_owned(),
                    shell_pid: st.live.get(&e.id).and_then(|l| l.pid).or(e.last_pid).unwrap_or(0),
                    cwd: e.cwd.clone().unwrap_or_default(),
                    known_session_id: e.effective_session_id().map(str::to_owned),
                    known_transcript: e.transcript_path.clone(),
                    started_at: e.started_at,
                })
                .collect()
        };
        // (Files and `/proc`, read off the service's own thread.)
        let Ok(found) = tokio::task::spawn_blocking(move || crate::agent::inspect_items(&items)).await else { return HashMap::new() };
        let mut map = HashMap::new();
        let mut changed = false;
        {
            let mut st = self.lock();
            for item in found {
                let Ok(v) = serde_json::to_value(&item) else { continue };
                let Some(id) = v["id"].as_str().map(str::to_owned) else { continue };
                changed |= st.reg.update_discovered(&id, v["cli_session_id"].as_str(), v["session_confidence"].as_str(), v["transcript_path"].as_str(), v["pid"].as_i64().map(|p| p as i32));
                map.insert(id, v);
            }
        }
        if changed {
            self.save.notify_one();
        }
        map
    }

    pub fn pending_tools(info: Option<&Value>) -> Vec<Value> {
        if let Some(arr) = info.and_then(|v| v["pending_tools"].as_array()).filter(|a| !a.is_empty()) {
            return arr.clone();
        }
        info.map(|v| &v["pending_tool"]).filter(|v| !v.is_null()).map(|v| vec![v.clone()]).unwrap_or_default()
    }

    // ---- What happens to a child ----

    /// Writes an event down and tells who waits: in that order. An event
    /// a watcher has been told is in the log's file already (one line
    /// appended, there whatever becomes of this process), so a service
    /// killed the moment after goes on from that event's number and
    /// never says it twice.
    pub fn record(&self, event: SubagentEvent) -> u64 {
        let seq = {
            let mut st = self.lock();
            let before = st.log.latest_seq();
            let seq = st.log.push(event.clone());
            if seq > before {
                let mut event = event;
                event.seq = seq;
                st.reg.event_seq = seq;
                // (Under the lock: the file's lines are in the log's order.)
                if let Ok(mut line) = serde_json::to_string(&event) {
                    use std::io::Write as _;
                    line.push('\n');
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.paths.events) {
                        let _ = f.write_all(line.as_bytes());
                    }
                }
            }
            seq
        };
        self.save.notify_one();
        self.changed.notify_waiters();
        seq
    }

    pub fn event_of(&self, entry: &NativeSubagent, state: SubagentLiveState, summary: Option<String>, dedupe_key: String) -> SubagentEvent {
        let ancestors = self.lock().reg.ancestor_windows(HOST, &entry.session_id, &entry.window_id);
        SubagentEvent {
            seq: 0,
            timestamp_ms: now_unix_ms(),
            host_key: HOST.to_owned(),
            session_id: entry.session_id.clone(),
            parent_window_id: entry.parent_window_id.clone(),
            ancestor_windows: ancestors,
            id: entry.id.clone(),
            name: entry.name.clone(),
            window_id: entry.window_id.clone(),
            block_id: entry.block_id.clone(),
            resume_id: entry.resume_id.clone(),
            cli_session_id: entry.effective_session_id().map(str::to_owned),
            event: state.as_str().to_owned(),
            state,
            summary,
            last_message: None,
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key,
        }
    }

    /// Tells followers how a child stands, where that differs from what
    /// they were last told.
    fn tell(&self, entry: &NativeSubagent, state: SubagentLiveState, summary: Option<String>) {
        // (What a working child last printed changes with every line: not said.)
        let summary = if state == SubagentLiveState::Working { None } else { summary };
        {
            let mut st = self.lock();
            let Some(live) = st.live.get_mut(&entry.id) else { return };
            if live.told.as_ref().is_some_and(|(s, m)| *s == state && *m == summary) {
                return;
            }
            live.told = Some((state, summary.clone()));
        }
        let _ = self.frames.send(Frame::Agent { agent: Box::new(AgentView { entry: entry.clone(), state, summary }) });
    }

    /// The registry has something to write.
    pub fn saved(&self) {
        self.save.notify_one();
    }

    /// Says a thing to followers.
    pub fn said(&self, frame: Frame) {
        let _ = self.frames.send(frame);
    }

    /// Tells followers a child's record as it is now (it started, was
    /// found out more of, exited).
    pub fn tell_entry(&self, id: &str) {
        let view = {
            let st = self.lock();
            let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
            let (state, summary) = match (entry.exited_at, st.live.get(id).and_then(|l| l.told.clone())) {
                (Some(_), _) => (SubagentLiveState::Exited, None),
                (None, Some((state, summary))) => (state, summary),
                (None, None) => (SubagentLiveState::Working, None),
            };
            AgentView { entry, state, summary }
        };
        let _ = self.frames.send(Frame::Agent { agent: Box::new(view) });
    }

    /// A child's program ended (`code`), or its tab closed (none).
    pub fn exit(&self, id: &str, code: Option<i32>) {
        let moved: Vec<String>;
        let entry = {
            let mut st = self.lock();
            if let (Some(live), Some(code)) = (st.live.get_mut(id), code) {
                live.exit_code = Some(code);
            }
            let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
            // (Its own children go under its parent: said to followers too.)
            moved = st
                .reg
                .entries
                .iter()
                .filter(|e| e.session_id == entry.session_id && e.exited_at.is_none() && e.parent_window_id == entry.window_id && e.id != entry.id)
                .map(|e| e.id.clone())
                .collect();
            if !st.reg.mark_exited_with_code(HOST, &entry.session_id, &entry.window_id, code) {
                // (Said already; its tab closing after its program ended ends its watcher.)
                if code.is_none()
                    && let Some(live) = st.live.remove(id)
                    && let Some(task) = live.task
                {
                    task.abort();
                }
                return;
            }
            if code.is_none()
                && let Some(live) = st.live.remove(id)
                && let Some(task) = live.task
            {
                task.abort();
            }
            entry
        };
        let (summary, error) = match code {
            Some(code) => (format!("exited (code {code})"), (code != 0).then(|| format!("process exited with code {code}"))),
            None => ("tab closed".to_owned(), None),
        };
        let mut event = self.event_of(&entry, SubagentLiveState::Exited, Some(summary), "exited".into());
        event.error = error;
        self.record(event);
        self.tell_entry(id);
        for child in &moved {
            self.tell_entry(child);
        }
        self.idle.notify_one();
    }

    /// Reads a child's screen and, where its state differs from what was
    /// last made of it, writes that down once it has held ([`CONFIRM`]):
    /// with its last message whole and the tool it waits on, from its
    /// transcript.
    async fn transition(self: &Arc<Self>, id: &str) {
        let Some(entry) = self.lock().reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
        let looked = self.look(&entry, false).await;
        let cls = looked.cls;
        // Its tab is gone (the server said so of its screen): its end is
        // said once, as an end is said (`exit`), whichever word of it
        // comes first: this, or the server's own event that the tab
        // closed. Never as a state that held.
        if cls.state == SubagentLiveState::Exited {
            if !self.still_there(&entry).await {
                self.exit(id, None);
            }
            return;
        }
        self.tell(&entry, cls.state, cls.summary.clone());
        if self.text_watchers.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            self.changed.notify_waiters();
        }
        let key = subkey(&cls);
        let generation = {
            let mut st = self.lock();
            let Some(live) = st.live.get_mut(id) else { return };
            // (Anything but rest says it has begun: at work, asking, failed.)
            if cls.state != SubagentLiveState::Idle {
                live.seen_working = true;
            }
            // (A child just started is at rest before it ever worked: not an event.)
            let age_ms = (now_unix_ms() as i64).saturating_sub(to_unix_ms(entry.started_at));
            let unstarted = cls.state == SubagentLiveState::Idle && !live.seen_working && age_ms < 15_000;
            if live.last_state == Some(cls.state) && live.last_key.as_deref() == Some(key.as_str()) {
                return;
            }
            live.last_state = Some(cls.state);
            live.last_key = Some(key.clone());
            live.last_state_change_at = now_unix_ms();
            live.check_gen = live.check_gen.wrapping_add(1);
            if unstarted || cls.state == SubagentLiveState::Working { None } else { Some(live.check_gen) }
        };
        // (A question come or gone: who waits on that looks at once.)
        if cls.folder_trust.is_some() || cls.approval.is_some() || cls.state == SubagentLiveState::Exited || cls.state == SubagentLiveState::Working {
            self.changed.notify_waiters();
        }
        let Some(generation) = generation else { return };
        let (service, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(CONFIRM).await;
            let current = |service: &Service| service.lock().live.get(&id).is_some_and(|l| l.check_gen == generation);
            if !current(&service) {
                return;
            }
            let Some(entry) = service.lock().reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
            let again = service.look(&entry, true).await;
            if again.cls.state == SubagentLiveState::Exited {
                // (Gone meanwhile: its end is `exit`'s to say.)
                service.poke(&id);
                return;
            }
            if again.cls.state != cls.state || subkey(&again.cls) != key {
                // (It didn't hold: what it shows now is read in its turn.)
                service.poke(&id);
                return;
            }
            let inspected = service.inspect(std::slice::from_ref(&entry)).await;
            // (What took a while to read may have been overtaken.)
            if !current(&service) {
                return;
            }
            let info = inspected.get(&id);
            let entry = service.lock().reg.entries.iter().rev().find(|e| e.id == id).cloned().unwrap_or(entry);
            let last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
            let pending = Self::pending_tools(info);
            let approval = again.cls.approval.as_ref().map(|ap| olo_rex_link::agents::approval_to_json(ap, &pending));
            let (state, error, _) = olo_rex_link::agents::effective_state(&again.cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), true);
            let dedupe = if !key.is_empty() {
                key
            } else if let Some(message) = &last_message {
                use std::hash::{Hash as _, Hasher as _};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                message.hash(&mut h);
                format!("msg:{:x}", h.finish())
            } else {
                // (What it shows: the same after a service is started
                // again, so what was said of it isn't said twice; another
                // after another turn.)
                format!("scr:{:x}", olo_agent_screen::fingerprint(&again.screen))
            };
            let mut event = service.event_of(&entry, state, again.cls.summary.clone(), dedupe);
            event.last_message = last_message;
            event.approval = approval;
            event.error = error;
            event.tail = tail_of(&again.screen, 10);
            service.tell(&entry, state, again.cls.summary);
            service.record(event);
        });
    }

    /// Watches a live child: its stream for output and its program's
    /// end, and its screen after each pause. Ends with the child.
    pub fn watch_child(self: &Arc<Self>, entry: &NativeSubagent) {
        let (service, entry) = (self.clone(), entry.clone());
        let poke = {
            let mut st = self.lock();
            let live = st.live.entry(entry.id.clone()).or_insert_with(Live::new);
            if let Some(old) = live.task.take() {
                old.abort();
            }
            live.poke.clone()
        };
        let id = entry.id.clone();
        let task = tokio::spawn(async move {
            // What the terminal's own process is, its size and what its
            // program says of itself so far.
            service.learn(&entry).await;
            let mut retry = Duration::from_millis(250);
            loop {
                let mut stream = match service.rex.stream(&entry.session_id, &entry.block_id).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        if e.gone() || !service.still_there(&entry).await {
                            service.exit(&entry.id, None);
                            return;
                        }
                        tokio::time::sleep(retry).await;
                        retry = (retry * 2).min(Duration::from_secs(30));
                        continue;
                    }
                };
                retry = Duration::from_millis(250);
                // (As it stands when the watching begins.)
                service.transition(&entry.id).await;
                // Since when something written is unread, and when it
                // was last written.
                let mut unread: Option<(tokio::time::Instant, tokio::time::Instant)> = None;
                loop {
                    let due = unread.map(|(first, last)| (last + QUIET).min(first + AT_MOST));
                    tokio::select! {
                        said = stream.next() => match said {
                            Said::Output(_) => {
                                let now = tokio::time::Instant::now();
                                unread = Some((unread.map_or(now, |(first, _)| first), now));
                                if let Some(live) = service.lock().live.get_mut(&entry.id) {
                                    live.out_seq += 1;
                                    live.last_output_at = now_unix_ms();
                                }
                            }
                            Said::Exited(code) => {
                                service.exit(&entry.id, Some(code));
                                // (Its tab may stay: watched until it closes.)
                            }
                            Said::Resized { rows } => {
                                if let Some(live) = service.lock().live.get_mut(&entry.id) {
                                    live.rows = rows;
                                    live.read = None;
                                }
                            }
                            Said::Closed => break,
                        },
                        _ = async { tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now)).await }, if due.is_some() => {
                            unread = None;
                            service.transition(&entry.id).await;
                        }
                        _ = poke.notified() => {
                            service.transition(&entry.id).await;
                        }
                    }
                }
                // The stream ended: with the tab, or by itself.
                if !service.still_there(&entry).await {
                    service.exit(&entry.id, None);
                    return;
                }
                tokio::time::sleep(retry).await;
            }
        });
        if let Some(live) = self.lock().live.get_mut(&id) {
            live.task = Some(task.abort_handle());
        }
    }

    /// Whether a child's tab is still the server's.
    async fn still_there(&self, entry: &NativeSubagent) -> bool {
        if self.refresh(&entry.session_id).await.is_err() {
            // (No such session any more; or the server is gone, and the service with it.)
            return false;
        }
        self.lock().sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&entry.window_id) && s.blocks.contains_key(&entry.block_id))
    }

    /// What a child's terminal is: its process, its size, what its
    /// program has said of itself.
    async fn learn(&self, entry: &NativeSubagent) {
        let (process, size, status) = tokio::join!(
            self.rex.terminal(&entry.session_id, &entry.block_id, "process", json!({})),
            self.rex.terminal(&entry.session_id, &entry.block_id, "size", json!({})),
            self.rex.terminal(&entry.session_id, &entry.block_id, "program_status", json!({})),
        );
        let exit_code = process.as_ref().ok().and_then(|p| p["last_exit"]["exit_code"].as_i64()).map(|c| c as i32);
        {
            let mut st = self.lock();
            let Some(live) = st.live.get_mut(&entry.id) else { return };
            if let Ok(p) = &process {
                live.pid = p["child"]["pid"].as_i64().or_else(|| p["foreground"]["pid"].as_i64()).map(|p| p as i32).filter(|p| *p > 0).or(live.pid);
            }
            if let Ok(s) = &size {
                live.rows = s["rows"].as_u64().unwrap_or(0) as u16;
            }
            if let Ok(s) = &status {
                live.statuses = s["records"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        (r["id"].as_str().unwrap_or_default().to_owned(), Activity::of(r["state"].as_str().unwrap_or_default()), r["msg"].as_str().or_else(|| r["title"].as_str()).map(str::to_owned))
                    })
                    .collect();
            }
        }
        if let Some(code) = exit_code {
            self.exit(&entry.id, Some(code));
        }
    }

    // ---- What the server says unasked ----

    async fn on_event(self: &Arc<Self>, event: Event) {
        match event {
            Event::Session { session, kind, payload } => {
                let child_of = |st: &State, window: Option<&str>, block: Option<&str>| {
                    st.reg
                        .entries
                        .iter()
                        .rev()
                        .find(|e| e.session_id == session && st.live.contains_key(&e.id) && (window.is_some_and(|w| e.window_id == w) || block.is_some_and(|b| e.block_id == b)))
                        .map(|e| e.id.clone())
                };
                match kind.as_str() {
                    "window_closed" | "block_closed" => {
                        let id = {
                            let mut st = self.lock();
                            if let Some(sess) = st.sessions.get_mut(&session) {
                                sess.fresh = false;
                            }
                            child_of(&st, payload["window_id"].as_str(), payload["block_id"].as_str())
                        };
                        if let Some(id) = id {
                            self.exit(&id, None);
                        }
                    }
                    "window_created" | "block_created" | "session_view_changed" | "active_window_changed" => {
                        if let Some(sess) = self.lock().sessions.get_mut(&session) {
                            sess.fresh = false;
                        }
                    }
                    "block_event" => {
                        let name = payload["event"].as_str().or_else(|| payload["name"].as_str()).unwrap_or_default();
                        let Some(id) = child_of(&self.lock(), None, payload["block_id"].as_str()) else { return };
                        let inner = if payload["payload"].is_null() { &payload["data"] } else { &payload["payload"] };
                        match name {
                            "program_status_changed" => {
                                let record = &inner["record"];
                                let said = (
                                    inner["id"].as_str().unwrap_or_default().to_owned(),
                                    Activity::of(record["state"].as_str().unwrap_or_default()),
                                    record["msg"].as_str().or_else(|| record["title"].as_str()).map(str::to_owned),
                                );
                                let before = {
                                    let mut st = self.lock();
                                    let Some(live) = st.live.get_mut(&id) else { return };
                                    let before = live.activity().0;
                                    match live.statuses.iter_mut().find(|(name, _, _)| *name == said.0) {
                                        Some(old) => *old = said,
                                        None => live.statuses.push(said),
                                    }
                                    (before, live.activity().0)
                                };
                                // (What it does now, not each word it says of it.)
                                if before.0 != before.1 {
                                    self.poke(&id);
                                }
                            }
                            "program_status_removed" => {
                                let gone: Vec<&str> = inner["ids"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                                if let Some(live) = self.lock().live.get_mut(&id) {
                                    live.statuses.retain(|(name, _, _)| !gone.contains(&name.as_str()));
                                }
                                self.poke(&id);
                            }
                            "process_changed" => {
                                let Some(entry) = self.lock().reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
                                let service = self.clone();
                                tokio::spawn(async move {
                                    let Ok(p) = service.rex.terminal(&entry.session_id, &entry.block_id, "process", json!({})).await else { return };
                                    if let Some(live) = service.lock().live.get_mut(&entry.id) {
                                        live.pid = p["child"]["pid"].as_i64().or_else(|| p["foreground"]["pid"].as_i64()).map(|p| p as i32).filter(|p| *p > 0).or(live.pid);
                                    }
                                    match p["last_exit"]["exit_code"].as_i64() {
                                        Some(code) => service.exit(&entry.id, Some(code as i32)),
                                        None => service.poke(&entry.id),
                                    }
                                });
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::Server { kind, payload } => {
                if kind == "session_destroyed"
                    && let Some(session) = payload["session_id"].as_str()
                {
                    let ids: Vec<String> = {
                        let mut st = self.lock();
                        st.sessions.remove(session);
                        st.reg.entries.iter().filter(|e| e.session_id == session && e.exited_at.is_none()).map(|e| e.id.clone()).collect()
                    };
                    for id in ids {
                        self.exit(&id, None);
                    }
                }
            }
            Event::Closed(_) => {}
        }
    }

    /// What a new service does with what the last one wrote down: each
    /// child whose tab the server still has is watched again, and each
    /// whose tab is gone has exited (after a restart of the server every
    /// one has, and keeps its conversation's id to be taken up again).
    async fn reconcile(self: &Arc<Self>) {
        let sessions: HashSet<String> = self.session_ids().await.unwrap_or_default().into_iter().collect();
        let mine: Vec<NativeSubagent> = self.lock().reg.entries.iter().filter(|e| e.exited_at.is_none()).cloned().collect();
        let wanted: HashSet<&str> = mine.iter().map(|e| e.session_id.as_str()).filter(|s| sessions.contains(*s)).collect();
        for session in wanted {
            let _ = self.refresh(session).await;
        }
        for entry in mine {
            let there = self.lock().sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&entry.window_id) && s.blocks.contains_key(&entry.block_id));
            if there && self.attach(&entry.session_id).await.is_ok() {
                self.watch_child(&entry);
            } else {
                self.lock().live.insert(entry.id.clone(), Live::new());
                self.exit(&entry.id, None);
            }
        }
    }

    /// A child begins: written down, watched, said to followers.
    pub async fn begin(self: &Arc<Self>, entry: NativeSubagent) -> Result<(), RexErr> {
        self.attach(&entry.session_id).await?;
        {
            let mut st = self.lock();
            st.reg.register(entry.clone());
            st.live.insert(entry.id.clone(), Live::new());
            st.idle_since = None;
        }
        // (Written down before anybody is told it started: a service
        // killed the moment after knows the tab for its child.)
        self.write_now().await;
        self.tell_entry(&entry.id);
        self.watch_child(&entry);
        Ok(())
    }

    // ---- What is written down ----

    /// Writes the registry whenever there is something of it to write:
    /// one writer, so what is written last is what is newest. (An
    /// event's line is written as it is recorded, `record`.)
    async fn writer(self: Arc<Self>) {
        loop {
            self.save.notified().await;
            self.write_now().await;
        }
    }

    async fn write_now(&self) {
        let reg = self.lock().reg.clone();
        let paths = self.paths.clone();
        let _ = tokio::task::spawn_blocking(move || {
            reg.save_to(&paths.registry);
            // (The log's file kept to the last of it: read whole by the next service.)
            if std::fs::metadata(&paths.events).is_ok_and(|m| m.len() > 16 << 20)
                && let Ok(all) = std::fs::read_to_string(&paths.events)
            {
                let keep: Vec<&str> = all.lines().rev().take(2000).collect();
                let kept: String = keep.into_iter().rev().map(|l| format!("{l}\n")).collect();
                let tmp = paths.events.with_extension("jsonl.tmp");
                if std::fs::write(&tmp, kept).is_ok() {
                    let _ = std::fs::rename(&tmp, &paths.events);
                }
            }
        })
        .await;
    }

    // ---- Those that ask ----

    fn views(&self) -> Vec<AgentView> {
        let st = self.lock();
        st.reg
            .entries
            .iter()
            .map(|entry| {
                let (state, summary) = match (entry.exited_at, st.live.get(&entry.id).and_then(|l| l.told.clone())) {
                    (Some(_), _) => (SubagentLiveState::Exited, None),
                    (None, Some((state, summary))) => (state, summary),
                    (None, None) => (SubagentLiveState::Working, None),
                };
                AgentView { entry: entry.clone(), state, summary }
            })
            .collect()
    }

    /// Children an earlier Olo's desktop kept (it was their keeper then):
    /// taken over once, those whose tabs are still there as live.
    async fn adopt(self: &Arc<Self>, obj: Option<&Value>) -> Response {
        let given: Vec<NativeSubagent> = obj.and_then(|v| v.get("agents")).and_then(|a| serde_json::from_value(a.clone()).ok()).unwrap_or_default();
        let mut taken = 0;
        for mut entry in given {
            entry.host_key = HOST.to_owned();
            if self.lock().reg.entries.iter().any(|e| e.id == entry.id || (e.session_id == entry.session_id && e.window_id == entry.window_id)) {
                continue;
            }
            let _ = self.seen(&entry.session_id).await;
            let there = self.lock().sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&entry.window_id));
            taken += 1;
            if entry.exited_at.is_none() && there {
                if self.begin(entry).await.is_err() {
                    taken -= 1;
                }
            } else {
                if entry.exited_at.is_none() {
                    entry.exited_at = Some(now_unix_ms() as i64);
                }
                self.lock().reg.entries.push(entry.clone());
                self.save.notify_one();
                self.tell_entry(&entry.id);
            }
        }
        Response::ok(format!("Took over {taken} child record(s).\n"), json!({ "ok": true, "adopted": taken }))
    }

    /// One connection: a request, and its answer (or, for `follow`,
    /// everything and then each change until the asker goes).
    async fn serve_one(self: Arc<Self>, stream: UnixStream) {
        let peer_pid = stream.peer_cred().ok().and_then(|c| c.pid());
        // (The account's own alone: the socket's folder is too, and this says so again.)
        if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) {
            return;
        }
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read);
        let mut line = String::new();
        let mut limited = (&mut lines).take(REQUEST_MOST as u64);
        let said = |answer: &Response| serde_json::to_string(answer).unwrap_or_default() + "\n";
        if limited.read_line(&mut line).await.is_err() || !line.ends_with('\n') {
            if line.len() >= REQUEST_MOST {
                let _ = write
                    .write_all(
                        format!("\n{}", said(&Response::err("", format!("a request of more than {} MiB (a long prompt goes in a file the agent is told to read)", REQUEST_MOST >> 20), 2))).as_bytes(),
                    )
                    .await;
            }
            return;
        }
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            let _ = write.write_all(format!("\n{}", said(&Response::err("", "a request that isn't one", 2))).as_bytes()).await;
            return;
        };
        // An asker of a later build: its own service takes over. This
        // one finishes what it is doing, writes down what it knows and
        // ends; the children are the server's and go on. Later by what
        // it speaks, or by its file: a helper put on the host after this
        // one was (an upgrade that speaks the same) is asked through at
        // once, and a service that had no such word ran the old build for
        // as long as anything kept it busy: on a host whose agents never
        // rest, for good (2026-11-26).
        if req.proto > PROTO || peer_pid.is_some_and(asker_is_a_later_build) {
            let _ = write.write_all(format!("\n{}", said(&Response { ok: false, stdout: String::new(), stderr: String::new(), code: 75, data: json!({ "handover": true }) })).as_bytes()).await;
            self.retire.notify_one();
            return;
        }
        if req.op == "follow" {
            // (Asked for before the picture is taken: nothing between the two is missed.)
            let mut frames = self.frames.subscribe();
            let cursor = self.lock().log.latest_seq();
            let first = Frame::Snapshot { proto: PROTO, cursor, agents: self.views() };
            if write.write_all((serde_json::to_string(&first).unwrap_or_default() + "\n").as_bytes()).await.is_err() {
                return;
            }
            let mut gone = String::new();
            loop {
                tokio::select! {
                    frame = frames.recv() => {
                        let frame = match frame {
                            Ok(frame) => frame,
                            // (Fallen behind: everything again.)
                            Err(broadcast::error::RecvError::Lagged(_)) => Frame::Snapshot { proto: PROTO, cursor: self.lock().log.latest_seq(), agents: self.views() },
                            Err(broadcast::error::RecvError::Closed) => return,
                        };
                        if write.write_all((serde_json::to_string(&frame).unwrap_or_default() + "\n").as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    // (The follower went, or closed its side.)
                    n = lines.read_line(&mut gone) => if n.unwrap_or(0) == 0 { return } else { gone.clear() },
                }
            }
        }
        // Taken: said before anything is done (`wire::TAKEN`).
        if write.write_all(b"\n").await.is_err() {
            return;
        }
        let waits = matches!(req.op.as_str(), "watch" | "wait");
        if !waits {
            self.lock().doing += 1;
        }
        // (An asker that goes away while a watch is waited for is let
        // go: nobody waits for it any more. What is done once is done to
        // its end, asker or no asker.)
        let mut gone = String::new();
        let answer = if waits {
            tokio::select! {
                answer = self.answer(&req, peer_pid) => Some(answer),
                _ = async { while lines.read_line(&mut gone).await.unwrap_or(0) > 0 { gone.clear(); } } => None,
            }
        } else {
            let answer = self.answer(&req, peer_pid).await;
            self.lock().doing -= 1;
            self.idle.notify_one();
            Some(answer)
        };
        if let Some(answer) = answer {
            let _ = write.write_all(said(&answer).as_bytes()).await;
            let _ = write.shutdown().await;
        }
    }

    async fn answer(self: &Arc<Self>, req: &Request, peer_pid: Option<i32>) -> Response {
        match req.op.as_str() {
            "ping" => Response::ok(String::new(), json!({ "ok": true, "proto": PROTO, "pid": std::process::id(), "cursor": self.lock().log.latest_seq() })),
            "retire" => {
                self.retire.notify_one();
                Response::ok(String::new(), json!({ "ok": true }))
            }
            "adopt" => self.adopt(req.obj.as_ref()).await,
            "policy" => self.policy(req).await,
            _ => self.agent_op(req, peer_pid).await,
        }
    }

    /// Which child a process (or the pane it names) is, and what that
    /// child may ask of the desktop: what a desktop asks before it does
    /// something for a pane it doesn't know as a child. The process's
    /// ancestors say whose tab it runs in, whatever pane it names.
    async fn policy(self: &Arc<Self>, req: &Request) -> Response {
        let pid = req.obj.as_ref().and_then(|o| o["pid"].as_i64()).map(|p| p as i32).filter(|p| *p > 1);
        let child = self.caller(req, pid).await.child;
        Response::ok(String::new(), json!({ "ok": true, "child": child.as_ref().map(|c| &c.id), "desktop": child.as_ref().map(|c| c.desktop_policy.describe()) }))
    }
}

/// The lock a running service holds: the file is locked for as long as
/// its descriptor lives.
fn take_lock(paths: &Paths) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd as _;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&paths.lock).ok()?;
    // SAFETY: a lock on a descriptor this function owns.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other(format!("{} isn't a folder of this account's", dir.display())));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

fn load_events(paths: &Paths, start_seq: u64) -> SubagentEventLog {
    let events: Vec<SubagentEvent> = std::fs::read_to_string(&paths.events).map(|all| all.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default();
    SubagentEventLog::restored(events, start_seq)
}

/// Runs the service for the Rex server at `server` until it has nothing
/// left to do. 0: ended in order, or another is running; 69: the server
/// couldn't be reached.
pub async fn serve(server: PathBuf, paths: Paths) -> i32 {
    if let Err(e) = private_dir(&paths.dir).and_then(|()| paths.socket.parent().map_or(Ok(()), private_dir)) {
        eprintln!("olo agents: {e}");
        return 1;
    }
    // One service at a time. Another's lock is waited for a moment: it
    // may be on its way out (handing over, or with nothing left to do),
    // and whoever started this one is waiting for a service to answer.
    let waited = Instant::now();
    let _lock = loop {
        if let Some(lock) = take_lock(&paths) {
            break lock;
        }
        // (It serves: this one has nothing to do.)
        if std::os::unix::net::UnixStream::connect(&paths.socket).is_ok() || waited.elapsed() > Duration::from_secs(20) {
            return 0;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (rex, mut events) = match Rex::connect(&server).await {
        Ok(connected) => connected,
        Err(e) => {
            eprintln!("olo agents: the Rex server at {} couldn't be reached: {e}", server.display());
            return 69;
        }
    };
    // (Every session's windows coming and going; a child's own block's
    // events come with following its session.)
    if let Err(e) = rex.call("session.attach_all", json!({ "event_scope": "session" })).await {
        eprintln!("olo agents: the Rex server wouldn't say what happens in its sessions: {e}");
        return 69;
    }
    let reg = SubagentRegistry::load_from(&paths.registry);
    let log = load_events(&paths, reg.event_seq);
    let _ = std::fs::remove_file(&paths.socket);
    let listener = match UnixListener::bind(&paths.socket) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("olo agents: couldn't listen at {}: {e}", paths.socket.display());
            return 1;
        }
    };
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600));
    }
    let (frames, _) = broadcast::channel(1024);
    let service = Arc::new(Service {
        rex,
        paths: paths.clone(),
        st: Mutex::new(State { reg, log, live: HashMap::new(), sessions: HashMap::new(), clients: 0, doing: 0, idle_since: None }),
        changed: Notify::new(),
        frames,
        text_watchers: AtomicUsize::new(0),
        save: Notify::new(),
        idle: Notify::new(),
        retire: Notify::new(),
    });
    tokio::spawn(service.clone().writer());
    service.reconcile().await;

    // What ends it: the server going, being asked to hand over, a
    // signal to end, or nothing left to do for a while.
    let (ended_tx, mut ended) = mpsc::unbounded_channel::<&'static str>();
    {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if let Event::Closed(why) = &event {
                    eprintln!("olo agents: the Rex server's connection ended: {why}");
                    let _ = ended.send("the Rex server went away");
                    return;
                }
                service.on_event(event).await;
            }
            let _ = ended.send("the Rex server went away");
        });
    }
    {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            service.retire.notified().await;
            let _ = ended.send("handed over");
        });
    }
    {
        let ended = ended_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else { return };
            tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            let _ = ended.send("told to end");
        });
    }
    {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            loop {
                let since = {
                    let mut st = service.lock();
                    // (A child that ended and left its tab open isn't work.)
                    let busy = st.clients > 0 || st.reg.entries.iter().any(|e| e.exited_at.is_none());
                    if busy {
                        st.idle_since = None;
                    } else if st.idle_since.is_none() {
                        st.idle_since = Some(Instant::now());
                    }
                    st.idle_since
                };
                match since {
                    // (Busy: nothing to wait for but the word that it no longer is.)
                    None => service.idle.notified().await,
                    Some(since) if since.elapsed() >= IDLE_EXIT => {
                        let _ = ended.send("nothing left to do");
                        return;
                    }
                    Some(since) => {
                        tokio::select! {
                            _ = tokio::time::sleep(IDLE_EXIT.saturating_sub(since.elapsed())) => {}
                            _ = service.idle.notified() => {}
                        }
                    }
                }
            }
        });
    }
    let why = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let service = service.clone();
                tokio::spawn(async move {
                    {
                        let mut st = service.lock();
                        st.clients += 1;
                        st.idle_since = None;
                    }
                    service.clone().serve_one(stream).await;
                    service.lock().clients -= 1;
                    service.idle.notify_one();
                });
            }
            why = ended.recv() => break why.unwrap_or("ended"),
        }
    };
    // No more is taken (its socket goes, so the next asker starts
    // another at once); what is being done is done to its end (a tab
    // being started is written down, a key being typed is typed), a
    // while at most; and what it knows is written before it goes.
    drop(listener);
    let _ = std::fs::remove_file(&paths.socket);
    let finishing = Instant::now();
    while service.lock().doing > 0 && finishing.elapsed() < Duration::from_secs(20) {
        tokio::select! {
            _ = service.idle.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    service.write_now().await;
    eprintln!("olo agents: ending ({why})");
    0
}
