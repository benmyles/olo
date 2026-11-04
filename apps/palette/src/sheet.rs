//! The shortcuts sheet: every shortcut of the keyboard profile on one
//! sheet, over everything (Keyboard Shortcuts: ⇧⌘/ on a Mac, Win+Shift+/
//! on Windows, Alt+Shift+/ on BeOS, as a Mac's Help is ⌘?). app_server
//! starts `olo-palette --shortcuts`, which hands it to the running copy.
//! The front app's own come first (its menus' keys, as it shows them),
//! then text editing's, then the desktop's (app_server's), each group a
//! card in columns. Esc, a click outside or the keys again put it away.

use olo_kit_base::masonry;
use olo_protocol::{Chord, Client, KeyboardProfile, Platform, Reply, Request, shortcuts};
use olo_ui::{
    Frame, Proxy, agent, commands,
    gpui::{
        App, AppContext as _, Bounds, Context, FocusHandle, FontWeight, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent, MouseDownEvent, ParentElement as _, Render, ScrollHandle,
        Styled as _, Window, WindowHandle, canvas, div, point, px, size,
    },
    metrics as m,
    style::{self, c},
    theme,
    widgets::{SCROLL_BAR, ScrollView},
};
use olo_xdg::apps::our_app;

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
/// Text editing's keys as the person presses them with `platform`'s
/// profile (app_server's keymap turns them into what each program takes):
/// each a line of the sheet ("Start or end of the line") and what each of
/// its keys does, a command of its own in the palette ("Move to the end
/// of the line"; with Shift, "Select to …", which the sheet says once).
pub fn text_keys(platform: Platform) -> &'static [(&'static str, &'static [(&'static str, &'static str)])] {
    match platform {
        Platform::Mac => &[
            ("Start or end of the line", &[("Move to the start of the line", "Cmd+Left"), ("Move to the end of the line", "Cmd+Right")]),
            ("Start or end of the document", &[("Move to the start of the document", "Cmd+Up"), ("Move to the end of the document", "Cmd+Down")]),
            ("A word back or on", &[("Move a word back", "Alt+Left"), ("Move a word on", "Alt+Right")]),
            ("Delete to the line's start", &[("Delete to the start of the line", "Cmd+Backspace")]),
            ("Delete a word", &[("Delete the word before", "Alt+Backspace")]),
        ],
        Platform::Windows => &[
            ("Start or end of the line", &[("Move to the start of the line", "Home"), ("Move to the end of the line", "End")]),
            ("Start or end of the document", &[("Move to the start of the document", "Ctrl+Home"), ("Move to the end of the document", "Ctrl+End")]),
            ("A word back or on", &[("Move a word back", "Ctrl+Left"), ("Move a word on", "Ctrl+Right")]),
            ("Delete a word", &[("Delete the word before", "Ctrl+Backspace")]),
        ],
        Platform::Beos => &[
            ("Start or end of the line", &[("Move to the start of the line", "Home"), ("Move to the end of the line", "End")]),
            ("Start or end of the document", &[("Move to the start of the document", "Alt+Home"), ("Move to the end of the document", "Alt+End")]),
            ("A word back or on", &[("Move a word back", "Alt+Left"), ("Move a word on", "Alt+Right")]),
            ("Delete a word", &[("Delete the word before", "Alt+Backspace")]),
        ],
    }
}

fn text_editing(platform: Platform) -> Group {
    let rows = text_keys(platform)
        .iter()
        .map(|(line, keys)| {
            let keys = keys.iter().filter_map(|(_, k)| k.parse::<Chord>().ok()).map(|c| c.label(platform)).collect::<Vec<_>>().join("  ");
            ((*line).to_owned(), keys)
        })
        .chain(std::iter::once(("Select as you go: with any of these".to_owned(), if platform == Platform::Mac { "⇧" } else { "Shift" }.to_owned())))
        .collect();
    Group { title: "Text".into(), rows }
}

/// The sheet's window.
struct Sheet {
    profile: String,
    groups: Vec<Group>,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

/// Covers the screen with the sheet.
pub fn open(learned: Learned, cx: &mut App) -> Option<WindowHandle<Frame>> {
    let sheet = cx.new(|cx| Sheet { profile: learned.profile, groups: learned.groups, scroll: ScrollHandle::new(), focus: cx.focus_handle() });
    let focus = sheet.read(cx).focus.clone();
    olo_ui::open_window(cx, crate::overlay("Keyboard Shortcuts", "shortcuts", None), move |window, _| {
        window.focus(&focus);
        sheet
    })
}

// Everything on one grid of rows, so the cards' lines line up across
// the columns: a heading takes a row, and the gap between cards one.
const COLUMN: f32 = 300.0;
const ROW: f32 = 20.0;
const GAP: f32 = ROW;
const HEADING: f32 = ROW;
const TITLE: f32 = 40.0;
const FOOT: f32 = 30.0;

impl Render for Sheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = window.viewport_size();
        let room = (f32::from(screen.width) - 4.0 * GAP, f32::from(screen.height) - 4.0 * GAP);
        let (cols, col_w) = masonry::columns(room.0.min(COLUMN * 4.0 + GAP * 5.0), COLUMN, GAP);
        let cols = cols.min(self.groups.len().max(1));
        let items: Vec<(usize, f32)> = self.groups.iter().map(|g| (1, HEADING + g.rows.len() as f32 * ROW)).collect();
        let (places, content_h) = masonry::arrange(cols, col_w, GAP, &items);
        let body_h = content_h.min(room.1 - TITLE - FOOT).max(ROW);
        // Room for the scroll bar beside the cards if they don't all fit.
        let scrolls = content_h > body_h;
        let bar = if scrolls { SCROLL_BAR } else { 0.0 };
        let width = masonry::span_width(cols, col_w, GAP) + 2.0 * GAP + bar;
        let height = TITLE + body_h + FOOT;
        let panel = style::snap(window, Bounds { origin: point((screen.width - px(width)) / 2.0, (screen.height - px(height)) / 2.0), size: size(px(width), px(height)) });
        let profile = self.profile.clone();
        let window_handle = window.window_handle();

        // The panel, its title and foot; a click outside puts it away.
        let backdrop = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                style::soft_shadow(window, panel, &p);
                style::fill_rect(window, panel, c(p.panel_background));
                style::frame_rect(window, panel, style::tint(p.panel_background, 1.6));
                style::raised(window, style::inset(window, panel, 1.0), &p);
                let (fg, dim) = (c(p.panel_text), style::disabled_text(&p));
                let title_y = panel.top() + px(TITLE / 2.0 + 2.0);
                style::label(window, cx, panel.left() + px(GAP), title_y, style::Align::Left, "Keyboard Shortcuts", 15.0, style::ui_font(FontWeight::BOLD), fg);
                style::label(window, cx, panel.right() - px(GAP), title_y, style::Align::Right, format!("{profile} keys"), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                // Where to change them.
                let foot_y = panel.bottom() - px(FOOT / 2.0);
                let small = style::ui_font(FontWeight::NORMAL);
                style::label(window, cx, panel.left() + px(GAP), foot_y, style::Align::Left, "Change them in Preferences ▸ Keyboard ▸ Shortcuts.", style::DETAIL_SIZE, small.clone(), dim);
                let more = if scrolls { "Scroll for more · Esc closes" } else { "Esc closes" };
                style::label(window, cx, panel.right() - px(GAP), foot_y, style::Align::Right, more, style::DETAIL_SIZE, small, dim);
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble() && hitbox.is_hovered(window) && !panel.contains(&e.position) {
                        cx.close_window(window_handle);
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        // The cards (from the panel's edge, so they line up with the
        // title), in a scroll view if they don't all fit.
        let groups = self.groups.clone();
        let cards = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let fg = c(p.panel_text);
                let font = style::ui_font(FontWeight::NORMAL);
                for (group, place) in groups.iter().zip(&places) {
                    let r = Bounds { origin: point(panel.left() + px(place.x), b.top() - px(GAP) + px(place.y)), size: size(px(place.w), px(place.h)) };
                    let heading = Bounds { origin: r.origin, size: size(r.size.width, px(HEADING)) };
                    style::label_fit(window, cx, r.left(), r.top() + px(HEADING / 2.0), style::Align::Left, group.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), fg, r.size.width);
                    agent::record(window, cx, agent::Node { role: "Heading", name: Some(group.title.clone().into()), bounds: heading, enabled: true, ..Default::default() });
                    for (i, (title, keys)) in group.rows.iter().enumerate() {
                        let y = r.top() + px(HEADING + i as f32 * ROW + ROW / 2.0);
                        let keys_w = style::key_combo_width(window, keys);
                        style::key_combo(window, cx, point(r.right(), y), keys, fg);
                        style::label_fit(window, cx, r.left(), y, style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), fg, (r.size.width - px(keys_w + m::SPACING)).max(px(40.0)));
                        // For agents: each shortcut and its keys.
                        let row = Bounds { origin: point(r.left(), y - px(ROW / 2.0)), size: size(r.size.width, px(ROW)) };
                        agent::record(window, cx, agent::Node { role: "Row", name: Some(title.clone().into()), value: Some(keys.clone().into()), bounds: row, enabled: true, ..Default::default() });
                    }
                }
            },
        )
        .w(px(width - 4.0 - bar))
        .h(px(content_h - GAP));
        let body = div().absolute().left(panel.left() + px(2.0)).top(panel.top() + px(TITLE)).w(px(width - 4.0)).h(px(body_h));
        let body = if scrolls { body.child(ScrollView::new("shortcuts-sheet", &self.scroll).background(theme::palette(cx).panel_background).child(cards)) } else { body.child(cards) };
        div().size_full().relative().track_focus(&self.focus).child(backdrop).child(body).on_key_down(move |e: &KeyDownEvent, window, _| {
            if e.keystroke.key == "escape" {
                window.remove_window();
            }
        })
    }
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
        let menus = commands::Menus { app_id: "olo.tracker".into(), entries: vec![entry("File", "Open", Some("⌘O")), entry("File", "Get Info…", Some("⌘I")), entry("Edit", "Select All", None)] };
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
