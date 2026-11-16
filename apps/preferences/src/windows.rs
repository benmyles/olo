//! Windows: the windowing mode a workspace has until one is chosen for it
//! in Deskbar's tray (`TilingSettings::default_mode`), how auto-tiling
//! lays windows out (`TilingSettings::layout`), and the space Olo
//! leaves around the windows it arranges (`Settings::tiling`): a margin at
//! the display's edges and a gap between windows, kept by tiling, zooming,
//! Tile All, Smart Tile, cascading and fanning out. app_server fits tiled
//! and zoomed windows to it as it changes, so the sliders move them live;
//! the miniature shows the main display with three windows tiled. And
//! the apps whose windows always float over the tiles
//! (`TilingSettings::floating_apps`, a window menu's Float Window ▸
//! Always), to tile them again.

use olo_kit::{
    Bounds, ContentMask, Painter, Pixels, SharedString, Styled as _, canvas,
    metrics::DESKBAR_WIDTH,
    palette::Palette,
    point, px, rgb, size,
    style::{self, c},
    theme,
};
use olo_protocol::{OutputInfo, Settings, Tile, TilingLayout, WindowingMode};

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
/// What the miniature adds to the gap between windows (points), as a
/// tab keeps a size that reads.
const MIN_GAP: f32 = 4.0;

pub fn ui(ui: &mut Ui, outputs: &[OutputInfo]) {
    // (Two columns: in one, with the apps that float, it was taller than
    // the smallest screen.)
    let s = ui.s.clone();
    let left = ui.collect(|ui| {
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
            let layouts: Vec<SharedString> = TilingLayout::ALL.iter().map(|l| SharedString::from(l.title())).collect();
            let i = TilingLayout::ALL.iter().position(|l| *l == s.tiling.layout).unwrap_or(0);
            ui.menu("tiling-layout", layouts, i, |f| f.label("Auto-tiling layout:"), |s, i| s.tiling.layout = TilingLayout::ALL[i]);
            // (The one a workspace starts with: each keeps its own, chosen
            // from Deskbar's tray.)
            ui.note(format!("{} A workspace starts with this; choose its own from Deskbar's tray.", s.tiling.layout.detail()));
            // Where Main and Stack's main window goes on a workspace that
            // hasn't a place of its own (Deskbar's tray ▸ Main Window).
            let places: Vec<(Tile, &str)> = Tile::CHOOSABLE.iter().map(|t| (*t, t.title())).collect();
            ui.enabled_if(s.tiling.layout == TilingLayout::MainAndStack, |ui| {
                ui.choose("main-place", &places, &s.tiling.main_place, |f| f.label("Main window:"), |s, t| s.tiling.main_place = t);
            });
        });
        // The apps whose windows always float over the tiles (a window's
        // menu ▸ Float Window ▸ Always), by name; unchecked, one's
        // windows tile again.
        ui.bbox("Apps That Always Float", |ui| {
            if s.tiling.floating_apps.is_empty() {
                ui.note("None. To add an app, choose Float Window ▸ Always in the window menu of one of its windows.");
            }
            for id in &s.tiling.floating_apps {
                let name = app_name(id);
                let id = id.clone();
                ui.check(name, true, move |s, on| {
                    let apps = &mut s.tiling.floating_apps;
                    apps.retain(|a| *a != id);
                    if on {
                        apps.push(id.clone());
                    }
                });
            }
            if !s.tiling.floating_apps.is_empty() {
                ui.note("Their windows float over the tiles, and over the window Focus shows. Uncheck an app to tile its windows again.");
            }
        });
    });
    let right = ui.collect(|ui| {
        ui.bbox("Space Around Arranged Windows", |ui| {
            let margin = s.tiling.margin as f32;
            ui.slider("margin", margin, 0.0..=64.0, |sl| sl.step(1.0).label("Space at the edges").value_text(format!("{margin:.0}")).limits("None", "Wide"), |s, v| s.tiling.margin = v as u32);
            let gap = s.tiling.gap as f32;
            ui.slider("gap", gap, 0.0..=48.0, |sl| sl.step(1.0).label("Space between windows").value_text(format!("{gap:.0}")).limits("None", "Wide"), |s, v| s.tiling.gap = v as u32);
        });
        let main = outputs.iter().find(|o| o.main).or(outputs.first());
        let display = main.map(crate::display::logical).map_or((1280.0, 800.0), |(w, h)| (w as f32, h as f32));
        let s = s.clone();
        ui.add(canvas(move |g, b| miniature(g, b, &theme::palette(), &s, display)).w_full().h(px(MINIATURE_H)).flex_shrink_0());
        ui.note("Tiling, zooming, Tile All, Smart Tile, cascading and fanning out all leave this space.");
    });
    ui.add_columns(left, right);
}

/// An app's name, by its id: from its desktop entry, read once (finding
/// one by its window class reads them all, milliseconds), not each time
/// the panel is laid out.
fn app_name(id: &str) -> String {
    thread_local! {
        static NAMES: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
    }
    NAMES.with_borrow_mut(|names| {
        names
            .entry(id.to_owned())
            .or_insert_with(|| {
                let entry = if olo_xdg::apps::our_app(id).is_some() { None } else { olo_xdg::desktop_entry::find(id) };
                olo_xdg::apps::app_name(id, entry.as_ref()).0
            })
            .clone()
    })
}

/// The main display to scale, Deskbar in its corner and three windows
/// tiled as app_server would: in Halves, the left half and the right
/// half's top and bottom; in Main and Stack, the main window in its
/// place (in front) and two more in the rest.
fn miniature(g: &mut Painter, well: Bounds<Pixels>, p: &Palette, s: &Settings, (dw, dh): (f32, f32)) {
    style::fill_rect(g, well, c(p.document_background));
    style::sunken(g, well, p, false);
    let room = well.dilate(px(-10.0));
    let k = (f32::from(room.size.width) / dw).min(f32::from(room.size.height) / dh);
    let screen = style::snap(g, Bounds::centered_at(room.center(), size(px(dw * k), px(dh * k))));
    g.with_content_mask(Some(ContentMask { bounds: screen }), |g| {
        let [red, green, blue] = s.workspace_color(0);
        style::fill_rect(g, screen, rgb((u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)).into());
        let at = |g: &Painter, x: f32, y: f32, w: f32, h: f32| style::snap(g, Bounds { origin: point(screen.left() + px(x * k), screen.top() + px(y * k)), size: size(px(w * k), px(h * k)) });

        let bar = at(g, dw - DESKBAR_WIDTH, 0.0, DESKBAR_WIDTH, DESKBAR_H);
        style::fill_rect(g, bar, c(p.panel_background));
        style::frame_rect(g, bar, c(p.frame_focused[0]));

        // The usable area stops beside Deskbar; the margin and gap as in
        // app_server's `tile_area` and `tiling::part` (edges inside the
        // area give up half the gap each).
        let usable = (dw - DESKBAR_WIDTH - DESKBAR_GAP, dh);
        let margin = (s.tiling.margin.min(400) as f32).min(usable.0 / 4.0).min(usable.1 / 4.0);
        let (ax, ay, aw, ah) = (margin, margin, usable.0 - 2.0 * margin, usable.1 - 2.0 * margin);
        // (A gap to scale is a sliver, two frames' edges almost one: four
        // points more than that, so they read as two, and the slider
        // still moves them.)
        let gap = if s.tiling.gap > 0 { s.tiling.gap.min(200) as f32 + MIN_GAP / k } else { 0.0 };
        let (lo, hi) = ((gap / 2.0).floor(), gap - (gap / 2.0).floor());
        let parts = match s.tiling.layout {
            TilingLayout::MainAndStack => s.tiling.main_place.main_and_stack_parts(2),
            TilingLayout::Halves => vec![(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5)],
        };
        let front = if s.tiling.layout == TilingLayout::MainAndStack { 0 } else { parts.len() - 1 };
        // (An edge inside the area gives up half the gap, each side.)
        let windows = parts.iter().enumerate().map(|(i, &(px, py, pw, ph))| {
            let edge = |a: f32, b: f32, start: f32, len: f32| {
                let (lo_in, hi_in) = (a > 1e-3, a + b < 1.0 - 1e-3);
                let (s0, s1) = ((start + a * len).round() + if lo_in { hi } else { 0.0 }, (start + (a + b) * len).round() - if hi_in { lo } else { 0.0 });
                (s0, s1 - s0)
            };
            let ((x, w), (y, h)) = (edge(px, pw, ax, aw), edge(py, ph, ay, ah));
            (x, y, w, h, i == front)
        });
        // Tabs to scale would be slivers; they keep a size that reads.
        let tab_h = (TAB_H * k).max(MIN_TAB_H) / k;
        for (x, y, w, h, front) in windows {
            let (body, tab) = (at(g, x, y + tab_h, w, h - tab_h), at(g, x, y, (w * 0.55).min(220.0), tab_h));
            let line = c(p.frame_focused[0]);
            style::fill_rect(g, tab, c(if front { p.tab_focused } else { p.tab_unfocused }));
            style::frame_rect(g, tab, line);
            style::fill_rect(g, body, c(p.document_background));
            style::frame_rect(g, body, line);
        }
    });
}
