//! Signatures: drawn once on a pad (trackpad or mouse), kept in
//! ~/.config/olo/signatures.json, and placed on any PDF from the Sign
//! menu.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use olo_kit::{
    egui::{self, Align, Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::Button,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Signature {
    /// Strokes normalised to the signature's bounds (0..1, y up).
    pub strokes: Vec<Vec<[f32; 2]>>,
    /// Width over height.
    pub aspect: f32,
    /// Seconds since the epoch, for its name in the menu.
    pub created: u64,
}

fn store_path() -> PathBuf {
    olo_kit::config_dir().join("signatures.json")
}

pub fn load() -> Vec<Signature> {
    std::fs::read(store_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Save the signatures (readable only by the user).
pub fn store(all: &[Signature]) {
    use std::os::unix::fs::PermissionsExt;
    let path = store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_vec_pretty(all) else { return };
    if let Err(e) = crate::pdfwork::write_atomic(&path, &json) {
        tracing::warn!("saving signatures: {e}");
        return;
    }
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
}

/// A name for the menu: "Signature from 25 Oct 2026, 14:03".
pub fn label(s: &Signature, n: usize) -> String {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(s.created);
    format!("Signature {} — {}", n + 1, olo_kit::files::format_time(Some(t)))
}

/// Normalise pad strokes (screen points, y down) into a signature.
pub fn from_strokes(strokes: &[Vec<Pos2>]) -> Option<Signature> {
    let pts = strokes.iter().flatten();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    if x0 > x1 {
        return None;
    }
    // Pad a little so the stroke width isn't clipped, and keep a dot or a
    // straight line from collapsing to zero size.
    let (w, h) = ((x1 - x0).max(4.0) + 4.0, (y1 - y0).max(4.0) + 4.0);
    let (x0, y0) = (x0 - 2.0, y0 - 2.0);
    let strokes = strokes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.iter().map(|p| [(p.x - x0) / w, 1.0 - (p.y - y0) / h]).collect())
        .collect();
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Some(Signature { strokes, aspect: w / h, created })
}

pub enum PadResult {
    Done(Signature),
    Cancel,
}

/// The New Signature window: sign on the line, then Done.
#[derive(Default)]
pub struct Pad {
    strokes: Vec<Vec<Pos2>>,
    drawing: bool,
}

pub const PAD_SIZE: (u32, u32) = (480, 250);

impl Pad {
    pub fn ui(&mut self, ui: &mut Ui) -> Option<PadResult> {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let buttons_top = full.bottom() - m::CONTROL_HEIGHT;
        let pad = Rect::from_min_max(full.min, pos2(full.right(), buttons_top - m::WINDOW_INSET));
        let painter = ui.painter().clone();
        // Paper: always white with dark ink, whatever the theme, since it
        // stands for the page.
        painter.rect_filled(pad, 0.0, Color32::WHITE);
        style::sunken(&painter, pad.expand(2.0), &p, false);
        let base_y = pad.bottom() - pad.height() * 0.28;
        let line_l = pad.left() + 28.0;
        style::hline(&painter, line_l, pad.right() - 20.0, base_y, Color32::from_gray(170));
        style::label(&painter, pad.left() + 12.0, base_y - 8.0, Align::Min, "×", style::plain_font(), Color32::from_gray(150));
        if self.strokes.is_empty() {
            style::label(
                &painter,
                pad.center().x,
                pad.bottom() - pad.height() * 0.14,
                Align::Center,
                "Sign on the line with the trackpad or mouse",
                style::plain_font(),
                Color32::from_gray(140),
            );
        }
        let resp = ui.interact(pad, ui.make_persistent_id("pad"), Sense::drag()).on_hover_cursor(egui::CursorIcon::Crosshair);
        if resp.drag_started() {
            self.strokes.push(Vec::new());
            self.drawing = true;
        }
        if self.drawing
            && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
            && let Some(s) = self.strokes.last_mut()
        {
            let pos = pos.clamp(pad.min, pad.max);
            if s.last().is_none_or(|l| l.distance(pos) > 0.8) {
                s.push(pos);
            }
        }
        if resp.drag_stopped() {
            self.drawing = false;
        }
        let ink = Color32::from_rgb(10, 20, 60);
        let clipped = painter.with_clip_rect(pad);
        for s in &self.strokes {
            match s.len() {
                0 => {}
                1 => {
                    clipped.circle_filled(s[0], 1.2, ink);
                }
                _ => {
                    clipped.add(egui::Shape::line(s.clone(), Stroke::new(2.4, ink)));
                }
            }
        }
        // Clear on the left; Cancel and Done on the right.
        let mut result = None;
        let bw = m::BUTTON_MIN_WIDTH;
        let row = |x: f32| Rect::from_min_size(pos2(x, buttons_top), vec2(bw, m::CONTROL_HEIGHT));
        if ui.put(row(full.left()), Button::new("Clear")).clicked() {
            self.strokes.clear();
        }
        if ui.put(row(full.right() - 2.0 * bw - m::SPACING), Button::new("Cancel")).clicked()
            || ui.input(|i| i.key_pressed(egui::Key::Escape))
        {
            result = Some(PadResult::Cancel);
        }
        let done = ui.add_enabled_ui(!self.strokes.is_empty(), |ui| ui.put(row(full.right() - bw), Button::new("Done").default(true)));
        if done.inner.clicked()
            && let Some(sig) = from_strokes(&self.strokes)
        {
            result = Some(PadResult::Done(sig));
        }
        let _ = c;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises() {
        let s = from_strokes(&[vec![pos2(10.0, 50.0), pos2(110.0, 10.0)]]).unwrap();
        assert!((s.aspect - 104.0 / 44.0).abs() < 1e-3);
        let [a, b] = [s.strokes[0][0], s.strokes[0][1]];
        assert!(a[0] < 0.05 && a[1] < 0.1, "{a:?}");
        assert!(b[0] > 0.95 && b[1] > 0.9, "{b:?}");
        assert!(from_strokes(&[]).is_none());
    }
}
