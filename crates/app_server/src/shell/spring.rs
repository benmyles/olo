//! Spring-loaded tabs: something dragged (a file, text, a picture) and
//! held a moment over a window's yellow tab brings that window forward,
//! to the front of its group if it's in one, so what's dragged can be
//! dropped into it (Ben, 2026-11-09: "when i drag a file and hover over
//! the title yellow tab of another window (whether in a group or on its
//! own) it should switch to that window so i can drop somewhere in
//! there"), as Finder's spring-loaded folders open, and as BeOS let a
//! drag find the window it's for.
//!
//! The drag goes on: the window comes forward under it, and the next
//! motion finds what's in it to drop on. Nothing waits on a timer while
//! nothing is dragged; a drag that leaves the tab before the moment is up
//! brings nothing forward.

use std::time::Duration;

use smithay::{
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::IsAlive as _,
};

use super::{SSD, WindowElement, deco::Part};
use crate::{
    focus::PointerFocusTarget,
    state::{AnvilState, Backend},
};

/// How long a drag rests on a tab before its window comes forward (as a
/// Mac's spring-loading, "medium").
pub const SPRING: Duration = Duration::from_millis(600);

/// The tab a drag is resting on: its window, and which wait is its.
#[derive(Debug, Clone)]
pub struct Spring {
    pub window: WindowElement,
    pub token: u64,
}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// The window whose tab is under the pointer, if any (the tab that
    /// shows there: one in front covers those behind).
    fn tab_under_pointer(&self) -> Option<WindowElement> {
        let pos = self.pointer.current_location();
        let (PointerFocusTarget::SSD(SSD(w)), loc) = self.surface_under(pos)? else { return None };
        let (title, content) = (w.title(), w.content_size());
        let part = w.decoration().hit(&title, content, pos - loc);
        (part == Part::Tab).then_some(w)
    }

    /// Already where a drop goes: the window in front, with the keyboard
    /// (and in front of its group).
    fn forward_already(&self, w: &WindowElement) -> bool {
        let front = self.space.elements().next_back().is_some_and(|f| f == w);
        front && !self.behind_in_group(w) && self.seat.get_keyboard().and_then(|k| k.current_focus()).is_some_and(|f| f == crate::focus::KeyboardFocusTarget::from(w.clone()))
    }

    /// The pointer moved: while something is dragged, a tab it rests on
    /// is waited on, and its window comes forward once the moment is up.
    pub fn spring_pointer(&mut self) {
        if !self.dragging {
            self.spring = None;
            return;
        }
        let Some(w) = self.tab_under_pointer().filter(|w| !self.forward_already(w)) else {
            self.spring = None;
            return;
        };
        if self.spring.as_ref().is_some_and(|s| s.window == w) {
            return;
        }
        self.spring_token += 1;
        let token = self.spring_token;
        self.spring = Some(Spring { window: w, token });
        let _ = self.handle.insert_source(Timer::from_duration(SPRING), move |_, _, state| {
            state.spring_due(token);
            TimeoutAction::Drop
        });
    }

    /// The moment is up: if the drag is still on the same tab, its window
    /// comes forward, as a click on its tab would bring it.
    fn spring_due(&mut self, token: u64) {
        let Some(s) = self.spring.take_if(|s| s.token == token) else { return };
        if !self.dragging || self.tab_under_pointer().as_ref() != Some(&s.window) || !s.window.alive() {
            return;
        }
        tracing::info!(window = s.window.id(), app_id = s.window.app_id(), "a drag rested on its tab: to the front, to drop in");
        self.focus_window(&s.window);
        // (What's under the drag now: the window's content, for the drop.)
        let at = self.pointer.current_location();
        self.warp_pointer(at);
    }
}
