//! The shortcuts sheet: every shortcut of the keyboard profile on one
//! sheet, over everything (Keyboard Shortcuts: ⇧⌘/ on a Mac, Win+Shift+/
//! on Windows, Alt+Shift+/ on BeOS, as a Mac's Help is ⌘?). app_server
//! starts `olo-palette --shortcuts`, which hands it to the running copy.
//! The front app's own come first (its menus' keys, as it shows them;
//! Chrome's, which has no menus to read, from the profile: `app_keys`'
//! Chrome table, sent as Chrome takes them by app_server), then text
//! editing's, then the desktop's (app_server's), each group a
//! card in columns. Esc, a click outside or the keys again put it away.

use olo_kit::{
    Bounds, Cx, Element, FocusHandle, FontWeight, InteractiveElement as _, KeyDownEvent, MouseButton, MouseDownEvent, Painter, ParentElement as _, Pixels, Proxy, ScrollHandle, Styled as _, View,
    WindowHandle, agent, commands, div, metrics as m, point, px, size,
    style::{self, c},
    theme,
    widgets::{SCROLL_BAR, ScrollView},
};
use olo_kit_base::masonry;
use olo_protocol::{Chord, Client, KeyboardProfile, Platform, Reply, Request, shortcuts};
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
        let app_id = front.as_ref().map(|w| w.app_id.clone());
        let menus = front.and_then(|w| w.pid).and_then(|p| u32::try_from(p).ok()).and_then(commands::menus);
        proxy.send(Learned { profile: profile.name.clone(), groups: groups(&profile, menus.as_ref(), app_id.as_deref()) });
    });
}

/// Chrome's keys on the sheet, by card: its title and the commands' ids.
const CHROME_CARDS: &[(&str, &[&str])] = &[
    ("Chrome: Tabs and Windows", &["chrome.new-tab", "chrome.reopen-tab", "chrome.close-tab", "chrome.new-window", "chrome.incognito", "chrome.close-window", "chrome.quit"]),
    ("Chrome: Going Places", &["chrome.location", "chrome.back", "chrome.forward", "chrome.home", "chrome.reload", "chrome.hard-reload", "chrome.find-next", "chrome.find-previous"]),
    ("Chrome: View", &["chrome.zoom-in", "chrome.zoom-out", "chrome.actual-size", "chrome.full-screen", "chrome.bookmarks-bar"]),
    ("Chrome: Bookmarks and History", &["chrome.bookmark", "chrome.bookmark-manager", "chrome.history", "chrome.downloads", "chrome.clear-data"]),
    ("Chrome: Developer", &["chrome.devtools", "chrome.console", "chrome.view-source"]),
];

/// The sheet's cards: the front app's menus (by menu; Chrome's keys for a
/// Chromium browser, `front` its app id), text editing, then app_server's
/// groups.
pub fn groups(profile: &KeyboardProfile, menus: Option<&commands::Menus>, front: Option<&str>) -> Vec<Group> {
    let mut out = Vec::new();
    if menus.is_none() && front.is_some_and(|a| olo_protocol::app_keys::CHROMIUM_APPS.contains(&a)) {
        for (title, ids) in CHROME_CARDS {
            let rows: Vec<(String, String)> = ids
                .iter()
                .filter_map(|id| {
                    let e = shortcuts::entry(id)?;
                    let keys = e.keys(profile);
                    (!keys.is_empty()).then(|| (e.title.to_owned(), keys.iter().map(|c| c.label(profile.platform)).collect::<Vec<_>>().join("  ")))
                })
                .collect();
            if !rows.is_empty() {
                out.push(Group { title: (*title).to_owned(), rows });
            }
        }
    }
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
pub struct Sheet {
    profile: String,
    groups: Vec<Group>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// Where it was last laid out: the panel, and each card in it.
    panel: Bounds<Pixels>,
    places: Vec<masonry::Place>,
    scrolls: bool,
}

/// Covers the screen with the sheet.
pub fn open(learned: Learned) -> WindowHandle<Sheet> {
    olo_kit::open_window(crate::overlay("Keyboard Shortcuts", "shortcuts", None), move |cx| {
        let focus = FocusHandle::new();
        focus.focus(cx);
        Sheet { profile: learned.profile, groups: learned.groups, scroll: ScrollHandle::new(), focus, panel: Bounds::default(), places: Vec::new(), scrolls: false }
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

impl Sheet {
    /// The panel, its title and foot.
    fn paint_panel(&self, g: &mut Painter) {
        let (panel, p) = (self.panel, theme::palette());
        style::soft_shadow(g, panel, &p);
        style::fill_rect(g, panel, c(p.panel_background));
        style::frame_rect(g, panel, style::tint(p.panel_background, 1.6));
        style::raised(g, style::inset(g, panel, 1.0), &p);
        let (fg, dim) = (c(p.panel_text), style::disabled_text(&p));
        let title_y = panel.top() + px(TITLE / 2.0 + 2.0);
        style::label(g, panel.left() + px(GAP), title_y, style::Align::Left, "Keyboard Shortcuts", 15.0, style::ui_font(FontWeight::BOLD), fg);
        style::label(g, panel.right() - px(GAP), title_y, style::Align::Right, format!("{} keys", self.profile), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
        // Where to change them.
        let foot_y = panel.bottom() - px(FOOT / 2.0);
        let small = style::ui_font(FontWeight::NORMAL);
        style::label(g, panel.left() + px(GAP), foot_y, style::Align::Left, "Change them in Preferences ▸ Keyboard ▸ Shortcuts.", style::DETAIL_SIZE, small.clone(), dim);
        let more = if self.scrolls { "Scroll for more · Esc closes" } else { "Esc closes" };
        style::label(g, panel.right() - px(GAP), foot_y, style::Align::Right, more, style::DETAIL_SIZE, small, dim);
    }

    /// The cards, from the panel's edge (so they line up with the title).
    fn paint_cards(&self, g: &mut Painter, b: Bounds<Pixels>) {
        let p = theme::palette();
        let fg = c(p.panel_text);
        let font = style::ui_font(FontWeight::NORMAL);
        for (group, place) in self.groups.iter().zip(&self.places) {
            let r = Bounds { origin: point(self.panel.left() + px(place.x), b.top() - px(GAP) + px(place.y)), size: size(px(place.w), px(place.h)) };
            let heading = Bounds { origin: r.origin, size: size(r.size.width, px(HEADING)) };
            style::label_fit(g, r.left(), r.top() + px(HEADING / 2.0), style::Align::Left, group.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), fg, r.size.width);
            agent::record(g, agent::Node { role: "Heading", name: Some(group.title.clone().into()), bounds: heading, enabled: true, ..Default::default() });
            for (i, (title, keys)) in group.rows.iter().enumerate() {
                let y = r.top() + px(HEADING + i as f32 * ROW + ROW / 2.0);
                let keys_w = style::key_combo_width(keys);
                style::key_combo(g, point(r.right(), y), keys, fg);
                style::label_fit(g, r.left(), y, style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), fg, (r.size.width - px(keys_w + m::SPACING)).max(px(40.0)));
                // For agents: each shortcut and its keys.
                let row = Bounds { origin: point(r.left(), y - px(ROW / 2.0)), size: size(r.size.width, px(ROW)) };
                agent::record(g, agent::Node { role: "Row", name: Some(title.clone().into()), value: Some(keys.clone().into()), bounds: row, enabled: true, ..Default::default() });
            }
        }
    }
}

impl View for Sheet {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let screen = cx.viewport_size();
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
        let panel = crate::snapped(cx.scale_factor(), Bounds { origin: point((screen.width - px(width)) / 2.0, (screen.height - px(height)) / 2.0), size: size(px(width), px(height)) });
        (self.panel, self.places, self.scrolls) = (panel, places, scrolls);

        // The panel, its title and foot; a click outside puts it away.
        let backdrop = cx.canvas(|s: &Sheet, g, _| s.paint_panel(g)).id("sheet-backdrop").absolute().left(px(0.0)).top(px(0.0)).size_full().on_mouse_down(
            MouseButton::Left,
            cx.listener(|s: &mut Self, e: &MouseDownEvent, cx| {
                if !s.panel.contains(&e.position) {
                    cx.close_window();
                }
            }),
        );
        // The cards, in a scroll view if they don't all fit.
        let cards = cx.canvas(|s: &Sheet, g, b| s.paint_cards(g, b)).w(px(width - 4.0 - bar)).h(px(content_h - GAP));
        let body = div().absolute().left(panel.left() + px(2.0)).top(panel.top() + px(TITLE)).w(px(width - 4.0)).h(px(body_h));
        let body = if scrolls { body.child(ScrollView::new(&self.scroll).border(false).background(theme::palette().panel_background).child(cards)) } else { body.child(cards) };
        div().size_full().relative().track_focus(&self.focus).child(backdrop).child(body).on_key_down(cx.listener(|_: &mut Self, e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                cx.close_window();
            }
        }))
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
        let g = groups(&mac, Some(&menus), Some("olo.tracker"));
        assert_eq!(g[0], Group { title: "Tracker: File".into(), rows: vec![("Open".into(), "⌘O".into()), ("Get Info".into(), "⌘I".into())] });
        assert_eq!(g[1].title, "Text");
        assert_eq!(g[1].rows[0], ("Start or end of the line".into(), "⌘←  ⌘→".into()));
        let switching = g.iter().find(|g| g.title == shortcuts::SWITCHING).unwrap();
        assert!(switching.rows.contains(&("Switch applications (Twitcher)".into(), "⌘⇥".into())));
        // Windows' names.
        let w = groups(&KeyboardProfile::builtin(Platform::Windows), None, None);
        assert_eq!(w[0].rows[0].1, "Home  End");
        let desktop = w.iter().find(|g| g.title == shortcuts::DESKTOP).unwrap();
        assert!(desktop.rows.contains(&("Keyboard shortcuts".into(), "Win+Shift+/".into())));
        // Chrome in front: its keys first, as the profile has them.
        let c = groups(&mac, None, Some("google-chrome"));
        assert_eq!(c[0].title, "Chrome: Tabs and Windows");
        assert_eq!(c[0].rows[0], ("New Tab".into(), "⌘T".into()));
        let tools = c.iter().find(|g| g.title == "Chrome: Bookmarks and History").unwrap();
        assert!(tools.rows.contains(&("History".into(), "⌘Y".into())), "{:?}", tools.rows);
        let w = groups(&KeyboardProfile::builtin(Platform::Windows), None, Some("chromium"));
        assert!(w.iter().any(|g| g.rows.contains(&("History".into(), "Ctrl+H".into()))));
        // Not for another browser.
        assert!(groups(&mac, None, Some("firefox")).iter().all(|g| !g.title.starts_with("Chrome")));
    }
}
