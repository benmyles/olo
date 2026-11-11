//! A word processor's ruler, after Gobe Productive's and ClarisWorks'
//! (StyledEdit's): a scale across the text in inches or
//! centimetres (as the locale measures, [`metric`]), a paragraph's
//! indents as markers to drag (the first line's hanging from the top; the
//! left and right ones standing on the bottom, with a box under the left
//! one that moves it and the first line together) and its tab stops
//! along the bottom of the scale: a click on the scale sets a left one,
//! one dragged moves, dragged off the ruler it goes, and double-clicked
//! (or chosen from its right-click menu) it lines text up another way:
//! left, centre, right, on the decimal point.
//!
//! Lengths are points from the text's left edge (the right indent from
//! its right edge), as [`crate::editor::Paragraph`] takes them; what is
//! dragged snaps to sixteenths of an inch, or quarter centimetres. The
//! owner gives it the paragraph's setting and where the text is
//! ([`Ruler::set`]) and hears [`RulerEvent`]s: a change when a marker is
//! let go, and where one is while it's held ([`Ruler::guide`], for a line
//! down the text). Agents read each marker as a slider ("Left Indent",
//! "Tab 1.5 in, Right") at its place.

use olo_interface::{
    Bounds, CursorStyle, Cx, Element, FontWeight, InteractiveElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, PathBuilder, Pixels, Point, Styled as _, View, point, px,
};

use crate::{
    agent,
    editor::{TabAlign, TabStop},
    menu::Menu,
    style, theme,
};

/// The ruler's height, in points.
pub const HEIGHT: f32 = 26.0;

/// A paragraph's indents, in points: `left` from the text's left edge,
/// `first` (the first line's) further in from `left`, `right` from the
/// text's right edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Indents {
    pub left: f32,
    pub first: f32,
    pub right: f32,
}

/// What a ruler shows and changes: indents and tab stops (left to right).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setting {
    pub indents: Indents,
    pub tabs: Vec<TabStop>,
}

/// What happened on a ruler.
#[derive(Clone, Debug)]
pub enum RulerEvent {
    /// The setting as changed, when a marker is let go (or a tab set,
    /// turned or taken off).
    Changed(Setting),
    /// A marker moved while held ([`Ruler::guide`]).
    Guide,
}

/// Inches where the locale measures in them (the US, Liberia, Myanmar:
/// `LC_MEASUREMENT`, else `LC_ALL` or `LANG`), else centimetres.
pub fn metric() -> bool {
    let lang = ["LC_MEASUREMENT", "LC_ALL", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty() && v != "C" && v != "POSIX").unwrap_or_default();
    let territory = lang.split(['.', '@']).next().unwrap_or("").rsplit('_').next().unwrap_or("");
    !matches!(territory, "US" | "LR" | "MM")
}

/// Points in an inch, and a centimetre.
const INCH: f32 = 72.0;
const CM: f32 = 72.0 / 2.54;
/// How near a marker the pointer takes hold of it, in points.
const REACH: f32 = 5.0;
/// The narrowest the text between the indents is dragged to.
const MIN_TEXT: f32 = 36.0;
/// A tab dragged this far above or below the ruler comes off.
const OFF: f32 = 14.0;

/// A length as the ruler's unit has it ("1.5 in", "3.25 cm").
pub fn length(points: f32, metric: bool) -> String {
    let (v, unit) = if metric { (points / CM, "cm") } else { (points / INCH, "in") };
    let v = (v * 100.0).round() / 100.0;
    format!("{} {unit}", trim(v))
}

fn trim(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn align_name(a: TabAlign) -> &'static str {
    match a {
        TabAlign::Left => "Left",
        TabAlign::Center => "Centre",
        TabAlign::Right => "Right",
        TabAlign::Decimal => "Decimal",
    }
}

fn next_align(a: TabAlign) -> TabAlign {
    match a {
        TabAlign::Left => TabAlign::Center,
        TabAlign::Center => TabAlign::Right,
        TabAlign::Right => TabAlign::Decimal,
        TabAlign::Decimal => TabAlign::Left,
    }
}

/// What is held.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    First,
    Left,
    /// The box under the left indent: it and the first line.
    Both,
    Right,
    Tab(usize),
}

#[derive(Clone, Debug)]
struct Drag {
    held: Held,
    start: Setting,
    press: f32,
    now: Setting,
    /// A tab dragged off: it goes when let go.
    off: bool,
}

#[derive(Clone)]
pub struct Ruler {
    setting: Setting,
    /// Where the scale's 0 is across the window, and how many points of
    /// text wide it is (the right indent measured back from there).
    origin: f32,
    width: f32,
    drag: Option<Drag>,
    metric: bool,
    /// The last press, for a click without a drag.
    press: Option<(Point<Pixels>, Option<Held>)>,
    /// Where the ruler is in the window (as its handlers last found).
    bounds: Bounds<Pixels>,
    /// The pointer is over a marker.
    over: bool,
}

impl Default for Ruler {
    fn default() -> Self {
        Ruler::new()
    }
}

impl Ruler {
    pub fn new() -> Ruler {
        Ruler { setting: Setting::default(), origin: 0.0, width: 1.0, drag: None, metric: metric(), press: None, bounds: Bounds::default(), over: false }
    }

    /// What it shows: `setting`, the scale's 0 at window x `origin`,
    /// `width` points of text wide.
    pub fn set(&mut self, setting: &Setting, origin: f32, width: f32, cx: &mut Cx<Self>) {
        if self.setting != *setting || self.origin != origin || self.width != width {
            self.setting = setting.clone();
            self.origin = origin;
            self.width = width;
            cx.invalidate();
        }
    }

    /// The text it measures moved (where it starts across the window,
    /// and its width) as the frame being drawn was laid out: followed in
    /// that frame, as the ruler is painted after.
    pub fn follow(&mut self, origin: f32, width: f32) {
        self.origin = origin;
        self.width = width;
    }

    /// Where the marker being dragged is (window x), for a line down the
    /// text.
    pub fn guide(&self) -> Option<f32> {
        let d = self.drag.as_ref().filter(|d| !d.off)?;
        let i = d.now.indents;
        Some(
            self.origin
                + match d.held {
                    Held::First => i.left + i.first,
                    Held::Left | Held::Both => i.left,
                    Held::Right => self.width - i.right,
                    Held::Tab(k) => d.now.tabs.get(k).map_or(0.0, |t| t.x),
                },
        )
    }

    fn step(&self) -> f32 {
        if self.metric { CM / 4.0 } else { INCH / 16.0 }
    }

    fn snap_len(&self, v: f32) -> f32 {
        (v / self.step()).round() * self.step()
    }

    /// The scale's strip (the paper behind the numbers).
    fn strip(&self) -> (f32, f32) {
        (f32::from(self.bounds.top()) + 6.0, f32::from(self.bounds.bottom()) - 7.0)
    }

    fn hit(&self, pos: Point<Pixels>, s: &Setting) -> Option<Held> {
        let i = s.indents;
        let (x, y) = (f32::from(pos.x), f32::from(pos.y));
        let x_of = |v: f32| self.origin + v;
        let near = |at: f32| (x - at).abs() <= REACH;
        let (top, bottom) = self.strip();
        let low = y >= (top + bottom) / 2.0;
        if !low && near(x_of(i.left + i.first)) {
            return Some(Held::First);
        }
        if low && near(x_of(i.left)) {
            return Some(if y >= bottom + 1.0 { Held::Both } else { Held::Left });
        }
        if low && near(x_of(self.width - i.right)) {
            return Some(Held::Right);
        }
        if low {
            // The nearest tab in reach.
            let best = s.tabs.iter().enumerate().map(|(k, t)| (k, (x - x_of(t.x)).abs())).filter(|(_, d)| *d <= REACH).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((k, _)) = best {
                return Some(Held::Tab(k));
            }
        }
        None
    }

    fn down(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        self.bounds = cx.listener_bounds();
        let held = self.hit(e.position, &self.setting);
        if e.button == MouseButton::Right {
            let k = match held {
                Some(Held::Tab(k)) => Some(k),
                _ => None,
            };
            return self.menu(k, e.position, cx);
        }
        if e.click_count >= 2 {
            // A double click on a tab turns it.
            if let Some(Held::Tab(k)) = held {
                let mut s = self.setting.clone();
                s.tabs[k].align = next_align(s.tabs[k].align);
                self.setting = s.clone();
                cx.emit(RulerEvent::Changed(s));
                cx.invalidate();
            }
            self.press = None;
            return;
        }
        self.press = Some((e.position, held));
        if let Some(held) = held {
            self.drag = Some(Drag { held, start: self.setting.clone(), press: f32::from(e.position.x), now: self.setting.clone(), off: false });
            cx.emit(RulerEvent::Guide);
            cx.invalidate();
        }
    }

    fn moved(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let width = self.width;
        let step = self.step();
        let snap_len = |v: f32| (v / step).round() * step;
        let (top, bottom) = (f32::from(self.bounds.top()), f32::from(self.bounds.bottom()));
        let Some(d) = &mut self.drag else { return };
        let dx = f32::from(e.position.x) - d.press;
        let s = d.start.indents;
        let mut now = d.start.clone();
        let line = |left: f32, first: f32| left + first;
        match d.held {
            Held::First => {
                let at = snap_len(line(s.left, s.first) + dx).clamp(s.left, (width - s.right - MIN_TEXT).max(s.left));
                now.indents.first = at - s.left;
            }
            Held::Left => {
                // The first line stays where it is, unless passed.
                let left = snap_len(s.left + dx).clamp(0.0, (width - s.right - MIN_TEXT).max(0.0));
                now.indents.left = left;
                now.indents.first = (line(s.left, s.first) - left).max(0.0);
            }
            Held::Both => {
                now.indents.left = snap_len(s.left + dx).clamp(0.0, (width - s.right - s.first - MIN_TEXT).max(0.0));
            }
            Held::Right => {
                let x = snap_len(width - s.right + dx).clamp(line(s.left, s.first).max(s.left) + MIN_TEXT, width);
                now.indents.right = (width - x).max(0.0);
            }
            Held::Tab(k) => {
                if let Some(t) = now.tabs.get_mut(k) {
                    t.x = snap_len(d.start.tabs[k].x + dx).clamp(0.0, width);
                }
                let y = f32::from(e.position.y);
                d.off = y < top - OFF || y > bottom + OFF;
            }
        }
        if d.now != now || d.off {
            d.now = now;
            cx.emit(RulerEvent::Guide);
            cx.invalidate();
        }
    }

    fn up(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        let press = self.press.take();
        if let Some(d) = self.drag.take() {
            let mut s = d.now;
            if let (Held::Tab(k), true) = (d.held, d.off) {
                s.tabs.remove(k);
            }
            s.tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
            s.tabs.dedup_by(|a, b| (a.x - b.x).abs() < 0.5);
            if s != self.setting {
                self.setting = s.clone();
                cx.emit(RulerEvent::Changed(s));
            }
            cx.emit(RulerEvent::Guide);
            cx.invalidate();
            return;
        }
        // A click on the scale sets a tab.
        if let Some((at, None)) = press
            && (e.position - at).x.abs() < px(3.0)
            && self.bounds.contains(&e.position)
        {
            let x = self.snap_len(f32::from(e.position.x) - self.origin);
            if (0.0..=self.width).contains(&x) && !self.setting.tabs.iter().any(|t| (t.x - x).abs() < 0.5) {
                let mut s = self.setting.clone();
                s.tabs.push(TabStop { x, align: TabAlign::Left });
                s.tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
                self.setting = s.clone();
                cx.emit(RulerEvent::Changed(s));
                cx.invalidate();
            }
        }
    }

    /// A tab's own menu (tab `k`, if one was clicked), and Clear All Tabs.
    fn menu(&mut self, k: Option<usize>, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let mut menu = Menu::new("Ruler");
        if let Some(t) = k.and_then(|k| self.setting.tabs.get(k)) {
            for a in [TabAlign::Left, TabAlign::Center, TabAlign::Right, TabAlign::Decimal] {
                menu = menu.item(format!("{} Tab", align_name(a)), format!("align:{}", align_name(a))).checked(t.align == a);
            }
            menu = menu.separator().item("Remove Tab", "remove");
        }
        menu = menu.separator().item("Clear All Tabs", "clear").enabled(!self.setting.tabs.is_empty());
        let me = cx.view_ref();
        crate::menu::context_menu(cx, menu, at, move |cmd, cx| {
            let cmd = cmd.to_owned();
            me.update(cx, move |r, cx| {
                let mut s = r.setting.clone();
                match (cmd.as_str(), k) {
                    ("clear", _) => s.tabs.clear(),
                    ("remove", Some(k)) if k < s.tabs.len() => {
                        s.tabs.remove(k);
                    }
                    (a, Some(k)) if k < s.tabs.len() => {
                        if let Some(align) = [TabAlign::Left, TabAlign::Center, TabAlign::Right, TabAlign::Decimal].into_iter().find(|x| a == format!("align:{}", align_name(*x))) {
                            s.tabs[k].align = align;
                        }
                    }
                    _ => {}
                }
                if s != r.setting {
                    r.setting = s.clone();
                    cx.emit(RulerEvent::Changed(s));
                    cx.invalidate();
                }
            });
        });
    }

    /// Draws it in `rect`.
    fn paint(&self, rect: Bounds<Pixels>, window: &mut Painter) {
        // (Its marks sit on its edges, where they set the text's indents:
        // a grid of its own to the layout review.)
        crate::review::own_grid(window, rect);
        let p = theme::palette();
        let s = window.scale_factor();
        let snap = |v: f32| (v * s).round() / s;
        let line = style::line_pixels(s) / s;
        let shown = self.drag.as_ref().map_or(&self.setting, |d| &d.now);
        let ind = shown.indents;
        let x_of = |v: f32| self.origin + v;
        let (first_x, left_x, right_x) = (x_of(ind.left + ind.first), x_of(ind.left), x_of(self.width - ind.right));
        let (st, sb) = self.strip();
        let (top, bottom) = (f32::from(rect.top()), f32::from(rect.bottom()));

        // The panel, the text's strip and its scale, then the markers.
        style::fill_rect(window, rect, style::c(p.panel_background));
        let paper = Bounds::from_corners(point(px(snap(self.origin)), px(snap(st))), point(px(snap(self.origin + self.width)), px(snap(sb))));
        style::fill_rect(window, paper, style::c(p.document_background));
        style::frame_rect(window, Bounds::from_corners(point(paper.left() - px(line), paper.top() - px(line)), point(paper.right() + px(line), paper.bottom() + px(line))), style::c(p.control_border));
        let ink = style::c(p.document_text);
        let faint = ink.opacity(0.55);
        let (unit, parts) = if self.metric { (CM, 4) } else { (INCH, 8) };
        let n = (self.width / unit * parts as f32).floor() as i32;
        let base = f32::from(paper.bottom());
        let mid = f32::from(paper.center().y);
        for k in 1..=n {
            let x = snap(self.origin + k as f32 * unit / parts as f32);
            if k % parts == 0 {
                // The number, in the strip's middle.
                style::label(window, px(x), px(mid), style::Align::Center, (k / parts).to_string(), 9.0, style::ui_font(FontWeight::NORMAL), ink);
                continue;
            }
            let h = if k % (parts / 2) == 0 {
                5.0
            } else if parts == 8 && k % 2 == 0 {
                3.0
            } else {
                2.0
            };
            style::fill_rect(window, Bounds::from_corners(point(px(x), px(snap(base - h))), point(px(x + line), px(base))), faint);
        }

        // Tabs: their marks under the scale, standing on the ruler's bottom.
        let thick = ((1.5 * s).round() / s).max(line);
        let dragged_tab = self.drag.as_ref().and_then(|d| if let Held::Tab(k) = d.held { Some((k, d.off)) } else { None });
        for (k, t) in shown.tabs.iter().enumerate() {
            let off = dragged_tab == Some((k, true));
            let color = if off { faint.opacity(0.275) } else { style::c(p.panel_text) };
            let x = snap(x_of(t.x));
            let y = snap(bottom - 1.0);
            style::fill_rect(window, Bounds::from_corners(point(px(x - thick / 2.0), px(base + line)), point(px(x + thick / 2.0), px(y))), color);
            let (l, r) = match t.align {
                TabAlign::Left => (x, x + 5.0),
                TabAlign::Right => (x - 5.0, x),
                TabAlign::Center | TabAlign::Decimal => (x - 3.5, x + 3.5),
            };
            style::fill_rect(window, Bounds::from_corners(point(px(l), px(y - thick)), point(px(r), px(y))), color);
            if t.align == TabAlign::Decimal {
                let b = style::snap_centred(window, point(px(x + 2.5), px(y - 3.0)), thick, thick);
                style::fill_rect(window, b, color);
            }
        }

        // Markers: filled in the control's colour, outlined in ink.
        let fill = style::c(p.control_background);
        let edge = style::c(p.control_text);
        let half = 4.5;
        let tri = |window: &mut Painter, ax: f32, ay: f32, down: bool| {
            let (ax, ay) = (snap(ax), snap(ay));
            let by = if down { ay - 6.0 } else { ay + 6.0 };
            let pts = [point(px(ax), px(ay)), point(px(ax - half), px(by)), point(px(ax + half), px(by))];
            let mut f = PathBuilder::fill();
            f.move_to(pts[0]);
            f.line_to(pts[1]);
            f.line_to(pts[2]);
            f.close();
            if let Ok(path) = f.build() {
                window.paint_path(path, fill);
            }
            let mut o = PathBuilder::stroke(px(line));
            o.move_to(pts[0]);
            o.line_to(pts[1]);
            o.line_to(pts[2]);
            o.close();
            if let Ok(path) = o.build() {
                window.paint_path(path, edge);
            }
        };
        tri(window, first_x, st + 4.0, true);
        tri(window, left_x, sb - 4.0, false);
        let b = Bounds::from_corners(point(px(snap(left_x - half)), px(snap(sb + 2.0))), point(px(snap(left_x + half)), px(snap(bottom - 1.0))));
        style::fill_rect(window, b, fill);
        style::frame_rect(window, b, edge);
        tri(window, right_x, sb - 4.0, false);

        // For agents: each marker, as a slider at its place.
        let mid_y = (st + sb) / 2.0;
        let describe = |window: &mut Painter, name: String, value: f32, x: f32, upper: bool| {
            let r = if upper {
                Bounds::from_corners(point(px(x - half), px(top)), point(px(x + half), px(mid_y)))
            } else {
                Bounds::from_corners(point(px(x - half), px(mid_y)), point(px(x + half), px(bottom)))
            };
            agent::record(
                window,
                agent::Node { role: "Slider", name: Some(name.into()), value: Some(length(value, self.metric).into()), bounds: r, enabled: true, clickable: true, ..Default::default() },
            );
        };
        describe(window, "First Line Indent".into(), ind.left + ind.first, first_x, true);
        describe(window, "Left Indent".into(), ind.left, left_x, false);
        describe(window, "Right Indent".into(), ind.right, right_x, false);
        for t in &shown.tabs {
            describe(window, format!("Tab {}, {}", length(t.x, self.metric), align_name(t.align)), t.x, x_of(t.x), false);
        }
    }
}

impl View for Ruler {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        cx.canvas(|r: &Ruler, g, b| {
            if r.over || r.drag.is_some() {
                g.set_cursor_style(CursorStyle::ResizeLeftRight, b);
            }
            // One layer for the lot: a ruler across a wide window is
            // hundreds of ticks, each its own place in the scene
            // otherwise. (Its marks are paths, drawn over the ticks'
            // fills; its numbers glyphs, over both.)
            g.paint_layer(b, |g| r.paint(b, g));
        })
        .w_full()
        .h(px(HEIGHT))
        .on_any_mouse_down(cx.listener(|r: &mut Ruler, e: &MouseDownEvent, cx| {
            r.down(e, cx);
            cx.stop_propagation();
        }))
        // (Over a marker, the pointer says it can be dragged.)
        .on_mouse_move(cx.listener(|r: &mut Ruler, e: &MouseMoveEvent, cx| {
            r.bounds = cx.listener_bounds();
            let over = r.hit(e.position, &r.setting).is_some();
            if over != r.over {
                r.over = over;
                cx.invalidate();
            }
        }))
        .on_hover(cx.listener(|r: &mut Ruler, inside: &bool, cx| {
            if !*inside && std::mem::take(&mut r.over) {
                cx.invalidate();
            }
        }))
        // (Anywhere in the window: a marker held goes on being dragged.)
        .on_window_mouse_move(cx.listener(|r: &mut Ruler, e: &MouseMoveEvent, cx| {
            if r.drag.is_some() {
                r.bounds = cx.listener_bounds();
                r.moved(e, cx);
            }
        }))
        .on_window_mouse_up(cx.listener(|r: &mut Ruler, e: &MouseUpEvent, cx| {
            if e.button == MouseButton::Left && (r.drag.is_some() || r.press.is_some()) {
                r.bounds = cx.listener_bounds();
                r.up(e, cx);
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(length(108.0, false), "1.5 in");
        assert_eq!(length(72.0, false), "1 in");
        assert_eq!(length(CM * 3.25, true), "3.25 cm");
        assert_eq!(next_align(TabAlign::Decimal), TabAlign::Left);
    }
}
