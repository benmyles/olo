//! Grouped windows: windows on the screen that share one frame, stacked,
//! with their tabs side by side along its top as a tab bar (never
//! overlapping: narrowed alike when they don't all fit), and the front
//! one showing. Clicking a tab brings that window forward; moving or
//! sizing the front one takes the others along.
//!
//! An app's windows group by themselves (`Grouping::Auto`), unless
//! `TilingSettings::ungrouped_apps` lists it (Tracker's and Deskbar's
//! Group Windows). By hand, as in Haiku's Stack & Tile (with ⌘ for its
//! Option key; `tabdrag`), any window can be taken out of its group
//! (`Grouping::Alone`: it stays on its own until put in one) or put in
//! any other, whatever its app (`Grouping::With`: its app's group, as a
//! guest, or one made by hand), and the tabs put in any order
//! (`WindowMeta::tab_order`). Agents do the same with `GroupWindow`,
//! `UngroupWindow` and `MoveTab`. A hand-made group left with one window,
//! or a guest whose app has no windows of its own grouping, is no more:
//! that window is on its own (`dissolve_lone`). Guests whose host's
//! windows have all left their group (taken out, put in another, closed)
//! stay together as a group of their own (`hand_down`).
//!
//! Membership isn't stored: once per loop iteration (`sync_groups`, from
//! `tick`) the groupable windows in the space are gathered by key (their
//! app's, or the group they were put in), and each group of two or more
//! is brought together. Its frame is that of its front-most window that
//! was already in the group (else, for a group just forming, of one that
//! was already on screen, else the front-most; never one just put in by
//! hand; at the size it has been asked for, if it hasn't drawn that yet,
//! and no smaller than any of its windows can be):
//! a window that opens, comes back from being hidden or arrives from
//! another workspace joins the group where it is, rather than pulling it
//! to where the new window happened to open; one that arrived keeps the
//! frame it came with, for when it is on its own again (`unpark`). It
//! doesn't lead the group, though in front, until it has drawn at the
//! group's size (`Groups::settling`): the size it opened at, or an app's
//! own resize just after (Terminal fitting its grid), isn't the
//! group's. Sizes follow only while no window's edge is being dragged,
//! so a hidden window isn't asked for a new size on every step of a
//! resize. Only the front window is drawn whole: the others
//! show just their tabs (`mark_behind`), so they don't show around it
//! while it is sized smaller. Dialogs (windows with a parent, or that
//! say they are one with xdg-dialog), fixed-size panels and fullscreen
//! windows are never grouped.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use smithay::{
    desktop::{Space, WindowSurface},
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};

use super::{
    PointerResizeSurfaceGrab, TouchResizeSurfaceGrab, WindowElement,
    tiling::{CASCADE, resizable},
};
use crate::state::{AnvilState, Backend};

/// How a window groups.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    /// With its app's other windows (when its app's are grouped).
    #[default]
    Auto,
    /// Taken out of its group by hand: on its own until put in one.
    Alone,
    /// Put in this group by hand.
    With(GroupKey),
}

/// What a group's windows have in common.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKey {
    /// An app's own group, with any guests put in it.
    App(String),
    /// A group made by hand.
    Id(u64),
}

static NEXT_GROUP: AtomicU64 = AtomicU64::new(1);
static NEXT_TAB: AtomicU64 = AtomicU64::new(1);

/// A number for a group, never given before (a hand-made group's id, an
/// app group's number for agents).
pub fn new_group_id() -> u64 {
    NEXT_GROUP.fetch_add(1, Ordering::Relaxed)
}

/// A tab place after every one given so far: a new window's tab goes at
/// the end of its group.
pub fn next_tab_order() -> u64 {
    NEXT_TAB.fetch_add(1, Ordering::Relaxed)
}

/// A window put back as it was before a restart: groups and tab places
/// given from now on come after its.
pub fn note_restored(grouping: &Grouping, tab_order: u64) {
    if let Grouping::With(GroupKey::Id(n)) = grouping {
        NEXT_GROUP.fetch_max(n + 1, Ordering::Relaxed);
    }
    NEXT_TAB.fetch_max(tab_order + 1, Ordering::Relaxed);
}

/// What `sync_groups` saw last time.
#[derive(Debug, Default)]
pub struct Groups {
    /// The groupable windows in the space.
    seen: Vec<WindowElement>,
    /// The groups on screen, each with its number (for agents) and its
    /// windows back to front.
    list: Vec<(u64, Vec<WindowElement>)>,
    /// Ones of those away on another workspace (not hidden), and whether
    /// each was in a group: back, they are still what was on screen, and
    /// a window shown with them (Twitcher passing a hidden one) joins
    /// them rather than leading.
    away: Vec<(WindowElement, bool)>,
    /// Every window there was, to notice one closing.
    known: Vec<WindowElement>,
    /// Each app group's number, kept while app_server runs.
    numbers: Vec<(String, u64)>,
    /// Put in a group by hand since: not its leader, which keeps the
    /// group's frame and arrangement.
    pub joining: Vec<WindowElement>,
    /// Joined a group by itself (opened, came back, arrived) and not yet
    /// drawn at the group's size, with since when: not its leader
    /// meanwhile, so the size it opened at isn't the group's (it is in
    /// front, and would lead).
    settling: Vec<(WindowElement, Instant)>,
}

impl Groups {
    /// Whether `w` is in a group.
    pub fn has(&self, w: &WindowElement) -> bool {
        self.list.iter().any(|(_, g)| g.contains(w))
    }

    /// The number of `w`'s group, if it is in one.
    pub fn number_of(&self, w: &WindowElement) -> Option<u64> {
        self.list.iter().find(|(_, g)| g.contains(w)).map(|(n, _)| *n)
    }

    /// The windows in `w`'s group, back to front (none if it is in none).
    pub fn mates(&self, w: &WindowElement) -> Vec<WindowElement> {
        self.list.iter().find(|(_, g)| g.contains(w)).map(|(_, g)| g.clone()).unwrap_or_default()
    }

    /// `w`'s group's number and its tab's place in it (from the left).
    pub fn place(&self, w: &WindowElement) -> Option<(u64, u32)> {
        let (n, g) = self.list.iter().find(|(_, g)| g.contains(w))?;
        let tabs = in_tab_order(g);
        Some((*n, tabs.iter().position(|o| o == w)? as u32))
    }
}

/// `windows` in the order of their tabs.
pub fn in_tab_order(windows: &[WindowElement]) -> Vec<WindowElement> {
    let mut tabs = windows.to_vec();
    tabs.sort_by_key(|w| (w.meta().tab_order.get(), w.id()));
    tabs
}

/// Give `windows` their tabs' places in this order (after every place
/// given so far, so the group's own order is all that counts).
pub fn set_tab_order(windows: &[WindowElement]) {
    for w in windows {
        w.meta().tab_order.set(next_tab_order());
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

/// Where a tab `width` wide (its own wanted width: `wanted[index]` once
/// in) goes among tabs wanting `wanted`, on a top `frame_w` wide, when its
/// middle is at `middle` (from the top's left): the place whose slot's
/// middle is nearest, `current` on a tie. Returns the place and that
/// slot's offset and width.
pub fn tab_place(wanted: &[i32], width: i32, frame_w: i32, middle: f64, current: Option<usize>) -> (usize, (i32, i32)) {
    let mut best: Option<(f64, usize, (i32, i32))> = None;
    for i in 0..=wanted.len() {
        let mut with = wanted.to_vec();
        with.insert(i, width);
        let slot = tab_bar(&with, frame_w)[i];
        let d = (slot.0 as f64 + slot.1 as f64 / 2.0 - middle).abs();
        // Half a pixel's slack to the current place, so it doesn't flicker
        // between two equally near.
        let d = if Some(i) == current { d - 0.5 } else { d };
        if best.is_none_or(|(b, _, _)| d < b) {
            best = Some((d, i, slot));
        }
    }
    best.map(|(_, i, s)| (i, s)).unwrap_or((0, (0, width)))
}

/// Each window behind another of its group marked so, to show only its
/// tab: done before drawing (and once a turn, for the pointer), from the
/// stacking order then, so a tab clicked forward is drawn whole at once.
/// A window being put in a group by hand (`Decoration::docked`) shows
/// only its tab, and isn't its group's front until it has drawn at the
/// group's size.
pub fn mark_behind(space: &Space<WindowElement>) {
    let mut fronts: Vec<u64> = Vec::new();
    for w in space.elements().rev().filter(|w| w.decorated()) {
        let mut d = w.decoration();
        let behind = match d.group {
            _ if d.docked => false,
            Some(g) => fronts.contains(&g) || {
                fronts.push(g);
                false
            },
            None => false,
        };
        d.behind = behind;
    }
}

/// A dialog or panel, not a document window: it names a parent, or says
/// so with xdg-dialog (the kit's `WindowOptions::dialog`, Quick Look).
pub(super) fn is_dialog(t: &smithay::wayland::shell::xdg::ToplevelSurface) -> bool {
    use smithay::wayland::{compositor, shell::xdg::{XdgToplevelSurfaceData, dialog::ToplevelDialogHint}};
    t.parent().is_some()
        || compositor::with_states(t.wl_surface(), |s| {
            s.data_map.get::<XdgToplevelSurfaceData>().is_some_and(|d| d.lock().unwrap().dialog_hint != ToplevelDialogHint::Unknown)
        })
}

/// The frame size `w` was asked for and hasn't drawn yet, if any.
pub(super) fn size_on_the_way(w: &WindowElement) -> Option<Size<i32, Logical>> {
    let WindowSurface::Wayland(t) = w.0.underlying_surface();
    let asked = t.with_pending_state(|s| s.size)?;
    let drawn = t.with_committed_state(|s| s.and_then(|s| s.size));
    // (Outside the surface's lock: `outer_size` takes it too.)
    (drawn != Some(asked)).then(|| w.outer_size(asked))
}

/// `w` at `frame` and drawn at its size.
fn drawn_at(space: &Space<WindowElement>, w: &WindowElement, frame: Rectangle<i32, Logical>) -> bool {
    space.element_geometry(w) == Some(frame) && size_on_the_way(w).is_none()
}

/// Ask `w` again for the size it was asked for and drew otherwise, once
/// it has answered.
fn ask_again(w: &WindowElement) {
    let WindowSurface::Wayland(t) = w.0.underlying_surface();
    if t.is_initial_configure_sent() && !super::grabs::awaiting_ack(t) {
        t.send_configure();
    }
}

/// The smallest frame `w` can be (its app's minimum, with the tab and
/// border).
fn least_frame(w: &WindowElement) -> Size<i32, Logical> {
    use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
    let content = w.wl_surface().map(|s| with_states(&s, |st| st.cached_state.get::<SurfaceCachedState>().current().min_size)).unwrap_or_default();
    w.outer_size(content)
}

/// How long a window put in a group by hand shows only its tab, at most,
/// while it draws at the group's size.
const DOCKING: Duration = Duration::from_millis(400);

/// How long a window that joined a group by itself is given to draw at
/// the group's size before it may lead the group anyway (an app that
/// won't).
const SETTLING: Duration = Duration::from_secs(2);

impl<BackendData: Backend> AnvilState<BackendData> {
    /// A window in a group but not its front one: arranging commands
    /// move the front one and the rest follow.
    pub fn behind_in_group(&self, w: &WindowElement) -> bool {
        let mates = self.groups.mates(w);
        if mates.is_empty() {
            return false;
        }
        self.space.elements().rev().find(|o| mates.contains(o)).is_some_and(|front| front != w)
    }

    /// A window behind others in its group to the front of it, as
    /// clicking its tab does, before an arranging command is done to it:
    /// the group follows its front window, and would put it back.
    pub fn front_of_group(&mut self, w: &WindowElement) {
        if self.behind_in_group(w) {
            self.space.raise_element(w, false);
        }
    }

    /// Whether `app_id`'s windows are grouped (the default).
    pub fn groups_app(&self, app_id: &str) -> bool {
        !app_id.is_empty() && !self.settings.tiling.ungrouped_apps.iter().any(|a| a == app_id)
    }

    /// Whether `w` can be in a group: a document window (not a dialog,
    /// panel or fullscreen), once it has shown and been put in its place,
    /// which may be where it was before a restart (before then it isn't
    /// there yet, and would lead its group off to that place).
    pub fn groupable(&self, w: &WindowElement) -> bool {
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        w.meta().placed.get() && w.decorated() && !is_dialog(t) && resizable(w)
    }

    /// What `w` groups by, if anything.
    pub fn group_key(&self, w: &WindowElement) -> Option<GroupKey> {
        let grouping = w.meta().grouping.borrow().clone();
        match grouping {
            Grouping::Auto => {
                let app = w.app_id();
                self.groups_app(&app).then_some(GroupKey::App(app))
            }
            Grouping::Alone => None,
            Grouping::With(k) => Some(k),
        }
    }

    /// The groups on screen now, each back to front, with their keys.
    fn current_groups(&self) -> (Vec<WindowElement>, Vec<(GroupKey, Vec<WindowElement>)>) {
        let loose = self.tab_drag.as_ref().and_then(|d| d.loose().then(|| d.window.clone()));
        let seen: Vec<WindowElement> = self.space.elements().filter(|w| self.groupable(w)).cloned().collect();
        let mut by_key: Vec<(GroupKey, Vec<WindowElement>)> = Vec::new();
        // (Not the window being dragged on its own: it is in no group
        // until it is let go.)
        for w in seen.iter().filter(|w| Some(*w) != loose.as_ref()) {
            let Some(key) = self.group_key(w) else { continue };
            match by_key.iter_mut().find(|(k, _)| *k == key) {
                Some((_, g)) => g.push(w.clone()),
                None => by_key.push((key, vec![w.clone()])),
            }
        }
        (seen, by_key.into_iter().filter(|(_, g)| g.len() > 1).collect())
    }

    /// Hand-made groups down to one window, and guests of an app with no
    /// windows grouping by themselves any more: those windows on their
    /// own (they don't fall into another group by surprise).
    pub(super) fn dissolve_lone(&self) {
        let all = self.all_windows();
        let groupings: Vec<Grouping> = all.iter().map(|w| w.meta().grouping.borrow().clone()).collect();
        for (w, g) in all.iter().zip(&groupings) {
            let lone = match g {
                Grouping::With(GroupKey::Id(_)) => groupings.iter().filter(|o| *o == g).count() < 2,
                Grouping::With(GroupKey::App(a)) => {
                    !all.iter().zip(&groupings).any(|(o, og)| *og == Grouping::Auto && o.app_id() == *a)
                }
                _ => false,
            };
            if lone {
                *w.meta().grouping.borrow_mut() = Grouping::Alone;
            }
        }
    }

    /// What is left of a group when `leaving` leave it (taken out, put in
    /// another, closed): if they were guests in an app's group with none
    /// of that app's own windows left among them, a group of their own,
    /// as if made by hand (just one, on its own), rather than coming
    /// apart or going after that app's windows elsewhere.
    fn hand_down(&self, leaving: &[WindowElement]) {
        for (_, g) in &self.groups.list {
            if !g.iter().any(|w| leaving.contains(w)) {
                continue;
            }
            let rest: Vec<&WindowElement> = g.iter().filter(|w| !leaving.contains(w)).collect();
            if !matches!(rest.first().and_then(|w| self.group_key(w)), Some(GroupKey::App(_)))
                || rest.iter().any(|w| *w.meta().grouping.borrow() == Grouping::Auto)
            {
                continue;
            }
            let to = if rest.len() > 1 { Grouping::With(GroupKey::Id(new_group_id())) } else { Grouping::Alone };
            for w in rest {
                *w.meta().grouping.borrow_mut() = to.clone();
            }
        }
    }

    /// Bring each group together (see the module's notes).
    pub fn sync_groups(&mut self) {
        let all = self.all_windows();
        let closed: Vec<WindowElement> = self.groups.known.iter().filter(|w| !all.contains(w)).cloned().collect();
        if !closed.is_empty() {
            self.hand_down(&closed);
            self.dissolve_lone();
        }
        let (seen, groups) = self.current_groups();
        let before = std::mem::take(&mut self.groups);
        let resizing = self.resizing();
        let dragged = self.tab_drag.as_ref().map(|d| d.window.clone());
        let was_member = |w: &WindowElement| before.list.iter().any(|(_, g)| g.contains(w)) || before.away.iter().any(|(a, m)| a == w && *m);
        let was_seen = |w: &WindowElement| before.seen.contains(w) || before.away.iter().any(|(a, _)| a == w);
        let was_settling = |w: &WindowElement| before.settling.iter().find(|(s, _)| s == w).map(|(_, since)| *since);
        let mut numbers = before.numbers.clone();
        let mut list = Vec::new();
        let mut settling = Vec::new();
        for (key, g) in &groups {
            let leads = |w: &&WindowElement| !before.joining.contains(w) && was_settling(w).is_none();
            let leader = g
                .iter()
                .rev()
                .filter(leads)
                .find(|w| was_member(w))
                .or_else(|| g.iter().rev().filter(leads).find(|w| was_seen(w)))
                .unwrap_or(&g[g.len() - 1])
                .clone();
            let Some(mut frame) = self.space.element_geometry(&leader) else { continue };
            // Where it is going, not the size it is leaving: a window that
            // has just joined (opened, come back, arrived) leads from the
            // next turn, before it has drawn at the group's size, and the
            // others would follow its old one, for a frame (a flicker) or
            // for good if it left again first (Twitcher passing it over).
            if let Some(size) = size_on_the_way(&leader) {
                frame.size = size;
            }
            // No smaller than any of them can be (different apps' windows
            // put together by hand), so they are all one size: the leader
            // too, once no grab is sizing it.
            let least = g.iter().map(least_frame).fold(frame.size, |a, b| Size::from((a.w.max(b.w), a.h.max(b.h))));
            if least != frame.size && !resizing {
                frame.size = least;
                self.join_group(&leader, frame, resizing);
            }
            for w in g.iter().filter(|w| **w != leader) {
                if let Some(own) = w.meta().arrived.take()
                    && own != frame
                {
                    w.meta().joined.set(Some((own, frame)));
                }
                self.join_group(w, frame, resizing);
                self.share_arrangement(&leader, w);
                // New to the group by itself: it settles (see `Groups`),
                // asked again if its app drew another size anyway (its
                // own resize crossing the group's).
                let new = !was_member(w) && !before.joining.contains(w);
                if let Some(since) = was_settling(w).or_else(|| new.then(Instant::now))
                    && !drawn_at(&self.space, w, frame)
                {
                    let since = if resizing { Instant::now() } else { since };
                    if since.elapsed() < SETTLING {
                        settling.push((w.clone(), since));
                    }
                    if !resizing && size_on_the_way(w).is_none() && self.space.element_geometry(w).is_some_and(|g| g.size != frame.size) {
                        ask_again(w);
                    }
                }
            }
            self.keep_together(g);
            let number = match key {
                GroupKey::Id(n) => *n,
                GroupKey::App(app) => match numbers.iter().find(|(a, _)| a == app) {
                    Some((_, n)) => *n,
                    None => {
                        let n = new_group_id();
                        numbers.push((app.clone(), n));
                        n
                    }
                },
            };
            self.lay_tabs(g, frame);
            list.push((number, g.clone()));
        }
        for w in self.space.elements() {
            // (Before borrowing the decoration: `outer_size` borrows it.)
            let docked = w.decoration().docked;
            let drawn = docked && size_on_the_way(w).is_none();
            let mut d = w.decoration();
            d.group = list.iter().find(|(_, g)| g.contains(w)).map(|(n, _)| *n);
            // In now, and drawn at the group's size (or given long enough).
            if d.docked && Some(w) != dragged.as_ref() && d.docked_since.is_none_or(|t| drawn || t.elapsed() > DOCKING) {
                d.docked = false;
                d.docked_since = None;
            }
        }
        // Out of a group now (closed down to one, taken out, ungrouped,
        // minimized): its tab back to its own width, at the left; and a
        // group that came apart still on screen stepped out like a
        // cascade, so the windows it hid show. (Not the window being
        // dragged: its tab is the drag's.)
        let left: Vec<(WindowElement, u64)> = before
            .list
            .iter()
            .flat_map(|(n, g)| g.iter().map(move |w| (w.clone(), *n)))
            .filter(|(w, _)| !list.iter().any(|(_, g)| g.contains(w)) && Some(w) != dragged.as_ref())
            .collect();
        for (w, _) in &left {
            let mut d = w.decoration();
            if d.tab_width.take().is_some() {
                d.tab_offset = 0;
            }
        }
        let mut stacked: Vec<(u64, Rectangle<i32, Logical>)> = Vec::new();
        let mut apart: Vec<&(WindowElement, u64)> = left.iter().filter(|(w, _)| self.space.element_location(w).is_some()).collect();
        apart.sort_by_key(|(w, _)| w.id());
        for (w, n) in apart {
            let Some(frame) = self.space.element_geometry(w) else { continue };
            let under = stacked.iter().filter(|(o, f)| o == n && f.loc == frame.loc).count() as i32;
            stacked.push((*n, frame));
            if under > 0 {
                self.space.relocate_element(w, frame.loc + Point::from((under * CASCADE, under * CASCADE)));
            }
        }
        let away = before
            .seen
            .iter()
            .map(|w| (w.clone(), before.list.iter().any(|(_, g)| g.contains(w))))
            .chain(before.away.iter().cloned())
            .filter(|(w, _)| w.alive() && self.space.element_location(w).is_none() && !w.meta().minimized.get())
            .collect();
        // Only the turn it came back in counts as arriving.
        for w in self.space.elements() {
            w.meta().arrived.set(None);
        }
        // A window put in a group by hand is one of it from now on, if it
        // made it in (else it may still: it is being placed).
        let joining = before.joining.into_iter().filter(|w| w.alive() && !list.iter().any(|(_, g)| g.contains(w))).collect();
        self.groups = Groups { seen, list, away, known: all, numbers, joining, settling };
        mark_behind(&self.space);
    }

    /// A window's edge or corner being dragged: windows' sizes wait till
    /// it is let go. (Not any grab: a menu's lasts until the pointer next
    /// moves, and a button held down is one.)
    fn resizing(&self) -> bool {
        let pointer = self.pointer.with_grab(|_, g| g.is::<PointerResizeSurfaceGrab<BackendData>>());
        let touch = self.seat.get_touch().and_then(|t| t.with_grab(|_, g| g.is::<TouchResizeSurfaceGrab<BackendData>>()));
        pointer == Some(true) || touch == Some(true)
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

    /// The tabs side by side, in their order; while a tab is dragged
    /// along them, it where it is held and the others around the place it
    /// would take, and while one is held over them to be put in, the
    /// others around the place it would go, it in that place
    /// (`tabdrag`).
    pub(super) fn lay_tabs(&mut self, g: &[WindowElement], frame: Rectangle<i32, Logical>) {
        let tabs = in_tab_order(g);
        let wanted = |w: &WindowElement| {
            let title = w.title();
            w.decoration().wanted_tab_width(&title)
        };
        let (slots, held) = self.drag_slots(&tabs, frame);
        let widths: Vec<i32> = slots.iter().map(wanted).collect();
        for (w, (x, width)) in slots.iter().zip(tab_bar(&widths, frame.size.w)) {
            let mut d = w.decoration();
            d.tab_width = Some(width);
            d.tab_offset = match &held {
                Some((h, at)) if h == w => *at,
                _ => x,
            };
        }
    }

    /// Put `w` in `target`'s group (or a new one with it, if it is in
    /// none), its tab at `index` among the group's (or last), as dropping
    /// its tab there does. It takes the group's frame and is put in front.
    /// False if either can't be grouped, or they are one window.
    pub fn put_in_group(&mut self, w: &WindowElement, target: &WindowElement, index: Option<usize>) -> bool {
        if w == target || !self.groupable(w) || !self.groupable(target) || self.space.element_location(target).is_none() {
            return false;
        }
        self.hand_down(std::slice::from_ref(w));
        let mut mates = self.groups.mates(target);
        mates.retain(|o| o != w);
        let key = match self.group_key(target) {
            Some(k) if !mates.is_empty() || matches!(k, GroupKey::App(_)) => k,
            // On its own, or in a hand-made group with no one else here:
            // a new group of the two.
            _ => {
                let k = GroupKey::Id(new_group_id());
                *target.meta().grouping.borrow_mut() = Grouping::With(k.clone());
                k
            }
        };
        let app = w.app_id();
        *w.meta().grouping.borrow_mut() = if key == GroupKey::App(app.clone()) && self.groups_app(&app) { Grouping::Auto } else { Grouping::With(key) };
        let mut tabs = in_tab_order(if mates.is_empty() { std::slice::from_ref(target) } else { &mates });
        tabs.insert(index.unwrap_or(tabs.len()).min(tabs.len()), w.clone());
        set_tab_order(&tabs);
        let front = mates.last().unwrap_or(target).clone();
        if let Some(mut frame) = self.space.element_geometry(&front) {
            if let Some(size) = size_on_the_way(&front) {
                frame.size = size;
            }
            self.forget_tile(w);
            if self.space.element_location(w).is_some() {
                self.set_frame(w, frame, false);
            }
        }
        w.decoration().rolled = target.decoration().rolled;
        if !self.groups.joining.contains(w) {
            self.groups.joining.push(w.clone());
        }
        self.dissolve_lone();
        tracing::info!(id = w.id(), with = target.id(), "put in a group");
        true
    }

    /// Take `w` out of its group by hand: on its own from now on, its tab
    /// its own again. False if it is in none.
    pub fn take_out_of_group(&mut self, w: &WindowElement) -> bool {
        if !self.groups.has(w) {
            return false;
        }
        self.hand_down(std::slice::from_ref(w));
        *w.meta().grouping.borrow_mut() = Grouping::Alone;
        self.dissolve_lone();
        tracing::info!(id = w.id(), "taken out of its group");
        true
    }

    /// Take `w` out of its group, stepped out like a cascade so the group
    /// shows behind it, in front (agents, the tab's menu).
    pub fn ungroup_window(&mut self, w: &WindowElement) -> bool {
        let Some(frame) = self.space.element_geometry(w) else { return false };
        if !self.take_out_of_group(w) {
            return false;
        }
        {
            let mut d = w.decoration();
            d.tab_width = None;
            d.tab_offset = 0;
        }
        self.forget_tile(w);
        let to = self.reachable(Rectangle::new(frame.loc + Point::from((CASCADE, CASCADE)), frame.size));
        self.set_frame(w, to, false);
        self.focus_window(w);
        true
    }

    /// Move `w`'s tab to `index` among its group's. False if it is in none.
    pub fn move_tab(&mut self, w: &WindowElement, index: usize) -> bool {
        let mates = self.groups.mates(w);
        if mates.is_empty() {
            return false;
        }
        let mut tabs = in_tab_order(&mates);
        tabs.retain(|o| o != w);
        tabs.insert(index.min(tabs.len()), w.clone());
        set_tab_order(&tabs);
        true
    }

    /// Apps whose windows are grouped again (Group Windows turned back
    /// on): their windows taken out by hand go back in too.
    pub fn regroup_apps(&mut self, apps: &[String]) {
        for w in self.all_windows() {
            if apps.contains(&w.app_id()) && *w.meta().grouping.borrow() == Grouping::Alone {
                *w.meta().grouping.borrow_mut() = Grouping::Auto;
            }
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

    #[test]
    fn a_held_tab_goes_where_its_middle_is() {
        let others = [100, 100, 100];
        // Over the start, the middle, the end of a top with room.
        assert_eq!(tab_place(&others, 80, 600, 10.0, None), (0, (0, 80)));
        assert_eq!(tab_place(&others, 80, 600, 150.0, None).0, 1);
        assert_eq!(tab_place(&others, 80, 600, 239.0, None), (2, (200, 80)));
        assert_eq!(tab_place(&others, 80, 600, 1000.0, None), (3, (300, 80)));
        // Far to the left of everything: first.
        assert_eq!(tab_place(&others, 80, 600, -500.0, None).0, 0);
        // Into a group that has to share its top: slots alike.
        assert_eq!(tab_place(&[300, 300], 300, 600, 590.0, None), (2, (400, 200)));
        // Into no tabs: the only place.
        assert_eq!(tab_place(&[], 80, 600, 300.0, None), (0, (0, 80)));
    }

    #[test]
    fn a_tie_keeps_the_place_it_has() {
        // Middles at 50 (place 0) and 150 (place 1): 100 is as near both.
        let tabs = [100];
        assert_eq!(tab_place(&tabs, 100, 400, 100.0, Some(1)).0, 1);
        assert_eq!(tab_place(&tabs, 100, 400, 100.0, Some(0)).0, 0);
    }

    #[test]
    fn groupings_keep_through_a_restart() {
        for g in [Grouping::Auto, Grouping::Alone, Grouping::With(GroupKey::App("olo.terminal".into())), Grouping::With(GroupKey::Id(7))] {
            let text = serde_json::to_string(&g).unwrap();
            assert_eq!(serde_json::from_str::<Grouping>(&text).unwrap(), g, "{text}");
        }
        note_restored(&Grouping::With(GroupKey::Id(500)), 900);
        assert!(new_group_id() > 500);
        assert!(next_tab_order() > 900);
    }
}
