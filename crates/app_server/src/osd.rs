//! The level indicator: a small BeOS panel low on the screen under the
//! pointer when the brightness or volume keys are pressed, or caffeinated
//! mode is switched from the keyboard. One at a time; each change
//! replaces it and restarts its timer.

use olo_sync::Mutex;
use std::time::{Duration, Instant};

use crate::render::Exact;
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
    utils::{Buffer, Rectangle, Size, Transform},
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke};

use crate::{
    shell::fill,
    state::{AnvilState, Backend},
    text, theme,
};

/// What the panel shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Indicator {
    Brightness(f32),
    KeyboardBacklight(f32),
    Volume {
        level: f32,
        muted: bool,
    },
    /// The microphone key: the microphone in use muted, or not.
    Microphone {
        muted: bool,
    },
    /// The airplane key: every radio off, or back on.
    Airplane {
        on: bool,
    },
    /// On or off, with a line saying for how long.
    Caffeinated {
        on: bool,
        detail: String,
    },
    /// A windowing mode chosen (⌃⌥⌘T, ⌃⌥⌘O).
    Mode(olo_protocol::WindowingMode),
    /// Main and Stack's main window pinned (⌃⌥⇧⌘M), or let go.
    Pinned(bool),
    /// Auto-tiling slot lock toggled (dragged onto a hotspot or via menu/IPC).
    SlotLocked {
        tile: olo_protocol::Tile,
        locked: bool,
        window: Option<String>,
    },
    /// Do Not Disturb on or off (a Magic Keyboard's key).
    DoNotDisturb {
        on: bool,
    },
}

/// How long the panel stays up after the last change.
const SHOWN: Duration = Duration::from_millis(1600);
/// Logical metrics.
const PAD: f32 = 12.0;
const ICON: f32 = 32.0;
const GAP: f32 = 12.0;
const TITLE: f32 = 12.0;
const SEGMENTS: usize = 16;
const SEGMENT: f32 = 8.0;
const BAR: f32 = 10.0;
/// From the bottom of the screen to the panel, as a share of its height.
const LOW: f64 = 1.0 / 6.0;

type Shown = (Instant, MemoryRenderBuffer, (i32, i32));

#[derive(Default)]
struct Slot(Mutex<Option<Shown>>);

/// Whether the indicator is up on `output` (or fading).
pub fn showing(output: &Output) -> bool {
    slot(output).0.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

fn slot(output: &Output) -> &Slot {
    output.user_data().insert_if_missing_threadsafe(Slot::default);
    output.user_data().get::<Slot>().unwrap()
}

impl Indicator {
    fn title(&self) -> &'static str {
        match self {
            Indicator::Brightness(l) if crate::power::off_at(*l) => "Display off",
            Indicator::Brightness(_) => "Brightness",
            Indicator::KeyboardBacklight(_) => "Keyboard Backlight",
            Indicator::Volume { muted: true, .. } => "Volume (muted)",
            Indicator::Volume { .. } => "Volume",
            Indicator::Microphone { muted: true } => "Microphone muted",
            Indicator::Microphone { muted: false } => "Microphone on",
            Indicator::Airplane { on: true } => "Airplane mode on",
            Indicator::Airplane { on: false } => "Airplane mode off",
            Indicator::Caffeinated { on: true, .. } => "Caffeinated",
            Indicator::Caffeinated { on: false, .. } => "Sleep allowed",
            Indicator::Mode(m) => m.title(),
            Indicator::Pinned(true) => "Main window pinned",
            Indicator::Pinned(false) => "Main window let go",
            Indicator::SlotLocked { tile, locked: true, .. } => match tile {
                olo_protocol::Tile::Left => "Locked to Left Half",
                olo_protocol::Tile::Right => "Locked to Right Half",
                olo_protocol::Tile::Top => "Locked to Top Half",
                olo_protocol::Tile::Bottom => "Locked to Bottom Half",
                olo_protocol::Tile::TopLeft => "Locked to Top Left Quarter",
                olo_protocol::Tile::TopRight => "Locked to Top Right Quarter",
                olo_protocol::Tile::BottomLeft => "Locked to Bottom Left Quarter",
                olo_protocol::Tile::BottomRight => "Locked to Bottom Right Quarter",
                _ => "Locked to Slot",
            },
            Indicator::SlotLocked { tile, locked: false, .. } => match tile {
                olo_protocol::Tile::Left => "Unlocked from Left Half",
                olo_protocol::Tile::Right => "Unlocked from Right Half",
                olo_protocol::Tile::Top => "Unlocked from Top Half",
                olo_protocol::Tile::Bottom => "Unlocked from Bottom Half",
                olo_protocol::Tile::TopLeft => "Unlocked from Top Left Quarter",
                olo_protocol::Tile::TopRight => "Unlocked from Top Right Quarter",
                olo_protocol::Tile::BottomLeft => "Unlocked from Bottom Left Quarter",
                olo_protocol::Tile::BottomRight => "Unlocked from Bottom Right Quarter",
                _ => "Unlocked from Slot",
            },
            Indicator::DoNotDisturb { on: true } => "Do Not Disturb on",
            Indicator::DoNotDisturb { on: false } => "Do Not Disturb off",
        }
    }

    fn detail(&self) -> Option<std::borrow::Cow<'_, str>> {
        use std::borrow::Cow;
        match self {
            Indicator::Caffeinated { detail, .. } => Some(Cow::Borrowed(detail.as_str())),
            Indicator::Mode(m) => Some(Cow::Borrowed(m.detail())),
            Indicator::Pinned(true) => Some(Cow::Borrowed("It keeps its place")),
            Indicator::Pinned(false) => Some(Cow::Borrowed("A new window takes it")),
            Indicator::SlotLocked { locked, window, .. } => {
                let tail = if *locked { "Others tile around it" } else { "Back to Auto-Tiling" };
                match window.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
                    Some(name) => {
                        let short: String = if name.chars().count() > 22 { format!("{}…", name.chars().take(21).collect::<String>()) } else { name.to_owned() };
                        Some(Cow::Owned(format!("{short} · {tail}")))
                    }
                    None => Some(Cow::Borrowed(tail)),
                }
            }
            _ => None,
        }
    }

    fn level(&self) -> Option<f32> {
        match self {
            Indicator::Brightness(l) | Indicator::KeyboardBacklight(l) => Some(*l),
            Indicator::Volume { level, muted } => Some(if *muted { 0.0 } else { *level }),
            Indicator::Caffeinated { .. }
            | Indicator::Mode(_)
            | Indicator::Pinned(_)
            | Indicator::SlotLocked { .. }
            | Indicator::Microphone { .. }
            | Indicator::Airplane { .. }
            | Indicator::DoNotDisturb { .. } => None,
        }
    }
}

/// How many of the bar's segments `level` lights (any sound lights one).
fn lit(level: f32) -> usize {
    let n = (level.clamp(0.0, 1.0) * SEGMENTS as f32).round() as usize;
    if level > 0.0 { n.max(1) } else { 0 }
}

/// Draw the panel at `s` device pixels per logical pixel.
fn draw(ind: &Indicator, s: f64) -> Pixmap {
    let p = theme::palette();
    let px = |l: f32| (l as f64 * s).round() as i32;
    let u = px(1.0).max(1);
    let title_size = TITLE * s as f32;
    let detail_size = (TITLE - 1.0) * s as f32;
    let detail_owned = ind.detail();
    let is_slot = matches!(ind, Indicator::SlotLocked { .. });
    let pad = px(if is_slot { 16.0 } else { PAD });
    let gap = px(GAP);
    let icon_w = px(if is_slot { 44.0 } else { ICON });
    let bar_w = SEGMENTS as i32 * px(SEGMENT);
    let content_w = if ind.level().is_some() {
        bar_w
    } else {
        let title_w = text::measure(ind.title(), title_size, true).ceil() as i32;
        let detail_w = detail_owned.as_deref().map_or(0, |d| text::measure(d, detail_size, false).ceil() as i32);
        title_w.max(detail_w).max(px(110.0)).min(px(if is_slot { 280.0 } else { 230.0 }))
    };
    let w = pad * 2 + icon_w + gap + content_w;
    let h = if is_slot { pad * 2 + px(30.0) } else { px(PAD) * 2 + px(ICON + 8.0) };
    let mut pm = Pixmap::new(w as u32, h as u32).expect("indicator pixmap");
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let bg = if dark { olo_theme::tint(p.panel_background, 0.9) } else { p.panel_background };
    fill(&mut pm, 0, 0, w, h, p.frame_focused[0]);
    fill(&mut pm, u, u, w - 2 * u, h - 2 * u, bg);
    fill(&mut pm, u, u, w - 2 * u, u, p.shine);
    fill(&mut pm, u, u, u, h - 2 * u, p.shine);
    fill(&mut pm, u, h - 2 * u, w - 2 * u, u, p.shadow);
    fill(&mut pm, w - 2 * u, u, u, h - 2 * u, p.shadow);

    let icon = (pad as f32, ((h - icon_w) / 2) as f32, icon_w as f32);
    draw_icon(&mut pm, ind, icon, p.panel_text, &p, s);

    let x = pad + icon_w + gap;
    if is_slot {
        // Center the two-line optical letter bounds cleanly on the mini-screen's centerline
        // with 16 pt padding on all four sides.
        let cy = h as f32 / 2.0;
        let title_y = text::top_for_centre(cy - (7.0 * s) as f32, title_size, true);
        text::draw(&mut pm, ind.title(), x as f32, title_y, title_size, true, p.panel_text);
        if let Some(detail) = detail_owned.as_deref() {
            let detail = text::ellipsize(detail, detail_size, false, content_w as f32);
            let sub_ink = olo_theme::mix(p.panel_text, bg, 0.30);
            let detail_y = text::top_for_centre(cy + (7.5 * s) as f32, detail_size, false);
            text::draw(&mut pm, &detail, x as f32, detail_y, detail_size, false, sub_ink);
        }
        return pm;
    }
    let line = (title_size * 1.25).ceil() as i32;
    let top = (h - line - px(5.0) - px(BAR)) / 2;
    text::draw(&mut pm, ind.title(), x as f32, top as f32, title_size, true, p.panel_text);
    let y = top + line + px(5.0);
    match ind.level() {
        Some(level) => {
            let on = lit(level);
            let seg = px(SEGMENT);
            for i in 0..SEGMENTS as i32 {
                let sx = x + i * seg;
                let (sw, sh) = (seg - u, px(BAR));
                // Sunken cells, lit in the status-bar blue.
                fill(&mut pm, sx, y, sw, sh, p.shadow);
                fill(&mut pm, sx + u, y + u, sw - u, sh - u, p.shine);
                let c = if (i as usize) < on { p.status_bar } else { p.document_background };
                fill(&mut pm, sx + u, y + u, sw - 2 * u, sh - 2 * u, c);
            }
        }
        None => {
            if let Some(detail) = detail_owned.as_deref() {
                let detail = text::ellipsize(detail, detail_size, false, content_w as f32);
                let sub_ink = olo_theme::mix(p.panel_text, bg, 0.30);
                text::draw(&mut pm, &detail, x as f32, (y - px(1.0)) as f32, detail_size, false, sub_ink);
            }
        }
    }
    pm
}

/// The indicator's symbol in `(x, y, size)`.
fn draw_icon(pm: &mut Pixmap, ind: &Indicator, (x, y, size): (f32, f32, f32), rgba: [u8; 4], palette: &olo_theme::Palette, s: f64) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = true;
    let k = size / 32.0;
    let at = |px: f32, py: f32| (x + px * k, y + py * k);
    let stroke = |w: f32| Stroke { width: w * k, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
    let id = tiny_skia::Transform::identity();
    match ind {
        Indicator::Brightness(_) => {
            // A sun: a disc and eight rays.
            let (cx, cy) = at(16.0, 16.0);
            if let Some(disc) = PathBuilder::from_circle(cx, cy, 6.0 * k) {
                pm.fill_path(&disc, &paint, FillRule::Winding, id, None);
            }
            let mut pb = PathBuilder::new();
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (s, c) = a.sin_cos();
                pb.move_to(cx + c * 10.0 * k, cy + s * 10.0 * k);
                pb.line_to(cx + c * 14.0 * k, cy + s * 14.0 * k);
            }
            if let Some(rays) = pb.finish() {
                pm.stroke_path(&rays, &paint, &stroke(2.5), id, None);
            }
        }
        Indicator::KeyboardBacklight(level) => {
            // Three rows of keys, glowing from above while lit.
            for (row, keys) in [(18.0, 5), (23.0, 5), (28.0, 3)] {
                let w = if keys == 3 { 7.0 } else { 4.0 };
                for i in 0..keys {
                    let kx = if keys == 3 { 4.0 + i as f32 * 8.5 } else { 3.0 + i as f32 * 5.4 };
                    let (a, b) = at(kx, row - 3.5);
                    if let Some(r) = tiny_skia::Rect::from_xywh(a, b, w * k, 3.5 * k) {
                        pm.fill_rect(r, &paint, id, None);
                    }
                }
            }
            if *level > 0.0 {
                let mut pb = PathBuilder::new();
                let rays = 1 + (*level * 2.99) as i32;
                for i in 0..[1, 3, 5][(rays - 1) as usize] {
                    let i = i as f32 - ([1, 3, 5][(rays - 1) as usize] as f32 - 1.0) / 2.0;
                    let (a, b) = at(16.0 + i * 5.0, 10.0);
                    let (c, d) = at(16.0 + i * 6.5, 3.0);
                    pb.move_to(a, b);
                    pb.line_to(c, d);
                }
                if let Some(r) = pb.finish() {
                    pm.stroke_path(&r, &paint, &stroke(2.0), id, None);
                }
            }
        }
        Indicator::Volume { level, muted } => {
            // A speaker, with waves for the level or a cross when muted.
            let mut pb = PathBuilder::new();
            let pts = [(3.0, 12.0), (9.0, 12.0), (17.0, 5.0), (17.0, 27.0), (9.0, 20.0), (3.0, 20.0)];
            let (sx, sy) = at(pts[0].0, pts[0].1);
            pb.move_to(sx, sy);
            for &(px, py) in &pts[1..] {
                let (a, b) = at(px, py);
                pb.line_to(a, b);
            }
            pb.close();
            if let Some(body) = pb.finish() {
                pm.fill_path(&body, &paint, FillRule::Winding, id, None);
            }
            let mut pb = PathBuilder::new();
            if *muted {
                let ((a, b), (c, d)) = (at(21.0, 11.0), at(29.0, 21.0));
                pb.move_to(a, b);
                pb.line_to(c, d);
                let ((a, b), (c, d)) = (at(29.0, 11.0), at(21.0, 21.0));
                pb.move_to(a, b);
                pb.line_to(c, d);
            } else {
                let (cx, cy) = at(17.0, 16.0);
                let waves = if *level <= 0.0 { 0 } else { 1 + (*level * 2.99) as i32 };
                for i in 0..waves {
                    let r = (6.0 + i as f32 * 4.5) * k;
                    let a = 0.75f32;
                    pb.move_to(cx + r * a.cos(), cy - r * a.sin());
                    // A gentle arc through the right.
                    let (c1, c2) = ((cx + r * 1.25, cy - r * 0.35), (cx + r * 1.25, cy + r * 0.35));
                    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, cx + r * a.cos(), cy + r * a.sin());
                }
            }
            if let Some(marks) = pb.finish() {
                pm.stroke_path(&marks, &paint, &stroke(2.2), id, None);
            }
        }
        Indicator::Microphone { muted } => {
            // A microphone: its capsule, the cradle round it, the stand;
            // a slash across while muted.
            let (cx, top) = at(16.0, 3.0);
            let (l, r, b, rad) = (cx - 5.0 * k, cx + 5.0 * k, top + 16.0 * k, 5.0 * k);
            let mut pb = PathBuilder::new();
            pb.move_to(l, top + rad);
            pb.quad_to(l, top, cx, top);
            pb.quad_to(r, top, r, top + rad);
            pb.line_to(r, b - rad);
            pb.quad_to(r, b, cx, b);
            pb.quad_to(l, b, l, b - rad);
            pb.close();
            if let Some(capsule) = pb.finish() {
                pm.fill_path(&capsule, &paint, FillRule::Winding, id, None);
            }
            let mut pb = PathBuilder::new();
            let ((a, b), (c, d)) = (at(8.5, 13.0), at(23.5, 13.0));
            pb.move_to(a, b);
            pb.cubic_to(a, b + 8.0 * k, c, d + 8.0 * k, c, d);
            let ((g, h), (i, j)) = (at(16.0, 23.0), at(16.0, 28.5));
            pb.move_to(g, h);
            pb.line_to(i, j);
            let ((l, m), (n, o)) = (at(11.0, 28.5), at(21.0, 28.5));
            pb.move_to(l, m);
            pb.line_to(n, o);
            if *muted {
                let ((p, q), (r, s)) = (at(5.0, 4.0), at(27.0, 28.0));
                pb.move_to(p, q);
                pb.line_to(r, s);
            }
            if let Some(marks) = pb.finish() {
                pm.stroke_path(&marks, &paint, &stroke(2.2), id, None);
            }
        }
        Indicator::Airplane { on } => {
            // An airplane seen from above, nose up; crossed out while
            // the radios are back on.
            let pts = [
                (16.0, 2.5),
                (17.8, 5.0),
                (17.8, 12.0),
                (29.0, 19.0),
                (29.0, 22.0),
                (17.8, 18.5),
                (17.8, 25.0),
                (21.5, 28.0),
                (21.5, 30.0),
                (16.0, 28.5),
                (10.5, 30.0),
                (10.5, 28.0),
                (14.2, 25.0),
                (14.2, 18.5),
                (3.0, 22.0),
                (3.0, 19.0),
                (14.2, 12.0),
                (14.2, 5.0),
            ];
            let mut pb = PathBuilder::new();
            let (sx, sy) = at(pts[0].0, pts[0].1);
            pb.move_to(sx, sy);
            for &(px, py) in &pts[1..] {
                let (a, b) = at(px, py);
                pb.line_to(a, b);
            }
            pb.close();
            if let Some(plane) = pb.finish() {
                pm.fill_path(&plane, &paint, FillRule::Winding, id, None);
            }
            if !*on {
                // The slash, with the panel's colour beside it so it reads
                // over the wings.
                let mut pb = PathBuilder::new();
                let ((p, q), (r, s)) = (at(5.0, 4.0), at(27.0, 28.0));
                pb.move_to(p, q);
                pb.line_to(r, s);
                if let Some(slash) = pb.finish() {
                    let mut gap = paint.clone();
                    let bg = theme::palette().panel_background;
                    gap.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
                    pm.stroke_path(&slash, &gap, &stroke(5.0), id, None);
                    pm.stroke_path(&slash, &paint, &stroke(2.2), id, None);
                }
            }
        }
        Indicator::DoNotDisturb { on } => {
            // A crescent moon (a disc, a smaller one of the panel's colour
            // over its upper right); crossed out when it's off, the other
            // way from the others' (theirs runs along its inner edge).
            let bg = theme::palette().panel_background;
            let mut gap = paint.clone();
            gap.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
            let (cx, cy) = at(15.0, 17.0);
            let (mx, my) = at(20.5, 11.5);
            if let (Some(disc), Some(bite)) = (PathBuilder::from_circle(cx, cy, 12.0 * k), PathBuilder::from_circle(mx, my, 10.0 * k)) {
                pm.fill_path(&disc, &paint, FillRule::Winding, id, None);
                pm.fill_path(&bite, &gap, FillRule::Winding, id, None);
            }
            if !*on {
                let mut pb = PathBuilder::new();
                let ((p, q), (r, s)) = (at(5.0, 28.0), at(27.0, 4.0));
                pb.move_to(p, q);
                pb.line_to(r, s);
                if let Some(slash) = pb.finish() {
                    pm.stroke_path(&slash, &gap, &stroke(5.0), id, None);
                    pm.stroke_path(&slash, &paint, &stroke(2.2), id, None);
                }
            }
        }
        Indicator::Mode(mode) => {
            use olo_protocol::WindowingMode as M;
            // Auto-Tiling: a screen in tiles, as dwindle splits it (a
            // half, then a quarter and two eighths). Focus: one window in
            // a viewfinder's corners. Standard: two windows, one over the
            // other.
            let (filled, outlined): (&[(f32, f32, f32, f32)], &[(f32, f32, f32, f32)]) = match mode {
                M::AutoTiling => (&[(3.0, 5.0, 12.0, 22.0), (17.0, 5.0, 12.0, 10.0), (17.0, 17.0, 5.0, 10.0), (24.0, 17.0, 5.0, 10.0)], &[]),
                M::Focus => (&[(9.0, 11.0, 14.0, 10.0)], &[]),
                M::Standard => (&[(11.0, 12.0, 18.0, 15.0)], &[(3.0, 5.0, 18.0, 15.0)]),
            };
            for (tx, ty, tw, th) in outlined {
                let (a, b) = at(*tx, *ty);
                if let Some(r) = tiny_skia::Rect::from_xywh(a, b, tw * k, th * k) {
                    pm.stroke_path(&PathBuilder::from_rect(r), &paint, &stroke(1.6), id, None);
                }
            }
            for (tx, ty, tw, th) in filled {
                let (a, b) = at(*tx, *ty);
                if let Some(r) = tiny_skia::Rect::from_xywh(a, b, tw * k, th * k) {
                    pm.fill_rect(r, &paint, id, None);
                }
            }
            if *mode == M::Focus {
                let mut pb = PathBuilder::new();
                for (cx, cy, dx, dy) in [(3.0, 5.0, 1.0, 1.0), (29.0, 5.0, -1.0, 1.0), (3.0, 27.0, 1.0, -1.0), (29.0, 27.0, -1.0, -1.0)] {
                    let (a, b) = at(cx, cy + dy * 6.0);
                    pb.move_to(a, b);
                    let (a, b) = at(cx, cy);
                    pb.line_to(a, b);
                    let (a, b) = at(cx + dx * 7.0, cy);
                    pb.line_to(a, b);
                }
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &paint, &stroke(2.0), id, None);
                }
            }
        }
        Indicator::Pinned(on) => {
            // Main and Stack: the main tile and two of the stack beside
            // it. Let go, the main tile is filled; pinned, it's drawn
            // round a pin (its head, and its point under it).
            for (tx, ty, tw, th, filled) in [(3.0, 5.0, 15.0, 22.0, !*on), (20.0, 5.0, 9.0, 10.0, false), (20.0, 17.0, 9.0, 10.0, false)] {
                let (a, b) = at(tx, ty);
                if let Some(r) = tiny_skia::Rect::from_xywh(a, b, tw * k, th * k) {
                    if filled {
                        pm.fill_rect(r, &paint, id, None);
                    } else {
                        pm.stroke_path(&PathBuilder::from_rect(r), &paint, &stroke(1.6), id, None);
                    }
                }
            }
            if *on {
                let (cx, cy) = at(10.5, 12.5);
                if let Some(head) = PathBuilder::from_circle(cx, cy, 3.4 * k) {
                    pm.fill_path(&head, &paint, tiny_skia::FillRule::Winding, id, None);
                }
                let mut pb = PathBuilder::new();
                let ((a, b), (c, d)) = (at(10.5, 15.0), at(10.5, 22.5));
                pb.move_to(a, b);
                pb.line_to(c, d);
                if let Some(point) = pb.finish() {
                    pm.stroke_path(&point, &paint, &stroke(1.8), id, None);
                }
            }
        }
        Indicator::SlotLocked { tile, locked, .. } => {
            // Pixel-snapped BeOS mini-screen matching the drag hotspot plates:
            // locked = blue fill + white closed padlock in the slot;
            // unlocked = blue/sky-blue 1 pt outline + open padlock in the same slot.
            let u = (s.floor() as i32).max(1);
            let px = |l: f64| (l * s).round() as i32;
            let sw = px(44.0);
            let sh = px(30.0);
            let sx = x.round() as i32;
            let sy = (pm.height() as i32 - sh) / 2;
            let dark = palette.mode == olo_theme::ThemeMode::Dark;
            let ground = if dark { olo_theme::tint(palette.panel_background, 0.9) } else { palette.panel_background };
            let lit_c = if dark { olo_theme::mix(ground, [255, 255, 255, 255], 0.2) } else { olo_theme::mix(ground, palette.shine, 0.8) };
            fill(pm, sx, sy, sw, u, palette.shade(ground, 1.15));
            fill(pm, sx, sy + u, u, sh - u, palette.shade(ground, 1.15));
            fill(pm, sx + u, sy + sh - u, sw - u, u, lit_c);
            fill(pm, sx + sw - u, sy + u, u, sh - 2 * u, lit_c);
            fill(pm, sx + u, sy + u, sw - 2 * u, u, palette.shade(ground, 1.55));
            fill(pm, sx + u, sy + 2 * u, u, sh - 3 * u, palette.shade(ground, 1.55));
            fill(pm, sx + 2 * u, sy + sh - 2 * u, sw - 3 * u, u, palette.shade(ground, 1.06));
            fill(pm, sx + sw - 2 * u, sy + 2 * u, u, sh - 4 * u, palette.shade(ground, 1.06));
            let (ix, iy, iw, ih) = (sx + 2 * u, sy + 2 * u, sw - 4 * u, sh - 4 * u);
            fill(pm, ix, iy, iw, ih, palette.document_background);
            let stroke_c = if dark { [126, 142, 162, 255] } else { [132, 146, 165, 255] };
            let fill_c = olo_theme::mix(palette.document_background, stroke_c, if dark { 0.16 } else { 0.12 });
            let mut draw_pane = |fx: f32, fy: f32, fw: f32, fh: f32| {
                let x0 = ix + (fx * iw as f32).round() as i32 + u;
                let y0 = iy + (fy * ih as f32).round() as i32 + u;
                let x1 = ix + ((fx + fw) * iw as f32).round() as i32 - u;
                let y1 = iy + ((fy + fh) * ih as f32).round() as i32 - u;
                let (rw, rh) = (x1 - x0, y1 - y0);
                if rw > 2 * u && rh > 2 * u {
                    fill(pm, x0, y0, rw, rh, stroke_c);
                    fill(pm, x0 + u, y0 + u, rw - 2 * u, rh - 2 * u, fill_c);
                }
            };
            match tile {
                olo_protocol::Tile::Left => {
                    draw_pane(0.5, 0.0, 0.5, 0.5);
                    draw_pane(0.5, 0.5, 0.5, 0.5);
                }
                olo_protocol::Tile::Right => {
                    draw_pane(0.0, 0.0, 0.5, 0.5);
                    draw_pane(0.0, 0.5, 0.5, 0.5);
                }
                olo_protocol::Tile::Top => {
                    draw_pane(0.0, 0.5, 0.5, 0.5);
                    draw_pane(0.5, 0.5, 0.5, 0.5);
                }
                olo_protocol::Tile::Bottom => {
                    draw_pane(0.0, 0.0, 0.5, 0.5);
                    draw_pane(0.5, 0.0, 0.5, 0.5);
                }
                olo_protocol::Tile::TopLeft => {
                    draw_pane(0.0, 0.5, 0.5, 0.5);
                    draw_pane(0.5, 0.0, 0.5, 1.0);
                }
                olo_protocol::Tile::TopRight => {
                    draw_pane(0.5, 0.5, 0.5, 0.5);
                    draw_pane(0.0, 0.0, 0.5, 1.0);
                }
                olo_protocol::Tile::BottomLeft => {
                    draw_pane(0.0, 0.0, 0.5, 0.5);
                    draw_pane(0.5, 0.0, 0.5, 1.0);
                }
                olo_protocol::Tile::BottomRight => {
                    draw_pane(0.5, 0.0, 0.5, 0.5);
                    draw_pane(0.0, 0.0, 0.5, 1.0);
                }
                _ => {}
            }
            let (fx, fy, fw, fh) = tile.fraction();
            let x0 = ix + (fx * iw as f32).round() as i32 + u;
            let y0 = iy + (fy * ih as f32).round() as i32 + u;
            let x1 = ix + ((fx + fw) * iw as f32).round() as i32 - u;
            let y1 = iy + ((fy + fh) * ih as f32).round() as i32 - u;
            let (rw, rh) = (x1 - x0, y1 - y0);
            let short = fh <= 0.55;
            let want_w = if short { px(5.0) } else { px(7.0) };
            let want_h = if short { px(7.0) } else { px(9.0) };
            if *locked {
                fill(pm, x0, y0, rw, rh, palette.control_highlight);
                let pad_y = if rh >= 10 * u { 2 * u } else { u };
                let lw = want_w.min(rw - 2 * u).max(5 * u);
                let lh = want_h.min(rh - 2 * pad_y).max(5 * u);
                if rw >= lw + 2 * u && rh >= lh + 2 * u {
                    let lx = x0 + (rw - lw) / 2;
                    let ly = y0 + (rh - lh) / 2;
                    crate::shell::deco::draw_lock_glyph(pm, lx, ly, lw, lh, u, [255, 255, 255, 255], palette.control_highlight);
                }
            } else if rw > 2 * u && rh > 2 * u {
                let accent_ink = if dark { olo_theme::mix(palette.control_highlight, [255, 255, 255, 255], 0.38) } else { palette.control_highlight };
                fill(pm, x0, y0, rw, rh, accent_ink);
                fill(pm, x0 + u, y0 + u, rw - 2 * u, rh - 2 * u, fill_c);
                let inner_w = rw - 2 * u;
                let inner_h = rh - 2 * u;
                let pad_y = if inner_h >= 10 * u { 2 * u } else { u };
                let lw = want_w.min(inner_w - 2 * u).max(5 * u);
                let lh = want_h.min(inner_h - 2 * pad_y).max(5 * u);
                if inner_w >= lw && inner_h >= lh {
                    let lx = x0 + u + (inner_w - lw) / 2;
                    let ly = y0 + u + (inner_h - lh) / 2;
                    crate::shell::deco::draw_open_lock_glyph(pm, lx, ly, lw, lh, u, accent_ink, fill_c);
                }
            }
        }
        Indicator::Caffeinated { on, .. } => {
            // A cup with a handle; steam while on.
            let mut pb = PathBuilder::new();
            let (a, b) = at(5.0, 13.0);
            pb.move_to(a, b);
            let (a, b) = at(23.0, 13.0);
            pb.line_to(a, b);
            let ((c1x, c1y), (c2x, c2y), (ex, ey)) = (at(23.0, 24.0), at(19.0, 28.0), at(14.0, 28.0));
            pb.cubic_to(c1x, c1y, c2x, c2y, ex, ey);
            let ((c1x, c1y), (c2x, c2y), (ex, ey)) = (at(9.0, 28.0), at(5.0, 24.0), at(5.0, 13.0));
            pb.cubic_to(c1x, c1y, c2x, c2y, ex, ey);
            pb.close();
            if let Some(cup) = pb.finish() {
                if *on {
                    pm.fill_path(&cup, &paint, FillRule::Winding, id, None);
                } else {
                    pm.stroke_path(&cup, &paint, &stroke(2.0), id, None);
                }
            }
            let mut pb = PathBuilder::new();
            let (cx, cy) = at(23.5, 18.5);
            pb.push_circle(cx, cy, 3.5 * k);
            if *on {
                for sx in [10.0, 15.0, 20.0] {
                    let (a, b) = at(sx - 1.0, 9.0);
                    pb.move_to(a, b);
                    let ((c1x, c1y), (c2x, c2y), (ex, ey)) = (at(sx - 3.0, 6.5), at(sx + 1.5, 5.0), at(sx - 0.5, 2.0));
                    pb.cubic_to(c1x, c1y, c2x, c2y, ex, ey);
                }
            }
            if let Some(rest) = pb.finish() {
                pm.stroke_path(&rest, &paint, &stroke(1.8), id, None);
            }
        }
    }
}

/// Put `ind` up on `output` (and take it down elsewhere).
pub fn show<'a>(outputs: impl Iterator<Item = &'a Output>, on: &Output, ind: &Indicator) {
    for o in outputs {
        if o != on {
            *slot(o).0.lock().unwrap() = None;
        }
    }
    let s = on.current_scale().fractional_scale();
    let pm = draw(ind, s);
    let size: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
    let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, Some(vec![Rectangle::from_size(size)]));
    *slot(on).0.lock().unwrap() = Some((Instant::now() + SHOWN, buffer, (size.w, size.h)));
}

/// The indicator on `output`, while it is up: centred, low down.
pub fn element<R>(renderer: &mut R, output: &Output, screen: Size<i32, smithay::utils::Logical>) -> Option<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut slot = slot(output).0.lock().unwrap();
    if slot.as_ref().is_some_and(|(until, _, _)| Instant::now() >= *until) {
        *slot = None;
    }
    let (_, buffer, (w, h)) = slot.as_ref()?;
    let s = output.current_scale().fractional_scale();
    let screen = screen.to_f64().to_physical(s);
    let x = ((screen.w - *w as f64) / 2.0).round();
    let y = (screen.h * (1.0 - LOW) - *h as f64).round();
    crate::render::picture(renderer, (x, y).into(), buffer, (*w, *h), None, Kind::Unspecified)
}

impl<B: Backend> AnvilState<B> {
    /// Show `ind` on the screen under the pointer.
    pub fn show_indicator(&mut self, ind: Indicator) {
        self.backend_data.wake(None);
        let at = self.pointer.current_location();
        let Some(output) = self.space.output_under(at).next().or_else(|| self.space.outputs().next()).cloned() else { return };
        show(self.space.outputs(), &output, &ind);
    }

    /// Show an indicator directly on `output` without querying the pointer
    /// (safe to call from inside a pointer grab's `unset`).
    pub fn show_indicator_on(&mut self, output: &Output, ind: Indicator) {
        self.backend_data.wake(None);
        show(self.space.outputs(), output, &ind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_level_lights_a_segment() {
        assert_eq!(lit(0.0), 0);
        assert_eq!(lit(0.01), 1);
        assert_eq!(lit(0.5), 8);
        assert_eq!(lit(1.0), 16);
        assert_eq!(lit(1.5), 16);
    }

    /// `OLO_OSD_SHEET=/var/tmp/osd.png cargo test -p app_server osd_sheet`
    #[test]
    fn osd_sheet() {
        let Some(out) = std::env::var_os("OLO_OSD_SHEET") else { return };
        let inds = [
            Indicator::Brightness(0.75),
            Indicator::KeyboardBacklight(0.5),
            Indicator::Volume { level: 0.4, muted: false },
            Indicator::Volume { level: 0.4, muted: true },
            Indicator::Microphone { muted: false },
            Indicator::Microphone { muted: true },
            Indicator::Airplane { on: true },
            Indicator::Airplane { on: false },
            Indicator::Caffeinated { on: true, detail: "For 1 hour, until 15:30".into() },
            Indicator::Caffeinated { on: false, detail: "Sleeps when idle".into() },
            Indicator::Mode(olo_protocol::WindowingMode::Standard),
            Indicator::Mode(olo_protocol::WindowingMode::AutoTiling),
            Indicator::Mode(olo_protocol::WindowingMode::Focus),
            Indicator::Pinned(true),
            Indicator::Pinned(false),
            Indicator::SlotLocked { tile: olo_protocol::Tile::Left, locked: true, window: Some("Kache".into()) },
            Indicator::SlotLocked { tile: olo_protocol::Tile::BottomRight, locked: true, window: Some("Weather".into()) },
            Indicator::SlotLocked { tile: olo_protocol::Tile::Left, locked: false, window: Some("Kache".into()) },
            Indicator::DoNotDisturb { on: true },
            Indicator::DoNotDisturb { on: false },
        ];
        // A row for each look at each scale, 1× to 2×.
        let mut rows = Vec::new();
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for mode in [olo_theme::ThemeMode::Light, olo_theme::ThemeMode::Dark] {
                theme::set(olo_theme::looks::palette(olo_theme::looks::DEFAULT, mode));
                rows.push(inds.iter().map(|i| draw(i, scale)).collect::<Vec<_>>());
            }
        }
        let w = rows.iter().map(|r| r.iter().map(|pm| pm.width() + 16).sum::<u32>()).max().unwrap();
        let h = rows.iter().map(|r| r[0].height() + 16).sum::<u32>();
        let mut sheet = Pixmap::new(w, h).unwrap();
        sheet.fill(tiny_skia::Color::from_rgba8(51, 102, 152, 255));
        let mut y = 8;
        for row in &rows {
            let mut x = 8;
            for pm in row {
                sheet.draw_pixmap(x, y, pm.as_ref(), &Default::default(), tiny_skia::Transform::identity(), None);
                x += pm.width() as i32 + 16;
            }
            y += row[0].height() as i32 + 16;
        }
        sheet.save_png(out).unwrap();
    }
}
