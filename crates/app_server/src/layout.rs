//! Window placement memory, so windows come back where they were when
//! app_server restarts in place (`olo-session reload`) or an app
//! re-executes itself after an upgrade.
//!
//! On exit app_server writes every window's place, and the workspace
//! showing, to `$XDG_RUNTIME_DIR/olo-layout-$WAYLAND_DISPLAY.json`; a
//! closing window is remembered in memory too. A new window that is one of those coming
//! back takes its place and size instead of the next cascade slot:
//!
//! - the same process on a new connection (it reconnected, or re-executed
//!   itself after an upgrade), by app id and title, else app id (titles
//!   change) within a minute;
//! - an app started again after its old process ended (Chrome restarting
//!   restores its windows), by app id and the very same title, within a
//!   quarter of an hour.
//!
//! Anything else is a new window, and opens where new windows open: on
//! the display in use (under the pointer, or beside the app's focused
//! window). That includes a running app opening a window again.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use smithay::{reexports::wayland_server::protocol::wl_surface::WlSurface, utils::Point, wayland::seat::WaylandFocus};
use tracing::{info, warn};

use crate::{
    shell::WindowElement,
    state::{AnvilState, Backend},
};

/// How long a remembered place is offered to a returning window.
const KEEP: Duration = Duration::from_secs(60);
/// How long a closed window's place waits for its app to start again.
const RESTART_KEEP: Duration = Duration::from_secs(15 * 60);
/// At most this many closed windows are remembered.
const MAX_REMEMBERED: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub app_id: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    /// Its content's size (0: not known).
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
    pub workspace: u32,
    /// Tiled by a tiling command (and the part its edges were dragged to).
    #[serde(default)]
    pub tile: Option<olo_protocol::Tile>,
    #[serde(default)]
    pub part: Option<(f32, f32, f32, f32)>,
    pub minimized: bool,
    pub focused: bool,
    /// Rolled up to its tab.
    #[serde(default)]
    pub rolled: bool,
    /// How far its tab is slid along its top (Fan Out, rolled windows
    /// after Tile All).
    #[serde(default)]
    pub tab_offset: i32,
    /// How it groups (taken out of its group or put in one by hand), and
    /// its tab's place among its group's (0: not known).
    #[serde(default)]
    pub grouping: crate::shell::groups::Grouping,
    #[serde(default)]
    pub tab_order: u64,
    /// Its place in the stacking when saved, bottom first from 1 (0: not
    /// known), so windows back in whatever order their clients reconnect
    /// still stack as they did.
    #[serde(default)]
    pub depth: u32,
    /// The workspace showing when it was saved (the same in every place
    /// saved together), so a restart comes back to it.
    #[serde(default)]
    pub showing: Option<u32>,
    /// Seconds since the epoch when it was recorded.
    pub at: u64,
    /// The process that showed it (0: not known, any may take it).
    #[serde(default)]
    pub pid: i32,
    /// Its connection then: a window on the same one isn't coming back.
    #[serde(skip)]
    pub client: Option<smithay::reexports::wayland_server::backend::ClientId>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(olo_xdg::legacy::runtime_file(std::path::Path::new(&dir), &format!("olo-layout-{display}.json")))
}

/// Places saved by the previous app_server, if it exited moments ago.
pub fn load() -> Vec<Placement> {
    let Some(p) = path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    let _ = std::fs::remove_file(&p);
    let mut list: Vec<Placement> = serde_json::from_str(&text).unwrap_or_default();
    // Left by an app_server from before the rename (Tabula).
    for e in &mut list {
        e.app_id = olo_xdg::legacy::new_app_id(&e.app_id);
    }
    let fresh: Vec<Placement> = list.into_iter().filter(|e| now().saturating_sub(e.at) < RESTART_KEEP.as_secs()).collect();
    info!(windows = fresh.len(), "restoring window layout");
    fresh
}

/// The workspace that was showing when `list` was saved (of `count`), to
/// start on; 0 if none was.
pub fn showing(list: &[Placement], count: u32) -> u32 {
    list.iter().find_map(|p| p.showing).unwrap_or(0).min(count.max(1) - 1)
}

impl<B: Backend + 'static> AnvilState<B> {
    fn placement_of(&self, w: &WindowElement, focused: Option<&WlSurface>) -> Option<Placement> {
        let meta = w.meta();
        let app_id = w.app_id();
        if app_id.is_empty() {
            return None;
        }
        // Not shown yet, it is only waiting in a cascade slot: if it is
        // one coming back, its place is the one it claimed (app_server
        // restarting just as an upgraded app reconnects mustn't lose it);
        // else it has none.
        if !meta.placed.get() {
            let p = meta.returning.borrow().clone()?;
            info!(app_id = %p.app_id, x = p.x, y = p.y, "a window coming back, not shown yet: keeping its place");
            let owner = meta.owner.borrow().clone();
            return Some(Placement {
                at: now(),
                pid: owner.as_ref().map_or(p.pid, |(pid, _)| *pid),
                client: owner.map(|(_, c)| c).or(p.client.clone()),
                ..p
            });
        }
        let loc = self.space.element_location(w).unwrap_or_else(|| meta.saved_location.get());
        let size = w.content_size();
        let (tile, part) = Self::tile_state(w);
        Some(Placement {
            tile,
            part,
            app_id: olo_xdg::legacy::new_app_id(&app_id),
            title: w.title(),
            x: loc.x,
            y: loc.y,
            width: size.w,
            height: size.h,
            workspace: meta.workspace.get().unwrap_or(self.workspace),
            minimized: meta.minimized.get(),
            focused: focused.is_some() && w.wl_surface().as_deref() == focused,
            rolled: w.rolled(),
            tab_offset: w.decoration().tab_offset,
            grouping: meta.grouping.borrow().clone(),
            tab_order: meta.tab_order.get(),
            depth: 0,
            showing: None,
            at: now(),
            pid: meta.owner.borrow().as_ref().map_or(0, |(pid, _)| *pid),
            client: meta.owner.borrow().as_ref().map(|(_, c)| c.clone()),
        })
    }

    /// Remember a window that is going away (it may come straight back, as
    /// when an app re-executes itself after an upgrade).
    pub fn remember_placement(&mut self, w: &WindowElement) {
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface().map(|s| s.into_owned()));
        if let Some(p) = self.placement_of(w, focused.as_ref()) {
            self.layout.retain(|e| now().saturating_sub(e.at) < RESTART_KEEP.as_secs());
            if self.layout.len() >= MAX_REMEMBERED {
                self.layout.remove(0);
            }
            self.layout.push(p);
        }
    }

    /// Write every window's place for the next app_server.
    pub fn save_layout(&mut self) {
        let Some(path) = path() else { return };
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface().map(|s| s.into_owned()));
        // Bottom-most first, so restoring in order rebuilds the stacking
        // (one not shown yet keeps the depth it is coming back to).
        let shown = self.space.elements().count();
        let mut list: Vec<Placement> = self
            .all_windows()
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let p = self.placement_of(w, focused.as_ref())?;
                Some(Placement { depth: if i < shown && w.meta().placed.get() { i as u32 + 1 } else { p.depth }, ..p })
            })
            .collect();
        list.extend(self.layout.iter().cloned());
        for p in &mut list {
            p.showing = Some(self.workspace);
        }
        match serde_json::to_string(&list) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    warn!(?e, "could not save window layout");
                } else {
                    info!(windows = list.len(), "saved window layout");
                }
            }
            Err(e) => warn!(?e, "could not encode window layout"),
        }
    }

    /// The remembered place `window` is coming back to, if it is one
    /// coming back (see the top of this file); taken from the list.
    fn take_placement(&mut self, window: &WindowElement) -> Option<Placement> {
        if self.layout.is_empty() {
            return None;
        }
        let (app_id, title) = (window.app_id(), window.title());
        if app_id.is_empty() {
            return None;
        }
        // Places are kept under new app ids: a program from before the
        // rename (Tabula) has its old one until it re-executes.
        let app_id = olo_xdg::legacy::new_app_id(&app_id);
        let owner = window.meta().owner.borrow().clone();
        let age = |e: &Placement| now().saturating_sub(e.at);
        // The same process, reconnected or re-executed.
        let same = |e: &Placement| {
            age(e) < KEEP.as_secs() && owner.as_ref().is_none_or(|(pid, client)| (e.pid == 0 || e.pid == *pid) && e.client.as_ref() != Some(client))
        };
        // The app started again: its old process is gone.
        let restarted = |e: &Placement| {
            age(e) < RESTART_KEEP.as_secs()
                && e.pid > 0
                && owner.as_ref().is_some_and(|(pid, _)| e.pid != *pid)
                && !std::path::Path::new(&format!("/proc/{}", e.pid)).exists()
        };
        // The newest first: the place it had last.
        let i = (0..self.layout.len()).rev().find(|&i| {
            let e = &self.layout[i];
            e.app_id == app_id && (e.title == title && (same(e) || restarted(e)))
        });
        // Titles can change (a terminal's follows its directory).
        let i = i.or_else(|| (0..self.layout.len()).rev().find(|&i| same(&self.layout[i]) && self.layout[i].app_id == app_id))?;
        Some(self.layout.remove(i))
    }

    /// On a window's first commit, before its first configure: if it is
    /// coming back, claim its place and ask for its old size.
    pub fn claim_placement(&mut self, surface: &WlSurface) {
        let Some(window) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else { return };
        if window.meta().placed.get() || window.meta().returning.borrow().is_some() {
            return;
        }
        let Some(p) = self.take_placement(&window) else { return };
        // A fixed-size window (an alert) is the size it asks for now: its
        // text, or the version drawing it, may have changed since.
        if p.width > 0
            && p.height > 0
            && crate::shell::tiling::resizable(&window)
            && let Some(toplevel) = window.0.toplevel()
        {
            toplevel.with_pending_state(|state| state.size = Some((p.width, p.height).into()));
        }
        *window.meta().returning.borrow_mut() = Some(p);
    }

    /// On a window's first showing: put it back where it was, if it is
    /// coming back. True when it isn't, so it is still to be placed.
    pub fn restore_placement(&mut self, surface: &WlSurface) -> bool {
        let Some(window) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else {
            return false;
        };
        let meta = window.meta();
        if meta.placed.replace(true) {
            return false;
        }
        // Claimed at its first commit, or (its title came later) now.
        let claimed = meta.returning.borrow_mut().take();
        let Some(p) = claimed.or_else(|| self.take_placement(&window)) else { return true };
        self.space.map_element(window.clone(), Point::from((p.x, p.y)), p.focused);
        meta.put_back.set(true);
        Self::set_tile_state(&window, p.tile, p.part);
        window.decoration().rolled = p.rolled;
        window.decoration().tab_offset = p.tab_offset;
        crate::shell::groups::note_restored(&p.grouping, p.tab_order);
        *meta.grouping.borrow_mut() = p.grouping.clone();
        if p.tab_order > 0 {
            meta.tab_order.set(p.tab_order);
        }
        // The displays may have changed since it was there: in its tile on
        // the display it is on, or else onto one if it is out of reach.
        self.refit(&window);
        meta.restored_focus.set(p.focused.then(std::time::Instant::now));
        if p.workspace != self.workspace {
            self.move_to_workspace(&window, p.workspace);
        } else if p.minimized {
            self.minimize(&window);
        } else if p.focused {
            self.focus_window(&window);
        } else {
            // New windows come up focused, but while windows are coming
            // back after a restart, the one that had focus keeps it. (Not
            // later: a dialog opened again a minute on must come up in
            // front, not under that window.)
            let coming_back = |w: &&WindowElement| w.meta().restored_focus.get().is_some_and(|t| t.elapsed() < KEEP);
            let front = self.space.elements().rev().find(coming_back).cloned().unwrap_or_else(|| window.clone());
            self.focus_window(&front);
        }
        if p.depth > 0 && self.space.element_location(&window).is_some() {
            meta.restored_depth.set(Some((p.depth, std::time::Instant::now())));
            self.restack_returning(&window, p.depth);
        }
        false
    }

    /// Put the windows back before `window` (at `depth`) that were over it
    /// just over it again, highest first, so the stacking comes back
    /// whatever order their clients reconnected in. Windows opened since
    /// stay where they are.
    fn restack_returning(&mut self, window: &WindowElement, depth: u32) {
        let mut over: Vec<(u32, WindowElement)> = self
            .space
            .elements()
            .filter(|w| *w != window)
            .filter_map(|w| w.meta().restored_depth.get().filter(|(d, t)| *d > depth && t.elapsed() < KEEP).map(|(d, _)| (d, w.clone())))
            .collect();
        over.sort_by_key(|(d, _)| std::cmp::Reverse(*d));
        for (_, w) in &over {
            self.space.raise_element_above(w, window, false);
        }
        self.keep_children_above();
    }

    /// Open a window that names a parent (an alert, a dialog) over it, as
    /// Haiku's BAlert does: centred across, a quarter of the way down.
    pub fn place_over_parent(&mut self, surface: &WlSurface) {
        let Some(window) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else {
            return;
        };
        let Some(parent) = window.0.toplevel().and_then(|t| t.parent()) else { return };
        let Some(parent) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(&parent)).cloned() else {
            return;
        };
        let (Some(pg), Some(g)) = (self.space.element_geometry(&parent), self.space.element_geometry(&window)) else {
            return;
        };
        let mut x = pg.loc.x + (pg.size.w - g.size.w) / 2;
        let mut y = (pg.loc.y + pg.size.h / 4 - g.size.h / 3).max(pg.loc.y);
        // A child bigger than its parent (Quick Look) mustn't hang off the
        // screen or under Deskbar: it rises above the parent's top if it
        // has to.
        let centre = (pg.loc + Point::from((pg.size.w / 2, pg.size.h / 2))).to_f64();
        if let Some(out) = self.space.output_under(centre).next().cloned().and_then(|o| self.usable_area(&o)) {
            x = x.min(out.loc.x + out.size.w - g.size.w).max(out.loc.x);
            y = y.min(out.loc.y + out.size.h - g.size.h).max(out.loc.y);
        }
        let loc = self.space.element_location(&window).unwrap_or_default();
        self.space.map_element(window, loc + Point::from((x - g.loc.x, y - g.loc.y)), true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_showing_comes_back() {
        // As an app_server from before `showing` saved it: none to go back to.
        let old = r#"[{"app_id":"olo.tracker","title":"Home","x":200,"y":600,"workspace":2,"minimized":false,"focused":true,"at":1}]"#;
        let list: Vec<Placement> = serde_json::from_str(old).unwrap();
        assert_eq!((list[0].showing, showing(&list, 4)), (None, 0));
        let saved = serde_json::to_string(&[Placement { showing: Some(2), ..list[0].clone() }]).unwrap();
        let list: Vec<Placement> = serde_json::from_str(&saved).unwrap();
        assert_eq!(showing(&list, 4), 2);
        // Fewer workspaces now: the last.
        assert_eq!(showing(&list, 2), 1);
        assert_eq!(showing(&[], 4), 0);
    }
}
