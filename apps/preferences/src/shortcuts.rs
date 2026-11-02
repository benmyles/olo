//! Keyboard ▸ Shortcuts: every shortcut of the keyboard profile:
//! app_server's (tiling, workspaces, Twitcher, Find, screenshots, …),
//! then every app's (Copy, Close, …) and each app's own (Tracker's Move to
//! Trash), as the profile's platform writes keys. Click a shortcut's keys,
//! then type new ones: app_server records them (only it can tell ⌘ from
//! Ctrl) and they replace the old, taken from any other shortcut that
//! had them. Any change makes a platform's profile "… (Customized)".

use std::rc::Rc;

use olo_protocol::{
    Chord, Settings, app_keys,
    shortcuts::{self, Entry, GROUPS, SHORTCUTS},
};
use olo_ui::{
    gpui::{Bounds, Context, DispatchPhase, Entity, FontWeight, MouseButton, MouseDownEvent, ParentElement as _, ScrollHandle, Styled as _, canvas, div, point, px, size},
    metrics as m,
    style::{self, c},
    theme,
    widgets::{Button, ScrollView, StringView, TextField},
};

use crate::{PanelView, form::Ui, keyboard, server::Server};

pub struct State {
    /// The shortcut being recorded (its id).
    recording: Option<&'static str>,
    /// What the last change did, to say under the list.
    note: Option<String>,
    /// The name being typed for the profile's Save As.
    pub saving: Option<Entity<TextField>>,
    scroll: ScrollHandle,
    rows: Rc<Vec<Row>>,
}

impl State {
    pub fn new(_: &mut Context<PanelView>) -> State {
        State { recording: None, note: None, saving: None, scroll: ScrollHandle::new(), rows: Rc::new(rows()) }
    }
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

pub fn ui(ui: &mut Ui, server: &Server, st: &mut State) {
    // The keys typed while recording (or that it ended).
    if let Some(id) = st.recording
        && let Some(recorded) = server.take_recorded()
    {
        st.recording = None;
        st.note = match recorded {
            None => None,
            Some(keys) => {
                let chord = if keys.is_empty() { None } else { keys.parse::<Chord>().ok() };
                let mut keyboard = ui.s.keyboard.clone();
                let taken = shortcuts::assign(&mut keyboard.profile, id, chord);
                keyboard.profile_changed();
                let platform = keyboard.profile.platform;
                let prefs = ui.prefs();
                ui.cx.defer(move |cx| {
                    let _ = prefs.update(cx, |p, cx| p.edit(cx, |s| s.keyboard = keyboard));
                });
                (!taken.is_empty()).then(|| format!("{} was taken from “{}”.", chord.map(|c| c.label(platform)).unwrap_or_default(), taken.join("”, “")))
            }
        };
    }

    keyboard::profile(ui, st.saving.as_ref());

    let rows = st.rows.clone();
    let s = ui.s.clone();
    let has_own = !s.keyboard.profile.shortcuts.is_empty();
    let recording = st.recording;
    let n = rows.len();
    // A click on a shortcut records its keys (again: stops).
    let server2 = server.clone();
    let click = Rc::new(ui.on_view(move |v, i: usize, _, cx| {
        let Some(st) = keyboard::state_of(v).map(|k| &mut k.shortcuts) else { return };
        let Some(Row::Shortcut(info)) = st.rows.get(i) else { return };
        let id = info.id;
        if st.recording == Some(id) {
            st.recording = None;
            server2.record_shortcut(false);
        } else {
            st.recording = Some(id);
            st.note = None;
            server2.take_recorded();
            server2.record_shortcut(true);
        }
        cx.notify();
    }));
    // Only the rows in view are drawn.
    let list = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let clip = window.content_mask().bounds.intersect(&b);
            let first = (f32::from(clip.top() - b.top()) / m::LIST_ROW).floor().max(0.0) as usize;
            let last = ((f32::from(clip.bottom() - b.top()) / m::LIST_ROW).ceil().max(0.0) as usize).min(n);
            let text = c(p.document_text);
            for i in first..last {
                let r = Bounds { origin: point(b.left(), b.top() + px(i as f32 * m::LIST_ROW)), size: size(b.size.width, px(m::LIST_ROW)) };
                match &rows[i] {
                    Row::Group(g) => {
                        style::label(window, cx, r.left() + px(6.0), r.center().y, style::Align::Left, *g, style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), text);
                    }
                    Row::Shortcut(info) => {
                        let on = recording == Some(info.id);
                        let (fg, keys) = if on {
                            style::fill_rect(window, r, c(p.list_selected_background));
                            (c(p.list_selected_item_text), "Type the keys…".to_owned())
                        } else {
                            (text, keys_text(info, &s))
                        };
                        style::label(window, cx, r.left() + px(18.0), r.center().y, style::Align::Left, info.title, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), fg);
                        let right = point(r.right() - px(8.0), r.center().y);
                        if on || keys == "None" {
                            let ink = if on { fg } else { style::disabled_text(&p) };
                            style::label(window, cx, right.x, right.y, style::Align::Right, keys.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                        } else {
                            style::key_combo(window, cx, right, &keys, fg);
                        }
                        olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "ListItem", name: Some(format!("{}: {keys}", info.title).into()), bounds: r, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
                    }
                }
            }
            let click = click.clone();
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && e.button == MouseButton::Left && clip.contains(&e.position) {
                    click((f32::from(e.position.y - b.top()) / m::LIST_ROW).floor().max(0.0) as usize, window, cx);
                }
            });
        },
    )
    .w_full()
    .h(px(n as f32 * m::LIST_ROW));
    let p = theme::palette(ui.cx);
    ui.add(div().flex_1().min_h_0().child(ScrollView::new("shortcuts", &st.scroll).background(p.document_background).child(list)));

    let text = match (&st.recording, &st.note) {
        (Some(_), _) => "Type the keys. Esc cancels; ⌫ removes them.".to_owned(),
        (None, Some(note)) => note.clone(),
        (None, None) => "Click a shortcut to give it other keys.".to_owned(),
    };
    let dim = st.note.is_none() || st.recording.is_some();
    let defaults = ui.edit_click(|s| {
        s.keyboard.profile.shortcuts.clear();
        s.keyboard.profile_changed();
    });
    let clear_note = ui.on_click_view(|v, _, cx| {
        if let Some(k) = keyboard::state_of(v) {
            k.shortcuts.note = None;
        }
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(div().flex_1().child(StringView::new(text).dim(dim)))
            .child(Button::new("Default Keys").enabled(has_own).on_click(move |window, cx| {
                defaults(window, cx);
                clear_note(window, cx);
            })),
    );
}
