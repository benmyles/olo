//! BeOS scroll bars and the scroll view that wears them: a recessed
//! track, arrow buttons at both ends
//! (held, they scroll on), a bevelled knob with three grip lines, sized
//! by how much of the content shows; a click in the track pages towards
//! it. A [`ScrollBar`] drives any GPUI `ScrollHandle` (a `div` that
//! `.track_scroll`s it, a `uniform_list`'s), so the wheel and touchpad
//! (eased, Olo's GPUI) and the bar move the same content.

use std::{cell::RefCell, rc::Rc, time::Instant};

use gpui::{
    AnyElement, App, Bounds, CursorStyle, Element, ElementId, InteractiveElement as _, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Pixels, Point, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement as _, Style, Styled as _, Window, canvas, div, point, px, relative, size,
};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{agent, style, theme};

/// A scroll bar's breadth.
pub const SCROLL_BAR: f32 = 14.0;
/// How far an arrow scrolls at a click (then on, while it's held).
const LINE: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Back,
    Forward,
    Track,
    Knob,
}

/// What the pointer holds: which part, where along it was pressed, the
/// offset then, and when (an arrow scrolls on after a moment).
#[derive(Clone, Copy)]
struct Held {
    part: Part,
    at: f32,
    offset: f32,
    since: Instant,
    last: Instant,
}

#[derive(Default)]
struct BarState {
    held: Option<Held>,
}

/// One BeOS scroll bar for `handle`, filling the box it's given
/// ([`SCROLL_BAR`] across).
pub struct ScrollBar {
    id: ElementId,
    handle: ScrollHandle,
    vertical: bool,
    /// Placed in a scroll view's frame, given the whole view: inside its
    /// sunken lines (when bordered), short of the other bar's corner.
    frame: Option<(bool, bool)>,
}

impl ScrollBar {
    pub fn vertical(id: impl Into<ElementId>, handle: ScrollHandle) -> ScrollBar {
        ScrollBar { id: id.into(), handle, vertical: true, frame: None }
    }

    pub fn horizontal(id: impl Into<ElementId>, handle: ScrollHandle) -> ScrollBar {
        ScrollBar { id: id.into(), handle, vertical: false, frame: None }
    }

    /// Given a scroll view's bounds: places itself inside its frame's two
    /// lines (`border`) on device pixels, short of the corner when there's
    /// another bar (`other`).
    pub fn in_frame(mut self, border: bool, other: bool) -> Self {
        self.frame = Some((border, other));
        self
    }

    /// Where the bar is, in the bounds it was laid out in.
    fn place(&self, bounds: Bounds<Pixels>, window: &Window) -> Bounds<Pixels> {
        let Some((border, other)) = self.frame else { return bounds };
        let inner = style::inset(window, bounds, if border { 2.0 } else { 0.0 });
        let (bar, corner) = (px(SCROLL_BAR), if other { px(SCROLL_BAR) } else { px(0.0) });
        if self.vertical {
            Bounds::from_corners(point(inner.right() - bar, inner.top()), point(inner.right(), inner.bottom() - corner))
        } else {
            Bounds::from_corners(point(inner.left(), inner.bottom() - bar), point(inner.right() - corner, inner.bottom()))
        }
    }
}

impl IntoElement for ScrollBar {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// The bar's parts: the arrows at each end and the track between.
struct Geometry {
    back: Bounds<Pixels>,
    forward: Bounds<Pixels>,
    track: Bounds<Pixels>,
    knob: Option<Bounds<Pixels>>,
    /// How far the knob can travel, and the content's.
    travel: f32,
    max: f32,
    view: f32,
}

impl ScrollBar {
    /// Where it is (0 to `max`), how far it can go and how much shows.
    fn offsets(&self) -> (f32, f32, f32) {
        let (o, m, b) = (self.handle.offset(), self.handle.max_offset(), self.handle.bounds());
        if self.vertical {
            (-f32::from(o.y), f32::from(m.height), f32::from(b.size.height))
        } else {
            (-f32::from(o.x), f32::from(m.width), f32::from(b.size.width))
        }
    }

    fn set(&self, offset: f32, window: &mut Window) {
        let (_, max, _) = self.offsets();
        let offset = offset.clamp(0.0, max);
        let o = self.handle.offset();
        let to = if self.vertical { point(o.x, px(-offset)) } else { point(px(-offset), o.y) };
        if to != o {
            self.handle.set_offset(to);
            window.refresh();
        }
    }

    fn geometry(&self, bar: Bounds<Pixels>, window: &Window) -> Geometry {
        let (offset, max, view) = self.offsets();
        let a = px(SCROLL_BAR);
        let (back, forward, track) = if self.vertical {
            (
                Bounds { origin: bar.origin, size: size(bar.size.width, a) },
                Bounds { origin: point(bar.left(), bar.bottom() - a), size: size(bar.size.width, a) },
                Bounds::from_corners(point(bar.left(), bar.top() + a), point(bar.right(), bar.bottom() - a)),
            )
        } else {
            (
                Bounds { origin: bar.origin, size: size(a, bar.size.height) },
                Bounds { origin: point(bar.right() - a, bar.top()), size: size(a, bar.size.height) },
                Bounds::from_corners(point(bar.left() + a, bar.top()), point(bar.right() - a, bar.bottom())),
            )
        };
        let along = |b: Bounds<Pixels>| f32::from(if self.vertical { b.size.height } else { b.size.width });
        let track_len = along(track);
        let content = max + view;
        let mut knob = None;
        let mut travel = 0.0;
        if max > 0.5 && track_len > 8.0 && content > 0.0 {
            let knob_len = (track_len * view / content).clamp((SCROLL_BAR + 4.0).min(track_len), track_len);
            travel = track_len - knob_len;
            let pos = (offset / max).clamp(0.0, 1.0) * travel;
            let k = if self.vertical {
                Bounds { origin: point(track.left(), track.top() + px(pos)), size: size(track.size.width, px(knob_len)) }
            } else {
                Bounds { origin: point(track.left() + px(pos), track.top()), size: size(px(knob_len), track.size.height) }
            };
            knob = Some(style::snap(window, k));
        }
        Geometry { back, forward, track, knob, travel, max, view }
    }

    fn part_at(&self, g: &Geometry, p: Point<Pixels>) -> Option<Part> {
        if g.back.contains(&p) {
            Some(Part::Back)
        } else if g.forward.contains(&p) {
            Some(Part::Forward)
        } else if g.knob.is_some_and(|k| k.contains(&p)) {
            Some(Part::Knob)
        } else if g.track.contains(&p) {
            Some(Part::Track)
        } else {
            None
        }
    }
}

impl Element for ScrollBar {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, _: &mut App) -> Hitbox {
        let bar = self.place(bounds, window);
        window.insert_hitbox(bar, HitboxBehavior::Normal)
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), hitbox: &mut Hitbox, window: &mut Window, cx: &mut App) {
        let bar = hitbox.bounds;
        let p = theme::palette(cx);
        let state = window.with_optional_element_state::<Rc<RefCell<BarState>>, _>(id, |s, _| {
            let s = s.flatten().unwrap_or_default();
            (s.clone(), Some(s))
        });
        let g = self.geometry(bar, window);
        let enabled = g.max > 0.5;
        // An arrow held: on it scrolls, after a moment, at a steady
        // pace.
        let held = state.borrow().held;
        if let Some(h) = held
            && matches!(h.part, Part::Back | Part::Forward)
            && enabled
        {
            let now = Instant::now();
            if now.duration_since(h.since).as_secs_f32() > 0.3 {
                let dt = now.duration_since(h.last).as_secs_f32().min(0.05);
                let dir = if h.part == Part::Back { -1.0 } else { 1.0 };
                let (offset, _, _) = self.offsets();
                self.set(offset + dir * LINE * dt * 20.0, window);
            }
            if let Some(hs) = state.borrow_mut().held.as_mut() {
                hs.last = now;
            }
            window.request_animation_frame();
        }

        // The track: recessed, a shade darker than the panel.
        let bg = p.panel_background;
        style::fill_rect(window, bar, style::shade(&p, bg, 1.1));
        if self.vertical {
            style::vline(window, bar.left(), bar.top(), bar.bottom(), style::shade(&p, bg, 1.35));
        } else {
            style::hline(window, bar.left(), bar.right(), bar.top(), style::shade(&p, bg, 1.35));
        }
        let down = |part| held.is_some_and(|h| h.part == part) && enabled;
        let (d0, d1) = if self.vertical { (Arrow::Up, Arrow::Down) } else { (Arrow::Left, Arrow::Right) };
        arrow_button(window, g.back, d0, down(Part::Back), enabled, &p);
        arrow_button(window, g.forward, d1, down(Part::Forward), enabled, &p);
        if let Some(k) = g.knob {
            knob(window, k, self.vertical, down(Part::Knob), &p);
        }
        let (offset, _, _) = self.offsets();
        let at = if g.max > 0.0 { (offset / g.max).clamp(0.0, 1.0) } else { 0.0 };
        agent::record(window, cx, agent::Node { role: "ScrollBar", name: Some("Scroll Bar".into()), value: Some(SharedString::from(format!("{at:.2}"))), bounds: g.track, enabled, ..Default::default() });
        for (b, name) in [(g.back, d0.name()), (g.forward, d1.name())] {
            agent::record(window, cx, agent::Node { role: "Button", name: Some(name.into()), bounds: b, enabled, clickable: enabled, ..Default::default() });
        }
        if !enabled {
            return;
        }
        window.set_cursor_style(CursorStyle::Arrow, hitbox);

        let this = (self.handle.clone(), self.vertical);
        let bar_of = move || ScrollBar { id: ElementId::Name("bar".into()), handle: this.0.clone(), vertical: this.1, frame: None };
        let along = move |vertical: bool, p: Point<Pixels>| f32::from(if vertical { p.y } else { p.x });
        let hb = hitbox.clone();
        let (st, bar_down) = (state.clone(), bar_of.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, _| {
            if !phase.bubble() || e.button != MouseButton::Left || !hb.is_hovered(window) {
                return;
            }
            let b = bar_down();
            let g = b.geometry(hb.bounds, window);
            let Some(part) = b.part_at(&g, e.position) else { return };
            let (offset, _, view) = b.offsets();
            let now = Instant::now();
            match part {
                Part::Back => b.set(offset - LINE, window),
                Part::Forward => b.set(offset + LINE, window),
                Part::Track => {
                    // A page towards the click.
                    let before = g.knob.is_some_and(|k| if b.vertical { e.position.y < k.top() } else { e.position.x < k.left() });
                    b.set(offset + if before { -view } else { view } * 0.9, window);
                }
                Part::Knob => {}
            }
            st.borrow_mut().held = Some(Held { part, at: along(b.vertical, e.position), offset, since: now, last: now });
            window.refresh();
        });
        let (st, bar_move) = (state.clone(), bar_of.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _| {
            if !phase.capture() {
                return;
            }
            let Some(h) = st.borrow().held else { return };
            if h.part != Part::Knob || !e.dragging() {
                return;
            }
            let b = bar_move();
            let g = b.geometry(bar, window);
            if g.travel > 0.0 {
                b.set(h.offset + (along(b.vertical, e.position) - h.at) * g.max / g.travel, window);
            }
        });
        let st = state.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, _| {
            if phase.capture() && e.button == MouseButton::Left && st.borrow_mut().held.take().is_some() {
                window.refresh();
            }
        });
        let _ = g.view;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Arrow {
    Up,
    Down,
    Left,
    Right,
}

impl Arrow {
    fn name(self) -> &'static str {
        match self {
            Arrow::Up => "Scroll Up",
            Arrow::Down => "Scroll Down",
            Arrow::Left => "Scroll Left",
            Arrow::Right => "Scroll Right",
        }
    }
}

fn arrow_button(window: &mut Window, r: Bounds<Pixels>, dir: Arrow, down: bool, enabled: bool, p: &olo_theme::Palette) {
    let base = p.control_background;
    let bg = p.panel_background;
    let fill = if down { style::shade(p, base, 1.15) } else { style::tint(base, 0.9) };
    let inner = style::inset(window, r, 1.0);
    style::fill_rect(window, inner, fill);
    style::frame_rect(window, r, style::shade(p, bg, 1.55));
    if down {
        style::bevel(window, inner, style::shade(p, base, 1.3), style::shade(p, base, 1.05));
    } else {
        style::bevel(window, inner, style::c(p.shine), style::shade(p, base, 1.18));
    }
    let color = if enabled { style::c(p.control_text) } else { style::shade(p, bg, 1.3) };
    let c = r.center() + if down { point(px(0.5), px(0.5)) } else { point(px(0.0), px(0.0)) };
    let (s, h) = (px(3.5), px(2.0));
    let pts = match dir {
        Arrow::Up => [point(c.x - s, c.y + h), point(c.x + s, c.y + h), point(c.x, c.y - h)],
        Arrow::Down => [point(c.x - s, c.y - h), point(c.x + s, c.y - h), point(c.x, c.y + h)],
        Arrow::Left => [point(c.x + h, c.y - s), point(c.x + h, c.y + s), point(c.x - h, c.y)],
        Arrow::Right => [point(c.x - h, c.y - s), point(c.x - h, c.y + s), point(c.x + h, c.y)],
    };
    let mut path = PathBuilder::fill();
    path.move_to(pts[0]);
    path.line_to(pts[1]);
    path.line_to(pts[2]);
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn knob(window: &mut Window, k: Bounds<Pixels>, vertical: bool, pressed: bool, p: &olo_theme::Palette) {
    let base = p.control_background;
    let bg = p.panel_background;
    let (t0, t1) = if pressed { (style::shade(p, base, 1.12), style::shade(p, base, 1.05)) } else { (style::tint(base, 0.85), style::shade(p, base, 1.04)) };
    let inner = style::inset(window, k, 1.0);
    if vertical {
        // Shaded across the knob, as light falls on a vertical bar.
        style::hgradient(window, inner, t0, t1);
    } else {
        style::vgradient(window, inner, t0, t1);
    }
    style::frame_rect(window, k, style::shade(p, bg, 1.55));
    style::bevel(window, inner, style::c(p.shine), style::shade(p, base, 1.18));
    // Three grip lines in the middle.
    let len = f32::from(if vertical { k.size.height } else { k.size.width });
    if len > 20.0 {
        let c = k.center();
        let (cx, cy) = (style::snap_coord(window, c.x), style::snap_coord(window, c.y));
        for i in -1..=1 {
            let o = px(i as f32 * 3.0);
            if vertical {
                style::hline_above(window, k.left() + px(4.0), k.right() - px(4.0), cy + o, style::shade(p, base, 1.3));
                style::hline(window, k.left() + px(4.0), k.right() - px(4.0), cy + o, style::c(p.shine));
            } else {
                style::vline_left_of(window, cx + o, k.top() + px(4.0), k.bottom() - px(4.0), style::shade(p, base, 1.3));
                style::vline(window, cx + o, k.top() + px(4.0), k.bottom() - px(4.0), style::c(p.shine));
            }
        }
    }
}

/// BScrollView: what it holds scrolled in a sunken frame, with a BeOS
/// scroll bar down the right (and along the bottom, `horizontal`). Keep
/// its `ScrollHandle` in the view's state so it scrolls on from frame to
/// frame.
#[derive(IntoElement)]
pub struct ScrollView {
    id: ElementId,
    handle: ScrollHandle,
    vertical: bool,
    horizontal: bool,
    /// The vertical bar only while the content overflows.
    auto_hide: bool,
    border: bool,
    focused: bool,
    background: Option<olo_theme::Rgba>,
    /// The desk sheets of paper lie on (a document's Page View).
    desk: bool,
    /// The content scrolls itself (the editor, laying out what shows) and
    /// tells the handle how far it goes (`ScrollHandle::set_extent`).
    own: bool,
    children: SmallVec<[AnyElement; 4]>,
}

impl ScrollView {
    pub fn new(id: impl Into<ElementId>, handle: &ScrollHandle) -> ScrollView {
        ScrollView { id: id.into(), handle: handle.clone(), vertical: true, horizontal: false, auto_hide: false, border: true, focused: false, background: None, desk: false, own: false, children: SmallVec::new() }
    }

    /// Shows the vertical bar only while the content overflows (as the
    /// content was last laid out): columns of lists side by side, where
    /// bars on every one would be heavy.
    pub fn auto_hide(mut self, on: bool) -> Self {
        self.auto_hide = on;
        self
    }

    pub fn horizontal(mut self, on: bool) -> Self {
        self.horizontal = on;
        self
    }

    pub fn vertical(mut self, on: bool) -> Self {
        self.vertical = on;
        self
    }

    pub fn border(mut self, on: bool) -> Self {
        self.border = on;
        self
    }

    /// The keyboard's frame round it (what's in it has the keyboard).
    pub fn focused(mut self, on: bool) -> Self {
        self.focused = on;
        self
    }

    pub fn background(mut self, rgba: olo_theme::Rgba) -> Self {
        self.background = Some(rgba);
        self
    }

    /// The desk under a document's sheets of paper
    /// ([`crate::editor::desk`]), in the theme's colours.
    pub fn desk(mut self) -> Self {
        self.desk = true;
        self
    }

    /// Its content scrolls itself, laying out only what shows, and tells
    /// the handle where it is and how far it goes
    /// (`ScrollHandle::set_extent`); the bars drive the handle as ever.
    pub fn scrolls_itself(mut self, on: bool) -> Self {
        self.own = on;
        self
    }
}

impl ParentElement for ScrollView {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollView {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        // Inside the sunken frame's two lines, on device pixels (two
        // points would be three device pixels at 1.5×, a gap of one).
        let s = window.scale_factor();
        let inset = if self.border { style::line_pixels(s) * 2.0 / s } else { 0.0 };
        let (vbar, hbar) = (self.vertical && (!self.auto_hide || self.handle.max_offset().height > px(0.5)), self.horizontal);
        let right = inset + if vbar { SCROLL_BAR } else { 0.0 };
        let bottom = inset + if hbar { SCROLL_BAR } else { 0.0 };
        let (border, focused, background, desk) = (self.border, self.focused, self.background, self.desk);
        let mut content = div().id(self.id.clone()).absolute().left(px(inset)).top(px(inset)).right(px(right)).bottom(px(bottom));
        if !self.own {
            content = content.track_scroll(&self.handle);
            content = match (vbar, hbar) {
                (true, true) => content.overflow_scroll(),
                (true, false) => content.overflow_y_scroll(),
                (false, true) => content.overflow_x_scroll(),
                (false, false) => content,
            };
        }
        let noted = self.handle.clone();
        let backdrop = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                // Where it shows its content from, for the frame's mark
                // (measuring how evenly it scrolls).
                let (o, m) = (noted.offset(), noted.max_offset());
                window.note_scroll([-f32::from(o.x), -f32::from(o.y), f32::from(m.width), f32::from(m.height)]);
                let p = theme::palette(cx);
                let inner = style::inset(window, b, if border { 2.0 } else { 0.0 });
                let (r, bt) = (if vbar { px(SCROLL_BAR) } else { px(0.0) }, if hbar { px(SCROLL_BAR) } else { px(0.0) });
                let background = if desk { Some(crate::editor::desk(&p)) } else { background };
                if let Some(bg) = background {
                    style::fill_rect(window, Bounds::from_corners(inner.origin, point(inner.right() - r, inner.bottom() - bt)), style::c(bg));
                }
                if vbar && hbar {
                    style::fill_rect(window, Bounds::from_corners(point(inner.right() - r, inner.bottom() - bt), inner.bottom_right()), style::c(p.panel_background));
                }
            },
        )
        .absolute()
        .inset_0();
        let frame = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                if border {
                    let p = theme::palette(cx);
                    style::sunken(window, b, &p, focused);
                }
            },
        )
        .absolute()
        .inset_0();
        let mut root = div().relative().size_full().child(backdrop).child(content.children(self.children));
        // The bars place themselves in the frame, on device pixels.
        if vbar {
            root = root.child(div().absolute().inset_0().child(ScrollBar::vertical("vbar", self.handle.clone()).in_frame(border, hbar)));
        }
        if hbar {
            root = root.child(div().absolute().inset_0().child(ScrollBar::horizontal("hbar", self.handle.clone()).in_frame(border, vbar)));
        }
        let _ = m::SPACING;
        root.child(frame)
    }
}
