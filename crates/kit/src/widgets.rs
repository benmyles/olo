//! The BeOS control set, drawn pixel-exact from the palette: buttons,
//! check boxes, radio buttons, text controls, sliders, menu fields, boxes,
//! status bars, scroll views, column list views and tab views.
//!
//! Each control is an [`egui::Widget`] (or has a `show` method for the
//! container ones), so apps mix them freely with ordinary egui layout.
//! Geometry is in logical pixels; one "line" is one logical pixel.

use std::{collections::BTreeSet, ops::RangeInclusive};

use egui::{
    Align, Color32, CursorIcon, FontId, Galley, Id, Key, Painter, Pos2, Rect, Response, Sense, Shape, Stroke,
    Ui, Vec2, Widget, WidgetInfo, Role, pos2, vec2,
};
use olo_theme::Palette;

use crate::{
    menu::{self, Menu, MenuAnchor, MenuRequest},
    style::{self, bold_font, c, frame_rect, hline, plain_font, shade, vline},
};

pub use crate::metrics::CONTROL_HEIGHT;
use crate::metrics::{self as m};
/// Width of a scroll bar (B_V_SCROLL_BAR_WIDTH).
pub const SCROLL_BAR: f32 = 14.0;
const MARK_GAP: f32 = m::MARK_GAP;

fn text_color(ui: &Ui, p: &Palette) -> Color32 {
    if ui.is_enabled() { c(p.control_text) } else { style::disabled_text(p) }
}

fn layout(ui: &Ui, text: &str, font: FontId, color: Color32) -> std::sync::Arc<Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

/// Galley top-left that centres its capitals on `center_y`, snapped to
/// device pixels.
fn text_pos(ui: &Ui, x: f32, center_y: f32, galley: &Galley) -> Pos2 {
    let size = galley.job.sections.first().map_or(style::PLAIN_SIZE, |s| s.format.font_id.size);
    style::text_origin(ui.ctx(), galley, x, center_y, size)
}

/// BeOS marks keyboard focus by underlining the label in the navigation
/// colour.
fn focus_underline(painter: &Painter, text: Rect, p: &Palette) {
    hline(painter, text.left(), text.right(), text.bottom() - 1.0, c(p.keyboard_navigation));
}

// ---- Button -----------------------------------------------------------

/// BButton: a bevelled push button. The default button carries a recessed
/// ring and answers Enter anywhere in the window.
pub struct Button<'a> {
    label: &'a str,
    default: bool,
    min_width: f32,
    pushed: bool,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Button { label, default: false, min_width: m::BUTTON_MIN_WIDTH, pushed: false }
    }
    /// Draw it held down: the chosen one of a set of tool buttons.
    pub fn pushed(mut self, on: bool) -> Self {
        self.pushed = on;
        self
    }
    pub fn default(mut self, default: bool) -> Self {
        self.default = default;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
}

impl Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = text_color(ui, &p);
        let galley = layout(ui, self.label, plain_font(), color);
        // The default ring is drawn outside the button's own rect, so a
        // row of buttons stays aligned.
        let ring = if self.default { 3.0 } else { 0.0 };
        let w = (galley.size().x + 2.0 * m::CONTROL_PAD).max(self.min_width).round();
        let (r, mut response) = ui.allocate_exact_size(vec2(w, CONTROL_HEIGHT), Sense::click());
        let outer = r.expand(ring);
        if self.default && ui.is_enabled() && ui.input(|i| i.key_pressed(Key::Enter)) && !ui.ctx().egui_wants_keyboard_input()
        {
            response.mark_changed();
            response.flags |= egui::response::Flags::FAKE_PRIMARY_CLICKED;
        }
        response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), self.label));
        if !ui.is_rect_visible(outer) {
            return response;
        }
        let painter = ui.painter();
        let bg = p.panel_background;
        if self.default {
            // A recessed groove the button sits in.
            painter.rect_filled(outer, 0.0, shade(&p, bg, 1.06));
            style::bevel(painter, outer, shade(&p, bg, 1.3), c(p.shine));
        }
        let pressed = response.is_pointer_button_down_on() && response.hovered() || self.pushed;
        let hovered = response.hovered() && ui.is_enabled();
        let base = p.control_background;
        let (top, bottom) = if pressed {
            (shade(&p, base, 1.2), shade(&p, base, 1.12))
        } else if hovered {
            (c(olo_theme::tint(base, 0.7)), shade(&p, base, 1.02))
        } else {
            (c(olo_theme::tint(base, 0.85)), shade(&p, base, 1.06))
        };
        let border = if ui.is_enabled() { shade(&p, bg, 1.6) } else { shade(&p, bg, 1.3) };
        style::vgradient(painter, r.shrink(1.0), top, bottom);
        frame_rect(painter, r, border);
        let inner = r.shrink(1.0);
        if pressed {
            style::bevel(painter, inner, shade(&p, base, 1.35), shade(&p, base, 1.1));
        } else {
            style::bevel(painter, inner, c(p.shine), shade(&p, base, 1.2));
        }
        let off = if pressed { 1.0 } else { 0.0 };
        let pos = text_pos(ui, r.center().x - galley.size().x / 2.0 + off, r.center().y + off, &galley);
        let text_rect = Rect::from_min_size(pos, galley.size());
        painter.galley(pos, galley, color);
        if response.has_focus() {
            focus_underline(painter, text_rect, &p);
        }
        response
    }
}

// ---- CheckBox / RadioButton ---------------------------------------------

/// The 12px sunken well shared by check boxes.
fn mark_well(painter: &Painter, r: Rect, p: &Palette, pressed: bool, enabled: bool) {
    let fill = if pressed {
        shade(p, p.document_background, 1.12)
    } else if enabled {
        c(p.document_background)
    } else {
        c(p.panel_background)
    };
    painter.rect_filled(r, 0.0, fill);
    style::bevel(painter, r, shade(p, p.panel_background, 1.18), c(p.shine));
    let inner = r.shrink(1.0);
    style::bevel(painter, inner, shade(p, p.panel_background, 1.6), shade(p, p.panel_background, 1.08));
}

/// BCheckBox.
pub struct CheckBox<'a> {
    value: &'a mut bool,
    label: &'a str,
}

impl<'a> CheckBox<'a> {
    pub fn new(value: &'a mut bool, label: &'a str) -> Self {
        CheckBox { value, label }
    }
}

impl Widget for CheckBox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = text_color(ui, &p);
        let galley = layout(ui, self.label, plain_font(), color);
        let size = vec2(12.0 + MARK_GAP + galley.size().x.ceil(), m::MARK_ROW);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
        if response.clicked() {
            *self.value = !*self.value;
            response.mark_changed();
        }
        let checked = *self.value;
        response.widget_info(|| WidgetInfo::selected(Role::CheckBox, ui.is_enabled(), checked, self.label));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        let pressed = response.is_pointer_button_down_on() && response.hovered();
        // Centred on the label's x-height rather than its line box.
        let well = Rect::from_min_size(pos2(rect.left(), (rect.center().y - 6.5).round()), vec2(12.0, 12.0));
        mark_well(painter, well, &p, pressed, ui.is_enabled());
        if checked {
            let mark = if ui.is_enabled() { c(p.control_mark) } else { style::disabled_text(&p) };
            style::check_mark(painter, Rect::from_min_max(well.min + vec2(3.0, 3.5), well.max - vec2(2.5, 3.5)), mark);
        }
        let pos = text_pos(ui, well.right() + MARK_GAP, rect.center().y, &galley);
        let text_rect = Rect::from_min_size(pos, galley.size());
        painter.galley(pos, galley, color);
        if response.has_focus() {
            focus_underline(painter, text_rect, &p);
        }
        response
    }
}

/// BRadioButton. Returns a response that is `clicked()` when chosen; the
/// caller sets the group's value.
pub struct RadioButton<'a> {
    selected: bool,
    label: &'a str,
}

impl<'a> RadioButton<'a> {
    pub fn new(selected: bool, label: &'a str) -> Self {
        RadioButton { selected, label }
    }
}

impl Widget for RadioButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = text_color(ui, &p);
        let galley = layout(ui, self.label, plain_font(), color);
        let size = vec2(13.0 + MARK_GAP + galley.size().x.ceil(), m::MARK_ROW);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), self.selected, self.label));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        let pressed = response.is_pointer_button_down_on() && response.hovered();
        let center = pos2(rect.left() + 6.5, rect.center().y.round() - 0.5);
        let bg = p.panel_background;
        // Outer ring: dark upper-left, light lower-right, like the well.
        painter.circle_filled(center + vec2(0.5, 0.5), 6.5, c(p.shine));
        painter.circle_filled(center - vec2(0.3, 0.3), 6.5, shade(&p, bg, 1.25));
        let fill = if pressed {
            shade(&p, p.document_background, 1.12)
        } else if ui.is_enabled() {
            c(p.document_background)
        } else {
            c(bg)
        };
        painter.circle(center, 5.5, fill, Stroke::new(1.0, shade(&p, bg, 1.55)));
        if self.selected {
            let mark = if ui.is_enabled() { c(p.control_mark) } else { style::disabled_text(&p) };
            painter.circle_filled(center, 3.0, mark);
            // A small specular highlight keeps the dot from looking flat.
            painter.circle_filled(center - vec2(1.0, 1.0), 0.9, mark.linear_multiply(0.4).additive());
        }
        let pos = text_pos(ui, rect.left() + 13.0 + MARK_GAP, rect.center().y, &galley);
        let text_rect = Rect::from_min_size(pos, galley.size());
        painter.galley(pos, galley, color);
        if response.has_focus() {
            focus_underline(painter, text_rect, &p);
        }
        response
    }
}

// ---- TextControl ------------------------------------------------------

/// BTextControl: an optional label and a sunken single-line text well.
pub struct TextControl<'a> {
    text: &'a mut String,
    label: Option<&'a str>,
    divider: Option<f32>,
    width: Option<f32>,
    hint: Option<&'a str>,
    password: bool,
    mono: bool,
    autofocus: bool,
}

impl<'a> TextControl<'a> {
    pub fn new(text: &'a mut String) -> Self {
        TextControl { text, label: None, divider: None, width: None, hint: None, password: false, mono: false, autofocus: false }
    }
    /// Take keyboard focus when the window first shows (the first field of
    /// a dialog or a Find window), with its text selected. Keeps asking
    /// until focus arrives, since a window that isn't yet activated can't
    /// hold it.
    pub fn autofocus(mut self, on: bool) -> Self {
        self.autofocus = on;
        self
    }
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
    /// Where the text well begins, measured from the control's left edge.
    pub fn divider(mut self, x: f32) -> Self {
        self.divider = Some(x);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }
    pub fn password(mut self, on: bool) -> Self {
        self.password = on;
        self
    }
    pub fn mono(mut self, on: bool) -> Self {
        self.mono = on;
        self
    }
}

impl Widget for TextControl<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover());
        let mut well = rect;
        if let Some(label) = self.label {
            let color = text_color(ui, &p);
            let galley = layout(ui, label, plain_font(), color);
            let divider = self.divider.unwrap_or(galley.size().x.ceil() + m::LABEL_GAP);
            let pos = text_pos(ui, rect.left(), rect.center().y, &galley);
            ui.painter().galley(pos, galley, color);
            well.min.x = rect.left() + divider;
        }
        let id = ui.next_auto_id();
        if self.autofocus {
            let done = id.with("autofocused");
            if ui.memory(|m| m.has_focus(id)) {
                if !ui.data(|d| d.get_temp::<bool>(done).unwrap_or(false)) {
                    // As BeOS dialogs: the text starts selected, so typing
                    // replaces it.
                    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
                    let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.text.chars().count()));
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), id);
                }
                ui.data_mut(|d| d.insert_temp(done, true));
            } else if !ui.data(|d| d.get_temp::<bool>(done).unwrap_or(false)) {
                ui.memory_mut(|m| m.request_focus(id));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        let focused = ui.memory(|m| m.has_focus(id));
        let painter = ui.painter();
        let fill = if ui.is_enabled() { c(p.document_background) } else { c(p.panel_background) };
        painter.rect_filled(well.shrink(2.0), 0.0, fill);
        style::sunken(painter, well, &p, focused);
        let font = if self.mono { style::mono_font(style::MONO_SIZE) } else { plain_font() };
        // Place the editor on exactly the line a label would use, so text
        // in the well shares the label's baseline.
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let probe = ui.painter().layout_no_wrap("X".into(), font.clone(), Color32::BLACK);
        let top = style::text_origin(ui.ctx(), &probe, 0.0, well.center().y, font.size).y;
        let line = Rect::from_min_max(pos2(well.left() + m::TEXT_INSET, top), pos2(well.right() - m::TEXT_INSET, top + row_h));
        let enabled = ui.is_enabled();
        let mut drag = crate::text_drag::TextDrag::new(ui, id, enabled, self.password, false);
        if let Some(d) = drag.take_drop() {
            let r = crate::text_drag::apply(&mut *self.text, &d);
            crate::text_drag::select(ui.ctx(), id, r);
        }
        let mut edit = egui::TextEdit::singleline(&mut *self.text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(font)
            .text_color(c(p.document_text))
            .password(self.password)
            .desired_width(line.width());
        if let Some(hint) = self.hint {
            edit = edit.hint_text(egui::RichText::new(hint).color(style::disabled_text(&p)));
        }
        // The whole well focuses the editor, not just the text line (not a
        // Tab stop of its own: Tab goes from field to field). Made before
        // the editor so the editor is on top: double and triple clicks on
        // the text must reach it.
        let well_resp = ui.interact(well, id.with("well"), Sense::CLICK);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(line));
        child.set_clip_rect(well.shrink(2.0).intersect(ui.clip_rect()));
        let out = edit.interactive(drag.interactive()).show(&mut child);
        drag.finish(ui, &out);
        let r = out.response.response;
        if let Some(label) = self.label {
            // For agents (the widget tree): the field is named by its label.
            let label = label.trim_end_matches(':').to_owned();
            ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(label));
        }
        if well_resp.clicked() {
            ui.memory_mut(|m| m.request_focus(id));
        }
        if well_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        edit_menu(ui, id, &(r.clone() | well_resp), self.text, ui.is_enabled(), self.password);
        r
    }
}

/// A line of static text (BStringView) one control tall, so it sits on the
/// same baseline as the labels of neighbouring controls in a row. Text
/// wider than the space left is cut short with "…".
pub struct StringView<'a> {
    text: &'a str,
    dim: bool,
    wrap: bool,
    color: Option<Color32>,
}

impl<'a> StringView<'a> {
    pub fn new(text: &'a str) -> Self {
        StringView { text, dim: false, wrap: false, color: None }
    }
    /// Secondary text (status, hints) in the disabled-text colour.
    pub fn dim(mut self, on: bool) -> Self {
        self.dim = on;
        self
    }
    /// Wrap onto as many lines as it takes (a paragraph), rather than
    /// cutting it short with an ellipsis.
    pub fn wrap(mut self, on: bool) -> Self {
        self.wrap = on;
        self
    }
    /// Text in a colour of its own (from the palette: an error in
    /// `failure`).
    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }
}

impl Widget for StringView<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = self.color.unwrap_or_else(|| if self.dim { style::disabled_text(&p) } else { text_color(ui, &p) });
        let width = ui.available_width().max(0.0);
        let mut job = egui::text::LayoutJob::simple_singleline(self.text.to_owned(), plain_font(), color);
        job.wrap = if self.wrap { egui::text::TextWrapping::wrap_at_width(width) } else { egui::text::TextWrapping::truncate_at_width(width) };
        let galley = ui.painter().layout_job(job);
        if self.wrap && galley.rows.len() > 1 {
            // A paragraph: lines at the font's own spacing, the first
            // where a single line's would be.
            let pad = (CONTROL_HEIGHT - galley.rows[0].height()).max(0.0) / 2.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x.ceil(), galley.size().y + 2.0 * pad), Sense::hover());
            ui.painter().galley(pos2(rect.left(), rect.top() + pad), galley, color);
            return resp;
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x.ceil(), CONTROL_HEIGHT), Sense::hover());
        let pos = text_pos(ui, rect.left(), rect.center().y, &galley);
        ui.painter().galley(pos, galley, color);
        resp
    }
}

/// Lays out a [`TextView`]'s text in colour: the text, its font and the
/// plain text colour.
pub type Highlighter<'a> = dyn Fn(&str, FontId, Color32) -> egui::text::LayoutJob + 'a;

/// A multi-line text well with a BeOS scroll bar (BTextView in a
/// BScrollView).
pub struct TextView<'a> {
    id: Id,
    text: &'a mut String,
    mono: bool,
    editable: bool,
    highlight: Option<&'a Highlighter<'a>>,
}

impl<'a> TextView<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, text: &'a mut String) -> Self {
        TextView { id: Id::unique(id), text, mono: false, editable: true, highlight: None }
    }
    /// Colour the text: `highlight` lays it out (given the font and the
    /// plain text colour) in sections of their own colours. It runs when
    /// the text changes, so keep it quick.
    pub fn highlight(mut self, highlight: &'a Highlighter<'a>) -> Self {
        self.highlight = Some(highlight);
        self
    }
    pub fn mono(mut self, on: bool) -> Self {
        self.mono = on;
        self
    }
    pub fn editable(mut self, on: bool) -> Self {
        self.editable = on;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let font = if self.mono { style::mono_font(style::MONO_SIZE) } else { plain_font() };
        let edit_id = self.id.with("edit");
        let focused = ui.memory(|m| m.has_focus(edit_id));
        let editable = self.editable;
        let text = self.text;
        let highlight = self.highlight;
        let mut drag = crate::text_drag::TextDrag::new(ui, edit_id, editable, false, true);
        if let Some(d) = drag.take_drop() {
            let r = crate::text_drag::apply(&mut *text, &d);
            crate::text_drag::select(ui.ctx(), edit_id, r);
        }
        let r = ScrollView::new(self.id)
            .focused(focused)
            .background(p.document_background)
            .show(ui, |ui| {
                let size = ui.available_size();
                let color = c(p.document_text);
                let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let mut job = highlight.map_or_else(
                        || egui::text::LayoutJob::simple(buf.as_str().to_owned(), font.clone(), color, wrap),
                        |h| h(buf.as_str(), font.clone(), color),
                    );
                    job.wrap.max_width = wrap;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };
                let mut edit = egui::TextEdit::multiline(&mut *text)
                    .id(edit_id)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(6, 4)))
                    .font(font.clone())
                    .text_color(color)
                    .interactive(editable && drag.interactive())
                    .desired_width(f32::INFINITY)
                    .min_size(size);
                if highlight.is_some() {
                    edit = edit.layouter(&mut layouter);
                }
                let out = edit.show(ui);
                drag.finish(ui, &out);
                out.response.response
            });
        edit_menu(ui, edit_id, &r, text, editable, false);
        r
    }
}

// ---- Slider -----------------------------------------------------------

/// BSlider: label and value above a sunken bar with a filled portion and a
/// raised block thumb.
pub struct Slider<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    label: Option<&'a str>,
    value_text: Option<String>,
    limits: Option<(&'a str, &'a str)>,
    fill: Option<Color32>,
    step: f32,
    width: Option<f32>,
}

impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        Slider { value, range, label: None, value_text: None, limits: None, fill: None, step: 0.0, width: None }
    }
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
    /// Text shown at the right above the bar (e.g. "75 %").
    pub fn value_text(mut self, text: impl Into<String>) -> Self {
        self.value_text = Some(text.into());
        self
    }
    /// Labels under the ends of the bar.
    pub fn limits(mut self, min: &'a str, max: &'a str) -> Self {
        self.limits = Some((min, max));
        self
    }
    /// Colour of the bar left of the thumb; defaults to the highlight.
    pub fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self
    }
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

const THUMB_W: f32 = 11.0;
const THUMB_H: f32 = 17.0;
const BAR_H: f32 = 6.0;

impl Widget for Slider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = text_color(ui, &p);
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let top = if self.label.is_some() || self.value_text.is_some() { m::LABEL_ROW + m::TIGHT } else { 0.0 };
        let bottom = if self.limits.is_some() { m::LABEL_ROW } else { 0.0 };
        let height = top + THUMB_H + 2.0 + bottom;
        let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let track = Rect::from_min_max(
            pos2(rect.left() + THUMB_W / 2.0, rect.top() + top),
            pos2(rect.right() - THUMB_W / 2.0, rect.top() + top + THUMB_H),
        );
        let to_value = |x: f32| {
            let t = ((x - track.left()) / track.width()).clamp(0.0, 1.0);
            let v = lo + t * (hi - lo);
            if self.step > 0.0 { (v / self.step).round() * self.step } else { v }
        };
        let mut v = *self.value;
        if let Some(pos) = response.interact_pointer_pos()
            && (response.dragged() || response.clicked())
        {
            v = to_value(pos.x);
        }
        if response.has_focus() {
            let step = if self.step > 0.0 { self.step } else { (hi - lo) / 100.0 };
            ui.input(|i| {
                if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowDown) {
                    v -= step;
                }
                if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowUp) {
                    v += step;
                }
                if i.key_pressed(Key::Home) {
                    v = lo;
                }
                if i.key_pressed(Key::End) {
                    v = hi;
                }
            });
        }
        let v = v.clamp(lo, hi);
        if v != *self.value {
            *self.value = v;
            response.mark_changed();
        }
        response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), v as f64, self.label.unwrap_or("")));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        if let Some(label) = self.label {
            let g = layout(ui, label, plain_font(), color);
            let pos = text_pos(ui, rect.left(), rect.top() + m::LABEL_ROW / 2.0, &g);
            let text_rect = Rect::from_min_size(pos, g.size());
            painter.galley(pos, g, color);
            if response.has_focus() {
                focus_underline(painter, text_rect, &p);
            }
        }
        if let Some(text) = &self.value_text {
            let g = layout(ui, text, plain_font(), color);
            let pos = text_pos(ui, rect.right() - g.size().x, rect.top() + m::LABEL_ROW / 2.0, &g);
            painter.galley(pos, g, color);
        }
        // The bar.
        let bar = Rect::from_center_size(
            pos2(rect.center().x, (track.center().y - BAR_H / 2.0).round() + BAR_H / 2.0),
            vec2(rect.width(), BAR_H),
        );
        let t = if hi > lo { (v - lo) / (hi - lo) } else { 0.0 };
        let thumb_x = (track.left() + t * track.width()).round();
        let fill = self.fill.unwrap_or(c(p.control_highlight));
        let fill = if ui.is_enabled() { fill } else { fill.gamma_multiply(0.4) };
        painter.rect_filled(bar.shrink(1.0), 0.0, shade(&p, p.panel_background, 1.08));
        let filled = Rect::from_min_max(bar.min + vec2(1.0, 1.0), pos2(thumb_x, bar.bottom() - 1.0));
        style::vgradient(painter, filled, fill, fill.gamma_multiply(0.85).to_opaque());
        style::bevel(painter, bar, shade(&p, p.panel_background, 1.45), c(p.shine));
        // Thumb.
        let thumb = Rect::from_center_size(pos2(thumb_x + 0.5, track.center().y), vec2(THUMB_W, THUMB_H)).round_ui();
        let base = p.control_background;
        let pressed = response.is_pointer_button_down_on();
        let (tt, tb) = if pressed {
            (shade(&p, base, 1.15), shade(&p, base, 1.08))
        } else {
            (c(olo_theme::tint(base, 0.8)), shade(&p, base, 1.06))
        };
        // A soft drop shadow lifts the thumb off the bar.
        painter.rect_filled(thumb.translate(vec2(1.0, 1.0)), 1.0, Color32::from_black_alpha(40));
        style::vgradient(painter, thumb.shrink(1.0), tt, tb);
        frame_rect(painter, thumb, shade(&p, p.panel_background, 1.6));
        style::bevel(painter, thumb.shrink(1.0), c(p.shine), shade(&p, base, 1.2));
        // Grip line.
        let gx = thumb.center().x.floor();
        vline(painter, gx - 1.0, thumb.top() + 4.0, thumb.bottom() - 4.0, shade(&p, base, 1.35));
        vline(painter, gx, thumb.top() + 4.0, thumb.bottom() - 4.0, c(p.shine));
        if let Some((min, max)) = self.limits {
            let small = FontId::proportional(style::SMALL_SIZE);
            let y = rect.bottom() - m::LABEL_ROW / 2.0 + 1.0;
            let g = layout(ui, min, small.clone(), color);
            painter.galley(text_pos(ui, rect.left(), y, &g), g, color);
            let g = layout(ui, max, small, color);
            let x = rect.right() - g.size().x;
            painter.galley(text_pos(ui, x, y, &g), g, color);
        }
        response
    }
}

trait RoundUi {
    fn round_ui(self) -> Self;
}
impl RoundUi for Rect {
    fn round_ui(self) -> Rect {
        Rect::from_min_max(self.min.round(), self.max.round())
    }
}

// ---- MenuField / PopUpButton --------------------------------------------

/// BMenuField: an optional label and a pop-up menu showing the current
/// choice. Clicking opens the menu with the current item over the field.
pub struct MenuField<'a> {
    id: Id,
    label: Option<&'a str>,
    divider: Option<f32>,
    selected: &'a mut usize,
    options: &'a [&'a str],
    width: Option<f32>,
}

impl<'a> MenuField<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, selected: &'a mut usize, options: &'a [&'a str]) -> Self {
        MenuField { id: Id::unique(id), label: None, divider: None, selected, options, width: None }
    }
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
    pub fn divider(mut self, x: f32) -> Self {
        self.divider = Some(x);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

impl Widget for MenuField<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = text_color(ui, &p);
        let widest = self
            .options
            .iter()
            .map(|o| layout(ui, o, plain_font(), color).size().x)
            .fold(0.0, f32::max);
        let label_w = match self.label {
            Some(l) => self.divider.unwrap_or(layout(ui, l, plain_font(), color).size().x.ceil() + m::LABEL_GAP),
            None => 0.0,
        };
        let width = self.width.unwrap_or((label_w + m::TEXT_INSET + widest + 12.0 + 18.0).ceil());
        let (rect, mut response) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::click());
        let field = Rect::from_min_max(pos2(rect.left() + label_w, rect.top()), rect.max);
        let response_field = ui.interact(field, self.id, Sense::click());
        if let Some(cmd) = menu::take_chosen(ui.ctx(), self.id)
            && let Ok(i) = cmd.parse::<usize>()
            && i != *self.selected
        {
            *self.selected = i;
            response.mark_changed();
        }
        let open = response_field.is_pointer_button_down_on() && response_field.drag_started()
            || response_field.clicked();
        if open && !self.options.is_empty() {
            let mut m = Menu::new("");
            for (i, o) in self.options.iter().enumerate() {
                m = m.item(*o, i.to_string(), None).checked(i == *self.selected);
            }
            menu::request_popup(
                ui.ctx(),
                MenuRequest {
                    menu: m,
                    source: self.id,
                    anchor: MenuAnchor::Over(field, *self.selected),
                    selected: Some(*self.selected),
                },
            );
        }
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        if let Some(label) = self.label {
            let g = layout(ui, label, plain_font(), color);
            let pos = text_pos(ui, rect.left(), rect.center().y, &g);
            painter.galley(pos, g, color);
        }
        // Raised field with a drop-down mark.
        let base = p.menu_background;
        style::vgradient(painter, field.shrink(1.0), c(olo_theme::tint(base, 0.82)), shade(&p, base, 1.05));
        frame_rect(painter, field, shade(&p, p.panel_background, 1.55));
        style::bevel(painter, field.shrink(1.0), c(p.shine), shade(&p, base, 1.2));
        let text = self.options.get(*self.selected).copied().unwrap_or("");
        let g = layout(ui, text, plain_font(), color);
        let pos = text_pos(ui, field.left() + m::TEXT_INSET, field.center().y, &g);
        let text_rect = Rect::from_min_size(pos, g.size());
        painter.galley(pos, g, color);
        let ax = field.right() - 11.0;
        let ay = field.center().y.round();
        painter.add(Shape::convex_polygon(
            vec![pos2(ax - 4.0, ay - 2.0), pos2(ax + 4.0, ay - 2.0), pos2(ax, ay + 2.5)],
            color,
            Stroke::NONE,
        ));
        if response_field.has_focus() {
            focus_underline(painter, text_rect, &p);
        }
        let info = || WidgetInfo {
            label: self.label.map(|l| l.trim_end_matches(':').to_owned()),
            current_text_value: Some(text.to_owned()),
            enabled: ui.is_enabled(),
            ..WidgetInfo::new(Role::ComboBox)
        };
        response.widget_info(info);
        response_field.widget_info(info);
        response | response_field
    }
}

/// The right-click menu of a text field (Cut, Copy, Paste, Select All),
/// for the `egui::TextEdit` `id` editing `text`; call it after adding the
/// editor, with its response. TextControl and TextView have it already;
/// apps with their own `TextEdit` call this so every field behaves alike.
pub fn edit_menu(ui: &Ui, id: Id, response: &Response, text: &mut String, editable: bool, password: bool) {
    let ctx = ui.ctx();
    let source = id.with("edit-menu");
    // egui moves the caret on any press, the right button's too: keep the
    // selection a right-click lands on, so the menu can act on it.
    let kept = id.with("edit-menu-selection");
    let right = ctx.input(|i| (i.pointer.button_pressed(egui::PointerButton::Secondary), i.pointer.button_down(egui::PointerButton::Secondary)));
    if right.0 && response.hovered() {
        if let Some(range) = ctx.data(|d| d.get_temp::<egui::text::CCursorRange>(kept))
            && let Some(mut s) = egui::TextEdit::load_state(ctx, id)
        {
            s.cursor.set_char_range(Some(range));
            s.store(ctx, id);
        }
    } else if !right.1
        && let Some(range) = egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range())
    {
        ctx.data_mut(|d| d.insert_temp(kept, range));
    }
    let state = egui::TextEdit::load_state(ctx, id);
    // The selection as byte offsets, start first.
    let selection = state.as_ref().and_then(|s| s.cursor.char_range()).map(|r| {
        let (p, q): (usize, usize) = (r.primary.index.into(), r.secondary.index.into());
        let (a, b) = (p.min(q), p.max(q));
        let byte = |i: usize| text.char_indices().nth(i).map_or(text.len(), |(o, _)| o);
        (byte(a), byte(b), a)
    });
    let selected = selection.filter(|(a, b, _)| a < b);
    if response.secondary_clicked()
        && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
    {
        ctx.memory_mut(|m| m.request_focus(id));
        let copyable = selected.is_some() && !password;
        let menu = Menu::new("Edit")
            .item("Cut", "cut", Some(Key::X))
            .enabled(editable && copyable)
            .item("Copy", "copy", Some(Key::C))
            .enabled(copyable)
            .item("Paste", "paste", Some(Key::V))
            .enabled(editable)
            .separator()
            .item("Select All", "select-all", Some(Key::A))
            .enabled(!text.is_empty());
        menu::request_popup(ctx, MenuRequest { menu, source, anchor: MenuAnchor::At(pos), selected: None });
    }
    let Some(command) = menu::take_chosen(ctx, source) else { return };
    ctx.memory_mut(|m| m.request_focus(id));
    let set = |range: egui::text::CCursorRange| {
        let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        s.cursor.set_char_range(Some(range));
        s.store(ctx, id);
    };
    match command.as_str() {
        "copy" | "cut" if !password => {
            let Some((a, b, start)) = selected else { return };
            ctx.copy_text(text[a..b].to_owned());
            if command == "cut" && editable {
                text.replace_range(a..b, "");
                set(egui::text::CCursorRange::one(egui::text::CCursor::new(start)));
            }
        }
        "paste" if editable => menu::request_paste(ctx),
        "select-all" => set(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        )),
        _ => {}
    }
    ctx.request_repaint();
}

/// Show a context menu when `response` is right-clicked; returns the chosen
/// command (a frame or two later, once the popup closes).
pub fn context_menu(response: &Response, build: impl FnOnce() -> Menu) -> Option<String> {
    let ctx = &response.ctx;
    let id = response.id.with("context");
    if response.secondary_clicked()
        && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
    {
        menu::request_popup(ctx, MenuRequest { menu: build(), source: id, anchor: MenuAnchor::At(pos), selected: None });
    }
    menu::take_chosen(ctx, id)
}

/// Open `menu` at `pos` on behalf of `source`; poll the result with
/// [`menu::take_chosen`] (or [`popup_result`]) on later frames.
pub fn popup_menu_at(ctx: &egui::Context, source: Id, pos: Pos2, menu: Menu) {
    menu::request_popup(ctx, MenuRequest { menu, source, anchor: MenuAnchor::At(pos), selected: None });
}

/// The command chosen from a popup opened for `source`, once.
pub fn popup_result(ctx: &egui::Context, source: Id) -> Option<String> {
    menu::take_chosen(ctx, source)
}

// ---- BBox / separators / status bar -------------------------------------

/// A one-pixel etched line (dark over light).
pub fn separator(ui: &mut Ui) -> Response {
    let p = style::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let y = rect.center().y.floor();
    hline(ui.painter(), rect.left(), rect.right(), y - 1.0, shade(&p, p.panel_background, 1.2));
    hline(ui.painter(), rect.left(), rect.right(), y, c(p.shine));
    response
}

/// BBox: an etched frame with an optional bold label set into its top
/// edge.
pub struct BBox<'a> {
    label: Option<&'a str>,
}

impl<'a> BBox<'a> {
    pub fn new(label: &'a str) -> Self {
        BBox { label: Some(label) }
    }
    pub fn plain() -> Self {
        BBox { label: None }
    }

    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
        let p = style::palette(ui.ctx());
        let galley = self.label.map(|l| layout(ui, l, bold_font(), c(p.panel_text)));
        // The frame's top line runs through the middle of the label.
        let label_h = galley.as_ref().map_or(0.0, |g| g.size().y.round());
        let top = (label_h / 2.0).floor();
        let pad = m::BOX_INSET;
        let outer = ui.available_rect_before_wrap();
        let content_top = if galley.is_some() { outer.top() + label_h + m::TIGHT } else { outer.top() + pad };
        let content = Rect::from_min_max(
            pos2(outer.left() + pad, content_top),
            pos2(outer.right() - pad, outer.bottom() - pad),
        );
        let horizontal = ui.layout().is_horizontal();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
        // Items in a box are a group: stack them tighter.
        child.spacing_mut().item_spacing.y = m::TIGHT;
        let r = add(&mut child);
        let used = child.min_rect();
        let right = if horizontal {
            let label_w = galley.as_ref().map_or(0.0, |g| g.size().x + 24.0);
            (used.right() + pad).max(outer.left() + label_w).min(outer.right())
        } else {
            outer.right()
        };
        let frame = Rect::from_min_max(pos2(outer.left(), outer.top() + top), pos2(right, used.bottom() + pad));
        let painter = ui.painter();
        let dark = shade(&p, p.panel_background, 1.22);
        let light = c(p.shine);
        // Etched rectangle: dark line, then light line inset by one.
        let gap = galley.as_ref().map(|g| (frame.left() + pad - 4.0, frame.left() + pad + g.size().x.ceil() + 4.0));
        let etched_h = |y: f32, col: Color32| match gap {
            Some((a, b)) => {
                hline(painter, frame.left(), a, y, col);
                hline(painter, b, frame.right(), y, col);
            }
            None => hline(painter, frame.left(), frame.right(), y, col),
        };
        etched_h(frame.top(), dark);
        etched_h(frame.top() + 1.0, light);
        hline(painter, frame.left(), frame.right(), frame.bottom() - 2.0, dark);
        hline(painter, frame.left(), frame.right(), frame.bottom() - 1.0, light);
        vline(painter, frame.left(), frame.top(), frame.bottom() - 1.0, dark);
        vline(painter, frame.left() + 1.0, frame.top() + 1.0, frame.bottom() - 2.0, light);
        vline(painter, frame.right() - 2.0, frame.top(), frame.bottom() - 1.0, dark);
        vline(painter, frame.right() - 1.0, frame.top(), frame.bottom(), light);
        if let (Some(g), Some((a, _))) = (galley, gap) {
            let pos = text_pos(ui, a + 4.0, outer.top() + label_h / 2.0, &g);
            painter.galley(pos, g, c(p.panel_text));
        }
        ui.advance_cursor_after_rect(frame);
        r
    }
}

/// BStatusBar: a label, trailing text and a progress bar.
pub struct StatusBar<'a> {
    fraction: f32,
    label: Option<&'a str>,
    trailing: Option<&'a str>,
    color: Option<Color32>,
    height: f32,
}

impl<'a> StatusBar<'a> {
    pub fn new(fraction: f32) -> Self {
        StatusBar { fraction, label: None, trailing: None, color: None, height: 14.0 }
    }
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
    pub fn trailing(mut self, text: &'a str) -> Self {
        self.trailing = Some(text);
        self
    }
    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }
    pub fn bar_height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
}

impl Widget for StatusBar<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let color = c(p.panel_text);
        let top = if self.label.is_some() || self.trailing.is_some() { m::LABEL_ROW + m::TIGHT } else { 0.0 };
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), top + self.height), Sense::hover());
        let painter = ui.painter();
        if let Some(label) = self.label {
            let g = layout(ui, label, plain_font(), color);
            painter.galley(text_pos(ui, rect.left(), rect.top() + m::LABEL_ROW / 2.0, &g), g, color);
        }
        if let Some(t) = self.trailing {
            let g = layout(ui, t, plain_font(), color);
            let x = rect.right() - g.size().x;
            painter.galley(text_pos(ui, x, rect.top() + m::LABEL_ROW / 2.0, &g), g, color);
        }
        let bar = Rect::from_min_max(pos2(rect.left(), rect.top() + top), rect.max);
        level_bar(painter, bar, self.fraction, self.color);
        response
    }
}

/// [`StatusBar`]'s bar alone, for where there's only a painter (a list
/// cell): a sunken well filled to `fraction` (with `fill`, or the
/// palette's status bar colour).
pub fn level_bar(painter: &Painter, bar: Rect, fraction: f32, fill: Option<Color32>) {
    let p = style::palette(painter.ctx());
    painter.rect_filled(bar.shrink(1.0), 0.0, c(p.document_background));
    let fill = fill.unwrap_or(c(p.status_bar));
    let f = fraction.clamp(0.0, 1.0);
    let inner = bar.shrink(1.0);
    let done = Rect::from_min_max(inner.min, pos2((inner.left() + inner.width() * f).round(), inner.bottom()));
    if done.width() > 0.0 {
        style::vgradient(painter, done, fill, fill.gamma_multiply(0.8).to_opaque());
        hline(painter, done.left(), done.right(), done.top(), Color32::from_white_alpha(70));
    }
    style::bevel(painter, bar, shade(&p, p.panel_background, 1.45), c(p.shine));
}

// ---- ThumbnailPicker ----------------------------------------------------

/// One choice in a [`ThumbnailPicker`].
pub struct Thumbnail {
    /// Its picture (None: still loading, or none).
    pub texture: Option<egui::TextureHandle>,
    /// What fills the frame without a picture (a plain colour choice).
    pub color: Option<Color32>,
    pub label: String,
}

/// A wrapping grid of pictures to pick one from, labelled underneath (a
/// wallpaper, say): each in a sunken frame, the chosen one ringed in the
/// navigation colour. Arrow keys move the choice while it has focus.
pub struct ThumbnailPicker<'a> {
    id: Id,
    items: &'a [Thumbnail],
    selected: Option<usize>,
    size: Vec2,
}

impl<'a> ThumbnailPicker<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, items: &'a [Thumbnail], selected: Option<usize>) -> Self {
        ThumbnailPicker { id: Id::unique(("thumbnail-picker", id)), items, selected, size: vec2(128.0, 72.0) }
    }
    /// The pictures' size (logical pixels; default 128×72, 16:9).
    pub fn picture_size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }

    /// Draw it; the index chosen this frame, if any.
    pub fn show(self, ui: &mut Ui) -> Option<usize> {
        let p = style::palette(ui.ctx());
        let gap = m::SPACING;
        let cell = vec2(self.size.x + 4.0, self.size.y + 4.0 + m::TIGHT + m::LABEL_ROW);
        let cols = (((ui.available_width() + gap) / (cell.x + gap)).floor() as usize).max(1);
        let rows = self.items.len().div_ceil(cols);
        let height = rows as f32 * cell.y + rows.saturating_sub(1) as f32 * gap;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let mut chosen = None;
        let mut focused = false;
        for (i, item) in self.items.iter().enumerate() {
            let at = rect.min + vec2((i % cols) as f32 * (cell.x + gap), (i / cols) as f32 * (cell.y + gap));
            let r = Rect::from_min_size(at, cell);
            let resp = ui.interact(r, self.id.with(i), Sense::click());
            let on = self.selected == Some(i);
            resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, &item.label));
            focused |= resp.has_focus();
            if resp.clicked() {
                resp.request_focus();
                chosen = Some(i);
            }
            let painter = ui.painter();
            let frame = style::snap(ui.ctx(), Rect::from_min_size(at, self.size + vec2(4.0, 4.0)));
            let well = frame.shrink(2.0);
            painter.rect_filled(well, 0.0, c(p.document_background));
            match &item.texture {
                Some(t) => {
                    painter.image(t.id(), well, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    if let Some(fill) = item.color {
                        painter.rect_filled(well, 0.0, fill);
                    }
                }
            }
            style::bevel(painter, frame.shrink(1.0), shade(&p, p.panel_background, 1.45), c(p.shine));
            if on {
                frame_rect(painter, frame, c(p.keyboard_navigation));
                frame_rect(painter, frame.expand(1.0), c(p.keyboard_navigation));
            }
            let label_y = frame.bottom() + m::TIGHT + m::LABEL_ROW / 2.0;
            let font = if on { bold_font() } else { plain_font() };
            let clip = painter.with_clip_rect(Rect::from_min_max(pos2(r.left(), frame.bottom()), r.max));
            style::label(&clip, r.center().x, label_y, egui::Align::Center, item.label.clone(), font, c(p.panel_text));
        }
        // Arrows move the choice (as radio buttons do).
        if focused && let Some(sel) = self.selected {
            let step = ui.input(|i| {
                if i.key_pressed(Key::ArrowRight) {
                    1
                } else if i.key_pressed(Key::ArrowLeft) {
                    -1
                } else if i.key_pressed(Key::ArrowDown) {
                    cols as isize
                } else if i.key_pressed(Key::ArrowUp) {
                    -(cols as isize)
                } else {
                    0
                }
            });
            let next = sel as isize + step;
            if step != 0 && (0..self.items.len() as isize).contains(&next) {
                chosen = Some(next as usize);
                ui.memory_mut(|m| m.request_focus(self.id.with(next as usize)));
            }
        }
        chosen
    }
}

// ---- CellGrid -----------------------------------------------------------

/// One cell of a [`CellGrid`].
#[derive(Debug, Clone, Default)]
pub struct GridCell {
    /// Lit in this colour (from the palette); None: an empty well.
    pub color: Option<Color32>,
    /// A small mark drawn in it when there's room ("✦").
    pub mark: Option<String>,
    /// What it is, for the tooltip and agents.
    pub name: String,
    pub selected: bool,
}

/// What a [`CellGrid`] did this frame.
pub struct GridResponse {
    pub response: Response,
    /// The cell clicked, by index.
    pub clicked: Option<usize>,
}

/// A grid of small lit cells in sunken wells, as Pulse's LED meters on
/// BeOS: a heatmap of things going on (requests, jobs). Its height is
/// fixed (`rows` cells of `cell_size`); with more cells than fit, they
/// shrink and more rows and columns fit in the same space. Each cell is
/// clickable (and named for agents), with its name as a tooltip.
pub struct CellGrid<'a> {
    id: Id,
    cells: &'a [GridCell],
    rows: usize,
    cell: f32,
    gap: f32,
    slots: usize,
    fill: bool,
}

impl<'a> CellGrid<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, cells: &'a [GridCell]) -> Self {
        CellGrid { id: Id::unique(id), cells, rows: 1, cell: 12.0, gap: 2.0, slots: 0, fill: false }
    }
    /// Rows at full size (the height it takes).
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }
    pub fn cell_size(mut self, size: f32) -> Self {
        self.cell = size;
        self
    }
    /// Show at least this many wells, lit or not (a strip of the last 25).
    pub fn slots(mut self, n: usize) -> Self {
        self.slots = n;
        self
    }
    /// Wells across the whole area, however few cells are lit (a meter
    /// of what's going on now).
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// The cell size and columns that fit `n` cells in `width` × the
    /// grid's height.
    fn fit(&self, n: usize, width: f32) -> (f32, usize, usize) {
        let height = self.rows as f32 * self.cell + (self.rows as f32 - 1.0) * self.gap;
        let mut size = self.cell;
        loop {
            let cols = (((width + self.gap) / (size + self.gap)).floor() as usize).max(1);
            let rows = (((height + self.gap) / (size + self.gap)).floor() as usize).max(1);
            if cols * rows >= n || size <= 4.0 {
                return (size, cols, rows);
            }
            size -= 1.0;
        }
    }

    pub fn show(self, ui: &mut Ui) -> GridResponse {
        let p = style::palette(ui.ctx());
        let n = self.cells.len().max(self.slots);
        let width = ui.available_width();
        let height = self.rows as f32 * self.cell + (self.rows as f32 - 1.0) * self.gap;
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let (size, cols, rows) = self.fit(n, width);
        let mut clicked = None;
        let wells = if self.fill { cols * rows } else { n.min(cols * rows) };
        // Filling: spread the columns to both edges.
        let step_x = if self.fill && cols > 1 { (width - size) / (cols - 1) as f32 } else { size + self.gap };
        for i in 0..wells {
            let (row, col) = (i / cols, i % cols);
            let cell = style::snap(
                ui.ctx(),
                Rect::from_min_size(pos2(rect.left() + col as f32 * step_x, rect.top() + row as f32 * (size + self.gap)), vec2(size, size)),
            );
            let painter = ui.painter();
            // The well: sunken, as an unlit LED.
            painter.rect_filled(cell, 0.0, shade(&p, p.panel_background, 1.12));
            style::bevel(painter, cell, shade(&p, p.panel_background, 1.4), c(p.shine));
            let Some(item) = self.cells.get(i) else { continue };
            let resp = ui.interact(cell, self.id.with(i), Sense::click());
            let name = item.name.clone();
            resp.widget_info(|| WidgetInfo::selected(Role::Button, true, item.selected, &name));
            let inner = cell.shrink(1.0);
            if let Some(color) = item.color {
                let lit = if resp.hovered() { color.gamma_multiply(1.15) } else { color };
                style::vgradient(painter, inner, lit, lit.gamma_multiply(0.8).to_opaque());
                hline(painter, inner.left(), inner.right(), inner.top(), Color32::from_white_alpha(80));
            }
            if let Some(mark) = item.mark.as_deref().filter(|_| size >= 11.0) {
                // Centred on its capitals (not the line box, which sits
                // it high), and kept inside the well.
                let font = FontId::proportional((size * 0.64).round());
                let ink = if item.color.is_some() { Color32::WHITE } else { c(p.panel_text) };
                let g = layout(ui, mark, font, ink);
                let at = text_pos(ui, inner.center().x - g.size().x / 2.0, inner.center().y, &g);
                painter.with_clip_rect(inner).galley(at, g, ink);
            }
            if item.selected {
                style::frame_rect(painter, cell.expand(1.0), c(p.keyboard_navigation));
            }
            let resp = if item.name.is_empty() { resp } else { resp.on_hover_text(&item.name) };
            if resp.clicked() {
                clicked = Some(i);
            }
        }
        GridResponse { response, clicked }
    }
}

// ---- ScrollView ---------------------------------------------------------

/// BScrollView: a sunken frame around scrolling content with BeOS scroll
/// bars (arrow buttons at both ends, a raised knob with grip lines).
pub struct ScrollView {
    id: Id,
    vertical: bool,
    horizontal: bool,
    border: bool,
    focused: bool,
    background: Option<olo_theme::Rgba>,
    stick_to_bottom: bool,
    auto_hide: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Arrow {
    Up,
    Down,
    Left,
    Right,
}

impl Arrow {
    fn scroll_name(self) -> &'static str {
        match self {
            Arrow::Up => "Scroll Up",
            Arrow::Down => "Scroll Down",
            Arrow::Left => "Scroll Left",
            Arrow::Right => "Scroll Right",
        }
    }
}

impl ScrollView {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        ScrollView {
            id: Id::unique(id),
            vertical: true,
            horizontal: false,
            border: true,
            focused: false,
            background: None,
            stick_to_bottom: false,
            auto_hide: false,
        }
    }
    pub fn horizontal(mut self, on: bool) -> Self {
        self.horizontal = on;
        self
    }
    pub fn vertical(mut self, on: bool) -> Self {
        self.vertical = on;
        self
    }
    pub fn border(mut self, on: bool) -> Self {
        self.border = on;
        self
    }
    /// Draw the keyboard-focus frame (the target view has focus).
    pub fn focused(mut self, on: bool) -> Self {
        self.focused = on;
        self
    }
    pub fn background(mut self, rgba: olo_theme::Rgba) -> Self {
        self.background = Some(rgba);
        self
    }
    /// Show the vertical scroll bar only when the content overflows
    /// (multi-column browsers, where bars on every column would be heavy).
    pub fn auto_hide(mut self, on: bool) -> Self {
        self.auto_hide = on;
        self
    }
    pub fn stick_to_bottom(mut self, on: bool) -> Self {
        self.stick_to_bottom = on;
        self
    }

    /// Fill the available space with the scroll view.
    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
        let rect = ui.available_rect_before_wrap();
        self.show_in(ui, rect, |ui, _| add(ui))
    }

    /// Like [`show`](Self::show) for `total` rows of equal height; only
    /// visible rows are built.
    pub fn show_rows<R>(
        self,
        ui: &mut Ui,
        row_height: f32,
        total: usize,
        add: impl FnOnce(&mut Ui, std::ops::Range<usize>) -> R,
    ) -> R {
        let rect = ui.available_rect_before_wrap();
        let mut out = None;
        self.show_area(ui, rect, |area, ui| {
            let o = area.show_rows(ui, row_height, total, |ui, range| out = Some(add(ui, range)));
            (o.state, o.content_size, o.id)
        });
        out.unwrap()
    }

    /// Show in `rect`; `add` is given the visible part of the content.
    pub fn show_in<R>(self, ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui, Rect) -> R) -> R {
        let mut out = None;
        self.show_area(ui, rect, |area, ui| {
            let o = area.show_viewport(ui, |ui, vp| out = Some(add(ui, vp)));
            (o.state, o.content_size, o.id)
        });
        out.unwrap()
    }

    /// Ask the scroll view `id` to bring `top..bottom` (content
    /// coordinates) into view on its next frame.
    pub fn reveal(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, top: f32, bottom: f32) {
        ctx.data_mut(|d| d.insert_temp(Id::unique(id).with("reveal"), (top, bottom)));
        ctx.request_repaint();
    }

    /// Scroll the view `id` so content point `offset` is at its top-left,
    /// on its next frame.
    pub fn scroll_to(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, offset: Vec2) {
        ctx.data_mut(|d| d.insert_temp(Id::unique(id).with("pending"), offset));
        ctx.request_repaint();
    }

    fn show_area(
        self,
        ui: &mut Ui,
        rect: Rect,
        body: impl FnOnce(egui::ScrollArea, &mut Ui) -> (egui::scroll_area::State, Vec2, Id),
    ) {
        let p = style::palette(ui.ctx());
        let b = if self.border { 2.0 } else { 0.0 };
        let inner = rect.shrink(b);
        let mut content = inner;
        let size_id = self.id.with("content-size");
        let mut vertical_bar = self.vertical;
        if self.auto_hide {
            let last = ui.ctx().data(|d| d.get_temp::<f32>(size_id)).unwrap_or(0.0);
            vertical_bar &= last > inner.height() - if self.horizontal { SCROLL_BAR } else { 0.0 };
        }
        if vertical_bar {
            content.max.x -= SCROLL_BAR;
        }
        if self.horizontal {
            content.max.y -= SCROLL_BAR;
        }
        if let Some(bg) = self.background {
            ui.painter().rect_filled(content, 0.0, c(bg));
        }
        let pending_id = self.id.with("pending");
        let mut area = egui::ScrollArea::new([self.horizontal, self.vertical])
            .id_salt(self.id)
            .auto_shrink([false, false])
            .stick_to_bottom(self.stick_to_bottom)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        // Offsets our scroll bars set last frame (State has no setter).
        if let Some(off) = ui.ctx().data_mut(|d| {
            let v = d.get_temp::<Vec2>(pending_id);
            d.remove::<Vec2>(pending_id);
            v
        }) {
            area = area.scroll_offset(off);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
        child.set_clip_rect(content.intersect(ui.clip_rect()));
        let (state, full, _area_id) = body(area, &mut child);
        if self.auto_hide {
            ui.ctx().data_mut(|d| d.insert_temp(size_id, full.y));
            if (full.y > content.height()) != vertical_bar {
                ui.ctx().request_repaint();
            }
        }
        let mut offset = state.clamped_offset();
        ui.advance_cursor_after_rect(rect);

        let mut changed = false;
        if let Some((top, bottom)) = ui.ctx().data_mut(|d| d.remove_temp::<(f32, f32)>(self.id.with("reveal"))) {
            let view = content.height();
            if top < offset.y {
                offset.y = top;
                changed = true;
            } else if bottom > offset.y + view {
                offset.y = bottom - view;
                changed = true;
            }
        }
        // Something dragged near an edge: scroll on that way, faster the
        // nearer it is, so a drop can reach what is out of view.
        if let Some(h) = crate::dnd::hover(ui.ctx())
            && content.contains(h.pos)
        {
            const EDGE: f32 = 24.0;
            const SPEED: f32 = 900.0;
            let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
            let step = |into: f32| (1.0 - into / EDGE).clamp(0.0, 1.0) * SPEED * dt;
            let mut scrolled = false;
            for (vertical, len, view) in [(true, full.y, content.height()), (false, full.x, content.width())] {
                if (vertical && !self.vertical) || (!vertical && !self.horizontal) || len <= view {
                    continue;
                }
                let (pos, lo, hi, off) = if vertical { (h.pos.y, content.top(), content.bottom(), &mut offset.y) } else { (h.pos.x, content.left(), content.right(), &mut offset.x) };
                let before = *off;
                if pos < lo + EDGE {
                    *off -= step(pos - lo);
                } else if pos > hi - EDGE {
                    *off += step(hi - pos);
                }
                *off = off.clamp(0.0, len - view);
                scrolled |= *off != before;
            }
            if scrolled {
                changed = true;
            }
        }
        if vertical_bar {
            let bar = Rect::from_min_max(pos2(content.right(), inner.top()), pos2(inner.right(), content.bottom()));
            changed |= scroll_bar(ui, self.id.with("v"), bar, true, &mut offset.y, full.y, content.height(), &p);
        }
        if self.horizontal {
            let bar = Rect::from_min_max(pos2(inner.left(), content.bottom()), pos2(content.right(), inner.bottom()));
            changed |= scroll_bar(ui, self.id.with("h"), bar, false, &mut offset.x, full.x, content.width(), &p);
        }
        if vertical_bar && self.horizontal {
            let corner = Rect::from_min_max(content.max, inner.max);
            ui.painter().rect_filled(corner, 0.0, c(p.panel_background));
        }
        if changed {
            ui.ctx().data_mut(|d| d.insert_temp(pending_id, offset));
            ui.ctx().request_repaint();
        }
        if self.border {
            style::sunken(ui.painter(), rect, &p, self.focused);
        }
    }
}

/// A lone BeOS scroll bar, for content the kit doesn't scroll itself (a
/// web page). `offset` runs from 0 to `content - view`, in the content's
/// own units; with nothing to scroll the bar is drawn disabled.
pub struct ScrollBar {
    id: Id,
    vertical: bool,
}

impl ScrollBar {
    pub fn vertical(id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        ScrollBar { id: Id::unique(id), vertical: true }
    }
    pub fn horizontal(id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        ScrollBar { id: Id::unique(id), vertical: false }
    }
    /// Draw it in `rect` ([`SCROLL_BAR`] across); true if the user moved it.
    pub fn show(self, ui: &mut Ui, rect: Rect, offset: &mut f32, content: f32, view: f32) -> bool {
        let p = style::palette(ui.ctx());
        scroll_bar(ui, self.id, rect, self.vertical, offset, content, view, &p)
    }
}

/// Draw and drive one scroll bar. Returns whether `offset` changed.
#[allow(clippy::too_many_arguments)]
fn scroll_bar(ui: &mut Ui, id: Id, bar: Rect, vertical: bool, offset: &mut f32, content: f32, view: f32, p: &Palette) -> bool {
    let painter = ui.painter().clone();
    let along = |r: Rect| if vertical { r.height() } else { r.width() };
    let max_offset = (content - view).max(0.0);
    let enabled = max_offset > 0.5;
    let arrow_len = SCROLL_BAR;
    let (a0, a1, track) = if vertical {
        (
            Rect::from_min_size(bar.min, vec2(bar.width(), arrow_len)),
            Rect::from_min_size(pos2(bar.left(), bar.bottom() - arrow_len), vec2(bar.width(), arrow_len)),
            Rect::from_min_max(pos2(bar.left(), bar.top() + arrow_len), pos2(bar.right(), bar.bottom() - arrow_len)),
        )
    } else {
        (
            Rect::from_min_size(bar.min, vec2(arrow_len, bar.height())),
            Rect::from_min_size(pos2(bar.right() - arrow_len, bar.top()), vec2(arrow_len, bar.height())),
            Rect::from_min_max(pos2(bar.left() + arrow_len, bar.top()), pos2(bar.right() - arrow_len, bar.bottom())),
        )
    };
    let bg = p.panel_background;
    // Track: recessed, a shade darker than the panel.
    painter.rect_filled(bar, 0.0, shade(p, bg, 1.1));
    if vertical {
        vline(&painter, bar.left(), bar.top(), bar.bottom(), shade(p, bg, 1.35));
    } else {
        hline(&painter, bar.left(), bar.right(), bar.top(), shade(p, bg, 1.35));
    }
    let start = *offset;
    let line = 16.0;
    let mut press_arrow = |r: Rect, dir: Arrow, delta: f32, painter: &Painter| {
        let resp = ui.interact(r, id.with(dir as u8), Sense::click_and_drag());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, dir.scroll_name()));
        let down = resp.is_pointer_button_down_on() && enabled;
        if down {
            // Auto-repeat while held: first step immediately, then smooth.
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            if resp.drag_started() || resp.clicked() {
                *offset += delta;
            } else {
                *offset += delta * dt * 20.0;
            }
            ui.ctx().request_repaint();
        }
        draw_arrow_button(painter, r, dir, down, enabled, p);
    };
    let (d0, d1) = if vertical { (Arrow::Up, Arrow::Down) } else { (Arrow::Left, Arrow::Right) };
    press_arrow(a0, d0, -line, &painter);
    press_arrow(a1, d1, line, &painter);

    // Knob.
    let track_len = along(track);
    if enabled && track_len > 8.0 {
        let knob_len = (track_len * view / content).clamp(SCROLL_BAR + 4.0, track_len);
        let travel = track_len - knob_len;
        let pos = (*offset / max_offset).clamp(0.0, 1.0) * travel;
        let knob = if vertical {
            Rect::from_min_size(pos2(track.left(), track.top() + pos), vec2(track.width(), knob_len))
        } else {
            Rect::from_min_size(pos2(track.left() + pos, track.top()), vec2(knob_len, track.height()))
        }
        .round_ui();
        let track_resp = ui.interact(track, id.with("track"), Sense::click_and_drag());
        let knob_resp = ui.interact(knob, id.with("knob"), Sense::click_and_drag());
        // For agents: where it is, 0 (top or left) to 1.
        let at = (*offset / max_offset).clamp(0.0, 1.0) as f64;
        track_resp.widget_info(|| WidgetInfo { value: Some(at), label: Some("Scroll Bar".into()), ..WidgetInfo::new(Role::ScrollBar) });
        knob_resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Scroll Knob"));
        if knob_resp.dragged() {
            let d = knob_resp.drag_delta();
            let d = if vertical { d.y } else { d.x };
            if travel > 0.0 {
                *offset += d * max_offset / travel;
            }
        } else if track_resp.clicked()
            && let Some(at) = track_resp.interact_pointer_pos()
        {
            // Page towards the click.
            let before = if vertical { at.y < knob.top() } else { at.x < knob.left() };
            *offset += if before { -view } else { view } * 0.9;
        }
        let base = p.control_background;
        let pressed = knob_resp.is_pointer_button_down_on();
        let (t0, t1) = if pressed {
            (shade(p, base, 1.12), shade(p, base, 1.05))
        } else {
            (c(olo_theme::tint(base, 0.85)), shade(p, base, 1.04))
        };
        let k = knob;
        if vertical {
            // Shade across the knob, as light falls on a vertical bar.
            hgradient(&painter, k.shrink(1.0), t0, t1);
        } else {
            style::vgradient(&painter, k.shrink(1.0), t0, t1);
        }
        frame_rect(&painter, k, shade(p, bg, 1.55));
        style::bevel(&painter, k.shrink(1.0), c(p.shine), shade(p, base, 1.18));
        // Three grip lines in the middle.
        if knob_len > 20.0 {
            let ctr = k.center().round();
            for i in -1..=1 {
                let o = i as f32 * 3.0;
                if vertical {
                    hline(&painter, k.left() + 4.0, k.right() - 4.0, ctr.y + o - 1.0, shade(p, base, 1.3));
                    hline(&painter, k.left() + 4.0, k.right() - 4.0, ctr.y + o, c(p.shine));
                } else {
                    vline(&painter, ctr.x + o - 1.0, k.top() + 4.0, k.bottom() - 4.0, shade(p, base, 1.3));
                    vline(&painter, ctr.x + o, k.top() + 4.0, k.bottom() - 4.0, c(p.shine));
                }
            }
        }
    }
    *offset = offset.clamp(0.0, max_offset);
    *offset != start
}

fn hgradient(p: &Painter, r: Rect, left: Color32, right: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), left);
    mesh.colored_vertex(r.right_top(), right);
    mesh.colored_vertex(r.left_bottom(), left);
    mesh.colored_vertex(r.right_bottom(), right);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(mesh);
}

fn draw_arrow_button(painter: &Painter, r: Rect, dir: Arrow, down: bool, enabled: bool, p: &Palette) {
    let base = p.control_background;
    let bg = p.panel_background;
    let fill = if down { shade(p, base, 1.15) } else { c(olo_theme::tint(base, 0.9)) };
    painter.rect_filled(r.shrink(1.0), 0.0, fill);
    frame_rect(painter, r, shade(p, bg, 1.55));
    if down {
        style::bevel(painter, r.shrink(1.0), shade(p, base, 1.3), shade(p, base, 1.05));
    } else {
        style::bevel(painter, r.shrink(1.0), c(p.shine), shade(p, base, 1.18));
    }
    let color = if enabled { c(p.control_text) } else { shade(p, bg, 1.3) };
    let ctr = r.center() + if down { vec2(0.5, 0.5) } else { Vec2::ZERO };
    let (s, h) = (3.5, 2.0);
    let pts = match dir {
        Arrow::Up => vec![pos2(ctr.x - s, ctr.y + h), pos2(ctr.x + s, ctr.y + h), pos2(ctr.x, ctr.y - h)],
        Arrow::Down => vec![pos2(ctr.x - s, ctr.y - h), pos2(ctr.x + s, ctr.y - h), pos2(ctr.x, ctr.y + h)],
        Arrow::Left => vec![pos2(ctr.x + h, ctr.y - s), pos2(ctr.x + h, ctr.y + s), pos2(ctr.x - h, ctr.y)],
        Arrow::Right => vec![pos2(ctr.x - h, ctr.y - s), pos2(ctr.x - h, ctr.y + s), pos2(ctr.x + h, ctr.y)],
    };
    painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

// ---- ColumnListView -------------------------------------------------------

/// A column in a [`ColumnListView`].
#[derive(Debug, Clone)]
pub struct Column {
    pub title: String,
    pub width: f32,
    pub align: Align,
    pub min_width: f32,
}

impl Column {
    pub fn new(title: impl Into<String>, width: f32) -> Self {
        Column { title: title.into(), width, align: Align::Min, min_width: 24.0 }
    }
    pub fn right(mut self) -> Self {
        self.align = Align::Max;
        self
    }
}

/// Selection and sort state the app keeps for a list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListState {
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    /// Sort column and ascending flag.
    pub sort: Option<(usize, bool)>,
    /// Whether more than one row may be selected.
    pub multiple: bool,
}

impl ListState {
    pub fn single(&self) -> Option<usize> {
        self.selected.iter().next().copied()
    }
    pub fn select(&mut self, row: usize) {
        self.selected.clear();
        self.selected.insert(row);
        self.anchor = Some(row);
    }
}

#[derive(Debug, Default)]
pub struct ListResponse {
    /// Row double-clicked or Enter pressed on.
    pub activated: Option<usize>,
    /// Row right-clicked, with the pointer position.
    pub context: Option<(usize, Pos2)>,
    pub selection_changed: bool,
    pub sort_changed: bool,
    /// With [`ColumnListView::draggable`]: a drag started on this row
    /// (now selected, if it wasn't), and whether with the secondary
    /// button. Start the real drag with [`crate::Cx::start_drag`].
    pub drag_started: Option<(usize, bool)>,
    /// The rows on screen and where, clipped to the list (to find the
    /// row a drop is over).
    pub visible: Vec<(usize, Rect)>,
}

impl ListResponse {
    /// The row on screen at `pos`.
    pub fn row_at(&self, pos: Pos2) -> Option<usize> {
        self.visible.iter().find(|(_, r)| r.contains(pos)).map(|(row, _)| *row)
    }
}

/// What a cell callback is given.
pub struct Cell<'a> {
    pub painter: &'a Painter,
    pub rect: Rect,
    pub row: usize,
    pub column: usize,
    pub selected: bool,
    pub text_color: Color32,
    pub align: Align,
}

impl Cell<'_> {
    /// Draw `text`, ellipsized to the cell.
    pub fn text(&self, text: &str) {
        self.text_with(text, plain_font(), self.text_color);
    }
    pub fn text_with(&self, text: &str, font: FontId, color: Color32) {
        self.text_job(egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color));
    }
    /// Draw text laid out in sections of their own colours and fonts (a
    /// highlighted line), on one line, ellipsized to the cell.
    pub fn text_job(&self, mut job: egui::text::LayoutJob) {
        let r = self.rect.shrink2(vec2(m::CELL_INSET, 0.0));
        job.wrap = egui::text::TextWrapping::truncate_at_width(r.width().max(0.0));
        job.break_on_newline = false;
        let g = self.painter.layout_job(job);
        let x = match self.align {
            Align::Max => r.right() - g.size().x,
            Align::Center => r.center().x - g.size().x / 2.0,
            Align::Min => r.left(),
        };
        let size = g.job.sections.first().map_or(style::PLAIN_SIZE, |s| s.format.font_id.size);
        let pos = style::text_origin(self.painter.ctx(), &g, x, r.center().y, size);
        style::describe_text(self.painter.ctx(), egui::accesskit::Role::Cell, g.text(), Rect::from_min_size(pos, g.size()).intersect(r).intersect(self.painter.clip_rect()), Some(self.selected));
        self.painter.galley(pos, g, self.text_color);
    }
}

/// BColumnListView: resizable, sortable column headers over a virtualised
/// list with BeOS selection, keyboard navigation and context clicks.
pub struct ColumnListView<'a> {
    id: Id,
    columns: &'a [Column],
    rows: usize,
    state: &'a mut ListState,
    row_height: f32,
    reveal: Option<usize>,
    draggable: bool,
    empty: Option<&'a str>,
}

pub const HEADER_HEIGHT: f32 = 18.0;

impl<'a> ColumnListView<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, columns: &'a [Column], rows: usize, state: &'a mut ListState) -> Self {
        ColumnListView { id: Id::unique(id), columns, rows, state, row_height: m::LIST_ROW, reveal: None, draggable: false, empty: None }
    }
    pub fn row_height(mut self, h: f32) -> Self {
        self.row_height = h;
        self
    }
    /// Scroll so `row` shows, with a few rows of context around it (to
    /// bring a selection made in code into view).
    pub fn reveal(mut self, row: Option<usize>) -> Self {
        self.reveal = row;
        self
    }
    /// Say `text` in the middle of the list while it has no rows ("No
    /// lines match.").
    pub fn empty_text(mut self, text: &'a str) -> Self {
        self.empty = Some(text);
        self
    }
    /// Rows can be dragged (see [`ListResponse::drag_started`]).
    pub fn draggable(mut self) -> Self {
        self.draggable = true;
        self
    }

    pub fn show(self, ui: &mut Ui, mut cell: impl FnMut(&Cell)) -> ListResponse {
        let p = style::palette(ui.ctx());
        let mut out = ListResponse::default();
        let outer = ui.available_rect_before_wrap();
        let widths_id = self.id.with("widths");
        let mut widths: Vec<f32> = ui
            .ctx()
            .data(|d| d.get_temp::<Vec<f32>>(widths_id))
            .filter(|w| w.len() == self.columns.len())
            .unwrap_or_else(|| self.columns.iter().map(|c| c.width).collect());

        // Header.
        let inner = outer.shrink(2.0);
        let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT));
        let painter = ui.painter().clone();
        let bg = p.panel_background;
        style::vgradient(&painter, header, c(olo_theme::tint(bg, 0.8)), shade(&p, bg, 1.04));
        hline(&painter, header.left(), header.right(), header.bottom() - 1.0, shade(&p, bg, 1.45));
        let mut x = header.left();
        let last = self.columns.len().saturating_sub(1);
        for (i, col) in self.columns.iter().enumerate() {
            let w = if i == last { (header.right() - x - SCROLL_BAR).max(0.0) } else { widths[i] };
            let r = Rect::from_min_size(pos2(x, header.top()), vec2(w, HEADER_HEIGHT - 1.0));
            let resp = ui.interact(r, self.id.with(("hdr", i)), Sense::click());
            let sorted = self.state.sort.filter(|(c, _)| *c == i);
            resp.widget_info(|| WidgetInfo { selected: Some(sorted.is_some()), ..WidgetInfo::labeled(Role::ColumnHeader, true, col.title.as_str()) });
            if resp.clicked() {
                self.state.sort = Some((i, sorted.is_none_or(|(_, asc)| !asc)));
                out.sort_changed = true;
            }
            if resp.is_pointer_button_down_on() {
                painter.rect_filled(r, 0.0, shade(&p, bg, 1.1));
            }
            // Light leading edge, dark trailing edge.
            vline(&painter, r.left(), r.top(), r.bottom(), c(p.shine));
            vline(&painter, r.right() - 1.0, r.top() + 2.0, r.bottom() - 2.0, shade(&p, bg, 1.3));
            let font = if sorted.is_some() { bold_font() } else { plain_font() };
            let hc = Cell {
                painter: &painter,
                rect: r.with_max_x(r.right() - if sorted.is_some() { 12.0 } else { 0.0 }),
                row: 0,
                column: i,
                selected: false,
                text_color: c(p.panel_text),
                align: col.align,
            };
            hc.text_with(&col.title, font, c(p.panel_text));
            if let Some((_, asc)) = sorted {
                let ax = r.right() - 10.0;
                let ay = r.center().y.round();
                let pts = if asc {
                    vec![pos2(ax - 3.5, ay + 2.0), pos2(ax + 3.5, ay + 2.0), pos2(ax, ay - 2.0)]
                } else {
                    vec![pos2(ax - 3.5, ay - 2.0), pos2(ax + 3.5, ay - 2.0), pos2(ax, ay + 2.0)]
                };
                painter.add(Shape::convex_polygon(pts, c(p.panel_text), Stroke::NONE));
            }
            // Resize handle on the divider.
            if i != last {
                let handle = Rect::from_center_size(pos2(r.right(), r.center().y), vec2(6.0, HEADER_HEIGHT));
                let h = ui.interact(handle, self.id.with(("resize", i)), Sense::drag());
                if h.hovered() || h.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn);
                }
                if h.dragged() {
                    widths[i] = (widths[i] + h.drag_delta().x).max(col.min_width);
                }
            }
            x += w;
        }
        ui.ctx().data_mut(|d| d.insert_temp(widths_id, widths.clone()));

        // Above the scroll bar: a blank header cell.
        let filler = Rect::from_min_max(pos2(header.right() - SCROLL_BAR, header.top()), pos2(header.right(), header.bottom() - 1.0));
        vline(&painter, filler.left(), filler.top(), filler.bottom(), c(p.shine));

        // Rows.
        let body = Rect::from_min_max(pos2(inner.left(), header.bottom()), inner.max);
        let focus_id = self.id.with("focus");
        let list_resp = ui.interact(body, focus_id, Sense::focusable_noninteractive());
        let focused = list_resp.has_focus();
        let row_h = self.row_height;
        let rows = self.rows;
        let state = &mut *self.state;
        let columns = self.columns;
        let id = self.id;
        let draggable = self.draggable;
        let visible = &mut out.visible;
        let drag_started = &mut out.drag_started;
        let mut selection_changed = false;
        let mut activated = None;
        let mut context = None;
        // A selection band: where it started (y in content coordinates)
        // and what was selected before it (⇧ or ⌘ adds to that).
        let band_id = id.with("band");
        let mut band = ui.ctx().data(|d| d.get_temp::<(Pos2, BTreeSet<usize>)>(band_id));
        let mut band_from = None;
        let mut content_top = body.top();
        if let Some(row) = self.reveal {
            let context = ((body.height() / row_h / 2.0).floor() - 1.0).max(0.0);
            let (top, bottom) = ((row as f32 - context).max(0.0) * row_h, (row as f32 + 1.0 + context) * row_h);
            ScrollView::reveal(ui.ctx(), id.with("scroll"), top, bottom);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
        child.spacing_mut().item_spacing.y = 0.0;
        ScrollView::new(id.with("scroll")).border(false).background(p.list_background).show_rows(
            &mut child,
            row_h,
            rows,
            |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let left = ui.max_rect().left();
                let right = ui.max_rect().right();
                content_top = ui.max_rect().top() - range.start as f32 * row_h;
                let sense = if draggable || state.multiple { Sense::click_and_drag() } else { Sense::click() };
                for row in range {
                    let (r, resp) = ui.allocate_exact_size(vec2(right - left, row_h), sense);
                    let r = Rect::from_min_max(pos2(left, r.top()), pos2(right, r.bottom()));
                    visible.push((row, r.intersect(ui.clip_rect())));
                    if resp.drag_started() {
                        ui.memory_mut(|m| m.request_focus(focus_id));
                        // As in Tracker's list view: a selected row, or the
                        // first (name) column, drags the items; the rest of
                        // a row draws a selection band.
                        let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(r.min);
                        let on_item = state.selected.contains(&row) || columns.len() == 1 || press.x < left + widths[0];
                        if draggable && on_item {
                            if !state.selected.contains(&row) {
                                state.select(row);
                                selection_changed = true;
                            }
                            *drag_started = Some((row, resp.dragged_by(egui::PointerButton::Secondary)));
                        } else if state.multiple && resp.dragged_by(egui::PointerButton::Primary) {
                            band_from = Some(press);
                        }
                    }
                    if resp.clicked() || resp.secondary_clicked() || resp.double_clicked() {
                        ui.memory_mut(|m| m.request_focus(focus_id));
                        let mods = ui.input(|i| i.modifiers);
                        if resp.secondary_clicked() && state.selected.contains(&row) {
                            // Keep a multi-selection for the context menu.
                        } else if state.multiple && mods.command {
                            if !state.selected.remove(&row) {
                                state.selected.insert(row);
                            }
                            state.anchor = Some(row);
                        } else if state.multiple && mods.shift {
                            let a = state.anchor.unwrap_or(row);
                            state.selected = (a.min(row)..=a.max(row)).collect();
                        } else {
                            state.select(row);
                        }
                        selection_changed = true;
                    }
                    if resp.double_clicked() {
                        activated = Some(row);
                    }
                    if resp.secondary_clicked() {
                        context = resp.interact_pointer_pos().map(|p| (row, p));
                    }
                    let selected = state.selected.contains(&row);
                    // For agents: a row, named by its cells' text, where it
                    // shows (a row scrolled half under the header is only
                    // its visible half).
                    resp.widget_info(|| WidgetInfo { selected: Some(selected), ..WidgetInfo::new(Role::Row) });
                    let shown = r.intersect(ui.clip_rect());
                    let shown = if shown.is_positive() { shown } else { Rect::from_min_size(r.min, Vec2::ZERO) };
                    ui.ctx().accesskit_node_builder(resp.id, |n| {
                        n.set_bounds(egui::accesskit::Rect { x0: shown.min.x as f64, y0: shown.min.y as f64, x1: shown.max.x as f64, y1: shown.max.y as f64 });
                        // Which row of how many, so an agent knows how far
                        // it has to scroll.
                        n.set_row_index(row);
                        n.set_row_count(rows);
                    });
                    let painter = ui.painter();
                    if selected {
                        let sel = if focused {
                            c(p.list_selected_background)
                        } else {
                            c(olo_theme::mix(p.list_selected_background, p.list_background, 0.35))
                        };
                        painter.rect_filled(r, 0.0, sel);
                    }
                    let text_color = if selected { c(p.list_selected_item_text) } else { c(p.list_item_text) };
                    let mut x = left;
                    for (i, col) in columns.iter().enumerate() {
                        let w = if i == columns.len() - 1 { (right - x).max(0.0) } else { widths[i] };
                        let cr = Rect::from_min_size(pos2(x, r.top()), vec2(w, row_h));
                        let painter = painter.with_clip_rect(cr.intersect(painter.clip_rect()));
                        cell(&Cell { painter: &painter, rect: cr, row, column: i, selected, text_color, align: col.align });
                        x += w;
                    }
                }
                // Below the last row: a click clears the selection, a drag
                // draws a band.
                let rest = Rect::from_min_max(pos2(left, content_top + rows as f32 * row_h), pos2(right, ui.clip_rect().bottom()));
                if rest.height() > 0.0 {
                    let resp = ui.interact(rest, id.with("rest"), sense);
                    if resp.clicked() {
                        ui.memory_mut(|m| m.request_focus(focus_id));
                        let mods = ui.input(|i| i.modifiers);
                        if !(mods.command || mods.shift) && !state.selected.is_empty() {
                            state.selected.clear();
                            state.anchor = None;
                            selection_changed = true;
                        }
                    }
                    if resp.drag_started() && state.multiple && resp.dragged_by(egui::PointerButton::Primary) {
                        ui.memory_mut(|m| m.request_focus(focus_id));
                        band_from = ui.input(|i| i.pointer.press_origin());
                    }
                }
            },
        );
        if rows == 0
            && let Some(text) = self.empty
        {
            let dim = c(olo_theme::mix(p.list_item_text, p.list_background, 0.5));
            style::label(&child.painter().with_clip_rect(body), body.center().x, body.center().y, Align::Center, text, plain_font(), dim);
        }
        if let Some(from) = band_from {
            let mods = ui.input(|i| i.modifiers);
            let base = if mods.command || mods.shift { state.selected.clone() } else { BTreeSet::new() };
            band = Some((pos2(from.x, from.y - content_top), base));
        }
        let (down, pointer) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
        match (&band, down, pointer) {
            (Some((from, base)), true, Some(now)) => {
                let (top, bottom) = (from.y.min(now.y - content_top), from.y.max(now.y - content_top));
                let first = (top / row_h).floor().max(0.0) as usize;
                let end = ((bottom / row_h).ceil().max(0.0) as usize).min(rows);
                let mut selected = base.clone();
                selected.extend(first..end.max(first));
                if selected != state.selected {
                    state.anchor = (first < end).then_some(first).or(state.anchor);
                    state.selected = selected;
                    selection_changed = true;
                }
                let r = Rect::from_two_pos(pos2(from.x, from.y + content_top), now).intersect(body);
                style::selection_band(&ui.painter().with_clip_rect(body), r, &p);
                // Near or past the top or bottom: scroll on.
                let edge = row_h / 2.0;
                let step = if now.y > body.bottom() - edge {
                    ((now.y - body.bottom() + edge) / 2.0).clamp(2.0, row_h * 2.0)
                } else if now.y < body.top() + edge {
                    -((body.top() + edge - now.y) / 2.0).clamp(2.0, row_h * 2.0)
                } else {
                    0.0
                };
                if step != 0.0 {
                    let (vt, vb) = (body.top() - content_top, body.bottom() - content_top);
                    let (t, b) = if step > 0.0 { (vb + step - 1.0, vb + step) } else { (vt + step, vt + step + 1.0) };
                    ScrollView::reveal(ui.ctx(), id.with("scroll"), t.max(0.0), b.min(rows as f32 * row_h).max(1.0));
                }
                let kept = (*from, base.clone());
                ui.ctx().data_mut(|d| d.insert_temp(band_id, kept));
            }
            _ => ui.ctx().data_mut(|d| d.remove::<(Pos2, BTreeSet<usize>)>(band_id)),
        }
        out.selection_changed |= selection_changed;
        out.activated = out.activated.or(activated);
        out.context = out.context.or(context);
        style::sunken(ui.painter(), outer, &p, focused);
        ui.advance_cursor_after_rect(outer);

        // Keyboard navigation.
        if focused && rows > 0 {
            let page = ((body.height() - 4.0) / row_h).floor().max(1.0) as isize;
            let cur = state.anchor.map(|a| a as isize);
            let mut next = None;
            ui.input(|i| {
                if i.key_pressed(Key::ArrowDown) {
                    next = Some(cur.map_or(0, |c| c + 1));
                }
                if i.key_pressed(Key::ArrowUp) {
                    next = Some(cur.map_or(0, |c| c - 1));
                }
                if i.key_pressed(Key::PageDown) {
                    next = Some(cur.map_or(0, |c| c + page));
                }
                if i.key_pressed(Key::PageUp) {
                    next = Some(cur.map_or(0, |c| c - page));
                }
                if i.key_pressed(Key::Home) {
                    next = Some(0);
                }
                if i.key_pressed(Key::End) {
                    next = Some(rows as isize - 1);
                }
                if i.key_pressed(Key::Enter) {
                    out.activated = state.anchor;
                }
            });
            if let Some(n) = next {
                let n = n.clamp(0, rows as isize - 1) as usize;
                state.select(n);
                out.selection_changed = true;
                ScrollView::reveal(ui.ctx(), id.with("scroll"), n as f32 * row_h, (n + 1) as f32 * row_h);
            }
        }
        out
    }
}

// ---- TabView ------------------------------------------------------------

/// BTabView: BeOS folder tabs over a raised content panel.
pub struct TabView<'a> {
    selected: &'a mut usize,
    tabs: &'a [&'a str],
}

pub const TAB_HEIGHT: f32 = 22.0;

impl<'a> TabView<'a> {
    pub fn new(selected: &'a mut usize, tabs: &'a [&'a str]) -> Self {
        TabView { selected, tabs }
    }

    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui, usize) -> R) -> R {
        let p = style::palette(ui.ctx());
        let outer = ui.available_rect_before_wrap();
        let painter = ui.painter().clone();
        let bg = p.panel_background;
        let panel = Rect::from_min_max(pos2(outer.left(), outer.top() + TAB_HEIGHT - 1.0), outer.max);
        // Content panel: raised.
        style::bevel(&painter, panel, c(p.shine), shade(&p, bg, 1.35));
        vline(&painter, panel.right() - 2.0, panel.top() + 1.0, panel.bottom() - 1.0, shade(&p, bg, 1.12));
        hline(&painter, panel.left() + 1.0, panel.right() - 1.0, panel.bottom() - 2.0, shade(&p, bg, 1.12));

        let slant = 6.0;
        let mut x = outer.left() + 6.0;
        let mut rects = Vec::new();
        for (i, label) in self.tabs.iter().enumerate() {
            let font = if i == *self.selected { bold_font() } else { plain_font() };
            let g = layout(ui, label, bold_font(), c(p.panel_text));
            let w = (g.size().x + 2.0 * slant + 18.0).round();
            let r = Rect::from_min_size(pos2(x, outer.top()), vec2(w, TAB_HEIGHT));
            let resp = ui.interact(r, ui.make_persistent_id(("tab", i)), Sense::click());
            if resp.clicked() {
                *self.selected = i;
            }
            resp.widget_info(|| WidgetInfo { selected: Some(i == *self.selected), ..WidgetInfo::labeled(Role::Tab, true, *label) });
            rects.push((r, font, *label));
            x += w - slant + 1.0;
        }
        // Unselected tabs first, right to left so left ones overlap.
        let draw_tab = |r: Rect, selected: bool, label: &str, font: FontId| {
            let top = if selected { r.top() } else { r.top() + 2.0 };
            let bottom = if selected { r.bottom() } else { r.bottom() - 1.0 };
            let pts = [
                pos2(r.left(), bottom),
                pos2(r.left() + slant, top),
                pos2(r.right() - slant, top),
                pos2(r.right(), bottom),
            ];
            let fill = if selected { c(bg) } else { shade(&p, bg, 1.07) };
            painter.add(Shape::convex_polygon(pts.to_vec(), fill, Stroke::NONE));
            let dark = shade(&p, bg, 1.4);
            painter.line_segment([pts[0] + vec2(0.5, 0.0), pts[1] + vec2(0.5, 0.5)], Stroke::new(1.0, c(p.shine)));
            hline(&painter, pts[1].x, pts[2].x, top, c(p.shine));
            painter.line_segment([pts[2] + vec2(-0.5, 0.5), pts[3] + vec2(-0.5, 0.0)], Stroke::new(1.0, dark));
            if selected {
                // Open into the panel.
                hline(&painter, pts[0].x + 1.0, pts[3].x - 1.0, panel.top(), c(bg));
            }
            style::label(&painter, r.center().x, (top + bottom) / 2.0, Align::Center, label, font, c(p.panel_text));
        };
        for (i, (r, font, label)) in rects.iter().enumerate().rev() {
            if i != *self.selected {
                draw_tab(*r, false, label, font.clone());
            }
        }
        if let Some((r, font, label)) = rects.get(*self.selected) {
            draw_tab(*r, true, label, font.clone());
        }

        let content = panel.shrink(m::WINDOW_INSET);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
        let r = add(&mut child, *self.selected);
        ui.advance_cursor_after_rect(outer);
        r
    }
}

// ---- TabStrip -----------------------------------------------------------

/// Paints an icon into its rectangle.
pub type PaintIcon<'a> = &'a dyn Fn(&Painter, Rect);

/// One tab in a [`TabStrip`].
pub struct StripTab<'a> {
    pub label: &'a str,
    /// Paints the 16×16 icon left of the label.
    pub icon: Option<PaintIcon<'a>>,
    /// A band along the tab's top edge: the colour of its group.
    pub color: Option<Color32>,
    /// What the tab is doing, for screen readers and agents ("waiting
    /// for an answer"); the icon shows it.
    pub status: Option<&'a str>,
}

/// What the person did to a [`TabStrip`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StripAction {
    Select(usize),
    Close(usize),
    /// The + after the last tab.
    New,
    /// A right-click on a tab, at the pointer.
    Menu(usize, Pos2),
    /// A tab dragged along the row to another place.
    Move { from: usize, to: usize },
    /// A click on the group's name, at the pointer.
    Group(Pos2),
}

/// Widest a [`TabStrip`] tab grows.
pub const STRIP_TAB_MAX: f32 = 220.0;

/// How tall a [`TabStrip`] is: roomier than a window tab, as a row of
/// documents is something you aim at all day.
pub const STRIP_HEIGHT: f32 = 30.0;

/// The group band along a [`TabStrip`] tab's top edge.
const STRIP_BAND: f32 = 3.0;

/// A row of document tabs under a window's tool or menu bar
/// (Terminal's shells): the selected one open into the content below,
/// each with a close box (middle-click closes too), a plus button after
/// the last, and tabs dragged along the row to reorder them.
/// Each tab is a `Tab` in the accessibility tree, its status in its name.
pub struct TabStrip<'a> {
    id: Id,
    tabs: &'a [StripTab<'a>],
    active: usize,
    max_width: f32,
    new_tab: bool,
    group: Option<(&'a str, Color32)>,
}

impl<'a> TabStrip<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, tabs: &'a [StripTab<'a>], active: usize) -> Self {
        TabStrip { id: Id::unique(id), tabs, active, max_width: STRIP_TAB_MAX, new_tab: true, group: None }
    }

    /// Without the + button.
    pub fn no_new_tab(mut self) -> Self {
        self.new_tab = false;
        self
    }

    /// The tabs are a named group: its name leads the row in its colour
    /// (clicking it gives [`StripAction::Group`]).
    pub fn group(mut self, name: &'a str, color: Color32) -> Self {
        self.group = Some((name, color));
        self
    }

    /// `full` is [`STRIP_HEIGHT`] tall and the window's width.
    pub fn show(self, ui: &mut Ui, full: Rect) -> Option<StripAction> {
        let p = style::palette(ui.ctx());
        let painter = ui.painter_at(full);
        let bg = p.panel_background;
        painter.rect_filled(full, 0.0, shade(&p, bg, 1.07));
        hline(&painter, full.left(), full.right(), full.bottom() - 1.0, shade(&p, bg, 1.3));
        let mut action = None;
        let mut strip = full;
        if let Some((name, color)) = self.group {
            let text_w = style::label_width(ui.ctx(), name, bold_font());
            let chip_h = m::CONTROL_HEIGHT - 2.0;
            let chip = Rect::from_min_size(pos2(full.left() + m::SPACING, (full.center().y - chip_h / 2.0).round()), vec2((text_w + 2.0 * m::SPACING).round(), chip_h));
            let chip = style::snap(ui.ctx(), chip);
            let resp = ui.interact(chip, self.id.with("group"), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Group {name}")));
            painter.rect_filled(chip, 4.0, color);
            let [r, g, b, _] = color.to_array();
            let light = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 150.0;
            let ink = if light { Color32::from_gray(20) } else { Color32::WHITE };
            style::label(&painter, chip.center().x, chip.center().y, Align::Center, name, bold_font(), ink);
            if resp.clicked() || resp.secondary_clicked() {
                action = resp.interact_pointer_pos().map(StripAction::Group);
            }
            strip.min.x = chip.right() + m::SPACING - m::TIGHT;
        }
        let n = self.tabs.len();
        let new_w = if self.new_tab { m::CONTROL_HEIGHT + m::TIGHT } else { 0.0 };
        let width = ((strip.width() - 2.0 * m::TIGHT - new_w) / n.max(1) as f32).min(self.max_width).floor().max(1.0);
        let slot = |i: usize| Rect::from_min_size(pos2(strip.left() + m::TIGHT + i as f32 * width, strip.top() + 4.0), vec2(width, STRIP_HEIGHT - 4.0));
        let drag_id = self.id.with("dragging");
        for (i, tab) in self.tabs.iter().enumerate() {
            let r = slot(i);
            let selected = i == self.active;
            let resp = ui.interact(r, self.id.with(("tab", i)), Sense::click_and_drag());
            let name = match tab.status {
                Some(s) => format!("{}, {s}", tab.label),
                None => tab.label.to_owned(),
            };
            resp.widget_info(|| WidgetInfo { selected: Some(selected), ..WidgetInfo::labeled(Role::Tab, true, &name) });
            if selected {
                painter.rect_filled(r, 0.0, c(bg));
                hline(&painter, r.left(), r.right(), r.top(), c(p.shine));
                vline(&painter, r.left(), r.top(), r.bottom(), c(p.shine));
                vline(&painter, r.right() - 1.0, r.top(), r.bottom(), shade(&p, bg, 1.4));
            } else {
                vline(&painter, r.right() - 1.0, r.top() + 3.0, r.bottom() - 3.0, shade(&p, bg, 1.3));
            }
            // The group's band: inside the tab's edges, above what's in it.
            let mut top = r.top();
            if let Some(color) = tab.color {
                let band = Rect::from_min_max(pos2(r.left() + 1.0, r.top() + 1.0), pos2(r.right() - 2.0, r.top() + 1.0 + STRIP_BAND));
                painter.rect_filled(band, 0.0, color);
                top = band.bottom();
            }
            let cy = ((top + r.bottom()) / 2.0).round();
            let close = Rect::from_center_size(pos2(r.right() - m::SPACING - 4.0, cy), vec2(12.0, 12.0));
            let close_resp = ui.interact(close, self.id.with(("close", i)), Sense::click());
            close_resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Close {}", tab.label)));
            if selected || resp.hovered() || close_resp.hovered() {
                let color = if close_resp.hovered() { c(p.panel_text) } else { shade(&p, bg, 1.6) };
                let k = close.shrink(3.0);
                painter.line_segment([k.left_top(), k.right_bottom()], Stroke::new(1.5, color));
                painter.line_segment([k.right_top(), k.left_bottom()], Stroke::new(1.5, color));
            }
            let mut left = r.left() + m::SPACING;
            if let Some(paint) = tab.icon {
                let icon = style::snap(ui.ctx(), Rect::from_min_size(pos2(left, cy - 8.0), vec2(16.0, 16.0)));
                paint(&painter, icon);
                left = icon.right() + m::TIGHT + 1.0;
            }
            let font = if selected { bold_font() } else { plain_font() };
            let clip = Rect::from_min_max(pos2(left, r.top()), pos2(close.left() - m::TIGHT, r.bottom()));
            let text_color = if selected { c(p.panel_text) } else { shade(&p, p.panel_text, 0.8) };
            style::label(&painter.with_clip_rect(clip), clip.left(), cy, Align::Min, tab.label, font, text_color);
            if close_resp.clicked() || resp.middle_clicked() {
                action = Some(StripAction::Close(i));
            } else if resp.secondary_clicked() {
                action = resp.interact_pointer_pos().map(|at| StripAction::Menu(i, at));
            } else if resp.clicked() {
                action = Some(StripAction::Select(i));
            }
            if resp.drag_started() && n > 1 {
                ui.ctx().data_mut(|d| d.insert_temp(drag_id, i));
            }
        }
        // A tab being dragged: a mark where it would go.
        if let Some(from) = ui.ctx().data(|d| d.get_temp::<usize>(drag_id)) {
            let pointer = ui.ctx().pointer_latest_pos();
            let to = pointer.map_or(from, |pos| (((pos.x - strip.left() - m::TIGHT) / width).round().max(0.0) as usize).min(n));
            let x = slot(to).left().clamp(strip.left() + 1.0, strip.right() - 2.0);
            let mark = c(p.keyboard_navigation);
            painter.rect_filled(Rect::from_min_max(pos2(x - 1.0, strip.top() + 2.0), pos2(x + 1.0, strip.bottom() - 1.0)), 0.0, mark);
            if !ui.input(|i| i.pointer.any_down()) {
                ui.ctx().data_mut(|d| d.remove::<usize>(drag_id));
                let to = if to > from { to - 1 } else { to };
                if to != from && from < n {
                    action = Some(StripAction::Move { from, to });
                }
            } else {
                ui.ctx().request_repaint();
            }
        }
        if self.new_tab {
            let plus = Rect::from_min_size(pos2(strip.left() + m::TIGHT + n as f32 * width + m::TIGHT, strip.top() + 4.0), vec2(new_w, STRIP_HEIGHT - 6.0));
            let resp = ui.interact(plus, self.id.with("new"), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "New Tab"));
            let color = if resp.hovered() { c(p.panel_text) } else { shade(&p, bg, 1.6) };
            let k = Rect::from_center_size(plus.center(), vec2(9.0, 9.0));
            hline(&painter, k.left(), k.right(), k.center().y.floor(), color);
            vline(&painter, k.center().x.floor(), k.top(), k.bottom(), color);
            if resp.clicked() {
                action = Some(StripAction::New);
            }
        }
        action
    }
}

// ---- small helpers ------------------------------------------------------

/// The standard window content area: inset from the window edges by
/// [`m::WINDOW_INSET`], stacking top to bottom.
pub fn window_content<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = ui.available_rect_before_wrap().shrink(m::WINDOW_INSET);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(Align::Min)));
    let r = add(&mut child);
    ui.advance_cursor_after_rect(ui.available_rect_before_wrap());
    r
}

/// A row of buttons aligned to the right edge, as at the bottom of
/// dialogs and preference panels. Add buttons in reading order.
pub fn button_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING + 2.0;
        add(ui)
    })
    .inner
}

/// BColorControl: grey, red, green and blue ramps from black, each with a
/// marker you click or drag (grey sets all three), and Red/Green/Blue
/// number fields beside them. Changes `rgb` and marks the response changed.
pub struct ColorControl<'a> {
    id: Id,
    rgb: &'a mut [u8; 3],
    width: f32,
}

impl<'a> ColorControl<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, rgb: &'a mut [u8; 3]) -> Self {
        ColorControl { id: Id::unique(id), rgb, width: 256.0 + 4.0 }
    }
    /// Width of the ramps (the fields come after).
    pub fn ramp_width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
}

/// Width of a ColorControl's number fields with their labels.
const COLOR_FIELD_W: f32 = 96.0;

impl Widget for ColorControl<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let height = 3.0 * m::CONTROL_HEIGHT + 2.0 * m::TIGHT;
        let size = vec2(self.width + m::SPACING + COLOR_FIELD_W, height);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let ramps = Rect::from_min_size(rect.min, vec2(self.width, height));
        let mut response = ui.interact(ramps, self.id, Sense::click_and_drag());
        let inner = ramps.shrink(2.0);
        let row = (inner.height() / 4.0).floor();
        let lanes: [Rect; 4] = std::array::from_fn(|i| {
            let top = inner.top() + i as f32 * row;
            let bottom = if i == 3 { inner.bottom() } else { top + row };
            Rect::from_min_max(pos2(inner.left(), top), pos2(inner.right(), bottom))
        });
        let value_at = |x: f32| (((x - inner.left()) / (inner.width() - 1.0)).clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut rgb = *self.rgb;
        // The lane pressed in keeps the drag, wherever it wanders.
        if (response.is_pointer_button_down_on() || response.clicked())
            && let (Some(pos), Some(origin)) = (response.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin()))
        {
            let v = value_at(pos.x);
            match lanes.iter().position(|r| origin.y < r.bottom()).unwrap_or(3) {
                0 => rgb = [v; 3],
                l => rgb[l - 1] = v,
            }
        }

        // The number fields; their text follows the colour unless being typed in.
        let field_x = ramps.right() + m::SPACING;
        for (i, label) in ["Red:", "Green:", "Blue:"].into_iter().enumerate() {
            let text_id = self.id.with(("text", i));
            let mut text = ui.ctx().data(|d| d.get_temp::<String>(text_id)).unwrap_or_default();
            let r = Rect::from_min_size(pos2(field_x, rect.top() + i as f32 * (m::CONTROL_HEIGHT + m::TIGHT)), vec2(COLOR_FIELD_W, m::CONTROL_HEIGHT));
            let field = ui.place(r, TextControl::new(&mut text).label(label).divider(COLOR_FIELD_W - 40.0).width(COLOR_FIELD_W));
            if field.has_focus() {
                if field.changed()
                    && let Ok(v) = text.trim().parse::<u32>()
                {
                    rgb[i] = v.min(255) as u8;
                }
            } else {
                text = rgb[i].to_string();
            }
            ui.ctx().data_mut(|d| d.insert_temp(text_id, text));
        }

        if rgb != *self.rgb {
            *self.rgb = rgb;
            response.mark_changed();
        }
        let painter = ui.painter();
        let ends = [Color32::WHITE, Color32::from_rgb(255, 0, 0), Color32::from_rgb(0, 255, 0), Color32::from_rgb(0, 0, 255)];
        for (i, lane) in lanes.iter().enumerate() {
            let fill = if ui.is_enabled() { ends[i] } else { ends[i].gamma_multiply(0.4) };
            hgradient(painter, *lane, Color32::BLACK, fill);
            // The marker: where the value is (grey only when it is one).
            let v = match i {
                0 if rgb[0] == rgb[1] && rgb[1] == rgb[2] => Some(rgb[0]),
                0 => None,
                l => Some(rgb[l - 1]),
            };
            if let Some(v) = v {
                let x = (lane.left() + v as f32 / 255.0 * (lane.width() - 1.0)).floor();
                let mark = Rect::from_min_max(pos2(x - 1.0, lane.top()), pos2(x + 2.0, lane.bottom()));
                painter.rect_filled(mark, 0.0, Color32::WHITE);
                vline(painter, x, lane.top(), lane.bottom(), Color32::BLACK);
            }
        }
        style::sunken(painter, ramps, &p, response.has_focus());
        response
    }
}

/// A colour swatch well (BColorControl's sample); clicking returns true.
pub fn color_well(ui: &mut Ui, color: Color32, size: Vec2) -> Response {
    let p = style::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect.shrink(2.0), 0.0, color);
    style::sunken(ui.painter(), rect, &p, response.has_focus());
    response
}

/// Bold section heading as used in preference panels.
pub fn heading(ui: &mut Ui, text: &str) -> Response {
    let p = style::palette(ui.ctx());
    ui.label(egui::RichText::new(text).font(bold_font()).color(c(p.panel_text)))
}

/// Align a label to the left of a fixed-width column (form rows).
pub fn form_label(ui: &mut Ui, text: &str, width: f32) {
    let p = style::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover());
    let g = layout(ui, text, plain_font(), c(p.panel_text));
    let pos = text_pos(ui, rect.right() - g.size().x - m::LABEL_GAP, rect.center().y, &g);
    ui.painter().galley(pos, g, c(p.panel_text));
}

// ---- inline label editing ------------------------------------------------

/// How an [`InlineEdit`] ended this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineResult {
    Editing,
    /// Enter, or focus moved elsewhere (BeOS commits on click-away).
    Commit,
    /// Escape.
    Cancel,
}

/// Edit a label where it stands (Tracker's Edit Name): the label is
/// replaced by a field sitting on the label's own baseline, so the text
/// doesn't move when editing starts. `origin` is where the label's galley
/// was drawn and `align` how it was anchored (`Center` for icon labels,
/// whose rows are laid out around x = 0). It starts focused with the name
/// up to its extension selected.
pub struct InlineEdit<'a> {
    id: Id,
    text: &'a mut String,
    origin: Pos2,
    align: Align,
    font: FontId,
}

/// Padding between an edited label's text and its field's frame (matches
/// the selected-label highlight, so selection and editing line up).
pub const INLINE_PAD: Vec2 = vec2(4.0, 4.0);

impl<'a> InlineEdit<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, text: &'a mut String, origin: Pos2, align: Align) -> Self {
        InlineEdit { id: Id::unique(id), text, origin, align, font: plain_font() }
    }
    pub fn font(mut self, font: FontId) -> Self {
        self.font = font;
        self
    }

    pub fn show(self, ui: &mut Ui) -> InlineResult {
        let p = style::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        let started = self.id.with("started");
        let first = !ctx.data(|d| d.get_temp::<bool>(started).unwrap_or(false));

        // The field hugs the text (plus room for the caret) and grows as
        // you type, staying anchored the way the label was.
        let measure = if self.text.is_empty() { "M".to_owned() } else { self.text.clone() };
        let g = ui.painter().layout_no_wrap(measure, self.font.clone(), Color32::BLACK);
        let w = g.size().x;
        let left = match self.align {
            Align::Center => self.origin.x - w / 2.0,
            Align::Max => self.origin.x - w,
            Align::Min => self.origin.x,
        };
        let text_origin = pos2(left, self.origin.y);
        let field = style::snap(&ctx, style::text_highlight(&g, text_origin - vec2(g.rect.left(), 0.0), self.font.size, INLINE_PAD));
        let painter = ui.painter();
        painter.rect_filled(field, 0.0, c(p.document_background));
        style::frame_rect(painter, field, c(p.keyboard_navigation));

        let row_h = ui.fonts_mut(|f| f.row_height(&self.font));
        // Room for the caret after the last character, inside the padding.
        let line = Rect::from_min_size(text_origin, vec2(w + INLINE_PAD.x, row_h));
        let edit = egui::TextEdit::singleline(self.text)
            .id(self.id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(self.font.clone())
            .text_color(c(p.document_text))
            .desired_width(line.width());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(line));
        child.set_clip_rect(field.shrink(1.0).intersect(ui.clip_rect()));
        let r = child.add(edit);

        if first {
            ctx.data_mut(|d| d.insert_temp(started, true));
            r.request_focus();
            // Select the name, not its extension (".txt" rarely changes).
            let stem = match self.text.rfind('.') {
                Some(i) if i > 0 => self.text[..i].chars().count(),
                _ => self.text.chars().count(),
            };
            if let Some(mut state) = egui::TextEdit::load_state(&ctx, self.id) {
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(stem),
                )));
                state.store(&ctx, self.id);
            }
            ctx.request_repaint();
            return InlineResult::Editing;
        }
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        // A press outside the field commits, as does losing focus to
        // another widget. Until the window has keyboard focus (say, just
        // after a popup menu closed) keep asking for it rather than ending.
        let pressed_outside =
            ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !field.contains(p)));
        let done = if escape {
            InlineResult::Cancel
        } else if r.lost_focus() || pressed_outside {
            InlineResult::Commit
        } else {
            if !r.has_focus() {
                if ctx.memory(|m| m.focused().is_some_and(|f| f != self.id)) {
                    // Something else took focus.
                    ctx.data_mut(|d| d.remove::<bool>(started));
                    return InlineResult::Commit;
                }
                r.request_focus();
                ctx.request_repaint();
            }
            InlineResult::Editing
        };
        if done != InlineResult::Editing {
            ctx.data_mut(|d| d.remove::<bool>(started));
        }
        done
    }
}

// ---- alerts --------------------------------------------------------------

/// Width of a BAlert window, and of its tinted stripe down the left.
pub const ALERT_WIDTH: u32 = 380;
const ALERT_STRIPE: f32 = 30.0;
const ALERT_ICON: f32 = 32.0;
/// Text column: from the stripe's edge, past the icon that overlaps it.
const ALERT_TEXT_X: f32 = ALERT_STRIPE + ALERT_ICON / 2.0 + m::WINDOW_INSET;

/// The window height an alert with `message` needs (for `open_window`).
/// Estimated from the message length, since no context exists yet.
pub fn alert_height(message: &str) -> u32 {
    let per_line = ((ALERT_WIDTH as f32 - ALERT_TEXT_X - m::WINDOW_INSET) / 6.2) as usize;
    let lines = message.split('\n').map(|l| l.chars().count().div_ceil(per_line).max(1)).sum::<usize>();
    let text = lines as f32 * (style::PLAIN_SIZE + 4.0);
    (m::WINDOW_INSET + text.max(ALERT_ICON) + m::WINDOW_INSET + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil() as u32
}

/// BAlert: a darker stripe down the left with the icon straddling it, the
/// message beside it, and buttons along the bottom right (the last one is
/// the default, chosen by Enter). Returns the index of the button chosen;
/// Escape chooses the first when there is more than one, as Cancel
/// usually comes first.
/// The BAlert backdrop: the stripe down the left with `icon` straddling
/// it. Returns the column for the alert's content, beside the icon and
/// inside the window's margins (for alerts with more than a message, such
/// as a password field).
pub fn alert_frame(ui: &mut Ui, icon: &str) -> Rect {
    let p = style::palette(ui.ctx());
    let full = ui.available_rect_before_wrap();
    let painter = ui.painter();
    let stripe = Rect::from_min_max(full.min, pos2(full.left() + ALERT_STRIPE, full.bottom()));
    painter.rect_filled(stripe, 0.0, shade(&p, p.panel_background, 1.1));
    let icon_rect = Rect::from_min_size(pos2(stripe.right() - ALERT_ICON / 2.0, full.top() + m::WINDOW_INSET), vec2(ALERT_ICON, ALERT_ICON));
    crate::icons::paint(painter, icon_rect, icon);
    Rect::from_min_max(pos2(full.left() + ALERT_TEXT_X, full.top() + m::WINDOW_INSET), pos2(full.right() - m::WINDOW_INSET, full.bottom() - m::WINDOW_INSET))
}

/// Text in an alert's `column` (from [`alert_frame`]) from `top`, wrapped
/// to it; where it ends.
pub fn alert_text(ui: &Ui, column: Rect, top: f32, text: &str) -> f32 {
    let p = style::palette(ui.ctx());
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(plain_font(), c(p.panel_text)));
    job.wrap.max_width = column.width();
    let g = ui.painter().layout_job(job);
    let bottom = top + g.size().y;
    ui.painter().galley(pos2(column.left(), top), g, c(p.panel_text));
    bottom
}

/// The buttons along the bottom of an alert's `column` (from
/// [`alert_frame`]), for alerts with fields of their own: the last is the
/// default, chosen by Enter, and is disabled until `ready`; Escape
/// chooses the first.
pub fn alert_buttons(ui: &mut Ui, column: Rect, buttons: &[&str], ready: bool) -> Option<usize> {
    let row = Rect::from_min_max(pos2(column.left(), column.bottom() - m::CONTROL_HEIGHT), column.max);
    let mut chosen = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
        button_row(ui, |ui| {
            for (i, label) in buttons.iter().enumerate().rev() {
                let last = i + 1 == buttons.len();
                if ui.add_enabled(ready || !last, Button::new(label).default(last)).clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    ui.input(|i| {
        if i.key_pressed(Key::Enter) && ready {
            chosen = Some(buttons.len() - 1);
        } else if i.key_pressed(Key::Escape) {
            chosen = Some(0);
        }
    });
    chosen
}

pub fn alert(ui: &mut Ui, icon: &str, message: &str, buttons: &[&str]) -> Option<usize> {
    let p = style::palette(ui.ctx());
    let full = ui.available_rect_before_wrap();
    let column = alert_frame(ui, icon);
    let icon_top = column.top();
    let text_left = column.left();
    let mut job = egui::text::LayoutJob::single_section(message.to_owned(), egui::TextFormat::simple(plain_font(), c(p.panel_text)));
    job.wrap.max_width = full.right() - m::WINDOW_INSET - text_left;
    let g = ui.painter().layout_job(job);
    // First line's capitals level with the icon's upper third.
    let pos = style::text_origin(ui.ctx(), &g, text_left, icon_top + style::PLAIN_SIZE / 2.0 + 2.0, style::PLAIN_SIZE);
    ui.painter().galley(pos, g, c(p.panel_text));

    let row = Rect::from_min_max(
        pos2(text_left, full.bottom() - m::WINDOW_INSET - m::CONTROL_HEIGHT),
        pos2(full.right() - m::WINDOW_INSET, full.bottom() - m::WINDOW_INSET),
    );
    let mut chosen = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
        button_row(ui, |ui| {
            for (i, label) in buttons.iter().enumerate().rev() {
                if ui.add(Button::new(label).default(i + 1 == buttons.len())).clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    ui.input(|i| {
        if i.key_pressed(Key::Enter) {
            chosen = Some(buttons.len() - 1);
        } else if buttons.len() > 1 && i.key_pressed(Key::Escape) {
            chosen = Some(0);
        }
    });
    chosen
}

