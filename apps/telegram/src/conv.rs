//! The conversation on the window's right: its header (picture, name,
//! who's typing or when they were last seen), a forum's topics, the
//! messages from the oldest loaded to the newest (laid out once per
//! width, only those in view painted; scrolling up loads older pages,
//! keeping what's in view where it was), and the field to write in, with
//! what's being answered or edited and the files going with it.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::PathBuf,
};

use olo_kit::{
    Menu,
    egui::{self, Color32, FontId, Key, Rect, Sense, Ui, pos2, vec2},
    metrics as m, style,
    widgets::{self, ScrollView},
};
use olo_tg::{ChatId, ChatKind, Media, Message, MsgId, Pic};

use crate::{
    bubble::{self, Hit, Laid, Look, Place},
    when,
};

pub const HEADER: f32 = 52.0;
const DAY: f32 = 30.0;
const RUN_GAP: f32 = 8.0;
const GAP: f32 = 2.0;
const MARGIN: f32 = 16.0;

/// The conversation showing, and what's being written in it.
pub struct Conv {
    pub chat: ChatId,
    pub topic: Option<MsgId>,
    pub text: String,
    pub reply: Option<MsgId>,
    pub edit: Option<MsgId>,
    pub files: Vec<PathBuf>,
    /// The first message that was unread when it opened.
    pub unread_from: Option<MsgId>,
    /// Laid-out messages, by id (ours being sent by negative local id).
    laid: HashMap<i64, (u64, Laid)>,
    width: f32,
    ppp: f32,
    theme: u64,
    /// Last frame: the content's height, the offset, the view's height,
    /// the oldest message.
    last: (f32, f32, f32, Option<MsgId>),
    pub at_bottom: bool,
    /// Scroll to this message once it's laid out (and flash it).
    pub jump: Option<MsgId>,
    flash: Option<(MsgId, f64)>,
    pub revealed: HashSet<MsgId>,
    pub typing_sent: f64,
    pub focus_field: bool,
}

impl Conv {
    pub fn new(chat: ChatId, topic: Option<MsgId>, text: String) -> Conv {
        Conv {
            chat,
            topic,
            text,
            reply: None,
            edit: None,
            files: Vec::new(),
            unread_from: None,
            laid: HashMap::new(),
            width: 0.0,
            ppp: 0.0,
            theme: 0,
            last: (0.0, 0.0, 0.0, None),
            at_bottom: true,
            jump: None,
            flash: None,
            revealed: HashSet::new(),
            typing_sent: 0.0,
            focus_field: true,
        }
    }

}

/// What the person did in the conversation.
pub enum Act {
    Send,
    Older,
    Read(MsgId),
    Typing,
    /// A menu command on a message.
    Message(MsgId, String),
    Hit(MsgId, Hit),
    Attach,
    Info,
    Topic(Option<MsgId>),
    CancelReply,
    Unstage(usize),
    /// Jump to the newest.
    Bottom,
}

fn key_of(m: &Message) -> i64 {
    match m.pending {
        Some((local, _)) => -(local as i64),
        None => m.id as i64,
    }
}

/// What a layout depends on in a message.
fn digest(m: &Message, place: Place, revealed: bool, time: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.text.hash(&mut h);
    m.edited.hash(&mut h);
    m.views.hash(&mut h);
    m.pending.map(|p| p.0).hash(&mut h);
    m.reply_to.as_ref().map(|r| (r.from.clone(), r.text.clone())).hash(&mut h);
    for r in &m.reactions {
        (&r.emoji, r.count, r.mine).hash(&mut h);
    }
    format!("{:?}", m.media).hash(&mut h);
    m.service.hash(&mut h);
    (place.first, place.last, place.many, revealed, time).hash(&mut h);
    olo_kit::emoji::ready().hash(&mut h);
    h.finish()
}

/// One entry in the list: a day's heading, the unread line, or a message.
enum Item<'a> {
    Day(String),
    Unread,
    Msg(&'a Message, Place),
}

pub struct ConvUi<'a> {
    pub store: &'a crate::store::Store,
    pub pics: &'a mut crate::pics::Pics,
    pub backend: &'a olo_tg::Backend,
    pub h24: bool,
    pub me: i64,
    pub playing: Option<(ChatId, MsgId, f32)>,
}

pub fn show(ui: &mut Ui, rect: Rect, c: &mut Conv, u: ConvUi) -> Vec<Act> {
    let mut acts = Vec::new();
    let p = style::palette(ui.ctx());
    let look = Look::of(&p);
    let now = ui.input(|i| i.time);
    let chat = u.store.chats.get(&c.chat).cloned().unwrap_or_default();
    let painter = ui.painter_at(rect);

    // The header.
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
    painter.rect_filled(head, 0.0, style::c(p.panel_background));
    let pic = Rect::from_center_size(pos2(head.left() + m::SPACING * 2.0 + 18.0, head.center().y), vec2(36.0, 36.0));
    let tex = if chat.photo.is_some() { u.pics.get(&Pic::Avatar(chat.id), u.backend).cloned() } else { None };
    if chat.kind == ChatKind::Saved {
        painter.circle_filled(pic.center(), 18.0, look.accent);
    } else {
        bubble::avatar(&painter, pic, chat.id, &chat.title, tex.as_ref(), look.dark);
        olo_kit::layout_review::own_grid(ui, pic);
    }
    let x = pic.right() + 10.0;
    let ink = style::c(p.panel_text);
    let dim = style::disabled_text(&p);
    style::label(&painter, x, head.top() + 18.0, egui::Align::LEFT, &chat.title, FontId::new(style::PLAIN_SIZE + 1.0, style::bold()), ink);
    let (status, live) = match u.store.doing(c.chat, c.topic, crate::now()) {
        Some(t) => (t, true),
        None => (subtitle(&chat, u.store, c.topic), false),
    };
    style::label(&painter, x, head.top() + 35.0, egui::Align::LEFT, &status, style::small_font(), if live { look.accent } else { dim });
    let info = Rect::from_center_size(pos2(head.right() - 24.0, head.center().y), vec2(28.0, 28.0));
    let ir = ui.interact(info, ui.make_persistent_id("info"), Sense::click());
    if ir.hovered() {
        painter.circle_filled(info.center(), 14.0, ink.gamma_multiply(0.08));
    }
    painter.circle_stroke(info.center(), 8.0, egui::Stroke::new(1.4, ink));
    style::label(&painter, info.center().x, info.center().y, egui::Align::Center, "i", FontId::new(11.0, style::bold()), ink);
    ir.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Chat Info"));
    if ir.clicked() {
        acts.push(Act::Info);
    }
    let title_area = ui.interact(Rect::from_min_max(head.min, pos2(info.left() - 8.0, head.bottom())), ui.make_persistent_id("head"), Sense::click());
    if title_area.clicked() {
        acts.push(Act::Info);
    }
    style::hline(&painter, head.left(), head.right(), head.bottom() - style::hairline(ui.ctx()), style::c(p.shadow).gamma_multiply(0.5));
    let mut top = head.bottom();

    // A forum's topics, as chips.
    if let Some(topics) = u.store.topics.get(&c.chat).filter(|t| !t.is_empty()) {
        let row = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), 32.0));
        painter.rect_filled(row, 0.0, style::c(p.panel_background));
        let mut x = row.left() + m::SPACING;
        let all = std::iter::once((None, "All".to_owned(), 0u32, 0x6FB9F0u32)).chain(topics.iter().map(|t| (Some(t.id), t.emoji.clone().map_or_else(|| t.title.clone(), |e| format!("{e} {}", t.title)), t.unread, t.color)));
        for (id, title, unread, color) in all {
            let font = FontId::proportional(style::SMALL_SIZE + 0.5);
            let tw = style::label_width(ui.ctx(), &title, font.clone()) + 6.0;
            // Counts the names' size: emoji in names are centred on their capitals.
            let count_font = FontId::new(style::SMALL_SIZE + 0.5, style::bold());
            let badge = if unread > 0 { style::pill_width(ui.ctx(), &unread.to_string(), count_font.clone()) + 4.0 } else { 0.0 };
            let w = tw + badge + 26.0;
            if x + w > row.right() {
                break;
            }
            let chip = style::snap(ui.ctx(), Rect::from_min_size(pos2(x, row.top() + 5.0), vec2(w, 22.0)));
            let chosen = c.topic == id;
            let r = ui.interact(chip, ui.make_persistent_id(("topic", id)), Sense::click());
            let fill = if chosen { style::selection_ink(&p).fill } else if r.hovered() { ink.gamma_multiply(0.08) } else { Color32::TRANSPARENT };
            painter.rect_filled(chip, 11.0, fill);
            // The dot on the name's capitals, the count on its baseline.
            let base = style::baseline_at(ui.ctx(), chip.center().y, font.size);
            let rgb = Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8);
            painter.circle_filled(pos2(chip.left() + 12.0, base - font.size * style::CAP_HEIGHT / 2.0), 4.0, rgb);
            let tink = if chosen { style::selection_ink(&p).text } else { ink };
            let g = one_line(ui.ctx(), &title, font.clone(), tink, tw + 20.0);
            let gp = style::galley_on_baseline(ui.ctx(), &g.0, chip.left() + 20.0, base);
            painter.galley(gp, g.0.clone(), tink);
            olo_kit::emoji::paint_in(&painter, gp, &g.0, &g.1, tink);
            if unread > 0 {
                style::pill_on_baseline(&painter, chip.left() + 20.0 + tw, base, &unread.to_string(), count_font, look.accent, Color32::WHITE);
            }
            r.widget_info(|| egui::WidgetInfo::selected(egui::accesskit::Role::Tab, true, chosen, &title));
            if r.clicked() {
                acts.push(Act::Topic(id));
            }
            x = chip.right() + 4.0;
        }
        style::hline(&painter, row.left(), row.right(), row.bottom() - style::hairline(ui.ctx()), style::c(p.shadow).gamma_multiply(0.35));
        top = row.bottom();
    }

    // The composer, from the bottom up.
    let composer_h = composer_height(ui.ctx(), c, rect.width());
    let bottom = Rect::from_min_max(pos2(rect.left(), rect.bottom() - composer_h), rect.max);
    let list = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), bottom.top()));
    painter.rect_filled(list, 0.0, look.ground);

    // The messages.
    let thread = u.store.threads.get(&(c.chat, c.topic));
    let col = (list.width() - 2.0 * MARGIN - olo_kit::widgets::SCROLL_BAR).max(200.0);
    let ppp = ui.ctx().pixels_per_point();
    let theme = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (p.mode as u8, p.document_background, p.keyboard_navigation).hash(&mut h);
        h.finish()
    };
    if (col - c.width).abs() > 0.5 || ppp != c.ppp || theme != c.theme {
        c.laid.clear();
        c.width = col;
        c.ppp = ppp;
        c.theme = theme;
    }
    let mut items: Vec<Item> = Vec::new();
    if let Some(t) = thread {
        let all: Vec<&Message> = t.all().filter(|m| c.topic.is_none() || m.topic == c.topic || m.id == c.topic.unwrap_or(0)).collect();
        let many = chat.kind.many();
        let mut last_day = None;
        for (i, m) in all.iter().enumerate() {
            let day = when::day_number(m.date);
            if last_day != Some(day) {
                items.push(Item::Day(when::day(m.date, when::now())));
                last_day = Some(day);
            }
            if c.unread_from == Some(m.id) && m.pending.is_none() {
                items.push(Item::Unread);
            }
            let same = |a: &Message, b: &Message| a.service.is_none() && b.service.is_none() && a.out == b.out && a.from.as_ref().map(|f| f.id) == b.from.as_ref().map(|f| f.id) && (b.date - a.date).abs() < 300 && when::day_number(a.date) == when::day_number(b.date);
            let first = i == 0 || !same(all[i - 1], m) || c.unread_from == Some(m.id);
            let last = i + 1 == all.len() || !same(m, all[i + 1]) || c.unread_from == Some(all[i + 1].id);
            items.push(Item::Msg(m, Place { first, last, many }));
        }
    }
    // Heights, laying out what's new.
    let mut ys = Vec::with_capacity(items.len());
    let mut y = MARGIN;
    for (i, it) in items.iter().enumerate() {
        let h = match it {
            Item::Day(_) | Item::Unread => DAY,
            Item::Msg(m, place) => {
                let time = when::time(m.date, u.h24);
                let key = key_of(m);
                let revealed = c.revealed.contains(&m.id);
                let d = digest(m, *place, revealed, &time);
                if c.laid.get(&key).is_none_or(|(k, _)| *k != d) {
                    let name = m.from.as_ref().map(|f| f.name.as_str());
                    let laid = bubble::lay(ui.ctx(), m, *place, &look, col, &time, name, revealed);
                    c.laid.insert(key, (d, laid));
                }
                c.laid[&key].1.height
            }
        };
        ys.push(y);
        let gap = match (it, items.get(i + 1)) {
            (Item::Msg(_, pl), Some(Item::Msg(..))) if !pl.last => GAP,
            _ => RUN_GAP,
        };
        y += h + gap;
    }
    let content = y + MARGIN - RUN_GAP;
    let view_h = list.height();
    // Keep what's in view where it was: at the bottom if it was, else
    // the same messages when older ones came in above.
    let (last_content, last_offset, last_view, last_oldest) = c.last;
    let oldest = thread.and_then(|t| t.oldest());
    let mut offset = last_offset;
    if c.at_bottom {
        offset = (content - view_h).max(0.0);
    } else if oldest != last_oldest && last_oldest.is_some() && content > last_content {
        offset = last_offset + (content - last_content);
    }
    if let Some(id) = c.jump {
        if let Some(i) = items.iter().position(|it| matches!(it, Item::Msg(m, _) if m.id >= id && m.pending.is_none())) {
            offset = (ys[i] - view_h / 3.0).clamp(0.0, (content - view_h).max(0.0));
            c.jump = None;
            c.at_bottom = false;
            c.flash = Some((id, now));
        }
    }
    if offset != last_offset || (view_h - last_view).abs() > 0.5 {
        ScrollView::scroll_to(ui.ctx(), ("conv", c.chat, c.topic), vec2(0.0, offset));
    }
    // Shorter than the view: the messages sit at its bottom.
    let lift = (view_h - content).max(0.0);
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|q| list.contains(*q));
    let clicked = ui.input(|i| i.pointer.primary_clicked());
    let mut seen_bottom = None;
    let mut vp_top = 0.0;
    let mut hovered_msg: Option<(MsgId, Option<Hit>)> = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
        ScrollView::new(("conv", c.chat, c.topic)).border(false).background(rgba(look.ground)).auto_hide(true).show_in(ui, list, |ui, vp| {
            vp_top = vp.min.y;
            ui.set_min_size(vec2(list.width() - olo_kit::widgets::SCROLL_BAR, content.max(view_h)));
            let origin = ui.max_rect().min + vec2(MARGIN, lift);
            let painter = ui.painter().clone();
            for (i, it) in items.iter().enumerate() {
                let y = ys[i];
                let h = match it {
                    Item::Msg(m, _) => c.laid[&key_of(m)].1.height,
                    _ => DAY,
                };
                if y + h + lift < vp.min.y - 200.0 || y + lift > vp.max.y + 200.0 {
                    continue;
                }
                let o = origin + vec2(0.0, y);
                match it {
                    Item::Day(t) => {
                        let font = FontId::new(style::SMALL_SIZE, style::bold());
                        let w = style::label_width(ui.ctx(), t, font.clone()) + 20.0;
                        let r = Rect::from_center_size(pos2(origin.x + col / 2.0, o.y + DAY / 2.0), vec2(w, 20.0));
                        painter.rect_filled(style::snap(ui.ctx(), r), 10.0, look.pill);
                        style::label(&painter, r.center().x, r.center().y, egui::Align::Center, t, font, look.pill_text);
                    }
                    Item::Unread => {
                        let r = style::snap(ui.ctx(), Rect::from_min_size(pos2(list.left(), o.y + 3.0), vec2(list.width(), DAY - 6.0)));
                        painter.rect_filled(r, 0.0, look.theirs.gamma_multiply(0.85));
                        style::label(&painter, origin.x + col / 2.0, r.center().y, egui::Align::Center, "Unread Messages", FontId::new(style::SMALL_SIZE, style::bold()), look.link);
                    }
                    Item::Msg(msg, place) => {
                        let laid = &c.laid[&key_of(msg)].1;
                        let pic = match &msg.media {
                            Some(md) if md.visual() => {
                                let key = Pic::Thumb(msg.chat, msg.id);
                                if msg.pending.is_none() { u.pics.get(&key, u.backend).cloned() } else { None }
                            }
                            _ => None,
                        };
                        let sender_pic = if laid.avatar && msg.from.as_ref().is_some_and(|f| f.photo.is_some()) {
                            msg.from.as_ref().and_then(|f| u.pics.get(&Pic::Avatar(f.id), u.backend).cloned())
                        } else {
                            None
                        };
                        let progress = u.pics.progress.get(&Pic::File(msg.chat, msg.id)).copied();
                        let playing = u.playing.filter(|pl| pl.0 == msg.chat && pl.1 == msg.id).map(|pl| pl.2);
                        let flash = c.flash.filter(|f| f.0 == msg.id && now - f.1 < 1.2).is_some();
                        let pp = bubble::Paint {
                            look: &look,
                            place: *place,
                            picture: pic.as_ref(),
                            sender_pic: sender_pic.as_ref(),
                            progress,
                            read: msg.out && msg.id <= chat.read_outbox,
                            playing,
                            selected: flash,
                        };
                        let hit = bubble::paint(&painter, o, msg, laid, &pp, pointer);
                        // The time is a stamp in the balloon's corner, set on
                        // the text's baseline; a sender's picture is art.
                        olo_kit::layout_review::own_grid(ui, Rect::from_min_size(o + laid.meta.1.to_vec2(), laid.meta.0.size()).expand(1.0));
                        if laid.avatar {
                            olo_kit::layout_review::own_grid(ui, Rect::from_min_size(pos2(o.x, o.y + laid.bubble.bottom() - bubble::AVATAR), vec2(bubble::AVATAR, bubble::AVATAR)));
                        }
                        let whole = Rect::from_min_max(pos2(o.x + laid.bubble.left(), o.y), pos2(o.x + laid.bubble.right(), o.y + laid.height));
                        if pointer.is_some_and(|q| whole.contains(q)) || hit.is_some() {
                            hovered_msg = Some((msg.id, hit));
                        }
                        // For agents: each message as a list item.
                        let who = if msg.out { "You".to_owned() } else { msg.from.as_ref().map_or_else(String::new, |f| f.name.clone()) };
                        let said = if who.is_empty() { msg.summary() } else { format!("{who}: {}", msg.summary()) };
                        style::describe_item(ui.ctx(), &said, whole, false);
                        if msg.pending.is_none() && !msg.out && o.y + laid.height <= vp.max.y + origin.y - ui.max_rect().min.y + 4.0 {
                            seen_bottom = Some(seen_bottom.map_or(msg.id, |s: MsgId| s.max(msg.id)));
                        }
                    }
                }
            }
            if c.flash.is_some_and(|f| now - f.1 < 1.2) {
                ui.ctx().request_repaint();
            }
        });
    });
    c.at_bottom = vp_top >= content - view_h - 24.0 || content <= view_h;
    c.last = (content, vp_top, view_h, oldest);
    if c.jump.is_some() || u.pics.progress.keys().any(|k| matches!(k, Pic::File(ch, _) if *ch == c.chat)) || u.playing.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    // Older pages, as the top comes near.
    if let Some(t) = thread
        && t.more
        && !t.loading
        && t.loaded
        && vp_top < 800.0
    {
        acts.push(Act::Older);
    }
    if let Some(id) = seen_bottom {
        acts.push(Act::Read(id));
    }
    // Clicks and the context menu on messages.
    let list_resp = ui.interact(list, ui.make_persistent_id(("conv-list", c.chat)), Sense::click());
    if let Some((id, hit)) = &hovered_msg {
        if hit.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if clicked && let Some(h) = hit.clone() {
            acts.push(Act::Hit(*id, h));
        }
        if list_resp.double_clicked() {
            acts.push(Act::Message(*id, "reply".into()));
        }
    }
    if let Some(id) = hovered_msg.as_ref().map(|h| h.0) {
        ui.ctx().data_mut(|d| d.insert_temp(ui.make_persistent_id("menu-msg"), id));
    }
    let menu_msg: Option<MsgId> = ui.ctx().data(|d| d.get_temp(ui.make_persistent_id("menu-msg")));
    let target = menu_msg.and_then(|id| thread.and_then(|t| t.msgs.get(&id)));
    if let Some(cmd) = widgets::context_menu(&list_resp, || message_menu(target, &chat, u.me)) {
        if let Some(id) = menu_msg {
            acts.push(Act::Message(id, cmd));
        }
    }
    // Down to the newest.
    if !c.at_bottom {
        let b = Rect::from_center_size(pos2(list.right() - 34.0, list.bottom() - 30.0), vec2(40.0, 40.0));
        let r = ui.interact(b, ui.make_persistent_id("to-bottom"), Sense::click());
        painter.circle_filled(b.center() + vec2(0.0, 1.0), 20.0, Color32::from_black_alpha(30));
        painter.circle_filled(b.center(), 20.0, if r.hovered() { look.theirs.lerp_to_gamma(look.text, 0.06) } else { look.theirs });
        painter.circle_stroke(b.center(), 20.0, egui::Stroke::new(style::hairline(ui.ctx()), look.theirs_edge));
        let s = egui::Stroke::new(1.8, look.dim);
        painter.line_segment([b.center() + vec2(-6.0, -3.0), b.center() + vec2(0.0, 3.0)], s);
        painter.line_segment([b.center() + vec2(0.0, 3.0), b.center() + vec2(6.0, -3.0)], s);
        if chat.unread > 0 {
            style::pill(&painter, b.center().x - 10.0, b.top() - 2.0, &chat.unread.to_string(), FontId::new(9.5, style::bold()), look.accent, Color32::WHITE);
        }
        r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Scroll to Newest"));
        if r.clicked() {
            acts.push(Act::Bottom);
        }
    }
    if thread.is_none_or(|t| !t.loaded) {
        style::label(&painter, list.center().x, list.center().y, egui::Align::Center, "Loading messages…", style::plain_font(), look.dim);
    } else if items.is_empty() {
        style::label(&painter, list.center().x, list.center().y, egui::Align::Center, "No messages here yet.", style::plain_font(), look.dim);
    }

    composer(ui, bottom, c, u.store, &chat, &look, &mut acts);
    acts
}

fn rgba(c: Color32) -> olo_kit::theme::Rgba {
    [c.r(), c.g(), c.b(), c.a()]
}

fn one_line(ctx: &egui::Context, text: &str, font: FontId, color: Color32, width: f32) -> (std::sync::Arc<egui::Galley>, Vec<String>) {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let (job, found) = olo_kit::emoji::make_room(&job);
    (ctx.fonts_mut(|f| f.layout_job(job)), found)
}

/// Under the name: "online", "last seen …", "5 members", a topic's name.
fn subtitle(chat: &olo_tg::Chat, store: &crate::store::Store, topic: Option<MsgId>) -> String {
    if let Some(t) = topic.and_then(|t| store.topics.get(&chat.id)?.iter().find(|x| x.id == t)) {
        return t.title.clone();
    }
    match chat.kind {
        ChatKind::Saved => "your cloud storage".into(),
        ChatKind::Bot => "bot".into(),
        ChatKind::User => chat.seen.as_ref().map_or_else(|| "last seen recently".into(), |s| s.describe(when::now())),
        ChatKind::Channel => chat.members.map_or_else(|| "channel".into(), |n| format!("{} subscribers", bubble::short_count(n))),
        ChatKind::Group | ChatKind::Forum => chat.members.map_or_else(|| "group".into(), |n| format!("{n} members")),
    }
}

fn message_menu(m: Option<&Message>, chat: &olo_tg::Chat, _me: i64) -> Menu {
    let Some(m) = m else { return Menu::new("") };
    let mine = m.out;
    let has_text = !m.text.is_empty();
    let file = matches!(m.media, Some(Media::Photo { .. } | Media::Video { .. } | Media::File { .. } | Media::Audio { .. } | Media::Voice { .. }));
    let mut react = Menu::new("React");
    for e in ["👍", "❤️", "😂", "😮", "😢", "🙏", "🔥", "🎉", "👏", "🤔"] {
        let mine_too = m.reactions.iter().any(|r| r.mine && r.emoji == e);
        react = react.item(e, format!("react:{e}"), None).checked(mine_too);
    }
    let mut menu = Menu::new("").item("Reply", "reply", None).enabled(m.pending.is_none() && m.service.is_none());
    if mine && has_text {
        menu = menu.item("Edit", "edit", None).enabled(m.pending.is_none());
    }
    menu = menu
        .item("Copy Text", "copy", None)
        .enabled(has_text)
        .submenu(react)
        .separator()
        .item("Forward…", "forward", None)
        .enabled(m.pending.is_none() && m.service.is_none())
        .item(if m.pinned { "Unpin" } else { "Pin" }, "pin", None)
        .enabled(m.pending.is_none() && chat.kind != ChatKind::Channel || mine);
    if file {
        menu = menu.separator().item("Open", "open", None).item("Save As…", "save", None);
    }
    menu.separator().item("Delete…", "delete", None).enabled(m.pending.is_none())
}

/// How tall the field to write in needs to be.
fn composer_height(ctx: &egui::Context, c: &Conv, width: f32) -> f32 {
    let w = (width - 2.0 * m::SPACING - 36.0 - 44.0 - 16.0).max(80.0);
    let lines = if c.text.is_empty() {
        1.0
    } else {
        let g = ctx.fonts_mut(|f| f.layout_job(egui::text::LayoutJob::simple(c.text.clone(), FontId::proportional(bubble::TEXT), Color32::WHITE, w)));
        g.rows.len().max(1) as f32
    };
    let field = (lines.min(8.0) * 17.0 + 16.0).max(36.0);
    let reply = if c.reply.is_some() || c.edit.is_some() { 42.0 } else { 0.0 };
    let files = if c.files.is_empty() { 0.0 } else { 32.0 };
    field + reply + files + 2.0 * m::SPACING
}

fn composer(ui: &mut Ui, rect: Rect, c: &mut Conv, store: &crate::store::Store, chat: &olo_tg::Chat, look: &Look, acts: &mut Vec<Act>) {
    let p = style::palette(ui.ctx());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, style::c(p.panel_background));
    style::hline(&painter, rect.left(), rect.right(), rect.top(), style::c(p.shadow).gamma_multiply(0.5));
    let mut y = rect.top() + m::SPACING;
    // What's being answered or edited.
    let quoted = c.edit.or(c.reply).and_then(|id| store.threads.get(&(c.chat, c.topic)).and_then(|t| t.msgs.get(&id)));
    if let Some(q) = quoted {
        let bar = Rect::from_min_size(pos2(rect.left() + m::SPACING, y), vec2(rect.width() - 2.0 * m::SPACING, 34.0));
        painter.rect_filled(Rect::from_min_size(bar.min + vec2(28.0, 2.0), vec2(2.5, 30.0)), 1.0, look.accent);
        // A curved arrow (reply) or a pencil (edit).
        let ic = pos2(bar.left() + 12.0, bar.center().y);
        let s = egui::Stroke::new(1.6, look.accent);
        if c.edit.is_some() {
            painter.line_segment([ic + vec2(-5.0, 5.0), ic + vec2(4.0, -4.0)], egui::Stroke::new(3.0, look.accent));
            painter.line_segment([ic + vec2(-6.0, 6.0), ic + vec2(-4.5, 4.5)], s);
        } else {
            painter.line_segment([ic + vec2(-6.0, 0.0), ic + vec2(-1.0, -5.0)], s);
            painter.line_segment([ic + vec2(-6.0, 0.0), ic + vec2(-1.0, 5.0)], s);
            painter.line_segment([ic + vec2(-6.0, 0.0), ic + vec2(6.0, 0.0)], s);
        }
        let head = if c.edit.is_some() { "Edit Message".to_owned() } else { format!("Reply to {}", q.from.as_ref().map_or("message", |f| f.name.as_str())) };
        style::label(&painter, bar.left() + 38.0, bar.top() + 10.0, egui::Align::LEFT, &head, FontId::new(style::SMALL_SIZE + 0.5, style::bold()), look.accent);
        let (g, found) = one_line(ui.ctx(), &q.summary(), style::small_font(), style::c(p.panel_text), bar.width() - 80.0);
        let gp = pos2(bar.left() + 38.0, bar.top() + 18.0);
        painter.galley(gp, g.clone(), style::c(p.panel_text));
        olo_kit::emoji::paint_in(&painter, gp, &g, &found, style::c(p.panel_text));
        let x = Rect::from_center_size(pos2(bar.right() - 12.0, bar.center().y), vec2(22.0, 22.0));
        let xr = ui.interact(x, ui.make_persistent_id("cancel-reply"), Sense::click());
        close_mark(&painter, x.center(), if xr.hovered() { style::c(p.panel_text) } else { style::disabled_text(&p) });
        xr.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Cancel Reply"));
        if xr.clicked() {
            acts.push(Act::CancelReply);
        }
        y += 42.0;
    }
    // Files going with it.
    if !c.files.is_empty() {
        let mut x = rect.left() + m::SPACING + 36.0;
        for (i, f) in c.files.iter().enumerate() {
            let name = f.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let font = style::small_font();
            let w = style::label_width(ui.ctx(), &name, font.clone()).min(200.0) + 34.0;
            let chip = Rect::from_min_size(pos2(x, y), vec2(w, 24.0));
            painter.rect_filled(style::snap(ui.ctx(), chip), 12.0, look.accent.gamma_multiply(0.15));
            let (g, _) = one_line(ui.ctx(), &name, font, style::c(p.panel_text), w - 34.0);
            painter.galley(pos2(chip.left() + 10.0, chip.center().y - g.size().y / 2.0), g, style::c(p.panel_text));
            let xr = Rect::from_center_size(pos2(chip.right() - 12.0, chip.center().y), vec2(18.0, 18.0));
            let r = ui.interact(xr, ui.make_persistent_id(("unstage", i)), Sense::click());
            close_mark(&painter, xr.center(), style::disabled_text(&p));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, format!("Remove {name}")));
            if r.clicked() {
                acts.push(Act::Unstage(i));
            }
            x = chip.right() + 6.0;
        }
        y += 32.0;
    }
    // Attach, the field, send.
    let field_h = rect.bottom() - m::SPACING - y;
    let attach = Rect::from_min_size(pos2(rect.left() + m::SPACING, rect.bottom() - m::SPACING - 36.0), vec2(32.0, 36.0));
    let ar = ui.interact(attach, ui.make_persistent_id("attach"), Sense::click());
    paperclip(&painter, attach.center(), if ar.hovered() { look.accent } else { style::disabled_text(&p) });
    ar.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Attach Files"));
    if ar.clicked() {
        acts.push(Act::Attach);
    }
    let send = Rect::from_min_size(pos2(rect.right() - m::SPACING - 36.0, rect.bottom() - m::SPACING - 36.0), vec2(36.0, 36.0));
    let well = Rect::from_min_max(pos2(attach.right() + 4.0, y), pos2(send.left() - 8.0, y + field_h));
    painter.rect(style::snap(ui.ctx(), well), 18.0, style::c(p.document_background), egui::Stroke::new(style::hairline(ui.ctx()), style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.22))), egui::StrokeKind::Inside);
    let can_send = !c.text.trim().is_empty() || !c.files.is_empty();
    let sr = ui.interact(send, ui.make_persistent_id("send"), Sense::click());
    let fill = if can_send { look.accent } else { look.accent.gamma_multiply(0.35) };
    painter.circle_filled(send.center(), 17.0, if sr.hovered() && can_send { fill.lerp_to_gamma(Color32::BLACK, 0.1) } else { fill });
    plane(&painter, send.center(), Color32::WHITE);
    sr.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, can_send, if c.edit.is_some() { "Save" } else { "Send" }));
    if sr.clicked() && can_send {
        acts.push(Act::Send);
    }
    let inner = well.shrink2(vec2(14.0, 8.0));
    let id = egui::Id::unique(("composer", c.chat, c.topic));
    // Return sends; ⇧Return a new line.
    let focused = ui.ctx().memory(|mem| mem.has_focus(id));
    if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) && can_send {
        acts.push(Act::Send);
    }
    if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) && (c.reply.is_some() || c.edit.is_some()) {
        acts.push(Act::CancelReply);
    }
    let hint = match chat.kind {
        ChatKind::Channel => "Broadcast",
        _ if c.edit.is_some() => "Edit message",
        _ => "Message",
    };
    let before = c.text.len();
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.add_sized(
                inner.size(),
                egui::TextEdit::multiline(&mut c.text).id(id).frame(egui::Frame::NONE).hint_text(hint).font(FontId::proportional(bubble::TEXT)).desired_rows(1).lock_focus(true).desired_width(inner.width()),
            )
        })
        .inner;
    widgets::edit_menu(ui, id, &resp, &mut c.text, true, false);
    if c.focus_field {
        resp.request_focus();
        c.focus_field = false;
    }
    if c.text.len() != before && !c.text.is_empty() {
        acts.push(Act::Typing);
    }
}

fn close_mark(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let s = egui::Stroke::new(1.5, ink);
    p.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], s);
    p.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], s);
}

/// The attach button's paperclip.
fn paperclip(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let s = egui::Stroke::new(1.6, ink);
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<egui::Pos2> {
        (0..=12).map(|i| {
            let a = from + (to - from) * i as f32 / 12.0;
            pos2(c.x + cx + r * a.cos(), c.y + cy + r * a.sin())
        }).collect()
    };
    use std::f32::consts::PI;
    // Lean it like a paperclip on a desk.
    let rot = |q: egui::Pos2| {
        let (sn, cs) = (-0.6f32).sin_cos();
        let d = q - c;
        c + vec2(d.x * cs - d.y * sn, d.x * sn + d.y * cs)
    };
    let mut pts = vec![pos2(c.x + 3.0, c.y + 4.0), pos2(c.x + 3.0, c.y - 6.0)];
    pts.extend(arc(0.0, -6.0, 3.0, 0.0, -PI));
    pts.push(pos2(c.x - 3.0, c.y + 7.0));
    pts.extend(arc(1.0, 7.0, 4.0, PI, 0.0).into_iter().skip(1));
    pts.push(pos2(c.x + 5.0, c.y - 8.0));
    p.add(egui::Shape::line(pts.into_iter().map(rot).collect(), s));
}

/// The send button's paper plane.
fn plane(p: &egui::Painter, c: egui::Pos2, ink: Color32) {
    let tip = c + vec2(8.0, 0.0);
    let pts = vec![tip, c + vec2(-7.0, -7.0), c + vec2(-4.0, 0.0), c + vec2(-7.0, 7.0)];
    p.add(egui::Shape::convex_polygon(vec![pts[0], pts[1], pts[2]], ink, egui::Stroke::NONE));
    p.add(egui::Shape::convex_polygon(vec![pts[0], pts[2], pts[3]], ink, egui::Stroke::NONE));
}
