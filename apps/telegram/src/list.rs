//! The chat list down the window's left: a search field, the account's
//! folders as chips, and the chats (pinned first) with their pictures,
//! latest messages, times and unread counts; the archive behind a row of
//! its own. While searching: the chats whose names match, then
//! Telegram's people, chats and messages.

use olo_kit::{
    Menu,
    egui::{self, Color32, FontId, Rect, Sense, Ui, vec2},
    metrics as m,
    search::SearchField,
    style,
    widgets::{self, ScrollView},
};
use olo_tg::{Backend, Chat, ChatId, ChatKind, Message, MsgId, Pic};

use crate::{bubble, pics::Pics, store::Store, when};

pub const ROW: f32 = 60.0;
const PICTURE: f32 = 46.0;
pub const WIDTH: f32 = 300.0;
pub const MIN_WIDTH: f32 = 220.0;

/// Which chats the list shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// A folder (its id), or all chats.
    pub folder: Option<i32>,
    pub archive: bool,
    pub query: String,
}

pub enum Act {
    Open(ChatId),
    OpenAt(ChatId, MsgId),
    Command(ChatId, String),
    Search(String),
}

/// What a search found on Telegram.
#[derive(Default)]
pub struct Found {
    pub query: String,
    pub chats: Vec<Chat>,
    pub msgs: Vec<Message>,
}

/// The chats the view shows, in order.
pub fn shown(store: &Store, view: &View) -> Vec<ChatId> {
    let folder = view.folder.and_then(|id| store.folders.iter().find(|f| f.id == id));
    let q = view.query.trim().to_lowercase();
    let mut out: Vec<ChatId> = store
        .order
        .iter()
        .filter(|id| {
            let Some(c) = store.chats.get(id) else { return false };
            if !q.is_empty() {
                return c.title.to_lowercase().contains(&q) || c.username.as_ref().is_some_and(|u| u.to_lowercase().contains(&q));
            }
            match folder {
                Some(f) => f.admits(c),
                None => c.archived == view.archive,
            }
        })
        .copied()
        .collect();
    // A folder's own pins first, in its order.
    if let Some(f) = folder.filter(|_| q.is_empty()) {
        out.sort_by_key(|id| f.pinned.iter().position(|p| p == id).unwrap_or(usize::MAX));
    }
    out
}

/// The text a row's name and preview say, for agents.
fn describe(c: &Chat) -> String {
    let mut s = c.title.clone();
    if c.unread > 0 {
        s.push_str(&format!(", {} unread", c.unread));
    }
    if c.mentions > 0 {
        s.push_str(", mentioned");
    }
    if c.muted {
        s.push_str(", muted");
    }
    if let Some(l) = &c.last {
        s.push_str(": ");
        s.push_str(&l.text);
    }
    s
}

pub struct ListUi<'a> {
    pub store: &'a Store,
    pub pics: &'a mut Pics,
    pub backend: &'a Backend,
    pub view: &'a mut View,
    pub selected: Option<ChatId>,
    pub found: &'a Found,
    pub h24: bool,
}

/// What a row draws from.
struct Rows<'a> {
    store: &'a Store,
    pics: &'a mut Pics,
    backend: &'a Backend,
    h24: bool,
}

pub fn show(ui: &mut Ui, rect: Rect, l: ListUi) -> Option<Act> {
    let p = style::palette(ui.ctx());
    let mut act = None;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, style::c(p.panel_background));
    // The search field.
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), m::TOOLBAR_HEIGHT));
    let before = l.view.query.clone();
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(m::SPACING, m::TIGHT))), |ui| {
        ui.add(SearchField::new("chat-search", &mut l.view.query).hint("Search").width(bar.width() - 2.0 * m::SPACING));
    });
    if l.view.query != before && !l.view.query.trim().is_empty() {
        act = Some(Act::Search(l.view.query.trim().to_owned()));
    }
    let mut top = bar.bottom();
    // Folders, as chips (with the archive's back row when in it).
    let searching = !l.view.query.trim().is_empty();
    if !searching && (l.view.archive || !l.store.folders.is_empty()) {
        let row = Rect::from_min_size(egui::pos2(rect.left(), top), vec2(rect.width(), 28.0));
        if l.view.archive {
            let r = ui.interact(row, ui.make_persistent_id("archive-back"), Sense::click());
            let ink = style::c(p.panel_text);
            back_arrow(&painter, egui::pos2(row.left() + m::SPACING + 4.0, row.center().y), ink);
            style::label(&painter, row.left() + m::SPACING + 16.0, row.center().y, egui::Align::LEFT, "Archived Chats", style::bold_font(), ink);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Back to Chats"));
            if r.clicked() {
                l.view.archive = false;
            }
        } else {
            let mut x = row.left() + m::SPACING;
            let chips: Vec<(Option<i32>, String, u32)> = std::iter::once((None, "All Chats".to_owned(), 0))
                .chain(l.store.folders.iter().map(|f| {
                    let unread = l.store.chats.values().filter(|c| f.admits(c) && !c.muted).filter(|c| c.unread > 0 || c.marked).count() as u32;
                    (Some(f.id), f.title.clone(), unread)
                }))
                .collect();
            let font = FontId::proportional(style::SMALL_SIZE + 0.5);
            let width_of = |title: &str, unread: u32| {
                let badge = if unread > 0 { style::pill_width(ui.ctx(), &unread.to_string(), FontId::proportional(9.0)) + 4.0 } else { 0.0 };
                style::label_width(ui.ctx(), title, font.clone()) + badge + 16.0
            };
            // As many as fit; the rest behind a last chip with a menu.
            let room = row.right() - m::SPACING - x;
            let total: f32 = chips.iter().map(|c| width_of(&c.1, c.2) + 4.0).sum();
            let mut shown = chips.len();
            if total > room {
                let mut used = 0.0;
                shown = 0;
                for c in &chips {
                    let w = width_of(&c.1, c.2) + 4.0;
                    if used + w > room - 70.0 {
                        break;
                    }
                    used += w;
                    shown += 1;
                }
            }
            let hidden: Vec<(Option<i32>, String, u32)> = chips[shown..].to_vec();
            let current = l.view.folder;
            let chip_ui = |id: Option<i32>, title: &str, unread: u32, x: &mut f32, more: bool| -> egui::Response {
                let w = width_of(title, unread) + if more { 12.0 } else { 0.0 };
                let chip = Rect::from_min_size(egui::pos2(*x, row.top() + 3.0), vec2(w, 22.0));
                let chosen = if more { hidden.iter().any(|h| h.0 == current) } else { current == id };
                let r = ui.interact(chip, ui.make_persistent_id(("folder", id, more)), Sense::click());
                let (fill, ink) = if chosen {
                    let s = style::selection_ink(&p);
                    (s.fill, s.text)
                } else if r.hovered() {
                    (style::c(p.panel_text).gamma_multiply(0.08), style::c(p.panel_text))
                } else {
                    (Color32::TRANSPARENT, style::c(p.panel_text))
                };
                let chip = style::snap(ui.ctx(), chip);
                painter.rect_filled(chip, 11.0, fill);
                let tw = style::label_width(ui.ctx(), title, font.clone());
                // The name level with its count's (the pill sets its own on device pixels).
                let mut cy = chip.center().y;
                if unread > 0 {
                    cy = style::pill(&painter, chip.left() + 8.0 + tw + 4.0, cy, &unread.to_string(), FontId::proportional(9.0), style::c(p.keyboard_navigation), Color32::WHITE).center().y;
                }
                style::label(&painter, chip.left() + 8.0, cy, egui::Align::LEFT, title, font.clone(), ink);
                if more {
                    let c = egui::pos2(chip.right() - 10.0, chip.center().y);
                    let s = egui::Stroke::new(1.3, ink);
                    painter.line_segment([c + vec2(-3.0, -1.5), c + vec2(0.0, 1.5)], s);
                    painter.line_segment([c + vec2(0.0, 1.5), c + vec2(3.0, -1.5)], s);
                }
                r.widget_info(|| egui::WidgetInfo::selected(egui::accesskit::Role::Tab, true, chosen, title));
                *x = chip.right() + 4.0;
                r
            };
            for (id, title, unread) in &chips[..shown] {
                if chip_ui(*id, title, *unread, &mut x, false).clicked() {
                    l.view.folder = *id;
                }
            }
            if !hidden.is_empty() {
                // The chosen one's name when it's among them.
                let label = hidden.iter().find(|h| h.0 == l.view.folder).map_or_else(|| "More".to_owned(), |h| h.1.clone());
                let unread: u32 = hidden.iter().map(|h| h.2).sum();
                let source = ui.make_persistent_id("more-folders");
                let r = chip_ui(None, &label, unread, &mut x, true);
                if r.clicked() {
                    let mut menu = Menu::new("");
                    for (id, title, unread) in &hidden {
                        let t = if *unread > 0 { format!("{title} ({unread})") } else { title.clone() };
                        menu = menu.item(&t, format!("folder:{}", id.unwrap_or(0)), None).checked(l.view.folder == *id);
                    }
                    widgets::popup_menu_at(ui.ctx(), source, r.rect.left_bottom(), menu);
                }
                if let Some(cmd) = widgets::popup_result(ui.ctx(), source) {
                    l.view.folder = cmd.trim_start_matches("folder:").parse().ok();
                }
            }
        }
        top = row.bottom();
    }
    style::hline(&painter, rect.left(), rect.right(), top, style::c(p.shadow).gamma_multiply(0.5));
    let body = Rect::from_min_max(egui::pos2(rect.left(), top + style::hairline(ui.ctx())), rect.max);

    let ids = shown(l.store, l.view);
    // The archive's row, at the top of all chats.
    let archived: Vec<&Chat> = if !searching && !l.view.archive && l.view.folder.is_none() {
        l.store.order.iter().filter_map(|id| l.store.chats.get(id)).filter(|c| c.archived).collect()
    } else {
        Vec::new()
    };
    let extra = usize::from(!archived.is_empty());
    let found_rows = if searching { l.found.chats.iter().filter(|c| !ids.contains(&c.id)).count() + l.found.msgs.len() } else { 0 };
    let total = ids.len() + extra + found_rows + if searching { 2 } else { 0 };
    let ink = style::c(p.list_item_text);
    let dim = style::c(olo_theme::mix(p.list_item_text, p.list_background, 0.45));
    let now = when::now();
    let accent = style::c(p.keyboard_navigation);
    let mut pointer_row = None;
    let found = l.found;
    let view = l.view;
    let selected_chat = l.selected;
    let mut rows = Rows { store: l.store, pics: l.pics, backend: l.backend, h24: l.h24 };
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        ScrollView::new("chat-list").border(false).background(p.list_background).auto_hide(true).show_rows(ui, ROW, total, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
                let painter = ui.painter();
                // What row i is.
                let row: Row = if i < extra {
                    Row::Archive
                } else if i - extra < ids.len() {
                    Row::Chat(ids[i - extra])
                } else if searching {
                    let k = i - extra - ids.len();
                    let more_chats: Vec<&Chat> = found.chats.iter().filter(|c| !ids.contains(&c.id)).collect();
                    if k == 0 {
                        Row::Heading(if found.query == view.query.trim() { "Global search" } else { "Searching…" })
                    } else if k - 1 < more_chats.len() {
                        Row::Found(more_chats[k - 1])
                    } else if k - 1 == more_chats.len() {
                        Row::Heading("Messages")
                    } else {
                        match found.msgs.get(k - 2 - more_chats.len()) {
                            Some(m) => Row::Msg(m),
                            None => Row::Heading(""),
                        }
                    }
                } else {
                    Row::Heading("")
                };
                match row {
                    Row::Heading(t) => {
                        painter.rect_filled(r, 0.0, style::c(p.panel_background));
                        style::label(painter, r.left() + m::SPACING + 4.0, r.bottom() - 12.0, egui::Align::LEFT, t, FontId::new(style::SMALL_SIZE, style::bold()), dim);
                    }
                    Row::Archive => {
                        let resp = ui.interact(r, ui.make_persistent_id("archive"), Sense::click());
                        if resp.hovered() {
                            painter.rect_filled(r, 0.0, ink.gamma_multiply(0.05));
                        }
                        let pic = Rect::from_center_size(egui::pos2(r.left() + 10.0 + PICTURE / 2.0, r.center().y), vec2(PICTURE, PICTURE));
                        painter.circle_filled(pic.center(), PICTURE / 2.0, style::c(olo_theme::mix(p.list_background, p.list_item_text, 0.35)));
                        olo_kit::icons::paint(painter, Rect::from_center_size(pic.center(), vec2(26.0, 26.0)), "archive");
                        let x = pic.right() + 12.0;
                        style::label(painter, x, r.top() + 19.0, egui::Align::LEFT, "Archived Chats", FontId::new(style::PLAIN_SIZE + 0.5, style::bold()), ink);
                        let names = archived.iter().take(4).map(|c| c.title.as_str()).collect::<Vec<_>>().join(", ");
                        let g = one_line(ui.ctx(), &names, style::small_font(), dim, r.right() - x - 12.0);
                        painter.galley(style::text_origin(ui.ctx(), &g, x, r.top() + 41.0, style::small_font().size), g, dim);
                        let unread: u32 = archived.iter().map(|c| c.unread).sum();
                        if unread > 0 {
                            let t = unread.to_string();
                            let w = style::pill_width(ui.ctx(), &t, FontId::proportional(10.0));
                            style::pill(painter, r.right() - 12.0 - w, r.top() + 41.0, &t, FontId::proportional(10.0), dim, Color32::WHITE);
                        }
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Archived Chats"));
                        if resp.clicked() {
                            view.archive = true;
                        }
                    }
                    Row::Found(c) => {
                        if chat_row(ui, r, c, &mut rows, &p, now, accent, false).clicked() {
                            act = Some(Act::Open(c.id));
                        }
                    }
                    Row::Msg(msg) => {
                        let resp = ui.interact(r, ui.make_persistent_id(("msg", msg.chat, msg.id)), Sense::click());
                        if resp.hovered() {
                            painter.rect_filled(r, 0.0, ink.gamma_multiply(0.05));
                        }
                        let title = rows.store.chats.get(&msg.chat).map_or_else(|| msg.from.as_ref().map(|f| f.name.clone()).unwrap_or_default(), |c| c.title.clone());
                        let pic = Rect::from_center_size(egui::pos2(r.left() + 10.0 + PICTURE / 2.0, r.center().y), vec2(PICTURE, PICTURE));
                        let tex = rows.pics.get(&Pic::Avatar(msg.chat), rows.backend).cloned();
                        bubble::avatar(painter, pic, msg.chat, &title, tex.as_ref(), p.mode == olo_theme::ThemeMode::Dark);
                        let x = pic.right() + 12.0;
                        let t = when::short(msg.date, now, rows.h24);
                        let tw = style::label_width(ui.ctx(), &t, style::small_font());
                        style::label(painter, r.right() - 12.0 - tw, r.top() + 19.0, egui::Align::LEFT, &t, style::small_font(), dim);
                        let g = one_line(ui.ctx(), &title, FontId::new(style::PLAIN_SIZE + 0.5, style::bold()), ink, r.right() - x - tw - 24.0);
                        painter.galley(style::text_origin(ui.ctx(), &g, x, r.top() + 19.0, style::PLAIN_SIZE + 0.5), g, ink);
                        let g = one_line(ui.ctx(), &msg.summary(), style::small_font(), dim, r.right() - x - 12.0);
                        painter.galley(style::text_origin(ui.ctx(), &g, x, r.top() + 41.0, style::small_font().size), g, dim);
                        style::describe_item(ui.ctx(), &format!("{title}: {}", msg.summary()), r, false);
                        if resp.clicked() {
                            act = Some(Act::OpenAt(msg.chat, msg.id));
                        }
                    }
                    Row::Chat(id) => {
                        let Some(c) = rows.store.chats.get(&id) else { continue };
                        let selected = selected_chat == Some(id);
                        let resp = chat_row(ui, r, c, &mut rows, &p, now, accent, selected);
                        if resp.hovered() {
                            pointer_row = Some(id);
                        }
                        if resp.clicked() {
                            act = Some(Act::Open(id));
                        }
                        let menu = || {
                            Menu::new("")
                                .item(if c.pinned { "Unpin" } else { "Pin" }, "chat:pin", None)
                                .item(if c.muted { "Unmute" } else { "Mute" }, "chat:mute", None)
                                .item(if c.unread > 0 || c.marked { "Mark as Read" } else { "Mark as Unread" }, "chat:read", None)
                                .separator()
                                .item(if c.archived { "Unarchive" } else { "Archive" }, "chat:archive", None)
                        };
                        if let Some(cmd) = widgets::context_menu(&resp, menu) {
                            act = Some(Act::Command(id, cmd));
                        }
                    }
                }
            }
        });
    });
    if ids.is_empty() && !searching && rows.store.complete {
        let msg = if view.archive { "No archived chats." } else if view.folder.is_some() { "No chats in this folder." } else { "No chats yet." };
        style::label(&painter, body.center().x, body.top() + 40.0, egui::Align::Center, msg, style::plain_font(), dim);
    }
    let _ = pointer_row;
    act
}

enum Row<'a> {
    Archive,
    Chat(ChatId),
    Found(&'a Chat),
    Msg(&'a Message),
    Heading(&'static str),
}

fn one_line(ctx: &egui::Context, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.replace('\n', " "), font, color, width.max(10.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let (job, found) = olo_kit::emoji::make_room(&job);
    let g = ctx.fonts_mut(|f| f.layout_job(job));
    if !found.is_empty() {
        // Remembered with the galley for painting.
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(("emoji", std::sync::Arc::as_ptr(&g) as usize)), found));
    }
    g
}

/// Paint a one-line galley made by `one_line`, its emoji in colour.
fn put(painter: &egui::Painter, pos: egui::Pos2, g: std::sync::Arc<egui::Galley>, ink: Color32) {
    let found: Option<Vec<String>> = painter.ctx().data(|d| d.get_temp(egui::Id::unique(("emoji", std::sync::Arc::as_ptr(&g) as usize))));
    painter.galley(pos, g.clone(), ink);
    if let Some(found) = found {
        olo_kit::emoji::paint_in(painter, pos, &g, &found, ink);
    }
}

#[allow(clippy::too_many_arguments)]
fn chat_row(ui: &mut Ui, r: Rect, c: &Chat, l: &mut Rows, p: &olo_theme::Palette, now: i64, accent: Color32, selected: bool) -> egui::Response {
    let resp = ui.interact(r, ui.make_persistent_id(("chat", c.id)), Sense::click());
    let painter = ui.painter();
    let sel = style::selection_ink(p);
    let (ink, dim) = if selected {
        (sel.text, sel.dim)
    } else {
        (style::c(p.list_item_text), style::c(olo_theme::mix(p.list_item_text, p.list_background, 0.45)))
    };
    if selected {
        painter.rect_filled(r, 0.0, sel.fill);
    } else if resp.hovered() {
        painter.rect_filled(r, 0.0, ink.gamma_multiply(0.05));
    }
    let pic = Rect::from_center_size(egui::pos2(r.left() + 10.0 + PICTURE / 2.0, r.center().y), vec2(PICTURE, PICTURE));
    if c.kind == ChatKind::Saved {
        painter.circle_filled(pic.center(), PICTURE / 2.0, accent);
        bookmark(painter, pic.center(), Color32::WHITE);
    } else {
        let tex = if c.photo.is_some() { l.pics.get(&Pic::Avatar(c.id), l.backend).cloned() } else { None };
        bubble::avatar(painter, pic, c.id, &c.title, tex.as_ref(), p.mode == olo_theme::ThemeMode::Dark);
    }
    if c.seen == Some(olo_tg::Seen::Online) {
        let dot = pic.right_bottom() - vec2(7.0, 7.0);
        painter.circle_filled(dot, 6.0, if selected { sel.fill } else { style::c(p.list_background) });
        painter.circle_filled(dot, 4.5, Color32::from_rgb(0x4C, 0xC1, 0x4F));
    }
    let x = pic.right() + 12.0;
    let right = r.right() - 12.0;
    // Time, then the title in what's left of the line.
    let (t, out_read) = match &c.last {
        Some(l2) => (when::short(l2.date.max(if c.draft.is_some() { c.order } else { 0 }), now, l.h24), (l2.out && !matches!(c.kind, ChatKind::Saved | ChatKind::Channel)).then(|| c.last_read())),
        None => (String::new(), None),
    };
    let tw = style::label_width(ui.ctx(), &t, style::small_font());
    style::label(painter, right - tw, r.top() + 19.0, egui::Align::LEFT, &t, style::small_font(), dim);
    let mut tx = right - tw;
    if let Some(read) = out_read {
        tx -= 16.0;
        let s = egui::Stroke::new(1.3, if read && !selected { accent } else { dim });
        let at = egui::pos2(tx, r.top() + 19.0);
        painter.line_segment([at, at + vec2(3.0, 3.0)], s);
        painter.line_segment([at + vec2(3.0, 3.0), at + vec2(9.0, -3.5)], s);
        if read {
            painter.line_segment([at + vec2(7.0, 3.0), at + vec2(13.0, -3.5)], s);
        }
    }
    let mut title_room = tx - x - 8.0;
    let mut marks = 0.0;
    if c.muted {
        marks += 14.0;
    }
    if c.verified {
        marks += 16.0;
    }
    title_room -= marks;
    let tg = one_line(ui.ctx(), &c.title, FontId::new(style::PLAIN_SIZE + 0.5, style::bold()), ink, title_room);
    let tsize = tg.size();
    let at = style::text_origin(ui.ctx(), &tg, x, r.top() + 19.0, style::PLAIN_SIZE + 0.5);
    put(painter, at, tg, ink);
    let mut mx = x + tsize.x + 4.0;
    if c.verified {
        let cc = egui::pos2(mx + 6.0, r.top() + 19.0);
        painter.circle_filled(cc, 6.0, accent);
        let s = egui::Stroke::new(1.4, Color32::WHITE);
        painter.line_segment([cc + vec2(-3.0, 0.0), cc + vec2(-1.0, 2.2)], s);
        painter.line_segment([cc + vec2(-1.0, 2.2), cc + vec2(3.0, -2.2)], s);
        mx += 16.0;
    }
    if c.muted {
        speaker_off(painter, egui::pos2(mx + 5.0, r.top() + 19.0), dim);
    }
    // The preview, and the counts at its right.
    let mut px = right;
    let pill_font = FontId::new(10.0, style::bold());
    let count = if c.unread > 0 { Some(c.unread.to_string()) } else if c.marked { Some(String::new()) } else { None };
    if let Some(n) = &count {
        let fill = if c.muted { style::c(olo_theme::mix(p.list_item_text, p.list_background, 0.55)) } else { accent };
        let fill = if selected { Color32::WHITE } else { fill };
        let tint = if selected { sel.fill } else { Color32::WHITE };
        let w = style::pill_width(ui.ctx(), n, pill_font.clone()).max(18.0);
        let pr = style::pill(painter, right - w, r.top() + 41.0, n, pill_font.clone(), fill, tint);
        px = pr.left() - 4.0;
    }
    if c.mentions > 0 {
        let fill = if selected { Color32::WHITE } else { accent };
        let pr = style::pill(painter, px - 18.0, r.top() + 41.0, "@", pill_font.clone(), fill, if selected { sel.fill } else { Color32::WHITE });
        px = pr.left() - 4.0;
    }
    if c.pinned && count.is_none() && c.mentions == 0 && !c.archived {
        pin(painter, egui::pos2(right - 6.0, r.top() + 41.0), dim);
        px = right - 16.0;
    }
    let room = px - x - 6.0;
    let typing = l.store.doing(c.id, None, crate::now());
    let mut job = egui::text::LayoutJob { wrap: egui::text::TextWrapping { max_width: room.max(10.0), max_rows: 1, break_anywhere: true, ..Default::default() }, ..Default::default() };
    let small = style::small_font();
    let fmt = |color: Color32| egui::TextFormat { font_id: small.clone(), color, ..Default::default() };
    if let Some(t) = typing {
        job.append(&t, 0.0, fmt(if selected { ink } else { accent }));
    } else if let Some(d) = &c.draft {
        job.append("Draft: ", 0.0, fmt(if selected { ink } else { style::c(p.failure) }));
        job.append(&d.replace('\n', " "), 0.0, fmt(dim));
    } else if let Some(last) = &c.last {
        let who = if last.out && c.kind != ChatKind::Saved && c.kind != ChatKind::Channel {
            Some("You".to_owned())
        } else if c.kind.many() {
            last.from.as_ref().map(|f| f.split(' ').next().unwrap_or(f).to_owned())
        } else {
            None
        };
        if let Some(w) = who {
            job.append(&format!("{w}: "), 0.0, fmt(ink));
        }
        job.append(&last.text.replace('\n', " "), 0.0, fmt(dim));
    }
    let (job, found) = olo_kit::emoji::make_room(&job);
    let g = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let pp = style::text_origin(ui.ctx(), &g, x, r.top() + 41.0, style::small_font().size);
    painter.galley(pp, g.clone(), dim);
    olo_kit::emoji::paint_in(painter, pp, &g, &found, dim);
    // A hairline between rows, from the text's edge.
    if !selected {
        style::hline(painter, x, r.right(), r.bottom() - style::hairline(ui.ctx()), style::c(p.shadow).gamma_multiply(0.18));
    }
    style::describe_item(ui.ctx(), &describe(c), r, selected);
    resp
}

/// Saved Messages' bookmark.
fn bookmark(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let pts = vec![c + vec2(-7.0, -10.0), c + vec2(7.0, -10.0), c + vec2(7.0, 10.0), c + vec2(0.0, 4.0), c + vec2(-7.0, 10.0)];
    p.add(egui::Shape::convex_polygon(pts[..3].iter().copied().chain([c + vec2(0.0, 4.0)]).collect(), ink, egui::Stroke::NONE));
    p.add(egui::Shape::convex_polygon(vec![pts[0], pts[3], pts[4]], ink, egui::Stroke::NONE));
}

/// A muted chat's mark: a speaker with a slash.
fn speaker_off(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let body = vec![c + vec2(-5.0, -2.0), c + vec2(-2.5, -2.0), c + vec2(1.0, -5.0), c + vec2(1.0, 5.0), c + vec2(-2.5, 2.0), c + vec2(-5.0, 2.0)];
    p.add(egui::Shape::convex_polygon(body, ink, egui::Stroke::NONE));
    p.line_segment([c + vec2(-5.0, -5.5), c + vec2(5.0, 5.5)], egui::Stroke::new(1.2, ink));
}

/// A pinned chat's mark: a pin, leaning.
fn pin(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let s = egui::Stroke::new(1.4, ink);
    p.circle_filled(c + vec2(2.0, -2.5), 3.2, ink);
    p.line_segment([c + vec2(1.0, -1.5), c + vec2(-4.0, 4.5)], s);
}

fn back_arrow(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let s = egui::Stroke::new(1.6, ink);
    p.line_segment([c + vec2(4.0, -5.0), c + vec2(-1.0, 0.0)], s);
    p.line_segment([c + vec2(-1.0, 0.0), c + vec2(4.0, 5.0)], s);
}
