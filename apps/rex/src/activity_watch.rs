//! Live streaming Transcript viewer (`Show Transcript`) and Activity sidebar fold state.
//!
//! Zero background host polling: transcript paths are resolved on demand only
//! when the user explicitly requests `Show Transcript` for a pane, and the tail
//! stream stops the moment the viewer closes.

use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, OnceLock},
};

use olo_rex_client::{
    RexEndpoint,
    activity::{ActivitySnapshot, BodySegment, PaneQuery, SearchMatch, Speaker, TailFrame, ToolStatus, TranscriptDoc, TranscriptEntry, tail_command},
    helper::{Asked, ask as helper_ask, put as helper_put},
};
use olo_sync::Mutex;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::hub::{BlockKey, HostConnState, HostHelper, RexHub};

/// Which alternate view (if any) a Rex window is currently displaying in its
/// center area instead of the normal interactive terminal pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewTarget {
    /// Live streaming transcript for a Rex pane (`Show Transcript`).
    PaneTranscript { bk: BlockKey, title: String, agent: String, path: Option<String> },
}

impl ViewTarget {
    pub fn has_terminal(&self) -> bool {
        match self {
            Self::PaneTranscript { .. } => true,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::PaneTranscript { title, .. } => title.clone(),
        }
    }

    pub fn status_bar_text(&self, _updated: Option<&str>) -> String {
        match self {
            Self::PaneTranscript { title, .. } => format!("{title} · transcript"),
        }
    }
}

/// Live state for a streaming transcript viewer.
pub struct TranscriptSession {
    pub target: ViewTarget,
    pub doc: TranscriptDoc,
    pub path: Option<String>,
    pub loading: bool,
    pub streaming: bool,
    pub error: Option<String>,
    /// Entries manually toggled by the user (for normal rows, means expanded;
    /// for failed/running tool rows which open by default, `collapsed_errors`
    /// and `collapsed_running` record explicit user collapse).
    pub expanded: HashSet<usize>,
    pub collapsed_errors: HashSet<usize>,
    pub collapsed_running: HashSet<usize>,
    /// Single collapsed entry temporarily expanded to reveal the active Find match;
    /// automatically folds back when Find steps to another entry or closes.
    pub find_auto_expanded: Option<usize>,
    pub follow: bool,
    /// Number of new transcript entries received while paused (`!follow`).
    pub paused_unseen: usize,
    /// Last observed vertical scroll offset (`y`, `0` at top down to `-max_y`) and max extent,
    /// persisted across renders so `on_scroll` can tell user scroll apart from layout reflow.
    pub last_scroll_y: f32,
    pub last_max_y: f32,
    /// Entry ID requested to be scrolled into view (e.g., active Find match).
    pub scroll_to_entry: Option<usize>,
    pub search_query: String,
    pub matches: Vec<SearchMatch>,
    pub match_idx: Option<usize>,
    pub generation: u64,
    abort: Option<tokio::task::AbortHandle>,
}

impl TranscriptSession {
    fn new(target: ViewTarget, agent_label: String, path: Option<String>) -> Self {
        Self {
            target,
            doc: TranscriptDoc::new(agent_label),
            path,
            loading: true,
            streaming: false,
            error: None,
            expanded: HashSet::new(),
            collapsed_errors: HashSet::new(),
            collapsed_running: HashSet::new(),
            find_auto_expanded: None,
            follow: true,
            paused_unseen: 0,
            last_scroll_y: 0.0,
            last_max_y: 0.0,
            scroll_to_entry: None,
            search_query: String::new(),
            matches: Vec::new(),
            match_idx: None,
            generation: 1,
            abort: None,
        }
    }

    pub fn last_timestamp(&self) -> Option<&str> {
        self.doc.entries.iter().rev().find_map(|e| {
            let ts = match e {
                TranscriptEntry::Message { timestamp, .. } | TranscriptEntry::Thinking { timestamp, .. } | TranscriptEntry::ToolCall { timestamp, .. } | TranscriptEntry::Divider { timestamp, .. } => {
                    timestamp.as_str()
                }
            };
            (!ts.is_empty()).then_some(ts)
        })
    }

    pub fn stop(&mut self) {
        if let Some(a) = self.abort.take() {
            a.abort();
        }
    }

    pub fn recompute_search(&mut self) {
        self.matches = self.doc.find(&self.search_query);
        if self.matches.is_empty() {
            self.match_idx = None;
            self.find_auto_expanded = None;
            self.scroll_to_entry = None;
        } else {
            let idx = self.match_idx.unwrap_or(0).min(self.matches.len() - 1);
            self.match_idx = Some(idx);
            let hit = &self.matches[idx];
            self.find_auto_expanded = hit.in_collapsed_body.then_some(hit.entry_id);
            self.scroll_to_entry = Some(hit.entry_id);
            self.follow = false;
        }
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.matches.clear();
        self.match_idx = None;
        self.find_auto_expanded = None;
        self.scroll_to_entry = None;
    }

    pub fn step_search(&mut self, back: bool) -> Option<usize> {
        if self.matches.is_empty() {
            self.match_idx = None;
            self.find_auto_expanded = None;
            self.scroll_to_entry = None;
            return None;
        }
        let n = self.matches.len();
        let next = match self.match_idx {
            None => 0,
            Some(i) if back => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
        };
        self.match_idx = Some(next);
        let hit = &self.matches[next];
        self.find_auto_expanded = hit.in_collapsed_body.then_some(hit.entry_id);
        self.scroll_to_entry = Some(hit.entry_id);
        self.follow = false;
        Some(hit.entry_index)
    }

    pub fn agent_json(&self) -> String {
        #[derive(Serialize)]
        struct Snap<'a> {
            title: String,
            path: Option<&'a str>,
            loading: bool,
            streaming: bool,
            error: Option<&'a str>,
            has_terminal: bool,
            follow: bool,
            entries: usize,
            match_count: usize,
            selected_match: Option<usize>,
            recent_texts: Vec<String>,
        }
        let recent_texts = self.doc.entries.iter().rev().take(12).rev().map(|e| e.searchable_text().chars().take(180).collect()).collect();
        let s = Snap {
            title: self.target.title(),
            path: self.path.as_deref(),
            loading: self.loading,
            streaming: self.streaming,
            error: self.error.as_deref(),
            has_terminal: self.target.has_terminal(),
            follow: self.follow,
            entries: self.doc.entries.len(),
            match_count: self.matches.len(),
            selected_match: self.match_idx,
            recent_texts,
        };
        serde_json::to_string(&s).unwrap_or_default()
    }
}

/// Global Transcript viewer & Activity sidebar fold-state coordinator.
#[derive(Default)]
pub struct ActivityManager {
    /// Folded parent blocks in the Activity sidebar (expanded by default).
    pub folded: HashSet<BlockKey>,
    /// Currently open Transcript viewer (if active).
    pub transcript: Option<Arc<Mutex<TranscriptSession>>>,
}

static MGR: OnceLock<Arc<Mutex<ActivityManager>>> = OnceLock::new();

pub fn manager() -> &'static Arc<Mutex<ActivityManager>> {
    MGR.get_or_init(|| Arc::new(Mutex::new(ActivityManager::default())))
}

/// Locate (or extract from the bundled static builds) a local `olo-rex-link`
/// binary so local hosts use the exact same transcript tailing helper as remote hosts.
pub fn local_helper_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("olo-rex-link");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    let arch = std::env::consts::ARCH;
    let build = crate::link::BUILDS.iter().find(|b| !b.bytes.is_empty() && b.machines.contains(&arch))?;
    let dir = olo_xdg::cache_dir().join("rex-link");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("olo-rex-link-{}-{arch}", crate::link::ID));
    if path.is_file() && std::fs::metadata(&path).is_ok_and(|m| m.len() == build.bytes.len() as u64) {
        return Some(path);
    }
    let tmp = dir.join(format!(".new-{}", std::process::id()));
    std::fs::write(&tmp, build.bytes).ok()?;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    if std::fs::rename(&tmp, &path).is_ok() {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&tmp);
        path.is_file().then_some(path)
    }
}

/// Ensure the remote host has this build's `olo-rex-link` installed before invoking it.
async fn ensure_remote_helper(
    state: &Arc<Mutex<crate::hub::HubState>>,
    host_key: &str,
) -> Result<(olo_rex_client::SshTarget, Arc<olo_rex_client::SshSessionPool>, Option<Arc<dyn olo_rex_client::SshAskpass>>), String> {
    let (target, pool, askpass, ready) = {
        let st = state.lock().unwrap_or_else(|e| e.into_inner());
        if !st.settings.host_helper || st.settings.helper_hosts.get(host_key).is_some_and(|w| w == "off") {
            return Err("Olo’s helper is turned off for this host.".into());
        }
        let Some(h) = st.hosts.get(host_key) else { return Err("Host not found.".into()) };
        if !matches!(h.conn_state, HostConnState::Connected) {
            return Err("Host is not connected.".into());
        }
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = h.endpoint.clone() else {
            return Err("Not an SSH host.".into());
        };
        (target, pool, askpass, matches!(h.helper, HostHelper::Used | HostHelper::Ready) && h.helper_current == Some(true))
    };
    if !ready {
        let host_lock = crate::files_sock::helper_host_lock(host_key);
        let _guard = host_lock.lock().await;
        {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.settings.host_helper || st.settings.helper_hosts.get(host_key).is_some_and(|w| w == "off") {
                return Err("Olo’s helper is turned off for this host.".into());
            }
            if st.hosts.get(host_key).is_some_and(|h| matches!(h.helper, HostHelper::Used | HostHelper::Ready) && h.helper_current == Some(true)) {
                return Ok((target, pool, askpass));
            }
        }
        match helper_ask(&pool, &target, askpass.clone(), crate::link::ID, &crate::link::BUILDS).await {
            Ok(Ok(Asked::Here)) => {}
            Ok(Ok(Asked::Needs(build) | Asked::Outdated(build))) => {
                {
                    let st = state.lock().unwrap_or_else(|e| e.into_inner());
                    if !st.settings.host_helper || st.settings.helper_hosts.get(host_key).is_some_and(|w| w == "off") {
                        return Err("Olo’s helper is turned off for this host.".into());
                    }
                }
                helper_put(&pool, &target, askpass.clone(), crate::link::ID, build).await.map_err(|e| e.why)?;
            }
            Ok(Err(kind)) => return Err(format!("No helper build for {kind}")),
            Err(e) => return Err(e.why),
        }
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        if !st.settings.host_helper || st.settings.helper_hosts.get(host_key).is_some_and(|w| w == "off") {
            return Err("Olo’s helper is turned off for this host.".into());
        }
        if let Some(h) = st.hosts.get_mut(host_key) {
            h.helper_current = Some(true);
            h.helper_sync_err = None;
            if !matches!(h.helper, HostHelper::Used) {
                h.helper = HostHelper::Ready;
            }
        }
    }
    Ok((target, pool, askpass))
}

/// Collect the current `PaneQuery` list for a connected host from `HubState`.
fn pane_queries_for_host(hub: &RexHub, host_key: &str) -> (Vec<PaneQuery>, HashMap<String, BlockKey>) {
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(h) = st.hosts.get(host_key) else {
        return (Vec::new(), HashMap::new());
    };
    let mut queries = Vec::new();
    let mut block_map = HashMap::new();
    for s in &h.session_summaries {
        let Some(v) = h.sessions.get(&s.session_id) else { continue };
        for w in &v.windows {
            for b in w.all_blocks() {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                if let Some(br) = st.blocks.get(&bk) {
                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                    let pid = g.process.as_ref().and_then(|p| p.child.as_ref().or(p.foreground.as_ref())).map_or(0, |p| p.pid as i32);
                    if pid > 0 {
                        let (_, rows) = g.terminal.size();
                        let screens = (0..rows.min(48)).map(|r| g.terminal.viewport_row_text(u32::from(r))).filter(|l| l.trim().len() >= 16).take(12).collect();
                        queries.push(PaneQuery { block_id: b.block_id.clone(), pid, cwd: g.pwd.clone(), screens });
                        block_map.insert(b.block_id.clone(), bk);
                    }
                }
            }
        }
    }
    queries.sort_by(|a, b| a.block_id.cmp(&b.block_id));
    (queries, block_map)
}

/// Stop any active transcript stream cleanly on shutdown.
pub fn stop_all() {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tr) = mgr.transcript.take() {
        tr.lock().unwrap_or_else(|e| e.into_inner()).stop();
    }
}

pub fn is_folded(bk: &BlockKey) -> bool {
    manager().lock().unwrap_or_else(|e| e.into_inner()).folded.contains(bk)
}

pub fn toggle_fold(bk: &BlockKey) {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    if !mgr.folded.remove(bk) {
        mgr.folded.insert(bk.clone());
    }
    drop(mgr);
    crate::ui::activity_changed();
}

/// Active transcript viewer target (if any is open).
pub fn active_view_target() -> Option<ViewTarget> {
    let mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    mgr.transcript.as_ref().map(|tr| tr.lock().unwrap_or_else(|e| e.into_inner()).target.clone())
}

pub fn active_transcript() -> Option<Arc<Mutex<TranscriptSession>>> {
    manager().lock().unwrap_or_else(|e| e.into_inner()).transcript.clone()
}

/// Close any active transcript overlay and return to the interactive terminal.
pub fn close_viewer() {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    let taken = mgr.transcript.take();
    drop(mgr);
    if let Some(tr) = taken {
        tr.lock().unwrap_or_else(|e| e.into_inner()).stop();
        crate::ui::activity_changed();
    }
}

fn take_matching_transcript<T>(slot: &mut Option<Arc<T>>, expected: &Arc<T>) -> Option<Arc<T>> {
    if slot.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, expected)) { slot.take() } else { None }
}

/// Close the active transcript viewer ONLY if it is still the exact
/// `expected` session, comparing and removing under a single manager lock so a
/// concurrently opened transcript can never be closed by a late callback.
pub fn close_viewer_if(expected: &Arc<Mutex<TranscriptSession>>) -> bool {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    let taken = take_matching_transcript(&mut mgr.transcript, expected);
    drop(mgr);
    if let Some(tr) = taken {
        tr.lock().unwrap_or_else(|e| e.into_inner()).stop();
        crate::ui::activity_changed();
        true
    } else {
        false
    }
}

/// Open the live streaming transcript view for a Rex pane (`Show Transcript`).
pub fn open_pane_transcript(hub: &Arc<RexHub>, bk: &BlockKey, title: String) {
    if crate::server::demo_mode() {
        open_demo_transcript(bk.clone(), title);
        return;
    }
    let agent = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.subagents
            .by_block(&bk.host_key, Some(&bk.session_id), &bk.block_id)
            .map(|s| s.cli.display_name().to_owned())
            .or_else(|| st.blocks.get(bk).and_then(|br| br.lock().unwrap_or_else(|e| e.into_inner()).program_title()))
            .unwrap_or_else(|| "Agent".into())
    };
    let target = ViewTarget::PaneTranscript { bk: bk.clone(), title, agent: agent.clone(), path: None };
    start_transcript_stream(hub, bk.clone(), target, agent, None);
}

fn start_transcript_stream(hub: &Arc<RexHub>, bk: BlockKey, target: ViewTarget, agent: String, known_path: Option<String>) {
    close_viewer();
    let session = Arc::new(Mutex::new(TranscriptSession::new(target, agent, known_path.clone())));
    {
        let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
        mgr.transcript = Some(session.clone());
    }
    crate::ui::activity_changed();

    let is_local = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.hosts.get(&bk.host_key).is_some_and(|h| h.is_local)
    };
    let hub_clone = hub.clone();
    let sess_clone = session.clone();
    let abort = hub
        .rt
        .spawn(async move {
            let path = match known_path {
                Some(p) => Some(p),
                None => resolve_pane_transcript_once(&hub_clone, &bk, is_local).await,
            };
            let Some(path) = path else {
                let mut g = sess_clone.lock().unwrap_or_else(|e| e.into_inner());
                g.loading = false;
                g.streaming = false;
                g.error = Some("No transcript file found for this agent yet.".into());
                g.generation += 1;
                drop(g);
                crate::ui::activity_changed();
                return;
            };
            {
                let mut g = sess_clone.lock().unwrap_or_else(|e| e.into_inner());
                g.path = Some(path.clone());
                g.generation += 1;
            }
            crate::ui::activity_changed();

            if is_local {
                let Some(helper) = local_helper_path() else {
                    set_transcript_err(&sess_clone, "Local olo-rex-link helper unavailable.");
                    return;
                };
                let Ok(mut child) =
                    tokio::process::Command::new(&helper).args(["activity", "tail", &path]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn()
                else {
                    set_transcript_err(&sess_clone, "Couldn’t start transcript stream.");
                    return;
                };
                let _keep_stdin = child.stdin.take();
                let Some(stdout) = child.stdout.take() else { return };
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    apply_tail_frame(&sess_clone, &line);
                }
                let _ = child.kill().await;
            } else {
                let Ok((target, pool, askpass)) = ensure_remote_helper(&hub_clone.state, &bk.host_key).await else {
                    set_transcript_err(&sess_clone, "Host helper is not ready.");
                    return;
                };
                let cmd = tail_command(crate::link::ID, &path);
                let Ok(stream) = pool.exec_io(&target, askpass, &cmd).await else {
                    set_transcript_err(&sess_clone, "Couldn’t stream transcript from host.");
                    return;
                };
                let (read_half, _write_half) = tokio::io::split(stream);
                let mut lines = BufReader::new(read_half).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    apply_tail_frame(&sess_clone, &line);
                }
            }
            let mut g = sess_clone.lock().unwrap_or_else(|e| e.into_inner());
            g.loading = false;
            g.streaming = false;
            g.generation += 1;
            drop(g);
            crate::ui::activity_changed();
        })
        .abort_handle();
    session.lock().unwrap_or_else(|e| e.into_inner()).abort = Some(abort);
}

/// Runs `olo-rex-link <args...>` on `host_key` (locally or over SSH via the
/// deployed remote helper), piping optional `stdin_payload` and returning stdout.
pub async fn run_host_link(state: &Arc<Mutex<crate::hub::HubState>>, host_key: &str, args: &[&str], stdin_payload: Option<&str>) -> Option<String> {
    run_host_link_within(state, host_key, args, stdin_payload, std::time::Duration::from_secs(10)).await
}

/// [`run_host_link`], waited for as long as `within` (a host's agents
/// service asked something that takes its time: a watch, a stop that
/// saves a long scrollback).
/// Whether Olo's helper may be run on a host at all: never by the
/// demo (`OLO_REX_DEMO`), whose hosts and children are made up. Its
/// "This computer" is the computer it runs on, and what it asked of its
/// made-up children went to that computer's real agents service: a stop
/// asked from a picture's script reached a host where four agents were
/// hours into their work, and the asking itself took the service from
/// them (2026-11-26).
fn may_ask_hosts() -> bool {
    !crate::server::demo_mode()
}

pub async fn run_host_link_within(state: &Arc<Mutex<crate::hub::HubState>>, host_key: &str, args: &[&str], stdin_payload: Option<&str>, within: std::time::Duration) -> Option<String> {
    if !may_ask_hosts() {
        return None;
    }
    let is_local = state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).is_some_and(|h| h.is_local);
    if is_local {
        let helper = local_helper_path()?;
        let mut child = tokio::process::Command::new(&helper).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(payload) = stdin_payload {
                let _ = stdin.write_all(payload.as_bytes()).await;
            }
            let _ = stdin.shutdown().await;
        }
        let out = tokio::time::timeout(within, child.wait_with_output()).await.ok()?.ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let (target, pool, askpass) = ensure_remote_helper(state, host_key).await.ok()?;
        let mut cmd = olo_rex_client::helper::file(crate::link::ID);
        for a in args {
            cmd.push(' ');
            cmd.push_str(&shell_escape_arg(a));
        }
        let mut stream = pool.exec_io(&target, askpass, &cmd).await.ok()?;
        if let Some(payload) = stdin_payload {
            stream.write_all(payload.as_bytes()).await.ok()?;
        }
        let _ = stream.shutdown().await;
        let mut buf = String::new();
        let _ = tokio::time::timeout(within, stream.read_to_string(&mut buf)).await;
        Some(buf)
    }
}

/// Runs `olo-rex-link <args...>` on `host_key` for as long as it says
/// things, giving `said` each line it writes (a host's agents service
/// followed: `agent follow`). Its input is held open meanwhile and
/// closed when this is let go of, which ends it there. Whether it said
/// anything at all.
pub async fn follow_host_link(state: &Arc<Mutex<crate::hub::HubState>>, host_key: &str, args: &[&str], mut said: impl FnMut(&str)) -> bool {
    if !may_ask_hosts() {
        return false;
    }
    let is_local = state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).is_some_and(|h| h.is_local);
    let mut any = false;
    if is_local {
        let Some(helper) = local_helper_path() else { return false };
        let Ok(mut child) = tokio::process::Command::new(&helper).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn() else { return false };
        let _held_open = child.stdin.take();
        let Some(stdout) = child.stdout.take() else { return false };
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            any = true;
            said(&line);
        }
        let _ = child.kill().await;
    } else {
        let Ok((target, pool, askpass)) = ensure_remote_helper(state, host_key).await else { return false };
        let mut cmd = olo_rex_client::helper::file(crate::link::ID);
        for a in args {
            cmd.push(' ');
            cmd.push_str(&shell_escape_arg(a));
        }
        let Ok(stream) = pool.exec_io(&target, askpass, &cmd).await else { return false };
        let (read_half, _held_open) = tokio::io::split(stream);
        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            any = true;
            said(&line);
        }
    }
    any
}

fn shell_escape_arg(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b'+')) {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

async fn resolve_pane_transcript_once(hub: &Arc<RexHub>, bk: &BlockKey, is_local: bool) -> Option<String> {
    let (queries, _) = pane_queries_for_host(hub, &bk.host_key);
    let one: Vec<PaneQuery> = queries.into_iter().filter(|q| q.block_id == bk.block_id).collect();
    if one.is_empty() {
        return None;
    }
    let payload = format!("{}\n", serde_json::json!({"panes": one}));
    let output = if is_local {
        let helper = local_helper_path()?;
        let mut child = tokio::process::Command::new(&helper).args(["activity", "once"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let out = child.wait_with_output().await.ok()?;
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        let (target, pool, askpass) = ensure_remote_helper(&hub.state, &bk.host_key).await.ok()?;
        let cmd = format!("{} activity once", olo_rex_client::helper::file(crate::link::ID));
        let mut stream = pool.exec_io(&target, askpass, &cmd).await.ok()?;
        stream.write_all(payload.as_bytes()).await.ok()?;
        let _ = stream.shutdown().await;
        let mut buf = String::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read_to_string(&mut buf)).await;
        buf
    };
    let snap: ActivitySnapshot = serde_json::from_str(output.trim()).ok()?;
    snap.panes.into_iter().find(|p| p.block_id == bk.block_id).and_then(|p| p.transcript)
}

fn set_transcript_err(sess: &Arc<Mutex<TranscriptSession>>, msg: &str) {
    let mut g = sess.lock().unwrap_or_else(|e| e.into_inner());
    g.loading = false;
    g.streaming = false;
    g.error = Some(msg.to_owned());
    g.generation += 1;
    drop(g);
    crate::ui::activity_changed();
}

fn apply_tail_frame(sess: &Arc<Mutex<TranscriptSession>>, line: &str) {
    let Ok(frame) = serde_json::from_str::<TailFrame>(line.trim()) else { return };
    let mut g = sess.lock().unwrap_or_else(|e| e.into_inner());
    match frame {
        TailFrame::Meta { truncated, waiting, .. } => {
            g.doc.truncated = truncated;
            g.loading = waiting;
            g.streaming = true;
            g.error = None;
        }
        TailFrame::Lines { lines } => {
            g.loading = false;
            g.streaming = true;
            let before = g.doc.entries.len();
            for l in &lines {
                g.doc.push_line(l);
            }
            let added = g.doc.entries.len().saturating_sub(before);
            if !g.follow && added > 0 {
                g.paused_unseen = g.paused_unseen.saturating_add(added);
            }
            if !g.search_query.is_empty() {
                g.recompute_search();
            }
        }
        TailFrame::Reset => {
            g.doc.clear();
            g.expanded.clear();
            g.collapsed_errors.clear();
            g.collapsed_running.clear();
            g.paused_unseen = 0;
            g.matches.clear();
            g.match_idx = None;
        }
        TailFrame::Error { error } => {
            g.loading = false;
            g.streaming = false;
            g.error = Some(error);
        }
    }
    g.generation += 1;
    drop(g);
    crate::ui::activity_changed();
}

// ---------------------------------------------------------------------------
// Deterministic demo fixtures for headless visual QA (`OLO_REX_DEMO_ACTIVITY`)
// ---------------------------------------------------------------------------

fn demo_transcript_doc(agent: &str) -> TranscriptDoc {
    let mut doc = TranscriptDoc::new(agent);
    doc.model = "claude-opus-5-5".into();
    doc.entries = vec![
        TranscriptEntry::Message {
            id: 1,
            speaker: Speaker::User,
            label: "You".into(),
            timestamp: "14:52:10".into(),
            segments: vec![BodySegment::Text(
                "Launch native Rex child tabs for the API audit, schema diff, and unit test suite.".into(),
            )],
        },
        TranscriptEntry::Thinking {
            id: 2,
            summary: "Spawn child tabs via `olo agent start` with explicit model and effort so each subagent stays interactive…".into(),
            text: "Spawn child tabs via `olo agent start` with explicit `--model` and `--effort` so each subagent runs in its own interactive Rex terminal tab under the parent tab.".into(),
            timestamp: "14:52:12".into(),
        },
        TranscriptEntry::ToolCall {
            id: 3,
            call_id: Some("tu_1".into()),
            name: "Bash".into(),
            summary: "olo agent start claude --model claude-opus-5-5 --effort max --title 'API Audit'".into(),
            input: "{\n  \"command\": \"olo agent start claude --model claude-opus-5-5 --effort max --title 'API Audit'\"\n}".into(),
            output: Some("{\"id\":\"sa-audit1\",\"window_id\":\"win-sub-1\",\"cli\":\"claude\",\"model\":\"claude-opus-5-5\",\"effort\":\"max\"}".into()),
            status: ToolStatus::Ok,
            timestamp: "14:52:13".into(),
        },
        TranscriptEntry::ToolCall {
            id: 4,
            call_id: Some("tu_edit".into()),
            name: "Edit".into(),
            summary: "subagents.rs".into(),
            input: "{\n  \"file_path\": \"apps/rex/src/subagents.rs\",\n  \"old_string\": \"let effort = spec.effort.unwrap_or_default();\",\n  \"new_string\": \"let effort = spec.effort.ok_or_else(|| \\\"--effort is required\\\")?;\"\n}".into(),
            output: Some("Updated apps/rex/src/subagents.rs".into()),
            status: ToolStatus::Ok,
            timestamp: "14:52:14".into(),
        },
        TranscriptEntry::ToolCall {
            id: 5,
            call_id: Some("tu_2".into()),
            name: "Bash".into(),
            summary: "jato test -p olo-rex subagents".into(),
            input: "{\n  \"command\": \"jato test -p olo-rex subagents\"\n}".into(),
            output: Some("running 4 tests\ntest subagents::tests::requires_explicit_model_and_effort ... ok\ntest subagents::tests::nested_tree_and_reparenting ... ok\n\ntest result: ok. 4 passed; 0 failed".into()),
            status: ToolStatus::Ok,
            timestamp: "14:52:16".into(),
        },
        TranscriptEntry::Message {
            id: 6,
            speaker: Speaker::Assistant,
            label: agent.into(),
            timestamp: "14:52:18".into(),
            segments: vec![
                BodySegment::Text("## Summary\n- Every subagent launches in a native Rex child tab with explicit `--model` and `--effort`.\n- Grandchildren nest cleanly under their parent in both the tab-tree popover and Activity sidebar.".into()),
            ],
        },
    ];
    doc
}

/// Apply optional `OLO_REX_DEMO_ACTIVITY` QA states (collapsed tree, transcript viewer, Find).
pub fn apply_demo_activity_mode(mode: &str) {
    let demo_bk = BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-agent".into() };
    match mode {
        "collapsed" => {
            manager().lock().unwrap_or_else(|e| e.into_inner()).folded.insert(demo_bk);
        }
        "transcript" => {
            open_demo_transcript(demo_bk, "Olo workspace".into());
            if let Some(tr) = active_transcript() {
                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                g.follow = false;
                g.paused_unseen = 2;
                g.expanded.insert(4);
            }
        }
        "follow" => {
            open_demo_transcript(demo_bk, "Olo workspace".into());
        }
        "find" => {
            open_demo_transcript(demo_bk, "Olo workspace".into());
            if let Some(tr) = active_transcript() {
                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                g.search_query = "effort".into();
                g.recompute_search();
                g.expanded.insert(4);
            }
        }
        "nomatch" => {
            open_demo_transcript(demo_bk, "Olo workspace".into());
            if let Some(tr) = active_transcript() {
                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                g.search_query = "xyznomatch".into();
                g.recompute_search();
            }
        }
        _ => {}
    }
}

fn open_demo_transcript(bk: BlockKey, title: String) {
    close_viewer();
    let target = ViewTarget::PaneTranscript { bk, title, agent: "Claude Code".into(), path: Some("/home/you/.claude/projects/-home-you-olo/11111111.jsonl".into()) };
    let mut sess = TranscriptSession::new(target, "Claude Code".into(), Some("/home/you/.claude/projects/-home-you-olo/11111111.jsonl".into()));
    sess.doc = demo_transcript_doc("Claude Code");
    sess.loading = false;
    sess.streaming = true;
    manager().lock().unwrap_or_else(|e| e.into_inner()).transcript = Some(Arc::new(Mutex::new(sess)));
    crate::ui::activity_changed();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_viewer_if_atomically_closes_only_matching_transcript() {
        let tr_a = Arc::new(1u8);
        let tr_b = Arc::new(2u8);

        // Matching transcript is removed and returned under one lock.
        let mut slot = Some(tr_a.clone());
        let taken = take_matching_transcript(&mut slot, &tr_a);
        assert!(taken.as_ref().is_some_and(|t| Arc::ptr_eq(t, &tr_a)));
        assert!(slot.is_none());

        // A concurrently replaced transcript (tr_B) is left untouched.
        let mut slot = Some(tr_b.clone());
        assert!(take_matching_transcript(&mut slot, &tr_a).is_none());
        assert!(slot.as_ref().is_some_and(|t| Arc::ptr_eq(t, &tr_b)));

        // Empty slot returns None cleanly.
        let mut empty: Option<Arc<u8>> = None;
        assert!(take_matching_transcript(&mut empty, &tr_a).is_none());

        // End-to-end check on the real global manager lock with real TranscriptSessions.
        let bk = BlockKey { host_key: "local".into(), session_id: "s-close-test".into(), block_id: "b-1".into() };
        let sess_a = Arc::new(Mutex::new(TranscriptSession::new(ViewTarget::PaneTranscript { bk: bk.clone(), title: "A".into(), agent: "Claude".into(), path: None }, "Claude".into(), None)));
        let sess_b = Arc::new(Mutex::new(TranscriptSession::new(ViewTarget::PaneTranscript { bk, title: "B".into(), agent: "Claude".into(), path: None }, "Claude".into(), None)));
        {
            let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
            mgr.transcript = Some(sess_b.clone());
        }
        // Late callback for sess_a MUST NOT close sess_b.
        assert!(!close_viewer_if(&sess_a));
        assert!(active_transcript().is_some_and(|cur| Arc::ptr_eq(&cur, &sess_b)));
        // Callback for sess_b closes and removes sess_b.
        assert!(close_viewer_if(&sess_b));
        assert!(active_transcript().is_none());
    }
}
