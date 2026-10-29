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

use olo_kit::{
    brand,
    egui::{self, Rect, Role, Sense, Ui, WidgetInfo, pos2, vec2},
    icons, metrics as m, style,
    style::c,
    theme::{Palette, ThemeMode, looks::{self, ColorSet}},
    themes::{self, THEMES},
    widgets::{BBox, RadioButton, StringView},
};
use olo_protocol::Settings;

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
/// Under it, its name and two lines about it (a wrapped `StringView`:
/// the first line where a single line's would be, then the second).
const ABOUT_H: f32 = m::CONTROL_HEIGHT + m::LABEL_ROW;
const CARD_H: f32 = PREVIEW_H + m::TIGHT + m::MARK_ROW + ABOUT_H;

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    ui.add(StringView::new("The look of every icon, window and control, in every app, and a wallpaper to match.").dim(true));
    let p = style::palette(ui.ctx());
    let dark = p.mode == olo_kit::theme::ThemeMode::Dark;
    let chosen = themes::resolve(&s.appearance.icon_theme).id;
    let icons_w = SAMPLES.len() as f32 * ICON + (SAMPLES.len() - 1) as f32 * ICON_GAP;
    let etch = style::shade(&p, p.panel_background, 1.2);
    // Rows touch, an etched line between each.
    ui.spacing_mut().item_spacing.y = 0.0;
    for (i, t) in THEMES.iter().enumerate() {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
        let painter = ui.painter().clone();
        if i > 0 {
            // Etched, as `widgets::separator`.
            style::hline_above(&painter, row.left(), row.right(), row.top(), etch);
            style::hline(&painter, row.left(), row.right(), row.top(), style::c(p.shine));
        }
        // Anywhere on the row chooses it; the radio button is on top.
        if ui.interact(row, ui.scope_id().with(("theme-row", t.id)), Sense::click()).clicked() {
            choose(s, t.id);
        }
        let icons_at = row.right() - icons_w;
        let word = Rect::from_center_size(pos2(icons_at - m::SPACING - WORD_SLOT / 2.0, row.center().y), vec2(WORD_SLOT, WORD));
        let text_w = word.left() - row.left() - m::SPACING;
        let name_y = row.center().y - m::MARK_ROW / 2.0;
        let radio = Rect::from_min_size(pos2(row.left(), name_y - m::MARK_ROW / 2.0), vec2(text_w, m::MARK_ROW));
        if at_left(ui, radio, RadioButton::new(t.id == chosen, t.name)).clicked() {
            choose(s, t.id);
        }
        // The line about it, under the name.
        let indent = row.left() + 13.0 + m::MARK_GAP;
        let about = Rect::from_min_size(pos2(indent, name_y + m::MARK_ROW / 2.0), vec2(text_w - (indent - row.left()), m::LABEL_ROW));
        at_left(ui, about, StringView::new(t.about).dim(true));
        let w = brand::wordmark_width_in(t.id, WORD, dark).min(WORD_SLOT);
        let h = WORD * w / brand::wordmark_width_in(t.id, WORD, dark).max(1.0);
        brand::wordmark_centred_in(&painter, Rect::from_center_size(word.center(), vec2(w, h)), dark, t.id);
        for (k, name) in SAMPLES.iter().enumerate() {
            let at = pos2(icons_at + k as f32 * (ICON + ICON_GAP), row.center().y - ICON / 2.0);
            icons::paint_in(&painter, Rect::from_min_size(at, vec2(ICON, ICON)), t.id, name);
        }
    }
    ui.spacing_mut().item_spacing.y = m::SPACING;
    ui.add_space(m::SPACING);
    colors(ui, s);
}

/// The chosen theme's colour sets, each a preview over its name and a
/// line about it; clicking any of it chooses the set.
fn colors(ui: &mut Ui, s: &mut Settings) {
    let theme = themes::resolve(&s.appearance.icon_theme);
    let chosen = ColorSet::resolve(&s.appearance.colors);
    let title = format!("{} Colors", theme.name);
    BBox::new(&title).show(ui, |ui| {
        let n = ColorSet::ALL.len() as f32;
        let w = ((ui.available_width() - (n - 1.0) * m::SPACING) / n).floor();
        let (all, _) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_H), Sense::hover());
        for (i, set) in ColorSet::ALL.into_iter().enumerate() {
            let card = Rect::from_min_size(pos2(all.left() + i as f32 * (w + m::SPACING), all.top()), vec2(w, CARD_H));
            let name = set.name(theme);
            let resp = ui.interact(card, ui.scope_id().with(("colors", set.id())), Sense::click());
            resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, set == chosen, name));
            if resp.clicked() {
                s.appearance.colors = set.id().to_owned();
            }
            if set == chosen {
                style::selection_plate(ui.painter(), card, &style::palette(ui.ctx()));
            }
            let preview = style::snap(ui.ctx(), Rect::from_min_size(card.min, vec2(w, PREVIEW_H)));
            paint_preview(ui, preview, theme.id, set);
            let radio = Rect::from_min_size(pos2(card.left(), preview.bottom() + m::TIGHT), vec2(w, m::MARK_ROW));
            if at_left(ui, radio, RadioButton::new(set == chosen, name)).clicked() {
                s.appearance.colors = set.id().to_owned();
            }
            let about = Rect::from_min_size(pos2(card.left(), radio.bottom()), vec2(w, ABOUT_H));
            let about_text = set.about(theme);
            let mut text = StringView::new(&about_text).dim(true).wrap(true);
            if set == chosen {
                text = text.color(style::selection_ink(&style::palette(ui.ctx())).dim);
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(about), |ui| ui.add(text));
        }
    });
}

/// Colour set `set` of theme `theme` in `r`: a small window in its light
/// palette on the left and its dark one on the right, framed.
fn paint_preview(ui: &Ui, r: Rect, theme: &str, set: ColorSet) {
    let p = style::palette(ui.ctx());
    let painter = ui.painter();
    let half = (r.width() / 2.0).floor();
    for (k, mode) in [ThemeMode::Light, ThemeMode::Dark].into_iter().enumerate() {
        let look = looks::palette(theme, set.id(), mode);
        let side = Rect::from_min_size(pos2(r.left() + k as f32 * half, r.top()), vec2(if k == 0 { half } else { r.width() - half }, r.height()));
        mini_window(painter, side, &look);
    }
    style::frame_rect(painter, r, c(p.control_border));
}

/// A window in palette `p`, wireframe-small: its desk, a focused tab, the
/// frame, a list with a row chosen, a button and a default button.
fn mini_window(painter: &egui::Painter, r: Rect, p: &Palette) {
    let fill = |r: Rect, rgba| painter.rect_filled(style::snap(painter.ctx(), r), 0.0, c(rgba));
    // A bar of "text": `w` long, centred on `y`.
    let text = |x: f32, y: f32, w: f32, rgba| fill(Rect::from_min_size(pos2(x, (y - 1.0).round()), vec2(w.round(), 2.0)), rgba);
    fill(r, olo_kit::theme::tint(p.panel_background, if p.is_dark() { 1.25 } else { 1.12 }));
    // The tab.
    let tab = Rect::from_min_size(pos2(r.left() + 6.0, r.top() + 5.0), vec2((r.width() * 0.55).round(), 10.0));
    fill(tab, p.frame_focused[0]);
    let face = tab.shrink2(vec2(1.0, 0.0)).with_max_y(tab.bottom()).translate(vec2(0.0, 1.0)).with_max_y(tab.bottom());
    fill(face, p.tab_focused);
    fill(Rect::from_min_size(face.min, vec2(face.width(), 1.0)), p.tab_focused_light);
    text(face.left() + 4.0, face.center().y, face.width() * 0.5, p.tab_text_focused);
    // The window: frame, panel.
    let win = Rect::from_min_max(pos2(r.left() + 6.0, tab.bottom()), pos2(r.right() - 6.0, r.bottom() - 5.0));
    fill(win, p.frame_focused[0]);
    fill(win.shrink(1.0), p.frame_focused[1]);
    let panel = Rect::from_min_max(win.min + vec2(2.0, 2.0), win.max - vec2(1.0, 1.0));
    fill(panel, p.panel_background);
    // A list, its second row chosen.
    let list = Rect::from_min_max(panel.min + vec2(3.0, 3.0), pos2(panel.right() - 3.0, panel.top() + 18.0));
    fill(list, p.control_border);
    let doc = list.shrink(1.0);
    fill(doc, p.list_background);
    let row = 5.0;
    fill(Rect::from_min_size(pos2(doc.left(), doc.top() + row), vec2(doc.width(), row)), p.list_selected_background);
    for (i, w) in [0.6, 0.45, 0.7].into_iter().enumerate() {
        let color = if i == 1 { p.list_selected_item_text } else { p.list_item_text };
        text(doc.left() + 2.0, doc.top() + row * (i as f32 + 0.5), (doc.width() - 4.0) * w, color);
    }
    // A button, and the default one in the highlight colour.
    let bw = ((panel.width() - 9.0) / 2.0).floor();
    let button = Rect::from_min_size(pos2(panel.left() + 3.0, list.bottom() + 3.0), vec2(bw, (panel.bottom() - list.bottom() - 6.0).max(4.0)));
    fill(button, p.control_border);
    fill(button.shrink(1.0), p.control_background);
    text(button.center().x - bw * 0.25, button.center().y, bw * 0.5, p.control_text);
    let default = button.translate(vec2(bw + 3.0, 0.0));
    fill(default, p.control_border);
    fill(default.shrink(1.0), p.menu_selected_background);
    text(default.center().x - bw * 0.25, default.center().y, bw * 0.5, p.menu_selected_item_text);
}

/// Switch to theme `id`, and to its wallpaper.
fn choose(s: &mut Settings, id: &str) {
    s.appearance.icon_theme = id.to_owned();
    s.wallpaper = Some(themes::wallpaper(id));
}

/// `widget` at the left of `rect`, centred vertically (`Ui::put` would
/// centre it across too).
fn at_left(ui: &mut Ui, rect: Rect, widget: impl egui::Widget) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| ui.add(widget)).inner
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
