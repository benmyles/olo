//! Text that doesn't show in full (cut short with "…" to fit, or cut by
//! the edge of what it's in): its whole shown in a tip when the pointer
//! rests on it (`tip`, after the usual pause), at once while ⌘ (the Super
//! key) is held. Every kit window does it (its `Frame` follows the
//! pointer) for every text the kit cuts (`style::label_fit`,
//! `label_runs_fit`, a label running past its clip), and for what an app
//! cuts itself and says so ([`note`]).
//!
//! What was cut is kept with the frame (a paint record: replayed with a
//! cached view's paint), and looked through only when the pointer moves
//! after a frame was drawn; between frames, its few rectangles.

use std::{collections::HashMap, rc::Rc};

use gpui::{App, Bounds, Global, Pixels, Point, SharedString, Window, WindowId, px};

/// Text shown cut short where it was painted.
struct Cut {
    full: SharedString,
    bounds: Bounds<Pixels>,
}

/// `full` was shown cut short in `bounds` (in the window, as painted).
/// What shows of it is where the pointer finds it.
pub fn note(window: &mut Window, full: SharedString, bounds: Bounds<Pixels>) {
    let shown = bounds.intersect(&window.content_mask().bounds);
    if shown.size.width > px(0.0) && shown.size.height > px(0.0) && !full.trim().is_empty() {
        window.push_paint_record(Rc::new(Cut { full, bounds: shown }));
    }
}

/// A label `text` painted at `bounds`: noted if what it's painted in cuts
/// it at a side (a row scrolled half out of view, top or bottom, isn't).
pub(crate) fn note_if_clipped(window: &mut Window, text: &str, bounds: Bounds<Pixels>) {
    let clip = window.content_mask().bounds;
    // (A pixel's grace: a label flush against its clip isn't cut.)
    let cut = bounds.left() < clip.left() - px(1.0) || bounds.right() > clip.right() + px(1.0);
    let seen = bounds.top() < clip.bottom() && bounds.bottom() > clip.top();
    if cut && seen {
        note(window, SharedString::from(text.to_owned()), bounds);
    }
}

/// Whether ⌘ (the Super key) is held: app_server sends it as the right
/// Ctrl with a profile that has a command key (Terminal tells it so
/// too); else Super itself.
pub(crate) fn command_held(m: &gpui::Modifiers) -> bool {
    m.platform || (m.control && gpui::right_control_held() && olo_kit_base::keys::command_key())
}

/// Each window's cut texts as last drawn, and the one the pointer is on.
#[derive(Default)]
struct Cuts(HashMap<WindowId, Seen>);

impl Global for Cuts {}

#[derive(Default)]
struct Seen {
    /// A frame was drawn since `cuts` was made.
    stale: bool,
    cuts: Vec<(Bounds<Pixels>, SharedString)>,
    /// The one under the pointer: its tip's key.
    over: Option<SharedString>,
}

/// A frame of `window` is being drawn: what's cut is looked for again
/// when the pointer next moves.
pub(crate) fn painted(window: &Window, cx: &mut App) {
    if let Some(seen) = cx.try_global::<Cuts>().is_some().then(|| cx.global_mut::<Cuts>()).and_then(|c| c.0.get_mut(&window.window_handle().window_id())) {
        seen.stale = true;
    }
}

/// The pointer moved to `at` in `window` (with ⌘ held, `now`: its tip
/// at once).
pub(crate) fn moved(window: &mut Window, cx: &mut App, at: Point<Pixels>, now: bool) {
    let id = window.window_handle().window_id();
    let (under, key, was) = {
        let seen = cx.default_global::<Cuts>().0.entry(id).or_insert_with(|| Seen { stale: true, ..Default::default() });
        if seen.stale {
            seen.stale = false;
            seen.cuts = window.paint_records().iter().filter_map(|(r, moved)| r.downcast_ref::<Cut>().map(|c| (Bounds { origin: c.bounds.origin + *moved, size: c.bounds.size }, c.full.clone()))).collect();
        }
        // (The last painted: the one on top.)
        let under = seen.cuts.iter().rev().find(|(b, _)| b.contains(&at)).cloned();
        let key = under.as_ref().map(|(b, full)| SharedString::from(format!("cut:{:?}:{full}", b.origin)));
        if key == seen.over {
            // The same: its tip as it is, unless ⌘ asks for it now.
            if now && let (Some(key), Some((b, full))) = (key, under) {
                crate::tip::show_now(window, cx, key, full, b);
            }
            return;
        }
        let was = std::mem::replace(&mut seen.over, key.clone());
        (under, key, was)
    };
    if let Some(was) = was {
        crate::tip::hover(window, cx, was, "", Bounds::default(), false);
    }
    if let (Some(key), Some((b, full))) = (key, under) {
        if now {
            crate::tip::show_now(window, cx, key, full, b);
        } else {
            crate::tip::hover(window, cx, key, full, b, true);
        }
    }
}

/// ⌘ (Super) went down or up: down on cut text, its tip at once.
pub(crate) fn modifiers(window: &mut Window, cx: &mut App, at: Point<Pixels>, held: bool) {
    if held {
        // (As if the pointer moved there, with ⌘.)
        if let Some(seen) = cx.try_global::<Cuts>().and_then(|c| c.0.get(&window.window_handle().window_id()))
            && seen.over.is_some()
        {
            moved(window, cx, at, true);
        }
    }
}

/// The pointer left `window`, or pressed: the tip goes (pressed, until
/// the pointer leaves the text).
pub(crate) fn left(window: &Window, cx: &mut App, pressed: bool) {
    let Some(seen) = cx.try_global::<Cuts>().is_some().then(|| cx.global_mut::<Cuts>()).and_then(|c| c.0.get_mut(&window.window_handle().window_id())) else { return };
    let Some(key) = (if pressed { seen.over.clone() } else { seen.over.take() }) else { return };
    if pressed {
        crate::tip::pressed(window, cx, key);
    } else {
        crate::tip::hover(window, cx, key, "", Bounds::default(), false);
    }
}

/// Windows closed: what they showed goes.
pub(crate) fn forget_closed(cx: &mut App) {
    if cx.try_global::<Cuts>().is_some() {
        let open: std::collections::HashSet<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
        cx.global_mut::<Cuts>().0.retain(|id, _| open.contains(id));
    }
}
