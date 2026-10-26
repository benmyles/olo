//! Display: an arrangement map of the connected screens (click one to pick
//! it), and the chosen screen's resolution, refresh rate and scale. Changes
//! apply at once through app_server's ConfigureOutput.

use tabula_kit::{
    egui::{Align2, Color32, Rect, Sense, Ui, vec2},
    metrics as m,
    style::{self, c},
    widgets::{BBox, MenuField, StringView},
};
use tabula_protocol::{Mode, OutputConfig, OutputInfo, Settings};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    selected: Option<String>,
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

/// Logical size (what apps lay out in) of an output at its mode and scale.
fn logical(o: &OutputInfo) -> (f64, f64) {
    let (w, h) = o.current_mode.as_ref().map_or((1920, 1080), |m| (m.width, m.height));
    (w as f64 / o.scale.max(0.1), h as f64 / o.scale.max(0.1))
}

fn display_name(o: &OutputInfo) -> String {
    let model = o.model.trim();
    if o.name.starts_with("eDP") || o.name.starts_with("LVDS") {
        "Built-in Display".into()
    } else if !model.is_empty() && model != "Unknown" {
        model.to_owned()
    } else {
        o.name.clone()
    }
}

pub fn ui(ui: &mut Ui, s: &Settings, outputs: &[OutputInfo], server: &Server, st: &mut State) {
    let p = style::palette(ui.ctx());
    if outputs.is_empty() {
        ui.add(StringView::new("No displays reported by app_server.").dim(true));
        return;
    }
    if st.selected.as_ref().is_none_or(|n| !outputs.iter().any(|o| &o.name == n)) {
        st.selected = Some(outputs[0].name.clone());
    }

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
    for o in outputs {
        let (w, h) = logical(o);
        let r = Rect::from_min_size(
            off + vec2((o.position.0 as f64 - x0) as f32 * k, (o.position.1 as f64 - y0) as f32 * k),
            vec2(w as f32 * k, h as f32 * k),
        )
        .shrink(1.0);
        let chosen = st.selected.as_deref() == Some(&o.name);
        let resp = ui.interact(r, ui.make_persistent_id(("display", &o.name)), Sense::click());
        if resp.clicked() {
            st.selected = Some(o.name.clone());
        }
        let [rr, gg, bb] = s.workspace_color(0);
        ui.painter().rect_filled(r, 0.0, Color32::from_rgb(rr, gg, bb));
        let border = if chosen { c(p.keyboard_navigation) } else { c(p.frame_focused[0]) };
        let stroke_w = if chosen { 2.0 } else { 1.0 };
        ui.painter().rect_stroke(r, 0.0, (stroke_w, border), tabula_kit::egui::StrokeKind::Inside);
        // A menu bar strip on the primary (leftmost-topmost) screen, as macOS does.
        if o.position == (x0 as i32, y0 as i32) {
            let bar = Rect::from_min_size(r.min + vec2(stroke_w, stroke_w), vec2(r.width() - 2.0 * stroke_w, 4.0));
            ui.painter().rect_filled(bar, 0.0, Color32::from_white_alpha(200));
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, display_name(o), style::plain_font(), Color32::WHITE);
    }

    let Some(o) = outputs.iter().find(|o| st.selected.as_deref() == Some(&o.name)) else {
        return;
    };
    let label_w = 90.0;
    BBox::new(&display_name(o)).show(ui, |ui| {
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
            server.configure_output(&o.name, OutputConfig { mode: Some(chosen.clone()), ..Default::default() });
        }

        let scale_names: Vec<String> = SCALES.iter().map(|&s| scale_text(s)).collect();
        let scale_names: Vec<&str> = scale_names.iter().map(String::as_str).collect();
        let mut si = SCALES.iter().position(|&s| (s - o.scale).abs() < 0.01).unwrap_or(4);
        let before = si;
        ui.add(MenuField::new(("scale", &o.name), &mut si, &scale_names).label("Scale:").divider(label_w));
        if si != before {
            server.configure_output(&o.name, OutputConfig { scale: Some(SCALES[si]), ..Default::default() });
        }
        let (lw, lh) = (chosen.width as f64 / SCALES[si], chosen.height as f64 / SCALES[si]);
        ui.horizontal(|ui| {
            ui.add_space(label_w + m::LABEL_GAP);
            ui.add(StringView::new(&format!("Looks like {lw:.0} × {lh:.0}")).dim(true));
        });
    });
}
