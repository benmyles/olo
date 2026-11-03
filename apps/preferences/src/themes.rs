//! Themes: the whole look, everywhere (Deskbar, Tracker, Twitcher, window
//! tabs and frames, every app, GTK and Chrome too). A row per theme, each
//! drawn in that theme: its name and a line about it, its wordmark and a
//! few of its icons. Choosing one sets `Appearance::icon_theme`, which
//! every app follows at once, and the wallpaper to the theme's own
//! (`themes::wallpaper`), which Backgrounds can change after.
//!
//! Under them, the theme's colour sets (`olo_theme::looks::ColorSet`:
//! its own, BeOS's, Catppuccin's), each previewed as a small window in
//! its light and its dark palette. Choosing one sets
//! `Appearance::colors`; Terminal's colours follow it.

use olo_protocol::Settings;
use olo_ui::{
    brand,
    gpui::{Bounds, Hsla, InteractiveElement as _, MouseButton, ParentElement as _, Pixels, SharedString, Styled as _, Window, canvas, div, point, px, size},
    icons, metrics as m,
    palette::{
        Palette, ThemeMode,
        looks::{self, ColorSet},
    },
    style::{self, c},
    theme,
    widgets::{RadioButton, StringView},
};
use olo_xdg::themes::{self, THEMES};

use crate::form::Ui;

/// A theme's row.
const ROW: f32 = 56.0;
/// The icons each row shows, and their size.
const SAMPLES: &[&str] = &["folder", "text", "application", "terminal", "disk", "trash-full"];
const ICON: f32 = 32.0;
const ICON_GAP: f32 = 6.0;
/// The wordmark's height, centred in a slot this wide.
const WORD: f32 = 24.0;
const WORD_SLOT: f32 = 64.0;
/// A colour set's preview: a small window in each mode, side by side.
const PREVIEW_H: f32 = 58.0;

pub fn ui(ui: &mut Ui) {
    let s = ui.s.clone();
    ui.note("The look of every icon, window and control, in every app, and a wallpaper to match.");
    let dark = theme::palette(ui.cx).mode == ThemeMode::Dark;
    let chosen = themes::resolve(&s.appearance.icon_theme).id;
    let icons_w = SAMPLES.len() as f32 * ICON + (SAMPLES.len() - 1) as f32 * ICON_GAP;
    // Rows touch, an etched line between each.
    let mut rows = Vec::new();
    for (i, t) in THEMES.iter().enumerate() {
        let id = t.id;
        let pick = ui.edit_click(move |s| choose(s, id));
        let pick_row = ui.edit_click(move |s| choose(s, id));
        let art = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                if i > 0 {
                    // Etched, as `widgets::separator`.
                    let etch = style::shade(&p, p.panel_background, 1.2);
                    style::hline_above(window, b.left(), b.right(), b.top(), etch);
                    style::hline(window, b.left(), b.right(), b.top(), c(p.shine));
                }
                let icons_at = b.right() - px(icons_w);
                let word = Bounds::centered_at(point(icons_at - px(m::SPACING + WORD_SLOT / 2.0), b.center().y), size(px(WORD_SLOT), px(WORD)));
                let full = brand::wordmark_width_in(id, WORD, dark).unwrap_or(WORD_SLOT);
                let w = full.min(WORD_SLOT);
                let h = WORD * w / full.max(1.0);
                brand::wordmark_centred_in(window, Bounds::centered_at(word.center(), size(px(w), px(h))), dark, id);
                for (k, name) in SAMPLES.iter().enumerate() {
                    let at = point(icons_at + px(k as f32 * (ICON + ICON_GAP)), b.center().y - px(ICON / 2.0));
                    icons::paint_in(window, Bounds { origin: at, size: size(px(ICON), px(ICON)) }, id, name);
                }
            },
        )
        .absolute()
        .inset_0();
        // Anywhere on the row chooses it; the radio button is on top.
        let text_w = 13.0 + m::MARK_GAP;
        let row = div()
            .id(SharedString::from(format!("theme-{id}")))
            .relative()
            .h(px(ROW))
            .flex()
            .flex_col()
            .justify_center()
            .child(art)
            .child(RadioButton::new(t.name, id == chosen).on_select(pick))
            .child(div().pl(px(text_w)).child(StringView::new(t.about).dim(true)))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| pick_row(window, cx));
        rows.push(row);
    }
    ui.add(div().flex().flex_col().children(rows));
    ui.space(m::SPACING);
    colors(ui, &s);
}

/// The chosen theme's colour sets, each a preview over its name and a
/// line about it; clicking any of it chooses the set.
fn colors(ui: &mut Ui, s: &Settings) {
    let theme = themes::resolve(&s.appearance.icon_theme);
    let chosen = ColorSet::resolve(&s.appearance.colors);
    let title = format!("{} Colors", theme.name);
    let p = olo_ui::theme::palette(ui.cx);
    let [r, g, b, _] = p.list_selected_item_text;
    let selected_dim: Hsla = olo_ui::gpui::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 150.0 / 255.0 }.into();
    ui.bbox(&title, |ui| {
        let mut cards = Vec::new();
        for set in ColorSet::ALL {
            let name = set.name(theme);
            let on = set == chosen;
            let pick = ui.bind_click(on, move |s| s.appearance.colors = set.id().to_owned());
            let pick_card = ui.edit_click(move |s| s.appearance.colors = set.id().to_owned());
            let theme_id = theme.id;
            let preview = canvas(|_, _, _| {}, move |b, _, window, cx| paint_preview(window, b, &theme::palette(cx), theme_id, set)).w_full().h(px(PREVIEW_H));
            let plate = canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    if on {
                        style::selection_plate(window, b, &theme::palette(cx));
                    }
                },
            )
            .absolute()
            .inset_0();
            let about = set.about(theme);
            let mut text = StringView::new(about).dim(true).wrap(true);
            if on {
                text = text.color(selected_dim);
            }
            cards.push(
                div()
                    .id(SharedString::from(format!("colors-{}", set.id())))
                    .relative()
                    // (Each an equal share of the row, whatever its text.)
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .child(plate)
                    .child(preview)
                    .child(RadioButton::new(name, on).on_select(pick))
                    // (Three lines: the longest set's, at this width.)
                    .child(div().h(px(m::CONTROL_HEIGHT + 2.0 * m::LABEL_ROW)).child(text))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| pick_card(window, cx)),
            );
        }
        ui.add(div().flex().flex_row().gap(px(m::SPACING)).children(cards));
    });
}

/// Colour set `set` of theme `theme` in `r`: a small window in its light
/// palette on the left and its dark one on the right, framed.
fn paint_preview(window: &mut Window, r: Bounds<Pixels>, p: &Palette, theme: &str, set: ColorSet) {
    let r = style::snap(window, r);
    let half = (f32::from(r.size.width) / 2.0).floor();
    for (k, mode) in [ThemeMode::Light, ThemeMode::Dark].into_iter().enumerate() {
        let look = looks::palette(theme, set.id(), mode);
        let w = if k == 0 { half } else { f32::from(r.size.width) - half };
        let side = Bounds { origin: point(r.left() + px(k as f32 * half), r.top()), size: size(px(w), r.size.height) };
        mini_window(window, side, &look);
    }
    style::frame_rect(window, r, c(p.control_border));
}

/// A window in palette `p`, wireframe-small: its desk, a focused tab, the
/// frame, a list with a row chosen, a button and a default button.
fn mini_window(window: &mut Window, r: Bounds<Pixels>, p: &Palette) {
    let at = |x: Pixels, y: Pixels, w: f32, h: f32| Bounds { origin: point(x, y), size: size(px(w), px(h)) };
    let fill = |window: &mut Window, b: Bounds<Pixels>, rgba| {
        let b = style::snap(window, b);
        style::fill_rect(window, b, c(rgba));
    };
    let shrink = |b: Bounds<Pixels>, d: f32| b.dilate(px(-d));
    // A bar of "text": `w` long, centred on `y`.
    let text = |window: &mut Window, x: Pixels, y: Pixels, w: f32, rgba| fill(window, at(x, px((f32::from(y) - 1.0).round()), w.round(), 2.0), rgba);
    fill(window, r, olo_ui::palette::tint(p.panel_background, if p.is_dark() { 1.25 } else { 1.12 }));
    // The tab.
    let tab = at(r.left() + px(6.0), r.top() + px(5.0), (f32::from(r.size.width) * 0.55).round(), 10.0);
    fill(window, tab, p.frame_focused[0]);
    let face = Bounds::from_corners(point(tab.left() + px(1.0), tab.top() + px(1.0)), tab.bottom_right() - point(px(1.0), px(0.0)));
    fill(window, face, p.tab_focused);
    fill(window, at(face.left(), face.top(), f32::from(face.size.width), 1.0), p.tab_focused_light);
    text(window, face.left() + px(4.0), face.center().y, f32::from(face.size.width) * 0.5, p.tab_text_focused);
    // The window: frame, panel.
    let win = Bounds::from_corners(point(r.left() + px(6.0), tab.bottom()), point(r.right() - px(6.0), r.bottom() - px(5.0)));
    fill(window, win, p.frame_focused[0]);
    fill(window, shrink(win, 1.0), p.frame_focused[1]);
    let panel = Bounds::from_corners(point(win.left() + px(2.0), win.top() + px(2.0)), point(win.right() - px(1.0), win.bottom() - px(1.0)));
    fill(window, panel, p.panel_background);
    // A list, its second row chosen.
    let list = Bounds::from_corners(point(panel.left() + px(3.0), panel.top() + px(3.0)), point(panel.right() - px(3.0), panel.top() + px(18.0)));
    fill(window, list, p.control_border);
    let doc = shrink(list, 1.0);
    fill(window, doc, p.list_background);
    let row = 5.0;
    fill(window, at(doc.left(), doc.top() + px(row), f32::from(doc.size.width), row), p.list_selected_background);
    for (i, w) in [0.6, 0.45, 0.7].into_iter().enumerate() {
        let color = if i == 1 { p.list_selected_item_text } else { p.list_item_text };
        text(window, doc.left() + px(2.0), doc.top() + px(row * (i as f32 + 0.5)), (f32::from(doc.size.width) - 4.0) * w, color);
    }
    // A button, and the default one in the highlight colour.
    let bw = ((f32::from(panel.size.width) - 9.0) / 2.0).floor();
    let button = at(panel.left() + px(3.0), list.bottom() + px(3.0), bw, f32::from(panel.bottom() - list.bottom() - px(6.0)).max(4.0));
    fill(window, button, p.control_border);
    fill(window, shrink(button, 1.0), p.control_background);
    text(window, button.center().x - px(bw * 0.25), button.center().y, bw * 0.5, p.control_text);
    let default = Bounds { origin: point(button.left() + px(bw + 3.0), button.top()), size: button.size };
    fill(window, default, p.control_border);
    fill(window, shrink(default, 1.0), p.menu_selected_background);
    text(window, default.center().x - px(bw * 0.25), default.center().y, bw * 0.5, p.menu_selected_item_text);
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
        choose(&mut s, "halftone");
        assert_eq!((s.appearance.icon_theme.as_str(), s.wallpaper.as_deref()), ("halftone", Some("pack:halftone")));
    }
}
