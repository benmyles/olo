//! The banners: as many cards as fit on the screen at its lower right,
//! grouped by app (in the order the apps first show), oldest first in
//! each, on a layer surface of their own as tall as they are. A group
//! is headed by its app's icon and name (a website's address beside it)
//! and a close box for them all; each card shows its face (`look`), its
//! actions as buttons in a row along its bottom and, when it can be
//! answered, a reply field with Send. A click on a card is its default
//! action (or brings its app forward), on a link it opens the link; a
//! right click has its actions, and pins or snoozes it; one of several
//! has its own close box under the pointer. Its large picture, where
//! that is a file (a screenshot), is the open hand under the pointer:
//! pressed and moved, the file is dragged out, the card staying while
//! it is, and going once it has been dropped somewhere.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use olo_kit::{
    Bounds, Child, ClickEvent, CursorStyle, Cx, DragAction, Element, FontWeight, Hsla, InteractiveElement as _, LayerKeyboard, MouseButton, MouseDownEvent, MouseMoveEvent, Painter,
    ParentElement as _, Pixels, Point, Styled as _, View, WindowCx, agent, canvas, div, dnd, fill, menu, metrics as m, point, px, size,
    style::{self, Align},
    theme,
    widgets::{Button, SMALL_BUTTON_HEIGHT, TextControl, TextField, TextFieldEvent, button_width, text_field},
};
use olo_protocol::{NoteLook, notifications::FileId};

use crate::{
    Card, GAP, SHOWN, Shared, WIDTH, look,
    markup::Span,
    note_menu, paint_mark, pictures, raise_app,
    server::{Kind, Reason},
    web, with,
};

/// A card's words, picture and buttons stand this far in from its sides
/// and its bottom: the same all round, no tighter at the edge than the
/// gaps inside it (they were 8 points, less its bevel, against 10 to 12
/// between its parts, and the buttons looked to slide out of the bottom).
const INSET: f32 = 10.0;
/// Its first line's top, under the rule over it (its tallest letters 8
/// points under the rule: measured).
const TOP: f32 = 6.0;
/// Between what it shows (its picture, its words) and its buttons.
const BUTTON_GAP: f32 = 8.0;
/// The clear edge round the header's 16-point icon: set this much left of
/// the words, its drawing begins where they do.
const ICON_MARGIN: f32 = 1.0;
/// A picture the app sent (a contact, a site); without one the group's
/// header already shows whose it is, and the text takes the width.
const ICON: f32 = 24.0;
/// A group's title row: the app's icon and name.
const HEADER: f32 = 22.0;
/// Room kept clear above and below the cards on screen.
const SCREEN_EDGE: f32 = 32.0;
/// The line under the banners that says how many more are waiting
/// their turn.
const MORE: f32 = 22.0;
const BODY_ROWS: usize = 5;
/// A large picture's greatest height: a 16:10 screen's, as wide as the
/// card (a tall window's is this tall, narrower, centred: at 160 points,
/// a phone-shaped one made a card a third of the screen's height).
const IMAGE_MOST: f32 = 98.0;
/// How far a press on a picture moves before it's a drag (points).
const DRAG_AFTER: f32 = 4.0;

/// What a face (and its row of buttons) was measured for: a card's
/// look and its actions then. Measured again only when that changes
/// (the buttons' words were shaped again for every frame).
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
    /// Its actions on its row (their words measured once, with its face).
    actions: Vec<(String, String)>,
    /// One of several of its app's: room kept at its title's end for its
    /// own × (`card_close_at`); alone, its name has the whole width.
    several: bool,
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
    /// Its actions as laid out in its row; those that don't fit are in
    /// its menu (a right click), with all the rest.
    row: Row,
    /// All its actions, for its menu: key and label (its default one as
    /// Open).
    actions: Vec<(String, String)>,
    /// It can be answered: the hint in its reply field.
    reply: Option<String>,
    pinned: bool,
    /// A click on it is its default action (else its app comes forward).
    has_default: bool,
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

/// What the banners' view is told.
pub enum Told {
    /// The model changed (a card came, went, was pinned).
    Changed,
    /// A card's menu's command (`note_menu`, or `action:KEY`), for note
    /// `id`.
    Command(u32, String),
    /// Said (to the model's `shown`, with this wait's tag) once what the
    /// banners show now is on the screen (`PutAway`: those just put away
    /// are not; the last gone: the surface empty).
    Shown(u64),
    /// Note `id`'s picture, dragged out, was dropped as this (None:
    /// nowhere).
    Dropped(u32, Option<DragAction>),
}

/// Where each card's body was drawn, by note (for its links).
type Bodies = Rc<RefCell<HashMap<u32, (Point<Pixels>, look::Text)>>>;

/// Where each card's large picture was drawn, by note, where it can be
/// dragged out as its file.
type Images = Rc<RefCell<HashMap<u32, Bounds<Pixels>>>>;

/// Whose a card is, for its group: app name, desktop entry, icon, accent,
/// whether a website's, and where in the app it came from.
type Whose = (String, Option<String>, Option<String>, Option<[u8; 3]>, bool, Option<String>);

pub struct Banners {
    shared: Shared,
    /// Faces measured, by card.
    faces: HashMap<u32, (FaceKey, look::Laid, Row)>,
    /// The surface's height and keyboard last asked for.
    height: f32,
    keyboard: bool,
    /// Where each card's body went in the last frame (links).
    bodies: Bodies,
    /// Where each card's large picture went in the last frame (drags).
    images: Images,
    /// The close box under the pointer: a card's, or (None) a group's
    /// (by its first card).
    close_hover: Option<(u32, bool)>,
    /// Reply fields, by card (Return sends, as Send does).
    replies: HashMap<u32, Child<TextField>>,
    /// The card whose picture the pointer is over (its frame lights).
    picture_lit: Option<u32>,
    /// A press on a card's picture: which card, and where (a drag once
    /// it has moved far enough).
    press: Option<(u32, Point<Pixels>)>,
    /// The card whose picture is being dragged out (a click on it isn't
    /// one).
    dragging: Option<u32>,
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
        paper: style::c(p.panel_background),
    }
}

/// A close box's ×, centred on `centre` (`look::close_cross`).
fn close_mark(g: &mut Painter, centre: Point<Pixels>, color: Hsla) {
    look::close_cross(g, centre, color);
}

/// A card's actions along its bottom (not its default one, nor a
/// browser's own Settings, which stays in the history).
fn buttons(card: &Card) -> Vec<(String, String)> {
    card.note.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(card.note.via.as_deref(), k)).cloned().collect()
}

/// A card's actions for its menu: its default one as Open (one its app
/// no longer has open, with what shows what it's about, all the same:
/// a click on it still opens it), then the rest, as the History's rows
/// have them.
fn menu_actions(card: &Card) -> Vec<(String, String)> {
    let opens = card.note.actions.iter().any(|(k, _)| k == "default") || !card.note.show.is_empty();
    let mut all: Vec<(String, String)> = if opens { vec![("default".to_owned(), "Open".to_owned())] } else { Vec::new() };
    all.extend(buttons(card));
    all
}

/// A card's row of buttons, laid out.
#[derive(Clone, Default)]
struct Row {
    /// Key, label (cut short only when even one alone is too wide for
    /// the row) and width, in the app's order.
    buttons: Vec<(String, String, f32)>,
    /// Small ones (as the History's cards have them): side by side at
    /// the alert's size they wouldn't fit.
    small: bool,
    /// The first stands apart at the row's start (`NoteLook::apart`), the
    /// rest at its end.
    apart: bool,
}

/// `actions` laid out in a card's row: side by side at their own widths,
/// as in an alert, when they fit its width; else as small buttons (as
/// the History's cards have them, a little closer), two or three short
/// words fitting where two of the alert's size don't (Deskbar's width
/// is a little more than two); else as many of those as fit, in the
/// app's order, the rest in the card's menu, where all of them are (one
/// alone too wide cut short with "…"). The one the note says stands
/// apart (`apart`) goes first, at the row's start, at least a spacing
/// from the rest, as a BAlert's leftmost with `B_OFFSET_SPACING`. (One
/// above another, each the card's width, three of Screenshot's took
/// more of the card than its picture did.)
fn lay_buttons(cx: &WindowCx, actions: Vec<(String, String)>, apart: Option<&str>) -> Row {
    let room = WIDTH - 2.0 * INSET;
    let mut rest = actions;
    let lead = take_apart(&mut rest, apart);
    let gap = |small: bool| if small { m::TIGHT } else { m::SPACING };
    // (Each as wide as it reads, in the person's language.)
    let total = |small: bool| -> f32 {
        let theirs: f32 = rest.iter().map(|(_, l)| button_width(l, small)).sum::<f32>() + gap(small) * rest.len().saturating_sub(1) as f32;
        theirs + lead.as_ref().map_or(0.0, |(_, l)| button_width(l, small) + m::SPACING)
    };
    if total(false) <= room {
        let buttons = lead.iter().chain(&rest).map(|(k, l)| (k.clone(), l.clone(), button_width(l, false))).collect();
        return Row { buttons, small: false, apart: lead.is_some() };
    }
    // Small. The one apart only with at least the first of the rest
    // beside it: alone, it would sit where the one a person reaches for
    // goes (it's in the card's menu all the same).
    let lead = lead.map(|(k, l)| {
        let w = button_width(&l, true);
        (k, l, w)
    });
    let first = rest.first().map(|(_, l)| button_width(l, true));
    let lead = lead.filter(|(_, _, w)| first.is_some_and(|f| w + m::SPACING + f <= room));
    let mut used = lead.as_ref().map_or(0.0, |(_, _, w)| w + m::SPACING);
    let apart = lead.is_some();
    let mut laid: Vec<(String, String, f32)> = lead.into_iter().collect();
    for (placed, (k, l)) in rest.into_iter().enumerate() {
        let w = button_width(&l, true);
        let at = if placed > 0 { used + m::TIGHT } else { used };
        if at + w > room {
            if laid.is_empty() {
                // (One alone too wide: cut short, as it reads.)
                let font = style::ui_font(FontWeight::NORMAL);
                let cut = style::fit_with(cx.text_system(), olo_kit::words::of_text(&l), style::DETAIL_SIZE, &font, px(room - 2.0 * m::CONTROL_PAD * 0.6)).to_string();
                laid.push((k, cut, room));
            }
            break;
        }
        used = at + w;
        laid.push((k, l, w));
    }
    Row { buttons: laid, small: true, apart }
}

/// The tint of a card a picture is being dragged out of: the selection's
/// colour, paled toward the panel's.
fn drag_tint(p: &olo_theme::Palette) -> Hsla {
    style::c(olo_theme::mix(p.list_selected_background, p.panel_background, 0.35))
}

/// Where one card's own ×, of several in a group, goes in the card `rect`:
/// its right end on the words' right edge, level with the title's first
/// line.
fn card_close_at(rect: Bounds<Pixels>) -> Point<Pixels> {
    point(rect.right() - px(INSET + look::CLOSE / 2.0), rect.top() + px(TOP + 7.0))
}

/// What a right press on card `id` (anywhere on it: on its action buttons
/// too) does: its menu, with its actions; pinning and snoozing it; and
/// the one set apart (a Delete) last, on its own, as Tracker's Move to
/// Trash.
fn card_menu(cx: &Cx<Banners>, id: u32, actions: Vec<(String, String)>, apart: Option<String>, pinned: bool) -> impl Fn(&MouseDownEvent, &mut WindowCx) + 'static {
    cx.listener(move |_: &mut Banners, e: &MouseDownEvent, cx| {
        let chosen = cx.msg_with(move |command: &str| Told::Command(id, command.to_owned()));
        let is_apart = |k: &String| apart.as_ref() == Some(k);
        let mut menu = olo_kit::Menu::new("Notification");
        for (key, label) in actions.iter().filter(|(k, _)| !is_apart(k)) {
            menu = menu.item(label.clone(), format!("action:{key}"));
        }
        if actions.iter().any(|(k, _)| !is_apart(k)) {
            menu = menu.separator();
        }
        let mut menu = note_menu(menu, Some(pinned));
        if let Some((key, label)) = actions.iter().find(|(k, _)| is_apart(k)) {
            menu = menu.separator().item(label.clone(), format!("action:{key}"));
        }
        menu::context_menu(cx, menu, e.position, chosen);
        cx.stop_propagation();
    })
}

/// The one set apart (`apart`: a Delete) taken out of `actions`, to lead
/// the row at its start, if another stays to be the one a person reaches
/// for; alone (the others gone with the app that sent them), it's no
/// button at all, only in the card's menu: as a button it would sit
/// where that one goes.
fn take_apart(actions: &mut Vec<(String, String)>, apart: Option<&str>) -> Option<(String, String)> {
    let at = apart.and_then(|a| actions.iter().position(|(k, _)| k == a))?;
    let lead = actions.remove(at);
    (!actions.is_empty()).then_some(lead)
}

/// The height a card's actions take, with the gap above them.
fn buttons_height(buttons: &[(String, String, f32)], small: bool) -> f32 {
    match buttons.len() {
        0 => 0.0,
        _ if small => BUTTON_GAP + SMALL_BUTTON_HEIGHT,
        _ => BUTTON_GAP + m::CONTROL_HEIGHT,
    }
}

/// The file a note's picture, dragged out, is: by its own name a picture
/// or a video (`NoteLook::dragged`), still the file the note came with
/// (another given its name since isn't this note's to hand on), and a
/// file of its own, not a link to another one.
pub fn draggable(look: &NoteLook) -> Option<PathBuf> {
    let (path, id) = look.dragged()?;
    let path = PathBuf::from(path);
    // (Its own entry, not a link, and that entry the file it came with.)
    let m = std::fs::symlink_metadata(&path).ok()?;
    (m.file_type().is_file() && FileId::of_meta(&m) == id).then_some(path)
}

impl Banners {
    pub fn new(shared: Shared, _: &mut Cx<Self>) -> Banners {
        Banners {
            shared,
            faces: HashMap::new(),
            height: 0.0,
            keyboard: false,
            bodies: Rc::default(),
            images: Rc::default(),
            close_hover: None,
            replies: HashMap::new(),
            picture_lit: None,
            press: None,
            dragging: None,
        }
    }

    /// A press on card `id`'s picture moved far enough: its file dragged
    /// out (`dnd::files`: a folder copies it, a chat or a mail attaches
    /// it), its picture under the pointer (made with the one shown, on
    /// its worker; the kind's icon and its name where that isn't in yet).
    /// The card stays while it's dragged.
    fn drag(&mut self, id: u32, cx: &mut Cx<Self>) {
        self.press = None;
        let Some(look) = self.shared.lock().unwrap().cards.iter().find(|c| c.note.id == id).map(|c| c.note.look.clone()) else { return };
        // (Known from its look-over as the pointer came: no file system
        // here, on the thread that takes the pointer.)
        let Some(path) = pictures::checked_file(&look) else { return };
        let mut drag = dnd::files(std::slice::from_ref(&path), true, false);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // (A file of its own beside its picture: a recording's video.)
        let icon = if look.file.as_deref().is_some_and(|f| Some(f) != look.image.as_deref()) { "video-x-generic" } else { "image-x-generic" };
        drag.icon = look::large_source(&look).and_then(|s| pictures::drag_image(&s, cx.scale_factor())).or_else(|| dnd::picture(cx, icon, 48.0, Some(&name), point(px(24.0), px(24.0))));
        self.dragging = Some(id);
        with(&self.shared, |n| n.drag(Some(id)));
        dnd::start(cx, drag, move |action| Told::Dropped(id, action));
    }

    /// The cards that show, laid out and grouped: as many as fit on the
    /// screen (one, at least), grouped by app in the order the apps first
    /// show, oldest first in each; and the height they take.
    fn arrange(&mut self, cx: &WindowCx) -> (Vec<Group>, usize, f32) {
        let p = theme::palette();
        let room = |several: bool| look::Room {
            width: WIDTH - 2.0 * INSET,
            title_rows: 2,
            body_rows: BODY_ROWS,
            title_reserve: if several { look::CLOSE + 8.0 } else { 0.0 },
            picture: ICON,
            image_max: IMAGE_MOST,
            fields_max: 4,
            image_start: false,
            well: false,
        };
        let scale = (cx.scale_factor() * 1000.0).round() as u32;
        let most = olo_kit::display_height(cx) - 2.0 * SCREEN_EDGE;
        let shared = self.shared.clone();
        let app = shared.lock().unwrap();
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
                actions: buttons(card),
                several: cards.iter().filter(|c| c.note.app_name == card.note.app_name).count() > 1,
            };
            let id = card.note.id;
            seen.push(id);
            let (laid, row) = match self.faces.get(&id) {
                Some((k, laid, row)) if *k == key => (laid.clone(), row.clone()),
                _ => {
                    let face = look::Face {
                        title: &card.note.summary,
                        body: &card.note.body,
                        look: &card.note.look,
                        progress: card.note.progress,
                        picture: card.picture.clone(),
                        failed: key.failed,
                        lit: false,
                    };
                    let laid = look::lay(cx, &face, &ink, &room(key.several));
                    let row = lay_buttons(cx, key.actions.clone(), card.note.look.apart.as_deref());
                    self.faces.insert(id, (key, laid.clone(), row.clone()));
                    (laid, row)
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
                row,
                actions: menu_actions(card),
                reply: card.note.reply.clone(),
                pinned: app.history.newest(id).is_some_and(|e| e.pinned),
                has_default: card.note.actions.iter().any(|(k, _)| k == "default"),
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
                    None => groups.push(Group {
                        app: app_name.clone(),
                        frame: Bounds::default(),
                        cards: vec![card.clone()],
                        desktop_entry: desktop_entry.clone(),
                        icon: icon.clone(),
                        accent: *accent,
                        via: *via,
                        source: source.clone(),
                    }),
                }
            }
            let mut y = 0.0;
            for g in &mut groups {
                let top = y;
                y += HEADER;
                for c in &mut g.cards {
                    let mut h = TOP + c.laid.height;
                    if c.reply.is_some() {
                        h += BUTTON_GAP + m::CONTROL_HEIGHT;
                    }
                    h += buttons_height(&c.row.buttons, c.row.small) + INSET;
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

    fn did(&mut self, id: u32, did: Did, cx: &mut Cx<Self>) {
        let shared = self.shared.clone();
        with(&shared, |n| match did {
            // (Its ×: cleared, as × is everywhere.)
            Did::Dismiss => n.clear(id),
            Did::Action(key) => n.invoke(Some(&**cx), id, key),
            // (No action to run: not open at its app, a banner brought
            // back after a restart, or its app gone since. What it's
            // about, if it says how to show that; else its app.)
            Did::Raise => {
                if let Some(card) = n.cards.iter().find(|c| c.note.id == id)
                    && !crate::show_what(card.note.desktop_entry.as_deref(), &card.note.show)
                {
                    raise_app(card.note.desktop_entry.as_deref(), &card.note.app_name);
                }
                n.remove(id, Reason::Dismissed);
            }
            Did::Reply(text) => n.reply(id, &text),
            Did::Link(url) => {
                n.set_read(id, true);
                std::thread::spawn(move || {
                    if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::parse(&url)) {
                        tracing::warn!("notifications: can't open {url}: {e}");
                    }
                });
            }
        });
    }

    /// The link under `pos` in card `id`'s body.
    fn link_at(&self, id: u32, pos: Point<Pixels>) -> Option<String> {
        let bodies = self.bodies.borrow();
        let (origin, text) = bodies.get(&id)?;
        let at = text.index_at(pos - *origin)?;
        let app = self.shared.lock().unwrap();
        let card = app.cards.iter().find(|c| c.note.id == id)?;
        look::link_at(&card.note.body, at)
    }

    /// Card `id`'s reply field, made the first time the card shows one.
    fn ensure_reply(&mut self, id: u32, hint: &str, cx: &mut Cx<Self>) {
        if self.replies.contains_key(&id) {
            return;
        }
        let hint = hint.to_owned();
        let field = text_field(cx, move |f| f.name("Reply").hint(hint));
        cx.subscribe(&field, move |me: &mut Banners, e: &TextFieldEvent, cx| {
            let Some(text) = me.replies.get(&id).map(|f| f.read(cx).text().trim().to_owned()) else { return };
            match e {
                // Return sends, as Send does.
                TextFieldEvent::Entered if !text.is_empty() => me.did(id, Did::Reply(text), cx),
                // A reply begun: the card stays; emptied, it has its
                // time again. (Send follows what's typed.)
                TextFieldEvent::Changed => {
                    with(&me.shared, |n| n.typed(id, text.is_empty()));
                    cx.invalidate();
                }
                _ => {}
            }
        });
        self.replies.insert(id, field);
    }
}

impl View for Banners {
    type Message = Told;

    fn message(&mut self, told: Told, cx: &mut Cx<Self>) {
        match told {
            Told::Changed => {}
            // (One of its actions, chosen from its menu: as its button.)
            Told::Command(id, command) => match command.strip_prefix("action:") {
                Some(key) => self.did(id, Did::Action(key.to_owned()), cx),
                None => with(&self.shared, |n| n.note_command(id, &command)),
            },
            // Once drawn as they are now, and that drawing on the screen:
            // the second frame after it was drawn (the first may be the
            // one before, called for already when the drawing went to
            // the display). Asked for here, before this turn's drawing is
            // sent, so that the drawing asks the display for a frame: from
            // `after_flush`, it went without, and with nothing to draw
            // again (a card of the same height taking the place of the one
            // put away) none came, and the put-away waited for good. Then,
            // put away to nothing, the surface goes (`shown`).
            Told::Shown(tag) => {
                let shared = self.shared.clone();
                cx.on_next_frame(move |cx| {
                    cx.on_next_frame(move |_| with(&shared, |n| n.shown(tag)));
                });
            }
            Told::Dropped(id, action) => {
                self.dragging = None;
                self.picture_lit = None;
                with(&self.shared, |n| match action {
                    Some(_) => n.dropped(id),
                    None => n.drag(None),
                });
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let (groups, fits, y) = self.arrange(cx);
        // Those coming on screen have their time from now.
        let (hovered, more) = {
            let mut n = self.shared.lock().unwrap();
            if fits > n.fits {
                let from = n.fits;
                n.wake(from..fits);
            }
            n.fits = fits;
            (n.hovered, n.cards.len().saturating_sub(fits))
        };
        // As tall as they are (and a line under them while more wait
        // their turn than fit: they're in the list and on the bell, and
        // this says so where the person is looking); the keyboard only
        // while there is something to type into.
        let more_at = y;
        let total = (y - GAP + if more > 0 { GAP + MORE } else { 0.0 }).max(1.0).ceil();
        if total != self.height {
            self.height = total;
            cx.resize_window(size(px(WIDTH), px(total)));
        }
        let wants_keys = groups.iter().flat_map(|g| &g.cards).any(|c| c.reply.is_some());
        if wants_keys != self.keyboard {
            self.keyboard = wants_keys;
            cx.set_layer_keyboard(if wants_keys { LayerKeyboard::OnDemand } else { LayerKeyboard::None });
        }
        // Reply fields go with their cards.
        let ids: Vec<u32> = groups.iter().flat_map(|g| &g.cards).filter(|c| c.reply.is_some()).map(|c| c.id).collect();
        self.replies.retain(|id, _| ids.contains(id));

        let close_hover = self.close_hover;
        let bodies = self.bodies.clone();
        let images = self.images.clone();
        let picture_lit = self.picture_lit;
        let dragging = self.dragging;
        let painted = groups.clone();
        let paint = canvas(move |g, b| {
            let p = theme::palette();
            let origin = b.origin;
            bodies.borrow_mut().clear();
            images.borrow_mut().clear();
            for group in &painted {
                let frame = group.frame + origin;
                // (A banner is a card on a surface as big as itself, its
                // words set in by the card's own measure, `INSET`: not a
                // window's panel, whose inset the layout review would
                // hold its title and close box to.)
                olo_kit::review::own_grid(g, frame);
                g.paint_quad(fill(frame, style::c(p.panel_background)));
                // A picture dragged out of its only card: the whole toast
                // tinted, its header too, as a History card is (its picture
                // and buttons drawn over it as they are).
                let single_dragged = group.cards.len() == 1 && dragging == Some(group.cards[0].id);
                if single_dragged {
                    g.paint_quad(fill(style::inset(g, frame, 1.0), drag_tint(&p)));
                }
                style::frame_rect(g, frame, style::tint(p.panel_background, 1.6));
                style::raised(g, style::inset(g, frame, 1.0), &p);
                // The group's title: the app's icon and name (and, for a
                // website, its address) and the close box, for all of them.
                let head = Bounds::new(frame.origin, size(px(WIDTH), px(HEADER)));
                let mid = style::snap_coord(g, head.center().y);
                // (The icon's drawing and the ×'s ends on the words' edges.)
                let small = Bounds::new(point(head.left() + px(INSET - ICON_MARGIN), mid - px(8.0)), size(px(16.0), px(16.0)));
                let names = crate::named_icons(group.icon.as_deref(), group.desktop_entry.as_deref(), &group.app);
                paint_mark(g, small, &group.app, group.accent, group.via, &names);
                let name = if group.app.is_empty() { "Notification" } else { group.app.as_str() };
                let mut close_room = look::CLOSE + 8.0;
                let small_font = style::ui_font(FontWeight::NORMAL);
                // Pinned (all of them): it says so, by the close box.
                if group.cards.iter().all(|c| c.pinned) {
                    let r = style::label(g, head.right() - px(INSET + close_room), mid, Align::Right, "Pinned", style::DETAIL_SIZE, small_font.clone(), style::disabled_text(&p));
                    agent::record(g, agent::Node::label("Pinned", r));
                    close_room += f32::from(r.size.width) + 8.0;
                }
                // Whose they are, quietly: its title is what's bold (two
                // bold lines, one over the other, and the eye went first
                // to the least of them).
                let name_x = small.right() + px(6.0);
                let n =
                    style::label_fit(g, name_x, mid, Align::Left, name.to_owned(), style::DETAIL_SIZE, small_font.clone(), style::secondary_text(&p), head.right() - px(INSET + close_room) - name_x);
                agent::record(g, agent::Node::label(name.to_owned(), n));
                if let Some(src) = &group.source {
                    let x = n.right() + px(5.0);
                    let room = head.right() - px(INSET + close_room) - x;
                    if room > px(24.0) {
                        style::label_fit(g, x, mid, Align::Left, src.clone(), style::DETAIL_SIZE, small_font.clone(), style::disabled_text(&p), room);
                    }
                }
                let lit = close_hover == Some((group.cards[0].id, false));
                close_mark(g, point(head.right() - px(INSET + look::CLOSE / 2.0), mid), if lit { style::c(p.panel_text) } else { style::secondary_text(&p) });
                style::hline_above(g, frame.left() + px(2.0), frame.right() - px(2.0), head.bottom(), style::tint(p.panel_background, 1.25));
                let single = group.cards.len() == 1;
                for (k, c) in group.cards.iter().enumerate() {
                    let rect = c.rect + origin;
                    if k > 0 {
                        style::hline(g, rect.left() + px(INSET), rect.right() - px(INSET), rect.top(), style::tint(p.panel_background, 1.18));
                        // (Right under it: a point lower left a device
                        // pixel between them at 1.5×.)
                        let under = style::snap_coord(g, rect.top()) + style::hairline(g);
                        style::hline(g, rect.left() + px(INSET), rect.right() - px(INSET), under, style::c(p.shine));
                    }
                    // A picture dragged out of one of several: that card
                    // tinted while it's dragged (one alone: the whole toast,
                    // above).
                    if dragging == Some(c.id) && !single_dragged {
                        g.paint_quad(fill(style::inset(g, rect, 2.0), drag_tint(&p)));
                    }
                    // One of several has its own close box too, while
                    // the pointer is over it.
                    if !single && hovered == Some(c.id) {
                        let lit = close_hover == Some((c.id, true));
                        close_mark(g, card_close_at(rect), if lit { style::c(p.panel_text) } else { style::secondary_text(&p) });
                    }
                    // (Which card the pointer is on: only cards' edges
                    // matter.)
                    g.note_pointer_edge(rect);
                    let face = look::Face { title: &c.title, body: &c.body, look: &c.look, progress: c.progress, picture: c.picture.clone(), failed: c.failed, lit: picture_lit == Some(c.id) };
                    let at = rect.origin + point(px(INSET), px(TOP));
                    let painted = look::paint(g, &face, &c.laid, &c.ink, at, WIDTH - 2.0 * INSET);
                    // A link's a hand where it is, as the pointer goes
                    // over it, without the app hearing each move.
                    for link in &painted.links {
                        g.set_cursor_style(CursorStyle::PointingHand, *link);
                    }
                    if let Some(body) = painted.body {
                        bodies.borrow_mut().insert(c.id, body);
                    }
                    // A picture that drags out as its file is the open
                    // hand: it can be taken hold of (its frame lighting as
                    // the pointer crosses its edge, which is heard).
                    if let Some(picture) = painted.image {
                        g.set_cursor_style(CursorStyle::OpenHand, picture);
                        g.note_pointer_edge(picture);
                        images.borrow_mut().insert(c.id, picture);
                    }
                }
            }
            // More waiting their turn: a small card of its own, saying so.
            if more > 0 {
                let r = style::snap(g, Bounds::new(point(origin.x, origin.y + px(more_at)), size(px(WIDTH), px(MORE))));
                olo_kit::review::own_grid(g, r);
                g.paint_quad(fill(r, style::c(p.panel_background)));
                style::frame_rect(g, r, style::tint(p.panel_background, 1.6));
                let said = olo_kit::words::count(more as u64, "1 more waiting", "{n} more waiting");
                style::label(g, r.center().x, r.center().y, style::Align::Center, said.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
                agent::record(g, agent::Node::button(olo_kit::tr!("{}: Show Notifications", said), r));
            }
        })
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full();

        let rects: Vec<(u32, Bounds<Pixels>)> = groups.iter().flat_map(|g| g.cards.iter().map(|c| (c.id, c.rect))).collect();
        let mut root = div()
            .id("banners")
            .relative()
            .w(px(WIDTH))
            .h(px(total))
            .child(paint)
            // Which card the pointer is on (it stays while it is): heard
            // as it crosses a card's edge (the drawing notes them).
            .moves_matter_at_edges()
            .on_mouse_move(cx.listener(move |b: &mut Banners, e: &MouseMoveEvent, cx| {
                let pos = e.position;
                let over = rects.iter().find(|(_, r)| r.contains(&pos)).map(|(id, _)| *id);
                with(&b.shared, |n| n.hover(over));
                let lit = b.images.borrow().iter().find(|(_, r)| r.contains(&pos)).map(|(id, _)| *id);
                if lit != b.picture_lit && b.dragging.is_none() {
                    b.picture_lit = lit;
                    // (What a drag of it carries, made now if it isn't
                    // kept, so that it's there when it's taken hold of:
                    // asked for only as the drag began, it wasn't, and the
                    // drag carried the kind's icon instead.)
                    // And its file looked at, on a worker, so that the
                    // drag finds it known (`pictures::checked_file`).
                    if let Some(id) = lit
                        && let Some(look) = b.shared.lock().unwrap().cards.iter().find(|c| c.note.id == id).map(|c| c.note.look.clone())
                    {
                        if let Some(source) = look::large_source(&look) {
                            let _ = pictures::drag_image(&source, cx.scale_factor());
                        }
                        let _ = pictures::checked_file(&look);
                    }
                    cx.invalidate();
                }
            }))
            .on_hover(cx.listener(|b: &mut Banners, inside: &bool, cx| {
                if !*inside {
                    b.close_hover = None;
                    with(&b.shared, |n| n.hover(None));
                    if b.picture_lit.take().is_some() {
                        cx.invalidate();
                    }
                }
            }));
        for g in &groups {
            let single = g.cards.len() == 1;
            // The group's close box: all of them (not those pinned, which
            // stay put; one alone goes all the same).
            let head_mid = f32::from(g.frame.top()) + HEADER / 2.0;
            let all = Bounds::centered_at(point(px(WIDTH - INSET - look::CLOSE / 2.0), px(head_mid)), size(px(18.0), px(18.0)));
            let (first, members): (u32, Vec<(u32, bool)>) = (g.cards[0].id, g.cards.iter().map(|c| (c.id, c.pinned)).collect());
            let name = if g.app.is_empty() { "Notification".to_owned() } else { g.app.clone() };
            root = root.child(close_area(cx, format!("close-all-{first}"), all, if single { "Close".into() } else { olo_kit::tr!("Close All from {}", name) }, (first, false), move |b, cx| {
                for &(id, pinned) in &members {
                    if single || !pinned {
                        b.did(id, Did::Dismiss, cx);
                    }
                }
            }));
            for c in &g.cards {
                let id = c.id;
                // The whole card is the default action; its menu has its
                // actions, and pins or snoozes it; a link in its body
                // opens; its picture, pressed and moved, is dragged out.
                let (has_default, pinned) = (c.has_default, c.pinned);
                let (actions, apart) = (c.actions.clone(), c.look.apart.clone());
                let area = div()
                    .id(format!("card-{id}"))
                    .absolute()
                    .left(c.rect.left())
                    .top(c.rect.top())
                    .w(c.rect.size.width)
                    .h(c.rect.size.height)
                    .on_click(cx.listener(move |b: &mut Banners, e: &ClickEvent, cx| {
                        // (A press that dragged its picture out isn't a
                        // click; nor is a right one, which opens its menu:
                        // the kit hears a right press and release as a
                        // click too, and the card's default action ran as
                        // its menu came up: a screenshot's picture opened.)
                        if b.dragging == Some(id) || !e.standard_click() {
                            return;
                        }
                        let did = match b.link_at(id, e.position()) {
                            Some(url) => Did::Link(url),
                            None if has_default => Did::Action("default".into()),
                            None => Did::Raise,
                        };
                        b.did(id, did, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |b: &mut Banners, e: &MouseDownEvent, _| {
                            let on_picture = b.images.borrow().get(&id).is_some_and(|r| r.contains(&e.position));
                            b.press = on_picture.then_some((id, e.position));
                            // (Its file looked at now, if it isn't known: the
                            // drag, a few points on, finds the answer.)
                            if on_picture && let Some(look) = b.shared.lock().unwrap().cards.iter().find(|c| c.note.id == id).map(|c| c.note.look.clone()) {
                                let _ = pictures::checked_file(&look);
                            }
                        }),
                    )
                    // (Heard wherever the pointer goes while it's held, as
                    // the History's: a brisk drag left, toward the windows,
                    // was off the card by its first move, and never began.)
                    .on_drag_move(cx.listener(move |b: &mut Banners, e: &MouseMoveEvent, cx| {
                        let Some((pressed, at)) = b.press.filter(|(p, _)| *p == id) else { return };
                        if e.pressed_button != Some(MouseButton::Left) {
                            b.press = None;
                        } else if f32::from((e.position - at).x).hypot(f32::from((e.position - at).y)) >= DRAG_AFTER {
                            b.drag(pressed, cx);
                        }
                    }))
                    // (Let go of, anywhere: what was pressed is forgotten, so
                    // a later press elsewhere doesn't drag this picture.)
                    .on_mouse_up(MouseButton::Left, cx.listener(|b: &mut Banners, _, _| b.press = None))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|b: &mut Banners, _, _| b.press = None))
                    .on_mouse_down(MouseButton::Right, card_menu(cx, id, actions.clone(), apart.clone(), pinned));
                root = root.child(area);
                // A name cut short to its line: the whole of it a tip over
                // it (and to agents, the whole of it all along).
                if c.laid.title_cut {
                    let r = c.laid.title_at(c.rect.origin + point(px(INSET), px(TOP)));
                    let (key, full) = (format!("title-{id}"), c.title.clone());
                    root = root.child(
                        div()
                            .id(format!("title-{id}"))
                            .absolute()
                            .left(r.left())
                            .top(r.top())
                            .w(r.size.width)
                            .h(r.size.height)
                            .on_hover(cx.listener(move |_: &mut Banners, inside: &bool, cx| olo_kit::tip::hover(cx, key.clone(), full.clone(), r, *inside))),
                    );
                }
                if !single && hovered == Some(id) {
                    let close = Bounds::centered_at(card_close_at(c.rect), size(px(18.0), px(18.0)));
                    root = root.child(close_area(cx, format!("close-{id}"), close, "Close".into(), (id, true), move |b, cx| b.did(id, Did::Dismiss, cx)));
                }
                // Actions along the bottom, side by side from the right as
                // in an alert (small ones a little closer), the one that
                // stands apart at the start.
                let row = &c.row;
                let (button_h, button_gap) = if row.small { (SMALL_BUTTON_HEIGHT, m::TIGHT) } else { (m::CONTROL_HEIGHT, m::SPACING) };
                let mut by = c.rect.bottom() - px(INSET + m::CONTROL_HEIGHT);
                if !row.buttons.is_empty() {
                    let top = c.rect.bottom() - px(INSET + button_h);
                    let mut bx = c.rect.right() - px(INSET);
                    for (i, (key, label, w)) in row.buttons.iter().enumerate().rev() {
                        let (key, w) = (key.clone(), *w);
                        // (Small first: it sets the least width a small
                        // one has, and the width laid out comes after.)
                        let button = Button::new(label.clone()).id(format!("action-{id}-{key}"));
                        let button = if row.small { button.small() } else { button }.min_width(w);
                        // (Pressed with the primary button, or the
                        // keyboard: a right click on Delete, meant for
                        // the card's menu, deletes nothing.)
                        let button = button.on_click(cx.listener(move |b: &mut Banners, e: &ClickEvent, cx| {
                            if e.standard_click() {
                                b.did(id, Did::Action(key.clone()), cx);
                            }
                        }));
                        let left = if i == 0 && row.apart { c.rect.left() + px(INSET) } else { bx - px(w) };
                        // (A right press on it opens the card's menu: it
                        // keeps the card under it from the pointer, and
                        // the menu, opened there, didn't come.)
                        let menu = card_menu(cx, id, actions.clone(), apart.clone(), pinned);
                        root = root.child(div().absolute().left(left).top(top).w(px(w)).occlude().on_mouse_down(MouseButton::Right, menu).child(button));
                        bx -= px(w + button_gap);
                    }
                    by = top - px(m::CONTROL_HEIGHT + BUTTON_GAP);
                }
                // The reply field and its Send button.
                if let Some(hint) = &c.reply {
                    self.ensure_reply(id, hint, cx);
                    let Some(field) = self.replies.get(&id) else { continue };
                    let x = c.rect.left() + px(INSET);
                    let send_w = m::BUTTON_MIN_WIDTH;
                    let field_w = f32::from(c.rect.right() - x) - INSET - send_w - m::SPACING;
                    let typed = !field.read(cx).text().trim().is_empty();
                    // (Controls keep the card under them from the pointer.)
                    root = root.child(div().absolute().left(x).top(by).occlude().child(TextControl::new(cx, field).width(field_w)));
                    let send = Button::new("Send").id(format!("send-{id}")).min_width(send_w).enabled(typed).on_click(cx.listener(move |b: &mut Banners, e: &ClickEvent, cx| {
                        if !e.standard_click() {
                            return;
                        }
                        let Some(text) = b.replies.get(&id).map(|f| f.read(cx).text().trim().to_owned()) else { return };
                        if !text.is_empty() {
                            b.did(id, Did::Reply(text), cx);
                        }
                    }));
                    root = root.child(div().absolute().left(c.rect.right() - px(INSET + send_w)).top(by).occlude().child(send));
                }
            }
        }
        // (The line of those waiting opens the list.)
        if more > 0 {
            root = root.child(div().id("more").absolute().left(px(0.0)).top(px(more_at)).w(px(WIDTH)).h(px(MORE)).on_click(cx.listener(|b: &mut Banners, e: &ClickEvent, _| {
                if e.standard_click() {
                    with(&b.shared, |n| n.open_history());
                }
            })));
        }
        root
    }
}

/// A close box's place: it lights under the pointer and closes on a
/// click (`what` for agents).
fn close_area(cx: &Cx<Banners>, id: String, r: Bounds<Pixels>, what: String, key: (u32, bool), on_click: impl Fn(&mut Banners, &mut Cx<Banners>) + 'static) -> Element {
    div()
        .id(id)
        .absolute()
        .left(r.left())
        .top(r.top())
        .w(r.size.width)
        .h(r.size.height)
        .occlude()
        .child(canvas(move |g, b| agent::record(g, agent::Node::button(what.clone(), b))).size_full())
        .on_hover(cx.listener(move |b: &mut Banners, inside: &bool, cx| {
            let now = if *inside { Some(key) } else { None };
            if b.close_hover != now && (*inside || b.close_hover == Some(key)) {
                b.close_hover = now;
                cx.invalidate();
            }
        }))
        .on_click(cx.listener(move |b: &mut Banners, e: &ClickEvent, cx| {
            // (A right click closes nothing.)
            if e.standard_click() {
                on_click(b, cx);
            }
            cx.stop_propagation();
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one set apart (a Delete) leads the row only beside another action; left alone (its app gone, and
    /// the others with it), it's no button, only in the card's menu, never where the one reached for goes.
    #[test]
    fn a_delete_alone_is_no_button() {
        let a = |keys: &[&str]| keys.iter().map(|k| (k.to_string(), k.to_uppercase())).collect::<Vec<_>>();
        let mut actions = a(&["copy", "delete"]);
        assert_eq!(take_apart(&mut actions, Some("delete")), Some(("delete".into(), "DELETE".into())));
        assert_eq!(actions, a(&["copy"]));
        let mut alone = a(&["delete"]);
        assert_eq!((take_apart(&mut alone, Some("delete")), alone.len()), (None, 0), "a Delete alone stays in the menu");
        let mut none_apart = a(&["copy", "show"]);
        assert_eq!((take_apart(&mut none_apart, None), none_apart.len()), (None, 2));
        assert_eq!((take_apart(&mut none_apart, Some("delete")), none_apart.len()), (None, 2));
    }
}
