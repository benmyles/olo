//! BeOS scroll bars and the scroll view that wears them: a recessed
//! track, arrow buttons at both ends (held, they scroll on), a bevelled
//! knob with three grip lines, sized by how much of the content shows; a
//! click in the track pages towards it. A [`ScrollBar`] drives any
//! [`ScrollHandle`] (an element that scrolls and `track_scroll`s it), so
//! the wheel and the touchpad, which the window's stage scrolls with at
//! once, and the bar move the same content: the bar is drawn again,
//! alone, when its scroller moves.

use olo_interface::{
    Bounds, CursorStyle, Element, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Painter, ParentElement, PathBuilder, Pixels, Point, ScrollHandle, SharedString,
    Styled as _, WindowCx, agent, canvas, div, point, px, size,
};
use smallvec::SmallVec;

use crate::{style, theme};

/// A scroll bar's breadth.
pub const SCROLL_BAR: f32 = 14.0;
/// How far an arrow scrolls at a click (then on, while it's held).
const LINE: f32 = 16.0;
/// How long an arrow is held before it scrolls on, and how fast it does
/// (points a second).
const HOLD: std::time::Duration = std::time::Duration::from_millis(300);
const HELD_SPEED: f32 = LINE * 20.0;

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
    since: std::time::Instant,
    last: std::time::Instant,
}

/// What a bar keeps for itself (its element's state).
#[derive(Default)]
struct BarState {
    held: Option<Held>,
}

/// One BeOS scroll bar for `handle`, filling the box it's given
/// ([`SCROLL_BAR`] across).
pub struct ScrollBar {
    handle: ScrollHandle,
    vertical: bool,
}

impl ScrollBar {
    pub fn vertical(handle: &ScrollHandle) -> ScrollBar {
        ScrollBar { handle: handle.clone(), vertical: true }
    }

    pub fn horizontal(handle: &ScrollHandle) -> ScrollBar {
        ScrollBar { handle: handle.clone(), vertical: false }
    }
}

/// A bar for `handle`, as its handlers and its drawing know it.
#[derive(Clone)]
struct Bar {
    handle: ScrollHandle,
    vertical: bool,
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
}

impl Bar {
    /// Where it is (0 to `max`), how far it can go and how much shows.
    fn offsets(&self) -> (f32, f32, f32) {
        let (o, m, b) = (self.handle.offset(), self.handle.max_offset(), self.handle.bounds());
        if self.vertical { (-f32::from(o.y), f32::from(m.height), f32::from(b.size.height)) } else { (-f32::from(o.x), f32::from(m.width), f32::from(b.size.width)) }
    }

    fn set(&self, offset: f32, cx: &mut WindowCx) {
        let (_, max, _) = self.offsets();
        let offset = offset.clamp(0.0, max);
        let o = self.handle.offset();
        let to = if self.vertical { point(o.x, px(-offset)) } else { point(px(-offset), o.y) };
        self.handle.set_offset(cx, to);
    }

    fn along(&self, p: Point<Pixels>) -> f32 {
        f32::from(if self.vertical { p.y } else { p.x })
    }

    fn geometry(&self, bar: Bounds<Pixels>, scale: f32) -> Geometry {
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
            // (On device pixels.)
            let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
            knob = Some(Bounds::from_corners(point(snap(k.left()), snap(k.top())), point(snap(k.right()), snap(k.bottom()))));
        }
        Geometry { back, forward, track, knob, travel, max }
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

    fn paint(&self, g: &mut Painter, bar: Bounds<Pixels>) {
        // (Part of its scroll view, for the layout review: it sits in the
        // view's frame as the kit draws it.)
        crate::review::scroll_area(g, bar);
        let p = theme::palette();
        let geo = self.geometry(bar, g.scale_factor());
        let enabled = geo.max > 0.5;
        let held = g.node_state::<BarState>().held;
        // The track: recessed, a shade darker than the panel.
        let bg = p.panel_background;
        style::fill_rect(g, bar, style::shade(&p, bg, 1.1));
        if self.vertical {
            style::vline(g, bar.left(), bar.top(), bar.bottom(), style::shade(&p, bg, 1.35));
        } else {
            style::hline(g, bar.left(), bar.right(), bar.top(), style::shade(&p, bg, 1.35));
        }
        let down = |part| held.is_some_and(|h| h.part == part) && enabled;
        let (d0, d1) = if self.vertical { (Arrow::Up, Arrow::Down) } else { (Arrow::Left, Arrow::Right) };
        arrow_button(g, geo.back, d0, down(Part::Back), enabled, &p);
        arrow_button(g, geo.forward, d1, down(Part::Forward), enabled, &p);
        if let Some(k) = geo.knob {
            knob(g, k, self.vertical, down(Part::Knob), &p);
        }
        let (offset, _, _) = self.offsets();
        let at = if geo.max > 0.0 { (offset / geo.max).clamp(0.0, 1.0).abs() } else { 0.0 };
        g.agent(agent::Node { role: "ScrollBar", name: Some("Scroll Bar".into()), value: Some(SharedString::from(format!("{at:.2}"))), bounds: geo.track, enabled, ..Default::default() });
        for (b, name) in [(geo.back, d0.name()), (geo.forward, d1.name())] {
            g.agent(agent::Node { role: "Button", name: Some(name.into()), bounds: b, enabled, clickable: enabled, ..Default::default() });
        }
    }

    fn mouse_down(&self, e: &MouseDownEvent, cx: &mut WindowCx) {
        let geo = self.geometry(cx.listener_bounds(), cx.scale_factor());
        if geo.max <= 0.5 {
            return;
        }
        let Some(part) = self.part_at(&geo, e.position) else { return };
        let (offset, _, view) = self.offsets();
        match part {
            Part::Back => self.set(offset - LINE, cx),
            Part::Forward => self.set(offset + LINE, cx),
            Part::Track => {
                // A page towards the click.
                let before = geo.knob.is_some_and(|k| if self.vertical { e.position.y < k.top() } else { e.position.x < k.left() });
                self.set(offset + if before { -view } else { view } * 0.9, cx);
            }
            Part::Knob => {}
        }
        let now = std::time::Instant::now();
        cx.node_state::<BarState>().held = Some(Held { part, at: self.along(e.position), offset, since: now, last: now });
        cx.repaint();
        cx.stop_propagation();
        if matches!(part, Part::Back | Part::Forward) {
            self.scroll_on(cx);
        }
    }

    /// An arrow held: on it scrolls, after a moment, at a steady pace,
    /// a step a frame while it's held.
    fn scroll_on(&self, cx: &mut WindowCx) {
        let Some(node) = cx.listener_node() else { return };
        let bar = self.clone();
        cx.on_next_frame(move |cx| {
            cx.with_node(node, |cx| {
                let now = std::time::Instant::now();
                let Some(held) = cx.node_state::<BarState>().held.as_mut() else { return };
                let (part, since) = (held.part, held.since);
                let dt = now.duration_since(held.last).as_secs_f32().min(0.05);
                held.last = now;
                if now.duration_since(since) > HOLD {
                    let dir = if part == Part::Back { -1.0 } else { 1.0 };
                    let (offset, _, _) = bar.offsets();
                    bar.set(offset + dir * HELD_SPEED * dt, cx);
                }
                bar.scroll_on(cx);
            });
        });
    }

    fn mouse_move(&self, e: &MouseMoveEvent, cx: &mut WindowCx) {
        let Some(held) = cx.node_state::<BarState>().held else { return };
        if held.part != Part::Knob || !e.dragging() {
            return;
        }
        let geo = self.geometry(cx.listener_bounds(), cx.scale_factor());
        if geo.travel > 0.0 {
            self.set(held.offset + (self.along(e.position) - held.at) * geo.max / geo.travel, cx);
        }
    }

    fn mouse_up(&self, cx: &mut WindowCx) {
        if cx.node_state::<BarState>().held.take().is_some() {
            cx.repaint();
        }
    }
}

impl IntoElement for ScrollBar {
    fn into_element(self) -> Element {
        let bar = Bar { handle: self.handle.clone(), vertical: self.vertical };
        let (painted, pressed, moved, released) = (bar.clone(), bar.clone(), bar.clone(), bar);
        // (Drawn again when its scroller moves or a part of it is held:
        // not for the view that holds it saying what it shows again.)
        canvas(move |g, bounds| painted.paint(g, bounds))
            .paint_key(self.vertical as u64)
            .size_full()
            .cursor(CursorStyle::Arrow)
            .watch_scroll(&self.handle)
            .on_mouse_down(MouseButton::Left, move |e, cx| pressed.mouse_down(e, cx))
            .on_window_mouse_move(move |e, cx| moved.mouse_move(e, cx))
            .on_window_mouse_up(move |e, cx| {
                if e.button == MouseButton::Left {
                    released.mouse_up(cx)
                }
            })
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

fn arrow_button(g: &mut Painter, r: Bounds<Pixels>, dir: Arrow, down: bool, enabled: bool, p: &olo_theme::Palette) {
    let base = p.control_background;
    let bg = p.panel_background;
    let fill = if down { style::shade(p, base, 1.15) } else { style::tint(base, 0.9) };
    let inner = style::inset(g, r, 1.0);
    style::fill_rect(g, inner, fill);
    style::frame_rect(g, r, style::shade(p, bg, 1.55));
    if down {
        style::bevel(g, inner, style::shade(p, base, 1.3), style::shade(p, base, 1.05));
    } else {
        style::bevel(g, inner, style::c(p.shine), style::shade(p, base, 1.18));
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
        g.paint_path(path, color);
    }
}

fn knob(g: &mut Painter, k: Bounds<Pixels>, vertical: bool, pressed: bool, p: &olo_theme::Palette) {
    let base = p.control_background;
    let bg = p.panel_background;
    let (t0, t1) = if pressed { (style::shade(p, base, 1.12), style::shade(p, base, 1.05)) } else { (style::tint(base, 0.85), style::shade(p, base, 1.04)) };
    let inner = style::inset(g, k, 1.0);
    if vertical {
        // Shaded across the knob, as light falls on a vertical bar.
        style::hgradient(g, inner, t0, t1);
    } else {
        style::vgradient(g, inner, t0, t1);
    }
    style::frame_rect(g, k, style::shade(p, bg, 1.55));
    style::bevel(g, inner, style::c(p.shine), style::shade(p, base, 1.18));
    // Three grip lines in the middle.
    let len = f32::from(if vertical { k.size.height } else { k.size.width });
    if len > 20.0 {
        let c = k.center();
        let (cx, cy) = (style::snap_coord(g, c.x), style::snap_coord(g, c.y));
        for i in -1..=1 {
            let o = px(i as f32 * 3.0);
            if vertical {
                style::hline_above(g, k.left() + px(4.0), k.right() - px(4.0), cy + o, style::shade(p, base, 1.3));
                style::hline(g, k.left() + px(4.0), k.right() - px(4.0), cy + o, style::c(p.shine));
            } else {
                style::vline_left_of(g, cx + o, k.top() + px(4.0), k.bottom() - px(4.0), style::shade(p, base, 1.3));
                style::vline(g, cx + o, k.top() + px(4.0), k.bottom() - px(4.0), style::c(p.shine));
            }
        }
    }
}

/// BScrollView: what it holds scrolled in a sunken frame, with a BeOS
/// scroll bar down the right (and along the bottom, `horizontal`). Keep
/// its [`ScrollHandle`] in the view's state: it's how the bars and the
/// view know the same scroller.
pub struct ScrollView {
    handle: ScrollHandle,
    vertical: bool,
    horizontal: bool,
    /// The vertical bar only while the content overflows.
    auto_hide: bool,
    border: bool,
    focused: bool,
    background: Option<olo_theme::Rgba>,
    children: SmallVec<[Element; 4]>,
}

impl ScrollView {
    pub fn new(handle: &ScrollHandle) -> ScrollView {
        ScrollView { handle: handle.clone(), vertical: true, horizontal: false, auto_hide: false, border: true, focused: false, background: None, children: SmallVec::new() }
    }

    /// Shows the vertical bar only while the content overflows: columns
    /// of lists side by side, where bars on every one would be heavy.
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
}

impl ParentElement for ScrollView {
    fn extend(&mut self, elements: impl IntoIterator<Item = Element>) {
        self.children.extend(elements);
    }
}

impl IntoElement for ScrollView {
    fn into_element(self) -> Element {
        // Inside the sunken frame's two lines, on device pixels (two
        // points would be three device pixels at 1.5×, a gap of one).
        let s = olo_interface::scale_factor();
        let inset = if self.border { style::line_pixels(s) * 2.0 / s } else { 0.0 };
        let overflows = self.handle.max_offset().height > px(0.5);
        let (vbar, hbar) = (self.vertical && (!self.auto_hide || overflows), self.horizontal);
        let right = inset + if vbar { SCROLL_BAR } else { 0.0 };
        let bottom = inset + if hbar { SCROLL_BAR } else { 0.0 };
        let (border, focused, background) = (self.border, self.focused, self.background);
        let mut content = div().absolute().left(px(inset)).top(px(inset)).right(px(right)).bottom(px(bottom)).track_scroll(&self.handle);
        // (A hidden bar's content scrolls all the same: clipped to the
        // view, and measured, so the bar comes when it overflows.)
        let vscroll = vbar || (self.vertical && self.auto_hide);
        content = match (vscroll, hbar) {
            (true, true) => content.overflow_scroll(),
            (true, false) => content.overflow_y_scroll(),
            (false, true) => content.overflow_x_scroll(),
            (false, false) => content,
        };
        if self.vertical && self.auto_hide {
            // The bar comes and goes with what there is to scroll: the
            // view that holds this says what it shows again when that
            // changes.
            let handle = self.handle.clone();
            content = content.on_scroll(move |_, cx| {
                if (handle.max_offset().height > px(0.5)) != overflows {
                    cx.invalidate_owner();
                }
            });
        }
        let backdrop = canvas(move |g, b| {
            let p = theme::palette();
            let inner = style::inset(g, b, if border { 2.0 } else { 0.0 });
            crate::review::scroll_area(g, inner);
            let (r, bt) = (if vbar { px(SCROLL_BAR) } else { px(0.0) }, if hbar { px(SCROLL_BAR) } else { px(0.0) });
            if let Some(bg) = background {
                style::fill_rect(g, Bounds::from_corners(inner.origin, point(inner.right() - r, inner.bottom() - bt)), style::c(bg));
            }
            if vbar && hbar {
                style::fill_rect(g, Bounds::from_corners(point(inner.right() - r, inner.bottom() - bt), inner.bottom_right()), style::c(p.panel_background));
            }
        })
        .paint_key(style::key(&(border, vbar, hbar, background)))
        .absolute()
        .inset_0();
        let frame = canvas(move |g, b| {
            if border {
                let p = theme::palette();
                style::sunken(g, b, &p, focused);
            }
        })
        .paint_key(style::key(&(border, focused)))
        .absolute()
        .inset_0();
        let mut root = div().relative().size_full().child(backdrop).child(content.children(self.children));
        // The bars, inside the frame.
        let corner = |other: bool| inset + if other { SCROLL_BAR } else { 0.0 };
        if vbar {
            root = root.child(div().absolute().top(px(inset)).right(px(inset)).bottom(px(corner(hbar))).w(px(SCROLL_BAR)).child(ScrollBar::vertical(&self.handle)));
        }
        if hbar {
            root = root.child(div().absolute().left(px(inset)).bottom(px(inset)).right(px(corner(vbar))).h(px(SCROLL_BAR)).child(ScrollBar::horizontal(&self.handle)));
        }
        root.child(frame)
    }
}
