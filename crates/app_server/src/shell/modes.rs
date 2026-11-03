//! Windowing modes (Deskbar's tray, `Request::SetWindowingMode`, ⌃⌥⌘T and
//! ⌃⌥⌘O; Preferences ▸ Windows for the default): each workspace of each
//! display is Standard (windows stay where they are put), Auto-Tiling
//! (`autotile`) or Focus.
//!
//! Focus shows one window at a time on its display, zoomed: the front one
//! of those that would be tiles (`floats` says which aren't: dialogs,
//! alerts, fixed-size and floating windows, which show over it as ever).
//! Activating another (Deskbar, ⌘Tab, Twitcher passing it, the overview)
//! raises it, so it is the one that shows. An app's windows are always
//! grouped there (`group_key`), so its others are tabs along the top.
//! The rest are covered (`WindowMeta::covered`): not drawn and not under
//! the pointer, but still in the space, zoomed at the same place, so
//! showing one is only raising it (its picture is ready, at that size),
//! and Deskbar, Twitcher and the overview have them as ever. A dialog of
//! a covered window is covered with it; an alert that names no window
//! (asked by an app whose windows are covered) never is.
//!
//! Every window that would be a tile is zoomed (`zoom_window`, so leaving
//! Focus puts each back where it was) and a new one opens zoomed
//! (`focus_claim`). Moved or sized by hand, a window zooms again when let
//! go, on the display it was let go on. The arranging commands (tiling,
//! Tile All, Cascade, Fan Out, Smart Tile) leave Focus for Standard.

use olo_protocol::WindowingMode;
use smithay::{
    desktop::{Space, WindowSurface},
    output::Output,
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::protocol::wl_surface::WlSurface},
};
use tracing::info;

use super::WindowElement;
use crate::state::{AnvilState, Backend};

impl<B: Backend + 'static> AnvilState<B> {
    /// The windowing mode of `output` on its workspace `workspace`.
    pub fn mode_of(&self, output: &Output, workspace: u32) -> WindowingMode {
        self.settings.tiling.mode(workspace, &crate::desktop::display_key(output))
    }

    /// The windowing mode of `output` now, on the workspace it shows.
    pub fn mode_now(&self, output: &Output) -> WindowingMode {
        self.mode_of(output, self.showing(output))
    }

    /// Whether Focus is the mode anywhere (else nothing here has work).
    fn any_focus(&self) -> bool {
        let t = &self.settings.tiling;
        t.default_mode == WindowingMode::Focus || t.modes.iter().any(|m| m.mode == WindowingMode::Focus)
    }

    /// Whether `w` is on a display in Focus mode, on the workspace it
    /// shows.
    pub fn in_focus_mode(&self, w: &WindowElement) -> bool {
        self.any_focus() && self.window_output(w).is_some_and(|o| self.mode_now(&o) == WindowingMode::Focus)
    }

    /// Choose the windowing mode of `output` (the active display unless
    /// given) on its workspace `workspace` (the one showing unless
    /// given). The mode it has now.
    pub fn set_windowing_mode(&mut self, mode: WindowingMode, workspace: Option<u32>, output: Option<Output>) -> WindowingMode {
        let Some(output) = output.or_else(|| self.active_output()) else { return WindowingMode::Standard };
        let showing = self.showing(&output);
        let ws = workspace.unwrap_or(showing).min(self.settings.workspace_count.max(1) - 1);
        let was = self.mode_of(&output, ws);
        if mode == was {
            return mode;
        }
        let outputs = vec![output.name()];
        let shown = vec![output.clone()];
        if ws == showing && mode == WindowingMode::AutoTiling {
            let windows: Vec<WindowElement> = shown.iter().flat_map(|o| self.windows_on(o)).collect();
            self.remember_arrangement(&windows);
            self.auto_turned_on(ws, outputs.clone(), was);
        }
        self.settings.tiling.set_mode(ws, &crate::desktop::display_key(&output), mode);
        if was == WindowingMode::AutoTiling {
            self.auto_forget(ws, &outputs);
        }
        if was == WindowingMode::Focus && ws == showing {
            self.leave_focus(&shown, mode);
        }
        crate::desktop::save_settings(&self.settings);
        self.ipc.broadcast(olo_protocol::Event::SettingsChanged { settings: self.settings.clone() });
        info!(workspace = ws, output = output.name(), ?mode, "windowing mode");
        self.auto.changed();
        mode
    }

    /// Each display's windowing mode on its workspace `workspace` (the
    /// one it shows unless given), as the IPC tells it.
    pub fn windowing_mode_infos(&self, workspace: Option<u32>) -> Vec<olo_protocol::WindowingModeInfo> {
        self.space
            .outputs()
            .map(|o| {
                let ws = workspace.unwrap_or_else(|| self.showing(o));
                olo_protocol::WindowingModeInfo { output: o.name(), mode: self.mode_of(o, ws), workspace: ws }
            })
            .collect()
    }

    /// An arranging command for `output` (the active display unless
    /// given): Focus gives way to Standard first, saying so.
    pub fn arranging(&mut self, output: Option<Output>) {
        let Some(o) = output.or_else(|| self.active_output()) else { return };
        if self.mode_now(&o) == WindowingMode::Focus {
            self.set_windowing_mode(WindowingMode::Standard, None, Some(o));
            self.show_indicator(crate::osd::Indicator::Mode(WindowingMode::Standard));
        }
    }

    /// Focus left for `mode` on these displays: the windows it zoomed go
    /// back to their own frames (for Auto-Tiling, to the tiles it gives
    /// them), and none is covered.
    fn leave_focus(&mut self, outputs: &[Output], mode: WindowingMode) {
        let windows: Vec<WindowElement> = self.space.elements().cloned().collect();
        for w in windows {
            let m = w.meta();
            m.focus_on.set(None);
            m.focus_display.set(None);
            m.covered.set(false);
            if !m.focus_zoomed.replace(false) || !self.window_output(&w).is_some_and(|o| outputs.contains(&o)) || !self.is_zoomed(&w) {
                continue;
            }
            if mode == WindowingMode::AutoTiling {
                // Its tile's configure comes next; its own frame is kept
                // for Restore.
                let WindowSurface::Wayland(t) = w.0.underlying_surface();
                t.with_pending_state(|s| s.states.unset(xdg_toplevel::State::Maximized));
            } else {
                self.unzoom_window(&w);
            }
        }
    }

    /// Once a turn (`tick`): which windows Focus shows, covers and zooms.
    pub fn focus_tick(&mut self) {
        let focus: Vec<Output> = if self.any_focus() { self.space.outputs().filter(|o| self.mode_now(o) == WindowingMode::Focus).cloned().collect() } else { Vec::new() };
        if focus.is_empty() && !self.space.elements().any(|w| w.meta().focus_on.get().is_some() || w.meta().focus_display.get().is_some() || w.meta().focus_zoomed.get()) {
            return;
        }
        // Not while a window is held: it zooms where it is let go.
        let held = self.pointer.is_grabbed() || self.seat.get_touch().is_some_and(|t| t.is_grabbed()) || self.tab_drag.is_some() || self.tile_drag.is_some();
        let windows: Vec<WindowElement> = self.space.elements().cloned().collect();
        for w in &windows {
            let display = if focus.is_empty() { None } else { self.window_output(w).and_then(|o| focus.iter().position(|f| *f == o)) };
            let unit = display.filter(|_| w.meta().placed.get() && !w.rolled() && !self.floats(w));
            let m = w.meta();
            m.focus_on.set(unit.map(|i| i as u32));
            m.focus_display.set(display.map(|i| i as u32));
            if held {
                continue;
            }
            match unit {
                Some(i) => {
                    if !super::autotile::fullscreen(w) && !self.behind_in_group(w) {
                        self.focus_zoom(w, &focus[i].clone());
                    }
                }
                None if m.focus_zoomed.replace(false) => {
                    // Gone to a display not in Focus, or floated.
                    if self.is_zoomed(w) {
                        self.unzoom_window(w);
                    }
                }
                None => {}
            }
        }
        if !focus.is_empty() {
            self.keep_floating_over_focus(&windows);
        }
        mark_covered(&self.space);
    }

    /// `w` zoomed on `output`, unless it is already.
    fn focus_zoom(&mut self, w: &WindowElement, output: &Output) {
        let Some(area) = self.tile_area(output) else { return };
        let want = w.inner_size(area.size);
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        let (zoomed, size) = t.with_pending_state(|s| (s.states.contains(xdg_toplevel::State::Maximized), s.size));
        if zoomed && size == Some(want) && self.space.element_location(w) == Some(area.loc) {
            return;
        }
        self.zoom_window(w);
        w.meta().focus_zoomed.set(true);
    }

    /// Floating windows over the one Focus shows, in the order they were
    /// (as over auto-tiling's tiles).
    fn keep_floating_over_focus(&mut self, order: &[WindowElement]) {
        let Some(top) = order.iter().rposition(|w| w.meta().focus_on.get().is_some()) else { return };
        let under: Vec<WindowElement> = order[..top].iter().filter(|w| w.meta().focus_on.get().is_none() && self.floats(w)).cloned().collect();
        for w in under {
            self.space.raise_element(&w, false);
        }
    }

    /// A window's first commit (before its first configure goes): on a
    /// display in Focus, it opens zoomed rather than drawing another size
    /// first.
    pub fn focus_claim(&mut self, surface: &WlSurface) {
        if !self.any_focus() {
            return;
        }
        let Some(w) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else { return };
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        if t.is_initial_configure_sent() || w.meta().placed.get() || w.meta().returning.borrow().is_some() || self.floats(&w) {
            return;
        }
        let Some(output) = self.window_output(&w).filter(|o| self.mode_now(o) == WindowingMode::Focus) else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let content = w.inner_size(area.size);
        t.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Maximized);
            s.size = Some(content);
        });
        self.space.relocate_element(&w, area.loc);
        w.meta().focus_zoomed.set(true);
        info!(window = w.id(), app_id = w.app_id(), "a new window zoomed for Focus");
    }
}

/// Each window Focus doesn't show marked covered, from the stacking order
/// now (done before drawing, as `groups::mark_behind`, so a window raised
/// shows at once): on each display in Focus, the front window of those
/// that would be tiles shows, with its group; the others are covered, and
/// their dialogs with them.
pub fn mark_covered(space: &Space<WindowElement>) {
    // Each display's front window's group (None: on its own).
    let mut fronts: Vec<(u32, Option<u64>)> = Vec::new();
    let mut any = false;
    for w in space.elements().rev() {
        let m = w.meta();
        let Some(d) = m.focus_on.get() else {
            m.covered.set(false);
            continue;
        };
        let group = w.decorated().then(|| w.decoration().group).flatten();
        let covered = match fronts.iter().find(|(o, _)| *o == d) {
            None => {
                fronts.push((d, group));
                false
            }
            Some((_, g)) => g.is_none() || *g != group,
        };
        m.covered.set(covered);
        any |= covered;
    }
    if !any {
        return;
    }
    // Back to front, and children stay over their parents, so a dialog is
    // done before its own alerts, which go with it.
    let others: Vec<&WindowElement> = space.elements().filter(|w| w.meta().focus_on.get().is_none()).collect();
    for w in &others {
        // (Picture in picture is over whatever Focus shows.)
        if w.meta().pip.get() {
            w.meta().covered.set(false);
            continue;
        }
        let covered = match w.0.toplevel().and_then(|t| t.parent()) {
            Some(parent) => space.elements().any(|p| p.meta().covered.get() && p.wl_surface().as_deref() == Some(&parent)),
            // An alert that names no window asks the person something,
            // whichever app's window shows (Passwords' passkey question
            // over Chrome): it shows over it, never covered with its app.
            None if super::groups::is_parentless_alert(w) => false,
            // An app's own panel (StyledEdit's Find), as BeOS's floating
            // app feel: shown while one of its app's windows on its
            // display is, covered when they all are. An app of only
            // panels (DeskCalc) shows, and so does every window on a
            // display not in Focus: the app's windows elsewhere are
            // theirs to show (Chrome on another display went when
            // Terminal was chosen over Chrome on this one).
            None => match w.meta().focus_display.get() {
                Some(d) => {
                    let app = w.app_id();
                    let mut units = space.elements().filter(|u| u.meta().focus_on.get() == Some(d) && u.app_id() == app).peekable();
                    units.peek().is_some() && units.all(|u| u.meta().covered.get())
                }
                None => false,
            },
        };
        w.meta().covered.set(covered);
    }
}
