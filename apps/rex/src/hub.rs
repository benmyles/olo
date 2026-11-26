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
    BlockInfo, BytesDataStream, ClientPromptRequested, ControlClient, ControlEvent, DATA_TERMINAL_SUBPROTOCOL, DataTicketRequest, DemuxItem, HostRecord, InlineApc, InlineApcDemuxer,
    KeyEvent as RexKeyEvent, KeymapBinding, KeymapDiagnostic, KeymapMode, LocalForwardHandle, PortsSnapshot, ProgramStatusRecord, RexEndpoint, ServerStatus, ServerVersion, SessionSummary,
    SessionView, SshAskpass, SshSessionPool, TailscaleStatus, TerminalDataStream, TerminalProcessSnapshot, TerminalStreamFrame, start_local_forward,
};
use olo_rex_vt::{GhosttyTerminal, RenderState, TerminalMemoryUsage, TerminalSearch, TerminalTheme};
use olo_sync::Mutex;
use serde_json::{Value, json};

use crate::{
    agent::{AgentActivity, Do, Help, Watch, detect_agent_from_screen, which_agent},
    attention::Target,
    bridge,
    copy_mode::CopyMode,
    git::{GitCache, GitInfo},
    keymap,
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
    /// A host's file to copy here and open in its app (`olo open PATH`
    /// on a host, its `xdg-open`): Quick Look's way, in the window that
    /// shows the asking pane.
    OpenHostFile(BlockKey, String),
    /// A passing word of the hub's own, for something it did that
    /// nobody asked for in a window (Olo's helper put on a host): the
    /// workspace's status bar says it for a few seconds, where nothing
    /// else is being said.
    Said(String),
    /// A silent desktop notification (e.g. a host command auto-approved
    /// while Deskbar's Approve Without Asking is active).
    QuietNote {
        title: String,
        body: String,
    },
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

/// A terminal's stream is pouring: it has brought this much, with no
/// pause as long as [`POUR_PAUSE`], for [`POURED_FOR`] (a flood, or a
/// link too narrow for what the terminal writes). The pause is long:
/// what a narrow link brings comes a whole frame at a time (64 kB,
/// two seconds of a link of 30,000 bytes a second), and a quarter of
/// a second between frames made every frame the first of a new pour.
/// And it is still pouring if [`POURS_ON`] more has come
/// [`POUR_PAUSE`] after the host said its shell was at its prompt (a
/// flood that ended just now, on a link that kept up, brings nothing
/// more).
const POURED: u64 = 64 * 1024;
const POURED_FOR: Duration = Duration::from_millis(1500);
const POUR_PAUSE: Duration = Duration::from_secs(2);
const POURS_ON: u64 = 16 * 1024;

/// How long after a pouring terminal's host was asked whether the
/// flood is over it is asked again, to begin with; twice as long after
/// each time the terminal was caught up and went on pouring (a job
/// that writes in the background, with the shell at its prompt), up to
/// [`CATCH_UP_LOOK_MOST`].
const CATCH_UP_LOOK: Duration = Duration::from_secs(2);
const CATCH_UP_LOOK_MOST: Duration = Duration::from_secs(60);

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
    /// Epoch milliseconds when its last output chunk arrived (for stuck-child detection).
    pub last_output_at: u64,
    /// Epoch milliseconds when its subagent state last changed.
    pub last_state_change_at: u64,
    /// Last observed subagent state for O(1) dirty-block transition tracking.
    pub subagent_last_state: Option<crate::subagents::SubagentLiveState>,
    /// Monotonic generation counter cancelling superseded 250ms stability checks.
    subagent_check_gen: u64,
    /// What its agent is doing as Keep It Going and Auto Approve see it
    /// (the person's choice of them is `help`, by the pane's key), and
    /// the timer for the next thing either waits for, and the one that
    /// redraws a countdown each second while a question waits.
    pub watch: Watch,
    /// The mouse as its program asked for it (`mouse_report`): Ghostty's
    /// encoder, made the first time, and the button held down, reported.
    mouse: Option<olo_rex_vt::input::MouseEncoder>,
    pub mouse_held: Option<olo_rex_vt::input::MouseButton>,
    watch_timer: Option<tokio::task::AbortHandle>,
    watch_ticker: Option<tokio::task::AbortHandle>,
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
    /// The host's helper said, unasked, that the shell itself reads
    /// this terminal at its line editor: the number of the last typed
    /// request the server had taken when it looked
    /// ([`ControlClient::last_typed`]); 0, before any. `None` where it
    /// said otherwise, or nothing.
    host_reads: Option<u64>,
    /// For how many seconds the host is said (in the window's status
    /// bar) to have been silent about something typed.
    stall_said: Option<u64>,
    /// Since when the program has been in the middle of an update of
    /// its screen (its synchronized output, DEC mode 2026): the pane
    /// goes on showing the frame from before it.
    updating_since: Option<Instant>,
    pub received_bytes: u64,
    /// A file going to or from its host, said in the window's status bar
    /// (a strip on the pane took rows from its terminal, which a program
    /// there is told is a resize; Opus's review, 2026-11-24).
    pub upload_progress: Option<Transfer>,
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
            last_output_at: crate::subagents::now_unix_ms(),
            last_state_change_at: crate::subagents::now_unix_ms(),
            subagent_last_state: None,
            subagent_check_gen: 0,
            watch: Watch::new(Instant::now()),
            mouse: None,
            mouse_held: None,
            watch_timer: None,
            watch_ticker: None,
            unread: false,
            unread_cleared_at: None,
            muted: false,
            last_bell: None,
            data_connected: false,
            input_ready: false,
            predict: crate::predict::Predictor::new(false),
            predict_wake: None,
            host_reads: None,
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
    /// The host's helper has said its shell reads here
    /// ([`Self::host_reads`]), having looked after everything typed on
    /// the connection (`typed`, [`ControlClient::last_typed`], is what
    /// its word names): where the caret stands at a prompt known to
    /// echo on `screen`, the terminal as drawn now, what is typed next
    /// is drawn at once, as when the host is asked (`ask_the_host`),
    /// without the asking. Whether it is trusted so now.
    fn host_says_prompt(&mut self, typed: u64, screen: Option<&crate::predict::Screen>) -> bool {
        if self.host_reads != Some(typed) {
            return false;
        }
        match self.predict.wants_prompt(screen) {
            Some(asked) => self.predict.prompt_said(asked, true, screen),
            None => false,
        }
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
        let program = self.program_title();
        let home = process.and_then(|p| p.home.as_deref()).or(local_home);
        pane_title(&self.title, &self.label, program.as_deref(), &folder_said(&self.pwd, home))
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
        self.last_output_at = crate::subagents::now_unix_ms();
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
        // Claude Code, Codex and Antigravity as Terminal reads them (by
        // their program, Claude Code's own build named by its version, and
        // their screens' words: `olo_agent_screen`); another agent (aider,
        // goose, …) by the older, rougher reading. (By the older alone, a
        // Claude Code installed as its own build went unseen once its
        // banner had scrolled away.)
        // (Its bottom 16 rows, where an agent's status line and its
        // question's last lines are: read after every pause in its output,
        // and 30, as Auto Approve reads a whole question, cost 20 million
        // instructions in 10 s more than the 12 read before, with a
        // spinner turning four times a second.)
        let screen = self.screen_bottom(ACTIVITY_LOOK);
        let (name, exe, args) = self.agent_program();
        self.agent_activity = match which_agent(name.as_deref(), &exe, &args, &screen) {
            Some(agent) => match olo_agent_screen::classify(agent, &screen) {
                olo_agent_screen::State::Working => AgentActivity::Working,
                olo_agent_screen::State::Blocked => AgentActivity::Blocked,
                olo_agent_screen::State::Idle => AgentActivity::Idle,
            },
            None => {
                let tail: Vec<String> = screen.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().map(str::to_owned).collect();
                detect_agent_from_screen(name.as_deref(), &tail).unwrap_or(AgentActivity::Idle)
            }
        };
        self.agent_activity
    }

    /// The bottom of its screen, as text: where an agent's prompt, its
    /// questions and its status line are (as Terminal reads its tabs').
    pub fn agent_screen(&self) -> String {
        self.screen_bottom(AGENT_LOOK)
    }

    /// Its screen's bottom `n` rows, as text.
    fn screen_bottom(&self, n: usize) -> String {
        let (_, rows) = self.terminal.size();
        let start = rows.saturating_sub(n as u16);
        (start..rows).map(|r| self.terminal.viewport_row_text(r as u32)).collect::<Vec<_>>().join("\n")
    }

    /// Whether a coding agent runs in it: its foreground program is one,
    /// or a program has told its state (OSC 7501).
    pub fn runs_agent(&self) -> bool {
        let (name, exe, args) = self.agent_program();
        !self.program_statuses.is_empty() || name.is_some_and(|n| which_agent(Some(&n), &exe, &args, "").is_some())
    }

    /// A press, a release or a move of the mouse over cell `(col, row)`,
    /// as its program asked to be told of them (its terminal's modes: which
    /// events, and how written): the bytes to send it, or None where it
    /// hasn't asked for the mouse, or for this (a move with no button held
    /// under plain tracking). A wheel's step is a press of button four or
    /// five, never held.
    pub fn mouse_report(
        &mut self,
        action: olo_rex_vt::input::MouseAction,
        button: Option<olo_rex_vt::input::MouseButton>,
        mods: olo_rex_vt::input::KeyMods,
        (col, row): (u16, u16),
    ) -> Option<Vec<u8>> {
        use olo_rex_vt::input::{MouseAction, MouseButton, MouseEncoder, MouseGeometry};
        if !self.terminal.mouse_tracking() {
            return None;
        }
        if self.mouse.is_none() {
            self.mouse = MouseEncoder::new().ok();
        }
        let encoder = self.mouse.as_mut()?;
        encoder.sync_from_terminal(&self.terminal).ok()?;
        // (Cells of a size of their own: the cell under the pointer is the
        // drawing's to say, by its own measure; a program asking for
        // pixels, rare, gets them in proportion.)
        let (cols, rows) = self.terminal.size();
        let (w, h) = MOUSE_CELL;
        encoder
            .set_geometry(MouseGeometry {
                screen_width: u32::from(cols.max(1)) * w,
                screen_height: u32::from(rows.max(1)) * h,
                cell_width: w,
                cell_height: h,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            })
            .ok()?;
        let wheel = matches!(button, Some(MouseButton::Four | MouseButton::Five));
        if action == MouseAction::Press && !wheel {
            self.mouse_held = button;
        }
        encoder.set_any_button_pressed(self.mouse_held.is_some()).ok()?;
        let at = (f32::from(col) * w as f32 + w as f32 / 2.0, f32::from(row) * h as f32 + h as f32 / 2.0);
        let bytes = encoder.encode(action, button, mods, at.0, at.1).ok();
        if action == MouseAction::Release {
            self.mouse_held = None;
        }
        bytes.filter(|b| !b.is_empty())
    }

    /// Its program in the foreground: its name, its program file, and
    /// what it was run as (its first argument, the script an interpreter
    /// runs, the path it was run by).
    pub fn agent_program(&self) -> (Option<String>, String, Vec<String>) {
        let fg = self.process.as_ref().and_then(|p| p.foreground.as_ref());
        let args = fg.map_or(Vec::new(), |p| [p.argv0.clone(), p.script.clone(), p.invoked_path.clone()].into_iter().flatten().collect());
        (fg.map(|p| p.name.clone()), fg.and_then(|p| p.path.clone()).unwrap_or_default(), args)
    }

    /// What the pane's program goes by in a title (its head, its tab's,
    /// Activity's row): a coding agent by its own name, else the program's
    /// name (the foreground one, else the shell's child). Claude Code's
    /// program is named by its version ("2.1.331", in
    /// `~/.local/share/claude/versions`), which titled Activity's rows
    /// (Ben, 2026-11-24).
    pub fn program_title(&self) -> Option<String> {
        let (name, exe, args) = self.agent_program();
        if let Some(agent) = name.as_deref().and_then(|n| crate::agent::which_agent(Some(n), &exe, &args, "")) {
            return Some(agent.name().to_owned());
        }
        self.process.as_ref().and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.clone())
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
fn predict_wake(
    rt: &tokio::runtime::Handle,
    state: &Arc<Mutex<HubState>>,
    block: &Arc<Mutex<BlockRuntime>>,
    held: &mut BlockRuntime,
    key: &BlockKey,
    notify: &Arc<dyn Fn(HubNotice) + Send + Sync>,
    at: Instant,
) {
    if held.predict_wake.is_some_and(|waited| waited <= at) {
        return;
    }
    held.predict_wake = Some(at);
    let (state, block, key, notify) = (state.clone(), block.clone(), key.clone(), notify.clone());
    rt.spawn(async move {
        let mut at = at;
        loop {
            tokio::time::sleep_until(at.into()).await;
            let (next, changed, chrome, ask) = {
                let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                // (A sooner wait was begun since: this one is its.)
                if b.predict_wake != Some(at) {
                    return;
                }
                let (next, changed, chrome) = b.predict_tick(Instant::now());
                b.predict_wake = next;
                // (The clock gave up the last key that waited, at a
                // prompt known to echo: the pause in the output came
                // and went while it waited, and the host wasn't asked
                // about its prompt then. Asked now.)
                let ask = next.is_none() && b.predict.wants_prompt(b.caret_screen().as_ref()).is_some();
                (next, changed, chrome, ask)
            };
            if ask {
                let ctrl = state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&key.host_key).and_then(|h| h.control.clone());
                if let Some(ctrl) = ctrl {
                    ask_the_host(&block, &ctrl, &key, &notify).await;
                }
            }
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
/// What a pane goes by, said alone (an alert's question, a menu's item):
/// its title, else its program (`pane_title`, without its folder).
pub fn name_of_pane(st: &HubState, bk: &BlockKey) -> Option<String> {
    let br = st.blocks.get(bk)?;
    let g = br.lock().unwrap_or_else(|e| e.into_inner());
    let program = g.program_title();
    Some(pane_title(&g.title, &g.label, program.as_deref(), ""))
}

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
                    let program = g.program_title();
                    let home = process.and_then(|p| p.home.as_deref()).or(local.as_deref());
                    title = tab_title(&g.title, &g.label, program.as_deref(), &g.pwd, home);
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

/// The most a program in a terminal may copy at once (OSC 52), and how
/// many such copies there have been (a long one, read in pieces, isn't
/// put over one made since).
const CLIPBOARD_MOST: u64 = 8 << 20;
static CLIPBOARD_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long the sending of Olo's helper to a host is waited for before
/// the host's first connection is made without it.
const HELPER_FIRST: Duration = Duration::from_secs(8);

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
    /// Who made it: a host closes only what was made from it.
    pub made: crate::forwards::Made,
    /// Persisted across Rex restarts and host reconnects (`--keep` or Keep).
    pub kept: bool,
    handle: LocalForwardHandle,
}

/// Stable synthetic ID for a kept forward that doesn't currently have a live
/// `ActiveForward` (waiting for host/session, or failed to bind).
pub fn kept_forward_id(kf: &olo_protocol::RexKeptForward) -> String {
    format!("kept:{}:{}:{}:{}", kf.host, kf.remote_host, kf.remote_port, kf.local_port)
}

fn kept_forward_key(kf: &olo_protocol::RexKeptForward) -> (String, String, u16, u16) {
    (kf.host.clone(), kf.remote_host.clone(), kf.remote_port, kf.local_port)
}

fn kept_matches_active(kf: &olo_protocol::RexKeptForward, af: &ActiveForward) -> bool {
    kf.host == af.host_key && kf.remote_host == af.remote_host && kf.remote_port == af.remote_port && (kf.local_port == 0 || kf.local_port == af.local_addr.port())
}

fn maker_to_made(m: olo_protocol::RexForwardMaker) -> crate::forwards::Made {
    match m {
        olo_protocol::RexForwardMaker::Person => crate::forwards::Made::Person,
        olo_protocol::RexForwardMaker::Agent => crate::forwards::Made::Agent,
    }
}

fn made_to_maker(m: crate::forwards::Made) -> olo_protocol::RexForwardMaker {
    match m {
        crate::forwards::Made::Person => olo_protocol::RexForwardMaker::Person,
        crate::forwards::Made::Link | crate::forwards::Made::Agent => olo_protocol::RexForwardMaker::Agent,
    }
}

/// A host's folder kept in step here (`RexKept`), as it stands now.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub host_key: String,
    /// The folder there, whole.
    pub there: String,
    pub mirror: bool,
    pub direction: olo_protocol::RexSyncDirection,
    pub shallow: bool,
    pub exclude: Vec<String>,
    /// Conflict copies currently present in the folder (two-way sync).
    pub conflicts: Vec<String>,
    pub state: KeptState,
    /// Its last sync: when, how many files it had, how much went.
    pub last: Option<(std::time::SystemTime, u64, u64)>,
}

impl Kept {
    fn spec(&self) -> KeptSpec {
        KeptSpec { there: self.there.clone(), mirror: self.mirror, direction: self.direction, shallow: self.shallow, exclude: self.exclude.clone() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeptSpec {
    there: String,
    mirror: bool,
    direction: olo_protocol::RexSyncDirection,
    shallow: bool,
    exclude: Vec<String>,
}

impl KeptSpec {
    fn folder(&self) -> olo_rex_client::sync::HostFolder {
        olo_rex_client::sync::HostFolder { path: self.there.clone(), delete: self.mirror, shallow: self.shallow, exclude: self.exclude.clone(), ..Default::default() }
    }
}

struct SyncOutcome {
    files: u64,
    bytes: u64,
    changed: bool,
    new_conflicts: Vec<olo_rex_sync::twoway::Conflict>,
    all_conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeptState {
    /// Its host isn't connected (or its helper not there yet).
    Waiting,
    Syncing,
    InStep,
    /// Gone there: the folder was taken away or moved.
    Gone,
    Failed(String),
}

impl KeptState {
    /// As `olo sync ls` says it.
    pub fn said(&self) -> String {
        match self {
            KeptState::Waiting => "waiting for its host".into(),
            KeptState::Syncing => "syncing".into(),
            KeptState::InStep => "in step".into(),
            KeptState::Gone => "gone there".into(),
            KeptState::Failed(why) => format!("failed: {why}"),
        }
    }
}

/// Where what a host sends or keeps in step lands here: Downloads ▸ Rex ▸
/// the host's name (this computer's to say, never the host's).
pub fn host_here(label: &str) -> Option<std::path::PathBuf> {
    let name: String = label.chars().map(|c| if c == '/' || c == '\0' { '-' } else { c }).collect();
    let downloads = olo_xdg::user_dir("DOWNLOAD").or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Downloads")))?;
    Some(downloads.join("Rex").join(if name.is_empty() { "host".into() } else { name }))
}

/// Where a host's folder is kept here: [`host_here`] ▸ the folder's own
/// name.
pub fn kept_here(label: &str, there: &str) -> Option<std::path::PathBuf> {
    let leaf = there.trim_end_matches('/').rsplit('/').next().filter(|l| !l.is_empty() && *l != "." && *l != "..")?;
    Some(host_here(label)?.join(leaf))
}

/// Local stat cache and 3-way baseline paths for a kept folder.
fn kept_cache_paths(here: &std::path::Path, there: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let base = olo_rex_client::sessions::Copied::cache_for(&here.to_string_lossy(), there);
    (base.with_extension("kept.idx"), base.with_extension("baseline.idx"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyncAsk {
    List { json: bool },
    Conflicts { path: Option<String>, json: bool },
    Wait { path: String, json: bool },
    Stop { path: String },
    Keep { path: String, direction: olo_protocol::RexSyncDirection, mirror: bool, shallow: bool, exclude: Vec<String>, json: bool },
}

fn parse_sync_ask(args: &[String]) -> Result<SyncAsk, String> {
    use olo_protocol::RexSyncDirection as Dir;
    let Some(first) = args.first().map(String::as_str) else {
        return Err("say which folder".into());
    };
    match first {
        "ls" | "list" => {
            let mut json = false;
            for a in &args[1..] {
                match a.as_str() {
                    "--json" => json = true,
                    other => return Err(format!("{other} isn't an option for `ls`")),
                }
            }
            return Ok(SyncAsk::List { json });
        }
        "conflicts" => {
            let (mut path, mut json) = (None, false);
            for a in &args[1..] {
                match a.as_str() {
                    "--json" => json = true,
                    other if other.starts_with('-') => return Err(format!("{other} isn't an option for `conflicts`")),
                    p if path.is_none() => path = Some(p.to_owned()),
                    other => return Err(format!("unexpected argument `{other}`")),
                }
            }
            return Ok(SyncAsk::Conflicts { path, json });
        }
        "stop" | "rm" => {
            if args.len() != 2 || args[1].starts_with('-') {
                return Err("`stop` takes one folder path".into());
            }
            return Ok(SyncAsk::Stop { path: args[1].clone() });
        }
        "wait" => {
            let (mut path, mut json) = (None, false);
            for a in &args[1..] {
                match a.as_str() {
                    "--json" => json = true,
                    other if other.starts_with('-') => return Err(format!("{other} isn't an option for `wait`")),
                    p if path.is_none() => path = Some(p.to_owned()),
                    other => return Err(format!("unexpected argument `{other}`")),
                }
            }
            let Some(path) = path else { return Err("`wait` takes one folder path".into()) };
            return Ok(SyncAsk::Wait { path, json });
        }
        _ => {}
    }

    let mut path: Option<String> = None;
    let mut direction: Option<Dir> = None;
    let mut mirror = false;
    let mut shallow = false;
    let mut exclude: Vec<String> = Vec::new();
    let mut json = false;
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        let set_dir = |slot: &mut Option<Dir>, d: Dir| -> Result<(), String> {
            if slot.is_some_and(|prev| prev != d) {
                return Err("pick one direction (--from-host, --to-host, or --two-way)".into());
            }
            *slot = Some(d);
            Ok(())
        };
        match a {
            "--from-host" => set_dir(&mut direction, Dir::FromHost)?,
            "--to-host" => set_dir(&mut direction, Dir::ToHost)?,
            "--two-way" | "--both" => set_dir(&mut direction, Dir::Both)?,
            "--mirror" => mirror = true,
            "--shallow" => shallow = true,
            "--json" => json = true,
            "--exclude" | "-x" => {
                i += 1;
                let Some(pat) = args.get(i) else { return Err("--exclude takes a pattern".into()) };
                exclude.push(pat.clone());
            }
            other if other.starts_with("--exclude=") => {
                let pat = &other["--exclude=".len()..];
                if pat.is_empty() {
                    return Err("--exclude takes a pattern".into());
                }
                exclude.push(pat.to_owned());
            }
            other if other.starts_with('-') => return Err(format!("{other} isn't one of its options")),
            p => {
                if path.is_some() {
                    return Err(format!("unexpected argument `{p}` (one folder at a time)"));
                }
                path = Some(p.to_owned());
            }
        }
        i += 1;
    }
    let Some(path) = path else { return Err("say which folder".into()) };
    Ok(SyncAsk::Keep { path, direction: direction.unwrap_or(Dir::FromHost), mirror, shallow, exclude, json })
}

/// A forward as Rex keeps it over an upgrade in place: its block is left
/// open on its host's server as the old Rex ends, and taken up again
/// (`resume_local_forward`) at the same port here when the host connects
/// (`RexHubInner::resume_saved_forwards`): no orphaned block left there
/// (`rex forward ls` warned of one for every forward at every upgrade),
/// and no forward an agent's sign-in depends on gone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedForward {
    pub host_key: String,
    pub session_id: String,
    pub block_id: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
    #[serde(default)]
    pub made: crate::forwards::Made,
    #[serde(default)]
    pub kept: bool,
}

/// The forwards Rex had before an upgrade in place, each taken up again
/// when its host connects.
static SAVED_FORWARDS: std::sync::Mutex<Vec<SavedForward>> = std::sync::Mutex::new(Vec::new());

/// (As Rex comes back from an upgrade, before its hub starts.)
pub fn set_saved_forwards(forwards: Vec<SavedForward>) {
    *SAVED_FORWARDS.lock().unwrap_or_else(|e| e.into_inner()) = forwards;
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
        Self::from_bootstrap(ctrl, ctrl.session_bootstrap("all", None, None, tickets).await).await
    }

    /// From the answer to `session.bootstrap`, however it was asked
    /// (by itself, or with the connection's hello).
    async fn from_bootstrap(ctrl: &ControlClient, bootstrap: Result<olo_rex_client::RpcResult<olo_rex_client::BootstrapResponse>, olo_rex_client::RexError>) -> Result<Self, olo_rex_client::RexError> {
        let mut snapshot = match bootstrap {
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
        // (Asked together, not one after another: a round trip of the
        // link for all of them, where it was one for each session.)
        let asked: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|row| !snapshot.views.iter().any(|v| v.session_id == row.session_id))
            .map(|row| {
                let (ctrl, id) = (ctrl.clone(), row.session_id.clone());
                tokio::spawn(async move { ctrl.session_attach(&id).await })
            })
            .collect();
        for attached in asked {
            match attached.await.map_err(|e| olo_rex_client::RexError::Closed(format!("attaching a session: {e}")))? {
                Ok(r) => snapshot.views.push(r.payload.view),
                // Removed since the list. Its ordered destruction event follows.
                Err(olo_rex_client::RexError::Api(e)) if e.status == 404 => {}
                Err(e) => return Err(e),
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

/// One of the things of Olo's that is put on a far host from here (the
/// Host window; `RexHub::change_host`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostThing {
    /// Olo's typing helper: one file in the account's home.
    Helper,
    /// Olo's bridge: its shims.
    Bridge,
    /// The skill that tells agents there of the bridge and of Rex.
    Skill,
    /// The desktop commands there (`xdg-open`, `notify-send`, the
    /// clipboard's…): `bridge::COMMANDS`.
    Commands,
    /// A Token Pool of its own there, with this computer's accounts
    /// (put there and kept in step by this computer's Token Pool:
    /// `pool`).
    Pool,
}

/// What action is being done to a [`HostThing`] on a far host.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HostOp {
    Install,
    Update,
    Remove,
}

/// How a host stands, for the Host window: what it has of Olo's, as
/// last heard.
#[derive(Clone, Debug, PartialEq)]
pub struct HostStands {
    /// Its folders kept in step here (`olo sync`).
    pub kept: Vec<Kept>,
    pub name: String,
    /// Where it is, as a person writes it: `you@host` for one reached
    /// over SSH.
    pub address: String,
    pub conn: HostConnState,
    /// Reached over SSH: what is put there and taken off goes that way.
    pub ssh: bool,
    pub helper: HostHelper,
    /// Whether the helper installed on the host is this Olo's build
    /// (`crate::link::ID`): `Some(true)` up to date, `Some(false)` an
    /// older build is there or auto-update failed, `None` not installed
    /// or not yet checked.
    pub helper_current: Option<bool>,
    /// What failed when auto-updating the helper while an older build
    /// was already in use.
    pub helper_sync_err: Option<String>,
    /// The helper was taken off this host here, and isn't put back by
    /// itself (`Settings::helper_hosts`: "off").
    pub helper_kept_off: bool,
    /// Olo's typing helper on hosts (Preferences ▸ Rex ▸ General).
    pub helper_setting: bool,
    /// Whether the bridge's shims are there; none: not asked, or not
    /// answered.
    pub bridge: Option<bool>,
    /// What of the bridge's is granted to the host here (Preferences ▸
    /// Rex), each in a word or two; none: nothing.
    pub granted: Vec<&'static str>,
    /// Where the SSH keys are granted: what the host was last offered
    /// of them, by their items' names ([`HostRuntime::agent_keys`]).
    pub agent_keys: Option<Result<Vec<String>, String>>,
    pub skill: Option<bool>,
    /// Whether the installed bridge skill matches this Olo's stamp.
    pub skill_current: Option<bool>,
    /// Last auto-sync failure for the bridge skill, if any.
    pub skill_sync_err: Option<String>,
    /// The desktop commands there (`bridge::COMMANDS`): none until asked.
    pub commands: Option<bool>,
    /// Whether the installed desktop commands match this Olo's stamp.
    pub commands_current: Option<bool>,
    /// Last auto-sync failure for desktop commands, if any.
    pub commands_sync_err: Option<String>,
    /// How its Token Pool stands, as this computer's Token Pool says;
    /// none: that isn't running here (or wasn't asked yet).
    pub pool: Option<crate::pool::PoolStands>,
    /// Its Rex server's version, as it said when it connected.
    pub version: Option<String>,
    /// Newer Rex server package known for this host (from the Server
    /// window's package-repository check).
    pub server_newer: Option<String>,
    /// How the account there stands; none: not asked yet.
    pub account: Option<olo_rex_client::ready::Account>,
    /// The person's agent settings there: summary of files to copy,
    /// conflicts, new items, and remembered choices.
    pub carried: Option<olo_rex_client::carry::Summary>,
    /// The agents' sessions there, of those used in the window chosen
    /// (`olo_rex_client::sessions`); none: not asked yet.
    pub sessions: Option<olo_rex_client::sessions::Summary>,
    pub sessions_window: olo_rex_client::sessions::Window,
    /// The person's repositories, and how each stands there (none:
    /// not asked yet, or the list changed since).
    pub repos: Vec<olo_rex_client::repos::Repo>,
    pub repos_there: Option<Vec<olo_rex_client::repos::There>>,
}

/// One of the things of Olo's a far host may have, as the status bar
/// shows it: on or off, and its tip's sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Has {
    pub on: bool,
    pub said: String,
}

/// What a far host has of Olo's, for the status bar's marks (Ben,
/// 2026-11-23: "make the typing helper status as well as status of olo
/// bridge and skills little icons in the status bar that are either in
/// an on or off state not the weird popover": it was said in the
/// host's tip in the sidebar). The bridge and the skill are there once
/// the host has been asked ([`RexHubInner::look_at_what_the_host_has`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostHas {
    pub helper: Has,
    pub bridge: Option<Has>,
    pub skill: Option<Has>,
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
    pub server_newer: Option<String>,
    /// Olo's helper there, for this connection.
    pub helper: HostHelper,
    pub helper_current: Option<bool>,
    pub helper_sync_err: Option<String>,
    /// Whether Olo's bridge is on the host, and the skill for agents
    /// (their files in the account's home there): none until the host
    /// has been asked, and for a host that can't be (not over SSH).
    pub bridge_there: Option<bool>,
    pub skill_there: Option<bool>,
    pub skill_current: Option<bool>,
    pub skill_sync_err: Option<String>,
    pub commands_there: Option<bool>,
    pub commands_current: Option<bool>,
    pub commands_sync_err: Option<String>,
    /// What Passwords last answered when the host's programs asked the
    /// forwarded agent for the SSH keys: how many, or why none could be
    /// had; none: not asked on this connection. (Unsaid, a host was
    /// offered none and nothing showed it: Rex moved to an agent screen
    /// asked that screen's Passwords, 2026-11-23.)
    pub agent_keys: Option<Result<Vec<String>, String>>,
    /// How the account there stands (its agents, its path, git's
    /// identity: `olo_rex_client::ready`), as last asked; none until
    /// it has been (the Account window asks).
    pub account: Option<olo_rex_client::ready::Account>,
    /// What of the person's settings for their agents the host has
    /// (`olo_rex_client::carry`), asked with the account: summary of
    /// files to copy, conflicts, new items, and remembered choices.
    pub carried: Option<olo_rex_client::carry::Summary>,
    /// The agents' sessions there, asked with the account
    /// (`olo_rex_client::sessions`).
    pub agent_sessions: Option<olo_rex_client::sessions::Summary>,
    /// How each of the person's repositories stands there
    /// (`olo_rex_client::repos`), in the list's order as it was when
    /// the host was asked.
    pub repos_there: Option<Vec<olo_rex_client::repos::There>>,
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
    /// The server's word for the copy kept (asked again, the file is
    /// fetched only where it has changed since).
    version: Option<String>,
    /// Where the copy is, on this computer.
    local: PathBuf,
}

// A normal end-of-stream and the advertised size are both needed. In particular,
// even an error after the last byte isn't a completed transfer.
/// Where a file of a host's is kept for Quick Look: in Rex's folder of
/// them, a folder of its own for each host and each of its paths (by a
/// hash: a path's own folders could clash), keeping its name for the app
/// that opens it.
fn quick_look_file(host: &str, path: &str) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (host, path).hash(&mut h);
    let host_dir: String = host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    let name = path.rsplit('/').find(|s| !s.is_empty() && *s != "." && *s != "..").unwrap_or("file");
    olo_xdg::cache_dir().join("rex-quicklook").join(host_dir).join(format!("{:016x}", h.finish())).join(name)
}

/// (The tests' reading of a whole download into memory; Quick Look
/// streams it to a file.)
#[cfg(test)]
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
    /// Hosts' port forwards kept across Rex restarts and host reconnects
    /// (mirrored from `RexSettings::kept_forwards`).
    pub kept_forwards: Vec<olo_protocol::RexKeptForward>,
    /// Error from the last attempt to restore a kept forward (shown in Ports).
    pub kept_forward_errors: HashMap<(String, String, u16, u16), String>,
    /// Kept forwards currently being opened asynchronously (prevents duplicates).
    kept_forwards_starting: std::collections::HashSet<(String, String, u16, u16)>,
    /// Hosts' folders kept in step here (the desktop's `RexSettings::
    /// kept`), each as it stands (`RexHubInner::keep`).
    pub kept: Vec<Kept>,
    pub settings: Settings,
    pub active_theme: TerminalTheme,
    /// How each far host's Token Pool stands, by the host's key, as
    /// this computer's Token Pool last said (`pool::hosts`); none: it
    /// didn't answer, or wasn't asked yet.
    pub pools: Option<HashMap<String, crate::pool::PoolStands>>,
    /// The person's repositories (the desktop's settings: Preferences'
    /// Rex, kept by the Repositories window), in their order.
    pub repos: Vec<olo_rex_client::repos::Repo>,
    /// Remembered per-item conflict choices for carried agent settings,
    /// mirrored from `RexSettings::carry_choices`.
    pub carry_choices: Vec<olo_protocol::RexCarryRemembered>,
    /// How far back the agents' sessions copied to a host go (the
    /// desktop's settings).
    pub sessions_window: olo_rex_client::sessions::Window,
    pub git_cache: Arc<GitCache>,
    pub local_bin_dir: Option<PathBuf>,
    /// Native nested subagent tabs registry (parent_window_id -> child window_ids).
    pub subagents: crate::subagents::SubagentRegistry,
    quick_look_cache: HashMap<(String, String, String), QuickLookCacheEntry>,
}

impl HubState {
    /// How long a host has been silent about something typed into one
    /// of its terminals, where that is long enough to say: the longest
    /// of them. What the status bar says and the sidebar's light for
    /// the host shows are both this, so they can't disagree. (Typing
    /// goes to a host one write at a time: a terminal of it unanswered
    /// is the host unanswering.)
    /// What a far host reached over SSH has of Olo's, while it is
    /// connected: for the status bar's three marks. None for this
    /// computer and for a host reached another way (nothing of these
    /// is put there).
    pub fn host_has(&self, host_key: &str) -> Option<HostHas> {
        let host = self.hosts.get(host_key)?;
        if host.is_local || host.conn_state != HostConnState::Connected || !matches!(host.endpoint, Some(RexEndpoint::Ssh { .. })) {
            return None;
        }
        let name = &host.label;
        let helper = match &host.helper {
            HostHelper::Used if host.helper_current == Some(false) => {
                Has { on: true, said: olo_kit::tr!("Olo’s typing helper on “{}” has an update available. Open Manage Host to update it.", name).to_string() }
            }
            HostHelper::Used => Has { on: true, said: olo_kit::tr!("Olo’s typing helper is on “{}” and up to date.", name).to_string() },
            HostHelper::Installing => Has { on: false, said: olo_kit::tr!("Olo’s typing helper is being added to “{}”…", name).to_string() },
            HostHelper::Ready if host.helper_current == Some(false) => {
                Has { on: false, said: olo_kit::tr!("An older build of Olo’s typing helper is on “{}”. Open Manage Host to update it.", name).to_string() }
            }
            HostHelper::Ready => Has { on: false, said: olo_kit::tr!("Olo’s typing helper was added to “{}”. It will be used the next time you connect.", name).to_string() },
            HostHelper::Without(why) => Has { on: false, said: olo_kit::tr!("Olo’s typing helper isn’t on “{}”: {}", name, why).to_string() },
            HostHelper::None if !self.settings.host_helper => {
                Has { on: false, said: olo_kit::words::tr("Olo’s typing helper is turned off. To use it, turn it on in Preferences\u{a0}▸\u{a0}Rex.").to_string() }
            }
            HostHelper::None => Has { on: false, said: olo_kit::tr!("Olo’s typing helper isn’t on “{}”.", name).to_string() },
        };
        // (What is granted is what the settings say now, not whether the
        // bridge's server for the host has been started yet.)
        // (Nothing of the bridge is on the host: on is the permissions'.)
        let bridge = Some(if self.settings.policy_for_host(host_key, false).any_enabled() {
            Has { on: true, said: olo_kit::tr!("Programs on “{}” may use this computer through Rex, as its permissions say (Preferences\u{a0}▸\u{a0}Rex).", name).to_string() }
        } else {
            Has { on: false, said: olo_kit::tr!("Nothing on this computer is granted to “{}”. To grant it something, see Preferences\u{a0}▸\u{a0}Rex.", name).to_string() }
        });
        let skill = host.skill_there.map(|there| match (there, host.skill_current) {
            (true, Some(false)) => Has { on: true, said: olo_kit::tr!("Olo’s skill for agents on “{}” has an update available. Open Manage Host to update it.", name).to_string() },
            (true, _) => Has { on: true, said: olo_kit::tr!("Olo’s skill for agents is on “{}” and up to date.", name).to_string() },
            (false, _) => Has {
                on: false,
                said: olo_kit::tr!("Olo’s skill for agents isn’t on “{}”. To install it, choose Agents\u{a0}▸\u{a0}Install\u{a0}Olo’s\u{a0}Skill\u{a0}for\u{a0}Agents.", name).to_string(),
            },
        });
        Some(HostHas { helper, bridge, skill })
    }

    /// How the host `host_key` stands, for the Host window.
    pub fn host_stands(&self, host_key: &str) -> Option<HostStands> {
        let host = self.hosts.get(host_key)?;
        let address = match crate::setup::address(&host.endpoint_raw) {
            // (As Hosts and Ports writes it: its port too, where it
            // isn't SSH's own.)
            crate::setup::Address::Ssh(target) => {
                let port = target.port.filter(|port| *port != 22).map(|port| format!(":{port}")).unwrap_or_default();
                match &target.user {
                    Some(user) => format!("{user}@{}{port}", target.host),
                    None => format!("{}{port}", target.host),
                }
            }
            _ => host.endpoint_raw.clone(),
        };
        // (Rex's demo, where nothing is connected to: a host with an
        // SSH address stands as one reached so, and the one that is
        // "connected" has a little of everything, for the window's
        // pictures and its lint.)
        if crate::server::demo_mode() && host.endpoint_raw.starts_with("ssh://") {
            let there = host.conn_state == HostConnState::Connected;
            let demo_host_mode = std::env::var("OLO_REX_DEMO_HOST").ok();
            let behind = demo_host_mode.as_deref() == Some("behind");
            let current_demo = demo_host_mode.as_deref() == Some("current");
            let skill = if behind || current_demo {
                there.then_some(true)
            } else {
                match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
                    Some("asking") => None,
                    Some("ready") => there.then_some(true),
                    _ => there.then_some(false),
                }
            };
            return Some(HostStands {
                name: host.label.clone(),
                address,
                conn: host.conn_state.clone(),
                ssh: true,
                helper: if there { HostHelper::Used } else { HostHelper::None },
                helper_current: there.then_some(!behind),
                helper_sync_err: None,
                helper_kept_off: false,
                helper_setting: true,
                bridge: there.then_some(true),
                // (`OLO_REX_DEMO_KEYS=2`, `0`, or a reason: the SSH keys
                // granted too, and what the host was offered of them.)
                granted: match (there, std::env::var("OLO_REX_DEMO_KEYS").is_ok()) {
                    (false, _) => Vec::new(),
                    (true, false) => vec!["Links", "Passwords", "Agent Screens"],
                    (true, true) => vec!["Links", "Passwords", "SSH keys", "Agent Screens"],
                },
                agent_keys: std::env::var("OLO_REX_DEMO_KEYS")
                    .ok()
                    .filter(|_| there)
                    .map(|keys| keys.parse::<usize>().map(|n| ["GitHub", "Work laptop", "Deploy"].iter().cycle().take(n).map(|k| k.to_string()).collect()).map_err(|_| keys)),
                // (`OLO_REX_DEMO_ACCOUNT=ready`: the skill there too;
                // `asking`: not answered; `OLO_REX_DEMO_HOST=current|behind`: host sync QA.)
                skill,
                skill_current: skill.filter(|&s| s).map(|_| !behind),
                skill_sync_err: (there && behind).then(|| "chmod: cannot access ~/.claude/skills/olo-rex-bridge/olo: Read-only file system".to_owned()),
                commands: Some(there),
                commands_current: there.then_some(!behind),
                commands_sync_err: None,
                pool: Some(crate::pool::PoolStands {
                    wanted: there,
                    in_step: there && !behind,
                    accounts: there.then_some(3),
                    error: (there && behind).then(|| "The host couldn’t reach the Token Pool gateway.".to_owned()),
                    ..Default::default()
                }),
                version: there.then(|| "0.19.2".to_owned()),
                server_newer: (there && behind).then(|| "0.19.5".to_owned()),
                account: there.then(demo_account).flatten(),
                carried: match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
                    Some("asking") => None,
                    Some("ready") => there.then_some(olo_rex_client::carry::Summary { same_files: 14, ..Default::default() }),
                    Some("conflicts") => {
                        there.then_some(olo_rex_client::carry::Summary { to_copy_files: 4, new_items: 2, conflicts: 4, auto_updates: 1, same_files: 6, remembered_total: 3, ..Default::default() })
                    }
                    _ => there.then_some(olo_rex_client::carry::Summary { to_copy_files: 14, new_items: 14, ..Default::default() }),
                },
                // (`OLO_REX_DEMO_SESSIONS=none`: none used in the window;
                // `newer`: some continued there.)
                sessions: match (std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref(), std::env::var("OLO_REX_DEMO_SESSIONS").ok().as_deref()) {
                    (Some("asking"), _) => None,
                    (_, Some("none")) => there.then_some(olo_rex_client::sessions::Summary::default()),
                    (_, Some("newer")) => there.then_some(olo_rex_client::sessions::Summary { same: 21, newer_there: 2, ..Default::default() }),
                    // (The longest line: many, large, and some continued there.)
                    (_, Some("long")) => there.then_some(olo_rex_client::sessions::Summary { sessions: 138, bytes: 13_314_398_618, same: 4, newer_there: 2 }),
                    (Some("ready"), _) => there.then_some(olo_rex_client::sessions::Summary { same: 24, ..Default::default() }),
                    _ => there.then_some(olo_rex_client::sessions::Summary { sessions: 38, bytes: 1_288_490_189, same: 3, newer_there: 0 }),
                },
                sessions_window: self.sessions_window,
                repos: self.repos.clone(),
                repos_there: demo_repos_there(&self.repos).filter(|_| there),
                kept: demo_kept(host_key),
            });
        }
        Some(HostStands {
            name: host.label.clone(),
            address,
            conn: host.conn_state.clone(),
            ssh: matches!(host.endpoint, Some(RexEndpoint::Ssh { .. })),
            helper: host.helper.clone(),
            helper_current: host.helper_current,
            helper_sync_err: host.helper_sync_err.clone(),
            helper_kept_off: self.settings.helper_hosts.get(host_key).is_some_and(|word| word == "off"),
            helper_setting: self.settings.host_helper,
            bridge: host.bridge_there,
            granted: self.settings.policy_for_host(host_key, false).granted(),
            agent_keys: host.agent_keys.clone().filter(|_| self.settings.policy_for_host(host_key, false).allow_ssh_agent),
            skill: host.skill_there,
            skill_current: host.skill_current,
            skill_sync_err: host.skill_sync_err.clone(),
            commands: host.commands_there,
            commands_current: host.commands_current,
            commands_sync_err: host.commands_sync_err.clone(),
            pool: self.pools.as_ref().map(|pools| pools.get(host_key).cloned().unwrap_or_default()),
            version: host.server_version.as_ref().map(|v| v.version.clone()).filter(|v| !v.is_empty()),
            server_newer: host.server_newer.clone(),
            account: host.account.clone(),
            carried: host.carried,
            sessions: host.agent_sessions,
            sessions_window: self.sessions_window,
            repos: self.repos.clone(),
            // (As asked of the list as it is now, or not known.)
            repos_there: host.repos_there.clone().filter(|there| there.len() == self.repos.len()),
            kept: self.kept.iter().filter(|k| k.host_key == host_key).cloned().collect(),
        })
    }

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
    pub(crate) rt: tokio::runtime::Runtime,
    notify: Arc<dyn Fn(HubNotice) + Send + Sync>,
    inner: Arc<RexHubInner>,
}

struct RexHubInner {
    state: Arc<Mutex<HubState>>,
    ssh_pool: Arc<SshSessionPool>,
    askpass: Option<Arc<dyn SshAskpass>>,
    notify: Arc<dyn Fn(HubNotice) + Send + Sync>,
    /// Terminals' streams opened beforehand for a host that is being
    /// connected to, by the host's key ([`Spare`]).
    spares: Mutex<HashMap<String, Vec<Spare>>>,
    /// Where Passwords' SSH agent is on each host that has it, by the
    /// host's key: the path its terminals take as `SSH_AUTH_SOCK`
    /// ([`Self::give_the_agent`]).
    agent_socks: Mutex<HashMap<String, String>>,
    /// What hosts' bridges asked that isn't over (`bridge::Calls`).
    bridge_calls: bridge::Calls,
    /// Active `olo run` desktop command runs (`bridge_run::RunJobs`) and their approval queue.
    run_jobs: crate::bridge_run::RunJobs,
    run_approvals: Arc<crate::bridge_run::RunApprovals>,
    /// Each kept folder's runner and the lock its syncs take (one at a
    /// time into a folder: the runner's, and an agent's `olo sync wait`),
    /// by its host and its path there.
    keeping: Mutex<HashMap<(String, String), (Option<tokio::task::AbortHandle>, Arc<tokio::sync::Mutex<()>>)>>,
    /// Event-driven wakeups for `olo agent watch` whenever terminal output,
    /// program status, or session window state changes (zero polling).
    subagent_events: Arc<tokio::sync::Notify>,
    /// Monotonic cursor-addressed ring buffer of confirmed child subagent events.
    subagent_log: Mutex<crate::subagents::SubagentEventLog>,
}

/// A terminal's stream being opened as far as its upgrade, before the
/// ticket that says whose it is has come (`TerminalUpgrade`): begun
/// while the control connection says its own hello and asks for the
/// sessions, so that a terminal shows a round trip of the link after
/// those are in, not three (over SSH: a channel, the upgrade, the
/// hello). `None`: it couldn't be opened.
type Spare = tokio::task::JoinHandle<Option<olo_rex_client::TerminalUpgrade<olo_rex_client::RexStream>>>;

/// The most streams opened beforehand for a host: as many as it had
/// terminals when its connection was lost, one where none is known
/// (the pane that shows first), and no more than this.
const MOST_SPARES: usize = 8;

/// A terminal's stream opened as far as its upgrade, now.
async fn open_terminal_stream(ep: &RexEndpoint) -> Option<olo_rex_client::TerminalUpgrade<olo_rex_client::RexStream>> {
    let (raw, header) = ep.connect_stream().await.ok()?;
    TerminalDataStream::upgrade(raw, &header).await.ok()
}

/// This computer's home folder, as the status bar shortens folders
/// under it to `~` (the demo's own, in the demo).
pub fn local_home() -> Option<String> {
    if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") { Some("/home/you".into()) } else { std::env::var("HOME").ok() }
}

/// The demo's repositories on its connected host
/// (`OLO_REX_DEMO_ACCOUNT`: `asking`, not answered; `ready`, all of
/// them; else the first cloned, the second not, a third's folder
/// taken).
fn demo_repos_there(repos: &[olo_rex_client::repos::Repo]) -> Option<Vec<olo_rex_client::repos::There>> {
    use olo_rex_client::repos::There;
    match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
        Some("asking") => None,
        Some("ready") => Some(repos.iter().map(|_| There::Cloned { set_up: true }).collect()),
        // (All cloned but the last, whose folder holds something else.)
        Some("blocked") => Some(repos.iter().enumerate().map(|(i, _)| if i + 1 == repos.len() { There::Taken } else { There::Cloned { set_up: true } }).collect()),
        // (One cloned whose set-up hasn't run, at rest.)
        Some("notsetup") => Some(repos.iter().enumerate().map(|(i, _)| [There::Cloned { set_up: false }, There::Cloned { set_up: true }, There::Cloned { set_up: true }][i % 3].clone()).collect()),
        // (One cloned and not set up yet.)
        Some("cloning") => Some(repos.iter().enumerate().map(|(i, _)| [There::Cloned { set_up: true }, There::Absent, There::Cloned { set_up: false }][i % 3].clone()).collect()),
        _ => Some(repos.iter().enumerate().map(|(i, _)| [There::Cloned { set_up: true }, There::Absent, There::Taken][i % 3].clone()).collect()),
    }
}

/// The demo's list of repositories (`OLO_REX_DEMO_REPOS=1`).
pub fn demo_repos() -> Vec<olo_rex_client::repos::Repo> {
    use olo_rex_client::repos::Repo;
    if !crate::server::demo_mode() || std::env::var_os("OLO_REX_DEMO_REPOS").is_none() {
        return Vec::new();
    }
    vec![
        Repo { url: "git@github.com:example/olo.git".into(), folder: String::new(), setup: "cargo fetch".into() },
        Repo { url: "https://github.com/example/website".into(), folder: "sites/website".into(), setup: "npm ci".into() },
        Repo { url: "git@git.example.org:team/notes.git".into(), folder: String::new(), setup: String::new() },
    ]
}

/// Full item-level plan comparing this computer's carried agent settings
/// against what a host currently has, respecting `remembered` choices.
async fn carried_plan(
    pool: &olo_rex_client::SshSessionPool,
    target: &olo_rex_client::SshTarget,
    askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>,
    remembered: &HashMap<String, olo_protocol::RexCarryChoice>,
) -> Option<olo_rex_client::carry::Plan> {
    use olo_rex_client::carry;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let (files, short) = tokio::task::spawn_blocking(move || carry::gather(&home)).await.ok()?;
    let ran = pool.put(target, askpass, "sh", carry::look(&files).as_bytes()).await.ok()?;
    carry::Plan::read(&ran.output, &files, short, remembered)
}

/// What a host has of the person's settings for their agents, against
/// what this computer carries (`olo_rex_client::carry`), respecting
/// remembered per-item choices for that host.
async fn carried_stands(
    pool: &olo_rex_client::SshSessionPool,
    target: &olo_rex_client::SshTarget,
    askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>,
    remembered: &HashMap<String, olo_protocol::RexCarryChoice>,
) -> Option<olo_rex_client::carry::Stands> {
    let plan = carried_plan(pool, target, askpass, remembered).await?;
    let to_copy = plan.apply_auto();
    let summary = plan.summary();
    let same = plan.files.len().saturating_sub(to_copy.len());
    Some(olo_rex_client::carry::Stands { home: plan.home, to_copy, same, short: plan.short, summary })
}

/// How long a copy of `total` bytes has left, `sent` of them gone in
/// `elapsed`, at the pace so far, as a person reads it: none in its
/// first seconds (a pace from a moment says nothing), nor once done.
fn time_left(sent: u64, total: u64, elapsed: Duration) -> Option<String> {
    if elapsed < Duration::from_secs(5) || sent == 0 || sent >= total {
        return None;
    }
    let left = (total - sent) as f64 * elapsed.as_secs_f64() / sent as f64;
    Some(match left as u64 {
        0..60 => olo_kit::words::tr("less than a minute left").to_string(),
        60..5_400 => olo_kit::words::count(((left / 60.0).round() as u64).max(1), "about a minute left", "about {n} minutes left"),
        s => olo_kit::words::count((s as f64 / 3600.0).round() as u64, "about an hour left", "about {n} hours left"),
    })
}

/// Codex's server on the host asked for each of `ids` (sessions just
/// copied there), which adds those its database doesn't have: Codex
/// reads its sessions' folder only once, as it makes its database.
/// Its answers waited for a minute at most; nothing where Codex isn't
/// there.
async fn tell_codex(pool: &olo_rex_client::SshSessionPool, target: &olo_rex_client::SshTarget, askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>, ids: &[String]) {
    use olo_rex_client::sessions;
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    let Ok(io) = pool.exec_io(target, askpass, &format!("sh -c {}", shell_quote(sessions::REGISTER))).await else { return };
    let (from, mut to) = tokio::io::split(io);
    let mut lines = tokio::io::BufReader::new(from).lines();
    let asked = sessions::register(ids);
    let mut asked = asked.lines();
    let told = async {
        // (Its first, then the rest once it has answered: a server is
        // told nothing before it says it is ready.)
        let first = asked.next()?;
        to.write_all(format!("{first}\n").as_bytes()).await.ok()?;
        while let Some(line) = lines.next_line().await.ok()? {
            if serde_json::from_str::<serde_json::Value>(&line).is_ok_and(|v| v["id"] == 1) {
                break;
            }
        }
        let rest: String = asked.map(|l| format!("{l}\n")).collect();
        to.write_all(rest.as_bytes()).await.ok()?;
        let mut waiting: std::collections::HashSet<u64> = (10..10 + ids.len() as u64).collect();
        while !waiting.is_empty() {
            let line = lines.next_line().await.ok()??;
            if let Some(id) = serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|v| v["id"].as_u64()) {
                waiting.remove(&id);
            }
        }
        Some(())
    };
    if tokio::time::timeout(Duration::from_secs(60), told).await.ok().flatten().is_none() {
        tracing::info!("Rex: Codex on the host didn't answer for each session copied");
    }
    let _ = to.shutdown().await;
}

/// The agents' sessions copied by the engine (`olo_rex_client::sync`, the
/// host's helper receiving into the home, looking at those files alone and
/// leaving one newer there): each made as the archive made it
/// (`sessions::Copied`), what it was made into kept between copies, so a
/// file unchanged since is neither read nor sent and a transcript a turn
/// longer sends that turn. How far it is said as the archive said it while
/// the files are looked at, then what has gone. None where the helper
/// never answered (not there, or built without it): made as an archive
/// then.
#[allow(clippy::too_many_arguments)]
async fn copy_by_engine(
    pool: &olo_rex_client::SshSessionPool,
    target: &olo_rex_client::SshTarget,
    askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>,
    parts: Vec<olo_rex_client::sessions::Part>,
    here: &str,
    there: &str,
    total: u64,
    said: Arc<impl Fn(&str) + Send + Sync + 'static>,
) -> Option<Result<(), String>> {
    use olo_rex_client::{sessions, sync};
    use std::sync::atomic::{AtomicU64, Ordering};
    let looked = Arc::new(AtomicU64::new(0));
    let stop = sync::Stop::default();
    let counted = looked.clone();
    let source = sessions::Copied::new(parts, here, there, Some(sessions::Copied::cache_for(here, there)), move |n| {
        counted.fetch_add(n, Ordering::Relaxed);
    });
    // (The host's home by its whole path, as it said it: what a command
    // there is run in isn't always its home.)
    let folder = sync::HostFolder { path: there.to_owned(), keep_newer: true, scoped: true, ..Default::default() };
    let (seen, sent, tell) = (looked.clone(), stop.sent.clone(), said.clone());
    let telling = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let seen = seen.load(Ordering::Relaxed);
            tell(&if seen < total {
                olo_kit::tr!("{}%  ·  {}", (seen * 100 / total.max(1)).min(100), olo_kit::files::format_progress(seen, total)).to_string()
            } else {
                olo_kit::tr!("Sending what changed  ·  {}", olo_kit::files::format_size(sent.load(Ordering::Relaxed))).to_string()
            });
        }
    });
    let ran = sync::send_to(pool, target, askpass.clone(), crate::link::ID, Box::new(source), &folder, stop.clone()).await;
    telling.abort();
    match ran {
        Ok(report) => {
            if let Some(e) = report.errors.first() {
                return Some(Err(format!("{}: {}", e.path, e.reason)));
            }
            tracing::info!(sent = report.bytes_sent, appended = report.appended, new = report.new, same = report.same, "Rex: sessions copied by the engine");
            // (Codex's names of those copied, sent beside them, put
            // beside the host's own.)
            let merge = format!("sh -c {}", shell_quote(&format!("cd && {}", sessions::merge_codex())));
            match pool.exec(target, askpass, &merge, &mut |_| {}).await {
                Ok(ran) if ran.ok() => Some(Ok(())),
                Ok(ran) => Some(Err(ran.output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn’t say why").trim().to_owned())),
                Err(e) => Some(Err(e.to_string())),
            }
        }
        // (Nothing ever came back: the helper isn't there to do it.)
        Err(_) if stop.received.load(Ordering::Relaxed) == 0 => None,
        Err(e) => Some(Err(e.to_string())),
    }
}

/// How a host stands against the agents' sessions of this computer used
/// in `window` (`olo_rex_client::sessions`): this computer's gathered
/// off the async threads, the host asked its home, then each file's size
/// and time. None: it couldn't be asked.
async fn sessions_stands(
    pool: &olo_rex_client::SshSessionPool,
    target: &olo_rex_client::SshTarget,
    askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>,
    window: olo_rex_client::sessions::Window,
) -> Option<(olo_rex_client::sessions::Stands, olo_rex_client::sessions::Gathered)> {
    use olo_rex_client::sessions;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let gathered = tokio::task::spawn_blocking(move || sessions::gather(&home, window, std::time::SystemTime::now())).await.ok()?;
    let ran = pool.exec(target, askpass.clone(), &format!("sh -c {}", shell_quote(sessions::HOME)), &mut |_| {}).await.ok()?;
    let there = ran.output.lines().find_map(|l| l.strip_prefix("home ")).map(str::trim).filter(|h| h.starts_with('/'))?.to_owned();
    if gathered.parts.is_empty() {
        return Some((sessions::Stands { home: there, ..Default::default() }, gathered));
    }
    let paths: Vec<String> = gathered.parts.iter().map(|p| sessions::there_path(&p.path, &gathered.home, &there)).collect();
    let ran = pool.put(target, askpass, &format!("sh -c {}", shell_quote(sessions::LOOK)), sessions::look(&paths).as_bytes()).await.ok()?;
    let stands = sessions::Stands::read(&there, &ran.output, &gathered)?;
    Some((stands, gathered))
}

/// The demo's kept folders on its connected host, for the Host window's
/// pictures and its lint (`OLO_REX_DEMO_KEPT`: `none`; `states`, one in
/// each state, more than the Files tab shows at once; `trouble`, one
/// failed and one gone, for their buttons; else two in step).
fn demo_kept(host_key: &str) -> Vec<Kept> {
    use olo_protocol::RexSyncDirection as Dir;
    let at = |there: &str, dir: Dir, mirror: bool, shallow: bool, exclude: &[&str], conflicts: &[&str], state: KeptState, ago: u64, files: u64| Kept {
        host_key: host_key.to_owned(),
        there: there.to_owned(),
        mirror,
        direction: dir,
        shallow,
        exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
        conflicts: conflicts.iter().map(|s| (*s).to_owned()).collect(),
        state,
        last: Some((std::time::SystemTime::now() - Duration::from_secs(ago), files, 48_000)),
    };
    match std::env::var("OLO_REX_DEMO_KEPT").ok().as_deref() {
        Some("none") => Vec::new(),
        // (What needs the person: a conflict copy, one that failed, one gone there.)
        Some("trouble") => vec![
            at("/home/dev/old/build", Dir::FromHost, false, false, &[], &[], KeptState::Gone, 3600, 0),
            at("/home/dev/notes", Dir::Both, false, false, &[], &["plan.conflict-far-20261125-100000.md"], KeptState::InStep, 120, 18),
            at("/home/dev/src/site/public", Dir::FromHost, true, false, &[], &[], KeptState::Failed("dev-box is out of disk space".into()), 600, 40),
        ],
        Some("states") => vec![
            at("/home/dev/src/app/assets", Dir::ToHost, false, true, &[], &[], KeptState::Syncing, 5, 3),
            at("/home/dev/src/app/dist", Dir::FromHost, true, false, &["*.map"], &[], KeptState::InStep, 90, 12),
            at("/home/dev/notes", Dir::Both, false, false, &[], &["plan.conflict-far-20261125-100000.md", "todo.conflict-desktop-20261125-100112.md"], KeptState::InStep, 45, 18),
            at("/home/dev/src/site/public", Dir::FromHost, false, false, &[], &[], KeptState::Failed("dev-box is out of disk space".into()), 600, 40),
            at("/home/dev/old/build", Dir::FromHost, false, false, &[], &[], KeptState::Gone, 3600, 0),
        ],
        _ => vec![
            at("/home/dev/src/app/dist", Dir::FromHost, false, false, &[], &[], KeptState::InStep, 90, 12),
            at("/home/dev/notes", Dir::Both, false, false, &["*.tmp"], &[], KeptState::InStep, 300, 8),
        ],
    }
}

/// The demo's account on its connected host, for the Account window's
/// pictures and its lint (`OLO_REX_DEMO_ACCOUNT`: `new`, nothing there
/// yet; `ready`, all of it; `asking`, not answered yet; `other`,
/// another name for git there; `nogit`, a Fedora cloud image's, no git
/// and an administrator's account; else some of it).
fn demo_account() -> Option<olo_rex_client::ready::Account> {
    use olo_rex_client::ready::Account;
    let who = Some(("You".to_owned(), "you@example.org".to_owned()));
    let some = Account { path: true, claude: Some("2.1.331".into()), codex: None, antigravity: None, git: who, has_curl: true, has_git: true, admin: false };
    Some(match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
        Some("asking") => return None,
        Some("new") => Account { has_curl: true, has_git: true, ..Account::default() },
        Some("ready") => Account { codex: Some("0.172.0".into()), antigravity: Some("1.5.15".into()), ..some },
        Some("other") => Account { git: Some(("Somebody Else".to_owned(), "else@example.org".to_owned())), ..some },
        Some("nogit") => Account { git: None, has_git: false, admin: true, ..some },
        _ => some,
    })
}

/// The hosts the person let go of (Disconnect) in this run of Rex: kept
/// over an upgrade in place with the windows, so that Rex comes back
/// with them as they were, not connected again because they are to be
/// "as Rex starts".
static LET_GO: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn let_go() -> Vec<String> {
    LET_GO.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// (As Rex comes back from an upgrade, before its hub starts.)
pub fn set_let_go(hosts: Vec<String>) {
    *LET_GO.lock().unwrap_or_else(|e| e.into_inner()) = hosts;
}

/// A file going to or from a pane's host, as its window's status bar says
/// it: what ("Downloading shot.png from far…"), how much of how much, and
/// whether it can be stopped (a Quick Look's download can).
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub said: String,
    pub done: u64,
    pub total: u64,
    pub stoppable: bool,
}

/// The size, in its own units, of a cell as the mouse encoder is told it.
const MOUSE_CELL: (u32, u32) = (10, 20);

/// Where a turn of the wheel over a pane goes, as in a local terminal
/// (Terminal's rules: `apps/terminal/src/view.rs`, `Mouse::wheel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    /// Its history scrolls.
    Scrollback,
    /// Its program asked for the mouse: told of each step (Shift keeps it).
    Report,
    /// The alternate screen (a pager, an editor), the program asked for
    /// arrows for the wheel (mode 1007): an arrow key a line.
    Arrows,
    /// The alternate screen, no history to scroll and nothing asked for.
    Nothing,
}

pub fn wheel_goes(tracking: bool, shift: bool, alternate: bool, alternate_scroll: bool) -> Wheel {
    let reporting = tracking && !shift;
    if !reporting && !alternate {
        Wheel::Scrollback
    } else if reporting {
        Wheel::Report
    } else if alternate_scroll {
        Wheel::Arrows
    } else {
        Wheel::Nothing
    }
}

/// `n` presses of the up or down arrow, as the program reads them (its
/// application cursor keys, mode 1, or not).
pub fn arrows(up: bool, n: u32, app_cursor: bool) -> Vec<u8> {
    let key = if up { "A" } else { "B" };
    let one = if app_cursor { format!("\x1bO{key}") } else { format!("\x1b[{key}") };
    one.repeat(n as usize).into_bytes()
}

/// How many of a pane's bottom rows Keep It Going and Auto Approve read.
const AGENT_LOOK: usize = 30;
/// How many of them what its agent is doing is read from.
const ACTIVITY_LOOK: usize = 16;

/// What the person turned on for each pane (Keep It Going, Auto Approve),
/// by its key: the same for a pane reattached, and kept over an upgrade
/// in place, as the hosts let go of are.
static HELP: std::sync::LazyLock<std::sync::Mutex<HashMap<BlockKey, Help>>> = std::sync::LazyLock::new(Default::default);

/// What the panes have on, as kept over an upgrade.
pub fn helps() -> Vec<(String, String, String, Help)> {
    HELP.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, h)| (k.host_key.clone(), k.session_id.clone(), k.block_id.clone(), h.clone())).collect()
}

/// (As Rex comes back from an upgrade, before its hub starts.)
pub fn set_helps(kept: Vec<(String, String, String, Help)>) {
    *HELP.lock().unwrap_or_else(|e| e.into_inner()) = kept.into_iter().map(|(host_key, session_id, block_id, h)| (BlockKey { host_key, session_id, block_id }, h)).collect();
}

/// What a pane has on.
pub fn help_of(key: &BlockKey) -> Help {
    HELP.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned().unwrap_or_default()
}

/// How long a question waits before Auto Approve answers it:
/// `AUTO_APPROVE_AFTER`, or in a test `OLO_REX_AUTO_APPROVE_SECS`.
fn auto_approve_after() -> Duration {
    std::env::var("OLO_REX_AUTO_APPROVE_SECS").ok().and_then(|v| v.parse().ok()).map_or(crate::agent::AUTO_APPROVE_AFTER, Duration::from_secs)
}

/// The quiet Keep It Going waits for: `KEEP_GOING_QUIET`, or in a test
/// `OLO_REX_KEEP_GOING_SECS`.
fn keep_going_quiet() -> Duration {
    std::env::var("OLO_REX_KEEP_GOING_SECS").ok().and_then(|v| v.parse().ok()).map_or(crate::agent::KEEP_GOING_QUIET, Duration::from_secs)
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether a host is one Rex was connecting to and isn't connected to:
/// what a network that came back is a reason to try again.
fn wanted_back(conn: &HostConnState) -> bool {
    matches!(conn, HostConnState::Reconnecting { .. } | HostConnState::Error(_))
}

impl RexHub {
    pub fn start(
        settings: Settings,
        active_theme: TerminalTheme,
        askpass: Option<Arc<dyn SshAskpass>>,
        run_approvals: Option<Arc<crate::bridge_run::RunApprovals>>,
        notify: impl Fn(HubNotice) + Send + Sync + 'static,
    ) -> Arc<Self> {
        // (Every key and its echo pass through these, at the shell's tier as
        // Terminal's echo path is: they waited behind whatever else ran.)
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("rex-io")
            .on_thread_start(|| olo_system::priority::raise_self_patiently(olo_protocol::performance::SHELL_NICE))
            .enable_all()
            .build()
            .expect("create Rex tokio runtime");

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
                server_newer: None,
                helper: HostHelper::None,
                helper_current: None,
                helper_sync_err: None,
                bridge_there: None,
                skill_there: None,
                skill_current: None,
                skill_sync_err: None,
                commands_there: None,
                commands_current: None,
                commands_sync_err: None,
                agent_keys: None,
                account: None,
                carried: None,
                agent_sessions: None,
                repos_there: None,
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
                        server_newer: None,
                        helper: HostHelper::None,
                        helper_current: None,
                        helper_sync_err: None,
                        bridge_there: None,
                        skill_there: None,
                        skill_current: None,
                        skill_sync_err: None,
                        commands_there: None,
                        commands_current: None,
                        commands_sync_err: None,
                        agent_keys: None,
                        account: None,
                        carried: None,
                        agent_sessions: None,
                        repos_there: None,
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
                    },
                );
                host_order.push(sh.id.clone());
            }
        }

        let is_demo = std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1");
        let state = Arc::new(Mutex::new(HubState {
            hosts,
            host_order,
            blocks: HashMap::new(),
            forwards: Vec::new(),
            kept_forwards: Vec::new(),
            kept_forward_errors: HashMap::new(),
            kept_forwards_starting: std::collections::HashSet::new(),
            settings,
            active_theme,
            pools: None,
            repos: Vec::new(),
            carry_choices: Vec::new(),
            kept: Vec::new(),
            sessions_window: olo_rex_client::sessions::Window::default(),
            git_cache: GitCache::new(),
            local_bin_dir,
            subagents: if is_demo { crate::subagents::SubagentRegistry::default() } else { crate::subagents::SubagentRegistry::load() },
            quick_look_cache: HashMap::new(),
        }));

        let inner = Arc::new(RexHubInner {
            state: state.clone(),
            ssh_pool,
            askpass,
            notify: notify.clone(),
            spares: Mutex::new(HashMap::new()),
            bridge_calls: Default::default(),
            run_jobs: crate::bridge_run::RunJobs::new(),
            run_approvals: run_approvals.unwrap_or_else(|| crate::bridge_run::RunApprovals::new(|| {}, || {})),
            agent_socks: Mutex::new(HashMap::new()),
            keeping: Mutex::new(HashMap::new()),
            subagent_events: Arc::new(tokio::sync::Notify::new()),
            subagent_log: Mutex::new(crate::subagents::SubagentEventLog::default()),
        });

        let hub = Arc::new(Self { state, rt, notify, inner });

        // Isolated demo workspace (`OLO_REX_DEMO=1`) for `tools/dev/ui-lint` and offline visual QA:
        // populates realistic multi-pane splits, agent activity, git status, and a remote host
        // without touching any real Rex server socket or user files.
        if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") {
            hub.populate_demo_workspace();
            if std::env::var_os("OLO_REX_FILES_SOCKET").is_some() {
                crate::files_sock::spawn(hub.rt.handle(), hub.state.clone());
            }
            return hub;
        }

        // Expose connected hosts over Rex's local Unix socket so Olo Tracker
        // can browse, read and write `rex://HOST/PATH` through Rex's SSH pool.
        crate::files_sock::spawn(hub.rt.handle(), hub.state.clone());

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
                    // (Those that were being connected to: one the
                    // person let go of, or never connected, isn't
                    // connected because the network came back.)
                    st.hosts.values().filter(|hr| !hr.is_local && wanted_back(&hr.conn_state)).map(|hr| hr.key.clone()).collect()
                };
                for hk in to_reconnect {
                    h.connect_host(&hk);
                }
            }
        });

        hub.connect_host("local");
        let auto_hosts: Vec<String> = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let let_go = let_go();
            st.settings.saved_hosts.iter().filter(|h| h.auto_connect && !let_go.contains(&h.id)).map(|h| h.id.clone()).collect()
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
        let mk_single_win = |wid: &str, label: &str, bid: &str, blabel: &str| WindowView {
            window_id: wid.to_owned(),
            label: label.to_owned(),
            focused_block_id: Some(bid.to_owned()),
            zoomed_block_id: None,
            layers: vec![LayerView {
                layer_id: format!("layer-{wid}"),
                kind: "tiled".to_owned(),
                bounds: [0.0, 0.0, 1.0, 1.0],
                state: Some("normal".to_owned()),
                layout: Some(LayoutNode { rect: [0.0, 0.0, 1.0, 1.0], block_id: Some(bid.to_owned()), split: None }),
                blocks: vec![mk_block(bid, blabel)],
            }],
        };
        let win2 = mk_single_win("win-2", "server-logs", "demo-logs", "rex server logs");
        // Native nested child-tab subagents under `win-1` (depth 1 + depth 2 grandchild!)
        let win_sub1 = mk_single_win("win-sub-1", "API Audit", "demo-sub-1", "claude — API Audit");
        let win_sub1a = mk_single_win("win-sub-1a", "Schema Diff", "demo-sub-1a", "agy — Schema Diff");
        let win_sub2 = mk_single_win("win-sub-2", "Unit Tests", "demo-sub-2", "codex — Unit Tests");

        let active_wid = if std::env::var("OLO_REX_DEMO_ACTIVE_CHILD").ok().as_deref() == Some("1") { "win-sub-1a" } else { "win-1" };
        let main_view = SessionView {
            revision: 1,
            session_id: "demo-main".to_owned(),
            label: "main".to_owned(),
            active_window_id: Some(active_wid.to_owned()),
            windows: vec![win1, win2, win_sub1, win_sub1a, win_sub2],
        };

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
        // (What it says of itself, as an agent's status does: Activity's
        // line under its name, on two lines where one won't hold it.)
        br_agent.program_statuses =
            vec![ProgramStatusRecord { state: "working".into(), msg: Some("Running visual QA on the terminal pane's renderer at 1x, 1.5x and 2x".into()), ..Default::default() }];
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

        // Native child-tab subagents under `win-1` (`olo-rex`): depth-1 workers + depth-2 grandchild
        let mut br_sub1 = BlockRuntime::new(110, 32, &theme);
        br_sub1.title = "claude — API Audit".to_owned();
        br_sub1.pwd = "/home/you/olo/rex".to_owned();
        br_sub1.agent_activity = AgentActivity::Working;
        br_sub1.program_statuses = vec![ProgramStatusRecord { state: "working".into(), msg: Some("Checking action schemas in keymap.rs".into()), ..Default::default() }];
        br_sub1.data_connected = true;
        br_sub1.terminal.vt_write(
            concat!(
                "\x1b[1;35m◆ Claude Code\x1b[0m \x1b[2mclaude-opus-5-5 · effort: max\x1b[0m\r\n",
                "\x1b[2m────────────────────────────────────────────\x1b[0m\r\n",
                "\x1b[32m✓\x1b[0m Read \x1b[36mapps/rex/src/keymap.rs\x1b[0m\r\n",
                "\x1b[33m⠋\x1b[0m Checking action schemas and JSON validation…\r\n"
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-sub-1".into() }, Arc::new(Mutex::new(br_sub1)));

        let mut br_sub1a = BlockRuntime::new(110, 32, &theme);
        br_sub1a.title = "agy — Schema Diff".to_owned();
        br_sub1a.pwd = "/home/you/olo/rex".to_owned();
        br_sub1a.agent_activity = AgentActivity::Blocked;
        br_sub1a.program_statuses = vec![ProgramStatusRecord { state: "blocked".into(), msg: Some("Approve editing bridge_skill.md?".into()), ..Default::default() }];
        br_sub1a.data_connected = true;
        br_sub1a.terminal.vt_write(
            concat!(
                "\x1b[1;36m◈ Antigravity\x1b[0m \x1b[2mgemini-4-neon · effort: xhigh\x1b[0m\r\n",
                "\x1b[2m────────────────────────────────────────────\x1b[0m\r\n",
                "\x1b[32m✓\x1b[0m Compared rex actions --json against bridge_skill.md\r\n",
                "\x1b[33m?\x1b[0m Allow updating bridge_skill.md? [y/n] "
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-sub-1a".into() }, Arc::new(Mutex::new(br_sub1a)));

        let mut br_sub2 = BlockRuntime::new(110, 32, &theme);
        br_sub2.title = "codex — Unit Tests".to_owned();
        br_sub2.pwd = "/home/you/olo/rex".to_owned();
        br_sub2.agent_activity = AgentActivity::Done;
        br_sub2.program_statuses = vec![ProgramStatusRecord { state: "done".into(), msg: Some("4 new subagent tests passed".into()), ..Default::default() }];
        br_sub2.data_connected = true;
        br_sub2.terminal.vt_write(
            concat!(
                "\x1b[1;34m● Codex\x1b[0m \x1b[2mgpt-5.4-codex · effort: high\x1b[0m\r\n",
                "\x1b[2m────────────────────────────────────────────\x1b[0m\r\n",
                "\x1b[32m✓\x1b[0m cargo test -p olo-rex subagents — 4 passed, 0 failed\r\n"
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-sub-2".into() }, Arc::new(Mutex::new(br_sub2)));

        let now_s = crate::subagents::now_unix();
        st.subagents.register(crate::subagents::NativeSubagent {
            id: "sa-audit1".into(),
            name: Some("api-audit".into()),
            host_key: "local".into(),
            session_id: "demo-main".into(),
            window_id: "win-sub-1".into(),
            block_id: "demo-sub-1".into(),
            parent_window_id: "win-1".into(),
            parent_block_id: Some("demo-agent".into()),
            cli: crate::subagents::AgentCli::Claude,
            model: "claude-opus-5-5".into(),
            effort: "max".into(),
            title: "API Audit".into(),
            role: Some("Code Auditor".into()),
            cwd: Some("/home/you/olo/rex".into()),
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: now_s - 95,
            exited_at: None,
            last_pid: None,
        });
        st.subagents.register(crate::subagents::NativeSubagent {
            id: "sa-diff1a".into(),
            name: Some("schema-diff".into()),
            host_key: "local".into(),
            session_id: "demo-main".into(),
            window_id: "win-sub-1a".into(),
            block_id: "demo-sub-1a".into(),
            parent_window_id: "win-sub-1".into(),
            parent_block_id: Some("demo-sub-1".into()),
            cli: crate::subagents::AgentCli::Agy,
            model: "gemini-4-neon".into(),
            effort: "xhigh".into(),
            title: "Schema Diff".into(),
            role: Some("Schema Checker".into()),
            cwd: Some("/home/you/olo/rex".into()),
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: now_s - 42,
            exited_at: None,
            last_pid: None,
        });
        st.subagents.register(crate::subagents::NativeSubagent {
            id: "sa-tests2".into(),
            name: Some("unit-tests".into()),
            host_key: "local".into(),
            session_id: "demo-main".into(),
            window_id: "win-sub-2".into(),
            block_id: "demo-sub-2".into(),
            parent_window_id: "win-1".into(),
            parent_block_id: Some("demo-agent".into()),
            cli: crate::subagents::AgentCli::Codex,
            model: "gpt-5.4-codex".into(),
            effort: "high".into(),
            title: "Unit Tests".into(),
            role: Some("Test Runner".into()),
            cwd: Some("/home/you/olo/rex".into()),
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: now_s - 180,
            exited_at: None,
            last_pid: None,
        });

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
            server_newer: None,
            helper: HostHelper::None,
            helper_current: None,
            helper_sync_err: None,
            bridge_there: None,
            skill_there: None,
            skill_current: None,
            skill_sync_err: None,
            commands_there: None,
            commands_current: None,
            commands_sync_err: None,
            agent_keys: None,
            account: None,
            carried: None,
            agent_sessions: None,
            repos_there: None,
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
        };
        // (`OLO_REX_DEMO_LONG_NAME=1`: the host under a long name, as
        // real ones have: every window that says the host's name is
        // pictured with it too. With "dev-box" alone a sentence that
        // named the host fitted its line in every picture and lost its
        // end on the person's own host.)
        let named = if std::env::var_os("OLO_REX_DEMO_LONG_NAME").is_some() { "build-farm-eu-west-production-01" } else { "dev-box" };
        st.hosts.insert("dev-box".to_owned(), demo_host("dev-box", named, "ssh://you@dev-box.example", HostConnState::Connected, vec![remote]));
        st.host_order.push("dev-box".to_owned());
        // (`OLO_REX_DEMO_HOSTS=states`: hosts in the other states too,
        // for the Hosts window's pictures: connecting, disconnected, and
        // one that failed with a reason long enough to be cut.)
        if std::env::var("OLO_REX_DEMO_HOSTS").ok().as_deref() == Some("states") {
            for (key, label, endpoint, state) in [
                ("build", "build", "ssh://you@build.example", HostConnState::Connecting),
                ("nas", "nas", "nas.ts.net", HostConnState::Disconnected),
                (
                    "staging",
                    "staging",
                    "ssh://deploy@staging.example:2222",
                    // (`OLO_REX_DEMO_REASON=long`: a reason longer than the
                    // Hosts window's line, for the picture of it cut.)
                    HostConnState::Error(if std::env::var("OLO_REX_DEMO_REASON").ok().as_deref() == Some("long") {
                        "Permission denied: the host rejected all the keys your SSH agent offered (ed25519 SHA256:demo, rsa SHA256:demo), and it takes no passwords from this address.".to_owned()
                    } else {
                        "Permission denied: the host rejected all the keys your SSH agent offered.".to_owned()
                    }),
                ),
            ] {
                st.hosts.insert(key.to_owned(), demo_host(key, label, endpoint, state, Vec::new()));
                st.host_order.push(key.to_owned());
            }
        }
    }

    pub fn connect_host(&self, host_key: &str) {
        let key = host_key.to_owned();
        LET_GO.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| k != host_key);
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
    /// The kept folders as the desktop's settings have them: each as it
    /// stood where it was kept before, a new one waiting; runners started
    /// for those of connected hosts, those let go of stopped.
    fn follow_kept(&self, kept: &[olo_protocol::RexKept]) {
        let (stop, connected) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let before = std::mem::take(&mut st.kept);
            let mut changed_cfg: Vec<(String, String)> = Vec::new();
            st.kept = kept
                .iter()
                .map(|k| {
                    before
                        .iter()
                        .find(|b| b.host_key == k.host && b.there == k.there)
                        .cloned()
                        .map(|b| {
                            if b.mirror != k.mirror || b.direction != k.direction || b.shallow != k.shallow || b.exclude != k.exclude {
                                changed_cfg.push((k.host.clone(), k.there.clone()));
                            }
                            Kept { mirror: k.mirror, direction: k.direction, shallow: k.shallow, exclude: k.exclude.clone(), ..b }
                        })
                        .unwrap_or(Kept {
                            host_key: k.host.clone(),
                            there: k.there.clone(),
                            mirror: k.mirror,
                            direction: k.direction,
                            shallow: k.shallow,
                            exclude: k.exclude.clone(),
                            conflicts: Vec::new(),
                            state: KeptState::Waiting,
                            last: None,
                        })
                })
                .collect();
            let mut stop: Vec<(String, String)> =
                before.iter().filter(|b| !st.kept.iter().any(|k| k.host_key == b.host_key && k.there == b.there)).map(|b| (b.host_key.clone(), b.there.clone())).collect();
            stop.extend(changed_cfg);
            let connected: Vec<String> = st.kept.iter().map(|k| k.host_key.clone()).filter(|h| st.hosts.get(h).is_some_and(|h| h.conn_state == HostConnState::Connected)).collect();
            (stop, connected)
        };
        for (host, there) in stop {
            self.inner.stop_keeping(&host, &there);
        }
        let _in_runtime = self.rt.enter();
        for host in connected {
            self.inner.keep_folders_of(&host);
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// Sync kept port forwards from desktop `RexSettings::kept_forwards`,
    /// stopping any removed kept forwards and opening newly added ones on
    /// connected hosts.
    fn follow_kept_forwards(&self, kept: &[olo_protocol::RexKeptForward]) {
        let to_open: Vec<(String, ControlClient, RexEndpoint)> = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.kept_forwards == kept {
                return;
            }
            st.kept_forwards = kept.to_vec();
            let valid_keys: std::collections::HashSet<_> = st.kept_forwards.iter().map(kept_forward_key).collect();
            st.kept_forward_errors.retain(|k, _| valid_keys.contains(k));
            // Mark matching active forwards as kept; drop active kept forwards
            // that were removed from settings.
            let kept_snapshot = st.kept_forwards.clone();
            st.forwards.retain_mut(|af| {
                let is_kept = kept_snapshot.iter().any(|kf| kept_matches_active(kf, af));
                if af.kept && !is_kept {
                    return false;
                }
                if is_kept {
                    af.kept = true;
                }
                true
            });
            let hosts: std::collections::HashSet<String> = st.kept_forwards.iter().map(|kf| kf.host.clone()).collect();
            hosts
                .into_iter()
                .filter_map(|hk| {
                    let h = st.hosts.get(&hk)?;
                    if h.conn_state == HostConnState::Connected { Some((hk, h.control.clone()?, h.endpoint.clone()?)) } else { None }
                })
                .collect()
        };
        let _in_runtime = self.rt.enter();
        for (hk, ctrl, ep) in to_open {
            self.inner.ensure_kept_forwards(&hk, &ctrl, &ep);
        }
        (self.notify)(HubNotice::StateChanged);
    }

    pub fn follow_shared(&self, rex: &olo_protocol::RexSettings) {
        // (The person's repositories, as the settings have them; the
        // demo's own, where it has some.)
        {
            let repos: Vec<olo_rex_client::repos::Repo> = match demo_repos() {
                demo if !demo.is_empty() => demo,
                _ => rex.repos.iter().map(|r| olo_rex_client::repos::Repo { url: r.url.clone(), folder: r.folder.clone(), setup: r.setup.clone() }).collect(),
            };
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let window = olo_rex_client::sessions::Window::of_days(rex.sessions_days);
            if st.sessions_window != window {
                st.sessions_window = window;
                for host in st.hosts.values_mut() {
                    host.agent_sessions = None;
                }
            }
            if st.repos != repos {
                st.repos = repos;
                for host in st.hosts.values_mut() {
                    host.repos_there = None;
                }
            }
            if st.carry_choices != rex.carry_choices {
                st.carry_choices.clone_from(&rex.carry_choices);
            }
        }
        crate::carry::sync_remembered(self);
        self.follow_kept(&rex.kept);
        self.follow_kept_forwards(&rex.kept_forwards);
        if !crate::shared::settled(rex) {
            return;
        }
        // Typing over a slow link and Keep It Going's message are the
        // desktop's (Preferences ▸ Rex): what Rex's own file had of them,
        // carried there once (where the desktop's still say what they come
        // with), then followed.
        let carry = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let fresh = olo_protocol::RexSettings::default();
            let ours = (st.settings.type_ahead, st.settings.host_helper, st.settings.keep_going_message.clone());
            let theirs_fresh = (rex.show_typing_at_once, rex.typing_helper, rex.keep_going_says.as_str()) == (fresh.show_typing_at_once, fresh.typing_helper, fresh.keep_going_says.as_str());
            let first = !st.settings.carried_typing_to_preferences;
            if first {
                st.settings.carried_typing_to_preferences = true;
                st.settings.save();
            }
            (first && theirs_fresh && ours != (fresh.show_typing_at_once, fresh.typing_helper, fresh.keep_going_says.clone())).then_some(ours)
        };
        if let Some((type_ahead, helper, says)) = carry {
            olo_kit::settings::update(move |d| {
                d.rex.show_typing_at_once = type_ahead;
                d.rex.typing_helper = helper;
                d.rex.keep_going_says = says;
            });
        } else {
            let (type_ahead, helper, says) = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                (st.settings.type_ahead, st.settings.host_helper, st.settings.keep_going_message.clone())
            };
            if type_ahead != rex.show_typing_at_once {
                self.set_type_ahead(rex.show_typing_at_once);
            }
            if helper != rex.typing_helper {
                self.set_host_helper(rex.typing_helper);
            }
            if says != rex.keep_going_says && !rex.keep_going_says.trim().is_empty() {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.keep_going_message = rex.keep_going_says.clone();
                st.settings.save();
            }
        }
        let (changes, agents) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // (What Rex's own file held from before, if this start
            // couldn't read the desktop's settings to carry it over.)
            let pending = st.settings.before_preferences.is_some();
            let rex = crate::shared::adopt(&mut st.settings, rex.clone());
            let had_agent = |st: &HubState| -> Vec<(String, bool)> { st.hosts.keys().map(|id| (id.clone(), st.settings.policy_for_host(id, false).allow_ssh_agent)).collect() };
            let before = had_agent(&st);
            let changes = crate::shared::apply(&rex, &mut st.settings);
            if changes.any || pending {
                st.settings.save();
            }
            for host in &changes.renamed {
                if let Some(h) = st.hosts.get_mut(&host.id) {
                    h.label.clone_from(&host.label);
                }
            }
            // (The hosts whose leave to use the SSH keys was given or
            // taken back, and that are connected: told now, not at
            // their next connection.)
            let agents: Vec<(String, RexEndpoint)> = had_agent(&st)
                .into_iter()
                .filter(|now| !before.contains(now))
                .filter_map(|(id, _)| st.hosts.get(&id).filter(|h| h.control.is_some()).and_then(|h| h.endpoint.clone()).map(|endpoint| (id, endpoint)))
                .collect();
            (changes, agents)
        };
        if !agents.is_empty() {
            let _in_runtime = self.rt.enter();
            for (id, endpoint) in &agents {
                self.inner.give_the_agent(id, endpoint);
            }
        }
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
                    server_newer: None,
                    helper: HostHelper::None,
                    helper_current: None,
                    helper_sync_err: None,
                    bridge_there: None,
                    skill_there: None,
                    skill_current: None,
                    skill_sync_err: None,
                    commands_there: None,
                    commands_current: None,
                    commands_sync_err: None,
                    agent_keys: None,
                    account: None,
                    carried: None,
                    agent_sessions: None,
                    repos_there: None,
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
                },
            );
            if !st.settings.saved_hosts.iter().any(|h| h.id == id) {
                st.settings.saved_hosts.push(SavedHost { id: id.clone(), label: label.clone(), endpoint: endpoint.clone(), icon: None, auto_connect: true, ssh_agent_socket: None });
                st.settings.save();
            }
        }
        (self.notify)(HubNotice::StateChanged);
        if tell_desktop {
            let host = olo_protocol::RexHost { id: id.clone(), label: label.clone(), endpoint: endpoint.clone(), connect_at_start: true, grants: Default::default(), moved_to: None };
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

    /// Lets go of a host that is connected, or on its way (Hosts and
    /// Ports' Disconnect, where Connect is for one that isn't): its
    /// connection to the server closed, with the SSH connection under
    /// it and what went with that (the link over UDP, the forwarded
    /// keys), its panes gone from the windows and its forwards waiting.
    /// The host stays saved and its sessions go on on its server:
    /// Connect brings them back. Not this computer's own server, which
    /// Rex is nothing without.
    fn detach_host(&self, host_key: &str) -> Option<(olo_rex_client::SshTarget, Arc<olo_rex_client::SshSessionPool>)> {
        if host_key == "local" {
            return None;
        }
        let endpoint = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let host = st.hosts.get_mut(host_key)?;
            // (The loop that keeps it connected ends at its next look:
            // it is no longer the one the host names.)
            if let Some(retry) = host.retry.take() {
                retry.notify_one();
            }
            if let Some(ctrl) = host.control.take() {
                ctrl.close();
            }
            host.input_tx = None;
            host.tailscale = None;
            host.server_status = None;
            host.sessions.clear();
            host.pending_focus.clear();
            host.session_summaries.clear();
            host.connection_lost = None;
            host.conn_state = HostConnState::Disconnected;
            let endpoint = host.endpoint.clone();
            for f in st.forwards.iter_mut().filter(|f| f.host_key == host_key) {
                f.state = ForwardState::Waiting;
            }
            st.retain_blocks(|k| k.host_key != host_key);
            endpoint
        };
        {
            let mut let_go = LET_GO.lock().unwrap_or_else(|e| e.into_inner());
            if !let_go.iter().any(|k| k == host_key) {
                let_go.push(host_key.to_owned());
            }
        }
        self.inner.write_palette_index();
        (self.notify)(HubNotice::StateChanged);
        match endpoint {
            Some(RexEndpoint::Ssh { target, pool, .. }) => Some((target, pool)),
            _ => None,
        }
    }

    pub fn disconnect_host(&self, host_key: &str) {
        if let Some((target, pool)) = self.detach_host(host_key) {
            self.rt.spawn(async move { pool.close(&target).await });
        }
    }

    /// Like [`Self::disconnect_host`], waiting for the host's SSH sessions to
    /// finish closing before returning (so a caller that immediately opens a
    /// fresh SSH command on the same pool doesn't race with `pool.close`).
    pub async fn disconnect_host_and_wait(&self, host_key: &str) {
        if let Some((target, pool)) = self.detach_host(host_key) {
            pool.close(&target).await;
        }
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
    /// every terminal (Preferences ▸ Rex ▸ General).
    /// Olo's helper on the hosts, used or not (Preferences ▸ Rex ▸
    /// General). From each host's next connection: one that
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
    /// Commands run on `target`'s host: as its account, and as root as
    /// `root_by` says (a `root@` login with keys, `root_key` the one the
    /// person chose; or sudo over the account's connection).
    pub fn over_ssh(&self, target: olo_rex_client::SshTarget, root_key: Option<String>, root_by: olo_rex_client::provision::RootBy) -> olo_rex_client::provision::OverSsh {
        olo_rex_client::provision::OverSsh { pool: self.inner.ssh_pool.clone(), target, root_key, root_by, askpass: self.inner.askpass.clone() }
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
        } else if let Some(agent) = self.inner.agent_socks.lock().unwrap_or_else(|e| e.into_inner()).get(host_key) {
            // (Passwords' SSH agent, where the host has been given it:
            // what Rex opens there finds it. A terminal that was there
            // before, or that something else opens, is told by hand:
            // the path is the same at every connection.)
            opts["env"] = json!({ "SSH_AUTH_SOCK": agent });
            any = true;
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

    /// Gracefully stops and closes a native subagent child tab (and optionally its
    /// entire descendant subtree bottom-up when `tree` is true).
    pub fn stop_subagent(&self, host_key: &str, session_id: &str, window_id: &str, force: bool, tree: bool) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let wid = window_id.to_owned();
        self.with_control(host_key, move |_ctrl, _ep, inner| async move {
            let _ = inner.stop_subagent_windows(&hk, &sid, &wid, force, tree, false).await;
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

    /// What a pane has on: Keep It Going, Auto Approve.
    pub fn help(&self, key: &BlockKey) -> Help {
        help_of(key)
    }

    /// Keep It Going or Auto Approve turned on or off for a pane (`change`
    /// says how). On, the pane is looked at now: a question already
    /// showing waits its time as one that has just come; off, nothing of
    /// it waits.
    pub fn set_help(&self, key: &BlockKey, change: impl FnOnce(&mut Help)) {
        let help = {
            let mut all = HELP.lock().unwrap_or_else(|e| e.into_inner());
            let h = all.entry(key.clone()).or_default();
            change(h);
            if !h.auto_approve {
                h.theirs = None;
            }
            let help = h.clone();
            if !help.any() {
                all.remove(key);
            }
            help
        };
        tracing::info!(pane = key.block_id, keep_going = help.keep_going, auto_approve = help.auto_approve, "agent help");
        if let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned() {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if !help.auto_approve {
                g.watch.pending = None;
            }
            if !help.any() {
                for t in [g.watch_timer.take(), g.watch_ticker.take()].into_iter().flatten() {
                    t.abort();
                }
            }
        }
        let _in = self.rt.enter();
        self.inner.agent_look(key);
        (self.notify)(HubNotice::StateChanged);
    }

    /// Stop in Auto Approve's countdown: the question is the person's.
    pub fn stop_auto_approve(&self, key: &BlockKey) {
        let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned() else { return };
        if let Some(theirs) = br.lock().unwrap_or_else(|e| e.into_inner()).watch.stop()
            && let Some(h) = HELP.lock().unwrap_or_else(|e| e.into_inner()).get_mut(key)
        {
            h.theirs = Some(theirs);
        }
        let _in = self.rt.enter();
        self.inner.agent_schedule(key, &br, &help_of(key));
        (self.notify)(HubNotice::StateChanged);
    }

    pub fn write_input(&self, key: &BlockKey, data: Vec<u8>, connection: Option<&ControlClient>) {
        // (Typed by the person: a question waiting is theirs.)
        {
            let _in = self.rt.enter();
            self.inner.agent_typed(key);
        }
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
        let (block, last_typed) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (st.blocks.get(key).cloned(), st.hosts.get(&key.host_key).and_then(|h| h.control.as_ref()).map_or(0, |ctrl| ctrl.last_typed()))
        };
        let Some(block) = block else { return 0 };
        let (seq, changed) = {
            let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
            // (Typed on what was last drawn of it.)
            let screen = b.caret_screen();
            // (Where the host's helper has said its shell reads here and
            // a known prompt has been drawn since: this key is drawn at
            // once.)
            b.host_says_prompt(last_typed, screen.as_ref());
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
        predict_wake(self.rt.handle(), &self.state, block, held, key, &self.notify, at);
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

    pub fn resize_block(&self, key: &BlockKey, cols: u16, rows: u16, cell_w: u32, cell_h: u32) -> bool {
        let br_opt = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned();
        let changed = if let Some(br) = br_opt {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            !g.other_size_owner && g.resize(cols, rows, cell_w, cell_h).unwrap_or(false)
        } else {
            false
        };
        if !changed {
            return false;
        }
        let sid = key.session_id.clone();
        let bid = key.block_id.clone();
        self.with_control(&key.host_key, move |ctrl, _, _| async move {
            let _ = ctrl.terminal_resize(&sid, &bid, cols, rows, Some(cell_w as u16), Some(cell_h as u16)).await;
        });
        true
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

    /// Download a file via `com.superlogical.terminal.download_begin` + `rex.data.bytes.v1`
    /// with `if_none_match` ETag caching for instant Quick Look (`⌘Y`).
    /// Resolve a path typed or clicked in a terminal pane against its `pwd`.
    /// Keeps absolute paths and `~` / `~/…` intact, and joins relative paths
    /// against `pwd` when available.
    pub fn resolve_pane_path(path: &str, pwd: &str) -> String {
        let p = path.trim();
        if p.starts_with('/') || p == "~" || p.starts_with("~/") || p.starts_with(olo_kit_base::tracker::REX_SCHEME) || pwd.trim().is_empty() {
            return p.to_owned();
        }
        let base = pwd.trim().trim_end_matches('/');
        let rel = p.trim_start_matches("./");
        if base.is_empty() { format!("/{rel}") } else { format!("{base}/{rel}") }
    }

    /// Reveal any file or open any folder in Olo Tracker — locally or on a
    /// remote Rex host (`rex://HOST/PATH`) — from a terminal pane (Cmd+click).
    /// Works for any remote path the user has permissions to access (e.g.
    /// `/var/tmp/…`, `/etc/…`, `~/…`), never invoking Quick Look directly.
    pub fn reveal_path_in_tracker(&self, key: &BlockKey, path: String, feedback: Feedback) {
        if path.starts_with(olo_kit_base::tracker::REX_SCHEME) {
            let uri = path;
            self.rt.spawn_blocking(move || {
                if let Err(e) = olo_kit_base::tracker::open_uri(&uri) {
                    feedback.problem("Couldn’t open in Tracker.", e);
                }
            });
            return;
        }
        let (is_local, host_label, pwd) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let pwd = st.blocks.get(key).map(|b| b.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone()).unwrap_or_default();
            let h = st.hosts.get(&key.host_key);
            (h.is_some_and(|h| h.is_local), h.map(|h| h.label.clone()).unwrap_or_default(), pwd)
        };
        let resolved = Self::resolve_pane_path(&path, &pwd);
        let target = if is_local {
            let expanded = if resolved == "~" {
                local_home().unwrap_or_else(|| "/".into())
            } else if let Some(rest) = resolved.strip_prefix("~/") {
                match local_home() {
                    Some(h) => format!("{}/{rest}", h.trim_end_matches('/')),
                    None => resolved,
                }
            } else {
                resolved
            };
            expanded
        } else {
            olo_kit_base::tracker::RexPath::new(host_label, &resolved).to_uri()
        };
        self.rt.spawn_blocking(move || {
            if let Err(e) = olo_kit_base::tracker::open_uri(&target) {
                feedback.problem("Couldn’t open in Tracker.", e);
            }
        });
    }

    /// Quick Look: the file at `path` (as the pane names it) opened in
    /// Olo's own app for its kind (Viewer for a picture, Styled Edit for
    /// text, …), as Tracker opens one. On this computer, where it is; on a
    /// host, copied first into Rex's folder of them
    /// (`<cache>/rex-quicklook/HOST/…/NAME`), streamed to disk with its
    /// progress on the pane, stoppable, and kept: asked again, it is
    /// fetched only where it has changed (the server's version). (Rex's
    /// own preview, whole in memory and shown in an overlay of its own, was
    /// "some weird crippled viewer type thing", Ben, 2026-11-24.)
    pub fn quick_look_path(&self, key: &BlockKey, path: String, feedback: Feedback) -> Option<tokio::task::AbortHandle> {
        let (is_local, pwd) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let pwd = st.blocks.get(key).map(|b| b.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone()).unwrap_or_default();
            (st.hosts.get(&key.host_key).is_some_and(|h| h.is_local), pwd)
        };
        if is_local {
            let p = PathBuf::from(&path);
            let p = if p.is_absolute() || pwd.is_empty() { p } else { PathBuf::from(&pwd).join(p) };
            if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Path(p)) {
                feedback.problem("Couldn’t open the file.", e);
            }
            return None;
        }
        let cache_key = (key.host_key.clone(), key.block_id.clone(), path.clone());
        let (sid, bid) = (key.session_id.clone(), key.block_id.clone());
        let cached = self.state.lock().unwrap_or_else(|e| e.into_inner()).quick_look_cache.get(&cache_key).cloned();
        let bk = key.clone();
        let job = self.with_control(&key.host_key, move |ctrl, ep, inner| async move {
            // (Its progress taken off the pane however it ends: done,
            // failed, or stopped.)
            struct Clear(Arc<RexHubInner>, BlockKey);
            impl Drop for Clear {
                fn drop(&mut self) {
                    if let Some(br) = self.0.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&self.1).cloned() {
                        br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress = None;
                    }
                    (self.0.notify)(HubNotice::StateChanged);
                }
            }
            let _clear = Clear(inner.clone(), bk.clone());
            let name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("file").to_owned();
            let host = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&bk.host_key).map(|h| h.label.clone()).unwrap_or_default();
            let received = async {
                let data_ref = transfer_reference("download");
                let version = cached.as_ref().filter(|e| e.local.exists()).and_then(|e| e.version.as_deref());
                let resp = ctrl.terminal_download_begin(&sid, &bid, &data_ref, &path, version).await?.payload;
                if resp.not_modified
                    && let Some(e) = cached.as_ref().filter(|e| e.local.exists())
                {
                    return Ok(e.local.clone());
                }
                let ticket = resp.ticket.ok_or_else(|| olo_rex_client::RexError::Protocol("The server didn’t provide a download ticket.".into()))?;
                let local = quick_look_file(&bk.host_key, &path);
                if let Some(dir) = local.parent() {
                    tokio::fs::create_dir_all(dir).await?;
                }
                let part = local.with_extension("rex-part");
                let (stream, header) = ep.connect_stream().await?;
                let mut stream = BytesDataStream::connect_hello(stream, &header, &ticket).await?;
                let mut file = tokio::fs::File::create(&part).await?;
                let (mut got, mut said) = (0u64, None::<Instant>);
                // (A test's own slow link: a pause after each chunk, so its
                // progress and Stop can be seen. Never set otherwise.)
                let pause = std::env::var("OLO_REX_TRANSFER_PAUSE_MS").ok().and_then(|v| v.parse().ok()).map(Duration::from_millis);
                loop {
                    use tokio::io::AsyncWriteExt;
                    if let Some(p) = pause {
                        tokio::time::sleep(p).await;
                    }
                    match stream.read_chunk().await? {
                        Some(chunk) => {
                            got += chunk.len() as u64;
                            if got > resp.size {
                                return Err(olo_rex_client::RexError::Protocol("The file is larger than the server reported.".into()));
                            }
                            file.write_all(&chunk).await?;
                            if said.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                                if let Some(br) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                                    br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress =
                                        Some(Transfer { said: olo_kit::tr!("Downloading {} from {}…", name, host), done: got, total: resp.size, stoppable: true });
                                }
                                (inner.notify)(HubNotice::StateChanged);
                                said = Some(Instant::now());
                            }
                        }
                        None if got == resp.size => break,
                        None => return Err(olo_rex_client::RexError::Protocol(format!("Received {got} of {} bytes.", resp.size))),
                    }
                }
                {
                    use tokio::io::AsyncWriteExt;
                    file.flush().await?;
                }
                tokio::fs::rename(&part, &local).await?;
                inner.state.lock().unwrap_or_else(|e| e.into_inner()).quick_look_cache.insert(cache_key, QuickLookCacheEntry { version: resp.version, local: local.clone() });
                Ok::<PathBuf, olo_rex_client::RexError>(local)
            }
            .await;
            match received {
                Ok(local) => {
                    if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Path(local)) {
                        feedback.problem("Couldn’t open the file.", e);
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    let lower = msg.to_ascii_lowercase();
                    if lower.contains("is a directory") || lower.contains("not a regular file") {
                        let resolved = RexHub::resolve_pane_path(&path, &pwd);
                        let uri = olo_kit_base::tracker::RexPath::new(&host, &resolved).to_uri();
                        if let Err(err) = tokio::task::spawn_blocking(move || olo_kit_base::tracker::open_uri(&uri)).await.unwrap_or_else(|je| Err(std::io::Error::other(je.to_string()))) {
                            feedback.problem("Couldn’t open the folder in Tracker.", err);
                        }
                    } else {
                        feedback.problem("Couldn’t copy the file from the host.", e);
                    }
                }
            }
        });
        if job.is_none() {
            feedback.problem("Couldn’t copy the file from the host.", "The Rex server is disconnected.");
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
                                br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress =
                                    Some(Transfer { said: olo_kit::tr!("Uploading {}…", filename), done: sent, total: size, stoppable: false });
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

    #[allow(clippy::too_many_arguments)]
    pub fn add_local_forward(&self, host_key: &str, session_id: &str, remote_host: String, remote_port: u16, local_port: u16, open_in_browser: bool, keep: bool, feedback: Feedback) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        if self
            .with_control(host_key, move |ctrl, ep, inner| async move {
                match inner.create_forward(&hk, &ctrl, ep, &sid, remote_host.clone(), remote_port, local_port, crate::forwards::Made::Person, keep).await {
                    Ok(local_addr) => {
                        if keep {
                            let entry = olo_protocol::RexKeptForward { host: hk.clone(), remote_host, remote_port, local_port, made: olo_protocol::RexForwardMaker::Person };
                            inner.remember_kept_forward(entry);
                        }
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

    /// The forwards as kept over an upgrade in place: each one here (its
    /// block is left open as the process is replaced, `exec` running no
    /// destructor), and those of before that no connection has taken up
    /// yet.
    pub fn saved_forwards(&self) -> Vec<SavedForward> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut kept: Vec<SavedForward> = st
            .forwards
            .iter()
            .filter(|f| !matches!(f.state, ForwardState::Failed(_)))
            .map(|f| SavedForward {
                host_key: f.host_key.clone(),
                session_id: f.handle.session_id.clone(),
                block_id: f.handle.block_id.clone(),
                remote_host: f.remote_host.clone(),
                remote_port: f.remote_port,
                local_port: f.local_addr.port(),
                made: f.made,
                kept: f.kept,
            })
            .collect();
        drop(st);
        kept.extend(SAVED_FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
        kept
    }

    /// A kept folder let go of (the Host window's Stop), what it brought
    /// staying: `RexHubInner::stop_kept`.
    pub fn stop_kept(&self, host_key: &str, there: &str) {
        self.inner.stop_kept(host_key, there);
    }

    /// A kept folder that failed kept in step again (the Host window's Try
    /// Again): `RexHubInner::retry_kept`.
    pub fn retry_kept(&self, host_key: &str, there: &str) {
        self.inner.retry_kept(host_key, there);
    }

    /// Trigger an immediate sync of a kept folder (the Host window's Sync Now).
    pub fn sync_kept_now(&self, host_key: &str, there: &str) {
        let inner = self.inner.clone();
        let (hk, path) = (host_key.to_owned(), there.to_owned());
        self.rt.spawn(async move {
            inner.sync_kept_immediate(&hk, &path).await;
        });
    }

    pub fn remove_forward(&self, forward_id: &str) {
        let mut forgotten: Vec<(String, String, u16, u16)> = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pos) = st.forwards.iter().position(|f| f.id == forward_id) {
                let removed = st.forwards.remove(pos);
                if removed.kept {
                    st.kept_forwards.retain(|kf| {
                        let m = kept_matches_active(kf, &removed);
                        if m {
                            forgotten.push(kept_forward_key(kf));
                        }
                        !m
                    });
                }
            } else {
                // Waiting or failed kept forward listed under its synthetic id.
                st.kept_forwards.retain(|kf| {
                    let m = kept_forward_id(kf) == forward_id;
                    if m {
                        forgotten.push(kept_forward_key(kf));
                    }
                    !m
                });
            }
            for k in &forgotten {
                st.kept_forward_errors.remove(k);
            }
        }
        if !forgotten.is_empty() {
            olo_kit::settings::update(move |s| {
                s.rex.kept_forwards.retain(|kf| !forgotten.contains(&kept_forward_key(kf)));
            });
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// Toggle whether an existing forward in the Ports list is kept across
    /// Rex restarts and host reconnects.
    pub fn set_forward_kept(&self, forward_id: &str, keep: bool) {
        let mut to_add: Option<olo_protocol::RexKeptForward> = None;
        let mut forgotten: Vec<(String, String, u16, u16)> = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(f) = st.forwards.iter_mut().find(|f| f.id == forward_id) {
                if f.kept == keep {
                    return;
                }
                f.kept = keep;
                let snap = (f.host_key.clone(), f.remote_host.clone(), f.remote_port, f.local_addr.port(), f.made);
                if keep {
                    let entry = olo_protocol::RexKeptForward {
                        host: snap.0,
                        remote_host: snap.1,
                        remote_port: snap.2,
                        local_port: if snap.4 == crate::forwards::Made::Agent { snap.3 } else { 0 },
                        made: made_to_maker(snap.4),
                    };
                    let key = kept_forward_key(&entry);
                    if !st.kept_forwards.iter().any(|k| kept_forward_key(k) == key) {
                        st.kept_forwards.push(entry.clone());
                    }
                    to_add = Some(entry);
                } else {
                    st.kept_forwards.retain(|kf| {
                        let m = kf.host == snap.0 && kf.remote_host == snap.1 && kf.remote_port == snap.2 && (kf.local_port == 0 || kf.local_port == snap.3);
                        if m {
                            forgotten.push(kept_forward_key(kf));
                        }
                        !m
                    });
                }
            } else if !keep {
                // Unkeeping a waiting/failed kept forward removes it from kept settings.
                st.kept_forwards.retain(|kf| {
                    let m = kept_forward_id(kf) == forward_id;
                    if m {
                        forgotten.push(kept_forward_key(kf));
                    }
                    !m
                });
            }
            for k in &forgotten {
                st.kept_forward_errors.remove(k);
            }
        }
        if let Some(entry) = to_add {
            let key = kept_forward_key(&entry);
            olo_kit::settings::update(move |s| {
                if !s.rex.kept_forwards.iter().any(|k| kept_forward_key(k) == key) {
                    s.rex.kept_forwards.push(entry);
                }
            });
        } else if !forgotten.is_empty() {
            olo_kit::settings::update(move |s| {
                s.rex.kept_forwards.retain(|kf| !forgotten.contains(&kept_forward_key(kf)));
            });
        }
        (self.notify)(HubNotice::StateChanged);
    }

    pub fn open_link_from_pane(&self, key: &BlockKey, url: String, feedback: Feedback) {
        // `rex://HOST/PATH` and `rex://local/PATH` URIs always open/reveal in Olo Tracker.
        if url.starts_with(olo_kit_base::tracker::REX_SCHEME) {
            self.rt.spawn_blocking(move || {
                if let Err(e) = olo_kit_base::tracker::open_uri(&url) {
                    feedback.problem("Couldn’t open in Tracker.", e);
                }
            });
            return;
        }
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

    /// The host was just told to put the skill there
    /// (a script typed into a pane): it is asked what it has once that
    /// has had time to run, and once more for a slow one.
    fn look_again_at_what_the_host_has(&self, host_key: &str) {
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            for wait in [3, 12] {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                let endpoint = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).and_then(|h| h.endpoint.clone());
                let Some(endpoint) = endpoint else { return };
                inner.look_at_what_the_host_has(&host_key, &endpoint);
            }
        });
    }

    /// Records whether a newer Rex server package was found for the
    /// host matching `target` (reported by the Server window's check).
    pub fn set_server_newer(&self, target: &olo_rex_client::SshTarget, newer: Option<String>) {
        let mut changed = false;
        {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            for h in st.hosts.values_mut() {
                if let Some(RexEndpoint::Ssh { target: ep_target, .. }) = &h.endpoint
                    && ep_target.host == target.host
                    && ep_target.port == target.port
                    && ep_target.user == target.user
                    && h.server_newer != newer
                {
                    h.server_newer = newer.clone();
                    changed = true;
                }
            }
        }
        if changed {
            (self.notify)(HubNotice::StateChanged);
        }
    }

    /// One of Olo's things is put on a far host, updated, or taken off it,
    /// over SSH and off the connection's own path (never typed into a
    /// pane, where a program running there would take it): `done`,
    /// on the hub's threads, once the host has answered, with what it
    /// said if it couldn't. The host is asked what it has again
    /// before that.
    pub fn change_host(&self, host_key: &str, thing: HostThing, op: HostOp, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            // (A shell's command there; what it wrote last, if it failed.)
            let run = |command: String| {
                let (pool, target, askpass) = (pool.clone(), target.clone(), askpass.clone());
                async move {
                    match pool.exec(&target, askpass, &command, &mut |_| {}).await {
                        Ok(ran) if ran.ok() => Ok(()),
                        Ok(ran) => Err(ran.output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn’t say why").trim().to_owned()),
                        Err(e) => Err(e.to_string()),
                    }
                }
            };
            let script = |script: &str| format!("sh -c {}", shell_quote(script));
            let result = match (thing, op) {
                (HostThing::Helper, HostOp::Install | HostOp::Update) => {
                    inner.helper_kept_off(&host_key, false);
                    if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                        host.helper_sync_err = None;
                        if host.helper != HostHelper::Used {
                            host.helper = HostHelper::Installing;
                        }
                    }
                    (inner.notify)(HubNotice::StateChanged);
                    let stands = olo_rex_client::helper::ensure(&pool, &target, askpass.clone(), crate::link::ID, &crate::link::BUILDS).await;
                    let said = match &stands {
                        Ok(olo_rex_client::helper::Stands::NoBuild(kind)) => Err(olo_kit::tr!("Olo has no typing helper for {}.", kind).to_string()),
                        Err(failed) => Err(failed.to_string()),
                        Ok(_) => Ok(()),
                    };
                    inner.helper_stands(&host_key, stands, false);
                    said
                }
                (HostThing::Helper, HostOp::Remove) => {
                    let ran = run(script(olo_rex_client::helper::REMOVE)).await;
                    if ran.is_ok() {
                        inner.helper_kept_off(&host_key, true);
                        // (A connection that goes through it goes on
                        // through it: said so while it does.)
                        if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                            host.helper_current = None;
                            host.helper_sync_err = None;
                            if host.helper != HostHelper::Used {
                                host.helper = HostHelper::None;
                            }
                        }
                    }
                    ran
                }
                // (Nothing of the bridge is on a host: `bridge`.)
                (HostThing::Bridge, _) => Ok(()),
                // (This computer's Token Pool does it; asked, and how
                // it stands read again until it has.)
                (HostThing::Pool, op) => {
                    let (key, what) = (
                        host_key.clone(),
                        match op {
                            HostOp::Install => "install",
                            HostOp::Update => "sync",
                            HostOp::Remove => "remove",
                        },
                    );
                    match tokio::task::spawn_blocking(move || crate::pool::act(&key, what)).await.unwrap_or_else(|e| Err(e.to_string())) {
                        Err(why) => Err(why),
                        Ok(()) => {
                            let stands = inner.pools_settled(&host_key).await;
                            match (stands, op) {
                                (Some(p), HostOp::Update) if p.in_step && p.error.is_none() => Ok(()),
                                (Some(p), HostOp::Install) if p.wanted && p.error.is_none() => Ok(()),
                                (Some(p), HostOp::Remove) if !p.wanted => Ok(()),
                                (Some(p), _) => Err(p.error.unwrap_or_else(|| olo_kit::words::tr("It didn’t say why.").to_string())),
                                (None, _) => Err(olo_kit::words::tr("Token Pool isn’t running on this computer.").to_string()),
                            }
                        }
                    }
                }
                (HostThing::Skill, HostOp::Install | HostOp::Update) => run(script(&bridge::generate_remote_skill_install_script())).await,
                (HostThing::Skill, HostOp::Remove) => run(script(bridge::REMOVE_SKILL)).await,
                (HostThing::Commands, HostOp::Install | HostOp::Update) => run(script(&bridge::install_commands_script())).await,
                (HostThing::Commands, HostOp::Remove) => run(script(bridge::REMOVE_COMMANDS)).await,
            };
            if thing != HostThing::Helper && thing != HostThing::Pool {
                inner.ask_what_the_host_has(&host_key, &pool, &target, askpass, false).await;
            }
            (inner.notify)(HubNotice::StateChanged);
            done(result);
        });
    }

    /// A far host is asked again what it has of Olo's (the Host
    /// window's Check Again: something may have been changed there by
    /// hand): the bridge and the skill, and whether this app's build
    /// of the typing helper is there. `done` once it has answered.
    /// How the far hosts' Token Pools stand, asked of this computer's
    /// Token Pool now (`pool`); the windows are told where it differs.
    pub fn ask_pools(&self) {
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            inner.ask_the_pools().await;
        });
    }

    /// Follows this computer's Token Pool while `keep` says so (a Host
    /// window is open): one question held open by the pool until its
    /// hosts stand otherwise (`pool::hosts_after`), asked again as it
    /// is answered. Nothing is asked while nothing changes. One at a
    /// time; it ends when the pool can't be asked (not running, or one
    /// from before it could hold a question), and is begun again as a
    /// Host window opens.
    pub fn watch_pools(&self, keep: impl Fn() -> bool + Send + Sync + 'static) {
        static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if WATCHING.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            let mut after = u64::MAX;
            while keep() {
                let Ok(Ok((pools, stamp))) = tokio::task::spawn_blocking(move || crate::pool::hosts_after(after)).await else { break };
                let pools = Some(pools);
                let changed = {
                    let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    let changed = st.pools != pools;
                    st.pools = pools;
                    changed
                };
                if changed {
                    (inner.notify)(HubNotice::StateChanged);
                }
                match stamp {
                    Some(stamp) => after = stamp,
                    None => break,
                }
            }
            WATCHING.store(false, std::sync::atomic::Ordering::Release);
        });
    }

    pub fn check_host(&self, host_key: &str, done: impl FnOnce() + Send + 'static) {
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done();
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            use olo_rex_client::helper::{Asked, ask};
            inner.ask_what_the_host_has(&host_key, &pool, &target, askpass.clone(), false).await;
            inner.ask_the_pools().await;
            if crate::link::carried()
                && let Ok(Ok(asked)) = ask(&pool, &target, askpass, crate::link::ID, &crate::link::BUILDS).await
                && let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key)
            {
                match asked {
                    Asked::Here => {
                        host.helper_current = Some(true);
                        host.helper_sync_err = None;
                        if matches!(host.helper, HostHelper::None | HostHelper::Without(_)) {
                            host.helper = HostHelper::Ready;
                        }
                    }
                    Asked::Outdated(_) => {
                        host.helper_current = Some(false);
                        if matches!(host.helper, HostHelper::None) {
                            host.helper = HostHelper::Ready;
                        }
                    }
                    Asked::Needs(_) => {
                        host.helper_current = if host.helper == HostHelper::Used { Some(false) } else { None };
                        if matches!(host.helper, HostHelper::Ready) {
                            host.helper = HostHelper::None;
                        }
                    }
                }
            }
            (inner.notify)(HubNotice::StateChanged);
            done();
        });
    }

    /// Asks a host how its account stands (`olo_rex_client::ready`:
    /// its agents, its path, git's identity), over SSH, and keeps the
    /// answer for the Account window (`HostStands::account`).
    pub fn ask_account(&self, host_key: &str, done: impl FnOnce() + Send + 'static) {
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done();
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            inner.ask_the_account(&host_key, &pool, &target, askpass).await;
            (inner.notify)(HubNotice::StateChanged);
            done();
        });
    }

    /// Does one step of making a host's account ready
    /// (`olo_rex_client::ready::Step`), over SSH, off the connection's
    /// own path: `said` hears each line the host writes as it goes (an
    /// installer's progress), `done` how it ended, after the host has
    /// been asked again how the account stands. `who`: this computer's
    /// name and address for git. What comes back stops it: the script
    /// there ends with its channel (`ready::cancellable`), and `done`
    /// is then never called.
    pub fn ready_step(
        &self,
        host_key: &str,
        step: olo_rex_client::ready::Step,
        who: Option<(String, String)>,
        said: impl Fn(&str) + Send + Sync + 'static,
        done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) -> Option<tokio::task::AbortHandle> {
        // (`OLO_REX_READY_FROM`: a folder of stand-in installers, for
        // Olo's own tests.)
        let Some(script) = step.run(who.as_ref(), std::env::var("OLO_REX_READY_FROM").ok().as_deref()) else {
            done(Err(olo_kit::words::tr("This computer’s git doesn’t say who you are.").into()));
            return None;
        };
        self.account_script(host_key, script, said, done)
    }

    /// Clones and sets up, on a host, those of the person's
    /// repositories that aren't there yet (`olo_rex_client::repos`),
    /// one after another, as [`Self::ready_step`] does a step: its
    /// lines heard, stopped by what comes back.
    pub fn clone_repos(&self, host_key: &str, said: impl Fn(&str) + Send + Sync + 'static, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Option<tokio::task::AbortHandle> {
        let script: String = {
            let st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let there = st.hosts.get(host_key).and_then(|h| h.repos_there.clone()).filter(|there| there.len() == st.repos.len());
            st.repos
                .iter()
                .enumerate()
                // (What can be done: not one that is there, nor one whose
                // folder holds something else. Not known: tried, which
                // is safe.)
                .filter(|(i, repo)| repo.wrong().is_none() && there.as_ref().is_none_or(|there| there[*i].to_do()))
                .map(|(_, repo)| format!("echo {}\nsh -c {} || exit $?\n", shell_quote(&format!("Cloning {}…", repo.name())), shell_quote(&repo.clone_script())))
                .collect()
        };
        if script.is_empty() {
            done(Ok(()));
            return None;
        }
        self.account_script(host_key, script, said, done)
    }

    /// A script run on a host as the account, for its Account window:
    /// off the connection's own path, each line it writes heard, ended
    /// when what comes back is aborted (`ready::cancellable`); the host
    /// asked again how the account stands before `done` hears how it
    /// went.
    fn account_script(&self, host_key: &str, script: String, said: impl Fn(&str) + Send + Sync + 'static, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Option<tokio::task::AbortHandle> {
        use olo_rex_client::ready;
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return None;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        let task = self.rt.spawn(async move {
            let command = format!("sh -c {}", shell_quote(&ready::cancellable(&script)));
            let mut hear = |line: &str| {
                let line = ready::plain_line(line);
                if !line.is_empty() {
                    said(&line);
                }
            };
            let result = match pool.exec(&target, askpass.clone(), &command, &mut hear).await {
                Ok(ran) if ran.ok() => Ok(()),
                Ok(ran) => Err(ran.output.lines().rev().map(ready::plain_line).find(|l| !l.is_empty()).unwrap_or_else(|| olo_kit::words::tr("it didn’t say why").into())),
                Err(e) => Err(e.to_string()),
            };
            inner.ask_the_account(&host_key, &pool, &target, askpass).await;
            (inner.notify)(HubNotice::StateChanged);
            done(result);
        });
        Some(task.abort_handle())
    }

    /// The person's repositories are these from now: kept (the
    /// desktop's settings), and every host's standing forgotten until
    /// it is asked again.
    pub fn set_repos(&self, repos: Vec<olo_rex_client::repos::Repo>) {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.repos.clone_from(&repos);
            for host in st.hosts.values_mut() {
                host.repos_there = None;
            }
        }
        let kept: Vec<olo_protocol::RexRepo> = repos.into_iter().map(|r| olo_protocol::RexRepo { url: r.url, folder: r.folder, setup: r.setup }).collect();
        olo_kit::settings::update(move |s| s.rex.repos = kept);
        (self.notify)(HubNotice::StateChanged);
    }

    /// How far back the agents' sessions copied to a host go, chosen:
    /// kept in the desktop's settings, and each host asked again.
    pub fn set_sessions_window(&self, window: olo_rex_client::sessions::Window) {
        let hosts: Vec<String> = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.sessions_window = window;
            for host in st.hosts.values_mut() {
                host.agent_sessions = None;
            }
            st.hosts.iter().filter(|(_, h)| h.conn_state == HostConnState::Connected).map(|(k, _)| k.clone()).collect()
        };
        let days = window.to_days();
        olo_kit::settings::update(move |s| s.rex.sessions_days = days);
        (self.notify)(HubNotice::StateChanged);
        for key in hosts {
            self.ask_sessions(&key);
        }
    }

    /// Asks a host how the agents' sessions stand there.
    pub fn ask_sessions(&self, host_key: &str) {
        let (endpoint, window) = {
            let st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            (st.hosts.get(host_key).and_then(|h| h.endpoint.clone()), st.sessions_window)
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else { return };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            let summary = sessions_stands(&pool, &target, askpass, window).await.map(|(stands, _)| stands.summary());
            if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                host.agent_sessions = summary;
            }
            (inner.notify)(HubNotice::StateChanged);
        });
    }

    /// Copies to a host the agents' sessions used in the window chosen
    /// that are missing or older there (`olo_rex_client::sessions`): one
    /// archive, made as it is sent; `said` told how far it is now and
    /// then. Stopped by what comes back (the channel closed, the
    /// archive's making ended with it); then the host asked again.
    pub fn copy_sessions(&self, host_key: &str, said: impl Fn(&str) + Send + Sync + 'static, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Option<tokio::task::AbortHandle> {
        use olo_rex_client::sessions;
        let (endpoint, window) = {
            let st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            (st.hosts.get(host_key).and_then(|h| h.endpoint.clone()), st.sessions_window)
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return None;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        let said = Arc::new(said);
        let task = self.rt.spawn(async move {
            let result = async {
                let (stands, gathered) = sessions_stands(&pool, &target, askpass.clone(), window).await.ok_or_else(|| olo_kit::words::tr("The host didn’t say what it has.").to_string())?;
                if stands.to_copy.is_empty() {
                    return Ok(());
                }
                let parts: Vec<sessions::Part> = stands.to_copy.iter().map(|i| gathered.parts[*i].clone()).collect();
                let (total, here, there) = (stands.bytes, gathered.home.clone(), stands.home.clone());
                // Through the engine where the host's helper keeps files in
                // step: only what changed of each goes (a turn added to a
                // transcript: its new lines). Else one archive, as before.
                let helper = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).is_some_and(|h| matches!(h.helper, HostHelper::Used | HostHelper::Ready));
                if helper && crate::link::syncs() {
                    let codex = sessions::codex_ids(&parts);
                    match copy_by_engine(&pool, &target, askpass.clone(), parts.clone(), &here, &there, total, said.clone()).await {
                        Some(Ok(())) => {
                            if !codex.is_empty() {
                                said(&olo_kit::words::tr("Telling Codex there of them"));
                                tell_codex(&pool, &target, askpass.clone(), &codex).await;
                            }
                            return Ok(());
                        }
                        Some(Err(e)) => return Err(e),
                        None => tracing::info!("Rex: the host's helper didn't keep the sessions in step; copied as an archive"),
                    }
                }
                let (chunks, from) = sessions::Chunks::new();
                let tell = said.clone();
                let tell_after = said.clone();
                let making = tokio::task::spawn_blocking(move || {
                    let (mut sent, mut told, began) = (0u64, std::time::Instant::now(), std::time::Instant::now());
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    sessions::stream(
                        &parts,
                        &here,
                        &there,
                        chunks,
                        &mut |n| {
                            sent += n;
                            if told.elapsed() >= Duration::from_millis(200) {
                                told = std::time::Instant::now();
                                // (And how long is left, at the pace so far.)
                                let left = time_left(sent, total, began.elapsed());
                                tell(&match left {
                                    Some(left) => olo_kit::tr!("{}%  ·  {}  ·  {}", (sent * 100 / total.max(1)).min(100), olo_kit::files::format_progress(sent, total), left).to_string(),
                                    None => olo_kit::tr!("{}%  ·  {}", (sent * 100 / total.max(1)).min(100), olo_kit::files::format_progress(sent, total)).to_string(),
                                });
                            }
                        },
                        &stop,
                    )
                });
                let codex = sessions::codex_ids(&stands.to_copy.iter().map(|i| gathered.parts[*i].clone()).collect::<Vec<_>>());
                let ran = pool.put_stream(&target, askpass.clone(), &format!("sh -c {}", shell_quote(sessions::PUT)), from).await.map_err(|e| e.to_string())?;
                let made = making.await.map_err(|e| e.to_string())?;
                if !ran.ok() {
                    return Err(ran.output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn’t say why").trim().to_owned());
                }
                made.map_err(|e| e.to_string())?;
                // (Codex's database there told of each, so its resume
                // lists them.)
                if !codex.is_empty() {
                    tell_after(&olo_kit::words::tr("Telling Codex there of them"));
                    tell_codex(&pool, &target, askpass.clone(), &codex).await;
                }
                Ok(())
            }
            .await;
            inner.ask_the_account(&host_key, &pool, &target, askpass).await;
            (inner.notify)(HubNotice::StateChanged);
            done(result);
        });
        Some(task.abort_handle())
    }

    /// Remembered carried-settings choices stored for `host_key`.
    pub fn carry_choices_for(&self, host_key: &str) -> Vec<olo_protocol::RexCarryRemembered> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.carry_choices.iter().filter(|r| r.host == host_key).cloned().collect()
    }

    /// Persist per-item remembered choice edits for `host_key` (`Some(choice)` sets/updates, `None` removes).
    pub fn update_carry_choices(&self, host_key: &str, edits: Vec<(String, Option<olo_protocol::RexCarryChoice>)>) {
        if edits.is_empty() {
            return;
        }
        let next = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for (item, choice) in edits {
                st.carry_choices.retain(|r| !(r.host == host_key && r.item == item));
                if let Some(c) = choice {
                    st.carry_choices.push(olo_protocol::RexCarryRemembered { host: host_key.to_owned(), item, choice: c });
                }
            }
            st.carry_choices.clone()
        };
        olo_kit::settings::update(move |s| s.rex.carry_choices = next);
        (self.notify)(HubNotice::StateChanged);
    }

    /// Forget specific remembered items (`Some(ids)`) or all remembered items (`None`) for `host_key`.
    pub fn forget_carry_choices(&self, host_key: &str, items: Option<&[String]>) {
        let next = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match items {
                Some(ids) => st.carry_choices.retain(|r| !(r.host == host_key && ids.contains(&r.item))),
                None => st.carry_choices.retain(|r| r.host != host_key),
            }
            if let Some(host) = st.hosts.get_mut(host_key) {
                host.carried = None;
            }
            st.carry_choices.clone()
        };
        olo_kit::settings::update(move |s| s.rex.carry_choices = next);
        (self.notify)(HubNotice::StateChanged);
    }

    /// Inspect item-level differences between this computer's carried agent settings
    /// and `host_key`, respecting remembered choices.
    pub fn inspect_carry_plan(&self, host_key: &str, done: impl FnOnce(Result<olo_rex_client::carry::Plan, String>) + Send + 'static) {
        let (endpoint, remembered) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let endpoint = st.hosts.get(host_key).and_then(|h| h.endpoint.clone());
            let remembered: HashMap<String, olo_protocol::RexCarryChoice> = st.carry_choices.iter().filter(|r| r.host == host_key).map(|r| (r.item.clone(), r.choice)).collect();
            (endpoint, remembered)
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            let res = match carried_plan(&pool, &target, askpass.clone(), &remembered).await {
                Some(plan) => {
                    let summary = plan.summary();
                    if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                        host.carried = Some(summary);
                    }
                    (inner.notify)(HubNotice::StateChanged);
                    Ok(plan)
                }
                None => Err(olo_kit::words::tr("The host didn’t say what it has.").into()),
            };
            done(res);
        });
    }

    /// Write an explicit set of resolved carried files to `host_key` in one tar archive,
    /// then refresh the host's account summary.
    pub fn apply_carry_files(&self, host_key: &str, files: Vec<olo_rex_client::carry::File>, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        use olo_rex_client::carry;
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            let result = if files.is_empty() {
                Ok(())
            } else {
                match pool.put(&target, askpass.clone(), &format!("sh -c {}", shell_quote(carry::PUT)), &carry::archive(&files)).await {
                    Ok(ran) if ran.ok() => Ok(()),
                    Ok(ran) => Err(ran.output.lines().rev().find(|l| !l.trim().is_empty() && !l.starts_with("put ")).unwrap_or("it didn’t say why").trim().to_owned()),
                    Err(e) => Err(e.to_string()),
                }
            };
            inner.ask_the_account(&host_key, &pool, &target, askpass).await;
            (inner.notify)(HubNotice::StateChanged);
            done(result);
        });
    }

    /// Puts on a host what it lacks of the person's settings for their
    /// agents (`olo_rex_client::carry`): asked what it has, given the
    /// files that are missing or differ in one archive, respecting
    /// remembered per-item choices; what was there kept beside each;
    /// then asked again how the account stands.
    pub fn carry_settings(&self, host_key: &str, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        use olo_rex_client::carry;
        let (endpoint, remembered) = {
            let st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let endpoint = st.hosts.get(host_key).and_then(|h| h.endpoint.clone());
            let remembered: HashMap<String, olo_protocol::RexCarryChoice> = st.carry_choices.iter().filter(|r| r.host == host_key).map(|r| (r.item.clone(), r.choice)).collect();
            (endpoint, remembered)
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            let result = match carried_stands(&pool, &target, askpass.clone(), &remembered).await {
                None => Err(olo_kit::words::tr("The host didn’t say what it has.").into()),
                Some(stands) if stands.to_copy.is_empty() => Ok(()),
                Some(stands) => match pool.put(&target, askpass.clone(), &format!("sh -c {}", shell_quote(carry::PUT)), &carry::archive(&stands.to_copy)).await {
                    Ok(ran) if ran.ok() => Ok(()),
                    Ok(ran) => Err(ran.output.lines().rev().find(|l| !l.trim().is_empty() && !l.starts_with("put ")).unwrap_or("it didn’t say why").trim().to_owned()),
                    Err(e) => Err(e.to_string()),
                },
            };
            inner.ask_the_account(&host_key, &pool, &target, askpass).await;
            (inner.notify)(HubNotice::StateChanged);
            done(result);
        });
    }

    pub fn install_remote_skill_in_pane(&self, key: &BlockKey) {
        let script = bridge::generate_remote_skill_install_script();
        let cmd = format!("sh -c {}\n", shell_quote(&script));
        self.send_input(key, cmd.into_bytes());
        self.look_again_at_what_the_host_has(&key.host_key);
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
    /// What is known of the helper on a host is kept (`Settings::
    /// helper_hosts`): `Some(true)` it has one, `Some(false)` this
    /// build of the app has none to give it, `None` nothing is known
    /// (it is asked before its next connection).
    fn helper_known(&self, host_key: &str, known: Option<bool>) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let word = known.map(|has| if has { "yes".to_owned() } else { format!("no:{}", crate::link::ID) });
        if st.settings.helper_hosts.get(host_key) != word.as_ref() {
            match word {
                Some(word) => st.settings.helper_hosts.insert(host_key.to_owned(), word),
                None => st.settings.helper_hosts.remove(host_key),
            };
            st.settings.save();
        }
    }

    /// The helper was taken off a host here (the Host window), and
    /// isn't to be put back by itself (`off`); or it is asked for
    /// there again.
    fn helper_kept_off(&self, host_key: &str, off: bool) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let is = st.settings.helper_hosts.get(host_key).is_some_and(|word| word == "off");
        if off != is {
            if off {
                st.settings.helper_hosts.insert(host_key.to_owned(), "off".to_owned());
            } else {
                st.settings.helper_hosts.remove(host_key);
            }
            st.settings.save();
        }
    }

    /// How the helper came to stand on a host, taken note of: the
    /// host's tip, what is kept of it, and where it was put there now,
    /// the status bar's word (something of Olo's now lies on the
    /// person's host: said, once, as it happens). `in_use`: the
    /// connection about to be made goes through it.
    fn helper_stands(&self, host_key: &str, stands: Result<olo_rex_client::helper::Stands, olo_rex_client::provision::Failed>, in_use: bool) {
        use olo_rex_client::helper::Stands;
        let (helper, current, sync_err) = match stands {
            Ok(Stands::Already) => {
                tracing::info!("rex: Olo's helper is on host {host_key} already");
                self.helper_known(host_key, Some(true));
                (HostHelper::Ready, Some(true), None)
            }
            Ok(Stands::Installed { bytes, took }) => {
                tracing::info!("rex: Olo's helper put on host {host_key}: {bytes} bytes in {} ms", took.as_millis());
                self.helper_known(host_key, Some(true));
                let label = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).map(|h| h.label.clone()).unwrap_or_default();
                let said = if in_use {
                    olo_kit::tr!("Olo’s typing helper was added to your account on “{}”.", label)
                } else {
                    olo_kit::tr!("Olo’s typing helper was added to your account on “{}” and will be used the next time you connect.", label)
                };
                (self.notify)(HubNotice::Said(said.to_string()));
                (HostHelper::Ready, Some(true), None)
            }
            Ok(Stands::NoBuild(kind)) => {
                tracing::info!("rex: no helper is carried for host {host_key} ({kind})");
                self.helper_known(host_key, Some(false));
                (HostHelper::Without(olo_kit::tr!("there isn’t one for {}", kind).to_string()), None, None)
            }
            Err(e) => {
                tracing::warn!("rex: Olo's helper couldn't be put on host {host_key}: {e}");
                let why = e.to_string();
                (HostHelper::Without(why.clone()), Some(false), Some(why))
            }
        };
        if let Some(host) = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(host_key) {
            if host.helper == HostHelper::Used {
                host.helper_current = current;
                host.helper_sync_err = sync_err;
            } else {
                host.helper = helper;
                host.helper_current = if current == Some(true) { Some(true) } else { None };
                host.helper_sync_err = None;
            }
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// A host of which nothing is known gets the helper **before** its
    /// connection is made, so its first connection already goes
    /// through it: asked (which makes the SSH connection, with
    /// whatever that asks of the person), and where it has none, given
    /// it. The sending is waited for [`HELPER_FIRST`] and no longer: on
    /// a link too slow for that the connection is made without, and
    /// the helper is there for the next. Whether the app's build is on
    /// the host now (nothing more is to be asked after connecting).
    ///
    /// Only where nothing is known: a host that has a helper (any
    /// build: `ssh::dial_command` runs what is there) or can't have
    /// this app's is connected to at once, as before.
    async fn helper_before_connecting(self: &Arc<Self>, host_key: &str, endpoint: &RexEndpoint) -> bool {
        use olo_rex_client::helper::{Asked, Stands, ask, put};
        let RexEndpoint::Ssh { target, pool, askpass } = endpoint else { return false };
        {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // (Or it was taken off this host here: not put back by itself.)
            let known = st.settings.helper_hosts.get(host_key).is_some_and(|word| word == "yes" || word == "off" || word.strip_prefix("no:") == Some(crate::link::ID));
            if !crate::link::carried() || !st.settings.host_helper || known {
                return false;
            }
        }
        let build = match ask(pool, target, askpass.clone(), crate::link::ID, &crate::link::BUILDS).await {
            Ok(Ok(Asked::Here)) => {
                self.helper_stands(host_key, Ok(Stands::Already), true);
                return true;
            }
            Ok(Ok(Asked::Needs(build) | Asked::Outdated(build))) => build,
            Ok(Err(kind)) => {
                self.helper_stands(host_key, Ok(Stands::NoBuild(kind)), true);
                return true;
            }
            // (Why it can't be reached is the connection's to say.)
            Err(_) => return false,
        };
        let (pool, target, askpass) = (pool.clone(), target.clone(), askpass.clone());
        let mut sending = tokio::spawn(async move { put(&pool, &target, askpass, crate::link::ID, build).await });
        match tokio::time::timeout(HELPER_FIRST, &mut sending).await {
            Ok(done) => {
                let done = done.unwrap_or_else(|e| Err(e.to_string().into()));
                let there = done.is_ok();
                self.helper_stands(host_key, done, true);
                there
            }
            Err(_) => {
                let (hub, host_key) = (self.clone(), host_key.to_owned());
                tokio::spawn(async move {
                    let done = sending.await.unwrap_or_else(|e| Err(e.to_string().into()));
                    hub.helper_stands(&host_key, done, false);
                });
                // (It is on its way: not to be asked for again meanwhile.)
                true
            }
        }
    }

    /// How the far hosts' Token Pools stand is asked of this
    /// computer's Token Pool (off the runtime's own threads: a few
    /// lines of HTTP that block), and kept; the windows are told where
    /// it differs.
    async fn ask_the_pools(&self) -> Option<HashMap<String, crate::pool::PoolStands>> {
        let pools = tokio::task::spawn_blocking(crate::pool::hosts).await.ok().and_then(Result::ok);
        let changed = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let changed = st.pools != pools;
            st.pools.clone_from(&pools);
            changed
        };
        if changed {
            (self.notify)(HubNotice::StateChanged);
        }
        pools
    }

    /// Asked again, a second apart, while something is being done to
    /// the pool of `host_key` (two minutes at most): how it stands
    /// once it is done.
    async fn pools_settled(&self, host_key: &str) -> Option<crate::pool::PoolStands> {
        let mut stands = None;
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            stands = self.ask_the_pools().await.map(|pools| pools.get(host_key).cloned().unwrap_or_default());
            if stands.as_ref().is_none_or(|p| p.doing.is_none()) {
                break;
            }
        }
        stands
    }

    /// The SSH agent on a host (HACKING, Rex, "The SSH agent on a
    /// host"): where the person has given a host reached over SSH the
    /// use of their SSH keys (Preferences ▸ Rex), Passwords' agent is
    /// forwarded to it over the SSH connection Rex keeps: a program
    /// there that asks the agent (`git`, `ssh`) is answered by
    /// Passwords here, which asks the person about that host as it
    /// asks about a terminal session of this computer. Rex only
    /// carries the messages (`olo_keychain::agent`), saying whose they
    /// are. Without the permission the host is given nothing, and what
    /// its SSH server opens is refused. Done after each connection (the
    /// SSH server's socket is that connection's) and when the
    /// permission changes.
    fn give_the_agent(self: &Arc<Self>, host_key: &str, endpoint: &RexEndpoint) {
        let RexEndpoint::Ssh { target, pool, askpass } = endpoint else { return };
        let (allowed, label) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let is_local = st.hosts.get(host_key).is_some_and(|h| h.is_local);
            (st.settings.policy_for_host(host_key, is_local).allow_ssh_agent, st.hosts.get(host_key).map(|h| h.label.clone()).unwrap_or_default())
        };
        // (What it is offered is said anew for each connection.)
        if let Some(host) = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(host_key) {
            host.agent_keys = None;
        }
        if !allowed {
            pool.set_agent(target, None);
            if self.agent_socks.lock().unwrap_or_else(|e| e.into_inner()).remove(host_key).is_some() {
                (self.notify)(HubNotice::StateChanged);
            }
            return;
        }
        // (Whose request it is, as Passwords says it to the person: the
        // host's name here and how it is reached.)
        let address = format!(
            "{}{}{}",
            target.user.as_ref().map(|user| format!("{user}@")).unwrap_or_default(),
            target.host,
            target.port.filter(|port| *port != 22).map(|port| format!(":{port}")).unwrap_or_default()
        );
        let origin = olo_keychain::SshOrigin { host: label, address };
        // (What Passwords answers the host's asking for the keys is kept
        // on the host and said: how many it was offered, or why none.)
        let (state, notify, key) = (Arc::downgrade(&self.state), self.notify.clone(), host_key.to_owned());
        let offered = move |answer: Result<Vec<String>, String>| {
            let Some(state) = state.upgrade() else { return };
            let changed = {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                let Some(host) = st.hosts.get_mut(&key) else { return };
                let changed = host.agent_keys.as_ref() != Some(&answer);
                host.agent_keys = Some(answer.clone());
                changed
            };
            if changed {
                match &answer {
                    Ok(names) => tracing::info!("Rex: {key} was offered {} SSH keys", names.len()),
                    Err(why) => tracing::warn!("Rex: {key} was offered no SSH keys: {why}"),
                }
                notify(HubNotice::StateChanged);
            }
        };
        pool.set_agent(
            target,
            Some(Arc::new(move |message: &[u8]| {
                olo_keychain::agent::answer(message, Some(&origin), &|request| {
                    let listing = matches!(request, olo_keychain::Request::SshKeysFor { .. });
                    let answer = olo_keychain::call(&request);
                    if listing {
                        offered(keys_offered(&answer));
                    }
                    answer.ok()
                })
            })),
        );
        let (target, pool, askpass) = (target.clone(), pool.clone(), askpass.clone());
        let (hub, host_key) = (self.clone(), host_key.to_owned());
        tokio::spawn(async move {
            match pool.forward_agent(&target, askpass).await {
                Ok(path) => {
                    hub.agent_socks.lock().unwrap_or_else(|e| e.into_inner()).insert(host_key, path);
                }
                Err(e) => {
                    tracing::info!("Rex: the SSH agent couldn't be given to {host_key}: {e}");
                    hub.agent_socks.lock().unwrap_or_else(|e| e.into_inner()).remove(&host_key);
                }
            }
            (hub.notify)(HubNotice::StateChanged);
        });
    }

    /// A host reached over SSH is asked whether Olo's bridge and the
    /// skill for agents are on it (one command, off the connection's
    /// path), for the status bar's marks.
    fn look_at_what_the_host_has(self: &Arc<Self>, host_key: &str, endpoint: &RexEndpoint) {
        let RexEndpoint::Ssh { target, pool, askpass } = endpoint else { return };
        let (target, pool, askpass) = (target.clone(), pool.clone(), askpass.clone());
        let (hub, host_key) = (self.clone(), host_key.to_owned());
        tokio::spawn(async move { hub.ask_what_the_host_has(&host_key, &pool, &target, askpass, true).await });
    }

    /// The host asked how its account stands, and its answer kept
    /// (`RexHub::ask_account`).
    async fn ask_the_account(&self, host_key: &str, pool: &olo_rex_client::SshSessionPool, target: &olo_rex_client::SshTarget, askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>) {
        use olo_rex_client::ready;
        let Ok(ran) = pool.exec(target, askpass.clone(), &format!("sh -c {}", shell_quote(ready::LOOK)), &mut |_| {}).await else { return };
        if !ran.ok() {
            return;
        }
        let account = ready::Account::read(&ran.output);
        // (And what of the person's settings for their agents it has,
        // and of their sessions.)
        let remembered: HashMap<String, olo_protocol::RexCarryChoice> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.carry_choices.iter().filter(|r| r.host == host_key).map(|r| (r.item.clone(), r.choice)).collect()
        };
        let carried = carried_stands(pool, target, askpass.clone(), &remembered).await.map(|stands| stands.summary);
        let window = self.state.lock().unwrap_or_else(|e| e.into_inner()).sessions_window;
        let sessions = sessions_stands(pool, target, askpass.clone(), window).await.map(|(stands, _)| stands.summary());
        // (And how each of the person's repositories stands there.)
        let repos = self.state.lock().unwrap_or_else(|e| e.into_inner()).repos.clone();
        let repos_there = match repos.is_empty() {
            true => Some(Vec::new()),
            false => pool.put(target, askpass, "sh", olo_rex_client::repos::look(&repos).as_bytes()).await.ok().and_then(|ran| olo_rex_client::repos::There::read(&ran.output, &repos)),
        };
        if let Some(host) = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(host_key) {
            host.account = Some(account);
            host.carried = carried;
            host.agent_sessions = sessions;
            host.repos_there = repos_there;
        }
    }

    /// [`Self::look_at_what_the_host_has`], waited for: the host is
    /// asked, and what it answered kept. When `auto_sync` is true
    /// (on connect), installed bridge skills and desktop commands from
    /// an older Olo are updated automatically over SSH, and any failure
    /// is recorded so Manage Host shows "Update available" with a
    /// manual Update button instead of hiding the failure.
    async fn ask_what_the_host_has(
        &self,
        host_key: &str,
        pool: &olo_rex_client::SshSessionPool,
        target: &olo_rex_client::SshTarget,
        askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>,
        auto_sync: bool,
    ) {
        let Ok(ran) = pool.exec(target, askpass.clone(), bridge::PRESENT, &mut |_| {}).await else { return };
        if !ran.ok() {
            return;
        }
        let err_of = |output: &str| output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn’t say why").trim().to_owned();
        let (old_shims, skill_there) = bridge::present(&ran.output);
        // (The shims of before did nothing, and an agent that finds
        // them takes them for the way: off they come.)
        if old_shims {
            let _ = pool.exec(target, askpass.clone(), &format!("sh -c {}", shell_quote(bridge::OLD_SHIMS)), &mut |_| {}).await;
        }
        // (A skill there from another Olo than this one: this one's is
        // put in its place on connect. An agent goes by what the skill there says.)
        let mut skill_cur = skill_there.then(|| bridge::skill_is_current(&ran.output));
        let mut skill_err: Option<String> = None;
        if auto_sync && skill_cur == Some(false) {
            match pool.exec(target, askpass.clone(), &format!("sh -c {}", shell_quote(&bridge::generate_remote_skill_install_script())), &mut |_| {}).await {
                Ok(r) if r.ok() => skill_cur = Some(true),
                Ok(r) => skill_err = Some(err_of(&r.output)),
                Err(e) => skill_err = Some(e.to_string()),
            }
        }
        // (The desktop commands kept in step as the skill is: another
        // Olo's are given this one's on connect.)
        let (commands_there, commands_current) = bridge::commands_there(&ran.output);
        let mut commands_cur = commands_there.then_some(commands_current);
        let mut commands_err: Option<String> = None;
        if auto_sync && commands_cur == Some(false) {
            match pool.exec(target, askpass, &format!("sh -c {}", shell_quote(&bridge::install_commands_script())), &mut |_| {}).await {
                Ok(r) if r.ok() => commands_cur = Some(true),
                Ok(r) => commands_err = Some(err_of(&r.output)),
                Err(e) => commands_err = Some(e.to_string()),
            }
        }
        let bridge_there = true;
        let changed = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get_mut(host_key) else { return };
            // On a read-only check (`!auto_sync`), preserve a prior auto-sync error
            // if the component is still out of date; clear it once up to date or removed.
            if !auto_sync {
                if skill_cur == Some(false) {
                    skill_err = host.skill_sync_err.clone();
                }
                if commands_cur == Some(false) {
                    commands_err = host.commands_sync_err.clone();
                }
            }
            let next = (Some(bridge_there), Some(skill_there), skill_cur, skill_err, Some(commands_there), commands_cur, commands_err);
            let prev = (host.bridge_there, host.skill_there, host.skill_current, host.skill_sync_err.clone(), host.commands_there, host.commands_current, host.commands_sync_err.clone());
            let changed = prev != next;
            (host.bridge_there, host.skill_there, host.skill_current, host.skill_sync_err, host.commands_there, host.commands_current, host.commands_sync_err) = next;
            changed
        };
        if changed {
            (self.notify)(HubNotice::StateChanged);
        }
    }

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
        {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let cant = st.settings.helper_hosts.get(host_key).is_some_and(|word| word == "off" || word.strip_prefix("no:") == Some(crate::link::ID));
            if !crate::link::carried() || !st.settings.host_helper || cant {
                return;
            }
        }
        let (target, pool, askpass) = (target.clone(), pool.clone(), askpass.clone());
        let (hub, host_key) = (self.clone(), host_key.to_owned());
        if let Some(host) = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key).filter(|h| h.helper != HostHelper::Used) {
            host.helper = HostHelper::Installing;
        }
        tokio::spawn(async move {
            let stands = olo_rex_client::helper::ensure(&pool, &target, askpass, crate::link::ID, &crate::link::BUILDS).await;
            hub.helper_stands(&host_key, stands, false);
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
                predict_wake(&tokio::runtime::Handle::current(), &self.state, &block, &mut b, &key, &self.notify, at);
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
            predict_wake(&tokio::runtime::Handle::current(), &self.state, &block, &mut b, &key, &self.notify, at);
        }
        drop(b);
        if changed {
            (self.notify)(HubNotice::BlockOutput(key));
        }
        if chrome {
            (self.notify)(HubNotice::StateChanged);
        }
    }

    /// Keep It Going's and Auto Approve's look at a pane whose screen or
    /// program changed, where the person has either on: what its agent
    /// is doing, the question it asks, and the timer set for the next
    /// thing to do. Nothing for a pane with neither on.
    pub fn agent_look(self: &Arc<Self>, bk: &BlockKey) {
        let help = help_of(bk);
        if !help.any() {
            return;
        }
        let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(bk).cloned() else { return };
        let changed = {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            let screen = g.agent_screen();
            let (name, exe, args) = g.agent_program();
            let agent = which_agent(name.as_deref(), &exe, &args, &screen);
            if agent != g.watch.agent {
                tracing::info!(pane = bk.block_id, ?name, exe, ?args, agent = agent.map(|a| a.name()), "agent in the pane");
            }
            g.watch.looked(agent, &screen, &help, auto_approve_after(), Instant::now())
        };
        self.agent_schedule(bk, &br, &help);
        if changed {
            (self.notify)(HubNotice::StateChanged);
        }
    }

    /// The timer for a pane's next thing to do (a question's answer, the
    /// end of Keep It Going's quiet), and while a question counts down,
    /// a redraw each second (nothing otherwise).
    fn agent_schedule(self: &Arc<Self>, bk: &BlockKey, br: &Arc<Mutex<BlockRuntime>>, help: &Help) {
        let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = g.watch_timer.take() {
            t.abort();
        }
        if let Some(at) = g.watch.next(help, keep_going_quiet(), Instant::now(), epoch_secs()) {
            let (inner, key) = (self.clone(), bk.clone());
            let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
            g.watch_timer = Some(
                rt.spawn(async move {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
                    inner.agent_due(&key).await;
                })
                .abort_handle(),
            );
        }
        if g.watch.counting(Instant::now()).is_some() && g.watch_ticker.is_none() {
            let (notify, block) = (self.notify.clone(), br.clone());
            let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
            g.watch_ticker = Some(
                rt.spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        let mut g = block.lock().unwrap_or_else(|e| e.into_inner());
                        if g.watch.counting(Instant::now()).is_none() {
                            g.watch_ticker = None;
                            break;
                        }
                        drop(g);
                        notify(HubNotice::StateChanged);
                    }
                })
                .abort_handle(),
            );
        }
    }

    /// A pane's timer went: what is due, read from its screen as it is
    /// now, sent as the person would type it (Keep It Going's message,
    /// then its Return a moment after: all at once, a program may take
    /// it as pasted text, the Return a new line in it).
    async fn agent_due(self: &Arc<Self>, bk: &BlockKey) {
        let help = help_of(bk);
        let (br, message, ctrl) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (st.blocks.get(bk).cloned(), st.settings.keep_going_message.clone(), st.hosts.get(&bk.host_key).and_then(|h| h.control.clone()))
        };
        let Some(br) = br else { return };
        let todo = {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            g.watch_timer = None;
            let screen = g.agent_screen();
            let question = g.watch.pending.as_ref().map(|p| p.question.clone());
            g.watch.due(&help, &screen, keep_going_quiet(), &message, Instant::now(), epoch_secs()).map(|todo| (todo, question))
        };
        if let (Some((todo, question)), Some(ctrl)) = (todo, ctrl) {
            self.input_by_itself(bk);
            match todo {
                Do::Return => {
                    tracing::info!(pane = bk.block_id, question = question.unwrap_or_default(), "auto approve: answered Yes");
                    let _ = ctrl.terminal_write(&bk.session_id, &bk.block_id, "\r").await;
                }
                Do::Say(message) => {
                    tracing::info!(pane = bk.block_id, "keep it going: said \"{message}\"");
                    let _ = ctrl.terminal_write(&bk.session_id, &bk.block_id, &message).await;
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    let _ = ctrl.terminal_write(&bk.session_id, &bk.block_id, "\r").await;
                }
            }
        }
        (self.notify)(HubNotice::StateChanged);
        self.agent_schedule(bk, &br, &help);
    }

    /// The person typed in a pane: a question waiting there is theirs
    /// (kept as theirs where Auto Approve is on), and Keep It Going's
    /// quiet begins again.
    fn agent_typed(self: &Arc<Self>, bk: &BlockKey) {
        let help = help_of(bk);
        if !help.any() {
            return;
        }
        let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(bk).cloned() else { return };
        let stopped = br.lock().unwrap_or_else(|e| e.into_inner()).watch.typed(Instant::now());
        if let Some(key) = stopped.filter(|_| help.auto_approve) {
            if let Some(h) = HELP.lock().unwrap_or_else(|e| e.into_inner()).get_mut(bk) {
                h.theirs = Some(key);
            }
            (self.notify)(HubNotice::StateChanged);
        }
        self.agent_schedule(bk, &br, &help_of(bk));
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
    /// What a host's `olo forward` asks, done: a forward made at the port
    /// it names (its target's own unless it says), its host's forwards
    /// listed, those made from it closed. Answered as the command's output.
    async fn bridge_forward(self: &Arc<Self>, host_key: &str, is_local: bool, session_id: Option<String>, args: &[String]) -> Value {
        use crate::forwards::{Ask, LOWEST, Listed, Made, USAGE, address_here, listed};
        let fail = |why: String| json!({ "ok": false, "stderr": format!("olo: {why}\n") });
        let ask = match crate::forwards::ask(args) {
            Ok(ask) => ask,
            Err(why) => return json!({ "ok": false, "stderr": format!("olo: {why}\n{USAGE}") }),
        };
        match ask {
            Ask::Help => json!({ "ok": true, "stdout": USAGE }),
            Ask::List { json: as_json } => {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let mut rows: Vec<Listed> = st
                    .forwards
                    .iter()
                    .filter(|f| f.host_key == host_key)
                    .map(|f| Listed {
                        port: f.local_addr.port(),
                        to: if f.remote_host.contains(':') { format!("[{}]:{}", f.remote_host, f.remote_port) } else { format!("{}:{}", f.remote_host, f.remote_port) },
                        state: match &f.state {
                            ForwardState::Active => "forwarded".into(),
                            ForwardState::Waiting => "waiting".into(),
                            ForwardState::Failed(why) => format!("failed: {why}"),
                        },
                        made: f.made,
                        kept: f.kept,
                    })
                    .collect();
                for kf in st.kept_forwards.iter().filter(|kf| kf.host == host_key) {
                    if st.forwards.iter().any(|af| kept_matches_active(kf, af)) {
                        continue;
                    }
                    let state = match st.kept_forward_errors.get(&kept_forward_key(kf)) {
                        Some(why) => format!("failed: {why}"),
                        None => "waiting".into(),
                    };
                    rows.push(Listed {
                        port: kf.local_port,
                        to: if kf.remote_host.contains(':') { format!("[{}]:{}", kf.remote_host, kf.remote_port) } else { format!("{}:{}", kf.remote_host, kf.remote_port) },
                        state,
                        made: maker_to_made(kf.made),
                        kept: true,
                    });
                }
                json!({ "ok": true, "stdout": listed(&rows, as_json) })
            }
            Ask::Close { ports, all } => {
                let (mut closed, mut refused, mut forgotten) = (Vec::new(), Vec::new(), Vec::new());
                {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    for p in &ports {
                        let active = st.forwards.iter().find(|f| f.host_key == host_key && f.local_addr.port() == *p);
                        let kept_only = st.kept_forwards.iter().find(|kf| kf.host == host_key && kf.local_port == *p);
                        match (active, kept_only) {
                            (Some(f), _) if !f.made.hosts() => refused.push(format!("the forward on port {p} is the person's: closed in Rex, not from here")),
                            (None, Some(kf)) if !maker_to_made(kf.made).hosts() => refused.push(format!("the forward on port {p} is the person's: closed in Rex, not from here")),
                            (None, None) => refused.push(format!("no forward of this host's listens on port {p} of the Olo desktop (olo forward ls lists them)")),
                            _ => {}
                        }
                    }
                    let mut closed_specs: Vec<(String, u16, u16)> = Vec::new();
                    st.forwards.retain(|f| {
                        let close = f.host_key == host_key && f.made.hosts() && (all || ports.contains(&f.local_addr.port()));
                        if close {
                            closed.push(f.local_addr.port());
                            if f.kept {
                                closed_specs.push((f.remote_host.clone(), f.remote_port, f.local_addr.port()));
                            }
                        }
                        !close
                    });
                    st.kept_forwards.retain(|kf| {
                        if kf.host != host_key || !maker_to_made(kf.made).hosts() {
                            return true;
                        }
                        let matched_active = closed_specs.iter().any(|(rh, rp, lp)| kf.remote_host == *rh && kf.remote_port == *rp && (kf.local_port == 0 || kf.local_port == *lp));
                        let matched_waiting = all || (kf.local_port != 0 && ports.contains(&kf.local_port));
                        if matched_active || matched_waiting {
                            if matched_waiting && !closed.contains(&kf.local_port) && kf.local_port != 0 {
                                closed.push(kf.local_port);
                            }
                            forgotten.push(kept_forward_key(kf));
                            return false;
                        }
                        true
                    });
                    for k in &forgotten {
                        st.kept_forward_errors.remove(k);
                    }
                }
                if !forgotten.is_empty() {
                    olo_kit::settings::update(move |s| {
                        s.rex.kept_forwards.retain(|kf| !forgotten.contains(&kept_forward_key(kf)));
                    });
                }
                if !closed.is_empty() {
                    (self.notify)(HubNotice::StateChanged);
                }
                let stdout: String = closed.iter().map(|p| format!("closed {p}\n")).collect();
                let stderr: String = refused.iter().map(|why| format!("olo: {why}\n")).collect();
                json!({ "ok": refused.is_empty(), "stdout": stdout, "stderr": stderr })
            }
            Ask::Make { host, port, here, url, keep } => {
                // (This computer's own server: its ports are here already.)
                if is_local {
                    return json!({ "ok": true, "stdout": format!("{}\n", address_here(url.as_deref(), port)) });
                }
                let want = here.unwrap_or(if port >= LOWEST { port } else { 0 });
                let (ctrl, ep, sid) = {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let existing_port = st
                        .forwards
                        .iter_mut()
                        .find(|f| f.host_key == host_key && f.remote_host == host && f.remote_port == port && f.state == ForwardState::Active && (want == 0 || f.local_addr.port() == want))
                        .map(|f| {
                            let promoted = keep && !f.kept;
                            if promoted {
                                f.kept = true;
                            }
                            (f.local_addr.port(), f.made, promoted)
                        });
                    if let Some((bound, made, promoted)) = existing_port {
                        drop(st);
                        if promoted {
                            let saved_port = if here == Some(0) { 0 } else { bound };
                            self.remember_kept_forward(olo_protocol::RexKeptForward {
                                host: host_key.to_owned(),
                                remote_host: host,
                                remote_port: port,
                                local_port: saved_port,
                                made: made_to_maker(made),
                            });
                            (self.notify)(HubNotice::StateChanged);
                        }
                        return json!({ "ok": true, "stdout": format!("{}\n", address_here(url.as_deref(), bound)) });
                    }
                    if want != 0
                        && let Some(f) = st.forwards.iter().find(|f| f.local_addr.port() == want)
                    {
                        return fail(if f.host_key == host_key {
                            format!("port {want} of the Olo desktop is this host's forward to {}:{} (olo forward close {want} closes it); or --port N, --any-port", f.remote_host, f.remote_port)
                        } else {
                            format!("port {want} of the Olo desktop is a forward to another host: --port N, or --any-port")
                        });
                    }
                    let Some(h) = st.hosts.get(host_key) else { return fail("this host isn't Rex's on the Olo desktop".into()) };
                    let (Some(ctrl), Some(ep)) = (h.control.clone(), h.endpoint.clone()) else { return fail("this host isn't connected to the Olo desktop".into()) };
                    let Some(sid) = session_id.filter(|s| h.session_summaries.iter().any(|x| &x.session_id == s)).or_else(|| h.session_summaries.first().map(|s| s.session_id.clone())) else {
                        return fail("this host has no Rex session to forward through".into());
                    };
                    (ctrl, ep, sid)
                };
                if want != 0 && std::net::TcpListener::bind(("127.0.0.1", want)).is_err() {
                    // (Rex's own, made with `rex forward new` and outliving
                    // what made it, was "another program": an agent's
                    // report, 2026-11-24. Said by name and how to close it.)
                    return fail(match self.forward_at(want, host_key).await {
                        Some(which) => format!("port {want} of the Olo desktop is taken by {which}; or --port N, --any-port"),
                        None => format!("port {want} of the Olo desktop is taken by another program there: --port N, or --any-port"),
                    });
                }
                match self.create_forward(host_key, &ctrl, ep, &sid, host.clone(), port, want, Made::Agent, keep).await {
                    Ok(at) => {
                        if keep {
                            let saved_port = if here == Some(0) { 0 } else { at.port() };
                            self.remember_kept_forward(olo_protocol::RexKeptForward {
                                host: host_key.to_owned(),
                                remote_host: host,
                                remote_port: port,
                                local_port: saved_port,
                                made: olo_protocol::RexForwardMaker::Agent,
                            });
                        }
                        json!({ "ok": true, "stdout": format!("{}\n", address_here(url.as_deref(), at.port())) })
                    }
                    Err(why) => fail(format!("the forward couldn't be made: {why}")),
                }
            }
        }
    }

    /// The forwards' blocks this Rex made on `host_key`'s server and lost
    /// (it ended without closing them: a crash; an upgrade in place takes
    /// its own up again, `resuming`), closed as the host connects, and
    /// those the server has no more forgotten (`forwards::lost_and_gone`).
    /// An earlier Rex's forward stayed on a host's server, its port there
    /// in `rex forward ls` with no one behind it (an agent's report,
    /// 2026-11-24). Another desktop's Rex's are left: it comes back for
    /// its own. A `session.list_blocks` a session with such a block, as a
    /// host connects.
    async fn close_lost_forwards(&self, host_key: &str, ctrl: &ControlClient, resuming: &[String]) {
        let made = crate::forwards::made_blocks();
        if !made.iter().any(|m| m.host_key == host_key) {
            return;
        }
        let (here, sessions, hosts): (Vec<String>, Vec<String>, Vec<String>) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                st.forwards.iter().filter(|f| f.host_key == host_key).map(|f| f.handle.block_id.clone()).collect(),
                st.hosts.get(host_key).map(|h| h.session_summaries.iter().map(|s| s.session_id.clone()).collect()).unwrap_or_default(),
                st.hosts.keys().cloned().collect(),
            )
        };
        let mut asked: HashMap<String, Option<HashMap<String, Option<String>>>> = HashMap::new();
        for m in made.iter().filter(|m| m.host_key == host_key) {
            if asked.contains_key(&m.session_id) {
                continue;
            }
            // (A session the host has no more holds none of its blocks.)
            let blocks = if !sessions.contains(&m.session_id) {
                Some(HashMap::new())
            } else {
                ctrl.session_list_blocks(&m.session_id).await.ok().map(|r| r.payload.blocks.into_iter().filter_map(|b| Some((b.block_id?, b.owner))).collect())
            };
            asked.insert(m.session_id.clone(), blocks);
        }
        let (lost, gone) = crate::forwards::lost_and_gone(&made, host_key, ctrl.client_id(), &here, resuming, &|s| asked.get(s).cloned().flatten());
        let mut done: Vec<String> = gone.iter().map(|m| m.block_id.clone()).collect();
        for m in lost {
            if ctrl.block_close(&m.session_id, &m.block_id).await.is_ok() {
                tracing::info!(host = host_key, block = m.block_id.as_str(), "closed a forward this Rex had lost");
                done.push(m.block_id.clone());
            }
        }
        // (And those of hosts this Rex has no more: their forwards closed
        // as the host went.)
        if !done.is_empty() || made.iter().any(|m| !hosts.contains(&m.host_key)) {
            crate::forwards::change_made(|made| made.retain(|m| !done.contains(&m.block_id) && hosts.contains(&m.host_key)));
        }
    }

    /// The Rex forward whose client listens at `port` (127.0.0.1 or every
    /// address) of this computer, said for a host's `olo forward` that found
    /// the port taken (`asker`, the host asking): one made with `rex
    /// forward new`, or by another Rex, in a session of a server this Rex
    /// is connected to, by its short id (as `rex forward ls` shows it: its
    /// block's id's last six), what it reaches and how to close it. None
    /// where none says so. Asked only when a port is taken: a
    /// `session.list_blocks` a session and a `tunnel.info` a forward, two
    /// seconds at most in all.
    async fn forward_at(&self, port: u16, asker: &str) -> Option<String> {
        let servers: Vec<(String, String, bool, ControlClient, Vec<(String, String)>)> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts
                .iter()
                // (A session by its name where it has one, else its id: as
                // `rex forward close -s` takes it.)
                .filter_map(|(key, h)| {
                    let sessions = h.session_summaries.iter().map(|s| (s.session_id.clone(), if s.label.is_empty() { s.session_id.clone() } else { s.label.clone() })).collect();
                    Some((key.clone(), h.label.clone(), h.is_local, h.control.clone()?, sessions))
                })
                .collect()
        };
        let look = async {
            for (key, label, local, ctrl, sessions) in &servers {
                for (sid, session) in sessions {
                    let Ok(listed) = ctrl.session_list_blocks(sid).await else { continue };
                    for b in listed.payload.blocks.iter().filter(|b| b.flavor.starts_with("com.superlogical.tunnel.dial")) {
                        let Some(bid) = &b.block_id else { continue };
                        let Ok(info) = ctrl.tunnel_info(sid, bid).await else { continue };
                        if listens_at(info.payload.client_address.as_deref()) != Some(port) {
                            continue;
                        }
                        return Some(forward_said(bid, info.payload.target.as_deref(), session, label, *local, key == asker));
                    }
                }
            }
            None
        };
        tokio::time::timeout(Duration::from_secs(2), look).await.ok().flatten()
    }

    /// What a host's `olo sync` asks, done: `olo sync PATH [--mirror]` keeps
    /// that folder in step here (Downloads ▸ Rex ▸ HOST ▸ its name: synced
    /// now, then whenever it changes there, `keep`), `ls` lists this host's
    /// kept folders, `wait PATH` syncs one now and answers when it is in
    /// step (after a build: `make && olo sync wait dist`), `stop PATH`
    /// lets one go (what it brought stays).
    async fn bridge_sync(self: &Arc<Self>, host_key: &str, args: Vec<String>) -> bridge::BridgeResponse {
        use olo_protocol::RexSyncDirection as Dir;
        const USAGE: &str = "\
usage: olo sync PATH [--from-host | --to-host | --two-way] [--mirror] [--shallow] [--exclude PATTERN]… [--json]
       olo sync ls [--json]
       olo sync conflicts [PATH] [--json]
       olo sync wait PATH [--json]
       olo sync stop PATH";
        let fail = |why: String| bridge::BridgeResponse::err(format!("olo sync: {why}\n{USAGE}"));
        let label = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).map(|h| h.label.clone()).unwrap_or_default();
        let shown =
            |there: &str| kept_here(&label, there).map(|p| format!("Downloads/Rex/{}/{}", label, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).unwrap_or_default();
        let ask = match parse_sync_ask(&args) {
            Ok(ask) => ask,
            Err(why) => return fail(why),
        };
        let dir_arrow = |d: Dir| match d {
            Dir::FromHost => "→",
            Dir::ToHost => "←",
            Dir::Both => "↔",
        };
        let dir_word = |d: Dir| match d {
            Dir::FromHost => "from-host",
            Dir::ToHost => "to-host",
            Dir::Both => "both",
        };
        match ask {
            SyncAsk::List { json } => {
                let rows: Vec<Kept> = self.state.lock().unwrap_or_else(|e| e.into_inner()).kept.iter().filter(|k| k.host_key == host_key).cloned().collect();
                if json {
                    let arr: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|k| {
                            json!({
                                "there": k.there,
                                "here": shown(&k.there),
                                "direction": dir_word(k.direction),
                                "mirror": k.mirror,
                                "shallow": k.shallow,
                                "exclude": k.exclude,
                                "state": k.state.said(),
                                "files": k.last.map(|(_, f, _)| f),
                                "bytes": k.last.map(|(_, _, b)| b),
                                "conflicts": k.conflicts,
                            })
                        })
                        .collect();
                    let out = serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".into());
                    return bridge::BridgeResponse { ok: true, stdout: Some(format!("{out}\n")), ..Default::default() };
                }
                let lines: String = rows
                    .iter()
                    .map(|k| {
                        let mut tags: Vec<String> = Vec::new();
                        if k.direction == Dir::ToHost {
                            tags.push("to host".into());
                        } else if k.direction == Dir::Both {
                            tags.push("both ways".into());
                        }
                        if k.mirror && k.direction != Dir::Both {
                            tags.push("mirrored".into());
                        }
                        if k.shallow {
                            tags.push("shallow".into());
                        }
                        if !k.exclude.is_empty() {
                            tags.push(format!("excluding {}", k.exclude.join(", ")));
                        }
                        let tag_str = if tags.is_empty() { String::new() } else { format!(" ({})", tags.join(", ")) };
                        let conf_str = if k.conflicts.is_empty() { String::new() } else { format!(", {} conflict{}", k.conflicts.len(), if k.conflicts.len() == 1 { "" } else { "s" }) };
                        let last = k.last.map(|(at, files, sent)| {
                            let ago = at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
                            format!(" ({files} file{}; last {ago} s ago, {} sent)", if files == 1 { "" } else { "s" }, olo_kit::files::format_size(sent))
                        });
                        format!("{} {} {}{}: {}{}{}\n", k.there, dir_arrow(k.direction), shown(&k.there), tag_str, k.state.said(), conf_str, last.unwrap_or_default())
                    })
                    .collect();
                bridge::BridgeResponse { ok: true, stdout: Some(if lines.is_empty() { "No folder of this host's is kept in step on the Olo desktop.\n".into() } else { lines }), ..Default::default() }
            }
            SyncAsk::Conflicts { path, json } => {
                let targets: Vec<Kept> = {
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.kept.iter().filter(|k| k.host_key == host_key && path.as_deref().is_none_or(|p| k.there == p)).cloned().collect()
                };
                if let Some(p) = &path
                    && targets.is_empty()
                {
                    return fail(format!("{p} isn't kept in step"));
                }
                let mut found: Vec<(String, String, String)> = Vec::new();
                for k in targets {
                    let Some(here) = kept_here(&label, &k.there) else { continue };
                    let filter = k.spec().folder().options().filter;
                    let here_scan = here.clone();
                    let list = tokio::task::spawn_blocking(move || olo_rex_sync::twoway::list_conflicts(&here_scan, &filter)).await.ok().and_then(Result::ok).unwrap_or_default();
                    let shown_here = shown(&k.there);
                    // Keep cached conflict list on Kept in sync with disk.
                    self.set_kept_conflicts(host_key, &k.there, list.clone());
                    for rel in list {
                        found.push((k.there.clone(), shown_here.clone(), rel));
                    }
                }
                if json {
                    let arr: Vec<serde_json::Value> = found.iter().map(|(t, h, rel)| json!({ "there": t, "here": h, "path": rel })).collect();
                    let out = serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".into());
                    return bridge::BridgeResponse { ok: true, stdout: Some(format!("{out}\n")), ..Default::default() };
                }
                if found.is_empty() {
                    return bridge::BridgeResponse { ok: true, stdout: Some("No conflict copies.\n".into()), ..Default::default() };
                }
                let lines: String = found.iter().map(|(t, h, rel)| format!("{t}/{rel}  ({h}/{rel})\n")).collect();
                bridge::BridgeResponse { ok: true, stdout: Some(lines), ..Default::default() }
            }
            SyncAsk::Stop { path: there } => {
                let had = {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let before = st.kept.len();
                    st.kept.retain(|k| !(k.host_key == host_key && k.there == there));
                    before != st.kept.len()
                };
                if !had {
                    return fail(format!("{there} isn't kept in step"));
                }
                self.stop_keeping(host_key, &there);
                if let Some(here) = kept_here(&label, &there) {
                    let (_, baseline) = kept_cache_paths(&here, &there);
                    let _ = std::fs::remove_file(baseline);
                }
                let (hk, gone) = (host_key.to_owned(), there.clone());
                olo_kit::settings::update(move |s| s.rex.kept.retain(|k| !(k.host == hk && k.there == gone)));
                (self.notify)(HubNotice::StateChanged);
                bridge::BridgeResponse { ok: true, stdout: Some(format!("{there} is no longer kept in step (files stay on both sides; desktop copy in {}).\n", shown(&there))), ..Default::default() }
            }
            SyncAsk::Wait { path: there, json } => {
                let Some(spec) = self.state.lock().unwrap_or_else(|e| e.into_inner()).kept.iter().find(|k| k.host_key == host_key && k.there == there).map(Kept::spec) else {
                    return fail(format!("{there} isn't kept in step: `olo sync {there}` keeps it"));
                };
                self.sync_now(host_key, &spec, &shown(&there), false, json).await
            }
            SyncAsk::Keep { path: there, direction, mirror, shallow, exclude, json } => {
                if !there.starts_with('/') {
                    return fail(format!("{there}: a whole path"));
                }
                if kept_here(&label, &there).is_none() {
                    return fail(format!("{there}: a folder"));
                }
                let spec = KeptSpec { there: there.clone(), mirror, direction, shallow, exclude: exclude.clone() };
                let (new, restart) = {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    match st.kept.iter_mut().find(|k| k.host_key == host_key && k.there == there) {
                        Some(k) => {
                            let changed = k.mirror != mirror || k.direction != direction || k.shallow != shallow || k.exclude != exclude;
                            k.mirror = mirror;
                            k.direction = direction;
                            k.shallow = shallow;
                            k.exclude = exclude.clone();
                            (false, changed)
                        }
                        None => {
                            st.kept.push(Kept {
                                host_key: host_key.to_owned(),
                                there: there.clone(),
                                mirror,
                                direction,
                                shallow,
                                exclude: exclude.clone(),
                                conflicts: Vec::new(),
                                state: KeptState::Waiting,
                                last: None,
                            });
                            (true, false)
                        }
                    }
                };
                if restart {
                    self.stop_keeping(host_key, &there);
                }
                let (hk, kept, excl) = (host_key.to_owned(), there.clone(), exclude);
                olo_kit::settings::update(move |s| {
                    if let Some(k) = s.rex.kept.iter_mut().find(|k| k.host == hk && k.there == kept) {
                        k.mirror = mirror;
                        k.direction = direction;
                        k.shallow = shallow;
                        k.exclude = excl;
                    } else {
                        s.rex.kept.push(olo_protocol::RexKept { host: hk.clone(), there: kept.clone(), mirror, direction, shallow, exclude: excl });
                    }
                });
                // Run initial sync first (creating the remote dir if needed for
                // --to-host / --two-way), then start the live watcher(s) so the
                // host watch finds the folder in place.
                let create_remote = direction != Dir::FromHost;
                let mut answer = self.sync_now(host_key, &spec, &shown(&there), create_remote, json).await;
                self.keep_folders_of(host_key);
                if answer.ok && new && !json {
                    let how = match direction {
                        Dir::FromHost => "what changes there is brought here as it changes",
                        Dir::ToHost => "what changes on the desktop is pushed here as it changes",
                        Dir::Both => "changes flow both ways as they happen (conflicts kept side by side)",
                    };
                    answer.stdout = answer.stdout.map(|s| format!("{s}Kept in step from now on: {how} (`olo sync stop {there}` lets it go).\n"));
                }
                answer
            }
        }
    }

    /// A kept folder synced now, under its lock, and said.
    async fn sync_now(self: &Arc<Self>, host_key: &str, spec: &KeptSpec, shown: &str, create_remote: bool, json: bool) -> bridge::BridgeResponse {
        use olo_protocol::RexSyncDirection as Dir;
        let lock = {
            let mut keeping = self.keeping.lock().unwrap_or_else(|e| e.into_inner());
            keeping.entry((host_key.to_owned(), spec.there.clone())).or_insert_with(|| (None, Arc::new(tokio::sync::Mutex::new(())))).1.clone()
        };
        let arrow = match spec.direction {
            Dir::FromHost => "→",
            Dir::ToHost => "←",
            Dir::Both => "↔",
        };
        match self.sync_kept(host_key, spec, create_remote, true, &lock).await {
            Ok(out) => {
                self.set_kept_done(host_key, &spec.there, out.files, out.bytes, out.all_conflicts.clone(), true);
                if json {
                    let body = json!({
                        "there": spec.there,
                        "here": shown,
                        "files": out.files,
                        "bytes": out.bytes,
                        "new_conflicts": out.new_conflicts.iter().map(|c| json!({ "path": c.path, "copy": c.copy })).collect::<Vec<_>>(),
                        "conflicts": out.all_conflicts,
                    });
                    return bridge::BridgeResponse { ok: true, stdout: Some(format!("{}\n", serde_json::to_string_pretty(&body).unwrap_or_default())), ..Default::default() };
                }
                let mut text = format!(
                    "{} {arrow} {shown} on the Olo desktop: in step, {} file{}, {} sent\n",
                    spec.there,
                    out.files,
                    if out.files == 1 { "" } else { "s" },
                    olo_kit::files::format_size(out.bytes)
                );
                for c in &out.new_conflicts {
                    text.push_str(&format!("  conflict copy kept: {} (for {})\n", c.copy, c.path));
                }
                bridge::BridgeResponse { ok: true, stdout: Some(text), ..Default::default() }
            }
            Err(why) => {
                self.set_kept(host_key, &spec.there, KeptState::Failed(why.clone()), None);
                bridge::BridgeResponse::err(format!("olo sync: {}: {why}", spec.there))
            }
        }
    }

    /// What a host's `olo send PATH… [--mirror]` asks, done: each file or
    /// folder there saved here, in Downloads ▸ Rex ▸ HOST (a file by its
    /// name, a folder as a folder of that name), by the engine through
    /// the host's helper, so a file sent again sends what changed and a
    /// folder sent again is brought in step (`--mirror`: what the host
    /// no longer has taken away from it here, within that folder alone).
    /// A host never names a path here: where it goes is this side's.
    async fn bridge_send(self: &Arc<Self>, host_key: &str, args: Vec<String>) -> bridge::BridgeResponse {
        use olo_rex_client::sync::{HostFolder, Stop, receive_from};
        let mirror = args.iter().any(|a| a == "--mirror");
        if let Some(other) = args.iter().find(|a| a.starts_with('-') && *a != "--mirror") {
            return bridge::BridgeResponse::err(format!("olo send: {other} isn't one of its options (--mirror)\nusage: olo send PATH… [--mirror]"));
        }
        let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if paths.is_empty() {
            return bridge::BridgeResponse::err("usage: olo send PATH… [--mirror]  (files and folders of this host, saved on the Olo desktop in Downloads ▸ Rex ▸ HOST)");
        }
        let (endpoint, label, helper) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get(host_key) else { return bridge::BridgeResponse::err("this host isn't Rex's on the Olo desktop") };
            (h.endpoint.clone(), h.label.clone(), matches!(h.helper, HostHelper::Used | HostHelper::Ready))
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            return bridge::BridgeResponse::err("olo send: this host isn't reached over SSH from the Olo desktop");
        };
        if !(helper && crate::link::syncs()) {
            return bridge::BridgeResponse::err("olo send: Olo's helper isn't on this host, built to send files (Rex's Typing helper on the desktop puts it here)");
        }
        let name: String = label.chars().map(|c| if c == '/' || c == '\0' { '-' } else { c }).collect();
        let Some(base) = host_here(&label) else {
            return bridge::BridgeResponse::err("olo send: the Olo desktop has no home to save files in");
        };
        // (What each path is there: a file, a folder, or nothing.)
        let ask: String = paths.iter().map(|p| format!("if [ -d {q} ]; then echo d; elif [ -f {q} ]; then echo f; else echo -; fi; ", q = olo_rex_client::provision::shell_word(p))).collect();
        let kinds = match pool.exec(&target, askpass.clone(), &format!("sh -c {}", shell_quote(&ask)), &mut |_| {}).await {
            Ok(ran) => ran.output.lines().map(str::trim).map(str::to_owned).collect::<Vec<_>>(),
            Err(e) => return bridge::BridgeResponse::err(format!("olo send: {e}")),
        };
        let (mut said, mut failed) = (String::new(), String::new());
        for (path, kind) in paths.iter().zip(kinds.iter().map(String::as_str).chain(std::iter::repeat("-"))) {
            let leaf = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_owned();
            let (folder, here, shown) = match kind {
                "f" => (HostFolder { path: (*path).clone(), scoped: true, ..Default::default() }, base.clone(), format!("Downloads/Rex/{name}/{leaf}")),
                "d" => (HostFolder { path: (*path).clone(), delete: mirror, ..Default::default() }, base.join(&leaf), format!("Downloads/Rex/{name}/{leaf}")),
                _ => {
                    failed.push_str(&format!("olo send: {path}: no such file or folder here\n"));
                    continue;
                }
            };
            if let Err(e) = std::fs::create_dir_all(&here) {
                failed.push_str(&format!("olo send: {shown}: {e}\n"));
                continue;
            }
            match receive_from(&pool, &target, askpass.clone(), crate::link::ID, &folder, here, None, Stop::default()).await {
                Ok(report) => {
                    for e in &report.errors {
                        failed.push_str(&format!("olo send: {leaf}/{}: {}\n", e.path, e.reason));
                    }
                    let files = report.new + report.appended + report.patched + report.copied + report.same;
                    said.push_str(&format!(
                        "{path} → {shown} on the Olo desktop: {files} file{}, {} sent\n",
                        if files == 1 { "" } else { "s" },
                        olo_kit::files::format_size(report.bytes_sent + report.bytes_received)
                    ));
                    (self.notify)(HubNotice::Attention {
                        target: Target::Host(host_key.to_owned()),
                        title: olo_kit::tr!("{} sent “{}”", label, leaf).to_string(),
                        body: olo_kit::tr!("In Downloads ▸ Rex ▸ {}.", name).to_string(),
                        kind: olo_kit::notify::NoteKind::Information,
                    });
                }
                Err(e) => failed.push_str(&format!("olo send: {path}: {e}\n")),
            }
        }
        bridge::BridgeResponse { ok: failed.is_empty(), stdout: Some(said), stderr: (!failed.is_empty()).then_some(failed), ..Default::default() }
    }

    /// The forward a host's page opened here goes through (`olo open
    /// http://localhost:…`, a link clicked in a pane): one of the host's
    /// to that address already, whoever made it (an agent's `olo forward
    /// 5173` first, so the page opens at the port its sign-in names), or
    /// a new one at a free port.
    async fn ensure_ephemeral_forward(&self, host_key: &str, session_id: &str, remote_host: &str, remote_port: u16) -> Result<u16, String> {
        let (ctrl, ep) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let to = |f: &&ActiveForward| f.host_key == host_key && f.remote_host == remote_host && f.remote_port == remote_port;
            if let Some(f) = st.forwards.iter().filter(to).find(|f| f.state == ForwardState::Active) {
                return Ok(f.local_addr.port());
            }
            if let Some(f) = st.forwards.iter().filter(to).find(|f| f.handle.session_id == session_id) {
                return match &f.state {
                    ForwardState::Active => Ok(f.local_addr.port()),
                    ForwardState::Waiting => Err("The forward is waiting for its host.".into()),
                    ForwardState::Failed(why) => Err(why.clone()),
                };
            }
            st.hosts.get(host_key).and_then(|h| Some((h.control.clone()?, h.endpoint.clone()?))).ok_or_else(|| format!("host '{host_key}' is not connected"))?
        };
        self.create_forward(host_key, &ctrl, ep, session_id, remote_host.to_owned(), remote_port, 0, crate::forwards::Made::Link, false).await.map(|a| a.port())
    }

    /// Persist a kept forward in `HubState` and desktop `RexSettings::kept_forwards`.
    fn remember_kept_forward(&self, entry: olo_protocol::RexKeptForward) {
        let key = kept_forward_key(&entry);
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.kept_forwards.iter().any(|k| kept_forward_key(k) == key) {
                st.kept_forwards.push(entry.clone());
            }
            st.kept_forward_errors.remove(&key);
        }
        olo_kit::settings::update(move |s| {
            if !s.rex.kept_forwards.iter().any(|k| kept_forward_key(k) == key) {
                s.rex.kept_forwards.push(entry);
            }
        });
    }

    /// Open any kept forwards for `host_key` that don't yet have a running or
    /// resuming forward (called on host connect, on `SessionCreated`, and when
    /// `RexSettings::kept_forwards` changes while connected).
    fn ensure_kept_forwards(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint) {
        let pending: Vec<(olo_protocol::RexKeptForward, String)> = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get(host_key) else { return };
            if !h.control.as_ref().is_some_and(|c| c.same_connection(ctrl)) {
                return;
            }
            let Some(sid) = h.session_summaries.first().map(|s| s.session_id.clone()) else {
                return;
            };
            let saved = SAVED_FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
            let mut out = Vec::new();
            for kf in st.kept_forwards.clone().into_iter().filter(|kf| kf.host == host_key) {
                let key = kept_forward_key(&kf);
                let has_active = st.forwards.iter().any(|af| kept_matches_active(&kf, af));
                let is_saved =
                    saved.iter().any(|sf| sf.host_key == kf.host && sf.remote_host == kf.remote_host && sf.remote_port == kf.remote_port && (kf.local_port == 0 || kf.local_port == sf.local_port));
                if has_active || is_saved || st.kept_forwards_starting.contains(&key) {
                    continue;
                }
                st.kept_forwards_starting.insert(key);
                out.push((kf, sid.clone()));
            }
            out
        };
        for (kf, sid) in pending {
            let (inner, ctrl, ep) = (self.clone(), ctrl.clone(), ep.clone());
            tokio::spawn(async move {
                let key = kept_forward_key(&kf);
                let made = maker_to_made(kf.made);
                let res = inner.create_forward(&kf.host, &ctrl, ep, &sid, kf.remote_host.clone(), kf.remote_port, kf.local_port, made, true).await;
                {
                    let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.kept_forwards_starting.remove(&key);
                    match res {
                        Ok(_) => {
                            st.kept_forward_errors.remove(&key);
                        }
                        Err(why) => {
                            st.kept_forward_errors.insert(key, why);
                        }
                    }
                }
                (inner.notify)(HubNotice::StateChanged);
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_forward(
        &self,
        host_key: &str,
        ctrl: &ControlClient,
        ep: RexEndpoint,
        session_id: &str,
        remote_host: String,
        remote_port: u16,
        local_port: u16,
        made: crate::forwards::Made,
        kept: bool,
    ) -> Result<SocketAddr, String> {
        let handle = start_local_forward(ctrl.clone(), ep, session_id.into(), remote_host.clone(), remote_port, local_port).await.map_err(|e| e.to_string())?;
        let local_addr = handle.local_addr;
        // (Written down until the server has it no more: one this Rex
        // loses, ending without closing it, is closed as the host next
        // connects, `close_lost_forwards`.)
        let block = crate::forwards::MadeBlock { host_key: host_key.into(), session_id: session_id.into(), block_id: handle.block_id.clone(), owner: ctrl.client_id().map(str::to_owned) };
        crate::forwards::change_made(|made| made.push(block));
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.hosts.get(host_key).and_then(|h| h.control.as_ref()).is_some_and(|c| c.same_connection(ctrl)) {
                return Err("The host disconnected while opening the forward.".into());
            }
            st.forwards.push(ActiveForward { id: uuid::Uuid::new_v4().to_string(), host_key: host_key.into(), remote_host, remote_port, local_addr, state: ForwardState::Active, made, kept, handle });
        }
        (self.notify)(HubNotice::StateChanged);
        Ok(local_addr)
    }

    /// The kept folders of `host_key` kept in step, each by a runner of
    /// its own (`keep`), started where one isn't running (as the host
    /// connects, and as the settings name a new one).
    fn keep_folders_of(self: &Arc<Self>, host_key: &str) {
        let mine: Vec<KeptSpec> = self.state.lock().unwrap_or_else(|e| e.into_inner()).kept.iter().filter(|k| k.host_key == host_key).map(Kept::spec).collect();
        let mut keeping = self.keeping.lock().unwrap_or_else(|e| e.into_inner());
        for spec in mine {
            let entry = keeping.entry((host_key.to_owned(), spec.there.clone())).or_insert_with(|| (None, Arc::new(tokio::sync::Mutex::new(()))));
            if entry.0.as_ref().is_some_and(|t| !t.is_finished()) {
                continue;
            }
            let (inner, hk, lock) = (self.clone(), host_key.to_owned(), entry.1.clone());
            entry.0 = Some(tokio::spawn(async move { inner.keep(hk, spec, lock).await }).abort_handle());
        }
    }

    /// A kept folder let go of: its runner stopped (what it brought here
    /// stays).
    fn stop_keeping(&self, host_key: &str, there: &str) {
        if let Some((Some(task), _)) = self.keeping.lock().unwrap_or_else(|e| e.into_inner()).remove(&(host_key.to_owned(), there.to_owned())) {
            task.abort();
        }
    }

    fn set_kept(&self, host_key: &str, there: &str, state: KeptState, last: Option<(std::time::SystemTime, u64, u64)>) {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(k) = st.kept.iter_mut().find(|k| k.host_key == host_key && k.there == there) else { return };
            if k.state == state && last.is_none() {
                return;
            }
            k.state = state;
            if last.is_some() {
                k.last = last;
            }
        }
        (self.notify)(HubNotice::StateChanged);
    }

    fn set_kept_conflicts(&self, host_key: &str, there: &str, conflicts: Vec<String>) {
        let changed = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(k) = st.kept.iter_mut().find(|k| k.host_key == host_key && k.there == there) else { return };
            if k.conflicts == conflicts {
                false
            } else {
                k.conflicts = conflicts;
                true
            }
        };
        if changed {
            (self.notify)(HubNotice::StateChanged);
        }
    }

    fn set_kept_done(&self, host_key: &str, there: &str, files: u64, bytes: u64, conflicts: Vec<String>, stamp_last: bool) {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(k) = st.kept.iter_mut().find(|k| k.host_key == host_key && k.there == there) else { return };
            let prev_same = k.state == KeptState::InStep && k.conflicts == conflicts && !stamp_last && k.last.is_some();
            if prev_same {
                return;
            }
            k.state = KeptState::InStep;
            k.conflicts = conflicts;
            if stamp_last || k.last.is_none() {
                k.last = Some((std::time::SystemTime::now(), files, bytes));
            }
        }
        (self.notify)(HubNotice::StateChanged);
    }

    /// One sync of a kept folder, under its lock: direction-aware (`FromHost`,
    /// `ToHost`, or `Both`), using cached indexes so idle/echo checks do zero
    /// file reads and zero unnecessary UI churn.
    async fn sync_kept(self: &Arc<Self>, host_key: &str, spec: &KeptSpec, create_remote: bool, show_syncing: bool, lock: &tokio::sync::Mutex<()>) -> Result<SyncOutcome, String> {
        use olo_protocol::RexSyncDirection as Dir;
        use olo_rex_client::sync::{Stop, receive_from, send_to, sync_both};
        let _held = lock.lock().await;
        let (endpoint, label) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let h = st.hosts.get(host_key).ok_or("the host isn't Rex's")?;
            (h.endpoint.clone(), h.label.clone())
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else { return Err("the host isn't reached over SSH".into()) };
        let here = kept_here(&label, &spec.there).ok_or("no folder here to keep it in")?;
        std::fs::create_dir_all(&here).map_err(|e| e.to_string())?;
        let (cache, baseline) = kept_cache_paths(&here, &spec.there);
        let folder = spec.folder();
        match spec.direction {
            Dir::FromHost => {
                self.set_kept(host_key, &spec.there, KeptState::Syncing, None);
                let report = receive_from(&pool, &target, askpass, crate::link::ID, &folder, here, Some(cache), Stop::default()).await.map_err(|e| e.to_string())?;
                if let Some(e) = report.errors.first() {
                    return Err(format!("{}: {}", e.path, e.reason));
                }
                let files = report.new + report.appended + report.patched + report.copied + report.same;
                let changed = report.new + report.appended + report.patched + report.copied + report.removed > 0;
                Ok(SyncOutcome { files, bytes: report.bytes_sent + report.bytes_received, changed, new_conflicts: Vec::new(), all_conflicts: Vec::new() })
            }
            Dir::ToHost => {
                self.set_kept(host_key, &spec.there, KeptState::Syncing, None);
                let source = Box::new(olo_rex_sync::wire::FolderSource { root: here, filter: folder.options().filter, cache_path: Some(cache) });
                let report = send_to(&pool, &target, askpass, crate::link::ID, source, &folder, Stop::default()).await.map_err(|e| e.to_string())?;
                if let Some(e) = report.errors.first() {
                    return Err(format!("{}: {}", e.path, e.reason));
                }
                let files = report.new + report.appended + report.patched + report.copied + report.same;
                let changed = report.new + report.appended + report.patched + report.copied + report.removed > 0;
                Ok(SyncOutcome { files, bytes: report.bytes_sent + report.bytes_received, changed, new_conflicts: Vec::new(), all_conflicts: Vec::new() })
            }
            Dir::Both => {
                if show_syncing {
                    self.set_kept(host_key, &spec.there, KeptState::Syncing, None);
                }
                let inner_cb = self.clone();
                let hk_cb = host_key.to_owned();
                let there_cb = spec.there.clone();
                let on_work: Option<Box<dyn FnOnce() + Send>> = if show_syncing { None } else { Some(Box::new(move || inner_cb.set_kept(&hk_cb, &there_cb, KeptState::Syncing, None))) };
                let report = sync_both(&pool, &target, askpass, crate::link::ID, &label, &folder, here, cache, baseline, create_remote, Stop::default(), on_work).await.map_err(|e| e.to_string())?;
                if let Some(e) = report.errors.first() {
                    return Err(format!("{}: {}", e.path, e.reason));
                }
                if !report.conflicts.is_empty() {
                    let count = report.conflicts.len();
                    let body = if count == 1 {
                        format!("Kept conflict copy {}.", report.conflicts[0].copy)
                    } else {
                        format!("Kept {count} conflict copies in {}.", spec.there.rsplit('/').next().unwrap_or(&spec.there))
                    };
                    (self.notify)(HubNotice::Attention {
                        target: Target::Host(host_key.to_owned()),
                        title: olo_kit::tr!("Sync conflict on {}", label).to_string(),
                        body,
                        kind: olo_kit::notify::NoteKind::Important,
                    });
                }
                Ok(SyncOutcome { files: report.files, bytes: report.wire_bytes(), changed: report.changed(), new_conflicts: report.conflicts, all_conflicts: report.all_conflicts })
            }
        }
    }

    /// Run an immediate sync of a kept folder (called by the Host window's Sync Now).
    pub async fn sync_kept_immediate(self: &Arc<Self>, host_key: &str, there: &str) {
        let Some(spec) = self.state.lock().unwrap_or_else(|e| e.into_inner()).kept.iter().find(|k| k.host_key == host_key && k.there == there).map(Kept::spec) else {
            return;
        };
        let lock = {
            let mut keeping = self.keeping.lock().unwrap_or_else(|e| e.into_inner());
            keeping.entry((host_key.to_owned(), there.to_owned())).or_insert_with(|| (None, Arc::new(tokio::sync::Mutex::new(())))).1.clone()
        };
        match self.sync_kept(host_key, &spec, spec.direction != olo_protocol::RexSyncDirection::FromHost, true, &lock).await {
            Ok(out) => self.set_kept_done(host_key, there, out.files, out.bytes, out.all_conflicts, true),
            Err(why) => self.set_kept(host_key, there, KeptState::Failed(why), None),
        }
    }

    /// A kept folder kept in step while its host is connected: watches the
    /// host (`FromHost`), this computer (`ToHost`), or both (`Both`) using
    /// zero-polling Linux inotify, syncing whenever either watched side changes.
    async fn keep(self: Arc<Self>, host_key: String, spec: KeptSpec, lock: Arc<tokio::sync::Mutex<()>>) {
        use olo_protocol::RexSyncDirection as Dir;
        use olo_rex_sync::watch::{DEFAULT_SETTLE_MS, WatchEvent, spawn_watch};
        use tokio::io::AsyncBufReadExt as _;

        let (endpoint, label, helper) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let h = st.hosts.get(&host_key);
            (h.and_then(|h| h.endpoint.clone()), h.map(|h| h.label.clone()).unwrap_or_default(), h.is_some_and(|h| matches!(h.helper, HostHelper::Used | HostHelper::Ready)))
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            self.set_kept(&host_key, &spec.there, KeptState::Waiting, None);
            return;
        };
        if !(helper && crate::link::syncs()) {
            self.set_kept(&host_key, &spec.there, KeptState::Waiting, None);
            return;
        }
        let folder = spec.folder();
        let watch_remote = matches!(spec.direction, Dir::FromHost | Dir::Both);
        let watch_local = matches!(spec.direction, Dir::ToHost | Dir::Both);

        // Local watcher (for ToHost and Both): held for the life of this task
        // so aborting the task closes the stop pipe and ends the watcher thread.
        let (_local_guard, mut local_rx) = if watch_local {
            let Some(here) = kept_here(&label, &spec.there) else {
                self.set_kept(&host_key, &spec.there, KeptState::Failed("no folder here to keep it in".into()), None);
                return;
            };
            if let Err(e) = std::fs::create_dir_all(&here) {
                self.set_kept(&host_key, &spec.there, KeptState::Failed(e.to_string()), None);
                return;
            }
            let (tx, rx) = tokio::sync::mpsc::channel::<WatchEvent>(8);
            match spawn_watch(here, folder.options().filter, DEFAULT_SETTLE_MS, move |ev| tx.blocking_send(ev).is_ok()) {
                Ok(handle) => (Some(handle), Some(rx)),
                Err(e) => {
                    self.set_kept(&host_key, &spec.there, KeptState::Failed(format!("watch: {e}")), None);
                    return;
                }
            }
        } else {
            (None, None)
        };

        // Remote watcher (for FromHost and Both).
        let mut remote_lines = if watch_remote {
            let watch_cmd = folder.watch_command(crate::link::ID);
            let Ok(stream) = pool.exec_io(&target, askpass.clone(), &watch_cmd).await else {
                self.set_kept(&host_key, &spec.there, KeptState::Waiting, None);
                return;
            };
            Some(tokio::io::BufReader::new(stream).lines())
        } else {
            None
        };

        // Wait for each active watcher to announce "watching" before the first
        // sync so no change occurring during the first sync is missed.
        if let Some(lines) = &mut remote_lines {
            match lines.next_line().await {
                Ok(Some(said)) if said == "watching" => {}
                Ok(Some(said)) if said == "gone" => {
                    self.set_kept(&host_key, &spec.there, KeptState::Gone, None);
                    return;
                }
                Ok(Some(said)) if said.starts_with("error: ") => {
                    self.set_kept(&host_key, &spec.there, KeptState::Failed(said.trim_start_matches("error: ").to_owned()), None);
                    return;
                }
                _ => {
                    self.set_kept(&host_key, &spec.there, KeptState::Waiting, None);
                    return;
                }
            }
        }
        if let Some(rx) = &mut local_rx {
            match rx.recv().await {
                Some(WatchEvent::Watching) => {}
                Some(WatchEvent::Gone) => {
                    self.set_kept(&host_key, &spec.there, KeptState::Failed("The folder on this computer was removed.".into()), None);
                    return;
                }
                Some(WatchEvent::Error(why)) => {
                    self.set_kept(&host_key, &spec.there, KeptState::Failed(why), None);
                    return;
                }
                _ => return,
            }
        }

        // Initial sync after watches are live.
        match self.sync_kept(&host_key, &spec, false, false, &lock).await {
            Ok(out) => self.set_kept_done(&host_key, &spec.there, out.files, out.bytes, out.all_conflicts, out.changed),
            Err(why) => self.set_kept(&host_key, &spec.there, KeptState::Failed(why), None),
        }

        // Main event loop: sleep with zero CPU until either watcher reports a change.
        loop {
            let next_remote = async {
                match &mut remote_lines {
                    Some(lines) => lines.next_line().await,
                    None => std::future::pending().await,
                }
            };
            let next_local = async {
                match &mut local_rx {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            let trigger = tokio::select! {
                res = next_remote => match res {
                    Ok(Some(said)) if said == "changed" => true,
                    Ok(Some(said)) if said == "gone" => {
                        self.set_kept(&host_key, &spec.there, KeptState::Gone, None);
                        return;
                    }
                    Ok(Some(said)) if said.starts_with("error: ") => {
                        self.set_kept(&host_key, &spec.there, KeptState::Failed(said.trim_start_matches("error: ").to_owned()), None);
                        return;
                    }
                    Ok(Some(_)) => false,
                    _ => {
                        self.set_kept(&host_key, &spec.there, KeptState::Waiting, None);
                        return;
                    }
                },
                ev = next_local => match ev {
                    Some(WatchEvent::Changed) => true,
                    Some(WatchEvent::Gone) => {
                        self.set_kept(&host_key, &spec.there, KeptState::Failed("The folder on this computer was removed.".into()), None);
                        return;
                    }
                    Some(WatchEvent::Error(why)) => {
                        self.set_kept(&host_key, &spec.there, KeptState::Failed(why), None);
                        return;
                    }
                    Some(WatchEvent::Watching) => false,
                    None => return,
                },
            };
            if !trigger {
                continue;
            }
            match self.sync_kept(&host_key, &spec, false, false, &lock).await {
                Ok(out) => self.set_kept_done(&host_key, &spec.there, out.files, out.bytes, out.all_conflicts, out.changed),
                Err(why) => self.set_kept(&host_key, &spec.there, KeptState::Failed(why), None),
            }
        }
    }

    /// A kept folder that failed kept in step again from the start (the
    /// Host window's Try Again): its runner ended, and a new one started,
    /// which watches the folder and syncs it at once.
    pub fn retry_kept(self: &Arc<Self>, host_key: &str, there: &str) {
        self.stop_keeping(host_key, there);
        self.keep_folders_of(host_key);
    }

    /// A kept folder let go of from here (the Host window's Stop): as `olo
    /// sync stop` does, files staying on both sides.
    pub fn stop_kept(self: &Arc<Self>, host_key: &str, there: &str) {
        let label = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.kept.retain(|k| !(k.host_key == host_key && k.there == there));
            st.hosts.get(host_key).map(|h| h.label.clone()).unwrap_or_default()
        };
        self.stop_keeping(host_key, there);
        if let Some(here) = kept_here(&label, there) {
            let (_, baseline) = kept_cache_paths(&here, there);
            let _ = std::fs::remove_file(baseline);
        }
        let (hk, gone) = (host_key.to_owned(), there.to_owned());
        olo_kit::settings::update(move |s| s.rex.kept.retain(|k| !(k.host == hk && k.there == gone)));
        (self.notify)(HubNotice::StateChanged);
    }

    /// The host's forwards of before an upgrade in place, taken up again
    /// at their ports here: their own blocks, owned again; a new block
    /// where one is gone (in the same session, or the host's first), and
    /// the person told where a port here is taken now.
    fn resume_saved_forwards(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint) {
        let mine: Vec<SavedForward> = {
            let mut saved = SAVED_FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
            let (mine, rest) = std::mem::take(&mut *saved).into_iter().partition(|f| f.host_key == host_key);
            *saved = rest;
            mine
        };
        // (Those this Rex made and lost, all but these: closed.)
        let resuming: Vec<String> = mine.iter().map(|f| f.block_id.clone()).collect();
        let (inner, lost_ctrl, host) = (self.clone(), ctrl.clone(), host_key.to_owned());
        tokio::spawn(async move { inner.close_lost_forwards(&host, &lost_ctrl, &resuming).await });
        for f in mine {
            let (inner, ctrl, ep) = (self.clone(), ctrl.clone(), ep.clone());
            tokio::spawn(async move {
                let resumed = olo_rex_client::resume_local_forward(ctrl.clone(), ep.clone(), f.session_id.clone(), f.block_id.clone(), f.remote_host.clone(), f.remote_port, f.local_port).await;
                let made = match resumed {
                    Ok(handle) => {
                        let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                        if !st.hosts.get(&f.host_key).and_then(|h| h.control.as_ref()).is_some_and(|c| c.same_connection(&ctrl)) {
                            // (Gone again meanwhile: kept for its next connection.)
                            drop(st);
                            handle.leave();
                            SAVED_FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).push(f);
                            return;
                        }
                        let local_addr = handle.local_addr;
                        st.forwards.push(ActiveForward {
                            id: uuid::Uuid::new_v4().to_string(),
                            host_key: f.host_key.clone(),
                            remote_host: f.remote_host.clone(),
                            remote_port: f.remote_port,
                            local_addr,
                            state: ForwardState::Active,
                            made: f.made,
                            kept: f.kept,
                            handle,
                        });
                        Ok(())
                    }
                    Err(olo_rex_client::RexError::Io(e)) => {
                        // (Its port here is taken now: its block closed,
                        // not left orphaned on the server.)
                        let _ = ctrl.block_close(&f.session_id, &f.block_id).await;
                        Err(e.to_string())
                    }
                    Err(_) => {
                        // (Its block is gone: a new one, in its session if
                        // that is still there.)
                        let sid = {
                            let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                            let h = st.hosts.get(&f.host_key);
                            h.and_then(|h| h.session_summaries.iter().find(|s| s.session_id == f.session_id).or_else(|| h.session_summaries.first()).map(|s| s.session_id.clone()))
                        };
                        match sid {
                            Some(sid) => inner.create_forward(&f.host_key, &ctrl, ep, &sid, f.remote_host.clone(), f.remote_port, f.local_port, f.made, f.kept).await.map(|_| ()),
                            None => Err("the host has no session to forward through".into()),
                        }
                    }
                };
                if let Err(why) = made {
                    tracing::warn!(port = f.local_port, "a forward of before the upgrade couldn't be made again: {why}");
                    let label = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&f.host_key).map(|h| h.label.clone()).unwrap_or_default();
                    (inner.notify)(HubNotice::Attention {
                        target: Target::Host(f.host_key.clone()),
                        title: olo_kit::tr!("Port {} isn’t forwarded from {} any more", f.local_port, label),
                        body: why,
                        kind: olo_kit::notify::NoteKind::Important,
                    });
                }
                (inner.notify)(HubNotice::StateChanged);
            });
        }
    }

    fn restore_forwards(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for f in st.forwards.iter().filter(|f| f.host_key == host_key && f.state != ForwardState::Active) {
            let reconnect = f.handle.reconnect(ctrl.clone(), ep.clone());
            let (id, sid, hk, ctrl, ep, inner, kept) = (f.id.clone(), f.handle.session_id.clone(), host_key.to_owned(), ctrl.clone(), ep.clone(), self.clone(), f.kept);
            tokio::spawn(async move {
                let result = reconnect.await;
                let mut reattach_kept = false;
                {
                    let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(host) = st.hosts.get(&hk).filter(|h| h.control.as_ref().is_some_and(|c| c.same_connection(&ctrl))) else { return };
                    let session_ended = !host.session_summaries.iter().any(|s| s.session_id == sid);
                    if kept && session_ended && result.is_err() {
                        // A kept forward outlives the session that made it: drop
                        // the stale handle and let `ensure_kept_forwards` reopen it
                        // on the host's first live session.
                        st.forwards.retain(|f| f.id != id);
                        reattach_kept = true;
                    } else {
                        let Some(f) = st.forwards.iter_mut().find(|f| f.id == id) else { return };
                        f.state = match result {
                            Ok(()) => ForwardState::Active,
                            Err(olo_rex_client::RexError::Api(_)) if session_ended => ForwardState::Failed(olo_kit::words::tr("its session has ended.").into()),
                            Err(olo_rex_client::RexError::Api(e)) => ForwardState::Failed(e.message),
                            Err(e) => ForwardState::Failed(e.to_string()),
                        };
                    }
                }
                if reattach_kept {
                    inner.ensure_kept_forwards(&hk, &ctrl, &ep);
                }
                (inner.notify)(HubNotice::StateChanged);
            });
        }
    }

    async fn run_host_connection(self: Arc<Self>, host_key: String, retry: Arc<tokio::sync::Notify>) {
        let mut attempt = 0u32;
        let mut last_seq = 0;
        loop {
            let (endpoint_raw, reconnect_token, user_overrides) = {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let overrides = st.settings.keymap_overrides.clone();
                let Some(h) = st.hosts.get_mut(&host_key) else { return };
                if !h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry)) {
                    return;
                }
                if attempt == 0 {
                    h.conn_state = HostConnState::Connecting;
                }
                (h.endpoint_raw.clone(), h.reconnect_token.clone(), overrides)
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

            // (A host nothing is known of: its helper first, so this
            // connection goes through it.)
            let helper_seen = self.helper_before_connecting(&host_key, &endpoint).await;

            // (Connecting in as few round trips of the link as it takes,
            // HACKING, Rex, "Connecting in fewer round trips": nine
            // passed before a terminal's first screen came, one after
            // another, and over SSH more.)
            let terminals_known = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.keys().filter(|bk| bk.host_key == host_key).count();
            let connected = async {
                // The host is reached first (over SSH: signed in to):
                // from here its streams are opened side by side. (Not
                // before: a host that can't be reached, or whose key
                // the person doesn't trust, would be tried, and asked
                // about, once for each.)
                endpoint.reach().await?;
                // Its terminals' streams, as far as their upgrade,
                // while the control connection is made and the
                // sessions asked for.
                let spares: Vec<Spare> = (0..terminals_known.clamp(1, MOST_SPARES))
                    .map(|_| {
                        let ep = endpoint.clone();
                        tokio::spawn(async move { open_terminal_stream(&ep).await })
                    })
                    .collect();
                let label = std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
                // The server's status and version, the hosts it knows,
                // the keys, and the sessions: none needs another's
                // answer, and they were five round trips, each waited
                // for before the next was asked. (Of the keys: the
                // commands Rex has, and no keys of its own but what its
                // old recorder left: what comes back is the keys of the
                // person's Rex configuration, and those.)
                let km_actions = keymap::register_actions_payload();
                let ticket_req =
                    [DataTicketRequest { creator_name: "com.superlogical.terminal".to_owned(), data_ref_id: "terminal".to_owned(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.to_owned()) }];
                // In one flight with the upgrade and the hello, where
                // nothing between here and the server reads HTTP: one
                // round trip for all of it. A server that doesn't take
                // it so is connected to the plain way.
                let mut flown = None;
                if endpoint.takes_a_flight() {
                    let asked = vec![
                        ("server.status", None),
                        ("server.version", None),
                        ("hosts.list", None),
                        ("client.keymap", Some(ControlClient::keymap_request(&km_actions, &[], &user_overrides)?)),
                        ("session.bootstrap", Some(ControlClient::bootstrap_request("all", None, None, &ticket_req)?)),
                    ];
                    let (stream, host_hdr) = endpoint.connect_control_stream().await?;
                    match ControlClient::connect_asking(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(label.clone()), reconnect_token.as_deref(), asked).await {
                        Ok(connected) => flown = Some(connected),
                        Err(e) => tracing::info!("Rex: {host_key} didn't take the hello and the first requests together ({e}); connecting the plain way"),
                    }
                }
                let (ctrl, events, status, version, hosts_resp, km_resp, boot) = match flown {
                    Some((ctrl, events, answers)) => {
                        let mut answers = answers.into_iter();
                        let mut next = || answers.next().expect("five were asked");
                        let (status, version, hosts, keys, boot) = tokio::join!(next(), next(), next(), next(), next());
                        let boot = SessionSnapshot::from_bootstrap(&ctrl, olo_rex_client::answered("session.bootstrap", boot)).await;
                        (
                            ctrl,
                            events,
                            olo_rex_client::answered::<ServerStatus>("server.status", status),
                            olo_rex_client::answered::<ServerVersion>("server.version", version),
                            olo_rex_client::answered::<olo_rex_client::HostsListResponse>("hosts.list", hosts),
                            olo_rex_client::answered::<olo_rex_client::KeymapResponse>("client.keymap", keys),
                            boot,
                        )
                    }
                    None => {
                        let (stream, host_hdr) = endpoint.connect_control_stream().await?;
                        let (ctrl, events) = ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(label), reconnect_token.as_deref()).await?;
                        // (Asked together even so: one round trip.)
                        let (status, version, hosts, keys, boot) = tokio::join!(
                            ctrl.server_status(),
                            ctrl.server_version(),
                            ctrl.hosts_list(),
                            ctrl.client_keymap(&km_actions, &[], &user_overrides),
                            SessionSnapshot::read(&ctrl, &ticket_req)
                        );
                        (ctrl, events, status, version, hosts, keys, boot)
                    }
                };
                let client_id = ctrl.client_id().map(str::to_owned);
                let token = ctrl.reconnect_token().map(str::to_owned);
                let (status, version, hosts_resp, km_resp, boot) = (status.ok(), version.ok().map(|r| r.payload), hosts_resp.ok().map(|r| r.payload), km_resp.ok().map(|r| r.payload), boot?);
                Ok::<_, olo_rex_client::RexError>((ctrl, events, client_id, token, status, version, hosts_resp, km_resp, boot, spares))
            }
            .await;
            match connected {
                Ok((ctrl, mut events, client_id, token, status, version, hosts_resp, km_resp, boot, spares)) => {
                    self.spares.lock().unwrap_or_else(|e| e.into_inner()).insert(host_key.clone(), spares);
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

                    // (What is kept of the helper there follows what this
                    // connection found: gone from a host that had one, it
                    // is asked for before the next.)
                    if matches!(endpoint, RexEndpoint::Ssh { .. }) && self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.host_helper {
                        if ctrl.keeps_typing_in_order() {
                            self.helper_known(&host_key, Some(true));
                        } else if self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.helper_hosts.get(&host_key).is_some_and(|word| word == "yes") {
                            self.helper_known(&host_key, None);
                        }
                    }
                    if !helper_seen {
                        self.look_after_helper(&host_key, &endpoint);
                    }
                    self.look_at_what_the_host_has(&host_key, &endpoint);
                    self.give_the_agent(&host_key, &endpoint);

                    for view in initial_views {
                        self.ingest_session(&host_key, &ctrl, &endpoint, view, &pre_tickets).await;
                    }
                    // (The streams opened beforehand that no terminal
                    // took are let go: one kept would be closed by the
                    // host before a pane opened later could use it.)
                    self.spares.lock().unwrap_or_else(|e| e.into_inner()).remove(&host_key);
                    self.write_palette_index();

                    self.restore_forwards(&host_key, &ctrl, &endpoint);
                    self.resume_saved_forwards(&host_key, &ctrl, &endpoint);
                    self.ensure_kept_forwards(&host_key, &ctrl, &endpoint);
                    self.keep_folders_of(&host_key);

                    (self.notify)(HubNotice::StateChanged);

                    let resumed = ctrl.welcome.client.as_ref().is_some_and(|c| c.resumed);
                    let mut sequence = EventSequence { seen: if resumed && last_seq <= boot_seq { last_seq } else { 0 }, membership: boot_seq };
                    while let Some(env) = events.recv().await {
                        let Some(gap) = sequence.next(env.seq) else { continue };
                        if gap || matches!(env.event, ControlEvent::EventDropped { block_id: None, .. }) {
                            // (Said, so that it can be found afterwards: it costs a
                            // round of the link, the keys and the sessions among it.)
                            tracing::info!("Rex: events of {host_key} were missed before number {}; its sessions are asked for again", env.seq);
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
        let newly_exited;
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
            let live_windows: HashSet<String> = h.sessions.get(&session_id).map(|s| s.windows.iter().map(|w| w.window_id.clone()).collect()).unwrap_or_default();
            let theme = st.active_theme.clone();
            let live_ids: HashSet<String> = blocks.iter().map(|b| b.block_id.clone()).collect();
            st.retain_blocks(|bk| !(bk.host_key == host_key && bk.session_id == session_id && !live_ids.contains(&bk.block_id)));
            let (sub_changed, exited_list) = st.subagents.sync_live_windows(host_key, &session_id, &live_windows);
            if sub_changed {
                st.subagents.save();
            }
            newly_exited = exited_list;

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
        if !newly_exited.is_empty() {
            let mut log = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner());
            let ts = crate::subagents::now_unix_ms();
            for e in newly_exited {
                log.push(crate::subagents::SubagentEvent {
                    seq: 0,
                    timestamp_ms: ts,
                    host_key: e.host_key.clone(),
                    session_id: e.session_id.clone(),
                    parent_window_id: e.parent_window_id.clone(),
                    id: e.id.clone(),
                    name: e.name.clone(),
                    window_id: e.window_id.clone(),
                    cli_session_id: e.effective_session_id().map(str::to_owned),
                    event: "exited".into(),
                    state: crate::subagents::SubagentLiveState::Exited,
                    summary: Some("tab closed".into()),
                    last_message: None,
                    approval: None,
                    error: None,
                    tail: Vec::new(),
                });
            }
        }
        self.write_palette_index();
        self.subagent_events.notify_waiters();
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
        // (One opened beforehand, where the host is being connected to.)
        let spare = self.spares.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&bk.host_key).and_then(|spares| spares.pop());
        tokio::spawn(async move {
            // The stream is opened while its ticket is asked for (or was
            // opened beforehand): the ticket goes in the hello, which is
            // all that waits for it.
            let asked = async {
                match pre_ticket {
                    Some(ticket) => Ok(ticket),
                    None => ctrl.data_connect(&bk.session_id, &bk.block_id, "terminal", DATA_TERMINAL_SUBPROTOCOL).await.map(|r| r.payload.ticket),
                }
            };
            let opened = async {
                match spare {
                    Some(spare) => match spare.await {
                        Ok(Some(upgrade)) => Some((upgrade, true)),
                        _ => open_terminal_stream(&ep).await.map(|upgrade| (upgrade, false)),
                    },
                    None => open_terminal_stream(&ep).await.map(|upgrade| (upgrade, false)),
                }
            };
            let (ticket, opened) = tokio::join!(asked, opened);
            let ticket = match ticket {
                Ok(ticket) => ticket,
                Err(e) => {
                    tracing::debug!("data.connect {} failed: {e}", bk.block_id);
                    br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
                    return;
                }
            };
            let Some((upgrade, beforehand)) = opened else {
                br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
                return;
            };
            let mut stream = match upgrade.hello(&ticket).await {
                Ok(s) => s,
                // (One opened beforehand may have been closed by the
                // host while it waited, and the ticket spent on it or
                // not: both anew, once.)
                Err(first) if beforehand => {
                    tracing::debug!("terminal stream {} opened beforehand failed ({first}): opened anew", bk.block_id);
                    let anew = async {
                        let ticket = ctrl.data_connect(&bk.session_id, &bk.block_id, "terminal", DATA_TERMINAL_SUBPROTOCOL).await?.payload.ticket;
                        let (raw, header) = ep.connect_stream().await?;
                        TerminalDataStream::connect_hello(raw, &header, &ticket).await
                    };
                    match anew.await {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::debug!("terminal stream connect {} failed: {e}", bk.block_id);
                            br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
                            return;
                        }
                    }
                }
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
                // (Asked together: one round trip of the link for the
                // three, where each waited for the one before.)
                let (status, ports, process) = tokio::join!(
                    c_meta.terminal_program_status(&bk_meta.session_id, &bk_meta.block_id),
                    c_meta.terminal_ports(&bk_meta.session_id, &bk_meta.block_id),
                    c_meta.terminal_process(&bk_meta.session_id, &bk_meta.block_id)
                );
                {
                    let mut block = br_meta.lock().unwrap_or_else(|e| e.into_inner());
                    block.status_snapshot(status.ok());
                    if let Ok(r) = ports {
                        block.set_ports(r.seq, r.payload);
                    }
                    if let Ok(r) = process {
                        block.process_snapshot(r);
                    }
                }
                notify_meta(HubNotice::StateChanged);
            });

            let mut attachment = Some(crate::attach::Attachment::default());
            let mut apc = InlineApcDemuxer::new(stream.welcome.channel_nonce);
            // Catching up (HACKING, Rex, "A pane catches up"): the
            // server sends a stream everything its terminal wrote,
            // however far behind the reader is, so after a flood on a
            // narrow link a pane goes on scrolling what is long over.
            // While a stream pours, the host is asked now and then
            // whether the shell itself is at its prompt again; where it
            // is and the stream still pours, what comes is the past,
            // and the terminal is given a new stream, whose first
            // screen is the present.
            let (mut pouring_since, mut poured, mut last_chunk) = (None::<Instant>, 0u64, Instant::now());
            let (mut look_at, mut looks_apart, mut caught_up_at) = (None::<tokio::time::Instant>, CATCH_UP_LOOK, None::<Instant>);
            let (looked_tx, mut looked_rx) = tokio::sync::mpsc::unbounded_channel::<bool>();
            // (The host said the shell reads: when to see whether the
            // stream went on pouring all the same, and how much had
            // come by then.)
            let mut said_over = None::<(tokio::time::Instant, u64)>;
            loop {
                let frame = tokio::select! {
                    _ = &mut rx => { stream.close_clean().await; break; }
                    res=stream.next_frame()=>res,
                    _ = async { tokio::time::sleep_until(look_at.unwrap_or_else(tokio::time::Instant::now)).await }, if look_at.is_some() => {
                        look_at = None;
                        let pours = last_chunk.elapsed() < POUR_PAUSE && poured >= POURED && pouring_since.is_some_and(|since| since.elapsed() >= POURED_FOR);
                        if pours {
                            // (Not from under someone reading what scrolled by.)
                            let reading = {
                                let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                g.copy_mode.is_some() || !g.terminal.is_viewport_at_bottom()
                            };
                            if !reading {
                                let (ctrl, bk, looked) = (ctrl.clone(), bk.clone(), looked_tx.clone());
                                tokio::spawn(async move {
                                    match tokio::try_join!(ctrl.terminal_input_mode(&bk.session_id, &bk.block_id), ctrl.terminal_process(&bk.session_id, &bk.block_id)) {
                                        Ok((mode, process)) => {
                                            let _ = looked.send(shell_reads(&mode.payload, &process.payload));
                                        }
                                        Err(e) => tracing::debug!("{} pours, and its host couldn't be asked whether its shell reads: {e}", bk.block_id),
                                    }
                                });
                            }
                        }
                        // (Looked at again while it brings anything: a
                        // narrow link's stream takes its time to have
                        // poured, and was looked at once, too soon, and
                        // never again.)
                        if last_chunk.elapsed() < POUR_PAUSE {
                            look_at = Some(tokio::time::Instant::now() + if pours { looks_apart } else { POURED_FOR });
                        }
                        continue;
                    }
                    Some(reads) = looked_rx.recv() => {
                        if reads && said_over.is_none() {
                            said_over = Some((tokio::time::Instant::now() + POUR_PAUSE, poured));
                        }
                        continue;
                    }
                    _ = async { tokio::time::sleep_until(said_over.map_or_else(tokio::time::Instant::now, |(at, _)| at)).await }, if said_over.is_some() => {
                        let had = said_over.take().map_or(0, |(_, had)| had);
                        // The shell was at its prompt, and its terminal
                        // has gone on pouring since: what comes is the
                        // past. A new stream in the old one's place.
                        // (The old one isn't read meanwhile, so the host
                        // sends no more of it than its window.)
                        tracing::debug!("{} poured {} kB more in the {POUR_PAUSE:?} after its shell was said to read", bk.block_id, poured.saturating_sub(had) / 1024);
                        if poured >= had + POURS_ON && pouring_since.is_some() {
                            // The new stream is opened as far as its upgrade and
                            // its ticket had while the old one still stands; then
                            // the old one is ended, and the new one says whose it
                            // is: the server takes one stream of a terminal from a
                            // client at a time ("AlreadyConnected" otherwise, and
                            // again if its end hasn't reached it yet: tried anew
                            // a few times, each with a ticket of its own).
                            let anew = async {
                                let (ticket, opened) = tokio::join!(ctrl.data_connect(&bk.session_id, &bk.block_id, "terminal", DATA_TERMINAL_SUBPROTOCOL), open_terminal_stream(&ep));
                                let (ticket, opened) = (ticket?.payload.ticket, opened.ok_or_else(|| olo_rex_client::RexError::Closed("no stream".to_owned()))?);
                                stream.close().await;
                                let mut tried = opened.hello(&ticket).await;
                                for wait in [200u64, 500, 1200] {
                                    if tried.is_ok() {
                                        break;
                                    }
                                    tokio::time::sleep(Duration::from_millis(wait)).await;
                                    let ticket = ctrl.data_connect(&bk.session_id, &bk.block_id, "terminal", DATA_TERMINAL_SUBPROTOCOL).await?.payload.ticket;
                                    let (raw, header) = ep.connect_stream().await?;
                                    tried = TerminalDataStream::connect_hello(raw, &header, &ticket).await;
                                }
                                tried
                            };
                            match tokio::time::timeout(Duration::from_secs(30), anew).await {
                                Ok(Ok(fresh)) => {
                                    tracing::info!("Rex: {} went on pouring with its shell at its prompt ({} kB in {:.1} s): caught up with a new stream", bk.block_id, poured / 1024, pouring_since.map_or(0.0, |since| since.elapsed().as_secs_f64()));
                                    stream = fresh;
                                    attachment = Some(crate::attach::Attachment::default());
                                    apc = InlineApcDemuxer::new(stream.welcome.channel_nonce);
                                    // (Caught up twice within a short while: something goes on
                                    // writing there; looked at less often.)
                                    looks_apart = if caught_up_at.is_some_and(|at| at.elapsed() < looks_apart * 5) { (looks_apart * 2).min(CATCH_UP_LOOK_MOST) } else { CATCH_UP_LOOK };
                                    caught_up_at = Some(Instant::now());
                                    (pouring_since, poured, look_at) = (None, 0, None);
                                }
                                // (The old stream may be ended by now: the terminal is
                                // left to be attached again, as after any stream's end.)
                                Ok(Err(e)) => {
                                    tracing::warn!("Rex: catching {} up failed: {e}", bk.block_id);
                                    break;
                                }
                                Err(_) => {
                                    tracing::warn!("Rex: catching {} up took too long", bk.block_id);
                                    break;
                                }
                            }
                        }
                        continue;
                    }
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
                // (Pouring: what the terminal writes, with no long
                // pause in it. Not a stream's first screen, which on a
                // narrow link takes seconds to come and isn't the
                // terminal writing: counted, it had a terminal caught
                // up again and again, each time for the last time's
                // first screen.)
                if !live.is_empty() {
                    if last_chunk.elapsed() > POUR_PAUSE || pouring_since.is_none() {
                        (pouring_since, poured) = (Some(Instant::now()), 0);
                        look_at = Some(tokio::time::Instant::now() + POURED_FOR);
                    }
                    poured += live.len() as u64;
                    last_chunk = Instant::now();
                }
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
                if !live.is_empty() {
                    inner.subagent_events.notify_waiters();
                }
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
                    let (br, notify, ctrl, bk, helper) = (br.clone(), inner.notify.clone(), ctrl.clone(), bk.clone(), inner.clone());
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(GUESS_AFTER).await;
                            let (changed, quiet) = br.lock().unwrap_or_else(|e| e.into_inner()).guess_now(GUESS_AFTER);
                            if changed {
                                notify(HubNotice::StateChanged);
                            }
                            helper.record_subagent_transition(&bk);
                            helper.subagent_events.notify_waiters();
                            // (Keep It Going's and Auto Approve's look,
                            // where the pane has either on.)
                            helper.agent_look(&bk);
                            if quiet {
                                break;
                            }
                        }
                        ask_the_host(&br, &ctrl, &bk, &notify).await;
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
        // (Asked together: one round trip of the link, not four.)
        let overrides = self.state.lock().unwrap_or_else(|e| e.into_inner()).settings.keymap_overrides.clone();
        let actions = keymap::register_actions_payload();
        let (snapshot, server, hosts, keymap) = tokio::join!(SessionSnapshot::read(ctrl, &[]), ctrl.server_status(), ctrl.hosts_list(), ctrl.client_keymap(&actions, &[], &overrides));
        let (snapshot, server, hosts, keymap) = (snapshot?, server?, hosts?, keymap?.payload);
        let rows = snapshot.rows;
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
                self.ensure_kept_forwards(host_key, ctrl, ep);
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
                let mut reattach_kept = false;
                {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(h) = st.hosts.get_mut(host_key) {
                        h.sessions.remove(&session_id);
                        h.pending_focus.remove(&session_id);
                        h.session_summaries.retain(|s| s.session_id != session_id);
                    }
                    st.retain_blocks(|bk| !(bk.host_key == host_key && bk.session_id == session_id));
                    st.forwards.retain(|f| {
                        if f.host_key == host_key && f.handle.session_id == session_id {
                            if f.kept {
                                reattach_kept = true;
                                return false;
                            }
                            if f.made == crate::forwards::Made::Agent {
                                return false;
                            }
                        }
                        true
                    });
                }
                if reattach_kept {
                    self.ensure_kept_forwards(host_key, ctrl, ep);
                }
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
                    self.subagent_events.notify_waiters();
                    (self.notify)(HubNotice::StateChanged);
                }
            }
            ControlEvent::BlockEvent { session_id, block_id, name, payload, .. } => {
                if name == "clipboard_written" {
                    // A program in the terminal copied something (OSC 52):
                    // it is copied here, on this computer or a far host, as
                    // in Terminal (Ben, 2026-11-23: "make sure osc 52 for
                    // copy/paste etc works for the remote rex terminals").
                    // It was dropped for every host not granted the
                    // clipboard, which a host isn't until the person says
                    // so: that grant is for what the host's programs ask
                    // of the bridge (to read the clipboard too), not for
                    // what a terminal the person works in copies.
                    //
                    // The server gives the text with its word where it is
                    // short, and only its number and size where it is long
                    // (over some kilobytes: measured, a copy of 300 kB came
                    // as `{"id":2,"bytes":300003}`, and nothing was copied):
                    // then it is read, a piece at a time.
                    let stamp = CLIPBOARD_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if let Some(text) = payload.get("text").and_then(Value::as_str) {
                        (self.notify)(HubNotice::SetClipboard(text.to_owned()));
                    } else if let Some(id) = payload.get("id").and_then(Value::as_u64)
                        && payload.get("bytes").and_then(Value::as_u64).is_some_and(|bytes| bytes <= CLIPBOARD_MOST)
                    {
                        let (ctrl, notify) = (ctrl.clone(), self.notify.clone());
                        tokio::spawn(async move {
                            let mut text = String::new();
                            loop {
                                let Ok(piece) = ctrl.terminal_clipboard_read(&session_id, &block_id, Some(id), text.len()).await else { return };
                                text.push_str(&piece.payload.data);
                                if piece.payload.eof {
                                    break;
                                }
                                if piece.payload.data.is_empty() || text.len() as u64 > CLIPBOARD_MOST {
                                    return;
                                }
                            }
                            // (Not over something copied since.)
                            if CLIPBOARD_WRITES.load(std::sync::atomic::Ordering::Relaxed) == stamp {
                                notify(HubNotice::SetClipboard(text));
                            }
                        });
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
                                    let canonical = payload.get("canonical").and_then(Value::as_bool).unwrap_or(true);
                                    g.set_password_prompt(asks_for_secret(echo, canonical));
                                    // (The helper's, said unasked: with `shell`, whether the
                                    // terminal's own first process is in its foreground, and
                                    // `after`, the last thing typed that the server had taken
                                    // when it looked. The server has no such event of its own.)
                                    if let Some(shell) = payload.get("shell").and_then(Value::as_bool) {
                                        let after = match payload.get("after") {
                                            Some(Value::String(id)) => id.parse::<u64>().ok(),
                                            _ => Some(0),
                                        };
                                        g.host_reads = after.filter(|_| shell && !echo && !canonical);
                                        let screen = g.caret_screen();
                                        g.host_says_prompt(ctrl.last_typed(), screen.as_ref());
                                    }
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
                    self.record_subagent_transition(&bk);
                    self.subagent_events.notify_waiters();
                    if let Some((title, body, kind)) = activity_note {
                        self.pane_attention(&bk, title, body, kind);
                    }
                    // (What the agent is doing may have changed: Keep It
                    // Going's and Auto Approve's look.)
                    self.agent_look(&bk);
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
                // (Not on the events' own thread of work: a bridge's
                // command may take as long as the person takes to answer
                // Passwords, and the host's events go on meanwhile.)
                let (inner, ctrl, host_key) = (self.clone(), ctrl.clone(), host_key.to_owned());
                tokio::spawn(async move {
                    match inner.handle_client_perform(&host_key, req.action.clone(), req.args.clone(), req.session_id.clone(), req.block_id.clone()).await {
                        Some(answer) => {
                            let _ = ctrl.client_reply_ok(&req.call_id, answer).await;
                        }
                        None => {
                            let _ = ctrl.client_reply_err(&req.call_id, "permission_denied", "No matching Rex workspace or permission for this action").await;
                        }
                    }
                });
            }
            ControlEvent::ClientCallRequested(req) => {
                let _ = ctrl.client_reply_err(&req.call_id, "unsupported", &format!("Unknown RPC {}.{}", req.server, req.method)).await;
            }
            ControlEvent::Unknown { kind, .. } => tracing::debug!(%kind,"Unrecognized Rex event"),
            _ => {}
        }
    }

    async fn handle_client_perform(self: &Arc<Self>, host_key: &str, action: String, args: Option<Value>, session_id: Option<String>, block_id: Option<String>) -> Option<Value> {
        if action.starts_with("olo.") {
            // Server-side per-child desktop isolation policy (`--desktop none|screens=NAME,…|inherit`):
            // default for every child subagent tab is `none`, refusing `op`, `screen`, `open`,
            // clipboard, file bridge, and desktop commands even if the host allows them.
            if let Some(ref bid) = block_id {
                let child_deny = {
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.subagents.by_block(host_key, session_id.as_deref(), bid).and_then(|child| child.desktop_policy.check_action(&action, args.as_ref()).err())
                };
                if let Some(reason) = child_deny {
                    return Some(json!({
                        "ok": false,
                        "stderr": format!("olo: {reason}\n"),
                        "data": { "code": 69 }
                    }));
                }
            }
            let (is_local, policy) = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let is_local = st.hosts.get(host_key).is_some_and(|h| h.is_local);
                let policy = st.settings.policy_for_host(host_key, is_local);
                (is_local, policy)
            };
            let done = Some(json!({ "ok": true }));
            // A `rex://HOST/PATH` URI from a host (`olo open rex://…`): opened in Tracker.
            if action == "olo.open"
                && let Some(target) = args.as_ref().and_then(|a| a.get("target")).and_then(Value::as_str).filter(|t| t.starts_with(olo_kit_base::tracker::REX_SCHEME))
            {
                if !policy.allow_open_url {
                    return None;
                }
                crate::bridge::open_uri_in_tracker(target);
                return Some(json!({ "ok": true, "stdout": format!("Opening {target} in Tracker\n") }));
            }
            // A file of a far host's, by its path (its `xdg-open FILE`,
            // `olo open PATH`): not this computer's file at that path, as
            // it was opened before, but the host's, copied here and opened
            // in its app as Quick Look does.
            if action == "olo.open"
                && !is_local
                && let Some(path) = args.as_ref().and_then(|a| a.get("target")).and_then(Value::as_str).filter(|t| t.starts_with('/'))
            {
                if !policy.allow_open_url {
                    return None;
                }
                let (Some(session_id), Some(block_id)) = (session_id.clone(), block_id.clone()) else {
                    return Some(json!({ "ok": false, "stderr": "olo: a file is opened from a pane of a Rex session\n" }));
                };
                (self.notify)(HubNotice::OpenHostFile(BlockKey { host_key: host_key.to_owned(), session_id, block_id }, path.to_owned()));
                return Some(json!({ "ok": true, "stdout": format!("Opening {path} on the Olo desktop\n") }));
            }
            match action.as_str() {
                // What a session asks of this computer (`bridge`): a
                // link opened, Passwords' `op`, an Agent Screen's
                // `olo-screen`, each done as the host's permissions
                // allow and answered with what the command wrote; a
                // command asked before waited for again; a piece of a
                // picture an answer named.
                "olo.wait" | "olo.file" | "olo.open" | "olo.op" | "olo.screen" | "olo.get" | "olo.ls" => {
                    let arg = |name: &str| args.as_ref().and_then(|a| a.get(name));
                    let text = |name: &str| arg(name).and_then(Value::as_str).unwrap_or_default().to_owned();
                    let (cmd, given) = match action.as_str() {
                        "olo.wait" => return Some(self.bridge_calls.wait(&text("id")).await),
                        "olo.file" => return Some(self.bridge_calls.piece(&text("id"), arg("offset").and_then(Value::as_u64).unwrap_or(0) as usize)),
                        "olo.open" => ("open", vec![text("target")]),
                        other => (
                            match other {
                                "olo.op" => "op",
                                "olo.get" => "get",
                                "olo.ls" => "ls",
                                _ => "screen",
                            },
                            arg("args").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default(),
                        ),
                    };
                    // (`op`'s standard input and the files its flags read: the
                    // host's, sent with the request; `bridge::OpCarry`.)
                    let payload = if cmd == "op" { json!({ "stdin": arg("stdin"), "files": arg("files") }) } else { Value::Null };
                    let req = bridge::BridgeRequest { cmd: cmd.to_owned(), session_id: session_id.clone(), block_id: block_id.clone(), args: given, payload };
                    let label = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).map(|h| h.label.clone()).unwrap_or_else(|| host_key.to_owned());
                    let (inner, hk, sid) = (self.clone(), host_key.to_owned(), session_id.clone());
                    let forward: bridge::LocalhostForwardFn = Arc::new(move |host, port| {
                        let (inner, hk, sid) = (inner.clone(), hk.clone(), sid.clone());
                        Box::pin(async move {
                            // (Through the asking session, or the host's first.)
                            let sid = sid
                                .or_else(|| inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&hk).and_then(|h| h.session_summaries.first().map(|s| s.session_id.clone())))
                                .ok_or_else(|| "no session of the host to forward through".to_owned())?;
                            inner.ensure_ephemeral_forward(&hk, &sid, &host, port).await
                        })
                    });
                    // (This computer's own sessions reach it directly: no forward.)
                    let policy = if is_local { crate::settings::HostBridgePolicy { allow_localhost_autoforward: false, ..policy } } else { policy };
                    return Some(self.bridge_calls.answer(async move { bridge::handle_bridge_request(&label, &policy, req, &forward, &|_| {}).await }).await);
                }
                // A host's `olo send PATH…`: its files and folders saved
                // here, in Downloads ▸ Rex ▸ HOST, by the engine through
                // its helper (`bridge_send`).
                "olo.send" => {
                    let given: Vec<String> = args
                        .as_ref()
                        .and_then(|a| a.get("args"))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect())
                        .unwrap_or_default();
                    if !policy.allow_send_files {
                        return Some(json!({ "ok": false, "stderr": "olo: this host may not send files to the Olo desktop (Preferences ▸ Rex: Send files to Downloads on this computer)\n" }));
                    }
                    if is_local {
                        return Some(json!({ "ok": true, "stdout": "These are the Olo desktop's own files: nothing to send.\n" }));
                    }
                    let (inner, hk) = (self.clone(), host_key.to_owned());
                    return Some(self.bridge_calls.answer(async move { inner.bridge_send(&hk, given).await }).await);
                }
                // A host's `olo sync …`: its folders kept in step here
                // (`bridge_sync`), under the same leave as `olo send`.
                "olo.sync" => {
                    let given: Vec<String> = args
                        .as_ref()
                        .and_then(|a| a.get("args"))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect())
                        .unwrap_or_default();
                    if !policy.allow_send_files {
                        return Some(json!({ "ok": false, "stderr": "olo: this host may not send files to the Olo desktop (Preferences ▸ Rex: Send files to Downloads on this computer)\n" }));
                    }
                    if is_local {
                        return Some(json!({ "ok": true, "stdout": "These are the Olo desktop's own folders: nothing to keep in step.\n" }));
                    }
                    let (inner, hk) = (self.clone(), host_key.to_owned());
                    return Some(self.bridge_calls.answer(async move { inner.bridge_sync(&hk, given).await }).await);
                }
                // A host's `olo forward`: a server it reaches, reached
                // from here at the port it names (`forwards`).
                "olo.forward" => {
                    let given: Vec<String> = args
                        .as_ref()
                        .and_then(|a| a.get("args"))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect())
                        .unwrap_or_default();
                    if !policy.allow_localhost_autoforward {
                        return Some(json!({ "ok": false, "stderr": "olo: this host may not reach its servers from the Olo desktop (Preferences ▸ Rex: Reach its local servers)\n" }));
                    }
                    return Some(self.bridge_forward(host_key, is_local, session_id, &given).await);
                }
                // A host's `olo run [--cwd DIR] -- COMMAND…`: runs a command on
                // the Olo desktop after user approval (or silently when tray
                // Approve Without Asking is active), streaming stdout/stderr live.
                "olo.run" => {
                    if !policy.allow_run_commands {
                        return Some(json!({
                            "ok": false,
                            "done": true,
                            "stderr": "olo: this host may not run commands on the Olo desktop (Preferences ▸ Rex: Ask to run commands on this computer)\n",
                            "data": { "code": 69 }
                        }));
                    }
                    let given: Vec<String> = args
                        .as_ref()
                        .and_then(|a| a.get("args"))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect())
                        .unwrap_or_default();
                    let ask = match crate::bridge_run::RunAsk::parse(&given) {
                        Ok(a) => a,
                        Err(e) => return Some(json!({ "ok": false, "done": true, "stderr": format!("olo: {e}\n"), "data": { "code": 2 } })),
                    };
                    let req_id = args.as_ref().and_then(|a| a.get("id")).and_then(Value::as_str).map(str::to_owned);
                    let label = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).map(|h| h.label.clone()).unwrap_or_else(|| host_key.to_owned());
                    let notify_cb = self.notify.clone();
                    let on_quiet = Arc::new(move |title: String, body: String| {
                        (notify_cb)(HubNotice::QuietNote { title, body });
                    });
                    return Some(self.run_jobs.start(req_id, label, ask, self.run_approvals.clone(), on_quiet).await);
                }
                "olo.run.poll" => {
                    let id = args.as_ref().and_then(|a| a.get("id")).and_then(Value::as_str).unwrap_or_default();
                    return Some(self.run_jobs.poll(id).await);
                }
                "olo.run.cancel" => {
                    let id = args.as_ref().and_then(|a| a.get("id")).and_then(Value::as_str).unwrap_or_default();
                    return Some(self.run_jobs.cancel(id));
                }
                "olo.notify" => {
                    self.bridge_notice(host_key, session_id.as_deref(), block_id.as_deref(), args.as_ref().unwrap_or(&Value::Null));
                    return done;
                }
                // What's on this computer's clipboard, as text (a host's
                // `wl-paste`, `xclip -o`, `olo paste`), where the host may
                // read and set it.
                "olo.clipboard.get" => {
                    if !policy.allow_remote_control {
                        return None;
                    }
                    const TEXT: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];
                    let read = tokio::task::spawn_blocking(|| olo_kit_base::clipboard::read(&TEXT)).await;
                    return Some(match read {
                        Ok(Ok(bytes)) => json!({ "ok": true, "stdout": String::from_utf8_lossy(&bytes.unwrap_or_default()) }),
                        Ok(Err(e)) => json!({ "ok": false, "stderr": format!("olo: the clipboard couldn't be read: {e}\n") }),
                        Err(e) => json!({ "ok": false, "stderr": format!("olo: the clipboard couldn't be read: {e}\n") }),
                    });
                }
                "olo.agent.start" | "olo.agent.list" | "olo.agent.status" | "olo.agent.read" | "olo.agent.watch" | "olo.agent.send" | "olo.agent.approve" | "olo.agent.stop" | "olo.agent.prune"
                | "olo.agent.focus" => {
                    return Some(self.handle_agent_action(host_key, &action, args, session_id, block_id).await);
                }
                "olo.clipboard.set" => {
                    if !policy.allow_remote_control {
                        return None;
                    }
                    if let Some(text) = args.as_ref().and_then(|a| a.get("text")).and_then(Value::as_str) {
                        (self.notify)(HubNotice::SetClipboard(text.to_owned()));
                    }
                    return done;
                }
                _ => {}
            }
        }

        crate::ui::request_action(host_key, session_id, block_id, action, args).then_some(json!({ "ok": true }))
    }

    /// Dispatches native nested subagent tab actions (`olo.agent.*`).
    async fn handle_agent_action(self: &Arc<Self>, host_key: &str, action: &str, args: Option<Value>, session_id: Option<String>, block_id: Option<String>) -> Value {
        match action {
            "olo.agent.start" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_start(&hk, args.as_ref(), session_id, block_id).await }).await
            }
            "olo.agent.list" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_list(&hk, args.as_ref(), session_id, block_id).await }).await
            }
            "olo.agent.status" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_status(&hk, args.as_ref(), session_id).await }).await
            }
            "olo.agent.read" => self.agent_read(host_key, args.as_ref(), session_id).await,
            "olo.agent.watch" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_watch(&hk, args.as_ref(), session_id, block_id).await }).await
            }
            "olo.agent.send" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_send(&hk, args.as_ref(), session_id).await }).await
            }
            "olo.agent.approve" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_approve(&hk, args.as_ref(), session_id).await }).await
            }
            "olo.agent.stop" => {
                let (inner, hk) = (self.clone(), host_key.to_owned());
                self.bridge_calls.answer(async move { inner.agent_stop(&hk, args.as_ref(), session_id).await }).await
            }
            "olo.agent.prune" => self.agent_prune(host_key, args.as_ref(), session_id).await,
            "olo.agent.focus" => self.agent_focus(host_key, args.as_ref(), session_id).await,
            _ => json!(bridge::BridgeResponse::err(format!("unknown agent action `{action}`"))),
        }
    }

    fn resolve_session_and_parent(
        &self,
        host_key: &str,
        session_id: Option<&str>,
        block_id: Option<&str>,
        explicit_parent: Option<&str>,
    ) -> Result<(ControlClient, RexEndpoint, String, String, Option<String>), String> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let h = st.hosts.get(host_key).ok_or_else(|| format!("unknown Rex host `{host_key}`"))?;
        let ctrl = h.control.clone().ok_or_else(|| "Rex host is not connected".to_owned())?;
        let ep = h.endpoint.clone().ok_or_else(|| "Rex host endpoint unavailable".to_owned())?;
        // Resolve session: explicit `session_id` -> session owning `block_id` -> first session summary.
        let sid = session_id
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| block_id.filter(|s| !s.is_empty()).and_then(|bid| h.sessions.iter().find(|(_, v)| v.windows.iter().any(|w| w.all_blocks().any(|b| b.block_id == bid))).map(|(k, _)| k.clone())))
            .or_else(|| h.session_summaries.first().map(|s| s.session_id.clone()))
            .ok_or_else(|| "no active Rex session found for this host".to_owned())?;
        let view = h.sessions.get(&sid).ok_or_else(|| format!("Rex session `{sid}` is not loaded"))?;
        let parent_win = if let Some(p) = explicit_parent.filter(|s| !s.is_empty()) {
            if view.windows.iter().any(|w| w.window_id == p) {
                p.to_owned()
            } else if let Some(w) = view.windows.iter().find(|w| w.all_blocks().any(|b| b.block_id == p)) {
                w.window_id.clone()
            } else if let Some(sub) = st.subagents.resolve_target(host_key, Some(&sid), p) {
                sub.window_id
            } else {
                return Err(format!("parent tab `{p}` not found in session `{sid}`"));
            }
        } else if let Some(bid) = block_id.filter(|s| !s.is_empty()) {
            view.windows
                .iter()
                .find(|w| w.all_blocks().any(|b| b.block_id == bid))
                .map(|w| w.window_id.clone())
                .or_else(|| view.active_window_id.clone())
                .or_else(|| view.windows.first().map(|w| w.window_id.clone()))
                .ok_or_else(|| "could not determine parent tab for this pane".to_owned())?
        } else {
            view.active_window_id.clone().or_else(|| view.windows.first().map(|w| w.window_id.clone())).ok_or_else(|| "session has no open tabs".to_owned())?
        };
        let parent_cwd =
            block_id.filter(|s| !s.is_empty()).and_then(|bid| st.blocks.get(&BlockKey { host_key: host_key.to_owned(), session_id: sid.clone(), block_id: bid.to_owned() })).and_then(|br| {
                let pwd = br.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone();
                (!pwd.is_empty()).then_some(pwd)
            });
        Ok((ctrl, ep, sid, parent_win, parent_cwd))
    }

    fn live_subagent_rich(&self, entry: &crate::subagents::NativeSubagent) -> (crate::subagents::ScreenClassification, String, u64, u64, Option<i32>) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let alive =
            entry.exited_at.is_none() && st.hosts.get(&entry.host_key).and_then(|h| h.sessions.get(&entry.session_id)).is_some_and(|s| s.windows.iter().any(|w| w.window_id == entry.window_id));
        if !alive {
            let cls = crate::subagents::classify_screen_rich(entry.cli, AgentActivity::Idle, "", false);
            return (cls, String::new(), 0, 0, entry.last_pid);
        }
        let bk = BlockKey { host_key: entry.host_key.clone(), session_id: entry.session_id.clone(), block_id: entry.block_id.clone() };
        let Some(br) = st.blocks.get(&bk) else {
            let cls = crate::subagents::ScreenClassification {
                state: crate::subagents::SubagentLiveState::Working,
                summary: Some("starting".into()),
                approval: None,
                folder_trust: None,
                api_error: None,
                usage: None,
            };
            return (cls, String::new(), 0, 0, entry.last_pid);
        };
        let g = br.lock().unwrap_or_else(|e| e.into_inner());
        let screen = g.terminal.format(olo_rex_vt::FormatKind::Plain, false, true).unwrap_or_default();
        let mut cls = crate::subagents::classify_screen_rich(entry.cli, g.agent_activity, &screen, true);
        if cls.summary.is_none() {
            cls.summary = g.program_statuses.first().and_then(|r| r.msg.clone().or_else(|| r.title.clone()));
        }
        let pid = g.process.as_ref().and_then(|p| p.child.as_ref().or(p.foreground.as_ref())).map(|p| p.pid as i32).filter(|&p| p > 0).or(entry.last_pid);
        (cls, screen, g.last_output_at, g.last_state_change_at, pid)
    }

    /// Runs a single batched `olo-rex-link agent inspect` call on `host_key` for all given
    /// entries, persisting any newly discovered `cli_session_id` / `transcript_path` / `pid`.
    async fn inspect_subagents_on_host(self: &Arc<Self>, host_key: &str, entries: &[crate::subagents::NativeSubagent]) -> HashMap<String, Value> {
        if entries.is_empty() {
            return HashMap::new();
        }
        let req_items: Vec<Value> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            entries
                .iter()
                .map(|e| {
                    let bk = BlockKey { host_key: e.host_key.clone(), session_id: e.session_id.clone(), block_id: e.block_id.clone() };
                    let (pid, block_cwd) = st
                        .blocks
                        .get(&bk)
                        .map(|br| {
                            let g = br.lock().unwrap_or_else(|er| er.into_inner());
                            let p = g.process.as_ref().and_then(|pr| pr.child.as_ref().or(pr.foreground.as_ref())).map(|pr| pr.pid as i32).filter(|&p| p > 0);
                            let d = (!g.pwd.is_empty()).then(|| g.pwd.clone());
                            (p, d)
                        })
                        .unwrap_or((None, None));
                    json!({
                        "id": e.id,
                        "cli": e.cli.binary(),
                        "shell_pid": pid.or(e.last_pid).unwrap_or(0),
                        "cwd": block_cwd.or_else(|| e.cwd.clone()).unwrap_or_default(),
                        "known_session_id": e.effective_session_id(),
                        "known_transcript": e.transcript_path,
                        "started_at": e.started_at,
                    })
                })
                .collect()
        };
        let payload = format!("{}\n", json!({ "items": req_items }));
        let Some(raw) = crate::activity_watch::run_host_link(&self.state, host_key, &["agent", "inspect"], Some(&payload)).await else {
            return HashMap::new();
        };
        let Ok(parsed) = serde_json::from_str::<Value>(raw.trim()) else {
            return HashMap::new();
        };
        let mut map = HashMap::new();
        let mut any_changed = false;
        if let Some(arr) = parsed.get("items").and_then(Value::as_array) {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for item in arr {
                let Some(id) = item.get("id").and_then(Value::as_str) else { continue };
                let sid = item.get("cli_session_id").and_then(Value::as_str);
                let tp = item.get("transcript_path").and_then(Value::as_str);
                let pid = item.get("pid").and_then(Value::as_i64).map(|p| p as i32);
                if st.subagents.update_discovered(id, sid, tp, pid) {
                    any_changed = true;
                }
                map.insert(id.to_owned(), item.clone());
            }
            if any_changed {
                st.subagents.save();
            }
        }
        map
    }

    /// O(1) dirty-block subagent state tracker: called only when a block receives output or
    /// status updates. Returns immediately for non-child panes; for child subagents, debounces
    /// non-working transitions by 250 ms before recording them in `subagent_log` so `watch --all
    /// --since` never fires on transient repaints and never misses a confirmed event.
    fn record_subagent_transition(self: &Arc<Self>, bk: &BlockKey) {
        let entry = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.subagents.by_block(&bk.host_key, Some(&bk.session_id), &bk.block_id).cloned()
        };
        let Some(entry) = entry else { return };
        let (cls, _screen, _, _, _) = self.live_subagent_rich(&entry);
        let br = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.blocks.get(bk).cloned()
        };
        let Some(br) = br else { return };
        let next_gen = {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if g.subagent_last_state == Some(cls.state) {
                return;
            }
            g.subagent_last_state = Some(cls.state);
            g.last_state_change_at = crate::subagents::now_unix_ms();
            g.subagent_check_gen = g.subagent_check_gen.wrapping_add(1);
            g.subagent_check_gen
        };
        if matches!(cls.state, crate::subagents::SubagentLiveState::Working) {
            return;
        }
        let target_state = cls.state;
        let inner = self.clone();
        let bk = bk.clone();
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let still_same_gen = {
                let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                st.blocks.get(&bk).is_some_and(|b| b.lock().unwrap_or_else(|e| e.into_inner()).subagent_check_gen == next_gen)
            };
            if !still_same_gen {
                return;
            }
            let Some(fresh_entry) = ({
                let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                st.subagents.by_block(&bk.host_key, Some(&bk.session_id), &bk.block_id).cloned()
            }) else {
                return;
            };
            let (cls2, screen2, _, _, _) = inner.live_subagent_rich(&fresh_entry);
            if cls2.state != target_state {
                return;
            }
            // Inspect transcript once on confirmed notable state so `last_message`, `cli_session_id`,
            // and untruncated `approval` commands are attached right inside the event!
            let inspected = inner.inspect_subagents_on_host(&fresh_entry.host_key, std::slice::from_ref(&fresh_entry)).await;
            let info = inspected.get(&fresh_entry.id);
            let latest_entry = {
                let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                st.subagents.by_block(&bk.host_key, Some(&bk.session_id), &bk.block_id).cloned().unwrap_or(fresh_entry)
            };
            let last_message = info.and_then(|v| v.get("last_message")).and_then(Value::as_str).map(str::to_owned);
            let tx_err = info.and_then(|v| v.get("transcript_error")).and_then(Value::as_str).map(str::to_owned);
            let pending_tool = info.and_then(|v| v.get("pending_tool"));
            let approval = cls2.approval.as_ref().map(|ap| crate::subagents::approval_to_json(ap, pending_tool));
            let error = cls2.api_error.clone().or(tx_err);
            let final_state = if error.is_some() && cls2.state != crate::subagents::SubagentLiveState::WaitingApproval { crate::subagents::SubagentLiveState::Error } else { cls2.state };
            let tail: Vec<String> = screen2.lines().rev().filter(|l| !l.trim().is_empty()).take(10).map(str::to_owned).collect::<Vec<_>>().into_iter().rev().collect();
            {
                let mut log = inner.subagent_log.lock().unwrap_or_else(|e| e.into_inner());
                log.push(crate::subagents::SubagentEvent {
                    seq: 0,
                    timestamp_ms: crate::subagents::now_unix_ms(),
                    host_key: latest_entry.host_key.clone(),
                    session_id: latest_entry.session_id.clone(),
                    parent_window_id: latest_entry.parent_window_id.clone(),
                    id: latest_entry.id.clone(),
                    name: latest_entry.name.clone(),
                    window_id: latest_entry.window_id.clone(),
                    cli_session_id: latest_entry.effective_session_id().map(str::to_owned),
                    event: final_state.as_str().to_owned(),
                    state: final_state,
                    summary: cls2.summary,
                    last_message,
                    approval,
                    error,
                    tail,
                });
            }
            inner.subagent_events.notify_waiters();
        });
    }

    async fn agent_start(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>, block_id: Option<String>) -> bridge::BridgeResponse {
        let err_resp =
            |msg: String, code: i32| bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent start: {msg}\n")), data: Some(json!({ "code": code })), carried: Vec::new() };
        let spec = match crate::subagents::StartSpec::parse(raw) {
            Ok(s) => s,
            Err(e) => return err_resp(e, 2),
        };
        let (cli, model, effort, raw_argv) = match spec.build_command() {
            Ok(v) => v,
            Err(e) => return err_resp(e, 2),
        };
        let (ctrl, ep, sid, parent_win, parent_cwd) = match self.resolve_session_and_parent(host_key, session_id.as_deref(), block_id.as_deref(), spec.parent_window.as_deref()) {
            Ok(v) => v,
            Err(e) => return err_resp(e, 1),
        };

        let before_ids: HashSet<String> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let live: HashSet<String> = st.hosts.get(host_key).and_then(|h| h.sessions.get(&sid)).map(|s| s.windows.iter().map(|w| w.window_id.clone()).collect()).unwrap_or_default();
            if let Some(ref name) = spec.name
                && st.subagents.is_live_name_taken(host_key, &sid, &parent_win, name, &live)
            {
                return err_resp(format!("a live child named `{name}` already exists under this parent (choose a unique `--name` or stop it first)"), 2);
            }
            live
        };

        let cwd = spec.cwd.clone().or(parent_cwd);
        let title = spec.effective_title(cli);

        // Item #1: Pre-trust workspace & `--add-dir` paths atomically in `~/.claude.json` BEFORE spawning Claude!
        if spec.trust && cli == crate::subagents::AgentCli::Claude {
            let mut trust_paths: Vec<String> = Vec::new();
            if let Some(ref d) = cwd {
                trust_paths.push(d.clone());
            }
            for d in &spec.add_dirs {
                if !trust_paths.iter().any(|p| p == d) {
                    trust_paths.push(d.clone());
                }
            }
            if !trust_paths.is_empty() {
                let mut link_args: Vec<&str> = Vec::with_capacity(2 + trust_paths.len());
                link_args.push("agent");
                link_args.push("trust");
                for p in &trust_paths {
                    link_args.push(p.as_str());
                }
                let _ = crate::activity_watch::run_host_link(&self.state, host_key, &link_args, None).await;
            }
        }

        // Item #11: Wrap argv with `env -u ...` for desktop isolation & `--unset-env`.
        let argv = spec.wrap_with_env_isolation(raw_argv.clone());

        // Build Rex shell options matching local/remote environment + desktop policy
        let mut opts = json!({
            "command": argv,
            "interactive": true,
            "exit": { "on_completion": spec.close_on_exit },
        });
        if let Some(ref dir) = cwd {
            opts["cwd"] = json!(dir);
        }
        let mut env_map = serde_json::Map::new();
        {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let is_local = st.hosts.get(host_key).is_some_and(|h| h.is_local);
            let inherit_desktop = matches!(spec.desktop, crate::subagents::DesktopPolicy::Inherit);
            if is_local {
                if inherit_desktop {
                    if let Some(bin) = &st.local_bin_dir {
                        opts["path_prepend"] = json!([bin.to_string_lossy()]);
                    }
                    if let Some(open_bin) = olo_xdg::find_program("olo-open") {
                        env_map.insert("BROWSER".into(), json!(open_bin.to_string_lossy()));
                    }
                }
            } else if let Some(agent) = self.agent_socks.lock().unwrap_or_else(|e| e.into_inner()).get(host_key) {
                env_map.insert("SSH_AUTH_SOCK".into(), json!(agent));
            }
        }
        for (k, v) in &spec.env {
            env_map.insert(k.clone(), json!(v));
        }
        if !env_map.is_empty() {
            opts["env"] = Value::Object(env_map);
        }

        let created = match ctrl.session_new_window_with_focus(&sid, Some(&title), Some(opts), spec.focus).await {
            Ok(r) => r.payload,
            Err(e) => return err_resp(format!("failed to create Rex tab: {e}"), 1),
        };
        let view = match ctrl.session_view(&sid).await {
            Ok(v) => v.payload,
            Err(e) => return err_resp(format!("failed to read session view: {e}"), 1),
        };

        let explicit_wid = created.get("window_id").and_then(Value::as_str).map(str::to_owned);
        let new_win =
            explicit_wid.as_deref().and_then(|wid| view.windows.iter().find(|w| w.window_id == wid)).or_else(|| view.windows.iter().rev().find(|w| !before_ids.contains(&w.window_id))).cloned();
        let Some(new_win) = new_win else {
            return err_resp("Rex created the tab but its window_id could not be identified".into(), 1);
        };
        let new_bid = new_win.focused_block_id.clone().or_else(|| new_win.all_blocks().next().map(|b| b.block_id.clone())).unwrap_or_default();

        let entry = crate::subagents::NativeSubagent {
            id: crate::subagents::new_subagent_id(),
            name: spec.name.clone(),
            host_key: host_key.to_owned(),
            session_id: sid.clone(),
            window_id: new_win.window_id.clone(),
            block_id: new_bid.clone(),
            parent_window_id: parent_win.clone(),
            parent_block_id: block_id,
            cli,
            model: model.clone(),
            effort: effort.clone(),
            title: title.clone(),
            role: spec.role.clone(),
            cwd: cwd.clone(),
            resume_id: spec.resume.clone(),
            cli_session_id: spec.session_id.clone().or_else(|| spec.resume.clone()),
            transcript_path: None,
            desktop_policy: spec.desktop.clone(),
            stream_mode: spec.print_mode,
            started_at: crate::subagents::now_unix(),
            exited_at: None,
            last_pid: None,
        };
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.subagents.register(entry.clone());
            st.subagents.save();
        }
        self.ingest_session(host_key, &ctrl, &ep, view, &[]).await;

        // Fallback event-driven folder-trust confirmation (max 15s, zero polling) in case Claude
        // still renders a first-run workspace safety prompt (e.g. symlinked path / fresh config).
        if spec.trust && cli == crate::subagents::AgentCli::Claude {
            let (inner, ctrl_w, ent_w) = (self.clone(), ctrl.clone(), entry.clone());
            tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                let mut attempts = 0u8;
                loop {
                    let notified = inner.subagent_events.notified();
                    let (cls, _, _, _, _) = inner.live_subagent_rich(&ent_w);
                    if cls.state == crate::subagents::SubagentLiveState::Exited {
                        break;
                    }
                    if let Some(trust) = cls.folder_trust {
                        if attempts >= 3 {
                            break;
                        }
                        attempts += 1;
                        if trust.selected_trust {
                            let _ = ctrl_w.terminal_write(&ent_w.session_id, &ent_w.block_id, "\r").await;
                        } else {
                            let _ = ctrl_w.terminal_write(&ent_w.session_id, &ent_w.block_id, "\x1b[B").await;
                            tokio::time::sleep(Duration::from_millis(60)).await;
                            let _ = ctrl_w.terminal_write(&ent_w.session_id, &ent_w.block_id, "\r").await;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        continue;
                    }
                    if attempts > 0 {
                        // Trust prompt dismissed cleanly!
                        break;
                    }
                    tokio::select! {
                        _ = notified => {}
                        _ = tokio::time::sleep_until(deadline) => break,
                    }
                }
            });
        }

        let cursor = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner()).latest_seq();
        let payload = json!({
            "ok": true,
            "id": entry.id,
            "name": entry.name,
            "window_id": entry.window_id,
            "block_id": entry.block_id,
            "parent_window_id": entry.parent_window_id,
            "cli": cli.binary(),
            "model": model,
            "effort": effort,
            "title": title,
            "role": entry.role,
            "cwd": cwd,
            "cli_session_id": entry.effective_session_id(),
            "desktop": entry.desktop_policy.describe(),
            "cursor": cursor,
            "command": raw_argv,
        });
        let stdout = if spec.json { format!("{payload}\n") } else { format!("{}  {} ({}, effort={})  tab={}  {}\n", entry.id, cli.binary(), model, effort, entry.window_id, title) };
        bridge::BridgeResponse { ok: true, stdout: Some(stdout), stderr: None, data: Some(payload), carried: Vec::new() }
    }

    async fn agent_list(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>, block_id: Option<String>) -> bridge::BridgeResponse {
        let mut all = false;
        let mut fast = false;
        let mut as_json = false;
        let mut explicit_parent: Option<String> = None;
        if let Some(v) = raw {
            if let Some(arr) = v.get("args").and_then(Value::as_array) {
                let strs: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
                let mut i = 0;
                while i < strs.len() {
                    match strs[i].as_str() {
                        "--all" | "-a" => all = true,
                        "--fast" => fast = true,
                        "--json" => as_json = true,
                        "--parent" => {
                            i += 1;
                            if let Some(p) = strs.get(i) {
                                explicit_parent = Some(p.clone());
                            }
                        }
                        s if s.starts_with("--parent=") => explicit_parent = Some(s["--parent=".len()..].to_owned()),
                        _ => {}
                    }
                    i += 1;
                }
            } else {
                all = v.get("all").and_then(Value::as_bool).unwrap_or(false);
                fast = v.get("fast").and_then(Value::as_bool).unwrap_or(false);
                as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
                explicit_parent = v.get("parent").and_then(Value::as_str).map(str::to_owned);
            }
        }
        let (_ctrl, _ep, sid, caller_win, _) = match self.resolve_session_and_parent(host_key, session_id.as_deref(), block_id.as_deref(), explicit_parent.as_deref()) {
            Ok(v) => v,
            Err(e) => return bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent list: {e}\n")), data: Some(json!({ "code": 1 })), carried: Vec::new() },
        };

        let (mut rows, exited, live_windows) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let live: HashSet<String> = st.hosts.get(host_key).and_then(|h| h.sessions.get(&sid)).map(|s| s.windows.iter().map(|w| w.window_id.clone()).collect()).unwrap_or_default();
            let rows = if all && explicit_parent.is_none() {
                let mut collected = Vec::new();
                if let Some(sess) = st.hosts.get(host_key).and_then(|h| h.sessions.get(&sid)) {
                    for w in &sess.windows {
                        if !st.subagents.is_child_window(host_key, &sid, &w.window_id, &live) {
                            collected.extend(st.subagents.descendants_tree(host_key, &sid, &w.window_id, &live));
                        }
                    }
                }
                collected
            } else {
                st.subagents.descendants_tree(host_key, &sid, &caller_win, &live)
            };
            let exited = if all {
                let mut ex = st.subagents.exited_entries(host_key, Some(&sid), explicit_parent.as_ref().map(|_| caller_win.as_str()));
                if ex.is_empty() && explicit_parent.is_none() {
                    // Also surface exited children from a previous Rex server session on the same host (Item #9).
                    ex = st.subagents.exited_entries(host_key, None, None);
                }
                ex
            } else {
                Vec::new()
            };
            (rows, exited, live)
        };

        // Single-pass batched host inspection for all live children (Item #6 & #10: 1 round-trip for 40 children!).
        if !fast && !rows.is_empty() {
            let live_entries: Vec<crate::subagents::NativeSubagent> = rows.iter().map(|r| r.entry.clone()).collect();
            let _ = self.inspect_subagents_on_host(host_key, &live_entries).await;
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for row in &mut rows {
                if let Some(updated) = st.subagents.by_window(host_key, &sid, &row.entry.window_id) {
                    row.entry = updated.clone();
                }
            }
        }

        let mut items = Vec::with_capacity(rows.len() + exited.len());
        let mut text = String::new();
        for row in &rows {
            let (cls, _, _, _, _) = self.live_subagent_rich(&row.entry);
            let kids = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                st.subagents.direct_children(host_key, &sid, &row.entry.window_id, &live_windows).len()
            };
            items.push(crate::subagents::summary_json(&row.entry, cls.state, cls.summary.as_deref(), row.depth, kids));
            let tail = cls.summary.as_deref().map(|s| format!(" — {s}")).unwrap_or_default();
            let name_col = row.entry.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default();
            text.push_str(&format!(
                "{}{}{}  {}  {} ({}, effort={})  [{}]{}\n",
                row.menu_prefix(),
                row.entry.id,
                name_col,
                row.entry.display_title(),
                row.entry.cli.binary(),
                row.entry.model,
                row.entry.effort,
                cls.state.as_str(),
                tail
            ));
        }
        for ex in &exited {
            items.push(crate::subagents::summary_json(ex, crate::subagents::SubagentLiveState::Exited, Some("exited"), 1, 0));
            let sid_note = ex.effective_session_id().map(|s| format!("  session={s}")).unwrap_or_default();
            let name_col = ex.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default();
            text.push_str(&format!("   {}{}  {}  {} ({}, effort={})  [exited]{}\n", ex.id, name_col, ex.display_title(), ex.cli.binary(), ex.model, ex.effort, sid_note));
        }
        let cursor = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner()).latest_seq();
        let payload = json!({ "ok": true, "session_id": sid, "parent_window_id": caller_win, "cursor": cursor, "subagents": items });
        let stdout = if as_json {
            format!("{payload}\n")
        } else if text.is_empty() {
            "No child subagent tabs.\n".to_owned()
        } else {
            text
        };
        bridge::BridgeResponse { ok: true, stdout: Some(stdout), stderr: None, data: Some(payload), carried: Vec::new() }
    }

    fn resolve_agent_target(&self, host_key: &str, session_id: Option<&str>, target: &str) -> Result<crate::subagents::NativeSubagent, String> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.subagents
            .resolve_target(host_key, session_id, target)
            .or_else(|| st.subagents.resolve_target(host_key, None, target))
            .ok_or_else(|| format!("subagent `{target}` not found (run `olo agent list --all` to see active and exited child tabs)"))
    }

    async fn agent_status(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> bridge::BridgeResponse {
        let mut target: Option<String> = None;
        let mut as_json = false;
        if let Some(v) = raw {
            if let Some(arr) = v.get("args").and_then(Value::as_array) {
                for a in arr.iter().filter_map(Value::as_str) {
                    if a == "--json" {
                        as_json = true;
                    } else if !a.starts_with('-') && target.is_none() {
                        target = Some(a.to_owned());
                    }
                }
            } else {
                target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
                as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
            }
        }
        let Some(target) = target else {
            return bridge::BridgeResponse { ok: false, stdout: None, stderr: Some("usage: olo agent status <ID|NAME> [--json]\n".into()), data: Some(json!({ "code": 2 })), carried: Vec::new() };
        };
        let entry = match self.resolve_agent_target(host_key, session_id.as_deref(), &target) {
            Ok(e) => e,
            Err(e) => return bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent status: {e}\n")), data: Some(json!({ "code": 1 })), carried: Vec::new() },
        };
        let inspected = self.inspect_subagents_on_host(host_key, std::slice::from_ref(&entry)).await;
        let info = inspected.get(&entry.id);
        let entry = self.resolve_agent_target(host_key, Some(&entry.session_id), &entry.id).unwrap_or(entry);
        let (cls, screen, last_output_at, last_state_change_at, pid) = self.live_subagent_rich(&entry);
        let now_ms = crate::subagents::now_unix_ms();
        let quiet_secs = if last_output_at > 0 { now_ms.saturating_sub(last_output_at) / 1000 } else { 0 };
        let state_age_secs = if last_state_change_at > 0 { now_ms.saturating_sub(last_state_change_at) / 1000 } else { 0 };

        let last_message = info.and_then(|v| v.get("last_message")).and_then(Value::as_str).map(str::to_owned);
        let tx_state = info.and_then(|v| v.get("transcript_state")).and_then(Value::as_str);
        let tx_error = info.and_then(|v| v.get("transcript_error")).and_then(Value::as_str).map(str::to_owned);
        let pending_tool = info.and_then(|v| v.get("pending_tool"));
        let bg_shells = info.and_then(|v| v.get("background_shells")).cloned().unwrap_or_else(|| json!([]));
        let waiting_on = info.and_then(|v| v.get("waiting_on")).and_then(Value::as_str).map(str::to_owned);
        let error = cls.api_error.clone().or(tx_error);
        let effective_state = if error.is_some() && !matches!(cls.state, crate::subagents::SubagentLiveState::Exited | crate::subagents::SubagentLiveState::WaitingApproval) {
            crate::subagents::SubagentLiveState::Error
        } else if tx_state == Some("end_turn") && cls.state == crate::subagents::SubagentLiveState::Idle {
            crate::subagents::SubagentLiveState::Done
        } else {
            cls.state
        };
        let approval_val = cls.approval.as_ref().map(|ap| crate::subagents::approval_to_json(ap, pending_tool));
        let usage_val = cls.usage.as_ref().map(|u| json!({ "line": u.line, "percent": u.percent, "limit_reached": u.limit_reached }));
        let tail: Vec<&str> = screen.lines().rev().filter(|l| !l.trim().is_empty()).take(8).collect::<Vec<_>>().into_iter().rev().collect();
        let cursor = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner()).latest_seq();

        let mut payload = crate::subagents::summary_json(&entry, effective_state, cls.summary.as_deref(), 1, 0);
        if let Some(p) = pid {
            payload["pid"] = json!(p);
        }
        payload["last_message"] = json!(last_message);
        payload["approval"] = json!(approval_val);
        payload["error"] = json!(error);
        payload["last_output_at"] = json!(last_output_at);
        payload["last_state_change_at"] = json!(last_state_change_at);
        payload["quiet_secs"] = json!(quiet_secs);
        payload["state_age_secs"] = json!(state_age_secs);
        payload["background_shells"] = bg_shells;
        payload["waiting_on"] = json!(waiting_on);
        payload["usage"] = json!(usage_val);
        payload["cursor"] = json!(cursor);
        payload["tail"] = json!(tail);

        let stdout = if as_json {
            format!("{payload}\n")
        } else {
            let mut s = format!("{}  {}  {} ({}, effort={})  state={}\n", entry.id, entry.display_title(), entry.cli.binary(), entry.model, entry.effort, effective_state.as_str());
            if let Some(sid) = entry.effective_session_id() {
                s.push_str(&format!("session: {sid}\n"));
            }
            if let Some(ref sum) = cls.summary {
                s.push_str(&format!("summary: {sum}\n"));
            }
            if let Some(ref err) = error {
                s.push_str(&format!("error: {err}\n"));
            }
            if let Some(ref msg) = last_message {
                s.push_str(&format!("last_message:\n{msg}\n"));
            }
            for l in &tail {
                s.push_str("  ");
                s.push_str(l);
                s.push('\n');
            }
            s
        };
        bridge::BridgeResponse { ok: true, stdout: Some(stdout), stderr: None, data: Some(payload), carried: Vec::new() }
    }

    async fn agent_read(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> Value {
        let mut target: Option<String> = None;
        let mut lines: usize = 80;
        let mut scrollback = false;
        let mut ansi = false;
        let mut as_json = false;
        if let Some(v) = raw {
            if let Some(arr) = v.get("args").and_then(Value::as_array) {
                let strs: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
                let mut i = 0;
                while i < strs.len() {
                    match strs[i].as_str() {
                        "--scrollback" | "--all" => scrollback = true,
                        "--ansi" => ansi = true,
                        "--json" => as_json = true,
                        "-n" | "--lines" => {
                            i += 1;
                            if let Some(n) = strs.get(i).and_then(|s| s.parse::<usize>().ok()) {
                                lines = n.clamp(1, 5000);
                            }
                        }
                        s if s.starts_with("--lines=") => {
                            if let Ok(n) = s["--lines=".len()..].parse::<usize>() {
                                lines = n.clamp(1, 5000);
                            }
                        }
                        s if !s.starts_with('-') && target.is_none() => target = Some(s.to_owned()),
                        _ => {}
                    }
                    i += 1;
                }
            } else {
                target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
                lines = v.get("lines").and_then(Value::as_u64).unwrap_or(80).clamp(1, 5000) as usize;
                scrollback = v.get("scrollback").and_then(Value::as_bool).unwrap_or(false);
                ansi = v.get("ansi").and_then(Value::as_bool).unwrap_or(false);
                as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
            }
        }
        let Some(target) = target else {
            return json!({ "ok": false, "stderr": "usage: olo agent read <ID|NAME> [--lines N] [--scrollback] [--ansi] [--json]\n", "data": { "code": 2 } });
        };
        let entry = match self.resolve_agent_target(host_key, session_id.as_deref(), &target) {
            Ok(e) => e,
            Err(e) => return json!({ "ok": false, "stderr": format!("olo agent read: {e}\n"), "data": { "code": 1 } }),
        };
        let ctrl = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| h.control.clone())
        };
        let mut text = if scrollback
            && let Some(ctrl) = ctrl
            && let Ok(r) = ctrl.terminal_format(&entry.session_id, &entry.block_id, if ansi { "vt" } else { "plain" }).await
        {
            r.payload
        } else {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let bk = BlockKey { host_key: entry.host_key.clone(), session_id: entry.session_id.clone(), block_id: entry.block_id.clone() };
            let kind = if ansi { olo_rex_vt::FormatKind::Vt } else { olo_rex_vt::FormatKind::Plain };
            st.blocks.get(&bk).and_then(|br| br.lock().unwrap_or_else(|e| e.into_inner()).terminal.format(kind, scrollback, true).ok()).unwrap_or_default()
        };
        let all_lines: Vec<&str> = text.lines().collect();
        if all_lines.len() > lines {
            text = all_lines[all_lines.len() - lines..].join("\n");
        }
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        let (cls, _, _, _, _) = self.live_subagent_rich(&entry);
        let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "state": cls.state.as_str(), "summary": cls.summary, "output": text });
        let stdout = if as_json { format!("{payload}\n") } else { text };
        json!({ "ok": true, "stdout": stdout, "data": payload })
    }

    async fn agent_approve(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> bridge::BridgeResponse {
        let err_resp =
            |msg: String, code: i32| bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent approve: {msg}\n")), data: Some(json!({ "code": code })), carried: Vec::new() };
        let spec = match crate::subagents::ApproveSpec::parse(raw) {
            Ok(s) => s,
            Err(e) => return err_resp(e, 2),
        };
        let entry = match self.resolve_agent_target(host_key, session_id.as_deref(), &spec.target) {
            Ok(e) => e,
            Err(e) => return err_resp(e, 1),
        };
        let ctrl = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| h.control.clone())
        };
        let Some(ctrl) = ctrl else {
            return err_resp("Rex host is not connected".into(), 69);
        };
        let (cls, _, _, _, _) = self.live_subagent_rich(&entry);
        let Some(ap) = cls.approval else {
            return err_resp(format!("`{}` has no active approval prompt (current state: {})", entry.id, cls.state.as_str()), 1);
        };
        let want = if spec.once { olo_agent_screen::ChoiceKind::Once } else { olo_agent_screen::ChoiceKind::No };
        let Some(choice) = ap.choices.iter().find(|c| c.kind == want) else {
            let avail: Vec<String> = ap.choices.iter().map(|c| format!("{} [{}]", c.label, c.kind.as_str())).collect();
            return err_resp(format!("no safe `{}` choice found in active dialog (choices: {})", want.as_str(), avail.join("; ")), 1);
        };
        let current_idx = ap.selected_index.unwrap_or(0);
        let delta = choice.index as isize - current_idx as isize;
        let step_key = if delta > 0 { "\x1b[B" } else { "\x1b[A" };
        for _ in 0..delta.unsigned_abs() {
            if let Err(e) = ctrl.terminal_write(&entry.session_id, &entry.block_id, step_key).await {
                return err_resp(format!("failed to navigate approval dialog: {e}"), 1);
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        if delta != 0 {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        if let Err(e) = ctrl.terminal_write(&entry.session_id, &entry.block_id, "\r").await {
            return err_resp(format!("failed to confirm approval dialog: {e}"), 1);
        }
        if !spec.once
            && let Some(ref reason) = spec.reason
            && !reason.trim().is_empty()
        {
            tokio::time::sleep(Duration::from_millis(140)).await;
            let _ = ctrl.terminal_write(&entry.session_id, &entry.block_id, reason).await;
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _ = ctrl.terminal_write(&entry.session_id, &entry.block_id, "\r").await;
        }
        // Verify event-driven that the approval dialog dismissed within 2s (retrying Enter once at 700ms).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let retry_at = tokio::time::Instant::now() + Duration::from_millis(700);
        let mut retried = false;
        let mut dismissed = false;
        loop {
            let notified = self.subagent_events.notified();
            let (now_cls, _, _, _, _) = self.live_subagent_rich(&entry);
            if now_cls.approval.is_none() {
                dismissed = true;
                break;
            }
            let now_t = tokio::time::Instant::now();
            if now_t >= deadline {
                break;
            }
            let next_wake = if !retried && retry_at < deadline { retry_at } else { deadline };
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep_until(next_wake) => {
                    if !retried && tokio::time::Instant::now() >= retry_at {
                        retried = true;
                        let (chk, _, _, _, _) = self.live_subagent_rich(&entry);
                        if chk.approval.as_ref().is_some_and(|cur| cur.selected_index == Some(choice.index)) {
                            let _ = ctrl.terminal_write(&entry.session_id, &entry.block_id, "\r").await;
                        }
                    }
                }
            }
        }
        let payload = json!({
            "ok": dismissed,
            "id": entry.id,
            "name": entry.name,
            "chosen_label": choice.label,
            "chosen_kind": choice.kind.as_str(),
            "dismissed": dismissed,
        });
        let stdout = if spec.json {
            format!("{payload}\n")
        } else if dismissed {
            format!("{}  approved ({}) — {}\n", entry.id, choice.kind.as_str(), choice.label)
        } else {
            format!("{}  sent ({}) but dialog did not dismiss within 2s\n", entry.id, choice.kind.as_str())
        };
        bridge::BridgeResponse {
            ok: dismissed,
            stdout: Some(stdout),
            stderr: (!dismissed).then(|| format!("olo agent approve: dialog on `{}` did not dismiss within 2s\n", entry.id)),
            data: Some(payload),
            carried: Vec::new(),
        }
    }

    async fn agent_watch(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>, block_id: Option<String>) -> bridge::BridgeResponse {
        let spec = match crate::subagents::WatchSpec::parse(raw) {
            Ok(s) => s,
            Err(e) => return bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent watch: {e}\n")), data: Some(json!({ "code": 2 })), carried: Vec::new() },
        };
        let compiled_re = match &spec.regex_pat {
            Some(pat) => match regex::Regex::new(pat) {
                Ok(r) => Some(r),
                Err(e) => {
                    return bridge::BridgeResponse {
                        ok: false,
                        stdout: None,
                        stderr: Some(format!("olo agent watch: invalid --regex `{pat}`: {e}\n")),
                        data: Some(json!({ "code": 2 })),
                        carried: Vec::new(),
                    };
                }
            },
            None => None,
        };

        let (scope_sid, scope_parent, explicit_entries): (Option<String>, Option<String>, Vec<crate::subagents::NativeSubagent>) = if spec.all {
            match self.resolve_session_and_parent(host_key, session_id.as_deref(), block_id.as_deref(), spec.parent.as_deref()) {
                Ok((_, _, sid, parent_win, _)) => (Some(sid), Some(parent_win), Vec::new()),
                Err(e) => return bridge::BridgeResponse::err(format!("olo agent watch: {e}\n")),
            }
        } else {
            let mut entries = Vec::with_capacity(spec.targets.len());
            for t in &spec.targets {
                match self.resolve_agent_target(host_key, session_id.as_deref(), t) {
                    Ok(e) => entries.push(e),
                    Err(e) => return bridge::BridgeResponse::err(format!("olo agent watch: {e}\n")),
                }
            }
            (session_id.clone(), None, entries)
        };
        let explicit_ids: HashSet<String> = explicit_entries.iter().map(|e| e.id.clone()).collect();

        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(spec.timeout_secs);
        let since_floor = spec.since.unwrap_or_else(|| self.subagent_log.lock().unwrap_or_else(|e| e.into_inner()).latest_seq());

        let format_event_resp = |ev: &crate::subagents::SubagentEvent, matched_line: Option<String>| {
            let payload = json!({
                "ok": true,
                "cursor": ev.seq,
                "id": ev.id,
                "name": ev.name,
                "window_id": ev.window_id,
                "cli_session_id": ev.cli_session_id,
                "event": ev.event,
                "state": ev.state.as_str(),
                "summary": ev.summary,
                "last_message": ev.last_message,
                "approval": ev.approval,
                "error": ev.error,
                "matched_line": matched_line,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "tail": ev.tail,
            });
            let stdout = if spec.json {
                format!("{payload}\n")
            } else {
                let detail = matched_line.or_else(|| ev.summary.clone()).unwrap_or_default();
                format!("{}  event={}  state={}  cursor={}  {}\n", ev.id, ev.event, ev.state.as_str(), ev.seq, detail)
            };
            bridge::BridgeResponse { ok: true, stdout: Some(stdout), stderr: None, data: Some(payload), carried: Vec::new() }
        };

        let active_entries_in_scope = || -> Vec<crate::subagents::NativeSubagent> {
            if spec.all {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let (Some(sid), Some(pwin)) = (scope_sid.as_deref(), scope_parent.as_deref()) else {
                    return Vec::new();
                };
                let live: HashSet<String> = st.hosts.get(host_key).and_then(|h| h.sessions.get(sid)).map(|s| s.windows.iter().map(|w| w.window_id.clone()).collect()).unwrap_or_default();
                st.subagents.descendants_tree(host_key, sid, pwin, &live).into_iter().map(|r| r.entry).collect()
            } else {
                explicit_entries.clone()
            }
        };

        let event_in_scope = |ev: &crate::subagents::SubagentEvent, live_ids: &HashSet<String>| -> bool {
            if ev.host_key != host_key {
                return false;
            }
            if !spec.all {
                return explicit_ids.contains(&ev.id);
            }
            if let Some(sid) = scope_sid.as_deref()
                && ev.session_id != sid
            {
                return false;
            }
            live_ids.contains(&ev.id) || scope_parent.as_deref() == Some(ev.parent_window_id.as_str())
        };

        loop {
            let notified = self.subagent_events.notified();
            let current_entries = active_entries_in_scope();
            let live_ids: HashSet<String> = current_entries.iter().map(|e| e.id.clone()).collect();

            // 1. Check monotonic event log strictly after `since_floor` (zero missed events between consecutive watches!).
            let queued = {
                let log = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner());
                log.find_since(since_floor, |ev| event_in_scope(ev, &live_ids) && spec.matches_state(ev.state))
            };
            if let Some(ev) = queued {
                return format_event_resp(&ev, None);
            }

            // 2. Check live screens: always for `--match`/`--regex`, and for state predicates when `--since` wasn't supplied.
            let check_live_state = spec.since.is_none();
            let check_text = spec.match_text.is_some() || compiled_re.is_some();
            if check_live_state || check_text {
                let mut candidate: Option<(crate::subagents::NativeSubagent, crate::subagents::ScreenClassification, String, Option<String>)> = None;
                for entry in &current_entries {
                    let (cls, screen, _, _, _) = self.live_subagent_rich(entry);
                    if let Some((ev_name, matched_line)) = spec.check(cls.state, &screen, compiled_re.as_ref()) {
                        let is_text_match = ev_name == "match" || ev_name == "regex";
                        if is_text_match || check_live_state {
                            candidate = Some((entry.clone(), cls, ev_name, matched_line));
                            break;
                        }
                    }
                }
                if let Some((entry, cls1, ev_name, matched_line)) = candidate {
                    // Stability confirmation (250 ms) for non-exited state matches to eliminate transient repaint glitches (Item #4).
                    let confirmed = if cls1.state == crate::subagents::SubagentLiveState::Exited || ev_name == "match" || ev_name == "regex" {
                        true
                    } else {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        let (cls2, screen2, _, _, _) = self.live_subagent_rich(&entry);
                        spec.check(cls2.state, &screen2, compiled_re.as_ref()).is_some_and(|(e2, _)| e2 == ev_name)
                    };
                    if confirmed {
                        let inspected = self.inspect_subagents_on_host(host_key, std::slice::from_ref(&entry)).await;
                        let info = inspected.get(&entry.id);
                        let latest_entry = self.resolve_agent_target(host_key, Some(&entry.session_id), &entry.id).unwrap_or(entry);
                        let (cls_final, screen_final, _, _, _) = self.live_subagent_rich(&latest_entry);
                        let last_message = info.and_then(|v| v.get("last_message")).and_then(Value::as_str).map(str::to_owned);
                        let tx_err = info.and_then(|v| v.get("transcript_error")).and_then(Value::as_str).map(str::to_owned);
                        let pending_tool = info.and_then(|v| v.get("pending_tool"));
                        let approval = cls_final.approval.as_ref().map(|ap| crate::subagents::approval_to_json(ap, pending_tool));
                        let error = cls_final.api_error.clone().or(tx_err);
                        let tail: Vec<String> = screen_final.lines().rev().filter(|l| !l.trim().is_empty()).take(10).map(str::to_owned).collect::<Vec<_>>().into_iter().rev().collect();
                        let mut ev = crate::subagents::SubagentEvent {
                            seq: 0,
                            timestamp_ms: crate::subagents::now_unix_ms(),
                            host_key: latest_entry.host_key.clone(),
                            session_id: latest_entry.session_id.clone(),
                            parent_window_id: latest_entry.parent_window_id.clone(),
                            id: latest_entry.id.clone(),
                            name: latest_entry.name.clone(),
                            window_id: latest_entry.window_id.clone(),
                            cli_session_id: latest_entry.effective_session_id().map(str::to_owned),
                            event: ev_name,
                            state: cls_final.state,
                            summary: cls_final.summary,
                            last_message,
                            approval,
                            error,
                            tail,
                        };
                        let seq = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner()).push(ev.clone());
                        ev.seq = seq;
                        return format_event_resp(&ev, matched_line);
                    }
                }
            }

            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep_until(deadline) => {
                    let cursor = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner()).latest_seq();
                    let payload = json!({
                        "ok": false,
                        "cursor": cursor,
                        "event": "timeout",
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                        "code": 124,
                    });
                    let stdout = if spec.json { Some(format!("{payload}\n")) } else { None };
                    return bridge::BridgeResponse {
                        ok: false,
                        stdout,
                        stderr: Some(format!("olo agent watch: timed out after {}s\n", spec.timeout_secs)),
                        data: Some(payload),
                        carried: Vec::new(),
                    };
                }
            }
        }
    }

    async fn agent_send(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> bridge::BridgeResponse {
        let err_resp =
            |msg: String, code: i32| bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent send: {msg}\n")), data: Some(json!({ "code": code })), carried: Vec::new() };
        let cli_args: Vec<String> =
            raw.and_then(|v| v.get("args")).and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect()).unwrap_or_default();
        let spec = match crate::subagents::parse_send_payload(&cli_args, raw) {
            Ok(v) => v,
            Err(e) => return err_resp(e, 2),
        };
        let entry = match self.resolve_agent_target(host_key, session_id.as_deref(), &spec.target) {
            Ok(e) => e,
            Err(e) => return err_resp(e, 1),
        };
        let ctrl = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| h.control.clone())
        };
        let Some(ctrl) = ctrl else {
            return err_resp("Rex host is not connected".into(), 69);
        };

        let (_, before_screen, before_out_at, _, _) = self.live_subagent_rich(&entry);
        let mut total_bytes = 0usize;

        if !spec.body.is_empty() {
            let chunk = if spec.paste { format!("\x1b[200~{}\x1b[201~", spec.body) } else { spec.body.clone() };
            total_bytes += chunk.len();
            if let Err(e) = ctrl.terminal_write(&entry.session_id, &entry.block_id, &chunk).await {
                return err_resp(format!("{e}"), 1);
            }
            if spec.enter {
                // Separate pasted body from trailing Enter so TUIs never swallow `\r` into the paste buffer (Item #13).
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        }
        if spec.enter {
            total_bytes += 1;
            if let Err(e) = ctrl.terminal_write(&entry.session_id, &entry.block_id, "\r").await {
                return err_resp(format!("{e}"), 1);
            }
        }
        if !spec.post_keys.is_empty() {
            total_bytes += spec.post_keys.len();
            if let Err(e) = ctrl.terminal_write(&entry.session_id, &entry.block_id, &spec.post_keys).await {
                return err_resp(format!("{e}"), 1);
            }
        }

        // Event-driven delivery verification for submitted pastes (up to 3s, 1 safe Enter retry at 800ms).
        let mut delivered = true;
        if spec.verify_delivery {
            delivered = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let retry_at = tokio::time::Instant::now() + Duration::from_millis(800);
            let mut retried = false;
            let first_line = spec.body.lines().next().map(str::trim).filter(|s| s.len() >= 4);
            loop {
                let notified = self.subagent_events.notified();
                let (cls, screen, out_at, _, _) = self.live_subagent_rich(&entry);
                let still_in_input = first_line.is_some_and(|fl| {
                    screen.lines().rev().take(6).any(|l| {
                        let t = l.trim();
                        (t.starts_with('❯') || t.starts_with('>') || t.starts_with('›')) && t.contains(fl)
                    })
                });
                if cls.state == crate::subagents::SubagentLiveState::Working || (out_at > before_out_at && screen != before_screen && !still_in_input) {
                    delivered = true;
                    break;
                }
                let now_t = tokio::time::Instant::now();
                if now_t >= deadline {
                    break;
                }
                let wake = if !retried && retry_at < deadline { retry_at } else { deadline };
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep_until(wake) => {
                        if !retried && tokio::time::Instant::now() >= retry_at {
                            retried = true;
                            let _ = ctrl.terminal_write(&entry.session_id, &entry.block_id, "\r").await;
                        }
                    }
                }
            }
        }

        let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "bytes": total_bytes, "delivered": delivered });
        let stdout = if spec.as_json { format!("{payload}\n") } else { format!("Sent {} bytes to {} (delivered={}).\n", total_bytes, entry.id, delivered) };
        bridge::BridgeResponse { ok: true, stdout: Some(stdout), stderr: None, data: Some(payload), carried: Vec::new() }
    }

    async fn agent_stop(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> bridge::BridgeResponse {
        let mut target: Option<String> = None;
        let mut force = false;
        let mut tree = false;
        let mut keep_tab = false;
        let mut as_json = false;
        let mut save_path: Option<String> = None;
        if let Some(v) = raw {
            if let Some(arr) = v.get("args").and_then(Value::as_array) {
                let strs: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
                let mut i = 0;
                while i < strs.len() {
                    match strs[i].as_str() {
                        "--force" | "-f" => force = true,
                        "--tree" | "--recursive" | "-r" => tree = true,
                        "--keep-tab" => keep_tab = true,
                        "--json" => as_json = true,
                        "--save" => {
                            i += 1;
                            if let Some(p) = strs.get(i) {
                                save_path = Some(p.clone());
                            }
                        }
                        s if s.starts_with("--save=") => save_path = Some(s["--save=".len()..].to_owned()),
                        s if !s.starts_with('-') && target.is_none() => target = Some(s.to_owned()),
                        _ => {}
                    }
                    i += 1;
                }
            } else {
                target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
                force = v.get("force").and_then(Value::as_bool).unwrap_or(false);
                tree = v.get("tree").and_then(Value::as_bool).unwrap_or(false);
                keep_tab = v.get("keep_tab").and_then(Value::as_bool).unwrap_or(false);
                as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
                save_path = v.get("save").and_then(Value::as_str).map(str::to_owned);
            }
        }
        let Some(target) = target else {
            return bridge::BridgeResponse {
                ok: false,
                stdout: None,
                stderr: Some("usage: olo agent stop <ID|NAME> [--save PATH] [--force] [--tree] [--keep-tab] [--json]\n".into()),
                data: Some(json!({ "code": 2 })),
                carried: Vec::new(),
            };
        };
        let entry = match self.resolve_agent_target(host_key, session_id.as_deref(), &target) {
            Ok(e) => e,
            Err(e) => return bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent stop: {e}\n")), data: Some(json!({ "code": 1 })), carried: Vec::new() },
        };

        // Best-effort quick inspect before stopping so `cli_session_id` and `transcript_path` are saved!
        if entry.cli_session_id.is_none() || entry.transcript_path.is_none() {
            let _ = self.inspect_subagents_on_host(host_key, std::slice::from_ref(&entry)).await;
        }
        let entry = self.resolve_agent_target(host_key, Some(&entry.session_id), &entry.id).unwrap_or(entry);

        // Item #14: Archive full scrollback before Ctrl-C / closing when `--save PATH` is given.
        let mut carried = Vec::new();
        if let Some(ref dest) = save_path {
            let ctrl = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                st.hosts.get(host_key).and_then(|h| h.control.clone())
            };
            let text = if let Some(ctrl) = ctrl
                && let Ok(r) = ctrl.terminal_format(&entry.session_id, &entry.block_id, "plain").await
            {
                r.payload
            } else {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let bk = BlockKey { host_key: entry.host_key.clone(), session_id: entry.session_id.clone(), block_id: entry.block_id.clone() };
                st.blocks.get(&bk).and_then(|br| br.lock().unwrap_or_else(|e| e.into_inner()).terminal.format(olo_rex_vt::FormatKind::Plain, true, true).ok()).unwrap_or_default()
            };
            carried.push(bridge::Carried { path: dest.clone(), to: Some(dest.clone()), stdout: false, bytes: text.into_bytes() });
        }

        match self.stop_subagent_windows(host_key, &entry.session_id, &entry.window_id, force, tree, keep_tab).await {
            Ok(stopped) => {
                let payload = json!({
                    "ok": true,
                    "id": entry.id,
                    "name": entry.name,
                    "stopped": stopped,
                    "cli_session_id": entry.effective_session_id(),
                    "transcript_path": entry.transcript_path,
                    "saved_path": save_path,
                });
                let stdout = if as_json { format!("{payload}\n") } else { format!("Stopped {} subagent tab(s).\n", stopped.len()) };
                bridge::BridgeResponse { ok: true, stdout: Some(stdout), stderr: None, data: Some(payload), carried }
            }
            Err(e) => bridge::BridgeResponse { ok: false, stdout: None, stderr: Some(format!("olo agent stop: {e}\n")), data: Some(json!({ "code": 1 })), carried: Vec::new() },
        }
    }

    async fn stop_subagent_windows(self: &Arc<Self>, host_key: &str, session_id: &str, window_id: &str, force: bool, tree: bool, keep_tab: bool) -> Result<Vec<String>, String> {
        let (ctrl, ep, targets) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let h = st.hosts.get(host_key).ok_or_else(|| format!("unknown host `{host_key}`"))?;
            let ctrl = h.control.clone().ok_or_else(|| "Rex host is not connected".to_owned())?;
            let ep = h.endpoint.clone().ok_or_else(|| "Rex host endpoint unavailable".to_owned())?;
            let live: HashSet<String> = h.sessions.get(session_id).map(|s| s.windows.iter().map(|w| w.window_id.clone()).collect()).unwrap_or_default();
            let mut targets: Vec<crate::subagents::NativeSubagent> = Vec::new();
            if tree {
                let mut desc = st.subagents.descendants_tree(host_key, session_id, window_id, &live);
                // Stop deepest grandchildren first (bottom-up)!
                desc.reverse();
                for row in desc {
                    targets.push(row.entry);
                }
            }
            if let Some(own) = st.subagents.by_window(host_key, session_id, window_id) {
                targets.push(own.clone());
            }
            (ctrl, ep, targets)
        };
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        if !force {
            for ent in &targets {
                let _ = ctrl.terminal_write(session_id, &ent.block_id, "\x03").await;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let mut stopped = Vec::with_capacity(targets.len());
        for ent in &targets {
            if !keep_tab {
                let _ = ctrl.session_close_window(session_id, Some(&ent.window_id)).await;
            }
            stopped.push(ent.id.clone());
        }
        if !keep_tab {
            {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let mut changed = false;
                for ent in &targets {
                    changed |= st.subagents.mark_exited(host_key, session_id, &ent.window_id);
                }
                if changed {
                    st.subagents.save();
                }
            }
            {
                let mut log = self.subagent_log.lock().unwrap_or_else(|e| e.into_inner());
                let ts = crate::subagents::now_unix_ms();
                for ent in &targets {
                    log.push(crate::subagents::SubagentEvent {
                        seq: 0,
                        timestamp_ms: ts,
                        host_key: ent.host_key.clone(),
                        session_id: ent.session_id.clone(),
                        parent_window_id: ent.parent_window_id.clone(),
                        id: ent.id.clone(),
                        name: ent.name.clone(),
                        window_id: ent.window_id.clone(),
                        cli_session_id: ent.effective_session_id().map(str::to_owned),
                        event: "exited".into(),
                        state: crate::subagents::SubagentLiveState::Exited,
                        summary: Some("stopped".into()),
                        last_message: None,
                        approval: None,
                        error: None,
                        tail: Vec::new(),
                    });
                }
            }
            if let Ok(v) = ctrl.session_view(session_id).await {
                self.ingest_session(host_key, &ctrl, &ep, v.payload, &[]).await;
            }
        }
        Ok(stopped)
    }

    async fn agent_prune(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> Value {
        let mut all = false;
        let mut as_json = false;
        if let Some(v) = raw {
            if let Some(arr) = v.get("args").and_then(Value::as_array) {
                for a in arr.iter().filter_map(Value::as_str) {
                    match a {
                        "--all" | "-a" => all = true,
                        "--json" => as_json = true,
                        _ => {}
                    }
                }
            } else {
                all = v.get("all").and_then(Value::as_bool).unwrap_or(false);
                as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
            }
        }
        let pruned = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let n = st.subagents.prune_exited(host_key, if all { None } else { session_id.as_deref() });
            if n > 0 {
                st.subagents.save();
            }
            n
        };
        let payload = json!({ "ok": true, "pruned": pruned });
        let stdout = if as_json { format!("{payload}\n") } else { format!("Pruned {pruned} exited subagent record(s).\n") };
        json!({ "ok": true, "stdout": stdout, "data": payload })
    }

    async fn agent_focus(self: &Arc<Self>, host_key: &str, raw: Option<&Value>, session_id: Option<String>) -> Value {
        let target = raw.and_then(|v| {
            v.get("target")
                .or_else(|| v.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| v.get("args").and_then(Value::as_array).and_then(|a| a.iter().filter_map(Value::as_str).find(|s| !s.starts_with('-')).map(str::to_owned)))
        });
        let Some(target) = target else {
            return json!({ "ok": false, "stderr": "usage: olo agent focus <ID|NAME>\n", "data": { "code": 2 } });
        };
        let entry = match self.resolve_agent_target(host_key, session_id.as_deref(), &target) {
            Ok(e) => e,
            Err(e) => return json!({ "ok": false, "stderr": format!("olo agent focus: {e}\n"), "data": { "code": 1 } }),
        };
        let ctrl = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| h.control.clone())
        };
        let Some(ctrl) = ctrl else {
            return json!({ "ok": false, "stderr": "olo agent focus: Rex host is not connected\n", "data": { "code": 69 } });
        };
        if let Err(e) = ctrl.session_focus_window(&entry.session_id, &entry.window_id).await {
            return json!({ "ok": false, "stderr": format!("olo agent focus: {e}\n"), "data": { "code": 1 } });
        }
        (self.notify)(HubNotice::StateChanged);
        json!({ "ok": true, "stdout": format!("Focused {} ({}).\n", entry.id, entry.display_title()), "data": { "ok": true, "id": entry.id, "name": entry.name, "window_id": entry.window_id } })
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

/// A terminal's host is asked how the terminal reads, at a pause in
/// its output: whether the program there now asks for something secret
/// (the server has no event for a terminal's input mode). And, where
/// the caret stands at a prompt known to echo with nothing typed on
/// its way, whether the shell itself is reading there: then what is
/// typed next is drawn at once, not only after a letter of it has come
/// back (`predict`, "The first letter at a shell's prompt"). That is
/// asked with the mode, in the same round trip; not of a terminal
/// whose echo is quick, or that isn't at such a prompt.
async fn ask_the_host(br: &Arc<Mutex<BlockRuntime>>, ctrl: &ControlClient, bk: &BlockKey, notify: &Arc<dyn Fn(HubNotice) + Send + Sync>) {
    let asked = {
        let mut b = br.lock().unwrap_or_else(|e| e.into_inner());
        let screen = b.caret_screen();
        // (Not asked what the host's helper has said already.)
        b.host_says_prompt(ctrl.last_typed(), screen.as_ref());
        b.predict.wants_prompt(screen.as_ref())
    };
    let (mode, process) = match asked {
        Some(_) => match tokio::try_join!(ctrl.terminal_input_mode(&bk.session_id, &bk.block_id), ctrl.terminal_process(&bk.session_id, &bk.block_id)) {
            Ok((mode, process)) => (Some(mode.payload), Some(process.payload)),
            Err(_) => (None, None),
        },
        None => (ctrl.terminal_input_mode(&bk.session_id, &bk.block_id).await.ok().map(|mode| mode.payload), None),
    };
    let Some(mode) = mode else { return };
    let asks = asks_for_secret(mode.echo, mode.canonical);
    let changed = {
        let mut b = br.lock().unwrap_or_else(|e| e.into_inner());
        let changed = b.set_password_prompt(asks);
        if let (Some(asked), Some(process)) = (asked, process) {
            let screen = b.caret_screen();
            b.predict.prompt_said(asked, shell_reads(&mode, &process), screen.as_ref());
        }
        changed
    };
    if changed {
        notify(HubNotice::StateChanged);
    }
}

/// Whether the program reading a terminal is its shell itself, at its
/// line editor: the terminal's foreground process is the one the
/// terminal was started with, and it reads key by key with the echo
/// its own (the line discipline's echo and line editing both off). Not
/// a program the shell started (whatever its mode), and not a line the
/// shell reads through the terminal's own editing (`read`, `read -s`).
fn shell_reads(mode: &olo_rex_client::InputMode, process: &olo_rex_client::TerminalProcessSnapshot) -> bool {
    !mode.echo && !mode.canonical && matches!((&process.child, &process.foreground), (Some(child), Some(foreground)) if child.pid == foreground.pid && child.pid != 0)
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

/// The port a forward's client says it listens at ("127.0.0.1:5176",
/// "[::1]:5176", "0.0.0.0:5176"), if it says one.
fn listens_at(address: Option<&str>) -> Option<u16> {
    address?.rsplit_once(':')?.1.parse().ok().filter(|p| *p != 0)
}

/// A Rex forward as an agent is told of it, holding a port it wanted: by
/// its short id (a block's id's last six, as `rex forward ls` shows it),
/// what it reaches, where it is and how to close it: there with `rex
/// forward close` where it is in a session of the asking host's own
/// server, else the person, in Rex on the Olo desktop.
fn forward_said(block_id: &str, target: Option<&str>, session: &str, server: &str, local: bool, asker_s: bool) -> String {
    let short = &block_id[block_id.len().saturating_sub(6)..];
    let to = target.filter(|t| !t.is_empty()).map(|t| format!(" to {t}")).unwrap_or_default();
    let session = format!(" -s {}", shell_word(session));
    if asker_s {
        format!("Rex forward {short}{to}, of this host's own Rex: `rex forward close {short}{session}` here closes it")
    } else if local {
        format!("Rex forward {short}{to}, of the Olo desktop's own Rex: the person can close it there (`rex forward close {short}{session}`)")
    } else {
        format!("Rex forward {short}{to}, on {server}: `rex forward close {short}{session}` there closes it")
    }
}

/// `s` as one word of a shell command: as it is where it is plain, else
/// in single quotes.
fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@".contains(c)) { s.to_owned() } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

#[cfg(test)]
mod tests {
    /// How long a copy has left, at its pace so far: nothing in its
    /// first seconds, minutes as a person says them, then hours.
    #[test]
    fn a_copys_time_left() {
        let s = Duration::from_secs;
        assert_eq!(super::time_left(100, 1000, s(2)), None, "too soon to say");
        assert_eq!(super::time_left(500, 1000, s(20)).as_deref(), Some("less than a minute left"));
        assert_eq!(super::time_left(100, 1000, s(60)).as_deref(), Some("about 9 minutes left"));
        assert_eq!(super::time_left(1, 1000, s(10)).as_deref(), Some("about 3 hours left"));
        assert_eq!(super::time_left(1000, 1000, s(60)), None, "done");
    }

    /// A forward holding a port an agent wanted: the port its client says
    /// it listens at, and how it is said (by its short id, where it is,
    /// how to close it).
    #[test]
    fn a_forward_holding_a_port_is_said_by_name() {
        assert_eq!(super::listens_at(Some("127.0.0.1:5176")), Some(5176));
        assert_eq!(super::listens_at(Some("[::1]:5176")), Some(5176));
        assert_eq!(super::listens_at(Some("127.0.0.1:0")), None);
        assert_eq!(super::listens_at(None), None);
        let id = "block:01a20c79-0250-7c7e-b2a8-6a5bc2cda661";
        assert_eq!(
            super::forward_said(id, Some("localhost:5176"), "work", "build", false, true),
            "Rex forward cda661 to localhost:5176, of this host's own Rex: `rex forward close cda661 -s work` here closes it"
        );
        assert_eq!(
            super::forward_said(id, None, "my work", "This computer", true, false),
            "Rex forward cda661, of the Olo desktop's own Rex: the person can close it there (`rex forward close cda661 -s 'my work'`)"
        );
        assert_eq!(
            super::forward_said(id, Some("db:5432"), "session:01ab", "build", false, false),
            "Rex forward cda661 to db:5432, on build: `rex forward close cda661 -s session:01ab` there closes it"
        );
        assert_eq!(super::shell_word("it's"), "'it'\\''s'");
    }

    /// What Passwords answers a host's asking for the keys is said as
    /// which (by their items' names), or why none.
    #[test]
    fn what_a_host_is_offered_of_the_keys_is_said() {
        use olo_keychain::{CallError, Response, SshKey};
        let key = |name: &str| SshKey { key: "AAAA".into(), comment: name.into() };
        assert_eq!(super::keys_offered(&Ok(Response::SshKeys { keys: vec![key("work"), key("github")] })), Ok(vec!["work".to_owned(), "github".to_owned()]));
        assert_eq!(super::keys_offered(&Ok(Response::SshKeys { keys: Vec::new() })), Ok(Vec::new()));
        assert_eq!(super::keys_offered(&Ok(Response::Locked)), Err("Passwords is locked".into()));
        assert_eq!(super::keys_offered(&Err(CallError::NotRunning)), Err("Passwords isn’t running".into()));
        assert_eq!(super::keys_offered(&Ok(Response::Error { message: "only Rex asks for a host".into() })), Err("only Rex asks for a host".into()));
    }

    use super::*;

    /// The wheel over a pane goes where a local terminal sends it.
    #[test]
    fn the_wheel_goes_as_in_a_local_terminal() {
        assert_eq!(wheel_goes(false, false, false, true), Wheel::Scrollback, "a shell: its history");
        assert_eq!(wheel_goes(true, false, false, false), Wheel::Report, "a program with the mouse");
        assert_eq!(wheel_goes(true, true, false, false), Wheel::Scrollback, "Shift keeps it from the program");
        assert_eq!(wheel_goes(false, false, true, true), Wheel::Arrows, "a pager: arrows");
        assert_eq!(wheel_goes(false, false, true, false), Wheel::Nothing, "the alternate screen has no history");
        assert_eq!(wheel_goes(true, false, true, true), Wheel::Report, "an editor with the mouse");
        assert_eq!(arrows(true, 2, false), b"\x1b[A\x1b[A");
        assert_eq!(arrows(false, 1, true), b"\x1bOB");
    }

    /// A program that asked for the mouse is told of presses, drags and
    /// releases where they are, and of the wheel; one that hasn't, of
    /// nothing.
    /// A pane's program in a title: a coding agent by its own name, Claude
    /// Code's program being named by its version; any other by its name.
    #[test]
    fn an_agent_program_is_titled_by_the_agent() {
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        assert_eq!(b.program_title(), None);
        let running = |name: &str, path: &str| TerminalProcessSnapshot {
            foreground: Some(olo_rex_client::ProcessInfo { pid: 4210, name: name.to_owned(), path: Some(path.to_owned()), ..Default::default() }),
            ..Default::default()
        };
        b.process = Some(running("2.1.331", "/home/u/.local/share/claude/versions/2.1.331"));
        assert_eq!(b.program_title().as_deref(), Some("Claude Code"));
        b.process = Some(running("agy", "/home/u/.local/bin/agy"));
        assert_eq!(b.program_title().as_deref(), Some("Antigravity"));
        b.process = Some(running("htop", "/usr/bin/htop"));
        assert_eq!(b.program_title().as_deref(), Some("htop"));
        // (What the pane's head says by it, a folder after.)
        b.process = Some(running("2.1.331", "/home/u/.local/share/claude/versions/2.1.331"));
        assert_eq!(b.said(None), "Claude Code");
    }

    #[test]
    fn the_mouse_is_reported_to_a_program_that_asked() {
        use olo_rex_vt::input::{KeyMods, MouseAction, MouseButton};
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        assert_eq!(b.mouse_report(MouseAction::Press, Some(MouseButton::Left), KeyMods::NONE, (2, 3)), None, "not asked");
        // Button events and SGR's encoding, as vim and tmux ask.
        b.terminal.vt_write(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(b.mouse_report(MouseAction::Press, Some(MouseButton::Left), KeyMods::NONE, (2, 3)).as_deref(), Some(&b"\x1b[<0;3;4M"[..]));
        assert_eq!(b.mouse_held, Some(MouseButton::Left));
        assert_eq!(b.mouse_report(MouseAction::Motion, Some(MouseButton::Left), KeyMods::NONE, (4, 3)).as_deref(), Some(&b"\x1b[<32;5;4M"[..]), "dragged");
        assert_eq!(b.mouse_report(MouseAction::Release, Some(MouseButton::Left), KeyMods::NONE, (4, 3)).as_deref(), Some(&b"\x1b[<0;5;4m"[..]));
        assert_eq!(b.mouse_held, None);
        assert_eq!(b.mouse_report(MouseAction::Press, Some(MouseButton::Four), KeyMods::NONE, (0, 0)).as_deref(), Some(&b"\x1b[<64;1;1M"[..]), "the wheel up");
        assert_eq!(b.mouse_held, None, "a wheel's step isn't held");
        // Plain tracking: a move with no button isn't wanted.
        b.terminal.vt_write(b"\x1b[?1002l\x1b[?1000h");
        assert_eq!(b.mouse_report(MouseAction::Motion, None, KeyMods::NONE, (1, 1)), None);
    }
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
    fn what_was_let_go_of_is_kept_over_an_upgrade_and_what_was_kept_before_still_reads() {
        // (What Rex kept before there was a Disconnect has no such list.)
        let before: crate::ui::SavedAppState = serde_json::from_str(r#"{"workspaces":[]}"#).unwrap();
        assert!(before.let_go.is_empty() && before.forwards.is_empty());
        let forward = SavedForward {
            host_key: "host-1".into(),
            session_id: "s".into(),
            block_id: "b".into(),
            remote_host: "localhost".into(),
            remote_port: 5173,
            local_port: 5173,
            made: crate::forwards::Made::Agent,
            kept: true,
        };
        let kept = serde_json::to_string(&crate::ui::SavedAppState { workspaces: Vec::new(), let_go: vec!["host-1".into()], help: Vec::new(), forwards: vec![forward.clone()] }).unwrap();
        let read = serde_json::from_str::<crate::ui::SavedAppState>(&kept).unwrap();
        assert_eq!(read.let_go, ["host-1"]);
        assert_eq!(read.forwards, [forward]);
    }

    #[test]
    fn a_network_that_came_back_connects_only_what_was_being_connected() {
        assert!(wanted_back(&HostConnState::Reconnecting { attempt: 3, last_error: "closed".into() }));
        assert!(wanted_back(&HostConnState::Error("no route".into())));
        // (Let go of by the person, or never connected: left so.)
        assert!(!wanted_back(&HostConnState::Disconnected));
        assert!(!wanted_back(&HostConnState::Connected) && !wanted_back(&HostConnState::Connecting));
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

    #[test]
    fn parse_sync_ask_covers_directions_filters_and_subcommands() {
        use olo_protocol::RexSyncDirection as Dir;
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(parse_sync_ask(&s(&["ls", "--json"])), Ok(SyncAsk::List { json: true }));
        assert_eq!(parse_sync_ask(&s(&["conflicts", "/home/dev/notes", "--json"])), Ok(SyncAsk::Conflicts { path: Some("/home/dev/notes".into()), json: true }));
        assert_eq!(parse_sync_ask(&s(&["wait", "/home/dev/notes"])), Ok(SyncAsk::Wait { path: "/home/dev/notes".into(), json: false }));
        assert_eq!(parse_sync_ask(&s(&["stop", "/home/dev/notes"])), Ok(SyncAsk::Stop { path: "/home/dev/notes".into() }));
        assert_eq!(
            parse_sync_ask(&s(&["/home/dev/notes", "--two-way", "--shallow", "--exclude", "target", "--exclude=*.tmp"])),
            Ok(SyncAsk::Keep { path: "/home/dev/notes".into(), direction: Dir::Both, mirror: false, shallow: true, exclude: vec!["target".into(), "*.tmp".into()], json: false })
        );
        assert_eq!(
            parse_sync_ask(&s(&["--to-host", "--mirror", "/home/dev/assets"])),
            Ok(SyncAsk::Keep { path: "/home/dev/assets".into(), direction: Dir::ToHost, mirror: true, shallow: false, exclude: vec![], json: false })
        );
        assert!(parse_sync_ask(&s(&["/home/dev/a", "--to-host", "--from-host"])).is_err());
        assert!(parse_sync_ask(&s(&["/home/dev/a", "--bogus"])).is_err());
    }

    /// End-to-end live integration test for the 18-item `olo agent` subagent suite:
    /// runs an isolated scratch `rex server run` connected to a real `RexHubInner`,
    /// verifying: named children, server-side `--desktop none` and `--desktop screens=X`
    /// isolation, `olo.agent.approve --once` / `--no` dialog navigation & dismissal,
    /// `olo.agent.watch --all --since CURSOR` zero-miss event delivery without false
    /// approvals, `olo.agent.send` paste+Enter delivery, `olo.agent.stop --save` scrollback
    /// archival via carried files, `list --all` persistence across Rex server restart + `prune`,
    /// and 40-child `list` scaling.
    #[test]
    fn live_subagent_suite_e2e_and_40_child_scale() {
        if !std::path::Path::new("/usr/bin/rex").exists() {
            return;
        }
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-subagent-e2e-{}", std::process::id()));
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
        rt.block_on(async {
            let ep = RexEndpoint::Unix(sock.clone());
            let (stream, host_hdr) = ep.connect_stream().await.unwrap();
            let (ctrl, _ev) = ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(Some("subagent-e2e".into())), None).await.unwrap();
            let created = ctrl.session_create(Some("lead"), None).await.unwrap();
            let sid = created.payload.get("session_id").and_then(Value::as_str).unwrap().to_owned();
            let att = ctrl.session_attach(&sid).await.unwrap();
            let parent_wid = att.payload.view.windows[0].window_id.clone();
            let parent_bid = att.payload.view.windows[0].all_blocks().next().unwrap().block_id.clone();

            let mut hosts = HashMap::new();
            let hr = HostRuntime {
                key: "local".into(),
                label: "This computer".into(),
                endpoint_raw: "local".into(),
                endpoint: Some(ep.clone()),
                is_local: true,
                conn_state: HostConnState::Connected,
                connection_lost: None,
                client_id: None,
                reconnect_token: None,
                retry: None,
                server_status: None,
                server_version: None,
                server_newer: None,
                helper: HostHelper::Ready,
                helper_current: Some(true),
                helper_sync_err: None,
                bridge_there: None,
                skill_there: None,
                skill_current: None,
                skill_sync_err: None,
                commands_there: None,
                commands_current: None,
                commands_sync_err: None,
                agent_keys: None,
                account: None,
                carried: None,
                agent_sessions: None,
                repos_there: None,
                tailscale: None,
                tailscale_seq: 0,
                synced_hosts: Vec::new(),
                session_summaries: vec![SessionSummary { session_id: sid.clone(), label: "lead".into() }],
                sessions: HashMap::new(),
                pending_focus: HashMap::new(),
                keymap_bindings: Vec::new(),
                keymap_modes: Vec::new(),
                keymap_diagnostics: Vec::new(),
                control: Some(ctrl.clone()),
                input_tx: None,
            };
            hosts.insert("local".to_owned(), hr);

            let state = Arc::new(Mutex::new(HubState {
                hosts,
                host_order: vec!["local".into()],
                blocks: HashMap::new(),
                forwards: Vec::new(),
                kept_forwards: Vec::new(),
                kept_forward_errors: HashMap::new(),
                kept_forwards_starting: HashSet::new(),
                settings: crate::settings::Settings::default(),
                active_theme: TerminalTheme::default(),
                pools: None,
                repos: Vec::new(),
                carry_choices: Vec::new(),
                kept: Vec::new(),
                sessions_window: olo_rex_client::sessions::Window::default(),
                git_cache: GitCache::new(),
                local_bin_dir: None,
                subagents: crate::subagents::SubagentRegistry::default(),
                quick_look_cache: HashMap::new(),
            }));
            let inner = Arc::new(RexHubInner {
                state: state.clone(),
                ssh_pool: Arc::new(SshSessionPool::default()),
                askpass: None,
                notify: Arc::new(|_| {}),
                spares: Mutex::new(HashMap::new()),
                agent_socks: Mutex::new(HashMap::new()),
                bridge_calls: bridge::Calls::default(),
                run_jobs: crate::bridge_run::RunJobs::new(),
                run_approvals: crate::bridge_run::RunApprovals::new(|| {}, || {}),
                keeping: Mutex::new(HashMap::new()),
                subagent_events: Arc::new(tokio::sync::Notify::new()),
                subagent_log: Mutex::new(crate::subagents::SubagentEventLog::default()),
            });
            inner.ingest_session("local", &ctrl, &ep, att.payload.view, &[]).await;

            // Create two real child tabs on the scratch Rex server (one with `--desktop none`, one with `--desktop screens=qa-1`).
            let make_child = |name: &str, desktop: crate::subagents::DesktopPolicy, sid_uuid: &str, title: &str| {
                let (inner, ctrl, ep, sid, parent_wid, parent_bid) = (inner.clone(), ctrl.clone(), ep.clone(), sid.clone(), parent_wid.clone(), parent_bid.clone());
                let (name, sid_uuid, title) = (name.to_owned(), sid_uuid.to_owned(), title.to_owned());
                async move {
                    let w = ctrl
                        .session_new_window_with_focus(&sid, Some(&title), Some(json!({"command": ["sh", "-c", "printf 'SCROLLBACK_MARKER\\r\\n'; cat"], "interactive": true})), false)
                        .await
                        .unwrap()
                        .payload;
                    let wid = w["window_id"].as_str().unwrap().to_owned();
                    let view = ctrl.session_view(&sid).await.unwrap().payload;
                    let bid = view.windows.iter().find(|x| x.window_id == wid).unwrap().all_blocks().next().unwrap().block_id.clone();
                    let entry = crate::subagents::NativeSubagent {
                        id: crate::subagents::new_subagent_id(),
                        name: Some(name),
                        host_key: "local".into(),
                        session_id: sid.clone(),
                        window_id: wid,
                        block_id: bid,
                        parent_window_id: parent_wid,
                        parent_block_id: Some(parent_bid),
                        cli: crate::subagents::AgentCli::Agy,
                        model: "gemini-4-neon".into(),
                        effort: "xhigh".into(),
                        title,
                        role: None,
                        cwd: Some("/var/tmp".into()),
                        resume_id: None,
                        cli_session_id: Some(sid_uuid),
                        transcript_path: Some("/var/tmp/fake-transcript.jsonl".into()),
                        desktop_policy: desktop,
                        stream_mode: false,
                        started_at: crate::subagents::now_unix(),
                        exited_at: None,
                        last_pid: None,
                    };
                    inner.state.lock().unwrap().subagents.register(entry.clone());
                    inner.ingest_session("local", &ctrl, &ep, view, &[]).await;
                    entry
                }
            };

            let c_none = make_child("builder-1", crate::subagents::DesktopPolicy::None, "11111111-2222-3333-4444-555555555555", "Builder 1").await;
            let c_qa = make_child("qa-1", crate::subagents::DesktopPolicy::parse("screens=qa-1").unwrap(), "22222222-3333-4444-5555-666666666666", "QA 1").await;

            // 1. Server-side desktop isolation enforcement (Item #11)
            let deny_op = inner.handle_client_perform("local", "olo.op".into(), Some(json!({"args": ["read", "op://V/I/f"]})), Some(sid.clone()), Some(c_none.block_id.clone())).await.unwrap();
            assert_eq!(deny_op["ok"], json!(false));
            assert_eq!(deny_op["data"]["code"], json!(69));
            let deny_screen = inner.handle_client_perform("local", "olo.screen".into(), Some(json!({"args": ["tree", "qa-1"]})), Some(sid.clone()), Some(c_none.block_id.clone())).await.unwrap();
            assert_eq!(deny_screen["ok"], json!(false));
            // Scoped QA child: `olo.op` refused, `olo.screen tree other` refused!
            let qa_deny_op = inner.handle_client_perform("local", "olo.op".into(), Some(json!({"args": ["whoami"]})), Some(sid.clone()), Some(c_qa.block_id.clone())).await.unwrap();
            assert_eq!(qa_deny_op["ok"], json!(false));
            let qa_deny_other = inner.handle_client_perform("local", "olo.screen".into(), Some(json!({"args": ["tree", "other"]})), Some(sid.clone()), Some(c_qa.block_id.clone())).await.unwrap();
            assert_eq!(qa_deny_other["ok"], json!(false));

            // Wait briefly for `SCROLLBACK_MARKER` to reach the VT of c_none
            tokio::time::sleep(Duration::from_millis(200)).await;

            // 2. Named target resolution + rich status + approval dialog navigation (Items #3, #4, #8, #17)
            {
                let bk = BlockKey { host_key: "local".into(), session_id: sid.clone(), block_id: c_none.block_id.clone() };
                let br = inner.state.lock().unwrap().blocks.get(&bk).unwrap().clone();
                let mut g = br.lock().unwrap();
                g.terminal.vt_write(b"\x1b[2J\x1b[HCommand\r\nRequesting permission for:\r\n   cargo test -p api\r\nRun this command?\r\n> 1. Yes, run command\r\n  2. Yes, and always allow in this conversation\r\n  3. No, cancel\r\n");
            }
            let st_resp = inner.agent_status("local", Some(&json!({"args": ["builder-1", "--json"]})), Some(sid.clone())).await;
            assert!(st_resp.ok, "{st_resp:?}");
            let st_data = st_resp.data.unwrap();
            assert_eq!(st_data["state"], json!("waiting_approval"));
            assert_eq!(st_data["name"], json!("builder-1"));
            assert_eq!(st_data["cli_session_id"], json!("11111111-2222-3333-4444-555555555555"));
            assert_eq!(st_data["approval"]["choices"].as_array().unwrap().len(), 3);
            assert_eq!(st_data["approval"]["choices"][0]["kind"], json!("once"));
            assert_eq!(st_data["approval"]["choices"][1]["kind"], json!("allow_session"));
            assert_eq!(st_data["approval"]["choices"][2]["kind"], json!("no"));

            // Trigger `record_subagent_transition` + confirm watch `--all --since 0` receives the confirmed event!
            inner.record_subagent_transition(&BlockKey { host_key: "local".into(), session_id: sid.clone(), block_id: c_none.block_id.clone() });
            let w1 = inner
                .agent_watch("local", Some(&json!({"args": ["--all", "--since", "0", "--on", "approval,done,error,exit", "--timeout", "3", "--json"]})), Some(sid.clone()), Some(parent_bid.clone()))
                .await;
            assert!(w1.ok, "watch --all --since 0 failed: {w1:?}");
            let w1_data = w1.data.unwrap();
            assert_eq!(w1_data["id"], json!(c_none.id));
            assert_eq!(w1_data["event"], json!("waiting_approval"));
            let cursor1 = w1_data["cursor"].as_u64().unwrap();
            assert!(cursor1 >= 1);

            // 3. `olo agent send` paste + Enter (Item #13)
            let send_resp = inner.agent_send("local", Some(&json!({"args": ["builder-1", "hello\nworld", "--enter", "--json"]})), Some(sid.clone())).await;
            assert!(send_resp.ok, "{send_resp:?}");

            // 4. `olo agent stop --save PATH` carries scrollback file and preserves `cli_session_id` in `list --all` (Items #9, #14)
            let save_to = dir.join("saved-scrollback.txt");
            let stop_resp = inner
                .agent_stop("local", Some(&json!({"args": ["builder-1", "--force", "--save", save_to.to_string_lossy(), "--json"]})), Some(sid.clone()))
                .await;
            assert!(stop_resp.ok, "{stop_resp:?}");
            assert_eq!(stop_resp.carried.len(), 1);
            assert_eq!(stop_resp.carried[0].to.as_deref(), Some(save_to.to_string_lossy().as_ref()));
            let stop_data = stop_resp.data.unwrap();
            assert_eq!(stop_data["cli_session_id"], json!("11111111-2222-3333-4444-555555555555"));
            assert_eq!(stop_data["transcript_path"], json!("/var/tmp/fake-transcript.jsonl"));

            // Watch `--all --since cursor1` immediately receives the `exited` event without missing it! (Item #5)
            let w2 = inner
                .agent_watch("local", Some(&json!({"args": ["--all", "--since", cursor1.to_string(), "--on", "exit", "--timeout", "2", "--json"]})), Some(sid.clone()), Some(parent_bid.clone()))
                .await;
            assert!(w2.ok, "watch --all --since cursor1 missed exited event: {w2:?}");
            assert_eq!(w2.data.unwrap()["event"], json!("exited"));

            // `list --all --fast` shows `builder-1` as `exited` with its `cli_session_id` intact! (Item #9)
            let ls_all = inner.agent_list("local", Some(&json!({"args": ["--all", "--fast", "--json"]})), Some(sid.clone()), Some(parent_bid.clone())).await;
            assert!(ls_all.ok);
            let subs = ls_all.data.unwrap()["subagents"].as_array().unwrap().clone();
            assert!(subs.iter().any(|s| s["name"] == "builder-1" && s["state"] == "exited" && s["cli_session_id"] == "11111111-2222-3333-4444-555555555555"));

            // `prune` cleans exited entries cleanly!
            let pr = inner.agent_prune("local", Some(&json!({"args": ["--json"]})), Some(sid.clone())).await;
            assert_eq!(pr["data"]["pruned"], json!(1));

            // 5. 40-child `list` batch scaling check (Item #10): register 40 children and time single-pass `list`!
            {
                let mut st = inner.state.lock().unwrap();
                for n in 0..40 {
                    let wid = format!("win-scale-{n}");
                    let bid = format!("blk-scale-{n}");
                    st.hosts.get_mut("local").unwrap().sessions.get_mut(&sid).unwrap().windows.push(olo_rex_client::WindowView {
                        window_id: wid.clone(),
                        label: format!("Worker {n}"),
                        focused_block_id: Some(bid.clone()),
                        zoomed_block_id: None,
                        layers: vec![],
                    });
                    st.blocks.insert(
                        BlockKey { host_key: "local".into(), session_id: sid.clone(), block_id: bid.clone() },
                        Arc::new(Mutex::new(BlockRuntime::new(80, 24, &TerminalTheme::default()))),
                    );
                    st.subagents.register(crate::subagents::NativeSubagent {
                        id: format!("sa-sc{n:02}"),
                        name: Some(format!("worker-{n}")),
                        host_key: "local".into(),
                        session_id: sid.clone(),
                        window_id: wid,
                        block_id: bid,
                        parent_window_id: parent_wid.clone(),
                        parent_block_id: Some(parent_bid.clone()),
                        cli: crate::subagents::AgentCli::Claude,
                        model: "claude-opus-5-5".into(),
                        effort: "max".into(),
                        title: format!("Worker {n}"),
                        role: None,
                        cwd: Some("/var/tmp".into()),
                        resume_id: None,
                        cli_session_id: None,
                        transcript_path: None,
                        desktop_policy: crate::subagents::DesktopPolicy::None,
                        stream_mode: false,
                        started_at: crate::subagents::now_unix(),
                        exited_at: None,
                        last_pid: None,
                    });
                }
            }
            let t0 = Instant::now();
            let ls40 = inner.agent_list("local", Some(&json!({"args": ["--json"]})), Some(sid.clone()), Some(parent_bid.clone())).await;
            let dt = t0.elapsed();
            assert!(ls40.ok, "{ls40:?}");
            let count = ls40.data.unwrap()["subagents"].as_array().unwrap().len();
            assert!(count >= 40, "expected >=40 children, got {count}");
            assert!(dt < Duration::from_secs(2), "40-child batched list took {dt:?}");
            eprintln!("40-child batched `olo agent list` (with single-pass olo-rex-link inspect) finished in {dt:?}");
        });
    }
}

#[cfg(test)]
mod reconnect_tests;

/// What Passwords' answer to a host's asking for the SSH keys offered
/// it: how many, or why none could be had, as a person reads it.
fn keys_offered(answer: &Result<olo_keychain::Response, olo_keychain::CallError>) -> Result<Vec<String>, String> {
    use olo_keychain::Response;
    match answer {
        // (By their items' names, as Passwords offers them.)
        Ok(Response::SshKeys { keys }) => Ok(keys.iter().map(|k| k.comment.clone()).collect()),
        Ok(Response::Locked) => Err(olo_kit::words::tr("Passwords is locked").into()),
        Ok(Response::NotSetUp) => Err(olo_kit::words::tr("Passwords isn’t set up").into()),
        Ok(Response::Error { message }) => Err(message.clone()),
        Ok(_) => Err(olo_kit::words::tr("Passwords answered something else").into()),
        Err(e) => Err(e.to_string()),
    }
}
