//! Themes: the whole look, everywhere (Deskbar, Tracker, Twitcher, window
//! tabs and frames, every app, GTK and Chrome too). The two themes,
//! Classic and Platinum, side by side, each a card drawn in that theme: a
//! preview (its wallpaper as the mode is now, a window in its own colours
//! on it, a few of its icons on the desk), its name and the Olo mark as
//! it draws it, a line about it and a row of its icons. Choosing one (anywhere on its card)
//! sets `Appearance::icon_theme`, which every app follows at once, and
//! the wallpaper to the theme's own (`themes::wallpaper`), which
//! Backgrounds can change after.
//!
//! Under them, the chosen theme's colour sets (`olo_theme::looks::
//! ColorSet`: its own and BeOS's), side by side, each previewed as a
//! small window in its light and its dark palette. Choosing one sets
//! `Appearance::colors`; Terminal's colours follow it.

use std::sync::Arc;

use olo_kit::{
    Bounds, Corners, Hsla, InteractiveElement as _, MouseButton, Painter, ParentElement as _, Pixels, RenderImage, SharedString, Styled as _, canvas, div, icons, metrics as m,
    palette::{
        Palette, ThemeMode,
        looks::{self, ColorSet},
    },
    pictures, point, px, size,
    style::{self, c},
    theme,
    widgets::{RadioButton, StringView},
};
use olo_protocol::Settings;
use olo_xdg::{
    themes::{self, THEMES},
    wallpapers,
};

use crate::form::Ui;

/// A theme's preview is its wallpaper's shape.
const PREVIEW_ASPECT: f32 = 9.0 / 16.0;
/// The icons under each theme, and their size.
const SAMPLES: &[&str] = &["folder", "text", "application", "terminal", "mail", "disk", "trash-full"];
const ICON: f32 = 32.0;
/// The Olo mark beside a theme's name, as the theme draws it (the
/// wordmark is the brand's lettering, the same in every theme).
const MARK: f32 = 24.0;
/// A colour set's preview: a small window in each mode, side by side.
const SET_PREVIEW_H: f32 = 58.0;

pub fn ui(ui: &mut Ui) {
    let s = ui.s.clone();
    ui.note("The look of every icon, window and control, in every app, and a wallpaper to match.");
    let dark = theme::palette().mode == ThemeMode::Dark;
    let chosen = themes::resolve(&s.appearance.icon_theme).id;
    // Two cards across the window, each its preview and what's under it,
    // inset by a plate's width (the chosen one's plate shows round it).
    let wide = f32::from(ui.cx.viewport_size().width) - 2.0 * m::WINDOW_INSET;
    let card_w = ((wide - m::SPACING) / 2.0).floor();
    let inner_w = card_w - 2.0 * m::TIGHT;
    let preview_h = (inner_w * PREVIEW_ASPECT).round();
    let px_wide = (inner_w * ui.cx.scale_factor()).round() as u32;
    let text_w = 13.0 + m::MARK_GAP;
    let mut cards = Vec::new();
    for t in THEMES.iter() {
        let id = t.id;
        let on = id == chosen;
        let pick = ui.bind_click(on, move |s| choose(s, id));
        let pick_card = ui.edit_click(move |s| choose(s, id));
        // Its wallpaper as the mode is now, made at the preview's device
        // width on a worker (drawn when it's ready).
        let picture = wallpapers::resolve(&themes::wallpaper(id), (3840, 2160), dark).and_then(|p| {
            pictures::thumbnail_soon(&p, px_wide, {
                let again = ui.cx.invalidator();
                move || again.invalidate()
            })
        });
        let preview = canvas(move |g, b| paint_theme_preview(g, b, id, dark, picture.clone())).w_full().h(px(preview_h));
        let plate = canvas(move |g, b| {
            if on {
                style::selection_plate(g, b, &theme::palette());
            }
        })
        .absolute()
        .inset_0();
        let word = canvas(move |g, b| {
            // On the name's own line (its capitals' middle, where the
            // radio button, as tall as this, sets its label on a device
            // pixel): centred in its own rect, each snapped apart, the
            // two sat a device pixel off level at 1.25×.
            let middle = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
            icons::paint_in(g, style::snap_centred(g, point(b.center().x, middle), MARK, MARK), id, "olo");
        })
        .w(px(MARK))
        .h(px(MARK))
        .flex_shrink_0();
        let mut about = StringView::new(t.about).dim(true);
        if on {
            about = about.color(selected_dim());
        }
        let samples = canvas(move |g, b| {
            // Spread evenly across the card, each on whole pixels.
            let n = SAMPLES.len() as f32;
            let step = ((f32::from(b.size.width) - ICON) / (n - 1.0)).floor();
            let left = b.left() + px(((f32::from(b.size.width) - ICON - step * (n - 1.0)) / 2.0).floor());
            for (k, name) in SAMPLES.iter().enumerate() {
                let at = Bounds { origin: point(left + px(k as f32 * step), b.top()), size: size(px(ICON), px(ICON)) };
                icons::paint_in(g, at, id, name);
            }
        })
        .w_full()
        .h(px(ICON));
        cards.push(
            div()
                .id(SharedString::from(format!("theme-{id}")))
                .relative()
                .w(px(card_w))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .p(px(m::TIGHT))
                .child(plate)
                .child(preview)
                // (Its name and the line about it together, as a radio
                // button's label and its note.)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().flex().flex_row().items_center().justify_between().child(RadioButton::new(t.name, on).height(MARK).on_select(pick)).child(word))
                        .child(div().pl(px(text_w)).child(about)),
                )
                .child(div().pt(px(m::TIGHT)).child(samples))
                .on_mouse_down(MouseButton::Left, pick_card),
        );
    }
    ui.add(div().flex().flex_row().gap(px(m::SPACING)).children(cards));
    ui.space(m::SPACING);
    colors(ui, &s);
}

/// The text under a chosen card's name: its list-selection ink, dimmed.
fn selected_dim() -> Hsla {
    let [r, g, b, _] = theme::palette().list_selected_item_text;
    olo_kit::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 150.0 / 255.0 }.into()
}

/// The chosen theme's colour sets, side by side, each a preview over its
/// name and a line about it; clicking any of it chooses the set.
fn colors(ui: &mut Ui, s: &Settings) {
    let theme = themes::resolve(&s.appearance.icon_theme);
    let chosen = ColorSet::resolve(&s.appearance.colors);
    let title = format!("{} Colors", theme.name);
    let wide = f32::from(ui.cx.viewport_size().width) - 2.0 * (m::WINDOW_INSET + m::BOX_INSET);
    let card_w = ((wide - m::SPACING) / 2.0).floor();
    ui.bbox(&title, |ui| {
        let mut cards = Vec::new();
        for set in ColorSet::ALL {
            let name = set.name(theme);
            let on = set == chosen;
            let pick = ui.bind_click(on, move |s| s.appearance.colors = set.id().to_owned());
            let pick_card = ui.edit_click(move |s| s.appearance.colors = set.id().to_owned());
            let theme_id = theme.id;
            let preview = canvas(move |g, b| paint_set_preview(g, b, &theme::palette(), theme_id, set)).w_full().h(px(SET_PREVIEW_H));
            let plate = canvas(move |g, b| {
                if on {
                    style::selection_plate(g, b, &theme::palette());
                }
            })
            .absolute()
            .inset_0();
            let mut text = StringView::new(set.about(theme)).dim(true).wrap(true);
            if on {
                text = text.color(selected_dim());
            }
            cards.push(
                div()
                    .id(SharedString::from(format!("colors-{}", set.id())))
                    .relative()
                    .w(px(card_w))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .p(px(m::TIGHT))
                    .child(plate)
                    .child(preview)
                    .child(RadioButton::new(name, on).on_select(pick))
                    // (Two lines, the longer set's, at this width: the
                    // cards stay the same height.)
                    .child(div().h(px(2.0 * m::LABEL_ROW)).child(text))
                    .on_mouse_down(MouseButton::Left, pick_card),
            );
        }
        ui.add(div().flex().flex_row().gap(px(m::SPACING)).children(cards));
    });
}

/// Theme `theme`'s preview in `r`: its wallpaper (`picture`, once it's
/// made; till then the desk's colour), a window in its own colours on it
/// and three of its icons on the desk at the right, framed as a picture
/// well. On a grid of its own: nothing in it is laid out with the panel.
fn paint_theme_preview(g: &mut Painter, r: Bounds<Pixels>, theme: &'static str, dark: bool, picture: Option<Arc<RenderImage>>) {
    let p = olo_kit::theme::palette();
    let mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
    let look = looks::palette(theme, ColorSet::Theme.id(), mode);
    let frame = style::snap(g, r);
    let well = style::inset(g, frame, 2.0);
    olo_kit::review::own_grid(g, frame);
    style::fill_rect(g, well, c(olo_kit::palette::tint(look.panel_background, if dark { 0.8 } else { 1.1 })));
    if let Some(img) = picture {
        // A texture pixel to a device pixel, centred, clipped to the well.
        let s = g.scale_factor();
        let isz = img.size(0);
        let (w, h) = (isz.width.0 as f32 / s, isz.height.0 as f32 / s);
        let at = style::snap_centred(g, well.center(), w, h);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: well }), |g| {
            let _ = g.paint_image(at, Corners::default(), img, 0, false);
        });
    }
    // The window: left of the middle, most of the height.
    let ww = (f32::from(well.size.width) * 0.6).round();
    let wh = (f32::from(well.size.height) * 0.7).round();
    let win = Bounds { origin: point(well.left() + px((f32::from(well.size.width) * 0.07).round()), well.top() + px((f32::from(well.size.height) * 0.13).round())), size: size(px(ww), px(wh)) };
    g.with_content_mask(Some(olo_kit::ContentMask { bounds: well }), |g| {
        preview_window(g, win, &look, theme);
        // Three icons on the desk, down the right.
        let side = 24.0;
        let x = well.right() - px((f32::from(well.size.width) * 0.12).round() + side / 2.0);
        for (k, name) in ["home", "disk", "trash-full"].into_iter().enumerate() {
            let y = well.top() + px((f32::from(well.size.height) * (0.14 + 0.28 * k as f32)).round());
            icons::paint_in(g, Bounds { origin: point(x, y), size: size(px(side), px(side)) }, theme, name);
        }
    });
    let inner = style::inset(g, frame, 1.0);
    style::bevel(g, inner, style::shade(&p, p.panel_background, 1.45), c(p.shine));
    style::frame_rect(g, frame, style::shade(&p, p.panel_background, 1.2));
}

/// A window in palette `p`, small but whole, as theme `theme` draws one:
/// its focused tab with a title, the frame, a menu bar, a list of a few
/// of its icons with one chosen, and two buttons, the default one in the
/// highlight colour.
fn preview_window(g: &mut Painter, r: Bounds<Pixels>, p: &Palette, theme: &str) {
    let fill = |g: &mut Painter, b: Bounds<Pixels>, rgba| {
        let b = style::snap(g, b);
        style::fill_rect(g, b, c(rgba));
    };
    let at = |x: Pixels, y: Pixels, w: f32, h: f32| Bounds { origin: point(x, y), size: size(px(w), px(h)) };
    let shrink = |b: Bounds<Pixels>, d: f32| b.dilate(px(-d));
    // A bar of "text", `w` long, centred on `y`.
    let text = |g: &mut Painter, x: Pixels, y: Pixels, w: f32, rgba| fill(g, at(x, px((f32::from(y) - 1.5).round()), w.round(), 3.0), rgba);
    let tab_h = 14.0;
    // The tab: a third of the width, BeOS's yellow (or the look's).
    let tab = at(r.left(), r.top(), (f32::from(r.size.width) * 0.42).round(), tab_h + 1.0);
    fill(g, tab, p.frame_focused[0]);
    let face = Bounds::from_corners(point(tab.left() + px(1.0), tab.top() + px(1.0)), point(tab.right() - px(1.0), tab.bottom()));
    fill(g, face, p.tab_focused);
    fill(g, at(face.left(), face.top(), f32::from(face.size.width), 1.0), p.tab_focused_light);
    // Its close box, then its title.
    let close = at(face.left() + px(3.0), face.top() + px(3.0), 8.0, 8.0);
    fill(g, close, p.tab_focused_shadow);
    fill(g, shrink(close, 1.0), p.tab_focused_light);
    text(g, close.right() + px(4.0), face.center().y, f32::from(face.size.width) * 0.45, p.tab_text_focused);
    // The frame and its panel.
    let win = Bounds::from_corners(point(r.left(), tab.bottom()), r.bottom_right());
    fill(g, win, p.frame_focused[0]);
    fill(g, shrink(win, 1.0), p.frame_focused[1]);
    let panel = Bounds::from_corners(point(win.left() + px(2.0), win.top() + px(2.0)), point(win.right() - px(1.0), win.bottom() - px(1.0)));
    fill(g, panel, p.panel_background);
    // The menu bar, a few titles in it.
    let bar = at(panel.left(), panel.top(), f32::from(panel.size.width), 12.0);
    fill(g, bar, p.menu_background);
    fill(g, at(bar.left(), bar.bottom() - px(1.0), f32::from(bar.size.width), 1.0), p.frame_focused[4]);
    let mut x = bar.left() + px(5.0);
    for w in [12.0, 10.0, 14.0, 11.0] {
        text(g, x, bar.center().y - px(0.5), w, p.menu_item_text);
        x += px(w + 7.0);
    }
    // A list of icons, the second chosen.
    let list = Bounds::from_corners(point(panel.left() + px(5.0), bar.bottom() + px(5.0)), point(panel.right() - px(5.0), panel.bottom() - px(24.0)));
    fill(g, list, p.control_border);
    let doc = shrink(list, 1.0);
    fill(g, doc, p.list_background);
    let row = 18.0;
    let rows = ((f32::from(doc.size.height) - 2.0) / row).floor().max(1.0) as usize;
    for (i, name) in ["folder", "text", "image", "audio"].into_iter().take(rows).enumerate() {
        let y = doc.top() + px(1.0 + i as f32 * row);
        let ink = if i == 1 {
            fill(g, at(doc.left(), y, f32::from(doc.size.width), row), p.list_selected_background);
            p.list_selected_item_text
        } else {
            p.list_item_text
        };
        icons::paint_in(g, at(doc.left() + px(3.0), y + px(1.0), 16.0, 16.0), theme, name);
        text(g, doc.left() + px(24.0), y + px(row / 2.0), (f32::from(doc.size.width) - 30.0) * [0.55, 0.4, 0.62, 0.48][i], ink);
    }
    // The buttons, at the bottom right.
    let bw = 34.0;
    let bh = 13.0;
    let by = panel.bottom() - px(bh + 5.0);
    let default = at(panel.right() - px(bw + 5.0), by, bw, bh);
    let other = at(default.left() - px(bw + 5.0), by, bw, bh);
    fill(g, other, p.control_border);
    fill(g, shrink(other, 1.0), p.control_background);
    text(g, other.center().x - px(bw * 0.25), other.center().y, bw * 0.5, p.control_text);
    fill(g, default, p.control_border);
    fill(g, shrink(default, 1.0), p.menu_selected_background);
    text(g, default.center().x - px(bw * 0.25), default.center().y, bw * 0.5, p.menu_selected_item_text);
}

/// Colour set `set` of theme `theme` in `r`: a small window in its light
/// palette on the left and its dark one on the right, framed.
fn paint_set_preview(g: &mut Painter, r: Bounds<Pixels>, p: &Palette, theme: &str, set: ColorSet) {
    let r = style::snap(g, r);
    olo_kit::review::own_grid(g, r);
    let half = (f32::from(r.size.width) / 2.0).floor();
    for (k, mode) in [ThemeMode::Light, ThemeMode::Dark].into_iter().enumerate() {
        let look = looks::palette(theme, set.id(), mode);
        let w = if k == 0 { half } else { f32::from(r.size.width) - half };
        let side = Bounds { origin: point(r.left() + px(k as f32 * half), r.top()), size: size(px(w), r.size.height) };
        mini_window(g, side, &look);
    }
    style::frame_rect(g, r, c(p.control_border));
}

/// A window in palette `p`, wireframe-small: its desk, a focused tab, the
/// frame, a list with a row chosen, a button and a default button.
fn mini_window(g: &mut Painter, r: Bounds<Pixels>, p: &Palette) {
    let at = |x: Pixels, y: Pixels, w: f32, h: f32| Bounds { origin: point(x, y), size: size(px(w), px(h)) };
    let fill = |g: &mut Painter, b: Bounds<Pixels>, rgba| {
        let b = style::snap(g, b);
        style::fill_rect(g, b, c(rgba));
    };
    let shrink = |b: Bounds<Pixels>, d: f32| b.dilate(px(-d));
    // A bar of "text": `w` long, centred on `y`.
    let text = |g: &mut Painter, x: Pixels, y: Pixels, w: f32, rgba| fill(g, at(x, px((f32::from(y) - 1.0).round()), w.round(), 2.0), rgba);
    fill(g, r, olo_kit::palette::tint(p.panel_background, if p.is_dark() { 1.25 } else { 1.12 }));
    // The tab.
    let tab = at(r.left() + px(6.0), r.top() + px(5.0), (f32::from(r.size.width) * 0.55).round(), 10.0);
    fill(g, tab, p.frame_focused[0]);
    let face = Bounds::from_corners(point(tab.left() + px(1.0), tab.top() + px(1.0)), tab.bottom_right() - point(px(1.0), px(0.0)));
    fill(g, face, p.tab_focused);
    fill(g, at(face.left(), face.top(), f32::from(face.size.width), 1.0), p.tab_focused_light);
    text(g, face.left() + px(4.0), face.center().y, f32::from(face.size.width) * 0.5, p.tab_text_focused);
    // The frame, the panel.
    let win = Bounds::from_corners(point(r.left() + px(6.0), tab.bottom()), point(r.right() - px(6.0), r.bottom() - px(5.0)));
    fill(g, win, p.frame_focused[0]);
    fill(g, shrink(win, 1.0), p.frame_focused[1]);
    let panel = Bounds::from_corners(point(win.left() + px(2.0), win.top() + px(2.0)), point(win.right() - px(1.0), win.bottom() - px(1.0)));
    fill(g, panel, p.panel_background);
    // A list, its second row chosen.
    let list = Bounds::from_corners(point(panel.left() + px(3.0), panel.top() + px(3.0)), point(panel.right() - px(3.0), panel.top() + px(18.0)));
    fill(g, list, p.control_border);
    let doc = shrink(list, 1.0);
    fill(g, doc, p.list_background);
    let row = 5.0;
    fill(g, at(doc.left(), doc.top() + px(row), f32::from(doc.size.width), row), p.list_selected_background);
    for (i, w) in [0.6, 0.45, 0.7].into_iter().enumerate() {
        let color = if i == 1 { p.list_selected_item_text } else { p.list_item_text };
        text(g, doc.left() + px(2.0), doc.top() + px(row * (i as f32 + 0.5)), (f32::from(doc.size.width) - 4.0) * w, color);
    }
    // A button, and the default one in the highlight colour.
    let bw = ((f32::from(panel.size.width) - 9.0) / 2.0).floor();
    let button = at(panel.left() + px(3.0), list.bottom() + px(3.0), bw, f32::from(panel.bottom() - list.bottom() - px(6.0)).max(4.0));
    fill(g, button, p.control_border);
    fill(g, shrink(button, 1.0), p.control_background);
    text(g, button.center().x - px(bw * 0.25), button.center().y, bw * 0.5, p.control_text);
    let default = Bounds { origin: point(button.left() + px(bw + 3.0), button.top()), size: button.size };
    fill(g, default, p.control_border);
    fill(g, shrink(default, 1.0), p.menu_selected_background);
    text(g, default.center().x - px(bw * 0.25), default.center().y, bw * 0.5, p.menu_selected_item_text);
}

/// Switch to theme `id`, and to its wallpaper.
fn choose(s: &mut Settings, id: &str) {
    s.appearance.icon_theme = id.to_owned();
    s.wallpaper = Some(themes::wallpaper(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_brings_its_wallpaper() {
        // A new install starts in the default theme on its wallpaper.
        let mut s = Settings::default();
        assert_eq!(s.appearance.icon_theme, themes::DEFAULT);
        assert_eq!(s.wallpaper, Some(themes::wallpaper(themes::DEFAULT)));
        choose(&mut s, "platinum");
        assert_eq!((s.appearance.icon_theme.as_str(), s.wallpaper.as_deref()), ("platinum", Some("pack:quantum-foam")));
    }
}
