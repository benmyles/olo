//! Signatures: drawn once on a pad (trackpad or mouse), kept in
//! ~/.config/olo/signatures.json, and placed on any PDF from the Sign
//! menu.

use std::path::PathBuf;

use olo_ui::{
    agent,
    gpui::{
        self, Context, CursorStyle, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Point,
        Render, Styled as _, Window, canvas, div, point, px, rgb,
    },
    metrics as m,
    style::{self, Align},
    theme,
    widgets::Button,
};
use serde::{Deserialize, Serialize};

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
    olo_ui::config_dir().join("signatures.json")
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
    format!("Signature {} — {}", n + 1, olo_ui::files::format_time(Some(t)))
}

/// Normalise pad strokes (points, y down) into a signature.
pub fn from_strokes(strokes: &[Vec<[f32; 2]>]) -> Option<Signature> {
    let pts = strokes.iter().flatten();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    if x0 > x1 {
        return None;
    }
    // Pad a little so the stroke width isn't clipped, and keep a dot or a
    // straight line from collapsing to zero size.
    let (w, h) = ((x1 - x0).max(4.0) + 4.0, (y1 - y0).max(4.0) + 4.0);
    let (x0, y0) = (x0 - 2.0, y0 - 2.0);
    let strokes = strokes.iter().filter(|s| !s.is_empty()).map(|s| s.iter().map(|p| [(p[0] - x0) / w, 1.0 - (p[1] - y0) / h]).collect()).collect();
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Some(Signature { strokes, aspect: w / h, created })
}

/// The New Signature window: sign on the line, then Done (`done` gets
/// the signature, or None for Cancel).
pub struct Pad {
    strokes: Vec<Vec<[f32; 2]>>,
    drawing: bool,
    /// Where the pad was drawn (window points).
    area: gpui::Bounds<gpui::Pixels>,
    focus: FocusHandle,
    done: Option<Box<dyn FnOnce(Option<Signature>, &mut Window, &mut gpui::App)>>,
}

pub const PAD_SIZE: (f32, f32) = (480.0, 250.0);
/// The ink on the pad: always dark on white, whatever the look, since it
/// stands for the page.
const PAD_INK: u32 = 0x0a143c;

impl Pad {
    pub fn new(cx: &mut Context<Self>, done: impl FnOnce(Option<Signature>, &mut Window, &mut gpui::App) + 'static) -> Pad {
        Pad { strokes: Vec::new(), drawing: false, area: Default::default(), focus: cx.focus_handle(), done: Some(Box::new(done)) }
    }

    fn finish(&mut self, sig: Option<Signature>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(done) = self.done.take() {
            done(sig, window, cx);
        }
    }

    fn add(&mut self, at: Point<gpui::Pixels>) {
        let a = self.area;
        let x = f32::from(at.x.clamp(a.left(), a.right()) - a.left());
        let y = f32::from(at.y.clamp(a.top(), a.bottom()) - a.top());
        if let Some(s) = self.strokes.last_mut()
            && s.last().is_none_or(|l| (l[0] - x).hypot(l[1] - y) > 0.8)
        {
            s.push([x, y]);
        }
    }
}

impl Render for Pad {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) && self.done.is_some() {
            window.focus(&self.focus);
        }
        let me = cx.entity();
        let strokes = self.strokes.clone();
        let paper = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                me.update(cx, |pad, _| pad.area = b);
                window.paint_quad(gpui::fill(b, gpui::white()));
                let well = gpui::Bounds::from_corners(b.origin - point(px(2.0), px(2.0)), b.bottom_right() + point(px(2.0), px(2.0)));
                style::sunken(window, well, &p, false);
                let base_y = b.bottom() - b.size.height * 0.28;
                style::hline(window, b.left() + px(28.0), b.right() - px(20.0), base_y, rgb(0xaaaaaa).into());
                style::label(window, cx, b.left() + px(12.0), base_y - px(8.0), Align::Left, "×", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), rgb(0x969696).into());
                if strokes.is_empty() {
                    let hint = "Sign on the line with the trackpad or mouse";
                    let at =
                        style::label(window, cx, b.center().x, b.bottom() - b.size.height * 0.14, Align::Center, hint, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), rgb(0x8c8c8c).into());
                    agent::record(window, cx, agent::Node::label(hint, at));
                }
                let ink: gpui::Hsla = rgb(PAD_INK).into();
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    for s in &strokes {
                        let at = |p: &[f32; 2]| point(b.left() + px(p[0]), b.top() + px(p[1]));
                        match s.len() {
                            0 => {}
                            1 => window.paint_sector(at(&s[0]), px(0.0), px(1.2), 0.0, std::f32::consts::TAU, ink),
                            _ => {
                                for w in s.windows(2) {
                                    window.paint_segment(at(&w[0]), at(&w[1]), px(2.4), true, ink);
                                }
                            }
                        }
                    }
                });
                agent::record(window, cx, agent::Node { role: "Canvas", name: Some("Signature".into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
            },
        )
        .size_full();
        let pad_area = div()
            .id("pad")
            .flex_1()
            .min_h_0()
            .cursor(CursorStyle::Crosshair)
            .child(paper)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pad, e: &MouseDownEvent, _, cx| {
                    pad.strokes.push(Vec::new());
                    pad.drawing = true;
                    pad.add(e.position);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|pad, e: &MouseMoveEvent, _, cx| {
                if pad.drawing && e.pressed_button == Some(MouseButton::Left) {
                    pad.add(e.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|pad, _: &MouseUpEvent, _, _| pad.drawing = false));
        let (clear, cancel, done) = (cx.entity(), cx.entity(), cx.entity());
        let signed = !self.strokes.is_empty();
        let buttons = div()
            .flex()
            .items_center()
            .h(px(m::CONTROL_HEIGHT))
            .child(Button::new("Clear").on_click(move |_, cx| {
                clear.update(cx, |pad, cx| {
                    pad.strokes.clear();
                    cx.notify();
                })
            }))
            .child(div().flex_1())
            .child(Button::new("Cancel").on_click(move |window, cx| cancel.update(cx, |pad, cx| pad.finish(None, window, cx))))
            .child(div().w(px(m::SPACING)))
            .child(Button::new("Done").default(true).enabled(signed).on_click(move |window, cx| {
                done.update(cx, |pad, cx| {
                    if let Some(sig) = from_strokes(&pad.strokes) {
                        pad.finish(Some(sig), window, cx);
                    }
                })
            }));
        div()
            .size_full()
            .track_focus(&self.focus)
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::WINDOW_INSET))
            .on_key_down(cx.listener(|pad, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    pad.finish(None, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(pad_area)
            .child(buttons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises() {
        let s = from_strokes(&[vec![[10.0, 50.0], [110.0, 10.0]]]).unwrap();
        assert!((s.aspect - 104.0 / 44.0).abs() < 1e-3);
        let [a, b] = [s.strokes[0][0], s.strokes[0][1]];
        assert!(a[0] < 0.05 && a[1] < 0.1, "{a:?}");
        assert!(b[0] > 0.95 && b[1] > 0.9, "{b:?}");
        assert!(from_strokes(&[]).is_none());
    }
}
