//! Display: an arrangement map of the connected screens (click one to pick
//! it, drag one to move it; it snaps edge to edge with the others), and the
//! chosen screen's resolution, refresh rate and scale. Changes apply at
//! once through app_server's ConfigureOutput; a new resolution reverts by
//! itself unless kept within `KEEP_SECS`, in case the screen went dark.

use std::time::{Duration, Instant};

use olo_kit::{
    egui::{Align2, Color32, Rect, Sense, Ui, Vec2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{BBox, Button, CheckBox, MenuField, Slider, StringView},
};
use olo_protocol::{Mode, OutputConfig, OutputInfo, PowerStatus, Settings};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    selected: Option<String>,
    /// The screen whose brightness is being dragged, and the level
    /// (app_server's echo lags a little).
    brightness: Option<(String, f32, std::time::Instant)>,
    /// The screen being dragged in the map, and how far (points).
    drag: Option<(String, Vec2)>,
    /// A resolution change waiting to be kept: the screen, its previous
    /// mode, and when it changed.
    confirm: Option<(String, Mode, Instant)>,
}

/// How long a new resolution has to be kept before it reverts.
pub const KEEP_SECS: u64 = 15;

/// A logical rectangle (x, y, w, h).
type LRect = (f64, f64, f64, f64);

fn overlaps(a: LRect, b: LRect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Where a screen of size (w, h) dropped at (x, y) goes: against the
/// nearest edge of another screen, sharing at least a strip of that edge,
/// lined up with its edges or centre when close, and never overlapping.
pub fn snap((x, y, w, h): LRect, others: &[LRect]) -> Option<(i32, i32)> {
    const SHARE: f64 = 64.0;
    let align = 0.06 * w.max(h);
    let near = |v: f64, targets: [f64; 3]| {
        targets
            .into_iter()
            .filter(|t| (v - t).abs() < align)
            .min_by(|a, b| (v - a).abs().total_cmp(&(v - b).abs()))
            .unwrap_or(v)
    };
    let mut best: Option<((f64, f64), f64)> = None;
    for &(ox, oy, ow, oh) in others {
        let yy = near(y.clamp(oy - h + SHARE, oy + oh - SHARE), [oy, oy + oh - h, oy + (oh - h) / 2.0]);
        let xx = near(x.clamp(ox - w + SHARE, ox + ow - SHARE), [ox, ox + ow - w, ox + (ow - w) / 2.0]);
        for cand in [(ox + ow, yy), (ox - w, yy), (xx, oy + oh), (xx, oy - h)] {
            if others.iter().any(|&o| overlaps((cand.0, cand.1, w, h), o)) {
                continue;
            }
            let d = (cand.0 - x).hypot(cand.1 - y);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((cand, d));
            }
        }
    }
    best.map(|((x, y), _)| (x.round() as i32, y.round() as i32))
}

pub const SCALES: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

fn mode_text(mode: &Mode) -> String {
    format!("{} × {}", mode.width, mode.height)
}

fn hz_text(refresh: i32) -> String {
    let hz = refresh as f64 / 1000.0;
    if (hz - hz.round()).abs() < 0.05 { format!("{hz:.0} Hz") } else { format!("{hz:.2} Hz") }
}

fn scale_text(scale: f64) -> String {
    let pct = (scale * 100.0).round() as i32;
    if (scale - 2.0).abs() < 1e-6 { "200% (Retina)".into() } else { format!("{pct}%") }
}

/// Logical size (what apps lay out in) of an output at its mode, scale
/// and rotation.
pub(crate) fn logical(o: &OutputInfo) -> (f64, f64) {
    let (w, h) = o.current_mode.as_ref().map_or((1920, 1080), |m| (m.width, m.height));
    let (w, h) = if o.rotation % 180 == 90 { (h, w) } else { (w, h) };
    (w as f64 / o.scale.max(0.1), h as f64 / o.scale.max(0.1))
}

const ROTATION_NAMES: [&str; 4] = ["Standard", "90°", "180°", "270°"];

/// The name shown for `o`, with its connector when another screen would
/// have the same name.
fn shown_name(o: &OutputInfo, all: &[OutputInfo]) -> String {
    let name = display_name(o);
    if all.iter().filter(|x| display_name(x) == name).count() > 1 { format!("{name} ({})", o.name) } else { name }
}

fn display_name(o: &OutputInfo) -> String {
    o.display_name()
}

pub fn ui(ui: &mut Ui, s: &mut Settings, outputs: &[OutputInfo], power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let p = style::palette(ui.ctx());
    if outputs.is_empty() {
        ui.add(StringView::new("No displays reported by app_server.").dim(true));
        return;
    }
    if st.selected.as_ref().is_none_or(|n| !outputs.iter().any(|o| &o.name == n)) {
        st.selected = Some(outputs[0].name.clone());
    }

    let all = outputs;
    // Mirrors show another screen's picture: they aren't in the arrangement.
    let placed: Vec<OutputInfo> = outputs.iter().filter(|o| o.mirror_of.is_none()).cloned().collect();
    let outputs = &placed[..];

    // Arrangement: every output to scale, in its logical position.
    let map_h = 150.0;
    let (map, _) = ui.allocate_exact_size(vec2(ui.available_width(), map_h), Sense::hover());
    ui.painter().rect_filled(map, 0.0, c(p.document_background));
    style::sunken(ui.painter(), map, &p, false);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for o in outputs {
        let (w, h) = logical(o);
        x0 = x0.min(o.position.0 as f64);
        y0 = y0.min(o.position.1 as f64);
        x1 = x1.max(o.position.0 as f64 + w);
        y1 = y1.max(o.position.1 as f64 + h);
    }
    let inner = map.shrink(16.0);
    let k = (inner.width() as f64 / (x1 - x0)).min(inner.height() as f64 / (y1 - y0)) as f32;
    let off = inner.center() - vec2((x1 - x0) as f32 * k, (y1 - y0) as f32 * k) / 2.0;
    let rect_of = |o: &OutputInfo| {
        let (w, h) = logical(o);
        Rect::from_min_size(
            off + vec2((o.position.0 as f64 - x0) as f32 * k, (o.position.1 as f64 - y0) as f32 * k),
            vec2(w as f32 * k, h as f32 * k),
        )
        .shrink(1.0)
    };
    let mut dropped = None;
    // The dragged screen is drawn last, over the others.
    let mut order: Vec<&OutputInfo> = outputs.iter().collect();
    order.sort_by_key(|o| st.drag.as_ref().is_some_and(|(n, _)| *n == o.name));
    for o in order {
        let mut r = rect_of(o);
        let chosen = st.selected.as_deref() == Some(&o.name);
        let sense = if outputs.len() > 1 { Sense::click_and_drag() } else { Sense::click() };
        let resp = ui.interact(r, ui.make_persistent_id(("display", &o.name)), sense);
        if resp.clicked() || resp.drag_started() {
            st.selected = Some(o.name.clone());
        }
        if resp.dragged() {
            let d = st.drag.get_or_insert_with(|| (o.name.clone(), Vec2::ZERO));
            d.1 += resp.drag_delta();
        }
        if let Some((n, d)) = &st.drag
            && *n == o.name
        {
            r = r.translate(*d).intersect(map.shrink(2.0));
            if resp.drag_stopped() {
                dropped = Some((o.name.clone(), *d));
            }
        }
        let [rr, gg, bb] = s.workspace_color(0);
        ui.painter().rect_filled(r, 0.0, Color32::from_rgb(rr, gg, bb));
        let border = if chosen { c(p.keyboard_navigation) } else { c(p.frame_focused[0]) };
        let stroke_w = if chosen { 2.0 } else { 1.0 };
        ui.painter().rect_stroke(r, 0.0, (stroke_w, border), olo_kit::egui::StrokeKind::Inside);
        // A menu bar strip on the main display, where Deskbar goes.
        if o.main {
            let bar = Rect::from_min_size(r.min + vec2(stroke_w, stroke_w), vec2(r.width() - 2.0 * stroke_w, 4.0));
            ui.painter().rect_filled(bar, 0.0, Color32::from_white_alpha(200));
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, display_name(o), style::plain_font(), Color32::WHITE);
    }
    if let Some((name, d)) = dropped {
        st.drag = None;
        rearrange(outputs, &name, d, k, server);
    }
    for m in all.iter().filter(|o| o.mirror_of.is_some()) {
        let source = all.iter().find(|x| Some(&x.name) == m.mirror_of.as_ref()).map(|x| shown_name(x, all)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add(StringView::new(&format!("{} mirrors {source}.", shown_name(m, all))).dim(true));
            if st.selected.as_deref() != Some(&m.name) && ui.add(Button::new("Settings")).clicked() {
                st.selected = Some(m.name.clone());
            }
        });
    }

    let outputs = all;
    let Some(o) = outputs.iter().find(|o| st.selected.as_deref() == Some(&o.name)) else {
        return;
    };
    let label_w = 90.0;
    BBox::new(&shown_name(o, outputs)).show(ui, |ui| {
        // Resolutions, largest first; the refresh rate is a separate choice.
        let mut sizes: Vec<(i32, i32)> = o.modes.iter().map(|m| (m.width, m.height)).collect();
        sizes.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.0.cmp(&a.0)));
        sizes.dedup();
        let current = o.current_mode.clone().or_else(|| o.modes.first().cloned());
        let Some(current) = current else {
            ui.add(StringView::new("This display reports no modes.").dim(true));
            return;
        };
        let names: Vec<String> =
            sizes.iter().map(|&(w, h)| mode_text(&Mode { width: w, height: h, refresh: 0 })).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut i = sizes.iter().position(|&(w, h)| (w, h) == (current.width, current.height)).unwrap_or(0);
        let before = i;
        ui.add(MenuField::new(("res", &o.name), &mut i, &names).label("Resolution:").divider(label_w));
        let mut chosen = current.clone();
        if i != before {
            let (w, h) = sizes[i];
            // Keep the refresh rate if the new size offers it, else the fastest.
            chosen = o
                .modes
                .iter()
                .filter(|m| (m.width, m.height) == (w, h))
                .min_by_key(|m| ((m.refresh - current.refresh).abs(), -m.refresh))
                .cloned()
                .unwrap_or(chosen);
        }
        let mut rates: Vec<i32> = o
            .modes
            .iter()
            .filter(|m| (m.width, m.height) == (chosen.width, chosen.height))
            .map(|m| m.refresh)
            .collect();
        rates.sort_by(|a, b| b.cmp(a));
        rates.dedup();
        let rate_names: Vec<String> = rates.iter().map(|&r| hz_text(r)).collect();
        let rate_names: Vec<&str> = rate_names.iter().map(String::as_str).collect();
        let mut ri = rates.iter().position(|&r| r == chosen.refresh).unwrap_or(0);
        ui.add_enabled_ui(rates.len() > 1, |ui| {
            ui.add(MenuField::new(("hz", &o.name), &mut ri, &rate_names).label("Refresh rate:").divider(label_w));
        });
        if let Some(&r) = rates.get(ri) {
            chosen.refresh = r;
        }
        if chosen != current {
            // Revert to what worked before, even after several changes.
            if st.confirm.as_ref().is_none_or(|(n, _, _)| *n != o.name) {
                st.confirm = Some((o.name.clone(), current.clone(), Instant::now()));
            } else if let Some(c) = st.confirm.as_mut() {
                c.2 = Instant::now();
            }
            server.configure_output(&o.name, OutputConfig { mode: Some(chosen.clone()), ..Default::default() });
        }

        // A mirror shows its source as it is: no scale or rotation of its own.
        let mut si = SCALES.iter().position(|&s| (s - o.scale).abs() < 0.01).unwrap_or(4);
        if o.mirror_of.is_none() {
            let scale_names: Vec<String> = SCALES.iter().map(|&s| scale_text(s)).collect();
            let scale_names: Vec<&str> = scale_names.iter().map(String::as_str).collect();
            let before = si;
            ui.add(MenuField::new(("scale", &o.name), &mut si, &scale_names).label("Scale:").divider(label_w));
            if si != before {
                server.configure_output(&o.name, OutputConfig { scale: Some(SCALES[si]), ..Default::default() });
            }
            let mut ri = olo_protocol::ROTATIONS.iter().position(|&r| r == o.rotation).unwrap_or(0);
            let before = ri;
            ui.add(MenuField::new(("rotation", &o.name), &mut ri, &ROTATION_NAMES).label("Rotation:").divider(label_w));
            if ri != before {
                let rotation = Some(olo_protocol::ROTATIONS[ri]);
                server.configure_output(&o.name, OutputConfig { rotation, ..Default::default() });
            }
        }
        // Extend the desktop, or show another display's picture.
        if outputs.len() > 1 {
            let sources: Vec<&OutputInfo> = outputs.iter().filter(|x| x.name != o.name && x.mirror_of.is_none()).collect();
            let mut uses: Vec<String> = vec!["Extended desktop".into()];
            uses.extend(sources.iter().map(|x| format!("Mirror of {}", shown_name(x, outputs))));
            let uses: Vec<&str> = uses.iter().map(String::as_str).collect();
            let mut ui_i = o.mirror_of.as_ref().and_then(|m| sources.iter().position(|x| &x.name == m)).map_or(0, |i| i + 1);
            let before = ui_i;
            // A display others mirror can't mirror in turn.
            let mirrored = outputs.iter().any(|x| x.mirror_of.as_ref() == Some(&o.name));
            ui.add_enabled_ui(!mirrored, |ui| {
                ui.add(MenuField::new(("use", &o.name), &mut ui_i, &uses).label("Use as:").divider(label_w));
            });
            if ui_i != before {
                let mirror = if ui_i == 0 {
                    String::new()
                } else {
                    let x = sources[ui_i - 1];
                    if x.key.is_empty() { x.name.clone() } else { x.key.clone() }
                };
                server.configure_output(&o.name, OutputConfig { mirror: Some(mirror), ..Default::default() });
            }
        }
        if o.mirror_of.is_none() {
            let (cw, ch) = if o.rotation % 180 == 90 { (chosen.height, chosen.width) } else { (chosen.width, chosen.height) };
            let (lw, lh) = (cw as f64 / SCALES[si], ch as f64 / SCALES[si]);
            ui.horizontal(|ui| {
                ui.add_space(label_w + m::LABEL_GAP);
                ui.add(StringView::new(&format!("Looks like {lw:.0} × {lh:.0}")).dim(true));
            });
        }
        if outputs.len() > 1 && o.mirror_of.is_none() {
            ui.horizontal(|ui| {
                ui.add_space(label_w + m::LABEL_GAP);
                if o.main {
                    ui.add(StringView::new("Main display: Deskbar and the desktop are here.").dim(true));
                } else {
                    let mut main = false;
                    ui.add(CheckBox::new(&mut main, "Make this the main display"));
                    if main {
                        s.display.main = Some(if o.key.is_empty() { o.name.clone() } else { o.key.clone() });
                    }
                }
                ui.with_layout(olo_kit::egui::Layout::right_to_left(olo_kit::egui::Align::Center), |ui| {
                    if ui.add(Button::new("Identify")).clicked() {
                        server.identify_outputs();
                    }
                });
            });
        }

        // Each display whose brightness app_server can set (the built-in
        // one's backlight, monitors controlled over USB).
        let builtin = || power.and_then(|p| p.brightness).filter(|_| display_name(o) == "Built-in Display" || outputs.len() == 1);
        if let Some(level) = o.brightness.or_else(builtin) {
            if st.brightness.as_ref().is_some_and(|(_, _, at)| at.elapsed() > std::time::Duration::from_millis(600)) {
                st.brightness = None;
            }
            let mut v = st.brightness.as_ref().filter(|(n, ..)| *n == o.name).map_or(level, |(_, l, _)| *l);
            let before = v;
            let text = format!("{:.0}%", v * 100.0);
            ui.add(Slider::new(&mut v, 0.0..=1.0).step(1.0 / 64.0).label("Brightness").value_text(text));
            if v != before {
                st.brightness = Some((o.name.clone(), v, std::time::Instant::now()));
                server.set_output_brightness(&o.name, v);
            }
        }
    });
    keep_band(ui, outputs, server, st);
}

/// Move `name` by `delta` points in the map (`k` points per logical
/// pixel), snapped against the others. Every screen's position is sent so
/// the arrangement doesn't depend on which ones had been placed before.
fn rearrange(outputs: &[OutputInfo], name: &str, delta: Vec2, k: f32, server: &Server) {
    let Some(o) = outputs.iter().find(|o| o.name == name) else { return };
    let (w, h) = logical(o);
    let x = o.position.0 as f64 + (delta.x / k) as f64;
    let y = o.position.1 as f64 + (delta.y / k) as f64;
    let others: Vec<LRect> = outputs
        .iter()
        .filter(|x| x.name != name)
        .map(|x| {
            let (w, h) = logical(x);
            (x.position.0 as f64, x.position.1 as f64, w, h)
        })
        .collect();
    let Some(at) = snap((x, y, w, h), &others) else { return };
    if at == o.position {
        return;
    }
    for x in outputs.iter().filter(|x| x.name != name) {
        server.configure_output(&x.name, OutputConfig { position: Some(x.position), ..Default::default() });
    }
    server.configure_output(name, OutputConfig { position: Some(at), ..Default::default() });
}

/// "Keep this resolution?" with a countdown; reverts by itself.
fn keep_band(ui: &mut Ui, outputs: &[OutputInfo], server: &Server, st: &mut State) {
    let Some((name, previous, at)) = st.confirm.clone() else { return };
    let Some(o) = outputs.iter().find(|o| o.name == name) else {
        st.confirm = None;
        return;
    };
    let left = KEEP_SECS.saturating_sub(at.elapsed().as_secs());
    let revert = |st: &mut State| {
        server.configure_output(&name, OutputConfig { mode: Some(previous.clone()), ..Default::default() });
        st.confirm = None;
    };
    if left == 0 {
        revert(st);
        return;
    }
    ui.add_space(m::SPACING);
    ui.horizontal(|ui| {
        let _ = o;
        ui.add(StringView::new(&format!("Keep this resolution? Reverting in {left} s.")));
        ui.with_layout(olo_kit::egui::Layout::right_to_left(olo_kit::egui::Align::Center), |ui| {
            if ui.add(Button::new("Keep").default(true)).clicked() {
                st.confirm = None;
            }
            if ui.add(Button::new("Revert")).clicked() {
                revert(st);
            }
        });
    });
    ui.ctx().request_repaint_after(Duration::from_millis(250));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_edge_to_edge() {
        let laptop = (0.0, 0.0, 1728.0, 1117.0);
        // Dropped a little above and right of the laptop's right edge: snaps against it.
        assert_eq!(snap((1750.0, -30.0, 1920.0, 1080.0), &[laptop]), Some((1728, 0)));
        // Dropped above, roughly centred: centred exactly on top.
        assert_eq!(snap((-80.0, -1050.0, 1920.0, 1080.0), &[laptop]), Some((-96, -1080)));
        // Dropped on top of it: moved to the nearest free edge.
        let (x, y) = snap((100.0, 900.0, 1920.0, 1080.0), &[laptop]).unwrap();
        assert!(!overlaps((x as f64, y as f64, 1920.0, 1080.0), laptop));
        assert_eq!(y, 1117);
        // Far away: still touching, sharing at least a strip of edge.
        assert_eq!(snap((1728.0, 5000.0, 1920.0, 1080.0), &[laptop]), Some((1728 - 64, 1117)));
    }
}
