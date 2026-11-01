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
//! what is in view. Clicks are found by where things were drawn.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

use olo_kit_base::{masonry, metrics as m};
use olo_protocol::{CardSize, HistoryView, NoteStyle};
use olo_ui::{
    Menu, MenuBar, agent,
    gpui::{
        self, App, Bounds, BoxShadow, Context, Corners, Entity, FocusHandle, Focusable as _, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Point, Render,
        ScrollHandle, StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div, point, px, size,
    },
    style::{self, Align},
    theme,
    widgets::{Button, CheckBox, ScrollView, TextControl, TextField, TextFieldEvent, scroll::SCROLL_BAR, text_field},
};

use crate::{Notifications, look, markup::Span, named_icons, now_secs, paint_mark, pictures, raise_app, server::Reason, store, web};

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
/// A card's first line (whose it is and when), its app's mark on it,
/// the room an unread dot takes before the time, and the gap under it.
const CARD_HEAD: f32 = 14.0;
const CARD_MARK: f32 = 14.0;
const CARD_DOT: f32 = 11.0;
const CARD_HEAD_GAP: f32 = 7.0;
/// A card's action buttons.
const CARD_BUTTON: f32 = 20.0;
/// Room for the × at a row's right end.
const CLOSE_W: f32 = 18.0;
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
    /// The selected note's id.
    pub selected: Option<u32>,
    /// The apps folded away in By App.
    pub folded: BTreeSet<String>,
    /// Read while the window is open: they keep their places among the
    /// unread until it closes, so nothing moves from under the pointer.
    pub held: HashSet<u32>,
    /// Bring the selection into view at the next frame.
    reveal: bool,
}

/// One line of the list.
enum Item {
    /// The unread ones' heading, and how many.
    Unread(usize),
    Day(String),
    Group { app: String, count: usize, members: Vec<usize>, open: bool },
    /// The entry at `i`; `older` are the rest of its conversation.
    Note { i: usize, older: Vec<usize>, grouped: bool },
}

/// An entry's menu (a right click on it).
fn entry_menu(e: &store::Entry) -> Menu {
    let menu = Menu::new("Notification").item("Open", "open").separator().item(if e.unread { "Mark as Read" } else { "Mark as Unread" }, "toggle-read").separator();
    crate::note_menu(menu, Some(e.pinned)).separator().item("Forget", "forget")
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

/// A card face's colours (`style::card_ink`).
fn card_ink(p: &olo_theme::Palette, e: &store::Entry) -> look::Ink {
    let card = style::card_ink(p);
    look::Ink {
        text: card.text,
        soft: style::c(olo_theme::mix(p.document_text, p.document_background, 0.15)),
        dim: card.dim,
        link: style::c(p.keyboard_navigation),
        balloon: card.well,
        balloon_edge: card.edge,
        accent: e.look.accent.map(|[r, g, b]| [r, g, b, 255]),
    }
}

/// An entry's picture, from its file.
fn picture(e: &store::Entry) -> Option<pictures::Source> {
    e.picture.as_deref().map(|p| pictures::Source::File(p.into()))
}

/// What a note in the history shows.
fn entry_face(e: &store::Entry) -> look::Face<'_> {
    look::Face { title: if e.summary.is_empty() { app_name(e) } else { &e.summary }, body: &e.body, look: &e.look, progress: e.progress.filter(|_| e.live), picture: picture(e), failed: failed(e) }
}

/// A heading's band: a shade darker than the list.
fn heading_fill(window: &mut Window, p: &olo_theme::Palette, r: Bounds<Pixels>) {
    window.paint_quad(gpui::fill(r, style::tint(p.document_background, 1.04)));
    style::hline_above(window, r.left(), r.right(), r.bottom(), style::tint(p.document_background, 1.1));
}

/// The icon a note shows, in `r`: its sender's picture, round, for a
/// message; else its app's (or the one it names), or the generic app's,
/// as its banner's header shows it (not a warning or an info mark: the
/// title of a failure already starts with its stop sign).
fn paint_icon(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, e: &store::Entry, person: bool) {
    if person
        && e.look.style == NoteStyle::Message
        && let Some(src) = picture(e)
        && let Some(img) = pictures::get(cx, &src, look::device(window, r), pictures::Shape::Cover)
    {
        let _ = window.paint_image(r, Corners::all(r.size.width / 2.0), img, 0, false);
        return;
    }
    let names = named_icons(e.icon.as_deref(), e.desktop_entry.as_deref(), &e.app);
    paint_mark(window, cx, r, &e.app, e.look.accent, e.via.is_some(), &names);
}

/// The mark of what is unread: a dot in the colour of what the keyboard
/// is on (not the tab's yellow, which would repeat the window's own on
/// every note).
fn unread_dot(window: &mut Window, center: Point<Pixels>, p: &olo_theme::Palette) {
    window.paint_sector(center, px(0.0), px(3.5), 0.0, std::f32::consts::TAU, style::c(p.keyboard_navigation));
}

/// A card's paper, its shadow under it, as the egui kit's `style::card`.
fn paint_card(window: &mut Window, r: Bounds<Pixels>, p: &olo_theme::Palette, hover: bool, selected: bool) {
    let ink = style::card_ink(p);
    let r = style::snap(window, r);
    let radius = px(m::CARD_RADIUS);
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let (y, blur, a) = if selected { (2.0, 10.0, if dark { 110 } else { 40 }) } else { (1.0, 4.0, if dark { 70 } else { 22 }) };
    let shadow = BoxShadow { color: style::c([0, 0, 0, a]), offset: point(px(0.0), px(y)), blur_radius: px(blur / 2.0), spread_radius: px(0.0) };
    window.paint_shadows(r, Corners::all(radius), &[shadow]);
    let (fill, edge) = if selected { (style::c(p.list_selected_background), style::c(p.selection_edge())) } else { (if hover { ink.hover } else { ink.card }, ink.edge) };
    window.paint_quad(gpui::fill(r, fill).corner_radii(radius).border_widths(style::hairline(window)).border_color(edge));
}

/// A row laid out.
#[derive(Clone)]
struct RowLaid {
    title: look::Text,
    body: Option<look::Text>,
    /// The rest of a conversation: when, and a line of each.
    older: Vec<(String, look::Text)>,
    /// Its buttons, when selected: (key, label, width); an empty key
    /// dismisses it.
    buttons: Vec<(String, String, f32)>,
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
}

/// A card measured: how many columns it spans, how tall it is, its face.
#[derive(Clone)]
struct CardLaid {
    height: f32,
    face: look::Laid,
    ink: look::Ink,
    /// Its app's actions, while it's open there: (key, label, width).
    buttons: Vec<(String, String, f32)>,
}

#[derive(Clone, PartialEq)]
struct CardKey {
    entry: store::Entry,
    span: usize,
    width: u32,
    scale: u32,
    dark: bool,
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
    entries: Rc<Vec<store::Entry>>,
    view: HistoryView,
    cards_view: bool,
    searching: bool,
    /// The notes in the order they're listed (as `entries` indices).
    order: Rc<Vec<usize>>,
    lines: Rc<Vec<(Item, f32, f32)>>,
    rows: Rc<HashMap<usize, RowLaid>>,
    cards: Rc<Vec<(usize, CardLaid, masonry::Place)>>,
    total: f32,
    sel_id: Option<u32>,
    /// The selected row's index, top and the rest of its conversation.
    sel_row: Option<(usize, f32, Vec<usize>)>,
    /// How many show, of how many; unread; snoozed.
    shown: usize,
    all: usize,
    unread: u32,
    snoozed: usize,
    can_clear: bool,
}

/// What the list was laid out for, besides the notes.
#[derive(PartialEq)]
struct LaidKey {
    width: u32,
    scale: u32,
    palette: std::sync::Arc<olo_theme::Palette>,
}

/// What takes a click where, as last drawn (later ones on top).
type Hits = Rc<RefCell<Vec<(Bounds<Pixels>, Hit)>>>;

pub struct HistoryWindow {
    app: Entity<Notifications>,
    search: Entity<TextField>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    rows: HashMap<u32, (RowKey, RowLaid)>,
    cards: HashMap<u32, (CardKey, CardLaid)>,
    /// Where things were drawn in the last frame, in the list's own
    /// coordinates; later ones on top.
    hits: Hits,
    /// What the pointer is over.
    hover: Option<Hit>,
    /// Where each card was placed (the arrow keys go by it), by entry.
    places: Vec<(u32, masonry::Place)>,
    laid: Option<(LaidKey, Rc<ListLaid>)>,
    /// The notes, the view or the selection changed since: lay it out
    /// again.
    stale: bool,
    _subs: Vec<Subscription>,
}

impl HistoryWindow {
    pub fn new(app: Entity<Notifications>, window: &mut Window, cx: &mut Context<Self>) -> HistoryWindow {
        let typed = app.read(cx).view.search.clone();
        let search = text_field(cx, |f| f.name("Search").hint("Search"));
        search.update(cx, |f, cx| f.set_text(typed, cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        let observe = cx.observe(&app, |me: &mut HistoryWindow, _, cx| {
            me.stale = true;
            cx.notify();
        });
        let typing = cx.subscribe(&search, |me: &mut HistoryWindow, field, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Changed) {
                let text = field.read(cx).text().to_owned();
                me.app.update(cx, |n, cx| {
                    n.view.search = text;
                    cx.notify();
                });
            }
        });
        HistoryWindow { app, search, scroll: ScrollHandle::new(), focus, rows: HashMap::new(), cards: HashMap::new(), hits: Rc::default(), hover: None, places: Vec::new(), laid: None, stale: true, _subs: vec![observe, typing] }
    }

    fn menus(&self, cx: &App) -> Vec<Menu> {
        let n = self.app.read(cx);
        let view = n.settings.view;
        let mut views = Menu::new("View");
        for (v, label, command, key) in VIEWS {
            views = views.item(label, command).key(key).checked(view == v);
        }
        let selected = n.selected_unread();
        let pinned = n.view.selected.and_then(|id| n.history.newest(id)).map(|e| e.pinned);
        let edit = Menu::new("Edit")
            .item("Find", "find")
            .key("f")
            .separator()
            .item(if selected == Some(false) { "Mark as Unread" } else { "Mark as Read" }, "toggle-read")
            .key("u")
            .enabled(selected.is_some())
            .item("Mark All as Read", "read-all")
            .enabled(n.history.unread() > 0)
            .separator();
        let edit = crate::note_menu(edit, pinned).separator().item("Clear All", "clear").enabled(n.history.entries.iter().any(|e| !e.shown && !e.pinned && !e.snoozed()));
        vec![Menu::new("File").item("Close", "close").key("w"), edit, views]
    }

    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.stale = true;
        let app = self.app.clone();
        match command {
            "close" => app.update(cx, |n, cx| n.close_history(cx)),
            "clear" => app.update(cx, |n, cx| n.clear_history(cx)),
            "find" => {
                let f = self.search.read(cx).focus_handle(cx);
                window.focus(&f);
            }
            "toggle-read" => {
                app.update(cx, |n, cx| n.toggle_read(cx));
            }
            "read-all" => app.update(cx, |n, cx| {
                let unread: Vec<u32> = n.history.entries.iter().filter(|e| e.unread).map(|e| e.id).collect();
                n.view.held.extend(unread);
                n.mark_all_read(cx);
            }),
            // The selected note's.
            c if c == "pin" || c.starts_with("snooze:") => app.update(cx, |n, cx| {
                if let Some(id) = n.view.selected {
                    n.note_command(cx, id, c);
                }
            }),
            _ => {
                if let Some((v, ..)) = VIEWS.iter().find(|(_, _, c, _)| *c == command) {
                    let v = *v;
                    app.update(cx, |n, cx| {
                        n.set_view(v);
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    /// A row laid out for `width` (kept while what it shows is the same).
    #[allow(clippy::too_many_arguments)]
    fn row(&mut self, window: &mut Window, p: &olo_theme::Palette, entries: &[store::Entry], i: usize, older: &[usize], meta: String, selected: bool, indent: bool, width: f32) -> RowLaid {
        let e = &entries[i];
        let key = RowKey {
            entry: e.clone(),
            older: older.iter().map(|&o| entries[o].id).collect(),
            meta: meta.clone(),
            selected,
            indent,
            width: width as u32,
            scale: (window.scale_factor() * 1000.0) as u32,
            dark: p.mode == olo_theme::ThemeMode::Dark,
        };
        if let Some((k, laid)) = self.rows.get(&e.id)
            && *k == key
        {
            return laid.clone();
        }
        let (fg, bg) = if selected { (p.list_selected_item_text, p.list_selected_background) } else { (p.document_text, p.document_background) };
        let soft = style::c(olo_theme::mix(fg, bg, 0.2));
        let x0 = if indent { 8.0 } else { 0.0 };
        let text_x = x0 + TEXT_X;
        let meta_w = f32::from(style::text_width(window, meta, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let title_w = (width - text_x - CLOSE_W - meta_w - 8.0).max(40.0);
        let body_w = width - text_x - CLOSE_W;
        let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
        // Unread: bold, as in a mailbox.
        let weight = if e.unread || selected { FontWeight::BOLD } else { FontWeight::NORMAL };
        let title = look::title(window, title_text, failed(e), style::PLAIN_SIZE, style::ui_font(weight), style::c(fg), title_w, if selected { 3 } else { 1 });
        let body = (!e.text.is_empty()).then(|| {
            // Unselected, the body is one line: its breaks as spaces.
            let spans: Vec<Span> = e.body.iter().map(|s| Span { text: if selected { s.text.clone() } else { s.text.replace('\n', " ") }, bold: s.bold, italic: s.italic, underline: false, link: None }).collect();
            let ink = look::Ink { text: style::c(fg), soft, dim: soft, link: soft, balloon: soft, balloon_edge: soft, accent: None };
            look::body(window, &spans, &ink, body_w, if selected { 12 } else { 1 })
        });
        // The rest of a conversation, under the newest, when selected.
        let older: Vec<(String, look::Text)> = if selected {
            older.iter().map(|&o| (crate::when(entries[o].time), look::text(window, &entries[o].text.replace('\n', " "), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), soft, body_w - 44.0, 1))).collect()
        } else {
            Vec::new()
        };
        let buttons: Vec<(String, String, f32)> = if !selected {
            Vec::new()
        } else {
            let mut actions: Vec<(String, String)> = if e.live {
                // The default action is Open, whatever the app called it ("Activate").
                e.actions.iter().filter(|(k, l)| k == "default" || !l.is_empty()).map(|(k, l)| (k.clone(), if k == "default" { "Open".to_owned() } else { l.clone() })).collect()
            } else {
                vec![("default".into(), format!("Show {}", app_name(e)))]
            };
            // Dismiss last: the notification, or the whole conversation.
            actions.push((String::new(), "Dismiss".to_owned()));
            actions.into_iter().map(|(k, l)| (k, l.clone(), (f32::from(style::text_width(window, l, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))) + 2.0 * m::CONTROL_PAD).max(m::BUTTON_MIN_WIDTH * 0.75).ceil())).collect()
        };
        let mut h = ROW_PAD + title.height + body.as_ref().map_or(0.0, |b| 1.0 + b.height);
        let older_h = style::SMALL_SIZE + 5.0;
        if !older.is_empty() {
            h += m::TIGHT + older.len() as f32 * older_h;
        }
        if selected {
            h += m::TIGHT + m::CONTROL_HEIGHT + 2.0;
        }
        // On the 2-point grid (whole device pixels at 1.5×), so the rows
        // under it start on one too.
        let height = ((h + ROW_PAD).max(ROW_PAD * 2.0 + SMALL_ICON + 4.0) / 2.0).ceil() * 2.0;
        let laid = RowLaid { title, body, older, buttons, height };
        self.rows.insert(e.id, (key, laid.clone()));
        laid
    }

    /// A card measured `width` wide.
    fn card(&mut self, window: &mut Window, p: &olo_theme::Palette, e: &store::Entry, span: usize, width: f32) -> CardLaid {
        let key = CardKey { entry: e.clone(), span, width: width as u32, scale: (window.scale_factor() * 1000.0) as u32, dark: p.mode == olo_theme::ThemeMode::Dark };
        if let Some((k, laid)) = self.cards.get(&e.id)
            && *k == key
        {
            return laid.clone();
        }
        let ink = card_ink(p, e);
        let small = e.look.size == CardSize::Small;
        let room = look::Room {
            width: width - 2.0 * m::CARD_INSET,
            title_rows: if span > 1 { 2 } else { 3 },
            body_rows: if small { 1 } else if span > 1 { WIDE_ROWS } else { CARD_ROWS },
            title_reserve: 0.0,
            picture: 28.0,
            image_max: if span > 1 { 180.0 } else { 120.0 },
            fields_max: if span > 1 { 8 } else { 4 },
        };
        let face = look::lay(window, &entry_face(e), &ink, &room);
        let buttons: Vec<(String, String, f32)> = if e.live {
            e.actions
                .iter()
                .filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(e.via.as_deref(), k))
                .map(|(k, l)| (k.clone(), l.clone(), (f32::from(style::text_width(window, l.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL))) + 16.0).max(56.0).min(room.width).ceil()))
                .collect()
        } else {
            Vec::new()
        };
        let mut height = m::CARD_INSET + CARD_HEAD + CARD_HEAD_GAP + face.height + m::CARD_INSET;
        if !buttons.is_empty() {
            height += m::SPACING + CARD_BUTTON;
        }
        let laid = CardLaid { height: height.ceil(), face, ink, buttons };
        self.cards.insert(e.id, (key, laid.clone()));
        laid
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>, order: &[usize], cards: bool) {
        self.stale = true;
        let key = e.keystroke.key.as_str();
        if e.keystroke.modifiers.modified() {
            return;
        }
        let typing = self.search.read(cx).focus_handle(cx).is_focused(window);
        let app = self.app.clone();
        let (selected, entries_ids): (Option<usize>, Vec<u32>) = {
            let n = app.read(cx);
            let ids: Vec<u32> = order.iter().map(|&i| n.history.entries[i].id).collect();
            (n.view.selected.and_then(|id| ids.iter().position(|&x| x == id)), ids)
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
                        let places: Vec<masonry::Place> = entries_ids.iter().map(|id| self.places.iter().find(|(p, _)| p == id).map_or_else(masonry::Place::default, |(_, r)| *r)).collect();
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
            let id = entries_ids[to];
            app.update(cx, |n, cx| {
                n.select(id, cx);
                n.view.reveal = true;
            });
            cx.stop_propagation();
            return;
        }
        let Some(s) = selected else {
            if key == "escape" {
                self.escape(window, cx);
                cx.stop_propagation();
            }
            return;
        };
        match key {
            "enter" => app.update(cx, |n, cx| n.open_entry(cx, Some(window), order[s], None)),
            "delete" | "backspace" if !typing => app.update(cx, |n, cx| n.forget(cx, vec![order[s]], order)),
            "left" | "right" if !typing && app.read(cx).settings.view == HistoryView::Apps => app.update(cx, |n, cx| {
                let a = n.history.entries[order[s]].app.clone();
                let folded = n.view.folded.contains(&a);
                if (key == "left") != folded {
                    n.fold(a, cx);
                }
            }),
            "escape" => self.escape(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Escape: the search clears, else the selection goes.
    fn escape(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.stale = true;
        if !self.search.read(cx).text().trim().is_empty() {
            self.search.update(cx, |f, cx| f.set_text("", cx));
        } else {
            self.app.update(cx, |n, cx| {
                n.view.selected = None;
                cx.notify();
            });
        }
    }

    /// What the pointer is on at `pos` (the list's own coordinates).
    fn hit_at(&self, pos: Point<Pixels>) -> Option<Hit> {
        self.hits.borrow().iter().rev().find(|(r, _)| r.contains(&pos)).map(|(_, h)| h.clone())
    }

    fn press(&mut self, e: &MouseDownEvent, origin: Point<Pixels>, order: Rc<Vec<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        self.stale = true;
        let Some(hit) = self.hit_at(e.position - origin) else { return };
        let app = self.app.clone();
        let select = e.modifiers.platform || e.modifiers.control || e.modifiers.shift;
        app.update(cx, |n, cx| match hit {
            Hit::Row(i) => {
                let id = n.history.entries[i].id;
                n.select(id, cx);
                if e.click_count >= 2 {
                    n.open_entry(cx, Some(window), i, None);
                }
            }
            Hit::Card(i) => {
                // One click acts, as on a banner; ⌘ or Shift only selects it.
                let id = n.history.entries[i].id;
                if select || e.click_count >= 2 {
                    n.select(id, cx);
                } else {
                    n.view.selected = Some(id);
                    n.open_entry(cx, Some(window), i, None);
                }
            }
            Hit::Forget(i) => n.forget(cx, vec![i], &order),
            Hit::Fold(a) => n.fold(a, cx),
            Hit::Dismiss(_, members) => n.forget(cx, members, &order),
            Hit::MarkAllRead => {
                let unread: Vec<u32> = n.history.entries.iter().filter(|e| e.unread).map(|e| e.id).collect();
                n.view.held.extend(unread);
                n.mark_all_read(cx);
            }
        });
        window.focus(&self.focus);
        cx.stop_propagation();
    }

    fn menu(&mut self, e: &MouseDownEvent, origin: Point<Pixels>, order: Rc<Vec<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        self.stale = true;
        let i = match self.hit_at(e.position - origin) {
            Some(Hit::Row(i) | Hit::Card(i) | Hit::Forget(i)) => i,
            _ => return,
        };
        let Some(entry) = self.app.read(cx).history.entries.get(i).cloned() else { return };
        let me = cx.entity();
        olo_ui::menu::context_menu(window, cx, entry_menu(&entry), e.position, move |command, window, cx| {
            let (command, order) = (command.to_owned(), order.clone());
            me.update(cx, |h, cx| {
                h.app.update(cx, |n, cx| {
                    let Some(i) = n.history.entries.iter().position(|x| x.id == entry.id) else { return };
                    match command.as_str() {
                        "open" => {
                            n.view.selected = Some(entry.id);
                            n.open_entry(cx, Some(window), i, None);
                        }
                        "toggle-read" => {
                            n.view.held.insert(entry.id);
                            let unread = n.history.entries[i].unread;
                            n.set_read(cx, entry.id, unread);
                        }
                        "forget" => n.forget(cx, vec![i], &order),
                        c => n.note_command(cx, entry.id, c),
                    }
                })
            });
        });
        cx.stop_propagation();
    }
}

/// The History window's views: View menu label, command and key.
const VIEWS: [(HistoryView, &str, &str, &str); 4] = [
    (HistoryView::List, "List", "view-list", "1"),
    (HistoryView::Apps, "By App", "view-apps", "2"),
    (HistoryView::Cards, "Cards", "view-cards", "3"),
    (HistoryView::Conversations, "Conversations", "view-conversations", "4"),
];

impl Render for HistoryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let me = cx.entity();
        let bar = MenuBar::new(self.menus(cx), {
            let me = me.clone();
            move |cmd, window, cx| me.update(cx, |h, cx| h.command(cmd, window, cx))
        });
        let (dnd, quiet, schedule_quiet, settings) = {
            let n = self.app.read(cx);
            (n.settings.dnd, n.quiet(), !n.settings.dnd, n.settings.clone())
        };
        let now = now_secs();
        let strength = move |e: &store::Entry| if e.pinned { 1.0 } else { settings.strength(now.saturating_sub(e.time)) };

        // The list, laid out for its width (or as it was).
        let w = (f32::from(window.viewport_size().width) - SCROLL_BAR - 2.0).max(100.0);
        let key = LaidKey { width: w.to_bits(), scale: window.scale_factor().to_bits(), palette: p.clone() };
        let list = match &self.laid {
            Some((k, l)) if !self.stale && *k == key => l.clone(),
            _ => {
                let l = Rc::new(self.lay_out(window, cx, &p, w));
                self.laid = Some((key, l.clone()));
                self.stale = false;
                l
            }
        };
        let (view, cards_view, sel_id, total, searching) = (list.view, list.cards_view, list.sel_id, list.total, list.searching);
        let order = list.order.clone();

        // The keyboard.
        let keys_order = order.clone();
        let content_focus = self.focus.clone();

        // What's drawn, in the list's coordinates.
        let hits = self.hits.clone();
        let hover = self.hover.clone();
        let p2 = p.clone();
        let (entries2, rows2, cards2, lines2) = (list.entries.clone(), list.rows.clone(), list.cards.clone(), list.lines.clone());
        let order2 = order.clone();
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = p2.clone();
                let mut hits = hits.borrow_mut();
                hits.clear();
                let view_box = window.content_mask().bounds;
                let at = |r: Bounds<Pixels>| r + b.origin;
                let local = |r: Bounds<Pixels>| r;
                let rows_total = order2.len();
                if cards_view {
                    for (k, (i, laid, pl)) in cards2.iter().enumerate() {
                        let e = &entries2[*i];
                        let r = style::snap(window, at(Bounds::new(point(px(pl.x), px(pl.y + m::SPACING)), size(px(pl.w), px(pl.h)))));
                        let lr = Bounds::new(point(px(pl.x), px(pl.y + m::SPACING)), size(px(pl.w), px(pl.h)));
                        hits.push((local(lr), Hit::Card(*i)));
                        if !r.intersects(&view_box) {
                            continue;
                        }
                        // Staggered in its column: level with a card beside
                        // it only by chance.
                        olo_ui::review::own_grid(window, r);
                        let hovered = hover.as_ref().is_some_and(|h| matches!(h, Hit::Card(x) | Hit::Forget(x) if x == i));
                        let is_sel = sel_id == Some(e.id);
                        let opacity = if !is_sel && !hovered { fade_opacity(strength(e)) } else { 1.0 };
                        window.with_element_opacity(Some(opacity), |window| {
                            paint_card(window, r, &p, hovered, is_sel);
                            let ink = style::card_ink(&p);
                            let inner = Bounds::from_corners(r.origin + point(px(m::CARD_INSET), px(m::CARD_INSET)), r.bottom_right() - point(px(m::CARD_INSET), px(m::CARD_INSET)));
                            // Whose and when: the app's mark and name, the time at the right.
                            let head = Bounds::new(inner.origin, size(inner.size.width, px(CARD_HEAD)));
                            let icon = Bounds::new(point(head.left(), head.center().y - px(CARD_MARK / 2.0)), size(px(CARD_MARK), px(CARD_MARK)));
                            paint_icon(window, cx, icon, e, false);
                            let time = if e.pinned { format!("Pinned · {}", crate::when(e.time)) } else { crate::when(e.time) };
                            let small = style::ui_font(FontWeight::NORMAL);
                            let time_w = f32::from(style::text_width(window, time.clone(), style::DETAIL_SIZE, small.clone()));
                            let name_x = icon.right() + px(5.0);
                            let name_room = head.right() - px(time_w + CARD_DOT + 8.0) - name_x;
                            let n = style::label_fit(window, cx, name_x, head.center().y, Align::Left, app_name(e).to_owned(), style::DETAIL_SIZE, style::ui_font(FontWeight::BOLD), ink.dim, name_room);
                            let baseline = style::baseline_at(window, head.center().y, style::DETAIL_SIZE);
                            let _ = n;
                            let forgettable = !e.shown && hovered;
                            if !forgettable {
                                style::label_on_baseline(window, cx, head.right(), baseline, Align::Right, time.clone(), style::DETAIL_SIZE, small.clone(), ink.dim);
                                if e.unread {
                                    unread_dot(window, point(head.right() - px(time_w + CARD_DOT / 2.0 + 1.0), head.center().y), &p);
                                }
                            } else {
                                // Forget it: a × where the time was.
                                let close = Bounds::centered_at(point(head.right() - px(6.0), head.center().y), size(px(12.0), px(12.0)));
                                let lit = hover == Some(Hit::Forget(*i));
                                style::close_box(window, close, if lit { ink.text } else { ink.dim });
                                hits.push((local(close.dilate(px(3.0)) - b.origin), Hit::Forget(*i)));
                                agent::record(window, cx, agent::Node::button("Forget", close));
                            }
                            look::paint(window, cx, &entry_face(e), &laid.face, &laid.ink, point(inner.left(), head.bottom() + px(CARD_HEAD_GAP)), f32::from(inner.size.width));
                        });
                        let title = if e.summary.is_empty() { app_name(e) } else { &e.summary };
                        agent::record(window, cx, agent::Node { role: "ListItem", name: Some(title.to_owned().into()), bounds: r, enabled: true, clickable: true, selected: Some(is_sel), ..Default::default() });
                        let _ = (k, rows_total);
                    }
                    return;
                }
                let mut row = 0;
                for (item, y, h) in lines2.iter() {
                    let lr = Bounds::new(point(px(0.0), px(*y)), size(b.size.width, px(*h)));
                    let r = at(lr);
                    let shown = r.intersects(&view_box);
                    match item {
                        Item::Unread(count) => {
                            if !shown {
                                continue;
                            }
                            heading_fill(window, &p, r);
                            let font = style::ui_font(FontWeight::BOLD);
                            let t = style::label(window, cx, r.left() + px(ICON_X), r.center().y, Align::Left, "Unread", style::SMALL_SIZE, font, style::disabled_text(&p));
                            agent::record(window, cx, agent::Node::label("Unread", t));
                            if *count == 0 {
                                continue;
                            }
                            let baseline = style::baseline_at(window, r.center().y, style::SMALL_SIZE);
                            style::label_on_baseline(window, cx, t.right() + px(5.0), baseline, Align::Left, count.to_string(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                            // Read them all, here where they are.
                            let all = "Mark All Read";
                            let tw = f32::from(style::text_width(window, all.to_owned(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL)));
                            // (Its right edge on the times' column under it.)
                            let br = Bounds::from_corners(point(r.right() - px(CLOSE_W + tw + 4.0), r.top() + px(2.0)), point(r.right() - px(CLOSE_W - 4.0), r.bottom() - px(3.0)));
                            let lit = hover == Some(Hit::MarkAllRead);
                            let ink = if lit { style::c(p.document_text) } else { style::disabled_text(&p) };
                            style::label(window, cx, r.right() - px(CLOSE_W), r.center().y, Align::Right, all, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                            hits.push((br - b.origin, Hit::MarkAllRead));
                            agent::record(window, cx, agent::Node::button(all, br));
                        }
                        Item::Day(label) => {
                            if !shown {
                                continue;
                            }
                            heading_fill(window, &p, r);
                            let t = style::label(window, cx, r.left() + px(ICON_X), r.center().y, Align::Left, label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::disabled_text(&p));
                            agent::record(window, cx, agent::Node::label(label.clone(), t));
                        }
                        Item::Group { app, count, members, open } => {
                            hits.push((lr, Hit::Fold(app.clone())));
                            if !shown {
                                continue;
                            }
                            heading_fill(window, &p, r);
                            let name = if app.is_empty() { "Notification" } else { app.as_str() };
                            let text = style::c(p.document_text);
                            let dim = style::disabled_text(&p);
                            let lit = hover.as_ref().is_some_and(|h| matches!(h, Hit::Fold(a) | Hit::Dismiss(a, _) if a == app));
                            style::latch(window, Bounds::centered_at(point(r.left() + px(7.0), r.center().y), size(px(10.0), px(10.0))), *open, if lit { text } else { dim });
                            paint_icon(window, cx, Bounds::centered_at(point(r.left() + px(ICON_X + SMALL_ICON / 2.0), r.center().y), size(px(SMALL_ICON), px(SMALL_ICON))), &entries2[members[0]], false);
                            let n = style::label(window, cx, r.left() + px(TEXT_X), r.center().y, Align::Left, name.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), text);
                            let baseline = style::baseline_at(window, r.center().y, style::PLAIN_SIZE);
                            style::label_on_baseline(window, cx, n.right() + px(6.0), baseline, Align::Left, count.to_string(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                            agent::record(window, cx, agent::Node { role: "Button", name: Some(format!("{name} ({count})").into()), bounds: r, enabled: true, clickable: true, selected: Some(*open), ..Default::default() });
                            if lit {
                                let small = style::ui_font(FontWeight::NORMAL);
                                let tw = f32::from(style::text_width(window, "Dismiss".to_owned(), style::DETAIL_SIZE, small.clone()));
                                let cr = Bounds::from_corners(point(r.right() - px(8.0 + tw + 6.0), r.top() + px(3.0)), point(r.right() - px(4.0), r.bottom() - px(4.0)));
                                let on = matches!(&hover, Some(Hit::Dismiss(a, _)) if a == app);
                                style::label(window, cx, cr.right() - px(4.0), r.center().y, Align::Right, "Dismiss", style::DETAIL_SIZE, small, if on { text } else { dim });
                                // (Not those pinned, or on screen: they stay put.)
                                let gone: Vec<usize> = members.iter().copied().filter(|&i| !entries2[i].shown && !entries2[i].pinned).collect();
                                hits.push((cr - b.origin, Hit::Dismiss(app.clone(), gone)));
                                agent::record(window, cx, agent::Node::button(format!("Dismiss {name}"), cr));
                            }
                        }
                        Item::Note { i, older, grouped } => {
                            row += 1;
                            hits.push((lr, Hit::Row(*i)));
                            if !shown {
                                continue;
                            }
                            let e = &entries2[*i];
                            let Some(laid) = rows2.get(i) else { continue };
                            let is_sel = sel_id == Some(e.id);
                            let hovered = hover.as_ref().is_some_and(|h| matches!(h, Hit::Row(x) | Hit::Forget(x) if x == i));
                            let (fg, bg) = if is_sel { (p.list_selected_item_text, p.list_selected_background) } else { (p.document_text, p.document_background) };
                            let dim = style::c(olo_theme::mix(fg, bg, 0.45));
                            if is_sel {
                                window.paint_quad(gpui::fill(r, style::c(bg)));
                            } else if hovered {
                                window.paint_quad(gpui::fill(r, style::tint(p.document_background, 1.04)));
                            }
                            let x0 = if *grouped { 8.0 } else { 0.0 };
                            let x = r.left() + px(x0 + TEXT_X);
                            // Older, fainter; full again while selected or under the pointer.
                            let opacity = if !is_sel && !hovered { fade_opacity(strength(e)) } else { 1.0 };
                            let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
                            window.with_element_opacity(Some(opacity), |window| {
                                // The title's first line, the app and time, the dot and the
                                // icon all centre on one line.
                                let title_top = style::snap_coord(window, r.top() + px(ROW_PAD));
                                let baseline = laid.title.baseline(title_top);
                                let line1 = baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
                                if e.unread {
                                    unread_dot(window, point(r.left() + px(x0 + 9.0), line1), &p);
                                }
                                paint_icon(window, cx, Bounds::centered_at(point(r.left() + px(x0 + ICON_X + SMALL_ICON / 2.0), line1), size(px(SMALL_ICON), px(SMALL_ICON))), e, true);
                                laid.title.paint(window, cx, point(x, title_top));
                                agent::record(window, cx, agent::Node::label(title_text.to_owned(), Bounds::new(point(x, title_top), size(px(laid.title.width), px(laid.title.height)))));
                                if failed(e) {
                                    look::paint_stop(window, cx, &laid.title, point(x, title_top), style::PLAIN_SIZE);
                                }
                                let meta = match view {
                                    HistoryView::List => format!("{} · {}", app_name(e), clock(e.time)),
                                    HistoryView::Apps => crate::when(e.time),
                                    _ => format!("{} · {}", app_name(e), crate::when(e.time)),
                                };
                                let meta = if e.pinned { format!("{meta} · Pinned") } else { meta };
                                style::label_on_baseline(window, cx, r.right() - px(CLOSE_W), baseline, Align::Right, meta, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                                if !older.is_empty() && !is_sel {
                                    // How many in the conversation.
                                    let n = (older.len() + 1).to_string();
                                    let tx = x + px(laid.title.first_row_width() + 6.0);
                                    style::pill(window, cx, tx, line1, &n, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim, style::c(p.document_background));
                                }
                                let mut y = title_top + px(laid.title.height);
                                if let Some(body) = &laid.body {
                                    y += px(1.0);
                                    body.paint(window, cx, point(x, y));
                                    agent::record(window, cx, agent::Node::label(e.text.clone(), Bounds::new(point(x, y), size(px(body.width), px(body.height)))));
                                    y += px(body.height);
                                }
                                if !laid.older.is_empty() {
                                    y += px(m::TIGHT);
                                    let older_h = style::SMALL_SIZE + 5.0;
                                    for (t, g) in &laid.older {
                                        let cy = y + px(older_h / 2.0);
                                        style::label(window, cx, x, cy, Align::Left, t.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                                        let top = style::snap_coord(window, cy - px(g.height / 2.0));
                                        g.paint(window, cx, point(x + px(44.0), top));
                                        agent::record(window, cx, agent::Node::label(g.text.clone(), Bounds::new(point(x + px(44.0), top), size(px(g.width), px(g.height)))));
                                        y += px(older_h);
                                    }
                                }
                                // Forget it: a × while the pointer is over it.
                                if !e.shown && hovered {
                                    let close = Bounds::centered_at(point(r.right() - px(CLOSE_W / 2.0), line1), size(px(12.0), px(12.0)));
                                    let lit = hover == Some(Hit::Forget(*i));
                                    style::close_box(window, close, if lit { style::c(fg) } else { dim });
                                    hits.push((close.dilate(px(3.0)) - b.origin, Hit::Forget(*i)));
                                    agent::record(window, cx, agent::Node::button("Forget", close));
                                }
                            });
                            if !is_sel {
                                style::hline_above(window, r.left() + px(x0 + TEXT_X), r.right(), r.bottom(), style::tint(p.document_background, 1.07));
                            }
                            agent::record(window, cx, agent::Node { role: "ListItem", name: Some(title_text.to_owned().into()), bounds: r, enabled: true, clickable: true, selected: Some(is_sel), ..Default::default() });
                            let _ = (row, rows_total);
                        }
                    }
                }
            },
        )
        .absolute()
        .inset_0();

        // The list's own buttons: the selected row's, and live cards'.
        let mut content = div().id("history-list").relative().w(px(w)).h(px(total.max(1.0))).child(paint);
        if cards_view {
            for (i, laid, pl) in list.cards.iter() {
                if laid.buttons.is_empty() {
                    continue;
                }
                let inner_right = pl.x + pl.w - m::CARD_INSET;
                let by = pl.y + m::SPACING + pl.h - m::CARD_INSET - CARD_BUTTON;
                let mut bx = inner_right;
                for (key, label, bw) in laid.buttons.iter().rev() {
                    if bx - bw < pl.x + m::CARD_INSET && bx < inner_right {
                        break;
                    }
                    let (me, key, i) = (me.clone(), key.clone(), *i);
                    let b = Button::new(label.clone()).small().min_width(*bw).on_click(move |window, cx| me.update(cx, |h, cx| h.app.update(cx, |n, cx| n.open_entry(cx, Some(window), i, Some(key.clone())))));
                    content = content.child(div().absolute().left(px(bx - bw)).top(px(by)).occlude().child(b));
                    bx -= bw + m::TIGHT;
                }
            }
        } else if let Some((i, top, older)) = list.sel_row.clone()
            && let Some(laid) = list.rows.get(&i)
        {
            // The selected row's buttons: its app's actions, then Dismiss.
            let x = if view == HistoryView::Apps { 8.0 } else { 0.0 } + TEXT_X;
            let mut by = top + ROW_PAD + laid.title.height + laid.body.as_ref().map_or(0.0, |b| 1.0 + b.height);
            if !laid.older.is_empty() {
                by += m::TIGHT + laid.older.len() as f32 * (style::SMALL_SIZE + 5.0);
            }
            by += m::TIGHT;
            let mut bx = x;
            for (key, label, bw) in &laid.buttons {
                if bx + bw > w - 4.0 && bx > x {
                    break;
                }
                let (me, key, older, order) = (me.clone(), key.clone(), older.clone(), order.clone());
                let b = Button::new(label.clone()).min_width(*bw).on_click(move |window, cx| {
                    let (key, older, order) = (key.clone(), older.clone(), order.clone());
                    me.update(cx, |h, cx| {
                        h.app.update(cx, |n, cx| {
                            if key.is_empty() {
                                // The notification, or the whole conversation it gathers.
                                n.forget(cx, std::iter::once(i).chain(older.iter().copied()).collect(), &order);
                            } else {
                                n.open_entry(cx, Some(window), i, Some(key));
                            }
                        })
                    })
                });
                content = content.child(div().absolute().left(px(bx)).top(px(by)).occlude().child(b));
                bx += bw + m::TIGHT;
            }
        }
        let (press_order, menu_order) = (order.clone(), order.clone());
        let content = content
            .on_mouse_down(MouseButton::Left, {
                let me = me.clone();
                move |e: &MouseDownEvent, window, cx| {
                    let origin = me.read(cx).list_origin(window);
                    let order = press_order.clone();
                    me.update(cx, |h, cx| h.press(e, origin, order, window, cx))
                }
            })
            .on_mouse_down(MouseButton::Right, {
                let me = me.clone();
                move |e: &MouseDownEvent, window, cx| {
                    let origin = me.read(cx).list_origin(window);
                    let order = menu_order.clone();
                    me.update(cx, |h, cx| h.menu(e, origin, order, window, cx))
                }
            })
            .on_mouse_move({
                let me = me.clone();
                move |e: &MouseMoveEvent, window, cx| {
                    me.update(cx, |h, cx| {
                        let origin = h.list_origin(window);
                        let now = h.hit_at(e.position - origin);
                        if now != h.hover {
                            h.hover = now;
                            cx.notify();
                        }
                    })
                }
            })
            .on_hover({
                let me = me.clone();
                move |inside, _, cx| {
                    if !*inside {
                        me.update(cx, |h, cx| {
                            if h.hover.take().is_some() {
                                cx.notify();
                            }
                        });
                    }
                }
            });
        let shown_list: gpui::AnyElement = if list.shown == 0 {
            let empty = if searching { "Nothing matches" } else { "No notifications" };
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    window.paint_quad(gpui::fill(b, style::c(p.document_background)));
                    style::sunken(window, b, &p, false);
                    let at = style::label(window, cx, b.center().x, b.center().y, Align::Center, empty, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                    agent::record(window, cx, agent::Node::label(empty, at));
                },
            )
            .size_full()
            .into_any_element()
        } else {
            let background = if cards_view { style::card_paper(&p).0 } else { p.document_background };
            ScrollView::new("history", &self.scroll).background(background).child(content).into_any_element()
        };

        // The bar: Do Not Disturb, the search, Clear All.
        let can_clear = list.can_clear;
        let dnd_box = CheckBox::new("Do Not Disturb", dnd).height(m::CONTROL_HEIGHT).on_toggle({
            let me = me.clone();
            move |on, _, cx| me.update(cx, |h, cx| h.app.update(cx, |n, cx| n.set_quiet(on, 0, cx)))
        });
        let clear = Button::new("Clear All").min_width(m::BUTTON_MIN_WIDTH).enabled(can_clear).on_click({
            let me = me.clone();
            move |_, cx| me.update(cx, |h, cx| h.app.update(cx, |n, cx| n.clear_history(cx)))
        });
        let bar_row = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT + 2.0 * m::SPACING))
            .px(px(m::SPACING))
            .child(div().flex_shrink_0().child(dnd_box))
            // The search takes what the rest leave.
            .child(div().flex_1().min_w_0().child(TextControl::new(self.search.clone())))
            .child(clear);

        // The status line.
        let (snoozed, all, unread_now) = (list.snoozed, list.all, list.unread);
        let mut status = if searching { format!("{} of {all}", list.shown) } else if all == 1 { "1 notification".into() } else { format!("{all} notifications") };
        if unread_now > 0 && !searching {
            status += &format!(", {unread_now} unread");
        }
        if snoozed > 0 && !searching {
            status += &format!(", {snoozed} snoozed");
        }
        if quiet {
            status += if !schedule_quiet { " · Do Not Disturb" } else { " · Quiet by schedule" };
        }
        let foot = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let at = style::label(window, cx, b.left() + px(m::SPACING), b.center().y, Align::Left, status.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                agent::record(window, cx, agent::Node::label(status.clone(), at));
            },
        )
        .w_full()
        .h(px(FOOTER));

        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&content_focus)
            .on_key_down(cx.listener(move |h, e: &KeyDownEvent, window, cx| h.key(e, window, cx, &keys_order, cards_view)))
            .child(bar)
            .child(bar_row)
            .child(div().flex_1().min_h_0().child(shown_list))
            .child(foot)
    }
}

impl HistoryWindow {
    /// Where the list's content starts in the window (it scrolls).
    /// The list laid out `w` wide: the notes that show (unread first),
    /// in the view's lines, each row or card laid out.
    fn lay_out(&mut self, window: &mut Window, cx: &mut Context<Self>, p: &olo_theme::Palette, w: f32) -> ListLaid {
        let p = p.clone();
        let (view, entries, unread_now, selected_id, folded, held, reveal) = {
            let n = self.app.read(cx);
            (n.settings.view, n.history.entries.clone(), n.history.unread(), n.view.selected, n.view.folded.clone(), n.view.held.clone(), n.view.reveal)
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
        let selected = selected_id.and_then(|id| {
            order.iter().position(|&i| entries[i].id == id).or_else(|| {
                lines.iter().find_map(|l| match l {
                    Item::Note { i, older, .. } if older.iter().any(|&o| entries[o].id == id) => order.iter().position(|o| o == i),
                    _ => None,
                })
            })
        });
        let sel_id = selected.map(|s| entries[order[s]].id);
        if sel_id != selected_id && sel_id.is_some() {
            let app = self.app.clone();
            window.defer(cx, move |_, cx| app.update(cx, |n, _| n.view.selected = sel_id));
        }

        // The list, laid out for its width.
        let cards_view = view == HistoryView::Cards;
        let mut placed: Vec<Line> = Vec::new();
        let mut row_laid: HashMap<usize, RowLaid> = HashMap::new();
        // The selected row's top, and the rest of its conversation.
        let mut sel_row: Option<(usize, f32, Vec<usize>)> = None;
        let mut card_laid: Vec<(usize, CardLaid, masonry::Place)> = Vec::new();
        let total;
        if cards_view {
            let (cols, col_w) = masonry::columns(w, CARD_W, m::SPACING);
            let mut sizes = Vec::new();
            let mut laid = Vec::new();
            for (k, &i) in order.iter().enumerate() {
                let e = &entries[i];
                let span = card_span(e, cols, k == 0 && e.unread);
                let l = self.card(window, &p, e, span, masonry::span_width(span, col_w, m::SPACING));
                sizes.push((span, l.height));
                laid.push((i, l));
            }
            let (places, height) = masonry::arrange(cols, col_w, m::SPACING, &sizes);
            self.places = laid.iter().zip(&places).map(|((i, _), pl)| (entries[*i].id, *pl)).collect();
            card_laid = laid.into_iter().zip(places).map(|((i, l), pl)| (i, l, pl)).collect();
            total = height + m::SPACING;
        } else {
            let mut y = 0.0;
            for item in lines {
                let h = match &item {
                    Item::Unread(_) | Item::Day(_) => HEADING,
                    Item::Group { .. } => HEADING + 2.0,
                    Item::Note { i, older, grouped } => {
                        let e = &entries[*i];
                        let meta = match view {
                            HistoryView::List => format!("{} · {}", app_name(e), clock(e.time)),
                            HistoryView::Apps => crate::when(e.time),
                            _ => format!("{} · {}", app_name(e), crate::when(e.time)),
                        };
                        let meta = if e.pinned { format!("{meta} · Pinned") } else { meta };
                        let is_sel = sel_id == Some(e.id);
                        let l = self.row(window, &p, &entries, *i, older, meta, is_sel, *grouped, w);
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
        let ids: HashSet<u32> = entries.iter().map(|e| e.id).collect();
        self.rows.retain(|id, _| ids.contains(id));
        self.cards.retain(|id, _| ids.contains(id));
        // The selection brought into view.
        if reveal && let Some(id) = sel_id {
            let span = if cards_view {
                card_laid.iter().find(|(i, ..)| entries[*i].id == id).map(|(_, _, pl)| (pl.y, pl.y + pl.h + m::SPACING))
            } else {
                placed.iter().find(|l| matches!(l.item, Item::Note { i, .. } if entries[i].id == id)).map(|l| (l.y, l.y + l.h))
            };
            if let Some((top, bottom)) = span {
                let view_h = f32::from(self.scroll.bounds().size.height);
                let at = -f32::from(self.scroll.offset().y);
                let to = if top < at { top } else if bottom > at + view_h { bottom - view_h } else { at };
                if to != at {
                    self.scroll.set_offset(point(px(0.0), px(-to.max(0.0))));
                }
            }
            let app = self.app.clone();
            window.defer(cx, move |_, cx| app.update(cx, |n, _| n.view.reveal = false));
        }

        let snoozed = entries.iter().filter(|e| e.snoozed()).count();
        ListLaid {
            can_clear: entries.iter().any(|e| !e.shown && !e.pinned && !e.snoozed()),
            all: entries.len() - snoozed,
            snoozed,
            shown: shown.len(),
            unread: unread_now,
            entries: Rc::new(entries),
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

    fn list_origin(&self, _window: &Window) -> Point<Pixels> {
        let b = self.scroll.bounds();
        b.origin + self.scroll.offset()
    }
}

impl Notifications {
    /// Select the note `id`: a click reads it (it keeps its place while
    /// the window is open).
    pub fn select(&mut self, id: u32, cx: &mut Context<Self>) {
        self.view.selected = Some(id);
        if self.history.entries.iter().any(|e| e.id == id && e.unread) {
            self.view.held.insert(id);
            self.set_read(cx, id, true);
        }
        cx.notify();
    }

    /// Fold an app's group in By App, or unfold it.
    pub fn fold(&mut self, app: String, cx: &mut Context<Self>) {
        if !self.view.folded.remove(&app) {
            self.view.folded.insert(app);
        }
        cx.notify();
    }

    /// Forget the entries at `at` (`order`: the list as shown, for what
    /// is selected next); what is still open at its app closes.
    pub fn forget(&mut self, cx: &mut Context<Self>, mut at: Vec<usize>, order: &[usize]) {
        // From the end, so the indices stay good.
        at.sort_unstable();
        at.dedup();
        let next = (at.len() == 1).then(|| order.iter().position(|&i| i == at[0])).flatten().and_then(|s| order.get(s + 1).or(s.checked_sub(1).and_then(|s| order.get(s)))).map(|&i| self.history.entries[i].id);
        for &i in at.iter().rev() {
            let Some(e) = self.history.entries.get(i).cloned() else { continue };
            self.history.remove(i);
            if e.live {
                self.take(cx, e.id);
                self.bus(|b| b.closed(e.id, Reason::Dismissed));
            }
        }
        if self.view.selected.is_some() {
            self.view.selected = next;
        }
        self.publish(true);
        cx.notify();
    }

    /// Run a history entry's action (`None`: its default). Still open at
    /// the app, that is what a click on it does there; otherwise the app
    /// comes forward.
    pub fn open_entry(&mut self, cx: &mut Context<Self>, window: Option<&Window>, i: usize, key: Option<String>) {
        let Some(e) = self.history.entries.get(i).cloned() else { return };
        let key = key.unwrap_or_else(|| "default".into());
        if e.unread {
            self.view.held.insert(e.id);
        }
        if e.live && e.actions.iter().any(|(k, _)| *k == key) {
            self.invoke(cx, window, e.id, key);
        } else {
            self.set_read(cx, e.id, true);
            raise_app(e.desktop_entry.as_deref(), &e.app);
        }
    }

    /// The selected note read or unread (Edit menu); whether there is one.
    pub fn toggle_read(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.view.selected else { return false };
        let Some(unread) = self.history.entries.iter().rev().find(|e| e.id == id).map(|e| e.unread) else { return false };
        self.view.held.insert(id);
        self.set_read(cx, id, unread);
        true
    }

    /// Whether the selected note is unread (None: nothing selected).
    pub fn selected_unread(&self) -> Option<bool> {
        let id = self.view.selected?;
        self.history.entries.iter().rev().find(|e| e.id == id).map(|e| e.unread)
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

    #[test]
    fn search_takes_every_word() {
        let mut e = entry(1, "Mail", "Lunch on Friday", 0);
        e.text = "At the usual place".into();
        assert!(found(&e, "mail usual"));
        assert!(found(&e, "  FRIDAY "));
        assert!(!found(&e, "lunch dinner"));
    }
}
