//! The BeOS window decorator: a yellow tab sized to the title, close box on
//! the left, zoom box on the right, and a five-line 3D frame.
//!
//! Everything is laid out in logical pixels relative to the element origin
//! (the frame's top-left corner, tab row included) and drawn directly in
//! physical pixels so lines stay crisp at any output scale.

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
use crate::{text, theme};

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
    pub last_tab_press: Option<(Instant, Point<f64, Logical>)>,
    /// Rolled up: only the tab shows (and takes the pointer).
    pub rolled: bool,
    /// Its tab's width when the window is in a group (each window's tab
    /// gets its place along the shared top, `groups`); else as wide as its
    /// title wants.
    pub tab_width: Option<i32>,
    title_width: Option<(String, i32)>,
    tab: Option<(TabKey, MemoryRenderBuffer)>,
    frame_ids: Vec<Id>,
    frame_commit: CommitCounter,
    frame_key: Option<(bool, u64)>,
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
}

pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

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
            last_tab_press: None,
            rolled: false,
            tab_width: None,
            title_width: None,
            tab: None,
            frame_ids: (0..20).map(|_| Id::new()).collect(),
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
        let width = wanted.min(frame_w).max(1);
        let x = self.tab_offset.clamp(0, (frame_w - width).max(0));
        TabLayout { x, width }
    }

    /// Classify an element-local point.
    pub fn hit(&mut self, title: &str, content: Size<i32, Logical>, p: Point<f64, Logical>) -> Part {
        let tab = self.tab_layout(title, content);
        let (w, h) = (content.w + 2 * BORDER, content.h + TAB_HEIGHT + 2 * BORDER);
        let contains = |r: Rectangle<i32, Logical>| r.to_f64().contains(p);
        if self.rolled && p.y >= TAB_HEIGHT as f64 {
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
    pub fn render_elements<R>(
        &mut self,
        renderer: &mut R,
        title: &str,
        content: Size<i32, Logical>,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> (Vec<MemoryRenderBufferRenderElement<R>>, Vec<SolidColorRenderElement>)
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let palette = theme::palette();
        let generation = theme::generation();
        let s = scale.x;
        let px = |l: i32| (l as f64 * s).round() as i32;
        let tab = self.tab_layout(title, content);

        // Tab: rendered to a pixmap at the exact physical size it covers.
        let phys = (px(tab.x + tab.width) - px(tab.x), px(TAB_HEIGHT));
        let key = TabKey {
            title: cut(title).into_owned(),
            focused: self.focused,
            hover: self.hover,
            pressed: self.pressed,
            width: tab.width,
            phys,
            generation,
        };
        if self.tab.as_ref().is_none_or(|(k, _)| *k != key) && phys.0 > 0 && phys.1 > 0 {
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
            self.tab = Some((key, buffer));
        }
        let mut tabs = Vec::new();
        if let Some((key, buffer)) = &self.tab {
            let loc = location + Point::<i32, Physical>::from((px(tab.x), 0));
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc.to_f64(),
                buffer,
                Some(alpha),
                Some(Rectangle::from_size((key.phys.0 as f64, key.phys.1 as f64).into())),
                Some((tab.width, TAB_HEIGHT).into()),
                Kind::Unspecified,
            ) {
                tabs.push(elem);
            }
        }

        // Frame: five concentric bevel lines, each drawn as four rects.
        let fkey = (self.focused, generation);
        if self.frame_key != Some(fkey) {
            self.frame_key = Some(fkey);
            self.frame_commit.increment();
        }
        let f = if self.focused { &palette.frame_focused } else { &palette.frame_unfocused };
        let (w, h) = (content.w + 2 * BORDER, content.h + TAB_HEIGHT + 2 * BORDER);
        let (x0, y0) = (location.x, location.y);
        let rect = |l: i32, t: i32, r: i32, b: i32| -> Rectangle<i32, Physical> {
            Rectangle::new((x0 + px(l), y0 + px(t)).into(), (px(r) - px(l), px(b) - px(t)).into())
        };
        let mut frame = Vec::with_capacity(20);
        for i in 0..BORDER {
            let (tl, br) = match i {
                0 => (f[0], f[0]),
                1 => (f[1], f[4]),
                4 => (f[4], f[1]),
                _ => (f[2], f[2]),
            };
            let (l, t, r, b) = (i, TAB_HEIGHT + i, w - i, h - i);
            let sides = [
                (rect(l, t, r, t + 1), tl),         // top
                (rect(l, t + 1, l + 1, b), tl),     // left
                (rect(r - 1, t + 1, r, b), br),     // right
                (rect(l + 1, b - 1, r - 1, b), br), // bottom
            ];
            for (j, (geo, color)) in sides.into_iter().enumerate() {
                if geo.size.w <= 0 || geo.size.h <= 0 {
                    continue;
                }
                let c = theme::to_f32(color);
                frame.push(SolidColorRenderElement::new(
                    self.frame_ids[(i * 4) as usize + j].clone(),
                    geo,
                    self.frame_commit,
                    [c[0] * alpha, c[1] * alpha, c[2] * alpha, c[3] * alpha],
                    Kind::Unspecified,
                ));
            }
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

fn draw_tab(palette: &Palette, key: &TabKey, tab: &TabLayout, s: f64) -> Pixmap {
    let (w, h) = key.phys;
    let mut pixmap = Pixmap::new(w as u32, h as u32).expect("tab pixmap");
    // Positions inside the tab, in physical pixels relative to its left edge.
    let px = |l: i32| ((tab.x + l) as f64 * s).round() as i32 - (tab.x as f64 * s).round() as i32;
    let py = |l: i32| (l as f64 * s).round() as i32;
    let u = (s.round() as i32).max(1);

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

    fill(&mut pixmap, 0, 0, w, h, base);
    // Outline on three sides; the frame's top line closes the bottom.
    fill(&mut pixmap, 0, 0, w, u, outline);
    fill(&mut pixmap, 0, 0, u, h, outline);
    fill(&mut pixmap, w - u, 0, u, h, outline);
    // Bevel.
    fill(&mut pixmap, u, u, w - 2 * u, u, light);
    fill(&mut pixmap, u, u, u, h - u, light);
    fill(&mut pixmap, w - 2 * u, u, u, h - u, shadow);

    let local = |r: Rectangle<i32, Logical>| (px(r.loc.x - tab.x), py(r.loc.y), px(r.loc.x - tab.x + r.size.w) - px(r.loc.x - tab.x));
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
    let (cx, cy, cs) = local(tab.close_rect());
    let (from, to) = box_colors(Part::Close);
    blended_box(&mut pixmap, cx, cy, cs, u, from, to, box_outline);

    // Zoom box: a small square overlapping a larger one behind it.
    let (zx, zy, zs) = local(tab.zoom_rect());
    let (from, to) = box_colors(Part::Zoom);
    let big = (zs as f64 * 0.69).round() as i32;
    let small = (zs as f64 * 0.54).round() as i32;
    blended_box(&mut pixmap, zx + zs - big, zy + zs - big, big, u, from, to, box_outline);
    blended_box(&mut pixmap, zx, zy, small, u, from, to, box_outline);

    // Title.
    let space = tab.title_space();
    if space > 4 {
        let title = text::ellipsize(&key.title, fonts::TAB_SIZE, true, space as f32);
        let size = fonts::TAB_SIZE * s as f32;
        let line = size * 1.25;
        let tx = px(BOX_INSET + BOX_SIZE + TITLE_GAP) as f32;
        let ty = ((h as f32 - line) / 2.0).round() + (s as f32 * 0.5).round();
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
