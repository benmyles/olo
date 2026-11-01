//! The conversation on the window's right: its header (picture, name,
//! who's typing or when they were last seen), a forum's topics, the
//! messages from the oldest loaded to the newest (laid out once per
//! width, only those in view painted, in one canvas; scrolling up
//! loads older pages, keeping what's in view where it was), and the
//! field to write in, with what's being answered or edited and the files
//! going with it.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_tg::{ChatId, ChatKind, Media, Message, MsgId, Pic};
use olo_ui::{
    Menu,
    gpui::{AnyElement, App, Bounds, ClickEvent, Context, CursorStyle, Entity, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, PathBuilder, Pixels, Point, RenderImage, ScrollHandle, StatefulInteractiveElement as _, Styled as _, Window, black, canvas, div, hsla, point, px, size, transparent_black, white},
    metrics as m,
    style::{self, Align, c},
    theme,
    widgets::{SCROLL_BAR, ScrollView, TextField},
};

use crate::{
    Tg,
    bubble::{self, Hit, Laid, Look, Place},
    when,
};

pub const HEADER: f32 = 52.0;
const DAY: f32 = 30.0;
const RUN_GAP: f32 = 8.0;
const GAP: f32 = 2.0;
const MARGIN: f32 = 16.0;
/// The topics' row, in a forum.
const TOPICS: f32 = 32.0;
/// How long a message jumped to stays lit.
const FLASH: Duration = Duration::from_millis(1200);

/// The conversation showing, and what's being written in it.
pub struct Conv {
    pub chat: ChatId,
    pub topic: Option<MsgId>,
    pub field: Entity<TextField>,
    pub reply: Option<MsgId>,
    pub edit: Option<MsgId>,
    pub files: Vec<PathBuf>,
    /// The first message that was unread when it opened.
    pub unread_from: Option<MsgId>,
    /// The messages laid out (kept with the chat when it's left).
    pub layouts: Layouts,
    /// Where each entry goes, as last planned.
    plan: Option<Plan>,
    pub scroll: ScrollHandle,
    /// Last frame: the content's height, the offset, the view's height,
    /// the oldest message.
    last: (f32, f32, f32, Option<MsgId>),
    pub at_bottom: bool,
    /// Scroll to this message once it's laid out (and flash it).
    pub jump: Option<MsgId>,
    flash: Option<(MsgId, Instant)>,
    pub revealed: HashSet<MsgId>,
    pub typing_sent: f64,
    pub focus_field: bool,
    /// What's under the pointer: a message and what clicking it does.
    pub hover: Option<(MsgId, Option<Hit>)>,
    /// The messages as last painted, for the pointer.
    painted: Rc<RefCell<Vec<Painted>>>,
}

/// A message as painted: where, and its layout.
struct Painted {
    whole: Bounds<Pixels>,
    origin: Point<Pixels>,
    m: Message,
    laid: Rc<Laid>,
}

/// A chat's messages laid out, by id (ours being sent by negative local
/// id), with a digest of what each layout depends on; for one width,
/// scale and look.
#[derive(Default)]
pub struct Layouts {
    laid: HashMap<i64, (u64, Rc<Laid>)>,
    width: f32,
    scale: f32,
    theme: u64,
}

/// What the plan was made from: when none of it changes, nor the width,
/// the plan stands.
#[derive(Clone, Copy, PartialEq)]
struct PlanKey {
    version: u64,
    unread_from: Option<MsgId>,
    h24: bool,
    today: i32,
    revealed: usize,
    many: bool,
    width: u32,
}

/// Where each entry of the conversation goes (its top in the content
/// and its height), in order: what painting, scrolling and the pointer
/// read, without going through the messages again.
struct Plan {
    key: PlanKey,
    items: Vec<Planned>,
    content: f32,
    oldest: Option<MsgId>,
    /// Some messages' heights are guesses still (laid out next frame).
    partial: bool,
}

struct Planned {
    what: What,
    y: f32,
    h: f32,
}

enum What {
    Day(String),
    Unread,
    Msg { key: i64, id: MsgId, place: Place, theirs: bool },
}

/// How long laying out may take in a frame before the rest waits for
/// the next (a chat opened with a long page of messages).
const LAY_BUDGET: Duration = Duration::from_millis(3);

/// A message's height before it's laid out: near enough to scroll by.
fn guess(m: &Message, col: f32) -> f32 {
    let w = (col.min(bubble::MAX_WIDTH) - 2.0 * bubble::PAD.0).max(50.0);
    let lines = (m.text.chars().count() as f32 * bubble::TEXT * 0.5 / w).ceil().max(1.0);
    let media = if m.media.is_some() { 160.0 } else { 0.0 };
    lines * bubble::line_height(bubble::TEXT) + 2.0 * bubble::PAD.1 + 18.0 + media
}

impl Conv {
    pub fn new(chat: ChatId, topic: Option<MsgId>, field: Entity<TextField>) -> Conv {
        Conv {
            chat,
            topic,
            field,
            reply: None,
            edit: None,
            files: Vec::new(),
            unread_from: None,
            layouts: Layouts::default(),
            plan: None,
            scroll: ScrollHandle::new(),
            last: (0.0, 0.0, 0.0, None),
            at_bottom: true,
            jump: None,
            flash: None,
            revealed: HashSet::new(),
            typing_sent: 0.0,
            focus_field: true,
            hover: None,
            painted: Rc::default(),
        }
    }

    pub fn text(&self, cx: &App) -> String {
        self.field.read(cx).text().to_owned()
    }

    /// What's under `q` (window coordinates) in the messages as last
    /// painted: which, and what a click there does.
    pub fn hit_at(&self, q: Point<Pixels>) -> Option<(MsgId, Option<Hit>)> {
        let painted = self.painted.borrow();
        let p = painted.iter().find(|p| p.whole.contains(&q))?;
        Some((p.m.id, bubble::hit_at(p.origin, &p.m, &p.laid, q)))
    }
}

/// What the person did in the conversation.
pub enum Act {
    Send,
    Older,
    Read(MsgId),
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
    h.finish()
}

/// What's painted for an entry near the view, and where (its top in the
/// content).
enum Draw {
    Day(String, f32),
    Unread(f32),
    Msg {
        m: Box<Message>,
        place: Place,
        laid: Rc<Laid>,
        y: f32,
        picture: Option<Arc<RenderImage>>,
        sender_pic: Option<Arc<RenderImage>>,
        progress: Option<f32>,
        read: bool,
        playing: Option<f32>,
        selected: bool,
    },
}

/// What a pane paints, in order.
pub type Paints = Vec<Box<dyn FnOnce(&mut Window, &mut App)>>;

fn r(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

fn disc(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    window.paint_sector(center, px(0.0), px(radius), 0.0, std::f32::consts::TAU, color);
}

/// The conversation, filling `rect` (window coordinates).
pub fn render(tg: &mut Tg, rect: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Tg>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    let look = Look::of(&p);
    let Some((chat_id, topic)) = tg.conv.as_ref().map(|c| (c.chat, c.topic)) else { return div().into_any_element() };
    let chat = tg.store.chats.get(&chat_id).cloned().unwrap_or_default();
    let (x0, y0, w) = (f32::from(rect.origin.x), f32::from(rect.origin.y), f32::from(rect.size.width));
    let mut hits: Vec<AnyElement> = Vec::new();
    let mut paint: Paints = Vec::new();
    let hot = tg.hot.clone();
    let is_hot = move |k: &str| hot.as_ref().is_some_and(|h| h.as_ref() == k);

    // The header.
    let head = r(x0, y0, w, HEADER);
    let pic_r = r(x0 + m::SPACING * 2.0, y0 + HEADER / 2.0 - 18.0, 36.0, 36.0);
    let pic = if chat.photo.is_some() { tg.pics.get(&Pic::Avatar(chat.id), &tg.backend) } else { None };
    let (status, live) = match tg.store.doing(chat_id, topic, crate::now()) {
        Some(t) => (t, true),
        None => (subtitle(&chat, &tg.store, topic), false),
    };
    let info = r(x0 + w - 24.0 - 14.0, y0 + HEADER / 2.0 - 14.0, 28.0, 28.0);
    {
        let (chat, look, info_hot) = (chat.clone(), look.clone(), is_hot("conv-info"));
        paint.push(Box::new(move |window, cx| {
            let p = theme::palette(cx);
            style::fill_rect(window, head, c(p.panel_background));
            if chat.kind == ChatKind::Saved {
                disc(window, pic_r.center(), 18.0, look.accent);
                crate::list::bookmark(window, pic_r.center(), 36.0, white());
            } else {
                bubble::avatar(window, cx, pic_r, chat.id, &chat.title, pic.as_ref(), look.dark);
                olo_ui::review::own_grid(window, pic_r);
            }
            let x = pic_r.right() + px(10.0);
            let ink = c(p.panel_text);
            let room = info.origin.x - x - px(8.0);
            let t = style::label_fit(window, cx, x, head.origin.y + px(18.0), Align::Left, chat.title.clone(), style::PLAIN_SIZE + 1.0, style::ui_font(FontWeight::BOLD), ink, room);
            olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Heading", name: Some(chat.title.clone().into()), bounds: t, enabled: true, ..Default::default() });
            let s = style::label_fit(window, cx, x, head.origin.y + px(35.0), Align::Left, status.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), if live { look.accent } else { style::disabled_text(&p) }, room);
            olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(status, s));
            if info_hot {
                disc(window, info.center(), 14.0, ink.opacity(0.08));
            }
            window.paint_sector(info.center(), px(8.0 - 0.7), px(8.0 + 0.7), 0.0, std::f32::consts::TAU, ink);
            style::label(window, cx, info.center().x, info.center().y, Align::Center, "i", 11.0, style::ui_font(FontWeight::BOLD), ink);
            olo_ui::agent::record(window, cx, olo_ui::agent::Node::button("Chat Info", info));
            style::hline_above(window, head.origin.x, head.right(), head.bottom(), c(p.shadow).opacity(0.5));
        }));
    }
    for (key, b) in [("conv-head", r(x0, y0, f32::from(info.origin.x) - 8.0 - x0, HEADER)), ("conv-info", info)] {
        let t = me.clone();
        hits.push(crate::hit(&me, key, b).on_click(move |_, window, cx| t.update(cx, |tg, cx| tg.conv_act(Act::Info, window, cx))).into_any_element());
    }
    let mut top = y0 + HEADER;

    // A forum's topics, as chips.
    if let Some(topics) = tg.store.topics.get(&chat_id).filter(|t| !t.is_empty()) {
        let row = r(x0, top, w, TOPICS);
        let mut x = x0 + m::SPACING;
        let all = std::iter::once((None, "All".to_owned(), 0u32, 0x6FB9F0u32)).chain(topics.iter().map(|t| (Some(t.id), t.emoji.clone().map_or_else(|| t.title.clone(), |e| format!("{e} {}", t.title)), t.unread, t.color)));
        let size_ = style::SMALL_SIZE + 0.5;
        let normal = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let mut chips = Vec::new();
        for (id, title, unread, color) in all {
            let tw = f32::from(style::text_width(window, title.clone(), size_, normal.clone())) + 6.0;
            let badge = if unread > 0 { style::pill_width(window, &unread.to_string(), size_, &bold) + 4.0 } else { 0.0 };
            let cw = tw + badge + 26.0;
            if x + cw > x0 + w {
                break;
            }
            let key = format!("topic:{}", id.unwrap_or(0));
            let b = r(x, top + 5.0, cw, 22.0);
            let t = me.clone();
            hits.push(crate::hit(&me, &key, b).on_click(move |_, window, cx| t.update(cx, |tg, cx| tg.conv_act(Act::Topic(id), window, cx))).into_any_element());
            chips.push((id, title, unread, color, b, tw, is_hot(&key)));
            x += cw + 4.0;
        }
        let accent = look.accent;
        paint.push(Box::new(move |window, cx| {
            let p = theme::palette(cx);
            style::fill_rect(window, row, c(p.panel_background));
            let ink = c(p.panel_text);
            for (id, title, unread, color, b, tw, hot) in chips {
                let chosen = topic == id;
                let b = style::snap(window, b);
                let face = if chosen { c(p.list_selected_background) } else if hot { ink.opacity(0.08) } else { transparent_black() };
                window.paint_quad(olo_ui::gpui::fill(b, face).corner_radii(px(11.0)));
                // The dot on the name's capitals, the count on its baseline.
                let base = style::baseline_at(window, b.center().y, size_);
                disc(window, point(b.origin.x + px(12.0), base - px(size_ * style::CAP_HEIGHT / 2.0)), 4.0, olo_ui::gpui::rgb(color).into());
                let tink = if chosen { c(p.list_selected_item_text) } else { ink };
                style::label_on_baseline(window, cx, b.origin.x + px(20.0), base, Align::Left, title.clone(), size_, style::ui_font(FontWeight::NORMAL), tink);
                if unread > 0 {
                    style::pill_on_baseline(window, cx, b.origin.x + px(20.0 + tw), base, &unread.to_string(), size_, style::ui_font(FontWeight::BOLD), accent, white());
                }
                olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Tab", name: Some(title.into()), bounds: b, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
            }
            style::hline_above(window, row.origin.x, row.right(), row.bottom(), c(p.shadow).opacity(0.35));
        }));
        top += TOPICS;
    }

    // The messages, down to the composer (a pane of its own).
    let Tg { store, conv, pics, backend, player, h24, .. } = &mut *tg;
    let conv = conv.as_mut().expect("a conversation");
    let list = r(x0, top, w, f32::from(rect.bottom()) - top);

    // The messages: planned once for each change to the thread (or the
    // width, scale, look or clock), each laid out once for each change to
    // it; a frame then does only what's near the view.
    let col = (w - 2.0 * MARGIN - SCROLL_BAR).max(200.0);
    let scale = window.scale_factor();
    let theme_key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (p.mode as u8, p.document_background, p.keyboard_navigation).hash(&mut h);
        h.finish()
    };
    let layouts = &mut conv.layouts;
    if (col - layouts.width).abs() > 0.5 || scale != layouts.scale || theme_key != layouts.theme {
        layouts.laid.clear();
        layouts.width = col;
        layouts.scale = scale;
        layouts.theme = theme_key;
        conv.plan = None;
    }
    let thread = store.threads.get(&(chat_id, topic));
    let key = PlanKey { version: thread.map_or(0, |t| t.version), unread_from: conv.unread_from, h24: *h24, today: when::day_number(when::now()), revealed: conv.revealed.len(), many: chat.kind.many(), width: col.round() as u32 };
    if conv.plan.as_ref().is_none_or(|p| p.key != key || p.partial) {
        conv.plan = Some(plan(thread, topic, conv.unread_from, &conv.revealed, &mut conv.layouts, &look, col, *h24, key, window));
    }
    let plan = conv.plan.as_ref().expect("a plan");
    let content = plan.content;
    let view_h = f32::from(list.size.height);
    // Where it's scrolled to: kept at the bottom if it was there, else
    // on the same messages when older ones come in above.
    let (last_content, last_offset, last_view, last_oldest) = conv.last;
    let oldest = plan.oldest;
    let now_offset = -f32::from(conv.scroll.offset().y);
    if (now_offset - last_offset).abs() > 0.5 && conv.jump.is_none() {
        // Scrolled by hand (or the wheel's easing) since.
        conv.at_bottom = now_offset >= last_content - last_view - 24.0 || last_content <= last_view;
    }
    let mut offset = now_offset;
    if conv.at_bottom {
        offset = (content - view_h).max(0.0);
    } else if oldest != last_oldest && last_oldest.is_some() && content > last_content {
        offset = now_offset + (content - last_content);
    }
    // (A jump waits for the heights above its message to be real.)
    if let Some(id) = conv.jump.filter(|_| !plan.partial)
        && let Some(it) = plan.items.iter().find(|it| matches!(it.what, What::Msg { id: m, key, .. } if m >= id && key >= 0))
    {
        offset = (it.y - view_h / 3.0).clamp(0.0, (content - view_h).max(0.0));
        conv.jump = None;
        conv.at_bottom = false;
        conv.flash = Some((id, Instant::now()));
    }
    let offset = offset.clamp(0.0, (content - view_h).max(0.0));
    if (offset - now_offset).abs() > 0.5 {
        conv.scroll.set_offset(point(px(0.0), px(-offset)));
    }
    conv.last = (content, offset, view_h, oldest);
    // Shorter than the view: the messages sit at its bottom.
    let lift = (view_h - content).max(0.0);
    let items = &plan.items;
    // Read: up to the newest of theirs wholly in view.
    let in_view = items.partition_point(|it| it.y + lift + it.h <= offset + view_h + 4.0);
    let seen_bottom = items[..in_view].iter().rev().find_map(|it| match it.what {
        What::Msg { id, theirs: true, .. } => Some(id),
        _ => None,
    });
    let flashing = conv.flash.filter(|f| f.1.elapsed() < FLASH);
    // What's in view (and a sliver more), to paint: a scroll draws again.
    let mut draws = Vec::new();
    let first = items.partition_point(|it| it.y + it.h + lift < offset - 40.0);
    for it in items[first..].iter().take_while(|it| it.y + lift <= offset + view_h + 40.0) {
        match &it.what {
            What::Day(t) => draws.push(Draw::Day(t.clone(), it.y)),
            What::Unread => draws.push(Draw::Unread(it.y)),
            &What::Msg { key, id, place, .. } => {
                let found = if key < 0 { thread.and_then(|t| t.pending.iter().find(|m| key_of(m) == key)) } else { thread.and_then(|t| t.msgs.get(&id)) };
                let (Some(msg), Some((_, laid))) = (found, conv.layouts.laid.get(&key)) else { continue };
                let laid = laid.clone();
                let picture = match &msg.media {
                    Some(md) if md.visual() && msg.pending.is_none() => pics.get(&Pic::Thumb(msg.chat, msg.id), backend),
                    _ => None,
                };
                let sender_pic = if laid.avatar && msg.from.as_ref().is_some_and(|f| f.photo.is_some()) { msg.from.as_ref().and_then(|f| pics.get(&Pic::Avatar(f.id), backend)) } else { None };
                let playing = player.as_ref().filter(|pl| pl.0 == msg.chat && pl.1 == msg.id).map(|pl| crate::played(&pl.2));
                draws.push(Draw::Msg {
                    m: Box::new(msg.clone()),
                    place,
                    laid,
                    y: it.y,
                    picture,
                    sender_pic,
                    progress: pics.progress.get(&Pic::File(msg.chat, msg.id)).copied(),
                    read: msg.out && msg.id <= chat.read_outbox,
                    playing,
                    selected: flashing.is_some_and(|f| f.0 == msg.id),
                });
            }
        }
    }
    let loaded = thread.is_some_and(|t| t.loaded);
    let empty = items.is_empty();
    let partial = plan.partial;
    let older = thread.is_some_and(|t| t.more && !t.loading && t.loaded) && offset < 800.0;
    let painted = conv.painted.clone();
    let hover_hit = conv.hover.as_ref().is_some_and(|h| h.1.is_some());
    let at_bottom = conv.at_bottom;
    if partial {
        // The rest laid out next frame.
        window.request_animation_frame();
    }
    // A flash to put out, a voice message's progress to move on.
    if let Some((_, since)) = flashing {
        tg.wake_in(FLASH.saturating_sub(since.elapsed()) + Duration::from_millis(10), cx);
    }
    if tg.player.as_ref().is_some_and(|pl| pl.0 == chat_id) {
        tg.wake_in(Duration::from_millis(100), cx);
    }
    // Older pages, as the top comes near; read up to what's seen.
    if older {
        tg.conv_act(Act::Older, window, cx);
    }
    if let Some(id) = seen_bottom {
        tg.conv_act(Act::Read(id), window, cx);
    }

    let look2 = look.clone();
    let messages = canvas(
        move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
        move |b, hitbox: Hitbox, window, cx| {
            if hover_hit {
                window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
            }
            let mut out = painted.borrow_mut();
            out.clear();
            // It scrolls: on a grid of its own, not level with the chat
            // list's.
            olo_ui::review::own_grid(window, window.content_mask().bounds);
            let origin = point(b.origin.x + px(MARGIN), b.origin.y + px(lift));
            for d in draws {
                match d {
                    Draw::Day(t, y) => {
                        let font = style::ui_font(FontWeight::BOLD);
                        let tw = f32::from(style::text_width(window, t.clone(), style::SMALL_SIZE, font.clone())) + 20.0;
                        let pill = style::snap(window, Bounds { origin: point(origin.x + px(col / 2.0 - tw / 2.0), origin.y + px(y + DAY / 2.0 - 10.0)), size: size(px(tw), px(20.0)) });
                        window.paint_quad(olo_ui::gpui::fill(pill, look2.pill).corner_radii(px(10.0)));
                        style::label(window, cx, pill.center().x, pill.center().y, Align::Center, t.clone(), style::SMALL_SIZE, font, look2.pill_text);
                        crate::seen(window, cx, olo_ui::agent::Node::label(t, pill));
                    }
                    Draw::Unread(y) => {
                        let band = style::snap(window, Bounds { origin: point(b.origin.x, origin.y + px(y + 3.0)), size: size(b.size.width, px(DAY - 6.0)) });
                        style::fill_rect(window, band, look2.theirs.opacity(0.85));
                        style::label(window, cx, origin.x + px(col / 2.0), band.center().y, Align::Center, "Unread Messages", style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), look2.link);
                    }
                    Draw::Msg { m, place, laid, y, picture, sender_pic, progress, read, playing, selected } => {
                        let o = point(origin.x, origin.y + px(y));
                        let pp = bubble::Paint { look: &look2, place, picture, sender_pic, progress, read, playing, selected };
                        bubble::paint(window, cx, o, &m, &laid, &pp);
                        // The time is a stamp in the balloon's corner, set
                        // on the text's baseline; a sender's picture is art.
                        let (g, mp) = &laid.meta;
                        olo_ui::review::own_grid(window, Bounds { origin: point(o.x + px(mp.0 - 1.0), o.y + px(mp.1 - 1.0)), size: size(px(g.w + 2.0), px(g.h + 2.0)) });
                        if laid.avatar {
                            olo_ui::review::own_grid(window, Bounds { origin: point(o.x, o.y + px(laid.bubble.bottom() - bubble::AVATAR)), size: size(px(bubble::AVATAR), px(bubble::AVATAR)) });
                        }
                        let whole = Bounds::from_corners(point(o.x + px(laid.bubble.x), o.y), point(o.x + px(laid.bubble.right()), o.y + px(laid.height)));
                        // For agents: each message as a list item.
                        let who = if m.out { "You".to_owned() } else { m.from.as_ref().map_or_else(String::new, |f| f.name.clone()) };
                        let said = if who.is_empty() { m.summary() } else { format!("{who}: {}", m.summary()) };
                        crate::seen(window, cx, olo_ui::agent::Node { role: "ListItem", name: Some(said.into()), bounds: whole, enabled: true, clickable: true, ..Default::default() });
                        out.push(Painted { whole, origin: o, m: *m, laid });
                    }
                }
            }
        },
    )
    .w_full()
    .h(px(content.max(view_h)));
    let (t_move, t_click, t_menu, t_out) = (me.clone(), me.clone(), me.clone(), me.clone());
    let scroller = div()
        .id("conv-messages")
        .w_full()
        .h(px(content.max(view_h)))
        .child(messages)
        // (What's under the pointer redraws this pane, not the window.)
        .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
            // (Only the pointer's shape shows it: drawn again when that
            // changes, over a link or off it.)
            let changed = t_move.update(cx, |tg, _| {
                let pane = tg.pane(crate::Kind::Conv);
                let c = tg.conv.as_mut()?;
                let now = c.hit_at(e.position);
                let pointing = |h: &Option<(MsgId, Option<Hit>)>| h.as_ref().is_some_and(|h| h.1.is_some());
                let redraw = pointing(&now) != pointing(&c.hover);
                c.hover = now;
                redraw.then_some(pane)
            });
            if let Some(pane) = changed.flatten() {
                cx.notify(pane);
            }
        })
        .on_hover(move |on: &bool, _, cx| {
            if !*on {
                let changed = t_out.update(cx, |tg, _| tg.conv.as_mut().and_then(|c| c.hover.take()).filter(|h| h.1.is_some()).and(tg.pane(crate::Kind::Conv)));
                if let Some(pane) = changed {
                    cx.notify(pane);
                }
            }
        })
        .on_click(move |e: &ClickEvent, window, cx| {
            let ClickEvent::Mouse(mc) = e else { return };
            t_click.update(cx, |tg, cx| {
                let Some((id, h)) = tg.conv.as_ref().and_then(|c| c.hit_at(mc.up.position)) else { return };
                if mc.down.click_count >= 2 {
                    tg.conv_act(Act::Message(id, "reply".into()), window, cx);
                } else if let Some(h) = h {
                    tg.conv_act(Act::Hit(id, h), window, cx);
                }
            })
        })
        .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, cx| {
            let menu = t_menu.update(cx, |tg, _| {
                let c = tg.conv.as_ref()?;
                let (id, _) = c.hit_at(e.position)?;
                let msg = tg.store.threads.get(&(c.chat, c.topic)).and_then(|t| t.msgs.get(&id))?;
                let chat = tg.store.chats.get(&c.chat).cloned().unwrap_or_default();
                Some((id, message_menu(msg, &chat)))
            });
            if let Some((id, menu)) = menu {
                let to = t_menu.clone();
                olo_ui::menu::context_menu(window, cx, menu, e.position, move |cmd, window, cx| to.update(cx, |tg, cx| tg.conv_act(Act::Message(id, cmd.to_owned()), window, cx)));
            }
        });
    let conv = tg.conv.as_ref().expect("a conversation");
    let list_el = div()
        .absolute()
        .left(list.origin.x)
        .top(list.origin.y)
        .w(list.size.width)
        .h(list.size.height)
        .child(ScrollView::new("conv-scroll", &conv.scroll).border(false).background(rgba(look.ground)).auto_hide(true).child(scroller));
    let mut overlay: Vec<AnyElement> = Vec::new();
    if !loaded || empty {
        let (text, dim) = (if !loaded { "Loading messages…" } else { "No messages here yet." }, look.dim);
        overlay.push(
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let at = style::label(window, cx, b.center().x, b.center().y, Align::Center, text, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(text, at));
                },
            )
            .absolute()
            .left(list.origin.x)
            .top(list.origin.y)
            .w(list.size.width)
            .h(list.size.height)
            .into_any_element(),
        );
    }
    // Down to the newest.
    if !at_bottom {
        let b = r(f32::from(list.right()) - 34.0 - 20.0, f32::from(list.bottom()) - 30.0 - 20.0, 40.0, 40.0);
        let (look, hot_b, unread) = (look.clone(), is_hot("to-bottom"), chat.unread);
        overlay.push(
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let cc = b.center();
                    disc(window, point(cc.x, cc.y + px(1.0)), 20.0, hsla(0.0, 0.0, 0.0, 30.0 / 255.0));
                    disc(window, cc, 20.0, if hot_b { bubble::blend(look.theirs, look.text, 0.06) } else { look.theirs });
                    let hair = style::hairline(window);
                    window.paint_sector(cc, px(20.0) - hair, px(20.0), 0.0, std::f32::consts::TAU, look.theirs_edge);
                    window.paint_segment(point(cc.x - px(6.0), cc.y - px(3.0)), point(cc.x, cc.y + px(3.0)), px(1.8), true, look.dim);
                    window.paint_segment(point(cc.x, cc.y + px(3.0)), point(cc.x + px(6.0), cc.y - px(3.0)), px(1.8), true, look.dim);
                    if unread > 0 {
                        style::pill(window, cx, cc.x - px(10.0), b.origin.y - px(2.0), &unread.to_string(), 9.5, style::ui_font(FontWeight::BOLD), look.accent, white());
                    }
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::button("Scroll to Newest", b));
                },
            )
            .absolute()
            .left(b.origin.x)
            .top(b.origin.y)
            .w(b.size.width)
            .h(b.size.height)
            .into_any_element(),
        );
        let t = me.clone();
        overlay.push(crate::hit(&me, "to-bottom", b).occlude().on_click(move |_, window, cx| t.update(cx, |tg, cx| tg.conv_act(Act::Bottom, window, cx))).into_any_element());
    }

    div()
        .absolute()
        .inset_0()
        .child(canvas(|_, _, _| {}, move |_, _, window, cx| paint.into_iter().for_each(|f| f(window, cx))).absolute().inset_0())
        .child(list_el)
        .children(hits)
        .children(overlay)
        .into_any_element()
}

/// The conversation's entries in order with day headings and the unread
/// line, laid out (newest first, within [`LAY_BUDGET`]; the rest
/// guessed) and placed.
#[allow(clippy::too_many_arguments)]
fn plan(thread: Option<&crate::store::Thread>, topic: Option<MsgId>, unread_from: Option<MsgId>, revealed: &HashSet<MsgId>, layouts: &mut Layouts, look: &Look, col: f32, h24: bool, key: PlanKey, window: &mut Window) -> Plan {
    let mut entries: Vec<(What, Option<&Message>)> = Vec::new();
    if let Some(t) = thread {
        let all: Vec<&Message> = t.all().filter(|m| topic.is_none() || m.topic == topic || m.id == topic.unwrap_or(0)).collect();
        let mut last_day = None;
        let same = |a: &Message, b: &Message| a.service.is_none() && b.service.is_none() && a.out == b.out && a.from.as_ref().map(|f| f.id) == b.from.as_ref().map(|f| f.id) && (b.date - a.date).abs() < 300 && when::day_number(a.date) == when::day_number(b.date);
        for (i, m) in all.iter().enumerate() {
            let day = when::day_number(m.date);
            if last_day != Some(day) {
                entries.push((What::Day(when::day(m.date, when::now())), None));
                last_day = Some(day);
            }
            if unread_from == Some(m.id) && m.pending.is_none() {
                entries.push((What::Unread, None));
            }
            let first = i == 0 || !same(all[i - 1], m) || unread_from == Some(m.id);
            let last = i + 1 == all.len() || !same(m, all[i + 1]) || unread_from == Some(all[i + 1].id);
            let place = Place { first, last, many: key.many };
            entries.push((What::Msg { key: key_of(m), id: m.id, place, theirs: !m.out && m.pending.is_none() }, Some(m)));
        }
    }
    // Heights: the newest first, as the view starts there.
    let until = Instant::now() + LAY_BUDGET;
    let mut partial = false;
    let mut heights = vec![DAY; entries.len()];
    for (i, (what, m)) in entries.iter().enumerate().rev() {
        let (What::Msg { key, place, .. }, Some(m)) = (what, m) else { continue };
        let time = when::time(m.date, h24);
        let shown = revealed.contains(&m.id);
        let d = digest(m, *place, shown, &time);
        heights[i] = match layouts.laid.get(key) {
            Some((k, l)) if *k == d => l.height,
            _ if Instant::now() < until => {
                let name = m.from.as_ref().map(|f| f.name.clone());
                let laid = bubble::lay(window, m, *place, look, col, &time, name.as_deref(), shown);
                let h = laid.height;
                layouts.laid.insert(*key, (d, Rc::new(laid)));
                h
            }
            _ => {
                partial = true;
                guess(m, col)
            }
        };
    }
    // Placed top to bottom.
    let mut items = Vec::with_capacity(entries.len());
    let mut y = MARGIN;
    for (i, ((what, _), h)) in entries.iter().zip(&heights).enumerate() {
        let gap = match (what, entries.get(i + 1)) {
            (What::Msg { place, .. }, Some((What::Msg { .. }, _))) if !place.last => GAP,
            _ => RUN_GAP,
        };
        items.push(Planned { what: match what {
            What::Day(t) => What::Day(t.clone()),
            What::Unread => What::Unread,
            &What::Msg { key, id, place, theirs } => What::Msg { key, id, place, theirs },
        }, y, h: *h });
        y += h + gap;
    }
    // Layouts of messages gone (deleted, sent under another id) go too.
    if !partial && layouts.laid.len() > items.len() + 64 {
        let keep: HashSet<i64> = items.iter().filter_map(|it| match it.what { What::Msg { key, .. } => Some(key), _ => None }).collect();
        layouts.laid.retain(|k, _| keep.contains(k));
    }
    Plan { key, items, content: y + MARGIN - RUN_GAP, oldest: thread.and_then(|t| t.oldest()), partial }
}

fn rgba(h: Hsla) -> olo_ui::palette::Rgba {
    let r = h.to_rgb();
    [(r.r * 255.0).round() as u8, (r.g * 255.0).round() as u8, (r.b * 255.0).round() as u8, (r.a * 255.0).round() as u8]
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

fn message_menu(m: &Message, chat: &olo_tg::Chat) -> Menu {
    let mine = m.out;
    let has_text = !m.text.is_empty();
    let file = matches!(m.media, Some(Media::Photo { .. } | Media::Video { .. } | Media::File { .. } | Media::Audio { .. } | Media::Voice { .. }));
    let mut react = Menu::new("React");
    for e in ["👍", "❤️", "😂", "😮", "😢", "🙏", "🔥", "🎉", "👏", "🤔"] {
        let mine_too = m.reactions.iter().any(|r| r.mine && r.emoji == e);
        react = react.item(e, format!("react:{e}")).checked(mine_too);
    }
    let mut menu = Menu::new("").item("Reply", "reply").enabled(m.pending.is_none() && m.service.is_none());
    if mine && has_text {
        menu = menu.item("Edit", "edit").enabled(m.pending.is_none());
    }
    menu = menu
        .item("Copy Text", "copy")
        .enabled(has_text)
        .submenu(react)
        .separator()
        .item("Forward…", "forward")
        .enabled(m.pending.is_none() && m.service.is_none())
        .item(if m.pinned { "Unpin" } else { "Pin" }, "pin")
        .enabled(m.pending.is_none() && chat.kind != ChatKind::Channel || mine);
    if file {
        menu = menu.separator().item("Open", "open").item("Save As…", "save");
    }
    menu.separator().item("Delete…", "delete").enabled(m.pending.is_none())
}

/// The composer's measure for a conversation `w` wide: the field's
/// height (its lines, up to eight), the whole composer's, and what's
/// being answered or edited.
pub fn composer_size(tg: &Tg, w: f32, window: &mut Window, cx: &App) -> (f32, f32, Option<Message>) {
    let Some(conv) = tg.conv.as_ref() else { return (0.0, 0.0, None) };
    let field_w = (w - 2.0 * m::SPACING - 36.0 - 44.0 - 28.0).max(80.0);
    let lines = conv.field.read(cx).rows_at(window, px(field_w)).min(8);
    let lh = f32::from(TextField::line_height_for(window, bubble::TEXT));
    let field_h = (lines as f32 * lh + 16.0).max(36.0);
    let quoted = conv.edit.or(conv.reply).and_then(|id| tg.store.threads.get(&(conv.chat, conv.topic)).and_then(|t| t.msgs.get(&id)).cloned());
    let composer_h = field_h + if quoted.is_some() { 42.0 } else { 0.0 } + if conv.files.is_empty() { 0.0 } else { 32.0 } + 2.0 * m::SPACING;
    (field_h, composer_h, quoted)
}

/// The field to write in, at `rect`: what's being answered or edited,
/// the files going with it, the paperclip, the field, send.
pub fn composer(tg: &mut Tg, rect: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Tg>) -> AnyElement {
    let me = cx.entity();
    let me = &me;
    let p = theme::palette(cx);
    let look = Look::of(&p);
    let look = &look;
    let chat = tg.conv.as_ref().and_then(|c| tg.store.chats.get(&c.chat)).cloned().unwrap_or_default();
    let (field_h, _, quoted) = composer_size(tg, f32::from(rect.size.width), window, cx);
    let hot = tg.hot.clone();
    let is_hot = move |k: &str| hot.as_ref().is_some_and(|h| h.as_ref() == k);
    let mut paint: Paints = Vec::new();
    let mut hits: Vec<AnyElement> = Vec::new();
    let mut overlay: Vec<AnyElement> = Vec::new();
    let Some(conv) = tg.conv.as_mut() else { return div().into_any_element() };
    let (x0, y0, w, h) = (f32::from(rect.origin.x), f32::from(rect.origin.y), f32::from(rect.size.width), f32::from(rect.size.height));
    let mut y = y0 + m::SPACING;
    let editing = conv.edit.is_some();
    let quote = quoted.map(|q| {
        let bar = r(x0 + m::SPACING, y, w - 2.0 * m::SPACING, 34.0);
        let x = r(f32::from(bar.right()) - 23.0, y + 6.0, 22.0, 22.0);
        y += 42.0;
        (bar, x, q)
    });
    let mut files = Vec::new();
    if !conv.files.is_empty() {
        let mut x = x0 + m::SPACING + 36.0;
        let font = style::ui_font(FontWeight::NORMAL);
        for f in &conv.files {
            let name = f.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            // Cut short to 200 pt once, here; the chip fits what's left.
            let name = style::fit(window, name.into(), style::DETAIL_SIZE, &font, px(200.0)).to_string();
            let cw = f32::from(style::text_width(window, name.clone(), style::DETAIL_SIZE, font.clone())).ceil() + 34.0;
            let chip = r(x, y, cw, 24.0);
            x += cw + 6.0;
            files.push((chip, r(f32::from(chip.right()) - 21.0, y + 3.0, 18.0, 18.0), name));
        }
        y += 32.0;
    }
    let attach = r(x0 + m::SPACING, y0 + h - m::SPACING - 36.0, 32.0, 36.0);
    let send = r(x0 + w - m::SPACING - 36.0, y0 + h - m::SPACING - 36.0, 36.0, 36.0);
    let well = Bounds::from_corners(point(attach.right() + px(4.0), px(y)), point(send.origin.x - px(8.0), px(y + field_h)));
    let inner = Bounds::from_corners(point(well.origin.x + px(14.0), well.origin.y + px(8.0)), point(well.right() - px(14.0), well.bottom() - px(8.0)));
    let can_send = !conv.text(cx).trim().is_empty() || !conv.files.is_empty();
    let hint = match chat.kind {
        ChatKind::Channel => "Broadcast",
        _ if editing => "Edit message",
        _ => "Message",
    };
    conv.field.update(cx, |f, cx| f.set_hint(hint, cx));
    {
        let look = look.clone();
        let (attach_hot, send_hot, x_hot) = (is_hot("attach"), is_hot("send"), is_hot("cancel-reply"));
        let file_hots: Vec<bool> = (0..files.len()).map(|i| is_hot(&format!("unstage:{i}"))).collect();
        let (quote, files) = (quote.clone(), files.clone());
        paint.push(Box::new(move |window, cx| {
            let p = theme::palette(cx);
            style::fill_rect(window, rect, c(p.panel_background));
            style::hline(window, rect.origin.x, rect.right(), rect.origin.y, c(p.shadow).opacity(0.5));
            if let Some((bar, x, q)) = quote {
                let accent = look.accent;
                window.paint_quad(olo_ui::gpui::fill(style::snap(window, Bounds { origin: point(bar.origin.x + px(28.0), bar.origin.y + px(2.0)), size: size(px(2.5), px(30.0)) }), accent).corner_radii(px(1.0)));
                // A curved arrow (reply) or a pencil (edit).
                let ic = point(bar.origin.x + px(12.0), bar.center().y);
                let at = |dx: f32, dy: f32| point(ic.x + px(dx), ic.y + px(dy));
                if editing {
                    window.paint_segment(at(-5.0, 5.0), at(4.0, -4.0), px(3.0), false, accent);
                    window.paint_segment(at(-6.0, 6.0), at(-4.5, 4.5), px(1.6), false, accent);
                } else {
                    window.paint_segment(at(-6.0, 0.0), at(-1.0, -5.0), px(1.6), true, accent);
                    window.paint_segment(at(-6.0, 0.0), at(-1.0, 5.0), px(1.6), true, accent);
                    window.paint_segment(at(-6.0, 0.0), at(6.0, 0.0), px(1.6), true, accent);
                }
                let head = if editing { "Edit Message".to_owned() } else { format!("Reply to {}", q.from.as_ref().map_or("message", |f| f.name.as_str())) };
                style::label(window, cx, bar.origin.x + px(38.0), bar.origin.y + px(10.0), Align::Left, head, style::SMALL_SIZE + 0.5, style::ui_font(FontWeight::BOLD), accent);
                style::label_fit(window, cx, bar.origin.x + px(38.0), bar.origin.y + px(25.0), Align::Left, q.summary().replace('\n', " "), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), c(p.panel_text), bar.size.width - px(80.0));
                close_mark(window, x.center(), if x_hot { c(p.panel_text) } else { style::disabled_text(&p) });
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::button("Cancel Reply", x));
            }
            for ((chip, xr, name), hot) in files.into_iter().zip(file_hots) {
                let chip = style::snap(window, chip);
                window.paint_quad(olo_ui::gpui::fill(chip, look.accent.opacity(0.15)).corner_radii(px(12.0)));
                style::label(window, cx, chip.origin.x + px(10.0), chip.center().y, Align::Left, name.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), c(p.panel_text));
                close_mark(window, xr.center(), if hot { c(p.panel_text) } else { style::disabled_text(&p) });
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::button(format!("Remove {name}"), xr));
            }
            paperclip(window, attach.center(), if attach_hot { look.accent } else { style::disabled_text(&p) });
            olo_ui::agent::record(window, cx, olo_ui::agent::Node::button("Attach Files", attach));
            let well = style::snap(window, well);
            window.paint_quad(olo_ui::gpui::quad(well, px(18.0), c(p.document_background), style::hairline(window), c(olo_ui::palette::mix(p.panel_background, p.panel_text, 0.22)), olo_ui::gpui::BorderStyle::Solid));
            let face = if can_send { look.accent } else { look.accent.opacity(0.35) };
            let face = if send_hot && can_send { bubble::blend(face, black(), 0.1) } else { face };
            disc(window, send.center(), 17.0, face);
            plane(window, send.center(), white());
            olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Button", name: Some(if editing { "Save" } else { "Send" }.into()), bounds: send, enabled: can_send, clickable: can_send, ..Default::default() });
        }));
    }
    if let Some((_, x, _)) = &quote {
        let t = me.clone();
        hits.push(crate::hit(me, "cancel-reply", *x).on_click(move |_, window, cx| t.update(cx, |tg, cx| tg.conv_act(Act::CancelReply, window, cx))).into_any_element());
    }
    for (i, (_, xr, _)) in files.iter().enumerate() {
        let t = me.clone();
        hits.push(crate::hit(me, &format!("unstage:{i}"), *xr).on_click(move |_, window, cx| t.update(cx, |tg, cx| tg.conv_act(Act::Unstage(i), window, cx))).into_any_element());
    }
    let t = me.clone();
    hits.push(crate::hit(me, "attach", attach).on_click(move |_, window, cx| t.update(cx, |tg, cx| tg.conv_act(Act::Attach, window, cx))).into_any_element());
    let t = me.clone();
    hits.push(
        crate::hit(me, "send", send)
            .on_click(move |_, window, cx| {
                t.update(cx, |tg, cx| {
                    if tg.conv.as_ref().is_some_and(|c| !c.text(cx).trim().is_empty() || !c.files.is_empty()) {
                        tg.conv_act(Act::Send, window, cx);
                    }
                })
            })
            .into_any_element(),
    );
    // The field itself; Escape puts a reply or edit away.
    let field = conv.field.clone();
    if std::mem::take(&mut conv.focus_field) {
        field.read(cx).focus(window);
    }
    let t = me.clone();
    overlay.push(
        div()
            .absolute()
            .left(inner.origin.x)
            .top(inner.origin.y)
            .w(inner.size.width)
            .h(inner.size.height)
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    t.update(cx, |tg, cx| {
                        if tg.conv.as_ref().is_some_and(|c| c.reply.is_some() || c.edit.is_some()) {
                            tg.conv_act(Act::CancelReply, window, cx);
                            cx.stop_propagation();
                        }
                    })
                }
            })
            .child(field)
            .into_any_element(),
    );
    div()
        .absolute()
        .inset_0()
        .child(canvas(|_, _, _| {}, move |_, _, window, cx| paint.into_iter().for_each(|f| f(window, cx))).absolute().inset_0())
        .children(hits)
        .children(overlay)
        .into_any_element()
}

fn close_mark(window: &mut Window, cc: Point<Pixels>, ink: Hsla) {
    let at = |dx: f32, dy: f32| point(cc.x + px(dx), cc.y + px(dy));
    window.paint_segment(at(-4.0, -4.0), at(4.0, 4.0), px(1.5), false, ink);
    window.paint_segment(at(-4.0, 4.0), at(4.0, -4.0), px(1.5), false, ink);
}

/// The attach button's paperclip.
fn paperclip(window: &mut Window, cc: Point<Pixels>, ink: Hsla) {
    use std::f32::consts::PI;
    let (cx_, cy_) = (f32::from(cc.x), f32::from(cc.y));
    let arc = |ax: f32, ay: f32, r: f32, from: f32, to: f32| -> Vec<(f32, f32)> {
        (0..=12)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 12.0;
                (cx_ + ax + r * a.cos(), cy_ + ay + r * a.sin())
            })
            .collect()
    };
    let mut pts = vec![(cx_ + 3.0, cy_ + 4.0), (cx_ + 3.0, cy_ - 6.0)];
    pts.extend(arc(0.0, -6.0, 3.0, 0.0, -PI));
    pts.push((cx_ - 3.0, cy_ + 7.0));
    pts.extend(arc(1.0, 7.0, 4.0, PI, 0.0).into_iter().skip(1));
    pts.push((cx_ + 5.0, cy_ - 8.0));
    // Leant like a paperclip on a desk.
    let (sn, cs) = (-0.6f32).sin_cos();
    let rot = |(x, y): (f32, f32)| {
        let (dx, dy) = (x - cx_, y - cy_);
        point(px(cx_ + dx * cs - dy * sn), px(cy_ + dx * sn + dy * cs))
    };
    let mut path = PathBuilder::stroke(px(1.6));
    path.move_to(rot(pts[0]));
    for &q in &pts[1..] {
        path.line_to(rot(q));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
}

/// The send button's paper plane.
fn plane(window: &mut Window, cc: Point<Pixels>, ink: Hsla) {
    let at = |dx: f32, dy: f32| point(cc.x + px(dx), cc.y + px(dy));
    let mut path = PathBuilder::fill();
    path.move_to(at(8.0, 0.0));
    path.line_to(at(-7.0, -7.0));
    path.line_to(at(-4.0, 0.0));
    path.line_to(at(-7.0, 7.0));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
}
