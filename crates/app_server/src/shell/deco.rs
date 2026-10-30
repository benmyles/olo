//! The BeOS window decorator: a yellow tab sized to the title, close box on
//! the left, zoom box on the right, and a five-line 3D frame.
//!
//! Everything is laid out in logical pixels relative to the element origin
//! (the frame's top-left corner, tab row included) and drawn directly in
//! device pixels, so lines stay crisp at any output scale: the frame is
//! drawn out from where the content lands, its lines whole pixels and the
//! same on every side ([`lines`]), and the tab is a picture drawn pixel for
//! pixel (`render::exact`).

use std::{
    cell::{RefCell, RefMut},
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Id, Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::SolidColorRenderElement,
            },
            utils::CommitCounter,
        },
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};
use olo_theme::{Palette, Rgba, deco::*, fonts, mix};
use tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Pixmap, Rect, Shader, SpreadMode,
    Transform as SkTransform,
};

use super::{ResizeEdge, WindowElement};
use crate::{render::Exact, text, theme};

/// What part of the decoration a point falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Part {
    #[default]
    None,
    Tab,
    Close,
    Zoom,
    Frame(ResizeEdge),
}

pub struct Decoration {
    /// Server-side decorations negotiated (and not fullscreen).
    pub enabled: bool,
    pub focused: bool,
    /// Shift-drag slides the tab along the top edge.
    pub tab_offset: i32,
    pub hover: Part,
    /// Part the button went down on; close/zoom act on release over it.
    pub pressed: Part,
    /// Where the pointer last moved over the decoration. Button handlers
    /// run inside smithay's pointer lock, so they can't ask the pointer.
    pub pointer: Point<f64, Logical>,
    /// The last press on the tab or an edge (when, where, which), to
    /// tell a double-click.
    pub last_press: Option<(Instant, Point<f64, Logical>, Part)>,
    /// Rolled up: only the tab shows (and takes the pointer).
    pub rolled: bool,
    /// Behind another window of its group: only the tab shows, as when
    /// rolled up (`groups::mark_behind`).
    pub behind: bool,
    /// Its tab's width when the window is in a group (each window's tab
    /// gets its place along the shared top, `groups`); else as wide as its
    /// title wants.
    pub tab_width: Option<i32>,
    /// The number of the group it is in (`groups::Groups::place`).
    pub group: Option<u64>,
    /// Being put in a group by hand (`tabdrag`): only its tab shows, in
    /// its place among the group's, while it is held over them and until
    /// it has drawn at the group's size (since then, if let go).
    pub docked: bool,
    pub docked_since: Option<Instant>,
    /// Its group is where the tab being dragged would go: lit up.
    pub target: bool,
    title_width: Option<(String, i32)>,
    /// The tab as drawn for its last few looks (focused or not, a box
    /// hovered or pressed), the current one first: going back to one
    /// (Cmd+Tab back and forth, the pointer over the boxes) shows the
    /// texture already on the GPU rather than uploading another, which
    /// can hold up a frame by milliseconds while the GPU is busy.
    tabs: Vec<(TabKey, MemoryRenderBuffer)>,
    frame_ids: Vec<Id>,
    frame_commit: CommitCounter,
    frame_key: Option<(bool, u64, bool)>,
}

#[derive(Debug, Clone, PartialEq)]
struct TabKey {
    title: String,
    focused: bool,
    hover: Part,
    pressed: Part,
    width: i32,
    phys: (i32, i32),
    generation: u64,
    target: bool,
}

pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How many looks of a tab are kept drawn (`Decoration::tabs`).
const TAB_LOOKS: usize = 6;

/// Logical extra size the decoration adds around the client's geometry.
pub fn extra_size() -> Size<i32, Logical> {
    (2 * BORDER, TAB_HEIGHT + 2 * BORDER).into()
}

/// Where the client's geometry sits inside the decorated element.
pub fn content_offset() -> Point<i32, Logical> {
    (BORDER, TAB_HEIGHT + BORDER).into()
}

impl Default for Decoration {
    fn default() -> Self {
        Decoration {
            enabled: false,
            focused: false,
            tab_offset: 0,
            hover: Part::None,
            pressed: Part::None,
            pointer: Point::default(),
            last_press: None,
            rolled: false,
            behind: false,
            tab_width: None,
            group: None,
            docked: false,
            docked_since: None,
            target: false,
            title_width: None,
            tabs: Vec::new(),
            frame_ids: (0..16).map(|_| Id::new()).collect(),
            frame_commit: CommitCounter::default(),
            frame_key: None,
        }
    }
}

/// Tab placement for a window whose content is `content` wide.
#[derive(Debug, Clone, Copy)]
pub struct TabLayout {
    pub x: i32,
    pub width: i32,
}

impl TabLayout {
    pub fn rect(&self) -> Rectangle<i32, Logical> {
        Rectangle::new((self.x, 0).into(), (self.width, TAB_HEIGHT).into())
    }
    pub fn close_rect(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (self.x + BOX_INSET, (TAB_HEIGHT - BOX_SIZE) / 2).into(),
            (BOX_SIZE, BOX_SIZE).into(),
        )
    }
    pub fn zoom_rect(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (self.x + self.width - BOX_INSET - BOX_SIZE, (TAB_HEIGHT - BOX_SIZE) / 2).into(),
            (BOX_SIZE, BOX_SIZE).into(),
        )
    }
    /// Room for the title between the boxes.
    pub fn title_space(&self) -> i32 {
        self.width - 2 * (BOX_INSET + BOX_SIZE + TITLE_GAP)
    }
}

/// The most characters of a title a tab shows (Preferences ▸ Appearance).
static TITLE_CHARS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(100);

pub fn set_title_chars(n: u32) {
    TITLE_CHARS.store(n.max(1) as usize, std::sync::atomic::Ordering::Relaxed);
}

/// `title` cut to the most a tab shows, with an ellipsis if it was cut.
fn cut(title: &str) -> std::borrow::Cow<'_, str> {
    let max = TITLE_CHARS.load(std::sync::atomic::Ordering::Relaxed);
    match title.char_indices().nth(max) {
        Some(_) => {
            let end = title.char_indices().nth(max.saturating_sub(1)).map_or(title.len(), |(i, _)| i);
            format!("{}…", title[..end].trim_end()).into()
        }
        None => title.into(),
    }
}

impl Decoration {
    fn title_width(&mut self, title: &str) -> i32 {
        let title = &*cut(title);
        match &self.title_width {
            Some((t, w)) if t == title => *w,
            _ => {
                let w = text::measure(title, fonts::TAB_SIZE, true).ceil() as i32;
                self.title_width = Some((title.to_owned(), w));
                w
            }
        }
    }

    /// How wide a tab wants to be for `title`, with room for it all.
    pub fn wanted_tab_width(&mut self, title: &str) -> i32 {
        (2 * (BOX_INSET + BOX_SIZE + TITLE_GAP) + self.title_width(title)).max(MIN_TAB_WIDTH)
    }

    pub fn tab_layout(&mut self, title: &str, content: Size<i32, Logical>) -> TabLayout {
        let frame_w = content.w + 2 * BORDER;
        let wanted = match self.tab_width {
            Some(w) => w,
            None => self.wanted_tab_width(title),
        };
        // Docked: its place among another frame's tabs, wherever that is.
        if self.docked {
            return TabLayout { x: self.tab_offset.max(0), width: wanted.max(1) };
        }
        let width = wanted.min(frame_w).max(1);
        let x = self.tab_offset.clamp(0, (frame_w - width).max(0));
        TabLayout { x, width }
    }

    /// Classify an element-local point.
    pub fn hit(&mut self, title: &str, content: Size<i32, Logical>, p: Point<f64, Logical>) -> Part {
        let tab = self.tab_layout(title, content);
        let (w, h) = (content.w + 2 * BORDER, content.h + TAB_HEIGHT + 2 * BORDER);
        let contains = |r: Rectangle<i32, Logical>| r.to_f64().contains(p);
        if (self.rolled || self.behind || self.docked) && p.y >= TAB_HEIGHT as f64 {
            return Part::None;
        }
        if p.y < TAB_HEIGHT as f64 {
            if !contains(tab.rect()) {
                return Part::None;
            }
            // Generous hit boxes: the full tab height around each box.
            let grow = |r: Rectangle<i32, Logical>| {
                Rectangle::<i32, Logical>::new((r.loc.x - 2, 0).into(), (r.size.w + 4, TAB_HEIGHT).into())
            };
            if contains(grow(tab.close_rect())) {
                return Part::Close;
            }
            if contains(grow(tab.zoom_rect())) {
                return Part::Zoom;
            }
            return Part::Tab;
        }
        let (x, y) = (p.x, p.y - TAB_HEIGHT as f64);
        let (fw, fh) = (w as f64, (h - TAB_HEIGHT) as f64);
        if x < 0.0 || y < 0.0 || x >= fw || y >= fh {
            return Part::None;
        }
        let b = BORDER as f64;
        let c = CORNER as f64;
        let (left, right) = (x < b, x >= fw - b);
        let (top, bottom) = (y < b, y >= fh - b);
        if !(left || right || top || bottom) {
            return Part::None;
        }
        let mut edges = ResizeEdge::NONE;
        if left {
            edges |= ResizeEdge::LEFT;
        }
        if right {
            edges |= ResizeEdge::RIGHT;
        }
        if top {
            edges |= ResizeEdge::TOP;
        }
        if bottom {
            edges |= ResizeEdge::BOTTOM;
        }
        // Corners extend CORNER pixels along each edge.
        if left || right {
            if y < c {
                edges |= ResizeEdge::TOP;
            } else if y >= fh - c {
                edges |= ResizeEdge::BOTTOM;
            }
        }
        if top || bottom {
            if x < c {
                edges |= ResizeEdge::LEFT;
            } else if x >= fw - c {
                edges |= ResizeEdge::RIGHT;
            }
        }
        Part::Frame(edges)
    }

    /// Whether `p` is on the tab or the frame (not the content).
    pub fn is_on_decoration(&mut self, title: &str, content: Size<i32, Logical>, p: Point<f64, Logical>) -> bool {
        self.hit(title, content, p) != Part::None
    }

    /// Render elements for the tab and frame, front-most first.
    /// Its outline in device pixels from `location`, as `render_elements`
    /// draws it: the frame's outer rectangle, and its tab's left and right
    /// (from the frame's left) and height (`shell::glow`).
    pub fn outline(&mut self, title: &str, content: Size<i32, Logical>, location: Point<i32, Physical>, scale: Scale<f64>) -> (Rectangle<i32, Physical>, (i32, i32, i32)) {
        let s = scale.x;
        let px = |l: i32| (l as f64 * s).round() as i32;
        let tab = self.tab_layout(title, content);
        let border = px(BORDER);
        let frame_top = px(TAB_HEIGHT + BORDER) - border;
        let content_px = Size::<i32, Physical>::from(((content.w as f64 * s).round() as i32, (content.h as f64 * s).round() as i32));
        let frame_w = content.w + 2 * BORDER;
        let frame_right = border + content_px.w + border;
        let tx = |l: i32| if l >= frame_w { frame_right + px(l) - px(frame_w) } else { px(l) };
        let frame = Rectangle::new((location.x, location.y + frame_top).into(), (content_px.w + 2 * border, content_px.h + 2 * border).into());
        (frame, (tx(tab.x), tx(tab.x + tab.width), frame_top))
    }

    pub fn render_elements<R>(
        &mut self,
        renderer: &mut R,
        title: &str,
        content: Size<i32, Logical>,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> (Vec<Exact<MemoryRenderBufferRenderElement<R>>>, Vec<SolidColorRenderElement>)
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let palette = theme::palette();
        let generation = theme::generation();
        let s = scale.x;
        let px = |l: i32| (l as f64 * s).round() as i32;
        let tab = self.tab_layout(title, content);

        // Where things land, in device pixels from `location`. The content
        // is where `client_render_offset` puts it, as big as its buffer;
        // the frame is a border's width around it on every side (so it can
        // reach a pixel past `px` of its logical right and bottom), and the
        // tab sits on it.
        let border = px(BORDER);
        let content_at = Point::<i32, Physical>::from((border, px(TAB_HEIGHT + BORDER)));
        let content_px = Size::<i32, Physical>::from(((content.w as f64 * s).round() as i32, (content.h as f64 * s).round() as i32));
        let frame_top = content_at.y - border;
        let frame_w = content.w + 2 * BORDER;
        let frame_right = content_at.x + content_px.w + border;
        // Along the top: past the frame's right edge, as far past where it
        // is drawn, so a tab ending there ends on it.
        let tx = |l: i32| if l >= frame_w { frame_right + px(l) - px(frame_w) } else { px(l) };

        // Tab: rendered to a pixmap at the exact device size it covers.
        let phys = (tx(tab.x + tab.width) - tx(tab.x), frame_top);
        let key = TabKey {
            title: cut(title).into_owned(),
            focused: self.focused,
            hover: self.hover,
            pressed: self.pressed,
            width: tab.width,
            phys,
            generation,
            target: self.target,
        };
        if let Some(i) = self.tabs.iter().position(|(k, _)| *k == key) {
            let looked = self.tabs.remove(i);
            self.tabs.insert(0, looked);
        } else if phys.0 > 0 && phys.1 > 0 {
            let pixmap = draw_tab(&palette, &key, &tab, s);
            let size: Size<i32, Buffer> = phys.into();
            let buffer = MemoryRenderBuffer::from_slice(
                pixmap.data(),
                Fourcc::Abgr8888,
                size,
                1,
                Transform::Normal,
                Some(vec![Rectangle::from_size(size)]),
            );
            self.tabs.insert(0, (key, buffer));
            self.tabs.truncate(TAB_LOOKS);
        }
        let mut tabs = Vec::new();
        if let Some((key, buffer)) = self.tabs.first() {
            let at = location + Point::<i32, Physical>::from((tx(tab.x), 0));
            let rect = Rectangle::new(at, key.phys.into());
            tabs.extend(crate::render::exact(renderer, rect, buffer, key.phys, Some(alpha), Kind::Unspecified));
        }

        // Frame: concentric bands out from the content (an outline, the
        // bevel's light, the flat middle and the bevel's shadow, BeOS's five
        // lines), each drawn as four rects.
        let fkey = (self.focused, generation, self.target);
        if self.frame_key != Some(fkey) {
            self.frame_key = Some(fkey);
            self.frame_commit.increment();
        }
        let f = if self.focused { &palette.frame_focused } else { &palette.frame_unfocused };
        // A group a tab is held over: its outer lines in the focus blue,
        // as its tabs' outlines are.
        let lit = [palette.keyboard_navigation, palette.keyboard_navigation, f[2], f[3], f[4], f[5]];
        let f = if self.target { &lit } else { f };
        let (outline, bevel) = lines(s);
        let rings = [(outline, f[0], f[0]), (bevel, f[1], f[4]), (border - outline - 2 * bevel, f[2], f[2]), (bevel, f[4], f[1])];
        let (x0, y0) = (location.x + content_at.x - border, location.y + frame_top);
        let (mut l, mut t, mut r, mut b) = (x0, y0, x0 + content_px.w + 2 * border, y0 + content_px.h + 2 * border);
        let rect = |l: i32, t: i32, r: i32, b: i32| -> Rectangle<i32, Physical> { Rectangle::new((l, t).into(), (r - l, b - t).into()) };
        let mut frame = Vec::with_capacity(16);
        for (i, (n, tl, br)) in rings.into_iter().enumerate() {
            if n <= 0 {
                continue;
            }
            let sides = [
                (rect(l, t, r, t + n), tl),         // top
                (rect(l, t + n, l + n, b), tl),     // left
                (rect(r - n, t + n, r, b), br),     // right
                (rect(l + n, b - n, r - n, b), br), // bottom
            ];
            for (j, (geo, color)) in sides.into_iter().enumerate() {
                if geo.size.w <= 0 || geo.size.h <= 0 {
                    continue;
                }
                let c = theme::to_f32(color);
                frame.push(SolidColorRenderElement::new(
                    self.frame_ids[i * 4 + j].clone(),
                    geo,
                    self.frame_commit,
                    [c[0] * alpha, c[1] * alpha, c[2] * alpha, c[3] * alpha],
                    Kind::Unspecified,
                ));
            }
            (l, t, r, b) = (l + n, t + n, r - n, b - n);
        }
        (tabs, frame)
    }
}

fn sk(c: Rgba) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

pub(crate) fn fill(pixmap: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
    if let Some(r) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        let mut paint = Paint::default();
        paint.set_color(sk(c));
        paint.anti_alias = false;
        pixmap.fill_rect(r, &paint, SkTransform::identity(), None);
    }
}

/// A BeOS "blended" box: diagonal gradient with a dark outline.
#[allow(clippy::too_many_arguments)]
fn blended_box(pixmap: &mut Pixmap, x: i32, y: i32, size: i32, u: i32, from: Rgba, to: Rgba, outline: Rgba) {
    fill(pixmap, x, y, size, size, outline);
    let (ix, iy, is) = (x + u, y + u, size - 2 * u);
    if is <= 0 {
        return;
    }
    let Some(r) = Rect::from_xywh(ix as f32, iy as f32, is as f32, is as f32) else {
        return;
    };
    let shader = LinearGradient::new(
        tiny_skia::Point::from_xy(ix as f32, iy as f32),
        tiny_skia::Point::from_xy((ix + is) as f32, (iy + is) as f32),
        vec![GradientStop::new(0.0, sk(from)), GradientStop::new(1.0, sk(to))],
        SpreadMode::Pad,
        SkTransform::identity(),
    )
    .unwrap_or(Shader::SolidColor(sk(from)));
    let paint = Paint { shader, anti_alias: false, ..Default::default() };
    pixmap.fill_rect(r, &paint, SkTransform::identity(), None);
}

/// The decoration's lines in device pixels at scale `s`, the same on
/// every side: an outline is a hairline, whole pixels and never more than
/// a point (as the kit's, `style::line_pixels`), and a bevel's light or
/// shadow is its point rounded. The frame's flat middle takes the rest of
/// the border, so at 1.5× it is 1, 2, 3 and 2 pixels (not the 2, 1, 2, 1, 2
/// that rounding each logical line from the window's corner gave, and
/// mirrored differently on the far side).
pub fn lines(s: f64) -> (i32, i32) {
    ((s.floor() as i32).max(1), (s.round() as i32).max(1))
}

fn draw_tab(palette: &Palette, key: &TabKey, tab: &TabLayout, s: f64) -> Pixmap {
    let (w, h) = key.phys;
    let mut pixmap = Pixmap::new(w as u32, h as u32).expect("tab pixmap");
    // Sizes and insets in device pixels, from the tab's own edges, so
    // every tab looks the same wherever it is.
    let px = |l: i32| (l as f64 * s).round() as i32;
    let (hair, bevel) = lines(s);

    let (base, light, shadow, text_color) = if key.focused {
        (
            palette.tab_focused,
            palette.tab_focused_light,
            palette.tab_focused_shadow,
            palette.tab_text_focused,
        )
    } else {
        (
            palette.tab_unfocused,
            palette.tab_unfocused_light,
            palette.tab_unfocused_shadow,
            palette.tab_text_unfocused,
        )
    };
    let outline = if key.focused { palette.frame_focused[0] } else { palette.frame_unfocused[0] };
    // Where a dragged tab would go: tinted, and outlined twice as thick
    // in the focus blue (`Decoration::target`).
    // As thick as the frame's lit outline and light under it.
    let nav = palette.keyboard_navigation;
    let (base, outline, line) = if key.target { (mix(base, nav, 0.18), nav, hair + bevel) } else { (base, outline, hair) };

    fill(&mut pixmap, 0, 0, w, h, base);
    // Outline on three sides; the frame's top line closes the bottom.
    fill(&mut pixmap, 0, 0, w, line, outline);
    fill(&mut pixmap, 0, 0, line, h, outline);
    fill(&mut pixmap, w - line, 0, line, h, outline);
    // Bevel, inside it.
    fill(&mut pixmap, line, line, w - 2 * line, bevel, light);
    fill(&mut pixmap, line, line, bevel, h - line, light);
    fill(&mut pixmap, w - line - bevel, line, bevel, h - line, shadow);

    // The boxes: the close box from the left edge, the zoom box from the
    // right, the same size and as far in.
    let size = px(BOX_SIZE);
    let top = px(tab.close_rect().loc.y);
    let inset = px(BOX_INSET);
    let box_colors = |part: Part| {
        let (mut from, mut to) = (mix(light, [255, 255, 255, 255], 0.5), mix(base, shadow, 0.6));
        if key.pressed == part && key.hover == part {
            std::mem::swap(&mut from, &mut to);
        } else if key.hover == part && key.pressed == Part::None {
            from = [255, 255, 255, 255];
            to = mix(base, shadow, 0.3);
        }
        (from, to)
    };
    let box_outline = mix(shadow, [0, 0, 0, 255], 0.35);

    // Close box.
    let (from, to) = box_colors(Part::Close);
    blended_box(&mut pixmap, inset, top, size, hair, from, to, box_outline);

    // Zoom box: a small square overlapping a larger one behind it.
    let (zx, zy, zs) = (w - inset - size, top, size);
    let (from, to) = box_colors(Part::Zoom);
    let big = (zs as f64 * 0.69).round() as i32;
    let small = (zs as f64 * 0.54).round() as i32;
    blended_box(&mut pixmap, zx + zs - big, zy + zs - big, big, hair, from, to, box_outline);
    blended_box(&mut pixmap, zx, zy, small, hair, from, to, box_outline);

    // Title.
    let space = tab.title_space();
    if space > 4 {
        let title = text::ellipsize(&key.title, fonts::TAB_SIZE, true, space as f32);
        let size = fonts::TAB_SIZE * s as f32;
        let tx = px(BOX_INSET + BOX_SIZE + TITLE_GAP) as f32;
        // Level with the boxes.
        let ty = text::top_for_centre(top as f32 + px(BOX_SIZE) as f32 / 2.0, size, true);
        text::draw(&mut pixmap, &title, tx, ty, size, true, text_color);
    }
    pixmap
}

impl WindowElement {
    pub fn decoration(&self) -> RefMut<'_, Decoration> {
        self.user_data()
            .insert_if_missing(|| RefCell::new(Decoration::default()));
        self.user_data()
            .get::<RefCell<Decoration>>()
            .unwrap()
            .borrow_mut()
    }

    pub fn set_ssd(&self, ssd: bool) {
        self.decoration().enabled = ssd;
    }

    pub fn has_ssd(&self) -> bool {
        self.decoration().enabled
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn long_titles_are_cut() {
        set_title_chars(5);
        assert_eq!(cut("abcde"), "abcde");
        assert_eq!(cut("abcdef"), "abcd…");
        assert_eq!(cut("ab  défg"), "ab…");
        set_title_chars(100);
    }
}

#[cfg(test)]
mod line_tests {
    use super::*;

    /// The frame's bands (outline, light, middle, shadow) in device pixels
    /// at every scale Preferences offers: whole, the same on every side,
    /// the middle never gone, and adding up to the border the content's
    /// offset leaves.
    #[test]
    fn frame_bands_at_every_scale() {
        let bands = |s: f64| {
            let (outline, bevel) = lines(s);
            let border = (BORDER as f64 * s).round() as i32;
            [outline, bevel, border - outline - 2 * bevel, bevel]
        };
        assert_eq!(bands(1.0), [1, 1, 2, 1]);
        assert_eq!(bands(1.25), [1, 1, 3, 1]);
        assert_eq!(bands(1.5), [1, 2, 3, 2]);
        assert_eq!(bands(1.75), [1, 2, 4, 2]);
        assert_eq!(bands(2.0), [2, 2, 4, 2]);
        assert_eq!(bands(2.5), [2, 3, 5, 3]);
        assert_eq!(bands(3.0), [3, 3, 6, 3]);
    }
}
