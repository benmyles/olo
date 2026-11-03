//! The chat list down the window's left: a search field, the account's
//! folders as chips, and the chats (pinned first) with their pictures,
//! latest messages, times and unread counts; the archive behind a row of
//! its own. While searching: the chats whose names match, then
//! Telegram's people, chats and messages. The rows are painted in one
//! canvas, only those in view.

use std::sync::Arc;

use olo_tg::{Chat, ChatId, ChatKind, Message, MsgId, Pic};
use olo_ui::{
    Menu,
    gpui::{
        AnyElement, App, Bounds, ClickEvent, Context, Corner, CursorStyle, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
        ParentElement as _, PathBuilder, Pixels, Point, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _, TextRun, Window, canvas, div, point, px, rgb, size, transparent_black,
        white,
    },
    metrics as m,
    palette::{self as theme_palette, Palette},
    style::{self, Align, c},
    theme,
    widgets::ScrollView,
};

use crate::{Tg, bubble, store::Store, when};

pub const ROW: f32 = 60.0;
const PICTURE: f32 = 40.0;
pub const WIDTH: f32 = 300.0;
pub const MIN_WIDTH: f32 = 220.0;
/// The folders' row.
const FOLDERS: f32 = 28.0;

/// Which chats the list shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// A folder (its id), or all chats.
    pub folder: Option<i32>,
    pub archive: bool,
    /// Unknown senders' chats (`Chat::unknown`).
    pub unknown: bool,
    pub query: String,
}

pub enum Act {
    Open(ChatId),
    OpenAt(ChatId, MsgId),
    Command(ChatId, String),
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
                // The archive has its own, unknown or not; unknown
                // senders theirs; all chats the rest.
                None if view.archive => c.archived,
                None => !c.archived && c.unknown == view.unknown,
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
    if c.unknown {
        s.push_str(", unknown sender");
    } else if c.unread > 0 {
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

/// What row i is.
#[derive(Clone, Copy, PartialEq)]
pub enum Row {
    Archive,
    Unknown,
    Chat(ChatId),
    /// A chat Telegram's search found (its index there).
    Found(usize),
    /// A message it found.
    Msg(usize),
    Heading(&'static str),
}

/// The list's rows as the view and a search have them.
pub fn rows(store: &Store, view: &View, found: &Found) -> Vec<Row> {
    let searching = !view.query.trim().is_empty();
    let ids = shown(store, view);
    let mut rows = Vec::with_capacity(ids.len() + 4);
    // The archive's row and unknown senders', at the top of all chats.
    let top = !searching && !view.archive && !view.unknown && view.folder.is_none();
    if top && store.chats.values().any(|c| c.archived) {
        rows.push(Row::Archive);
    }
    if top && store.chats.values().any(|c| c.unknown && !c.archived) {
        rows.push(Row::Unknown);
    }
    rows.extend(ids.iter().map(|&id| Row::Chat(id)));
    if searching {
        rows.push(Row::Heading(if found.query == view.query.trim() { "Global search" } else { "Searching…" }));
        rows.extend(found.chats.iter().enumerate().filter(|(_, c)| !ids.contains(&c.id)).map(|(i, _)| Row::Found(i)));
        rows.push(Row::Heading("Messages"));
        rows.extend((0..found.msgs.len()).map(Row::Msg));
    }
    rows
}

/// What a row paints, gathered for the rows near the view.
enum Draw {
    Heading(&'static str),
    /// The archive's row or unknown senders': its title, icon, a few of
    /// its chats' names and the unread among them (unknown senders: none
    /// counted).
    Pile {
        title: &'static str,
        icon: &'static str,
        names: String,
        unread: u32,
    },
    Chat {
        c: Box<Chat>,
        pic: Option<Arc<RenderImage>>,
        selected: bool,
        typing: Option<String>,
    },
    Msg {
        title: String,
        chat: ChatId,
        saved: bool,
        pic: Option<Arc<RenderImage>>,
        summary: String,
        date: i64,
    },
}

/// The list, filling `rect` (window coordinates).
pub fn render(tg: &mut Tg, rect: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Tg>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    let (x0, y0, w, h) = (f32::from(rect.origin.x), f32::from(rect.origin.y), f32::from(rect.size.width), f32::from(rect.size.height));
    let r = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    let mut paint: crate::conv::Paints = Vec::new();
    let mut hits: Vec<AnyElement> = Vec::new();
    let hot = tg.hot.clone();
    let is_hot = move |k: &str| hot.as_ref().is_some_and(|h| h.as_ref() == k);

    // The search field.
    let bar = r(x0, y0, w, m::TOOLBAR_HEIGHT);
    paint.push(Box::new(move |window, cx| {
        let p = theme::palette(cx);
        style::fill_rect(window, rect, c(p.panel_background));
        let _ = bar;
    }));
    let search = div().absolute().left(px(x0 + m::SPACING)).top(px(y0 + m::TIGHT)).w(px(w - 2.0 * m::SPACING)).h(px(m::CONTROL_HEIGHT)).child(tg.search.clone()).into_any_element();
    let mut top = y0 + m::TOOLBAR_HEIGHT;

    // Folders, as chips (with the archive's back row when in it).
    let searching = !tg.view.query.trim().is_empty();
    if !searching && (tg.view.archive || tg.view.unknown || !tg.store.folders.is_empty()) {
        let row = r(x0, top, w, FOLDERS);
        if tg.view.archive || tg.view.unknown {
            let title = if tg.view.archive { "Archived Chats" } else { "Unknown Senders" };
            let t = me.clone();
            hits.push(
                crate::hit(&me, "archive-back", row)
                    .on_click(move |_, _, cx| {
                        t.update(cx, |tg, cx| {
                            tg.view.archive = false;
                            tg.view.unknown = false;
                            cx.notify();
                        })
                    })
                    .into_any_element(),
            );
            paint.push(Box::new(move |window, cx| {
                let p = theme::palette(cx);
                let ink = c(p.panel_text);
                back_arrow(window, point(row.origin.x + px(m::SPACING + 4.0), row.center().y), ink);
                style::label(window, cx, row.origin.x + px(m::SPACING + 16.0), row.center().y, Align::Left, title, style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink);
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::button("Back to Chats", row));
            }));
        } else {
            let chips: Vec<(Option<i32>, String, u32)> = std::iter::once((None, "All Chats".to_owned(), 0))
                .chain(tg.store.folders.iter().map(|f| {
                    let unread = tg.store.chats.values().filter(|c| f.admits(c) && !c.muted && !c.unknown).filter(|c| c.unread > 0 || c.marked).count() as u32;
                    (Some(f.id), f.title.clone(), unread)
                }))
                .collect();
            let size_ = style::SMALL_SIZE + 0.5;
            let font = style::ui_font(FontWeight::NORMAL);
            let count_font = style::ui_font(FontWeight::NORMAL);
            let width_of = |window: &mut Window, title: &str, unread: u32| {
                let badge = if unread > 0 { style::pill_width(window, &unread.to_string(), 9.0, &count_font) + 4.0 } else { 0.0 };
                f32::from(style::text_width(window, title.to_owned(), size_, font.clone())) + badge + 16.0
            };
            // As many as fit; the rest behind a last chip with a menu.
            let mut x = x0 + m::SPACING;
            let room = x0 + w - m::SPACING - x;
            let widths: Vec<f32> = chips.iter().map(|c| width_of(window, &c.1, c.2) + 4.0).collect();
            let mut fits = chips.len();
            if widths.iter().sum::<f32>() > room {
                let mut used = 0.0;
                fits = 0;
                for cw in &widths {
                    if used + cw > room - 70.0 {
                        break;
                    }
                    used += cw;
                    fits += 1;
                }
            }
            let hidden: Vec<(Option<i32>, String, u32)> = chips[fits..].to_vec();
            let current = tg.view.folder;
            let mut drawn = Vec::new();
            for (i, (id, title, unread)) in chips[..fits].iter().enumerate() {
                let chip = r(x, top + 3.0, widths[i] - 4.0, 22.0);
                x += widths[i];
                let key = format!("folder:{}", id.unwrap_or(0));
                let (t, id) = (me.clone(), *id);
                hits.push(
                    crate::hit(&me, &key, chip)
                        .on_click(move |_, _, cx| {
                            t.update(cx, |tg, cx| {
                                tg.view.folder = id;
                                cx.notify();
                            })
                        })
                        .into_any_element(),
                );
                drawn.push((chip, title.clone(), *unread, current == id, is_hot(&key), false));
            }
            if !hidden.is_empty() {
                // The chosen one's name when it's among them.
                let label = hidden.iter().find(|h| h.0 == current).map_or_else(|| "More".to_owned(), |h| h.1.clone());
                let unread: u32 = hidden.iter().map(|h| h.2).sum();
                let cw = width_of(window, &label, unread) + 12.0;
                let chip = r(x, top + 3.0, cw, 22.0);
                let t = me.clone();
                let hidden2 = hidden.clone();
                hits.push(
                    crate::hit(&me, "folder:more", chip)
                        .on_click(move |_, window, cx| {
                            let current = t.read(cx).view.folder;
                            let mut menu = Menu::new("");
                            for (id, title, unread) in &hidden2 {
                                let label = if *unread > 0 { format!("{title} ({unread})") } else { title.clone() };
                                menu = menu.item(label, format!("folder:{}", id.unwrap_or(0))).checked(current == *id);
                            }
                            let to = t.clone();
                            olo_ui::menu::popup(window, cx, menu, chip, Corner::BottomLeft, move |cmd, _, cx| {
                                to.update(cx, |tg, cx| {
                                    tg.view.folder = cmd.trim_start_matches("folder:").parse().ok().filter(|&id: &i32| id != 0);
                                    cx.notify();
                                })
                            });
                        })
                        .into_any_element(),
                );
                drawn.push((chip, label, unread, hidden.iter().any(|h| h.0 == current), is_hot("folder:more"), true));
            }
            paint.push(Box::new(move |window, cx| {
                let p = theme::palette(cx);
                // A strip of chips, set by their own insets (not the
                // menu titles' above).
                olo_ui::review::own_grid(window, row);
                for (chip, title, unread, chosen, hot, more) in drawn {
                    let (fill, ink) = if chosen {
                        (c(p.list_selected_background), c(p.list_selected_item_text))
                    } else if hot {
                        (c(p.panel_text).opacity(0.08), c(p.panel_text))
                    } else {
                        (transparent_black(), c(p.panel_text))
                    };
                    let chip = style::snap(window, chip);
                    window.paint_quad(olo_ui::gpui::fill(chip, fill).corner_radii(px(11.0)));
                    // The count on the name's baseline.
                    let base = style::baseline_at(window, chip.center().y, size_);
                    let t = style::label_on_baseline(window, cx, chip.origin.x + px(8.0), base, Align::Left, title.clone(), size_, style::ui_font(FontWeight::NORMAL), ink);
                    if unread > 0 {
                        style::pill_on_baseline(window, cx, t.right() + px(4.0), base, &unread.to_string(), 9.0, style::ui_font(FontWeight::NORMAL), c(p.keyboard_navigation), white());
                    }
                    if more {
                        let cc = point(chip.right() - px(10.0), chip.center().y);
                        window.paint_segment(point(cc.x - px(3.0), cc.y - px(1.5)), point(cc.x, cc.y + px(1.5)), px(1.3), true, ink);
                        window.paint_segment(point(cc.x, cc.y + px(1.5)), point(cc.x + px(3.0), cc.y - px(1.5)), px(1.3), true, ink);
                    }
                    olo_ui::agent::record(
                        window,
                        cx,
                        olo_ui::agent::Node { role: "Tab", name: Some(title.into()), bounds: chip, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() },
                    );
                }
            }));
        }
        top += FOLDERS;
    }
    {
        let line_y = px(top);
        paint.push(Box::new(move |window, cx| {
            let p = theme::palette(cx);
            style::hline(window, rect.origin.x, rect.right(), line_y, c(p.shadow).opacity(0.5));
        }));
    }
    let hair = f32::from(style::hairline(window));
    let body = r(x0, top + hair, w, (y0 + h - top - hair).max(0.0));

    // The rows: those in view gathered to paint.
    let rows = rows(&tg.store, &tg.view, &tg.found);
    tg.list_rows = rows.clone();
    let total = rows.len();
    let view_h = f32::from(body.size.height);
    let offset = -f32::from(tg.list_scroll.offset().y);
    // (Those in view and a row either side: a scroll draws again.)
    let first = (offset / ROW).floor().max(1.0) as usize - 1;
    let last = (((offset + view_h) / ROW).ceil() as usize + 1).min(total);
    let selected_chat = tg.conv.as_ref().map(|c| c.chat);
    let now = crate::now();
    let mut draws = Vec::new();
    for (i, row) in rows.iter().enumerate().take(last).skip(first) {
        let d = match *row {
            Row::Heading(t) => Draw::Heading(t),
            Row::Archive | Row::Unknown => {
                let archive = *row == Row::Archive;
                let pile: Vec<&Chat> = tg.store.order.iter().filter_map(|id| tg.store.chats.get(id)).filter(|c| if archive { c.archived } else { c.unknown && !c.archived }).collect();
                let names = pile.iter().take(4).map(|c| c.title.as_str()).collect::<Vec<_>>().join(", ");
                if archive {
                    Draw::Pile { title: "Archived Chats", icon: "archive", names, unread: pile.iter().map(|c| c.unread).sum() }
                } else {
                    Draw::Pile { title: "Unknown Senders", icon: "mail", names, unread: 0 }
                }
            }
            Row::Chat(id) => {
                let Some(ch) = tg.store.chats.get(&id) else { continue };
                let pic = if ch.photo.is_some() && ch.kind != ChatKind::Saved { tg.pics.get(&Pic::Avatar(id), &tg.backend) } else { None };
                // Those chosen together, else the one open.
                let selected = if tg.chosen.is_empty() { selected_chat == Some(id) } else { tg.chosen.contains(&id) };
                Draw::Chat { c: Box::new(ch.clone()), pic, selected, typing: tg.store.doing(id, None, now) }
            }
            Row::Found(k) => {
                let ch = &tg.found.chats[k];
                let pic = if ch.photo.is_some() { tg.pics.get(&Pic::Avatar(ch.id), &tg.backend) } else { None };
                Draw::Chat { c: Box::new(ch.clone()), pic, selected: false, typing: None }
            }
            Row::Msg(k) => {
                let msg = &tg.found.msgs[k];
                let title = tg.store.chats.get(&msg.chat).map_or_else(|| msg.from.as_ref().map(|f| f.name.clone()).unwrap_or_default(), |c| c.title.clone());
                let saved = tg.store.chats.get(&msg.chat).is_some_and(|c| c.kind == ChatKind::Saved);
                let pic = if saved { None } else { tg.pics.get(&Pic::Avatar(msg.chat), &tg.backend) };
                Draw::Msg { title, chat: msg.chat, saved, pic, summary: msg.summary(), date: msg.date }
            }
        };
        draws.push((i, d));
    }
    let hover = tg.list_hover.filter(|&i| i < total);
    let h24 = tg.h24;
    let top_of = tg.list_top.clone();
    let canvas_el = canvas(
        move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
        move |b, hitbox: Hitbox, window, cx| {
            top_of.set(f32::from(b.origin.y));
            // It scrolls: its rows are on a grid of their own, not level
            // with the conversation's beside it.
            olo_ui::review::own_grid(window, window.content_mask().bounds);
            if hover.is_some() {
                window.set_cursor_style(CursorStyle::Arrow, &hitbox);
            }
            let p = theme::palette(cx);
            let now = when::now();
            for (i, d) in draws {
                let rr = Bounds { origin: point(b.origin.x, b.origin.y + px(i as f32 * ROW)), size: size(b.size.width, px(ROW)) };
                paint_row(window, cx, rr, d, &p, hover == Some(i), now, h24);
            }
        },
    )
    .w_full()
    .h(px(total as f32 * ROW));
    let (t_move, t_click, t_menu, t_out) = (me.clone(), me.clone(), me.clone(), me.clone());
    let content = div()
        .id("chat-rows")
        .w_full()
        .h(px(total as f32 * ROW))
        .child(canvas_el)
        // (The row under the pointer redraws the list, not the window.)
        .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
            let changed = t_move.update(cx, |tg, _| {
                let i = tg.list_row_at(e.position);
                (i != tg.list_hover).then(|| {
                    tg.list_hover = i;
                    tg.pane(crate::Kind::List)
                })
            });
            if let Some(pane) = changed.flatten() {
                cx.notify(pane);
            }
        })
        .on_hover(move |on: &bool, _, cx| {
            if !*on {
                let changed = t_out.update(cx, |tg, _| tg.list_hover.take().and(tg.pane(crate::Kind::List)));
                if let Some(pane) = changed {
                    cx.notify(pane);
                }
            }
        })
        .on_click(move |e: &ClickEvent, window, cx| {
            let ClickEvent::Mouse(mc) = e else { return };
            t_click.update(cx, |tg, cx| {
                let Some(row) = tg.list_row_at(mc.up.position).and_then(|i| tg.list_rows.get(i).copied()) else { return };
                let act = match row {
                    Row::Archive => {
                        tg.view.archive = true;
                        cx.notify();
                        None
                    }
                    Row::Unknown => {
                        tg.view.unknown = true;
                        cx.notify();
                        None
                    }
                    Row::Chat(id) => {
                        let mods = mc.down.modifiers;
                        if mods.control || mods.platform {
                            // ⌘: this one chosen too, or not (with the
                            // chat open, when it's the first).
                            match tg.chosen.iter().position(|c| *c == id) {
                                Some(i) => {
                                    tg.chosen.remove(i);
                                }
                                None => {
                                    if tg.chosen.is_empty()
                                        && let Some(open) = tg.conv.as_ref().map(|c| c.chat).filter(|o| *o != id)
                                    {
                                        tg.chosen.push(open);
                                    }
                                    tg.chosen.push(id);
                                }
                            }
                            tg.chosen_from = Some(id);
                            cx.notify();
                            None
                        } else if mods.shift {
                            // ⇧: every chat from the last chosen (or the
                            // one open) to this one.
                            let ids: Vec<ChatId> = tg.list_rows.iter().filter_map(|r| if let Row::Chat(c) = r { Some(*c) } else { None }).collect();
                            let from = tg.chosen_from.or(tg.conv.as_ref().map(|c| c.chat)).and_then(|f| ids.iter().position(|x| *x == f));
                            if let Some(to) = ids.iter().position(|x| *x == id) {
                                let from = from.unwrap_or(to);
                                tg.chosen = ids[from.min(to)..=from.max(to)].to_vec();
                            }
                            cx.notify();
                            None
                        } else {
                            tg.chosen.clear();
                            tg.chosen_from = Some(id);
                            Some(Act::Open(id))
                        }
                    }
                    Row::Found(k) => tg.found.chats.get(k).map(|c| Act::Open(c.id)),
                    Row::Msg(k) => tg.found.msgs.get(k).map(|m| Act::OpenAt(m.chat, m.id)),
                    Row::Heading(_) => None,
                };
                if let Some(a) = act {
                    tg.list_act(a, window, cx);
                }
            })
        })
        .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, cx| {
            let menu = t_menu.update(cx, |tg, _| {
                let Row::Chat(id) = tg.list_row_at(e.position).and_then(|i| tg.list_rows.get(i).copied())? else { return None };
                // On one of several chosen: what's done to them all.
                if tg.chosen.len() > 1 && tg.chosen.contains(&id) {
                    return Some((id, tg.chats_menu(Menu::new(""))));
                }
                tg.chosen.clear();
                let c = tg.store.chats.get(&id)?;
                Some((
                    id,
                    Menu::new("")
                        .item(if c.pinned { "Unpin" } else { "Pin" }, "chat:pin")
                        .item(if c.muted { "Unmute" } else { "Mute" }, "chat:mute")
                        .item(if c.unread > 0 || c.marked { "Mark as Read" } else { "Mark as Unread" }, "chat:read")
                        .separator()
                        .item(if c.archived { "Unarchive" } else { "Archive" }, "chat:archive"),
                ))
            });
            if let Some((id, menu)) = menu {
                let to = t_menu.clone();
                olo_ui::menu::context_menu(window, cx, menu, e.position, move |cmd, window, cx| {
                    to.update(cx, |tg, cx| {
                        if cmd.starts_with("chats:") {
                            tg.chats_command(cmd, cx);
                        } else {
                            tg.list_act(Act::Command(id, cmd.to_owned()), window, cx);
                        }
                    })
                });
            }
        });
    let list_bg = p.list_background;
    let list_el = div()
        .absolute()
        .left(body.origin.x)
        .top(body.origin.y)
        .w(body.size.width)
        .h(body.size.height)
        .child(ScrollView::new("chat-list", &tg.list_scroll).border(false).background(list_bg).auto_hide(true).child(content));
    if total == 0 && !searching && tg.store.complete {
        let msg = if tg.view.archive {
            "No archived chats."
        } else if tg.view.unknown {
            "No unknown senders."
        } else if tg.view.folder.is_some() {
            "No chats in this folder."
        } else {
            "No chats yet."
        };
        hits.push(
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let dim = c(theme_palette::mix(p.list_item_text, p.list_background, 0.45));
                    style::label(window, cx, b.center().x, b.origin.y + px(40.0), Align::Center, msg, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(msg, Bounds { origin: point(b.origin.x, b.origin.y + px(30.0)), size: size(b.size.width, px(20.0)) }));
                },
            )
            .absolute()
            .left(body.origin.x)
            .top(body.origin.y)
            .w(body.size.width)
            .h(px(60.0))
            .into_any_element(),
        );
    }
    div()
        .absolute()
        .inset_0()
        .child(canvas(|_, _, _| {}, move |_, _, window, cx| paint.into_iter().for_each(|f| f(window, cx))).absolute().inset_0())
        .child(search)
        .child(list_el)
        .children(hits)
        .into_any_element()
}

impl Tg {
    /// The row under `q` (window coordinates), as last painted.
    fn list_row_at(&self, q: Point<Pixels>) -> Option<usize> {
        let y = f32::from(q.y) - self.list_top.get();
        let i = (y / ROW).floor();
        (y >= 0.0 && (i as usize) < self.list_rows.len()).then_some(i as usize)
    }
}

/// The text a one-line preview of `n` characters keeps: the first line's
/// worth, newlines as spaces.
fn one_line(text: &str) -> String {
    text.chars().take(300).map(|ch| if ch == '\n' { ' ' } else { ch }).collect()
}

#[allow(clippy::too_many_arguments)]
fn paint_row(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, d: Draw, p: &Palette, hovered: bool, now: i64, h24: bool) {
    let ink = c(p.list_item_text);
    let dim = c(theme_palette::mix(p.list_item_text, p.list_background, 0.45));
    let dark = p.mode == theme_palette::ThemeMode::Dark;
    let pic = Bounds { origin: point(r.origin.x + px(10.0), r.center().y - px(PICTURE / 2.0)), size: size(px(PICTURE), px(PICTURE)) };
    let x = pic.right() + px(12.0);
    let title_size = style::PLAIN_SIZE + 0.5;
    let small = style::DETAIL_SIZE;
    let base1 = style::baseline_at(window, r.origin.y + px(19.0), title_size);
    let base2 = style::baseline_at(window, r.origin.y + px(41.0), small);
    let bold = style::ui_font(FontWeight::BOLD);
    let normal = style::ui_font(FontWeight::NORMAL);
    match d {
        Draw::Heading(t) => {
            style::fill_rect(window, r, c(p.panel_background));
            style::label(window, cx, r.origin.x + px(m::SPACING + 4.0), r.bottom() - px(12.0), Align::Left, t, style::SMALL_SIZE, bold, dim);
            if !t.is_empty() {
                crate::seen(window, cx, olo_ui::agent::Node::label(t, r));
            }
        }
        Draw::Pile { title, icon: icon_name, names, unread } => {
            if hovered {
                style::fill_rect(window, r, ink.opacity(0.05));
            }
            window.paint_sector(pic.center(), px(0.0), px(PICTURE / 2.0), 0.0, std::f32::consts::TAU, c(theme_palette::mix(p.list_background, p.list_item_text, 0.35)));
            let icon = Bounds { origin: point(pic.center().x - px(13.0), pic.center().y - px(13.0)), size: size(px(26.0), px(26.0)) };
            olo_ui::icons::paint(window, icon, icon_name);
            style::label_on_baseline(window, cx, x, base1, Align::Left, title, title_size, bold.clone(), ink);
            let mut room = r.right() - px(12.0) - x;
            if unread > 0 {
                let t = unread.to_string();
                let w = style::pill_width(window, &t, 10.0, &normal);
                style::pill_on_baseline(window, cx, r.right() - px(12.0 + w), base2, &t, 10.0, normal.clone(), dim, white());
                room -= px(w + 6.0);
            }
            label_fit_on(window, cx, x, base2, one_line(&names), small, normal, dim, room);
            crate::seen(window, cx, olo_ui::agent::Node::button(title, r));
        }
        Draw::Msg { title, chat, saved, pic: img, summary, date } => {
            if hovered {
                style::fill_rect(window, r, ink.opacity(0.05));
            }
            if saved {
                window.paint_sector(pic.center(), px(0.0), px(PICTURE / 2.0), 0.0, std::f32::consts::TAU, c(p.keyboard_navigation));
                bookmark(window, pic.center(), PICTURE, white());
            } else {
                bubble::avatar(window, cx, pic, chat, &title, img.as_ref(), dark);
                olo_ui::review::own_grid(window, pic);
            }
            let t = when::short(date, now, h24);
            let tw = style::text_width(window, t.clone(), small, normal.clone());
            style::label_on_baseline(window, cx, r.right() - px(12.0) - tw, base1, Align::Left, t, small, normal.clone(), dim);
            label_fit_on(window, cx, x, base1, title.clone(), title_size, bold, ink, r.right() - x - tw - px(24.0));
            label_fit_on(window, cx, x, base2, one_line(&summary), small, normal, dim, r.right() - x - px(12.0));
            crate::seen(window, cx, olo_ui::agent::Node { role: "ListItem", name: Some(format!("{title}: {summary}").into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
        }
        Draw::Chat { c: ch, pic: img, selected, typing } => chat_row(window, cx, r, &ch, img, selected, typing, hovered, p, now, h24),
    }
}

/// One line of text on `baseline`, cut short with "…" to `max_width`.
#[allow(clippy::too_many_arguments)]
fn label_fit_on(window: &mut Window, cx: &mut App, x: Pixels, baseline: Pixels, text: impl Into<SharedString>, size_: f32, font: olo_ui::gpui::Font, color: Hsla, max_width: Pixels) -> Bounds<Pixels> {
    style::label_on_baseline_fit(window, cx, x, baseline, Align::Left, text, size_, font, color, max_width)
}

#[allow(clippy::too_many_arguments)]
fn chat_row(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, ch: &Chat, img: Option<Arc<RenderImage>>, selected: bool, typing: Option<String>, hovered: bool, p: &Palette, now: i64, h24: bool) {
    let accent = c(p.keyboard_navigation);
    let (ink, dim) = if selected {
        let t = c(p.list_selected_item_text);
        (t, t.opacity(150.0 / 255.0))
    } else {
        (c(p.list_item_text), c(theme_palette::mix(p.list_item_text, p.list_background, 0.45)))
    };
    let sel_fill = c(p.list_selected_background);
    if selected {
        style::fill_rect(window, r, sel_fill);
    } else if hovered {
        style::fill_rect(window, r, ink.opacity(0.05));
    }
    let pic = Bounds { origin: point(r.origin.x + px(10.0), r.center().y - px(PICTURE / 2.0)), size: size(px(PICTURE), px(PICTURE)) };
    if ch.kind == ChatKind::Saved {
        window.paint_sector(pic.center(), px(0.0), px(PICTURE / 2.0), 0.0, std::f32::consts::TAU, accent);
        bookmark(window, pic.center(), PICTURE, white());
    } else {
        bubble::avatar(window, cx, pic, ch.id, &ch.title, img.as_ref(), p.mode == theme_palette::ThemeMode::Dark);
        olo_ui::review::own_grid(window, pic);
    }
    if ch.seen == Some(olo_tg::Seen::Online) {
        let dot = point(pic.right() - px(7.0), pic.bottom() - px(7.0));
        window.paint_sector(dot, px(0.0), px(6.0), 0.0, std::f32::consts::TAU, if selected { sel_fill } else { c(p.list_background) });
        window.paint_sector(dot, px(0.0), px(4.5), 0.0, std::f32::consts::TAU, rgb(0x4CC14F).into());
    }
    let x = pic.right() + px(12.0);
    let right = r.right() - px(12.0);
    let title_size = style::PLAIN_SIZE + 0.5;
    let small = style::DETAIL_SIZE;
    let bold = style::ui_font(FontWeight::BOLD);
    let normal = style::ui_font(FontWeight::NORMAL);
    // Each line's parts on one baseline: the title and time, the preview
    // and counts.
    let base1 = style::baseline_at(window, r.origin.y + px(19.0), title_size);
    let base2 = style::baseline_at(window, r.origin.y + px(41.0), small);
    let mid1 = base1 - px(title_size * style::CAP_HEIGHT / 2.0);
    // Time, then the title in what's left of the line.
    let (t, out_read) = match &ch.last {
        Some(l2) => (when::short(l2.date.max(if ch.draft.is_some() { ch.order } else { 0 }), now, h24), (l2.out && !matches!(ch.kind, ChatKind::Saved | ChatKind::Channel)).then(|| ch.last_read())),
        None => (String::new(), None),
    };
    let tw = style::text_width(window, t.clone(), small, normal.clone());
    style::label_on_baseline(window, cx, right - tw, base1, Align::Left, t, small, normal.clone(), dim);
    let mut tx = right - tw;
    if let Some(read) = out_read {
        tx -= px(16.0);
        let ink_t = if read && !selected { accent } else { dim };
        let at = |dx: f32, dy: f32| point(tx + px(dx), mid1 + px(dy));
        window.paint_segment(at(0.0, 0.0), at(3.0, 3.0), px(1.3), true, ink_t);
        window.paint_segment(at(3.0, 3.0), at(9.0, -3.5), px(1.3), true, ink_t);
        if read {
            window.paint_segment(at(7.0, 3.0), at(13.0, -3.5), px(1.3), true, ink_t);
        }
    }
    let mut marks = 0.0;
    if ch.muted {
        marks += 14.0;
    }
    if ch.verified {
        marks += 16.0;
    }
    let title_room = tx - x - px(8.0 + marks);
    let tr = label_fit_on(window, cx, x, base1, ch.title.clone(), title_size, bold.clone(), ink, title_room);
    let mut mx = tr.right() + px(4.0);
    if ch.verified {
        let cc = point(mx + px(6.0), mid1);
        window.paint_sector(cc, px(0.0), px(6.0), 0.0, std::f32::consts::TAU, accent);
        window.paint_segment(point(cc.x - px(3.0), cc.y), point(cc.x - px(1.0), cc.y + px(2.2)), px(1.4), true, white());
        window.paint_segment(point(cc.x - px(1.0), cc.y + px(2.2)), point(cc.x + px(3.0), cc.y - px(2.2)), px(1.4), true, white());
        mx += px(16.0);
    }
    if ch.muted {
        speaker_off(window, point(mx + px(5.0), mid1), dim);
    }
    // The preview, and the counts at its right.
    let mut pxr = right;
    let pill_size = 10.0;
    // (Unknown senders' aren't counted unread: they wait, unannounced.)
    let count = if ch.unknown {
        None
    } else if ch.unread > 0 {
        Some(ch.unread.to_string())
    } else if ch.marked {
        Some(String::new())
    } else {
        None
    };
    if let Some(n) = &count {
        let fill = if ch.muted { c(theme_palette::mix(p.list_item_text, p.list_background, 0.55)) } else { accent };
        let fill = if selected { white() } else { fill };
        let tint = if selected { sel_fill } else { white() };
        let w = style::pill_width(window, n, pill_size, &bold).max(18.0);
        let pr = style::pill_on_baseline(window, cx, right - px(w), base2, n, pill_size, bold.clone(), fill, tint);
        pxr = pr.origin.x - px(4.0);
    }
    if ch.mentions > 0 {
        let fill = if selected { white() } else { accent };
        let pr = style::pill_on_baseline(window, cx, pxr - px(18.0), base2, "@", pill_size, bold.clone(), fill, if selected { sel_fill } else { white() });
        pxr = pr.origin.x - px(4.0);
    }
    if ch.pinned && count.is_none() && ch.mentions == 0 && !ch.archived {
        pin(window, point(right - px(6.0), base2 - px(small * style::CAP_HEIGHT / 2.0)), dim);
        pxr = right - px(16.0);
    }
    let room = pxr - x - px(6.0);
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut push = |s: &str, color: Hsla| {
        text.push_str(s);
        runs.push(TextRun { len: s.len(), font: normal.clone(), color, background_color: None, underline: None, strikethrough: None, size: None });
    };
    if let Some(t) = &typing {
        push(t, if selected { ink } else { accent });
    } else if let Some(d) = &ch.draft {
        push("Draft: ", if selected { ink } else { c(p.failure) });
        push(&one_line(d), dim);
    } else if let Some(last) = &ch.last {
        let who = if last.out && ch.kind != ChatKind::Saved && ch.kind != ChatKind::Channel {
            Some("You".to_owned())
        } else if ch.kind.many() {
            last.from.as_ref().map(|f| f.split(' ').next().unwrap_or(f).to_owned())
        } else {
            None
        };
        if let Some(w) = who {
            push(&format!("{w}: "), ink);
        }
        push(&one_line(&last.text), dim);
    }
    if !text.is_empty() {
        let mid2 = base2 - px(small * style::CAP_HEIGHT / 2.0);
        style::label_runs_fit(window, cx, x, mid2, Align::Left, text.into(), small, &runs, room);
    }
    // A hairline between rows, from the text's edge.
    if !selected {
        style::hline_above(window, x, r.right(), r.bottom(), c(p.shadow).opacity(0.18));
    }
    crate::seen(window, cx, olo_ui::agent::Node { role: "ListItem", name: Some(describe(ch).into()), bounds: r, enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
}

/// Saved Messages' bookmark, on a picture `d` across (drawn for one 46
/// across).
pub fn bookmark(window: &mut Window, cc: Point<Pixels>, d: f32, ink: Hsla) {
    let k = d / 46.0;
    let at = |dx: f32, dy: f32| point(cc.x + px(dx * k), cc.y + px(dy * k));
    let mut path = PathBuilder::fill();
    path.move_to(at(-7.0, -10.0));
    path.line_to(at(7.0, -10.0));
    path.line_to(at(7.0, 10.0));
    path.line_to(at(0.0, 4.0));
    path.line_to(at(-7.0, 10.0));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
}

/// A muted chat's mark: a speaker with a slash.
fn speaker_off(window: &mut Window, cc: Point<Pixels>, ink: Hsla) {
    let at = |dx: f32, dy: f32| point(cc.x + px(dx), cc.y + px(dy));
    let mut path = PathBuilder::fill();
    path.move_to(at(-5.0, -2.0));
    path.line_to(at(-2.5, -2.0));
    path.line_to(at(1.0, -5.0));
    path.line_to(at(1.0, 5.0));
    path.line_to(at(-2.5, 2.0));
    path.line_to(at(-5.0, 2.0));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
    window.paint_segment(at(-5.0, -5.5), at(5.0, 5.5), px(1.2), true, ink);
}

/// A pinned chat's mark: a pin, leaning.
fn pin(window: &mut Window, cc: Point<Pixels>, ink: Hsla) {
    window.paint_sector(point(cc.x + px(2.0), cc.y - px(2.5)), px(0.0), px(3.2), 0.0, std::f32::consts::TAU, ink);
    window.paint_segment(point(cc.x + px(1.0), cc.y - px(1.5)), point(cc.x - px(4.0), cc.y + px(4.5)), px(1.4), true, ink);
}

fn back_arrow(window: &mut Window, cc: Point<Pixels>, ink: Hsla) {
    window.paint_segment(point(cc.x + px(4.0), cc.y - px(5.0)), point(cc.x - px(1.0), cc.y), px(1.6), true, ink);
    window.paint_segment(point(cc.x - px(1.0), cc.y), point(cc.x + px(4.0), cc.y + px(5.0)), px(1.6), true, ink);
}
