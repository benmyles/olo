//! Complete typed data models for Rex API revision 19.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The API revision this client speaks (matches installed Rex server rev 19).
pub const REX_API_REVISION: u32 = 19;
/// Minimum API revision we can interoperate with gracefully.
pub const REX_API_MIN_REVISION: u32 = 1;
/// Control plane WebSocket subprotocol.
pub const CONTROL_SUBPROTOCOL: &str = "rex.control.v1";
/// Terminal binary stream WebSocket subprotocol.
pub const DATA_TERMINAL_SUBPROTOCOL: &str = "rex.data.terminal.v1";
/// Bidirectional raw bytes WebSocket subprotocol (uploads, downloads, tunnels).
pub const DATA_BYTES_SUBPROTOCOL: &str = "rex.data.bytes.v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ClientIdentity {
    pub fn olo_rex(label: Option<String>) -> Self {
        Self { kind: Some("app".to_owned()), platform: Some("linux".to_owned()), name: Some("Olo Rex".to_owned()), version: Some(env!("CARGO_PKG_VERSION").to_owned()), label }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WelcomeLimits {
    pub max_message_bytes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WelcomeClient {
    pub id: String,
    pub reconnect_token: String,
    #[serde(default)]
    pub resumed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CarrierInfo {
    pub carrier: String,
    pub port: u16,
    #[serde(default)]
    pub alpn: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WelcomeMessage {
    pub protocol: String,
    pub limits: WelcomeLimits,
    #[serde(default)]
    pub carriers: Option<Vec<CarrierInfo>>,
    #[serde(default)]
    pub client: Option<WelcomeClient>,
    #[serde(default)]
    pub features: Option<Vec<String>>,
    #[serde(default)]
    pub api_revision: u32,
    #[serde(default)]
    pub api_server_revision: u32,
    #[serde(default)]
    pub api_min_revision: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Principal {
    pub kind: String,
    pub transport: String,
    pub display: String,
    #[serde(default)]
    pub remote_addr: Option<String>,
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub gid: Option<u32>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub sid: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub login_name: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectedClient {
    pub client_id: String,
    #[serde(default)]
    pub info: Option<ClientIdentity>,
    pub principal: Principal,
    #[serde(default)]
    pub connected_at: String,
    #[serde(default)]
    pub connection_state: Option<String>,
    #[serde(default)]
    pub connection_lost_at: Option<String>,
    #[serde(default)]
    pub session_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerConfigInfo {
    pub generation: u64,
    #[serde(default)]
    pub hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListenerInfo {
    pub endpoint: String,
    #[serde(default)]
    pub addresses: Option<Vec<String>>,
    pub transport: String,
    #[serde(default)]
    pub admission: Option<String>,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailscaleAccount {
    pub login_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailscaleHttps {
    pub state: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailscaleStatus {
    #[serde(default)]
    pub error: Option<String>,
    pub state: String,
    #[serde(default)]
    pub dns_name: Option<String>,
    #[serde(default)]
    pub auth_url: Option<String>,
    #[serde(default)]
    pub account: Option<TailscaleAccount>,
    #[serde(default)]
    pub tailnet_name: Option<String>,
    #[serde(default)]
    pub allow_tailnet_members: bool,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub https: Option<TailscaleHttps>,
    #[serde(default)]
    pub control_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerStatus {
    pub pid: u32,
    pub instance_id: String,
    #[serde(default)]
    pub host_id: Option<String>,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub exit_empty: bool,
    #[serde(default)]
    pub session_count: usize,
    #[serde(default)]
    pub tailscale: Option<TailscaleStatus>,
    #[serde(default)]
    pub api_revision: u32,
    #[serde(default)]
    pub api_min_revision: u32,
    #[serde(default)]
    pub config: ServerConfigInfo,
    #[serde(default)]
    pub listeners: Option<Vec<ListenerInfo>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerVersion {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub build_date: String,
    #[serde(default)]
    pub api_revision: u32,
    #[serde(default)]
    pub api_min_revision: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostsListResponse {
    #[serde(default)]
    pub hosts: Vec<HostRecord>,
    #[serde(default)]
    pub hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
    #[serde(other)]
    Other,
}

impl SplitDirection {
    pub fn is_horizontal(self) -> bool {
        !matches!(self, Self::Vertical)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitNode {
    pub split_id: String,
    pub direction: SplitDirection,
    pub ratio: f64,
    pub before: Box<LayoutNode>,
    pub after: Box<LayoutNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutNode {
    #[serde(default)]
    pub rect: [f64; 4],
    #[serde(default)]
    pub block_id: Option<String>,
    #[serde(default)]
    pub split: Option<SplitNode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockInfo {
    pub block_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub creator_name: String,
    #[serde(default)]
    pub flavor: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerKind {
    #[default]
    Tiled,
    Floating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerState {
    #[default]
    Normal,
    Minimized,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayerView {
    pub layer_id: String,
    pub kind: String,
    #[serde(default)]
    pub bounds: [f64; 4],
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub layout: Option<LayoutNode>,
    #[serde(default)]
    pub blocks: Vec<BlockInfo>,
}

impl LayerView {
    pub fn layer_kind(&self) -> LayerKind {
        if self.kind == "tiled" { LayerKind::Tiled } else { LayerKind::Floating }
    }

    pub fn layer_state(&self) -> LayerState {
        if self.state.as_deref() == Some("minimized") { LayerState::Minimized } else { LayerState::Normal }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowView {
    pub window_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub focused_block_id: Option<String>,
    #[serde(default)]
    pub zoomed_block_id: Option<String>,
    #[serde(default)]
    pub layers: Vec<LayerView>,
}

impl WindowView {
    pub fn find_block(&self, block_id: &str) -> Option<&BlockInfo> {
        self.layers.iter().flat_map(|l| l.blocks.iter()).find(|b| b.block_id == block_id)
    }

    pub fn all_blocks(&self) -> impl Iterator<Item = &BlockInfo> {
        self.layers.iter().flat_map(|l| l.blocks.iter())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionView {
    pub revision: u64,
    pub session_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub active_window_id: Option<String>,
    #[serde(default)]
    pub windows: Vec<WindowView>,
}

impl SessionView {
    pub fn active_window(&self) -> Option<&WindowView> {
        let id = self.active_window_id.as_deref()?;
        self.windows.iter().find(|w| w.window_id == id)
    }

    pub fn find_block(&self, block_id: &str) -> Option<(&WindowView, &LayerView, &BlockInfo)> {
        for w in &self.windows {
            for l in &w.layers {
                if let Some(b) = l.blocks.iter().find(|b| b.block_id == block_id) {
                    return Some((w, l, b));
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTicketRequest {
    pub creator_name: String,
    pub data_ref_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subprotocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTicket {
    pub session_id: String,
    pub block_id: String,
    pub data_ref_id: String,
    #[serde(default)]
    pub subprotocol: Option<String>,
    pub ticket: String,
    pub expires_in_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttachAllPayload {
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedSessionEntry {
    pub session: SessionSummary,
    pub view: SessionView,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub client_id: String,
    pub server_status: ServerStatus,
    pub attach_all: AttachAllPayload,
    #[serde(default)]
    pub selected_session: Option<AttachedSessionEntry>,
    #[serde(default)]
    pub attached_sessions: Option<Vec<AttachedSessionEntry>>,
    #[serde(default)]
    pub data_tickets: Option<Vec<DataTicket>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachResponse {
    pub client_id: String,
    pub view: SessionView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataConnectResponse {
    pub ticket: String,
    pub expires_in_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEvent {
    pub key: String,
    pub mods: String,
    pub consumed_mods: String,
    pub text: String,
    pub unshifted_codepoint: u32,
    pub composing: bool,
    pub action: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapAction {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeats: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapBinding {
    pub keys: Vec<String>,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapRemoval {
    pub keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapLayer {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<KeymapBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removals: Vec<KeymapRemoval>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapDiagnostic {
    pub severity: String,
    #[serde(default)]
    pub source: Option<String>,
    pub message: String,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapMode {
    pub name: String,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default)]
    pub blocked: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapConfigInfo {
    pub path: String,
    #[serde(default)]
    pub exists: bool,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub instance_id: String,
    #[serde(default)]
    pub hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeymapResponse {
    #[serde(default)]
    pub bindings: Vec<KeymapBinding>,
    #[serde(default)]
    pub diagnostics: Vec<KeymapDiagnostic>,
    #[serde(default)]
    pub actions: Option<Vec<KeymapAction>>,
    #[serde(default)]
    pub modes: Option<Vec<KeymapMode>>,
    pub config: KeymapConfigInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub selected: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptSpec {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub allow_other: bool,
    #[serde(default)]
    pub items: Option<Vec<PromptItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPromptRequested {
    pub call_id: String,
    #[serde(default)]
    pub action: Option<String>,
    pub prompt: PromptSpec,
    #[serde(default)]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPromptEnded {
    pub call_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPerformRequested {
    pub call_id: String,
    pub action: String,
    #[serde(default)]
    pub args: Option<Value>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub block_id: Option<String>,
    pub requester: RequesterInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequesterInfo {
    pub client_id: String,
    #[serde(default)]
    pub info: Option<ClientIdentity>,
    pub principal: Principal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCallRequested {
    pub call_id: String,
    pub server: String,
    pub method: String,
    #[serde(default)]
    pub args: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessOwner {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgramStatusRecord {
    #[serde(default)]
    pub id: Option<String>,
    pub state: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub effective_app: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProgramPriority {
    Idle = 0,
    Done = 1,
    Working = 2,
    Error = 3,
    Blocked = 4,
}

impl ProgramStatusRecord {
    pub fn priority(&self) -> ProgramPriority {
        match self.state.as_str() {
            "blocked" => ProgramPriority::Blocked,
            "error" => ProgramPriority::Error,
            "working" => ProgramPriority::Working,
            "done" => ProgramPriority::Done,
            _ => ProgramPriority::Idle,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    #[serde(default)]
    pub user_id: u32,
    #[serde(default)]
    pub user: Option<String>,
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub invoked_path: Option<String>,
    #[serde(default)]
    pub argv0: Option<String>,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

impl ProcessInfo {
    pub fn display_label(&self) -> &str {
        self.script.as_deref().or(self.argv0.as_deref()).filter(|s| !s.is_empty()).unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LastExit {
    pub exit_code: i32,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TerminalProcessSnapshot {
    #[serde(default)]
    pub last_exit: Option<LastExit>,
    #[serde(default)]
    pub child: Option<ProcessInfo>,
    #[serde(default)]
    pub foreground: Option<ProcessInfo>,
    #[serde(default)]
    pub ancestors: Option<Vec<ProcessInfo>>,
    #[serde(default)]
    pub home: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortEntry {
    pub port: u16,
    pub address: String,
    pub protocol: String,
    #[serde(default)]
    pub first_seen_at: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PortsSnapshot {
    pub status: String,
    #[serde(default)]
    pub ports: Vec<PortEntry>,
    #[serde(default)]
    pub processes: Vec<ProcessInfo>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub id: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputMode {
    pub echo: bool,
    pub canonical: bool,
    pub signals: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    #[serde(default)]
    pub symlink: bool,
    #[serde(default)]
    pub file: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,
    #[serde(default)]
    pub home: String,
    pub status: String,
    #[serde(default)]
    pub entries: Vec<DirEntry>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadBeginResponse {
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub expires_in_ms: Option<u64>,
    #[serde(default)]
    pub not_modified: bool,
    pub path: String,
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub modified_at: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UploadBeginResponse {
    pub ticket: String,
    pub expires_in_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub flavor: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub client_address: Option<String>,
    #[serde(default)]
    pub client_target: Option<String>,
    #[serde(default)]
    pub last_owner: Option<String>,
    #[serde(default)]
    pub connections: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlobReadResponse {
    pub id: u64,
    pub data: String,
    pub total: usize,
    pub eof: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionResult {
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub client_actions: Vec<QueuedClientAction>,
    #[serde(default)]
    pub ordered_after: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueuedClientAction {
    pub action: String,
    #[serde(default)]
    pub args: Option<Value>,
}

/// Inline Rex APC control messages carried inside `rex.data.terminal.v1`:
/// `ESC _ rex1;<decimal_nonce>;<json> ESC \`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineApc {
    Resize { cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32, owner: Option<String> },
    Theme { foreground: Option<String>, background: Option<String>, cursor: Option<String>, palette: Vec<String>, scheme: Option<String> },
    ChildExited { exit_code: i32 },
    Unknown(Value),
}

/// Top-level event envelope arriving on `rex.control.v1`.
#[derive(Debug, Clone)]
pub struct ControlEventEnvelope {
    pub seq: u64,
    pub event: ControlEvent,
}

/// Parsed server, session, and block events from `rex.control.v1`.
#[derive(Debug, Clone)]
pub enum ControlEvent {
    SessionCreated(SessionSummary),
    SessionDestroyed { session_id: String },
    SessionMoved { session_id: String, before_session_id: Option<String> },
    SessionViewChanged { session_id: String, revision: u64 },
    SessionLabelChanged { session_id: String, label: String },
    ActiveWindowChanged { session_id: String, window_id: Option<String> },
    WindowCreated { session_id: String, window_id: String },
    WindowClosed { session_id: String, window_id: String },
    WindowLabelChanged { session_id: String, window_id: String, label: String },
    BlockCreated { session_id: String, block_id: String },
    BlockClosed { session_id: String, block_id: String },
    BlockOwnerChanged { session_id: String, block_id: String, owner: Option<String> },
    BlockEvent { session_id: String, block_id: String, creator_name: String, name: String, payload: Value },
    HostsChanged { hash: String },
    KeymapChanged { generation: u64, hash: String },
    TailscaleStatusChanged(TailscaleStatus),
    ClientPromptRequested(ClientPromptRequested),
    ClientPromptEnded(ClientPromptEnded),
    ClientPerformRequested(ClientPerformRequested),
    ClientCallRequested(ClientCallRequested),
    ClientJoined(ConnectedClient),
    ClientChanged(ConnectedClient),
    ClientLeft { client_id: String },
    EventDropped { event_type: String, session_id: Option<String>, block_id: Option<String> },
    Unknown { kind: String, payload: Value },
}

impl ControlEvent {
    pub fn parse(kind: &str, payload: Value) -> Self {
        match kind {
            "session_created" => {
                if let Some(s) = payload.get("session").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                    return Self::SessionCreated(s);
                }
            }
            "session_destroyed" => {
                if let Some(id) = payload.get("session_id").and_then(Value::as_str) {
                    return Self::SessionDestroyed { session_id: id.to_owned() };
                }
            }
            "session_moved" => {
                if let Some(id) = payload.get("session_id").and_then(Value::as_str) {
                    return Self::SessionMoved { session_id: id.to_owned(), before_session_id: payload.get("before_session_id").and_then(Value::as_str).map(str::to_owned) };
                }
            }
            "hosts_changed" => {
                return Self::HostsChanged { hash: payload.get("hash").and_then(Value::as_str).unwrap_or_default().to_owned() };
            }
            "keymap_changed" => {
                return Self::KeymapChanged {
                    generation: payload.get("generation").and_then(Value::as_u64).unwrap_or(0),
                    hash: payload.get("hash").and_then(Value::as_str).unwrap_or_default().to_owned(),
                };
            }
            "tailscale_status_changed" => {
                if let Ok(ts) = serde_json::from_value(payload.clone()) {
                    return Self::TailscaleStatusChanged(ts);
                }
            }
            "client_prompt_requested" => {
                if let Ok(req) = serde_json::from_value(payload.clone()) {
                    return Self::ClientPromptRequested(req);
                }
            }
            "client_prompt_ended" => {
                if let Ok(end) = serde_json::from_value(payload.clone()) {
                    return Self::ClientPromptEnded(end);
                }
            }
            "client_perform_requested" => {
                if let Ok(req) = serde_json::from_value(payload.clone()) {
                    return Self::ClientPerformRequested(req);
                }
            }
            "client_call_requested" => {
                if let Ok(req) = serde_json::from_value(payload.clone()) {
                    return Self::ClientCallRequested(req);
                }
            }
            "client_joined" => {
                if let Some(c) = payload.get("client").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                    return Self::ClientJoined(c);
                }
            }
            "client_changed" => {
                if let Some(c) = payload.get("client").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                    return Self::ClientChanged(c);
                }
            }
            "client_left" => {
                if let Some(id) = payload.get("client_id").and_then(Value::as_str) {
                    return Self::ClientLeft { client_id: id.to_owned() };
                }
            }
            "event_dropped" => {
                return Self::EventDropped {
                    event_type: payload.get("event_type").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    session_id: payload.get("session_id").and_then(Value::as_str).map(str::to_owned),
                    block_id: payload.get("block_id").and_then(Value::as_str).map(str::to_owned),
                };
            }
            "session_event" => {
                let sid = payload.get("session_id").and_then(Value::as_str).unwrap_or_default().to_owned();
                if let Some(inner) = payload.get("event") {
                    let itype = inner.get("type").and_then(Value::as_str).unwrap_or_default();
                    let ip = inner.get("payload").cloned().unwrap_or(Value::Null);
                    return Self::parse_session_event(sid, itype, ip);
                }
            }
            _ => {}
        }
        Self::Unknown { kind: kind.to_owned(), payload }
    }

    fn parse_session_event(session_id: String, kind: &str, payload: Value) -> Self {
        match kind {
            "session_view_changed" => Self::SessionViewChanged { session_id, revision: payload.get("revision").and_then(Value::as_u64).unwrap_or(0) },
            "session_label_changed" => Self::SessionLabelChanged { session_id, label: payload.get("label").and_then(Value::as_str).unwrap_or_default().to_owned() },
            "active_window_changed" => Self::ActiveWindowChanged { session_id, window_id: payload.get("window_id").and_then(Value::as_str).map(str::to_owned) },
            "window_created" => Self::WindowCreated { session_id, window_id: payload.get("window_id").and_then(Value::as_str).unwrap_or_default().to_owned() },
            "window_closed" => Self::WindowClosed { session_id, window_id: payload.get("window_id").and_then(Value::as_str).unwrap_or_default().to_owned() },
            "window_label_changed" => Self::WindowLabelChanged {
                session_id,
                window_id: payload.get("window_id").and_then(Value::as_str).unwrap_or_default().to_owned(),
                label: payload.get("label").and_then(Value::as_str).unwrap_or_default().to_owned(),
            },
            "block_created" => Self::BlockCreated { session_id, block_id: payload.get("block_id").and_then(Value::as_str).unwrap_or_default().to_owned() },
            "block_closed" => Self::BlockClosed { session_id, block_id: payload.get("block_id").and_then(Value::as_str).unwrap_or_default().to_owned() },
            "block_owner_changed" => Self::BlockOwnerChanged {
                session_id,
                block_id: payload.get("block_id").and_then(Value::as_str).unwrap_or_default().to_owned(),
                owner: payload.get("owner").and_then(Value::as_str).map(str::to_owned),
            },
            "block_event" => {
                let block_id = payload.get("block_id").and_then(Value::as_str).unwrap_or_default().to_owned();
                let creator_name = payload.get("creator_name").or_else(|| payload.get("creator")).and_then(Value::as_str).unwrap_or_default().to_owned();
                let name = payload.get("event").or_else(|| payload.get("name")).and_then(Value::as_str).unwrap_or_default().to_owned();
                let inner = payload.get("payload").or_else(|| payload.get("data")).cloned().unwrap_or(Value::Null);
                Self::BlockEvent { session_id, block_id, creator_name, name, payload: inner }
            }
            _ => Self::Unknown { kind: format!("session_event.{kind}"), payload },
        }
    }
}
