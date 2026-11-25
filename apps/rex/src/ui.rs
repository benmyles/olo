//! Olo Rex UI: main workspace window (`Shell` + isolated `Panes` child view),
//! adaptive `SidePane` (Hosts/Sessions + Live Activity feed), all 4 toolbar layouts,
//! recursive binary splits with draggable dividers, floating & minimized layers,
//! full keyboard/mouse/IME/drag-drop handling, and secondary panels.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, CursorStyle, DragAction, DropAccept, DropOffer, Dropped, Element, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, Pixels, Point, ScrollWheelEvent, Styled as _, TextInput, View, WindowCx, WindowHandle, WindowOptions, agent, canvas, div, icons,
    menu::{self, Menu, MenuBar},
    metrics as m, point, px, size, style, theme,
    widgets::{
        self, Banner, Button, Front, Glyph, GlyphButton, Panes as SidePanes, SidePane, StringView, StripAction, StripTab, TabStrip, TextControl, TextField, TextFieldEvent, Tone, find_bar, find_said,
        side_divider_within,
    },
};
use olo_rex_client::{LayerKind, LayerState, LayoutNode, WindowView};
use olo_rex_vt::{
    FormatKind, TerminalTheme, encode_paste,
    input::{MouseAction, MouseButton as VtButton},
    paste_is_safe,
};
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    agent::AgentActivity,
    attention::{Attention, Target},
    bridge,
    copy_mode::{CopyMode, CopyModeAction},
    hub::{BlockKey, HostConnState, HubNotice, RexHub},
    keymap::{self, KeyMatch, KeymapEngine},
    links::{self, LinkTarget},
    overlays::{self, ActiveOverlay, JumpTarget},
    panels::{self, AskpassReply, AskpassRequest, OloAskpass},
    settings::{BarBehavior, SchemeChoice, Settings, SidebarMode, ToolbarLayout},
    terminal_pane::{self, Metrics, PanePaintArgs, PaneVisualState},
    themes::{self, hsla},
};

pub const APP_ID: &str = "olo.rex";

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WinId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinRole {
    Workspace,
    Hosts,
    /// Remote Access: this computer's server over Tailscale, and a
    /// device paired with it.
    Access,
    Diagnostics,
    Askpass,
    /// Desktop command approval sheet (`olo run` from a Rex host).
    RunApprove,
    /// A new name for a session or a tab ([`Renaming`]).
    Rename,
    /// A host to add: its address, and a name.
    AddHost,
    /// An account moved to another host (`crate::moving`).
    Move,
    /// A port of another host to forward here.
    Forward,
    /// A host's own Rex server, looked after as root (`server`).
    Server,
    /// Everything of Olo's on a far host, in one place (`host`).
    Host,
    /// A far host's account made ready for work (`account`).
    Account,
    /// The person's repositories (`repos`), and the form that adds
    /// one or changes one.
    Repos,
    RepoEdit,
    /// Agent settings carried to a host: batch conflict review, preview,
    /// and remembered choices (`carry`).
    Carry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedWorkspaceState {
    pub active_host: String,
    pub active_session: Option<String>,
    #[serde(default)]
    pub session_index: usize,
    pub sidebar_collapsed: bool,
    pub sidebar_width: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAppState {
    pub workspaces: Vec<SavedWorkspaceState>,
    /// The hosts let go of (Hosts and Ports' Disconnect): they stay so.
    #[serde(default)]
    pub let_go: Vec<String>,
    /// What each pane has on (Keep It Going, Auto Approve), by its host,
    /// session and pane.
    #[serde(default)]
    pub help: Vec<(String, String, String, crate::agent::Help)>,
    /// The forwards, each taken up again at its port when its host
    /// connects (`hub::SavedForward`).
    #[serde(default)]
    pub forwards: Vec<crate::hub::SavedForward>,
}

/// Under this width (points) a pane's Auto Approve countdown says only
/// "Yes in N s" (at full length it took two thirds of a half pane).
const COUNTDOWN_SHORT: f32 = 480.0;

/// Auto Approve on or off for a pane (`said`: its name, for the
/// question). Turning it on is the person's (Terminal's rule): asked for
/// through the commands socket (an agent, the palette), they are asked
/// first in an alert whose Return is Cancel, so an agent can't answer its
/// own questions; turning it off needs no asking.
pub fn toggle_auto_approve(hub: &Arc<RexHub>, key: &BlockKey, said: &str) {
    let on = !hub.help(key).auto_approve;
    if !on || !olo_kit::agent::invoking() {
        hub.set_help(key, |h| h.auto_approve = on);
        return;
    }
    tracing::info!(pane = key.block_id, "auto approve: asked for through the commands socket; asking the person");
    let secs = std::env::var("OLO_REX_AUTO_APPROVE_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(crate::agent::AUTO_APPROVE_AFTER.as_secs());
    let message = olo_kit::tr!(
        "Turn on Auto Approve for \u{201c}{}\u{201d}?\n\nAnother program asked for it, not you. When the coding agent in this pane asks to go on (\u{201c}Do you want to proceed?\u{201d}), Rex will answer Yes after {}\u{a0}seconds, unless you type in the pane or press Stop.",
        said,
        secs
    );
    let (hub, key) = (hub.clone(), key.clone());
    olo_kit::widgets::open_app_alert("Rex", "warning", &message, &["Turn On", "Cancel"], move |chosen| {
        if chosen == Some(0) {
            hub.set_help(&key, |h| h.auto_approve = true);
        }
    });
}

/// The panes of a tab Keep It Going and Auto Approve are offered for in
/// its menu: those an agent runs in or with either on, else the one with
/// the keyboard.
/// Open a pane's working folder in Olo Tracker (local path on this computer,
/// canonical `rex://HOST/PATH` URI on a remote Rex host).
fn open_block_in_tracker(hub: &Arc<RexHub>, bk: &BlockKey) {
    let info = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(&bk.host_key).map(|h| (h.is_local, h.label.clone()));
        let pwd = st.blocks.get(bk).map(|br| br.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone()).unwrap_or_default();
        host.map(|(is_local, label)| (is_local, label, pwd))
    };
    let Some((is_local, label, pwd)) = info else { return };
    if is_local {
        let dir = if pwd.is_empty() { std::env::var("HOME").unwrap_or_else(|_| "/".into()) } else { pwd };
        bridge::open_folder_in_tracker(std::path::Path::new(&dir));
    } else {
        let path = if pwd.is_empty() { "~" } else { &pwd };
        let uri = olo_kit_base::tracker::RexPath::new(label, path).to_uri();
        bridge::open_uri_in_tracker(&uri);
    }
}

fn tab_agent_panes(hub: &RexHub, host_key: &str, session_id: &str, w: &olo_rex_client::WindowView) -> Vec<BlockKey> {
    let key = |block_id: &str| BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: block_id.to_owned() };
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let with_agent: Vec<BlockKey> =
        w.all_blocks().map(|b| key(&b.block_id)).filter(|k| crate::hub::help_of(k).any() || st.blocks.get(k).is_some_and(|b| b.lock().unwrap_or_else(|e| e.into_inner()).runs_agent())).collect();
    if !with_agent.is_empty() {
        return with_agent;
    }
    w.focused_block_id.as_deref().or_else(|| w.all_blocks().next().map(|b| b.block_id.as_str())).map(key).into_iter().collect()
}

/// A file going to or from a pane's host, at the status bar's end: what,
/// a bar, how much of how much, and Stop where it can be stopped.
fn transfer_said(t: crate::hub::Transfer, win_id: WinId) -> Element {
    let fraction = if t.total > 0 { t.done as f32 / t.total as f32 } else { 0.0 };
    let bar = canvas(move |g, b| {
        let p = theme::palette();
        let bar = style::snap_centred(g, b.center(), 120.0, 8.0);
        widgets::level_bar(g, bar, fraction, None, &p);
    })
    .paint_key(style::key((t.done * 1000 / t.total.max(1), t.total)))
    .w(px(120.0))
    .h(px(m::LABEL_ROW))
    .flex_shrink_0();
    // (The news while it lasts: it keeps its room, the status line gives
    // way; a long name is cut, at most 260 points.)
    let mut row = div()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::TIGHT))
        .child({
            // (Its own width, measured: a cut line has none of its own.)
            let wide = f32::from(style::text_width(t.said.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil().min(260.0);
            div().w(px(wide)).child(StringView::new(t.said).size(style::SMALL_SIZE).height(m::LABEL_ROW).cut(true))
        })
        .child(bar)
        .child(StringView::new(olo_kit::tr!("{} of {}", megabytes(t.done), megabytes(t.total))).size(style::SMALL_SIZE).height(m::LABEL_ROW).dim(true));
    if t.stoppable {
        row = row.child(Button::new("Stop").small().id("rex-transfer-stop").on_click(move |_, _| {
            if let Some(slot) = wins().map.get(&win_id) {
                slot.handle.messenger().send(WinMsg::StopTransfer);
            }
        }));
    }
    row.into_element()
}

/// Bytes as a person reads them: "2.1 MB", "640 KB".
fn megabytes(bytes: u64) -> String {
    if bytes >= 1_000_000 { format!("{:.1}\u{a0}MB", bytes as f64 / 1_000_000.0) } else { format!("{}\u{a0}KB", bytes.div_ceil(1000)) }
}

/// A picture pasted into a pane, as a file to put on its host: in Rex's
/// own folder of them (`<cache>/rex-pasted`), named by when, its kind's
/// ending ("pasted-2026-11-24-101203.png": no spaces, which an agent
/// would read as two words); those older than a day removed.
fn pasted_image_file(mime: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let dir = olo_xdg::cache_dir().join("rex-pasted");
    std::fs::create_dir_all(&dir)?;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(86_400)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let ending = match mime {
        "image/jpeg" => "jpg",
        "image/svg+xml" => "svg",
        m => m.strip_prefix("image/").filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("png"),
    };
    let when = utc_stamp(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    let mut path = dir.join(format!("pasted-{when}.{ending}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("pasted-{when}-{n}.{ending}"));
        n += 1;
    }
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// `secs` since the epoch as "2026-11-24-101203" (UTC: a name, not a
/// time to read).
fn utc_stamp(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // (Days to a date: Howard Hinnant's civil_from_days.)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// The modifiers held, as a terminal's mouse report says them.
fn key_mods(m: &olo_kit::Modifiers) -> olo_rex_vt::input::KeyMods {
    use olo_rex_vt::input::KeyMods;
    KeyMods((if m.shift { KeyMods::SHIFT } else { 0 }) | (if m.control { KeyMods::CTRL } else { 0 }) | (if m.alt { KeyMods::ALT } else { 0 }) | (if m.platform { KeyMods::SUPER } else { 0 }))
}

/// What a pane is called, as its tab would show it.
fn pane_name(hub: &RexHub, key: &BlockKey) -> String {
    crate::hub::name_of_pane(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), key).unwrap_or_default()
}

/// Global Rex application controller (shared safely across windows via `olo_sync::Mutex`).
pub struct RexApp {
    pub hub: Arc<RexHub>,
    pub askpass: Arc<OloAskpass>,
    pub run_approvals: Arc<crate::bridge_run::RunApprovals>,
    pub keymap: KeymapEngine,
    /// How the SSH agent stands (asked once as Rex starts), for Diagnostics.
    pub ssh_agent: olo_rex_client::AgentStatus,
    attention: Attention,
    broadcast_windows: HashSet<(String, String, String)>,
    /// What the Rename window is naming.
    pub renaming: Option<Renaming>,
}

/// What is being given a new name: a session, or one of its tabs.
#[derive(Clone, Debug, PartialEq)]
pub struct Renaming {
    pub host_key: String,
    pub session_id: String,
    /// The tab; none, the session itself.
    pub window_id: Option<String>,
    /// The name it has (a tab without one: empty), and what it shows
    /// as now (the tab's program, where it has no name).
    pub was: String,
    pub shown: String,
    /// The window it was asked from, which hears how it went.
    pub from: WinId,
}

impl Renaming {
    /// What its field shows while empty: for a tab, what it shows as
    /// with no name (what runs in it), which is what leaving it empty
    /// gives; nothing for a session, which must have a name.
    fn hint(&self) -> String {
        if self.window_id.is_some() { self.shown.clone() } else { String::new() }
    }

    /// Whether `name` (as typed, trimmed) can be given: a session's may
    /// not be empty (it would show as its id); a tab's may (it shows
    /// what runs in it again).
    fn takes(&self, name: &str) -> bool {
        self.window_id.is_some() || !name.is_empty()
    }
}

/// Asks for a new name for `what`, in a small window of its own.
pub fn open_rename(what: Renaming) {
    app().renaming = Some(what);
    // (One already open is for another thing: it takes this one.)
    let open = wins().map.values().find(|s| s.role == WinRole::Rename).map(|s| s.handle.clone());
    match open {
        Some(h) => {
            h.messenger().send(WinMsg::RenameAgain);
            h.messenger().send(WinMsg::Raise);
        }
        None => open_or_raise_panel(WinRole::Rename),
    }
}

#[derive(Default)]
struct WorkspaceSelection {
    host: String,
    session: Option<String>,
    block: Option<String>,
    active: bool,
}

impl WorkspaceSelection {
    fn matches(&self, host: &str, session: Option<&str>, block: Option<&str>) -> bool {
        self.host == host && session.is_none_or(|s| self.session.as_deref() == Some(s)) && block.is_none_or(|b| self.block.as_deref() == Some(b))
    }
}

struct WinSlot {
    selection: WorkspaceSelection,
    role: WinRole,
    handle: WindowHandle<RexWindow>,
}

#[derive(Default)]
struct WinRegistry {
    map: HashMap<WinId, WinSlot>,
    next: u64,
}

static APP: OnceLock<Mutex<RexApp>> = OnceLock::new();
static WINS: OnceLock<Mutex<WinRegistry>> = OnceLock::new();

pub fn app() -> olo_sync::MutexGuard<'static, RexApp> {
    APP.wait().lock().unwrap_or_else(|e| e.into_inner())
}

/// The hub, where the app has started (its lock let go at once).
pub fn app_hub() -> Option<Arc<RexHub>> {
    APP.get().map(|a| a.lock().unwrap_or_else(|e| e.into_inner()).hub.clone())
}

fn wins() -> olo_sync::MutexGuard<'static, WinRegistry> {
    WINS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The desktop's settings for Rex changed (the settings' own thread).
pub fn follow_desktop_settings(rex: &olo_protocol::RexSettings) {
    // (The app's lock let go before the hub's is taken.)
    let Some(hub) = APP.get().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()).hub.clone()) else { return };
    hub.follow_shared(rex);
}

pub fn init_app(settings: Settings, _initial_cwd: Option<String>, _restored: Vec<SavedWorkspaceState>) {
    let active_theme = themes::resolve_theme(&settings, &theme::palette());
    let askpass = OloAskpass::new(
        || {
            open_or_raise_panel(WinRole::Askpass);
        },
        || broadcast_msg(WinMsg::Redraw),
    );
    let run_approvals = crate::bridge_run::RunApprovals::new(
        || {
            open_or_raise_panel(WinRole::RunApprove);
        },
        || broadcast_to_role(WinRole::RunApprove, || WinMsg::RunApproveChanged),
    );
    let hub = RexHub::start(settings.clone(), active_theme, Some(askpass.clone()), Some(run_approvals.clone()), move |notice| handle_hub_notice(notice));
    let keymap = KeymapEngine::new(&[], &[], &[]);
    let _ = APP.set(Mutex::new(RexApp { hub, askpass, run_approvals, keymap, ssh_agent: Default::default(), renaming: None, attention: Attention::new(), broadcast_windows: HashSet::new() }));

    // How the SSH agent stands, asked once and off this thread, for Diagnostics.
    let agent_override = settings.ssh_agent_socket.map(PathBuf::from);
    let _ = std::thread::Builder::new().name("rex-ssh-probe".into()).spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            let status = rt.block_on(olo_rex_client::inspect_ssh_agent(agent_override.as_deref()));
            if let Some(m) = APP.get() {
                m.lock().unwrap_or_else(|e| e.into_inner()).ssh_agent = status;
                broadcast_msg(WinMsg::Redraw);
            }
        }
    });
}

fn handle_hub_notice(notice: HubNotice) {
    match notice {
        HubNotice::BlockOutput(key) => {
            for slot in wins().map.values().filter(|w| w.selection.matches(&key.host_key, Some(&key.session_id), None)) {
                slot.handle.messenger().send(WinMsg::Screens);
            }
        }
        HubNotice::StateChanged => {
            // Rebuild keymap if server bindings updated.
            if let Some(m) = APP.get() {
                let hub = m.lock().unwrap_or_else(|e| e.into_inner()).hub.clone();
                let (bindings, modes, diags) = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    let local = st.hosts.get("local");
                    (
                        local.map(|h| h.keymap_bindings.clone()).unwrap_or_default(),
                        local.map(|h| h.keymap_modes.clone()).unwrap_or_default(),
                        local.map(|h| h.keymap_diagnostics.clone()).unwrap_or_default(),
                    )
                };
                let mut a = m.lock().unwrap_or_else(|e| e.into_inner());
                a.keymap.rebuild(&bindings, &modes, &diags);
                if a.attention.has_notes() {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    a.attention.prune(|target| match target {
                        Target::Host(h) => st.hosts.contains_key(h),
                        Target::Pane(p) => st.blocks.contains_key(p),
                        Target::Session { host_key, session_id } => st.hosts.get(host_key).is_some_and(|h| h.session_summaries.iter().any(|row| &row.session_id == session_id)),
                    });
                }
            }
            broadcast_msg(WinMsg::Redraw);
        }
        HubNotice::PromptRequested { host_key, req } => {
            if let Some(w) = selected_workspace(&host_key, None, None) {
                w.messenger().send(WinMsg::ShowPrompt { host_key, req });
            } else if let Some(a) = APP.get() {
                let hub = a.lock().unwrap_or_else(|e| e.into_inner()).hub.clone();
                hub.respond_prompt(&host_key, &req.call_id, true, None);
            }
        }
        HubNotice::PromptEnded { host_key, call_id } => {
            broadcast_to_role(WinRole::Workspace, move || WinMsg::EndPrompt { host_key: host_key.clone(), call_id: call_id.clone() });
        }
        HubNotice::Attention { target, title, body, kind } => {
            if let Some(a) = APP.get() {
                a.lock().unwrap_or_else(|e| e.into_inner()).attention.show(target, title, body, kind);
            }
        }
        HubNotice::SetClipboard(text) => {
            if let Some(w) = workspace() {
                w.messenger().send(WinMsg::SetClipboard(text));
            }
        }
        // (In the window showing the asking pane, as its own Quick Look.)
        HubNotice::OpenHostFile(bk, path) => {
            if let Some(w) = selected_workspace(&bk.host_key, Some(&bk.session_id), Some(&bk.block_id)).or_else(workspace) {
                w.messenger().send(WinMsg::OpenPreview(bk, path));
            }
        }
        HubNotice::Said(text) => {
            if let Some(w) = workspace() {
                w.messenger().send(WinMsg::Said(text));
            }
        }
        HubNotice::QuietNote { title, body } => {
            if let Some(a) = APP.get() {
                a.lock().unwrap_or_else(|e| e.into_inner()).attention.quiet(title, body);
            }
        }
    }
}

/// Open the `olo run` desktop command approval sheet populated with a
/// deterministic demo prompt (`OLO_REX_DEMO_RUN=1|long|plain`) for headless visual QA.
pub fn open_demo_run_approve(mode: &str) {
    app().run_approvals.show_demo(mode);
    open_or_raise_panel(WinRole::RunApprove);
}

/// Open the Agent Settings batch conflict review window populated with
/// deterministic demo data (`OLO_REX_DEMO_CARRY=1|empty|remembered|merged`) for headless visual QA.
pub fn open_demo_carry() {
    let hub = app().hub.clone();
    crate::carry::open(&hub, "dev", "dev-box");
    open_or_raise_panel(WinRole::Carry);
    carry_changed();
}

/// Switch the demo workspace to Activity sidebar with deterministic nested subagents
/// (`OLO_REX_DEMO_ACTIVITY=1|collapsed|transcript|follow|failed|find|nomatch|tmux|tmux_big`) for headless visual QA.
pub fn open_demo_activity(mode: &str) {
    let hub = app().hub.clone();
    {
        let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.settings.sidebar_mode = SidebarMode::Activity;
        st.settings.sidebar_collapsed = false;
        // Focus the active agent block so its Activity row carries the selection band.
        if let Some(h) = st.hosts.get_mut("local")
            && let Some(s) = h.sessions.get_mut("demo-main")
            && let Some(w) = s.windows.first_mut()
        {
            w.focused_block_id = Some("demo-agent".to_owned());
        }
    }
    crate::activity_watch::sync_watchers(&hub, true);
    if matches!(mode, "find" | "nomatch") {
        broadcast_to_role(WinRole::Workspace, || WinMsg::Command("terminal.find".into()));
    }
    activity_changed();
}

fn broadcast_msg(msg: WinMsg) {
    let handles: Vec<WindowHandle<RexWindow>> = wins().map.values().map(|s| s.handle.clone()).collect();
    for h in handles {
        h.messenger().send(msg.clone());
    }
}

/// The host being set up stands otherwise: Add Host shows it.
pub fn setup_changed() {
    broadcast_to_role(WinRole::AddHost, || WinMsg::SetupChanged);
}

/// The move under way stands otherwise: the Move window shows it.
pub fn move_changed() {
    *MOVE_CHANGED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
    broadcast_to_role(WinRole::Move, || WinMsg::MoveChanged);
}

/// When the Move window last changed: Cancel comes where Continue and
/// Move were, so a press of it a double-click's time after is the second
/// click of a double-click on them, not a Cancel (Opus's design review,
/// 2026-11-25).
static MOVE_CHANGED_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

fn move_just_changed() -> bool {
    MOVE_CHANGED_AT.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(500))
}

/// An account moved from host `from` to host `to`: each window showing
/// `from` shows `to`, each of its sessions the one made again from it
/// (`sessions`: by the old session's id).
pub fn host_moved(from: String, to: String, sessions: std::collections::HashMap<String, String>) {
    broadcast_to_role(WinRole::Workspace, || WinMsg::HostMoved(from.clone(), to.clone(), sessions.clone()));
}

/// The host whose server is looked after stands otherwise: the Server
/// window shows it.
pub fn server_changed() {
    broadcast_to_role(WinRole::Server, || WinMsg::ServerChanged);
}

/// A host of the hub's whose server can be looked after from here: one
/// reached over SSH as an account. Its name here, and where it is.
fn managed_host(hub: &RexHub, key: &str) -> Option<(String, olo_rex_client::SshTarget)> {
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let host = st.hosts.get(key).filter(|h| !h.is_local)?;
    match crate::setup::address(&host.endpoint_raw) {
        crate::setup::Address::Ssh(target) => Some((if host.label.is_empty() { target.host.clone() } else { host.label.clone() }, target)),
        _ => None,
    }
}

/// Whether `key` is a host of the hub's other than this computer.
fn far_host(hub: &RexHub, key: &str) -> bool {
    hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(key).is_some_and(|h| !h.is_local)
}

/// The Host window shows otherwise (another host, a thing being done
/// to it, what the host said of one).
pub fn host_changed() {
    broadcast_to_role(WinRole::Host, || WinMsg::HostChanged);
}

/// Opens the Host window on the host `key`: what it has of Olo's, and
/// its Rex server. (Not for this computer: what is Olo's here is the
/// desktop's own.)
/// The Move window, for the account on host `key` (the move under way
/// shown, if there is one).
fn open_move(hub: &Arc<RexHub>, key: &str) {
    if crate::moving::current().is_none() {
        if !far_host(hub, key) {
            return;
        }
        crate::moving::begin(hub, key);
    }
    open_or_raise_panel(WinRole::Move);
}

fn open_host(hub: &Arc<RexHub>, key: &str) {
    let Some(name) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(key).filter(|h| !h.is_local).map(|h| h.label.clone()) else { return };
    crate::host::open(key, &name);
    open_or_raise_panel(WinRole::Host);
    host_changed();
    // (How its Token Pool stands is this computer's Token Pool's to
    // say: asked as the window opens. And its account, of the host.)
    hub.ask_pools();
    // (And followed while the window is open: a pool installed or
    // synced from the Token Pool's own window shows here as it
    // happens.)
    hub.watch_pools(|| crate::host::current().is_some());
    hub.ask_account(key, host_changed);
}

/// The Host window's title: whose things it shows. (By what `host`
/// keeps, never through `app()`: the window is opened from handlers
/// that hold it, and asking for it again there stopped the app for
/// good at a click on a status mark.)
/// The Account window shows otherwise (another host, a step begun or
/// ended, a line its installer wrote).
pub fn account_changed() {
    broadcast_to_role(WinRole::Account, || WinMsg::AccountChanged);
}

pub fn carry_changed() {
    broadcast_to_role(WinRole::Carry, || WinMsg::CarryChanged);
}

pub fn activity_changed() {
    broadcast_to_role(WinRole::Workspace, || WinMsg::ActivityChanged);
}

fn open_carry(hub: &Arc<RexHub>, key: &str) {
    let name = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(key).filter(|h| !h.is_local).map(|h| h.label.clone());
    let Some(name) = name else { return };
    crate::carry::open(hub, key, &name);
    open_or_raise_panel(WinRole::Carry);
    carry_changed();
}

fn carry_title() -> String {
    match crate::carry::current() {
        Some(shown) => olo_kit::tr!("Agent Settings on “{}”", shown.name).into(),
        None => olo_kit::words::tr("Agent Settings").into(),
    }
}

/// Opens the Account window on the host `key`: its account made ready
/// for work. (Not for this computer.)
fn open_account(hub: &Arc<RexHub>, key: &str) {
    let Some(name) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(key).filter(|h| !h.is_local).map(|h| h.label.clone()) else { return };
    crate::account::open(hub, key, &name);
    open_or_raise_panel(WinRole::Account);
    account_changed();
}

fn account_title() -> String {
    match crate::account::current() {
        Some(shown) => olo_kit::tr!("Account on “{}”", shown.name).into(),
        None => olo_kit::words::tr("Account").into(),
    }
}

fn host_title() -> String {
    match crate::host::current() {
        Some(shown) => olo_kit::tr!("Host “{}”", shown.name).into(),
        None => olo_kit::words::tr("Host").into(),
    }
}

/// Opens the Server window on the host `key` (nothing, where its
/// server isn't one to look after from here).
fn open_server(hub: &Arc<RexHub>, key: &str) {
    let Some((name, target)) = managed_host(hub, key) else { return };
    crate::server::open(hub, name, target);
    open_or_raise_panel(WinRole::Server);
}

fn broadcast_to_role(role: WinRole, make: impl Fn() -> WinMsg) {
    let handles: Vec<WindowHandle<RexWindow>> = wins().map.values().filter(|s| s.role == role).map(|s| s.handle.clone()).collect();
    for h in handles {
        h.messenger().send(make());
    }
}

fn selected_workspace(host: &str, session: Option<&str>, block: Option<&str>) -> Option<WindowHandle<RexWindow>> {
    wins().map.iter().filter(|(_, w)| w.role == WinRole::Workspace && w.selection.matches(host, session, block)).min_by_key(|(id, w)| (!w.selection.active, **id)).map(|(_, w)| w.handle.clone())
}

pub fn request_action(host: &str, session: Option<String>, block: Option<String>, action: String, args: Option<Value>) -> bool {
    let Some(w) = selected_workspace(host, session.as_deref(), block.as_deref()) else { return false };
    w.messenger().send(WinMsg::PerformAction { host: host.to_owned(), session, block, action, args });
    true
}

fn workspace() -> Option<WindowHandle<RexWindow>> {
    wins().map.iter().filter(|(_, s)| s.role == WinRole::Workspace).min_by_key(|(id, w)| (!w.selection.active, **id)).map(|(_, s)| s.handle.clone())
}

pub fn notification_event(event: olo_kit::notify::NoteEvent) {
    let target = app().attention.event(event);
    if let Some((target, token)) = target {
        show_target(target, token);
    }
}

pub fn show_target(target: Target, token: Option<String>) {
    if workspace().is_none() {
        open_workspace_window(None);
    }
    if let Some(w) = workspace() {
        w.messenger().send(WinMsg::ShowTarget(target, token));
    }
}

pub fn focus_block_from_cli(host_key: String, session_id: String, block_id: String) {
    show_target(Target::Pane(BlockKey { host_key, session_id, block_id }), None);
}

pub fn focus_session_from_cli(host_key: String, session_id: String) {
    if let Some(w) = workspace() {
        w.messenger().send(WinMsg::SelectSession(host_key, session_id));
        w.messenger().send(WinMsg::Raise);
    }
}

pub fn create_session_from_cli(dir: String) {
    if workspace().is_none() {
        open_workspace_window(None);
    }
    if let Some(w) = workspace() {
        w.messenger().send(WinMsg::CreateSession("local".into(), Some(dir)));
    }
}

#[derive(Clone, Copy)]
pub struct Feedback {
    window: WinId,
    serial: u64,
}

impl Feedback {
    fn start(window: WinId) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let out = Self { window, serial: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) };
        out.send(WinMsg::StartFeedback(out.serial));
        out
    }

    fn send(self, msg: WinMsg) {
        if let Some(slot) = wins().map.get(&self.window) {
            slot.handle.messenger().send(msg);
        }
    }

    pub fn problem(self, title: &str, detail: impl std::fmt::Display) {
        let detail = detail.to_string();
        let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        self.failed((title.to_owned(), Some(detail)));
    }

    pub fn failed(self, (title, detail): (String, Option<String>)) {
        self.send(WinMsg::Problem(self.serial, title, detail));
    }

    pub fn created(self, host: String, sid: String) {
        self.send(WinMsg::Created(self.serial, host, sid));
    }

    pub fn say(self, text: &str) {
        self.send(WinMsg::Confirmation(self.serial, text.to_owned()));
    }
}

/// `--settings`: Rex's settings are the desktop's, Preferences ▸ Rex.
pub fn open_settings_from_cli() {
    olo_kit::app_menu::open_settings("rex");
}

pub fn open_panel_from_cli(role: WinRole) {
    open_or_raise_panel(role);
}

pub fn open_workspace_window(restored: Option<SavedWorkspaceState>) {
    let id = next_win_id();
    let mut opts = WindowOptions::new("Rex", 1080.0, 700.0);
    opts.min_size = Some((320.0, 220.0));
    opts.keep_size = Some("rex-main".into());
    opts.restore_key = Some(format!("rex-ws-{}", id.0));
    let handle = olo_kit::open_window(opts, move |cx| RexWindow::new_workspace(id, restored, cx));
    wins().map.insert(id, WinSlot { role: WinRole::Workspace, handle, selection: Default::default() });
}

pub fn open_or_raise_panel(role: WinRole) {
    let existing = wins().map.values().find(|s| s.role == role).map(|s| s.handle.clone());
    if let Some(h) = existing {
        h.messenger().send(WinMsg::Raise);
        return;
    }
    let id = next_win_id();
    let opts = match role {
        WinRole::Hosts => {
            let mut o = WindowOptions { fixed: true, ..WindowOptions::new("Hosts and Ports", 580.0, 120.0).dialog() };
            o.restore_key = Some("rex-hosts".into());
            o
        }
        WinRole::Access => {
            let mut o = WindowOptions { fixed: true, ..WindowOptions::new("Remote Access", 520.0, 120.0).dialog() };
            o.restore_key = Some("rex-access".into());
            o
        }
        WinRole::Diagnostics => {
            let mut o = WindowOptions { fixed: true, ..WindowOptions::new("Diagnostics", 448.0, 120.0).dialog() };
            o.restore_key = Some("rex-diagnostics".into());
            o
        }
        WinRole::Askpass => WindowOptions::alert("SSH Authentication", 460.0, 120.0),
        WinRole::RunApprove => WindowOptions::alert("Run Command", panels::RUN_APPROVE_WIDTH, 120.0),
        WinRole::AddHost => WindowOptions::alert("Add Host", panels::ADD_HOST_WIDTH, 120.0),
        WinRole::Move => WindowOptions::alert("Move Account", panels::MOVE_WIDTH, 120.0),
        WinRole::Forward => WindowOptions::alert("Forward Port", widgets::ALERT_WIDTH, 120.0),
        // (Not brought back after a reload: it would sign in as root
        // again with nobody having asked it to.)
        WinRole::Server => WindowOptions { fixed: true, ..WindowOptions::new(server_title(), crate::server_panel::SERVER_WIDTH, 120.0).dialog() },
        // (Nor this one, as none of Rex's panels is: it is of a host
        // chosen in another window.)
        WinRole::Host => WindowOptions { fixed: true, ..WindowOptions::new(host_title(), crate::host_panel::HOST_WIDTH, 120.0).dialog() },
        WinRole::Account => WindowOptions { fixed: true, ..WindowOptions::new(account_title(), crate::account_panel::ACCOUNT_WIDTH, 120.0).dialog() },
        WinRole::Repos => WindowOptions { fixed: true, ..WindowOptions::new("Repositories", crate::repos::WIDTH, 120.0).dialog() },
        WinRole::RepoEdit => WindowOptions::alert(if crate::repos::editing().is_some() { "Edit Repository" } else { "Add Repository" }, crate::repos::FORM_WIDTH, 120.0),
        WinRole::Carry => WindowOptions { fixed: true, ..WindowOptions::new(carry_title(), crate::carry_panel::CARRY_WIDTH, 120.0).dialog() },
        WinRole::Rename => {
            let tab = app().renaming.as_ref().is_some_and(|r| r.window_id.is_some());
            WindowOptions::alert(if tab { "Rename Tab" } else { "Rename Session" }, widgets::ALERT_WIDTH, 120.0)
        }
        WinRole::Workspace => {
            open_workspace_window(None);
            return;
        }
    };
    let handle = olo_kit::open_window(opts, move |cx| RexWindow::new_panel(id, role, cx));
    wins().map.insert(id, WinSlot { role, handle, selection: Default::default() });
}

/// The Server window's title: whose server it is.
fn server_title() -> String {
    match crate::server::current() {
        Some(managed) => olo_kit::tr!("Rex Server on {}", managed.name).into(),
        None => olo_kit::words::tr("Rex Server").into(),
    }
}

fn next_win_id() -> WinId {
    let mut w = wins();
    w.next += 1;
    WinId(w.next)
}

#[derive(Clone)]
pub enum WinMsg {
    Redraw,
    Screens,
    ShowTarget(Target, Option<String>),
    CreateSession(String, Option<String>),
    Created(u64, String, String),
    StartFeedback(u64),
    Problem(u64, String, Option<String>),
    Confirmation(u64, String),
    /// A passing word nobody asked for here (`HubNotice::Said`).
    Said(String),
    Raise,
    ShowPrompt {
        host_key: String,
        req: olo_rex_client::ClientPromptRequested,
    },
    EndPrompt {
        host_key: String,
        call_id: String,
    },
    PerformAction {
        host: String,
        session: Option<String>,
        block: Option<String>,
        action: String,
        args: Option<Value>,
    },
    /// The pane's file transfer stopped (a Quick Look's download).
    StopTransfer,
    QrReady(String, Option<Arc<qrcode::QrCode>>),
    OpenPreview(BlockKey, String),
    SetClipboard(String),
    PasteText(BlockKey, olo_rex_client::ControlClient, String),
    /// A picture pasted (its type and bytes): to the pane's host, its path typed.
    PasteImage(BlockKey, String, Vec<u8>),
    PaneCommand(BlockKey, String),
    FocusBlock(BlockKey),
    Command(String),
    RunOverlayIndex(usize),
    TogglePromptCheck(String),
    ConfirmOverlay,
    DismissOverlay,
    TabChanged,
    SelectSession(String, String),
    SelectHost(String),
    FindStep(bool),
    FindClose,
    /// The Hosts window's list chose another host; its ports' list,
    /// another port.
    HostsChosen,
    PortsChosen,
    /// The Forward Port window: Forward (true), or Cancel.
    ForwardDone(bool),
    ForwardKeep(bool),
    /// The Add Host window: Add and Connect (true), or Cancel.
    AddHostDone(bool),
    /// How Rex runs on the host being added, and whether to set it up.
    AddHostMode(usize),
    AddHostSetUp(bool),
    /// The host being set up stands otherwise (`setup::current`).
    SetupChanged,
    /// The move under way stands otherwise.
    MoveChanged,
    /// An account moved: the host it was on, the one it is on, its sessions
    /// there by their old ids.
    HostMoved(String, String, std::collections::HashMap<String, String>),
    /// What the Move window asks of the move.
    MoveDo(panels::MoveDo),
    /// A busy bar's block moves on.
    SetupTick,
    SetupDo(panels::SetupDo),
    SetupKey(usize),
    /// The Server window: open it on this host (its key); the host
    /// looked after stands otherwise (`server::current`); a button of
    /// it; the key chosen for root, the page, the person to let in, how
    /// much of the log, the row chosen in its list.
    /// The Host window: opened on a host (its key); it shows otherwise;
    /// one of its buttons.
    HostOpen(String),
    /// The Account window opened on a host (the skill's mark).
    AccountOpen(String),
    HostChanged,
    HostDo(crate::host_panel::HostDo),
    AccountChanged,
    AccountDo(crate::account_panel::AccountDo),
    ReposChosen,
    ReposDo(crate::repos::ReposDo),
    /// The repository form: done (true), or cancelled.
    RepoEditDone(bool),
    /// The Agent Settings conflict review window.
    CarryChanged,
    CarryChangesChosen,
    CarryRemChosen,
    CarryDo(crate::carry_panel::CarryDo),
    ServerChanged,
    ServerDo(crate::server_panel::ServerDo),
    ServerKey(usize),
    ServerPage(usize),
    ServerPerson(usize),
    ServerRange(usize),
    ServerChosen,
    HostsForwardFrom(String),
    AskpassConfirm(bool),
    AskpassSubmit,
    RunApproveChoice(bool),
    RunApproveChanged,
    /// The Rename window: Rename (true) or Cancel; and another thing to
    /// name while it is open.
    RenameDone(bool),
    RenameAgain,
    /// A session's row asked to be renamed (its host, its id).
    RenameSession(String, String),
    /// A tab's menu's Rename (its host, its session, the tab).
    RenameTab(String, String, String),
    /// A tab's menu asked for (which tab, where the pointer is).
    TabMenu(usize, Point<Pixels>),
    ActivityChanged,
    ToggleActivityFold(BlockKey),
    OpenSubagent(BlockKey, olo_rex_client::activity::SubagentInfo, bool),
    OpenPaneTranscript(BlockKey, String),
    TranscriptDo(crate::transcript_view::TranscriptDo),
    SidebarMoved,
    CloseWindow,
}

/// One window of Olo Rex (either a main Workspace window or a secondary dialog panel).
pub struct RexWindow {
    pub id: WinId,
    pub role: WinRole,
    pub focus: FocusHandle,
    pub active_host: String,
    pub active_session: Option<String>,
    session_index: usize,
    pending_focus: Option<BlockKey>,
    created_session: Option<(String, String)>,
    qr_url: Option<String>,
    qr: Option<Arc<qrcode::QrCode>>,
    preview_job: Option<tokio::task::AbortHandle>,
    feedback_serial: u64,
    problem: Option<(String, Option<Child<widgets::TextView>>)>,
    notice: olo_kit::due::Notice,
    due: olo_kit::due::Due,
    pub side_pane: SidePane,
    sidebar_scroll: olo_kit::ScrollHandle,
    run_scroll: olo_kit::ScrollHandle,
    transcript_scroll: olo_kit::ScrollHandle,
    tmux_scroll: olo_kit::ScrollHandle,
    last_transcript_gen: u64,
    tmux_visual: Rc<crate::terminal_pane::PaneVisualState>,
    pub panes_view: Option<Child<PanesView>>,
    pub overlay: Option<ActiveOverlay>,
    pub overlay_field: Option<Child<TextField>>,
    pub find_open: bool,
    /// The key of the command key's chord just handed to the menu bar,
    /// while it is still down (its repeats are swallowed).
    pub command_chord: Option<String>,
    pub find_field: Option<Child<TextField>>,
    pub host_label_field: Option<Child<TextField>>,
    pub host_addr_field: Option<Child<TextField>>,
    pub fwd_port_field: Option<Child<TextField>>,
    /// The host a port typed in the Hosts window is forwarded from (its key).
    pub fwd_host: Option<String>,
    /// Whether the Forward Port dialog's "Keep when Rex restarts" checkbox is ticked.
    pub fwd_keep: bool,
    /// The Hosts window's list: its state, its rows as last said (the
    /// list draws from them), and the host chosen in it (its key).
    /// Add Host: how Rex runs on the host, and whether to set it up.
    add_mode: olo_rex_client::ServerMode,
    add_set_up: bool,
    /// While the host being set up is waited for (looked at, root
    /// signed in), and in the Server window while its host is: the
    /// timer that moves its busy bar, and where the bar is. None, and
    /// nothing drawn, at any other time.
    setup_tick: Option<olo_kit::TimerId>,
    setup_phase: f32,
    pub hosts_list: widgets::ListModel,
    pub hosts_rows: Rc<Vec<panels::HostRow>>,
    pub hosts_chosen: Option<String>,
    /// And its list of ports, the same way (the one chosen by what it is).
    pub ports_list: widgets::ListModel,
    pub ports_rows: Rc<Vec<panels::PortRow>>,
    pub ports_chosen: Option<panels::PortKind>,
    /// The Server window's list of accounts (or members): its state,
    /// its rows as last said, and the one chosen (its name); and its
    /// Log page's text, with what it was last filled from.
    pub server_list: widgets::ListModel,
    /// The Repositories window's list and its rows, and the form's
    /// three fields (address, folder, set-up).
    pub repos_list: widgets::ListModel,
    pub repos_rows: Rc<Vec<crate::repos::RepoRow>>,
    repo_fields: Option<[Child<TextField>; 3]>,
    /// The Agent Settings batch review window's lists, rows, and preview scrollers.
    pub carry_changes_list: widgets::ListModel,
    pub carry_changes_rows: Rc<Vec<crate::carry_panel::ChangeRow>>,
    pub carry_rem_list: widgets::ListModel,
    pub carry_rem_rows: Rc<Vec<crate::carry_panel::RememberedRow>>,
    carry_here_scroll: olo_kit::ScrollHandle,
    carry_there_scroll: olo_kit::ScrollHandle,
    pub server_rows: Rc<Vec<crate::server_panel::PersonRow>>,
    server_chosen: Option<String>,
    server_log: Option<Child<widgets::TextView>>,
    server_log_key: u64,
    pub askpass_field: Option<Child<TextField>>,
    pub preedit: Option<String>,
    pub window_active: bool,
    pub dropping: bool,
    pub unread_marked: bool,
    /// The look the terminals' colours were last made for: its mode and
    /// its own terminal colours' id (another theme in the same mode is
    /// another look).
    pub last_look: Option<(olo_theme::ThemeMode, String)>,
}

fn selected_block(session: &olo_rex_client::SessionView) -> Option<&str> {
    let win = session.active_window().or_else(|| session.windows.first())?;
    win.focused_block_id.as_deref().filter(|id| win.all_blocks().any(|b| b.block_id == *id)).or_else(|| win.all_blocks().next().map(|b| b.block_id.as_str()))
}

fn connection_age(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0 => olo_kit::words::tr("Connection lost just now.").into(),
        1..60 => olo_kit::words::count(minutes, "Connection lost 1 minute ago.", "Connection lost {n} minutes ago."),
        60..1440 => olo_kit::words::count(minutes / 60, "Connection lost 1 hour ago.", "Connection lost {n} hours ago."),
        _ => olo_kit::words::count(minutes / 1440, "Connection lost 1 day ago.", "Connection lost {n} days ago."),
    }
}

fn selected_session_index(rows: &[olo_rex_client::SessionSummary], selected: Option<&str>, previous: usize) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    Some(selected.map_or(0, |id| rows.iter().position(|r| r.session_id == id).unwrap_or_else(|| previous.saturating_sub(1).min(rows.len() - 1))))
}

#[derive(Default)]
struct PaneVisuals(HashMap<BlockKey, Rc<PaneVisualState>>);

impl RexWindow {
    fn clear_feedback(&mut self) {
        self.created_session = None;
        self.feedback_serial = 0;
        self.problem = None;
        self.notice.clear();
    }

    fn feedback_line(&mut self, cx: &mut olo_kit::Cx<Self>) -> Option<Element> {
        self.due.at(self.notice.until(), cx, |_, cx| cx.invalidate());
        if let Some((title, detail)) = &self.problem {
            let scale = cx.scale_factor();
            let frame = 4.0 * style::line_pixels(scale) / scale;
            return Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .w_full()
                    .child(StringView::new(title.clone()).color(style::c(theme::palette().failure)).wrap(true))
                    .children(detail.as_ref().map(|view| div().w_full().h(px(widgets::TextView::line_height() + 2.0 * widgets::TextView::MARGINS.1 + frame)).child(view))),
            );
        }
        self.notice.text().map(|s| div().child(StringView::new(s.clone()).size(style::SMALL_SIZE).height(m::LABEL_ROW)))
    }

    fn new_workspace(id: WinId, restored: Option<SavedWorkspaceState>, cx: &mut olo_kit::Cx<Self>) -> Self {
        let (side_w, side_col) = {
            let hub = app().hub.clone();
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            (restored.as_ref().map(|r| r.sidebar_width).unwrap_or(st.settings.sidebar_width), restored.as_ref().map(|r| r.sidebar_collapsed).unwrap_or(st.settings.sidebar_collapsed))
        };
        // (No narrower than its control's two names and a count whole:
        // at 160 "Activity" was cut once an agent waited.)
        let side_pane = SidePane::new(side_w, SIDE_LEAST).first(Front::Rest).restored(Some(side_w), side_col);

        let focus = FocusHandle::new();
        focus.focus(cx);

        let overlay_field = widgets::text_field(cx, |f| f.name("Filter").hint("Type to filter…"));
        cx.subscribe(&overlay_field, |w: &mut Self, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                let txt = w.overlay_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default();
                if let Some(ov) = &mut w.overlay {
                    match ov {
                        ActiveOverlay::RexPrompt { query, selected_idx, .. } => {
                            *query = txt;
                            *selected_idx = 0;
                        }
                    }
                }
                cx.invalidate();
            }
            TextFieldEvent::Entered => {
                w.confirm_overlay(cx);
            }
            TextFieldEvent::Typed(_) => {}
        });

        let find_field = widgets::text_field(cx, |f| f.name("Find").hint("Find in terminal…"));
        cx.subscribe(&find_field, |w: &mut Self, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => w.run_find_query(cx),
            TextFieldEvent::Entered => w.step_find(false, cx),
            TextFieldEvent::Typed(_) => {}
        });

        let panes_view = cx.new_view(|_| PanesView { win_id: id, session: None, width: None });

        // (The demo's other windows, opened as it starts, for the live
        // lint and for pictures: `OLO_REX_DEMO_OPEN=app.hosts`, the
        // commands that open them, with commas between. The lint
        // reviews the windows an app shows; shown none of these, it
        // passed a row of buttons in Hosts and Ports that was wider
        // than its window, 2026-11-23.)
        if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1")
            && restored.is_none()
            && let Ok(open) = std::env::var("OLO_REX_DEMO_OPEN")
        {
            for command in open.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                cx.send(WinMsg::Command(command.to_owned()));
            }
        }

        Self {
            id,
            role: WinRole::Workspace,
            focus,
            active_host: restored.as_ref().map(|r| r.active_host.clone()).unwrap_or_else(|| "local".to_owned()),
            session_index: restored.as_ref().map_or(0, |r| r.session_index),
            active_session: restored.and_then(|r| r.active_session),
            side_pane,
            pending_focus: None,
            sidebar_scroll: olo_kit::ScrollHandle::new(),
            run_scroll: olo_kit::ScrollHandle::new(),
            transcript_scroll: olo_kit::ScrollHandle::new(),
            tmux_scroll: olo_kit::ScrollHandle::new(),
            last_transcript_gen: 0,
            tmux_visual: Rc::new(crate::terminal_pane::PaneVisualState::default()),
            created_session: None,
            qr_url: None,
            qr: None,
            preview_job: None,
            feedback_serial: 0,
            problem: None,
            notice: Default::default(),
            due: Default::default(),
            panes_view: Some(panes_view),
            overlay: None,
            overlay_field: Some(overlay_field),
            find_open: false,
            command_chord: None,
            find_field: Some(find_field),
            host_label_field: None,
            host_addr_field: None,
            fwd_port_field: None,
            fwd_host: None,
            fwd_keep: false,
            add_mode: Default::default(),
            add_set_up: true,
            setup_tick: None,
            setup_phase: 0.0,
            hosts_list: widgets::ListModel::new(false),
            hosts_rows: Rc::default(),
            hosts_chosen: None,
            ports_list: widgets::ListModel::new(false),
            ports_rows: Rc::default(),
            ports_chosen: None,
            server_list: widgets::ListModel::new(false),
            repos_list: widgets::ListModel::new(false),
            repos_rows: Rc::default(),
            repo_fields: None,
            carry_changes_list: widgets::ListModel::new(true),
            carry_changes_rows: Rc::default(),
            carry_rem_list: widgets::ListModel::new(true),
            carry_rem_rows: Rc::default(),
            carry_here_scroll: olo_kit::ScrollHandle::new(),
            carry_there_scroll: olo_kit::ScrollHandle::new(),
            server_rows: Rc::default(),
            server_chosen: None,
            server_log: None,
            server_log_key: 0,
            askpass_field: None,
            preedit: None,
            window_active: cx.is_window_active(),
            dropping: false,
            unread_marked: false,
            last_look: None,
        }
    }

    fn new_panel(id: WinId, role: WinRole, cx: &mut olo_kit::Cx<Self>) -> Self {
        let focus = FocusHandle::new();
        let (host_label_field, host_addr_field, fwd_port_field, askpass_field) = match role {
            WinRole::Forward => {
                let pf = widgets::text_field(cx, |f| f.name("Port").hint("8080"));
                pf.focus(cx);
                cx.subscribe(&pf, |w: &mut Self, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => w.submit_forward(cx),
                    TextFieldEvent::Changed => cx.invalidate(),
                    _ => {}
                });
                (None, None, Some(pf), None)
            }
            WinRole::Move => {
                // (A made-up move at a stage, for the window's pictures
                // and its lint; in the demo only.)
                if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1")
                    && let Ok(stage) = std::env::var("OLO_REX_DEMO_MOVE")
                {
                    let stage = stage.strip_prefix('@').map_or(stage.clone(), |file| std::fs::read_to_string(file).unwrap_or_default().trim().to_owned());
                    if crate::moving::show_demo(&stage) {
                        cx.send(WinMsg::MoveChanged);
                    }
                }
                (None, None, None, None)
            }
            WinRole::AddHost => {
                // (Each with the name agents know it by. The address
                // first: it is what a host must have, and what is typed.)
                // (A made-up host at a stage of being set up, for the
                // window's pictures and its lint; in the demo only.)
                if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1")
                    && let Ok(stage) = std::env::var("OLO_REX_DEMO_SETUP")
                {
                    // (`@FILE`: the stage named in that file, read each
                    // time the window opens: one run pictures them all.)
                    let stage = stage.strip_prefix('@').map_or(stage.clone(), |file| std::fs::read_to_string(file).unwrap_or_default().trim().to_owned());
                    // (Told as a real one is: a stage that waits moves its bar.)
                    if crate::setup::show_demo(&stage) {
                        cx.send(WinMsg::SetupChanged);
                    }
                }
                let af = widgets::text_field(cx, |f| f.name("Address").hint("ssh://you@host or host.ts.net"));
                let lf = widgets::text_field(cx, |f| f.name("Name"));
                af.focus(cx);
                cx.subscribe(&af, |w: &mut Self, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => w.submit_add_host(cx),
                    TextFieldEvent::Changed => {
                        // (The name it will go by with none given, shown
                        // in the empty Name field as the address is typed.)
                        let address = w.host_addr_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default();
                        if let Some(name) = &w.host_label_field {
                            name.update(cx, |f, cx| f.set_hint(crate::hub::host_name_from(&address), cx));
                        }
                        cx.invalidate();
                    }
                    _ => {}
                });
                cx.subscribe(&lf, |w: &mut Self, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        w.submit_add_host(cx);
                    }
                });
                (Some(lf), Some(af), None, None)
            }
            WinRole::Rename => {
                let (was, hint) = app().renaming.as_ref().map(|r| (r.was.clone(), r.hint())).unwrap_or_default();
                let f = widgets::text_field(cx, |f| f.name("Name").with_text(was).hint(hint));
                f.update(cx, |f, cx| f.select_all_text(cx));
                f.focus(cx);
                cx.subscribe(&f, |w: &mut Self, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => w.submit_rename(cx),
                    // (Rename is offered or not by what is typed.)
                    TextFieldEvent::Changed => cx.invalidate(),
                    _ => {}
                });
                (None, None, None, Some(f))
            }
            WinRole::Askpass => {
                let is_secret = app().askpass.current_request().is_some_and(|r| matches!(r, AskpassRequest::Secret { echo: false, .. }));
                let f = widgets::text_field(cx, |f| f.name("Answer").password(is_secret).hint("Enter secret…"));
                f.focus(cx);
                cx.subscribe(&f, |w: &mut Self, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        w.submit_askpass(cx);
                    }
                });
                (None, None, None, Some(f))
            }
            _ => {
                focus.focus(cx);
                (None, None, None, None)
            }
        };
        // (The Server window: its log's text, and, in the demo, a made-up
        // host's server at a stage, for the window's pictures and its
        // lint: `OLO_REX_DEMO_SERVER=STAGE`, or `@FILE` naming it.)
        let server_log = (role == WinRole::Server).then(|| widgets::text_view(cx, |v| v.mono(true).size(style::SMALL_SIZE).wrap(false).clear_top(true).empty_text("Reading the log…")));
        if role == WinRole::Server {
            if crate::server::demo_mode()
                && let Ok(stage) = std::env::var("OLO_REX_DEMO_SERVER")
            {
                let stage = stage.strip_prefix('@').map_or(stage.clone(), |file| std::fs::read_to_string(file).unwrap_or_default().trim().to_owned());
                crate::server::show_demo(&stage);
            }
            // (Told as a real one is: a stage that waits moves its bar.)
            cx.send(WinMsg::ServerChanged);
        }
        // (Hosts and Ports: the keyboard in its hosts' list from the
        // start, for the arrows and a name typed.)
        let hosts_list = widgets::ListModel::new(false);
        if role == WinRole::Hosts {
            hosts_list.focus(cx);
        }
        let repos_list = widgets::ListModel::new(false);
        if role == WinRole::Repos {
            repos_list.focus(cx);
        }
        let carry_changes_list = widgets::ListModel::new(true);
        let carry_rem_list = widgets::ListModel::new(true);
        if role == WinRole::Carry {
            if crate::carry::current().is_some_and(|s| s.tab == 1) {
                carry_rem_list.focus(cx);
            } else {
                carry_changes_list.focus(cx);
            }
        }
        let repo_fields = (role == WinRole::RepoEdit).then(|| {
            let was = crate::repos::editing().and_then(|i| app().hub.state.lock().unwrap_or_else(|e| e.into_inner()).repos.get(i).cloned()).unwrap_or_default();
            let address = widgets::text_field(cx, |f| f.name("Address").hint("git@github.com:you/project.git").with_text(was.url.clone()));
            // (The folder as the list writes it, "~/…"; each hint what
            // an empty field comes to: where it goes by its name, and
            // no command.)
            let place = |repo: &olo_rex_client::repos::Repo| format!("~/{}", repo.place());
            let folder = widgets::text_field(cx, |f| {
                f.name("Folder")
                    .hint(if was.url.is_empty() { "~/src/project".to_owned() } else { place(&olo_rex_client::repos::Repo { folder: String::new(), ..was.clone() }) })
                    .with_text(if was.folder.is_empty() { String::new() } else { place(&was) })
            });
            let setup = widgets::text_field(cx, |f| f.name("Set up with").hint("Nothing").with_text(was.setup.clone()));
            address.focus(cx);
            for field in [&address, &folder, &setup] {
                cx.subscribe(field, |w: &mut Self, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => cx.send(WinMsg::RepoEditDone(true)),
                    TextFieldEvent::Changed => {
                        // (The folder an empty field comes to, as its hint.)
                        if let Some(fields) = &w.repo_fields {
                            let typed = crate::repos::typed(cx, fields);
                            let place = if typed.url.is_empty() || typed.wrong().is_some() {
                                "~/src/project".to_owned()
                            } else {
                                format!("~/{}", olo_rex_client::repos::Repo { folder: String::new(), ..typed }.place())
                            };
                            fields[1].update(cx, |f, cx| f.set_hint(place, cx));
                        }
                        cx.invalidate();
                    }
                    _ => {}
                });
            }
            [address, folder, setup]
        });

        Self {
            id,
            role,
            focus,
            active_host: "local".to_owned(),
            active_session: None,
            session_index: 0,
            side_pane: SidePane::new(220.0, 168.0),
            pending_focus: None,
            sidebar_scroll: olo_kit::ScrollHandle::new(),
            run_scroll: olo_kit::ScrollHandle::new(),
            transcript_scroll: olo_kit::ScrollHandle::new(),
            tmux_scroll: olo_kit::ScrollHandle::new(),
            last_transcript_gen: 0,
            tmux_visual: Rc::new(crate::terminal_pane::PaneVisualState::default()),
            created_session: None,
            qr_url: None,
            qr: None,
            preview_job: None,
            feedback_serial: 0,
            problem: None,
            notice: Default::default(),
            due: Default::default(),
            panes_view: None,
            overlay: None,
            overlay_field: None,
            find_open: false,
            command_chord: None,
            find_field: None,
            host_label_field,
            host_addr_field,
            fwd_port_field,
            fwd_host: None,
            fwd_keep: false,
            add_mode: Default::default(),
            add_set_up: true,
            setup_tick: None,
            setup_phase: 0.0,
            hosts_list,
            hosts_rows: Rc::default(),
            hosts_chosen: None,
            ports_list: widgets::ListModel::new(false),
            ports_rows: Rc::default(),
            ports_chosen: None,
            server_list: widgets::ListModel::new(false),
            repos_list,
            repos_rows: Rc::default(),
            repo_fields,
            carry_changes_list,
            carry_changes_rows: Rc::default(),
            carry_rem_list,
            carry_rem_rows: Rc::default(),
            carry_here_scroll: olo_kit::ScrollHandle::new(),
            carry_there_scroll: olo_kit::ScrollHandle::new(),
            server_rows: Rc::default(),
            server_chosen: None,
            server_log,
            server_log_key: 0,
            askpass_field,
            preedit: None,
            window_active: cx.is_window_active(),
            dropping: false,
            unread_marked: false,
            last_look: None,
        }
    }

    /// Where this window is, for its status bar at rest: its host, and
    /// its session if it shows one ("Local · main").
    /// What the status bar says at rest: the folder and the size of the
    /// pane the keyboard is in, which nothing else in the window says
    /// (it said the host and the session: the sidebar's selected row,
    /// again); the host and session where no pane is.
    /// What the status bar says, at its far end, of a host that has
    /// been silent about something typed into the pane the keyboard is
    /// in: the link has stopped, or the host has. What was typed is
    /// still drawn; this is how it is known not to have arrived.
    /// What the host of the pane the keyboard is in has of Olo's (its
    /// typing helper, its bridge, the skill for agents): the status
    /// bar's three marks, for a far host.
    fn host_has(&mut self) -> Option<crate::hub::HostHas> {
        let hub = app().hub.clone();
        let host = self.focused_block_key().map_or_else(|| self.active_host.clone(), |focused| focused.host_key);
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_has(&host)
    }

    fn silent_host(&mut self) -> Option<String> {
        let hub = app().hub.clone();
        let focused = self.focused_block_key()?;
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(&focused.host_key)?;
        Some(crate::hub::silent_said(&host.label, host.is_local, st.host_silent(&focused.host_key)?))
    }

    fn where_it_is(&mut self) -> String {
        if let Some(tr) = crate::activity_watch::active_transcript() {
            let g = tr.lock().unwrap_or_else(|e| e.into_inner());
            return g.target.status_bar_text(g.last_timestamp());
        }
        if let Some(tm) = crate::activity_watch::active_tmux() {
            let g = tm.lock().unwrap_or_else(|e| e.into_inner());
            return g.target.status_bar_text(None);
        }
        let hub = app().hub.clone();
        let focused = self.focused_block_key();
        let (block, local, far) = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            match &focused {
                Some(bk) => {
                    let host = st.hosts.get(&bk.host_key);
                    let local = host.is_some_and(|h| h.is_local);
                    (st.blocks.get(bk).cloned(), local, host.filter(|_| !local).map(|h| h.label.clone()).unwrap_or_default())
                }
                None => (None, false, String::new()),
            }
        };
        if let Some(block) = block {
            let g = block.lock().unwrap_or_else(|e| e.into_inner());
            let (columns, rows) = g.terminal.size();
            // (This computer's home is known, and said as a shell says
            // it; a pane on another host says which first.)
            let home = if local { crate::hub::local_home() } else { None };
            let home = g.process.as_ref().and_then(|p| p.home.clone()).or(home);
            let folder = crate::hub::folder_said(&g.pwd, home.as_deref());
            let size = olo_kit::tr!("{} × {}", columns, rows).to_string();
            return [far.as_str(), folder.as_str(), size.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        }
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(host) = st.hosts.get(&self.active_host) else { return String::new() };
        let session =
            self.active_session.as_deref().and_then(|id| host.session_summaries.iter().find(|s| s.session_id == id)).map(|s| if s.label.is_empty() { s.session_id.as_str() } else { s.label.as_str() });
        match session {
            Some(session) => olo_kit::tr!("{} · {}", host.label, session).to_string(),
            None => host.label.clone(),
        }
    }

    fn resolve_active_session(&mut self) -> Option<(String, olo_rex_client::SessionView)> {
        let hub = app().hub.clone();
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(&self.active_host).or_else(|| st.hosts.get("local"))?;
        self.active_host.clone_from(&host.key);
        // Keep a restored selection until bootstrap has supplied the host's list.
        if host.conn_state != HostConnState::Connected {
            return None;
        }
        let index = selected_session_index(&host.session_summaries, self.active_session.as_deref(), self.session_index);
        let Some(index) = index else {
            self.active_session = None;
            self.session_index = 0;
            return None;
        };
        self.session_index = index;
        let sid = &host.session_summaries[index].session_id;
        if self.active_session.as_ref() != Some(sid) {
            self.active_session = Some(sid.clone());
        }
        Some((host.key.clone(), host.sessions.get(sid)?.clone()))
    }

    fn focused_block_key(&mut self) -> Option<BlockKey> {
        let (hk, sess) = self.resolve_active_session()?;
        let bid = selected_block(&sess)?.to_owned();
        Some(BlockKey { host_key: hk, session_id: sess.session_id, block_id: bid })
    }

    fn cancel_preview(&mut self) {
        if let Some(job) = self.preview_job.take() {
            job.abort();
        }
    }

    /// Quick Look: the file opened in Olo's app for it, copied from its
    /// host first (`RexHub::quick_look_path`).
    fn open_preview(&mut self, bk: BlockKey, path: String) {
        self.cancel_preview();
        self.preview_job = app().hub.quick_look_path(&bk, path, Feedback::start(self.id));
    }

    fn dismiss_overlay(&mut self, cx: &mut olo_kit::Cx<Self>) {
        self.cancel_preview();
        if let Some(ActiveOverlay::RexPrompt { host_key, req, .. }) = self.overlay.take() {
            app().hub.respond_prompt(&host_key, &req.call_id, true, None);
        }
        self.focus.focus(cx);
        cx.invalidate();
    }

    fn confirm_overlay(&mut self, cx: &mut olo_kit::Cx<Self>) {
        let ov = self.overlay.take();
        match ov {
            Some(ActiveOverlay::RexPrompt { host_key, req, query, selected_idx, checked_ids }) => {
                let val = if req.prompt.kind == "pick" {
                    if req.prompt.multiple {
                        let ids: Vec<String> = checked_ids.into_iter().collect();
                        json!({ "ids": ids })
                    } else {
                        let items = req.prompt.items.as_deref().unwrap_or(&[]);
                        let filtered: Vec<&olo_rex_client::PromptItem> = items.iter().filter(|it| overlays::fuzzy_score(&it.title, &query).is_some()).collect();
                        if let Some(chosen) = filtered.get(selected_idx) {
                            json!({ "id": chosen.id })
                        } else if req.prompt.allow_other {
                            json!({ "other": query })
                        } else {
                            Value::Null
                        }
                    }
                } else {
                    json!({ "value": query })
                };
                app().hub.respond_prompt(&host_key, &req.call_id, false, Some(val));
            }
            None => {}
        }
        self.focus.focus(cx);
        cx.invalidate();
    }

    fn pick_overlay_index(&mut self, index: usize, cx: &mut olo_kit::Cx<Self>) {
        if let Some(ActiveOverlay::RexPrompt { host_key, req, .. }) = &mut self.overlay {
            let hk = host_key.clone();
            let cid = req.call_id.clone();
            let chosen = req.prompt.items.as_ref().and_then(|v| v.get(index)).map(|i| i.id.clone());
            self.overlay = None;
            if let Some(id) = chosen {
                app().hub.respond_prompt(&hk, &cid, false, Some(json!({ "id": id })));
            }
            self.focus.focus(cx);
            cx.invalidate();
        }
    }

    fn jump_to(&mut self, target: JumpTarget, cx: &mut olo_kit::Cx<Self>) {
        match target {
            JumpTarget::Host(hk) => {
                self.active_host = hk;
                self.active_session = None;
            }
            JumpTarget::Session { host_key, session_id } => {
                self.active_host = host_key;
                self.active_session = Some(session_id);
            }
            JumpTarget::Window { host_key, session_id, window_id } => {
                self.active_host = host_key.clone();
                self.active_session = Some(session_id.clone());
                let hub = app().hub.clone();
                hub.select_window(&host_key, &session_id, &window_id);
            }
            JumpTarget::Pane(bk) => {
                self.active_host = bk.host_key.clone();
                self.active_session = Some(bk.session_id.clone());
                let hub = app().hub.clone();
                hub.focus_block(&bk.host_key, &bk.session_id, &bk.block_id);
            }
            JumpTarget::Directory(dir) => {
                if let Some(bk) = self.focused_block_key() {
                    app().hub.send_input(&bk, format!("cd {}\r", crate::hub::shell_quote(&dir)).into_bytes());
                }
            }
        }
        cx.invalidate();
    }

    fn run_find_query(&mut self, cx: &mut olo_kit::Cx<Self>) {
        let q = self.find_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default();
        if let Some(tr) = crate::activity_watch::active_transcript() {
            let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
            g.search_query = q;
            g.match_idx = None;
            g.recompute_search();
            g.generation += 1;
            cx.invalidate();
            return;
        }
        if let Some(bk) = self.focused_block_key() {
            let hub = app().hub.clone();
            hub.find(&bk, (!q.is_empty()).then_some(q.as_str()));
        }
        cx.invalidate();
    }

    fn step_find(&mut self, back: bool, cx: &mut olo_kit::Cx<Self>) {
        if let Some(tr) = crate::activity_watch::active_transcript() {
            let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
            let _ = g.step_search(back);
            g.generation += 1;
            cx.invalidate();
            return;
        }
        if let Some(bk) = self.focused_block_key() {
            let hub = app().hub.clone();
            if let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                if g.finding {
                    return;
                }
                if back {
                    let _ = g.search.select_prev();
                } else {
                    let _ = g.search.select_next();
                }
            }
        }
        cx.invalidate();
    }

    /// The Rename window's name, given: the session or the tab is
    /// renamed (as typed, less the space round it; nothing typed takes
    /// its name away), and how it went is said in the window it was
    /// asked from.
    /// The hosts a port can be forwarded from (key, name): those other
    /// than this computer (a port of it is here already) that have a
    /// session to forward through.
    fn forward_hosts(&self) -> Vec<(String, String)> {
        let hub = app().hub.clone();
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.host_order.iter().filter_map(|k| st.hosts.get(k)).filter(|h| !h.is_local && !h.session_summaries.is_empty()).map(|h| (h.key.clone(), h.label.clone())).collect()
    }

    /// The Forward Port window's port, given: forwarded here from the
    /// host chosen (the first, with none chosen), through a session of
    /// it. Forwarded, not opened: a port typed may be a database's
    /// ("Forward and Open" is for one a program in a pane is seen to
    /// listen on). How it went is said in the workspace it was asked
    /// from, and its row in Hosts says where it is here.
    /// The person's repositories but the one the form is changing: what
    /// a folder typed there mustn't be another's.
    fn repo_others(&self) -> Vec<olo_rex_client::repos::Repo> {
        let repos = app().hub.state.lock().unwrap_or_else(|e| e.into_inner()).repos.clone();
        let editing = crate::repos::editing();
        repos.into_iter().enumerate().filter(|(i, _)| Some(*i) != editing).map(|(_, repo)| repo).collect()
    }

    fn submit_forward(&mut self, cx: &mut olo_kit::Cx<Self>) {
        let port = self.fwd_port_field.as_ref().and_then(|f| f.read(cx).text().trim().parse::<u16>().ok()).filter(|p| *p > 0);
        let from = self.forward_hosts();
        let key = self.fwd_host.clone().filter(|k| from.iter().any(|(key, _)| key == k)).or_else(|| from.first().map(|(k, _)| k.clone()));
        let (Some(port), Some(key)) = (port, key) else { return };
        let hub = app().hub.clone();
        let session = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&key).and_then(|h| h.session_summaries.first().map(|s| s.session_id.clone()));
        if let Some(session) = session {
            hub.add_local_forward(&key, &session, "127.0.0.1".to_owned(), port, 0, false, self.fwd_keep, Feedback::start(self.id));
        }
        cx.request_close();
    }

    /// The Add Host window's host, given: added (to Preferences ▸ Rex's
    /// hosts, as every host is) and connected to; it goes by the host in
    /// its address where it was given no name. Nothing without an address.
    /// What the address typed into Add Host is.
    fn add_host_address(&self, cx: &olo_kit::Cx<Self>) -> crate::setup::Address {
        crate::setup::address(self.host_addr_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default().as_str())
    }

    /// Adds the host typed: one reached over SSH is set up first, if
    /// that is wanted (the window stays, and shows how it goes), and
    /// added at the address its kind of server is reached at.
    fn submit_add_host(&mut self, cx: &mut olo_kit::Cx<Self>) {
        use crate::setup::Address;
        let typed = |f: &Option<Child<TextField>>| f.as_ref().map(|f| f.read(cx).text().trim().to_owned()).unwrap_or_default();
        let (name, address) = (typed(&self.host_label_field), typed(&self.host_addr_field));
        let name = if name.is_empty() { crate::hub::host_name_from(&address) } else { name };
        let hub = app().hub.clone();
        let endpoint = match crate::setup::address(&address) {
            Address::Empty | Address::Unfit(_) => return,
            Address::Other => address,
            Address::Ssh(target) if self.add_set_up => {
                crate::setup::begin(&hub, name, target, self.add_mode);
                cx.invalidate();
                return;
            }
            Address::Ssh(target) => olo_rex_client::provision::address(self.add_mode, &target),
        };
        hub.add_and_connect_host(format!("host-{}", uuid::Uuid::new_v4().simple()), name, endpoint, true);
        cx.request_close();
    }

    /// Asks for a session's new name.
    fn rename_session(&self, host_key: &str, session_id: &str) {
        let label = {
            let hub = app().hub.clone();
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).and_then(|h| h.session_summaries.iter().find(|s| s.session_id == session_id)).map(|s| s.label.clone()).unwrap_or_default()
        };
        let shown = if label.is_empty() { session_id.to_owned() } else { label.clone() };
        open_rename(Renaming { host_key: host_key.to_owned(), session_id: session_id.to_owned(), window_id: None, was: label, shown, from: self.id });
    }

    /// Asks for a tab's name: the one it was given, if any, to change;
    /// what it shows as now, to say which tab.
    fn rename_tab(&self, host_key: &str, session: &olo_rex_client::SessionView, window_id: &str) {
        let Some(i) = session.windows.iter().position(|w| w.window_id == window_id) else { return };
        let was = crate::hub::tab_label(&session.windows[i]).unwrap_or_default().to_owned();
        let shown = {
            let hub = app().hub.clone();
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            crate::hub::session_tab_names(&st, host_key, session).into_iter().nth(i).unwrap_or_default()
        };
        open_rename(Renaming { host_key: host_key.to_owned(), session_id: session.session_id.clone(), window_id: Some(window_id.to_owned()), was, shown, from: self.id });
    }

    fn submit_rename(&mut self, cx: &mut olo_kit::Cx<Self>) {
        let name = self.askpass_field.as_ref().map(|f| f.read(cx).text().trim().to_owned()).unwrap_or_default();
        // (Taken in a statement of its own: held through the `if`, the
        // app's lock was asked for again under it, which a debug build
        // refuses and a release build waits on for ever.)
        // (A name it can't have, a session's empty one: Return does
        // nothing, as its button, which is off, does nothing.)
        if app().renaming.as_ref().is_some_and(|r| !r.takes(&name)) {
            return;
        }
        let renaming = app().renaming.take();
        if let Some(r) = renaming
            && name != r.was
        {
            let hub = app().hub.clone();
            match &r.window_id {
                Some(tab) => hub.rename_window(&r.host_key, &r.session_id, tab, name, Feedback::start(r.from)),
                None => hub.rename_session(&r.host_key, &r.session_id, name, Feedback::start(r.from)),
            }
        }
        cx.request_close();
    }

    fn submit_askpass(&mut self, cx: &mut olo_kit::Cx<Self>) {
        let text = self.askpass_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default();
        if let Some(f) = &self.askpass_field {
            f.update(cx, |f, cx| f.clear_secret(cx));
        }
        app().askpass.answer(AskpassReply::Secret(Some(text)));
        cx.request_close();
    }

    /// Execute any of the ~105 Rex actions cleanly.
    pub fn execute_action(&mut self, action: &str, args: Option<Value>, cx: &mut olo_kit::Cx<Self>) {
        let hub = app().hub.clone();
        match action {
            "app.view_activity" => {
                {
                    let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.settings.sidebar_mode = SidebarMode::Activity;
                    st.settings.sidebar_collapsed = false;
                    st.settings.save();
                }
                crate::activity_watch::sync_watchers(&hub, true);
                self.side_pane.show_pane();
                cx.invalidate();
            }
            "app.view_sessions" => {
                {
                    let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.settings.sidebar_mode = SidebarMode::Sessions;
                    st.settings.sidebar_collapsed = false;
                    st.settings.save();
                }
                crate::activity_watch::sync_watchers(&hub, false);
                self.side_pane.show_pane();
                cx.invalidate();
            }
            "pane.view:transcript" => {
                if let Some(bk) = self.focused_block_key() {
                    let title = pane_name(&hub, &bk);
                    crate::activity_watch::open_pane_transcript(&hub, &bk, title);
                }
            }
            "pane.view:terminal" => {
                if let Some(crate::activity_watch::ViewTarget::SubagentTranscript { parent, sub }) = crate::activity_watch::active_view_target()
                    && sub.tmux.is_some()
                {
                    crate::activity_watch::open_subagent(&hub, &parent, sub, false);
                } else {
                    crate::activity_watch::close_viewer();
                }
            }
            c if c.starts_with("activity.open.transcript:") => {
                let id = c.trim_start_matches("activity.open.transcript:");
                if let Some((parent, sub)) = crate::activity_watch::find_subagent_by_id(id) {
                    crate::activity_watch::open_subagent(&hub, &parent, sub, true);
                }
            }
            c if c.starts_with("activity.open.terminal:") => {
                let id = c.trim_start_matches("activity.open.terminal:");
                if let Some((parent, sub)) = crate::activity_watch::find_subagent_by_id(id)
                    && sub.tmux.is_some()
                {
                    crate::activity_watch::open_subagent(&hub, &parent, sub, false);
                }
            }
            c if c.starts_with("activity.open:") => {
                let id = c.trim_start_matches("activity.open:");
                if let Some((parent, sub)) = crate::activity_watch::find_subagent_by_id(id) {
                    crate::activity_watch::open_subagent(&hub, &parent, sub, false);
                }
            }
            c if c.starts_with("activity.fold:") => {
                let bid = c.trim_start_matches("activity.fold:");
                let found = {
                    let mgr = crate::activity_watch::manager().lock().unwrap_or_else(|e| e.into_inner());
                    mgr.by_block.keys().find(|bk| bk.block_id == bid).cloned()
                };
                if let Some(bk) = found {
                    crate::activity_watch::toggle_fold(&bk);
                }
            }
            // Where the tabs are, said outright (it was one command that
            // went round the four).
            "tabs.top" | "tabs.side" => {
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.toolbar_layout = if action == "tabs.side" { ToolbarLayout::Side } else { ToolbarLayout::Above };
                st.settings.save();
                cx.invalidate();
            }
            // Pane headers shown or not (the third state, "on hover", had
            // nothing behind it: it was the same as hidden).
            "panes.headers" => {
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.pane_header_behavior = if st.settings.pane_header_behavior == BarBehavior::Always { BarBehavior::Hidden } else { BarBehavior::Always };
                st.settings.save();
                cx.invalidate();
            }
            "app.toggle_sidebar" => {
                self.side_pane.toggle();
                let collapsed = self.side_pane.collapsed();
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.sidebar_collapsed = collapsed;
                st.settings.save();
                cx.invalidate();
            }
            "app.settings" => olo_kit::app_menu::open_settings("rex"),
            "app.hosts" => open_or_raise_panel(WinRole::Hosts),
            "app.add_host" => open_or_raise_panel(WinRole::AddHost),
            "host.manage" => open_host(&hub, &self.active_host),
            "host.move" => open_move(&hub, &self.active_host),
            c if c.starts_with("host.move:") => open_move(&hub, c.trim_start_matches("host.move:")),
            // (What the pane's host may do here: Preferences ▸ Rex, on
            // Hosts, at that host; as its window's Permissions… does.)
            "host.permissions" => olo_kit::app_menu::open_settings(&format!("rex/host:{}", self.active_host)),
            // (A host named by its key: for agents, and the window's pictures.)
            // (`host.manage:KEY`, or `host.manage:KEY:TAB` on a tab of it by
            // name, `rex`, `olo` or `files`: agents reach each tab as the
            // person does, its page being hidden to them until chosen.)
            c if c.starts_with("host.manage:") => {
                let rest = c.trim_start_matches("host.manage:");
                let (key, tab) = match rest.rsplit_once(':').and_then(|(k, t)| crate::host::tab_named(t).map(|i| (k, i))) {
                    Some((key, tab)) => (key, Some(tab)),
                    None => (rest, None),
                };
                open_host(&hub, key);
                if let Some(tab) = tab {
                    crate::host::show_tab(tab);
                }
            }
            // (A host's account made ready: its window; and, for
            // agents, one of its items done there by its name, or all
            // that is left: `host.ready:KEY:path|claude|codex|agy|git|
            // skill|all`, `host.ready.stop`. In the Account window,
            // where it shows.)
            "host.account" => open_account(&hub, &self.active_host),
            c if c.starts_with("host.account:") => open_account(&hub, c.trim_start_matches("host.account:")),
            // (Agent settings batch conflict review & per-item agent API:
            // `host.carry:KEY` opens & inspects differences, exposing them as JSON
            // on the window's `carry-diff` node; `host.carry.choose:KEY:{JSON}` sets
            // choices; `host.carry.apply:KEY` or `host.carry.apply:KEY:{JSON}` applies;
            // `host.carry.forget:KEY` or `host.carry.forget:KEY:ITEM_ID` forgets.)
            "host.carry" => open_carry(&hub, &self.active_host),
            c if c.starts_with("host.carry:") => open_carry(&hub, c.trim_start_matches("host.carry:")),
            c if c.starts_with("host.carry.choose:") => {
                let rest = c.trim_start_matches("host.carry.choose:");
                if let Some((key, json)) = rest.split_once(':')
                    && crate::carry::current().is_some_and(|s| s.key == key)
                    && let Ok(batch) = serde_json::from_str::<crate::carry::AgentChoiceBatch>(json)
                {
                    crate::carry::apply_agent_batch(&batch);
                }
            }
            c if c.starts_with("host.carry.apply:") => {
                let rest = c.trim_start_matches("host.carry.apply:");
                let (key, json) = match rest.split_once(':') {
                    Some((k, j)) => (k, Some(j)),
                    None => (rest, None),
                };
                if crate::carry::current().is_some_and(|s| s.key == key) {
                    if let Some(j) = json
                        && let Ok(batch) = serde_json::from_str::<crate::carry::AgentChoiceBatch>(j)
                    {
                        crate::carry::apply_agent_batch(&batch);
                    }
                    crate::carry::apply(&hub);
                }
            }
            c if c.starts_with("host.carry.forget:") => {
                let rest = c.trim_start_matches("host.carry.forget:");
                match rest.split_once(':') {
                    Some((key, item_id)) if !item_id.is_empty() => crate::carry::forget(&hub, Some(key), Some(&[item_id.to_owned()])),
                    Some((key, _)) => crate::carry::forget(&hub, Some(key), None),
                    None => crate::carry::forget(&hub, Some(rest), None),
                }
            }
            "host.ready.stop" => crate::account::stop(&hub),
            // (How far back the agents' sessions copied go: days, 0 for
            // all of them, as the Account window's pop-up chooses.)
            c if c.starts_with("host.sessions.days:") => {
                if let Ok(days) = c.trim_start_matches("host.sessions.days:").parse::<u32>() {
                    hub.set_sessions_window(olo_rex_client::sessions::Window::of_days(days));
                }
            }
            // (The person's repositories: their window, and the form
            // for a new one or for the one at a place in the list.)
            "host.repos" => open_or_raise_panel(WinRole::Repos),
            "host.repos.add" => {
                crate::repos::edit(None);
                open_or_raise_panel(WinRole::RepoEdit);
            }
            c if c.starts_with("host.repos.edit:") => {
                if let Ok(i) = c.trim_start_matches("host.repos.edit:").parse::<usize>() {
                    crate::repos::edit(Some(i));
                    open_or_raise_panel(WinRole::RepoEdit);
                }
            }
            c if c.starts_with("host.ready:") => {
                let mut parts = c.trim_start_matches("host.ready:").rsplitn(2, ':');
                if let (Some(what), Some(key)) = (parts.next(), parts.next()) {
                    open_account(&hub, key);
                    let items: Vec<crate::account::Item> = match (what, crate::account::Item::of(what)) {
                        (_, Some(item)) => vec![item],
                        // (All: those not there already, as far as is
                        // known; the rest are asked for by name.)
                        ("all", _) => {
                            let stands = hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_stands(key);
                            let who = crate::account::current().and_then(|shown| shown.who);
                            // (Not the sessions, which are copied by their
                            // own command alone: `Item::Sessions`.)
                            crate::account::Item::all().filter(|item| item.counted() && stands.as_ref().and_then(|s| item.done(s, who.as_ref())) != Some(true)).collect()
                        }
                        _ => Vec::new(),
                    };
                    crate::account::run(&hub, &items);
                }
            }
            // (One of Olo's things put on a host or taken off, for
            // agents: `host.put:KEY:helper|bridge|skill`, `host.take:…`.
            // In the Host window, where it shows.)
            c if c.starts_with("host.put:") || c.starts_with("host.take:") => {
                let add = c.starts_with("host.put:");
                let rest = c.split_once(':').map_or("", |(_, rest)| rest);
                let thing = rest.rsplit_once(':').and_then(|(key, thing)| {
                    let thing = match thing {
                        "helper" => crate::hub::HostThing::Helper,
                        "skill" => crate::hub::HostThing::Skill,
                        "pool" => crate::hub::HostThing::Pool,
                        _ => return None,
                    };
                    Some((key.to_owned(), thing))
                });
                if let Some((key, thing)) = thing {
                    open_host(&hub, &key);
                    if crate::host::current().is_some_and(|shown| shown.key == key) {
                        crate::host::change(&hub, thing, add);
                    }
                }
            }
            "host.server" => open_server(&hub, &self.active_host),
            // (A host named by its key: for agents, and the window's pictures.)
            c if c.starts_with("host.server:") => open_server(&hub, c.trim_start_matches("host.server:")),
            "app.forward_port" => open_or_raise_panel(WinRole::Forward),
            "app.pair_device" => open_or_raise_panel(WinRole::Access),
            "app.diagnostics" => open_or_raise_panel(WinRole::Diagnostics),
            "app.new_window" => open_workspace_window(None),
            "app.close_window" => cx.request_close(),
            "app.quit" => olo_kit::quit(),
            "app.reload_config" => hub.reload_server_config(&self.active_host, Feedback::start(self.id)),
            "host.reconnect" => hub.connect_host(&self.active_host),
            // (A host let go of, and connected again, by its key: what
            // Hosts and Ports' one button does, for agents.)
            c if c.starts_with("host.disconnect:") => hub.disconnect_host(c.trim_start_matches("host.disconnect:")),
            c if c.starts_with("host.connect:") => hub.connect_host(c.trim_start_matches("host.connect:")),
            "session.new" => hub.create_session(&self.active_host, None, None, Feedback::start(self.id)),
            "session.close" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.close_session(&hk, &sess.session_id, Feedback::start(self.id));
                }
            }
            "session.rename" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    self.rename_session(&hk, &sess.session_id);
                }
            }
            "window.rename" => {
                if let Some((hk, sess)) = self.resolve_active_session()
                    && let Some(w) = sess.active_window()
                {
                    self.rename_tab(&hk, &sess, &w.window_id.clone());
                }
            }
            "window.move_left" | "window.move_right" => {
                if let Some((hk, sess)) = self.resolve_active_session()
                    && let Some(cur) = sess.active_window_id.as_deref().and_then(|id| sess.windows.iter().position(|w| w.window_id == id))
                {
                    let ids: Vec<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
                    let to = if action == "window.move_left" { cur.checked_sub(1) } else { Some(cur + 1) };
                    if let Some(before) = to.and_then(|to| crate::hub::moved_before(&ids, cur, to)) {
                        hub.move_window(&hk, &sess.session_id, &ids[cur], before);
                    }
                }
            }
            "window.new" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.create_window(&hk, &sess.session_id, None);
                } else {
                    hub.create_session(&self.active_host, None, None, Feedback::start(self.id));
                }
            }
            "window.close" => {
                if let Some((hk, sess)) = self.resolve_active_session()
                    && let Some(w) = sess.active_window()
                {
                    hub.close_window(&hk, &sess.session_id, &w.window_id);
                }
            }
            "window.next" | "window.previous" => {
                if let Some((hk, sess)) = self.resolve_active_session()
                    && sess.windows.len() > 1
                {
                    let cur = sess.active_window_id.as_deref().and_then(|id| sess.windows.iter().position(|w| w.window_id == id)).unwrap_or(0);
                    let next = if action == "window.next" { (cur + 1) % sess.windows.len() } else { (cur + sess.windows.len() - 1) % sess.windows.len() };
                    hub.select_window(&hk, &sess.session_id, &sess.windows[next].window_id);
                }
            }
            a if a.starts_with("window.select_") => {
                if let Ok(n) = a.trim_start_matches("window.select_").parse::<usize>()
                    && n >= 1
                    && let Some((hk, sess)) = self.resolve_active_session()
                    && let Some(w) = sess.windows.get(n - 1)
                {
                    hub.select_window(&hk, &sess.session_id, &w.window_id);
                }
            }
            "pane.split_right" | "pane.split_down" | "pane.split_best_fit" => {
                if let Some(bk) = self.focused_block_key() {
                    let dir = match action {
                        "pane.split_right" => "right",
                        "pane.split_down" => "down",
                        _ => {
                            let rect = cx.window_state::<PaneVisuals>().0.get(&bk).map(|v| v.bounds.get());
                            if rect.is_some_and(|r| r.size.width >= r.size.height) { "right" } else { "down" }
                        }
                    };
                    hub.split_pane(&bk.host_key, &bk.session_id, &bk.block_id, dir, None);
                }
            }
            "pane.close" => {
                if let Some(bk) = self.focused_block_key() {
                    hub.close_block(&bk.host_key, &bk.session_id, &bk.block_id);
                }
            }
            "pane.focus_left" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.focus_direction(&hk, &sess.session_id, "left");
                }
            }
            "pane.focus_right" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.focus_direction(&hk, &sess.session_id, "right");
                }
            }
            "pane.focus_up" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.focus_direction(&hk, &sess.session_id, "up");
                }
            }
            "pane.focus_down" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.focus_direction(&hk, &sess.session_id, "down");
                }
            }
            "pane.zoom_toggle" => {
                if let Some(bk) = self.focused_block_key() {
                    hub.toggle_zoom(&bk.host_key, &bk.session_id, Some(bk.block_id));
                }
            }
            "pane.broadcast_toggle" => {
                if let Some(key) = self.active_window_key() {
                    let mut a = app();
                    if !a.broadcast_windows.remove(&key) {
                        a.broadcast_windows.insert(key);
                    }
                }
                cx.invalidate();
            }
            "layer.new_floating" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.create_floating_layer(&hk, &sess.session_id, [0.16, 0.14, 0.68, 0.68]);
                }
            }
            "terminal.copy" => {
                if let Some(bk) = self.focused_block_key() {
                    let sel = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        st.blocks.get(&bk).and_then(|br| br.lock().unwrap_or_else(|e| e.into_inner()).terminal.selection_text().ok().flatten())
                    };
                    if let Some(txt) = sel.filter(|s| !s.is_empty()) {
                        olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(txt));
                        cx.invalidate();
                    }
                }
            }
            "terminal.paste" => {
                let Some(bk) = self.focused_block_key() else { return };
                let Some(connection) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&bk.host_key).and_then(|h| h.control.clone()) else { return };
                let to = cx.messenger();
                olo_kit::clipboard::paste_item(move |item| {
                    if let Some(text) = item.and_then(|i| i.text()).filter(|s| !s.is_empty()) {
                        to.send(WinMsg::PasteText(bk, connection, text));
                        return;
                    }
                    // No text: a picture, put on the pane's host as a
                    // file of its own and its path typed, as a file
                    // dropped on the pane is (Claude Code and Codex take
                    // a pasted picture's path as the picture: an agent
                    // there can't read this computer's clipboard). Ben,
                    // 2026-11-24: "make it so that copy and paste of
                    // images works properly in a rex session".
                    olo_kit::clipboard::paste_as(vec!["image/png".into(), "image/".into()], move |got| {
                        if let Some((mime, bytes)) = got {
                            to.send(WinMsg::PasteImage(bk, mime, bytes));
                        }
                    });
                });
            }
            "terminal.select_all" => {
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let _ = br.lock().unwrap_or_else(|e| e.into_inner()).terminal.select_all();
                    cx.invalidate();
                }
            }
            "terminal.select_output" => {
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    let (cx_pos, cy_pos) = g.render_state.cursor().viewport_pos.unwrap_or((0, 0));
                    let _ = g.terminal.select_output_at_viewport(cx_pos, cy_pos as u32);
                    cx.invalidate();
                }
            }
            "terminal.clear_scrollback" => {
                if let Some(bk) = self.focused_block_key() {
                    if let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                        br.lock().unwrap_or_else(|e| e.into_inner()).terminal.vt_write(b"\x1b[2J\x1b[3J\x1b[H");
                    }
                    cx.invalidate();
                }
            }
            "terminal.reset" => {
                if let Some(bk) = self.focused_block_key() {
                    if let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                        br.lock().unwrap_or_else(|e| e.into_inner()).terminal.reset();
                    }
                    cx.invalidate();
                }
            }
            "terminal.find" => {
                self.find_open = true;
                if let Some(f) = &self.find_field {
                    if let Some(tr) = crate::activity_watch::active_transcript() {
                        let q = tr.lock().unwrap_or_else(|e| e.into_inner()).search_query.clone();
                        if !q.is_empty() && f.read(cx).text() != q {
                            f.update(cx, |tf, cx| tf.set_text(q, cx));
                        }
                    }
                    f.focus(cx);
                }
                cx.invalidate();
            }
            "terminal.find_next" => self.step_find(false, cx),
            "terminal.find_previous" => self.step_find(true, cx),
            "terminal.copy_mode" => {
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    if g.copy_mode.is_some() {
                        g.copy_mode = None;
                        let _ = g.terminal.clear_selection();
                    } else {
                        let (cx_pos, cy_pos) = g.render_state.cursor().viewport_pos.unwrap_or((0, 0));
                        g.copy_mode = Some(CopyMode::enter(&g.terminal, cx_pos, cy_pos));
                    }
                }
                cx.invalidate();
            }
            "terminal.scroll_page_up" | "terminal.scroll_page_down" | "terminal.scroll_top" | "terminal.scroll_bottom" | "terminal.prompt_prev" | "terminal.prompt_next" => {
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    let rows = g.terminal.size().1 as isize;
                    match action {
                        "terminal.scroll_page_up" => {
                            let _ = g.terminal.scroll_delta(-rows.max(1));
                        }
                        "terminal.scroll_page_down" => {
                            let _ = g.terminal.scroll_delta(rows.max(1));
                        }
                        "terminal.scroll_top" => {
                            let _ = g.terminal.scroll_to_top();
                        }
                        "terminal.scroll_bottom" => {
                            let _ = g.terminal.scroll_to_bottom();
                        }
                        "terminal.prompt_prev" => {
                            let _ = g.terminal.scroll_prompt_delta(-1);
                        }
                        "terminal.prompt_next" => {
                            let _ = g.terminal.scroll_prompt_delta(1);
                        }
                        _ => {}
                    }
                }
                cx.invalidate();
            }
            "terminal.font_increase" | "terminal.font_decrease" | "terminal.font_reset" => {
                let (new_settings, new_theme) = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    let mut s = st.settings.clone();
                    match action {
                        "terminal.font_increase" => s.font_size = (s.font_size + 1.0).min(crate::settings::TEXT_SIZES.1),
                        "terminal.font_decrease" => s.font_size = (s.font_size - 1.0).max(crate::settings::TEXT_SIZES.0),
                        // (The size Rex comes with: Default went to 12, and Rex comes with 10.5.)
                        _ => s.font_size = crate::settings::TEXT_SIZE,
                    }
                    let th = themes::resolve_theme(&s, &theme::palette());
                    (s, th)
                };
                hub.update_settings(new_settings, new_theme);
                cx.invalidate();
            }
            // The colours, as Terminal's Colors menu: the look's mode
            // followed or one mode always; and for each mode the look's
            // own colours (followed) or one of Olo's, which stays.
            c if c.starts_with("scheme.") || c.starts_with("colors.") => {
                let (new_settings, new_theme) = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    let mut s = st.settings.clone();
                    match c {
                        "scheme.auto" => s.scheme_choice = SchemeChoice::Auto,
                        "scheme.light" => s.scheme_choice = SchemeChoice::Light,
                        "scheme.dark" => s.scheme_choice = SchemeChoice::Dark,
                        _ => {
                            let Some((slot, id)) = c.strip_prefix("colors.").and_then(|c| c.split_once(':')) else { return };
                            if !id.is_empty() && olo_theme::looks::terminal(id).is_none() {
                                return;
                            }
                            let id = Some(id.to_owned()).filter(|id| !id.is_empty());
                            if slot == "dark" { s.dark_theme_id = id } else { s.light_theme_id = id }
                        }
                    }
                    let th = themes::resolve_theme(&s, &theme::palette());
                    (s, th)
                };
                hub.update_settings(new_settings, new_theme);
                cx.invalidate();
            }
            "activity.include_idle" => {
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.activity_include_idle = !st.settings.activity_include_idle;
                st.settings.save();
                cx.invalidate();
            }
            "terminal.export_text" | "terminal.export_html" => {
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let kind = if action == "terminal.export_html" { FormatKind::Html } else { FormatKind::Plain };
                    if let Ok(text) = br.lock().unwrap_or_else(|e| e.into_inner()).terminal.format(kind, false, true) {
                        olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(text));
                    }
                }
            }
            "files.quick_look" => {
                if let Some(bk) = self.focused_block_key() {
                    let selected_path = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        st.blocks.get(&bk).and_then(|br| {
                            let g = br.lock().unwrap_or_else(|e| e.into_inner());
                            g.terminal.selection_text().ok().flatten().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
                        })
                    };
                    if let Some(p) = selected_path {
                        self.open_preview(bk, p);
                        cx.invalidate();
                    }
                }
            }
            "files.open_in_tracker" => {
                if let Some(bk) = self.focused_block_key() {
                    open_block_in_tracker(&hub, &bk);
                }
            }
            "agent.keep_going_toggle" => {
                if let Some(bk) = self.focused_block_key() {
                    hub.set_help(&bk, |h| h.keep_going = !h.keep_going);
                    cx.invalidate();
                }
            }
            "agent.auto_approve_toggle" => {
                if let Some(bk) = self.focused_block_key() {
                    toggle_auto_approve(&hub, &bk, &pane_name(&hub, &bk));
                    cx.invalidate();
                }
            }
            "agent.auto_approve_stop" => {
                if let Some(bk) = self.focused_block_key() {
                    hub.stop_auto_approve(&bk);
                }
            }
            "agent.jump_next_blocked" => {
                let found = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.blocks
                        .iter()
                        .find(|(_, br)| {
                            let a = br.lock().unwrap_or_else(|e| e.into_inner()).agent_activity;
                            a == AgentActivity::Blocked || a == AgentActivity::Error
                        })
                        .map(|(bk, _)| bk.clone())
                };
                if let Some(bk) = found {
                    self.jump_to(JumpTarget::Pane(bk), cx);
                }
            }
            "agent.mark_all_read" => {
                let blocks: Vec<_> = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.values().cloned().collect();
                for br in blocks {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    if g.unread {
                        g.unread = false;
                        g.unread_cleared_at = Some(Instant::now());
                    }
                }
                cx.invalidate();
            }
            "client.mode.enter" => {
                if let Some(mode) = args.as_ref().and_then(|a| a.get("mode")).and_then(Value::as_str) {
                    let once = args.as_ref().and_then(|a| a.get("once")).and_then(Value::as_bool).unwrap_or(false);
                    app().keymap.enter_mode(mode.to_owned(), once);
                    cx.invalidate();
                }
            }
            "client.mode.exit" => {
                app().keymap.exit_mode();
                cx.invalidate();
            }
            "client.mode.exit_all" => {
                app().keymap.exit_all_modes();
                cx.invalidate();
            }
            // (On a host reached over SSH: put there over SSH, in the
            // Host window, where it shows and what the host says of it
            // does. On any other, the script is typed into the pane,
            // the one way there.)
            "olo.install_remote_skill" => {
                if let Some(bk) = self.focused_block_key() {
                    let over_ssh = far_host(&hub, &bk.host_key) && hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_stands(&bk.host_key).is_some_and(|h| h.ssh);
                    if over_ssh {
                        open_host(&hub, &bk.host_key);
                        crate::host::change(&hub, crate::hub::HostThing::Skill, true);
                    } else {
                        hub.install_remote_skill_in_pane(&bk);
                    }
                }
            }
            "olo.passwords.fill" => {
                if let Some(bk) = self.focused_block_key() {
                    let hub_c = hub.clone();
                    let bk_c = bk.clone();
                    let _ = std::thread::Builder::new().name("rex-op-fill".into()).spawn(move || {
                        if let Ok(secret) = bridge::fetch_op_reference("op://Personal/Rex/password", "Fill password into Rex pane") {
                            hub_c.send_input(&bk_c, format!("{secret}\r").into_bytes());
                        }
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_terminal_key(&mut self, e: &KeyDownEvent, cx: &mut olo_kit::Cx<Self>) {
        // Shortcut recorder inside Settings → Keybindings.
        if self.role != WinRole::Workspace {
            return;
        }

        // Overlay navigation keys take top priority when an overlay is open.
        if self.overlay.is_some() {
            match e.keystroke.key.as_str() {
                "escape" => {
                    self.dismiss_overlay(cx);
                    cx.stop_propagation();
                    return;
                }
                "up" => {
                    if let Some(ActiveOverlay::RexPrompt { selected_idx, .. }) = &mut self.overlay {
                        *selected_idx = selected_idx.saturating_sub(1);
                    }
                    cx.invalidate();
                    cx.stop_propagation();
                    return;
                }
                "down" => {
                    if let Some(ActiveOverlay::RexPrompt { selected_idx, .. }) = &mut self.overlay {
                        *selected_idx += 1;
                    }
                    cx.invalidate();
                    cx.stop_propagation();
                    return;
                }
                _ => return,
            }
        }

        if self.find_open && self.find_field.as_ref().is_some_and(|f| f.is_focused(cx)) {
            return;
        }

        // Check Copy Mode first!
        if let Some(bk) = self.focused_block_key() {
            let hub = app().hub.clone();
            if let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                let cm_action = {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(mut cm) = g.copy_mode.take() {
                        let res = cm.handle_key(&mut g.terminal, &e.keystroke.key, e.keystroke.key_char.as_deref(), e.keystroke.modifiers.control, e.keystroke.modifiers.shift);
                        match res {
                            CopyModeAction::Continue => {
                                g.copy_mode = Some(cm);
                                Some(CopyModeAction::Continue)
                            }
                            other => {
                                let _ = g.terminal.clear_selection();
                                Some(other)
                            }
                        }
                    } else {
                        None
                    }
                };
                if let Some(act) = cm_action {
                    if let CopyModeAction::Yank(txt) = act {
                        olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(txt));
                    }
                    cx.invalidate();
                    cx.stop_propagation();
                    return;
                }
            }
        }

        // Whose key it is, in this order (Terminal's, with one layer
        // more: the person's own Rex configuration):
        // 1. part way through one of the configuration's sequences, or
        //    in one of its modes: the configuration's, if it has it;
        // 2. the command key with a menu shortcut's key (or one of the
        //    profile's keys for Rex that takes no command key: Shift+F3
        //    with Windows' keys): Rex's own command, left to the
        //    window's menu bar. By the menus as things are this instant
        //    (BeOS's MenusBeginning): Copy is off without a selection,
        //    and a shortcut whose item is off is the shell's, so ⌘C with
        //    nothing selected interrupts, as it does in Terminal;
        // 3. a key the configuration binds (a prefix key, a chord);
        // 4. Shift with Page Up, Page Down, Home and End: the
        //    scrollback (⌘↑ and ⌘↓ arrive as the last two);
        // 5. Ctrl+Tab and Ctrl+Page Up/Down with more tabs than one:
        //    the tabs, as in other terminals;
        // 6. the program's in the pane: the command key's as Control's
        //    (⌘R searches a shell's history like Ctrl+R), without the
        //    Shift the command key brings to a terminal.
        let k = &e.keystroke;
        // A key kept down after the command key's chord it began was let
        // go (⌘D, then D held): its repeats are that chord's still, and
        // nothing for the shell. (A repeat says Control as its press had
        // it and the command key as it is now: it read as Ctrl+D, the
        // end of the shell, in every pane the split had just made.)
        if e.is_held && self.command_chord.as_deref() == Some(k.key.as_str()) {
            cx.stop_propagation();
            return;
        }
        if !e.is_held {
            self.command_chord = None;
        }
        let command = keymap::command_held(k);
        let cmd = keymap::cmd_held(k);
        let engaged = app().keymap.engaged();
        let bound = engaged || !app().keymap.is_empty();
        let ask_configuration = |w: &mut Self, cx: &mut olo_kit::Cx<Self>| -> bool {
            let km_match = app().keymap.handle_key(k, cmd);
            match km_match {
                KeyMatch::Execute { action, args } => {
                    w.execute_action(&action, args, cx);
                    cx.stop_propagation();
                    true
                }
                KeyMatch::Pending { .. } | KeyMatch::Blocked => {
                    cx.invalidate();
                    cx.stop_propagation();
                    true
                }
                // (No key of its: whose it is goes on being asked below.)
                KeyMatch::Passthrough => false,
            }
        };
        if engaged && ask_configuration(self, cx) {
            return;
        }
        if command || own_plain_shortcut(k) {
            let menus = self.build_menu_bar();
            olo_kit::menu::menus_beginning(cx, menus);
            if olo_kit::menu::shortcut_command(cx, k).is_some() {
                if command {
                    self.command_chord = Some(k.key.clone());
                }
                return;
            }
        }
        if bound && !engaged && ask_configuration(self, cx) {
            return;
        }
        if k.modifiers.shift && !k.modifiers.control && !k.modifiers.alt {
            let scroll = match k.key.as_str() {
                "pageup" => Some("terminal.scroll_page_up"),
                "pagedown" => Some("terminal.scroll_page_down"),
                "home" => Some("terminal.scroll_top"),
                "end" => Some("terminal.scroll_bottom"),
                _ => None,
            };
            if let Some(action) = scroll {
                self.execute_action(action, None, cx);
                cx.stop_propagation();
                return;
            }
        }
        let step = olo_kit::tab_step(k);
        if step != 0 && self.resolve_active_session().is_some_and(|(_, s)| s.windows.len() > 1) {
            self.execute_action(if step > 0 { "window.next" } else { "window.previous" }, None, cx);
            cx.stop_propagation();
            return;
        }

        // While a read-only transcript or read-only tmux subagent view is open,
        // never leak keyboard input to the hidden parent terminal; Escape closes it.
        if crate::activity_watch::active_view_target().is_some() {
            if k.key == "escape" && !self.find_open {
                crate::activity_watch::close_viewer();
                cx.stop_propagation();
            }
            return;
        }

        // To the pane the keyboard is in (to every pane of the tab while Type in All Panes is on).
        let Some(bk) = self.focused_block_key() else { return };
        let targets = self.input_target_blocks(&bk);
        let hub = app().hub.clone();

        // (What the pane's program asked of its keys.)
        let modes = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).map_or_else(Default::default, |b| {
            let b = b.lock().unwrap_or_else(|e| e.into_inner());
            olo_kit::term_keys::Modes { app_cursor: b.terminal.dec_mode(1), kitty: b.terminal.kitty_keyboard_flags() != 0 }
        });
        if let Some(bytes) = encode_terminal_special_key(k, modes).filter(|_| !cmd) {
            for t in targets {
                hub.send_input(&t, bytes.clone());
            }
            cx.stop_propagation();
        } else if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            let rex_ev = keymap::to_rex_key_event(e, cmd);
            for t in targets {
                hub.send_rex_key(&t, rex_ev.clone());
            }
            cx.stop_propagation();
        } else if let Some(ch) = e.keystroke.key_char.as_deref().filter(|s| !s.is_empty()) {
            let bytes = ch.as_bytes().to_vec();
            for t in targets {
                hub.send_input(&t, bytes.clone());
            }
            cx.stop_propagation();
        }
    }

    fn active_window_key(&mut self) -> Option<(String, String, String)> {
        let (host, session) = self.resolve_active_session()?;
        let id = session.active_window().or_else(|| session.windows.first())?.window_id.clone();
        Some((host, session.session_id, id))
    }

    fn input_target_blocks(&mut self, focused: &BlockKey) -> Vec<BlockKey> {
        let is_broadcast = self.active_window_key().is_some_and(|key| app().broadcast_windows.contains(&key));
        if !is_broadcast {
            return vec![focused.clone()];
        }
        if let Some((hk, sess)) = self.resolve_active_session()
            && let Some(w) = sess.active_window()
        {
            return w.all_blocks().map(|b| BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() }).collect();
        }
        vec![focused.clone()]
    }

    /// The menu bar, on Terminal's model (apps/terminal): the app's own
    /// menu first (its panels in Preferences, what is made new, what
    /// closes), Edit, View, Tabs, Pane, Agents, and a Settings menu for
    /// this window's quick choices. An item's keys are the keyboard
    /// profile's for its command (`olo_protocol::app_keys`, the entries
    /// for Rex), given by the kit. Every toggle shows its check and every
    /// item is enabled only when it can act. (It was Rex, File, Edit,
    /// View, Pane, Tools as the app came: items that cycled through
    /// states one couldn't see, "Toggle …" with no check, "Export" that
    /// copied, and Copy enabled with nothing selected.)
    fn build_menu_bar(&mut self) -> Vec<Menu> {
        let hub = app().hub.clone();
        let broadcast_on = self.active_window_key().is_some_and(|key| app().broadcast_windows.contains(&key));
        let sidebar_shown = self.side_pane.showing();
        let (sidebar_mode, layout, headers, text_size, include_idle, scheme, light, dark) = {
            let s = &hub.state.lock().unwrap_or_else(|e| e.into_inner()).settings;
            (s.sidebar_mode, s.toolbar_layout, s.pane_header_behavior, s.font_size, s.activity_include_idle, s.scheme_choice, s.light_theme_id.clone(), s.dark_theme_id.clone())
        };
        // For each mode: the look's own colours (whatever Preferences ▸
        // Themes has, followed as it changes), or one chosen here, which
        // stays: any theme's. (Terminal's Colors menu, the same.)
        let look = theme::palette();
        let pick = |title: &str, mode: olo_theme::ThemeMode| {
            let chosen = if mode == olo_theme::ThemeMode::Dark { dark.as_deref() } else { light.as_deref() }.filter(|id| olo_theme::looks::terminal(id).is_some());
            let slot = if mode == olo_theme::ThemeMode::Dark { "dark" } else { "light" };
            let menu = Menu::new(title.to_owned()).item(olo_kit::tr!("As in Themes ({})", look.terminal_in(mode).name), format!("colors.{slot}:")).checked(chosen.is_none()).separator();
            olo_theme::looks::terminals(&look.theme, mode).iter().fold(menu, |m, t| m.item(&t.name, format!("colors.{slot}:{}", t.id)).checked(chosen == Some(t.id.as_str())))
        };
        let colors = Menu::new("Colors")
            .item("Match Appearance", "scheme.auto")
            .checked(scheme == SchemeChoice::Auto)
            .item("Always Light", "scheme.light")
            .checked(scheme == SchemeChoice::Light)
            .item("Always Dark", "scheme.dark")
            .checked(scheme == SchemeChoice::Dark)
            .separator()
            .submenu(pick("Light Colors", olo_theme::ThemeMode::Light))
            .submenu(pick("Dark Colors", olo_theme::ThemeMode::Dark));
        // The pane the keyboard is in, and what of it the menus say.
        let focused = self.focused_block_key();
        let pane = focused.is_some();
        let block = focused.as_ref().and_then(|key| hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned());
        let help = self.focused_block_key().map(|bk| hub.help(&bk)).unwrap_or_default();
        let (keep_going, selected, copying, found) = block.as_ref().map_or((false, false, false, false), |block| {
            let g = block.lock().unwrap_or_else(|e| e.into_inner());
            (help.keep_going, g.terminal.has_selection(), g.copy_mode.is_some(), g.search.total_matches() > 0)
        });
        let session = self.resolve_active_session().map(|(_, s)| s);
        let window = session.as_ref().and_then(|s| s.active_window().or_else(|| s.windows.first()).map(|w| (w.zoomed_block_id.is_some(), w.all_blocks().count())));
        let (zoomed, panes) = window.unwrap_or((false, 0));
        let tab = window.is_some();
        // The session's tabs by name, the first nine with their keys
        // (⌘1 to ⌘9), as Terminal's Tabs menu.
        let count = session.as_ref().map_or(0, |s| s.windows.len());
        let at = session.as_ref().and_then(|s| s.active_window_id.as_deref().and_then(|id| s.windows.iter().position(|w| w.window_id == id)));
        let mut tabs = Menu::new("Tabs")
            .item("New Tab", "window.new")
            .item("Rename Tab…", "window.rename")
            .enabled(tab)
            .item("Close Tab", "window.close")
            .enabled(tab)
            .separator()
            .item("Next Tab", "window.next")
            .enabled(count > 1)
            .item("Previous Tab", "window.previous")
            .enabled(count > 1)
            .separator()
            .item("Move Tab Left", "window.move_left")
            .enabled(at.is_some_and(|i| i > 0))
            .item("Move Tab Right", "window.move_right")
            .enabled(at.is_some_and(|i| i + 1 < count));
        if let Some(s) = &session {
            let active = s.active_window().or_else(|| s.windows.first()).map(|w| w.window_id.clone());
            tabs = tabs.separator();
            let names = crate::hub::session_tab_names(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), &self.active_host, s);
            for (i, (w, name)) in s.windows.iter().zip(names).enumerate() {
                tabs = tabs.item(name, format!("window.select_{}", i + 1)).checked(Some(&w.window_id) == active.as_ref());
            }
        }
        vec![
            Menu::new("Rex")
                .app_items(&["rex", "notifications"])
                .item("New Window", "app.new_window")
                .item("New Session", "session.new")
                .item("Rename Session…", "session.rename")
                .enabled(session.is_some())
                .separator()
                .item("Close Window", "close")
                .item("Quit", "quit"),
            Menu::new("Edit")
                .item("Copy", "terminal.copy")
                .enabled(selected)
                .item("Paste", "terminal.paste")
                .enabled(pane)
                .item("Paste Password…", "olo.passwords.fill")
                .enabled(pane)
                .separator()
                .item("Select All", "terminal.select_all")
                .enabled(pane)
                .item("Select Last Output", "terminal.select_output")
                .enabled(pane)
                .separator()
                .item("Find…", "terminal.find")
                .enabled(pane)
                // (With something found: off, ⌘G is the shell's Ctrl+G.)
                .item("Find Again", "terminal.find_next")
                .enabled(found)
                .item("Find Previous", "terminal.find_previous")
                .enabled(found)
                .separator()
                .item("Copy Mode", "terminal.copy_mode")
                .checked(copying)
                .enabled(pane)
                .item("Copy Screen as Text", "terminal.export_text")
                .enabled(pane)
                .item("Copy Screen as HTML", "terminal.export_html")
                .enabled(pane)
                .separator()
                .item("Clear Scrollback", "terminal.clear_scrollback")
                .enabled(pane)
                .item("Reset Terminal", "terminal.reset")
                .enabled(pane),
            Menu::new("View")
                .item("Show Sidebar", "app.toggle_sidebar")
                .checked(sidebar_shown)
                .item("Show Pane Headers", "panes.headers")
                .checked(headers == BarBehavior::Always)
                .item("Show Idle Panes in Activity", "activity.include_idle")
                .checked(include_idle)
                .separator()
                .item("Sessions", "app.view_sessions")
                .checked(sidebar_mode == SidebarMode::Sessions)
                .item("Activity", "app.view_activity")
                .checked(sidebar_mode == SidebarMode::Activity),
            // The hosts: all of them; the one of the pane in front (its
            // window, its account, what it may do here); its Rex server;
            // then this computer's own, as others reach it. After View,
            // before Tabs: a host holds sessions, a session tabs, a tab
            // panes. (They were in the first menu, and the account in
            // Agents: Ben, 2026-11-24, "all the different rex prefs and
            // settings and stuff has got really confusing and
            // disorganized".)
            Menu::new("Hosts")
                .item("Add Host…", "app.add_host")
                .item("Hosts and Ports…", "app.hosts")
                .separator()
                .item("Manage Host…", "host.manage")
                .enabled(far_host(&hub, &self.active_host))
                .item("Move to Another Host…", "host.move")
                .enabled(far_host(&hub, &self.active_host))
                .item("Set Up This Host’s Account…", "host.account")
                .enabled(far_host(&hub, &self.active_host))
                .item("This Host’s Permissions…", "host.permissions")
                .separator()
                .item("Manage Rex Server…", "host.server")
                .enabled(managed_host(&hub, &self.active_host).is_some())
                .item("Reload Server Configuration", "app.reload_config")
                .separator()
                .item("Remote Access…", "app.pair_device")
                .item("Diagnostics…", "app.diagnostics"),
            tabs,
            Menu::new("Pane")
                .item("Split Right", "pane.split_right")
                .enabled(pane)
                .item("Split Down", "pane.split_down")
                .enabled(pane)
                .item("Split to Fit", "pane.split_best_fit")
                .enabled(pane)
                .item("New Floating Terminal", "layer.new_floating")
                .enabled(tab)
                .separator()
                .submenu(
                    Menu::new("Focus")
                        .item("Left", "pane.focus_left")
                        .enabled(panes > 1)
                        .item("Right", "pane.focus_right")
                        .enabled(panes > 1)
                        .item("Up", "pane.focus_up")
                        .enabled(panes > 1)
                        .item("Down", "pane.focus_down")
                        .enabled(panes > 1),
                )
                .item("Zoom Pane", "pane.zoom_toggle")
                .checked(zoomed)
                .enabled(zoomed || panes > 1)
                .item("Type in All Panes", "pane.broadcast_toggle")
                .checked(broadcast_on)
                .enabled(panes > 1 || broadcast_on)
                .separator()
                // (The files of the pane's folder, on this computer or its host.)
                .item("Quick Look", "files.quick_look")
                .enabled(pane)
                .item(olo_kit_base::tracker::LABEL, "files.open_in_tracker")
                .enabled(pane)
                .separator()
                .item("Close Pane", "pane.close")
                .enabled(pane),
            Menu::new("Agents")
                .item("Next Waiting Agent", "agent.jump_next_blocked")
                .item("Mark All as Read", "agent.mark_all_read")
                .separator()
                .item("Keep It Going", "agent.keep_going_toggle")
                .checked(keep_going)
                .item("Auto Approve", "agent.auto_approve_toggle")
                .checked(help.auto_approve)
                .enabled(pane)
                .separator()
                .item("Install Olo’s Skill for Agents", "olo.install_remote_skill")
                .enabled(pane),
            Menu::new("Settings")
                .submenu(
                    Menu::new("Text Size")
                        .item("Bigger", "terminal.font_increase")
                        .enabled(text_size < crate::settings::TEXT_SIZES.1)
                        .item("Smaller", "terminal.font_decrease")
                        .enabled(text_size > crate::settings::TEXT_SIZES.0)
                        .item("Default", "terminal.font_reset")
                        .enabled(text_size != crate::settings::TEXT_SIZE),
                )
                .submenu(colors)
                // (How the window looks, as Terminal's Settings menu: typing
                // over a slow link, set once and left, is Preferences ▸
                // Rex's alone, General. It was here too: Opus's review,
                // 2026-11-24, "one more place to find them".)
                .submenu(Menu::new("Tabs").item("Along the Top", "tabs.top").checked(layout == ToolbarLayout::Above).item("Down the Side", "tabs.side").checked(layout == ToolbarLayout::Side)),
        ]
    }

    fn render_workspace(&mut self, cx: &mut olo_kit::Cx<Self>) -> Element {
        let viewport = cx.viewport_size();
        let scale = cx.scale_factor();
        let surface = cx.surface_id();
        let hub = app().hub.clone();

        // Provide structured context to Olo's `/ask` palette!
        if !olo_kit::agent::has_about(cx) {
            olo_kit::agent::about_from(cx, move |_| {
                let a = app();
                let st = a.hub.state.lock().unwrap_or_else(|e| e.into_inner());
                let hosts: Vec<String> = st.hosts.values().map(|h| format!("{} ({})", h.label, h.endpoint_raw)).collect();
                Some(format!("Asked from Olo Rex (multiplexer for local and remote workspaces).\nConnected hosts: {}", hosts.join(", ")))
            });
        }

        // Auto-sync terminal color theme whenever Olo's desktop look flips Light <-> Dark!
        let look = theme::palette();
        if !self.last_look.as_ref().is_some_and(|(mode, id)| *mode == look.mode && *id == look.terminal.id) {
            self.last_look = Some((look.mode, look.terminal.id.clone()));
            let new_theme = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                themes::resolve_theme(&st.settings, &look)
            };
            let blocks: Vec<_> = {
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.active_theme = new_theme.clone();
                st.blocks.values().cloned().collect()
            };
            for br in blocks {
                let _ = br.lock().unwrap_or_else(|e| e.into_inner()).apply_theme(&new_theme);
            }
            if let Some(pv) = &self.panes_view {
                pv.update(cx, |_, cx| cx.invalidate());
            }
        }

        let settings = hub.state.lock().unwrap_or_else(|e| e.into_inner()).settings.clone();
        let active_theme = hub.state.lock().unwrap_or_else(|e| e.into_inner()).active_theme.clone();
        if let Some((hk, sid)) = &self.created_session {
            let ready = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(hk).is_some_and(|h| h.sessions.contains_key(sid));
            if ready {
                let (hk, sid) = self.created_session.take().unwrap();
                self.active_host = hk;
                self.active_session = Some(sid);
                self.focus.focus(cx);
                if self.side_pane.alone() {
                    self.side_pane.show_rest();
                }
            }
        }
        if let Some(bk) = &self.pending_focus {
            let ready = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.hosts.get(&bk.host_key).and_then(|h| h.sessions.get(&bk.session_id)).is_some_and(|s| s.find_block(&bk.block_id).is_some())
            };
            if ready {
                let bk = self.pending_focus.take().unwrap();
                self.jump_to(JumpTarget::Pane(bk), cx);
            }
        }
        let previous = self.active_session.clone();
        let sess_info = self.resolve_active_session();
        if let Some(slot) = wins().map.get_mut(&self.id) {
            let focused = sess_info.as_ref().and_then(|(_, s)| selected_block(s));
            slot.selection.host.clone_from(&self.active_host);
            slot.selection.session.clone_from(&self.active_session);
            slot.selection.block = focused.map(str::to_owned);
            slot.selection.active = self.window_active;
        }
        if previous.is_some() && previous != self.active_session {
            self.clear_feedback();
            self.find_open = false;
            self.focus.focus(cx);
        }

        if self.window_active {
            let content_visible = !matches!(self.side_pane.panes(f32::from(viewport.width), 460.0), SidePanes::Pane) && self.overlay.is_none();
            let pane = if content_visible { self.focused_block_key() } else { None };
            if let Some(key) = &pane {
                let block = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned();
                if let Some(block) = block {
                    let mut b = block.lock().unwrap_or_else(|e| e.into_inner());
                    if b.unread {
                        b.unread = false;
                        b.unread_cleared_at = Some(Instant::now());
                    }
                }
            }
            let front = match pane {
                Some(p) => Target::Pane(p),
                None if content_visible && let Some((host, session)) = &sess_info => Target::Session { host_key: host.clone(), session_id: session.session_id.clone() },
                None => Target::Host(self.active_host.clone()),
            };
            app().attention.showing(self.id, front);
        }
        // Sync Deskbar + window tab unread badge!
        let has_unread = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            st.blocks.values().any(|b| b.lock().unwrap_or_else(|e| e.into_inner()).unread)
        };
        if has_unread != self.unread_marked {
            self.unread_marked = has_unread;
            olo_kit::unread::mark(cx, has_unread, false);
        }

        // (A terminal-style window's: the command key's chords reach it with Shift.)
        let said = self.build_menu_bar();
        let menu_bar = MenuBar::said(cx, said, |v: &mut Self, _| v.build_menu_bar(), olo_kit::menu::on_command, true);

        let total_w = f32::from(viewport.width).max(320.0);

        // Adaptive SidePane: never squeezes split terminals below 460 pt; folds cleanly in vertical thirds!
        let side_layout = self.side_pane.panes(total_w, 460.0);
        let side_w = side_layout.width();

        // Build TabStrip from active session's windows.
        let mut strip_tabs = Vec::new();
        let mut active_tab_idx = 0usize;
        if let Some((ref hk, ref sess)) = sess_info {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let names = crate::hub::session_tab_names(&st, hk, sess);
            for (idx, w) in sess.windows.iter().enumerate() {
                if sess.active_window_id.as_deref() == Some(w.window_id.as_str()) {
                    active_tab_idx = idx;
                }
                let mut tab_act = AgentActivity::Idle;
                let mut tab_unread: Option<Option<Instant>> = None;
                let mut tab_help = crate::agent::Help::default();
                for b in w.all_blocks() {
                    let bk = BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                    let h = crate::hub::help_of(&bk);
                    tab_help.keep_going |= h.keep_going;
                    tab_help.auto_approve |= h.auto_approve;
                    if let Some(br) = st.blocks.get(&bk) {
                        let g = br.lock().unwrap_or_else(|e| e.into_inner());
                        if g.unread {
                            tab_unread = Some(None);
                        } else if let Some(cleared) = g.unread_cleared_at
                            && tab_unread.is_none()
                        {
                            tab_unread = Some(Some(cleared));
                        }
                        tab_act = higher_activity(tab_act, g.agent_activity);
                    }
                }
                let label = names.get(idx).cloned().unwrap_or_default();
                let icon_fn: Option<olo_kit::widgets::strip::PaintFn> = Some(widgets::agent_marks::icon("terminal", marks_of(tab_act, &tab_help, false)));
                let what = match tab_act {
                    AgentActivity::Blocked => Some(olo_kit::words::tr("Waiting for approval").to_string()),
                    AgentActivity::Working => Some(olo_kit::words::tr("Working").to_string()),
                    AgentActivity::Error => Some(olo_kit::words::tr("Error").to_string()),
                    AgentActivity::Done => Some(olo_kit::words::tr("Done").to_string()),
                    AgentActivity::Idle => None,
                };
                let words: Vec<String> = [what, help_words(&tab_help)].into_iter().flatten().collect();
                strip_tabs.push(StripTab { label: label.into(), icon: icon_fn, status: (!words.is_empty()).then(|| words.join("; ").into()), unread: tab_unread, ..Default::default() });
            }
        }

        let active_has_splits = sess_info.as_ref().and_then(|(_, s)| s.active_window().or_else(|| s.windows.first())).is_some_and(|w| w.all_blocks().count() > 1 || w.zoomed_block_id.is_some());
        let active_shows_header = active_has_splits && settings.pane_header_behavior != BarBehavior::Hidden;
        // Active tab opens seamlessly into the focused pane header (panel_background), visibly lighter than inactive tabs!
        let (tab_content_bg, tab_content_fg) =
            if active_shows_header { (style::c(look.panel_background), style::c(look.panel_text)) } else { (hsla(active_theme.background), hsla(active_theme.foreground)) };

        let is_side_strip = settings.toolbar_layout == ToolbarLayout::Side;
        let strip_el: Option<Element> = if !strip_tabs.is_empty()
            && let Some((hk, sess)) = &sess_info
        {
            let hub = hub.clone();
            let (hk, sid) = (hk.clone(), sess.session_id.clone());
            let ids: Vec<_> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
            let mut strip = TabStrip::new("rex-tabs", strip_tabs, active_tab_idx).content(tab_content_bg, tab_content_fg).in_window(surface).on_action(cx.msg_with(move |act: &StripAction| {
                match act {
                    StripAction::Select(i) => {
                        crate::activity_watch::close_viewer();
                        if let Some(id) = ids.get(*i) {
                            hub.select_window(&hk, &sid, id);
                        }
                    }
                    StripAction::Close(i) => {
                        if let Some(id) = ids.get(*i) {
                            hub.close_window(&hk, &sid, id);
                        }
                    }
                    StripAction::New => hub.create_window(&hk, &sid, None),
                    // A tab's own menu, and a tab dragged to another
                    // place among its session's. (Neither did anything.)
                    StripAction::Menu(i, at) => return WinMsg::TabMenu(*i, *at),
                    StripAction::Move { from, to, .. } => {
                        if let Some(before) = crate::hub::moved_before(&ids, *from, *to) {
                            hub.move_window(&hk, &sid, &ids[*from], before);
                        }
                    }
                    _ => return WinMsg::Redraw,
                }
                WinMsg::TabChanged
            }));
            if is_side_strip {
                strip = strip.side();
            } else {
                strip = strip.max_tab_width(220.0);
            }
            Some(if is_side_strip { div().w(px(196.0)).h_full().flex_shrink_0().child(strip) } else { div().w_full().h(px(widgets::STRIP_HEIGHT)).flex_shrink_0().child(strip) })
        } else {
            None
        };

        // Optional Find Bar (for Transcript view, placed neatly beneath the transcript header;
        // for normal Terminal panes, placed above the panes).
        let tr_active = crate::activity_watch::active_transcript();
        let focused_bk_for_find = if self.find_open { self.focused_block_key() } else { None };
        let (tr_find_bar, find_el) = if self.find_open
            && let Some(ff) = &self.find_field
        {
            let query = ff.read(cx).text().to_owned();
            if let Some(tr) = &tr_active {
                let (total, sel) = {
                    let g = tr.lock().unwrap_or_else(|e| e.into_inner());
                    (g.matches.len(), g.match_idx)
                };
                let said = find_said(&query, total, sel, None);
                (Some(crate::transcript_view::render_transcript_find_bar(cx, ff, said, total > 0, cx.msg_with(|back: &bool| WinMsg::FindStep(*back)), cx.msg_with(|_: &()| WinMsg::FindClose))), None)
            } else {
                let (total, sel, finding) = focused_bk_for_find
                    .and_then(|bk| {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        st.blocks.get(&bk).map(|br| {
                            let g = br.lock().unwrap_or_else(|e| e.into_inner());
                            (g.search.total_matches(), g.search.selected_index(), g.finding)
                        })
                    })
                    .unwrap_or((0, None, false));
                let said = if finding { "Searching…".into() } else { find_said(&query, total, sel, None) };
                (None, Some(find_bar(cx, ff, said, total > 0, cx.msg_with(|back: &bool| WinMsg::FindStep(*back)), cx.msg_with(|_: &()| WinMsg::FindClose))))
            }
        } else {
            (None, None)
        };

        // Main center content: live Transcript view, safe read-only Tmux viewer, or normal tiled terminal panes!
        let panes_el = if let Some(tr_sess) = tr_active {
            let (cur_gen, follow) = {
                let g = tr_sess.lock().unwrap_or_else(|e| e.into_inner());
                (g.generation, g.follow)
            };
            if follow && cur_gen != self.last_transcript_gen {
                self.last_transcript_gen = cur_gen;
                self.transcript_scroll.set_offset(cx, point(px(0.0), px(-1_000_000.0)));
            } else {
                self.last_transcript_gen = cur_gen;
            }
            div().flex_1().min_w_0().min_h_0().size_full().child(crate::transcript_view::render_transcript_view(
                &tr_sess,
                &self.transcript_scroll,
                self.find_open,
                tr_find_bar,
                cx.msg_with(|what: &crate::transcript_view::TranscriptDo| WinMsg::TranscriptDo(what.clone())),
            ))
        } else if let Some(tm_viewer) = crate::activity_watch::active_tmux() {
            let metrics = Metrics::from_settings(scale, &settings);
            let avail_w = (total_w - side_w).max(200.0);
            let avail_h = (f32::from(cx.viewport_size().height) - 26.0 - widgets::STRIP_HEIGHT - m::LABEL_ROW - 4.0).max(160.0);
            div().flex_1().min_w_0().min_h_0().size_full().child(crate::transcript_view::render_tmux_viewer(
                &tm_viewer,
                &self.tmux_visual,
                &self.tmux_scroll,
                avail_w,
                avail_h,
                &active_theme,
                &metrics,
                cx.msg_with(|what: &crate::transcript_view::TranscriptDo| WinMsg::TranscriptDo(what.clone())),
            ))
        } else if let Some((hk, sess)) = &sess_info
            && let Some(w) = sess.active_window().or_else(|| sess.windows.first())
            && w.all_blocks().next().is_some()
            && let Some(child) = &self.panes_view
        {
            let current = child.read(cx).session.as_ref();
            if !current.is_some_and(|(h, s)| h == hk && s == &sess.session_id) {
                child.update(cx, |pv, cx| {
                    pv.session = Some((hk.clone(), sess.session_id.clone()));
                    cx.invalidate();
                });
            }
            let width = Some((total_w - side_w).max(200.0));
            if child.read(cx).width != width {
                child.update(cx, |pv, cx| {
                    pv.width = width;
                    cx.invalidate();
                });
            }
            div().flex_1().min_w_0().min_h_0().size_full().child(child)
        } else {
            // (The app's own state before the hub's, never inside it:
            // everywhere else the hub is reached through the app, the
            // other way round, and two threads taking the two in
            // opposite orders stop for good. This one only happened
            // while a host was connecting, which a slow link makes long:
            // found there, 2026-11-22, by the dev build's lock checker.)
            let askpass = app().askpass.clone();
            let state = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let host = state.hosts.get(&self.active_host);
            let name = host.map_or("This computer", |h| h.label.as_str());
            let (message, action, offline) = match host.map(|h| &h.conn_state) {
                Some(HostConnState::Connected) if let Some((_, sess)) = &sess_info => {
                    let label = if sess.label.is_empty() { &sess.session_id } else { &sess.label };
                    (olo_kit::tr!("“{}” has no terminals.", label), Some(("New Terminal", "window.new")), false)
                }
                Some(HostConnState::Connected) if self.active_session.is_some() => (olo_kit::words::tr("Opening session…").into(), None, false),
                // (A name the person gave is quoted, a host's as a session's.)
                Some(HostConnState::Connected) => (olo_kit::tr!("“{}” has no sessions.", name), Some(("New Session", "session.new")), false),
                // (Its key agent asked to sign, and silent: said, with the
                // likeliest reason. It read "Connecting…" for as long as
                // Passwords stayed locked.)
                Some(HostConnState::Connecting) if host.is_some_and(|h| askpass.waits_for_agent(&h.endpoint_raw)) => {
                    (olo_kit::tr!("Waiting for your key agent to sign in to “{}”. If your keys are in Passwords, it may be locked.", name), None, false)
                }
                Some(HostConnState::Connecting) => (olo_kit::tr!("Connecting to “{}”…", name), None, false),
                _ => (olo_kit::tr!("“{}” is disconnected.", name), Some(("Reconnect", "host.reconnect")), true),
            };
            let lost = host.and_then(|h| h.connection_lost);
            drop(state);
            let icon = canvas(move |g, b| {
                icons::paint(g, b, "terminal");
                if offline {
                    let p = theme::palette();
                    let center = point(b.right() - px(m::TIGHT), b.bottom() - px(m::TIGHT));
                    style::dot(g, center, 2.0 * m::TIGHT, style::c(p.panel_background));
                    style::dot(g, center, 2.0 * m::TIGHT - 2.0, style::disabled_text(&p));
                }
            })
            .w(px(48.0))
            .h(px(48.0))
            .paint_key(style::key(offline));
            let mut group = div().w_full().h(olo_kit::relative(0.90)).flex().flex_col().items_center().justify_center().gap(px(m::SPACING)).px(px(m::SPACING)).child(icon);
            // (The two lines are one thing: closer to each other than to
            // the icon above and the button below.)
            let mut lines = div().flex().flex_col().items_center().gap(px(m::TIGHT / 2.0)).child(StringView::new(message).height(m::LABEL_ROW));
            if offline && let Some(at) = lost {
                lines = lines.child(StringView::new(olo_kit::ago::ago(at, connection_age)).dim(true).height(m::LABEL_ROW));
            }
            group = group.child(lines);
            if let Some((label, cmd)) = action {
                group = group.child(Button::new(label).default(true).on_click(cx.msg(WinMsg::Command(cmd.into()))));
            }
            div().flex_1().min_w_0().min_h_0().child(group)
        };

        // The tabs along the top of the panes, or down their side.
        let mut center = div().flex_1().min_w_0().min_h_0().flex();
        center = match settings.toolbar_layout {
            ToolbarLayout::Above => {
                let mut c = center.flex_col();
                if let Some(s) = strip_el {
                    c = c.child(s);
                }
                if let Some(fb) = find_el {
                    c = c.child(fb);
                }
                c.child(panes_el)
            }
            ToolbarLayout::Side => {
                let mut inner = div().flex_1().min_w_0().min_h_0().flex().flex_col();
                if let Some(fb) = find_el {
                    inner = inner.child(fb);
                }
                inner = inner.child(panes_el);
                let mut c = center.flex_row();
                if let Some(s) = strip_el {
                    c = c.child(s);
                }
                c.child(inner)
            }
        };

        // Sidebar (`Sessions` / `Activity`) beside center.
        let mut body_row = div().relative().flex_1().min_w_0().min_h_0().w_full().flex().flex_row();
        match side_layout {
            SidePanes::Both(w) => {
                body_row = body_row.child(self.render_sidebar(w, cx)).child(center);
            }
            SidePanes::Pane => {
                body_row = body_row.child(self.render_sidebar(total_w, cx));
            }
            SidePanes::Rest => {
                crate::activity_watch::sync_watchers(&hub, false);
                body_row = body_row.child(center);
            }
        }

        // The sidebar's divider, in the row it divides: not over the
        // status bar under it, whose words went under its grip.
        if !self.side_pane.alone() {
            body_row = body_row.child(side_divider_within(&self.side_pane, side_w, "Rex Sidebar", cx.msg_with(|_: &()| WinMsg::SidebarMoved)));
        }

        // Overlays layered above the workspace!
        let mut root =
            div().relative().size_full().flex().flex_col().track_focus(&self.focus).on_key_down(cx.listener(|w, e: &KeyDownEvent, cx| w.handle_terminal_key(e, cx))).child(menu_bar).child(body_row);

        // The status bar is always there, as Tracker's is: where the
        // window is (its host, its session) while there is nothing to
        // say, a passing word or a problem when there is. (It came and
        // went with each word: the panes above it lost and regained a
        // row each time, which to a program in a terminal is the window
        // resized twice. The second design review.)
        let line = match self.feedback_line(cx) {
            Some(line) => line,
            None => div().child(StringView::new(self.where_it_is()).size(style::SMALL_SIZE).height(m::LABEL_ROW).dim(true).cut(true)).into_any_element(),
        };
        let silent = self.silent_host();
        let has = self.host_has();
        // (A file going to or from the focused pane's host.)
        let transfer = self
            .focused_block_key()
            .and_then(|bk| hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned())
            .and_then(|b| b.lock().unwrap_or_else(|e| e.into_inner()).upload_progress.clone());
        let marks_host = self.focused_block_key().map_or_else(|| self.active_host.clone(), |focused| focused.host_key);
        let open_host_window: Rc<dyn Fn(&String, &mut WindowCx)> = Rc::new(cx.msg_with(|host: &String| WinMsg::HostOpen(host.clone())));
        let open_account_window: Rc<dyn Fn(&String, &mut WindowCx)> = Rc::new(cx.msg_with(|host: &String| WinMsg::AccountOpen(host.clone())));
        root = root.child(
            div().w_full().flex_shrink_0().flex().flex_col().child(widgets::separator()).child(
                div()
                    .relative()
                    .w_full()
                    .min_h(px(m::LABEL_ROW))
                    .px(px(m::SPACING))
                    // Symmetric 1 pt vertical padding inside the 18 pt status bar so
                    // the 16 pt status text sits centred to the pixel at every scale.
                    .py(px(1.0))
                    .child(canvas(|g, b| style::fill_rect(g, b, style::c(theme::palette().panel_background))).paint_key(1).absolute().inset_0())
                    // What the bar says stays where it is; a host that
                    // is silent is said at the bar's far end, with the
                    // light a waiting host has in the sidebar, in the
                    // words' own colour (the folder and the size are
                    // facts, and dimmer: this is a state). The two
                    // design reviews, 2026-11-22: as a fourth fact at
                    // the front it was too quiet, and pushed the others
                    // along with each second.
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(m::SPACING))
                            .child(div().flex_1().min_w_0().child(line))
                            .children(transfer.map(|t| transfer_said(t, self.id)))
                            .children(silent.map(|said| {
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    // (The sidebar's light and its gap, in the
                                    // proportion of the bar's smaller words:
                                    // seven points and five, of eight and six.)
                                    .gap(px((m::TIGHT * style::SMALL_SIZE / style::PLAIN_SIZE).round()))
                                    .child(crate::panels::light_of(crate::panels::Light::Waiting, (crate::panels::LIGHT * style::SMALL_SIZE / style::PLAIN_SIZE).round()))
                                    .child(StringView::new(said).size(style::SMALL_SIZE).height(m::LABEL_ROW))
                            }))
                            // What the host has of Olo's, at the very end: three
                            // marks, each on or off (its typing helper, its
                            // bridge, the skill for agents), with what each
                            // means for its tip.
                            .children(has.map(|has| {
                                // (A click on one: the window where it is
                                // put on the host or taken off. The host's
                                // own for its helper and the bridge; the
                                // account's for the skill, which is a row
                                // there.)
                                let mark = |name: &'static str, mark: widgets::Mark, has: crate::hub::Has, open: &Rc<dyn Fn(&String, &mut WindowCx)>| {
                                    let (open, host) = (open.clone(), marks_host.clone());
                                    widgets::Indicator::new(name, mark).on(has.on).says(has.said).on_click(move |cx| open(&host, cx))
                                };
                                let marks = [
                                    Some(mark("Typing helper", widgets::Mark::Keyboard, has.helper, &open_host_window)),
                                    has.bridge.map(|bridge| mark("Olo’s bridge", widgets::Mark::Bridge, bridge, &open_host_window)),
                                    has.skill.map(|skill| mark("Skill for agents", widgets::Mark::Book, skill, &open_account_window)),
                                ];
                                // (Their tips clear of the bar's rule and of
                                // the band over it: the separator the bar
                                // begins with, six points. Clear of the rule
                                // alone, a tip stood a point over the band's
                                // edge: Gemini's review.)
                                widgets::indicators(marks.into_iter().flatten().collect(), 6.0)
                            })),
                    ),
            ),
        );

        // Mode pill (Copy Mode or Key Mode) anchored bottom-left.
        let mode_pill_text = self
            .focused_block_key()
            .and_then(|bk| {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.blocks.get(&bk).and_then(|br| br.lock().unwrap_or_else(|e| e.into_inner()).copy_mode.as_ref().map(CopyMode::status_text))
            })
            .or_else(|| app().keymap.active_mode_name().map(|m| format!("MODE: {}", m.to_uppercase())));
        if let Some(pill) = mode_pill_text {
            root = root.child(overlays::render_mode_pill(pill));
        }

        // Which-Key Sequence Hints HUD when waiting on a multi-chord prefix.
        if let Some((prefix, hints)) = app().keymap.pending_hints() {
            root = root.child(overlays::render_which_key_hud(&prefix, &hints));
        }

        // What is open over the workspace: a question a program on a host
        // asks (rex.ui), the folders of a pane's host, Quick Look. (Rex's own
        // Command Palette and Go to Anything were two more: its commands
        // and its sessions, tabs and panes are found in Olo's palette, as
        // every app's.)
        if let Some(ov) = &self.overlay
            && let Some(of) = &self.overlay_field
        {
            let overlay_el = overlays::render_active_overlay(
                cx,
                ov,
                of,
                cx.msg_with(|_: &()| WinMsg::DismissOverlay),
                cx.msg_with(|i: &usize| WinMsg::RunOverlayIndex(*i)),
                cx.msg_with(|id: &String| WinMsg::TogglePromptCheck(id.clone())),
                cx.msg_with(|_: &()| WinMsg::ConfirmOverlay),
            );
            root = root.child(overlay_el);
        }

        let _ = scale;
        root
    }

    fn render_sidebar(&mut self, width: f32, cx: &mut olo_kit::Cx<Self>) -> Element {
        let hub = app().hub.clone();
        // (The pane this window shows: Activity's selected row.)
        let shown = self.focused_block_key();
        let (mode, hosts_snap, activity_rows) = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let mode = st.settings.sidebar_mode;
            let mut hosts_snap = Vec::new();
            let mut activity_rows = Vec::new();
            for hk in &st.host_order {
                let Some(h) = st.hosts.get(hk) else { continue };
                let mut sessions = Vec::new();
                for s in &h.session_summaries {
                    let view = h.sessions.get(&s.session_id);
                    let mut sess_act = AgentActivity::Idle;
                    let mut sess_help = crate::agent::Help::default();
                    let mut unread_count = 0usize;
                    if let Some(v) = view {
                        // (Each agent named by its tab, as the tab strip names
                        // it: the name it was given, else its program's title.
                        // By its pane's own title it went by what its program
                        // calls itself, Claude Code's version, agy's name: Ben,
                        // 2026-11-24, "it isn't showing the terminal tab title
                        // it's showing some weird "2.1.331" title".)
                        let tab_names = crate::hub::session_tab_names(&st, &h.key, v);
                        for (tab, w) in v.windows.iter().enumerate() {
                            // (A tab of more than one agent: each also by its own
                            // pane's title, where that says something more.)
                            let agents_here = w
                                .all_blocks()
                                .filter(|b| {
                                    let bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                    st.blocks.get(&bk).is_some_and(|br| {
                                        let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                        g.agent_activity != AgentActivity::Idle || g.runs_agent() || st.settings.activity_include_idle
                                    })
                                })
                                .count();
                            for b in w.all_blocks() {
                                let bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                if let Some(br) = st.blocks.get(&bk) {
                                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                    sess_act = higher_activity(sess_act, g.agent_activity);
                                    let pane_help = crate::hub::help_of(&bk);
                                    sess_help.keep_going |= pane_help.keep_going;
                                    sess_help.auto_approve |= pane_help.auto_approve;
                                    if g.unread {
                                        unread_count += 1;
                                    }
                                    // (An agent stays while its program runs, at rest
                                    // too: Ben, 2026-11-24, "the agent shouldn't
                                    // disappear if it just stops working but the
                                    // harness is still open".)
                                    let running = g.runs_agent();
                                    if g.agent_activity != AgentActivity::Idle || running || st.settings.activity_include_idle {
                                        // (What it goes by: its title, else its program.)
                                        let process = g.process.as_ref();
                                        let program = g.program_title();
                                        let home = process.and_then(|p| p.home.clone()).or_else(|| h.is_local.then(crate::hub::local_home).flatten());
                                        // (Its folder is the line under it: `tab_title` leaves a
                                        // pane outside home unnamed, its tab to be named by it.)
                                        let pane = crate::hub::pane_title(&g.title, &g.label, program.as_deref(), "");
                                        let label = activity_title(tab_names.get(tab).map(String::as_str), agents_here, pane);
                                        // (What it says of itself, else its folder by its own
                                        // name: a path cut at its end lost the part that
                                        // tells two apart.)
                                        let detail = g.program_statuses.first().and_then(|r| r.msg.clone().or_else(|| r.title.clone())).unwrap_or_else(|| folder_name(&g.pwd, home.as_deref()));
                                        let help = crate::hub::help_of(&bk);
                                        activity_rows.push(ActivityRow { bk, host: h.label.clone(), title: label, detail, act: g.agent_activity, help, running });
                                    }
                                }
                            }
                        }
                    }
                    let label = if !s.label.is_empty() { s.label.clone() } else { s.session_id.clone() };
                    sessions.push((s.session_id.clone(), label, sess_act, sess_help, unread_count));
                }
                // (And whether it is silent about something typed: its
                // light and its words then say so, as the status bar does.)
                let silent = st.host_silent(&h.key).map(crate::hub::silent_tip);
                hosts_snap.push((h.key.clone(), h.label.clone(), h.conn_state.clone(), sessions, silent));
            }
            (mode, hosts_snap, activity_rows)
        };

        let back = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
        })
        .paint_key(2)
        .absolute()
        .inset_0();

        let blocked_n = activity_rows.iter().filter(|r| matches!(r.act, AgentActivity::Blocked | AgentActivity::Error)).count();

        let hub_s1 = hub.clone();
        // Match TabStrip exactly (`STRIP_HEIGHT = 34`, tabs `y = 8..34`, label center `y = 21`)
        // so Sessions/Activity shares the exact baseline and bottom line of the tab strip!
        let header_bg = canvas(|g, b| {
            let p = theme::palette();
            let (shadow, _) = bevel_colors(&p);
            style::hline_above(g, b.left(), b.right(), b.bottom(), shadow);
        })
        .paint_key(201)
        .absolute()
        .inset_0();

        // The sidebar's two lists: the kit's segmented control, the
        // row's width, the agents waiting as Activity's count; its
        // marks show once the sidebar is wide enough for them beside
        // the names and the count. (They were a pair of small buttons,
        // the chosen one held down, and before that two boxes drawn by
        // hand; Ben, 2026-11-22: "this sessions/activity segmented
        // control looks weird".)
        let segments = widgets::SegmentedControl::new(
            "rex-side",
            [widgets::Segment::new("Sessions").mark(widgets::Mark::Terminal), widgets::Segment::new("Activity").mark(widgets::Mark::Pulse).waiting(blocked_n)],
            if mode == SidebarMode::Activity { 1 } else { 0 },
        )
        .width(((width - 2.0 * m::SPACING) / 2.0).floor() * 2.0)
        .on_choose(cx.msg_with(move |i: &usize| {
            let activity = *i == 1;
            {
                let mut st = hub_s1.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.sidebar_mode = if activity { SidebarMode::Activity } else { SidebarMode::Sessions };
                st.settings.save();
            }
            crate::activity_watch::sync_watchers(&hub_s1, activity);
            WinMsg::Redraw
        }));
        let header = div()
            .relative()
            .w_full()
            .h(px(widgets::STRIP_HEIGHT))
            .flex_shrink_0()
            // (In the middle of its band, between the menu bar's rule
            // and the band's own, which is its last point: to the device
            // pixel, the odd one over it. Set on the tabs' words' line it
            // stood three points over the band's rule and twelve under
            // the menu bar; and a point high of the middle after that,
            // five over it and four under, measured in its picture.)
            .pt(px({
                let scale = cx.scale_factor();
                let free = (widgets::STRIP_HEIGHT - 1.0 - m::CONTROL_HEIGHT) * scale;
                (free / 2.0).ceil() / scale
            }))
            .px(px(m::SPACING))
            .flex()
            .flex_row()
            .items_start()
            .child(header_bg)
            .child(segments);

        let mut list = div().flex_shrink_0().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SPACING));
        if mode == SidebarMode::Sessions {
            for (hk, hlabel, conn, sessions, silent) in hosts_snap {
                let connected = conn == HostConnState::Connected;
                let tip = match &conn {
                    HostConnState::Connected if silent.is_some() => silent.clone().unwrap_or_default().into(),
                    HostConnState::Connected => olo_kit::words::tr("Connected").into(),
                    HostConnState::Connecting => olo_kit::words::tr("Connecting…").into(),
                    HostConnState::Disconnected => olo_kit::words::tr("Disconnected").into(),
                    HostConnState::Error(why) | HostConnState::Reconnecting { last_error: why, .. } => olo_kit::tr!("Disconnected: {}", why),
                };
                let selected = self.active_host == hk && !sessions.iter().any(|(s, ..)| self.active_session.as_ref() == Some(s));
                // (A host that is connected and silent about something
                // typed waits: the light the status bar's word has. The
                // second design review: the bar said it hadn't answered
                // beside a green light here.)
                let lit = if connected && silent.is_some() { crate::panels::Light::Waiting } else { crate::panels::light_for(&conn) };
                let key = style::key((&hlabel, &tip, lit, selected));
                let state_words = tip.clone();
                let tip_key = format!("rex-host-{hk}");
                olo_kit::tip::retext(cx, tip_key.clone(), tip.clone());
                let hk_new = hk.clone();
                let window = self.id;
                let pick = cx.msg(WinMsg::SelectHost(hk.clone()));
                let row_label = canvas(move |g, b| {
                    let p = theme::palette();
                    let y = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
                    style::dot(g, point(b.left() + px(m::TIGHT), y), crate::panels::LIGHT, lit.color(&p));
                    // (Selected, a disconnected host's name is a selected
                    // name: dimmed on the selection it read as something
                    // that can't be chosen, and choosing it is the way
                    // to Reconnect. Its grey dot says the rest.)
                    let ink = if selected {
                        style::c(style::selection_text(&p, p.panel_text, false))
                    } else if connected {
                        style::c(p.panel_text)
                    } else {
                        style::secondary_text(&p)
                    };
                    let left = b.left() + px(m::WINDOW_INSET);
                    style::label_fit(g, left, b.center().y, style::Align::Left, hlabel.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, b.right() - left);
                    g.agent(agent::Node {
                        role: "Button",
                        name: Some(hlabel.clone().into()),
                        value: Some(state_words.clone().into()),
                        selected: Some(selected),
                        enabled: true,
                        clickable: true,
                        bounds: b,
                        ..Default::default()
                    });
                })
                .paint_key(key)
                .h_full()
                .flex_1()
                .min_w_0()
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, move |_, cx| {
                    pick(&(), cx);
                    cx.stop_propagation();
                });
                let mut row = div().w_full().h(px(m::LABEL_ROW + m::TIGHT)).flex_shrink_0().flex().flex_row().items_center().gap(px(m::TIGHT));
                if selected {
                    let p = theme::palette();
                    row = row.bg(style::c(style::selection_ground(&p, p.panel_background, false)));
                }
                let new_session = cx.msg(WinMsg::CreateSession(hk_new, None));
                row = row.child(row_label).child(GlyphButton::new("New Session", Glyph::Plus).id(format!("rex-new-session-{hk}")).enabled(connected).on_click(move |_, cx| new_session(&(), cx)));
                let mut group = div().w_full().flex_shrink_0().flex().flex_col().child(olo_kit::tip::on(row, tip_key, tip));
                for (sid, slabel, sact, shelp, unread) in sessions {
                    let is_active = self.active_host == hk && self.active_session.as_deref() == Some(sid.as_str());
                    let hk_sel = hk.clone();
                    let sid_sel = sid.clone();
                    let badge = if unread > 0 { Some(format!("{unread}")) } else { None };
                    let hk_end = hk.clone();
                    let hub_end = hub.clone();
                    // (What the mark on its icon says, in words: its tip.)
                    let tip_key = format!("rex-session-{hk}-{sid}");
                    let doing = activity_words(sact);
                    if let Some(words) = doing {
                        olo_kit::tip::retext(cx, tip_key.clone(), words);
                    }
                    let tipped = move |row: Element| match doing {
                        Some(words) => olo_kit::tip::on(row, tip_key, words),
                        None => row,
                    };
                    group = group.child(tipped(sidebar_row(slabel, badge, sact, shelp, is_active, cx.msg_with(move |_: &()| WinMsg::SelectSession(hk_sel.clone(), sid_sel.clone()))).on_mouse_down(
                        MouseButton::Right,
                        move |e, cx| {
                            let hub = hub_end.clone();
                            let (hk, sid) = (hk_end.clone(), sid.clone());
                            let mut menu = Menu::new("").item("Rename Session…", "session.rename").separator().item("End Session", "session.close");
                            menu::resolve(std::slice::from_mut(&mut menu), true);
                            menu::context_menu(cx, menu, e.position, move |cmd, _| match cmd {
                                "session.close" => hub.close_session(&hk, &sid, Feedback::start(window)),
                                "session.rename" => {
                                    if let Some(slot) = wins().map.get(&window) {
                                        slot.handle.messenger().send(WinMsg::RenameSession(hk.clone(), sid.clone()));
                                    }
                                }
                                _ => {}
                            });
                            cx.stop_propagation();
                        },
                    )));
                }
                list = list.child(group);
            }
        }
        crate::activity_watch::sync_watchers(&hub, mode == SidebarMode::Activity);
        if mode == SidebarMode::Activity {
            if activity_rows.is_empty() {
                list = list.child(div().py(px(m::SECTION)).child(StringView::new("All agents and shells are quiet.").dim(true)));
            }
            let active_view = crate::activity_watch::active_view_target();
            let active_sub_id = active_view.as_ref().and_then(|v| v.subagent_id()).map(str::to_owned);
            let win_id = self.id;
            // Group Activity rows by host under clean section headers (so rows
            // never repeat "· This computer" at the end of every description line).
            let mut rows = div().w_full().flex_shrink_0().flex().flex_col().agent("Group", "activity-subagents").agent_value(crate::activity_watch::activity_agent_json());
            let mut last_host: Option<String> = None;
            for row in activity_rows {
                if last_host.as_deref() != Some(row.host.as_str()) {
                    let host_name = row.host.clone();
                    let top_pad = if last_host.is_some() { 10.0 } else { 0.0 };
                    rows = rows.child(
                        div().w_full().pt(px(top_pad)).pb(px(6.0)).pl(px(m::WINDOW_INSET)).pr(px(m::TIGHT)).child(StringView::new(host_name).bold(true).size(style::SMALL_SIZE).dim(true).height(14.0)),
                    );
                    last_host = Some(row.host.clone());
                }
                let subs = crate::activity_watch::subagents_for(&row.bk);
                let total_children = subs.len();
                let failed_children = subs.iter().filter(|s| s.state == olo_rex_client::activity::SubagentState::Failed).count();
                let has_children = total_children > 0;
                let expanded = has_children && !crate::activity_watch::is_folded(&row.bk);
                let bk_jump = row.bk.clone();
                let bk_fold = row.bk.clone();
                let bk_children = row.bk.clone();
                let bk_menu = row.bk.clone();
                let row_title = row.title.clone();
                let viewing_tr = matches!(&active_view, Some(crate::activity_watch::ViewTarget::PaneTranscript { bk, .. }) if bk == &row.bk);
                let selected = active_sub_id.is_none() && shown.as_ref() == Some(&row.bk);
                // (As wide as the list's room: the sidebar less its inset.)
                let parent_el = sidebar_activity_row(
                    row,
                    selected,
                    has_children,
                    expanded,
                    total_children,
                    failed_children,
                    width - 2.0 * m::SPACING,
                    cx.msg_with(move |_: &()| WinMsg::FocusBlock(bk_jump.clone())),
                    cx.msg_with(move |_: &()| WinMsg::ToggleActivityFold(bk_fold.clone())),
                )
                .on_mouse_down(MouseButton::Right, move |e, cx| {
                    let bk = bk_menu.clone();
                    let title = row_title.clone();
                    let mut menu = Menu::new("").item("Show Transcript", "row.transcript").checked(viewing_tr).item("Show Terminal", "row.terminal").checked(!viewing_tr);
                    menu::resolve(std::slice::from_mut(&mut menu), true);
                    menu::context_menu(cx, menu, e.position, move |cmd, _| match cmd {
                        "row.transcript" => {
                            if let Some(slot) = wins().map.get(&win_id) {
                                slot.handle.messenger().send(WinMsg::OpenPaneTranscript(bk.clone(), title.clone()));
                            }
                        }
                        "row.terminal" => {
                            if let Some(slot) = wins().map.get(&win_id) {
                                slot.handle.messenger().send(WinMsg::FocusBlock(bk.clone()));
                            }
                        }
                        _ => {}
                    });
                    cx.stop_propagation();
                });
                rows = rows.child(parent_el);

                if expanded {
                    let parent_bk = bk_children;
                    for (sub_idx, sub) in subs.iter().enumerate() {
                        let is_last = sub_idx + 1 == subs.len();
                        let sub_selected = active_sub_id.as_deref() == Some(sub.id.as_str());
                        let sub_click = sub.clone();
                        let sub_menu = sub.clone();
                        let parent_click = parent_bk.clone();
                        let parent_menu = parent_bk.clone();
                        let has_tmux = sub.tmux.is_some();
                        let has_tr = sub.transcript.is_some();
                        let viewing_sub_tr = matches!(&active_view, Some(crate::activity_watch::ViewTarget::SubagentTranscript { sub: s, .. }) if s.id == sub.id);
                        let viewing_sub_tm = matches!(&active_view, Some(crate::activity_watch::ViewTarget::SubagentTmux { sub: s, .. }) if s.id == sub.id);
                        let tip_key = format!("rex-sub-{}", sub.id);
                        let tip_action = if has_tmux { "Opens read-only terminal" } else { "Opens transcript" };
                        let tip_text = if sub.description.is_empty() { format!("{} — {tip_action}", sub.name) } else { format!("{} — {} ({tip_action})", sub.name, sub.description) };
                        olo_kit::tip::retext(cx, tip_key.clone(), tip_text.clone());
                        let sub_el = olo_kit::tip::on(
                            sidebar_subagent_row(
                                sub.clone(),
                                sub_selected,
                                is_last,
                                width - 2.0 * m::SPACING,
                                cx.msg_with(move |_: &()| WinMsg::OpenSubagent(parent_click.clone(), sub_click.clone(), false)),
                            )
                            .on_mouse_down(MouseButton::Right, move |e, cx| {
                                let sub = sub_menu.clone();
                                let parent = parent_menu.clone();
                                let mut menu = Menu::new("")
                                    .item("Show Transcript", "sub.transcript")
                                    .enabled(has_tr)
                                    .checked(viewing_sub_tr)
                                    .item("Show Terminal", "sub.terminal")
                                    .enabled(has_tmux)
                                    .checked(viewing_sub_tm);
                                menu::resolve(std::slice::from_mut(&mut menu), true);
                                menu::context_menu(cx, menu, e.position, move |cmd, _| match cmd {
                                    "sub.transcript" => {
                                        if let Some(slot) = wins().map.get(&win_id) {
                                            slot.handle.messenger().send(WinMsg::OpenSubagent(parent.clone(), sub.clone(), true));
                                        }
                                    }
                                    "sub.terminal" if has_tmux => {
                                        if let Some(slot) = wins().map.get(&win_id) {
                                            slot.handle.messenger().send(WinMsg::OpenSubagent(parent.clone(), sub.clone(), false));
                                        }
                                    }
                                    _ => {}
                                });
                                cx.stop_propagation();
                            }),
                            tip_key,
                            tip_text,
                        );
                        rows = rows.child(sub_el);
                    }
                }
            }
            list = list.child(rows);
        }

        // Bottom-anchored sidebar footer ("Add Host…" is a Sessions action; hide in Activity).
        let footer = (mode == SidebarMode::Sessions).then(|| {
            let footer_bg = canvas(|g, b| {
                let p = theme::palette();
                style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.25));
            })
            .paint_key(202)
            .absolute()
            .inset_0();
            div()
                .relative()
                .w_full()
                .flex_shrink_0()
                .px(px(m::SPACING))
                .py(px(8.0))
                .flex()
                .flex_row()
                .items_center()
                .child(footer_bg)
                .child(Button::new("Add Host…").small().on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::Command("app.add_host".into()))))
        });

        div()
            .relative()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(back)
            .child(header)
            .child(div().flex_1().min_h_0().child(widgets::ScrollView::new(&self.sidebar_scroll).border(false).auto_hide(true).background(theme::palette().panel_background).child(list)))
            .children(footer)
            .child(
                canvas(|g, b| {
                    let p = theme::palette();
                    style::vline_left_of(g, b.right(), b.top(), b.bottom(), style::shade(&p, p.panel_background, 1.3));
                })
                .paint_key(1)
                .absolute()
                .right(px(0.))
                .top(px(0.))
                .h_full()
                .w(px(2.)),
            )
    }

    fn qr_for(&mut self, url: Option<String>, cx: &mut olo_kit::Cx<Self>) -> Option<Arc<qrcode::QrCode>> {
        if self.qr_url != url {
            self.qr = None;
            self.qr_url = url.clone();
            if let Some(url) = url {
                cx.work(
                    move || {
                        let code = qrcode::QrCode::new(url.as_bytes()).ok().map(Arc::new);
                        (url, code)
                    },
                    |(url, code)| WinMsg::QrReady(url, code),
                );
            }
        }
        self.qr.clone()
    }

    fn render_secondary_panel(&mut self, cx: &mut olo_kit::Cx<Self>) -> Element {
        let hub = app().hub.clone();
        match self.role {
            WinRole::Hosts => {
                let hub_r = hub.clone();
                let hub_d = hub.clone();
                let window = self.id;
                let hub_f = hub.clone();
                let hub_s = hub.clone();
                let askpass = app().askpass.clone();
                // The hosts as they stand now, for the list to draw
                // from; the one chosen kept by its key, so it stays
                // chosen as others come and go (the first, to begin with).
                self.hosts_rows = Rc::new(panels::host_rows(&hub, &|endpoint| askpass.waits_for_agent(endpoint)));
                let chosen = self.hosts_chosen.as_ref().and_then(|key| self.hosts_rows.iter().position(|h| &h.key == key)).or((!self.hosts_rows.is_empty()).then_some(0));
                self.hosts_chosen = chosen.map(|i| self.hosts_rows[i].key.clone());
                if self.hosts_list.state().single() != chosen
                    && let Some(i) = chosen
                {
                    self.hosts_list.state_mut().select(i);
                }
                // The ports likewise: the one chosen kept by what it is.
                self.ports_rows = Rc::new(panels::port_rows(&hub));
                let port = self.ports_chosen.as_ref().and_then(|kind| self.ports_rows.iter().position(|p| &p.kind == kind));
                if port.is_none() {
                    self.ports_chosen = None;
                }
                if self.ports_list.state().single() != port {
                    match port {
                        Some(i) => self.ports_list.state_mut().select(i),
                        None => self.ports_list.state_mut().selected.clear(),
                    }
                }
                // (A port is forwarded from a host other than this
                // computer, through a session of it.)
                let can_forward = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.hosts.values().any(|h| !h.is_local && !h.session_summaries.is_empty())
                };
                let (hosts, ports) = (self.hosts_rows.clone(), self.ports_rows.clone());
                let (hosts_list, ports_list) = (self.hosts_list.clone(), self.ports_list.clone());
                let hub_k = hub.clone();
                let panel = panels::render_hosts_panel(
                    cx,
                    &hosts_list,
                    &hosts,
                    chosen,
                    &ports_list,
                    &ports,
                    port,
                    can_forward,
                    cx.msg_with(|_: &()| WinMsg::HostsChosen),
                    cx.msg_with(|_: &()| WinMsg::Command("app.add_host".into())),
                    cx.msg_with(move |(hk, disconnect): &(String, bool)| {
                        if *disconnect {
                            hub_r.disconnect_host(hk);
                        } else {
                            hub_r.connect_host(hk);
                        }
                        WinMsg::Redraw
                    }),
                    cx.msg_with(move |hk: &String| {
                        hub_d.remove_host(hk);
                        WinMsg::Redraw
                    }),
                    cx.msg_with(|hk: &String| WinMsg::HostOpen(hk.clone())),
                    cx.msg_with(|_: &()| WinMsg::PortsChosen),
                    cx.msg_with(|_: &()| WinMsg::Command("app.forward_port".into())),
                    cx.msg_with(move |(fid, keep): &(String, bool)| {
                        hub_k.set_forward_kept(fid, *keep);
                        WinMsg::Redraw
                    }),
                    cx.msg_with(move |fid: &String| {
                        hub_s.remove_forward(fid);
                        WinMsg::Redraw
                    }),
                    cx.msg_with(move |kind: &panels::PortKind| {
                        match kind {
                            panels::PortKind::Listening { port, here: true, .. } => hub_f.open_local_port(*port, Feedback::start(window)),
                            panels::PortKind::Listening { host_key, session_id, port, here: false } => {
                                hub_f.add_local_forward(host_key, session_id, "127.0.0.1".to_owned(), *port, 0, true, false, Feedback::start(window))
                            }
                            panels::PortKind::Forwarded { local_port, .. } => hub_f.open_local_port(*local_port, Feedback::start(window)),
                        }
                        WinMsg::Redraw
                    }),
                );
                div().w_full().flex().flex_col().child(panel).children(self.feedback_line(cx).map(|el| div().px(px(m::WINDOW_INSET)).pb(px(m::SPACING)).child(el)))
            }
            WinRole::Forward => {
                let from = self.forward_hosts();
                let chosen = self.fwd_host.as_ref().and_then(|key| from.iter().position(|(k, _)| k == key)).unwrap_or(0);
                let Some(pf) = &self.fwd_port_field else {
                    return div();
                };
                let typed = pf.read(cx).text().trim().parse::<u16>().is_ok_and(|p| p > 0);
                let keys: Vec<String> = from.iter().map(|(k, _)| k.clone()).collect();
                div()
                    .w_full()
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" {
                            cx.stop_propagation();
                            cx.send(WinMsg::ForwardDone(false));
                        }
                    }))
                    .child(panels::render_forward_panel(
                        cx,
                        pf,
                        from.iter().map(|(_, name)| name.clone().into()).collect(),
                        chosen,
                        self.fwd_keep,
                        typed && !from.is_empty(),
                        cx.msg_with(move |i: &usize| WinMsg::HostsForwardFrom(keys.get(*i).cloned().unwrap_or_default())),
                        cx.msg_with(|keep: &bool| WinMsg::ForwardKeep(*keep)),
                        cx.msg_with(|forward: &bool| WinMsg::ForwardDone(*forward)),
                    ))
            }
            WinRole::Server => {
                let Some(managed) = crate::server::current() else {
                    return div();
                };
                // The list's rows as the host last said, the one chosen
                // kept by its name; the log's text as it was last read.
                self.server_rows = Rc::new(crate::server_panel::person_rows(&managed));
                let chosen = self.server_chosen.as_ref().and_then(|name| self.server_rows.iter().position(|r| &r.name == name));
                if chosen.is_none() {
                    self.server_chosen = None;
                }
                if self.server_list.state().single() != chosen {
                    match chosen {
                        Some(i) => self.server_list.state_mut().select(i),
                        None => self.server_list.state_mut().selected.clear(),
                    }
                }
                if let Some(log) = &self.server_log {
                    // (Its colours are the look's: set again when that changes.)
                    let look = theme::palette();
                    let key = style::key((&managed.log, managed.log_of, managed.log_range, managed.log_reading, look.document_text, look.warning, look.failure));
                    if key != self.server_log_key {
                        self.server_log_key = key;
                        let (text, runs) = crate::server_panel::log_text(managed.log.as_deref().unwrap_or_default());
                        let empty = if managed.log_reading || managed.log.is_none() { "Reading the log…" } else { "Nothing was written in that time." };
                        // (The newest last, and shown first.)
                        log.update(cx, |v, cx| {
                            v.set_empty_text(empty, cx);
                            let other = v.text() != text;
                            v.set_text(text, Some(runs), cx);
                            if other {
                                v.scroll_to_end(cx);
                            }
                        });
                    }
                }
                let (list, rows, log) = (self.server_list.clone(), self.server_rows.clone(), self.server_log.as_ref().map(|v| v.element()));
                div()
                    .w_full()
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" {
                            cx.stop_propagation();
                            // (Escape leaves what is asked; what is being
                            // done isn't left by a key, nor the page.)
                            match crate::server::current().map(|m| (m.stage, m.server.is_some())) {
                                Some((crate::server::Stage::Confirm { .. } | crate::server::Stage::Failed { .. }, true)) => cx.send(WinMsg::ServerDo(crate::server_panel::ServerDo::Back)),
                                Some((crate::server::Stage::Working { .. } | crate::server::Stage::Shown, _)) => {}
                                _ => cx.send(WinMsg::ServerDo(crate::server_panel::ServerDo::Close)),
                            }
                        }
                    }))
                    .child(crate::server_panel::render_server_panel(
                        cx,
                        &managed,
                        self.setup_phase,
                        &list,
                        &rows,
                        chosen,
                        log,
                        cx.msg_with(|what: &crate::server_panel::ServerDo| WinMsg::ServerDo(what.clone())),
                        cx.msg_with(|i: &usize| WinMsg::ServerKey(*i)),
                        cx.msg_with(|i: &usize| WinMsg::ServerPage(*i)),
                        cx.msg_with(|i: &usize| WinMsg::ServerPerson(*i)),
                        cx.msg_with(|i: &usize| WinMsg::ServerRange(*i)),
                        cx.msg_with(|_: &()| WinMsg::ServerChosen),
                    ))
            }
            WinRole::Repos => {
                // (How each stands is said of the Account window's host.)
                let (repos, stands) = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    (st.repos.clone(), crate::account::current().and_then(|shown| st.host_stands(&shown.key)))
                };
                let shown = crate::account::current().unwrap_or_default();
                let reached = stands.as_ref().filter(|h| h.conn == HostConnState::Connected && h.ssh);
                let cloning = shown.doing(crate::account::Item::Repos).map(|doing| crate::repos::Cloning(doing.of.clone()));
                // (What a run couldn't clone: which, and what the host said.)
                let why = shown.failed(crate::account::Item::Repos);
                let failed = shown.failed_repo.as_deref().zip(why);
                self.repos_rows = Rc::new(crate::repos::rows(&repos, stands.as_ref(), cloning.as_ref(), failed));
                let failed_line = why.map(|why| match &shown.failed_repo {
                    Some(name) => olo_kit::tr!("{} couldn’t be cloned: {}", name, why).to_string(),
                    None => why.to_owned(),
                });
                let chosen = self.repos_list.state().single().filter(|i| *i < self.repos_rows.len());
                let left: Vec<olo_rex_client::repos::There> = reached
                    .filter(|h| crate::account_panel::left(h, &shown).contains(&crate::account::Item::Repos))
                    .and_then(|h| h.repos_there.clone())
                    .map_or(Vec::new(), |there| there.into_iter().filter(|t| t.to_do()).collect());
                let (to_clone, set_up_only) = (left.len(), !left.is_empty() && left.iter().all(|t| matches!(t, olo_rex_client::repos::There::Cloned { .. })));
                let (rows, list) = (self.repos_rows.clone(), self.repos_list.clone());
                div().w_full().child(crate::repos::render_panel(
                    cx,
                    &list,
                    &rows,
                    chosen,
                    reached.map(|h| h.name.as_str()),
                    to_clone,
                    set_up_only,
                    failed_line,
                    cx.msg_with(|_: &()| WinMsg::ReposChosen),
                    cx.msg_with(|what: &crate::repos::ReposDo| WinMsg::ReposDo(what.clone())),
                ))
            }
            WinRole::RepoEdit => {
                let Some(fields) = &self.repo_fields else {
                    return div();
                };
                let others = self.repo_others();
                div()
                    .w_full()
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" {
                            cx.stop_propagation();
                            cx.send(WinMsg::RepoEditDone(false));
                        }
                    }))
                    .child(crate::repos::render_form(cx, fields, &others, crate::repos::editing().is_none(), cx.msg_with(|done: &bool| WinMsg::RepoEditDone(*done))))
            }
            WinRole::Account => {
                let Some(shown) = crate::account::current() else {
                    return div();
                };
                let Some(stands) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_stands(&shown.key) else {
                    return div();
                };
                div().w_full().child(crate::account_panel::render_account_panel(&stands, &shown, cx.msg_with(|what: &crate::account_panel::AccountDo| WinMsg::AccountDo(what.clone()))))
            }
            WinRole::Carry => {
                let Some(shown) = crate::carry::current() else {
                    return div();
                };
                self.carry_changes_rows = Rc::new(crate::carry_panel::change_rows(&shown));
                self.carry_rem_rows = Rc::new(crate::carry_panel::remembered_rows(&shown));
                if let Some(demo_sel) = crate::carry::take_demo_selection() {
                    if shown.tab == 0 {
                        let len = self.carry_changes_rows.len();
                        let mut st = self.carry_changes_list.state_mut();
                        st.selected = demo_sel.into_iter().filter(|&i| i < len).collect();
                        st.anchor = st.selected.iter().next().copied();
                        st.cursor = st.selected.iter().next().copied();
                    } else {
                        let len = self.carry_rem_rows.len();
                        let mut st = self.carry_rem_list.state_mut();
                        st.selected = demo_sel.into_iter().filter(|&i| i < len).collect();
                        st.anchor = st.selected.iter().next().copied();
                        st.cursor = st.selected.iter().next().copied();
                    }
                }
                {
                    let len = self.carry_changes_rows.len();
                    let mut st = self.carry_changes_list.state_mut();
                    st.selected.retain(|&i| i < len);
                    if st.selected.is_empty() && len > 0 {
                        st.select(0);
                    }
                }
                {
                    let len = self.carry_rem_rows.len();
                    let mut st = self.carry_rem_list.state_mut();
                    st.selected.retain(|&i| i < len);
                }
                let changes_list = self.carry_changes_list.clone();
                let changes_rows = self.carry_changes_rows.clone();
                let rem_list = self.carry_rem_list.clone();
                let rem_rows = self.carry_rem_rows.clone();
                let here_scroll = self.carry_here_scroll.clone();
                let there_scroll = self.carry_there_scroll.clone();
                div()
                    .w_full()
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" && !crate::carry::current().is_some_and(|s| s.applying) {
                            cx.stop_propagation();
                            cx.request_close();
                        }
                    }))
                    .child(crate::carry_panel::render_carry_panel(
                        cx,
                        &shown,
                        &changes_list,
                        &changes_rows,
                        &rem_list,
                        &rem_rows,
                        &here_scroll,
                        &there_scroll,
                        cx.msg_with(|_: &()| WinMsg::CarryChangesChosen),
                        cx.msg_with(|_: &()| WinMsg::CarryRemChosen),
                        cx.msg_with(|what: &crate::carry_panel::CarryDo| WinMsg::CarryDo(what.clone())),
                    ))
            }
            WinRole::Host => {
                let Some(shown) = crate::host::current() else {
                    return div();
                };
                let Some(stands) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_stands(&shown.key) else {
                    return div();
                };
                let managed = managed_host(&hub, &shown.key).is_some();
                div().w_full().child(crate::host_panel::render_host_panel(&stands, &shown, managed, cx.msg_with(|what: &crate::host_panel::HostDo| WinMsg::HostDo(what.clone()))))
            }
            WinRole::Move => {
                let Some(mv) = crate::moving::current() else {
                    return div();
                };
                div().w_full().flex_1().flex().flex_col().child(panels::render_move_panel(&mv, self.setup_phase, cx.msg_with(|what: &panels::MoveDo| WinMsg::MoveDo(what.clone()))))
            }
            WinRole::AddHost => {
                let (Some(name), Some(address)) = (&self.host_label_field, &self.host_addr_field) else {
                    return div();
                };
                // (It fills its window: an alert shorter than the window's
                // least height has its stripe to the window's foot, not
                // to its own.)
                div()
                    .w_full()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" {
                            cx.stop_propagation();
                            // (A step being carried out isn't left by a key.)
                            match crate::setup::current().map(|s| s.stage) {
                                Some(crate::setup::Stage::Working { .. }) => {}
                                Some(_) => cx.send(WinMsg::SetupDo(panels::SetupDo::Back)),
                                None => cx.send(WinMsg::AddHostDone(false)),
                            }
                        }
                    }))
                    .child(div().w_full().flex_1().flex().flex_row().child(match crate::setup::current() {
                        Some(setup) => {
                            panels::render_setup_panel(&setup, self.setup_phase, cx.msg_with(|what: &panels::SetupDo| WinMsg::SetupDo(*what)), cx.msg_with(|i: &usize| WinMsg::SetupKey(*i)))
                        }
                        None => panels::render_add_host_panel(
                            cx,
                            name,
                            address,
                            &self.add_host_address(cx),
                            self.add_mode,
                            self.add_set_up,
                            cx.msg_with(|i: &usize| WinMsg::AddHostMode(*i)),
                            cx.msg_with(|on: &bool| WinMsg::AddHostSetUp(*on)),
                            cx.msg_with(|add: &bool| WinMsg::AddHostDone(*add)),
                        ),
                    }))
            }
            WinRole::Access => {
                let step = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    let local = st.hosts.get("local").filter(|h| h.conn_state == HostConnState::Connected);
                    panels::Step::of(local.and_then(|h| h.tailscale.as_ref()))
                };
                let code = self.qr_for(step.code().map(str::to_owned), cx);
                let window = self.id;
                let hub_ts = hub.clone();
                let feedback = self.feedback_line(cx);
                panels::render_access_panel(
                    &hub,
                    feedback,
                    code,
                    cx.msg_with(move |action: &crate::tailscale::Action| {
                        hub_ts.change_tailscale("local", action.clone(), Feedback::start(window));
                        WinMsg::Redraw
                    }),
                    cx.msg_with(|u: &String| WinMsg::SetClipboard(u.clone())),
                    cx.msg_with(|_: &()| WinMsg::CloseWindow),
                )
            }
            WinRole::Diagnostics => panels::render_diagnostics_panel(
                &hub,
                &app().ssh_agent.clone(),
                cx.msg_with(|v: &String| WinMsg::SetClipboard(v.clone())),
                cx.msg_with(|d: &String| WinMsg::SetClipboard(d.clone())),
                cx.msg_with(|_: &()| WinMsg::CloseWindow),
            ),
            WinRole::Askpass => {
                let req = app().askpass.current_request();
                match req {
                    Some(AskpassRequest::ConfirmHostKey { host, port, fingerprint, key_type, changed }) => div()
                        .flex()
                        .flex_col()
                        .gap(px(m::SECTION))
                        .p(px(m::WINDOW_INSET))
                        .w_full()
                        .child(Banner::new(
                            Tone::Failure,
                            if changed { "SSH Host Key Changed" } else { "Unknown SSH Host Key" },
                            if changed { format!("HOST KEY CHANGED for {host}:{port}! Possible MITM attack.") } else { format!("Verify fingerprint before trusting {host}:{port}.") },
                        ))
                        .child(StringView::new(format!("Key type: {key_type}")).bold(true))
                        .child(StringView::new(format!("SHA-256: {fingerprint}")).dim(true))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(px(m::SPACING))
                                .child(Button::new("Reject").on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::AskpassConfirm(false))))
                                .child(Button::new("Trust & Connect").default(true).on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::AskpassConfirm(true)))),
                        ),
                    Some(AskpassRequest::Secret { title, prompt, .. }) => {
                        let field_el = self.askpass_field.as_ref().map(|f| TextControl::new(cx, f).into_element()).unwrap_or_else(|| div());
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(m::SECTION))
                            .p(px(m::WINDOW_INSET))
                            .w_full()
                            .child(StringView::new(title).bold(true))
                            .child(StringView::new(prompt).dim(true))
                            .child(field_el)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .gap(px(m::SPACING))
                                    .child(Button::new("Cancel").on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::AskpassConfirm(false))))
                                    .child(Button::new("Authenticate").default(true).on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::AskpassSubmit))),
                            )
                    }
                    None => div().p(px(m::WINDOW_INSET)).child(StringView::new("No active SSH authentication request.").dim(true)),
                }
            }
            WinRole::RunApprove => {
                let prompt = app().run_approvals.current();
                div()
                    .w_full()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        // Both Escape and Return deny: unlike `olo-sudo` (where the
                        // user typed `sudo` and expects the prompt), `olo run` is
                        // started by another machine and may appear over a terminal
                        // the user is typing in. A stray Return must never allow it.
                        if matches!(e.keystroke.key.as_str(), "escape" | "enter" | "return") && !e.keystroke.modifiers.modified() {
                            cx.stop_propagation();
                            cx.send(WinMsg::RunApproveChoice(false));
                        }
                    }))
                    .child(panels::render_run_approve_panel(prompt, &self.run_scroll, cx.msg_with(|allow: &bool| WinMsg::RunApproveChoice(*allow))))
            }
            WinRole::Rename => {
                let what = app().renaming.clone();
                let typed = self.askpass_field.as_ref().map(|f| f.read(cx).text().trim().to_owned()).unwrap_or_default();
                let field = self.askpass_field.as_ref().map(|f| TextControl::new(cx, f).into_element()).unwrap_or_else(|| div());
                // The same words for both, saying which it is; under a
                // tab's field, what leaving it empty does (its field
                // shows that name while empty).
                let (asks, under) = match &what {
                    Some(r) if r.window_id.is_some() => (olo_kit::tr!("A new name for the tab “{}”:", r.shown), Some("Leave it empty to show what runs in the tab.")),
                    Some(r) => (olo_kit::tr!("A new name for the session “{}”:", r.shown), None),
                    None => (olo_kit::words::tr("Nothing to rename.").into(), None),
                };
                let can = what.as_ref().is_some_and(|r| r.takes(&typed));
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .p(px(m::WINDOW_INSET))
                    .w_full()
                    .on_key_down(cx.listener(|_, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" {
                            cx.stop_propagation();
                            cx.send(WinMsg::RenameDone(false));
                        }
                    }))
                    .child(StringView::new(asks).wrap(true))
                    // (The line under the field is the field's: close under it.)
                    .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(field).children(under.map(|u| StringView::new(u).size(style::SMALL_SIZE).dim(true).wrap(true))))
                    .child(
                        widgets::button_row()
                            .pt(px(m::TIGHT))
                            .child(Button::new("Cancel").on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::RenameDone(false))))
                            .child(Button::new("Rename").default(true).enabled(can).on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::RenameDone(true)))),
                    )
            }
            WinRole::Workspace => div(),
        }
    }
}

impl View for RexWindow {
    type Message = WinMsg;

    fn message(&mut self, msg: WinMsg, cx: &mut olo_kit::Cx<Self>) {
        match msg {
            WinMsg::QrReady(url, qr) => {
                if self.qr_url.as_ref() == Some(&url) {
                    self.qr = qr;
                    cx.invalidate();
                }
            }
            WinMsg::StartFeedback(serial) => {
                self.clear_feedback();
                self.feedback_serial = serial;
                cx.invalidate();
            }
            WinMsg::Problem(serial, title, detail) => {
                if self.feedback_serial == serial {
                    self.cancel_preview();
                    let panel = self.role == WinRole::Access;
                    let view = detail.map(|s| widgets::text_view(cx, |v| v.panel_well(panel).with_text(s, None).auto_hide(true)));
                    self.problem = Some((title, view));
                    cx.invalidate();
                }
            }
            WinMsg::Confirmation(serial, title) => {
                if self.feedback_serial == serial {
                    self.notice.say(title, Duration::from_secs(4));
                    cx.invalidate();
                }
            }
            // (Never in place of what the person is being told about
            // something they did: that stays until they try again.)
            WinMsg::Said(text) => {
                if self.problem.is_none() && self.notice.text().is_none() {
                    self.notice.say(text, Duration::from_secs(6));
                    cx.invalidate();
                }
            }
            WinMsg::ShowTarget(target, token) => {
                self.pending_focus = None;
                self.overlay = None;
                self.find_open = false;
                self.focus.focus(cx);
                match target {
                    Target::Host(hk) => self.jump_to(JumpTarget::Host(hk), cx),
                    Target::Session { host_key, session_id } => self.jump_to(JumpTarget::Session { host_key, session_id }, cx),
                    Target::Pane(bk) => {
                        self.active_host = bk.host_key.clone();
                        self.active_session = Some(bk.session_id.clone());
                        self.pending_focus = Some(bk);
                    }
                }
                if self.side_pane.alone() {
                    self.side_pane.show_rest();
                }
                if let Some(token) = token {
                    cx.activate_window_with_token(&token);
                } else {
                    cx.activate_window();
                }
                cx.invalidate();
            }
            WinMsg::Created(serial, hk, sid) => {
                if self.feedback_serial == serial {
                    self.created_session = Some((hk, sid));
                    cx.invalidate();
                }
            }
            WinMsg::CreateSession(hk, dir) => app().hub.create_session(&hk, None, dir, Feedback::start(self.id)),
            WinMsg::Redraw => {
                let visuals = &mut cx.window_state::<PaneVisuals>().0;
                if !visuals.is_empty() {
                    let hub = app().hub.clone();
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    visuals.retain(|k, _| st.blocks.contains_key(k));
                }
                if let Some(p) = &self.panes_view {
                    p.update(cx, |_, cx| cx.invalidate());
                }
                cx.invalidate();
            }
            WinMsg::Screens => {
                if let Some(p) = &self.panes_view {
                    p.update(cx, |_, cx| cx.invalidate_at_frame());
                } else {
                    cx.invalidate_at_frame();
                }
            }
            WinMsg::Raise => {
                cx.activate_window();
                cx.invalidate();
            }
            WinMsg::ShowPrompt { host_key, req } => {
                if self.active_host != host_key {
                    let hub = app().hub.clone();
                    hub.respond_prompt(&host_key, &req.call_id, true, None);
                    return;
                }
                let mut checked = HashSet::new();
                if let Some(items) = &req.prompt.items {
                    for it in items {
                        if it.selected {
                            checked.insert(it.id.clone());
                        }
                    }
                }
                let init_val = req.prompt.value.clone().unwrap_or_default();
                self.overlay = Some(ActiveOverlay::RexPrompt { host_key, req, query: init_val.clone(), selected_idx: 0, checked_ids: checked });
                if let Some(f) = &self.overlay_field {
                    f.update(cx, |f, cx| f.set_text(init_val, cx));
                    f.focus(cx);
                }
                cx.invalidate();
            }
            WinMsg::EndPrompt { host_key, call_id } => {
                if matches!(&self.overlay, Some(ActiveOverlay::RexPrompt { host_key: source, req, .. }) if source == &host_key && req.call_id == call_id) {
                    self.overlay = None;
                    self.focus.focus(cx);
                    cx.invalidate();
                }
            }
            WinMsg::PerformAction { host, session, block, action, args } => {
                let focused = self.focused_block_key();
                if host == self.active_host
                    && session.as_ref().is_none_or(|s| Some(s) == self.active_session.as_ref())
                    && block.as_ref().is_none_or(|b| focused.as_ref().is_some_and(|f| &f.block_id == b))
                {
                    self.execute_action(&action, args, cx);
                }
            }
            WinMsg::OpenPreview(bk, path) => {
                self.open_preview(bk, path);
                cx.invalidate();
            }
            WinMsg::StopTransfer => {
                self.cancel_preview();
                cx.invalidate();
            }
            WinMsg::SetClipboard(txt) => {
                olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(txt));
            }
            WinMsg::PasteText(bk, connection, txt) => {
                if self.focused_block_key().as_ref() == Some(&bk) {
                    let hub = app().hub.clone();
                    let bracketed = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).is_some_and(|b| b.lock().unwrap_or_else(|e| e.into_inner()).terminal.bracketed_paste_mode());
                    let encoded = encode_paste(&txt, bracketed).unwrap_or_else(|_| txt.into_bytes());
                    hub.write_input(&bk, encoded, Some(&connection));
                }
            }
            WinMsg::PasteImage(bk, mime, bytes) => {
                if self.focused_block_key().as_ref() == Some(&bk) {
                    let feedback = Feedback::start(self.id);
                    // (Written on a worker: a picture is megabytes.)
                    std::thread::spawn(move || match pasted_image_file(&mime, &bytes) {
                        Ok(path) => app().hub.upload_files(&bk, vec![path], feedback),
                        Err(e) => feedback.problem("Couldn’t paste the picture.", e),
                    });
                }
            }
            WinMsg::FocusBlock(bk) => {
                crate::activity_watch::close_viewer();
                self.jump_to(JumpTarget::Pane(bk), cx);
                cx.activate_window();
            }
            WinMsg::ActivityChanged => {
                cx.invalidate();
            }
            WinMsg::ToggleActivityFold(bk) => {
                crate::activity_watch::toggle_fold(&bk);
            }
            WinMsg::OpenSubagent(parent, sub, force_transcript) => {
                let hub = app().hub.clone();
                self.jump_to(JumpTarget::Pane(parent.clone()), cx);
                crate::activity_watch::open_subagent(&hub, &parent, sub, force_transcript);
                cx.invalidate();
            }
            WinMsg::OpenPaneTranscript(bk, title) => {
                let hub = app().hub.clone();
                self.jump_to(JumpTarget::Pane(bk.clone()), cx);
                crate::activity_watch::open_pane_transcript(&hub, &bk, title);
                cx.invalidate();
            }
            WinMsg::TranscriptDo(what) => {
                use crate::transcript_view::TranscriptDo;
                let hub = app().hub.clone();
                match what {
                    TranscriptDo::ShowTerminal => {
                        if let Some(crate::activity_watch::ViewTarget::SubagentTranscript { parent, sub }) = crate::activity_watch::active_view_target()
                            && sub.tmux.is_some()
                        {
                            crate::activity_watch::open_subagent(&hub, &parent, sub, false);
                        } else {
                            crate::activity_watch::close_viewer();
                        }
                    }
                    TranscriptDo::ShowTranscript => {
                        if let Some(crate::activity_watch::ViewTarget::SubagentTmux { parent, sub, .. }) = crate::activity_watch::active_view_target() {
                            crate::activity_watch::open_subagent(&hub, &parent, sub, true);
                        }
                    }
                    TranscriptDo::ToggleExpand(eid) => {
                        if let Some(tr) = crate::activity_watch::active_transcript() {
                            let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                            let is_failed_tool = g
                                .doc
                                .entries
                                .iter()
                                .any(|e| matches!(e, olo_rex_client::activity::TranscriptEntry::ToolCall { id, status: olo_rex_client::activity::ToolStatus::Error, .. } if *id == eid));
                            if is_failed_tool {
                                if g.collapsed_errors.contains(&eid) {
                                    g.collapsed_errors.remove(&eid);
                                    g.expanded.insert(eid);
                                } else {
                                    g.expanded.remove(&eid);
                                    g.collapsed_errors.insert(eid);
                                }
                            } else if !g.expanded.remove(&eid) {
                                g.expanded.insert(eid);
                            }
                            g.generation += 1;
                        }
                        cx.invalidate();
                    }
                    TranscriptDo::ToggleFollow => {
                        if let Some(tr) = crate::activity_watch::active_transcript() {
                            let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                            g.follow = !g.follow;
                            if g.follow {
                                self.transcript_scroll.set_offset(cx, point(px(0.0), px(-1_000_000.0)));
                            }
                            g.generation += 1;
                        }
                        cx.invalidate();
                    }
                    TranscriptDo::ToggleFind => {
                        if self.find_open {
                            self.find_open = false;
                            if let Some(tr) = crate::activity_watch::active_transcript() {
                                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                                g.clear_search();
                                g.generation += 1;
                            }
                            cx.invalidate();
                        } else {
                            self.execute_action("terminal.find", None, cx);
                        }
                    }
                    TranscriptDo::Close => {
                        crate::activity_watch::close_viewer();
                        self.find_open = false;
                        cx.invalidate();
                    }
                }
            }
            WinMsg::PaneCommand(key, cmd) => {
                if self.focused_block_key().as_ref() == Some(&key) {
                    self.execute_action(&cmd, None, cx);
                }
            }
            WinMsg::Command(cmd) => self.execute_action(&cmd, None, cx),
            WinMsg::RunOverlayIndex(i) => self.pick_overlay_index(i, cx),
            WinMsg::TogglePromptCheck(id) => {
                if let Some(ActiveOverlay::RexPrompt { checked_ids, .. }) = &mut self.overlay {
                    if !checked_ids.remove(&id) {
                        checked_ids.insert(id);
                    }
                    cx.invalidate();
                }
            }
            WinMsg::ConfirmOverlay => self.confirm_overlay(cx),
            WinMsg::DismissOverlay => self.dismiss_overlay(cx),
            WinMsg::TabChanged => {
                self.dismiss_overlay(cx);
                self.find_open = false;
                self.clear_feedback();
                self.pending_focus = None;
                self.focus.focus(cx);
                cx.invalidate();
            }
            WinMsg::SelectHost(hk) => {
                self.dismiss_overlay(cx);
                self.find_open = false;
                self.clear_feedback();
                self.pending_focus = None;
                self.jump_to(JumpTarget::Host(hk), cx);
                self.focus.focus(cx);
                if self.side_pane.alone() {
                    self.side_pane.show_rest();
                }
            }
            WinMsg::SelectSession(hk, sid) => {
                self.dismiss_overlay(cx);
                self.clear_feedback();
                self.pending_focus = None;
                self.focus.focus(cx);
                self.find_open = false;
                self.active_host = hk;
                self.active_session = Some(sid);
                if self.side_pane.alone() {
                    self.side_pane.show_rest();
                }
                cx.invalidate();
            }
            WinMsg::FindStep(back) => self.step_find(back, cx),
            WinMsg::FindClose => {
                self.find_open = false;
                if let Some(tr) = crate::activity_watch::active_transcript() {
                    let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                    g.clear_search();
                    g.generation += 1;
                }
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = app().hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let _ = br.lock().unwrap_or_else(|e| e.into_inner()).set_find_query(None);
                }
                self.focus.focus(cx);
                cx.invalidate();
            }
            WinMsg::HostsChosen => {
                let row = self.hosts_list.state().single();
                self.hosts_chosen = row.and_then(|i| self.hosts_rows.get(i)).map(|h| h.key.clone());
                cx.invalidate();
            }
            WinMsg::PortsChosen => {
                let row = self.ports_list.state().single();
                self.ports_chosen = row.and_then(|i| self.ports_rows.get(i)).map(|p| p.kind.clone());
                cx.invalidate();
            }
            WinMsg::ForwardDone(true) => self.submit_forward(cx),
            WinMsg::ForwardDone(false) => cx.request_close(),
            WinMsg::ForwardKeep(keep) => {
                self.fwd_keep = keep;
                cx.invalidate();
            }
            WinMsg::AddHostDone(true) => self.submit_add_host(cx),
            WinMsg::AddHostDone(false) => cx.request_close(),
            WinMsg::AddHostMode(i) => {
                self.add_mode = olo_rex_client::ServerMode::ALL.get(i).copied().unwrap_or_default();
                cx.invalidate();
            }
            WinMsg::AddHostSetUp(on) => {
                self.add_set_up = on;
                cx.invalidate();
            }
            WinMsg::SetupChanged => {
                let stage = crate::setup::current().map(|s| s.stage);
                // A busy bar moves while the host is waited for, and
                // only then: a timer begun with the wait and ended with
                // it (ten times a second: a block's step along its bar).
                let waited_for = matches!(stage, Some(crate::setup::Stage::Looking | crate::setup::Stage::Root { .. }));
                match (waited_for, self.setup_tick) {
                    (true, None) => {
                        self.setup_phase = 0.0;
                        self.setup_tick = Some(cx.every(std::time::Duration::from_millis(100), || WinMsg::SetupTick));
                    }
                    (false, Some(timer)) => {
                        cx.cancel(timer);
                        self.setup_tick = None;
                    }
                    _ => {}
                }
                // (Set up and added: the window has done what it was for.)
                if stage == Some(crate::setup::Stage::Done) {
                    crate::setup::back();
                    cx.request_close();
                } else {
                    cx.invalidate();
                }
            }
            WinMsg::MoveChanged => {
                let stage = crate::moving::current().map(|m| m.stage);
                // (Its busy bar moves while both hosts are looked at, and
                // only then: Add Host's timer, the same way.)
                // (Only while nothing measured is shown: a copy's bar moves
                // by its own status, twice a second, for hours.)
                let waited_for = match &stage {
                    Some(crate::moving::Stage::Looking { .. } | crate::moving::Stage::GoingBack) => true,
                    Some(crate::moving::Stage::Copying { status }) => status["total_bytes"].as_u64().unwrap_or(0) == 0,
                    _ => false,
                };
                match (waited_for, self.setup_tick) {
                    (true, None) => {
                        self.setup_phase = 0.0;
                        self.setup_tick = Some(cx.every(std::time::Duration::from_millis(100), || WinMsg::SetupTick));
                    }
                    (false, Some(timer)) => {
                        cx.cancel(timer);
                        self.setup_tick = None;
                    }
                    _ => {}
                }
                if stage.is_none() && self.role == WinRole::Move {
                    cx.close_window();
                    return;
                }
                cx.invalidate();
            }
            WinMsg::MoveDo(what) => {
                let hub = app().hub.clone();
                match what {
                    panels::MoveDo::Pick(i) => crate::moving::pick(i),
                    panels::MoveDo::Choose => {
                        let chosen = match crate::moving::current().map(|m| m.stage) {
                            Some(crate::moving::Stage::Choosing { chosen, .. }) => chosen,
                            _ => 0,
                        };
                        crate::moving::choose(&hub, chosen);
                    }
                    panels::MoveDo::VarTmp(on) => crate::moving::choose_option(Some(on), None),
                    panels::MoveDo::Cache(on) => crate::moving::choose_option(None, Some(on)),
                    panels::MoveDo::Go => crate::moving::go(&hub),
                    panels::MoveDo::GoBack => crate::moving::go_back(&hub),
                    // (No other host to move to: Add Host, this window's
                    // move given up.)
                    panels::MoveDo::AddHost => {
                        crate::moving::cancel(&hub);
                        open_or_raise_panel(WinRole::AddHost);
                    }
                    // (Refused: the move given up, begun again for another
                    // host; nothing was changed on either.)
                    panels::MoveDo::Rechoose => {
                        if let Some(source) = crate::moving::current().map(|m| m.source) {
                            crate::moving::cancel(&hub);
                            crate::moving::begin(&hub, &source);
                        }
                    }
                    panels::MoveDo::Cancel if move_just_changed() => {}
                    panels::MoveDo::Cancel | panels::MoveDo::Close => crate::moving::cancel(&hub),
                }
            }
            WinMsg::HostMoved(from, to, sessions) => {
                if self.active_host == from {
                    self.active_session = self.active_session.as_ref().and_then(|s| sessions.get(s).cloned()).or_else(|| sessions.values().next().cloned());
                    self.active_host = to;
                    cx.invalidate();
                }
            }
            WinMsg::SetupTick => {
                // (A turn there and back in two seconds.)
                self.setup_phase += 0.05;
                cx.invalidate();
            }
            WinMsg::SetupDo(what) => {
                let hub = app().hub.clone();
                match what {
                    panels::SetupDo::Back => crate::setup::back(),
                    panels::SetupDo::Go => crate::setup::go(&hub),
                    panels::SetupDo::Again => crate::setup::again(&hub),
                    panels::SetupDo::Stop => crate::setup::stop(),
                    // (As DiskBackup opens it: the app that runs takes the launch, and comes to the front.)
                    panels::SetupDo::OpenPasswords => {
                        let _ = olo_xdg::spawn_detached("olo-passwords");
                    }
                }
            }
            WinMsg::SetupKey(i) => crate::setup::choose_key(i),
            WinMsg::HostOpen(key) => open_host(&app().hub.clone(), &key),
            WinMsg::AccountOpen(key) => open_account(&app().hub.clone(), &key),
            WinMsg::HostChanged => {
                // (Another host's things in the window: its name in the title.)
                if self.role == WinRole::Host {
                    cx.set_window_title(host_title());
                }
                cx.invalidate();
            }
            WinMsg::AccountChanged => {
                if self.role == WinRole::Account {
                    cx.set_window_title(account_title());
                }
                cx.invalidate();
            }
            WinMsg::AccountDo(what) => {
                use crate::account_panel::AccountDo;
                let hub = app().hub.clone();
                match what {
                    AccountDo::Run(items) => crate::account::run(&hub, &items),
                    AccountDo::RemoveSkill => crate::account::remove_skill(&hub),
                    AccountDo::Stop => crate::account::stop(&hub),
                    AccountDo::Check => crate::account::check(&hub),
                    AccountDo::Repos => open_or_raise_panel(WinRole::Repos),
                    AccountDo::Carry => {
                        if let Some(shown) = crate::account::current() {
                            open_carry(&hub, &shown.key);
                        }
                    }
                    AccountDo::Window(window) => hub.set_sessions_window(window),
                    AccountDo::Host => {
                        if let Some(shown) = crate::account::current() {
                            open_host(&hub, &shown.key);
                        }
                    }
                    AccountDo::Connect => {
                        if let Some(shown) = crate::account::current() {
                            hub.connect_host(&shown.key);
                        }
                    }
                }
            }
            WinMsg::ReposChosen => cx.invalidate(),
            WinMsg::ReposDo(what) => {
                use crate::repos::ReposDo;
                let hub = app().hub.clone();
                match what {
                    ReposDo::Add => {
                        crate::repos::edit(None);
                        open_or_raise_panel(WinRole::RepoEdit);
                    }
                    ReposDo::Edit(i) => {
                        crate::repos::edit(Some(i));
                        open_or_raise_panel(WinRole::RepoEdit);
                    }
                    ReposDo::Remove(i) => {
                        let mut repos = hub.state.lock().unwrap_or_else(|e| e.into_inner()).repos.clone();
                        if i < repos.len() {
                            repos.remove(i);
                            hub.set_repos(repos);
                            crate::account::check(&hub);
                        }
                    }
                    ReposDo::Clone => crate::account::run(&hub, &[crate::account::Item::Repos]),
                }
                cx.invalidate();
            }
            WinMsg::RepoEditDone(false) => cx.request_close(),
            WinMsg::RepoEditDone(true) => {
                // (Kept, in its place or at the list's end, where
                // nothing is wrong with it; the host asked again how
                // they stand.)
                let others = self.repo_others();
                let Some(repo) = self.repo_fields.as_ref().map(|fields| crate::repos::typed(cx, fields)).filter(|repo| crate::repos::wrong(repo, &others).is_none()) else { return };
                let hub = app().hub.clone();
                let mut repos = hub.state.lock().unwrap_or_else(|e| e.into_inner()).repos.clone();
                match crate::repos::editing().filter(|i| *i < repos.len()) {
                    Some(i) => repos[i] = repo,
                    None => repos.push(repo),
                }
                hub.set_repos(repos);
                crate::account::check(&hub);
                cx.request_close();
            }
            WinMsg::CarryChanged => {
                if self.role == WinRole::Carry {
                    cx.set_window_title(carry_title());
                }
                cx.invalidate();
            }
            WinMsg::CarryChangesChosen => {
                self.carry_here_scroll.set_offset(cx, point(px(0.0), px(0.0)));
                self.carry_there_scroll.set_offset(cx, point(px(0.0), px(0.0)));
                cx.invalidate();
            }
            WinMsg::CarryRemChosen => cx.invalidate(),
            WinMsg::CarryDo(what) => {
                use crate::carry_panel::CarryDo;
                let hub = app().hub.clone();
                match what {
                    CarryDo::Tab(i) => {
                        crate::carry::set_tab(i);
                        if i == 0 {
                            self.carry_changes_list.focus(cx);
                        } else {
                            self.carry_rem_list.focus(cx);
                        }
                    }
                    CarryDo::Check => crate::carry::check(&hub),
                    CarryDo::SelectAll => {
                        let len = self.carry_changes_rows.len();
                        self.carry_changes_list.state_mut().select_all(len);
                        self.carry_changes_list.focus(cx);
                        cx.invalidate();
                    }
                    CarryDo::SetChoice(choice) => {
                        let ids: Vec<String> = self.carry_changes_list.state().selected.iter().filter_map(|&i| self.carry_changes_rows.get(i).map(|r| r.id.clone())).collect();
                        crate::carry::set_choices_for(&ids, choice);
                    }
                    CarryDo::SetRemember(remember) => {
                        let ids: Vec<String> = self.carry_changes_list.state().selected.iter().filter_map(|&i| self.carry_changes_rows.get(i).map(|r| r.id.clone())).collect();
                        crate::carry::set_remember_for(&ids, remember);
                    }
                    CarryDo::SetPreviewResult(result) => crate::carry::set_preview_result(result),
                    CarryDo::Apply => crate::carry::apply(&hub),
                    CarryDo::Close => cx.request_close(),
                    CarryDo::ForgetSelected => {
                        let ids: Vec<String> = self.carry_rem_list.state().selected.iter().filter_map(|&i| self.carry_rem_rows.get(i).map(|r| r.id.clone())).collect();
                        if !ids.is_empty() {
                            crate::carry::forget(&hub, None, Some(&ids));
                        }
                    }
                    CarryDo::ForgetAll => crate::carry::forget(&hub, None, None),
                }
            }
            WinMsg::HostDo(what) => {
                use crate::host_panel::HostDo;
                let hub = app().hub.clone();
                match what {
                    HostDo::Move => {
                        if let Some(shown) = crate::host::current() {
                            open_move(&hub, &shown.key);
                        }
                    }
                    HostDo::Account => {
                        if let Some(shown) = crate::host::current() {
                            open_account(&hub, &shown.key);
                        }
                    }
                    HostDo::Change(thing, add) => crate::host::change(&hub, thing, add),
                    HostDo::Check => crate::host::check(&hub),
                    HostDo::Connect => {
                        if let Some(shown) = crate::host::current() {
                            hub.connect_host(&shown.key);
                        }
                    }
                    HostDo::Server => {
                        if let Some(shown) = crate::host::current() {
                            open_server(&hub, &shown.key);
                        }
                    }
                    // (Preferences ▸ Rex, on Hosts, with this host chosen.)
                    HostDo::Permissions => {
                        let key = crate::host::current().map(|shown| shown.key).unwrap_or_default();
                        olo_kit::app_menu::open_settings(&format!("rex/host:{key}"));
                    }
                    HostDo::OpenPool => {
                        let _ = olo_xdg::spawn_detached("olo-pool --hosts-window");
                    }
                    HostDo::AskAll(asking) => crate::host::ask_all(asking),
                    HostDo::RemoveAll(things) => crate::host::remove_all(&hub, &things),
                    HostDo::Tab(tab) => crate::host::show_tab(tab),
                    // (Its copy here, in Tracker; or the host's own folder
                    // here, made if nothing came from it yet.)
                    HostDo::ShowKept(there) => {
                        if let Some(shown) = crate::host::current() {
                            let label = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&shown.key).map(|h| h.label.clone()).unwrap_or(shown.name.clone());
                            let here = match &there {
                                Some(there) => crate::hub::kept_here(&label, there),
                                None => crate::hub::host_here(&label),
                            };
                            if let Some(here) = here {
                                let _ = std::fs::create_dir_all(&here);
                                crate::bridge::open_folder_in_tracker(&here);
                            }
                        }
                    }
                    HostDo::Copy(text) => olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(text)),
                    HostDo::SyncKept(there) => {
                        if let Some(shown) = crate::host::current() {
                            hub.sync_kept_now(&shown.key, &there);
                        }
                    }
                    HostDo::RetryKept(there) => {
                        if let Some(shown) = crate::host::current() {
                            hub.retry_kept(&shown.key, &there);
                        }
                    }
                    HostDo::StopKept(there, confirm) => {
                        if let Some(shown) = crate::host::current() {
                            if confirm {
                                let leaf = there.trim_end_matches('/').rsplit('/').next().unwrap_or(&there).to_owned();
                                let msg = olo_kit::tr!("{} will stop keeping “{}” in step. Both copies stay where they are.", shown.name, leaf).to_string();
                                let hub = hub.clone();
                                let key = shown.key.clone();
                                let _ = olo_kit::widgets::open_alert(cx, "Rex", "network", &msg, &["Cancel", "Forget"], move |choice, _| {
                                    if *choice == Some(1) {
                                        hub.stop_kept(&key, &there);
                                    }
                                });
                            } else {
                                hub.stop_kept(&shown.key, &there);
                            }
                        }
                    }
                }
            }
            WinMsg::ServerChanged => {
                let stage = crate::server::current().map(|m| m.stage);
                // (The busy bar's timer, as Add Host's: begun with a
                // wait and ended with it.)
                let waited_for = matches!(stage, Some(crate::server::Stage::Looking | crate::server::Stage::Working { .. }));
                match (waited_for, self.setup_tick) {
                    (true, None) => {
                        self.setup_phase = 0.0;
                        self.setup_tick = Some(cx.every(std::time::Duration::from_millis(100), || WinMsg::SetupTick));
                    }
                    (false, Some(timer)) => {
                        cx.cancel(timer);
                        self.setup_tick = None;
                    }
                    _ => {}
                }
                // (Another host's server opened in the window: its name
                // in the title.)
                if self.role == WinRole::Server {
                    cx.set_window_title(server_title());
                }
                cx.invalidate();
            }
            WinMsg::ServerDo(what) => {
                use crate::server_panel::ServerDo;
                let hub = app().hub.clone();
                match what {
                    ServerDo::Back => crate::server::back(),
                    ServerDo::Go => crate::server::go(&hub),
                    ServerDo::Again => crate::server::again(&hub),
                    ServerDo::Close => cx.request_close(),
                    ServerDo::Ask(action) => crate::server::ask(action),
                    ServerDo::LetIn => crate::server::ask_to_let_in(),
                    ServerDo::ShowLog => crate::server::show_log(&hub),
                    ServerDo::LogOf(service) => {
                        crate::server::page(&hub, crate::server::Page::Log);
                        crate::server::read_log(&hub, Some(service), None);
                    }
                    ServerDo::Refresh => crate::server::read_log(&hub, None, None),
                    // (As DiskBackup opens it: the app that runs takes the launch, and comes to the front.)
                    ServerDo::OpenPasswords => {
                        let _ = olo_xdg::spawn_detached("olo-passwords");
                    }
                    ServerDo::Copy => {
                        if let Some(lines) = crate::server::current().and_then(|m| m.log).filter(|lines| !lines.is_empty()) {
                            olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(lines.join("\n")));
                        }
                    }
                }
            }
            WinMsg::ServerKey(i) => crate::server::choose_key(i),
            WinMsg::ServerPage(i) => {
                if let Some(page) = crate::server::Page::ALL.get(i) {
                    crate::server::page(&app().hub.clone(), *page);
                }
            }
            WinMsg::ServerPerson(i) => crate::server::choose_person(i),
            WinMsg::ServerRange(i) => {
                if let Some(range) = olo_rex_client::manage::LogRange::ALL.get(i) {
                    crate::server::read_log(&app().hub.clone(), None, Some(*range));
                }
            }
            WinMsg::ServerChosen => {
                let row = self.server_list.state().single();
                self.server_chosen = row.and_then(|i| self.server_rows.get(i)).map(|r| r.name.clone());
                cx.invalidate();
            }
            WinMsg::HostsForwardFrom(key) => {
                self.fwd_host = Some(key);
                cx.invalidate();
            }
            WinMsg::AskpassConfirm(ok) => {
                let is_secret = matches!(app().askpass.current_request(), Some(AskpassRequest::Secret { .. }));
                if is_secret {
                    app().askpass.answer(AskpassReply::Secret(None));
                } else {
                    app().askpass.answer(AskpassReply::Confirm(ok));
                }
                cx.request_close();
            }
            WinMsg::AskpassSubmit => self.submit_askpass(cx),
            WinMsg::RunApproveChoice(allow) => {
                let approvals = app().run_approvals.clone();
                if let Some(cur) = approvals.current() {
                    if !approvals.respond_from_ui(&cur.id, allow) {
                        // Inside the 0.5 s accidental-click guard for Allow: ignore.
                        return;
                    }
                }
                if approvals.current().is_none() {
                    cx.request_close();
                } else {
                    cx.invalidate();
                }
            }
            WinMsg::RunApproveChanged => {
                if app().run_approvals.current().is_none() {
                    cx.request_close();
                } else {
                    cx.invalidate();
                }
            }
            WinMsg::RenameDone(true) => self.submit_rename(cx),
            WinMsg::RenameDone(false) => {
                app().renaming = None;
                cx.request_close();
            }
            WinMsg::RenameSession(hk, sid) => self.rename_session(&hk, &sid),
            WinMsg::TabMenu(i, at) => {
                let Some((hk, sess)) = self.resolve_active_session() else { return };
                let Some(tab) = sess.windows.get(i).map(|w| w.window_id.clone()) else { return };
                let count = sess.windows.len();
                // Keep It Going and Auto Approve of the tab's panes that run
                // an agent (or have either on), else of its pane with the
                // keyboard (Ben, 2026-11-24: "make sure those options show
                // when right clicking the rex tab too"). In a tab of
                // several panes each pair names its pane: the pair of the
                // pane with the keyboard, unchecked, beside another's
                // countdown read as Auto Approve off (Opus's review).
                let panes = sess.windows.get(i).map(|w| tab_agent_panes(&app().hub, &hk, &sess.session_id, w)).unwrap_or_default();
                let tab_block = sess.windows.get(i).and_then(|w| {
                    w.focused_block_id.as_deref().filter(|id| w.all_blocks().any(|b| b.block_id == *id)).or_else(|| w.all_blocks().next().map(|b| b.block_id.as_str())).map(|bid| BlockKey {
                        host_key: hk.clone(),
                        session_id: sess.session_id.clone(),
                        block_id: bid.to_owned(),
                    })
                });
                let several = sess.windows.get(i).is_some_and(|w| w.all_blocks().count() > 1);
                let active_view = crate::activity_watch::active_view_target();
                let viewing_transcript = tab_block.as_ref().is_some_and(|tb| matches!(&active_view, Some(crate::activity_watch::ViewTarget::PaneTranscript { bk, .. }) if bk == tb));
                let mut menu = Menu::new("").item("Rename Tab…", "window.rename").item(olo_kit_base::tracker::LABEL, "files.open_in_tracker").enabled(tab_block.is_some()).separator();
                if tab_block.is_some() {
                    menu = menu.item("Show Transcript", "pane.show_transcript").checked(viewing_transcript).item("Show Terminal", "pane.show_terminal").checked(!viewing_transcript).separator();
                }
                for (n, p) in panes.iter().enumerate() {
                    let help = app().hub.help(p);
                    let (keep, approve) = if several {
                        let name = pane_name(&app().hub, p);
                        (olo_kit::tr!("Keep It Going in \u{201c}{}\u{201d}", name), olo_kit::tr!("Auto Approve in \u{201c}{}\u{201d}", name))
                    } else {
                        ("Keep It Going".to_owned(), "Auto Approve".to_owned())
                    };
                    menu = menu.item(keep, format!("agent.keep_going_toggle#{n}")).checked(help.keep_going).item(approve, format!("agent.auto_approve_toggle#{n}")).checked(help.auto_approve);
                }
                if !panes.is_empty() {
                    menu = menu.separator();
                }
                let mut menu =
                    menu.item("Move Tab Left", "window.move_left").enabled(i > 0).item("Move Tab Right", "window.move_right").enabled(i + 1 < count).separator().item("Close Tab", "window.close");
                menu::resolve(std::slice::from_mut(&mut menu), true);
                let hub = app().hub.clone();
                let window = self.id;
                menu::context_menu(cx, menu, at, move |cmd, _| {
                    let ids: Vec<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
                    match cmd {
                        "window.rename" => {
                            if let Some(slot) = wins().map.get(&window) {
                                slot.handle.messenger().send(WinMsg::RenameTab(hk.clone(), sess.session_id.clone(), tab.clone()));
                            }
                        }
                        "files.open_in_tracker" => {
                            if let Some(bk) = &tab_block {
                                open_block_in_tracker(&hub, bk);
                            }
                        }
                        "pane.show_transcript" => {
                            if let Some(bk) = &tab_block {
                                let title = pane_name(&hub, bk);
                                if let Some(slot) = wins().map.get(&window) {
                                    slot.handle.messenger().send(WinMsg::OpenPaneTranscript(bk.clone(), title));
                                }
                            }
                        }
                        "pane.show_terminal" => {
                            crate::activity_watch::close_viewer();
                        }
                        _ if cmd.starts_with("agent.keep_going_toggle#") => {
                            if let Some(p) = cmd.rsplit('#').next().and_then(|n| n.parse::<usize>().ok()).and_then(|n| panes.get(n)) {
                                hub.set_help(p, |h| h.keep_going = !h.keep_going);
                            }
                        }
                        _ if cmd.starts_with("agent.auto_approve_toggle#") => {
                            if let Some(p) = cmd.rsplit('#').next().and_then(|n| n.parse::<usize>().ok()).and_then(|n| panes.get(n)) {
                                toggle_auto_approve(&hub, p, &pane_name(&hub, p));
                            }
                        }
                        "window.move_left" | "window.move_right" => {
                            let to = if cmd == "window.move_left" { i.checked_sub(1) } else { Some(i + 1) };
                            if let Some(before) = to.and_then(|to| crate::hub::moved_before(&ids, i, to)) {
                                hub.move_window(&hk, &sess.session_id, &tab, before);
                            }
                        }
                        "window.close" => hub.close_window(&hk, &sess.session_id, &tab),
                        _ => {}
                    }
                });
            }
            WinMsg::RenameTab(hk, sid, tab) => {
                let session = {
                    let hub = app().hub.clone();
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.hosts.get(&hk).and_then(|h| h.sessions.get(&sid)).cloned()
                };
                if let Some(session) = session {
                    self.rename_tab(&hk, &session, &tab);
                }
            }
            WinMsg::RenameAgain => {
                let (was, hint) = app().renaming.as_ref().map(|r| (r.was.clone(), r.hint())).unwrap_or_default();
                if let Some(f) = &self.askpass_field {
                    f.update(cx, |f, cx| {
                        f.set_text(was, cx);
                        f.set_hint(hint, cx);
                        f.select_all_text(cx);
                    });
                    f.focus(cx);
                }
                cx.invalidate();
            }
            WinMsg::SidebarMoved => {
                let (w, col) = (self.side_pane.width(), self.side_pane.collapsed());
                let hub = app().hub.clone();
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.sidebar_width = w;
                st.settings.sidebar_collapsed = col;
                st.settings.save();
                cx.invalidate();
            }
            WinMsg::CloseWindow => cx.request_close(),
        }
    }

    fn command(&mut self, command: &str, cx: &mut olo_kit::Cx<Self>) -> bool {
        if olo_kit::app_menu::run(command, cx) {
            return true;
        }
        match command {
            "close" => cx.request_close(),
            "quit" => olo_kit::quit(),
            _ => self.execute_action(command, None, cx),
        }
        true
    }

    fn window_activated(&mut self, active: bool, cx: &mut olo_kit::Cx<Self>) {
        self.window_active = active;
        if !active {
            app().attention.hide(self.id);
        }
        cx.invalidate();
    }

    fn takes_drops(&self) -> bool {
        self.role == WinRole::Workspace
    }

    fn drop_over(&mut self, offer: Option<&DropOffer>, _: Point<Pixels>, cx: &mut olo_kit::Cx<Self>) -> Option<DropAccept> {
        let accepted = offer.filter(|o| o.copy).and_then(|o| {
            let mime = if o.types.iter().any(|t| t == "text/uri-list") {
                Some("text/uri-list")
            } else {
                ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"].into_iter().find(|t| o.types.iter().any(|x| x == t))
            }?;
            Some(DropAccept { target: "rex-pane".into(), mime: mime.into(), action: DragAction::Copy })
        });
        if self.dropping != accepted.is_some() {
            self.dropping = accepted.is_some();
            cx.invalidate();
        }
        accepted
    }

    fn dropped(&mut self, d: Dropped, cx: &mut olo_kit::Cx<Self>) {
        self.dropping = false;
        if let Some(bk) = self.focused_block_key() {
            if d.mime == "text/uri-list" {
                app().hub.upload_files(&bk, d.paths(), Feedback::start(self.id));
            } else {
                let text = String::from_utf8_lossy(&d.data).into_owned();
                if !text.is_empty() && paste_is_safe(&text) {
                    app().hub.send_input(&bk, text.into_bytes());
                }
            }
        }
        cx.invalidate();
    }

    fn save_state(&mut self, _: &mut olo_kit::Cx<Self>) -> Option<String> {
        if self.role != WinRole::Workspace {
            return None;
        }
        let saved = SavedWorkspaceState {
            active_host: self.active_host.clone(),
            active_session: self.active_session.clone(),
            session_index: self.session_index,
            sidebar_collapsed: self.side_pane.collapsed(),
            sidebar_width: self.side_pane.width(),
        };
        serde_json::to_string(&saved).ok()
    }

    fn body(&mut self, cx: &mut olo_kit::Cx<Self>) -> Element {
        if self.role == WinRole::Workspace { self.render_workspace(cx) } else { self.render_secondary_panel(cx) }
    }
}

impl Drop for RexWindow {
    fn drop(&mut self) {
        if self.role == WinRole::AddHost {
            crate::setup::back();
        }
        // (A move that hasn't stopped the old host ends with its window;
        // one copying or switching goes on, and opens it again to show.)
        if self.role == WinRole::Move && !matches!(crate::moving::current().map(|m| m.stage), Some(crate::moving::Stage::Copying { .. } | crate::moving::Stage::Switching { .. })) {
            crate::moving::cancel(&app().hub.clone());
        }
        if self.role == WinRole::Server {
            crate::server::close(&app().hub.clone());
        }
        if self.role == WinRole::Host {
            crate::host::close();
        }
        if self.role == WinRole::Account {
            crate::account::close();
        }
        if self.role == WinRole::Carry {
            crate::carry::close();
        }
        if self.role == WinRole::RunApprove {
            // Closing the alert window denies the pending command.
            app().run_approvals.clone().deny_front();
        }
        self.cancel_preview();
        app().attention.hide(self.id);
        wins().map.remove(&self.id);
        if self.role == WinRole::Workspace && !wins().map.values().any(|s| s.role == WinRole::Workspace) {
            crate::activity_watch::stop_all();
        }
    }
}

impl TextInput for RexWindow {
    fn insert_text(&mut self, text: &str, cx: &mut olo_kit::Cx<Self>) {
        self.preedit = None;
        if self.role != WinRole::Workspace || self.overlay.is_some() || text.is_empty() || crate::activity_watch::active_view_target().is_some() {
            return;
        }
        if let Some(bk) = self.focused_block_key() {
            // If Copy Mode is active, route single characters through Copy Mode.
            let hub = app().hub.clone();
            let handled_cm = {
                if let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                    let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(mut cm) = g.copy_mode.take() {
                        let res = cm.handle_key(&mut g.terminal, text, Some(text), false, false);
                        match res {
                            CopyModeAction::Continue => {
                                g.copy_mode = Some(cm);
                                Some(None)
                            }
                            CopyModeAction::Exit => {
                                let _ = g.terminal.clear_selection();
                                Some(None)
                            }
                            CopyModeAction::Yank(y) => {
                                let _ = g.terminal.clear_selection();
                                Some(Some(y))
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some(maybe_yank) = handled_cm {
                if let Some(y) = maybe_yank {
                    olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(y));
                }
                cx.invalidate();
                return;
            }
            let targets = self.input_target_blocks(&bk);
            for t in targets {
                hub.send_input(&t, text.as_bytes().to_vec());
            }
        }
    }

    fn set_marked_text(&mut self, text: &str, cx: &mut olo_kit::Cx<Self>) {
        self.preedit = Some(text.to_owned());
        cx.invalidate();
    }

    fn unmark_text(&mut self, cx: &mut olo_kit::Cx<Self>) {
        self.preedit = None;
        cx.invalidate();
    }

    fn delete_marked_text(&mut self, cx: &mut olo_kit::Cx<Self>) {
        self.preedit = None;
        cx.invalidate();
    }
}

/// Isolated child view rendering the active window's tiled split panes + floating layers.
/// Output arriving on `rex.data.terminal.v1` invalidates ONLY this child view at vsync!
pub struct PanesView {
    win_id: WinId,
    session: Option<(String, String)>,
    /// The width it is given: the window's less the sidebar's beside it
    /// (none yet: the window's). A split shares it among its panes, and a
    /// pane's head decides its buttons by its share. (The window's whole
    /// width was shared, the sidebar's in it, and a head kept buttons it
    /// had no room for: Opus's look at Activity, 2026-11-24.)
    width: Option<f32>,
}

impl View for PanesView {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut olo_kit::Cx<Self>) {
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut olo_kit::Cx<Self>) -> Element {
        let hub = app().hub.clone();
        let scale = cx.scale_factor();
        let Some((hk, sid)) = &self.session else { return div() };
        let (settings, active_theme, host_key, host_label, is_local, sess) = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get(hk) else { return div() };
            let Some(sess) = h.sessions.get(sid) else { return div() };
            (st.settings.clone(), st.active_theme.clone(), hk.clone(), h.label.clone(), h.is_local, sess.clone())
        };
        let Some(win_view) = sess.active_window().or_else(|| sess.windows.first()).cloned() else { return div() };

        let metrics = Metrics::from_settings(scale, &settings);
        let pad = crate::settings::PADDING;
        let win_id = self.win_id;

        // Render tiled layer (or zoomed block) + any normal/minimized floating layers!
        let tiled_layer = win_view.layers.iter().find(|l| l.layer_kind() == LayerKind::Tiled).cloned();
        let floating_layers: Vec<_> = win_view.layers.iter().filter(|l| l.layer_kind() == LayerKind::Floating).cloned().collect();

        let mut layer_stack = div().relative().size_full();

        let vp_w = self.width.unwrap_or_else(|| f32::from(cx.viewport_size().width)).max(200.0);
        if let Some(zoom_id) = &win_view.zoomed_block_id {
            let bk = BlockKey { host_key: host_key.clone(), session_id: sess.session_id.clone(), block_id: zoom_id.clone() };
            layer_stack = layer_stack.child(render_single_pane(bk, &host_label, is_local, true, true, true, vp_w, &settings, &active_theme, &metrics, pad, win_id, cx));
        } else if let Some(tiled) = tiled_layer {
            if let Some(root_node) = &tiled.layout {
                layer_stack =
                    layer_stack.child(render_layout_node(root_node, vp_w, &host_key, &host_label, is_local, &sess.session_id, &win_view, &settings, &active_theme, &metrics, pad, win_id, cx));
            } else if let Some(b) = tiled.blocks.first() {
                let bk = BlockKey { host_key: host_key.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                layer_stack = layer_stack.child(render_single_pane(
                    bk,
                    &host_label,
                    is_local,
                    win_view.focused_block_id.as_deref() == Some(b.block_id.as_str()),
                    false,
                    false,
                    vp_w,
                    &settings,
                    &active_theme,
                    &metrics,
                    pad,
                    win_id,
                    cx,
                ));
            }
        }

        // Render floating layers and minimized capsules over the tiled area!
        let mut minimized_bar = div().absolute().right(px(m::SPACING)).bottom(px(m::SPACING)).flex().flex_row().gap(px(m::TIGHT));
        let mut has_minimized = false;

        for fl in floating_layers {
            if fl.layer_state() == LayerState::Minimized {
                has_minimized = true;
                let lid = fl.layer_id.clone();
                let hk = host_key.clone();
                let sid = sess.session_id.clone();
                let title = fl.blocks.first().map(|b| if b.label.is_empty() { "Floating".to_owned() } else { b.label.clone() }).unwrap_or_else(|| "Floating".to_owned());
                minimized_bar = minimized_bar.child(Button::new(title).small().on_click(move |_, _| {
                    app().hub.set_layer_minimized(&hk, &sid, &lid, false);
                }));
            } else if let Some(b) = fl.blocks.first() {
                let [bx, by, bw, bh] = fl.bounds;
                let vp = cx.viewport_size();
                let left = (f32::from(vp.width) * bx as f32).max(16.0);
                let top = (f32::from(vp.height) * by as f32).max(16.0);
                let width = (f32::from(vp.width) * bw as f32).max(260.0);
                let height = (f32::from(vp.height) * bh as f32).max(180.0);
                let bk = BlockKey { host_key: host_key.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                let lid_min = fl.layer_id.clone();
                let lid_raise = fl.layer_id.clone();
                let hk_m = host_key.clone();
                let sid_m = sess.session_id.clone();
                let hk_r = host_key.clone();
                let sid_r = sess.session_id.clone();
                let fl_title = if b.label.is_empty() { "Floating Terminal".to_owned() } else { b.label.clone() };
                let drag_move_start = Rc::new(std::cell::Cell::new(None::<(Point<Pixels>, [f64; 4])>));
                let dm1 = drag_move_start.clone();
                let dm2 = drag_move_start.clone();
                let lid_mv = fl.layer_id.clone();
                let hk_mv = host_key.clone();
                let sid_mv = sess.session_id.clone();
                let vp_w = f32::from(vp.width).max(320.0) as f64;
                let vp_h = f32::from(vp.height).max(200.0) as f64;
                let init_bounds = fl.bounds;

                let drag_res_start = Rc::new(std::cell::Cell::new(None::<(Point<Pixels>, [f64; 4])>));
                let dr1 = drag_res_start.clone();
                let dr2 = drag_res_start.clone();
                let lid_rs = fl.layer_id.clone();
                let hk_rs = host_key.clone();
                let sid_rs = sess.session_id.clone();

                layer_stack = layer_stack.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(width))
                        .h(px(height))
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, move |_, _| {
                            app().hub.raise_layer(&hk_r, &sid_r, &lid_raise);
                        })
                        .child(overlays::overlay_card_back("Floating Terminal"))
                        .child(
                            div()
                                .h(px(24.0))
                                .px(px(m::SPACING))
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .cursor(CursorStyle::OpenHand)
                                .on_mouse_down(MouseButton::Left, move |e, cx| {
                                    dm1.set(Some((e.position, init_bounds)));
                                    cx.stop_propagation();
                                })
                                .on_drag_move(move |e: &MouseMoveEvent, _| {
                                    if e.pressed_button != Some(MouseButton::Left) {
                                        return;
                                    }
                                    let Some((origin, orig)) = dm2.get() else { return };
                                    let dx = (f32::from(e.position.x) - f32::from(origin.x)) as f64 / vp_w;
                                    let dy = (f32::from(e.position.y) - f32::from(origin.y)) as f64 / vp_h;
                                    let nx = (orig[0] + dx).clamp(0.02, (0.98 - orig[2]).max(0.02));
                                    let ny = (orig[1] + dy).clamp(0.02, (0.98 - orig[3]).max(0.02));
                                    app().hub.resize_layer(&hk_mv, &sid_mv, &lid_mv, [nx, ny, orig[2], orig[3]]);
                                })
                                .child(StringView::new(fl_title).bold(true).cut(true))
                                .child(Button::new("—").small().on_click(move |_, _| {
                                    app().hub.set_layer_minimized(&hk_m, &sid_m, &lid_min, true);
                                })),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .p(px(2.0))
                                .child(render_single_pane(
                                    bk,
                                    &host_label,
                                    is_local,
                                    win_view.focused_block_id.as_deref() == Some(b.block_id.as_str()),
                                    false,
                                    false,
                                    width,
                                    &settings,
                                    &active_theme,
                                    &metrics,
                                    pad,
                                    win_id,
                                    cx,
                                ))
                                .child(
                                    // Bottom-right corner resize handle for floating layers.
                                    canvas(|g, b| {
                                        let p = theme::palette();
                                        let c = style::shade(&p, p.panel_background, 1.4);
                                        style::fill_rect(g, Bounds::new(point(b.right() - px(6.0), b.bottom() - px(2.0)), size(px(5.0), px(1.5))), c);
                                        style::fill_rect(g, Bounds::new(point(b.right() - px(2.0), b.bottom() - px(6.0)), size(px(1.5), px(5.0))), c);
                                    })
                                    .paint_key(71)
                                    .absolute()
                                    .right(px(1.0))
                                    .bottom(px(1.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                                    .on_mouse_down(MouseButton::Left, move |e, cx| {
                                        dr1.set(Some((e.position, init_bounds)));
                                        cx.stop_propagation();
                                    })
                                    .on_drag_move(move |e: &MouseMoveEvent, _| {
                                        if e.pressed_button != Some(MouseButton::Left) {
                                            return;
                                        }
                                        let Some((origin, orig)) = dr2.get() else { return };
                                        let dx = (f32::from(e.position.x) - f32::from(origin.x)) as f64 / vp_w;
                                        let dy = (f32::from(e.position.y) - f32::from(origin.y)) as f64 / vp_h;
                                        let nw = (orig[2] + dx).clamp(0.24, (0.98 - orig[0]).max(0.24));
                                        let nh = (orig[3] + dy).clamp(0.22, (0.98 - orig[1]).max(0.22));
                                        app().hub.resize_layer(&hk_rs, &sid_rs, &lid_rs, [orig[0], orig[1], nw, nh]);
                                    }),
                                ),
                        ),
                );
            }
        }

        if has_minimized {
            layer_stack = layer_stack.child(minimized_bar);
        }

        layer_stack
    }
}

#[allow(clippy::too_many_arguments)]
fn render_layout_node(
    node: &LayoutNode,
    avail_w: f32,
    host_key: &str,
    host_label: &str,
    is_local: bool,
    session_id: &str,
    win_view: &WindowView,
    settings: &Settings,
    active_theme: &TerminalTheme,
    metrics: &Metrics,
    pad: (f32, f32),
    win_id: WinId,
    cx: &mut olo_kit::Cx<PanesView>,
) -> Element {
    if let Some(split) = &node.split {
        // Adaptive tiling: on tall narrow tiles (e.g. vertical third, < 540 pt wide),
        // automatically stack horizontal splits vertically at full tile width so no pane
        // squeezes into cramped ~200 pt columns!
        let eff_horiz = split.direction.is_horizontal() && avail_w >= 540.0;
        let ratio = (split.ratio as f32).clamp(0.1, 0.9);
        let (before_w, after_w) = if eff_horiz { ((avail_w - 1.0).max(80.0) * ratio, (avail_w - 1.0).max(80.0) * (1.0 - ratio)) } else { (avail_w, avail_w) };
        let before_el = render_layout_node(&split.before, before_w, host_key, host_label, is_local, session_id, win_view, settings, active_theme, metrics, pad, win_id, cx);
        let after_el = render_layout_node(&split.after, after_w, host_key, host_label, is_local, session_id, win_view, settings, active_theme, metrics, pad, win_id, cx);
        let split_id = split.split_id.clone();
        let split_id_drag = split.split_id.clone();
        let hk_d = host_key.to_owned();
        let sid_d = session_id.to_owned();
        let hk_dr = host_key.to_owned();
        let sid_dr = session_id.to_owned();
        let line = 1.0 / cx.scale_factor();

        // The line between the two, a device line of the panes' edge
        // colour (as under their heads, and as Terminal's panes have
        // between them).
        let divider = canvas(move |g, b| style::fill_rect(g, style::snap(g, b), pane_edge(&theme::palette()))).paint_key(if eff_horiz { 3 } else { 4 }).flex_shrink_0();

        // What drags it: over the two panes, a grip a few points either
        // side of the line (two points of line were all there was to
        // catch). The drag is kept with the element, by the split's id:
        // every change of the ratio lays the panes out again, and a
        // drag kept in what one layout made ended at its first move.
        // Agents read it as a Splitter, its value the first pane's share.
        let name = olo_kit::words::tr(if eff_horiz { "Pane Width" } else { "Pane Height" });
        let grip = canvas(move |g, b| {
            let grip = split_grip(b, ratio, eff_horiz, 1.0 / g.scale_factor());
            g.set_cursor_style(if eff_horiz { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown }, grip);
            g.agent(agent::Node {
                role: "Splitter",
                name: Some(name.into()),
                value: Some(format!("{:.0}%", ratio * 100.0).into()),
                bounds: grip,
                enabled: true,
                clickable: true,
                ..Default::default()
            });
        })
        .id(olo_kit::SharedString::from(format!("rex-split-{}", split.split_id)))
        .paint_key(style::key((eff_horiz, (ratio * 10_000.0) as i32)))
        .absolute()
        .inset_0()
        .on_mouse_down(MouseButton::Left, move |e, cx| {
            if !split_grip(cx.listener_bounds(), ratio, eff_horiz, line).contains(&e.position) {
                return;
            }
            cx.stop_propagation();
            if e.click_count >= 2 {
                cx.node_state::<SplitDrag>().0 = None;
                app().hub.set_split_ratio(&hk_d, &sid_d, &split_id, 0.5);
            } else {
                cx.node_state::<SplitDrag>().0 = Some((e.position, ratio));
            }
        })
        .on_drag_move(move |e: &MouseMoveEvent, cx| {
            let b = cx.listener_bounds();
            let drag = cx.node_state::<SplitDrag>();
            let Some((from, was)) = drag.0 else { return };
            if e.pressed_button != Some(MouseButton::Left) {
                drag.0 = None;
                return;
            }
            let (span, moved) = if eff_horiz { (b.size.width, e.position.x - from.x) } else { (b.size.height, e.position.y - from.y) };
            if let Some(next) = split_dragged(was, f32::from(moved), f32::from(span) - line) {
                app().hub.set_split_ratio(&hk_dr, &sid_dr, &split_id_drag, f64::from(next));
            }
        })
        .on_window_mouse_up(|e, cx| {
            if e.button == MouseButton::Left {
                cx.node_state::<SplitDrag>().0 = None;
            }
        });

        // Each pane its share of what the split has, whatever that is.
        // (They had a thousand points between them by the ratio and
        // *equal* shares of the rest: in a window wider than that, a
        // quarter's pane had a third, and the divider moved slower than
        // the pointer dragging it.)
        let share = |el: Element, share: f32| {
            let mut el = el;
            el.style().flex_grow = Some(share);
            el.style().flex_shrink = Some(1.0);
            el.style().flex_basis = Some(olo_kit::relative(0.0).into());
            el
        };
        let mut container = div().relative().size_full().flex();
        if eff_horiz {
            container = container
                .flex_row()
                .child(share(div().min_w_0().h_full(), ratio).child(before_el))
                .child(divider.w(px(line)).h_full())
                .child(share(div().min_w_0().h_full(), 1.0 - ratio).child(after_el));
        } else {
            container = container
                .flex_col()
                .child(share(div().min_h_0().w_full(), ratio).child(before_el))
                .child(divider.h(px(line)).w_full())
                .child(share(div().min_h_0().w_full(), 1.0 - ratio).child(after_el));
        }
        container = container.child(grip);
        return container;
    }

    if let Some(bid) = &node.block_id {
        let bk = BlockKey { host_key: host_key.to_owned(), session_id: session_id.to_owned(), block_id: bid.clone() };
        let focused = win_view.focused_block_id.as_deref() == Some(bid.as_str());
        let has_splits = win_view.all_blocks().count() > 1;
        return render_single_pane(bk, host_label, is_local, focused, false, has_splits, avail_w, settings, active_theme, metrics, pad, win_id, cx);
    }

    div().size_full()
}

/// The link under `at` in a pane (the pointer, where the key that
/// opens a link is held; `None`: no link is looked for), kept for the
/// pane to underline ([`PaneVisualState::hover`]). Whether it changed.
/// The rows around the pointer are read from the terminal only when
/// the pointer is over another cell than last time.
fn link_hover(br: &Arc<Mutex<crate::hub::BlockRuntime>>, vis: &PaneVisualState, metrics: &Metrics, pad: (f32, f32), at: Option<Point<Pixels>>) -> bool {
    /// How many rows above and below the pointer a link may reach.
    const AROUND: u32 = 24;
    let found = at.and_then(|at| {
        let (col, row) = metrics.cell_at(vis.bounds.get(), pad, at, vis.shift.get());
        // (Still on the link it was on: nothing to look for.)
        if let Some(hover) = vis.hover.borrow().as_ref()
            && hover.cells.contains(&(u32::from(row), col))
        {
            return Some(hover.clone());
        }
        let g = br.lock().unwrap_or_else(|e| e.into_inner());
        let (_, count) = g.terminal.size();
        let row = u32::from(row);
        let top = row.saturating_sub(AROUND);
        let bottom = (row + AROUND).min(u32::from(count).saturating_sub(1));
        let rows: Vec<(Vec<char>, bool)> = (top..=bottom).map(|r| g.terminal.viewport_row_cells(r)).collect();
        let osc8 = |r: usize, c: usize| g.terminal.hyperlink_at_viewport(c as u16, top + r as u32);
        links::hover(&rows, (row - top) as usize, usize::from(col), &osc8).map(|mut hover| {
            hover.cells.iter_mut().for_each(|cell| cell.0 += top);
            hover
        })
    });
    let mut kept = vis.hover.borrow_mut();
    let changed = *kept != found;
    *kept = found;
    changed
}

/// Where a link goes, said in a tip under it where its text doesn't
/// say (an address a program named for some words, OSC 8: any program
/// on a far host can name any address for any words): as the pointer
/// rests on it with the key held, and gone with the underline.
fn link_tip(cx: &mut olo_kit::WindowCx, vis: &PaneVisualState, metrics: &Metrics, pad: (f32, f32)) {
    const KEY: &str = "rex-link-goes";
    let hover = vis.hover.borrow();
    let Some((hover, address)) = hover.as_ref().filter(|h| h.named).and_then(|h| match &h.target {
        LinkTarget::Url(address) => Some((h, address.clone())),
        LinkTarget::FilePath { .. } => None,
    }) else {
        olo_kit::tip::hover(cx, KEY, "", Bounds::default(), false);
        return;
    };
    // (Under its first row's cells.)
    let (row, from) = hover.cells[0];
    let to = hover.cells.iter().take_while(|cell| cell.0 == row).last().map_or(from, |cell| cell.1);
    let rect = vis.bounds.get();
    let (x, y) = (f32::from(rect.left()) + pad.0 + f32::from(from) * metrics.cell.0, f32::from(rect.top()) + pad.1 + vis.shift.get() + row as f32 * metrics.cell.1);
    let anchor = Bounds::new(point(px(x), px(y)), size(px(f32::from(to - from + 1) * metrics.cell.0), px(metrics.cell.1)));
    olo_kit::tip::hover(cx, KEY, address, anchor, true);
}

#[allow(clippy::too_many_arguments)]
fn render_single_pane(
    bk: BlockKey,
    host_label: &str,
    is_local: bool,
    focused: bool,
    zoomed: bool,
    has_splits: bool,
    pane_w: f32,
    settings: &Settings,
    active_theme: &TerminalTheme,
    metrics: &Metrics,
    pad: (f32, f32),
    win_id: WinId,
    cx: &mut olo_kit::Cx<PanesView>,
) -> Element {
    let hub = app().hub.clone();
    let br = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.blocks.get(&bk).cloned()
    };
    let Some(br) = br else {
        return div().size_full().child(StringView::new("Attaching terminal stream…").dim(true));
    };

    let (creator_name, title, pwd, activity, ports_list, password_prompt, scrolled, counting) = {
        let g = br.lock().unwrap_or_else(|e| e.into_inner());
        (
            g.creator_name.clone(),
            g.said(if is_local { crate::hub::local_home() } else { None }.as_deref()),
            g.pwd.clone(),
            g.agent_activity,
            g.ports.ports.iter().map(|p| p.port).collect::<Vec<u16>>(),
            g.password_prompt,
            g.terminal.scrollbar(),
            // (Auto Approve's question, and how long till it's answered:
            // what the countdown says.)
            g.watch.counting(Instant::now()).map(|(_, left)| left.as_secs_f32().ceil() as u64),
        )
    };

    // Native onboarding walkthrough card for `com.superlogical.onboarding` blocks!
    if creator_name == "com.superlogical.onboarding" {
        let hk_ob = bk.host_key.clone();
        let sid_ob = bk.session_id.clone();
        let bid_ob = bk.block_id.clone();
        return div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(m::WINDOW_INSET * 2.0))
            .gap(px(m::SECTION))
            .child(StringView::new("Welcome to Olo Rex").bold(true))
            .child(StringView::new("Multiplex local & remote SSH/Tailscale workspaces with Ghostty VT, splits, floating layers, and live AI agent monitoring.").dim(true))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(Button::new("Start Terminal Here").default(true).on_click(move |_, _| {
                        app().hub.split_pane(&hk_ob, &sid_ob, &bid_ob, "right", None);
                    }))
                    .child(Button::new("Connect Remote Host…").on_click(|_, _| {
                        open_or_raise_panel(WinRole::Hosts);
                    })),
            );
    }
    let git_info = hub.git_info_for(&bk, &pwd);

    let show_header = (has_splits && settings.pane_header_behavior == BarBehavior::Always) || zoomed;
    let header_h = crate::settings::PANE_HEADER;
    let header_el: Option<Element> = if show_header {
        // (What the pane is called: `hub::pane_title`.)
        let header_title = title.clone();
        let git_pill = git_info.and_then(|gi| {
            let b = gi.branch?;
            Some(if gi.dirty_count > 0 { format!("{b} *{}", gi.dirty_count) } else { b })
        });
        let ports_pill = match ports_list.as_slice() {
            [] => None,
            [single] => Some(format!(":{single}")),
            many => Some(format!("{} ports", many.len())),
        };
        let host_pill = (!is_local).then(|| host_label.to_owned());
        let zoom_pill = zoomed.then(|| "Zoomed".to_owned());

        // The same buttons on every pane that has the room, the keyboard
        // in it or not: quiet marks, a button's face under the pointer.
        // (Split was on the focused pane only: its head's contents moved
        // with the keyboard, and were, by accident, what said where the
        // keyboard was. Opus's review, 2026-11-22.)
        let show_split_btns = pane_w >= 230.0;
        let show_close_btn = pane_w >= 140.0;
        let show_extras = pane_w >= 260.0;

        let bk_click = bk.clone();
        let bk_split_r = bk.clone();
        let bk_split_d = bk.clone();
        let bk_zoom = bk.clone();
        let bk_close = bk.clone();
        let title_paint = header_title.clone();
        let pane_help = crate::hub::help_of(&bk);
        let key = style::key((focused, pane_w as i32));

        let head_bg = canvas(move |g, b| {
            let p = theme::palette();
            // Every pane's head on the panel's ground, the chosen tab's
            // face, which opens onto the heads under it: the keyboard in
            // a pane on the right left the tab open onto a darker band
            // when the head it was in alone had that ground. Where the
            // keyboard is, the title says (a weight heavier, the others
            // quieter) and the line under it.
            g.paint_quad(olo_kit::fill(b, style::c(p.panel_background)));
            style::hline_above(g, b.left(), b.right(), b.bottom(), pane_edge(&p));
            if focused {
                // And a line along its foot in the look's colour for
                // where the keyboard is, two points in whole device
                // pixels. (It was a bar of an ochre no look has, two and
                // a half points; with no line at all, the grounds alone
                // didn't say which pane it was, least of all in dark:
                // Opus's review, 2026-11-22.)
                let line = style::snap(g, Bounds::from_corners(point(b.left(), b.bottom() - px(2.0)), b.bottom_right()));
                style::fill_rect(g, line, style::c(p.keyboard_navigation));
            }
        })
        .paint_key(key)
        .absolute()
        .inset_0();

        let icon_el = {
            let pane_help = pane_help.clone();
            let ikey = style::key(("pane-icon", activity as u8, &pane_help));
            canvas(move |g, b| {
                // Same `m::LABEL_ROW` box as title, pills, and LinkButton so
                // every element in the pane header shares one snapped baseline
                // at 1×, 1.25×, 1.5×, 1.75×, and 2×.
                let text_middle = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
                let icon_b = style::snap_centred(g, point(b.left() + px(ROW_ICON / 2.0), text_middle), ROW_ICON, ROW_ICON);
                paint_activity_icon(g, icon_b, activity, &pane_help);
            })
            .paint_key(ikey)
            .w(px(ROW_ICON))
            .h(px(m::LABEL_ROW))
            .flex_shrink_0()
        };

        let title_el = {
            let p = theme::palette();
            let title_ink = if focused { style::c(p.panel_text) } else { style::secondary_text(&p) };
            let title_w = if focused { FontWeight::MEDIUM } else { FontWeight::NORMAL };
            let title_paint = title_paint.clone();
            let tkey = style::key(("pane-title", &title_paint, focused, style::ink_key(Some(title_ink))));
            canvas(move |g, b| {
                let r = style::label_fit(g, b.left(), b.center().y, style::Align::Left, title_paint.clone(), style::PLAIN_SIZE, style::ui_font(title_w), title_ink, b.size.width);
                g.agent(agent::Node::label(title_paint.clone(), r));
            })
            .paint_key(tkey)
            .w_full()
            .min_w_0()
            .h(px(m::LABEL_ROW))
        };

        let mut bar = div()
            .relative()
            .w_full()
            .h(px(header_h))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT))
            .pl(px(pad.0))
            .pr(px(HEADER_END))
            .on_mouse_down(MouseButton::Left, move |_, _| {
                let hub = app().hub.clone();
                hub.focus_block(&bk_click.host_key, &bk_click.session_id, &bk_click.block_id);
            })
            .child(head_bg)
            .child(icon_el)
            .child(div().flex_1().min_w_0().child(title_el));

        if show_extras {
            if let Some(hp) = host_pill {
                bar = bar.child(StringView::new(hp).bold(true).height(m::LABEL_ROW));
            }
            if let Some(gp) = git_pill {
                bar = bar.child(StringView::new(gp).dim(true).height(m::LABEL_ROW));
            }
            if let Some(pp) = ports_pill {
                // (The ports it listens on, and Zoomed: the kit's link
                // buttons, small actions beside what they act on. They
                // were bevelled wells drawn here, which read as fields
                // to type in.)
                // (Quieter in a pane without the keyboard, as its title and
                // marks are: the link's blue was the loudest thing there.)
                bar = bar.child(
                    div()
                        .opacity(if focused { 1.0 } else { 0.7 })
                        .child(widgets::LinkButton::new(pp).beside(style::PLAIN_SIZE).id(format!("rex-ports-{}", bk.block_id)).on_click(move |_, _| open_or_raise_panel(WinRole::Hosts))),
                );
            }
            if let Some(zp) = zoom_pill {
                let z = bk_zoom.clone();
                bar = bar.child(widgets::LinkButton::new(zp).beside(style::PLAIN_SIZE).id(format!("rex-zoomed-{}", bk.block_id)).on_click(move |_, _| {
                    app().hub.toggle_zoom(&z.host_key, &z.session_id, Some(z.block_id.clone()));
                }));
            }
        }
        if show_split_btns {
            bar = bar
                .child(GlyphButton::new("Split Right", Glyph::SplitRight).id(format!("rex-split-right-{}", bk.block_id)).quiet(!focused).on_click(move |_, _| {
                    app().hub.split_pane(&bk_split_r.host_key, &bk_split_r.session_id, &bk_split_r.block_id, "right", None);
                }))
                .child(GlyphButton::new("Split Down", Glyph::SplitDown).id(format!("rex-split-down-{}", bk.block_id)).quiet(!focused).on_click(move |_, _| {
                    app().hub.split_pane(&bk_split_d.host_key, &bk_split_d.session_id, &bk_split_d.block_id, "down", None);
                }));
        }
        if show_close_btn {
            bar = bar.child(GlyphButton::new("Close Pane", Glyph::Close).id(format!("rex-close-pane-{}", bk.block_id)).quiet(!focused).on_click(move |_, _| {
                app().hub.close_block(&bk_close.host_key, &bk_close.session_id, &bk_close.block_id);
            }));
        }
        Some(bar)
    } else {
        None
    };

    let visual = cx.window_state::<PaneVisuals>().0.entry(bk.clone()).or_default().clone();

    let br_paint = br.clone();
    let said_paint = title.clone();
    let bk_paint = bk.clone();
    let vis_paint = visual.clone();
    let theme_paint = active_theme.clone();
    let metrics_paint = metrics.clone();

    let br_mouse = br.clone();
    let bk_mouse = bk.clone();
    let vis_mouse = visual.clone();
    let metrics_mouse = metrics.clone();

    let br_scroll = br.clone();
    let vis_scroll = visual.clone();
    let ch_scroll = metrics.cell.1;

    let bk_rclick = bk.clone();
    let br_menu = br.clone();
    let metrics_drag = metrics.clone();
    let br_drag = br.clone();
    let vis_drag = visual.clone();
    // (The mouse given to a program that asked for it, as in a local
    // terminal: `BlockRuntime::mouse_report`, Ghostty's encoder; Shift
    // keeps it for selecting. Ben, 2026-11-24: "in olo rex we need to get
    // mouse scrolling / mouse interactions working properly like a local
    // terminal".)
    let bk_drag = bk.clone();
    let (br_right, vis_right, metrics_right) = (br.clone(), visual.clone(), metrics.clone());
    let (br_middle, vis_middle, metrics_middle, bk_middle) = (br.clone(), visual.clone(), metrics.clone(), bk.clone());
    let (bk_scroll, metrics_scroll) = (bk.clone(), metrics.clone());
    // (Every move of the pointer told to the window only while the
    // program asked for every move, mode 1003: else none, as before.)
    let wants_motion = br.lock().unwrap_or_else(|e| e.into_inner()).terminal.dec_mode(1003);

    let pane_m1 = cx.messenger();
    let pane_m2 = cx.messenger();
    let pane_m3 = cx.messenger();
    let (pane_m4, pane_m5, pane_m6) = (cx.messenger(), cx.messenger(), cx.messenger());
    let (br_hover, vis_hover, metrics_hover) = (br.clone(), visual.clone(), metrics.clone());
    let (br_keys, vis_keys, metrics_keys) = (br.clone(), visual.clone(), metrics.clone());
    let (vis_left, metrics_left) = (visual.clone(), metrics.clone());

    let hub_paint = hub.clone();
    let screen_canvas = canvas(move |g, rect| {
        // Auto-resize terminal grid cleanly when layout bounds change!
        let (want_cols, want_rows) = metrics_paint.grid_size(f32::from(rect.size.width), f32::from(rect.size.height), pad);
        let (cw_px, ch_px) = metrics_paint.device_cell();
        let resize = {
            let g = br_paint.lock().unwrap_or_else(|e| e.into_inner());
            !g.other_size_owner && g.geometry != Some((want_cols, want_rows, cw_px, ch_px))
        };
        if resize {
            app().hub.resize_block(&bk_paint, want_cols, want_rows, cw_px, ch_px);
        }

        let mut guard = br_paint.lock().unwrap_or_else(|e| e.into_inner());
        let search_matches = if guard.finding { Vec::new() } else { guard.search.viewport_matches(&guard.terminal) };
        let copy_mode = guard.copy_mode.clone();

        let br_ref = &mut *guard;
        // In the middle of a program's update of its screen (its
        // synchronized output), what the terminal holds is a piece of a
        // frame: the pane goes on showing the last whole one. (A
        // program that draws its whole screen anew at each key, as
        // Claude Code and Codex do, was seen half drawn whenever its
        // update came in two pieces, which over a slow link is often.)
        let now = std::time::Instant::now();
        match br_ref.held(now) {
            Some(left) => g.paint_again_within(left.as_secs_f64()),
            None => {
                let missing = br_ref.frame.capture(&mut br_ref.render_state, &br_ref.terminal).unwrap_or_default();
                if !missing.is_empty() {
                    hub_paint.prepare_images(br_paint.clone(), bk_paint.clone(), br_ref.frame.owner, missing);
                }
            }
        }
        // What was typed here and is drawn ahead of the host's echo
        // (over a slow link): shown this frame, it says what to draw
        // over it, and when it next waits on time for something.
        let hover = vis_paint.hover.borrow().clone();
        let (predicted, wake) = br_ref.predicted(now);
        if let Some(at) = wake {
            hub_paint.wake_predictions(&br_paint, br_ref, &bk_paint, at);
        }
        let _ime_rect = terminal_pane::paint_pane(
            g,
            rect,
            PanePaintArgs {
                term: &mut br_ref.terminal,
                frame: &br_ref.frame,
                theme: &theme_paint,
                metrics: &metrics_paint,
                focused,
                window_active: true,
                copy_mode: copy_mode.as_ref(),
                search_matches: &search_matches,
                hover_link: hover.as_ref(),
                preedit: None,
                predicted: predicted.as_ref(),
            },
            &vis_paint,
        );
        // (A hand over a link while the key that opens it is held.)
        g.set_cursor_style(if hover.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam }, rect);

        g.agent(agent::Node { role: "Terminal", name: Some(said_paint.clone().into()), bounds: rect, focused, enabled: true, clickable: false, ..Default::default() });
    })
    .size_full()
    // The link under the pointer, while the key that opens one is held
    // (and not otherwise: a pointer moving over a terminal costs
    // nothing): as the pointer moves, as the key goes down or up under
    // it, and none once the pointer has left.
    .on_mouse_move(move |e: &MouseMoveEvent, cx| {
        vis_hover.pointer.set(Some(e.position));
        if link_hover(&br_hover, &vis_hover, &metrics_hover, pad, olo_kit::keys::opens_links(&e.modifiers).then_some(e.position)) {
            link_tip(cx, &vis_hover, &metrics_hover, pad);
            pane_m4.send(());
        }
    })
    .on_modifiers_changed(move |e: &olo_kit::ModifiersChangedEvent, cx| {
        let at = vis_keys.pointer.get().filter(|_| olo_kit::keys::opens_links(&e.modifiers));
        if link_hover(&br_keys, &vis_keys, &metrics_keys, pad, at) {
            link_tip(cx, &vis_keys, &metrics_keys, pad);
            pane_m5.send(());
        }
    })
    .on_hover(move |over: &bool, cx| {
        if !*over {
            vis_left.pointer.set(None);
            if vis_left.hover.borrow_mut().take().is_some() {
                link_tip(cx, &vis_left, &metrics_left, pad);
                pane_m6.send(());
            }
        }
    })
    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
        let hub = app().hub.clone();
        hub.focus_block(&bk_mouse.host_key, &bk_mouse.session_id, &bk_mouse.block_id);
        let rect = vis_mouse.bounds.get();
        let (col, row) = metrics_mouse.cell_at(rect, pad, e.position, vis_mouse.shift.get());

        // A click with the command key opens the link under it: an
        // address a program named (OSC 8), one in the text, a path on
        // the host (an address of the host's own is forwarded here).
        if olo_kit::keys::opens_links(&e.modifiers) {
            link_hover(&br_mouse, &vis_mouse, &metrics_mouse, pad, Some(e.position));
            let target = vis_mouse.hover.borrow().as_ref().map(|h| h.target.clone());
            if let Some(t) = target {
                match t {
                    LinkTarget::Url(u) => app().hub.open_link_from_pane(&bk_mouse, u, Feedback::start(win_id)),
                    LinkTarget::FilePath { path, .. } => {
                        if let Some(slot) = wins().map.get(&win_id) {
                            slot.handle.messenger().send(WinMsg::OpenPreview(bk_mouse.clone(), path));
                        }
                    }
                }
                cx.stop_propagation();
                return;
            }
        }

        if !e.modifiers.shift {
            let sent = br_mouse.lock().unwrap_or_else(|e| e.into_inner()).mouse_report(MouseAction::Press, Some(VtButton::Left), key_mods(&e.modifiers), (col, row));
            if let Some(bytes) = sent {
                vis_mouse.drag_anchor.set(None);
                vis_mouse.reported_at.set(Some((col, row)));
                hub.send_input(&bk_mouse, bytes);
                cx.stop_propagation();
                return;
            }
        }
        let mut g = br_mouse.lock().unwrap_or_else(|e| e.into_inner());
        if e.click_count == 2 {
            let _ = g.terminal.select_word_at_viewport(col, row as u32);
            vis_mouse.drag_anchor.set(None);
        } else if e.click_count >= 3 {
            let _ = g.terminal.select_line_at_viewport(col, row as u32);
            vis_mouse.drag_anchor.set(None);
        } else {
            let _ = g.terminal.clear_selection();
            vis_mouse.drag_anchor.set(Some((col, row)));
        }
        pane_m1.send(());
    })
    .on_drag_move(move |e: &MouseMoveEvent, _| {
        // A program that has the mouse: told of the move, once a cell, the
        // button held with it (none where it asked for every move).
        let held = br_drag.lock().unwrap_or_else(|e| e.into_inner()).mouse_held;
        if held.is_some() || (e.pressed_button.is_none() && !e.modifiers.shift) {
            let rect = vis_drag.bounds.get();
            let cell = metrics_drag.cell_at(rect, pad, e.position, vis_drag.shift.get());
            if vis_drag.reported_at.get() != Some(cell) {
                let sent = br_drag.lock().unwrap_or_else(|e| e.into_inner()).mouse_report(MouseAction::Motion, held, key_mods(&e.modifiers), cell);
                if let Some(bytes) = sent {
                    vis_drag.reported_at.set(Some(cell));
                    app().hub.send_input(&bk_drag, bytes);
                }
            }
            if held.is_some() || e.pressed_button.is_none() {
                return;
            }
        }
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((sc, sr)) = vis_drag.drag_anchor.get() else { return };
        let rect = vis_drag.bounds.get();
        let (col, row) = metrics_drag.cell_at(rect, pad, e.position, vis_drag.shift.get());
        let mut g = br_drag.lock().unwrap_or_else(|e| e.into_inner());
        let _ = g.terminal.set_viewport_selection((sc, sr as u32), (col, row as u32), e.modifiers.alt);
        pane_m2.send(());
    })
    .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, cx| {
        // (A program that has the mouse is told of the right button too;
        // Shift brings the menu, as in a local terminal.)
        if !e.modifiers.shift {
            let cell = metrics_right.cell_at(vis_right.bounds.get(), pad, e.position, vis_right.shift.get());
            let sent = br_right.lock().unwrap_or_else(|e| e.into_inner()).mouse_report(MouseAction::Press, Some(VtButton::Right), key_mods(&e.modifiers), cell);
            if let Some(bytes) = sent {
                vis_right.reported_at.set(Some(cell));
                app().hub.send_input(&bk_rclick, bytes);
                cx.stop_propagation();
                return;
            }
        }
        // The menu bar's commands for this pane, by the names and with
        // the checks and keys they have there: the clipboard; its text;
        // the pane itself; closing it, alone and last.
        let help = app().hub.help(&bk_rclick);
        let (selected, copying, agent) = {
            let g = br_menu.lock().unwrap_or_else(|e| e.into_inner());
            (g.terminal.has_selection(), g.copy_mode.is_some(), g.runs_agent() || g.agent_activity != AgentActivity::Idle)
        };
        let mut menu = Menu::new("")
            .item("Copy", "terminal.copy")
            .enabled(selected)
            .item("Paste", "terminal.paste")
            .separator()
            .item("Find…", "terminal.find")
            .item("Copy Mode", "terminal.copy_mode")
            .checked(copying)
            .item("Quick Look", "files.quick_look")
            .enabled(selected)
            .item(olo_kit_base::tracker::LABEL, "files.open_in_tracker")
            .item("Clear Scrollback", "terminal.clear_scrollback")
            .separator()
            .item("Split Right", "pane.split_right")
            .item("Split Down", "pane.split_down")
            .item("Zoom Pane", "pane.zoom_toggle")
            .checked(zoomed)
            .enabled(zoomed || has_splits);
        // (For a pane an agent runs in, or where either is already on.)
        if agent || help.any() {
            menu = menu.item("Keep It Going", "agent.keep_going_toggle").checked(help.keep_going).item("Auto Approve", "agent.auto_approve_toggle").checked(help.auto_approve);
        }
        menu = menu.separator().item("Close Pane", "pane.close");
        menu::resolve(std::slice::from_mut(&mut menu), true);
        let hub = app().hub.clone();
        hub.focus_block(&bk_rclick.host_key, &bk_rclick.session_id, &bk_rclick.block_id);
        let target = bk_rclick.clone();
        let w = win_id;
        menu::context_menu(cx, menu, e.position, move |cmd: &str, _| {
            if let Some(slot) = wins().map.get(&w) {
                slot.handle.messenger().send(WinMsg::PaneCommand(target.clone(), cmd.to_owned()));
            }
        });
        cx.stop_propagation();
    })
    .on_scroll_wheel(move |e: &ScrollWheelEvent, cx| {
        let delta_y = f32::from(e.delta.pixel_delta(px(ch_scroll)).y);
        let mut g = br_scroll.lock().unwrap_or_else(|e| e.into_inner());
        // Where a local terminal sends it (`hub::wheel_goes`): the
        // history; the program that has the mouse; arrows for a pager or
        // an editor on the alternate screen that asked for them.
        let alternate = g.terminal.active_screen() == olo_rex_vt::ScreenKind::Alternate;
        let goes = crate::hub::wheel_goes(g.terminal.mouse_tracking(), e.modifiers.shift, alternate, g.terminal.dec_mode(1007));
        if goes == crate::hub::Wheel::Scrollback {
            vis_scroll.scroll_smooth(delta_y, ch_scroll, &mut g.terminal);
            pane_m3.send(());
            cx.stop_propagation();
            return;
        }
        let lines = vis_scroll.wheel_lines(delta_y, ch_scroll);
        let bytes = match goes {
            _ if lines == 0 => Vec::new(),
            crate::hub::Wheel::Report => {
                let cell = metrics_scroll.cell_at(vis_scroll.bounds.get(), pad, e.position, vis_scroll.shift.get());
                let button = if lines > 0 { VtButton::Four } else { VtButton::Five };
                (0..lines.unsigned_abs()).filter_map(|_| g.mouse_report(MouseAction::Press, Some(button), key_mods(&e.modifiers), cell)).flatten().collect()
            }
            crate::hub::Wheel::Arrows => crate::hub::arrows(lines > 0, lines.unsigned_abs(), g.terminal.dec_mode(1)),
            _ => Vec::new(),
        };
        drop(g);
        if !bytes.is_empty() {
            app().hub.send_input(&bk_scroll, bytes);
        }
        cx.stop_propagation();
    })
    // The middle button: the program's where it has the mouse, else a
    // paste, as in a local terminal.
    .on_mouse_down(MouseButton::Middle, move |e: &MouseDownEvent, cx| {
        let cell = metrics_middle.cell_at(vis_middle.bounds.get(), pad, e.position, vis_middle.shift.get());
        let sent = if e.modifiers.shift { None } else { br_middle.lock().unwrap_or_else(|e| e.into_inner()).mouse_report(MouseAction::Press, Some(VtButton::Middle), key_mods(&e.modifiers), cell) };
        match sent {
            Some(bytes) => {
                vis_middle.reported_at.set(Some(cell));
                app().hub.send_input(&bk_middle, bytes);
            }
            None => {
                if let Some(slot) = wins().map.get(&win_id) {
                    slot.handle.messenger().send(WinMsg::PaneCommand(bk_middle.clone(), "terminal.paste".into()));
                }
            }
        }
        cx.stop_propagation();
    });
    // A button let go of, over the screen or anywhere after a press on it:
    // the program that was told of the press is told of its release.
    let mut screen_canvas = screen_canvas;
    for (button, vt) in [(MouseButton::Left, VtButton::Left), (MouseButton::Right, VtButton::Right), (MouseButton::Middle, VtButton::Middle)] {
        for out in [false, true] {
            let (br_up, vis_up, metrics_up, bk_up) = (br.clone(), visual.clone(), metrics.clone(), bk.clone());
            let release = move |e: &MouseUpEvent, _: &mut WindowCx| {
                // (A selection's drag ends with the left button's release,
                // wherever it is: its anchor, kept, made the next press
                // anywhere in the window a drag of it, a tab or a sidebar's
                // row clicked with the pointer a pixel off selecting text in
                // the pane from the anchor, which the tab then opened with.
                // Ben, 2026-11-24: "a tab will open with some text selected
                // for no obvious reason"; the second investigator found it.)
                if button == MouseButton::Left {
                    vis_up.drag_anchor.set(None);
                }
                let cell = metrics_up.cell_at(vis_up.bounds.get(), pad, e.position, vis_up.shift.get());
                let sent = {
                    let mut g = br_up.lock().unwrap_or_else(|e| e.into_inner());
                    if g.mouse_held != Some(vt) {
                        return;
                    }
                    g.mouse_report(MouseAction::Release, Some(vt), key_mods(&e.modifiers), cell)
                };
                vis_up.reported_at.set(None);
                if let Some(bytes) = sent {
                    app().hub.send_input(&bk_up, bytes);
                }
            };
            screen_canvas = if out { screen_canvas.on_mouse_up_out(button, release) } else { screen_canvas.on_mouse_up(button, release) };
        }
    }
    if wants_motion {
        screen_canvas = screen_canvas.hears_motion();
    }

    let mut pane_col = div().relative().size_full().flex().flex_col();
    if let Some(h) = header_el {
        pane_col = pane_col.child(h);
    }
    if password_prompt {
        pane_col = pane_col.child(
            div()
                .w_full()
                .px(px(m::SPACING))
                .py(px(4.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(m::SPACING))
                .child(StringView::new("The program in this pane is asking for a password.").cut(true))
                .child(Button::new("Paste Password…").small().on_click(move |_, _| {
                    if let Some(slot) = wins().map.get(&win_id) {
                        slot.handle.messenger().send(WinMsg::Command("olo.passwords.fill".into()));
                    }
                })),
        );
    }
    // Its scrollback: the kit's slim bar over the screen's right edge, as
    // Terminal's, there when the pointer comes near or it scrolls; its
    // thumb drags the view, a click in its track pages. In rows: where
    // the view's top is, how many show, how many there are. (It had no
    // bar: how far back a pane was scrolled, nothing said.)
    let br_bar = br.clone();
    let pane_m4 = cx.messenger();
    let bar = widgets::SlimScrollBar::new(olo_kit::SharedString::from(format!("rex-scroll-{}", bk.block_id)), scrolled.offset as f32, scrolled.len as f32, scrolled.total as f32).on_scroll(
        move |to: &f32, _| {
            let mut g = br_bar.lock().unwrap_or_else(|e| e.into_inner());
            let want = to.round().max(0.0) as u64;
            if want != g.terminal.scrollbar().offset {
                let _ = g.terminal.scroll_to_row(want as usize);
                drop(g);
                pane_m4.send(());
            }
        },
    );
    let mut screen_box = div().relative().flex_1().min_h_0().size_full().child(screen_canvas).child(bar);
    // Auto Approve counting down to its Yes, with Stop (the question is
    // then the person's), as Terminal has it: at the screen's top right,
    // over older output (at its foot it covered, in a narrow pane, the
    // question it was about: an agent asks at the bottom).
    if let Some(left) = counting {
        let hub_stop = app().hub.clone();
        let key_stop = bk.clone();
        // (Its screen's width as last drawn: `pane_w` is the window's
        // share, the sidebar's width in it.)
        let drawn = f32::from(visual.bounds.get().size.width);
        let narrow = if drawn > 0.0 { drawn } else { pane_w } < COUNTDOWN_SHORT;
        screen_box = screen_box.child(div().absolute().right(px(m::SPACING)).top(px(m::SPACING)).child(widgets::agent_marks::auto_approve_countdown(
            left,
            narrow,
            format!("rex-auto-approve-stop-{}", bk.block_id),
            move |_, _| hub_stop.stop_auto_approve(&key_stop),
        )));
    }
    pane_col = pane_col.child(screen_box);

    pane_col
}

/// A row's icon (a session's in the sidebar, a pane's in its header and
/// in Activity): 16 points, the size the icons are drawn for.
/// The sidebar's least width, the kit's for a pane at a window's side:
/// its Sessions and Activity control still has both names whole and
/// room round them with a two-figure count (at 160, as it was,
/// "Activity" was cut once an agent waited).
const SIDE_LEAST: f32 = widgets::STRIP_SIDE_WIDTH;
const ROW_ICON: f32 = 16.0;
/// An Activity row: its name's line and one line under it; a second line
/// under it adds [`ACTIVITY_LINE`].
const ACTIVITY_ROW: f32 = 38.0;
const ACTIVITY_LINE: f32 = 15.0;

/// `text` in at most two lines of `size` points, the first no wider than
/// `room` (broken where a word ends: a word longer than that alone on it),
/// the second what is left (cut short when it's drawn).
fn two_lines(text: &str, room: f32, size: f32) -> (String, String) {
    let font = style::ui_font(FontWeight::NORMAL);
    let fits = |s: &str| f32::from(style::text_width(s.to_owned(), size, font.clone())) <= room;
    if fits(text) {
        return (text.to_owned(), String::new());
    }
    let words: Vec<&str> = text.split(' ').collect();
    let mut first = 0;
    while first < words.len() && fits(&words[..first + 1].join(" ")) {
        first += 1;
    }
    let first = first.max(1);
    (words[..first].join(" "), words[first..].join(" "))
}
/// What a pane's head has after its last button: as far from the pane's
/// edge as its icon is from the other (the terminal's own inset, a
/// window's).
const HEADER_END: f32 = crate::settings::PADDING.0;

fn sidebar_row(label: String, badge: Option<String>, act: AgentActivity, help: crate::agent::Help, selected: bool, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let key = style::key((&label, &badge, act as u8, &help, selected));
    let l_paint = label.clone();
    let b_paint = badge.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        // (The keyboard is the pane's, never the sidebar's: a list's
        // selection without it, as the kit's lists and sidebar draw one.
        // Opus's look at Activity.)
        if selected {
            style::fill_rect(g, b, style::c(style::selection_ground(&p, p.panel_background, false)));
        }
        let ink = style::c(if selected { style::selection_text(&p, p.panel_text, false) } else { p.panel_text });
        // Indent sessions 15 pt under host header so icon left aligns to exact pixel with host title (x=27).
        let text_middle = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        // (The icon at the size it is drawn for, as a list's small icons
        // are: at 12 points it was the 16-point one made smaller, and
        // soft at 1×. Its left edge is the host's name's.)
        let icon_b = style::snap_centred(g, point(b.left() + px(m::WINDOW_INSET + ROW_ICON / 2.0), text_middle), ROW_ICON, ROW_ICON);
        paint_activity_icon(g, icon_b, act, &help);
        let mut right = b.right() - px(8.0);
        if let Some(bg) = &b_paint {
            let r = style::label(g, right, b.center().y, style::Align::Right, bg.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::c([216, 56, 56, 255]));
            right = r.left() - px(6.0);
        }
        style::label_fit(
            g,
            b.left() + px(m::WINDOW_INSET + ROW_ICON + m::TIGHT),
            b.center().y,
            style::Align::Left,
            l_paint.clone(),
            style::PLAIN_SIZE,
            style::ui_font(FontWeight::NORMAL),
            ink,
            (right - (b.left() + px(m::WINDOW_INSET + ROW_ICON + m::TIGHT))).max(px(24.0)),
        );
        g.agent(agent::Node {
            role: "ListItem",
            name: Some(l_paint.clone().into()),
            value: activity_words(act).map(|w| olo_kit::words::tr(w).into()),
            bounds: b,
            selected: Some(selected),
            enabled: true,
            clickable: true,
            ..Default::default()
        });
    })
    .paint_key(key)
    .w_full()
    .h(px(m::LABEL_ROW + m::TIGHT))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

/// A split's divider being dragged: where the pointer took it, and the
/// ratio then.
#[derive(Default)]
struct SplitDrag(Option<(Point<Pixels>, f32)>);

/// How far either side of a split's line the pointer catches it; over
/// a line across, and under it. (Under it is the head of the pane below,
/// with its title and its buttons: the grip stops short of them, and
/// has the foot of the pane above, where nothing is, instead.)
const SPLIT_GRIP: f32 = 4.0;
const SPLIT_GRIP_OVER: f32 = 6.0;
const SPLIT_GRIP_UNDER: f32 = 1.0;

/// Where a split's divider is caught, in the split's box `b`: its line
/// (`line` points thick, after the first pane's `ratio` of the rest)
/// and the grip round it, the box's length less a grip's breadth at
/// each end (where it meets the grip of the split it is in, which has
/// the corner).
fn split_grip(b: Bounds<Pixels>, ratio: f32, side_by_side: bool, line: f32) -> Bounds<Pixels> {
    if side_by_side {
        let x = b.left() + px((f32::from(b.size.width) - line) * ratio);
        Bounds::from_corners(point(x - px(SPLIT_GRIP), b.top() + px(SPLIT_GRIP)), point(x + px(line + SPLIT_GRIP), b.bottom() - px(SPLIT_GRIP)))
    } else {
        // Horizontal divider grip inset 7 pt horizontally so it clears the 6 pt
        // Rex Sidebar divider grip on the left when splits stack vertically in
        // narrow tiles (vertical half / quarter).
        let inset = SPLIT_GRIP.max(7.0);
        let y = b.top() + px((f32::from(b.size.height) - line) * ratio);
        Bounds::from_corners(point(b.left() + px(inset), y - px(SPLIT_GRIP_OVER)), point(b.right() - px(inset), y + px(line + SPLIT_GRIP_UNDER)))
    }
}

/// A split's ratio with its divider dragged `moved` points from where
/// the ratio was `was`, the two panes having `span` points between
/// them: the divider under the pointer, neither pane under 15%. None
/// where there is no room to say.
fn split_dragged(was: f32, moved: f32, span: f32) -> Option<f32> {
    (span >= 1.0).then(|| (was + moved / span).clamp(0.15, 0.85))
}

/// The line under a pane's head and between panes: Terminal's.
fn pane_edge(p: &olo_theme::Palette) -> olo_kit::Hsla {
    if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.14)) } else { style::shade(p, p.panel_background, 1.22) }
}

/// High-contrast 2-line bevel pair `(shadow, highlight)` that stays crisp in Light, Dark, and Night modes.
fn bevel_colors(p: &olo_theme::Palette) -> (olo_kit::Hsla, olo_kit::Hsla) {
    if p.mode == olo_theme::ThemeMode::Dark {
        (style::shade(p, p.panel_background, 1.55), style::c(olo_theme::tint(p.panel_background, 0.80)))
    } else {
        (style::shade(p, p.panel_background, 1.45), style::shade(p, p.panel_background, 0.72))
    }
}

#[allow(clippy::too_many_arguments)]
fn sidebar_activity_row(
    row: ActivityRow,
    selected: bool,
    has_children: bool,
    expanded: bool,
    total_children: usize,
    failed_children: usize,
    width: f32,
    on_click: impl Fn(&(), &mut WindowCx) + 'static,
    on_toggle_fold: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let ActivityRow { title, detail, act, help, running, .. } = row;
    let state = activity_state_word(act, running);
    let left = m::WINDOW_INSET + ROW_ICON + m::TIGHT;
    let collapsed_badge: Option<(String, bool)> = if has_children && !expanded {
        if failed_children > 0 { Some((format!("{failed_children} failed"), true)) } else { Some((format!("{total_children} subagent{}", if total_children == 1 { "" } else { "s" }), false)) }
    } else {
        None
    };
    let badge_reserve = collapsed_badge.as_ref().map_or(0.0, |(s, is_err)| {
        let w = if *is_err { FontWeight::BOLD } else { FontWeight::NORMAL };
        f32::from(style::text_width(s.clone(), style::SMALL_SIZE, style::ui_font(w))) + 8.0
    });
    // Host is shown once in the section header above the list, leaving full width
    // for a clean 1-to-2 line description (short of the collapsed summary if any).
    let sub_lines = if detail.is_empty() {
        (String::new(), String::new())
    } else if collapsed_badge.is_some() {
        (detail.clone(), String::new())
    } else {
        two_lines(&detail, (width - left - m::TIGHT).max(48.0), style::SMALL_SIZE)
    };
    let height = if sub_lines.1.is_empty() { ACTIVITY_ROW } else { ACTIVITY_ROW + ACTIVITY_LINE };
    let key = style::key((&title, &sub_lines, act as u8, &help, running, selected, has_children, expanded, total_children, failed_children));
    let words = activity_row_words(act, running, &help);
    let fold_hit = Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
    let fold_hit_paint = fold_hit.clone();
    canvas(move |g, b| {
        fold_hit_paint.set(crate::transcript_view::disclosure_hit_rect(b));
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let ground = if selected { style::c(style::selection_ground(&p, p.panel_background, false)) } else { style::c(p.panel_background) };
        if selected {
            style::fill_rect(g, b, ground);
        }
        let (ink, quiet) = (style::c(p.panel_text), style::secondary_text(&p));
        let baseline = style::baseline_at(g, b.top() + px(12.0), style::PLAIN_SIZE);
        // Align 16 pt icon and disclosure latch cleanly on the name line's capitals middle.
        let name_mid = baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        if has_children {
            crate::transcript_view::paint_disclosure(g, point(b.left() + px(m::WINDOW_INSET / 2.0), name_mid), expanded, quiet);
        }
        let icon_b = style::snap_centred(g, point(b.left() + px(m::WINDOW_INSET + ROW_ICON / 2.0), name_mid), ROW_ICON, ROW_ICON);
        // Unified colour roles: green live dot for Working, orange ! for Waiting, red × for Failed, clean icon for Done.
        match act {
            AgentActivity::Working => {
                icons::paint(g, icon_b, "terminal");
                let dot_c = point(icon_b.right() - px(3.0), icon_b.top() + px(3.0));
                style::dot(g, dot_c, 7.0, ground);
                style::dot(g, dot_c, 5.0, style::c(p.success));
            }
            AgentActivity::Done => {
                icons::paint(g, icon_b, "terminal");
            }
            _ => {
                widgets::strip::drawn_on(ground, || widgets::agent_marks::paint(g, icon_b, "terminal", marks_of(act, &crate::agent::Help::default(), running)));
            }
        }
        // Connect vertical tree guide directly from 2 pt below the parent icon down to the first child row.
        if has_children && expanded {
            let guide_x = style::snap_coord(g, icon_b.center().x);
            let guide_c = if dark {
                let base_g = if selected { style::selection_ground(&p, p.panel_background, false) } else { p.panel_background };
                style::c(olo_theme::mix(base_g, p.panel_text, 0.34))
            } else {
                style::shade(&p, p.panel_background, 1.32)
            };
            style::vline(g, guide_x, icon_b.bottom() + px(2.0), b.bottom(), guide_c);
        }

        let mut right = b.right() - px(m::TIGHT);
        if help.auto_approve {
            widgets::agent_marks::auto_approve_mark(g, point(right - px(4.5), name_mid), 9.0, ground);
            right -= px(9.0 + m::MARK_GAP);
        }
        if help.keep_going {
            widgets::agent_marks::keep_going_mark(g, point(right - px(4.0), name_mid), 9.0, ground);
            right -= px(8.0 + m::MARK_GAP);
        }
        let left_x = b.left() + px(left);
        if let Some((word, urgent)) = state {
            let color = match urgent {
                Some(_) if act == AgentActivity::Error => style::c(p.failure),
                Some(tone) => style::c(olo_theme::mix(tone, p.panel_text, 0.3)),
                None if act == AgentActivity::Working => ink,
                None => style::disabled_text(&p),
            };
            let weight = if urgent.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL };
            let at = style::label_on_baseline(g, right - px(m::TIGHT), baseline, style::Align::Right, olo_kit::words::tr(word).to_string(), style::SMALL_SIZE, style::ui_font(weight), color);
            right = at.left();
        }
        style::label_fit(g, left_x, b.top() + px(12.0), style::Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, (right - px(10.0) - left_x).max(px(24.0)));
        let right_edge = b.right() - px(2.0 * m::TIGHT);
        if let Some((badge_str, is_err)) = &collapsed_badge {
            let b_color = if *is_err { style::c(p.failure) } else { quiet };
            let b_weight = if *is_err { FontWeight::BOLD } else { FontWeight::NORMAL };
            let b2 = style::baseline_at(g, b.top() + px(26.0), style::SMALL_SIZE);
            style::label_on_baseline(g, right_edge, b2, style::Align::Right, badge_str.clone(), style::SMALL_SIZE, style::ui_font(b_weight), b_color);
        }
        if !sub_lines.0.is_empty() {
            style::label_fit(
                g,
                left_x,
                b.top() + px(26.0),
                style::Align::Left,
                sub_lines.0.clone(),
                style::SMALL_SIZE,
                style::ui_font(FontWeight::NORMAL),
                quiet,
                (right_edge - px(badge_reserve) - left_x).max(px(24.0)),
            );
        }
        if !sub_lines.1.is_empty() {
            style::label_fit(
                g,
                left_x,
                b.top() + px(26.0 + ACTIVITY_LINE),
                style::Align::Left,
                sub_lines.1.clone(),
                style::SMALL_SIZE,
                style::ui_font(FontWeight::NORMAL),
                quiet,
                (right_edge - left_x).max(px(24.0)),
            );
        }
        g.agent(agent::Node {
            role: "ListItem",
            name: Some(title.clone().into()),
            value: words.clone().map(Into::into),
            bounds: b,
            selected: Some(selected),
            enabled: true,
            clickable: true,
            ..Default::default()
        });
    })
    .paint_key(key)
    .w_full()
    .h(px(height))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |e, cx| {
        if has_children && fold_hit.get().contains(&e.position) {
            on_toggle_fold(&(), cx);
        } else {
            on_click(&(), cx);
        }
        cx.stop_propagation();
    })
}

/// Compact 2-line nested subagent row (`38 pt`) under its parent Activity row:
/// vertically centered inside its selection band (~5.5 pt top & bottom padding),
/// connected tree guide, name-line-centred icon (terminal vs document carries
/// whether Terminal mode exists), title + right-aligned status on line 1
/// (>= 10 pt gutter), and full-width readable description on line 2.
fn sidebar_subagent_row(sub: olo_rex_client::activity::SubagentInfo, selected: bool, is_last: bool, _width: f32, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    use olo_rex_client::activity::{SubagentOrigin, SubagentState};
    const SUB_INDENT: f32 = 18.0;
    const SUB_ROW_H: f32 = 38.0;

    let origin_word = match sub.origin {
        SubagentOrigin::Tmux => "tmux",
        SubagentOrigin::Native => "subagent",
        SubagentOrigin::Shell => "shell",
    };
    let left = m::WINDOW_INSET + SUB_INDENT + ROW_ICON + m::TIGHT;
    let icon_name = if sub.tmux.is_some() { "terminal" } else { "text" };
    let (state_word, urgent) = crate::transcript_view::subagent_state_word(sub.state);
    let mark = crate::transcript_view::subagent_mark(sub.state);
    let key = style::key((&sub.id, &sub.name, &sub.description, sub.state, sub.tmux.is_some(), selected, is_last));
    let title = sub.name.clone();
    let desc = sub.description.clone();
    let sub_state = sub.state;
    let agent_val = format!("{origin_word}; {state_word}");

    canvas(move |g, b| {
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let sel_rgba = style::selection_ground(&p, p.panel_background, false);
        let ground = if selected { style::c(sel_rgba) } else { style::c(p.panel_background) };
        if selected {
            style::fill_rect(g, b, ground);
        }
        let (ink, quiet, tertiary) = (style::c(p.panel_text), style::secondary_text(&p), style::disabled_text(&p));
        // Centered vertically in the 38 pt band (~5.5 pt above line 1 and below line 2).
        let baseline1 = style::baseline_at(g, b.top() + px(11.5), style::PLAIN_SIZE);
        let name_mid = baseline1 - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(b.left() + px(m::WINDOW_INSET + SUB_INDENT + ROW_ICON / 2.0), name_mid), ROW_ICON, ROW_ICON);

        // Connected vertical & horizontal tree guide in crisp separator strength.
        let guide_x = style::snap_coord(g, b.left() + px(m::WINDOW_INSET + ROW_ICON / 2.0));
        let guide_end = if is_last { name_mid } else { b.bottom() };
        let guide_c = if dark {
            let base_g = if selected { sel_rgba } else { p.panel_background };
            style::c(olo_theme::mix(base_g, p.panel_text, if selected { 0.42 } else { 0.34 }))
        } else {
            style::shade(&p, p.panel_background, 1.32)
        };
        style::vline(g, guide_x, b.top(), guide_end, guide_c);
        style::hline(g, guide_x, icon_b.left() - px(2.0), name_mid, guide_c);

        match sub_state {
            SubagentState::Working => {
                icons::paint(g, icon_b, icon_name);
                let dot_c = point(icon_b.right() - px(3.0), icon_b.top() + px(3.0));
                style::dot(g, dot_c, 7.0, ground);
                style::dot(g, dot_c, 5.0, style::c(p.success));
            }
            SubagentState::Done => {
                icons::paint(g, icon_b, icon_name);
            }
            _ => {
                widgets::strip::drawn_on(ground, || {
                    widgets::agent_marks::paint(g, icon_b, icon_name, widgets::agent_marks::Marks { agent: mark, keep_going: false, auto_approve: false });
                });
            }
        }

        let right_edge = b.right() - px(2.0 * m::TIGHT);
        let state_color = match urgent {
            Some(_) if sub_state == SubagentState::Failed => style::c(p.failure),
            Some(tone) => style::c(olo_theme::mix(tone, p.panel_text, 0.3)),
            None if sub_state == SubagentState::Working => ink,
            None => tertiary,
        };
        let state_weight = if urgent.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL };
        let at = style::label_on_baseline(g, right_edge, baseline1, style::Align::Right, olo_kit::words::tr(state_word).to_string(), style::SMALL_SIZE, style::ui_font(state_weight), state_color);
        let left_x = b.left() + px(left);
        style::label_fit(g, left_x, b.top() + px(11.5), style::Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::MEDIUM), ink, (at.left() - px(10.0) - left_x).max(px(24.0)));

        // Full-width readable description on line 2 (origin lives in tooltip & status bar).
        if !desc.is_empty() {
            style::label_fit(g, left_x, b.top() + px(25.5), style::Align::Left, desc.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), quiet, (right_edge - left_x).max(px(24.0)));
        }

        g.agent(agent::Node {
            role: "ListItem",
            name: Some(title.clone().into()),
            value: Some(agent_val.clone().into()),
            bounds: b,
            selected: Some(selected),
            enabled: true,
            clickable: true,
            ..Default::default()
        });
    })
    .paint_key(key)
    .w_full()
    .h(px(SUB_ROW_H))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

/// An Activity row: a pane an agent runs in (or a shell, where idle panes
/// are shown), its host and what it says of itself, its agent's state,
/// what it has on, and whether an agent runs there now.
/// What an Activity row is called: its tab's name, as the tab strip
/// names it (`hub::session_tab_names`), and where the tab holds more than
/// one agent, each also by its own pane's title where that says more;
/// with no name for its tab, its pane's.
fn activity_title(tab: Option<&str>, agents_in_tab: usize, pane: String) -> String {
    match tab.filter(|n| !n.trim().is_empty()) {
        Some(name) if agents_in_tab > 1 && pane != name => format!("{name} · {pane}"),
        Some(name) => name.to_owned(),
        None => pane,
    }
}

struct ActivityRow {
    bk: BlockKey,
    host: String,
    title: String,
    detail: String,
    act: AgentActivity,
    help: crate::agent::Help,
    running: bool,
}

/// What an Activity row says its agent is doing, in a word, and the
/// colour of a state that wants the person (waiting, failed).
fn activity_state_word(act: AgentActivity, running: bool) -> Option<(&'static str, Option<olo_theme::Rgba>)> {
    let p = theme::palette();
    match act {
        AgentActivity::Blocked => Some(("Waiting", Some(p.hue(olo_theme::Hue::Orange)))),
        AgentActivity::Error => Some(("Failed", Some(p.failure))),
        AgentActivity::Working => Some(("Working", None)),
        AgentActivity::Done => Some(("Done", None)),
        AgentActivity::Idle if running => Some(("At rest", None)),
        AgentActivity::Idle => None,
    }
}

/// What an Activity row says, in words: its agent's state and what it
/// has on (its value to agents).
fn activity_row_words(act: AgentActivity, running: bool, help: &crate::agent::Help) -> Option<String> {
    let state = activity_words(act).map(|w| olo_kit::words::tr(w).to_string()).or_else(|| running.then(|| olo_kit::words::tr("An agent is at rest here").to_string()));
    let parts: Vec<String> = [state, help_words(help)].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// A folder as a row says it: its own name ("Home" for home), the part that
/// tells two apart.
fn folder_name(pwd: &str, home: Option<&str>) -> String {
    if pwd.is_empty() || Some(pwd.trim_end_matches('/')) == home.map(|h| h.trim_end_matches('/')) {
        // (Home in words: a bare "~" at a line's end read as a stray mark.)
        return if pwd.is_empty() { String::new() } else { olo_kit::words::tr("Home").to_string() };
    }
    pwd.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/").to_owned()
}

/// they are on.
fn paint_activity_icon(g: &mut Painter, b: Bounds<Pixels>, act: AgentActivity, help: &crate::agent::Help) {
    widgets::agent_marks::paint(g, b, "terminal", marks_of(act, help, false));
}

fn marks_of(act: AgentActivity, help: &crate::agent::Help, running: bool) -> widgets::agent_marks::Marks {
    use widgets::agent_marks::AgentMark;
    let agent = match act {
        // (At rest, where an agent runs: Activity's rows, where every row
        // is one and a quiet one still reads as there.)
        AgentActivity::Idle if running => AgentMark::AtRest,
        AgentActivity::Idle => AgentMark::None,
        AgentActivity::Working => AgentMark::Working,
        AgentActivity::Blocked => AgentMark::Asking,
        AgentActivity::Error => AgentMark::Failed,
        AgentActivity::Done => AgentMark::Done,
    };
    widgets::agent_marks::Marks { agent, keep_going: help.keep_going, auto_approve: help.auto_approve }
}

/// What a pane has on, in words, for its tab's and its row's words
/// (Terminal's).
fn help_words(help: &crate::agent::Help) -> Option<String> {
    let parts: Vec<String> =
        [help.keep_going.then(|| olo_kit::words::tr("kept going").to_string()), help.auto_approve.then(|| olo_kit::words::tr("auto approving").to_string())].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// What the mark on a terminal's icon says, in words (a row's tip, and
/// what agents read as its value); nothing at rest.
fn activity_words(act: AgentActivity) -> Option<&'static str> {
    match act {
        AgentActivity::Idle => None,
        AgentActivity::Working => Some("An agent is working here"),
        AgentActivity::Blocked => Some("An agent is waiting for you here"),
        AgentActivity::Error => Some("An agent stopped on an error here"),
        AgentActivity::Done => Some("An agent has finished here"),
    }
}

fn higher_activity(a: AgentActivity, b: AgentActivity) -> AgentActivity {
    if activity_sort_rank(b) < activity_sort_rank(a) { b } else { a }
}

fn activity_sort_rank(a: AgentActivity) -> u8 {
    match a {
        AgentActivity::Blocked => 0,
        AgentActivity::Error => 1,
        AgentActivity::Working => 2,
        AgentActivity::Done => 3,
        AgentActivity::Idle => 4,
    }
}

/// Encode special keys (Enter, Backspace, Tab, Esc, Arrows, Ctrl+letter, Alt+letter, F1-F12)
/// into standard VT escape sequences for direct responsive PTY transmission.
/// Whether a keystroke without Control is one of the keyboard profile's
/// keys for Rex (a function key, Shift+F3 for Find Previous with
/// Windows' keys): asked of the menu bar before it goes to the pane.
/// The profile's keys are kept until it changes; a letter or a digit
/// typed is never one, and is not looked up at all.
fn own_plain_shortcut(k: &olo_kit::Keystroke) -> bool {
    if k.modifiers.control || (k.key.chars().count() == 1 && !k.modifiers.alt) {
        return false;
    }
    thread_local! {
        static KEPT: std::cell::RefCell<(u64, Vec<olo_kit_base::keys::Shortcut>)> = const { std::cell::RefCell::new((u64::MAX, Vec::new())) };
    }
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        let now = olo_kit_base::keys::generation();
        if kept.0 != now {
            *kept = (now, olo_kit_base::keys::own_shortcuts("olo.rex", true).into_iter().filter(|s| !s.command).collect());
        }
        kept.1.iter().any(|s| olo_kit::keys::pressed(s, k))
    })
}

/// A key's bytes for the pane's program, as Terminal sends them
/// (`olo_kit::term_keys`: Option+←/→ a word back and on, modifiers in
/// xterm's `CSI 1;mods X`, the program's application cursor keys), or
/// None for what goes as text or to Rex's own key event. (It sent ESC
/// before an arrow's own sequence for Alt, ESC ESC [ D, which a shell
/// takes as Escape and then a stray `[D`; Ctrl and Shift on an arrow were
/// dropped.)
fn encode_terminal_special_key(ks: &olo_kit::Keystroke, modes: olo_kit::term_keys::Modes) -> Option<Vec<u8>> {
    if ks.modifiers.platform {
        return None;
    }
    olo_kit::term_keys::bytes(olo_kit::term_keys::Key::of(ks), modes)
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    /// What a key press costs: its pane's modes read from Ghostty and its
    /// bytes encoded (`cargo test --release -p olo-rex key_cost --
    /// --ignored --nocapture`).
    #[test]
    #[ignore]
    fn key_cost() {
        let terminal = std::sync::Mutex::new(olo_rex_vt::GhosttyTerminal::new(120, 40).unwrap());
        let mut k = olo_kit::Keystroke::parse("left").unwrap();
        k.modifiers.alt = true;
        let n = 200_000u32;
        let t = std::time::Instant::now();
        for _ in 0..n {
            let modes = {
                let t = terminal.lock().unwrap();
                olo_kit::term_keys::Modes { app_cursor: t.dec_mode(1), kitty: t.kitty_keyboard_flags() != 0 }
            };
            std::hint::black_box(encode_terminal_special_key(std::hint::black_box(&k), modes));
        }
        println!("a key press: {:?}", t.elapsed() / n);
    }

    /// A pane's keys as Terminal sends them: Option+←/→ a word back and
    /// on (not ESC before the arrow's own sequence), Ctrl and Shift on an
    /// arrow in xterm's form, application cursor keys where the program
    /// asked, Cmd's left to the app.
    #[test]
    fn a_panes_keys_are_terminals() {
        use olo_kit::term_keys::Modes;
        let key = |name: &str, alt: bool, ctrl: bool, shift: bool| {
            let mut k = olo_kit::Keystroke::parse(name).unwrap();
            (k.modifiers.alt, k.modifiers.control, k.modifiers.shift) = (alt, ctrl, shift);
            k
        };
        let plain = Modes::default();
        assert_eq!(encode_terminal_special_key(&key("left", true, false, false), plain).unwrap(), b"\x1bb");
        assert_eq!(encode_terminal_special_key(&key("right", true, false, false), plain).unwrap(), b"\x1bf");
        assert_eq!(encode_terminal_special_key(&key("left", false, true, false), plain).unwrap(), b"\x1b[1;5D");
        assert_eq!(encode_terminal_special_key(&key("up", false, false, true), plain).unwrap(), b"\x1b[1;2A");
        assert_eq!(encode_terminal_special_key(&key("left", false, false, false), plain).unwrap(), b"\x1b[D");
        assert_eq!(encode_terminal_special_key(&key("up", false, false, false), Modes { app_cursor: true, kitty: false }).unwrap(), b"\x1bOA");
        // (Under the kitty protocol, Option+← keeps xterm's form.)
        assert_eq!(encode_terminal_special_key(&key("left", true, false, false), Modes { app_cursor: false, kitty: true }).unwrap(), b"\x1b[1;3D");
        assert_eq!(encode_terminal_special_key(&key("c", false, true, false), plain).unwrap(), vec![3]);
        let mut cmd = key("left", false, false, false);
        cmd.modifiers.platform = true;
        assert_eq!(encode_terminal_special_key(&cmd, plain), None);
    }

    #[test]
    fn a_session_must_have_a_name_and_a_tab_need_not() {
        let session = Renaming { host_key: "local".into(), session_id: "s".into(), window_id: None, was: "work".into(), shown: "work".into(), from: WinId(1) };
        let tab = Renaming { window_id: Some("w".into()), was: String::new(), shown: "shell 3".into(), ..session.clone() };
        // A session's empty name isn't taken (it would show as its id).
        assert!(session.takes("build") && !session.takes(""));
        assert_eq!(session.hint(), "");
        // A tab's is: it shows what runs in it again, which its empty
        // field says.
        assert!(tab.takes("") && tab.takes("logs"));
        assert_eq!(tab.hint(), "shell 3");
    }

    #[test]
    fn a_splits_divider_is_caught_round_its_line_and_follows_the_pointer() {
        let b = Bounds { origin: point(px(100.0), px(50.0)), size: size(px(801.0), px(400.0)) };
        // Side by side, a quarter: the line after a quarter of the 800
        // points the panes have, caught four points either side of it,
        // the split's height less a grip's breadth at each end.
        let g = split_grip(b, 0.25, true, 1.0);
        assert_eq!((g.left(), g.right()), (px(296.0), px(305.0)));
        assert_eq!((g.top(), g.bottom()), (px(54.0), px(446.0)));
        // One above the other: over the line, and hardly under it (the
        // head of the pane below is there).
        let g = split_grip(b, 0.5, false, 0.5);
        assert_eq!((g.top(), g.bottom()), (px(50.0 + 199.75 - 6.0), px(50.0 + 199.75 + 0.5 + 1.0)));
        assert_eq!((g.left(), g.right()), (px(107.0), px(894.0)));
        // Dragged, the divider stays under the pointer: 80 points of 800
        // is a tenth, in a split of any width.
        assert_eq!(split_dragged(0.25, 80.0, 800.0), Some(0.35));
        assert_eq!(split_dragged(0.25, 160.0, 1600.0), Some(0.35));
        // Neither pane under 15%; and no room, nothing said.
        assert_eq!(split_dragged(0.25, -400.0, 800.0), Some(0.15));
        assert_eq!(split_dragged(0.5, 700.0, 800.0), Some(0.85));
        assert_eq!(split_dragged(0.5, 10.0, 0.0), None);
    }
    #[test]
    fn lost_connection_age_advances_at_word_boundaries() {
        assert_eq!(connection_age(Duration::from_secs(59)), "Connection lost just now.");
        assert_eq!(connection_age(Duration::from_secs(60)), "Connection lost 1 minute ago.");
        assert_eq!(connection_age(Duration::from_secs(120)), "Connection lost 2 minutes ago.");
        assert_eq!(connection_age(Duration::from_secs(3600)), "Connection lost 1 hour ago.");
    }
    #[test]
    fn server_requests_match_the_workspace_host_session_and_pane() {
        let view = WorkspaceSelection { host: "dev-box".into(), session: Some("api".into()), block: Some("shell".into()), active: true };
        assert!(view.matches("dev-box", Some("api"), Some("shell")));
        assert!(view.matches("dev-box", None, None));
        assert!(!view.matches("local", Some("api"), Some("shell")));
        assert!(!view.matches("dev-box", Some("main"), None));
        assert!(!view.matches("dev-box", Some("api"), Some("log")));
    }

    #[test]
    fn a_destroyed_selection_uses_its_neighbor_and_an_empty_host_has_none() {
        let rows = ["first", "second", "fourth"].map(|id| olo_rex_client::SessionSummary { session_id: id.into(), label: id.into() });
        assert_eq!(selected_session_index(&rows, Some("third"), 2), Some(1));
        assert_eq!(selected_session_index(&rows, Some("gone"), 0), Some(0));
        assert_eq!(selected_session_index(&rows, Some("fourth"), 3), Some(2));
        assert_eq!(selected_session_index(&[], Some("gone"), 0), None);
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;

    /// An Activity row goes by its tab's name, as the strip shows it: the
    /// name the tab was given over what its program calls itself (Ben,
    /// 2026-11-24: "it's showing some weird "2.1.331" title"); two agents
    /// in one tab, each by its pane too; a tab with no name, its pane's.
    #[test]
    fn an_agent_is_named_by_its_tab() {
        assert_eq!(activity_title(Some("Release notes"), 1, "Claude Code".into()), "Release notes");
        assert_eq!(activity_title(Some("Review"), 2, "Antigravity".into()), "Review · Antigravity");
        assert_eq!(activity_title(Some("Claude Code"), 2, "Claude Code".into()), "Claude Code");
        assert_eq!(activity_title(None, 1, "Claude Code".into()), "Claude Code");
        assert_eq!(activity_title(Some("  "), 1, "Codex".into()), "Codex");
    }

    /// What an agent says of itself on two lines where one won't hold it:
    /// broken where a word ends, the first line within its room, the rest
    /// on the second; one that fits, one line.
    #[test]
    fn a_long_description_takes_two_lines() {
        assert_eq!(two_lines("rex · This computer", 400.0, style::SMALL_SIZE), ("rex · This computer".to_owned(), String::new()));
        let said = "Running visual QA on the terminal pane's renderer at 1x, 1.5x and 2x · This computer";
        let (first, second) = two_lines(said, 160.0, style::SMALL_SIZE);
        assert!(!first.is_empty() && !second.is_empty(), "{first:?} {second:?}");
        assert_eq!(format!("{first} {second}"), said);
        assert!(f32::from(style::text_width(first.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))) <= 160.0, "{first:?}");
        // (A word longer than the room: alone on the first line.)
        assert_eq!(two_lines("Supercalifragilistic word", 20.0, style::SMALL_SIZE).0, "Supercalifragilistic");
    }
}

#[cfg(test)]
mod paste_tests {
    use super::*;

    /// A pasted picture's name: the UTC time, as a date and a time.
    #[test]
    fn a_pasted_pictures_name() {
        assert_eq!(utc_stamp(0), "1970-01-01-000000");
        assert_eq!(utc_stamp(1_795_512_965), "2026-11-24-093605");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29-000000");
    }
}
