//! Windows that snap into place as they're dragged (Ben, 2026-11-18:
//! "if i drag to different areas of the screen it takes that tile
//! location while dragging but isn't "saved" until i let go … show each
//! hotspot as a semi-transparent overlay with a letter in each. the user
//! can drag to that hotspot and let go, or they can just press the letter
//! and it'll be like they dragged it there"). The study of how others do
//! it, and why Olo's is as it is: docs/reports/window-snapping.md.
//!
//! While a window is moved by its tab (by the pointer, or a finger),
//! each place it can be let go into has a pad on the display the pointer
//! is on: a small plate with a picture of the place and a letter on a key
//! cap as keyboard hints draw theirs. Nine places as a map of the screen
//! on the nine keys at a keyboard's left (Q W E, A S D, Z X C: the
//! quarters at the corners, the halves at the sides and at the top and
//! bottom in the middle, the whole screen along the very top), and one
//! at the middle of each other window there (its place: the two swap).
//! With the pointer in a pad the window itself is in that place, gliding
//! there (`glide`), not an outline of it, and the other pads step back
//! to their letters; out of it again, it's under the pointer at its own
//! size. Let go in a pad it stays, tiled there as a tiling command would
//! have it; let go elsewhere it's where it was let go; Escape puts
//! everything back; a letter pressed is the window let go in that
//! letter's pad.
//!
//! Not for a tile of an auto-tiled display's tree (dragging one swaps it
//! or puts it beside another, as it did), nor for what never tiles
//! (dialogs, alerts, windows of one size).
//!
//! Nothing here runs unless a window is being moved: the pads are worked
//! out as the move begins (and when the pointer changes display), a
//! pointer motion looks through them (a dozen rectangles), their
//! pictures are made as they change (not in a frame), and they're drawn
//! only while they're up.

use std::collections::HashMap;

use olo_protocol::shortcuts::Tile;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::{IsAlive as _, Logical, Physical, Point, Rectangle, Size, Transform},
};
use tiny_skia::Pixmap;

use super::{
    WindowElement,
    tiling::Part,
    wm::{PreZoom, ZoomedFrom},
};
use crate::{
    render::Exact,
    state::{AnvilState, Backend},
    theme,
};

type Rect = Rectangle<i32, Logical>;

/// A place a dragged window can be let go into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// A half or a quarter of the display.
    Tile(Tile),
    /// The whole of it (zoomed).
    Whole,
    /// Another window's (by its id): the two swap.
    Window(u64),
}

impl Place {
    /// What agents read of it.
    pub fn name(self) -> String {
        match self {
            Place::Tile(t) => format!("{t:?}"),
            Place::Whole => "Whole".into(),
            Place::Window(id) => format!("Window {id}"),
        }
    }
}

/// A place's pad: where it's drawn, and where the pointer counts as in
/// it (the pad, and from it out to the display's edge where it's at one:
/// the pointer stops at the screen's edge, past the windows' margin and
/// whatever Deskbar takes).
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub place: Place,
    pub rect: Rect,
    pub hot: Rect,
    pub letter: char,
}

/// A pad at a corner or at the top or bottom in the middle (points); one
/// at a side stands the other way. Another window's is wider and taller,
/// for the window's title over its letter.
const PAD: (i32, i32) = (112, 72);
const WINDOW_PAD: (i32, i32) = (128, 90);
/// The whole screen's bar along the very top: how tall (its key cap and
/// four points over and under it: at 20 the cap's bevel and the bar's
/// read as one frame, Astra's review).
const BAR: i32 = 24;
/// Between pads at the least.
const GAP: i32 = 8;

/// The nine places' letters, as their keys lie on a QWERTY keyboard (as
/// keyboard hints' letters are keys' places, `hints::letter`).
const PLACES: [(Place, char); 9] = [
    (Place::Tile(Tile::TopLeft), 'q'),
    (Place::Tile(Tile::TopRight), 'e'),
    (Place::Tile(Tile::BottomLeft), 'z'),
    (Place::Tile(Tile::BottomRight), 'c'),
    (Place::Tile(Tile::Left), 'a'),
    (Place::Tile(Tile::Right), 'd'),
    (Place::Whole, 's'),
    (Place::Tile(Tile::Top), 'w'),
    (Place::Tile(Tile::Bottom), 'x'),
];
/// The other windows' letters: the right hand's, its home row first.
const WINDOWS: &str = "jklhuiopynmbgtrfv";

/// The pads of a display: `screen` is all of it, `area` what windows
/// tile into, `others` the other windows there that can swap places (the
/// front one first) with their frames. In the order they're looked
/// through: the corners, the sides, the top's bar, the top and bottom in
/// the middle, then the windows'. One that would lie over an earlier one
/// (a small display, a window under a pad) isn't made.
pub fn pads(screen: Rect, area: Rect, others: &[(u64, Rect)]) -> Vec<Pad> {
    let (l, t, r, b) = (area.loc.x, area.loc.y, area.loc.x + area.size.w, area.loc.y + area.size.h);
    let (cx, cy) = (l + area.size.w / 2, t + area.size.h / 2);
    let (w, h) = PAD;
    let at = |x: i32, y: i32, w: i32, h: i32| Rect::new((x, y).into(), (w, h).into());
    let rect_of = |place: Place| match place {
        Place::Tile(Tile::TopLeft) => at(l, t, w, h),
        Place::Tile(Tile::TopRight) => at(r - w, t, w, h),
        Place::Tile(Tile::BottomLeft) => at(l, b - h, w, h),
        Place::Tile(Tile::BottomRight) => at(r - w, b - h, w, h),
        Place::Tile(Tile::Left) => at(l, cy - w / 2, h, w),
        Place::Tile(Tile::Right) => at(r - h, cy - w / 2, h, w),
        Place::Whole => at(l + w + GAP, t, area.size.w - 2 * (w + GAP), BAR),
        Place::Tile(Tile::Top) => at(cx - w / 2, t + BAR + GAP, w, h),
        Place::Tile(Tile::Bottom) => at(cx - w / 2, b - h, w, h),
        _ => Rect::default(),
    };
    // (Out to the display's edge on each side it's at the area's.)
    let hot = |rect: Rect| {
        let (mut x0, mut y0, mut x1, mut y1) = (rect.loc.x, rect.loc.y, rect.loc.x + rect.size.w, rect.loc.y + rect.size.h);
        if x0 <= l {
            x0 = x0.min(screen.loc.x);
        }
        if y0 <= t {
            y0 = y0.min(screen.loc.y);
        }
        if x1 >= r {
            x1 = x1.max(screen.loc.x + screen.size.w);
        }
        if y1 >= b {
            y1 = y1.max(screen.loc.y + screen.size.h);
        }
        at(x0, y0, x1 - x0, y1 - y0)
    };
    let apart = |a: &Rect, b: &Rect| {
        let grown = at(a.loc.x - GAP, a.loc.y - GAP, a.size.w + 2 * GAP, a.size.h + 2 * GAP);
        !grown.overlaps(*b)
    };
    let mut out: Vec<Pad> = Vec::with_capacity(PLACES.len() + others.len());
    for (place, letter) in PLACES {
        let rect = rect_of(place);
        if rect.size.w >= BAR && rect.size.h >= BAR && out.iter().all(|p| apart(&p.rect, &rect)) {
            out.push(Pad { place, rect, hot: hot(rect), letter });
        }
    }
    let mut letters = WINDOWS.chars();
    for &(id, frame) in others {
        // At the middle of what of it is on the display's area.
        let Some(seen) = frame.intersection(area) else { continue };
        let (w, h) = WINDOW_PAD;
        let rect = at(seen.loc.x + (seen.size.w - w) / 2, seen.loc.y + (seen.size.h - h) / 2, w, h);
        if seen.size.w < w || seen.size.h < h || !out.iter().all(|p| apart(&p.rect, &rect)) {
            continue;
        }
        let Some(letter) = letters.next() else { break };
        out.push(Pad { place: Place::Window(id), rect, hot: rect, letter });
    }
    out
}

/// The pad `p` is in, if any.
pub fn pad_at(pads: &[Pad], p: Point<f64, Logical>) -> Option<usize> {
    pads.iter().position(|pad| pad.hot.to_f64().contains(p))
}

/// How a window is arranged: what Escape gives a moved one back, and
/// what a swap gives each of the two of the other's.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Arranged {
    frame: Rect,
    tile: Option<Tile>,
    /// The part its tile's edges were dragged to.
    part: Option<Part>,
    /// Zoomed, and where it unzooms to (a tile's frame; none: its own).
    zoomed: bool,
    zoomed_from: Option<Rect>,
}

/// A window being moved, and its places.
#[derive(Debug)]
pub struct Snap {
    pub window: WindowElement,
    output: Output,
    pads: Vec<Pad>,
    /// The other windows with a pad, as they were when the pads were made.
    others: Vec<(u64, Arranged)>,
    /// The pad the window is shown in: the pointer's, or a letter's.
    at: Option<usize>,
    /// The pad the move began in, till the pointer has left it: not a
    /// place the window was dragged to. (A window at the top of the
    /// screen is taken by its tab inside the whole screen's bar, and was
    /// the whole screen the moment it was taken: `tools/dev/snap-check`.)
    began_in: Option<usize>,
    /// The other window shown where this one was, while this one is shown
    /// in its place.
    swapped: Option<u64>,
    /// Its own size, which it has out of every pad.
    own: Size<i32, Logical>,
    /// How it was as the move began, and its own frame then (what Restore
    /// took it to).
    before: Arranged,
    pre: Option<Rect>,
    /// The display's scale and the look the pads were last drawn for.
    shown_for: (f64, u64),
}

/// The other window of a swap glows for as long as it's shown swapped
/// (`glow::strength` is whole until a second before its time is up), and
/// fades over that second once it isn't.
const GLOW_HELD: std::time::Duration = std::time::Duration::from_secs(86_400);
const GLOW_FADE: std::time::Duration = std::time::Duration::from_secs(1);

/// How a move ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Let go (or a letter pressed).
    Dropped,
    /// Escape: as it was.
    Cancelled,
}

/// A pad as a display shows it: where (the display's device pixels), and
/// its picture.
struct Shown {
    at: Point<i32, Physical>,
    size: (i32, i32),
    picture: MemoryRenderBuffer,
}

/// What a display shows of them, made as the pads change (`snap_show`):
/// a frame only places the pictures.
#[derive(Default)]
struct Slot(olo_sync::Mutex<Vec<Shown>>);

fn slot(output: &Output) -> &Slot {
    output.user_data().insert_if_missing_threadsafe(Slot::default);
    output.user_data().get::<Slot>().unwrap()
}

/// How a pad is drawn: at rest, with the dragged window in it, or
/// stepped back while the window is in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Look {
    Rest,
    In,
    Back,
}

/// What a pad's picture is of: its place's kind and, for a window's, the
/// window's title (none: one stepped back, which shows none); its
/// letter, its size in device pixels, whether it's a window's, and its
/// look.
type PadKey = (Option<Tile>, Option<String>, char, (i32, i32), bool, Look);

/// The pads' pictures, each made once for a scale and a look: the
/// picture, where it goes in its pad, and its size (device pixels).
#[derive(Default)]
struct Drawn {
    key: (i64, u64),
    pads: HashMap<PadKey, (MemoryRenderBuffer, (i32, i32), (i32, i32))>,
}

static DRAWN: olo_sync::Mutex<Option<Drawn>> = olo_sync::Mutex::new(None);

/// How much of a plate hides what's behind it (out of 255): all of it.
/// Ben asked for see-through hotspots ("a semi-transparent overlay with
/// a letter in each"), and so they were first: but a pad is a small
/// plate over whatever is there, a window's words more often than not,
/// and at 80%, at 92% and at 95% those words still read through it as
/// if they were the pad's own (Astra's and Opus's reviews, three times
/// over, 2026-11-20). What's see-through now is the screen itself: with
/// the window in a place, every other pad is no more than its letter.
/// (Lower this to try them see-through again.)
const PAD_ALPHA: u8 = 255;
/// A plate's shadow, a point down and right of it (out of 255).
const SHADOW: u8 = 76;
/// A pad's small picture of the display, in points; and between it and
/// the letter's key cap.
const SCREEN: (f64, f64) = (44.0, 30.0);
const SCREEN_SMALL: (f64, f64) = (22.0, 14.0);
const BETWEEN: f64 = 6.0;
/// A window's pad says whose it is, between its picture and its key cap:
/// the title's size, the line's height, the room over and under it (the
/// words as far from the picture as from the cap: eight points each, by
/// eye and by Astra's measure), and what's kept at each side of it.
const TITLE: f32 = 11.0;
const TITLE_LINE: f64 = 14.0;
const TITLE_OVER: f64 = 5.0;
const TITLE_UNDER: f64 = 5.0;
const TITLE_INSET: f64 = 8.0;
/// The small window's tab: how much of the window's width, and how tall
/// (its top line and the window's among them).
const SMALL_TAB: (f64, f64) = (0.45, 8.0);
/// A stepped-back pad's key cap has this much plate round it.
const COLLAR: f64 = 3.0;

/// One pad's picture at scale `s`: `size` device pixels and its shadow's
/// (a point more each way). A plate, raised as Olo's panels are, with
/// the highlight's edge; on it a small picture of the display, sunken as
/// a well is, with the place filled in (another window's: a small
/// window, raised as one is, and under it that window's `title`), and
/// the letter's key cap as keyboard hints draw theirs. The picture over
/// the cap; side by side on the top's bar, which is too low for both.
/// With the dragged window in it (`Look::In`): the selection's ground,
/// an edge twice as wide, the cap pressed in. Stepped back
/// (`Look::Back`): its key cap where it was, on a collar of plate, and
/// nothing else (faded whole, a pad over a window's words was a smudge;
/// as an outline, a box struck through them; the cap alone, a letter
/// among them: Opus's and Astra's reviews).
///
/// Answers the picture and where its top left is in the pad (a stepped
/// back one is no bigger than its collar: eleven pad-sized pictures,
/// nearly all of them nothing, were 5 ms to hand the GPU in the frame
/// the first place was shown).
fn pad_pixmap(p: &olo_theme::Palette, place: Place, title: Option<&str>, letter: char, size: (i32, i32), look: Look, s: f64) -> Option<(Pixmap, (i32, i32))> {
    let (w, h) = size;
    let u = (s.floor() as i32).max(1);
    let px = |l: f64| (l * s).round() as i32;
    let rect = |pm: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, c: [u8; 4], a: u8| {
        let Some(r) = tiny_skia::Rect::from_xywh(x as f32, y as f32, w.max(0) as f32, h.max(0) as f32) else { return };
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(c[0], c[1], c[2], a);
        paint.anti_alias = false;
        paint.blend_mode = tiny_skia::BlendMode::Source;
        pm.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
    };
    // (Top and left in one colour, bottom and right in another: a bevel
    // a point wide just inside the rectangle.)
    let bevel = |pm: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, tl: [u8; 4], br: [u8; 4], a: u8| {
        rect(pm, x, y, w, u, tl, a);
        rect(pm, x, y + u, u, h - u, tl, a);
        rect(pm, x + u, y + h - u, w - u, u, br, a);
        rect(pm, x + w - u, y + u, u, h - 2 * u, br, a);
    };
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let window = matches!(place, Place::Window(_));
    // A raised edge's light and shade on `ground`: by day the palette's
    // shine and a tint darker; by night, where the shine is as dark as
    // the panels and a plate was a flat card (Opus's review), a step
    // towards white and a deeper one towards black.
    let lit = |ground: [u8; 4]| if dark { olo_theme::mix(ground, [255, 255, 255, 255], 0.2) } else { olo_theme::mix(ground, p.shine, 0.8) };
    let shaded = |ground: [u8; 4]| if dark { olo_theme::mix(ground, [0, 0, 0, 255], 0.4) } else { p.shade(ground, 1.2) };
    let edge_ink = if dark { olo_theme::mix(p.control_highlight, [255, 255, 255, 255], 0.15) } else { p.control_highlight };
    // The plate: the panel's ground (by night a step lighter, to stand
    // off a window's own panels), the highlight's edge a point wide;
    // with the window in it, the selection's ground and an edge twice as
    // wide.
    let panel = if dark { olo_theme::tint(p.panel_background, 0.9) } else { p.panel_background };
    let (ground, edge) = if look == Look::In { (p.list_selected_background, 2 * u) } else { (panel, u) };
    let cap = crate::hints::key_cap(p, letter, s, look == Look::In);
    let (cw, ch) = (cap.width() as i32, cap.height() as i32);
    let low = h < px(SCREEN.1 + BETWEEN) + ch + 2 * u;
    let (sw, sh) = if low { (px(SCREEN_SMALL.0), px(SCREEN_SMALL.1)) } else { (px(SCREEN.0), px(SCREEN.1)) };
    let between = px(BETWEEN);
    // (A window's pad: its title's line between the two, in place of
    // the room between them.)
    let named = if window && !low { px(TITLE_OVER) + px(TITLE_LINE) + px(TITLE_UNDER) - between } else { 0 };
    // Where each goes: one over the other; in a low pad side by side, the
    // cap in the pad's middle as every other is (so it's where the eye
    // left it when the plate steps back, and over W's: Astra's review)
    // and the picture at its left.
    let ((sx, sy), (cx, cy)) = if low {
        let x = (w - cw) / 2;
        ((x - between - sw, (h - sh) / 2), (x, (h - ch) / 2))
    } else {
        let y = (h - (sh + named + between + ch)) / 2;
        (((w - sw) / 2, y), ((w - cw) / 2, y + sh + named + between))
    };
    // Stepped back: the cap on a collar of its own, a small plate with a
    // dark edge (not the highlight's: it's no place now, only a key), so
    // it's a thing on the window under it and not a letter among that
    // window's words.
    if look == Look::Back {
        let c = px(COLLAR);
        let (kw, kh) = (cw + 2 * c, ch + 2 * c);
        let mut pm = Pixmap::new(kw.max(1) as u32, kh.max(1) as u32)?;
        rect(&mut pm, 0, 0, kw, kh, p.shade(panel, 1.55), 255);
        rect(&mut pm, u, u, kw - 2 * u, kh - 2 * u, panel, 255);
        bevel(&mut pm, u, u, kw - 2 * u, kh - 2 * u, lit(panel), shaded(panel), 255);
        pm.draw_pixmap(c, c, cap.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        return Some((pm, (cx - c, cy - c)));
    }
    let mut pm = Pixmap::new((w + u).max(1) as u32, (h + u).max(1) as u32)?;
    rect(&mut pm, u, u, w, h, [0, 0, 0, 255], SHADOW);
    rect(&mut pm, 0, 0, w, h, edge_ink, 255);
    rect(&mut pm, edge, edge, w - 2 * edge, h - 2 * edge, ground, PAD_ALPHA);
    bevel(&mut pm, edge, edge, w - 2 * edge, h - 2 * edge, lit(ground), shaded(ground), PAD_ALPHA);
    pm.draw_pixmap(cx, cy, cap.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    match place {
        // A small window, where the moved one would go: raised, as a
        // window is on a screen (not a well: that's a screen), its tab
        // and its body one outline, the tab's close box in it (what
        // makes it a window of Olo's and not a folder: Opus's review),
        // the body filled as a place is behind a line of white.
        Place::Window(_) => {
            let ink = p.shade(ground, 1.55);
            let (tw, th) = ((SMALL_TAB.0 * sw as f64).round() as i32, px(SMALL_TAB.1));
            let (by, bh) = (sy + th - u, sh - th + u);
            // (The line of light inside its outline: the paper's white,
            // or by night, where paper is dark, the highlight lightened.)
            let light = if dark { olo_theme::mix(p.control_highlight, [255, 255, 255, 255], 0.5) } else { p.document_background };
            rect(&mut pm, sx, by, sw, bh, ink, 255);
            rect(&mut pm, sx + u, by + u, sw - 2 * u, bh - 2 * u, light, 255);
            rect(&mut pm, sx + 2 * u, by + 2 * u, sw - 4 * u, bh - 4 * u, p.control_highlight, 255);
            rect(&mut pm, sx, sy, tw, th, ink, 255);
            rect(&mut pm, sx + u, sy + u, tw - 2 * u, th - 2 * u, p.tab_focused, 255);
            // (Its close box: hollow, as a tab's is, a device pixel in
            // from the tab's edges all round.)
            let close = th - 2 * u - 2;
            rect(&mut pm, sx + u + 1, sy + u + 1, close, close, ink, 255);
            rect(&mut pm, sx + 2 * u + 1, sy + 2 * u + 1, close - 2 * u, close - 2 * u, p.tab_focused_light, 255);
        }
        // The display, small, in a well sunken as the kit's are: the
        // screen's white, the place filled in the highlight, a line of
        // the white left round the filling (at an edge too: it's a part
        // of a screen, not a block).
        _ => {
            let (fx, fy, fw, fh) = match place {
                Place::Tile(tile) => tile.fraction(),
                _ => (0.0, 0.0, 1.0, 1.0),
            };
            bevel(&mut pm, sx, sy, sw, sh, p.shade(ground, 1.147), lit(ground), 255);
            bevel(&mut pm, sx + u, sy + u, sw - 2 * u, sh - 2 * u, p.shade(ground, 1.55), p.shade(ground, 1.06), 255);
            let (ix, iy, iw, ih) = (sx + 2 * u, sy + 2 * u, sw - 4 * u, sh - 4 * u);
            rect(&mut pm, ix, iy, iw, ih, p.document_background, 255);
            let (x0, y0) = ((fx * iw as f32).round() as i32 + u, (fy * ih as f32).round() as i32 + u);
            let (x1, y1) = (((fx + fw) * iw as f32).round() as i32 - u, ((fy + fh) * ih as f32).round() as i32 - u);
            rect(&mut pm, ix + x0, iy + y0, x1 - x0, y1 - y0, p.control_highlight, 255);
        }
    }
    // Whose place: the window's title, cut short to the pad.
    if let Some(title) = title.filter(|_| !low) {
        let font = TITLE * s as f32;
        let room = (w - 2 * px(TITLE_INSET)) as f32;
        let title = crate::text::ellipsize(title, font, false, room);
        let tw = crate::text::measure(&title, font, false).ceil() as i32;
        let line = sy + sh + px(TITLE_OVER);
        let y = crate::text::top_for_centre(line as f32 + px(TITLE_LINE) as f32 / 2.0, font, false);
        crate::text::draw(&mut pm, &title, ((w - tw) / 2) as f32, y, font, false, p.panel_text);
    }
    Some((pm, (0, 0)))
}

/// The pads on `output`, while a window is moved there.
pub fn elements<R>(renderer: &mut R, output: &Output) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let shown = slot(output).0.lock().unwrap();
    let mut out = Vec::with_capacity(shown.len());
    for pad in shown.iter() {
        out.extend(crate::render::picture(renderer, pad.at.to_f64(), &pad.picture, pad.size, None, Kind::Unspecified));
    }
    out
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Whether a window snaps as it's dragged: one that tiles, not a tile
    /// of an auto-tiled display's tree.
    fn snaps(&self, window: &WindowElement) -> bool {
        self.settings.tiling.snap && !self.auto_tiled(window) && !Self::floats_always(window) && !window.meta().fullscreen.get() && !window.meta().pip.get() && super::tiling::resizable(window)
    }

    /// How `window` is arranged now (where it's going, if it's on its way).
    fn snap_arranged(&self, window: &WindowElement) -> Option<Arranged> {
        let goal = self.glide_goal(window);
        Some(Arranged {
            frame: goal.map_or_else(|| self.space.element_geometry(window), |(to, _)| Some(to))?,
            tile: Self::tiled(window),
            part: Self::custom_part(window),
            zoomed: self.is_zoomed(window) || goal.is_some_and(|(_, zoom)| zoom),
            zoomed_from: window.user_data().get::<ZoomedFrom>().and_then(|z| *z.0.borrow()),
        })
    }

    /// `window` arranged as `a` says, gliding there.
    fn snap_arrange(&mut self, window: &WindowElement, a: &Arranged) {
        Self::set_tiled(window, a.tile);
        Self::set_part(window, a.part);
        window.user_data().insert_if_missing(ZoomedFrom::default);
        *window.user_data().get::<ZoomedFrom>().unwrap().0.borrow_mut() = a.zoomed_from;
        self.snap_put(window, a.frame);
        if a.zoomed {
            self.glide_then_zoom(window);
        }
    }

    /// `window`'s own frame, which Restore takes it to, is `own` (none:
    /// where it is).
    fn set_own_frame(window: &WindowElement, own: Option<Rect>) {
        window.user_data().insert_if_missing(PreZoom::default);
        *window.user_data().get::<PreZoom>().unwrap().0.borrow_mut() = own;
    }

    /// The pads for `window` moved on `output`, and the other windows
    /// that have one.
    fn snap_pads(&self, window: &WindowElement, output: &Output) -> (Vec<Pad>, Vec<(u64, Arranged)>) {
        let (Some(screen), Some(area)) = (self.space.output_geometry(output), self.tile_area(output)) else { return Default::default() };
        // (Another window of its own app is told it's over it, to take
        // it as a tab: `window_drop`. Not a place to swap with.)
        let client = window.meta().owner.borrow().as_ref().map(|(_, c)| c.clone());
        let others: Vec<(u64, Arranged)> = self
            .space
            .elements()
            .rev()
            .filter(|w| {
                *w != window
                    && w.alive()
                    && !w.meta().covered.get()
                    && !w.meta().fullscreen.get()
                    && !Self::floats_always(w)
                    && !self.auto_tiled(w)
                    && super::tiling::resizable(w)
                    && (client.is_none() || w.meta().owner.borrow().as_ref().map(|(_, c)| c.clone()) != client)
            })
            .filter_map(|w| Some((w.id(), self.snap_arranged(w)?)))
            .filter(|(_, a)| a.frame.overlaps(area))
            .collect();
        let frames: Vec<(u64, Rect)> = others.iter().map(|(id, a)| (*id, a.frame)).collect();
        (pads(screen, area, &frames), others)
    }

    /// What the displays show of the pads, made again (they changed, or
    /// which the window is in did): each pad's picture, from those made
    /// already for this scale and look, and where it goes in the display's
    /// own pixels. With the window in one, the others step back.
    fn snap_show(&mut self) {
        let palette = theme::palette();
        for output in self.space.outputs() {
            let mut shown = slot(output).0.lock().unwrap();
            shown.clear();
            let Some(snap) = self.snap.as_ref().filter(|s| s.output == *output) else { continue };
            let Some(geo) = self.space.output_geometry(output) else { continue };
            let scale = output.current_scale().fractional_scale();
            let key = ((scale * 1000.0).round() as i64, theme::generation());
            let mut drawn = DRAWN.lock().unwrap();
            let drawn = drawn.get_or_insert_with(Drawn::default);
            if drawn.key != key {
                *drawn = Drawn { key, ..Default::default() };
            }
            let device = |v: i32| (v as f64 * scale).round() as i32;
            for (i, pad) in snap.pads.iter().enumerate() {
                let local = Rect::new(pad.rect.loc - geo.loc, pad.rect.size);
                let at: Point<i32, Physical> = (device(local.loc.x), device(local.loc.y)).into();
                let size = (device(local.loc.x + local.size.w) - at.x, device(local.loc.y + local.size.h) - at.y);
                let look = match snap.at {
                    Some(at) if at == i => Look::In,
                    Some(_) => Look::Back,
                    None => Look::Rest,
                };
                let (tile, title) = match pad.place {
                    Place::Tile(tile) => (Some(tile), None),
                    Place::Whole => (None, None),
                    Place::Window(_) if look == Look::Back => (None, None),
                    Place::Window(id) => (None, Some(self.window_by_id(id).map(|w| w.title()).unwrap_or_default())),
                };
                let pad_key = (tile, title, pad.letter, size, matches!(pad.place, Place::Window(_)), look);
                if !drawn.pads.contains_key(&pad_key) {
                    let Some((pm, offset)) = pad_pixmap(&palette, pad.place, pad_key.1.as_deref(), pad.letter, size, look, scale) else { continue };
                    let whole = (pm.width() as i32, pm.height() as i32);
                    drawn.pads.insert(pad_key.clone(), (MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, whole, 1, Transform::Normal, None), offset, whole));
                }
                let (picture, offset, whole) = &drawn.pads[&pad_key];
                shown.push(Shown { at: (at.x + offset.0, at.y + offset.1).into(), size: *whole, picture: picture.clone() });
            }
        }
        if let Some(snap) = self.snap.as_mut() {
            snap.shown_for = (snap.output.current_scale().fractional_scale(), theme::generation());
        }
        self.backend_data.wake(None);
    }

    /// Before a frame: the pads made again if the display's scale or the
    /// look changed while they're up; and gone, everything as it was, if
    /// the window being moved has closed or nothing holds it any more
    /// (pads left up with no move to end them would stay for good).
    pub fn snap_tick(&mut self) {
        let Some(snap) = self.snap.as_ref() else { return };
        let held = self.pointer.is_grabbed() || self.seat.get_touch().is_some_and(|t| t.is_grabbed());
        if !snap.window.alive() || !held {
            let window = snap.window.clone();
            self.snap_end(&window, End::Cancelled);
            return;
        }
        if snap.shown_for != (snap.output.current_scale().fractional_scale(), theme::generation()) {
            self.snap_show();
        }
    }

    /// `window`'s move began (pressed at `pressed`, the pointer now at
    /// `pointer`, past a click's distance) with the window at `from`: its
    /// pads come up. Out of a tile or a zoom it's its own size again, its
    /// tab still under the pointer. Answers where the window would have
    /// been at the press at that size, which the move goes on from; none
    /// if the window isn't one that snaps.
    pub fn snap_begin(&mut self, window: &WindowElement, from: Rect, pressed: Point<f64, Logical>, pointer: Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        self.snap = None;
        if !self.snaps(window) {
            return None;
        }
        let output = self.space.output_under(pointer).next().cloned()?;
        // (Taken while on its way somewhere: as it would have been there.)
        let before = self.snap_arranged(window)?;
        let pre = window.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
        let own = if before.tile.is_some() || before.zoomed { pre.map_or(from.size, |k| k.size) } else { from.size };
        let mut initial = from.loc;
        if own != from.size {
            // Its tab stays under the pointer: where it is along the
            // frame, at this width and at its own (a tab slid along is
            // held in by a narrower frame). A window with no tab of
            // Olo's: as far along its top as it was pressed.
            initial.x = if window.decorated() {
                let (title, content) = (window.title(), window.content_size());
                let mut deco = window.decoration();
                let tab = deco.tab_layout(&title, content);
                from.loc.x + tab.x - deco.tab_offset.clamp(0, (own.w - tab.width).max(0))
            } else {
                let along = ((pressed.x - from.loc.x as f64) / from.size.w.max(1) as f64).clamp(0.0, 1.0);
                (pressed.x - along * own.w as f64).round() as i32
            };
            self.snap_put(window, Rect::new(initial + (pointer - pressed).to_i32_round(), own));
        }
        let (pads, others) = self.snap_pads(window, &output);
        let began_in = pad_at(&pads, pointer);
        self.snap = Some(Snap { window: window.clone(), output, pads, others, at: None, began_in, swapped: None, own, before, pre, shown_for: (0.0, 0) });
        self.snap_show();
        Some(initial)
    }

    /// The frame `place` gives the window being moved: a part of the
    /// display as a tiling command would make it (no smaller than the
    /// window can be), all of it, or the other window's.
    fn snap_frame(&self, snap: &Snap, place: Place) -> Option<Rect> {
        let area = self.tile_area(&snap.output)?;
        match place {
            Place::Tile(tile) => Some(self.given_way(&snap.window, super::tiling::part(area, tile.fraction(), self.tile_gap()), area)),
            Place::Whole => Some(area),
            Place::Window(id) => snap.others.iter().find(|(i, _)| *i == id).map(|(_, a)| a.frame),
        }
    }

    /// The window being moved is shown in pad `at` (none: under the
    /// pointer at its own size, at `under`), and the other window of a
    /// swap where this one was.
    fn snap_enter(&mut self, at: Option<usize>, under: Point<i32, Logical>) {
        let Some(snap) = self.snap.as_ref() else { return };
        let place = at.map(|i| snap.pads[i].place);
        let frame = match place {
            Some(place) => self.snap_frame(snap, place),
            None => Some(Rect::new(under, snap.own)),
        };
        let Some(frame) = frame else { return };
        let window = snap.window.clone();
        self.snap.as_mut().unwrap().at = at;
        self.snap_put(&window, frame);
        self.snap_swap(match place {
            Some(Place::Window(id)) => Some(id),
            _ => None,
        });
        self.snap_show();
    }

    /// The other window of a swap (`with`) is shown where the moved one
    /// was; the one that was shown there before is as it was again.
    fn snap_swap(&mut self, with: Option<u64>) {
        let Some(snap) = self.snap.as_mut() else { return };
        if snap.swapped == with {
            return;
        }
        let (back, home) = (snap.swapped.take(), snap.before.frame);
        if let Some(id) = back {
            self.snap_back(id);
        }
        if let Some(id) = with
            && let Some(other) = self.window_by_id(id)
        {
            self.snap_put(&other, home);
            // It glows while it's shown there, as a new window does
            // (`glow`, at its own place among the windows): this one
            // moves too.
            other.meta().glow.set(Some((std::time::Instant::now(), GLOW_HELD)));
            self.snap.as_mut().unwrap().swapped = Some(id);
        }
    }

    /// The other window of a swap, no longer shown as that (back where it
    /// was, or there for good): its glow fades.
    fn snap_unglow(other: &WindowElement) {
        if other.meta().glow.get().is_some() {
            other.meta().glow.set(Some((std::time::Instant::now(), GLOW_FADE)));
        }
    }

    /// The other window `id`, shown where the moved one was, is as it was
    /// again.
    fn snap_back(&mut self, id: u64) {
        let Some(a) = self.snap.as_ref().and_then(|s| s.others.iter().find(|(i, _)| *i == id).map(|(_, a)| *a)) else { return };
        if let Some(other) = self.window_by_id(id) {
            self.snap_arrange(&other, &a);
            Self::snap_unglow(&other);
        }
    }

    /// The pointer moved to `pointer` with `window` being moved, which
    /// would be at `under` at its own size. True while it's in a pad:
    /// the window is in that place, and the move leaves it there.
    pub fn snap_motion(&mut self, window: &WindowElement, pointer: Point<f64, Logical>, under: Point<i32, Logical>) -> bool {
        let Some(snap) = self.snap.as_ref().filter(|s| s.window == *window) else { return false };
        // (On another display now: that one's pads.)
        let on = self.space.output_under(pointer).next().cloned();
        if let Some(output) = on
            && output != snap.output
        {
            if snap.at.is_some() {
                self.snap_enter(None, under);
            }
            let (pads, others) = self.snap_pads(window, &output);
            let snap = self.snap.as_mut().unwrap();
            (snap.output, snap.pads, snap.others, snap.began_in) = (output, pads, others, None);
            self.snap_show();
        }
        let snap = self.snap.as_mut().unwrap();
        let mut at = pad_at(&snap.pads, pointer);
        // (Still in the pad the move began in: not dragged there.)
        if snap.began_in.is_some() {
            if at == snap.began_in {
                at = None;
            } else {
                snap.began_in = None;
            }
        }
        if at != snap.at {
            self.snap_enter(at, under);
        }
        self.snap.as_ref().is_some_and(|s| s.at.is_some())
    }

    /// The move ended. True if the snap saw to where the window is now
    /// (let go in a place; or Escape, and it's as it was): false if it was
    /// let go in the open, where it stays.
    pub fn snap_end(&mut self, window: &WindowElement, how: End) -> bool {
        let Some(snap) = self.snap.take_if(|s| s.window == *window) else { return false };
        self.snap_show();
        let place = snap.at.map(|i| snap.pads[i].place);
        match (how, place) {
            // As it was: the window, and the other one of a swap shown.
            (End::Cancelled, _) => {
                if let Some((_, a)) = snap.swapped.and_then(|id| snap.others.iter().find(|(i, _)| *i == id))
                    && let Some(other) = snap.swapped.and_then(|id| self.window_by_id(id))
                {
                    self.snap_arrange(&other, a);
                    Self::snap_unglow(&other);
                }
                self.snap_arrange(window, &snap.before);
                Self::set_own_frame(window, snap.pre);
                true
            }
            // In the open: there it is. Out of a tile or a zoom, at its
            // own size, that's its own frame now.
            (End::Dropped, None) => {
                if snap.before.tile.is_some() || snap.before.zoomed {
                    self.forget_own_frame(window);
                }
                false
            }
            (End::Dropped, Some(place)) => {
                self.snap_keep(&snap, place);
                true
            }
        }
    }

    /// The window let go in `place`: it's there for good, as a tiling
    /// command would have put it, its own frame kept for Restore.
    fn snap_keep(&mut self, snap: &Snap, place: Place) {
        let (window, before) = (&snap.window, snap.before);
        tracing::info!(window = window.id(), place = %place.name(), "let go in a place: snapped there");
        // Its own frame: where it was, if it was there at its own size.
        let arranged = before.tile.is_some() || before.zoomed;
        let own = if arranged { snap.pre.unwrap_or(Rect::new(before.frame.loc, snap.own)) } else { before.frame };
        let Some(frame) = self.snap_frame(snap, place) else { return };
        let free = Arranged { frame, tile: None, part: None, zoomed: false, zoomed_from: None };
        match place {
            Place::Tile(tile) => {
                self.snap_arrange(window, &Arranged { tile: Some(tile), ..free });
                Self::set_own_frame(window, Some(own));
            }
            Place::Whole => {
                self.snap_arrange(window, &Arranged { zoomed: true, ..free });
                Self::set_own_frame(window, Some(own));
            }
            // The two swap: each as the other was. One in a tile or
            // zoomed keeps its own frame for Restore; one at a size of
            // its own is where it's put.
            Place::Window(id) => {
                let Some(theirs) = snap.others.iter().find(|(i, _)| *i == id).map(|(_, a)| *a) else { return };
                let Some(other) = self.window_by_id(id) else { return };
                let their_own = other.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow()).unwrap_or(theirs.frame);
                self.snap_arrange(&other, &before);
                Self::snap_unglow(&other);
                Self::set_own_frame(&other, arranged.then_some(their_own));
                self.snap_arrange(window, &theirs);
                Self::set_own_frame(window, (theirs.tile.is_some() || theirs.zoomed).then_some(own));
            }
        }
    }

    /// `window`'s frame is to be `frame`, gliding there.
    fn snap_put(&mut self, window: &WindowElement, frame: Rect) {
        self.glided(|s| s.set_frame(window, frame, false));
    }

    /// A key, while a window is moved: a pad's letter is the window let go
    /// in that pad (the move ends). True if the key was taken.
    pub fn snap_key(&mut self, key: u32, pressed: bool) -> bool {
        if !pressed {
            return self.snap_key_up.take_if(|k| *k == key).is_some();
        }
        let Some(snap) = self.snap.as_ref() else { return false };
        // (With ⌃, ⌥ or ⌘ held it's a shortcut's key, not a place's.)
        if self.seat.get_keyboard().map(|k| k.modifier_state()).is_some_and(|m| m.ctrl || m.alt || m.logo) {
            return false;
        }
        let Some(i) = crate::hints::letter(key).and_then(|c| snap.pads.iter().position(|p| p.letter == c)) else { return false };
        if snap.at != Some(i) {
            self.snap_enter(Some(i), Point::default());
        }
        // (Its letting up types nothing either.)
        self.snap_key_up = Some(key);
        // The move ends, as if let go there: the pointer's, or a finger's.
        let pointer = self.pointer.clone();
        if pointer.is_grabbed() {
            pointer.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), smithay::backend::input::InputTime::now());
        }
        if self.snap.is_some()
            && let Some(touch) = self.seat.get_touch()
        {
            touch.unset_grab(self);
        }
        true
    }

    /// What agents read of a move's places (`Request::Snap`): none while
    /// no window is moved.
    pub fn snap_info(&self) -> olo_protocol::SnapInfo {
        let Some(snap) = &self.snap else { return olo_protocol::SnapInfo::default() };
        olo_protocol::SnapInfo {
            window: Some(snap.window.id()),
            output: Some(snap.output.name()),
            pads: snap
                .pads
                .iter()
                .enumerate()
                .map(|(i, p)| olo_protocol::SnapPad {
                    letter: p.letter.to_ascii_uppercase().to_string(),
                    place: p.place.name(),
                    x: p.rect.loc.x,
                    y: p.rect.loc.y,
                    width: p.rect.size.w,
                    height: p.rect.size.h,
                    inside: snap.at == Some(i),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect::new((x, y).into(), (w, h).into())
    }

    fn place_at(pads: &[Pad], x: f64, y: f64) -> Option<Place> {
        pad_at(pads, (x, y).into()).map(|i| pads[i].place)
    }

    /// The smallest screen with Deskbar at its right and a margin round
    /// the windows: every place has its pad, none over another, each on
    /// the area; and the pointer finds each where a hand goes for it, the
    /// screen's very edges and corners too, past the margin and Deskbar.
    #[test]
    fn the_nine_places_of_a_display() {
        let (screen, area) = (r(0, 0, 1280, 800), r(8, 8, 1100, 784));
        let pads = pads(screen, area, &[]);
        assert_eq!(pads.iter().map(|p| p.letter).collect::<String>(), "qezcadswx");
        for (i, a) in pads.iter().enumerate() {
            assert!(area.contains_rect(a.rect), "{a:?} on the area");
            assert!(a.hot.contains_rect(a.rect) && screen.contains_rect(a.hot), "{a:?}");
            for b in &pads[i + 1..] {
                assert!(!a.rect.overlaps(b.rect) && !a.hot.overlaps(b.hot), "{a:?} over {b:?}");
            }
        }
        use Place::Whole;
        use Tile::*;
        let t = |tile| Some(Place::Tile(tile));
        // The corners, to the screen's own.
        assert_eq!((place_at(&pads, 0.0, 0.0), place_at(&pads, 1279.0, 0.0), place_at(&pads, 0.0, 799.0), place_at(&pads, 1279.0, 799.0)), (t(TopLeft), t(TopRight), t(BottomLeft), t(BottomRight)));
        // The sides in the middle; the right one past Deskbar.
        assert_eq!((place_at(&pads, 0.0, 400.0), place_at(&pads, 1279.0, 400.0), place_at(&pads, 1100.0, 400.0)), (t(Left), t(Right), t(Right)));
        // The very top anywhere between the corners: the whole screen.
        assert_eq!((place_at(&pads, 200.0, 0.0), place_at(&pads, 640.0, 3.0), place_at(&pads, 950.0, 0.0)), (Some(Whole), Some(Whole), Some(Whole)));
        // Under it in the middle, the top half; at the bottom, the bottom.
        assert_eq!((place_at(&pads, 558.0, 60.0), place_at(&pads, 558.0, 799.0)), (t(Top), t(Bottom)));
        // Anywhere else: none (the window is moved, no more).
        for (x, y) in [(400.0, 400.0), (200.0, 60.0), (900.0, 300.0), (300.0, 780.0), (60.0, 200.0)] {
            assert_eq!(place_at(&pads, x, y), None, "{x},{y}");
        }
    }

    /// Other windows have a pad at their middle, lettered from the right
    /// hand's keys, the front one first; one whose middle is under a pad
    /// already there, or with too little of it on the display, has none.
    #[test]
    fn other_windows_places() {
        let (screen, area) = (r(0, 0, 1280, 800), r(0, 0, 1280, 800));
        let others = [(7, r(200, 150, 400, 300)), (9, r(700, 200, 500, 400)), (11, r(240, 180, 400, 300)), (12, r(-300, 300, 380, 300)), (13, r(500, 0, 300, 150))];
        let pads = pads(screen, area, &others);
        let windows: Vec<(Place, char, Rect)> = pads.iter().filter(|p| matches!(p.place, Place::Window(_))).map(|p| (p.place, p.letter, p.rect)).collect();
        // 11's middle is under 7's pad, 12 shows 80 points, 13's is under the top's.
        assert_eq!(windows, [(Place::Window(7), 'j', r(336, 255, 128, 90)), (Place::Window(9), 'k', r(886, 355, 128, 90))]);
        assert_eq!((place_at(&pads, 400.0, 300.0), place_at(&pads, 250.0, 200.0), place_at(&pads, 950.0, 400.0)), (Some(Place::Window(7)), None, Some(Place::Window(9))));
    }

    /// A display too small for them all: the corners first, then what
    /// fits; none over another.
    #[test]
    fn a_small_display_has_what_fits() {
        let screen = r(0, 0, 320, 240);
        let pads = pads(screen, screen, &[]);
        assert_eq!(pads.iter().map(|p| p.letter).collect::<String>(), "qezcs");
        for (i, a) in pads.iter().enumerate() {
            for b in &pads[i + 1..] {
                assert!(!a.rect.overlaps(b.rect), "{a:?} over {b:?}");
            }
        }
    }
}
