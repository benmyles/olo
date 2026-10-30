//! A sidebar of sections (a Mac's source list; Haiku's preference apps
//! list theirs the same way, in a BListView down the left): rows of an
//! icon, a name and, at the right, a quiet value ("12%"), under optional
//! headings, the chosen one in the list's selection. Click a row, or use
//! ↑ and ↓ while it has the keyboard (it takes it when clicked); agents
//! find each row as a selectable `ListItem` named by it, its value as the
//! row's value.
//!
//! ```ignore
//! let rows = [Row::heading("Monitor"), Row::new("cpu", "CPU").value("12%"), Row::new("memory", "Memory")];
//! if Sidebar::new("sections", &rows, &mut chosen).show(ui, rect).changed { … }
//! ```

use egui::{Align, Key, Rect, Sense, Ui, pos2, vec2};

use crate::{
    icons, metrics as m,
    style::{self, c},
};

/// A row's height.
pub const ROW: f32 = 24.0;
/// A heading's.
pub const HEADING: f32 = 22.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    /// Its value at the right, if it has one.
    pub value: Option<&'a str>,
    /// A heading over the rows that follow (not chosen).
    pub heading: bool,
}

impl<'a> Row<'a> {
    pub fn new(icon: &'a str, label: &'a str) -> Self {
        Row { icon, label, value: None, heading: false }
    }

    pub fn heading(label: &'a str) -> Self {
        Row { icon: "", label, value: None, heading: true }
    }

    pub fn value(mut self, value: &'a str) -> Self {
        self.value = Some(value);
        self
    }
}

#[derive(Debug, Default)]
pub struct SidebarResponse {
    /// A row was chosen this frame.
    pub changed: bool,
}

pub struct Sidebar<'a> {
    id: egui::Id,
    rows: &'a [Row<'a>],
    chosen: &'a mut usize,
}

impl<'a> Sidebar<'a> {
    /// `chosen` is a row's index (never a heading's).
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, rows: &'a [Row<'a>], chosen: &'a mut usize) -> Self {
        Sidebar { id: egui::Id::unique(("olo-sidebar", id)), rows, chosen }
    }

    pub fn show(self, ui: &mut Ui, rect: Rect) -> SidebarResponse {
        let p = style::palette(ui.ctx());
        let ppp = ui.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let painter = ui.painter().with_clip_rect(rect);
        // Its rows are on a grid of their own, not level with what it
        // lists for.
        crate::layout_review::own_grid(ui, rect);
        painter.rect_filled(rect, 0.0, style::shade(&p, p.panel_background, 1.04));
        // Its edge against what it lists for.
        style::vline_left_of(&painter, rect.right(), rect.top(), rect.bottom(), style::shade(&p, p.panel_background, 1.25));
        let mut out = SidebarResponse::default();
        let focus = ui.interact(rect, self.id, Sense::focusable_noninteractive());
        let focused = focus.has_focus();
        if focused {
            // ↑ and ↓ step over headings.
            let step = ui.input(|i| i32::from(i.key_pressed(Key::ArrowDown)) - i32::from(i.key_pressed(Key::ArrowUp)));
            if step != 0 {
                let mut i = *self.chosen as i32;
                loop {
                    i += step;
                    if i < 0 || i >= self.rows.len() as i32 {
                        break;
                    }
                    if !self.rows[i as usize].heading {
                        *self.chosen = i as usize;
                        out.changed = true;
                        break;
                    }
                }
            }
        }
        let mut y = rect.top() + m::TIGHT;
        for (i, row) in self.rows.iter().enumerate() {
            let h = if row.heading { HEADING } else { ROW };
            // Up to its edge line, on device pixels.
            let r = Rect::from_min_max(pos2(rect.left(), snap(y)), pos2(snap(rect.right()) - 1.0 / ppp, snap(y) + h));
            y += h;
            if row.heading {
                style::label(&painter, r.left() + m::WINDOW_INSET, r.center().y + 2.0, Align::Min, row.label.to_uppercase(), egui::FontId::new(style::SMALL_SIZE, style::bold()), style::disabled_text(&p));
                style::describe_text(ui.ctx(), egui::accesskit::Role::Heading, row.label, r, None);
                continue;
            }
            let resp = ui.interact(r, self.id.with(i), Sense::click());
            let selected = *self.chosen == i;
            if resp.clicked() {
                if !selected {
                    *self.chosen = i;
                    out.changed = true;
                }
                focus.request_focus();
            }
            resp.widget_info(|| egui::WidgetInfo {
                selected: Some(selected),
                current_text_value: row.value.map(str::to_owned),
                ..egui::WidgetInfo::labeled(egui::Role::ListItem, true, row.label)
            });
            if selected {
                let sel = if focused { c(p.list_selected_background) } else { c(olo_theme::mix(p.list_selected_background, p.panel_background, 0.35)) };
                painter.rect_filled(r, 0.0, sel);
            } else if resp.hovered() {
                painter.rect_filled(r, 0.0, style::shade(&p, p.panel_background, 1.1));
            }
            let text = if selected { c(p.list_selected_item_text) } else { c(p.panel_text) };
            let icon = Rect::from_center_size(pos2(r.left() + m::WINDOW_INSET + 8.0, r.center().y), vec2(16.0, 16.0));
            if !row.icon.is_empty() {
                icons::paint(&painter, icon, row.icon);
            }
            let value_w = row.value.map_or(0.0, |v| painter.layout_no_wrap(v.to_owned(), style::plain_font(), text).size().x + m::SPACING);
            let left = icon.right() + 6.0;
            style::label_fit(&painter, left, r.center().y, Align::Min, row.label, style::plain_font(), text, (r.right() - m::WINDOW_INSET - value_w - left).max(0.0));
            if let Some(v) = row.value {
                let quiet = if selected { text } else { style::disabled_text(&p) };
                style::label(&painter, r.right() - m::WINDOW_INSET, r.center().y, Align::Max, v, style::plain_font(), quiet);
            }
        }
        out
    }
}

/// How tall `rows` stand in a sidebar.
pub fn height(rows: &[Row]) -> f32 {
    m::TIGHT + rows.iter().map(|r| if r.heading { HEADING } else { ROW }).sum::<f32>()
}
