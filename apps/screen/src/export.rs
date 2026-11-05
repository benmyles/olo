//! A screen's history as an MP4 video, made in process: the frames
//! (`record`) at 10 a second, H.264 into an MP4 (`olo_video`, shared
//! with the screen recorder). Stretches with nothing happening (no action and no
//! frame changing for over `QUIET_MS`) are cut, each replaced by a card
//! saying how long it was ("4 min 12 s with nothing happening"). Run on a
//! worker: `progress` is told how far it has got and says whether to go
//! on.

use std::path::Path;

use crate::{
    history,
    record::{self, Frame},
};

const FPS: u64 = 10;
const STEP_MS: u64 = 1000 / FPS;
/// Longer than this with nothing happening is cut.
const QUIET_MS: u64 = 10_000;
/// Kept either side of what happens, around a cut.
const LEAD_MS: u64 = 1500;
const CARD_MS: u64 = 2000;

/// One piece of the video: the screen from `from` to `to`, or a card for
/// a cut of `ms`.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Screen { from: u64, to: u64 },
    Card { ms: u64 },
}

/// What happens when: the actions' starts and the frames' times, cut
/// into pieces where it goes quiet.
fn pieces(events: &mut [u64]) -> Vec<Piece> {
    events.sort_unstable();
    let (Some(&first), Some(&last)) = (events.first(), events.last()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut from = first.saturating_sub(LEAD_MS / 3);
    for w in events.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b - a > QUIET_MS {
            out.push(Piece::Screen { from, to: a + LEAD_MS });
            out.push(Piece::Card { ms: (b - LEAD_MS) - (a + LEAD_MS) });
            from = b - LEAD_MS;
        }
    }
    out.push(Piece::Screen { from, to: last + LEAD_MS });
    out
}

/// "4 min 12 s".
fn span(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, s / 60 % 60),
    }
}

/// Make screen `name`'s history into `out`. `progress(0..1)` false
/// stops it (and removes what was written).
pub fn export(name: &str, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<(), String> {
    let actions = history::read(name);
    let segments = record::index(name);
    let mut events: Vec<u64> = actions.iter().map(|a| a.at).chain(segments.iter().flat_map(|s| s.frames.iter().map(|f| f.at))).collect();
    let plan = pieces(&mut events);
    if plan.is_empty() {
        return Err("nothing has been done on this screen".into());
    }
    let first = segments.iter().flat_map(|s| s.frames.first()).map(|f| f.at).next().ok_or("no frames were kept")?;
    let (_, probe) = record::frame_at(&segments, first).ok_or("no frames were kept")?;
    // H.264 wants even sizes.
    let (w, h) = (probe.width & !1, probe.height & !1);
    let total: u64 = plan
        .iter()
        .map(|p| match p {
            Piece::Screen { from, to } => (to - from) / STEP_MS,
            Piece::Card { .. } => CARD_MS / STEP_MS,
        })
        .sum::<u64>()
        .max(1);
    let mut encoder = olo_video::H264::new(w as u32, h as u32, FPS as u32)?;
    let tmp = out.with_extension("mp4.part");
    // (Each frame a tenth of a second: FPS × 1000 ticks a second, 1000 a frame.)
    let mut mp4 = olo_video::Mp4::create(&tmp, w as u32, h as u32, FPS as u32 * 1000).map_err(|e| e.to_string())?;
    let mut done = 0u64;
    let mut last = probe;
    for p in &plan {
        let frames: Box<dyn Iterator<Item = Frame>> = match p {
            Piece::Screen { from, to } => {
                let mut player = record::Player::new(&segments);
                let mut keep = last.clone();
                Box::new((*from..*to).step_by(STEP_MS as usize).map(move |t| {
                    if let Some(f) = player.at(t) {
                        keep = f.clone();
                    }
                    keep.clone()
                }))
            }
            Piece::Card { ms } => {
                let card = card(w, h, &format!("{} with nothing happening", span(*ms)));
                Box::new(std::iter::repeat_n(card, (CARD_MS / STEP_MS) as usize))
            }
        };
        for f in frames {
            let even = crop(&f, w, h);
            let (bits, key) = encoder.encode(&even, w * 4, olo_video::Order::Rgba)?;
            mp4.add(&bits, key, 1000).map_err(|e| e.to_string())?;
            if matches!(p, Piece::Screen { .. }) {
                last = f;
            }
            done += 1;
            if !progress(done as f32 / total as f32) {
                drop(mp4);
                let _ = std::fs::remove_file(&tmp);
                return Err("cancelled".into());
            }
        }
    }
    mp4.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// `f` as exactly `w` × `h` RGBA (cut at the right and bottom, or
/// padded with black).
fn crop(f: &Frame, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h.min(f.height) {
        let n = w.min(f.width) * 4;
        out[y * w * 4..y * w * 4 + n].copy_from_slice(&f.rgba[y * f.width * 4..y * f.width * 4 + n]);
    }
    out
}

/// Olo's UI face (Olo's own fonts first, then the system's): its file's
/// bytes and the face's index in it, read once.
fn ui_font() -> Option<&'static (Vec<u8>, u32)> {
    static FONT: std::sync::OnceLock<Option<(Vec<u8>, u32)>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        let query = fontdb::Query { families: &[fontdb::Family::Name("Noto Sans"), fontdb::Family::SansSerif], weight: fontdb::Weight::NORMAL, ..Default::default() };
        let mut own = fontdb::Database::new();
        if let Some(dir) = olo_xdg::fonts_dir() {
            own.load_fonts_dir(dir);
        }
        let mut system = fontdb::Database::new();
        system.load_system_fonts();
        [own, system].into_iter().find_map(|db| {
            let id = db.query(&query)?;
            db.with_face_data(id, |data, index| (data.to_vec(), index))
        })
    })
    .as_ref()
}

/// A dark card with `text` in the middle, in the kit's own type.
fn card(w: usize, h: usize, text: &str) -> Frame {
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
    let mut rgba = [28u8, 30, 36, 255].repeat(w * h);
    if let Some(data) = ui_font()
        && let Ok(font) = FontRef::try_from_slice_and_index(&data.0, data.1)
    {
        let scale = PxScale::from((h as f32 / 18.0).max(16.0));
        let sf = font.as_scaled(scale);
        let width: f32 = text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum();
        let mut x = (w as f32 - width) / 2.0;
        let baseline = h as f32 / 2.0 + sf.ascent() / 2.5;
        for c in text.chars() {
            let g = font.glyph_id(c).with_scale_and_position(scale, ab_glyph::point(x, baseline));
            x += sf.h_advance(font.glyph_id(c));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let i = (py as usize * w + px as usize) * 4;
                        for k in 0..3 {
                            let bg = rgba[i + k] as f32;
                            rgba[i + k] = (bg + (235.0 - bg) * cov.clamp(0.0, 1.0)) as u8;
                        }
                    }
                });
            }
        }
    }
    Frame { width: w, height: h, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_stretches_become_cards() {
        let mut t = vec![1_000, 2_000, 3_000, 300_000, 301_000];
        let p = pieces(&mut t);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Piece::Screen { from: 500, to: 4_500 });
        assert_eq!(p[1], Piece::Card { ms: 298_500 - 4_500 });
        assert_eq!(p[2], Piece::Screen { from: 298_500, to: 302_500 });
        assert_eq!(span(252_000), "4 min 12 s");
        assert!(pieces(&mut []).is_empty());
    }
}
