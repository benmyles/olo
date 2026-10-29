//! Windows: the windowing mode a workspace has until one is chosen for it
//! in Deskbar's tray (`TilingSettings::default_mode`), and the space Olo
//! leaves around the windows it arranges (`Settings::tiling`): a margin at
//! the display's edges and a gap between windows, kept by tiling, zooming,
//! Tile All, Smart Tile, cascading and fanning out. app_server fits tiled
//! and zoomed windows to it as it changes, so the sliders move them live;
//! the miniature shows the main display with three windows tiled.

use olo_kit::{
    egui::{Color32, Rect, Sense, Ui, vec2},
    metrics::DESKBAR_WIDTH,
    style::{self, c},
    widgets::{BBox, MenuField, Slider, StringView},
};
use olo_protocol::{OutputInfo, Settings, WindowingMode};

/// The miniature's height; its width follows the display's shape.
const MINIATURE_H: f32 = 180.0;
/// Deskbar's height in the miniature (it grows with its apps), and the
/// space app_server leaves beside it (logical px).
const DESKBAR_H: f32 = 100.0;
const DESKBAR_GAP: f32 = 4.0;
/// A tab's height (logical px), and the least it is drawn in the
/// miniature (points).
const TAB_H: f32 = 22.0;
const MIN_TAB_H: f32 = 7.0;

pub fn ui(ui: &mut Ui, s: &mut Settings, outputs: &[OutputInfo]) {
    BBox::new("Windowing Mode").show(ui, |ui| {
        let names: Vec<&str> = WindowingMode::ALL.iter().map(|m| m.title()).collect();
        let mut i = WindowingMode::ALL.iter().position(|m| *m == s.tiling.default_mode).unwrap_or(0);
        ui.add(MenuField::new("default-mode", &mut i, &names).label("Default mode:"));
        s.tiling.default_mode = WindowingMode::ALL[i];
        let what = match s.tiling.default_mode {
            WindowingMode::Standard => "Windows stay where you put them.",
            WindowingMode::AutoTiling => "Each new window takes a tile, and the others make room.",
            WindowingMode::Focus => "Only the active window shows, zoomed, with its dialogs over it.",
        };
        // Two lines for each mode, so nothing below moves as it changes.
        let text = format!("{what} Choose a workspace's own from Deskbar's tray.");
        ui.add(StringView::new(&text).dim(true).wrap(true));
    });
    BBox::new("Space Around Arranged Windows").show(ui, |ui| {
        let t = &mut s.tiling;
        let mut margin = t.margin as f32;
        let text = format!("{margin:.0}");
        ui.add(Slider::new(&mut margin, 0.0..=64.0).step(1.0).label("Space at the edges").value_text(text).limits("None", "Wide"));
        t.margin = margin as u32;
        let mut gap = t.gap as f32;
        let text = format!("{gap:.0}");
        ui.add(Slider::new(&mut gap, 0.0..=48.0).step(1.0).label("Space between windows").value_text(text).limits("None", "Wide"));
        t.gap = gap as u32;
    });
    miniature(ui, s, outputs);
    ui.add(StringView::new("Tiling, zooming, Tile All, Smart Tile, cascading and fanning out all leave this space.").dim(true).wrap(true));
}

/// The main display to scale, Deskbar in its corner and three windows
/// tiled as app_server would: the left half, and the right half's top
/// and bottom.
fn miniature(ui: &mut Ui, s: &Settings, outputs: &[OutputInfo]) {
    let p = style::palette(ui.ctx());
    let main = outputs.iter().find(|o| o.main).or(outputs.first());
    let (dw, dh) = main.map(crate::display::logical).map_or((1280.0, 800.0), |(w, h)| (w as f32, h as f32));
    let (well, _) = ui.allocate_exact_size(vec2(ui.available_width(), MINIATURE_H), Sense::hover());
    ui.painter().rect_filled(well, 0.0, c(p.document_background));
    style::sunken(ui.painter(), well, &p, false);
    let room = well.shrink(10.0);
    let k = (room.width() / dw).min(room.height() / dh);
    let screen = style::snap(ui.ctx(), Rect::from_center_size(room.center(), vec2(dw * k, dh * k)));
    let painter = ui.painter().with_clip_rect(screen);
    let [r, g, b] = s.workspace_color(0);
    painter.rect_filled(screen, 0.0, Color32::from_rgb(r, g, b));
    let at = |x: f32, y: f32, w: f32, h: f32| style::snap(ui.ctx(), Rect::from_min_size(screen.min + vec2(x * k, y * k), vec2(w * k, h * k)));

    let bar = at(dw - DESKBAR_WIDTH, 0.0, DESKBAR_WIDTH, DESKBAR_H);
    painter.rect_filled(bar, 0.0, c(p.panel_background));
    style::frame_rect(&painter, bar, c(p.frame_focused[0]));

    // The usable area stops beside Deskbar; the margin and gap as in
    // app_server's `tile_area` and `tiling::part` (edges inside the area
    // give up half the gap each).
    let usable = (dw - DESKBAR_WIDTH - DESKBAR_GAP, dh);
    let margin = (s.tiling.margin.min(400) as f32).min(usable.0 / 4.0).min(usable.1 / 4.0);
    let (ax, ay, aw, ah) = (margin, margin, usable.0 - 2.0 * margin, usable.1 - 2.0 * margin);
    let gap = s.tiling.gap.min(200) as f32;
    let (lo, hi) = ((gap / 2.0).floor(), gap - (gap / 2.0).floor());
    let half_w = (aw / 2.0).round();
    let half_h = (ah / 2.0).round();
    let windows = [
        (ax, ay, half_w - lo, ah, false),
        (ax + half_w + hi, ay, aw - half_w - hi, half_h - lo, false),
        (ax + half_w + hi, ay + half_h + hi, aw - half_w - hi, ah - half_h - hi, true),
    ];
    // Tabs to scale would be slivers; they keep a size that reads.
    let tab_h = (TAB_H * k).max(MIN_TAB_H) / k;
    for (x, y, w, h, front) in windows {
        window(&painter, &p, at(x, y + tab_h, w, h - tab_h), at(x, y, (w * 0.55).min(220.0), tab_h), front);
    }
}

/// A window in the miniature: a tab on its top edge and a frame.
fn window(painter: &olo_kit::egui::Painter, p: &olo_kit::theme::Palette, body: Rect, tab: Rect, front: bool) {
    let line = c(p.frame_focused[0]);
    painter.rect_filled(tab, 0.0, c(if front { p.tab_focused } else { p.tab_unfocused }));
    style::frame_rect(painter, tab, line);
    painter.rect_filled(body, 0.0, c(p.document_background));
    style::frame_rect(painter, body, line);
}
