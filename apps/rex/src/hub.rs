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
    /// Desktop or remote host screens list/ownership/metadata changed.
    ScreensChanged,
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
    /// Bring Rex's Host window forward for `host_key` (requested via Rex's
    /// local screens socket `{"op":"open_host","host":"..."}`).
    OpenHost(String),
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
    /// Whether a stream is had for it ([`attaches_a_stream`], said of
    /// it by its session's view): what the streams opened beforehand
    /// for its host are counted by ([`terminals_of`]).
    pub is_terminal: bool,
    pub terminal: GhosttyTerminal,
    pub render_state: RenderState,
    pub frame: crate::frame::Frame,
    pub current_theme: TerminalTheme,
    pub search: TerminalSearch,
    find_query: Option<String>,
    find_generation: u64,
    pub finding: bool,
    pub geometry: Option<(u16, u16, u32, u32)>,
    /// The size its host is to give the terminal, and how far that has
    /// got ([`RexHub::resize_block`]).
    pub size: SizeAsked,
    pub other_size_owner: bool,
    /// Its size asked to be this client's, the answer not in yet: asked
    /// once (a window dragged asked again at every step, each a round
    /// trip ahead of what was typed next).
    owning: bool,
    /// The event number of the last word of whose its size is (an owner
    /// change, or a session's view): an older view's is not taken over a
    /// newer one's.
    owner_seq: u64,
    /// Woken when the view wants a new size while its asker waits to ask
    /// again ([`RexHubInner::ask_size`]).
    size_wake: Arc<tokio::sync::Notify>,
    pub copy_mode: Option<CopyMode>,
    pub title: String,
    /// The name the server has for the pane itself ("shell"), which its
    /// title starts as; a title unlike it is one a program gave.
    pub label: String,
    pub pwd: String,
    pub program_statuses: Vec<ProgramStatusRecord>,
    // Events can arrive while the initial program_status request is in flight.
    pending_status: Option<Vec<(u64, StatusUpdate)>>,
    /// The number of the last status request ([`Self::begin_status`]).
    status_asked: u64,
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
    /// When this pane was first seen by Rex (for bounded ownership resolution
    /// of newly started minded subagents).
    created_at: Instant,
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
    /// It is unread for one reason only: its agent asked leave
    /// ([`BlockRuntime::marked_unread`]). Such a mark goes when the
    /// agent no longer asks.
    unread_asked: bool,
    pub unread_cleared_at: Option<Instant>,
    /// Monotonic counter incremented whenever activity changes or a new
    /// `Blocked` status record replaces a prior one, so minder-detach
    /// reconciliation only delivers an approval that is still open and unseen.
    blocked_gen: u64,
    /// `blocked_gen` that was already notified or observed in the front pane.
    delivered_blocked_gen: Option<u64>,
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

/// A terminal's size: columns, rows, and a cell's width and height in
/// pixels.
pub type TermSize = (u16, u16, u32, u32);

/// The size a pane's host is to give its terminal, and how far that has
/// got ([`RexHub::resize_block`]): one request at a time (two sent
/// together may be done there in either order), the last size wanted
/// asked last, and none taken as given until the host says so (its
/// answer, or its own word of its size).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeAsked {
    /// What the view wants.
    wanted: Option<TermSize>,
    /// What the host has, as it last said or answered; none where that
    /// isn't known (an answer that didn't come: the size may have been
    /// given, or not).
    host: Option<TermSize>,
    /// Asked of the host, its answer not in yet.
    asking: Option<TermSize>,
    /// The host said its size while `asking` was on its way: the answer,
    /// come after, isn't taken over that word.
    said_meanwhile: bool,
    /// Asks of the size wanted now that failed in a row.
    failures: u32,
    /// An asker runs for the pane (one at a time: a size wanted while it
    /// waits to ask again wakes it, `BlockRuntime::size_wake`).
    asker: bool,
}

/// Asks of a pane's size that failed in a row before it is left (until
/// the view's size next changes): about a minute, at
/// [`attach_again_after`]'s pace.
const SIZE_TRIES: u32 = 8;

/// What a pane's size asker does next ([`BlockRuntime::size_answered`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizeStep {
    /// Nothing more (stopped: the host has the size, or the size is
    /// another's, or no try is left).
    Done,
    /// Ask this size now.
    Ask(TermSize),
    /// Ask again after the wait for so many failures in a row.
    Wait(u32),
}

/// How the host answered being asked a size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizeAnswer {
    Given,
    /// No answer that says (a time-out, the connection's trouble): the
    /// size may have been given or not.
    Unsure,
    /// Refused as such (a request it doesn't take, a pane gone).
    Refused,
}

impl SizeAsked {
    fn want(&mut self, size: TermSize) {
        if self.wanted != Some(size) {
            self.wanted = Some(size);
            self.failures = 0;
        }
    }

    /// Whether the view's `size` is a size to hand the hub: a new one,
    /// or one its host isn't known to have that nobody is asking for
    /// (the view's grid may be that size already: it is made so here at
    /// once, its host asked after).
    pub fn needs_ask(&self, size: TermSize) -> bool {
        self.wanted != Some(size) || (self.host != Some(size) && !self.asker && self.failures < SIZE_TRIES)
    }

    /// The size to ask the host for now, if any (it is then on its way):
    /// what is wanted, where the host isn't known to have it and nothing
    /// is on its way already.
    fn next(&mut self) -> Option<TermSize> {
        let wanted = self.wanted?;
        if self.asking.is_some() || self.host == Some(wanted) || self.failures >= SIZE_TRIES {
            return None;
        }
        self.asking = Some(wanted);
        self.said_meanwhile = false;
        Some(wanted)
    }

    /// The host's answer to being asked `asked`. Whether it was a failure
    /// of the size wanted now (one of a size wanted no longer is no
    /// failure of the new one's).
    fn answered(&mut self, asked: TermSize, answer: SizeAnswer) -> bool {
        if self.asking == Some(asked) {
            self.asking = None;
        }
        let said = std::mem::take(&mut self.said_meanwhile);
        let current = self.wanted == Some(asked);
        match answer {
            SizeAnswer::Given => {
                if !said {
                    self.host = Some(asked);
                }
                if current {
                    self.failures = 0;
                }
                false
            }
            SizeAnswer::Unsure => {
                self.host = None;
                if current {
                    self.failures += 1;
                }
                current
            }
            SizeAnswer::Refused => {
                if current {
                    self.failures = SIZE_TRIES;
                }
                current
            }
        }
    }

    /// The host's own word of its size (its stream's resize).
    fn host_said(&mut self, size: TermSize) {
        self.host = Some(size);
        if self.asking.is_some() {
            self.said_meanwhile = true;
        }
    }
}

/// The panes `window` shows whose size another client holds (whose size
/// the host keeps to that client's view), not asked for already: never
/// one it hides ([`panes_shown`]).
fn panes_sized_by_others(host_key: &str, session_id: &str, window: &olo_rex_client::WindowView, blocks: &HashMap<BlockKey, Arc<Mutex<BlockRuntime>>>) -> Vec<String> {
    panes_shown(window).into_iter().filter(|id| sized_by_another(host_key, session_id, id, blocks)).map(str::to_owned).collect()
}

/// The panes a tab shows, as Rex draws it (`ui::PanesView`): the one
/// zoomed, else those of its tiled layer's layout (its first alone
/// where it has none); and over them each floating layer's, but for one
/// put away (minimized: a button to bring it back is all that shows of
/// it). What taking a tab's panes takes (choosing it, bringing its
/// window forward, its window's size or scale changed): a pane hidden
/// here may be shown at its size on another desktop, and was taken
/// from it all the same (Astra R2 #13, Neon R2 #7).
pub(crate) fn panes_shown(window: &olo_rex_client::WindowView) -> Vec<&str> {
    use olo_rex_client::{LayerKind, LayerState};
    fn of_layout<'a>(node: &'a olo_rex_client::LayoutNode, shown: &mut Vec<&'a str>) {
        if let Some(id) = &node.block_id {
            shown.push(id);
        }
        if let Some(split) = &node.split {
            of_layout(&split.before, shown);
            of_layout(&split.after, shown);
        }
    }
    let mut shown = Vec::new();
    if let Some(zoomed) = &window.zoomed_block_id {
        shown.push(zoomed.as_str());
    } else if let Some(tiled) = window.layers.iter().find(|l| l.layer_kind() == LayerKind::Tiled) {
        match &tiled.layout {
            Some(root) => of_layout(root, &mut shown),
            None => shown.extend(tiled.blocks.first().map(|b| b.block_id.as_str())),
        }
    }
    for floating in window.layers.iter().filter(|l| l.layer_kind() == LayerKind::Floating && l.layer_state() != LayerState::Minimized) {
        shown.extend(floating.blocks.first().map(|b| b.block_id.as_str()));
    }
    shown
}

/// Whether a pane's size is another client's, and not asked for already.
fn sized_by_another(host_key: &str, session_id: &str, block_id: &str, blocks: &HashMap<BlockKey, Arc<Mutex<BlockRuntime>>>) -> bool {
    let key = BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: block_id.to_owned() };
    blocks.get(&key).is_some_and(|runtime| {
        let g = runtime.lock().unwrap_or_else(|e| e.into_inner());
        g.other_size_owner && !g.owning
    })
}

/// `block_ids`' size asked to be this client's, each marked as asked
/// (`BlockRuntime::owning`) until the host answers: asked once however
/// often a window asks. Whether it was asked.
fn ask_to_own(host: &HostRuntime, session_id: &str, block_ids: Vec<String>, blocks: &HashMap<BlockKey, Arc<Mutex<BlockRuntime>>>) -> bool {
    let Some(tx) = host.input_tx.as_ref().filter(|_| !block_ids.is_empty()) else { return false };
    let mark = |on: bool| {
        for id in &block_ids {
            let key = BlockKey { host_key: host.key.clone(), session_id: session_id.to_owned(), block_id: id.clone() };
            if let Some(block) = blocks.get(&key) {
                block.lock().unwrap_or_else(|e| e.into_inner()).owning = on;
            }
        }
    };
    mark(true);
    let sent = tx.send(HostInputOp::Own { session_id: session_id.to_owned(), block_ids: block_ids.clone() }).is_ok();
    if !sent {
        mark(false);
    }
    sent
}

impl BlockRuntime {
    pub fn new(cols: u16, rows: u16, theme: &TerminalTheme) -> Self {
        let mut terminal = GhosttyTerminal::new(cols.max(2), rows.max(1)).expect("libghostty-vt terminal allocation");
        let _ = terminal.apply_theme(theme);
        let render_state = RenderState::new().expect("libghostty-vt render state");
        let search = TerminalSearch::new(&terminal).expect("libghostty-vt search");
        Self {
            creator_name: "com.superlogical.terminal".to_owned(),
            is_terminal: true,
            terminal,
            render_state,
            frame: crate::frame::Frame::new(),
            current_theme: theme.clone(),
            search,
            find_query: None,
            find_generation: 0,
            finding: false,
            geometry: None,
            size: SizeAsked::default(),
            other_size_owner: false,
            owning: false,
            owner_seq: 0,
            size_wake: Arc::new(tokio::sync::Notify::new()),
            copy_mode: None,
            title: String::new(),
            label: String::new(),
            pwd: String::new(),
            program_statuses: Vec::new(),
            pending_status: None,
            status_asked: 0,
            snapshot_seq: 0,
            ports: PortsSnapshot::default(),
            process: None,
            process_seq: 0,
            ports_seq: 0,
            agent_activity: AgentActivity::Idle,
            guess_wanted: false,
            last_output: Instant::now(),
            created_at: Instant::now(),
            watch: Watch::new(Instant::now()),
            mouse: None,
            mouse_held: None,
            watch_timer: None,
            watch_ticker: None,
            unread: false,
            unread_asked: false,
            unread_cleared_at: None,
            blocked_gen: 0,
            delivered_blocked_gen: None,
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
        // (A new stream's screen is the host's size now, whatever it was
        // asked before: the view's size is asked again, not taken as had;
        // with its tries anew, those spent being the last stream's, and an
        // answer on its way to an ask of before it taken as saying nothing
        // of the host's size now, as one after the host's own word: Astra
        // R2 #14, a pane whose host timed out eight asks kept its old size
        // until its window's next changed.)
        self.size.host = None;
        self.size.failures = 0;
        if self.size.asking.is_some() {
            self.size.said_meanwhile = true;
        }
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
                // (Whose its size is, from its stream, only until the host
                // has numbered a word of it (an owner change, a session's
                // view, a claim's answer): the stream's comes in its own
                // time, unnumbered, and a resize sent before a newer owner
                // change undid it: Astra R2 #11.)
                if self.resize(cols, rows, cell_width_px, cell_height_px).is_ok() && self.owner_seq == 0 {
                    self.other_size_owner = owner.is_some() && owner.as_deref() != client;
                }
                // (The host's own word of its size: where it isn't what this
                // view wants, the view asks again at its next paint.)
                self.size.host_said((cols, rows, cell_width_px, cell_height_px));
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
    fn blocked_record(&self) -> Option<&ProgramStatusRecord> {
        AgentActivity::from_records(&self.program_statuses).and_then(|(act, rec)| (act == AgentActivity::Blocked).then_some(rec))
    }

    fn current_blocked_notice(&self) -> Option<(u64, String)> {
        if self.agent_activity != AgentActivity::Blocked || self.muted || self.delivered_blocked_gen == Some(self.blocked_gen) {
            return None;
        }
        let record = self.blocked_record()?;
        let body = record.msg.clone().or_else(|| record.title.clone()).unwrap_or_else(|| self.title.clone());
        Some((self.blocked_gen, body))
    }

    /// The host's answer to being asked `asked` its size, and what its
    /// asker does next: the next size at once; after a failure of the
    /// size wanted now, the same a little later, but not after the last
    /// try; nothing once the size is another client's (whose answer `{}`
    /// says nothing of the host's size: it kept the size as a wish).
    fn size_answered(&mut self, asked: TermSize, answer: SizeAnswer) -> SizeStep {
        if self.other_size_owner {
            self.size.asking = None;
            self.size.said_meanwhile = false;
            self.size.asker = false;
            return SizeStep::Done;
        }
        let failed = self.size.answered(asked, answer);
        if failed && self.size.failures >= SIZE_TRIES {
            self.size.asker = false;
            return SizeStep::Done;
        }
        if failed {
            return SizeStep::Wait(self.size.failures);
        }
        match self.size.next() {
            Some(next) => SizeStep::Ask(next),
            None => {
                self.size.asker = false;
                SizeStep::Done
            }
        }
    }

    /// Whose its size is, as the host said at event number `seq` (an
    /// owner change, or a session's view: `0` where the number isn't
    /// known): taken unless a later word was had already.
    fn owner_said(&mut self, seq: u64, owner: Option<&str>, client: Option<&str>) {
        if seq != 0 && seq < self.owner_seq {
            return;
        }
        self.owner_seq = self.owner_seq.max(seq);
        self.other_size_owner = owner.is_some_and(|o| Some(o) != client);
    }

    /// Its programs' status is being asked for (a stream had, events
    /// missed): what changes meanwhile is kept, to be replayed over the
    /// answer. The request's number: only the last one asked
    /// ([`Self::status_answered`]) applies its answer, so an earlier
    /// one's answer, come later, neither spends what the later one
    /// keeps nor undoes what that replayed.
    fn begin_status(&mut self) -> u64 {
        self.status_asked = self.status_asked.wrapping_add(1);
        self.pending_status = Some(Vec::new());
        self.status_asked
    }

    /// The answer to the status request numbered `asked`.
    fn status_answered(&mut self, asked: u64, snapshot: Option<olo_rex_client::RpcResult<Vec<ProgramStatusRecord>>>) {
        if asked == self.status_asked {
            self.status_snapshot(snapshot);
        }
    }

    fn status_snapshot(&mut self, snapshot: Option<olo_rex_client::RpcResult<Vec<ProgramStatusRecord>>>) {
        if snapshot.as_ref().is_some_and(|r| r.seq < self.snapshot_seq) {
            // (Older than what is had: nothing to replay over it either.)
            self.pending_status = None;
            return;
        }
        let prev_blocked = self.blocked_record().cloned();
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
        let now = self.recompute_activity();
        if now == AgentActivity::Blocked && prev_blocked.as_ref() != self.blocked_record() {
            self.blocked_gen = self.blocked_gen.wrapping_add(1);
        }
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

    /// The pane is unread: its agent asks leave or failed (`now`), or
    /// its bell rang (None).
    pub fn marked_unread(&mut self, now: Option<AgentActivity>) {
        self.unread_asked = now == Some(AgentActivity::Blocked) && (!self.unread || self.unread_asked);
        self.unread = true;
    }

    /// Its agent went from `was` to `now`. **A pane marked unread
    /// because its agent asked leave is unread no longer once the agent
    /// no longer asks**: somebody answered (its minder, its parent, its
    /// own Auto Approve, the person in another window), and there is
    /// nothing left there to see. The mark was cleared only by putting
    /// that very pane in front: a tab's dot, which is its own panes'
    /// and its child tabs', stayed lit for as long as any child had
    /// ever asked anything, with the tab in front and nothing to do
    /// (Ben, 2026-11-27, of a lead's tab whose helpers' minders had
    /// answered every question in seconds: "why does this tab have a
    /// red dot? and it doesn't go away when i am on it either"). A
    /// failure, or a bell, is news until it is seen, as before.
    fn activity_went(&mut self, was: AgentActivity, now: AgentActivity) {
        if was != now {
            self.blocked_gen = self.blocked_gen.wrapping_add(1);
        }
        if was == AgentActivity::Blocked && now != AgentActivity::Blocked && self.unread && self.unread_asked {
            self.unread = false;
            self.unread_asked = false;
            self.unread_cleared_at = Some(Instant::now());
        }
    }

    pub fn recompute_activity(&mut self) -> AgentActivity {
        let was = self.agent_activity;
        let now = self.activity_now();
        self.activity_went(was, now);
        now
    }

    fn activity_now(&mut self) -> AgentActivity {
        if let Some((act, _)) = AgentActivity::from_records(&self.program_statuses) {
            // The server's word that it works is its program's own, and
            // Claude Code goes on saying so while a shell it started is
            // still running, its turn long over: a pane read Working for
            // as long as its dev server ran, and what it had asked sat
            // unseen (the same word misled the hosts' agents service:
            // ShiftUp's item 24, 2026-11-26). An agent whose screen we
            // read says itself whether a turn is on: at its prompt, with
            // nothing saying so, it is at rest.
            let act = if act == AgentActivity::Working && self.turn_is_over() { AgentActivity::Idle } else { act };
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

    /// Whether its agent's own screen says no turn is on: one of the
    /// agents whose screens are read (`olo_agent_screen`), standing at
    /// its prompt with nothing that says it works or asks.
    fn turn_is_over(&self) -> bool {
        let screen = self.screen_bottom(ACTIVITY_LOOK);
        let (name, exe, args) = self.agent_program();
        which_agent(name.as_deref(), &exe, &args, &screen)
            .is_some_and(|agent| olo_agent_screen::classify(agent, &screen) == olo_agent_screen::State::Idle && olo_agent_screen::at_prompt(agent, &screen))
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
    /// These panes' size taken from whoever holds it ([`RexHub::select`]).
    Own {
        session_id: String,
        block_ids: Vec<String>,
    },
}

/// Window focus and lifecycle operations serialized on their own per-host FIFO
/// channel so `Write`/`Key` typing on any session never stalls behind tab
/// creation, CWD lookup, or placement RPCs.
enum HostWindowOp {
    Focus {
        session_id: String,
        switch: bool,
        target: Arc<FocusTarget>,
    },
    CreateWindow {
        session_id: String,
        explicit_cwd: Option<String>,
        source_root_id: Option<String>,
        active_block_id: Option<String>,
        known_cwds: Vec<String>,
        base_opts: Option<Value>,
        feedback: Option<Feedback>,
        on_created: Option<Box<dyn FnOnce() + Send>>,
    },
}

/// Tracks per-session `(original_enqueued_root, last_created_window_id)` so
/// rapid consecutive `Cmd+T` presses chain left-to-right (`[A, N1, N2, B]`)
/// rather than reversing into `[A, N2, N1, B]`, resetting on any intervening
/// explicit tab selection (`Focus`).
#[derive(Default)]
struct CreateChainTracker {
    by_session: HashMap<String, (String, String)>,
}

impl CreateChainTracker {
    fn resolve_source_root(&self, session_id: &str, enqueued_root: Option<String>) -> Option<String> {
        let root = enqueued_root?;
        if let Some((orig, last_new)) = self.by_session.get(session_id)
            && orig == &root
        {
            return Some(last_new.clone());
        }
        Some(root)
    }

    fn record_created(&mut self, session_id: &str, enqueued_root: Option<&str>, new_window_id: &str) {
        if let Some(root) = enqueued_root {
            self.by_session.insert(session_id.to_owned(), (root.to_owned(), new_window_id.to_owned()));
        }
    }

    fn on_focus(&mut self, session_id: &str) {
        self.by_session.remove(session_id);
    }
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

/// Top-level ancestor window ID of the currently active tab in `sess`.
/// Validates `active_window_id` against `sess.windows` (falling back to the
/// first window, matching `active_session_block`).
pub fn active_top_level_window(subagents: &crate::subagents::SubagentRegistry, host_key: &str, sess: &SessionView) -> Option<String> {
    let live: HashSet<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
    let active_raw = sess.active_window().or_else(|| sess.windows.first())?.window_id.as_str();
    Some(subagents.root_ancestor(host_key, &sess.session_id, active_raw, &live))
}

/// Which top-level window stands immediately to the right of `source_root_id`
/// in `sess`, ignoring `exclude_window_id` (the newly created tab itself).
/// Returns `None` if `source_root_id` is gone or is already the rightmost
/// top-level tab.
pub fn successor_top_level_window(subagents: &crate::subagents::SubagentRegistry, host_key: &str, sess: &SessionView, source_root_id: &str, exclude_window_id: Option<&str>) -> Option<String> {
    let live: HashSet<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
    let top_ids: Vec<&str> = sess
        .windows
        .iter()
        .filter(|w| Some(w.window_id.as_str()) != exclude_window_id && !subagents.is_child_window(host_key, &sess.session_id, &w.window_id, &live))
        .map(|w| w.window_id.as_str())
        .collect();
    let pos = top_ids.iter().position(|&id| id == source_root_id)?;
    top_ids.get(pos + 1).map(|&id| id.to_owned())
}

/// Compute whether `new_wid` needs a `session.move_window` call to stand
/// immediately to the right of `source_root_id` among top-level tabs in `sess`.
///
/// - `None`: already adjacent right of `source_root_id` (or `source_root_id` /
///   `new_wid` is no longer present in `sess`).
/// - `Some(Some(before_wid))`: move `new_wid` immediately before `before_wid`.
/// - `Some(None)`: move `new_wid` to the end (`before_window_id = None`),
///   because `source_root_id` is the rightmost other top-level tab while
///   `new_wid` currently stands to its left.
pub fn desired_move_for_new_tab(subagents: &crate::subagents::SubagentRegistry, host_key: &str, sess: &SessionView, source_root_id: &str, new_wid: &str) -> Option<Option<String>> {
    let live: HashSet<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
    let all_top: Vec<&str> = sess.windows.iter().filter(|w| !subagents.is_child_window(host_key, &sess.session_id, &w.window_id, &live)).map(|w| w.window_id.as_str()).collect();
    let src_pos = all_top.iter().position(|&id| id == source_root_id)?;
    let new_pos = all_top.iter().position(|&id| id == new_wid)?;
    if new_pos == src_pos + 1 {
        return None;
    }
    let other_top: Vec<&str> = all_top.into_iter().filter(|&id| id != new_wid).collect();
    let src_other = other_top.iter().position(|&id| id == source_root_id)?;
    Some(other_top.get(src_other + 1).map(|&id| id.to_owned()))
}

/// Which top-level window a new tab opened from the active tab should be placed
/// immediately before (`None`: append at the end of the tab strip).
///
/// When the active tab is a nested child/subagent tab, walks up to its
/// top-level ancestor and returns the next top-level tab to its right (after
/// any child tabs grouped under that ancestor).
#[cfg(test)]
pub fn new_tab_before_window(subagents: &crate::subagents::SubagentRegistry, host_key: &str, sess: &SessionView) -> Option<String> {
    let root_id = active_top_level_window(subagents, host_key, sess)?;
    successor_top_level_window(subagents, host_key, sess, &root_id, None)
}

fn active_session_block(sess: &SessionView) -> Option<&str> {
    let win = sess.active_window().or_else(|| sess.windows.first())?;
    win.focused_block_id.as_deref().filter(|id| win.all_blocks().any(|b| b.block_id == *id)).or_else(|| win.all_blocks().next().map(|b| b.block_id.as_str()))
}

fn is_usable_cwd(dir: &str) -> bool {
    !dir.is_empty() && !dir.ends_with(" (deleted)")
}

/// What a pane's Open in Tracker is checked against
/// ([`RexHub::tracker_place`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPin {
    /// The host as it is now: a command that acts at once (the menu
    /// bar's, a pane's own menu).
    Any,
    /// The host a menu was made for, as `RexHub::tracker_pin` gave it
    /// then: nothing opens where the host is another, or reconnected,
    /// since; and none (no pin could be had) opens nothing, never as if
    /// unpinned.
    MustMatch(Option<(u64, u64)>),
}

/// How Rex names a host by its key in a Tracker `rex://` URI
/// (`rex://id:KEY/…`): the files service takes it as that host's key
/// alone; a name without it as it always has (a host's label first, so
/// a bookmark made before goes where it went).
pub(crate) const KEY_AUTHORITY: &str = "id:";

/// How a host is named in a Tracker `rex://` URI (and in the files
/// service's answers, which Tracker names the folder by from then on):
/// by its label, as a person reads it, where that names it alone; else
/// by its key, tagged so ([`KEY_AUTHORITY`]): where its label is another
/// host's too, or this computer's reserved name (`rex://local/…` is a
/// path here), or empty, or tagged so itself. (Untagged, a key was
/// looked up first, and a bookmark of a host whose label was another's
/// key went to that other: Astra R2 #2.)
pub(crate) fn tracker_authority(hosts: &HashMap<String, HostRuntime>, host: &HostRuntime) -> String {
    let label = host.label.trim();
    let reserved = label.is_empty() || label.eq_ignore_ascii_case(olo_kit_base::tracker::LOCAL_HOST) || label.starts_with(KEY_AUTHORITY);
    let shared = hosts.values().any(|other| other.key != host.key && !other.is_local && other.label == host.label);
    if reserved || shared { format!("{KEY_AUTHORITY}{}", host.key) } else { host.label.clone() }
}

/// Where a pane's Open in Tracker opens ([`RexHub::tracker_place`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackerPlace {
    /// A folder of this computer's.
    Here(PathBuf),
    /// A folder of a host's, as Tracker reaches it (`rex://HOST/PATH`).
    There(String),
}

/// The folder a pane's Open in Tracker opens: where its program works
/// now (`live`: its folder as the terminal or its host said, an
/// agent's worktree); else where the agent there was started
/// (`started_in`, the agents service's word: a child that has exited,
/// or whose host hasn't said its folder yet); else the home folder
/// (`home`, this computer's; a host's own `~`). A host's as Tracker
/// names it (`authority`, [`tracker_authority`]).
pub(crate) fn tracker_place(is_local: bool, authority: &str, live: &[String], started_in: Option<&str>, home: Option<&str>) -> TrackerPlace {
    let dir = live.iter().map(String::as_str).find(|d| is_usable_cwd(d)).or(started_in.filter(|d| is_usable_cwd(d)));
    if !is_local {
        return TrackerPlace::There(olo_kit_base::tracker::RexPath::new(authority, dir.unwrap_or("~")).to_uri());
    }
    // (A folder said from home, `~/…` or relative, as a host's is read.)
    let home = home.filter(|h| !h.is_empty()).unwrap_or("/");
    TrackerPlace::Here(match dir {
        Some("~") | None => PathBuf::from(home),
        Some(d) if d.starts_with('/') => PathBuf::from(d),
        Some(d) => Path::new(home).join(d.strip_prefix("~/").unwrap_or(d)),
    })
}

fn push_unique_cwd(out: &mut Vec<String>, dir: &str) {
    if is_usable_cwd(dir) && !out.iter().any(|existing| existing == dir) {
        out.push(dir.to_owned());
    }
}

/// Ordered deduplicated CWD candidates from a `TerminalProcessSnapshot`:
/// foreground CWD first, followed by a distinct shell (`child`) CWD fallback,
/// filtering out empty paths and kernel `" (deleted)"` markers.
fn cwds_from_process(process: Option<&TerminalProcessSnapshot>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(p) = process else { return out };
    if let Some(dir) = p.foreground.as_ref().and_then(|info| info.cwd.as_deref()) {
        push_unique_cwd(&mut out, dir);
    }
    if let Some(dir) = p.child.as_ref().and_then(|info| info.cwd.as_deref()) {
        push_unique_cwd(&mut out, dir);
    }
    out
}

#[cfg(test)]
fn cwd_from_process(process: Option<&TerminalProcessSnapshot>) -> Option<String> {
    cwds_from_process(process).into_iter().next()
}

fn block_cwd_candidates(pwd: &str, process: Option<&TerminalProcessSnapshot>) -> Vec<String> {
    let mut out = Vec::new();
    push_unique_cwd(&mut out, pwd);
    for dir in cwds_from_process(process) {
        push_unique_cwd(&mut out, &dir);
    }
    out
}

/// Bounded deadline for querying live process CWD from the host before
/// creating a new tab (never wait for the 30 s default RPC timeout).
const CREATE_WINDOW_CWD_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounded deferral for `WindowCreated` event publication while awaiting our
/// own `session.new_window` reply so the placement guard can arm for the exact
/// `new_window_id` before any unmoved creation view paints.
const CREATE_WINDOW_EVENT_DEFER: Duration = Duration::from_millis(250);

/// Resolve ordered CWD candidates for a newly created tab:
/// - If an explicit `cwd` was passed, use ONLY that explicit directory without
///   consulting the active pane.
/// - Otherwise query live process CWDs (`[foreground, shell]`) from the host
///   that owns the PTY first under `timeout`, falling back to cached
///   `known_cwds` only when live lookup returns empty or times out.
async fn resolve_new_window_cwd_candidates<F, Fut>(explicit_cwd: Option<String>, cached_cwds: Vec<String>, timeout: Duration, live_lookup: F) -> Vec<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    if let Some(dir) = explicit_cwd.filter(|d| !d.is_empty()) {
        return vec![dir];
    }
    let live: Vec<String> = tokio::time::timeout(timeout, live_lookup()).await.ok().unwrap_or_default().into_iter().filter(|d| is_usable_cwd(d)).collect();
    if !live.is_empty() { live } else { cached_cwds.into_iter().filter(|d| is_usable_cwd(d)).collect() }
}

/// Whether a failed `session.new_window` that passed a `cwd` may safely retry
/// with the next fallback directory (or default server cwd). Allowlists ONLY
/// verified pre-creation working-directory rejections — never retries
/// `not_found`, `session_not_found`, window-limit errors, generic internal
/// errors, timeouts, or transport errors.
fn should_retry_new_window_without_cwd(err: &olo_rex_client::RexError, used_cwd: bool) -> bool {
    if !used_cwd {
        return false;
    }
    let olo_rex_client::RexError::Api(api) = err else {
        return false;
    };
    let code = api.code.to_ascii_lowercase();
    if matches!(code.as_str(), "invalid_cwd" | "cwd_not_found") {
        return true;
    }
    if matches!(code.as_str(), "invalid_argument" | "invalid_params" | "bad_request" | "invalid_option") {
        let msg = api.message.to_ascii_lowercase();
        return msg.contains("cwd")
            || msg.contains("working directory")
            || msg.contains("chdir")
            || msg.contains("no such file or directory")
            || msg.contains("not a directory")
            || msg.contains("directory");
    }
    false
}

/// Try `session.new_window` across ordered `cwd_candidates` (e.g.
/// `[foreground_cwd, shell_cwd]`), stepping to the next candidate and finally
/// to `None` (server default) ONLY on a verified pre-creation directory
/// rejection.
async fn create_window_rpc_candidates<F, Fut>(cwd_candidates: &[String], base_opts: Option<Value>, mut call: F) -> Result<olo_rex_client::RpcResult<Value>, olo_rex_client::RexError>
where
    F: FnMut(Option<Value>) -> Fut,
    Fut: std::future::Future<Output = Result<olo_rex_client::RpcResult<Value>, olo_rex_client::RexError>>,
{
    if cwd_candidates.is_empty() {
        return call(base_opts).await;
    }
    for dir in cwd_candidates {
        let opts = shell_opts_with_cwd(base_opts.clone(), Some(dir.as_str()));
        match call(opts).await {
            Ok(res) => return Ok(res),
            Err(ref err) if should_retry_new_window_without_cwd(err, true) => continue,
            Err(err) => return Err(err),
        }
    }
    call(base_opts).await
}

fn timeout_problem_detail(refreshed: bool) -> &'static str {
    if refreshed { "The server didn’t answer in time; refreshed the session." } else { "The server didn’t answer in time." }
}

fn shell_opts_with_cwd(base: Option<Value>, cwd: Option<&str>) -> Option<Value> {
    let Some(dir) = cwd.filter(|d| !d.is_empty()) else {
        return base;
    };
    let mut opts = base.unwrap_or_else(|| json!({}));
    opts["cwd"] = json!(dir);
    Some(opts)
}

/// An in-flight new-tab placement for the exact `new_window_id` returned by
/// THIS request: keeps intermediate `WindowCreated` / `SessionViewChanged`
/// snapshots arriving while `session.move_window` is in flight from flickering
/// the new tab at the end of the strip first, without ever touching windows
/// created by other clients or concurrent requests.
struct PendingWindowPlacement {
    token: u64,
    new_window_id: String,
    before_window_id: String,
}

fn next_placement_token() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn remove_placement_token(map: &mut HashMap<String, Vec<PendingWindowPlacement>>, session_id: &str, token: u64) {
    if let Some(list) = map.get_mut(session_id) {
        list.retain(|p| p.token != token);
        if list.is_empty() {
            map.remove(session_id);
        }
    }
}

fn apply_pending_placements(view: &mut SessionView, placements: &[PendingWindowPlacement]) -> bool {
    let mut changed = false;
    for p in placements {
        let Some(before_pos) = view.windows.iter().position(|w| w.window_id == p.before_window_id) else {
            continue;
        };
        if let Some(cur) = view.windows.iter().position(|w| w.window_id == p.new_window_id)
            && cur > before_pos
        {
            let win = view.windows.remove(cur);
            view.windows.insert(before_pos, win);
            changed = true;
        }
    }
    changed
}

struct PlacementGuard {
    inner: Arc<RexHubInner>,
    hk: String,
    sid: String,
    token: Option<u64>,
}

impl PlacementGuard {
    /// Register or update the pending placement for `(new_window_id, before_window_id)`
    /// and retroactively apply it to `host.sessions[sid]` if an early `WindowCreated`
    /// view already installed `new_window_id` at the end, notifying the UI if order changed.
    fn set(&mut self, new_window_id: String, before_window_id: String) {
        let changed = {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get_mut(&self.hk) else { return };
            let mut updated = false;
            if let Some(token) = self.token
                && let Some(list) = host.pending_placements.get_mut(&self.sid)
                && let Some(entry) = list.iter_mut().find(|p| p.token == token)
            {
                entry.new_window_id = new_window_id.clone();
                entry.before_window_id = before_window_id.clone();
                updated = true;
            }
            if !updated {
                let token = next_placement_token();
                host.pending_placements.entry(self.sid.clone()).or_default().push(PendingWindowPlacement { token, new_window_id, before_window_id });
                self.token = Some(token);
            }
            if let Some(list) = host.pending_placements.get(&self.sid)
                && let Some(sess) = host.sessions.get_mut(&self.sid)
            {
                apply_pending_placements(sess, list)
            } else {
                false
            }
        };
        if changed {
            (self.inner.notify)(HubNotice::StateChanged);
        }
    }

    fn clear(&mut self) {
        if let Some(token) = self.token.take() {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(host) = st.hosts.get_mut(&self.hk) {
                remove_placement_token(&mut host.pending_placements, &self.sid, token);
            }
        }
    }

    fn take_token(&mut self) -> Option<u64> {
        self.token.take()
    }
}

impl Drop for PlacementGuard {
    fn drop(&mut self) {
        self.clear();
    }
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

/// What a host's agents service said a process may ask of this computer
/// (`RexHubInner::child_refusal`), by its host, the pane it named and
/// its number, and when: kept a minute, so a command that asks again
/// and again (`olo run`'s polling) asks the service once.
#[allow(clippy::type_complexity)]
static POLICIES_SAID: std::sync::LazyLock<std::sync::Mutex<HashMap<(String, String, u32), (Instant, Option<crate::subagents::DesktopPolicy>)>>> = std::sync::LazyLock::new(Default::default);

/// (As Rex comes back from an upgrade, before its hub starts.)
static SAVED_HOST_SCREENS: Mutex<Vec<crate::screens_sock::SavedHostScreens>> = Mutex::new(Vec::new());

pub fn set_saved_forwards(forwards: Vec<SavedForward>) {
    *SAVED_FORWARDS.lock().unwrap_or_else(|e| e.into_inner()) = forwards;
}

pub fn set_saved_host_screens(screens: Vec<crate::screens_sock::SavedHostScreens>) {
    *SAVED_HOST_SCREENS.lock().unwrap_or_else(|e| e.into_inner()) = screens;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardState {
    Active,
    Waiting,
    Failed(String),
}

/// Authoritative membership and the views attached on this connection. Some servers
/// bootstrap only the preferred view. Older peers support the list/attach calls.
/// (`seq`: the membership's number; each view has its own, the number of the
/// answer it came in: one attached after the list was stamped with the list's,
/// and an event between them, older than the view, was taken over it, and a
/// pane's owner said in the view was missed: Astra R2 #12.)
struct SessionSnapshot {
    seq: u64,
    rows: Vec<SessionSummary>,
    views: Vec<(SessionView, u64)>,
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
                    Some(attached) => attached.into_iter().map(|a| (a.view, r.seq)).collect(),
                    None => b.selected_session.into_iter().map(|a| (a.view, r.seq)).collect(),
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
            .filter(|row| !snapshot.views.iter().any(|(v, _)| v.session_id == row.session_id))
            .map(|row| {
                let (ctrl, id) = (ctrl.clone(), row.session_id.clone());
                tokio::spawn(async move { ctrl.session_attach(&id).await })
            })
            .collect();
        for attached in asked {
            match attached.await.map_err(|e| olo_rex_client::RexError::Closed(format!("attaching a session: {e}")))? {
                Ok(r) => snapshot.views.push((r.payload.view, r.seq)),
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
    /// Olo itself there, with Chrome, for Agent Screens of the host's
    /// own (put there by the host's helper from Olo's release:
    /// `host_screens`).
    Screens,
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
    /// What it has for Agent Screens of its own; none: not asked yet.
    pub screens: Option<crate::host_screens::ScreensThere>,
    /// How far putting Olo there is, while it is being put: in words,
    /// and of the whole (0 to 1).
    pub screens_doing: Option<(String, f32)>,
    /// This Olo's own release, if it is one: when it was made (a
    /// host's Olo from an earlier one can be updated) and its version.
    pub this_olo: Option<(u64, String)>,
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
    pub host_instance_id: u64,
    pub screens_conn_id: u64,
    pub screens_state_seq: u64,
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
    /// Agent Screens of its own (`host_screens`): what it has, and how
    /// far putting Olo there is.
    pub screens: Option<crate::host_screens::ScreensThere>,
    pub screens_doing: Option<(String, f32)>,
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
    pending_placements: HashMap<String, Vec<PendingWindowPlacement>>,
    pending_creates: HashMap<String, usize>,
    creating_defer: HashMap<String, (u64, Instant, Arc<tokio::sync::Notify>)>,
    pub keymap_bindings: Vec<KeymapBinding>,
    pub keymap_modes: Vec<KeymapMode>,
    pub keymap_diagnostics: Vec<KeymapDiagnostic>,
    pub control: Option<ControlClient>,
    input_tx: Option<tokio::sync::mpsc::UnboundedSender<HostInputOp>>,
    window_tx: Option<tokio::sync::mpsc::UnboundedSender<HostWindowOp>>,
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
    /// The children of the hosts' tabs (`olo agent`), as each host's agents
    /// service says them (`follow_agents`): theirs to keep, shown here.
    /// Nothing of it is this computer's to decide or write down.
    pub subagents: crate::subagents::SubagentRegistry,
    /// The hosts whose service has said its children since they connected.
    pub agents_told: HashSet<String>,
    /// Monotonic generation per host for `follow_agents` so superseded
    /// followers can never overwrite or clear a newer follower's confirmation.
    agents_follow_gen: HashMap<String, u64>,
    /// Hosts with a fresh follower generation awaiting its first `Frame::Snapshot`
    /// (bounded by `NEW_PANE_OWNERSHIP_GRACE`).
    agents_snapshot_pending: HashMap<String, Instant>,
    /// Latest live state reported by the host's agents follower per `(host_key, agent_id)`.
    agent_states: HashMap<(String, String), olo_rex_link::agents::SubagentLiveState>,
    /// Controlling Rex pane for each desktop Agent Screen (from `NAME.owner.json`
    /// and bridged `olo.screen` calls), keyed by screen name.
    pub desktop_screen_owners: HashMap<String, olo_rex_link::screen_wire::ScreenOwner>,
    /// Live local desktop screens (running + owned stopped) scanned off the UI thread.
    pub desktop_screens: Vec<olo_screen_pane::rex_sock::RexScreen>,
    /// True once the initial off-thread desktop screens scan has completed.
    pub desktop_screens_loaded: bool,
    /// Monotonic revision bumped whenever desktop/remote screens or their owners change,
    /// allowing UI windows to cache resolved screen inventories across redraws (Astra #23).
    pub screens_rev: u64,
    /// Monotonic revision bumped whenever hosts, connection states, session/window/block
    /// layouts, or subagent parent/membership topology change (Astra #25).
    pub topology_rev: u64,
    /// Live Agent Screens reported by each connected (or last-known disconnected)
    /// remote host's `olo-rex-link screen follow`, keyed by host key.
    pub host_screens: HashMap<String, crate::screens_sock::LiveHostScreens>,
    quick_look_cache: HashMap<(String, String, String), QuickLookCacheEntry>,
    /// Active front pane per focused Rex workspace window (so held notices are
    /// discarded if the user views the pane during the ownership hold).
    visible_panes: HashMap<crate::ui::WinId, BlockKey>,
    /// Candidate notices for newly seen panes (or panes on a host awaiting its
    /// initial follower snapshot) whose subagent ownership is unresolved.
    /// Stored as a per-pane list so an inline APC or bridge notice never
    /// overwrites a held `Blocked` approval.
    held_pane_notices: HashMap<BlockKey, Vec<HeldPaneNotice>>,
    held_notice_seq: u64,
    #[cfg(test)]
    test_now: Option<Instant>,
}

/// How long to hold a candidate notice for a brand-new pane (or a fresh
/// follower awaiting its initial snapshot) while waiting for ownership.
const NEW_PANE_OWNERSHIP_GRACE: Duration = Duration::from_millis(750);

#[derive(Clone, Debug)]
struct PendingPaneNotice {
    title: String,
    body: String,
    kind: olo_kit::notify::NoteKind,
}

struct HeldPaneNotice {
    notice: PendingPaneNotice,
    activity: Option<AgentActivity>,
    blocked_gen: Option<u64>,
    seq: u64,
    timer: Option<tokio::task::AbortHandle>,
}

impl HostRuntime {
    pub(crate) fn demo_connected(key: &str, label: &str, endpoint: &str, sessions: Vec<SessionView>) -> Self {
        Self {
            key: key.to_owned(),
            host_instance_id: crate::screens_sock::next_host_instance_id(),
            screens_conn_id: 1,
            screens_state_seq: 1,
            label: label.to_owned(),
            endpoint_raw: endpoint.to_owned(),
            endpoint: None,
            is_local: false,
            conn_state: HostConnState::Connected,
            connection_lost: None,
            client_id: None,
            reconnect_token: None,
            retry: None,
            server_status: None,
            server_version: None,
            server_newer: None,
            helper: HostHelper::Used,
            helper_current: Some(true),
            helper_sync_err: None,
            bridge_there: None,
            skill_there: None,
            skill_current: None,
            skill_sync_err: None,
            commands_there: None,
            commands_current: None,
            commands_sync_err: None,
            screens: None,
            screens_doing: None,
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
            pending_placements: HashMap::new(),
            pending_creates: HashMap::new(),
            creating_defer: HashMap::new(),
            keymap_bindings: Vec::new(),
            keymap_modes: Vec::new(),
            keymap_diagnostics: Vec::new(),
            control: None,
            input_tx: None,
            window_tx: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_connected(key: &str, sessions: HashMap<String, SessionView>) -> Self {
        Self {
            key: key.to_owned(),
            host_instance_id: crate::screens_sock::next_host_instance_id(),
            screens_conn_id: crate::screens_sock::next_conn_id(),
            screens_state_seq: 0,
            label: key.to_owned(),
            endpoint_raw: format!("ssh://{key}"),
            endpoint: None,
            is_local: false,
            conn_state: HostConnState::Connected,
            connection_lost: None,
            client_id: None,
            reconnect_token: None,
            retry: None,
            server_status: None,
            server_version: None,
            server_newer: None,
            helper: HostHelper::Used,
            helper_current: Some(true),
            helper_sync_err: None,
            bridge_there: None,
            skill_there: None,
            skill_current: None,
            skill_sync_err: None,
            commands_there: None,
            commands_current: None,
            commands_sync_err: None,
            screens: None,
            screens_doing: None,
            agent_keys: None,
            account: None,
            carried: None,
            agent_sessions: None,
            repos_there: None,
            tailscale: None,
            tailscale_seq: 0,
            synced_hosts: Vec::new(),
            session_summaries: Vec::new(),
            sessions,
            pending_focus: HashMap::new(),
            pending_placements: HashMap::new(),
            pending_creates: HashMap::new(),
            creating_defer: HashMap::new(),
            keymap_bindings: Vec::new(),
            keymap_modes: Vec::new(),
            keymap_diagnostics: Vec::new(),
            control: None,
            input_tx: None,
            window_tx: None,
        }
    }
}

impl HubState {
    fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(t) = self.test_now {
            return t;
        }
        Instant::now()
    }

    fn is_pane_visible(&self, bk: &BlockKey) -> bool {
        self.visible_panes.values().any(|front| front == bk)
    }

    /// Strict same-host, same-session block lookup (preferring a live entry and
    /// falling back only to an exited entry in the SAME session — never across
    /// sessions).
    fn subagent_for_block(&self, bk: &BlockKey) -> Option<&crate::subagents::NativeSubagent> {
        let same = |e: &&crate::subagents::NativeSubagent| e.host_key == bk.host_key && e.session_id == bk.session_id && e.block_id == bk.block_id;
        self.subagents.entries.iter().rev().find(|e| same(e) && e.exited_at.is_none()).or_else(|| self.subagents.entries.iter().rev().find(same))
    }

    /// Whether `bk` belongs to a subagent currently watched by a positively
    /// identified, live minder on a host with a confirmed agents follower
    /// (`agents_told`):
    /// - explicitly assigned via `minded_by` to a present, live minder in the
    ///   same host and session, or
    /// - watched by a live legacy 1:1 minder (`minds == Some(sub.id)`) in the
    ///   same host and session, or
    /// - nested under a live, non-errored launcher-managed tab minder
    ///   (`role == "tab-minder"`, or unroled strict `minder[[-_ ]DIGITS]`
    ///   label) in the same host and session.
    ///
    /// Service-managed shared minders only watch their explicitly assigned
    /// agents, not unassigned grandchildren. Independent sibling split panes in
    /// a minded worker's tab are not matched (`subagent_for_block` only).
    /// Top-level lead tabs, unminded subagents, minder tabs themselves, and
    /// blocks on hosts without a confirmed agents follower return `false`.
    pub(crate) fn is_minded_subagent(&self, bk: &BlockKey) -> bool {
        if !self.agents_told.contains(&bk.host_key) {
            return false;
        }
        let Some(sub) = self.subagent_for_block(bk) else {
            return false;
        };
        if sub.is_minder() || entry_is_tab_minder(sub) {
            return false;
        }
        if let Some(mid) = sub.minded_by.as_deref().filter(|s| !s.is_empty())
            && self.subagents.entries.iter().rev().find(|e| e.host_key == bk.host_key && e.session_id == sub.session_id && e.id == mid).is_some_and(|m| m.exited_at.is_none())
        {
            return true;
        }
        if self.subagents.entries.iter().any(|e| e.host_key == bk.host_key && e.session_id == sub.session_id && e.exited_at.is_none() && e.minds.as_deref() == Some(sub.id.as_str())) {
            return true;
        }
        self.subagents.ancestor_windows(&bk.host_key, &sub.session_id, &sub.window_id).iter().filter_map(|wid| self.subagents.by_window(&bk.host_key, &sub.session_id, wid)).any(|anc| {
            entry_is_tab_minder(anc)
                && !matches!(
                    self.agent_states.get(&(bk.host_key.clone(), anc.id.clone())).copied(),
                    Some(olo_rex_link::agents::SubagentLiveState::Error | olo_rex_link::agents::SubagentLiveState::Exited)
                )
        })
    }

    fn has_subagent_record(&self, bk: &BlockKey) -> bool {
        self.subagent_for_block(bk).is_some()
    }

    /// Whether desktop notifications for `bk` are allowed by the minded-subagent rule.
    /// Matches Preferences ▸ Rex ▸ Notifications where the minded-subagent option is
    /// disabled whenever all three agent notification toggles are off.
    pub(crate) fn allows_pane_notice(&self, bk: &BlockKey) -> bool {
        let n = self.settings.notices;
        (n.minded_subagents && (n.blocked || n.done || n.failed)) || !self.is_minded_subagent(bk)
    }

    pub fn bump_topology(&mut self) {
        self.topology_rev = self.topology_rev.wrapping_add(1);
    }

    #[cfg(test)]
    pub(crate) fn test_empty() -> Self {
        Self {
            hosts: HashMap::new(),
            host_order: Vec::new(),
            blocks: HashMap::new(),
            forwards: Vec::new(),
            kept_forwards: Vec::new(),
            kept_forward_errors: HashMap::new(),
            kept_forwards_starting: HashSet::new(),
            settings: Settings::default(),
            active_theme: TerminalTheme::default(),
            pools: None,
            repos: Vec::new(),
            carry_choices: Vec::new(),
            kept: Vec::new(),
            sessions_window: olo_rex_client::sessions::Window::default(),
            git_cache: GitCache::new(),
            local_bin_dir: None,
            subagents: crate::subagents::SubagentRegistry::default(),
            agents_told: HashSet::new(),
            agents_follow_gen: HashMap::new(),
            agents_snapshot_pending: HashMap::new(),
            agent_states: HashMap::new(),
            desktop_screen_owners: HashMap::new(),
            desktop_screens: Vec::new(),
            desktop_screens_loaded: false,
            screens_rev: 0,
            topology_rev: 0,
            host_screens: HashMap::new(),
            quick_look_cache: HashMap::new(),
            visible_panes: HashMap::new(),
            held_pane_notices: HashMap::new(),
            held_notice_seq: 0,
            test_now: None,
        }
    }

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
                    // (`OLO_REX_DEMO_SCREENS=notallowed`: a host not let have screens here.)
                    (true, false) if std::env::var("OLO_REX_DEMO_SCREENS").as_deref() == Ok("notallowed") => vec!["Links", "Passwords"],
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
                screens: demo_screens().filter(|_| there),
                screens_doing: (there && std::env::var("OLO_REX_DEMO_SCREENS").as_deref() == Ok("installing")).then(|| ("Downloading Olo".to_owned(), 0.21)),
                this_olo: Some((1_795_715_000, "2026.11.26-c2a7db3c".into())),
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
            screens: host.screens.clone(),
            screens_doing: host.screens_doing.clone(),
            this_olo: crate::host_screens::this_olo(),
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
        self.held_pane_notices.retain(|k, list| {
            if keep(k) {
                true
            } else {
                for h in list {
                    if let Some(t) = h.timer.take() {
                        t.abort();
                    }
                }
                false
            }
        });
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
    /// connected to, by the host's key, with the connection they are
    /// for ([`SparesOf`]).
    spares: Mutex<HashMap<String, SparesOf>>,
    /// What each terminal's last stream left to be taken over with, by
    /// the pane's key ([`StreamLeft`]): for a stream the server may
    /// still hold after its way here went with a network.
    streams_left: Mutex<HashMap<BlockKey, StreamLeft>>,
    /// The number of the last task begun to have a pane's streams
    /// ([`RexHubInner::spawn_block_data_stream`]): each one's is higher
    /// than any before it ([`StreamLeft::task`]).
    stream_gen: std::sync::atomic::AtomicU64,
    /// The host and session each workspace window shows, as chosen
    /// there: kept while its host is disconnected (the window then shows
    /// none), for the order a connection made again attaches in.
    shown_sessions: Mutex<HashMap<crate::ui::WinId, (String, String)>>,
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
    /// Each connected host's agents service being followed (`follow_agents`),
    /// tagged with its monotonic follower generation.
    agents_following: Mutex<HashMap<String, (u64, tokio::task::AbortHandle)>>,
    /// Olo being put on a host for its screens (`change_host`, of
    /// `HostThing::Screens`), by the host: stopped by `cancel_screens`.
    screens_putting: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    /// Remote host Agent Screens follower, ownership resolver, state file, and
    /// local Unix socket server (`screens_sock`).
    screens_service: Arc<crate::screens_sock::ScreensService>,
    /// Tokio runtime handle for waking/spawning screen tasks from sync callbacks.
    rt_handle: tokio::runtime::Handle,
    /// Set true once `ui::init_app` has installed `HUB` so `OpenHost` never drops notices.
    ui_ready: Arc<std::sync::atomic::AtomicBool>,
    /// Whether this hub is running in isolated demo mode (`OLO_REX_DEMO=1`).
    demo_mode: bool,
    /// Zero-polling inotify watch on desktop screens folders keeping
    /// `HubState::desktop_screen_owners` current.
    desktop_screens_watch: Mutex<Option<crate::screens_sock::DesktopOwnersWatch>>,
}

/// A terminal's stream being opened as far as its upgrade, before the
/// ticket that says whose it is has come (`TerminalUpgrade`): begun
/// while the control connection says its own hello and asks for the
/// sessions, so that a terminal shows a round trip of the link after
/// those are in, not three (over SSH: a channel, the upgrade, the
/// hello). An error: it couldn't be opened.
type Spare = tokio::task::JoinHandle<Result<olo_rex_client::TerminalUpgrade<olo_rex_client::RexStream>, olo_rex_client::RexError>>;

/// The streams opened beforehand for a host's connection, and the
/// connection: kept once the connection is the host's (it was kept
/// under the host's key before that was known, and a connection the
/// host had replaced left its streams there, to its endpoint, for the
/// next connection's panes to send their tickets down: Astra R2 #3),
/// taken by its own panes only, and let go of by itself only.
struct SparesOf {
    connection: ControlClient,
    streams: Vec<Spare>,
}

impl RexHubInner {
    /// `streams`, opened beforehand for `connection`, the host's now.
    fn keep_spares(&self, host_key: &str, connection: &ControlClient, streams: Vec<Spare>) {
        let old = self.spares.lock().unwrap_or_else(|e| e.into_inner()).insert(host_key.to_owned(), SparesOf { connection: connection.clone(), streams });
        for spare in old.into_iter().flat_map(|s| s.streams) {
            spare.abort();
        }
    }

    /// One of the streams opened beforehand for `connection`: none of
    /// another connection's.
    fn take_spare(&self, host_key: &str, connection: &ControlClient) -> Option<Spare> {
        self.spares.lock().unwrap_or_else(|e| e.into_inner()).get_mut(host_key).filter(|s| s.connection.same_connection(connection)).and_then(|s| s.streams.pop())
    }

    /// The streams opened beforehand for `connection` that no terminal
    /// took are let go (one kept would be closed by the host before a
    /// pane opened later could use it); another connection's stay.
    fn let_go_of_spares(&self, host_key: &str, connection: &ControlClient) {
        let mut all = self.spares.lock().unwrap_or_else(|e| e.into_inner());
        if all.get(host_key).is_some_and(|s| s.connection.same_connection(connection))
            && let Some(left) = all.remove(host_key)
        {
            for spare in left.streams {
                spare.abort();
            }
        }
    }
}

/// The most streams opened beforehand for a host: as many as it had
/// terminals when its connection was lost, one where none is known
/// (the pane that shows first), and no more than this.
const MOST_SPARES: usize = 8;

/// A terminal's stream opened as far as its upgrade, now.
async fn open_terminal_stream(ep: &RexEndpoint) -> Result<olo_rex_client::TerminalUpgrade<olo_rex_client::RexStream>, olo_rex_client::RexError> {
    let (raw, header) = ep.connect_stream().await?;
    TerminalDataStream::upgrade(raw, &header).await
}

/// A terminal's ticket for a stream of its own (`data.connect`).
async fn terminal_ticket(ctrl: &ControlClient, bk: &BlockKey) -> Result<String, olo_rex_client::RexError> {
    Ok(ctrl.data_connect(&bk.session_id, &bk.block_id, "terminal", DATA_TERMINAL_SUBPROTOCOL).await?.payload.ticket)
}

/// What a terminal's stream left when it ended here without the
/// server's word (cut, or ended as its connection was lost): its own
/// token and how far it had come, with which the server gives that
/// very stream a new way here (`TerminalDataStream::connect_resume`).
///
/// A server holds a client's stream until its end reaches it, and the
/// end of one whose way here went with a network never does (Ben,
/// 2026-11-28: "after i switch from wifi to ethernet and rex
/// reconnects, most of the subagents come back but the parent ones
/// don't seem to show anything"): the server's own connection to an
/// address that went stays open until its timeouts end it, a quarter
/// of an hour or hours over SSH, where its SSH server keeps no watch
/// of its own. Meanwhile every new stream of that terminal from the
/// same client, which a connection made again with its token is, is
/// answered "AlreadyConnected" (tried on a real server, a cut that
/// left its side open: refused for the 100 s tried, the bootstrap's
/// tickets and new ones alike; taken at once once its end came). The
/// stream is taken over and ended instead, and the terminal attached
/// anew straight after (the same server: a resume of the held stream
/// was taken, and a new stream after its end).
///
/// It is kept until that stream's end is certain: the server's own word
/// of its close, a refusal of the token itself, or a new stream had. A
/// takeover that failed on the way (its connection, a time-out, the
/// pane let go of meanwhile) leaves it for the next try, renewed with
/// the token of the stream resumed, if it was. And it is the client's
/// and the host's it was had of (`StreamOwner`): a host connected
/// anew from its settings, or replaced, is another server, which is
/// never handed it.
#[derive(Clone, Debug)]
struct StreamLeft {
    token: [u8; 16],
    bytes: u64,
    at: Instant,
    owner: StreamOwner,
    /// The pane's task that left it ([`RexHubInner::stream_gen`]): what
    /// a task that ends late leaves never takes the place of what a
    /// later one of the pane's left, nor does its new stream forget that
    /// (Astra R2 #4: a pane had anew after a reconnect lost its stream,
    /// and the old task, ending after, left its own token over the new
    /// one's, which the server then refused).
    task: u64,
}

/// Whose a terminal's stream is, as its task has it: its host's
/// instance (`HostRuntime::host_instance_id`) and the client its
/// control connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StreamOwner {
    host_instance: u64,
    client: Option<String>,
}

/// How taking over a stream the server still held came out
/// ([`RexHubInner::end_stream_left`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Taken {
    /// The server said it ended (its close, answering ours).
    Ended,
    /// The server refused the token itself: nothing to take over with.
    Refused,
    /// Not known (the way there failed, or ended without the server's
    /// word): what the stream left is kept for the next try.
    Unsure,
}

/// What a stream left is kept no longer than this: a connection is
/// made again in seconds, and a terminal not attached by then has its
/// stream ended or long given up.
const STREAM_LEFT_KEPT: Duration = Duration::from_secs(3600);

/// The longest a stream may bring nothing before its first screen is
/// whole: then it is ended and the terminal had again (a stream whose
/// way stopped just after its welcome waited for ever, the pane blank
/// and taking no keys). Not after: a terminal at rest brings nothing,
/// and costs nothing.
const FIRST_SCREEN_QUIET: Duration = Duration::from_secs(20);

/// A terminal whose stream couldn't be had, or ended while its host's
/// connection stands, is attached again (nothing else would: until
/// now, until its session next changed): this soon after a first
/// failure, twice as long after each next, never longer apart than
/// [`ATTACH_AGAIN_MOST`], and no more than [`ATTACH_TRIES`] times in a
/// row (about three minutes; then, as before, when its session next
/// changes). Nothing is tried after the connection ends, or the pane.
const ATTACH_AGAIN: Duration = Duration::from_millis(250);
const ATTACH_AGAIN_MOST: Duration = Duration::from_secs(30);
const ATTACH_TRIES: u32 = 12;

/// [`ATTACH_TRIES`], or in a test `OLO_REX_ATTACH_TRIES`.
fn attach_tries() -> u32 {
    std::env::var("OLO_REX_ATTACH_TRIES").ok().and_then(|v| v.parse().ok()).unwrap_or(ATTACH_TRIES)
}

/// [`FIRST_SCREEN_QUIET`], or in a test `OLO_REX_FIRST_SCREEN_QUIET_MS`.
fn first_screen_quiet() -> Duration {
    std::env::var("OLO_REX_FIRST_SCREEN_QUIET_MS").ok().and_then(|v| v.parse().ok()).map_or(FIRST_SCREEN_QUIET, Duration::from_millis)
}

/// How long a stream has to have shown its pane for its end to be a
/// new failure's first, not one more in a row: [`ATTACH_AGAIN_MOST`],
/// counted from its first screen (it was counted from its welcome, so a
/// stream that never brought a screen, each try lasting half a minute,
/// was tried for ever: Astra R2 #7); or in a test
/// `OLO_REX_STREAM_LASTED_MS`.
fn stream_lasted() -> Duration {
    std::env::var("OLO_REX_STREAM_LASTED_MS").ok().and_then(|v| v.parse().ok()).map_or(ATTACH_AGAIN_MOST, Duration::from_millis)
}

/// How long to wait before the `failures`-th attempt at a terminal's
/// stream: [`ATTACH_AGAIN`], doubling, at most [`ATTACH_AGAIN_MOST`],
/// and up to a quarter more by the pane (a host's thirty panes cut at
/// once don't all ask again in the same instant).
fn attach_again_after(failures: u32, key: &BlockKey) -> Duration {
    use std::hash::{Hash as _, Hasher as _};
    let base = ATTACH_AGAIN.saturating_mul(1u32 << failures.saturating_sub(1).min(16)).min(ATTACH_AGAIN_MOST);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    let quarter = u64::try_from(base.as_millis() / 4).unwrap_or(0);
    base + Duration::from_millis(h.finish() % (quarter + 1))
}

/// Whether trying again may get a terminal a stream this failure
/// didn't: anything of the way there (a connection, a channel the SSH
/// server had no room for, a time-out), a ticket gone stale, a stream
/// still held. Not a pane the server no longer has, nor what no
/// trying mends (a request or a handshake it won't take, a host whose
/// key or sign-in isn't).
fn attach_again(e: &olo_rex_client::RexError) -> bool {
    use olo_rex_client::{DataRejectReason as Reject, RexError as E};
    match e {
        E::Api(api) => !matches!(api.status, 400 | 403 | 404),
        E::DataRejected(Reject::MalformedHandshake | Reject::WrongSubprotocol) => false,
        E::InvalidEndpoint(_) | E::SshRefused(_) | E::SshHostUnknown(_) | E::Tls(_) => false,
        _ => true,
    }
}

/// How many terminals a host has here (its other panes have no stream:
/// none is opened beforehand for them), as each was had: by the same
/// word that has a stream had for it ([`attaches_a_stream`]; they were
/// counted by its creator's name, which a terminal's may not have, and
/// a pane of another creator whose name begins so did: Astra R2 #17).
fn terminals_of(blocks: &HashMap<BlockKey, Arc<Mutex<BlockRuntime>>>, host_key: &str) -> usize {
    blocks.iter().filter(|(key, block)| key.host_key == host_key && block.lock().unwrap_or_else(|e| e.into_inner()).is_terminal).count()
}

/// Whether a pane of a session's view is a terminal, whose stream is
/// had: its creator is the terminal, or its flavor one of a terminal's.
fn attaches_a_stream(b: &BlockInfo) -> bool {
    b.creator_name == "com.superlogical.terminal" || b.flavor.starts_with("com.superlogical.terminal")
}

/// Where a session's panes come in the order their streams are had
/// (`ingest_session`): the one typed into in the tab in front, then
/// that tab's others; the rest after, as the session lists them.
fn attach_ranks(view: &SessionView) -> HashMap<String, u8> {
    let Some(front) = view.active_window() else { return HashMap::new() };
    front.all_blocks().map(|b| (b.block_id.clone(), if front.focused_block_id.as_deref() == Some(b.block_id.as_str()) { 0 } else { 1 })).collect()
}

/// How a terminal's stream ended.
enum StreamEnd {
    /// The pane was let go of (gone, or its host's connection with it).
    LetGo,
    /// Otherwise: had again while the connection stands.
    Lost,
}

/// Why a terminal's stream couldn't be had: said in the log, and
/// whether trying again may help ([`attach_again`]).
#[derive(Debug)]
struct AttachFailed {
    why: String,
    again: bool,
}

impl AttachFailed {
    fn of(what: &str, e: olo_rex_client::RexError) -> Self {
        Self { again: attach_again(&e), why: format!("{what}: {e}") }
    }
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
/// The demo's far host's screens of its own (`OLO_REX_DEMO_SCREENS`:
/// `none`, nothing there; `behind`, an older Olo; `lacking`, a host
/// that couldn't draw one; `asking`, not answered; `nohelper`, a helper
/// from before it had them; `running`, a screen running there; `host`,
/// the host's own the ones an agent gets; else all of it, this Olo's).
fn demo_screens() -> Option<crate::host_screens::ScreensThere> {
    use crate::host_screens::ScreensThere;
    let all = ScreensThere {
        helper: true,
        installed: Some("2026.11.26-c2a7db3c".into()),
        committed: Some(1_795_715_000),
        chrome: Some("158.0.8127.88".into()),
        software: true,
        bytes: 2_394_000_000,
        ..Default::default()
    };
    // (The screens there, for the states that have some: two running,
    // one named for what it shows; one stopped; `many`, past the list's
    // room.)
    use crate::host_screens::ScreenRow;
    let on =
        |name: &str, title: Option<&str>, started: u64| ScreenRow { name: name.into(), title: title.map(str::to_owned), running: true, archived: false, size: (1440, 900), started: Some(started) };
    let off = |name: &str, archived: bool| ScreenRow { name: name.into(), title: None, running: false, archived, size: (1280, 800), started: None };
    let some = vec![on("review", Some("Checkout flow"), 1_795_701_720), on("agent-2", None, 1_795_686_420), off("notes", false)];
    let with = |mut rows: Vec<ScreenRow>, all: ScreensThere| {
        crate::host_screens::in_order(&mut rows);
        ScreensThere { running: rows.iter().filter(|r| r.running).map(|r| r.name.clone()).collect(), rows, ..all }
    };
    Some(match std::env::var("OLO_REX_DEMO_SCREENS").ok().as_deref() {
        Some("asking") => return None,
        Some("nohelper") => ScreensThere::default(),
        Some("none" | "installing" | "failed") => ScreensThere { helper: true, software: true, ..Default::default() },
        Some("refused") => with(some, all),
        Some("behind") => with(some, ScreensThere { installed: Some("2026.11.24-8f10fb11".into()), committed: Some(1_795_500_000), ..all }),
        Some("running" | "stopping" | "stopfailed" | "chosen") => with(some, all),
        Some("many") => with(
            vec![
                on("review", Some("Checkout flow"), 1_795_701_720),
                on("agent-2", None, 1_795_686_420),
                on("docs-site", Some("Preview of the docs"), 1_795_690_000),
                off("billing", false),
                off("notes", false),
                off("old-trial", true),
            ],
            all,
        ),
        Some("lacking") => ScreensThere { missing: vec!["libEGL_mesa.so.0".into(), "libgbm.so.1".into()], software: false, ..all },
        Some("host") => ScreensThere { on_host: true, ..all },
        _ => all,
    })
}

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

/// The control connections a change of the network may have taken the
/// way of: those of hosts reached over it (SSH, HTTP), not this
/// computer's own (a socket here says at once when its peer goes).
fn connections_to_ask(hosts: &HashMap<String, HostRuntime>) -> Vec<ControlClient> {
    hosts.values().filter(|h| !h.is_local && matches!(h.endpoint, Some(RexEndpoint::Ssh { .. } | RexEndpoint::Http { .. } | RexEndpoint::Https { .. }))).filter_map(|h| h.control.clone()).collect()
}

/// Matches strict launcher minder names (`minder`, `minder-2`, `Minder 2`, `minder2`)
/// without false-matching descriptive worker names like `minder-fix` or `Minder notifications`.
fn is_minder_label(s: &str) -> bool {
    let t = s.trim();
    let Some(rest) = t.get(..6).filter(|p| p.eq_ignore_ascii_case("minder")).map(|_| &t[6..]) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let digits = rest.strip_prefix(['-', '_', ' ']).unwrap_or(rest);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// True only for a launcher-managed tab minder (`tools/dev/agents minder start`)
/// that watches its entire descendant subtree. Service-managed minders
/// (`is_minder()`) return `false` because they only watch explicitly assigned
/// agents. Any explicit non-empty `role` is authoritative (only `"tab-minder"`
/// qualifies); name/title label matching applies only when `role` is absent.
fn entry_is_tab_minder(e: &crate::subagents::NativeSubagent) -> bool {
    if e.is_minder() {
        return false;
    }
    if let Some(role) = e.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        return role.eq_ignore_ascii_case("tab-minder");
    }
    e.name.as_deref().is_some_and(is_minder_label) || is_minder_label(&e.title)
}

fn apply_program_status(g: &mut BlockRuntime, seq: u64, change: StatusUpdate, rules: crate::settings::Notices) -> Option<(AgentActivity, u64, &'static str, String, olo_kit::notify::NoteKind)> {
    let previous = AgentActivity::from_records(&g.program_statuses).map(|(_, r)| r.clone());
    let prev_blocked = g.blocked_record().cloned();
    g.status_event(seq, change);
    let now = g.recompute_activity();
    if now == AgentActivity::Blocked && prev_blocked.as_ref() != g.blocked_record() {
        g.blocked_gen = g.blocked_gen.wrapping_add(1);
    }
    // Whether the person is told (the desktop's settings say),
    // and whether the pane is marked unread: one that waits or
    // failed is, one that finished isn't.
    let rule = match now {
        AgentActivity::Blocked => Some((rules.blocked, true, "Agent blocked", olo_kit::notify::NoteKind::Important)),
        AgentActivity::Error => Some((rules.failed, true, "Agent failed", olo_kit::notify::NoteKind::Error)),
        AgentActivity::Done => Some((rules.done, false, "Agent finished", olo_kit::notify::NoteKind::Information)),
        _ => None,
    };
    let mut activity_note = None;
    if let Some((tell, unread, title, kind)) = rule
        && let Some((_, record)) = AgentActivity::from_records(&g.program_statuses)
        && previous.as_ref() != Some(record)
    {
        if !g.muted {
            if tell {
                let body = record.msg.clone().or_else(|| record.title.clone()).unwrap_or_else(|| g.title.clone());
                activity_note = Some((now, g.blocked_gen, title, body, kind));
            } else if now == AgentActivity::Blocked {
                // Disabled when this approval arrived: mark handled so detaching
                // later never replays it retroactively.
                g.delivered_blocked_gen = Some(g.blocked_gen);
            }
            if unread {
                g.marked_unread(Some(now));
            }
        } else if now == AgentActivity::Blocked {
            g.delivered_blocked_gen = Some(g.blocked_gen);
        }
    }
    activity_note
}

/// (See [`RexHub::minder_and_its_agents`].)
pub(crate) fn minder_and_its_agents(entries: &[crate::subagents::NativeSubagent], host_key: &str, session_id: &str, minder_window_id: &str) -> Vec<String> {
    let here = |e: &&crate::subagents::NativeSubagent| e.host_key == host_key && e.session_id == session_id && e.exited_at.is_none();
    let Some(minder) = entries.iter().filter(here).find(|e| e.window_id == minder_window_id) else { return vec![minder_window_id.to_owned()] };
    let mut order: Vec<String> = entries.iter().filter(here).filter(|e| e.minded_by.as_deref() == Some(minder.id.as_str())).map(|e| e.window_id.clone()).collect();
    order.push(minder.window_id.clone());
    order
}

impl RexHub {
    #[cfg(test)]
    fn test_offline(notify: impl Fn(HubNotice) + Send + Sync + 'static) -> Arc<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("test runtime");
        let state = Arc::new(Mutex::new(HubState::test_empty()));
        let notify: Arc<dyn Fn(HubNotice) + Send + Sync> = Arc::new(notify);
        let ui_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let screen_opener = Arc::new(crate::screens_sock::HubScreenOpener::new(state.clone(), notify.clone(), ui_ready.clone()));
        let owner_resolver = Arc::new(|_: &str, _: &olo_rex_link::screen_wire::OwnerPane| None);
        let screens_service = crate::screens_sock::ScreensService::new(PathBuf::new(), PathBuf::new(), std::process::id(), screen_opener, owner_resolver, None, None);
        let inner = Arc::new(RexHubInner {
            state: state.clone(),
            ssh_pool: Arc::new(SshSessionPool::default()),
            askpass: None,
            notify: notify.clone(),
            spares: Mutex::new(HashMap::new()),
            stream_gen: std::sync::atomic::AtomicU64::new(0),
            streams_left: Mutex::new(HashMap::new()),
            shown_sessions: Mutex::new(HashMap::new()),
            bridge_calls: Default::default(),
            run_jobs: crate::bridge_run::RunJobs::new(),
            run_approvals: crate::bridge_run::RunApprovals::new(|| {}, || {}),
            agent_socks: Mutex::new(HashMap::new()),
            keeping: Mutex::new(HashMap::new()),
            agents_following: Mutex::new(HashMap::new()),
            screens_putting: Mutex::new(HashMap::new()),
            screens_service,
            rt_handle: rt.handle().clone(),
            ui_ready,
            demo_mode: true,
            desktop_screens_watch: Mutex::new(None),
        });
        Arc::new(Self { state, rt, notify, inner })
    }

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
                host_instance_id: crate::screens_sock::next_host_instance_id(),
                screens_conn_id: 0,
                screens_state_seq: 0,
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
                screens: None,
                screens_doing: None,
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
                pending_placements: HashMap::new(),
                pending_creates: HashMap::new(),
                creating_defer: HashMap::new(),
                keymap_bindings: Vec::new(),
                keymap_modes: Vec::new(),
                keymap_diagnostics: Vec::new(),
                control: None,
                input_tx: None,
                window_tx: None,
            },
        );
        host_order.push("local".to_owned());

        for sh in &settings.saved_hosts {
            if !hosts.contains_key(&sh.id) {
                hosts.insert(
                    sh.id.clone(),
                    HostRuntime {
                        key: sh.id.clone(),
                        host_instance_id: crate::screens_sock::next_host_instance_id(),
                        screens_conn_id: 0,
                        screens_state_seq: 0,
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
                        screens: None,
                        screens_doing: None,
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
                        pending_placements: HashMap::new(),
                        pending_creates: HashMap::new(),
                        creating_defer: HashMap::new(),
                        keymap_bindings: Vec::new(),
                        keymap_modes: Vec::new(),
                        keymap_diagnostics: Vec::new(),
                        control: None,
                        input_tx: None,
                        window_tx: None,
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
            subagents: crate::subagents::SubagentRegistry::default(),
            agents_told: HashSet::new(),
            agents_follow_gen: HashMap::new(),
            agents_snapshot_pending: HashMap::new(),
            agent_states: HashMap::new(),
            desktop_screen_owners: HashMap::new(),
            desktop_screens: Vec::new(),
            desktop_screens_loaded: false,
            screens_rev: 0,
            topology_rev: 0,
            host_screens: HashMap::new(),
            quick_look_cache: HashMap::new(),
            visible_panes: HashMap::new(),
            held_pane_notices: HashMap::new(),
            held_notice_seq: 0,
            #[cfg(test)]
            test_now: None,
        }));

        let demo_mode = std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1");
        let (sock_path, state_path) = if demo_mode { crate::screens_sock::demo_screens_paths().unwrap_or_else(|| (PathBuf::new(), PathBuf::new())) } else { crate::screens_sock::default_paths() };
        let ui_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let screen_opener = Arc::new(crate::screens_sock::HubScreenOpener::new(state.clone(), notify.clone(), ui_ready.clone()));
        let owner_resolver = {
            let state = state.clone();
            Arc::new(move |host_key: &str, pane: &olo_rex_link::screen_wire::OwnerPane| crate::screens_sock::resolve_controlled_by_in_hub(&state, host_key, pane))
        };
        let on_host_screens = {
            let state = state.clone();
            let notify = notify.clone();
            Arc::new(move |upd: crate::screens_sock::HostScreensUpdate| {
                let changed = {
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    // Verify live host membership and instance ID BEFORE mutating
                    // `host_screens` so a queued update can never reinsert a
                    // removed or replaced host (Finding 14).
                    let live_inst_matches = st.hosts.get(&upd.host_key).is_some_and(|h| !h.is_local && h.host_instance_id == upd.host_instance_id);
                    if !live_inst_matches {
                        return;
                    }
                    let mut changed = false;
                    let accept = st.host_screens.get(&upd.host_key).is_none_or(|prev| prev.host_instance_id != upd.host_instance_id || upd.rev >= prev.rev);
                    if !accept {
                        return;
                    }
                    let next_live = crate::screens_sock::LiveHostScreens {
                        host_instance_id: upd.host_instance_id,
                        conn_id: upd.conn_id,
                        rev: upd.rev,
                        conn_state: upd.conn_state,
                        has_follow_snapshot: upd.has_follow_snapshot,
                        status: upd.status.clone(),
                        can_control: upd.can_control,
                        control_disabled_reason: upd.control_disabled_reason.clone(),
                        proto: upd.proto,
                        commands: upd.commands.clone(),
                        capabilities: upd.capabilities.clone(),
                        disconnected_at: upd.disconnected_at,
                        screens: upd.screens.clone(),
                    };
                    if st.host_screens.get(&upd.host_key) != Some(&next_live) {
                        st.host_screens.insert(upd.host_key.clone(), next_live);
                        st.screens_rev = st.screens_rev.wrapping_add(1);
                        changed = true;
                    }
                    // Only merge follower rows/status into `host.screens` once a
                    // real follower snapshot arrived for the current connection
                    // AND `ask_what_the_host_has` has populated machine fields
                    // (never clobbering older helpers' `status/list` rows or
                    // guessing `software: true` before the machine answers).
                    let live_ref = st.host_screens.get(&upd.host_key).cloned();
                    if let (Some(live), Some(host)) = (live_ref.as_ref(), st.hosts.get_mut(&upd.host_key))
                        && host.host_instance_id == upd.host_instance_id
                        && let Some(there) = host.screens.as_mut()
                        && merge_live_screens_into_there(there, live, host.screens_conn_id)
                    {
                        changed = true;
                    }
                    changed
                };
                if changed {
                    notify(HubNotice::ScreensChanged);
                }
            })
        };
        let on_host_removed = {
            let state = state.clone();
            let notify = notify.clone();
            Arc::new(move |host_key: String, host_instance_id: u64| {
                let removed = {
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    if st.host_screens.get(&host_key).is_some_and(|l| l.host_instance_id == host_instance_id || !st.hosts.contains_key(&host_key)) {
                        let did = st.host_screens.remove(&host_key).is_some();
                        if did {
                            st.screens_rev = st.screens_rev.wrapping_add(1);
                        }
                        did
                    } else {
                        false
                    }
                };
                if removed {
                    notify(HubNotice::ScreensChanged);
                    notify(HubNotice::StateChanged);
                }
            })
        };
        let on_last_used = {
            let state = state.clone();
            let notify = notify.clone();
            Arc::new(move |upd: crate::screens_sock::LastUsedUpdate| {
                let changed = {
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    // Match live host instance + connection ID so a delayed activity
                    // tick for a replaced host never touches the replacement's row (Finding 15).
                    let matches_live = st.hosts.get(&upd.host_key).is_some_and(|h| !h.is_local && h.host_instance_id == upd.host_instance_id && h.screens_conn_id == upd.conn_id);
                    if !matches_live {
                        return;
                    }
                    let mut diff = false;
                    if let Some(live) = st.host_screens.get_mut(&upd.host_key)
                        && live.host_instance_id == upd.host_instance_id
                        && live.conn_id == upd.conn_id
                        && let Some(sc) = live.screens.iter_mut().find(|s| s.name == upd.name)
                    {
                        if sc.last_used.is_none_or(|prev| upd.last_used > prev) {
                            sc.last_used = Some(upd.last_used);
                            diff = true;
                        }
                        if let Some(a) = upd.actions
                            && sc.actions != Some(a)
                        {
                            sc.actions = Some(a);
                            diff = true;
                        }
                    }
                    if diff {
                        st.screens_rev = st.screens_rev.wrapping_add(1);
                    }
                    diff
                };
                if changed {
                    notify(HubNotice::ScreensChanged);
                }
            })
        };
        let screens_service = crate::screens_sock::ScreensService::new(sock_path, state_path, std::process::id(), screen_opener, owner_resolver, Some(on_host_screens), Some(on_last_used));
        screens_service.set_on_host_removed(on_host_removed);
        // Start the bounded ordered publisher independently of socket startup so
        // the Host window still receives updates even if socket bind fails (Findings 9, 24, 35).
        screens_service.start_publisher(rt.handle());
        {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            let let_go_list = let_go();
            let auto_set: HashSet<&str> = st.settings.saved_hosts.iter().filter(|h| h.auto_connect && !let_go_list.contains(&h.id)).map(|h| h.id.as_str()).collect();
            let initial_hosts: Vec<(String, u64, String, olo_screen_pane::rex_sock::RexHostConn)> = st
                .host_order
                .iter()
                .filter_map(|k| st.hosts.get(k))
                .filter(|h| !h.is_local && crate::screens_sock::is_ssh_endpoint(&h.endpoint_raw))
                .map(|h| {
                    let init_conn = if auto_set.contains(h.key.as_str()) { olo_screen_pane::rex_sock::RexHostConn::Connecting } else { olo_screen_pane::rex_sock::RexHostConn::Disconnected };
                    (h.key.clone(), h.host_instance_id, h.label.clone(), init_conn)
                })
                .collect();
            let endpoints: HashMap<String, String> = st.hosts.values().filter(|h| !h.is_local).map(|h| (h.key.clone(), h.endpoint_raw.clone())).collect();
            screens_service.set_initial_hosts(initial_hosts);
            let restored = std::mem::take(&mut *SAVED_HOST_SCREENS.lock().unwrap_or_else(|e| e.into_inner()));
            if !restored.is_empty() {
                drop(st);
                let seeded = screens_service.restore_cached_hosts(restored, &endpoints);
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                for (host_key, live) in seeded {
                    if st.hosts.get(&host_key).is_some_and(|h| h.host_instance_id == live.host_instance_id) {
                        st.host_screens.insert(host_key, live);
                    }
                }
            }
        }

        let inner = Arc::new(RexHubInner {
            state: state.clone(),
            ssh_pool,
            askpass,
            notify: notify.clone(),
            spares: Mutex::new(HashMap::new()),
            stream_gen: std::sync::atomic::AtomicU64::new(0),
            streams_left: Mutex::new(HashMap::new()),
            shown_sessions: Mutex::new(HashMap::new()),
            bridge_calls: Default::default(),
            run_jobs: crate::bridge_run::RunJobs::new(),
            run_approvals: run_approvals.unwrap_or_else(|| crate::bridge_run::RunApprovals::new(|| {}, || {})),
            agent_socks: Mutex::new(HashMap::new()),
            keeping: Mutex::new(HashMap::new()),
            agents_following: Mutex::new(HashMap::new()),
            screens_putting: Mutex::new(HashMap::new()),
            screens_service,
            rt_handle: rt.handle().clone(),
            ui_ready,
            demo_mode,
            desktop_screens_watch: Mutex::new(None),
        });

        let hub = Arc::new(Self { state, rt, notify, inner });

        // Isolated demo workspace (`OLO_REX_DEMO=1`) for `tools/dev/ui-lint` and offline visual QA:
        // populates realistic multi-pane splits, agent activity, git status, and a remote host
        // without touching any real Rex server socket or user files.
        if demo_mode {
            hub.populate_demo_workspace();
            // (`OLO_REX_DEMO_CHILD_ENDS=SECONDS`: the child "Unit Tests"
            // ends that long after the start, as a child stopped on its
            // host does: its row goes from every list while whatever the
            // pointer was doing goes on. For the pictures of what a row's
            // going leaves behind: a tip stayed up for a child nowhere in
            // the list, 2026-11-26.)
            if let Some(after) = std::env::var("OLO_REX_DEMO_CHILD_ENDS").ok().and_then(|s| s.parse::<u64>().ok()) {
                let (state, notify) = (hub.state.clone(), hub.notify.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(after));
                    state.lock().unwrap_or_else(|e| e.into_inner()).subagents.entries.retain(|e| e.title != "Unit Tests");
                    notify(HubNotice::StateChanged);
                });
            }
            if std::env::var_os("OLO_REX_FILES_SOCKET").is_some() {
                crate::files_sock::spawn(hub.rt.handle(), hub.state.clone());
            }
            if crate::screens_sock::demo_screens_paths().is_some() {
                let initial_demo: Vec<(String, u64, String, olo_screen_pane::rex_sock::RexHostConn)> = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.host_order
                        .iter()
                        .filter_map(|k| st.hosts.get(k))
                        .filter(|h| !h.is_local && crate::screens_sock::is_ssh_endpoint(&h.endpoint_raw))
                        .map(|h| (h.key.clone(), h.host_instance_id, h.label.clone(), crate::screens_sock::conn_state_to_rex(&h.conn_state).0))
                        .collect()
                };
                hub.inner.screens_service.set_initial_hosts(initial_demo);
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
            if let Some(h) = hub_net.upgrade() {
                let (to_reconnect, to_ask) = {
                    let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
                    // (Those that were being connected to, once there is a
                    // way out: one the person let go of, or never
                    // connected, isn't connected because the network came
                    // back.)
                    let back: Vec<String> = if olo_net::online() { st.hosts.values().filter(|hr| !hr.is_local && wanted_back(&hr.conn_state)).map(|hr| hr.key.clone()).collect() } else { Vec::new() };
                    (back, connections_to_ask(&st.hosts))
                };
                for hk in to_reconnect {
                    h.connect_host(&hk);
                }
                // Those connected over the network are asked at once
                // whether they still are (a ping answered within five
                // seconds, `ControlClient::probe`): a connection whose way
                // went with the network is found now, not by its next
                // keepalive, minutes on (over the helper's own link, which
                // the network's news doesn't end); one that stands costs
                // a ping.
                for ctrl in to_ask {
                    ctrl.probe();
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
        // (`OLO_REX_DEMO_COMMAND_CHILD=1`: and a child that is a command of the
        // asker's own, `olo agent start --command`, for its pictures.)
        let command_child = std::env::var("OLO_REX_DEMO_COMMAND_CHILD").ok().as_deref() == Some("1");
        let win_sub3 = mk_single_win("win-sub-3", "Build Log", "demo-sub-3", "tail — Build Log");

        let active_wid = if std::env::var("OLO_REX_DEMO_ACTIVE_CHILD").ok().as_deref() == Some("1") { "win-sub-1a" } else { "win-1" };
        let main_view = SessionView {
            revision: 1,
            session_id: "demo-main".to_owned(),
            label: "main".to_owned(),
            active_window_id: Some(active_wid.to_owned()),
            windows: if command_child { vec![win1, win2, win_sub1, win_sub1a, win_sub2, win_sub3] } else { vec![win1, win2, win_sub1, win_sub1a, win_sub2] },
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

        let now_ms = crate::subagents::now_unix_ms() as i64;
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
            started_at: now_ms - 95_000,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 0,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: None,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
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
            started_at: now_ms - 42_000,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 0,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: None,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
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
            started_at: now_ms - 180_000,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 0,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: None,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
        });

        if command_child {
            let mut br_sub3 = BlockRuntime::new(110, 32, &theme);
            br_sub3.title = "tail — Build Log".to_owned();
            br_sub3.pwd = "/home/you/olo/rex".to_owned();
            br_sub3.data_connected = true;
            br_sub3
                .terminal
                .vt_write(concat!("   Compiling olo-rex-link v0.1.0\r\n", "   Compiling olo-rex v0.1.0\r\n", "    Finished `dev` profile [optimized + debuginfo] target(s) in 41.07s\r\n").as_bytes());
            st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-sub-3".into() }, Arc::new(Mutex::new(br_sub3)));
            st.subagents.register(crate::subagents::NativeSubagent {
                id: "sa-log003".into(),
                name: Some("build-log".into()),
                host_key: "local".into(),
                session_id: "demo-main".into(),
                window_id: "win-sub-3".into(),
                block_id: "demo-sub-3".into(),
                parent_window_id: "win-1".into(),
                parent_block_id: Some("demo-agent".into()),
                cli: crate::subagents::AgentCli::Command,
                model: String::new(),
                effort: String::new(),
                title: "Build Log".into(),
                role: None,
                cwd: Some("/home/you/olo/rex".into()),
                resume_id: None,
                cli_session_id: None,
                transcript_path: None,
                desktop_policy: crate::subagents::DesktopPolicy::None,
                stream_mode: false,
                started_at: now_ms - 60_000,
                exited_at: None,
                exit_code: None,
                last_pid: None,
                created_rev: 0,
                seen_live: true,
                exited_by_sync: false,
                session_confidence: None,
                add_dirs: Vec::new(),
                command: vec!["tail".into(), "-f".into(), "/var/tmp/olo/build.log".into()],
                minds: None,
                minded_by: None,
                shared_minder: false,
            });
        }

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
            host_instance_id: crate::screens_sock::next_host_instance_id(),
            screens_conn_id: 0,
            screens_state_seq: 0,
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
            screens: None,
            screens_doing: None,
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
            pending_placements: HashMap::new(),
            pending_creates: HashMap::new(),
            creating_defer: HashMap::new(),
            keymap_bindings: Vec::new(),
            keymap_modes: Vec::new(),
            keymap_diagnostics: Vec::new(),
            control: None,
            input_tx: None,
            window_tx: None,
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
        if let Some(mode) = crate::activity_screens::demo_screens_mode() {
            crate::activity_screens::seed_demo_screens_state(&mut st, &theme, &mode);
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
        let (changes, agents, renamed_info) = {
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
            let renamed_info: Vec<(String, u64, String)> = changes
                .renamed
                .iter()
                .filter_map(|host| {
                    let h = st.hosts.get(&host.id)?;
                    (!h.is_local && crate::screens_sock::is_ssh_endpoint(&h.endpoint_raw)).then(|| (host.id.clone(), h.host_instance_id, host.label.clone()))
                })
                .collect();
            if !changes.renamed.is_empty() {
                st.bump_topology();
            }
            (changes, agents, renamed_info)
        };
        for (id, inst_id, label) in &renamed_info {
            self.inner.screens_service.host_renamed(id, *inst_id, label);
        }
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
        let inst_id = crate::screens_sock::next_host_instance_id();
        let is_ssh = crate::screens_sock::is_ssh_endpoint(&endpoint);
        let init_conn_state = if connect { HostConnState::Connecting } else { HostConnState::Disconnected };
        let init_rex_conn = if connect { olo_screen_pane::rex_sock::RexHostConn::Connecting } else { olo_screen_pane::rex_sock::RexHostConn::Disconnected };
        let replaced_inst = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.hosts.contains_key(&id) {
                st.host_order.push(id.clone());
            }
            let replaced = st.hosts.insert(
                id.clone(),
                HostRuntime {
                    key: id.clone(),
                    host_instance_id: inst_id,
                    screens_conn_id: 0,
                    screens_state_seq: 0,
                    label: label.clone(),
                    endpoint_raw: endpoint.clone(),
                    endpoint: None,
                    is_local: false,
                    conn_state: init_conn_state,
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
                    screens: None,
                    screens_doing: None,
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
                    pending_placements: HashMap::new(),
                    pending_creates: HashMap::new(),
                    creating_defer: HashMap::new(),
                    keymap_bindings: Vec::new(),
                    keymap_modes: Vec::new(),
                    keymap_diagnostics: Vec::new(),
                    control: None,
                    input_tx: None,
                    window_tx: None,
                },
            );
            let old_inst = if let Some(mut old) = replaced {
                if let Some(retry) = old.retry.take() {
                    retry.notify_one();
                }
                if let Some(ctrl) = old.control.take() {
                    ctrl.close();
                }
                st.host_screens.remove(&id);
                Some(old.host_instance_id)
            } else {
                None
            };
            if !st.settings.saved_hosts.iter().any(|h| h.id == id) {
                st.settings.saved_hosts.push(SavedHost { id: id.clone(), label: label.clone(), endpoint: endpoint.clone(), icon: None, auto_connect: true, ssh_agent_socket: None });
                st.settings.save();
            }
            st.bump_topology();
            old_inst
        };
        if let Some(old_inst) = replaced_inst {
            self.inner.screens_service.host_removed(&id, old_inst);
            // (Another server under the same name: nothing of the old
            // one's streams goes to it.)
            self.inner.forget_streams_of(&id);
        }
        if is_ssh {
            self.inner.screens_service.host_added(&id, inst_id, &label, init_rex_conn);
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
    /// Start Rex's local Agent Screens Unix socket server and desktop screen
    /// owner watch on the background runtime once `ui::init_app` has installed
    /// `HUB` (Findings 24 & 35: never blocks GUI startup, and `OpenHost` can
    /// only be accepted once `HUB` is ready).
    pub fn start_screens_service(self: &Arc<Self>) {
        self.inner.ui_ready.store(true, std::sync::atomic::Ordering::Release);
        if self.inner.demo_mode && crate::screens_sock::demo_screens_paths().is_none() {
            return;
        }
        let svc = self.inner.screens_service.clone();
        let rt_handle = self.rt.handle().clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = svc.start_server(&rt_handle) {
                tracing::warn!("Rex screens socket start failed: {e}");
            }
        });
        if !self.inner.demo_mode {
            *self.inner.desktop_screens_watch.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(crate::screens_sock::watch_desktop_screen_owners(self.rt.handle().clone(), self.state.clone(), self.notify.clone()));
        }
    }

    /// Snapshot last-known remote host screens for in-place upgrade survival
    /// (`SavedAppState::cached_host_screens`, Finding 26).
    pub fn saved_host_screens(&self) -> Vec<crate::screens_sock::SavedHostScreens> {
        let endpoints: HashMap<String, String> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.values().filter(|h| !h.is_local).map(|h| (h.key.clone(), h.endpoint_raw.clone())).collect()
        };
        self.inner.screens_service.saved_hosts_for_upgrade(&endpoints)
    }

    /// Stop Rex's local screens Unix socket server, host screen followers,
    /// desktop screen owner watch, and unlink the socket and state file.
    pub fn shutdown_screens(&self) {
        *self.inner.desktop_screens_watch.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.inner.screens_service.shutdown();
    }

    /// Direct handle to [`ScreensService`] for Rex's embedded [`olo_screen_pane::ScreenPane`].
    pub fn screens_service(&self) -> Arc<crate::screens_sock::ScreensService> {
        self.inner.screens_service.clone()
    }

    /// Runtime handle used by [`crate::activity_screens::HubStreamOpener`].
    pub fn rt_handle(&self) -> tokio::runtime::Handle {
        self.rt.handle().clone()
    }

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
        let (endpoint, inst_id, prev_conn_id, state_seq, is_ssh) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let host = st.hosts.get_mut(host_key)?;
            let inst_id = host.host_instance_id;
            let prev_conn_id = host.screens_conn_id;
            let is_ssh = matches!(host.endpoint, Some(RexEndpoint::Ssh { .. })) || crate::screens_sock::is_ssh_endpoint(&host.endpoint_raw);
            host.screens_conn_id = 0;
            host.screens_state_seq += 1;
            let state_seq = host.screens_state_seq;
            // (The loop that keeps it connected ends at its next look:
            // it is no longer the one the host names.)
            if let Some(retry) = host.retry.take() {
                retry.notify_one();
            }
            if let Some(ctrl) = host.control.take() {
                ctrl.close();
            }
            host.input_tx = None;
            host.window_tx = None;
            host.tailscale = None;
            host.server_status = None;
            host.sessions.clear();
            host.pending_focus.clear();
            host.pending_placements.clear();
            host.pending_creates.clear();
            for (_, (_, _, notify)) in host.creating_defer.drain() {
                notify.notify_waiters();
            }
            host.session_summaries.clear();
            host.connection_lost = None;
            host.conn_state = HostConnState::Disconnected;
            let endpoint = host.endpoint.clone();
            for f in st.forwards.iter_mut().filter(|f| f.host_key == host_key) {
                f.state = ForwardState::Waiting;
            }
            st.retain_blocks(|k| k.host_key != host_key);
            st.bump_topology();
            (endpoint, inst_id, prev_conn_id, state_seq, is_ssh)
        };
        self.inner.invalidate_agents_follower(host_key, false);
        {
            let mut let_go = LET_GO.lock().unwrap_or_else(|e| e.into_inner());
            if !let_go.iter().any(|k| k == host_key) {
                let_go.push(host_key.to_owned());
            }
        }
        if is_ssh {
            self.inner.screens_service.host_state_changed(host_key, inst_id, (prev_conn_id != 0).then_some(prev_conn_id), state_seq, olo_screen_pane::rex_sock::RexHostConn::Disconnected, None, None);
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
        let removed_inst_id = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let inst_id = if let Some(host) = st.hosts.remove(&key) {
                let id = host.host_instance_id;
                if let Some(ctrl) = host.control {
                    ctrl.close();
                }
                if let Some(retry) = host.retry {
                    retry.notify_one();
                }
                Some(id)
            } else {
                None
            };
            st.host_order.retain(|k| k != &key);
            st.host_screens.remove(&key);
            st.screens_rev = st.screens_rev.wrapping_add(1);
            st.bump_topology();
            st.forwards.retain(|f| f.host_key != key);
            st.retain_blocks(|bk| bk.host_key != key);
            st.settings.saved_hosts.retain(|h| h.id != key);
            st.settings.save();
            inst_id
        };
        self.inner.invalidate_agents_follower(&key, false);
        self.inner.forget_streams_of(&key);
        if let Some(inst_id) = removed_inst_id {
            self.inner.screens_service.host_removed(&key, inst_id);
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
        (self.notify)(HubNotice::ScreensChanged);
        (self.notify)(HubNotice::StateChanged);
    }

    /// Records which pane is currently shown in front in workspace `window`,
    /// discarding any held notices for that pane and marking its open `Blocked`
    /// approval as seen so a later minder detach does not alert on it.
    pub fn set_window_front_pane(&self, window: crate::ui::WinId, pane: Option<BlockKey>) {
        self.inner.set_window_front_pane(window, pane);
    }

    /// The host and session a workspace window shows, as chosen there
    /// (none: it shows none, or it closed): whatever its host's
    /// connection, so that a connection made again attaches what the
    /// person was looking at first.
    pub fn window_shows(&self, window: crate::ui::WinId, host_key: &str, session_id: Option<&str>) {
        let mut shown = self.inner.shown_sessions.lock().unwrap_or_else(|e| e.into_inner());
        match session_id {
            Some(sid) => {
                if shown.get(&window).is_none_or(|(h, s)| h != host_key || s != sid) {
                    shown.insert(window, (host_key.to_owned(), sid.to_owned()));
                }
            }
            None => {
                shown.remove(&window);
            }
        }
    }

    pub fn hide_window_front_pane(&self, window: crate::ui::WinId) {
        self.inner.set_window_front_pane(window, None);
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
                            inner.ingest_session(&hk, &ctrl, &ep, att.payload.view, att.seq, &[]).await;
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

    pub fn create_window(&self, host_key: &str, session_id: &str, cwd: Option<String>, feedback: Option<Feedback>, on_created: Option<Box<dyn FnOnce() + Send>>) {
        let explicit_cwd = cwd.filter(|d| !d.is_empty());
        let has_explicit_cwd = explicit_cwd.is_some();
        let base_opts = self.local_shell_opts(host_key, None);
        let (window_tx, source_root_id, active_block_id, known_cwds) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let st = &mut *st;
            let Some(host) = st.hosts.get_mut(host_key) else {
                if let Some(fb) = feedback {
                    fb.problem("Couldn’t create the tab.", "The host wasn’t found.");
                }
                return;
            };
            let (source_root_id, active_block_id) = match host.sessions.get(session_id) {
                Some(sess) => {
                    let root = active_top_level_window(&st.subagents, host_key, sess);
                    let active_bid = (!has_explicit_cwd).then(|| active_session_block(sess).map(str::to_owned)).flatten();
                    (root, active_bid)
                }
                None => (None, None),
            };
            let known_cwds = if !has_explicit_cwd && let Some(ref bid) = active_block_id {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: bid.clone() };
                st.blocks
                    .get(&bk)
                    .map(|br| {
                        let g = br.lock().unwrap_or_else(|e| e.into_inner());
                        block_cwd_candidates(&g.pwd, g.process.as_ref())
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let tx = host.window_tx.clone();
            if tx.is_some() {
                *host.pending_creates.entry(session_id.to_owned()).or_insert(0) += 1;
            }
            (tx, source_root_id, active_block_id, known_cwds)
        };
        let Some(tx) = window_tx else {
            if let Some(fb) = feedback {
                fb.problem("Couldn’t create the tab.", "The Rex server is disconnected.");
            }
            return;
        };
        if tx.send(HostWindowOp::CreateWindow { session_id: session_id.to_owned(), explicit_cwd, source_root_id, active_block_id, known_cwds, base_opts, feedback, on_created }).is_err() {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(host) = st.hosts.get_mut(host_key)
                && let Some(n) = host.pending_creates.get_mut(session_id)
            {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    host.pending_creates.remove(session_id);
                }
            }
            if let Some(fb) = feedback {
                fb.problem("Couldn’t create the tab.", "The Rex server is disconnected.");
            }
        }
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
                            inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
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
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
            }
        });
    }

    /// Verify that `host_key` currently matches a live pinned `(host_instance_id, conn_id)` (Astra #1).
    pub(crate) fn host_matches_pin(&self, host_key: &str, host_instance_id: u64, conn_id: u64) -> bool {
        self.inner.host_matches_pin(host_key, host_instance_id, conn_id)
    }

    /// Capture the live `(host_instance_id, screens_conn_id)` pin for `host_key`
    /// when rendering a row or building a context menu (Astra #1). Returns `None`
    /// for missing hosts or disconnected remote hosts (`screens_conn_id == 0`).
    pub(crate) fn host_live_pin(&self, host_key: &str) -> Option<(u64, u64)> {
        self.inner.host_live_pin(host_key)
    }

    /// Gracefully stops and closes a native subagent child tab (and optionally its
    /// entire descendant subtree bottom-up when `tree` is true).
    #[allow(dead_code)]
    pub fn stop_subagent(&self, host_key: &str, session_id: &str, window_id: &str, force: bool, tree: bool) {
        self.stop_subagent_pinned(host_key, None, session_id, window_id, force, tree);
    }

    /// Like [`Self::stop_subagent`], pinning `(host_instance_id, screens_conn_id)`
    /// captured when a confirmation dialog opened so a host replacement or
    /// reconnect cannot route the stop to a different server (Astra #5).
    pub fn stop_subagent_pinned(&self, host_key: &str, pin: Option<(u64, u64)>, session_id: &str, window_id: &str, force: bool, tree: bool) {
        // (Asked of the host's agents service, as `olo agent stop` asks it.)
        let mut args = vec![window_id.to_owned(), "--json".to_owned()];
        if force {
            args.push("--force".into());
        }
        if tree {
            args.push("--tree".into());
        }
        let (inner, hk, sid) = (self.inner.clone(), host_key.to_owned(), session_id.to_owned());
        self.rt.spawn(async move {
            if let Some((want_inst, want_conn)) = pin
                && !inner.host_matches_pin(&hk, want_inst, want_conn)
            {
                tracing::warn!("Refusing stale subagent stop on {hk}: host instance or connection changed");
                return;
            }
            let answer = inner.ask_agents_pinned(&hk, pin, "stop", Some(json!({ "args": args })), Some(sid), None, None).await;
            if !answer.ok {
                tracing::warn!("Stopping a child tab on {hk}: {}", answer.stderr.unwrap_or_default().trim());
            }
        });
    }

    /// Stops several child tabs one after the other, in the order given,
    /// each with what is under it: a stop is answered before the next is
    /// asked. Asked each by itself (as `stop_subagent` asks), they went
    /// on side by side: a minder's stop overtook its agents', and the
    /// service, which never stops a minder's agents with it, put each
    /// back under the tab and told the tab's own agent "its minder
    /// ended: what it asks comes to you again", of agents about to end
    /// (a code review, 2026-11-27).
    #[allow(dead_code)]
    pub fn stop_subagents_in_order(&self, host_key: &str, session_id: &str, window_ids: Vec<String>) {
        self.stop_subagents_in_order_pinned(host_key, None, session_id, window_ids);
    }

    /// Like [`Self::stop_subagents_in_order`], pinning `(host_instance_id, conn_id)`
    /// before EVERY dispatch and abandoning remaining stops if either changes
    /// mid-sequence (Astra #5).
    pub fn stop_subagents_in_order_pinned(&self, host_key: &str, pin: Option<(u64, u64)>, session_id: &str, window_ids: Vec<String>) {
        let (inner, hk, sid) = (self.inner.clone(), host_key.to_owned(), session_id.to_owned());
        self.rt.spawn(async move {
            for window_id in window_ids {
                if let Some((want_inst, want_conn)) = pin
                    && !inner.host_matches_pin(&hk, want_inst, want_conn)
                {
                    tracing::warn!("Aborting remaining subagent stops on {hk}: host instance or connection changed");
                    break;
                }
                let answer = inner.ask_agents_pinned(&hk, pin, "stop", Some(json!({ "args": [window_id, "--json", "--tree"] })), Some(sid.clone()), None, None).await;
                if !answer.ok {
                    tracing::warn!("Stopping a child tab on {hk}: {}", answer.stderr.unwrap_or_default().trim());
                }
            }
        });
    }

    /// The tabs of the agents a minder minds now, then the minder's own:
    /// the order to stop them in ([`Self::stop_subagents_in_order`]).
    /// By who minds each (`minded_by`), read when asked: never by where
    /// rows stand in a list, which a filter thins (another minder's
    /// agent stood under this one's row with its own minder filtered
    /// out, and was stopped for this one's: a code review, 2026-11-27).
    pub fn minder_and_its_agents(&self, host_key: &str, session_id: &str, minder_window_id: &str) -> Vec<String> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        minder_and_its_agents(&st.subagents.entries, host_key, session_id, minder_window_id)
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
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
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
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
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
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
            }
        });
    }

    pub fn focus_block(&self, host_key: &str, session_id: &str, block_id: &str) {
        self.select(host_key, session_id, "", Some(block_id));
    }

    /// A tab or a pane chosen by the person. **Its panes' size is taken
    /// with it**, where another client holds it: in Rex's protocol a
    /// pane has an owner, whose size is the one that holds, and a pane
    /// somebody else owns is drawn here at their size, from its top
    /// left: larger than this view, its last rows are cut off, where a
    /// program keeps its prompt. An agent's child tab was the agents
    /// service's (which made it, at the size of the pane it was asked
    /// from) for as long as it ran, and nothing here ever took a pane:
    /// Ben, 2026-11-27, "some antigravity agy terminals that are
    /// launched as subagents … it doesn't show the full tui stuff like
    /// the typing line" (a 108 × 64 tab in a view of 56 rows). Whoever
    /// chooses a tab to look at it is the one whose view counts: taken
    /// (`session.own_block`), the next paint sizes it to this view. On
    /// a choice of the person's only, never as it paints: two desktops
    /// showing one pane would take it from each other for ever.
    fn select(&self, host_key: &str, session_id: &str, window_id: &str, block: Option<&str>) {
        let mut to_attach = None;
        let told = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let st = &mut *st;
            let (hosts, blocks) = (&mut st.hosts, &st.blocks);
            let Some(host) = hosts.get_mut(host_key) else { return };
            let Some(view) = host.sessions.get(session_id) else { return };
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
            // (Asked before anything else is: a tab already in front, a
            // pane already the one typed into, is taken all the same; of
            // its panes, those it shows: a zoomed tab's hidden splits, a
            // floating pane put away, keep whose they are.)
            let mut theirs = panes_sized_by_others(host_key, session_id, window, blocks);
            // (A floating pane put away, brought back by this choice, is
            // shown once it is: taken with them.)
            if let (Some(b), Some(_)) = (block, &restore)
                && !theirs.iter().any(|t| t == b)
                && sized_by_another(host_key, session_id, b, blocks)
            {
                theirs.push(b.to_owned());
            }
            // (And a pane of it left with no stream, its tries spent, is
            // had again: choosing its tab is asking to see it.)
            let blank: Vec<(BlockKey, Arc<Mutex<BlockRuntime>>)> = window
                .all_blocks()
                .filter(|b| attaches_a_stream(b))
                .filter_map(|b| {
                    let key = BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: b.block_id.clone() };
                    let runtime = blocks.get(&key)?;
                    (!runtime.lock().unwrap_or_else(|e| e.into_inner()).data_connected).then(|| (key, runtime.clone()))
                })
                .collect();
            let window_id = window.window_id.clone();
            let focused = window.focused_block_id.clone();
            let has_pending_create = host.pending_creates.get(session_id).copied().unwrap_or(0) > 0;
            ask_to_own(host, session_id, theirs, blocks);
            if !blank.is_empty()
                && let (Some(ctrl), Some(ep)) = (host.control.clone(), host.endpoint.clone())
            {
                to_attach = Some((ctrl, ep, blank));
            }
            if !switch && !has_pending_create && restore.is_none() && block.is_none_or(|b| focused.as_deref() == Some(b)) {
                false
            } else {
                let target = Arc::new(FocusTarget { window: window_id, block: block.map(str::to_owned), restore });
                if let Some(view) = host.sessions.get_mut(session_id) {
                    target.apply(view);
                }
                if let Some(tx) = &host.window_tx {
                    let op = HostWindowOp::Focus { session_id: session_id.to_owned(), switch: switch || has_pending_create, target: target.clone() };
                    if tx.send(op).is_ok() {
                        host.pending_focus.insert(session_id.to_owned(), target);
                    }
                }
                true
            }
        };
        if let Some((ctrl, ep, blank)) = to_attach {
            let _guard = self.rt.enter();
            for (key, runtime) in blank {
                tracing::info!("Rex: {} chosen with no stream: had again", key.block_id);
                self.inner.spawn_block_data_stream(ctrl.clone(), ep.clone(), key, runtime, None);
            }
        }
        if told {
            (self.notify)(HubNotice::StateChanged);
        }
    }

    pub fn focus_direction(&self, host_key: &str, session_id: &str, direction: &str) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let dir = direction.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_focus_direction(&sid, &dir).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
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
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
            }
        });
    }

    pub fn create_floating_layer(&self, host_key: &str, session_id: &str, bounds: [f64; 4]) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let opts = self.local_shell_opts(host_key, None);
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_new_layer(&sid, None, bounds, true, opts).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
            }
        });
    }

    /// A floating layer closed, and what ran in it ended.
    pub fn close_layer(&self, host_key: &str, session_id: &str, layer_id: &str) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let lid = layer_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_close_layer(&sid, &lid).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
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
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, v.seq, &[]).await;
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

    /// What the pointer did over a pane, for a program that asked to be
    /// told (a press, a release, a move, a step of the wheel, and the
    /// wheel's arrows where it asked for those): its input like any
    /// other, but not typed by the person. A question waiting there is
    /// still Auto Approve's, and Keep It Going's quiet goes on: only
    /// keys and text make a question the person's, as in Terminal. They
    /// went as typing until 2026-11-27: Claude Code and Codex ask for
    /// every move of the pointer (mode 1003), so the pointer crossing a
    /// pane, a click to bring it forward or the wheel turned to read a
    /// long command each took the question for the person, for good,
    /// and its countdown never showed (Ben, of a question that had
    /// waited so: "it seems auto approve still doesn't work in rex
    /// terminal? i see no countdown"). Auto Approve presses Return only
    /// with the mark seen on Yes, so a click or an arrow that moves the
    /// mark is no danger: it waits while the mark is elsewhere.
    pub fn send_pointer(&self, key: &BlockKey, data: Vec<u8>) {
        self.write(key, data, None);
    }

    /// The same for a button pressed or let go, which a chooser may
    /// answer to. `in_front`: the pane had the keyboard already when the
    /// button went down. Then the click is the person's doing something
    /// in the program, as a key is, and a question waiting there is
    /// theirs: what the program makes of the click is drawn here only
    /// after it, and Return pressed meanwhile would land on whatever the
    /// click moved the mark to. A click that brought the pane forward
    /// (and its release) is not that: the question stays Auto Approve's,
    /// but is not answered until the screen can have caught up with what
    /// the program made of it (`Watch::touched`). (The wheel's arrows are
    /// keys, and go by `send_input`.)
    pub fn send_pointer_press(&self, key: &BlockKey, data: Vec<u8>, in_front: bool) {
        if in_front {
            self.typed(key);
        } else {
            let _in = self.rt.enter();
            self.inner.agent_touched(key);
        }
        self.write(key, data, None);
    }

    /// Whether a pane is the one typed into, in the tab in front of its
    /// session.
    pub fn has_focus(&self, key: &BlockKey) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.hosts.get(&key.host_key).and_then(|h| h.sessions.get(&key.session_id)).is_some_and(|view| {
            view.find_block(&key.block_id).is_some_and(|(window, _, _)| view.active_window_id.as_ref() == Some(&window.window_id) && window.focused_block_id.as_deref() == Some(key.block_id.as_str()))
        })
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
        self.typed(key);
        self.write(key, data, connection);
    }

    /// Typed by the person in a pane (a key, text, a paste): a question
    /// waiting there is theirs.
    fn typed(&self, key: &BlockKey) {
        let _in = self.rt.enter();
        self.inner.agent_typed(key);
    }

    fn write(&self, key: &BlockKey, data: Vec<u8>, connection: Option<&ControlClient>) {
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
        // (A key is typed whichever way it goes: these, the keys the
        // server encodes (all of a program's that asked for the kitty
        // protocol's, Escape among them, and every key with Control or
        // Alt), left a question counting down under the person's own
        // key until 2026-11-27: Escape pressed at a question, and
        // Return was still pressed for it four seconds later.)
        self.typed(key);
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

    /// A pane's view is this many cells of this many pixels: its
    /// terminal is made so here at once, and its host is asked to make
    /// its own so (the program in it is told: SIGWINCH). Whether the
    /// terminal here changed (to be drawn again).
    ///
    /// The host is asked one size at a time and the last one wanted
    /// last ([`SizeAsked`]): asked as each came, two at once could be
    /// done there in either order and the terminal left at the earlier;
    /// and a size it wasn't given (its connection busy or lost, a time-
    /// out) is asked again, where it was taken as given and never asked
    /// again: the view had its size, so its next paint asked nothing,
    /// and a program there kept the size it had (Ben, 2026-11-28: "when
    /// i move it to a bigger display the terminal TUIs didn't auto
    /// resize").
    pub fn resize_block(&self, key: &BlockKey, cols: u16, rows: u16, cell_w: u32, cell_h: u32) -> bool {
        let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned() else { return false };
        let (changed, ask) = {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if g.other_size_owner {
                return false;
            }
            let changed = g.resize(cols, rows, cell_w, cell_h).unwrap_or(false);
            g.size.want((cols, rows, cell_w, cell_h));
            // (One asker a pane: one that waits to ask again is woken for
            // the new size, and none is started beside it. Not one whose
            // ask is on its way: it asks the last size wanted when its
            // answer comes, and a wake kept for it meanwhile had its next
            // wait after a failure skipped: Neon R2 #6.)
            if g.size.asker {
                if g.size.asking.is_none() {
                    g.size_wake.notify_one();
                }
                (changed, None)
            } else {
                let ask = g.size.next();
                g.size.asker = ask.is_some();
                (changed, ask)
            }
        };
        if let Some(asked) = ask {
            let _guard = self.rt.enter();
            self.inner.ask_size(key.clone(), br, asked);
        }
        changed
    }

    /// Where a pane's Open in Tracker opens ([`tracker_place`]): its
    /// program's folder now, else the one its agent was started in
    /// (`olo agent`'s word, for an exited child or one whose host hasn't
    /// said its folder yet), else home. With `pin` (the host's live pin
    /// as a menu was made, `host_live_pin`): none where the host is
    /// another or no longer connected since (its paths would be another
    /// host's, or not reached).
    pub(crate) fn tracker_place(&self, bk: &BlockKey, pin: HostPin) -> Option<TrackerPlace> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match pin {
            HostPin::Any => {}
            HostPin::MustMatch(Some((instance, conn))) if host_matches_pin_in(&st.hosts, &bk.host_key, instance, conn) => {}
            HostPin::MustMatch(_) => return None,
        }
        // (And Tracker reaches it still: connected, over SSH, its helper
        // allowed there.)
        if !crate::files_sock::tracker_reaches(&st, &bk.host_key) {
            return None;
        }
        let host = st.hosts.get(&bk.host_key)?;
        let live = st.blocks.get(bk).map(|b| {
            let g = b.lock().unwrap_or_else(|e| e.into_inner());
            block_cwd_candidates(&g.pwd, g.process.as_ref())
        });
        let started_in = st.subagents.by_block_any(&bk.host_key, Some(&bk.session_id), &bk.block_id).and_then(|e| e.cwd.as_deref());
        let home = host.is_local.then(local_home).flatten();
        Some(tracker_place(host.is_local, &tracker_authority(&st.hosts, host), live.as_deref().unwrap_or_default(), started_in, home.as_deref()))
    }

    /// The host's live pin (`host_live_pin`) where Tracker can reach its
    /// folders now (`files_sock::tracker_reaches`: this computer; a host
    /// connected over SSH with Olo's helper allowed there), none
    /// otherwise: what a menu offering Open in Tracker is made with,
    /// enabled by, and checked against when chosen.
    pub(crate) fn tracker_pin(&self, host_key: &str) -> Option<(u64, u64)> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !crate::files_sock::tracker_reaches(&st, host_key) {
            return None;
        }
        host_live_pin_in(&st.hosts, host_key)
    }

    /// The panes of the tab in front of a session another client holds
    /// the size of are taken, as choosing the tab takes them
    /// ([`Self::select`]): for a window of Rex's the person brings
    /// forward, or that changes size while it is forward (moved to a
    /// larger display, the display's scale changed, the window resized):
    /// the person is looking at it, and its view is the one whose size
    /// counts. Never as it paints. Whether any was asked for.
    pub fn take_tab_in_front(&self, host_key: &str, session_id: &str) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(host) = st.hosts.get(host_key) else { return false };
        let Some(window) = host.sessions.get(session_id).and_then(|view| view.active_window().or_else(|| view.windows.first())) else { return false };
        // (Those it shows: a tab with a pane zoomed shows that pane alone,
        // and a floating pane put away shows only its button; the others
        // keep whose they are.)
        let theirs = panes_sized_by_others(host_key, session_id, window, &st.blocks);
        ask_to_own(host, session_id, theirs, &st.blocks)
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
        let sampled = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| Some((h.endpoint.clone()?, h.host_instance_id)));
        let Some((RexEndpoint::Ssh { target, pool, askpass }, inst_id)) = sampled else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        let (putting, putting_on) = (thing == HostThing::Screens && op != HostOp::Remove, host_key.clone());
        let task = self.rt.spawn(async move {
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
                    let helper_lock = crate::files_sock::helper_host_lock(&host_key);
                    let _helper_guard = helper_lock.lock().await;
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
                    inner.helper_stands(&host_key, inst_id, stands, false);
                    said
                }
                (HostThing::Helper, HostOp::Remove) => {
                    let helper_lock = crate::files_sock::helper_host_lock(&host_key);
                    let _helper_guard = helper_lock.lock().await;
                    let was_off = inner.state.lock().unwrap_or_else(|e| e.into_inner()).settings.helper_hosts.get(&host_key).map(String::as_str) == Some("off");
                    // Publish opt-out and clear readiness BEFORE running REMOVE
                    // so concurrent files_sock preparations see DisabledForHost
                    // immediately and cannot reinstall after REMOVE runs.
                    inner.helper_kept_off(&host_key, true);
                    if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                        host.helper_current = None;
                        host.helper_sync_err = None;
                    }
                    let ran = run(script(olo_rex_client::helper::REMOVE)).await;
                    if ran.is_ok() {
                        // (A connection that goes through it goes on
                        // through it: said so while it does.)
                        if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key)
                            && host.helper != HostHelper::Used
                        {
                            host.helper = HostHelper::None;
                        }
                        inner.screens_service.wake_host_follower(&inner.rt_handle, &host_key, inst_id);
                    } else if !was_off {
                        inner.helper_kept_off(&host_key, false);
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
                // (The host's helper puts Olo there, from Olo's release,
                // and says how far it is, a line a step: each heard, and
                // kept for the window.)
                (HostThing::Screens, HostOp::Install | HostOp::Update) => {
                    use crate::host_screens::{Heard, Installing, helper_script};
                    let mut heard = Installing::default();
                    let (mut error, mut held) = (None, None);
                    let doing = |words: Option<(String, f32)>| {
                        if let Some(host) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key) {
                            host.screens_doing = words;
                        }
                        (inner.notify)(HubNotice::StateChanged);
                    };
                    // (A host whose helper is from before it had screens,
                    // or has none: this Olo's put there first, as the
                    // Rex tab's own Install puts it. One click, not a
                    // line that sends the person to another tab: a
                    // reviewer's.)
                    let helper_there = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).and_then(|h| h.screens.as_ref().map(|s| s.helper));
                    if helper_there == Some(false) && crate::link::carried() {
                        doing(Some((olo_kit::words::tr("Putting the typing helper there").to_string(), 0.0)));
                        inner.helper_kept_off(&host_key, false);
                        let stands = olo_rex_client::helper::ensure(&pool, &target, askpass.clone(), crate::link::ID, &crate::link::BUILDS).await;
                        inner.helper_stands(&host_key, inst_id, stands, false);
                    }
                    doing(Some((olo_kit::words::tr("Asking Olo’s releases").to_string(), 0.0)));
                    let ran = pool
                        // (Ended with its channel, where this is stopped: Cancel.)
                        .exec(&target, askpass.clone(), &script(&olo_rex_client::ready::cancellable(&helper_script("install"))), &mut |line| match heard.hear(line) {
                            Heard::Progress(words, far) => doing(Some((words, far))),
                            Heard::Error(said) => error = Some(said),
                            Heard::Held(names) => held = Some(names),
                            Heard::Nothing => {}
                        })
                        .await;
                    doing(None);
                    match (ran, error, held) {
                        (Err(e), ..) => Err(e.to_string()),
                        (Ok(ran), ..) if ran.status == Some(69) => {
                            Err(olo_kit::words::tr("This host’s typing helper is from before it had screens: update it (the Rex tab), then install.").to_string())
                        }
                        (Ok(_), _, Some(names)) => Err(olo_kit::tr!("Still running: {}. Update once they are stopped.", names.join(", ")).to_string()),
                        (Ok(_), Some(said), None) => Err(crate::host_screens::why_not_installed(&said)),
                        // (3: Olo is there, and the host lacks something
                        // its screens need: said on its line, as it stands.)
                        (Ok(ran), None, None) if matches!(ran.status, Some(0 | 3)) => Ok(()),
                        (Ok(ran), None, None) => Err(ran.output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn’t say why").trim().to_owned()),
                    }
                }
                (HostThing::Screens, HostOp::Remove) => run(script(&crate::host_screens::helper_script("remove"))).await.map_err(|said| crate::host_screens::why_not_removed(&said)),
            };
            if thing != HostThing::Helper && thing != HostThing::Pool {
                inner.ask_what_the_host_has(&host_key, &pool, &target, askpass, false).await;
            }
            inner.screens_putting.lock().unwrap_or_else(|e| e.into_inner()).remove(&host_key);
            (inner.notify)(HubNotice::StateChanged);
            done(result);
        });
        if putting {
            self.inner.screens_putting.lock().unwrap_or_else(|e| e.into_inner()).insert(putting_on, task.abort_handle());
        }
    }

    /// Olo being put on a far host for its screens is stopped where it
    /// is (the Host window's Cancel): what was running there ends with
    /// its channel (`ready::cancellable`), what was fetched is kept for
    /// the next try, and the host is asked again what it has. Whether
    /// anything was stopped.
    pub fn cancel_screens(&self, host_key: &str, done: impl FnOnce() + Send + 'static) -> bool {
        let Some(task) = self.inner.screens_putting.lock().unwrap_or_else(|e| e.into_inner()).remove(host_key) else { return false };
        task.abort();
        if let Some(host) = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(host_key) {
            host.screens_doing = None;
        }
        (self.inner.notify)(HubNotice::StateChanged);
        self.check_host(host_key, done);
        true
    }

    /// Stop one displayed screen from the Host window through Rex's guarded
    /// Stop path (`stop-expected` + `--expected-started`), pinned to the exact
    /// `(host_key, host_instance_id)` the button was drawn for.
    pub fn stop_screen_guarded(&self, host_key: &str, host_instance_id: u64, name: &str, expected_started: Option<u64>, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        let sampled = {
            let st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).filter(|h| h.host_instance_id == host_instance_id).and_then(|h| h.endpoint.clone())
        };
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = sampled else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key, name) = (self.inner.clone(), host_key.to_owned(), name.to_owned());
        self.rt.spawn(async move {
            let result = inner.screens_service.stop_screen_for_host_instance(&host_key, host_instance_id, &name, expected_started).await;
            let still_same = inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).is_some_and(|h| h.host_instance_id == host_instance_id);
            if still_same {
                inner.ask_what_the_host_has(&host_key, &pool, &target, askpass, false).await;
                (inner.notify)(HubNotice::StateChanged);
            }
            done(result);
        });
    }

    /// Where an agent's screen opens on a far host when the agent
    /// doesn't say: the host's own (`true`), or this computer's as it
    /// always was. The host's helper keeps the word there (`olo screen
    /// default`); the host is asked again after.
    pub fn screens_default(&self, host_key: &str, on_host: bool, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        let endpoint = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| h.endpoint.clone());
        let Some(RexEndpoint::Ssh { target, pool, askpass }) = endpoint else {
            done(Err(olo_kit::words::tr("This host isn’t reached over SSH.").into()));
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            let command = format!("sh -c {}", shell_quote(&crate::host_screens::helper_script(if on_host { "default host" } else { "default desktop" })));
            let result = match pool.exec(&target, askpass.clone(), &command, &mut |_| {}).await {
                Ok(ran) if ran.ok() => Ok(()),
                Ok(ran) => Err(ran.output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn’t say why").trim().to_owned()),
                Err(e) => Err(e.to_string()),
            };
            inner.ask_what_the_host_has(&host_key, &pool, &target, askpass, false).await;
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
        let sampled = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).and_then(|h| Some((h.endpoint.clone()?, h.host_instance_id)));
        let Some((RexEndpoint::Ssh { target, pool, askpass }, inst_id)) = sampled else {
            done();
            return;
        };
        let (inner, host_key) = (self.inner.clone(), host_key.to_owned());
        self.rt.spawn(async move {
            use olo_rex_client::helper::{Asked, ask};
            inner.ask_what_the_host_has(&host_key, &pool, &target, askpass.clone(), false).await;
            inner.ask_the_pools().await;
            let wake_kind = if crate::link::carried()
                && let Ok(Ok(asked)) = ask(&pool, &target, askpass, crate::link::ID, &crate::link::BUILDS).await
            {
                let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(host) = st.hosts.get_mut(&host_key).filter(|h| h.host_instance_id == inst_id) {
                    let prev_helper = host.helper.clone();
                    let prev_current = host.helper_current;
                    match asked {
                        Asked::Here => {
                            host.helper_current = Some(true);
                            host.helper_sync_err = None;
                            if matches!(host.helper, HostHelper::None | HostHelper::Without(_)) {
                                host.helper = HostHelper::Ready;
                            }
                            if prev_current != Some(true) || matches!(prev_helper, HostHelper::None | HostHelper::Without(_)) { Some(true) } else { Some(false) }
                        }
                        Asked::Outdated(_) => {
                            host.helper_current = Some(false);
                            if matches!(host.helper, HostHelper::None) {
                                host.helper = HostHelper::Ready;
                            }
                            if matches!(prev_helper, HostHelper::None | HostHelper::Without(_)) || prev_current == Some(true) { Some(true) } else { Some(false) }
                        }
                        Asked::Needs(_) => {
                            host.helper_current = if host.helper == HostHelper::Used { Some(false) } else { None };
                            if matches!(host.helper, HostHelper::Ready) {
                                host.helper = HostHelper::None;
                            }
                            if prev_current.is_some() || !matches!(prev_helper, HostHelper::None) { Some(true) } else { None }
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
            match wake_kind {
                Some(true) => inner.screens_service.wake_host_follower(&inner.rt_handle, &host_key, inst_id),
                Some(false) => inner.screens_service.wake_host_if_parked(&inner.rt_handle, &host_key, inst_id),
                None => {}
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
        // Never overwrite an explicit per-host opt-out ("off") with a stale "yes"
        // from an in-flight automatic helper check.
        if known == Some(true) && st.settings.helper_hosts.get(host_key).is_some_and(|w| w == "off") {
            return;
        }
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
    fn helper_stands(&self, host_key: &str, expected_inst_id: u64, stands: Result<olo_rex_client::helper::Stands, olo_rex_client::provision::Failed>, in_use: bool) {
        use olo_rex_client::helper::Stands;
        let newly_installed = matches!(stands, Ok(Stands::Installed { .. }));
        let already_there = matches!(stands, Ok(Stands::Already));
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
        let matched = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(host) = st.hosts.get_mut(host_key).filter(|h| expected_inst_id == 0 || h.host_instance_id == expected_inst_id) {
                if host.helper == HostHelper::Used {
                    host.helper_current = current;
                    host.helper_sync_err = sync_err;
                } else {
                    host.helper = helper;
                    host.helper_current = if current == Some(true) { Some(true) } else { None };
                    host.helper_sync_err = None;
                }
                Some(host.host_instance_id)
            } else {
                None
            }
        };
        if let Some(inst_id) = matched {
            if newly_installed {
                self.screens_service.wake_host_follower(&self.rt_handle, host_key, inst_id);
            } else if already_there {
                self.screens_service.wake_host_if_parked(&self.rt_handle, host_key, inst_id);
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
        let inst_id = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // (Or it was taken off this host here: not put back by itself.)
            let known = st.settings.helper_hosts.get(host_key).is_some_and(|word| word == "yes" || word == "off" || word.strip_prefix("no:") == Some(crate::link::ID));
            if !crate::link::carried() || !st.settings.host_helper || known {
                return false;
            }
            st.hosts.get(host_key).map_or(0, |h| h.host_instance_id)
        };
        let build = match ask(pool, target, askpass.clone(), crate::link::ID, &crate::link::BUILDS).await {
            Ok(Ok(Asked::Here)) => {
                self.helper_stands(host_key, inst_id, Ok(Stands::Already), true);
                return true;
            }
            Ok(Ok(Asked::Needs(build) | Asked::Outdated(build))) => build,
            Ok(Err(kind)) => {
                self.helper_stands(host_key, inst_id, Ok(Stands::NoBuild(kind)), true);
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
                self.helper_stands(host_key, inst_id, done, true);
                there
            }
            Err(_) => {
                let (hub, host_key) = (self.clone(), host_key.to_owned());
                tokio::spawn(async move {
                    let done = sending.await.unwrap_or_else(|e| Err(e.to_string().into()));
                    hub.helper_stands(&host_key, inst_id, done, false);
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
        // (The connection it is given for: what the forward ends in is said
        // of the host only while that is still the host's, so that one
        // given for a connection since replaced never sets or clears what
        // the new one's said: Astra R2 #10.)
        let given_for = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).and_then(|h| h.control.clone());
        tokio::spawn(async move {
            let forwarded = pool.forward_agent(&target, askpass).await;
            let still = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.hosts.get(&host_key).and_then(|h| h.control.as_ref()).is_some_and(|now| given_for.as_ref().is_some_and(|given| given.same_connection(now)))
            };
            if !still {
                tracing::debug!("Rex: the SSH agent given to {host_key} for a connection since replaced: left as it is");
                return;
            }
            match forwarded {
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
        let (probe_inst_id, probe_conn_id) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).map_or((0, 0), |h| (h.host_instance_id, h.screens_conn_id))
        };
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
        // (Its screens of its own, as its helper says: `host_screens`.
        // Not told, what was last heard stands.)
        let screens = match pool.exec(target, askpass.clone(), &format!("sh -c {}", shell_quote(&crate::host_screens::asked_script())), &mut |_| {}).await {
            Ok(ran) => crate::host_screens::ScreensThere::read(ran.status, &ran.output),
            Err(_) => None,
        };
        if auto_sync && commands_cur == Some(false) {
            match pool.exec(target, askpass, &format!("sh -c {}", shell_quote(&bridge::install_commands_script())), &mut |_| {}).await {
                Ok(r) if r.ok() => commands_cur = Some(true),
                Ok(r) => commands_err = Some(err_of(&r.output)),
                Err(e) => commands_err = Some(e.to_string()),
            }
        }
        let bridge_there = true;
        let (lacking, helper_has_screens) = match &screens {
            Some(s) => (s.lacking(), Some(s.helper_follow)),
            None => (None, None),
        };
        let changed = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let live_screens = st.host_screens.get(host_key).cloned();
            let Some(host) = st.hosts.get_mut(host_key) else { return };
            // Drop stale probe completions if the host was replaced or reconnected.
            if probe_inst_id != 0 && (host.host_instance_id != probe_inst_id || (probe_conn_id != 0 && host.screens_conn_id != probe_conn_id)) {
                return;
            }
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
            // Merge live follower status and rows ON TOP of probe-owned fields
            // (`chrome`, `missing`, `older`, `software`, `bytes`) ONLY when a
            // real follower snapshot arrived for THIS connection (Findings 12 & 13).
            // Older helpers without `screen follow` keep their probe rows intact!
            let mut screens = screens.or_else(|| host.screens.clone());
            if let (Some(there), Some(live)) = (screens.as_mut(), live_screens.as_ref()) {
                let _ = merge_live_screens_into_there(there, live, probe_conn_id);
            }
            let changed = prev != next || host.screens != screens;
            host.screens = screens;
            (host.bridge_there, host.skill_there, host.skill_current, host.skill_sync_err, host.commands_there, host.commands_current, host.commands_sync_err) = next;
            changed
        };
        if helper_has_screens.is_some() && probe_inst_id != 0 {
            self.screens_service.set_host_lacking(host_key, probe_inst_id, probe_conn_id, lacking, helper_has_screens);
        }
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
        let inst_id = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let cant = st.settings.helper_hosts.get(host_key).is_some_and(|word| word == "off" || word.strip_prefix("no:") == Some(crate::link::ID));
            if !crate::link::carried() || !st.settings.host_helper || cant {
                return;
            }
            st.hosts.get(host_key).map_or(0, |h| h.host_instance_id)
        };
        let (target, pool, askpass) = (target.clone(), pool.clone(), askpass.clone());
        let (hub, host_key) = (self.clone(), host_key.to_owned());
        if let Some(host) = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut(&host_key).filter(|h| h.host_instance_id == inst_id && h.helper != HostHelper::Used) {
            host.helper = HostHelper::Installing;
        }
        tokio::spawn(async move {
            let host_lock = crate::files_sock::helper_host_lock(&host_key);
            let _guard = host_lock.lock().await;
            let allowed = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                let cant = st.settings.helper_hosts.get(&host_key).is_some_and(|word| word == "off" || word.strip_prefix("no:") == Some(crate::link::ID));
                crate::link::carried() && st.settings.host_helper && !cant
            };
            if !allowed {
                return;
            }
            let stands = olo_rex_client::helper::ensure(&pool, &target, askpass, crate::link::ID, &crate::link::BUILDS).await;
            let still_allowed = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                let cant = st.settings.helper_hosts.get(&host_key).is_some_and(|word| word == "off");
                st.settings.host_helper && !cant
            };
            if !still_allowed {
                return;
            }
            hub.helper_stands(&host_key, inst_id, stands, false);
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
                Do::Down(answers) => {
                    tracing::info!(pane = bk.block_id, question = question.unwrap_or_default(), answers, "auto approve: its yes for this once is further down; the mark moved to it");
                    for _ in 0..answers {
                        let _ = ctrl.terminal_write(&bk.session_id, &bk.block_id, "\x1b[B").await;
                        tokio::time::sleep(Duration::from_millis(45)).await;
                    }
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

    /// The pointer pressed in a pane, or its wheel's arrows went to it:
    /// a question counting down there waits for the screen to settle.
    fn agent_touched(self: &Arc<Self>, bk: &BlockKey) {
        let help = help_of(bk);
        if !help.auto_approve {
            return;
        }
        let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(bk).cloned() else { return };
        if br.lock().unwrap_or_else(|e| e.into_inner()).watch.touched(Instant::now()) {
            self.agent_schedule(bk, &br, &help);
        }
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
        // (How many panes the host had when its connection was last lost,
        // which are let go of with it: as many streams are opened
        // beforehand when it is made again. Counted once they were gone,
        // it was always none, and one was opened.)
        let mut panes_at_loss = 0usize;
        // (The network the host's SSH connections were made on.)
        let mut pool_net = olo_net::generation();
        loop {
            let (endpoint_raw, reconnect_token, user_overrides, is_local_host, host_inst_id, is_ssh_host, connecting_seq) = {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let overrides = st.settings.keymap_overrides.clone();
                let Some(h) = st.hosts.get_mut(&host_key) else { return };
                if !h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry)) {
                    return;
                }
                let connecting_seq = if attempt == 0 {
                    h.conn_state = HostConnState::Connecting;
                    h.screens_state_seq += 1;
                    Some(h.screens_state_seq)
                } else {
                    None
                };
                let is_ssh = crate::screens_sock::is_ssh_endpoint(&h.endpoint_raw);
                (h.endpoint_raw.clone(), h.reconnect_token.clone(), overrides, h.is_local, h.host_instance_id, is_ssh, connecting_seq)
            };
            if let Some(state_seq) = connecting_seq
                && !is_local_host
                && is_ssh_host
            {
                self.screens_service.host_state_changed(&host_key, host_inst_id, None, state_seq, olo_screen_pane::rex_sock::RexHostConn::Connecting, None, None);
            }
            (self.notify)(HubNotice::StateChanged);

            let endpoint = match RexEndpoint::parse(&endpoint_raw, self.ssh_pool.clone(), self.askpass.clone()) {
                Ok(ep) => ep,
                Err(e) => {
                    let err_msg = e.to_string();
                    let err_seq = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.host_instance_id == host_inst_id && h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry))) {
                            h.conn_state = HostConnState::Error(err_msg.clone());
                            h.screens_conn_id = 0;
                            h.screens_state_seq += 1;
                            h.retry = None;
                            Some(h.screens_state_seq)
                        } else {
                            None
                        }
                    };
                    if let Some(state_seq) = err_seq
                        && !is_local_host
                        && is_ssh_host
                    {
                        self.screens_service.host_state_changed(&host_key, host_inst_id, None, state_seq, olo_screen_pane::rex_sock::RexHostConn::Error, Some(err_msg), None);
                    }
                    (self.notify)(HubNotice::StateChanged);
                    return;
                }
            };

            // A connection lives on the network it was made on: once the
            // network has changed since the host's SSH connections were
            // made, this attempt (and its helper's) goes on new ones, not
            // on the old network's, which may not know yet that they're
            // gone (where an address stayed and the way out changed, the
            // kernel ends one only when it has gone 15 s unanswered, and a
            // channel asked of it meanwhile waits). Looked at before each
            // attempt, so a change during the wait between two, or during
            // one, is never missed.
            // (The pool is another saved host's too where both reach the
            // same account: it lets go of a target's connections once a
            // network, so the second to look doesn't let go of what the
            // first made on the new one: Astra R2 #9.)
            let network_moved = |pool_net: &mut u64| {
                let net = olo_net::generation();
                if net != *pool_net {
                    if let RexEndpoint::Ssh { target, pool, .. } = &endpoint
                        && pool.on_network_change(target, net)
                    {
                        tracing::info!("Rex: the network changed since {host_key}'s SSH connections were made: made anew");
                    }
                    *pool_net = net;
                }
            };
            network_moved(&mut pool_net);

            // (A host nothing is known of: its helper first, so this
            // connection goes through it.)
            let helper_seen = self.helper_before_connecting(&host_key, &endpoint).await;
            // (And looked at again after it: a network that changed while
            // the helper was being put there, over the old one's SSH, would
            // have this attempt reach the host on the old one's too, and
            // wait through its time-outs: Astra R2 #8.)
            network_moved(&mut pool_net);

            // (Connecting in as few round trips of the link as it takes,
            // HACKING, Rex, "Connecting in fewer round trips": nine
            // passed before a terminal's first screen came, one after
            // another, and over SSH more.)
            let terminals_known = terminals_of(&self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks, &host_key).max(panes_at_loss);
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
                    let SessionSnapshot { seq: boot_seq, rows: summaries, views: initial_views, tickets: pre_tickets } = boot;

                    // Spawn one strictly-FIFO input writer task for this host connection so rapid
                    // keystrokes can never race and arrive out of order on `terminal.write`.
                    // Window focus and lifecycle ops run on their own FIFO worker (`window_worker`)
                    // so typing on any session never stalls behind tab creation or placement.
                    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<HostInputOp>();
                    let (win_tx, mut win_rx) = tokio::sync::mpsc::unbounded_channel::<HostWindowOp>();
                    let ctrl_in = ctrl.clone();
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
                                let room = MOST_UNANSWERED - 1;
                                while unanswered.len() > room {
                                    let Some(done) = oldest_answer(&mut unanswered).await else { break };
                                    inner_in.input_answered(&host_in, &done.session_id, &done.block_id, done.seq);
                                }
                                match op {
                                    // (Taken: its size is this client's to set from
                                    // now, said here at once so the next paint sets
                                    // it; the server's word of it follows.)
                                    HostInputOp::Own { session_id, block_ids } => {
                                        for block_id in block_ids {
                                            let key = BlockKey { host_key: host_in.to_string(), session_id: session_id.clone(), block_id };
                                            let block = inner_in.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&key).cloned();
                                            let Some(block) = block else { continue };
                                            // (Still another's? It may have been given
                                            // up, or taken, while this waited its turn:
                                            // looked at, and let go of if not, at once.)
                                            {
                                                let mut g = block.lock().unwrap_or_else(|e| e.into_inner());
                                                if !g.other_size_owner {
                                                    g.owning = false;
                                                    continue;
                                                }
                                            }
                                            let taken = ctrl_in.session_own_block(&session_id, &key.block_id).await;
                                            let mut g = block.lock().unwrap_or_else(|e| e.into_inner());
                                            g.owning = false;
                                            // (Taken as the host numbered it: a later word
                                            // of whose it is, come first, stands. It was
                                            // taken as this client's whatever came between:
                                            // Astra R2 #11, Neon R2 #3.)
                                            if let Ok(answer) = taken {
                                                g.owner_said(answer.seq, ctrl_in.client_id(), ctrl_in.client_id());
                                            }
                                        }
                                        (inner_in.notify)(HubNotice::StateChanged);
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

                    let ctrl_win = ctrl.clone();
                    let ep_win = endpoint.clone();
                    let host_win = host_key.clone();
                    let inner_win = self.clone();
                    let window_connection = ctrl.clone();
                    let window_worker = tokio::spawn(async move {
                        let run_ops = async {
                            let mut chain = CreateChainTracker::default();
                            let mut placement_tail: HashMap<String, tokio::sync::oneshot::Receiver<()>> = HashMap::new();
                            while let Some(op) = win_rx.recv().await {
                                match op {
                                    HostWindowOp::Focus { session_id, switch, target } => {
                                        chain.on_focus(&session_id);
                                        if switch || target.block.is_none() {
                                            let _ = ctrl_win.session_focus_window(&session_id, &target.window).await;
                                        }
                                        if let Some(l) = &target.restore {
                                            let _ = ctrl_win.session_set_layer_state(&session_id, l, "normal", true).await;
                                        }
                                        if let Some(b) = &target.block {
                                            let _ = ctrl_win.session_focus_block(&session_id, b).await;
                                        }
                                        let view = ctrl_win.session_view(&session_id).await;
                                        {
                                            let mut st = inner_win.state.lock().unwrap_or_else(|e| e.into_inner());
                                            let Some(host) = st.hosts.get_mut(&host_win) else { continue };
                                            if !host.pending_focus.get(&session_id).is_some_and(|newest| Arc::ptr_eq(newest, &target)) {
                                                continue;
                                            }
                                            host.pending_focus.remove(&session_id);
                                        }
                                        if let Ok(view) = view {
                                            inner_win.ingest_session(&host_win, &ctrl_win, &ep_win, view.payload, view.seq, &[]).await;
                                        }
                                    }
                                    HostWindowOp::CreateWindow { session_id, explicit_cwd, source_root_id, active_block_id, known_cwds, base_opts, feedback, on_created } => {
                                        let orig_root = source_root_id.clone();
                                        let effective_root = chain.resolve_source_root(&session_id, source_root_id);
                                        if let Some((new_wid, guard)) = inner_win
                                            .start_create_window(&host_win, &ctrl_win, &ep_win, &session_id, explicit_cwd, effective_root.as_deref(), active_block_id, known_cwds, base_opts, feedback)
                                            .await
                                        {
                                            if let Some(ref wid) = new_wid {
                                                chain.record_created(&session_id, orig_root.as_deref(), wid);
                                            }
                                            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                                            let prev_placement = placement_tail.insert(session_id.clone(), done_rx);
                                            let (inner_p, hk_p, ctrl_p, ep_p) = (inner_win.clone(), host_win.clone(), ctrl_win.clone(), ep_win.clone());
                                            tokio::spawn(async move {
                                                inner_p.finish_create_window_placement(&hk_p, &ctrl_p, &ep_p, &session_id, new_wid, effective_root, guard, prev_placement, feedback, on_created).await;
                                                let _ = done_tx.send(());
                                            });
                                        }
                                    }
                                }
                            }
                        };
                        tokio::select! { biased; _=window_connection.disconnected()=>{}, _=run_ops=>{} }
                    });

                    let screens_connected = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        let preferred_helper = (st.settings.host_helper && crate::link::carried()).then_some(crate::link::ID);
                        if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.host_instance_id == host_inst_id && h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry))) {
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
                            h.window_tx = Some(win_tx);
                            let res = if !is_local_host && let RexEndpoint::Ssh { target, pool, askpass } = &endpoint {
                                let conn_id = crate::screens_sock::next_conn_id();
                                h.screens_conn_id = conn_id;
                                h.screens_state_seq += 1;
                                let state_seq = h.screens_state_seq;
                                let pinned = Arc::new(crate::screens_sock::PinnedSshTarget { target: target.clone(), pool: pool.clone(), askpass: askpass.clone(), preferred_helper });
                                Some((conn_id, state_seq, h.label.clone(), pinned))
                            } else {
                                None
                            };
                            st.bump_topology();
                            res
                        } else {
                            ctrl.close();
                            input_worker.abort();
                            window_worker.abort();
                            for spare in spares {
                                spare.abort();
                            }
                            return;
                        }
                    };
                    // (Its streams opened beforehand, now that it is the
                    // host's: for its own panes.)
                    self.keep_spares(&host_key, &ctrl, spares);
                    // (The network changed while it was being made: the
                    // change's probe of each connection missed this one,
                    // not yet the host's, which may have been made on the
                    // old network's way out. Asked now: one gone ends in
                    // seconds, and is made again on the new network.)
                    if olo_net::generation() != pool_net {
                        tracing::info!("Rex: the network changed while {host_key} was being connected to: asked whether it still is");
                        ctrl.probe();
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
                    self.follow_agents(&host_key);
                    if let Some((conn_id, state_seq, label, pinned)) = screens_connected {
                        let still_live = self
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .hosts
                            .get(&host_key)
                            .is_some_and(|h| h.host_instance_id == host_inst_id && h.screens_conn_id == conn_id && h.screens_state_seq == state_seq);
                        if still_live {
                            self.screens_service.host_connected(&tokio::runtime::Handle::current(), &host_key, host_inst_id, conn_id, state_seq, &label, Some(pinned));
                        }
                    }

                    // (A session shown in a window first: its panes are
                    // what the person is waiting to see.)
                    let mut initial_views = initial_views;
                    let shown = self.sessions_shown(&host_key);
                    initial_views.sort_by_key(|(v, _)| !shown.contains(&v.session_id));
                    for (view, seq) in initial_views {
                        self.ingest_session(&host_key, &ctrl, &endpoint, view, seq, &pre_tickets).await;
                    }
                    // (The streams opened beforehand that no terminal
                    // took are let go: one kept would be closed by the
                    // host before a pane opened later could use it.)
                    self.let_go_of_spares(&host_key, &ctrl);
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
                    window_worker.abort();
                    let dropped_host = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        let label = if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.control.as_ref().is_some_and(|c| c.same_connection(&ctrl))) {
                            h.tailscale = None;
                            h.server_status = None;
                            h.server_version = None;
                            h.sessions.clear();
                            h.pending_focus.clear();
                            h.pending_placements.clear();
                            h.pending_creates.clear();
                            for (_, (_, _, notify)) in h.creating_defer.drain() {
                                notify.notify_waiters();
                            }
                            h.session_summaries.clear();
                            h.control = None;
                            h.input_tx = None;
                            h.window_tx = None;
                            h.window_tx = None;
                            let prev_conn_id = h.screens_conn_id;
                            h.screens_conn_id = 0;
                            h.screens_state_seq += 1;
                            let state_seq = h.screens_state_seq;
                            attempt = 1;
                            h.connection_lost = Some(SystemTime::now());
                            h.conn_state = HostConnState::Reconnecting { attempt: 1, last_error: "Connection closed".to_owned() };
                            (h.label.clone(), prev_conn_id, state_seq)
                        } else {
                            return;
                        };
                        for f in st.forwards.iter_mut().filter(|f| f.host_key == host_key) {
                            f.state = ForwardState::Waiting;
                        }
                        panes_at_loss = terminals_of(&st.blocks, &host_key);
                        st.retain_blocks(|k| k.host_key != host_key);
                        st.bump_topology();
                        label
                    };
                    self.invalidate_agents_follower(&host_key, false);
                    if !is_local_host && is_ssh_host {
                        self.screens_service.host_state_changed(
                            &host_key,
                            host_inst_id,
                            (dropped_host.1 != 0).then_some(dropped_host.1),
                            dropped_host.2,
                            olo_screen_pane::rex_sock::RexHostConn::Reconnecting,
                            Some("Connection closed".to_owned()),
                            None,
                        );
                    }
                    (self.notify)(HubNotice::Attention {
                        target: Target::Host(host_key.clone()),
                        title: format!("{} disconnected", dropped_host.0),
                        body: "The connection to the Rex server closed.".into(),
                        kind: olo_kit::notify::NoteKind::Important,
                    });
                    self.write_palette_index();
                    (self.notify)(HubNotice::StateChanged);
                }
                Err(e) => {
                    attempt += 1;
                    let err_msg = e.to_string();
                    let reconnect_seq = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key).filter(|h| h.host_instance_id == host_inst_id && h.retry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &retry))) {
                            h.control = None;
                            h.input_tx = None;
                            h.window_tx = None;
                            h.screens_conn_id = 0;
                            h.screens_state_seq += 1;
                            h.conn_state = HostConnState::Reconnecting { attempt, last_error: err_msg.clone() };
                            let seq = h.screens_state_seq;
                            st.bump_topology();
                            Some(seq)
                        } else {
                            None
                        }
                    };
                    if let Some(state_seq) = reconnect_seq
                        && !is_local_host
                        && is_ssh_host
                    {
                        self.screens_service.host_state_changed(&host_key, host_inst_id, None, state_seq, olo_screen_pane::rex_sock::RexHostConn::Reconnecting, Some(err_msg), None);
                    }
                    (self.notify)(HubNotice::StateChanged);
                    if reconnect_seq.is_none() {
                        return;
                    }
                }
            }

            let backoff = Duration::from_secs((1u64 << attempt.min(4)).min(15));
            tokio::select! { _ = tokio::time::sleep(backoff) => {}, _ = retry.notified() => {} }
        }
    }

    /// Phase 1 of `HostWindowOp::CreateWindow` (runs inside `window_worker`):
    /// resolves live CWD candidates, runs `session.new_window` (focusing the
    /// new tab on the server), arms a provisional `PlacementGuard` immediately
    /// for the exact `new_window_id` (retroactively reordering if `WindowCreated`
    /// won), and releases the `window_worker` so subsequent `Focus` ops never
    /// stall behind post-create reordering.
    #[allow(clippy::too_many_arguments)]
    async fn start_create_window(
        self: &Arc<Self>,
        host_key: &str,
        ctrl: &ControlClient,
        ep: &RexEndpoint,
        session_id: &str,
        explicit_cwd: Option<String>,
        source_root_id: Option<&str>,
        active_block_id: Option<String>,
        known_cwds: Vec<String>,
        base_opts: Option<Value>,
        feedback: Option<Feedback>,
    ) -> Option<(Option<String>, PlacementGuard)> {
        struct CreateCountGuard {
            inner: Arc<RexHubInner>,
            hk: String,
            sid: String,
        }
        impl Drop for CreateCountGuard {
            fn drop(&mut self) {
                let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(host) = st.hosts.get_mut(&self.hk)
                    && let Some(n) = host.pending_creates.get_mut(&self.sid)
                {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        host.pending_creates.remove(&self.sid);
                    }
                }
            }
        }
        struct DeferGuard {
            inner: Arc<RexHubInner>,
            hk: String,
            sid: String,
            token: u64,
            notify: Arc<tokio::sync::Notify>,
        }
        impl Drop for DeferGuard {
            fn drop(&mut self) {
                let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(host) = st.hosts.get_mut(&self.hk)
                    && host.creating_defer.get(&self.sid).is_some_and(|(t, _, _)| *t == self.token)
                {
                    host.creating_defer.remove(&self.sid);
                }
                drop(st);
                self.notify.notify_waiters();
            }
        }

        let _count_guard = CreateCountGuard { inner: self.clone(), hk: host_key.to_owned(), sid: session_id.to_owned() };

        // Resolve live process CWDs (`[foreground, shell]`) from the host that
        // owns the PTY first (under a bounded deadline), falling back to cached
        // `known_cwds` only when live lookup returns empty or times out.
        let cwd_candidates = resolve_new_window_cwd_candidates(explicit_cwd, known_cwds, CREATE_WINDOW_CWD_TIMEOUT, || async {
            let Some(bid) = active_block_id.as_deref() else { return Vec::new() };
            match ctrl.terminal_process(session_id, bid).await {
                Ok(proc_res) => cwds_from_process(Some(&proc_res.payload)),
                Err(_) => Vec::new(),
            }
        })
        .await;

        // Briefly defer `WindowCreated` event publication while `session.new_window`
        // is in flight so `PlacementGuard` arms for the exact returned `new_window_id`
        // before any unmoved creation view paints (Astra #9).
        let defer_guard = {
            let token = next_placement_token();
            let notify = Arc::new(tokio::sync::Notify::new());
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(host) = st.hosts.get_mut(host_key) {
                host.creating_defer.insert(session_id.to_owned(), (token, Instant::now() + CREATE_WINDOW_EVENT_DEFER, notify.clone()));
            }
            DeferGuard { inner: self.clone(), hk: host_key.to_owned(), sid: session_id.to_owned(), token, notify }
        };

        let created = create_window_rpc_candidates(&cwd_candidates, base_opts, |opts| ctrl.session_new_window(session_id, None, opts)).await;
        let res = match created {
            Ok(res) => res,
            Err(err) => {
                drop(defer_guard);
                if matches!(err, olo_rex_client::RexError::Timeout) {
                    let refreshed = match tokio::time::timeout(Duration::from_secs(5), ctrl.session_view(session_id)).await {
                        Ok(Ok(v)) => {
                            self.ingest_session(host_key, ctrl, ep, v.payload, v.seq, &[]).await;
                            true
                        }
                        _ => false,
                    };
                    if let Some(fb) = feedback {
                        fb.problem("Couldn’t confirm the new tab.", timeout_problem_detail(refreshed));
                    }
                } else if let Some(fb) = feedback {
                    fb.problem("Couldn’t create the tab.", err);
                }
                return None;
            }
        };

        let new_wid = res.payload.get("window_id").and_then(Value::as_str).map(str::to_owned);
        let mut guard = PlacementGuard { inner: self.clone(), hk: host_key.to_owned(), sid: session_id.to_owned(), token: None };

        // Arm provisional placement IMMEDIATELY for the exact `new_wid` before
        // releasing `defer_guard` or awaiting post-create `session_view` so
        // an early `WindowCreated` event never flickers `new_wid` to the end.
        if let (Some(new_wid), Some(root_id)) = (new_wid.as_deref(), source_root_id) {
            let provisional_before = {
                let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                st.hosts.get(host_key).and_then(|h| h.sessions.get(session_id)).and_then(|sess| successor_top_level_window(&st.subagents, host_key, sess, root_id, Some(new_wid)))
            };
            if let Some(bwid) = provisional_before {
                guard.set(new_wid.to_owned(), bwid);
            }
        }
        drop(defer_guard);
        Some((new_wid, guard))
    }

    /// Phase 2 of `HostWindowOp::CreateWindow` (spawned onto its own task so
    /// subsequent `Focus` ops never stall on reordering): awaits any earlier
    /// placement task on the same session, fetches a fresh post-create view,
    /// moves `new_wid` immediately right of `source_root_id`, verifies post-move
    /// adjacency with one bounded correction (including move-to-end), and retires
    /// `PlacementGuard` atomically inside `ingest_session_retiring`.
    #[allow(clippy::too_many_arguments)]
    async fn finish_create_window_placement(
        self: &Arc<Self>,
        host_key: &str,
        ctrl: &ControlClient,
        ep: &RexEndpoint,
        session_id: &str,
        new_wid: Option<String>,
        source_root_id: Option<String>,
        mut guard: PlacementGuard,
        prev_placement: Option<tokio::sync::oneshot::Receiver<()>>,
        feedback: Option<Feedback>,
        on_created: Option<Box<dyn FnOnce() + Send>>,
    ) {
        if let Some(prev) = prev_placement {
            let _ = prev.await;
        }

        let mut final_view: Option<(SessionView, u64)> = None;

        if let (Some(new_wid), Some(root_id)) = (new_wid.as_deref(), source_root_id.as_deref()) {
            // Fetch fresh post-create view (retry once on transient failure so
            // placement is never silently skipped; Astra #8).
            let post_create = match ctrl.session_view(session_id).await {
                Ok(v) => Ok(v),
                Err(_) => ctrl.session_view(session_id).await,
            };
            match post_create {
                Ok(v) => {
                    let desired = {
                        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        desired_move_for_new_tab(&st.subagents, host_key, &v.payload, root_id, new_wid)
                    };
                    match desired {
                        None => {
                            guard.clear();
                            final_view = Some((v.payload, v.seq));
                        }
                        Some(target_before) => {
                            if let Some(ref bwid) = target_before {
                                guard.set(new_wid.to_owned(), bwid.clone());
                            } else {
                                guard.clear();
                            }
                            let mut move_err = ctrl.session_move_window(session_id, new_wid, target_before.as_deref()).await.err();
                            // Fetch post-move view to verify adjacency (Astra #6)
                            // or recompute once if the target neighbor vanished.
                            match ctrl.session_view(session_id).await {
                                Ok(after) => {
                                    let correction = {
                                        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                                        desired_move_for_new_tab(&st.subagents, host_key, &after.payload, root_id, new_wid)
                                    };
                                    match correction {
                                        None => {
                                            // Adjacency confirmed (even if first move errored
                                            // because the neighbor vanished and `new_wid` is now
                                            // adjacent right of `root_id`).
                                            move_err = None;
                                            final_view = Some((after.payload, after.seq));
                                        }
                                        Some(retry_before) => {
                                            if let Some(ref bwid2) = retry_before {
                                                guard.set(new_wid.to_owned(), bwid2.clone());
                                            } else {
                                                guard.clear();
                                            }
                                            match ctrl.session_move_window(session_id, new_wid, retry_before.as_deref()).await {
                                                Ok(_) => {
                                                    move_err = None;
                                                    final_view = ctrl.session_view(session_id).await.ok().map(|v| (v.payload, v.seq));
                                                }
                                                Err(e) => {
                                                    move_err = Some(e);
                                                    final_view = ctrl.session_view(session_id).await.ok().map(|v| (v.payload, v.seq)).or(Some((after.payload, after.seq)));
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    if move_err.is_none() {
                                        move_err = Some(e);
                                    }
                                }
                            }
                            if let Some(err) = move_err {
                                tracing::warn!("Couldn't place new tab {new_wid}: {err}");
                                if let Some(fb) = feedback {
                                    fb.problem("Couldn’t place the new tab.", err);
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    guard.clear();
                    tracing::warn!("Couldn't refresh session to place new tab {new_wid}: {err}");
                    if let Some(fb) = feedback {
                        fb.problem("Couldn’t place the new tab.", err);
                    }
                }
            }
        }

        if final_view.is_none() {
            final_view = ctrl.session_view(session_id).await.ok().map(|v| (v.payload, v.seq));
        }
        // Atomically retire `guard`'s placement token under `self.state` lock
        // right after `install_view` succeeds (and BEFORE `apply_pending_placements`),
        // so lagging pre-move views cannot flicker while an authoritative
        // post-move view is never rewritten back to stale local order (Astra #7 / Neon #2).
        if let Some((v, seq)) = final_view {
            let retire = guard.take_token();
            self.ingest_session_retiring(host_key, ctrl, ep, v, seq, &[], retire).await;
        } else {
            drop(guard);
        }
        if let Some(cb) = on_created {
            cb();
        }
    }

    async fn ingest_session(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint, view: SessionView, seq: u64, pre_tickets: &[olo_rex_client::DataTicket]) {
        self.ingest_session_retiring(host_key, ctrl, ep, view, seq, pre_tickets, None).await;
    }

    async fn ingest_session_retiring(
        self: &Arc<Self>,
        host_key: &str,
        ctrl: &ControlClient,
        ep: &RexEndpoint,
        view: SessionView,
        seq: u64,
        pre_tickets: &[olo_rex_client::DataTicket],
        retire_placement: Option<u64>,
    ) {
        let session_id = view.session_id.clone();
        let blocks: Vec<BlockInfo> = view.windows.iter().flat_map(|w| w.all_blocks()).cloned().collect();
        let in_front = attach_ranks(&view);

        let mut to_attach = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            {
                let st = &mut *st;
                let Some(h) = st.hosts.get_mut(host_key) else { return };
                if !h.control.as_ref().is_some_and(|c| c.same_connection(ctrl)) {
                    return;
                }
                if !install_view(&mut h.session_summaries, &mut h.sessions, view) {
                    if let Some(token) = retire_placement {
                        remove_placement_token(&mut h.pending_placements, &session_id, token);
                    }
                    return;
                }
                if let Some(token) = retire_placement {
                    remove_placement_token(&mut h.pending_placements, &session_id, token);
                }
                if let Some(placements) = h.pending_placements.get(&session_id)
                    && let Some(view) = h.sessions.get_mut(&session_id)
                {
                    apply_pending_placements(view, placements);
                }
                if let Some(target) = h.pending_focus.get(&session_id)
                    && let Some(view) = h.sessions.get_mut(&session_id)
                {
                    target.apply(view);
                }
            }
            let theme = st.active_theme.clone();
            let live_ids: HashSet<String> = blocks.iter().map(|b| b.block_id.clone()).collect();
            st.retain_blocks(|bk| !(bk.host_key == host_key && bk.session_id == session_id && !live_ids.contains(&bk.block_id)));
            st.bump_topology();
            for b in &blocks {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.clone(), block_id: b.block_id.clone() };
                let is_terminal = attaches_a_stream(b);
                if let Some(existing) = st.blocks.get(&bk) {
                    let mut g = existing.lock().unwrap_or_else(|e| e.into_inner());
                    if !b.label.is_empty() && g.title.is_empty() {
                        g.title = b.label.clone();
                    }
                    g.label.clone_from(&b.label);
                    g.is_terminal = is_terminal;
                    // (Whose its size is, as a view newer than the last
                    // word of it says: an owner change missed, while its
                    // stream was down, is mended by the next view.)
                    if seq != 0 {
                        g.owner_said(seq, b.owner.as_deref(), ctrl.client_id());
                    }
                    if !g.data_connected && is_terminal {
                        let ticket = pre_tickets.iter().find(|t| t.session_id == session_id && t.block_id == b.block_id).map(|t| t.ticket.clone());
                        to_attach.push((bk, existing.clone(), ticket));
                    }
                } else {
                    let mut br = BlockRuntime::new(120, 32, &theme);
                    br.predict.set_on(st.settings.type_ahead);
                    // (Whose size it is, as the session says, from the
                    // first: it was known only once its stream said so,
                    // and a pane whose stream hadn't come was sized as
                    // this client's, its host asked for a size it kept.)
                    br.owner_said(seq, b.owner.as_deref(), ctrl.client_id());
                    br.creator_name = b.creator_name.clone();
                    br.is_terminal = is_terminal;
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

        // (What the person sees first: its streams asked for, and the one
        // opened beforehand taken, before a session's thirty others.)
        to_attach.sort_by_key(|(bk, ..): &(BlockKey, _, _)| in_front.get(&bk.block_id).copied().unwrap_or(u8::MAX));
        for (bk, br, pre_ticket) in to_attach {
            self.spawn_block_data_stream(ctrl.clone(), ep.clone(), bk, br, pre_ticket);
        }
        self.write_palette_index();
        self.screens_service.refresh_owner_labels(Some(host_key));
        (self.notify)(HubNotice::StateChanged);
    }

    fn spawn_block_data_stream(self: &Arc<Self>, ctrl: ControlClient, ep: RexEndpoint, bk: BlockKey, br: Arc<Mutex<BlockRuntime>>, pre_ticket: Option<String>) {
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
        // (Whose its streams are, read as this connection is seen to be
        // the host's and the pane this one, together: what one leaves is
        // offered back to this host and this client only. And this task's
        // number: what a task of the pane's that ends after a later one
        // began leaves, or forgets, never touches the later one's.)
        let (owner, task) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get(&bk.host_key).filter(|h| h.control.as_ref().is_some_and(|c| c.same_connection(&ctrl))) else { return };
            if !st.blocks.get(&bk).is_some_and(|b| Arc::ptr_eq(b, &br)) {
                return;
            }
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if g.data_connected {
                return;
            }
            g.data_connected = true;
            g.input_ready = false;
            g.data_cancel = Some(tx);
            (StreamOwner { host_instance: host.host_instance_id, client: ctrl.client_id().map(str::to_owned) }, self.stream_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1)
        };
        let inner = self.clone();
        // (One opened beforehand for this connection, where the host is
        // being connected to.)
        let spare = self.take_spare(&bk.host_key, &ctrl);
        tokio::spawn(async move {
            let (mut pre_ticket, mut spare) = (pre_ticket, spare);
            // (Failures in a row, of having the stream or of its lasting.)
            let mut failures = 0u32;
            // A stream had, read until it ends, and had again while this
            // connection is the host's and the pane this one: a pane whose
            // stream couldn't be had, or ended, stayed blank until its
            // session next changed, which may be never (a lead's tab idle
            // while its agents work in theirs).
            'attach: loop {
                let got = tokio::select! {
                    biased;
                    _ = &mut rx => break 'attach,
                    _ = ctrl.disconnected() => break 'attach,
                    got = inner.attach_terminal(&ctrl, &ep, &bk, &owner, pre_ticket.take(), spare.take()) => got,
                };
                let mut stream = match got {
                    Ok(stream) => stream,
                    Err(failed) => {
                        failures += 1;
                        if !failed.again || failures >= attach_tries() {
                            tracing::warn!("Rex: no stream for {} ({}; {failures} tries): left until its tab is chosen or its session changes", bk.block_id, failed.why);
                            break 'attach;
                        }
                        tracing::debug!("Rex: no stream for {} yet ({}): tried again", bk.block_id, failed.why);
                        if !inner.attach_again_later(&ctrl, &bk, &br, &mut rx, failures).await {
                            break 'attach;
                        }
                        continue 'attach;
                    }
                };
                // (Had: nothing of an older one is the server's now.)
                inner.forget_stream_left_before(&bk, task);
                // (When its first screen came: a stream that has shown the
                // pane long enough is a new failure's first when it ends.)
                let mut shown_since = None::<Instant>;

                // Also fetch initial terminal metadata (ports, process, program_status) asynchronously.
                let c_meta = ctrl.clone();
                let bk_meta = bk.clone();
                let br_meta = br.clone();
                let notify_meta = inner.notify.clone();
                tokio::spawn(async move {
                    // (Its programs' status asked anew with the stream: what
                    // changes meanwhile is kept for after its answer, and
                    // only this request's answer replays it.)
                    let asked = br_meta.lock().unwrap_or_else(|e| e.into_inner()).begin_status();
                    // (Asked together: one round trip of the link for the
                    // three, where each waited for the one before.)
                    let (status, ports, process) = tokio::join!(
                        c_meta.terminal_program_status(&bk_meta.session_id, &bk_meta.block_id),
                        c_meta.terminal_ports(&bk_meta.session_id, &bk_meta.block_id),
                        c_meta.terminal_process(&bk_meta.session_id, &bk_meta.block_id)
                    );
                    {
                        let mut block = br_meta.lock().unwrap_or_else(|e| e.into_inner());
                        block.status_answered(asked, status.ok());
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
                // (Until its first screen is whole: given up after so long
                // with nothing, `FIRST_SCREEN_QUIET`; no clock after.)
                let mut first_screen_by = tokio::time::Instant::now() + first_screen_quiet();
                let ended = loop {
                    let frame = tokio::select! {
                        _ = &mut rx => {
                            // (Its end may not reach the server, whose way
                            // here may be gone with its host's connection:
                            // what it leaves is kept, and it isn't waited for.)
                            inner.leave_stream(&bk, &owner, task, &stream);
                            let _ = tokio::time::timeout(Duration::from_secs(2), stream.close_clean()).await;
                            break StreamEnd::LetGo;
                        }
                        res=stream.next_frame()=>res,
                        _ = tokio::time::sleep_until(first_screen_by), if attachment.is_some() => {
                            tracing::info!("Rex: {}'s first screen brought nothing for {:?}: had again", bk.block_id, first_screen_quiet());
                            inner.leave_stream(&bk, &owner, task, &stream);
                            let _ = tokio::time::timeout(Duration::from_secs(2), stream.close_clean()).await;
                            break StreamEnd::Lost;
                        }
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
                                // (What the old one leaves is kept before its end
                                // is asked: a network gone meanwhile leaves it
                                // the server's.)
                                inner.leave_stream(&bk, &owner, task, &stream);
                                let old_token = stream.welcome.reconnect_token;
                                // (No keys while the screen they'd be typed
                                // against is the old stream's, until the new
                                // one's comes: Astra R2 #5.)
                                br.lock().unwrap_or_else(|e| e.into_inner()).input_ready = false;
                                let anew = async {
                                    let (ticket, opened) = tokio::join!(terminal_ticket(&ctrl, &bk), open_terminal_stream(&ep));
                                    let (ticket, opened) = (ticket?, opened?);
                                    stream.close().await;
                                    let mut tried = opened.hello(&ticket).await;
                                    for wait in [200u64, 500, 1200] {
                                        if tried.is_ok() {
                                            break;
                                        }
                                        tokio::time::sleep(Duration::from_millis(wait)).await;
                                        let ticket = terminal_ticket(&ctrl, &bk).await?;
                                        let (raw, header) = ep.connect_stream().await?;
                                        tried = TerminalDataStream::connect_hello(raw, &header, &ticket).await;
                                    }
                                    tried
                                };
                                // (Not waited for once the pane is let go of, or its
                                // host's connection ends.)
                                let caught = tokio::select! {
                                    biased;
                                    _ = &mut rx => None,
                                    _ = ctrl.disconnected() => None,
                                    caught = tokio::time::timeout(Duration::from_secs(30), anew) => Some(caught),
                                };
                                let Some(caught) = caught else { break StreamEnd::LetGo };
                                match caught {
                                    Ok(Ok(fresh)) => {
                                        tracing::info!("Rex: {} went on pouring with its shell at its prompt ({} kB in {:.1} s): caught up with a new stream", bk.block_id, poured / 1024, pouring_since.map_or(0.0, |since| since.elapsed().as_secs_f64()));
                                        inner.forget_stream_left(&bk, &old_token);
                                        stream = fresh;
                                        attachment = Some(crate::attach::Attachment::default());
                                        // (Its first screen given its whole time from
                                        // its welcome: the old stream's had long run
                                        // out, and the new one was ended as it began,
                                        // its screen still on its way: Astra R2 #5,
                                        // Neon R2 #1.)
                                        first_screen_by = tokio::time::Instant::now() + first_screen_quiet();
                                        shown_since = None;
                                        apc = InlineApcDemuxer::new(stream.welcome.channel_nonce);
                                        // (Caught up twice within a short while: something goes on
                                        // writing there; looked at less often.)
                                        looks_apart = if caught_up_at.is_some_and(|at| at.elapsed() < looks_apart * 5) { (looks_apart * 2).min(CATCH_UP_LOOK_MOST) } else { CATCH_UP_LOOK };
                                        caught_up_at = Some(Instant::now());
                                        (pouring_since, poured, look_at) = (None, 0, None);
                                    }
                                    // (The old stream may be ended by now: the terminal is
                                    // attached again, as after any stream's end.)
                                    Ok(Err(e)) => {
                                        tracing::warn!("Rex: catching {} up failed: {e}", bk.block_id);
                                        break StreamEnd::Lost;
                                    }
                                    Err(_) => {
                                        tracing::warn!("Rex: catching {} up took too long", bk.block_id);
                                        break StreamEnd::Lost;
                                    }
                                }
                            }
                            continue;
                        }
                    };
                    let chunk = match frame {
                        Ok(TerminalStreamFrame::Bytes(chunk)) => chunk,
                        // (Ended by the server, its close said: nothing of it is
                        // held there. A stream that just stopped, with no close,
                        // is said as 1006, never sent: maybe still held.)
                        Ok(TerminalStreamFrame::Closed { code, reason }) => {
                            tracing::debug!("Rex: {}'s stream ended ({code} {reason})", bk.block_id);
                            if code == 1006 {
                                inner.leave_stream(&bk, &owner, task, &stream);
                            }
                            break StreamEnd::Lost;
                        }
                        Err(e) => {
                            tracing::debug!("Rex: {}'s stream was lost: {e}", bk.block_id);
                            inner.leave_stream(&bk, &owner, task, &stream);
                            break StreamEnd::Lost;
                        }
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    // (Its first screen coming: its time given again with
                    // each part of it, and never for a frame with nothing in
                    // it, of which one every so often kept a stream that
                    // brought no screen for ever: Astra R2 #7.)
                    if attachment.is_some() {
                        first_screen_by = tokio::time::Instant::now() + first_screen_quiet();
                    }
                    let (terminal, snapshot_size, live) = if let Some(attach) = &mut attachment {
                        match attach.push(&chunk) {
                            Ok(None) => continue,
                            Ok(Some(screen)) => {
                                attachment = None;
                                shown_since = Some(Instant::now());
                                (screen.terminal, screen.snapshot_size, screen.live)
                            }
                            Err(e) => {
                                tracing::warn!("Could not read the snapshot for {}: {e}", bk.block_id);
                                inner.leave_stream(&bk, &owner, task, &stream);
                                let _ = tokio::time::timeout(Duration::from_secs(2), stream.close_clean()).await;
                                break StreamEnd::Lost;
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
                        inner.leave_stream(&bk, &owner, task, &stream);
                        let _ = tokio::time::timeout(Duration::from_secs(2), stream.close_clean()).await;
                        break StreamEnd::Lost;
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
                };
                if matches!(ended, StreamEnd::LetGo) {
                    break 'attach;
                }
                // (No keys go to a pane with no stream: none is seen go in.)
                br.lock().unwrap_or_else(|e| e.into_inner()).input_ready = false;
                // A stream that showed the pane long enough is a new
                // failure's first; one that ended as it began, or never
                // showed it however long it lasted, one more in a row.
                failures = if shown_since.is_some_and(|at| at.elapsed() >= stream_lasted()) { 1 } else { failures + 1 };
                if failures >= attach_tries() {
                    tracing::warn!("Rex: {}'s stream ended again ({failures} times in a row): left until its tab is chosen or its session changes", bk.block_id);
                    break 'attach;
                }
                if !inner.attach_again_later(&ctrl, &bk, &br, &mut rx, failures).await {
                    break 'attach;
                }
            }
            br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
        });
    }

    /// A terminal's stream, had ([`Self::spawn_block_data_stream`]): it
    /// is opened while its ticket is asked for (or the one the sessions
    /// came with is taken), or one opened beforehand is taken, and the
    /// ticket goes in the hello, which is all that waits for it. One
    /// opened beforehand that the host has closed meanwhile is opened
    /// anew, once; and where the server still holds this terminal's
    /// last stream ("AlreadyConnected"), that one is taken over and
    /// ended first, where what it left is known ([`StreamLeft`]).
    async fn attach_terminal(
        self: &Arc<Self>,
        ctrl: &ControlClient,
        ep: &RexEndpoint,
        bk: &BlockKey,
        owner: &StreamOwner,
        pre_ticket: Option<String>,
        spare: Option<Spare>,
    ) -> Result<TerminalDataStream<olo_rex_client::RexStream>, AttachFailed> {
        let asked = async {
            match pre_ticket {
                Some(ticket) => Ok(ticket),
                None => terminal_ticket(ctrl, bk).await,
            }
        };
        let opened = async {
            if let Some(spare) = spare
                && let Ok(Ok(upgrade)) = spare.await
            {
                return Ok((upgrade, true));
            }
            open_terminal_stream(ep).await.map(|upgrade| (upgrade, false))
        };
        let (ticket, opened) = tokio::join!(asked, opened);
        let ticket = ticket.map_err(|e| AttachFailed::of("its ticket", e))?;
        let (upgrade, beforehand) = opened.map_err(|e| AttachFailed::of("its stream", e))?;
        let first = match upgrade.hello(&ticket).await {
            Ok(stream) => return Ok(stream),
            Err(e) => e,
        };
        if matches!(first, olo_rex_client::RexError::DataRejected(olo_rex_client::DataRejectReason::AlreadyConnected)) {
            // (Else tried again later: with what the old one left, if its
            // takeover failed on the way; or when the server may have
            // heard its end.)
            let Some(left) = self.stream_left_for(bk, owner) else { return Err(AttachFailed::of("its hello", first)) };
            match self.end_stream_left(ep, bk, &left).await {
                Taken::Ended => {}
                Taken::Refused | Taken::Unsure => return Err(AttachFailed::of("its hello", first)),
            }
        } else if beforehand {
            // (The ticket spent on it or not: both anew.)
            tracing::debug!("terminal stream {} opened beforehand failed ({first}): opened anew", bk.block_id);
        } else {
            return Err(AttachFailed::of("its hello", first));
        }
        let (ticket, opened) = tokio::join!(terminal_ticket(ctrl, bk), open_terminal_stream(ep));
        let ticket = ticket.map_err(|e| AttachFailed::of("its ticket", e))?;
        let upgrade = opened.map_err(|e| AttachFailed::of("its stream", e))?;
        upgrade.hello(&ticket).await.map_err(|e| AttachFailed::of("its hello", e))
    }

    /// The server's hold on this terminal's last stream ended
    /// ([`StreamLeft`]): that very stream taken over (the server's
    /// resume, with its token and count), closed, and its end waited for
    /// (a new stream asked for before the server had it was refused as
    /// the old one still). Ended only on the server's own word of it, a
    /// close answering ours; a stream that stops with none (1006, never
    /// sent), an error, a time-out: unsure, and what it left is kept,
    /// renewed with the resumed stream's own token, for the next try. A
    /// token the server refuses as such is forgotten.
    async fn end_stream_left(&self, ep: &RexEndpoint, bk: &BlockKey, left: &StreamLeft) -> Taken {
        use olo_rex_client::{DataRejectReason as Reject, RexError};
        let mut token = left.token;
        let taken = async {
            let (raw, header) = match ep.connect_stream().await {
                Ok(opened) => opened,
                Err(e) => return (Taken::Unsure, e.to_string()),
            };
            let mut held = match TerminalDataStream::connect_resume(raw, &header, &left.token, left.bytes).await {
                Ok(held) => held,
                Err(e @ RexError::DataRejected(Reject::InvalidOrExpiredTicket | Reject::StreamUnavailable | Reject::MalformedHandshake | Reject::WrongSubprotocol)) => {
                    return (Taken::Refused, e.to_string());
                }
                Err(e) => return (Taken::Unsure, e.to_string()),
            };
            // (Resumed: its own token is the one that takes it now.)
            token = held.welcome.reconnect_token;
            self.renew_stream_left(bk, &left.token, held.welcome.reconnect_token, held.bytes_received);
            held.close().await;
            loop {
                match held.next_frame().await {
                    Ok(TerminalStreamFrame::Bytes(_)) => {}
                    Ok(TerminalStreamFrame::Closed { code, .. }) if code != 1006 => return (Taken::Ended, String::new()),
                    Ok(TerminalStreamFrame::Closed { code, reason }) => return (Taken::Unsure, format!("{code} {reason}")),
                    Err(e) => return (Taken::Unsure, e.to_string()),
                }
            }
        };
        let (taken, why) = tokio::time::timeout(Duration::from_secs(10), taken).await.unwrap_or_else(|_| (Taken::Unsure, "took too long".into()));
        match taken {
            Taken::Ended => {
                tracing::info!("Rex: {}'s last stream, left {:.1} s ago, was still the server's: taken over and ended", bk.block_id, left.at.elapsed().as_secs_f64());
                self.forget_stream_left(bk, &token);
            }
            Taken::Refused => {
                tracing::info!("Rex: {}'s last stream can't be taken over ({why}): forgotten", bk.block_id);
                self.forget_stream_left(bk, &token);
            }
            Taken::Unsure => tracing::debug!("Rex: {}'s last stream wasn't taken over yet ({why}): tried again later", bk.block_id),
        }
        taken
    }

    /// What `stream` leaves to be taken over with, should its end not
    /// reach the server ([`StreamLeft`]), as `owner`'s, by the pane's task
    /// numbered `task`.
    fn leave_stream(&self, bk: &BlockKey, owner: &StreamOwner, task: u64, stream: &TerminalDataStream<olo_rex_client::RexStream>) {
        self.keep_stream_left(bk, StreamLeft { token: stream.welcome.reconnect_token, bytes: stream.bytes_received, at: Instant::now(), owner: owner.clone(), task });
    }

    /// `left` kept for its pane, unless a later task of the pane's has
    /// left its own already: that one stays, for the stream it is of may
    /// be the server's still, and this one's end is known by then.
    fn keep_stream_left(&self, bk: &BlockKey, left: StreamLeft) {
        let mut all = self.streams_left.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|_, l| l.at.elapsed() < STREAM_LEFT_KEPT);
        if all.get(bk).is_some_and(|kept| kept.task > left.task) {
            return;
        }
        all.insert(bk.clone(), left);
    }

    /// What the pane's last stream left, if it is `owner`'s and not too
    /// old: none of another host instance or client (never sent to
    /// another server), whose it stays, and none past its time, which is
    /// forgotten. (Looked up by another, it was forgotten, and its own,
    /// asking next, had nothing to take its stream over with.)
    fn stream_left_for(&self, bk: &BlockKey, owner: &StreamOwner) -> Option<StreamLeft> {
        let mut all = self.streams_left.lock().unwrap_or_else(|e| e.into_inner());
        let left = all.get(bk)?;
        if left.at.elapsed() >= STREAM_LEFT_KEPT {
            all.remove(bk);
            return None;
        }
        (&left.owner == owner).then(|| left.clone())
    }

    /// What the pane's tasks up to `task` left is forgotten: that task had
    /// a stream anew, which the server takes only where it holds none of
    /// the client's for the terminal. A later task's stays.
    fn forget_stream_left_before(&self, bk: &BlockKey, task: u64) {
        let mut all = self.streams_left.lock().unwrap_or_else(|e| e.into_inner());
        if all.get(bk).is_some_and(|kept| kept.task <= task) {
            all.remove(bk);
        }
    }

    /// What the pane's last stream left goes with the token `was` for
    /// the stream resumed with it (`now`, and as far as it had come).
    fn renew_stream_left(&self, bk: &BlockKey, was: &[u8; 16], now: [u8; 16], bytes: u64) {
        if let Some(left) = self.streams_left.lock().unwrap_or_else(|e| e.into_inner()).get_mut(bk).filter(|l| &l.token == was) {
            (left.token, left.bytes, left.at) = (now, bytes, Instant::now());
        }
    }

    /// What the stream of `token` left is forgotten: its end is known
    /// (only that one's record: a newer one stays).
    fn forget_stream_left(&self, bk: &BlockKey, token: &[u8; 16]) {
        let mut all = self.streams_left.lock().unwrap_or_else(|e| e.into_inner());
        if all.get(bk).is_some_and(|l| &l.token == token) {
            all.remove(bk);
        }
    }

    /// The sessions of `host_key` a window shows ([`RexHub::window_shows`]).
    fn sessions_shown(&self, host_key: &str) -> HashSet<String> {
        self.shown_sessions.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|(h, _)| h == host_key).map(|(_, s)| s.clone()).collect()
    }

    /// What a host's streams left is forgotten: the host is gone, or
    /// another under its name.
    fn forget_streams_of(&self, host_key: &str) {
        self.streams_left.lock().unwrap_or_else(|e| e.into_inner()).retain(|bk, _| bk.host_key != host_key);
    }

    /// Waits to attach a terminal again, its `failures`-th time in a
    /// row ([`attach_again_after`]): whether it still is to be once
    /// the wait is over. Never once the host's connection is another
    /// or none, or the pane another or let go of (`cancel`).
    async fn attach_again_later(&self, ctrl: &ControlClient, bk: &BlockKey, br: &Arc<Mutex<BlockRuntime>>, cancel: &mut tokio::sync::oneshot::Receiver<()>, failures: u32) -> bool {
        if !self.attach_still_wanted(ctrl, bk, br) {
            return false;
        }
        tokio::select! {
            biased;
            _ = &mut *cancel => false,
            _ = ctrl.disconnected() => false,
            _ = tokio::time::sleep(attach_again_after(failures, bk)) => self.attach_still_wanted(ctrl, bk, br),
        }
    }

    /// Asks a pane's host for the size its view wants (`first` on its
    /// way now), one request at a time, until the host has the last one
    /// wanted ([`SizeAsked`]); one not given is asked again a little
    /// later, while the host's connection is this one and the pane this
    /// one, as a stream is had again ([`attach_again_after`]).
    fn ask_size(self: &Arc<Self>, key: BlockKey, br: Arc<Mutex<BlockRuntime>>, first: TermSize) {
        let found = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&key.host_key).and_then(|h| h.control.clone());
        let Some(ctrl) = found else {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            g.size.answered(first, SizeAnswer::Unsure);
            g.size.asker = false;
            return;
        };
        let inner = self.clone();
        self.rt_handle.spawn(async move {
            let wake = br.lock().unwrap_or_else(|e| e.into_inner()).size_wake.clone();
            let mut asked = first;
            loop {
                let (cols, rows, cell_w, cell_h) = asked;
                let given = ctrl.terminal_resize(&key.session_id, &key.block_id, cols, rows, u16::try_from(cell_w).ok(), u16::try_from(cell_h).ok()).await;
                let answer = match &given {
                    Ok(_) => SizeAnswer::Given,
                    Err(e) if attach_again(e) => SizeAnswer::Unsure,
                    Err(_) => SizeAnswer::Refused,
                };
                if let Err(e) = &given {
                    tracing::debug!("Rex: {}'s host didn't take its size {cols}×{rows}: {e}", key.block_id);
                }
                let step = br.lock().unwrap_or_else(|e| e.into_inner()).size_answered(asked, answer);
                let wait = match step {
                    SizeStep::Done => return,
                    SizeStep::Ask(next) => {
                        asked = next;
                        continue;
                    }
                    SizeStep::Wait(failures) => attach_again_after(failures, &key),
                };
                let still = tokio::select! {
                    biased;
                    _ = ctrl.disconnected() => false,
                    _ = wake.notified() => inner.attach_still_wanted(&ctrl, &key, &br),
                    _ = tokio::time::sleep(wait) => inner.attach_still_wanted(&ctrl, &key, &br),
                };
                let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                match (still && !g.other_size_owner).then(|| g.size.next()).flatten() {
                    Some(next) => asked = next,
                    None => {
                        g.size.asker = false;
                        return;
                    }
                }
            }
        });
    }

    /// Whether a pane's stream is still this task's to have: the host's
    /// connection is `ctrl`'s, and the pane is `br`.
    fn attach_still_wanted(&self, ctrl: &ControlClient, bk: &BlockKey, br: &Arc<Mutex<BlockRuntime>>) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.hosts.get(&bk.host_key).is_some_and(|h| h.control.as_ref().is_some_and(|c| c.same_connection(ctrl))) && st.blocks.get(bk).is_some_and(|b| Arc::ptr_eq(b, br))
    }

    fn set_window_front_pane(&self, window: crate::ui::WinId, pane: Option<BlockKey>) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match pane {
            Some(bk) => {
                if let Some(held_list) = st.held_pane_notices.remove(&bk) {
                    for mut h in held_list {
                        if let Some(t) = h.timer.take() {
                            t.abort();
                        }
                    }
                }
                if let Some(br) = st.blocks.get(&bk) {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    if g.agent_activity == AgentActivity::Blocked {
                        g.delivered_blocked_gen = Some(g.blocked_gen);
                    }
                }
                st.visible_panes.insert(window, bk);
            }
            None => {
                st.visible_panes.remove(&window);
            }
        }
    }

    fn handle_inline_apc(self: &Arc<Self>, bk: &BlockKey, apc: InlineApc) {
        let (title, body) = match apc {
            InlineApc::ChildExited { exit_code } => ("Process exited".to_owned(), format!("Exit code {exit_code}")),
            InlineApc::Unknown(val) => {
                let Some(title) = val.get("title").and_then(Value::as_str) else { return };
                (title.to_owned(), val.get("body").and_then(Value::as_str).unwrap_or("").to_owned())
            }
            _ => return,
        };
        self.dispatch_pane_notice(bk, title, body, olo_kit::notify::NoteKind::Information, None, None);
    }

    /// Evaluates visibility and minded-subagent suppression once at notice
    /// dispatch. For a freshly created pane on a host with a live agents
    /// follower whose ownership row has not yet arrived, or on a host awaiting
    /// its initial follower snapshot, holds the candidate notice briefly until
    /// `agents_frame_for_gen` resolves it or `NEW_PANE_OWNERSHIP_GRACE` elapses.
    fn dispatch_pane_notice(self: &Arc<Self>, bk: &BlockKey, title: String, body: String, kind: olo_kit::notify::NoteKind, activity: Option<AgentActivity>, blocked_gen: Option<u64>) {
        let to_emit = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.is_pane_visible(bk) {
                if let Some(held_list) = st.held_pane_notices.remove(bk) {
                    for mut h in held_list {
                        if let Some(t) = h.timer.take() {
                            t.abort();
                        }
                    }
                }
                if activity == Some(AgentActivity::Blocked)
                    && let Some(br) = st.blocks.get(bk)
                {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    if g.agent_activity == AgentActivity::Blocked && blocked_gen.is_none_or(|bg| bg == g.blocked_gen) {
                        g.delivered_blocked_gen = Some(g.blocked_gen);
                    }
                }
                return;
            }
            let n = st.settings.notices;
            let suppress_active = !(n.minded_subagents && (n.blocked || n.done || n.failed));
            if suppress_active {
                if st.is_minded_subagent(bk) {
                    if let Some(held_list) = st.held_pane_notices.remove(bk) {
                        for mut h in held_list {
                            if let Some(t) = h.timer.take() {
                                t.abort();
                            }
                        }
                    }
                    // For `Blocked`, leave `delivered_blocked_gen` unset so
                    // `reconcile_locked` can deliver it live if the minder
                    // detaches while this approval is still open.
                    return;
                }
                let now = st.now();
                let wait = if let Some(&since) = st.agents_snapshot_pending.get(&bk.host_key) {
                    NEW_PANE_OWNERSHIP_GRACE.checked_sub(now.saturating_duration_since(since)).filter(|d| !d.is_zero())
                } else if st.agents_told.contains(&bk.host_key) && !st.has_subagent_record(bk) {
                    st.blocks
                        .get(bk)
                        .and_then(|br| {
                            let created = br.lock().unwrap_or_else(|e| e.into_inner()).created_at;
                            NEW_PANE_OWNERSHIP_GRACE.checked_sub(now.saturating_duration_since(created))
                        })
                        .filter(|d| !d.is_zero())
                } else {
                    None
                };
                if let Some(wait) = wait {
                    st.held_notice_seq = st.held_notice_seq.wrapping_add(1);
                    let seq = st.held_notice_seq;
                    let inner = Arc::clone(self);
                    let bk_owned = bk.clone();
                    let timer = self
                        .rt_handle
                        .spawn(async move {
                            tokio::time::sleep(wait).await;
                            inner.flush_held_pane_notice(&bk_owned, Some(seq));
                        })
                        .abort_handle();
                    let list = st.held_pane_notices.entry(bk.clone()).or_default();
                    if activity == Some(AgentActivity::Blocked) {
                        // Replace any earlier held `Blocked` without touching
                        // held inline APC or bridge notices.
                        list.retain_mut(|h| {
                            if h.activity == Some(AgentActivity::Blocked) {
                                if let Some(t) = h.timer.take() {
                                    t.abort();
                                }
                                false
                            } else {
                                true
                            }
                        });
                    } else if list.len() >= 32
                        && let Some(pos) = list.iter().position(|h| h.activity != Some(AgentActivity::Blocked))
                    {
                        let mut dropped = list.remove(pos);
                        if let Some(t) = dropped.timer.take() {
                            t.abort();
                        }
                    }
                    list.push(HeldPaneNotice { notice: PendingPaneNotice { title, body, kind }, activity, blocked_gen, seq, timer: Some(timer) });
                    return;
                }
            }
            let mut to_emit = Vec::new();
            if let Some(prev_held) = st.held_pane_notices.remove(bk) {
                for mut h in prev_held {
                    if let Some(t) = h.timer.take() {
                        t.abort();
                    }
                    if activity == Some(AgentActivity::Blocked) && h.activity == Some(AgentActivity::Blocked) {
                        continue;
                    }
                    if let Some(notice) = Self::finish_held_notice(&mut st, bk, h) {
                        to_emit.push(notice);
                    }
                }
            }
            let mut emit_current = true;
            if activity == Some(AgentActivity::Blocked)
                && let Some(br) = st.blocks.get(bk)
            {
                let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                if g.muted || g.agent_activity != AgentActivity::Blocked || blocked_gen.is_some_and(|bg| bg != g.blocked_gen) || g.delivered_blocked_gen == Some(g.blocked_gen) {
                    emit_current = false;
                } else {
                    g.delivered_blocked_gen = Some(g.blocked_gen);
                }
            }
            if emit_current && let Some(session) = st.hosts.get(&bk.host_key).and_then(|h| h.sessions.get(&bk.session_id)).map(|v| v.label.clone()) {
                to_emit.push(HubNotice::Attention { target: Target::Pane(bk.clone()), title: format!("{session} · {title}"), body, kind });
            }
            to_emit
        };
        for notice in to_emit {
            (self.notify)(notice);
        }
    }

    fn finish_held_notice(st: &mut HubState, bk: &BlockKey, held: HeldPaneNotice) -> Option<HubNotice> {
        if st.is_pane_visible(bk) {
            if held.activity == Some(AgentActivity::Blocked)
                && let Some(br) = st.blocks.get(bk)
            {
                let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                if g.agent_activity == AgentActivity::Blocked && held.blocked_gen.is_none_or(|bg| bg == g.blocked_gen) {
                    g.delivered_blocked_gen = Some(g.blocked_gen);
                }
            }
            return None;
        }
        if let Some(act) = held.activity {
            let rule_on = match act {
                AgentActivity::Blocked => st.settings.notices.blocked,
                AgentActivity::Error => st.settings.notices.failed,
                AgentActivity::Done => st.settings.notices.done,
                _ => false,
            };
            if !rule_on {
                return None;
            }
            let still_valid = st.blocks.get(bk).is_some_and(|br| {
                let g = br.lock().unwrap_or_else(|e| e.into_inner());
                if g.muted {
                    return false;
                }
                if act == AgentActivity::Blocked {
                    g.agent_activity == AgentActivity::Blocked
                        && g.blocked_record().is_some()
                        && held.blocked_gen.is_none_or(|bg| bg == g.blocked_gen)
                        && g.delivered_blocked_gen != Some(g.blocked_gen)
                } else {
                    true
                }
            });
            if !still_valid {
                return None;
            }
        }
        if !st.allows_pane_notice(bk) {
            return None;
        }
        let session = st.hosts.get(&bk.host_key).and_then(|h| h.sessions.get(&bk.session_id)).map(|v| v.label.clone())?;
        if held.activity == Some(AgentActivity::Blocked)
            && let Some(br) = st.blocks.get(bk)
        {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            g.delivered_blocked_gen = Some(g.blocked_gen);
        }
        Some(HubNotice::Attention { target: Target::Pane(bk.clone()), title: format!("{session} · {}", held.notice.title), body: held.notice.body, kind: held.notice.kind })
    }

    fn flush_held_pane_notice(&self, bk: &BlockKey, expected_seq: Option<u64>) {
        let to_emit = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut out = Vec::new();
            match expected_seq {
                Some(exp) => {
                    let removed = if let Some(list) = st.held_pane_notices.get_mut(bk) {
                        let item = list.iter().position(|h| h.seq == exp).map(|pos| list.remove(pos));
                        if list.is_empty() {
                            st.held_pane_notices.remove(bk);
                        }
                        item
                    } else {
                        None
                    };
                    if let Some(mut held) = removed {
                        if let Some(t) = held.timer.take() {
                            t.abort();
                        }
                        if let Some(notice) = Self::finish_held_notice(&mut st, bk, held) {
                            out.push(notice);
                        }
                    }
                }
                None => {
                    if let Some(list) = st.held_pane_notices.remove(bk) {
                        for mut held in list {
                            if let Some(t) = held.timer.take() {
                                t.abort();
                            }
                            if let Some(notice) = Self::finish_held_notice(&mut st, bk, held) {
                                out.push(notice);
                            }
                        }
                    }
                }
            }
            out
        };
        for notice in to_emit {
            (self.notify)(notice);
        }
    }

    /// Reconciles held notices and open `Blocked` approvals after a host's
    /// subagent ownership or live minder state changes, or its follower ends.
    fn reconcile_locked(st: &mut HubState, host_key: &str, was_minded: &HashSet<BlockKey>, follower_dropped: bool, snapshot_arrived: bool) -> Vec<HubNotice> {
        let mut to_emit = Vec::new();
        if !st.held_pane_notices.is_empty() {
            let now = st.now();
            let ready: Vec<BlockKey> = st
                .held_pane_notices
                .keys()
                .filter(|bk| {
                    bk.host_key == host_key
                        && (follower_dropped
                            || st.has_subagent_record(bk)
                            || (snapshot_arrived
                                && st.blocks.get(*bk).is_none_or(|br| now.saturating_duration_since(br.lock().unwrap_or_else(|e| e.into_inner()).created_at) >= NEW_PANE_OWNERSHIP_GRACE)))
                })
                .cloned()
                .collect();
            for bk in ready {
                let Some(list) = st.held_pane_notices.remove(&bk) else { continue };
                for mut held in list {
                    if let Some(t) = held.timer.take() {
                        t.abort();
                    }
                    if let Some(notice) = Self::finish_held_notice(st, &bk, held) {
                        to_emit.push(notice);
                    }
                }
            }
        }
        if st.settings.notices.blocked {
            for bk in was_minded {
                if bk.host_key != host_key || st.is_minded_subagent(bk) {
                    continue;
                }
                if st.is_pane_visible(bk) {
                    if let Some(br) = st.blocks.get(bk) {
                        let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                        if g.agent_activity == AgentActivity::Blocked {
                            g.delivered_blocked_gen = Some(g.blocked_gen);
                        }
                    }
                    continue;
                }
                let Some(session) = st.hosts.get(&bk.host_key).and_then(|h| h.sessions.get(&bk.session_id)).map(|v| v.label.clone()) else {
                    continue;
                };
                let Some(br) = st.blocks.get(bk).cloned() else { continue };
                let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                let Some((bgen, body)) = g.current_blocked_notice() else { continue };
                g.delivered_blocked_gen = Some(bgen);
                drop(g);
                to_emit.push(HubNotice::Attention { target: Target::Pane(bk.clone()), title: format!("{session} · Agent blocked"), body, kind: olo_kit::notify::NoteKind::Important });
            }
        }
        to_emit
    }

    /// Legacy bridge callers may only identify their host. An explicit pane belongs
    /// to its originating host and session, even if another pane now has focus.
    fn bridge_notice(self: &Arc<Self>, host_key: &str, session_id: Option<&str>, block_id: Option<&str>, payload: &Value) {
        let session_id = session_id.or_else(|| payload.get("session_id").and_then(Value::as_str));
        let block_id = block_id.or_else(|| payload.get("block_id").and_then(Value::as_str));
        let title = payload.get("title").and_then(Value::as_str).unwrap_or("Rex");
        let body = payload.get("body").and_then(Value::as_str).unwrap_or_default();
        let pane_or_notice = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(host) = st.hosts.get(host_key) else { return };
            match (session_id, block_id) {
                (sid, Some(bid)) => {
                    let session = match sid {
                        Some(id) => host.sessions.get(id).filter(|s| s.find_block(bid).is_some()),
                        None => host.sessions.values().find(|s| s.find_block(bid).is_some()),
                    };
                    let Some(s) = session else { return };
                    Err(BlockKey { host_key: host_key.into(), session_id: s.session_id.clone(), block_id: bid.into() })
                }
                (Some(sid), None) => {
                    let Some(s) = host.session_summaries.iter().find(|s| s.session_id == sid) else { return };
                    Ok(HubNotice::Attention {
                        target: Target::Session { host_key: host_key.into(), session_id: sid.into() },
                        title: format!("{} · {title}", s.label),
                        body: body.to_owned(),
                        kind: olo_kit::notify::NoteKind::Important,
                    })
                }
                (None, None) => {
                    Ok(HubNotice::Attention { target: Target::Host(host_key.into()), title: format!("{} · {title}", host.label), body: body.to_owned(), kind: olo_kit::notify::NoteKind::Important })
                }
            }
        };
        match pane_or_notice {
            Ok(notice) => (self.notify)(notice),
            Err(bk) => self.dispatch_pane_notice(&bk, title.to_owned(), body.to_owned(), olo_kit::notify::NoteKind::Important, None, None),
        }
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
            st.bump_topology();
        }
        for (view, seq) in snapshot.views {
            let session_id = view.session_id.clone();
            let ids: Vec<_> = view.windows.iter().flat_map(|w| w.all_blocks()).filter(|b| attaches_a_stream(b)).map(|b| b.block_id.clone()).collect();
            self.ingest_session(host_key, ctrl, ep, view, seq, &[]).await;
            for bid in ids {
                let sid = &session_id;
                let key = BlockKey { host_key: host_key.into(), session_id: sid.clone(), block_id: bid.clone() };
                let block = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&key).cloned();
                let Some(block) = block else { continue };
                let asked = block.lock().unwrap_or_else(|e| e.into_inner()).begin_status();
                let (status, ports, process, mode) =
                    tokio::try_join!(ctrl.terminal_program_status(sid, &bid), ctrl.terminal_ports(sid, &bid), ctrl.terminal_process(sid, &bid), ctrl.terminal_input_mode(sid, &bid))?;
                let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                b.status_answered(asked, Some(status));
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
                    st.bump_topology();
                }
                if let Ok(att) = ctrl.session_attach(&s.session_id).await {
                    self.ingest_session(host_key, ctrl, ep, att.payload.view, att.seq, &[]).await;
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
                        h.pending_placements.remove(&session_id);
                        h.pending_creates.remove(&session_id);
                        if let Some((_, _, notify)) = h.creating_defer.remove(&session_id) {
                            notify.notify_waiters();
                        }
                        h.session_summaries.retain(|s| s.session_id != session_id);
                    }
                    st.retain_blocks(|bk| !(bk.host_key == host_key && bk.session_id == session_id));
                    st.bump_topology();
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
                    block.lock().unwrap_or_else(|e| e.into_inner()).owner_said(seq, owner.as_deref(), ctrl.client_id());
                }
                (self.notify)(HubNotice::StateChanged);
            }
            ControlEvent::SessionViewChanged { session_id, .. } | ControlEvent::ActiveWindowChanged { session_id, .. } | ControlEvent::WindowCreated { session_id, .. } => {
                // If this desktop is awaiting its own `session.new_window` reply for
                // `session_id`, wait briefly for the reply to identify `new_wid` and
                // arm `PlacementGuard` before publishing the creation view (Astra #9).
                let defer = {
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.hosts.get(host_key).and_then(|h| h.creating_defer.get(&session_id)).map(|(_, deadline, notify)| (*deadline, notify.clone()))
                };
                if let Some((deadline, notify)) = defer {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    if !wait.is_zero() {
                        let _ = tokio::time::timeout(wait, notify.notified()).await;
                    }
                }
                if let Ok(v) = ctrl.session_view(&session_id).await {
                    self.ingest_session(host_key, ctrl, ep, v.payload, v.seq, &[]).await;
                }
            }
            ControlEvent::WindowClosed { session_id, .. }
            | ControlEvent::WindowLabelChanged { session_id, .. }
            | ControlEvent::SessionLabelChanged { session_id, .. }
            | ControlEvent::BlockCreated { session_id, .. }
            | ControlEvent::BlockClosed { session_id, .. } => {
                if let Ok(v) = ctrl.session_view(&session_id).await {
                    self.ingest_session(host_key, ctrl, ep, v.payload, v.seq, &[]).await;
                }
            }
            ControlEvent::EventDropped { session_id: Some(sid), block_id, block_event_name, .. } => {
                if let Ok(v) = ctrl.session_view(&sid).await {
                    self.ingest_session(host_key, ctrl, ep, v.payload, v.seq, &[]).await;
                }
                if let Some(bid) = block_id {
                    let bk = BlockKey { host_key: host_key.into(), session_id: sid.clone(), block_id: bid.clone() };
                    let block = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned();
                    let Some(block) = block else { return };
                    let what = block_event_name.as_deref();
                    if matches!(what, None | Some("program_status_changed" | "program_status_removed")) {
                        let asked = block.lock().unwrap_or_else(|e| e.into_inner()).begin_status();
                        let result = ctrl.terminal_program_status(&sid, &bid).await.ok();
                        block.lock().unwrap_or_else(|e| e.into_inner()).status_answered(asked, result);
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
                let is_status = matches!(name.as_str(), "program_status_changed" | "program_status_removed");
                let (br_opt, rules) = {
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    (st.blocks.get(&bk).cloned(), if is_status { st.settings.notices } else { crate::settings::Notices::default() })
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
                                    activity_note = apply_program_status(&mut g, seq, change, rules);
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
                                    g.marked_unread(None);
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some((activity, blocked_gen, title, body, kind)) = activity_note {
                        self.dispatch_pane_notice(&bk, title.to_owned(), body, kind, Some(activity), Some(blocked_gen));
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
                    match inner.handle_client_perform(&host_key, req.action.clone(), req.args.clone(), req.session_id.clone(), req.block_id.clone(), req.requester.principal.pid).await {
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

    async fn handle_client_perform(
        self: &Arc<Self>,
        host_key: &str,
        action: String,
        args: Option<Value>,
        session_id: Option<String>,
        block_id: Option<String>,
        requester_pid: Option<u32>,
    ) -> Option<Value> {
        // What a tab's child may ask of this computer (`olo agent start
        // --desktop none|screens=NAME,…|inherit`) is said before anything
        // else is looked at, for every action there is.
        if let Some(reason) = self.child_refusal(host_key, &action, args.as_ref(), session_id.as_deref(), block_id.as_deref(), requester_pid).await {
            return Some(json!({
                "ok": false,
                "stderr": format!("olo: {reason}\n"),
                "data": { "code": 69 }
            }));
        }

        if action.starts_with("olo.") {
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
                    let is_screen = cmd == "screen";
                    let screen_args = if is_screen { req.args.clone() } else { Vec::new() };
                    let owner_state = self.state.clone();
                    let owner_hk = host_key.to_owned();
                    let owner_sid = session_id.clone();
                    let owner_bid = block_id.clone();
                    return Some(
                        self.bridge_calls
                            .answer(async move {
                                // Snapshot the target desktop screen's current running generation
                                // BEFORE running `olo-screen` so a late completion can never
                                // claim or unlink a replacement session (Finding 29).
                                let pre_gen = if is_screen {
                                    if let Some(target_name) = crate::screens_sock::target_name_for_bridge_args(&screen_args) {
                                        tokio::task::spawn_blocking(move || crate::screens_sock::snapshot_desktop_screen_gen(&target_name)).await.ok().flatten()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                let resp = bridge::handle_bridge_request(&label, &policy, req, &forward, &|_| {}).await;
                                if is_screen && resp.ok {
                                    let stdout = resp.stdout.clone().unwrap_or_default();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        crate::screens_sock::record_bridge_screen_outcome(&owner_state, &owner_hk, owner_sid.as_deref(), owner_bid.as_deref(), &screen_args, &stdout, pre_gen);
                                    })
                                    .await;
                                }
                                resp
                            })
                            .await,
                    );
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
                        return Some(
                            json!({ "ok": false, "stderr": "olo: this host may not send files to the Olo desktop (Preferences ▸ Rex: Send files to Downloads on this computer)\n", "data": { "code": bridge::REFUSED } }),
                        );
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
                        return Some(
                            json!({ "ok": false, "stderr": "olo: this host may not send files to the Olo desktop (Preferences ▸ Rex: Send files to Downloads on this computer)\n", "data": { "code": bridge::REFUSED } }),
                        );
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
                        return Some(
                            json!({ "ok": false, "stderr": "olo: this host may not reach its servers from the Olo desktop (Preferences ▸ Rex: Reach its local servers)\n", "data": { "code": bridge::REFUSED } }),
                        );
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

    /// `olo.agent.*` asked through Rex (`rex do olo.agent.…`; an `olo` put
    /// on a host before its agents service was): passed to the host's
    /// agents service, whose answer is the action's. The children are
    /// the host's (`olo_rex_link::agents`): nothing of them is decided
    /// here, and `olo agent` there doesn't come this way at all.
    async fn handle_agent_action(self: &Arc<Self>, host_key: &str, action: &str, args: Option<Value>, session_id: Option<String>, block_id: Option<String>) -> Value {
        let op = action.strip_prefix("olo.agent.").unwrap_or(action).to_owned();
        let (inner, hk) = (self.clone(), host_key.to_owned());
        self.bridge_calls.answer(async move { inner.ask_agents(&hk, &op, args, session_id, block_id, None).await }).await
    }

    /// Capture the live `(host_instance_id, screens_conn_id)` pin for `host_key`
    /// when rendering a row or building a context menu (Astra #1). Returns `None`
    /// for missing hosts or disconnected remote hosts (`screens_conn_id == 0`) so
    /// a stale menu never acquires a replacement host's identity or `(0, 0)` wildcard.
    pub(crate) fn host_live_pin(&self, host_key: &str) -> Option<(u64, u64)> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        host_live_pin_in(&st.hosts, host_key)
    }

    /// Verify that `host_key` currently matches a live pinned `(host_instance_id, conn_id)`.
    /// Rejects zero `host_instance_id` and (for remote hosts) zero `conn_id` so a
    /// missing or disconnected host never becomes a wildcard (Astra #1).
    pub(crate) fn host_matches_pin(&self, host_key: &str, host_instance_id: u64, conn_id: u64) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        host_matches_pin_in(&st.hosts, host_key, host_instance_id, conn_id)
    }

    /// Asks a host's agents service one thing (`olo-rex-link agent
    /// request` there: locally, or over the host's SSH connection) and
    /// gives back its answer. `args`: the command's own, as `olo` sends
    /// them (`{"args":[…],"cwd":…}`) or structured.
    pub(crate) async fn ask_agents(&self, host_key: &str, op: &str, args: Option<Value>, session_id: Option<String>, block_id: Option<String>, within: Option<Duration>) -> bridge::BridgeResponse {
        self.ask_agents_pinned(host_key, None, op, args, session_id, block_id, within).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn ask_agents_pinned(
        &self,
        host_key: &str,
        pin: Option<(u64, u64)>,
        op: &str,
        args: Option<Value>,
        session_id: Option<String>,
        block_id: Option<String>,
        within: Option<Duration>,
    ) -> bridge::BridgeResponse {
        use olo_rex_link::agents::wire::{PROTO, Request, Response};
        let mut req = Request { proto: PROTO, op: op.to_owned(), session: session_id.unwrap_or_default(), block: block_id.unwrap_or_default(), ..Default::default() };
        if let Some(v) = args {
            req.cwd = v.get("cwd").and_then(Value::as_str).unwrap_or_default().to_owned();
            match v.get("args").and_then(Value::as_array) {
                Some(list) => req.args = list.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect(),
                // (No arguments at all; or the structured form.)
                None if v.as_object().is_some_and(|o| o.keys().all(|k| k == "cwd")) => {}
                None => req.obj = Some(v),
            }
        }
        // (As long as the thing asked may take: a watch waits up to an
        // hour, a stop saves a long scrollback.)
        let within = within.unwrap_or(match op {
            "watch" | "wait" => Duration::from_secs(3700),
            "stop" | "close" | "kill" => Duration::from_secs(300),
            _ => Duration::from_secs(60),
        });
        let Ok(line) = serde_json::to_string(&req) else { return bridge::BridgeResponse::err("olo agent: the request couldn't be written\n") };
        let answer = crate::activity_watch::run_host_link_within_pinned(&self.state, host_key, pin, &["agent", "request"], Some(&(line + "\n")), within).await;
        let Some(answer) = answer.and_then(|out| serde_json::from_str::<Response>(out.trim()).ok()) else {
            return bridge::BridgeResponse {
                ok: false,
                stdout: None,
                stderr: Some("olo agent: this host's agents service couldn't be asked (Olo's helper is put on a host over SSH, and runs it there)\n".into()),
                // (`unreached`: nothing came back from the service. What it
                // answers itself, a refusal too, never has this.)
                data: Some(json!({ "code": 69, "unreached": true })),
                carried: Vec::new(),
            };
        };
        let mut data = if answer.data.is_object() { answer.data } else { json!({}) };
        if !answer.ok {
            data["code"] = json!(if answer.code != 0 { answer.code } else { 1 });
        }
        bridge::BridgeResponse { ok: answer.ok, stdout: Some(answer.stdout).filter(|s| !s.is_empty()), stderr: Some(answer.stderr).filter(|s| !s.is_empty()), data: Some(data), carried: Vec::new() }
    }

    /// Why a tab's child may not ask `action` of this computer, where it
    /// may not. A pane known here as a child is answered at once, by
    /// what its host's service said of it. Any other is asked of the
    /// service, by the asking process itself where the server says which
    /// it is: its ancestors say whose tab it runs in, whatever pane it
    /// names, and a child started a moment ago is known there before it
    /// is said here. (One question for a process, kept a minute: a
    /// command that polls asks once.)
    async fn child_refusal(&self, host_key: &str, action: &str, args: Option<&Value>, session_id: Option<&str>, block_id: Option<&str>, requester_pid: Option<u32>) -> Option<String> {
        use crate::subagents::DesktopPolicy;
        // (What every child may ask needs no question.)
        if DesktopPolicy::None.check_action(action, args).is_ok() {
            return None;
        }
        let bid = block_id.map(str::trim).filter(|s| !s.is_empty());
        let key = (host_key.to_owned(), bid.unwrap_or_default().to_owned(), requester_pid.unwrap_or(0));
        let asked: Option<Option<DesktopPolicy>> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(child) = bid.and_then(|b| st.subagents.by_block_any(host_key, session_id, b)) {
                return child.desktop_policy.check_action(action, args).err();
            }
            drop(st);
            let mut said = POLICIES_SAID.lock().unwrap_or_else(|e| e.into_inner());
            said.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
            said.get(&key).map(|(_, policy)| policy.clone())
        };
        let policy = match asked {
            Some(policy) => policy,
            None => {
                let answer = self.ask_agents(host_key, "policy", Some(json!({ "pid": requester_pid })), session_id.map(str::to_owned), bid.map(str::to_owned), Some(Duration::from_secs(10))).await;
                // The service answered, and would not say: a no. (It was
                // taken for a service that couldn't be asked, and the
                // action allowed wherever the pane could be placed: a
                // minder's pane, known to the session's view a moment
                // before the service's word of it arrived, was refused
                // its policy by the service's own gate and then let
                // through here, with nothing of the desktop its to ask.
                // A code review, 2026-11-27.)
                if !answer.ok && !answer.data.as_ref().is_some_and(|d| d["unreached"].as_bool() == Some(true)) {
                    return Some(format!(
                        "action `{action}` refused: this host's agents service would not say what this pane may ask of the desktop ({})",
                        answer.stderr.as_deref().unwrap_or("no reason given").trim()
                    ));
                }
                if !answer.ok {
                    // The service couldn't be asked (a host with no
                    // helper has no children either; a helper that
                    // doesn't answer is a fault): by what is known here.
                    // A pane that can't be placed in a session with
                    // children that are held back is refused.
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let placed = bid.is_some_and(|b| {
                        st.hosts.get(host_key).is_some_and(|h| match session_id.filter(|s| !s.is_empty()) {
                            Some(sid) => h.sessions.get(sid).is_some_and(|v| v.find_block(b).is_some()),
                            None => h.sessions.values().any(|v| v.find_block(b).is_some()),
                        })
                    });
                    return (!placed && st.subagents.has_restricted_children(host_key, session_id))
                        .then(|| format!("action `{action}` refused: this pane can't be placed in a session whose child tabs are held back from the desktop"));
                }
                let policy = answer.data.as_ref().and_then(|d| d["desktop"].as_str()).and_then(|d| DesktopPolicy::parse(d).ok());
                POLICIES_SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), policy.clone()));
                policy
            }
        };
        policy.and_then(|p| p.check_action(action, args).err())
    }

    fn host_connected(&self, host_key: &str) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(host_key).is_some_and(|h| matches!(h.conn_state, HostConnState::Connected))
    }

    /// Aborts any active agents follower for `host_key`, bumps its per-host
    /// generation so superseded frames/teardowns are ignored, and clears
    /// `agents_told`, `agents_snapshot_pending`, and cached `agent_states`.
    fn invalidate_agents_follower(&self, host_key: &str, await_snapshot: bool) -> u64 {
        let (epoch, old_handle, to_emit) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let old_handle = self.agents_following.lock().unwrap_or_else(|e| e.into_inner()).remove(host_key).map(|(_, h)| h);
            let was_minded: HashSet<BlockKey> = st.blocks.keys().filter(|bk| bk.host_key == host_key && st.is_minded_subagent(bk)).cloned().collect();
            let entry = st.agents_follow_gen.entry(host_key.to_owned()).or_insert(0);
            *entry = entry.wrapping_add(1);
            let epoch = *entry;
            st.agents_told.remove(host_key);
            st.agent_states.retain(|(hk, _), _| hk != host_key);
            if await_snapshot {
                let now = st.now();
                st.agents_snapshot_pending.insert(host_key.to_owned(), now);
                (epoch, old_handle, Vec::new())
            } else {
                st.agents_snapshot_pending.remove(host_key);
                let to_emit = Self::reconcile_locked(&mut st, host_key, &was_minded, true, false);
                (epoch, old_handle, to_emit)
            }
        };
        if let Some(old) = old_handle {
            old.abort();
        }
        for notice in to_emit {
            (self.notify)(notice);
        }
        epoch
    }

    /// Follows a host's agents service for as long as the host is
    /// connected (`olo-rex-link agent follow` there): everything it
    /// knows at once, then each change, a line each. What it says is
    /// what this app shows of the tabs' children, so they are all there
    /// the moment a host connects, however long they ran with no desktop
    /// to see them. A service that goes away (its host's Rex server
    /// restarted, a new build taking over) is followed again.
    fn follow_agents(self: &Arc<Self>, host_key: &str) {
        let epoch = self.invalidate_agents_follower(host_key, true);
        let (inner, hk) = (self.clone(), host_key.to_owned());
        let task = tokio::spawn(async move {
            let mut wait = Duration::from_secs(1);
            while inner.host_connected(&hk) {
                inner.hand_over_kept_children(&hk).await;
                let said = {
                    let (inner, hk) = (inner.clone(), hk.clone());
                    crate::activity_watch::follow_host_link(&inner.state.clone(), &hk.clone(), &["agent", "follow"], move |line| inner.agents_frame_for_gen(&hk, epoch, line)).await
                };
                if !inner.agents_follower_ended(&hk, epoch) {
                    break;
                }
                if !inner.host_connected(&hk) {
                    break;
                }
                // (It ended: again in a moment; later each time where it
                // never said anything, as on a host that can't have the helper.)
                wait = if said { Duration::from_secs(1) } else { (wait * 2).min(Duration::from_secs(60)) };
                tokio::time::sleep(wait).await;
                if said {
                    let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    if st.agents_follow_gen.get(&hk).copied() == Some(epoch) {
                        let now = st.now();
                        st.agents_snapshot_pending.insert(hk.clone(), now);
                    }
                }
            }
        });
        let handle = task.abort_handle();
        let abort_handle = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.agents_follow_gen.get(host_key).copied() == Some(epoch) {
                self.agents_following.lock().unwrap_or_else(|e| e.into_inner()).insert(host_key.to_owned(), (epoch, handle)).map(|(_, old)| old)
            } else {
                Some(handle)
            }
        };
        if let Some(old) = abort_handle {
            old.abort();
        }
    }

    /// Called when a follower stream ends for `(host_key, epoch)`. Ignores
    /// superseded generations so an old follower can never strip a newer
    /// follower's `agents_told`. Returns `true` if `epoch` is still current.
    fn agents_follower_ended(&self, host_key: &str, epoch: u64) -> bool {
        let (current, to_emit) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.agents_follow_gen.get(host_key).copied() != Some(epoch) {
                return false;
            }
            let was_minded: HashSet<BlockKey> = st.blocks.keys().filter(|bk| bk.host_key == host_key && st.is_minded_subagent(bk)).cloned().collect();
            st.agents_told.remove(host_key);
            st.agents_snapshot_pending.remove(host_key);
            let to_emit = Self::reconcile_locked(&mut st, host_key, &was_minded, true, false);
            (true, to_emit)
        };
        for notice in to_emit {
            (self.notify)(notice);
        }
        current
    }

    /// One thing a host's agents service said (`olo_rex_link::agents::wire::Frame`),
    /// ignored if `epoch` has been superseded by a newer follower.
    fn agents_frame_for_gen(&self, host_key: &str, epoch: u64, line: &str) {
        use olo_rex_link::agents::wire::Frame;
        let Ok(frame) = serde_json::from_str::<Frame>(line) else { return };
        let to_emit = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.agents_follow_gen.get(host_key).copied() != Some(epoch) {
                return;
            }
            let was_minded: HashSet<BlockKey> = st.blocks.keys().filter(|bk| bk.host_key == host_key && st.is_minded_subagent(bk)).cloned().collect();
            let snapshot_arrived = matches!(frame, Frame::Snapshot { .. });
            match frame {
                Frame::Snapshot { agents, .. } => {
                    st.subagents.entries.retain(|e| e.host_key != host_key);
                    st.agent_states.retain(|(hk, _), _| hk != host_key);
                    for a in agents {
                        st.agent_states.insert((host_key.to_owned(), a.entry.id.clone()), a.state);
                        st.subagents.entries.push(crate::subagents::NativeSubagent { host_key: host_key.to_owned(), ..a.entry });
                    }
                    st.agents_snapshot_pending.remove(host_key);
                    st.agents_told.insert(host_key.to_owned());
                }
                Frame::Agent { agent } => {
                    let state = agent.state;
                    let entry = crate::subagents::NativeSubagent { host_key: host_key.to_owned(), ..agent.entry };
                    st.agent_states.insert((host_key.to_owned(), entry.id.clone()), state);
                    match st.subagents.entries.iter_mut().find(|e| e.host_key == host_key && e.id == entry.id) {
                        Some(old) => *old = entry,
                        None => st.subagents.entries.push(entry),
                    }
                }
                Frame::Gone { id } => {
                    st.subagents.entries.retain(|e| !(e.host_key == host_key && e.id == id));
                    st.agent_states.remove(&(host_key.to_owned(), id));
                }
            }
            st.bump_topology();
            Self::reconcile_locked(&mut st, host_key, &was_minded, false, snapshot_arrived)
        };
        for notice in to_emit {
            (self.notify)(notice);
        }
        self.screens_service.refresh_owner_labels(Some(host_key));
        (self.notify)(HubNotice::StateChanged);
    }

    /// The children this app kept for a host when it was their keeper
    /// (Olo before 2026-11-26, in `rex-subagents.json`): handed to the
    /// host's service once, which takes those whose tabs are still
    /// there as live and the rest as exited. Then they are the host's.
    async fn hand_over_kept_children(&self, host_key: &str) {
        let path = crate::subagents::kept_before();
        let Ok(raw) = std::fs::read_to_string(&path) else { return };
        let Ok(mut kept) = serde_json::from_str::<crate::subagents::SubagentRegistry>(&raw) else {
            let _ = std::fs::remove_file(&path);
            return;
        };
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut kept.entries).into_iter().partition(|e| e.host_key == host_key);
        if mine.is_empty() {
            return;
        }
        let payload = format!("{}\n", json!({ "agents": mine }));
        let taken = crate::activity_watch::run_host_link(&self.state, host_key, &["agent", "adopt"], Some(&payload)).await.is_some_and(|out| out.contains("Took over"));
        if !taken {
            return;
        }
        if rest.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            kept.entries = rest;
            if let Err(e) = kept.save_to(&path) {
                eprintln!("rex: couldn't rewrite legacy subagents at {}: {e}", path.display());
            }
        }
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
                HostInputOp::Own { .. } => "own".to_owned(),
            });
        }
        assert_eq!(out, ["a:echo", "key", "a: x", "b:ls", "a:\r"]);
        assert!(typed_together(&mut waiting).is_none());
    }

    /// A pane unread because its agent asked leave is unread no longer
    /// once it stops asking; one that failed, or whose bell rang, stays
    /// unread until it is seen.
    #[test]
    fn a_pane_unread_for_a_question_is_read_once_it_is_answered() {
        use AgentActivity::{Blocked, Error, Idle, Working};
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        // Asked, and answered by somebody else: nothing left to see.
        b.marked_unread(Some(Blocked));
        assert!(b.unread && b.unread_cleared_at.is_none());
        b.activity_went(Blocked, Working);
        assert!(!b.unread && b.unread_cleared_at.is_some(), "its question answered, the pane is read");
        // Still asking: still unread (a change that isn't an answer changes nothing).
        b.marked_unread(Some(Blocked));
        b.activity_went(Working, Blocked);
        assert!(b.unread);
        // It failed: news until seen, whatever it does next.
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        b.marked_unread(Some(Error));
        b.activity_went(Error, Idle);
        assert!(b.unread, "a failure is unread until it is seen");
        // Asked, and its bell rang besides: the bell is still to be seen.
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        b.marked_unread(Some(Blocked));
        b.marked_unread(None);
        b.activity_went(Blocked, Working);
        assert!(b.unread, "a bell is unread until it is seen");
        // Failed first, then asked: the failure is still to be seen.
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        b.marked_unread(Some(Error));
        b.marked_unread(Some(Blocked));
        b.activity_went(Blocked, Working);
        assert!(b.unread);
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

    /// Open in Tracker opens where the pane's program works now, else
    /// where its agent was started, else home: on this computer as a
    /// folder, on a host as `rex://HOST/PATH`.
    #[test]
    fn open_in_tracker_goes_to_the_agents_own_folder() {
        let live = |dirs: &[&str]| dirs.iter().map(|d| (*d).to_owned()).collect::<Vec<_>>();
        let here = |p: &str| TrackerPlace::Here(PathBuf::from(p));
        // Where it works now first; a folder gone (" (deleted)") isn't one.
        assert_eq!(tracker_place(true, "local", &live(&["/home/you/worktrees/agent-a"]), Some("/home/you/src/olo"), Some("/home/you")), here("/home/you/worktrees/agent-a"));
        assert_eq!(tracker_place(true, "local", &live(&["/var/tmp/x (deleted)", "/home/you/worktrees/b"]), None, Some("/home/you")), here("/home/you/worktrees/b"));
        // Then where its agent was started: an exited child, or one whose host hasn't said yet.
        assert_eq!(tracker_place(true, "local", &live(&[]), Some("/home/you/worktrees/agent-a"), Some("/home/you")), here("/home/you/worktrees/agent-a"));
        assert_eq!(tracker_place(true, "local", &live(&[""]), Some("~/worktrees/b"), Some("/home/you")), here("/home/you/worktrees/b"));
        // Then home.
        assert_eq!(tracker_place(true, "local", &live(&[]), None, Some("/home/you")), here("/home/you"));
        assert_eq!(tracker_place(true, "local", &live(&[]), Some(""), None), here("/"));
        // A host's: its own path, as Tracker reaches it, spaces and all.
        assert_eq!(tracker_place(false, "dev-box", &live(&["/srv/work tree"]), Some("/srv/other"), None), TrackerPlace::There("rex://dev-box/srv/work%20tree".into()));
        assert_eq!(tracker_place(false, "dev-box", &live(&[]), Some("/srv/worktrees/agent-a"), None), TrackerPlace::There("rex://dev-box/srv/worktrees/agent-a".into()));
        assert_eq!(tracker_place(false, "Dev Box", &live(&[]), None, Some("/home/you")), TrackerPlace::There("rex://Dev%20Box/~".into()));
    }

    /// A connected host reached over SSH, as Tracker reaches it: the
    /// tests' own hosts (`HostRuntime::test_connected`) have no endpoint.
    fn host_over_ssh(key: &str, label: &str, instance: u64, conn: u64) -> HostRuntime {
        let mut host = HostRuntime::test_connected(key, HashMap::new());
        host.label = label.into();
        (host.is_local, host.host_instance_id, host.screens_conn_id) = (false, instance, conn);
        host.endpoint = Some(RexEndpoint::parse(&format!("ssh://you@{key}"), Arc::new(SshSessionPool::default()), None).unwrap());
        // (This build's helper there, which serves its files.)
        (host.helper, host.helper_current) = (HostHelper::Used, Some(true));
        host
    }

    fn child_of(host_key: &str, block_id: &str, cwd: &str) -> crate::subagents::NativeSubagent {
        crate::subagents::NativeSubagent {
            id: "sa-000001".into(),
            name: None,
            host_key: host_key.into(),
            session_id: "s1".into(),
            window_id: "w-child".into(),
            block_id: block_id.into(),
            parent_window_id: "w-lead".into(),
            parent_block_id: Some("b-lead".into()),
            cli: crate::subagents::AgentCli::Claude,
            model: String::new(),
            effort: String::new(),
            title: "child".into(),
            role: None,
            cwd: Some(cwd.into()),
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: 0,
            exited_at: Some(1),
            exit_code: Some(0),
            last_pid: None,
            created_rev: 0,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: None,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
        }
    }

    /// The hub's answer for a pane: its live folder, else its child's
    /// registry folder, else home; and none for a host replaced or
    /// reconnected since the menu was made, a menu that had no pin (it
    /// failed closed: Neon R1 #6), or a host Tracker can't reach (not
    /// over SSH; Olo's helper off there: Astra R1 #16).
    #[test]
    fn open_in_tracker_is_for_the_host_the_menu_was_made_for() {
        let hub = RexHub::test_offline(|_| {});
        let key = |host: &str, block: &str| BlockKey { host_key: host.into(), session_id: "s1".into(), block_id: block.into() };
        {
            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut local = HostRuntime::test_connected("local", HashMap::new());
            (local.is_local, local.host_instance_id, local.screens_conn_id) = (true, 11, 0);
            st.hosts.insert("local".into(), local);
            st.hosts.insert("dev-box".into(), host_over_ssh("dev-box", "dev-box", 22, 7));
            let mut web = HostRuntime::test_connected("web-box", HashMap::new());
            (web.is_local, web.host_instance_id, web.screens_conn_id) = (false, 33, 0);
            web.endpoint = Some(RexEndpoint::parse("https://web-box", Arc::new(SshSessionPool::default()), None).unwrap());
            st.hosts.insert("web-box".into(), web);
            let theme = TerminalTheme::default();
            let mut lead = BlockRuntime::new(80, 24, &theme);
            lead.pwd = "/srv/lead".into();
            st.blocks.insert(key("dev-box", "b-lead"), Arc::new(Mutex::new(lead)));
            st.blocks.insert(key("dev-box", "b-child"), Arc::new(Mutex::new(BlockRuntime::new(80, 24, &theme))));
            st.subagents.register(child_of("dev-box", "b-child", "/srv/worktrees/child"));
        }
        let pin = hub.tracker_pin("dev-box");
        assert_eq!(pin, Some((22, 7)));
        let made_for = HostPin::MustMatch(pin);
        assert_eq!(hub.tracker_place(&key("dev-box", "b-lead"), made_for), Some(TrackerPlace::There("rex://dev-box/srv/lead".into())), "the lead's live folder");
        assert_eq!(hub.tracker_place(&key("dev-box", "b-child"), made_for), Some(TrackerPlace::There("rex://dev-box/srv/worktrees/child".into())), "an exited child's worktree");
        assert_eq!(hub.tracker_place(&key("dev-box", "b-none"), made_for), Some(TrackerPlace::There("rex://dev-box/~".into())), "home");
        assert!(matches!(hub.tracker_place(&key("local", "b-none"), HostPin::MustMatch(hub.tracker_pin("local"))), Some(TrackerPlace::Here(_))));
        // A menu that had no pin opens nothing: never as if unpinned.
        assert_eq!(hub.tracker_place(&key("dev-box", "b-lead"), HostPin::MustMatch(None)), None);
        // A host reached otherwise than over SSH: Tracker can't, so no pin and nothing.
        assert_eq!(hub.tracker_pin("web-box"), None);
        assert_eq!(hub.tracker_place(&key("web-box", "b-none"), HostPin::Any), None);
        // Olo's helper turned off for the host: the same.
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).settings.helper_hosts.insert("dev-box".into(), "off".into());
        assert_eq!(hub.tracker_pin("dev-box"), None);
        assert_eq!(hub.tracker_place(&key("dev-box", "b-lead"), made_for), None);
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).settings.helper_hosts.remove("dev-box");
        // The host connected again (another connection) or replaced since the menu: nothing.
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut("dev-box").unwrap().screens_conn_id = 8;
        assert_eq!(hub.tracker_place(&key("dev-box", "b-lead"), made_for), None);
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get_mut("dev-box").unwrap().screens_conn_id = 0;
        assert_eq!(hub.tracker_pin("dev-box"), None, "disconnected: the menu's item is off");
    }

    /// A host is named in Tracker's URI, and in the files service's
    /// answers (by which Tracker names the folder, and asks, from then
    /// on), by its label where that names it alone; else by its key,
    /// tagged so (`id:KEY`): two hosts of one label went to whichever the
    /// files service found first (Astra R1 #3), and one labelled "Local"
    /// to this computer's folders. The answers named it by its label
    /// all the same (Astra R2 #1, Neon R2 #2). An untagged name is looked
    /// up as it always was, its label first: a bookmark made before goes
    /// where it went (Astra R2 #2: a key looked up first sent one of a
    /// host whose label is another's key to that other).
    #[test]
    fn tracker_names_each_host_unambiguously() {
        let mut hosts = HashMap::new();
        for (key, label) in [("h-1", "Build"), ("h-2", "Build"), ("h-3", "Local"), ("h-4", "Alpha"), ("h-5", "h-4"), ("h-6", "build")] {
            hosts.insert(key.to_owned(), host_over_ssh(key, label, 1, 1));
        }
        let named = |key: &str| tracker_authority(&hosts, &hosts[key]);
        assert_eq!(named("h-1"), "id:h-1", "a label another host has too");
        assert_eq!(named("h-2"), "id:h-2");
        assert_eq!(named("h-3"), "id:h-3", "this computer's reserved name");
        assert_eq!(named("h-4"), "Alpha", "a label of its own");
        assert_eq!(named("h-5"), "h-4", "its label, though another's key: a label is looked up first");
        assert_eq!(named("h-6"), "build", "another's label in another case: its own exactly");
        let names: Vec<(String, String)> = ["h-1", "h-2", "h-3", "h-4", "h-5", "h-6"].iter().map(|k| ((*k).to_owned(), named(k))).collect();
        assert!(!olo_kit_base::tracker::RexPath::parse(&format!("rex://{}/srv", named("h-3"))).unwrap().is_local(), "not this computer's");
        let hub = RexHub::test_offline(|_| {});
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts = hosts;
        // (Each opened, answered, and asked next by the answer's name: the
        // same host, each time.)
        for (key, name) in &names {
            assert_eq!(crate::files_sock::answers_name_for_test(&hub.state, name).as_deref(), Ok(name.as_str()), "the answers name {key} as it was named");
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(crate::files_sock::find_host_key_for_test(&st, name).as_deref(), Some(key.as_str()), "{name} is {key}");
        }
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(crate::files_sock::find_host_key_for_test(&st, "h-4").as_deref(), Some("h-5"), "a bookmark of h-5 made before: still h-5");
        assert_eq!(crate::files_sock::find_host_key_for_test(&st, "id:h-4").as_deref(), Some("h-4"));
        assert_eq!(crate::files_sock::find_host_key_for_test(&st, "Alpha").as_deref(), Some("h-4"));
        assert_eq!(crate::files_sock::find_host_key_for_test(&st, "h-1").as_deref(), Some("h-1"), "a key no label has, untagged: as before");
        assert_eq!(crate::files_sock::find_host_key_for_test(&st, "id:nobody"), None, "a key no host has");
    }

    /// Open in Tracker is offered where the host's files can be served:
    /// this build's helper there, or one this build carries that keeps
    /// files, to put there (Astra R2 #18: offered where the files service
    /// then refused every request).
    #[test]
    fn tracker_is_offered_where_files_can_be_served() {
        use crate::files_sock::files_served_for_test as served;
        assert!(served(&HostHelper::Used, Some(true), false), "this build's helper there");
        assert!(served(&HostHelper::None, None, true), "one to put there");
        assert!(!served(&HostHelper::None, None, false), "neither");
        assert!(!served(&HostHelper::Ready, Some(false), false), "another build's, and none to put there");
        let hub = RexHub::test_offline(|_| {});
        let mut host = host_over_ssh("h-1", "Build", 1, 1);
        host.helper_current = Some(false);
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.insert("h-1".into(), host);
        let provides = crate::link::carried() && crate::link::syncs();
        assert_eq!(hub.tracker_pin("h-1").is_some(), provides, "this build's own word decides, where the helper there isn't current");
    }

    /// A tab of two panes side by side whose sizes another client holds,
    /// on a test host whose requests land on `in_tx`'s other end.
    fn a_tab_held_by_another(zoomed: Option<&str>) -> (Arc<RexHub>, tokio::sync::mpsc::UnboundedReceiver<HostInputOp>) {
        a_tab_with_a_float_held_by_another(zoomed, None)
    }

    /// [`a_tab_held_by_another`], with a third pane floating over the
    /// two (`float`: its layer's state, "normal" or "minimized"), its
    /// size another client's too.
    fn a_tab_with_a_float_held_by_another(zoomed: Option<&str>, float: Option<&str>) -> (Arc<RexHub>, tokio::sync::mpsc::UnboundedReceiver<HostInputOp>) {
        let hub = RexHub::test_offline(|_| {});
        let pane = |id: &str| BlockInfo {
            block_id: id.into(),
            label: "shell".into(),
            creator_name: "com.superlogical.terminal".into(),
            flavor: "com.superlogical.terminal.shell".into(),
            host_id: String::new(),
            owner: Some("other".into()),
        };
        let leaf = |id: &str| olo_rex_client::LayoutNode { rect: [0.0; 4], block_id: Some(id.into()), split: None };
        let layout = olo_rex_client::LayoutNode {
            rect: [0.0, 0.0, 1.0, 1.0],
            block_id: None,
            split: Some(olo_rex_client::SplitNode {
                split_id: "sp1".into(),
                direction: olo_rex_client::SplitDirection::Horizontal,
                ratio: 0.5,
                before: Box::new(leaf("b-1")),
                after: Box::new(leaf("b-2")),
            }),
        };
        let mut layers =
            vec![olo_rex_client::LayerView { layer_id: "l1".into(), kind: "tiled".into(), bounds: [0.0, 0.0, 1.0, 1.0], state: None, layout: Some(layout), blocks: vec![pane("b-1"), pane("b-2")] }];
        if let Some(state) = float {
            layers.push(olo_rex_client::LayerView { layer_id: "l2".into(), kind: "floating".into(), bounds: [0.1, 0.1, 0.5, 0.5], state: Some(state.into()), layout: None, blocks: vec![pane("b-3")] });
        }
        let window = olo_rex_client::WindowView { window_id: "w1".into(), label: "w1".into(), focused_block_id: Some("b-1".into()), zoomed_block_id: zoomed.map(str::to_owned), layers };
        let sess = SessionView { session_id: "s1".into(), label: "work".into(), revision: 1, active_window_id: Some("w1".into()), windows: vec![window] };
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<HostInputOp>();
        {
            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut host = HostRuntime::test_connected("dev-box", HashMap::from([("s1".into(), sess)]));
            host.input_tx = Some(in_tx);
            st.hosts.insert("dev-box".into(), host);
            for id in ["b-1", "b-2", "b-3"] {
                let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
                b.other_size_owner = true;
                st.blocks.insert(BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: id.into() }, Arc::new(Mutex::new(b)));
            }
        }
        (hub, in_rx)
    }

    /// The panes claimed (`HostInputOp::Own`) as asked, none if nothing was.
    fn claimed(asked: &mut tokio::sync::mpsc::UnboundedReceiver<HostInputOp>) -> Vec<String> {
        match asked.try_recv() {
            Ok(HostInputOp::Own { block_ids, .. }) => block_ids,
            _ => Vec::new(),
        }
    }

    /// Choosing a tab, as bringing its window forward, takes the panes it
    /// shows, never one it hides (Astra R2 #13, Neon R2 #7): a zoomed
    /// tab's other splits, chosen, were taken from the desktop that shows
    /// them; a floating pane put away (only its button shows) was taken
    /// as its window came forward. One brought back by the choice is
    /// taken with them.
    #[test]
    fn a_tab_chosen_claims_only_the_panes_it_shows() {
        let (hub, mut asked) = a_tab_held_by_another(Some("b-2"));
        hub.select("dev-box", "s1", "w1", None);
        assert_eq!(claimed(&mut asked), ["b-2"], "a zoomed tab chosen: its zoomed pane alone");
        let (hub, mut asked) = a_tab_with_a_float_held_by_another(None, Some("minimized"));
        assert!(hub.take_tab_in_front("dev-box", "s1"));
        assert_eq!(claimed(&mut asked), ["b-1", "b-2"], "brought forward: not the float put away");
        let (hub, mut asked) = a_tab_with_a_float_held_by_another(None, Some("normal"));
        hub.select("dev-box", "s1", "w1", None);
        assert_eq!(claimed(&mut asked), ["b-1", "b-2", "b-3"], "a float shown is shown");
        let (hub, mut asked) = a_tab_with_a_float_held_by_another(None, Some("minimized"));
        hub.select("dev-box", "s1", "", Some("b-3"));
        assert_eq!(claimed(&mut asked), ["b-1", "b-2", "b-3"], "a float brought back by the choice is taken with them");
    }

    /// A tab taken as its window is dragged is asked for once, however
    /// many steps the drag has before the host answers (Astra R1 #7,
    /// Neon R1 #2: each step queued its own claim, a round trip each,
    /// ahead of what was typed next); asked again once answered and
    /// still another's.
    #[test]
    fn a_tab_is_claimed_once_however_often_asked() {
        let (hub, mut asked) = a_tab_held_by_another(None);
        for _ in 0..20 {
            hub.take_tab_in_front("dev-box", "s1");
        }
        match asked.try_recv() {
            Ok(HostInputOp::Own { block_ids, .. }) => assert_eq!(block_ids, ["b-1", "b-2"]),
            _ => panic!("one claim of both panes"),
        }
        assert!(asked.try_recv().is_err(), "and no other");
        // (Answered, the pane still another's: asked anew.)
        for block in hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.values() {
            block.lock().unwrap_or_else(|e| e.into_inner()).owning = false;
        }
        assert!(hub.take_tab_in_front("dev-box", "s1"));
        assert!(matches!(asked.try_recv(), Ok(HostInputOp::Own { .. })));
    }

    /// A tab with a pane zoomed shows that pane alone: only it is taken
    /// as the window comes forward (Astra R1 #6).
    #[test]
    fn a_zoomed_tab_claims_the_pane_it_shows() {
        let (hub, mut asked) = a_tab_held_by_another(Some("b-2"));
        assert!(hub.take_tab_in_front("dev-box", "s1"));
        match asked.try_recv() {
            Ok(HostInputOp::Own { block_ids, .. }) => assert_eq!(block_ids, ["b-2"]),
            _ => panic!("a claim of the zoomed pane"),
        }
    }

    /// Whose a pane's size is, from an older word than the last had, is
    /// not taken (Astra R1 #15): a view fetched before an owner change
    /// can't undo it; a newer view mends one missed.
    #[test]
    fn an_older_word_of_whose_a_pane_is_is_not_taken() {
        let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
        b.owner_said(10, Some("other"), Some("me"));
        assert!(b.other_size_owner);
        b.owner_said(8, None, Some("me"));
        assert!(b.other_size_owner, "an older view's word isn't taken");
        b.owner_said(12, Some("me"), Some("me"));
        assert!(!b.other_size_owner, "a newer one's is");
        b.owner_said(0, Some("other"), Some("me"));
        assert!(b.other_size_owner, "a word of no number (a pane's first) is taken");
    }

    /// A size asked while the pane became another client's says nothing
    /// of the host's size: the answer (`{}`, the size kept as a wish)
    /// isn't taken as given, and the asker stops (Neon R1 #3).
    #[test]
    fn a_size_answered_once_the_pane_is_anothers_is_not_taken_as_had() {
        let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
        let wide = (160, 40, 8, 16);
        b.size.want(wide);
        let asked = b.size.next().unwrap();
        b.size.asker = true;
        b.owner_said(5, Some("other"), Some("me"));
        assert_eq!(b.size_answered(asked, SizeAnswer::Given), SizeStep::Done);
        assert_eq!(b.size.host, None, "not had: kept as a wish there");
        assert!(!b.size.asker && b.size.asking.is_none());
        // (Its own again, the size is asked anew.)
        b.owner_said(6, None, Some("me"));
        assert!(b.size.needs_ask(wide));
    }

    /// An asker that failed its last try stops at once (Neon R1 #5: it
    /// slept up to 37.5 s first, to find nothing to do).
    #[test]
    fn an_asker_out_of_tries_stops_at_once() {
        let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
        b.size.want((120, 30, 8, 16));
        b.size.asker = true;
        let mut asked = b.size.next().unwrap();
        for n in 1..SIZE_TRIES {
            assert_eq!(b.size_answered(asked, SizeAnswer::Unsure), SizeStep::Wait(n));
            asked = b.size.next().unwrap();
        }
        assert_eq!(b.size_answered(asked, SizeAnswer::Unsure), SizeStep::Done);
        assert!(!b.size.asker);
        b.size.want((121, 30, 8, 16));
        b.size.asker = true;
        let asked = b.size.next().unwrap();
        assert_eq!(b.size_answered(asked, SizeAnswer::Refused), SizeStep::Done, "a refusal: no wait either");
    }

    /// Only the last status request's answer applies (Astra R1 #11): two
    /// streams had in a row asked twice; the first answer, come after the
    /// second, replayed what the second kept, and the second, its buffer
    /// spent, undid a newer change. And nothing is kept when nothing is
    /// asked.
    #[test]
    fn only_the_last_status_request_applies_its_answer() {
        let mut b = BlockRuntime::new(40, 12, &TerminalTheme::default());
        let change = |v| StatusUpdate::parse("program_status_changed", &v).unwrap();
        b.status_event(39, change(json!({"id":"","record":{"state":"working"},"reason":"report"})));
        assert!(b.pending_status.is_none(), "nothing asked: nothing kept");
        let first = b.begin_status();
        let second = b.begin_status();
        b.status_event(42, change(json!({"id":"","record":{"state":"blocked"},"reason":"report"})));
        b.status_answered(second, Some(olo_rex_client::RpcResult { seq: 41, payload: vec![ProgramStatusRecord { state: "working".into(), ..Default::default() }] }));
        assert_eq!(b.recompute_activity(), AgentActivity::Blocked, "42 replayed over 41");
        b.status_answered(first, Some(olo_rex_client::RpcResult { seq: 40, payload: vec![ProgramStatusRecord { state: "working".into(), ..Default::default() }] }));
        assert_eq!(b.recompute_activity(), AgentActivity::Blocked, "the earlier request's answer, come late, changes nothing");
        assert!(b.pending_status.is_none());
    }

    /// What a stream left is offered back to its own host instance and
    /// client only, and not after its time (Astra R1 #2): never sent to
    /// another server under the host's name. A takeover's resumed stream
    /// renews it; only its own token's end forgets it.
    #[test]
    fn what_a_stream_left_is_its_owners_alone() {
        let hub = RexHub::test_offline(|_| {});
        let inner = &hub.inner;
        let bk = BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: "b-1".into() };
        let owner = StreamOwner { host_instance: 7, client: Some("client:a".into()) };
        let left = |token: u8, owner: &StreamOwner| StreamLeft { token: [token; 16], bytes: 100, at: Instant::now(), owner: owner.clone(), task: 1 };
        inner.streams_left.lock().unwrap().insert(bk.clone(), left(1, &owner));
        assert!(inner.stream_left_for(&bk, &StreamOwner { host_instance: 8, ..owner.clone() }).is_none(), "another host instance: none");
        assert!(inner.stream_left_for(&bk, &owner).is_some(), "and its own still has it (Astra R2 #4: it was forgotten)");
        assert!(inner.stream_left_for(&bk, &StreamOwner { client: Some("client:b".into()), ..owner.clone() }).is_none(), "another client: none");
        inner.streams_left.lock().unwrap().insert(bk.clone(), StreamLeft { at: Instant::now() - STREAM_LEFT_KEPT, ..left(1, &owner) });
        assert!(inner.stream_left_for(&bk, &owner).is_none(), "past its time, looked up");
        inner.streams_left.lock().unwrap().insert(bk.clone(), left(1, &owner));
        inner.renew_stream_left(&bk, &[1; 16], [2; 16], 120);
        let renewed = inner.stream_left_for(&bk, &owner).unwrap();
        assert_eq!((renewed.token, renewed.bytes), ([2; 16], 120), "the resumed stream's own token");
        inner.forget_stream_left(&bk, &[1; 16]);
        assert!(inner.stream_left_for(&bk, &owner).is_some(), "an old token's end doesn't forget the new");
        inner.forget_stream_left(&bk, &[2; 16]);
        assert!(inner.stream_left_for(&bk, &owner).is_none());
        inner.streams_left.lock().unwrap().insert(bk.clone(), left(3, &owner));
        inner.forget_streams_of("dev-box");
        assert!(inner.streams_left.lock().unwrap().is_empty(), "a host gone or replaced: all of its");
    }

    /// A task of a pane's that ends after a later one began (the pane had
    /// anew after a reconnect, the old task's end signalled, not waited
    /// for) neither leaves its token over what the later one left nor
    /// forgets it (Astra R2 #4): the later stream, lost, then had nothing
    /// to take it over with, and its pane stayed blank until the server
    /// ended it.
    #[test]
    fn a_task_that_ends_late_leaves_the_later_ones_alone() {
        let hub = RexHub::test_offline(|_| {});
        let inner = &hub.inner;
        let bk = BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: "b-1".into() };
        let owner = StreamOwner { host_instance: 7, client: Some("client:a".into()) };
        let left = |token: u8, task: u64| StreamLeft { token: [token; 16], bytes: 100, at: Instant::now(), owner: owner.clone(), task };
        // (The later task, 2, lost its stream; then the old one, 1, ends.)
        inner.keep_stream_left(&bk, left(2, 2));
        inner.keep_stream_left(&bk, left(1, 1));
        assert_eq!(inner.stream_left_for(&bk, &owner).map(|l| l.token), Some([2; 16]), "the later one's stays");
        // (The old task had a stream anew, as it ended: the later one's isn't its to forget.)
        inner.forget_stream_left_before(&bk, 1);
        assert!(inner.stream_left_for(&bk, &owner).is_some());
        // (The later task's own new stream: its own is forgotten.)
        inner.forget_stream_left_before(&bk, 2);
        assert!(inner.stream_left_for(&bk, &owner).is_none());
        // (A task's own later loss: kept over its earlier one.)
        inner.keep_stream_left(&bk, left(3, 3));
        inner.keep_stream_left(&bk, left(4, 3));
        assert_eq!(inner.stream_left_for(&bk, &owner).map(|l| l.token), Some([4; 16]));
    }

    /// The sessions the windows show outlast their host's connection
    /// (Astra R1 #14, Neon R1 #7: they were read from the front panes,
    /// which a disconnect clears, and a connection made again attached
    /// in the server's order); and only terminals count for the streams
    /// opened beforehand.
    #[test]
    fn the_sessions_shown_outlast_a_disconnect() {
        let hub = RexHub::test_offline(|_| {});
        let win = crate::ui::WinId(1);
        hub.window_shows(win, "dev-box", Some("s2"));
        hub.set_window_front_pane(win, None);
        assert_eq!(hub.inner.sessions_shown("dev-box"), HashSet::from(["s2".to_owned()]));
        assert!(hub.inner.sessions_shown("other").is_empty());
        hub.window_shows(win, "dev-box", None);
        assert!(hub.inner.sessions_shown("dev-box").is_empty(), "a window closed shows nothing");
        let mut blocks = HashMap::new();
        for (id, creator) in [("t-1", "com.superlogical.terminal"), ("t-2", "com.superlogical.terminal"), ("w-1", "com.superlogical.web")] {
            let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
            // (As its session's view had it: `ingest_session`.)
            b.creator_name = creator.into();
            b.is_terminal = attaches_a_stream(&BlockInfo { block_id: id.into(), label: String::new(), creator_name: creator.into(), flavor: String::new(), host_id: String::new(), owner: None });
            blocks.insert(BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: id.into() }, Arc::new(Mutex::new(b)));
        }
        assert_eq!(terminals_of(&blocks, "dev-box"), 2);
    }

    /// A size its host didn't take isn't taken as given: it is asked
    /// again (it was taken as given the moment it was asked, and a view
    /// already that size asked nothing more).
    #[test]
    fn a_size_not_taken_is_asked_again() {
        let mut s = SizeAsked::default();
        let big = (269, 70, 6, 15);
        s.want(big);
        assert_eq!(s.next(), Some(big));
        assert_eq!(s.next(), None, "one on its way at a time");
        assert!(s.answered(big, SizeAnswer::Unsure), "a failure of the size wanted now");
        assert_eq!(s.next(), Some(big), "not known to be given: asked again");
        assert!(!s.answered(big, SizeAnswer::Given));
        assert_eq!(s.next(), None, "given: nothing more to ask");
        // (Never more than so many times in a row, until the view's size changes.)
        let mut s = SizeAsked::default();
        s.want(big);
        for _ in 0..SIZE_TRIES {
            let asked = s.next().expect("asked while tries are left");
            s.answered(asked, SizeAnswer::Unsure);
        }
        assert_eq!(s.next(), None, "left after {SIZE_TRIES} failures in a row");
        s.want((270, 70, 6, 15));
        assert_eq!(s.next(), Some((270, 70, 6, 15)), "a new size is asked anew");
    }

    /// Sizes wanted while one is on its way: the last is asked when its
    /// answer comes, and none between (as a window is dragged larger).
    #[test]
    fn the_last_size_wanted_is_asked_last() {
        let mut s = SizeAsked::default();
        s.want((100, 30, 8, 16));
        let first = s.next().unwrap();
        for cols in 101..120 {
            s.want((cols, 30, 8, 16));
            assert_eq!(s.next(), None);
        }
        s.answered(first, SizeAnswer::Given);
        assert_eq!(s.next(), Some((119, 30, 8, 16)));
        s.answered((119, 30, 8, 16), SizeAnswer::Given);
        assert_eq!(s.next(), None);
    }

    /// The host's own word of its size (its stream's resize, another
    /// client's view having it) is what it has: the view's size is asked
    /// again where they differ.
    #[test]
    fn the_hosts_word_of_its_size_is_what_it_has() {
        let mut b = BlockRuntime::new(60, 12, &TerminalTheme::default());
        b.size.want((163, 43, 6, 15));
        let asked = b.size.next().unwrap();
        b.size.answered(asked, SizeAnswer::Given);
        assert!(b.inline(InlineApc::Resize { cols: 80, rows: 24, cell_width_px: 8, cell_height_px: 16, owner: None }, Some("here")).is_none());
        assert_eq!(b.size.next(), Some((163, 43, 6, 15)), "the host went back to 80×24: this view's size is asked again");
    }

    /// An answer that didn't come leaves the host's size unknown (Astra
    /// R1 #4): 100 had, 150 asked and given there, the view dragged back
    /// to 100 meanwhile, the answer to 150 lost. It was taken as the host
    /// still at 100, and nothing more was asked: the terminal stayed 150.
    #[test]
    fn a_lost_answer_leaves_the_hosts_size_unknown() {
        let mut s = SizeAsked::default();
        let (narrow, wide) = ((100, 30, 8, 16), (150, 30, 8, 16));
        s.want(narrow);
        let asked = s.next().unwrap();
        s.answered(asked, SizeAnswer::Given);
        s.want(wide);
        let asked = s.next().unwrap();
        s.want(narrow);
        assert!(!s.answered(asked, SizeAnswer::Unsure), "150 is wanted no longer: no failure of 100's");
        assert_eq!(s.next(), Some(narrow), "the host's size unknown: 100 asked again");
    }

    /// A refusal of a size wanted no longer isn't the new size's (Astra
    /// R1 #4: a 400 for a superseded size left the new one never asked).
    #[test]
    fn a_refusal_counts_against_the_size_refused() {
        let mut s = SizeAsked::default();
        s.want((100, 30, 8, 16));
        let asked = s.next().unwrap();
        s.want((120, 30, 8, 16));
        assert!(!s.answered(asked, SizeAnswer::Refused));
        assert_eq!(s.next(), Some((120, 30, 8, 16)));
        assert!(s.answered((120, 30, 8, 16), SizeAnswer::Refused), "refused itself");
        assert_eq!(s.next(), None, "a size refused as such isn't asked again");
    }

    /// The host's word of its size while an ask was on its way is what
    /// it has, not the answer that comes after it (Neon R1 #3): 160 asked,
    /// the host then at 80 (another client's view, its stream said so),
    /// the answer to 160 come last. It was taken as 160, and the view,
    /// 160 too, asked nothing.
    #[test]
    fn the_hosts_word_during_an_ask_outlasts_its_answer() {
        let mut b = BlockRuntime::new(60, 12, &TerminalTheme::default());
        let wide = (160, 40, 8, 16);
        b.size.want(wide);
        let asked = b.size.next().unwrap();
        b.inline(InlineApc::Resize { cols: 80, rows: 24, cell_width_px: 8, cell_height_px: 16, owner: None }, Some("here"));
        b.size.answered(asked, SizeAnswer::Given);
        assert_eq!(b.size.next(), Some(wide), "80×24 is what the host has: 160×40 asked again");
    }

    /// A new stream's screen is the host's size now: what was known of
    /// it is forgotten (Neon R1 #4: a pane had again after its PTY was
    /// resized elsewhere kept the old size as had, and asked nothing).
    #[test]
    fn a_new_streams_screen_forgets_the_hosts_size() {
        let mut b = BlockRuntime::new(60, 12, &TerminalTheme::default());
        let size = (163, 43, 6, 15);
        b.size.want(size);
        let asked = b.size.next().unwrap();
        b.size.answered(asked, SizeAnswer::Given);
        assert!(!b.size.needs_ask(size));
        let restored = GhosttyTerminal::new(90, 20).unwrap();
        b.restore_terminal(restored).unwrap();
        assert!(b.size.needs_ask(size), "the view hands its size on again");
        assert_eq!(b.size.next(), Some(size));
    }

    /// A new stream's screen tries its size anew, and an answer on its
    /// way to an ask of before it says nothing of the host's size now
    /// (Astra R2 #14): with its tries spent by the last stream's host,
    /// the pane kept its old size until its window's size next changed;
    /// and a late "given" set back the size the new screen had forgotten.
    #[test]
    fn a_new_screen_tries_its_size_anew() {
        let mut b = BlockRuntime::new(60, 12, &TerminalTheme::default());
        let size = (150, 40, 6, 15);
        b.size.want(size);
        for _ in 0..SIZE_TRIES {
            let asked = b.size.next().unwrap();
            b.size.answered(asked, SizeAnswer::Unsure);
        }
        assert!(!b.size.needs_ask(size), "its tries spent");
        b.restore_terminal(GhosttyTerminal::new(80, 24).unwrap()).unwrap();
        assert!(b.size.needs_ask(size), "tried anew on the new screen");
        // (An ask on its way as the new screen comes: its answer isn't the host's size.)
        let asked = b.size.next().unwrap();
        b.restore_terminal(GhosttyTerminal::new(80, 24).unwrap()).unwrap();
        b.size.answered(asked, SizeAnswer::Given);
        assert!(b.size.needs_ask(size), "asked again after it: the host's size isn't known");
    }

    /// A size wanted while its ask is on its way doesn't wake the asker
    /// (Neon R2 #6): the wake was kept, and the asker's next wait after
    /// a failure was skipped, asking again at once. One waiting is woken.
    #[test]
    fn a_size_wanted_while_one_is_asked_keeps_no_wake() {
        let (hub, _asked) = a_tab_held_by_another(None);
        let key = BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: "b-1".into() };
        let br = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks[&key].clone();
        let wake = {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            g.other_size_owner = false;
            g.size.want((100, 30, 8, 16));
            g.size.asker = true;
            g.size.asking = g.size.next();
            g.size_wake.clone()
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let woken = |wake: &Arc<tokio::sync::Notify>| rt.block_on(async { tokio::time::timeout(Duration::from_millis(20), wake.notified()).await.is_ok() });
        for cols in 101..=120 {
            hub.resize_block(&key, cols, 30, 8, 16);
        }
        assert!(!woken(&wake), "no wake kept while an ask is on its way");
        // (Answered, failed, waiting to ask again: a new size wakes it.)
        br.lock().unwrap_or_else(|e| e.into_inner()).size.asking = None;
        hub.resize_block(&key, 121, 30, 8, 16);
        assert!(woken(&wake), "one waiting is woken");
    }

    /// A pane's stream's word of whose its size is, unnumbered, doesn't
    /// undo the host's numbered word (Astra R2 #11): a resize sent before
    /// another desktop took the pane, read after the owner change, gave
    /// it back to this client, which then sent a size the host kept as a
    /// wish, and skipped a claim it needed.
    #[test]
    fn a_streams_word_of_the_owner_is_not_taken_over_the_hosts() {
        let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
        b.inline(InlineApc::Resize { cols: 90, rows: 20, cell_width_px: 8, cell_height_px: 16, owner: Some("other".into()) }, Some("me"));
        assert!(b.other_size_owner, "before any numbered word: the stream's");
        b.owner_said(12, Some("other"), Some("me"));
        b.inline(InlineApc::Resize { cols: 100, rows: 30, cell_width_px: 8, cell_height_px: 16, owner: Some("me".into()) }, Some("me"));
        assert!(b.other_size_owner, "the host's numbered word stands");
        assert_eq!(b.terminal.size(), (100, 30), "its size taken all the same");
        // (A claim answered at 10, after the owner change at 12: not taken.)
        b.owner_said(10, Some("me"), Some("me"));
        assert!(b.other_size_owner);
        b.owner_said(14, Some("me"), Some("me"));
        assert!(!b.other_size_owner, "a later claim's answer is");
    }

    /// The streams opened beforehand are counted by the terminals there,
    /// as each was had: by the word that has a stream had for it (Astra
    /// R2 #17), not by a creator's name.
    #[test]
    fn terminals_are_counted_as_their_streams_are_had() {
        let block = |creator: &str, flavor: &str| BlockInfo { block_id: "b".into(), label: String::new(), creator_name: creator.into(), flavor: flavor.into(), host_id: String::new(), owner: None };
        assert!(attaches_a_stream(&block("", "com.superlogical.terminal.shell")), "a terminal's flavor, no creator");
        assert!(attaches_a_stream(&block("com.superlogical.terminal", "")));
        assert!(!attaches_a_stream(&block("com.superlogical.terminal-notes", "notes")), "a creator whose name only begins so");
        let mut blocks = HashMap::new();
        for (id, terminal) in [("a", true), ("b", true), ("c", false)] {
            let mut b = BlockRuntime::new(80, 24, &TerminalTheme::default());
            (b.creator_name, b.is_terminal) = (String::new(), terminal);
            blocks.insert(BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: id.into() }, Arc::new(Mutex::new(b)));
        }
        assert_eq!(terminals_of(&blocks, "dev-box"), 2);
    }

    /// The view hands its size on where the host isn't known to have
    /// it, though the grid here is that size already (Astra R1 #5: the
    /// canvas compared only with its own grid and left a queued size).
    #[test]
    fn the_view_hands_on_a_size_its_grid_has() {
        let mut s = SizeAsked::default();
        let size = (100, 30, 8, 16);
        assert!(s.needs_ask(size), "never wanted");
        s.want(size);
        let asked = s.next().unwrap();
        s.asker = true;
        assert!(!s.needs_ask(size), "being asked");
        s.answered(asked, SizeAnswer::Unsure);
        s.asker = false;
        assert!(s.needs_ask(size), "the host may not have it, and nobody asks");
        let asked = s.next().unwrap();
        s.answered(asked, SizeAnswer::Given);
        assert!(!s.needs_ask(size), "had");
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
        let kept = serde_json::to_string(&crate::ui::SavedAppState {
            workspaces: Vec::new(),
            let_go: vec!["host-1".into()],
            help: Vec::new(),
            forwards: vec![forward.clone()],
            cached_host_screens: Vec::new(),
        })
        .unwrap();
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

    /// The server's word that a pane works is believed until the agent's
    /// own screen says its turn is over: Claude Code goes on reporting
    /// work while a shell it started still runs (its screen as it drew
    /// it, 2026-11-26: "1 shell" in its footer, no "esc to interrupt").
    #[test]
    fn a_turn_that_is_over_is_not_work_whatever_the_server_says() {
        let working = || vec![ProgramStatusRecord { state: "working".into(), ..Default::default() }];
        let rule = "─".repeat(96);
        let mut b = BlockRuntime::new(100, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        b.program_statuses = working();
        // In a turn: its footer says so.
        b.terminal.vt_write(format!("Claude Code\r\n✢ Fermenting… (37s)\r\n{rule}\r\n❯ \r\n{rule}\r\n  ⏵⏵ auto mode on · 1 shell · esc to interrupt").as_bytes());
        assert_eq!(b.recompute_activity(), AgentActivity::Working);
        // Its turn over, the shell still running: at its prompt, nothing saying a turn is on.
        let mut b = BlockRuntime::new(100, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        b.program_statuses = working();
        b.terminal
            .vt_write(format!("Claude Code\r\n● QUESTION: is this seen?\r\n✻ Cooked for 9s · 1 shell still running\r\n{rule}\r\n❯ \r\n{rule}\r\n  ⏵⏵ auto mode on · 1 shell · ← 1 agent").as_bytes());
        assert_eq!(b.recompute_activity(), AgentActivity::Idle);
        // A program whose screen isn't read (no agent of ours): the server's word stands.
        let mut b = BlockRuntime::new(100, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        b.program_statuses = working();
        b.terminal.vt_write(b"make: building\r\n$ ");
        assert_eq!(b.recompute_activity(), AgentActivity::Working);
        // What the server says that isn't work is the server's to say.
        let mut b = BlockRuntime::new(100, 12, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
        b.program_statuses = vec![ProgramStatusRecord { state: "done".into(), ..Default::default() }];
        assert_eq!(b.recompute_activity(), AgentActivity::Done);
    }

    /// What a look at a pane's activity costs (`cargo test --release -p
    /// olo-rex activity_cost -- --ignored --nocapture`): made every
    /// `GUESS_AFTER` (150 ms) while a pane has output, so at most seven
    /// times a second a pane. With the server saying it works, its screen
    /// is read too now, to hear the agent say its turn is over.
    #[test]
    #[ignore]
    fn activity_cost() {
        let rule = "─".repeat(116);
        let screen = format!("Claude Code\r\n● Read 6 files\r\n\r\n✢ Fermenting… (37s · ↓ 3.0k tokens)\r\n{rule}\r\n❯ \r\n{rule}\r\n  ⏵⏵ auto mode on · 1 shell · esc to interrupt");
        let n = 20_000u32;
        for (what, records) in [("the server silent (as before)", Vec::new()), ("the server saying it works", vec![ProgramStatusRecord { state: "working".into(), ..Default::default() }])] {
            let mut b = BlockRuntime::new(120, 40, &themes::from_olo_terminal(&olo_theme::Palette::light().terminal));
            b.terminal.vt_write(screen.as_bytes());
            b.program_statuses = records;
            let t = Instant::now();
            for _ in 0..n {
                std::hint::black_box(b.recompute_activity());
            }
            println!("a look with {what}: {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / f64::from(n));
        }
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
        // (Its status asked for: events come while the answer is on its way.)
        let asked = b.begin_status();
        b.status_event(3, change(json!({"id":"","record":{"state":"working"},"reason":"report"})));
        b.status_event(4, change(json!({"id":"job","record":{"state":"blocked"},"reason":"report"})));
        b.status_answered(asked, Some(olo_rex_client::RpcResult { seq: 2, payload: vec![] }));
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

    #[test]
    fn round3_f12_f13_f32_host_screens_merge_preserves_probe_fields_and_older_helpers() {
        use olo_rex_link::screen_wire::{HostScreen, HostScreenState, HostScreenStatus};
        use olo_screen_pane::rex_sock::RexScreen;

        let initial_row = crate::host_screens::ScreenRow { name: "legacy-row".into(), title: Some("From Probe".into()), running: true, archived: false, size: (1280, 720), started: Some(100) };
        let base_probe = crate::host_screens::ScreensThere {
            helper: true,
            helper_follow: false,
            installed: Some("2026.11.20".into()),
            committed: Some(100),
            on_host: false,
            chrome: Some("131.0".into()),
            missing: vec!["libwayland-client.so.0".into()],
            older: vec!["libc.so.6".into()],
            software: false,
            bytes: 12_345,
            running: vec!["legacy-row".into()],
            rows: vec![initial_row],
        };

        // Finding 12: when `has_follow_snapshot == false` (older helper lacking `follow`),
        // probe rows and fields MUST stay untouched!
        let no_snap = crate::screens_sock::LiveHostScreens { host_instance_id: 1, conn_id: 7, rev: 2, has_follow_snapshot: false, status: None, screens: vec![], ..Default::default() };
        let mut kept = base_probe.clone();
        assert!(!merge_live_screens_into_there(&mut kept, &no_snap, 7));
        assert_eq!(kept, base_probe);

        // Stale conn_id must also be ignored!
        let live_old_conn = crate::screens_sock::LiveHostScreens {
            host_instance_id: 1,
            conn_id: 6,
            rev: 3,
            has_follow_snapshot: true,
            status: Some(HostScreenStatus { installed: Some("2026.11.27".into()), committed: Some(200), default_on_host: true, not_set_up_reason: None }),
            screens: vec![],
            ..Default::default()
        };
        assert!(!merge_live_screens_into_there(&mut kept, &live_old_conn, 7));
        assert_eq!(kept, base_probe);

        // Finding 13: matching conn_id with `has_follow_snapshot == true` updates rows/status
        // while preserving probe-owned `chrome`, `missing`, `older`, `software`, and `bytes`!
        let live_now = crate::screens_sock::LiveHostScreens {
            host_instance_id: 1,
            conn_id: 7,
            rev: 4,
            has_follow_snapshot: true,
            status: Some(HostScreenStatus { installed: Some("2026.11.27".into()), committed: Some(200), default_on_host: true, not_set_up_reason: None }),
            screens: vec![RexScreen::from(HostScreen {
                name: "live-qa".into(),
                title: Some("QA".into()),
                state: HostScreenState::Running,
                width: 1440,
                height: 900,
                scale: 1.0,
                sound: true,
                pid: Some(42),
                started: Some(500),
                last_used: Some(900),
                actions: Some(3),
                controlled: false,
                owner: None,
                started_by: None,
            })],
            ..Default::default()
        };
        assert!(merge_live_screens_into_there(&mut kept, &live_now, 7));
        assert_eq!(kept.installed.as_deref(), Some("2026.11.27"));
        assert_eq!(kept.committed, Some(200));
        assert!(kept.on_host);
        assert_eq!(kept.running, vec!["live-qa".to_owned()]);
        assert_eq!(kept.rows.len(), 1);
        assert_eq!(kept.rows[0].name, "live-qa");
        // Probe-owned machine fields MUST survive verbatim:
        assert_eq!(kept.chrome.as_deref(), Some("131.0"));
        assert_eq!(kept.missing, vec!["libwayland-client.so.0".to_owned()]);
        assert_eq!(kept.older, vec!["libc.so.6".to_owned()]);
        assert!(!kept.software);
        assert_eq!(kept.bytes, 12_345);

        // Verify `HostRuntime::test_connected` constructs an SSH host runtime cleanly.
        let rt_host = HostRuntime::test_connected("dev-box", HashMap::new());
        assert_eq!(rt_host.key, "dev-box");
        assert!(crate::screens_sock::is_ssh_endpoint(&rt_host.endpoint_raw));
    }

    #[test]
    fn new_tab_before_window_places_right_of_active_and_respects_subagents() {
        let win = |id: &str| olo_rex_client::WindowView { window_id: id.into(), label: id.into(), focused_block_id: None, zoomed_block_id: None, layers: Vec::new() };
        let sub = |id: &str, wid: &str, parent: &str| crate::subagents::NativeSubagent {
            id: id.into(),
            name: None,
            host_key: "local".into(),
            session_id: "s1".into(),
            window_id: wid.into(),
            block_id: format!("b-{wid}"),
            parent_window_id: parent.into(),
            parent_block_id: None,
            cli: crate::subagents::AgentCli::Claude,
            model: String::new(),
            effort: String::new(),
            title: wid.into(),
            role: None,
            cwd: None,
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: 0,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 0,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: None,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
        };

        let mut reg = crate::subagents::SubagentRegistry::default();
        reg.register(sub("sa-1", "c1", "w1"));
        reg.register(sub("sa-1a", "c1a", "c1"));
        reg.register(sub("sa-2", "c2", "w2"));

        let mk_sess = |ids: &[&str], active: Option<&str>| SessionView {
            session_id: "s1".into(),
            label: "work".into(),
            revision: 1,
            active_window_id: active.map(str::to_owned),
            windows: ids.iter().map(|id| win(id)).collect(),
        };

        // Empty session & single tab: append at end (None).
        assert_eq!(new_tab_before_window(&reg, "local", &mk_sess(&[], None)), None);
        assert_eq!(new_tab_before_window(&reg, "local", &mk_sess(&["w0"], Some("w0"))), None);

        // Active tab in middle (with children grouped under w1): insert before w2.
        let sess_mid = mk_sess(&["w0", "w1", "c1", "c1a", "w2", "c2"], Some("w1"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_mid).as_deref(), Some("w2"));

        // Active tab at start: insert before w1.
        let sess_start = mk_sess(&["w0", "w1", "c1", "c1a", "w2", "c2"], Some("w0"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_start).as_deref(), Some("w1"));

        // Active tab at end (last top-level tab): append at end (None).
        let sess_end = mk_sess(&["w0", "w1", "c1", "c1a", "w2", "c2"], Some("w2"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_end), None);

        // Active tab is a direct child or grandchild subagent under middle top-level tab w1:
        // insert immediately right of w1 (and its children), i.e., before w2.
        let sess_child = mk_sess(&["w0", "w1", "c1", "c1a", "w2", "c2"], Some("c1"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_child).as_deref(), Some("w2"));
        let sess_grand = mk_sess(&["w0", "w1", "c1", "c1a", "w2", "c2"], Some("c1a"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_grand).as_deref(), Some("w2"));

        // Active tab is a child under the last top-level tab w2: append at end (None).
        let sess_last_child = mk_sess(&["w0", "w1", "c1", "c1a", "w2", "c2"], Some("c2"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_last_child), None);

        // Stale or empty `active_window_id` validates against `sess.windows` and
        // falls back to the first window (Neon #5).
        let sess_stale_active = mk_sess(&["w0", "w1", "w2"], Some("w_gone"));
        assert_eq!(active_top_level_window(&reg, "local", &sess_stale_active).as_deref(), Some("w0"));
        assert_eq!(new_tab_before_window(&reg, "local", &sess_stale_active).as_deref(), Some("w1"));
        let sess_empty_active = mk_sess(&["w0", "w1"], Some(""));
        assert_eq!(active_top_level_window(&reg, "local", &sess_empty_active).as_deref(), Some("w0"));

        // Neighbor recomputation after concurrent reorder or close (Astra #6, #8):
        // Start with [w0, w1, w2], source_root_id = w0. If another client moves
        // w1 to the front -> [w1, w0, w2, w_new], successor of w0 is w2 (NOT w1!).
        let moved_neighbor = mk_sess(&["w1", "w0", "w2", "w_new"], Some("w_new"));
        assert_eq!(successor_top_level_window(&reg, "local", &moved_neighbor, "w0", Some("w_new")).as_deref(), Some("w2"));
        // Post-move adjacency verification: if a concurrent move of w1 put w_new
        // to the LEFT of w0 (`[w_new, w1, w0, w2]`), `desired_move_for_new_tab`
        // detects non-adjacency and requests moving w_new before w2.
        let wrong_side = mk_sess(&["w_new", "w1", "w0", "w2"], Some("w_new"));
        assert_eq!(desired_move_for_new_tab(&reg, "local", &wrong_side, "w0", "w_new"), Some(Some("w2".into())));
        // If another client moved w0 to the end behind w_new (`[w_new, w1, w0]`),
        // `desired_move_for_new_tab` requests moving w_new to the END (`Some(None)`).
        let source_at_end = mk_sess(&["w_new", "w1", "w0"], Some("w_new"));
        assert_eq!(desired_move_for_new_tab(&reg, "local", &source_at_end, "w0", "w_new"), Some(None));
        // Already adjacent right of w0 (`[w1, w0, w_new, w2]` or `[w1, w0, w_new]`): None.
        let adjacent_mid = mk_sess(&["w1", "w0", "w_new", "w2"], Some("w_new"));
        assert_eq!(desired_move_for_new_tab(&reg, "local", &adjacent_mid, "w0", "w_new"), None);
        let adjacent_end = mk_sess(&["w1", "w0", "w_new"], Some("w_new"));
        assert_eq!(desired_move_for_new_tab(&reg, "local", &adjacent_end, "w0", "w_new"), None);
        // If w1 closed instead -> [w0, w2, w_new], successor of w0 is w2.
        let closed_neighbor = mk_sess(&["w0", "w2", "w_new"], Some("w_new"));
        assert_eq!(successor_top_level_window(&reg, "local", &closed_neighbor, "w0", Some("w_new")).as_deref(), Some("w2"));
        // If all neighbors right of w0 closed -> [w0, w_new], successor is None.
        let last_now = mk_sess(&["w0", "w_new"], Some("w_new"));
        assert_eq!(successor_top_level_window(&reg, "local", &last_now, "w0", Some("w_new")), None);

        // Rapid double/triple Cmd+T chaining resets cleanly on intervening Focus (Astra #2 / Neon #3).
        let mut chain = CreateChainTracker::default();
        assert_eq!(chain.resolve_source_root("s1", Some("w0".into())).as_deref(), Some("w0"));
        chain.record_created("s1", Some("w0"), "n1");
        assert_eq!(chain.resolve_source_root("s1", Some("w0".into())).as_deref(), Some("n1"));
        chain.record_created("s1", Some("w0"), "n2");
        assert_eq!(chain.resolve_source_root("s1", Some("w0".into())).as_deref(), Some("n2"));
        // Intervening explicit Focus clears the chain back to the clicked tab.
        chain.on_focus("s1");
        assert_eq!(chain.resolve_source_root("s1", Some("w0".into())).as_deref(), Some("w0"));
    }

    #[test]
    fn pending_window_placement_only_moves_exact_id_and_retires_atomically() {
        let win = |id: &str| olo_rex_client::WindowView { window_id: id.into(), label: id.into(), focused_block_id: None, zoomed_block_id: None, layers: Vec::new() };
        let p_w3 = PendingWindowPlacement { token: 1, new_window_id: "w3".into(), before_window_id: "w2".into() };

        // Intermediate view where server appended w3 before session.move_window finishes.
        let mut view = SessionView { session_id: "s1".into(), label: "work".into(), revision: 2, active_window_id: Some("w3".into()), windows: vec![win("w0"), win("w1"), win("w2"), win("w3")] };
        assert!(apply_pending_placements(&mut view, &[p_w3]));
        let order: Vec<&str> = view.windows.iter().map(|w| w.window_id.as_str()).collect();
        assert_eq!(order, ["w0", "w1", "w3", "w2"]);
        assert_eq!(view.active_window_id.as_deref(), Some("w3"));

        // An unfamiliar window `x_other` created by another client is NEVER moved
        // by a placement targeting `w3` (Astra #7 / Neon #4).
        let p_w3 = PendingWindowPlacement { token: 1, new_window_id: "w3".into(), before_window_id: "w2".into() };
        let mut view_other =
            SessionView { session_id: "s1".into(), label: "work".into(), revision: 3, active_window_id: Some("w1".into()), windows: vec![win("w0"), win("w1"), win("w2"), win("x_other")] };
        assert!(!apply_pending_placements(&mut view_other, &[p_w3]));
        let order: Vec<&str> = view_other.windows.iter().map(|w| w.window_id.as_str()).collect();
        assert_eq!(order, ["w0", "w1", "w2", "x_other"]);

        // Two concurrent placements with exact IDs never collide or steal each other's window.
        let p1 = PendingWindowPlacement { token: 1, new_window_id: "w3".into(), before_window_id: "w2".into() };
        let p2 = PendingWindowPlacement { token: 2, new_window_id: "w4".into(), before_window_id: "w1".into() };
        let mut view_both =
            SessionView { session_id: "s1".into(), label: "work".into(), revision: 4, active_window_id: Some("w4".into()), windows: vec![win("w0"), win("w1"), win("w2"), win("w3"), win("w4")] };
        assert!(apply_pending_placements(&mut view_both, &[p1, p2]));
        let order: Vec<&str> = view_both.windows.iter().map(|w| w.window_id.as_str()).collect();
        assert_eq!(order, ["w0", "w4", "w1", "w3", "w2"]);

        // Atomic retirement inside ingest (Astra #7 / Neon #2): if another client
        // moved w3 to the end (rev 11: [w0, w1, w2, w3]) before final ingest,
        // retiring token 1 BEFORE `apply_pending_placements` preserves server order.
        let mut placements: HashMap<String, Vec<PendingWindowPlacement>> = HashMap::new();
        placements.insert("s1".into(), vec![PendingWindowPlacement { token: 1, new_window_id: "w3".into(), before_window_id: "w2".into() }]);
        let mut rows = vec![SessionSummary { session_id: "s1".into(), label: "work".into() }];
        let mut sessions = HashMap::new();
        let rev11 = SessionView { session_id: "s1".into(), label: "work".into(), revision: 11, active_window_id: Some("w3".into()), windows: vec![win("w0"), win("w1"), win("w2"), win("w3")] };
        assert!(install_view(&mut rows, &mut sessions, rev11));
        remove_placement_token(&mut placements, "s1", 1);
        if let Some(list) = placements.get("s1")
            && let Some(v) = sessions.get_mut("s1")
        {
            apply_pending_placements(v, list);
        }
        let final_order: Vec<&str> = sessions["s1"].windows.iter().map(|w| w.window_id.as_str()).collect();
        assert_eq!(final_order, ["w0", "w1", "w2", "w3"]);
        assert!(placements.is_empty());
    }

    #[test]
    fn select_honors_source_tab_click_while_create_pending_and_guard_reorders_retroactively() {
        let win = |id: &str| olo_rex_client::WindowView { window_id: id.into(), label: id.into(), focused_block_id: None, zoomed_block_id: None, layers: Vec::new() };
        let notified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let notified_cb = notified.clone();
        let hub = RexHub::test_offline(move |_| {
            notified_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<HostInputOp>();
        let (win_tx, mut win_rx) = tokio::sync::mpsc::unbounded_channel::<HostWindowOp>();
        let sess = SessionView { session_id: "s1".into(), label: "work".into(), revision: 5, active_window_id: Some("w0".into()), windows: vec![win("w0"), win("w1")] };
        {
            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut host = HostRuntime::test_connected("local", HashMap::from([("s1".into(), sess)]));
            host.input_tx = Some(in_tx);
            host.window_tx = Some(win_tx);
            st.hosts.insert("local".into(), host);
        }

        // Without a pending create, clicking already-active `w0` is a no-op.
        hub.select_window("local", "s1", "w0");
        assert!(win_rx.try_recv().is_err());

        // With a pending create on `s1` (Astra #2), clicking `w0` MUST queue
        // `HostWindowOp::Focus { switch: true }` on `window_tx` (NOT `input_tx`, Astra #3 / Neon #1).
        hub.create_window("local", "s1", None, None, None);
        hub.select_window("local", "s1", "w0");
        assert!(in_rx.try_recv().is_err(), "window lifecycle/focus must stay off typing channel");
        assert!(matches!(win_rx.try_recv(), Ok(HostWindowOp::CreateWindow { ref session_id, .. }) if session_id == "s1"));
        match win_rx.try_recv() {
            Ok(HostWindowOp::Focus { session_id, switch, target }) => {
                assert_eq!(session_id, "s1");
                assert!(switch, "source-tab click while create is pending must force focus RPC");
                assert_eq!(target.window, "w0");
            }
            _ => panic!("expected HostWindowOp::Focus for source tab w0"),
        }

        // Retroactive reorder (Neon #2 / Astra #9): if `WindowCreated` already
        // installed `[w0, w1, w2]`, calling `PlacementGuard::set("w2", "w1")`
        // reorders `host.sessions["s1"]` immediately and notifies UI.
        {
            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let sess = st.hosts.get_mut("local").unwrap().sessions.get_mut("s1").unwrap();
            sess.windows.push(win("w2"));
            sess.active_window_id = Some("w2".into());
        }
        let before_notifies = notified.load(std::sync::atomic::Ordering::SeqCst);
        let mut guard = PlacementGuard { inner: hub.inner.clone(), hk: "local".into(), sid: "s1".into(), token: None };
        guard.set("w2".into(), "w1".into());
        {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let order: Vec<&str> = st.hosts["local"].sessions["s1"].windows.iter().map(|w| w.window_id.as_str()).collect();
            assert_eq!(order, ["w0", "w2", "w1"]);
        }
        assert!(notified.load(std::sync::atomic::Ordering::SeqCst) > before_notifies, "retroactive reorder must notify UI");
        drop(guard);
        assert!(hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts["local"].pending_placements.is_empty());
    }

    #[test]
    fn cwd_resolution_prefers_live_keeps_shell_fallback_filters_deleted_and_respects_timeout() {
        let proc_snap = TerminalProcessSnapshot {
            foreground: Some(olo_rex_client::ProcessInfo { pid: 222, name: "vim".into(), cwd: Some("/work/sub".into()), ..Default::default() }),
            child: Some(olo_rex_client::ProcessInfo { pid: 111, name: "zsh".into(), cwd: Some("/work".into()), ..Default::default() }),
            ..Default::default()
        };
        // Retains BOTH foreground and distinct shell fallback candidates (Astra #13).
        assert_eq!(cwds_from_process(Some(&proc_snap)), vec!["/work/sub", "/work"]);
        assert_eq!(block_cwd_candidates("/cached/dir", Some(&proc_snap)), vec!["/cached/dir", "/work/sub", "/work"]);
        assert_eq!(block_cwd_candidates("", Some(&proc_snap)), vec!["/work/sub", "/work"]);

        // Kernel ` (deleted)` foreground CWD is skipped in favor of live shell CWD (Neon #9).
        let deleted_fg = TerminalProcessSnapshot {
            foreground: Some(olo_rex_client::ProcessInfo { pid: 222, name: "vim".into(), cwd: Some("/var/tmp/tmp123 (deleted)".into()), ..Default::default() }),
            child: Some(olo_rex_client::ProcessInfo { pid: 111, name: "zsh".into(), cwd: Some("/work".into()), ..Default::default() }),
            ..Default::default()
        };
        assert_eq!(cwds_from_process(Some(&deleted_fg)), vec!["/work"]);
        assert_eq!(block_cwd_candidates("/gone (deleted)", Some(&deleted_fg)), vec!["/work"]);
        assert!(block_cwd_candidates("", None).is_empty());

        // shell_opts_with_cwd preserves existing options and attaches cwd cleanly.
        let base = Some(json!({ "env": { "SSH_AUTH_SOCK": "/tmp/a" } }));
        let with = shell_opts_with_cwd(base.clone(), Some("/srv/app")).unwrap();
        assert_eq!(with["cwd"], "/srv/app");
        assert_eq!(with["env"]["SSH_AUTH_SOCK"], "/tmp/a");
        assert_eq!(shell_opts_with_cwd(base.clone(), None), base);
        assert_eq!(shell_opts_with_cwd(base.clone(), Some("")), base);

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            // Live host lookup beats stale cached `g.pwd`, keeping distinct shell fallback.
            let got = resolve_new_window_cwd_candidates(None, vec!["/stale/home".into()], Duration::from_secs(1), || async { vec!["/live/fg".into(), "/live/shell".into()] }).await;
            assert_eq!(got, vec!["/live/fg", "/live/shell"]);

            // Explicit `cwd` skips live lookup and cached `g.pwd` entirely.
            let mut called = false;
            let got = resolve_new_window_cwd_candidates(Some("/explicit/dir".into()), vec!["/stale/home".into()], Duration::from_secs(1), || {
                called = true;
                async { vec!["/live/cd/dir".into()] }
            })
            .await;
            assert_eq!(got, vec!["/explicit/dir"]);
            assert!(!called);

            // Timed-out live lookup falls back to cached candidates cleanly within deadline.
            let got = resolve_new_window_cwd_candidates(None, vec!["/cached/fallback".into()], Duration::from_millis(20), || async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                vec!["/too/late".into()]
            })
            .await;
            assert_eq!(got, vec!["/cached/fallback"]);

            // Empty live lookup falls back to cached candidates.
            let got = resolve_new_window_cwd_candidates(None, vec!["/cached/fallback".into()], Duration::from_secs(1), || async { Vec::new() }).await;
            assert_eq!(got, vec!["/cached/fallback"]);
        });
    }

    #[test]
    fn create_window_retry_allowlists_directory_rejections_and_uses_shell_fallback() {
        use olo_rex_client::{ApiError, RexError};
        let api_bad_cwd = RexError::Api(ApiError { code: "invalid_argument".into(), message: "cwd does not exist".into(), status: 400 });
        let api_invalid_cwd = RexError::Api(ApiError { code: "invalid_cwd".into(), message: "missing".into(), status: 400 });
        let api_not_found = RexError::Api(ApiError { code: "not_found".into(), message: "session gone".into(), status: 404 });
        let api_session_not_found = RexError::Api(ApiError { code: "session_not_found".into(), message: "session gone".into(), status: 404 });
        let api_window_limit = RexError::Api(ApiError { code: "resource_exhausted".into(), message: "window limit reached".into(), status: 429 });
        let api_internal = RexError::Api(ApiError { code: "internal".into(), message: "spawn failed".into(), status: 500 });
        let api_bad_other = RexError::Api(ApiError { code: "invalid_argument".into(), message: "invalid window_label".into(), status: 400 });

        assert!(should_retry_new_window_without_cwd(&api_bad_cwd, true));
        assert!(should_retry_new_window_without_cwd(&api_invalid_cwd, true));
        assert!(!should_retry_new_window_without_cwd(&api_bad_cwd, false));
        assert!(!should_retry_new_window_without_cwd(&api_not_found, true));
        assert!(!should_retry_new_window_without_cwd(&api_session_not_found, true));
        assert!(!should_retry_new_window_without_cwd(&api_window_limit, true));
        assert!(!should_retry_new_window_without_cwd(&api_internal, true));
        assert!(!should_retry_new_window_without_cwd(&api_bad_other, true));
        assert!(!should_retry_new_window_without_cwd(&RexError::Timeout, true));
        assert!(!should_retry_new_window_without_cwd(&RexError::Closed("eof".into()), true));
        assert!(!should_retry_new_window_without_cwd(&RexError::Io(std::io::Error::other("reset")), true));

        // Accurate timeout error copy (Astra #8 / Neon #7).
        assert!(timeout_problem_detail(true).contains("refreshed"));
        assert!(!timeout_problem_detail(false).contains("refreshed"));

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let base = Some(json!({ "env": { "SSH_AUTH_SOCK": "/sock" } }));

            // When foreground cwd is rejected as invalid directory, tries shell cwd
            // NEXT before falling back to server default (Astra #13).
            let mut seen_opts = Vec::new();
            let candidates = vec!["/fg/deleted".to_owned(), "/work/shell".to_owned()];
            let res = create_window_rpc_candidates(&candidates, base.clone(), |opts| {
                seen_opts.push(opts.clone());
                let attempt = seen_opts.len();
                async move {
                    if attempt == 1 {
                        Err(RexError::Api(ApiError { code: "invalid_argument".into(), message: "working directory not found".into(), status: 400 }))
                    } else {
                        Ok(olo_rex_client::RpcResult { seq: 1, payload: json!({ "window_id": "w-shell" }) })
                    }
                }
            })
            .await;
            assert!(res.is_ok());
            assert_eq!(seen_opts.len(), 2);
            assert_eq!(seen_opts[0].as_ref().unwrap()["cwd"], "/fg/deleted");
            assert_eq!(seen_opts[1].as_ref().unwrap()["cwd"], "/work/shell");
            assert_eq!(seen_opts[1].as_ref().unwrap()["env"]["SSH_AUTH_SOCK"], "/sock");

            // If both candidates are rejected as bad directories, falls back to None (preserving base_opts).
            let mut seen_opts = Vec::new();
            let res = create_window_rpc_candidates(&candidates, base.clone(), |opts| {
                seen_opts.push(opts.clone());
                let attempt = seen_opts.len();
                async move {
                    if attempt <= 2 {
                        Err(RexError::Api(ApiError { code: "invalid_cwd".into(), message: "no such directory".into(), status: 400 }))
                    } else {
                        Ok(olo_rex_client::RpcResult { seq: 2, payload: json!({ "window_id": "w-default" }) })
                    }
                }
            })
            .await;
            assert!(res.is_ok());
            assert_eq!(seen_opts.len(), 3);
            assert!(seen_opts[2].as_ref().unwrap().get("cwd").is_none());
            assert_eq!(seen_opts[2].as_ref().unwrap()["env"]["SSH_AUTH_SOCK"], "/sock");

            // Window limit, session_not_found, internal error, and Timeout MUST NOT retry (call count == 1; Astra #5).
            for err_factory in [
                || RexError::Api(ApiError { code: "resource_exhausted".into(), message: "window limit reached".into(), status: 429 }),
                || RexError::Api(ApiError { code: "session_not_found".into(), message: "session missing".into(), status: 404 }),
                || RexError::Api(ApiError { code: "internal".into(), message: "internal error".into(), status: 500 }),
                || RexError::Timeout,
            ] {
                let mut calls = 0usize;
                let res = create_window_rpc_candidates(&["/srv/app".to_owned()], base.clone(), |_opts| {
                    calls += 1;
                    let e = err_factory();
                    async move { Err(e) }
                })
                .await;
                assert!(res.is_err());
                assert_eq!(calls, 1, "non-cwd error must never retry");
            }
        });
    }

    /// Resolve the concrete absolute path to the `rex` binary in the parent
    /// before clearing `PATH` for the isolated child test process (Astra #15 / Neon #6).
    fn resolve_rex_binary_abs() -> Option<PathBuf> {
        let candidate = olo_rex_client::find_rex_binary();
        if candidate.is_absolute() && candidate.is_file() {
            return Some(candidate);
        }
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join(&candidate)).find(|p| p.is_absolute() && p.is_file())
    }

    /// End-to-end live check driving the real `RexHub::create_window` against
    /// an isolated scratch Rex server in its own private `/var/tmp` directory:
    /// - Pane runs `/bin/bash --norc` (no OSC 7) and runs `cd project-sub`
    ///   after attach while cached `BlockRuntime::pwd` still holds stale `dir`.
    /// - Drives real `RexHub::create_window` and asserts:
    ///   1. Placement immediately right of `w0` (`[w0, w_mid, w1]`).
    ///   2. Focus on `w_mid` and exactly 1 additional window.
    ///   3. New tab's shell starts in `project-sub` (live lookup beats stale cache).
    ///   4. `on_created` callback fires only after creation succeeds.
    /// - Exercises rapid double `Cmd+T` producing left-to-right chaining.
    /// - Exercises explicit stale/deleted `cwd` producing 1 usable shell tab.
    #[test]
    fn live_new_tab_placement_cwd_and_stale_dir_recovery() {
        const CASE: &str = "OLO_REX_CMD_T_CASE";
        const SCRATCH_ENV: &str = "OLO_REX_CMD_T_SCRATCH";
        const BIN_ENV: &str = "OLO_REX_CMD_T_BIN";
        const CHILD_SENTINEL: &str = "isolated-child-v1";

        let root = std::env::var_os("OLO_REX_TEST_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/tmp"));

        if std::env::var_os(CASE).is_none() {
            let Some(rex_abs) = resolve_rex_binary_abs() else {
                eprintln!("skipping live Rex Cmd+T test: rex binary unavailable");
                return;
            };
            use std::os::unix::fs::DirBuilderExt as _;
            let dir = root.join(format!("olo-rex-cmd-t-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]));
            std::fs::DirBuilder::new().mode(0o700).create(&dir).expect("create private scratch dir");

            struct DirGuard(PathBuf);
            impl Drop for DirGuard {
                fn drop(&mut self) {
                    let _ = std::fs::remove_dir_all(&self.0);
                }
            }
            let _dir_guard = DirGuard(dir.clone());

            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "hub::tests::live_new_tab_placement_cwd_and_stale_dir_recovery", "--nocapture"])
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .env("SHELL", "/bin/sh")
                .env("HOME", &dir)
                .env("XDG_RUNTIME_DIR", &dir)
                .env("XDG_CONFIG_HOME", dir.join("config"))
                .env("XDG_CACHE_HOME", dir.join("cache"))
                .env("XDG_STATE_HOME", dir.join("state-home"))
                .env("TMPDIR", &dir)
                .env("REX_SERVER_SOCKET", dir.join("rex.sock"))
                .env("OLO_REX_AGENTS_DIR", dir.join("agents"))
                .env("OLO_REX_TEST_DIR", &root)
                .env(CASE, CHILD_SENTINEL)
                .env(SCRATCH_ENV, &dir)
                .env(BIN_ENV, &rex_abs)
                .current_dir(&dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
                cmd.env("LD_LIBRARY_PATH", path);
            }
            let mut child = cmd.spawn().expect("spawn isolated live Cmd+T test child");
            let deadline = Instant::now() + Duration::from_secs(45);
            let status = loop {
                if let Some(st) = child.try_wait().expect("poll live Cmd+T child") {
                    break Some(st);
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let mut out = String::new();
            let mut err = String::new();
            if let Some(mut s) = child.stdout.take() {
                let _ = std::io::Read::read_to_string(&mut s, &mut out);
            }
            if let Some(mut s) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut s, &mut err);
            }
            let last_phase = err.lines().rev().find(|l| l.contains("[phase]")).unwrap_or("(no phase reported)");
            match status {
                Some(st) => assert!(st.success(), "child failed ({st}) at {last_phase}\nstdout:\n{out}\nstderr:\n{err}"),
                None => panic!("isolated live Cmd+T test timed out after 45s at {last_phase}\nstdout:\n{out}\nstderr:\n{err}"),
            }
            return;
        }

        // Strict child isolation validation (Astra #14 & #15): reject accidental
        // direct entry with inherited env vars before touching any path.
        assert_eq!(std::env::var(CASE).as_deref(), Ok(CHILD_SENTINEL), "unexpected {CASE} value; refuse unisolated run");
        let dir = PathBuf::from(std::env::var_os(SCRATCH_ENV).expect("missing parent-issued scratch dir"));
        let home = PathBuf::from(std::env::var_os("HOME").expect("missing HOME"));
        let sock = PathBuf::from(std::env::var_os("REX_SERVER_SOCKET").expect("missing REX_SERVER_SOCKET"));
        let rex_bin = PathBuf::from(std::env::var_os(BIN_ENV).expect("missing parent-resolved rex binary"));
        assert!(dir.is_absolute() && dir.starts_with(&root) && dir.is_dir(), "scratch dir {} must be an existing directory inside {}", dir.display(), root.display());
        assert_eq!(home, dir, "HOME must equal parent-issued scratch dir");
        assert_eq!(sock, dir.join("rex.sock"), "REX_SERVER_SOCKET must live inside scratch dir");
        assert!(rex_bin.is_absolute() && rex_bin.is_file(), "parent-resolved rex binary missing: {}", rex_bin.display());

        eprintln!("[phase] server_starting");
        let sub_dir = dir.join("project-sub");
        std::fs::create_dir_all(&sub_dir).expect("create sub_dir");
        let addr = format!("unix://{}", sock.display());
        let child = std::process::Command::new(&rex_bin)
            .args(["-S", &addr, "server", "run", "--listen", &addr, "--restore=false", "--state-dir"])
            .arg(dir.join("rex-state"))
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(dir.join("server.log")).expect("server.log"))
            .spawn()
            .expect("spawn scratch rex server");

        struct Fixture(std::process::Child);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut fixture = Fixture(child);

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let (ctrl, sid, w0, b0, w1) = rt
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(15), async {
                    // Wait until the scratch server is actually accepting connections.
                    tokio::time::timeout(Duration::from_secs(6), async {
                        loop {
                            if tokio::net::UnixStream::connect(&sock).await.is_ok() {
                                return Ok::<(), String>(());
                            }
                            if let Some(status) = fixture.0.try_wait().map_err(|e| e.to_string())? {
                                let log = std::fs::read_to_string(dir.join("server.log")).unwrap_or_default();
                                return Err(format!("scratch rex exited {status}: {log}"));
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    })
                    .await
                    .map_err(|_| "scratch rex did not start listening".to_owned())??;
                    eprintln!("[phase] server_listening");

                    let ep = RexEndpoint::Unix(sock.clone());
                    let (stream, host_hdr) = ep.connect_stream().await.map_err(|e| e.to_string())?;
                    let (ctrl, _ev) = ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(Some("cmd-t-setup".into())), None).await.map_err(|e| e.to_string())?;

                    // Start w0 with `/bin/bash --norc` (no OSC 7) in `dir`, then add w1 and focus w0.
                    let created = ctrl.session_create(Some("tabs"), Some(json!({ "command": ["/bin/bash", "--norc"], "cwd": dir }))).await.map_err(|e| e.to_string())?;
                    let sid = created.payload.get("session_id").and_then(Value::as_str).ok_or("missing session_id")?.to_owned();
                    let att = ctrl.session_attach(&sid).await.map_err(|e| e.to_string())?;
                    let w0 = att.payload.view.windows[0].window_id.clone();
                    let b0 = att.payload.view.windows[0].all_blocks().next().ok_or("missing b0")?.block_id.clone();
                    let w1 = ctrl.session_new_window(&sid, Some("W1"), Some(json!({ "command": ["/bin/bash", "--norc"], "cwd": dir }))).await.map_err(|e| e.to_string())?.payload["window_id"]
                        .as_str()
                        .ok_or("missing w1")?
                        .to_owned();
                    ctrl.session_focus_window(&sid, &w0).await.map_err(|e| e.to_string())?;
                    Ok::<_, String>((ctrl, sid, w0, b0, w1))
                })
                .await
                .expect("scratch Rex setup timed out")
            })
            .expect("scratch Rex setup failed");

        // Boot a real RexHub outside `rt.block_on` so its internal runtime
        // drops cleanly in a non-async context after `rt.block_on` returns.
        let hub = RexHub::start(Settings::default(), TerminalTheme::default(), None, None, |_| {});
        let res: Result<(), String> = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                let bk0 = BlockKey { host_key: "local".into(), session_id: sid.clone(), block_id: b0.clone() };
                loop {
                    let ready = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        st.hosts.get("local").is_some_and(|h| h.window_tx.is_some() && h.sessions.get(&sid).is_some_and(|s| s.windows.len() == 2 && s.active_window_id.as_deref() == Some(w0.as_str())))
                            && st.blocks.contains_key(&bk0)
                    };
                    if ready {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                eprintln!("[phase] hub_ready");

                // Change directory inside w0's non-OSC-7 shell to `sub_dir` and
                // wait until the host's live `terminal_process` reflects it.
                ctrl.terminal_write(&sid, &b0, &format!("cd {}\n", shell_quote(sub_dir.to_str().unwrap()))).await.map_err(|e| e.to_string())?;
                loop {
                    if let Ok(proc_res) = ctrl.terminal_process(&sid, &b0).await
                        && cwd_from_process(Some(&proc_res.payload)).as_deref() == sub_dir.to_str()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
                eprintln!("[phase] cd_complete");

                // Force cached `BlockRuntime::pwd` to the stale initial `dir` to
                // prove `RexHub::create_window` resolves live host CWD first!
                {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(br) = st.blocks.get(&bk0) {
                        br.lock().unwrap_or_else(|e| e.into_inner()).pwd = dir.to_string_lossy().into_owned();
                    }
                }

                // Drive the real `RexHub::create_window` entry point.
                let (created_tx, created_rx) = tokio::sync::oneshot::channel::<()>();
                hub.create_window(
                    "local",
                    &sid,
                    None,
                    None,
                    Some(Box::new(move || {
                        let _ = created_tx.send(());
                    })),
                );
                created_rx.await.map_err(|_| "on_created callback did not fire".to_owned())?;
                eprintln!("[phase] create_window_called");

                let server_view = ctrl.session_view(&sid).await.map_err(|e| e.to_string())?.payload;
                assert_eq!(server_view.windows.len(), 3, "expected exactly 1 new window");
                let w_mid = server_view.windows[1].window_id.clone();
                let mid_block = server_view.windows[1].all_blocks().next().ok_or("missing mid block")?.block_id.clone();
                let order: Vec<&str> = server_view.windows.iter().map(|w| w.window_id.as_str()).collect();
                assert_eq!(order, [w0.as_str(), w_mid.as_str(), w1.as_str()]);
                assert_eq!(server_view.active_window_id.as_deref(), Some(w_mid.as_str()));

                // Hub state matches server order and focus.
                {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    let hub_view = st.hosts.get("local").and_then(|h| h.sessions.get(&sid)).ok_or("missing hub session")?;
                    let hub_order: Vec<&str> = hub_view.windows.iter().map(|w| w.window_id.as_str()).collect();
                    assert_eq!(hub_order, [w0.as_str(), w_mid.as_str(), w1.as_str()]);
                    assert_eq!(hub_view.active_window_id.as_deref(), Some(w_mid.as_str()));
                }

                // Verify the new tab's shell is running in `sub_dir` (not stale `dir`).
                let mut observed_cwd = None;
                for _ in 0..40 {
                    if let Ok(proc_res) = ctrl.terminal_process(&sid, &mid_block).await {
                        observed_cwd = cwd_from_process(Some(&proc_res.payload));
                        if observed_cwd.as_deref() == sub_dir.to_str() {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                assert_eq!(observed_cwd.as_deref(), sub_dir.to_str());

                // Rapid double Cmd+T from `w_mid` produces left-to-right order `[w0, w_mid, r1, r2, w1]` (Astra #2 / Neon #3).
                let (r1_tx, r1_rx) = tokio::sync::oneshot::channel::<()>();
                let (r2_tx, r2_rx) = tokio::sync::oneshot::channel::<()>();
                hub.create_window(
                    "local",
                    &sid,
                    Some(sub_dir.to_string_lossy().into_owned()),
                    None,
                    Some(Box::new(move || {
                        let _ = r1_tx.send(());
                    })),
                );
                hub.create_window(
                    "local",
                    &sid,
                    Some(sub_dir.to_string_lossy().into_owned()),
                    None,
                    Some(Box::new(move || {
                        let _ = r2_tx.send(());
                    })),
                );
                r1_rx.await.map_err(|_| "rapid create #1 did not finish".to_owned())?;
                r2_rx.await.map_err(|_| "rapid create #2 did not finish".to_owned())?;
                let after_rapid = ctrl.session_view(&sid).await.map_err(|e| e.to_string())?.payload;
                assert_eq!(after_rapid.windows.len(), 5, "rapid double Cmd+T must add exactly 2 tabs");
                assert_eq!(after_rapid.windows[0].window_id, w0);
                assert_eq!(after_rapid.windows[1].window_id, w_mid);
                assert_eq!(after_rapid.windows[4].window_id, w1);
                // Active window should be the second rapid tab (at index 3, right of index 2).
                assert_eq!(after_rapid.active_window_id.as_deref(), Some(after_rapid.windows[3].window_id.as_str()));
                eprintln!("[phase] rapid_double_checked");

                // Verify explicit stale/deleted `cwd` still creates exactly 1 usable tab via RexHub::create_window.
                let stale = dir.join("deleted-dir-does-not-exist");
                let (stale_tx, stale_rx) = tokio::sync::oneshot::channel::<()>();
                hub.create_window(
                    "local",
                    &sid,
                    Some(stale.to_string_lossy().into_owned()),
                    None,
                    Some(Box::new(move || {
                        let _ = stale_tx.send(());
                    })),
                );
                stale_rx.await.map_err(|_| "stale cwd create_window did not complete".to_owned())?;
                let after_stale = ctrl.session_view(&sid).await.map_err(|e| e.to_string())?.payload;
                assert_eq!(after_stale.windows.len(), 6, "stale cwd create must add exactly 1 tab");
                let stale_win = after_stale.active_window().ok_or("missing active stale window")?;
                let stale_block = stale_win.all_blocks().next().ok_or("missing stale block")?.block_id.clone();
                let mut stale_has_shell = false;
                for _ in 0..40 {
                    if let Ok(proc_res) = ctrl.terminal_process(&sid, &stale_block).await
                        && cwd_from_process(Some(&proc_res.payload)).is_some()
                    {
                        stale_has_shell = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                assert!(stale_has_shell, "tab created after stale cwd should have a running shell");
                eprintln!("[phase] done");
                Ok(())
            })
            .await
            .expect("scratch Rex cmd-t test timed out")
        });
        res.expect("live new tab placement & cwd test failed");
    }

    fn test_inner_with_state(state: Arc<Mutex<HubState>>, rt: &tokio::runtime::Runtime) -> (Arc<RexHubInner>, Arc<Mutex<Vec<HubNotice>>>) {
        let notices: Arc<Mutex<Vec<HubNotice>>> = Arc::new(Mutex::new(Vec::new()));
        let notices_cb = notices.clone();
        let notify: Arc<dyn Fn(HubNotice) + Send + Sync> = Arc::new(move |n| {
            notices_cb.lock().unwrap().push(n);
        });
        let ui_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let opener = Arc::new(crate::screens_sock::HubScreenOpener::new(state.clone(), notify.clone(), ui_ready.clone()));
        let state_res = state.clone();
        let resolver = Arc::new(move |hk: &str, pane: &olo_rex_link::screen_wire::OwnerPane| crate::screens_sock::resolve_controlled_by_in_hub(&state_res, hk, pane));
        let screens_service = crate::screens_sock::ScreensService::new(PathBuf::new(), PathBuf::new(), 0, opener, resolver, None, None);
        let inner = Arc::new(RexHubInner {
            state,
            ssh_pool: Arc::new(SshSessionPool::default()),
            askpass: None,
            notify,
            spares: Mutex::new(HashMap::new()),
            stream_gen: std::sync::atomic::AtomicU64::new(0),
            streams_left: Mutex::new(HashMap::new()),
            shown_sessions: Mutex::new(HashMap::new()),
            agent_socks: Mutex::new(HashMap::new()),
            bridge_calls: Default::default(),
            run_jobs: crate::bridge_run::RunJobs::new(),
            run_approvals: crate::bridge_run::RunApprovals::new(|| {}, || {}),
            keeping: Mutex::new(HashMap::new()),
            agents_following: Mutex::new(HashMap::new()),
            screens_putting: Mutex::new(HashMap::new()),
            screens_service,
            rt_handle: rt.handle().clone(),
            ui_ready,
            demo_mode: true,
            desktop_screens_watch: Mutex::new(None),
        });
        (inner, notices)
    }

    fn attention_events(notices: &Arc<Mutex<Vec<HubNotice>>>) -> Vec<(Target, String, String)> {
        notices
            .lock()
            .unwrap()
            .iter()
            .filter_map(|n| match n {
                HubNotice::Attention { target, title, body, .. } => Some((target.clone(), title.clone(), body.clone())),
                _ => None,
            })
            .collect()
    }

    fn attention_titles(notices: &Arc<Mutex<Vec<HubNotice>>>) -> Vec<(Target, String)> {
        attention_events(notices).into_iter().map(|(t, title, _)| (t, title)).collect()
    }

    fn mk_test_subagent(id: &str, parent_win: &str, minded_by: Option<&str>, shared_minder: bool, minds: Option<&str>, role: Option<&str>, name: Option<&str>) -> crate::subagents::NativeSubagent {
        crate::subagents::NativeSubagent {
            id: id.to_owned(),
            name: name.map(str::to_owned),
            host_key: "local".into(),
            session_id: "s1".into(),
            window_id: format!("win-{id}"),
            block_id: format!("blk-{id}"),
            parent_window_id: parent_win.to_owned(),
            parent_block_id: None,
            cli: crate::subagents::AgentCli::Agy,
            model: "m".into(),
            effort: "high".into(),
            title: name.unwrap_or(id).to_owned(),
            role: role.map(str::to_owned),
            cwd: None,
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: 1,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 0,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: None,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: minds.map(str::to_owned),
            minded_by: minded_by.map(str::to_owned),
            shared_minder,
        }
    }

    #[test]
    fn minded_subagent_notifications_suppressed_by_default_and_configurable() {
        let bk = |block_id: &str| BlockKey { host_key: "local".into(), session_id: "s1".into(), block_id: block_id.into() };
        let mut st = HubState::test_empty();
        st.agents_told.insert("local".into());

        // Service-managed shared minder under lead, with assigned worker and unassigned grandchild.
        st.subagents.register(mk_test_subagent("m-shared", "win-lead", None, true, None, Some("minder"), Some("shared")));
        st.subagents.register(mk_test_subagent("b-shared", "win-m-shared", Some("m-shared"), false, None, None, Some("builder-1")));
        st.subagents.register(mk_test_subagent("g-shared", "win-b-shared", None, false, None, None, Some("grandchild")));
        // Launcher-managed tab minder (`tools/dev/agents minder start`): strict default name AND custom `--name supervisor --role tab-minder`.
        st.subagents.register(mk_test_subagent("m-agy", "win-lead", None, false, None, None, Some("minder")));
        st.subagents.register(mk_test_subagent("b-agy", "win-m-agy", None, false, None, None, Some("builder-2")));
        st.subagents.register(mk_test_subagent("m-agy-2", "win-lead", None, false, None, None, Some("minder-2")));
        st.subagents.register(mk_test_subagent("b-agy-2", "win-m-agy-2", None, false, None, None, Some("builder-2b")));
        st.subagents.register(mk_test_subagent("m-super", "win-lead", None, false, None, Some("tab-minder"), Some("supervisor")));
        st.subagents.register(mk_test_subagent("b-super", "win-m-super", None, false, None, None, Some("builder-super")));
        // Explicit non-minder role (`--role reviewer`) overrides a title of "Minder" (Astra #4).
        let mut reviewer = mk_test_subagent("b-reviewer", "win-lead", None, false, None, Some("reviewer"), Some("review"));
        reviewer.title = "Minder".into();
        st.subagents.register(reviewer);
        st.subagents.register(mk_test_subagent("b-under-reviewer", "win-b-reviewer", None, false, None, None, Some("helper-rev")));
        // Nested minder under a tab minder: the nested minder's own failure MUST still notify.
        st.subagents.register(mk_test_subagent("m-nested", "win-b-super", None, true, None, Some("minder"), Some("nested-shared")));
        // Legacy 1:1 minder (`minds: Some("b-legacy")`).
        st.subagents.register(mk_test_subagent("b-legacy", "win-lead", None, false, None, None, Some("legacy-worker")));
        st.subagents.register(mk_test_subagent("m-legacy", "win-b-legacy", None, false, Some("b-legacy"), Some("minder"), Some("m-legacy")));
        // Exited minder + dangling/pruned minder ID: neither may suppress.
        let mut dead_minder = mk_test_subagent("m-dead", "win-lead", None, true, None, Some("minder"), Some("minder-old"));
        dead_minder.exited_at = Some(10);
        st.subagents.register(dead_minder);
        st.subagents.register(mk_test_subagent("b-orphaned", "win-m-dead", Some("m-dead"), false, None, None, Some("orphaned")));
        st.subagents.register(mk_test_subagent("b-dangling", "win-lead", Some("m-pruned-gone"), false, None, None, Some("dangling")));
        // Descriptive worker names/titles starting with or containing "minder" must NOT be mistaken for minders.
        st.subagents.register(mk_test_subagent("b-fix", "win-lead", None, false, None, None, Some("fix-minder")));
        st.subagents.register(mk_test_subagent("b-under-fix", "win-b-fix", None, false, None, None, Some("helper-1")));
        st.subagents.register(mk_test_subagent("b-prefix", "win-lead", None, false, None, None, Some("minder-fix")));
        st.subagents.register(mk_test_subagent("b-under-prefix", "win-b-prefix", None, false, None, None, Some("helper-2")));
        let mut titled_worker = mk_test_subagent("b-titled", "win-lead", None, false, None, None, Some("notes"));
        titled_worker.title = "Minder notifications".into();
        st.subagents.register(titled_worker);
        st.subagents.register(mk_test_subagent("b-under-titled", "win-b-titled", None, false, None, None, Some("helper-3")));

        // Positive cases: genuinely watched by a live minder.
        for id in ["blk-b-shared", "blk-b-agy", "blk-b-agy-2", "blk-b-super", "blk-b-legacy"] {
            assert!(st.is_minded_subagent(&bk(id)), "{id} should be minded");
            assert!(!st.allows_pane_notice(&bk(id)), "{id} should be suppressed by default");
        }
        // Negative cases: lead, unassigned shared-minder grandchild, explicit non-minder role with "Minder" title,
        // minders (including nested), exited/dangling minders, descriptive "minder-*" builders.
        for id in [
            "blk-lead",
            "blk-g-shared",
            "blk-b-reviewer",
            "blk-b-under-reviewer",
            "blk-m-shared",
            "blk-m-agy",
            "blk-m-super",
            "blk-m-nested",
            "blk-m-legacy",
            "blk-b-orphaned",
            "blk-b-dangling",
            "blk-b-fix",
            "blk-b-under-fix",
            "blk-b-prefix",
            "blk-b-under-prefix",
            "blk-b-titled",
            "blk-b-under-titled",
        ] {
            assert!(!st.is_minded_subagent(&bk(id)), "{id} must NOT be suppressed");
            assert!(st.allows_pane_notice(&bk(id)), "{id} should notify by default");
        }

        // Strict label checks.
        assert!(is_minder_label("minder") && is_minder_label("Minder 2") && is_minder_label("minder-2") && is_minder_label("minder2"));
        assert!(!is_minder_label("minder-fix") && !is_minder_label("minder-notes") && !is_minder_label("Minder notifications"));

        // Same `block_id` in another session (`s2`) MUST NOT match `s1`'s minded worker (Astra #7 / Neon #3).
        let other_session_bk = BlockKey { host_key: "local".into(), session_id: "s2".into(), block_id: "blk-b-shared".into() };
        assert!(!st.has_subagent_record(&other_session_bk));
        assert!(!st.is_minded_subagent(&other_session_bk));
        assert!(st.allows_pane_notice(&other_session_bk));

        // Independent agent in a sibling split pane of a minded worker's tab MUST still notify.
        let mk_block = |id: &str| BlockInfo {
            block_id: id.to_owned(),
            label: id.to_owned(),
            creator_name: "com.superlogical.terminal".into(),
            flavor: "com.superlogical.terminal".into(),
            host_id: "local".into(),
            owner: None,
        };
        let split_session = SessionView {
            revision: 1,
            session_id: "s1".into(),
            label: "work".into(),
            windows: vec![olo_rex_client::WindowView {
                window_id: "win-b-shared".into(),
                label: "builder-1".into(),
                focused_block_id: Some("blk-b-shared".into()),
                zoomed_block_id: None,
                layers: vec![olo_rex_client::LayerView {
                    layer_id: "layer-1".into(),
                    kind: "tiled".into(),
                    bounds: [0.0, 0.0, 1.0, 1.0],
                    state: Some("normal".into()),
                    layout: None,
                    blocks: vec![mk_block("blk-b-shared"), mk_block("blk-b-shared-split")],
                }],
            }],
            active_window_id: Some("win-b-shared".into()),
        };
        st.hosts.insert("local".into(), HostRuntime::test_connected("local", HashMap::from([("s1".into(), split_session)])));
        assert!(!st.is_minded_subagent(&bk("blk-b-shared-split")), "independent sibling split pane must not inherit minded suppression");

        // Disconnected agents follower (`!agents_told`): cached rows are unconfirmed and must NOT suppress.
        st.agents_told.remove("local");
        assert!(!st.is_minded_subagent(&bk("blk-b-shared")));
        assert!(st.allows_pane_notice(&bk("blk-b-shared")));
        st.agents_told.insert("local".into());

        // All-three-off consistency: `minded_subagents = true` with `blocked = done = failed = false` must NOT allow minded notices while UI checkbox is disabled.
        st.settings.notices = crate::settings::Notices { blocked: false, done: false, failed: false, minded_subagents: true };
        assert!(!st.allows_pane_notice(&bk("blk-b-shared")));
        // Enabling any trigger + `minded_subagents = true` allows minded notices.
        st.settings.notices.blocked = true;
        assert!(st.allows_pane_notice(&bk("blk-b-shared")));
    }

    #[test]
    fn minded_subagent_dispatch_detach_race_apc_and_bridge_end_to_end() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let bk = |block_id: &str| BlockKey { host_key: "local".into(), session_id: "s1".into(), block_id: block_id.into() };
        let mk_block = |id: &str| BlockInfo {
            block_id: id.to_owned(),
            label: id.to_owned(),
            creator_name: "com.superlogical.terminal".into(),
            flavor: "com.superlogical.terminal".into(),
            host_id: "local".into(),
            owner: None,
        };
        let session = SessionView {
            revision: 1,
            session_id: "s1".into(),
            label: "work".into(),
            windows: vec![olo_rex_client::WindowView {
                window_id: "win-1".into(),
                label: "work".into(),
                focused_block_id: Some("blk-lead".into()),
                zoomed_block_id: None,
                layers: vec![olo_rex_client::LayerView {
                    layer_id: "layer-1".into(),
                    kind: "tiled".into(),
                    bounds: [0.0, 0.0, 1.0, 1.0],
                    state: Some("normal".into()),
                    layout: None,
                    blocks: vec![mk_block("blk-lead"), mk_block("blk-b1"), mk_block("blk-fresh-minded"), mk_block("blk-fresh-unminded"), mk_block("blk-fresh-lead")],
                }],
            }],
            active_window_id: Some("win-1".into()),
        };
        let mut host = HostRuntime::test_connected("local", HashMap::from([("s1".into(), session)]));
        host.session_summaries.push(SessionSummary { session_id: "s1".into(), label: "work".into() });

        let mut st = HubState::test_empty();
        st.agents_told.insert("local".into());
        st.agents_follow_gen.insert("local".into(), 1);
        st.hosts.insert("local".into(), host);
        st.subagents.register(mk_test_subagent("m1", "win-lead", None, true, None, Some("minder"), Some("shared")));
        st.subagents.register(mk_test_subagent("b1", "win-m1", Some("m1"), false, None, None, Some("worker")));

        // Established blocks (older than NEW_PANE_OWNERSHIP_GRACE) vs brand-new blocks.
        let old_time = Instant::now() - Duration::from_secs(5);
        for id in ["blk-lead", "blk-b1"] {
            let mut br = BlockRuntime::new(80, 24, &TerminalTheme::default());
            br.created_at = old_time;
            st.blocks.insert(bk(id), Arc::new(Mutex::new(br)));
        }
        for id in ["blk-fresh-minded", "blk-fresh-unminded", "blk-fresh-lead"] {
            st.blocks.insert(bk(id), Arc::new(Mutex::new(BlockRuntime::new(80, 24, &TerminalTheme::default()))));
        }

        let state = Arc::new(Mutex::new(st));
        let (inner, notices) = test_inner_with_state(state.clone(), &rt);
        let status = |state: &str, msg: &str| StatusUpdate::parse("program_status_changed", &json!({ "id": "agy", "record": { "state": state, "msg": msg } })).unwrap();
        let agent_frame = |entry: crate::subagents::NativeSubagent| {
            serde_json::to_string(&olo_rex_link::agents::wire::Frame::Agent {
                agent: Box::new(olo_rex_link::agents::wire::AgentView { entry, state: olo_rex_link::agents::SubagentLiveState::Working, summary: None }),
            })
            .unwrap()
        };

        // 1. Minded worker blocks -> suppressed, unread/activity updated, `delivered_blocked_gen` left unset.
        let b1_br = state.lock().unwrap().blocks.get(&bk("blk-b1")).unwrap().clone();
        let cand = {
            let rules = state.lock().unwrap().settings.notices;
            let mut g = b1_br.lock().unwrap();
            let c = apply_program_status(&mut g, 1, status("blocked", "approve rm?"), rules);
            assert!(g.unread && g.agent_activity == AgentActivity::Blocked);
            c.unwrap()
        };
        inner.dispatch_pane_notice(&bk("blk-b1"), cand.2.to_owned(), cand.3, cand.4, Some(cand.0), Some(cand.1));
        assert!(attention_titles(&notices).is_empty(), "minded worker approval must be suppressed");

        // 2. Minder detaches while the approval is STILL open (`Frame::Agent` with `minded_by: None`) -> pending approval delivered ONCE!
        let frame = agent_frame(mk_test_subagent("b1", "win-lead", None, false, None, None, Some("worker")));
        inner.agents_frame_for_gen("local", 1, &frame);
        assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("blk-b1")), "work · Agent blocked".to_owned(), "approve rm?".to_owned())]);
        notices.lock().unwrap().clear();
        // A second ownership frame must NOT re-deliver the same approval.
        inner.agents_frame_for_gen("local", 1, &frame);
        assert!(attention_titles(&notices).is_empty());

        // Re-attach minder to b1 for APC & bridge tests.
        inner.agents_frame_for_gen("local", 1, &agent_frame(mk_test_subagent("b1", "win-m1", Some("m1"), false, None, None, Some("worker"))));

        // 3. `handle_inline_apc` (ChildExited & Unknown) suppressed for minded worker, delivered for lead, and still suppressed when all-three-off + minded_subagents=true.
        inner.handle_inline_apc(&bk("blk-b1"), InlineApc::ChildExited { exit_code: 0 });
        inner.handle_inline_apc(&bk("blk-b1"), InlineApc::Unknown(json!({ "title": "Done", "body": "ok" })));
        assert!(attention_titles(&notices).is_empty(), "minded APC notices suppressed by default");
        state.lock().unwrap().settings.notices = crate::settings::Notices { blocked: false, done: false, failed: false, minded_subagents: true };
        inner.handle_inline_apc(&bk("blk-b1"), InlineApc::ChildExited { exit_code: 1 });
        assert!(attention_titles(&notices).is_empty(), "all-three-off with minded_subagents=true still suppresses minded APC notices");
        state.lock().unwrap().settings.notices = crate::settings::Notices { blocked: true, done: false, failed: false, minded_subagents: true };
        inner.handle_inline_apc(&bk("blk-b1"), InlineApc::ChildExited { exit_code: 0 });
        assert_eq!(attention_titles(&notices), vec![(Target::Pane(bk("blk-b1")), "work · Process exited".to_owned())]);
        notices.lock().unwrap().clear();
        state.lock().unwrap().settings.notices = crate::settings::Notices::default();

        // 4. `bridge_notice`: pane-targeted respects minded suppression; session-only and host-only keep existing scope.
        inner.bridge_notice("local", Some("s1"), Some("blk-b1"), &json!({ "title": "Ping", "body": "hi" }));
        assert!(attention_titles(&notices).is_empty(), "pane-targeted bridge_notice suppressed for minded subagent");
        inner.bridge_notice("local", Some("s1"), None, &json!({ "title": "Session", "body": "hi" }));
        inner.bridge_notice("local", None, None, &json!({ "title": "Host", "body": "hi" }));
        assert_eq!(
            attention_titles(&notices),
            vec![(Target::Session { host_key: "local".into(), session_id: "s1".into() }, "work · Session".to_owned()), (Target::Host("local".into()), "local · Host".to_owned())]
        );
        notices.lock().unwrap().clear();
        state.lock().unwrap().settings.notices.minded_subagents = true;
        inner.bridge_notice("local", Some("s1"), Some("blk-b1"), &json!({ "title": "Ping", "body": "hi" }));
        assert_eq!(attention_titles(&notices), vec![(Target::Pane(bk("blk-b1")), "work · Ping".to_owned())]);
        notices.lock().unwrap().clear();
        state.lock().unwrap().settings.notices.minded_subagents = false;

        // 5. Newly started pane race (status BEFORE Frame::Agent):
        //    (a) Minded worker + titled inline APC during hold: APC must NOT erase the held approval;
        //        when Frame::Agent confirms minded both are suppressed; if minder then detaches while
        //        still blocked, the approval is still delivered once (Astra #1)!
        let _guard = rt.enter();
        {
            let (br, rules) = {
                let st = state.lock().unwrap();
                (st.blocks.get(&bk("blk-fresh-minded")).unwrap().clone(), st.settings.notices)
            };
            let c = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "early ask"), rules).unwrap();
            inner.dispatch_pane_notice(&bk("blk-fresh-minded"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
        }
        inner.handle_inline_apc(&bk("blk-fresh-minded"), InlineApc::Unknown(json!({ "title": "Progress", "body": "still thinking" })));
        assert!(attention_titles(&notices).is_empty(), "newly seen pane held awaiting ownership");
        inner.agents_frame_for_gen("local", 1, &agent_frame(mk_test_subagent("fresh-minded", "win-m1", Some("m1"), false, None, None, Some("fresh-minded"))));
        assert!(attention_titles(&notices).is_empty(), "held notices suppressed once Frame::Agent confirms minded");
        // Now minder detaches while `early ask` is still open -> approval MUST be delivered once!
        inner.agents_frame_for_gen("local", 1, &agent_frame(mk_test_subagent("fresh-minded", "win-lead", None, false, None, None, Some("fresh-minded"))));
        assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("blk-fresh-minded")), "work · Agent blocked".to_owned(), "early ask".to_owned())]);
        notices.lock().unwrap().clear();

        //    (b) Unminded worker: held while unresolved + APC during hold -> BOTH delivered when Frame::Agent confirms unminded!
        {
            let (br, rules) = {
                let st = state.lock().unwrap();
                (st.blocks.get(&bk("blk-fresh-unminded")).unwrap().clone(), st.settings.notices)
            };
            let c = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "unminded ask"), rules).unwrap();
            inner.dispatch_pane_notice(&bk("blk-fresh-unminded"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
        }
        inner.handle_inline_apc(&bk("blk-fresh-unminded"), InlineApc::Unknown(json!({ "title": "Note", "body": "extra" })));
        assert!(attention_titles(&notices).is_empty());
        inner.agents_frame_for_gen("local", 1, &agent_frame(mk_test_subagent("fresh-unminded", "win-lead", None, false, None, None, Some("fresh-unminded"))));
        assert_eq!(
            attention_events(&notices),
            vec![
                (Target::Pane(bk("blk-fresh-unminded")), "work · Agent blocked".to_owned(), "unminded ask".to_owned()),
                (Target::Pane(bk("blk-fresh-unminded")), "work · Note".to_owned(), "extra".to_owned()),
            ]
        );
        notices.lock().unwrap().clear();

        //    (c) Established lead pane (older than grace): emitted immediately without holding.
        inner.dispatch_pane_notice(&bk("blk-lead"), "Agent failed".into(), "boom".into(), olo_kit::notify::NoteKind::Error, Some(AgentActivity::Error), None);
        assert_eq!(attention_titles(&notices), vec![(Target::Pane(bk("blk-lead")), "work · Agent failed".to_owned())]);
    }

    #[test]
    fn minded_subagent_stale_blocked_snapshot_visibility_follower_and_timer_regressions() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let bk = |sid: &str, bid: &str| BlockKey { host_key: "local".into(), session_id: sid.into(), block_id: bid.into() };
        let mk_block = |id: &str| BlockInfo {
            block_id: id.to_owned(),
            label: id.to_owned(),
            creator_name: "com.superlogical.terminal".into(),
            flavor: "com.superlogical.terminal".into(),
            host_id: "local".into(),
            owner: None,
        };
        let mk_session = |sid: &str, label: &str, bids: &[&str]| SessionView {
            revision: 1,
            session_id: sid.into(),
            label: label.into(),
            windows: vec![olo_rex_client::WindowView {
                window_id: format!("win-{sid}"),
                label: label.into(),
                focused_block_id: bids.first().map(|s| (*s).to_owned()),
                zoomed_block_id: None,
                layers: vec![olo_rex_client::LayerView {
                    layer_id: "layer-1".into(),
                    kind: "tiled".into(),
                    bounds: [0.0, 0.0, 1.0, 1.0],
                    state: Some("normal".into()),
                    layout: None,
                    blocks: bids.iter().map(|b| mk_block(b)).collect(),
                }],
            }],
            active_window_id: Some(format!("win-{sid}")),
        };
        let s1 = mk_session("s1", "work", &["blk-w1", "blk-w2", "blk-w3", "blk-w4", "blk-tabw", "blk-vis", "blk-timer", "blk-same"]);
        let s2 = mk_session("s2", "other", &["blk-same"]);
        let mut host = HostRuntime::test_connected("local", HashMap::from([("s1".into(), s1), ("s2".into(), s2)]));
        host.session_summaries.push(SessionSummary { session_id: "s1".into(), label: "work".into() });
        host.session_summaries.push(SessionSummary { session_id: "s2".into(), label: "other".into() });

        let base = Instant::now();
        let old = base - Duration::from_secs(5);
        let mut st = HubState::test_empty();
        st.test_now = Some(base);
        st.agents_told.insert("local".into());
        st.agents_follow_gen.insert("local".into(), 2);
        st.hosts.insert("local".into(), host);
        st.subagents.register(mk_test_subagent("m1", "win-lead", None, true, None, Some("minder"), Some("shared")));
        st.subagents.register(mk_test_subagent("m-tab", "win-lead", None, false, None, Some("tab-minder"), Some("supervisor")));
        for id in ["w1", "w2", "w3", "w4"] {
            st.subagents.register(mk_test_subagent(id, "win-m1", Some("m1"), false, None, None, Some(id)));
        }
        st.subagents.register(mk_test_subagent("tabw", "win-m-tab", None, false, None, None, Some("tabw")));
        let mut same_in_s1 = mk_test_subagent("same", "win-m1", Some("m1"), false, None, None, Some("same"));
        same_in_s1.block_id = "blk-same".into();
        st.subagents.register(same_in_s1);

        for bid in ["blk-w1", "blk-w2", "blk-w3", "blk-w4", "blk-tabw", "blk-same"] {
            let mut br = BlockRuntime::new(80, 24, &TerminalTheme::default());
            br.created_at = old;
            st.blocks.insert(bk("s1", bid), Arc::new(Mutex::new(br)));
        }
        for (sid, bid) in [("s1", "blk-vis"), ("s1", "blk-timer"), ("s2", "blk-same")] {
            let mut br = BlockRuntime::new(80, 24, &TerminalTheme::default());
            // Remaining ownership grace = 40 ms so the real spawned tokio timer runs fast and deterministically.
            br.created_at = base - Duration::from_millis(710);
            st.blocks.insert(bk(sid, bid), Arc::new(Mutex::new(br)));
        }

        let state = Arc::new(Mutex::new(st));
        let (inner, notices) = test_inner_with_state(state.clone(), &rt);
        let status = |state: &str, msg: &str| StatusUpdate::parse("program_status_changed", &json!({ "id": "agy", "record": { "state": state, "msg": msg } })).unwrap();
        let clear_status = || StatusUpdate::parse("program_status_removed", &json!({ "ids": ["agy"] })).unwrap();
        let frame_with_state = |entry: crate::subagents::NativeSubagent, live: olo_rex_link::agents::SubagentLiveState| {
            serde_json::to_string(&olo_rex_link::agents::wire::Frame::Agent { agent: Box::new(olo_rex_link::agents::wire::AgentView { entry, state: live, summary: None }) }).unwrap()
        };
        let detach_frame = |id: &str| frame_with_state(mk_test_subagent(id, "win-lead", None, false, None, None, Some(id)), olo_rex_link::agents::SubagentLiveState::Working);

        // Astra #2a: Blocked("Approve A?") -> Working -> screen-guessed Blocked (no record) -> detach MUST NOT replay "Approve A?".
        {
            let (br, rules) = {
                let st = state.lock().unwrap();
                (st.blocks.get(&bk("s1", "blk-w1")).unwrap().clone(), st.settings.notices)
            };
            let c = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "Approve A?"), rules).unwrap();
            inner.dispatch_pane_notice(&bk("s1", "blk-w1"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
            let mut g = br.lock().unwrap();
            apply_program_status(&mut g, 2, clear_status(), rules);
            assert_ne!(g.agent_activity, AgentActivity::Blocked);
            // Simulate a screen-guessed Blocked with no program_status record.
            g.agent_activity = AgentActivity::Blocked;
        }
        inner.agents_frame_for_gen("local", 2, &detach_frame("w1"));
        assert!(attention_events(&notices).is_empty(), "screen-guessed Blocked without record must not replay stale Approve A?");

        // Astra #2b / Neon #1: Blocked("Approve A?") -> Working -> Blocked("Allow deploy?" while notify_blocked=false)
        // -> re-enable notify_blocked + detach MUST NOT replay "Allow deploy?" or "Approve A?".
        {
            let br = state.lock().unwrap().blocks.get(&bk("s1", "blk-w2")).unwrap().clone();
            let rules_on = state.lock().unwrap().settings.notices;
            let c = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "Approve A?"), rules_on).unwrap();
            inner.dispatch_pane_notice(&bk("s1", "blk-w2"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
            apply_program_status(&mut br.lock().unwrap(), 2, status("working", "running"), rules_on);
            state.lock().unwrap().settings.notices.blocked = false;
            let rules_off = state.lock().unwrap().settings.notices;
            assert!(apply_program_status(&mut br.lock().unwrap(), 3, status("blocked", "Allow deploy?"), rules_off).is_none());
            state.lock().unwrap().settings.notices.blocked = true;
        }
        inner.agents_frame_for_gen("local", 2, &detach_frame("w2"));
        assert!(attention_events(&notices).is_empty(), "approval that arrived while notify_blocked=false must never replay on detach");

        // Astra #2c: status_snapshot replacing "Approve A?" with "Approve B?" before detach delivers "Approve B?".
        {
            let (br, rules) = {
                let st = state.lock().unwrap();
                (st.blocks.get(&bk("s1", "blk-w3")).unwrap().clone(), st.settings.notices)
            };
            let c = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "Approve A?"), rules).unwrap();
            inner.dispatch_pane_notice(&bk("s1", "blk-w3"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
            let rec_b: ProgramStatusRecord = serde_json::from_value(json!({ "id": "agy", "state": "blocked", "msg": "Approve B?" })).unwrap();
            br.lock().unwrap().status_snapshot(Some(olo_rex_client::RpcResult { seq: 5, payload: vec![rec_b] }));
        }
        inner.agents_frame_for_gen("local", 2, &detach_frame("w3"));
        assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("s1", "blk-w3")), "work · Agent blocked".to_owned(), "Approve B?".to_owned())]);
        notices.lock().unwrap().clear();

        // Astra #2d: reload/attach status_snapshot restoring "Approve A?" on minded worker -> detach delivers "Approve A?".
        {
            let br = state.lock().unwrap().blocks.get(&bk("s1", "blk-w4")).unwrap().clone();
            let rec_a: ProgramStatusRecord = serde_json::from_value(json!({ "id": "agy", "state": "blocked", "msg": "Approve A?" })).unwrap();
            br.lock().unwrap().status_snapshot(Some(olo_rex_client::RpcResult { seq: 1, payload: vec![rec_a] }));
        }
        inner.agents_frame_for_gen("local", 2, &detach_frame("w4"));
        assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("s1", "blk-w4")), "work · Agent blocked".to_owned(), "Approve A?".to_owned())]);
        notices.lock().unwrap().clear();

        // Astra #6: Tab minder transitioning to SubagentLiveState::Error releases standing approval AND stops suppressing new ones.
        {
            let (br, rules) = {
                let st = state.lock().unwrap();
                (st.blocks.get(&bk("s1", "blk-tabw")).unwrap().clone(), st.settings.notices)
            };
            let c = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "tab approval 1"), rules).unwrap();
            inner.dispatch_pane_notice(&bk("s1", "blk-tabw"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
            assert!(attention_events(&notices).is_empty(), "live tab minder suppresses worker");
        }
        let err_minder = frame_with_state(mk_test_subagent("m-tab", "win-lead", None, false, None, Some("tab-minder"), Some("supervisor")), olo_rex_link::agents::SubagentLiveState::Error);
        inner.agents_frame_for_gen("local", 2, &err_minder);
        assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("s1", "blk-tabw")), "work · Agent blocked".to_owned(), "tab approval 1".to_owned())]);
        notices.lock().unwrap().clear();
        {
            let (br, rules) = {
                let st = state.lock().unwrap();
                (st.blocks.get(&bk("s1", "blk-tabw")).unwrap().clone(), st.settings.notices)
            };
            let c = apply_program_status(&mut br.lock().unwrap(), 2, status("blocked", "tab approval 2"), rules).unwrap();
            inner.dispatch_pane_notice(&bk("s1", "blk-tabw"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
        }
        assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("s1", "blk-tabw")), "work · Agent blocked".to_owned(), "tab approval 2".to_owned())]);
        notices.lock().unwrap().clear();

        // Astra #5 / Neon #2: Superseded follower (gen 1) cannot strip or mutate gen 2's `agents_told`.
        assert!(!inner.agents_follower_ended("local", 1), "stale follower gen must be ignored");
        assert!(state.lock().unwrap().agents_told.contains("local"));
        inner.agents_frame_for_gen("local", 1, &detach_frame("same"));
        assert!(state.lock().unwrap().is_minded_subagent(&bk("s1", "blk-same")), "stale follower frame must be ignored");

        // Astra #7 / Neon #3: Same block_id in session s2 is NOT satisfied by s1's subagent row, and holds then notifies.
        rt.block_on(async {
            // Astra #8: Pane visible at event time is discarded; pane viewed during hold discards held notice even after switching away.
            let win = crate::ui::WinId(1);
            inner.set_window_front_pane(win, Some(bk("s1", "blk-vis")));
            inner.dispatch_pane_notice(&bk("s1", "blk-vis"), "Agent finished".into(), "done".into(), olo_kit::notify::NoteKind::Information, Some(AgentActivity::Done), None);
            assert!(attention_events(&notices).is_empty(), "visible pane notice discarded immediately");
            inner.set_window_front_pane(win, None);
            // Now send while NOT visible (held for 40 ms), view pane mid-hold, then switch away before timer fires.
            inner.dispatch_pane_notice(&bk("s1", "blk-vis"), "Agent finished".into(), "done 2".into(), olo_kit::notify::NoteKind::Information, Some(AgentActivity::Done), None);
            assert!(state.lock().unwrap().held_pane_notices.contains_key(&bk("s1", "blk-vis")));
            inner.set_window_front_pane(win, Some(bk("s1", "blk-vis")));
            assert!(!state.lock().unwrap().held_pane_notices.contains_key(&bk("s1", "blk-vis")), "viewing pane clears held notices");
            inner.set_window_front_pane(win, None);
            tokio::time::sleep(Duration::from_millis(80)).await;
            assert!(attention_events(&notices).is_empty(), "cleared held notice must stay discarded after switching away");

            // Astra #9 + #7: Drive the REAL spawned fallback timer callback through its deadline without manual flush,
            // and verify a superseded timer seq cannot flush a newer held notice.
            {
                let (br, rules) = {
                    let st = state.lock().unwrap();
                    (st.blocks.get(&bk("s1", "blk-timer")).unwrap().clone(), st.settings.notices)
                };
                let c1 = apply_program_status(&mut br.lock().unwrap(), 1, status("blocked", "ask 1"), rules).unwrap();
                inner.dispatch_pane_notice(&bk("s1", "blk-timer"), c1.2.to_owned(), c1.3, c1.4, Some(c1.0), Some(c1.1));
            }
            let seq1 = state.lock().unwrap().held_pane_notices.get(&bk("s1", "blk-timer")).unwrap()[0].seq;
            {
                let (br, rules) = {
                    let st = state.lock().unwrap();
                    (st.blocks.get(&bk("s1", "blk-timer")).unwrap().clone(), st.settings.notices)
                };
                let c2 = apply_program_status(&mut br.lock().unwrap(), 2, status("blocked", "ask 2"), rules).unwrap();
                inner.dispatch_pane_notice(&bk("s1", "blk-timer"), c2.2.to_owned(), c2.3, c2.4, Some(c2.0), Some(c2.1));
            }
            // Stale seq1 callback must NOT flush seq2!
            inner.flush_held_pane_notice(&bk("s1", "blk-timer"), Some(seq1));
            assert!(attention_events(&notices).is_empty(), "stale timer seq must not flush newer held notice");
            // Also dispatch on s2/blk-same (fresh pane, NOT matched by s1/blk-same!).
            inner.dispatch_pane_notice(&bk("s2", "blk-same"), "Agent finished".into(), "s2 done".into(), olo_kit::notify::NoteKind::Information, Some(AgentActivity::Done), None);
            assert!(attention_events(&notices).is_empty(), "held notices must remain held immediately after dispatch");

            // Wait for the real spawned timers (~40 ms) to fire on the runtime.
            let deadline = Instant::now() + Duration::from_secs(2);
            while attention_events(&notices).len() < 2 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let got = attention_events(&notices);
            assert_eq!(got.len(), 2, "real spawned timers must flush both held notices at deadline");
            assert!(got.contains(&(Target::Pane(bk("s1", "blk-timer")), "work · Agent blocked".to_owned(), "ask 2".to_owned())));
            assert!(got.contains(&(Target::Pane(bk("s2", "blk-same")), "other · Agent finished".to_owned(), "s2 done".to_owned())));
            notices.lock().unwrap().clear();

            // Astra #5 / Neon #2: Fresh follower awaiting initial Frame::Snapshot holds minded worker's notice,
            // then suppresses it once Frame::Snapshot arrives (or releases immediately on follower end).
            let gen3 = inner.invalidate_agents_follower("local", true);
            assert!(state.lock().unwrap().agents_snapshot_pending.contains_key("local"));
            {
                let (br, rules) = {
                    let st = state.lock().unwrap();
                    (st.blocks.get(&bk("s1", "blk-same")).unwrap().clone(), st.settings.notices)
                };
                let c = apply_program_status(&mut br.lock().unwrap(), 10, status("blocked", "during reconnect"), rules).unwrap();
                inner.dispatch_pane_notice(&bk("s1", "blk-same"), c.2.to_owned(), c.3, c.4, Some(c.0), Some(c.1));
            }
            assert!(attention_events(&notices).is_empty(), "notice held while initial Frame::Snapshot pending");
            let mut same_entry = mk_test_subagent("same", "win-m1", Some("m1"), false, None, None, Some("same"));
            same_entry.block_id = "blk-same".into();
            let snap = serde_json::to_string(&olo_rex_link::agents::wire::Frame::Snapshot {
                proto: 1,
                cursor: 1,
                agents: vec![
                    olo_rex_link::agents::wire::AgentView {
                        entry: mk_test_subagent("m1", "win-lead", None, true, None, Some("minder"), Some("shared")),
                        state: olo_rex_link::agents::SubagentLiveState::Working,
                        summary: None,
                    },
                    olo_rex_link::agents::wire::AgentView { entry: same_entry, state: olo_rex_link::agents::SubagentLiveState::WaitingApproval, summary: None },
                ],
            })
            .unwrap();
            inner.agents_frame_for_gen("local", gen3, &snap);
            assert!(!state.lock().unwrap().agents_snapshot_pending.contains_key("local"));
            assert!(state.lock().unwrap().agents_told.contains("local"));
            assert!(attention_events(&notices).is_empty(), "Frame::Snapshot suppresses minded worker held during reconnect");

            // Follower ends (current gen3) -> releases open approval immediately!
            assert!(inner.agents_follower_ended("local", gen3));
            assert_eq!(attention_events(&notices), vec![(Target::Pane(bk("s1", "blk-same")), "work · Agent blocked".to_owned(), "during reconnect".to_owned())]);
        });
    }

    #[test]
    fn r3_polish_astra_f1_host_live_pin_and_host_matches_pin_reject_wildcards_and_replacements() {
        let mut hosts: HashMap<String, HostRuntime> = HashMap::new();
        let mut local = HostRuntime::test_connected("local", HashMap::new());
        local.is_local = true;
        local.host_instance_id = 11;
        local.screens_conn_id = 0;
        hosts.insert("local".into(), local);

        let mut remote = HostRuntime::test_connected("dev-box", HashMap::new());
        remote.is_local = false;
        remote.host_instance_id = 22;
        remote.screens_conn_id = 7;
        hosts.insert("dev-box".into(), remote);

        let mut offline = HostRuntime::test_connected("offline-box", HashMap::new());
        offline.is_local = false;
        offline.host_instance_id = 33;
        offline.screens_conn_id = 0;
        hosts.insert("offline-box".into(), offline);

        // Live pin capture:
        assert_eq!(host_live_pin_in(&hosts, "local"), Some((11, 0)));
        assert_eq!(host_live_pin_in(&hosts, "dev-box"), Some((22, 7)));
        assert_eq!(host_live_pin_in(&hosts, "offline-box"), None, "disconnected remote host must not yield wildcard pin");
        assert_eq!(host_live_pin_in(&hosts, "missing"), None);

        // Pin matching:
        assert!(host_matches_pin_in(&hosts, "local", 11, 0));
        assert!(host_matches_pin_in(&hosts, "dev-box", 22, 7));
        assert!(!host_matches_pin_in(&hosts, "dev-box", 0, 0), "zero instance wildcard must be rejected");
        assert!(!host_matches_pin_in(&hosts, "dev-box", 22, 0), "zero remote conn_id wildcard must be rejected");
        assert!(!host_matches_pin_in(&hosts, "dev-box", 22, 6), "stale remote conn_id after reconnect must be rejected");
        assert!(!host_matches_pin_in(&hosts, "dev-box", 21, 7), "stale host_instance_id after host replacement must be rejected");
        assert!(!host_matches_pin_in(&hosts, "offline-box", 33, 0), "disconnected remote host must not match");
    }
}

fn host_live_pin_in(hosts: &HashMap<String, HostRuntime>, host_key: &str) -> Option<(u64, u64)> {
    let h = hosts.get(host_key)?;
    if h.host_instance_id == 0 {
        return None;
    }
    if !h.is_local && h.screens_conn_id == 0 {
        return None;
    }
    Some((h.host_instance_id, h.screens_conn_id))
}

fn host_matches_pin_in(hosts: &HashMap<String, HostRuntime>, host_key: &str, host_instance_id: u64, conn_id: u64) -> bool {
    if host_instance_id == 0 {
        return false;
    }
    hosts.get(host_key).is_some_and(|h| h.host_instance_id == host_instance_id && (h.is_local || (conn_id != 0 && h.screens_conn_id == conn_id)))
}

/// Merge live follower rows and install metadata ON TOP of probe-owned machine
/// fields (`chrome`, `missing`, `older`, `software`, `bytes`) ONLY when a real
/// follower snapshot arrived for `expected_conn_id` (or `expected_conn_id == 0`).
pub(crate) fn merge_live_screens_into_there(there: &mut crate::host_screens::ScreensThere, live: &crate::screens_sock::LiveHostScreens, expected_conn_id: u64) -> bool {
    if !live.has_follow_snapshot || (expected_conn_id != 0 && live.conn_id != expected_conn_id) {
        return false;
    }
    let Some(status) = live.status.as_ref() else { return false };
    let rows = crate::screens_sock::rex_screens_to_rows(&live.screens);
    let running: Vec<String> = rows.iter().filter(|r| r.running).map(|r| r.name.clone()).collect();
    let changed =
        !there.helper || there.installed != status.installed || there.committed != status.committed || there.on_host != status.default_on_host || there.running != running || there.rows != rows;
    if changed {
        there.helper = true;
        there.installed = status.installed.clone();
        there.committed = status.committed;
        there.on_host = status.default_on_host;
        there.running = running;
        there.rows = rows;
    }
    changed
}

#[cfg(test)]
mod agents_tests;
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
