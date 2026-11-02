//! Which of the app's windows are on this workspace (the one the active
//! display shows), as app_server says, for an app that keeps a window per
//! thing on each workspace (Tracker's folder windows, as the egui kit's
//! `Cx::on_this_workspace`). Followed on a thread of its own once an app
//! asks ([`follow`]): app_server sends it our windows (by their
//! surfaces' object ids) only when where they are changes, and each
//! display's workspace as it changes. Every display has its own
//! workspaces.

use olo_sync::Mutex;
use std::{collections::HashMap, time::Duration};

use gpui::{App, Global, Window};
use olo_protocol::{Client, DisplayWorkspace, Event, EventKind, Reply, Request, WindowInfo};

struct Known {
    /// The active display's workspace showing, for a display not listed.
    current: u32,
    /// Each display's (by connector) workspace showing.
    showing: HashMap<String, u32>,
    /// The active display, if app_server said.
    active: Option<String>,
    /// Our windows' surfaces (object ids), with their workspaces and
    /// displays, and whether each is hidden (minimized, or behind another
    /// in its group).
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

struct Following;

impl Global for Following {}

/// Starts following where the app's windows are (once; outside Olo it
/// does nothing, and every window counts as here).
pub fn follow(cx: &mut App) {
    if cx.has_global::<Following>() || olo_protocol::socket_path().is_none() {
        return;
    }
    cx.set_global(Following);
    std::thread::Builder::new()
        .name("workspaces".into())
        .spawn(|| {
            loop {
                if let Ok(client) = Client::connect()
                    && let Ok(events) = client.into_events_of(&[EventKind::Workspace, EventKind::OwnWindows])
                {
                    catch_up();
                    for event in events {
                        match event {
                            Ok(Event::WindowsChanged { windows }) => windows_changed(&windows),
                            Ok(Event::WorkspaceChanged { current, displays, .. }) => workspace_changed(current, displays),
                            _ => {}
                        }
                    }
                    // app_server has gone (restarting): our windows come
                    // back with other surfaces.
                    if let Ok(mut k) = KNOWN.lock() {
                        *k = None;
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}

/// What app_server says now (on connecting to it, again after a restart).
fn catch_up() {
    let Ok(mut c) = Client::connect() else { return };
    let Ok(Reply::Workspaces { current, displays, .. }) = c.request(&Request::Workspaces) else { return };
    let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
    if let Ok(mut k) = KNOWN.lock() {
        let mut known = Known { current, showing: HashMap::new(), active: None, mine: mine(&windows) };
        known.set_showing(current, displays);
        *k = Some(known);
    }
}

fn windows_changed(windows: &[WindowInfo]) {
    if let Ok(mut k) = KNOWN.lock()
        && let Some(k) = k.as_mut()
    {
        k.mine = mine(windows);
    }
}

fn workspace_changed(current: u32, displays: Vec<DisplayWorkspace>) {
    if let Ok(mut k) = KNOWN.lock()
        && let Some(k) = k.as_mut()
    {
        k.set_showing(current, displays);
    }
}

fn mine(windows: &[WindowInfo]) -> HashMap<u32, (u32, Option<String>, bool)> {
    let pid = std::process::id() as i32;
    windows.iter().filter(|w| w.pid == Some(pid)).filter_map(|w| Some((w.surface?, (w.workspace, w.output.clone(), w.minimized || w.behind)))).collect()
}

/// Whether `window` is on this workspace: the one the active display
/// shows (a window showing on another display is on another workspace);
/// None if app_server hasn't said (outside Olo, not followed, or it has
/// only just opened).
pub fn here(window: &Window) -> Option<bool> {
    here_surface(window.surface_id()?)
}

/// [`here`], for the window whose surface is `surface` (as
/// `Window::surface_id` gave it): for asking about a window from outside
/// its update.
pub fn here_surface(surface: u32) -> Option<bool> {
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
