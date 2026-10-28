//! Keyboard ▸ Shortcuts: app_server's own shortcuts (tiling, workspaces,
//! Find, screenshots, …); the space they leave around windows is in
//! Windows. Click a shortcut's keys, then type new ones: app_server
//! records them (only it can tell ⌘ from Ctrl) and they replace the old,
//! taken from any other shortcut that had them.

use olo_kit::{
    egui::{Align2, Rect, Sense, Ui, UiBuilder, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{Button, ScrollView, StringView},
};
use olo_protocol::{
    Chord, Settings,
    shortcuts::{self, FIXED, GROUPS, SHORTCUTS, ShortcutInfo},
};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    /// The shortcut being recorded (its id).
    recording: Option<&'static str>,
    /// What the last change did, to say under the list.
    note: Option<String>,
}

enum Row {
    Group(&'static str),
    Shortcut(&'static ShortcutInfo),
    Fixed(&'static str, &'static str),
}

fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for g in GROUPS {
        rows.push(Row::Group(g));
        rows.extend(SHORTCUTS.iter().filter(|i| i.group == *g).map(Row::Shortcut));
        if *g == shortcuts::WINDOWS {
            rows.extend(FIXED.iter().map(|(t, k)| Row::Fixed(t, k)));
        }
    }
    rows
}

/// How a shortcut's keys read: "⇧⌘←", several joined, or "None".
fn keys_text(info: &ShortcutInfo, s: &Settings) -> String {
    let keys = shortcuts::keys_for(info, &s.shortcuts);
    if keys.is_empty() { "None".to_owned() } else { keys.iter().map(Chord::symbols).collect::<Vec<_>>().join("  ") }
}

pub fn ui(ui: &mut Ui, s: &mut Settings, server: &Server, st: &mut State) {
    // The keys typed while recording (or that it ended).
    if let Some(id) = st.recording
        && let Some(recorded) = server.take_recorded()
    {
        st.recording = None;
        st.note = match recorded {
            None => None,
            Some(keys) => {
                let chord = if keys.is_empty() { None } else { keys.parse::<Chord>().ok() };
                let taken = shortcuts::assign(&mut s.shortcuts, id, chord);
                (!taken.is_empty()).then(|| format!("{} was taken from “{}”.", chord.map(|c| c.symbols()).unwrap_or_default(), taken.join("”, “")))
            }
        };
    }

    let p = style::palette(ui.ctx());
    let rows = rows();
    let hint_h = m::CONTROL_HEIGHT;
    let avail = ui.available_rect_before_wrap();
    let list = Rect::from_min_max(avail.min, pos2(avail.right(), avail.bottom() - hint_h - m::SPACING));
    let mut clicked = None;
    ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollView::new("shortcuts").background(p.document_background).show_rows(ui, m::LIST_ROW, rows.len(), |ui, range| {
            for i in range {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), m::LIST_ROW), Sense::click());
                let painter = ui.painter();
                let text = c(p.document_text);
                match &rows[i] {
                    Row::Group(g) => {
                        painter.text(pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, *g, style::bold_font(), text);
                    }
                    Row::Fixed(title, keys) => {
                        let dim = style::disabled_text(&p);
                        painter.text(pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, *title, style::plain_font(), dim);
                        style::key_combo(painter, pos2(r.right() - 8.0, r.center().y), keys, dim);
                    }
                    Row::Shortcut(info) => {
                        let recording = st.recording == Some(info.id);
                        let (fg, keys) = if recording {
                            painter.rect_filled(r, 0.0, c(p.list_selected_background));
                            (c(p.list_selected_item_text), "Type the keys…".to_owned())
                        } else {
                            (text, keys_text(info, s))
                        };
                        painter.text(pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, info.title, style::plain_font(), fg);
                        let right = pos2(r.right() - 8.0, r.center().y);
                        if recording || keys == "None" {
                            painter.text(right, Align2::RIGHT_CENTER, keys, style::plain_font(), if recording { fg } else { style::disabled_text(&p) });
                        } else {
                            style::key_combo(painter, right, &keys, fg);
                        }
                        if resp.clicked() {
                            clicked = Some(info.id);
                        }
                    }
                }
            }
        });
    });
    if let Some(id) = clicked {
        if st.recording == Some(id) {
            st.recording = None;
            server.record_shortcut(false);
        } else {
            st.recording = Some(id);
            st.note = None;
            server.take_recorded();
            server.record_shortcut(true);
        }
    }

    let hint = Rect::from_min_max(pos2(avail.left(), list.bottom() + m::SPACING), avail.max);
    ui.scope_builder(UiBuilder::new().max_rect(hint), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let text = match (&st.recording, &st.note) {
            (Some(_), _) => "Type the keys. Esc cancels; ⌫ removes them.".to_owned(),
            (None, Some(note)) => note.clone(),
            (None, None) => "Click a shortcut to give it other keys.".to_owned(),
        };
        ui.horizontal(|ui| {
            ui.add(StringView::new(&text).dim(st.note.is_none() || st.recording.is_some()));
            ui.with_layout(olo_kit::egui::Layout::right_to_left(olo_kit::egui::Align::Center), |ui| {
                if ui.add_enabled(!s.shortcuts.is_empty(), Button::new("Default Keys")).clicked() {
                    s.shortcuts.clear();
                    st.note = None;
                }
            });
        });
    });
}
