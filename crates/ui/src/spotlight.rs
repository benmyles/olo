//! A control lit for a moment, to say "here": where a setting the command
//! palette found is in its panel (as macOS's System Settings lights what
//! its search found), or what an agent asked to be shown. Found by its
//! name in the window's agent tree (`agent::tree`), on its tab (clicked
//! first, if it isn't the one showing) and in its box if it's named; lit
//! with a ring in the keyboard-navigation colour that comes, stays a
//! moment and goes, drawn by the window's [`crate::Frame`] over all else.
//! Nothing runs once it's gone.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, App, Bounds, Global, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, PlatformInput, Window, WindowId, point, px};

use crate::{style, theme};

/// How long a control stays lit, coming and going included.
pub const LIT: Duration = Duration::from_millis(1600);
const COMING: f32 = 0.15;
const GOING: f32 = 0.45;

/// Each window's lit control: where, since when, and whether a timer
/// will wake it for its going (it's still while held: no frames then).
#[derive(Default)]
struct Lit(HashMap<WindowId, (Bounds<Pixels>, Instant, bool)>);

impl Global for Lit {}

/// Light the control called `name` in window `handle` (a label's colon
/// aside), on tab `tab` and in the box called `group` where given (one in
/// no such box is taken if that's all there is: a box's name can be
/// generic, "Display" for a display's). False if there's none.
pub fn show(handle: AnyWindowHandle, tab: Option<&str>, group: Option<&str>, name: &str, cx: &mut App) -> bool {
    let Some(tree) = crate::agent::tree(handle, cx) else { return false };
    let (group, name) = (group.filter(|g| !g.is_empty()).map(str::to_owned), name.trim_end_matches(':').trim().to_owned());
    if let Some(tab) = tab.filter(|t| !t.is_empty())
        && let Some(t) = tree.nodes.iter().find(|n| n.role == "Tab" && n.name.as_deref() == Some(tab))
        && t.selected != Some(true)
    {
        // Its tab first, as a click would choose it; the control is looked
        // for once that has drawn.
        let at = point(px(t.x + t.width / 2.0), px(t.y + t.height / 2.0));
        let _ = handle.update(cx, |_, window, cx| {
            click(window, at, cx);
            // (Each frame asked for: one waited for that nothing asks for
            // is a second away, the compositor's at the least.)
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |window, cx| {
                    let handle = window.window_handle();
                    cx.defer(move |cx| {
                        light(handle, group.as_deref(), &name, cx);
                    });
                });
                window.refresh();
            });
            window.refresh();
        });
        return true;
    }
    light(handle, group.as_deref(), &name, cx)
}

fn click(window: &mut Window, at: gpui::Point<Pixels>, cx: &mut App) {
    let down = MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1, first_mouse: false };
    window.dispatch_event(PlatformInput::MouseDown(down), cx);
    window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1 }), cx);
}

fn light(handle: AnyWindowHandle, group: Option<&str>, name: &str, cx: &mut App) -> bool {
    let Some(tree) = crate::agent::tree(handle, cx) else { return false };
    let named = |n: &&olo_kit_base::commands::UiNode| n.role != "Group" && n.role != "Window" && !n.offscreen && n.name.as_deref().map(|s| s.trim_end_matches(':').trim()) == Some(name);
    let inside = |n: &olo_kit_base::commands::UiNode, o: &olo_kit_base::commands::UiNode| {
        let (x, y) = (n.x + n.width / 2.0, n.y + n.height / 2.0);
        o.x <= x && x <= o.x + o.width && o.y <= y && y <= o.y + o.height
    };
    let in_group = |n: &olo_kit_base::commands::UiNode| {
        group.is_none_or(|g| {
            tree.nodes.iter().filter(|o| o.role == "Group" && inside(n, o)).min_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height))).and_then(|o| o.name.as_deref()) == Some(g)
        })
    };
    let Some(n) = tree.nodes.iter().filter(named).find(|n| in_group(n)).or_else(|| tree.nodes.iter().find(named)) else { return false };
    let bounds = Bounds { origin: point(px(n.x), px(n.y)), size: gpui::size(px(n.width), px(n.height)) };
    cx.default_global::<Lit>().0.insert(handle.window_id(), (bounds, Instant::now(), false));
    let _ = handle.update(cx, |_, window, _| window.refresh());
    true
}

/// The lit control's ring, if `window` has one now: drawn last by the
/// window's frame, asking for frames only while it shows.
pub(crate) fn paint(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let Some(&(b, since, woken)) = cx.try_global::<Lit>().and_then(|l| l.0.get(&id)) else { return };
    let t = since.elapsed().as_secs_f32();
    let total = LIT.as_secs_f32();
    if t >= total {
        cx.global_mut::<Lit>().0.remove(&id);
        return;
    }
    // In, held, out.
    let a = if t < COMING { t / COMING } else if t > total - GOING { (total - t) / GOING } else { 1.0 };
    let p = theme::palette(cx);
    let mut ring = style::c(p.navigation_base);
    let mut wash = ring;
    ring.a *= a;
    wash.a *= 0.12 * a;
    // (On device pixels, two lines wide: as sharp at 1.5× as at 2×.)
    let r = style::snap(window, Bounds::from_corners(point(b.left() - px(4.0), b.top() - px(3.0)), point(b.right() + px(4.0), b.bottom() + px(3.0))));
    window.paint_quad(gpui::quad(r, px(5.0), wash, style::hairline(window) * 2.0, ring, gpui::BorderStyle::Solid));
    // Frames while it comes and goes; held, it's still: the window is
    // woken once, for its going.
    let held = t >= COMING && t < total - GOING;
    if !held {
        window.request_animation_frame();
    } else if !woken {
        if let Some(l) = cx.global_mut::<Lit>().0.get_mut(&id) {
            l.2 = true;
        }
        let wait = Duration::from_secs_f32(total - GOING - t);
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(wait).await;
                let _ = cx.update(|window, _| window.refresh());
            })
            .detach();
    }
}
