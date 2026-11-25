//! Host Activity watcher, nested subagent discovery, live streaming transcript
//! tailing, and safe read-only `tmux attach -r -f ignore-size` viewer.
//!
//! Zero background work while Activity isn't shown: `sync_watchers` starts
//! `olo-rex-link activity watch` for each connected host only while at least
//! one window displays the Activity sidebar, and closes stdin (exiting the
//! helper cleanly) the moment Activity is hidden.

use std::{
    collections::{HashMap, HashSet},
    io::Read as _,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, OnceLock},
};

use olo_rex_client::{
    RexEndpoint,
    activity::{
        ActivitySnapshot, BodySegment, PaneActivity, PaneQuery, SearchMatch, Speaker, SubagentInfo, SubagentOrigin, SubagentState, TailFrame, TmuxTarget, ToolStatus, TranscriptDoc, TranscriptEntry,
        tail_command, tmux_attach_command, watch_command,
    },
    helper::{Asked, ask as helper_ask, put as helper_put},
};
use olo_rex_vt::{GhosttyTerminal, RenderState, TerminalTheme};
use olo_sync::Mutex;
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};

use crate::hub::{BlockKey, HostConnState, HostHelper, RexHub};

/// Which alternate view (if any) a Rex window is currently displaying in its
/// center area instead of the normal interactive terminal pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewTarget {
    /// Live streaming transcript for a top-level Rex pane (`Show Transcript`).
    PaneTranscript { bk: BlockKey, title: String, agent: String, path: Option<String> },
    /// Live streaming transcript for a nested subagent.
    SubagentTranscript { parent: BlockKey, sub: SubagentInfo },
    /// Safe read-only `tmux attach -r -f ignore-size` for a tmux-launched subagent.
    SubagentTmux { parent: BlockKey, sub: SubagentInfo, target: TmuxTarget },
}

impl ViewTarget {
    pub fn has_terminal(&self) -> bool {
        match self {
            Self::PaneTranscript { .. } => true,
            Self::SubagentTranscript { sub, .. } | Self::SubagentTmux { sub, .. } => sub.tmux.is_some(),
        }
    }

    pub fn has_transcript(&self) -> bool {
        match self {
            Self::PaneTranscript { .. } => true,
            Self::SubagentTranscript { sub, .. } | Self::SubagentTmux { sub, .. } => sub.transcript.is_some(),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::PaneTranscript { title, .. } => title.clone(),
            Self::SubagentTranscript { sub, .. } | Self::SubagentTmux { sub, .. } => {
                if !sub.description.is_empty() && sub.description != sub.name {
                    format!("{} — {}", sub.name, sub.description)
                } else {
                    sub.name.clone()
                }
            }
        }
    }

    pub fn status_bar_text(&self, updated: Option<&str>) -> String {
        match self {
            Self::PaneTranscript { title, .. } => match updated {
                Some(ts) if !ts.is_empty() => format!("{title} · transcript · updated {ts}"),
                _ => format!("{title} · transcript"),
            },
            Self::SubagentTranscript { sub, .. } => {
                let kind = match sub.origin {
                    SubagentOrigin::Tmux => "tmux",
                    SubagentOrigin::Native => "subagent",
                    SubagentOrigin::Shell => "shell",
                };
                match updated {
                    Some(ts) if !ts.is_empty() => format!("{} · {kind} · transcript · updated {ts}", sub.name),
                    _ => format!("{} · {kind} · transcript", sub.name),
                }
            }
            Self::SubagentTmux { sub, target, .. } => {
                format!("{} · tmux {} × {} · read-only", sub.name, target.cols, target.rows)
            }
        }
    }

    pub fn subagent_id(&self) -> Option<&str> {
        match self {
            Self::PaneTranscript { .. } => None,
            Self::SubagentTranscript { sub, .. } | Self::SubagentTmux { sub, .. } => Some(&sub.id),
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
    /// for failed tool rows which open by default, `collapsed_errors` records user collapse).
    pub expanded: HashSet<usize>,
    pub collapsed_errors: HashSet<usize>,
    /// Single collapsed entry temporarily expanded to reveal the active Find match;
    /// automatically folds back when Find steps to another entry or closes.
    pub find_auto_expanded: Option<usize>,
    pub follow: bool,
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
            find_auto_expanded: None,
            follow: true,
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
        } else {
            let idx = self.match_idx.unwrap_or(0).min(self.matches.len() - 1);
            self.match_idx = Some(idx);
            let hit = &self.matches[idx];
            self.find_auto_expanded = hit.in_collapsed_body.then_some(hit.entry_id);
            self.follow = false;
        }
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.matches.clear();
        self.match_idx = None;
        self.find_auto_expanded = None;
    }

    pub fn step_search(&mut self, back: bool) -> Option<usize> {
        if self.matches.is_empty() {
            self.match_idx = None;
            self.find_auto_expanded = None;
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

/// Live state for a safe read-only `tmux attach -r -f ignore-size` viewer.
pub struct TmuxViewer {
    pub target: ViewTarget,
    pub terminal: GhosttyTerminal,
    pub render_state: RenderState,
    pub frame: crate::frame::Frame,
    pub current_theme: TerminalTheme,
    pub status: String,
    pub error: Option<String>,
    pub generation: u64,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    abort: Option<tokio::task::AbortHandle>,
}

impl TmuxViewer {
    fn new(target: ViewTarget, tmux: TmuxTarget, theme: &TerminalTheme) -> Result<Self, String> {
        let (cols, rows) = tmux.attach_size();
        let mut terminal = GhosttyTerminal::new(cols, rows).map_err(|e| format!("terminal init: {e:?}"))?;
        let _ = terminal.apply_theme(theme);
        let render_state = RenderState::new().map_err(|e| format!("render state: {e:?}"))?;
        Ok(Self {
            target,
            terminal,
            render_state,
            frame: crate::frame::Frame::new(),
            current_theme: theme.clone(),
            status: "Attaching read-only…".into(),
            error: None,
            generation: 1,
            stop_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            abort: None,
        })
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(a) = self.abort.take() {
            a.abort();
        }
    }
}

impl Drop for TmuxViewer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct HostWatcherHandle {
    tx: mpsc::UnboundedSender<String>,
    last_panes: Vec<PaneQuery>,
    abort: tokio::task::AbortHandle,
}

impl Drop for HostWatcherHandle {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// Global Activity discovery & viewer coordinator.
pub struct ActivityManager {
    watchers: HashMap<String, HostWatcherHandle>,
    /// Latest discovered metadata & nested subagents per top-level Rex `BlockKey`.
    pub by_block: HashMap<BlockKey, PaneActivity>,
    /// Folded parent blocks in the Activity sidebar (expanded by default).
    pub folded: HashSet<BlockKey>,
    /// Currently open Transcript viewer (if active).
    pub transcript: Option<Arc<Mutex<TranscriptSession>>>,
    /// Currently open read-only Tmux viewer (if active).
    pub tmux: Option<Arc<Mutex<TmuxViewer>>>,
    /// Demo mode populated flag.
    demo_loaded: bool,
}

impl Default for ActivityManager {
    fn default() -> Self {
        Self { watchers: HashMap::new(), by_block: HashMap::new(), folded: HashSet::new(), transcript: None, tmux: None, demo_loaded: false }
    }
}

static MGR: OnceLock<Arc<Mutex<ActivityManager>>> = OnceLock::new();

pub fn manager() -> &'static Arc<Mutex<ActivityManager>> {
    MGR.get_or_init(|| Arc::new(Mutex::new(ActivityManager::default())))
}

/// Locate (or extract from the bundled static builds) a local `olo-rex-link`
/// binary so local hosts run the exact same zero-polling `activity` helper as remote hosts.
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
async fn ensure_remote_helper(hub: &Arc<RexHub>, host_key: &str) -> Result<(olo_rex_client::SshTarget, Arc<olo_rex_client::SshSessionPool>, Option<Arc<dyn olo_rex_client::SshAskpass>>), String> {
    let (target, pool, askpass, ready) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get(host_key) else { return Err("Host not found.".into()) };
        if !matches!(h.conn_state, HostConnState::Connected) {
            return Err("Host is not connected.".into());
        }
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = h.endpoint.clone() else {
            return Err("Not an SSH host.".into());
        };
        (target, pool, askpass, matches!(h.helper, HostHelper::Used | HostHelper::Ready))
    };
    if !ready {
        match helper_ask(&pool, &target, askpass.clone(), crate::link::ID, &crate::link::BUILDS).await {
            Ok(Ok(Asked::Here)) => {}
            Ok(Ok(Asked::Needs(build))) => {
                helper_put(&pool, &target, askpass.clone(), crate::link::ID, build).await.map_err(|e| e.why)?;
            }
            Ok(Err(kind)) => return Err(format!("No helper build for {kind}")),
            Err(e) => return Err(e.why),
        }
        let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = st.hosts.get_mut(host_key)
            && !matches!(h.helper, HostHelper::Used)
        {
            h.helper = HostHelper::Ready;
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

/// Start/update host watchers while Activity is visible; stop ALL watchers
/// immediately when Activity is hidden (`active == false`).
pub fn sync_watchers(hub: &Arc<RexHub>, active: bool) {
    if crate::server::demo_mode() {
        ensure_demo_loaded(hub);
        return;
    }
    if !active {
        let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
        if !mgr.watchers.is_empty() {
            mgr.watchers.clear();
        }
        return;
    }

    let connected_hosts: Vec<(String, bool)> = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.host_order.iter().filter_map(|hk| st.hosts.get(hk)).filter(|h| matches!(h.conn_state, HostConnState::Connected)).map(|h| (h.key.clone(), h.is_local)).collect()
    };
    let live_keys: HashSet<String> = connected_hosts.iter().map(|(k, _)| k.clone()).collect();

    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    mgr.watchers.retain(|k, _| live_keys.contains(k));
    drop(mgr);

    for (hk, is_local) in connected_hosts {
        let (queries, _) = pane_queries_for_host(hub, &hk);
        let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(handle) = mgr.watchers.get_mut(&hk) {
            if handle.last_panes != queries {
                handle.last_panes = queries.clone();
                if let Ok(line) = serde_json::to_string(&serde_json::json!({"panes": queries})) {
                    let _ = handle.tx.send(format!("{line}\n"));
                }
            }
            continue;
        }
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        if let Ok(line) = serde_json::to_string(&serde_json::json!({"panes": queries})) {
            let _ = tx.send(format!("{line}\n"));
        }
        let hub_clone = hub.clone();
        let hk_clone = hk.clone();
        let abort = hub
            .rt
            .spawn(async move {
                run_host_watcher(hub_clone, hk_clone, is_local, rx).await;
            })
            .abort_handle();
        mgr.watchers.insert(hk, HostWatcherHandle { tx, last_panes: queries, abort });
    }
}

/// Nudge an active host watcher to rescan immediately (e.g. when Rex receives a
/// pane process-change event while Activity is visible). No-op when Activity is hidden.
pub fn nudge_host(host_key: &str) {
    let mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = mgr.watchers.get(host_key) {
        let _ = h.tx.send("{\"refresh\":true}\n".to_owned());
    }
}

/// Stop all watchers and viewers cleanly on shutdown.
pub fn stop_all() {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    mgr.watchers.clear();
    if let Some(tr) = mgr.transcript.take() {
        tr.lock().unwrap_or_else(|e| e.into_inner()).stop();
    }
    if let Some(tm) = mgr.tmux.take() {
        tm.lock().unwrap_or_else(|e| e.into_inner()).stop();
    }
}

async fn run_host_watcher(hub: Arc<RexHub>, host_key: String, is_local: bool, mut rx: mpsc::UnboundedReceiver<String>) {
    if is_local {
        let Some(helper) = local_helper_path() else { return };
        let Ok(mut child) = tokio::process::Command::new(&helper).args(["activity", "watch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn() else {
            return;
        };
        let Some(mut stdin) = child.stdin.take() else { return };
        let Some(stdout) = child.stdout.take() else { return };
        let writer = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            apply_watcher_line(&hub, &host_key, &line);
        }
        writer.abort();
        let _ = child.kill().await;
    } else {
        let Ok((target, pool, askpass)) = ensure_remote_helper(&hub, &host_key).await else { return };
        let cmd = watch_command(crate::link::ID);
        let Ok(stream) = pool.exec_io(&target, askpass, &cmd).await else { return };
        let (read_half, mut write_half) = tokio::io::split(stream);
        let writer = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if write_half.write_all(line.as_bytes()).await.is_err() || write_half.flush().await.is_err() {
                    break;
                }
            }
        });
        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            apply_watcher_line(&hub, &host_key, &line);
        }
        writer.abort();
    }
    // Remove stale watcher handle so next `sync_watchers` can restart if still active.
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    mgr.watchers.remove(&host_key);
}

fn apply_watcher_line(hub: &RexHub, host_key: &str, line: &str) {
    let Ok(snap) = serde_json::from_str::<ActivitySnapshot>(line.trim()) else { return };
    if snap.ready && snap.panes.is_empty() {
        return;
    }
    let (_, block_map) = pane_queries_for_host(hub, host_key);
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    let mut changed = false;
    let mut seen: HashSet<BlockKey> = HashSet::new();
    for p in snap.panes {
        if let Some(bk) = block_map.get(&p.block_id).cloned() {
            seen.insert(bk.clone());
            if mgr.by_block.get(&bk) != Some(&p) {
                mgr.by_block.insert(bk, p);
                changed = true;
            }
        }
    }
    let before = mgr.by_block.len();
    mgr.by_block.retain(|bk, _| bk.host_key != host_key || seen.contains(bk));
    if mgr.by_block.len() != before {
        changed = true;
    }
    drop(mgr);
    if changed {
        crate::ui::activity_changed();
    }
}

// ---------------------------------------------------------------------------
// Queries for UI & Agent API
// ---------------------------------------------------------------------------

pub fn pane_activity(bk: &BlockKey) -> Option<PaneActivity> {
    manager().lock().unwrap_or_else(|e| e.into_inner()).by_block.get(bk).cloned()
}

pub fn subagents_for(bk: &BlockKey) -> Vec<SubagentInfo> {
    manager().lock().unwrap_or_else(|e| e.into_inner()).by_block.get(bk).map(|p| p.subagents.clone()).unwrap_or_default()
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

/// Active viewer target (if any transcript or read-only tmux viewer is open).
pub fn active_view_target() -> Option<ViewTarget> {
    let mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tr) = &mgr.transcript {
        return Some(tr.lock().unwrap_or_else(|e| e.into_inner()).target.clone());
    }
    if let Some(tm) = &mgr.tmux {
        return Some(tm.lock().unwrap_or_else(|e| e.into_inner()).target.clone());
    }
    None
}

pub fn active_transcript() -> Option<Arc<Mutex<TranscriptSession>>> {
    manager().lock().unwrap_or_else(|e| e.into_inner()).transcript.clone()
}

pub fn active_tmux() -> Option<Arc<Mutex<TmuxViewer>>> {
    manager().lock().unwrap_or_else(|e| e.into_inner()).tmux.clone()
}

/// Close any active transcript or read-only tmux overlay and return to the interactive terminal.
pub fn close_viewer() {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    let mut had = false;
    if let Some(tr) = mgr.transcript.take() {
        tr.lock().unwrap_or_else(|e| e.into_inner()).stop();
        had = true;
    }
    if let Some(tm) = mgr.tmux.take() {
        tm.lock().unwrap_or_else(|e| e.into_inner()).stop();
        had = true;
    }
    drop(mgr);
    if had {
        crate::ui::activity_changed();
    }
}

/// Structured JSON summary of all discovered panes and nested subagents for agents.
pub fn activity_agent_json() -> String {
    #[derive(Serialize)]
    struct Row<'a> {
        host: &'a str,
        session_id: &'a str,
        block_id: &'a str,
        agent: Option<&'a str>,
        transcript: Option<&'a str>,
        subagents: &'a [SubagentInfo],
    }
    let mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    let mut rows: Vec<Row<'_>> = mgr
        .by_block
        .iter()
        .map(|(bk, pa)| Row { host: &bk.host_key, session_id: &bk.session_id, block_id: &bk.block_id, agent: pa.agent.as_deref(), transcript: pa.transcript.as_deref(), subagents: &pa.subagents })
        .collect();
    rows.sort_by(|a, b| a.host.cmp(b.host).then_with(|| a.block_id.cmp(b.block_id)));
    serde_json::to_string(&rows).unwrap_or_default()
}

/// Find a discovered subagent by its id across all hosts/blocks.
pub fn find_subagent_by_id(id: &str) -> Option<(BlockKey, SubagentInfo)> {
    let mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    for (bk, pa) in &mgr.by_block {
        if let Some(sub) = pa.subagents.iter().find(|s| s.id == id || s.name == id) {
            return Some((bk.clone(), sub.clone()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Opening Transcript View or Read-Only Tmux View
// ---------------------------------------------------------------------------

/// Open the live streaming transcript view for a top-level Rex pane (`Show Transcript`).
pub fn open_pane_transcript(hub: &Arc<RexHub>, bk: &BlockKey, title: String) {
    if crate::server::demo_mode() {
        open_demo_transcript(bk.clone(), title, true);
        return;
    }
    let known = pane_activity(bk);
    let agent = known.as_ref().and_then(|p| p.agent.clone()).unwrap_or_else(|| "Agent".into());
    let path = known.as_ref().and_then(|p| p.transcript.clone());
    let target = ViewTarget::PaneTranscript { bk: bk.clone(), title, agent: agent.clone(), path: path.clone() };
    start_transcript_stream(hub, bk.clone(), target, agent, path);
}

/// Open a nested subagent: prefers safe read-only tmux attach when `sub.tmux.is_some()`
/// (unless `force_transcript` is true), otherwise opens its live streaming transcript!
pub fn open_subagent(hub: &Arc<RexHub>, parent: &BlockKey, sub: SubagentInfo, force_transcript: bool) {
    if !force_transcript && let Some(tmux) = sub.tmux.clone() {
        open_subagent_tmux(hub, parent, sub, tmux);
        return;
    }
    if crate::server::demo_mode() {
        open_demo_subagent_transcript(parent.clone(), sub);
        return;
    }
    let path = sub.transcript.clone();
    let agent = sub.name.clone();
    let target = ViewTarget::SubagentTranscript { parent: parent.clone(), sub };
    start_transcript_stream(hub, parent.clone(), target, agent, path);
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
            // Resolve path via one-shot `activity once` if not yet discovered (e.g. from Sessions tab menu).
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
                let Ok((target, pool, askpass)) = ensure_remote_helper(&hub_clone, &bk.host_key).await else {
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
        let (target, pool, askpass) = ensure_remote_helper(hub, &bk.host_key).await.ok()?;
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
            for l in &lines {
                g.doc.push_line(l);
            }
            if !g.search_query.is_empty() {
                g.recompute_search();
            }
        }
        TailFrame::Reset => {
            g.doc.clear();
            g.expanded.clear();
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

/// Open a safe read-only `tmux attach -r -f ignore-size` terminal for a tmux subagent.
fn open_subagent_tmux(hub: &Arc<RexHub>, parent: &BlockKey, sub: SubagentInfo, tmux: TmuxTarget) {
    close_viewer();
    let (theme, is_local) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        (st.active_theme.clone(), st.hosts.get(&parent.host_key).is_some_and(|h| h.is_local))
    };
    let target = ViewTarget::SubagentTmux { parent: parent.clone(), sub: sub.clone(), target: tmux.clone() };
    let Ok(viewer) = TmuxViewer::new(target, tmux.clone(), &theme) else {
        open_subagent(hub, parent, sub, true);
        return;
    };
    let viewer = Arc::new(Mutex::new(viewer));
    if crate::server::demo_mode() {
        {
            let mut g = viewer.lock().unwrap_or_else(|e| e.into_inner());
            g.status = format!("Read-only · tmux {} × {}", tmux.cols, tmux.rows);
            // Authentic agent TUI output only — never inject synthetic Rex chrome inside the remote pane.
            let demo_vt = "\x1b[1;36mAntigravity CLI\x1b[0m \x1b[2m(gemini-4-neon-xhigh · tmux: agy-reviewer)\x1b[0m\r\n\r\n\x1b[32m●\x1b[0m Reading \x1b[1m/var/tmp/olo-activity-shots/transcript-light-2x.png\x1b[0m\r\n\x1b[32m●\x1b[0m Checking BeOS sunken well bevels and tree guides at 1×, 1.5×, 2×…\r\n\x1b[33m▸\x1b[0m Writing visual design review to \x1b[1magy-look.md\x1b[0m\r\n\r\n";
            g.terminal.vt_write(demo_vt.as_bytes());
            g.generation += 1;
        }
        manager().lock().unwrap_or_else(|e| e.into_inner()).tmux = Some(viewer);
        crate::ui::activity_changed();
        return;
    }

    {
        manager().lock().unwrap_or_else(|e| e.into_inner()).tmux = Some(viewer.clone());
    }
    crate::ui::activity_changed();

    let stop_flag = viewer.lock().unwrap_or_else(|e| e.into_inner()).stop_flag.clone();
    if is_local {
        let v_bg = viewer.clone();
        let tmux_bg = tmux.clone();
        std::thread::spawn(move || {
            run_local_tmux_pty(v_bg, tmux_bg, stop_flag);
        });
    } else {
        let hub_clone = hub.clone();
        let hk = parent.host_key.clone();
        let v_bg = viewer.clone();
        let abort = hub
            .rt
            .spawn(async move {
                let Ok((target, pool, askpass)) = ensure_remote_helper(&hub_clone, &hk).await else {
                    let mut g = v_bg.lock().unwrap_or_else(|e| e.into_inner());
                    g.error = Some("Host is not connected.".into());
                    g.generation += 1;
                    drop(g);
                    crate::ui::activity_changed();
                    return;
                };
                let cmd = tmux_attach_command(&tmux);
                let (pty_cols, pty_rows) = tmux.attach_size();
                let Ok(mut stream) = pool.exec_pty(&target, askpass, pty_cols, pty_rows, &cmd).await else {
                    let mut g = v_bg.lock().unwrap_or_else(|e| e.into_inner());
                    g.error = Some("Couldn’t attach read-only tmux session.".into());
                    g.generation += 1;
                    drop(g);
                    crate::ui::activity_changed();
                    return;
                };
                {
                    let mut g = v_bg.lock().unwrap_or_else(|e| e.into_inner());
                    g.status = format!("Read-only · tmux {} × {}", tmux.cols, tmux.rows);
                    g.generation += 1;
                }
                crate::ui::activity_changed();
                let mut buf = [0u8; 8192];
                while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut g = v_bg.lock().unwrap_or_else(|e| e.into_inner());
                            g.terminal.vt_write(&buf[..n]);
                            g.generation += 1;
                            drop(g);
                            crate::ui::activity_changed();
                        }
                    }
                }
                let mut g = v_bg.lock().unwrap_or_else(|e| e.into_inner());
                g.status = "Detached".into();
                g.generation += 1;
                drop(g);
                crate::ui::activity_changed();
            })
            .abort_handle();
        viewer.lock().unwrap_or_else(|e| e.into_inner()).abort = Some(abort);
    }
}

/// Spawn local `tmux -S <sock> attach -r -f ignore-size -t <target>` inside a
/// real Linux PTY sized to `tmux.cols × tmux.rows`, streaming VT bytes read-only.
fn run_local_tmux_pty(viewer: Arc<Mutex<TmuxViewer>>, tmux: TmuxTarget, stop_flag: Arc<std::sync::atomic::AtomicBool>) {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let (pty_cols, pty_rows) = tmux.attach_size();
    let mut ws = libc::winsize { ws_row: pty_rows, ws_col: pty_cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: standard `openpty` allocating a master/slave PTY pair.
    let rc = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) };
    if rc != 0 || master < 0 || slave < 0 {
        let mut g = viewer.lock().unwrap_or_else(|e| e.into_inner());
        g.error = Some("Couldn’t allocate PTY for read-only tmux attach.".into());
        g.generation += 1;
        drop(g);
        crate::ui::activity_changed();
        return;
    }

    // SAFETY: slave fd is valid and freshly opened; we pass duplicated copies to stdio.
    let (s_in, s_out, s_err) = unsafe { (libc::dup(slave), libc::dup(slave), libc::dup(slave)) };
    let mut cmd = std::process::Command::new("tmux");
    cmd.args(["-S", &tmux.socket, "attach", "-r", "-f", "ignore-size", "-t", &tmux.target])
        .env("TERM", "xterm-256color")
        .env_remove("TMUX")
        .stdin(unsafe { Stdio::from_raw_fd(s_in) })
        .stdout(unsafe { Stdio::from_raw_fd(s_out) })
        .stderr(unsafe { Stdio::from_raw_fd(s_err) });
    // SAFETY: `pre_exec` only calls async-signal-safe `setsid` and `ioctl(TIOCSCTTY)`.
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let child = cmd.spawn();
    // SAFETY: close parent's copy of slave fd.
    unsafe {
        libc::close(slave);
    }
    let Ok(mut child) = child else {
        // SAFETY: close master fd on spawn error.
        unsafe {
            libc::close(master);
        }
        let mut g = viewer.lock().unwrap_or_else(|e| e.into_inner());
        g.error = Some("Couldn’t start tmux attach.".into());
        g.generation += 1;
        drop(g);
        crate::ui::activity_changed();
        return;
    };

    {
        let mut g = viewer.lock().unwrap_or_else(|e| e.into_inner());
        g.status = format!("Read-only · tmux {} × {}", tmux.cols, tmux.rows);
        g.generation += 1;
    }
    crate::ui::activity_changed();

    // SAFETY: `master` is a valid exclusively-owned fd.
    let mut master_file = unsafe { std::fs::File::from_raw_fd(master) };
    // Non-blocking poll on master so `stop_flag` exits immediately without waiting.
    // SAFETY: valid fcntl on master fd.
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let mut buf = [0u8; 8192];
    while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
        let mut pfd = libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 };
        // SAFETY: valid pollfd; 100 ms timeout only while this read-only viewer is open.
        let n = unsafe { libc::poll(&mut pfd, 1, 100) };
        if n <= 0 {
            continue;
        }
        match master_file.read(&mut buf) {
            Ok(0) => break,
            Ok(got) => {
                let mut g = viewer.lock().unwrap_or_else(|e| e.into_inner());
                g.terminal.vt_write(&buf[..got]);
                g.generation += 1;
                drop(g);
                crate::ui::activity_changed();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let mut g = viewer.lock().unwrap_or_else(|e| e.into_inner());
    g.status = "Detached".into();
    g.generation += 1;
    drop(g);
    crate::ui::activity_changed();
}

// ---------------------------------------------------------------------------
// Deterministic demo fixtures for headless visual QA (`OLO_REX_DEMO_ACTIVITY`)
// ---------------------------------------------------------------------------

fn demo_transcript_doc(agent: &str) -> TranscriptDoc {
    let mut doc = TranscriptDoc::new(agent);
    doc.entries = vec![
        TranscriptEntry::Message {
            id: 1,
            speaker: Speaker::User,
            label: "You".into(),
            timestamp: "14:52:10".into(),
            segments: vec![BodySegment::Text(
                "Audit how Rex discovers nested subagents and ensure tmux-attached subagents never resize the parent agent's screen.".into(),
            )],
        },
        TranscriptEntry::Thinking {
            id: 2,
            summary: "Check panes.rs to reuse agent_of and claude_record, then inspect tmux’s client flags…".into(),
            text: "Check panes.rs to reuse agent_of and claude_record, then inspect tmux’s client flags: `tmux attach -r -f ignore-size` attaches read-only and excludes the viewer from window-size calculation so the parent agent’s pane dimensions stay untouched.".into(),
            timestamp: "14:52:12".into(),
        },
        TranscriptEntry::ToolCall {
            id: 3,
            call_id: Some("tu_1".into()),
            name: "Read".into(),
            summary: "panes.rs".into(),
            input: "{\n  \"path\": \"crates/rex-link/src/panes.rs\",\n  \"lines\": \"1-120\"\n}".into(),
            output: Some("pub fn agent_of(argv: &[String], comm: &str, exe: &str) -> Option<(&'static str, Vec<String>)>\n// Supports claude, codex, and agy processes cleanly.".into()),
            status: ToolStatus::Ok,
            timestamp: "14:52:13".into(),
        },
        TranscriptEntry::ToolCall {
            id: 4,
            call_id: Some("tu_2".into()),
            name: "Bash".into(),
            summary: "jato test -p olo-rex-link --features quic,sync,move activity".into(),
            input: "{\n  \"command\": \"jato test -p olo-rex-link --features quic,sync,move activity\"\n}".into(),
            output: Some("running 2 tests\ntest activity::tests::discovers_native_claude_codex_and_agy_subagents ... ok\ntest activity::tests::splits_complete_and_partial_tail_lines ... ok\n\ntest result: ok. 2 passed; 0 failed".into()),
            status: ToolStatus::Ok,
            timestamp: "14:52:16".into(),
        },
        TranscriptEntry::ToolCall {
            id: 5,
            call_id: Some("tu_err".into()),
            name: "Bash".into(),
            summary: "tmux -S /tmp/tmux-1000/stale list-panes".into(),
            input: "{\n  \"command\": \"tmux -S /tmp/tmux-1000/stale list-panes\"\n}".into(),
            output: Some("error connecting to /tmp/tmux-1000/stale (No such file or directory)\nexit status 1".into()),
            status: ToolStatus::Error,
            timestamp: "14:52:17".into(),
        },
        TranscriptEntry::Message {
            id: 6,
            speaker: Speaker::Assistant,
            label: agent.into(),
            timestamp: "14:52:18".into(),
            segments: vec![
                BodySegment::Text("Stale tmux sockets fail fast without stalling discovery. Live sockets attach with `ignore-size` so the subagent’s pane geometry never changes:".into()),
                BodySegment::Code {
                    lang: "rust".into(),
                    code: "pub fn tmux_attach_command(target: &TmuxTarget) -> String {\n    format!(\n        \"tmux -S {} attach -r -f ignore-size -t {}\",\n        shell_word(&target.socket),\n        shell_word(&target.target),\n    )\n}".into(),
                },
                BodySegment::Text("While Activity is hidden, the host watcher closes cleanly and uses zero CPU. Native Claude, Codex, and Antigravity subagents stream live over inotify.".into()),
            ],
        },
        TranscriptEntry::ToolCall {
            id: 7,
            call_id: Some("tu_3".into()),
            name: "Bash".into(),
            summary: "python3 tools/dev/rex-slow/try.py activity".into(),
            input: "{\n  \"command\": \"/usr/bin/python3 tools/dev/rex-slow/try.py activity\"\n}".into(),
            output: None,
            status: ToolStatus::Running,
            timestamp: "14:52:21".into(),
        },
    ];
    doc
}

fn ensure_demo_loaded(hub: &Arc<RexHub>) {
    let mut mgr = manager().lock().unwrap_or_else(|e| e.into_inner());
    if mgr.demo_loaded {
        return;
    }
    mgr.demo_loaded = true;
    // Pick the first active agent block (shown in Activity!) to attach demo nested subagents.
    let first = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut active_block: Option<BlockKey> = None;
        let mut fallback: Option<BlockKey> = None;
        for hk in &st.host_order {
            let Some(h) = st.hosts.get(hk) else { continue };
            for s in &h.session_summaries {
                let Some(v) = h.sessions.get(&s.session_id) else { continue };
                for w in &v.windows {
                    for b in w.all_blocks() {
                        let bk = BlockKey { host_key: hk.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                        if fallback.is_none() {
                            fallback = Some(bk.clone());
                        }
                        if active_block.is_none()
                            && st.blocks.get(&bk).is_some_and(|br| {
                                let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                g.agent_activity != crate::agent::AgentActivity::Idle || g.runs_agent()
                            })
                        {
                            active_block = Some(bk);
                        }
                    }
                }
            }
        }
        active_block.or(fallback)
    };
    let Some(first) = first else { return };
    let demo_subs = vec![
        SubagentInfo {
            id: "claude-sub:a15acb8e".into(),
            kind: "claude".into(),
            origin: SubagentOrigin::Native,
            name: "Explore".into(),
            description: "Audit nested subagent tmux attach".into(),
            state: SubagentState::Working,
            pid: None,
            transcript: Some("/home/you/.claude/projects/-home-you-src-olo/s1/subagents/agent-a15acb8e.jsonl".into()),
            tmux: None,
            started_ms: 1,
        },
        SubagentInfo {
            id: "tmux:/tmp/tmux-1000/default:agy-reviewer:%2".into(),
            kind: "agy".into(),
            origin: SubagentOrigin::Tmux,
            name: "agy-reviewer".into(),
            description: "Visual QA of 2× light and dark shots".into(),
            state: SubagentState::Working,
            pid: Some(41201),
            transcript: Some("/home/you/.gemini/antigravity-cli/brain/c22183c1/transcript.jsonl".into()),
            tmux: Some(TmuxTarget { socket: "/tmp/tmux-1000/default".into(), session: "agy-reviewer".into(), target: "agy-reviewer:0.0".into(), cols: 120, rows: 36, pty_cols: 120, pty_rows: 37 }),
            started_ms: 2,
        },
        SubagentInfo {
            id: "proc:41902:98123".into(),
            kind: "codex".into(),
            origin: SubagentOrigin::Shell,
            name: "Codex".into(),
            description: "Run nightly benchmark in olo".into(),
            state: SubagentState::Done,
            pid: Some(41902),
            transcript: Some("/home/you/.codex/sessions/2026/11/25/rollout-demo.jsonl".into()),
            tmux: None,
            started_ms: 3,
        },
        SubagentInfo {
            id: "proc:41990:98155".into(),
            kind: "claude".into(),
            origin: SubagentOrigin::Shell,
            name: "Lint".into(),
            description: "Check release symbols on Fedora 45".into(),
            state: SubagentState::Failed,
            pid: Some(41990),
            transcript: Some("/home/you/.claude/projects/-home-you-src-olo/s1/subagents/agent-lint.jsonl".into()),
            tmux: None,
            started_ms: 4,
        },
    ];
    mgr.by_block.insert(
        first.clone(),
        PaneActivity {
            block_id: first.block_id.clone(),
            agent: Some("claude".into()),
            pid: Some(40001),
            session_id: Some("11111111-0000-4000-8000-000000000001".into()),
            transcript: Some("/home/you/.claude/projects/-home-you-src-olo/11111111.jsonl".into()),
            subagents: demo_subs.clone(),
        },
    );
    drop(mgr);

    let mode = std::env::var("OLO_REX_DEMO_ACTIVITY").unwrap_or_default();
    match mode.as_str() {
        "collapsed" => {
            manager().lock().unwrap_or_else(|e| e.into_inner()).folded.insert(first);
        }
        "transcript" => open_demo_subagent_transcript(first, demo_subs[0].clone()),
        "follow" => {
            open_demo_subagent_transcript(first, demo_subs[0].clone());
            if let Some(tr) = active_transcript() {
                tr.lock().unwrap_or_else(|e| e.into_inner()).follow = true;
            }
        }
        "failed" => {
            let mut sub = demo_subs[3].clone();
            sub.state = SubagentState::Failed;
            open_demo_subagent_transcript(first, sub);
            if let Some(tr) = active_transcript() {
                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                g.streaming = false;
            }
        }
        "find" => {
            open_demo_subagent_transcript(first, demo_subs[0].clone());
            if let Some(tr) = active_transcript() {
                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                g.search_query = "ignore-size".into();
                g.recompute_search();
                g.expanded.insert(4);
            }
        }
        "nomatch" => {
            open_demo_subagent_transcript(first, demo_subs[0].clone());
            if let Some(tr) = active_transcript() {
                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                g.search_query = "xyznomatch".into();
                g.recompute_search();
            }
        }
        "tmux" => {
            let sub = demo_subs[1].clone();
            if let Some(t) = sub.tmux.clone() {
                open_subagent_tmux(hub, &first, sub, t);
            }
        }
        "tmux_big" => {
            let mut sub = demo_subs[1].clone();
            let big = TmuxTarget { socket: "/tmp/tmux-1000/default".into(), session: "agy-reviewer".into(), target: "agy-reviewer:0.0".into(), cols: 160, rows: 52, pty_cols: 160, pty_rows: 53 };
            sub.tmux = Some(big.clone());
            open_subagent_tmux(hub, &first, sub, big);
        }
        _ => {}
    }
}

fn open_demo_transcript(bk: BlockKey, title: String, has_terminal: bool) {
    close_viewer();
    let target = ViewTarget::PaneTranscript { bk, title, agent: "Claude".into(), path: Some("/home/you/.claude/projects/-home-you-src-olo/11111111.jsonl".into()) };
    let mut sess = TranscriptSession::new(target, "Claude".into(), Some("/home/you/.claude/projects/-home-you-src-olo/11111111.jsonl".into()));
    sess.doc = demo_transcript_doc("Claude");
    sess.loading = false;
    sess.streaming = true;
    let _ = has_terminal;
    manager().lock().unwrap_or_else(|e| e.into_inner()).transcript = Some(Arc::new(Mutex::new(sess)));
    crate::ui::activity_changed();
}

fn open_demo_subagent_transcript(parent: BlockKey, sub: SubagentInfo) {
    close_viewer();
    let label = sub.name.clone();
    let path = sub.transcript.clone();
    let target = ViewTarget::SubagentTranscript { parent, sub };
    let mut sess = TranscriptSession::new(target, label.clone(), path);
    sess.doc = demo_transcript_doc(&label);
    sess.loading = false;
    sess.streaming = true;
    // Start scrolled at top with Follow paused so the Follow toggle matches viewport position.
    sess.follow = false;
    sess.expanded.insert(4);
    manager().lock().unwrap_or_else(|e| e.into_inner()).transcript = Some(Arc::new(Mutex::new(sess)));
    crate::ui::activity_changed();
}
