//! The banners: as many cards as fit on the screen at its lower right,
//! grouped by app (in the order the apps first show), oldest first in
//! each, on a layer surface of their own as tall as they are. A group
//! is headed by its app's icon and name (a website's address beside it)
//! and a close box for them all; each card shows its face (`look`), its
//! actions as buttons along its bottom and, when it can be answered, a
//! reply field with Send. A click on a card is its default action (or
//! brings its app forward), on a link it opens the link; a right click
//! pins or snoozes it; one of several has its own close box under the
//! pointer.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use olo_kit_base::metrics as m;
use olo_protocol::NoteLook;
use olo_ui::{
    agent,
    gpui::{
        self, App, Bounds, ClickEvent, Context, CursorStyle, Entity, FontWeight, InteractiveElement as _, IntoElement, LayerKeyboard, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Point, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
        Window, canvas, div, point, prelude::FluentBuilder as _, px, size,
    },
    style::{self, Align},
    theme,
    widgets::{Button, TextControl, TextField, TextFieldEvent},
};

use crate::{
    Card, GAP, Notifications, SHOWN, WIDTH, look, markup::Span, note_menu, paint_mark, pictures, raise_app,
    server::{Kind, Reason},
    web,
};

const PAD: f32 = 8.0;
/// A picture the app sent (a contact, a site); without one the group's
/// header already shows whose it is, and the text takes the width.
const ICON: f32 = 24.0;
/// A group's title row: the app's icon and name.
const HEADER: f32 = 22.0;
/// Room kept clear above and below the cards on screen.
const SCREEN_EDGE: f32 = 32.0;
const BODY_ROWS: usize = 5;

/// What a face was measured for: a card's look then.
#[derive(Clone, PartialEq)]
struct FaceKey {
    summary: String,
    body: Vec<Span>,
    look: NoteLook,
    progress: Option<f32>,
    picture: bool,
    failed: bool,
    scale: u32,
    ink: look::Ink,
}

/// A card placed: its id, where, its face and colours, what it shows.
#[derive(Clone)]
struct Placed {
    id: u32,
    rect: Bounds<Pixels>,
    laid: look::Laid,
    ink: look::Ink,
    title: String,
    body: Vec<Span>,
    look: NoteLook,
    progress: Option<f32>,
    picture: Option<pictures::Source>,
    failed: bool,
    buttons: Vec<(String, String)>,
    reply: Option<Entity<TextField>>,
    pinned: bool,
}

/// A group of cards: its app, its frame and its cards.
#[derive(Clone)]
struct Group {
    app: String,
    frame: Bounds<Pixels>,
    cards: Vec<Placed>,
    desktop_entry: Option<String>,
    icon: Option<String>,
    accent: Option<[u8; 3]>,
    via: bool,
    source: Option<String>,
}

/// What a click on a card did.
enum Did {
    Dismiss,
    Action(String),
    /// A click with no default action: the app comes forward.
    Raise,
    Reply(String),
    Link(String),
}

/// Where each card's body was drawn, by note (for its links).
type Bodies = Rc<RefCell<HashMap<u32, (Point<Pixels>, look::Text)>>>;

/// Whose a card is, for its group: app name, desktop entry, icon, accent,
/// whether a website's, and where in the app it came from.
type Whose = (String, Option<String>, Option<String>, Option<[u8; 3]>, bool, Option<String>);

pub struct Banners {
    app: Entity<Notifications>,
    /// Faces measured, by card.
    faces: HashMap<u32, (FaceKey, look::Laid)>,
    /// The surface's height and keyboard last asked for.
    height: f32,
    keyboard: bool,
    /// Where each card's body went in the last frame (links).
    bodies: Bodies,
    /// The close box under the pointer: a card's, or (None) a group's
    /// (by its first card).
    close_hover: Option<(u32, bool)>,
    /// The pointer is on a link.
    on_link: bool,
    /// Reply fields' Return, by card.
    replies: HashMap<u32, Subscription>,
    _app: Subscription,
}

/// The colours of a card on screen (on the panel's grey).
fn banner_ink(p: &olo_theme::Palette, note_look: &NoteLook) -> look::Ink {
    look::Ink {
        text: style::c(p.panel_text),
        soft: style::c(p.panel_text),
        dim: style::disabled_text(p),
        link: style::c(p.keyboard_navigation),
        balloon: style::c(p.document_background),
        balloon_edge: style::tint(p.panel_background, 1.25),
        accent: note_look.accent.map(|[r, g, b]| [r, g, b, 255]),
    }
}

/// A card's actions along its bottom (not its default one, nor a
/// browser's own Settings, which stays in the history).
fn buttons(card: &Card) -> Vec<(String, String)> {
    card.note.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(card.note.via.as_deref(), k)).cloned().collect()
}

impl Banners {
    pub fn new(app: Entity<Notifications>, cx: &mut Context<Self>) -> Banners {
        let sub = cx.observe(&app, |_, _, cx| cx.notify());
        Banners { app, faces: HashMap::new(), height: 0.0, keyboard: false, bodies: Rc::default(), close_hover: None, on_link: false, replies: HashMap::new(), _app: sub }
    }

    /// The cards that show, laid out and grouped: as many as fit on the
    /// screen (one, at least), grouped by app in the order the apps first
    /// show, oldest first in each; and the height they take.
    fn arrange(&mut self, window: &mut Window, cx: &mut Context<Self>) -> (Vec<Group>, usize, f32) {
        let p = theme::palette(cx);
        let room = look::Room { width: WIDTH - 2.0 * PAD, title_rows: 2, body_rows: BODY_ROWS, title_reserve: 16.0, picture: ICON, image_max: 120.0, fields_max: 4 };
        let scale = (window.scale_factor() * 1000.0).round() as u32;
        let most = window.display(cx).map_or(f32::INFINITY, |d| f32::from(d.bounds().size.height) - 2.0 * SCREEN_EDGE);
        let app = self.app.read(cx);
        let cards: Vec<&Card> = app.cards.iter().take(SHOWN).collect();
        // Each card's face, measured once for what it shows.
        let mut placed: Vec<Placed> = Vec::new();
        let mut seen = Vec::new();
        for card in &cards {
            let ink = banner_ink(&p, &card.note.look);
            let key = FaceKey {
                summary: card.note.summary.clone(),
                body: card.note.body.clone(),
                look: card.note.look.clone(),
                progress: card.note.progress,
                picture: card.picture.is_some(),
                failed: card.note.kind == Kind::Error,
                scale,
                ink: ink.clone(),
            };
            let id = card.note.id;
            seen.push(id);
            let laid = match self.faces.get(&id) {
                Some((k, laid)) if *k == key => laid.clone(),
                _ => {
                    let face = look::Face { title: &card.note.summary, body: &card.note.body, look: &card.note.look, progress: card.note.progress, picture: card.picture.clone(), failed: key.failed };
                    let laid = look::lay(window, &face, &ink, &room);
                    self.faces.insert(id, (key, laid.clone()));
                    laid
                }
            };
            placed.push(Placed {
                id,
                rect: Bounds::default(),
                laid,
                ink,
                title: card.note.summary.clone(),
                body: card.note.body.clone(),
                look: card.note.look.clone(),
                progress: card.note.progress,
                picture: card.picture.clone(),
                failed: card.note.kind == Kind::Error,
                buttons: buttons(card),
                reply: card.reply.clone(),
                pinned: app.history.newest(id).is_some_and(|e| e.pinned),
            });
        }
        self.faces.retain(|id, _| seen.contains(id));
        let notes: Vec<Whose> =
            cards.iter().map(|c| (c.note.app_name.clone(), c.note.desktop_entry.clone(), c.note.icon.clone(), c.note.look.accent, c.note.via.is_some(), c.note.look.source.clone())).collect();
        let mut n = placed.len();
        loop {
            let mut groups: Vec<Group> = Vec::new();
            for (i, card) in placed.iter().take(n).enumerate() {
                let (app_name, desktop_entry, icon, accent, via, source) = &notes[i];
                match groups.iter_mut().find(|g| g.app == *app_name) {
                    Some(g) => g.cards.push(card.clone()),
                    None => groups.push(Group { app: app_name.clone(), frame: Bounds::default(), cards: vec![card.clone()], desktop_entry: desktop_entry.clone(), icon: icon.clone(), accent: *accent, via: *via, source: source.clone() }),
                }
            }
            let mut y = 0.0;
            for g in &mut groups {
                let top = y;
                y += HEADER;
                for c in &mut g.cards {
                    let mut h = PAD + c.laid.height + PAD;
                    if c.reply.is_some() {
                        h += m::CONTROL_HEIGHT + PAD;
                    }
                    if !c.buttons.is_empty() {
                        h += m::CONTROL_HEIGHT + PAD;
                    }
                    c.rect = Bounds::new(point(px(0.0), px(y)), size(px(WIDTH), px(h.ceil())));
                    y += h.ceil();
                }
                g.frame = Bounds::new(point(px(0.0), px(top)), size(px(WIDTH), px(y - top)));
                y += GAP;
            }
            if n <= 1 || y - GAP <= most {
                return (groups, n, y);
            }
            n -= 1;
        }
    }

    fn did(&mut self, id: u32, did: Did, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        app.update(cx, |n, cx| match did {
            Did::Dismiss => n.remove(cx, id, Reason::Dismissed),
            Did::Action(key) => n.invoke(cx, Some(window), id, key),
            Did::Raise => {
                if let Some(card) = n.cards.iter().find(|c| c.note.id == id) {
                    raise_app(card.note.desktop_entry.as_deref(), &card.note.app_name);
                }
                n.remove(cx, id, Reason::Dismissed);
            }
            Did::Reply(text) => n.reply(cx, id, &text),
            Did::Link(url) => {
                n.set_read(cx, id, true);
                std::thread::spawn(move || {
                    if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::parse(&url)) {
                        tracing::warn!("notifications: can't open {url}: {e}");
                    }
                });
            }
        });
    }

    /// The link under `pos` in card `id`'s body.
    fn link_at(&self, id: u32, pos: Point<Pixels>, cx: &App) -> Option<String> {
        let bodies = self.bodies.borrow();
        let (origin, text) = bodies.get(&id)?;
        let at = text.index_at(pos - *origin)?;
        let app = self.app.read(cx);
        let card = app.cards.iter().find(|c| c.note.id == id)?;
        look::link_at(&card.note.body, at)
    }
}

impl Render for Banners {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (groups, fits, y) = self.arrange(window, cx);
        // Those coming on screen have their time from now.
        let app = self.app.clone();
        window.defer(cx, move |_, cx| {
            app.update(cx, |n, _| {
                if fits > n.fits {
                    n.wake(n.fits..fits);
                }
                n.fits = fits;
            })
        });
        // As tall as they are; the keyboard only while there is something
        // to type into.
        let total = (y - GAP).max(1.0).ceil();
        if total != self.height {
            self.height = total;
            window.defer(cx, move |window, _| window.resize(size(px(WIDTH), px(total))));
        }
        let wants_keys = groups.iter().flat_map(|g| &g.cards).any(|c| c.reply.is_some());
        if wants_keys != self.keyboard {
            self.keyboard = wants_keys;
            window.set_layer_keyboard(if wants_keys { LayerKeyboard::OnDemand } else { LayerKeyboard::None });
        }
        // Reply fields' Return sends.
        let ids: Vec<u32> = groups.iter().flat_map(|g| &g.cards).map(|c| c.id).collect();
        self.replies.retain(|id, _| ids.contains(id));
        for c in groups.iter().flat_map(|g| &g.cards) {
            if let Some(field) = &c.reply
                && !self.replies.contains_key(&c.id)
            {
                let id = c.id;
                let sub = cx.subscribe_in(field, window, move |me: &mut Banners, field, e: &TextFieldEvent, window, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        let text = field.read(cx).text().trim().to_owned();
                        if !text.is_empty() {
                            me.did(id, Did::Reply(text), window, cx);
                        }
                    }
                });
                self.replies.insert(c.id, sub);
            }
        }

        let hovered = self.app.read(cx).hovered;
        let close_hover = self.close_hover;
        let bodies = self.bodies.clone();
        let painted = groups.clone();
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let origin = b.origin;
                bodies.borrow_mut().clear();
                for g in &painted {
                    let frame = g.frame + origin;
                    window.paint_quad(gpui::fill(frame, style::c(p.panel_background)));
                    style::frame_rect(window, frame, style::tint(p.panel_background, 1.6));
                    style::raised(window, style::inset(window, frame, 1.0), &p);
                    // The group's title: the app's icon and name (and, for a
                    // website, its address) and the close box, for all of them.
                    let head = Bounds::new(frame.origin, size(px(WIDTH), px(HEADER)));
                    let mid = style::snap_coord(window, head.center().y);
                    let small = Bounds::new(point(head.left() + px(PAD), mid - px(8.0)), size(px(16.0), px(16.0)));
                    let names = crate::named_icons(g.icon.as_deref(), g.desktop_entry.as_deref(), &g.app);
                    paint_mark(window, cx, small, &g.app, g.accent, g.via, &names);
                    let name = if g.app.is_empty() { "Notification" } else { g.app.as_str() };
                    let mut close_room = 22.0;
                    let small_font = style::ui_font(FontWeight::NORMAL);
                    // Pinned (all of them): it says so, by the close box.
                    if g.cards.iter().all(|c| c.pinned) {
                        let r = style::label(window, cx, head.right() - px(PAD + close_room), mid, Align::Right, "Pinned", style::DETAIL_SIZE, small_font.clone(), style::disabled_text(&p));
                        agent::record(window, cx, agent::Node::label("Pinned", r));
                        close_room += f32::from(r.size.width) + 8.0;
                    }
                    let name_x = small.right() + px(6.0);
                    let n = style::label_fit(window, cx, name_x, mid, Align::Left, name.to_owned(), style::DETAIL_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), head.right() - px(PAD + close_room) - name_x);
                    agent::record(window, cx, agent::Node::label(name.to_owned(), n));
                    if let Some(src) = &g.source {
                        let x = n.right() + px(5.0);
                        let room = head.right() - px(PAD + close_room) - x;
                        if room > px(24.0) {
                            style::label_fit(window, cx, x, mid, Align::Left, src.clone(), style::DETAIL_SIZE, small_font.clone(), style::disabled_text(&p), room);
                        }
                    }
                    let all = Bounds::centered_at(point(head.right() - px(PAD + 6.0), mid), size(px(12.0), px(12.0)));
                    let lit = close_hover == Some((g.cards[0].id, false));
                    style::close_box(window, all, if lit { style::c(p.panel_text) } else { style::disabled_text(&p) });
                    style::hline_above(window, frame.left() + px(2.0), frame.right() - px(2.0), head.bottom(), style::tint(p.panel_background, 1.25));
                    let single = g.cards.len() == 1;
                    for (k, c) in g.cards.iter().enumerate() {
                        let rect = c.rect + origin;
                        if k > 0 {
                            style::hline(window, rect.left() + px(PAD), rect.right() - px(PAD), rect.top(), style::tint(p.panel_background, 1.18));
                            // (Right under it: a point lower left a device
                            // pixel between them at 1.5×.)
                            style::hline(window, rect.left() + px(PAD), rect.right() - px(PAD), style::snap_coord(window, rect.top()) + style::hairline(window), style::c(p.shine));
                        }
                        // One of several has its own close box too, while
                        // the pointer is over it.
                        if !single && hovered == Some(c.id) {
                            let close = Bounds::centered_at(point(rect.right() - px(PAD + 6.0), rect.top() + px(PAD + 7.0)), size(px(12.0), px(12.0)));
                            let lit = close_hover == Some((c.id, true));
                            style::close_box(window, close, if lit { style::c(p.panel_text) } else { style::disabled_text(&p) });
                        }
                        let face = look::Face { title: &c.title, body: &c.body, look: &c.look, progress: c.progress, picture: c.picture.clone(), failed: c.failed };
                        let at = rect.origin + point(px(PAD), px(PAD));
                        let painted = look::paint(window, cx, &face, &c.laid, &c.ink, at, WIDTH - 2.0 * PAD);
                        if let Some(body) = painted.body {
                            bodies.borrow_mut().insert(c.id, body);
                        }
                    }
                }
            },
        )
        .absolute()
        .inset_0();

        let me = cx.entity();
        let mut root = div()
            .id("banners")
            .relative()
            .w(px(WIDTH))
            .h(px(total))
            .child(paint)
            // Which card the pointer is on (it stays while it is).
            .on_mouse_move({
                let me = me.clone();
                let rects: Vec<(u32, Bounds<Pixels>)> = groups.iter().flat_map(|g| g.cards.iter().map(|c| (c.id, c.rect))).collect();
                move |e: &MouseMoveEvent, _, cx| {
                    me.update(cx, |b, cx| {
                        let pos = e.position;
                        let over = rects.iter().find(|(_, r)| r.contains(&pos)).map(|(id, _)| *id);
                        let link = over.is_some_and(|id| b.link_at(id, pos, cx).is_some());
                        if link != b.on_link {
                            b.on_link = link;
                            cx.notify();
                        }
                        b.app.update(cx, |n, cx| n.hover(over, cx));
                    })
                }
            })
            .on_hover({
                let me = me.clone();
                move |inside, _, cx| {
                    if !*inside {
                        me.update(cx, |b, cx| {
                            b.close_hover = None;
                            b.app.update(cx, |n, cx| n.hover(None, cx));
                        });
                    }
                }
            });
        for g in &groups {
            let single = g.cards.len() == 1;
            // The group's close box: all of them (not those pinned, which
            // stay put; one alone goes all the same).
            let head_mid = f32::from(g.frame.top()) + HEADER / 2.0;
            let all = Bounds::centered_at(point(px(WIDTH - PAD - 6.0), px(head_mid)), size(px(18.0), px(18.0)));
            let (first, members): (u32, Vec<(u32, bool)>) = (g.cards[0].id, g.cards.iter().map(|c| (c.id, c.pinned)).collect());
            let name = if g.app.is_empty() { "Notification".to_owned() } else { g.app.clone() };
            root = root.child(close_area(("close-all", first), all, if single { "Close".into() } else { format!("Close All from {name}") }, (first, false), &me, move |b, window, cx| {
                for &(id, pinned) in &members {
                    if single || !pinned {
                        b.did(id, Did::Dismiss, window, cx);
                    }
                }
            }));
            for c in &g.cards {
                let id = c.id;
                // The whole card is the default action; its menu pins or
                // snoozes it; a link in its body opens.
                let has_default = self.app.read(cx).cards.iter().any(|k| k.note.id == id && k.note.actions.iter().any(|(k, _)| k == "default"));
                let pinned = c.pinned;
                let area = div()
                    .id(("card", id))
                    .absolute()
                    .left(c.rect.left())
                    .top(c.rect.top())
                    .w(c.rect.size.width)
                    .h(c.rect.size.height)
                    .when(self.on_link && hovered == Some(id), |d| d.cursor(CursorStyle::PointingHand))
                    .on_click({
                        let me = me.clone();
                        move |e: &ClickEvent, window, cx| {
                            let pos = e.position();
                            me.update(cx, |b, cx| {
                                let did = match b.link_at(id, pos, cx) {
                                    Some(url) => Did::Link(url),
                                    None if has_default => Did::Action("default".into()),
                                    None => Did::Raise,
                                };
                                b.did(id, did, window, cx);
                            })
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let me = me.clone();
                        move |e: &MouseDownEvent, window, cx| {
                            let me = me.clone();
                            olo_ui::menu::context_menu(window, cx, note_menu(olo_ui::Menu::new("Notification"), Some(pinned)), e.position, move |command, _, cx| {
                                let command = command.to_owned();
                                me.update(cx, |b, cx| b.app.update(cx, |n, cx| n.note_command(cx, id, &command)));
                            });
                            cx.stop_propagation();
                        }
                    });
                root = root.child(area);
                if !single && hovered == Some(id) {
                    let close = Bounds::centered_at(point(c.rect.right() - px(PAD + 6.0), c.rect.top() + px(PAD + 7.0)), size(px(18.0), px(18.0)));
                    root = root.child(close_area(("close", id), close, "Close".into(), (id, true), &me, move |b, window, cx| b.did(id, Did::Dismiss, window, cx)));
                }
                // Actions along the bottom, right to left as in an alert.
                let mut by = c.rect.bottom() - px(PAD + m::CONTROL_HEIGHT);
                if !c.buttons.is_empty() {
                    let mut bx = c.rect.right() - px(PAD);
                    // In the narrow column they share the width when they must.
                    let n = c.buttons.len() as f32;
                    let share = ((WIDTH - 2.0 * PAD - (n - 1.0) * m::SPACING) / n).floor();
                    for (key, label) in c.buttons.iter().rev() {
                        let w = (f32::from(style::text_width(window, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))) + 24.0).max(m::BUTTON_MIN_WIDTH).min(share).ceil();
                        let key = key.clone();
                        let me = me.clone();
                        let button = Button::new(label.clone()).min_width(w).on_click(move |window, cx| me.update(cx, |b, cx| b.did(id, Did::Action(key.clone()), window, cx)));
                        root = root.child(div().absolute().left(bx - px(w)).top(by).w(px(w)).occlude().child(button));
                        bx -= px(w + m::SPACING);
                    }
                    by -= px(m::CONTROL_HEIGHT + PAD);
                }
                // The reply field and its Send button.
                if let Some(field) = &c.reply {
                    let x = c.rect.left() + px(PAD);
                    let send_w = m::BUTTON_MIN_WIDTH;
                    let field_w = f32::from(c.rect.right() - x) - PAD - send_w - m::SPACING;
                    let typed = !field.read(cx).text().trim().is_empty();
                    // (Controls keep the card under them from the pointer.)
                    root = root.child(div().absolute().left(x).top(by).occlude().child(TextControl::new(field.clone()).width(field_w)));
                    let (me, field) = (me.clone(), field.clone());
                    let send = Button::new("Send").min_width(send_w).enabled(typed).on_click(move |window, cx| {
                        let text = field.read(cx).text().trim().to_owned();
                        if !text.is_empty() {
                            me.update(cx, |b, cx| b.did(id, Did::Reply(text), window, cx));
                        }
                    });
                    root = root.child(div().absolute().left(c.rect.right() - px(PAD + send_w)).top(by).occlude().child(send));
                }
            }
        }
        root
    }
}

/// A close box's place: it lights under the pointer and closes on a
/// click (`what` for agents).
fn close_area(id: (&'static str, u32), r: Bounds<Pixels>, what: String, key: (u32, bool), me: &Entity<Banners>, on_click: impl Fn(&mut Banners, &mut Window, &mut Context<Banners>) + 'static) -> impl IntoElement {
    let (hover_me, click_me) = (me.clone(), me.clone());
    div()
        .id(id)
        .absolute()
        .left(r.left())
        .top(r.top())
        .w(r.size.width)
        .h(r.size.height)
        .occlude()
        .child(canvas(|_, _, _| {}, move |b, _, window, cx| agent::record(window, cx, agent::Node::button(what.clone(), b))).size_full())
        .on_hover(move |inside, _, cx| {
            hover_me.update(cx, |b, cx| {
                let now = if *inside { Some(key) } else { None };
                if b.close_hover != now && (*inside || b.close_hover == Some(key)) {
                    b.close_hover = now;
                    cx.notify();
                }
            })
        })
        .on_click(move |_, window, cx| {
            click_me.update(cx, |b, cx| on_click(b, window, cx));
            cx.stop_propagation();
        })
}
