//! A program hiding the pointer, and when Olo goes along with it.
//!
//! A program may ask for no pointer (`wl_pointer.set_cursor` with no
//! surface). Players of video do when the pointer has rested over the
//! picture a few seconds, and show it again when it moves; a program
//! that draws a pointer of its own hides the system's as it comes in;
//! a game holds the pointer and hides it. The rule here (Ben,
//! 2026-11-18: "the cursor shouldn't disappear when over a video at
//! all, it should only ever disappear if in full screen and hasn't
//! moved for 5s and then come back if it moves again"):
//!
//! - A pointer hidden **while it rested** (it had been shown since it
//!   came to the program, and hadn't moved for a second: `Kind::Resting`)
//!   is hidden only over a full-screen window, once it has been still
//!   five seconds (`STILL`), and shows again the moment it moves,
//!   whatever the program says or doesn't: a program whose asking for
//!   it back never arrived left the pointer gone until it left the
//!   window and came back (Chrome and Slack over a video).
//! - A pointer hidden **as it came in or while it moved** (`Kind::Own`:
//!   the program draws its own) stays hidden, as asked.
//! - A pointer a window **holds** (locked or confined) stays hidden,
//!   as asked.
//!
//! `AnvilState::cursor_status` is what's shown; this keeps what the
//! program asked.

use std::time::{Duration, Instant};

use smithay::{
    input::pointer::CursorImageStatus,
    reexports::calloop::timer::{TimeoutAction, Timer},
    wayland::{compositor::get_parent, pointer_constraints::with_pointer_constraint, seat::WaylandFocus},
};

use crate::state::{AnvilState, Backend};

/// How long a pointer is still over a full-screen window before a
/// program's wish to hide it is granted.
pub const STILL: Duration = Duration::from_secs(5);
/// A pointer hidden after this long without moving was hidden for
/// resting; sooner, it's the program's own pointer taking its place.
pub const RESTED: Duration = Duration::from_secs(1);

/// Why a program hid the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// It draws its own (hidden as the pointer came in, or while it moved).
    Own,
    /// The pointer rested (a video's player).
    Resting,
}

/// What the program under the pointer asked of the pointer's look.
#[derive(Debug)]
pub struct Hide {
    /// The last look it asked for that can be seen.
    last_shown: Option<CursorImageStatus>,
    /// It has shown a pointer since the pointer came to it.
    shown_since_enter: bool,
    /// It asked for none: why.
    hidden: Option<Kind>,
    /// When the pointer last moved.
    moved: Instant,
    /// The timer for `STILL` is set.
    timer: bool,
}

impl Default for Hide {
    fn default() -> Self {
        Hide { last_shown: None, shown_since_enter: false, hidden: None, moved: Instant::now(), timer: false }
    }
}

/// Why a pointer was hidden, from whether the program had shown one
/// since the pointer came to it and how long the pointer had been still.
pub fn kind(shown_since_enter: bool, still: Duration) -> Kind {
    if shown_since_enter && still >= RESTED { Kind::Resting } else { Kind::Own }
}

/// Whether a pointer a program hid is hidden now.
pub fn hidden_now(kind: Kind, held: bool, fullscreen: bool, still: Duration) -> bool {
    match kind {
        Kind::Own => true,
        Kind::Resting => held || (fullscreen && still >= STILL),
    }
}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    /// A look for the pointer, asked by the program under it (or the
    /// library's own arrow, as the pointer leaves one).
    pub fn client_cursor(&mut self, image: CursorImageStatus) {
        // Between a leave and the next enter: the library's arrow for a
        // pointer over no program. What the last one asked is forgotten.
        if self.pointer.last_enter().is_none() {
            self.pointer_hide = Hide { moved: self.pointer_hide.moved, timer: self.pointer_hide.timer, ..Hide::default() };
            self.cursor_status = image;
            return;
        }
        match image {
            CursorImageStatus::Hidden => {
                let hide = &mut self.pointer_hide;
                // (Asked again, it's for the reason it was.)
                if hide.hidden.is_none() {
                    hide.hidden = Some(kind(hide.shown_since_enter, hide.moved.elapsed()));
                }
                self.pointer_hide_apply();
            }
            shown => {
                let hide = &mut self.pointer_hide;
                hide.hidden = None;
                hide.shown_since_enter = true;
                hide.last_shown = Some(shown.clone());
                self.cursor_status = shown;
            }
        }
    }

    /// The pointer moved (a mouse, a touchpad, a synthetic move).
    pub fn pointer_moved(&mut self) {
        self.pointer_hide.moved = Instant::now();
        if self.pointer_hide.hidden == Some(Kind::Resting) && matches!(self.cursor_status, CursorImageStatus::Hidden) {
            self.pointer_hide_apply();
        }
    }

    /// A pointer its program hid is shown or hidden as the rule has it
    /// now (after it's asked, the pointer moved, a window went to or from
    /// full screen, or `STILL` passed).
    pub fn pointer_hide_apply(&mut self) {
        let Some(kind) = self.pointer_hide.hidden else { return };
        let still = self.pointer_hide.moved.elapsed();
        let (held, fullscreen) = match kind {
            Kind::Own => (false, false),
            Kind::Resting => self.pointer_held_and_over_fullscreen(),
        };
        let hide = hidden_now(kind, held, fullscreen, still);
        let status = if hide {
            CursorImageStatus::Hidden
        } else {
            match &self.pointer_hide.last_shown {
                Some(CursorImageStatus::Surface(surface)) if !smithay::utils::IsAlive::alive(surface) => CursorImageStatus::default_named(),
                Some(shown) => shown.clone(),
                None => CursorImageStatus::default_named(),
            }
        };
        if status != self.cursor_status {
            self.cursor_status = status;
            // (Nothing else may draw a frame for it: a timer's hiding.)
            self.wake_under_pointer(self.pointer.current_location());
        }
        // Shown over a full-screen window for now: looked at again when
        // it will have been still long enough (one timer; a pointer that
        // moves meanwhile makes it look once more, never per move).
        if kind == Kind::Resting && !hide && fullscreen && !self.pointer_hide.timer {
            self.pointer_hide.timer = true;
            let after = STILL.saturating_sub(still) + Duration::from_millis(20);
            let _ = self.handle.insert_source(Timer::from_duration(after), |_, _, state| {
                state.pointer_hide.timer = false;
                state.pointer_hide_apply();
                TimeoutAction::Drop
            });
        }
    }

    /// Whether the window under the pointer holds it (a lock, a fence),
    /// and whether it's full screen.
    fn pointer_held_and_over_fullscreen(&self) -> (bool, bool) {
        let Some(surface) = self.pointer.current_focus().and_then(|focus| focus.wl_surface().map(|s| s.into_owned())) else { return (false, false) };
        let held = with_pointer_constraint(&surface, &self.pointer, |constraint| constraint.is_some_and(|c| c.is_active()));
        let mut root = surface;
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        (held, self.window_for_surface(&root).is_some_and(|w| crate::shell::autotile::fullscreen(&w)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    /// A video's player hides a pointer that rested; a program with its
    /// own pointer hides the system's as it comes in or while it moves.
    #[test]
    fn why_it_was_hidden() {
        assert_eq!(kind(true, s(3)), Kind::Resting);
        assert_eq!(kind(true, RESTED), Kind::Resting);
        // (Moving, or just come in: the program's own takes its place.)
        assert_eq!(kind(true, Duration::from_millis(200)), Kind::Own);
        assert_eq!(kind(false, s(30)), Kind::Own);
        assert_eq!(kind(false, Duration::ZERO), Kind::Own);
    }

    /// The rule: a rested pointer is hidden only over a full-screen
    /// window once it's been still five seconds, or where a window
    /// holds it; a program's own pointer always takes the system's place.
    #[test]
    fn when_it_is_hidden() {
        // In a window: never, however long it rests.
        for still in [s(0), s(3), s(5), s(600)] {
            assert!(!hidden_now(Kind::Resting, false, false, still), "{still:?}");
        }
        // Full screen: after five seconds still, and back as it moves.
        assert!(!hidden_now(Kind::Resting, false, true, s(3)));
        assert!(!hidden_now(Kind::Resting, false, true, Duration::from_millis(4999)));
        assert!(hidden_now(Kind::Resting, false, true, STILL));
        assert!(hidden_now(Kind::Resting, false, true, s(60)));
        assert!(!hidden_now(Kind::Resting, false, true, s(0)));
        // Held by its window (a game): hidden as asked, moving or not.
        assert!(hidden_now(Kind::Resting, true, false, s(0)));
        assert!(hidden_now(Kind::Resting, true, true, s(0)));
        // The program's own pointer: hidden as asked, wherever.
        for (held, fullscreen) in [(false, false), (false, true), (true, false)] {
            assert!(hidden_now(Kind::Own, held, fullscreen, s(0)));
        }
    }
}
