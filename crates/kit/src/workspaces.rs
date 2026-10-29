//! Which of this app's windows are on the workspace showing
//! ([`Cx::on_this_workspace`](crate::Cx::on_this_workspace)), as app_server
//! says: its windows and workspace come with the events theme_watch's
//! thread follows, each of ours known by its surface's object id.

use std::{collections::HashMap, sync::Mutex};

use olo_protocol::{Client, Reply, Request, WindowInfo};

struct Known {
    current: u32,
    /// Our windows' surfaces (object ids), with their workspaces.
    mine: HashMap<u32, u32>,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// What app_server says now (on connecting to it, again after a restart).
pub(crate) fn catch_up() {
    let Ok(mut c) = Client::connect() else { return };
    let Ok(Reply::Workspaces { current, .. }) = c.request(&Request::Workspaces) else { return };
    let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
    if let Ok(mut k) = KNOWN.lock() {
        *k = Some(Known { current, mine: mine(&windows) });
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

pub(crate) fn workspace_changed(current: u32) {
    if let Ok(mut k) = KNOWN.lock()
        && let Some(k) = k.as_mut()
    {
        k.current = current;
    }
}

fn mine(windows: &[WindowInfo]) -> HashMap<u32, u32> {
    let pid = std::process::id() as i32;
    windows.iter().filter(|w| w.pid == Some(pid)).filter_map(|w| Some((w.surface?, w.workspace))).collect()
}

/// Whether our window with this surface is on the workspace showing; None
/// if app_server hasn't said (outside Olo, or it has only just opened).
pub(crate) fn here(surface: u32) -> Option<bool> {
    let k = KNOWN.lock().ok()?;
    let k = k.as_ref()?;
    k.mine.get(&surface).map(|ws| *ws == k.current)
}
