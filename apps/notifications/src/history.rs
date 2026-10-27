//! The History window: what came, newest first, in one of four views
//! (View menu, remembered in `notifications.json`). Rows are dense: a
//! small icon, the title in bold with the app and time on the same line,
//! one line of the body under it; the selected one opens up to its whole
//! body and its actions. A dot marks what came since the window was last
//! opened. A search field filters, and the keyboard works the list
//! (arrows, Return to open, Delete to forget, Escape).

use std::collections::{BTreeSet, HashSet};

use tabula_kit::{
    Cx, WindowId,
    egui::{self, Align, Color32, Key, Rect, Sense, WidgetInfo, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    theme::mix,
    widgets::{Button, CheckBox, ScrollView, TextControl},
};
use tabula_protocol::HistoryView;

use crate::{Notifications, close_box, galley, named_icons, now_secs, raise_app, server::Reason, store};

/// Above and below a row's text.
const ROW_PAD: f32 = 5.0;
/// The small icon in a row.
const SMALL_ICON: f32 = 16.0;
/// Where a row's text begins: the unread dot, then the icon.
const TEXT_X: f32 = ICON_X + SMALL_ICON + 7.0;
/// Where a row's icon begins (the dot is before it).
const ICON_X: f32 = 14.0;
/// A group's or a day's heading.
const HEADING: f32 = 22.0;
/// The status line along the bottom.
const FOOTER: f32 = 18.0;
/// A card's least width, and its body's lines.
const CARD_W: f32 = 176.0;
const CARD_ROWS: usize = 3;
/// Room for the × at a row's right end.
const CLOSE_W: f32 = 18.0;
const SCROLL_ID: &str = "history";

/// What the History window remembers while it is open (and across a
/// reload, see `save_state`).
#[derive(Default)]
pub struct HistoryState {
    pub search: String,
    /// The selected note's id.
    pub selected: Option<u32>,
    /// The apps folded away in By App.
    pub folded: BTreeSet<String>,
    /// Unread when the window was opened: they keep their dot until it
    /// closes or they are selected.
    pub fresh: HashSet<u32>,
    /// The search field's id (for ⌘F) and whether to focus it.
    search_id: Option<egui::Id>,
    pub focus_search: bool,
    /// Bring the selection into view, for this many frames (a new view
    /// takes two: the first is clamped to the old view's height).
    reveal: u8,
}

/// One line of the list.
enum Item {
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

/// The lines of `view` for the entries at `shown` (newest first).
fn items(entries: &[store::Entry], shown: &[usize], view: HistoryView, folded: &BTreeSet<String>, searching: bool) -> Vec<Item> {
    let mut out = Vec::new();
    match view {
        HistoryView::List => {
            let mut last = None;
            for &i in shown {
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

        // What shows.
        let entries = &self.history.entries;
        let searching = !self.view.search.trim().is_empty();
        let shown: Vec<usize> = (0..entries.len()).rev().filter(|&i| !searching || found(&entries[i], &self.view.search)).collect();
        let lines = items(entries, &shown, view, &self.view.folded, searching);
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
        let cols = if view == HistoryView::Cards { (((list.width() - 16.0 - m::SPACING) / (CARD_W + m::SPACING)).floor() as usize).max(1) } else { 1 };
        let (step, home) = ui.input_mut(|i| {
            let mut step = 0isize;
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                step = cols as isize;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                step = -(cols as isize);
            }
            if view == HistoryView::Cards && !typing {
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowRight) {
                    step = 1;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft) {
                    step = -1;
                }
            }
            let home = if typing { None } else if i.consume_key(egui::Modifiers::NONE, Key::Home) { Some(false) } else if i.consume_key(egui::Modifiers::NONE, Key::End) { Some(true) } else { None };
            (step, home)
        });
        if !order.is_empty() {
            let to = match (home, selected) {
                (Some(end), _) => Some(if end { order.len() - 1 } else { 0 }),
                (None, _) if step == 0 => None,
                (None, None) => Some(if step > 0 { 0 } else { order.len() - 1 }),
                (None, Some(s)) => Some((s as isize + step).clamp(0, order.len() as isize - 1) as usize),
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
            let fresh = &self.view.fresh;
            let sel_id = self.view.selected;
            ScrollView::new(SCROLL_ID).background(p.document_background).show(&mut child, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let top = ui.cursor().top();
                let w = ui.available_width();
                let mut row = 0;
                let mut cards = lines.iter().filter_map(|l| if let Item::Note { i, .. } = l { Some(*i) } else { None }).peekable();
                if view == HistoryView::Cards {
                    let card_w = ((w - m::SPACING) / cols as f32 - m::SPACING).floor();
                    ui.add_space(m::SPACING);
                    while cards.peek().is_some() {
                        // A row of cards as tall as its tallest.
                        let laid: Vec<(usize, CardText)> = cards.by_ref().take(cols).map(|i| (i, card_text(ui, &p, &entries[i], card_w))).collect();
                        let card_h = laid.iter().map(|(_, t)| t.height).fold(0.0, f32::max);
                        let (r, _) = ui.allocate_exact_size(vec2(w, card_h + m::SPACING), Sense::hover());
                        for (col, (i, laid)) in laid.into_iter().enumerate() {
                            let cr = Rect::from_min_size(pos2(r.left() + m::SPACING + col as f32 * (card_w + m::SPACING), r.top()), vec2(card_w, card_h));
                            let e = &entries[i];
                            let is_sel = sel_id == Some(e.id);
                            if is_sel && reveal {
                                ScrollView::reveal(ui.ctx(), SCROLL_ID, cr.top() - top - m::SPACING, cr.bottom() - top + m::SPACING);
                            }
                            if let Some(d) = card(ui, &p, cr, laid, i, e, is_sel, e.unread || fresh.contains(&e.id), row, order.len()) {
                                did = Some(d);
                            }
                            row += 1;
                        }
                    }
                    return;
                }
                for line in &lines {
                    match line {
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
                            let r = note_row(ui, &p, w, &RowArgs { i: *i, e, older, entries, meta, selected: is_sel, unread: e.unread || fresh.contains(&e.id), indent: *grouped, row, rows: order.len() }, &mut did);
                            if is_sel && reveal {
                                ScrollView::reveal(ui.ctx(), SCROLL_ID, r.top() - top, r.bottom() - top);
                            }
                            row += 1;
                        }
                    }
                }
            });
        }

        // The status line.
        let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOTER), full.max);
        let total = self.history.entries.len();
        let mut status = if searching { format!("{} of {total}", shown.len()) } else if total == 1 { "1 notification".into() } else { format!("{total} notifications") };
        if self.quiet() {
            status += if self.settings.dnd { " · Do Not Disturb" } else { " · Quiet by schedule" };
        }
        style::label(ui.painter(), foot.left() + m::SPACING, foot.center().y, Align::Min, status, style::small_font(), style::disabled_text(&p));

        match did {
            Some(Did::Quiet(on)) => self.set_quiet(on, 0),
            Some(Did::Clear) => self.clear_history(cx),
            Some(Did::Select(id)) => {
                self.view.selected = Some(id);
                self.view.fresh.remove(&id);
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
        if e.live && e.actions.iter().any(|(k, _)| *k == key) {
            self.invoke(cx, Some(window), e.id, key);
        } else {
            raise_app(e.desktop_entry.as_deref(), &e.app);
        }
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
fn heading_fill(ui: &egui::Ui, p: &tabula_kit::theme::Palette, r: Rect) {
    ui.painter().rect_filled(r, 0.0, style::darken(p.document_background, 1.04));
    style::hline(ui.painter(), r.left(), r.right(), r.bottom() - 1.0, style::darken(p.document_background, 1.1));
}

/// The icon a note shows, in `r`.
fn paint_icon(painter: &egui::Painter, r: Rect, e: &store::Entry) {
    let names = named_icons(e.icon.as_deref(), e.desktop_entry.as_deref(), &e.app, e.kind == store::KindName::Error);
    let _ = names.iter().any(|n| icons::paint(painter, r, n));
}

/// The dot for what is new.
fn unread_dot(painter: &egui::Painter, center: egui::Pos2, p: &tabula_kit::theme::Palette) {
    painter.circle_filled(center, 3.0, c(p.keyboard_navigation));
}

/// The edge an important note (blue) or an error (red) wears.
fn kind_edge(e: &store::Entry, p: &tabula_kit::theme::Palette) -> Option<Color32> {
    match e.kind {
        store::KindName::Important => Some(c(p.keyboard_navigation)),
        store::KindName::Error => Some(c(p.failure)),
        _ => None,
    }
}

/// An app's heading in By App: its latch, icon, name and count; a click
/// folds it; Clear forgets the app's notes.
#[allow(clippy::too_many_arguments)]
fn group(ui: &mut egui::Ui, p: &tabula_kit::theme::Palette, w: f32, app: &str, count: usize, members: &[usize], open: bool, entries: &[store::Entry]) -> Option<Did> {
    let (r, resp) = ui.allocate_exact_size(vec2(w, HEADING + 2.0), Sense::click());
    let name = if app.is_empty() { "Notification" } else { app };
    resp.widget_info(|| WidgetInfo { selected: Some(open), ..WidgetInfo::labeled(egui::Role::Button, true, format!("{name} ({count})")) });
    heading_fill(ui, p, r);
    let text = c(p.document_text);
    let dim = style::disabled_text(p);
    style::latch(ui.painter(), Rect::from_center_size(pos2(r.left() + 7.0, r.center().y), vec2(10.0, 10.0)), open, if resp.hovered() { text } else { dim });
    paint_icon(ui.painter(), Rect::from_center_size(pos2(r.left() + ICON_X + SMALL_ICON / 2.0, r.center().y), vec2(SMALL_ICON, SMALL_ICON)), &entries[members[0]]);
    let n = style::label(ui.painter(), r.left() + TEXT_X, r.center().y, Align::Min, name, style::bold_font(), text);
    style::label(ui.painter(), n.right() + 6.0, r.center().y, Align::Min, count.to_string(), style::small_font(), dim);
    let mut did = None;
    if resp.hovered() || ui.rect_contains_pointer(r) {
        let font = style::small_font();
        let g = ui.painter().layout_no_wrap("Clear".into(), font.clone(), dim);
        let cr = Rect::from_min_max(pos2(r.right() - 8.0 - g.size().x - 6.0, r.top() + 3.0), pos2(r.right() - 4.0, r.bottom() - 4.0));
        let clear = ui.interact(cr, ui.make_persistent_id(("clear-group", app)), Sense::click());
        clear.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, format!("Clear {name}")));
        style::label(ui.painter(), cr.right() - 4.0, r.center().y, Align::Max, "Clear", font, if clear.hovered() { text } else { dim });
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
    /// Under its app's heading.
    indent: bool,
    row: usize,
    rows: usize,
}

/// One note as a row; its rect.
fn note_row(ui: &mut egui::Ui, p: &tabula_kit::theme::Palette, w: f32, a: &RowArgs, did: &mut Option<Did>) -> Rect {
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
    let title = galley(ui, title_text, c(fg), title_w, if a.selected { 3 } else { 1 }, style::bold_font());
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
        e.actions.iter().map(|(k, l)| (k.clone(), if k == "default" && l.trim().is_empty() { "Open".to_owned() } else { l.clone() })).collect()
    } else {
        vec![("default".into(), format!("Show {}", app_name(e)))]
    };
    let mut h = ROW_PAD + title.size().y + body.as_ref().map_or(0.0, |b| 1.0 + b.size().y);
    let older_h = style::SMALL_SIZE + 5.0;
    if !older.is_empty() {
        h += m::TIGHT + older.len() as f32 * older_h;
    }
    if !actions.is_empty() {
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
    let painter = ui.painter().clone();
    if a.selected {
        painter.rect_filled(r, 0.0, c(bg));
    } else if resp.hovered() {
        painter.rect_filled(r, 0.0, style::darken(p.document_background, 1.04));
    }
    if let Some(color) = kind_edge(e, p) {
        painter.rect_filled(Rect::from_min_size(r.min + vec2(x0, 2.0), vec2(3.0, r.height() - 4.0)), 0.0, color);
    }
    // The title's first line, the app and time, the dot and the icon all
    // centre on one line.
    let first_h = title.rows.first().map_or(title.size().y, |row| row.rect().height());
    let line1 = r.top() + ROW_PAD + first_h / 2.0;
    if a.unread {
        unread_dot(&painter, pos2(r.left() + x0 + 7.0, line1), p);
    }
    paint_icon(&painter, Rect::from_center_size(pos2(r.left() + x0 + ICON_X + SMALL_ICON / 2.0, line1), vec2(SMALL_ICON, SMALL_ICON)), e);
    let x = r.left() + text_x;
    let title_r = Rect::from_min_size(style::text_origin(ui.ctx(), &title, x, line1, style::PLAIN_SIZE), title.size());
    let mut y = title_r.top();
    style::describe_text(ui.ctx(), egui::accesskit::Role::Label, title_text, title_r, None);
    painter.galley(title_r.min, title.clone(), c(fg));
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
    if !actions.is_empty() {
        y += m::TIGHT;
        let mut bx = x;
        for (key, label) in &actions {
            let bw = (painter.layout_no_wrap(label.clone(), style::plain_font(), dim).size().x + 2.0 * m::CONTROL_PAD).max(m::BUTTON_MIN_WIDTH * 0.75).ceil();
            if bx + bw > r.right() - 4.0 && bx > x {
                break;
            }
            let br = Rect::from_min_size(pos2(bx, y), vec2(bw, m::CONTROL_HEIGHT));
            if ui.put(br, Button::new(label)).clicked() {
                *did = Some(Did::Action(a.i, key.clone()));
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
        close_box(ui.painter(), close, if cr.hovered() { c(fg) } else { dim });
        forget = cr.clicked();
    }
    if !a.selected {
        style::hline(ui.painter(), r.left() + text_x, r.right(), r.bottom() - 1.0, style::darken(p.document_background, 1.07));
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

/// A card's title and body laid out, and the height they need.
struct CardText {
    title: std::sync::Arc<egui::Galley>,
    body: Option<std::sync::Arc<egui::Galley>>,
    height: f32,
}

/// Lay out a card `width` wide: its app line, the title, up to three
/// lines of body.
fn card_text(ui: &egui::Ui, p: &tabula_kit::theme::Palette, e: &store::Entry, width: f32) -> CardText {
    let inner = width - 2.0 * m::SPACING;
    let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
    let title = galley(ui, title_text, c(p.document_text), inner, 2, style::bold_font());
    let soft = c(mix(p.document_text, p.document_background, 0.2));
    let body = (!e.text.is_empty()).then(|| galley(ui, &e.text, soft, inner, CARD_ROWS, style::plain_font()));
    let height = m::SPACING + SMALL_ICON + m::TIGHT + title.size().y + body.as_ref().map_or(0.0, |b| 2.0 + b.size().y) + m::SPACING;
    CardText { title, body, height: height.ceil() }
}

/// One note as a card in `r`.
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut egui::Ui, p: &tabula_kit::theme::Palette, r: Rect, laid: CardText, i: usize, e: &store::Entry, selected: bool, unread: bool, row: usize, rows: usize) -> Option<Did> {
    let resp = ui.interact(r, ui.make_persistent_id(("card", i, e.id)), Sense::click());
    let title_text = if e.summary.is_empty() { app_name(e) } else { &e.summary };
    resp.widget_info(|| WidgetInfo { selected: Some(selected), ..WidgetInfo::labeled(egui::Role::ListItem, true, title_text) });
    ui.ctx().accesskit_node_builder(resp.id, |n| {
        n.set_row_index(row);
        n.set_row_count(rows);
    });
    let painter = ui.painter().clone();
    let fill = if resp.hovered() { style::lighten(p.document_background, 0.97) } else { c(p.document_background) };
    painter.rect_filled(r, 0.0, fill);
    style::frame_rect(&painter, r, style::darken(p.document_background, 1.18));
    if selected {
        style::frame_rect(&painter, r, c(p.keyboard_navigation));
        style::frame_rect(&painter, r.shrink(1.0), c(p.keyboard_navigation));
    }
    if let Some(color) = kind_edge(e, p) {
        painter.rect_filled(Rect::from_min_size(r.min + vec2(1.0, 1.0), vec2(3.0, r.height() - 2.0)), 0.0, color);
    }
    let text = c(p.document_text);
    let dim = style::disabled_text(p);
    let inner = r.shrink(m::SPACING);
    let line1 = inner.top() + SMALL_ICON / 2.0;
    paint_icon(&painter, Rect::from_min_size(inner.min, vec2(SMALL_ICON, SMALL_ICON)), e);
    let right = inner.right() - 14.0;
    let t = style::label(&painter, right, line1, Align::Max, crate::when(e.time), style::small_font(), dim);
    let app = galley(ui, app_name(e), dim, (t.left() - inner.left() - SMALL_ICON - 12.0).max(10.0), 1, style::small_font());
    let app_pos = style::text_origin(ui.ctx(), &app, inner.left() + SMALL_ICON + 6.0, line1, style::small_font().size);
    style::describe_text(ui.ctx(), egui::accesskit::Role::Label, app.text(), Rect::from_min_size(app_pos, app.size()), None);
    painter.galley(app_pos, app, dim);
    let mut y = inner.top() + SMALL_ICON + m::TIGHT;
    let title = laid.title;
    style::describe_text(ui.ctx(), egui::accesskit::Role::Label, title_text, Rect::from_min_size(pos2(inner.left(), y), title.size()), None);
    painter.galley(pos2(inner.left(), y), title.clone(), text);
    y += title.size().y + 2.0;
    if let Some(body) = laid.body {
        let soft = c(mix(p.document_text, p.document_background, 0.2));
        style::describe_text(ui.ctx(), egui::accesskit::Role::Label, &e.text, Rect::from_min_size(pos2(inner.left(), y), body.size()), None);
        painter.galley(pos2(inner.left(), y), body, soft);
    }
    let close = Rect::from_center_size(pos2(inner.right() - 4.0, line1), vec2(12.0, 12.0));
    let mut did = None;
    if !e.shown && (resp.hovered() || ui.rect_contains_pointer(close.expand(3.0))) {
        let cr = ui.interact(close.expand(3.0), ui.make_persistent_id(("forget", i, e.id)), Sense::click());
        cr.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, "Forget"));
        close_box(&painter, close, if cr.hovered() { text } else { dim });
        if cr.clicked() {
            did = Some(Did::Forget(vec![i]));
        }
    } else if unread {
        unread_dot(&painter, pos2(inner.right() - 4.0, line1), p);
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
        let list = items(&entries, &shown, HistoryView::List, &BTreeSet::new(), false);
        assert!(matches!(&list[0], Item::Day(d) if d == "Today"));
        assert!(matches!(list.last(), Some(Item::Note { i: 0, .. })));
        assert_eq!(notes(&list), 4);
        // By app: Chat (newest) first; folding hides its notes.
        let folded: BTreeSet<String> = ["Chat".to_owned()].into();
        let apps = items(&entries, &shown, HistoryView::Apps, &folded, false);
        assert!(matches!(&apps[0], Item::Group { app, count: 2, open: false, .. } if app == "Chat"));
        assert_eq!(notes(&apps), 2);
        // ...but not while searching.
        assert_eq!(notes(&items(&entries, &shown, HistoryView::Apps, &folded, true)), 4);
        // Conversations: Mail's two Lunches as one, the older under it.
        let threads = items(&entries, &shown, HistoryView::Conversations, &BTreeSet::new(), false);
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
