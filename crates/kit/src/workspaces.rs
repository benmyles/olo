//! Which of this app's windows are on this workspace, the active
//! display's showing ([`Cx::on_this_workspace`](crate::Cx::on_this_workspace)),
//! as app_server says: its windows, each display's workspace and which
//! display is active come with the events theme_watch's thread follows,
//! each of ours known by its surface's object id. Every display has its
//! own workspaces.

use std::{collections::HashMap, sync::Mutex};

use olo_protocol::{Client, DisplayWorkspace, Reply, Request, WindowInfo};

struct Known {
    /// The active display's workspace showing, for a display not listed.
    current: u32,
    /// Each display's (by connector) workspace showing.
    showing: HashMap<String, u32>,
    /// The active display, if app_server said.
    active: Option<String>,
    /// Our windows' surfaces (object ids), with their workspaces and
    /// displays, and whether each is hidden (minimized).
    mine: HashMap<u32, (u32, Option<String>, bool)>,
}

impl Known {
    fn set_showing(&mut self, current: u32, displays: Vec<DisplayWorkspace>) {
        self.current = current;
        self.active = displays.iter().find(|d| d.active).map(|d| d.output.clone());
        self.showing = displays.into_iter().map(|d| (d.output, d.current)).collect();
    }
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// What app_server says now (on connecting to it, again after a restart).
pub(crate) fn catch_up() {
    let Ok(mut c) = Client::connect() else { return };
    let Ok(Reply::Workspaces { current, displays, .. }) = c.request(&Request::Workspaces) else { return };
    let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
    if let Ok(mut k) = KNOWN.lock() {
        let mut known = Known { current, showing: HashMap::new(), active: None, mine: mine(&windows) };
        known.set_showing(current, displays);
        *k = Some(known);
    }
}

/// app_server has gone (restarting): our windows come back with other
/// surfaces, so what was known of them is no longer.
pub(crate) fn forget() {
    if let Ok(mut k) = KNOWN.lock() {
        *k = None;
    }
}

pub(crate) fn windows_changed(windows: &[WindowInfo]) {
    if let Ok(mut k) = KNOWN.lock()
        && let Some(k) = k.as_mut()
    {
        k.mine = mine(windows);
    }
}

pub(crate) fn workspace_changed(current: u32, displays: Vec<DisplayWorkspace>) {
    if let Ok(mut k) = KNOWN.lock()
        && let Some(k) = k.as_mut()
    {
        k.set_showing(current, displays);
    }
}

fn mine(windows: &[WindowInfo]) -> HashMap<u32, (u32, Option<String>, bool)> {
    let pid = std::process::id() as i32;
    // (A tab behind another in its group shows no more than a hidden one.)
    windows.iter().filter(|w| w.pid == Some(pid)).filter_map(|w| Some((w.surface?, (w.workspace, w.output.clone(), w.minimized || w.behind)))).collect()
}

/// Whether our window with this surface is on this workspace: the one
/// the active display shows (a window showing on another display is on
/// another workspace); None if app_server hasn't said (outside Olo, or it
/// has only just opened).
pub(crate) fn here(surface: u32) -> Option<bool> {
    let k = KNOWN.lock().ok()?;
    let k = k.as_ref()?;
    let (ws, output, _) = k.mine.get(&surface)?;
    if let (Some(active), Some(output)) = (&k.active, output)
        && active != output
    {
        return Some(false);
    }
    let showing = output.as_ref().and_then(|o| k.showing.get(o)).copied().unwrap_or(k.current);
    Some(*ws == showing)
}

/// Whether our window with this surface shows: on the workspace its
/// display shows, not hidden and not behind another in its group; None
/// if app_server hasn't said.
pub(crate) fn shown(surface: u32) -> Option<bool> {
    let k = KNOWN.lock().ok()?;
    let k = k.as_ref()?;
    let (ws, output, minimized) = k.mine.get(&surface)?;
    let showing = output.as_ref().and_then(|o| k.showing.get(o)).copied().unwrap_or(k.current);
    Some(*ws == showing && !minimized)
}
