//! The History window: what came, unread first, newest first, in one of
//! four views (View menu, remembered in `notifications.json`). Rows are
//! dense: a small icon, the title with the app and time on the same line,
//! one line of the body under it; the selected one opens up to its whole
//! body and its actions. What is unread wears a dot (in the keyboard's
//! blue) and a bold title until it is clicked; what was read while the
//! window is open keeps its place until it closes. With fading on, each
//! fades as it ages, the newest strongest, back to full while selected
//! or under the pointer. The Cards view packs cards of different sizes
//! (`masonry`), each a card headed by its app and time, the dot by the
//! time while unread. A search field filters, and the keyboard works the
//! list (arrows, Return to open, Delete to forget, Escape).
//!
//! Each row's and card's text is laid out once for what it shows (its
//! note, the width, the scale and the look) and kept; a frame draws only
//! what is in view: each line and each card is a drawing of its own, kept
//! while what it shows is the same, and only those near what shows are in
//! the window at all (more come as it scrolls). Clicks are found by where
//! things are laid out.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

use olo_kit::{
    Bounds, Child, ClickEvent, Corners, Cx, Element, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, MenuBar, MouseButton, MouseDownEvent, MouseMoveEvent,
    Painter, ParentElement as _, Pixels, Point, ScrollHandle, Styled as _, View, WindowCx, agent, canvas, div, fill, menu, point, px, size,
    style::{self, Align},
    theme,
    widgets::{Button, CheckBox, LinkButton, SCROLL_BAR, ScrollView, StringView, TextControl, TextField, TextFieldEvent, text_field},
};
use olo_kit_base::{masonry, metrics as m};
use olo_protocol::NotificationSettings;
use olo_protocol::{CardSize, HistoryView, NoteStyle};

use crate::{Notifications, Shared, look, markup::Span, named_icons, now_secs, paint_mark, pictures, raise_app, server::Reason, store, web, with};

/// Above and below a row's text.
const ROW_PAD: f32 = 5.0;
/// The small icon in a row.
const SMALL_ICON: f32 = 16.0;
/// Where a row's text begins: the unread dot, then the icon.
const TEXT_X: f32 = ICON_X + SMALL_ICON + 7.0;
/// Where a row's icon begins (the dot is before it).
const ICON_X: f32 = 16.0;
/// A group's or a day's heading.
const HEADING: f32 = 22.0;
/// The status line along the bottom.
const FOOTER: f32 = 18.0;
/// A card's least width; its body's lines, one column wide and two.
const CARD_W: f32 = 176.0;
const CARD_ROWS: usize = 4;
const WIDE_ROWS: usize = 8;
/// A card's title row, from its top, as a banner's (whose it is and
/// when, a line under it), its app's mark on it, the room its × takes
/// beside the time, and the gap under the row.
const CARD_HEAD: f32 = 22.0;
const CARD_MARK: f32 = 16.0;
/// The clear edge round a 16-point app mark: set this much left of the
/// card's column, its drawing starts on it.
const ICON_MARGIN: f32 = 1.0;
const CARD_CLOSE_ROOM: f32 = 20.0;
const CARD_HEAD_GAP: f32 = 8.0;
/// A card's action buttons.
const CARD_BUTTON: f32 = 20.0;
/// Between a card's picture (its well) and its buttons: 8 points clear, as
/// a banner's between its picture and its buttons (at 12 they were further
/// from the picture they act on than from the next card).
const CARD_BUTTON_GAP: f32 = 8.0;
/// Room for the × at a row's right end: what's beside it (the app and
/// the time) ends this far in, `TIGHT` and a point clear of the ×
/// (three points clear before: the review, with the pointer on a row).
const CLOSE_W: f32 = 25.0;
/// The middle of that ×, from the row's right end: its own edge 6
/// points clear of the list's (3, and it sat against the scroll bar).
const CLOSE_AT: f32 = 12.0;
/// The faintest a notification draws, at the very end of its fade: still
/// legible while it is there to read and click, then gone.
const FAINTEST: f32 = 0.3;

/// How opaque a notification of fade `strength` (1 new, 0 gone) draws.
fn fade_opacity(strength: f32) -> f32 {
    FAINTEST + (1.0 - FAINTEST) * strength.clamp(0.0, 1.0)
}

/// What the History window remembers while it is open (and across a
/// reload, see `on_upgrade`).
#[derive(Default)]
pub struct HistoryState {
    /// The search, as typed.
    pub search: String,
    /// The selected note: its entry's `uid` (two entries, one put back
    /// beside one sent since, or two of the Cleared list, can have one
    /// number).
    pub selected: Option<u64>,
    /// The apps folded away in By App.
    pub folded: BTreeSet<String>,
    /// Read while the window is open: they keep their places among the
    /// unread until it closes, so nothing moves from under the pointer.
    pub held: HashSet<u32>,
    /// Bring the selection into view at the next frame.
    reveal: bool,
    /// Showing the archive (View ▸ Archived), not the history.
    pub archived: bool,
}

/// One line of the list.
enum Item {
    /// The unread ones' heading, and how many.
    Unread(usize),
    Day(String),
    Group {
        app: String,
        count: usize,
        members: Vec<usize>,
        open: bool,
    },
    /// The entry at `i`; `older` are the rest of its conversation.
    Note {
        i: usize,
        older: Vec<usize>,
        grouped: bool,
    },
}

/// An entry's menu (a right click on it); `archived`: one in the
/// Cleared list, which can be opened, put back in the list, or deleted.
fn entry_menu(e: &store::Entry, archived: bool) -> Menu {
    if archived {
        return Menu::new("Notification").item("Open", "open").item("Put Back", "put-back").separator().item("Delete", "forget");
    }
    let mut menu = Menu::new("Notification").item("Open", "open");
    // (Its actions as they can be chosen now: its app's while it's open
    // there, after those that run something of their own; the one set
    // apart, a Delete, last, by Clear, away from those.)
    let actions: Vec<(String, String)> = e.actions_now().into_iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(e.via.as_deref(), k)).collect();
    let is_apart = |k: &String| e.look.apart.as_ref() == Some(k);
    for (key, label) in actions.iter().filter(|(k, _)| !is_apart(k)) {
        menu = menu.item(label.clone(), format!("action:{key}"));
    }
    let menu = menu.separator().item(if e.unread { "Mark as Read" } else { "Mark as Unread" }, "toggle-read").separator();
    let mut menu = crate::note_menu(menu, Some(e.pinned)).separator();
    if let Some((key, label)) = actions.iter().find(|(k, _)| is_apart(k)) {
        menu = menu.item(label.clone(), format!("action:{key}"));
    }
    menu.item("Clear", "forget")
}

/// The local date of `secs`: (a key that differs from day to day, its
/// heading).
fn day(secs: u64) -> (i64, String) {
    let local = |t: u64| {
        // SAFETY: localtime_r writes into the zeroed struct we own.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&(t as libc::time_t), &mut tm);
            tm
        }
    };
    let (tm, now) = (local(secs), local(now_secs()));
    let key = |t: &libc::tm| t.tm_year as i64 * 400 + t.tm_yday as i64;
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let label = match key(&now) - key(&tm) {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        _ => {
            let d = format!("{}, {} {}", DAYS[tm.tm_wday.clamp(0, 6) as usize], MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday);
            if tm.tm_year == now.tm_year { d } else { format!("{d}, {}", tm.tm_year + 1900) }
        }
    };
    (key(&tm), label)
}

/// "14:05".
fn clock(secs: u64) -> String {
    // SAFETY: localtime_r writes into the zeroed struct we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&(secs as libc::time_t), &mut tm);
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// Whether `e` has `words` in its app, title or body.
fn found(e: &store::Entry, words: &str) -> bool {
    let words = words.trim().to_lowercase();
    words.split_whitespace().all(|w| [&e.app, &e.summary, &e.text].iter().any(|s| s.to_lowercase().contains(w)))
}

/// The lines of `view` for the entries at `shown` (unread first, the
/// first `unread` of them, then the rest; newest first in each).
fn items(entries: &[store::Entry], shown: &[usize], unread: usize, view: HistoryView, folded: &BTreeSet<String>, searching: bool) -> Vec<Item> {
    let mut out = Vec::new();
    match view {
        HistoryView::List => {
            if unread > 0 {
                // (Those read while the window is open stay under it, uncounted.)
                out.push(Item::Unread(shown[..unread].iter().filter(|&&i| entries[i].unread).count()));
                out.extend(shown[..unread].iter().map(|&i| Item::Note { i, older: Vec::new(), grouped: false }));
            }
            let mut last = None;
            for &i in &shown[unread..] {
                let (key, label) = day(entries[i].time);
                if last != Some(key) {
                    out.push(Item::Day(label));
                    last = Some(key);
                }
                out.push(Item::Note { i, older: Vec::new(), grouped: false });
            }
        }
        HistoryView::Cards => out.extend(shown.iter().map(|&i| Item::Note { i, older: Vec::new(), grouped: false })),
        HistoryView::Apps => {
            // In the order of each app's newest.
            let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
            for &i in shown {
                let app = &entries[i].app;
                match groups.iter_mut().find(|(a, _)| a == app) {
                    Some((_, members)) => members.push(i),
                    None => groups.push((app.clone(), vec![i])),
                }
            }
            for (app, members) in groups {
                let open = searching || !folded.contains(&app);
                out.push(Item::Group { app, count: members.len(), members: members.clone(), open });
                if open {
                    out.extend(members.into_iter().map(|i| Item::Note { i, older: Vec::new(), grouped: true }));
                }
            }
        }
        HistoryView::Conversations => {
            let mut threads: Vec<(usize, Vec<usize>)> = Vec::new();
            for &i in shown {
                let e = &entries[i];
                match threads.iter_mut().find(|(head, _)| entries[*head].app == e.app && entries[*head].summary == e.summary) {
                    Some((_, older)) => older.push(i),
                    None => threads.push((i, Vec::new())),
                }
            }
            out.extend(threads.into_iter().map(|(i, older)| Item::Note { i, older, grouped: false }));
        }
    }
    out
}

/// The app's name as shown.
fn app_name(e: &store::Entry) -> &str {
    if e.app.is_empty() { "Notification" } else { &e.app }
}

/// Whether a note reports a failure (its title starts with a stop sign).
fn failed(e: &store::Entry) -> bool {
    e.kind == store::KindName::Error
}

/// How many columns a card takes: as many as its app asks; else two for a
/// picture, a table or a long body, and for the newest unread one (when
/// there are three columns or more), so what's new stands out.
fn card_span(e: &store::Entry, cols: usize, newest_unread: bool) -> usize {
    if cols < 2 {
        return 1;
    }
    match e.look.size {
        CardSize::Large => 2,
        CardSize::Small | CardSize::Medium => 1,
        CardSize::Auto if e.look.image.is_some() || e.look.fields.len() >= 3 || e.text.chars().count() > 160 => 2,
        CardSize::Auto if newest_unread && cols >= 3 && !e.text.is_empty() => 2,
        CardSize::Auto => 1,
    }
}

/// A card face's colours: a banner's, on the panel's grey (the card is
/// the banner it was).
fn card_ink(p: &olo_theme::Palette, e: &store::Entry) -> look::Ink {
    look::Ink {
        text: style::c(p.panel_text),
        soft: style::c(p.panel_text),
        dim: style::secondary_text(p),
        link: style::c(p.keyboard_navigation),
        balloon: style::c(p.document_background),
        balloon_edge: style::tint(p.panel_background, 1.25),
        accent: e.look.accent.map(|[r, g, b]| [r, g, b, 255]),
        paper: style::c(p.panel_background),
    }
}

/// An entry's picture, from its file.
fn picture(e: &store::Entry) -> Option<pictures::Source> {
    e.picture.as_deref().map(|p| pictures::Source::File(p.into()))
}

/// What a note in the history shows (`lit`: the pointer over its large
/// picture, which can be dragged out).
fn entry_face(e: &store::Entry, lit: bool) -> look::Face<'_> {
    look::Face {
        title: if e.summary.is_empty() { app_name(e) } else { &e.summary },
        body: &e.body,
        look: &e.look,
        progress: e.progress.filter(|_| e.live),
        picture: picture(e),
        failed: failed(e),
        lit,
    }
}

/// Where a card's face begins, the card in `r` (the list's
/// coordinates), and how wide it is.
fn card_face_at(r: Bounds<Pixels>) -> (Point<Pixels>, f32) {
    let head = card_head(r);
    (point(head.left(), head.bottom() + px(CARD_HEAD_GAP)), f32::from(r.size.width) - 2.0 * m::CARD_INSET)
}

/// A heading's band: a shade darker than the list.
fn heading_fill(g: &mut Painter, p: &olo_theme::Palette, r: Bounds<Pixels>) {
    // (In the dark, the panel's colour, a step lighter than the rows: a
    // step darker there, as in the light, it was the rows' own, and the
    // list's sections ran together.)
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let (band, line) =
        if dark { (style::c(p.panel_background), style::shade(p, p.panel_background, 1.15)) } else { (style::tint(p.document_background, 1.04), style::tint(p.document_background, 1.1)) };
    g.paint_quad(fill(r, band));
    style::hline_above(g, r.left(), r.right(), r.bottom(), line);
}

/// The icon a note shows, in `r`: its sender's picture, round, for a
/// message; else its app's (or the one it names), or the generic app's,
/// as its banner's header shows it (not a warning or an info mark: the
/// title of a failure already starts with its stop sign).
fn paint_icon(g: &mut Painter, r: Bounds<Pixels>, e: &store::Entry, person: bool) {
    if person
        && e.look.style == NoteStyle::Message
        && let Some(src) = picture(e)
        && let Some(img) = pictures::get(g, &src, look::device(g, r), pictures::Shape::Cover)
    {
        let _ = g.paint_image(r, Corners::all(r.size.width / 2.0), img, 0, false);
        return;
    }
    let names = named_icons(e.icon.as_deref(), e.desktop_entry.as_deref(), &e.app);
    paint_mark(g, r, &e.app, e.look.accent, e.via.is_some(), &names);
}

/// The mark of what is unread: a dot in the colour of what the keyboard
/// is on (not the tab's yellow, which would repeat the window's own on
/// every note).
/// Red (`waiting`) where its app says something is waiting in it; blue,
/// news (as Deskbar's dot beside the app, and its Inbox widget's cards).
fn unread_dot(g: &mut Painter, center: Point<Pixels>, p: &olo_theme::Palette) {
    // (Blue as it is beside its app in Deskbar and on the bell: unread.
    // Never red: that is what waits in an app, which isn't a notification.)
    g.paint_sector(center, px(0.0), px(3.5), 0.0, std::f32::consts::TAU, style::c(p.notice));
}

/// A row of the strip of what waits in apps.
const WAITING_ROW: f32 = 24.0;

/// "Waiting in apps": what apps say is waiting in them, over the list
/// and apart from it (they aren't notifications: `crate::waiting`). A
/// heading, then a row an app: a red dot, the app's icon and name, and
/// how much ("3 waiting"); a click goes to the app. Where the app says
/// what waits where (Terminal's tabs), a row for each instead: what it is
/// and where, the app's name at the end, a click going there. Nothing
/// here is selected, read or cleared: each is there while its app says
/// so.
fn waiting_strip(cx: &mut Cx<HistoryWindow>, waiting: &[crate::waiting::Waiting]) -> Element {
    // On the window's own ground, over the list's well (it isn't of the
    // list), its dots, icons and words in line with the list's: the
    // well's edge in from the window's, at every scale.
    let scale = cx.scale_factor();
    let edge = style::line_pixels(scale) * 2.0 / scale;
    let mut strip = div().w_full().flex().flex_col().flex_shrink_0().px(px(edge)).pb(px(m::TIGHT)).child(
        canvas(|g, r| {
            let p = theme::palette();
            let t = style::label(g, r.left() + px(ICON_X), r.center().y, Align::Left, "Waiting in apps", style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::secondary_text(&p));
            agent::record(g, agent::Node::label("Waiting in apps", t));
        })
        .w_full()
        .h(px(HEADING)),
    );
    for w in waiting {
        for x in &w.waits {
            let (id, name, app, what, place, said) = (x.id, w.name.clone(), w.app.clone(), x.waits.what.clone(), x.waits.place.clone(), x.said());
            strip = strip.child(
                canvas(move |g, r| {
                    let p = theme::palette();
                    g.note_pointer_edge(r);
                    let cy = r.center().y;
                    g.paint_sector(point(r.left() + px(9.0), cy), px(0.0), px(3.5), 0.0, std::f32::consts::TAU, style::c(p.unread));
                    let icon = style::snap_centred(g, point(r.left() + px(ICON_X + SMALL_ICON / 2.0), cy), SMALL_ICON, SMALL_ICON);
                    paint_mark(g, icon, &name, None, false, &named_icons(None, Some(&app), &name));
                    // What it is, then where (quieter), the app at the end,
                    // all on one baseline.
                    let base = style::baseline_at(g, cy, style::PLAIN_SIZE);
                    let whose =
                        style::label_on_baseline(g, r.right() - px(CLOSE_W), base, Align::Right, name.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
                    let end = whose.left() - px(m::SPACING);
                    let it = style::label_on_baseline_fit(
                        g,
                        r.left() + px(TEXT_X),
                        base,
                        Align::Left,
                        what.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::c(p.panel_text),
                        (end - r.left() - px(TEXT_X)).max(px(0.0)),
                    );
                    let room = end - it.right() - px(m::SPACING);
                    if !place.is_empty() && room > px(m::SPACING * 2.0) {
                        style::label_on_baseline_fit(
                            g,
                            it.right() + px(m::SPACING),
                            base,
                            Align::Left,
                            place.clone(),
                            style::DETAIL_SIZE,
                            style::ui_font(FontWeight::NORMAL),
                            style::secondary_text(&p),
                            room,
                        );
                    }
                    // (To an agent: what it says, and that a click goes there.)
                    agent::record(g, agent::Node { role: "Button", name: Some(format!("{said} ({name})").into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
                })
                .w_full()
                .h(px(WAITING_ROW))
                .id(("waiting", id as usize))
                .on_click(cx.listener(move |h: &mut HistoryWindow, _: &ClickEvent, _| with(&h.shared, |n| n.open_waiting(id)))),
            );
        }
        if !w.waits.is_empty() {
            continue;
        }
        let (id, name, app, short, said) = (w.id, w.name.clone(), w.app.clone(), w.short(), w.said());
        strip = strip.child(
            canvas(move |g, r| {
                let p = theme::palette();
                g.note_pointer_edge(r);
                let cy = r.center().y;
                g.paint_sector(point(r.left() + px(9.0), cy), px(0.0), px(3.5), 0.0, std::f32::consts::TAU, style::c(p.unread));
                let icon = style::snap_centred(g, point(r.left() + px(ICON_X + SMALL_ICON / 2.0), cy), SMALL_ICON, SMALL_ICON);
                paint_mark(g, icon, &name, None, false, &named_icons(None, Some(&app), &name));
                // (Its name and how much on one baseline.)
                let base = style::baseline_at(g, cy, style::PLAIN_SIZE);
                let how = style::label_on_baseline(g, r.right() - px(CLOSE_W), base, Align::Right, short.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
                let room = (how.left() - px(m::SPACING) - r.left() - px(TEXT_X)).max(px(0.0));
                style::label_on_baseline_fit(g, r.left() + px(TEXT_X), base, Align::Left, name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text), room);
                // (To an agent: what it says, and that a click opens the app.)
                agent::record(g, agent::Node { role: "Button", name: Some(said.clone().into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
            })
            .w_full()
            .h(px(WAITING_ROW))
            .id(("waiting", id as usize))
            .on_click(cx.listener(move |h: &mut HistoryWindow, _: &ClickEvent, _| with(&h.shared, |n| n.open_waiting(id)))),
        );
    }
    strip.into_element()
}

/// A row laid out.
#[derive(Clone)]
struct RowLaid {
    title: look::Text,
    body: Option<look::Text>,
    /// The rest of a conversation: when, and a line of each.
    older: Vec<(String, look::Text)>,
    /// Its buttons, when selected: (key, label, width), and the key of
    /// the one set apart at the row's far end (a Delete).
    buttons: Vec<(String, String, f32)>,
    apart: Option<String>,
    height: f32,
}

/// What a row's layout depends on.
#[derive(Clone, PartialEq)]
struct RowKey {
    entry: store::Entry,
    older: Vec<u32>,
    meta: String,
    selected: bool,
    indent: bool,
    width: u32,
    scale: u32,
    dark: bool,
    /// In the Cleared list (its buttons are that list's).
    archived: bool,
}

/// A card measured: how many columns it spans, how tall it is, its face.
#[derive(Clone)]
struct CardLaid {
    /// Made anew when what it shows changes (in its drawing's key).
    generation: u64,
    height: f32,
    face: look::Laid,
    ink: look::Ink,
    /// Its app's actions, while it's open there: (key, label, width);
    /// and the key of the one set apart at its start (a Delete).
    buttons: Vec<(String, String, f32)>,
    apart: Option<String>,
}

#[derive(Clone, PartialEq)]
struct CardKey {
    entry: store::Entry,
    span: usize,
    width: u32,
    scale: u32,
    dark: bool,
    /// In the Cleared list (its buttons are that list's).
    archived: bool,
}

/// Something that takes a click, where it was drawn.
#[derive(Clone, PartialEq)]
enum Hit {
    Row(usize),
    Forget(usize),
    Fold(String),
    /// A group's Dismiss: those of its notes that may go.
    Dismiss(String, Vec<usize>),
    MarkAllRead,
    Card(usize),
    /// A card's large picture, which drags out as its file (a click on
    /// it, let go of where it was pressed, is the card's).
    Picture(usize),
}

/// What a line of the list is, placed.
struct Line {
    item: Item,
    y: f32,
    h: f32,
}

/// The list laid out, kept from frame to frame (while it scrolls, as the
/// pointer moves over it) until what it shows changes: two hundred notes
/// laid out again for every frame of a scroll took 1–3 ms of each.
struct ListLaid {
    /// A number of its own (in each line's drawing's key: laid out again,
    /// it's drawn again).
    generation: u64,
    /// How notes fade (`NotificationSettings::strength`).
    settings: NotificationSettings,
    entries: std::sync::Arc<Vec<store::Entry>>,
    view: HistoryView,
    cards_view: bool,
    searching: bool,
    /// The notes in the order they're listed (as `entries` indices).
    order: Rc<Vec<usize>>,
    lines: Rc<Vec<(Item, f32, f32)>>,
    rows: Rc<HashMap<usize, Rc<RowLaid>>>,
    cards: Rc<Vec<(usize, Rc<CardLaid>, masonry::Place)>>,
    total: f32,
    /// The selected entry's `uid`.
    sel_id: Option<u64>,
    /// The selected row's index, top and the rest of its conversation.
    sel_row: Option<(usize, f32, Vec<usize>)>,
    /// How many show, of how many; unread; snoozed.
    shown: usize,
    all: usize,
    unread: u32,
    snoozed: usize,
    can_clear: bool,
    /// How many the other list has (the archive's, or, showing the
    /// archive, the notifications'): the foot's link says.
    other: usize,
}

/// What the list was laid out for, besides the notes.
#[derive(PartialEq)]
struct LaidKey {
    width: u32,
    scale: u32,
    palette: std::sync::Arc<olo_theme::Palette>,
}

/// What an entry's menu was opened on, held till its command is chosen:
/// the entries of its conversation as drawn then (what Delete or Clear
/// takes, never what a note come since made of it), and whether it was
/// the Cleared list (a command chosen after the list changed is no one's).
#[derive(Clone)]
pub struct Menued {
    members: Vec<store::EntryKey>,
    archived: bool,
}

/// What the History window is told.
pub enum Told {
    /// The model changed.
    Changed,
    /// An entry's menu's command, for that very entry (as it was drawn:
    /// two in the Cleared list, or one put back beside one sent again
    /// since, can have the one number), with what its menu was opened on:
    /// its conversation's entries then, and which list (`Menued`).
    Command(store::EntryKey, String, Menued),
    /// That entry's picture, dragged out of the list (`archived`: the
    /// Cleared one), was dropped as this (None: nowhere).
    Dropped(store::EntryKey, bool, Option<olo_kit::DragAction>),
}

pub struct HistoryWindow {
    shared: Shared,
    search: Child<TextField>,
    scroll: ScrollHandle,
    /// The list's keys (the arrows, Return, Delete), when the search
    /// field hasn't the keyboard.
    focus: FocusHandle,
    /// Laid out rows and cards, by their entries' `uid`.
    rows: HashMap<u64, (RowKey, Rc<RowLaid>)>,
    cards: HashMap<u64, (CardKey, Rc<CardLaid>)>,
    /// The palette painted with, and a number that changes with it (in
    /// the drawings' keys).
    palette: Option<(std::sync::Arc<olo_theme::Palette>, u64)>,
    /// What the pointer is over.
    hover: Option<Hit>,
    /// Where each card was placed (the arrow keys go by it), by entry.
    places: Vec<(u64, masonry::Place)>,
    laid: Option<(LaidKey, Rc<ListLaid>)>,
    /// How many times the list has been laid out.
    laid_count: u64,
    /// The notes, the view or the selection changed since: lay it out
    /// again.
    stale: bool,
    /// The list's width, as last laid out.
    width: f32,
    /// The span of the list (top, bottom) whose lines and cards are in
    /// the window: more are put in as it scrolls near either end.
    built: (f32, f32),
    /// A card's picture pressed: its entry as it was drawn, and where
    /// (the window's points).
    held_picture: Option<(store::EntryKey, Point<Pixels>)>,
}

/// How far a press on a picture moves before it's a drag (points).
const DRAG_AFTER: f32 = 4.0;

/// The History window's views: View menu label, command and key.
const VIEWS: [(HistoryView, &str, &str, &str); 4] = [
    (HistoryView::List, "List", "view-list", "1"),
    (HistoryView::Apps, "By App", "view-apps", "2"),
    (HistoryView::Cards, "Cards", "view-cards", "3"),
    (HistoryView::Conversations, "Conversations", "view-conversations", "4"),
];

/// A drawing's key: what it shows, hashed.
fn key_of(what: impl std::hash::Hash) -> u64 {
    style::key(what)
}

/// "Mark All Read"'s place in the Unread heading `r`: its right edge on
/// the column under it, where the rows' times and an open row's last
/// button end (as a control, to agents and the layout review, too: a
/// margin past its words put its edge 4 points off theirs).
fn mark_all_rect(r: Bounds<Pixels>) -> Bounds<Pixels> {
    let tw = f32::from(style::text_width("Mark All Read", style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL)));
    let right = r.right() - px(CLOSE_W);
    Bounds::from_corners(point(right - px(tw + 4.0), r.top() + px(2.0)), point(right, r.bottom() - px(3.0)))
}

/// What a × or a group's link does, in a word: clears a notification
/// (out of the list, into the Cleared one); deletes one that's there.
fn clears(e: Option<&store::Entry>) -> &'static str {
    if e.is_some_and(|e| e.archived > 0) { "Delete" } else { "Clear" }
}

/// A group's Clear, at the right of its heading `r`.
fn dismiss_rect(r: Bounds<Pixels>) -> Bounds<Pixels> {
    let tw = f32::from(style::text_width("Delete", style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL)));
    Bounds::from_corners(point(r.right() - px(8.0 + tw + 6.0), r.top() + px(3.0)), point(r.right() - px(4.0), r.bottom() - px(4.0)))
}

/// `w` rounded up to whole even points, as a button rounds its own
/// width (`Button`'s `min_width`).
fn even(w: f32) -> f32 {
    (w / 2.0).ceil() * 2.0
}

/// The middle of a row's first line (its title's capitals), the row's
/// top at `top`: the dot, the icon and the × centre on it.
fn first_line(laid: &RowLaid, top: Pixels) -> Pixels {
    laid.title.baseline(top + px(ROW_PAD)) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0)
}

/// A row's × (Forget), in the row `r`.
fn row_close(laid: &RowLaid, r: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::centered_at(point(r.right() - px(CLOSE_AT), first_line(laid, r.top())), size(px(12.0), px(12.0)))
}

/// A card's title row (whose it is and when), in the card `r`: its
/// words set in by the card's inset, as a banner's by its own.
fn card_head(r: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(r.origin + point(px(m::CARD_INSET), px(0.0)), size(r.size.width - px(2.0 * m::CARD_INSET), px(CARD_HEAD)))
}

/// A card's × (Forget), beside its time, at the row's end.
fn card_close(r: Bounds<Pixels>) -> Bounds<Pixels> {
    let head = card_head(r);
    Bounds::centered_at(point(head.right() - px(look::CLOSE / 2.0), head.center().y), size(px(12.0), px(12.0)))
}

/// A row's or card's line under its title: whose it is and when; in
/// the Cleared list, whose it is, and why it left and when ("Telegram ·
/// Withdrawn 4:41 PM": it's under the day it came, and short enough to
/// sit beside its title).
fn meta_of(e: &store::Entry, view: HistoryView) -> String {
    if e.archived > 0 {
        // (Its app is said already: "Withdrawn", not "Withdrawn by …".)
        let why = format!("{} {}", e.why.map_or_else(|| "Cleared".to_owned(), |w| w.said("")), crate::when(e.archived));
        return if view == HistoryView::Apps { why } else { format!("{} · {why}", app_name(e)) };
    }
    let meta = match view {
        HistoryView::List => format!("{} · {}", app_name(e), clock(e.time)),
        HistoryView::Apps => crate::when(e.time),
        _ => format!("{} · {}", app_name(e), crate::when(e.time)),
    };
    if e.pinned { format!("{meta} · Pinned") } else { meta }
}

impl HistoryWindow {
    pub fn new(shared: Shared, cx: &mut Cx<Self>) -> HistoryWindow {
        let typed = shared.lock().unwrap().view.search.clone();
        let search = text_field(cx, |f| f.name("Search").hint("Search").with_text(typed));
        cx.subscribe(&search, |me: &mut HistoryWindow, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Changed) {
                let text = me.search.read(cx).text().to_owned();
                with(&me.shared, |n| {
                    n.view.search = text;
                    n.changed();
                });
            }
        });
        // (The list has the keyboard: the arrows work it.)
        let focus = FocusHandle::new();
        focus.focus(cx);
        HistoryWindow {
            shared,
            search,
            scroll: ScrollHandle::new(),
            focus,
            rows: HashMap::new(),
            cards: HashMap::new(),
            palette: None,
            hover: None,
            places: Vec::new(),
            laid: None,
            laid_count: 0,
            stale: true,
            width: 0.0,
            built: (0.0, 0.0),
            held_picture: None,
        }
    }

    fn menus(&self) -> Vec<Menu> {
        let n = self.shared.lock().unwrap();
        let view = n.settings.view;
        let mut views = Menu::new("View");
        for (v, label, command, key) in VIEWS {
            views = views.item(label, command).key(key).checked(view == v);
        }
        // (The Cleared list beside the list's views: what has left it.)
        let archived = n.view.archived;
        views = views.separator().item("Cleared", "archived").key("5").checked(archived);
        let selected = n.selected_unread();
        let pinned = n.selected_entry().filter(|_| !archived).map(|e| e.pinned);
        let edit = n
            .steps
            .edit_items(Menu::new("Edit"))
            .separator()
            .item("Find…", "find")
            .key("f")
            .separator()
            .item(if selected == Some(false) { "Mark as Unread" } else { "Mark as Read" }, "toggle-read")
            .key("u")
            .enabled(selected.is_some())
            .item("Mark All as Read", "read-all")
            .enabled(!archived && n.history.unread() > 0)
            .separator();
        let edit = crate::note_menu(edit, pinned).separator();
        let edit = if archived {
            edit.item("Delete All", "empty-archive").enabled(!n.archive.entries.is_empty())
        } else {
            edit.item("Clear All", "clear").enabled(n.history.entries.iter().any(|e| !e.shown && !e.pinned && !e.snoozed()))
        };
        // (The service stays running: Close, no Quit.)
        vec![Menu::new("File").app_items(&["notifications"]).close_item(), edit, views]
    }

    fn do_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        self.stale = true;
        let shared = self.shared.clone();
        match command {
            "close" => {
                with(&shared, |n| n.history_closed());
                cx.close_window();
            }
            "clear" => with(&shared, |n| n.clear_history()),
            "archived" => with(&shared, |n| {
                let on = !n.view.archived;
                n.show_archived(on);
            }),
            "empty-archive" => with(&shared, |n| n.delete_archived(None)),
            "undo" => with(&shared, |n| n.undo_redo(false)),
            "redo" => with(&shared, |n| n.undo_redo(true)),
            "find" => self.search.focus(cx),
            "toggle-read" => {
                with(&shared, |n| n.toggle_read());
            }
            "read-all" => with(&shared, |n| {
                let unread: Vec<u32> = n.history.entries.iter().filter(|e| e.unread).map(|e| e.id).collect();
                n.view.held.extend(unread);
                n.mark_all_read();
            }),
            // The selected note's.
            c if c == "pin" || c.starts_with("snooze:") => with(&shared, |n| {
                if let Some(id) = n.selected_entry().filter(|_| !n.view.archived).map(|e| e.id) {
                    n.note_command(id, c);
                }
            }),
            _ => {
                if let Some((v, ..)) = VIEWS.iter().find(|(_, _, c, _)| *c == command) {
                    let v = *v;
                    with(&shared, |n| {
                        n.set_view(v);
                        n.changed();
                    });
                }
            }
        }
        cx.invalidate();
    }

    /// A row laid out for `width` (kept while what it shows is the same).
    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        cx: &WindowCx,
        p: &olo_theme::Palette,
        entries: &[store::Entry],
        i: usize,
        older: &[usize],
        meta: String,
        selected: bool,
        indent: bool,
        width: f32,
        archived: bool,
    ) -> Rc<RowLaid> {
        let e = &entries[i];
        // (Compared with the note as it is, not a copy; laid out again for
        // another width or look, the note kept in the key stays.)
        let (width_key, scale, dark) = (width as u32, (cx.scale_factor() * 1000.0) as u32, p.mode == olo_theme::ThemeMode::Dark);
        let older_ids = || older.iter().map(|&o| entries[o].id);
        let same_note = match self.rows.get(&e.uid) {
            Some((k, laid)) if k.entry == *e && k.meta == meta && k.archived == archived && k.older.iter().copied().eq(older_ids()) => {
                if (k.selected, k.indent, k.width, k.scale, k.dark) == (selected, indent, width_key, scale, dark) {
                    return laid.clone();
                }
                true
            }
            _ => false,
        };
        let (fg, bg) = if selected { (p.list_selected_item_text, p.list_selected_background) } else { (p.document_text, p.document_background) };
        let soft = style::c(olo_theme::mix(fg, bg, 0.2));
        let x0 = if indent { 8.0 } else { 0.0 };
        let text_x = x0 + TEXT_X;
        let meta_w = f32::from(style::text_width(meta.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let title_w = (width - text_x - CLOSE_W - meta_w - 8.0).max(40.0);
        let body_w = width - text_x - CLOSE_W;
        let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
        // Unread: bold, as in a mailbox (and only then: selected, a read
        // one stays as it was, bold saying only "unread").
        let weight = if e.unread { FontWeight::BOLD } else { FontWeight::NORMAL };
        let title = look::title(cx, title_text, failed(e), style::PLAIN_SIZE, style::ui_font(weight), style::c(fg), title_w, if selected { 3 } else { 1 });
        let body = (!e.text.is_empty()).then(|| {
            // Unselected, the body is one line: its breaks as spaces.
            let spans: Vec<Span> =
                e.body.iter().map(|s| Span { text: if selected { s.text.clone() } else { s.text.replace('\n', " ") }, bold: s.bold, italic: s.italic, underline: false, link: None }).collect();
            let ink = look::Ink { text: style::c(fg), soft, dim: soft, link: soft, balloon: soft, balloon_edge: soft, accent: None, paper: style::c(bg) };
            // A file's details ("1280 × 800 · 29 KB") quieter than its
            // name, in the details' size and ink, as on its banner and its
            // card (as loud as the name, and darker than whose it is).
            if about_file(e) {
                look::body_in(cx, &spans, row_dim(fg, bg, selected), style::DETAIL_SIZE, &ink, body_w, if selected { 12 } else { 1 })
            } else {
                look::body(cx, &spans, &ink, body_w, if selected { 12 } else { 1 })
            }
        });
        // The rest of a conversation, under the newest, when selected.
        let older: Vec<(String, look::Text)> = if selected {
            older
                .iter()
                .map(|&o| (crate::when(entries[o].time), look::text(cx, &entries[o].text.replace('\n', " "), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), soft, body_w - 44.0, 1)))
                .collect()
        } else {
            Vec::new()
        };
        let (buttons, apart) = if !selected {
            (Vec::new(), None)
        } else {
            // (Opening it, its default, is no button: a double-click or
            // Return does it, as a card's click does; its menu says Open.
            // As on a card: `Delete ··· Copy · Show in Tracker`.)
            let actions: Vec<(String, String)> = if archived {
                // In the Cleared list, as its menu there: back in the
                // list, or gone for good (set apart).
                [("put-back", "Put Back"), ("forget", "Delete")].map(|(k, l)| (k.to_owned(), l.to_owned())).to_vec()
            } else {
                // Its app's actions while it's open there; after, those
                // that run something of their own (`x-olo-runs`: a
                // screenshot's Copy and Delete).
                e.actions_now().into_iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(e.via.as_deref(), k)).collect()
            };
            // (The row's × clears it: no Clear among them.) The one that
            // stands apart (a Delete) goes at the row's far end.
            let apart = if archived { Some("forget".to_owned()) } else { e.look.apart.clone().filter(|k| actions.iter().any(|(a, _)| a == k)) };
            let buttons = actions
                .into_iter()
                // (Whole even points, as a button rounds its own width:
                // an odd one came out a point wider than laid out, and
                // the gap after it a point short. Measured as it reads,
                // in the person's language.)
                .map(|(k, l)| {
                    // (As a card's: small, its label the details' size;
                    // one window, one size for the same three buttons.)
                    let w = even((f32::from(style::text_width(olo_kit::words::of_text(&l), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL))) + 16.0).max(56.0));
                    (k, l, w)
                })
                .collect();
            (buttons, apart)
        };
        let mut h = ROW_PAD + title.height + body.as_ref().map_or(0.0, |b| row_body_gap(e) + b.height);
        let older_h = style::SMALL_SIZE + 5.0;
        if !older.is_empty() {
            h += m::TIGHT + older.len() as f32 * older_h;
        }
        if selected && !buttons.is_empty() {
            h += m::TIGHT + CARD_BUTTON + 2.0;
        }
        // On the 2-point grid (whole device pixels at 1.5×), so the rows
        // under it start on one too.
        let height = ((h + ROW_PAD).max(ROW_PAD * 2.0 + SMALL_ICON + 4.0) / 2.0).ceil() * 2.0;
        let laid = Rc::new(RowLaid { title, body, older, buttons, apart, height });
        match self.rows.get_mut(&e.uid) {
            Some((k, kept)) if same_note => {
                (k.selected, k.indent, k.width, k.scale, k.dark) = (selected, indent, width_key, scale, dark);
                *kept = laid.clone();
            }
            _ => {
                let key = RowKey { entry: e.clone(), older: older_ids().collect(), meta, selected, indent, width: width_key, scale, dark, archived };
                self.rows.insert(e.uid, (key, laid.clone()));
            }
        }
        laid
    }

    /// A card measured `width` wide. (Its key is compared with the note as
    /// it is, not a copy of it; laid out again for another width, the
    /// note kept in the key stays; the card is shared, not copied.)
    fn card(&mut self, cx: &WindowCx, p: &olo_theme::Palette, e: &store::Entry, span: usize, width: f32, archived: bool) -> Rc<CardLaid> {
        let (width_key, scale, dark) = (width as u32, (cx.scale_factor() * 1000.0) as u32, p.mode == olo_theme::ThemeMode::Dark);
        let same_note = match self.cards.get(&e.uid) {
            Some((k, laid)) if k.entry == *e && k.archived == archived => {
                if (k.span, k.width, k.scale, k.dark) == (span, width_key, scale, dark) {
                    return laid.clone();
                }
                true
            }
            _ => false,
        };
        let ink = card_ink(p, e);
        let small = e.look.size == CardSize::Small;
        let room = look::Room {
            width: width - 2.0 * m::CARD_INSET,
            title_rows: if span > 1 { 2 } else { 3 },
            body_rows: if small {
                1
            } else if span > 1 {
                WIDE_ROWS
            } else {
                CARD_ROWS
            },
            title_reserve: 0.0,
            picture: 28.0,
            // (Its well 120 points at most, whatever the card's width: one
            // card no taller than another for the shape of its picture.)
            image_max: 120.0,
            fields_max: if span > 1 { 8 } else { 4 },
            // (On the text's edge, where its buttons start: in a wide
            // card, centred, it lined up with nothing.)
            image_start: true,
            // Its picture in a sunken well across the card, fitted and
            // centred there: a wide one, a tall one and a strip alike one
            // block, not a picture against the left and a hole beside it.
            well: true,
        };
        let face = look::lay(cx, &entry_face(e, false), &ink, &room);
        // (Its app's actions while it's open there; after, those that run
        // something of their own. In the Cleared list, that list's: back
        // in the list, or gone for good; a click opens it.)
        let actions: Vec<(String, String)> = if archived {
            [("put-back", "Put Back"), ("forget", "Delete")].map(|(k, l)| (k.to_owned(), l.to_owned())).to_vec()
        } else {
            e.actions_now().into_iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(e.via.as_deref(), k)).collect()
        };
        let apart = if archived { Some("forget".to_owned()) } else { e.look.apart.clone().filter(|k| actions.iter().any(|(a, _)| a == k)) };
        let buttons: Vec<(String, String, f32)> = actions
            .into_iter()
            .map(|(k, l)| {
                // (Its whole width, whatever the card's: one wider than
                // the card is cut short as it reads where it's placed,
                // `lay_list_buttons`; capped here, its whole label made the
                // button wider than its place, over the next card.)
                let w = even((f32::from(style::text_width(olo_kit::words::of_text(&l), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL))) + 16.0).max(56.0));
                (k, l, w)
            })
            .collect();
        let mut height = CARD_HEAD + CARD_HEAD_GAP + face.height + m::CARD_INSET;
        if !buttons.is_empty() {
            height += CARD_BUTTON_GAP + CARD_BUTTON;
        }
        static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let generation = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let laid = Rc::new(CardLaid { generation, height: height.ceil(), face, ink, buttons, apart });
        match self.cards.get_mut(&e.uid) {
            Some((k, kept)) if same_note => {
                (k.span, k.width, k.scale, k.dark) = (span, width_key, scale, dark);
                *kept = laid.clone();
            }
            _ => {
                self.cards.insert(e.uid, (CardKey { entry: e.clone(), span, width: width_key, scale, dark, archived }, laid.clone()));
            }
        }
        laid
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let Some(list) = self.laid.as_ref().map(|(_, l)| l.clone()) else { return };
        let (order, cards) = (list.order.clone(), list.cards_view);
        self.stale = true;
        let key = e.keystroke.key.as_str();
        if e.keystroke.modifiers.modified() {
            return;
        }
        let typing = self.search.is_focused(cx);
        let shared = self.shared.clone();
        // (The list as drawn: what's acted on is found again by its key as
        // it's done, never by a place in a list that has changed since.)
        let drawn = self.drawn();
        let entries_uids: Vec<u64> = drawn.iter().map(|k| k.uid).collect();
        let (selected, by_app): (Option<usize>, bool) = {
            let n = shared.lock().unwrap();
            (n.view.selected.and_then(|uid| entries_uids.iter().position(|&x| x == uid)), n.settings.view == HistoryView::Apps)
        };
        let step = match key {
            "down" => Some(masonry::Step::Down),
            "up" => Some(masonry::Step::Up),
            "right" if cards && !typing => Some(masonry::Step::Right),
            "left" if cards && !typing => Some(masonry::Step::Left),
            _ => None,
        };
        let to = if let Some(step) = step {
            if order.is_empty() {
                None
            } else {
                let forward = matches!(step, masonry::Step::Down | masonry::Step::Right);
                Some(match selected {
                    None => {
                        if forward {
                            0
                        } else {
                            order.len() - 1
                        }
                    }
                    Some(s) if cards => {
                        let places: Vec<masonry::Place> = entries_uids.iter().map(|uid| self.places.iter().find(|(p, _)| p == uid).map_or_else(masonry::Place::default, |(_, r)| *r)).collect();
                        masonry::neighbour(&places, s, step)
                            .or(match step {
                                masonry::Step::Right => (s + 1 < order.len()).then_some(s + 1),
                                masonry::Step::Left => s.checked_sub(1),
                                _ => None,
                            })
                            .unwrap_or(s)
                    }
                    Some(s) => {
                        if forward {
                            (s + 1).min(order.len() - 1)
                        } else {
                            s.saturating_sub(1)
                        }
                    }
                })
            }
        } else if !typing && (key == "home" || key == "end") && !order.is_empty() {
            Some(if key == "end" { order.len() - 1 } else { 0 })
        } else {
            None
        };
        if let Some(to) = to {
            let uid = entries_uids[to];
            with(&shared, |n| {
                n.select(uid);
                n.view.reveal = true;
            });
            cx.stop_propagation();
            return cx.invalidate();
        }
        let Some(s) = selected else {
            if key == "escape" {
                self.escape(cx);
                cx.stop_propagation();
            }
            return;
        };
        let which = drawn[s];
        let now_at = |n: &Notifications| n.listed().entries.iter().position(|e| which.matches(e));
        match key {
            "enter" => with(&shared, |n| {
                if let Some(i) = now_at(n) {
                    n.open_entry(Some(&**cx), i, None);
                }
            }),
            // (A conversation's row: all of it, as its × does.)
            "delete" | "backspace" if !typing => {
                let all = self.conversation(order[s]);
                with(&shared, |n| n.forget(&all, &drawn));
            }
            "left" | "right" if !typing && by_app => with(&shared, |n| {
                let Some(i) = now_at(n) else { return };
                let a = n.listed().entries[i].app.clone();
                let folded = n.view.folded.contains(&a);
                if (key == "left") != folded {
                    n.fold(a);
                }
            }),
            "escape" => self.escape(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.invalidate();
    }

    /// Escape: the search clears, else the selection goes.
    fn escape(&mut self, cx: &mut Cx<Self>) {
        self.stale = true;
        if !self.search.read(cx).text().trim().is_empty() {
            self.search.update(cx, |f, cx| f.set_text("", cx));
        } else {
            with(&self.shared, |n| {
                n.set_selected(None);
                n.changed();
            });
        }
    }

    /// What the pointer is on at `pos` (the list's own coordinates):
    /// found by where the list is laid out. (A row's or a card's ×, where
    /// its time is, takes the click when it may be forgotten; a group's
    /// Dismiss and the unread ones' Mark All Read, at their headings'
    /// right.)
    fn hit_at(&self, pos: Point<Pixels>) -> Option<Hit> {
        let (_, list) = self.laid.as_ref()?;
        if pos.x < px(0.0) || pos.x > px(self.width) {
            return None;
        }
        if list.cards_view {
            // (Later ones on top, as they're drawn.)
            return list.cards.iter().rev().find_map(|(i, laid, pl)| {
                let r = Bounds::new(point(px(pl.x), px(pl.y)), size(px(pl.w), px(pl.h)));
                if !r.contains(&pos) {
                    return None;
                }
                let e = &list.entries[*i];
                let (at, width) = card_face_at(r);
                let on_picture = e.look.dragged().is_some() && laid.face.image_at(at, width).is_some_and(|p| p.contains(&pos));
                Some(if !e.shown && card_close(r).dilate(px(3.0)).contains(&pos) {
                    Hit::Forget(*i)
                } else if on_picture {
                    Hit::Picture(*i)
                } else {
                    Hit::Card(*i)
                })
            });
        }
        let y = f32::from(pos.y);
        let (item, top, h) = list.lines.iter().find(|(_, top, h)| y >= *top && y < top + h)?;
        let r = Bounds::new(point(px(0.0), px(*top)), size(px(self.width), px(*h)));
        match item {
            Item::Unread(count) => (*count > 0 && mark_all_rect(r).contains(&pos)).then_some(Hit::MarkAllRead),
            Item::Day(_) => None,
            Item::Group { app, members, .. } => Some(if dismiss_rect(r).contains(&pos) {
                // (Not those pinned, or on screen: they stay put.)
                Hit::Dismiss(app.clone(), members.iter().copied().filter(|&i| !list.entries[i].shown && !list.entries[i].pinned).collect())
            } else {
                Hit::Fold(app.clone())
            }),
            Item::Note { i, .. } => {
                let forget = !list.entries[*i].shown && list.rows.get(i).is_some_and(|laid| row_close(laid, r).dilate(px(3.0)).contains(&pos));
                Some(if forget { Hit::Forget(*i) } else { Hit::Row(*i) })
            }
        }
    }

    /// Where the list's content starts in the window (it scrolls).
    fn list_origin(&self) -> Point<Pixels> {
        let b = self.scroll.bounds();
        b.origin + self.scroll.offset()
    }

    fn press(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        self.stale = true;
        let Some(hit) = self.hit_at(e.position - self.list_origin()) else { return };
        let order = self.drawn();
        let select = e.modifiers.platform || e.modifiers.control || e.modifiers.shift;
        let shared = self.shared.clone();
        // (The entry pressed as it was drawn, found again in the list as
        // it is: a note that came or went meanwhile moved its places, and
        // the place alone named another.)
        let drawn = match &hit {
            Hit::Row(i) | Hit::Card(i) | Hit::Picture(i) | Hit::Forget(i) => self.laid.as_ref().and_then(|(_, l)| l.entries.get(*i)).map(store::EntryKey::of),
            _ => None,
        };
        // (What a × or a group's Dismiss takes, as drawn: a row's whole
        // conversation, a group's notes that may go.)
        let taken: Vec<store::EntryKey> = match &hit {
            Hit::Forget(i) => self.conversation(*i),
            Hit::Dismiss(_, members) => self.laid.as_ref().map(|(_, l)| members.iter().filter_map(|&m| l.entries.get(m)).map(store::EntryKey::of).collect()).unwrap_or_default(),
            _ => Vec::new(),
        };
        with(&shared, |n| {
            let now = drawn.and_then(|k| n.listed().entries.iter().position(|e| k.matches(e)));
            match hit {
                Hit::Row(_) => {
                    let Some(i) = now else { return };
                    let uid = n.listed().entries[i].uid;
                    n.select(uid);
                    if e.click_count >= 2 {
                        n.open_entry(Some(&**cx), i, None);
                    }
                }
                Hit::Card(_) => {
                    // One click acts, as on a banner; ⌘ or Shift only selects it.
                    let Some(i) = now else { return };
                    let uid = n.listed().entries[i].uid;
                    if select || e.click_count >= 2 {
                        n.select(uid);
                    } else {
                        n.set_selected(Some(uid));
                        n.open_entry(Some(&**cx), i, None);
                    }
                }
                // Its picture: held, to be dragged out as its file once it
                // has moved far enough, or, let go of where it was, a click on
                // the card (`release`). ⌘ or Shift only selects it.
                Hit::Picture(_) => {
                    let (Some(i), Some(key)) = (now, drawn) else { return };
                    let uid = n.listed().entries[i].uid;
                    if select || e.click_count >= 2 {
                        n.select(uid);
                    } else {
                        n.set_selected(Some(uid));
                        self.held_picture = Some((key, e.position));
                        // (Its file looked at now, if it isn't known: the
                        // drag, a few points on, finds the answer.)
                        let _ = pictures::checked_file(&n.listed().entries[i].look);
                    }
                }
                Hit::Forget(_) | Hit::Dismiss(..) => n.forget(&taken, &order),
                Hit::Fold(a) => n.fold(a),
                Hit::MarkAllRead => {
                    let unread: Vec<u32> = n.history.entries.iter().filter(|e| e.unread).map(|e| e.id).collect();
                    n.view.held.extend(unread);
                    n.mark_all_read();
                }
            }
        });
        self.focus.focus(cx);
        cx.stop_propagation();
        cx.invalidate();
    }

    /// The pointer moved with a card's picture held: far enough, and its
    /// file is dragged out (`dnd::files`), its picture under the pointer
    /// (made with the one shown); the card stays where it is.
    fn held_moved(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let Some((key, at)) = self.held_picture else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.held_picture = None;
            return;
        }
        if f32::from((e.position - at).x).hypot(f32::from((e.position - at).y)) < DRAG_AFTER {
            return;
        }
        self.held_picture = None;
        let (look, archived) = {
            let n = self.shared.lock().unwrap();
            (n.listed().entries.iter().rev().find(|x| key.matches(x)).map(|x| x.look.clone()), n.view.archived)
        };
        let Some(look) = look else { return };
        let Some(path) = pictures::checked_file(&look) else { return };
        let mut drag = olo_kit::dnd::files(std::slice::from_ref(&path), true, false);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        drag.icon =
            look::large_source(&look).and_then(|s| pictures::drag_image(&s, cx.scale_factor())).or_else(|| olo_kit::dnd::picture(cx, "image-x-generic", 48.0, Some(&name), point(px(24.0), px(24.0))));
        olo_kit::dnd::start(cx, drag, move |action| Told::Dropped(key, archived, action));
    }

    /// The button let go of with a card's picture held, not having moved
    /// far enough to drag: a click on the card, which acts.
    fn release(&mut self, cx: &mut Cx<Self>) {
        let Some((key, _)) = self.held_picture.take() else { return };
        self.stale = true;
        // (The very entry pressed: a note that came meanwhile moved the
        // list's places.)
        with(&self.shared.clone(), |n| {
            if let Some(i) = n.listed().entries.iter().position(|e| key.matches(e)) {
                n.open_entry(Some(&**cx), i, None);
            }
        });
        cx.invalidate();
    }

    /// One of the list's buttons (a row's or a card's): action `key` of
    /// the entry `which` as it was drawn, the rest of its conversation
    /// `older`. Pressed with the primary button or the keyboard only: a
    /// right click on Delete, meant for the menu, deletes nothing.
    #[allow(clippy::too_many_arguments)]
    fn list_button(cx: &mut Cx<Self>, place: &str, which: &store::EntryKey, key: &str, label: &str, width: f32, small: bool, older: Vec<store::EntryKey>) -> Button {
        let (which, key) = (*which, key.to_owned());
        let b = Button::new(label.to_owned()).id(format!("{place}-action-{}-{key}", which.uid));
        // (Small first: it sets the least width a small one has, and the
        // width laid out comes after.)
        let b = if small { b.small() } else { b }.min_width(width);
        b.on_click(cx.listener(move |h: &mut HistoryWindow, e: &ClickEvent, cx| {
            if e.standard_click() {
                h.pressed(which, &key, &older, cx);
            }
        }))
    }

    /// A list button pressed: its entry found again as it was drawn (one
    /// that has left the list since, or moved in it, is never another's
    /// to act on), and its action done; in the Cleared list, that list's.
    fn pressed(&mut self, which: store::EntryKey, key: &str, older: &[store::EntryKey], cx: &mut Cx<Self>) {
        self.stale = true;
        let order = self.drawn();
        with(&self.shared.clone(), |n| {
            let at = |k: &store::EntryKey| n.listed().entries.iter().position(|e| k.matches(e));
            let Some(i) = at(&which) else { return };
            match (n.view.archived, key) {
                (true, "open") => n.open_entry(Some(&**cx), i, None),
                (true, "put-back") => n.put_back(i),
                // (The notification, or the whole conversation it gathers.)
                (true, "forget") => {
                    let all: Vec<store::EntryKey> = std::iter::once(which).chain(older.iter().copied()).collect();
                    n.forget(&all, &order);
                }
                (true, _) => {}
                // (Its button pressed: that's all, the selection as it
                // was; a card's pressed selected it, tinting it all for
                // one button's press.)
                (false, key) => n.open_entry(Some(&**cx), i, Some(key.to_owned())),
            }
        });
        cx.invalidate();
    }

    fn menu(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        self.stale = true;
        let i = match self.hit_at(e.position - self.list_origin()) {
            Some(Hit::Row(i) | Hit::Card(i) | Hit::Forget(i) | Hit::Picture(i)) => i,
            _ => return,
        };
        // (The entry as it was drawn, found again in the list as it is:
        // the place alone, a note come or gone since, named another; and
        // by its number, another of the Cleared list's with the same.)
        let Some(key) = self.laid.as_ref().and_then(|(_, l)| l.entries.get(i)).map(store::EntryKey::of) else { return };
        let (entry, archived) = {
            let n = self.shared.lock().unwrap();
            (n.listed().entries.iter().find(|x| key.matches(x)).cloned(), n.view.archived)
        };
        let Some(entry) = entry else { return };
        // (Its conversation and its list now, as the menu opens: what's
        // chosen acts on those.)
        let menued = Menued { members: self.conversation(i), archived };
        let chosen = cx.msg_with(move |command: &str| Told::Command(key, command.to_owned(), menued.clone()));
        menu::context_menu(cx, entry_menu(&entry, archived), e.position, chosen);
        cx.stop_propagation();
    }

    /// The list as drawn: its notes' keys in the order shown; and those
    /// of drawn entry `i` and the rest of its conversation (what its ×
    /// clears: all of it).
    fn drawn(&self) -> Vec<store::EntryKey> {
        self.laid.as_ref().map(|(_, l)| l.order.iter().filter_map(|&i| l.entries.get(i)).map(store::EntryKey::of).collect()).unwrap_or_default()
    }

    fn conversation(&self, i: usize) -> Vec<store::EntryKey> {
        let Some((_, l)) = self.laid.as_ref() else { return Vec::new() };
        let older = l.lines.iter().find_map(|(item, ..)| match item {
            Item::Note { i: lead, older, .. } if *lead == i => Some(older.clone()),
            _ => None,
        });
        std::iter::once(i).chain(older.unwrap_or_default()).filter_map(|x| l.entries.get(x)).map(store::EntryKey::of).collect()
    }

    /// An entry's menu's command, for that very entry, found again now.
    fn entry_command(&mut self, key: store::EntryKey, command: &str, menued: Menued, cx: &mut Cx<Self>) {
        self.stale = true;
        let order = self.drawn();
        // (Its conversation's as the menu opened: what Delete or Clear takes.)
        let all = if menued.members.is_empty() { vec![key] } else { menued.members };
        let shared = self.shared.clone();
        with(&shared, |n| {
            // (The list it was opened on gone from the window: no one's.)
            if n.view.archived != menued.archived {
                return;
            }
            let Some(i) = n.listed().entries.iter().position(|x| key.matches(x)) else { return };
            let id = key.id;
            match command {
                "open" => {
                    n.set_selected(Some(n.listed().entries[i].uid));
                    n.open_entry(Some(&**cx), i, None);
                }
                "forget" => n.forget(&all, &order),
                "put-back" if n.view.archived => n.put_back(i),
                _ if n.view.archived => {}
                "toggle-read" => {
                    n.view.held.insert(id);
                    let unread = n.history.entries[i].unread;
                    n.set_read_of(&key, unread);
                }
                c if c.starts_with("action:") => {
                    n.set_selected(Some(n.listed().entries[i].uid));
                    n.open_entry(Some(&**cx), i, Some(c["action:".len()..].to_owned()));
                }
                c => n.note_command(id, c),
            }
        });
    }

    /// The list laid out `w` wide: the notes that show (unread first),
    /// in the view's lines, each row or card laid out.
    fn lay_out(&mut self, cx: &mut Cx<Self>, p: &olo_theme::Palette, w: f32) -> ListLaid {
        let shared = self.shared.clone();
        let (view, entries, unread_now, selected_id, folded, held, reveal, settings, archived) = {
            let n = shared.lock().unwrap();
            // (In the archive nothing fades: it's all as old as it will get.)
            let settings = if n.view.archived { NotificationSettings { fade: false, ..n.settings.clone() } } else { n.settings.clone() };
            (
                n.settings.view,
                n.listed().entries.clone(),
                if n.view.archived { 0 } else { n.history.unread() },
                n.view.selected,
                n.view.folded.clone(),
                n.view.held.clone(),
                n.view.reveal,
                settings,
                n.view.archived,
            )
        };
        let other = {
            let n = shared.lock().unwrap();
            if n.view.archived { n.history.entries.iter().filter(|e| !e.snoozed()).count() } else { n.archive.entries.len() }
        };
        let search_text = self.search.read(cx).text().to_owned();
        let searching = !search_text.trim().is_empty();

        // What shows (not what is snoozed): unread (and what was read
        // while the window is open) first, newest first in each.
        let first = |e: &store::Entry| e.unread || held.contains(&e.id);
        let mut shown: Vec<usize> = (0..entries.len()).rev().filter(|&i| !entries[i].snoozed() && (!searching || found(&entries[i], &search_text))).collect();
        shown.sort_by_key(|&i| !first(&entries[i]));
        let unread = shown.iter().take_while(|&&i| first(&entries[i])).count();
        let lines = items(&entries, &shown, unread, view, &folded, searching);
        let order: Rc<Vec<usize>> = Rc::new(lines.iter().filter_map(|l| if let Item::Note { i, .. } = l { Some(*i) } else { None }).collect());
        // (A selection stacked under a conversation's newest is that.)
        let selected = selected_id.and_then(|uid| {
            order.iter().position(|&i| entries[i].uid == uid).or_else(|| {
                lines.iter().find_map(|l| match l {
                    Item::Note { i, older, .. } if older.iter().any(|&o| entries[o].uid == uid) => order.iter().position(|o| o == i),
                    _ => None,
                })
            })
        });
        let sel_id = selected.map(|s| entries[order[s]].uid);
        if sel_id != selected_id && sel_id.is_some() {
            shared.lock().unwrap().view.selected = sel_id;
        }

        // The list, laid out for its width.
        let cards_view = view == HistoryView::Cards;
        let mut placed: Vec<Line> = Vec::new();
        let mut row_laid: HashMap<usize, Rc<RowLaid>> = HashMap::new();
        // The selected row's top, and the rest of its conversation.
        let mut sel_row: Option<(usize, f32, Vec<usize>)> = None;
        let mut card_laid: Vec<(usize, Rc<CardLaid>, masonry::Place)> = Vec::new();
        let total;
        if cards_view {
            let (cols, col_w) = masonry::columns(w, CARD_W, m::SPACING);
            let mut sizes = Vec::new();
            let mut laid = Vec::new();
            for (k, &i) in order.iter().enumerate() {
                let e = &entries[i];
                let span = card_span(e, cols, k == 0 && e.unread);
                let l = self.card(cx, p, e, span, masonry::span_width(span, col_w, m::SPACING), archived);
                sizes.push((span, l.height));
                laid.push((i, l));
            }
            let (places, height) = masonry::arrange(cols, col_w, m::SPACING, &sizes);
            self.places = laid.iter().zip(&places).map(|((i, _), pl)| (entries[*i].uid, *pl)).collect();
            card_laid = laid.into_iter().zip(places).map(|((i, l), pl)| (i, l, pl)).collect();
            // (The arrangement has its gap above the first and below the
            // last: drawn a gap lower again, the first card stood twice as
            // far from the top as from the side.)
            total = height;
        } else {
            let mut y = 0.0;
            for item in lines {
                let h = match &item {
                    Item::Unread(_) | Item::Day(_) => HEADING,
                    Item::Group { .. } => HEADING + 2.0,
                    Item::Note { i, older, grouped } => {
                        let e = &entries[*i];
                        let is_sel = sel_id == Some(e.uid);
                        let l = self.row(cx, p, &entries, *i, older, meta_of(e, view), is_sel, *grouped, w, archived);
                        let h = l.height;
                        if is_sel {
                            sel_row = Some((*i, y, older.clone()));
                        }
                        row_laid.insert(*i, l);
                        h
                    }
                };
                placed.push(Line { item, y, h });
                y += h;
            }
            total = y;
        }
        // Rows and cards no longer shown are let go.
        let ids: HashSet<u64> = entries.iter().map(|e| e.uid).collect();
        self.rows.retain(|id, _| ids.contains(id));
        self.cards.retain(|id, _| ids.contains(id));
        // The selection brought into view.
        if reveal && let Some(id) = sel_id {
            let span = if cards_view {
                card_laid.iter().find(|(i, ..)| entries[*i].uid == id).map(|(_, _, pl)| (pl.y - m::SPACING, pl.y + pl.h + m::SPACING))
            } else {
                placed.iter().find(|l| matches!(l.item, Item::Note { i, .. } if entries[i].uid == id)).map(|l| (l.y, l.y + l.h))
            };
            if let Some((top, bottom)) = span {
                let view_h = f32::from(self.scroll.bounds().size.height);
                let at = -f32::from(self.scroll.offset().y);
                let to = if top < at {
                    top
                } else if bottom > at + view_h {
                    bottom - view_h
                } else {
                    at
                };
                if to != at {
                    self.scroll.set_offset(cx, point(px(0.0), px(-to.max(0.0))));
                }
            }
            shared.lock().unwrap().view.reveal = false;
        }

        let snoozed = entries.iter().filter(|e| e.snoozed()).count();
        self.laid_count += 1;
        ListLaid {
            generation: self.laid_count,
            settings,
            // (All of the archive can go: nothing in it is on screen, pinned or snoozed.)
            can_clear: entries.iter().any(|e| !e.shown && !e.pinned && !e.snoozed()),
            other,
            all: entries.len() - snoozed,
            snoozed,
            shown: shown.len(),
            unread: unread_now,
            entries,
            view,
            cards_view,
            searching,
            order,
            lines: Rc::new(placed.into_iter().map(|l| (l.item, l.y, l.h)).collect()),
            rows: Rc::new(row_laid),
            cards: Rc::new(card_laid),
            total,
            sel_id,
            sel_row,
        }
    }
}

/// How a line or a card is drawn now: what its drawing's key is made of
/// besides what it shows.
#[derive(Clone, Copy, Hash)]
struct Shown {
    hovered: bool,
    /// The pointer is on its × (or its Dismiss, its Mark All Read).
    lit: bool,
    /// The pointer is on its large picture, which drags out as its file.
    picture: bool,
    selected: bool,
    /// How opaque (as bits: it fades as it ages).
    opacity: u32,
}

/// A card, in `r`.
fn paint_card(g: &mut Painter, r: Bounds<Pixels>, e: &store::Entry, laid: &CardLaid, shown: Shown) {
    let p = theme::palette();
    // What the pointer lights (`HistoryWindow::hit_at`): the list hears
    // only the moves across these.
    g.note_pointer_edge(r);
    if !e.shown {
        g.note_pointer_edge(card_close(r).dilate(px(3.0)));
    }
    let r = style::snap(g, r);
    // Staggered in its column: level with a card beside it only by
    // chance.
    olo_kit::review::own_grid(g, r);
    let (hovered, is_sel, lit) = (shown.hovered, shown.selected, shown.lit);
    let head = card_head(r);
    let inner = Bounds::from_corners(head.origin, r.bottom_right() - point(px(m::CARD_INSET), px(m::CARD_INSET)));
    // Whose and when: the app's mark and name, the time at the right.
    let time = if e.pinned { format!("Pinned · {}", crate::when(e.time)) } else { crate::when(e.time) };
    // Forget it: a × beside the time, always (as a banner's is), its right
    // end on the buttons' right edge. (Only while the pointer was over the
    // card, it went as the pointer went onto one of its buttons, and the
    // time stood short of the right edge where it wasn't.)
    let forgettable = !e.shown;
    let close = card_close(r);
    let picture = g.with_element_opacity(Some(f32::from_bits(shown.opacity)), |g| {
        // As the banner it was: the panel's grey, square, a frame a line
        // round it and raised inside it; under the pointer a little
        // lighter; selected, the list's selection colour, square.
        let ground = if is_sel {
            style::c(p.list_selected_background)
        } else if hovered {
            style::shade(&p, p.panel_background, 0.97)
        } else {
            style::c(p.panel_background)
        };
        g.paint_quad(fill(r, ground));
        style::frame_rect(g, r, style::tint(p.panel_background, 1.6));
        if !is_sel {
            style::raised(g, style::inset(g, r, 1.0), &p);
        }
        let (text, dim) = if is_sel {
            (style::c(p.list_selected_item_text), style::c(olo_theme::mix(p.list_selected_item_text, p.list_selected_background, 0.3)))
        } else {
            (style::c(p.panel_text), style::secondary_text(&p))
        };
        // (Unread: its dot first in the header, its edge on the card's
        // text column, as the × ends on it at the right, and the app's mark
        // a clear gap after it; read, the mark's drawing starts on that
        // column, where the name, the picture and Delete below start. In the
        // 10 points between the card's edge and its column the dot was
        // pressed against both.)
        // (The dot is 7 points across, `unread_dot`.)
        let dot = 7.0;
        let at = if e.unread {
            unread_dot(g, point(head.left() + px(dot / 2.0), head.center().y), &p);
            dot + 5.0
        } else {
            0.0
        };
        let mid = style::snap_coord(g, head.center().y);
        let icon = Bounds::new(point(head.left() + px(at - ICON_MARGIN), mid - px(CARD_MARK / 2.0)), size(px(CARD_MARK), px(CARD_MARK)));
        paint_icon(g, icon, e, false);
        let small = style::ui_font(FontWeight::NORMAL);
        let time_w = f32::from(style::text_width(time.clone(), style::DETAIL_SIZE, small.clone()));
        let time_right = head.right() - px(if e.shown { 0.0 } else { CARD_CLOSE_ROOM });
        let name_x = icon.right() + px(6.0);
        let name_room = time_right - px(time_w + 8.0) - name_x;
        // (Whose it is, quietly: its title is what's bold.)
        style::label_fit(g, name_x, mid, Align::Left, app_name(e).to_owned(), style::DETAIL_SIZE, small.clone(), dim, name_room);
        let baseline = style::baseline_at(g, mid, style::DETAIL_SIZE);
        style::label_on_baseline(g, time_right, baseline, Align::Right, time.clone(), style::DETAIL_SIZE, small.clone(), dim);
        if forgettable {
            look::close_cross(g, close.center(), if lit { text } else { dim });
            agent::record(g, agent::Node::button(clears(Some(e)), close));
        }
        style::hline_above(g, r.left() + px(2.0), r.right() - px(2.0), r.top() + px(CARD_HEAD), style::tint(p.panel_background, 1.25));
        // (What a strip's cut end fades into: the card's ground as it is.)
        let ink = look::Ink { paper: ground, ..laid.ink.clone() };
        look::paint(g, &entry_face(e, shown.picture), &laid.face, &ink, point(inner.left(), head.bottom() + px(CARD_HEAD_GAP)), f32::from(inner.size.width)).image
    });
    // A picture that drags out as its file is the open hand, and the list
    // hears the pointer cross its edge (its frame lights): noted beside
    // the card's own edge.
    if let Some(picture) = picture {
        g.set_cursor_style(olo_kit::CursorStyle::OpenHand, picture);
        g.note_pointer_edge(picture);
    }
    let title = if e.summary.is_empty() { app_name(e) } else { &e.summary };
    agent::record(g, agent::Node { role: "ListItem", name: Some(title.to_owned().into()), bounds: r, enabled: true, clickable: true, selected: Some(is_sel), ..Default::default() });
}

/// A heading or a group's line, in `r`.
fn paint_heading(g: &mut Painter, r: Bounds<Pixels>, item: &Item, entries: &[store::Entry], shown: Shown) {
    let p = theme::palette();
    // What the pointer lights (`HistoryWindow::hit_at`).
    g.note_pointer_edge(r);
    match item {
        Item::Unread(count) if *count > 0 => g.note_pointer_edge(mark_all_rect(r)),
        Item::Group { .. } => g.note_pointer_edge(dismiss_rect(r)),
        _ => {}
    }
    heading_fill(g, &p, r);
    match item {
        Item::Unread(count) => {
            let t = style::label(g, r.left() + px(ICON_X), r.center().y, Align::Left, "Unread", style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::disabled_text(&p));
            agent::record(g, agent::Node::label("Unread", t));
            if *count == 0 {
                return;
            }
            let baseline = style::baseline_at(g, r.center().y, style::SMALL_SIZE);
            style::label_on_baseline(g, t.right() + px(5.0), baseline, Align::Left, count.to_string(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
            // Read them all, here where they are.
            let all = "Mark All Read";
            let ink = if shown.lit { style::c(p.document_text) } else { style::disabled_text(&p) };
            // (Its end on the rows' right column, where their time and a
            // row's last button end: past it, it stood out of the list.)
            style::label(g, r.right() - px(CLOSE_W), r.center().y, Align::Right, all, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), ink);
            agent::record(g, agent::Node::button(all, mark_all_rect(r)));
        }
        Item::Day(label) => {
            let t = style::label(g, r.left() + px(ICON_X), r.center().y, Align::Left, label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::disabled_text(&p));
            agent::record(g, agent::Node::label(label.clone(), t));
        }
        Item::Group { app, count, members, open } => {
            let name = if app.is_empty() { "Notification" } else { app.as_str() };
            let text = style::c(p.document_text);
            let dim = style::disabled_text(&p);
            // (Under the pointer: its latch lit, and its Clear shown.)
            let lit = shown.hovered;
            style::latch(g, Bounds::centered_at(point(r.left() + px(7.0), r.center().y), size(px(10.0), px(10.0))), *open, if lit { text } else { dim });
            if let Some(first) = members.first().and_then(|&i| entries.get(i)) {
                paint_icon(g, Bounds::centered_at(point(r.left() + px(ICON_X + SMALL_ICON / 2.0), r.center().y), size(px(SMALL_ICON), px(SMALL_ICON))), first, false);
            }
            let n = style::label(g, r.left() + px(TEXT_X), r.center().y, Align::Left, name.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), text);
            let baseline = style::baseline_at(g, r.center().y, style::PLAIN_SIZE);
            style::label_on_baseline(g, n.right() + px(6.0), baseline, Align::Left, count.to_string(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
            agent::record(g, agent::Node { role: "Button", name: Some(format!("{name} ({count})").into()), bounds: r, enabled: true, clickable: true, selected: Some(*open), ..Default::default() });
            if lit {
                let cr = dismiss_rect(r);
                let word = clears(members.first().and_then(|&i| entries.get(i)));
                style::label(g, cr.right() - px(4.0), r.center().y, Align::Right, word, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), if shown.lit { text } else { dim });
                agent::record(g, agent::Node::button(format!("{word} {name}"), cr));
            }
        }
        Item::Note { .. } => {}
    }
}

/// A note's row, in `r`.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn paint_row(g: &mut Painter, r: Bounds<Pixels>, e: &store::Entry, laid: &RowLaid, older: usize, grouped: bool, view: HistoryView, shown: Shown, rule: bool) {
    let p = theme::palette();
    // What the pointer lights (`HistoryWindow::hit_at`).
    g.note_pointer_edge(r);
    if !e.shown {
        g.note_pointer_edge(row_close(laid, r).dilate(px(3.0)));
    }
    let (is_sel, hovered) = (shown.selected, shown.hovered);
    let (fg, bg) = if is_sel { (p.list_selected_item_text, p.list_selected_background) } else { (p.document_text, p.document_background) };
    let dim = row_dim(fg, bg, is_sel);
    if is_sel {
        g.paint_quad(fill(r, style::c(bg)));
    } else if hovered {
        g.paint_quad(fill(r, style::tint(p.document_background, 1.04)));
    }
    let x0 = if grouped { 8.0 } else { 0.0 };
    let x = r.left() + px(x0 + TEXT_X);
    let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
    // Older, fainter; full again while selected or under the pointer.
    g.with_element_opacity(Some(f32::from_bits(shown.opacity)), |g| {
        // The title's first line, the app and time, the dot and the icon
        // all centre on one line.
        let title_top = style::snap_coord(g, r.top() + px(ROW_PAD));
        let baseline = laid.title.baseline(title_top);
        let line1 = baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        if e.unread {
            unread_dot(g, point(r.left() + px(x0 + 9.0), line1), &p);
        }
        paint_icon(g, Bounds::centered_at(point(r.left() + px(x0 + ICON_X + SMALL_ICON / 2.0), line1), size(px(SMALL_ICON), px(SMALL_ICON))), e, true);
        laid.title.paint(g, point(x, title_top));
        agent::record(g, agent::Node::label(title_text.to_owned(), Bounds::new(point(x, title_top), size(px(laid.title.width), px(laid.title.height)))));
        if failed(e) {
            look::paint_stop(g, &laid.title, point(x, title_top), style::PLAIN_SIZE);
        }
        style::label_on_baseline(g, r.right() - px(CLOSE_W), baseline, Align::Right, meta_of(e, view), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
        if older > 0 && !is_sel {
            // How many in the conversation.
            let n = (older + 1).to_string();
            let tx = x + px(laid.title.first_row_width() + 6.0);
            style::pill(g, tx, line1, &n, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim, style::c(p.document_background));
        }
        let mut y = title_top + px(laid.title.height);
        if let Some(body) = &laid.body {
            y += px(row_body_gap(e));
            body.paint(g, point(x, y));
            agent::record(g, agent::Node::label(e.text.clone(), Bounds::new(point(x, y), size(px(body.width), px(body.height)))));
            y += px(body.height);
        }
        if !laid.older.is_empty() {
            y += px(m::TIGHT);
            let older_h = style::SMALL_SIZE + 5.0;
            for (t, text) in &laid.older {
                let cy = y + px(older_h / 2.0);
                style::label(g, x, cy, Align::Left, t.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                let top = style::snap_coord(g, cy - px(text.height / 2.0));
                text.paint(g, point(x + px(44.0), top));
                agent::record(g, agent::Node::label(text.text.clone(), Bounds::new(point(x + px(44.0), top), size(px(text.width), px(text.height)))));
                y += px(older_h);
            }
        }
        // Forget it: a × while the pointer is over it.
        if !e.shown && hovered {
            let close = Bounds::centered_at(point(r.right() - px(CLOSE_AT), line1), size(px(12.0), px(12.0)));
            look::close_cross(g, close.center(), if shown.lit { style::c(fg) } else { dim });
            agent::record(g, agent::Node::button(clears(Some(e)), close));
        }
    });
    if !is_sel && rule {
        style::hline_above(g, r.left() + px(x0 + TEXT_X), r.right(), r.bottom(), style::tint(p.document_background, 1.07));
    }
    agent::record(g, agent::Node { role: "ListItem", name: Some(title_text.to_owned().into()), bounds: r, enabled: true, clickable: true, selected: Some(is_sel), ..Default::default() });
}

impl View for HistoryWindow {
    type Message = Told;

    fn message(&mut self, told: Told, cx: &mut Cx<Self>) {
        self.stale = true;
        match told {
            Told::Changed => {}
            Told::Command(key, command, menued) => self.entry_command(key, &command, menued, cx),
            // Dropped somewhere: used, so read (and off the screen, if
            // its banner is there too). One of the Cleared list is read
            // and has no banner: nothing to do, and the note in the list
            // with its number, a newer one, is no part of it.
            Told::Dropped(key, false, Some(_)) => with(&self.shared, |n| n.dropped_entry(&key)),
            Told::Dropped(..) => {}
        }
        cx.invalidate();
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        self.do_command(name, cx);
        true
    }

    /// Closed: what was read while it was open moves.
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        with(&self.shared, |n| n.history_closed());
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let bar = MenuBar::of(self, cx, |v, _| v.menus());
        let (dnd, quiet, schedule_quiet, archived, waiting) = {
            let n = self.shared.lock().unwrap();
            // (What waits in apps: over the list, never in the Cleared one.)
            (n.settings.dnd, n.quiet(), !n.settings.dnd, n.view.archived, if n.view.archived { Vec::new() } else { n.waiting.apps.clone() })
        };
        let now = now_secs();

        // The list, laid out for its width (or as it was).
        let w = (f32::from(cx.viewport_size().width) - SCROLL_BAR - 2.0).max(100.0);
        self.width = w;
        let key = LaidKey { width: w.to_bits(), scale: cx.scale_factor().to_bits(), palette: p.clone() };
        let list = match &self.laid {
            Some((k, l)) if !self.stale && *k == key => l.clone(),
            _ => {
                let l = Rc::new(self.lay_out(cx, &p, w));
                self.laid = Some((key, l.clone()));
                self.stale = false;
                l
            }
        };
        let (view, cards_view, sel_id, total, searching) = (list.view, list.cards_view, list.sel_id, list.total, list.searching);
        let palette_gen = match &self.palette {
            Some((was, n)) if **was == *p => *n,
            was => {
                let n = was.as_ref().map_or(0, |(_, n)| n + 1);
                self.palette = Some((p.clone(), n));
                n
            }
        };
        // (An unread one shows whole: it isn't going anywhere. A read one
        // fades by the time since it was read, until it's tidied away.)
        let strength = |e: &store::Entry| if e.pinned || e.unread { 1.0 } else { list.settings.strength(now.saturating_sub(e.since())) };
        // What every drawing's key starts with.
        let base = (list.generation, palette_gen, crate::icons_made());

        // Only what's near what shows is in the window: a screen above
        // and below it (more come as it scrolls near either end).
        let view_h = f32::from(self.scroll.bounds().size.height).max(f32::from(cx.viewport_size().height));
        let top = -f32::from(self.scroll.offset().y);
        let (lo, hi) = (top - view_h, top + 2.0 * view_h);
        self.built = (lo, hi);
        let hover = self.hover.clone();
        let mut content = div().id("history-list").relative().w(px(w)).h(px(total.max(1.0)));
        if cards_view {
            for (i, laid, pl) in list.cards.iter() {
                let (y0, y1) = (pl.y, pl.y + pl.h);
                if y1 < lo || y0 > hi {
                    continue;
                }
                let e = &list.entries[*i];
                let hovered = hover.as_ref().is_some_and(|h| matches!(h, Hit::Card(x) | Hit::Forget(x) | Hit::Picture(x) if x == i));
                let is_sel = sel_id == Some(e.uid);
                let opacity = if !is_sel && !hovered { fade_opacity(strength(e)) } else { 1.0 };
                let shown = Shown { hovered, lit: hover == Some(Hit::Forget(*i)), picture: hover == Some(Hit::Picture(*i)), selected: is_sel, opacity: opacity.to_bits() };
                // Painted again only when what it shows changes; a screen
                // of cards is tens of thousands of glyphs.
                let key = key_of((base, e.uid, laid.generation, shown, crate::when(e.time)));
                let (entries, cards, k) = (list.entries.clone(), list.cards.clone(), *i);
                let card = canvas(move |g, r| {
                    if let Some((_, laid, _)) = cards.iter().find(|(i, ..)| *i == k) {
                        paint_card(g, r, &entries[k], laid, shown);
                    }
                })
                .paint_key(key)
                .id(("card", e.uid))
                .absolute()
                .left(px(pl.x))
                .top(px(y0))
                .w(px(pl.w))
                .h(px(pl.h));
                content = content.child(card);
            }
        } else {
            for (k, (item, y, h)) in list.lines.iter().enumerate() {
                if y + h < lo || *y > hi {
                    continue;
                }
                let line = match item {
                    Item::Note { i, older, grouped } => {
                        let e = &list.entries[*i];
                        if !list.rows.contains_key(i) {
                            continue;
                        }
                        let is_sel = sel_id == Some(e.uid);
                        let hovered = hover.as_ref().is_some_and(|h| matches!(h, Hit::Row(x) | Hit::Forget(x) if x == i));
                        let opacity = if !is_sel && !hovered { fade_opacity(strength(e)) } else { 1.0 };
                        let shown = Shown { hovered, lit: hover == Some(Hit::Forget(*i)), picture: false, selected: is_sel, opacity: opacity.to_bits() };
                        // (A line under it only between it and the next row:
                        // none under the list's last, nor over a heading.)
                        let rule = matches!(list.lines.get(k + 1), Some((Item::Note { .. }, ..)));
                        let key = key_of((base, "row", e.uid, shown, meta_of(e, view), rule));
                        let (entries, rows, i, older, grouped) = (list.entries.clone(), list.rows.clone(), *i, older.len(), *grouped);
                        canvas(move |g, r| {
                            if let Some(laid) = rows.get(&i) {
                                paint_row(g, r, &entries[i], laid, older, grouped, view, shown, rule);
                            }
                        })
                        .paint_key(key)
                        .id(("note", e.uid))
                    }
                    heading => {
                        let (hovered, lit) = match (heading, &hover) {
                            (Item::Unread(_), Some(Hit::MarkAllRead)) => (true, true),
                            (Item::Group { app, .. }, Some(Hit::Fold(a))) if a == app => (true, false),
                            (Item::Group { app, .. }, Some(Hit::Dismiss(a, _))) if a == app => (true, true),
                            _ => (false, false),
                        };
                        let shown = Shown { hovered, lit, picture: false, selected: false, opacity: 1.0f32.to_bits() };
                        let key = key_of((base, "heading", k, shown));
                        let (lines, entries) = (list.lines.clone(), list.entries.clone());
                        canvas(move |g, r| {
                            if let Some((item, ..)) = lines.get(k) {
                                paint_heading(g, r, item, &entries, shown);
                            }
                        })
                        .paint_key(key)
                        .id(("heading", k))
                    }
                };
                content = content.child(line.absolute().left(px(0.0)).top(px(*y)).w(px(w)).h(px(*h)));
            }
        }

        // The list's own buttons: the selected row's, and live cards'.
        if cards_view {
            for (i, laid, pl) in list.cards.iter() {
                if laid.buttons.is_empty() || pl.y + pl.h < lo || pl.y > hi {
                    continue;
                }
                // As on a banner: the one set apart (a Delete) at the
                // start, the rest from the far end, in their order; what
                // doesn't fit is in the card's menu.
                let by = pl.y + pl.h - m::CARD_INSET - CARD_BUTTON;
                let which = store::EntryKey::of(&list.entries[*i]);
                let small_font = style::ui_font(FontWeight::NORMAL);
                let fit = |l: &str, w: f32| style::fit_with(cx.text_system(), olo_kit::words::of_text(l), style::DETAIL_SIZE, &small_font, px(w - 16.0)).to_string();
                for (bx, key, label, bw) in lay_list_buttons(&laid.buttons, laid.apart.as_deref(), pl.x + m::CARD_INSET, pl.x + pl.w - m::CARD_INSET, m::TIGHT, fit) {
                    content = content.child(div().absolute().left(px(bx)).top(px(by)).occlude().child(Self::list_button(cx, "card", &which, &key, &label, bw, true, Vec::new())));
                }
            }
        } else if let Some((i, top, older)) = list.sel_row.clone()
            && let Some(laid) = list.rows.get(&i)
        {
            // The selected row's buttons, under its text, as a card's
            // are: the one set apart (a Delete) at the start, under the
            // title; the rest from the far end, under the time, in their
            // order, 6 points apart; what doesn't fit is in the row's
            // menu. (Double-click or Return opens it; its × clears it.)
            let x = if view == HistoryView::Apps { 8.0 } else { 0.0 } + TEXT_X;
            let mut by = top + ROW_PAD + laid.title.height + laid.body.as_ref().map_or(0.0, |b| row_body_gap(&list.entries[i]) + b.height);
            if !laid.older.is_empty() {
                by += m::TIGHT + laid.older.len() as f32 * (style::SMALL_SIZE + 5.0);
            }
            by += m::TIGHT;
            let which = store::EntryKey::of(&list.entries[i]);
            let older: Vec<store::EntryKey> = older.iter().filter_map(|&o| list.entries.get(o)).map(store::EntryKey::of).collect();
            let font = style::ui_font(FontWeight::NORMAL);
            let fit = |l: &str, w: f32| style::fit_with(cx.text_system(), olo_kit::words::of_text(l), style::DETAIL_SIZE, &font, px(w - 16.0)).to_string();
            for (bx, key, label, bw) in lay_list_buttons(&laid.buttons, laid.apart.as_deref(), x, w - CLOSE_W, m::TIGHT, fit) {
                content = content.child(div().absolute().left(px(bx)).top(px(by)).occlude().child(Self::list_button(cx, "row", &which, &key, &label, bw, true, older.clone())));
            }
        }
        let content = content
            .on_mouse_down(MouseButton::Left, cx.listener(|h: &mut HistoryWindow, e: &MouseDownEvent, cx| h.press(e, cx)))
            // (A card's picture held: dragged out as it moves, a click if
            // it's let go of first.)
            .on_drag_move(cx.listener(|h: &mut HistoryWindow, e: &MouseMoveEvent, cx| h.held_moved(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|h: &mut HistoryWindow, _: &olo_kit::MouseUpEvent, cx| h.release(cx)))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|h: &mut HistoryWindow, _: &olo_kit::MouseUpEvent, _| {
                    h.held_picture = None;
                }),
            )
            .on_mouse_down(MouseButton::Right, cx.listener(|h: &mut HistoryWindow, e: &MouseDownEvent, cx| h.menu(e, cx)))
            // (Heard only across the edges of what it lights, which each
            // row, card and heading notes as it's drawn.)
            .moves_matter_at_edges()
            .on_mouse_move(cx.listener(|h: &mut HistoryWindow, e: &MouseMoveEvent, cx| {
                let now = h.hit_at(e.position - h.list_origin());
                if now != h.hover {
                    // (Over a card's picture: what a drag of it carries
                    // made now if it isn't kept, there when it's taken
                    // hold of.)
                    if let Some(Hit::Picture(i)) = &now
                        && let Some(look) = h.laid.as_ref().and_then(|(_, l)| l.entries.get(*i)).map(|e| e.look.clone())
                    {
                        if let Some(source) = look::large_source(&look) {
                            let _ = pictures::drag_image(&source, cx.scale_factor());
                        }
                        // (Its file looked at on a worker: the drag finds it known.)
                        let _ = pictures::checked_file(&look);
                    }
                    h.hover = now;
                    cx.invalidate();
                }
            }))
            .on_hover(cx.listener(|h: &mut HistoryWindow, inside: &bool, cx| {
                if !*inside && h.hover.take().is_some() {
                    cx.invalidate();
                }
            }));
        let shown_list: Element = if list.shown == 0 {
            let empty = match (searching, archived) {
                (true, _) => "Nothing matches",
                (false, true) => "Nothing cleared",
                (false, false) => "No notifications",
            };
            // In the list's own scroll view, its frame and its bar (no thumb)
            // as they are with notes in it: the first that comes doesn't
            // make the list jump 14 points narrower.
            let sign = canvas(move |g, b| {
                let p = theme::palette();
                // The app's icon, 48 points, grey and half see-through, as
                // what can't be used is (its own colours, faded, were a pale
                // cream in the light look and mustard in the dark), and the
                // words 12 points under it, in secondary ink: the two round
                // two fifths of the way down the list (one grey line in its
                // very middle looked unfinished).
                const ICON: f32 = 48.0;
                let words_h = style::PLAIN_SIZE * style::LINE_HEIGHT;
                let top = b.top() + (b.size.height * 0.4 - px((ICON + 12.0 + words_h) / 2.0)).max(px(8.0));
                let icon = style::snap(g, Bounds::new(point(b.center().x - px(ICON / 2.0), top), size(px(ICON), px(ICON))));
                let side = (ICON * g.scale_factor()).round() as u32;
                match grey_icon("notifications", side) {
                    Some(img) => {
                        let at = style::snap_centred(g, icon.center(), side as f32 / g.scale_factor(), side as f32 / g.scale_factor());
                        g.with_element_opacity(Some(0.5), |g| {
                            let _ = g.paint_image(at, Corners::default(), img, 0, false);
                        });
                    }
                    None => g.paint_unfinished(),
                }
                let at =
                    style::label(g, b.center().x, icon.bottom() + px(12.0 + words_h / 2.0), Align::Center, empty, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
                agent::record(g, agent::Node::label(empty, at));
            })
            .paint_key(key_of(("empty", searching, archived, ICONS_SHOWN.load(std::sync::atomic::Ordering::Relaxed))))
            .size_full();
            ScrollView::new(&self.scroll).background(p.document_background).child(sign).into_element()
        } else {
            let background = if cards_view { style::card_paper(&p).0 } else { p.document_background };
            ScrollView::new(&self.scroll)
                .background(background)
                // More of the list put in the window as it nears the
                // end of what's there.
                .on_scroll(cx.listener(move |h: &mut HistoryWindow, at: &Point<Pixels>, cx| {
                    let view_h = f32::from(h.scroll.bounds().size.height);
                    let top = -f32::from(at.y);
                    let (lo, hi) = h.built;
                    if (lo > 0.0 && top < lo + view_h * 0.5) || (hi < total && top + view_h > hi - view_h * 0.5) {
                        cx.invalidate();
                    }
                }))
                .child(content)
                .into_element()
        };

        // The bar: Do Not Disturb, the search, Clear All (in the archive:
        // the search and Empty Archive; Do Not Disturb is nothing of the
        // archive's).
        let dnd_box = CheckBox::new("Do Not Disturb", dnd).height(m::CONTROL_HEIGHT).on_toggle(cx.listener(|h: &mut HistoryWindow, on: &bool, _| with(&h.shared, |n| n.set_quiet(*on, 0))));
        let clear = Button::new(if archived { "Delete All" } else { "Clear All" }).min_width(m::BUTTON_MIN_WIDTH).enabled(list.can_clear).on_click(cx.listener(
            move |h: &mut HistoryWindow, e: &ClickEvent, _| {
                // (Not on a right click: Delete All, above all.)
                if e.standard_click() {
                    h.stale = true;
                    with(&h.shared, |n| if archived { n.delete_archived(None) } else { n.clear_history() });
                }
            },
        ));
        let mut bar_row = div().flex().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT + 2.0 * m::SPACING)).px(px(m::SPACING));
        bar_row = if archived {
            // (Where Do Not Disturb is in the notifications' bar: which list this is.)
            bar_row.child(div().flex_shrink_0().child(StringView::new("Cleared").bold(true)))
        } else {
            bar_row.child(div().flex_shrink_0().child(dnd_box))
        };
        let bar_row = bar_row
            // The search takes what the rest leave.
            .child(div().flex_1().min_w_0().child(TextControl::new(cx, &self.search)))
            .child(clear);

        // The status line.
        let (snoozed, all, unread_now) = (list.snoozed, list.all, list.unread);
        let mut status = if searching {
            format!("{} of {all}", list.shown)
        } else if archived {
            // (What it is, and how long it's kept.)
            olo_kit::tr!("{} cleared · kept for {} days, the last {}", all, store::ARCHIVE_DAYS, store::ARCHIVE_KEEP)
        } else if all == 1 {
            "1 notification".into()
        } else {
            format!("{all} notifications")
        };
        if unread_now > 0 && !searching {
            status += &format!(", {unread_now} unread");
        }
        if snoozed > 0 && !searching {
            status += &format!(", {snoozed} snoozed");
        }
        if quiet && !archived {
            status += if !schedule_quiet { " · Do Not Disturb" } else { " · Quiet by schedule" };
        }
        // A status bar along the window's foot, as Tracker's count bar:
        // its own ground, its own insets. In it, the count at the left
        // and, at the right, the way to the other list: the archive
        // (what has left the notifications: faded away, dismissed,
        // cleared), and back (also View ▸ Archived). The two share one
        // row, so their words are level at every scale.
        let ground = canvas(|g, b| style::fill_rect(g, b, style::c(theme::palette().panel_background))).paint_key(key_of("foot-ground")).absolute().inset_0();
        let count = canvas({
            let status = status.clone();
            move |g, b| {
                let p = theme::palette();
                let at = style::label(g, b.left(), b.center().y, Align::Left, status.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                agent::record(g, agent::Node::label(status.clone(), at));
            }
        })
        .paint_key(key_of(("foot", &status)))
        .flex_1()
        .h(px(m::LABEL_ROW));
        // (Back to the list: by its name alone; its unread are counted on
        // the bell and in its own foot, and another number here was one
        // more that agreed with neither.)
        let other = if archived { "Notifications".to_owned() } else { format!("Cleared ({})", list.other) };
        // (Nothing cleared: no link, to nothing. Drawn as one that can't be
        // used, it was still a link's blue.)
        let has_other = archived || list.other > 0;
        let to_other = LinkButton::new(other).beside(style::DETAIL_SIZE).on_click(cx.listener(move |h: &mut HistoryWindow, _: &ClickEvent, cx| {
            h.stale = true;
            with(&h.shared, |n| n.show_archived(!archived));
            cx.invalidate();
        }));
        let foot =
            div().relative().w_full().h(px(FOOTER)).child(ground).child(
                div().absolute().inset_0().flex().flex_row().items_center().gap(px(m::SPACING)).px(px(m::SPACING)).child(count).children(has_other.then(|| div().flex_shrink_0().child(to_other))),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|h: &mut HistoryWindow, e: &KeyDownEvent, cx| h.key(e, cx)))
            .child(bar)
            .child(bar_row)
            .children((!waiting.is_empty()).then(|| waiting_strip(cx, &waiting)))
            .child(div().flex_1().min_h_0().child(shown_list))
            .child(foot)
    }
}

/// Icon `name`, `px` device pixels a side, grey (its colours gone, its
/// shading kept), made once from its bitmap; None while that loads (the
/// drawing is made again when it comes, `ICONS_SHOWN` in its key).
fn grey_icon(name: &str, px: u32) -> Option<std::sync::Arc<olo_kit::RenderImage>> {
    type Made = Option<(String, u32, std::sync::Arc<olo_kit::RenderImage>)>;
    static MADE: olo_sync::Mutex<Made> = olo_sync::Mutex::new(None);
    if let Some((n, p, img)) = MADE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && n == name
        && *p == px
    {
        return Some(img.clone());
    }
    let Some(img) = olo_kit::icons::bitmap(name, px) else {
        // (Asked for: the drawing is made again when it comes.)
        ICONS_SHOWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return None;
    };
    let size = img.size(0);
    let mut bgra = img.as_bytes(0)?.to_vec();
    for p in bgra.as_chunks_mut::<4>().0 {
        let l = (0.114 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.299 * f32::from(p[2])).round() as u8;
        (p[0], p[1], p[2]) = (l, l, l);
    }
    let grey = olo_kit::pictures::from_bgra(size.width.0 as u32, size.height.0 as u32, bgra)?;
    *MADE.lock().unwrap_or_else(|e| e.into_inner()) = Some((name.to_owned(), px, grey.clone()));
    Some(grey)
}

/// Bumped while an icon the empty list shows is being loaded: in its
/// drawing's key, so that it's drawn again once the icon is there.
static ICONS_SHOWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether `e` is about a file, its picture shown large (a screenshot's,
/// a recording's): what's said under its name is the file's details.
fn about_file(e: &store::Entry) -> bool {
    e.look.file.is_some() && e.look.image.is_some() && e.look.style == olo_protocol::NoteStyle::Standard
}

/// Between a row's title and what's under it: a file's details right
/// under its name (read as one with it, as on its banner); a message's
/// words a point under.
fn row_body_gap(e: &store::Entry) -> f32 {
    if about_file(e) { 0.0 } else { 1.0 }
}

/// A row's quieter ink (its time, whose it is, a file's details): nearer
/// the selection's own text while selected (at the plain rows' mix they
/// were faint on the dark selection).
fn row_dim(fg: olo_theme::Rgba, bg: olo_theme::Rgba, selected: bool) -> olo_kit::Hsla {
    style::c(olo_theme::mix(fg, bg, if selected { 0.3 } else { 0.45 }))
}

/// A row's or a card's buttons laid out across `left`..`right` as a
/// banner's are (`banners::lay_buttons`): the one set apart (a Delete)
/// at `left`, the rest from `right` back in their order, as many as fit
/// from the first, `gap` apart, none nearer the one apart than
/// `m::SPACING`; what doesn't fit is in the menu. The one apart only with
/// at least the first of the rest beside it (alone, a Delete would stand
/// where the one reached for goes); the first too wide even alone, cut
/// short as it reads (`fit`: its label for a width), never past `left`.
/// Each (x, key, label, width).
fn lay_list_buttons(buttons: &[(String, String, f32)], apart: Option<&str>, left: f32, right: f32, gap: f32, fit: impl Fn(&str, f32) -> String) -> Vec<(f32, String, String, f32)> {
    let lead = apart.and_then(|a| buttons.iter().find(|(k, ..)| k == a));
    let rest: Vec<&(String, String, f32)> = buttons.iter().filter(|(k, ..)| apart != Some(k.as_str())).collect();
    let Some(first) = rest.first() else { return Vec::new() };
    // (How many of the rest fit from `from` to the right end, and the
    // room they take.)
    let fits = |from: f32| {
        let (mut used, mut n) = (0.0, 0);
        for (i, (_, _, w)) in rest.iter().enumerate() {
            let next = used + if i > 0 { gap } else { 0.0 } + w;
            if from + next > right + 0.01 {
                break;
            }
            (used, n) = (next, n + 1);
        }
        (n, used)
    };
    let beside = lead.map(|(_, _, w)| fits(left + w + m::SPACING)).filter(|(n, _)| *n > 0);
    let ((n, used), lead) = match beside {
        Some(laid) => (laid, lead),
        None => (fits(left), None),
    };
    let mut out: Vec<(f32, String, String, f32)> = lead.map(|(k, l, w)| (left, k.clone(), l.clone(), *w)).into_iter().collect();
    if n == 0 {
        let w = (right - left).max(0.0);
        out.push((left, first.0.clone(), fit(&first.1, w), w));
        return out;
    }
    let mut x = right - used;
    for (i, (k, l, w)) in rest.iter().take(n).enumerate() {
        if i > 0 {
            x += gap;
        }
        out.push((x, k.clone(), l.clone(), *w));
        x += w;
    }
    out
}

/// What's selected once `which` (one note, or one conversation, drawn
/// in `order` as its newest's row) is gone: the one drawn after it, else
/// before it, not itself gone. (Many rows at once, a group's: nothing.)
fn next_after(which: &[store::EntryKey], order: &[store::EntryKey]) -> Option<store::EntryKey> {
    let first = *which.first()?;
    let s = order.iter().position(|k| *k == first)?;
    if which[1..].iter().any(|k| order.contains(k)) {
        return None;
    }
    let left = |k: &&store::EntryKey| !which.contains(k);
    order[s + 1..].iter().find(left).or_else(|| order[..s].iter().rev().find(left)).copied()
}

impl Notifications {
    /// The very entry `key` of the list read or unread (one put back
    /// beside one sent again since with its number: that one, not the
    /// other).
    pub fn set_read_of(&mut self, key: &store::EntryKey, read: bool) {
        let Some(at) = self.history.entries.iter().position(|e| key.matches(e)) else { return };
        if self.history.entries[at].unread == read {
            self.history.entries_mut()[at].set_unread(!read, crate::now_secs());
            self.publish(true);
            self.changed();
        }
    }

    /// Entry `key`'s picture was dropped somewhere as its file: used,
    /// so read; and if it's the note its banner is of (the newest with
    /// its number), the banner leaves the screen too (`dropped`). An
    /// older one put back beside it is read alone.
    pub fn dropped_entry(&mut self, key: &store::EntryKey) {
        if self.history.newest(key.id).is_some_and(|e| key.matches(e)) {
            self.dropped(key.id);
        } else {
            self.set_read_of(key, true);
        }
    }

    /// Select the note whose entry is `uid` (opened under the rest).
    pub fn select(&mut self, uid: u64) {
        self.set_selected(Some(uid));
        self.changed();
    }

    /// The selection from now (an entry's `uid`, or none). The one open
    /// before it, if it was unread, is read now that it's closed, and
    /// keeps its place while the window is open: open, it stays as it
    /// was, bold with its dot under "Unread" (read as it opened, it sat
    /// read under an "Unread" that no longer counted it).
    pub fn set_selected(&mut self, uid: Option<u64>) {
        let was = self.view.selected;
        self.view.selected = uid;
        if was == uid || self.view.archived {
            return;
        }
        if let Some(key) = was.and_then(|w| self.history.entries.iter().find(|e| e.uid == w && e.unread)).map(store::EntryKey::of) {
            self.view.held.insert(key.id);
            self.set_read_of(&key, true);
        }
    }

    /// The selected entry, in the list shown.
    pub fn selected_entry(&self) -> Option<&crate::Entry> {
        let uid = self.view.selected?;
        self.listed().entries.iter().find(|e| e.uid == uid)
    }

    /// Whether the selection is note `id`'s (or no longer anything's: an
    /// entry gone), to be let go of as that note goes.
    pub fn selected_is(&self, id: u32) -> bool {
        self.view.selected.is_some_and(|uid| self.history.entries.iter().chain(self.archive.entries.iter()).find(|e| e.uid == uid).is_none_or(|e| e.id == id))
    }

    /// Fold an app's group in By App, or unfold it.
    pub fn fold(&mut self, app: String) {
        if !self.view.folded.remove(&app) {
            self.view.folded.insert(app);
        }
        self.changed();
    }

    /// Forget the entries `which`, as they were drawn (each found again
    /// now by its key: one that has left the list since, or moved in it,
    /// is never another's to forget); `order`, the list as it was shown
    /// (its entries' keys), says what is selected next: the one after
    /// them, else before, as it is now. What is still open at its app
    /// closes.
    pub fn forget(&mut self, which: &[store::EntryKey], order: &[store::EntryKey]) {
        let mut at: Vec<usize> = which.iter().filter_map(|k| self.listed().entries.iter().position(|e| k.matches(e))).collect();
        // From the end, so the indices stay good.
        at.sort_unstable();
        at.dedup();
        if at.is_empty() {
            return;
        }
        let next = next_after(which, order).and_then(|k| self.listed().entries.iter().find(|e| k.matches(e))).map(|e| e.uid);
        // (In the archive: deleted from it.)
        if self.view.archived {
            let had = self.view.selected.is_some();
            self.delete_archived(Some(at));
            if had {
                self.view.selected = next;
            }
            return;
        }
        let gone = self.history.take_out(|i, _| at.binary_search(&i).is_err());
        for (_, e) in &gone {
            if e.live {
                self.take(e.id);
                self.queue_closed(e.id, Reason::Dismissed);
            }
        }
        if !gone.is_empty() {
            let archive_changed = self.archive.archive(gone.iter().map(|(_, e)| (e.clone(), store::Why::Cleared)), crate::now_secs());
            if archive_changed {
                self.archived_changed();
            }
            self.steps.done("Clear", crate::Gone::Forgot { entries: gone, batch: None, only_eligible: true });
            store::History::save_pair(&self.archive, &self.history, None);
        }
        if self.view.selected.is_some() {
            self.view.selected = next;
        }
        self.publish(false);
        self.changed();
    }

    /// Run a history entry's action (`None`: its default). Still open at
    /// the app, that is what a click on it does there; otherwise the app
    /// comes forward.
    pub fn open_entry(&mut self, window: Option<&WindowCx>, i: usize, key: Option<String>) {
        let Some(e) = self.listed().entries.get(i).cloned() else { return };
        // One in the archive: what it's about, if it said how to show
        // that; else its app.
        if self.view.archived {
            if !crate::show_what(e.desktop_entry.as_deref(), &e.show) {
                raise_app(e.desktop_entry.as_deref(), &e.app);
            }
            return;
        }
        self.open_note(window, e, key);
    }

    /// Run history entry `e`'s action (`None`: its default), whichever
    /// list the window shows (`open_entry`'s, and by its id from the
    /// Inbox or the command line). Still open at the app, that is what a
    /// click on it does there; otherwise what it's about, else its app.
    pub fn open_note(&mut self, window: Option<&WindowCx>, e: crate::Entry, key: Option<String>) {
        let key = key.unwrap_or_else(|| "default".into());
        // (It keeps its place among the unread while the window shows
        // them; opened from elsewhere, an agent's or the Inbox's, with no
        // window, there's no place to keep: it sat among the unread,
        // read, when the window next opened.)
        if e.unread && self.history_win.is_some() && !self.view.archived {
            self.view.held.insert(e.id);
        }
        // An older one, put back beside one sent again since with its
        // number (no longer open at its app, nor on the screen): what it
        // says it does itself, from what it holds, never by its number,
        // which finds the newer one's (another file, another banner).
        let this = store::EntryKey::of(&e);
        if !self.history.newest(e.id).is_none_or(|n| this.matches(n)) {
            // (Checked against its key, file and receipt as it's run, as
            // `runs_of` checks a note's.)
            let checked = e.runs_for(&key).filter(|a| crate::relaunch_for(e.desktop_entry.as_deref(), &key, a, e.look.file.as_deref(), e.look.file_id));
            match checked {
                Some(args) => {
                    if !crate::act_what(e.desktop_entry.as_deref(), &key, args) {
                        raise_app(e.desktop_entry.as_deref(), &e.app);
                    }
                }
                None if key == "default" && crate::show_what(e.desktop_entry.as_deref(), &e.show) => {}
                None => raise_app(e.desktop_entry.as_deref(), &e.app),
            }
            self.set_read_of(&this, true);
            return;
        }
        // (Open at its app, any of its actions; after, those that run
        // something of their own: `invoke` starts that.)
        if e.actions_now().iter().any(|(k, _)| *k == key) {
            self.invoke(window, e.id, key);
        } else {
            self.set_read(e.id, true);
            // (Its banner, if it's on the screen still, goes with it: seen
            // to here. A screenshot's opened from the list, its program
            // gone, stayed on the screen, and "shown" in the list.)
            if self.take(e.id)
                && let Some(shown) = self.history.newest_mut(e.id)
            {
                shown.shown = false;
                self.publish(true);
                self.changed();
            }
            // No longer open at its app (the app closed it as things
            // moved on, or one of the two started again since): what it's
            // about all the same, if it said how to show that; else its
            // app. (An agent's note clicked after the agent had gone on
            // brought Terminal forward as it was last left, not the
            // window and tab the note was about.)
            if key != "default" || !crate::show_what(e.desktop_entry.as_deref(), &e.show) {
                raise_app(e.desktop_entry.as_deref(), &e.app);
            }
        }
    }

    /// The selected note read or unread (Edit menu); whether there is one.
    pub fn toggle_read(&mut self) -> bool {
        if self.view.archived {
            return false;
        }
        let Some((key, unread)) = self.selected_entry().map(|e| (store::EntryKey::of(e), e.unread)) else { return false };
        self.view.held.insert(key.id);
        self.set_read_of(&key, unread);
        true
    }

    /// Whether the selected note is unread (None: nothing selected).
    pub fn selected_unread(&self) -> Option<bool> {
        self.selected_entry().filter(|_| !self.view.archived).map(|e| e.unread)
    }

    /// Show the history this way from now on.
    pub fn set_view(&mut self, view: HistoryView) {
        self.refresh_settings();
        self.settings.view = view;
        self.save_settings();
        self.view.reveal = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, app: &str, summary: &str, time: u64) -> store::Entry {
        store::Entry { id, app: app.into(), summary: summary.into(), time, ..Default::default() }
    }

    #[test]
    fn views_group_as_they_say() {
        let now = now_secs();
        let entries = vec![entry(1, "Mail", "Lunch", now - 90_000), entry(2, "Chat", "Ann", now - 60), entry(3, "Mail", "Lunch", now - 30), entry(4, "Chat", "Bo", now)];
        let shown: Vec<usize> = (0..entries.len()).rev().collect();
        let notes = |items: &[Item]| items.iter().filter(|l| matches!(l, Item::Note { .. })).count();
        // List: under Today, then Yesterday's.
        let list = items(&entries, &shown, 0, HistoryView::List, &BTreeSet::new(), false);
        assert!(matches!(&list[0], Item::Day(d) if d == "Today"));
        assert!(matches!(list.last(), Some(Item::Note { i: 0, .. })));
        assert_eq!(notes(&list), 4);
        // By app: Chat (newest) first; folding hides its notes.
        let folded: BTreeSet<String> = ["Chat".to_owned()].into();
        let apps = items(&entries, &shown, 0, HistoryView::Apps, &folded, false);
        assert!(matches!(&apps[0], Item::Group { app, count: 2, open: false, .. } if app == "Chat"));
        assert_eq!(notes(&apps), 2);
        // ...but not while searching.
        assert_eq!(notes(&items(&entries, &shown, 0, HistoryView::Apps, &folded, true)), 4);
        // Conversations: Mail's two Lunches as one, the older under it.
        let threads = items(&entries, &shown, 0, HistoryView::Conversations, &BTreeSet::new(), false);
        assert_eq!(notes(&threads), 3);
        assert!(threads.iter().any(|l| matches!(l, Item::Note { i: 2, older, .. } if older == &vec![0])));
    }

    /// What's selected once a note or a conversation is cleared is found by the keys of what was drawn, never
    /// by places in a list that has changed since (a place past the shortened list was read): the one drawn
    /// after it, else before it, never one of those that went; many rows at once leave nothing selected.
    #[test]
    fn the_next_selected_is_found_by_its_key() {
        let k = |id: u32, uid: u64| store::EntryKey { id, time: u64::from(id), uid };
        let order = [k(4, 40), k(3, 30), k(2, 20), k(1, 10)];
        assert_eq!(next_after(&[k(3, 30)], &order), Some(k(2, 20)));
        assert_eq!(next_after(&[k(1, 10)], &order), Some(k(2, 20)), "the last: the one before it");
        // A conversation (drawn as its newest's row, the older under it, not in the order): after it.
        assert_eq!(next_after(&[k(3, 30), k(9, 9), k(8, 8)], &order), Some(k(2, 20)));
        // A group's notes, many rows: nothing.
        assert_eq!(next_after(&[k(3, 30), k(2, 20)], &order), None);
        // Two of the Cleared list with one number (a note cleared, sent again and cleared again): told apart.
        let twins = [k(5, 51), k(5, 50), k(6, 60)];
        assert_eq!(next_after(&[k(5, 51)], &twins), Some(k(5, 50)));
        assert!(!k(5, 51).matches(&store::Entry { id: 5, time: 5, uid: 50, ..Default::default() }));
        // Something no longer drawn: nothing.
        assert_eq!(next_after(&[k(7, 70)], &order), None);
        assert_eq!(next_after(&[], &order), None);
    }

    /// A row's and a card's buttons as a banner's: the one apart (Delete) at the start, the rest at the far
    /// end in their order, as many as fit from the first (not the last: `.rev()` kept Show in Tracker and
    /// dropped Copy); never nearer Delete than the spacing; Delete only beside another; one too wide alone
    /// cut short to its place, never wider than it (its whole label made the button run over the next card).
    #[test]
    fn list_buttons_are_laid_as_a_banners() {
        let b = |k: &str, w: f32| (k.to_owned(), k.to_uppercase(), w);
        let buttons = [b("copy", 60.0), b("show", 110.0), b("delete", 60.0)];
        let fit = |l: &str, w: f32| format!("{}…{w}", &l[..1]);
        let laid = |right: f32| lay_list_buttons(&buttons, Some("delete"), 0.0, right, m::TIGHT, fit).into_iter().map(|(x, k, l, w)| (x, k, l, w)).collect::<Vec<_>>();
        // Room for all: Delete at the start, Copy · Show in Tracker against the end, 6 points apart.
        assert_eq!(laid(300.0), vec![(0.0, "delete".into(), "DELETE".into(), 60.0), (124.0, "copy".into(), "COPY".into(), 60.0), (190.0, "show".into(), "SHOW".into(), 110.0)]);
        // Less: the first of the rest kept (Copy), the next in the menu; still the spacing clear of Delete.
        let some = laid(200.0);
        assert_eq!(some.iter().map(|(_, k, ..)| k.as_str()).collect::<Vec<_>>(), ["delete", "copy"]);
        assert!(some[1].0 >= 60.0 + m::SPACING && some[1].0 + some[1].3 <= 200.0);
        // Not even Copy beside Delete: Delete goes to the menu, Copy stays.
        assert_eq!(laid(100.0).iter().map(|(x, k, ..)| (*x, k.as_str())).collect::<Vec<_>>(), [(40.0, "copy")]);
        // Copy alone too wide: cut short to the room, never past its start.
        assert_eq!(laid(50.0), vec![(0.0, "copy".into(), "C…50".into(), 50.0)]);
        // A Delete alone is no button (in the menu); none set apart: all from the end.
        assert!(lay_list_buttons(&[b("delete", 60.0)], Some("delete"), 0.0, 300.0, m::TIGHT, fit).is_empty());
        let plain = lay_list_buttons(&buttons[..2], None, 0.0, 300.0, m::TIGHT, fit);
        assert_eq!(plain.iter().map(|(x, k, ..)| (*x, k.as_str())).collect::<Vec<_>>(), [(124.0, "copy"), (190.0, "show")]);
    }

    #[test]
    fn search_takes_every_word() {
        let mut e = entry(1, "Mail", "Lunch on Friday", 0);
        e.text = "At the usual place".into();
        assert!(found(&e, "mail usual"));
        assert!(found(&e, "  FRIDAY "));
        assert!(!found(&e, "lunch dinner"));
    }
}
