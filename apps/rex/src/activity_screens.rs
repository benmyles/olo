//! Agent Screens in Rex's Activity sidebar and embedded [`ScreenPane`] support.
//!
//! Covers:
//! - Resolving which live Rex agent (top-level tab pane, child, or grandchild)
//!   controls each host screen and desktop screen (design §5: `owner` ->
//!   `started_by` -> standalone running row at depth 0, omitting unowned
//!   stopped screens and archived screens).
//! - Formatting two-line screen rows, collapsed rollup counts, neutral-slate
//!   16 pt `agent-screens` icon painting, and width-driven subtitle fitting.
//! - Direct [`HubStreamOpener`] implementing [`olo_screen_pane::StreamOpener`]
//!   over [`crate::screens_sock::ScreensService`] (QUIC priority `-1` with SSH
//!   fallback) without a Unix-socket hop.
//! - Deterministic demo fixtures (`OLO_REX_DEMO_ACTIVITY=screens*`) and
//!   [`DemoPaneSource`] for Early and Built visual verification.

use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    net::Shutdown,
    os::unix::net::UnixStream as StdUnixStream,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use olo_kit::{
    Bounds, Corners, FontWeight, Hsla, Painter, Pixels, RenderImage, icons, point, px, size,
    style::{self, c},
    theme,
};
use olo_protocol::SyntheticEvent;
use olo_rex_link::screen_wire::{HostScreenState, OwnerPane, ScreenOwner};
use olo_screen_pane::{
    CommandOutput, Frame, FrameIndex, FrameMark, HistoryEvent, HistoryHandle, HostCapabilities, LiveEvent, LiveHandle, LiveStatus, LiveStream, NoopHandle, PaneCapabilities, PasswordsMode, ScreenId,
    ScreenInfo, ScreenLocation, ScreenSource, ScreenState, Shot, StreamOpener, export, history::Action, rex_sock::RexScreen,
};
use olo_sync::Mutex;
use smallvec::SmallVec;

use crate::{
    hub::BlockKey,
    screens_sock::{DirectRelayKind, LiveHostScreens, ScreensService},
};

/// A screen row resolved for display in Rex's Activity sidebar.
#[derive(Clone, Debug)]
pub struct ActivityScreenItem {
    /// Source-neutral screen identity (`Local` or `Host`).
    pub id: ScreenId,
    /// Host key where this screen is running (`"local"` for desktop screens).
    pub run_host_key: String,
    /// Monotonic host instance ID captured when this row was resolved (`0` for
    /// desktop screens), pinned across confirmation dialogs and openers (Astra #3).
    pub host_instance_id: u64,
    /// Monotonic remote screens connection ID captured when this row was resolved
    /// (`0` for desktop screens), verified before detaching control on Stop (Astra #9).
    pub conn_id: u64,
    /// Display label of the host where this screen is running.
    pub run_host_label: String,
    /// Host section in Activity where this row is displayed (the owner's host
    /// when controlled by a live agent, else `run_host_key`).
    pub section_host_key: String,
    /// Display title (`title` if set, else `name`).
    pub display_title: String,
    /// Raw optional title from metadata.
    pub title: Option<String>,
    pub state: ScreenState,
    pub size: Option<(i32, i32)>,
    pub scale: f64,
    pub sound: bool,
    /// Running session PID (used to verify remote Live handshake identity, Astra #3).
    pub pid: Option<i32>,
    /// Running session start time in unix seconds (for `--expected-started`).
    pub started: Option<u64>,
    /// Last action timestamp in unix milliseconds.
    pub last_used_ms: Option<u64>,
    pub actions: Option<usize>,
    /// Whether the host/local lock currently reports someone controlling it.
    pub controlled: bool,
    /// Whether the host running this screen is currently disconnected.
    pub host_disconnected: bool,
    /// Timestamp when the remote host disconnected, if disconnected.
    pub disconnected_at_ms: Option<u64>,
    /// Capabilities of the host/source for this screen.
    pub capabilities: PaneCapabilities,
    /// Live controlling pane in `HubState`, if any (`None` => standalone).
    pub owner_bk: Option<BlockKey>,
    /// Tree depth when rendered (`0` for standalone, `1..=4` under an agent).
    pub depth: usize,
    /// Whether this row is the last visible sibling under its parent.
    pub is_last: bool,
    /// Ancestor continuation guide flags for `0..depth-1`.
    pub guides: Vec<bool>,
}

impl ActivityScreenItem {
    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    pub fn is_standalone(&self) -> bool {
        self.owner_bk.is_none()
    }

    /// Build a [`ScreenInfo`] snapshot suitable for [`olo_screen_pane::ScreenPane`].
    pub fn to_screen_info(&self, show_location: bool) -> ScreenInfo {
        let mut caps = self.capabilities.clone();
        caps.host_disconnected = self.host_disconnected;
        caps.show_location = show_location || !self.id.is_local();
        ScreenInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            state: self.state,
            size: self.size,
            scale: self.scale,
            sound: self.sound,
            started_ms: if self.id.is_local() { None } else { self.started.map(|s| s.saturating_mul(1000)) },
            last_used_ms: self.last_used_ms,
            disconnected_at_ms: self.disconnected_at_ms,
            actions: self.actions,
            controlled: self.controlled,
            capabilities: caps,
        }
    }

    /// Whether the row's context menu can offer `Stop Screen…`.
    /// Requires the atomic `stop-expected` capability on remote hosts — never
    /// legacy `stop-expected-started` alone (Astra #2 / Neon #7).
    pub fn can_stop_from_menu(&self) -> Result<(), String> {
        if !self.is_running() {
            return Err("This screen is already stopped.".into());
        }
        if self.host_disconnected {
            return Err(format!("“{}” is disconnected. Reconnect in Rex to stop screens.", self.run_host_label));
        }
        if !self.id.is_local() && !self.capabilities.stop_expected {
            return Err(format!("Update Olo on “{}” to stop a screen from here.", self.run_host_label));
        }
        Ok(())
    }

    /// Whether the row's context menu can offer `Take Control`.
    pub fn can_take_control_from_menu(&self) -> Result<(), String> {
        if !self.is_running() {
            return Err("Start this screen before taking control.".into());
        }
        if self.host_disconnected {
            return Err(format!("“{}” is disconnected. Reconnect in Rex to take control.", self.run_host_label));
        }
        if !self.capabilities.can_control {
            return Err(self.capabilities.control_disabled_reason.clone().unwrap_or_else(|| format!("Update Olo on “{}” to take control.", self.run_host_label)));
        }
        Ok(())
    }
}

/// Resolved placement of all screens shown in Activity.
#[derive(Clone, Debug, Default)]
pub struct ResolvedActivityScreens {
    /// Screens nested under each live controlling pane (running first, most
    /// recently used first, then stopped).
    pub by_owner: HashMap<BlockKey, Vec<ActivityScreenItem>>,
    /// Running unowned screens placed at depth 0 after each host's agents.
    pub standalone_by_host: HashMap<String, Vec<ActivityScreenItem>>,
    /// Running unowned desktop screens when no local Rex server host exists
    /// (`default_local_host_key == None`, e.g. when `"local"` is an SSH home
    /// server or only remote hosts are configured — Astra #19 / Neon #5).
    pub standalone_desktop: Vec<ActivityScreenItem>,
    /// All screens by [`ScreenId`] for quick metadata lookup and pane sync.
    pub by_id: HashMap<ScreenId, ActivityScreenItem>,
}

/// Derive [`PaneCapabilities`] from a connected/disconnected host's [`LiveHostScreens`].
pub fn pane_capabilities_for_host(live: &LiveHostScreens, host_disconnected: bool) -> PaneCapabilities {
    let caps_list = &live.capabilities;
    let has = |names: &[&str]| caps_list.iter().any(|c| names.contains(&c.as_str()));
    let can_control = live.can_control.unwrap_or(false);
    let control_disabled_reason = if can_control { None } else { live.control_disabled_reason.clone().or_else(|| Some("Update Olo on this host to take control.".into())) };
    let effective_disconnected = host_disconnected || live.conn_state != olo_screen_pane::rex_sock::RexHostConn::Connected;
    PaneCapabilities {
        can_move_windows: false,
        can_airplay: false,
        can_control,
        control_disabled_reason,
        can_export_discontiguous: has(&["discontiguous_part", "discontiguous-part"]),
        explicit_control_ack: has(&["control-ack", "control_ack"]),
        warning_status: has(&["warning-status", "warning_status"]),
        history_frame_error: has(&["history-frame-error", "history_frame_error"]),
        history_snapshot_done: has(&["history-snapshot-done", "history_snapshot_done"]),
        stop_expected_started: has(&["stop-expected-started", "stop_expected_started"]),
        stop_expected: has(&["stop-expected", "stop_expected"]),
        clear_expected: has(&["clear-expected", "clear_expected"]),
        delete_expected: has(&["delete-expected", "delete_expected"]),
        host_disconnected: effective_disconnected,
        show_location: true,
        passwords: PasswordsMode::Unavailable("Password sharing for remote host screens isn’t available yet.".into()),
    }
}

/// Derive [`HostCapabilities`] from a host's [`LiveHostScreens`], returning
/// `None` when capabilities have not yet been probed (`proto == 0` and no
/// capability flags) so callers use `HostProtocolCache::new()` instead of
/// freezing an empty legacy cache (Astra #12 / Neon #5).
pub fn host_capabilities_for_live(live: &LiveHostScreens) -> Option<HostCapabilities> {
    let caps_list = &live.capabilities;
    if live.proto == 0 && caps_list.is_empty() && live.commands.is_empty() {
        return None;
    }
    let has = |names: &[&str]| caps_list.iter().any(|c| names.contains(&c.as_str()));
    Some(HostCapabilities {
        proto: live.proto,
        commands: if live.proto > 0 { live.commands.clone() } else { Vec::new() },
        control_ack: has(&["control-ack", "control_ack"]),
        warning_status: has(&["warning-status", "warning_status"]),
        history_frame_error: has(&["history-frame-error", "history_frame_error"]),
        history_snapshot_done: has(&["history-snapshot-done", "history_snapshot_done"]),
        export_part_ranges: has(&["export-part-ranges", "export_part_ranges"]),
        discontiguous_part: has(&["discontiguous_part", "discontiguous-part"]),
        stop_expected_started: has(&["stop-expected-started", "stop_expected_started"]),
        stop_expected: has(&["stop-expected", "stop_expected"]),
        clear_expected: has(&["clear-expected", "clear_expected"]),
        delete_expected: has(&["delete-expected", "delete_expected"]),
    })
}

/// Whether `live` advertises live streaming capability (`stream` command or
/// active follow snapshot) so Host ▸ Screens ▸ `Look at Screen` can gate
/// legacy helpers cleanly (Astra #9).
pub fn host_supports_live_stream(live: &LiveHostScreens) -> bool {
    if live.conn_state != olo_screen_pane::rex_sock::RexHostConn::Connected {
        return false;
    }
    if live.proto >= 1 {
        return live.commands.iter().any(|c| c == "stream");
    }
    live.has_follow_snapshot
}

/// Normalize a unix socket endpoint string (`unix:///path` or `/path`) for exact comparison.
fn normalize_socket_path(raw: &str) -> &str {
    raw.trim().strip_prefix("unix://").unwrap_or(raw.trim())
}

/// Resolve a single [`OwnerPane`] against the set of live Rex panes.
///
/// `forced_host_key`: for remote host screens, strictly binds ownership to that
/// authenticated host key (never trusting a remote host's self-reported `pane.host_key`).
fn resolve_owner_pane(
    pane: &OwnerPane,
    forced_host_key: Option<&str>,
    default_local_host_key: Option<&str>,
    local_host_keys: &HashSet<String>,
    local_rex_socket: Option<&str>,
    owner_rex_socket: Option<&str>,
    live_blocks: &HashSet<BlockKey>,
) -> Option<BlockKey> {
    if pane.block_id.is_empty() {
        return None;
    }
    let host_key = match forced_host_key {
        Some(hk) => hk,
        None => match pane.host_key.as_deref() {
            Some(hk) if !hk.is_empty() => hk,
            _ => default_local_host_key?,
        },
    };
    // Guard local-server socket identity so scratch test servers never cross-contaminate.
    if local_host_keys.contains(host_key)
        && let (Some(want), Some(got)) = (local_rex_socket.filter(|s| !s.is_empty()), owner_rex_socket.filter(|s| !s.is_empty()))
        && normalize_socket_path(want) != normalize_socket_path(got)
    {
        return None;
    }
    if !pane.session_id.is_empty() {
        let exact = BlockKey { host_key: host_key.to_owned(), session_id: pane.session_id.clone(), block_id: pane.block_id.clone() };
        if live_blocks.contains(&exact) {
            return Some(exact);
        }
    }
    // Fallback when session_id was omitted or changed: match uniquely by (host_key, block_id).
    let mut matched: Option<&BlockKey> = None;
    for bk in live_blocks {
        if bk.host_key == host_key && bk.block_id == pane.block_id {
            if matched.is_some() {
                return None;
            }
            matched = Some(bk);
        }
    }
    matched.cloned()
}

fn sort_screen_items(items: &mut [ActivityScreenItem]) {
    items.sort_by(|a, b| {
        let rank = |s: ScreenState| match s {
            ScreenState::Running => 0u8,
            ScreenState::Stopped => 1u8,
            ScreenState::Archived => 2u8,
        };
        rank(a.state).cmp(&rank(b.state)).then_with(|| b.last_used_ms.cmp(&a.last_used_ms)).then_with(|| a.id.name.cmp(&b.id.name))
    });
}

fn rex_state_to_pane(s: HostScreenState) -> ScreenState {
    match s {
        HostScreenState::Running => ScreenState::Running,
        HostScreenState::Stopped => ScreenState::Stopped,
        HostScreenState::Archived => ScreenState::Archived,
    }
}

/// Resolve all local desktop screens and remote host screens into their
/// controlling agents or standalone host buckets (design §5, Brief #1).
///
/// Uses `local_host_keys` (actual `HostRuntime::is_local` hosts) rather than
/// assuming key `"local"` is always a local server so an SSH home server stored
/// under key `"local"` with `is_local == false` still enumerates its host
/// screens cleanly (Astra #15).
#[allow(clippy::too_many_arguments)]
pub fn resolve_activity_screens(
    host_order: &[String],
    local_host_keys: &HashSet<String>,
    host_labels: &HashMap<String, String>,
    host_disconnected: &HashMap<String, bool>,
    local_rex_socket: Option<&str>,
    live_blocks: &HashSet<BlockKey>,
    desktop_screens: &[RexScreen],
    desktop_owners: &HashMap<String, ScreenOwner>,
    host_screens: &HashMap<String, LiveHostScreens>,
) -> ResolvedActivityScreens {
    let mut out = ResolvedActivityScreens::default();
    let default_local_host_key = host_order.iter().find(|k| local_host_keys.contains(*k)).map(String::as_str);
    // When no `is_local` Rex server exists (e.g. `"local"` is a remote SSH home
    // server or only remote hosts are configured), keep standalone desktop
    // screens out of `standalone_by_host["local"]` so they never merge into a
    // remote host or disappear (Astra #19 / Neon #5).
    let desktop_section_key = default_local_host_key.unwrap_or("").to_owned();
    let desktop_label = default_local_host_key.and_then(|k| host_labels.get(k)).cloned().unwrap_or_else(|| "This computer".to_owned());

    // 1. Desktop screens (can be owned by a local agent, a remote host agent
    //    via the bridge, or standalone on this computer).
    for sc in desktop_screens {
        let state = rex_state_to_pane(sc.state);
        if state == ScreenState::Archived {
            continue;
        }
        let owner_rec = desktop_owners.get(&sc.name);
        let owner_socket = owner_rec.and_then(|o| o.rex_socket.as_deref());
        let rec_owner_pane = owner_rec.map(|o| OwnerPane { host_key: o.host_key.clone(), session_id: o.session_id.clone(), block_id: o.block_id.clone() });
        let owner_pane = rec_owner_pane.as_ref().or(sc.owner.as_ref());
        let started_by = owner_rec.and_then(|o| o.started_by.as_ref()).or(sc.started_by.as_ref());

        let resolved_owner = owner_pane
            .and_then(|p| resolve_owner_pane(p, None, default_local_host_key, local_host_keys, local_rex_socket, owner_socket, live_blocks))
            .or_else(|| started_by.and_then(|p| resolve_owner_pane(p, None, default_local_host_key, local_host_keys, local_rex_socket, owner_socket, live_blocks)));

        if resolved_owner.is_none() && state != ScreenState::Running {
            // Stopped unowned screens are not shown in Activity (they belong in Agent Screens).
            continue;
        }

        let section_host_key = resolved_owner.as_ref().map_or_else(|| desktop_section_key.clone(), |bk| bk.host_key.clone());
        let display_title = sc.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&sc.name).to_owned();
        let size = (sc.width > 0 && sc.height > 0).then_some((sc.width as i32, sc.height as i32));
        let item = ActivityScreenItem {
            id: ScreenId::local(&sc.name),
            run_host_key: desktop_section_key.clone(),
            host_instance_id: 0,
            conn_id: 0,
            run_host_label: desktop_label.clone(),
            section_host_key: section_host_key.clone(),
            display_title,
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
            capabilities: PaneCapabilities::local(&sc.name),
            owner_bk: resolved_owner.clone(),
            depth: 0,
            is_last: false,
            guides: Vec::new(),
        };
        out.by_id.insert(item.id.clone(), item.clone());
        if let Some(bk) = resolved_owner {
            out.by_owner.entry(bk).or_default().push(item);
        } else if let Some(local_hk) = default_local_host_key {
            out.standalone_by_host.entry(local_hk.to_owned()).or_default().push(item);
        } else {
            out.standalone_desktop.push(item);
        }
    }

    // 2. Remote host screens (in canonical host_order, skipping ONLY actual `is_local` hosts).
    for hk in host_order {
        if local_host_keys.contains(hk) {
            continue;
        }
        let Some(live) = host_screens.get(hk) else { continue };
        let hlabel = host_labels.get(hk).cloned().unwrap_or_else(|| hk.clone());
        let disconnected = host_disconnected.get(hk).copied().unwrap_or(false) || live.conn_state != olo_screen_pane::rex_sock::RexHostConn::Connected;
        let caps = pane_capabilities_for_host(live, disconnected);
        for sc in &live.screens {
            let state = rex_state_to_pane(sc.state);
            if state == ScreenState::Archived {
                continue;
            }
            // Security: strictly bind remote host screen ownership to `hk`.
            let resolved_owner = sc
                .owner
                .as_ref()
                .and_then(|p| resolve_owner_pane(p, Some(hk), default_local_host_key, local_host_keys, None, None, live_blocks))
                .or_else(|| sc.started_by.as_ref().and_then(|p| resolve_owner_pane(p, Some(hk), default_local_host_key, local_host_keys, None, None, live_blocks)));

            if resolved_owner.is_none() && state != ScreenState::Running {
                continue;
            }

            let display_title = sc.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&sc.name).to_owned();
            let size = (sc.width > 0 && sc.height > 0).then_some((sc.width as i32, sc.height as i32));
            let item = ActivityScreenItem {
                id: ScreenId::host(hk, &hlabel, &sc.name),
                run_host_key: hk.clone(),
                host_instance_id: live.host_instance_id,
                conn_id: live.conn_id,
                run_host_label: hlabel.clone(),
                section_host_key: hk.clone(),
                display_title,
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
                capabilities: caps.clone(),
                owner_bk: resolved_owner.clone(),
                depth: 0,
                is_last: false,
                guides: Vec::new(),
            };
            out.by_id.insert(item.id.clone(), item.clone());
            if let Some(bk) = resolved_owner {
                out.by_owner.entry(bk).or_default().push(item);
            } else {
                out.standalone_by_host.entry(hk.clone()).or_default().push(item);
            }
        }
    }

    for list in out.by_owner.values_mut() {
        sort_screen_items(list);
    }
    for list in out.standalone_by_host.values_mut() {
        sort_screen_items(list);
    }
    sort_screen_items(&mut out.standalone_desktop);
    out
}

/// Build the ordered segments of a screen row's second line (Opus #4 / Neon-look #3).
///
/// - When the person holds control (`you_control == true`): `You have control`.
/// - Uniform format on EVERY screen (running, stopped, standalone, or cross-host):
///   `W × H · age` (with `"On this computer"` moved to the tooltip & pane detail
///   subtitle, and `"Standalone"` dropped since depth-0 placement already conveys it).
pub fn screen_subtitle_segments(item: &ActivityScreenItem, you_control: bool, age_override: Option<String>) -> Vec<String> {
    if you_control && item.is_running() && !item.host_disconnected {
        return vec![olo_kit::words::tr("You have control").to_owned()];
    }
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if let Some((w, h)) = item.size
        && w > 0
        && h > 0
    {
        parts.push(format!("{w} × {h}"));
    }
    let age = age_override.or_else(|| item.last_used_ms.map(|t| olo_kit::ago::ago_ms(t, olo_kit::ago::short)));
    if let Some(a) = age
        && !a.is_empty()
    {
        parts.push(a);
    }
    parts
}

/// Fit subtitle segments into `room` logical points by dropping lowest-priority
/// trailing segments whole (never cutting mid-word on `· 1440 × 900`), and only
/// ellipsizing an ultra-long leading segment if it cannot fit alone.
pub fn fit_screen_subtitle(parts: &[String], room: f32, size_pt: f32) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let font = style::ui_font(FontWeight::NORMAL);
    let measure = |s: &str| f32::from(style::text_width(s.to_owned(), size_pt, font.clone()));
    let full = parts.join(" · ");
    if room <= 24.0 || measure(&full) <= room {
        return full;
    }
    // Drop trailing segments whole from right to left.
    for len in (1..parts.len()).rev() {
        let cand = parts[..len].join(" · ");
        if measure(&cand) <= room {
            return cand;
        }
    }
    // Single remaining segment: clean ellipsis fit.
    let single = &parts[0];
    let ell = "…";
    let mut best = String::new();
    for ch in single.chars() {
        let mut next = best.clone();
        next.push(ch);
        if measure(&format!("{next}{ell}")) > room {
            break;
        }
        best = next;
    }
    if best.is_empty() { ell.to_owned() } else { format!("{best}{ell}") }
}

/// Format the collapsed rollup status text for an agent row that may hold
/// subagents, screens, or both (Brief #4, Opus #6).
///
/// Returns `(label, urgency)` where `urgency` is `2` for failed, `1` for
/// waiting, and `0` for neutral counts.
pub fn format_collapsed_rollup(subagents: usize, screens: usize, failed: usize, waiting: usize) -> Option<(String, u8)> {
    if failed > 0 && waiting > 0 {
        Some((format!("{waiting} waiting · {failed} failed"), 2))
    } else if failed > 0 {
        Some((format!("{failed} failed"), 2))
    } else if waiting > 0 {
        Some((format!("{waiting} waiting"), 1))
    } else {
        match (subagents, screens) {
            (0, 0) => None,
            (s, 0) => Some((format!("{s} subagent{}", if s == 1 { "" } else { "s" }), 0)),
            (0, sc) => Some((format!("{sc} screen{}", if sc == 1 { "" } else { "s" }), 0)),
            (s, sc) => Some((format!("{s} subagent{} · {sc} screen{}", if s == 1 { "" } else { "s" }, if sc == 1 { "" } else { "s" }), 0)),
        }
    }
}

/// Fit the collapsed line-2 rollup with the agent's harness prefix (`claude · 1 subagent · 4 screens`),
/// dropping truncated activity text and falling back to `claude · N items` when tight (Opus #6).
pub fn fit_collapsed_rollup_line(prefix: Option<&str>, subagents: usize, screens: usize, failed: usize, waiting: usize, room: f32, size_pt: f32) -> Option<(Option<String>, String, u8)> {
    let (rollup, urg) = format_collapsed_rollup(subagents, screens, failed, waiting)?;
    let pfx = prefix.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    if urg == 0 && subagents > 0 && screens > 0 && room > 24.0 {
        let font = style::ui_font(FontWeight::NORMAL);
        let full = match &pfx {
            Some(p) => format!("{p} · {rollup}"),
            None => rollup.clone(),
        };
        if f32::from(style::text_width(full, size_pt, font)) > room {
            let total = subagents + screens;
            return Some((pfx, format!("{total} item{}", if total == 1 { "" } else { "s" }), 0));
        }
    }
    Some((pfx, rollup, urg))
}

/// Paint the kit's 16 pt `agent-screens` icon matching `ScreenPane`'s header
/// ("desktop in a monitor" glyph):
/// - **Running & connected:** cool neutral-slate monitor housing with its **blue
///   desktop & window pane lit** (distinct from the dark-screen agent CRT), plus
///   a 6 pt rimmed dot (green when watching, focus-blue when `controlling`).
/// - **Stopped / disconnected:** unlit neutral-slate monitor at strong contrast
///   (`0.88` opacity, never washed out at 1×–2× light mode and always stronger
///   than dimmed context ancestors); hollow grey ring when disconnected, no dot
///   when stopped.
pub fn paint_neutral_screen_icon(g: &mut Painter, b: Bounds<Pixels>, ground: Hsla, running: bool, disconnected: bool, controlling: bool) {
    let p = theme::palette();
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let scale = g.scale_factor();
    let logical = f32::from(b.size.width.max(b.size.height));
    let px_size = (logical * scale).round().max(1.0) as u32;
    let theme_id = olo_xdg::themes::current().id;
    let lit = running && !disconnected;

    if let Some(img) = screen_list_bitmap(theme_id, dark, lit, px_size) {
        let s = img.size(0);
        let (w, h) = (s.width.0 as f32 / scale, s.height.0 as f32 / scale);
        let center = b.center();
        let x = px(((f32::from(center.x) - w / 2.0) * scale).round() / scale);
        let y = px(((f32::from(center.y) - h / 2.0) * scale).round() / scale);
        let at = Bounds { origin: point(x, y), size: size(px(w), px(h)) };

        if !lit {
            g.with_element_opacity(Some(0.88), |g| {
                let _ = g.paint_image(at, Corners::default(), img, 0, false);
            });
        } else {
            let _ = g.paint_image(at, Corners::default(), img, 0, false);
        }
    } else {
        // Fallback while the icon worker renders `agent-screens` (or if no data dir):
        // flat widescreen monitor with lit blue desktop when running, unlit slate when stopped.
        let slate = if dark { style::c([0x8c, 0x9d, 0xb3, 0xff]) } else { style::c([0x4c, 0x5c, 0x72, 0xff]) };
        let screen_fill = if lit {
            style::c([0x2d, 0x6f, 0xc4, 0xff])
        } else if dark {
            style::c([0x32, 0x3b, 0x47, 0xff])
        } else {
            style::c([0x9a, 0xa7, 0xb8, 0xff])
        };
        let draw_fallback = |g: &mut Painter| {
            let frame = style::snap_centred(g, point(b.center().x, b.center().y - px(1.0)), 14.0, 10.0);
            g.paint_quad(olo_kit::fill(frame, screen_fill));
            g.paint_quad(olo_kit::outline(frame, slate, olo_kit::BorderStyle::Solid));
            let stand = style::snap_centred(g, point(b.center().x, frame.bottom() + px(2.0)), 6.0, 1.5);
            g.paint_quad(olo_kit::fill(stand, slate));
        };
        if !lit {
            g.with_element_opacity(Some(0.88), draw_fallback);
        } else {
            draw_fallback(g);
        }
        g.paint_unfinished();
    }

    // Status mark on the top-right corner of the icon box, matching Activity's
    // `agent_marks` geometry: rimmed in `ground` for running/disconnected;
    // when controlling, use a crisp high-contrast light/dark halo + saturated
    // focus-blue dot so it never blends into the lit-blue icon or blue selection
    // fill (Neon-look #4 / Opus #5).
    let mark_at = point(b.right() - px(3.0), b.top() + px(3.0));
    if running && !disconnected {
        if controlling {
            let rim = style::snap_centred(g, mark_at, 9.0, 9.0);
            let halo = if dark { style::c([0x12, 0x18, 0x22, 0xff]) } else { style::c([0xff, 0xff, 0xff, 0xff]) };
            g.paint_quad(olo_kit::fill(rim, halo).corner_radii(rim.size.width / 2.0));
            let dot_color = if dark { style::c([0x7c, 0xc4, 0xff, 0xff]) } else { style::c([0x0b, 0x4f, 0xb8, 0xff]) };
            olo_screen_pane::pane::paint_status_dot(g, mark_at, 6.0, dot_color);
        } else {
            let rim = style::snap_centred(g, mark_at, 8.0, 8.0);
            g.paint_quad(olo_kit::fill(rim, ground).corner_radii(rim.size.width / 2.0));
            olo_screen_pane::pane::paint_status_dot(g, mark_at, 6.0, c(p.success));
        }
    } else if disconnected {
        let rim = style::snap_centred(g, mark_at, 8.0, 8.0);
        g.paint_quad(olo_kit::fill(rim, ground).corner_radii(rim.size.width / 2.0));
        olo_screen_pane::pane::paint_status_ring(g, mark_at, 6.0, style::secondary_text(&p));
    }
}

/// Derive the 16 pt list bitmap from `agent-screens` (1:1 device pixels, cached
/// per `(theme_id, dark, lit, px_size)`):
/// - `lit == true` (running): keeps the blue desktop pane and inner bright
///   window crisp and lit (matching `ScreenPane`'s header icon) while cooling
///   the outer monitor housing to neutral slate (no beige or stray green).
/// - `lit == false` (stopped/disconnected): renders the full monitor in a crisp,
///   high-contrast unlit neutral slate ramp.
type ScreenBitmapKey = (&'static str, bool, bool, u32);
type ScreenBitmapCache = HashMap<ScreenBitmapKey, Arc<RenderImage>>;

fn screen_list_bitmap(theme_id: &'static str, dark: bool, lit: bool, px_size: u32) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<Mutex<ScreenBitmapCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (theme_id, dark, lit, px_size);
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(hit.clone());
    }
    let src = icons::bitmap("agent-screens", px_size)?;
    let sz = src.size(0);
    let (w, h) = (sz.width.0.max(1) as u32, sz.height.0.max(1) as u32);
    let raw = src.as_bytes(0)?;
    if raw.len() < (w * h * 4) as usize {
        return None;
    }
    // Crisp neutral slate ramp (higher contrast in light mode so stopped screens stay clear at 1×–2×):
    let (lo, mid, hi): ([f32; 3], [f32; 3], [f32; 3]) = if dark {
        ([0x1c as f32, 0x23 as f32, 0x2d as f32], [0x88 as f32, 0x99 as f32, 0xb0 as f32], [0xda as f32, 0xe2 as f32, 0xec as f32])
    } else {
        ([0x22 as f32, 0x2b as f32, 0x36 as f32], [0x54 as f32, 0x64 as f32, 0x7a as f32], [0xc2 as f32, 0xcc as f32, 0xd8 as f32])
    };
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (idx, (src_px, dst_px)) in raw.as_chunks::<4>().0.iter().zip(out.as_chunks_mut::<4>().0.iter_mut()).enumerate() {
        // `RenderImage` frames from `icons` are BGRA (straight alpha).
        let (b, g, r, a) = (src_px[0] as f32, src_px[1] as f32, src_px[2] as f32, src_px[3]);
        if a == 0 {
            continue;
        }
        let px_x = (idx as u32) % w;
        let px_y = (idx as u32) / w;
        // Detect lit desktop screen interior (blue desktop, white window, yellow tab)
        // when `lit == true` so the monitor clearly shows an active desktop matching
        // the `ScreenPane` header icon while keeping the casing cool slate.
        let in_screen_box = px_x * 10 >= w * 2 && px_x * 10 <= w * 8 && px_y * 10 >= h * 2 && px_y * 10 <= h * 7;
        let is_blue_desktop = b > r + 10.0 && b > 60.0;
        let is_yellow_tab = r > 170.0 && g > 130.0 && b < 90.0 && in_screen_box;
        let is_white_window = r > 195.0 && g > 195.0 && b > 190.0 && in_screen_box;
        let is_green_cursor = g > r + 15.0 && g > b + 10.0 && in_screen_box;

        if lit && (is_blue_desktop || is_yellow_tab || is_white_window) {
            // Preserve the crisp blue desktop + window colours (slightly boost blue clarity at 16 pt).
            let (nr, ng, nb) = if is_blue_desktop { ((r * 0.85).clamp(0.0, 255.0), (g * 0.96).clamp(0.0, 255.0), (b * 1.12).clamp(0.0, 255.0)) } else { (r, g, b) };
            dst_px[0] = nb.round() as u8;
            dst_px[1] = ng.round() as u8;
            dst_px[2] = nr.round() as u8;
            dst_px[3] = a;
            continue;
        }
        if lit && is_green_cursor {
            // Neutralize the tiny green arrow inside the SVG to crisp white so green belongs solely to health dots.
            dst_px[0] = 248;
            dst_px[1] = 244;
            dst_px[2] = 240;
            dst_px[3] = a;
            continue;
        }
        let luma = ((0.299 * r + 0.587 * g + 0.114 * b) / 255.0).clamp(0.0, 1.0);
        // When unlit (stopped), keep the inner glass darker slate so it reads clearly as "powered off".
        let luma = if !lit && (is_blue_desktop || is_white_window || is_yellow_tab) { (luma * 0.65).clamp(0.15, 0.62) } else { luma };
        let rgb = if luma < 0.5 {
            let t = luma * 2.0;
            [lo[0] + (mid[0] - lo[0]) * t, lo[1] + (mid[1] - lo[1]) * t, lo[2] + (mid[2] - lo[2]) * t]
        } else {
            let t = (luma - 0.5) * 2.0;
            [mid[0] + (hi[0] - mid[0]) * t, mid[1] + (hi[1] - mid[1]) * t, mid[2] + (hi[2] - mid[2]) * t]
        };
        dst_px[0] = rgb[2].round().clamp(0.0, 255.0) as u8; // B
        dst_px[1] = rgb[1].round().clamp(0.0, 255.0) as u8; // G
        dst_px[2] = rgb[0].round().clamp(0.0, 255.0) as u8; // R
        dst_px[3] = a;
    }
    let buf = image::RgbaImage::from_raw(w, h, out)?;
    let frame = image::Frame::new(buf);
    let rendered = Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1)));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, rendered.clone());
    Some(rendered)
}

// ---------------------------------------------------------------------------
// Direct HubStreamOpener (`impl olo_screen_pane::StreamOpener`)
// ---------------------------------------------------------------------------

struct ShutdownWrite(StdUnixStream);

impl Write for ShutdownWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Drop for ShutdownWrite {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Write);
    }
}

/// [`StreamOpener`] backed directly by Rex's [`ScreensService`] (`HubScreenOpener`),
/// routing live/history/export streams, guarded lifecycle commands, and one-shot
/// commands directly over QUIC priority `-1` (with SSH fallback) without going
/// through Rex's local Unix socket server.
///
/// Pins `(host_key, host_instance_id, conn_id)` (Astra #1, #3), verifies remote
/// `(expected_started, expected_pid)` post-Ready on Live handshakes (Astra #3),
/// routes pane Stop/Clear through the service's guarded lifecycle paths with
/// live protocol capability checks (Astra #1), and carries an event-driven
/// `tokio::sync::Notify` wakeup for instant zero-polling cancellation.
pub struct HubStreamOpener {
    service: Arc<ScreensService>,
    rt: tokio::runtime::Handle,
    host_key: String,
    host_instance_id: u64,
    conn_id: Option<u64>,
    expected_started: Option<u64>,
    expected_pid: Option<i32>,
    cancel_notify: Arc<tokio::sync::Notify>,
}

impl HubStreamOpener {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        service: Arc<ScreensService>,
        rt: tokio::runtime::Handle,
        host_key: impl Into<String>,
        host_instance_id: u64,
        conn_id: Option<u64>,
        expected_started: Option<u64>,
        expected_pid: Option<i32>,
    ) -> Self {
        Self { service, rt, host_key: host_key.into(), host_instance_id, conn_id, expected_started, expected_pid, cancel_notify: Arc::new(tokio::sync::Notify::new()) }
    }

    fn open_relay(&self, name: &str, kind: DirectRelayKind, expected_started: Option<u64>, expected_pid: Option<i32>, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        let stream =
            self.rt.block_on(self.service.open_direct_relay(&self.host_key, self.host_instance_id, self.conn_id, name, kind, expected_started, expected_pid, cancelled, self.cancel_notify.clone()))?;
        let writer_sock = stream.try_clone().map_err(|e| format!("couldn’t clone screen stream: {e}"))?;
        let cancel_sock = stream.try_clone().map_err(|e| format!("couldn’t clone screen stream cancel handle: {e}"))?;
        Ok(LiveStream::with_cancel(stream, ShutdownWrite(writer_sock), move || {
            let _ = cancel_sock.shutdown(Shutdown::Both);
        }))
    }
}

impl StreamOpener for HubStreamOpener {
    fn open_live(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.open_relay(name, DirectRelayKind::Live, self.expected_started, self.expected_pid, cancelled)
    }

    fn open_history(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.open_relay(name, DirectRelayKind::History, None, None, cancelled)
    }

    fn open_export(&self, name: &str, args: &[String], cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.open_relay(name, DirectRelayKind::Export(args.to_vec()), None, None, cancelled)
    }

    fn run_command(&self, args: &[String], cancelled: Arc<AtomicBool>) -> Result<CommandOutput, String> {
        self.rt.block_on(self.service.run_direct_command(&self.host_key, self.host_instance_id, self.conn_id, args, cancelled, self.cancel_notify.clone()))
    }

    fn stop_screen_guarded(&self, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Option<Result<(), String>> {
        Some(self.rt.block_on(self.service.stop_screen_for_host_instance_expected_pinned(&self.host_key, self.host_instance_id, self.conn_id, name, expected_started, expected_last_used)))
    }

    fn clear_history_guarded(&self, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Option<Result<(), String>> {
        Some(self.rt.block_on(self.service.clear_history_for_host_instance_expected(&self.host_key, self.host_instance_id, self.conn_id, name, expected_started, expected_last_used)))
    }

    fn notify_cancel(&self) {
        self.cancel_notify.notify_waiters();
    }
}

/// Launch the standalone Agent Screens app focused on `id` (`olo-screen watch [HOST:]NAME`).
/// Reaps the spawned child via `olo_xdg::reap_child` so forwarded exits never
/// leave `<defunct>` zombies behind, and returns any spawn error (Astra #24 / Neon #11).
pub fn open_in_agent_screens(id: &ScreenId) -> Result<(), String> {
    if crate::server::demo_mode() {
        return Ok(());
    }
    let target = match &id.location {
        ScreenLocation::Local => id.name.clone(),
        ScreenLocation::Host { host_id, .. } => format!("{host_id}:{}", id.name),
    };
    let exe = std::env::current_exe().ok().map(|p| p.with_file_name("olo-screen")).filter(|p| p.is_file()).unwrap_or_else(|| std::path::PathBuf::from("olo-screen"));
    let child = Command::new(exe).args(["watch", &target]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Couldn’t launch Agent Screens: {e}"))?;
    olo_xdg::reap_child(child);
    Ok(())
}

// ---------------------------------------------------------------------------
// Deterministic Demo Fixtures (`OLO_REX_DEMO_ACTIVITY=screens*`)
// ---------------------------------------------------------------------------

/// Fixed base timestamp for demo screens so relative times are organic and
/// deterministic across every tab, sidebar row, and screenshot.
pub fn demo_now_ms() -> u64 {
    static NOW: OnceLock<u64> = OnceLock::new();
    *NOW.get_or_init(|| SystemTime::now().duration_since(UNIX_EPOCH).map_or(1_795_800_000_000, |d| d.as_millis() as u64))
}

/// Whether `OLO_REX_DEMO_ACTIVITY` requests the `screens` demo suite (excluding `screens-empty`).
pub fn demo_screens_mode() -> Option<String> {
    let raw = std::env::var("OLO_REX_DEMO_ACTIVITY").ok()?;
    let m = raw.trim();
    if m == "screens" || m.starts_with("screens-") { Some(m.to_owned()) } else { None }
}

struct DemoLiveHandle {
    drops: Arc<AtomicU64>,
}

impl LiveHandle for DemoLiveHandle {
    fn stop(&mut self) {}
}

impl Drop for DemoLiveHandle {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

/// Deterministic demo [`ScreenSource`] backed by coherent synthetic live frames
/// and history actions matching the screen's sidebar metadata.
pub struct DemoPaneSource {
    id: ScreenId,
    info: Mutex<ScreenInfo>,
    controlling: AtomicBool,
    actions: Vec<Action>,
    shot: Arc<Shot>,
    stream_drops: Arc<AtomicU64>,
}

impl DemoPaneSource {
    pub fn new(info: ScreenInfo) -> Self {
        let id = info.id.clone();
        let actions = demo_actions_for(&info);
        let shot = Arc::new(demo_shot_for(&info));
        Self { id, info: Mutex::new(info), controlling: AtomicBool::new(false), actions, shot, stream_drops: Arc::new(AtomicU64::new(0)) }
    }
}

impl ScreenSource for DemoPaneSource {
    fn id(&self) -> &ScreenId {
        &self.id
    }

    fn info(&self) -> ScreenInfo {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn watch_live(&self, _modifiers: Vec<u64>, sink: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
        let shot = self.shot.clone();
        let drops = self.stream_drops.clone();
        std::thread::spawn(move || {
            sink(LiveEvent::Status(LiveStatus::Streaming));
            sink(LiveEvent::Rgba(shot));
        });
        Box::new(DemoLiveHandle { drops })
    }

    fn set_controlled(&self, on: bool) -> Result<(), String> {
        self.controlling.store(on, Ordering::Relaxed);
        Ok(())
    }

    fn send_input(&self, _events: Vec<SyntheticEvent>) {}

    fn join_clipboard(&self) -> Option<Box<dyn Send>> {
        None
    }

    fn follow_history(&self, sink: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
        let actions = self.actions.clone();
        let (w, h) = self.info().size.unwrap_or((1440, 900));
        let marks: Vec<FrameMark> = actions.iter().map(|a| FrameMark { at: a.at, key: true }).collect();
        let index = Arc::new(FrameIndex { frames: marks, last_size: Some((w.max(1) as usize, h.max(1) as usize)) });
        std::thread::spawn(move || {
            sink(HistoryEvent::loaded(actions, true, index));
        });
        Box::new(NoopHandle)
    }

    fn frame_at(&self, at_ms: u64) -> Result<Option<(u64, Frame)>, String> {
        let frame = Frame { width: self.shot.size[0], height: self.shot.size[1], rgba: self.shot.rgba.clone() };
        Ok(Some((at_ms, frame)))
    }

    fn export_video(&self, _actions: Option<&[usize]>, _zoom: usize, _out: &Path, _progress: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
        Err("Export isn’t available in demo mode.".into())
    }

    fn start(&self) -> Result<ScreenInfo, String> {
        let mut g = self.info.lock().unwrap_or_else(|e| e.into_inner());
        g.state = ScreenState::Running;
        Ok(g.clone())
    }

    fn stop(&self) -> Result<(), String> {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).state = ScreenState::Stopped;
        Ok(())
    }

    fn rename(&self, title: Option<&str>) -> Result<(), String> {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).title = title.map(str::to_owned);
        Ok(())
    }

    fn clear_history(&self) -> Result<(), String> {
        Ok(())
    }

    fn archive(&self, _on: bool) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        Ok(())
    }
}

fn demo_actions_for(info: &ScreenInfo) -> Vec<Action> {
    let end_ms = info.last_used_ms.unwrap_or_else(demo_now_ms);
    let start_ms = info.started_ms.unwrap_or_else(|| end_ms.saturating_sub(6 * 60_000));
    let step = ((end_ms.saturating_sub(start_ms)) / 6).max(5_000);
    let mk = |i: u64, kind: &str, args: &[&str], ms: u64| Action {
        at: start_ms.saturating_add(i.saturating_mul(step)).min(end_ms),
        ms,
        kind: kind.to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        result: Some(serde_json::json!({ "window": "Checkout QA — Chromium", "point": [640.0, 380.0] })),
        error: None,
    };
    vec![
        mk(0, "start", &["--size", "1440x900"], 410),
        mk(1, "run", &["chromium", "http://localhost:3000/billing"], 890),
        mk(2, "click", &["--name", "Review Invoice"], 48),
        mk(3, "set", &["--name", "Coupon Code", "SAVE20"], 62),
        mk(4, "click", &["--name", "Apply"], 44),
        Action {
            at: end_ms.saturating_sub(35),
            ms: 35,
            kind: "click".to_owned(),
            args: vec!["--name".to_owned(), "Confirm Total".to_owned()],
            result: Some(serde_json::json!({ "window": "Checkout QA — Chromium", "point": [812.0, 524.0] })),
            error: None,
        },
    ]
}

/// Draw a crisp, clean synthetic Olo-styled desktop picture for demo screens
/// (never a blank black box, organic geometry matching the screen's dimensions).
fn demo_shot_for(info: &ScreenInfo) -> Shot {
    let (w, h) = info.size.map(|(w, h)| (w.clamp(320, 1440) as usize, h.clamp(240, 900) as usize)).unwrap_or((1440, 900));
    let mut rgba = vec![0u8; w * h * 4];
    let fill = |buf: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize, col: [u8; 4]| {
        let x1 = x1.min(w);
        let y1 = y1.min(h);
        for y in y0.min(h)..y1 {
            let row = y * w * 4;
            for x in x0.min(w)..x1 {
                let off = row + x * 4;
                buf[off..off + 4].copy_from_slice(&col);
            }
        }
    };
    // Calm BeOS slate-blue desktop wallpaper.
    fill(&mut rgba, 0, 0, w, h, [0x33, 0x66, 0x98, 0xff]);
    // Deskbar strip top-right.
    fill(&mut rgba, w.saturating_sub(156), 0, w, 24, [0xd8, 0xd8, 0xd8, 0xff]);
    fill(&mut rgba, w.saturating_sub(156), 23, w, 24, [0x88, 0x88, 0x88, 0xff]);

    // Centered BeOS window with yellow title tab.
    let wx = w / 10;
    let wy = h / 10;
    let ww = (w * 4) / 5;
    let wh = (h * 4) / 5;
    // Drop shadow
    fill(&mut rgba, wx + 4, wy + 26, wx + ww + 4, wy + wh + 4, [0x1c, 0x3b, 0x5c, 0xff]);
    // Yellow BeOS tab
    fill(&mut rgba, wx, wy, wx + 240, wy + 24, [0xff, 0xcb, 0x00, 0xff]);
    fill(&mut rgba, wx, wy, wx + 240, wy + 1, [0x60, 0x4c, 0x00, 0xff]);
    fill(&mut rgba, wx, wy, wx + 1, wy + 24, [0x60, 0x4c, 0x00, 0xff]);
    fill(&mut rgba, wx + 239, wy, wx + 240, wy + 24, [0x60, 0x4c, 0x00, 0xff]);
    // Window frame & panel background
    fill(&mut rgba, wx, wy + 24, wx + ww, wy + wh, [0xe6, 0xe6, 0xe6, 0xff]);
    fill(&mut rgba, wx, wy + 24, wx + ww, wy + 25, [0x70, 0x70, 0x70, 0xff]);
    fill(&mut rgba, wx, wy + wh - 1, wx + ww, wy + wh, [0x50, 0x50, 0x50, 0xff]);
    fill(&mut rgba, wx, wy + 24, wx + 1, wy + wh, [0x70, 0x70, 0x70, 0xff]);
    fill(&mut rgba, wx + ww - 1, wy + 24, wx + ww, wy + wh, [0x50, 0x50, 0x50, 0xff]);
    // Toolbar strip
    fill(&mut rgba, wx + 1, wy + 25, wx + ww - 1, wy + 61, [0xdc, 0xdc, 0xdc, 0xff]);
    fill(&mut rgba, wx + 1, wy + 60, wx + ww - 1, wy + 61, [0xa8, 0xa8, 0xa8, 0xff]);
    // URL field
    fill(&mut rgba, wx + 92, wy + 33, wx + ww - 92, wy + 53, [0xff, 0xff, 0xff, 0xff]);
    fill(&mut rgba, wx + 92, wy + 33, wx + ww - 92, wy + 34, [0x8c, 0x8c, 0x8c, 0xff]);
    // White document content well
    let cx = wx + 16;
    let cy = wy + 74;
    let cw = ww.saturating_sub(32);
    let ch = wh.saturating_sub(90);
    fill(&mut rgba, cx, cy, cx + cw, cy + ch, [0xfa, 0xfb, 0xfd, 0xff]);
    // Clean schematic UI cards inside document area (header bar, two columns, summary rows)
    fill(&mut rgba, cx + 28, cy + 24, cx + 260, cy + 44, [0x2b, 0x35, 0x44, 0xff]);
    fill(&mut rgba, cx + 28, cy + 54, cx + 420, cy + 66, [0x8c, 0x99, 0xa8, 0xff]);
    // Left summary card
    let card_w = (cw.saturating_sub(80)) * 3 / 5;
    fill(&mut rgba, cx + 28, cy + 88, cx + 28 + card_w, cy + 88 + 240, [0xff, 0xff, 0xff, 0xff]);
    fill(&mut rgba, cx + 28, cy + 88, cx + 28 + card_w, cy + 89, [0xd4, 0xd9, 0xe0, 0xff]);
    fill(&mut rgba, cx + 28, cy + 327, cx + 28 + card_w, cy + 328, [0xd4, 0xd9, 0xe0, 0xff]);
    for row_i in 0..4 {
        let ry = cy + 114 + row_i * 50;
        fill(&mut rgba, cx + 48, ry, cx + 48 + (card_w * 2 / 5), ry + 12, [0x3d, 0x4a, 0x5c, 0xff]);
        fill(&mut rgba, cx + 48, ry + 18, cx + 48 + (card_w / 3), ry + 26, [0x9a, 0xa6, 0xb5, 0xff]);
        fill(&mut rgba, cx + card_w - 72, ry, cx + card_w + 8, ry + 12, [0x2b, 0x35, 0x44, 0xff]);
    }
    // Right action card with green status pill & slate progress bar
    let rx = cx + 52 + card_w;
    let rw = cw.saturating_sub(card_w + 80);
    if rw > 120 {
        fill(&mut rgba, rx, cy + 88, rx + rw, cy + 88 + 240, [0xf3, 0xf5, 0xf8, 0xff]);
        fill(&mut rgba, rx, cy + 88, rx + rw, cy + 89, [0xd4, 0xd9, 0xe0, 0xff]);
        fill(&mut rgba, rx + 20, cy + 114, rx + rw - 20, cy + 128, [0x33, 0x40, 0x52, 0xff]);
        fill(&mut rgba, rx + 20, cy + 148, rx + rw - 20, cy + 160, [0xde, 0xe4, 0xec, 0xff]);
        fill(&mut rgba, rx + 20, cy + 148, rx + 20 + (rw.saturating_sub(40) * 2 / 3), cy + 160, [0x5e, 0x6e, 0x84, 0xff]);
        // Primary action button
        fill(&mut rgba, rx + 20, cy + 258, rx + rw - 20, cy + 294, [0x34, 0x74, 0xc4, 0xff]);
    }
    Shot { size: [w, h], rgba }
}

/// Seed the `OLO_REX_DEMO_ACTIVITY=screens*` demo hosts, agents, minder, and
/// screens directly into [`crate::hub::HubState`].
pub fn seed_demo_screens_state(st: &mut crate::hub::HubState, theme: &olo_rex_vt::TerminalTheme, mode: &str) {
    use crate::{
        agent::AgentActivity,
        hub::{BlockRuntime, HostRuntime},
    };
    use olo_rex_client::{BlockInfo, LayerView, LayoutNode, ProgramStatusRecord, SessionSummary, SessionView, WindowView};

    let now_ms = demo_now_ms();
    let now_s = now_ms / 1000;
    let now_i64 = now_ms as i64;

    let mk_block = |id: &str, host: &str, label: &str| BlockInfo {
        block_id: id.to_owned(),
        label: label.to_owned(),
        creator_name: "com.superlogical.terminal".to_owned(),
        flavor: "com.superlogical.terminal".to_owned(),
        host_id: host.to_owned(),
        owner: None,
    };
    let mk_win = |wid: &str, host: &str, label: &str, bid: &str, btitle: &str| WindowView {
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
            blocks: vec![mk_block(bid, host, btitle)],
        }],
    };
    let mk_rt = |title: &str, pwd: &str, act: AgentActivity, msg: &str| {
        let mut br = BlockRuntime::new(110, 32, theme);
        br.title = title.to_owned();
        br.pwd = pwd.to_owned();
        br.agent_activity = act;
        br.program_statuses = vec![ProgramStatusRecord { state: "working".into(), msg: Some(msg.to_owned()), ..Default::default() }];
        br.data_connected = true;
        br.terminal.vt_write(format!("\x1b[1;35m◆ {title}\x1b[0m\r\n{msg}\r\n").as_bytes());
        Arc::new(Mutex::new(br))
    };

    // 1. Add a shared minder with two minded agents under `local` (`win-1` / `demo-agent`)
    //    so right-clicking the minder shows `Stop Minder…` and `Stop Minder & Its Agents…`.
    let win_minder = mk_win("win-minder", "local", "Minder", "demo-minder", "claude — Minder");
    let win_m_a = mk_win("win-minded-1", "local", "Auth Sweep", "demo-minded-1", "codex — Auth Sweep");
    let win_m_b = mk_win("win-minded-2", "local", "Docs Sync", "demo-minded-2", "agy — Docs Sync");
    if let Some(local) = st.hosts.get_mut("local")
        && let Some(sess) = local.sessions.get_mut("demo-main")
    {
        sess.windows.push(win_minder);
        sess.windows.push(win_m_a);
        sess.windows.push(win_m_b);
    }
    st.blocks.insert(
        BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-minder".into() },
        mk_rt("claude — Minder", "/home/you/olo/rex", AgentActivity::Working, "Watching 2 subagents for approvals"),
    );
    crate::hub::set_helps(vec![("local".into(), "demo-main".into(), "demo-minder".into(), crate::agent::Help { keep_going: true, ..Default::default() })]);
    st.blocks.insert(
        BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-minded-1".into() },
        mk_rt("codex — Auth Sweep", "/home/you/olo/rex", AgentActivity::Working, "Auditing OAuth token refresh paths"),
    );
    st.blocks.insert(
        BlockKey { host_key: "local".into(), session_id: "demo-main".into(), block_id: "demo-minded-2".into() },
        mk_rt("agy — Docs Sync", "/home/you/olo/rex", AgentActivity::Done, "Updated bridge skill examples"),
    );
    let mk_sub = |id: &str,
                  name: &str,
                  hk: &str,
                  sid: &str,
                  wid: &str,
                  bid: &str,
                  pwid: &str,
                  pbid: &str,
                  cli: crate::subagents::AgentCli,
                  model: &str,
                  effort: &str,
                  title: &str,
                  role: Option<&str>,
                  shared_minder: bool,
                  minded_by: Option<&str>| {
        crate::subagents::NativeSubagent {
            id: id.into(),
            name: Some(name.into()),
            host_key: hk.into(),
            session_id: sid.into(),
            window_id: wid.into(),
            block_id: bid.into(),
            parent_window_id: pwid.into(),
            parent_block_id: Some(pbid.into()),
            cli,
            model: model.into(),
            effort: effort.into(),
            title: title.into(),
            role: role.map(str::to_owned),
            cwd: Some("/home/you/olo/rex".into()),
            resume_id: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: crate::subagents::DesktopPolicy::None,
            stream_mode: false,
            started_at: now_i64 - 120_000,
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
            minded_by: minded_by.map(str::to_owned),
            shared_minder,
        }
    };
    st.subagents.register(mk_sub(
        "sa-minder",
        "minder",
        "local",
        "demo-main",
        "win-minder",
        "demo-minder",
        "win-1",
        "demo-agent",
        crate::subagents::AgentCli::Agy,
        "gemini-4-neon",
        "high",
        "Minder",
        Some("Minder"),
        true,
        None,
    ));
    st.subagents.register(mk_sub(
        "sa-minded-1",
        "auth-sweep",
        "local",
        "demo-main",
        "win-minded-1",
        "demo-minded-1",
        "win-minder",
        "demo-minder",
        crate::subagents::AgentCli::Codex,
        "gpt-5.4-codex",
        "high",
        "Auth Sweep",
        Some("Security Auditor"),
        false,
        Some("sa-minder"),
    ));
    st.subagents.register(mk_sub(
        "sa-minded-2",
        "docs-sync",
        "local",
        "demo-main",
        "win-minded-2",
        "demo-minded-2",
        "win-minder",
        "demo-minder",
        crate::subagents::AgentCli::Claude,
        "claude-opus-5-5",
        "high",
        "Docs Sync",
        Some("Docs Writer"),
        false,
        Some("sa-minder"),
    ));

    // If `screens-empty`, keep the agents/minder list with ZERO screens!
    if mode == "screens-empty" {
        return;
    }

    // 2. Replace `dev-box` session with `Lead` + child `builder-1`.
    let dev_lead_win = mk_win("dev-win-lead", "dev-box", "Lead", "dev-lead", "claude — Lead");
    let dev_b1_win = mk_win("dev-win-b1", "dev-box", "builder-1", "dev-builder-1", "codex — builder-1");
    let dev_sess = SessionView { revision: 1, session_id: "dev-main".to_owned(), label: "work".to_owned(), active_window_id: Some("dev-win-lead".to_owned()), windows: vec![dev_lead_win, dev_b1_win] };
    let dev_inst_id = if let Some(h) = st.hosts.get_mut("dev-box") {
        h.session_summaries = vec![SessionSummary { session_id: "dev-main".into(), label: "work".into() }];
        h.sessions.clear();
        h.sessions.insert("dev-main".into(), dev_sess);
        h.host_instance_id
    } else {
        1
    };
    st.blocks.insert(
        BlockKey { host_key: "dev-box".into(), session_id: "dev-main".into(), block_id: "dev-lead".into() },
        mk_rt("claude — Lead", "/home/you/billing", AgentActivity::Working, "Coordinating checkout verification across host & desktop"),
    );
    st.blocks.insert(
        BlockKey { host_key: "dev-box".into(), session_id: "dev-main".into(), block_id: "dev-builder-1".into() },
        mk_rt("codex — builder-1", "/home/you/billing", AgentActivity::Working, "Verifying invoice totals in Chromium"),
    );
    st.subagents.register(mk_sub(
        "sa-dev-b1",
        "builder-1",
        "dev-box",
        "dev-main",
        "dev-win-b1",
        "dev-builder-1",
        "dev-win-lead",
        "dev-lead",
        crate::subagents::AgentCli::Codex,
        "gpt-5.4-codex",
        "high",
        "builder-1",
        Some("Builder"),
        false,
        None,
    ));

    // 3. Add a third host with a long host name (`staging-eu-west-build-02`) and
    //    an agent `Release QA` controlling four screens (for count & collapsed row).
    let stg_key = "staging";
    let stg_label = "staging-eu-west-build-02";
    let stg_win = mk_win("stg-win-qa", stg_key, "Release QA", "stg-qa", "agy — Release QA");
    let stg_sess = SessionView { revision: 1, session_id: "stg-main".to_owned(), label: "release".to_owned(), active_window_id: Some("stg-win-qa".to_owned()), windows: vec![stg_win] };
    let stg_host = HostRuntime::demo_connected(stg_key, stg_label, "ssh://deploy@staging.example", vec![stg_sess]);
    let stg_inst_id = stg_host.host_instance_id;
    st.hosts.insert(stg_key.to_owned(), stg_host);
    if !st.host_order.iter().any(|k| k == stg_key) {
        st.host_order.push(stg_key.to_owned());
    }
    st.blocks.insert(
        BlockKey { host_key: stg_key.into(), session_id: "stg-main".into(), block_id: "stg-qa".into() },
        mk_rt("agy — Release QA", "/srv/release", AgentActivity::Working, "Running matrix checks across 4 screens"),
    );

    // 4. Populate desktop screens:
    //    - `qa-local` (owned by local `demo-agent`)
    //    - `browser` (desktop screen owned by remote `dev-box` `dev-lead`!)
    //    - `scratch` (standalone running local screen)
    let mk_owner = |hk: &str, sid: &str, bid: &str| OwnerPane { host_key: Some(hk.to_owned()), session_id: sid.to_owned(), block_id: bid.to_owned() };
    let mk_screen = |name: &str, title: Option<&str>, state: HostScreenState, w: u32, h: u32, age_ms: u64, actions: usize, owner: Option<OwnerPane>| RexScreen {
        name: name.to_owned(),
        title: title.map(str::to_owned),
        state,
        width: w,
        height: h,
        scale: 1.0,
        sound: false,
        pid: (state == HostScreenState::Running).then_some(41000),
        started: (state == HostScreenState::Running).then_some(now_s.saturating_sub(6 * 60)),
        last_used: Some(now_ms.saturating_sub(age_ms)),
        actions: Some(actions),
        controlled: false,
        owner,
        started_by: None,
        controlled_by: None,
    };
    st.desktop_screens = vec![
        mk_screen("qa-local", Some("Checkout QA"), HostScreenState::Running, 1440, 900, 2 * 60_000, 14, Some(mk_owner("local", "demo-main", "demo-agent"))),
        mk_screen("browser", Some("Docs Browser"), HostScreenState::Running, 1440, 900, 4 * 60_000, 11, Some(mk_owner("dev-box", "dev-main", "dev-lead"))),
        mk_screen("scratch", None, HostScreenState::Running, 1280, 800, 14 * 60_000, 5, None),
    ];
    st.desktop_screen_owners.clear();
    st.desktop_screen_owners.insert(
        "qa-local".into(),
        ScreenOwner {
            host_key: Some("local".into()),
            session_id: "demo-main".into(),
            block_id: "demo-agent".into(),
            started_by: None,
            pid: Some(41000),
            began: Some(0),
            screen_pid: Some(41000),
            screen_started: Some(now_s.saturating_sub(6 * 60)),
            screen_start_ticks: None,
            rex_socket: None,
            at: now_ms.saturating_sub(2 * 60_000),
        },
    );
    st.desktop_screen_owners.insert(
        "browser".into(),
        ScreenOwner {
            host_key: Some("dev-box".into()),
            session_id: "dev-main".into(),
            block_id: "dev-lead".into(),
            started_by: None,
            pid: Some(41001),
            began: Some(0),
            screen_pid: Some(41001),
            screen_started: Some(now_s.saturating_sub(6 * 60)),
            screen_start_ticks: None,
            rex_socket: None,
            at: now_ms.saturating_sub(4 * 60_000),
        },
    );

    // 5. Populate remote host screens on `dev-box` and `staging`:
    let full_caps: Vec<String> = vec![
        "control-ack".into(),
        "warning-status".into(),
        "history-frame-error".into(),
        "history-snapshot-done".into(),
        "export-part-ranges".into(),
        "discontiguous_part".into(),
        "stop-expected-started".into(),
        "stop-expected".into(),
        "clear-expected".into(),
        "delete-expected".into(),
    ];
    let full_cmds: Vec<String> = vec!["follow".into(), "stream".into(), "history-stream".into(), "export".into()];
    st.host_screens.insert(
        "dev-box".into(),
        LiveHostScreens {
            host_instance_id: dev_inst_id,
            conn_id: 1,
            rev: 1,
            conn_state: olo_screen_pane::rex_sock::RexHostConn::Connected,
            has_follow_snapshot: true,
            status: None,
            can_control: Some(true),
            control_disabled_reason: None,
            proto: 1,
            commands: full_cmds.clone(),
            capabilities: full_caps.clone(),
            disconnected_at: None,
            screens: vec![
                // Long title under Lead on dev-box
                mk_screen("qa-1", Some("Billing Checkout Flow & Invoice Preview"), HostScreenState::Running, 1440, 900, 18_000, 27, Some(mk_owner("dev-box", "dev-main", "dev-lead"))),
                // Stopped screen under live Lead (retains kept 1440 × 900 dimensions for uniform W × H · age subtitle)
                mk_screen("smoke-old", Some("Smoke Pass"), HostScreenState::Stopped, 1440, 900, 38 * 60_000, 9, Some(mk_owner("dev-box", "dev-main", "dev-lead"))),
                // Running screen under child `builder-1` (6 actions matches DemoPaneSource::actions)
                mk_screen("verify-1", Some("Verify Build"), HostScreenState::Running, 1440, 900, 65_000, 6, Some(mk_owner("dev-box", "dev-main", "dev-builder-1"))),
                // Standalone running host screen on dev-box
                mk_screen("keep-demo", None, HostScreenState::Running, 1440, 900, 9 * 60_000, 6, None),
            ],
        },
    );
    st.host_screens.insert(
        stg_key.into(),
        LiveHostScreens {
            host_instance_id: stg_inst_id,
            conn_id: 1,
            rev: 1,
            conn_state: olo_screen_pane::rex_sock::RexHostConn::Connected,
            has_follow_snapshot: true,
            status: None,
            can_control: Some(true),
            control_disabled_reason: None,
            proto: 1,
            commands: full_cmds,
            capabilities: full_caps,
            disconnected_at: None,
            screens: vec![
                mk_screen("login-1", Some("Login Matrix"), HostScreenState::Running, 1440, 900, 2 * 60_000, 12, Some(mk_owner(stg_key, "stg-main", "stg-qa"))),
                mk_screen("cart-2", Some("Cart & Promo"), HostScreenState::Running, 1440, 900, 6 * 60_000, 18, Some(mk_owner(stg_key, "stg-main", "stg-qa"))),
                mk_screen("mobile-3", Some("Narrow Viewport"), HostScreenState::Running, 800, 1200, 11 * 60_000, 8, Some(mk_owner(stg_key, "stg-main", "stg-qa"))),
                mk_screen("i18n-4", Some("Locale Audit"), HostScreenState::Stopped, 1440, 900, 65 * 60_000, 15, Some(mk_owner(stg_key, "stg-main", "stg-qa"))),
            ],
        },
    );
    st.screens_rev = st.screens_rev.wrapping_add(1);
    st.bump_topology();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_bk(host: &str, sess: &str, block: &str) -> BlockKey {
        BlockKey { host_key: host.to_owned(), session_id: sess.to_owned(), block_id: block.to_owned() }
    }

    fn mk_pane(host: Option<&str>, sess: &str, block: &str) -> OwnerPane {
        OwnerPane { host_key: host.map(str::to_owned), session_id: sess.to_owned(), block_id: block.to_owned() }
    }

    fn mk_rex_screen(name: &str, title: Option<&str>, state: HostScreenState, last_used: u64, owner: Option<OwnerPane>, started_by: Option<OwnerPane>) -> RexScreen {
        RexScreen {
            name: name.to_owned(),
            title: title.map(str::to_owned),
            state,
            width: 1440,
            height: 900,
            scale: 1.0,
            sound: false,
            pid: (state == HostScreenState::Running).then_some(1234),
            started: (state == HostScreenState::Running).then_some(1_700_000_000),
            last_used: Some(last_used),
            actions: Some(3),
            controlled: false,
            owner,
            started_by,
            controlled_by: None,
        }
    }

    fn mk_live(screens: Vec<RexScreen>) -> LiveHostScreens {
        LiveHostScreens {
            host_instance_id: 1,
            conn_id: 1,
            rev: 1,
            conn_state: olo_screen_pane::rex_sock::RexHostConn::Connected,
            has_follow_snapshot: true,
            status: None,
            can_control: Some(true),
            control_disabled_reason: None,
            proto: 1,
            commands: vec!["follow".into(), "stream".into(), "history-stream".into(), "export".into()],
            capabilities: vec!["control-ack".into(), "stop-expected".into(), "stop-expected-started".into()],
            disconnected_at: None,
            screens,
        }
    }

    #[test]
    fn owner_resolution_covers_every_rule_and_fallback_of_design_section_5() {
        let bk_local = mk_bk("local", "s1", "b-local");
        let bk_lead = mk_bk("dev-box", "s2", "b-lead");
        let bk_child = mk_bk("dev-box", "s2", "b-child");
        let live_blocks: HashSet<BlockKey> = [bk_local.clone(), bk_lead.clone(), bk_child.clone()].into_iter().collect();

        let host_order = vec!["local".to_owned(), "dev-box".to_owned()];
        let host_labels: HashMap<String, String> = [("local".into(), "This computer".into()), ("dev-box".into(), "dev-box".into())].into_iter().collect();
        let host_disconnected: HashMap<String, bool> = HashMap::new();
        let local_host_keys: HashSet<String> = ["local".to_owned()].into_iter().collect();

        let desktop_screens = vec![
            mk_rex_screen("local-ok", Some("Local QA"), HostScreenState::Running, 500, None, None),
            mk_rex_screen("other-rex", Some("Other Rex"), HostScreenState::Running, 400, None, None),
            mk_rex_screen("desk-for-dev", Some("Docs Browser"), HostScreenState::Running, 600, None, None),
            mk_rex_screen("scratch", None, HostScreenState::Running, 300, None, None),
            mk_rex_screen("old-stopped", Some("Dead"), HostScreenState::Stopped, 200, None, None),
            mk_rex_screen("old-archived", Some("Archived"), HostScreenState::Archived, 100, None, None),
        ];
        let mut desktop_owners: HashMap<String, ScreenOwner> = HashMap::new();
        desktop_owners.insert(
            "local-ok".into(),
            ScreenOwner {
                host_key: None,
                session_id: "s1".into(),
                block_id: "b-local".into(),
                started_by: None,
                pid: None,
                began: None,
                screen_pid: None,
                screen_started: None,
                screen_start_ticks: None,
                rex_socket: Some("/run/user/1000/rex-ours.sock".into()),
                at: 500,
            },
        );
        desktop_owners.insert(
            "other-rex".into(),
            ScreenOwner {
                host_key: None,
                session_id: "s1".into(),
                block_id: "b-local".into(),
                started_by: None,
                pid: None,
                began: None,
                screen_pid: None,
                screen_started: None,
                screen_start_ticks: None,
                rex_socket: Some("/run/user/1000/rex-OTHER.sock".into()),
                at: 400,
            },
        );
        desktop_owners.insert(
            "desk-for-dev".into(),
            ScreenOwner {
                host_key: Some("dev-box".into()),
                session_id: "s2".into(),
                block_id: "b-lead".into(),
                started_by: None,
                pid: None,
                began: None,
                screen_pid: None,
                screen_started: None,
                screen_start_ticks: None,
                rex_socket: None,
                at: 600,
            },
        );

        let mut host_screens: HashMap<String, LiveHostScreens> = HashMap::new();
        host_screens.insert(
            "dev-box".into(),
            mk_live(vec![
                mk_rex_screen("child-scr", Some("Child Screen"), HostScreenState::Running, 700, Some(mk_pane(Some("dev-box"), "s2", "b-child")), Some(mk_pane(Some("dev-box"), "s2", "b-lead"))),
                mk_rex_screen("fallback-scr", Some("Fallback"), HostScreenState::Running, 550, Some(mk_pane(Some("dev-box"), "s2", "b-dead")), Some(mk_pane(Some("dev-box"), "s2", "b-lead"))),
                mk_rex_screen("stopped-owned", Some("Stopped Lead"), HostScreenState::Stopped, 900, Some(mk_pane(Some("dev-box"), "s2", "b-lead")), None),
                mk_rex_screen("standalone-dev", None, HostScreenState::Running, 500, Some(mk_pane(Some("dev-box"), "s2", "b-dead")), Some(mk_pane(Some("dev-box"), "s2", "b-dead-2"))),
                mk_rex_screen("unowned-stopped-dev", None, HostScreenState::Stopped, 800, Some(mk_pane(Some("dev-box"), "s2", "b-dead")), None),
                mk_rex_screen("spoof-local", Some("Spoof"), HostScreenState::Running, 450, Some(mk_pane(Some("local"), "s1", "b-local")), None),
                mk_rex_screen("archived-dev", Some("Archived"), HostScreenState::Archived, 999, Some(mk_pane(Some("dev-box"), "s2", "b-lead")), None),
            ]),
        );

        let res = resolve_activity_screens(
            &host_order,
            &local_host_keys,
            &host_labels,
            &host_disconnected,
            Some("/run/user/1000/rex-ours.sock"),
            &live_blocks,
            &desktop_screens,
            &desktop_owners,
            &host_screens,
        );

        // Local agent owns ONLY `local-ok` (not `other-rex`, not `spoof-local`).
        let local_owned = res.by_owner.get(&bk_local).expect("local owner");
        assert_eq!(local_owned.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["local-ok"]);

        // Local standalone has `other-rex` (400) before `scratch` (300); `old-stopped` & `old-archived` omitted.
        let local_stand = res.standalone_by_host.get("local").expect("local standalone");
        assert_eq!(local_stand.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["other-rex", "scratch"]);

        // `dev-box` Lead owns: running (`desk-for-dev` 600, `fallback-scr` 550) before stopped (`stopped-owned` 900); archived omitted.
        let lead_owned = res.by_owner.get(&bk_lead).expect("lead owner");
        assert_eq!(lead_owned.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["desk-for-dev", "fallback-scr", "stopped-owned"]);
        assert_eq!(lead_owned[0].run_host_key, "local");
        assert_eq!(lead_owned[0].section_host_key, "dev-box");
        assert_eq!(lead_owned[1].run_host_key, "dev-box");
        assert_eq!(lead_owned[1].section_host_key, "dev-box");

        // `dev-box` child owns `child-scr`.
        let child_owned = res.by_owner.get(&bk_child).expect("child owner");
        assert_eq!(child_owned.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["child-scr"]);

        // `dev-box` standalone has `standalone-dev` (500) and `spoof-local` (450); `unowned-stopped-dev` omitted.
        let dev_stand = res.standalone_by_host.get("dev-box").expect("dev standalone");
        assert_eq!(dev_stand.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["standalone-dev", "spoof-local"]);
    }

    #[test]
    fn astra_f15_ssh_home_host_under_local_key_does_not_claim_local_desktop_screens() {
        // When `hosts.local.endpoint` points at an SSH home host, `is_local` is false
        // so `local_host_keys` is empty: local desktop owners with `host_key: None`
        // must NOT attach to that SSH host's pane, and a remote host screen on `"local"`
        // must still be resolved cleanly.
        let bk_ssh_home = mk_bk("local", "s1", "b1");
        let live_blocks: HashSet<BlockKey> = [bk_ssh_home.clone()].into_iter().collect();
        let host_order = vec!["local".to_owned()];
        let host_labels: HashMap<String, String> = [("local".into(), "SSH Home".into())].into_iter().collect();
        let local_host_keys: HashSet<String> = HashSet::new();
        let host_disconnected: HashMap<String, bool> = HashMap::new();

        let desktop_screens = vec![mk_rex_screen("desk-qa", Some("Desk QA"), HostScreenState::Running, 500, None, None)];
        let mut desktop_owners = HashMap::new();
        desktop_owners.insert(
            "desk-qa".into(),
            ScreenOwner {
                host_key: None,
                session_id: "s1".into(),
                block_id: "b1".into(),
                started_by: None,
                pid: None,
                began: None,
                screen_pid: None,
                screen_started: None,
                screen_start_ticks: None,
                rex_socket: None,
                at: 500,
            },
        );
        let mut host_screens = HashMap::new();
        host_screens.insert("local".into(), mk_live(vec![mk_rex_screen("remote-qa", Some("Remote QA"), HostScreenState::Running, 600, Some(mk_pane(Some("local"), "s1", "b1")), None)]));

        let res = resolve_activity_screens(&host_order, &local_host_keys, &host_labels, &host_disconnected, None, &live_blocks, &desktop_screens, &desktop_owners, &host_screens);
        let owned = res.by_owner.get(&bk_ssh_home).expect("ssh home owns its own remote screen");
        assert_eq!(owned.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["remote-qa"]);
        // Astra #19 / Neon #5: standalone desktop screens MUST go into `standalone_desktop`,
        // NEVER merging into the SSH home server's `"local"` host bucket!
        assert!(res.standalone_by_host.get("local").is_none_or(|v| v.iter().all(|s| s.id.name != "desk-qa")));
        assert_eq!(res.standalone_desktop.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["desk-qa"]);
        assert_eq!(res.standalone_desktop[0].run_host_label, "This computer");
        assert_eq!(res.standalone_desktop[0].section_host_key, "");

        // Also when no `"local"` key exists at all (only remote hosts), standalone desktop screens
        // still populate `standalone_desktop` cleanly so Activity renders them under "This computer".
        let res_remote_only = resolve_activity_screens(
            &["dev-box".to_owned()],
            &HashSet::new(),
            &[("dev-box".into(), "dev-box".into())].into_iter().collect(),
            &HashMap::new(),
            None,
            &HashSet::new(),
            &desktop_screens,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(res_remote_only.standalone_desktop.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>(), vec!["desk-qa"]);
    }

    #[test]
    fn astra_f2_f12_capabilities_gating_and_stop_expected_requirement() {
        let mut live = LiveHostScreens::default();
        assert!(host_capabilities_for_live(&live).is_none(), "unprobed host must return None so HostProtocolCache probes live");
        assert!(!host_supports_live_stream(&live), "disconnected host must not advertise live stream");
        live.conn_state = olo_screen_pane::rex_sock::RexHostConn::Connected;
        live.has_follow_snapshot = true;
        assert!(host_supports_live_stream(&live), "connected host with follow snapshot allows stream probe until proven lacking");

        // Probed old helper with proto=0 and no stream command
        live.has_follow_snapshot = false;
        live.capabilities = vec!["stop-expected-started".into()];
        let caps_v0 = host_capabilities_for_live(&live).expect("probed");
        assert!(caps_v0.commands.is_empty(), "proto=0 must not fabricate commands");
        assert!(!host_supports_live_stream(&live), "proto=0 helper without follow must not advertise live stream support");

        // Probed v1 helper with stream + stop-expected-started ONLY (missing atomic stop-expected)
        live.proto = 1;
        live.commands = vec!["follow".into(), "stream".into()];
        live.conn_state = olo_screen_pane::rex_sock::RexHostConn::Connected;
        assert!(host_supports_live_stream(&live));
        let pane_caps = pane_capabilities_for_host(&live, false);
        assert!(!pane_caps.stop_expected);
        assert!(pane_caps.stop_expected_started);

        let item_without_atomic_stop = ActivityScreenItem {
            id: ScreenId::host("dev", "Dev", "qa"),
            run_host_key: "dev".into(),
            run_host_label: "Dev".into(),
            host_instance_id: 1,
            conn_id: 1,
            section_host_key: "dev".into(),
            display_title: "QA".into(),
            title: Some("QA".into()),
            state: ScreenState::Running,
            size: Some((1440, 900)),
            scale: 1.0,
            sound: false,
            pid: Some(1234),
            started: Some(1_700_000_000),
            last_used_ms: Some(1_700_000_000_000),
            actions: Some(2),
            controlled: false,
            host_disconnected: false,
            disconnected_at_ms: None,
            capabilities: pane_caps,
            owner_bk: None,
            depth: 1,
            is_last: true,
            guides: Vec::new(),
        };
        assert!(item_without_atomic_stop.can_stop_from_menu().is_err(), "can_stop_from_menu must require atomic stop-expected (Astra #2 / Neon #7)");
    }

    #[test]
    fn subtitles_follow_uniform_dimensions_and_age_format() {
        let mk_item = |id: ScreenId, run_hk: &str, run_lbl: &str, sec_hk: &str, owner: Option<BlockKey>, state: ScreenState| ActivityScreenItem {
            id,
            run_host_key: run_hk.into(),
            run_host_label: run_lbl.into(),
            host_instance_id: 1,
            conn_id: 1,
            section_host_key: sec_hk.into(),
            display_title: "Test".into(),
            title: Some("Test".into()),
            state,
            size: Some((1440, 900)),
            scale: 1.0,
            sound: false,
            pid: Some(1234),
            started: Some(1_700_000_000),
            last_used_ms: Some(1_700_000_000_000),
            actions: Some(2),
            controlled: false,
            host_disconnected: false,
            disconnected_at_ms: None,
            capabilities: olo_screen_pane::PaneCapabilities::remote_default(),
            owner_bk: owner,
            depth: 1,
            is_last: true,
            guides: Vec::new(),
        };

        // Opus #4: every screen row (owned, cross-host, standalone, stopped with kept size)
        // uses the uniform `W × H · age` subtitle grammar with zero location prefixes.
        let same_host = mk_item(ScreenId::host("dev-box", "dev-box", "qa"), "dev-box", "dev-box", "dev-box", Some(mk_bk("dev-box", "s", "b")), ScreenState::Running);
        assert_eq!(screen_subtitle_segments(&same_host, false, Some("6 min".into())), vec!["1440 × 900", "6 min"]);

        let cross_host = mk_item(ScreenId::local("browser"), "local", "This computer", "dev-box", Some(mk_bk("dev-box", "s", "b")), ScreenState::Running);
        assert_eq!(screen_subtitle_segments(&cross_host, false, Some("4 min".into())), vec!["1440 × 900", "4 min"]);

        let standalone = mk_item(ScreenId::local("scratch"), "local", "This computer", "local", None, ScreenState::Running);
        assert_eq!(screen_subtitle_segments(&standalone, false, Some("14 min".into())), vec!["1440 × 900", "14 min"]);

        let stopped = mk_item(ScreenId::host("dev-box", "dev-box", "old"), "dev-box", "dev-box", "dev-box", Some(mk_bk("dev-box", "s", "b")), ScreenState::Stopped);
        assert_eq!(screen_subtitle_segments(&stopped, false, Some("38 min".into())), vec!["1440 × 900", "38 min"]);

        // Controlled running screen: sentence-case `You have control`!
        assert_eq!(screen_subtitle_segments(&same_host, true, Some("6 min".into())), vec!["You have control"]);
    }

    #[test]
    fn collapsed_rollup_counts_urgency_and_tight_fit() {
        assert_eq!(format_collapsed_rollup(0, 0, 0, 0), None);
        assert_eq!(format_collapsed_rollup(2, 0, 0, 0), Some(("2 subagents".into(), 0)));
        assert_eq!(format_collapsed_rollup(0, 4, 0, 0), Some(("4 screens".into(), 0)));
        assert_eq!(format_collapsed_rollup(2, 1, 0, 0), Some(("2 subagents · 1 screen".into(), 0)));
        assert_eq!(format_collapsed_rollup(1, 4, 0, 0), Some(("1 subagent · 4 screens".into(), 0)));
        assert_eq!(format_collapsed_rollup(2, 3, 0, 1), Some(("1 waiting".into(), 1)));
        assert_eq!(format_collapsed_rollup(2, 3, 1, 0), Some(("1 failed".into(), 2)));
        assert_eq!(format_collapsed_rollup(2, 3, 1, 2), Some(("2 waiting · 1 failed".into(), 2)));

        // Opus #6: wide room keeps `<cli> · 1 subagent · 4 screens`; tight room compacts to `5 items`.
        assert_eq!(fit_collapsed_rollup_line(Some("claude"), 1, 4, 0, 0, 260.0, 11.0), Some((Some("claude".into()), "1 subagent · 4 screens".into(), 0)));
        assert_eq!(fit_collapsed_rollup_line(Some("claude"), 1, 4, 0, 0, 90.0, 11.0), Some((Some("claude".into()), "5 items".into(), 0)));
        // Urgent waiting rollup preserves urgency even when tight.
        assert_eq!(fit_collapsed_rollup_line(Some("claude"), 1, 0, 0, 1, 90.0, 11.0), Some((Some("claude".into()), "1 waiting".into(), 1)));
    }
}
