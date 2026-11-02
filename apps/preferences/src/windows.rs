//! Windows: the windowing mode a workspace has until one is chosen for it
//! in Deskbar's tray (`TilingSettings::default_mode`), and the space Olo
//! leaves around the windows it arranges (`Settings::tiling`): a margin at
//! the display's edges and a gap between windows, kept by tiling, zooming,
//! Tile All, Smart Tile, cascading and fanning out. app_server fits tiled
//! and zoomed windows to it as it changes, so the sliders move them live;
//! the miniature shows the main display with three windows tiled.

use olo_protocol::{OutputInfo, Settings, WindowingMode};
use olo_ui::{
    gpui::{Bounds, ContentMask, Pixels, SharedString, Styled as _, Window, canvas, point, px, rgb, size},
    metrics::DESKBAR_WIDTH,
    palette::Palette,
    style::{self, c},
    theme,
};

use crate::form::Ui;

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

pub fn ui(ui: &mut Ui, outputs: &[OutputInfo]) {
    let s = ui.s.clone();
    ui.bbox("Windowing Mode", |ui| {
        let names: Vec<SharedString> = WindowingMode::ALL.iter().map(|m| SharedString::from(m.title())).collect();
        let i = WindowingMode::ALL.iter().position(|m| *m == s.tiling.default_mode).unwrap_or(0);
        ui.menu("default-mode", names, i, |f| f.label("Default mode:"), |s, i| s.tiling.default_mode = WindowingMode::ALL[i]);
        let what = match s.tiling.default_mode {
            WindowingMode::Standard => "Windows stay where you put them.",
            WindowingMode::AutoTiling => "Each new window takes a tile, and the others make room.",
            WindowingMode::Focus => "Only the active window shows, zoomed, with its dialogs over it.",
        };
        // Two lines for each mode, so nothing below moves as it changes.
        ui.note(format!("{what} Choose a workspace's own from Deskbar's tray."));
    });
    ui.bbox("Space Around Arranged Windows", |ui| {
        let margin = s.tiling.margin as f32;
        ui.slider("margin", margin, 0.0..=64.0, |sl| sl.step(1.0).label("Space at the edges").value_text(format!("{margin:.0}")).limits("None", "Wide"), |s, v| s.tiling.margin = v as u32);
        let gap = s.tiling.gap as f32;
        ui.slider("gap", gap, 0.0..=48.0, |sl| sl.step(1.0).label("Space between windows").value_text(format!("{gap:.0}")).limits("None", "Wide"), |s, v| s.tiling.gap = v as u32);
    });
    let main = outputs.iter().find(|o| o.main).or(outputs.first());
    let display = main.map(crate::display::logical).map_or((1280.0, 800.0), |(w, h)| (w as f32, h as f32));
    ui.add(canvas(|_, _, _| {}, move |b, _, window, cx| miniature(window, b, &theme::palette(cx), &s, display)).w_full().h(px(MINIATURE_H)).flex_shrink_0());
    ui.note("Tiling, zooming, Tile All, Smart Tile, cascading and fanning out all leave this space.");
}

/// The main display to scale, Deskbar in its corner and three windows
/// tiled as app_server would: the left half, and the right half's top
/// and bottom.
fn miniature(window: &mut Window, well: Bounds<Pixels>, p: &Palette, s: &Settings, (dw, dh): (f32, f32)) {
    style::fill_rect(window, well, c(p.document_background));
    style::sunken(window, well, p, false);
    let room = well.dilate(px(-10.0));
    let k = (f32::from(room.size.width) / dw).min(f32::from(room.size.height) / dh);
    let screen = style::snap(window, Bounds::centered_at(room.center(), size(px(dw * k), px(dh * k))));
    window.with_content_mask(Some(ContentMask { bounds: screen }), |window| {
        let [r, g, b] = s.workspace_color(0);
        style::fill_rect(window, screen, rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into());
        let at = |window: &Window, x: f32, y: f32, w: f32, h: f32| style::snap(window, Bounds { origin: point(screen.left() + px(x * k), screen.top() + px(y * k)), size: size(px(w * k), px(h * k)) });

        let bar = at(window, dw - DESKBAR_WIDTH, 0.0, DESKBAR_WIDTH, DESKBAR_H);
        style::fill_rect(window, bar, c(p.panel_background));
        style::frame_rect(window, bar, c(p.frame_focused[0]));

        // The usable area stops beside Deskbar; the margin and gap as in
        // app_server's `tile_area` and `tiling::part` (edges inside the
        // area give up half the gap each).
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
            let (body, tab) = (at(window, x, y + tab_h, w, h - tab_h), at(window, x, y, (w * 0.55).min(220.0), tab_h));
            let line = c(p.frame_focused[0]);
            style::fill_rect(window, tab, c(if front { p.tab_focused } else { p.tab_unfocused }));
            style::frame_rect(window, tab, line);
            style::fill_rect(window, body, c(p.document_background));
            style::frame_rect(window, body, line);
        }
    });
}
