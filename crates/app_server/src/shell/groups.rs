//! Grouped windows: an app's windows on the screen share one frame,
//! stacked, with their tabs side by side along its top as a tab bar (in
//! the order they opened, never overlapping: narrowed alike when they
//! don't all fit), and the front one showing. Clicking a tab brings that
//! window forward; moving or sizing the front one takes the others along.
//! Apps are grouped unless `TilingSettings::ungrouped_apps` lists them
//! (Tracker's and Deskbar's Group Windows).
//!
//! Membership isn't stored: once per loop iteration (`sync_groups`, from
//! `tick`) the groupable windows in the space are gathered by app, and
//! each group of two or more is brought together. Its frame is that of
//! its front-most window that was already in the group (else, for a group
//! just forming, of one that was already on screen, else the front-most):
//! a window that opens, comes back from being hidden or arrives from
//! another workspace joins the group where it is, rather than pulling it
//! to where the new window happened to open. Sizes follow only between
//! pointer grabs, so a hidden window isn't asked for a new size on every
//! step of a resize. Dialogs (windows with a parent), fixed-size panels
//! and fullscreen windows are never grouped.

use smithay::{
    desktop::WindowSurface,
    utils::{Logical, Point, Rectangle},
};

use super::{
    WindowElement,
    tiling::{CASCADE, resizable},
};
use crate::state::{AnvilState, Backend};

/// What `sync_groups` saw last time.
#[derive(Debug, Default)]
pub struct Groups {
    /// The groupable windows in the space.
    seen: Vec<WindowElement>,
    /// The windows in a group.
    members: Vec<WindowElement>,
}

impl Groups {
    /// Whether `w` is in a group.
    pub fn has(&self, w: &WindowElement) -> bool {
        self.members.contains(w)
    }

    /// The windows in `app_id`'s group.
    pub fn members_of(&self, app_id: &str) -> Vec<WindowElement> {
        self.members.iter().filter(|w| w.app_id() == app_id).cloned().collect()
    }
}

/// Each tab's offset and width along a top `frame_w` wide, for tabs
/// wanting `wanted` widths: as wanted if they all fit, else all alike,
/// the last taking what is left over.
pub fn tab_bar(wanted: &[i32], frame_w: i32) -> Vec<(i32, i32)> {
    let n = wanted.len() as i32;
    if n == 0 {
        return Vec::new();
    }
    let widths: Vec<i32> = if wanted.iter().sum::<i32>() <= frame_w {
        wanted.to_vec()
    } else {
        let each = (frame_w / n).max(1);
        (0..n).map(|i| if i == n - 1 { (frame_w - each * (n - 1)).max(1) } else { each }).collect()
    };
    let mut x = 0;
    widths
        .into_iter()
        .map(|w| {
            let at = x;
            x += w;
            (at, w)
        })
        .collect()
}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// A window in a group but not its front one: arranging commands
    /// move the front one and the rest follow.
    pub fn behind_in_group(&self, w: &WindowElement) -> bool {
        if !self.groups.has(w) {
            return false;
        }
        let app = w.app_id();
        self.space.elements().rev().find(|o| o.app_id() == app && self.groups.has(o)).is_some_and(|front| front != w)
    }

    /// Whether `app_id`'s windows are grouped (the default).
    pub fn groups_app(&self, app_id: &str) -> bool {
        !app_id.is_empty() && !self.settings.tiling.ungrouped_apps.iter().any(|a| a == app_id)
    }

    fn groupable(&self, w: &WindowElement) -> bool {
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        w.decorated() && t.parent().is_none() && resizable(w) && self.groups_app(&w.app_id())
    }

    /// The groups on screen now, each back to front.
    fn current_groups(&self) -> (Vec<WindowElement>, Vec<Vec<WindowElement>>) {
        let seen: Vec<WindowElement> = self.space.elements().filter(|w| self.groupable(w)).cloned().collect();
        let mut by_app: Vec<(String, Vec<WindowElement>)> = Vec::new();
        for w in &seen {
            let app = w.app_id();
            match by_app.iter_mut().find(|(a, _)| *a == app) {
                Some((_, g)) => g.push(w.clone()),
                None => by_app.push((app, vec![w.clone()])),
            }
        }
        (seen, by_app.into_iter().map(|(_, g)| g).filter(|g| g.len() > 1).collect())
    }

    /// Bring each group together (see the module's notes).
    pub fn sync_groups(&mut self) {
        let (seen, groups) = self.current_groups();
        let before = std::mem::take(&mut self.groups);
        let resizing = self.pointer.is_grabbed();
        let mut members = Vec::new();
        for g in &groups {
            let leader = g
                .iter()
                .rev()
                .find(|w| before.members.contains(w))
                .or_else(|| g.iter().rev().find(|w| before.seen.contains(w)))
                .unwrap_or(&g[g.len() - 1])
                .clone();
            let Some(frame) = self.space.element_geometry(&leader) else { continue };
            for w in g.iter().filter(|w| **w != leader) {
                self.join_group(w, frame, resizing);
            }
            self.keep_together(g);
            self.lay_tabs(g, frame.size.w);
            members.extend(g.iter().cloned());
        }
        // Out of a group now (closed down to one, ungrouped, minimized):
        // its tab back to its own width, at the left; and a group that
        // came apart still on screen stepped out like a cascade, so the
        // windows it hid show.
        let left: Vec<WindowElement> = before.members.iter().filter(|w| !members.contains(w)).cloned().collect();
        for w in &left {
            let mut d = w.decoration();
            if d.tab_width.take().is_some() {
                d.tab_offset = 0;
            }
        }
        let mut stacked: Vec<(WindowElement, Rectangle<i32, Logical>)> = Vec::new();
        let mut apart: Vec<&WindowElement> = left.iter().filter(|w| self.space.element_location(w).is_some()).collect();
        apart.sort_by_key(|w| w.id());
        for w in apart {
            let Some(frame) = self.space.element_geometry(w) else { continue };
            let under = stacked.iter().filter(|(o, f)| o.app_id() == w.app_id() && f.loc == frame.loc).count() as i32;
            stacked.push((w.clone(), frame));
            if under > 0 {
                self.space.relocate_element(w, frame.loc + Point::from((under * CASCADE, under * CASCADE)));
            }
        }
        self.groups = Groups { seen, members };
    }

    /// `w` at the group's frame: moved there now, and sized to it unless
    /// a resize is under way (or it was already asked for that size).
    fn join_group(&mut self, w: &WindowElement, frame: Rectangle<i32, Logical>, resizing: bool) {
        let Some(now) = self.space.element_geometry(w) else { return };
        if now == frame {
            return;
        }
        let content = w.inner_size(frame.size);
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        let asked = t.with_pending_state(|s| s.size);
        if now.size != frame.size && !resizing && asked != Some(content) {
            self.set_frame(w, frame, false);
        } else if now.loc != frame.loc {
            self.space.relocate_element(w, frame.loc);
        }
    }

    /// The group's windows next to each other in the stack, under its
    /// front one, so no other window comes between them and hides tabs.
    fn keep_together(&mut self, g: &[WindowElement]) {
        let order: Vec<WindowElement> = self.space.elements().cloned().collect();
        let Some(front) = g.last().and_then(|f| order.iter().position(|w| w == f)) else { return };
        let below = &order[front + 1 - g.len()..=front];
        if below.iter().all(|w| g.contains(w)) {
            return;
        }
        // Space only raises to the top: the group, then whatever was in
        // front of it, back over it.
        for w in g.iter().chain(order[front + 1..].iter().filter(|w| !g.contains(w))) {
            self.space.raise_element(w, false);
        }
    }

    /// The tabs side by side, in the order the windows opened.
    fn lay_tabs(&mut self, g: &[WindowElement], frame_w: i32) {
        let mut tabs: Vec<&WindowElement> = g.iter().collect();
        tabs.sort_by_key(|w| w.id());
        let wanted: Vec<i32> = tabs
            .iter()
            .map(|w| {
                let title = w.title();
                w.decoration().wanted_tab_width(&title)
            })
            .collect();
        for (w, (x, width)) in tabs.iter().zip(tab_bar(&wanted, frame_w)) {
            let mut d = w.decoration();
            d.tab_offset = x;
            d.tab_width = Some(width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_side_by_side_or_shared_alike() {
        assert_eq!(tab_bar(&[100, 120, 90], 500), [(0, 100), (100, 120), (220, 90)]);
        // Too many to fit: the same width each, the last taking the rest.
        assert_eq!(tab_bar(&[300, 300, 300], 500), [(0, 166), (166, 166), (332, 168)]);
        assert!(tab_bar(&[], 500).is_empty());
    }
}
