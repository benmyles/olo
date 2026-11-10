//! Text that doesn't show in full (cut short with "…" to fit, or cut by
//! the edge of what it's in): its whole shown in a tip when the pointer
//! rests on it (`tip`, after the usual pause), at once while ⌘ (the Super
//! key) is held. Every kit window does it (its `Frame` follows the
//! pointer) for every text the kit cuts (`style::label_fit`,
//! `label_runs_fit`, a label running past its clip), for text elements
//! cut so (a `div().child(text)`), and for what an app cuts itself and
//! says so ([`note`]).
//!
//! What was cut is kept with what drew it (`Painter::note_cut`), and
//! looked through only when the pointer moves after the window showed
//! something new; between those, its few rectangles.

use olo_interface::{Bounds, Modifiers, Painter, Pixels, Point, SharedString, WindowCx};

/// `full` was shown cut short in `bounds` (in the window, as painted).
/// What shows of it is where the pointer finds it.
pub fn note(g: &mut Painter, full: SharedString, bounds: Bounds<Pixels>) {
    g.note_cut(full, bounds);
}

/// Whether ⌘ (the Super key) is held: app_server sends it as the right
/// Ctrl with a profile that has a command key (Terminal tells it so
/// too); else Super itself.
pub(crate) fn command_held(m: &Modifiers) -> bool {
    m.platform || (m.control && olo_interface::right_control_held() && olo_kit_base::keys::command_key())
}

/// A window's cut texts as last looked for, and the one the pointer is
/// on.
#[derive(Default)]
struct Seen {
    /// The window's commit `cuts` was made at.
    at: Option<u64>,
    cuts: Vec<(Bounds<Pixels>, SharedString)>,
    /// The one under the pointer: its tip's key.
    over: Option<SharedString>,
}

/// A cut text's tip's key: where it is, and what it says.
fn key_of(b: &Bounds<Pixels>, full: &SharedString) -> SharedString {
    SharedString::from(format!("cut:{:?}:{full}", b.origin))
}

/// The pointer moved to `at` in the window (with ⌘ held, `now`: its tip
/// at once).
pub(crate) fn moved(cx: &mut WindowCx, at: Point<Pixels>, now: bool) {
    let commit = cx.commit_count();
    if cx.window_state::<Seen>().at != Some(commit) {
        let cuts = cx.cut_texts();
        let seen = cx.window_state::<Seen>();
        seen.at = Some(commit);
        seen.cuts = cuts;
    }
    let (under, key, was) = {
        let seen = cx.window_state::<Seen>();
        // (The last painted: the one on top.)
        let under = seen.cuts.iter().rev().find(|(b, _)| b.contains(&at)).cloned();
        let key = under.as_ref().map(|(b, full)| key_of(b, full));
        if key == seen.over {
            // The same: its tip as it is, unless ⌘ asks for it now.
            if now && let (Some(key), Some((b, full))) = (key, under) {
                crate::tip::show_now(cx, key, full, b);
            }
            return;
        }
        let was = std::mem::replace(&mut seen.over, key.clone());
        (under, key, was)
    };
    if let Some(was) = was {
        crate::tip::hover(cx, was, "", Bounds::default(), false);
    }
    if let (Some(key), Some((b, full))) = (key, under) {
        if now {
            crate::tip::show_now(cx, key, full, b);
        } else {
            crate::tip::hover(cx, key, full, b, true);
        }
    }
}

/// ⌘ (Super) went down or up: down on cut text, its tip at once.
pub(crate) fn modifiers(cx: &mut WindowCx, at: Point<Pixels>, held: bool) {
    if held && cx.window_state::<Seen>().over.is_some() {
        // (As if the pointer moved there, with ⌘.)
        moved(cx, at, true);
    }
}

/// The pointer left the window, or pressed: the tip goes (pressed,
/// until the pointer leaves the text).
pub(crate) fn left(cx: &mut WindowCx, pressed: bool) {
    let seen = cx.window_state::<Seen>();
    let Some(key) = (if pressed { seen.over.clone() } else { seen.over.take() }) else { return };
    if pressed {
        crate::tip::pressed(cx, key);
    } else {
        crate::tip::hover(cx, key, "", Bounds::default(), false);
    }
}
