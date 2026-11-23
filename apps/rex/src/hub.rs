//! Central Rex hub: manages multi-host connections (local + remote SSH/HTTP/HTTPS/Tailscale),
//! live `SessionView` state, per-block `GhosttyTerminal` + `RenderState` + `TerminalSearch`
//! streams, `olo-net` connectivity recovery, active `-L`/`-R` tunnels, Quick Look ETag cache,
//! and Olo Bridge coordination.
//!
//! Concurrency rule (strictly obeying Olo's `lockcheck` and never-block-UI rules):
//! - All network/WebSocket/SSH/file-transfer I/O runs on a dedicated `tokio` runtime thread.
//! - UI never holds locks across IPC or async waits; every mutation posts an async task
//!   and receives lightweight redraw notices (`HubNotice`) back on the UI looper.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use olo_rex_client::{
    BlockInfo, BytesDataStream, ClientPromptRequested, ControlClient, ControlEvent, DATA_TERMINAL_SUBPROTOCOL, DataTicketRequest, DemuxItem, DirListing, HostRecord, InlineApc, InlineApcDemuxer,
    KeyEvent as RexKeyEvent, KeymapBinding, KeymapDiagnostic, KeymapMode, LocalForwardHandle, PortsSnapshot, ProgramStatusRecord, RexEndpoint, ServerStatus, ServerVersion, SessionSummary,
    SessionView, SshAskpass, SshSessionPool, TailscaleStatus, TerminalDataStream, TerminalProcessSnapshot, TerminalStreamFrame, start_local_forward,
};
use olo_rex_vt::{GhosttyTerminal, RenderState, TerminalMemoryUsage, TerminalSearch, TerminalTheme};
use olo_sync::Mutex;
use serde_json::{Value, json};

use crate::{
    agent::{AgentActivity, KeepGoingState, detect_agent_from_screen},
    attention::Target,
    bridge::{self, BridgeServerHandle},
    copy_mode::CopyMode,
    git::{GitCache, GitInfo},
    keymap,
    overlays::QuickLookContent,
    settings::{SavedHost, Settings},
    themes,
    ui::Feedback,
};

/// Unique key identifying a Rex block across multiple connected hosts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub host_key: String,
    pub session_id: String,
    pub block_id: String,
}

/// Lightweight notification pushed from `RexHub` background tasks to Olo UI windows.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum HubNotice {
    /// Only terminal grid output changed for one block (redraws `Panes` child view at next frame).
    BlockOutput(BlockKey),
    /// Host/session/layout/status/prompt metadata changed (redraws full chrome).
    StateChanged,
    /// A `rex.ui` custom prompt arrived from a server.
    PromptRequested {
        host_key: String,
        req: ClientPromptRequested,
    },
    /// A `rex.ui` custom prompt was cancelled/answered on another device.
    PromptEnded {
        host_key: String,
        call_id: String,
    },
    Attention {
        target: Target,
        title: String,
        body: String,
        kind: olo_kit::notify::NoteKind,
    },
    /// Clipboard text requested by `olo.clipboard.set`.
    SetClipboard(String),
    /// A passing word of the hub's own, for something it did that
    /// nobody asked for in a window (Olo's helper put on a host): the
    /// workspace's status bar says it for a few seconds, where nothing
    /// else is being said.
    Said(String),
}

/// Connection lifecycle state for one Rex host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostConnState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting { attempt: u32, last_error: String },
    Error(String),
}

/// How long after a pane's screen changes the guess at what an agent is
/// doing there is made again ([`BlockRuntime::guess_later`]).
const GUESS_AFTER: Duration = Duration::from_millis(150);

/// Live terminal + metadata runtime for one Rex block.
pub struct BlockRuntime {
    pub creator_name: String,
    pub terminal: GhosttyTerminal,
    pub render_state: RenderState,
    pub frame: crate::frame::Frame,
    pub current_theme: TerminalTheme,
    pub search: TerminalSearch,
    find_query: Option<String>,
    find_generation: u64,
    pub finding: bool,
    pub geometry: Option<(u16, u16, u32, u32)>,
    pub other_size_owner: bool,
    pub copy_mode: Option<CopyMode>,
    pub title: String,
    /// The name the server has for the pane itself ("shell"), which its
    /// title starts as; a title unlike it is one a program gave.
    pub label: String,
    pub pwd: String,
    pub program_statuses: Vec<ProgramStatusRecord>,
    // Events can arrive while the initial program_status request is in flight.
    pending_status: Option<Vec<(u64, StatusUpdate)>>,
    snapshot_seq: u64,
    pub ports: PortsSnapshot,
    pub process: Option<TerminalProcessSnapshot>,
    process_seq: u64,
    ports_seq: u64,
    pub agent_activity: AgentActivity,
    /// The screen changed since the guess at what an agent is doing
    /// there was last made, and someone is to make it
    /// ([`BlockRuntime::guess_later`]).
    guess_wanted: bool,
    /// When its last chunk of output came.
    last_output: Instant,
    pub keep_going: Option<KeepGoingState>,
    pub unread: bool,
    pub unread_cleared_at: Option<Instant>,
    pub muted: bool,
    pub last_bell: Option<Instant>,
    pub data_connected: bool,
    input_ready: bool,
    /// What is typed here shown at once, over a link that takes its
    /// time (`predict`).
    pub predict: crate::predict::Predictor,
    /// When the predictions are next looked at for what time brings
    /// (one wait, begun when there is something to wait for).
    predict_wake: Option<Instant>,
    /// For how many seconds the host is said (in the window's status
    /// bar) to have been silent about something typed.
    stall_said: Option<u64>,
    /// Since when the program has been in the middle of an update of
    /// its screen (its synchronized output, DEC mode 2026): the pane
    /// goes on showing the frame from before it.
    updating_since: Option<Instant>,
    pub received_bytes: u64,
    pub upload_progress: Option<(String, f32)>,
    pub password_prompt: bool,
    pub data_cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone)]
enum StatusUpdate {
    Set(String, ProgramStatusRecord),
    Remove(Vec<String>),
}

impl StatusUpdate {
    fn parse(name: &str, payload: &Value) -> Option<Self> {
        match name {
            "program_status_changed" => Some(Self::Set(payload.get("id")?.as_str()?.to_owned(), serde_json::from_value(payload.get("record")?.clone()).ok()?)),
            "program_status_removed" => Some(Self::Remove(serde_json::from_value(payload.get("ids")?.clone()).ok()?)),
            _ => None,
        }
    }
    fn apply(&self, records: &mut Vec<ProgramStatusRecord>) {
        match self {
            Self::Set(id, record) => {
                let mut record = record.clone();
                record.id = (!id.is_empty()).then(|| id.clone());
                if let Some(old) = records.iter_mut().find(|r| r.id.as_deref().unwrap_or("") == id) {
                    *old = record;
                } else {
                    records.push(record);
                }
            }
            Self::Remove(ids) => records.retain(|r| !ids.iter().any(|id| id == r.id.as_deref().unwrap_or(""))),
        }
    }
}

fn move_session(rows: &mut Vec<SessionSummary>, id: &str, before: Option<&str>) {
    if before == Some(id) {
        return;
    }
    if let Some(i) = rows.iter().position(|s| s.session_id == id) {
        let row = rows.remove(i);
        let target = rows.iter().position(|s| Some(s.session_id.as_str()) == before).unwrap_or(rows.len());
        rows.insert(target, row);
    }
}

fn transfer_reference(direction: &str) -> String {
    format!("{direction}:{}", uuid::Uuid::new_v4())
}

fn merge_process(current: &mut Option<TerminalProcessSnapshot>, payload: &Value) -> Option<String> {
    // A change describes the foreground only. Home, child and last exit
    // belong to the full method response and survive this event.
    let change: TerminalProcessSnapshot = serde_json::from_value(payload.clone()).ok()?;
    let stored = current.get_or_insert_with(Default::default);
    if payload.get("foreground").is_some() {
        stored.foreground = change.foreground;
    }
    if payload.get("ancestors").is_some() {
        stored.ancestors = change.ancestors;
    }
    stored.foreground.as_ref().or(stored.child.as_ref()).and_then(|p| p.cwd.clone())
}

impl BlockRuntime {
    pub fn new(cols: u16, rows: u16, theme: &TerminalTheme) -> Self {
        let mut terminal = GhosttyTerminal::new(cols.max(2), rows.max(1)).expect("libghostty-vt terminal allocation");
        let _ = terminal.apply_theme(theme);
        let render_state = RenderState::new().expect("libghostty-vt render state");
        let search = TerminalSearch::new(&terminal).expect("libghostty-vt search");
        Self {
            creator_name: "com.superlogical.terminal".to_owned(),
            terminal,
            render_state,
            frame: crate::frame::Frame::new(),
            current_theme: theme.clone(),
            search,
            find_query: None,
            find_generation: 0,
            finding: false,
            geometry: None,
            other_size_owner: false,
            copy_mode: None,
            title: String::new(),
            label: String::new(),
            pwd: String::new(),
            program_statuses: Vec::new(),
            pending_status: Some(Vec::new()),
            snapshot_seq: 0,
            ports: PortsSnapshot::default(),
            process: None,
            process_seq: 0,
            ports_seq: 0,
            agent_activity: AgentActivity::Idle,
            guess_wanted: false,
            last_output: Instant::now(),
            keep_going: None,
            unread: false,
            unread_cleared_at: None,
            muted: false,
            last_bell: None,
            data_connected: false,
            input_ready: false,
            predict: crate::predict::Predictor::new(false),
            predict_wake: None,
            stall_said: None,
            updating_since: None,
            received_bytes: 0,
            upload_progress: None,
            password_prompt: false,
            data_cancel: None,
        }
    }

    pub fn set_find_query(&mut self, query: Option<&str>) -> Result<(), olo_rex_vt::VtError> {
        self.search.set_needle(query)?;
        self.find_generation = self.find_generation.wrapping_add(1);
        self.finding = query.is_some();
        self.find_query = query.map(str::to_owned);
        Ok(())
    }

    fn restore_terminal(&mut self, terminal: GhosttyTerminal) -> Result<(), olo_rex_vt::VtError> {
        let mut search = TerminalSearch::new(&terminal)?;
        if let Some(query) = &self.find_query {
            search.set_needle(Some(query))?;
        }
        self.find_generation = self.find_generation.wrapping_add(1);
        self.finding = self.find_query.is_some();
        self.geometry = None;
        self.render_state = RenderState::new()?;
        self.frame = crate::frame::Frame::new();
        self.search = search;
        self.terminal = terminal;
        Ok(())
    }

    pub fn apply_theme(&mut self, theme: &TerminalTheme) -> Result<(), olo_rex_vt::VtError> {
        self.terminal.apply_theme(theme)?;
        self.current_theme.clone_from(theme);
        Ok(())
    }

    fn inline(&mut self, apc: InlineApc, client: Option<&str>) -> Option<InlineApc> {
        match apc {
            InlineApc::Resize { cols, rows, cell_width_px, cell_height_px, owner } => {
                if self.resize(cols, rows, cell_width_px, cell_height_px).is_ok() {
                    self.other_size_owner = owner.is_some() && owner.as_deref() != client;
                }
            }
            InlineApc::Theme { foreground, background, cursor, palette, .. } => {
                let mut theme = self.current_theme.clone();
                if let Some(c) = foreground.as_deref().and_then(olo_rex_vt::parse_color) {
                    theme.foreground = c;
                }
                if let Some(c) = background.as_deref().and_then(olo_rex_vt::parse_color) {
                    theme.background = c;
                }
                if let Some(c) = cursor.as_deref().and_then(olo_rex_vt::parse_color) {
                    theme.cursor = Some(c);
                }
                for (i, c) in palette.iter().filter_map(|s| olo_rex_vt::parse_palette_entry(s)) {
                    if let Some(entry) = theme.palette.get_mut(i as usize) {
                        *entry = c;
                    }
                }
                let _ = self.apply_theme(&theme);
            }
            other => return Some(other),
        }
        None
    }
    /// The caret's row of the frame as last captured, for predictions:
    /// none while text is being selected by keys or the pane looks
    /// back at what scrolled away (nothing typed is seen go in there).
    fn caret_screen(&self) -> Option<crate::predict::Screen> {
        if self.copy_mode.is_some() || !self.terminal.is_viewport_at_bottom() {
            return None;
        }
        crate::predict::screen_of(&self.frame, self.terminal.active_screen() == olo_rex_vt::ScreenKind::Alternate)
    }

    /// At a paint, the frame just captured: the predictions are shown
    /// it, and what they draw over it is returned, with the time they
    /// are to be looked at again if they wait on one. Nothing where
    /// nothing is typed here (its frames aren't looked at).
    pub fn predicted(&mut self, now: Instant) -> (Option<crate::predict::Overlay>, Option<Instant>) {
        if !self.predict.wants_frames() {
            return (None, None);
        }
        // (In the middle of a program's update, DEC mode 2026, the frame
        // isn't one: nothing is judged by a piece.)
        if !self.terminal.dec_mode(2026) {
            let screen = self.caret_screen();
            self.predict.frame(screen.as_ref(), now);
        }
        let (next, _, _) = self.predict_tick(now);
        (self.predict.overlay(), next)
    }

    /// How long the host has been silent about something typed here,
    /// where that is long enough to say.
    pub fn stalled(&self) -> Option<Duration> {
        self.predict.stalled(Instant::now())
    }

    /// What time brings to the predictions, looked at now: when to look
    /// next, whether what they draw changed, and whether what the
    /// status bar says of a silent host did.
    fn predict_tick(&mut self, now: Instant) -> (Option<Instant>, bool, bool) {
        let (next, changed) = self.predict.tick(now);
        // (Said by the second for a minute, by the minute from then.)
        let said = self.predict.stalled(now).map(|silent| silent.as_secs()).map(|secs| if secs < 60 { secs } else { secs / 60 * 60 });
        let chrome = std::mem::replace(&mut self.stall_said, said) != said;
        (next.into_iter().chain(self.predict.stall_changes(now)).min(), changed, chrome)
    }

    /// Whether the pane is to go on showing the frame it has: the
    /// program is in the middle of an update of its screen
    /// (synchronized output, DEC mode 2026), and what is on its screen
    /// now is a piece of a frame. Not for longer than a second (a
    /// program that never ends its update isn't waited on): `again`,
    /// when to paint whatever there is.
    pub fn held(&mut self, now: Instant) -> Option<Duration> {
        if !self.terminal.dec_mode(2026) {
            self.updating_since = None;
            return None;
        }
        let since = *self.updating_since.get_or_insert(now);
        Duration::from_secs(1).checked_sub(now.saturating_duration_since(since)).filter(|left| !left.is_zero())
    }

    /// Whether the terminal reads a secret, as the host says: kept, and
    /// the predictions told (nothing of a secret is drawn).
    fn set_password_prompt(&mut self, asks: bool) -> bool {
        self.predict.set_secret(asks);
        std::mem::replace(&mut self.password_prompt, asks) != asks
    }

    fn find_step(&mut self, generation: u64) -> Result<bool, olo_rex_vt::VtError> {
        if !self.finding || self.find_generation != generation {
            return Ok(true);
        }
        match self.search.tick()? {
            olo_rex_vt::SearchStatus::FeedRequired => self.search.feed()?,
            olo_rex_vt::SearchStatus::Complete => {
                self.finding = false;
                self.search.select_next()?;
                return Ok(true);
            }
            olo_rex_vt::SearchStatus::Running => {}
        }
        Ok(false)
    }

    fn resize(&mut self, cols: u16, rows: u16, cw: u32, ch: u32) -> Result<bool, olo_rex_vt::VtError> {
        let geometry = (cols, rows, cw, ch);
        if self.geometry == Some(geometry) {
            return Ok(false);
        }
        self.terminal.resize(cols, rows, cw, ch)?;
        self.geometry = Some(geometry);
        Ok(true)
    }
    fn status_event(&mut self, seq: u64, change: StatusUpdate) {
        if seq != 0 && seq <= self.snapshot_seq {
            return;
        }
        change.apply(&mut self.program_statuses);
        if let Some(events) = &mut self.pending_status {
            events.push((seq, change));
        }
    }

    fn process_snapshot(&mut self, result: olo_rex_client::RpcResult<TerminalProcessSnapshot>) {
        let mut snapshot = result.payload;
        if result.seq < self.process_seq {
            if let Some(latest) = self.process.take() {
                snapshot.foreground = latest.foreground;
                snapshot.ancestors = latest.ancestors;
            }
        } else {
            self.process_seq = result.seq;
        }
        if let Some(cwd) = snapshot.foreground.as_ref().and_then(|p| p.cwd.clone()).or_else(|| snapshot.child.as_ref().and_then(|p| p.cwd.clone())) {
            self.pwd = cwd;
        }
        self.process = Some(snapshot);
        self.recompute_activity();
    }
    fn set_ports(&mut self, seq: u64, ports: PortsSnapshot) {
        if seq >= self.ports_seq {
            self.ports_seq = seq;
            self.ports = ports;
        }
    }
    fn status_snapshot(&mut self, snapshot: Option<olo_rex_client::RpcResult<Vec<ProgramStatusRecord>>>) {
        if snapshot.as_ref().is_some_and(|r| r.seq < self.snapshot_seq) {
            return;
        }
        let pending = self.pending_status.take().unwrap_or_default();
        if let Some(snapshot) = snapshot {
            self.snapshot_seq = snapshot.seq;
            self.program_statuses = snapshot.payload;
            for (seq, event) in pending {
                if seq == 0 || seq > snapshot.seq {
                    event.apply(&mut self.program_statuses);
                }
            }
        }
        self.recompute_activity();
    }

    /// What the pane is called (its head, its name to agents): see
    /// [`pane_title`]. `local_home`: this computer's home, for a pane of
    /// its own server (a far one's is the server's word, if it gave it).
    pub fn said(&self, local_home: Option<&str>) -> String {
        let process = self.process.as_ref();
        let program = process.and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.as_str());
        let home = process.and_then(|p| p.home.as_deref()).or(local_home);
        pane_title(&self.title, &self.label, program, &folder_said(&self.pwd, home))
    }

    /// The screen changed: what an agent is doing there, guessed from
    /// the screen's last rows where the server has no word of it, is to
    /// be looked at again. True when nobody is yet looking: the caller
    /// then has someone look every [`GUESS_AFTER`] until the pane is
    /// quiet ([`BlockRuntime::guess_now`]), once for however many chunks
    /// come meanwhile, the last of them included. (It was done with
    /// every chunk: twelve rows read as text and searched, a hundred
    /// times a second while a pane streamed, 3% of what the streaming
    /// cost, under the lock the window draws the pane by.)
    pub fn guess_later(&mut self) -> bool {
        self.last_output = Instant::now();
        !std::mem::replace(&mut self.guess_wanted, true)
    }

    /// Looks again at what an agent is doing: whether that changed, and
    /// whether the pane has been quiet for `quiet` (no chunk since):
    /// then whoever looks is done, and the next chunk asks anew.
    pub fn guess_now(&mut self, quiet: Duration) -> (bool, bool) {
        let was = self.agent_activity;
        let changed = self.recompute_activity() != was;
        let still = self.last_output.elapsed() >= quiet;
        if still {
            self.guess_wanted = false;
        }
        (changed, still)
    }

    pub fn recompute_activity(&mut self) -> AgentActivity {
        if let Some((act, _)) = AgentActivity::from_records(&self.program_statuses) {
            self.agent_activity = act;
            return act;
        }
        let (_, rows) = self.terminal.size();
        let start = rows.saturating_sub(12);
        let mut tail = Vec::new();
        for r in start..rows {
            tail.push(self.terminal.viewport_row_text(r as u32));
        }
        let fg = self.process.as_ref().and_then(|p| p.foreground.as_ref()).map(|p| p.name.as_str());
        self.agent_activity = detect_agent_from_screen(fg, &tail).unwrap_or(AgentActivity::Idle);
        self.agent_activity
    }

    pub fn memory_usage(&self) -> TerminalMemoryUsage {
        self.terminal.memory_usage()
    }
}

/// Keep drawing and input on the clicked pane while earlier server views are in flight.
struct FocusTarget {
    window: String,
    block: Option<String>,
    restore: Option<String>,
}

impl FocusTarget {
    fn apply(&self, view: &mut SessionView) {
        let Some(w) = view.windows.iter_mut().find(|w| w.window_id == self.window) else { return };
        view.active_window_id = Some(w.window_id.clone());
        if let Some(b) = &self.block
            && w.all_blocks().any(|row| row.block_id == *b)
        {
            w.focused_block_id = Some(b.clone());
        }
        if let Some(l) = w.layers.iter_mut().find(|l| Some(&l.layer_id) == self.restore.as_ref()) {
            l.state = Some("normal".into());
        }
    }
}

/// Ordered terminal input operation serialized strictly FIFO per host so fast typing never reorders.
enum HostInputOp {
    Focus {
        session_id: String,
        switch: bool,
        target: Arc<FocusTarget>,
    },
    /// (`seq`: its number among what was typed in that terminal, given
    /// back to the terminal's predictions when the host has answered.)
    Write {
        session_id: String,
        block_id: String,
        text: String,
        seq: u64,
    },
    Key {
        session_id: String,
        block_id: String,
        event: RexKeyEvent,
        seq: u64,
    },
}

/// The name a host goes by when it is given none: the host in its
/// address, by its first part ("ssh://you@build.example:2222" is
/// "build"), not the address whole; a numeric address is itself.
pub fn host_name_from(address: &str) -> String {
    let rest = address.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    let rest = rest.split(['/', '?']).next().unwrap_or(rest);
    // (A port after the host; an IPv6 address in brackets keeps its colons.)
    let host = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((v6, _)) => v6,
        None => rest.rsplit_once(':').filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).map_or(rest, |(host, _)| host),
    };
    if host.is_empty() {
        return address.trim().to_owned();
    }
    // (A name's first part is the name people say, "build" of
    // "build.example"; a number is itself, whole.)
    let numeric = host.contains(':') || host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if numeric { host.to_owned() } else { host.split('.').next().unwrap_or(host).to_owned() }
}

/// The demo's word of the program running in one of its panes.
fn demo_process(name: &str) -> TerminalProcessSnapshot {
    let foreground = olo_rex_client::ProcessInfo { pid: 4210, name: name.to_owned(), ..Default::default() };
    TerminalProcessSnapshot { foreground: Some(foreground), home: Some("/home/you".to_owned()), ..Default::default() }
}

/// What an unnamed tab goes by, from its first pane: the title that
/// pane's program gave it, if it gave one; else nothing where its
/// folder's name will do (`tab_names` takes the folder then), and the
/// program running in it where the folder is home or unknown ("~" names
/// nothing). So a tab says "olo" or "bash", not the server's "shell"
/// for every one (Opus's review, 2026-11-22).
pub fn tab_title(title: &str, label: &str, program: Option<&str>, pwd: &str, home: Option<&str>) -> String {
    let (title, label) = (title.trim(), label.trim());
    if !title.is_empty() && title != label {
        return title.to_owned();
    }
    let at_home = home.is_some_and(|h| pwd.trim_end_matches('/') == h.trim_end_matches('/'));
    if !pwd.is_empty() && !at_home {
        return String::new();
    }
    program.map(str::trim).filter(|p| !p.is_empty()).unwrap_or(label).to_owned()
}

/// What a pane is called: the title its program gave it, if it gave
/// one (`title`, where it isn't the pane's own `label` still); else
/// the program running in it now and its folder, "bash — ~/src/olo"
/// (the program alone where no folder is known; the label, or "Shell",
/// where no program is). One form for every pane, so three shells in a
/// session aren't three heads that say "shell" (Opus's review,
/// 2026-11-22).
pub fn pane_title(title: &str, label: &str, program: Option<&str>, folder: &str) -> String {
    let (title, label) = (title.trim(), label.trim());
    if !title.is_empty() && title != label {
        return title.to_owned();
    }
    let what = program.map(str::trim).filter(|p| !p.is_empty()).or(Some(label).filter(|l| !l.is_empty())).map(str::to_owned).unwrap_or_else(|| olo_kit::words::tr("Shell").into());
    if folder.is_empty() { what } else { format!("{what} — {folder}") }
}

/// A folder as it is said: under `home`, from `~`.
pub fn folder_said(folder: &str, home: Option<&str>) -> String {
    match home.map(|h| h.trim_end_matches('/')).filter(|h| !h.is_empty()) {
        Some(home) if folder == home => "~".into(),
        Some(home) => match folder.strip_prefix(home).filter(|rest| rest.starts_with('/')) {
            Some(rest) => format!("~{rest}"),
            None => folder.to_owned(),
        },
        None => folder.to_owned(),
    }
}

/// What the status bar says of a host that has been silent about
/// something typed for `silent`: its name (quoted where the person gave
/// it), and for how long, by the second for a minute and by the minute
/// from then.
pub fn silent_said(host: &str, local: bool, silent: Duration) -> String {
    let who = if local { host.to_owned() } else { olo_kit::tr!("“{}”", host).to_string() };
    match silent.as_secs() {
        secs @ 0..=59 => olo_kit::tr!("{} hasn’t answered for {} seconds", who, secs),
        60..=119 => olo_kit::tr!("{} hasn’t answered for over a minute", who),
        secs => olo_kit::tr!("{} hasn’t answered for over {} minutes", who, secs / 60),
    }
    .to_string()
}

/// The same for the host's own row, which needs no name (the third
/// design review of the helper's words: a tip on a host's row never
/// names its host).
pub fn silent_tip(silent: Duration) -> String {
    match silent.as_secs() {
        secs @ 0..=59 => olo_kit::tr!("Hasn’t answered for {} seconds", secs).to_string(),
        60..=119 => olo_kit::words::tr("Hasn’t answered for over a minute").to_string(),
        secs => olo_kit::tr!("Hasn’t answered for over {} minutes", secs / 60).to_string(),
    }
}

/// A terminal's predictions are looked at once, at `at`, for what time
/// brings (a frame that was none of those expected is believed, a key
/// the host took and didn't show is given up), and the pane drawn again
/// if what they draw changed: one wait, begun when there is something
/// to wait for, never a clock that ticks. `held`: the block, locked by
/// the caller.
fn predict_wake(rt: &tokio::runtime::Handle, block: &Arc<Mutex<BlockRuntime>>, held: &mut BlockRuntime, key: &BlockKey, notify: &Arc<dyn Fn(HubNotice) + Send + Sync>, at: Instant) {
    if held.predict_wake.is_some_and(|waited| waited <= at) {
        return;
    }
    held.predict_wake = Some(at);
    let (block, key, notify) = (block.clone(), key.clone(), notify.clone());
    rt.spawn(async move {
        let mut at = at;
        loop {
            tokio::time::sleep_until(at.into()).await;
            let (next, changed, chrome) = {
                let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                // (A sooner wait was begun since: this one is its.)
                if b.predict_wake != Some(at) {
                    return;
                }
                let (next, changed, chrome) = b.predict_tick(Instant::now());
                b.predict_wake = next;
                (next, changed, chrome)
            };
            if changed {
                notify(HubNotice::BlockOutput(key.clone()));
            }
            if chrome {
                notify(HubNotice::StateChanged);
            }
            match next {
                Some(next) => at = next,
                None => return,
            }
        }
    });
}

/// Whether a program at a terminal in this input mode is asking for
/// something secret: what is typed isn't shown (`echo` off) and the
/// terminal itself is collecting a line of it (`canonical` on), as
/// `sudo`, `ssh`, `passwd` and a shell's `read -s` ask. A shell's own
/// line editor has echo off too, at every prompt, but reads key by key
/// (canonical off): by echo alone, every shell at its prompt was "asking
/// for a password", with Paste Password… over it (2026-11-22).
pub fn asks_for_secret(echo: bool, canonical: bool) -> bool {
    !echo && canonical
}

/// Where a tab goes when the one at `from` of `ids` is put at `to` among
/// the others (a strip's drag, Move Tab Left and Right): the tab it
/// then stands before, as the server is asked (none: the last). None
/// altogether where nothing moves.
pub fn moved_before(ids: &[String], from: usize, to: usize) -> Option<Option<String>> {
    if from >= ids.len() || to >= ids.len() || from == to {
        return None;
    }
    let others: Vec<&String> = ids.iter().enumerate().filter(|(i, _)| *i != from).map(|(_, id)| id).collect();
    Some(others.get(to).map(|id| (*id).clone()))
}

/// A tab's name, if it was given one. The server names a tab it wasn't
/// told a name for "Window" and the end of its id ("Window 65bb795d"):
/// no name to show a person, who sees its program's title, or which
/// tab it is, in its place.
pub fn tab_label(w: &olo_rex_client::WindowView) -> Option<&str> {
    let label = w.label.trim();
    let placeholder = label.strip_prefix("Window ").is_some_and(|id| id.len() == 8 && w.window_id.ends_with(id));
    (!label.is_empty() && !placeholder).then_some(label)
}

/// Names for a session's tabs that tell each from the others, from what
/// is known of each: the name it was given ([`tab_label`]), what its
/// program calls itself, and its folder. A tab with a name has it. One
/// without goes by its program's title, or its folder's name, or which
/// tab it is; and where two of those are the same, by the title with
/// the folder's name after it ("shell · rex"), or failing that with its
/// place among them ("shell 2").
pub fn tab_names(tabs: &[(Option<&str>, &str, &str)]) -> Vec<String> {
    let folder = |pwd: &str| pwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned();
    let mut names: Vec<String> = tabs
        .iter()
        .enumerate()
        .map(|(i, (given, title, pwd))| match given {
            Some(name) => (*name).to_owned(),
            None if !title.trim().is_empty() => title.trim().to_owned(),
            None if !folder(pwd).is_empty() => folder(pwd),
            None => olo_kit::tr!("Tab {}", i + 1),
        })
        .collect();
    // By their folders, where those tell them apart (two shells in one
    // folder gain nothing by its name).
    let apart = |i: usize| (0..tabs.len()).any(|j| j != i && names[j] == names[i] && folder(tabs[j].2) != folder(tabs[i].2));
    let with_folder: Vec<Option<String>> = (0..tabs.len())
        .map(|i| {
            let dir = folder(tabs[i].2);
            (tabs[i].0.is_none() && apart(i) && !dir.is_empty() && dir != names[i]).then(|| format!("{} · {dir}", names[i]))
        })
        .collect();
    for (name, better) in names.iter_mut().zip(with_folder) {
        if let Some(better) = better {
            *name = better;
        }
    }
    // And by their place, where they still say the same.
    for i in 0..names.len() {
        if tabs[i].0.is_none() {
            let before = names[..i].iter().filter(|n| **n == names[i]).count();
            if before > 0 {
                let mut k = before + 1;
                while names.contains(&format!("{} {k}", names[i])) {
                    k += 1;
                }
                names[i] = format!("{} {k}", names[i]);
            }
        }
    }
    names
}

/// [`tab_names`] for a session as the hub holds it: each tab's first
/// pane with a title, and that pane's folder.
pub fn session_tab_names(st: &HubState, host_key: &str, sess: &olo_rex_client::SessionView) -> Vec<String> {
    let local = if st.hosts.get(host_key).is_some_and(|h| h.is_local) { local_home() } else { None };
    let known: Vec<(Option<String>, String, String)> = sess
        .windows
        .iter()
        .map(|w| {
            // Its first pane that is known: what that one goes by.
            let (mut title, mut pwd) = (String::new(), String::new());
            for b in w.all_blocks() {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                if let Some(br) = st.blocks.get(&bk) {
                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                    let process = g.process.as_ref();
                    let program = process.and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.as_str());
                    let home = process.and_then(|p| p.home.as_deref()).or(local.as_deref());
                    title = tab_title(&g.title, &g.label, program, &g.pwd, home);
                    pwd = g.pwd.clone();
                    break;
                }
            }
            (tab_label(w).map(str::to_owned), title, pwd)
        })
        .collect();
    let tabs: Vec<(Option<&str>, &str, &str)> = known.iter().map(|(given, title, pwd)| (given.as_deref(), title.as_str(), pwd.as_str())).collect();
    tab_names(&tabs)
}

/// The most typed requests on their way to a host at once (where its
/// helper keeps their order): what is typed past that waits here, and
/// goes together.
const MOST_UNANSWERED: usize = 32;

/// Something typed, sent without waiting, and its answer still to come.
struct Unanswered {
    session_id: String,
    block_id: String,
    seq: u64,
    answer: std::pin::Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl Unanswered {
    fn new<T>(session_id: String, block_id: String, seq: u64, answer: impl Future<Output = T> + Send + 'static) -> Unanswered {
        Unanswered {
            session_id,
            block_id,
            seq,
            answer: Box::pin(async move {
                let _ = answer.await;
            }),
        }
    }
}

/// The oldest of them answered (or given up on), and taken off; `None`
/// at once where there is none.
async fn oldest_answer(unanswered: &mut std::collections::VecDeque<Unanswered>) -> Option<Unanswered> {
    unanswered.front_mut()?.answer.as_mut().await;
    unanswered.pop_front()
}

/// The next of what waits to be written to a host: the first, and with
/// it, where that is text for a terminal, the text that follows for the
/// same terminal (as far as the next thing that isn't: a key event, a
/// change of focus, another pane's text keep their places). So what was
/// typed while a request was on its way is one request, in the order
/// typed.
fn typed_together(waiting: &mut std::collections::VecDeque<HostInputOp>) -> Option<HostInputOp> {
    let mut first = waiting.pop_front()?;
    if let HostInputOp::Write { session_id, block_id, text, seq } = &mut first {
        while let Some(HostInputOp::Write { session_id: s, block_id: b, .. }) = waiting.front() {
            if s != session_id || b != block_id {
                break;
            }
            if let Some(HostInputOp::Write { text: more, seq: later, .. }) = waiting.pop_front() {
                text.push_str(&more);
                *seq = later.max(*seq);
            }
        }
    }
    Some(first)
}

/// Active `-L` port forward entry shown in Hosts & Port Forwards.
pub struct ActiveForward {
    pub id: String,
    pub host_key: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_addr: SocketAddr,
    pub state: ForwardState,
    handle: LocalForwardHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardState {
    Active,
    Waiting,
    Failed(String),
}

/// Authoritative membership and the views attached on this connection. Some servers
/// bootstrap only the preferred view. Older peers support the list/attach calls.
struct SessionSnapshot {
    seq: u64,
    rows: Vec<SessionSummary>,
    views: Vec<SessionView>,
    tickets: Vec<olo_rex_client::DataTicket>,
}

impl SessionSnapshot {
    async fn read(ctrl: &ControlClient, tickets: &[DataTicketRequest]) -> Result<Self, olo_rex_client::RexError> {
        let mut snapshot = match ctrl.session_bootstrap("all", None, None, tickets).await {
            Ok(r) => {
                let b = r.payload;
                let views = match b.attached_sessions {
                    Some(attached) => attached.into_iter().map(|a| a.view).collect(),
                    None => b.selected_session.into_iter().map(|a| a.view).collect(),
                };
                Self { seq: r.seq, rows: b.attach_all.sessions, views, tickets: b.data_tickets.unwrap_or_default() }
            }
            Err(_) => {
                let r = ctrl.session_list().await?;
                Self { seq: r.seq, rows: r.payload, views: Vec::new(), tickets: Vec::new() }
            }
        };
        for row in &snapshot.rows {
            if !snapshot.views.iter().any(|v| v.session_id == row.session_id) {
                match ctrl.session_attach(&row.session_id).await {
                    Ok(r) => snapshot.views.push(r.payload.view),
                    // Removed since the list. Its ordered destruction event follows.
                    Err(olo_rex_client::RexError::Api(e)) if e.status == 404 => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(snapshot)
    }
}

/// Each numbered envelope is delivered once. Snapshot watermarks also cover session membership.
#[derive(Default)]
struct EventSequence {
    seen: u64,
    membership: u64,
}

impl EventSequence {
    fn next(&mut self, seq: u64) -> Option<bool> {
        if seq == 0 {
            return Some(false);
        }
        if seq <= self.seen {
            return None;
        }
        let gap = seq.saturating_sub(self.seen.max(self.membership)) > 1;
        self.seen = seq;
        Some(gap)
    }
}

/// Olo's helper on a host (`olo-rex-link`), as the host's connection
/// found it or left it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HostHelper {
    /// Not looked for: the host isn't reached over SSH, the setting is
    /// off, or it isn't connected.
    #[default]
    None,
    /// The control connection goes through it: what is typed is sent
    /// as it is typed.
    Used,
    /// It is being put there now.
    Installing,
    /// It is there, for the next connection.
    Ready,
    /// The host goes without, and why (said to the person).
    Without(String),
}

impl HostHelper {
    /// What a connected host's tip says of it: with a comma what is
    /// settled, with a full stop what is on its way or to come (the
    /// second design review).
    pub fn said(&self) -> String {
        match self {
            HostHelper::None => olo_kit::words::tr("Connected").to_string(),
            HostHelper::Used => olo_kit::words::tr("Connected, with Olo’s typing helper").to_string(),
            HostHelper::Installing => olo_kit::words::tr("Connected. Adding Olo’s typing helper…").to_string(),
            HostHelper::Ready => olo_kit::words::tr("Connected. Olo’s typing helper was added and will be used the next time you connect.").to_string(),
            HostHelper::Without(why) => olo_kit::tr!("Connected, without Olo’s typing helper: {}", why).to_string(),
        }
    }
}

/// Full live state of one connected (or connecting) Rex host.
pub struct HostRuntime {
    pub key: String,
    pub label: String,
    pub endpoint_raw: String,
    pub endpoint: Option<RexEndpoint>,
    pub is_local: bool,
    pub conn_state: HostConnState,
    pub connection_lost: Option<SystemTime>,
    pub client_id: Option<String>,
    pub reconnect_token: Option<String>,
    retry: Option<Arc<tokio::sync::Notify>>,
    pub server_status: Option<ServerStatus>,
    pub server_version: Option<ServerVersion>,
    /// Olo's helper there, for this connection.
    pub helper: HostHelper,
    pub tailscale: Option<TailscaleStatus>,
    tailscale_seq: u64,
    pub synced_hosts: Vec<HostRecord>,
    pub session_summaries: Vec<SessionSummary>,
    pub sessions: HashMap<String, SessionView>,
    pending_focus: HashMap<String, Arc<FocusTarget>>,
    pub keymap_bindings: Vec<KeymapBinding>,
    pub keymap_modes: Vec<KeymapMode>,
    pub keymap_diagnostics: Vec<KeymapDiagnostic>,
    pub control: Option<ControlClient>,
    input_tx: Option<tokio::sync::mpsc::UnboundedSender<HostInputOp>>,
    pub bridge_server: Option<BridgeServerHandle>,
    pub bridge_remote_port: Option<u16>,
}

// Only the ordered lifecycle events and bootstrap own list membership.
// A view RPC can finish after a newer view, destruction, or disconnect.
fn install_view(rows: &mut [SessionSummary], sessions: &mut HashMap<String, SessionView>, view: SessionView) -> bool {
    let Some(row) = rows.iter_mut().find(|r| r.session_id == view.session_id) else { return false };
    if sessions.get(&view.session_id).is_some_and(|old| old.revision > view.revision) {
        return false;
    }
    row.label.clone_from(&view.label);
    sessions.insert(view.session_id.clone(), view);
    true
}

#[derive(Clone)]
struct QuickLookCacheEntry {
    version: Option<String>,
    preview: QuickLookContent,
}

// A normal end-of-stream and the advertised size are both needed. In particular,
// even an error after the last byte isn't a completed transfer.
fn preview_complete(bytes: &mut Vec<u8>, expected: u64, next: Result<Option<Vec<u8>>, olo_rex_client::RexError>) -> Result<bool, olo_rex_client::RexError> {
    use olo_rex_client::RexError;
    match next? {
        Some(chunk) => {
            if chunk.len() as u64 > expected.saturating_sub(bytes.len() as u64) {
                return Err(RexError::Protocol("The file is larger than the server reported.".into()));
            }
            bytes.try_reserve(chunk.len()).map_err(|_| RexError::Protocol("The preview is too large to load.".into()))?;
            bytes.extend_from_slice(&chunk);
            Ok(false)
        }
        None if bytes.len() as u64 == expected => Ok(true),
        None => Err(RexError::Protocol(format!("Received {} of {expected} bytes.", bytes.len()))),
    }
}

/// Shared state protected by `olo_sync::Mutex` (never held across `.await`!).
pub struct HubState {
    pub hosts: HashMap<String, HostRuntime>,
    pub host_order: Vec<String>,
    pub blocks: HashMap<BlockKey, Arc<Mutex<BlockRuntime>>>,
    pub forwards: Vec<ActiveForward>,
    pub settings: Settings,
    pub active_theme: TerminalTheme,
    pub git_cache: Arc<GitCache>,
    pub local_bin_dir: Option<PathBuf>,
    quick_look_cache: HashMap<(String, String, String), QuickLookCacheEntry>,
}

impl HubState {
    /// How long a host has been silent about something typed into one
    /// of its terminals, where that is long enough to say: the longest
    /// of them. What the status bar says and the sidebar's light for
    /// the host shows are both this, so they can't disagree. (Typing
    /// goes to a host one write at a time: a terminal of it unanswered
    /// is the host unanswering.)
    pub fn host_silent(&self, host_key: &str) -> Option<Duration> {
        self.blocks.iter().filter(|(key, _)| key.host_key == host_key).filter_map(|(_, block)| block.lock().unwrap_or_else(|e| e.into_inner()).stalled()).max()
    }

    fn retain_blocks(&mut self, keep: impl Fn(&BlockKey) -> bool) {
        self.blocks.retain(|k, b| {
            if keep(k) {
                return true;
            }
            let mut block = b.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cancel) = block.data_cancel.take() {
                let _ = cancel.send(());
            }
            block.find_generation = block.find_generation.wrapping_add(1);
            false
        })
    }
}

/// Thread-safe handle to the Rex background engine.
pub struct RexHub {
    pub state: Arc<Mutex<HubState>>,
    rt: tokio::runtime::Runtime,
    notify: Arc<dyn Fn(HubNotice) + Send + Sync>,
    inner: Arc<RexHubInner>,
}

struct RexHubInner {
    state: Arc<Mutex<HubState>>,
    ssh_pool: Arc<SshSessionPool>,
    askpass: Option<Arc<dyn SshAskpass>>,
    notify: Arc<dyn Fn(HubNotice) + Send + Sync>,
}

/// This computer's home folder, as the status bar shortens folders
/// under it to `~` (the demo's own, in the demo).
pub fn local_home() -> Option<String> {
    if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") { Some("/home/you".into()) } else { std::env::var("HOME").ok() }
}

impl RexHub {
    pub fn start(settings: Settings, active_theme: TerminalTheme, askpass: Option<Arc<dyn SshAskpass>>, notify: impl Fn(HubNotice) + Send + Sync + 'static) -> Arc<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("rex-io").enable_all().build().expect("create Rex tokio runtime");

        let local_bin_dir = bridge::ensure_local_bin_dir();
        let ssh_pool = Arc::new(SshSessionPool::default());
        // (Olo's helper on the hosts: a control connection goes through
        // it where it is. `link`.)
        ssh_pool.set_helper((settings.host_helper && crate::link::carried()).then_some(crate::link::ID));
        let notify: Arc<dyn Fn(HubNotice) + Send + Sync> = Arc::new(notify);

        let mut hosts = HashMap::new();
        let mut host_order = Vec::new();

        // Allow override via REX_SERVER_SOCKET for isolated headless tests (`tsession`).
        let env_sock = std::env::var("REX_SERVER_SOCKET").ok().filter(|s| !s.is_empty());
        let home_addr = env_sock.unwrap_or_else(|| if settings.home_server_address.trim().is_empty() { "local".to_owned() } else { settings.home_server_address.clone() });
        let home_is_local = home_addr.eq_ignore_ascii_case("local") || home_addr.starts_with("unix://") || home_addr.starts_with('/');
        hosts.insert(
            "local".to_owned(),
            HostRuntime {
                key: "local".to_owned(),
                // (As Preferences ▸ Rex names it.)
                label: if home_is_local { olo_kit::words::tr("This computer").into() } else { home_addr.clone() },
                endpoint_raw: home_addr,
                endpoint: None,
                is_local: home_is_local,
                conn_state: HostConnState::Disconnected,
                connection_lost: None,
                client_id: None,
                reconnect_token: None,
                retry: None,
                server_status: None,
                server_version: None,
                helper: HostHelper::None,
                tailscale: None,
                tailscale_seq: 0,
                synced_hosts: Vec::new(),
                session_summaries: Vec::new(),
                sessions: HashMap::new(),
                pending_focus: HashMap::new(),
                keymap_bindings: Vec::new(),
                keymap_modes: Vec::new(),
                keymap_diagnostics: Vec::new(),
                control: None,
                input_tx: None,
                bridge_server: None,
                bridge_remote_port: None,
            },
        );
        host_order.push("local".to_owned());

        for sh in &settings.saved_hosts {
            if !hosts.contains_key(&sh.id) {
                hosts.insert(
                    sh.id.clone(),
                    HostRuntime {
                        key: sh.id.clone(),
                        label: sh.label.clone(),
                        endpoint_raw: sh.endpoint.clone(),
                        endpoint: None,
                        is_local: false,
                        conn_state: HostConnState::Disconnected,
                        connection_lost: None,
                        client_id: None,
                        reconnect_token: None,
                        retry: None,
                        server_status: None,
                        server_version: None,
                        helper: HostHelper::None,
                        tailscale: None,
                        tailscale_seq: 0,
                        synced_hosts: Vec::new(),
                        session_summaries: Vec::new(),
                        sessions: HashMap::new(),
                        pending_focus: HashMap::new(),
                        keymap_bindings: Vec::new(),
                        keymap_modes: Vec::new(),
                        keymap_diagnostics: Vec::new(),
                        control: None,
                        input_tx: None,
                        bridge_server: None,
                        bridge_remote_port: None,
                    },
                );
                host_order.push(sh.id.clone());
            }
        }

        let state = Arc::new(Mutex::new(HubState {
            hosts,
            host_order,
            blocks: HashMap::new(),
            forwards: Vec::new(),
            settings,
            active_theme,
            git_cache: GitCache::new(),
            local_bin_dir,
            quick_look_cache: HashMap::new(),
        }));

        let inner = Arc::new(RexHubInner { state: state.clone(), ssh_pool, askpass, notify: notify.clone() });

        let hub = Arc::new(Self { state, rt, notify, inner });

        // Isolated demo workspace (`OLO_REX_DEMO=1`) for `tools/dev/ui-lint` and offline visual QA:
        // populates realistic multi-pane splits, agent activity, git status, and a remote host
        // without touching any real Rex server socket or user files.
        if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") {
            hub.populate_demo_workspace();
            return hub;
        }

        // Zero-polling Linux rtnetlink connectivity watcher (`olo-net`): immediately reconnect
        // any disconnected/reconnecting remote hosts when Wi-Fi, Ethernet, or VPN changes.
        let hub_net = Arc::downgrade(&hub);
        olo_net::on_change(move || {
            if !olo_net::online() {
                return;
            }
            if let Some(h) = hub_net.upgrade() {
                let to_reconnect: Vec<String> = {
                    let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.hosts.values().filter(|hr| !hr.is_local && !matches!(hr.conn_state, HostConnState::Connected | HostConnState::Connecting)).map(|hr| hr.key.clone()).collect()
                };
                for hk in to_reconnect {
                    h.connect_host(&hk);
                }
            }
        });

        hub.connect_host("local");
        let auto_hosts: Vec<String> = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            st.settings.saved_hosts.iter().filter(|h| h.auto_connect).map(|h| h.id.clone()).collect()
        };
        for hid in auto_hosts {
            hub.connect_host(&hid);
        }

        hub
    }

    fn populate_demo_workspace(&self) {
        use olo_rex_client::{LayerView, LayoutNode, SplitDirection, SplitNode, WindowView};
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let theme = st.active_theme.clone();

        let mk_block = |id: &str, label: &str| BlockInfo {
            block_id: id.to_owned(),
            label: label.to_owned(),
            creator_name: "com.superlogical.terminal".to_owned(),
            flavor: "com.superlogical.terminal".to_owned(),
            host_id: "local".to_owned(),
            owner: None,
        };

        let left_layout = LayoutNode { rect: [0.0, 0.0, 0.54, 1.0], block_id: Some("demo-build".to_owned()), split: None };
        let top_right = LayoutNode { rect: [0.54, 0.0, 0.46, 0.56], block_id: Some("demo-agent".to_owned()), split: None };
        let bot_right = LayoutNode { rect: [0.54, 0.56, 0.46, 0.44], block_id: Some("demo-shell".to_owned()), split: None };
        let right_split = LayoutNode {
            rect: [0.54, 0.0, 0.46, 1.0],
            block_id: None,
            split: Some(SplitNode { split_id: "split-r".to_owned(), direction: SplitDirection::Vertical, ratio: 0.56, before: Box::new(top_right), after: Box::new(bot_right) }),
        };
        let root_layout = LayoutNode {
            rect: [0.0, 0.0, 1.0, 1.0],
            block_id: None,
            split: Some(SplitNode { split_id: "split-root".to_owned(), direction: SplitDirection::Horizontal, ratio: 0.54, before: Box::new(left_layout), after: Box::new(right_split) }),
        };

        let win1 = WindowView {
            window_id: "win-1".to_owned(),
            label: "olo-rex".to_owned(),
            focused_block_id: Some("demo-build".to_owned()),
            zoomed_block_id: None,
            layers: vec![LayerView {
                layer_id: "layer-tiled".to_owned(),
                kind: "tiled".to_owned(),
                bounds: [0.0, 0.0, 1.0, 1.0],
                state: Some("normal".to_owned()),
                layout: Some(root_layout),
                blocks: vec![mk_block("demo-build", "cargo test — olo-rex"), mk_block("demo-agent", "claude — Refactor VT renderer"), mk_block("demo-shell", "zsh — ~/olo/rex")],
            }],
        };
        let win2 = WindowView {
            window_id: "win-2".to_owned(),
            label: "server-logs".to_owned(),
            focused_block_id: Some("demo-logs".to_owned()),
            zoomed_block_id: None,
            layers: vec![LayerView {
                layer_id: "layer-logs".to_owned(),
                kind: "tiled".to_owned(),
                bounds: [0.0, 0.0, 1.0, 1.0],
                state: Some("normal".to_owned()),
                layout: Some(LayoutNode { rect: [0.0, 0.0, 1.0, 1.0], block_id: Some("demo-logs".to_owned()), split: None }),
                blocks: vec![mk_block("demo-logs", "rex server logs")],
            }],
        };

        let main_view = SessionView { revision: 1, session_id: "demo-main".to_owned(), label: "main".to_owned(), active_window_id: Some("win-1".to_owned()), windows: vec![win1, win2] };

        if let Some(local) = st.hosts.get_mut("local") {
            local.endpoint_raw = "unix:///run/user/1000/rex/server.sock".to_owned();
            local.conn_state = HostConnState::Connected;
            // (How reaching the demo's server from elsewhere stands, for the
            // Remote Access window's pictures: off, to be signed in, or on.)
            let state = |state: &str| olo_rex_client::TailscaleStatus { state: state.into(), ..Default::default() };
            local.tailscale = Some(match std::env::var("OLO_REX_DEMO_TAILSCALE").ok().as_deref() {
                Some("sign-in") => olo_rex_client::TailscaleStatus { auth_url: Some("https://login.example/a/1f2e3d4c5b6a".into()), ..state("needs_login") },
                Some("on") => olo_rex_client::TailscaleStatus { dns_name: Some("studio.example-tailnet.ts.net.".into()), ..state("running") },
                _ => state("disabled"),
            });
            local.server_version = Some(ServerVersion {
                version: "0.1.0-snapshot.2506".to_owned(),
                commit: "a4f8c21".to_owned(),
                api_revision: olo_rex_client::REX_API_REVISION,
                api_min_revision: olo_rex_client::REX_API_MIN_REVISION,
                ..Default::default()
            });
            local.server_status = Some(ServerStatus {
                instance_id: "demo-local".to_owned(),
                pid: std::process::id(),
                started_at: "2026-11-21T14:00:00Z".to_owned(),
                session_count: 1,
                listeners: Some(vec![olo_rex_client::ListenerInfo {
                    endpoint: "unix:///run/user/1000/rex/server.sock".to_owned(),
                    addresses: None,
                    transport: "unix".to_owned(),
                    admission: Some("local".to_owned()),
                    primary: true,
                }]),
                ..Default::default()
            });
            local.session_summaries = vec![SessionSummary { session_id: "demo-main".to_owned(), label: "main".to_owned() }];
            local.sessions.insert("demo-main".to_owned(), main_view);
        }

        // Populate realistic, crisp terminal contents via libghostty-vt sized cleanly for splits.
        let mut br_build = BlockRuntime::new(72, 34, &theme);
        // (No title of its own: back at its prompt, it says its shell
        // and folder, as a real pane does.)
        br_build.process = Some(demo_process("zsh"));
        br_build.pwd = "/home/you/olo/rex".to_owned();
        br_build.data_connected = true;
        br_build.terminal.vt_write(
            concat!(
                "\x1b[1;36myou@olo\x1b[0m \x1b[1;34m~/olo/rex\x1b[0m \x1b[33m(rex-client)\x1b[0m % cargo test -p olo-rex\r\n",
                "\x1b[1;32m   Compiling\x1b[0m olo-rex-vt v0.1.0\r\n",
                "\x1b[1;32m   Compiling\x1b[0m olo-rex-client v0.1.0\r\n",
                "\x1b[1;32m    Finished\x1b[0m `test` profile [optimized] in 1.42s\r\n",
                "\x1b[1;32m     Running\x1b[0m unittests (olo-rex-vt)\r\n\r\n",
                "running 3 tests\r\n",
                "test snapshot::ghostsnp_roundtrip ... \x1b[32mok\x1b[0m\r\n",
                "test render::dirty_row_extraction ... \x1b[32mok\x1b[0m\r\n",
                "test palette::cielab_256_palette ... \x1b[32mok\x1b[0m\r\n\r\n",
                "test result: \x1b[1;32mok\x1b[0m. 3 passed; 0 failed\r\n\r\n",
                "\x1b[1;32m     Running\x1b[0m unittests (olo-rex-client)\r\n",
                "test protocol::live_bootstrap_ghostsnp ... \x1b[32mok\x1b[0m\r\n",
                "test ssh::parse_ssh_tailscale_targets ... \x1b[32mok\x1b[0m\r\n",
                "test result: \x1b[1;32mok\x1b[0m. 2 passed; 0 failed\r\n\r\n",
                "\x1b[1;36myou@olo\x1b[0m \x1b[1;34m~/olo/rex\x1b[0m % "
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-build".into() }, Arc::new(Mutex::new(br_build)));

        let mut br_agent = BlockRuntime::new(56, 18, &theme);
        br_agent.title = "claude — Refactor VT renderer".to_owned();
        br_agent.pwd = "/home/you/olo/rex".to_owned();
        br_agent.agent_activity = AgentActivity::Working;
        br_agent.data_connected = true;
        br_agent.terminal.vt_write(
            concat!(
                "\x1b[1;35m◆ Claude Code\x1b[0m \x1b[2mv2.1 · rex-client\x1b[0m\r\n",
                "\x1b[2m────────────────────────────────────\x1b[0m\r\n",
                "\x1b[32m✓\x1b[0m Inspected \x1b[36mterminal_pane.rs\x1b[0m\r\n",
                "\x1b[32m✓\x1b[0m 120 Hz damage: 0 idle frames/s\r\n",
                "\x1b[33m⠋\x1b[0m Running visual QA at 1x, 1.5x, 2x…\r\n"
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-agent".into() }, Arc::new(Mutex::new(br_agent)));

        let mut br_shell = BlockRuntime::new(56, 14, &theme);
        // (A server running: the program its head names, the port it
        // listens on, no prompt after it.)
        br_shell.process = Some(demo_process("node"));
        br_shell.pwd = "/home/you/olo/rex".to_owned();
        br_shell.data_connected = true;
        br_shell.ports.ports.push(olo_rex_client::PortEntry {
            port: 3000,
            protocol: "tcp".to_owned(),
            address: "127.0.0.1".to_owned(),
            first_seen_at: "2026-11-21T14:00:00Z".to_owned(),
            pid: Some(4210),
        });
        br_shell.terminal.vt_write(
            concat!(
                "\x1b[1;36myou@olo\x1b[0m \x1b[1;34m~/olo/rex\x1b[0m % npm run dev\r\n\r\n",
                "\x1b[2m> rex-site@0.1.0 dev\x1b[0m\r\n",
                "\x1b[2m> node server.js\x1b[0m\r\n\r\n",
                "\x1b[32mready\x1b[0m in 412 ms\r\n",
                "Listening: \x1b[4;34mhttp://localhost:3000\x1b[0m\r\n"
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-shell".into() }, Arc::new(Mutex::new(br_shell)));

        let mut br_logs = BlockRuntime::new(110, 32, &theme);
        br_logs.title = "rex server logs".to_owned();
        br_logs.pwd = "/home/you".to_owned();
        br_logs.data_connected = true;
        br_logs.terminal.vt_write(b"\x1b[2m2026-11-21T14:00:01Z\x1b[0m \x1b[32mINFO\x1b[0m rex_server: listening on unix:///run/user/1000/rex/server.sock (rev 19)\r\n");
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-logs".into() }, Arc::new(Mutex::new(br_logs)));

        // The remote example owns a separate tab, screen and focus just like a connected host.
        let remote_block = BlockInfo { host_id: "dev-box".into(), ..mk_block("remote-shell", "Remote shell") };
        let remote_window = WindowView {
            window_id: "remote-tab".into(),
            label: "API console".into(),
            focused_block_id: Some(remote_block.block_id.clone()),
            zoomed_block_id: None,
            layers: vec![LayerView {
                layer_id: "remote-layer".into(),
                kind: "tiled".into(),
                bounds: [0.0, 0.0, 1.0, 1.0],
                state: Some("normal".into()),
                layout: Some(LayoutNode { rect: [0.0, 0.0, 1.0, 1.0], block_id: Some(remote_block.block_id.clone()), split: None }),
                blocks: vec![remote_block],
            }],
        };
        let remote = SessionView { revision: 1, session_id: "remote-api".into(), label: "api-prod".into(), active_window_id: Some(remote_window.window_id.clone()), windows: vec![remote_window] };
        let mut shell = BlockRuntime::new(110, 32, &theme);
        shell.title = "Remote shell".into();
        shell.data_connected = true;
        shell.terminal.vt_write(b"api-prod: service ready\r\n");
        st.blocks.insert(BlockKey { host_key: "dev-box".into(), session_id: remote.session_id.clone(), block_id: "remote-shell".into() }, Arc::new(Mutex::new(shell)));
        let demo_host = |key: &str, label: &str, endpoint: &str, conn_state: HostConnState, sessions: Vec<SessionView>| HostRuntime {
            key: key.to_owned(),
            label: label.to_owned(),
            endpoint_raw: endpoint.to_owned(),
            endpoint: None,
            is_local: false,
            conn_state,
            connection_lost: None,
            client_id: None,
            reconnect_token: None,
            retry: None,
            server_status: None,
            server_version: None,
            helper: HostHelper::None,
            tailscale: None,
            tailscale_seq: 0,
            synced_hosts: Vec::new(),
            session_summaries: sessions.iter().map(|s| SessionSummary { session_id: s.session_id.clone(), label: s.label.clone() }).collect(),
            sessions: sessions.into_iter().map(|s| (s.session_id.clone(), s)).collect(),
            pending_focus: HashMap::new(),
            keymap_bindings: Vec::new(),
            keymap_modes: Vec::new(),
            keymap_diagnostics: Vec::new(),
            control: None,
            input_tx: None,
            bridge_server: None,
            bridge_remote_port: None,
        };
        st.hosts.insert("dev-box".to_owned(), demo_host("dev-box", "dev-box", "ssh://you@dev-box.example", HostConnState::Connected, vec![remote]));
        st.host_order.push("dev-box".to_owned());
        // (`OLO_REX_DEMO_HOSTS=states`: hosts in the other states too,
        // for the Hosts window's pictures: connecting, disconnected, and
        // one that failed with a reason long enough to be cut.)
        if std::env::var("OLO_REX_DEMO_HOSTS").ok().as_deref() == Some("states") {
            for (key, label, endpoint, state) in [
                ("build", "build", "ssh://you@build.example", HostConnState::Connecting),
                ("nas", "nas", "nas.ts.net", HostConnState::Disconnected),
                ("staging", "staging", "ssh://deploy@staging.example:2222", HostConnState::Error("Permission denied: the host rejected all the keys your agent offered.".to_owned())),
            ] {
                st.hosts.insert(key.to_owned(), demo_host(key, label, endpoint, state, Vec::new()));
                st.host_order.push(key.to_owned());
            }
        }
    }

    pub fn connect_host(&self, host_key: &str) {
        let key = host_key.to_owned();
        let retry = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get_mut(host_key) else { return };
            if let Some(retry) = &host.retry {
                if matches!(host.conn_state, HostConnState::Reconnecting { .. }) {
                    retry.notify_one();
                }
                return;
            }
            let retry = Arc::new(tokio::sync::Notify::new());
            host.retry = Some(retry.clone());
            retry
        };
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            inner.run_host_connection(key, retry).await;
        });
    }

    /// The desktop's settings for Rex, as first read or as changed
    /// (`shared::apply`): hosts that are new there are shown (and
    /// connected, if they are to be as Rex starts), those gone are let go
    /// of, a name follows, an address that changed is a host connected
    /// afresh; permissions and notices are simply read where they are
    /// used.
    pub fn follow_shared(&self, rex: &olo_protocol::RexSettings) {
        if !crate::shared::settled(rex) {
            return;
        }
        let changes = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // (What Rex's own file held from before, if this start
            // couldn't read the desktop's settings to carry it over.)
            let pending = st.settings.before_preferences.is_some();
            let rex = crate::shared::adopt(&mut st.settings, rex.clone());
            let changes = crate::shared::apply(&rex, &mut st.settings);
            if changes.any || pending {
                st.settings.save();
            }
            for host in &changes.renamed {
                if let Some(h) = st.hosts.get_mut(&host.id) {
                    h.label.clone_from(&host.label);
                }
            }
            changes
        };
        if !changes.any {
            return;
        }
        for id in changes.removed.iter().chain(changes.moved.iter().map(|h| &h.id)) {
            self.remove_host_as(id, false, false);
        }
        for host in changes.added.iter().chain(&changes.moved) {
            self.add_host_as(host.id.clone(), host.label.clone(), host.endpoint.clone(), false, false, host.auto_connect);
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// A host the person adds in Rex itself: shown, connected, and told
    /// to the desktop's settings (where Preferences ▸ Rex lists it).
    pub fn add_and_connect_host(&self, id: String, label: String, endpoint: String, save_remote: bool) {
        self.add_host_as(id, label, endpoint, save_remote, true, true);
    }

    /// `tell_desktop`: written to the desktop's settings too (not when it
    /// came from there). `connect`: connected now.
    fn add_host_as(&self, id: String, label: String, endpoint: String, save_remote: bool, tell_desktop: bool, connect: bool) {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.hosts.contains_key(&id) {
                st.host_order.push(id.clone());
            }
            st.hosts.insert(
                id.clone(),
                HostRuntime {
                    key: id.clone(),
                    label: label.clone(),
                    endpoint_raw: endpoint.clone(),
                    endpoint: None,
                    is_local: false,
                    conn_state: HostConnState::Connecting,
                    connection_lost: None,
                    client_id: None,
                    reconnect_token: None,
                    retry: None,
                    server_status: None,
                    server_version: None,
                    helper: HostHelper::None,
                    tailscale: None,
                    tailscale_seq: 0,
                    synced_hosts: Vec::new(),
                    session_summaries: Vec::new(),
                    sessions: HashMap::new(),
                    pending_focus: HashMap::new(),
                    keymap_bindings: Vec::new(),
                    keymap_modes: Vec::new(),
                    keymap_diagnostics: Vec::new(),
                    control: None,
                    input_tx: None,
                    bridge_server: None,
                    bridge_remote_port: None,
                },
            );
            if !st.settings.saved_hosts.iter().any(|h| h.id == id) {
                st.settings.saved_hosts.push(SavedHost { id: id.clone(), label: label.clone(), endpoint: endpoint.clone(), icon: None, auto_connect: true, ssh_agent_socket: None });
                st.settings.save();
            }
        }
        (self.notify)(HubNotice::StateChanged);
        if tell_desktop {
            let host = olo_protocol::RexHost { id: id.clone(), label: label.clone(), endpoint: endpoint.clone(), connect_at_start: true, grants: Default::default() };
            olo_kit::settings::update(move |s| {
                if !s.rex.hosts.iter().any(|h| h.id == host.id) {
                    s.rex.hosts.push(host);
                }
            });
        }
        if save_remote {
            let rec = HostRecord { id: Some(id.clone()), label, endpoints: vec![endpoint], ..Default::default() };
            self.with_control("local", move |ctrl, _, _| async move {
                let _ = ctrl.hosts_put(&rec).await;
            });
        }
        if connect {
            self.connect_host(&id);
        }
    }

    /// A host the person removes in Rex itself: let go of here, in the
    /// desktop's settings and in the server's own list.
    pub fn remove_host(&self, host_key: &str) {
        self.remove_host_as(host_key, true, true);
    }

    fn remove_host_as(&self, host_key: &str, tell_server: bool, tell_desktop: bool) {
        if host_key == "local" {
            return;
        }
        let key = host_key.to_owned();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(host) = st.hosts.remove(&key) {
                if let Some(ctrl) = host.control {
                    ctrl.close();
                }
                if let Some(retry) = host.retry {
                    retry.notify_one();
                }
            }
            st.host_order.retain(|k| k != &key);
            st.forwards.retain(|f| f.host_key != key);
            st.retain_blocks(|bk| bk.host_key != key);
            st.settings.saved_hosts.retain(|h| h.id != key);
            st.settings.save();
        }
        if tell_desktop {
            let id = key.clone();
            olo_kit::settings::update(move |s| s.rex.hosts.retain(|h| h.id != id));
        }
        if tell_server {
            let id_copy = key.clone();
            self.with_control("local", move |ctrl, _, _| async move {
                let _ = ctrl.hosts_remove(&id_copy).await;
            });
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// Whether what is typed is drawn ahead of a slow host's echo, in
    /// every terminal (the Settings menu's choice).
    /// Olo's helper on the hosts, used or not (Settings ▸ Use Olo’s
    /// Typing Helper on Hosts). From each host's next connection: one that
    /// stands isn't remade for it.
    pub fn set_host_helper(&self, on: bool) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.settings.host_helper = on;
        st.settings.save();
        self.inner.ssh_pool.set_helper((on && crate::link::carried()).then_some(crate::link::ID));
    }

    pub fn set_type_ahead(&self, on: bool) {
        let blocks: Vec<_> = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.settings.type_ahead = on;
            st.settings.save();
            st.blocks.iter().map(|(key, block)| (key.clone(), block.clone())).collect()
        };
        for (key, block) in blocks {
            block.lock().unwrap_or_else(|e| e.into_inner()).predict.set_on(on);
            (self.notify)(HubNotice::BlockOutput(key));
        }
    }

    pub fn update_settings(&self, settings: Settings, new_theme: TerminalTheme) {
        let mut terms_to_theme = Vec::new();
        let mut live_blocks = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let theme_changed = st.active_theme != new_theme;
            st.settings = settings;
            st.settings.save();
            st.active_theme = new_theme.clone();
            if theme_changed {
                for (bk, br) in &st.blocks {
                    terms_to_theme.push(br.clone());
                    live_blocks.push(bk.clone());
                }
            }
        }
        for b in terms_to_theme {
            let mut guard = b.lock().unwrap_or_else(|e| e.into_inner());
            let _ = guard.apply_theme(&new_theme);
        }
        if !live_blocks.is_empty() {
            let (fg, bg, cur, pal, scheme) = themes::to_rex_theme_args(&new_theme);
            for bk in live_blocks {
                let (fg, bg, cur, pal, scheme) = (fg.clone(), bg.clone(), cur.clone(), pal.clone(), scheme.to_owned());
                let sid = bk.session_id.clone();
                let bid = bk.block_id.clone();
                self.with_control(&bk.host_key, move |ctrl, _, _| async move {
                    let _ = ctrl.terminal_set_theme(&sid, &bid, &fg, &bg, cur.as_deref(), &pal, &scheme).await;
                });
            }
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// Runs `work` on the hub's own threads (never the window's).
    pub fn spawn(&self, work: impl std::future::Future<Output = ()> + Send + 'static) -> tokio::task::AbortHandle {
        self.rt.spawn(work).abort_handle()
    }

    /// `target`, signed in to with the agent Rex is set to use.
    pub fn with_agent(&self, mut target: olo_rex_client::SshTarget) -> olo_rex_client::SshTarget {
        target.agent_socket = self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.ssh_agent_socket.clone().map(std::path::PathBuf::from);
        target
    }

    /// What runs commands on `target`'s host over SSH, as its account
    /// and as root (`root_key`: offered that key of the agent's alone):
    /// for setting a host up and looking after it.
    pub fn over_ssh(&self, target: olo_rex_client::SshTarget, root_key: Option<String>) -> olo_rex_client::provision::OverSsh {
        olo_rex_client::provision::OverSsh { pool: self.inner.ssh_pool.clone(), target, root_key, askpass: self.inner.askpass.clone() }
    }

    /// Lets the SSH connection to `target` go (root's, when its work is
    /// done; an account's, to be made anew).
    pub async fn forget_ssh(&self, target: &olo_rex_client::SshTarget) {
        self.inner.ssh_pool.invalidate(target).await;
    }

    /// The keys the agent holds now, to choose among.
    pub async fn agent_keys(&self) -> Vec<olo_rex_client::AgentKey> {
        let agent = self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.ssh_agent_socket.clone().map(std::path::PathBuf::from);
        olo_rex_client::inspect_ssh_agent(agent.as_deref()).await.keys
    }

    fn with_control<F, Fut>(&self, host_key: &str, f: F) -> Option<tokio::task::AbortHandle>
    where
        F: FnOnce(ControlClient, RexEndpoint, Arc<RexHubInner>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let found = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| Some((h.control.clone()?, h.endpoint.clone()?)))
        };
        let (ctrl, ep) = found?;
        Some(self.rt.spawn(f(ctrl, ep, self.inner.clone())).abort_handle())
    }

    fn local_shell_opts(&self, host_key: &str, cwd: Option<String>) -> Option<Value> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let is_local = st.hosts.get(host_key).is_some_and(|h| h.is_local);
        let mut opts = json!({});
        let mut any = false;
        if let Some(d) = cwd {
            opts["cwd"] = json!(d);
            any = true;
        }
        if is_local {
            if let Some(bin) = &st.local_bin_dir {
                opts["path_prepend"] = json!([bin.to_string_lossy()]);
                any = true;
            }
            if let Some(open_bin) = olo_xdg::find_program("olo-open") {
                opts["env"] = json!({ "BROWSER": open_bin.to_string_lossy() });
                any = true;
            }
        }
        any.then_some(opts)
    }

    pub fn create_session(&self, host_key: &str, label: Option<String>, cwd: Option<String>, feedback: Feedback) {
        let hk = host_key.to_owned();
        let opts = self.local_shell_opts(host_key, cwd);
        if self
            .with_control(host_key, move |ctrl, ep, inner| async move {
                match ctrl.session_create(label.as_deref(), opts).await {
                    Ok(res) => {
                        if let Some(sid) = res.payload.get("session_id").and_then(Value::as_str)
                            && let Ok(att) = ctrl.session_attach(sid).await
                        {
                            inner.ingest_session(&hk, &ctrl, &ep, att.payload.view, &[]).await;
                            feedback.created(hk, sid.to_owned());
                        }
                    }
                    Err(e) => feedback.problem("Couldn’t create the session.", e),
                }
            })
            .is_none()
        {
            feedback.problem("Couldn’t create the session.", "The Rex server is disconnected.");
        }
    }

    /// Gives a session its name (none: the server's own for it).
    pub fn rename_session(&self, host_key: &str, session_id: &str, label: String, feedback: Feedback) {
        let (hk, sid) = (host_key.to_owned(), session_id.to_owned());
        if self
            .with_control(host_key, move |ctrl, ep, inner| async move {
                match ctrl.session_set_label(&sid, &label).await {
                    // (Shown at once: the list of sessions read again.)
                    Ok(_) => {
                        let _ = inner.refresh_host(&hk, &ctrl, &ep).await;
                        (inner.notify)(HubNotice::StateChanged);
                    }
                    Err(e) => feedback.problem("Couldn’t rename the session.", e),
                }
            })
            .is_none()
        {
            feedback.problem("Couldn’t rename the session.", "The Rex server is disconnected.");
        }
    }

    pub fn close_session(&self, host_key: &str, session_id: &str, feedback: Feedback) {
        let sid = session_id.to_owned();
        if self
            .with_control(host_key, move |ctrl, _, _| async move {
                if let Err(e) = ctrl.session_destroy(&sid).await {
                    feedback.problem("Couldn’t end the session.", e);
                }
            })
            .is_none()
        {
            feedback.problem("Couldn’t end the session.", "The Rex server is disconnected.");
        }
    }

    pub fn create_window(&self, host_key: &str, session_id: &str, cwd: Option<String>) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let opts = self.local_shell_opts(host_key, cwd);
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_new_window(&sid, None, opts).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn select_window(&self, host_key: &str, session_id: &str, window_id: &str) {
        self.select(host_key, session_id, window_id, None);
    }

    /// Gives a tab its name (none: it shows what runs in it again).
    pub fn rename_window(&self, host_key: &str, session_id: &str, window_id: &str, title: String, feedback: Feedback) {
        let (hk, sid, wid) = (host_key.to_owned(), session_id.to_owned(), window_id.to_owned());
        if self
            .with_control(host_key, move |ctrl, ep, inner| async move {
                match ctrl.session_set_window_label(&sid, &wid, &title).await {
                    Ok(_) => {
                        if let Ok(v) = ctrl.session_view(&sid).await {
                            inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
                        }
                        (inner.notify)(HubNotice::StateChanged);
                    }
                    Err(e) => feedback.problem("Couldn’t rename the tab.", e),
                }
            })
            .is_none()
        {
            feedback.problem("Couldn’t rename the tab.", "The Rex server is disconnected.");
        }
    }

    pub fn close_window(&self, host_key: &str, session_id: &str, window_id: &str) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let wid = window_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_close_window(&sid, Some(&wid)).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    /// Moves a tab before another of its session's (none: to the end).
    pub fn move_window(&self, host_key: &str, session_id: &str, window_id: &str, before_window_id: Option<String>) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let wid = window_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_move_window(&sid, &wid, before_window_id.as_deref()).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn split_pane(&self, host_key: &str, session_id: &str, target_block_id: &str, direction: &str, cwd: Option<String>) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let bid = target_block_id.to_owned();
        let dir = direction.to_owned();
        let opts = self.local_shell_opts(host_key, cwd);
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_new_split(&sid, Some(&bid), &dir, opts).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn close_block(&self, host_key: &str, session_id: &str, block_id: &str) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let bid = block_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.block_close(&sid, &bid).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn focus_block(&self, host_key: &str, session_id: &str, block_id: &str) {
        self.select(host_key, session_id, "", Some(block_id));
    }

    fn select(&self, host_key: &str, session_id: &str, window_id: &str, block: Option<&str>) {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get_mut(host_key) else { return };
            let Some(view) = host.sessions.get_mut(session_id) else { return };
            let (window, restore) = match block {
                Some(b) => {
                    let Some((w, l, _)) = view.find_block(b) else { return };
                    (w, (l.layer_state() == olo_rex_client::LayerState::Minimized).then(|| l.layer_id.clone()))
                }
                None => {
                    let Some(w) = view.windows.iter().find(|w| w.window_id == window_id) else { return };
                    (w, None)
                }
            };
            let switch = view.active_window_id.as_ref() != Some(&window.window_id);
            if !switch && restore.is_none() && block.is_none_or(|b| window.focused_block_id.as_deref() == Some(b)) {
                return;
            }
            let target = Arc::new(FocusTarget { window: window.window_id.clone(), block: block.map(str::to_owned), restore });
            target.apply(view);
            if let Some(tx) = &host.input_tx {
                let op = HostInputOp::Focus { session_id: session_id.to_owned(), switch, target: target.clone() };
                if tx.send(op).is_ok() {
                    host.pending_focus.insert(session_id.to_owned(), target);
                }
            }
        }
        (self.notify)(HubNotice::StateChanged);
    }

    pub fn focus_direction(&self, host_key: &str, session_id: &str, direction: &str) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let dir = direction.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_focus_direction(&sid, &dir).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn set_split_ratio(&self, host_key: &str, session_id: &str, split_id: &str, ratio: f64) {
        let sid = session_id.to_owned();
        let spid = split_id.to_owned();
        self.with_control(host_key, move |ctrl, _, _| async move {
            let _ = ctrl.session_resize_pane(&sid, &spid, ratio.clamp(0.1, 0.9)).await;
        });
    }

    pub fn toggle_zoom(&self, host_key: &str, session_id: &str, block_id: Option<String>) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_zoom_block(&sid, block_id.as_deref(), None).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn create_floating_layer(&self, host_key: &str, session_id: &str, bounds: [f64; 4]) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let opts = self.local_shell_opts(host_key, None);
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_new_layer(&sid, None, bounds, opts).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn resize_layer(&self, host_key: &str, session_id: &str, layer_id: &str, bounds: [f64; 4]) {
        let sid = session_id.to_owned();
        let lid = layer_id.to_owned();
        self.with_control(host_key, move |ctrl, _, _| async move {
            let _ = ctrl.session_resize_layer(&sid, &lid, bounds).await;
        });
    }

    pub fn set_layer_minimized(&self, host_key: &str, session_id: &str, layer_id: &str, minimized: bool) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let lid = layer_id.to_owned();
        let state = if minimized { "minimized" } else { "normal" };
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_set_layer_state(&sid, &lid, state, !minimized).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    pub fn raise_layer(&self, host_key: &str, session_id: &str, layer_id: &str) {
        let sid = session_id.to_owned();
        let lid = layer_id.to_owned();
        self.with_control(host_key, move |ctrl, _, _| async move {
            let _ = ctrl.session_raise_layer(&sid, &lid).await;
        });
    }

    fn writer_for_key(&self, key: &BlockKey, expected: Option<&ControlClient>) -> Option<tokio::sync::mpsc::UnboundedSender<HostInputOp>> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(&key.host_key)?;
        if expected.is_some_and(|c| !host.control.as_ref().is_some_and(|live| live.same_connection(c))) {
            return None;
        }
        let queue = host.input_tx.as_ref()?;
        let block = st.blocks.get(key)?.lock().unwrap_or_else(|e| e.into_inner());
        if !block.data_connected || !block.input_ready {
            return None;
        }
        // Probe when the key is accepted: the FIFO writer may already be waiting on a
        // silent link. A Unix socket tells us directly when its peer goes away.
        if matches!(host.endpoint, Some(RexEndpoint::Ssh { .. } | RexEndpoint::Http { .. } | RexEndpoint::Https { .. })) {
            host.control.as_ref()?.probe();
        }
        Some(queue.clone())
    }

    pub fn send_input(&self, key: &BlockKey, data: Vec<u8>) {
        self.write_input(key, data, None);
    }

    pub fn write_input(&self, key: &BlockKey, data: Vec<u8>, connection: Option<&ControlClient>) {
        if let Some(tx) = self.writer_for_key(key, connection) {
            let seq = self.input_typed(key, &data);
            let text = String::from_utf8_lossy(&data).into_owned();
            let _ = tx.send(HostInputOp::Write { session_id: key.session_id.clone(), block_id: key.block_id.clone(), text, seq });
        }
    }

    /// What was typed into a terminal, told to its predictions
    /// (`predict`): its number among what is typed there. Where it
    /// changes what is drawn (a letter shown at once), the pane is
    /// drawn again.
    fn input_typed(&self, key: &BlockKey, data: &[u8]) -> u64 {
        let Some(block) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned() else { return 0 };
        let (seq, changed) = {
            let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
            // (Typed on what was last drawn of it.)
            let screen = b.caret_screen();
            b.predict.typed(data, screen.as_ref(), Instant::now())
        };
        if changed {
            (self.notify)(HubNotice::BlockOutput(key.clone()));
        }
        seq
    }

    /// From a paint: the predictions of the terminal it draws wait on
    /// time (`at`) for something.
    pub fn wake_predictions(&self, block: &Arc<Mutex<BlockRuntime>>, held: &mut BlockRuntime, key: &BlockKey, at: Instant) {
        predict_wake(self.rt.handle(), block, held, key, &self.notify, at);
    }

    pub fn send_rex_key(&self, key: &BlockKey, event: RexKeyEvent) {
        if let Some(tx) = self.writer_for_key(key, None) {
            // (A key the host's own bindings are asked about: not one to predict.)
            let seq = self.input_typed(key, &[0x1b]);
            let _ = tx.send(HostInputOp::Key { session_id: key.session_id.clone(), block_id: key.block_id.clone(), event, seq });
        }
    }

    pub fn prepare_images(&self, block: Arc<Mutex<BlockRuntime>>, bk: BlockKey, owner: u64, keys: Vec<(u32, u64)>) {
        let notify = self.notify.clone();
        self.rt.spawn_blocking(move || {
            for key in keys {
                let decoded = {
                    let g = block.lock().unwrap_or_else(|e| e.into_inner());
                    if g.frame.owner != owner {
                        return;
                    }
                    if !g.frame.textures.contains_key(&key) {
                        continue;
                    }
                    g.terminal.kitty_image_rgba(key.0)
                };
                let Some(d) = decoded.filter(|d| d.generation == key.1) else { continue };
                let image = crate::terminal_pane::rgba_to_render_image(&d.rgba, d.width, d.height);
                let mut g = block.lock().unwrap_or_else(|e| e.into_inner());
                if g.frame.owner != owner {
                    return;
                }
                if let Some(slot) = g.frame.textures.get_mut(&key) {
                    *slot = image;
                }
            }
            notify(HubNotice::BlockOutput(bk));
        });
    }
    pub fn find(&self, bk: &BlockKey, query: Option<&str>) {
        let block = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(bk).cloned();
        if let Some(block) = block {
            let generation = {
                let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                if b.set_find_query(query).is_err() || !b.finding {
                    return;
                }
                b.find_generation
            };
            let _guard = self.rt.enter();
            self.inner.search_on_worker(block, generation);
        }
    }

    pub fn resize_block(&self, key: &BlockKey, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        let br_opt = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned();
        let changed = if let Some(br) = br_opt {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            !g.other_size_owner && g.resize(cols, rows, cell_w, cell_h).unwrap_or(false)
        } else {
            false
        };
        if !changed {
            return;
        }
        let sid = key.session_id.clone();
        let bid = key.block_id.clone();
        self.with_control(&key.host_key, move |ctrl, _, _| async move {
            let _ = ctrl.terminal_resize(&sid, &bid, cols, rows, Some(cell_w as u16), Some(cell_h as u16)).await;
        });
    }

    pub fn respond_prompt(&self, host_key: &str, call_id: &str, cancelled: bool, value: Option<Value>) {
        let cid = call_id.to_owned();
        self.with_control(host_key, move |ctrl, _, _| async move {
            if cancelled {
                let _ = ctrl.client_reply_err(&cid, "cancelled", "Prompt cancelled by user").await;
            } else {
                let _ = ctrl.client_reply_ok(&cid, value.unwrap_or(Value::Null)).await;
            }
        });
    }

    pub fn reload_server_config(&self, host_key: &str, feedback: Feedback) {
        if self
            .with_control(host_key, move |ctrl, _, _| async move {
                match ctrl.server_config_reload().await {
                    Ok(_) => feedback.say("Server configuration reloaded."),
                    Err(e) => feedback.problem("Couldn’t reload the server configuration.", e),
                }
            })
            .is_none()
        {
            feedback.problem("Couldn’t reload the server configuration.", "The Rex server is disconnected.");
        }
    }

    pub fn request_dir_listing(&self, key: &BlockKey, path: String, files: bool, ready: impl FnOnce(DirListing) + Send + 'static) {
        let (sid, bid) = (key.session_id.clone(), key.block_id.clone());
        self.with_control(&key.host_key, move |ctrl, _, _| async move {
            if let Ok(res) = ctrl.terminal_list_dir(&sid, &bid, &path, None, files).await {
                ready(res.payload);
            }
        });
    }
    /// Download a file via `com.superlogical.terminal.download_begin` + `rex.data.bytes.v1`
    /// with `if_none_match` ETag caching for instant Quick Look (`⌘Y`).
    pub fn quick_look_path(&self, key: &BlockKey, path: String, feedback: Feedback) -> Option<tokio::task::AbortHandle> {
        let cache_key = (key.host_key.clone(), key.block_id.clone(), path.clone());
        let (sid, bid) = (key.session_id.clone(), key.block_id.clone());
        let cached = self.state.lock().unwrap_or_else(|e| e.into_inner()).quick_look_cache.get(&cache_key).cloned();
        let job = self.with_control(&key.host_key, move |ctrl, ep, inner| async move {
            let received = async {
                let data_ref = transfer_reference("download");
                let version = cached.as_ref().and_then(|e| e.version.as_deref());
                let resp = ctrl.terminal_download_begin(&sid, &bid, &data_ref, &path, version).await?.payload;
                if resp.not_modified {
                    return cached.map(|e| e.preview).ok_or_else(|| olo_rex_client::RexError::Protocol("No cached preview for this file.".into()));
                }
                let ticket = resp.ticket.ok_or_else(|| olo_rex_client::RexError::Protocol("The server didn’t provide a download ticket.".into()))?;
                let (stream, header) = ep.connect_stream().await?;
                let mut stream = BytesDataStream::connect_hello(stream, &header, &ticket).await?;
                let mut bytes = Vec::new();
                while !preview_complete(&mut bytes, resp.size, stream.read_chunk().await)? {}
                let preview = QuickLookContent::from_bytes(&path, &bytes, None);
                inner.state.lock().unwrap_or_else(|e| e.into_inner()).quick_look_cache.insert(cache_key, QuickLookCacheEntry { version: resp.version, preview: preview.clone() });
                Ok(preview)
            }
            .await;
            match received {
                Ok(preview) => feedback.preview(path, preview),
                Err(e) => feedback.problem("Couldn’t preview the file.", e),
            }
        });
        if job.is_none() {
            feedback.problem("Couldn’t preview the file.", "The Rex server is disconnected.");
        }
        job
    }

    /// Upload dropped files into a remote Rex block via `upload_begin` + `rex.data.bytes.v1`
    /// (0-byte half-close EOF triggers Rex's automatic path paste!), or shell-escape paths directly on Local.
    pub fn upload_files(&self, key: &BlockKey, paths: Vec<PathBuf>, feedback: Feedback) {
        if paths.is_empty() {
            return;
        }
        let is_local = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&key.host_key).is_some_and(|h| h.is_local);

        if is_local {
            let quoted: Vec<String> = paths.iter().map(|p| shell_quote(&p.to_string_lossy())).collect();
            let mut text = quoted.join(" ");
            text.push(' ');
            self.send_input(key, text.into_bytes());
            return;
        }

        let bk = key.clone();
        self.with_control(&key.host_key, move |ctrl, ep, inner| async move {
            let total = paths.len();
            let batch_id = uuid::Uuid::new_v4().to_string();
            for (idx, path) in paths.into_iter().enumerate() {
                let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".to_owned());
                let uploaded = async {
                    use tokio::io::AsyncReadExt;
                    let mut file = tokio::fs::File::open(&path).await?;
                    let size = file.metadata().await?.len();
                    let data_ref = transfer_reference("upload");
                    let batch_info = (total > 1).then_some((batch_id.as_str(), idx, total));
                    let res = ctrl.terminal_upload_begin(&bk.session_id, &bk.block_id, &data_ref, &filename, size, batch_info).await?;
                    let (stream, header) = ep.connect_stream().await?;
                    let mut ds = BytesDataStream::connect_hello(stream, &header, &res.payload.ticket).await?;
                    let mut buf = vec![0; 65536];
                    let mut sent = 0u64;
                    let mut last = None;
                    loop {
                        let count = file.read(&mut buf).await?;
                        if count == 0 {
                            break;
                        }
                        sent += count as u64;
                        if sent > size {
                            return Err(olo_rex_client::RexError::Protocol("File changed during upload.".into()));
                        }
                        ds.write_all(&buf[..count]).await?;
                        if last.is_none_or(|t: Instant| t.elapsed() >= Duration::from_millis(100)) {
                            if let Some(br) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                                br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress = Some((filename.clone(), sent as f32 / size as f32));
                            }
                            (inner.notify)(HubNotice::StateChanged);
                            last = Some(Instant::now());
                        }
                    }
                    if sent != size {
                        return Err(olo_rex_client::RexError::Protocol("File changed during upload.".into()));
                    }
                    ds.finish_write().await
                }
                .await;
                if let Err(e) = uploaded {
                    feedback.problem(&format!("Couldn’t upload {filename}."), e);
                    break;
                }
            }
            if let Some(br) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress = None;
            }
            (inner.notify)(HubNotice::StateChanged);
        });
    }

    /// A port of this computer, opened in the browser: it needs no forward.
    pub fn open_local_port(&self, port: u16, feedback: Feedback) {
        self.rt.spawn(async move {
            if let Err(e) = bridge::open_url_with_autoforward(&format!("http://127.0.0.1:{port}"), false, None).await {
                feedback.problem("Couldn’t open the page.", e);
            }
        });
    }

    pub fn add_local_forward(&self, host_key: &str, session_id: &str, remote_host: String, remote_port: u16, local_port: u16, open_in_browser: bool, feedback: Feedback) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        if self
            .with_control(host_key, move |ctrl, ep, inner| async move {
                match inner.create_forward(&hk, &ctrl, ep, &sid, remote_host, remote_port, local_port).await {
                    Ok(local_addr) => {
                        if open_in_browser {
                            let url = format!("http://127.0.0.1:{}", local_addr.port());
                            let _ = bridge::open_url_with_autoforward(&url, false, None).await;
                        }
                    }
                    Err(e) => feedback.problem("Couldn’t forward the port.", e),
                }
            })
            .is_none()
        {
            feedback.problem("Couldn’t forward the port.", "The Rex server is disconnected.");
        }
    }

    pub fn remove_forward(&self, forward_id: &str) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.forwards.retain(|f| f.id != forward_id);
        drop(st);
        (self.notify)(HubNotice::StateChanged);
    }

    pub fn open_link_from_pane(&self, key: &BlockKey, url: String, feedback: Feedback) {
        let (is_local, policy) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let is_local = st.hosts.get(&key.host_key).is_some_and(|h| h.is_local);
            let policy = st.settings.policy_for_host(&key.host_key, is_local);
            (is_local, policy)
        };
        let bk = key.clone();
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            let fwd: Option<bridge::LocalhostForwardFn> = if !is_local {
                let hk = bk.host_key.clone();
                let sid = bk.session_id.clone();
                let inn = inner.clone();
                Some(Arc::new(move |rhost: String, rport: u16| {
                    let hk = hk.clone();
                    let sid = sid.clone();
                    let inn = inn.clone();
                    Box::pin(async move { inn.ensure_ephemeral_forward(&hk, &sid, &rhost, rport).await })
                }))
            } else {
                None
            };
            if let Err(e) = bridge::open_url_with_autoforward(&url, !is_local && policy.allow_localhost_autoforward, fwd.as_ref()).await {
                feedback.problem("Couldn’t open the link.", e);
            }
        });
    }

    pub fn install_remote_shim_in_pane(&self, key: &BlockKey) {
        let script = bridge::generate_remote_shim_install_script();
        let cmd = format!("sh -c {}\n", shell_quote(&script));
        self.send_input(key, cmd.into_bytes());
    }

    pub fn install_remote_skill_in_pane(&self, key: &BlockKey) {
        let script = bridge::generate_remote_skill_install_script();
        let cmd = format!("sh -c {}\n", shell_quote(&script));
        self.send_input(key, cmd.into_bytes());
    }

    pub fn git_info_for(&self, key: &BlockKey, pwd: &str) -> Option<GitInfo> {
        let (is_local, cache) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let is_local = st.hosts.get(&key.host_key).is_some_and(|h| h.is_local);
            (is_local, st.git_cache.clone())
        };
        if !is_local || pwd.is_empty() {
            return None;
        }
        let notify = self.notify.clone();
        cache.get_or_refresh(Path::new(pwd), move || {
            notify(HubNotice::StateChanged);
        })
    }

    pub fn total_vt_memory(&self) -> (usize, u64, u64) {
        let blocks: Vec<Arc<Mutex<BlockRuntime>>> = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.values().cloned().collect();
        let mut res = 0u64;
        let mut img = 0u64;
        for b in &blocks {
            let m = b.lock().unwrap_or_else(|e| e.into_inner()).memory_usage();
            res += m.resident_bytes;
            img += m.image_bytes;
        }
        (blocks.len(), res, img)
    }

    fn open_sign_in(&self, url: &str, feedback: Feedback) {
        let matches = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get("local").and_then(|h| h.tailscale.as_ref()).and_then(crate::tailscale::login_link) == Some(url);
        if matches {
            let url = url.to_owned();
            self.rt.spawn(async move {
                if let Err(e) = bridge::open_url_with_autoforward(&url, false, None).await {
                    feedback.failed(crate::tailscale::Action::OpenLink(url).server_problem(&e.to_string()));
                }
            });
        }
    }

    pub fn change_tailscale(&self, host_key: &str, action: crate::tailscale::Action, feedback: Feedback) {
        let hk = host_key.to_owned();
        if let crate::tailscale::Action::OpenLink(url) = &action {
            self.open_sign_in(url, feedback);
            return;
        }
        if self
            .with_control(host_key, move |ctrl, _, inner| async move {
                let res = match action {
                    crate::tailscale::Action::Enable => ctrl.tailscale_enable(None, Some(true), None, None).await,
                    crate::tailscale::Action::Disable => ctrl.tailscale_disable().await,
                    crate::tailscale::Action::Login => ctrl.tailscale_login().await,
                    crate::tailscale::Action::OpenLink(_) => unreachable!(),
                };
                match res {
                    Ok(r) => {
                        let status = {
                            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                            st.hosts
                                .get_mut(&hk)
                                .filter(|h| h.control.is_some() && h.client_id.as_deref() == ctrl.client_id())
                                .and_then(|h| crate::tailscale::apply(&mut h.tailscale, &mut h.tailscale_seq, r.seq, r.payload).then(|| h.tailscale.clone()).flatten())
                        };
                        if let Some(status) = status {
                            if let Some(error) = status.error {
                                feedback.failed(action.server_problem(&error));
                            } else if action == crate::tailscale::Action::Login && status.state == "needs_login" && crate::tailscale::login_link(&status).is_none() {
                                feedback.say("Waiting for a sign-in link…");
                            }
                        }
                        (inner.notify)(HubNotice::StateChanged);
                    }
                    Err(e) => feedback.failed(action.failed(&e)),
                }
            })
            .is_none()
        {
            feedback.failed((olo_kit::words::tr("The Rex server isn’t connected.").into(), None));
        }
    }
}

impl RexHubInner {
    /// A host reached over SSH is seen to have the helper the app
    /// carries (where the setting has it and one is carried for such a
    /// host): asked, and where it hasn't, given it, off the
    /// connection's own path. A connection that went straight to the
    /// server uses it from the host's next connection; one that went
    /// through an older build of the helper goes on through that.
    /// Nothing is asked of the person: it is one file in the account's
    /// own home that does nothing until the app runs it
    /// (`olo_rex_client::helper`).
    fn look_after_helper(self: &Arc<Self>, host_key: &str, endpoint: &RexEndpoint) {
        let RexEndpoint::Ssh { target, pool, askpass } = endpoint else { return };
        if !crate::link::carried() || !self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.host_helper {
            return;
        }
        let (target, pool, askpass) = (target.clone(), pool.clone(), askpass.clone());
        let (hub, host_key) = (self.clone(), host_key.to_owned());
        let set = move |hub: &RexHubInner, host_key: &str, helper: HostHelper| {
            if let Some(host) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(host_key).filter(|h| h.helper != HostHelper::Used) {
                host.helper = helper;
            }
            (hub.notify)(HubNotice::StateChanged);
        };
        set(&hub, &host_key, HostHelper::Installing);
        tokio::spawn(async move {
            use olo_rex_client::helper::{Stands, ensure};
            let helper = match ensure(&pool, &target, askpass, crate::link::ID, &crate::link::BUILDS).await {
                Ok(Stands::Already) => {
                    tracing::info!("rex: Olo's helper is on host {host_key} already, for its next connection");
                    HostHelper::Ready
                }
                Ok(Stands::Installed { bytes, took }) => {
                    tracing::info!("rex: Olo's helper put on host {host_key}: {bytes} bytes in {} ms", took.as_millis());
                    // (Something of Olo's now lies on the person's host:
                    // said, once, as it happens.)
                    let label = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).map(|h| h.label.clone()).unwrap_or_default();
                    (hub.notify)(HubNotice::Said(olo_kit::tr!("Olo’s typing helper was added to your account on “{}” and will be used the next time you connect.", label).to_string()));
                    HostHelper::Ready
                }
                Ok(Stands::NoBuild(kind)) => {
                    tracing::info!("rex: no helper is carried for host {host_key} ({kind})");
                    HostHelper::Without(olo_kit::tr!("there isn’t one for {}", kind).to_string())
                }
                Err(e) => {
                    tracing::warn!("rex: Olo's helper couldn't be put on host {host_key}: {e}");
                    HostHelper::Without(e.to_string())
                }
            };
            set(&hub, &host_key, helper);
        });
    }

    /// What was typed up to `seq` in a terminal leaves for the host
    /// now: its predictions are told (an echo can't be back sooner
    /// than a round trip from this).
    fn input_went(&self, host_key: &str, session_id: &str, block_id: &str, seq: u64) {
        let key = BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: block_id.to_owned() };
        if let Some(block) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&key).cloned() {
            let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            b.predict.went(seq, now);
            // (Its silence, if it comes to that, is said when it has
            // lasted: one wait, begun here.)
            if let Some(at) = b.predict.stall_changes(now) {
                predict_wake(&tokio::runtime::Handle::current(), &block, &mut b, &key, &self.notify, at);
            }
        }
    }

    /// The host answered what was typed up to `seq` in a terminal: its
    /// predictions are told, and what the host then doesn't echo in
    /// time is given up (looked at once, at that time).
    fn input_answered(&self, host_key: &str, session_id: &str, block_id: &str, seq: u64) {
        let key = BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: block_id.to_owned() };
        let Some(block) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&key).cloned() else { return };
        let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let seen = b.predict.answered(seq, now);
        let (next, changed, chrome) = b.predict_tick(now);
        let changed = changed || seen;
        if let Some(at) = next {
            predict_wake(&tokio::runtime::Handle::current(), &block, &mut b, &key, &self.notify, at);
        }
        drop(b);
        if changed {
            (self.notify)(HubNotice::BlockOutput(key));
        }
        if chrome {
            (self.notify)(HubNotice::StateChanged);
        }
    }

    /// Something the app itself writes into a terminal (a nudge to an
    /// agent to keep going): not typed, and not predicted, but the
    /// terminal's predictions are told that the screen will change by
    /// it.
    fn input_by_itself(&self, key: &BlockKey) {
        if let Some(block) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned() {
            let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let (seq, _) = b.predict.typed(&[0x1b], None, now);
            b.predict.went(seq, now);
            b.predict.answered(seq, now);
        }
    }

    fn search_on_worker(&self, block: Arc<Mutex<BlockRuntime>>, generation: u64) {
        let notify = self.notify.clone();
        tokio::task::spawn_blocking(move || {
            let mut last = Instant::now();
            loop {
                let done = {
                    let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                    if generation != b.find_generation {
                        return;
                    }
                    b.find_step(generation).unwrap_or_else(|_| {
                        b.finding = false;
                        true
                    })
                };
                if done || last.elapsed() >= Duration::from_millis(100) {
                    notify(HubNotice::StateChanged);
                    last = Instant::now();
                }
                if done {
                    return;
                }
                std::thread::yield_now();
            }
        });
    }
    fn clipboard_allowed(&self, host: &str) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.hosts.get(host).is_some_and(|h| st.settings.policy_for_host(host, h.is_local).allow_remote_control)
    }

    async fn ensure_ephemeral_forward(&self, host_key: &str, session_id: &str, remote_host: &str, remote_port: u16) -> Result<u16, String> {
        let (ctrl, ep) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(f) = st.forwards.iter().find(|f| f.host_key == host_key && f.handle.session_id == session_id && f.remote_host == remote_host && f.remote_port == remote_port) {
                return match &f.state {
                    ForwardState::Active => Ok(f.local_addr.port()),
                    ForwardState::Waiting => Err("The forward is waiting for its host.".into()),
                    ForwardState::Failed(why) => Err(why.clone()),
                };
            }
            st.hosts.get(host_key).and_then(|h| Some((h.control.clone()?, h.endpoint.clone()?))).ok_or_else(|| format!("host '{host_key}' is not connected"))?
        };
        self.create_forward(host_key, &ctrl, ep, session_id, remote_host.to_owned(), remote_port, 0).await.map(|a| a.port())
    }

    async fn create_forward(&self, host_key: &str, ctrl: &ControlClient, ep: RexEndpoint, session_id: &str, remote_host: String, remote_port: u16, local_port: u16) -> Result<SocketAddr, String> {
        let handle = start_local_forward(ctrl.clone(), ep, session_id.into(), remote_host.clone(), remote_port, local_port).await.map_err(|e| e.to_string())?;
        let local_addr = handle.local_addr;
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.hosts.get(host_key).and_then(|h| h.control.as_ref()).is_some_and(|c| c.same_connection(ctrl)) {
                return Err("The host disconnected while opening the forward.".into());
            }
            st.forwards.push(ActiveForward { id: uuid::Uuid::new_v4().to_string(), host_key: host_key.into(), remote_host, remote_port, local_addr, state: ForwardState::Active, handle });
        }
        (self.notify)(HubNotice::StateChanged);
        Ok(local_addr)
    }

    fn restore_forwards(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for f in st.forwards.iter().filter(|f| f.host_key == host_key && f.state != ForwardState::Active) {
            let reconnect = f.handle.reconnect(ctrl.clone(), ep.clone());
            let (id, sid, hk, ctrl, inner) = (f.id.clone(), f.handle.session_id.clone(), host_key.to_owned(), ctrl.clone(), self.clone());
            tokio::spawn(async move {
                let result = reconnect.await;
                let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                let Some(host) = st.hosts.get(&hk).filter(|h| h.control.as_ref().is_some_and(|c| c.same_connection(&ctrl))) else { return };
                let session_ended = !host.session_summaries.iter().any(|s| s.session_id == sid);
                let Some(f) = st.forwards.iter_mut().find(|f| f.id == id) else { return };
                f.state = match result {
                    Ok(()) => ForwardState::Active,
                    Err(olo_rex_client::RexError::Api(_)) if session_ended => ForwardState::Failed(olo_kit::words::tr("its session has ended.").into()),
                    Err(olo_rex_client::RexError::Api(e)) => ForwardState::Failed(e.message),
                    Err(e) => ForwardState::Failed(e.to_string()),
                };
                drop(st);
                (inner.notify)(HubNotice::StateChanged);
            });
        }
    }

    async fn run_host_connection(self: Arc<Self>, host_key: String, retry: Arc<tokio::sync::Notify>) {
        let mut attempt = 0u32;
        let mut last_seq = 0;
        loop {
            let (endpoint_raw, reconnect_token, is_local, user_overrides) = {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let overrides = st.settings.keymap_overrides.clone();
                let Some(h) = st.hosts.get_mut(&host_key) else { return };
                if !h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry)) {
                    return;
                }
                if attempt == 0 {
                    h.conn_state = HostConnState::Connecting;
                }
                (h.endpoint_raw.clone(), h.reconnect_token.clone(), h.is_local, overrides)
            };
            (self.notify)(HubNotice::StateChanged);

            let endpoint = match RexEndpoint::parse(&endpoint_raw, self.ssh_pool.clone(), self.askpass.clone()) {
                Ok(ep) => ep,
                Err(e) => {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(h) = st.hosts.get_mut(&host_key) {
                        h.conn_state = HostConnState::Error(e.to_string());
                        h.retry = None;
                    }
                    drop(st);
                    (self.notify)(HubNotice::StateChanged);
                    return;
                }
            };

            let connected = async {
                let (stream, host_hdr) = endpoint.connect_control_stream().await?;
                let label = std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
                let (ctrl, events) = ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(label), reconnect_token.as_deref()).await?;
                let client_id = ctrl.client_id().map(str::to_owned);
                let token = ctrl.reconnect_token().map(str::to_owned);

                // Fetch server status, version, synced hosts, keymap, and bootstrap sessions!
                let status = ctrl.server_status().await.ok();
                let version = ctrl.server_version().await.ok().map(|r| r.payload);
                let hosts_resp = ctrl.hosts_list().await.ok().map(|r| r.payload);
                // (The commands Rex has, and no keys of its own but what its
                // old recorder left: what comes back is the keys of the
                // person's Rex configuration, and those.)
                let km_actions = keymap::register_actions_payload();
                let km_resp = ctrl.client_keymap(&km_actions, &[], &user_overrides).await.ok().map(|r| r.payload);

                let ticket_req =
                    [DataTicketRequest { creator_name: "com.superlogical.terminal".to_owned(), data_ref_id: "terminal".to_owned(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.to_owned()) }];
                let boot = SessionSnapshot::read(&ctrl, &ticket_req).await?;
                Ok::<_, olo_rex_client::RexError>((ctrl, events, client_id, token, status, version, hosts_resp, km_resp, boot))
            }
            .await;
            match connected {
                Ok((ctrl, mut events, client_id, token, status, version, hosts_resp, km_resp, boot)) => {
                    let SessionSnapshot { seq: boot_seq, rows: summaries, views: initial_views, tickets: pre_tickets } = boot;

                    // Spawn one strictly-FIFO input writer task for this host connection so rapid
                    // keystrokes can never race and arrive out of order on `terminal.write`.
                    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<HostInputOp>();
                    let ctrl_in = ctrl.clone();
                    let ep_in = endpoint.clone();
                    let host_in = host_key.clone();
                    let inner_in = self.clone();
                    let input_connection = ctrl.clone();
                    let input_worker = tokio::spawn(async move {
                        // The queue belongs to this physical connection. A disconnected pane accepts
                        // no keys. Discard its unwritten backlog, including any uncertain in-flight
                        // request; input is never replayed by the app, even to a resumed client ID.
                        let write = async {
                            // Each is written and answered before the next (their order
                            // is the order typed, whatever the server does with two at
                            // once), so what was typed while one was on its way goes
                            // together in the next (`typed_together`): a request a key
                            // was a round trip a key, and 65 characters typed in 2.8 s
                            // took 18.7 s to reach a host 224 ms away.
                            // (Sent as typed, without waiting, each letter's echo is
                            // back in one round trip, not one and a half; but the
                            // server does take two at once, and forty letters came
                            // back "acbdefgjhilknm…": measured 2026-11-23,
                            // `ControlClient::send_raw`. What a slow link leaves is
                            // shown at once another way: HACKING, Rex, "A slow link".)
                            //
                            // Where Olo's helper is on the host
                            // (`keeps_typing_in_order`: it gives the server one at
                            // a time, there, where an answer takes no time), each
                            // is sent as it is typed and its answer waited for
                            // apart: a round trip a key whatever is on its way.
                            let in_order = ctrl_in.keeps_typing_in_order();
                            let mut waiting = std::collections::VecDeque::new();
                            let mut unanswered: std::collections::VecDeque<Unanswered> = std::collections::VecDeque::new();
                            loop {
                                if waiting.is_empty() {
                                    tokio::select! {
                                        op = in_rx.recv() => {
                                            let Some(op) = op else { break };
                                            waiting.push_back(op);
                                        }
                                        Some(done) = oldest_answer(&mut unanswered) => {
                                            inner_in.input_answered(&host_in, &done.session_id, &done.block_id, done.seq);
                                            continue;
                                        }
                                    }
                                }
                                while let Ok(op) = in_rx.try_recv() {
                                    waiting.push_back(op);
                                }
                                let Some(op) = typed_together(&mut waiting) else { continue };
                                // (What isn't typed keeps its place behind what was;
                                // and no more than so many are on their way at once.)
                                let room = if matches!(op, HostInputOp::Focus { .. }) { 0 } else { MOST_UNANSWERED - 1 };
                                while unanswered.len() > room {
                                    let Some(done) = oldest_answer(&mut unanswered).await else { break };
                                    inner_in.input_answered(&host_in, &done.session_id, &done.block_id, done.seq);
                                }
                                match op {
                                    HostInputOp::Focus { session_id, switch, target } => {
                                        if switch || target.block.is_none() {
                                            let _ = ctrl_in.session_focus_window(&session_id, &target.window).await;
                                        }
                                        if let Some(l) = &target.restore {
                                            let _ = ctrl_in.session_set_layer_state(&session_id, l, "normal", true).await;
                                        }
                                        if let Some(b) = &target.block {
                                            let _ = ctrl_in.session_focus_block(&session_id, b).await;
                                        }
                                        let view = ctrl_in.session_view(&session_id).await;
                                        {
                                            let mut st = inner_in.state.lock().unwrap_or_else(|e| e.into_inner());
                                            let Some(host) = st.hosts.get_mut(&host_in) else { continue };
                                            if !host.pending_focus.get(&session_id).is_some_and(|newest| Arc::ptr_eq(newest, &target)) {
                                                continue;
                                            }
                                            host.pending_focus.remove(&session_id);
                                        }
                                        if let Ok(view) = view {
                                            inner_in.ingest_session(&host_in, &ctrl_in, &ep_in, view.payload, &[]).await;
                                        }
                                    }
                                    HostInputOp::Write { session_id, block_id, text, seq } => {
                                        inner_in.input_went(&host_in, &session_id, &block_id, seq);
                                        match in_order.then(|| ctrl_in.send_terminal_write(&session_id, &block_id, &text)) {
                                            Some(Ok(answer)) => unanswered.push_back(Unanswered::new(session_id, block_id, seq, answer)),
                                            Some(Err(_)) => inner_in.input_answered(&host_in, &session_id, &block_id, seq),
                                            None => {
                                                let _ = ctrl_in.terminal_write(&session_id, &block_id, &text).await;
                                                inner_in.input_answered(&host_in, &session_id, &block_id, seq);
                                            }
                                        }
                                    }
                                    HostInputOp::Key { session_id, block_id, event, seq } => {
                                        inner_in.input_went(&host_in, &session_id, &block_id, seq);
                                        match in_order.then(|| ctrl_in.send_client_key(&session_id, Some(&block_id), &event)) {
                                            Some(Ok(answer)) => unanswered.push_back(Unanswered::new(session_id, block_id, seq, answer)),
                                            Some(Err(_)) => inner_in.input_answered(&host_in, &session_id, &block_id, seq),
                                            None => {
                                                let _ = ctrl_in.client_key(&session_id, Some(&block_id), &event, None).await;
                                                inner_in.input_answered(&host_in, &session_id, &block_id, seq);
                                            }
                                        }
                                    }
                                }
                            }
                        };
                        tokio::select! { biased; _=input_connection.disconnected()=>{}, _=write=>{} }
                    });

                    {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry))) {
                            h.conn_state = HostConnState::Connected;
                            h.connection_lost = None;
                            h.endpoint = Some(endpoint.clone());
                            h.client_id = client_id;
                            h.reconnect_token = token;
                            h.tailscale_seq = status.as_ref().map_or(0, |s| s.seq);
                            h.tailscale = status.as_ref().and_then(|s| s.payload.tailscale.clone());
                            h.server_status = status.map(|r| r.payload);
                            h.server_version = version;
                            // (Olo's helper: this connection goes through
                            // it, or it is seen to below.)
                            h.helper = if ctrl.keeps_typing_in_order() { HostHelper::Used } else { HostHelper::None };
                            if let Some(hr) = hosts_resp {
                                h.synced_hosts = hr.hosts;
                            }
                            if let Some(km) = km_resp {
                                h.keymap_bindings = km.bindings;
                                h.keymap_modes = km.modes.unwrap_or_default();
                                h.keymap_diagnostics = km.diagnostics;
                            }
                            h.session_summaries = summaries;
                            h.control = Some(ctrl.clone());
                            h.input_tx = Some(in_tx);
                        } else {
                            ctrl.close();
                            input_worker.abort();
                            return;
                        }
                    }

                    self.look_after_helper(&host_key, &endpoint);

                    for view in initial_views {
                        self.ingest_session(&host_key, &ctrl, &endpoint, view, &pre_tickets).await;
                    }
                    self.write_palette_index();

                    self.restore_forwards(&host_key, &ctrl, &endpoint);

                    // Auto-start Tier 2 Olo Bridge server for remote hosts whose bridge policy is enabled.
                    if !is_local {
                        let (should_bridge, hlabel) = {
                            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            let pol = st.settings.host_policies.get(&host_key).cloned().unwrap_or_else(crate::settings::HostBridgePolicy::remote_default);
                            let lbl = st.hosts.get(&host_key).map(|h| h.label.clone()).unwrap_or_else(|| host_key.clone());
                            (pol.any_enabled() && st.hosts.get(&host_key).is_some_and(|h| h.bridge_server.is_none()), lbl)
                        };
                        if should_bridge {
                            let st_ref = self.state.clone();
                            let hk_pol = host_key.clone();
                            let pol_fn: Arc<dyn Fn() -> crate::settings::HostBridgePolicy + Send + Sync> = Arc::new(move || {
                                st_ref.lock().unwrap_or_else(|e| e.into_inner()).settings.host_policies.get(&hk_pol).cloned().unwrap_or_else(crate::settings::HostBridgePolicy::remote_default)
                            });
                            let inn_fwd = self.clone();
                            let hk_fwd = host_key.clone();
                            let fwd_fn: bridge::LocalhostForwardFn = Arc::new(move |rh, rp| {
                                let inn = inn_fwd.clone();
                                let hk = hk_fwd.clone();
                                Box::pin(async move {
                                    let sid = inn
                                        .state
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .hosts
                                        .get(&hk)
                                        .and_then(|h| h.session_summaries.first().map(|s| s.session_id.clone()))
                                        .ok_or_else(|| "no active remote session for forward".to_owned())?;
                                    inn.ensure_ephemeral_forward(&hk, &sid, &rh, rp).await
                                })
                            });
                            let inner = Arc::downgrade(&self);
                            let source_host = host_key.clone();
                            let notify = move |req: &bridge::BridgeRequest| {
                                if let Some(inner) = inner.upgrade() {
                                    inner.bridge_notice(&source_host, req.session_id.as_deref(), req.block_id.as_deref(), &req.payload);
                                }
                            };
                            if let Ok(handle) = bridge::start_bridge_server(hlabel, pol_fn, fwd_fn, notify).await {
                                let port = handle.local_addr.port();
                                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(h) = st.hosts.get_mut(&host_key) {
                                    h.bridge_remote_port = Some(port);
                                    h.bridge_server = Some(handle);
                                }
                            }
                        }
                    }

                    (self.notify)(HubNotice::StateChanged);

                    let resumed = ctrl.welcome.client.as_ref().is_some_and(|c| c.resumed);
                    let mut sequence = EventSequence { seen: if resumed && last_seq <= boot_seq { last_seq } else { 0 }, membership: boot_seq };
                    while let Some(env) = events.recv().await {
                        let Some(gap) = sequence.next(env.seq) else { continue };
                        if gap || matches!(env.event, ControlEvent::EventDropped { block_id: None, .. }) {
                            match self.refresh_host(&host_key, &ctrl, &endpoint).await {
                                Ok(watermark) => sequence.membership = watermark,
                                Err(e) => {
                                    tracing::warn!("Rex event refresh failed: {e}");
                                    ctrl.close();
                                    break;
                                }
                            }
                            if matches!(env.event, ControlEvent::EventDropped { .. }) {
                                continue;
                            }
                        }
                        if env.seq != 0
                            && env.seq <= sequence.membership
                            && matches!(env.event, ControlEvent::SessionCreated(_) | ControlEvent::SessionMoved { .. } | ControlEvent::SessionDestroyed { .. })
                        {
                            continue;
                        }
                        self.handle_control_event(&host_key, &ctrl, &endpoint, env.seq, env.event).await;
                    }
                    last_seq = sequence.seen;
                    input_worker.abort();
                    let dropped_host = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        let label = if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.control.as_ref().is_some_and(|c| c.same_connection(&ctrl))) {
                            h.tailscale = None;
                            h.server_status = None;
                            h.server_version = None;
                            h.sessions.clear();
                            h.pending_focus.clear();
                            h.session_summaries.clear();
                            h.control = None;
                            h.input_tx = None;
                            attempt = 1;
                            h.connection_lost = Some(SystemTime::now());
                            h.conn_state = HostConnState::Reconnecting { attempt: 1, last_error: "Connection closed".to_owned() };
                            h.label.clone()
                        } else {
                            return;
                        };
                        for f in st.forwards.iter_mut().filter(|f| f.host_key == host_key) {
                            f.state = ForwardState::Waiting;
                        }
                        st.retain_blocks(|k| k.host_key != host_key);
                        label
                    };
                    (self.notify)(HubNotice::Attention {
                        target: Target::Host(host_key.clone()),
                        title: format!("{dropped_host} disconnected"),
                        body: "The connection to the Rex server closed.".into(),
                        kind: olo_kit::notify::NoteKind::Important,
                    });
                    self.write_palette_index();
                    (self.notify)(HubNotice::StateChanged);
                }
                Err(e) => {
                    attempt += 1;
                    let still_exists = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry))) {
                            h.control = None;
                            h.input_tx = None;
                            h.conn_state = HostConnState::Reconnecting { attempt, last_error: e.to_string() };
                            true
                        } else {
                            false
                        }
                    };
                    (self.notify)(HubNotice::StateChanged);
                    if !still_exists {
                        return;
                    }
                }
            }

            let backoff = Duration::from_secs((1u64 << attempt.min(4)).min(15));
            tokio::select! { _ = tokio::time::sleep(backoff) => {}, _ = retry.notified() => {} }
        }
    }

    async fn ingest_session(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint, view: SessionView, pre_tickets: &[olo_rex_client::DataTicket]) {
        let session_id = view.session_id.clone();
        let blocks: Vec<BlockInfo> = view.windows.iter().flat_map(|w| w.all_blocks()).cloned().collect();

        let mut to_attach = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(host_key) else { return };
            if !h.control.as_ref().is_some_and(|c| c.same_connection(ctrl)) {
                return;
            }
            if !install_view(&mut h.session_summaries, &mut h.sessions, view) {
                return;
            }
            if let Some(target) = h.pending_focus.get(&session_id)
                && let Some(view) = h.sessions.get_mut(&session_id)
            {
                target.apply(view);
            }
            let theme = st.active_theme.clone();
            let live_ids: HashSet<String> = blocks.iter().map(|b| b.block_id.clone()).collect();
            st.retain_blocks(|bk| !(bk.host_key == host_key && bk.session_id == session_id && !live_ids.contains(&bk.block_id)));

            for b in &blocks {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.clone(), block_id: b.block_id.clone() };
                let is_terminal = b.creator_name == "com.superlogical.terminal" || b.flavor.starts_with("com.superlogical.terminal");
                if let Some(existing) = st.blocks.get(&bk) {
                    let mut g = existing.lock().unwrap_or_else(|e| e.into_inner());
                    if !b.label.is_empty() && g.title.is_empty() {
                        g.title = b.label.clone();
                    }
                    g.label.clone_from(&b.label);
                    if !g.data_connected && is_terminal {
                        let ticket = pre_tickets.iter().find(|t| t.session_id == session_id && t.block_id == b.block_id).map(|t| t.ticket.clone());
                        to_attach.push((bk, existing.clone(), ticket));
                    }
                } else {
                    let mut br = BlockRuntime::new(120, 32, &theme);
                    br.predict.set_on(st.settings.type_ahead);
                    br.creator_name = b.creator_name.clone();
                    br.title = b.label.clone();
                    br.label = b.label.clone();
                    let arc = Arc::new(Mutex::new(br));
                    st.blocks.insert(bk.clone(), arc.clone());
                    if is_terminal {
                        let ticket = pre_tickets.iter().find(|t| t.session_id == session_id && t.block_id == b.block_id).map(|t| t.ticket.clone());
                        to_attach.push((bk, arc, ticket));
                    }
                }
            }
        }

        for (bk, br, pre_ticket) in to_attach {
            self.spawn_block_data_stream(ctrl.clone(), ep.clone(), bk, br, pre_ticket);
        }
        self.write_palette_index();
        (self.notify)(HubNotice::StateChanged);
    }

    fn spawn_block_data_stream(self: &Arc<Self>, ctrl: ControlClient, ep: RexEndpoint, bk: BlockKey, br: Arc<Mutex<BlockRuntime>>, pre_ticket: Option<String>) {
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if g.data_connected {
                return;
            }
            g.data_connected = true;
            g.input_ready = false;
            g.pending_status = Some(Vec::new());
            g.data_cancel = Some(tx);
        }
        let inner = self.clone();
        tokio::spawn(async move {
            let ticket = match pre_ticket {
                Some(t) => t,
                None => match ctrl.data_connect(&bk.session_id, &bk.block_id, "terminal", DATA_TERMINAL_SUBPROTOCOL).await {
                    Ok(r) => r.payload.ticket,
                    Err(e) => {
                        tracing::debug!("data.connect {} failed: {e}", bk.block_id);
                        br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
                        return;
                    }
                },
            };

            let Ok((raw_stream, host_hdr)) = ep.connect_stream().await else {
                br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
                return;
            };
            let mut stream = match TerminalDataStream::connect_hello(raw_stream, &host_hdr, &ticket).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("terminal stream connect {} failed: {e}", bk.block_id);
                    br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
                    return;
                }
            };

            // Also fetch initial terminal metadata (ports, process, program_status) asynchronously.
            let c_meta = ctrl.clone();
            let bk_meta = bk.clone();
            let br_meta = br.clone();
            let notify_meta = inner.notify.clone();
            tokio::spawn(async move {
                let snapshot = c_meta.terminal_program_status(&bk_meta.session_id, &bk_meta.block_id).await.ok();
                br_meta.lock().unwrap_or_else(|e| e.into_inner()).status_snapshot(snapshot);
                if let Ok(r) = c_meta.terminal_ports(&bk_meta.session_id, &bk_meta.block_id).await {
                    br_meta.lock().unwrap_or_else(|e| e.into_inner()).set_ports(r.seq, r.payload);
                }
                if let Ok(r) = c_meta.terminal_process(&bk_meta.session_id, &bk_meta.block_id).await {
                    br_meta.lock().unwrap_or_else(|e| e.into_inner()).process_snapshot(r);
                }
                notify_meta(HubNotice::StateChanged);
            });

            let mut attachment = Some(crate::attach::Attachment::default());
            let mut apc = InlineApcDemuxer::new(stream.welcome.channel_nonce);
            loop {
                let frame = tokio::select! {
                    _ = &mut rx => { stream.close_clean().await; break; }
                    res=stream.next_frame()=>res
                };
                let Ok(TerminalStreamFrame::Bytes(chunk)) = frame else { break };
                if chunk.is_empty() {
                    continue;
                }
                let (terminal, snapshot_size, live) = if let Some(attach) = &mut attachment {
                    match attach.push(&chunk) {
                        Ok(None) => continue,
                        Ok(Some(screen)) => {
                            attachment = None;
                            (screen.terminal, screen.snapshot_size, screen.live)
                        }
                        Err(e) => {
                            tracing::warn!("Could not read the snapshot for {}: {e}", bk.block_id);
                            stream.close_clean().await;
                            break;
                        }
                    }
                } else {
                    (None, 0, std::borrow::Cow::Borrowed(chunk.as_slice()))
                };
                let terminal = terminal.map(|mut t| {
                    let theme = inner.state.lock().unwrap_or_else(|e| e.into_inner()).active_theme.clone();
                    let _ = t.apply_theme(&theme);
                    t
                });
                let items = apc.push(&live);
                let mut notices = Vec::new();
                let restored = terminal.is_some();
                let (applied, guess) = {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    let result = terminal.map_or(Ok(()), |t| g.restore_terminal(t));
                    let mut guess = false;
                    if result.is_ok() {
                        for item in items {
                            match item {
                                DemuxItem::Vt(b) => g.terminal.vt_write(&b),
                                DemuxItem::Apc(a) => notices.extend(g.inline(a, ctrl.client_id())),
                            }
                        }
                        g.input_ready = true;
                        g.received_bytes += (snapshot_size + live.len()) as u64;
                        guess = g.guess_later();
                    }
                    (result, guess)
                };
                if guess {
                    // What an agent is doing there, looked at while the
                    // chunks come (every so often, not with each) and
                    // once more when they stop; the window is told when
                    // it changes (the sidebar and the pane's head show
                    // it). And when they stop, whether the program now
                    // asks for something secret: the server has no
                    // event for a terminal's input mode (it was read as
                    // a pane was attached and never again, so a
                    // password asked for later was never seen), and a
                    // program that asks has printed its question by
                    // then. One request for each pause in a pane's
                    // output; none while it streams or rests.
                    let (br, notify, ctrl, bk) = (br.clone(), inner.notify.clone(), ctrl.clone(), bk.clone());
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(GUESS_AFTER).await;
                            let (changed, quiet) = br.lock().unwrap_or_else(|e| e.into_inner()).guess_now(GUESS_AFTER);
                            if changed {
                                notify(HubNotice::StateChanged);
                            }
                            if quiet {
                                break;
                            }
                        }
                        if let Ok(mode) = ctrl.terminal_input_mode(&bk.session_id, &bk.block_id).await {
                            let asks = asks_for_secret(mode.payload.echo, mode.payload.canonical);
                            if br.lock().unwrap_or_else(|e| e.into_inner()).set_password_prompt(asks) {
                                notify(HubNotice::StateChanged);
                            }
                        }
                    });
                }
                if let Err(e) = applied {
                    tracing::warn!("Could not restore the terminal for {}: {e}", bk.block_id);
                    stream.close_clean().await;
                    break;
                }
                if restored {
                    let generation = {
                        let b = br.lock().unwrap_or_else(|e| e.into_inner());
                        b.finding.then_some(b.find_generation)
                    };
                    if let Some(generation) = generation {
                        inner.search_on_worker(br.clone(), generation);
                    }
                }
                for a in notices {
                    inner.handle_inline_apc(&bk, a);
                }
                (inner.notify)(HubNotice::BlockOutput(bk.clone()));
            }
            br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
        });
    }

    fn handle_inline_apc(&self, bk: &BlockKey, apc: InlineApc) {
        let (title, body) = match apc {
            InlineApc::ChildExited { exit_code } => ("Process exited".to_owned(), format!("Exit code {exit_code}")),
            InlineApc::Unknown(val) => {
                let Some(title) = val.get("title").and_then(Value::as_str) else { return };
                (title.to_owned(), val.get("body").and_then(Value::as_str).unwrap_or("").to_owned())
            }
            _ => return,
        };
        self.pane_attention(bk, &title, body, olo_kit::notify::NoteKind::Information);
    }

    fn pane_attention(&self, bk: &BlockKey, title: &str, body: String, kind: olo_kit::notify::NoteKind) {
        let session = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(view) = st.hosts.get(&bk.host_key).and_then(|h| h.sessions.get(&bk.session_id)) else { return };
            view.label.clone()
        };
        (self.notify)(HubNotice::Attention { target: Target::Pane(bk.clone()), title: format!("{session} · {title}"), body, kind });
    }

    /// Legacy bridge callers may only identify their host. An explicit pane belongs
    /// to its originating host and session, even if another pane now has focus.
    fn bridge_notice(&self, host_key: &str, session_id: Option<&str>, block_id: Option<&str>, payload: &Value) {
        let session_id = session_id.or_else(|| payload.get("session_id").and_then(Value::as_str));
        let block_id = block_id.or_else(|| payload.get("block_id").and_then(Value::as_str));
        let notice = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get(host_key) else { return };
            let (target, label) = match (session_id, block_id) {
                (sid, Some(bid)) => {
                    let session = match sid {
                        Some(id) => host.sessions.get(id).filter(|s| s.find_block(bid).is_some()),
                        None => host.sessions.values().find(|s| s.find_block(bid).is_some()),
                    };
                    let Some(s) = session else { return };
                    (Target::Pane(BlockKey { host_key: host_key.into(), session_id: s.session_id.clone(), block_id: bid.into() }), &s.label)
                }
                (Some(sid), None) => {
                    let Some(s) = host.session_summaries.iter().find(|s| s.session_id == sid) else { return };
                    (Target::Session { host_key: host_key.into(), session_id: sid.into() }, &s.label)
                }
                (None, None) => (Target::Host(host_key.into()), &host.label),
            };
            let title = payload.get("title").and_then(Value::as_str).unwrap_or("Rex");
            let body = payload.get("body").and_then(Value::as_str).unwrap_or_default().to_owned();
            HubNotice::Attention { target, title: format!("{label} · {title}"), body, kind: olo_kit::notify::NoteKind::Important }
        };
        (self.notify)(notice);
    }

    /// Re-read the resources that can change between two numbered envelopes.
    /// A fresh bootstrap also reinstalls subscriptions. Its membership watermark
    /// prevents events already represented in this snapshot from undoing it.
    async fn refresh_host(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint) -> Result<u64, olo_rex_client::RexError> {
        let snapshot = SessionSnapshot::read(ctrl, &[]).await?;
        let server = ctrl.server_status().await?;
        let rows = snapshot.rows;
        let hosts = ctrl.hosts_list().await?;
        let overrides = self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.keymap_overrides.clone();
        let keymap = ctrl.client_keymap(&keymap::register_actions_payload(), &[], &overrides).await?.payload;
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(host_key).filter(|h| h.control.as_ref().is_some_and(|c| c.same_connection(ctrl))) else { return Ok(snapshot.seq) };
            h.synced_hosts = hosts.payload.hosts;
            h.keymap_bindings = keymap.bindings;
            h.keymap_modes = keymap.modes.unwrap_or_default();
            h.keymap_diagnostics = keymap.diagnostics;
            h.tailscale_seq = server.seq;
            h.tailscale = server.payload.tailscale.clone();
            h.server_status = Some(server.payload);
            let ids: HashSet<_> = rows.iter().map(|r| r.session_id.as_str()).collect();
            h.sessions.retain(|id, _| ids.contains(id.as_str()));
            h.session_summaries = rows.clone();
            st.retain_blocks(|b| b.host_key != host_key || ids.contains(b.session_id.as_str()));
        }
        for view in snapshot.views {
            let session_id = view.session_id.clone();
            let ids: Vec<_> = view
                .windows
                .iter()
                .flat_map(|w| w.all_blocks())
                .filter(|b| b.creator_name == "com.superlogical.terminal" || b.flavor.starts_with("com.superlogical.terminal"))
                .map(|b| b.block_id.clone())
                .collect();
            self.ingest_session(host_key, ctrl, ep, view, &[]).await;
            for bid in ids {
                let sid = &session_id;
                let key = BlockKey { host_key: host_key.into(), session_id: sid.clone(), block_id: bid.clone() };
                let block = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&key).cloned();
                let Some(block) = block else { continue };
                let (status, ports, process, mode) =
                    tokio::try_join!(ctrl.terminal_program_status(sid, &bid), ctrl.terminal_ports(sid, &bid), ctrl.terminal_process(sid, &bid), ctrl.terminal_input_mode(sid, &bid))?;
                let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                b.status_snapshot(Some(status));
                b.set_ports(ports.seq, ports.payload);
                b.process_snapshot(process);
                b.set_password_prompt(asks_for_secret(mode.payload.echo, mode.payload.canonical));
            }
        }
        self.write_palette_index();
        (self.notify)(HubNotice::StateChanged);
        Ok(snapshot.seq)
    }

    async fn handle_control_event(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint, seq: u64, ev: ControlEvent) {
        match ev {
            ControlEvent::SessionCreated(s) => {
                {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(h) = st.hosts.get_mut(host_key) else { return };
                    if !h.session_summaries.iter().any(|row| row.session_id == s.session_id) {
                        h.session_summaries.push(s.clone());
                    }
                }
                if let Ok(att) = ctrl.session_attach(&s.session_id).await {
                    self.ingest_session(host_key, ctrl, ep, att.payload.view, &[]).await;
                }
            }
            ControlEvent::SessionMoved { session_id, before_session_id } => {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let Some(host) = st.hosts.get_mut(host_key) else {
                    return;
                };
                move_session(&mut host.session_summaries, &session_id, before_session_id.as_deref());
                drop(st);
                (self.notify)(HubNotice::StateChanged);
            }
            ControlEvent::SessionDestroyed { session_id } => {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(h) = st.hosts.get_mut(host_key) {
                    h.sessions.remove(&session_id);
                    h.pending_focus.remove(&session_id);
                    h.session_summaries.retain(|s| s.session_id != session_id);
                }
                st.retain_blocks(|bk| !(bk.host_key == host_key && bk.session_id == session_id));
                drop(st);
                self.write_palette_index();
                (self.notify)(HubNotice::StateChanged);
            }
            ControlEvent::BlockOwnerChanged { session_id, block_id, owner } => {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id, block_id };
                if let Some(block) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk) {
                    block.lock().unwrap_or_else(|e| e.into_inner()).other_size_owner = owner.is_some() && owner.as_deref() != ctrl.client_id();
                }
                (self.notify)(HubNotice::StateChanged);
            }
            ControlEvent::SessionViewChanged { session_id, .. }
            | ControlEvent::ActiveWindowChanged { session_id, .. }
            | ControlEvent::WindowCreated { session_id, .. }
            | ControlEvent::WindowClosed { session_id, .. }
            | ControlEvent::WindowLabelChanged { session_id, .. }
            | ControlEvent::SessionLabelChanged { session_id, .. }
            | ControlEvent::BlockCreated { session_id, .. }
            | ControlEvent::BlockClosed { session_id, .. } => {
                if let Ok(v) = ctrl.session_view(&session_id).await {
                    self.ingest_session(host_key, ctrl, ep, v.payload, &[]).await;
                }
            }
            ControlEvent::EventDropped { session_id: Some(sid), block_id, block_event_name, .. } => {
                if let Ok(v) = ctrl.session_view(&sid).await {
                    self.ingest_session(host_key, ctrl, ep, v.payload, &[]).await;
                }
                if let Some(bid) = block_id {
                    let bk = BlockKey { host_key: host_key.into(), session_id: sid.clone(), block_id: bid.clone() };
                    let block = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned();
                    let Some(block) = block else { return };
                    let what = block_event_name.as_deref();
                    if matches!(what, None | Some("program_status_changed" | "program_status_removed")) {
                        let result = ctrl.terminal_program_status(&sid, &bid).await.ok();
                        block.lock().unwrap_or_else(|e| e.into_inner()).status_snapshot(result);
                    }
                    if matches!(what, None | Some("ports_changed"))
                        && let Ok(r) = ctrl.terminal_ports(&sid, &bid).await
                    {
                        block.lock().unwrap_or_else(|e| e.into_inner()).set_ports(r.seq, r.payload);
                    }
                    if matches!(what, None | Some("process_changed"))
                        && let Ok(r) = ctrl.terminal_process(&sid, &bid).await
                    {
                        block.lock().unwrap_or_else(|e| e.into_inner()).process_snapshot(r);
                    }
                    if matches!(what, None | Some("input_mode_changed"))
                        && let Ok(r) = ctrl.terminal_input_mode(&sid, &bid).await
                    {
                        block.lock().unwrap_or_else(|e| e.into_inner()).set_password_prompt(asks_for_secret(r.payload.echo, r.payload.canonical));
                    }
                    (self.notify)(HubNotice::StateChanged);
                }
            }
            ControlEvent::BlockEvent { session_id, block_id, name, payload, .. } => {
                if name == "clipboard_written" {
                    let permitted = self.clipboard_allowed(host_key);
                    if permitted && let Some(text) = payload.get("text").and_then(Value::as_str) {
                        (self.notify)(HubNotice::SetClipboard(text.to_owned()));
                    }
                    return;
                }
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.clone(), block_id: block_id.clone() };
                let (br_opt, rules) = {
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    (st.blocks.get(&bk).cloned(), st.settings.notices)
                };
                if let Some(br) = br_opt {
                    let mut activity_note = None;
                    let mut keep_going_nudge: Option<String> = None;
                    {
                        let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                        match name.as_str() {
                            "title_changed" => {
                                if let Some(t) = payload.get("title").and_then(Value::as_str) {
                                    g.title = t.to_owned();
                                }
                            }
                            "pwd_changed" => {
                                if let Some(p) = payload.get("pwd").and_then(Value::as_str) {
                                    g.pwd = p.to_owned();
                                }
                            }
                            "input_mode_changed" => {
                                if let Some(echo) = payload.get("echo").and_then(Value::as_bool) {
                                    // (No word of `canonical`: taken as on, as it is at a terminal's start.)
                                    g.set_password_prompt(asks_for_secret(echo, payload.get("canonical").and_then(Value::as_bool).unwrap_or(true)));
                                }
                            }
                            "program_status_changed" | "program_status_removed" => {
                                if let Some(change) = StatusUpdate::parse(&name, &payload) {
                                    let previous = AgentActivity::from_records(&g.program_statuses).map(|(_, r)| r.clone());
                                    g.status_event(seq, change);
                                    let now = g.recompute_activity();
                                    // Whether the person is told (the desktop's settings say),
                                    // and whether the pane is marked unread: one that waits or
                                    // failed is, one that finished isn't.
                                    let rule = match now {
                                        AgentActivity::Blocked => Some((rules.blocked, true, "Agent blocked", olo_kit::notify::NoteKind::Important)),
                                        AgentActivity::Error => Some((rules.failed, true, "Agent failed", olo_kit::notify::NoteKind::Error)),
                                        AgentActivity::Done => Some((rules.done, false, "Agent finished", olo_kit::notify::NoteKind::Information)),
                                        _ => None,
                                    };
                                    if !g.muted
                                        && let Some((tell, unread, title, kind)) = rule
                                        && let Some((_, record)) = AgentActivity::from_records(&g.program_statuses)
                                        && previous.as_ref() != Some(record)
                                    {
                                        if tell {
                                            let body = record.msg.clone().or_else(|| record.title.clone()).unwrap_or_else(|| g.title.clone());
                                            activity_note = Some((title, body, kind));
                                        }
                                        g.unread |= unread;
                                    }
                                    if let Some(kg) = &mut g.keep_going
                                        && let Some(nudge) = kg.observe(now)
                                    {
                                        keep_going_nudge = Some(format!("{nudge}\r"));
                                    }
                                }
                            }
                            "ports_changed" => {
                                if let Ok(ps) = serde_json::from_value::<PortsSnapshot>(payload.clone()) {
                                    g.set_ports(seq, ps);
                                }
                            }
                            "process_changed" => {
                                if seq < g.process_seq {
                                    return;
                                }
                                g.process_seq = seq;
                                if let Some(cwd) = merge_process(&mut g.process, &payload) {
                                    g.pwd = cwd;
                                }
                                g.recompute_activity();
                            }
                            "bell" => {
                                g.last_bell = Some(Instant::now());
                                if !g.muted {
                                    g.unread = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some((title, body, kind)) = activity_note {
                        self.pane_attention(&bk, title, body, kind);
                    }
                    if let Some(nudge) = keep_going_nudge {
                        self.input_by_itself(&bk);
                        let _ = ctrl.terminal_write(&session_id, &block_id, &nudge).await;
                    }
                    (self.notify)(HubNotice::StateChanged);
                }
            }
            ControlEvent::HostsChanged { .. } => {
                if let Ok(r) = ctrl.hosts_list().await {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(h) = st.hosts.get_mut(host_key) {
                        h.synced_hosts = r.payload.hosts;
                    }
                    drop(st);
                    (self.notify)(HubNotice::StateChanged);
                }
            }
            ControlEvent::KeymapChanged { .. } => {
                let user_overrides = self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.keymap_overrides.clone();
                let km_actions = keymap::register_actions_payload();
                if let Ok(r) = ctrl.client_keymap(&km_actions, &[], &user_overrides).await {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(h) = st.hosts.get_mut(host_key) {
                        h.keymap_bindings = r.payload.bindings;
                        h.keymap_modes = r.payload.modes.unwrap_or_default();
                        h.keymap_diagnostics = r.payload.diagnostics;
                    }
                    drop(st);
                    (self.notify)(HubNotice::StateChanged);
                }
            }
            ControlEvent::TailscaleStatusChanged(status) => {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(h) = st.hosts.get_mut(host_key) {
                    crate::tailscale::apply(&mut h.tailscale, &mut h.tailscale_seq, seq, status);
                }
                drop(st);
                (self.notify)(HubNotice::StateChanged);
            }
            ControlEvent::ClientPromptRequested(req) => {
                (self.notify)(HubNotice::PromptRequested { host_key: host_key.to_owned(), req });
            }
            ControlEvent::ClientPromptEnded(end) => {
                (self.notify)(HubNotice::PromptEnded { host_key: host_key.to_owned(), call_id: end.call_id });
            }
            ControlEvent::ClientPerformRequested(req) => {
                let ok = self.handle_client_perform(host_key, req.action.clone(), req.args.clone(), req.session_id.clone(), req.block_id.clone()).await;
                if ok {
                    let _ = ctrl.client_reply_ok(&req.call_id, json!({ "ok": true })).await;
                } else {
                    let _ = ctrl.client_reply_err(&req.call_id, "permission_denied", "No matching Rex workspace or permission for this action").await;
                }
            }
            ControlEvent::ClientCallRequested(req) => {
                let _ = ctrl.client_reply_err(&req.call_id, "unsupported", &format!("Unknown RPC {}.{}", req.server, req.method)).await;
            }
            ControlEvent::Unknown { kind, .. } => tracing::debug!(%kind,"Unrecognized Rex event"),
            _ => {}
        }
    }

    async fn handle_client_perform(self: &Arc<Self>, host_key: &str, action: String, args: Option<Value>, session_id: Option<String>, block_id: Option<String>) -> bool {
        if action.starts_with("olo.") {
            let (is_local, policy) = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let is_local = st.hosts.get(host_key).is_some_and(|h| h.is_local);
                let policy = st.settings.policy_for_host(host_key, is_local);
                (is_local, policy)
            };
            match action.as_str() {
                "olo.open" => {
                    if !policy.allow_open_url {
                        return false;
                    }
                    if let Some(target) = args.as_ref().and_then(|a| a.get("target")).and_then(Value::as_str) {
                        let fwd: Option<bridge::LocalhostForwardFn> = if !is_local && let Some(sid) = session_id {
                            let hk = host_key.to_owned();
                            let inn = self.clone();
                            Some(Arc::new(move |rh, rp| {
                                let hk = hk.clone();
                                let sid = sid.clone();
                                let inn = inn.clone();
                                Box::pin(async move { inn.ensure_ephemeral_forward(&hk, &sid, &rh, rp).await })
                            }))
                        } else {
                            None
                        };
                        let _ = bridge::open_url_with_autoforward(target, !is_local && policy.allow_localhost_autoforward, fwd.as_ref()).await;
                    }
                    return true;
                }
                "olo.notify" => {
                    self.bridge_notice(host_key, session_id.as_deref(), block_id.as_deref(), args.as_ref().unwrap_or(&Value::Null));
                    return true;
                }
                "olo.clipboard.set" => {
                    if !policy.allow_remote_control {
                        return false;
                    }
                    if let Some(text) = args.as_ref().and_then(|a| a.get("text")).and_then(Value::as_str) {
                        (self.notify)(HubNotice::SetClipboard(text.to_owned()));
                    }
                    return true;
                }
                "olo.screen.ensure" | "olo.screen.watch" | "olo.screen.open_url" => {
                    if !policy.allow_agent_screens {
                        return false;
                    }
                    if let Some(bin) = olo_xdg::find_program("olo-screen")
                        && let Some(name) = args.as_ref().and_then(|a| a.get("name")).and_then(Value::as_str)
                    {
                        let mut cmd = tokio::process::Command::new(bin);
                        match action.as_str() {
                            "olo.screen.ensure" => {
                                cmd.args(["ensure", name]);
                            }
                            "olo.screen.watch" => {
                                cmd.args(["watch", name]);
                            }
                            "olo.screen.open_url" => {
                                if let Some(u) = args.as_ref().and_then(|a| a.get("url")).and_then(Value::as_str) {
                                    cmd.args(["run", name, "olo-open", u]);
                                }
                            }
                            _ => {}
                        }
                        let _ = cmd.stdin(std::process::Stdio::null()).spawn();
                    }
                    return true;
                }
                _ => {}
            }
        }

        crate::ui::request_action(host_key, session_id, block_id, action, args)
    }

    /// What Olo's command palette finds of Rex (apps/palette, its Rex
    /// source): each host's sessions, and the panes of the sessions
    /// shown, by their tab, title and folder. Written (in one piece)
    /// when it differs from what was last written, and never otherwise.
    /// Rex's own Go to Anything listed these in a window of its own;
    /// Olo's palette is where a person looks for a thing.
    fn write_palette_index(&self) {
        if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") {
            return;
        }
        let entries: Vec<PaletteEntry> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut out = Vec::new();
            for hk in &st.host_order {
                let Some(h) = st.hosts.get(hk) else { continue };
                for s in &h.session_summaries {
                    let view = h.sessions.get(&s.session_id);
                    let pane = |block_id: &str| {
                        let bk = BlockKey { host_key: hk.clone(), session_id: s.session_id.clone(), block_id: block_id.to_owned() };
                        st.blocks.get(&bk).map(|b| {
                            let g = b.lock().unwrap_or_else(|e| e.into_inner());
                            (g.title.clone(), g.pwd.clone())
                        })
                    };
                    let cwd = view
                        .and_then(|v| v.active_window().or_else(|| v.windows.first()))
                        .and_then(|w| w.focused_block_id.as_deref().or_else(|| w.all_blocks().next().map(|b| b.block_id.as_str())))
                        .and_then(&pane)
                        .map(|(_, pwd)| pwd)
                        .unwrap_or_default();
                    let session = if s.label.is_empty() { s.session_id.clone() } else { s.label.clone() };
                    out.push(PaletteEntry {
                        host_key: hk.clone(),
                        host_label: h.label.clone(),
                        session_id: s.session_id.clone(),
                        label: session.clone(),
                        cwd,
                        block_id: None,
                        session: String::new(),
                        tab: String::new(),
                    });
                    let tabs = view.map(|v| session_tab_names(&st, hk, v)).unwrap_or_default();
                    for (i, w) in view.map(|v| v.windows.as_slice()).unwrap_or_default().iter().enumerate() {
                        let tab = tabs.get(i).cloned().unwrap_or_default();
                        for b in w.all_blocks() {
                            let Some((title, pwd)) = pane(&b.block_id) else { continue };
                            out.push(PaletteEntry {
                                host_key: hk.clone(),
                                host_label: h.label.clone(),
                                session_id: s.session_id.clone(),
                                label: if title.is_empty() { tab.clone() } else { title },
                                cwd: pwd,
                                block_id: Some(b.block_id.clone()),
                                session: session.clone(),
                                tab: tab.clone(),
                            });
                        }
                    }
                }
            }
            out
        };
        let Ok(bytes) = serde_json::to_vec(&entries) else { return };
        static WRITTEN: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        if *written == bytes {
            return;
        }
        let dir = olo_xdg::state_dir().join("rex");
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("palette-index.json");
        let tmp = dir.join(format!("palette-index.json.tmp.{}", std::process::id()));
        if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &target).is_ok() {
            *written = bytes;
        }
    }
}

/// One thing of Rex's that Olo's palette finds: a session, or (with
/// `block_id`) a pane of one.
#[derive(serde::Serialize)]
struct PaletteEntry {
    host_key: String,
    host_label: String,
    session_id: String,
    label: String,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    session: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    tab: String,
}

pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_owned();
    }
    if s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-_.:@+=,".contains(&b)) {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_rex_vt::try_decode_snapshot_prefix;

    /// Tabs are told apart: by the name given, the program's title, the
    /// folder; two that say the same, by their folders; two in the same
    /// folder, by their place.
    #[test]
    fn tabs_are_told_apart() {
        let names = |tabs: &[(Option<&str>, &str, &str)]| tab_names(tabs);
        assert_eq!(names(&[(Some("build"), "cargo", "/w/olo"), (None, "vim", "/w/olo"), (None, "", "/w/rex/"), (None, "", "")]), ["build", "vim", "rex", "Tab 4"]);
        assert_eq!(names(&[(None, "shell", "/w/olo"), (None, "shell", "/w/rex")]), ["shell · olo", "shell · rex"]);
        assert_eq!(names(&[(None, "shell", "/w/olo"), (None, "shell", "/w/olo"), (None, "shell", "")]), ["shell · olo", "shell · olo 2", "shell"]);
        assert_eq!(names(&[(None, "shell", "/w/olo"), (None, "shell", "/w/olo")]), ["shell", "shell 2"], "one folder tells nothing apart");
        assert_eq!(names(&[(None, "shell", ""), (None, "shell", ""), (None, "shell", "")]), ["shell", "shell 2", "shell 3"]);
        // A name the person gave is left alone, twice too.
        assert_eq!(names(&[(Some("a"), "x", ""), (Some("a"), "y", "")]), ["a", "a"]);
        // A folder that is the title already isn't said twice.
        assert_eq!(names(&[(None, "", "/w/olo"), (None, "", "/w/olo")]), ["olo", "olo 2"]);
    }

    /// A tab the person named has that name; one the server named by
    /// its id has none.
    #[test]
    fn a_tabs_name_is_one_it_was_given() {
        let tab = |label: &str| olo_rex_client::WindowView { window_id: "window:01a201ce-027a-7b3e-b0f9-261f65bb795d".into(), label: label.into(), ..Default::default() };
        assert_eq!(tab_label(&tab("build")), Some("build"));
        assert_eq!(tab_label(&tab("Window 65bb795d")), None);
        assert_eq!(tab_label(&tab("")), None);
        assert_eq!(tab_label(&tab("Window 12345678")), Some("Window 12345678"), "a name of that shape that isn't its id's end is a name");
        assert_eq!(tab_label(&tab("Window shopping")), Some("Window shopping"));
    }

    /// What was typed while a request was on its way goes together, in
    /// order; a key event, another pane's text and a change of focus
    /// keep their places between.
    #[test]
    fn a_silent_host_is_said_by_the_second_then_by_the_minute() {
        let said = |local, secs| silent_said(if local { "This computer" } else { "far" }, local, Duration::from_secs(secs));
        assert_eq!(said(false, 2), "“far” hasn’t answered for 2 seconds");
        assert_eq!(said(false, 59), "“far” hasn’t answered for 59 seconds");
        assert_eq!(said(false, 60), "“far” hasn’t answered for over a minute");
        assert_eq!(said(false, 119), "“far” hasn’t answered for over a minute");
        assert_eq!(said(false, 185), "“far” hasn’t answered for over 3 minutes");
        assert_eq!(said(true, 4), "This computer hasn’t answered for 4 seconds");
        // (On the host's own row, without its name.)
        assert_eq!(silent_tip(Duration::from_secs(4)), "Hasn’t answered for 4 seconds");
        assert_eq!(silent_tip(Duration::from_secs(75)), "Hasn’t answered for over a minute");
        assert_eq!(silent_tip(Duration::from_secs(185)), "Hasn’t answered for over 3 minutes");
    }

    #[test]
    fn what_waits_is_written_together_in_order() {
        use std::collections::VecDeque;
        let text = |block: &str, text: &str| HostInputOp::Write { session_id: "s".into(), block_id: block.into(), text: text.into(), seq: 0 };
        let key = || HostInputOp::Key {
            session_id: "s".into(),
            block_id: "a".into(),
            seq: 0,
            event: RexKeyEvent { key: "key_c".into(), mods: "ctrl".into(), consumed_mods: String::new(), text: String::new(), unshifted_codepoint: 99, composing: false, action: "press".into() },
        };
        let mut waiting: VecDeque<HostInputOp> = [text("a", "e"), text("a", "c"), text("a", "ho"), key(), text("a", " x"), text("b", "ls"), text("a", "\r")].into();
        let mut out = Vec::new();
        while let Some(op) = typed_together(&mut waiting) {
            out.push(match op {
                HostInputOp::Write { block_id, text, .. } => format!("{block_id}:{text}"),
                HostInputOp::Key { .. } => "key".to_owned(),
                HostInputOp::Focus { .. } => "focus".to_owned(),
            });
        }
        assert_eq!(out, ["a:echo", "key", "a: x", "b:ls", "a:\r"]);
        assert!(typed_together(&mut waiting).is_none());
    }

    #[test]
    fn reorders_and_recovered_metadata_follow_the_server() {
        let mut rows = ["a", "b", "c"].map(|id| SessionSummary { session_id: id.into(), label: id.into() }).to_vec();
        move_session(&mut rows, "c", Some("b"));
        assert_eq!(rows[1].session_id, "c");
        let mut block = BlockRuntime::new(40, 12, &TerminalTheme::default());
        block.set_ports(3, PortsSnapshot { status: "current".into(), ..Default::default() });
        block.set_ports(2, PortsSnapshot::default());
        assert_eq!(block.ports.status, "current");
        block.status_snapshot(Some(olo_rex_client::RpcResult { seq: 5, payload: vec![ProgramStatusRecord { state: "done".into(), ..Default::default() }] }));
        block.status_snapshot(Some(olo_rex_client::RpcResult { seq: 4, payload: vec![] }));
        assert_eq!(block.agent_activity, AgentActivity::Done);
    }
    #[test]
    fn transfer_references_use_the_wire_format() {
        for direction in ["upload", "download"] {
            let r = transfer_reference(direction);
            let uuid = r.strip_prefix(&format!("{direction}:")).unwrap();
            assert_eq!(uuid, uuid::Uuid::parse_str(uuid).unwrap().to_string());
        }
    }

    #[test]
    fn foreground_event_preserves_snapshot_fields() {
        let mut p = Some(serde_json::from_value(json!({"child":{"pid":1,"name":"shell"},"home":"/fixture","last_exit":{"exit_code":7}})).unwrap());
        assert_eq!(merge_process(&mut p, &json!({"generation":3,"foreground":{"pid":2,"name":"editor","cwd":"/fixture/work"}})).as_deref(), Some("/fixture/work"));
        let p = p.unwrap();
        assert_eq!(p.child.unwrap().pid, 1);
        assert_eq!(p.home.as_deref(), Some("/fixture"));
        assert_eq!(p.last_exit.unwrap().exit_code, 7);
        assert!(!crate::settings::HostBridgePolicy::remote_default().allow_remote_control);
    }

    #[test]
    fn only_a_complete_download_can_become_a_preview() {
        let mut bytes = Vec::new();
        assert!(!preview_complete(&mut bytes, 2, Ok(Some(vec![1, 2]))).unwrap());
        assert!(preview_complete(&mut bytes, 2, Err(olo_rex_client::RexError::Closed("interrupted".into()))).is_err());
        assert!(preview_complete(&mut bytes, 3, Ok(None)).is_err());
        assert!(preview_complete(&mut bytes, 2, Ok(None)).unwrap());
        assert_eq!(bytes, [1, 2]);
    }

    #[test]
    fn an_inline_resize_and_theme_apply_to_the_stream_in_order() {
        let mut b = BlockRuntime::new(60, 12, &TerminalTheme::default());
        assert!(b.inline(InlineApc::Resize { cols: 90, rows: 20, cell_width_px: 8, cell_height_px: 16, owner: Some("other".into()) }, Some("here")).is_none());
        assert_eq!(b.terminal.size(), (90, 20));
        assert!(b.other_size_owner);
        b.inline(InlineApc::Theme { foreground: None, background: Some("#abcdef".into()), cursor: None, palette: vec![], scheme: None }, None);
        assert_eq!(b.current_theme.background.to_hex(), "#abcdef");
    }
    #[test]
    fn changed_queries_cancel_pending_search_and_rejected_resizes_keep_geometry() {
        let mut block = BlockRuntime::new(80, 24, &TerminalTheme::default());
        block.terminal.vt_write(b"apple berry");
        block.set_find_query(Some("apple")).unwrap();
        let stale = block.find_generation;
        block.set_find_query(Some("berry")).unwrap();
        assert!(block.find_step(stale).unwrap());
        assert!(block.finding);
        while !block.find_step(block.find_generation).unwrap() {}
        assert_eq!(block.search.total_matches(), 1);
        assert!(block.resize(60, 22, 8, 16).unwrap());
        assert!(!block.resize(60, 22, 8, 16).unwrap());
        assert!(block.resize(60, 22, 12, 24).unwrap());
        assert!(block.resize(0, 22, 12, 24).is_err());
        assert_eq!(block.geometry, Some((60, 22, 12, 24)));
    }
    #[test]
    fn find_stays_bound_to_the_restored_terminal() {
        let mut block = BlockRuntime::new(60, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        block.terminal.vt_write(b"needle on the old screen");
        block.set_find_query(Some("needle")).unwrap();
        block.search.run().unwrap();
        assert_eq!(block.search.total_matches(), 1);
        let mut terminal = GhosttyTerminal::new(60, 12).unwrap();
        terminal.vt_write(b"needle needle on the replacement screen");
        let snapshot = terminal.encode_snapshot().unwrap();
        let restored = try_decode_snapshot_prefix(&snapshot, true, None).unwrap().unwrap();
        block.restore_terminal(restored.terminal).unwrap();
        while !block.find_step(block.find_generation).unwrap() {}
        assert_eq!(block.search.total_matches(), 2);
        block.set_find_query(Some("replacement")).unwrap();
        block.search.run().unwrap();
        assert_eq!(block.search.total_matches(), 1);
    }

    #[test]
    fn a_host_given_no_name_goes_by_the_host_in_its_address() {
        assert_eq!(host_name_from("ssh://you@build.example"), "build");
        assert_eq!(host_name_from("ssh://deploy@staging.example:2222/"), "staging");
        assert_eq!(host_name_from("nas.ts.net"), "nas");
        assert_eq!(host_name_from("localhost"), "localhost");
        assert_eq!(host_name_from("you@10.0.0.7:22"), "10.0.0.7");
        assert_eq!(host_name_from("ssh://[fd7a::1]:22"), "fd7a::1");
        // Nothing to take a host from: the address as typed.
        assert_eq!(host_name_from("  "), "");
        assert_eq!(host_name_from("ssh://"), "ssh://");
    }

    #[test]
    fn an_unnamed_tab_goes_by_its_folder_or_at_home_by_its_program() {
        // A title its program gave.
        assert_eq!(tab_title("vim README.md", "shell", Some("vim"), "/home/you/src", Some("/home/you")), "vim README.md");
        // None: nothing, where the folder's name will do (`tab_names`
        // takes that)…
        assert_eq!(tab_title("shell", "shell", Some("bash"), "/home/you/src/olo", Some("/home/you")), "");
        assert_eq!(tab_names(&[(None, "", "/home/you/src/olo")]), ["olo"]);
        // …and the program where the folder is home, or isn't known.
        assert_eq!(tab_title("shell", "shell", Some("bash"), "/home/you/", Some("/home/you")), "bash");
        assert_eq!(tab_title("", "", Some("zsh"), "", None), "zsh");
        // No program known either: the pane's own label.
        assert_eq!(tab_title("shell", "shell", None, "", None), "shell");
        // Three shells at home are told apart by their place.
        assert_eq!(tab_names(&[(None, "bash", "/home/you"), (None, "bash", "/home/you"), (None, "bash", "/home/you")]), ["bash", "bash 2", "bash 3"]);
    }

    #[test]
    fn a_pane_is_called_by_its_programs_title_or_its_program_and_folder() {
        // A title the program gave: that.
        assert_eq!(pane_title("vim README.md", "shell", Some("vim"), "~/src/olo"), "vim README.md");
        // None (the title is the pane's own label still, or empty): the
        // program running now, and where.
        assert_eq!(pane_title("shell", "shell", Some("bash"), "~/src/olo"), "bash — ~/src/olo");
        assert_eq!(pane_title("", "", Some("cargo"), "/srv/www"), "cargo — /srv/www");
        // No folder known: the program; no program: the label, or Shell.
        assert_eq!(pane_title("shell", "shell", Some("zsh"), ""), "zsh");
        assert_eq!(pane_title("shell", "shell", None, "~"), "shell — ~");
        assert_eq!(pane_title("", "", None, ""), "Shell");
        // Under home, from the tilde; another's home that begins as ours
        // does is not ours; a far host's folders as they are.
        assert_eq!(folder_said("/home/you", Some("/home/you")), "~");
        assert_eq!(folder_said("/home/you/src/olo", Some("/home/you/")), "~/src/olo");
        assert_eq!(folder_said("/home/young/src", Some("/home/you")), "/home/young/src");
        assert_eq!(folder_said("/srv/www", None), "/srv/www");
        assert_eq!(folder_said("", Some("/home/you")), "");
    }

    #[test]
    fn a_shells_prompt_is_not_a_question_for_a_password() {
        // sudo, ssh, read -s: nothing shown, the terminal collecting a line.
        assert!(asks_for_secret(false, true));
        // A shell's line editor (bash, zsh, fish) at its prompt: nothing
        // echoed by the terminal, but read key by key.
        assert!(!asks_for_secret(false, false));
        // A program reading a line the ordinary way, and a full-screen one.
        assert!(!asks_for_secret(true, true));
        assert!(!asks_for_secret(true, false));
    }

    #[test]
    fn a_moved_tab_is_put_before_the_one_that_follows_it_there() {
        let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        // The first to the third place: before the fourth.
        assert_eq!(moved_before(&ids, 0, 2), Some(Some("d".into())));
        // The last to the first place: before the first.
        assert_eq!(moved_before(&ids, 3, 0), Some(Some("a".into())));
        // One to the right: before what was two on; to the end: before none.
        assert_eq!(moved_before(&ids, 1, 2), Some(Some("d".into())));
        assert_eq!(moved_before(&ids, 1, 3), Some(None));
        // One to the left.
        assert_eq!(moved_before(&ids, 2, 1), Some(Some("b".into())));
        // Nowhere, or nothing there: nothing asked.
        assert_eq!(moved_before(&ids, 2, 2), None);
        assert_eq!(moved_before(&ids, 4, 0), None);
        assert_eq!(moved_before(&ids, 0, 4), None);
    }

    #[test]
    fn the_guess_at_an_agent_is_made_once_for_many_chunks() {
        let mut b = BlockRuntime::new(40, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        let (never, at_once) = (Duration::from_secs(3600), Duration::ZERO);
        // The first chunk has someone look; those that come while they
        // look don't ask again.
        assert!(b.guess_later());
        assert!(!b.guess_later());
        // Looked at while chunks still come (a shell's screen: nothing an
        // agent's, nothing changed): whoever looks goes on looking.
        assert_eq!(b.guess_now(never), (false, false));
        assert!(!b.guess_later());
        // Looked at with the pane quiet: done, and the next chunk asks anew.
        assert_eq!(b.guess_now(at_once), (false, true));
        assert!(b.guess_later());
        // A screen that asks to be answered, in its last rows (where the
        // guess looks): the guess changes, and says so, once.
        b.terminal.vt_write(b"\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\nClaude Code\r\nDo you want to proceed? [y/n]");
        assert_eq!(b.guess_now(at_once), (true, true));
        assert_eq!(b.agent_activity, AgentActivity::Blocked);
        assert_eq!(b.guess_now(at_once), (false, true));
    }

    #[test]
    fn status_reports_update_one_record_and_removals_reveal_the_next() {
        let mut b = BlockRuntime::new(40, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        let change = |v| StatusUpdate::parse("program_status_changed", &v).unwrap();
        b.status_event(3, change(json!({"id":"","record":{"state":"working"},"reason":"report"})));
        b.status_event(4, change(json!({"id":"job","record":{"state":"blocked"},"reason":"report"})));
        b.status_snapshot(Some(olo_rex_client::RpcResult { seq: 2, payload: vec![] }));
        assert_eq!(b.program_statuses.len(), 2);
        assert_eq!(b.recompute_activity(), AgentActivity::Blocked);
        b.status_event(5, change(json!({"id":"","record":{"state":"done"},"reason":"report"})));
        assert_eq!(b.recompute_activity(), AgentActivity::Blocked);
        b.status_event(6, StatusUpdate::parse("program_status_removed", &json!({"ids":["job","another-job"],"reason":"removed"})).unwrap());
        assert_eq!(b.recompute_activity(), AgentActivity::Done);
        assert_eq!(b.program_statuses.len(), 1);
    }

    #[test]
    fn a_late_view_cannot_restore_closed_panes_or_an_ended_session() {
        let mut rows = vec![SessionSummary { session_id: "s".into(), label: String::new() }];
        let mut sessions = HashMap::new();
        let empty = SessionView { session_id: "s".into(), label: "empty".into(), revision: 2, ..Default::default() };
        assert!(install_view(&mut rows, &mut sessions, empty.clone()));
        assert_eq!(rows[0].label, "empty");
        let old = SessionView { revision: 1, ..empty.clone() };
        assert!(!install_view(&mut rows, &mut sessions, old));
        assert!(sessions["s"].windows.is_empty());
        rows.clear();
        sessions.clear();
        assert!(!install_view(&mut rows, &mut sessions, empty));
        assert!(sessions.is_empty());
    }

    /// End-to-end live integration test: spawns an isolated scratch `rex server run`,
    /// connects via `olo-rex-client`, opens a `rex.data.terminal.v1` stream, decodes
    /// the real `GHOSTSNP` binary snapshot via `olo-rex-vt::try_decode_snapshot_prefix`,
    /// and verifies `RenderState` extracts rendered rows cleanly.
    #[test]
    fn live_ghostsnp_to_render_state_pipeline() {
        if !std::path::Path::new("/usr/bin/rex").exists() {
            return;
        }
        let dir = std::env::var_os("TMPDIR").map(PathBuf::from).filter(|p| p.starts_with("/var/tmp")).unwrap_or_else(|| PathBuf::from("/var/tmp")).join(format!("olo-rex-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let sock = dir.join("rex.sock");
        let child = std::process::Command::new("/usr/bin/rex")
            .args(["-S", &format!("unix://{}", sock.display()), "server", "run", "--listen", &format!("unix://{}", sock.display()), "--restore=false", "--state-dir"])
            .arg(&dir)
            .current_dir(&dir)
            .env("HOME", &dir)
            .env("REX_SERVER_SOCKET", &sock)
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .env("XDG_RUNTIME_DIR", &dir)
            .env("XDG_STATE_HOME", &dir)
            .env("XDG_CONFIG_HOME", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn scratch rex server");

        struct Fixture(std::process::Child, PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _fixture = Fixture(child, dir.clone());
        let deadline = Instant::now() + Duration::from_secs(6);
        while !sock.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(sock.exists(), "scratch rex socket did not appear");

        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let res: Result<(), String> = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                let ep = RexEndpoint::Unix(sock.clone());
                let (stream, host_hdr) = ep.connect_stream().await.map_err(|e| e.to_string())?;
                let (ctrl, _ev) = ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(Some("e2e-test".into())), None).await.map_err(|e| e.to_string())?;

                let created = ctrl.session_create(Some("e2e"), None).await.map_err(|e| e.to_string())?;
                let sid = created.payload.get("session_id").and_then(Value::as_str).ok_or("missing session_id")?;
                let att = ctrl.session_attach(sid).await.map_err(|e| e.to_string())?;
                let bid = att.payload.view.windows.first().and_then(|w| w.all_blocks().next()).map(|b| b.block_id.clone()).ok_or("no terminal block in session")?;

                let open = ctrl.data_connect(sid, &bid, "terminal", DATA_TERMINAL_SUBPROTOCOL).await.map_err(|e| e.to_string())?;
                let (ds, ds_hdr) = ep.connect_stream().await.map_err(|e| e.to_string())?;
                let mut tstream = TerminalDataStream::connect_hello(ds, &ds_hdr, &open.payload.ticket).await.map_err(|e| e.to_string())?;

                let mut attach = crate::attach::Attachment::default();
                let mut term = tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let frame = tstream.next_frame().await.map_err(|e| e.to_string())?;
                        let TerminalStreamFrame::Bytes(bytes) = frame else {
                            return Err("closed before snapshot".to_owned());
                        };
                        if let Some(screen) = attach.push(&bytes).map_err(|e| e.to_string())? {
                            return screen.terminal.ok_or_else(|| "no snapshot".to_owned());
                        }
                    }
                })
                .await
                .map_err(|e| e.to_string())??;
                let fixture = dir.join("preview.txt");
                std::fs::write(&fixture, b"Rex preview").map_err(|e| e.to_string())?;
                let begin = ctrl.terminal_download_begin(sid, &bid, &transfer_reference("download"), fixture.to_str().unwrap(), None).await.map_err(|e| e.to_string())?.payload;
                let (raw, header) = ep.connect_stream().await.map_err(|e| e.to_string())?;
                let mut transfer = BytesDataStream::connect_hello(raw, &header, &begin.ticket.unwrap()).await.map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                while !preview_complete(&mut bytes, begin.size, transfer.read_chunk().await).map_err(|e| e.to_string())? {}
                assert_eq!(&bytes, b"Rex preview");
                let begin = ctrl.terminal_upload_begin(sid, &bid, &transfer_reference("upload"), "sample.txt", 4, None).await.map_err(|e| e.to_string())?.payload;
                let (raw, header) = ep.connect_stream().await.map_err(|e| e.to_string())?;
                let mut upload = BytesDataStream::connect_hello(raw, &header, &begin.ticket).await.map_err(|e| e.to_string())?;
                upload.write_all(b"test").await.map_err(|e| e.to_string())?;
                upload.finish_write().await.map_err(|e| e.to_string())?;
                let mut rs = RenderState::new().map_err(|e| e.to_string())?;
                rs.begin_update(&mut term).map_err(|e| e.to_string())?;
                let _ = rs.end_update();
                let colors = rs.colors();
                let rows = rs.extract_rows(&colors.palette, false).map_err(|e| e.to_string())?;
                assert!(!rows.is_empty(), "RenderState extracted zero rows");
                let split = ctrl.session_new_split(sid, Some(&bid), "down", None).await.map_err(|e| e.to_string())?.payload;
                assert_eq!(split["anchor_id"], bid);
                let second = split["block_ids"][0].as_str().unwrap();
                ctrl.session_resize_pane(sid, split["split_ids"][0].as_str().unwrap(), 0.6).await.map_err(|e| e.to_string())?;
                ctrl.session_move_block(sid, second, &bid, "left").await.map_err(|e| e.to_string())?;
                for result in [ctrl.session_own_block(sid, &bid).await, ctrl.session_disown_block(sid, &bid).await] {
                    let result = result.map_err(|e| e.to_string())?.payload;
                    assert_eq!(result["blocks"].as_array().unwrap().len(), 1);
                    assert_eq!(result["blocks"][0]["block_id"], bid);
                }
                tstream.close_clean().await;
                Ok(())
            })
            .await
            .expect("scratch Rex test timed out")
        });
        res.expect("live GHOSTSNP -> RenderState pipeline failed");
    }
}

#[cfg(test)]
mod reconnect_tests;
