//! Window placement memory, so windows come back where they were when
//! app_server restarts in place (`olo-session reload`) or an app
//! re-executes itself after an upgrade.
//!
//! On exit app_server writes every window's place, and the workspace
//! each display shows, to `$XDG_RUNTIME_DIR/olo-layout-$WAYLAND_DISPLAY.json`; a
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
    /// The active display's workspace showing when it was saved (the
    /// same in every place saved together); before each display had its
    /// own, the one every display showed.
    #[serde(default)]
    pub showing: Option<u32>,
    /// Each display's (by its `display_key`) workspace showing when it was
    /// saved, so a restart comes back to them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displays: Vec<(String, u32)>,
    /// Seconds since the epoch when it was recorded.
    pub at: u64,
    /// The process that showed it (0: not known, any may take it).
    #[serde(default)]
    pub pid: i32,
    /// The display it was on: its name and place. Back on that display
    /// unchanged, it is put back exactly; only after the displays changed
    /// is it fitted to them again.
    #[serde(default)]
    pub screen: Option<(String, [i32; 4])>,
    /// Floated over auto-tiled windows (true) or tiled (false) by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating: Option<bool>,
    /// Its number in the saved list (from 1), which the saved auto-tiling
    /// trees name its tile by (0: none).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slot: u64,
    /// Its connection then: a window on the same one isn't coming back.
    #[serde(skip)]
    pub client: Option<smithay::reexports::wayland_server::backend::ClientId>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(olo_xdg::legacy::runtime_file(std::path::Path::new(&dir), &format!("olo-layout-{display}.json")))
}

/// Where the auto-tiling trees go, beside the layout.
fn trees_path() -> Option<PathBuf> {
    path().map(|p| p.with_extension("tiles.json"))
}

/// The auto-tiling trees saved with the layout, if it was saved moments
/// ago (`load` comes first and says).
pub fn load_trees() -> Vec<crate::shell::autotile::SavedTree> {
    let Some(p) = trees_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    let _ = std::fs::remove_file(&p);
    serde_json::from_str(&text).unwrap_or_default()
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

/// The workspace each display showed when `list` was saved (of
/// `count`), to start on, by `display_key`; saved before each display had
/// its own, the one they all showed (under [`EVERY`]).
pub fn showing(list: &[Placement], count: u32) -> std::collections::HashMap<String, u32> {
    let last = count.max(1) - 1;
    let mut map: std::collections::HashMap<String, u32> =
        list.iter().find(|p| !p.displays.is_empty()).map(|p| p.displays.iter().map(|(k, ws)| (k.clone(), (*ws).min(last))).collect()).unwrap_or_default();
    if map.is_empty()
        && let Some(ws) = list.iter().find_map(|p| p.showing)
    {
        map.insert(EVERY.to_owned(), ws.min(last));
    }
    map
}

/// The key in `AnvilState::showing` for every display without one of its
/// own (as a layout saved before each display had its own workspaces
/// says).
pub const EVERY: &str = "*";

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
        // Away (minimized, on a workspace not showing): where it comes back
        // to, along with its display if that moved meanwhile (`unpark`).
        let loc = self.space.element_location(w).unwrap_or_else(|| {
            let mut loc = meta.saved_location.get();
            let left = meta.left_among.borrow().clone();
            if !left.is_empty() && left != self.display_frames() {
                loc += self.display_moved(smithay::utils::Rectangle::new(loc, smithay::desktop::space::SpaceElement::geometry(w).size), &left);
            }
            loc
        });
        let size = w.content_size();
        let (tile, part) = Self::tile_state(w);
        // (Before the decoration is borrowed below: the geometry borrows it.)
        let screen = self.screen_of(w);
        Some(Placement {
            tile,
            part,
            app_id: olo_xdg::legacy::new_app_id(&app_id),
            title: w.title(),
            x: loc.x,
            y: loc.y,
            width: size.w,
            height: size.h,
            workspace: self.workspace_of(w),
            minimized: meta.minimized.get(),
            focused: focused.is_some() && w.wl_surface().as_deref() == focused,
            rolled: w.rolled(),
            tab_offset: w.decoration().tab_offset,
            grouping: meta.grouping.borrow().clone(),
            tab_order: meta.tab_order.get(),
            depth: 0,
            showing: None,
            displays: Vec::new(),
            at: now(),
            pid: meta.owner.borrow().as_ref().map_or(0, |(pid, _)| *pid),
            client: meta.owner.borrow().as_ref().map(|(_, c)| c.clone()),
            screen,
            floating: Self::float_override(w),
            slot: 0,
        })
    }

    /// The display `w` is on (away, the one it left), its name and place.
    fn screen_of(&self, w: &WindowElement) -> Option<(String, [i32; 4])> {
        let o = self.display_of(w)?;
        let g = self.space.output_geometry(&o)?;
        Some((o.name(), [g.loc.x, g.loc.y, g.size.w, g.size.h]))
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
        // Each window's slot, by its id (the list follows `all_windows`
        // where it has a place).
        let windows = self.all_windows();
        let ids: Vec<u64> = windows.iter().filter(|w| !w.app_id().is_empty() && (w.meta().placed.get() || w.meta().returning.borrow().is_some())).map(|w| w.id()).collect();
        let open = list.len();
        list.extend(self.layout.iter().cloned());
        let (active, displays) = (self.workspace(), self.showing.iter().map(|(k, ws)| (k.clone(), *ws)).collect::<Vec<_>>());
        for (i, p) in list.iter_mut().enumerate() {
            p.showing = Some(active);
            p.displays = displays.clone();
            p.slot = i as u64 + 1;
        }
        // The auto-tiling trees, each tile naming its window's slots (a
        // group's windows', any of which may come back first), a closed
        // window's place the newest remembered one like it.
        let slot_of = |id: u64| ids.iter().position(|&i| i == id).filter(|&i| i < open).map(|i| i as u64 + 1);
        // The newest place remembered for a closed window like it, else
        // one like it already back (it re-executed and returned before
        // this app_server went, taking its remembered place).
        let like = |app_id: &str, title: &str| {
            let same = |i: &usize| list[*i].app_id == app_id && list[*i].title == title;
            (open..list.len()).rev().find(same).or_else(|| (0..open).find(same)).map(|i| i as u64 + 1)
        };
        let slots = |id: u64| -> Vec<u64> {
            // Closed since (its app re-executing, on a workspace not
            // showing, whose tree nothing has looked at): the newest place
            // remembered for one like it.
            let Some(w) = windows.iter().find(|w| w.id() == id) else {
                return self.auto_known(id).and_then(|(app_id, title)| like(&app_id, &title)).into_iter().collect();
            };
            let group = self.groups.place(w).map(|(n, _)| n);
            let mut out: Vec<u64> = slot_of(id).into_iter().collect();
            // Its tabs: its group's windows on its display's workspace.
            let here = |o: &WindowElement| self.workspace_of(o) == self.workspace_of(w) && self.display_of(o) == self.display_of(w);
            if let Some(g) = group {
                out.extend(windows.iter().filter(|o| o.id() != id && self.groups.place(o).map(|(n, _)| n) == Some(g) && here(o)).filter_map(|o| slot_of(o.id())));
            }
            out
        };
        let trees: Vec<crate::shell::autotile::SavedTree> = self
            .auto_trees_as_laid_out()
            .into_iter()
            .map(|((workspace, output), t)| crate::shell::autotile::SavedTree {
                workspace,
                output,
                tree: crate::shell::autotile::SavedNode::of(&t, &slots, &like),
            })
            .collect();
        if let Some(tp) = trees_path() {
            match serde_json::to_string(&trees) {
                Ok(text) if !trees.is_empty() => {
                    if let Err(e) = std::fs::write(&tp, text) {
                        warn!(?e, "could not save the auto-tiling trees");
                    }
                }
                _ => {
                    let _ = std::fs::remove_file(&tp);
                }
            }
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
        meta.slot.set(p.slot);
        if p.floating.is_some() {
            Self::restore_float_override(&window, p.floating);
        }
        meta.restored_part.set(p.tile.map(|t| p.part.unwrap_or(t.fraction())));
        Self::set_tile_state(&window, p.tile, p.part);
        window.decoration().rolled = p.rolled;
        window.decoration().tab_offset = p.tab_offset;
        // A window of an app started again is another window that only
        // has the same title ("Untitled 2"): its place, not a group it
        // was put in by hand (it would join another app's windows).
        let restarted = p.pid > 0 && meta.owner.borrow().as_ref().is_some_and(|(pid, _)| *pid != p.pid);
        let grouping = match &p.grouping {
            crate::shell::groups::Grouping::With(_) if restarted => crate::shell::groups::Grouping::Auto,
            g => g.clone(),
        };
        crate::shell::groups::note_restored(&grouping, p.tab_order);
        *meta.grouping.borrow_mut() = grouping;
        if p.tab_order > 0 {
            meta.tab_order.set(p.tab_order);
        }
        // The displays may have changed since it was there: in its tile on
        // the display it is on, or else onto one if it is out of reach. A
        // window of its own size back on the display it was on, as it was,
        // is exactly as it was (one bigger than the room beside Deskbar,
        // which may not be back yet, isn't squeezed into it).
        if !self.own_size(&window) || p.screen.is_none() || p.screen != self.screen_of(&window) {
            self.refit(&window);
        }
        meta.restored_focus.set(p.focused.then(std::time::Instant::now));
        if p.minimized {
            // Minimized, on its own workspace (showing or not).
            self.minimize(&window);
            if p.workspace != self.workspace_of(&window) {
                self.move_to_workspace(&window, p.workspace);
            }
        } else if p.workspace != self.workspace_of(&window) {
            self.move_to_workspace(&window, p.workspace);
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
    fn the_workspaces_showing_come_back() {
        // As an app_server from before `showing` saved it: none to go back to.
        let old = r#"[{"app_id":"olo.tracker","title":"Home","x":200,"y":600,"workspace":2,"minimized":false,"focused":true,"at":1}]"#;
        let list: Vec<Placement> = serde_json::from_str(old).unwrap();
        assert!(list[0].showing.is_none() && showing(&list, 4).is_empty());
        // From before each display had its own: the one for them all.
        let saved = serde_json::to_string(&[Placement { showing: Some(2), ..list[0].clone() }]).unwrap();
        let list: Vec<Placement> = serde_json::from_str(&saved).unwrap();
        assert_eq!(showing(&list, 4).get(EVERY), Some(&2));
        // Fewer workspaces now: the last.
        assert_eq!(showing(&list, 2).get(EVERY), Some(&1));
        assert!(showing(&[], 4).is_empty());
        // Each display's.
        let displays = vec![("eDP-1".to_owned(), 1), ("Dell Inc. DELL S2425HS 8K08534".to_owned(), 3)];
        let saved = serde_json::to_string(&[Placement { showing: Some(1), displays, ..list[0].clone() }]).unwrap();
        let map = showing(&serde_json::from_slice::<Vec<Placement>>(saved.as_bytes()).unwrap(), 4);
        assert_eq!((map.get("eDP-1"), map.get("Dell Inc. DELL S2425HS 8K08534"), map.get(EVERY)), (Some(&1), Some(&3), None));
    }
}
