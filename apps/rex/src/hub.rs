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
    time::{Duration, Instant},
};

use olo_rex_client::{
    BlockInfo, BytesDataStream, ClientPromptRequested, ControlClient, ControlEvent, DATA_TERMINAL_SUBPROTOCOL, DataTicketRequest, DemuxItem, DirListing, HostRecord, InlineApc, InlineApcDemuxer,
    KeyEvent as RexKeyEvent, KeymapBinding, KeymapDiagnostic, KeymapMode, LocalForwardHandle, PortsSnapshot, ProgramStatusRecord, RexEndpoint, ServerStatus, ServerVersion, SessionSummary,
    SessionView, SshAskpass, SshSessionPool, TailscaleStatus, TerminalDataStream, TerminalProcessSnapshot, TerminalStreamFrame, start_local_forward,
};
use olo_rex_vt::{GhosttyTerminal, RenderState, TerminalMemoryUsage, TerminalSearch, TerminalTheme, try_decode_snapshot_prefix};
use olo_sync::Mutex;
use serde_json::{Value, json};

use crate::{
    agent::{AgentActivity, AutoApproveState, KeepGoingState, detect_agent_from_screen},
    bridge::{self, BridgeServerHandle},
    copy_mode::CopyMode,
    git::{GitCache, GitInfo},
    keymap,
    settings::{SavedHost, Settings},
    themes,
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
    PromptRequested { host_key: String, req: ClientPromptRequested },
    /// A `rex.ui` custom prompt was cancelled/answered on another device.
    PromptEnded { host_key: String, call_id: String },
    /// Server requested this client perform an action (`rex perform` / Lua `client:perform`).
    PerformRequested { host_key: String, action: String, args: Option<Value>, session_id: Option<String> },
    /// In-app floating notification toast.
    Toast { title: String, body: String, host_key: Option<String>, session_id: Option<String>, block_id: Option<String> },
    /// Quick Look download finished and is ready to display.
    QuickLookReady { path: String, bytes: Vec<u8>, content_type: Option<String> },
    /// Directory listing result for Path Browser / Go to Directory.
    DirListingReady { host_key: String, block_id: String, listing: DirListing },
    /// Clipboard text requested by `olo.clipboard.set`.
    SetClipboard(String),
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

/// Live terminal + metadata runtime for one Rex block.
pub struct BlockRuntime {
    pub creator_name: String,
    pub terminal: GhosttyTerminal,
    pub render_state: RenderState,
    pub search: TerminalSearch,
    pub copy_mode: Option<CopyMode>,
    pub title: String,
    pub pwd: String,
    pub program_statuses: Vec<ProgramStatusRecord>,
    pub ports: PortsSnapshot,
    pub process: Option<TerminalProcessSnapshot>,
    pub agent_activity: AgentActivity,
    pub keep_going: Option<KeepGoingState>,
    pub auto_approve: Option<AutoApproveState>,
    pub unread: bool,
    pub unread_cleared_at: Option<Instant>,
    pub muted: bool,
    pub last_bell: Option<Instant>,
    pub data_connected: bool,
    pub received_bytes: u64,
    pub upload_progress: Option<(String, f32)>,
    pub password_prompt: bool,
    pub data_cancel: Option<tokio::sync::oneshot::Sender<()>>,
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
            search,
            copy_mode: None,
            title: String::new(),
            pwd: String::new(),
            program_statuses: Vec::new(),
            ports: PortsSnapshot::default(),
            process: None,
            agent_activity: AgentActivity::Idle,
            keep_going: None,
            auto_approve: None,
            unread: false,
            unread_cleared_at: None,
            muted: false,
            last_bell: None,
            data_connected: false,
            received_bytes: 0,
            upload_progress: None,
            password_prompt: false,
            data_cancel: None,
        }
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

/// Ordered terminal input operation serialized strictly FIFO per host so fast typing never reorders.
enum HostInputOp {
    Write { session_id: String, block_id: String, text: String },
    Key { session_id: String, block_id: String, event: RexKeyEvent },
}

/// Active `-L` port forward entry shown in Hosts & Port Forwards.
pub struct ActiveForward {
    pub id: String,
    pub host_key: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_addr: SocketAddr,
    pub handle: Option<LocalForwardHandle>,
}

impl Drop for ActiveForward {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.stop();
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
    pub client_id: Option<String>,
    pub reconnect_token: Option<String>,
    pub server_status: Option<ServerStatus>,
    pub server_version: Option<ServerVersion>,
    pub tailscale: Option<TailscaleStatus>,
    pub synced_hosts: Vec<HostRecord>,
    pub session_summaries: Vec<SessionSummary>,
    pub sessions: HashMap<String, SessionView>,
    pub keymap_bindings: Vec<KeymapBinding>,
    pub keymap_modes: Vec<KeymapMode>,
    pub keymap_diagnostics: Vec<KeymapDiagnostic>,
    pub control: Option<ControlClient>,
    input_tx: Option<tokio::sync::mpsc::UnboundedSender<HostInputOp>>,
    pub bridge_server: Option<BridgeServerHandle>,
    pub bridge_remote_port: Option<u16>,
}

struct QuickLookCacheEntry {
    version: Option<String>,
    bytes: Vec<u8>,
    content_type: Option<String>,
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

impl RexHub {
    pub fn start(settings: Settings, active_theme: TerminalTheme, askpass: Option<Arc<dyn SshAskpass>>, notify: impl Fn(HubNotice) + Send + Sync + 'static) -> Arc<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("rex-io").enable_all().build().expect("create Rex tokio runtime");

        let local_bin_dir = bridge::ensure_local_bin_dir();
        let ssh_pool = Arc::new(SshSessionPool::default());
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
                label: if home_is_local { "Local".to_owned() } else { home_addr.clone() },
                endpoint_raw: home_addr,
                endpoint: None,
                is_local: home_is_local,
                conn_state: HostConnState::Disconnected,
                client_id: None,
                reconnect_token: None,
                server_status: None,
                server_version: None,
                tailscale: None,
                synced_hosts: Vec::new(),
                session_summaries: Vec::new(),
                sessions: HashMap::new(),
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
                        client_id: None,
                        reconnect_token: None,
                        server_status: None,
                        server_version: None,
                        tailscale: None,
                        synced_hosts: Vec::new(),
                        session_summaries: Vec::new(),
                        sessions: HashMap::new(),
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
                blocks: vec![mk_block("demo-build", "cargo test — olo-rex"), mk_block("demo-agent", "claude — Refactor VT renderer"), mk_block("demo-shell", "zsh — ~/olo/rex-client")],
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
        br_build.title = "cargo test — olo-rex".to_owned();
        br_build.pwd = "/home/ben/olo/rex-client".to_owned();
        br_build.data_connected = true;
        br_build.terminal.vt_write(
            concat!(
                "\x1b[1;36mben@olo\x1b[0m \x1b[1;34m~/olo/rex\x1b[0m \x1b[33m(rex-client)\x1b[0m % cargo test -p olo-rex\r\n",
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
                "\x1b[1;36mben@olo\x1b[0m \x1b[1;34m~/olo/rex\x1b[0m % "
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-build".into() }, Arc::new(Mutex::new(br_build)));

        let mut br_agent = BlockRuntime::new(56, 18, &theme);
        br_agent.title = "claude — Refactor VT renderer".to_owned();
        br_agent.pwd = "/home/ben/olo/rex-client".to_owned();
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
        br_shell.title = "zsh — ~/olo/rex-client".to_owned();
        br_shell.pwd = "/home/ben/olo/rex-client".to_owned();
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
                "\x1b[1;36mben@olo\x1b[0m \x1b[1;34m~/olo/rex\x1b[0m % git status -sb\r\n",
                "\x1b[32m## rex-client...origin/main\x1b[0m\r\n",
                " \x1b[32mA\x1b[0m  apps/rex/src/terminal_pane.rs\r\n",
                " \x1b[32mA\x1b[0m  crates/rex-vt/src/snapshot.rs\r\n",
                " \x1b[33mM\x1b[0m  crates/protocol/src/glance.rs\r\n",
                "Listening: \x1b[4;34mhttp://localhost:3000\x1b[0m\r\n"
            )
            .as_bytes(),
        );
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-shell".into() }, Arc::new(Mutex::new(br_shell)));

        let mut br_logs = BlockRuntime::new(110, 32, &theme);
        br_logs.title = "rex server logs".to_owned();
        br_logs.pwd = "/home/ben".to_owned();
        br_logs.data_connected = true;
        br_logs.terminal.vt_write(b"\x1b[2m2026-11-21T14:00:01Z\x1b[0m \x1b[32mINFO\x1b[0m rex_server: listening on unix:///run/user/1000/rex/server.sock (rev 19)\r\n");
        st.blocks.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-logs".into() }, Arc::new(Mutex::new(br_logs)));

        // Second demo host (remote SSH dev server, shown in sidebar).
        st.hosts.insert(
            "dev-box".to_owned(),
            HostRuntime {
                key: "dev-box".to_owned(),
                label: "dev-box.ts.net".to_owned(),
                endpoint_raw: "ssh://ben@dev-box.ts.net".to_owned(),
                endpoint: None,
                is_local: false,
                conn_state: HostConnState::Connected,
                client_id: None,
                reconnect_token: None,
                server_status: None,
                server_version: None,
                tailscale: None,
                synced_hosts: Vec::new(),
                session_summaries: vec![SessionSummary { session_id: "remote-api".to_owned(), label: "api-prod".to_owned() }],
                sessions: HashMap::new(),
                keymap_bindings: Vec::new(),
                keymap_modes: Vec::new(),
                keymap_diagnostics: Vec::new(),
                control: None,
                input_tx: None,
                bridge_server: None,
                bridge_remote_port: None,
            },
        );
        st.host_order.push("dev-box".to_owned());
    }

    pub fn connect_host(&self, host_key: &str) {
        let key = host_key.to_owned();
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            inner.run_host_connection(key).await;
        });
    }

    pub fn add_and_connect_host(&self, id: String, label: String, endpoint: String, save_remote: bool) {
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
                    client_id: None,
                    reconnect_token: None,
                    server_status: None,
                    server_version: None,
                    tailscale: None,
                    synced_hosts: Vec::new(),
                    session_summaries: Vec::new(),
                    sessions: HashMap::new(),
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
        if save_remote {
            let rec = HostRecord { id: Some(id.clone()), label, endpoints: vec![endpoint], ..Default::default() };
            self.with_control("local", move |ctrl, _, _| async move {
                let _ = ctrl.hosts_put(&rec).await;
            });
        }
        self.connect_host(&id);
    }

    pub fn remove_host(&self, host_key: &str) {
        if host_key == "local" {
            return;
        }
        let key = host_key.to_owned();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.remove(&key);
            st.host_order.retain(|k| k != &key);
            st.blocks.retain(|bk, _| bk.host_key != key);
            st.settings.saved_hosts.retain(|h| h.id != key);
            st.settings.save();
        }
        let id_copy = key.clone();
        self.with_control("local", move |ctrl, _, _| async move {
            let _ = ctrl.hosts_remove(&id_copy).await;
        });
        (self.notify)(HubNotice::StateChanged);
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
            let _ = guard.terminal.apply_theme(&new_theme);
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

    fn with_control<F, Fut>(&self, host_key: &str, f: F)
    where
        F: FnOnce(ControlClient, RexEndpoint, Arc<RexHubInner>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let found = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| Some((h.control.clone()?, h.endpoint.clone()?)))
        };
        let Some((ctrl, ep)) = found else { return };
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            f(ctrl, ep, inner).await;
        });
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

    pub fn create_session(&self, host_key: &str, label: Option<String>, cwd: Option<String>) {
        let hk = host_key.to_owned();
        let opts = self.local_shell_opts(host_key, cwd);
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            match ctrl.session_create(label.as_deref(), opts).await {
                Ok(res) => {
                    if let Some(sid) = res.payload.get("session_id").and_then(Value::as_str)
                        && let Ok(att) = ctrl.session_attach(sid).await
                    {
                        inner.ingest_session(&hk, &ctrl, &ep, att.payload.view, &[]).await;
                    }
                }
                Err(e) => (inner.notify)(HubNotice::Toast { title: "Could not create session".into(), body: e.to_string(), host_key: Some(hk), session_id: None, block_id: None }),
            }
        });
    }

    #[allow(dead_code)]
    pub fn rename_session(&self, host_key: &str, session_id: &str, label: String) {
        let sid = session_id.to_owned();
        self.with_control(host_key, move |ctrl, _, _| async move {
            let _ = ctrl.session_set_label(&sid, &label).await;
        });
    }

    pub fn close_session(&self, host_key: &str, session_id: &str) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        self.with_control(host_key, move |ctrl, _, inner| async move {
            let _ = ctrl.session_destroy(&sid).await;
            {
                let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(h) = st.hosts.get_mut(&hk) {
                    h.sessions.remove(&sid);
                    h.session_summaries.retain(|s| s.session_id != sid);
                }
                st.blocks.retain(|bk, _| !(bk.host_key == hk && bk.session_id == sid));
            }
            (inner.notify)(HubNotice::StateChanged);
        });
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
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let wid = window_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_focus_window(&sid, &wid).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
    }

    #[allow(dead_code)]
    pub fn rename_window(&self, host_key: &str, session_id: &str, window_id: &str, title: String) {
        let sid = session_id.to_owned();
        let wid = window_id.to_owned();
        self.with_control(host_key, move |ctrl, _, _| async move {
            let _ = ctrl.session_set_window_label(&sid, &wid, &title).await;
        });
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

    #[allow(dead_code)]
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
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        let bid = block_id.to_owned();
        let bk = BlockKey { host_key: hk.clone(), session_id: sid.clone(), block_id: bid.clone() };
        if let Some(br) = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if g.unread {
                g.unread = false;
                g.unread_cleared_at = Some(Instant::now());
            }
        }
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            if ctrl.session_focus_block(&sid, &bid).await.is_ok()
                && let Ok(v) = ctrl.session_view(&sid).await
            {
                inner.ingest_session(&hk, &ctrl, &ep, v.payload, &[]).await;
            }
        });
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

    pub fn send_input(&self, key: &BlockKey, data: Vec<u8>) {
        let tx = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&key.host_key).and_then(|h| h.input_tx.clone());
        if let Some(tx) = tx {
            let text = String::from_utf8_lossy(&data).into_owned();
            let _ = tx.send(HostInputOp::Write { session_id: key.session_id.clone(), block_id: key.block_id.clone(), text });
        }
    }

    pub fn send_rex_key(&self, key: &BlockKey, event: RexKeyEvent) {
        let tx = self.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&key.host_key).and_then(|h| h.input_tx.clone());
        if let Some(tx) = tx {
            let _ = tx.send(HostInputOp::Key { session_id: key.session_id.clone(), block_id: key.block_id.clone(), event });
        }
    }

    pub fn resize_block(&self, key: &BlockKey, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        let br_opt = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned();
        let changed = if let Some(br) = br_opt {
            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
            if g.terminal.size() != (cols, rows) {
                let _ = g.terminal.resize(cols, rows, cell_w, cell_h);
                true
            } else {
                false
            }
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

    pub fn reload_server_config(&self, host_key: &str) {
        self.with_control(host_key, |ctrl, _, inner| async move {
            match ctrl.server_config_reload().await {
                Ok(_) => (inner.notify)(HubNotice::Toast {
                    title: "Rex configuration reloaded".into(),
                    body: "Server Lua configuration and keymaps are up to date.".into(),
                    host_key: None,
                    session_id: None,
                    block_id: None,
                }),
                Err(e) => (inner.notify)(HubNotice::Toast { title: "Reload failed".into(), body: e.to_string(), host_key: None, session_id: None, block_id: None }),
            }
        });
    }

    pub fn request_dir_listing(&self, key: &BlockKey, path: String, files: bool) {
        let hk = key.host_key.clone();
        let sid = key.session_id.clone();
        let bid = key.block_id.clone();
        self.with_control(&key.host_key, move |ctrl, _, inner| async move {
            if let Ok(res) = ctrl.terminal_list_dir(&sid, &bid, &path, None, files).await {
                (inner.notify)(HubNotice::DirListingReady { host_key: hk, block_id: bid, listing: res.payload });
            }
        });
    }

    /// Download a file via `com.superlogical.terminal.download_begin` + `rex.data.bytes.v1`
    /// with `if_none_match` ETag caching for instant Quick Look (`⌘Y`).
    pub fn quick_look_path(&self, key: &BlockKey, path: String) {
        let hk = key.host_key.clone();
        let sid = key.session_id.clone();
        let bid = key.block_id.clone();
        let cache_key = (hk.clone(), bid.clone(), path.clone());
        let cached_version = self.state.lock().unwrap_or_else(|e| e.into_inner()).quick_look_cache.get(&cache_key).and_then(|e| e.version.clone());

        self.with_control(&key.host_key, move |ctrl, ep, inner| async move {
            let data_ref = format!("dl:{}", uuid::Uuid::new_v4().simple());
            match ctrl.terminal_download_begin(&sid, &bid, &data_ref, &path, cached_version.as_deref()).await {
                Ok(res) => {
                    let resp = res.payload;
                    if resp.not_modified {
                        if let Some(entry) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).quick_look_cache.get(&cache_key) {
                            (inner.notify)(HubNotice::QuickLookReady { path, bytes: entry.bytes.clone(), content_type: entry.content_type.clone() });
                        }
                        return;
                    }
                    let Some(ticket) = resp.ticket else { return };
                    if let Ok((raw_stream, host_hdr)) = ep.connect_stream().await
                        && let Ok(mut ds) = BytesDataStream::connect_hello(raw_stream, &host_hdr, &ticket).await
                    {
                        let mut bytes = Vec::new();
                        while let Ok(Some(chunk)) = ds.read_chunk().await {
                            bytes.extend_from_slice(&chunk);
                        }
                        inner
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .quick_look_cache
                            .insert(cache_key, QuickLookCacheEntry { version: resp.version, bytes: bytes.clone(), content_type: None });
                        (inner.notify)(HubNotice::QuickLookReady { path, bytes, content_type: None });
                    }
                }
                Err(e) => (inner.notify)(HubNotice::Toast { title: "Quick Look failed".into(), body: e.to_string(), host_key: Some(hk), session_id: Some(sid), block_id: Some(bid) }),
            }
        });
    }

    /// Upload dropped files into a remote Rex block via `upload_begin` + `rex.data.bytes.v1`
    /// (0-byte half-close EOF triggers Rex's automatic path paste!), or shell-escape paths directly on Local.
    pub fn upload_files(&self, key: &BlockKey, paths: Vec<PathBuf>) {
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
                let Ok(bytes) = tokio::fs::read(&path).await else { continue };
                let size = bytes.len() as u64;
                let data_ref = format!("ul:{}", uuid::Uuid::new_v4().simple());
                let batch_info = if total > 1 { Some((batch_id.as_str(), idx, total)) } else { None };
                if let Ok(res) = ctrl.terminal_upload_begin(&bk.session_id, &bk.block_id, &data_ref, &filename, size, batch_info).await
                    && let Ok((raw_stream, host_hdr)) = ep.connect_stream().await
                    && let Ok(mut ds) = BytesDataStream::connect_hello(raw_stream, &host_hdr, &res.payload.ticket).await
                {
                    let mut sent = 0usize;
                    for chunk in bytes.chunks(65_536) {
                        if ds.write_all(chunk).await.is_err() {
                            break;
                        }
                        sent += chunk.len();
                        let frac = if size == 0 { 1.0 } else { sent as f32 / size as f32 };
                        if let Some(br) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                            br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress = Some((filename.clone(), frac));
                        }
                        (inner.notify)(HubNotice::StateChanged);
                    }
                    let _ = ds.finish_write().await;
                }
            }
            if let Some(br) = inner.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                br.lock().unwrap_or_else(|e| e.into_inner()).upload_progress = None;
            }
            (inner.notify)(HubNotice::StateChanged);
        });
    }

    pub fn add_local_forward(&self, host_key: &str, session_id: &str, remote_host: String, remote_port: u16, local_port: u16, open_in_browser: bool) {
        let hk = host_key.to_owned();
        let sid = session_id.to_owned();
        self.with_control(host_key, move |ctrl, ep, inner| async move {
            match start_local_forward(ctrl, ep, sid.clone(), remote_host.clone(), remote_port, local_port).await {
                Ok(handle) => {
                    let local_addr = handle.local_addr;
                    let fwd = ActiveForward { id: uuid::Uuid::new_v4().to_string(), host_key: hk.clone(), remote_host, remote_port, local_addr, handle: Some(handle) };
                    inner.state.lock().unwrap_or_else(|e| e.into_inner()).forwards.push(fwd);
                    (inner.notify)(HubNotice::StateChanged);
                    if open_in_browser {
                        let url = format!("http://127.0.0.1:{}", local_addr.port());
                        let _ = bridge::open_url_with_autoforward(&url, false, None).await;
                    }
                }
                Err(e) => (inner.notify)(HubNotice::Toast { title: "Port forward failed".into(), body: e.to_string(), host_key: Some(hk), session_id: None, block_id: None }),
            }
        });
    }

    pub fn remove_forward(&self, forward_id: &str) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.forwards.retain(|f| f.id != forward_id);
        drop(st);
        (self.notify)(HubNotice::StateChanged);
    }

    pub fn open_link_from_pane(&self, key: &BlockKey, url: String) {
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
                (inner.notify)(HubNotice::Toast { title: "Could not open link".into(), body: e, host_key: Some(bk.host_key), session_id: Some(bk.session_id), block_id: Some(bk.block_id) });
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

    pub fn toggle_tailscale(&self, host_key: &str, enable: bool) {
        let hk = host_key.to_owned();
        self.with_control(host_key, move |ctrl, _, inner| async move {
            let res = if enable { ctrl.tailscale_enable(None, Some(true), None, None).await } else { ctrl.tailscale_disable().await };
            match res {
                Ok(r) => {
                    let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(h) = st.hosts.get_mut(&hk) {
                        h.tailscale = Some(r.payload);
                    }
                    drop(st);
                    (inner.notify)(HubNotice::StateChanged);
                }
                Err(e) => (inner.notify)(HubNotice::Toast { title: "Tailscale update failed".into(), body: e.to_string(), host_key: Some(hk), session_id: None, block_id: None }),
            }
        });
    }
}

impl RexHubInner {
    async fn ensure_ephemeral_forward(&self, host_key: &str, session_id: &str, remote_host: &str, remote_port: u16) -> Result<u16, String> {
        {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(f) = st.forwards.iter().find(|f| f.host_key == host_key && f.remote_host == remote_host && f.remote_port == remote_port) {
                return Ok(f.local_addr.port());
            }
        }
        let (ctrl, ep) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| Some((h.control.clone()?, h.endpoint.clone()?))).ok_or_else(|| format!("host '{host_key}' is not connected"))?
        };
        let handle = start_local_forward(ctrl, ep, session_id.to_owned(), remote_host.to_owned(), remote_port, 0).await.map_err(|e| e.to_string())?;
        let port = handle.local_addr.port();
        self.state.lock().unwrap_or_else(|e| e.into_inner()).forwards.push(ActiveForward {
            id: uuid::Uuid::new_v4().to_string(),
            host_key: host_key.to_owned(),
            remote_host: remote_host.to_owned(),
            remote_port,
            local_addr: handle.local_addr,
            handle: Some(handle),
        });
        (self.notify)(HubNotice::StateChanged);
        Ok(port)
    }

    async fn run_host_connection(self: Arc<Self>, host_key: String) {
        let mut attempt = 0u32;
        loop {
            let (endpoint_raw, reconnect_token, is_local, local_bin, user_overrides) = {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let overrides = st.settings.keymap_overrides.clone();
                let local_bin = st.local_bin_dir.clone();
                let Some(h) = st.hosts.get_mut(&host_key) else { return };
                h.conn_state = if attempt == 0 { HostConnState::Connecting } else { HostConnState::Reconnecting { attempt, last_error: "Reconnecting…".to_owned() } };
                (h.endpoint_raw.clone(), h.reconnect_token.clone(), h.is_local, local_bin, overrides)
            };
            (self.notify)(HubNotice::StateChanged);

            let endpoint = match RexEndpoint::parse(&endpoint_raw, self.ssh_pool.clone(), self.askpass.clone()) {
                Ok(ep) => ep,
                Err(e) => {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(h) = st.hosts.get_mut(&host_key) {
                        h.conn_state = HostConnState::Error(e.to_string());
                    }
                    drop(st);
                    (self.notify)(HubNotice::StateChanged);
                    return;
                }
            };

            let connected = async {
                let (stream, host_hdr) = endpoint.connect_stream().await?;
                let label = std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
                ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(label), reconnect_token.as_deref()).await
            }
            .await;

            match connected {
                Ok((ctrl, mut events)) => {
                    attempt = 0;
                    let client_id = ctrl.client_id().map(str::to_owned);
                    let token = ctrl.reconnect_token().map(str::to_owned);

                    // Fetch server status, version, synced hosts, keymap, and bootstrap sessions!
                    let status = ctrl.server_status().await.ok().map(|r| r.payload);
                    let version = ctrl.server_version().await.ok().map(|r| r.payload);
                    let hosts_resp = ctrl.hosts_list().await.ok().map(|r| r.payload);
                    let km_actions = keymap::register_actions_payload();
                    let km_defaults = [keymap::default_client_layer()];
                    let km_resp = ctrl.client_keymap(&km_actions, &km_defaults, &user_overrides).await.ok().map(|r| r.payload);

                    let ticket_req =
                        [DataTicketRequest { creator_name: "com.superlogical.terminal".to_owned(), data_ref_id: "terminal".to_owned(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.to_owned()) }];
                    let bootstrap = ctrl.session_bootstrap("all", None, None, &ticket_req).await.ok().map(|r| r.payload);

                    let mut summaries = bootstrap.as_ref().map(|b| b.attach_all.sessions.clone()).unwrap_or_default();
                    let mut initial_views: Vec<SessionView> = Vec::new();
                    let pre_tickets = bootstrap.as_ref().and_then(|b| b.data_tickets.clone()).unwrap_or_default();

                    if let Some(b) = &bootstrap {
                        if let Some(att) = &b.attached_sessions {
                            for e in att {
                                initial_views.push(e.view.clone());
                            }
                        } else if let Some(sel) = &b.selected_session {
                            initial_views.push(sel.view.clone());
                        }
                    }

                    // If local server has no sessions yet, create a ready-to-use `main` session!
                    if summaries.is_empty() && is_local {
                        let mut opts = json!({});
                        if let Some(bin) = &local_bin {
                            opts["path_prepend"] = json!([bin.to_string_lossy()]);
                        }
                        if let Some(open_bin) = olo_xdg::find_program("olo-open") {
                            opts["env"] = json!({ "BROWSER": open_bin.to_string_lossy() });
                        }
                        if let Ok(created) = ctrl.session_create(Some("main"), Some(opts)).await
                            && let Some(sid) = created.payload.get("session_id").and_then(Value::as_str)
                            && let Ok(att) = ctrl.session_attach(sid).await
                        {
                            summaries.push(SessionSummary { session_id: att.payload.view.session_id.clone(), label: att.payload.view.label.clone() });
                            initial_views.push(att.payload.view);
                        }
                    }

                    // Spawn one strictly-FIFO input writer task for this host connection so rapid
                    // keystrokes can never race and arrive out of order on `terminal.write`.
                    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<HostInputOp>();
                    let ctrl_in = ctrl.clone();
                    tokio::spawn(async move {
                        while let Some(op) = in_rx.recv().await {
                            match op {
                                HostInputOp::Write { session_id, block_id, text } => {
                                    let _ = ctrl_in.terminal_write(&session_id, &block_id, &text).await;
                                }
                                HostInputOp::Key { session_id, block_id, event } => {
                                    let _ = ctrl_in.client_key(&session_id, Some(&block_id), &event, None).await;
                                }
                            }
                        }
                    });

                    {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key) {
                            h.conn_state = HostConnState::Connected;
                            h.endpoint = Some(endpoint.clone());
                            h.client_id = client_id;
                            h.reconnect_token = token;
                            h.tailscale = status.as_ref().and_then(|s| s.tailscale.clone());
                            h.server_status = status;
                            h.server_version = version;
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
                        }
                    }

                    for view in initial_views {
                        self.ingest_session(&host_key, &ctrl, &endpoint, view, &pre_tickets).await;
                    }
                    self.write_palette_index();

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
                            if let Ok(handle) = bridge::start_bridge_server(hlabel, pol_fn, fwd_fn).await {
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

                    while let Some(env) = events.recv().await {
                        self.handle_control_event(&host_key, &ctrl, &endpoint, env.event).await;
                    }

                    {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key) {
                            h.control = None;
                            h.input_tx = None;
                            h.conn_state = HostConnState::Reconnecting { attempt: 1, last_error: "Connection closed".to_owned() };
                        } else {
                            return;
                        }
                    }
                    (self.notify)(HubNotice::StateChanged);
                }
                Err(e) => {
                    attempt += 1;
                    let still_exists = {
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(h) = st.hosts.get_mut(&host_key) {
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
            tokio::time::sleep(backoff).await;
        }
    }

    async fn ingest_session(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint, view: SessionView, pre_tickets: &[olo_rex_client::DataTicket]) {
        let session_id = view.session_id.clone();
        let blocks: Vec<BlockInfo> = view.windows.iter().flat_map(|w| w.all_blocks()).cloned().collect();

        let theme = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let theme = st.active_theme.clone();
            if let Some(h) = st.hosts.get_mut(host_key) {
                if !h.session_summaries.iter().any(|s| s.session_id == session_id) {
                    h.session_summaries.push(SessionSummary { session_id: session_id.clone(), label: view.label.clone() });
                }
                h.sessions.insert(session_id.clone(), view);
            }
            theme
        };

        let mut to_attach = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let live_ids: HashSet<String> = blocks.iter().map(|b| b.block_id.clone()).collect();
            st.blocks.retain(|bk, _| !(bk.host_key == host_key && bk.session_id == session_id && !live_ids.contains(&bk.block_id)));

            for b in &blocks {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.clone(), block_id: b.block_id.clone() };
                let is_terminal = b.creator_name == "com.superlogical.terminal" || b.flavor.starts_with("com.superlogical.terminal");
                if let Some(existing) = st.blocks.get(&bk) {
                    let mut g = existing.lock().unwrap_or_else(|e| e.into_inner());
                    if !b.label.is_empty() && g.title.is_empty() {
                        g.title = b.label.clone();
                    }
                    if !g.data_connected && is_terminal {
                        let ticket = pre_tickets.iter().find(|t| t.session_id == session_id && t.block_id == b.block_id).map(|t| t.ticket.clone());
                        to_attach.push((bk, existing.clone(), ticket));
                    }
                } else {
                    let mut br = BlockRuntime::new(120, 32, &theme);
                    br.creator_name = b.creator_name.clone();
                    br.title = b.label.clone();
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
                if let Ok(r) = c_meta.terminal_program_status(&bk_meta.session_id, &bk_meta.block_id).await {
                    let mut g = br_meta.lock().unwrap_or_else(|e| e.into_inner());
                    g.program_statuses = r.payload;
                    g.recompute_activity();
                }
                if let Ok(r) = c_meta.terminal_ports(&bk_meta.session_id, &bk_meta.block_id).await {
                    br_meta.lock().unwrap_or_else(|e| e.into_inner()).ports = r.payload;
                }
                if let Ok(r) = c_meta.terminal_process(&bk_meta.session_id, &bk_meta.block_id).await {
                    let mut g = br_meta.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(cwd) = r.payload.foreground.as_ref().and_then(|p| p.cwd.as_ref()).or_else(|| r.payload.child.as_ref().and_then(|p| p.cwd.as_ref())) {
                        g.pwd = cwd.clone();
                    }
                    g.process = Some(r.payload);
                    g.recompute_activity();
                }
                notify_meta(HubNotice::StateChanged);
            });

            let mut snapshot_buf: Vec<u8> = Vec::new();
            let mut snapshot_done = false;
            let mut apc = InlineApcDemuxer::new(stream.welcome.channel_nonce);

            loop {
                tokio::select! {
                    _ = &mut rx => {
                        stream.close_clean().await;
                        break;
                    }
                    res = stream.next_frame() => {
                        let Ok(TerminalStreamFrame::Bytes(chunk)) = res else { break };
                        if chunk.is_empty() {
                            continue;
                        }
                        if !snapshot_done {
                            snapshot_buf.extend_from_slice(&chunk);
                            if snapshot_buf.len() >= 8 && &snapshot_buf[..8] == b"GHOSTSNP" {
                                match try_decode_snapshot_prefix(&snapshot_buf, true, None) {
                                    Ok(Some(res)) => {
                                        let mut new_term = res.terminal;
                                        let consumed = res.consumed;
                                        let theme = inner.state.lock().unwrap_or_else(|e| e.into_inner()).active_theme.clone();
                                        let _ = new_term.apply_theme(&theme);
                                        let trailing = &snapshot_buf[consumed..];
                                        if !trailing.is_empty() {
                                            for item in apc.push(trailing) {
                                                match item {
                                                    DemuxItem::Vt(b) => new_term.vt_write(&b),
                                                    DemuxItem::Apc(a) => inner.handle_inline_apc(&bk, a),
                                                }
                                            }
                                        }
                                        {
                                            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                                            g.terminal = new_term;
                                            g.received_bytes += snapshot_buf.len() as u64;
                                            g.recompute_activity();
                                        }
                                        snapshot_buf.clear();
                                        snapshot_done = true;
                                        (inner.notify)(HubNotice::BlockOutput(bk.clone()));
                                    }
                                    Ok(None) => continue,
                                    Err(e) => {
                                        tracing::warn!("GHOSTSNP decode fallback for {}: {e}", bk.block_id);
                                        let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                                        g.terminal.vt_write(&snapshot_buf);
                                        g.received_bytes += snapshot_buf.len() as u64;
                                        snapshot_buf.clear();
                                        snapshot_done = true;
                                        (inner.notify)(HubNotice::BlockOutput(bk.clone()));
                                    }
                                }
                            } else if snapshot_buf.len() >= 8 || !b"GHOSTSNP".starts_with(&snapshot_buf) {
                                let items = apc.push(&snapshot_buf);
                                {
                                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                                    for item in items {
                                        match item {
                                            DemuxItem::Vt(b) => g.terminal.vt_write(&b),
                                            DemuxItem::Apc(a) => inner.handle_inline_apc(&bk, a),
                                        }
                                    }
                                    g.received_bytes += snapshot_buf.len() as u64;
                                    g.recompute_activity();
                                }
                                snapshot_buf.clear();
                                snapshot_done = true;
                                (inner.notify)(HubNotice::BlockOutput(bk.clone()));
                            }
                            continue;
                        }

                        let items = apc.push(&chunk);
                        {
                            let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                            for item in items {
                                match item {
                                    DemuxItem::Vt(b) => g.terminal.vt_write(&b),
                                    DemuxItem::Apc(a) => inner.handle_inline_apc(&bk, a),
                                }
                            }
                            g.received_bytes += chunk.len() as u64;
                            g.recompute_activity();
                        }
                        (inner.notify)(HubNotice::BlockOutput(bk.clone()));
                    }
                }
            }
            br.lock().unwrap_or_else(|e| e.into_inner()).data_connected = false;
        });
    }

    fn handle_inline_apc(&self, bk: &BlockKey, apc: InlineApc) {
        match apc {
            InlineApc::ChildExited { exit_code } => {
                (self.notify)(HubNotice::Toast {
                    title: "Process exited".into(),
                    body: format!("Exit code {exit_code}"),
                    host_key: Some(bk.host_key.clone()),
                    session_id: Some(bk.session_id.clone()),
                    block_id: Some(bk.block_id.clone()),
                });
            }
            InlineApc::Unknown(val) => {
                if let Some(title) = val.get("title").and_then(Value::as_str) {
                    let body = val.get("body").and_then(Value::as_str).unwrap_or("");
                    (self.notify)(HubNotice::Toast {
                        title: title.to_owned(),
                        body: body.to_owned(),
                        host_key: Some(bk.host_key.clone()),
                        session_id: Some(bk.session_id.clone()),
                        block_id: Some(bk.block_id.clone()),
                    });
                }
            }
            InlineApc::Resize { .. } | InlineApc::Theme { .. } => {}
        }
    }

    async fn handle_control_event(self: &Arc<Self>, host_key: &str, ctrl: &ControlClient, ep: &RexEndpoint, ev: ControlEvent) {
        match ev {
            ControlEvent::SessionCreated(s) => {
                if let Ok(att) = ctrl.session_attach(&s.session_id).await {
                    self.ingest_session(host_key, ctrl, ep, att.payload.view, &[]).await;
                }
            }
            ControlEvent::SessionDestroyed { session_id } => {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(h) = st.hosts.get_mut(host_key) {
                    h.sessions.remove(&session_id);
                    h.session_summaries.retain(|s| s.session_id != session_id);
                }
                st.blocks.retain(|bk, _| !(bk.host_key == host_key && bk.session_id == session_id));
                drop(st);
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
            ControlEvent::EventDropped { session_id, .. } => {
                if let Some(sid) = session_id
                    && let Ok(v) = ctrl.session_view(&sid).await
                {
                    self.ingest_session(host_key, ctrl, ep, v.payload, &[]).await;
                }
            }
            ControlEvent::BlockEvent { session_id, block_id, name, payload, .. } => {
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.clone(), block_id: block_id.clone() };
                let br_opt = self.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned();
                if let Some(br) = br_opt {
                    let mut notify_toast: Option<(String, String, bool)> = None;
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
                                    g.password_prompt = !echo;
                                }
                            }
                            "program_status_changed" => {
                                if let Ok(recs) = serde_json::from_value::<Vec<ProgramStatusRecord>>(payload.get("records").cloned().unwrap_or(Value::Array(vec![]))) {
                                    let prev = g.agent_activity;
                                    g.program_statuses = recs;
                                    let now = g.recompute_activity();
                                    if now != prev && !g.muted && matches!(now, AgentActivity::Blocked | AgentActivity::Error | AgentActivity::Done) {
                                        g.unread = true;
                                        let msg = g.program_statuses.first().and_then(|r| r.msg.clone().or_else(|| r.title.clone())).unwrap_or_else(|| format!("{:?}", now));
                                        let label = if g.title.is_empty() { "Rex Pane".to_owned() } else { g.title.clone() };
                                        notify_toast = Some((label, msg, now == AgentActivity::Blocked || now == AgentActivity::Error));
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
                                    g.ports = ps;
                                }
                            }
                            "process_changed" => {
                                if let Ok(pr) = serde_json::from_value::<TerminalProcessSnapshot>(payload.clone()) {
                                    if let Some(cwd) = pr.foreground.as_ref().and_then(|p| p.cwd.as_ref()).or_else(|| pr.child.as_ref().and_then(|p| p.cwd.as_ref())) {
                                        g.pwd = cwd.clone();
                                    }
                                    g.process = Some(pr);
                                    g.recompute_activity();
                                }
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
                    if let Some((title, body, urgent)) = notify_toast {
                        (self.notify)(HubNotice::Toast {
                            title: title.clone(),
                            body: body.clone(),
                            host_key: Some(bk.host_key.clone()),
                            session_id: Some(bk.session_id.clone()),
                            block_id: Some(bk.block_id.clone()),
                        });
                        if urgent {
                            bridge::send_desktop_notification(&title, &body, Some(vec!["--focus-block".into(), bk.host_key.clone(), bk.session_id.clone(), bk.block_id.clone()]));
                        }
                    }
                    if let Some(nudge) = keep_going_nudge {
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
                let km_defaults = [keymap::default_client_layer()];
                if let Ok(r) = ctrl.client_keymap(&km_actions, &km_defaults, &user_overrides).await {
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
                    h.tailscale = Some(status);
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
                let ok = self.handle_client_perform(host_key, req.action.clone(), req.args.clone(), req.session_id.clone()).await;
                if ok {
                    let _ = ctrl.client_reply_ok(&req.call_id, json!({ "ok": true })).await;
                } else {
                    let _ = ctrl.client_reply_err(&req.call_id, "permission_denied", "Action denied by Olo host policy").await;
                }
            }
            ControlEvent::ClientCallRequested(req) => {
                let _ = ctrl.client_reply_err(&req.call_id, "unsupported", &format!("Unknown RPC {}.{}", req.server, req.method)).await;
            }
            _ => {}
        }
    }

    async fn handle_client_perform(self: &Arc<Self>, host_key: &str, action: String, args: Option<Value>, session_id: Option<String>) -> bool {
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
                    let title = args.as_ref().and_then(|a| a.get("title")).and_then(Value::as_str).unwrap_or("Rex");
                    let body = args.as_ref().and_then(|a| a.get("body")).and_then(Value::as_str).unwrap_or("");
                    bridge::send_desktop_notification(title, body, None);
                    return true;
                }
                "olo.clipboard.set" => {
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

        (self.notify)(HubNotice::PerformRequested { host_key: host_key.to_owned(), action, args, session_id });
        true
    }

    /// Atomically persist live Rex session summaries so Olo's global Command Palette
    /// (`olo-palette`) can search and jump directly into local & remote Rex sessions.
    fn write_palette_index(&self) {
        if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") {
            return;
        }
        #[derive(serde::Serialize)]
        struct PaletteEntry {
            host_key: String,
            host_label: String,
            session_id: String,
            label: String,
            cwd: String,
        }
        let entries: Vec<PaletteEntry> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut out = Vec::new();
            for hk in &st.host_order {
                let Some(h) = st.hosts.get(hk) else { continue };
                for s in &h.session_summaries {
                    let cwd = h
                        .sessions
                        .get(&s.session_id)
                        .and_then(|v| v.active_window().or_else(|| v.windows.first()))
                        .and_then(|w| w.focused_block_id.as_deref().or_else(|| w.all_blocks().next().map(|b| b.block_id.as_str())))
                        .and_then(|bid| {
                            let bk = BlockKey { host_key: hk.clone(), session_id: s.session_id.clone(), block_id: bid.to_owned() };
                            st.blocks.get(&bk).map(|b| b.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone())
                        })
                        .unwrap_or_default();
                    out.push(PaletteEntry {
                        host_key: hk.clone(),
                        host_label: h.label.clone(),
                        session_id: s.session_id.clone(),
                        label: if s.label.is_empty() { s.session_id.clone() } else { s.label.clone() },
                        cwd,
                    });
                }
            }
            out
        };
        let dir = olo_xdg::state_dir().join("rex");
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("palette-index.json");
        let tmp = dir.join(format!("palette-index.json.tmp.{}", std::process::id()));
        if let Ok(bytes) = serde_json::to_vec(&entries)
            && std::fs::write(&tmp, bytes).is_ok()
        {
            let _ = std::fs::rename(&tmp, &target);
        }
    }
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

    /// End-to-end live integration test: spawns an isolated scratch `rex server run`,
    /// connects via `olo-rex-client`, opens a `rex.data.terminal.v1` stream, decodes
    /// the real `GHOSTSNP` binary snapshot via `olo-rex-vt::try_decode_snapshot_prefix`,
    /// and verifies `RenderState` extracts rendered rows cleanly.
    #[test]
    fn live_ghostsnp_to_render_state_pipeline() {
        if !std::path::Path::new("/usr/bin/rex").exists() {
            return;
        }
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let sock = dir.join("rex.sock");
        let mut child = std::process::Command::new("/usr/bin/rex")
            .args(["server", "run", "--listen", &format!("unix://{}", sock.display())])
            .env("XDG_RUNTIME_DIR", &dir)
            .env("XDG_STATE_HOME", &dir)
            .env("XDG_CONFIG_HOME", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn scratch rex server");

        let deadline = Instant::now() + Duration::from_secs(6);
        while !sock.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(sock.exists(), "scratch rex socket did not appear");

        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let res: Result<(), String> = rt.block_on(async {
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

            let mut buf = Vec::new();
            let mut decoded = None;
            let read_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < read_deadline {
                match tokio::time::timeout(Duration::from_secs(2), tstream.next_frame()).await {
                    Ok(Ok(TerminalStreamFrame::Bytes(chunk))) => {
                        buf.extend_from_slice(&chunk);
                        if buf.len() >= 8 && &buf[..8] == b"GHOSTSNP" {
                            if let Some(sr) = try_decode_snapshot_prefix(&buf, true, None).map_err(|e| e.to_string())? {
                                decoded = Some(sr.terminal);
                                break;
                            }
                        }
                    }
                    _ => break,
                }
            }
            let mut term = decoded.ok_or_else(|| format!("did not receive complete GHOSTSNP (got {} bytes)", buf.len()))?;
            let mut rs = RenderState::new().map_err(|e| e.to_string())?;
            rs.begin_update(&mut term).map_err(|e| e.to_string())?;
            let _ = rs.end_update();
            let colors = rs.colors();
            let rows = rs.extract_rows(&colors.palette, false).map_err(|e| e.to_string())?;
            assert!(!rows.is_empty(), "RenderState extracted zero rows");
            tstream.close_clean().await;
            Ok(())
        });

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        res.expect("live GHOSTSNP -> RenderState pipeline failed");
    }
}
