//! The History window: what came, unread first, newest first, in one of
//! four views (View menu, remembered in `notifications.json`). Rows are
//! dense: a small icon, the title with the app and time on the same line,
//! one line of the body under it; the selected one opens up to its whole
//! body and its actions. What is unread wears a dot (in the keyboard's
//! blue) and a bold title until it is clicked; what was read while the
//! window is open keeps its place until it closes. With fading on, each
//! fades as it ages, the newest strongest, back to full while selected
//! or under the pointer. The Cards view packs cards of different sizes
//! (`masonry`), each a `style::card` headed by its app and time, the dot
//! by the time while unread. A search field filters, and the keyboard
//! works the list (arrows, Return to open, Delete to forget, Escape).

use std::collections::{BTreeSet, HashSet};

use olo_kit::{
    Cx, WindowId,
    egui::{self, Align, Color32, Key, Rect, Sense, WidgetInfo, pos2, vec2},
    icons, masonry, metrics as m,
    style::{self, c},
    theme::mix,
    widgets::{Button, CheckBox, ScrollView, TextControl},
};
use olo_protocol::{CardSize, HistoryView, NoteStyle};

use crate::{Notifications, close_box, galley, look, named_icons, now_secs, paint_mark, raise_app, server::Reason, store, web};

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
const SCROLL_ID: &str = "history";
/// The faintest a notification draws, at the very end of its fade: still
/// legible while it is there to read and click, then gone.
const FAINTEST: f32 = 0.3;

/// How opaque a notification of fade `strength` (1 new, 0 gone) draws.
fn fade_opacity(strength: f32) -> f32 {
    FAINTEST + (1.0 - FAINTEST) * strength.clamp(0.0, 1.0)
}

/// What the History window remembers while it is open (and across a
/// reload, see `save_state`).
#[derive(Default)]
pub struct HistoryState {
    pub search: String,
    /// The selected note's id.
    pub selected: Option<u32>,
    /// The apps folded away in By App.
    pub folded: BTreeSet<String>,
    /// Read while the window is open: they keep their places among the
    /// unread until it closes, so nothing moves from under the pointer.
    pub held: HashSet<u32>,
    /// Where each card was last drawn (Cards), for the arrow keys.
    cards: Vec<(u32, Rect)>,
    /// The search field's id (for ⌘F) and whether to focus it.
    search_id: Option<egui::Id>,
    pub focus_search: bool,
    /// Bring the selection into view, for this many frames (a new view
    /// takes two: the first is clamped to the old view's height).
    reveal: u8,
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

/// What a click or key did, done after drawing.
enum Did {
    Select(u32),
    Open(usize),
    Action(usize, String),
    Forget(Vec<usize>),
    Fold(String),
    Clear,
    Quiet(bool),
    MarkAllRead,
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
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
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

impl Notifications {
    /// The History window: Do Not Disturb, search and Clear All above, the
    /// notes in the chosen view, a count below.
    pub fn history_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect();
        let mut did = None;
        let view = self.settings.view;

        // The bar.
        let bar_h = m::CONTROL_HEIGHT + 2.0 * m::SPACING;
        let mut quiet = self.settings.dnd;
        let check = Rect::from_min_size(full.min + vec2(m::SPACING, m::SPACING), vec2(130.0, m::CONTROL_HEIGHT));
        let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(check).layout(egui::Layout::left_to_right(Align::Center)));
        let check_resp = bar.add(CheckBox::new(&mut quiet, "Do Not Disturb"));
        if check_resp.changed() {
            did = Some(Did::Quiet(quiet));
        }
        let clear_w = m::BUTTON_MIN_WIDTH;
        let clear = Rect::from_min_size(pos2(full.right() - m::SPACING - clear_w, full.top() + m::SPACING), vec2(clear_w, m::CONTROL_HEIGHT));
        let can_clear = self.history.entries.iter().any(|e| !e.shown);
        if ui.add_enabled_ui(can_clear, |ui| ui.put(clear, Button::new("Clear All")).clicked()).inner {
            did = Some(Did::Clear);
        }
        let search_r = Rect::from_min_max(pos2(check_resp.rect.right() + m::SPACING, clear.top()), pos2(clear.left() - m::SPACING, clear.bottom()));
        let search = ui.put(search_r, TextControl::new(&mut self.view.search).hint("Search"));
        self.view.search_id = Some(search.id);
        ui.ctx().accesskit_node_builder(search.id, |n| n.set_label("Search"));
        if std::mem::take(&mut self.view.focus_search) {
            search.request_focus();
        }
        let typing = search.has_focus();

        // What shows: unread (and what was read while the window is
        // open) first, newest first in each.
        let entries = &self.history.entries;
        let searching = !self.view.search.trim().is_empty();
        let held = &self.view.held;
        let first = |e: &store::Entry| e.unread || held.contains(&e.id);
        let mut shown: Vec<usize> = (0..entries.len()).rev().filter(|&i| !searching || found(&entries[i], &self.view.search)).collect();
        shown.sort_by_key(|&i| !first(&entries[i]));
        let unread = shown.iter().take_while(|&&i| first(&entries[i])).count();
        let lines = items(entries, &shown, unread, view, &self.view.folded, searching);
        let now = now_secs();
        let settings = &self.settings;
        let strength = |e: &store::Entry| settings.strength(now.saturating_sub(e.time));
        let order: Vec<usize> = lines.iter().filter_map(|l| if let Item::Note { i, .. } = l { Some(*i) } else { None }).collect();
        // (A selection stacked under a conversation's newest is that.)
        let in_thread = |id: u32| {
            lines.iter().find_map(|l| match l {
                Item::Note { i, older, .. } if older.iter().any(|&o| entries[o].id == id) => order.iter().position(|o| o == i),
                _ => None,
            })
        };
        let selected = self.view.selected.and_then(|id| order.iter().position(|&i| entries[i].id == id).or_else(|| in_thread(id)));
        if let Some(s) = selected {
            self.view.selected = Some(entries[order[s]].id);
        }

        // The keyboard.
        let list = Rect::from_min_max(pos2(full.left(), full.top() + bar_h), pos2(full.right(), full.bottom() - FOOTER));
        let cards = view == HistoryView::Cards;
        let (arrow, home) = ui.input_mut(|i| {
            let mut arrow = None;
            for (key, step) in [(Key::ArrowDown, masonry::Step::Down), (Key::ArrowUp, masonry::Step::Up), (Key::ArrowRight, masonry::Step::Right), (Key::ArrowLeft, masonry::Step::Left)] {
                let sideways = matches!(step, masonry::Step::Left | masonry::Step::Right);
                if (!sideways || (cards && !typing)) && i.consume_key(egui::Modifiers::NONE, key) {
                    arrow = Some(step);
                }
            }
            let home = if typing { None } else if i.consume_key(egui::Modifiers::NONE, Key::Home) { Some(false) } else if i.consume_key(egui::Modifiers::NONE, Key::End) { Some(true) } else { None };
            (arrow, home)
        });
        // Cards go by where they are (last drawn); the rest by order.
        let by_place = |s: usize, step: masonry::Step| -> Option<usize> {
            let rects: Vec<Rect> = order.iter().map(|&i| self.view.cards.iter().find(|(id, _)| *id == entries[i].id).map_or(Rect::NOTHING, |(_, r)| *r)).collect();
            masonry::neighbour(&rects, s, step).or(match step {
                masonry::Step::Right => (s + 1 < order.len()).then_some(s + 1),
                masonry::Step::Left => s.checked_sub(1),
                _ => None,
            })
        };
        if !order.is_empty() {
            let forward = matches!(arrow, Some(masonry::Step::Down | masonry::Step::Right));
            let to = match (home, arrow, selected) {
                (Some(end), ..) => Some(if end { order.len() - 1 } else { 0 }),
                (None, None, _) => None,
                (None, Some(_), None) => Some(if forward { 0 } else { order.len() - 1 }),
                (None, Some(step), Some(s)) if cards => by_place(s, step).or(Some(s)),
                (None, Some(_), Some(s)) => Some(if forward { (s + 1).min(order.len() - 1) } else { s.saturating_sub(1) }),
            };
            if let Some(to) = to {
                did = Some(Did::Select(entries[order[to]].id));
                self.view.reveal = 1;
            }
        }
        if let Some(s) = selected {
            let (enter, delete) = ui.input_mut(|i| {
                (i.consume_key(egui::Modifiers::NONE, Key::Enter), !typing && (i.consume_key(egui::Modifiers::NONE, Key::Delete) || i.consume_key(egui::Modifiers::NONE, Key::Backspace)))
            });
            if enter {
                did = Some(Did::Open(order[s]));
            } else if delete {
                did = Some(Did::Forget(vec![order[s]]));
            }
            if view == HistoryView::Apps && !typing {
                let app = &entries[order[s]].app;
                let (left, right) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft), i.consume_key(egui::Modifiers::NONE, Key::ArrowRight)));
                if (left && !self.view.folded.contains(app)) || (right && self.view.folded.contains(app)) {
                    did = Some(Did::Fold(app.clone()));
                }
            }
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if searching {
                self.view.search.clear();
            } else {
                self.view.selected = None;
            }
        }

        // The list.
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        if shown.is_empty() {
            child.painter().rect_filled(list, 0.0, c(p.document_background));
            style::sunken(child.painter(), list, &p, false);
            let empty = if searching { "Nothing matches" } else { "No notifications" };
            style::label(child.painter(), list.center().x, list.center().y, Align::Center, empty, style::plain_font(), style::disabled_text(&p));
        } else {
            let reveal = self.view.reveal > 0;
            if reveal {
                self.view.reveal -= 1;
                ui.ctx().request_repaint();
            }
            let sel_id = self.view.selected;
            let mut drawn = Vec::new();
            let background = if cards { style::card_paper(&p).0 } else { p.document_background };
            ScrollView::new(SCROLL_ID).background(background).show(&mut child, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let top = ui.cursor().top();
                let w = ui.available_width();
                let mut row = 0;
                if cards {
                    // Each card as big as what it holds (or its app asks),
                    // the newest unread one featured, packed in columns.
                    let (cols, col_w) = masonry::columns(w, CARD_W, m::SPACING);
                    let laid: Vec<CardLaid> = order
                        .iter()
                        .enumerate()
                        .map(|(k, &i)| {
                            let e = &entries[i];
                            let span = card_span(e, cols, k == 0 && e.unread);
                            card_lay(ui, &p, e, span, masonry::span_width(span, col_w, m::SPACING))
                        })
                        .collect();
                    let sizes: Vec<(usize, f32)> = laid.iter().map(|l| (l.span, l.height)).collect();
                    let (rects, height) = masonry::arrange(cols, col_w, m::SPACING, &sizes);
                    let (area, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
                    for (k, (&i, (laid, r))) in order.iter().zip(laid.into_iter().zip(rects)).enumerate() {
                        let e = &entries[i];
                        let cr = r.translate(area.min.to_vec2());
                        drawn.push((e.id, r));
                        let is_sel = sel_id == Some(e.id);
                        if is_sel && reveal {
                            ScrollView::reveal(ui.ctx(), SCROLL_ID, cr.top() - top - m::SPACING, cr.bottom() - top + m::SPACING);
                        }
                        if !cr.intersects(ui.clip_rect()) {
                            continue;
                        }
                        if let Some(d) = card(ui, &p, cr, laid, i, e, is_sel, strength(e), k, order.len()) {
                            did = Some(d);
                        }
                    }
                    return;
                }
                for line in &lines {
                    match line {
                        Item::Unread(n) => {
                            let (r, _) = ui.allocate_exact_size(vec2(w, HEADING), Sense::hover());
                            heading_fill(ui, &p, r);
                            let font = egui::FontId::new(style::SMALL_SIZE, style::bold());
                            let t = style::label(ui.painter(), r.left() + ICON_X, r.center().y, Align::Min, "Unread", font, style::disabled_text(&p));
                            if *n == 0 {
                                continue;
                            }
                            style::label(ui.painter(), t.right() + 5.0, r.center().y, Align::Min, n.to_string(), style::small_font(), style::disabled_text(&p));
                            // Read them all, here where they are.
                            let all = "Mark All Read";
                            let tw = style::label_width(ui.ctx(), all, style::small_font());
                            let br = Rect::from_min_max(pos2(r.right() - 8.0 - tw - 8.0, r.top() + 2.0), pos2(r.right() - 4.0, r.bottom() - 3.0));
                            let b = ui.interact(br, ui.make_persistent_id("mark-all-read"), Sense::click());
                            b.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, all));
                            let ink = if b.hovered() { c(p.document_text) } else { style::disabled_text(&p) };
                            style::label(ui.painter(), br.right() - 4.0, r.center().y, Align::Max, all, style::small_font(), ink);
                            if b.clicked() {
                                did = Some(Did::MarkAllRead);
                            }
                        }
                        Item::Day(label) => {
                            let (r, _) = ui.allocate_exact_size(vec2(w, HEADING), Sense::hover());
                            heading_fill(ui, &p, r);
                            let font = egui::FontId::new(style::SMALL_SIZE, style::bold());
                            style::label(ui.painter(), r.left() + ICON_X, r.center().y, Align::Min, label, font, style::disabled_text(&p));
                        }
                        Item::Group { app, count, members, open } => {
                            if let Some(d) = group(ui, &p, w, app, *count, members, *open, entries) {
                                did = Some(d);
                            }
                        }
                        Item::Note { i, older, grouped } => {
                            let e = &entries[*i];
                            let is_sel = sel_id == Some(e.id);
                            let meta = match view {
                                HistoryView::List => format!("{} · {}", app_name(e), clock(e.time)),
                                HistoryView::Apps => crate::when(e.time),
                                _ => format!("{} · {}", app_name(e), crate::when(e.time)),
                            };
                            let r = note_row(ui, &p, w, &RowArgs { i: *i, e, older, entries, meta, selected: is_sel, unread: e.unread, strength: strength(e), indent: *grouped, row, rows: order.len() }, &mut did);
                            if is_sel && reveal {
                                ScrollView::reveal(ui.ctx(), SCROLL_ID, r.top() - top, r.bottom() - top);
                            }
                            row += 1;
                        }
                    }
                }
            });
            self.view.cards = drawn;
        }

        // The status line.
        let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOTER), full.max);
        let total = self.history.entries.len();
        let mut status = if searching { format!("{} of {total}", shown.len()) } else if total == 1 { "1 notification".into() } else { format!("{total} notifications") };
        let unread_now = self.history.unread();
        if unread_now > 0 && !searching {
            status += &format!(", {unread_now} unread");
        }
        if self.quiet() {
            status += if self.settings.dnd { " · Do Not Disturb" } else { " · Quiet by schedule" };
        }
        style::label(ui.painter(), foot.left() + m::SPACING, foot.center().y, Align::Min, status, style::small_font(), style::disabled_text(&p));

        match did {
            Some(Did::Quiet(on)) => self.set_quiet(on, 0),
            Some(Did::Clear) => self.clear_history(cx),
            Some(Did::Select(id)) => {
                // A click reads it (it keeps its place while the window is
                // open).
                self.view.selected = Some(id);
                if self.history.entries.iter().any(|e| e.id == id && e.unread) {
                    self.view.held.insert(id);
                    self.set_read(cx, id, true);
                }
            }
            Some(Did::MarkAllRead) => {
                self.view.held.extend(self.history.entries.iter().filter(|e| e.unread).map(|e| e.id));
                self.mark_all_read(cx);
            }
            Some(Did::Fold(app)) => {
                if !self.view.folded.remove(&app) {
                    self.view.folded.insert(app);
                }
            }
            Some(Did::Forget(mut at)) => {
                // From the end, so the indices stay good.
                at.sort_unstable();
                let next = (at.len() == 1).then(|| order.iter().position(|&i| i == at[0])).flatten().and_then(|s| order.get(s + 1).or(s.checked_sub(1).and_then(|s| order.get(s)))).map(|&i| self.history.entries[i].id);
                for &i in at.iter().rev() {
                    let e = self.history.entries[i].clone();
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
            }
            Some(Did::Open(i)) => self.open_entry(cx, window, i, None),
            Some(Did::Action(i, key)) => self.open_entry(cx, window, i, Some(key)),
            None => {}
        }
    }

    /// Run a history entry's action (`None`: its default). Still open at
    /// the app, that is what a click on it does there; otherwise the app
    /// comes forward.
    fn open_entry(&mut self, cx: &mut Cx, window: WindowId, i: usize, key: Option<String>) {
        let e = self.history.entries[i].clone();
        let key = key.unwrap_or_else(|| "default".into());
        if e.unread {
            self.view.held.insert(e.id);
        }
        if e.live && e.actions.iter().any(|(k, _)| *k == key) {
            self.invoke(cx, Some(window), e.id, key);
        } else {
            self.set_read(cx, e.id, true);
            raise_app(e.desktop_entry.as_deref(), &e.app);
        }
    }

    /// The selected note read or unread (Edit menu); whether there is one.
    pub fn toggle_read(&mut self, cx: &mut Cx) -> bool {
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
        self.view.reveal = 2;
    }
}

/// A heading's band: a shade darker than the list.
fn heading_fill(ui: &egui::Ui, p: &olo_kit::theme::Palette, r: Rect) {
    ui.painter().rect_filled(r, 0.0, style::darken(p.document_background, 1.04));
    style::hline_above(ui.painter(), r.left(), r.right(), r.bottom(), style::darken(p.document_background, 1.1));
}

/// The icon a note shows, in `r`: its sender's picture, round, for a
/// message; else its app's (or the one it names).
fn paint_icon(painter: &egui::Painter, r: Rect, e: &store::Entry, person: bool) {
    if person
        && e.look.style == NoteStyle::Message
        && let Some(tex) = e.picture.as_deref().and_then(|path| icons::thumbnail(painter.ctx(), std::path::Path::new(path), r.width() * 2.0))
    {
        icons::paint_round(painter, r, &tex, Color32::TRANSPARENT);
        return;
    }
    let names = named_icons(e.icon.as_deref(), e.desktop_entry.as_deref(), &e.app);
    let generic = if e.kind == store::KindName::Error { "warning" } else { "info" };
    paint_mark(painter, r, &e.app, e.look.accent, e.via.is_some(), &names, generic);
}

/// The mark of what is unread: a dot in the colour of what the keyboard
/// is on (not the tab's yellow, which would repeat the window's own on
/// every note).
fn unread_dot(painter: &egui::Painter, center: egui::Pos2, p: &olo_kit::theme::Palette) {
    painter.circle_filled(center, 3.5, c(p.keyboard_navigation));
}

/// Whether a note reports a failure (its title starts with a stop sign).
fn failed(e: &store::Entry) -> bool {
    e.kind == store::KindName::Error
}

/// An app's heading in By App: its latch, icon, name and count; a click
/// folds it; Clear forgets the app's notes.
#[allow(clippy::too_many_arguments)]
fn group(ui: &mut egui::Ui, p: &olo_kit::theme::Palette, w: f32, app: &str, count: usize, members: &[usize], open: bool, entries: &[store::Entry]) -> Option<Did> {
    let (r, resp) = ui.allocate_exact_size(vec2(w, HEADING + 2.0), Sense::click());
    let name = if app.is_empty() { "Notification" } else { app };
    resp.widget_info(|| WidgetInfo { selected: Some(open), ..WidgetInfo::labeled(egui::Role::Button, true, format!("{name} ({count})")) });
    heading_fill(ui, p, r);
    let text = c(p.document_text);
    let dim = style::disabled_text(p);
    style::latch(ui.painter(), Rect::from_center_size(pos2(r.left() + 7.0, r.center().y), vec2(10.0, 10.0)), open, if resp.hovered() { text } else { dim });
    paint_icon(ui.painter(), Rect::from_center_size(pos2(r.left() + ICON_X + SMALL_ICON / 2.0, r.center().y), vec2(SMALL_ICON, SMALL_ICON)), &entries[members[0]], false);
    let n = style::label(ui.painter(), r.left() + TEXT_X, r.center().y, Align::Min, name, style::bold_font(), text);
    style::label(ui.painter(), n.right() + 6.0, r.center().y, Align::Min, count.to_string(), style::small_font(), dim);
    let mut did = None;
    if resp.hovered() || ui.rect_contains_pointer(r) {
        let font = style::small_font();
        let g = ui.painter().layout_no_wrap("Dismiss".into(), font.clone(), dim);
        let cr = Rect::from_min_max(pos2(r.right() - 8.0 - g.size().x - 6.0, r.top() + 3.0), pos2(r.right() - 4.0, r.bottom() - 4.0));
        let clear = ui.interact(cr, ui.make_persistent_id(("clear-group", app)), Sense::click());
        clear.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, format!("Dismiss {name}")));
        style::label(ui.painter(), cr.right() - 4.0, r.center().y, Align::Max, "Dismiss", font, if clear.hovered() { text } else { dim });
        if clear.clicked() {
            did = Some(Did::Forget(members.iter().copied().filter(|&i| !entries[i].shown).collect()));
        }
    }
    if did.is_none() && resp.clicked() {
        did = Some(Did::Fold(app.to_owned()));
    }
    did
}

struct RowArgs<'a> {
    i: usize,
    e: &'a store::Entry,
    older: &'a [usize],
    entries: &'a [store::Entry],
    meta: String,
    selected: bool,
    unread: bool,
    /// How strongly it shows (it fades as it ages).
    strength: f32,
    /// Under its app's heading.
    indent: bool,
    row: usize,
    rows: usize,
}

/// One note as a row; its rect.
fn note_row(ui: &mut egui::Ui, p: &olo_kit::theme::Palette, w: f32, a: &RowArgs, did: &mut Option<Did>) -> Rect {
    let e = a.e;
    let (fg, bg) = if a.selected { (p.list_selected_item_text, p.list_selected_background) } else { (p.document_text, p.document_background) };
    let dim = c(mix(fg, bg, 0.45));
    let soft = c(mix(fg, bg, 0.2));
    let x0 = if a.indent { 8.0 } else { 0.0 };
    let text_x = x0 + TEXT_X;
    let meta_font = style::small_font();
    let meta_g = ui.painter().layout_no_wrap(a.meta.clone(), meta_font.clone(), dim);
    let title_w = (w - text_x - CLOSE_W - meta_g.size().x - 8.0).max(40.0);
    let body_w = w - text_x - CLOSE_W;
    let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
    // Unread: bold, as in a mailbox.
    let title_font = if a.unread || a.selected { style::bold_font() } else { style::plain_font() };
    let lead = if failed(e) { look::MARK_ROOM } else { 0.0 };
    let title = look::led(ui, title_text, lead, title_font, c(fg), title_w, if a.selected { 3 } else { 1 });
    let body = (!e.text.is_empty()).then(|| {
        let mut job = egui::text::LayoutJob::default();
        for s in &e.body {
            let font = if s.bold { style::bold_font() } else if s.italic { style::italic_font() } else { style::plain_font() };
            // Unselected, the body is one line: its breaks as spaces.
            let t = if a.selected { s.text.clone() } else { s.text.replace('\n', " ") };
            job.append(&t, 0.0, egui::TextFormat::simple(font, soft));
        }
        job.wrap = egui::text::TextWrapping { max_width: body_w, max_rows: if a.selected { 12 } else { 1 }, break_anywhere: false, overflow_character: Some('…') };
        ui.fonts_mut(|f| f.layout_job(job))
    });
    // The rest of a conversation, under the newest, when selected.
    let older: Vec<(String, std::sync::Arc<egui::Galley>)> = if a.selected {
        a.older.iter().map(|&o| (crate::when(a.entries[o].time), galley(ui, &a.entries[o].text.replace('\n', " "), soft, body_w - 44.0, 1, style::small_font()))).collect()
    } else {
        Vec::new()
    };
    let actions: Vec<(String, String)> = if !a.selected {
        Vec::new()
    } else if e.live {
        // The default action is Open, whatever the app called it ("Activate").
        e.actions.iter().filter(|(k, l)| k == "default" || !l.is_empty()).map(|(k, l)| (k.clone(), if k == "default" { "Open".to_owned() } else { l.clone() })).collect()
    } else {
        vec![("default".into(), format!("Show {}", app_name(e)))]
    };
    let mut h = ROW_PAD + title.size().y + body.as_ref().map_or(0.0, |b| 1.0 + b.size().y);
    let older_h = style::SMALL_SIZE + 5.0;
    if !older.is_empty() {
        h += m::TIGHT + older.len() as f32 * older_h;
    }
    if a.selected {
        h += m::TIGHT + m::CONTROL_HEIGHT + 2.0;
    }
    let h = (h + ROW_PAD).max(ROW_PAD * 2.0 + SMALL_ICON + 4.0).ceil();
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    // For agents: a list item named by its title, which of how many.
    resp.widget_info(|| WidgetInfo { selected: Some(a.selected), ..WidgetInfo::labeled(egui::Role::ListItem, true, title_text) });
    let shown = r.intersect(ui.clip_rect());
    ui.ctx().accesskit_node_builder(resp.id, |n| {
        if shown.is_positive() {
            n.set_bounds(egui::accesskit::Rect { x0: shown.min.x as f64, y0: shown.min.y as f64, x1: shown.max.x as f64, y1: shown.max.y as f64 });
        }
        n.set_row_index(a.row);
        n.set_row_count(a.rows);
    });
    let mut painter = ui.painter().clone();
    if a.selected {
        painter.rect_filled(r, 0.0, c(bg));
    } else if resp.hovered() {
        painter.rect_filled(r, 0.0, style::darken(p.document_background, 1.04));
    }
    // Older, fainter; full again while selected or under the pointer.
    if !a.selected && !resp.hovered() {
        painter.multiply_opacity(fade_opacity(a.strength));
    }
    // The title's first line, the app and time, the dot and the icon all
    // centre on one line.
    let first_h = title.rows.first().map_or(title.size().y, |row| row.rect().height());
    let line1 = r.top() + ROW_PAD + first_h / 2.0;
    if a.unread {
        unread_dot(&painter, pos2(r.left() + x0 + 9.0, line1), p);
    }
    paint_icon(&painter, Rect::from_center_size(pos2(r.left() + x0 + ICON_X + SMALL_ICON / 2.0, line1), vec2(SMALL_ICON, SMALL_ICON)), e, true);
    let x = r.left() + text_x;
    let title_r = Rect::from_min_size(style::text_origin(ui.ctx(), &title, x, line1, style::PLAIN_SIZE), title.size());
    let mut y = title_r.top();
    style::describe_text(ui.ctx(), egui::accesskit::Role::Label, title_text, title_r, None);
    painter.galley(title_r.min, title.clone(), c(fg));
    if failed(e) {
        look::paint_stop(&painter, &title, title_r.min);
    }
    let meta_x = r.right() - CLOSE_W;
    style::label(&painter, meta_x, line1, Align::Max, a.meta.clone(), meta_font, dim);
    if !a.older.is_empty() && !a.selected {
        // How many in the conversation.
        let n = (a.older.len() + 1).to_string();
        let g = painter.layout_no_wrap(n.clone(), style::small_font(), c(p.document_background));
        let tx = title_r.min.x + title.rows.first().map_or(0.0, |row| row.rect().width()) + 6.0;
        let pill = Rect::from_min_size(pos2(tx, line1 - 7.0), vec2((g.size().x + 8.0).max(14.0), 14.0));
        painter.rect_filled(pill, 7.0, dim);
        style::label(&painter, pill.center().x, pill.center().y, Align::Center, n, style::small_font(), c(p.document_background));
    }
    y += title.size().y;
    if let Some(body) = &body {
        y += 1.0;
        let br = Rect::from_min_size(pos2(x, y), body.size());
        style::describe_text(ui.ctx(), egui::accesskit::Role::Label, &e.text, br, None);
        painter.galley(br.min, body.clone(), soft);
        y += body.size().y;
    }
    if !older.is_empty() {
        y += m::TIGHT;
        for (t, g) in &older {
            let cy = y + older_h / 2.0;
            style::label(&painter, x, cy, Align::Min, t.clone(), style::small_font(), dim);
            let gr = Rect::from_min_size(pos2(x + 44.0, cy - g.size().y / 2.0), g.size());
            style::describe_text(ui.ctx(), egui::accesskit::Role::Label, g.text(), gr, None);
            painter.galley(gr.min, g.clone(), soft);
            y += older_h;
        }
    }
    if a.selected {
        y += m::TIGHT;
        let mut bx = x;
        // Dismiss last: the notification, or the whole conversation it
        // gathers.
        let dismiss = (String::new(), "Dismiss".to_owned());
        for (key, label) in actions.iter().chain([&dismiss]) {
            let bw = (painter.layout_no_wrap(label.clone(), style::plain_font(), dim).size().x + 2.0 * m::CONTROL_PAD).max(m::BUTTON_MIN_WIDTH * 0.75).ceil();
            if bx + bw > r.right() - 4.0 && bx > x {
                break;
            }
            let br = Rect::from_min_size(pos2(bx, y), vec2(bw, m::CONTROL_HEIGHT));
            if ui.put(br, Button::new(label).min_width(bw)).clicked() {
                *did = Some(if key.is_empty() { Did::Forget(std::iter::once(a.i).chain(a.older.iter().copied()).collect()) } else { Did::Action(a.i, key.clone()) });
            }
            bx += bw + m::TIGHT;
        }
    }
    // Forget it: a × while the pointer is over it.
    let close = Rect::from_center_size(pos2(r.right() - CLOSE_W / 2.0, line1), vec2(12.0, 12.0));
    let mut forget = false;
    if !e.shown && (resp.hovered() || ui.rect_contains_pointer(close.expand(3.0))) {
        let cr = ui.interact(close.expand(3.0), ui.make_persistent_id(("forget", a.i, e.id)), Sense::click());
        cr.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, "Forget"));
        close_box(&painter, close, if cr.hovered() { c(fg) } else { dim });
        forget = cr.clicked();
    }
    if !a.selected {
        style::hline_above(ui.painter(), r.left() + text_x, r.right(), r.bottom(), style::darken(p.document_background, 1.07));
    }
    if forget {
        *did = Some(Did::Forget(vec![a.i]));
    } else if resp.double_clicked() {
        *did = Some(Did::Open(a.i));
    } else if resp.clicked() && did.is_none() {
        *did = Some(Did::Select(e.id));
    }
    r
}

/// A card measured: how many columns it spans, how tall it is, its face.
struct CardLaid {
    span: usize,
    height: f32,
    face: look::Laid,
    ink: look::Ink,
    /// Its app's actions, while it's open there.
    buttons: Vec<(String, String)>,
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

/// A card's face's colours (`style::card_ink`).
fn card_ink(p: &olo_kit::theme::Palette, e: &store::Entry) -> look::Ink {
    let card = style::card_ink(p);
    look::Ink {
        text: card.text,
        soft: c(mix(p.document_text, p.document_background, 0.15)),
        dim: card.dim,
        link: c(p.keyboard_navigation),
        balloon: card.well,
        balloon_edge: card.edge,
        accent: e.look.accent.map(|[r, g, b]| Color32::from_rgb(r, g, b)),
    }
}

/// What a note in the history shows.
fn entry_face(e: &store::Entry) -> look::Face<'_> {
    look::Face {
        title: if e.summary.is_empty() { app_name(e) } else { &e.summary },
        body: &e.body,
        look: &e.look,
        progress: e.progress.filter(|_| e.live),
        picture: e.picture.as_deref().map_or(look::Picture::None, look::Picture::File),
        failed: failed(e),
    }
}

/// Measure a card `width` wide.
fn card_lay(ui: &egui::Ui, p: &olo_kit::theme::Palette, e: &store::Entry, span: usize, width: f32) -> CardLaid {
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
    let face = look::lay(ui, &entry_face(e), &ink, &room);
    let buttons: Vec<(String, String)> = if e.live {
        e.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(e.via.as_deref(), k)).cloned().collect()
    } else {
        Vec::new()
    };
    let mut height = m::CARD_INSET + CARD_HEAD + CARD_HEAD_GAP + face.height + m::CARD_INSET;
    if !buttons.is_empty() {
        height += m::SPACING + CARD_BUTTON;
    }
    CardLaid { span, height: height.ceil(), face, ink, buttons }
}

/// One note as a card in `r` (`style::card`): a quiet line of whose it
/// is and when (a lit dot while unread; a close box in their place under
/// the pointer), then its face; fading as it ages.
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut egui::Ui, p: &olo_kit::theme::Palette, r: Rect, laid: CardLaid, i: usize, e: &store::Entry, selected: bool, strength: f32, row: usize, rows: usize) -> Option<Did> {
    let resp = ui.interact(r, ui.make_persistent_id(("card", i, e.id)), Sense::click());
    let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
    resp.widget_info(|| WidgetInfo { selected: Some(selected), ..WidgetInfo::labeled(egui::Role::ListItem, true, title_text) });
    ui.ctx().accesskit_node_builder(resp.id, |n| {
        n.set_row_index(row);
        n.set_row_count(rows);
    });
    let hovered = resp.hovered() || ui.rect_contains_pointer(r);
    let mut painter = ui.painter().clone();
    if !selected && !hovered {
        painter.multiply_opacity(fade_opacity(strength));
    }
    let ink = style::card_ink(p);
    style::card(&painter, r, p, hovered, selected);
    let inner = r.shrink(m::CARD_INSET);
    // Whose and when: the app's mark and name, the time at the right.
    let head = Rect::from_min_size(inner.min, vec2(inner.width(), CARD_HEAD));
    let icon = Rect::from_min_size(pos2(head.left(), head.center().y - CARD_MARK / 2.0), vec2(CARD_MARK, CARD_MARK));
    paint_icon(&painter, icon, e, false);
    let time = crate::when(e.time);
    let time_w = style::label_width(ui.ctx(), &time, style::small_font());
    let name_x = icon.right() + 5.0;
    let name_room = head.right() - time_w - CARD_DOT - 8.0 - name_x;
    style::label_fit(&painter, name_x, head.center().y, Align::Min, app_name(e), egui::FontId::new(style::SMALL_SIZE, style::bold()), ink.dim, name_room);
    let forgettable = !e.shown && hovered;
    if !forgettable {
        style::label(&painter, head.right(), head.center().y, Align::Max, time, style::small_font(), ink.dim);
        if e.unread {
            unread_dot(&painter, pos2(head.right() - time_w - CARD_DOT / 2.0 - 1.0, head.center().y), p);
        }
    }
    look::paint(ui, &painter, &entry_face(e), &laid.face, &laid.ink, pos2(inner.left(), head.bottom() + CARD_HEAD_GAP), inner.width());
    let mut did = None;
    // Its app's actions, small, at the bottom right as in an alert.
    if !laid.buttons.is_empty() {
        let mut bx = inner.right();
        let by = inner.bottom() - CARD_BUTTON;
        for (key, label) in laid.buttons.iter().rev() {
            let bw = (style::label_width(ui.ctx(), label, style::small_font()) + 16.0).max(56.0).min(inner.width()).ceil();
            if bx - bw < inner.left() && bx < inner.right() {
                break;
            }
            let br = Rect::from_min_size(pos2(bx - bw, by), vec2(bw, CARD_BUTTON));
            if ui.put(br, Button::new(label).small().min_width(bw)).clicked() {
                did = Some(Did::Action(i, key.clone()));
            }
            bx -= bw + m::TIGHT;
        }
    }
    // Forget it: a × where the time was, while the pointer is over it.
    if forgettable {
        let close = Rect::from_center_size(pos2(head.right() - 6.0, head.center().y), vec2(12.0, 12.0));
        let cr = ui.interact(close.expand(3.0), ui.make_persistent_id(("forget", i, e.id)), Sense::click());
        cr.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, "Forget"));
        close_box(ui.painter(), close, if cr.hovered() { ink.text } else { ink.dim });
        if cr.clicked() {
            did = Some(Did::Forget(vec![i]));
        }
    }
    if did.is_none() {
        if resp.double_clicked() {
            did = Some(Did::Open(i));
        } else if resp.clicked() {
            did = Some(Did::Select(e.id));
        }
    }
    did
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
