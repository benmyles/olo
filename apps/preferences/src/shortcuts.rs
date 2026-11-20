//! Keyboard ▸ Shortcuts: every shortcut of the keyboard profile:
//! app_server's (tiling, workspaces, Twitcher, Find, screenshots, …),
//! then every app's (Copy, Close, …) and each app's own (Tracker's Move to
//! Trash), as the profile's platform writes keys. Click a shortcut's keys,
//! then type new ones: app_server records them (only it can tell ⌘ from
//! Ctrl) and they replace the old, taken from any other shortcut that
//! had them. Any change makes a platform's profile "… (Customized)".
//!
//! A shortcut can be two chords in turn (`olo_protocol::Keys`: the first
//! begins a group, the second picks from it): Add a Second Key, for the
//! shortcut just given a chord, records the second. It is asked for, not
//! waited for after every chord: the next key typed after giving a
//! shortcut its keys is as likely that shortcut being tried.

use std::rc::Rc;

use olo_kit::{
    Bounds, Child, Cx, FontWeight, InteractiveElement as _, MouseButton, MouseDownEvent, ParentElement as _, ScrollHandle, Styled as _, canvas, div, metrics as m, point, px, size,
    style::{self, c},
    theme,
    widgets::{Button, ScrollView, StringView, TextField},
};
use olo_protocol::{
    Chord, Keys, Settings, app_keys,
    shortcuts::{self, Entry, GROUPS, SHORTCUTS},
};

use crate::{PanelView, form::Ui, keyboard, server::Server};

pub struct State {
    /// The shortcut being recorded (its id).
    recording: Option<&'static str>,
    /// The chord the shortcut being recorded has: what's recorded now is
    /// its second (Add a Second Key).
    first: Option<Chord>,
    /// The shortcut last given one chord here, and the chord: the one
    /// Add a Second Key adds to.
    last: Option<(&'static str, Chord)>,
    /// The shortcut whose keys were last changed here: lit faintly till
    /// another is clicked, so what a change did is in sight (after a
    /// second key too, when there's nothing more to add).
    lit: Option<&'static str>,
    /// What the last change did, to say under the list.
    note: Option<String>,
    /// The name being typed for the profile's Save As.
    pub saving: Option<Child<TextField>>,
    scroll: ScrollHandle,
    rows: Rc<Vec<Row>>,
}

impl State {
    /// A shortcut's keys are being typed: nothing else of the panel's
    /// is to be pressed meanwhile (Defaults, Revert).
    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn new(_: &mut Cx<PanelView>) -> State {
        State { recording: None, first: None, last: None, lit: None, note: None, saving: None, scroll: ScrollHandle::new(), rows: Rc::new(rows()) }
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

/// How a shortcut's keys read: "⇧⌘←", several with a comma between
/// (two chords in turn have room between: "⌃⌥⌘Z  V"), or "None".
fn keys_text(e: &Entry, s: &Settings) -> String {
    let p = &s.keyboard.profile;
    let keys = e.keys(p);
    if keys.is_empty() { "None".to_owned() } else { olo_protocol::Keys::labels(&keys, p.platform) }
}

pub fn ui(ui: &mut Ui, server: &Server, st: &mut State) {
    // The keys typed while recording (or that it ended).
    if let Some(id) = st.recording
        && let Some(recorded) = server.take_recorded()
    {
        st.recording = None;
        let first = st.first.take();
        // What it's given: the chord typed; with a first chord, that and
        // the one typed, in turn. (Esc, or no key, for a second: it
        // keeps the one it has.)
        let typed = recorded.map(|keys| if keys.is_empty() { None } else { keys.parse::<Chord>().ok() });
        let given = match (first, typed) {
            (_, None) | (Some(_), Some(None)) => None,
            (None, Some(chord)) => Some(chord.map(Keys::from)),
            (Some(first), Some(Some(then))) => Some(Some(Keys { first, then: Some(then) })),
        };
        st.last = None;
        st.lit = given.is_some().then_some(id).or(st.lit.filter(|l| *l == id));
        st.note = given.and_then(|keys| {
            let mut keyboard = ui.s.keyboard.clone();
            let taken = shortcuts::assign(&mut keyboard.profile, id, keys);
            keyboard.profile_changed();
            let platform = keyboard.profile.platform;
            crate::prefs().edit_as("", "Shortcut", |s| s.keyboard = keyboard);
            st.last = keys.and_then(|k| k.single()).map(|c| (id, c));
            let taken: Vec<&str> = taken.into_iter().map(olo_kit::words::tr).collect();
            (!taken.is_empty()).then(|| olo_kit::tr!("{} was taken from “{}”.", keys.map(|c| c.label(platform)).unwrap_or_default(), taken.join("”, “")))
        });
    }

    keyboard::profile(ui, st.saving.as_ref());

    let rows = st.rows.clone();
    let s = ui.s.clone();
    let has_own = !s.keyboard.profile.shortcuts.is_empty();
    let recording = st.recording;
    let has_first = st.first;
    // (The shortcut last changed: the one Add a Second Key is for,
    // while that's offered.)
    let given = st.lit.filter(|_| st.recording.is_none());
    let n = rows.len();
    // A click on a shortcut records its keys (again: stops).
    let server2 = server.clone();
    let click = Rc::new(ui.on_view(move |v, i: &usize, cx| {
        let Some(st) = keyboard::state_of(v).map(|k| &mut k.shortcuts) else { return };
        let Some(Row::Shortcut(info)) = st.rows.get(*i) else { return };
        let id = info.id;
        st.first = None;
        if st.recording == Some(id) {
            st.recording = None;
            server2.record_shortcut(false);
        } else {
            st.recording = Some(id);
            st.note = None;
            st.last = None;
            st.lit = None;
            server2.take_recorded();
            server2.record_shortcut(true);
        }
        cx.notify();
    }));
    // Add a Second Key: the shortcut last given a chord records another,
    // to be pressed after it.
    let server3 = server.clone();
    let second = ui.on_click_view(move |v, cx| {
        let Some(st) = keyboard::state_of(v).map(|k| &mut k.shortcuts) else { return };
        let Some((id, chord)) = st.last else { return };
        (st.recording, st.first, st.note) = (Some(id), Some(chord), None);
        server3.take_recorded();
        server3.record_shortcut(true);
        cx.notify();
    });
    let platform = s.keyboard.profile.platform;
    // Only the rows in view are drawn (again as it scrolls: what shows
    // of a drawing is the painter's to say).
    let list = canvas(move |g, b| {
        let p = theme::palette();
        let clip = g.visible_bounds().intersect(&b);
        let first = (f32::from(clip.top() - b.top()) / m::LIST_ROW).floor().max(0.0) as usize;
        let last = ((f32::from(clip.bottom() - b.top()) / m::LIST_ROW).ceil().max(0.0) as usize).min(n);
        let text = c(p.document_text);
        for i in first..last {
            let r = Bounds { origin: point(b.left(), b.top() + px(i as f32 * m::LIST_ROW)), size: size(b.size.width, px(m::LIST_ROW)) };
            match &rows[i] {
                // (The list is painted here, so its words are asked for
                // here: a shortcut's title and its group are the
                // protocol's, in English, for every program that shows
                // them.)
                Row::Group(name) => {
                    style::label(g, r.left() + px(6.0), r.center().y, style::Align::Left, olo_kit::words::tr(name), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), text);
                }
                Row::Shortcut(info) => {
                    let on = recording == Some(info.id);
                    let (fg, keys) = if on {
                        style::fill_rect(g, r, c(p.list_selected_background));
                        // (The second key's place kept by an ellipsis,
                        // where it will be: a comma is between a
                        // shortcut's several keys.)
                        let words = match has_first {
                            Some(chord) => format!("{}  …", chord.label(platform)),
                            None => olo_kit::words::tr("Type the keys…").to_owned(),
                        };
                        (c(p.list_selected_item_text), words)
                    } else {
                        // (The one last changed, which Add a Second Key
                        // is for: lit faintly, so it's in sight.)
                        if given == Some(info.id) {
                            style::fill_rect(g, r, c(p.list_selected_background).opacity(0.45));
                        }
                        (text, keys_text(info, &s))
                    };
                    let title = olo_kit::words::tr(info.title);
                    style::label(g, r.left() + px(18.0), r.center().y, style::Align::Left, title, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), fg);
                    let right = point(r.right() - px(8.0), r.center().y);
                    if on || keys == "None" {
                        let ink = if on { fg } else { style::disabled_text(&p) };
                        let keys = if on { keys.clone() } else { olo_kit::words::tr("None").to_owned() };
                        style::label(g, right.x, right.y, style::Align::Right, keys, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                    } else {
                        style::key_combo_column(g, right, &keys, fg);
                    }
                    olo_kit::agent::record(
                        g,
                        olo_kit::agent::Node { role: "ListItem", name: Some(format!("{title}: {keys}").into()), bounds: r, enabled: true, clickable: true, selected: Some(on), ..Default::default() },
                    );
                }
            }
        }
    })
    .w_full()
    .h(px(n as f32 * m::LIST_ROW))
    // (Where the list is now, scrolled: the listener's own bounds.)
    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
        let b = cx.listener_bounds();
        click(&((f32::from(e.position.y - b.top()) / m::LIST_ROW).floor().max(0.0) as usize), cx);
    });
    let p = theme::palette();

    let text = match (&st.recording, st.first, &st.note) {
        (Some(_), Some(chord), _) => olo_kit::tr!("Type the key to press after {}. Esc leaves it as it is.", chord.label(platform)),
        (Some(_), None, _) => olo_kit::words::tr("Type the keys. Esc cancels; ⌫ removes them.").to_owned(),
        (None, _, Some(note)) => note.clone(),
        (None, _, None) => olo_kit::words::tr("Click a shortcut to give it other keys.").to_owned(),
    };
    let can_add = st.last.is_some() && st.recording.is_none();
    // (Only the idle hint steps back: what to type now, and what a
    // change took, are said in full.)
    let dim = st.note.is_none() && st.recording.is_none();
    let defaults = ui.edit_click(|s| {
        s.keyboard.profile.shortcuts.clear();
        s.keyboard.profile_changed();
    });
    let clear_note = ui.on_click_view(|v, cx| {
        if let Some(k) = keyboard::state_of(v) {
            k.shortcuts.note = None;
        }
        cx.notify();
    });
    // (The note a line of its own under the list, whose caption it is,
    // nearer it than the buttons are: beside two buttons it had half
    // the width, and English's wrapped too. It wraps where a language's
    // words are longer.)
    ui.add(
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(div().flex_1().min_h_0().child(ScrollView::new(&st.scroll).background(p.document_background).child(list)))
            .child(div().w_full().min_w_0().flex_shrink_0().child(StringView::new(text).dim(dim).wrap(true))),
    );
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            // (The one shortcut's at the left, under the note about it;
            // every shortcut's at the right.)
            .justify_between()
            .gap(px(m::SPACING))
            // (Two chords in turn: the first begins a group, as ⌃⌥⌘Z does
            // Zoom's; the second picks from it.)
            .child(olo_kit::tip::on(
                olo_kit::IntoElement::into_element(Button::new("Add a Second Key").enabled(can_add).on_click(second)),
                "shortcut-second",
                "The shortcut you just gave keys can have a second, pressed after them: the first keys begin a group of shortcuts, the second picks one.",
            ))
            // (Every shortcut's, not the one lit: said, with the panel's
            // own Defaults a row below it.)
            .child(Button::new("All Default Keys").enabled(has_own && st.recording.is_none()).on_click(move |window, cx| {
                defaults(window, cx);
                clear_note(window, cx);
            })),
    );
}
