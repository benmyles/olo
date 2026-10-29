//! The shortcuts sheet: every shortcut of the keyboard profile on one
//! sheet, over everything (Keyboard Shortcuts: ⇧⌘/ on a Mac, Win+Shift+/
//! on Windows, Alt+Shift+/ on BeOS, as a Mac's Help is ⌘?). app_server
//! starts `olo-palette --shortcuts`, which hands it to the running copy.
//! The front app's own come first (its menus' keys, as it shows them),
//! then text editing's, then the desktop's (app_server's), each group a
//! card in columns. Esc, a click outside or the keys again put it away.

use olo_kit::{
    Cx, Keyboard, Layer, LayerOptions, Proxy, WindowId, WindowKind, WindowOptions, apps::our_app, commands,
    egui::{self, Align, Key, Rect, Sense, pos2, vec2},
    masonry, metrics as m,
    style::{self, c},
    widgets::{self, ScrollView},
};
use olo_protocol::{Chord, Client, KeyboardProfile, Platform, Reply, Request, shortcuts};

/// One card: a heading and its shortcuts (title, keys as the platform
/// writes them).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

/// What the sheet shows, learned off the UI thread.
pub struct Learned {
    pub profile: String,
    pub groups: Vec<Group>,
}

pub struct Shown {
    pub win: WindowId,
    profile: String,
    groups: Vec<Group>,
}

/// Learn the profile and the front app's menus, then send [`Learned`].
pub fn learn(proxy: Proxy) {
    let _ = std::thread::Builder::new().name("palette-sheet".into()).spawn(move || {
        let mut client = Client::connect().ok();
        let mut ask = |r: &Request| client.as_mut().and_then(|c| c.request(r).ok());
        let profile = match ask(&Request::GetSettings) {
            Some(Reply::Settings { settings }) => settings.keyboard.profile,
            _ => KeyboardProfile::default(),
        };
        let front = match ask(&Request::ListWindows) {
            Some(Reply::Windows { windows }) => windows.into_iter().find(|w| w.focused),
            _ => None,
        };
        let menus = front.and_then(|w| w.pid).and_then(|p| u32::try_from(p).ok()).and_then(commands::menus);
        proxy.send(Learned { profile: profile.name.clone(), groups: groups(&profile, menus.as_ref()) });
    });
}

/// The sheet's cards: the front app's menus (by menu), text editing, then
/// app_server's groups.
pub fn groups(profile: &KeyboardProfile, menus: Option<&commands::Menus>) -> Vec<Group> {
    let mut out = Vec::new();
    if let Some(menus) = menus {
        let app = our_app(&menus.app_id).map_or_else(|| menus.app_id.clone(), |(name, _)| name.to_owned());
        for e in menus.entries.iter().filter(|e| e.keys.is_some()) {
            let title = format!("{app}: {}", e.path.first().map_or("", String::as_str));
            let row = (e.label.trim_end_matches('…').to_owned(), e.keys.clone().unwrap_or_default());
            match out.iter_mut().find(|g: &&mut Group| g.title == title) {
                Some(g) if !g.rows.contains(&row) => g.rows.push(row),
                Some(_) => {}
                None => out.push(Group { title, rows: vec![row] }),
            }
        }
    }
    out.push(text_editing(profile.platform));
    for g in shortcuts::GROUPS {
        let rows: Vec<(String, String)> = shortcuts::SHORTCUTS
            .iter()
            .filter(|i| i.group == *g)
            .filter_map(|i| {
                let keys = shortcuts::keys_for(i, profile);
                (!keys.is_empty()).then(|| (i.title.to_owned(), keys.iter().map(|c| c.label(profile.platform)).collect::<Vec<_>>().join("  ")))
            })
            .collect();
        if !rows.is_empty() {
            out.push(Group { title: (*g).to_owned(), rows });
        }
    }
    out
}

/// Moving about text, as the platform does it (the kit's fields, and
/// app_server's keys for programs that aren't Olo's).
fn text_editing(platform: Platform) -> Group {
    let rows: &[(&str, &[&str])] = match platform {
        Platform::Mac => &[
            ("Start or end of the line", &["Cmd+Left", "Cmd+Right"]),
            ("Start or end of the document", &["Cmd+Up", "Cmd+Down"]),
            ("A word back or on", &["Alt+Left", "Alt+Right"]),
            ("Delete to the line's start", &["Cmd+Backspace"]),
            ("Delete a word", &["Alt+Backspace"]),
        ],
        Platform::Windows => &[
            ("Start or end of the line", &["Home", "End"]),
            ("Start or end of the document", &["Ctrl+Home", "Ctrl+End"]),
            ("A word back or on", &["Ctrl+Left", "Ctrl+Right"]),
            ("Delete a word", &["Ctrl+Backspace"]),
        ],
        Platform::Beos => &[
            ("Start or end of the line", &["Home", "End"]),
            ("Start or end of the document", &["Alt+Home", "Alt+End"]),
            ("A word back or on", &["Alt+Left", "Alt+Right"]),
            ("Delete a word", &["Alt+Backspace"]),
        ],
    };
    let rows = rows
        .iter()
        .map(|(title, keys)| {
            let keys = keys.iter().filter_map(|k| k.parse::<Chord>().ok()).map(|c| c.label(platform)).collect::<Vec<_>>().join("  ");
            ((*title).to_owned(), keys)
        })
        .chain(std::iter::once(("Select as you go: with any of these".to_owned(), if platform == Platform::Mac { "⇧" } else { "Shift" }.to_owned())))
        .collect();
    Group { title: "Text".into(), rows }
}

pub fn open(cx: &mut Cx, learned: Learned) -> Shown {
    let win = cx.open_window(WindowOptions {
        title: "Keyboard Shortcuts".into(),
        size: (1, 1),
        kind: WindowKind::Layer(LayerOptions {
            layer: Layer::Overlay,
            anchor: (true, true, true, true),
            exclusive_zone: -1,
            margin: (0, 0, 0, 0),
            keyboard: Keyboard::Exclusive,
            namespace: "shortcuts".into(),
            output: None,
        }),
        panel: false,
        ..Default::default()
    });
    Shown { win, profile: learned.profile, groups: learned.groups }
}

// Everything on one grid of rows, so the cards' lines line up across
// the columns: a heading takes a row, and the gap between cards one.
const COLUMN: f32 = 300.0;
const ROW: f32 = 20.0;
const GAP: f32 = ROW;
const HEADING: f32 = ROW;
const TITLE: f32 = 40.0;
const FOOT: f32 = 30.0;

/// Draw the sheet; true when it is to go away.
pub fn ui(s: &mut Shown, ui: &mut egui::Ui) -> bool {
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        return true;
    }
    let p = style::palette(ui.ctx());
    let screen = ui.max_rect();
    let room = vec2(screen.width() - 4.0 * GAP, screen.height() - 4.0 * GAP);
    let (cols, col_w) = masonry::columns(room.x.min(COLUMN * 4.0 + GAP * 5.0), COLUMN, GAP);
    let cols = cols.min(s.groups.len().max(1));
    let items: Vec<(usize, f32)> = s.groups.iter().map(|g| (1, HEADING + g.rows.len() as f32 * ROW)).collect();
    let (places, content_h) = masonry::arrange(cols, col_w, GAP, &items);
    let body_h = content_h.min(room.y - TITLE - FOOT).max(ROW);
    // Room for the scroll bar beside the cards if they don't all fit.
    let scrolls = content_h > body_h;
    let bar = if scrolls { widgets::SCROLL_BAR } else { 0.0 };
    let width = masonry::span_width(cols, col_w, GAP) + 2.0 * GAP + bar;
    let panel = Rect::from_center_size(screen.center(), vec2(width, TITLE + body_h + FOOT));
    let panel = style::snap(ui.ctx(), panel);

    // Outside the sheet: a click puts it away.
    let outside = ui.interact(screen, ui.make_persistent_id("sheet-outside"), Sense::click());
    if outside.clicked() && !outside.interact_pointer_pos().is_some_and(|pos| panel.contains(pos)) {
        return true;
    }

    let painter = ui.painter().clone();
    style::soft_shadow(&painter, panel, &p);
    painter.rect_filled(panel, 0.0, c(p.panel_background));
    style::frame_rect(&painter, panel, style::darken(p.panel_background, 1.6));
    style::raised(&painter, panel.shrink(1.0), &p);

    // The title, and whose keys these are.
    let (fg, dim) = (c(p.panel_text), style::disabled_text(&p));
    let title_y = panel.top() + TITLE / 2.0 + 2.0;
    style::label(&painter, panel.left() + GAP, title_y, Align::Min, "Keyboard Shortcuts", egui::FontId::new(15.0, style::bold()), fg);
    style::label(&painter, panel.right() - GAP, title_y, Align::Max, &format!("{} keys", s.profile), style::plain_font(), dim);

    // The cards, in a scroll view if they don't all fit.
    let body = Rect::from_min_size(pos2(panel.left() + 2.0, panel.top() + TITLE), vec2(width - 4.0, body_h));
    let draw = |ui: &mut egui::Ui| {
        let (area, _) = ui.allocate_exact_size(vec2(body.width() - bar, content_h - GAP), Sense::hover());
        let painter = ui.painter();
        // (From the panel's edge, so the cards line up with the title.)
        let origin = pos2(panel.left(), area.top() - GAP);
        let visible = ui.clip_rect();
        for (group, place) in s.groups.iter().zip(&places) {
            let r = place.translate(origin.to_vec2());
            if !r.intersects(visible) {
                continue;
            }
            style::label_fit(painter, r.left(), r.top() + HEADING / 2.0, Align::Min, &group.title, style::bold_font(), fg, r.width());
            for (i, (title, keys)) in group.rows.iter().enumerate() {
                let y = r.top() + HEADING + i as f32 * ROW + ROW / 2.0;
                let keys_w = style::key_combo_width(ui.ctx(), keys);
                style::key_combo(painter, pos2(r.right(), y), keys, fg);
                style::label_fit(painter, r.left(), y, Align::Min, title, style::plain_font(), fg, (r.width() - keys_w - m::SPACING).max(40.0));
            }
        }
    };
    if scrolls {
        ScrollView::new("shortcuts-sheet").background(p.panel_background).show_in(ui, body, |ui, _| draw(ui));
    } else {
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), draw);
    }

    // Where to change them.
    let foot_y = panel.bottom() - FOOT / 2.0;
    style::label(&painter, panel.left() + GAP, foot_y, Align::Min, "Change them in Preferences ▸ Keyboard ▸ Shortcuts.", style::small_font(), dim);
    let more = if scrolls { "Scroll for more · Esc closes" } else { "Esc closes" };
    style::label(&painter, panel.right() - GAP, foot_y, Align::Max, more, style::small_font(), dim);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_for_the_front_app_text_and_desktop() {
        let entry = |path: &str, label: &str, keys: Option<&str>| commands::Entry {
            path: vec![path.into()],
            label: label.into(),
            command: label.to_lowercase(),
            keys: keys.map(Into::into),
            enabled: true,
            checked: false,
        };
        let menus = commands::Menus {
            app_id: "olo.tracker".into(),
            entries: vec![entry("File", "Open", Some("⌘O")), entry("File", "Get Info…", Some("⌘I")), entry("Edit", "Select All", None)],
        };
        let mac = KeyboardProfile::default();
        let g = groups(&mac, Some(&menus));
        assert_eq!(g[0], Group { title: "Tracker: File".into(), rows: vec![("Open".into(), "⌘O".into()), ("Get Info".into(), "⌘I".into())] });
        assert_eq!(g[1].title, "Text");
        assert_eq!(g[1].rows[0], ("Start or end of the line".into(), "⌘←  ⌘→".into()));
        let switching = g.iter().find(|g| g.title == shortcuts::SWITCHING).unwrap();
        assert!(switching.rows.contains(&("Switch applications (Twitcher)".into(), "⌘⇥".into())));
        // Windows' names.
        let w = groups(&KeyboardProfile::builtin(Platform::Windows), None);
        assert_eq!(w[0].rows[0].1, "Home  End");
        let desktop = w.iter().find(|g| g.title == shortcuts::DESKTOP).unwrap();
        assert!(desktop.rows.contains(&("Keyboard shortcuts".into(), "Win+Shift+/".into())));
    }
}
