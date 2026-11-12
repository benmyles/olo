//! A control lit for a moment, to say "here": where a setting the command
//! palette found is in its panel (as macOS's System Settings lights what
//! its search found), or what an agent asked to be shown. Found by its
//! name in the window's agent tree, in its box if it's named; lit with a
//! ring in the keyboard-navigation colour that comes, stays a moment and
//! goes, drawn by the window's [`crate::Frame`] over all else. Nothing
//! runs once it's gone. A control on a tab that isn't showing:
//! [`show_tab`] chooses the tab as a click would, and the app lights the
//! control once that has drawn.

use std::time::{Duration, Instant};

use olo_interface::{BorderStyle, Bounds, Element, ParentElement as _, Pixels, Styled as _, WindowCx, canvas, div, point, px, quad, size};
use olo_kit_base::commands::UiNode;

use crate::{app::FrameMessage, style, theme};

/// How long a control stays lit, coming and going included.
pub const LIT: Duration = Duration::from_millis(1600);
const COMING: f32 = 0.15;
const GOING: f32 = 0.45;

/// Light the control called `name` in this window (a label's colon
/// aside), in the box called `group` where given (one in no such box is
/// taken if that's all there is: a box's name can be generic, "Display"
/// for a display's). False if there's none.
pub fn show(cx: &mut WindowCx, group: Option<&str>, name: &str) -> bool {
    let tree = crate::agent::tree(cx);
    let (group, name) = (group.filter(|g| !g.is_empty()), name.trim_end_matches(':').trim());
    let named = |n: &&UiNode| n.role != "Group" && n.role != "Window" && !n.offscreen && n.name.as_deref().map(|s| s.trim_end_matches(':').trim()) == Some(name);
    let inside = |n: &UiNode, o: &UiNode| {
        let (x, y) = (n.x + n.width / 2.0, n.y + n.height / 2.0);
        o.x <= x && x <= o.x + o.width && o.y <= y && y <= o.y + o.height
    };
    let in_group = |n: &UiNode| {
        group.is_none_or(|g| {
            tree.nodes.iter().filter(|o| o.role == "Group" && inside(n, o)).min_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height))).and_then(|o| o.name.as_deref()) == Some(g)
        })
    };
    let Some(n) = tree.nodes.iter().filter(named).find(|n| in_group(n)).or_else(|| tree.nodes.iter().find(named)) else { return false };
    let bounds = Bounds { origin: point(px(n.x), px(n.y)), size: size(px(n.width), px(n.height)) };
    cx.send_root::<crate::Frame>(FrameMessage::Light(bounds));
    true
}

/// Chooses the tab called `tab` in this window, as a click on it would,
/// if it isn't the one showing. True if there is such a tab (showing
/// already or not).
pub fn show_tab(cx: &mut WindowCx, tab: &str) -> bool {
    let tree = crate::agent::tree(cx);
    let Some(t) = tree.nodes.iter().find(|n| n.role == "Tab" && n.name.as_deref() == Some(tab)) else { return false };
    if t.selected != Some(true) {
        cx.click_at(point(px(t.x + t.width / 2.0), px(t.y + t.height / 2.0)));
    }
    true
}

/// Whether the tab called `tab` is the one showing in this window.
pub fn tab_selected(cx: &mut WindowCx, tab: &str) -> bool {
    crate::agent::tree(cx).nodes.iter().any(|n| n.role == "Tab" && n.name.as_deref() == Some(tab) && n.selected == Some(true))
}

/// The lit control's ring over the window (its frame's last child),
/// lit `since`: it asks to be drawn again only while it comes and goes,
/// and once for its going.
pub(crate) fn ring(b: Bounds<Pixels>, since: Instant) -> Element {
    let ring = canvas(move |g, _| {
        let t = since.elapsed().as_secs_f32();
        let total = LIT.as_secs_f32();
        if t >= total {
            return;
        }
        // In, held, out.
        let a = if t < COMING {
            t / COMING
        } else if t > total - GOING {
            (total - t) / GOING
        } else {
            1.0
        };
        let p = theme::palette();
        let mut ring = style::c(p.navigation_base);
        let mut wash = ring;
        ring.a *= a;
        wash.a *= 0.12 * a;
        // (On device pixels, two lines wide: as sharp at 1.5× as at 2×.)
        let r = style::snap(g, Bounds::from_corners(point(b.left() - px(4.0), b.top() - px(3.0)), point(b.right() + px(4.0), b.bottom() + px(3.0))));
        let line = style::hairline(g) * 2.0;
        g.paint_quad(quad(r, px(5.0), wash, line, ring, BorderStyle::Solid));
        // Frames while it comes and goes; held, it's still, and drawn
        // again once, for its going.
        let held = t >= COMING && t < total - GOING;
        g.paint_again_within(if held { f64::from(total - GOING - t) } else { 1.0 / 120.0 });
    })
    // (Another drawing each time it's lit, and where.)
    .paint_key(style::key((since, [b.origin.x, b.origin.y, b.size.width, b.size.height].map(|v| f32::from(v).to_bits()))));
    div().absolute().inset_0().child(ring.size_full())
}
