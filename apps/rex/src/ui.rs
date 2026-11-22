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
    MouseMoveEvent, Painter, ParentElement as _, PathBuilder, Pixels, Point, ScrollWheelEvent, Styled as _, TextInput, View, WindowCx, WindowHandle, WindowOptions, agent, canvas, div, icons,
    menu::{self, Menu, MenuBar},
    metrics as m, point, px, size, style, theme,
    widgets::{
        self, Banner, Button, Front, Panes as SidePanes, SidePane, StatusBar, StringView, StripAction, StripTab, TabStrip, TextControl, TextField, TextFieldEvent, Tone, find_bar, find_said,
        side_divider,
    },
};
use olo_rex_client::{LayerKind, LayerState, LayoutNode, WindowView};
use olo_rex_vt::{FormatKind, TerminalTheme, encode_paste, paste_is_safe};
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    agent::{AgentActivity, KeepGoingState},
    attention::{Attention, Target},
    bridge,
    copy_mode::{CopyMode, CopyModeAction},
    hub::{BlockKey, HostConnState, HubNotice, RexHub},
    keymap::{self, KeyMatch, KeymapEngine},
    links::{self, LinkTarget},
    overlays::{self, ActiveOverlay, JumpTarget, PathBrowserMode, QuickLookContent},
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
}

/// Global Rex application controller (shared safely across windows via `olo_sync::Mutex`).
pub struct RexApp {
    pub hub: Arc<RexHub>,
    pub askpass: Arc<OloAskpass>,
    pub keymap: KeymapEngine,
    /// How the SSH agent stands (asked once as Rex starts), for Diagnostics.
    pub ssh_agent: olo_rex_client::AgentStatus,
    attention: Attention,
    broadcast_windows: HashSet<(String, String, String)>,
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
    let hub = RexHub::start(settings.clone(), active_theme, Some(askpass.clone()), move |notice| handle_hub_notice(notice));
    let keymap = KeymapEngine::new(&[], &[], &[]);
    let _ = APP.set(Mutex::new(RexApp { hub, askpass, keymap, ssh_agent: Default::default(), attention: Attention::new(), broadcast_windows: HashSet::new() }));

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
    }
}

fn broadcast_msg(msg: WinMsg) {
    let handles: Vec<WindowHandle<RexWindow>> = wins().map.values().map(|s| s.handle.clone()).collect();
    for h in handles {
        h.messenger().send(msg.clone());
    }
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

    pub fn preview(self, path: String, preview: QuickLookContent) {
        self.send(WinMsg::QuickLookReady(self.serial, path, preview));
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
            let mut o = WindowOptions { fixed: true, ..WindowOptions::new("Hosts", 580.0, 120.0).dialog() };
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
        WinRole::Workspace => {
            open_workspace_window(None);
            return;
        }
    };
    let handle = olo_kit::open_window(opts, move |cx| RexWindow::new_panel(id, role, cx));
    wins().map.insert(id, WinSlot { role, handle, selection: Default::default() });
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
    Raise,
    ShowPrompt { host_key: String, req: olo_rex_client::ClientPromptRequested },
    EndPrompt { host_key: String, call_id: String },
    PerformAction { host: String, session: Option<String>, block: Option<String>, action: String, args: Option<Value> },
    QuickLookReady(u64, String, QuickLookContent),
    QrReady(String, Option<Arc<qrcode::QrCode>>),
    OpenPreview(BlockKey, String),
    DirListingReady { key: BlockKey, listing: olo_rex_client::DirListing },
    SetClipboard(String),
    PasteText(BlockKey, olo_rex_client::ControlClient, String),
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
    HostsAdd,
    HostsManualForward,
    HostsForwardFrom(String),
    AskpassConfirm(bool),
    AskpassSubmit,
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
        let side_pane = SidePane::new(side_w, 160.0).first(Front::Rest).restored(Some(side_w), side_col);

        let focus = FocusHandle::new();
        focus.focus(cx);

        let overlay_field = widgets::text_field(cx, |f| f.hint("Type to filter…"));
        cx.subscribe(&overlay_field, |w: &mut Self, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                let txt = w.overlay_field.as_ref().map(|f| f.read(cx).text().to_owned()).unwrap_or_default();
                if let Some(ov) = &mut w.overlay {
                    match ov {
                        ActiveOverlay::RexPrompt { query, selected_idx, .. } => {
                            *query = txt;
                            *selected_idx = 0;
                        }
                        ActiveOverlay::PathBrowser { query, selected, .. } => {
                            *query = txt;
                            *selected = 0;
                        }
                        ActiveOverlay::QuickLook { .. } => {}
                    }
                }
                cx.invalidate();
            }
            TextFieldEvent::Entered => {
                w.confirm_overlay(cx);
            }
            TextFieldEvent::Typed(_) => {}
        });

        let find_field = widgets::text_field(cx, |f| f.hint("Find in terminal…"));
        cx.subscribe(&find_field, |w: &mut Self, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => w.run_find_query(cx),
            TextFieldEvent::Entered => w.step_find(false, cx),
            TextFieldEvent::Typed(_) => {}
        });

        let panes_view = cx.new_view(|_| PanesView { win_id: id, session: None });

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
            WinRole::Hosts => {
                let lf = widgets::text_field(cx, |f| f.hint("Label (e.g. prod)"));
                let af = widgets::text_field(cx, |f| f.hint("ssh://user@host or host.ts.net"));
                let pf = widgets::text_field(cx, |f| f.hint("Port"));
                cx.subscribe(&af, |_, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Changed) {
                        cx.invalidate();
                    }
                });
                cx.subscribe(&pf, |_, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Changed) {
                        cx.invalidate();
                    }
                });
                (Some(lf), Some(af), Some(pf), None)
            }
            WinRole::Askpass => {
                let is_secret = app().askpass.current_request().is_some_and(|r| matches!(r, AskpassRequest::Secret { echo: false, .. }));
                let f = widgets::text_field(cx, |f| f.password(is_secret).hint("Enter secret…"));
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
    fn where_it_is(&self) -> String {
        let hub = app().hub.clone();
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

    fn open_path_browser(&mut self, mode: PathBrowserMode, cx: &mut olo_kit::Cx<Self>) {
        let Some(bk) = self.focused_block_key() else { return };
        let hub = app().hub.clone();
        let dir = {
            let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
            st.blocks.get(&bk).map(|br| br.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".to_owned())
        };
        let to = cx.messenger();
        let key = bk.clone();
        hub.request_dir_listing(&bk, dir.clone(), mode == PathBrowserMode::InsertPath, move |listing| {
            to.send(WinMsg::DirListingReady { key, listing });
        });
        self.overlay = Some(ActiveOverlay::PathBrowser { mode, block: bk, dir, query: String::new(), entries: Vec::new(), selected: 0 });
        if let Some(f) = &self.overlay_field {
            f.update(cx, |f, cx| f.set_text(String::new(), cx));
            f.focus(cx);
        }
        cx.invalidate();
    }

    fn cancel_preview(&mut self) {
        if let Some(job) = self.preview_job.take() {
            job.abort();
        }
        if matches!(self.overlay, Some(ActiveOverlay::QuickLook { .. })) {
            self.overlay = None;
            self.clear_feedback();
        }
    }

    fn open_preview(&mut self, bk: BlockKey, path: String) {
        self.cancel_preview();
        self.overlay = Some(ActiveOverlay::QuickLook { path: path.clone(), content: QuickLookContent::Loading });
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
            Some(ActiveOverlay::PathBrowser { mode, block, dir, query, entries, selected }) => {
                let filtered: Vec<&olo_rex_client::DirEntry> = entries
                    .iter()
                    .filter(|e| {
                        if mode == PathBrowserMode::GoToDir && e.file {
                            return false;
                        }
                        overlays::fuzzy_score(&e.name, &query).is_some()
                    })
                    .collect();
                if let Some(entry) = filtered.get(selected) {
                    let full = if dir.ends_with('/') { format!("{dir}{}", entry.name) } else { format!("{dir}/{}", entry.name) };
                    let hub = app().hub.clone();
                    match mode {
                        PathBrowserMode::InsertPath => {
                            let s = format!("{} ", crate::hub::shell_quote(&full));
                            hub.send_input(&block, s.into_bytes());
                        }
                        PathBrowserMode::GoToDir => {
                            let s = format!("cd {}\r", crate::hub::shell_quote(&full));
                            hub.send_input(&block, s.into_bytes());
                        }
                    }
                }
            }
            Some(ActiveOverlay::QuickLook { .. }) | None => {}
        }
        self.focus.focus(cx);
        cx.invalidate();
    }

    fn pick_overlay_index(&mut self, index: usize, cx: &mut olo_kit::Cx<Self>) {
        match &mut self.overlay {
            Some(ActiveOverlay::RexPrompt { host_key, req, .. }) => {
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
            Some(ActiveOverlay::PathBrowser { mode, block, dir, entries, .. }) => {
                if let Some(entry) = entries.get(index).cloned() {
                    let full = if dir.ends_with('/') { format!("{dir}{}", entry.name) } else { format!("{dir}/{}", entry.name) };
                    let hub = app().hub.clone();
                    let bk = block.clone();
                    let m = *mode;
                    self.overlay = None;
                    match m {
                        PathBrowserMode::InsertPath => {
                            hub.send_input(&bk, format!("{} ", crate::hub::shell_quote(&full)).into_bytes());
                        }
                        PathBrowserMode::GoToDir => {
                            hub.send_input(&bk, format!("cd {}\r", crate::hub::shell_quote(&full)).into_bytes());
                        }
                    }
                    self.focus.focus(cx);
                    cx.invalidate();
                }
            }
            _ => {}
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
        if let Some(bk) = self.focused_block_key() {
            let hub = app().hub.clone();
            hub.find(&bk, (!q.is_empty()).then_some(q.as_str()));
        }
        cx.invalidate();
    }

    fn step_find(&mut self, back: bool, cx: &mut olo_kit::Cx<Self>) {
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
            "app.pair_device" => open_or_raise_panel(WinRole::Access),
            "app.diagnostics" => open_or_raise_panel(WinRole::Diagnostics),
            "app.new_window" => open_workspace_window(None),
            "app.close_window" => cx.request_close(),
            "app.quit" => olo_kit::quit(),
            "app.reload_config" => hub.reload_server_config(&self.active_host, Feedback::start(self.id)),
            "host.reconnect" => hub.connect_host(&self.active_host),
            "session.new" => hub.create_session(&self.active_host, None, None, Feedback::start(self.id)),
            "session.close" => {
                if let Some((hk, sess)) = self.resolve_active_session() {
                    hub.close_session(&hk, &sess.session_id, Feedback::start(self.id));
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
                    }
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
            "files.path_browser" => self.open_path_browser(PathBrowserMode::InsertPath, cx),
            "files.go_to_dir" => self.open_path_browser(PathBrowserMode::GoToDir, cx),
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
                    } else {
                        self.open_path_browser(PathBrowserMode::InsertPath, cx);
                    }
                }
            }
            "files.open_in_tracker" => {
                if let Some(bk) = self.focused_block_key() {
                    let pwd = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).map(|br| br.lock().unwrap_or_else(|e| e.into_inner()).pwd.clone()).unwrap_or_default();
                    if !pwd.is_empty() {
                        bridge::open_folder_in_tracker(std::path::Path::new(&pwd));
                    }
                }
            }
            "agent.keep_going_toggle" => {
                if let Some(bk) = self.focused_block_key() {
                    let msg = hub.state.lock().unwrap_or_else(|e| e.into_inner()).settings.keep_going_message.clone();
                    if let Some(br) = hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                        let mut g = br.lock().unwrap_or_else(|e| e.into_inner());
                        if g.keep_going.is_some() {
                            g.keep_going = None;
                        } else {
                            g.keep_going = Some(KeepGoingState::new(msg));
                        }
                    }
                    cx.invalidate();
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
            "olo.install_remote_shim" => {
                if let Some(bk) = self.focused_block_key() {
                    hub.install_remote_shim_in_pane(&bk);
                }
            }
            "olo.install_remote_skill" => {
                if let Some(bk) = self.focused_block_key() {
                    hub.install_remote_skill_in_pane(&bk);
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
                    if let Some(ov) = &mut self.overlay {
                        match ov {
                            ActiveOverlay::PathBrowser { selected, .. } => *selected = selected.saturating_sub(1),
                            ActiveOverlay::RexPrompt { selected_idx, .. } => *selected_idx = selected_idx.saturating_sub(1),
                            ActiveOverlay::QuickLook { content: QuickLookContent::Text { scroll, .. }, .. } => {
                                *scroll = scroll.saturating_sub(3);
                            }
                            _ => {}
                        }
                    }
                    cx.invalidate();
                    cx.stop_propagation();
                    return;
                }
                "down" => {
                    if let Some(ov) = &mut self.overlay {
                        match ov {
                            ActiveOverlay::PathBrowser { selected, .. } => *selected += 1,
                            ActiveOverlay::RexPrompt { selected_idx, .. } => *selected_idx += 1,
                            ActiveOverlay::QuickLook { content: QuickLookContent::Text { scroll, lines }, .. } => {
                                *scroll = (*scroll + 3).min(lines.len().saturating_sub(1));
                            }
                            _ => {}
                        }
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

        // To the pane the keyboard is in (to every pane of the tab while Type in All Panes is on).
        let Some(bk) = self.focused_block_key() else { return };
        let targets = self.input_target_blocks(&bk);
        let hub = app().hub.clone();

        if let Some(bytes) = encode_terminal_special_key(k).filter(|_| !cmd) {
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
        let (keep_going, selected, copying, found) = block.as_ref().map_or((false, false, false, false), |block| {
            let g = block.lock().unwrap_or_else(|e| e.into_inner());
            (g.keep_going.is_some(), g.terminal.has_selection(), g.copy_mode.is_some(), g.search.total_matches() > 0)
        });
        let session = self.resolve_active_session().map(|(_, s)| s);
        let window = session.as_ref().and_then(|s| s.active_window().or_else(|| s.windows.first()).map(|w| (w.zoomed_block_id.is_some(), w.all_blocks().count())));
        let (zoomed, panes) = window.unwrap_or((false, 0));
        let tab = window.is_some();
        // The session's tabs by name, the first nine with their keys
        // (⌘1 to ⌘9), as Terminal's Tabs menu.
        let count = session.as_ref().map_or(0, |s| s.windows.len());
        let mut tabs = Menu::new("Tabs")
            .item("New Tab", "window.new")
            .item("Close Tab", "window.close")
            .enabled(tab)
            .separator()
            .item("Next Tab", "window.next")
            .enabled(count > 1)
            .item("Previous Tab", "window.previous")
            .enabled(count > 1);
        if let Some(s) = &session {
            let active = s.active_window().or_else(|| s.windows.first()).map(|w| w.window_id.clone());
            tabs = tabs.separator();
            let names = crate::hub::session_tab_names(&hub.state.lock().unwrap_or_else(|e| e.into_inner()), &self.active_host, s);
            for (i, (w, name)) in s.windows.iter().zip(names).enumerate() {
                tabs = tabs.item(name, format!("window.select_{}", i + 1)).checked(Some(&w.window_id) == active.as_ref());
            }
        }
        // A pane of this computer's own server: its folder is one Tracker
        // can open, and it needs no bridge installed.
        let here = self.active_host == "local";

        vec![
            Menu::new("Rex")
                .app_items(&["rex", "notifications"])
                .item("New Window", "app.new_window")
                .item("New Session", "session.new")
                .separator()
                .item("Hosts…", "app.hosts")
                .item("Remote Access…", "app.pair_device")
                .item("Diagnostics…", "app.diagnostics")
                .separator()
                .item("Reload Server Configuration", "app.reload_config")
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
                .checked(sidebar_mode == SidebarMode::Activity)
                .separator()
                .item("Quick Look", "files.quick_look")
                .enabled(pane)
                .item("Browse Folders…", "files.path_browser")
                .enabled(pane)
                .item("Go to Folder…", "files.go_to_dir")
                .enabled(pane)
                .item(olo_kit_base::tracker::LABEL, "files.open_in_tracker")
                .enabled(pane && here),
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
                .item("Close Pane", "pane.close")
                .enabled(pane),
            Menu::new("Agents")
                .item("Next Waiting Agent", "agent.jump_next_blocked")
                .item("Mark All as Read", "agent.mark_all_read")
                .separator()
                .item("Keep It Going", "agent.keep_going_toggle")
                .checked(keep_going)
                .enabled(pane)
                .separator()
                .item("Install Olo’s Bridge on This Host", "olo.install_remote_shim")
                .enabled(pane && !here)
                .item("Install Rex Skill for Agents", "olo.install_remote_skill")
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

        let menu_h = olo_kit::menu::BAR_HEIGHT;
        let total_w = f32::from(viewport.width).max(320.0);
        let total_h = (f32::from(viewport.height) - menu_h).max(160.0);

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
                for b in w.all_blocks() {
                    let bk = BlockKey { host_key: hk.clone(), session_id: sess.session_id.clone(), block_id: b.block_id.clone() };
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
                let act = tab_act;
                let icon_fn: Option<olo_kit::widgets::strip::PaintFn> = Some(Rc::new(move |g: &mut Painter, b: Bounds<Pixels>| {
                    paint_activity_icon(g, b, act);
                }));
                strip_tabs.push(StripTab {
                    label: label.into(),
                    icon: icon_fn,
                    status: match tab_act {
                        AgentActivity::Blocked => Some("Waiting for approval".into()),
                        AgentActivity::Working => Some("Working".into()),
                        AgentActivity::Error => Some("Error".into()),
                        AgentActivity::Done => Some("Done".into()),
                        AgentActivity::Idle => None,
                    },
                    unread: tab_unread,
                    ..Default::default()
                });
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

        // Main terminal panes view (isolated `PanesView` child so 120 Hz output doesn't rebuild menus/sidebar!).
        let panes_el = if let Some((hk, sess)) = &sess_info
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
            div().flex_1().min_w_0().min_h_0().size_full().child(child)
        } else {
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
                Some(HostConnState::Connecting) if host.is_some_and(|h| app().askpass.waits_for_agent(&h.endpoint_raw)) => {
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
                    paint_circle(g, center, m::TIGHT, style::c(p.panel_background));
                    paint_circle(g, center, m::TIGHT - f32::from(style::hairline(g)), style::disabled_text(&p));
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

        // Optional Find Bar above panes.
        let focused_bk_for_find = if self.find_open { self.focused_block_key() } else { None };
        let find_el = if self.find_open
            && let Some(ff) = &self.find_field
        {
            let query = ff.read(cx).text().to_owned();
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
            Some(find_bar(cx, ff, said, total > 0, cx.msg_with(|back: &bool| WinMsg::FindStep(*back)), cx.msg_with(|_: &()| WinMsg::FindClose)))
        } else {
            None
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
                body_row = body_row.child(center);
            }
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
        root = root.child(
            div().w_full().flex_shrink_0().flex().flex_col().child(widgets::separator()).child(
                div()
                    .relative()
                    .w_full()
                    .min_h(px(m::LABEL_ROW))
                    .px(px(m::SPACING))
                    .child(canvas(|g, b| style::fill_rect(g, b, style::c(theme::palette().panel_background))).paint_key(1).absolute().inset_0())
                    .child(line),
            ),
        );

        // Place `side_divider` directly on the outer window from `BAR_HEIGHT` to bottom (exact `apps/phone` pattern).
        if !self.side_pane.alone() {
            root = root.child(side_divider(&self.side_pane, side_w, menu_h, total_h, "Rex Sidebar", cx.msg_with(|_: &()| WinMsg::SidebarMoved)));
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

    fn render_sidebar(&mut self, width: f32, cx: &mut olo_kit::Cx<Self>) -> Element {
        let hub = app().hub.clone();
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
                    let mut unread_count = 0usize;
                    if let Some(v) = view {
                        for w in &v.windows {
                            for b in w.all_blocks() {
                                let bk = BlockKey { host_key: h.key.clone(), session_id: v.session_id.clone(), block_id: b.block_id.clone() };
                                if let Some(br) = st.blocks.get(&bk) {
                                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                                    sess_act = higher_activity(sess_act, g.agent_activity);
                                    if g.unread {
                                        unread_count += 1;
                                    }
                                    if g.agent_activity != AgentActivity::Idle || st.settings.activity_include_idle {
                                        let label = if !g.title.is_empty() { g.title.clone() } else { crate::hub::tab_label(w).map_or_else(|| olo_kit::words::tr("Shell").into(), str::to_owned) };
                                        let detail = g.program_statuses.first().and_then(|r| r.msg.clone().or_else(|| r.title.clone())).unwrap_or_else(|| g.pwd.clone());
                                        activity_rows.push((bk, h.label.clone(), label, detail, g.agent_activity));
                                    }
                                }
                            }
                        }
                    }
                    let label = if !s.label.is_empty() { s.label.clone() } else { s.session_id.clone() };
                    sessions.push((s.session_id.clone(), label, sess_act, unread_count));
                }
                hosts_snap.push((h.key.clone(), h.label.clone(), h.conn_state.clone(), sessions));
            }
            activity_rows.sort_by_key(|(_, _, _, _, a)| activity_sort_rank(*a));
            (mode, hosts_snap, activity_rows)
        };

        let back = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
        })
        .paint_key(2)
        .absolute()
        .inset_0();

        let blocked_n = activity_rows.iter().filter(|(_, _, _, _, a)| matches!(a, AgentActivity::Blocked | AgentActivity::Error)).count();
        let act_label = if blocked_n > 0 { format!("Activity ({blocked_n})") } else { "Activity".to_owned() };

        let hub_s1 = hub.clone();
        let hub_s2 = hub.clone();
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

        let seg_sessions = sidebar_segment_button(
            "Sessions".to_owned(),
            mode == SidebarMode::Sessions,
            true,
            cx.msg_with(move |_: &()| {
                let mut st = hub_s1.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.sidebar_mode = SidebarMode::Sessions;
                st.settings.save();
                WinMsg::Redraw
            }),
        );
        let seg_activity = sidebar_segment_button(
            act_label,
            mode == SidebarMode::Activity,
            false,
            cx.msg_with(move |_: &()| {
                let mut st = hub_s2.state.lock().unwrap_or_else(|e| e.into_inner());
                st.settings.sidebar_mode = SidebarMode::Activity;
                st.settings.save();
                WinMsg::Redraw
            }),
        );
        let header = div()
            .relative()
            .w_full()
            .h(px(widgets::STRIP_HEIGHT))
            .flex_shrink_0()
            .pt(px(8.0))
            .px(px(m::SPACING))
            .flex()
            .flex_row()
            .items_end()
            .child(header_bg)
            .child(div().w_full().flex().flex_row().items_end().child(seg_sessions).child(seg_activity));

        let mut list = div().flex_shrink_0().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SPACING));
        if mode == SidebarMode::Sessions {
            for (hk, hlabel, conn, sessions) in hosts_snap {
                let connected = conn == HostConnState::Connected;
                let tip = match &conn {
                    HostConnState::Connected => olo_kit::words::tr("Connected").into(),
                    HostConnState::Connecting => olo_kit::words::tr("Connecting…").into(),
                    HostConnState::Disconnected => olo_kit::words::tr("Disconnected").into(),
                    HostConnState::Error(why) | HostConnState::Reconnecting { last_error: why, .. } => olo_kit::tr!("Disconnected: {}", why),
                };
                let selected = self.active_host == hk && !sessions.iter().any(|(s, ..)| self.active_session.as_ref() == Some(s));
                let key = style::key((&hlabel, &tip, connected, selected));
                let state_words = tip.clone();
                let tip_key = format!("rex-host-{hk}");
                olo_kit::tip::retext(cx, tip_key.clone(), tip.clone());
                let hk_new = hk.clone();
                let window = self.id;
                let pick = cx.msg(WinMsg::SelectHost(hk.clone()));
                let row_label = canvas(move |g, b| {
                    let p = theme::palette();
                    let dot = if connected { style::c(p.success) } else { style::disabled_text(&p) };
                    let y = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
                    paint_circle(g, point(b.left() + px(m::TIGHT), y), 3.5, dot);
                    // (Selected, a disconnected host's name is a selected
                    // name: dimmed on the selection it read as something
                    // that can't be chosen, and choosing it is the way
                    // to Reconnect. Its grey dot says the rest.)
                    let ink = if selected {
                        style::c(p.list_selected_item_text)
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
                    row = row.bg(style::c(theme::palette().list_selected_background));
                }
                row = row.child(row_label).child(compact_icon_button("New Session", IconGlyph::Plus, connected, cx.msg(WinMsg::CreateSession(hk_new, None))));
                let mut group = div().w_full().flex_shrink_0().flex().flex_col().child(olo_kit::tip::on(row, tip_key, tip));
                for (sid, slabel, sact, unread) in sessions {
                    let is_active = self.active_host == hk && self.active_session.as_deref() == Some(sid.as_str());
                    let hk_sel = hk.clone();
                    let sid_sel = sid.clone();
                    let badge = if unread > 0 { Some(format!("{unread}")) } else { None };
                    let hk_end = hk.clone();
                    let hub_end = hub.clone();
                    group = group.child(sidebar_row(slabel, badge, sact, is_active, cx.msg_with(move |_: &()| WinMsg::SelectSession(hk_sel.clone(), sid_sel.clone()))).on_mouse_down(
                        MouseButton::Right,
                        move |e, cx| {
                            let hub = hub_end.clone();
                            let (hk, sid) = (hk_end.clone(), sid.clone());
                            menu::context_menu(cx, Menu::new("").item("End Session", "session.close"), e.position, move |cmd, _| {
                                if cmd == "session.close" {
                                    hub.close_session(&hk, &sid, Feedback::start(window));
                                }
                            });
                            cx.stop_propagation();
                        },
                    ));
                }
                list = list.child(group);
            }
        } else {
            if activity_rows.is_empty() {
                list = list.child(div().py(px(m::SECTION)).child(StringView::new("All agents and shells are quiet.").dim(true)));
            }
            for (bk, host_lbl, title, detail, act) in activity_rows {
                let bk_jump = bk.clone();
                let sub = if detail.is_empty() { host_lbl } else { format!("{host_lbl} • {detail}") };
                list = list.child(sidebar_activity_row(title, sub, act, cx.msg_with(move |_: &()| WinMsg::FocusBlock(bk_jump.clone()))));
            }
        }

        // Bottom-anchored sidebar footer (8 pt above bottom, crisp 1 px top rule).
        let footer_bg = canvas(|g, b| {
            let p = theme::palette();
            style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.25));
        })
        .paint_key(202)
        .absolute()
        .inset_0();
        let footer = div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .px(px(m::SPACING))
            .py(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .child(footer_bg)
            .child(Button::new("Add Host…").small().on_click(cx.msg_with(|_: &olo_kit::ClickEvent| WinMsg::Command("app.hosts".into()))));

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
            .child(footer)
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
                let (Some(lf), Some(af), Some(pf)) = (&self.host_label_field, &self.host_addr_field, &self.fwd_port_field) else {
                    return div();
                };
                let hub_r = hub.clone();
                let hub_d = hub.clone();
                let window = self.id;
                let hub_f = hub.clone();
                let hub_s = hub.clone();
                let from = self.fwd_host.clone();
                let askpass = app().askpass.clone();
                let panel = panels::render_hosts_panel(
                    cx,
                    &hub,
                    &|endpoint| askpass.waits_for_agent(endpoint),
                    lf,
                    af,
                    pf,
                    from.as_deref(),
                    cx.msg_with(|_: &()| WinMsg::HostsAdd),
                    cx.msg_with(move |hk: &String| {
                        hub_r.connect_host(hk);
                        WinMsg::Redraw
                    }),
                    cx.msg_with(move |hk: &String| {
                        hub_d.remove_host(hk);
                        WinMsg::Redraw
                    }),
                    cx.msg_with(move |(hk, sid, port, forward): &(String, String, u16, bool)| {
                        if *forward {
                            hub_f.add_local_forward(hk, sid, "127.0.0.1".to_owned(), *port, 0, true, Feedback::start(window));
                        } else {
                            hub_f.open_local_port(*port, Feedback::start(window));
                        }
                        WinMsg::Redraw
                    }),
                    cx.msg_with(move |fid: &String| {
                        hub_s.remove_forward(fid);
                        WinMsg::Redraw
                    }),
                    cx.msg_with(|_: &()| WinMsg::HostsManualForward),
                    cx.msg_with(|key: &String| WinMsg::HostsForwardFrom(key.clone())),
                    cx.msg_with(|_: &()| WinMsg::CloseWindow),
                );
                div().w_full().flex().flex_col().child(panel).children(self.feedback_line(cx).map(|el| div().px(px(m::WINDOW_INSET)).pb(px(m::SPACING)).child(el)))
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
            WinMsg::QuickLookReady(serial, path, content) => {
                if serial != self.feedback_serial {
                    return;
                }
                self.preview_job = None;
                self.overlay = Some(ActiveOverlay::QuickLook { path, content });
                cx.invalidate();
            }
            WinMsg::DirListingReady { key, listing } => {
                if let Some(ActiveOverlay::PathBrowser { block, dir, entries, selected, .. }) = &mut self.overlay
                    && *block == key
                {
                    *dir = listing.path;
                    *entries = listing.entries;
                    *selected = 0;
                    cx.invalidate();
                }
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
            WinMsg::FocusBlock(bk) => {
                self.jump_to(JumpTarget::Pane(bk), cx);
                cx.activate_window();
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
                if let Some(bk) = self.focused_block_key()
                    && let Some(br) = app().hub.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned()
                {
                    let _ = br.lock().unwrap_or_else(|e| e.into_inner()).set_find_query(None);
                }
                self.focus.focus(cx);
                cx.invalidate();
            }
            WinMsg::HostsAdd => {
                let label = self.host_label_field.as_ref().map(|f| f.read(cx).text().trim().to_owned()).unwrap_or_default();
                let addr = self.host_addr_field.as_ref().map(|f| f.read(cx).text().trim().to_owned()).unwrap_or_default();
                if !addr.is_empty() {
                    let lbl = if label.is_empty() { addr.clone() } else { label };
                    let id = format!("host-{}", uuid::Uuid::new_v4().simple());
                    let hub = app().hub.clone();
                    hub.add_and_connect_host(id, lbl, addr, true);
                    if let Some(f) = &self.host_label_field {
                        f.update(cx, |f, cx| f.set_text(String::new(), cx));
                    }
                    if let Some(f) = &self.host_addr_field {
                        f.update(cx, |f, cx| f.set_text(String::new(), cx));
                    }
                    cx.invalidate();
                }
            }
            WinMsg::HostsForwardFrom(key) => {
                self.fwd_host = Some(key);
                cx.invalidate();
            }
            WinMsg::HostsManualForward => {
                let port_s = self.fwd_port_field.as_ref().map(|f| f.read(cx).text().trim().to_owned()).unwrap_or_default();
                if let Ok(port) = port_s.parse::<u16>()
                    && port > 0
                {
                    let hub = app().hub.clone();
                    // From the host chosen (the first of the others, with none chosen),
                    // through a session of it.
                    let target = {
                        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
                        let key = self.fwd_host.clone().or_else(|| st.host_order.iter().find(|k| st.hosts.get(*k).is_some_and(|h| !h.is_local)).cloned());
                        key.and_then(|key| st.hosts.get(&key)).and_then(|h| h.session_summaries.first().map(|s| (h.key.clone(), s.session_id.clone())))
                    };
                    // (Forwarded, not opened: a port typed here may be a
                    // database's. "Forward and Open" is for one a program
                    // in a pane is seen to listen on.)
                    if let Some((hk, sid)) = target {
                        hub.add_local_forward(&hk, &sid, "127.0.0.1".to_owned(), port, 0, false, Feedback::start(self.id));
                    }
                    if let Some(f) = &self.fwd_port_field {
                        f.update(cx, |f, cx| f.set_text(String::new(), cx));
                    }
                }
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
        self.cancel_preview();
        app().attention.hide(self.id);
        wins().map.remove(&self.id);
    }
}

impl TextInput for RexWindow {
    fn insert_text(&mut self, text: &str, cx: &mut olo_kit::Cx<Self>) {
        self.preedit = None;
        if self.role != WinRole::Workspace || self.overlay.is_some() || text.is_empty() {
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

        let vp_w = f32::from(cx.viewport_size().width).max(200.0);
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
        let (before_w, after_w) = if eff_horiz { ((avail_w - 2.0).max(80.0) * ratio, (avail_w - 2.0).max(80.0) * (1.0 - ratio)) } else { (avail_w, avail_w) };
        let before_el = render_layout_node(&split.before, before_w, host_key, host_label, is_local, session_id, win_view, settings, active_theme, metrics, pad, win_id, cx);
        let after_el = render_layout_node(&split.after, after_w, host_key, host_label, is_local, session_id, win_view, settings, active_theme, metrics, pad, win_id, cx);
        let split_id = split.split_id.clone();
        let split_id_drag = split.split_id.clone();
        let hk_d = host_key.to_owned();
        let sid_d = session_id.to_owned();
        let hk_dr = host_key.to_owned();
        let sid_dr = session_id.to_owned();
        let vp_span = if eff_horiz {
            f32::from(cx.viewport_size().width).max(240.0) * (node.rect[2] as f32).clamp(0.15, 1.0)
        } else {
            f32::from(cx.viewport_size().height).max(160.0) * (node.rect[3] as f32).clamp(0.15, 1.0)
        };
        let drag_state = Rc::new(std::cell::Cell::new(None::<(Point<Pixels>, f32)>));
        let ds1 = drag_state.clone();
        let ds2 = drag_state.clone();

        let divider = canvas(move |g, b| {
            let p = theme::palette();
            let (shadow, light) = bevel_colors(&p);
            let scale = g.scale_factor();
            let dpx = 1.0 / scale;
            if eff_horiz {
                let x0 = (f32::from(b.left()) * scale).round() / scale;
                g.paint_quad(olo_kit::fill(Bounds::new(point(px(x0), b.top()), size(px(dpx), b.size.height)), shadow));
                g.paint_quad(olo_kit::fill(Bounds::new(point(px(x0 + dpx), b.top()), size(px(dpx), b.size.height)), light));
            } else {
                let y0 = (f32::from(b.top()) * scale).round() / scale;
                g.paint_quad(olo_kit::fill(Bounds::new(point(b.left(), px(y0)), size(b.size.width, px(dpx))), shadow));
                g.paint_quad(olo_kit::fill(Bounds::new(point(b.left(), px(y0 + dpx)), size(b.size.width, px(dpx))), light));
            }
        })
        .paint_key(if eff_horiz { 3 } else { 4 })
        .cursor(if eff_horiz { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown })
        .on_mouse_down(MouseButton::Left, move |e, cx| {
            if e.click_count >= 2 {
                app().hub.set_split_ratio(&hk_d, &sid_d, &split_id, 0.5);
                ds1.set(None);
            } else {
                ds1.set(Some((e.position, ratio)));
            }
            cx.stop_propagation();
        })
        .on_drag_move(move |e: &MouseMoveEvent, _| {
            if e.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let Some((origin, orig_ratio)) = ds2.get() else { return };
            let delta = if eff_horiz { f32::from(e.position.x) - f32::from(origin.x) } else { f32::from(e.position.y) - f32::from(origin.y) };
            let next = (orig_ratio + delta / vp_span).clamp(0.15, 0.85) as f64;
            app().hub.set_split_ratio(&hk_dr, &sid_dr, &split_id_drag, next);
        });

        let mut container = div().size_full().flex();
        if eff_horiz {
            container = container
                .flex_row()
                .child(div().flex_grow().flex_basis(px(ratio * 1000.0)).min_w_0().h_full().child(before_el))
                .child(divider.w(px(2.0)).h_full().flex_shrink_0())
                .child(div().flex_grow().flex_basis(px((1.0 - ratio) * 1000.0)).min_w_0().h_full().child(after_el));
        } else {
            container = container
                .flex_col()
                .child(div().flex_grow().flex_basis(px(ratio * 1000.0)).min_h_0().w_full().child(before_el))
                .child(divider.h(px(2.0)).w_full().flex_shrink_0())
                .child(div().flex_grow().flex_basis(px((1.0 - ratio) * 1000.0)).min_h_0().w_full().child(after_el));
        }
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

    let (creator_name, title, pwd, activity, ports_list, upload_prog, password_prompt) = {
        let g = br.lock().unwrap_or_else(|e| e.into_inner());
        (g.creator_name.clone(), g.title.clone(), g.pwd.clone(), g.agent_activity, g.ports.ports.iter().map(|p| p.port).collect::<Vec<u16>>(), g.upload_progress.clone(), g.password_prompt)
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
        let mut label_parts = Vec::new();
        if !title.is_empty() {
            label_parts.push(title);
        } else if !pwd.is_empty() {
            label_parts.push(pwd.clone());
        } else {
            label_parts.push("Shell".to_owned());
        }
        let header_title = label_parts.join(" — ");
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

        // Consistent, calm pane header rule at every width:
        // - Show Split Right / Split Down ONLY on the focused pane (acts as a clear second focus cue and cuts button noise!).
        // - Keep Close on every pane.
        let show_split_btns = focused && pane_w >= 230.0;
        let show_close_btn = pane_w >= 140.0;
        let show_extras = pane_w >= 260.0;
        let right_controls_w = m::WINDOW_INSET
            + if show_close_btn { 18.0 + m::TIGHT } else { 0.0 }
            + if show_split_btns { (18.0 + m::TIGHT) * 2.0 } else { 0.0 }
            + if show_extras && ports_pill.is_some() { 48.0 + m::TIGHT } else { 0.0 }
            + if show_extras && zoom_pill.is_some() { 54.0 + m::TIGHT } else { 0.0 };

        let show_hl = focused;
        let bk_click = bk.clone();
        let bk_split_r = bk.clone();
        let bk_split_d = bk.clone();
        let bk_zoom = bk.clone();
        let bk_close = bk.clone();
        let title_paint = header_title.clone();
        let key = style::key((&header_title, &git_pill, &ports_pill, &host_pill, focused, zoomed, activity as u8, pane_w as i32));

        let head_bg = canvas(move |g, b| {
            let p = theme::palette();
            // Focused pane header uses `panel_background` (so active TabStrip tab opens seamlessly into it);
            // unfocused pane headers sit one step quieter with secondary text!
            let bg = if focused { style::c(p.panel_background) } else { style::shade(&p, p.panel_background, 1.07) };
            g.paint_quad(olo_kit::fill(b, bg));
            let (shadow, _) = bevel_colors(&p);
            style::hline_above(g, b.left(), b.right(), b.bottom(), shadow);
            if show_hl {
                // Place a high-contrast amber/yellow focus bar along the bottom edge of the focused pane header:
                // rich ochre in Light mode (>= 4.5:1 on pale panel) and Olo yellow in Dark mode.
                let bar_color = if p.mode == olo_theme::ThemeMode::Light { style::c([168, 102, 0, 255]) } else { style::c(p.tab_focused) };
                let bar_h = if p.mode == olo_theme::ThemeMode::Light { 2.5 } else { 2.0 };
                g.paint_quad(olo_kit::fill(Bounds::new(point(b.left(), b.bottom() - px(bar_h)), size(b.size.width, px(bar_h))), bar_color));
            }
            let text_middle = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
            let icon_b = style::snap_centred(g, point(b.left() + px(16.0), text_middle), 12.0, 12.0);
            paint_activity_icon(g, icon_b, activity);
            let title_max_w = (f32::from(b.size.width) - 28.0 - right_controls_w - 6.0).max(24.0);
            let title_ink = if focused { style::c(p.panel_text) } else { style::secondary_text(&p) };
            style::label_fit(g, b.left() + px(28.0), b.center().y, style::Align::Left, title_paint.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), title_ink, px(title_max_w));
        })
        .paint_key(key)
        .absolute()
        .inset_0();

        let mut bar = div()
            .relative()
            .w_full()
            .h(px(header_h))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(m::TIGHT))
            .pl(px(24.0))
            .pr(px(m::WINDOW_INSET))
            .on_mouse_down(MouseButton::Left, move |_, _| {
                let hub = app().hub.clone();
                hub.focus_block(&bk_click.host_key, &bk_click.session_id, &bk_click.block_id);
            })
            .child(head_bg);

        if show_extras {
            if let Some(hp) = host_pill {
                bar = bar.child(StringView::new(hp).bold(true));
            }
            if let Some(gp) = git_pill {
                bar = bar.child(StringView::new(gp).dim(true));
            }
            if let Some(pp) = ports_pill {
                bar = bar.child(sunken_badge(pp, move |_, _| open_or_raise_panel(WinRole::Hosts)));
            }
            if let Some(zp) = zoom_pill {
                let z = bk_zoom.clone();
                bar = bar.child(sunken_badge(zp, move |_, _| {
                    app().hub.toggle_zoom(&z.host_key, &z.session_id, Some(z.block_id.clone()));
                }));
            }
        }
        if show_split_btns {
            bar = bar
                .child(compact_icon_button("Split Right", IconGlyph::SplitRight, true, move |_, _| {
                    app().hub.split_pane(&bk_split_r.host_key, &bk_split_r.session_id, &bk_split_r.block_id, "right", None);
                }))
                .child(compact_icon_button("Split Down", IconGlyph::SplitDown, true, move |_, _| {
                    app().hub.split_pane(&bk_split_d.host_key, &bk_split_d.session_id, &bk_split_d.block_id, "down", None);
                }));
        }
        if show_close_btn {
            bar = bar.child(compact_icon_button("Close Pane", IconGlyph::Close, true, move |_, _| {
                app().hub.close_block(&bk_close.host_key, &bk_close.session_id, &bk_close.block_id);
            }));
        }
        Some(bar)
    } else {
        None
    };

    let visual = cx.window_state::<PaneVisuals>().0.entry(bk.clone()).or_default().clone();

    let br_paint = br.clone();
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

    let pane_m1 = cx.messenger();
    let pane_m2 = cx.messenger();
    let pane_m3 = cx.messenger();

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
        let missing = br_ref.frame.capture(&mut br_ref.render_state, &br_ref.terminal).unwrap_or_default();
        if !missing.is_empty() {
            hub_paint.prepare_images(br_paint.clone(), bk_paint.clone(), br_ref.frame.owner, missing);
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
                hover_link: None,
                preedit: None,
            },
            &vis_paint,
        );

        g.agent(agent::Node {
            role: "Terminal",
            name: Some(if br_ref.title.is_empty() { "Rex Terminal".into() } else { br_ref.title.clone().into() }),
            bounds: rect,
            focused,
            enabled: true,
            clickable: false,
            ..Default::default()
        });
    })
    .size_full()
    .cursor(CursorStyle::IBeam)
    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
        let hub = app().hub.clone();
        hub.focus_block(&bk_mouse.host_key, &bk_mouse.session_id, &bk_mouse.block_id);
        let rect = vis_mouse.bounds.get();
        let (col, row) = metrics_mouse.cell_at(rect, pad, e.position, vis_mouse.shift.get());

        // ⌘-Click opens OSC 8 hyperlink / URL / file path (auto-forwarding remote localhost!).
        if e.modifiers.platform {
            let target = {
                let g = br_mouse.lock().unwrap_or_else(|e| e.into_inner());
                let osc8 = g.terminal.hyperlink_at_viewport(col, row as u32);
                let line = g.terminal.viewport_row_text(row as u32);
                links::link_at(&line, row as u32, col, osc8).map(|d| d.target)
            };
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
        // The menu bar's commands for this pane, by the names and with
        // the checks and keys they have there: the clipboard; its text;
        // the pane itself; closing it, alone and last.
        let (selected, copying, keep_going, agent) = {
            let g = br_menu.lock().unwrap_or_else(|e| e.into_inner());
            (g.terminal.has_selection(), g.copy_mode.is_some(), g.keep_going.is_some(), !g.program_statuses.is_empty())
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
            .item("Clear Scrollback", "terminal.clear_scrollback")
            .separator()
            .item("Split Right", "pane.split_right")
            .item("Split Down", "pane.split_down")
            .item("Zoom Pane", "pane.zoom_toggle")
            .checked(zoomed)
            .enabled(zoomed || has_splits);
        // (For a pane an agent runs in, or where it is already on.)
        if agent || keep_going {
            menu = menu.item("Keep It Going", "agent.keep_going_toggle").checked(keep_going);
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
        vis_scroll.scroll_smooth(delta_y, ch_scroll, &mut g.terminal);
        pane_m3.send(());
        cx.stop_propagation();
    });

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
    pane_col = pane_col.child(div().relative().flex_1().min_h_0().size_full().child(screen_canvas));

    if let Some((fname, frac)) = upload_prog {
        pane_col = pane_col.child(div().w_full().px(px(m::SPACING)).py(px(4.0)).child(StatusBar::new(frac).label(format!("Uploading {fname}…"))));
    }

    pane_col
}

fn sidebar_row(label: String, badge: Option<String>, act: AgentActivity, selected: bool, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let key = style::key((&label, &badge, act as u8, selected));
    let l_paint = label.clone();
    let b_paint = badge.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        if selected {
            style::fill_rect(g, b, style::c(p.list_selected_background));
        }
        let ink = style::c(if selected { p.list_selected_item_text } else { p.panel_text });
        // Indent sessions 15 pt under host header so icon left aligns to exact pixel with host title (x=27).
        let text_middle = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(b.left() + px(22.0), text_middle), 12.0, 12.0);
        paint_activity_icon(g, icon_b, act);
        let mut right = b.right() - px(8.0);
        if let Some(bg) = &b_paint {
            let r = style::label(g, right, b.center().y, style::Align::Right, bg.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::c([216, 56, 56, 255]));
            right = r.left() - px(6.0);
        }
        style::label_fit(
            g,
            b.left() + px(33.0),
            b.center().y,
            style::Align::Left,
            l_paint.clone(),
            style::PLAIN_SIZE,
            style::ui_font(FontWeight::NORMAL),
            ink,
            (right - (b.left() + px(33.0))).max(px(24.0)),
        );
        g.agent(agent::Node { role: "ListItem", name: Some(l_paint.clone().into()), bounds: b, selected: Some(selected), enabled: true, clickable: true, ..Default::default() });
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

fn sidebar_segment_button(label: String, active: bool, is_left: bool, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let key = style::key((&label, active, is_left));
    let l_paint = label.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        let bg = if active { style::shade(&p, p.control_background, 1.15) } else { style::c(p.control_background) };
        style::fill_rect(g, b, bg);
        if active {
            style::sunken(g, b, &p, false);
        } else {
            style::raised(g, b, &p);
        }
        let ink = style::c(p.control_text);
        style::label_fit(
            g,
            b.center().x,
            b.center().y,
            style::Align::Center,
            l_paint.clone(),
            style::SMALL_SIZE,
            style::ui_font(if active { FontWeight::BOLD } else { FontWeight::NORMAL }),
            ink,
            (b.size.width - px(8.0)).max(px(20.0)),
        );
        g.agent(agent::Node { role: "Tab", name: Some(l_paint.clone().into()), bounds: b, selected: Some(active), enabled: true, clickable: true, ..Default::default() });
    })
    .paint_key(key)
    .flex_1()
    .h(px(26.0))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

/// High-contrast 2-line bevel pair `(shadow, highlight)` that stays crisp in Light, Dark, and Night modes.
fn bevel_colors(p: &olo_theme::Palette) -> (olo_kit::Hsla, olo_kit::Hsla) {
    if p.mode == olo_theme::ThemeMode::Dark {
        (style::shade(p, p.panel_background, 1.55), style::c(olo_theme::tint(p.panel_background, 0.80)))
    } else {
        (style::shade(p, p.panel_background, 1.45), style::shade(p, p.panel_background, 0.72))
    }
}

#[derive(Clone, Copy)]
enum IconGlyph {
    Plus,
    SplitRight,
    SplitDown,
    Close,
}

fn compact_icon_button(aria_name: &'static str, glyph: IconGlyph, enabled: bool, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let key = style::key((aria_name, glyph as u8, enabled));
    let button = canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.control_background));
        style::raised(g, b, &p);
        let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
        let cx = f32::from(b.center().x);
        let cy = f32::from(b.center().y);
        match glyph {
            IconGlyph::Plus => {
                style::fill_rect(g, Bounds::new(point(px(cx - 3.5), px(cy - 0.5)), size(px(7.0), px(1.2))), ink);
                style::fill_rect(g, Bounds::new(point(px(cx - 0.5), px(cy - 3.5)), size(px(1.2), px(7.0))), ink);
            }
            IconGlyph::SplitRight => {
                let r = Bounds::new(point(px(cx - 4.5), px(cy - 3.5)), size(px(9.0), px(7.0)));
                g.paint_quad(olo_kit::outline(r, ink, olo_kit::BorderStyle::Solid).border_widths(px(1.0)));
                style::fill_rect(g, Bounds::new(point(px(cx - 0.5), px(cy - 3.0)), size(px(1.0), px(6.0))), ink);
            }
            IconGlyph::SplitDown => {
                let r = Bounds::new(point(px(cx - 4.5), px(cy - 3.5)), size(px(9.0), px(7.0)));
                g.paint_quad(olo_kit::outline(r, ink, olo_kit::BorderStyle::Solid).border_widths(px(1.0)));
                style::fill_rect(g, Bounds::new(point(px(cx - 4.0), px(cy - 0.5)), size(px(8.0), px(1.0))), ink);
            }
            IconGlyph::Close => {
                let d = 3.2f32;
                let mut p1 = PathBuilder::stroke(px(1.2));
                p1.move_to(point(px(cx - d), px(cy - d)));
                p1.line_to(point(px(cx + d), px(cy + d)));
                if let Ok(path) = p1.build() {
                    g.paint_path(path, ink);
                }
                let mut p2 = PathBuilder::stroke(px(1.2));
                p2.move_to(point(px(cx + d), px(cy - d)));
                p2.line_to(point(px(cx - d), px(cy + d)));
                if let Ok(path) = p2.build() {
                    g.paint_path(path, ink);
                }
            }
        }
        g.agent(agent::Node { role: "Button", name: Some(aria_name.into()), bounds: b, enabled, clickable: enabled, ..Default::default() });
    })
    .paint_key(key)
    .w(px(18.0))
    .h(px(16.0))
    .flex_shrink_0();
    if !enabled {
        return button;
    }
    button.cursor(CursorStyle::PointingHand).on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

fn sunken_badge(label: String, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let w = (label.len() as f32 * 6.0 + 12.0).max(40.0);
    let key = style::key(&label);
    let l_paint = label.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        // Clickable port/zoom badge matches the raised bevel style & height of adjacent header icon buttons!
        style::fill_rect(g, b, style::c(p.control_background));
        style::raised(g, b, &p);
        style::label_fit(
            g,
            b.center().x,
            b.center().y + px(0.5),
            style::Align::Center,
            l_paint.clone(),
            style::SMALL_SIZE,
            style::ui_font(FontWeight::NORMAL),
            style::c(p.control_text),
            (b.size.width - px(6.0)).max(px(16.0)),
        );
        g.agent(agent::Node { role: "Button", name: Some(l_paint.clone().into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
    })
    .paint_key(key)
    .w(px(w))
    .h(px(16.0))
    .flex_shrink_0()
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

fn paint_circle(g: &mut Painter, center: Point<Pixels>, radius: f32, color: olo_kit::Hsla) {
    let mut pb = PathBuilder::fill();
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let steps = 16;
    for i in 0..steps {
        let a = (i as f32 / steps as f32) * std::f32::consts::TAU;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 { pb.move_to(pt) } else { pb.line_to(pt) }
    }
    pb.close();
    if let Ok(path) = pb.build() {
        g.paint_path(path, color);
    }
}

fn sidebar_activity_row(title: String, subtitle: String, act: AgentActivity, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let key = style::key((&title, &subtitle, act as u8));
    let t_paint = title.clone();
    let s_paint = subtitle.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.control_background));
        style::raised(g, b, &p);
        let icon_b = style::snap_centred(g, point(b.left() + px(14.0), b.center().y), 12.0, 12.0);
        paint_activity_icon(g, icon_b, act);
        style::label_fit(
            g,
            b.left() + px(26.0),
            b.top() + px(13.0),
            style::Align::Left,
            t_paint.clone(),
            style::PLAIN_SIZE,
            style::ui_font(FontWeight::BOLD),
            style::c(p.panel_text),
            (b.size.width - px(34.0)).max(px(24.0)),
        );
        style::label_fit(
            g,
            b.left() + px(26.0),
            b.top() + px(28.0),
            style::Align::Left,
            s_paint.clone(),
            style::SMALL_SIZE,
            style::ui_font(FontWeight::NORMAL),
            style::secondary_text(&p),
            (b.size.width - px(34.0)).max(px(24.0)),
        );
    })
    .paint_key(key)
    .w_full()
    .h(px(38.0))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

fn paint_activity_icon(g: &mut Painter, b: Bounds<Pixels>, act: AgentActivity) {
    // Always draw the consistent Olo terminal icon first, then overlay a crisp circular badge dot only when active!
    if !icons::paint(g, b, "terminal") {
        paint_circle(g, b.center(), 3.0, style::c([130, 134, 144, 210]));
    }
    let dot_color = match act {
        AgentActivity::Idle => return,
        AgentActivity::Working => [52, 132, 240, 255],
        AgentActivity::Blocked => [235, 148, 24, 255],
        AgentActivity::Error => [216, 56, 56, 255],
        AgentActivity::Done => [56, 174, 92, 255],
    };
    let dot_center = point(b.right() - px(2.5), b.bottom() - px(2.5));
    paint_circle(g, dot_center, 3.4, style::c([18, 22, 30, 220]));
    paint_circle(g, dot_center, 2.4, style::c(dot_color));
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

fn encode_terminal_special_key(ks: &olo_kit::Keystroke) -> Option<Vec<u8>> {
    let ctrl = ks.modifiers.control;
    let alt = ks.modifiers.alt;
    let shift = ks.modifiers.shift;
    let meta = ks.modifiers.platform;
    if meta {
        return None;
    }

    if ctrl && !alt && ks.key.len() == 1 {
        let ch = ks.key.bytes().next()?;
        let lower = ch.to_ascii_lowercase();
        if (b'a'..=b'z').contains(&lower) {
            return Some(vec![lower - b'a' + 1]);
        }
        return match ch {
            b'[' => Some(vec![0x1b]),
            b'\\' => Some(vec![0x1c]),
            b']' => Some(vec![0x1d]),
            b'^' | b'6' => Some(vec![0x1e]),
            b'_' | b'-' => Some(vec![0x1f]),
            b' ' => Some(vec![0x00]),
            _ => None,
        };
    }

    let base: Option<&[u8]> = match ks.key.as_str() {
        "enter" | "return" => Some(b"\r"),
        "backspace" => Some(b"\x7f"),
        "tab" => Some(if shift { b"\x1b[Z" } else { b"\t" }),
        "escape" => Some(b"\x1b"),
        "up" => Some(b"\x1b[A"),
        "down" => Some(b"\x1b[B"),
        "right" => Some(b"\x1b[C"),
        "left" => Some(b"\x1b[D"),
        "home" => Some(b"\x1b[H"),
        "end" => Some(b"\x1b[F"),
        "pageup" => Some(b"\x1b[5~"),
        "pagedown" => Some(b"\x1b[6~"),
        "delete" => Some(b"\x1b[3~"),
        "insert" => Some(b"\x1b[2~"),
        "f1" => Some(b"\x1bOP"),
        "f2" => Some(b"\x1bOQ"),
        "f3" => Some(b"\x1bOR"),
        "f4" => Some(b"\x1bOS"),
        "f5" => Some(b"\x1b[15~"),
        "f6" => Some(b"\x1b[17~"),
        "f7" => Some(b"\x1b[18~"),
        "f8" => Some(b"\x1b[19~"),
        "f9" => Some(b"\x1b[20~"),
        "f10" => Some(b"\x1b[21~"),
        "f11" => Some(b"\x1b[23~"),
        "f12" => Some(b"\x1b[24~"),
        _ => None,
    };
    if let Some(seq) = base {
        let mut out = Vec::with_capacity(seq.len() + 1);
        if alt {
            out.push(0x1b);
        }
        out.extend_from_slice(seq);
        return Some(out);
    }
    if alt && ks.key.len() == 1 {
        let mut out = vec![0x1b];
        out.extend_from_slice(ks.key.as_bytes());
        return Some(out);
    }
    None
}

#[cfg(test)]
mod workspace_tests {
    use super::*;
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
