//! Choosing what to crop a picture to, drawn over it: drag out a frame,
//! move it from inside, or drag its corners and edges; what will go is
//! dimmed, the frame's thirds show while it moves, and its size shows
//! under it.

use olo_preview::edit::Op;
use olo_ui::{
    agent,
    gpui::{self, App, Bounds, CursorStyle, FontWeight, Pixels, Point, Window, point, px, size},
    style::{self, Align},
    theme,
};

const HANDLE: f32 = 7.0;

#[derive(Debug, Clone, Copy)]
enum Grab {
    /// A new frame from this corner (picture pixels).
    New([f32; 2]),
    Move([f32; 2]),
    /// Which edges follow the pointer: left, right, top, bottom.
    Edges(bool, bool, bool, bool),
}

#[derive(Debug, Default, Clone)]
pub struct CropFrame {
    /// In picture pixels: left, top, right, bottom.
    pub sel: Option<[f32; 4]>,
    drag: Option<Grab>,
    /// The pointer's shape where it last was (set when painting).
    shape: Option<CursorStyle>,
}

/// A picture point (its pixels) for window point `p`, the picture drawn
/// at `shown` and `full` pixels big.
fn to_px(p: Point<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) -> [f32; 2] {
    let k = f32::from(shown.size.width) / (full.0 as f32).max(1.0);
    [(f32::from(p.x - shown.left()) / k).clamp(0.0, full.0 as f32), (f32::from(p.y - shown.top()) / k).clamp(0.0, full.1 as f32)]
}

fn to_window(r: [f32; 4], shown: Bounds<Pixels>, full: (u32, u32)) -> Bounds<Pixels> {
    let k = f32::from(shown.size.width) / (full.0 as f32).max(1.0);
    Bounds::from_corners(point(shown.left() + px(r[0] * k), shown.top() + px(r[1] * k)), point(shown.left() + px(r[2] * k), shown.top() + px(r[3] * k)))
}

impl CropFrame {
    /// The crop step for the frame chosen, if there is one.
    pub fn op(&self) -> Option<Op> {
        let r = self.sel?;
        let (w, h) = ((r[2] - r[0]).round() as u32, (r[3] - r[1]).round() as u32);
        (w >= 1 && h >= 1).then(|| Op::Crop { x: r[0].round() as u32, y: r[1].round() as u32, w, h })
    }

    /// What's under the pointer.
    fn grab_at(&self, pos: Point<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) -> Option<Grab> {
        let s = to_window(self.sel?, shown, full);
        let near = |a: Pixels, b: Pixels| (a - b).abs() <= px(HANDLE);
        let inside_y = pos.y >= s.top() - px(HANDLE) && pos.y <= s.bottom() + px(HANDLE);
        let inside_x = pos.x >= s.left() - px(HANDLE) && pos.x <= s.right() + px(HANDLE);
        let (l, r, t, b) = (near(pos.x, s.left()) && inside_y, near(pos.x, s.right()) && inside_y, near(pos.y, s.top()) && inside_x, near(pos.y, s.bottom()) && inside_x);
        if l || r || t || b {
            Some(Grab::Edges(l, r && !l, t, b && !t))
        } else if s.contains(&pos) {
            Some(Grab::Move(to_px(pos, shown, full)))
        } else {
            None
        }
    }

    /// The pointer moved to `pos`; true when its shape changed.
    pub fn hover(&mut self, pos: Point<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) -> bool {
        let shape = Some(self.cursor(pos, shown, full));
        std::mem::replace(&mut self.shape, shape) != shape
    }

    /// The pointer's shape where it last was.
    pub fn shape(&self) -> CursorStyle {
        self.shape.unwrap_or(CursorStyle::Crosshair)
    }

    /// The pointer's shape at `pos`.
    fn cursor(&self, pos: Point<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) -> CursorStyle {
        match self.drag.or_else(|| self.grab_at(pos, shown, full)) {
            Some(Grab::Move(_)) => CursorStyle::OpenHand,
            Some(Grab::Edges(l, r, t, b)) => match (l || r, t || b) {
                (true, true) => {
                    if (l && t) || (r && b) {
                        CursorStyle::ResizeUpLeftDownRight
                    } else {
                        CursorStyle::ResizeUpRightDownLeft
                    }
                }
                (true, false) => CursorStyle::ResizeLeftRight,
                _ => CursorStyle::ResizeUpDown,
            },
            _ => CursorStyle::Crosshair,
        }
    }

    /// A press at `pos`: a grab of the frame, or a new one. A press
    /// outside the frame that doesn't drag drops it (see [`release`]).
    pub fn press(&mut self, pos: Point<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) {
        self.drag = Some(self.grab_at(pos, shown, full).unwrap_or(Grab::New(to_px(pos, shown, full))));
    }

    /// The pointer dragged to `pos`; true when the frame changed.
    pub fn drag_to(&mut self, pos: Point<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) -> bool {
        let Some(grab) = self.drag else { return false };
        let at = to_px(pos, shown, full);
        let (fw, fh) = (full.0 as f32, full.1 as f32);
        let sel = match grab {
            Grab::New(from) => Some([from[0].min(at[0]), from[1].min(at[1]), from[0].max(at[0]), from[1].max(at[1])]),
            Grab::Move(from) => self.sel.map(|s| {
                let (dx, dy) = (at[0] - from[0], at[1] - from[1]);
                let mut m = [s[0] + dx, s[1] + dy, s[2] + dx, s[3] + dy];
                // Kept inside the picture.
                let sx = (0.0 - m[0]).max(0.0) - (m[2] - fw).max(0.0);
                let sy = (0.0 - m[1]).max(0.0) - (m[3] - fh).max(0.0);
                m = [m[0] + sx, m[1] + sy, m[2] + sx, m[3] + sy];
                self.drag = Some(Grab::Move(at));
                m
            }),
            Grab::Edges(l, r, t, b) => self.sel.map(|mut s| {
                if l {
                    s[0] = at[0].min(s[2] - 1.0);
                }
                if r {
                    s[2] = at[0].max(s[0] + 1.0);
                }
                if t {
                    s[1] = at[1].min(s[3] - 1.0);
                }
                if b {
                    s[3] = at[1].max(s[1] + 1.0);
                }
                s
            }),
        };
        let sel = sel
            .map(|s| [s[0].max(0.0).round(), s[1].max(0.0).round(), s[2].min(fw).round(), s[3].min(fh).round()])
            .filter(|s| s[2] - s[0] >= 1.0 && s[3] - s[1] >= 1.0);
        let changed = sel != self.sel;
        self.sel = sel;
        changed
    }

    /// The button let go; `moved`: whether the pointer dragged at all (a
    /// click outside the frame drops it).
    pub fn release(&mut self, moved: bool) {
        if !moved && matches!(self.drag, Some(Grab::New(_))) {
            self.sel = None;
        }
        self.drag = None;
    }

    /// Dim what will go, outline what stays.
    pub fn paint(&self, window: &mut Window, cx: &mut App, view: Bounds<Pixels>, shown: Bounds<Pixels>, full: (u32, u32)) {
        let p = theme::palette(cx);
        let shade = style::c(p.scrim);
        window.with_content_mask(Some(gpui::ContentMask { bounds: view }), |window| {
            let Some(r) = self.sel else {
                window.paint_quad(gpui::fill(shown, shade.opacity(0.4)));
                return;
            };
            let s = style::snap(window, to_window(r, shown, full));
            for b in [
                Bounds::from_corners(shown.origin, point(shown.right(), s.top())),
                Bounds::from_corners(point(shown.left(), s.bottom()), shown.bottom_right()),
                Bounds::from_corners(point(shown.left(), s.top()), point(s.left(), s.bottom())),
                Bounds::from_corners(point(s.right(), s.top()), point(shown.right(), s.bottom())),
            ] {
                window.paint_quad(gpui::fill(b, shade));
            }
            style::frame_rect(window, s, gpui::white());
            // Dark just outside it, a line out (not a point: at 1.5× that
            // leaves a gap), to show on light pictures too.
            style::frame_rect(window, style::inset(window, s, -1.0), gpui::black().opacity(160.0 / 255.0));
            // Thirds, while it is being dragged.
            if self.drag.is_some() {
                let guide = gpui::white().opacity(110.0 / 255.0);
                for i in 1..3 {
                    let x = s.left() + s.size.width * (i as f32 / 3.0);
                    let y = s.top() + s.size.height * (i as f32 / 3.0);
                    style::vline(window, x, s.top(), s.bottom(), guide);
                    style::hline(window, s.left(), s.right(), y, guide);
                }
            }
            for c in [s.origin, point(s.right(), s.top()), point(s.left(), s.bottom()), s.bottom_right()] {
                let h = style::snap_centred(window, c, HANDLE, HANDLE);
                window.paint_quad(gpui::fill(h, gpui::white()));
                style::frame_rect(window, h, style::c(p.keyboard_navigation));
            }
            // The size it will be.
            let text = format!("{} × {}", (r[2] - r[0]).round(), (r[3] - r[1]).round());
            let font = style::ui_font(FontWeight::NORMAL);
            let w = f32::from(style::text_width(window, text.clone(), style::SMALL_SIZE, font.clone()));
            let h = (style::SMALL_SIZE * style::LINE_HEIGHT).ceil();
            let top = (s.bottom() + px(6.0)).min(view.bottom() - px(h + 10.0));
            let tip = style::snap(window, Bounds::new(point(s.center().x - px(w / 2.0 + 5.0), top), size(px(w + 10.0), px(h + 4.0))));
            window.paint_quad(gpui::fill(tip, style::c(p.tooltip_background)).corner_radii(px(4.0)));
            window.paint_quad(gpui::outline(tip, style::c(p.control_border), gpui::BorderStyle::Solid).corner_radii(px(4.0)));
            let at = style::label(window, cx, tip.center().x, tip.center().y, Align::Center, text.clone(), style::SMALL_SIZE, font, style::c(p.tooltip_text));
            agent::record(window, cx, agent::Node::label(text, at));
        });
    }
}
