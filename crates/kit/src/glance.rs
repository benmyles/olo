//! A glance drawn: what an app gives Deskbar to show of it
//! (`olo_protocol::glance`), the same wherever it's shown (Deskbar's row,
//! a cell beside other widgets, the tray, a preview).
//!
//! - [`height`]: how tall it is at a size, from what it has.
//! - [`paint`]: at a size in a row: small, its headline on one line;
//!   medium or large, the headline (an icon, the value big, the title
//!   after it, the detail under), a note, then its body: slots across
//!   (each a label over an icon over a value, and its level as a small
//!   column in the body's hue), or spans down (each a label, an icon, and
//!   its low to its high as a bar on one scale, coloured cold to warm).
//! - [`paint_cell`]: narrow, beside others: its icon and value, its
//!   title under. [`paint_tray`]: an icon's square.
//!
//! Everything said is recorded for agents as it's drawn (a label each
//! line, a list item each slot and span), so they read it as it shows.
//! Positions are on whole device pixels at every scale (icons through
//! `style::snap_centred`, at their device size: never resampled).

use olo_interface::{Bounds, ContentMask, FontWeight, Hsla, Painter, Pixels, SharedString, fill, linear_color_stop, linear_gradient, point, px};
use olo_protocol::{
    deskbar::Size,
    glance::{Body, Glance, Slot, Span},
};
use olo_theme::{Hue, Palette};

use crate::{
    agent, icons,
    style::{self, Align},
    theme,
};

/// Above the headline, and under the last part.
const PAD: f32 = 4.0;
/// The headline's row, medium and large.
const HEAD: f32 = 40.0;
const HEAD_LARGE: f32 = 50.0;
/// The note's line.
const NOTE: f32 = 16.0;
/// Slots: their label, icon and value (with no icon among them, the label
/// over the value: Kache's hours); and their levels' columns under.
const SLOTS: f32 = 46.0;
const SLOTS_PLAIN: f32 = 30.0;
const LEVELS: f32 = 12.0;

/// How tall slots are: their row, and their levels' if any has one.
fn slots_height(slots: &[Slot]) -> f32 {
    let row = if slots.iter().any(|s| s.icon.is_some()) { SLOTS } else { SLOTS_PLAIN };
    row + if slots.iter().any(|s| s.level.is_some()) { LEVELS } else { 0.0 }
}
/// A span's row.
const SPAN: f32 = 18.0;
/// A small glance's one line.
const LINE: f32 = 24.0;

/// A plain headline's lines (`Glance::plain`): the value and title,
/// and the detail under them where there is one.
const HEAD_PLAIN: f32 = 20.0;
const HEAD_PLAIN_DETAIL: f32 = 15.0;

fn head(size: Size) -> f32 {
    if size == Size::Large { HEAD_LARGE } else { HEAD }
}

/// How tall `gl`'s headline is at `size`.
fn head_of(gl: &Glance, size: Size) -> f32 {
    match (gl.plain, gl.detail.is_empty()) {
        (true, true) => HEAD_PLAIN,
        (true, false) => HEAD_PLAIN + HEAD_PLAIN_DETAIL,
        (false, _) => head(size),
    }
}

/// How tall `glance` is drawn at `size` (one not come yet: its headline's).
pub fn height(glance: Option<&Glance>, size: Size) -> f32 {
    if let Some(Body::Inbox { preview, cards, .. }) = glance.and_then(|gl| gl.body.as_ref()) {
        return inbox_height(cards.len(), *preview, size);
    }
    if size == Size::Small {
        return LINE;
    }
    let Some(gl) = glance else { return PAD + head(size) + PAD };
    let mut h = PAD + head_of(gl, size);
    if gl.note.is_some() {
        h += NOTE;
    }
    match &gl.body {
        Some(Body::Slots { slots, .. }) => h += slots_height(slots),
        Some(Body::Spans { spans, .. }) => h += 2.0 + spans.len() as f32 * SPAN,
        Some(Body::Lines { lines }) => h += 2.0 + lines.len() as f32 * SPAN,
        Some(Body::Inbox { .. }) | None => {}
    }
    h + PAD
}

/// The colours it's drawn in: words, and quieter words still read
/// (`style::secondary_text`, 4.5:1 on the panel).
fn inks(p: &Palette) -> (Hsla, Hsla) {
    (style::c(p.panel_text), style::secondary_text(p))
}

fn said(g: &mut Painter, text: &str, bounds: Bounds<Pixels>) {
    if !text.is_empty() {
        agent::record(g, agent::Node { role: "Label", name: Some(SharedString::from(text.to_owned())), bounds, enabled: true, ..Default::default() });
    }
}

/// Paint `glance` at `size` in row `r`, its words `inset` from each side.
pub fn paint(g: &mut Painter, r: Bounds<Pixels>, glance: &Glance, size: Size, inset: f32, inbox: Inbox) {
    let p = theme::palette();
    let (left, right) = (f32::from(r.left()) + inset, f32::from(r.right()) - inset);
    g.with_content_mask(Some(ContentMask { bounds: r }), |g| {
        if let Some(Body::Inbox { cards, preview, .. }) = &glance.body {
            paint_inbox(g, r, cards, *preview, size, inset, inbox, &p);
            return;
        }
        if size == Size::Small {
            paint_line(g, left, right, f32::from(r.center().y), glance, &p);
            return;
        }
        let mut y = f32::from(r.top()) + PAD;
        if glance.plain {
            paint_plain_head(g, left, right, y, glance, &p);
        } else {
            paint_head(g, left, right, y, glance, size, &p);
        }
        y += head_of(glance, size);
        if let Some(note) = &glance.note {
            let (ink, _) = inks(&p);
            let b = style::label_fit(g, px(left), px(y + NOTE / 2.0), Align::Left, note.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), ink, px(right - left));
            said(g, note, b);
            y += NOTE;
        }
        match &glance.body {
            Some(Body::Slots { slots, hue }) => paint_slots(g, left, right, y, slots, *hue, size, &p),
            Some(Body::Spans { spans, min, max }) => paint_spans(g, left, right, y + 2.0, spans, *min, *max, &p),
            Some(Body::Lines { lines }) => paint_lines(g, left, right, y + 2.0, lines, &p),
            Some(Body::Inbox { .. }) | None => {}
        }
    });
}

/// An inbox (`Body::Inbox`: Notifications' widget, Ben, 2026-11-17: "a
/// stack of cards that you can flip through, with two actions for each
/// one: Dismiss and Snooze ... to rapidly work through them to empty it
/// out"): the card on top is the one to deal with, the edges of two more
/// show under it while there are more; on it, whose it is (its app's
/// icon and name) and which of how many, a dot in the colour of its kind
/// (red: something waiting in an app; blue: a notification), its title
/// and, if asked for, its text; under them, Previous and Next at the
/// left, Snooze and Dismiss at the right, small marks that light under
/// the pointer. Small, it is one line: the dot, the title, Snooze and
/// Dismiss. Empty, it says so in a line.
///
/// Around the stack.
const INBOX_PAD: f32 = 4.0;
/// Inside a card, at its sides and top.
const INBOX_IN: f32 = 7.0;
/// Its lines: whose, the title, each line of text, the buttons.
const INBOX_HEAD: f32 = 16.0;
const INBOX_TITLE: f32 = 19.0;
const INBOX_TEXT: f32 = 15.0;
/// (A point over its buttons' 20 and their 3 below: at 1.25× a card's
/// text came 5.4 points from them, under the least things keep apart.)
const INBOX_BAR: f32 = 25.0;
/// A button on it.
const INBOX_BUTTON: (f32, f32) = (22.0, 20.0);
/// The cards under the top one: how many show, how much of each, and how
/// much narrower each is at either side.
const INBOX_UNDER: usize = 2;
const INBOX_PEEK: f32 = 3.0;
const INBOX_NARROWER: f32 = 4.0;
/// Small: its one card's height.
const INBOX_SMALL: f32 = 26.0;
/// The app's icon on a card, and the dot of its kind.
const INBOX_ICON: f32 = 16.0;
const INBOX_DOT: f32 = 3.5;

/// How an inbox is shown: which card is on top, what the pointer is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inbox {
    pub selected: usize,
    /// The pointer is on the widget.
    pub over: bool,
    pub hot: Option<InboxPart>,
}

/// A button on an inbox's card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InboxPart {
    Previous,
    Next,
    Snooze,
    Dismiss,
}

impl InboxPart {
    pub fn name(self) -> &'static str {
        match self {
            InboxPart::Previous => "Previous",
            InboxPart::Next => "Next",
            InboxPart::Snooze => "Snooze",
            InboxPart::Dismiss => "Dismiss",
        }
    }

    pub fn named(name: &str) -> Option<InboxPart> {
        [InboxPart::Previous, InboxPart::Next, InboxPart::Snooze, InboxPart::Dismiss].into_iter().find(|p| p.name() == name)
    }
}

/// Lines of a card's text at `size`, if it shows any.
fn inbox_lines(preview: bool, size: Size) -> usize {
    match (preview, size) {
        (false, _) | (_, Size::Small) => 0,
        (true, Size::Large) => 2,
        (true, Size::Medium) => 1,
    }
}

/// The top card's height.
fn inbox_card_height(preview: bool, size: Size) -> f32 {
    if size == Size::Small {
        return INBOX_SMALL;
    }
    INBOX_IN + INBOX_HEAD + INBOX_TITLE + inbox_lines(preview, size) as f32 * INBOX_TEXT + INBOX_BAR + 3.0
}

/// An inbox's height: the same however many cards (room for the ones
/// under is kept, so the bar doesn't move as they go), but empty.
fn inbox_height(cards: usize, preview: bool, size: Size) -> f32 {
    if cards == 0 {
        return LINE + PAD;
    }
    let under = if size == Size::Small { 0.0 } else { INBOX_UNDER as f32 * INBOX_PEEK };
    INBOX_PAD + inbox_card_height(preview, size) + under + INBOX_PAD + 2.0
}

/// Where an inbox's top card and its buttons are in row `r` (what's
/// painted, and what Deskbar's clicks land on); None with no card.
/// `waiting`: the top card is what waits in an app, not a notification:
/// it has no × (it can't be cleared: it's there while its app says so),
/// and its one button, where the × would be, puts it off (Later, drawn
/// as Snooze's clock): clicking through a stack at that spot clears the
/// notifications and puts off what waits.
pub fn inbox_laid(r: Bounds<Pixels>, cards: usize, preview: bool, size: Size, inset: f32, waiting: bool) -> Option<(Bounds<Pixels>, Vec<(InboxPart, Bounds<Pixels>)>)> {
    if cards == 0 {
        return None;
    }
    // (The card's sides half way between the row's and its words': room
    // for its edge and shadow, its own words in line with the bar's.)
    let side = (inset / 2.0).max(4.0);
    let card = Bounds::new(point(r.left() + px(side), r.top() + px(INBOX_PAD)), olo_interface::size(r.size.width - px(side * 2.0), px(inbox_card_height(preview, size))));
    let (bw, bh) = INBOX_BUTTON;
    let button = |x: Pixels, cy: Pixels| Bounds::new(point(x, cy - px(bh / 2.0)), olo_interface::size(px(bw), px(bh)));
    let mut buttons = Vec::with_capacity(4);
    let cy = if size == Size::Small { card.center().y } else { card.bottom() - px(3.0 + INBOX_BAR / 2.0) };
    if size != Size::Small && cards > 1 {
        buttons.push((InboxPart::Previous, button(card.left() + px(3.0), cy)));
        buttons.push((InboxPart::Next, button(card.left() + px(3.0 + bw), cy)));
    }
    if waiting {
        buttons.push((InboxPart::Snooze, button(card.right() - px(3.0 + bw), cy)));
    } else {
        buttons.push((InboxPart::Snooze, button(card.right() - px(3.0 + bw * 2.0), cy)));
        buttons.push((InboxPart::Dismiss, button(card.right() - px(3.0 + bw), cy)));
    }
    Some((card, buttons))
}

/// `text` as at most `lines` lines no wider than `width`, broken between
/// words, the last cut short with "…" where it must be.
fn inbox_wrap(g: &mut Painter, text: &str, lines: usize, width: f32, font: &olo_interface::Font, font_size: f32) -> Vec<SharedString> {
    let mut out: Vec<SharedString> = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && out.len() < lines {
        if out.len() + 1 == lines {
            out.push(style::fit(g, SharedString::from(rest.to_owned()), font_size, font, px(width)));
            break;
        }
        // As many whole words as fit.
        let mut end = 0;
        for (i, _) in rest.match_indices(' ').chain(std::iter::once((rest.len(), ""))) {
            if f32::from(style::text_width(rest[..i].to_owned(), font_size, font.clone())) > width {
                break;
            }
            end = i;
        }
        if end == 0 {
            out.push(style::fit(g, SharedString::from(rest.to_owned()), font_size, font, px(width)));
            break;
        }
        out.push(SharedString::from(rest[..end].to_owned()));
        rest = rest[end..].trim_start();
    }
    out
}

/// A button's mark on a card, in its box `b`: lines a little over a
/// device pixel wide, on the pixel grid.
fn inbox_mark(g: &mut Painter, part: InboxPart, b: Bounds<Pixels>, color: Hsla) {
    let c = style::snap_centred(g, b.center(), 12.0, 12.0).center();
    let w = px(1.4);
    let at = |dx: f32, dy: f32| point(c.x + px(dx), c.y + px(dy));
    match part {
        InboxPart::Previous => g.paint_segments(&[(at(1.75, -4.0), at(-2.25, 0.0)), (at(-2.25, 0.0), at(1.75, 4.0))], w, true, color),
        InboxPart::Next => g.paint_segments(&[(at(-1.75, -4.0), at(2.25, 0.0)), (at(2.25, 0.0), at(-1.75, 4.0))], w, true, color),
        // A clock: later.
        InboxPart::Snooze => {
            g.paint_sector(c, px(4.05), px(5.4), 0.0, std::f32::consts::TAU, color);
            g.paint_segments(&[(at(0.0, -2.6), at(0.0, 0.0)), (at(0.0, 0.0), at(2.0, 1.2))], w, true, color);
        }
        InboxPart::Dismiss => g.paint_segments(&[(at(-3.5, -3.5), at(3.5, 3.5)), (at(3.5, -3.5), at(-3.5, 3.5))], w, true, color),
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_inbox(g: &mut Painter, r: Bounds<Pixels>, cards: &[olo_protocol::notifications::InboxCard], preview: bool, size: Size, inset: f32, shown: Inbox, p: &Palette) {
    let top_waits = cards.get(shown.selected.min(cards.len().saturating_sub(1))).is_some_and(|c| c.attention);
    let Some((face, buttons)) = inbox_laid(r, cards.len(), preview, size, inset, top_waits) else {
        // Nothing to deal with: said quietly, where a headline would be.
        let (_, dim) = inks(p);
        let left = f32::from(r.left()) + inset;
        let icon = style::snap_centred(g, point(px(left + INBOX_ICON / 2.0), r.center().y), INBOX_ICON, INBOX_ICON);
        icons::paint_faded(g, icon, "notifications", 0.55);
        let b = style::label_fit(
            g,
            // (Where Deskbar's rows under it start their names: the review
            // found it two points short of them.)
            icon.right() + px(8.0),
            r.center().y,
            Align::Left,
            "Nothing new",
            style::PLAIN_SIZE,
            style::ui_font(FontWeight::NORMAL),
            dim,
            r.right() - px(inset) - icon.right() - px(8.0),
        );
        said(g, "Nothing new", b);
        return;
    };
    let at = shown.selected.min(cards.len() - 1);
    let card = &cards[at];
    let face = style::snap(g, face);
    let ink = style::card_ink(p);
    // The cards under it, the furthest first: an edge of each.
    if size != Size::Small {
        for k in (1..=INBOX_UNDER.min(cards.len() - 1)).rev() {
            let k = k as f32;
            let under =
                Bounds::new(point(face.left() + px(INBOX_NARROWER * k), face.top() + px(INBOX_PEEK * k)), olo_interface::size(face.size.width - px(INBOX_NARROWER * 2.0 * k), face.size.height));
            // (Each a shade further from the top one's paper, so the
            // stack reads on a panel as light as its cards.)
            let paper = olo_theme::mix(style::card_paper(p).1, p.document_text, 0.05 * k);
            let under = style::snap(g, under);
            g.paint_quad(fill(under, style::c(paper)).corner_radii(px(crate::metrics::CARD_RADIUS)).border_widths(style::hairline(g)).border_color(ink.edge));
        }
    }
    style::card(g, face, p, shown.over && shown.hot.is_none(), false);
    let (left, right) = (face.left() + px(INBOX_IN), face.right() - px(INBOX_IN));
    let dot = style::c(if card.attention { p.unread } else { p.notice });
    let kind = if card.attention { "waiting in its app" } else { "a notification" };
    // (A notification that fades away: its dot drains as it goes, as
    // Deskbar's beside its app does. The card itself stays whole.)
    let fresh = card.strength(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    let bold = style::ui_font(FontWeight::BOLD);
    let plain = style::ui_font(FontWeight::NORMAL);
    let who = if card.app.trim().is_empty() { "Notification" } else { card.app.trim() };
    if size == Size::Small {
        // One line: the dot, the title, Snooze and Dismiss.
        let cy = face.center().y;
        style::draining_dot(g, style::snap_centred(g, point(left + px(INBOX_DOT), cy), INBOX_DOT * 2.0, INBOX_DOT * 2.0).center(), INBOX_DOT, fresh, dot);
        let x = left + px(INBOX_DOT * 2.0 + 5.0);
        let room = buttons[0].1.left() - px(crate::metrics::TIGHT) - x;
        style::label_fit(g, x, cy, Align::Left, card.title.clone(), style::PLAIN_SIZE, bold, ink.text, room);
    } else {
        // Whose it is, and which of how many.
        let mut y = face.top() + px(INBOX_IN);
        let cy = y + px(INBOX_HEAD / 2.0);
        let icon = style::snap_centred(g, point(left + px(INBOX_ICON / 2.0), cy), INBOX_ICON, INBOX_ICON);
        if !card.icons.iter().any(|n| icons::paint(g, icon, n)) {
            icons::paint(g, icon, "application");
        }
        // (Its words level with the icon where that landed on the pixels.)
        let cy = icon.center().y;
        let mut name_right = right;
        if cards.len() > 1 {
            let count = format!("{} of {}", at + 1, cards.len());
            let b = style::label(g, right, cy, Align::Right, count, style::DETAIL_SIZE, plain.clone(), ink.dim);
            name_right = b.left() - px(6.0);
        }
        let x = icon.right() + px(5.0);
        style::label_fit(g, x, cy, Align::Left, who.to_owned(), style::DETAIL_SIZE, bold.clone(), ink.dim, name_right - x);
        y += px(INBOX_HEAD);
        // What it is: on two lines where one won't hold it and the card
        // shows text (its second line in the first line of the text's
        // place, the card as tall as ever): cut short, "Claude Code needs
        // y…" lost the word that said what to do (Codex's look, 2026-11-19).
        let cy = y + px(INBOX_TITLE / 2.0 + 1.0);
        style::draining_dot(g, style::snap_centred(g, point(left + px(INBOX_DOT), cy), INBOX_DOT * 2.0, INBOX_DOT * 2.0).center(), INBOX_DOT, fresh, dot);
        let x = left + px(INBOX_DOT * 2.0 + 5.0);
        let lines = inbox_lines(preview, size);
        // (Measured as it's drawn: the window's text system.)
        let long = lines > 0 && style::fit(g, card.title.clone().into(), style::PLAIN_SIZE, &bold, right - x).as_ref() != card.title.as_str();
        if long {
            for (i, line) in inbox_wrap(g, &card.title, 2, f32::from(right - x), &bold, style::PLAIN_SIZE).into_iter().enumerate() {
                style::label(g, x, cy + px((INBOX_TITLE / 2.0 - 1.0 + INBOX_TEXT / 2.0) * i as f32), Align::Left, line, style::PLAIN_SIZE, bold.clone(), ink.text);
            }
        } else {
            style::label_fit(g, x, cy, Align::Left, card.title.clone(), style::PLAIN_SIZE, bold, ink.text, right - x);
        }
        y += px(INBOX_TITLE);
        let taken = usize::from(long);
        for (i, line) in inbox_wrap(g, &card.detail, lines - taken, f32::from(right - left), &plain, style::SMALL_SIZE).into_iter().enumerate() {
            style::label(g, left, y + px(INBOX_TEXT * ((i + taken) as f32 + 0.5)), Align::Left, line, style::SMALL_SIZE, plain.clone(), ink.dim);
        }
    }
    // For agents: the card as one item, its buttons after.
    let words = [who, card.title.as_str(), card.detail.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(": ");
    agent::record(
        g,
        agent::Node {
            role: "ListItem",
            name: Some(SharedString::from(words)),
            value: Some(SharedString::from(format!("{} of {}, {kind}", at + 1, cards.len()))),
            bounds: face,
            enabled: true,
            ..Default::default()
        },
    );
    for (part, b) in buttons {
        let hot = shown.hot == Some(part);
        if hot {
            let plate = style::snap(g, b);
            g.paint_quad(fill(plate, ink.well).corner_radii(px(4.0)));
        }
        inbox_mark(g, part, b, if hot { ink.text } else { ink.dim });
        // (What waits in an app is put off, not snoozed for a time.)
        agent::record(g, agent::Node::button(if card.attention && part == InboxPart::Snooze { "Later" } else { part.name() }, b));
    }
}

fn paint_head(g: &mut Painter, left: f32, right: f32, top: f32, gl: &Glance, size: Size, p: &Palette) {
    let (ink, dim) = (inks(p).0, inks(p).1);
    let (icon_size, value_size) = if size == Size::Large { (40.0, 26.0) } else { (32.0, 21.0) };
    let h = head(size);
    let mut x = left;
    if let Some(icon) = &gl.icon {
        let b = style::snap_centred(g, point(px(left + icon_size / 2.0), px(top + h / 2.0)), icon_size, icon_size);
        icons::paint(g, b, icon);
        x += icon_size + 6.0;
    }
    // Two lines: the value and title, the detail under.
    let first = top + h / 2.0 - if size == Size::Large { 8.0 } else { 7.0 };
    let second = top + h / 2.0 + if size == Size::Large { 13.0 } else { 11.0 };
    let bold = style::ui_font(FontWeight::BOLD);
    let plain = style::ui_font(FontWeight::NORMAL);
    let mut after = x;
    if !gl.value.is_empty() {
        let b = style::label_fit(g, px(x), px(first), Align::Left, gl.value.clone(), value_size, bold.clone(), ink, px((right - x).max(0.0)));
        said(g, &gl.value, b);
        after = f32::from(b.right()) + 5.0;
    }
    if !gl.title.is_empty() && after < right {
        // (On the value's baseline, not its middle: smaller words beside
        // big figures.)
        let y = first + (value_size - style::PLAIN_SIZE) * 0.36;
        let b = style::label_fit(g, px(after), px(y), Align::Left, gl.title.clone(), style::PLAIN_SIZE, bold, ink, px(right - after));
        said(g, &gl.title, b);
    }
    if !gl.detail.is_empty() {
        let b = style::label_fit(g, px(x), px(second), Align::Left, gl.detail.clone(), style::SMALL_SIZE, plain, dim, px((right - x).max(0.0)));
        said(g, &gl.detail, b);
    }
}

/// A plain headline (`Glance::plain`): a small icon, the value and the
/// title on a line of ordinary words, the detail quieter under them.
fn paint_plain_head(g: &mut Painter, left: f32, right: f32, top: f32, gl: &Glance, p: &Palette) {
    let (ink, dim) = inks(p);
    let first = top + HEAD_PLAIN / 2.0;
    let mut x = left;
    if let Some(icon) = &gl.icon {
        let b = style::snap_centred(g, point(px(left + 8.0), px(first)), 16.0, 16.0);
        icons::paint(g, b, icon);
        x += 16.0 + 5.0;
    }
    let words = x;
    if !gl.value.is_empty() {
        let b = style::label_fit(g, px(x), px(first), Align::Left, gl.value.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, px((right - x).max(0.0)));
        said(g, &gl.value, b);
        x = f32::from(b.right()) + 5.0;
    }
    if !gl.title.is_empty() && x < right {
        let b = style::label_fit(g, px(x), px(first), Align::Left, gl.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, px(right - x));
        said(g, &gl.title, b);
    }
    if !gl.detail.is_empty() {
        let second = top + HEAD_PLAIN + HEAD_PLAIN_DETAIL / 2.0 - 1.0;
        let b = style::label_fit(g, px(words), px(second), Align::Left, gl.detail.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim, px((right - words).max(0.0)));
        said(g, &gl.detail, b);
    }
}

/// Small: the headline on one line (icon, value, title).
fn paint_line(g: &mut Painter, left: f32, right: f32, cy: f32, gl: &Glance, p: &Palette) {
    let (ink, dim) = inks(p);
    let mut x = left;
    if let Some(icon) = &gl.icon {
        let b = style::snap_centred(g, point(px(left + 8.0), px(cy)), 16.0, 16.0);
        icons::paint(g, b, icon);
        x += 16.0 + 5.0;
    }
    if !gl.value.is_empty() {
        let b = style::label_fit(g, px(x), px(cy), Align::Left, gl.value.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, px((right - x).max(0.0)));
        said(g, &gl.value, b);
        x = f32::from(b.right()) + 5.0;
    }
    if !gl.title.is_empty() && x < right {
        let b = style::label_fit(g, px(x), px(cy), Align::Left, gl.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, px(right - x));
        said(g, &gl.title, b);
    }
}

/// A hue by its name, or the accent.
fn hue_of(p: &Palette, hue: Option<Hue>) -> Hsla {
    style::c(hue.map_or(p.control_highlight, |h| p.hue(h)))
}

/// Slots across: each a label over an icon over a value; their levels'
/// columns under them, on a faint ground (0 shows as nothing on it).
#[allow(clippy::too_many_arguments)]
fn paint_slots(g: &mut Painter, left: f32, right: f32, top: f32, slots: &[Slot], hue: Option<Hue>, size: Size, p: &Palette) {
    if slots.is_empty() {
        return;
    }
    let (ink, dim) = inks(p);
    let plain = style::ui_font(FontWeight::NORMAL);
    let w = (right - left) / slots.len() as f32;
    let icon_size = if size == Size::Large { 24.0 } else { 20.0 };
    let levels = slots.iter().any(|s| s.level.is_some());
    let icons = slots.iter().any(|s| s.icon.is_some());
    let row = if icons { SLOTS } else { SLOTS_PLAIN };
    let colour = hue_of(p, hue);
    let ground = style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.07));
    let s = g.scale_factor();
    for (i, slot) in slots.iter().enumerate() {
        let cx = left + w * (i as f32 + 0.5);
        let cell = Bounds::from_corners(point(px(left + w * i as f32), px(top)), point(px(left + w * (i + 1) as f32), px(top + slots_height(slots))));
        style::label_fit(g, px(cx), px(top + 6.0), Align::Center, slot.label.clone(), style::SMALL_SIZE, plain.clone(), dim, px(w));
        if let Some(icon) = &slot.icon {
            let b = style::snap_centred(g, point(px(cx), px(top + 13.0 + 10.0)), icon_size, icon_size);
            icons::paint(g, b, icon);
        }
        style::label_fit(g, px(cx), px(top + if icons { 39.0 } else { 22.0 }), Align::Center, slot.value.clone(), style::SMALL_SIZE, plain.clone(), ink, px(w));
        let mut value = slot.value.clone();
        if levels {
            // A column as wide as a finger's breadth of the slot, its
            // height the level, rounded at its top.
            let bw = ((w - 8.0).min(12.0) * s).round() / s;
            let bottom = top + row + LEVELS - 2.0;
            let full = LEVELS - 3.0;
            let track = style::snap(g, Bounds::from_corners(point(px(cx - bw / 2.0), px(bottom - full)), point(px(cx + bw / 2.0), px(bottom))));
            g.paint_quad(fill(track, ground).corner_radii(px(1.5)));
            if let Some(level) = slot.level.map(|l| l.clamp(0.0, 1.0)).filter(|l| *l > 0.0) {
                // (At least a device pixel or two: a small chance shows.)
                let h = (full * level).max(2.0 / s);
                let bar = style::snap(g, Bounds::from_corners(point(px(cx - bw / 2.0), px(bottom - h)), point(px(cx + bw / 2.0), px(bottom))));
                g.paint_quad(fill(bar, colour).corner_radii(px(1.5)));
                value = format!("{value}, {}%", (level * 100.0).round());
            }
        }
        agent::record(g, agent::Node { role: "ListItem", name: Some(slot.label.clone().into()), value: Some(value.into()), bounds: cell, enabled: true, ..Default::default() });
    }
}

/// Where `v` lies on the scale from cold (0) to warm (1), as a colour:
/// blue, teal, yellow, orange.
fn warmth(p: &Palette, t: f32) -> olo_theme::Rgba {
    let stops = [(0.0, p.hue(Hue::Blue)), (0.35, p.hue(Hue::Teal)), (0.65, p.hue(Hue::Yellow)), (1.0, p.hue(Hue::Orange))];
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if t <= b {
            return olo_theme::mix(ca, cb, (t - a) / (b - a));
        }
    }
    stops[3].1
}

/// Spans down: each its label, its icon, its low, a bar from its low to
/// its high on the scale, its high.
#[allow(clippy::too_many_arguments)]
fn paint_spans(g: &mut Painter, left: f32, right: f32, top: f32, spans: &[Span], min: f32, max: f32, p: &Palette) {
    let (ink, dim) = inks(p);
    let plain = style::ui_font(FontWeight::NORMAL);
    let bold = style::ui_font(FontWeight::BOLD);
    // The columns: label, icon, low (to its right edge), bar, high.
    let label_w = spans.iter().map(|s| style::text_width_app(&s.label, style::SMALL_SIZE, &bold)).fold(0.0, f32::max).min(40.0) + 4.0;
    let figures = |f: &dyn Fn(&Span) -> &String| spans.iter().map(|s| style::text_width_app(f(s), style::SMALL_SIZE, &plain)).fold(0.0, f32::max);
    let (low_w, high_w) = (figures(&|s| &s.low_text), figures(&|s| &s.high_text));
    let icon_x = left + label_w;
    let low_right = icon_x + 16.0 + 4.0 + low_w;
    let (bar_left, bar_right) = (low_right + 5.0, right - high_w - 5.0);
    let range = (max - min).max(f32::EPSILON);
    let ground = style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.07));
    let s = g.scale_factor();
    let thick = ((4.0 * s).round() / s).max(2.0 / s);
    for (i, span) in spans.iter().enumerate() {
        let cy = top + SPAN * (i as f32 + 0.5);
        let row = Bounds::from_corners(point(px(left), px(top + SPAN * i as f32)), point(px(right), px(top + SPAN * (i + 1) as f32)));
        style::label_fit(g, px(left), px(cy), Align::Left, span.label.clone(), style::SMALL_SIZE, bold.clone(), ink, px(label_w - 2.0));
        if let Some(icon) = &span.icon {
            let b = style::snap_centred(g, point(px(icon_x + 8.0), px(cy)), 16.0, 16.0);
            icons::paint(g, b, icon);
        }
        style::label(g, px(low_right), px(cy), Align::Right, span.low_text.clone(), style::SMALL_SIZE, plain.clone(), dim);
        style::label(g, px(right - high_w), px(cy), Align::Left, span.high_text.clone(), style::SMALL_SIZE, plain.clone(), ink);
        if bar_right > bar_left + 4.0 {
            let track = style::snap_centred(g, point(px((bar_left + bar_right) / 2.0), px(cy)), bar_right - bar_left, thick);
            g.paint_quad(fill(track, ground).corner_radii(px(thick / 2.0)));
            let at = |v: f32| ((v - min) / range).clamp(0.0, 1.0);
            let (a, b) = (at(span.low.min(span.high)), at(span.high.max(span.low)));
            let x0 = f32::from(track.left()) + a * f32::from(track.size.width);
            let x1 = (f32::from(track.left()) + b * f32::from(track.size.width)).max(x0 + thick);
            let seg = style::snap(g, Bounds::from_corners(point(px(x0), track.top()), point(px(x1), track.bottom())));
            let paint = linear_gradient(90.0, linear_color_stop(style::c(warmth(p, a)), 0.0), linear_color_stop(style::c(warmth(p, b)), 1.0));
            g.paint_quad(fill(seg, paint).corner_radii(px(thick / 2.0)));
        }
        let value = format!("{} to {}", span.low_text, span.high_text);
        agent::record(g, agent::Node { role: "ListItem", name: Some(span.label.clone().into()), value: Some(value.into()), bounds: row, enabled: true, ..Default::default() });
    }
}

/// Lines down: each its label (quiet, in a column as wide as the widest),
/// a mark in its colour, and its words as far as there's room.
fn paint_lines(g: &mut Painter, left: f32, right: f32, top: f32, lines: &[olo_protocol::glance::Line], p: &Palette) {
    let (ink, dim) = inks(p);
    let plain = style::ui_font(FontWeight::NORMAL);
    // (The labels' column no wider than half of it: the words keep room.)
    let label_w = lines.iter().map(|l| style::text_width_app(&l.label, style::SMALL_SIZE, &plain)).fold(0.0, f32::max).min((right - left) / 2.0);
    let mark_x = left + label_w + 6.0 + 3.5;
    let text_x = mark_x + 3.5 + 5.0;
    for (i, line) in lines.iter().enumerate() {
        let cy = top + SPAN * (i as f32 + 0.5);
        let row = Bounds::from_corners(point(px(left), px(top + SPAN * i as f32)), point(px(right), px(top + SPAN * (i + 1) as f32)));
        style::label_fit(g, px(left), px(cy), Align::Left, line.label.clone(), style::SMALL_SIZE, plain.clone(), dim, px(label_w));
        let mark = style::snap_centred(g, point(px(mark_x), px(cy)), 7.0, 7.0);
        style::fill_rect(g, mark, hue_of(p, line.hue));
        style::label_fit(g, px(text_x), px(cy), Align::Left, line.text.clone(), style::SMALL_SIZE, plain.clone(), if line.dim { dim } else { ink }, px((right - text_x).max(0.0)));
        agent::record(g, agent::Node { role: "ListItem", name: Some(line.text.clone().into()), value: Some(line.label.clone().into()), bounds: row, enabled: true, ..Default::default() });
    }
}

/// Narrow, beside other widgets: its icon and value on the first line, its
/// title under.
pub fn paint_cell(g: &mut Painter, r: Bounds<Pixels>, glance: &Glance) {
    let p = theme::palette();
    let (ink, dim) = inks(&p);
    let (left, right) = (f32::from(r.left()), f32::from(r.right()));
    let first = f32::from(r.top()) + 4.0 + 7.0;
    g.with_content_mask(Some(ContentMask { bounds: r }), |g| {
        let mut x = left;
        if let Some(icon) = &glance.icon {
            let b = style::snap_centred(g, point(px(left + 8.0), px(first)), 16.0, 16.0);
            icons::paint(g, b, icon);
            x += 16.0 + 4.0;
        }
        let b = style::label_fit(g, px(x), px(first), Align::Left, glance.value.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, px((right - x).max(0.0)));
        said(g, &glance.value, b);
        let b = style::label_fit(g, px(left), px(first + 15.0), Align::Left, glance.title.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim, px(right - left));
        said(g, &glance.title, b);
    });
}

/// In the tray, an icon's square: its icon (its value without one).
pub fn paint_tray(g: &mut Painter, r: Bounds<Pixels>, glance: &Glance) {
    match &glance.icon {
        Some(icon) => {
            let side = f32::from(r.size.width.min(r.size.height));
            let b = style::snap_centred(g, r.center(), side, side);
            icons::paint(g, b, icon);
        }
        None => {
            let (ink, _) = inks(&theme::palette());
            style::label(g, r.center().x, r.center().y, Align::Center, glance.value.clone(), 9.0, style::ui_font(FontWeight::BOLD), ink);
        }
    }
}

/// Run as app `app`'s provider (`olo_protocol::glance`: no window; Deskbar
/// starts it), until Deskbar asks for nothing or its folder goes: each
/// widget asked for made with `make` (all at once: they share what's
/// fetched) and written where Deskbar reads it, only when it changed;
/// then asleep until the requests change, a path in `watched` does (the
/// app's settings, its cache), or the time `make` gave comes. One a
/// folder at a time (a lock): a second ends at once.
///
/// `make` returns each widget's timeline by its id, and when to make them
/// again (None: when something changes).
pub fn serve(
    app: &str,
    watched: Vec<std::path::PathBuf>,
    mut make: impl FnMut(&[olo_protocol::glance::Request]) -> (Vec<(u32, olo_protocol::glance::Timeline)>, Option<std::time::Duration>),
) -> std::process::ExitCode {
    use std::{collections::HashMap, os::fd::AsRawFd as _, sync::mpsc, time::Duration};

    use olo_protocol::glance::{self as g, Requests};
    let dir = g::dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return std::process::ExitCode::FAILURE;
    }
    let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(g::lock_path(&dir, app)) else { return std::process::ExitCode::FAILURE };
    // SAFETY: flock on a file this holds open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return std::process::ExitCode::SUCCESS;
    }
    let _ = std::fs::write(g::lock_path(&dir, app), std::process::id().to_string());
    let (tx, rx) = mpsc::channel::<()>();
    let requests = g::requests_path(&dir, app);
    let tx = std::sync::Mutex::new(tx);
    // (Its own writes in the folder aren't news: the requests, the folder
    // going, and the app's own paths are.)
    let (asked_path, folder) = (requests.clone(), dir.clone());
    // (A glance's words are the person's, in Olo's language: chosen as it
    // starts and again when the settings change, since a glance's process
    // isn't an app that follows them; the settings' folder is watched.)
    let settings = std::env::var_os("OLO_APP_SERVER_SETTINGS").map(std::path::PathBuf::from).unwrap_or_else(|| crate::config_dir().join("app_server.json"));
    let changed_at = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut chosen_for: Option<Option<std::time::SystemTime>> = None;
    let watched: Vec<std::path::PathBuf> = watched.into_iter().chain(settings.parent().map(std::path::Path::to_path_buf)).collect();
    let _watch = olo_kit_base::watch::watch(std::iter::once(dir.clone()).chain(watched), move |changed| {
        if changed.iter().any(|p| *p == asked_path || *p == folder || !p.starts_with(&folder)) {
            let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(());
        }
    });
    let mut written: HashMap<u32, g::Timeline> = HashMap::new();
    loop {
        if !dir.is_dir() {
            break;
        }
        let asked: Requests = std::fs::read(&requests).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if asked.widgets.is_empty() {
            break;
        }
        let now = changed_at(&settings);
        if chosen_for != Some(now) {
            crate::words::choose();
            chosen_for = Some(now);
        }
        let (made, again) = make(&asked.widgets);
        for (id, timeline) in made {
            if written.get(&id) != Some(&timeline) && g::write_json(&g::content_path(&dir, id), &timeline).is_ok() {
                written.insert(id, timeline);
            }
        }
        written.retain(|id, _| asked.widgets.iter().any(|r| r.widget == *id));
        // Asleep until something changes (a burst of changes at once: one
        // look, a moment after the first).
        let woke = match again {
            Some(d) => rx.recv_timeout(d).map_err(|e| matches!(e, mpsc::RecvTimeoutError::Disconnected)),
            None => rx.recv().map_err(|_| true),
        };
        if woke == Err(true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        while rx.try_recv().is_ok() {}
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use olo_protocol::glance::{Body, Glance, Slot, Span};

    use super::*;

    #[test]
    fn heights_follow_what_it_has() {
        let mut gl = Glance { value: "44°".into(), ..Glance::default() };
        assert_eq!(height(None, Size::Small), LINE);
        assert_eq!(height(Some(&gl), Size::Small), LINE, "small: one line, whatever it has");
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + PAD);
        assert_eq!(height(None, Size::Medium), height(Some(&gl), Size::Medium), "not come yet: as tall as its headline");
        gl.note = Some("Rain stopping in 34 minutes".into());
        gl.body = Some(Body::Slots { slots: vec![Slot { label: "14".into(), icon: Some("weather".into()), ..Slot::default() }; 6], hue: None });
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + NOTE + SLOTS + PAD);
        gl.body = Some(Body::Slots { slots: vec![Slot { level: Some(0.4), icon: Some("weather".into()), ..Slot::default() }; 6], hue: None });
        assert_eq!(height(Some(&gl), Size::Large), PAD + HEAD_LARGE + NOTE + SLOTS + LEVELS + PAD);
        // No icon among them: the label over the value.
        gl.body = Some(Body::Slots { slots: vec![Slot { label: "14".into(), value: "17".into(), level: Some(0.6), ..Slot::default() }; 6], hue: None });
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + NOTE + SLOTS_PLAIN + LEVELS + PAD);
        gl.note = None;
        gl.body = Some(Body::Spans { spans: vec![Span::default(); 7], min: 0.0, max: 1.0 });
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + 2.0 + 7.0 * SPAN + PAD);
        gl.body = Some(Body::Lines { lines: vec![olo_protocol::glance::Line::default(); 5] });
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD + 2.0 + 5.0 * SPAN + PAD);
        assert_eq!(height(Some(&gl), Size::Small), LINE);
        // A plain headline: a line of ordinary words, and its detail's
        // under it where it has one; the same at every size but Small.
        gl.plain = true;
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD_PLAIN + 2.0 + 5.0 * SPAN + PAD);
        assert_eq!(height(Some(&gl), Size::Large), height(Some(&gl), Size::Medium));
        gl.detail = "Today · Room 2".into();
        gl.body = None;
        assert_eq!(height(Some(&gl), Size::Medium), PAD + HEAD_PLAIN + HEAD_PLAIN_DETAIL + PAD);
        assert_eq!(height(Some(&gl), Size::Small), LINE);
    }

    #[test]
    fn warmth_runs_blue_to_orange() {
        let p = theme::palette();
        assert_eq!(warmth(&p, 0.0), p.hue(Hue::Blue));
        assert_eq!(warmth(&p, 1.0), p.hue(Hue::Orange));
        assert_eq!(warmth(&p, 2.0), p.hue(Hue::Orange), "past the end: the end's");
    }
}
