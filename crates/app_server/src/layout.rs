//! Window placement memory, so windows come back where they were when
//! app_server restarts in place (`tabula-session reload`) or an app
//! re-executes itself after an upgrade.
//!
//! On exit app_server writes every window's place to
//! `$XDG_RUNTIME_DIR/tabula-layout-$WAYLAND_DISPLAY.json`; a closing window
//! is remembered in memory too. A new window whose app id and title match
//! a remembered one (recent enough) takes its place instead of the next
//! cascade slot.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub app_id: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub workspace: u32,
    pub minimized: bool,
    pub focused: bool,
    /// Seconds since the epoch when it was recorded.
    pub at: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(PathBuf::from(dir).join(format!("tabula-layout-{display}.json")))
}

/// Places saved by the previous app_server, if it exited moments ago.
pub fn load() -> Vec<Placement> {
    let Some(p) = path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    let _ = std::fs::remove_file(&p);
    let list: Vec<Placement> = serde_json::from_str(&text).unwrap_or_default();
    let fresh: Vec<Placement> = list.into_iter().filter(|e| now().saturating_sub(e.at) < KEEP.as_secs()).collect();
    info!(windows = fresh.len(), "restoring window layout");
    fresh
}

impl<B: Backend + 'static> AnvilState<B> {
    fn placement_of(&self, w: &WindowElement, focused: Option<&WlSurface>) -> Option<Placement> {
        let meta = w.meta();
        let loc = self.space.element_location(w).unwrap_or_else(|| meta.saved_location.get());
        let app_id = w.app_id();
        if app_id.is_empty() {
            return None;
        }
        Some(Placement {
            app_id,
            title: w.title(),
            x: loc.x,
            y: loc.y,
            workspace: meta.workspace.get().unwrap_or(self.workspace),
            minimized: meta.minimized.get(),
            focused: focused.is_some() && w.wl_surface().as_deref() == focused,
            at: now(),
        })
    }

    /// Remember a window that is going away (it may come straight back, as
    /// when an app re-executes itself after an upgrade).
    pub fn remember_placement(&mut self, w: &WindowElement) {
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface().map(|s| s.into_owned()));
        if let Some(p) = self.placement_of(w, focused.as_ref()) {
            self.layout.retain(|e| now().saturating_sub(e.at) < KEEP.as_secs());
            self.layout.push(p);
        }
    }

    /// Write every window's place for the next app_server.
    pub fn save_layout(&mut self) {
        let Some(path) = path() else { return };
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface().map(|s| s.into_owned()));
        // Bottom-most first, so restoring in order rebuilds the stacking.
        let mut list: Vec<Placement> = self.all_windows().iter().filter_map(|w| self.placement_of(w, focused.as_ref())).collect();
        list.extend(self.layout.iter().cloned());
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

    /// On a window's first commit (when its app id and title are known):
    /// put it back where it was before the restart, if we remember it. True when this is its first showing and nothing was
    /// remembered for it, so it is still to be placed.
    pub fn restore_placement(&mut self, surface: &WlSurface) -> bool {
        let Some(window) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else {
            return false;
        };
        let meta = window.meta();
        if meta.placed.replace(true) {
            return false;
        }
        if self.layout.is_empty() {
            return true;
        }
        let (app_id, title) = (window.app_id(), window.title());
        let fresh = |e: &Placement| now().saturating_sub(e.at) < KEEP.as_secs();
        let i = self
            .layout
            .iter()
            .position(|e| fresh(e) && e.app_id == app_id && e.title == title)
            // Titles can change (a terminal's follows its directory).
            .or_else(|| self.layout.iter().position(|e| fresh(e) && e.app_id == app_id));
        let Some(i) = i else { return true };
        let p = self.layout.remove(i);
        self.space.map_element(window.clone(), Point::from((p.x, p.y)), p.focused);
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
        false
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
        // A child wider than its parent mustn't hang off the screen.
        let centre = (pg.loc + Point::from((pg.size.w / 2, pg.size.h / 2))).to_f64();
        if let Some(out) = self.space.output_under(centre).next().and_then(|o| self.space.output_geometry(o)) {
            x = x.min(out.loc.x + out.size.w - g.size.w).max(out.loc.x);
            y = y.min(out.loc.y + out.size.h - g.size.h).max(pg.loc.y);
        }
        let loc = self.space.element_location(&window).unwrap_or_default();
        self.space.map_element(window, loc + Point::from((x - g.loc.x, y - g.loc.y)), true);
    }
}
