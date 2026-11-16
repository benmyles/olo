//! A glance drawn: what an app gives Deskbar to show of it
//! (`olo_protocol::glance`), the same wherever it's shown (Deskbar's row,
//! a cell beside other widgets, the tray, a preview).
//!
//! - [`height`]: how tall it is at a size, from what it has.
//! - [`paint`]: at a size in a row: small, its headline on one line;
//!   medium or large, the headline (an icon, the value big, the title
//!   after it, the detail under), a note, then its body: slots across
//!   (each a label over an icon over a value, and its level as a small
//!   column in the body's hue), or spans down (each a label, an icon, and
//!   its low to its high as a bar on one scale, coloured cold to warm).
//! - [`paint_cell`]: narrow, beside others: its icon and value, its
//!   title under. [`paint_tray`]: an icon's square.
//!
//! Everything said is recorded for agents as it's drawn (a label each
//! line, a list item each slot and span), so they read it as it shows.
//! Positions are on whole device pixels at every scale (icons through
//! `style::snap_centred`, at their device size: never resampled).

use olo_interface::{Bounds, ContentMask, FontWeight, Hsla, Painter, Pixels, SharedString, fill, linear_color_stop, linear_gradient, point, px};
use olo_protocol::{
    deskbar::Size,
    glance::{Body, Glance, Slot, Span},
};
use olo_theme::{Hue, Palette};

use crate::{
    agent, icons,
    style::{self, Align},
    theme,
};

/// Above the headline, and under the last part.
const PAD: f32 = 4.0;
/// The headline's row, medium and large.
const HEAD: f32 = 40.0;
const HEAD_LARGE: f32 = 50.0;
/// The note's line.
const NOTE: f32 = 16.0;
/// Slots: their label, icon and value; and their levels' columns under.
const SLOTS: f32 = 46.0;
const LEVELS: f32 = 12.0;
/// A span's row.
const SPAN: f32 = 18.0;
/// A small glance's one line.
const LINE: f32 = 24.0;

fn head(size: Size) -> f32 {
    if size == Size::Large { HEAD_LARGE } else { HEAD }
}

/// How tall `glance` is drawn at `size` (one not come yet: its headline's).
pub fn height(glance: Option<&Glance>, size: Size) -> f32 {
    if size == Size::Small {
        return LINE;
    }
    let Some(gl) = glance else { return PAD + head(size) + PAD };
    let mut h = PAD + head(size);
    if gl.note.is_some() {
        h += NOTE;
    }
    match &gl.body {
        Some(Body::Slots { slots, .. }) => h += SLOTS + if slots.iter().any(|s| s.level.is_some()) { LEVELS } else { 0.0 },
        Some(Body::Spans { spans, .. }) => h += 2.0 + spans.len() as f32 * SPAN,
        None => {}
    }
    h + PAD
}

/// The colours it's drawn in: words, and quieter words still read
/// (`style::secondary_text`, 4.5:1 on the panel).
fn inks(p: &Palette) -> (Hsla, Hsla) {
    (style::c(p.panel_text), style::secondary_text(p))
}

fn said(g: &mut Painter, text: &str, bounds: Bounds<Pixels>) {
    if !text.is_empty() {
        agent::record(g, agent::Node { role: "Label", name: Some(SharedString::from(text.to_owned())), bounds, enabled: true, ..Default::default() });
    }
}

/// Paint `glance` at `size` in row `r`, its words `inset` from each side.
pub fn paint(g: &mut Painter, r: Bounds<Pixels>, glance: &Glance, size: Size, inset: f32) {
    let p = theme::palette();
    let (left, right) = (f32::from(r.left()) + inset, f32::from(r.right()) - inset);
    g.with_content_mask(Some(ContentMask { bounds: r }), |g| {
        if size == Size::Small {
            paint_line(g, left, right, f32::from(r.center().y), glance, &p);
            return;
        }
        let mut y = f32::from(r.top()) + PAD;
        paint_head(g, left, right, y, glance, size, &p);
        y += head(size);
        if let Some(note) = &glance.note {
            let (ink, _) = inks(&p);
            let b = style::label_fit(g, px(left), px(y + NOTE / 2.0), Align::Left, note.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), ink, px(right - left));
            said(g, note, b);
            y += NOTE;
        }
        match &glance.body {
            Some(Body::Slots { slots, hue }) => paint_slots(g, left, right, y, slots, *hue, size, &p),
            Some(Body::Spans { spans, min, max }) => paint_spans(g, left, right, y + 2.0, spans, *min, *max, &p),
            None => {}
        }
    });
}

/// The headline: its icon, its value big and its title after it, its
/// detail under them.
fn paint_head(g: &mut Painter, left: f32, right: f32, top: f32, gl: &Glance, size: Size, p: &Palette) {
    let (ink, dim) = (inks(p).0, inks(p).1);
    let (icon_size, value_size) = if size == Size::Large { (40.0, 26.0) } else { (32.0, 21.0) };
    let h = head(size);
    let mut x = left;
    if let Some(icon) = &gl.icon {
        let b = style::snap_centred(g, point(px(left + icon_size / 2.0), px(top + h / 2.0)), icon_size, icon_size);
        icons::paint(g, b, icon);
        x += icon_size + 6.0;
    }
    // Two lines: the value and title, the detail under.
    let first = top + h / 2.0 - if size == Size::Large { 8.0 } else { 7.0 };
    let second = top + h / 2.0 + if size == Size::Large { 13.0 } else { 11.0 };
    let bold = style::ui_font(FontWeight::BOLD);
    let plain = style::ui_font(FontWeight::NORMAL);
    let mut after = x;
    if !gl.value.is_empty() {
        let b = style::label_fit(g, px(x), px(first), Align::Left, gl.value.clone(), value_size, bold.clone(), ink, px((right - x).max(0.0)));
        said(g, &gl.value, b);
        after = f32::from(b.right()) + 5.0;
    }
    if !gl.title.is_empty() && after < right {
        // (On the value's baseline, not its middle: smaller words beside
        // big figures.)
        let y = first + (value_size - style::PLAIN_SIZE) * 0.36;
        let b = style::label_fit(g, px(after), px(y), Align::Left, gl.title.clone(), style::PLAIN_SIZE, bold, ink, px(right - after));
        said(g, &gl.title, b);
    }
    if !gl.detail.is_empty() {
        let b = style::label_fit(g, px(x), px(second), Align::Left, gl.detail.clone(), style::SMALL_SIZE, plain, dim, px((right - x).max(0.0)));
        said(g, &gl.detail, b);
    }
}

/// Small: the headline on one line (icon, value, title).
fn paint_line(g: &mut Painter, left: f32, right: f32, cy: f32, gl: &Glance, p: &Palette) {
    let (ink, dim) = inks(p);
    let mut x = left;
    if let Some(icon) = &gl.icon {
        let b = style::snap_centred(g, point(px(left + 8.0), px(cy)), 16.0, 16.0);
        icons::paint(g, b, icon);
        x += 16.0 + 5.0;
    }
    if !gl.value.is_empty() {
        let b = style::label_fit(g, px(x), px(cy), Align::Left, gl.value.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, px((right - x).max(0.0)));
        said(g, &gl.value, b);
        x = f32::from(b.right()) + 5.0;
    }
    if !gl.title.is_empty() && x < right {
        let b = style::label_fit(g, px(x), px(cy), Align::Left, gl.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, px(right - x));
        said(g, &gl.title, b);
    }
}

/// A hue by its name, or the accent.
fn hue_of(p: &Palette, hue: Option<Hue>) -> Hsla {
    style::c(hue.map_or(p.control_highlight, |h| p.hue(h)))
}

/// Slots across: each a label over an icon over a value; their levels'
/// columns under them, on a faint ground (0 shows as nothing on it).
#[allow(clippy::too_many_arguments)]
fn paint_slots(g: &mut Painter, left: f32, right: f32, top: f32, slots: &[Slot], hue: Option<Hue>, size: Size, p: &Palette) {
    if slots.is_empty() {
        return;
    }
    let (ink, dim) = inks(p);
    let plain = style::ui_font(FontWeight::NORMAL);
    let w = (right - left) / slots.len() as f32;
    let icon_size = if size == Size::Large { 24.0 } else { 20.0 };
    let levels = slots.iter().any(|s| s.level.is_some());
    let colour = hue_of(p, hue);
    let ground = style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.07));
    let s = g.scale_factor();
    for (i, slot) in slots.iter().enumerate() {
        let cx = left + w * (i as f32 + 0.5);
        let cell = Bounds::from_corners(point(px(left + w * i as f32), px(top)), point(px(left + w * (i + 1) as f32), px(top + SLOTS + if levels { LEVELS } else { 0.0 })));
        style::label_fit(g, px(cx), px(top + 6.0), Align::Center, slot.label.clone(), style::SMALL_SIZE, plain.clone(), dim, px(w));
        if let Some(icon) = &slot.icon {
            let b = style::snap_centred(g, point(px(cx), px(top + 13.0 + 10.0)), icon_size, icon_size);
            icons::paint(g, b, icon);
        }
        style::label_fit(g, px(cx), px(top + 39.0), Align::Center, slot.value.clone(), style::SMALL_SIZE, plain.clone(), ink, px(w));
        let mut value = slot.value.clone();
        if levels {
            // A column as wide as a finger's breadth of the slot, its
            // height the level, rounded at its top.
            let bw = ((w - 8.0).min(12.0) * s).round() / s;
            let bottom = top + SLOTS + LEVELS - 2.0;
            let full = LEVELS - 3.0;
            let track = style::snap(g, Bounds::from_corners(point(px(cx - bw / 2.0), px(bottom - full)), point(px(cx + bw / 2.0), px(bottom))));
            g.paint_quad(fill(track, ground).corner_radii(px(1.5)));
            if let Some(level) = slot.level.map(|l| l.clamp(0.0, 1.0)).filter(|l| *l > 0.0) {
                // (At least a device pixel or two: a small chance shows.)
                let h = (full * level).max(2.0 / s);
                let bar = style::snap(g, Bounds::from_corners(point(px(cx - bw / 2.0), px(bottom - h)), point(px(cx + bw / 2.0), px(bottom))));
                g.paint_quad(fill(bar, colour).corner_radii(px(1.5)));
                value = format!("{value}, {}%", (level * 100.0).round());
            }
        }
        agent::record(g, agent::Node { role: "ListItem", name: Some(slot.label.clone().into()), value: Some(value.into()), bounds: cell, enabled: true, ..Default::default() });
    }
}

/// Where `v` lies on the scale from cold (0) to warm (1), as a colour:
/// blue, teal, yellow, orange.
fn warmth(p: &Palette, t: f32) -> olo_theme::Rgba {
    let stops = [(0.0, p.hue(Hue::Blue)), (0.35, p.hue(Hue::Teal)), (0.65, p.hue(Hue::Yellow)), (1.0, p.hue(Hue::Orange))];
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if t <= b {
            return olo_theme::mix(ca, cb, (t - a) / (b - a));
        }
    }
    stops[3].1
}

/// Spans down: each its label, its icon, its low, a bar from its low to
/// its high on the scale, its high.
#[allow(clippy::too_many_arguments)]
fn paint_spans(g: &mut Painter, left: f32, right: f32, top: f32, spans: &[Span], min: f32, max: f32, p: &Palette) {
    let (ink, dim) = inks(p);
    let plain = style::ui_font(FontWeight::NORMAL);
    let bold = style::ui_font(FontWeight::BOLD);
    // The columns: label, icon, low (to its right edge), bar, high.
    let label_w = spans.iter().map(|s| style::text_width_app(&s.label, style::SMALL_SIZE, &bold)).fold(0.0, f32::max).min(40.0) + 4.0;
    let figures = |f: &dyn Fn(&Span) -> &String| spans.iter().map(|s| style::text_width_app(f(s), style::SMALL_SIZE, &plain)).fold(0.0, f32::max);
    let (low_w, high_w) = (figures(&|s| &s.low_text), figures(&|s| &s.high_text));
    let icon_x = left + label_w;
    let low_right = icon_x + 16.0 + 4.0 + low_w;
    let (bar_left, bar_right) = (low_right + 5.0, right - high_w - 5.0);
    let range = (max - min).max(f32::EPSILON);
    let ground = style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.07));
    let s = g.scale_factor();
    let thick = ((4.0 * s).round() / s).max(2.0 / s);
    for (i, span) in spans.iter().enumerate() {
        let cy = top + SPAN * (i as f32 + 0.5);
        let row = Bounds::from_corners(point(px(left), px(top + SPAN * i as f32)), point(px(right), px(top + SPAN * (i + 1) as f32)));
        style::label_fit(g, px(left), px(cy), Align::Left, span.label.clone(), style::SMALL_SIZE, bold.clone(), ink, px(label_w - 2.0));
        if let Some(icon) = &span.icon {
            let b = style::snap_centred(g, point(px(icon_x + 8.0), px(cy)), 16.0, 16.0);
            icons::paint(g, b, icon);
        }
        style::label(g, px(low_right), px(cy), Align::Right, span.low_text.clone(), style::SMALL_SIZE, plain.clone(), dim);
        style::label(g, px(right - high_w), px(cy), Align::Left, span.high_text.clone(), style::SMALL_SIZE, plain.clone(), ink);
        if bar_right > bar_left + 4.0 {
            let track = style::snap_centred(g, point(px((bar_left + bar_right) / 2.0), px(cy)), bar_right - bar_left, thick);
            g.paint_quad(fill(track, ground).corner_radii(px(thick / 2.0)));
            let at = |v: f32| ((v - min) / range).clamp(0.0, 1.0);
            let (a, b) = (at(span.low.min(span.high)), at(span.high.max(span.low)));
            let x0 = f32::from(track.left()) + a * f32::from(track.size.width);
            let x1 = (f32::from(track.left()) + b * f32::from(track.size.width)).max(x0 + thick);
            let seg = style::snap(g, Bounds::from_corners(point(px(x0), track.top()), point(px(x1), track.bottom())));
            let paint = linear_gradient(90.0, linear_color_stop(style::c(warmth(p, a)), 0.0), linear_color_stop(style::c(warmth(p, b)), 1.0));
            g.paint_quad(fill(seg, paint).corner_radii(px(thick / 2.0)));
        }
        let value = format!("{} to {}", span.low_text, span.high_text);
        agent::record(g, agent::Node { role: "ListItem", name: Some(span.label.clone().into()), value: Some(value.into()), bounds: row, enabled: true, ..Default::default() });
    }
}

/// Narrow, beside other widgets: its icon and value on the first line, its
/// title under.
pub fn paint_cell(g: &mut Painter, r: Bounds<Pixels>, glance: &Glance) {
    let p = theme::palette();
    let (ink, dim) = inks(&p);
    let (left, right) = (f32::from(r.left()), f32::from(r.right()));
    let first = f32::from(r.top()) + 4.0 + 7.0;
    g.with_content_mask(Some(ContentMask { bounds: r }), |g| {
        let mut x = left;
        if let Some(icon) = &glance.icon {
            let b = style::snap_centred(g, point(px(left + 8.0), px(first)), 16.0, 16.0);
            icons::paint(g, b, icon);
            x += 16.0 + 4.0;
        }
        let b = style::label_fit(g, px(x), px(first), Align::Left, glance.value.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, px((right - x).max(0.0)));
        said(g, &glance.value, b);
        let b = style::label_fit(g, px(left), px(first + 15.0), Align::Left, glance.title.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim, px(right - left));
        said(g, &glance.title, b);
    });
}

/// In the tray, an icon's square: its icon (its value without one).
pub fn paint_tray(g: &mut Painter, r: Bounds<Pixels>, glance: &Glance) {
    match &glance.icon {
        Some(icon) => {
            let side = f32::from(r.size.width.min(r.size.height));
            let b = style::snap_centred(g, r.center(), side, side);
            icons::paint(g, b, icon);
        }
        None => {
            let (ink, _) = inks(&theme::palette());
            style::label(g, r.center().x, r.center().y, Align::Center, glance.value.clone(), 9.0, style::ui_font(FontWeight::BOLD), ink);
        }
    }
}

/// Run as app `app`'s provider (`olo_protocol::glance`: no window; Deskbar
/// starts it), until Deskbar asks for nothing or its folder goes: each
/// widget asked for made with `make` (all at once: they share what's
/// fetched) and written where Deskbar reads it, only when it changed;
/// then asleep until the requests change, a path in `watched` does (the
/// app's settings, its cache), or the time `make` gave comes. One a
/// folder at a time (a lock): a second ends at once.
///
/// `make` returns each widget's timeline by its id, and when to make them
/// again (None: when something changes).
pub fn serve(
    app: &str,
    watched: Vec<std::path::PathBuf>,
    mut make: impl FnMut(&[olo_protocol::glance::Request]) -> (Vec<(u32, olo_protocol::glance::Timeline)>, Option<std::time::Duration>),
) -> std::process::ExitCode {
    use std::{collections::HashMap, os::fd::AsRawFd as _, sync::mpsc, time::Duration};

    use olo_protocol::glance::{self as g, Requests};
    let dir = g::dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return std::process::ExitCode::FAILURE;
    }
    let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(g::lock_path(&dir, app)) else { return std::process::ExitCode::FAILURE };
    // SAFETY: flock on a file this holds open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return std::process::ExitCode::SUCCESS;
    }
    let _ = std::fs::write(g::lock_path(&dir, app), std::process::id().to_string());
    let (tx, rx) = mpsc::channel::<()>();
    let requests = g::requests_path(&dir, app);
    let tx = std::sync::Mutex::new(tx);
    // (Its own writes in the folder aren't news: the requests, the folder
    // going, and the app's own paths are.)
    let (asked_path, folder) = (requests.clone(), dir.clone());
    let _watch = olo_kit_base::watch::watch(std::iter::once(dir.clone()).chain(watched), move |changed| {
        if changed.iter().any(|p| *p == asked_path || *p == folder || !p.starts_with(&folder)) {
            let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(());
        }
    });
    let mut written: HashMap<u32, g::Timeline> = HashMap::new();
    loop {
        if !dir.is_dir() {
            break;
        }
        let asked: Requests = std::fs::read(&requests).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if asked.widgets.is_empty() {
            break;
        }
        let (made, again) = make(&asked.widgets);
        for (id, timeline) in made {
            if written.get(&id) != Some(&timeline) && g::write_json(&g::content_path(&dir, id), &timeline).is_ok() {
                written.insert(id, timeline);
            }
        }
        written.retain(|id, _| asked.widgets.iter().any(|r| r.widget == *id));
        // Asleep until something changes (a burst of changes at once: one
        // look, a moment after the first).
        let woke = match again {
            Some(d) => rx.recv_timeout(d).map_err(|e| matches!(e, mpsc::RecvTimeoutError::Disconnected)),
            None => rx.recv().map_err(|_| true),
        };
        if woke == Err(true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        while rx.try_recv().is_ok() {}
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use olo_protocol::glance::{Body, Glance, Slot, Span};

    use super::*;

    #[test]
    fn heights_follow_what_it_has() {
        let mut gl = Glance { value: "44°".into(), ..Glance::default() };
        assert_eq!(height(None, Size::Small), LINE);
        assert_eq!(height(Some(&gl), Size::Small), LINE, "small: one line, whatever it has");
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + PAD);
        assert_eq!(height(None, Size::Medium), height(Some(&gl), Size::Medium), "not come yet: as tall as its headline");
        gl.note = Some("Rain stopping in 34 minutes".into());
        gl.body = Some(Body::Slots { slots: vec![Slot { label: "14".into(), ..Slot::default() }; 6], hue: None });
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + NOTE + SLOTS + PAD);
        gl.body = Some(Body::Slots { slots: vec![Slot { level: Some(0.4), ..Slot::default() }; 6], hue: None });
        assert_eq!(height(Some(&gl), Size::Large), PAD + HEAD_LARGE + NOTE + SLOTS + LEVELS + PAD);
        gl.note = None;
        gl.body = Some(Body::Spans { spans: vec![Span::default(); 7], min: 0.0, max: 1.0 });
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + 2.0 + 7.0 * SPAN + PAD);
    }

    #[test]
    fn warmth_runs_blue_to_orange() {
        let p = theme::palette();
        assert_eq!(warmth(&p, 0.0), p.hue(Hue::Blue));
        assert_eq!(warmth(&p, 1.0), p.hue(Hue::Orange));
        assert_eq!(warmth(&p, 2.0), p.hue(Hue::Orange), "past the end: the end's");
    }
}
