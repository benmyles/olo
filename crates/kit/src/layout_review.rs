//! Layout review: what a frame looks like it got wrong, read from what it
//! painted. Where the layout lint ([`crate::layout_lint`]) proves content
//! fits, the review reads one frame's shapes (text with its baseline,
//! filled bands, one-point lines, icons) at the scale it was drawn at and
//! finds what tends to look off.
//!
//! Errors, which tests fail on:
//! - an icon between device pixels, or not a whole number of them: it
//!   blurs;
//! - one-point lines of different weights (each edge rounded to the
//!   nearest device pixel on its own: at 1.5× some come out 1 px, some
//!   2 px);
//! - an outline whose width isn't a whole number of device pixels (a
//!   1-point stroke at 1.5×): it blurs;
//! - a seam: a line and what it borders, meant to touch (less than a
//!   point apart), a device pixel apart (an inner bevel placed a point in
//!   from a line at 1.5×, where a point is a pixel and a half: inset by
//!   [`crate::style::hairline`] from a rect on device pixels instead).
//!
//! For review, each a place to look at closely (most are mistakes, a few
//! are meant):
//! - bands stacked on each other, or side by side, almost but not quite
//!   matching (a 21-point menu bar over a 22-point header);
//! - text or icons almost but not quite level with their neighbour in
//!   the next column;
//! - left edges almost but not quite lined up down a column;
//! - text or an icon off-centre in the band it sits in;
//! - rows down a column spaced unevenly.
//!
//! [`review`] runs on any frame's shapes: `layout_lint::review` over a
//! view in a test, and every kit app's windows on request
//! (`{"review":true}` on its socket: `olo-screen review`, `tsession
//! review`). Text on a grid of its own ([`own_grid`]: a terminal's
//! screen) isn't held level with what is beside it; what is centred in
//! an area nothing fills ([`band`]: a tab hanging in a strip) is
//! measured against that. See "Layout review" in docs/HACKING.md.
//!
//! The checks themselves are `olo_kit_base::layout_review`'s, shared
//! with the GPUI kit (`olo_ui::review`); this collects egui's shapes for
//! them.


use egui::{Context, Id, Pos2, Rect, Shape, TextureId, Ui, epaint::ClippedShape};
use olo_kit_base::layout_review::{self as checks, Fill, Icon, Parts, Text};

pub use olo_kit_base::review::{Finding, Level};

/// Closer than this, two things count as lined up (a rounding).
const SAME: f32 = 0.5;

/// painted: [`marks`] after the pass.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    /// Areas on a grid of their own ([`own_grid`]).
    pub grids: Vec<Rect>,
    /// Areas what is in them is centred across, though nothing fills
    /// them ([`band`]).
    pub bands: Vec<Rect>,
    /// Each texture's size in pixels: an icon drawn a pixel or two off
    /// its texture's size is resampled.
    pub textures: std::collections::HashMap<TextureId, [usize; 2]>,
}

fn marks_id() -> Id {
    Id::unique("olo-layout-review-marks")
}

fn mark(ui: &Ui, add: impl FnOnce(&mut Marks)) {
    let pass = ui.ctx().cumulative_pass_nr();
    ui.ctx().data_mut(|d| {
        let (at, marks) = d.get_temp_mut_or_default::<(u64, Marks)>(marks_id());
        if *at != pass {
            *at = pass;
            *marks = Marks::default();
        }
        add(marks);
    });
}

/// The text in `rect` is laid out on a grid of its own, not the
/// window's (a terminal's screen: its lines are a cell apart at any text
/// size; a card in a masonry, staggered in its column): the review
/// doesn't hold it level with what's beside it, which it can only be at
/// some sizes. Call it each frame.
pub fn own_grid(ui: &Ui, rect: Rect) {
    mark(ui, |m| m.grids.push(rect));
}

/// What is in `rect` is centred across it, though nothing is painted to
/// show it (a tab hanging in a strip, its top below the strip's): the
/// review measures that centring against `rect`, not the fill around it.
/// Call it each frame.
pub fn band(ui: &Ui, rect: Rect) {
    mark(ui, |m| m.bands.push(rect));
}

/// What the pass just run marked with [`own_grid`] and [`band`].
pub fn marks(ctx: &Context) -> Marks {
    let pass = ctx.cumulative_pass_nr();
    let mut marks = ctx.data(|d| d.get_temp::<(u64, Marks)>(marks_id())).filter(|(at, _)| at + 1 == pass).map(|(_, m)| m).unwrap_or_default();
    marks.textures = ctx.tex_manager().read().allocated().map(|(id, meta)| (*id, meta.size)).collect();
    marks
}


fn collect(shape: &Shape, clip: Rect, textures: &std::collections::HashMap<TextureId, [usize; 2]>, out: &mut Parts) {
    match shape {
        Shape::Vec(shapes) => {
            for s in shapes {
                collect(s, clip, textures, out);
            }
        }
        Shape::Rect(r) => {
            let seen = r.rect.intersect(clip);
            if !seen.is_positive() {
                return;
            }
            if r.fill.a() > 0 {
                let cut = [r.rect.min.y < seen.min.y, r.rect.max.y > seen.max.y, r.rect.min.x < seen.min.x, r.rect.max.x > seen.max.x];
                out.fills.push(Fill { rect: kb(seen), cut });
                out.covers.push(kb(seen));
                if seen == r.rect {
                    out.whole_covers.push(kb(seen));
                }
            }
            if r.stroke.width > 0.0 && r.stroke.color.a() > 0 {
                out.strokes.push((r.stroke.width, kb(seen)));
            }
        }
        Shape::LineSegment { points: [a, b], stroke } => {
            let straight = (a.x - b.x).abs() < 0.01 || (a.y - b.y).abs() < 0.01;
            let seen = Rect::from_two_pos(*a, *b).intersect(clip.expand(stroke.width));
            if straight && stroke.width > 0.0 && stroke.color.a() > 0 && (seen.width() > 0.0 || seen.height() > 0.0) {
                out.strokes.push((stroke.width, kb(Rect::from_two_pos(*a, *b))));
            }
        }
        Shape::Text(t) => {
            let label: String = t.galley.job.text.split('\n').next().unwrap_or("").trim().chars().take(28).collect();
            let Some(row) = t.galley.rows.first() else { return };
            if row.row.glyphs.is_empty() {
                return;
            }
            let rect = t.galley.rect.translate(t.pos.to_vec2());
            // Text a clip cuts (in a scroll view) is left alone.
            if label.is_empty() || t.angle != 0.0 || !clip.expand(SAME).contains_rect(rect) {
                return;
            }
            let size = t.galley.job.sections.first().map_or(0.0, |s| s.format.font_id.size);
            let baseline = t.pos.y + crate::style::baseline_offset(&t.galley);
            let middle = baseline - size * crate::style::CAP_HEIGHT / 2.0;
            out.texts.push(Text { rect: kb(rect), baseline, middle, left: t.pos.x + row.pos.x, label, size });
        }
        Shape::Mesh(m) if m.texture_id != TextureId::default() && !m.vertices.is_empty() => {
            let (mut r, mut uv) = (Rect::NOTHING, Rect::NOTHING);
            for v in &m.vertices {
                r.extend_with(v.pos);
                uv.extend_with(v.uv);
            }
            // Pictures (thumbnails, previews) are scaled anyway; icons
            // are drawn at their size.
            if r.width() <= 128.0 && r.height() <= 128.0 && r.intersects(clip) {
                let whole = uv.min == Pos2::ZERO && uv.max == egui::pos2(1.0, 1.0);
                out.icons.push(Icon { rect: kb(r), texture: textures.get(&m.texture_id).copied(), whole });
            }
        }
        Shape::Mesh(m) if m.texture_id == TextureId::default() && !m.vertices.is_empty() => {
            // A gradient (`style::vgradient`): it covers its bounds.
            let mut r = Rect::NOTHING;
            for v in &m.vertices {
                r.extend_with(v.pos);
            }
            if m.vertices.iter().all(|v| v.color.a() == 255) {
                out.covers.push(kb(r.intersect(clip)));
                if clip.contains_rect(r) {
                    out.whole_covers.push(kb(r));
                }
            }
        }
        _ => {}
    }
}

/// An egui rectangle as the checks take it.
fn kb(r: Rect) -> checks::Rect {
    checks::Rect::from_min_max(checks::pos2(r.min.x, r.min.y), checks::pos2(r.max.x, r.max.y))
}

/// What looks wrong in a frame of `shapes` drawn at `ppp` device pixels
/// a point in a window of `size` (logical), errors first; `marks`, what
/// the frame marked ([`marks`]). The checks are
/// `olo_kit_base::layout_review`'s, shared with the GPUI kit.
pub fn review(shapes: &[ClippedShape], size: egui::Vec2, ppp: f32, marks: &Marks) -> Vec<Finding> {
    let mut parts = Parts { grids: marks.grids.iter().copied().map(kb).collect(), bands: marks.bands.iter().copied().map(kb).collect(), ..Default::default() };
    for s in shapes {
        collect(&s.shape, s.clip_rect, &marks.textures, &mut parts);
    }
    checks::review(&parts, size.x, size.y, ppp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style;
    use egui::{Align, Color32, Context, Painter, ViewportId, pos2, vec2};

    /// What the review finds in a frame painted by `paint` at `scale`.
    fn frame(scale: f32, paint: impl Fn(&Painter)) -> Vec<Finding> {
        frame_ui(scale, |ui| paint(ui.painter()))
    }

    /// The same of a frame `show` makes, marks and all.
    fn frame_ui(scale: f32, show: impl Fn(&mut Ui)) -> Vec<Finding> {
        let ctx = Context::default();
        ctx.set_fonts(style::all_font_definitions());
        let size = vec2(400.0, 300.0);
        let mut raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
        raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(raw.clone(), &show);
            shapes = out.shapes.clone();
            out.drop_without_applying_deltas();
        }
        let mut marks = marks(&ctx);
        marks.textures = [(TextureId::User(7), [23, 23])].into();
        review(&shapes, size, scale, &marks)
    }

    fn kinds(found: &[Finding]) -> Vec<&str> {
        found.iter().map(|f| f.kind.as_str()).collect()
    }

    fn text(p: &Painter, x: f32, cy: f32, s: &str) {
        style::label(p, x, cy, Align::Min, s, style::plain_font(), Color32::BLACK);
    }

    #[test]
    fn bands_of_nearly_one_height() {
        let bars = |second: f32| {
            move |p: &Painter| {
                p.rect_filled(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 21.0)), 0.0, Color32::GRAY);
                p.rect_filled(Rect::from_min_size(pos2(0.0, 21.0), vec2(400.0, second)), 0.0, Color32::DARK_GRAY);
            }
        };
        assert!(kinds(&frame(2.0, bars(22.0))).contains(&"near-height"));
        assert!(!kinds(&frame(2.0, bars(21.0))).contains(&"near-height"));
        assert!(!kinds(&frame(2.0, bars(30.0))).contains(&"near-height"));
    }

    #[test]
    fn text_almost_level_and_almost_lined_up() {
        let found = frame(2.0, |p| {
            text(p, 10.0, 20.0, "Left");
            text(p, 120.0, 23.0, "Right");
        });
        assert!(kinds(&found).contains(&"near-level"), "{found:?}");
        let found = frame(2.0, |p| {
            text(p, 10.0, 20.0, "Left");
            text(p, 120.0, 20.0, "Right");
        });
        assert!(!kinds(&found).contains(&"near-level"), "{found:?}");
        let found = frame(2.0, |p| {
            text(p, 11.0, 20.0, "Terminal");
            text(p, 14.0, 40.0, "Tabs");
        });
        assert!(kinds(&found).contains(&"near-edge"), "{found:?}");
        let found = frame(2.0, |p| {
            text(p, 11.0, 20.0, "Terminal");
            text(p, 23.0, 40.0, "Indented");
        });
        assert!(!kinds(&found).contains(&"near-edge"), "{found:?}");
    }

    #[test]
    fn text_off_centre_in_its_band() {
        let band = |cy: f32| {
            move |p: &Painter| {
                p.rect_filled(Rect::from_min_size(pos2(0.0, 10.0), vec2(400.0, 22.0)), 0.0, Color32::GRAY);
                text(p, 8.0, cy, "Header");
            }
        };
        assert!(!kinds(&frame(2.0, band(21.0))).contains(&"off-centre"));
        assert!(kinds(&frame(2.0, band(24.0))).contains(&"off-centre"));
        // A band a clip cuts (a card at the foot of a list) is centred on
        // in the whole of it, not in what shows.
        let cut = frame(2.0, |p| {
            let p = p.with_clip_rect(Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 36.0)));
            p.rect_filled(Rect::from_min_size(pos2(0.0, 10.0), vec2(400.0, 30.0)), 0.0, Color32::GRAY);
            text(&p, 8.0, 25.0, "Header");
        });
        assert!(!kinds(&cut).contains(&"off-centre"), "{cut:?}");
    }

    /// An icon in a band is centred in it, unless it is in a line of
    /// text (an emoji in a message), which it goes with.
    #[test]
    fn icons_in_a_line_go_with_the_line() {
        let icon = |p: &Painter, x: f32, cy: f32| {
            let r = style::snap_centred(p.ctx(), pos2(x + 5.75, cy), vec2(11.5, 11.5));
            p.image(TextureId::User(7), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        };
        let bubble = |p: &Painter| p.rect_filled(Rect::from_min_size(pos2(0.0, 10.0), vec2(200.0, 34.0)), 0.0, Color32::GRAY);
        // Alone, low in its band.
        let alone = frame(2.0, |p| {
            bubble(p);
            icon(p, 120.0, 35.0);
        });
        assert!(kinds(&alone).contains(&"off-centre"), "{alone:?}");
        // The second of two lines, after its text: level with it.
        let line = frame(2.0, |p| {
            bubble(p);
            text(p, 8.0, 19.0, "Theo");
            let font = style::plain_font();
            let (r, baseline) = style::label_with_baseline(p, 8.0, 35.0, Align::Min, "Soft 3D it is", font.clone(), Color32::BLACK);
            icon(p, r.right() + 4.0, baseline - font.size * style::CAP_HEIGHT / 2.0);
        });
        assert!(!kinds(&line).contains(&"off-centre"), "{line:?}");
    }

    /// Text set from its right (sizes in a column) starts on a device
    /// pixel: its right edges a pixel apart at 1× aren't a near miss.
    #[test]
    fn right_aligned_text_lines_up_on_the_right() {
        let right = |p: &Painter, cy: f32, s: &str| {
            let g = p.layout_no_wrap(s.to_owned(), style::plain_font(), Color32::BLACK);
            let x = 200.0 - g.size().x;
            style::label(p, x, cy, Align::Min, s, style::plain_font(), Color32::BLACK);
        };
        for words in [["1.3 MB", "4.0 KB"], ["12.4 GB", "980 KB"], ["1.2 MB", "4.0 KB"]] {
            let found = frame(1.0, |p| {
                right(p, 20.0, words[0]);
                right(p, 40.0, words[1]);
            });
            assert!(!kinds(&found).contains(&"near-edge"), "{words:?}: {found:?}");
        }
    }

    #[test]
    fn rows_spaced_unevenly() {
        let rows = |ys: &'static [f32]| {
            move |p: &Painter| {
                for (i, y) in ys.iter().enumerate() {
                    text(p, 10.0, *y, &format!("Row {i}"));
                }
            }
        };
        assert!(!kinds(&frame(2.0, rows(&[10.0, 30.0, 50.0, 70.0]))).contains(&"uneven-rows"));
        assert!(kinds(&frame(2.0, rows(&[10.0, 30.0, 52.0, 72.0]))).contains(&"uneven-rows"));
        // No two gaps alike: no rhythm to break.
        assert!(!kinds(&frame(2.0, rows(&[10.0, 29.0, 88.0, 151.0, 217.0]))).contains(&"uneven-rows"));
        // Runs of rows far apart, one a device pixel off: the long gaps
        // don't make the step 20.5.
        assert!(!kinds(&frame(2.0, rows(&[10.0, 30.0, 70.0, 130.0, 190.0, 210.5, 230.0, 250.0, 270.0]))).contains(&"uneven-rows"));
        // A list's heading over its rows scrolled a little: the rows are
        // on a grid of their own.
        let scrolled = |ui: &mut Ui| {
            text(ui.painter(), 10.0, 10.0, "Heading");
            own_grid(ui, Rect::from_min_max(pos2(0.0, 20.0), pos2(400.0, 300.0)));
            rows(&[27.0, 47.0, 67.0, 87.0])(ui.painter());
        };
        assert!(!kinds(&frame_ui(2.0, scrolled)).contains(&"uneven-rows"), "{:?}", frame_ui(2.0, scrolled));
    }

    #[test]
    fn seams_between_nested_lines() {
        // A frame, then a bevel a point inside it: at 1.5× a point is a
        // pixel and a half, so a one-pixel frame leaves a crack.
        let nested = |p: &Painter| {
            let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(60.0, 20.0));
            style::frame_rect(p, r, Color32::BLACK);
            style::bevel(p, r.shrink(1.0), Color32::WHITE, Color32::GRAY);
        };
        assert!(!kinds(&frame(2.0, nested)).contains(&"seam"));
        assert!(kinds(&frame(1.5, nested)).contains(&"seam"), "{:?}", frame(1.5, nested));
        // The sunken well's two bevels are placed on device pixels.
        let well = |p: &Painter| style::sunken(p, Rect::from_min_size(pos2(10.0, 10.0), vec2(60.0, 20.0)), &olo_theme::Palette::light(), false);
        assert!(!kinds(&frame(1.5, well)).contains(&"seam"), "{:?}", frame(1.5, well));
    }

    /// The kit's sizes and gaps are on the 2-point grid: whole device
    /// pixels at 1×, 1.5× and 2×, so what is laid out with them is too.
    /// (Lines are whole device pixels by `style::line_pixels`; radii and
    /// type sizes aren't places.)
    #[test]
    fn kit_sizes_on_the_two_point_grid() {
        use crate::{menu, metrics as m, widgets as w};
        let sizes = [
            ("metrics::SPACING", m::SPACING),
            ("metrics::DESKBAR_WIDTH", m::DESKBAR_WIDTH),
            ("metrics::TIGHT", m::TIGHT),
            ("metrics::CARD_INSET", m::CARD_INSET),
            ("metrics::SELECTION_PAD", m::SELECTION_PAD),
            ("metrics::WINDOW_INSET", m::WINDOW_INSET),
            ("metrics::BOX_INSET", m::BOX_INSET),
            ("metrics::LABEL_GAP", m::LABEL_GAP),
            ("metrics::MARK_GAP", m::MARK_GAP),
            ("metrics::MARK_SIZE", m::MARK_SIZE),
            ("metrics::CONTROL_HEIGHT", m::CONTROL_HEIGHT),
            ("metrics::MARK_ROW", m::MARK_ROW),
            ("metrics::LABEL_ROW", m::LABEL_ROW),
            ("metrics::CONTROL_PAD", m::CONTROL_PAD),
            ("metrics::BUTTON_MIN_WIDTH", m::BUTTON_MIN_WIDTH),
            ("metrics::TEXT_INSET", m::TEXT_INSET),
            ("metrics::LIST_ROW", m::LIST_ROW),
            ("metrics::CELL_INSET", m::CELL_INSET),
            ("metrics::TOOLBAR_HEIGHT", m::TOOLBAR_HEIGHT),
            ("menu::BAR_HEIGHT", menu::BAR_HEIGHT),
            ("menu::ITEM_HEIGHT", menu::ITEM_HEIGHT),
            ("widgets::SCROLL_BAR", w::SCROLL_BAR),
            ("widgets::SMALL_BUTTON_HEIGHT", w::SMALL_BUTTON_HEIGHT),
            ("widgets::HEADER_HEIGHT", w::HEADER_HEIGHT),
            ("widgets::TAB_HEIGHT", w::TAB_HEIGHT),
            ("widgets::STRIP_TAB_MAX", w::STRIP_TAB_MAX),
            ("widgets::STRIP_HEIGHT", w::STRIP_HEIGHT),
            ("widgets::STRIP_ROW", w::STRIP_ROW),
            ("widgets::STRIP_SIDE_TAB", w::STRIP_SIDE_TAB),
            ("widgets::STRIP_SIDE_HEAD", w::STRIP_SIDE_HEAD),
            ("widgets::STRIP_SIDE_WIDTH", w::STRIP_SIDE_WIDTH),
            ("widgets::STRIP_SIDE_MIN", w::STRIP_SIDE_MIN),
            ("widgets::STRIP_SIDE_MAX", w::STRIP_SIDE_MAX),
            ("widgets::ALERT_TEXT_X", w::ALERT_TEXT_X),
        ];
        let off: Vec<String> = sizes.iter().filter(|(_, v)| v % 2.0 != 0.0).map(|(n, v)| format!("{n} = {v}")).collect();
        assert!(off.is_empty(), "off the 2-point grid: {}", off.join(", "));
    }

    #[test]
    fn fills_between_device_pixels() {
        // A 21-point bar and what follows it: at 1.5× its bottom is 31.5
        // device pixels.
        let bars = |h: f32| {
            move |p: &Painter| {
                p.rect_filled(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, h)), 0.0, Color32::GRAY);
                p.rect_filled(Rect::from_min_size(pos2(0.0, h), vec2(400.0, 26.0)), 0.0, Color32::DARK_GRAY);
            }
        };
        assert!(kinds(&frame(1.5, bars(21.0))).contains(&"off-pixel"));
        assert!(!kinds(&frame(1.0, bars(21.0))).contains(&"off-pixel"));
        assert!(!kinds(&frame(2.0, bars(21.0))).contains(&"off-pixel"));
        assert!(!kinds(&frame(1.5, bars(26.0))).contains(&"off-pixel"));
        // At 1.25× the grid itself is between pixels: not flagged.
        assert!(!kinds(&frame(1.25, bars(26.0))).contains(&"off-pixel"));
        // Where a clip cuts it, the edge is the clip's.
        let clipped = |p: &Painter| {
            p.with_clip_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 30.0))).rect_filled(Rect::from_min_size(pos2(0.0, 10.0), vec2(400.0, 20.5)), 0.0, Color32::GRAY);
        };
        assert!(!kinds(&frame(1.5, clipped)).contains(&"off-pixel"), "{:?}", frame(1.5, clipped));
    }

    /// Every kit widget, alone in a window, light and dark at 1×, 1.5×
    /// and 2×: the review finds nothing at all in any of them, not even
    /// something to look at.
    #[test]
    fn kit_widgets_pass_the_review() {
        use crate::{
            layout_lint,
            menu::{self, Menu},
            widgets::*,
        };
        let size = vec2(320.0, 220.0);
        let icon = |p: &Painter, r: Rect| {
            p.rect_filled(r, 2.0, Color32::from_rgb(90, 120, 200));
        };
        let tabs = [StripTab { label: "One", icon: Some(&icon), status: None, badge: None, group: None }, StripTab { label: "Two", icon: Some(&icon), status: None, badge: Some(&icon), group: None }];
        let menus = [Menu::new("File").item("New", "new", None), Menu::new("Edit").item("Copy", "copy", None)];
        // Open, the row after a separator highlighted.
        let file = Menu::new("File")
            .item("New", "new", Some(egui::Key::N))
            .item("Open…", "open", Some(egui::Key::O))
            .separator()
            .item("Word Wrap", "wrap", None)
            .checked(true)
            .submenu(Menu::new("Recent").item("notes.txt", "recent", None))
            .separator()
            .item("Quit", "quit", Some(egui::Key::Q));
        let places = Menu::new("Go").item("Home", "home", None).icon("folder").separator().item("Trash", "trash", None).icon("trash");
        let popup = |ui: &mut egui::Ui, m: &Menu| {
            let r = Rect::from_min_size(ui.max_rect().min, menu::popup_size(ui.ctx(), m));
            let mut state = menu::PopupState { selected: Some(3.min(m.items.len() - 1)), ..Default::default() };
            menu::popup_ui(&mut ui.new_child(egui::UiBuilder::new().max_rect(r)), m, &mut state);
        };
        type Demo<'a> = (&'a str, Box<dyn Fn(&mut egui::Ui) + 'a>);
        let widgets: Vec<Demo> = vec![
            ("Button", Box::new(|ui| {
                ui.add(Button::new("Default").default(true));
                ui.add(Button::new("Cancel"));
            })),
            ("CheckBox", Box::new(|ui| {
                ui.add(CheckBox::new(&mut true, "On"));
                ui.add(CheckBox::new(&mut false, "Off"));
            })),
            ("RadioButton", Box::new(|ui| {
                ui.add(RadioButton::new(true, "On"));
                ui.add(RadioButton::new(false, "Off"));
            })),
            ("TextControl", Box::new(|ui| {
                ui.add(TextControl::new(&mut "Text".to_owned()).label("Name:"));
                ui.add(TextControl::new(&mut String::new()).label("Find:").hint("City or region"));
            })),
            ("TextView", Box::new(|ui| {
                TextView::new("t", &mut "Some text\nover lines".to_owned()).show(ui);
            })),
            ("Slider", Box::new(|ui| {
                ui.add(Slider::new(&mut 0.5, 0.0..=1.0).label("Speed").limits("Slow", "Fast"));
            })),
            ("MenuField", Box::new(|ui| {
                ui.add(MenuField::new("m", &mut 0, &["One", "Two"]).label("Pick:"));
            })),
            ("BBox", Box::new(|ui| {
                BBox::new("Box").show(ui, |ui| {
                    ui.add(StringView::new("Inside"));
                });
            })),
            ("StatusBar", Box::new(|ui| {
                ui.add(StatusBar::new(0.4));
            })),
            ("TabView", Box::new(|ui| {
                TabView::new(&mut 0, &["One", "Two"]).show(ui, |ui, _| {
                    ui.add(StringView::new("Inside"));
                });
            })),
            ("ScrollView", Box::new(|ui| {
                ScrollView::new("s").show(ui, |ui| {
                    for i in 0..30 {
                        ui.add(StringView::new(&format!("Line {i}")));
                    }
                });
            })),
            ("separator", Box::new(|ui| {
                separator(ui);
            })),
            ("ColorControl", Box::new(|ui| {
                ui.add(ColorControl::new("c", &mut [200, 120, 40]));
            })),
            ("menu bar", Box::new(|ui| {
                menu::menu_bar(ui, &menus, &mut Default::default());
            })),
            ("popup menu", Box::new(|ui| popup(ui, &file))),
            ("popup menu with icons", Box::new(|ui| popup(ui, &places))),
            ("TabStrip", Box::new(|ui| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(320.0, STRIP_HEIGHT));
                TabStrip::new("ts", &tabs, 0).show(ui, r);
            })),
            ("TabStrip, side", Box::new(|ui| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(STRIP_SIDE_WIDTH, 220.0));
                TabStrip::new("ts", &tabs, 0).side().show(ui, r);
            })),
            ("TabStrip, side, closed", Box::new(|ui| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(STRIP_SIDE_WIDTH, 220.0));
                TabStrip::new("ts", &tabs, 1).side().closed().show(ui, r);
            })),
        ];
        let mut found = Vec::new();
        for (name, w) in &widgets {
            for line in layout_lint::review(size, |ui| w(ui)) {
                found.push(format!("{name}: {line}"));
            }
        }
        assert!(found.is_empty(), "kit widgets the review finds something in:\n  - {}", found.join("\n  - "));
    }

    #[test]
    fn lines_and_icons_on_device_pixels() {
        let lines = |p: &Painter| {
            style::hline(p, 0.0, 200.0, 10.0, Color32::BLACK);
            style::hline(p, 0.0, 200.0, 11.0 + 2.0, Color32::BLACK);
        };
        assert!(!kinds(&frame(2.0, lines)).contains(&"line-weights"));
        assert!(!kinds(&frame(1.5, lines)).contains(&"line-weights"), "{:?}", frame(1.5, lines));
        let icon = |x: f32| {
            move |p: &Painter| {
                p.image(TextureId::User(1), Rect::from_min_size(pos2(x, 10.0), vec2(16.0, 16.0)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        };
        assert!(!kinds(&frame(2.0, icon(10.5))).contains(&"blurry-icon"));
        assert!(kinds(&frame(1.5, icon(10.5))).contains(&"blurry-icon"));
        assert!(!kinds(&frame(1.5, icon(10.0 + 2.0 / 3.0))).contains(&"blurry-icon"));
        // On whole pixels, but 24 device px from a 23 px texture (a 15.5
        // point icon whose corners were snapped one by one): resampled.
        let from = |w: f32| {
            move |p: &Painter| {
                p.image(TextureId::User(7), Rect::from_min_size(pos2(10.0, 10.0), vec2(w, w)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        };
        assert!(kinds(&frame(1.5, from(16.0))).contains(&"blurry-icon"));
        assert!(!kinds(&frame(1.5, from(23.0 / 1.5))).contains(&"blurry-icon"), "{:?}", frame(1.5, from(23.0 / 1.5)));
        assert!(!kinds(&frame(1.0, from(64.0))).contains(&"blurry-icon"));
        let outline = |p: &Painter| {
            p.rect_stroke(Rect::from_min_size(pos2(10.0, 10.0), vec2(40.0, 20.0)), 0.0, egui::Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
        };
        assert!(!kinds(&frame(2.0, outline)).contains(&"blurry-outline"));
        assert!(kinds(&frame(1.5, outline)).contains(&"blurry-outline"));
    }
}
