//! BTabView, as the egui kit draws it: BeOS folder tabs, slanted at
//! their sides, over a raised panel holding what the chosen tab shows;
//! the chosen tab is in front, open into the panel, its name in bold. A
//! click on a tab chooses it, and so do Next and Previous Tab (⌃Tab and
//! ⌃⇧Tab, ⌃Page Down and ⌃Page Up) wherever the keyboard is in the
//! window, unless what has it takes them (a screen being controlled):
//! the first tab view drawn in the window has them. Agents find each tab
//! as a `Tab`, selected or not.
//!
//! ```ignore
//! TabView::new("tabs", vec!["Live".into(), "History".into()], chosen).on_select(move |i, _, cx| …).child(content)
//! ```

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, FontWeight, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent, ParentElement, PathBuilder, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas, div, point, px, size,
};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{agent, style, theme};

/// The tabs' row.
pub const TAB_HEIGHT: f32 = 22.0;
/// How far a tab's side slants across.
const SLANT: f32 = 6.0;

type SelectFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct TabView {
    id: SharedString,
    tabs: Rc<[SharedString]>,
    selected: usize,
    on_select: Option<SelectFn>,
    children: SmallVec<[AnyElement; 2]>,
}

impl TabView {
    pub fn new(id: impl Into<SharedString>, tabs: impl Into<Rc<[SharedString]>>, selected: usize) -> TabView {
        TabView { id: id.into(), tabs: tabs.into(), selected, on_select: None, children: SmallVec::new() }
    }

    /// A tab is chosen (clicked, or the tab keys): its index.
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl ParentElement for TabView {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// Each tab's place along the top of `outer`: tabs overlap by their
/// slants, the first [`SLANT`] in.
fn places(window: &mut Window, outer: Bounds<Pixels>, tabs: &[SharedString]) -> Vec<Bounds<Pixels>> {
    let bold = style::ui_font(FontWeight::BOLD);
    let mut x = outer.left() + px(SLANT);
    tabs.iter()
        .map(|label| {
            let w = (f32::from(style::text_width(window, label.clone(), style::PLAIN_SIZE, bold.clone())) + 2.0 * SLANT + 18.0).round();
            let r = style::snap(window, Bounds { origin: point(x, outer.top()), size: size(px(w), px(TAB_HEIGHT)) });
            x += px(w - SLANT + 1.0);
            r
        })
        .collect()
}

impl RenderOnce for TabView {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let TabView { id, tabs, selected, on_select, children } = self;
        let n = tabs.len();
        let painted = tabs.clone();
        let select = on_select.clone();
        let face = canvas(
            move |b, window, _| places(window, b, &painted).into_iter().map(|r| (r, window.insert_hitbox(r, HitboxBehavior::Normal))).collect::<Vec<_>>(),
            move |outer, tabs_at, window, cx| {
                let p = theme::palette(cx);
                let bg = p.panel_background;
                let s = window.scale_factor();
                let dev = px(1.0 / s);
                let panel = Bounds::from_corners(point(outer.left(), outer.top() + px(TAB_HEIGHT - 1.0)), outer.bottom_right());
                let panel = style::snap(window, panel);
                // The panel: raised, a softer second line inside the dark one.
                style::bevel(window, panel, style::c(p.shine), style::shade(&p, bg, 1.35));
                let in_panel = style::inset(window, panel, 1.0);
                style::vline_left_of(window, in_panel.right(), in_panel.top(), in_panel.bottom(), style::shade(&p, bg, 1.12));
                style::hline_above(window, in_panel.left(), in_panel.right(), in_panel.bottom(), style::shade(&p, bg, 1.12));
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                // (The tabs go where their names' lengths put them: lined
                // up with what's under them only by chance.)
                crate::review::own_grid(window, Bounds::from_corners(outer.origin, point(outer.right(), outer.top() + px(TAB_HEIGHT))));
                let draw = |window: &mut Window, cx: &mut App, i: usize, r: Bounds<Pixels>| {
                    let on = i == selected;
                    let top = if on { r.top() } else { style::snap_coord(window, r.top() + px(2.0)) };
                    let bottom = if on { r.bottom() } else { style::snap_coord(window, r.bottom() - px(1.0)) };
                    let slant = style::snap_coord(window, px(SLANT));
                    let pts = [point(r.left(), bottom), point(r.left() + slant, top), point(r.right() - slant, top), point(r.right(), bottom)];
                    let fill = if on { style::c(bg) } else { style::shade(&p, bg, 1.07) };
                    let mut path = PathBuilder::fill();
                    path.move_to(pts[0]);
                    for pt in &pts[1..] {
                        path.line_to(*pt);
                    }
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, fill);
                    }
                    // Lit up the left side and along the top, shaded down
                    // the right: lines a device pixel wide, inside the face.
                    let half = dev / 2.0;
                    window.paint_segment(point(pts[0].x + half, pts[0].y), point(pts[1].x + half, pts[1].y + half), dev, false, style::c(p.shine));
                    style::hline(window, pts[1].x, pts[2].x, top, style::c(p.shine));
                    window.paint_segment(point(pts[2].x - half, pts[2].y + half), point(pts[3].x - half, pts[3].y), dev, false, style::shade(&p, bg, 1.4));
                    if on {
                        // Open into the panel.
                        style::hline(window, pts[0].x + dev, pts[3].x - dev, panel.top(), style::c(bg));
                    }
                    // Every name on one line, the chosen tab's too: centred
                    // in an unchosen tab's face.
                    let cy = (r.top() + px(2.0) + r.bottom() - px(1.0)) / 2.0;
                    let font = if on { bold.clone() } else { plain.clone() };
                    style::label(window, cx, r.center().x, cy, style::Align::Center, painted_label(&tabs, i), style::PLAIN_SIZE, font, style::c(p.panel_text));
                    agent::record(window, cx, agent::Node { role: "Tab", name: Some(painted_label(&tabs, i)), bounds: r, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
                };
                // The others first, right to left so left ones overlap;
                // the chosen one over them.
                for (i, (r, _)) in tabs_at.iter().enumerate().rev() {
                    if i != selected {
                        draw(window, cx, i, *r);
                    }
                }
                if let Some((r, _)) = tabs_at.get(selected) {
                    draw(window, cx, selected, *r);
                }
                let clicked = select.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left {
                        return;
                    }
                    // (The chosen tab is in front where they overlap.)
                    let hit = if tabs_at.get(selected).is_some_and(|(_, h)| h.is_hovered(window)) { Some(selected) } else { tabs_at.iter().position(|(_, h)| h.is_hovered(window)) };
                    if let (Some(i), Some(f)) = (hit, &clicked)
                        && i != selected
                    {
                        f(i, window, cx);
                    }
                });
                // The window's tab keys, unless a tab view drawn before
                // this one has them.
                if let Some(f) = select.clone() {
                    crate::app::set_tab_keys(window, cx, Rc::new(move |step: i32, window: &mut Window, cx: &mut App| {
                        if n > 1 {
                            f((selected as i32 + step).rem_euclid(n as i32) as usize, window, cx);
                        }
                    }));
                }
            },
        )
        .absolute()
        .inset_0();
        let _ = id;
        div()
            .relative()
            .size_full()
            .child(face)
            // Inside the panel, from under the tabs (its top line is under
            // them), so what's in it is on the two-point grid.
            .child(div().absolute().left(px(m::WINDOW_INSET)).top(px(TAB_HEIGHT + m::WINDOW_INSET)).right(px(m::WINDOW_INSET)).bottom(px(m::WINDOW_INSET)).flex().flex_col().children(children))
    }
}

fn painted_label(tabs: &[SharedString], i: usize) -> SharedString {
    tabs.get(i).cloned().unwrap_or_default()
}
