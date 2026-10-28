//! Themes: how every icon and the Olo wordmark look, everywhere (Deskbar,
//! Tracker, Twitcher, every app). A row per theme, each drawn in that
//! theme: its name and a line about it, its wordmark and a few of its
//! icons. Choosing one sets `Appearance::icon_theme`, which every app
//! follows at once.

use olo_kit::{
    brand,
    egui::{self, Rect, Sense, Ui, pos2, vec2},
    icons, metrics as m, style,
    themes::{self, THEMES},
    widgets::{RadioButton, StringView},
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

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    ui.add(StringView::new("The look of every icon and of the Olo wordmark, in every app.").dim(true));
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
            style::hline(&painter, row.left(), row.right(), row.top() - 1.0, etch);
            style::hline(&painter, row.left(), row.right(), row.top(), style::c(p.shine));
        }
        // Anywhere on the row chooses it; the radio button is on top.
        if ui.interact(row, ui.scope_id().with(("theme-row", t.id)), Sense::click()).clicked() {
            s.appearance.icon_theme = t.id.to_owned();
        }
        let icons_at = row.right() - icons_w;
        let word = Rect::from_center_size(pos2(icons_at - m::SPACING - WORD_SLOT / 2.0, row.center().y), vec2(WORD_SLOT, WORD));
        let text_w = word.left() - row.left() - m::SPACING;
        let name_y = row.center().y - m::MARK_ROW / 2.0;
        let radio = Rect::from_min_size(pos2(row.left(), name_y - m::MARK_ROW / 2.0), vec2(text_w, m::MARK_ROW));
        if at_left(ui, radio, RadioButton::new(t.id == chosen, t.name)).clicked() {
            s.appearance.icon_theme = t.id.to_owned();
        }
        // The line about it, under the name.
        let indent = row.left() + 13.0 + m::MARK_GAP;
        let about = Rect::from_min_size(pos2(indent, name_y + m::MARK_ROW / 2.0), vec2(text_w - (indent - row.left()), m::LABEL_ROW));
        at_left(ui, about, StringView::new(t.about).dim(true));
        let w = brand::wordmark_width_in(t.id, WORD).min(WORD_SLOT);
        let h = WORD * w / brand::wordmark_width_in(t.id, WORD).max(1.0);
        brand::wordmark_in(&painter, Rect::from_center_size(word.center(), vec2(w, h)), dark, t.id);
        for (k, name) in SAMPLES.iter().enumerate() {
            let at = pos2(icons_at + k as f32 * (ICON + ICON_GAP), row.center().y - ICON / 2.0);
            icons::paint_in(&painter, Rect::from_min_size(at, vec2(ICON, ICON)), t.id, name);
        }
    }
}

/// `widget` at the left of `rect`, centred vertically (`Ui::put` would
/// centre it across too).
fn at_left(ui: &mut Ui, rect: Rect, widget: impl egui::Widget) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| ui.add(widget)).inner
}
