//! A word processor's ruler, after Gobe Productive's and ClarisWorks':
//! a scale across the text in inches or centimetres (as the locale
//! measures, `metric`), a paragraph's indents as markers to drag (the
//! first line's hanging from the top; the left and right ones standing
//! on the bottom, with a box under the left one that moves it and the
//! first line together) and its tab stops along the bottom of the
//! scale: a click on the scale sets a left one, one dragged moves,
//! dragged off the ruler it goes, and double-clicked (or chosen from its
//! right-click menu) it lines text up another way: left, centre, right,
//! on the decimal point.
//!
//! Lengths are points from the text's left edge (the right indent from
//! its right edge), as [`crate::editor::Paragraph`] takes them; what is
//! dragged snaps to sixteenths of an inch, or quarter centimetres. While
//! a marker is held, [`Ruled::guide`] says where, for a line down the
//! text. Agents read each marker as a slider ("Left Indent", "Tab
//! 1.5 in, Right") at its place.

use egui::{
    CursorIcon, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, accesskit,
    epaint::text::{TabAlign, TabStop},
    pos2, vec2,
};

use crate::{
    menu::Menu,
    style::{self, c},
    widgets,
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

pub struct Ruled {
    pub response: Response,
    /// The setting as changed, when a marker is let go (or a tab set,
    /// turned or taken off).
    pub changed: Option<Setting>,
    /// Where the marker being dragged is (screen x), for a line down the
    /// text.
    pub guide: Option<f32>,
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

/// The ruler in `rect`, its scale's 0 at screen x `origin`, `width`
/// points of text wide (the right indent measured back from there).
pub fn show(ui: &mut Ui, id: Id, rect: Rect, origin: f32, width: f32, setting: &Setting) -> Ruled {
    let ctx = ui.ctx().clone();
    let p = style::palette(&ctx);
    let metric = metric();
    let step = if metric { CM / 4.0 } else { INCH / 16.0 };
    let snap_len = |v: f32| (v / step).round() * step;
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut drag: Option<Drag> = ctx.data(|d| d.get_temp(id));
    let mut changed = None;

    // Where things are: the scale's strip and the markers.
    let strip = Rect::from_min_max(pos2(rect.left(), rect.top() + 6.0), pos2(rect.right(), rect.bottom() - 7.0));
    let x_of = |v: f32| origin + v;

    let hit = |pos: Pos2, s: &Setting| -> Option<Held> {
        let i = s.indents;
        let near = |x: f32| (pos.x - x).abs() <= REACH;
        let low = pos.y >= strip.center().y;
        if !low && near(x_of(i.left + i.first)) {
            return Some(Held::First);
        }
        if low && near(x_of(i.left)) {
            return Some(if pos.y >= strip.bottom() + 1.0 { Held::Both } else { Held::Left });
        }
        if low && near(x_of(width - i.right)) {
            return Some(Held::Right);
        }
        if low {
            // The nearest tab in reach.
            let best = s.tabs.iter().enumerate().map(|(k, t)| (k, (pos.x - x_of(t.x)).abs())).filter(|(_, d)| *d <= REACH).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((k, _)) = best {
                return Some(Held::Tab(k));
            }
        }
        None
    };

    let hovered = response.hover_pos().and_then(|pos| hit(pos, setting));
    if hovered.is_some() || drag.is_some() {
        ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
    }

    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(held) = hit(ctx.input(|i| i.pointer.press_origin()).unwrap_or(pos), setting)
    {
        drag = Some(Drag { held, start: setting.clone(), press: ctx.input(|i| i.pointer.press_origin()).map_or(pos.x, |o| o.x), now: setting.clone(), off: false });
    }
    if let Some(d) = &mut drag
        && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
    {
        let dx = pos.x - d.press;
        let s = &d.start.indents;
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
                d.off = pos.y < rect.top() - OFF || pos.y > rect.bottom() + OFF;
            }
        }
        d.now = now;
    }
    let guide = drag.as_ref().filter(|d| !d.off).map(|d| {
        let i = d.now.indents;
        x_of(match d.held {
            Held::First => i.left + i.first,
            Held::Left | Held::Both => i.left,
            Held::Right => width - i.right,
            Held::Tab(k) => d.now.tabs.get(k).map_or(0.0, |t| t.x),
        })
    });
    if response.drag_stopped()
        && let Some(d) = drag.take()
    {
        let mut s = d.now;
        if let (Held::Tab(k), true) = (d.held, d.off) {
            s.tabs.remove(k);
        }
        s.tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
        s.tabs.dedup_by(|a, b| (a.x - b.x).abs() < 0.5);
        if s != *setting {
            changed = Some(s);
        }
    }
    // A click on the scale sets a tab; a double click on one turns it.
    if let Some(pos) = response.interact_pointer_pos() {
        if response.double_clicked() {
            if let Some(Held::Tab(k)) = hit(pos, setting) {
                let mut s = setting.clone();
                s.tabs[k].align = next_align(s.tabs[k].align);
                changed = Some(s);
            }
        } else if response.clicked() && hit(pos, setting).is_none() {
            let x = snap_len(pos.x - origin);
            if (0.0..=width).contains(&x) && !setting.tabs.iter().any(|t| (t.x - x).abs() < 0.5) {
                let mut s = setting.clone();
                s.tabs.push(TabStop { x, align: TabAlign::Left });
                s.tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
                changed = Some(s);
            }
        }
    }
    // A tab's own menu.
    let menu_tab = id.with("menu-tab");
    if response.secondary_clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let k = match hit(pos, setting) {
            Some(Held::Tab(k)) => Some(k),
            _ => None,
        };
        ctx.data_mut(|d| d.insert_temp(menu_tab, k));
    }
    let chosen = widgets::context_menu(&response, || {
        let k: Option<usize> = ctx.data(|d| d.get_temp(menu_tab)).flatten();
        let mut menu = Menu::new("Ruler");
        if let Some(t) = k.and_then(|k| setting.tabs.get(k)) {
            for a in [TabAlign::Left, TabAlign::Center, TabAlign::Right, TabAlign::Decimal] {
                menu = menu.item(format!("{} Tab", align_name(a)), format!("align:{}", align_name(a)), None).checked(t.align == a);
            }
            menu = menu.separator().item("Remove Tab", "remove", None);
        }
        menu.separator().item("Clear All Tabs", "clear", None).enabled(!setting.tabs.is_empty())
    });
    if let Some(cmd) = chosen {
        let k: Option<usize> = ctx.data(|d| d.get_temp(menu_tab)).flatten();
        let mut s = setting.clone();
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
        if s != *setting {
            changed = Some(s);
        }
    }
    ctx.data_mut(|d| match &drag {
        Some(dr) => {
            d.insert_temp(id, dr.clone());
        }
        None => d.remove::<Drag>(id),
    });
    if drag.is_some() {
        ctx.request_repaint();
    }

    let shown = drag.as_ref().map_or(setting, |d| &d.now);
    let ind = shown.indents;
    let first_x = x_of(ind.left + ind.first);
    let left_x = x_of(ind.left);
    let right_x = x_of(width - ind.right);

    // Drawn: the panel, the text's strip and its scale, then the markers.
    let painter = ui.painter_at(rect);
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let line = style::line_pixels(ppp) / ppp;
    painter.rect_filled(rect, 0.0, c(p.panel_background));
    let paper = Rect::from_min_max(pos2(snap(origin), snap(strip.top())), pos2(snap(origin + width), snap(strip.bottom())));
    painter.rect_filled(paper, 0.0, c(p.document_background));
    style::frame_rect(&painter, paper.expand(line), c(p.control_border));

    let ink = c(p.document_text);
    let faint = ink.gamma_multiply(0.55);
    let font = FontId::proportional(9.0);
    let (unit, parts) = if metric { (CM, 4) } else { (INCH, 8) };
    let n = (width / unit * parts as f32).floor() as i32;
    let base = paper.bottom();
    for k in 1..=n {
        let x = snap(origin + k as f32 * unit / parts as f32);
        if k % parts == 0 {
            // The number, in the strip's middle.
            let label = (k / parts).to_string();
            let g = painter.layout_no_wrap(label, font.clone(), ink);
            let at = pos2(snap(x - g.size().x / 2.0), snap(paper.center().y - g.size().y / 2.0));
            painter.galley(at, g, ink);
            continue;
        }
        let h = if k % (parts / 2) == 0 { 5.0 } else if parts == 8 && k % 2 == 0 { 3.0 } else { 2.0 };
        painter.rect_filled(Rect::from_min_max(pos2(x, snap(base - h)), pos2(x + line, base)), 0.0, faint);
    }

    // Markers: filled in the control's colour, outlined in ink.
    let fill = c(p.control_background);
    let edge = Stroke::new(line, c(p.control_text));
    let half = 4.5;
    let tri = |apex: Pos2, down: bool| {
        let (ax, ay) = (snap(apex.x), snap(apex.y));
        let by = if down { ay - 6.0 } else { ay + 6.0 };
        Shape::convex_polygon(vec![pos2(ax, ay), pos2(ax - half, by), pos2(ax + half, by)], fill, edge)
    };
    let marker_shapes = |painter: &egui::Painter| {
        painter.add(tri(pos2(first_x, strip.top() + 4.0), true));
        painter.add(tri(pos2(left_x, strip.bottom() - 4.0), false));
        let b = Rect::from_min_max(pos2(snap(left_x - half), snap(strip.bottom() + 2.0)), pos2(snap(left_x + half), snap(rect.bottom() - 1.0)));
        painter.rect_filled(b, 0.0, fill);
        style::frame_rect(painter, b, edge.color);
        painter.add(tri(pos2(right_x, strip.bottom() - 4.0), false));
    };
    // Tabs: their marks under the scale, standing on the ruler's bottom.
    let thick = ((1.5 * ppp).round() / ppp).max(line);
    let dragged_tab = drag.as_ref().and_then(|d| if let Held::Tab(k) = d.held { Some((k, d.off)) } else { None });
    for (k, t) in shown.tabs.iter().enumerate() {
        let off = dragged_tab == Some((k, true));
        let color = if off { faint.gamma_multiply(0.5) } else { c(p.panel_text) };
        let x = snap(x_of(t.x));
        let y = snap(rect.bottom() - 1.0);
        let up = Rect::from_min_max(pos2(x - thick / 2.0, base + line), pos2(x + thick / 2.0, y));
        let up = style::snap(&ctx, up);
        painter.rect_filled(up, 0.0, color);
        let (l, r) = match t.align {
            TabAlign::Left => (x, x + 5.0),
            TabAlign::Right => (x - 5.0, x),
            TabAlign::Center | TabAlign::Decimal => (x - 3.5, x + 3.5),
        };
        painter.rect_filled(style::snap(&ctx, Rect::from_min_max(pos2(l, y - thick), pos2(r, y))), 0.0, color);
        if t.align == TabAlign::Decimal {
            painter.rect_filled(style::snap_centred(&ctx, pos2(x + 2.5, y - 3.0), vec2(thick, thick)), 0.0, color);
        }
    }
    marker_shapes(&painter);

    // For agents: each marker, as a slider at its place.
    let describe = |name: String, value: f32, x: f32, top: bool| {
        let r = if top { Rect::from_min_max(pos2(x - half, rect.top()), pos2(x + half, strip.center().y)) } else { Rect::from_min_max(pos2(x - half, strip.center().y), pos2(x + half, rect.bottom())) };
        let nid = id.with(("marker", name.as_str()));
        ctx.accesskit_node_builder(nid, |n| {
            n.set_role(accesskit::Role::Slider);
            n.set_label(name.as_str());
            n.set_value(length(value, metric));
            n.set_numeric_value(value as f64);
            n.set_bounds(accesskit::Rect { x0: r.min.x as f64, y0: r.min.y as f64, x1: r.max.x as f64, y1: r.max.y as f64 });
        });
    };
    describe("First Line Indent".into(), ind.left + ind.first, first_x, true);
    describe("Left Indent".into(), ind.left, left_x, false);
    describe("Right Indent".into(), ind.right, right_x, false);
    for t in &shown.tabs {
        describe(format!("Tab {}, {}", length(t.x, metric), align_name(t.align)), t.x, x_of(t.x), false);
    }

    Ruled { response, changed, guide }
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
