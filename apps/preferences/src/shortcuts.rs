//! Keyboard ▸ Shortcuts: every shortcut of the keyboard profile:
//! app_server's (tiling, workspaces, Twitcher, Find, screenshots, …),
//! then every app's (Copy, Close, …) and each app's own (Tracker's Move to
//! Trash), as the profile's platform writes keys. Click a shortcut's keys,
//! then type new ones: app_server records them (only it can tell ⌘ from
//! Ctrl) and they replace the old, taken from any other shortcut that
//! had them. Any change makes a platform's profile "… (Customized)".

use olo_kit::{
    egui::{Align2, Rect, Sense, Ui, UiBuilder, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{Button, ScrollView, StringView},
};
use olo_protocol::{
    Chord, Settings, app_keys,
    shortcuts::{self, Entry, GROUPS, SHORTCUTS},
};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    /// The shortcut being recorded (its id).
    recording: Option<&'static str>,
    /// What the last change did, to say under the list.
    note: Option<String>,
    /// The name being typed for the profile's Save As.
    saving: Option<String>,
}

enum Row {
    Group(&'static str),
    Shortcut(Entry),
}

fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for g in GROUPS {
        rows.push(Row::Group(g));
        rows.extend(SHORTCUTS.iter().filter(|i| i.group == *g).filter_map(|i| shortcuts::entry(i.id)).map(Row::Shortcut));
    }
    let apps = std::iter::once(None).chain(app_keys::APPS.iter().map(|(a, _)| Some(*a)));
    for app in apps {
        let keys: Vec<Entry> = shortcuts::entries().filter(|e| e.scope == Some(app)).collect();
        if !keys.is_empty() {
            rows.push(Row::Group(app_keys::app_title(app)));
            rows.extend(keys.into_iter().map(Row::Shortcut));
        }
    }
    rows
}

/// How a shortcut's keys read: "⇧⌘←", several joined, or "None".
fn keys_text(e: &Entry, s: &Settings) -> String {
    let p = &s.keyboard.profile;
    let keys = e.keys(p);
    if keys.is_empty() { "None".to_owned() } else { keys.iter().map(|c| c.label(p.platform)).collect::<Vec<_>>().join("  ") }
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
                let taken = shortcuts::assign(&mut s.keyboard.profile, id, chord);
                s.keyboard.profile_changed();
                let platform = s.keyboard.profile.platform;
                (!taken.is_empty()).then(|| format!("{} was taken from “{}”.", chord.map(|c| c.label(platform)).unwrap_or_default(), taken.join("”, “")))
            }
        };
    }

    crate::keyboard::profile(ui, s, &mut st.saving);

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
                if ui.add_enabled(!s.keyboard.profile.shortcuts.is_empty(), Button::new("Default Keys")).clicked() {
                    s.keyboard.profile.shortcuts.clear();
                    s.keyboard.profile_changed();
                    st.note = None;
                }
            });
        });
    });
}
