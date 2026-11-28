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
    /// Screen currently shown in Rex's embedded `ScreenPane` (never restores Take Control).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_screen: Option<olo_screen_pane::ScreenId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_screen_tab: Option<olo_screen_pane::Tab>,
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
    /// Last-known remote host screens preserved across in-place Rex reloads.
    #[serde(default)]
    pub cached_host_screens: Vec<crate::screens_sock::SavedHostScreens>,
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
/// The hub, beside the app's state and never inside it: whatever is
/// asked of the hub may tell the windows at once, on the asking thread
/// (`handle_hub_notice`), and that takes the app's lock. With the hub
/// kept in `RexApp`, `app().hub.select_window(…)` held that lock for the
/// whole statement, and choosing a child from a tab's tree menu stopped
/// the window for good (Ben, 2026-11-26: "if i use the tree menu to try
/// switch to a subagent … it just hangs"; so did the path above a child
/// and Keep It Going). It is made once and never changes, so nothing
/// needs a lock to reach it.
static HUB: OnceLock<Arc<RexHub>> = OnceLock::new();
/// The same of what asks for a password and of what asks leave to run a
/// command: each tells the windows as it is answered, so neither is
/// reached through the app's lock.
static ASKPASS: OnceLock<Arc<OloAskpass>> = OnceLock::new();
static RUN_APPROVALS: OnceLock<Arc<crate::bridge_run::RunApprovals>> = OnceLock::new();
static WINS: OnceLock<Mutex<WinRegistry>> = OnceLock::new();

pub fn app() -> olo_sync::MutexGuard<'static, RexApp> {
    APP.wait().lock().unwrap_or_else(|e| e.into_inner())
}

/// The hub (the app has started: every window and message comes after).
/// No lock is taken for it; see [`HUB`].
pub fn the_hub() -> &'static Arc<RexHub> {
    HUB.wait()
}

fn the_askpass() -> &'static Arc<OloAskpass> {
    ASKPASS.wait()
}

fn the_run_approvals() -> &'static Arc<crate::bridge_run::RunApprovals> {
    RUN_APPROVALS.wait()
}

/// The hub, where the app has started.
pub fn app_hub() -> Option<Arc<RexHub>> {
    HUB.get().cloned()
}

fn wins() -> olo_sync::MutexGuard<'static, WinRegistry> {
    WINS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The desktop's settings for Rex changed (the settings' own thread).
pub fn follow_desktop_settings(rex: &olo_protocol::RexSettings) {
    let Some(hub) = app_hub() else { return };
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
    let hub = RexHub::start(settings.clone(), active_theme, Some(askpass.clone()), Some(run_approvals.clone()), handle_hub_notice);
    let keymap = KeymapEngine::new(&[], &[], &[]);
    let _ = HUB.set(hub.clone());
    let _ = ASKPASS.set(askpass);
    let _ = RUN_APPROVALS.set(run_approvals);
    let _ = APP.set(Mutex::new(RexApp { keymap, ssh_agent: Default::default(), renaming: None, attention: Attention::new(), broadcast_windows: HashSet::new() }));
    hub.start_screens_service();

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
            if let (Some(m), Some(hub)) = (APP.get(), HUB.get()) {
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
            } else if let Some(hub) = HUB.get() {
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
        HubNotice::OpenHost(host_key) => {
            // Dispatch onto Rex's single app looper so concurrent `OpenHost`
            // RPCs serialize cleanly around `open_or_raise_panel` and set the
            // initial `Screens` tab before opening the window.
            olo_kit::proxy().send(OpenHostOnApp { host_key, tab: crate::host::tab_named("screens") });
        }
        HubNotice::ScreensChanged => {
            broadcast_to_role(WinRole::Workspace, || WinMsg::ScreensMetaChanged);
            host_changed();
        }
    }
}

/// Cleanly tear down background listeners (such as Rex's local screens Unix
/// socket and state file) when Rex exits normally.
pub fn shutdown_app() {
    if let Some(hub) = HUB.get() {
        hub.shutdown_screens();
    }
}

/// Open the `olo run` desktop command approval sheet populated with a
/// deterministic demo prompt (`OLO_REX_DEMO_RUN=1|long|plain`) for headless visual QA.
pub fn open_demo_run_approve(mode: &str) {
    the_run_approvals().show_demo(mode);
    open_or_raise_panel(WinRole::RunApprove);
}

/// Open the Agent Settings batch conflict review window populated with
/// deterministic demo data (`OLO_REX_DEMO_CARRY=1|empty|remembered|merged`) for headless visual QA.
pub fn open_demo_carry() {
    let hub = the_hub().clone();
    crate::carry::open(&hub, "dev", "dev-box");
    open_or_raise_panel(WinRole::Carry);
    carry_changed();
}

/// Switch the demo workspace to Activity sidebar with deterministic native nested subagents
/// (`OLO_REX_DEMO_ACTIVITY=1|collapsed|child|transcript|follow|find|nomatch|screens|screens-empty|screens-selected|screens-collapsed|screens-collapsed-urgent|screens-filter|screens-type-filter|screens-control`) for headless visual QA.
pub fn open_demo_activity(mode: &str) {
    let hub = the_hub().clone();
    {
        let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.settings.sidebar_mode = SidebarMode::Activity;
        st.settings.sidebar_collapsed = false;
        if let Some(h) = st.hosts.get_mut("local")
            && let Some(s) = h.sessions.get_mut("demo-main")
        {
            if mode == "child" {
                s.active_window_id = Some("win-sub-1a".to_owned());
            } else if let Some(w) = s.windows.first_mut() {
                w.focused_block_id = Some("demo-agent".to_owned());
            }
        }
    }
    crate::activity_watch::apply_demo_activity_mode(mode);
    match mode {
        "find" | "nomatch" => {
            broadcast_to_role(WinRole::Workspace, || WinMsg::Command("terminal.find".into()));
        }
        "screens-collapsed" => {
            let mut mgr = crate::activity_watch::manager().lock().unwrap_or_else(|e| e.into_inner());
            mgr.folded.insert(BlockKey { host_key: "dev-box".into(), session_id: "dev-main".into(), block_id: "dev-lead".into() });
            mgr.folded.insert(BlockKey { host_key: "staging".into(), session_id: "stg-main".into(), block_id: "stg-qa".into() });
        }
        "screens-collapsed-urgent" => {
            let mut mgr = crate::activity_watch::manager().lock().unwrap_or_else(|e| e.into_inner());
            mgr.folded.insert(BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-sub-1".into() });
            mgr.folded.insert(BlockKey { host_key: "dev-box".into(), session_id: "dev-main".into(), block_id: "dev-lead".into() });
            mgr.folded.insert(BlockKey { host_key: "staging".into(), session_id: "stg-main".into(), block_id: "stg-qa".into() });
        }
        "screens-selected" | "screens-control" => {
            let take = mode == "screens-control";
            broadcast_to_role(WinRole::Workspace, move || WinMsg::OpenScreen {
                id: olo_screen_pane::ScreenId::host("dev-box", "dev-box", "verify-1"),
                tab: Some(olo_screen_pane::Tab::Live),
                take_control: take,
                expected_host_instance_id: None,
                expected_started: None,
            });
        }
        "screens-filter" => {
            broadcast_to_role(WinRole::Workspace, || WinMsg::SetActivitySearch("verify".into()));
        }
        "screens-type-filter" => {
            broadcast_to_role(WinRole::Workspace, || WinMsg::SetActivityTypeFilter(ActivityTypeFilter::Screens));
        }
        _ => {}
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

/// The Host window's Tweaks tab shows otherwise: the host being looked
/// at as root, a question, a change under way (`server::for_tweaks`).
pub fn tweaks_changed() {
    broadcast_to_role(WinRole::Host, || WinMsg::TweaksChanged);
}

/// The Host window's Tweaks tab is the one shown: its host is asked, as
/// root, how its settings stand, unless it has been (what it said is
/// kept while the window is of that host). Nothing where the host isn't
/// one reached over SSH, or isn't connected: the tab says so.
fn begin_tweaks(hub: &Arc<RexHub>) {
    let Some(shown) = crate::host::current().filter(|shown| shown.tab == crate::host::TWEAKS) else { return };
    let Some((name, target)) = managed_host(hub, &shown.key) else { return };
    if crate::server::for_tweaks(crate::server::current).is_some_and(|now| now.target == hub.with_agent(target.clone()) || crate::server::demo_mode()) {
        return;
    }
    let connected = hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_stands(&shown.key).is_some_and(|host| host.conn == crate::hub::HostConnState::Connected);
    if !connected {
        return;
    }
    // (Rex's demo, for the tab's pictures and its lint: a made-up
    // host's settings at a stage, `OLO_REX_DEMO_TWEAKS=STAGE`;
    // `unread`, the host not asked.)
    if crate::server::demo_mode() {
        let stage = std::env::var("OLO_REX_DEMO_TWEAKS").unwrap_or_else(|_| "limits".to_owned());
        crate::server::show_demo_tweaks(&stage, &name, target);
        tweaks_changed();
        return;
    }
    crate::server::for_tweaks(|| crate::server::open(hub, name, target));
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

/// Message sent through `olo_kit::proxy()` so `HubNotice::OpenHost` from a
/// background Tokio thread runs on Rex's single app looper (serializing
/// `open_or_raise_panel` and setting the initial tab before opening the window).
pub struct OpenHostOnApp {
    pub host_key: String,
    pub tab: Option<usize>,
}

/// Handle [`OpenHostOnApp`] on Rex's app looper.
pub fn open_host_on_app(host_key: &str, tab: Option<usize>) {
    if let Some(hub) = HUB.get() {
        open_host_with_tab(hub, host_key, tab);
    }
}

fn open_host(hub: &Arc<RexHub>, key: &str) {
    open_host_with_tab(hub, key, None);
}

fn open_host_with_tab(hub: &Arc<RexHub>, key: &str, tab: Option<usize>) {
    let Some((name, host_instance_id)) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(key).filter(|h| !h.is_local).map(|h| (h.label.clone(), h.host_instance_id)) else { return };
    // (Another host's in the window: what the one before said of its
    // settings goes, and root's connection to it.)
    if crate::host::current().is_some_and(|shown| shown.key != key || shown.host_instance_id != host_instance_id) {
        crate::server::for_tweaks(|| crate::server::close(hub));
    }
    crate::host::open_with_tab(key, &name, host_instance_id, tab);
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

/// Opens the Server window on the host `key` and jumps straight to the
/// Rex server Update confirmation once checked.
fn open_server_for_update(hub: &Arc<RexHub>, key: &str) {
    let Some((name, target)) = managed_host(hub, key) else { return };
    crate::server::open_for_update(hub, name, target);
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

/// Open a remote host's screen live inside Rex's workspace window (used by the
/// Host window's Screens tab "Look at Screen" button and `host.screen:KEY:look:NAME`).
/// Pins `(host_key, host_instance_id, expected_started)` (Astra #4), verifies
/// live streaming capability and screen presence, and reports failures directly
/// on the Host window's `screen_failed` row (Neon #12).
fn open_host_screen_live_in_rex(hub: &Arc<RexHub>, host_key: &str, host_instance_id: u64, host_label: &str, name: &str, expected_started: Option<u64>) {
    let check = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(host_key);
        let label = host.map(|h| h.label.clone()).unwrap_or_else(|| host_label.to_owned());
        if host_instance_id != 0 && host.is_none_or(|h| h.host_instance_id != host_instance_id) {
            Err(format!("“{label}” changed before this screen could open."))
        } else {
            match st.host_screens.get(host_key) {
                Some(live) if host_instance_id != 0 && live.host_instance_id != host_instance_id => Err(format!("“{label}” changed before this screen could open.")),
                Some(live) if !crate::activity_screens::host_supports_live_stream(live) => Err(format!("Update Olo’s helper on “{label}” to view screens live in Rex.")),
                Some(live) => match live.screens.iter().find(|s| s.name == name && s.state != olo_rex_link::screen_wire::HostScreenState::Archived) {
                    None => Err(format!("Screen “{name}” is no longer available on “{label}”.")),
                    Some(sc) if expected_started.is_some() && sc.started != expected_started => Err(format!("Screen “{name}” restarted on “{label}”. Please try again.")),
                    Some(sc) => Ok((label, live.host_instance_id, sc.started)),
                },
                None => Err(format!("Update Olo’s helper on “{label}” to view screens live in Rex.")),
            }
        }
    };
    let (label, inst_id, started) = match check {
        Ok(v) => v,
        Err(why) => {
            let what = olo_kit::tr!("Couldn’t open {}", name).to_string();
            crate::host::screen_failed(host_key, host_instance_id, name, &what, &why);
            return;
        }
    };
    let id = olo_screen_pane::ScreenId::host(host_key, label, name);
    if workspace().is_none() {
        open_workspace_window(None);
    }
    if let Some(w) = workspace() {
        w.messenger().send(WinMsg::OpenScreen { id, tab: Some(olo_screen_pane::Tab::Live), take_control: false, expected_host_instance_id: Some(inst_id), expected_started: started });
        w.messenger().send(WinMsg::Raise);
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

    /// What does this again, said to the window: its problem, should it
    /// have one, offers Try Again (Opus 5.5's look at the banner,
    /// 2026-11-25: "add optional retryable action … for actions that can
    /// be re-run directly"). Only for what is done again exactly as it
    /// was and harms nothing done twice.
    fn again(self, with: WinMsg) -> Self {
        self.send(WinMsg::RetryWith(self.serial, Box::new(with)));
        self
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
    /// What would do again the thing this feedback (its serial) is of:
    /// the problem's banner offers Try Again with it.
    RetryWith(u64, Box<WinMsg>),
    /// Try Again pressed on the problem's banner.
    TryAgain,
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
    /// Cmd+click a file or folder path in a terminal: open/reveal in Olo Tracker.
    RevealInTracker(BlockKey, String),
    /// Dismiss the current status-bar error banner (`×` button).
    DismissFeedback,
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
    /// The Host window's Tweaks tab stands otherwise.
    TweaksChanged,
    HostDo(crate::host_panel::HostDo),
    AccountChanged,
    AccountDo(crate::account_panel::AccountDo),
    ReposChosen,
    /// A row of the Host window's list of screens chosen.
    HostChosen,
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
    /// A tab's menu asked for (which top-level tab, where the pointer is).
    TabMenu(usize, Point<Pixels>),
    /// A tab's child-tab tree popover asked for (which top-level tab, anchor bounds).
    TabTreeMenu(usize, Bounds<Pixels>),
    /// The child-tab tree popover closed.
    TreeMenuClosed,
    /// Switch to a specific Rex window (parent or nested child subagent tab).
    SelectWindow(String, String, String),
    /// Stop a native Rex child-tab subagent (and optionally its descendants),
    /// pinned to the `(host_instance_id, conn_id)` captured when the menu built (Astra #1).
    StopSubagent {
        host_key: String,
        host_pin: Option<(u64, u64)>,
        session_id: String,
        window_id: String,
        force: bool,
        tree: bool,
    },
    /// Confirm stopping running subagents under a parent tab with a native kit alert,
    /// pinned to the `(host_instance_id, conn_id)` captured when the menu built (Astra #1).
    ConfirmStopSubagents {
        host_key: String,
        host_pin: Option<(u64, u64)>,
        session_id: String,
        children: Vec<RunningChild>,
    },
    ActivityChanged,
    ScreensMetaChanged,
    ToggleActivityFold(BlockKey),
    SetActivityTypeFilter(ActivityTypeFilter),
    SetActivityStateFilter(ActivityStateFilter),
    SetActivitySearch(String),
    ClearActivityFilters,
    OpenPaneTranscript(BlockKey, String),
    OpenScreen {
        id: olo_screen_pane::ScreenId,
        tab: Option<olo_screen_pane::Tab>,
        take_control: bool,
        expected_host_instance_id: Option<u64>,
        expected_started: Option<u64>,
    },
    OpenScreenMenu(Point<Pixels>),
    CloseScreen,
    ScreenStopConfirm(crate::activity_screens::ActivityScreenItem),
    ScreenStart(crate::activity_screens::ActivityScreenItem),
    ConfirmStopMinder {
        host_key: String,
        host_pin: Option<(u64, u64)>,
        session_id: String,
        window_id: String,
        title: String,
        with_agents: bool,
    },
    SetContextMenuRow(Option<String>),
    TranscriptDo(crate::transcript_view::TranscriptDo),
    SidebarMoved,
    CloseWindow,
}

/// Filter by agent harness, screens, or a specific native subagent role/title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ActivityTypeFilter {
    #[default]
    All,
    Claude,
    Codex,
    Agy,
    Shell,
    Screens,
    SubagentType(String),
}

impl ActivityTypeFilter {
    fn label(&self) -> String {
        match self {
            Self::All => olo_kit::words::tr("All types").to_owned(),
            Self::Claude => "Claude".into(),
            Self::Codex => "Codex".into(),
            Self::Agy => "Antigravity".into(),
            Self::Shell => "Shell".into(),
            Self::Screens => olo_kit::words::tr("Screens").to_owned(),
            Self::SubagentType(name) => name.clone(),
        }
    }

    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "all" => Self::All,
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "agy" | "antigravity" => Self::Agy,
            "shell" => Self::Shell,
            "screen" | "screens" => Self::Screens,
            _ => Self::SubagentType(s.trim().to_owned()),
        }
    }
}

/// Filter Activity rows and nested subagents by live run state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivityStateFilter {
    #[default]
    All,
    Active,
    Working,
    Waiting,
    Failed,
    Done,
}

impl ActivityStateFilter {
    const ALL_VARIANTS: [Self; 6] = [Self::All, Self::Active, Self::Working, Self::Waiting, Self::Failed, Self::Done];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All states",
            Self::Active => "Active",
            Self::Working => "Working",
            Self::Waiting => "Waiting",
            Self::Failed => "Failed",
            Self::Done => "Done",
        }
    }

    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "active" => Self::Active,
            "working" => Self::Working,
            "waiting" | "blocked" => Self::Waiting,
            "failed" | "error" => Self::Failed,
            "done" => Self::Done,
            _ => Self::All,
        }
    }
}

/// Active embedded Agent Screen view displayed in Rex's center area instead of
/// the terminal panes.
#[derive(Clone)]
enum BoundScreenSource {
    Local(Arc<olo_screen_pane::LocalScreenSource>),
    Remote(Arc<olo_screen_pane::StreamScreenSource>),
    Demo(Arc<crate::activity_screens::DemoPaneSource>),
}

impl BoundScreenSource {
    fn as_dyn(&self) -> Arc<dyn olo_screen_pane::ScreenSource> {
        match self {
            Self::Local(s) => s.clone(),
            Self::Remote(s) => s.clone(),
            Self::Demo(s) => s.clone(),
        }
    }

    fn set_info(&self, info: olo_screen_pane::ScreenInfo, started: Option<u64>) {
        match self {
            Self::Local(s) => {
                s.set_session_started(started);
                s.set_info(info);
            }
            Self::Remote(s) => {
                s.set_session_started(started);
                s.set_info(info);
            }
            Self::Demo(_) => {}
        }
    }

    fn set_probed_caps(&self, caps: olo_screen_pane::HostCapabilities) {
        if let Self::Remote(s) = self {
            s.set_probed_caps(caps);
        }
    }
}

struct ActiveScreenState {
    id: olo_screen_pane::ScreenId,
    tab: olo_screen_pane::Tab,
    controlling: bool,
    /// One-shot pending `Take Control` requested from the row's context menu while
    /// a remote screen's initial live frame is still arriving (cleared immediately
    /// on first eligible frame, deactivation, refusal, tab change, or rebind).
    pending_take_control: bool,
    bound_started: Option<u64>,
    bound_host_conn: u64,
    bound_host_instance_id: u64,
    bound_caps: Option<olo_screen_pane::HostCapabilities>,
    source: Option<BoundScreenSource>,
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
    /// What does again the thing the feedback is of, where it can be
    /// done again as it was (opening a session, reloading the server's
    /// configuration): Try Again on its problem's banner.
    retry: Option<(u64, WinMsg)>,
    /// Try Again was pressed and what it started hasn't said how it went:
    /// the problem stays where it is with its button dimmed, so the
    /// banner doesn't go and come back (Opus 5.5's look, 2026-11-26:
    /// "grey the button out where it is. Don't hide it").
    retrying: bool,
    problem: Option<(String, Option<(Child<widgets::TextView>, usize)>)>,
    notice: olo_kit::due::Notice,
    due: olo_kit::due::Due,
    pub side_pane: SidePane,
    sidebar_scroll: olo_kit::ScrollHandle,
    run_scroll: olo_kit::ScrollHandle,
    transcript_scroll: olo_kit::ScrollHandle,
    last_transcript_ptr: usize,
    last_transcript_gen: u64,
    activity_search_field: Option<Child<TextField>>,
    activity_type_filter: ActivityTypeFilter,
    activity_state_filter: ActivityStateFilter,
    active_screen: Option<ActiveScreenState>,
    screen_pane: Option<Child<olo_screen_pane::ScreenPane>>,
    resolved_screens_cache: Option<(u64, u64, Arc<crate::activity_screens::ResolvedActivityScreens>)>,
    pending_restored_screen: Option<(olo_screen_pane::ScreenId, olo_screen_pane::Tab)>,
    context_menu_row: Option<String>,
    sidebar_row_bounds: Rc<std::cell::RefCell<HashMap<String, Bounds<Pixels>>>>,
    open_tree_tab: Option<usize>,
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
    /// The Host window's Screens tab: the host's screens as its list
    /// has them, and the one chosen, kept by its name as others come,
    /// stop and go.
    pub host_screens_list: widgets::ListModel,
    pub host_screen_lines: Rc<Vec<crate::host_panel::ScreenLine>>,
    pub host_screen_chosen: Option<String>,
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
        self.retry = None;
        self.retrying = false;
        self.problem = None;
        self.notice.clear();
    }

    fn screen_controlling(&self) -> bool {
        self.active_screen.as_ref().is_some_and(|s| s.controlling)
    }

    /// Resolve the local Rex Unix socket path for `h` from cached state ONLY
    /// (zero UI-thread filesystem `stat()`/`exists()` calls — Neon #11.3).
    fn resolved_local_socket(h: &crate::hub::HostRuntime) -> Option<String> {
        if let Some(olo_rex_client::RexEndpoint::Unix(p)) = &h.endpoint {
            return Some(p.to_string_lossy().into_owned());
        }
        if let Some(path) = h.endpoint_raw.strip_prefix("unix://")
            && !path.is_empty()
        {
            return Some(path.to_owned());
        }
        if h.endpoint_raw.starts_with('/') {
            return Some(h.endpoint_raw.clone());
        }
        olo_rex_client::local_socket_candidates().into_iter().next().map(|p| p.to_string_lossy().into_owned())
    }

    /// Resolve all local and remote screens against rendered live blocks in `HubState`.
    ///
    /// - Uses actual `HostRuntime::is_local` instead of key `"local"` (Astra #15).
    /// - Resolves the local Unix socket path (Astra #16).
    /// - Maps every split pane inside a child subagent tab onto that subagent's
    ///   rendered primary block ID so screens owned by a split pane inside a child
    ///   tab are never orphaned (Astra #25).
    fn resolve_screens_from_hub(st: &crate::hub::HubState) -> crate::activity_screens::ResolvedActivityScreens {
        let mut live_blocks = HashSet::new();
        let mut child_block_alias: HashMap<BlockKey, BlockKey> = HashMap::new();
        let mut local_host_keys = HashSet::new();
        let mut host_labels = HashMap::new();
        let mut host_disconnected = HashMap::new();
        let mut local_socket: Option<String> = None;
        for hk in &st.host_order {
            let Some(h) = st.hosts.get(hk) else { continue };
            host_labels.insert(h.key.clone(), h.label.clone());
            host_disconnected.insert(h.key.clone(), !h.is_local && h.conn_state != HostConnState::Connected);
            if h.is_local {
                local_host_keys.insert(h.key.clone());
                local_socket = Self::resolved_local_socket(h);
            }
            for s in &h.session_summaries {
                let Some(v) = h.sessions.get(&s.session_id) else { continue };
                let live_wins: HashSet<String> = v.windows.iter().map(|w| w.window_id.clone()).collect();
                for w in &v.windows {
                    if st.subagents.is_child_window(&h.key, &v.session_id, &w.window_id, &live_wins) {
                        if let Some(sub) = st.subagents.by_window(&h.key, &v.session_id, &w.window_id) {
                            let primary_bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: sub.block_id.clone() };
                            live_blocks.insert(primary_bk.clone());
                            for b in w.all_blocks() {
                                let blk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                if blk != primary_bk {
                                    live_blocks.insert(blk.clone());
                                    child_block_alias.insert(blk, primary_bk.clone());
                                }
                            }
                        }
                    } else {
                        for b in w.all_blocks() {
                            live_blocks.insert(BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() });
                        }
                    }
                }
            }
        }
        let mut resolved = crate::activity_screens::resolve_activity_screens(
            &st.host_order,
            &local_host_keys,
            &host_labels,
            &host_disconnected,
            local_socket.as_deref(),
            &live_blocks,
            &st.desktop_screens,
            &st.desktop_screen_owners,
            &st.host_screens,
        );
        // Remap any screens attributed to a secondary split pane inside a child
        // subagent tab onto the child subagent's rendered primary block (Astra #25).
        if !child_block_alias.is_empty() {
            let mut moved: Vec<(BlockKey, Vec<crate::activity_screens::ActivityScreenItem>)> = Vec::new();
            for (alias_bk, primary_bk) in &child_block_alias {
                if let Some(mut items) = resolved.by_owner.remove(alias_bk) {
                    for it in &mut items {
                        it.owner_bk = Some(primary_bk.clone());
                        if let Some(by_id_it) = resolved.by_id.get_mut(&it.id) {
                            by_id_it.owner_bk = Some(primary_bk.clone());
                        }
                    }
                    moved.push((primary_bk.clone(), items));
                }
            }
            for (primary_bk, items) in moved {
                let list = resolved.by_owner.entry(primary_bk).or_default();
                list.extend(items);
                list.sort_by(|a, b| {
                    let rank = |s: olo_screen_pane::ScreenState| match s {
                        olo_screen_pane::ScreenState::Running => 0u8,
                        olo_screen_pane::ScreenState::Stopped => 1u8,
                        olo_screen_pane::ScreenState::Archived => 2u8,
                    };
                    rank(a.state).cmp(&rank(b.state)).then_with(|| b.last_used_ms.cmp(&a.last_used_ms)).then_with(|| a.id.name.cmp(&b.id.name))
                });
            }
        }
        resolved
    }

    /// Return a cached `Arc<ResolvedActivityScreens>` keyed in O(1) by
    /// `(st.screens_rev, st.topology_rev)` — zero terminal walks on cache hit,
    /// and catches `parent_window_id` / session / host / endpoint changes (Astra #25).
    fn resolved_screens_cached(&mut self, st: &crate::hub::HubState) -> Arc<crate::activity_screens::ResolvedActivityScreens> {
        let srev = st.screens_rev;
        let trev = st.topology_rev;
        if let Some((cs, ct, ref cached)) = self.resolved_screens_cache
            && cs == srev
            && ct == trev
        {
            return cached.clone();
        }
        let fresh = Arc::new(Self::resolve_screens_from_hub(st));
        self.resolved_screens_cache = Some((srev, trev, fresh.clone()));
        fresh
    }

    /// Look up a verified screen item (either resolved in Activity or a verified
    /// non-archived stopped screen in raw hub state), NEVER fabricating a running
    /// screen with unknown generation (Astra #9).
    fn lookup_verified_screen(
        st: &crate::hub::HubState,
        resolved: &crate::activity_screens::ResolvedActivityScreens,
        id: &olo_screen_pane::ScreenId,
    ) -> Option<crate::activity_screens::ActivityScreenItem> {
        if let Some(item) = resolved.by_id.get(id) {
            return Some(item.clone());
        }
        match &id.location {
            olo_screen_pane::ScreenLocation::Local => {
                let sc = st.desktop_screens.iter().find(|s| s.name == id.name)?;
                if sc.state == olo_rex_link::screen_wire::HostScreenState::Archived {
                    return None;
                }
                let state = match sc.state {
                    olo_rex_link::screen_wire::HostScreenState::Running => olo_screen_pane::ScreenState::Running,
                    olo_rex_link::screen_wire::HostScreenState::Stopped => olo_screen_pane::ScreenState::Stopped,
                    olo_rex_link::screen_wire::HostScreenState::Archived => return None,
                };
                let size = (sc.width > 0 && sc.height > 0).then_some((sc.width as i32, sc.height as i32));
                Some(crate::activity_screens::ActivityScreenItem {
                    id: id.clone(),
                    run_host_key: "local".to_owned(),
                    host_instance_id: 0,
                    conn_id: 0,
                    run_host_label: "This computer".to_owned(),
                    section_host_key: "local".to_owned(),
                    display_title: sc.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&sc.name).to_owned(),
                    title: sc.title.clone(),
                    state,
                    size,
                    scale: if sc.scale.is_finite() && sc.scale > 0.0 { sc.scale } else { 1.0 },
                    sound: sc.sound,
                    pid: sc.pid,
                    started: sc.started,
                    last_used_ms: sc.last_used,
                    actions: sc.actions,
                    controlled: sc.controlled,
                    host_disconnected: false,
                    disconnected_at_ms: None,
                    capabilities: olo_screen_pane::PaneCapabilities::local(&sc.name),
                    owner_bk: None,
                    depth: 0,
                    is_last: false,
                    guides: Vec::new(),
                })
            }
            olo_screen_pane::ScreenLocation::Host { host_id, host_label } => {
                let host = st.hosts.get(host_id)?;
                let live = st.host_screens.get(host_id)?;
                let sc = live.screens.iter().find(|s| s.name == id.name)?;
                if sc.state == olo_rex_link::screen_wire::HostScreenState::Archived {
                    return None;
                }
                let state = match sc.state {
                    olo_rex_link::screen_wire::HostScreenState::Running => olo_screen_pane::ScreenState::Running,
                    olo_rex_link::screen_wire::HostScreenState::Stopped => olo_screen_pane::ScreenState::Stopped,
                    olo_rex_link::screen_wire::HostScreenState::Archived => return None,
                };
                let disconnected = host.conn_state != HostConnState::Connected || live.conn_state != olo_screen_pane::rex_sock::RexHostConn::Connected;
                let caps = crate::activity_screens::pane_capabilities_for_host(live, disconnected);
                let size = (sc.width > 0 && sc.height > 0).then_some((sc.width as i32, sc.height as i32));
                Some(crate::activity_screens::ActivityScreenItem {
                    id: id.clone(),
                    run_host_key: host_id.clone(),
                    host_instance_id: live.host_instance_id,
                    conn_id: live.conn_id,
                    run_host_label: if host.label.is_empty() { host_label.clone() } else { host.label.clone() },
                    section_host_key: host_id.clone(),
                    display_title: sc.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&sc.name).to_owned(),
                    title: sc.title.clone(),
                    state,
                    size,
                    scale: if sc.scale.is_finite() && sc.scale > 0.0 { sc.scale } else { 1.0 },
                    sound: sc.sound,
                    pid: sc.pid,
                    started: sc.started,
                    last_used_ms: sc.last_used,
                    actions: sc.actions,
                    controlled: sc.controlled,
                    host_disconnected: disconnected,
                    disconnected_at_ms: if disconnected { live.disconnected_at } else { None },
                    capabilities: caps,
                    owner_bk: None,
                    depth: 0,
                    is_last: false,
                    guides: Vec::new(),
                })
            }
        }
    }

    fn build_screen_source(&mut self, item: &crate::activity_screens::ActivityScreenItem, show_location: bool) -> Option<(BoundScreenSource, u64, u64, Option<olo_screen_pane::HostCapabilities>)> {
        let info = item.to_screen_info(show_location);
        if crate::server::demo_mode() {
            return Some((BoundScreenSource::Demo(Arc::new(crate::activity_screens::DemoPaneSource::new(info))), 1, item.host_instance_id, None));
        }
        match &item.id.location {
            olo_screen_pane::ScreenLocation::Local => {
                let src = Arc::new(olo_screen_pane::LocalScreenSource::new(info));
                src.set_session_started(item.started);
                Some((BoundScreenSource::Local(src), 0, 0, None))
            }
            olo_screen_pane::ScreenLocation::Host { host_id, .. } => {
                let hub = the_hub().clone();
                let (conn_id, host_inst_id, probed_caps) = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    let host = st.hosts.get(host_id)?;
                    let live = st.host_screens.get(host_id)?;
                    // Astra #4: verify the selected host instance instead of silently
                    // substituting a replacement host that reused the same key.
                    if item.host_instance_id != 0 && (host.host_instance_id != item.host_instance_id || live.host_instance_id != item.host_instance_id) {
                        return None;
                    }
                    let conn = host.screens_conn_id;
                    let inst = live.host_instance_id;
                    let caps = crate::activity_screens::host_capabilities_for_live(live);
                    (conn, inst, caps)
                };
                // Only use `HostProtocolCache::fixed` once capabilities have actually
                // been probed; otherwise use `HostProtocolCache::new()` so `--protocol`
                // probing works cleanly.
                let cache = match probed_caps.clone() {
                    Some(caps) => Arc::new(olo_screen_pane::HostProtocolCache::fixed(caps)),
                    None => Arc::new(olo_screen_pane::HostProtocolCache::new()),
                };
                let opener =
                    Arc::new(crate::activity_screens::HubStreamOpener::new(hub.screens_service(), hub.rt_handle(), host_id, host_inst_id, (conn_id != 0).then_some(conn_id), item.started, item.pid));
                let src = Arc::new(olo_screen_pane::StreamScreenSource::with_host_cache(info, opener, cache));
                src.set_session_started(item.started);
                Some((BoundScreenSource::Remote(src), conn_id, host_inst_id, probed_caps))
            }
        }
    }

    fn ensure_screen_pane(&mut self, initial_tab: olo_screen_pane::Tab, cx: &mut olo_kit::Cx<Self>) {
        if self.screen_pane.is_some() {
            return;
        }
        let win_id = self.id;
        let pane = cx.new_view(move |cx| {
            let mut p = olo_screen_pane::ScreenPane::new(initial_tab, cx);
            p.set_raw_shortcuts(true);
            // Astra #25 / Neon #2: share Rex's physical command-key check with
            // `ScreenPane::key` so Left `Ctrl+Shift+K` passes through on Mac/BeOS.
            p.set_physical_command_check(keymap::command_held);
            // Opus #13: refresh host status-bar zoom % once whenever PictureWell
            // paints at a new drawn width (exact match to rendered rectangle).
            p.on_live_scale_changed(move |_| {
                if let Some(slot) = wins().map.get(&win_id) {
                    slot.handle.messenger().send(WinMsg::Redraw);
                }
            });
            // Opus Minor: Rex's status bar shows `W × H · N%`, so drop the duplicate
            // `· 1440 × 900` segment from Rex's embedded pane header subtitle.
            p.set_show_header_size(false, cx);
            p
        });
        cx.subscribe(&pane, |w: &mut Self, ev: &olo_screen_pane::ScreenPaneEvent, cx| match ev {
            olo_screen_pane::ScreenPaneEvent::Controlling(id, on) => {
                if let Some(st) = &mut w.active_screen
                    && &st.id == id
                {
                    st.controlling = *on;
                    // Clear one-shot pending intent on grant OR refusal/release (Astra #4 / Neon #3).
                    st.pending_take_control = false;
                    cx.invalidate();
                }
            }
            olo_screen_pane::ScreenPaneEvent::TabChanged(tab) => {
                if let Some(st) = &mut w.active_screen {
                    st.tab = *tab;
                    if *tab != olo_screen_pane::Tab::Live {
                        st.pending_take_control = false;
                    }
                }
                cx.invalidate();
            }
            olo_screen_pane::ScreenPaneEvent::OpenHost(hk) => {
                let hub = the_hub().clone();
                open_host_with_tab(&hub, hk, Some(3));
            }
            olo_screen_pane::ScreenPaneEvent::Renamed(id, title) => {
                if crate::server::demo_mode() {
                    let hub = the_hub().clone();
                    let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    match &id.location {
                        olo_screen_pane::ScreenLocation::Local => {
                            if let Some(sc) = st.desktop_screens.iter_mut().find(|s| s.name == id.name) {
                                sc.title = title.clone();
                            }
                        }
                        olo_screen_pane::ScreenLocation::Host { host_id, .. } => {
                            if let Some(live) = st.host_screens.get_mut(host_id)
                                && let Some(sc) = live.screens.iter_mut().find(|s| s.name == id.name)
                            {
                                sc.title = title.clone();
                            }
                        }
                    }
                    st.screens_rev = st.screens_rev.wrapping_add(1);
                }
                cx.invalidate();
            }
            olo_screen_pane::ScreenPaneEvent::Started(_)
            | olo_screen_pane::ScreenPaneEvent::Stopped(_)
            | olo_screen_pane::ScreenPaneEvent::Cleared(_)
            | olo_screen_pane::ScreenPaneEvent::Used(_, _)
            | olo_screen_pane::ScreenPaneEvent::RenameFailed(_)
            | olo_screen_pane::ScreenPaneEvent::StartRequested(_)
            | olo_screen_pane::ScreenPaneEvent::StopRequested(_) => {
                cx.invalidate();
            }
        });
        self.screen_pane = Some(pane);
    }

    fn open_screen(
        &mut self,
        id: olo_screen_pane::ScreenId,
        tab: Option<olo_screen_pane::Tab>,
        take_control: bool,
        expected_host_instance_id: Option<u64>,
        expected_started: Option<u64>,
        cx: &mut olo_kit::Cx<Self>,
    ) {
        // Astra #21: keep screen view local to this workspace window instead of
        // closing the process-wide transcript viewer in another window.
        self.pending_restored_screen = None;
        self.find_open = false;
        self.dismiss_overlay(cx);

        let (item_opt, show_location) = {
            let hub = the_hub().clone();
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let resolved = self.resolved_screens_cached(&st);
            let show_loc = st.hosts.values().any(|h| !h.is_local);
            let item = Self::lookup_verified_screen(&st, &resolved, &id);
            (item, show_loc)
        };
        let Some(item) = item_opt else {
            // Never fabricate a running screen with unknown generation.
            if self.active_screen.as_ref().is_some_and(|s| s.id == id) {
                self.close_screen(cx);
            }
            return;
        };
        // Astra #4: if the caller pinned a host instance or screen generation
        // (e.g. from a context menu or Host ▸ Look click), reject mismatches
        // cleanly instead of retargeting or arming Take Control on a replacement.
        if let Some(want_inst) = expected_host_instance_id
            && want_inst != 0
            && item.host_instance_id != want_inst
        {
            return;
        }
        if let Some(want_started) = expected_started
            && item.started != Some(want_started)
        {
            return;
        }

        let chosen_tab = tab.unwrap_or(if item.is_running() { olo_screen_pane::Tab::Live } else { olo_screen_pane::Tab::History });
        self.ensure_screen_pane(chosen_tab, cx);
        let already_same = self.active_screen.as_ref().is_some_and(|s| s.id == item.id && s.bound_started == item.started && s.bound_host_instance_id == item.host_instance_id);
        let eligible_for_ctrl = take_control && item.is_running() && !item.host_disconnected && chosen_tab == olo_screen_pane::Tab::Live;
        let can_try_now = eligible_for_ctrl && self.window_active;
        // Neon #7: if `Take Control` was requested while `!self.window_active`
        // (e.g. `OLO_REX_DEMO_ACTIVITY=screens-control` or right-click menu in an
        // inactive window), arm `pending_take_control` so `window_activated(true)`
        // flushes it once via `request_control_when_ready`.
        let arm_for_activation = eligible_for_ctrl && !self.window_active;
        if already_same {
            let info = item.to_screen_info(show_location);
            let mut controlling_now = false;
            if let Some(st) = &self.active_screen
                && let Some(src) = &st.source
            {
                src.set_info(info.clone(), item.started);
            }
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| {
                    p.update_info(info, cx);
                    if let Some(t) = tab {
                        p.set_tab(t, cx);
                    }
                    p.set_visible(true, cx);
                    if can_try_now {
                        p.request_control_when_ready(cx);
                    }
                });
                controlling_now = pane.read(cx).controlling();
            }
            if let Some(st) = &mut self.active_screen {
                if let Some(t) = tab {
                    st.tab = t;
                }
                st.controlling = controlling_now;
                st.pending_take_control = arm_for_activation && !controlling_now;
            }
        } else {
            let Some((source, bound_host_conn, bound_host_instance_id, bound_caps)) = self.build_screen_source(&item, show_location) else {
                return;
            };
            let pane_src = source.as_dyn();
            let mut controlling_now = false;
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| {
                    p.set_source(Some(pane_src), cx);
                    p.set_tab(chosen_tab, cx);
                    p.set_visible(true, cx);
                    if can_try_now {
                        p.request_control_when_ready(cx);
                    }
                });
                controlling_now = pane.read(cx).controlling();
            }
            self.active_screen = Some(ActiveScreenState {
                id: item.id.clone(),
                tab: chosen_tab,
                controlling: controlling_now,
                pending_take_control: arm_for_activation && !controlling_now,
                bound_started: item.started,
                bound_host_conn,
                bound_host_instance_id,
                bound_caps,
                source: Some(source),
            });
        }
        if self.side_pane.alone() {
            self.side_pane.show_rest();
        }
        cx.invalidate();
    }

    /// Retry or expire `pending_restored_screen` once host/desktop discovery and
    /// session topology arrive (Astra #18 / Neon #10).
    fn try_restore_pending_screen(&mut self, cx: &mut olo_kit::Cx<Self>) -> bool {
        let Some((want_id, want_tab)) = self.pending_restored_screen.clone() else {
            return false;
        };
        let (found, expired) = {
            let hub = the_hub().clone();
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let resolved = self.resolved_screens_cached(&st);
            let exists = Self::lookup_verified_screen(&st, &resolved, &want_id).is_some();
            let is_expired = if exists {
                false
            } else {
                match &want_id.location {
                    olo_screen_pane::ScreenLocation::Local => {
                        let local_ready = st.hosts.values().find(|h| h.is_local).is_none_or(|h| h.conn_state != HostConnState::Connecting && h.sessions.len() >= h.session_summaries.len());
                        st.desktop_screens_loaded && local_ready
                    }
                    olo_screen_pane::ScreenLocation::Host { host_id, .. } => match st.hosts.get(host_id) {
                        None => true,
                        Some(h) => {
                            let snap_ready = st.host_screens.get(host_id).is_some_and(|l| l.has_follow_snapshot);
                            let sess_ready = h.conn_state == HostConnState::Connected && h.sessions.len() >= h.session_summaries.len();
                            snap_ready && sess_ready
                        }
                    },
                }
            };
            (exists, is_expired)
        };
        if found {
            self.pending_restored_screen = None;
            self.open_screen(want_id, Some(want_tab), false, None, None, cx);
            return true;
        }
        if expired {
            self.pending_restored_screen = None;
        }
        false
    }

    fn sync_active_screen_info(&mut self, cx: &mut olo_kit::Cx<Self>) {
        if self.try_restore_pending_screen(cx) {
            return;
        }

        let Some(cur_id) = self.active_screen.as_ref().map(|s| s.id.clone()) else { return };
        let (item_opt, show_loc, cur_host_conn, cur_host_inst, cur_host_caps) = {
            let hub = the_hub().clone();
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let resolved = self.resolved_screens_cached(&st);
            let show_loc = st.hosts.values().any(|h| !h.is_local);
            let (conn, inst, caps) = match cur_id.location.host_id() {
                Some(hk) => (
                    st.hosts.get(hk).map_or(0, |h| h.screens_conn_id),
                    st.host_screens.get(hk).map_or(0, |l| l.host_instance_id),
                    st.host_screens.get(hk).and_then(crate::activity_screens::host_capabilities_for_live),
                ),
                None => (0, 0, None),
            };
            // Reconcile against BOTH Activity's resolved inventory AND verified
            // raw stopped screens; if host was removed or screen was deleted /
            // archived, close the pane cleanly (Astra #9 / Neon #6).
            let item = Self::lookup_verified_screen(&st, &resolved, &cur_id);
            (item, show_loc, conn, inst, caps)
        };
        let Some(item) = item_opt else {
            self.close_screen(cx);
            return;
        };
        // Neon #3: when background `ensure_protocol` finishes (`bound_caps: None -> Some(caps)`)
        // on the EXACT same `(bound_started, bound_host_conn, bound_host_instance_id)`, update
        // `HostProtocolCache` and `ScreenInfo` in place without tearing down the live stream or
        // dropping Take Control!
        let caps_only_discovered = self.active_screen.as_ref().is_some_and(|s| {
            s.bound_started == item.started
                && !cur_id.is_local()
                && s.bound_host_conn == cur_host_conn
                && s.bound_host_instance_id == cur_host_inst
                && s.bound_caps.is_none()
                && cur_host_caps.is_some()
        });
        let host_instance_changed = self.active_screen.as_ref().is_some_and(|s| !cur_id.is_local() && s.bound_host_instance_id != cur_host_inst);
        let need_rebind = !caps_only_discovered
            && self.active_screen.as_ref().is_some_and(|s| {
                s.bound_started != item.started || (!cur_id.is_local() && (s.bound_host_conn != cur_host_conn || s.bound_host_instance_id != cur_host_inst || s.bound_caps != cur_host_caps))
            });
        if need_rebind {
            let Some((source, bound_host_conn, bound_host_instance_id, bound_caps)) = self.build_screen_source(&item, show_loc) else {
                self.close_screen(cx);
                return;
            };
            if let Some(pane) = &self.screen_pane {
                let pane_src = source.as_dyn();
                // Astra #16: drop cached Replay when a host instance is replaced
                // under the same host key so the old host's history is never shown
                // under the new host, while keeping cached Replay across reconnects
                // to the same host instance.
                if host_instance_changed {
                    pane.update(cx, |p, cx| p.set_source_fresh(Some(pane_src), cx));
                } else {
                    pane.update(cx, |p, cx| p.set_source(Some(pane_src), cx));
                }
            }
            if let Some(st) = &mut self.active_screen {
                st.bound_started = item.started;
                st.bound_host_conn = bound_host_conn;
                st.bound_host_instance_id = bound_host_instance_id;
                st.bound_caps = bound_caps;
                st.controlling = false;
                st.pending_take_control = false;
                st.source = Some(source);
            }
        } else {
            let info = item.to_screen_info(show_loc);
            if let Some(st) = &mut self.active_screen {
                if !item.is_running() || item.host_disconnected {
                    st.controlling = false;
                    st.pending_take_control = false;
                }
                if caps_only_discovered && let Some(caps) = cur_host_caps.clone() {
                    if let Some(src) = &st.source {
                        src.set_probed_caps(caps);
                    }
                    st.bound_caps = cur_host_caps;
                }
                if let Some(src) = &st.source {
                    src.set_info(info.clone(), item.started);
                }
            }
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| p.update_info(info, cx));
            }
        }
    }

    /// Leave the embedded Agent Screen pane: releases Take Control and ends its
    /// live/history streams immediately so host `olo-rex-link` sees EOF at once.
    fn close_screen(&mut self, cx: &mut olo_kit::Cx<Self>) {
        if let Some(st) = self.active_screen.take() {
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| {
                    p.stop_all(cx);
                    p.set_source(None, cx);
                });
            } else if st.controlling
                && let Some(src) = st.source
            {
                let _ = src.as_dyn().set_controlled(false);
            }
            cx.invalidate();
        }
    }

    /// Centralized exit from the embedded screen view (and any pending reload
    /// restore) for all navigation commands, notifications, and pane jumps
    /// (Astra #10 / Neon #4).
    fn exit_screen_for_navigation(&mut self, cx: &mut olo_kit::Cx<Self>) {
        self.pending_restored_screen = None;
        self.close_screen(cx);
    }

    /// Whether `cmd` is a Rex navigation/app shortcut retained while an embedded
    /// screen holds Take Control (Astra #11 / Neon #4).
    fn is_retained_screen_shortcut(cmd: &str) -> bool {
        matches!(
            cmd,
            "window.new"
                | "window.close"
                | "window.next"
                | "window.previous"
                | "app.toggle_sidebar"
                | "app.view_sessions"
                | "app.view_activity"
                | "app.hosts"
                | "app.settings"
                | "app.new_window"
                | "app.quit"
                | "quit"
                | "close"
                | "session.new"
                | "agent.jump_next_blocked"
        ) || cmd.starts_with("window.select_")
    }

    /// Capture-phase key router when an Agent Screen pane is active with Take
    /// Control on.
    ///
    /// Rex keeps only its own window/tab/app navigation shortcuts (plus
    /// `screen.control` ⌘K/⇧⌘K to release control); Olo's Command Palette is
    /// already intercepted by `app_server`. Every other key passes down to
    /// `ScreenPane`, which forwards it to the controlled screen.
    fn capture_screen_key(&mut self, e: &KeyDownEvent, cx: &mut olo_kit::Cx<Self>) {
        if self.role != WinRole::Workspace || !self.screen_controlling() {
            return;
        }
        let k = &e.keystroke;
        // 1. Ctrl+Tab / Ctrl+Shift+Tab / Ctrl+PageUp / Ctrl+PageDown: switch Rex tabs.
        let step = olo_kit::tab_step(k);
        if step != 0 && self.resolve_active_session().is_some_and(|(_, s)| s.windows.len() > 1) {
            self.command(if step > 0 { "window.next" } else { "window.previous" }, cx);
            cx.stop_propagation();
            return;
        }
        // 2. Rex's own window/tab/app shortcuts (dispatched via `self.command` so
        //    `"close"` / `"quit"` actually execute — Astra #11 / Neon #4).
        if keymap::command_held(k) || own_plain_shortcut(k) {
            let menus = self.build_menu_bar();
            olo_kit::menu::menus_beginning(cx, menus);
            if let Some(cmd) = olo_kit::menu::shortcut_command(cx, k)
                && Self::is_retained_screen_shortcut(&cmd)
            {
                self.command(&cmd, cx);
                cx.stop_propagation();
                return;
            }
        }
        // 3. `screen.control` (⌘K / ⇧⌘K or custom non-command override like Alt+K / F9):
        //    evaluated OUTSIDE the `keymap::command_held(k)` gate with physical-command
        //    distinction (so Left `Ctrl+Shift+K` passes through on Mac/BeOS while custom
        //    overrides still toggle control), and skipped while editing a text field
        //    inside ScreenPane (Astra #25 / Neon #2).
        let editing_pane_text = self.screen_pane.as_ref().is_some_and(|p| p.read(cx).is_editing_text(cx));
        if !editing_pane_text && olo_screen_pane::is_control_shortcut_with_command(k, true, keymap::command_held(k)) {
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| {
                    p.run_command("control", cx);
                });
            }
            cx.stop_propagation();
        }
    }

    fn confirm_stop_screen(&mut self, item: crate::activity_screens::ActivityScreenItem, cx: &mut olo_kit::Cx<Self>) {
        if let Err(reason) = item.can_stop_from_menu() {
            olo_kit::widgets::open_alert(cx, "Stop Screen", "stop", &reason, &["OK"], |_, _| {});
            return;
        }
        let is_local = item.id.is_local();
        // Astra #6 / Neon #2: fail closed immediately when local history could not
        // be verified (`last_used_ms == None && actions == None`); reserve `Some(0)`
        // strictly for verified-empty history (`actions == Some(0)`).
        let (expected_started, expected_last_used) = match olo_screen_pane::capture_stop_expectations_strict(is_local, item.started, None, item.last_used_ms, item.actions, item.actions == Some(0)) {
            Ok(v) => v,
            Err(msg) => {
                olo_kit::widgets::open_alert(cx, "Stop Screen", "stop", &msg, &["OK"], |_, _| {});
                return;
            }
        };
        let show_location = !is_local || the_hub().state.lock().unwrap_or_else(|e| e.into_inner()).hosts.values().any(|h| !h.is_local);
        let where_str = match &item.id.location {
            olo_screen_pane::ScreenLocation::Host { host_label, .. } => format!(" on {host_label}"),
            olo_screen_pane::ScreenLocation::Local if show_location => " on this computer".to_owned(),
            olo_screen_pane::ScreenLocation::Local => String::new(),
        };
        let message = format!(
            "Stop the screen “{}”{where_str}? The agent’s programs on it are closed, and anything it hasn’t saved is lost. The screen is kept, with its history: Start Screen starts it again.",
            item.display_title
        );
        let id = item.id.clone();
        let host_key = item.run_host_key.clone();
        let host_instance_id = item.host_instance_id;
        // Pin the row's remote connection ID (falling back to live hub only if unset)
        // so a stale Stop dialog never detaches control on a newer connection (Astra #1, #9).
        let conn_id = if is_local {
            None
        } else if item.conn_id != 0 {
            Some(item.conn_id)
        } else {
            the_hub().state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(&host_key).map(|h| h.screens_conn_id).filter(|&c| c != 0)
        };
        let pinned_conn_u64 = conn_id.unwrap_or(0);
        let name = item.id.name.clone();
        let win_id = self.id;
        let me = cx.view_ref();
        olo_kit::widgets::open_alert(cx, "Stop Screen", "warning", &message, &["Cancel", "Stop Screen"], move |choice, cx| {
            if *choice != Some(1) {
                return;
            }
            if crate::server::demo_mode() {
                let hub = the_hub().clone();
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                match &id.location {
                    olo_screen_pane::ScreenLocation::Local => {
                        if let Some(sc) = st.desktop_screens.iter_mut().find(|s| s.name == name) {
                            sc.state = olo_rex_link::screen_wire::HostScreenState::Stopped;
                        }
                    }
                    olo_screen_pane::ScreenLocation::Host { host_id, .. } => {
                        if let Some(live) = st.host_screens.get_mut(host_id)
                            && let Some(sc) = live.screens.iter_mut().find(|s| s.name == name)
                        {
                            sc.state = olo_rex_link::screen_wire::HostScreenState::Stopped;
                        }
                    }
                }
                st.screens_rev = st.screens_rev.wrapping_add(1);
                drop(st);
                broadcast_to_role(WinRole::Workspace, || WinMsg::ScreensMetaChanged);
                return;
            }
            // Astra #15 / Neon #4 / Astra #9: detach control synchronously ONLY
            // when the active pane still matches `(id, expected_started,
            // host_instance_id, pinned_conn_u64)` so a stale sidebar Stop dialog
            // never detaches control on a newer connection! And propagate any
            // `release_control_and_wait` timeout error before stopping (Astra #3).
            let id_c = id.clone();
            let host_key_c = host_key.clone();
            let name_c = name.clone();
            me.update(cx, move |w, cx| {
                let mut ctrl_release: Option<(Arc<dyn olo_screen_pane::ScreenSource>, Vec<olo_protocol::SyntheticEvent>)> = None;
                let mut fallback_src: Option<Arc<dyn olo_screen_pane::ScreenSource>> = None;
                if let Some(st) = &mut w.active_screen
                    && st.id == id_c
                    && st.bound_started == expected_started
                    && st.bound_host_instance_id == host_instance_id
                    && (id_c.is_local() || (pinned_conn_u64 != 0 && st.bound_host_conn == pinned_conn_u64))
                {
                    st.controlling = false;
                    st.pending_take_control = false;
                    fallback_src = st.source.as_ref().map(BoundScreenSource::as_dyn);
                    if let Some(pane) = &w.screen_pane {
                        ctrl_release = pane.update(cx, |p, cx| p.detach_control_for_stop(&id_c, expected_started, cx));
                    }
                    cx.invalidate();
                }
                let hub = the_hub().clone();
                std::thread::spawn(move || {
                    let release_res = if let Some((src, up)) = ctrl_release {
                        src.release_control_and_wait(up)
                    } else if let Some(src) = fallback_src {
                        src.release_control_and_wait(Vec::new())
                    } else {
                        Ok(())
                    };
                    let res = match release_res {
                        Err(err) => Err(err),
                        Ok(()) if id_c.is_local() => olo_screen_pane::screens::stop_if_expected(&name_c, expected_started, expected_last_used),
                        Ok(()) => hub.rt_handle().block_on(hub.screens_service().stop_screen_for_host_instance_expected_pinned(
                            &host_key_c,
                            host_instance_id,
                            conn_id,
                            &name_c,
                            expected_started,
                            expected_last_used,
                        )),
                    };
                    // Never hold `wins()` across `Feedback::start(win_id)`.
                    if let Err(err) = res {
                        let fb = Feedback::start(win_id);
                        fb.problem(&format!("Couldn’t stop “{name_c}”."), err);
                    }
                });
            });
        });
    }

    fn start_screen_from_menu(&mut self, item: crate::activity_screens::ActivityScreenItem, _cx: &mut olo_kit::Cx<Self>) {
        if item.is_running() || item.host_disconnected {
            return;
        }
        let id = item.id.clone();
        let host_key = item.run_host_key.clone();
        let host_instance_id = item.host_instance_id;
        let name = item.id.name.clone();
        let win_id = self.id;
        if crate::server::demo_mode() {
            let hub = the_hub().clone();
            let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            match &id.location {
                olo_screen_pane::ScreenLocation::Local => {
                    if let Some(sc) = st.desktop_screens.iter_mut().find(|s| s.name == name) {
                        sc.state = olo_rex_link::screen_wire::HostScreenState::Running;
                        sc.width = 1440;
                        sc.height = 900;
                    }
                }
                olo_screen_pane::ScreenLocation::Host { host_id, .. } => {
                    if let Some(live) = st.host_screens.get_mut(host_id)
                        && let Some(sc) = live.screens.iter_mut().find(|s| s.name == name)
                    {
                        sc.state = olo_rex_link::screen_wire::HostScreenState::Running;
                        sc.width = 1440;
                        sc.height = 900;
                    }
                }
            }
            st.screens_rev = st.screens_rev.wrapping_add(1);
            drop(st);
            broadcast_to_role(WinRole::Workspace, || WinMsg::ScreensMetaChanged);
            return;
        }
        let hub = the_hub().clone();
        std::thread::spawn(move || {
            let res = if id.is_local() {
                let opts = olo_screen_pane::screens::kept(&name).unwrap_or_default();
                olo_screen_pane::screens::start(&name, &opts).map(|_| ())
            } else {
                // Pins `host_instance_id`, sends `["start", name, "--keep"]`, and
                // validates returned screen identity (Astra #3, #8 / Neon #2).
                hub.rt_handle().block_on(hub.screens_service().start_screen_for_host_instance(&host_key, host_instance_id, &name)).map(|_| ())
            };
            if let Err(err) = res {
                let fb = Feedback::start(win_id);
                fb.problem(&format!("Couldn’t start “{name}”."), err);
            }
        });
    }

    /// Shared height of the screen breadcrumb strip (Neon #11.1): `9 pt` top +
    /// `24 pt` control row + `9 pt` bottom = `42 pt`, aligning the breadcrumb text
    /// baseline and 24 pt buttons to the exact pixel with the sidebar's `pt(9)`
    /// `SearchField` beside it.
    const SCREEN_BREADCRUMB_H: f32 = 42.0;

    /// Top breadcrumb strip (`40 pt`, matching `ScreenPane`'s `16 pt` outer inset
    /// and the sidebar's top search band) rendered directly above the embedded
    /// `ScreenPane`:
    /// - Left: clickable ancestor breadcrumb trail (`Lead › builder-1`)
    /// - Right: `Open in Agent Screens` (folded into `ScreenPane`'s `Screen ▾`
    ///   menu when narrow rather than renamed) and `Show Terminal`, uniform 24 pt
    ///   buttons with 8 pt gap (Opus #1, #2, #3 / Neon #12).
    fn render_screen_breadcrumb_strip(&self, screen_id: &olo_screen_pane::ScreenId, resolved: &crate::activity_screens::ResolvedActivityScreens, avail_w: f32, cx: &mut olo_kit::Cx<Self>) -> Element {
        enum CrumbTarget {
            Window(String, String, String),
            Block(BlockKey),
            Host(String),
            Plain,
        }

        let (ancestors, owner_bk) = {
            let st = the_hub().state.lock().unwrap_or_else(|e| e.into_inner());
            let item = resolved.by_id.get(screen_id);
            let owner = item.and_then(|i| i.owner_bk.clone());
            let mut crumbs: Vec<(String, String, CrumbTarget)> = Vec::new();
            if let Some(ref bk) = owner {
                // Walk up from owning pane through any subagent parents to the top-level tab.
                if let Some(h) = st.hosts.get(&bk.host_key)
                    && let Some(sess) = h.sessions.get(&bk.session_id)
                {
                    let names = crate::hub::session_tab_names(&st, &bk.host_key, sess);
                    let live: HashSet<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
                    if let Some(sub) = st.subagents.by_block(&bk.host_key, Some(&bk.session_id), &bk.block_id) {
                        crumbs.push((format!("blk-{}", bk.block_id), sub.display_title(), CrumbTarget::Block(bk.clone())));
                        let mut cur = sub.parent_window_id.clone();
                        let mut seen = HashSet::new();
                        seen.insert(sub.window_id.clone());
                        while seen.insert(cur.clone()) {
                            if let Some(p_sub) = st.subagents.by_window(&bk.host_key, &bk.session_id, &cur) {
                                let next = p_sub.parent_window_id.clone();
                                crumbs.push((format!("win-{cur}"), p_sub.display_title(), CrumbTarget::Window(bk.host_key.clone(), bk.session_id.clone(), cur.clone())));
                                if live.contains(&next) {
                                    cur = next;
                                    continue;
                                }
                                break;
                            } else if let Some(widx) = sess.windows.iter().position(|w| w.window_id == cur) {
                                let t = names.get(widx).cloned().unwrap_or_else(|| "Tab".into());
                                crumbs.push((format!("win-{cur}"), t, CrumbTarget::Window(bk.host_key.clone(), bk.session_id.clone(), cur.clone())));
                                break;
                            } else {
                                break;
                            }
                        }
                        crumbs.reverse();
                    } else if let Some(widx) = sess.windows.iter().position(|w| w.all_blocks().any(|b| b.block_id == bk.block_id)) {
                        let t = names.get(widx).cloned().unwrap_or_else(|| "Tab".into());
                        crumbs.push((format!("blk-{}", bk.block_id), t, CrumbTarget::Block(bk.clone())));
                    }
                }
            } else {
                // Standalone screen: show its host as clickable leading breadcrumb,
                // ONLY targeting an actual `is_local` host for desktop screens so an
                // SSH home server keyed `"local"` is never mis-selected (Astra #19 / Neon #5).
                match &screen_id.location {
                    olo_screen_pane::ScreenLocation::Local => {
                        if let Some(local_hk) = st.host_order.iter().find(|k| st.hosts.get(*k).is_some_and(|h| h.is_local)) {
                            let lbl = st.hosts.get(local_hk).map(|h| h.label.clone()).unwrap_or_else(|| "This computer".to_owned());
                            crumbs.push((format!("host-{local_hk}"), lbl, CrumbTarget::Host(local_hk.clone())));
                        } else {
                            crumbs.push(("desktop".to_owned(), "This computer".to_owned(), CrumbTarget::Plain));
                        }
                    }
                    olo_screen_pane::ScreenLocation::Host { host_id, host_label } => {
                        let lbl = st.hosts.get(host_id).map(|h| h.label.clone()).unwrap_or_else(|| host_label.clone());
                        crumbs.push((format!("host-{host_id}"), lbl, CrumbTarget::Host(host_id.clone())));
                    }
                }
            }
            (crumbs, owner)
        };

        let head_bg = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
            let edge = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.14)) } else { style::shade(&p, p.panel_background, 1.28) };
            style::hline_above(g, b.left(), b.right(), b.bottom(), edge);
        })
        .paint_key(915)
        .absolute()
        .inset_0();

        const SEP_W: f32 = 16.0;
        let hdr_h = Self::SCREEN_BREADCRUMB_H;
        let row_h = m::CONTROL_HEIGHT;
        let mut crumb = div().flex_1().min_w_0().flex().flex_row().items_center();
        let n_ancestors = ancestors.len();
        for (idx, (seg_key, label, target)) in ancestors.into_iter().enumerate() {
            let is_last_crumb = idx + 1 == n_ancestors;
            let clickable = !matches!(target, CrumbTarget::Plain);
            let nav = match target {
                CrumbTarget::Window(hk, sid, wid) => Some(cx.msg(WinMsg::SelectWindow(hk, sid, wid))),
                CrumbTarget::Block(bk) => Some(cx.msg(WinMsg::FocusBlock(bk))),
                CrumbTarget::Host(hk) => Some(cx.msg(WinMsg::SelectHost(hk))),
                CrumbTarget::Plain => None,
            };
            let link_label = label.clone();
            let link_w = f32::from(style::text_width(label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil().min(160.0);
            let mut seg = canvas(move |g, b| {
                let hovered = clickable && g.node_state::<std::cell::Cell<bool>>().get();
                let p = theme::palette();
                if hovered {
                    let dark = p.mode == olo_theme::ThemeMode::Dark;
                    let pill = Bounds::from_corners(point(b.left() - px(5.0), b.top()), point(b.right() + px(5.0), b.bottom()));
                    let snapped = style::snap(g, pill);
                    let fill_c = if dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.08)) } else { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.35)) };
                    let shine = if dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.22)) } else { style::c(p.shine) };
                    let shadow = if dark { style::c(olo_theme::mix(p.panel_background, [0, 0, 0, 255], 0.45)) } else { style::c(olo_theme::mix(p.panel_background, [0, 0, 0, 255], 0.24)) };
                    style::fill_rect(g, snapped, fill_c);
                    style::bevel(g, snapped, shine, shadow);
                }
                let ink = if hovered || is_last_crumb { style::c(p.panel_text) } else { style::secondary_text(&p) };
                style::label_fit(g, b.left(), b.center().y, style::Align::Left, link_label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, b.size.width);
                if clickable {
                    g.agent(agent::Node::button(format!("Switch to {link_label}"), b));
                } else {
                    g.agent(agent::Node::label(link_label.clone(), b));
                }
            })
            .id(olo_kit::SharedString::from(format!("rex-scr-crumb-{seg_key}")))
            .w(px(link_w))
            .h(px(row_h))
            .flex_shrink_0();
            if let Some(nav) = nav {
                seg = seg
                    .cursor(CursorStyle::PointingHand)
                    .on_hover(|over: &bool, cx| {
                        let cell = cx.node_state::<std::cell::Cell<bool>>();
                        if cell.get() != *over {
                            cell.set(*over);
                            cx.repaint();
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |_, cx| {
                        nav(&(), cx);
                        cx.stop_propagation();
                    });
            }
            crumb = crumb.child(seg);
            if !is_last_crumb {
                let sep = canvas(|g, b| {
                    let p = theme::palette();
                    style::label(g, b.center().x, b.center().y, style::Align::Center, "›", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                })
                .paint_key(916)
                .w(px(SEP_W))
                .h(px(row_h))
                .flex_shrink_0();
                crumb = crumb.child(sep);
            }
        }

        let open_id = screen_id.clone();
        let win_id = self.id;
        // Keep full label `"Open in Agent Screens"` — never rename to `"Agent Screens"`;
        // on narrow panes (`avail_w < 460 pt`, e.g. `tile-v-third`), fold it into
        // `Screen ▾` and move `Screen ▾` up into this breadcrumb bar right of
        // `Show Terminal` (eliminating the wrapped button row above tabs — Opus #3).
        let show_open_in_strip = avail_w >= 460.0;
        let open_btn = show_open_in_strip.then(|| {
            Button::new(olo_kit::words::tr("Open in Agent Screens").to_owned()).id("rex-screen-open-app").min_width(0.0).on_click(move |_, _| {
                if let Err(err) = crate::activity_screens::open_in_agent_screens(&open_id) {
                    Feedback::start(win_id).problem("Couldn’t open Agent Screens.", err);
                }
            })
        });
        let show_term_cb = match owner_bk {
            Some(bk) => cx.msg(WinMsg::FocusBlock(bk)),
            None => cx.msg(WinMsg::CloseScreen),
        };
        let term_btn = Button::new(olo_kit::words::tr("Show Terminal").to_owned()).id("rex-screen-show-terminal").min_width(0.0).on_click(move |_, cx| show_term_cb(&(), cx));
        let screen_menu_btn = (!show_open_in_strip).then(|| {
            Button::new(olo_kit::words::tr("Screen").to_owned())
                .id("rex-screen-crumb-menu")
                .menu()
                .min_width(0.0)
                .on_click(cx.msg_with(|ev: &olo_kit::ClickEvent| WinMsg::OpenScreenMenu(ev.position())))
        });

        // Match ScreenPane's exact `m::WINDOW_INSET` (16 pt) outer padding so the
        // leading breadcrumb aligns flush with the monitor icon/tab card and
        // rightmost button aligns flush with the tab card right edge; `pt(9)` +
        // `24 pt` controls + `pb(9)` matches the sidebar SearchField's exact
        // vertical band so baselines and control tops align to 0 px (Opus Minor).
        div()
            .relative()
            .w_full()
            .h(px(hdr_h))
            .flex_shrink_0()
            .px(px(m::WINDOW_INSET))
            .pt(px(9.0))
            .pb(px(9.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(head_bg)
            .child(crumb)
            .child(div().flex_shrink_0().flex().flex_row().items_center().gap(px(8.0)).children(open_btn).child(term_btn).children(screen_menu_btn))
            .into_any_element()
    }

    fn feedback_line(&mut self, cx: &mut olo_kit::Cx<Self>) -> Option<Element> {
        self.due.at(self.notice.until(), cx, |_, cx| cx.invalidate());
        if let Some((title, detail)) = &self.problem {
            let p = theme::palette();
            let scale = cx.scale_factor();
            let frame = 4.0 * style::line_pixels(scale) / scale;
            let mono_size = style::SMALL_SIZE - 1.0;
            // (Darken light-mode failure red enough to clear 4.5:1 on the panel
            // and keep dark mode crisp: Opus & Gemini reviews, 2026-11-25.)
            let err_red = style::c(olo_theme::readable(p.failure, p.panel_background, 4.8));
            let row_h = style::line_row(style::PLAIN_SIZE);
            let alert_icon = canvas(move |g, b| {
                let cap_mid = b.top() + px(style::paragraph_caps_middle(style::PLAIN_SIZE));
                let icon_b = style::snap_centred(g, point(b.left() + px(8.0), cap_mid), 16.0, 16.0);
                icons::paint(g, icon_b, "stop");
            })
            .paint_key(911)
            .w(px(16.0))
            .h(px(row_h))
            .flex_shrink_0();
            let dismiss = GlyphButton::new("Dismiss Error", Glyph::Close).id("rex-dismiss-problem").on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::DismissFeedback));
            // (On the problem's own row, beside what went wrong: the one
            // thing to do about it. Where there is a detail, beside its
            // well; else at the head's end.)
            let again = (self.retrying || self.retry.as_ref().is_some_and(|(serial, _)| *serial == self.feedback_serial))
                .then(|| Button::new(olo_kit::words::tr("Try Again").to_owned()).id("rex-problem-again").enabled(!self.retrying).on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::TryAgain)));
            // (Beside a detail, a button of the desktop's own size, as tall
            // as the well's first line so their edges meet; at the head's
            // end, where a row of text sets the height, a small one.)
            let (again_head, again_detail) = if detail.is_some() { (None, again) } else { (again.map(Button::small), None) };
            let header = div()
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(m::TIGHT))
                .child(div().flex_1().min_w_0().child(StringView::new(title.clone()).bold(true).color(err_red).wrap(true)))
                .children(again_head.map(|b| div().flex_shrink_0().mt(px(-2.0)).child(b)))
                .child(div().flex_shrink_0().mt(px(-1.0)).child(dismiss));
            let col = div().flex_1().min_w_0().flex().flex_col().gap(px(8.0)).child(header).children(detail.as_ref().map(|(view, lines)| {
                // Hug the detail text width (capped to the banner width) so a short
                // one-line error doesn't stretch into a 1000 pt empty text-field trough.
                let text = view.read(cx).text().to_owned();
                let mono = style::mono_font(FontWeight::NORMAL);
                let max_line_w = text.lines().map(|l| f32::from(style::text_width(l.to_owned(), mono_size, mono.clone()))).fold(0.0_f32, f32::max);
                // (The same room after its last word as before its first:
                // with ten points to spare its right side was twice its
                // left, plain once a button stood beside it.)
                let hug_w = (max_line_w + 2.0 * widgets::TextView::MARGINS.0 + frame + 2.0).ceil().max(160.0);
                let line_h = widgets::TextView::line_height_at(mono_size) + 2.0 * widgets::TextView::MARGINS.1 + frame;
                let well = div().flex_1().min_w_0().max_w(px(hug_w)).h(px(((*lines - 1) as f32) * widgets::TextView::line_height_at(mono_size) + line_h)).child(view);
                // (Try Again at the banner's right edge, under the close
                // box, wherever the detail's words end: the banner's two
                // edges, what went wrong at the left and what to do at
                // the right, as an alert's buttons are (both looks of
                // 2026-11-26: beside the well it stood "wherever the
                // error text happens to end"). As tall as the well's first
                // line, so their tops and, for one line, their feet meet.)
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(m::SPACING))
                    .child(well)
                    .child(div().flex_1().min_w_0())
                    .children(again_detail.map(|b| div().flex_shrink_0().child(b.height(line_h))))
            }));
            return Some(div().w_full().flex().flex_row().items_start().gap(px(m::TIGHT)).child(alert_icon).child(col));
        }
        self.notice.text().map(|s| div().child(StringView::new(s.clone()).size(style::SMALL_SIZE).height(m::LABEL_ROW)))
    }

    fn new_workspace(id: WinId, restored: Option<SavedWorkspaceState>, cx: &mut olo_kit::Cx<Self>) -> Self {
        let (side_w, side_col) = {
            let hub = the_hub().clone();
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

        let activity_search_field = widgets::text_field(cx, |f| f.name("Filter Activity").hint("Filter agents & screens…"));
        cx.subscribe(&activity_search_field, |_w: &mut Self, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed | TextFieldEvent::Entered => cx.invalidate(),
            TextFieldEvent::Typed(_) => {}
        });

        let panes_view = cx.new_view(|_| PanesView { win_id: id, session: None, width: None, area: Rc::new(std::cell::Cell::new(olo_kit::Size::default())) });

        // (The demo's other windows, opened as it starts, for the live
        // lint and for pictures: `OLO_REX_DEMO_OPEN=app.hosts`, the
        // commands that open them, with commas between. The lint
        // reviews the windows an app shows; shown none of these, it
        // passed a row of buttons in Hosts and Ports that was wider
        // than its window, 2026-11-23.)
        if std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1") && restored.is_none() {
            if let Ok(open) = std::env::var("OLO_REX_DEMO_OPEN") {
                for command in open.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                    cx.send(WinMsg::Command(command.to_owned()));
                }
            }
            // (`OLO_REX_DEMO_ERROR=1`: a problem that can be tried again;
            // `=retrying`: its Try Again pressed and the try not yet
            // answered, for the dimmed button's picture.)
            let demo_error = std::env::var("OLO_REX_DEMO_ERROR").ok();
            if let Some(how) = demo_error.as_deref().filter(|h| matches!(*h, "1" | "retrying")) {
                let with = if how == "retrying" { WinMsg::Redraw } else { WinMsg::CreateSession("dev-box".into(), None) };
                cx.send(WinMsg::StartFeedback(999));
                cx.send(WinMsg::RetryWith(999, Box::new(with)));
                cx.send(WinMsg::Problem(999, "Couldn’t open a session on dev-box.".into(), Some("ssh: connect to host dev-box port 22: Connection timed out".into())));
                if how == "retrying" {
                    cx.send(WinMsg::TryAgain);
                }
            }
        }

        let pending_restored_screen = restored.as_ref().and_then(|r| r.active_screen.clone().map(|id| (id, r.active_screen_tab.unwrap_or_default())));
        if pending_restored_screen.is_some() {
            cx.send(WinMsg::ScreensMetaChanged);
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
            last_transcript_ptr: 0,
            last_transcript_gen: 0,
            activity_search_field: Some(activity_search_field),
            activity_type_filter: ActivityTypeFilter::All,
            activity_state_filter: ActivityStateFilter::All,
            active_screen: None,
            screen_pane: None,
            resolved_screens_cache: None,
            pending_restored_screen,
            context_menu_row: None,
            sidebar_row_bounds: Rc::new(std::cell::RefCell::new(HashMap::new())),
            open_tree_tab: None,
            created_session: None,
            qr_url: None,
            qr: None,
            preview_job: None,
            feedback_serial: 0,
            retry: None,
            retrying: false,
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
            host_screens_list: widgets::ListModel::new(false),
            host_screen_lines: Rc::default(),
            host_screen_chosen: None,
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
                let is_secret = the_askpass().current_request().is_some_and(|r| matches!(r, AskpassRequest::Secret { echo: false, .. }));
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
            let was = crate::repos::editing().and_then(|i| the_hub().state.lock().unwrap_or_else(|e| e.into_inner()).repos.get(i).cloned()).unwrap_or_default();
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
            last_transcript_ptr: 0,
            last_transcript_gen: 0,
            activity_search_field: None,
            activity_type_filter: ActivityTypeFilter::All,
            activity_state_filter: ActivityStateFilter::All,
            active_screen: None,
            screen_pane: None,
            resolved_screens_cache: None,
            pending_restored_screen: None,
            context_menu_row: None,
            sidebar_row_bounds: Rc::new(std::cell::RefCell::new(HashMap::new())),
            open_tree_tab: None,
            created_session: None,
            qr_url: None,
            qr: None,
            preview_job: None,
            feedback_serial: 0,
            retry: None,
            retrying: false,
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
            host_screens_list: widgets::ListModel::new(false),
            host_screen_lines: Rc::default(),
            host_screen_chosen: None,
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
        let hub = the_hub().clone();
        let host = self.focused_block_key().map_or_else(|| self.active_host.clone(), |focused| focused.host_key);
        hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_has(&host)
    }

    fn silent_host(&mut self) -> Option<String> {
        let hub = the_hub().clone();
        let focused = self.focused_block_key()?;
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(&focused.host_key)?;
        Some(crate::hub::silent_said(&host.label, host.is_local, st.host_silent(&focused.host_key)?))
    }

    fn where_it_is(&mut self, resolved_opt: Option<&Arc<crate::activity_screens::ResolvedActivityScreens>>, cx: &olo_kit::Cx<Self>) -> String {
        if let Some(scr_id) = self.active_screen.as_ref().map(|s| s.id.clone()) {
            let hub = the_hub().clone();
            let (place, item_opt) = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                let resolved = match resolved_opt {
                    Some(r) => r.clone(),
                    None => self.resolved_screens_cached(&st),
                };
                let place = match &scr_id.location {
                    olo_screen_pane::ScreenLocation::Local => olo_kit::words::tr("This computer").to_owned(),
                    olo_screen_pane::ScreenLocation::Host { host_id, host_label } => st.hosts.get(host_id).map(|h| h.label.clone()).unwrap_or_else(|| host_label.clone()),
                };
                (place, Self::lookup_verified_screen(&st, &resolved, &scr_id))
            };
            let mut parts: Vec<String> = Vec::with_capacity(3);
            parts.push(place);
            if let Some(item) = item_opt {
                if item.host_disconnected {
                    parts.push(olo_kit::words::tr("Disconnected").to_owned());
                } else if item.is_running() {
                    if let Some((w, h)) = item.size
                        && w > 0
                        && h > 0
                    {
                        parts.push(format!("{w} × {h}"));
                    }
                    // Show Live fit scale (non-redundant info) instead of repeating
                    // "You have control" (Opus #5 & #12).
                    if let Some(pane) = &self.screen_pane
                        && pane.read(cx).shown_tab() == olo_screen_pane::Tab::Live
                        && let Some(ratio) = pane.read(cx).live_fit_scale()
                    {
                        let pct = (ratio * 100.0).round().clamp(1.0, 400.0) as u32;
                        parts.push(format!("{pct}%"));
                    }
                } else {
                    parts.push(olo_kit::words::tr("Stopped").to_owned());
                }
            }
            return parts.join(" · ");
        }
        if let Some(tr) = crate::activity_watch::active_transcript() {
            let g = tr.lock().unwrap_or_else(|e| e.into_inner());
            return g.target.status_bar_text(g.last_timestamp());
        }
        let hub = the_hub().clone();
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
        let hub = the_hub().clone();
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
        self.preview_job = the_hub().quick_look_path(&bk, path, Feedback::start(self.id));
    }

    fn dismiss_overlay(&mut self, cx: &mut olo_kit::Cx<Self>) {
        self.cancel_preview();
        if let Some(ActiveOverlay::RexPrompt { host_key, req, .. }) = self.overlay.take() {
            the_hub().respond_prompt(&host_key, &req.call_id, true, None);
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
                the_hub().respond_prompt(&host_key, &req.call_id, false, Some(val));
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
                the_hub().respond_prompt(&hk, &cid, false, Some(json!({ "id": id })));
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
                let hub = the_hub().clone();
                hub.select_window(&host_key, &session_id, &window_id);
            }
            JumpTarget::Pane(bk) => {
                self.active_host = bk.host_key.clone();
                self.active_session = Some(bk.session_id.clone());
                let hub = the_hub().clone();
                hub.focus_block(&bk.host_key, &bk.session_id, &bk.block_id);
            }
            JumpTarget::Directory(dir) => {
                if let Some(bk) = self.focused_block_key() {
                    the_hub().send_input(&bk, format!("cd {}\r", crate::hub::shell_quote(&dir)).into_bytes());
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
            let hub = the_hub().clone();
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
            let hub = the_hub().clone();
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
        let hub = the_hub().clone();
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
        let repos = the_hub().state.lock().unwrap_or_else(|e| e.into_inner()).repos.clone();
        let editing = crate::repos::editing();
        repos.into_iter().enumerate().filter(|(i, _)| Some(*i) != editing).map(|(_, repo)| repo).collect()
    }

    fn submit_forward(&mut self, cx: &mut olo_kit::Cx<Self>) {
        let port = self.fwd_port_field.as_ref().and_then(|f| f.read(cx).text().trim().parse::<u16>().ok()).filter(|p| *p > 0);
        let from = self.forward_hosts();
        let key = self.fwd_host.clone().filter(|k| from.iter().any(|(key, _)| key == k)).or_else(|| from.first().map(|(k, _)| k.clone()));
        let (Some(port), Some(key)) = (port, key) else { return };
        let hub = the_hub().clone();
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
        let hub = the_hub().clone();
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
            let hub = the_hub().clone();
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
            let hub = the_hub().clone();
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
            let hub = the_hub().clone();
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
        the_askpass().answer(AskpassReply::Secret(Some(text)));
        cx.request_close();
    }

    /// Execute any of the ~105 Rex actions cleanly.
    pub fn execute_action(&mut self, action: &str, args: Option<Value>, cx: &mut olo_kit::Cx<Self>) {
        let hub = the_hub().clone();
        match action {
            "app.view_activity" => {
                {
                    let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.settings.sidebar_mode = SidebarMode::Activity;
                    st.settings.sidebar_collapsed = false;
                    st.settings.save();
                }
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
                self.side_pane.show_pane();
                cx.invalidate();
            }
            "pane.view:transcript" => {
                self.exit_screen_for_navigation(cx);
                if let Some(bk) = self.focused_block_key() {
                    let title = pane_name(&hub, &bk);
                    crate::activity_watch::open_pane_transcript(&hub, &bk, title);
                }
            }
            "pane.view:terminal" => {
                self.exit_screen_for_navigation(cx);
                crate::activity_watch::close_viewer();
            }
            c if c.starts_with("activity.open.transcript:") => {
                let id = c.trim_start_matches("activity.open.transcript:");
                let found = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.subagents
                        .entries
                        .iter()
                        .find(|e| e.id == id || e.window_id == id || e.block_id == id)
                        .map(|e| (BlockKey { host_key: e.host_key.clone(), session_id: e.session_id.clone(), block_id: e.block_id.clone() }, e.display_title()))
                };
                if let Some((bk, title)) = found {
                    self.exit_screen_for_navigation(cx);
                    self.jump_to(JumpTarget::Pane(bk.clone()), cx);
                    crate::activity_watch::open_pane_transcript(&hub, &bk, title);
                }
            }
            c if c.starts_with("activity.open.terminal:") || c.starts_with("activity.open:") => {
                let id = c.strip_prefix("activity.open.terminal:").or_else(|| c.strip_prefix("activity.open:")).unwrap_or("");
                let found = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.subagents.entries.iter().find(|e| e.id == id || e.window_id == id || e.block_id == id).map(|e| BlockKey {
                        host_key: e.host_key.clone(),
                        session_id: e.session_id.clone(),
                        block_id: e.block_id.clone(),
                    })
                };
                if let Some(bk) = found {
                    self.exit_screen_for_navigation(cx);
                    crate::activity_watch::close_viewer();
                    self.jump_to(JumpTarget::Pane(bk), cx);
                }
            }
            c if c.starts_with("activity.fold:") => {
                let bid = c.trim_start_matches("activity.fold:");
                let found = {
                    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.blocks.keys().find(|bk| bk.block_id == bid).cloned()
                };
                if let Some(bk) = found {
                    crate::activity_watch::toggle_fold(&bk);
                    cx.invalidate();
                }
            }
            c if c.starts_with("activity.filter.query:") => {
                let q = c.trim_start_matches("activity.filter.query:");
                if let Some(f) = &self.activity_search_field {
                    f.update(cx, |tf, cx| tf.set_text(q.to_owned(), cx));
                }
                cx.invalidate();
            }
            c if c.starts_with("activity.filter.type:") => {
                let t = c.trim_start_matches("activity.filter.type:");
                self.activity_type_filter = ActivityTypeFilter::parse(t);
                cx.invalidate();
            }
            c if c.starts_with("activity.filter.state:") => {
                let s = c.trim_start_matches("activity.filter.state:");
                self.activity_state_filter = ActivityStateFilter::parse(s);
                cx.invalidate();
            }
            "activity.filter.clear" => {
                if let Some(f) = &self.activity_search_field {
                    f.update(cx, |tf, cx| tf.set_text(String::new(), cx));
                }
                self.activity_type_filter = ActivityTypeFilter::All;
                self.activity_state_filter = ActivityStateFilter::All;
                cx.invalidate();
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
                    begin_tweaks(&hub);
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
            // (One of Olo's things put on a host, updated, or taken off, for
            // agents: `host.put:KEY:helper|skill|commands|pool|screens`,
            // `host.update:…`, `host.take:…`, `host.update-all:KEY`. In the Host
            // window, where it shows. And where an agent's screen opens there:
            // `host.screens:KEY:host|desktop`; one of the host's screens
            // looked at or stopped: `host.screen:KEY:look|stop:NAME`; Olo
            // being put there for its screens, stopped: `host.cancel:KEY:screens`.)
            c if c.starts_with("host.screen:") => {
                let mut parts = c.trim_start_matches("host.screen:").rsplitn(3, ':');
                if let (Some(name), Some(what @ ("look" | "stop")), Some(key)) = (parts.next(), parts.next(), parts.next()) {
                    open_host_with_tab(&hub, key, Some(3));
                    if let Some(shown) = crate::host::current().filter(|shown| shown.key == key) {
                        if what == "stop" {
                            let expected_started = hub
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .hosts
                                .get(key)
                                .and_then(|h| h.screens.as_ref())
                                .and_then(|s| s.rows.iter().find(|r| r.name == name && r.running))
                                .and_then(|r| r.started);
                            crate::host::stop_screen(&hub, &shown.key, shown.host_instance_id, name, expected_started);
                        } else {
                            let expected_started = hub
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .hosts
                                .get(key)
                                .and_then(|h| h.screens.as_ref())
                                .and_then(|s| s.rows.iter().find(|r| r.name == name && r.running))
                                .and_then(|r| r.started);
                            open_host_screen_live_in_rex(&hub, &shown.key, shown.host_instance_id, &shown.name, name, expected_started);
                        }
                    }
                }
            }
            c if c.starts_with("host.cancel:") => {
                if let Some((key, "screens")) = c.trim_start_matches("host.cancel:").rsplit_once(':') {
                    open_host(&hub, key);
                    if crate::host::current().is_some_and(|shown| shown.key == key) {
                        crate::host::cancel_screens(&hub);
                    }
                }
            }
            c if c.starts_with("host.screens:") => {
                let rest = c.trim_start_matches("host.screens:");
                if let Some((key, word @ ("host" | "desktop"))) = rest.rsplit_once(':') {
                    open_host(&hub, key);
                    if crate::host::current().is_some_and(|shown| shown.key == key) {
                        crate::host::show_tab(3);
                        crate::host::screens_default(&hub, word == "host");
                    }
                }
            }
            c if c.starts_with("host.put:") || c.starts_with("host.update:") || c.starts_with("host.take:") => {
                let op = if c.starts_with("host.put:") {
                    crate::hub::HostOp::Install
                } else if c.starts_with("host.update:") {
                    crate::hub::HostOp::Update
                } else {
                    crate::hub::HostOp::Remove
                };
                let rest = c.split_once(':').map_or("", |(_, rest)| rest);
                let thing = rest.rsplit_once(':').and_then(|(key, thing)| {
                    let thing = match thing {
                        "helper" => crate::hub::HostThing::Helper,
                        "skill" => crate::hub::HostThing::Skill,
                        "commands" => crate::hub::HostThing::Commands,
                        "pool" => crate::hub::HostThing::Pool,
                        "screens" => crate::hub::HostThing::Screens,
                        _ => return None,
                    };
                    Some((key.to_owned(), thing))
                });
                if let Some((key, thing)) = thing {
                    open_host(&hub, &key);
                    if crate::host::current().is_some_and(|shown| shown.key == key) {
                        crate::host::change(&hub, thing, op);
                    }
                }
            }
            c if c.starts_with("host.update-all:") => {
                let key = c.trim_start_matches("host.update-all:");
                open_host(&hub, key);
                if let Some(shown) = crate::host::current().filter(|shown| shown.key == key)
                    && let Some(stands) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_stands(key)
                {
                    let behind: Vec<crate::hub::HostThing> = [crate::hub::HostThing::Helper, crate::hub::HostThing::Commands, crate::hub::HostThing::Skill, crate::hub::HostThing::Pool]
                        .into_iter()
                        .filter(|&t| {
                            let s = crate::host_panel::thing_stands(&stands, &shown, t);
                            s.behind && !s.busy
                        })
                        .collect();
                    if !behind.is_empty() {
                        crate::host::update_all(&hub, &behind);
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
            "app.close_window" => {
                self.exit_screen_for_navigation(cx);
                cx.request_close();
            }
            "app.quit" => {
                self.exit_screen_for_navigation(cx);
                shutdown_app();
                olo_kit::quit();
            }
            "app.reload_config" => hub.reload_server_config(&self.active_host, Feedback::start(self.id).again(WinMsg::Command("app.reload_config".into()))),
            "host.reconnect" => hub.connect_host(&self.active_host),
            // (A host let go of, and connected again, by its key: what
            // Hosts and Ports' one button does, for agents.)
            c if c.starts_with("host.disconnect:") => hub.disconnect_host(c.trim_start_matches("host.disconnect:")),
            c if c.starts_with("host.connect:") => hub.connect_host(c.trim_start_matches("host.connect:")),
            "session.new" => {
                self.exit_screen_for_navigation(cx);
                hub.create_session(&self.active_host, None, None, Feedback::start(self.id).again(WinMsg::CreateSession(self.active_host.clone(), None)));
            }
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
                self.exit_screen_for_navigation(cx);
                if let Some((hk, sess)) = self.resolve_active_session() {
                    request_new_tab(&hub, self.id, &hk, &sess.session_id);
                } else {
                    hub.create_session(&self.active_host, None, None, Feedback::start(self.id));
                }
            }
            "window.close" => {
                self.exit_screen_for_navigation(cx);
                if let Some((hk, sess)) = self.resolve_active_session()
                    && let Some(w) = sess.active_window()
                {
                    hub.close_window(&hk, &sess.session_id, &w.window_id);
                }
            }
            "window.next" | "window.previous" => {
                self.exit_screen_for_navigation(cx);
                if let Some((hk, sess)) = self.resolve_active_session() {
                    let (live, top_indices, root_id) = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        let (live, top_indices) = top_level_window_indices(&st.subagents, &hk, &sess);
                        let root_id = sess.active_window_id.as_deref().map(|id| st.subagents.root_ancestor(&hk, &sess.session_id, id, &live));
                        (live, top_indices, root_id)
                    };
                    let _ = live;
                    if top_indices.len() > 1 {
                        let cur = root_id.as_deref().and_then(|rid| top_indices.iter().position(|&idx| sess.windows[idx].window_id == rid)).unwrap_or(0);
                        let next = if action == "window.next" { (cur + 1) % top_indices.len() } else { (cur + top_indices.len() - 1) % top_indices.len() };
                        hub.select_window(&hk, &sess.session_id, &sess.windows[top_indices[next]].window_id);
                    }
                }
            }
            a if a.starts_with("window.select_") => {
                self.exit_screen_for_navigation(cx);
                if let Ok(n) = a.trim_start_matches("window.select_").parse::<usize>()
                    && n >= 1
                    && let Some((hk, sess)) = self.resolve_active_session()
                {
                    let (_, top_indices) = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        top_level_window_indices(&st.subagents, &hk, &sess)
                    };
                    if let Some(&widx) = top_indices.get(n - 1) {
                        hub.select_window(&hk, &sess.session_id, &sess.windows[widx].window_id);
                    }
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
                    // (In the middle of the panes' area, both ways.)
                    hub.create_floating_layer(&hk, &sess.session_id, [0.16, 0.16, 0.68, 0.68]);
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
                    // No text: check for copied file URIs (`text/uri-list`, etc.)
                    // or a pasted picture (put on the pane's host as a file of its
                    // own and its path typed).
                    olo_kit::clipboard::paste_as(vec!["x-special/gnome-copied-files".into(), "text/uri-list".into(), "image/png".into(), "image/".into()], move |got| {
                        if let Some((mime, bytes)) = got {
                            if let Some((paths, _)) = olo_kit_base::clipboard::copied_files(&mime, &bytes) {
                                let joined = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
                                if !joined.is_empty() {
                                    to.send(WinMsg::PasteText(bk, connection, joined));
                                    return;
                                }
                            }
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
                if let Some(tr) = crate::activity_watch::active_transcript() {
                    let max_y = f32::from(self.transcript_scroll.max_offset().height);
                    let page = (f32::from(self.transcript_scroll.bounds().size.height) * 0.85).max(120.0);
                    match action {
                        "terminal.scroll_page_up" => {
                            if max_y > 0.5 {
                                self.transcript_scroll.scroll_by(cx, point(px(0.0), px(page)));
                            }
                        }
                        "terminal.scroll_page_down" => {
                            if max_y > 0.5 {
                                self.transcript_scroll.scroll_by(cx, point(px(0.0), px(-page)));
                            }
                        }
                        "terminal.scroll_top" => {
                            if max_y > 0.5 {
                                self.transcript_scroll.set_offset(cx, point(px(0.0), px(0.0)));
                            }
                        }
                        "terminal.scroll_bottom" => {
                            {
                                let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                                g.follow = true;
                                g.paused_unseen = 0;
                                g.generation += 1;
                            }
                            self.transcript_scroll.scroll_to_bottom(cx);
                        }
                        _ => {}
                    }
                    cx.invalidate();
                    return;
                }
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
            "activity.follow_live" => {
                cx.send(WinMsg::TranscriptDo(crate::transcript_view::TranscriptDo::ToggleFollow));
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
                    self.exit_screen_for_navigation(cx);
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
                        crate::host::change(&hub, crate::hub::HostThing::Skill, crate::hub::HostOp::Install);
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
        if self.activity_search_field.as_ref().is_some_and(|f| f.is_focused(cx)) {
            if e.keystroke.key == "escape" {
                if let Some(f) = &self.activity_search_field {
                    f.update(cx, |tf, cx| tf.set_text(String::new(), cx));
                }
                self.focus.focus(cx);
                cx.invalidate();
                cx.stop_propagation();
            }
            return;
        }

        // Guard embedded Agent Screen BEFORE Copy Mode, custom keymaps, menu-bar
        // terminal shortcuts, and Shift-scroll so hidden-terminal actions (`⌘V`,
        // `⌘F`, `⌘K`, Copy Mode, splits) NEVER fire behind a visible screen
        // (Astra #6 / Neon #4).
        if self.active_screen.is_some() {
            let k = &e.keystroke;
            if self.screen_controlling() {
                return;
            }
            // Ctrl+Tab / Ctrl+PageUp/Down: switch Rex tabs.
            let step = olo_kit::tab_step(k);
            if step != 0 && self.resolve_active_session().is_some_and(|(_, s)| s.windows.len() > 1) {
                self.command(if step > 0 { "window.next" } else { "window.previous" }, cx);
                cx.stop_propagation();
                return;
            }
            // `screen.control` (⌘K / ⇧⌘K or custom non-command binding like Alt+K / F9):
            // evaluated OUTSIDE `keymap::command_held(k)` with physical-command
            // distinction so custom bindings work and Left `Ctrl+Shift+K` isn't treated
            // as `⌘K`, while skipping when typing inside a ScreenPane text field (Astra #25 / Neon #2).
            let editing_pane_text = self.screen_pane.as_ref().is_some_and(|p| p.read(cx).is_editing_text(cx));
            if !editing_pane_text && olo_screen_pane::is_control_shortcut_with_command(k, true, keymap::command_held(k)) {
                if let Some(pane) = &self.screen_pane {
                    pane.update(cx, |p, cx| {
                        p.run_command("control", cx);
                    });
                }
                cx.stop_propagation();
                return;
            }
            // Allow enabled non-terminal Rex menu shortcuts (terminal-only items are
            // disabled in `build_menu_bar` while `active_screen.is_some()`).
            if keymap::command_held(k) || own_plain_shortcut(k) {
                let menus = self.build_menu_bar();
                olo_kit::menu::menus_beginning(cx, menus);
                if olo_kit::menu::shortcut_command(cx, k).is_some() {
                    return;
                }
            }
            if k.key == "escape" && !self.find_open {
                self.close_screen(cx);
                self.focus.focus(cx);
            }
            cx.stop_propagation();
            return;
        }

        // User is actively typing into the terminal: cancel any stale pending reload restore (Neon #10).
        self.pending_restored_screen = None;

        // Check Copy Mode first!
        if let Some(bk) = self.focused_block_key() {
            let hub = the_hub().clone();
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
        // never leak keyboard input to the hidden parent terminal; Escape closes it
        // and standard navigation keys scroll the transcript directly.
        if crate::activity_watch::active_view_target().is_some() {
            if k.key == "escape" && !self.find_open {
                crate::activity_watch::close_viewer();
                cx.stop_propagation();
            } else if crate::activity_watch::active_transcript().is_some() && !k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform {
                let max_y = f32::from(self.transcript_scroll.max_offset().height);
                let page = (f32::from(self.transcript_scroll.bounds().size.height) * 0.85).max(120.0);
                match k.key.as_str() {
                    "pageup" => {
                        if max_y > 0.5 {
                            self.transcript_scroll.scroll_by(cx, point(px(0.0), px(page)));
                        }
                        cx.stop_propagation();
                    }
                    "pagedown" => {
                        if max_y > 0.5 {
                            self.transcript_scroll.scroll_by(cx, point(px(0.0), px(-page)));
                        }
                        cx.stop_propagation();
                    }
                    "up" if !self.find_open => {
                        if max_y > 0.5 {
                            self.transcript_scroll.scroll_by(cx, point(px(0.0), px(48.0)));
                        }
                        cx.stop_propagation();
                    }
                    "down" if !self.find_open => {
                        if max_y > 0.5 {
                            self.transcript_scroll.scroll_by(cx, point(px(0.0), px(-48.0)));
                        }
                        cx.stop_propagation();
                    }
                    "home" if !self.find_open => {
                        if max_y > 0.5 {
                            self.transcript_scroll.set_offset(cx, point(px(0.0), px(0.0)));
                        }
                        cx.stop_propagation();
                    }
                    "end" if !self.find_open => {
                        self.execute_action("terminal.scroll_bottom", None, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }
            return;
        }

        // To the pane the keyboard is in (to every pane of the tab while Type in All Panes is on).
        let Some(bk) = self.focused_block_key() else { return };
        let targets = self.input_target_blocks(&bk);
        let hub = the_hub().clone();

        // (What the pane's program asked of its keys.)
        let (modes, kitty_flags) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).map_or_else(Default::default, |b| {
            let b = b.lock().unwrap_or_else(|e| e.into_inner());
            let flags = b.terminal.kitty_keyboard_flags();
            (olo_kit::term_keys::Modes { app_cursor: b.terminal.dec_mode(1), kitty: flags != 0 }, flags)
        });
        // A program that asked for the kitty keyboard protocol is told
        // its keys by the server's own encoder, which has the protocol
        // and the program's flags as they stand: sent as bytes from
        // here, Ctrl+Enter reached Claude Code as a Return (Ben,
        // 2026-11-26; `term_keys::kitty_encodes`).
        if !cmd && !k.modifiers.platform && olo_kit::term_keys::kitty_encodes(olo_kit::term_keys::Key::of(k), kitty_flags) {
            let rex_ev = keymap::to_rex_key_event(e, false);
            for t in targets {
                hub.send_rex_key(&t, rex_ev.clone());
            }
            cx.stop_propagation();
        } else if let Some(bytes) = encode_terminal_special_key(k, modes).filter(|_| !cmd) {
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
        let hub = the_hub().clone();
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
        // While an embedded Agent Screen is visible, disable terminal-only menu
        // items (Edit ▸ Paste, Paste Password…, Find…, Clear Scrollback, Pane ▸
        // Split/Close) so they never act on the hidden terminal (Astra #6 / Neon #4).
        let screen_visible = self.active_screen.is_some();
        let focused = self.focused_block_key();
        let pane = !screen_visible && focused.is_some();
        let block = if screen_visible { None } else { focused.as_ref().and_then(|key| hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(key).cloned()) };
        let help = self.focused_block_key().map(|bk| hub.help(&bk)).unwrap_or_default();
        let (keep_going, selected, copying, found) = block.as_ref().map_or((false, false, false, false), |block| {
            let g = block.lock().unwrap_or_else(|e| e.into_inner());
            (help.keep_going, g.terminal.has_selection(), g.copy_mode.is_some(), g.search.total_matches() > 0)
        });
        let session = self.resolve_active_session().map(|(_, s)| s);
        let window = session.as_ref().and_then(|s| s.active_window().or_else(|| s.windows.first()).map(|w| (w.zoomed_block_id.is_some(), w.all_blocks().count())));
        let (zoomed, panes) = if screen_visible { (false, 0) } else { window.unwrap_or((false, 0)) };
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
            {
                let tr_open = crate::activity_watch::active_transcript();
                let tr_following = tr_open.as_ref().is_some_and(|t| t.lock().unwrap_or_else(|e| e.into_inner()).follow);
                Menu::new("View")
                    .item("Show Sidebar", "app.toggle_sidebar")
                    .checked(sidebar_shown)
                    .item("Show Pane Headers", "panes.headers")
                    .checked(headers == BarBehavior::Always)
                    .item("Show Idle Panes in Activity", "activity.include_idle")
                    .checked(include_idle)
                    .item("Follow Live", "activity.follow_live")
                    .enabled(tr_open.is_some())
                    .checked(tr_open.is_some() && tr_following)
                    .separator()
                    .item("Sessions", "app.view_sessions")
                    .checked(sidebar_mode == SidebarMode::Sessions)
                    .item("Activity", "app.view_activity")
                    .checked(sidebar_mode == SidebarMode::Activity)
            },
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
                .enabled(pane)
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
                .enabled(pane && (panes > 1 || broadcast_on))
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
                .enabled(pane)
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
        let hub = the_hub().clone();

        // Provide structured context to Olo's `/ask` palette!
        if !olo_kit::agent::has_about(cx) {
            olo_kit::agent::about_from(cx, move |_| {
                let st = the_hub().state.lock().unwrap_or_else(|e| e.into_inner());
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

        let tr_active = crate::activity_watch::active_transcript();
        if self.window_active {
            // Astra #13 / Neon #8.1: when an embedded Agent Screen (or transcript)
            // occupies the center area, the underlying terminal is hidden — never
            // clear its unread flag or register it as frontmost for notification
            // suppression!
            if self.active_screen.is_some() {
                app().attention.hide(self.id);
                hub.hide_window_front_pane(self.id);
            } else {
                let content_visible = !matches!(self.side_pane.panes(f32::from(viewport.width), 460.0), SidePanes::Pane) && self.overlay.is_none() && tr_active.is_none();
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
                hub.set_window_front_pane(self.id, pane.clone());
                let front = match pane {
                    Some(p) => Target::Pane(p),
                    None if content_visible && let Some((host, session)) = &sess_info => Target::Session { host_key: host.clone(), session_id: session.session_id.clone() },
                    None => Target::Host(self.active_host.clone()),
                };
                app().attention.showing(self.id, front);
            }
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

        // Build TabStrip from active session's top-level windows only; native
        // child subagent tabs nest under their parent tab's kit tree button.
        let mut strip_tabs = Vec::new();
        let mut top_ids: Vec<String> = Vec::new();
        let mut active_tab_idx = 0usize;
        if let Some((ref hk, ref sess)) = sess_info {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let names = crate::hub::session_tab_names(&st, hk, sess);
            let (live, top_indices) = top_level_window_indices(&st.subagents, hk, sess);
            let active_raw_id = sess.active_window_id.as_deref().or_else(|| sess.windows.first().map(|w| w.window_id.as_str()));
            let active_root_id = active_raw_id.map(|id| st.subagents.root_ancestor(hk, &sess.session_id, id, &live));
            let active_is_child = active_raw_id.is_some_and(|raw| active_root_id.as_deref() != Some(raw));

            for (strip_idx, &w_idx) in top_indices.iter().enumerate() {
                let w = &sess.windows[w_idx];
                top_ids.push(w.window_id.clone());
                let is_active_root = active_root_id.as_deref() == Some(w.window_id.as_str());
                if is_active_root {
                    active_tab_idx = strip_idx;
                }
                let descendants = st.subagents.descendants_tree(hk, &sess.session_id, &w.window_id, &live);
                let children_count = descendants.len();
                let child_active = is_active_root && active_is_child;

                let mut tab_act = AgentActivity::Idle;
                let mut tab_unread: Option<Option<Instant>> = None;
                let mut tab_help = crate::agent::Help::default();
                let mut accum_block = |bk: BlockKey| {
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
                };
                for b in w.all_blocks() {
                    accum_block(BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() });
                }
                for row in &descendants {
                    accum_block(BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: row.entry.block_id.clone() });
                }
                let label = names.get(w_idx).cloned().unwrap_or_default();
                // Opus Minor: when a tab has a waiting/error badge, suppress the
                // secondary blue Keep It Going dot so the two badges don't blur at 1×.
                let icon_help = if matches!(tab_act, AgentActivity::Blocked | AgentActivity::Error) { crate::agent::Help { keep_going: false, ..tab_help.clone() } } else { tab_help.clone() };
                let icon_fn: Option<olo_kit::widgets::strip::PaintFn> = Some(widgets::agent_marks::icon("terminal", marks_of(tab_act, &icon_help, false)));
                let what = match tab_act {
                    AgentActivity::Blocked => Some(olo_kit::words::tr("Waiting for approval").to_string()),
                    AgentActivity::Working => Some(olo_kit::words::tr("Working").to_string()),
                    AgentActivity::Error => Some(olo_kit::words::tr("Error").to_string()),
                    AgentActivity::Done => Some(olo_kit::words::tr("Done").to_string()),
                    AgentActivity::Idle => None,
                };
                let words: Vec<String> = [what, help_words(&tab_help)].into_iter().flatten().collect();
                strip_tabs.push(StripTab {
                    label: label.into(),
                    icon: icon_fn,
                    status: (!words.is_empty()).then(|| words.join("; ").into()),
                    unread: tab_unread,
                    children_count,
                    child_active,
                    ..Default::default()
                });
            }
        }

        let active_screen_id = self.active_screen.as_ref().map(|s| s.id.clone());
        // If narrow-window mode hid the center pane behind the sidebar while a
        // screen was open, release control and end the stream immediately.
        if matches!(side_layout, SidePanes::Pane) && active_screen_id.is_some() {
            self.close_screen(cx);
        }
        let active_screen_id = self.active_screen.as_ref().map(|s| s.id.clone());
        // Astra #25: read the O(1)-cached resolved screen inventory ONCE per render
        // and share it across the breadcrumb, sidebar, and status bar.
        let need_resolved = active_screen_id.is_some() || (!matches!(side_layout, SidePanes::Rest) && settings.sidebar_mode == SidebarMode::Activity);
        let resolved_screens_once = need_resolved.then(|| {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            self.resolved_screens_cached(&st)
        });
        // When the active tab is a native child subagent tab, build a native
        // STRIP_ROW (26 pt) breadcrumb header (`parent › child › Current` + right-aligned
        // `harness · model · effort`) so the main view always says where you are.
        let child_header: Option<Element> = if tr_active.is_none()
            && active_screen_id.is_none()
            && let Some((ref hk, ref sess)) = sess_info
            && let Some(active_wid) = sess.active_window_id.as_deref()
        {
            let info = {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                let live: HashSet<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
                st.subagents.by_window(hk, &sess.session_id, active_wid).filter(|_| st.subagents.is_child_window(hk, &sess.session_id, active_wid, &live)).map(|entry| {
                    let names = crate::hub::session_tab_names(&st, hk, sess);
                    let mut chain: Vec<(String, String)> = Vec::new();
                    let mut cur = entry.parent_window_id.clone();
                    let mut seen = HashSet::new();
                    seen.insert(entry.window_id.clone());
                    while seen.insert(cur.clone()) {
                        if let Some(sub) = st.subagents.by_window(hk, &sess.session_id, &cur) {
                            let next = sub.parent_window_id.clone();
                            let t = sub.display_title();
                            chain.push((cur.clone(), t));
                            if live.contains(&next) {
                                cur = next;
                                continue;
                            }
                            break;
                        } else if let Some(widx) = sess.windows.iter().position(|w| w.window_id == cur) {
                            let t = names.get(widx).cloned().unwrap_or_else(|| "Tab".into());
                            chain.push((cur.clone(), t));
                            break;
                        } else {
                            break;
                        }
                    }
                    chain.reverse();
                    let bk = BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: entry.block_id.clone() };
                    let (act, running) = st
                        .blocks
                        .get(&bk)
                        .map(|br| {
                            let g = br.lock().unwrap_or_else(|e| e.into_inner());
                            (g.agent_activity, g.runs_agent() || g.agent_activity != AgentActivity::Idle)
                        })
                        .unwrap_or((AgentActivity::Idle, false));
                    (hk.clone(), sess.session_id.clone(), chain, entry.clone(), act, running)
                })
            };
            let hdr_avail_w = (total_w - side_w - if settings.toolbar_layout == ToolbarLayout::Side { widgets::STRIP_SIDE_WIDTH } else { 0.0 }).max(200.0);
            info.map(|(hk, sid, ancestors, entry, act, running)| render_child_breadcrumb_header(hk, sid, ancestors, entry, act, running, hdr_avail_w, cx))
        } else {
            None
        };

        let active_has_splits = sess_info.as_ref().and_then(|(_, s)| s.active_window().or_else(|| s.windows.first())).is_some_and(|w| w.all_blocks().count() > 1 || w.zoomed_block_id.is_some());
        let active_shows_header = (active_has_splits && settings.pane_header_behavior != BarBehavior::Hidden) || child_header.is_some();
        // Active tab opens seamlessly into the focused pane header or child breadcrumb header (panel_background)!
        let (tab_content_bg, tab_content_fg) =
            if active_shows_header { (style::c(look.panel_background), style::c(look.panel_text)) } else { (hsla(active_theme.background), hsla(active_theme.foreground)) };

        let is_side_strip = settings.toolbar_layout == ToolbarLayout::Side;
        let strip_el: Option<Element> = if !strip_tabs.is_empty()
            && let Some((hk, sess)) = &sess_info
        {
            let hub = hub.clone();
            let win_id = self.id;
            let (hk, sid) = (hk.clone(), sess.session_id.clone());
            let ids = top_ids;
            let mut strip = TabStrip::new("rex-tabs", strip_tabs, active_tab_idx).content(tab_content_bg, tab_content_fg).tree_open(self.open_tree_tab).in_window(surface).on_action(cx.msg_with(
                move |act: &StripAction| {
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
                        StripAction::New => request_new_tab(&hub, win_id, &hk, &sid),
                        StripAction::Menu(i, at) => return WinMsg::TabMenu(*i, *at),
                        StripAction::TreeMenu(i, bounds) => return WinMsg::TabTreeMenu(*i, *bounds),
                        StripAction::Move { from, to, .. } => {
                            if let Some(before) = crate::hub::moved_before(&ids, *from, *to) {
                                hub.move_window(&hk, &sid, &ids[*from], before);
                            }
                        }
                        _ => return WinMsg::Redraw,
                    }
                    WinMsg::TabChanged
                },
            ));
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
        if tr_active.is_none() {
            self.last_transcript_ptr = 0;
        }
        let panes_el = if let Some(ref scr_id) = active_screen_id
            && self.screen_pane.is_some()
        {
            let center_w = (total_w - side_w - if is_side_strip && strip_el.is_some() { widgets::STRIP_SIDE_WIDTH } else { 0.0 }).max(200.0);
            let vp_h = f32::from(cx.viewport_size().height);
            let hide_status_now = self.problem.is_none() && vp_h < 300.0;
            let top_chrome_h = 26.0 + if !is_side_strip && strip_el.is_some() { widgets::STRIP_HEIGHT } else { 0.0 } + if hide_status_now { 0.0 } else { 22.0 };
            let center_h = (vp_h - top_chrome_h).max(120.0);
            // Opus #1 & Neon #11.1: share exact `SCREEN_BREADCRUMB_H` (40 pt) so
            // `ScreenPane::set_size` receives the true available height!
            let show_breadcrumb = center_h >= 330.0;
            let pane_h = if show_breadcrumb { (center_h - Self::SCREEN_BREADCRUMB_H).max(100.0) } else { center_h };
            let narrow_breadcrumb_menu = show_breadcrumb && center_w < 460.0;
            let resolved = resolved_screens_once.clone().unwrap_or_else(|| {
                let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                self.resolved_screens_cached(&st)
            });
            let owner_bk_for_overflow = resolved.by_id.get(scr_id).and_then(|it| it.owner_bk.clone());
            if let Some(pane) = &self.screen_pane {
                let scr_id_cb = scr_id.clone();
                let win_id_cb = self.id;
                let owner_cb = owner_bk_for_overflow.clone();
                pane.update(cx, |p, cx| {
                    p.set_size(center_w, pane_h, cx);
                    // Opus #3: in `v-third` (`show_breadcrumb && center_w < 460`),
                    // `Screen ▾` lives in the breadcrumb bar beside `Show Terminal`,
                    // so hide the redundant wrapped button row in ScreenPane's header.
                    p.set_hide_header_actions(narrow_breadcrumb_menu, cx);
                    if !show_breadcrumb {
                        let actions = vec![
                            (olo_kit::SharedString::from(olo_kit::words::tr("Open in Agent Screens")), olo_kit::SharedString::from("open_app")),
                            (olo_kit::SharedString::from(olo_kit::words::tr("Show Terminal")), olo_kit::SharedString::from("show_terminal")),
                        ];
                        p.set_host_overflow(
                            actions,
                            move |cmd, _| match cmd {
                                "open_app" => {
                                    if let Err(why) = crate::activity_screens::open_in_agent_screens(&scr_id_cb) {
                                        Feedback::start(win_id_cb).problem("Couldn’t open Agent Screens.", why);
                                    }
                                }
                                "show_terminal" => {
                                    if let Some(slot) = wins().map.get(&win_id_cb) {
                                        if let Some(bk) = owner_cb.clone() {
                                            slot.handle.messenger().send(WinMsg::FocusBlock(bk));
                                        } else {
                                            slot.handle.messenger().send(WinMsg::CloseScreen);
                                        }
                                    }
                                }
                                _ => {}
                            },
                            cx,
                        );
                    } else if center_w < 460.0 {
                        let actions = vec![(olo_kit::SharedString::from(olo_kit::words::tr("Open in Agent Screens")), olo_kit::SharedString::from("open_app"))];
                        p.set_host_overflow(
                            actions,
                            move |cmd, _| {
                                if cmd == "open_app"
                                    && let Err(why) = crate::activity_screens::open_in_agent_screens(&scr_id_cb)
                                {
                                    Feedback::start(win_id_cb).problem("Couldn’t open Agent Screens.", why);
                                }
                            },
                            cx,
                        );
                    } else {
                        p.clear_host_overflow(cx);
                    }
                });
            }
            let top_strip = show_breadcrumb.then(|| self.render_screen_breadcrumb_strip(scr_id, &resolved, center_w, cx));
            let pane_ref = self.screen_pane.as_ref().unwrap();
            div().flex_1().min_w_0().min_h_0().w_full().flex().flex_col().children(top_strip).child(div().flex_1().min_w_0().min_h_0().w_full().child(pane_ref))
        } else if let Some(tr_sess) = tr_active {
            let ptr = Arc::as_ptr(&tr_sess) as usize;
            let tr_changed = ptr != self.last_transcript_ptr;
            if tr_changed {
                self.last_transcript_ptr = ptr;
                self.transcript_scroll.set_offset(cx, olo_kit::Point::default());
            }
            let (cur_gen, follow) = {
                let g = tr_sess.lock().unwrap_or_else(|e| e.into_inner());
                (g.generation, g.follow)
            };
            let (tr_el, scroll_to_child) = crate::transcript_view::render_transcript_view(
                &tr_sess,
                &self.transcript_scroll,
                &active_theme,
                self.find_open,
                tr_find_bar,
                cx.msg_with(|what: &crate::transcript_view::TranscriptDo| WinMsg::TranscriptDo(what.clone())),
            );
            if let Some(ix) = scroll_to_child {
                self.last_transcript_gen = cur_gen;
                self.transcript_scroll.scroll_to_item(cx, ix);
            } else if follow && (tr_changed || cur_gen != self.last_transcript_gen) {
                self.last_transcript_gen = cur_gen;
                self.transcript_scroll.scroll_to_bottom(cx);
            } else {
                self.last_transcript_gen = cur_gen;
            }
            div().flex_1().min_w_0().min_h_0().w_full().flex().flex_col().child(tr_el)
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
            // (Less the tabs' own column where they stand at the side: with
            // it counted in, a pane's head kept buttons it had no room for.)
            let width = Some((total_w - side_w - if is_side_strip && strip_el.is_some() { widgets::STRIP_SIDE_WIDTH } else { 0.0 }).max(200.0));
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
            let askpass = the_askpass().clone();
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
                if let Some(ch) = child_header {
                    c = c.child(ch);
                }
                if let Some(fb) = find_el {
                    c = c.child(fb);
                }
                c.child(panes_el)
            }
            ToolbarLayout::Side => {
                let mut inner = div().flex_1().min_w_0().min_h_0().flex().flex_col();
                if let Some(ch) = child_header {
                    inner = inner.child(ch);
                }
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
                body_row = body_row.child(self.render_sidebar(w, resolved_screens_once.as_ref(), cx)).child(center);
            }
            SidePanes::Pane => {
                body_row = body_row.child(self.render_sidebar(total_w, resolved_screens_once.as_ref(), cx));
            }
            SidePanes::Rest => {
                body_row = body_row.child(center);
            }
        }

        // The sidebar's divider, in the row it divides: not over the
        // status bar under it, whose words went under its grip, and not
        // over side-tab rows when the sidebar is collapsed.
        if !self.side_pane.alone() && !(is_side_strip && matches!(side_layout, SidePanes::Rest)) {
            body_row = body_row.child(side_divider_within(&self.side_pane, side_w, "Rex Sidebar", cx.msg_with(|_: &()| WinMsg::SidebarMoved)));
        }

        // Overlays layered above the workspace!
        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|w, e: &KeyDownEvent, cx| w.capture_screen_key(e, cx)))
            .on_key_down(cx.listener(|w, e: &KeyDownEvent, cx| w.handle_terminal_key(e, cx)))
            .child(menu_bar)
            .child(body_row);

        // The status bar is always there, as Tracker's is: where the
        // window is (its host, its session) while there is nothing to
        // say, a passing word or a problem when there is. (It came and
        // went with each word: the panes above it lost and regained a
        // row each time, which to a program in a terminal is the window
        // resized twice. The second design review.)
        const STATUS_ROW_H: f32 = 18.0;
        let line = match self.feedback_line(cx) {
            Some(line) => line,
            None => div().child(StringView::new(self.where_it_is(resolved_screens_once.as_ref(), cx)).size(style::SMALL_SIZE).height(STATUS_ROW_H).dim(true).cut(true)).into_any_element(),
        };
        let silent = self.silent_host();
        let tr_visible = !matches!(side_layout, SidePanes::Pane);
        let tr_max_y = f32::from(self.transcript_scroll.max_offset().height);
        let tr_scrollable = tr_max_y > 0.5;
        let tr_at_bottom = !tr_scrollable || -f32::from(self.transcript_scroll.offset().y) >= tr_max_y - 4.0;
        let tr_status = tr_visible.then(crate::activity_watch::active_transcript).flatten().map(|tr| {
            let g = tr.lock().unwrap_or_else(|e| e.into_inner());
            (g.follow, g.paused_unseen, g.streaming, !g.doc.entries.is_empty())
        });
        let has = if tr_status.is_some() { None } else { self.host_has() };
        // (A file going to or from the focused pane's host.)
        let transfer = self
            .focused_block_key()
            .and_then(|bk| hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned())
            .and_then(|b| b.lock().unwrap_or_else(|e| e.into_inner()).upload_progress.clone());
        let marks_host = self.focused_block_key().map_or_else(|| self.active_host.clone(), |focused| focused.host_key);
        let open_host_window: Rc<dyn Fn(&String, &mut WindowCx)> = Rc::new(cx.msg_with(|host: &String| WinMsg::HostOpen(host.clone())));
        let open_account_window: Rc<dyn Fn(&String, &mut WindowCx)> = Rc::new(cx.msg_with(|host: &String| WinMsg::AccountOpen(host.clone())));
        let jump_live_cb = cx.msg(WinMsg::TranscriptDo(crate::transcript_view::TranscriptDo::JumpToLive));
        let transcript_tail_status = tr_status.and_then(|(follow, unseen, is_live_agent, has_entries)| {
            if !has_entries {
                return None;
            }
            // When the whole transcript fits in the viewport (`!tr_scrollable`), never show
            // `Paused` + `Bottom` (there is nowhere to scroll); only show `● Live` if streaming.
            if !tr_scrollable {
                if !is_live_agent {
                    return None;
                }
                let p = theme::palette();
                let dc = style::c(p.success);
                let row = div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        canvas(move |g, b| {
                            let mid = style::baseline_at(g, b.center().y, style::SMALL_SIZE) - px(style::SMALL_SIZE * style::CAP_HEIGHT / 2.0);
                            style::dot(g, point(b.center().x, mid), 6.0, dc);
                        })
                        .paint_key(style::key(("rex-sb-live-dot", true)))
                        .w(px(8.0))
                        .h(px(STATUS_ROW_H))
                        .flex_shrink_0(),
                    )
                    .child(StringView::new(olo_kit::words::tr("Live").to_owned()).size(style::SMALL_SIZE).dim(true).height(STATUS_ROW_H));
                return Some(row.into_any_element());
            }
            let p = theme::palette();
            if follow || (tr_at_bottom && !is_live_agent) {
                let (dot_c, word) = if is_live_agent { (Some(style::c(p.success)), olo_kit::words::tr("Live")) } else { (None, olo_kit::words::tr("End")) };
                let mut row = div().flex_shrink_0().flex().flex_row().items_center().gap(px(5.0));
                if let Some(dc) = dot_c {
                    row = row.child(
                        canvas(move |g, b| {
                            let mid = style::baseline_at(g, b.center().y, style::SMALL_SIZE) - px(style::SMALL_SIZE * style::CAP_HEIGHT / 2.0);
                            style::dot(g, point(b.center().x, mid), 6.0, dc);
                        })
                        .paint_key(style::key(("rex-sb-live-dot", is_live_agent)))
                        .w(px(8.0))
                        .h(px(STATUS_ROW_H))
                        .flex_shrink_0(),
                    );
                }
                row = row.child(StringView::new(word.to_owned()).size(style::SMALL_SIZE).dim(true).height(STATUS_ROW_H));
                Some(row.into_any_element())
            } else {
                let note = if is_live_agent {
                    if unseen > 0 { olo_kit::tr!("Paused — {} new", unseen).to_string() } else { olo_kit::words::tr("Paused").to_owned() }
                } else {
                    olo_kit::words::tr("Paused").to_owned()
                };
                let dot_c = if is_live_agent { crate::transcript_view::waiting_chrome_color(&p) } else { style::secondary_text(&p) };
                let btn_label = if is_live_agent { olo_kit::words::tr("Jump to Live").to_owned() } else { olo_kit::words::tr("Jump to End").to_owned() };
                let btn_w = (f32::from(style::text_width(btn_label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))) + 16.0).ceil().max(64.0);
                let btn_key = style::key(("rex-sb-jump-btn", &btn_label));
                let cb = jump_live_cb;
                let jump_btn = olo_kit::control_canvas(move |g, r, s| {
                    let p = theme::palette();
                    let base = p.control_background;
                    let bg = p.panel_background;
                    let dark = p.mode == olo_theme::ThemeMode::Dark;
                    let (t0, t1) = if s.pressed {
                        (style::shade(&p, base, 1.15), style::shade(&p, base, 1.05))
                    } else if dark {
                        (style::c(olo_theme::mix(base, p.panel_text, 0.08)), style::c(base))
                    } else {
                        (style::tint(base, 0.88), style::shade(&p, base, 1.06))
                    };
                    let inner = style::inset(g, r, 1.0);
                    style::vgradient(g, inner, t0, t1);
                    style::frame_rect(g, r, style::shade(&p, bg, 1.55));
                    if s.pressed {
                        style::bevel(g, inner, style::shade(&p, base, 1.3), style::shade(&p, base, 1.05));
                    } else {
                        let shine = if dark { style::c(olo_theme::mix(base, [255, 255, 255, 255], 0.2)) } else { style::c(p.shine) };
                        style::bevel(g, inner, shine, style::shade(&p, base, 1.18));
                    }
                    let dx = if s.pressed { px(0.5) } else { px(0.0) };
                    let bl = style::baseline_at(g, r.center().y, style::SMALL_SIZE) + dx;
                    style::label_on_baseline(g, r.center().x + dx, bl, style::Align::Center, btn_label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.control_text));
                    g.agent(agent::Node { role: "Button", name: Some(btn_label.clone().into()), bounds: r, enabled: true, clickable: true, focused: s.focused, ..Default::default() });
                })
                .paint_key(btn_key)
                .id("rex-tr-jump-live")
                .w(px(btn_w))
                .h(px(STATUS_ROW_H))
                .flex_shrink_0()
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, move |_, cx| {
                    cb(&(), cx);
                    cx.stop_propagation();
                });

                Some(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    canvas(move |g, b| {
                                        let mid = style::baseline_at(g, b.center().y, style::SMALL_SIZE) - px(style::SMALL_SIZE * style::CAP_HEIGHT / 2.0);
                                        style::dot(g, point(b.center().x, mid), 6.0, dot_c);
                                    })
                                    .paint_key(style::key(("rex-sb-pause-dot", is_live_agent)))
                                    .w(px(8.0))
                                    .h(px(STATUS_ROW_H))
                                    .flex_shrink_0(),
                                )
                                .child(StringView::new(note).size(style::SMALL_SIZE).dim(true).height(STATUS_ROW_H)),
                        )
                        .child(jump_btn)
                        .into_any_element(),
                )
            }
        });
        // On a short horizontal third while viewing an embedded ScreenPane (with no error
        // banner), omit the redundant bottom status bar so ScreenPane's Live/History tabs
        // and Take Control footer fit without clipping (host & size are in the pane header).
        let hide_status_for_compact_screen = active_screen_id.is_some() && self.problem.is_none() && f32::from(cx.viewport_size().height) < 300.0;
        if !hide_status_for_compact_screen {
            root = root.child(
                div().w_full().flex_shrink_0().flex().flex_col().child(widgets::separator()).child(
                    div()
                        .relative()
                        .w_full()
                        .min_h(px(STATUS_ROW_H + 2.0))
                        .px(px(m::SPACING))
                        // Symmetric 1 pt vertical padding inside the 20 pt status bar; when an
                        // error banner is open, optical 12 pt above icon/headline (9 pt + line slack)
                        // and 12 pt below the sunken detail well.
                        .pt(px(if self.problem.is_some() { 9.0 } else { 1.0 }))
                        .pb(px(if self.problem.is_some() { m::SPACING } else { 1.0 }))
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
                                .children(transcript_tail_status)
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
        }

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

    fn render_sidebar(&mut self, width: f32, resolved_once: Option<&Arc<crate::activity_screens::ResolvedActivityScreens>>, cx: &mut olo_kit::Cx<Self>) -> Element {
        let hub = the_hub().clone();
        // (The pane this window shows: Activity's selected row. While a screen
        // is shown, that screen row is selected instead of a terminal row.)
        let active_screen_id = self.active_screen.as_ref().map(|s| s.id.clone());
        let active_screen_controlling = self.screen_controlling();
        let shown = if active_screen_id.is_some() { None } else { self.focused_block_key() };
        let (mode, hosts_snap, activity_rows, standalone_screens, standalone_desktop, blocked_n, native_agent_json) = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            let mode = st.settings.sidebar_mode;
            // Astra #25: reuse the single per-render resolved inventory.
            let resolved_screens = if mode == SidebarMode::Activity { Some(resolved_once.cloned().unwrap_or_else(|| self.resolved_screens_cached(&st))) } else { None };
            let mut hosts_snap = Vec::new();
            let mut activity_rows = Vec::new();
            let mut blocked_total = 0usize;
            for hk in &st.host_order {
                let Some(h) = st.hosts.get(hk) else { continue };
                let mut sessions = Vec::new();
                for s in &h.session_summaries {
                    let view = h.sessions.get(&s.session_id);
                    let mut sess_act = AgentActivity::Idle;
                    let mut sess_help = crate::agent::Help::default();
                    let mut unread_count = 0usize;
                    if let Some(v) = view {
                        let tab_names = crate::hub::session_tab_names(&st, &h.key, v);
                        let live: HashSet<String> = v.windows.iter().map(|w| w.window_id.clone()).collect();
                        for (tab, w) in v.windows.iter().enumerate() {
                            let is_child = st.subagents.is_child_window(&h.key, &v.session_id, &w.window_id, &live);
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
                                    if matches!(g.agent_activity, AgentActivity::Blocked | AgentActivity::Error) {
                                        blocked_total += 1;
                                    }
                                }
                            }
                            if is_child || mode != SidebarMode::Activity {
                                continue;
                            }
                            let descendants = st.subagents.descendants_tree(&h.key, &v.session_id, &w.window_id, &live);
                            let mut children_items: Vec<NativeChildItem> = Vec::with_capacity(descendants.len());
                            for (d_idx, trow) in descendants.iter().enumerate() {
                                let child_bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: trow.entry.block_id.clone() };
                                let command = !trow.entry.cli.is_agent();
                                let (act, running, detail) = if command {
                                    (AgentActivity::Idle, true, trow.entry.what())
                                } else if let Some(br) = st.blocks.get(&child_bk) {
                                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                    let live_msg = g.program_statuses.first().and_then(|r| r.msg.clone().or_else(|| r.title.clone()));
                                    let detail = match live_msg {
                                        Some(msg) if !msg.trim().is_empty() => format!("{} · {}", trow.entry.cli.chrome_name(), msg.trim()),
                                        _ => trow.entry.what(),
                                    };
                                    (g.agent_activity, g.runs_agent() || g.agent_activity != AgentActivity::Idle, detail)
                                } else {
                                    (AgentActivity::Idle, false, trow.entry.what())
                                };
                                let child_screens = resolved_screens.as_ref().and_then(|r| r.by_owner.get(&child_bk)).cloned().unwrap_or_default();
                                let has_children = descendants.get(d_idx + 1).is_some_and(|next| next.depth > trow.depth) || !child_screens.is_empty();
                                let help = crate::hub::help_of(&child_bk);
                                children_items.push(NativeChildItem {
                                    row: trow.clone(),
                                    bk: child_bk,
                                    act,
                                    detail,
                                    running,
                                    command,
                                    help,
                                    has_children,
                                    screens: child_screens,
                                    ancestor_only: false,
                                });
                            }

                            let agents_here = w
                                .all_blocks()
                                .filter(|b| {
                                    let bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                    resolved_screens.as_ref().is_some_and(|r| r.by_owner.contains_key(&bk))
                                        || st.blocks.get(&bk).is_some_and(|br| {
                                            let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                            g.agent_activity != AgentActivity::Idle || g.runs_agent() || st.settings.activity_include_idle
                                        })
                                })
                                .count();
                            let mut attached_children = false;
                            let child_target_bid: Option<String> = (!children_items.is_empty())
                                .then(|| {
                                    descendants
                                        .iter()
                                        .find_map(|d| d.entry.parent_block_id.as_deref())
                                        .filter(|pid| w.all_blocks().any(|b| b.block_id == *pid))
                                        .or_else(|| {
                                            w.all_blocks().find_map(|b| {
                                                let bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                                st.blocks
                                                    .get(&bk)
                                                    .is_some_and(|br| {
                                                        let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                                        g.agent_activity != AgentActivity::Idle || g.runs_agent()
                                                    })
                                                    .then_some(b.block_id.as_str())
                                            })
                                        })
                                        .or(w.focused_block_id.as_deref())
                                        .or_else(|| w.all_blocks().next().map(|b| b.block_id.as_str()))
                                        .map(str::to_owned)
                                })
                                .flatten();
                            for b in w.all_blocks() {
                                let bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                if let Some(br) = st.blocks.get(&bk) {
                                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                    let running = g.runs_agent();
                                    let attach_here = !attached_children && child_target_bid.as_deref() == Some(b.block_id.as_str());
                                    let own_screens = resolved_screens.as_ref().and_then(|r| r.by_owner.get(&bk)).cloned().unwrap_or_default();
                                    // Visibility guarantee (design §5): if a live top-level pane owns a screen,
                                    // Activity forces that pane row to show so owned screens are never orphaned.
                                    if g.agent_activity != AgentActivity::Idle || running || st.settings.activity_include_idle || attach_here || !own_screens.is_empty() {
                                        let process = g.process.as_ref();
                                        let program = g.program_title();
                                        let home = process.and_then(|p| p.home.clone()).or_else(|| h.is_local.then(crate::hub::local_home).flatten());
                                        let pane = crate::hub::pane_title(&g.title, &g.label, program.as_deref(), "");
                                        let label = activity_title(tab_names.get(tab).map(String::as_str), agents_here.max(1), pane);
                                        let raw_detail = g.program_statuses.first().and_then(|r| r.msg.clone().or_else(|| r.title.clone())).unwrap_or_else(|| folder_name(&g.pwd, home.as_deref()));
                                        let help = crate::hub::help_of(&bk);
                                        let parent_agent = detect_agent_harness(program.as_deref(), &g.title);
                                        let detail = match parent_agent.as_deref() {
                                            Some(h_name) if !raw_detail.is_empty() && !raw_detail.to_ascii_lowercase().starts_with(h_name) => {
                                                let chrome = if h_name == "agy" { "antigravity" } else { h_name };
                                                format!("{chrome} · {raw_detail}")
                                            }
                                            _ => raw_detail,
                                        };
                                        let kids = if attach_here {
                                            attached_children = true;
                                            std::mem::take(&mut children_items)
                                        } else {
                                            Vec::new()
                                        };
                                        activity_rows.push(ActivityRow {
                                            bk,
                                            host_key: h.key.clone(),
                                            host: h.label.clone(),
                                            title: label,
                                            detail,
                                            act: g.agent_activity,
                                            help,
                                            running,
                                            parent_agent,
                                            screens: own_screens,
                                            children: kids,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    let label = if !s.label.is_empty() { s.label.clone() } else { s.session_id.clone() };
                    sessions.push((s.session_id.clone(), label, sess_act, sess_help, unread_count));
                }
                let silent = st.host_silent(&h.key).map(crate::hub::silent_tip);
                hosts_snap.push((h.key.clone(), h.label.clone(), h.conn_state.clone(), sessions, silent));
            }
            let agent_json = if mode == SidebarMode::Activity { serde_json::to_string(&st.subagents.entries).unwrap_or_default() } else { String::new() };
            let standalone = resolved_screens.as_ref().map(|r| r.standalone_by_host.clone()).unwrap_or_default();
            let standalone_desk = resolved_screens.map(|r| r.standalone_desktop.clone()).unwrap_or_default();
            (mode, hosts_snap, activity_rows, standalone, standalone_desk, blocked_total, agent_json)
        };

        let back = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
        })
        .paint_key(2)
        .absolute()
        .inset_0();

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
        // the names and the count.
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
            WinMsg::Redraw
        }));
        let header = div()
            .relative()
            .w_full()
            .h(px(widgets::STRIP_HEIGHT))
            .flex_shrink_0()
            // Opus Minor: align `Sessions | Activity` labels closely with the
            // `olo-rex / server-logs` tab labels on the same 34 pt strip while
            // keeping a clean 2 pt gap above the bottom rule (`7 + 24 + 2 + 1 = 34`).
            .pt(px(7.0))
            .px(px(m::SPACING))
            .flex()
            .flex_row()
            .items_start()
            .child(header_bg)
            .child(segments);

        let mut list = div().flex_shrink_0().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SPACING));
        let host_order_labels: Vec<(String, String)> = hosts_snap.iter().map(|(hk, hl, ..)| (hk.clone(), hl.clone())).collect();
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
                let mut group = div().w_full().flex_shrink_0().flex().flex_col().child(olo_kit::tip::on_row(row, tip_key, tip));
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
                        Some(words) => olo_kit::tip::on_row(row, tip_key, words),
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
        // Compute filtered Activity rows BEFORE rendering the filter bar so the
        // bar can show exact match counts ("1 of 5 shown") + a bevelled Show All button.
        let query_now = self.activity_search_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default();
        let query_terms: Vec<String> = query_now.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
        let filter_active = !query_terms.is_empty() || self.activity_type_filter != ActivityTypeFilter::All || self.activity_state_filter != ActivityStateFilter::All;

        let mut total_activity_entities = 0usize;
        let mut matched_activity_entities = 0usize;
        type FilteredActivityGroup = (ActivityRow, Vec<crate::activity_screens::ActivityScreenItem>, Vec<NativeChildItem>, usize, usize, usize, usize, bool, bool);
        let mut filtered_items: Vec<FilteredActivityGroup> = Vec::new();
        let mut filtered_standalone: HashMap<String, Vec<crate::activity_screens::ActivityScreenItem>> = HashMap::new();
        let mut filtered_standalone_desktop: Vec<crate::activity_screens::ActivityScreenItem> = Vec::new();
        let mut dyn_names: Vec<String> = Vec::new();
        if mode == SidebarMode::Activity {
            for row in &activity_rows {
                let child_screens_total: usize = row.children.iter().map(|c| c.screens.len()).sum();
                total_activity_entities += 1 + row.screens.len() + row.children.len() + child_screens_total;
                for child in &row.children {
                    let n = child.row.entry.role.as_deref().filter(|r| !r.trim().is_empty()).unwrap_or(child.row.entry.title.as_str()).trim();
                    if !n.is_empty()
                        && !matches!(n.to_ascii_lowercase().as_str(), "claude" | "codex" | "agy" | "antigravity" | "shell" | "subagent" | "screen" | "screens")
                        && !dyn_names.iter().any(|x| x.eq_ignore_ascii_case(n))
                    {
                        dyn_names.push(n.to_owned());
                    }
                }
            }
            for (hk, list) in &standalone_screens {
                total_activity_entities += list.len();
                let hlabel = host_order_labels.iter().find(|(k, _)| k == hk).map_or(hk.as_str(), |(_, l)| l.as_str());
                let kept: Vec<crate::activity_screens::ActivityScreenItem> =
                    list.iter().filter(|s| screen_matches_filter(s, hlabel, None, &query_terms, &self.activity_type_filter, self.activity_state_filter)).cloned().collect();
                matched_activity_entities += kept.len();
                if !kept.is_empty() {
                    filtered_standalone.insert(hk.clone(), kept);
                }
            }
            // Astra #19 / Neon #5: count and filter standalone desktop screens
            // separately against `"This computer"` when no `is_local` Rex host exists.
            if !standalone_desktop.is_empty() {
                total_activity_entities += standalone_desktop.len();
                filtered_standalone_desktop =
                    standalone_desktop.iter().filter(|s| screen_matches_filter(s, "This computer", None, &query_terms, &self.activity_type_filter, self.activity_state_filter)).cloned().collect();
                matched_activity_entities += filtered_standalone_desktop.len();
            }
            for row in activity_rows {
                if let Some(fm) = filter_activity_entry(&row, &query_now, &self.activity_type_filter, self.activity_state_filter) {
                    let total_c = fm.subs.len();
                    let total_screens = fm.screens.len() + fm.subs.iter().map(|s| s.screens.len()).sum::<usize>();
                    let failed_c = fm.subs.iter().filter(|s| s.act == AgentActivity::Error).count();
                    let waiting_c = fm.subs.iter().filter(|s| s.act == AgentActivity::Blocked).count();
                    if fm.parent_matched {
                        matched_activity_entities += 1;
                    }
                    matched_activity_entities += fm.screens.len();
                    for s in &fm.subs {
                        if !s.ancestor_only {
                            matched_activity_entities += 1;
                        }
                        matched_activity_entities += s.screens.len();
                    }
                    filtered_items.push((row, fm.screens, fm.subs, total_c, total_screens, failed_c, waiting_c, fm.force_expand, fm.parent_matched));
                }
            }
        }

        // Pinned Activity Search & Filter bar (below Sessions|Activity header, above scroller).
        let activity_filter_bar = (mode == SidebarMode::Activity).then(|| {
            let mut type_options: Vec<ActivityTypeFilter> =
                vec![ActivityTypeFilter::All, ActivityTypeFilter::Claude, ActivityTypeFilter::Codex, ActivityTypeFilter::Agy, ActivityTypeFilter::Shell, ActivityTypeFilter::Screens];
            dyn_names.sort_by_key(|s| s.to_ascii_lowercase());
            for dn in dyn_names {
                type_options.push(ActivityTypeFilter::SubagentType(dn));
            }
            if let ActivityTypeFilter::SubagentType(ref cur) = self.activity_type_filter
                && !type_options.iter().any(|o| o == &self.activity_type_filter)
            {
                type_options.push(ActivityTypeFilter::SubagentType(cur.clone()));
            }
            let type_selected = type_options.iter().position(|o| o == &self.activity_type_filter).unwrap_or(0);
            let type_labels: Vec<olo_kit::SharedString> = type_options.iter().map(|o| olo_kit::SharedString::from(o.label())).collect();
            let type_opts_cb = type_options.clone();

            let state_selected = ActivityStateFilter::ALL_VARIANTS.iter().position(|&s| s == self.activity_state_filter).unwrap_or(0);
            let state_labels: Vec<olo_kit::SharedString> = ActivityStateFilter::ALL_VARIANTS.iter().map(|s| olo_kit::SharedString::from(olo_kit::words::tr(s.label()).to_string())).collect();

            let inner_w = (width - 2.0 * m::SPACING).max(120.0);
            let half_w = ((inner_w - m::TIGHT) / 2.0).floor().max(56.0);

            let mut search_row = div().w_full().flex().flex_row().items_center();
            if let Some(sf) = &self.activity_search_field {
                search_row = search_row.child(div().flex_1().min_w_0().child(TextControl::new(cx, sf)));
            }

            let type_menu = widgets::MenuField::new("rex-act-type-filter", type_labels, type_selected)
                .name("Type")
                .width(half_w)
                .on_change(cx.msg_with(move |idx: &usize| WinMsg::SetActivityTypeFilter(type_opts_cb.get(*idx).cloned().unwrap_or(ActivityTypeFilter::All))));
            let state_menu = widgets::MenuField::new("rex-act-state-filter", state_labels, state_selected)
                .name("State")
                .width(half_w)
                .on_change(cx.msg_with(move |idx: &usize| WinMsg::SetActivityStateFilter(ActivityStateFilter::ALL_VARIANTS.get(*idx).copied().unwrap_or(ActivityStateFilter::All))));

            let active_bar = filter_active.then(|| {
                let count_text = olo_kit::tr!("{} of {} shown", matched_activity_entities, total_activity_entities).to_string();
                let count_key = style::key(("act-filter-count", &count_text));
                let count_label = canvas(move |g, b| {
                    let p = theme::palette();
                    let baseline = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
                    let r = style::label_on_baseline(g, b.left(), baseline, style::Align::Left, count_text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
                    g.agent(agent::Node::label(count_text.clone(), r));
                })
                .paint_key(count_key)
                .w_full()
                .h(px(m::CONTROL_HEIGHT));
                let clear_cb = cx.msg(WinMsg::ClearActivityFilters);
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(div().flex_1().min_w_0().child(count_label))
                    .child(Button::new(olo_kit::words::tr("Show All").to_owned()).id("rex-act-clear-filter").min_width(0.0).on_click(move |_, cx| clear_cb(&(), cx)))
            });

            let filter_bg = canvas(|g, b| {
                let p = theme::palette();
                let (shadow, _) = bevel_colors(&p);
                style::hline_above(g, b.left(), b.right(), b.bottom(), shadow);
            })
            .paint_key(203)
            .absolute()
            .inset_0();

            // Opus Minor: crisp 9 / 6 / 6 / 9 pt vertical rhythm in filter bar.
            div()
                .relative()
                .w_full()
                .flex_shrink_0()
                .px(px(m::SPACING))
                .pt(px(9.0))
                .pb(px(9.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(filter_bg)
                .child(search_row)
                .child(div().w_full().flex().flex_row().items_center().gap(px(m::TIGHT)).child(type_menu).child(state_menu))
                .children(active_bar)
        });

        if mode == SidebarMode::Activity {
            let mut active_bounds_keys: HashSet<String> = HashSet::new();
            let any_visible = !filtered_items.is_empty() || !filtered_standalone.is_empty() || !filtered_standalone_desktop.is_empty();
            let any_screens = filtered_items.iter().any(|(_, ds, subs, ..)| !ds.is_empty() || subs.iter().any(|c| !c.screens.is_empty()))
                || !filtered_standalone.is_empty()
                || !filtered_standalone_desktop.is_empty();
            if !any_visible {
                if filter_active {
                    let clear_cb = cx.msg(WinMsg::ClearActivityFilters);
                    let empty_msg = if !query_now.trim().is_empty() {
                        olo_kit::tr!("No agents or screens match “{}”.", query_now.trim()).to_string()
                    } else {
                        olo_kit::words::tr("No matching agents or screens.").to_owned()
                    };
                    list = list.child(
                        div()
                            .w_full()
                            .pt(px(24.0))
                            .pb(px(m::SECTION))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(10.0))
                            .child(StringView::new(empty_msg).dim(true).wrap(true))
                            .child(Button::new(olo_kit::words::tr("Show All").to_owned()).small().min_width(0.0).on_click(move |_, cx| clear_cb(&(), cx))),
                    );
                } else {
                    list = list.child(div().py(px(m::SECTION)).child(StringView::new("All agents and shells are quiet.").dim(true)));
                }
            }
            let active_view = crate::activity_watch::active_view_target();
            let win_id = self.id;
            // Group Activity rows and standalone running screens by host in
            // canonical `host_order` under clean section headers (aligned flush
            // with the sidebar filter controls above).
            let mut rows = div().w_full().flex_shrink_0().flex().flex_col().agent("Group", "activity-subagents").agent_value(native_agent_json);
            let mut items_by_host: HashMap<String, Vec<_>> = HashMap::new();
            for entry in filtered_items {
                items_by_host.entry(entry.0.host_key.clone()).or_default().push(entry);
            }
            let mut rendered_any_host = false;
            for (hk, host_label) in &host_order_labels {
                let host_agents = items_by_host.remove(hk).unwrap_or_default();
                let host_standalones = filtered_standalone.remove(hk).unwrap_or_default();
                if host_agents.is_empty() && host_standalones.is_empty() {
                    continue;
                }
                let host_pin = capture_live_host_pin(the_hub(), hk);
                let top_pad = if rendered_any_host { 10.0 } else { 0.0 };
                rows = rows.child(div().w_full().pt(px(top_pad)).pb(px(6.0)).pr(px(m::TIGHT)).child(StringView::new(host_label.clone()).bold(true).size(style::SMALL_SIZE).dim(true).height(14.0)));
                rendered_any_host = true;

                for (row, direct_screens, subs, total_children, total_screens, failed_children, waiting_children, force_expand, parent_matched) in host_agents {
                    let has_children = total_children > 0 || total_screens > 0;
                    let expanded = has_children && (force_expand || !crate::activity_watch::is_folded(&row.bk));
                    let ancestor_only = filter_active && !parent_matched;
                    let bk_jump = row.bk.clone();
                    let bk_fold = row.bk.clone();
                    let bk_menu = row.bk.clone();
                    let row_title = row.title.clone();
                    let row_help = row.help.clone();
                    let viewing_tr = matches!(&active_view, Some(crate::activity_watch::ViewTarget::PaneTranscript { bk, .. }) if bk == &row.bk);
                    let screen_in_collapsed = has_children
                        && !expanded
                        && active_screen_id.as_ref().is_some_and(|aid| direct_screens.iter().any(|s| &s.id == aid) || subs.iter().any(|c| c.screens.iter().any(|s| &s.id == aid)));
                    let active_in_collapsed = has_children && !expanded && (subs.iter().any(|c| shown.as_ref() == Some(&c.bk)) || screen_in_collapsed);
                    let selected = !ancestor_only && (shown.as_ref() == Some(&row.bk) || active_in_collapsed);
                    // Opus #8: when collapsed with hidden urgent children, elevate the
                    // parent's icon badge to its most urgent hidden child (Error > Blocked).
                    let icon_act = if !ancestor_only && has_children && !expanded {
                        let hidden_urg = if failed_children > 0 {
                            AgentActivity::Error
                        } else if waiting_children > 0 {
                            AgentActivity::Blocked
                        } else {
                            AgentActivity::Idle
                        };
                        higher_activity(row.act, hidden_urg)
                    } else {
                        row.act
                    };
                    let running_kids: Vec<RunningChild> = subs
                        .iter()
                        .filter(|c| c.command || matches!(c.act, AgentActivity::Working | AgentActivity::Blocked | AgentActivity::Error))
                        .map(|c| RunningChild { window_id: c.row.entry.window_id.clone(), name: c.row.entry.display_title(), command: c.command, minder: c.row.entry.is_shared_minder() })
                        .collect();
                    let has_sub_children = total_children > 0;
                    let parent_menu_key = format!("row:{}:{}:{}", row.bk.host_key, row.bk.session_id, row.bk.block_id);
                    active_bounds_keys.insert(parent_menu_key.clone());
                    let parent_menu_target = self.context_menu_row.as_deref() == Some(parent_menu_key.as_str());
                    let parent_tip_key = format!("rex-act-row-{}-{}-{}", row.bk.host_key, row.bk.session_id, row.bk.block_id);
                    let mut parent_tip_text = if row.detail.is_empty() { row_title.clone() } else { format!("{row_title} — {}", row.detail) };
                    if row_help.keep_going {
                        parent_tip_text.push_str(" · Keep It Going");
                    }
                    olo_kit::tip::retext(cx, parent_tip_key.clone(), parent_tip_text.clone());
                    let parent_bounds = self.sidebar_row_bounds.clone();
                    let parent_bounds_menu = self.sidebar_row_bounds.clone();
                    let parent_bounds_key = parent_menu_key.clone();
                    let parent_bounds_key_menu = parent_menu_key.clone();
                    let parent_el = olo_kit::tip::on_row(
                        sidebar_activity_row(
                            row,
                            icon_act,
                            selected,
                            parent_menu_target,
                            ancestor_only,
                            has_children,
                            expanded,
                            total_children,
                            total_screens,
                            failed_children,
                            waiting_children,
                            &query_terms,
                            width - 2.0 * m::SPACING,
                            parent_bounds,
                            parent_bounds_key,
                            cx.msg_with(move |_: &()| WinMsg::FocusBlock(bk_jump.clone())),
                            cx.msg_with(move |_: &()| WinMsg::ToggleActivityFold(bk_fold.clone())),
                        )
                        .on_mouse_down(MouseButton::Right, move |e, cx| {
                            let bk = bk_menu.clone();
                            let title = row_title.clone();
                            let running_for_menu = running_kids.clone();
                            let mut menu = Menu::new("")
                                .item("Show Transcript", "row.transcript")
                                .checked(viewing_tr)
                                .item("Show Terminal", "row.terminal")
                                .checked(!viewing_tr)
                                .separator()
                                .item("Keep It Going  ↻", "row.keep_going")
                                .checked(row_help.keep_going)
                                .item("Auto Approve", "row.auto_approve")
                                .checked(row_help.auto_approve);
                            if has_children {
                                let fold_word = match (has_sub_children, total_screens > 0, expanded) {
                                    (true, _, true) => "Collapse Subagents",
                                    (true, _, false) => "Expand Subagents",
                                    (false, true, true) => "Collapse Screens",
                                    (false, true, false) => "Expand Screens",
                                    _ => "Collapse",
                                };
                                menu = menu.separator().item(fold_word, "row.toggle_fold");
                            }
                            if has_sub_children {
                                let stop_label = stop_children_label(&running_for_menu);
                                menu = menu.separator().item(stop_label, "row.stop_all").enabled(!running_for_menu.is_empty());
                            }
                            menu::resolve(std::slice::from_mut(&mut menu), true);
                            if let Some(slot) = wins().map.get(&win_id) {
                                slot.handle.messenger().send(WinMsg::SetContextMenuRow(Some(parent_menu_key.clone())));
                            }
                            let menu_at = parent_bounds_menu.borrow().get(&parent_bounds_key_menu).copied().map_or(e.position, |b| point(e.position.x.max(b.left() + px(12.0)), b.bottom() + px(2.0)));
                            menu::context_menu_then(
                                cx,
                                menu,
                                menu_at,
                                move |cmd, _| match cmd {
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
                                    "row.keep_going" => {
                                        the_hub().set_help(&bk, |h| h.keep_going = !h.keep_going);
                                    }
                                    "row.auto_approve" => {
                                        toggle_auto_approve(the_hub(), &bk, &title);
                                    }
                                    "row.toggle_fold" => {
                                        if let Some(slot) = wins().map.get(&win_id) {
                                            slot.handle.messenger().send(WinMsg::ToggleActivityFold(bk.clone()));
                                        }
                                    }
                                    "row.stop_all" if !running_for_menu.is_empty() => {
                                        if let Some(slot) = wins().map.get(&win_id) {
                                            slot.handle.messenger().send(WinMsg::ConfirmStopSubagents {
                                                host_key: bk.host_key.clone(),
                                                host_pin,
                                                session_id: bk.session_id.clone(),
                                                children: running_for_menu.clone(),
                                            });
                                        }
                                    }
                                    _ => {}
                                },
                                move |_| {
                                    if let Some(slot) = wins().map.get(&win_id) {
                                        slot.handle.messenger().send(WinMsg::SetContextMenuRow(None));
                                    }
                                },
                            );
                            cx.stop_propagation();
                        }),
                        parent_tip_key,
                        parent_tip_text,
                    );
                    rows = rows.child(parent_el);

                    if expanded {
                        // 1. Direct screens of this top-level agent appear FIRST,
                        //    immediately below the agent and before its child subagents.
                        let direct_len = direct_screens.len();
                        for (s_idx, mut scr) in direct_screens.into_iter().enumerate() {
                            scr.depth = 1;
                            scr.is_last = s_idx + 1 == direct_len && subs.is_empty();
                            scr.guides = Vec::new();
                            let is_sel = active_screen_id.as_ref() == Some(&scr.id);
                            let you_ctrl = is_sel && active_screen_controlling;
                            let scr_menu_key = format!("scr:{}", scr.id.key());
                            active_bounds_keys.insert(scr_menu_key.clone());
                            let scr_menu_target = self.context_menu_row.as_deref() == Some(scr_menu_key.as_str());
                            rows = rows.child(render_activity_screen_row_with_menu(
                                scr,
                                is_sel,
                                scr_menu_target,
                                you_ctrl,
                                filter_active,
                                &query_terms,
                                width - 2.0 * m::SPACING,
                                self.sidebar_row_bounds.clone(),
                                win_id,
                                cx,
                            ));
                        }

                        // 2. Child & grandchild subagents, each with its own screens
                        //    immediately beneath it before its child subagents.
                        let mut folded_depth: Option<usize> = None;
                        for (c_idx, child) in subs.iter().enumerate() {
                            if folded_depth.is_some_and(|fd| child.row.depth > fd) {
                                continue;
                            }
                            folded_depth = None;
                            let sub_end = subs[c_idx + 1..].iter().position(|n| n.row.depth <= child.row.depth).map_or(subs.len(), |off| c_idx + 1 + off);
                            let sub_kids: &[NativeChildItem] = &subs[c_idx + 1..sub_end];
                            let child_has_sub_kids = !sub_kids.is_empty();
                            let child_has_kids = child_has_sub_kids || !child.screens.is_empty();
                            let child_expanded = child_has_kids && (force_expand || !crate::activity_watch::is_folded(&child.bk));
                            if child_has_kids && !child_expanded {
                                folded_depth = Some(child.row.depth);
                            }
                            let sub_total = sub_kids.len();
                            let sub_screens_total = child.screens.len() + sub_kids.iter().map(|k| k.screens.len()).sum::<usize>();
                            let sub_failed = sub_kids.iter().filter(|k| k.act == AgentActivity::Error).count();
                            let sub_waiting = sub_kids.iter().filter(|k| k.act == AgentActivity::Blocked).count();
                            let active_scr_in_folded = child_has_kids
                                && !child_expanded
                                && active_screen_id.as_ref().is_some_and(|aid| child.screens.iter().any(|s| &s.id == aid) || sub_kids.iter().any(|k| k.screens.iter().any(|s| &s.id == aid)));
                            let active_in_folded_child = child_has_kids && !child_expanded && (sub_kids.iter().any(|k| shown.as_ref() == Some(&k.bk)) || active_scr_in_folded);
                            // Opus #8: when a child parent (e.g. `API Audit`) is collapsed with
                            // urgent descendants, elevate its icon badge to Error > Blocked!
                            let child_icon_act = if !child.ancestor_only && child_has_kids && !child_expanded {
                                let hidden_urg = if sub_failed > 0 {
                                    AgentActivity::Error
                                } else if sub_waiting > 0 {
                                    AgentActivity::Blocked
                                } else {
                                    AgentActivity::Idle
                                };
                                higher_activity(child.act, hidden_urg)
                            } else {
                                child.act
                            };

                            // Recompute exact `is_last` & ancestor `guides` against the filtered `subs` list
                            // so filtered trees never draw dangling vertical guide lines.
                            let mut child_for_draw = child.clone();
                            child_for_draw.has_children = child_has_kids;
                            let (c_is_last, c_guides) = recompute_sub_guides(&subs, c_idx);
                            child_for_draw.row.is_last = c_is_last;
                            child_for_draw.row.guides = c_guides.clone();

                            let child_bk_click = child.bk.clone();
                            let child_bk_fold = child.bk.clone();
                            let child_bk_menu = child.bk.clone();
                            let child_wid_menu = child.row.entry.window_id.clone();
                            let child_minds = child.row.entry.is_shared_minder();
                            let child_title = child.row.entry.display_title();
                            let child_help = child.help.clone();
                            let viewing_child_tr = matches!(&active_view, Some(crate::activity_watch::ViewTarget::PaneTranscript { bk, .. }) if bk == &child.bk);
                            let sub_selected = !child.ancestor_only && (shown.as_ref() == Some(&child.bk) || active_in_folded_child);
                            let sub_menu_key = format!("sub:{}", child.row.entry.id);
                            active_bounds_keys.insert(sub_menu_key.clone());
                            let sub_menu_target = self.context_menu_row.as_deref() == Some(sub_menu_key.as_str());
                            let tip_key = format!("rex-sub-{}", child.row.entry.id);
                            let mut tip_text = format!("{} ({}) — Click to open terminal", child_title, child.row.entry.what());
                            if child_help.keep_going {
                                tip_text.push_str(" · Keep It Going");
                            }
                            olo_kit::tip::retext(cx, tip_key.clone(), tip_text.clone());
                            let sub_bounds = self.sidebar_row_bounds.clone();
                            let sub_bounds_menu = self.sidebar_row_bounds.clone();
                            let sub_bounds_key = sub_menu_key.clone();
                            let sub_bounds_key_menu = sub_menu_key.clone();
                            let sub_el = olo_kit::tip::on_row(
                                sidebar_native_child_row(
                                    child_for_draw,
                                    child_icon_act,
                                    sub_selected,
                                    sub_menu_target,
                                    child_expanded,
                                    (sub_total, sub_screens_total, sub_failed, sub_waiting),
                                    &query_terms,
                                    width - 2.0 * m::SPACING,
                                    sub_bounds,
                                    sub_bounds_key,
                                    cx.msg_with(move |_: &()| WinMsg::FocusBlock(child_bk_click.clone())),
                                    cx.msg_with(move |_: &()| WinMsg::ToggleActivityFold(child_bk_fold.clone())),
                                )
                                .on_mouse_down(MouseButton::Right, move |e, cx| {
                                    let bk = child_bk_menu.clone();
                                    let wid = child_wid_menu.clone();
                                    let title = child_title.clone();
                                    let minds = child_minds;
                                    let child_has_agents = minds && the_hub().minder_and_its_agents(&bk.host_key, &bk.session_id, &wid).len() > 1;
                                    let mut menu = Menu::new("")
                                        .item("Show Transcript", "sub.transcript")
                                        .checked(viewing_child_tr)
                                        .item("Show Terminal", "sub.terminal")
                                        .checked(!viewing_child_tr)
                                        .separator()
                                        .item("Keep It Going  ↻", "sub.keep_going")
                                        .checked(child_help.keep_going)
                                        .item("Auto Approve", "sub.auto_approve")
                                        .checked(child_help.auto_approve)
                                        .separator();
                                    if child_has_kids {
                                        let fold_word = match (child_has_sub_kids, child_expanded) {
                                            (true, true) => "Collapse Subagents",
                                            (true, false) => "Expand Subagents",
                                            (false, true) => "Collapse Screens",
                                            (false, false) => "Expand Screens",
                                        };
                                        menu = menu.item(fold_word, "sub.toggle_fold").separator();
                                    }
                                    menu = menu.item(if minds { "Stop Minder…" } else { "Stop Subagent" }, "sub.stop");
                                    if (minds && child_has_agents) || (!minds && child_has_sub_kids) {
                                        menu = menu.item(if minds { "Stop Minder & Its Subagents…" } else { "Stop Subagent & Children" }, "sub.stop_tree");
                                    }
                                    menu = menu.separator().item("Force Close Tab", "sub.force_stop");
                                    menu::resolve(std::slice::from_mut(&mut menu), true);
                                    if let Some(slot) = wins().map.get(&win_id) {
                                        slot.handle.messenger().send(WinMsg::SetContextMenuRow(Some(sub_menu_key.clone())));
                                    }
                                    let menu_at =
                                        sub_bounds_menu.borrow().get(&sub_bounds_key_menu).copied().map_or(e.position, |b| point(e.position.x.max(b.left() + px(12.0)), b.bottom() + px(2.0)));
                                    menu::context_menu_then(
                                        cx,
                                        menu,
                                        menu_at,
                                        move |cmd, _| match cmd {
                                            "sub.terminal" => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::FocusBlock(bk.clone()));
                                                }
                                            }
                                            "sub.transcript" => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::OpenPaneTranscript(bk.clone(), title.clone()));
                                                }
                                            }
                                            "sub.keep_going" => {
                                                the_hub().set_help(&bk, |h| h.keep_going = !h.keep_going);
                                            }
                                            "sub.auto_approve" => {
                                                toggle_auto_approve(the_hub(), &bk, &title);
                                            }
                                            "sub.toggle_fold" => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::ToggleActivityFold(bk.clone()));
                                                }
                                            }
                                            "sub.stop" if minds => {
                                                // Astra #1: use `host_pin` captured with the row when building
                                                // the menu (never re-reading a replacement host at click time).
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::ConfirmStopMinder {
                                                        host_key: bk.host_key.clone(),
                                                        host_pin,
                                                        session_id: bk.session_id.clone(),
                                                        window_id: wid.clone(),
                                                        title: title.clone(),
                                                        with_agents: false,
                                                    });
                                                }
                                            }
                                            "sub.stop" => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::StopSubagent {
                                                        host_key: bk.host_key.clone(),
                                                        host_pin,
                                                        session_id: bk.session_id.clone(),
                                                        window_id: wid.clone(),
                                                        force: false,
                                                        tree: false,
                                                    });
                                                }
                                            }
                                            "sub.stop_tree" if minds => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::ConfirmStopMinder {
                                                        host_key: bk.host_key.clone(),
                                                        host_pin,
                                                        session_id: bk.session_id.clone(),
                                                        window_id: wid.clone(),
                                                        title: title.clone(),
                                                        with_agents: true,
                                                    });
                                                }
                                            }
                                            "sub.stop_tree" => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::StopSubagent {
                                                        host_key: bk.host_key.clone(),
                                                        host_pin,
                                                        session_id: bk.session_id.clone(),
                                                        window_id: wid.clone(),
                                                        force: false,
                                                        tree: true,
                                                    });
                                                }
                                            }
                                            "sub.force_stop" => {
                                                if let Some(slot) = wins().map.get(&win_id) {
                                                    slot.handle.messenger().send(WinMsg::StopSubagent {
                                                        host_key: bk.host_key.clone(),
                                                        host_pin,
                                                        session_id: bk.session_id.clone(),
                                                        window_id: wid.clone(),
                                                        force: true,
                                                        tree: false,
                                                    });
                                                }
                                            }
                                            _ => {}
                                        },
                                        move |_| {
                                            if let Some(slot) = wins().map.get(&win_id) {
                                                slot.handle.messenger().send(WinMsg::SetContextMenuRow(None));
                                            }
                                        },
                                    );
                                    cx.stop_propagation();
                                }),
                                tip_key,
                                tip_text,
                            );
                            rows = rows.child(sub_el);

                            // Nested screens owned by this child/grandchild subagent appear
                            // immediately below it (before any deeper subagent children).
                            if child_expanded && !child.screens.is_empty() {
                                let scr_depth = (child.row.depth + 1).clamp(1, 4);
                                let mut scr_guides = c_guides;
                                let want_len = scr_depth.saturating_sub(1);
                                while scr_guides.len() < want_len {
                                    scr_guides.push(!c_is_last);
                                }
                                scr_guides.truncate(want_len);
                                // Neon #11.2: only assign `!c_is_last` to the new guide slot when
                                // `scr_depth` actually increased past `child.row.depth` (not when
                                // clamped at depth 4, which would overwrite depth 3's guide).
                                if scr_depth > child.row.depth
                                    && let Some(last_g) = scr_guides.last_mut()
                                {
                                    *last_g = !c_is_last;
                                }
                                let n_scr = child.screens.len();
                                for (s_idx, mut scr) in child.screens.iter().cloned().enumerate() {
                                    scr.depth = scr_depth;
                                    scr.is_last = s_idx + 1 == n_scr && !child_has_sub_kids;
                                    scr.guides = scr_guides.clone();
                                    let is_sel = active_screen_id.as_ref() == Some(&scr.id);
                                    let you_ctrl = is_sel && active_screen_controlling;
                                    let scr_menu_key = format!("scr:{}", scr.id.key());
                                    active_bounds_keys.insert(scr_menu_key.clone());
                                    let scr_menu_target = self.context_menu_row.as_deref() == Some(scr_menu_key.as_str());
                                    rows = rows.child(render_activity_screen_row_with_menu(
                                        scr,
                                        is_sel,
                                        scr_menu_target,
                                        you_ctrl,
                                        filter_active,
                                        &query_terms,
                                        width - 2.0 * m::SPACING,
                                        self.sidebar_row_bounds.clone(),
                                        win_id,
                                        cx,
                                    ));
                                }
                            }
                        }
                    }
                }

                // Standalone running screens appear at depth 0 after this host's agents.
                for mut scr in host_standalones {
                    scr.depth = 0;
                    scr.is_last = true;
                    scr.guides = Vec::new();
                    let is_sel = active_screen_id.as_ref() == Some(&scr.id);
                    let you_ctrl = is_sel && active_screen_controlling;
                    let scr_menu_key = format!("scr:{}", scr.id.key());
                    active_bounds_keys.insert(scr_menu_key.clone());
                    let scr_menu_target = self.context_menu_row.as_deref() == Some(scr_menu_key.as_str());
                    rows = rows.child(render_activity_screen_row_with_menu(
                        scr,
                        is_sel,
                        scr_menu_target,
                        you_ctrl,
                        filter_active,
                        &query_terms,
                        width - 2.0 * m::SPACING,
                        self.sidebar_row_bounds.clone(),
                        win_id,
                        cx,
                    ));
                }
            }
            // Astra #19 / Neon #5: when no `is_local` Rex server exists, render standalone
            // desktop screens under their own `"This computer"` section so they neither
            // merge into an SSH home `"local"` host nor disappear.
            if !filtered_standalone_desktop.is_empty() {
                let top_pad = if rendered_any_host { 10.0 } else { 0.0 };
                rows = rows.child(
                    div()
                        .w_full()
                        .pt(px(top_pad))
                        .pb(px(6.0))
                        .pr(px(m::TIGHT))
                        .child(StringView::new(olo_kit::words::tr("This computer").to_owned()).bold(true).size(style::SMALL_SIZE).dim(true).height(14.0)),
                );
                for mut scr in filtered_standalone_desktop {
                    scr.depth = 0;
                    scr.is_last = true;
                    scr.guides = Vec::new();
                    let is_sel = active_screen_id.as_ref() == Some(&scr.id);
                    let you_ctrl = is_sel && active_screen_controlling;
                    let scr_menu_key = format!("scr:{}", scr.id.key());
                    active_bounds_keys.insert(scr_menu_key.clone());
                    let scr_menu_target = self.context_menu_row.as_deref() == Some(scr_menu_key.as_str());
                    rows = rows.child(render_activity_screen_row_with_menu(
                        scr,
                        is_sel,
                        scr_menu_target,
                        you_ctrl,
                        filter_active,
                        &query_terms,
                        width - 2.0 * m::SPACING,
                        self.sidebar_row_bounds.clone(),
                        win_id,
                        cx,
                    ));
                }
            }
            // Astra #26: prune retired row bounds entries so the map never grows unbounded.
            self.sidebar_row_bounds.borrow_mut().retain(|k, _| active_bounds_keys.contains(k));
            // Opus #10, #16 / Neon-look #11: align flush with section headers (12 pt = `pl(0)`
            // inside `list`'s 12 pt padding), 12 pt below the last row, with a bold primary-ink
            // title cleanly distinguished from the secondary body copy 6 pt below.
            if !filter_active && any_visible && !any_screens {
                rows = rows.child(
                    div()
                        .w_full()
                        .pt(px(12.0))
                        .pb(px(4.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(StringView::new(olo_kit::words::tr("No screens running").to_owned()).bold(true).size(style::SMALL_SIZE).height(15.0))
                        .child(StringView::new(olo_kit::words::tr("Screens appear here when an agent opens one.").to_owned()).size(style::SMALL_SIZE).dim(true).wrap(true)),
                );
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
            .children(activity_filter_bar)
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
        let hub = the_hub().clone();
        match self.role {
            WinRole::Hosts => {
                let hub_r = hub.clone();
                let hub_d = hub.clone();
                let window = self.id;
                let hub_f = hub.clone();
                let hub_s = hub.clone();
                let askpass = the_askpass().clone();
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
                // The host's screens for the Screens tab's list to draw
                // from; the one chosen kept by its name (a screen that
                // stops goes down the list, and stays the one chosen).
                self.host_screen_lines = Rc::new(crate::host_panel::screen_lines(&stands, &shown));
                let chosen = self.host_screen_chosen.as_ref().and_then(|name| self.host_screen_lines.iter().position(|l| &l.row.name == name));
                if chosen.is_none() {
                    self.host_screen_chosen = None;
                }
                if self.host_screens_list.state().single() != chosen {
                    match chosen {
                        Some(i) => self.host_screens_list.state_mut().select(i),
                        None => self.host_screens_list.state_mut().selected.clear(),
                    }
                }
                let (lines, list) = (self.host_screen_lines.clone(), self.host_screens_list.clone());
                let can_live_stream = hub.state.lock().unwrap_or_else(|e| e.into_inner()).host_screens.get(&shown.key).is_some_and(crate::activity_screens::host_supports_live_stream);
                let screens = crate::host_panel::screens_section(cx, &list, &lines, &stands, &shown, can_live_stream, cx.msg_with(|what: &crate::host_panel::HostDo| WinMsg::HostDo(what.clone())));
                let tweaks = crate::server::for_tweaks(crate::server::current);
                div().w_full().child(crate::host_panel::render_host_panel(
                    &stands,
                    &shown,
                    managed,
                    screens,
                    tweaks.as_ref(),
                    self.setup_phase,
                    cx.msg_with(|what: &crate::host_panel::HostDo| WinMsg::HostDo(what.clone())),
                ))
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
                let req = the_askpass().current_request();
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
                let prompt = the_run_approvals().current();
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
                // (What Try Again started: the problem it is trying again
                // stays shown until this says how it went.)
                let kept = if self.retrying { self.problem.take() } else { None };
                self.clear_feedback();
                self.feedback_serial = serial;
                self.retrying = kept.is_some();
                self.problem = kept;
                cx.invalidate();
            }
            WinMsg::Problem(serial, title, detail) => {
                if self.feedback_serial == serial {
                    self.cancel_preview();
                    let mono_size = style::SMALL_SIZE - 1.0;
                    let view = detail.map(|s| {
                        let est_lines = s.lines().map(|l| l.chars().count().max(1).div_ceil(88)).sum::<usize>().clamp(1, 3);
                        let tv = widgets::text_view(cx, |v| v.panel_well(true).mono(true).size(mono_size).with_text(s, None).auto_hide(true));
                        (tv, est_lines)
                    });
                    self.problem = Some((title, view));
                    self.retrying = false;
                    cx.invalidate();
                }
            }
            WinMsg::Confirmation(serial, title) => {
                if self.feedback_serial == serial {
                    // (It went well this time: the problem tried again is gone.)
                    if std::mem::take(&mut self.retrying) {
                        self.problem = None;
                    }
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
                self.exit_screen_for_navigation(cx);
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
                    if std::mem::take(&mut self.retrying) {
                        self.problem = None;
                    }
                    self.created_session = Some((hk, sid));
                    cx.invalidate();
                }
            }
            WinMsg::CreateSession(hk, dir) => {
                // Astra #27: close any embedded screen (and clear pending restore)
                // so the new session's terminal is never hidden behind the old picture.
                self.exit_screen_for_navigation(cx);
                let again = WinMsg::CreateSession(hk.clone(), dir.clone());
                the_hub().create_session(&hk, None, dir, Feedback::start(self.id).again(again));
            }
            WinMsg::Redraw => {
                if self.pending_restored_screen.is_some() {
                    self.try_restore_pending_screen(cx);
                }
                let visuals = &mut cx.window_state::<PaneVisuals>().0;
                if !visuals.is_empty() {
                    let hub = the_hub().clone();
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
                    let hub = the_hub().clone();
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
            WinMsg::RevealInTracker(bk, path) => {
                the_hub().reveal_path_in_tracker(&bk, path, Feedback::start(self.id));
                cx.invalidate();
            }
            WinMsg::DismissFeedback => {
                self.clear_feedback();
                cx.invalidate();
            }
            WinMsg::RetryWith(serial, with) => {
                if self.feedback_serial == serial {
                    self.retry = Some((serial, *with));
                    cx.invalidate();
                }
            }
            WinMsg::TryAgain => {
                // (The problem stays, its button dimmed, until the try
                // says how it went: `StartFeedback` keeps it.)
                if let Some((_, with)) = self.retry.take() {
                    self.retrying = true;
                    cx.send(with);
                    cx.invalidate();
                }
            }
            WinMsg::StopTransfer => {
                self.cancel_preview();
                cx.invalidate();
            }
            WinMsg::SetClipboard(txt) => {
                olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(txt));
            }
            WinMsg::PasteText(bk, connection, txt) => {
                if self.active_screen.is_none() && self.focused_block_key().as_ref() == Some(&bk) {
                    let hub = the_hub().clone();
                    let (is_local, bracketed) = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        let is_local = st.hosts.get(&bk.host_key).is_some_and(|h| h.is_local);
                        let bracketed = st.blocks.get(&bk).is_some_and(|b| b.lock().unwrap_or_else(|e| e.into_inner()).terminal.bracketed_paste_mode());
                        (is_local, bracketed)
                    };
                    // When pasting local desktop file paths into a remote Rex terminal,
                    // rewrite them to `rex://local/…` URIs so remote agents know they
                    // live on the desktop and can fetch them with `olo get`.
                    let pasted = if is_local {
                        txt
                    } else {
                        let home = crate::hub::local_home().map(PathBuf::from);
                        rewrite_local_paths_for_remote(&txt, home.as_deref(), |p| p.exists())
                    };
                    let encoded = encode_paste(&pasted, bracketed).unwrap_or_else(|_| pasted.into_bytes());
                    hub.write_input(&bk, encoded, Some(&connection));
                }
            }
            WinMsg::PasteImage(bk, mime, bytes) => {
                if self.active_screen.is_none() && self.focused_block_key().as_ref() == Some(&bk) {
                    let feedback = Feedback::start(self.id);
                    // (Written on a worker: a picture is megabytes.)
                    std::thread::spawn(move || match pasted_image_file(&mime, &bytes) {
                        Ok(path) => the_hub().upload_files(&bk, vec![path], feedback),
                        Err(e) => feedback.problem("Couldn’t paste the picture.", e),
                    });
                }
            }
            WinMsg::FocusBlock(bk) => {
                crate::activity_watch::close_viewer();
                self.pending_restored_screen = None;
                self.close_screen(cx);
                self.jump_to(JumpTarget::Pane(bk), cx);
                cx.activate_window();
            }
            WinMsg::ActivityChanged => {
                cx.invalidate();
            }
            WinMsg::ScreensMetaChanged => {
                self.sync_active_screen_info(cx);
                cx.invalidate();
            }
            WinMsg::ToggleActivityFold(bk) => {
                crate::activity_watch::toggle_fold(&bk);
                cx.invalidate();
            }
            WinMsg::SetActivityTypeFilter(tf) => {
                self.activity_type_filter = tf;
                cx.invalidate();
            }
            WinMsg::SetActivityStateFilter(sf) => {
                self.activity_state_filter = sf;
                cx.invalidate();
            }
            WinMsg::SetActivitySearch(query) => {
                if let Some(f) = &self.activity_search_field {
                    f.update(cx, |tf, cx| tf.set_text(query, cx));
                }
                cx.invalidate();
            }
            WinMsg::ClearActivityFilters => {
                if let Some(f) = &self.activity_search_field {
                    f.update(cx, |tf, cx| tf.set_text(String::new(), cx));
                }
                self.activity_type_filter = ActivityTypeFilter::All;
                self.activity_state_filter = ActivityStateFilter::All;
                cx.invalidate();
            }
            WinMsg::OpenScreen { id, tab, take_control, expected_host_instance_id, expected_started } => {
                self.open_screen(id, tab, take_control, expected_host_instance_id, expected_started, cx);
            }
            WinMsg::OpenScreenMenu(at) => {
                if let Some(pane) = &self.screen_pane {
                    pane.update(cx, |p, cx| p.open_screen_menu_at(at, cx));
                }
            }
            WinMsg::CloseScreen => {
                self.pending_restored_screen = None;
                self.close_screen(cx);
                self.focus.focus(cx);
            }
            WinMsg::ScreenStopConfirm(item) => {
                self.confirm_stop_screen(item, cx);
            }
            WinMsg::ScreenStart(item) => {
                self.start_screen_from_menu(item, cx);
            }
            WinMsg::ConfirmStopMinder { host_key, host_pin, session_id, window_id, title, with_agents } => {
                let hub = the_hub().clone();
                // Astra #1 & #5: require a valid live `(host_instance_id, conn_id)`
                // pin captured with the row (never wildcard `(0, 0)`), verify it
                // before opening the alert, and carry it into every stop dispatch.
                let Some(pin) = host_pin else { return };
                if !hub.host_matches_pin(&host_key, pin.0, pin.1) {
                    return;
                }
                let targets = if with_agents { hub.minder_and_its_agents(&host_key, &session_id, &window_id) } else { vec![window_id.clone()] };
                let agents_n = targets.len().saturating_sub(1);
                let (alert_title, msg, btn) = if with_agents && agents_n > 0 {
                    (
                        "Stop Minder and Its Subagents",
                        format!(
                            "Stop “{title}” and its {}?\n\nThis interrupts the minded subagents first, then stops the minder. Their tabs will close.",
                            olo_kit::words::count(agents_n as u64, "1 subagent", "{n} subagents")
                        ),
                        "Stop Minder & Its Subagents",
                    )
                } else {
                    ("Stop Minder", format!("Stop “{title}”?\n\nThis interrupts the minder and closes its tab. Any subagents it minds continue under their parent tab."), "Stop Minder")
                };
                olo_kit::widgets::open_alert(cx, alert_title, "warning", &msg, &[btn, "Cancel"], move |choice, _| {
                    if *choice == Some(0) {
                        if with_agents {
                            hub.stop_subagents_in_order_pinned(&host_key, Some(pin), &session_id, targets.clone());
                        } else {
                            hub.stop_subagent_pinned(&host_key, Some(pin), &session_id, &window_id, false, false);
                        }
                    }
                });
            }
            WinMsg::SetContextMenuRow(row_key) => {
                if self.context_menu_row != row_key {
                    self.context_menu_row = row_key;
                    cx.invalidate();
                }
            }
            WinMsg::TreeMenuClosed => {
                if self.open_tree_tab.take().is_some() {
                    cx.invalidate();
                }
            }
            WinMsg::SelectWindow(hk, sid, wid) => {
                crate::activity_watch::close_viewer();
                self.pending_restored_screen = None;
                self.close_screen(cx);
                self.active_host = hk.clone();
                self.active_session = Some(sid.clone());
                the_hub().select_window(&hk, &sid, &wid);
                if self.side_pane.alone() {
                    self.side_pane.show_rest();
                }
                self.focus.focus(cx);
                cx.invalidate();
            }
            WinMsg::StopSubagent { host_key, host_pin, session_id, window_id, force, tree } => {
                // Astra #1: require live host pin captured with the menu/row.
                let Some(pin) = host_pin else { return };
                if !the_hub().host_matches_pin(&host_key, pin.0, pin.1) {
                    return;
                }
                the_hub().stop_subagent_pinned(&host_key, Some(pin), &session_id, &window_id, force, tree);
                cx.invalidate();
            }
            WinMsg::ConfirmStopSubagents { host_key, host_pin, session_id, children } => {
                if children.is_empty() {
                    return;
                }
                let Some(pin) = host_pin else { return };
                if !the_hub().host_matches_pin(&host_key, pin.0, pin.1) {
                    return;
                }
                let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
                let running_wids = stop_order(&children);
                let commands = children.iter().filter(|c| c.command).count();
                let agents = children.len() - commands;
                let list = match names.as_slice() {
                    [a] => format!("“{a}”"),
                    [a, b] => format!("“{a}” and “{b}”"),
                    many => {
                        let head: Vec<String> = many[..many.len() - 1].iter().map(|s| format!("“{s}”")).collect();
                        format!("{}, and “{}”", head.join(", "), many[many.len() - 1])
                    }
                };
                // (Said by what they are: an agent is a subagent, a command
                // of the asker's own a command.)
                let (heading, body, btn) = match (agents, commands) {
                    (1, 0) => (format!("Stop {list}?"), format!("This interrupts {list}. Its tab will close."), "Stop Subagent"),
                    (0, 1) => (format!("Stop {list}?"), format!("This interrupts {list}. Its tab will close."), "Stop Command"),
                    (a, 0) => (format!("Stop {a} subagents?"), format!("This interrupts {list}.\nTheir tabs will close."), "Stop Subagents"),
                    (0, c) => (format!("Stop {c} commands?"), format!("This interrupts {list}.\nTheir tabs will close."), "Stop Commands"),
                    (a, c) => (
                        format!("Stop {a} subagent{} and {c} command{}?", if a == 1 { "" } else { "s" }, if c == 1 { "" } else { "s" }),
                        format!("This interrupts {list}.\nTheir tabs will close."),
                        "Stop All",
                    ),
                };
                let msg = format!("{heading}\n\n{body}");
                let hub = the_hub().clone();
                // Destructive action on the left (plain); Cancel on the right as the safe BeOS default (Return & Esc).
                // (The alert's own name, by what it would stop.)
                let named = match (agents, commands) {
                    (_, 0) => "Stop Subagents",
                    (0, _) => "Stop Commands",
                    _ => "Stop Subagents and Commands",
                };
                olo_kit::widgets::open_alert(cx, named, "warning", &msg, &[btn, "Cancel"], move |choice, _| {
                    if *choice == Some(0) {
                        // (One after the other, minders last: `stop_order`.)
                        hub.stop_subagents_in_order_pinned(&host_key, Some(pin), &session_id, running_wids.clone());
                    }
                });
            }
            WinMsg::OpenPaneTranscript(bk, title) => {
                let hub = the_hub().clone();
                self.pending_restored_screen = None;
                self.close_screen(cx);
                self.jump_to(JumpTarget::Pane(bk.clone()), cx);
                crate::activity_watch::open_pane_transcript(&hub, &bk, title);
                if self.side_pane.alone() {
                    self.side_pane.show_rest();
                }
                cx.invalidate();
            }
            WinMsg::TranscriptDo(what) => {
                use crate::transcript_view::TranscriptDo;
                match what {
                    TranscriptDo::ShowTerminal => {
                        crate::activity_watch::close_viewer();
                    }
                    TranscriptDo::ShowTranscript => {}
                    TranscriptDo::ToggleExpand(eid) => {
                        if let Some(tr) = crate::activity_watch::active_transcript() {
                            let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                            let is_failed_tool = g
                                .doc
                                .entries
                                .iter()
                                .any(|e| matches!(e, olo_rex_client::activity::TranscriptEntry::ToolCall { id, status: olo_rex_client::activity::ToolStatus::Error, .. } if *id == eid));
                            let is_running_tool = g
                                .doc
                                .entries
                                .iter()
                                .any(|e| matches!(e, olo_rex_client::activity::TranscriptEntry::ToolCall { id, status: olo_rex_client::activity::ToolStatus::Running, .. } if *id == eid));
                            if is_failed_tool {
                                if g.collapsed_errors.contains(&eid) {
                                    g.collapsed_errors.remove(&eid);
                                    g.expanded.insert(eid);
                                } else {
                                    g.expanded.remove(&eid);
                                    g.collapsed_errors.insert(eid);
                                }
                            } else if is_running_tool {
                                if g.collapsed_running.contains(&eid) {
                                    g.collapsed_running.remove(&eid);
                                    g.expanded.insert(eid);
                                } else {
                                    g.expanded.remove(&eid);
                                    g.collapsed_running.insert(eid);
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
                                g.paused_unseen = 0;
                                self.transcript_scroll.scroll_to_bottom(cx);
                            }
                            g.generation += 1;
                        }
                        cx.invalidate();
                    }
                    TranscriptDo::JumpToLive => {
                        if let Some(tr) = crate::activity_watch::active_transcript() {
                            let mut g = tr.lock().unwrap_or_else(|e| e.into_inner());
                            g.follow = true;
                            g.paused_unseen = 0;
                            g.generation += 1;
                        }
                        self.transcript_scroll.scroll_to_bottom(cx);
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
                self.pending_restored_screen = None;
                self.close_screen(cx);
                self.dismiss_overlay(cx);
                self.find_open = false;
                self.clear_feedback();
                self.pending_focus = None;
                self.focus.focus(cx);
                cx.invalidate();
            }
            WinMsg::SelectHost(hk) => {
                self.pending_restored_screen = None;
                self.close_screen(cx);
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
                self.pending_restored_screen = None;
                self.close_screen(cx);
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
                    && let Some(br) = the_hub().state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
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
                let hub = the_hub().clone();
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
                let hub = the_hub().clone();
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
            WinMsg::HostOpen(key) => open_host(the_hub(), &key),
            WinMsg::AccountOpen(key) => open_account(the_hub(), &key),
            WinMsg::HostChanged => {
                // (Another host's things in the window: its name in the title.)
                if self.role == WinRole::Host {
                    cx.set_window_title(host_title());
                }
                cx.invalidate();
            }
            WinMsg::TweaksChanged => {
                // (The busy bar's timer, as the Server window's: begun
                // with a wait and ended with it.)
                let stage = crate::server::for_tweaks(crate::server::current).map(|m| m.stage);
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
                let hub = the_hub().clone();
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
            WinMsg::HostChosen => {
                let row = self.host_screens_list.state().single();
                self.host_screen_chosen = row.and_then(|i| self.host_screen_lines.get(i)).map(|l| l.row.name.clone());
                cx.invalidate();
            }
            WinMsg::ReposDo(what) => {
                use crate::repos::ReposDo;
                let hub = the_hub().clone();
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
                let hub = the_hub().clone();
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
                let hub = the_hub().clone();
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
                let hub = the_hub().clone();
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
                    HostDo::Change(thing, op) => crate::host::change(&hub, thing, op),
                    HostDo::UpdateAll(things) => crate::host::update_all(&hub, &things),
                    HostDo::Check => {
                        crate::host::check(&hub);
                        // (On Tweaks, the host's settings are what is asked again.)
                        if crate::host::current().is_some_and(|shown| shown.tab == crate::host::TWEAKS)
                            && crate::server::for_tweaks(crate::server::current).is_some_and(|now| now.stage == crate::server::Stage::Shown)
                        {
                            crate::server::for_tweaks(|| crate::server::again(&hub));
                        }
                    }
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
                    HostDo::UpdateServer => {
                        if let Some(shown) = crate::host::current() {
                            open_server_for_update(&hub, &shown.key);
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
                    HostDo::Tab(tab) => {
                        crate::host::show_tab(tab);
                        begin_tweaks(&hub);
                    }
                    // (The Tweaks tab's: the Server window's own words,
                    // of the Host window's record of the host.)
                    HostDo::Tweak(what) => {
                        use crate::server_panel::ServerDo;
                        match what {
                            ServerDo::Again if crate::server::for_tweaks(crate::server::current).is_none() => begin_tweaks(&hub),
                            ServerDo::Again => crate::server::for_tweaks(|| crate::server::again(&hub)),
                            ServerDo::Back => crate::server::for_tweaks(crate::server::back),
                            ServerDo::Go => crate::server::for_tweaks(|| crate::server::go(&hub)),
                            ServerDo::Ask(action) => crate::server::for_tweaks(|| crate::server::ask(action)),
                            // (Left where it is: the tab says the settings aren't read.)
                            ServerDo::Close => {
                                crate::server::for_tweaks(|| crate::server::close(&hub));
                                tweaks_changed();
                            }
                            ServerDo::OpenPasswords => {
                                let _ = olo_xdg::spawn_detached("olo-passwords");
                            }
                            ServerDo::LetIn | ServerDo::ShowLog | ServerDo::LogOf(_) | ServerDo::Refresh | ServerDo::Copy => {}
                        }
                    }
                    HostDo::TweakKey(i) => crate::server::for_tweaks(|| crate::server::choose_key(i)),
                    HostDo::TweakEveryone(on) => crate::server::for_tweaks(|| crate::server::everyone(on)),
                    HostDo::ScreensDefault(on_host) => crate::host::screens_default(&hub, on_host),
                    HostDo::ScreensCancel => crate::host::cancel_screens(&hub),
                    HostDo::ScreenStop { host_key, host_instance_id, name, expected_started } => crate::host::stop_screen(&hub, &host_key, host_instance_id, &name, expected_started),
                    HostDo::ScreenLook { host_key, host_instance_id, host_label, name, expected_started } => {
                        open_host_screen_live_in_rex(&hub, &host_key, host_instance_id, &host_label, &name, expected_started);
                    }
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
                let hub = the_hub().clone();
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
                    crate::server::page(the_hub(), *page);
                }
            }
            WinMsg::ServerPerson(i) => crate::server::choose_person(i),
            WinMsg::ServerRange(i) => {
                if let Some(range) = olo_rex_client::manage::LogRange::ALL.get(i) {
                    crate::server::read_log(the_hub(), None, Some(*range));
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
                let is_secret = matches!(the_askpass().current_request(), Some(AskpassRequest::Secret { .. }));
                if is_secret {
                    the_askpass().answer(AskpassReply::Secret(None));
                } else {
                    the_askpass().answer(AskpassReply::Confirm(ok));
                }
                cx.request_close();
            }
            WinMsg::AskpassSubmit => self.submit_askpass(cx),
            WinMsg::RunApproveChoice(allow) => {
                let approvals = the_run_approvals().clone();
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
                if the_run_approvals().current().is_none() {
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
                let hub_ref = the_hub().clone();
                let (live, top_indices, running_kids) = {
                    let st = hub_ref.state.lock().unwrap_or_else(|e| e.into_inner());
                    let (live, top_indices) = top_level_window_indices(&st.subagents, &hk, &sess);
                    let running_kids: Vec<RunningChild> = top_indices
                        .get(i)
                        .map(|&widx| {
                            st.subagents
                                .descendants_tree(&hk, &sess.session_id, &sess.windows[widx].window_id, &live)
                                .into_iter()
                                .filter(|r| {
                                    let bk = BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: r.entry.block_id.clone() };
                                    !r.entry.cli.is_agent()
                                        || st.blocks.get(&bk).is_some_and(|br| {
                                            matches!(br.lock().unwrap_or_else(|e| e.into_inner()).agent_activity, AgentActivity::Working | AgentActivity::Blocked | AgentActivity::Error)
                                        })
                                })
                                .map(|r| RunningChild { name: r.entry.display_title(), command: !r.entry.cli.is_agent(), minder: r.entry.is_shared_minder(), window_id: r.entry.window_id })
                                .collect()
                        })
                        .unwrap_or_default();
                    (live, top_indices, running_kids)
                };
                let _ = live;
                let Some(&w_idx) = top_indices.get(i) else { return };
                let tab = sess.windows[w_idx].window_id.clone();
                let count = top_indices.len();
                let top_ids: Vec<String> = top_indices.iter().map(|&idx| sess.windows[idx].window_id.clone()).collect();
                let panes = sess.windows.get(w_idx).map(|w| tab_agent_panes(the_hub(), &hk, &sess.session_id, w)).unwrap_or_default();
                let tab_block = sess.windows.get(w_idx).and_then(|w| {
                    w.focused_block_id.as_deref().filter(|id| w.all_blocks().any(|b| b.block_id == *id)).or_else(|| w.all_blocks().next().map(|b| b.block_id.as_str())).map(|bid| BlockKey {
                        host_key: hk.clone(),
                        session_id: sess.session_id.clone(),
                        block_id: bid.to_owned(),
                    })
                });
                let several = sess.windows.get(w_idx).is_some_and(|w| w.all_blocks().count() > 1);
                let active_view = crate::activity_watch::active_view_target();
                let viewing_transcript = tab_block.as_ref().is_some_and(|tb| matches!(&active_view, Some(crate::activity_watch::ViewTarget::PaneTranscript { bk, .. }) if bk == tb));
                let mut menu = Menu::new("").item("Rename Tab…", "window.rename").item(olo_kit_base::tracker::LABEL, "files.open_in_tracker").enabled(tab_block.is_some()).separator();
                if tab_block.is_some() {
                    menu = menu.item("Show Transcript", "pane.show_transcript").checked(viewing_transcript).item("Show Terminal", "pane.show_terminal").checked(!viewing_transcript).separator();
                }
                for (n, p) in panes.iter().enumerate() {
                    let help = the_hub().help(p);
                    let (keep, approve) = if several {
                        let name = pane_name(the_hub(), p);
                        (olo_kit::tr!("Keep It Going in \u{201c}{}\u{201d}", name), olo_kit::tr!("Auto Approve in \u{201c}{}\u{201d}", name))
                    } else {
                        ("Keep It Going".to_owned(), "Auto Approve".to_owned())
                    };
                    menu = menu.item(keep, format!("agent.keep_going_toggle#{n}")).checked(help.keep_going).item(approve, format!("agent.auto_approve_toggle#{n}")).checked(help.auto_approve);
                }
                if !panes.is_empty() {
                    menu = menu.separator();
                }
                if !running_kids.is_empty() {
                    menu = menu.item(stop_children_label(&running_kids), "sub.stop_all").separator();
                }
                let mut menu =
                    menu.item("Move Tab Left", "window.move_left").enabled(i > 0).item("Move Tab Right", "window.move_right").enabled(i + 1 < count).separator().item("Close Tab", "window.close");
                menu::resolve(std::slice::from_mut(&mut menu), true);
                let hub = the_hub().clone();
                let window = self.id;
                let tab_host_pin = capture_live_host_pin(&hub, &hk);
                menu::context_menu(cx, menu, at, move |cmd, _| match cmd {
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
                    "sub.stop_all" if !running_kids.is_empty() => {
                        if let Some(slot) = wins().map.get(&window) {
                            slot.handle.messenger().send(WinMsg::ConfirmStopSubagents {
                                host_key: hk.clone(),
                                host_pin: tab_host_pin,
                                session_id: sess.session_id.clone(),
                                children: running_kids.clone(),
                            });
                        }
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
                        if let Some(before) = to.and_then(|to| crate::hub::moved_before(&top_ids, i, to)) {
                            hub.move_window(&hk, &sess.session_id, &tab, before);
                        }
                    }
                    "window.close" => hub.close_window(&hk, &sess.session_id, &tab),
                    _ => {}
                });
            }
            WinMsg::TabTreeMenu(i, bounds) => {
                let Some((hk, sess)) = self.resolve_active_session() else { return };
                let hub_ref = the_hub().clone();
                let (parent_wid, parent_label, parent_detail, rows, active_wid, is_side) = {
                    let st = hub_ref.state.lock().unwrap_or_else(|e| e.into_inner());
                    let (live, top_indices) = top_level_window_indices(&st.subagents, &hk, &sess);
                    let Some(&w_idx) = top_indices.get(i) else { return };
                    let names = crate::hub::session_tab_names(&st, &hk, &sess);
                    let parent_win = &sess.windows[w_idx];
                    let parent_wid = parent_win.window_id.clone();
                    let parent_label = names.get(w_idx).cloned().unwrap_or_else(|| "Tab".into());
                    // Identify the root tab's own agent harness and live state so its row matches its children.
                    let mut parent_detail: Option<(String, AgentActivity)> = None;
                    for b in parent_win.all_blocks() {
                        let bk = BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                        if let Some(br) = st.blocks.get(&bk) {
                            let g = br.lock().unwrap_or_else(|e| e.into_inner());
                            let program = g.program_title();
                            let raw_harness = detect_agent_harness(program.as_deref(), &g.title);
                            let chrome = raw_harness.map(|h| if h == "agy" { "antigravity".to_owned() } else { h });
                            if let Some(h) = chrome {
                                let act = g.agent_activity;
                                if parent_detail.as_ref().is_none_or(|(_, cur)| higher_activity(*cur, act) != *cur) {
                                    parent_detail = Some((h, act));
                                }
                            }
                        }
                    }
                    let descendants = st.subagents.descendants_tree(&hk, &sess.session_id, &parent_wid, &live);
                    let mut rows = Vec::with_capacity(descendants.len());
                    for row in descendants {
                        let bk = BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: row.entry.block_id.clone() };
                        let act = st.blocks.get(&bk).map(|br| br.lock().unwrap_or_else(|e| e.into_inner()).agent_activity).unwrap_or(AgentActivity::Idle);
                        rows.push((row, act));
                    }
                    let active_wid = sess.active_window_id.clone().or_else(|| sess.windows.first().map(|w| w.window_id.clone()));
                    let is_side = st.settings.toolbar_layout == ToolbarLayout::Side;
                    (parent_wid, parent_label, parent_detail, rows, active_wid, is_side)
                };
                if rows.is_empty() {
                    return;
                }
                let apply_detail = |m: Menu, harness: &str, act: AgentActivity| match act {
                    AgentActivity::Blocked => m.detail_status(harness.to_owned(), "Waiting", menu::DetailTone::Warning),
                    AgentActivity::Error => m.detail_status(harness.to_owned(), "Failed", menu::DetailTone::Error),
                    AgentActivity::Working => m.detail_status(harness.to_owned(), "Working", menu::DetailTone::Secondary),
                    AgentActivity::Done => m.detail_status(harness.to_owned(), "Done", menu::DetailTone::Muted),
                    AgentActivity::Idle => m.detail(harness.to_owned()),
                };
                let mut menu = Menu::new("").item(parent_label, format!("win:{parent_wid}")).checked(active_wid.as_deref() == Some(parent_wid.as_str()));
                if let Some((ref h, act)) = parent_detail {
                    menu = apply_detail(menu, h, act);
                }
                let mut running_kids: Vec<RunningChild> = Vec::new();
                for (row, act) in &rows {
                    let harness = row.entry.cli.chrome_name();
                    let command = !row.entry.cli.is_agent();
                    menu = menu
                        .item(row.entry.display_title(), format!("win:{}", row.entry.window_id))
                        .tree(row.depth, row.is_last, row.guides.clone())
                        .checked(active_wid.as_deref() == Some(row.entry.window_id.as_str()));
                    if command || matches!(act, AgentActivity::Blocked | AgentActivity::Error | AgentActivity::Working) {
                        running_kids.push(RunningChild { window_id: row.entry.window_id.clone(), name: row.entry.display_title(), command, minder: row.entry.is_shared_minder() });
                    }
                    // (A command of the asker's own says nothing of itself as
                    // an agent does: running, for as long as it is a child.)
                    menu = if command { menu.detail_status(harness.to_owned(), "Running", menu::DetailTone::Secondary) } else { apply_detail(menu, harness, *act) };
                }
                let stop_label = stop_children_label(&running_kids);
                menu = menu.separator().item(stop_label, "sub.stop_all").enabled(!running_kids.is_empty());
                menu::resolve(std::slice::from_mut(&mut menu), true);
                self.open_tree_tab = Some(i);
                cx.invalidate();
                let window = self.id;
                let sid = sess.session_id.clone();
                let tree_host_pin = capture_live_host_pin(&hub_ref, &hk);
                let corner = if is_side { olo_kit::Corner::TopRight } else { olo_kit::Corner::BottomLeft };
                menu::popup_then(
                    cx,
                    menu,
                    bounds,
                    corner,
                    move |cmd, _| {
                        if let Some(wid) = cmd.strip_prefix("win:") {
                            if let Some(slot) = wins().map.get(&window) {
                                slot.handle.messenger().send(WinMsg::SelectWindow(hk.clone(), sid.clone(), wid.to_owned()));
                            }
                        } else if cmd == "sub.stop_all" && !running_kids.is_empty() {
                            if let Some(slot) = wins().map.get(&window) {
                                slot.handle.messenger().send(WinMsg::ConfirmStopSubagents { host_key: hk.clone(), host_pin: tree_host_pin, session_id: sid.clone(), children: running_kids.clone() });
                            }
                        }
                    },
                    move |_| {
                        if let Some(slot) = wins().map.get(&window) {
                            slot.handle.messenger().send(WinMsg::TreeMenuClosed);
                        }
                    },
                );
            }
            WinMsg::RenameTab(hk, sid, tab) => {
                let session = {
                    let hub = the_hub().clone();
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
                let hub = the_hub().clone();
                let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.sidebar_width = w;
                st.settings.sidebar_collapsed = col;
                st.settings.save();
                cx.invalidate();
            }
            WinMsg::CloseWindow => {
                self.close_screen(cx);
                cx.request_close();
            }
        }
    }

    fn command(&mut self, command: &str, cx: &mut olo_kit::Cx<Self>) -> bool {
        if olo_kit::app_menu::run(command, cx) {
            return true;
        }
        match command {
            "close" => {
                self.close_screen(cx);
                cx.request_close();
            }
            "quit" => {
                self.close_screen(cx);
                shutdown_app();
                olo_kit::quit();
            }
            _ => self.execute_action(command, None, cx),
        }
        true
    }

    fn window_activated(&mut self, active: bool, cx: &mut olo_kit::Cx<Self>) {
        self.window_active = active;
        if !active {
            app().attention.hide(self.id);
            if let Some(hub) = HUB.get() {
                hub.hide_window_front_pane(self.id);
            }
            // Release Take Control and any pending take-control attempt immediately
            // when the window loses focus so a background or hidden Rex window never
            // holds or later grabs control of a screen.
            if let Some(st) = &mut self.active_screen {
                st.controlling = false;
                st.pending_take_control = false;
                if let Some(pane) = &self.screen_pane {
                    pane.update(cx, |p, cx| p.stop_controlling(cx));
                }
            }
        } else if let Some(st) = &mut self.active_screen
            && st.pending_take_control
        {
            // Neon #7: flush one-shot Take Control armed while `!self.window_active`
            // (e.g. `OLO_REX_DEMO_ACTIVITY=screens-control` or context menu on an
            // unfocused workspace) once the window activates.
            st.pending_take_control = false;
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| p.request_control_when_ready(cx));
                st.controlling = pane.read(cx).controlling();
            }
        }
        cx.invalidate();
    }

    fn window_closing(&mut self, cx: &mut olo_kit::Cx<Self>) {
        self.close_screen(cx);
    }

    fn takes_drops(&self) -> bool {
        self.role == WinRole::Workspace
    }

    fn drop_over(&mut self, offer: Option<&DropOffer>, _: Point<Pixels>, cx: &mut olo_kit::Cx<Self>) -> Option<DropAccept> {
        // Never accept terminal file/text drops while an embedded screen is open (Astra #6).
        if self.active_screen.is_some() {
            if self.dropping {
                self.dropping = false;
                cx.invalidate();
            }
            return None;
        }
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
        if self.active_screen.is_some() {
            cx.invalidate();
            return;
        }
        if let Some(bk) = self.focused_block_key() {
            if d.mime == "text/uri-list" {
                the_hub().upload_files(&bk, d.paths(), Feedback::start(self.id));
            } else {
                let text = String::from_utf8_lossy(&d.data).into_owned();
                if !text.is_empty() && paste_is_safe(&text) {
                    the_hub().send_input(&bk, text.into_bytes());
                }
            }
        }
        cx.invalidate();
    }

    fn save_state(&mut self, cx: &mut olo_kit::Cx<Self>) -> Option<String> {
        if self.role != WinRole::Workspace {
            return None;
        }
        // Release Take Control before in-place upgrade re-exec so local `.control`
        // files and remote control leases are never orphaned across `exec` (Astra #5).
        if let Some(st) = &mut self.active_screen {
            st.controlling = false;
            st.pending_take_control = false;
            if let Some(pane) = &self.screen_pane {
                pane.update(cx, |p, cx| p.stop_controlling(cx));
            }
        }
        let (active_screen, active_screen_tab) = match (&self.active_screen, &self.screen_pane) {
            (Some(st), Some(pane)) => (Some(st.id.clone()), Some(pane.read(cx).tab())),
            (Some(st), None) => (Some(st.id.clone()), Some(st.tab)),
            (None, _) => match &self.pending_restored_screen {
                Some((id, tab)) => (Some(id.clone()), Some(*tab)),
                None => (None, None),
            },
        };
        let saved = SavedWorkspaceState {
            active_host: self.active_host.clone(),
            active_session: self.active_session.clone(),
            session_index: self.session_index,
            sidebar_collapsed: self.side_pane.collapsed(),
            sidebar_width: self.side_pane.width(),
            active_screen,
            active_screen_tab,
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
            crate::moving::cancel(the_hub());
        }
        if self.role == WinRole::Server {
            crate::server::close(the_hub());
        }
        if self.role == WinRole::Host {
            crate::host::close();
            crate::server::for_tweaks(|| crate::server::close(the_hub()));
        }
        if self.role == WinRole::Account {
            crate::account::close();
        }
        if self.role == WinRole::Carry {
            crate::carry::close();
        }
        if self.role == WinRole::RunApprove {
            // Closing the alert window denies the pending command.
            the_run_approvals().clone().deny_front();
        }
        // Belt-and-suspenders: if the workspace window drops without `window_closing`,
        // release Take Control on any still-bound screen source immediately.
        if let Some(st) = self.active_screen.take()
            && st.controlling
            && let Some(src) = st.source
        {
            let _ = src.as_dyn().set_controlled(false);
        }
        self.cancel_preview();
        app().attention.hide(self.id);
        if let Some(hub) = HUB.get() {
            hub.hide_window_front_pane(self.id);
        }
        wins().map.remove(&self.id);
        if self.role == WinRole::Workspace && !wins().map.values().any(|s| s.role == WinRole::Workspace) {
            crate::activity_watch::stop_all();
        }
        if wins().map.is_empty() {
            // Stop the screens service BEFORE `olo_interface::run` can linger
            // windowless for the clipboard so a newly launched Rex instance can
            // take over cleanly (Astra finding 6).
            shutdown_app();
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
            let hub = the_hub().clone();
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
    /// How large its own area is, as it was last laid out: what a
    /// floating layer's place is a share of, and what a drag of one is
    /// measured against.
    area: Rc<std::cell::Cell<olo_kit::Size<Pixels>>>,
}

impl View for PanesView {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut olo_kit::Cx<Self>) {
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut olo_kit::Cx<Self>) -> Element {
        let hub = the_hub().clone();
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

        // (Its own size known as it is laid out, and said again when it
        // changes; what floats in it is cut at its edges.)
        let (area_now, area_told) = (self.area.clone(), cx.messenger());
        let mut layer_stack = div().relative().size_full().overflow_hidden().on_bounds_changed(move |b: &Bounds<Pixels>, _| {
            if area_now.get() != b.size {
                area_now.set(b.size);
                area_told.send(());
            }
        });

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
        let mut minimized_bar = div().absolute().right(px(m::SPACING)).bottom(px(m::SPACING)).occlude().flex().flex_row().gap(px(m::TIGHT));
        let mut has_minimized = false;

        for fl in floating_layers {
            if fl.layer_state() == LayerState::Minimized {
                has_minimized = true;
                let lid = fl.layer_id.clone();
                let hk = host_key.clone();
                let sid = sess.session_id.clone();
                // (Its head put away: the same icon and name, so it is the same pane brought back.)
                let chip = fl.blocks.first().map(|b| {
                    let bk = BlockKey { host_key: host_key.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                    let (title, activity) = float_said(&bk, &b.label, is_local);
                    (title, activity, crate::hub::help_of(&bk))
                });
                let (title, activity, help) = chip.unwrap_or_else(|| ("Floating Terminal".to_owned(), AgentActivity::Idle, crate::agent::Help::default()));
                minimized_bar = minimized_bar.child(float_chip(format!("rex-float-restore-{lid}"), title, activity, help, move || {
                    the_hub().set_layer_minimized(&hk, &sid, &lid, false);
                }));
            } else if let Some(b) = fl.blocks.first() {
                // Its place is a share of the panes' own area, and kept
                // inside it. (It was a share of the whole window, set
                // down from the panes' corner: with the sidebar open it
                // ran past the window's right edge, and its frame, its
                // buttons and its corner with it.)
                let [bx, by, bw, bh] = fl.bounds;
                let area = self.area.get();
                let (area_w, area_h) = if f32::from(area.width) > 1.0 && f32::from(area.height) > 1.0 {
                    (f32::from(area.width), f32::from(area.height))
                } else {
                    // (Before the first layout says: the width it was given, and the window's height less its bars.)
                    (vp_w, (f32::from(cx.viewport_size().height) - 90.0).max(180.0))
                };
                let width = (area_w * bw as f32).clamp(260.0_f32.min(area_w), area_w);
                let height = (area_h * bh as f32).clamp(180.0_f32.min(area_h), area_h);
                let left = (area_w * bx as f32).clamp(0.0, (area_w - width).max(0.0));
                let top = (area_h * by as f32).clamp(0.0, (area_h - height).max(0.0));
                let bk = BlockKey { host_key: host_key.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
                let focused = win_view.focused_block_id.as_deref() == Some(b.block_id.as_str());
                let (hk, sid, lid) = (host_key.clone(), sess.session_id.clone(), fl.layer_id.clone());
                // (Named as every pane is, with the icon of what runs in it.)
                let (fl_title, fl_activity) = float_said(&bk, &b.label, is_local);
                let fl_help = crate::hub::help_of(&bk);
                let init_bounds = fl.bounds;
                // Brought to the front and typed into: by a press anywhere on it.
                let front: Rc<dyn Fn()> = {
                    let (hk, sid, lid, bk) = (hk.clone(), sid.clone(), lid.clone(), bk.clone());
                    Rc::new(move || {
                        the_hub().raise_layer(&hk, &sid, &lid);
                        the_hub().focus_block(&bk.host_key, &bk.session_id, &bk.block_id);
                    })
                };
                // (How far the pointer went, as a share of the panes' area.)
                let moved = {
                    let area = self.area.clone();
                    move |from: Point<Pixels>, to: Point<Pixels>| {
                        let area = area.get();
                        let (w, h) = (f32::from(area.width).max(200.0), f32::from(area.height).max(160.0));
                        (f64::from((f32::from(to.x) - f32::from(from.x)) / w), f64::from((f32::from(to.y) - f32::from(from.y)) / h))
                    }
                };
                let (press_body, press_bar, press_corner) = (front.clone(), front.clone(), front.clone());
                let (moved_bar, moved_corner) = (moved.clone(), moved);
                let (hk_move, sid_move, lid_move) = (hk.clone(), sid.clone(), lid.clone());
                let (hk_size, sid_size, lid_size) = (hk.clone(), sid.clone(), lid.clone());
                let (hk_min, sid_min, lid_min) = (hk.clone(), sid.clone(), lid.clone());
                let (hk_close, sid_close, lid_close) = (hk.clone(), sid.clone(), lid.clone());

                layer_stack = layer_stack.child(
                    div()
                        .id(olo_kit::SharedString::from(format!("rex-float-{lid}")))
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(width))
                        .h(px(height))
                        // (What is under it hears nothing of what happens
                        // on it. A press went on to the tiled pane under
                        // the pointer, which took the focus straight
                        // back: the floating pane could not be typed
                        // into, and Close Pane closed the pane under it.)
                        .occlude()
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, move |_, cx| {
                            press_body();
                            cx.stop_propagation();
                        })
                        .child(float_under())
                        .child(
                            // Its bar, a pane's head: moved by it, and its
                            // two buttons at the right. (What a drag began with is kept
                            // with the bar, by its name, across the
                            // redraws each step of the drag causes: kept
                            // in a cell made anew at each of them, a drag
                            // went one step and stopped.)
                            div()
                                .id(olo_kit::SharedString::from(format!("rex-float-bar-{lid}")))
                                .relative()
                                .h(px(crate::settings::PANE_HEADER))
                                .flex_shrink_0()
                                .pl(px(pad.0))
                                .pr(px(HEADER_END))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(m::TIGHT))
                                .cursor(CursorStyle::OpenHand)
                                .on_mouse_down(MouseButton::Left, move |e, cx| {
                                    cx.node_state::<FloatDrag>().0 = Some((e.position, init_bounds));
                                    press_bar();
                                    cx.stop_propagation();
                                })
                                .on_drag_move(move |e: &MouseMoveEvent, cx| {
                                    let drag = cx.node_state::<FloatDrag>();
                                    let Some((from, was)) = drag.0 else { return };
                                    if e.pressed_button != Some(MouseButton::Left) {
                                        drag.0 = None;
                                        return;
                                    }
                                    let (dx, dy) = moved_bar(from, e.position);
                                    let x = (was[0] + dx).clamp(0.0, (1.0 - was[2]).max(0.0));
                                    let y = (was[1] + dy).clamp(0.0, (1.0 - was[3]).max(0.0));
                                    the_hub().resize_layer(&hk_move, &sid_move, &lid_move, [x, y, was[2], was[3]]);
                                })
                                .on_window_mouse_up(|e, cx| {
                                    if e.button == MouseButton::Left {
                                        cx.node_state::<FloatDrag>().0 = None;
                                    }
                                })
                                .child(head_ground(focused, style::key(("float-head", focused))))
                                .child(head_icon(fl_activity, fl_help))
                                .child(div().flex_1().min_w_0().child(head_title(fl_title, focused)))
                                .child(
                                    // (A press on a button is the button's, not the beginning of a move.)
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(m::TIGHT))
                                        .on_mouse_down(MouseButton::Left, |_, cx| cx.stop_propagation())
                                        .child(GlyphButton::new("Minimize Floating Terminal", Glyph::Minimize).id(format!("rex-float-minimize-{lid}")).quiet(!focused).on_click(move |_, _| {
                                            the_hub().set_layer_minimized(&hk_min, &sid_min, &lid_min, true);
                                        }))
                                        .child(GlyphButton::new("Close Floating Terminal", Glyph::Close).id(format!("rex-float-close-{lid}")).quiet(!focused).on_click(move |_, _| {
                                            the_hub().close_layer(&hk_close, &sid_close, &lid_close);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(render_single_pane(bk, &host_label, is_local, focused, false, false, width, &settings, &active_theme, &metrics, pad, win_id, cx))
                                .child(
                                    // Its bottom right corner: sized by it.
                                    canvas(move |g, b| float_grip(g, b, focused))
                                        .id(olo_kit::SharedString::from(format!("rex-float-corner-{lid}")))
                                        .paint_key(style::key(("float-grip", focused)))
                                        .absolute()
                                        .right(px(0.0))
                                        .bottom(px(0.0))
                                        .w(px(FLOAT_GRIP))
                                        .h(px(FLOAT_GRIP))
                                        .cursor(CursorStyle::ResizeUpLeftDownRight)
                                        .on_mouse_down(MouseButton::Left, move |e, cx| {
                                            cx.node_state::<FloatDrag>().0 = Some((e.position, init_bounds));
                                            press_corner();
                                            cx.stop_propagation();
                                        })
                                        .on_drag_move(move |e: &MouseMoveEvent, cx| {
                                            let drag = cx.node_state::<FloatDrag>();
                                            let Some((from, was)) = drag.0 else { return };
                                            if e.pressed_button != Some(MouseButton::Left) {
                                                drag.0 = None;
                                                return;
                                            }
                                            let (dx, dy) = moved_corner(from, e.position);
                                            let w = (was[2] + dx).clamp(0.24, (1.0 - was[0]).max(0.24));
                                            let h = (was[3] + dy).clamp(0.22, (1.0 - was[1]).max(0.22));
                                            the_hub().resize_layer(&hk_size, &sid_size, &lid_size, [was[0], was[1], w, h]);
                                        })
                                        .on_window_mouse_up(|e, cx| {
                                            if e.button == MouseButton::Left {
                                                cx.node_state::<FloatDrag>().0 = None;
                                            }
                                        }),
                                ),
                        )
                        .child(float_line()),
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
                the_hub().set_split_ratio(&hk_d, &sid_d, &split_id, 0.5);
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
                the_hub().set_split_ratio(&hk_dr, &sid_dr, &split_id_drag, f64::from(next));
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
    let hub = the_hub().clone();
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
                        the_hub().split_pane(&hk_ob, &sid_ob, &bid_ob, "right", None);
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

        let head_bg = head_ground(focused, key);
        let icon_el = head_icon(activity, pane_help.clone());
        let title_el = head_title(title_paint.clone(), focused);

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
                let hub = the_hub().clone();
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
                    the_hub().toggle_zoom(&z.host_key, &z.session_id, Some(z.block_id.clone()));
                }));
            }
        }
        if show_split_btns {
            bar = bar
                .child(GlyphButton::new("Split Right", Glyph::SplitRight).id(format!("rex-split-right-{}", bk.block_id)).quiet(!focused).on_click(move |_, _| {
                    the_hub().split_pane(&bk_split_r.host_key, &bk_split_r.session_id, &bk_split_r.block_id, "right", None);
                }))
                .child(GlyphButton::new("Split Down", Glyph::SplitDown).id(format!("rex-split-down-{}", bk.block_id)).quiet(!focused).on_click(move |_, _| {
                    the_hub().split_pane(&bk_split_d.host_key, &bk_split_d.session_id, &bk_split_d.block_id, "down", None);
                }));
        }
        if show_close_btn {
            bar = bar.child(GlyphButton::new("Close Pane", Glyph::Close).id(format!("rex-close-pane-{}", bk.block_id)).quiet(!focused).on_click(move |_, _| {
                the_hub().close_block(&bk_close.host_key, &bk_close.session_id, &bk_close.block_id);
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
        if resize && hub_paint.resize_block(&bk_paint, want_cols, want_rows, cw_px, ch_px) {
            if let Some(slot) = wins().map.get(&win_id) {
                slot.handle.messenger().send(WinMsg::Redraw);
            }
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
        let hub = the_hub().clone();
        // (Before it is brought forward: a click in the pane already in
        // front is the person's doing something there, `send_pointer_press`.)
        let in_front = hub.has_focus(&bk_mouse);
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
                    LinkTarget::Url(u) => the_hub().open_link_from_pane(&bk_mouse, u, Feedback::start(win_id)),
                    LinkTarget::FilePath { path, .. } => {
                        if let Some(slot) = wins().map.get(&win_id) {
                            slot.handle.messenger().send(WinMsg::RevealInTracker(bk_mouse.clone(), path));
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
                hub.send_pointer_press(&bk_mouse, bytes, in_front);
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
                    the_hub().send_pointer(&bk_drag, bytes);
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
                let in_front = the_hub().has_focus(&bk_rclick);
                the_hub().send_pointer_press(&bk_rclick, bytes, in_front);
                cx.stop_propagation();
                return;
            }
        }
        // The menu bar's commands for this pane, by the names and with
        // the checks and keys they have there: the clipboard; its text;
        // the pane itself; closing it, alone and last.
        let help = the_hub().help(&bk_rclick);
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
        let hub = the_hub().clone();
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
            // (A step of the wheel told to a program that hears the mouse scrolls
            // what it shows; the wheel's arrows are arrow keys to it, move a
            // chooser's mark, and are typing like any key.)
            if goes == crate::hub::Wheel::Arrows {
                the_hub().send_input(&bk_scroll, bytes);
            } else {
                the_hub().send_pointer(&bk_scroll, bytes);
            }
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
                let in_front = the_hub().has_focus(&bk_middle);
                the_hub().send_pointer_press(&bk_middle, bytes, in_front);
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
                    // (Never more than its press was: the release of a click that
                    // brought the pane forward finds it in front.)
                    the_hub().send_pointer_press(&bk_up, bytes, false);
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
        let hub_stop = the_hub().clone();
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

/// `text` in at most two lines of `size` points, the first no wider than
/// `room` (broken where a word ends: a word longer than that alone on it),
/// the second what is left (cut short when it's drawn).
#[cfg(test)]
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

/// Fit `text` onto a single line within `room` points, filling the full width
/// while stepping back to the previous word boundary whenever character cutoff
/// would leave a 1–3 letter trailing stub (e.g. `Running visual QA o…` -> `Running visual QA…`).
fn fit_clean_line(text: &str, room: f32, size: f32) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || room <= 24.0 {
        return trimmed.to_owned();
    }
    let font = style::ui_font(FontWeight::NORMAL);
    let fits = |s: &str| f32::from(style::text_width(s.to_owned(), size, font.clone())) <= room;
    if fits(trimmed) {
        return trimmed.to_owned();
    }
    let ell = "…";
    let mut best_byte = 0usize;
    for (idx, ch) in trimmed.char_indices() {
        let end = idx + ch.len_utf8();
        let cand = format!("{}{ell}", &trimmed[..end]);
        if fits(&cand) {
            best_byte = end;
        } else {
            break;
        }
    }
    if best_byte == 0 {
        return trimmed.to_owned();
    }
    let slice = trimmed[..best_byte].trim_end();
    // If we cut mid-word and the visible prefix of that trailing word is under 4 letters,
    // step back cleanly to the preceding word boundary.
    let mid_word = trimmed[best_byte..].chars().next().is_some_and(|c| !c.is_whitespace());
    if mid_word && let Some(sp) = slice.rfind(char::is_whitespace) {
        let tail_word = &slice[sp + 1..];
        if tail_word.chars().count() < 4 {
            let clean = slice[..sp].trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | ',' | ':' | ';' | '-' | '—' | '·'));
            if !clean.is_empty() {
                return format!("{clean}{ell}");
            }
        }
    }
    let clean = slice.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | ',' | ':' | ';' | '-' | '—'));
    format!("{clean}{ell}")
}

/// Fit `text` onto a single line within `room` points, breaking cleanly at word
/// boundaries before appending `…` so sidebar subtitles never end with awkward
/// 1–2 letter word stubs like `t…` or `an…`.
fn fit_words_line(text: &str, room: f32, size: f32) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || room <= 24.0 {
        return trimmed.to_owned();
    }
    let font = style::ui_font(FontWeight::NORMAL);
    let fits = |s: &str| f32::from(style::text_width(s.to_owned(), size, font.clone())) <= room;
    if fits(trimmed) {
        return trimmed.to_owned();
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() <= 1 {
        return trimmed.to_owned();
    }
    let mut best = 0usize;
    for k in 1..=words.len() {
        let cand = format!("{}…", words[..k].join(" ").trim_end_matches(['.', ',', ':', ';', '-', '—']));
        if fits(&cand) {
            best = k;
        } else {
            break;
        }
    }
    if best > 0 { format!("{}…", words[..best].join(" ").trim_end_matches(['.', ',', ':', ';', '-', '—'])) } else { trimmed.to_owned() }
}

/// Like [`fit_words_line`], but when a keyword search term matches past the
/// normal truncation point (e.g. `"shots"` in `"Visual QA of 2× light and dark shots"`),
/// shifts the visible fragment to start near the match (`"…light and dark shots"`).
fn fit_query_snippet(text: &str, terms: &[String], room: f32, size: f32) -> String {
    let normal = fit_words_line(text, room, size);
    if terms.is_empty() {
        return normal;
    }
    let normal_lower = normal.to_ascii_lowercase();
    if terms.iter().any(|t| !t.is_empty() && normal_lower.contains(t.as_str())) {
        return normal;
    }
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    let Some(hit_pos) = terms.iter().filter(|t| !t.is_empty()).filter_map(|t| lower.find(t.as_str())).min() else {
        return normal;
    };
    let font = style::ui_font(FontWeight::NORMAL);
    let fits = |s: &str| f32::from(style::text_width(s.to_owned(), size, font.clone())) <= room;
    // Walk backward word-by-word from the match so the fragment shows context before the hit.
    let words: Vec<(usize, &str)> = trimmed.split_whitespace().map(|w| (w.as_ptr() as usize - trimmed.as_ptr() as usize, w)).collect();
    let hit_word = words.iter().rposition(|(off, _)| *off <= hit_pos).unwrap_or(0);
    let mut start = hit_word;
    while start > 0 {
        let cand = format!("…{}", words[start - 1..].iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" "));
        if fits(&cand) {
            start -= 1;
        } else {
            break;
        }
    }
    let tail = words[start..].iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" ");
    let prefixed = if start > 0 { format!("…{tail}") } else { tail };
    if fits(&prefixed) { prefixed } else { fit_words_line(&prefixed, room, size) }
}

/// Capture the live `(host_instance_id, screens_conn_id)` pin for `host_key`
/// when rendering a row or building a context menu (Astra #1). Returns `None`
/// for missing hosts or disconnected remote hosts (`screens_conn_id == 0`) so
/// a stale menu never acquires a replacement host's identity or `(0, 0)` wildcard.
fn capture_live_host_pin(hub: &Arc<crate::hub::RexHub>, host_key: &str) -> Option<(u64, u64)> {
    hub.host_live_pin(host_key)
}

/// Strip dangling trailing connectors (`&`, `·`, `-`, `–`, `,`, `/`, `:`) and
/// whitespace before appending `…` when a label must be shortened to `max_w`
/// (Opus #19: `"Billing Checkout Flow &…"` -> `"Billing Checkout Flow…"`).
fn fit_clean_ellipsis(text: &str, size: f32, font: &olo_kit::Font, max_w: Pixels) -> String {
    if style::text_width(text.to_owned(), size, font.clone()) <= max_w {
        return text.to_owned();
    }
    let ell_w = style::text_width("…".to_owned(), size, font.clone());
    let room = (max_w - ell_w).max(px(0.0));
    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if style::text_width(text[..next].to_owned(), size, font.clone()) <= room {
            end = next;
        } else {
            break;
        }
    }
    let cleaned = trim_dangling_ellipsis_prefix(&text[..end]);
    if cleaned.is_empty() { "…".to_owned() } else { format!("{cleaned}…") }
}

fn trim_dangling_ellipsis_prefix(prefix: &str) -> &str {
    let mut s = prefix.trim_end();
    loop {
        let next = s.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '&' | '·' | '-' | '–' | ',' | '/' | ':')).trim_end();
        if next.len() == s.len() {
            return s;
        }
        s = next;
    }
}

/// Paint a single-line label, highlighting any case-insensitive matches of `terms`
/// with the same warm yellow Find-highlight role used in Transcript view.
#[allow(clippy::too_many_arguments)]
fn paint_query_label(g: &mut olo_kit::Painter, x: Pixels, y_center: Pixels, text: &str, size: f32, weight: FontWeight, ink: olo_kit::Hsla, max_w: Pixels, terms: &[String], p: &olo_theme::Palette) {
    let font = style::ui_font(weight);
    if terms.is_empty() || text.is_empty() {
        let fitted = if text.is_empty() { String::new() } else { fit_clean_ellipsis(text, size, &font, max_w) };
        style::label_fit(g, x, y_center, style::Align::Left, fitted, size, font, ink, max_w);
        return;
    }
    let lower = text.to_ascii_lowercase();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for t in terms {
        if t.is_empty() {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(t.as_str()) {
            let s = from + rel;
            let e = s + t.len();
            if text.is_char_boundary(s) && text.is_char_boundary(e) {
                spans.push((s, e));
            }
            from = e.max(from + 1);
        }
    }
    if spans.is_empty() {
        let fitted = fit_clean_ellipsis(text, size, &font, max_w);
        style::label_fit(g, x, y_center, style::Align::Left, fitted, size, font, ink, max_w);
        return;
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in spans {
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }
    // Warm gold fill paired with dark ink in BOTH light and dark mode (Opus #10:
    // dark-mode gold darkened ~15% and 1.5 pt horizontal pill padding around matches).
    let hl_bg = if p.mode == olo_theme::ThemeMode::Dark { style::c([0xc9, 0xa7, 0x36, 0xff]) } else { style::c([0xf6, 0xd8, 0x62, 0xff]) };
    let hl_ink = style::c([0x18, 0x16, 0x12, 0xff]);
    let baseline = style::baseline_at(g, y_center, size);
    let hl_top = baseline - px(size * 0.85);
    let hl_bot = baseline + px(size * 0.25);
    let total_w = style::text_width(text.to_owned(), size, font.clone());
    let ell_w = style::text_width("…".to_owned(), size, font.clone());
    let clip_right = if total_w > max_w { x + (max_w - ell_w).max(px(0.0)) } else { x + max_w };
    for &(s, e) in &merged {
        let sx = x + style::text_width(text[..s].to_owned(), size, font.clone());
        let ex = x + style::text_width(text[..e].to_owned(), size, font.clone());
        if sx < clip_right {
            let rx0 = (sx - px(1.0)).max(x - px(1.0));
            let rx1 = (ex + px(1.0)).min(clip_right);
            if rx1 > rx0 {
                style::fill_rect(g, style::snap(g, Bounds::from_corners(point(rx0, hl_top), point(rx1, hl_bot))), hl_bg);
            }
        }
    }
    let mut runs = Vec::new();
    let mut cur = 0usize;
    for (s, e) in merged {
        if s > cur {
            runs.push(olo_kit::TextRun { len: s - cur, font: font.clone(), color: ink, background_color: None, underline: None, strikethrough: None, size: None });
        }
        runs.push(olo_kit::TextRun { len: e - s, font: font.clone(), color: hl_ink, background_color: None, underline: None, strikethrough: None, size: None });
        cur = e;
    }
    if cur < text.len() {
        runs.push(olo_kit::TextRun { len: text.len() - cur, font, color: ink, background_color: None, underline: None, strikethrough: None, size: None });
    }
    style::label_runs_fit(g, x, y_center, style::Align::Left, olo_kit::SharedString::from(text.to_owned()), size, &runs, max_w);
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

/// A pane's head's ground: the panel's, a line along its foot, and
/// where the keyboard is a line there in the look's colour. A tiled
/// pane's head and a floating pane's bar, alike.
fn head_ground(focused: bool, key: u64) -> Element {
    canvas(move |g, b| {
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
    .inset_0()
}

/// The icon in a pane's head: what runs in it, and how it stands.
fn head_icon(activity: AgentActivity, pane_help: crate::agent::Help) -> Element {
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
    .into_element()
}

/// The title in a pane's head: a weight heavier where the keyboard is,
/// quieter elsewhere; cut with "…" where it has no room.
fn head_title(title: String, focused: bool) -> Element {
    let p = theme::palette();
    let title_ink = if focused { style::c(p.panel_text) } else { style::secondary_text(&p) };
    let title_w = if focused { FontWeight::MEDIUM } else { FontWeight::NORMAL };
    let tkey = style::key(("pane-title", &title, focused, style::ink_key(Some(title_ink))));
    canvas(move |g, b| {
        let r = style::label_fit(g, b.left(), b.center().y, style::Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(title_w), title_ink, b.size.width);
        g.agent(agent::Node::label(title.clone(), r));
    })
    .paint_key(tkey)
    .w_full()
    .min_w_0()
    .h(px(m::LABEL_ROW))
    .into_element()
}

/// What a floating pane is called (as every pane is: `hub::pane_title`)
/// and how what runs in it stands; its label, where the pane isn't
/// known yet.
fn float_said(bk: &BlockKey, label: &str, is_local: bool) -> (String, AgentActivity) {
    let br = the_hub().state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(bk).cloned();
    br.map(|br| {
        let g = br.lock().unwrap_or_else(|e| e.into_inner());
        (g.said(if is_local { crate::hub::local_home() } else { None }.as_deref()), g.agent_activity)
    })
    .unwrap_or_else(|| (if label.is_empty() { "Floating Terminal".to_owned() } else { label.to_owned() }, AgentActivity::Idle))
}

/// A floating pane put away: its head alone, small, at the panes' foot
/// (the same icon and name, the card's line round it and its shadow
/// under it), and a button: pressed, the pane is back. (It was a push
/// button with the pane's label: no icon, nothing to say it was a pane,
/// and in a dark look its edge was lost on the terminal under it.)
fn float_chip(id: String, title: String, activity: AgentActivity, help: crate::agent::Help, on_restore: impl Fn() + 'static) -> Element {
    let font = style::ui_font(FontWeight::NORMAL);
    let text_w = f32::from(style::text_width(title.clone(), style::PLAIN_SIZE, font.clone())).ceil().min(FLOAT_CHIP_NAME);
    let width = m::TIGHT + ROW_ICON + m::TIGHT + text_w + m::SPACING;
    let name = format!("Restore {title}");
    let key = style::key(("float-chip", &title, activity as u8, &help));
    let face = widgets::control(id, width, crate::settings::PANE_HEADER, move |g, r, s| {
        let p = theme::palette();
        let r = style::snap(g, r);
        let over = s.enabled && (s.hovered || s.pressed);
        // (Under the pointer a push button's face, as the head's own marks have: raised, sunk while pressed.)
        if over {
            widgets::button_face(g, r, s);
        } else {
            style::fill_rect(g, r, style::c(p.panel_background));
        }
        let text_middle = style::baseline_at(g, r.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(r.left() + px(m::TIGHT + ROW_ICON / 2.0), text_middle), ROW_ICON, ROW_ICON);
        paint_activity_icon(g, icon_b, activity, &help);
        let ink = if over { style::c(p.control_text) } else { style::secondary_text(&p) };
        style::label_fit(g, r.left() + px(m::TIGHT + ROW_ICON + m::TIGHT), r.center().y, style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), ink, px(text_w + 1.0));
        if !over {
            style::frame_rect(g, r, float_edge(&p));
        }
    })
    .look(key)
    .agent("Button", name.clone(), None)
    .tip(name)
    .on_click(move |_, _| on_restore());
    div().relative().flex_shrink_0().child(float_shadow()).child(face).into_element()
}

/// The widest a put-away floating pane's name is drawn: cut with "…" past it.
const FLOAT_CHIP_NAME: f32 = 180.0;

/// The soft shadow alone, under something small that floats over the panes (a put-away pane's chip).
fn float_shadow() -> Element {
    canvas(|g, b| {
        let card = style::snap(g, Bounds::from_corners(point(b.left() + px(FLOAT_SHADOW), b.top() + px(FLOAT_SHADOW)), point(b.right() - px(FLOAT_SHADOW), b.bottom() - px(FLOAT_SHADOW))));
        style::soft_shadow(g, card, &theme::palette());
    })
    .paint_key(74)
    .absolute()
    .top(px(-FLOAT_SHADOW))
    .left(px(-FLOAT_SHADOW))
    .right(px(-FLOAT_SHADOW))
    .bottom(px(-FLOAT_SHADOW))
    .into_element()
}

/// Under a floating pane's card: its ground, the panel's, and round it
/// a soft shadow, as whatever floats over the rest has. Larger than the
/// card by [`FLOAT_SHADOW`] on each side, for the shadow.
fn float_under() -> Element {
    canvas(|g, b| {
        let p = theme::palette();
        let card = style::snap(g, Bounds::from_corners(point(b.left() + px(FLOAT_SHADOW), b.top() + px(FLOAT_SHADOW)), point(b.right() - px(FLOAT_SHADOW), b.bottom() - px(FLOAT_SHADOW))));
        style::soft_shadow(g, card, &p);
        style::fill_rect(g, card, style::c(p.panel_background));
        g.agent(agent::Node { role: "Dialog", name: Some("Floating Terminal".into()), bounds: card, enabled: true, ..Default::default() });
    })
    .paint_key(72)
    .absolute()
    .top(px(-FLOAT_SHADOW))
    .left(px(-FLOAT_SHADOW))
    .right(px(-FLOAT_SHADOW))
    .bottom(px(-FLOAT_SHADOW))
    .into_element()
}

/// Over a floating pane's card: a line all round it that reads over a
/// terminal's ground in every look. (A raised panel's light top and
/// left edge is white on a white terminal: the card had no edge there
/// at all.) Drawn last, on the card's own bounds, over the outermost
/// pixel of its head and its terminal, so nothing in the card covers
/// it and nothing shows between it and them at any scale: drawn under
/// them, with them a point inside, at 1.25× the head and the terminal
/// covered its right side, and at 1.5× a pixel of the ground stood
/// between it and the head's line at the left. It takes no pointer.
fn float_line() -> Element {
    canvas(|g, b| style::frame_rect(g, b, float_edge(&theme::palette()))).paint_key(73).absolute().inset_0().into_element()
}

/// A floating pane's grip, in its bottom right corner: three ridges
/// across the corner, the longest ten points, three points apart, each
/// a stair of whole device pixels at every scale, in the ink of the
/// head's marks (quieter while another pane has the keyboard). (It was
/// two legs of an L a shade darker than the panel: a second corner
/// inside the corner, which read as a crop mark, black on a dark
/// terminal, and a hairline at some scales: both reviewers,
/// 2026-11-27.)
fn float_grip(g: &mut Painter, b: Bounds<Pixels>, focused: bool) {
    let p = theme::palette();
    let s = g.scale_factor();
    // (A ridge one device pixel thick below 2×: two-pixel stairs three
    // points apart ran together into a dotted triangle at 1.5×.)
    let step = style::line_pixels(s);
    let whole = |points: f32| (points * s).round().max(1.0);
    let corner = style::snap(g, b).bottom_right();
    let (right, bottom) = (f32::from(corner.x) * s - whole(3.0), f32::from(corner.y) * s - whole(3.0));
    let ink = if focused { style::secondary_text(&p) } else { style::secondary_text(&p).opacity(0.7) };
    for ridge in 0..3 {
        let long = whole(10.0 - 3.0 * ridge as f32);
        let steps = (long / step).floor() as i32;
        for at in 0..steps {
            let (x, y) = (right - long + at as f32 * step, bottom - step - at as f32 * step);
            style::fill_rect(g, Bounds::from_corners(point(px(x / s), px(y / s)), point(px((x + step) / s), px((y + step) / s))), ink);
        }
    }
}

/// How far a floating pane's shadow reaches past its card: whole device
/// pixels at every quarter scale, so the card under it is the card's
/// own pixels. (Ten points is 12.5 and 17.5 pixels at 1.25× and 1.75×:
/// the ground stood a pixel out past the line there.)
const FLOAT_SHADOW: f32 = 12.0;

/// A floating pane's grip's box, in its bottom right corner: what takes the pointer.
const FLOAT_GRIP: f32 = 16.0;

/// The line round a floating pane's card: a clear step off the lines
/// between panes and off a terminal's ground, in every look. Near black
/// in a light look, as a push button's outline is; in a dark one a
/// light grey, half way from the panel to white. (A third of the way,
/// it was close to the dividers' tone and the card read as more of the
/// grid: Opus's review, 2026-11-27. The palette's own `control_border`
/// is near black in a dark look, darker than a terminal's ground: no
/// edge at all, measured.)
fn float_edge(p: &olo_theme::Palette) -> olo_kit::Hsla {
    if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.5)) } else { style::shade(p, p.panel_background, 1.75) }
}

/// A floating layer being moved or sized: where the pointer took it,
/// and its place then (shares of the panes' area).
#[derive(Default)]
struct FloatDrag(Option<(Point<Pixels>, [f64; 4])>);

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

/// Dedicated horizontal column for the parent row's disclosure button so
/// clicking the latch never depends on paint-cached hit rects and never collides
/// with the pane icon.
const ACTIVITY_FOLD_W: f32 = 20.0;

/// Native BeOS child-tab breadcrumb header bar matching [`crate::settings::PANE_HEADER`]
/// (`22 pt`, identical to pane title bars so parent↔child switching never jumps terminal text):
/// `parent › child › Current` aligned flush with the terminal's first text column (`12 pt`)
/// on the left (with BeOS hover frames on clickable ancestors) and right-aligned
/// `harness · model · effort` with width-driven whole-segment dropping on narrow tiles.
#[allow(clippy::too_many_arguments)]
fn render_child_breadcrumb_header(
    hk: String,
    sid: String,
    ancestors: Vec<(String, String)>,
    entry: crate::subagents::NativeSubagent,
    _act: AgentActivity,
    _running: bool,
    avail_w: f32,
    cx: &mut olo_kit::Cx<RexWindow>,
) -> Element {
    let hdr_h = crate::settings::PANE_HEADER;
    let pad_x = crate::settings::PADDING.0;

    let head_bg = canvas(|g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.panel_background));
        style::hline_above(g, b.left(), b.right(), b.bottom(), pane_edge(&p));
    })
    .paint_key(912)
    .absolute()
    .inset_0();

    const SEP_W: f32 = 18.0;
    let mut fixed_left_w = 0.0f32;
    let mut crumb = div().flex_1().min_w_0().flex().flex_row().items_center();
    for (wid, label) in ancestors {
        let hk_c = hk.clone();
        let sid_c = sid.clone();
        let wid_c = wid.clone();
        let nav = cx.msg(WinMsg::SelectWindow(hk_c, sid_c, wid_c));
        let link_label = label.clone();
        let link_w = f32::from(style::text_width(label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        fixed_left_w += link_w + SEP_W;
        let seg_id = olo_kit::SharedString::from(format!("rex-crumb-{wid}"));
        let seg = canvas(move |g, b| {
            let hovered = g.node_state::<std::cell::Cell<bool>>().get();
            let p = theme::palette();
            if hovered {
                let dark = p.mode == olo_theme::ThemeMode::Dark;
                let pill = Bounds::from_corners(point(b.left() - px(5.0), b.top() + px(2.0)), point(b.right() + px(5.0), b.bottom() - px(2.0)));
                let snapped = style::snap(g, pill);
                let fill_c = if dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.08)) } else { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.35)) };
                let shine = if dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.22)) } else { style::c(p.shine) };
                let shadow = if dark { style::c(olo_theme::mix(p.panel_background, [0, 0, 0, 255], 0.45)) } else { style::c(olo_theme::mix(p.panel_background, [0, 0, 0, 255], 0.24)) };
                style::fill_rect(g, snapped, fill_c);
                style::bevel(g, snapped, shine, shadow);
            }
            let ink = if hovered { style::c(p.panel_text) } else { style::secondary_text(&p) };
            style::label(g, b.center().x, b.center().y, style::Align::Center, link_label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
            g.agent(agent::Node::button(format!("Switch to {link_label}"), b));
        })
        .id(seg_id)
        .w(px(link_w))
        .h(px(hdr_h))
        .flex_shrink_0()
        .cursor(CursorStyle::PointingHand)
        .on_hover(|over: &bool, cx| {
            let cell = cx.node_state::<std::cell::Cell<bool>>();
            if cell.get() != *over {
                cell.set(*over);
                cx.repaint();
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            nav(&(), cx);
            cx.stop_propagation();
        });
        let sep = canvas(|g, b| {
            let p = theme::palette();
            style::label(g, b.center().x, b.center().y, style::Align::Center, "›", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
        })
        .paint_key(913)
        .w(px(SEP_W))
        .h(px(hdr_h))
        .flex_shrink_0();
        crumb = crumb.child(seg).child(sep);
    }
    let current_title = entry.display_title();
    let cur_w = f32::from(style::text_width(current_title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD))).ceil().min(140.0);
    let cur_el = div().flex_1().min_w_0().child(StringView::new(current_title).bold(true).cut(true).height(hdr_h));
    crumb = crumb.child(cur_el);

    // Width-driven segment dropping from lowest priority first (effort, then model, then agent):
    // never truncate mid-word; drop trailing segments cleanly when tiled narrow.
    let meta_room = avail_w - 2.0 * pad_x - fixed_left_w - cur_w - m::SPACING;
    let small_font = style::ui_font(FontWeight::NORMAL);
    // (A command of the asker's own has no model or effort to drop.)
    let candidates = if entry.cli.is_agent() { vec![entry.what(), format!("{} · {}", entry.cli.chrome_name(), entry.model), entry.cli.chrome_name().to_owned()] } else { vec![entry.what()] };
    let chosen_meta = candidates.into_iter().find_map(|s| {
        let w = f32::from(style::text_width(s.clone(), style::SMALL_SIZE, small_font.clone())).ceil() + 2.0;
        (w <= meta_room).then_some((s, w))
    });

    let mut bar = div().relative().w_full().h(px(hdr_h)).flex_shrink_0().px(px(pad_x)).flex().flex_row().items_center().child(head_bg).child(crumb);

    if let Some((meta, meta_w)) = chosen_meta {
        let meta_paint = meta.clone();
        let meta_el = canvas(move |g, b| {
            let p = theme::palette();
            let baseline = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
            style::label_on_baseline(g, b.right(), baseline, style::Align::Right, meta_paint.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
            g.agent(agent::Node::label(meta_paint.clone(), b));
        })
        .paint_key(style::key(("child-hdr-meta", &meta)))
        .ml(px(m::SPACING))
        .w(px(meta_w))
        .h(px(hdr_h))
        .flex_shrink_0();
        bar = bar.child(meta_el);
    }
    bar
}

/// Strip the leading `<cli> · ` prefix from an agent detail string when rendering
/// Extract the leading harness/CLI prefix from a row detail (`"claude · ..."` -> `Some("claude")`).
fn detail_cli_prefix(detail: &str) -> Option<&str> {
    detail.split_once(" · ").map(|(pfx, _)| pfx.trim()).filter(|s| !s.is_empty())
}

/// Paint line 2 of a collapsed agent row: optional `<cli> · ` in secondary text
/// followed by `<rollup>` (bold Waiting-orange / failure-red when urgent, else
/// secondary text — no truncated activity text after the rollup, Opus #6).
#[allow(clippy::too_many_arguments)]
fn paint_collapsed_sub_line(g: &mut olo_kit::Painter, left_x: Pixels, y_center: Pixels, cli_prefix: Option<&str>, rollup: &str, urg: u8, sub_max: Pixels, p: &olo_theme::Palette) {
    let quiet = style::secondary_text(p);
    let rollup_ink = match urg {
        2 => style::c(p.failure),
        1 => crate::transcript_view::waiting_chrome_color(p),
        _ => quiet,
    };
    let rollup_w = if urg > 0 { FontWeight::BOLD } else { FontWeight::NORMAL };
    if let Some(pfx) = cli_prefix.filter(|s| !s.is_empty()) {
        let head = format!("{pfx} · ");
        let full = format!("{head}{rollup}");
        let runs = vec![
            olo_kit::TextRun { len: head.len(), font: style::ui_font(FontWeight::NORMAL), color: quiet, background_color: None, underline: None, strikethrough: None, size: None },
            olo_kit::TextRun { len: rollup.len(), font: style::ui_font(rollup_w), color: rollup_ink, background_color: None, underline: None, strikethrough: None, size: None },
        ];
        style::label_runs_fit(g, left_x, y_center, style::Align::Left, olo_kit::SharedString::from(full), style::SMALL_SIZE, &runs, sub_max);
    } else {
        style::label_fit(g, left_x, y_center, style::Align::Left, rollup.to_owned(), style::SMALL_SIZE, style::ui_font(rollup_w), rollup_ink, sub_max);
    }
}

/// Compact single-line height for non-matching context-only filter ancestors (Opus #8).
const ANCESTOR_ROW_H: f32 = 22.0;

#[allow(clippy::too_many_arguments)]
fn sidebar_activity_row(
    row: ActivityRow,
    icon_act: AgentActivity,
    selected: bool,
    menu_target: bool,
    ancestor_only: bool,
    has_children: bool,
    expanded: bool,
    total_children: usize,
    total_screens: usize,
    failed_children: usize,
    waiting_children: usize,
    query_terms: &[String],
    width: f32,
    row_bounds_map: Rc<std::cell::RefCell<HashMap<String, Bounds<Pixels>>>>,
    row_bounds_key: String,
    on_click: impl Fn(&(), &mut WindowCx) + 'static,
    on_toggle_fold: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let ActivityRow { title, detail, act, help, running, parent_agent, .. } = row;
    let own_state = if ancestor_only { None } else { activity_state_word(act, running) };
    let left = ACTIVITY_FOLD_W + ROW_ICON + m::TIGHT;
    let sub_room = (width - left - 8.0).max(48.0);
    // Opus #8: when collapsed with urgent hidden children (`failed_children > 0 || waiting_children > 0`),
    // place the urgent rollup (`1 waiting` / `1 failed`) in the right-hand status slot on line 1
    // and keep line 2 showing the agent's normal activity detail! Neutral rollups (`4 screens`,
    // `1 subagent · 4 screens`) stay on line 2 as `<cli> · <rollup>`.
    let urgent_collapsed_status: Option<(String, u8)> = if !ancestor_only && has_children && !expanded && (failed_children > 0 || waiting_children > 0) {
        crate::activity_screens::format_collapsed_rollup(total_children, total_screens, failed_children, waiting_children)
    } else {
        None
    };
    let cli_prefix_owned: Option<String> = detail_cli_prefix(&detail).map(str::to_owned).or_else(|| parent_agent.map(|h| if h == "agy" { "antigravity".to_owned() } else { h }));
    let collapsed_rollup: Option<(Option<String>, String, u8)> = if !ancestor_only && has_children && !expanded && urgent_collapsed_status.is_none() {
        crate::activity_screens::fit_collapsed_rollup_line(cli_prefix_owned.as_deref(), total_children, total_screens, failed_children, waiting_children, sub_room, style::SMALL_SIZE)
    } else {
        None
    };
    let sub_line = if ancestor_only || detail.is_empty() || collapsed_rollup.is_some() {
        String::new()
    } else if query_terms.is_empty() {
        fit_clean_line(&detail, sub_room, style::SMALL_SIZE)
    } else {
        fit_query_snippet(&detail, query_terms, sub_room, style::SMALL_SIZE)
    };
    let height = if ancestor_only { ANCESTOR_ROW_H } else { ACTIVITY_ROW };
    let terms_owned: Vec<String> = query_terms.to_vec();
    let key = style::key((
        (&title, &sub_line, &collapsed_rollup, &urgent_collapsed_status, act as u8, icon_act as u8, &help, running),
        (selected, menu_target, ancestor_only, has_children, expanded, total_children, total_screens, failed_children, waiting_children, &terms_owned),
    ));
    let words = activity_row_words(act, running, &help);
    let on_toggle_fold = Rc::new(on_toggle_fold);

    let row_canvas = canvas(move |g, b| {
        row_bounds_map.borrow_mut().insert(row_bounds_key.clone(), b);
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let ground = if selected { style::c(style::selection_ground(&p, p.panel_background, false)) } else { style::c(p.panel_background) };
        if selected {
            style::fill_rect(g, b, ground);
        }
        if menu_target {
            // Opus #11 / Neon-look #1: inset 2 pt vertically and 1 pt horizontally so
            // the target outline never merges with an adjacent selected row's fill.
            let outline_r = style::snap(g, Bounds::from_corners(point(b.left() + px(1.0), b.top() + px(2.0)), point(b.right() - px(1.0), b.bottom() - px(2.0))));
            g.paint_quad(olo_kit::outline(outline_r, style::c(p.keyboard_navigation), olo_kit::BorderStyle::Solid));
        }
        let (ink, quiet, muted) = (style::c(p.panel_text), style::secondary_text(&p), style::disabled_text(&p));
        let title_ink = if ancestor_only { quiet } else { ink };
        let line1_y = if ancestor_only { b.center().y } else { b.top() + px(12.0) };
        let baseline = style::baseline_at(g, line1_y, style::PLAIN_SIZE);
        let name_mid = baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(b.left() + px(ACTIVITY_FOLD_W + ROW_ICON / 2.0), name_mid), ROW_ICON, ROW_ICON);
        if ancestor_only {
            // Opus Minor: dimmed context ancestors paint their coloured icon at 60%
            // opacity so a grey unlit icon ALWAYS means Stopped.
            g.with_element_opacity(Some(0.60), |g| {
                if icon_act == AgentActivity::Done {
                    icons::paint(g, icon_b, "terminal");
                } else {
                    widgets::strip::drawn_on(ground, || {
                        widgets::agent_marks::paint(g, icon_b, "terminal", marks_of(icon_act, &crate::agent::Help::default(), running));
                    });
                }
            });
        } else if icon_act == AgentActivity::Done {
            icons::paint(g, icon_b, "terminal");
        } else {
            widgets::strip::drawn_on(ground, || {
                widgets::agent_marks::paint(g, icon_b, "terminal", marks_of(icon_act, &crate::agent::Help::default(), running));
            });
        }
        if has_children && expanded {
            let guide_x = style::snap_coord(g, icon_b.center().x);
            let sel_rgba = style::selection_ground(&p, p.panel_background, false);
            let guide_c = if dark {
                let base_g = if selected { sel_rgba } else { p.panel_background };
                style::c(olo_theme::mix(base_g, p.panel_text, if selected { 0.42 } else { 0.34 }))
            } else {
                style::shade(&p, if selected { sel_rgba } else { p.panel_background }, 1.32)
            };
            let stem_top = (icon_b.bottom() + px(2.0)).min(b.bottom());
            if b.bottom() - stem_top >= px(2.0) {
                style::vline(g, guide_x, stem_top, b.bottom(), guide_c);
            }
        }

        let right_edge = b.right() - px(8.0);
        let mut right = right_edge;
        if !ancestor_only && help.auto_approve {
            widgets::agent_marks::auto_approve_mark(g, point(right - px(4.5), name_mid), 9.0, ground);
            right -= px(9.0 + m::MARK_GAP);
        }
        let left_x = b.left() + px(left);
        if let Some((ref uword, urg)) = urgent_collapsed_status
            && !own_state.as_ref().is_some_and(|(_, u)| u.is_some())
        {
            let color = if urg >= 2 { style::c(p.failure) } else { crate::transcript_view::waiting_chrome_color(&p) };
            let at = style::label_on_baseline(g, right, baseline, style::Align::Right, uword.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), color);
            right = at.left();
        } else if let Some((word, urgent)) = own_state {
            let color = match urgent {
                Some(_) if act == AgentActivity::Error => style::c(p.failure),
                Some(_) => crate::transcript_view::waiting_chrome_color(&p),
                None if act == AgentActivity::Done => muted,
                None => quiet,
            };
            let weight = if urgent.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL };
            let at = style::label_on_baseline(g, right, baseline, style::Align::Right, olo_kit::words::tr(word).to_string(), style::SMALL_SIZE, style::ui_font(weight), color);
            right = at.left();
        }
        let title_gap = if right < right_edge { px(8.0) } else { px(0.0) };
        let title_weight = if ancestor_only { FontWeight::NORMAL } else { FontWeight::BOLD };
        // Opus #11 & Agy-look Check #3: crisp secondary-size `↻` with strong
        // light/dark contrast so it remains readable at 1×.
        let show_loop = !ancestor_only && help.keep_going;
        let loop_reserve = if show_loop { px(14.0) } else { px(0.0) };
        let title_max = (right - title_gap - loop_reserve - left_x).max(px(24.0));
        paint_query_label(g, left_x, line1_y, &title, style::PLAIN_SIZE, title_weight, title_ink, title_max, &terms_owned, &p);
        if show_loop {
            let tw = style::text_width(title.clone(), style::PLAIN_SIZE, style::ui_font(title_weight)).min(title_max);
            let loop_ink = if dark { quiet } else { style::c(olo_theme::readable(p.panel_text, p.panel_background, 5.2)) };
            style::label_on_baseline(g, left_x + tw + px(4.0), baseline, style::Align::Left, "↻", style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), loop_ink);
        }

        if !ancestor_only {
            let sub_max = (right_edge - left_x).max(px(24.0));
            if let Some((ref pfx, ref rtext, urg)) = collapsed_rollup {
                paint_collapsed_sub_line(g, left_x, b.top() + px(26.0), pfx.as_deref(), rtext, urg, sub_max, &p);
            } else if !sub_line.is_empty() {
                paint_query_label(g, left_x, b.top() + px(26.0), &sub_line, style::SMALL_SIZE, FontWeight::NORMAL, quiet, sub_max, &terms_owned, &p);
            }
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
    .size_full()
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    });

    let mut wrapper = div().relative().w_full().h(px(height)).child(row_canvas);
    if has_children {
        let fold_cb = on_toggle_fold.clone();
        let fold_label = if expanded { "Collapse subagents" } else { "Expand subagents" };
        let latch_key = style::key(("act-fold-btn", expanded, selected, ancestor_only));
        let latch_btn = canvas(move |g, b| {
            let p = theme::palette();
            let quiet = style::secondary_text(&p);
            let line1_y = if ancestor_only { b.center().y } else { b.top() + px(12.0) };
            let baseline = style::baseline_at(g, line1_y, style::PLAIN_SIZE);
            let name_mid = baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
            crate::transcript_view::paint_disclosure(g, point(b.left() + px(11.0), name_mid), expanded, quiet);
            g.agent(agent::Node { role: "Button", name: Some(fold_label.into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
        })
        .paint_key(latch_key)
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(ACTIVITY_FOLD_W))
        .h(px(height))
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            fold_cb(&(), cx);
            cx.stop_propagation();
        });
        wrapper = wrapper.child(latch_btn);
    }
    wrapper
}

/// Live native child-tab subagent row rendered inside the Activity sidebar tree.
#[derive(Clone)]
struct NativeChildItem {
    row: crate::subagents::TreeRow,
    bk: BlockKey,
    act: AgentActivity,
    detail: String,
    running: bool,
    /// A command of the asker's own, not an agent: "Running" and no
    /// agent's mark on its icon.
    command: bool,
    help: crate::agent::Help,
    has_children: bool,
    /// Screens controlled directly by this child/grandchild subagent.
    screens: Vec<crate::activity_screens::ActivityScreenItem>,
    /// True when shown only as a muted ancestor on the way to a matched child or screen.
    ancestor_only: bool,
}

/// A child of a tab that stopping its children would stop: its tab, its
/// name, and whether it is a command of the asker's own (not an agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningChild {
    window_id: String,
    name: String,
    command: bool,
    /// A minder of several agents (its agents are its children): stopped
    /// after them, since a minder that goes first has the service put
    /// each of its agents back under the tab and tell the tab's agent so.
    minder: bool,
}

/// The order a tab's children are stopped in (their tabs' ids): agents
/// and commands first, their minders after them. A minder stopped
/// first has the service put each of its agents back under the tab and
/// tell the tab's own agent "its minder ended: what it asks comes to you
/// again", of agents that are stopped a moment later.
fn stop_order(children: &[RunningChild]) -> Vec<String> {
    children.iter().filter(|c| !c.minder).chain(children.iter().filter(|c| c.minder)).map(|c| c.window_id.clone()).collect()
}

/// What stopping a tab's children is called, by what they are: agents
/// are subagents, a command of the asker's own is a command ("Stop 2
/// Subagents and 1 Command…"), so the words say what would be stopped.
fn stop_children_label(kids: &[RunningChild]) -> String {
    let commands = kids.iter().filter(|k| k.command).count();
    let agents = kids.len() - commands;
    let said = |count: usize, one: &str, many: &str| match count {
        1 if kids.len() == 1 => one.to_owned(),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    match (agents, commands) {
        (0, 0) => "Stop Subagents…".to_owned(),
        (a, 0) => format!("Stop {}…", said(a, "Subagent", "Subagents")),
        (0, c) => format!("Stop {}…", said(c, "Command", "Commands")),
        (a, c) => format!("Stop {} and {}…", said(a, "Subagent", "Subagents"), said(c, "Command", "Commands")),
    }
}

/// A command's line where its row has no room for it whole. Its end says
/// the most (the file a `tail -f` follows), so the folders of its paths
/// go first ("tail -f …/build.log"), then the mark that they went ("tail
/// -f build.log"), and only then its end.
fn fit_command_line(text: &str, room: f32, size: f32) -> String {
    let text = text.trim();
    if text.is_empty() || room <= 24.0 {
        return text.to_owned();
    }
    let font = style::ui_font(FontWeight::NORMAL);
    let fits = |s: &str| f32::from(style::text_width(s.to_owned(), size, font.clone())) <= room;
    if fits(text) {
        return text.to_owned();
    }
    let marked = paths_by_name(text, "…/");
    if fits(&marked) {
        return marked;
    }
    fit_clean_line(&paths_by_name(text, ""), room, size)
}

/// `text` with each path in it down to its last name, after `mark`
/// ("/var/tmp/olo/build.log": "…/build.log"). An option's value is a
/// path too (`--log=/var/log/x`); an address (`https://…`) is not.
fn paths_by_name(text: &str, mark: &str) -> String {
    let short = |path: &str| -> Option<String> {
        if path.contains("://") {
            return None;
        }
        let (dir, name) = path.trim_end_matches('/').rsplit_once('/')?;
        let made = format!("{mark}{name}");
        (!dir.is_empty() && !name.is_empty() && made.chars().count() < path.chars().count()).then_some(made)
    };
    text.split(' ')
        .map(|word| match word.split_once('=').filter(|_| word.starts_with('-')) {
            Some((option, value)) => short(value).map(|v| format!("{option}={v}")).unwrap_or_else(|| word.to_owned()),
            None => short(word).unwrap_or_else(|| word.to_owned()),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn top_level_window_indices(subagents: &crate::subagents::SubagentRegistry, hk: &str, sess: &olo_rex_client::SessionView) -> (HashSet<String>, Vec<usize>) {
    let live: HashSet<String> = sess.windows.iter().map(|w| w.window_id.clone()).collect();
    let indices: Vec<usize> = sess.windows.iter().enumerate().filter(|(_, w)| !subagents.is_child_window(hk, &sess.session_id, &w.window_id, &live)).map(|(i, _)| i).collect();
    (live, indices)
}

fn detect_agent_harness(program: Option<&str>, title: &str) -> Option<String> {
    let hay = format!("{} {}", program.unwrap_or(""), title).to_ascii_lowercase();
    if hay.contains("claude") {
        Some("claude".into())
    } else if hay.contains("codex") {
        Some("codex".into())
    } else if hay.contains("antigravity") || hay.contains("agy") {
        Some("agy".into())
    } else {
        None
    }
}

/// Compact 2-line native nested subagent row (`38 pt`) supporting arbitrary
/// parent -> child -> grandchild tree depth with connected 1px BeOS tree guides,
/// collapsible intermediate parents (disclosure triangle on branch + rollup),
/// and full-width character-accurate subtitle fitting.
#[allow(clippy::too_many_arguments)]
fn sidebar_native_child_row(
    child: NativeChildItem,
    icon_act: AgentActivity,
    selected: bool,
    menu_target: bool,
    expanded: bool,
    (sub_total, sub_screens, sub_failed, sub_waiting): (usize, usize, usize, usize),
    query_terms: &[String],
    width: f32,
    row_bounds_map: Rc<std::cell::RefCell<HashMap<String, Bounds<Pixels>>>>,
    row_bounds_key: String,
    on_click: impl Fn(&(), &mut WindowCx) + 'static,
    on_toggle_fold: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    const CHILD_INDENT: f32 = 15.0;
    const SUB_ROW_H: f32 = 38.0;

    let depth = child.row.depth.clamp(1, 4);
    let indent = (depth as f32) * CHILD_INDENT;
    // (Its name as far from its icon as a top row's is: with 4 points a
    // child's mark crowded its name, and the gap differed by level, Opus
    // 5.5's look of 2026-11-26.)
    let left = ACTIVITY_FOLD_W + indent + ROW_ICON + m::TIGHT;
    let title = child.row.entry.display_title();
    let command = child.command;
    let ancestor_only = child.ancestor_only;
    // Opus #1: drop redundant "Working"/"Running" words; keep Waiting, Failed, Done.
    let own_state = if ancestor_only || command { None } else { activity_state_word(child.act, child.running) };
    let has_children = child.has_children;

    let desc_room = (width - left - 8.0).max(48.0);
    let cli_word = child.row.entry.cli.chrome_name();
    // Opus #8: for a collapsed parent with urgent hidden children (e.g. `API Audit` ->
    // `1 waiting`), put the bold urgent rollup in the right-hand status slot on line 1
    // and keep line 2 showing the parent's live activity detail (`claude · Checking…`)!
    let urgent_collapsed_status: Option<(String, u8)> = if !ancestor_only && has_children && !expanded && (sub_failed > 0 || sub_waiting > 0) {
        crate::activity_screens::format_collapsed_rollup(sub_total, sub_screens, sub_failed, sub_waiting)
    } else {
        None
    };
    let collapsed_rollup: Option<(Option<String>, String, u8)> = if !ancestor_only && has_children && !expanded && urgent_collapsed_status.is_none() {
        let pfx = if command { None } else { Some(cli_word) };
        crate::activity_screens::fit_collapsed_rollup_line(pfx, sub_total, sub_screens, sub_failed, sub_waiting, desc_room, style::SMALL_SIZE)
    } else {
        None
    };

    let desc = if ancestor_only || child.detail.is_empty() || collapsed_rollup.is_some() {
        String::new()
    } else if !query_terms.is_empty() {
        fit_query_snippet(&child.detail, query_terms, desc_room, style::SMALL_SIZE)
    } else if command {
        fit_command_line(&child.detail, desc_room, style::SMALL_SIZE)
    } else {
        fit_clean_line(&child.detail, desc_room, style::SMALL_SIZE)
    };

    let row_h = if ancestor_only { ANCESTOR_ROW_H } else { SUB_ROW_H };
    let terms_owned: Vec<String> = query_terms.to_vec();
    let guides = child.row.guides.clone();
    let is_last = child.row.is_last;
    let act = child.act;
    let running = child.running;
    let help = child.help;
    let state_val = urgent_collapsed_status.as_ref().map(|(w, _)| w.as_str()).or_else(|| own_state.map(|(w, _)| w)).unwrap_or(if command || running { "Working" } else { "Idle" });
    let agent_val = format!("{cli_word}; {state_val}");
    let key = style::key((
        (&child.row.entry.id, &title, &desc, &collapsed_rollup, &urgent_collapsed_status, act as u8, icon_act as u8, running, command, ancestor_only, &help),
        (depth, is_last, has_children, expanded, sub_total, sub_screens, sub_failed, sub_waiting, &guides, selected, menu_target, &terms_owned),
    ));

    let title_for_fold = title.clone();
    let row_canvas = canvas(move |g, b| {
        row_bounds_map.borrow_mut().insert(row_bounds_key.clone(), b);
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let sel_rgba = style::selection_ground(&p, p.panel_background, false);
        let ground = if selected { style::c(sel_rgba) } else { style::c(p.panel_background) };
        if selected {
            style::fill_rect(g, b, ground);
        }
        if menu_target {
            // Opus #11 / Neon-look #1: inset 2 pt vertically and 1 pt horizontally.
            let outline_r = style::snap(g, Bounds::from_corners(point(b.left() + px(1.0), b.top() + px(2.0)), point(b.right() - px(1.0), b.bottom() - px(2.0))));
            g.paint_quad(olo_kit::outline(outline_r, style::c(p.keyboard_navigation), olo_kit::BorderStyle::Solid));
        }
        let (ink, quiet, muted) = (style::c(p.panel_text), style::secondary_text(&p), style::disabled_text(&p));
        let line1_y = if ancestor_only { b.center().y } else { b.top() + px(11.5) };
        let baseline1 = style::baseline_at(g, line1_y, style::PLAIN_SIZE);
        let name_mid = baseline1 - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(b.left() + px(ACTIVITY_FOLD_W + indent + ROW_ICON / 2.0), name_mid), ROW_ICON, ROW_ICON);

        let guide_c = if dark {
            let base_g = if selected { sel_rgba } else { p.panel_background };
            style::c(olo_theme::mix(base_g, p.panel_text, if selected { 0.48 } else { 0.34 }))
        } else {
            style::shade(&p, if selected { sel_rgba } else { p.panel_background }, 1.32)
        };

        // Ancestor continuation guides for nested depth >= 2.
        for (lvl, &more) in guides.iter().take(depth.saturating_sub(1)).enumerate() {
            if more {
                let gx = style::snap_coord(g, b.left() + px(ACTIVITY_FOLD_W + (lvl as f32) * CHILD_INDENT + ROW_ICON / 2.0));
                style::vline(g, gx, b.top(), b.bottom(), guide_c);
            }
        }
        // Own connector elbow from direct parent's icon center into this row's icon.
        let parent_x = style::snap_coord(g, b.left() + px(ACTIVITY_FOLD_W + ((depth - 1) as f32) * CHILD_INDENT + ROW_ICON / 2.0));
        if has_children {
            // Neon-look #3: descend the parent trunk cleanly down to a tight 5 pt
            // collar around the disclosure triangle (never stopping mid-air above
            // the row!) and continue cleanly beneath when `!is_last`.
            let clear = px(5.0);
            let gap_top = (name_mid - clear).max(b.top());
            if gap_top - b.top() >= px(1.5) {
                style::vline(g, parent_x, b.top(), gap_top, guide_c);
            }
            if !is_last {
                let gap_bot = (name_mid + clear).min(b.bottom());
                if b.bottom() - gap_bot >= px(2.0) {
                    style::vline(g, parent_x, gap_bot, b.bottom(), guide_c);
                }
            }
        } else {
            let guide_end = if is_last { name_mid } else { b.bottom() };
            style::vline(g, parent_x, b.top(), guide_end, guide_c);
            style::hline(g, parent_x, icon_b.left() - px(2.0), name_mid, guide_c);
        }

        // Downward stem connecting to nested grandchildren directly beneath this child's icon.
        if has_children && expanded {
            let own_x = style::snap_coord(g, icon_b.center().x);
            let stem_top = (icon_b.bottom() + px(2.0)).min(b.bottom());
            if b.bottom() - stem_top >= px(2.0) {
                style::vline(g, own_x, stem_top, b.bottom(), guide_c);
            }
        }

        if ancestor_only {
            // Opus Minor: dimmed filter ancestors paint their coloured icon at 60%
            // opacity so grey icons exclusively mean Stopped.
            g.with_element_opacity(Some(0.60), |g| {
                if icon_act == AgentActivity::Done {
                    icons::paint(g, icon_b, "terminal");
                } else {
                    widgets::strip::drawn_on(ground, || {
                        widgets::agent_marks::paint(g, icon_b, "terminal", marks_of(icon_act, &crate::agent::Help::default(), running && !command));
                    });
                }
            });
        } else if icon_act == AgentActivity::Done {
            icons::paint(g, icon_b, "terminal");
        } else {
            // (No agent's mark on a command of the asker's own: a terminal,
            // as any pane that runs a program is. What is under it folded
            // still shows through.)
            widgets::strip::drawn_on(ground, || {
                widgets::agent_marks::paint(g, icon_b, "terminal", marks_of(icon_act, &crate::agent::Help::default(), running && !command));
            });
        }

        let right_edge = b.right() - px(8.0);
        let mut right = right_edge;
        if !ancestor_only && help.auto_approve {
            widgets::agent_marks::auto_approve_mark(g, point(right - px(4.5), name_mid), 9.0, ground);
            right -= px(9.0 + m::MARK_GAP);
        }
        if let Some((ref uword, urg)) = urgent_collapsed_status
            && !own_state.as_ref().is_some_and(|(_, u)| u.is_some())
        {
            let state_color = if urg >= 2 { style::c(p.failure) } else { crate::transcript_view::waiting_chrome_color(&p) };
            let at = style::label_on_baseline(g, right, baseline1, style::Align::Right, uword.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), state_color);
            right = at.left();
        } else if let Some((word, urgent)) = own_state {
            let state_color = match urgent {
                Some(_) if act == AgentActivity::Error => style::c(p.failure),
                Some(_) => crate::transcript_view::waiting_chrome_color(&p),
                None if act == AgentActivity::Done => muted,
                None => quiet,
            };
            let state_weight = if urgent.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL };
            let at = style::label_on_baseline(g, right, baseline1, style::Align::Right, olo_kit::words::tr(word).to_string(), style::SMALL_SIZE, style::ui_font(state_weight), state_color);
            right = at.left();
        }
        let left_x = b.left() + px(left);
        let title_ink = if ancestor_only { quiet } else { ink };
        // Opus #7: keep all nested child titles regular weight so tree depth hierarchy
        // stays clear (urgent state is already carried by orange badge + bold status word).
        let title_weight = FontWeight::NORMAL;
        let title_gap = if right < right_edge { px(8.0) } else { px(0.0) };
        let show_loop = !ancestor_only && help.keep_going;
        let loop_reserve = if show_loop { px(14.0) } else { px(0.0) };
        let title_max = (right - title_gap - loop_reserve - left_x).max(px(24.0));
        paint_query_label(g, left_x, line1_y, &title, style::PLAIN_SIZE, title_weight, title_ink, title_max, &terms_owned, &p);
        if show_loop {
            let tw = style::text_width(title.clone(), style::PLAIN_SIZE, style::ui_font(title_weight)).min(title_max);
            let loop_ink = if dark { quiet } else { style::c(olo_theme::readable(p.panel_text, p.panel_background, 5.2)) };
            style::label_on_baseline(g, left_x + tw + px(4.0), baseline1, style::Align::Left, "↻", style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), loop_ink);
        }

        if !ancestor_only {
            let sub_max = (right_edge - left_x).max(px(24.0));
            if let Some((ref pfx, ref rtext, urg)) = collapsed_rollup {
                paint_collapsed_sub_line(g, left_x, b.top() + px(25.5), pfx.as_deref(), rtext, urg, sub_max, &p);
            } else if !desc.is_empty() {
                paint_query_label(g, left_x, b.top() + px(25.5), &desc, style::SMALL_SIZE, FontWeight::NORMAL, quiet, sub_max, &terms_owned, &p);
            }
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
    .size_full()
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    });

    let mut wrapper = div().relative().w_full().h(px(row_h)).child(row_canvas);
    if has_children {
        let latch_x = ACTIVITY_FOLD_W + ((depth - 1) as f32) * CHILD_INDENT + ROW_ICON / 2.0;
        let fold_label = format!("{} {}", if expanded { "Collapse" } else { "Expand" }, title_for_fold);
        let latch_key = style::key(("act-sub-fold", &child.row.entry.id, expanded, selected, ancestor_only));
        let latch_btn = canvas(move |g, b| {
            let p = theme::palette();
            let quiet = style::secondary_text(&p);
            let line1_y = if ancestor_only { b.center().y } else { b.top() + px(11.5) };
            let baseline = style::baseline_at(g, line1_y, style::PLAIN_SIZE);
            let name_mid = baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
            // Neon-look #3: align the disclosure triangle center with the exact
            // pixel center of the parent's vertical trunk (`snap_coord + line/2`).
            let scale = g.scale_factor();
            let line_w = style::line_pixels(scale) / scale;
            let trunk_cx = style::snap_coord(g, b.center().x) + px(line_w / 2.0);
            crate::transcript_view::paint_disclosure(g, point(trunk_cx, name_mid), expanded, quiet);
            g.agent(agent::Node { role: "Button", name: Some(fold_label.clone().into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
        })
        .paint_key(latch_key)
        .absolute()
        .left(px((latch_x - 8.0).max(0.0)))
        .top(px(0.0))
        .w(px(16.0))
        .h(px(row_h))
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            on_toggle_fold(&(), cx);
            cx.stop_propagation();
        });
        wrapper = wrapper.child(latch_btn);
    }
    wrapper
}

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
    host_key: String,
    host: String,
    title: String,
    detail: String,
    act: AgentActivity,
    help: crate::agent::Help,
    running: bool,
    parent_agent: Option<String>,
    /// Screens controlled directly by this top-level pane (rendered before `children`).
    screens: Vec<crate::activity_screens::ActivityScreenItem>,
    children: Vec<NativeChildItem>,
}

/// Recompute exact `(is_last, guides)` for visible child `idx` in a filtered
/// preorder `subs` list so filtered trees never draw dangling vertical guide lines.
fn recompute_sub_guides(subs: &[NativeChildItem], idx: usize) -> (bool, Vec<bool>) {
    let depth = subs[idx].row.depth.clamp(1, 4);
    let has_later_sibling_at = |target_depth: usize, from_idx: usize| -> bool {
        for item in &subs[from_idx + 1..] {
            let d = item.row.depth.clamp(1, 4);
            if d < target_depth {
                return false;
            }
            if d == target_depth {
                return true;
            }
        }
        false
    };
    let is_last = !has_later_sibling_at(depth, idx);
    let mut guides = Vec::with_capacity(depth.saturating_sub(1));
    for lvl in 1..depth {
        guides.push(has_later_sibling_at(lvl, idx));
    }
    (is_last, guides)
}

/// Render a 2-line Agent Screen leaf row in Activity (nested under its controlling
/// agent at `depth = 1..=4`, or standalone at `depth = 0`) with its context menu
/// and hover tooltip.
#[allow(clippy::too_many_arguments)]
fn render_activity_screen_row_with_menu(
    item: crate::activity_screens::ActivityScreenItem,
    selected: bool,
    menu_target: bool,
    you_control: bool,
    filter_active: bool,
    query_terms: &[String],
    width: f32,
    row_bounds_map: Rc<std::cell::RefCell<HashMap<String, Bounds<Pixels>>>>,
    win_id: WinId,
    cx: &mut olo_kit::Cx<RexWindow>,
) -> Element {
    const CHILD_INDENT: f32 = 15.0;
    const SUB_ROW_H: f32 = 38.0;

    let depth = item.depth.min(4);
    let indent = (depth as f32) * CHILD_INDENT;
    let left = ACTIVITY_FOLD_W + indent + ROW_ICON + m::TIGHT;
    let title = item.display_title.clone();
    // Opus #1: drop redundant "Running" word on active screen rows (the 6 pt dot carries it);
    // keep "Stopped" and "Disconnected" in tertiary grey.
    let state_word: Option<&'static str> = if item.host_disconnected {
        Some("Disconnected")
    } else if item.is_running() {
        None
    } else {
        Some("Stopped")
    };
    let state_a11y = state_word.unwrap_or("Running");
    let running = item.is_running();
    let disconnected = item.host_disconnected;
    let sub_parts = crate::activity_screens::screen_subtitle_segments(&item, you_control, None);
    let sub_room_est = (width - left - 22.0).max(48.0);
    let sub_line_est = crate::activity_screens::fit_screen_subtitle(&sub_parts, sub_room_est, style::SMALL_SIZE);

    let tip_key = format!("rex-scr-row-{}", item.id.key());
    let mut tip_parts = vec![title.clone()];
    if item.is_standalone() {
        tip_parts.push(olo_kit::words::tr("Standalone").to_owned());
    }
    if item.id.is_local() && item.section_host_key != item.run_host_key {
        tip_parts.push(olo_kit::words::tr("On this computer").to_owned());
    }
    if !sub_parts.is_empty() {
        tip_parts.push(sub_parts.join(" · "));
    }
    if let Err(why) = item.can_take_control_from_menu()
        && running
        && !disconnected
    {
        tip_parts.push(why);
    }
    let tip_text = tip_parts.join(" — ");
    olo_kit::tip::retext(cx, tip_key.clone(), tip_text.clone());

    let id_click = item.id.clone();
    let id_dbl = item.id.clone();
    let item_menu = item.clone();
    let menu_row_key = format!("scr:{}", item.id.key());
    let guides = item.guides.clone();
    let is_last = item.is_last;
    let terms_owned: Vec<String> = query_terms.to_vec();
    let agent_val = format!("Screen; {state_a11y}; {sub_line_est}");
    let key = style::key(((&item.id, &title, &sub_parts, running, disconnected, you_control), (filter_active, selected, menu_target, depth, is_last, &guides, &terms_owned)));

    let row_bounds_paint = row_bounds_map.clone();
    let row_bounds_key_paint = menu_row_key.clone();
    let row_bounds_menu = row_bounds_map;
    let row_bounds_key_menu = menu_row_key.clone();
    let open_cb = cx.msg(WinMsg::OpenScreen { id: id_click, tab: None, take_control: false, expected_host_instance_id: None, expected_started: None });
    let row_canvas = canvas(move |g, b| {
        row_bounds_paint.borrow_mut().insert(row_bounds_key_paint.clone(), b);
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let sel_rgba = style::selection_ground(&p, p.panel_background, false);
        let ground_rgba = if selected { sel_rgba } else { p.panel_background };
        let ground = style::c(ground_rgba);
        if selected {
            style::fill_rect(g, b, ground);
        }
        if menu_target {
            // Opus #11 / Neon-look #1: inset 2 pt vertically and 1 pt horizontally.
            let outline_r = style::snap(g, Bounds::from_corners(point(b.left() + px(1.0), b.top() + px(2.0)), point(b.right() - px(1.0), b.bottom() - px(2.0))));
            g.paint_quad(olo_kit::outline(outline_r, style::c(p.keyboard_navigation), olo_kit::BorderStyle::Solid));
        }
        let (ink, quiet, muted) = (style::c(p.panel_text), style::secondary_text(&p), style::disabled_text(&p));
        let baseline1 = style::baseline_at(g, b.top() + px(11.5), style::PLAIN_SIZE);
        let name_mid = baseline1 - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(b.left() + px(ACTIVITY_FOLD_W + indent + ROW_ICON / 2.0), name_mid), ROW_ICON, ROW_ICON);

        if depth >= 1 {
            let guide_c = if dark {
                let base_g = if selected { sel_rgba } else { p.panel_background };
                style::c(olo_theme::mix(base_g, p.panel_text, if selected { 0.48 } else { 0.34 }))
            } else {
                style::shade(&p, if selected { sel_rgba } else { p.panel_background }, 1.32)
            };
            for (lvl, &more) in guides.iter().take(depth.saturating_sub(1)).enumerate() {
                if more {
                    let gx = style::snap_coord(g, b.left() + px(ACTIVITY_FOLD_W + (lvl as f32) * CHILD_INDENT + ROW_ICON / 2.0));
                    style::vline(g, gx, b.top(), b.bottom(), guide_c);
                }
            }
            let parent_x = style::snap_coord(g, b.left() + px(ACTIVITY_FOLD_W + ((depth - 1) as f32) * CHILD_INDENT + ROW_ICON / 2.0));
            let guide_end = if is_last { name_mid } else { b.bottom() };
            style::vline(g, parent_x, b.top(), guide_end, guide_c);
            style::hline(g, parent_x, icon_b.left() - px(2.0), name_mid, guide_c);
        }

        crate::activity_screens::paint_neutral_screen_icon(g, icon_b, ground, running, disconnected, you_control && running && !disconnected);

        let right_edge = b.right() - px(8.0);
        let mut right = right_edge;
        if let Some(word) = state_word {
            let at = style::label_on_baseline(g, right_edge, baseline1, style::Align::Right, olo_kit::words::tr(word).to_string(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), muted);
            right = at.left();
        }
        let left_x = b.left() + px(left);
        // Opus #9: keep depth-0 standalone screen rows bold in EVERY filter state
        // (matching top-level root rows), while depth 1+ child screens stay regular.
        let title_weight = if depth == 0 { FontWeight::BOLD } else { FontWeight::NORMAL };
        // Neon #5: keep stopped screen titles crisp (matching Done agent rows, stronger than dimmed ancestors).
        let title_ink = if disconnected { quiet } else { ink };
        let title_gap = if right < right_edge { px(8.0) } else { px(0.0) };
        paint_query_label(g, left_x, b.top() + px(11.5), &title, style::PLAIN_SIZE, title_weight, title_ink, (right - title_gap - left_x).max(px(24.0)), &terms_owned, &p);

        if !sub_parts.is_empty() {
            let sub_max = (right_edge - left_x).max(px(24.0));
            let fitted = crate::activity_screens::fit_screen_subtitle(&sub_parts, f32::from(sub_max), style::SMALL_SIZE);
            if !fitted.is_empty() {
                if you_control && running && !disconnected {
                    // Opus Minor: crisp >=7.0:1 readable deep blue on light selection fill.
                    let ctrl_seed = if dark { [0x96, 0xcc, 0xff, 0xff] } else { [0x06, 0x36, 0x82, 0xff] };
                    let ctrl_ink = style::c(olo_theme::readable(ctrl_seed, ground_rgba, if dark { 5.5 } else { 7.0 }));
                    paint_query_label(g, left_x, b.top() + px(25.5), &fitted, style::SMALL_SIZE, FontWeight::NORMAL, ctrl_ink, sub_max, &terms_owned, &p);
                } else {
                    paint_query_label(g, left_x, b.top() + px(25.5), &fitted, style::SMALL_SIZE, FontWeight::NORMAL, quiet, sub_max, &terms_owned, &p);
                }
            }
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
    .size_full()
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |e, cx| {
        if e.click_count >= 2 {
            if let Err(why) = crate::activity_screens::open_in_agent_screens(&id_dbl) {
                Feedback::start(win_id).problem("Couldn’t open Agent Screens.", why);
            }
        } else {
            open_cb(&(), cx);
        }
        cx.stop_propagation();
    })
    .on_mouse_down(MouseButton::Right, move |e, cx| {
        let item = item_menu.clone();
        let can_ctrl = item.can_take_control_from_menu().is_ok();
        let can_stop = item.can_stop_from_menu().is_ok();
        let mut menu =
            Menu::new("").item("Show Screen", "scr.show").checked(selected).item("Take Control", "scr.control").checked(you_control).enabled(can_ctrl).item("Open in Agent Screens", "scr.open_app");
        if item.owner_bk.is_some() {
            menu = menu.item("Show Controlling Agent", "scr.show_owner");
        }
        menu = menu.separator();
        if item.is_running() {
            menu = menu.item("Stop Screen…", "scr.stop").enabled(can_stop);
        } else {
            menu = menu.item("Start Screen", "scr.start").enabled(!item.host_disconnected);
        }
        menu::resolve(std::slice::from_mut(&mut menu), true);
        if let Some(slot) = wins().map.get(&win_id) {
            slot.handle.messenger().send(WinMsg::SetContextMenuRow(Some(menu_row_key.clone())));
        }
        // Opus #11: anchor row context menu cleanly below the target row so its subtitle stays visible.
        let menu_at = row_bounds_menu.borrow().get(&row_bounds_key_menu).copied().map_or(e.position, |b| point(e.position.x.max(b.left() + px(12.0)), b.bottom() + px(2.0)));
        menu::context_menu_then(
            cx,
            menu,
            menu_at,
            move |cmd, _| match cmd {
                "scr.show" => {
                    if let Some(slot) = wins().map.get(&win_id) {
                        slot.handle.messenger().send(WinMsg::OpenScreen {
                            id: item.id.clone(),
                            tab: None,
                            take_control: false,
                            expected_host_instance_id: Some(item.host_instance_id),
                            expected_started: item.started,
                        });
                    }
                }
                "scr.control" if can_ctrl => {
                    if let Some(slot) = wins().map.get(&win_id) {
                        slot.handle.messenger().send(WinMsg::OpenScreen {
                            id: item.id.clone(),
                            tab: Some(olo_screen_pane::Tab::Live),
                            take_control: true,
                            expected_host_instance_id: Some(item.host_instance_id),
                            expected_started: item.started,
                        });
                    }
                }
                "scr.open_app" => {
                    if let Err(why) = crate::activity_screens::open_in_agent_screens(&item.id) {
                        Feedback::start(win_id).problem("Couldn’t open Agent Screens.", why);
                    }
                }
                "scr.stop" if can_stop => {
                    if let Some(slot) = wins().map.get(&win_id) {
                        slot.handle.messenger().send(WinMsg::ScreenStopConfirm(item.clone()));
                    }
                }
                "scr.start" if !item.is_running() && !item.host_disconnected => {
                    if let Some(slot) = wins().map.get(&win_id) {
                        slot.handle.messenger().send(WinMsg::ScreenStart(item.clone()));
                    }
                }
                "scr.show_owner" => {
                    if let Some(bk) = item.owner_bk.clone()
                        && let Some(slot) = wins().map.get(&win_id)
                    {
                        slot.handle.messenger().send(WinMsg::FocusBlock(bk));
                    }
                }
                _ => {}
            },
            move |_| {
                if let Some(slot) = wins().map.get(&win_id) {
                    slot.handle.messenger().send(WinMsg::SetContextMenuRow(None));
                }
            },
        );
        cx.stop_propagation();
    });

    olo_kit::tip::on_row(div().relative().w_full().h(px(SUB_ROW_H)).child(row_canvas), tip_key, tip_text)
}

fn screen_matches_filter(item: &crate::activity_screens::ActivityScreenItem, host_label: &str, owner_title: Option<&str>, terms: &[String], tf: &ActivityTypeFilter, sf: ActivityStateFilter) -> bool {
    if !matches!(tf, ActivityTypeFilter::All | ActivityTypeFilter::Screens) {
        return false;
    }
    let state_ok = match sf {
        ActivityStateFilter::All => true,
        ActivityStateFilter::Active => item.is_running() && !item.host_disconnected,
        ActivityStateFilter::Working | ActivityStateFilter::Waiting | ActivityStateFilter::Failed | ActivityStateFilter::Done => false,
    };
    if !state_ok {
        return false;
    }
    if terms.is_empty() {
        return true;
    }
    let place = if item.is_standalone() {
        "standalone"
    } else if item.id.is_local() {
        "this computer desktop local"
    } else {
        "host remote"
    };
    let state_w = if item.host_disconnected {
        "disconnected"
    } else if item.is_running() {
        "running"
    } else {
        "stopped"
    };
    let hay = format!("{} {} {} {} {} {} {} screen", item.display_title, item.id.name, item.run_host_label, host_label, owner_title.unwrap_or(""), place, state_w).to_ascii_lowercase();
    terms_match(&hay, terms)
}

fn native_child_matches_type(child: &NativeChildItem, tf: &ActivityTypeFilter) -> bool {
    use crate::subagents::AgentCli;
    match tf {
        ActivityTypeFilter::All => true,
        ActivityTypeFilter::Claude => child.row.entry.cli == AgentCli::Claude,
        ActivityTypeFilter::Codex => child.row.entry.cli == AgentCli::Codex,
        ActivityTypeFilter::Agy => child.row.entry.cli == AgentCli::Agy,
        // (A command of the asker's own is a program in a shell's place, not an agent.)
        ActivityTypeFilter::Shell => child.command,
        ActivityTypeFilter::Screens => false,
        ActivityTypeFilter::SubagentType(want) => child.row.entry.role.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(want)) || child.row.entry.title.eq_ignore_ascii_case(want),
    }
}

fn parent_matches_type(row: &ActivityRow, tf: &ActivityTypeFilter) -> bool {
    let parent_agent = row.parent_agent.as_deref();
    match tf {
        ActivityTypeFilter::All => true,
        ActivityTypeFilter::Claude => parent_agent.is_some_and(|a| a.eq_ignore_ascii_case("claude")) || row.title.to_ascii_lowercase().contains("claude"),
        ActivityTypeFilter::Codex => parent_agent.is_some_and(|a| a.eq_ignore_ascii_case("codex")) || row.title.to_ascii_lowercase().contains("codex"),
        ActivityTypeFilter::Agy => {
            parent_agent.is_some_and(|a| a.eq_ignore_ascii_case("agy")) || row.title.to_ascii_lowercase().contains("antigravity") || row.title.to_ascii_lowercase().contains("agy")
        }
        ActivityTypeFilter::Shell => !row.running && parent_agent.is_none(),
        ActivityTypeFilter::Screens | ActivityTypeFilter::SubagentType(_) => false,
    }
}

fn parent_matches_state(act: AgentActivity, sf: ActivityStateFilter) -> bool {
    match sf {
        ActivityStateFilter::All => true,
        ActivityStateFilter::Active => matches!(act, AgentActivity::Working | AgentActivity::Blocked),
        ActivityStateFilter::Working => act == AgentActivity::Working,
        ActivityStateFilter::Waiting => act == AgentActivity::Blocked,
        ActivityStateFilter::Failed => act == AgentActivity::Error,
        ActivityStateFilter::Done => act == AgentActivity::Done,
    }
}

fn terms_match(haystack_lower: &str, terms: &[String]) -> bool {
    terms.iter().all(|t| haystack_lower.contains(t.as_str()))
}

struct ActivityFilterMatch {
    screens: Vec<crate::activity_screens::ActivityScreenItem>,
    subs: Vec<NativeChildItem>,
    force_expand: bool,
    parent_matched: bool,
}

/// Filter a parent Activity row, its direct screens, and its native nested
/// subagents (and their screens) by keyword query, type filter, and live state
/// filter. Returns `Some(ActivityFilterMatch)` if the parent should appear
/// (either directly or as a muted ancestor opening the way to a matched child
/// or screen).
fn filter_activity_entry(row: &ActivityRow, query: &str, tf: &ActivityTypeFilter, sf: ActivityStateFilter) -> Option<ActivityFilterMatch> {
    let terms: Vec<String> = query.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
    let filter_active = !terms.is_empty() || *tf != ActivityTypeFilter::All || sf != ActivityStateFilter::All;
    if !filter_active {
        return Some(ActivityFilterMatch { screens: row.screens.clone(), subs: row.children.clone(), force_expand: false, parent_matched: true });
    }

    let parent_hay = format!("{} {} {} {}", row.host, row.title, row.detail, row.parent_agent.as_deref().unwrap_or("")).to_ascii_lowercase();
    let parent_ok = parent_matches_type(row, tf) && parent_matches_state(row.act, sf) && terms_match(&parent_hay, &terms);

    let matched_direct_screens: Vec<crate::activity_screens::ActivityScreenItem> =
        row.screens.iter().filter(|s| screen_matches_filter(s, &row.host, Some(&row.title), &terms, tf, sf)).cloned().collect();

    // Evaluate each subagent in preorder, preserving any ancestor subagent whose
    // own screen or descendant subagent matched (marked `ancestor_only = true`).
    let mut self_matched = vec![false; row.children.len()];
    let mut sub_screens: Vec<Vec<crate::activity_screens::ActivityScreenItem>> = Vec::with_capacity(row.children.len());
    for (i, c) in row.children.iter().enumerate() {
        let child_title = c.row.entry.display_title();
        let matched_sc: Vec<crate::activity_screens::ActivityScreenItem> = c.screens.iter().filter(|s| screen_matches_filter(s, &row.host, Some(&child_title), &terms, tf, sf)).cloned().collect();
        sub_screens.push(matched_sc);

        let state_ok = if c.command { matches!(sf, ActivityStateFilter::All | ActivityStateFilter::Active) } else { parent_matches_state(c.act, sf) };
        if native_child_matches_type(c, tf) && state_ok {
            if terms.is_empty() {
                self_matched[i] = true;
            } else {
                let state_w = match c.act {
                    _ if c.command => "running",
                    AgentActivity::Working => "working",
                    AgentActivity::Blocked => "waiting blocked",
                    AgentActivity::Done => "done",
                    AgentActivity::Error => "failed error",
                    AgentActivity::Idle => "idle",
                };
                let sub_hay = format!(
                    "{} {} {} {} {} {} {} {} {} {}",
                    child_title,
                    c.row.entry.role.as_deref().unwrap_or(""),
                    c.row.entry.cli.binary(),
                    c.row.entry.cli.display_name(),
                    c.row.entry.model,
                    c.row.entry.effort,
                    c.detail,
                    state_w,
                    row.host,
                    row.title
                )
                .to_ascii_lowercase();
                self_matched[i] = terms_match(&sub_hay, &terms);
            }
        }
    }

    // Astra #17 / Neon #8: propagate visibility upward through all ancestor depths
    // in preorder so multi-level trees (`Lead → Minder → Builder → screen`) retain
    // intermediate ancestors as dimmed context rows (`ancestor_only = true`).
    let mut keep = vec![false; row.children.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (i, c) in row.children.iter().enumerate() {
        let d = c.row.depth;
        while stack.last().is_some_and(|&(_, sd)| sd >= d) {
            stack.pop();
        }
        if self_matched[i] || !sub_screens[i].is_empty() {
            keep[i] = true;
            for &(anc_idx, _) in &stack {
                keep[anc_idx] = true;
            }
        }
        stack.push((i, d));
    }

    let mut matched_subs: Vec<NativeChildItem> = Vec::new();
    for (i, c) in row.children.iter().enumerate() {
        if keep[i] {
            let mut item = c.clone();
            item.screens = std::mem::take(&mut sub_screens[i]);
            item.ancestor_only = !self_matched[i];
            matched_subs.push(item);
        }
    }

    if !matched_direct_screens.is_empty() || !matched_subs.is_empty() {
        Some(ActivityFilterMatch { screens: matched_direct_screens, subs: matched_subs, force_expand: true, parent_matched: parent_ok })
    } else if parent_ok {
        Some(ActivityFilterMatch { screens: Vec::new(), subs: Vec::new(), force_expand: false, parent_matched: true })
    } else {
        None
    }
}

/// What an Activity row says its agent is doing, in a word, and the
/// colour of a state that wants the person (waiting, failed).
fn activity_state_word(act: AgentActivity, _running: bool) -> Option<(&'static str, Option<olo_theme::Rgba>)> {
    let p = theme::palette();
    match act {
        AgentActivity::Blocked => Some(("Waiting", Some(p.hue(olo_theme::Hue::Orange)))),
        AgentActivity::Error => Some(("Failed", Some(p.failure))),
        AgentActivity::Done => Some(("Done", None)),
        // Default active states ("Working" / "At rest") are carried by the icon mark
        // so sidebar names get the full row width without truncation (Opus #1).
        AgentActivity::Working | AgentActivity::Idle => None,
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
mod child_words_tests {
    use super::*;

    fn kid(name: &str, command: bool) -> RunningChild {
        RunningChild { window_id: format!("win-{name}"), name: name.to_owned(), command, minder: false }
    }

    /// Stopping a tab's children says what would be stopped: a command of
    /// the asker's own is not a subagent, and nothing is counted that
    /// isn't running.
    #[test]
    fn stopping_children_names_agents_and_commands_apart() {
        assert_eq!(stop_children_label(&[]), "Stop Subagents…");
        assert_eq!(stop_children_label(&[kid("a", false)]), "Stop Subagent…");
        assert_eq!(stop_children_label(&[kid("a", false), kid("b", false)]), "Stop 2 Subagents…");
        assert_eq!(stop_children_label(&[kid("log", true)]), "Stop Command…");
        assert_eq!(stop_children_label(&[kid("log", true), kid("srv", true)]), "Stop 2 Commands…");
        assert_eq!(stop_children_label(&[kid("a", false), kid("b", false), kid("log", true)]), "Stop 2 Subagents and 1 Command…");
        assert_eq!(stop_children_label(&[kid("a", false), kid("log", true), kid("srv", true)]), "Stop 1 Subagent and 2 Commands…");
    }

    /// A tab's children are stopped agents and commands first, their
    /// minders after them, whatever order the list has them in (a minder
    /// stands above its agents there).
    #[test]
    fn a_minder_is_stopped_after_what_it_minds() {
        let minder = RunningChild { minder: true, ..kid("minder", false) };
        let second = RunningChild { minder: true, ..kid("minder-2", false) };
        assert_eq!(stop_order(&[minder, kid("a", false), kid("b", false), second, kid("c", false), kid("log", true)]), ["win-a", "win-b", "win-c", "win-log", "win-minder", "win-minder-2"]);
        assert!(stop_order(&[]).is_empty());
    }

    /// A command's line keeps its end where it is cut: the folders of
    /// its paths go, never the file it is about.
    #[test]
    fn a_commands_paths_are_cut_to_their_names() {
        assert_eq!(paths_by_name("command · tail -f /var/tmp/olo/build.log", "…/"), "command · tail -f …/build.log");
        assert_eq!(paths_by_name("command · tail -f /var/tmp/olo/build.log", ""), "command · tail -f build.log");
        assert_eq!(paths_by_name("command · srv --log=/var/log/olo/srv.log --port 8080", "…/"), "command · srv --log=…/srv.log --port 8080");
        // (Nothing to gain, or not a path: left as typed.)
        assert_eq!(paths_by_name("command · ./run /tmp x/y", "…/"), "command · ./run /tmp x/y");
        assert_eq!(paths_by_name("command · curl https://example.org/a/b", "…/"), "command · curl https://example.org/a/b");
        assert_eq!(paths_by_name("command · ls /var/tmp/olo/", "…/"), "command · ls …/olo");
    }
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

    #[test]
    fn filters_activity_by_subagent_type_state_and_keywords() {
        use crate::subagents::{AgentCli, NativeSubagent, TreeRow};
        let mk_child = |id: &str, wid: &str, parent_wid: &str, depth: usize, cli: AgentCli, title: &str, role: &str, detail: &str, act: AgentActivity| NativeChildItem {
            row: TreeRow {
                depth,
                is_last: false,
                guides: vec![],
                entry: NativeSubagent {
                    id: id.into(),
                    name: Some(id.into()),
                    host_key: "local".into(),
                    session_id: "s1".into(),
                    window_id: wid.into(),
                    block_id: format!("b-{id}"),
                    parent_window_id: parent_wid.into(),
                    parent_block_id: None,
                    cli,
                    model: "test-model".into(),
                    effort: "high".into(),
                    title: title.into(),
                    role: Some(role.into()),
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
                    minds: None,
                    minded_by: None,
                    shared_minder: false,
                },
            },
            bk: BlockKey { host_key: "local".into(), session_id: "s1".into(), block_id: format!("b-{id}") },
            act,
            detail: detail.into(),
            running: true,
            command: cli == AgentCli::Command,
            help: crate::agent::Help::default(),
            has_children: false,
            screens: Vec::new(),
            ancestor_only: false,
        };
        let row = ActivityRow {
            bk: BlockKey { host_key: "local".into(), session_id: "s1".into(), block_id: "b1".into() },
            host_key: "local".into(),
            host: "This computer".into(),
            title: "Olo workspace".into(),
            detail: "olo".into(),
            act: AgentActivity::Working,
            help: crate::agent::Help::default(),
            running: true,
            parent_agent: Some("claude".into()),
            screens: Vec::new(),
            children: vec![
                mk_child("c1", "w2", "win-1", 1, AgentCli::Claude, "API Audit", "Auditor", "claude · Checking parser bugs", AgentActivity::Working),
                mk_child("a1", "w3", "w2", 2, AgentCli::Agy, "Schema Diff", "Reviewer", "agy · Visual QA of 2x shots", AgentActivity::Blocked),
                mk_child("x1", "w4", "win-1", 1, AgentCli::Codex, "Unit Tests", "Tester", "codex · Nightly benchmark", AgentActivity::Done),
            ],
        };

        // Unfiltered returns parent + all nested subagents without force-expanding.
        let all = filter_activity_entry(&row, "", &ActivityTypeFilter::All, ActivityStateFilter::All).unwrap();
        assert_eq!(all.subs.len(), 3);
        assert!(!all.force_expand);
        assert!(all.parent_matched);

        // Filter to a specific subagent role ("Auditor") keeps parent as ancestor-only and shows only c1.
        let exp = filter_activity_entry(&row, "", &ActivityTypeFilter::SubagentType("Auditor".into()), ActivityStateFilter::All).unwrap();
        assert_eq!(exp.subs.len(), 1);
        assert_eq!(exp.subs[0].row.entry.id, "c1");
        assert!(exp.force_expand);
        assert!(!exp.parent_matched);

        // Filter to harness ("agy") + keyword ("shots") matches grandchild a1 and retains
        // intermediate parent c1 as dimmed context ancestor (`ancestor_only = true`, Astra #17).
        let agy = filter_activity_entry(&row, "shots", &ActivityTypeFilter::Agy, ActivityStateFilter::All).unwrap();
        assert_eq!(agy.subs.iter().map(|s| (s.row.entry.id.as_str(), s.ancestor_only)).collect::<Vec<_>>(), vec![("c1", true), ("a1", false)]);
        assert!(!agy.parent_matched);

        // Filter by state (Waiting) keeps parent + intermediate parent c1 as ancestors because grandchild a1 is Blocked.
        let wait = filter_activity_entry(&row, "", &ActivityTypeFilter::All, ActivityStateFilter::Waiting).unwrap();
        assert_eq!(wait.subs.iter().map(|s| (s.row.entry.id.as_str(), s.ancestor_only)).collect::<Vec<_>>(), vec![("c1", true), ("a1", false)]);
        assert!(!wait.parent_matched);

        // Unmatched keyword hides the entire parent row.
        assert!(filter_activity_entry(&row, "nonexistent", &ActivityTypeFilter::All, ActivityStateFilter::All).is_none());

        // A command of the asker's own is a program, not an agent: among
        // the shells by type, active while it runs and in none of an
        // agent's states, and found by "running" and by what it runs.
        let mut with_command = row;
        with_command.children.push(mk_child("k1", "w5", "win-1", 1, AgentCli::Command, "Build Log", "", "command · tail -f /var/tmp/olo/build.log", AgentActivity::Idle));
        let ids = |tf: ActivityTypeFilter, sf: ActivityStateFilter, q: &str| -> Vec<String> {
            filter_activity_entry(&with_command, q, &tf, sf).map(|m| m.subs.iter().map(|c| c.row.entry.id.clone()).collect()).unwrap_or_default()
        };
        assert_eq!(ids(ActivityTypeFilter::Shell, ActivityStateFilter::All, ""), ["k1"]);
        assert_eq!(ids(ActivityTypeFilter::All, ActivityStateFilter::Active, ""), ["c1", "a1", "k1"]);
        assert_eq!(ids(ActivityTypeFilter::All, ActivityStateFilter::Working, ""), ["c1"]);
        assert!(ids(ActivityTypeFilter::All, ActivityStateFilter::Done, "").iter().all(|id| id != "k1"));
        assert_eq!(ids(ActivityTypeFilter::All, ActivityStateFilter::All, "running build.log"), ["k1"]);
    }

    #[test]
    fn screen_filter_matches_type_state_title_host_and_place() {
        let mk_scr = |id: olo_screen_pane::ScreenId, title: &str, state: olo_screen_pane::ScreenState, disconnected: bool, owner: Option<BlockKey>| crate::activity_screens::ActivityScreenItem {
            id,
            run_host_key: "dev-box".into(),
            run_host_label: "dev-box".into(),
            host_instance_id: 1,
            conn_id: 1,
            section_host_key: "dev-box".into(),
            display_title: title.into(),
            title: Some(title.into()),
            state,
            size: (state == olo_screen_pane::ScreenState::Running).then_some((1440, 900)),
            scale: 1.0,
            sound: false,
            pid: Some(1234),
            started: Some(1_700_000_000),
            last_used_ms: Some(1_700_000_000_000),
            actions: Some(3),
            controlled: false,
            host_disconnected: disconnected,
            disconnected_at_ms: None,
            capabilities: olo_screen_pane::PaneCapabilities::remote_default(),
            owner_bk: owner,
            depth: 1,
            is_last: true,
            guides: Vec::new(),
        };
        let bk = BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: "b1".into() };
        let running = mk_scr(olo_screen_pane::ScreenId::host("dev-box", "dev-box", "verify-1"), "Verify Build", olo_screen_pane::ScreenState::Running, false, Some(bk.clone()));
        let stopped = mk_scr(olo_screen_pane::ScreenId::host("dev-box", "dev-box", "smoke"), "Smoke Pass", olo_screen_pane::ScreenState::Stopped, false, Some(bk.clone()));
        let disc = mk_scr(olo_screen_pane::ScreenId::host("dev-box", "dev-box", "ghost"), "Ghost", olo_screen_pane::ScreenState::Running, true, Some(bk));
        let local_desk = mk_scr(olo_screen_pane::ScreenId::local("browser"), "Docs Browser", olo_screen_pane::ScreenState::Running, false, None);

        assert!(screen_matches_filter(&running, "dev-box", Some("builder-1"), &[], &ActivityTypeFilter::Screens, ActivityStateFilter::All));
        assert!(screen_matches_filter(&running, "dev-box", Some("builder-1"), &[], &ActivityTypeFilter::Screens, ActivityStateFilter::Active));
        assert!(!screen_matches_filter(&running, "dev-box", Some("builder-1"), &[], &ActivityTypeFilter::Claude, ActivityStateFilter::All));
        assert!(!screen_matches_filter(&stopped, "dev-box", Some("builder-1"), &[], &ActivityTypeFilter::Screens, ActivityStateFilter::Active));
        assert!(!screen_matches_filter(&disc, "dev-box", Some("builder-1"), &[], &ActivityTypeFilter::Screens, ActivityStateFilter::Active));
        assert!(screen_matches_filter(&running, "dev-box", Some("builder-1"), &["verify".into()], &ActivityTypeFilter::All, ActivityStateFilter::All));
        assert!(screen_matches_filter(&running, "dev-box", Some("builder-1"), &["dev-box".into()], &ActivityTypeFilter::All, ActivityStateFilter::All));
        assert!(screen_matches_filter(&local_desk, "This computer", None, &["standalone".into()], &ActivityTypeFilter::All, ActivityStateFilter::All));
    }

    #[test]
    fn astra_f17_three_level_filter_preserves_intermediate_ancestors() {
        use crate::subagents::{AgentCli, DesktopPolicy, NativeSubagent, TreeRow};
        let mk_sub = |id: &str, wid: &str, pwid: &str, depth: usize, title: &str, screens: Vec<crate::activity_screens::ActivityScreenItem>| NativeChildItem {
            row: TreeRow {
                entry: NativeSubagent {
                    id: id.into(),
                    name: Some(title.to_lowercase()),
                    host_key: "dev-box".into(),
                    session_id: "s1".into(),
                    window_id: wid.into(),
                    block_id: format!("b-{id}"),
                    parent_window_id: pwid.into(),
                    parent_block_id: None,
                    cli: AgentCli::Codex,
                    model: "gpt-5.4-codex".into(),
                    effort: "high".into(),
                    title: title.into(),
                    role: None,
                    cwd: None,
                    resume_id: None,
                    cli_session_id: None,
                    transcript_path: None,
                    desktop_policy: DesktopPolicy::None,
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
                },
                depth,
                is_last: true,
                guides: Vec::new(),
            },
            bk: BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: format!("b-{id}") },
            act: AgentActivity::Working,
            detail: "codex · working".into(),
            running: true,
            command: false,
            help: Default::default(),
            has_children: false,
            screens,
            ancestor_only: false,
        };
        let deep_screen = crate::activity_screens::ActivityScreenItem {
            id: olo_screen_pane::ScreenId::host("dev-box", "dev-box", "verify-1"),
            run_host_key: "dev-box".into(),
            run_host_label: "dev-box".into(),
            host_instance_id: 1,
            conn_id: 1,
            section_host_key: "dev-box".into(),
            display_title: "Verify Build".into(),
            title: Some("Verify Build".into()),
            state: olo_screen_pane::ScreenState::Running,
            size: Some((1440, 900)),
            scale: 1.0,
            sound: false,
            pid: Some(1234),
            started: Some(1_700_000_000),
            last_used_ms: Some(1_700_000_000_000),
            actions: Some(3),
            controlled: false,
            host_disconnected: false,
            disconnected_at_ms: None,
            capabilities: olo_screen_pane::PaneCapabilities::remote_default(),
            owner_bk: Some(BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: "b-builder".into() }),
            depth: 3,
            is_last: true,
            guides: Vec::new(),
        };
        let row = ActivityRow {
            bk: BlockKey { host_key: "dev-box".into(), session_id: "s1".into(), block_id: "b-lead".into() },
            host_key: "dev-box".into(),
            host: "dev-box".into(),
            title: "Lead".into(),
            detail: "claude · coordinating".into(),
            act: AgentActivity::Working,
            help: Default::default(),
            running: true,
            parent_agent: Some("claude".into()),
            screens: Vec::new(),
            children: vec![
                mk_sub("minder", "w-minder", "w-lead", 1, "Minder", Vec::new()),
                mk_sub("builder", "w-builder", "w-minder", 2, "Builder", vec![deep_screen]),
                mk_sub("unrelated", "w-other", "w-lead", 1, "Unrelated", Vec::new()),
            ],
        };

        // Under Type = Screens, Lead (ancestor), Minder (depth 1 ancestor), and Builder (depth 2 ancestor)
        // must all be retained so Verify Build stays attached in its true 3-level tree!
        let by_type = filter_activity_entry(&row, "", &ActivityTypeFilter::Screens, ActivityStateFilter::All).expect("match");
        assert!(!by_type.parent_matched);
        let ids: Vec<(&str, bool, usize)> = by_type.subs.iter().map(|c| (c.row.entry.id.as_str(), c.ancestor_only, c.screens.len())).collect();
        assert_eq!(ids, vec![("minder", true, 0), ("builder", true, 1)]);

        // Same for keyword search matching only the grandchild's screen.
        let by_query = filter_activity_entry(&row, "verify", &ActivityTypeFilter::All, ActivityStateFilter::All).expect("match");
        let q_ids: Vec<(&str, bool, usize)> = by_query.subs.iter().map(|c| (c.row.entry.id.as_str(), c.ancestor_only, c.screens.len())).collect();
        assert_eq!(q_ids, vec![("minder", true, 0), ("builder", true, 1)]);
    }

    #[test]
    fn astra_f11_retained_screen_shortcuts_restrict_window_mutations() {
        assert!(RexWindow::is_retained_screen_shortcut("quit"));
        assert!(RexWindow::is_retained_screen_shortcut("close"));
        assert!(RexWindow::is_retained_screen_shortcut("app.quit"));
        assert!(RexWindow::is_retained_screen_shortcut("app.toggle_sidebar"));
        assert!(RexWindow::is_retained_screen_shortcut("window.next"));
        assert!(RexWindow::is_retained_screen_shortcut("window.previous"));
        assert!(RexWindow::is_retained_screen_shortcut("window.select_1"));
        assert!(RexWindow::is_retained_screen_shortcut("window.select_9"));
        assert!(RexWindow::is_retained_screen_shortcut("agent.jump_next_blocked"));

        // Destructive or terminal-specific window/pane shortcuts must NOT fire while viewing a screen.
        assert!(!RexWindow::is_retained_screen_shortcut("window.rename"));
        assert!(!RexWindow::is_retained_screen_shortcut("window.move_left"));
        assert!(!RexWindow::is_retained_screen_shortcut("window.move_right"));
        assert!(!RexWindow::is_retained_screen_shortcut("terminal.paste"));
        assert!(!RexWindow::is_retained_screen_shortcut("pane.split_right"));
    }

    #[test]
    fn astra_f25_split_pane_child_owner_remaps_to_primary_block() {
        use crate::subagents::{AgentCli, DesktopPolicy, NativeSubagent};
        use olo_rex_client::{BlockInfo, LayerView, SessionView, WindowView};
        let mut st = crate::hub::HubState::test_empty();
        let sess = SessionView {
            revision: 1,
            session_id: "s1".into(),
            label: "work".into(),
            active_window_id: Some("w-lead".into()),
            windows: vec![
                WindowView {
                    window_id: "w-lead".into(),
                    label: "Lead".into(),
                    focused_block_id: Some("b-lead".into()),
                    zoomed_block_id: None,
                    layers: vec![LayerView {
                        layer_id: "l1".into(),
                        kind: "tiled".into(),
                        bounds: [0.0, 0.0, 1.0, 1.0],
                        state: None,
                        layout: None,
                        blocks: vec![BlockInfo { block_id: "b-lead".into(), label: "Lead".into(), creator_name: "".into(), flavor: "".into(), host_id: "dev".into(), owner: None }],
                    }],
                },
                WindowView {
                    window_id: "w-child".into(),
                    label: "Builder".into(),
                    focused_block_id: Some("b-child-primary".into()),
                    zoomed_block_id: None,
                    layers: vec![LayerView {
                        layer_id: "l2".into(),
                        kind: "tiled".into(),
                        bounds: [0.0, 0.0, 1.0, 1.0],
                        state: None,
                        layout: None,
                        blocks: vec![
                            BlockInfo { block_id: "b-child-primary".into(), label: "Builder".into(), creator_name: "".into(), flavor: "".into(), host_id: "dev".into(), owner: None },
                            BlockInfo { block_id: "b-child-split".into(), label: "Split".into(), creator_name: "".into(), flavor: "".into(), host_id: "dev".into(), owner: None },
                        ],
                    }],
                },
            ],
        };
        let host = crate::hub::HostRuntime::demo_connected("dev", "Dev", "ssh://dev", vec![sess]);
        st.hosts.insert("dev".into(), host);
        st.host_order.push("dev".into());
        st.subagents.register(NativeSubagent {
            id: "sa-1".into(),
            name: Some("builder".into()),
            host_key: "dev".into(),
            session_id: "s1".into(),
            window_id: "w-child".into(),
            block_id: "b-child-primary".into(),
            parent_window_id: "w-lead".into(),
            parent_block_id: Some("b-lead".into()),
            cli: AgentCli::Codex,
            model: "gpt-5.4-codex".into(),
            effort: "high".into(),
            title: "Builder".into(),
            role: None,
            cwd: None,
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: DesktopPolicy::None,
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
        });
        st.host_screens.insert(
            "dev".into(),
            crate::screens_sock::LiveHostScreens {
                host_instance_id: 1,
                conn_id: 1,
                rev: 1,
                conn_state: olo_screen_pane::rex_sock::RexHostConn::Connected,
                has_follow_snapshot: true,
                proto: 1,
                commands: vec!["follow".into(), "stream".into()],
                capabilities: vec!["control-ack".into(), "stop-expected".into()],
                screens: vec![olo_screen_pane::rex_sock::RexScreen {
                    name: "split-qa".into(),
                    title: Some("Split QA".into()),
                    state: olo_screen_pane::rex_sock::HostScreenState::Running,
                    width: 1440,
                    height: 900,
                    scale: 1.0,
                    sound: false,
                    pid: Some(100),
                    started: Some(1_700_000_000),
                    last_used: Some(1_700_000_000_000),
                    actions: Some(1),
                    controlled: false,
                    owner: Some(olo_screen_pane::rex_sock::OwnerPane { host_key: Some("dev".into()), session_id: "s1".into(), block_id: "b-child-split".into() }),
                    started_by: None,
                    controlled_by: None,
                }],
                ..Default::default()
            },
        );
        let res = RexWindow::resolve_screens_from_hub(&st);
        let primary_bk = BlockKey { host_key: "dev".into(), session_id: "s1".into(), block_id: "b-child-primary".into() };
        let owned = res.by_owner.get(&primary_bk).expect("split pane screen must attach to rendered child primary block");
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].id.name, "split-qa");
        assert!(res.standalone_by_host.get("dev").is_none_or(|v| v.is_empty()));
    }

    #[test]
    fn saved_workspace_state_round_trips_active_screen_and_tab_without_control() {
        let state = SavedWorkspaceState {
            active_host: "dev-box".into(),
            active_session: Some("dev-main".into()),
            session_index: 0,
            sidebar_collapsed: false,
            sidebar_width: 240.0,
            active_screen: Some(olo_screen_pane::ScreenId::host("dev-box", "dev-box", "verify-1")),
            active_screen_tab: Some(olo_screen_pane::Tab::History),
        };
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(!json.contains("controlling"), "Take Control must never be persisted across reload");
        let back: SavedWorkspaceState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.active_screen, state.active_screen);
        assert_eq!(back.active_screen_tab, Some(olo_screen_pane::Tab::History));
    }

    #[test]
    fn r3_regression_guards_strict_stop_raw_control_shortcut_host_error_and_topology_rev() {
        // 1. Astra #6 / Neon #2: unreadable/unknown local history MUST fail closed,
        //    never converting unknown into `expected_last_used = None`!
        let err_unknown = olo_screen_pane::capture_stop_expectations_strict(true, Some(1_700_000_000), None, None, None, false).unwrap_err();
        assert!(err_unknown.contains("activity") || err_unknown.contains("history") || err_unknown.contains("verify"), "expected fail-closed error, got: {err_unknown}");
        // Verified empty local history (`actions == Some(0)`) succeeds with `Some(0)`.
        let ok_empty = olo_screen_pane::capture_stop_expectations_strict(true, Some(1_700_000_000), None, None, Some(0), true).unwrap();
        assert_eq!(ok_empty, (Some(1_700_000_000), Some(0)));

        // 2. Neon #1: raw-mode `screen.control` (`⌘K` / `Alt+K` in terminal raw mode)
        //    matches with Shift added on Mac/BeOS profiles and NEVER swallows plain Ctrl+K!
        let p = olo_kit::keys::profile();
        let raw_list = olo_screen_pane::control_shortcuts(true);
        assert!(!raw_list.is_empty(), "screen.control must resolve in raw mode");
        if matches!(p.command_key, olo_protocol::CommandKey::Command | olo_protocol::CommandKey::Alt) {
            let plain_ctrl_k = olo_kit::Keystroke { key: "k".into(), key_char: None, modifiers: olo_kit::Modifiers { control: true, shift: false, alt: false, platform: false, function: false } };
            assert!(!olo_screen_pane::is_control_shortcut(&plain_ctrl_k, true), "plain Ctrl+K must pass through to controlled screens in raw mode");
            let shifted_cmd_k = olo_kit::Keystroke { key: "k".into(), key_char: None, modifiers: olo_kit::Modifiers { control: true, shift: true, alt: false, platform: false, function: false } };
            assert!(olo_screen_pane::is_control_shortcut(&shifted_cmd_k, true), "raw terminal Cmd+K / Alt+K (Ctrl+Shift+K) must match screen.control");
        }

        // 3. Astra #25: `HubState::bump_topology` advances `topology_rev` in O(1).
        let mut st = crate::hub::HubState::test_empty();
        let r0 = st.topology_rev;
        st.bump_topology();
        assert_eq!(st.topology_rev, r0.wrapping_add(1));

        // 4. Neon #12: `host::screen_failed` writes directly to the matching Host window
        //    and ignores stale `host_instance_id`.
        let _guard = crate::host::TEST_LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::host::open("dev-box", "Dev Box", 42);
        crate::host::screen_failed("dev-box", 99, "qa", "Couldn’t open qa", "stale instance");
        assert!(crate::host::current().unwrap().screen_failed.is_none(), "stale host_instance_id must not overwrite current Host window error");
        crate::host::screen_failed("dev-box", 42, "qa", "Couldn’t open qa", "Update Olo’s helper");
        let sf = crate::host::current().unwrap().screen_failed.expect("matching host_instance_id must record failure on Host window");
        assert_eq!(sf.0, "qa");
        assert!(sf.2.contains("Update Olo"));
        crate::host::close();
    }
}

/// Directories that exist identically on every Unix system and should never be
/// rewritten to `rex://local/…` when pasted bare.
fn is_bare_system_dir(p: &std::path::Path) -> bool {
    matches!(
        p.to_str(),
        Some(
            "/" | "/tmp" | "/var" | "/var/tmp" | "/etc" | "/usr" | "/bin" | "/sbin" | "/lib" | "/lib64" | "/opt" | "/dev" | "/proc" | "/sys" | "/home" | "/root" | "/run" | "/boot" | "/mnt" | "/media"
        )
    )
}

fn parse_candidate_local_path(token: &str, home: Option<&std::path::Path>) -> Option<(PathBuf, bool)> {
    let raw = token.trim();
    if raw.is_empty() || raw.starts_with(olo_kit_base::tracker::REX_SCHEME) {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("file://") {
        let path_part = rest.strip_prefix("localhost").unwrap_or(rest);
        if path_part.starts_with('/') {
            let p = olo_kit_base::tracker::RexPath::parse(&format!("rex://local{path_part}"))?.local_path()?;
            return Some((p, true));
        }
        return None;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        let h = home?;
        return Some((h.join(rest), true));
    }
    if raw.starts_with('/') {
        let p = PathBuf::from(raw);
        let under_home = home.is_some_and(|h| p.starts_with(h) && p != h);
        return Some((p, under_home));
    }
    None
}

/// Rewrite local desktop file/folder paths pasted into a remote Rex terminal
/// into canonical `rex://local/…` URIs so remote agents know they live on the
/// desktop and can fetch them with `olo get` (or open them with `olo open`).
///
/// Rewrites:
/// - Any `file:///…` URI pointing to an existing local path.
/// - Any path under the desktop's `$HOME/` (or `~/…`) that exists locally,
///   even inside a sentence (`look at /home/u/Pictures/a.png`).
/// - Any standalone pasted path(s) (one path per line or space-separated list
///   where every token is an existing local path outside bare system roots like
///   `/var/tmp` or `/etc`), including paths with spaces copied from Tracker.
/// Leaves shell commands like `cat /etc/hosts` or `ls /var/tmp` untouched.
pub(crate) fn rewrite_local_paths_for_remote(text: &str, home: Option<&std::path::Path>, exists: impl Fn(&std::path::Path) -> bool) -> String {
    if text.trim().is_empty() {
        return text.to_owned();
    }
    // Pass 1: if every non-empty line is itself a standalone existing local path
    // (covers Tracker Cmd+C copying one or more paths, including spaces in names!).
    let lines: Vec<&str> = text.split('\n').collect();
    let mut non_empty_lines = 0usize;
    let mut all_lines_paths = true;
    let mut line_rewrites: Vec<Option<String>> = Vec::with_capacity(lines.len());
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line_rewrites.push(None);
            continue;
        }
        non_empty_lines += 1;
        let unquoted = ['"', '\''].iter().find_map(|q| trimmed.strip_prefix(*q).and_then(|s| s.strip_suffix(*q))).unwrap_or(trimmed);
        if let Some((cand, _under_home)) = parse_candidate_local_path(unquoted, home)
            && !is_bare_system_dir(&cand)
            && exists(&cand)
        {
            line_rewrites.push(Some(olo_kit_base::tracker::RexPath::local(&cand).to_uri()));
        } else {
            all_lines_paths = false;
            break;
        }
    }
    if all_lines_paths && non_empty_lines > 0 {
        let mut out = String::with_capacity(text.len() + non_empty_lines * 12);
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if let Some(uri) = &line_rewrites[i] {
                out.push_str(uri);
            } else {
                out.push_str(line);
            }
        }
        return out;
    }

    // Pass 2: tokenize preserving exact whitespace. Check whether ALL tokens
    // are standalone existing local paths (e.g. "/var/tmp/a.png /var/tmp/b.png"),
    // else rewrite only `file://` URIs and paths under `$HOME/` that exist locally.
    let mut tokens: Vec<(&str, bool)> = Vec::new(); // (slice, is_ws)
    let mut start = 0usize;
    let mut in_ws = text.chars().next().is_some_and(char::is_whitespace);
    for (idx, ch) in text.char_indices() {
        let ws = ch.is_whitespace();
        if ws != in_ws {
            tokens.push((&text[start..idx], in_ws));
            start = idx;
            in_ws = ws;
        }
    }
    tokens.push((&text[start..], in_ws));

    let resolve_tok = |tok: &str| -> Option<(String, bool)> {
        let (lead_q, inner, trail_q) = if let Some(s) = tok.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            ("\"", s, "\"")
        } else if let Some(s) = tok.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            ("'", s, "'")
        } else {
            ("", tok, "")
        };
        let (core, trail_punct) = if let Some((cand, explicit)) = parse_candidate_local_path(inner, home)
            && exists(&cand)
        {
            return (!is_bare_system_dir(&cand)).then(|| (format!("{lead_q}{}{trail_q}", olo_kit_base::tracker::RexPath::local(&cand).to_uri()), explicit));
        } else {
            let trimmed = inner.trim_end_matches([',', ';', ')', ']', '.']);
            (trimmed, &inner[trimmed.len()..])
        };
        if core.len() < inner.len()
            && let Some((cand, explicit)) = parse_candidate_local_path(core, home)
            && !is_bare_system_dir(&cand)
            && exists(&cand)
        {
            return Some((format!("{lead_q}{}{trail_punct}{trail_q}", olo_kit_base::tracker::RexPath::local(&cand).to_uri()), explicit));
        }
        None
    };

    let mut non_ws_count = 0usize;
    let mut all_tokens_local_paths = true;
    for &(slice, ws) in &tokens {
        if ws || slice.is_empty() {
            continue;
        }
        non_ws_count += 1;
        if resolve_tok(slice).is_none() {
            all_tokens_local_paths = false;
            break;
        }
    }
    let rewrite_any_abs = all_tokens_local_paths && non_ws_count > 0;

    let mut out = String::with_capacity(text.len() + 32);
    for (slice, ws) in tokens {
        if ws || slice.is_empty() {
            out.push_str(slice);
            continue;
        }
        if let Some((rewritten, explicit)) = resolve_tok(slice)
            && (rewrite_any_abs || explicit)
        {
            out.push_str(&rewritten);
        } else {
            out.push_str(slice);
        }
    }
    out
}

/// Request a new tab right of the active tab in the active pane's working
/// directory, closing the initiating transcript viewer ONLY once creation
/// succeeds and atomically only if the user is still viewing that exact
/// transcript.
fn request_new_tab(hub: &Arc<RexHub>, window: WinId, host_key: &str, session_id: &str) {
    let started = crate::activity_watch::active_transcript();
    let on_created: Option<Box<dyn FnOnce() + Send>> = started.map(|tr| {
        Box::new(move || {
            let _ = crate::activity_watch::close_viewer_if(&tr);
        }) as Box<dyn FnOnce() + Send>
    });
    hub.create_window(host_key, session_id, None, Some(Feedback::start(window)), on_created);
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

    #[test]
    fn rewrites_local_desktop_paths_to_rex_local_uris_for_remote_panes() {
        let home = std::path::Path::new("/home/alice");
        let existing = |p: &std::path::Path| {
            matches!(p.to_str(), Some("/home/alice/Pictures/shot1.png" | "/home/alice/Pictures/shot2.png" | "/home/alice/My Photos/big shot.png" | "/var/tmp/my-shot.png" | "/etc/hosts" | "/var/tmp"))
        };

        // Home paths inside a prompt convert cleanly, preserving prose.
        assert_eq!(
            rewrite_local_paths_for_remote("/home/alice/Pictures/shot1.png /home/alice/Pictures/shot2.png look at these", Some(home), existing),
            "rex://local/home/alice/Pictures/shot1.png rex://local/home/alice/Pictures/shot2.png look at these"
        );
        // Single path with spaces (copied from Tracker) percent-encodes cleanly.
        assert_eq!(rewrite_local_paths_for_remote("/home/alice/My Photos/big shot.png", Some(home), existing), "rex://local/home/alice/My%20Photos/big%20shot.png");
        // Standalone file outside $HOME converts cleanly.
        assert_eq!(rewrite_local_paths_for_remote("/var/tmp/my-shot.png", Some(home), existing), "rex://local/var/tmp/my-shot.png");
        // file:// URI converts cleanly.
        assert_eq!(rewrite_local_paths_for_remote("see file:///home/alice/My%20Photos/big%20shot.png.", Some(home), existing), "see rex://local/home/alice/My%20Photos/big%20shot.png.");
        // Shell commands referencing system paths stay untouched!
        assert_eq!(rewrite_local_paths_for_remote("cat /etc/hosts", Some(home), existing), "cat /etc/hosts");
        assert_eq!(rewrite_local_paths_for_remote("ls /var/tmp", Some(home), existing), "ls /var/tmp");
        assert_eq!(rewrite_local_paths_for_remote("/var/tmp", Some(home), existing), "/var/tmp");
        // Existing rex:// URI stays untouched.
        assert_eq!(rewrite_local_paths_for_remote("rex://local/home/alice/Pictures/shot1.png", Some(home), existing), "rex://local/home/alice/Pictures/shot1.png");
    }

    #[test]
    fn r3_polish_opus_f19_dangling_ellipsis_prefix_cleaned() {
        assert_eq!(trim_dangling_ellipsis_prefix("Billing Checkout Flow &"), "Billing Checkout Flow");
        assert_eq!(trim_dangling_ellipsis_prefix("Billing Checkout Flow & "), "Billing Checkout Flow");
        assert_eq!(trim_dangling_ellipsis_prefix("Verify Build · "), "Verify Build");
        assert_eq!(trim_dangling_ellipsis_prefix("Step 7 — "), "Step 7 —");
        assert_eq!(trim_dangling_ellipsis_prefix("Step 7 - "), "Step 7");
        assert_eq!(trim_dangling_ellipsis_prefix("Plain Title"), "Plain Title");
    }
}
