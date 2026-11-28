//! What folder windows and the desktop share: an item's icon and label
//! (laid out, hit, drawn selected or not), dragging items out, the rules
//! for what dragged things a folder takes, spring-loaded folders and
//! editing a name in place.

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, Cx, DragAction, DropAccept, DropOffer, Element, Font, FontStyle, FontWeight, Hsla, LineFragment, Painter, Pixels, Point, SharedString, View, WindowCx, agent, icons, metrics as m,
    point, px, size,
    style::{self, Align},
    widgets::{TextField, text_field},
};
use olo_sync::Mutex;
use olo_theme::Palette;

use crate::fs::{self, Entry};

/// Icon view: 48-point icons in 96 × 100 cells: the icon, a gap, two
/// label lines and their highlight, and a spacing's room under it to the
/// next row's icon at every scale (`cell_room_under_a_label`). At 84 a
/// two-line label's highlight ran 2 to 3.3 points into the icon under it,
/// at every scale, on the desktop (a volume's long hyphenated name over
/// Home) and in folders (QA, 2026-11-28). A whole number of device pixels
/// at 1.25× and 1.75× too, so every row starts on one.
pub const ICON: f32 = 48.0;
pub const ICON_CELL: (f32, f32) = (96.0, 100.0);
/// Lines of a label in the icon view, selected or not: all the cell has
/// room for above the next row's icon. (Four when selected ran 18 points
/// into the icon under it, and its press with it; the whole name is in
/// the tip a name cut short has.)
const LABEL_ROWS: usize = 2;
/// How far above the icon box's foot a thumbnail stands: where an
/// icon's art stands, its shadow under it.
const THUMB_FOOT: f32 = 3.0;
/// Mini-icon view: 16-point icons with the label beside them.
pub const MINI: f32 = 16.0;
pub const MINI_CELL: (f32, f32) = (184.0, m::LIST_ROW);
/// Room between a selected label's text and its highlight (the field
/// editing a name has the same, so they line up).
const LABEL_PAD: (f32, f32) = (4.0, 4.0);
/// A selected icon is darkened, as on BeOS.
fn selected_tint() -> Hsla {
    olo_kit::hsla(0.0, 0.0, 185.0 / 255.0, 1.0)
}

/// What `e` is to make small, if its icon in the icon view is itself: a
/// picture, a video (a frame of it). (Ben, 2026-11-17: "in tracker's
/// icon view the icons should show preview thumb of images/videos".)
pub fn small_of(e: &Entry) -> Option<crate::thumbs::Of> {
    match e.kind {
        olo_kit_base::files::Kind::Image => Some(crate::thumbs::Of::Picture),
        olo_kit_base::files::Kind::Video => Some(crate::thumbs::Of::Video),
        _ => None,
    }
}

/// `e`'s own picture for the icon view, at the scale `g` draws at: none
/// while it's made (its kind's icon shows; drawn again when it has
/// been), or for anything else.
pub fn small(g: &Painter, e: &Entry) -> Option<crate::thumbs::Thumb> {
    if fs::is_remote(&e.path) {
        return None;
    }
    // (40 points at most: the icon box less room for its frame and
    // shadow, as an icon's art leaves.)
    let thumb = crate::thumbs::of(&e.path, small_of(e)?, (crate::thumbs::THUMB * g.scale_factor()).round() as u32, g.scale_factor())?;
    // Too thin to make out at this size, as drawn: its kind's icon instead.
    crate::thumbs::shows(thumb.size, thumb.whole, g.scale_factor()).then_some(thumb)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Icons,
    Mini,
}

/// The font a name is set in: italic for a link, as Tracker's.
pub fn name_font(link: bool) -> Font {
    let mut f = style::ui_font(FontWeight::NORMAL);
    if link {
        f.style = FontStyle::Italic;
    }
    f
}

/// One line's height of names.
fn row_height() -> f32 {
    style::PLAIN_SIZE * style::LINE_HEIGHT
}

/// An icon-view label's lines under an icon whose bottom is
/// `icon_bottom`, at `scale`: the first line's baseline and the step to
/// the next, each on the device pixel grid.
fn label_rows(icon_bottom: f32, scale: f32) -> (f32, f32) {
    let snap = |v: f32| (v * scale).round() / scale;
    let top = icon_bottom + m::TIGHT;
    (snap(top + row_height() / 2.0 + style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0), snap(row_height()))
}

/// Where an item's icon and label land in its cell.
#[derive(Clone, Debug)]
pub struct ItemLayout {
    pub icon: Bounds<Pixels>,
    /// The label's lines and their widths.
    lines: Rc<[(SharedString, Pixels)]>,
    /// The lines' centre (icons) or left (mini icons).
    x: Pixels,
    centred: bool,
    /// The first line's baseline, and from one to the next.
    baseline: Pixels,
    step: Pixels,
    pub font: Font,
    /// Its whole name (for agents).
    pub name: SharedString,
}

impl ItemLayout {
    /// What a click takes as the item: its icon and its label.
    pub fn hit(&self) -> Bounds<Pixels> {
        self.icon.union(&self.highlight())
    }

    /// The label's highlight when selected.
    pub fn highlight(&self) -> Bounds<Pixels> {
        let w = self.lines.iter().map(|l| l.1).fold(px(0.0), Pixels::max);
        let left = if self.centred { self.x - w / 2.0 } else { self.x };
        let rows = self.lines.len().max(1) - 1;
        let top = self.baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT + LABEL_PAD.1);
        let bottom = self.baseline + self.step * rows as f32 + px(LABEL_PAD.1);
        Bounds::from_corners(point(left - px(LABEL_PAD.0), top), point(left + w + px(LABEL_PAD.0), bottom))
    }

    /// Where a field editing the name goes: over the first line, as wide
    /// as `width` of text (and room for the caret), anchored as the
    /// label is.
    pub fn field(&self, width: Pixels) -> Bounds<Pixels> {
        let left = if self.centred { self.x - width / 2.0 } else { self.x };
        let top = self.baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT + LABEL_PAD.1);
        let bottom = self.baseline + px(LABEL_PAD.1);
        Bounds::from_corners(point(left - px(LABEL_PAD.0), top), point(left + width + px(LABEL_PAD.0 + 2.0), bottom))
    }
}

/// A label as laid out: its lines (with their widths) and its whole
/// name.
type Label = (Rc<[(SharedString, Pixels)]>, SharedString);

/// What decides how a label is laid out besides its text and font.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LabelShape {
    width: u32,
    /// Lines at most; 0 for one line cut short, as the mini-icon view's.
    rows: u8,
    scale: u32,
}

/// Labels by their text and font (hashed) and shape, each with the
/// text and font it was laid out for.
type LabelCache = HashMap<(u64, LabelShape), (SharedString, Font, Label)>;

thread_local! {
    /// Labels laid out, by their text and font (hashed) and shape:
    /// scrolling and band-selecting lay the same items out every frame,
    /// and wrapping a name shapes it again.
    static LABELS: RefCell<LabelCache> = RefCell::default();
}

/// `text` in `font`, at most `rows` lines `width` wide (0: one line cut
/// short), from the cache if laid out so before.
fn label(text: &str, font: &Font, width: Pixels, rows: usize) -> Label {
    let shape = LabelShape { width: f32::from(width).to_bits(), rows: rows as u8, scale: olo_kit::scale_factor().to_bits() };
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    font.hash(&mut h);
    let key = (h.finish(), shape);
    let known = LABELS.with_borrow(|c| c.get(&key).filter(|(t, f, _)| t.as_ref() == text && f == font).map(|(_, _, l)| l.clone()));
    if let Some(l) = known {
        return l;
    }
    let lines: Rc<[(SharedString, Pixels)]> = if rows == 0 {
        let fitted = style::fit_with(&olo_kit::thread_text_system(), text.to_owned().into(), style::PLAIN_SIZE, font, width);
        let w = style::text_width(fitted.clone(), style::PLAIN_SIZE, font.clone());
        Rc::from([(fitted, w)])
    } else {
        wrap(text, font, width, rows).into()
    };
    let name = SharedString::from(text.to_owned());
    let l = (lines, name.clone());
    LABELS.with_borrow_mut(|c| {
        // (A few folders' worth; a folder of thousands scrolled through
        // starts it again.)
        if c.len() >= 16_384 {
            c.clear();
        }
        c.insert(key, (name, font.clone(), l.clone()));
    });
    l
}

/// `text` broken into at most `rows` lines `width` wide, the last cut
/// short with "…" when it doesn't all fit. A name with no space to break
/// at is broken after a "-" or "_" where one is in the line
/// (`at_separator`), not mid-word, and never so that a line starts with
/// its extension: "storage-disk-" over "backup-01", "test_" over
/// "page.html" (they were "storage-disk-ba" over "ckup-01", then
/// "test_page" over ".html", which read as a hidden file: QA and both
/// visual reviews, 2026-11-28).
fn wrap(text: &str, font: &Font, width: Pixels, rows: usize) -> Vec<(SharedString, Pixels)> {
    let ts = olo_kit::thread_text_system();
    let mut wrapper = olo_kit::text_system().line_wrapper(font.clone(), px(style::PLAIN_SIZE));
    let fits = |s: &str| style::text_width(s.trim().to_owned(), style::PLAIN_SIZE, font.clone()) <= width;
    // Each line's start: where the wrapper breaks what's left, moved back
    // to a separator where it broke a word. (One past the last line, to
    // know whether it is cut short.)
    let mut starts = vec![0];
    while starts.len() <= rows {
        let start = starts[starts.len() - 1];
        let rest = &text[start..];
        let Some(b) = wrapper.wrap_line(&[LineFragment::text(rest)], width).next().map(|b| b.ix) else { break };
        // (Whether the line after this one is the last: what's left after
        // the break must fit it, or it's cut short.)
        let cut = at_separator(rest, b, starts.len() + 1 == rows, fits);
        if cut == 0 {
            break;
        }
        starts.push(start + cut);
    }
    drop(wrapper);
    let mut lines = Vec::new();
    for (k, &start) in starts.iter().enumerate() {
        if k + 1 == rows && k + 1 < starts.len() {
            // The rest, cut short.
            lines.push(style::fit_with(&ts, text[start..].trim().to_owned().into(), style::PLAIN_SIZE, font, width));
            break;
        }
        let end = starts.get(k + 1).copied().unwrap_or(text.len());
        lines.push(SharedString::from(text[start..end].trim().to_owned()));
    }
    lines
        .into_iter()
        .map(|l| {
            let w = style::text_width(l.clone(), style::PLAIN_SIZE, font.clone());
            (l, w)
        })
        .collect()
}

/// Where to break `line`, which the wrapper broke at byte `b` (as much as
/// fits): there, where that's at a space. In a word, after a "-" or "_"
/// in what fits (the break itself, where the wrapper broke after one):
/// where the next line is the last (`last`), the latest one after which
/// the rest of the name `fits` that line whole; else, or if none does,
/// the latest one leaving at least half of what fits on this line (a
/// line of a few letters would cut more of the name short than a broken
/// word). Without one, before the extension's "." where the wrapper broke
/// there, and else at `b`, mid-word.
fn at_separator(line: &str, b: usize, last: bool, fits: impl Fn(&str) -> bool) -> usize {
    let (before, after) = (line[..b].chars().next_back(), line[b..].chars().next());
    if before.is_none_or(char::is_whitespace) || after.is_none_or(char::is_whitespace) {
        return b;
    }
    let fit = line[..b].chars().count();
    // After each "-" or "_" in what fits, the last first.
    let cuts = || line[..b].char_indices().rev().filter(|(_, c)| matches!(c, '-' | '_')).map(|(i, c)| i + c.len_utf8());
    if last && let Some(cut) = cuts().find(|&cut| fits(&line[cut..])) {
        return cut;
    }
    if let Some(cut) = cuts().find(|&cut| 2 * line[..cut].chars().count() >= fit) {
        return cut;
    }
    b
}

/// `text` broken into at most `rows` lines `width` wide (the last cut
/// short with "…").
pub fn wrap_lines(text: &str, font: &Font, width: Pixels, rows: usize) -> Vec<SharedString> {
    wrap(text, font, width, rows).into_iter().map(|(l, _)| l).collect()
}

/// `v` on the device pixel grid of the window this looper is working
/// for (as `style::snap_coord`, where there's no painter: a layout
/// worked out as its view's `body` runs, or for a press).
fn snap(v: Pixels) -> Pixels {
    let s = olo_kit::scale_factor();
    px((f32::from(v) * s).round() / s)
}

/// Lays out an item in `cell`: an icon above a centred label of two
/// lines (`LABEL_ROWS`, selected or not), or a small icon with its name
/// beside it. (For the window this looper is working for: its scale; no
/// painter is needed, so a press can ask where a label is.)
pub fn layout_item(cell: Bounds<Pixels>, label: &str, look: Look, link: bool) -> ItemLayout {
    let font = name_font(link);
    match look {
        Look::Icons => {
            let icon = Bounds { origin: point(cell.center().x - px(ICON / 2.0), cell.top() + px(m::TIGHT)), size: size(px(ICON), px(ICON)) };
            let width = cell.size.width - px(2.0 * m::TIGHT);
            let (lines, name) = self::label(label, &font, width, LABEL_ROWS);
            let (baseline, step) = label_rows(f32::from(icon.bottom()), olo_kit::scale_factor());
            ItemLayout { icon, lines, x: snap(cell.center().x), centred: true, baseline: px(baseline), step: px(step), font, name }
        }
        Look::Mini => {
            let icon = Bounds { origin: point(cell.left() + px(m::TIGHT), cell.center().y - px(MINI / 2.0)), size: size(px(MINI), px(MINI)) };
            let x = icon.right() + px(m::MARK_GAP);
            let width = (cell.right() - x - px(m::TIGHT)).max(px(0.0));
            let (lines, name) = self::label(label, &font, width, 0);
            let baseline = snap(cell.center().y + px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0));
            ItemLayout { icon, lines, x, centred: false, baseline, step: px(row_height()), font, name }
        }
    }
}

/// Draws an item. On the desktop the label is in the desktop's text
/// colour over a shadow; elsewhere on the document's ground. With `label`
/// false only the icon is drawn (its name is being edited).
#[allow(clippy::too_many_arguments)]
pub fn paint_item(window: &mut Painter, l: &ItemLayout, icon: &str, small: Option<&crate::thumbs::Thumb>, selected: bool, desktop: bool, label: bool, broken: bool, p: &Palette) {
    agent::record(window, agent::Node { role: "ListItem", name: Some(l.name.clone()), bounds: l.hit(), enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
    if let Some(small) = small {
        // The picture itself (a video's frame), small: at its own size,
        // a texture pixel a device pixel; a tiny one whole times that,
        // each pixel a square of them (`thumbs::TINY`). Framed a point all
        // round, outside it, in an edge seen on light and on dark, the
        // frame's foot on the icons' baseline (the box's foot, as they
        // stand), centred across, a soft shadow falling a point from it;
        // darkened as an icon is when selected. Centred down the box,
        // unframed in the dark, they floated over their names (Opus's and
        // Gemini's looks, 2026-11-28).
        let s = window.scale_factor();
        // (A point, in whole device pixels: the frame, the shadow's fall.)
        let pt = px(s.round().max(1.0) / s);
        let k = small.whole.max(1);
        let (w, h) = ((small.size.0 * k) as f32 / s, (small.size.1 * k) as f32 / s);
        let foot = l.icon.bottom() - px(THUMB_FOOT) - pt;
        let at = style::snap(window, Bounds { origin: point(l.icon.center().x - px(w / 2.0), foot - px(h)), size: size(px(w), px(h)) });
        // (Whole device pixels: its size as it is, its place snapped.)
        let at = Bounds { origin: at.origin, size: size(px((small.size.0 * k) as f32 / s), px((small.size.1 * k) as f32 / s)) };
        let framed = Bounds { origin: point(at.left() - pt, at.top() - pt), size: size(at.size.width + pt * 2.0, at.size.height + pt * 2.0) };
        let shade = Bounds { origin: point(framed.left() + pt, framed.top() + pt), size: framed.size };
        style::fill_rect(window, shade, olo_kit::hsla(0.0, 0.0, 0.0, 0.22));
        if k > 1 {
            let _ = window.paint_image_nearest(at, small.image.clone());
        } else {
            let _ = window.paint_image(at, olo_kit::Corners::default(), small.image.clone(), 0, false);
        }
        if selected && label {
            style::fill_rect(window, at, olo_kit::hsla(0.0, 0.0, 0.0, 0.28));
        }
        let edge = if p.mode == olo_kit::palette::ThemeMode::Dark { olo_kit::hsla(0.0, 0.0, 1.0, 0.38) } else { olo_kit::hsla(0.0, 0.0, 0.0, 0.42) };
        for r in [
            Bounds { origin: framed.origin, size: size(framed.size.width, pt) },
            Bounds { origin: point(framed.left(), at.bottom()), size: size(framed.size.width, pt) },
            Bounds { origin: point(framed.left(), at.top()), size: size(pt, at.size.height) },
            Bounds { origin: point(at.right(), at.top()), size: size(pt, at.size.height) },
        ] {
            style::fill_rect(window, r, edge);
        }
    } else if broken {
        // (Half there, marked: `paint_broken`.)
        icons::paint_faded(window, l.icon, icon, 0.5);
    } else if selected && label {
        icons::paint_tinted(window, l.icon, icon, selected_tint());
    } else {
        icons::paint(window, l.icon, icon);
    }
    if broken {
        paint_broken(window, l.icon);
    }
    if !label {
        return;
    }
    // (The desktop's shadow: a point down and right, in whole device
    // pixels from where the words are, so the two never round apart: a
    // point apiece rounded 1.6 points apart at 1.25×.)
    let shadow = {
        let s = window.scale_factor();
        px(s.round().max(1.0) / s)
    };
    // Where line `k` starts: a centred one as `Align::Center` puts it.
    let left = |window: &Painter, k: usize| if l.centred { style::snap_coord(window, l.x - l.lines[k].1 / 2.0) } else { l.x };
    let line_at = |window: &mut Painter, k: usize, (dx, dy): (Pixels, Pixels), color: Hsla| {
        let (base, at) = (l.baseline + l.step * k as f32, left(window, k));
        style::label_on_baseline(window, at + dx, base + dy, Align::Left, l.lines[k].0.clone(), style::PLAIN_SIZE, l.font.clone(), color);
    };
    let line = |window: &mut Painter, k: usize, d: Pixels, color: Hsla| line_at(window, k, (d, d), color);
    // Centred under the icon, each line in a band of its own, though
    // nothing fills it (halfway to its shadow, which falls `fall`): where
    // a line of a label starts is its length's doing, not a column's to
    // line up with (the review's near-edge).
    let band = |window: &mut Painter, k: usize, fall: Pixels| {
        if !l.centred {
            return;
        }
        let (row, cap) = (row_height(), style::PLAIN_SIZE * style::CAP_HEIGHT);
        let (base, at, half) = (l.baseline + l.step * k as f32, left(window, k), fall / 2.0);
        let b = Bounds { origin: point(at + half - px(m::TIGHT), base + half - px((row + cap) / 2.0)), size: size(l.lines[k].1 + px(2.0 * m::TIGHT), px(row)) };
        olo_kit::review::band(window, b);
    };
    if selected {
        // BeOS inverts a selected icon's label; in dark mode that makes
        // it light on dark-on-light, which reads the same way.
        let h = style::snap(window, l.highlight());
        window.paint_quad(olo_kit::fill(h, style::c(p.document_text)).corner_radii(px(2.0)));
        for k in 0..l.lines.len() {
            line(window, k, px(0.0), style::c(p.document_background));
            band(window, k, px(0.0));
        }
    } else if desktop {
        // In the light look, a halo round the words, a device pixel each
        // way, under their shadow: white words with a shadow alone read
        // at about 2:1 over a light wallpaper (Opus's look, 2026-11-28).
        // (The dark look's as it was.)
        let halo = p.mode == olo_kit::palette::ThemeMode::Light;
        let soft = style::c(p.desktop_text_shadow).opacity(0.55);
        // (A device pixel: a point's at 2×, two, outlined the words.)
        let (z, d1) = (px(0.0), px(1.0 / window.scale_factor()));
        for k in 0..l.lines.len() {
            // (Decoration, the halo and the shadow: the review judges the
            // words. Recorded, a label's shadow was taken for a label of
            // its own, beside the next column's a point off level: the
            // review's near-level, wherever the icons are in columns.)
            olo_kit::review::unrecorded(window, |window| {
                if halo {
                    for d in [(-d1, z), (d1, z), (z, -d1), (z, d1), (-d1, -d1), (d1, -d1), (-d1, d1)] {
                        line_at(window, k, d, soft);
                    }
                }
                line(window, k, shadow, style::c(p.desktop_text_shadow));
            });
            line(window, k, px(0.0), style::c(p.desktop_text));
            band(window, k, shadow);
        }
    } else {
        for k in 0..l.lines.len() {
            line(window, k, px(0.0), style::c(p.document_text));
            band(window, k, px(0.0));
        }
    }
    // Cut short: the whole name in a tip for the pointer resting on it.
    if l.lines.last().is_some_and(|(t, _)| t.ends_with('…')) && !l.name.ends_with('…') {
        olo_kit::cut::note(window, l.name.clone(), l.highlight());
    }
}

/// A broken link's mark: Olo's warning at its icon's lower right, two
/// fifths of it (nine points at least), over the icon drawn half there
/// (`paint_item`, the list's name cell). It showed as the file it had
/// pointed to, nothing to say it was gone but its Kind (Opus's and
/// Gemini's looks, 2026-11-28).
pub fn paint_broken(window: &mut Painter, icon: Bounds<Pixels>) {
    let d = (f32::from(icon.size.width) * 0.42).round().max(9.0);
    let at = point(icon.right() - px(d / 2.0 - 1.0), icon.bottom() - px(d / 2.0 - 1.0));
    let badge = style::snap_centred(window, at, d, d);
    // (Its icon's, at its corner: not an icon of its own to the layout
    // review, which found it off-centre in a list's row.)
    olo_kit::review::unrecorded(window, |window| icons::paint(window, badge, "warning"));
    agent::record(window, agent::Node::label(olo_kit::words::tr("Broken Link"), badge));
}

/// DiskBackup's tick (`backups`): its current version is backed up. A
/// green disc at the icon's lower right, edged a shade darker, a white
/// tick in it; said to agents as "Backed up: NAME".
pub fn paint_mark(window: &mut Painter, icon: Bounds<Pixels>, name: &str, p: &Palette) {
    let d = (f32::from(icon.size.width) * 0.42).round().clamp(8.0, 14.0);
    let at = point(icon.right() - px(d / 2.0 - 1.0), icon.bottom() - px(d / 2.0 - 1.0));
    let disc = style::snap_centred(window, at, d, d);
    let green = style::c(p.success);
    let line = style::hairline(window);
    window.paint_quad(olo_kit::quad(disc, px(d / 2.0), green, line, style::shade(p, p.success, 0.75), olo_kit::BorderStyle::Solid));
    let (l, t) = (f32::from(disc.left()), f32::from(disc.top()));
    let mut tick = olo_kit::PathBuilder::stroke(px((d * 0.15).max(1.3)));
    tick.move_to(point(px(l + 0.28 * d), px(t + 0.53 * d)));
    tick.line_to(point(px(l + 0.44 * d), px(t + 0.69 * d)));
    tick.line_to(point(px(l + 0.73 * d), px(t + 0.35 * d)));
    if let Ok(path) = tick.build() {
        window.paint_path(path, olo_kit::white());
    }
    agent::record(window, agent::Node { role: "Image", name: Some(olo_kit::tr!("Backed up: {}", name).into()), bounds: disc, ..Default::default() });
}

/// How far along a download is, as a bar across the foot of its icon
/// (as Finder shows one).
pub fn paint_progress(window: &mut Painter, icon: Bounds<Pixels>, done: f32, p: &Palette) {
    let h = (f32::from(icon.size.height) / 8.0).clamp(5.0, 8.0).round();
    let w = (f32::from(icon.size.width) * 0.75).round();
    let bar = style::snap(window, Bounds { origin: point(icon.center().x - px(w / 2.0), icon.bottom() - px(h)), size: size(px(w), px(h)) });
    let mut well = style::c(p.document_text);
    well.a = 46.0 / 255.0;
    window.paint_quad(olo_kit::fill(bar, well).corner_radii(px(h / 2.0)));
    // As much of it filled as is done, never less than its round end.
    let end = style::snap_coord(window, bar.left() + bar.size.width * done.clamp(0.0, 1.0)).max(bar.left() + bar.size.height);
    window.paint_quad(olo_kit::fill(Bounds::from_corners(bar.origin, point(end, bar.bottom())), style::c(p.status_bar)).corner_radii(px(h / 2.0)));
}

/// The icon for an entry (folders that are special have their own).
pub fn icon_of(e: &Entry) -> &'static str {
    if e.is_dir {
        // (The three folders with icons of their own, worked out once:
        // this is asked for every row at every painting, and each was a
        // path made to be compared and thrown away.)
        static SPECIAL: std::sync::OnceLock<[std::path::PathBuf; 3]> = std::sync::OnceLock::new();
        let [home, desktop, trash] = SPECIAL.get_or_init(|| {
            let home = fs::home();
            [home.clone(), home.join("Desktop"), fs::trash_files()]
        });
        // (As they're written: a path compared as a path is taken apart
        // into its components, both of them, for every row.)
        let path = e.path.as_os_str();
        if path == home.as_os_str() {
            return "home";
        }
        if path == desktop.as_os_str() {
            return "desktop";
        }
        if path == trash.as_os_str() {
            return if fs::trash_is_empty() { "trash-empty" } else { "trash-full" };
        }
    }
    e.app_icon.or_else(|| crate::appimages::icon(e)).unwrap_or_else(|| e.kind.icon())
}

// ---- dragging out -----------------------------------------------------------

/// Marks a drag started with the secondary button: the place it is
/// dropped on asks what to do (Move Here, Copy Here, Create Link Here),
/// as on BeOS.
pub const ASK_TYPE: &str = "application/x-olo-ask";

/// The type (no data) naming the file system the dragged files are on,
/// so a folder they're dragged over knows before the drop whether it
/// would move or copy them.
fn device_type(dev: u64) -> String {
    format!("application/x-olo-device-{dev}")
}

/// The type (no data) naming the folder they come from (a hash of its
/// path), so a window can refuse them dropped back where they are.
fn folder_type(dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    format!("application/x-olo-from-folder-{:016x}", h.finish())
}

/// Items to drag, and how.
pub struct Drag {
    pub paths: Vec<PathBuf>,
    /// The icon of the one the pointer is on, its size, and where on it
    /// the pointer is.
    pub icon: &'static str,
    pub size: f32,
    pub hotspot: Point<Pixels>,
    /// With the secondary button.
    pub ask: bool,
    pub name: String,
}

/// The drag of ours under way: the window it started in (its number,
/// `ToHub`'s), and the folder its items come from (dropping them back
/// there does nothing). The app's: a drag out of one window is over
/// another, on another looper.
static DRAGGING: Mutex<Option<(u64, Option<PathBuf>)>> = Mutex::new(None);

/// Whether the drag over a window is ours, from window `view`.
pub fn from_view(view: u64) -> bool {
    DRAGGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(v, _)| *v == view)
}

/// The folder our drag under way comes from.
fn dragged_from() -> Option<PathBuf> {
    DRAGGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|(_, f)| f.clone())
}

fn remote_host_type(host: &str) -> String {
    let mut h = DefaultHasher::new();
    host.to_ascii_lowercase().hash(&mut h);
    format!("application/x-olo-rex-host-{:016x}", h.finish())
}

/// Starts dragging `d` out of window `view` (while the button is still
/// down): the files, plus Olo's own types saying which folder and file
/// system they are from and whether the drop should ask. The view is
/// sent `done` when it's over.
pub fn start<V: View>(cx: &mut Cx<V>, view: u64, d: Drag, done: impl FnOnce() -> V::Message + 'static)
where
    V::Message: Send,
{
    use std::os::unix::fs::MetadataExt;
    let from = d.paths.first().and_then(|p| fs::parent_of(p));
    let mut drag = olo_kit::dnd::files(&d.paths, true, true);
    drag.ask = true;
    let mut extra: Vec<String> = Vec::new();
    if let Some(dir) = &from {
        extra.push(folder_type(dir));
    }
    if let Some(rp) = d.paths.first().and_then(|p| fs::rex_path(p)) {
        extra.push(remote_host_type(&rp.host));
    } else if let Some(dev) = d.paths.first().and_then(|p| std::fs::symlink_metadata(p).ok()).map(|m| m.dev()) {
        extra.push(device_type(dev));
    }
    if d.ask {
        extra.push(ASK_TYPE.into());
    }
    for t in extra {
        drag.types.insert(0, (t, std::sync::Arc::from(Vec::new())));
    }
    let n = d.paths.len();
    let name = if n > 1 { format!("{n} items") } else { d.name.clone() };
    drag.icon = olo_kit::dnd::picture(cx, d.icon, d.size, Some(&name), d.hotspot);
    *DRAGGING.lock().unwrap_or_else(|e| e.into_inner()) = Some((view, from));
    olo_kit::dnd::start(cx, drag, move |_| {
        *DRAGGING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        done()
    });
}

// ---- taking drops -----------------------------------------------------------

/// Where a drop goes; [`DropTarget::name`] is what the window takes it
/// as, and comes back with the drop.
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    /// Into a folder (files) or as a clipping file in it (text).
    Folder(PathBuf),
    Trash,
    /// Icons dragged about the desktop: put them where they are dropped.
    Place,
}

impl DropTarget {
    pub fn name(&self) -> String {
        match self {
            DropTarget::Folder(p) => format!("folder:{}", p.display()),
            DropTarget::Trash => "trash".into(),
            DropTarget::Place => "place".into(),
        }
    }

    pub fn parse(name: &str) -> Option<DropTarget> {
        match name {
            "trash" => Some(DropTarget::Trash),
            "place" => Some(DropTarget::Place),
            _ => name.strip_prefix("folder:").map(|p| DropTarget::Folder(PathBuf::from(p))),
        }
    }
}

/// The first text type it comes in.
fn text_type(o: &DropOffer) -> Option<&'static str> {
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"].into_iter().find(|t| o.has(t))
}

/// Takes what is dragged over folder `dir` if it would go there, choosing
/// as BeOS's Tracker did: move within a file system, copy across them,
/// ⌥ to copy anyway, the secondary button to ask; the Trash takes files
/// only, and moves them. Text becomes a clipping file. Returns whether it
/// would be taken (None: refused; to highlight `dir`).
pub fn take_drop(cx: &WindowCx, o: &DropOffer, dir: &Path) -> Option<DropAccept> {
    use std::os::unix::fs::MetadataExt;
    let trash = dir == fs::trash_files();
    let target = if trash { DropTarget::Trash } else { DropTarget::Folder(dir.to_owned()) };
    let accept = |mime: &str, action: DragAction| Some(DropAccept { target: target.name(), mime: mime.to_owned(), action });
    if o.has("text/uri-list") {
        let action = if o.has(ASK_TYPE) && o.ask && !trash {
            DragAction::Ask
        } else if trash {
            if !o.move_ {
                return None;
            }
            DragAction::Move
        } else {
            let same_fs = if let Some(rp) = fs::rex_path(dir) { o.has(&remote_host_type(&rp.host)) } else { std::fs::metadata(dir).is_ok_and(|m| o.has(&device_type(m.dev()))) };
            let copy = cx.modifiers().alt || !same_fs || !o.move_;
            // Only our own drags move: another program might delete what
            // it gave us once told it moved.
            if copy || !o.from_us {
                if !o.copy {
                    return None;
                }
                DragAction::Copy
            } else {
                DragAction::Move
            }
        };
        accept("text/uri-list", action)
    } else if trash || !o.copy {
        None
    } else if let Some(t) = o.types.iter().find(|t| olo_kit_base::files::named_file(t).is_some()) {
        // A picture dragged out of a web page: the file itself.
        accept(t, DragAction::Copy)
    } else if o.has("text/x-moz-url") {
        // A link: a clipping of it, named by its text.
        accept("text/x-moz-url", DragAction::Copy)
    } else if let Some(t) = text_type(o) {
        accept(t, DragAction::Copy)
    } else {
        None
    }
}

/// Over a view of items in folder `here`: the item it would be dropped on
/// (`over`: its index, entry, and whether it's selected; it takes things
/// if it's a folder not itself dragged), else the folder. How it would
/// be taken (None: refused), and the item it would go on.
pub fn item_drop(cx: &WindowCx, view: u64, o: &DropOffer, over: Option<(usize, &Entry, bool)>, here: Option<&Path>) -> (Option<DropAccept>, Option<usize>) {
    let from_here = from_view(view);
    if let Some((i, e, selected)) = over
        && e.is_dir
        && !(from_here && selected)
    {
        return (take_drop(cx, o, &e.path), Some(i));
    }
    match here {
        // Dropping in the folder it came from would do nothing (another
        // column of the same window is another folder); nor from another
        // app's drag of a file here (Viewer, Quick Look).
        Some(dir) if !(from_here && dragged_from().as_deref() == Some(dir)) && !o.has(&folder_type(dir)) => (take_drop(cx, o, dir), None),
        _ => (None, None),
    }
}

/// Outlines `r` (a view, a column) as taking what's dragged over it: a
/// line two points wide inside it in the keyboard's colour, above what's
/// in it.
pub fn outline(window: &mut Painter, r: Bounds<Pixels>, p: &Palette) {
    let ink = style::c(p.keyboard_navigation);
    style::frame_rect(window, r, ink);
    style::frame_rect(window, style::inset(window, r, 1.0), ink);
}

/// How long a drag rests on a folder before it springs open.
pub const SPRING: Duration = Duration::from_millis(1000);

/// A folder a drag is resting on (spring-loaded folders, as Tracker's):
/// after [`SPRING`] it opens, so the drag can go on into it.
#[derive(Default)]
pub struct Spring {
    at: Option<(PathBuf, Instant, bool)>,
}

impl Spring {
    /// The drag rests on `dir` now (None: on nothing that opens), as
    /// its view was asked where it would take it. Returns whether to
    /// look again once [`SPRING`] has passed ([`Spring::due`]): the
    /// pointer may rest there and nothing more be asked.
    pub fn rest(&mut self, dir: Option<&Path>) -> bool {
        let Some(dir) = dir else {
            self.at = None;
            return false;
        };
        match &self.at {
            Some((d, ..)) if d == dir => false,
            _ => {
                self.at = Some((dir.to_owned(), Instant::now(), false));
                true
            }
        }
    }

    /// The folder to open now, if the drag has rested on one long
    /// enough (once for each rest).
    pub fn due(&mut self) -> Option<PathBuf> {
        match &mut self.at {
            Some((dir, since, opened)) if !*opened && since.elapsed() >= SPRING => {
                *opened = true;
                Some(dir.clone())
            }
            _ => None,
        }
    }
}

// ---- editing a name in place ------------------------------------------------

/// A name being edited where its label was (Edit Name): the field, and
/// what it renames.
pub struct Rename {
    pub path: PathBuf,
    pub field: Child<TextField>,
    link: bool,
}

impl Rename {
    /// Starts editing `path`'s name, its name up to its extension
    /// selected (".txt" rarely changes); the field has the keyboard.
    pub fn new<V: View>(path: PathBuf, link: bool, cx: &mut Cx<V>) -> Rename {
        let name = fs::leaf_name(&path);
        let stem = match name.rfind('.') {
            Some(i) if i > 0 => i,
            _ => name.len(),
        };
        let field = text_field(cx, |f| f.keeps_enter().font(name_font(link)).name("Name"));
        field.update(cx, |f, cx| {
            f.set_text(name, cx);
            f.set_selection(0..stem, cx);
        });
        field.focus(cx);
        Rename { path, field, link }
    }

    pub fn text(&self, cx: &WindowCx) -> String {
        self.field.read(cx).text().to_owned()
    }

    /// The field, over `l`'s label, its text on the label's baseline:
    /// the document's ground framed in the keyboard's colour, growing as
    /// the name does. `origin`: where what it's a child of starts.
    pub fn element(&self, cx: &WindowCx, l: &ItemLayout, origin: Point<Pixels>) -> Element {
        self.element_within(cx, l, origin, None)
    }

    /// The same, its right edge at `right` at most (a list's cell: a
    /// longer name scrolls in it).
    pub fn element_within(&self, cx: &WindowCx, l: &ItemLayout, origin: Point<Pixels>, right: Option<Pixels>) -> Element {
        use olo_kit::{IntoElement as _, ParentElement as _, Styled as _};
        let text = self.text(cx);
        let measure = if text.is_empty() { "M".to_owned() } else { text };
        let w = style::text_width(measure, style::PLAIN_SIZE, name_font(self.link));
        let f = l.field(w);
        let f_right = right.map_or(f.right(), |r| f.right().min(r).max(f.left()));
        let r = Bounds::from_corners(point(snap(f.left()), snap(f.top())), point(snap(f_right), snap(f.bottom())));
        let frame = olo_kit::canvas(move |g, b| {
            let p = olo_kit::theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
            style::frame_rect(g, b, style::c(p.keyboard_navigation));
        })
        .absolute()
        .inset_0();
        olo_kit::div()
            .absolute()
            .left(r.left() - origin.x)
            .top(r.top() - origin.y)
            .w(r.size.width)
            .h(r.size.height)
            .child(frame)
            .child(olo_kit::div().absolute().left(px(LABEL_PAD.0)).right(px(1.0)).top_0().bottom_0().child(&self.field))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How far below the top of its cell an icon-view label of `rows`
    /// lines ends, its highlight included, at `scale` (as
    /// `ItemLayout::highlight` has it).
    fn label_bottom(rows: usize, scale: f32) -> f32 {
        let (baseline, step) = label_rows(m::TIGHT + ICON, scale);
        baseline + step * rows.saturating_sub(1) as f32 + LABEL_PAD.1
    }

    /// At every scale a two-line label's highlight ends a spacing or more
    /// above the next row's icon (twice the gap between an icon and its
    /// own label, so a label reads as its icon's), where the 84-point
    /// cell had it run into that icon.
    #[test]
    fn cell_room_under_a_label() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let next_icon = ICON_CELL.1 + m::TIGHT;
            let bottom = label_bottom(2, scale);
            assert!(next_icon - bottom >= m::SPACING, "at {scale}×: {:.2} points under a label, less than a spacing", next_icon - bottom);
            // (The old cell's overlap, as the review found it: 2 to 3.3.)
            assert!(bottom > 84.0 + m::TIGHT, "at {scale}×: the old cell didn't overlap");
            // Every row on a whole device pixel.
            let device = ICON_CELL.1 * scale;
            assert_eq!(device, device.round(), "at {scale}× a row is {device} device pixels");
        }
    }

    /// Where a word too long for its line is broken (`fits`: what the
    /// next line holds, here 13 letters): after a "-" or "_" whose rest
    /// fits the last line whole, the latest; else the latest leaving half
    /// of what fits; at a space as the wrapper had it; never so a line
    /// starts with the extension where a separator would do.
    #[test]
    fn names_break_at_separators() {
        let fits = |s: &str| s.trim().chars().count() <= 13;
        let at = |line: &str, b: &str| at_separator(line, b.len(), true, fits);
        // (As the wrapper breaks them: as much as fits, mid-word.)
        assert_eq!(at("storage-disk-backup-01", "storage-disk-ba"), "storage-disk-".len());
        // An extension isn't left alone on the next line: "test_" over
        // "page.html", not "test_page" over ".html".
        assert_eq!(at("test_page.html", "test_page.htm"), "test_".len());
        assert_eq!(at("tiny_16x16.png", "tiny_16x16.pn"), "tiny_".len());
        assert_eq!(at("large_50k_lines.txt", "large_50k_line"), "large_50k_".len());
        assert_eq!(at("wide_strip_1200x40.png", "wide_strip_120"), "wide_strip_".len());
        // The wrapper's own break after a "-" or "_" is kept, and so is
        // one before a "." where nothing else would do.
        assert_eq!(at("report-final.pdf", "report-"), "report-".len());
        assert_eq!(at("reportfinal.pdf", "reportfinal"), "reportfinal".len());
        // An earlier "_" over the wrapper's break before ".": the extension
        // stays with its name.
        assert_eq!(at("report_final.pdf", "report_final"), "report_".len());
        // At a space: as the wrapper had it.
        assert_eq!(at("folder with spaces", "folder "), "folder ".len());
        assert_eq!(at("folder with spaces", "folder"), "folder".len());
        // No separator, or only one leaving under half the line with a
        // rest too long for the next: mid-word.
        assert_eq!(at("abcdefghijklmnop", "abcdefghij"), 10);
        assert_eq!(at("a-bcdefghijklmnopqrstu", "a-bcdefghijk"), 12);
        // The rest too long for the last line either way: the latest
        // separator keeping half the line.
        assert_eq!(at("super_long_filename_abcdefghij_end.txt", "super_long_filen"), "super_long_".len());
        // Not the last line (more to come): the latest keeping half.
        assert_eq!(at_separator("test_page.html", "test_page.htm".len(), false, fits), "test_page.htm".len());
        assert_eq!(at_separator("large_50k_lines.txt", "large_50k_line".len(), false, fits), "large_50k_".len());
        // A name starting with a dot isn't broken before it.
        assert_eq!(at(".bashrc_history_long", ".bashrc_hist"), ".bashrc_".len());
        // Letters of more than a byte.
        assert_eq!(at("日本語-ドキュメント", "日本語-ドキュ"), "日本語-".len());
    }

    /// The names the reviews found broken badly, wrapped as a 96-point
    /// cell wraps them; and a selected item's label as an unselected one's
    /// (two lines: four ran into the row under it).
    #[test]
    fn labels_wrap_at_separators() {
        let font = name_font(false);
        let width = px(ICON_CELL.0 - 2.0 * m::TIGHT);
        let lines = |text: &str| wrap_lines(text, &font, width, LABEL_ROWS).into_iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert_eq!(lines("storage-disk-backup-01"), ["storage-disk-", "backup-01"]);
        assert_eq!(lines("tiny_16x16.png"), ["tiny_", "16x16.png"]);
        assert_eq!(lines("test_page.html"), ["test_", "page.html"]);
        // No line starts with an extension's dot.
        for name in ["wide_strip_1200x40.png", "large_50k_lines.txt", "latin1_non_utf8.txt", "folder with spaces & symbols (2026)"] {
            let l = lines(name);
            assert!(l.iter().all(|line| !line.starts_with('.')), "{name}: {l:?}");
        }
        // What fits stays on one line.
        assert_eq!(lines("Home"), ["Home"]);
        // Too long for two lines: the second cut short with "…".
        let long = lines("super_long_filename_abcdefghij_abcdefghij_abcdefghij_end.txt");
        assert_eq!(long.len(), 2);
        assert!(long[1].ends_with('…'), "{long:?}");
        // Selected: the same two lines, its highlight a spacing above the
        // next row's icon.
        let cell = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(ICON_CELL.0), px(ICON_CELL.1)) };
        let l = layout_item(cell, "super_long_filename_abcdefghij_abcdefghij_abcdefghij_end.txt", Look::Icons, false);
        assert_eq!(l.lines.len(), 2);
        assert!(f32::from(l.hit().bottom()) + m::SPACING <= ICON_CELL.1 + m::TIGHT, "its label reaches {:?}", l.hit().bottom());
    }
}
