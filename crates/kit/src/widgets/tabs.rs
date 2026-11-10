//! BTabView: BeOS folder tabs, slanted at their sides, over a raised
//! panel holding what the chosen tab shows; the chosen tab is in front,
//! open into the panel, its name in bold. A click on a tab chooses it,
//! and so do Next and Previous Tab (⌃Tab and ⌃⇧Tab, ⌃Page Down and
//! ⌃Page Up) wherever the keyboard is in the window, unless what has it
//! takes them (a screen being controlled): the first tab view in the
//! window has them. Agents find each tab as a `Tab`, selected or not.
//!
//! ```ignore
//! TabView::new("tabs", vec!["Live".into(), "History".into()], chosen).on_select(cx.msg_with(|i: &usize| Msg::Tab(*i))).child(content)
//! ```

use std::rc::Rc;

use olo_interface::{
    Bounds, Element, FontWeight, InteractiveElement as _, IntoElement, MouseButton, Painter, ParentElement, PathBuilder, Pixels, SharedString, Styled as _, WindowCx, agent, canvas, div, point, px,
    size,
};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{style, theme};

/// The tabs' row.
pub const TAB_HEIGHT: f32 = 22.0;
/// How far a tab's side slants across.
const SLANT: f32 = 6.0;

type SelectFn = Rc<dyn Fn(&usize, &mut WindowCx)>;

pub struct TabView {
    id: SharedString,
    tabs: Rc<[SharedString]>,
    selected: usize,
    on_select: Option<SelectFn>,
    children: SmallVec<[Element; 2]>,
    grows: bool,
}

impl TabView {
    pub fn new(id: impl Into<SharedString>, tabs: impl Into<Rc<[SharedString]>>, selected: usize) -> TabView {
        TabView { id: id.into(), tabs: tabs.into(), selected, on_select: None, children: SmallVec::new(), grows: false }
    }

    /// As tall as what the chosen tab shows needs, at least (else it is as
    /// tall as it's given, and what it shows fits in that): for a window
    /// that grows to what it shows (Preferences' panels). Lists in it
    /// still shrink to the room there is.
    pub fn grows(mut self) -> Self {
        self.grows = true;
        self
    }

    /// A tab is chosen (clicked, or the tab keys): its index.
    pub fn on_select(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl ParentElement for TabView {
    fn extend(&mut self, elements: impl IntoIterator<Item = Element>) {
        self.children.extend(elements);
    }
}

/// Each tab's place along the top of `outer`: tabs overlap by their
/// slants, the first [`SLANT`] in.
fn places(scale: f32, outer: Bounds<Pixels>, tabs: &[SharedString]) -> Vec<Bounds<Pixels>> {
    let bold = style::ui_font(FontWeight::BOLD);
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let mut x = outer.left() + px(SLANT);
    tabs.iter()
        .map(|label| {
            let w = (f32::from(style::text_width(label.clone(), style::PLAIN_SIZE, bold.clone())) + 2.0 * SLANT + 18.0).round();
            let r = Bounds::from_corners(point(snap(x), snap(outer.top())), point(snap(x + px(w)), snap(outer.top() + px(TAB_HEIGHT))));
            x += px(w - SLANT + 1.0);
            r
        })
        .collect()
}

impl IntoElement for TabView {
    fn into_element(self) -> Element {
        let TabView { id, tabs, selected, on_select, children, grows } = self;
        let n = tabs.len();
        let (painted, pressed) = (tabs.clone(), tabs.clone());
        let (clicked, keys) = (on_select.clone(), on_select);
        let face = canvas(move |g, outer| paint_face(g, outer, &painted, selected))
            .paint_key(style::key((&*tabs, selected)))
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |e, cx| {
                let at = places(cx.scale_factor(), cx.listener_bounds(), &pressed);
                // (The chosen tab is in front where they overlap.)
                let hit = if at.get(selected).is_some_and(|r| r.contains(&e.position)) { Some(selected) } else { at.iter().position(|r| r.contains(&e.position)) };
                if let (Some(i), Some(f)) = (hit, &clicked)
                    && i != selected
                {
                    f(&i, cx);
                }
            })
            // The window's tab keys, unless a tab view before this one in
            // the window has them.
            .on_window_key_down(move |e, cx| {
                let step = crate::tab_step(&e.keystroke);
                if step != 0
                    && n > 1
                    && let Some(f) = &keys
                {
                    f(&((selected as i32 + step).rem_euclid(n as i32) as usize), cx);
                    cx.stop_propagation();
                }
            });
        let _ = id;
        if grows {
            // (In the flow, so it is as tall as what's in it.)
            return div()
                .relative()
                .w_full()
                .min_h_full()
                .flex()
                .flex_col()
                .child(face)
                .child(div().flex_1().flex().flex_col().mt(px(TAB_HEIGHT + m::WINDOW_INSET)).mx(px(m::WINDOW_INSET)).mb(px(m::WINDOW_INSET)).children(children));
        }
        div()
            .relative()
            .size_full()
            .child(face)
            // Inside the panel, from under the tabs (its top line is under
            // them), so what's in it is on the two-point grid.
            .child(div().absolute().left(px(m::WINDOW_INSET)).top(px(TAB_HEIGHT + m::WINDOW_INSET)).right(px(m::WINDOW_INSET)).bottom(px(m::WINDOW_INSET)).flex().flex_col().children(children))
    }
}

/// The tabs and the panel under them.
fn paint_face(g: &mut Painter, outer: Bounds<Pixels>, tabs: &[SharedString], selected: usize) {
    let p = theme::palette();
    let bg = p.panel_background;
    let s = g.scale_factor();
    let dev = px(1.0 / s);
    let panel = Bounds::from_corners(point(outer.left(), outer.top() + px(TAB_HEIGHT - 1.0)), outer.bottom_right());
    let panel = style::snap(g, panel);
    // The panel: raised, a softer second line inside the dark one.
    style::bevel(g, panel, style::c(p.shine), style::shade(&p, bg, 1.35));
    // (What's in it must fit inside its inset.)
    crate::review::pane(
        g,
        Bounds::from_corners(point(outer.left() + px(m::WINDOW_INSET), outer.top() + px(TAB_HEIGHT)), point(outer.right() - px(m::WINDOW_INSET), outer.bottom() - px(m::WINDOW_INSET))),
    );
    let in_panel = style::inset(g, panel, 1.0);
    style::vline_left_of(g, in_panel.right(), in_panel.top(), in_panel.bottom(), style::shade(&p, bg, 1.12));
    style::hline_above(g, in_panel.left(), in_panel.right(), in_panel.bottom(), style::shade(&p, bg, 1.12));
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    // (The tabs go where their names' lengths put them: lined up with
    // what's under them only by chance.)
    crate::review::own_grid(g, Bounds::from_corners(outer.origin, point(outer.right(), outer.top() + px(TAB_HEIGHT))));
    let rects = places(s, outer, tabs);
    let mut draw = |g: &mut Painter, i: usize, r: Bounds<Pixels>| {
        let on = i == selected;
        let top = if on { r.top() } else { style::snap_coord(g, r.top() + px(2.0)) };
        let bottom = if on { r.bottom() } else { style::snap_coord(g, r.bottom() - px(1.0)) };
        let slant = style::snap_coord(g, px(SLANT));
        let pts = [point(r.left(), bottom), point(r.left() + slant, top), point(r.right() - slant, top), point(r.right(), bottom)];
        let fill = if on { style::c(bg) } else { style::shade(&p, bg, 1.07) };
        let mut path = PathBuilder::fill();
        path.move_to(pts[0]);
        for pt in &pts[1..] {
            path.line_to(*pt);
        }
        path.close();
        if let Ok(path) = path.build() {
            g.paint_path(path, fill);
        }
        // Lit up the left side and along the top, shaded down the right:
        // lines a device pixel wide, inside the face.
        let half = dev / 2.0;
        g.paint_segment(point(pts[0].x + half, pts[0].y), point(pts[1].x + half, pts[1].y + half), dev, false, style::c(p.shine));
        style::hline(g, pts[1].x, pts[2].x, top, style::c(p.shine));
        g.paint_segment(point(pts[2].x - half, pts[2].y + half), point(pts[3].x - half, pts[3].y), dev, false, style::shade(&p, bg, 1.4));
        if on {
            // Open into the panel.
            style::hline(g, pts[0].x + dev, pts[3].x - dev, panel.top(), style::c(bg));
        }
        // Every name on one line, the chosen tab's too: centred in an
        // unchosen tab's face.
        let cy = (r.top() + px(2.0) + r.bottom() - px(1.0)) / 2.0;
        let font = if on { bold.clone() } else { plain.clone() };
        let label = tabs.get(i).cloned().unwrap_or_default();
        style::label(g, r.center().x, cy, style::Align::Center, label.clone(), style::PLAIN_SIZE, font, style::c(p.panel_text));
        // Agents get where a click picks it: the chosen tab over its
        // neighbours, of two others the left one (the press), so no two
        // overlap.
        let mut take = r;
        if !on && i > 0 {
            let left = rects[i - 1].right().min(r.right());
            take = Bounds::from_corners(point(left.max(r.left()), r.top()), r.bottom_right());
        }
        if !on && i + 1 == selected {
            let right = rects[selected].left().max(take.left());
            take = Bounds::from_corners(take.origin, point(right.min(take.right()), r.bottom()));
        }
        g.agent(agent::Node { role: "Tab", name: Some(label), bounds: take, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
    };
    // The others first, right to left so left ones overlap; the chosen
    // one over them.
    for (i, r) in rects.iter().enumerate().rev() {
        if i != selected {
            draw(g, i, *r);
        }
    }
    if let Some(r) = rects.get(selected) {
        draw(g, selected, *r);
    }
    let _ = size(px(0.), px(0.));
}
