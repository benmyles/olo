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
    let program = g.process.as_ref().and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.as_str());
    Some(pane_title(&g.title, &g.label, program, ""))
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
    /// A Token Pool of its own there, with this computer's accounts
    /// (put there and kept in step by this computer's Token Pool:
    /// `pool`).
    Pool,
}

/// How a host stands, for the Host window: what it has of Olo's, as
/// last heard.
#[derive(Clone, Debug, PartialEq)]
pub struct HostStands {
    pub name: String,
    /// Where it is, as a person writes it: `you@host` for one reached
    /// over SSH.
    pub address: String,
    pub conn: HostConnState,
    /// Reached over SSH: what is put there and taken off goes that way.
    pub ssh: bool,
    pub helper: HostHelper,
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
    /// of them ([`HostRuntime::agent_keys`]).
    pub agent_keys: Option<Result<usize, String>>,
    pub skill: Option<bool>,
    /// How its Token Pool stands, as this computer's Token Pool says;
    /// none: that isn't running here (or wasn't asked yet).
    pub pool: Option<crate::pool::PoolStands>,
    /// Its Rex server's version, as it said when it connected.
    pub version: Option<String>,
    /// How the account there stands; none: not asked yet.
    pub account: Option<olo_rex_client::ready::Account>,
    /// The person's agent settings there: files to copy, files the
    /// same, and whether this computer's weren't all gathered.
    pub carried: Option<(usize, usize, bool)>,
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
    /// Olo's helper there, for this connection.
    pub helper: HostHelper,
    /// Whether Olo's bridge is on the host, and the skill for agents
    /// (their files in the account's home there): none until the host
    /// has been asked, and for a host that can't be (not over SSH).
    pub bridge_there: Option<bool>,
    pub skill_there: Option<bool>,
    /// What Passwords last answered when the host's programs asked the
    /// forwarded agent for the SSH keys: how many, or why none could be
    /// had; none: not asked on this connection. (Unsaid, a host was
    /// offered none and nothing showed it: Rex moved to an agent screen
    /// asked that screen's Passwords, 2026-11-23.)
    pub agent_keys: Option<Result<usize, String>>,
    /// How the account there stands (its agents, its path, git's
    /// identity: `olo_rex_client::ready`), as last asked; none until
    /// it has been (the Account window asks).
    pub account: Option<olo_rex_client::ready::Account>,
    /// What of the person's settings for their agents the host has
    /// (`olo_rex_client::carry`), asked with the account: how many
    /// files would be copied now, how many are there as here, and
    /// whether this computer's weren't all gathered.
    pub carried: Option<(usize, usize, bool)>,
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
    pub settings: Settings,
    pub active_theme: TerminalTheme,
    /// How each far host's Token Pool stands, by the host's key, as
    /// this computer's Token Pool last said (`pool::hosts`); none: it
    /// didn't answer, or wasn't asked yet.
    pub pools: Option<HashMap<String, crate::pool::PoolStands>>,
    /// The person's repositories (the desktop's settings: Preferences'
    /// Rex, kept by the Repositories window), in their order.
    pub repos: Vec<olo_rex_client::repos::Repo>,
    /// How far back the agents' sessions copied to a host go (the
    /// desktop's settings).
    pub sessions_window: olo_rex_client::sessions::Window,
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
            HostHelper::Used => Has { on: true, said: olo_kit::tr!("Olo’s typing helper is on “{}”.", name).to_string() },
            HostHelper::Installing => Has { on: false, said: olo_kit::tr!("Olo’s typing helper is being added to “{}”…", name).to_string() },
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
        let skill = host.skill_there.map(|there| match there {
            true => Has { on: true, said: olo_kit::tr!("Olo’s skill for agents is on “{}”.", name).to_string() },
            false => Has {
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
            return Some(HostStands {
                name: host.label.clone(),
                address,
                conn: host.conn_state.clone(),
                ssh: true,
                helper: if there { HostHelper::Used } else { HostHelper::None },
                helper_kept_off: false,
                helper_setting: true,
                bridge: there.then_some(true),
                // (`OLO_REX_DEMO_KEYS=2`, `0`, or a reason: the SSH keys
                // granted too, and what the host was offered of them.)
                granted: match (there, std::env::var("OLO_REX_DEMO_KEYS").is_ok()) {
                    (false, _) => Vec::new(),
                    (true, false) => vec!["links", "Passwords", "Agent Screens"],
                    (true, true) => vec!["links", "Passwords", "SSH keys", "Agent Screens"],
                },
                agent_keys: std::env::var("OLO_REX_DEMO_KEYS").ok().filter(|_| there).map(|keys| keys.parse::<usize>().map_err(|_| keys)),
                // (`OLO_REX_DEMO_ACCOUNT=ready`: the skill there too;
                // `asking`: not answered.)
                skill: match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
                    Some("asking") => None,
                    Some("ready") => there.then_some(true),
                    _ => there.then_some(false),
                },
                pool: Some(crate::pool::PoolStands { wanted: there, in_step: there, accounts: there.then_some(3), ..Default::default() }),
                version: there.then(|| "0.19.2".to_owned()),
                account: there.then(demo_account).flatten(),
                carried: match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
                    Some("asking") => None,
                    Some("ready") => there.then_some((0, 14, false)),
                    _ => there.then_some((14, 0, false)),
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
            });
        }
        Some(HostStands {
            name: host.label.clone(),
            address,
            conn: host.conn_state.clone(),
            ssh: matches!(host.endpoint, Some(RexEndpoint::Ssh { .. })),
            helper: host.helper.clone(),
            helper_kept_off: self.settings.helper_hosts.get(host_key).is_some_and(|word| word == "off"),
            helper_setting: self.settings.host_helper,
            bridge: host.bridge_there,
            granted: self.settings.policy_for_host(host_key, false).granted(),
            agent_keys: host.agent_keys.clone().filter(|_| self.settings.policy_for_host(host_key, false).allow_ssh_agent),
            skill: host.skill_there,
            pool: self.pools.as_ref().map(|pools| pools.get(host_key).cloned().unwrap_or_default()),
            version: host.server_version.as_ref().map(|v| v.version.clone()).filter(|v| !v.is_empty()),
            account: host.account.clone(),
            carried: host.carried,
            sessions: host.agent_sessions,
            sessions_window: self.sessions_window,
            repos: self.repos.clone(),
            // (As asked of the list as it is now, or not known.)
            repos_there: host.repos_there.clone().filter(|there| there.len() == self.repos.len()),
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
    rt: tokio::runtime::Runtime,
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

/// What a host has of the person's settings for their agents, against
/// what this computer carries (`olo_rex_client::carry`): this
/// computer's files gathered off the async threads, the host asked by
/// a script given on a shell's input (a line a file: too long for a
/// command's own words). None: it couldn't be asked.
async fn carried_stands(pool: &olo_rex_client::SshSessionPool, target: &olo_rex_client::SshTarget, askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>) -> Option<olo_rex_client::carry::Stands> {
    use olo_rex_client::carry;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let (files, short) = tokio::task::spawn_blocking(move || carry::gather(&home)).await.ok()?;
    let ran = pool.put(target, askpass, "sh", carry::look(&files).as_bytes()).await.ok()?;
    carry::Stands::read(&ran.output, &files, short)
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

/// The demo's account on its connected host, for the Account window's
/// pictures and its lint (`OLO_REX_DEMO_ACCOUNT`: `new`, nothing there
/// yet; `ready`, all of it; `asking`, not answered yet; `other`,
/// another name for git there; else some of it).
fn demo_account() -> Option<olo_rex_client::ready::Account> {
    use olo_rex_client::ready::Account;
    let who = Some(("You".to_owned(), "you@example.org".to_owned()));
    let some = Account { path: true, claude: Some("2.1.331".into()), codex: None, antigravity: None, git: who, has_curl: true, has_git: true };
    Some(match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
        Some("asking") => return None,
        Some("new") => Account { has_curl: true, has_git: true, ..Account::default() },
        Some("ready") => Account { codex: Some("0.172.0".into()), antigravity: Some("1.5.15".into()), ..some },
        Some("other") => Account { git: Some(("Somebody Else".to_owned(), "else@example.org".to_owned())), ..some },
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
                bridge_there: None,
                skill_there: None,
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
                        helper: HostHelper::None,
                        bridge_there: None,
                        skill_there: None,
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

        let state = Arc::new(Mutex::new(HubState {
            hosts,
            host_order,
            blocks: HashMap::new(),
            forwards: Vec::new(),
            settings,
            active_theme,
            pools: None,
            repos: Vec::new(),
            sessions_window: olo_rex_client::sessions::Window::default(),
            git_cache: GitCache::new(),
            local_bin_dir,
            quick_look_cache: HashMap::new(),
        }));

        let inner = Arc::new(RexHubInner {
            state: state.clone(),
            ssh_pool,
            askpass,
            notify: notify.clone(),
            spares: Mutex::new(HashMap::new()),
            bridge_calls: Default::default(),
            agent_socks: Mutex::new(HashMap::new()),
        });

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
            bridge_there: None,
            skill_there: None,
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
        }
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
                    helper: HostHelper::None,
                    bridge_there: None,
                    skill_there: None,
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

    /// Lets go of a host that is connected, or on its way (Hosts and
    /// Ports' Disconnect, where Connect is for one that isn't): its
    /// connection to the server closed, with the SSH connection under
    /// it and what went with that (the link over UDP, the forwarded
    /// keys), its panes gone from the windows and its forwards waiting.
    /// The host stays saved and its sessions go on on its server:
    /// Connect brings them back. Not this computer's own server, which
    /// Rex is nothing without.
    pub fn disconnect_host(&self, host_key: &str) {
        if host_key == "local" {
            return;
        }
        let endpoint = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get_mut(host_key) else { return };
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
        if let Some(RexEndpoint::Ssh { target, pool, .. }) = endpoint {
            self.rt.spawn(async move { pool.close(&target).await });
        }
        self.inner.write_palette_index();
        (self.notify)(HubNotice::StateChanged);
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
                Err(e) => feedback.problem("Couldn’t copy the file from the host.", e),
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

    /// One of Olo's things is put on a far host, or taken off it, over
    /// SSH and off the connection's own path (never typed into a
    /// pane, where a program running there would take it): `done`,
    /// on the hub's threads, once the host has answered, with what it
    /// said if it couldn't. The host is asked what it has again
    /// before that.
    pub fn change_host(&self, host_key: &str, thing: HostThing, add: bool, done: impl FnOnce(Result<(), String>) + Send + 'static) {
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
            let result = match (thing, add) {
                (HostThing::Helper, true) => {
                    inner.helper_kept_off(&host_key, false);
                    if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key).filter(|h| h.helper != HostHelper::Used) {
                        host.helper = HostHelper::Installing;
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
                (HostThing::Helper, false) => {
                    let ran = run(script(olo_rex_client::helper::REMOVE)).await;
                    if ran.is_ok() {
                        inner.helper_kept_off(&host_key, true);
                        // (A connection that goes through it goes on
                        // through it: said so while it does.)
                        if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key).filter(|h| h.helper != HostHelper::Used) {
                            host.helper = HostHelper::None;
                        }
                    }
                    ran
                }
                // (Nothing of the bridge is on a host: `bridge`.)
                (HostThing::Bridge, _) => Ok(()),
                // (This computer's Token Pool does it; asked, and how
                // it stands read again until it has.)
                (HostThing::Pool, add) => {
                    let (key, what) = (host_key.clone(), if add { "install" } else { "remove" });
                    match tokio::task::spawn_blocking(move || crate::pool::act(&key, what)).await.unwrap_or_else(|e| Err(e.to_string())) {
                        Err(why) => Err(why),
                        Ok(()) => {
                            let stands = inner.pools_settled(&host_key).await;
                            match stands {
                                Some(p) if p.wanted == add && (p.error.is_none() || !add) => Ok(()),
                                Some(p) => Err(p.error.unwrap_or_else(|| olo_kit::words::tr("It didn’t say why.").to_string())),
                                None => Err(olo_kit::words::tr("Token Pool isn’t running on this computer.").to_string()),
                            }
                        }
                    }
                }
                (HostThing::Skill, true) => run(script(&bridge::generate_remote_skill_install_script())).await,
                (HostThing::Skill, false) => run(script(bridge::REMOVE_SKILL)).await,
            };
            if thing != HostThing::Helper {
                inner.ask_what_the_host_has(&host_key, &pool, &target, askpass).await;
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
            inner.ask_what_the_host_has(&host_key, &pool, &target, askpass.clone()).await;
            inner.ask_the_pools().await;
            if crate::link::carried()
                && let Ok(Ok(asked)) = ask(&pool, &target, askpass, crate::link::ID, &crate::link::BUILDS).await
            {
                let here = matches!(asked, Asked::Here);
                if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                    match (here, &host.helper) {
                        (true, HostHelper::None | HostHelper::Without(_)) => host.helper = HostHelper::Ready,
                        (false, HostHelper::Ready) => host.helper = HostHelper::None,
                        _ => {}
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

    /// Puts on a host what it lacks of the person's settings for their
    /// agents (`olo_rex_client::carry`): asked what it has, given the
    /// files that are missing or differ in one archive, what was there
    /// kept beside each; then asked again how the account stands.
    pub fn carry_settings(&self, host_key: &str, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        use olo_rex_client::carry;
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            let result = match carried_stands(&pool, &target, askpass.clone()).await {
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
        let helper = match stands {
            Ok(Stands::Already) => {
                tracing::info!("rex: Olo's helper is on host {host_key} already");
                self.helper_known(host_key, Some(true));
                HostHelper::Ready
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
                HostHelper::Ready
            }
            Ok(Stands::NoBuild(kind)) => {
                tracing::info!("rex: no helper is carried for host {host_key} ({kind})");
                self.helper_known(host_key, Some(false));
                HostHelper::Without(olo_kit::tr!("there isn’t one for {}", kind).to_string())
            }
            Err(e) => {
                tracing::warn!("rex: Olo's helper couldn't be put on host {host_key}: {e}");
                HostHelper::Without(e.to_string())
            }
        };
        if let Some(host) = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(host_key).filter(|h| h.helper != HostHelper::Used) {
            host.helper = helper;
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
            Ok(Ok(Asked::Needs(build))) => build,
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
        let offered = move |answer: Result<usize, String>| {
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
                    Ok(n) => tracing::info!("Rex: {key} was offered {n} SSH keys"),
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
        tokio::spawn(async move { hub.ask_what_the_host_has(&host_key, &pool, &target, askpass).await });
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
        let carried = carried_stands(pool, target, askpass.clone()).await.map(|stands| (stands.to_copy.len(), stands.same, stands.short));
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
    /// asked, and what it answered kept.
    async fn ask_what_the_host_has(&self, host_key: &str, pool: &olo_rex_client::SshSessionPool, target: &olo_rex_client::SshTarget, askpass: Option<Arc<dyn olo_rex_client::SshAskpass>>) {
        let Ok(ran) = pool.exec(target, askpass.clone(), bridge::PRESENT, &mut |_| {}).await else { return };
        if !ran.ok() {
            return;
        }
        let (old_shims, skill_there) = bridge::present(&ran.output);
        // (The shims of before did nothing, and an agent that finds
        // them takes them for the way: off they come.)
        if old_shims {
            let _ = pool.exec(target, askpass.clone(), &format!("sh -c {}", shell_quote(bridge::OLD_SHIMS)), &mut |_| {}).await;
        }
        // (A skill there from another Olo than this one: this one's is
        // put in its place. An agent goes by what the skill there says.)
        if skill_there && !bridge::skill_is_current(&ran.output) {
            let _ = pool.exec(target, askpass, &format!("sh -c {}", shell_quote(&bridge::generate_remote_skill_install_script())), &mut |_| {}).await;
        }
        let bridge_there = true;
        let changed = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get_mut(host_key) else { return };
            let changed = (host.bridge_there, host.skill_there) != (Some(bridge_there), Some(skill_there));
            (host.bridge_there, host.skill_there) = (Some(bridge_there), Some(skill_there));
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
            let (is_local, policy) = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let is_local = st.hosts.get(host_key).is_some_and(|h| h.is_local);
                let policy = st.settings.policy_for_host(host_key, is_local);
                (is_local, policy)
            };
            let done = Some(json!({ "ok": true }));
            match action.as_str() {
                // What a session asks of this computer (`bridge`): a
                // link opened, Passwords' `op`, an Agent Screen's
                // `olo-screen`, each done as the host's permissions
                // allow and answered with what the command wrote; a
                // command asked before waited for again; a piece of a
                // picture an answer named.
                "olo.wait" | "olo.file" | "olo.open" | "olo.op" | "olo.screen" => {
                    let arg = |name: &str| args.as_ref().and_then(|a| a.get(name));
                    let text = |name: &str| arg(name).and_then(Value::as_str).unwrap_or_default().to_owned();
                    let (cmd, given) = match action.as_str() {
                        "olo.wait" => return Some(self.bridge_calls.wait(&text("id")).await),
                        "olo.file" => return Some(self.bridge_calls.piece(&text("id"), arg("offset").and_then(Value::as_u64).unwrap_or(0) as usize)),
                        "olo.open" => ("open", vec![text("target")]),
                        other => (
                            if other == "olo.op" { "op" } else { "screen" },
                            arg("args").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default(),
                        ),
                    };
                    let req = bridge::BridgeRequest { cmd: cmd.to_owned(), session_id: session_id.clone(), block_id: block_id.clone(), args: given, payload: Value::Null };
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
                "olo.notify" => {
                    self.bridge_notice(host_key, session_id.as_deref(), block_id.as_deref(), args.as_ref().unwrap_or(&Value::Null));
                    return done;
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

    /// What Passwords answers a host's asking for the keys is said as
    /// how many, or why none.
    #[test]
    fn what_a_host_is_offered_of_the_keys_is_said() {
        use olo_keychain::{CallError, Response, SshKey};
        let key = SshKey { key: "AAAA".into(), comment: "work".into() };
        assert_eq!(super::keys_offered(&Ok(Response::SshKeys { keys: vec![key.clone(), key] })), Ok(2));
        assert_eq!(super::keys_offered(&Ok(Response::SshKeys { keys: Vec::new() })), Ok(0));
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
        assert!(before.let_go.is_empty());
        let kept = serde_json::to_string(&crate::ui::SavedAppState { workspaces: Vec::new(), let_go: vec!["host-1".into()], help: Vec::new() }).unwrap();
        assert_eq!(serde_json::from_str::<crate::ui::SavedAppState>(&kept).unwrap().let_go, ["host-1"]);
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
}

#[cfg(test)]
mod reconnect_tests;

/// What Passwords' answer to a host's asking for the SSH keys offered
/// it: how many, or why none could be had, as a person reads it.
fn keys_offered(answer: &Result<olo_keychain::Response, olo_keychain::CallError>) -> Result<usize, String> {
    use olo_keychain::Response;
    match answer {
        Ok(Response::SshKeys { keys }) => Ok(keys.len()),
        Ok(Response::Locked) => Err(olo_kit::words::tr("Passwords is locked").into()),
        Ok(Response::NotSetUp) => Err(olo_kit::words::tr("Passwords isn’t set up").into()),
        Ok(Response::Error { message }) => Err(message.clone()),
        Ok(_) => Err(olo_kit::words::tr("Passwords answered something else").into()),
        Err(e) => Err(e.to_string()),
    }
}
