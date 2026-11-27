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
    height_for(glance, size, Inbox::default())
}

/// [`height`], knowing an inbox's live state (a post-clear receipt keeps
/// the card's height even while no cards remain).
pub fn height_for(glance: Option<&Glance>, size: Size, inbox: Inbox) -> f32 {
    if let Some(Body::Inbox { preview, cards, .. }) = glance.and_then(|gl| gl.body.as_ref()) {
        return inbox_height_for(!cards.is_empty() || inbox.receipt.is_some(), *preview, size);
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
/// the pointer; and left of Snooze, apart as its own group, Clear All
/// where more than one unpinned notification is in the stack (Ben,
/// 2026-11-27: "add another icon button for clearing all notifications
/// not just the one on top"). Small, it is one line: the dot, the title,
/// Snooze and Dismiss. Empty, it says so in a line.
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
/// Clear All stands this far left of Snooze's plate: its own group
/// ("all" beside "this one"), never sharing Snooze's slot.
const INBOX_ALL_APART: f32 = 12.0;
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

/// What a post-clear receipt card on top of an inbox stack shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxReceipt {
    /// `count` notifications were cleared; `undoing` is true after Undo was
    /// clicked while waiting for the provider's authoritative list to return.
    Cleared { count: usize, undoing: bool },
    /// Clear All cleared nothing (service failure, timeout, or every target
    /// was already gone/pinned); cards were restored.
    NothingCleared,
    /// Undo could not restore the batch (service restarted, failed, or its
    /// entries were already put back or deleted).
    NothingToUndo,
}

/// How an inbox is shown: which card is on top, what the pointer is on,
/// which button is held down, and any post-clear receipt on top of the
/// stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inbox {
    pub selected: usize,
    /// The pointer is on the widget.
    pub over: bool,
    pub hot: Option<InboxPart>,
    /// The button held down while the primary mouse button is pressed.
    pub pressed: Option<InboxPart>,
    /// Post-clear receipt on top of the stack (shown for 8 s with Undo, or
    /// briefly on failure/no-op).
    pub receipt: Option<InboxReceipt>,
}

/// A button on an inbox's card (or its post-clear receipt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InboxPart {
    Previous,
    Next,
    /// Every unpinned notification in the stack cleared, not only the one on top.
    ClearAll,
    Snooze,
    Dismiss,
    /// Undo on the post-clear receipt card.
    Undo,
}

impl InboxPart {
    pub fn name(self) -> &'static str {
        match self {
            InboxPart::Previous => "Previous",
            InboxPart::Next => "Next",
            InboxPart::ClearAll => "Clear All",
            InboxPart::Snooze => "Snooze",
            InboxPart::Dismiss => "Dismiss",
            InboxPart::Undo => "Undo",
        }
    }

    pub fn named(name: &str) -> Option<InboxPart> {
        [InboxPart::Previous, InboxPart::Next, InboxPart::ClearAll, InboxPart::Snooze, InboxPart::Dismiss, InboxPart::Undo].into_iter().find(|p| p.name() == name)
    }
}

/// Whether an inbox of `cards` offers Clear All: every notification card
/// carries pin-aware metadata (`instance != 0`: an older service or glance
/// helper omits `pinned` and `instance`, so Clear All is withheld during a
/// mixed-version upgrade), and more than one card is an unpinned notification
/// (what waits in an app isn't one and can't be cleared, a pinned one stays
/// put, and one alone has its ×).
pub fn inbox_clears_all(cards: &[olo_protocol::notifications::InboxCard]) -> bool {
    if cards.iter().any(|c| !c.attention && c.instance == 0) {
        return false;
    }
    cards.iter().filter(|c| !c.attention && !c.pinned).nth(1).is_some()
}

/// The Clear All action's words (button tooltip and context menu):
/// "Clear All {n} Notifications" (or "Clear Both Notifications" for two)
/// when nothing will remain in the stack, and "Clear {n} Notifications"
/// when pinned notifications or waiting cards will stay behind.
pub fn inbox_clear_all_label(count: usize, some_stay: bool) -> String {
    if some_stay {
        crate::words::count(count as u64, "Clear 1 Notification", "Clear {n} Notifications")
    } else if count == 2 {
        crate::words::tr("Clear Both Notifications").to_owned()
    } else {
        crate::words::count(count as u64, "Clear 1 Notification", "Clear All {n} Notifications")
    }
}

/// The post-clear receipt's header counter text saying what stayed behind
/// under the receipt: "1 pinned", "1 waiting", or "2 kept" when both kinds
/// (or mixed items) remain; `None` when the stack is empty.
pub fn inbox_kept_label(cards: &[olo_protocol::notifications::InboxCard]) -> Option<String> {
    if cards.is_empty() {
        return None;
    }
    let pinned = cards.iter().filter(|c| !c.attention && c.pinned).count();
    let waiting = cards.iter().filter(|c| c.attention).count();
    let n = cards.len() as u64;
    if pinned == cards.len() {
        Some(crate::words::count(n, "1 pinned", "{n} pinned"))
    } else if waiting == cards.len() {
        Some(crate::words::count(n, "1 waiting", "{n} waiting"))
    } else {
        Some(crate::words::count(n, "1 kept", "{n} kept"))
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

/// An inbox's height given only whether a card or post-clear receipt is
/// showing (`has_card_or_receipt`): zero-copy sizing for Deskbar's layout.
pub fn inbox_height_for(has_card_or_receipt: bool, preview: bool, size: Size) -> f32 {
    if !has_card_or_receipt {
        return LINE + PAD;
    }
    let under = if size == Size::Small { 0.0 } else { INBOX_UNDER as f32 * INBOX_PEEK };
    INBOX_PAD + inbox_card_height(preview, size) + under + INBOX_PAD + 2.0
}

/// Where an inbox's top card and its buttons are in row `r` (what's
/// painted, and what Deskbar's clicks land on); None with no card.
/// Every button keeps its own fixed slot from the card's right edge on
/// every kind of card: Dismiss at the right on notifications (left empty
/// on waiting cards, which can't be dismissed), Snooze/Later in its own
/// slot left of Dismiss on every card, and Clear All `INBOX_ALL_APART`
/// (12 pt) left of Snooze on Medium and Large cards whenever more than
/// one unpinned notification is in the stack ([`inbox_clears_all`]) and
/// there is room beside Previous and Next.
pub fn inbox_laid(r: Bounds<Pixels>, cards: usize, preview: bool, size: Size, inset: f32, waiting: bool, clear_all: bool) -> Option<(Bounds<Pixels>, Vec<(InboxPart, Bounds<Pixels>)>)> {
    if cards == 0 {
        return None;
    }
    // (The card's sides half way between the row's and its words': room
    // for its edge and shadow, its own words in line with the bar's.)
    let side = (inset / 2.0).max(4.0);
    let card = Bounds::new(point(r.left() + px(side), r.top() + px(INBOX_PAD)), olo_interface::size(r.size.width - px(side * 2.0), px(inbox_card_height(preview, size))));
    let (bw, bh) = INBOX_BUTTON;
    let button = |x: Pixels, cy: Pixels| Bounds::new(point(x, cy - px(bh / 2.0)), olo_interface::size(px(bw), px(bh)));
    let mut buttons = Vec::with_capacity(5);
    let cy = if size == Size::Small { card.center().y } else { card.bottom() - px(3.0 + INBOX_BAR / 2.0) };
    if size != Size::Small && cards > 1 {
        buttons.push((InboxPart::Previous, button(card.left() + px(3.0), cy)));
        buttons.push((InboxPart::Next, button(card.left() + px(3.0 + bw), cy)));
    }
    // Every button keeps its own distance from the card's right edge on
    // notification and waiting cards alike: Dismiss's slot stays empty on
    // waiting cards, Snooze/Later never shifts, and Clear All stays 12 pt
    // left of Snooze on every card.
    let all_left = card.right() - px(3.0 + bw * 3.0 + INBOX_ALL_APART);
    if clear_all && size != Size::Small && all_left >= card.left() + px(3.0 + bw * 3.0) {
        buttons.push((InboxPart::ClearAll, button(all_left, cy)));
    }
    buttons.push((InboxPart::Snooze, button(card.right() - px(3.0 + bw * 2.0), cy)));
    if !waiting {
        buttons.push((InboxPart::Dismiss, button(card.right() - px(3.0 + bw), cy)));
    }
    Some((card, buttons))
}

/// The vacant Dismiss slot on a waiting card (where Dismiss stands on a
/// notification card, so a rapid second click after dismissing the last blue
/// card above a red one never opens the waiting item underneath). Returns
/// `None` when the stack is empty or the top card is not a waiting card.
pub fn inbox_vacant_dismiss(r: Bounds<Pixels>, cards: usize, preview: bool, size: Size, inset: f32, waiting: bool) -> Option<Bounds<Pixels>> {
    if cards == 0 || !waiting {
        return None;
    }
    let side = (inset / 2.0).max(4.0);
    let card = Bounds::new(point(r.left() + px(side), r.top() + px(INBOX_PAD)), olo_interface::size(r.size.width - px(side * 2.0), px(inbox_card_height(preview, size))));
    let (bw, bh) = INBOX_BUTTON;
    let cy = if size == Size::Small { card.center().y } else { card.bottom() - px(3.0 + INBOX_BAR / 2.0) };
    Some(Bounds::new(point(card.right() - px(3.0 + bw), cy - px(bh / 2.0)), olo_interface::size(px(bw), px(bh))))
}

/// Where an inbox's post-clear receipt card and its Undo button sit in
/// row `r`: the card keeps the cleared card's exact height and bounds,
/// and Undo is sized to fit its translated label in every catalog (the
/// message giving way on Small). On Medium/Large cards Undo's visible
/// frame is inset by `INBOX_IN` (flush with the header counter's right
/// edge and equally inset from the card's bottom).
pub fn inbox_receipt_laid(r: Bounds<Pixels>, preview: bool, size: Size, inset: f32) -> (Bounds<Pixels>, Bounds<Pixels>) {
    inbox_receipt_laid_for(r, preview, size, inset, crate::words::tr("Undo"))
}

fn inbox_receipt_laid_for(r: Bounds<Pixels>, preview: bool, size: Size, inset: f32, label: &str) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let side = (inset / 2.0).max(4.0);
    let card = Bounds::new(point(r.left() + px(side), r.top() + px(INBOX_PAD)), olo_interface::size(r.size.width - px(side * 2.0), px(inbox_card_height(preview, size))));
    let (bw, bh) = INBOX_BUTTON;
    let tw = style::text_width_app(label, style::DETAIL_SIZE, &style::ui_font(FontWeight::NORMAL));
    let edge_inset = if size == Size::Small { 3.0 } else { INBOX_IN };
    let max_w = (f32::from(card.size.width) - edge_inset * 2.0).max(bw * 2.0);
    let w = (tw + 14.0).ceil().clamp(bw * 2.0, max_w);
    let undo_top = if size == Size::Small { card.center().y - px(bh / 2.0) } else { card.bottom() - px(edge_inset + bh) };
    let undo = Bounds::new(point(card.right() - px(edge_inset + w), undo_top), olo_interface::size(px(w), px(bh)));
    (card, undo)
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

/// Paint a rounded sunken pressed plate for an inbox button (foot icons
/// and Undo alike): an outer ring one line wide in the border role
/// (`ink.edge`: Undo's resting outline in light, the card's outline in
/// dark) keeping the hover plate's exact footprint in both looks, with a
/// 1-line shadow on top/left and 1-line highlight (`p.shine`) on
/// bottom/right strictly inside the ring, framing Deskbar's well colour.
fn paint_inbox_sunken(g: &mut Painter, plate: Bounds<Pixels>, radius: Pixels, p: &Palette, ink: style::CardInk) {
    let line = style::hairline(g);
    let bg = p.panel_background;
    let shadow = style::shade(p, bg, 1.25);
    let shine = style::c(p.shine);
    let well = style::shade(p, bg, 1.03);
    g.paint_quad(fill(plate, ink.edge).corner_radii(radius));
    let inside = style::inset(g, plate, 1.0);
    let r1 = (radius - line).max(px(0.0));
    g.paint_quad(fill(inside, shine).corner_radii(r1));
    let tl = Bounds::from_corners(inside.origin, point(inside.right() - line, inside.bottom() - line));
    g.paint_quad(fill(tl, shadow).corner_radii(r1));
    let inner = style::inset(g, plate, 2.0);
    g.paint_quad(fill(inner, well).corner_radii((radius - line * 2.0).max(px(0.0))));
}

/// The hover plate colour for an inbox footer button: in dark mode the
/// card's well lift; in light mode halfway between card paper and the
/// sunken pressed well so it reads clearly on white at every scale.
fn inbox_hover_plate(p: &Palette, ink: style::CardInk) -> Hsla {
    if p.mode == olo_theme::ThemeMode::Dark {
        ink.well
    } else {
        let (_, card) = style::card_paper(p);
        let well = p.shade(p.panel_background, 1.03);
        style::c(olo_theme::mix(card, well, 0.5))
    }
}

/// The Undo text button's hover plate colour on the receipt card: a step
/// stronger than its resting `ink.well` plate in both looks.
fn inbox_undo_hover_plate(p: &Palette, ink: style::CardInk) -> Hsla {
    if p.mode == olo_theme::ThemeMode::Dark {
        let doc = p.document_text;
        style::c([doc[0], doc[1], doc[2], (0.12_f32 * 255.0).round() as u8])
    } else {
        inbox_hover_plate(p, ink)
    }
}

/// Paint the Clear All mark as one opaque shape centred inside `plate` to
/// the half pixel: exact pixel art with no anti-aliasing at 1× (9 × 9) and
/// 1.25× (11 × 11), and a single combined filled path with 1 pt strokes
/// rounded to whole pixels (2 px at 1.5×, 1.75×, 2×) where the × ends
/// strictly flush with the long row's right edge, row 2's top, and row 3's
/// bottom.
fn paint_clear_all_mark(g: &mut Painter, plate: Bounds<Pixels>, color: Hsla) {
    let s = g.scale_factor();
    let px0 = (f32::from(plate.origin.x) * s).round();
    let py0 = (f32::from(plate.origin.y) * s).round();
    let pw = (f32::from(plate.right()) * s).round() - px0;
    let ph = (f32::from(plate.bottom()) * s).round() - py0;

    let quad_px = |g: &mut Painter, ox: f32, oy: f32, x: f32, y: f32, w: f32, h: f32| {
        let b = Bounds::from_corners(point(px((ox + x) / s), px((oy + y) / s)), point(px((ox + x + w) / s), px((oy + y + h) / s)));
        g.paint_quad(fill(b, color));
    };

    if s <= 1.125 {
        // 1×: 9 × 9 pixel art, no anti-aliasing. Rows 1 px at y = 0, 4, 8;
        // long row x = 0..=8, short rows x = 0..=2; 5 × 5 × at x = 4..=8,
        // y = 4..=8 with 1 px diagonals crossing on single pixel (6, 6).
        let (bw, bh) = (9.0, 9.0);
        let ox = px0 + ((pw - bw) / 2.0).floor();
        let oy = py0 + ((ph - bh) / 2.0).floor();
        quad_px(g, ox, oy, 0.0, 0.0, 9.0, 1.0);
        quad_px(g, ox, oy, 0.0, 4.0, 3.0, 1.0);
        quad_px(g, ox, oy, 0.0, 8.0, 3.0, 1.0);
        for (x, y) in [(4, 4), (5, 5), (6, 6), (7, 7), (8, 8), (8, 4), (7, 5), (5, 7), (4, 8)] {
            quad_px(g, ox, oy, x as f32, y as f32, 1.0, 1.0);
        }
        return;
    }

    if s <= 1.375 {
        // 1.25×: 11 × 11 pixel art, no anti-aliasing. Rows 1 px at y = 0, 5, 10;
        // long row 11 px, short rows 4 px; 6 × 6 × at x = 5..=10, y = 5..=10
        // with a 2 × 2 knot at its crossing.
        let (bw, bh) = (11.0, 11.0);
        let ox = px0 + ((pw - bw) / 2.0).floor();
        let oy = py0 + ((ph - bh) / 2.0).floor();
        quad_px(g, ox, oy, 0.0, 0.0, 11.0, 1.0);
        quad_px(g, ox, oy, 0.0, 5.0, 4.0, 1.0);
        quad_px(g, ox, oy, 0.0, 10.0, 4.0, 1.0);
        for (x, y) in [(5, 5), (6, 6), (7, 7), (8, 8), (9, 9), (10, 10), (10, 5), (9, 6), (8, 7), (7, 8), (6, 9), (5, 10)] {
            quad_px(g, ox, oy, x as f32, y as f32, 1.0, 1.0);
        }
        return;
    }

    // 1.5×, 1.75×, 2× (and higher): 1 pt strokes rounded to whole pixels
    // (2 px at 1.5×, 1.75×, 2×), painted as one combined opaque filled path
    // so the crossing never double-blends and the × lands flush with the
    // long row's right end, row 2's top, and row 3's bottom.
    let sw = s.round().max(1.0);
    let pitch = (4.0 * s).round();
    let bh = 2.0 * pitch + sw;
    let bw = bh;
    let short_w = (3.0 * s).floor();
    let x_size = pitch + sw;
    let ox = px0 + ((pw - bw) / 2.0).floor();
    let oy = py0 + ((ph - bh) / 2.0).floor();

    let pt = |x: f32, y: f32| point(px((ox + x) / s), px((oy + y) / s));
    let mut path = olo_interface::PathBuilder::fill();
    let mut add_rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        path.move_to(pt(x0, y0));
        path.line_to(pt(x1, y0));
        path.line_to(pt(x1, y1));
        path.line_to(pt(x0, y1));
        path.close();
    };
    add_rect(0.0, 0.0, bw, sw);
    add_rect(0.0, pitch, short_w, pitch + sw);
    add_rect(0.0, 2.0 * pitch, short_w, 2.0 * pitch + sw);

    let (x0, x1) = (bw - x_size, bw);
    let (y0, y1) = (pitch, bh);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let a = sw / std::f32::consts::SQRT_2;
    // Trace the × as one simple 16-vertex non-self-intersecting outer polygon
    // strictly inside `[x0, x1] × [y0, y1]`, so the crossing is solid under
    // every fill rule with zero double-blending and zero overshoot.
    path.move_to(pt(x0, y0));
    path.line_to(pt(x0 + a, y0));
    path.line_to(pt(cx, cy - a));
    path.line_to(pt(x1 - a, y0));
    path.line_to(pt(x1, y0));
    path.line_to(pt(x1, y0 + a));
    path.line_to(pt(cx + a, cy));
    path.line_to(pt(x1, y1 - a));
    path.line_to(pt(x1, y1));
    path.line_to(pt(x1 - a, y1));
    path.line_to(pt(cx, cy + a));
    path.line_to(pt(x0 + a, y1));
    path.line_to(pt(x0, y1));
    path.line_to(pt(x0, y1 - a));
    path.line_to(pt(cx - a, cy));
    path.line_to(pt(x0, y0 + a));
    path.close();
    if let Ok(built) = path.build() {
        g.paint_path(built, color);
    }
}

/// A button's mark on a card, in its box `b`: lines a little over a
/// device pixel wide, on the pixel grid.
fn inbox_mark(g: &mut Painter, part: InboxPart, b: Bounds<Pixels>, color: Hsla) {
    let box12 = style::snap_centred(g, b.center(), 12.0, 12.0);
    let c = box12.center();
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
        InboxPart::ClearAll => paint_clear_all_mark(g, b, color),
        InboxPart::Undo => {}
    }
}

/// Paint the stack's peeking edges under `face` for `under` cards behind it.
fn paint_inbox_under(g: &mut Painter, face: Bounds<Pixels>, under: usize, p: &Palette, ink: style::CardInk) {
    for k in (1..=INBOX_UNDER.min(under)).rev() {
        let k = k as f32;
        let card = Bounds::new(point(face.left() + px(INBOX_NARROWER * k), face.top() + px(INBOX_PEEK * k)), olo_interface::size(face.size.width - px(INBOX_NARROWER * 2.0 * k), face.size.height));
        // (Each a shade further from the top one's paper, so the
        // stack reads on a panel as light as its cards.)
        let paper = olo_theme::mix(style::card_paper(p).1, p.document_text, 0.05 * k);
        let card = style::snap(g, card);
        g.paint_quad(fill(card, style::c(paper)).corner_radii(px(crate::metrics::CARD_RADIUS)).border_widths(style::hairline(g)).border_color(ink.edge));
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_inbox(g: &mut Painter, r: Bounds<Pixels>, cards: &[olo_protocol::notifications::InboxCard], preview: bool, size: Size, inset: f32, shown: Inbox, p: &Palette) {
    if let Some(receipt) = shown.receipt {
        let (face, undo) = inbox_receipt_laid(r, preview, size, inset);
        let face = style::snap(g, face);
        let ink = style::card_ink(p);
        if size != Size::Small && !cards.is_empty() {
            paint_inbox_under(g, face, cards.len(), p, ink);
        }
        style::card(g, face, p, false, false);
        let (left, right) = (face.left() + px(INBOX_IN), face.right() - px(INBOX_IN));
        let bold = style::ui_font(FontWeight::BOLD);
        let plain = style::ui_font(FontWeight::NORMAL);
        let (msg, show_undo, undoing) = match receipt {
            InboxReceipt::Cleared { count, undoing } => (crate::words::count(count as u64, "1 notification cleared", "{n} notifications cleared"), true, undoing),
            InboxReceipt::NothingCleared => (crate::words::tr("Nothing was cleared").to_owned(), false, false),
            InboxReceipt::NothingToUndo => (crate::words::tr("Nothing to undo").to_owned(), false, false),
        };
        let undo_label = crate::words::tr("Undo");
        let kept_text = inbox_kept_label(cards);
        if size == Size::Small {
            let cy = face.center().y;
            let room = if show_undo { undo.left() - px(crate::metrics::TIGHT) - left } else { right - left };
            style::label_fit(g, left, cy, Align::Left, msg.clone(), style::PLAIN_SIZE, bold, ink.text, room);
        } else {
            // Header row matching a notification card: Inbox icon + "Inbox"
            // on the left, and what stayed ("1 pinned", "1 waiting", "2 kept")
            // right-aligned in the counter slot.
            let head_cy_raw = face.top() + px(INBOX_IN + INBOX_HEAD / 2.0);
            let icon = style::snap_centred(g, point(left + px(INBOX_ICON / 2.0), head_cy_raw), INBOX_ICON, INBOX_ICON);
            icons::paint(g, icon, "notifications");
            let head_cy = icon.center().y;
            let mut name_right = right;
            if let Some(kept) = &kept_text {
                let b = style::label(g, right, head_cy, Align::Right, kept.clone(), style::DETAIL_SIZE, plain.clone(), ink.dim);
                name_right = b.left() - px(6.0);
            }
            let x = icon.right() + px(5.0);
            style::label_fit(g, x, head_cy, Align::Left, crate::words::tr("Inbox").to_owned(), style::DETAIL_SIZE, bold.clone(), ink.dim, name_right - x);
            // Message in the title's style (primary ink, bold, sentence case)
            // on the title's first baseline, wrapping if a translation or large
            // count exceeds one line.
            let title_cy = face.top() + px(INBOX_IN + INBOX_HEAD + INBOX_TITLE / 2.0 + 1.0);
            let lines = inbox_lines(preview, size);
            let long = lines > 0 && style::fit(g, msg.clone().into(), style::PLAIN_SIZE, &bold, right - left).as_ref() != msg.as_str();
            if long {
                for (i, line) in inbox_wrap(g, &msg, 2, f32::from(right - left), &bold, style::PLAIN_SIZE).into_iter().enumerate() {
                    style::label(g, left, title_cy + px((INBOX_TITLE / 2.0 - 1.0 + INBOX_TEXT / 2.0) * i as f32), Align::Left, line, style::PLAIN_SIZE, bold.clone(), ink.text);
                }
            } else {
                style::label_fit(g, left, title_cy, Align::Left, msg.clone(), style::PLAIN_SIZE, bold, ink.text, right - left);
            }
        }
        let remaining = kept_text.map(SharedString::from);
        agent::record(g, agent::Node { role: "ListItem", name: Some(SharedString::from(msg)), value: remaining, bounds: face, enabled: true, ..Default::default() });
        if show_undo {
            let plate = style::snap(g, undo);
            let hot = !undoing && shown.hot == Some(InboxPart::Undo);
            let pressed = !undoing && shown.pressed == Some(InboxPart::Undo) && hot;
            if pressed {
                paint_inbox_sunken(g, plate, px(4.0), p, ink);
            } else {
                let plate_fill = if hot { inbox_undo_hover_plate(p, ink) } else { ink.well };
                g.paint_quad(fill(plate, plate_fill).corner_radii(px(4.0)).border_widths(style::hairline(g)).border_color(ink.edge));
            }
            let off = if pressed { style::hairline(g) } else { px(0.0) };
            let inner_w = (plate.size.width - px(8.0)).max(px(0.0));
            let text_ink = if undoing { ink.dim } else { ink.text };
            style::label_fit(g, plate.center().x, plate.center().y + off, Align::Center, undo_label.to_owned(), style::DETAIL_SIZE, plain, text_ink, inner_w);
            if undoing {
                agent::record(g, agent::Node { role: "Button", name: Some(SharedString::from("Undo")), bounds: undo, enabled: false, ..Default::default() });
            } else {
                agent::record(g, agent::Node::button("Undo", undo));
            }
        }
        return;
    }
    let top_waits = cards.get(shown.selected.min(cards.len().saturating_sub(1))).is_some_and(|c| c.attention);
    let Some((face, buttons)) = inbox_laid(r, cards.len(), preview, size, inset, top_waits, inbox_clears_all(cards)) else {
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
        paint_inbox_under(g, face, cards.len() - 1, p, ink);
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
    // Opaque resolved secondary colour on the card's current surface so
    // intersecting glyph strokes never double-blend at their crossing.
    let card_bg = {
        let (_, base) = style::card_paper(p);
        if shown.over && shown.hot.is_none() { olo_theme::mix(base, p.document_text, 0.025) } else { base }
    };
    let rest_ink = style::c(olo_theme::mix(card_bg, p.document_text, 0.52));
    for (part, b) in buttons {
        let hot = shown.hot == Some(part);
        let pressed = shown.pressed == Some(part) && hot;
        let plate = style::snap(g, b);
        if pressed {
            paint_inbox_sunken(g, plate, px(4.0), p, ink);
        } else if hot {
            g.paint_quad(fill(plate, inbox_hover_plate(p, ink)).corner_radii(px(4.0)));
        }
        inbox_mark(g, part, plate, if hot { ink.text } else { rest_ink });
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
    // (A ring is a little wider than a square: the words start after
    // whichever the lines have.)
    let half = if lines.iter().any(|l| l.ring) { 4.5 } else { 3.5 };
    let mark_x = left + label_w + 6.0 + half;
    let text_x = mark_x + half + 5.0;
    for (i, line) in lines.iter().enumerate() {
        let cy = top + SPAN * (i as f32 + 0.5);
        let row = Bounds::from_corners(point(px(left), px(top + SPAN * i as f32)), point(px(right), px(top + SPAN * (i + 1) as f32)));
        style::label_fit(g, px(left), px(cy), Align::Left, line.label.clone(), style::SMALL_SIZE, plain.clone(), dim, px(label_w));
        if line.ring {
            // (A to-do's: a ring of its colour, its line whole device
            // pixels wide, round a middle on one.)
            let s = g.scale_factor();
            let w = (1.5 * s).round().max(1.0) / s;
            let c = style::snap_centred(g, point(px(mark_x), px(cy)), 9.0, 9.0).center();
            let r = px(4.5 - w / 2.0);
            let mut ring = olo_interface::PathBuilder::stroke(px(w));
            ring.move_to(point(c.x + r, c.y));
            ring.arc_to(point(r, r), px(0.0), false, true, point(c.x - r, c.y));
            ring.arc_to(point(r, r), px(0.0), false, true, point(c.x + r, c.y));
            ring.close();
            if let Ok(path) = ring.build() {
                g.paint_path(path, hue_of(p, line.hue));
            }
        } else {
            let mark = style::snap_centred(g, point(px(mark_x), px(cy)), 7.0, 7.0);
            style::fill_rect(g, mark, hue_of(p, line.hue));
        }
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
/// fetched) and written where Deskbar reads it, only when what it shows
/// changed; then asleep until the requests change, a path in `watched`
/// does (the app's settings, its cache: the files themselves, not their
/// folders, which change for everyone's reasons), or the time `make`
/// gave comes. One a folder at a time (a lock): a second ends at once.
///
/// `make` returns each widget's timeline by its id, and when to make them
/// again (None: when something changes).
pub fn serve(
    app: &str,
    watched: Vec<std::path::PathBuf>,
    make: impl FnMut(&[olo_protocol::glance::Request]) -> (Vec<(u32, olo_protocol::glance::Timeline)>, Option<std::time::Duration>),
) -> std::process::ExitCode {
    serve_following(app, move || watched.clone(), make)
}

/// [`serve`] for an app whose paths come and go (a store that is a
/// folder of folders: a watch of a folder tells of its items, not of
/// theirs): `follow` says which to follow, asked before each making, so
/// what is made is of paths already followed.
pub fn serve_following(
    app: &str,
    follow: impl FnMut() -> Vec<std::path::PathBuf>,
    make: impl FnMut(&[olo_protocol::glance::Request]) -> (Vec<(u32, olo_protocol::glance::Timeline)>, Option<std::time::Duration>),
) -> std::process::ExitCode {
    // (A glance's words are the person's, in Olo's language: chosen as it
    // starts and again when the settings change, since a glance's process
    // isn't an app that follows them.)
    let settings = std::env::var_os("OLO_APP_SERVER_SETTINGS").map(std::path::PathBuf::from).unwrap_or_else(|| crate::config_dir().join("app_server.json"));
    serve_in(&olo_protocol::glance::dir(), &settings, app, follow, make)
}

/// [`serve_following`], in the folder `dir`, with the desktop's settings
/// in the file `settings`.
///
/// What wakes it is its own business only: its requests' file (which
/// goes when the folder does) and the paths it follows. Never the
/// folder: every provider writes its widgets there, and a provider
/// woken by a write there (its own too) made its widgets again for
/// each, wrote any that said when they were made, and woke the others:
/// two of them kept each other at it twelve times a second for as long
/// as Deskbar ran (2026-11-21: Calendar's read every event and asked
/// the keyring each time, a third of a processor).
fn serve_in(
    dir: &std::path::Path,
    settings: &std::path::Path,
    app: &str,
    mut follow: impl FnMut() -> Vec<std::path::PathBuf>,
    mut make: impl FnMut(&[olo_protocol::glance::Request]) -> (Vec<(u32, olo_protocol::glance::Timeline)>, Option<std::time::Duration>),
) -> std::process::ExitCode {
    use std::{collections::HashMap, os::fd::AsRawFd as _, sync::mpsc, time::Duration};

    use olo_protocol::glance::{self as g, Requests};
    if std::fs::create_dir_all(dir).is_err() {
        return std::process::ExitCode::FAILURE;
    }
    let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(g::lock_path(dir, app)) else { return std::process::ExitCode::FAILURE };
    // SAFETY: flock on a file this holds open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return std::process::ExitCode::SUCCESS;
    }
    let _ = std::fs::write(g::lock_path(dir, app), std::process::id().to_string());
    let (tx, rx) = mpsc::channel::<()>();
    let requests = g::requests_path(dir, app);
    let tx = std::sync::Mutex::new(tx);
    let changed_at = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut chosen_for: Option<Option<std::time::SystemTime>> = None;
    let always = [requests.clone(), settings.to_path_buf()];
    let watch = olo_kit_base::watch::watch(always.iter().cloned().chain(follow()), move |_| {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(());
    });
    // What each widget's file says now (what it shows: when it was made
    // is no news to anyone).
    let mut written: HashMap<u32, Vec<g::Entry>> = HashMap::new();
    loop {
        if !dir.is_dir() {
            break;
        }
        let asked: Requests = std::fs::read(&requests).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if asked.widgets.is_empty() {
            break;
        }
        let now = changed_at(settings);
        if chosen_for != Some(now) {
            crate::words::choose();
            chosen_for = Some(now);
        }
        // (What it follows, as it is now: only what differs from before
        // is looked at. Before the making, so that what changes after it
        // read is told.)
        if let Some(w) = &watch {
            w.set(always.iter().cloned().chain(follow()));
        }
        let (made, again) = make(&asked.widgets);
        for (id, timeline) in made {
            let path = g::content_path(dir, id);
            // (Written again too if its file has gone: a Deskbar that
            // cleared the folder reads what is there.)
            if (written.get(&id) != Some(&timeline.entries) || !path.exists()) && g::write_json(&path, &timeline).is_ok() {
                written.insert(id, timeline.entries);
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

    /// Clear All on an inbox's card: there for a stack with more than one
    /// unpinned notification, 12 pt left of Snooze's slot in its own fixed
    /// place on every card (notification or waiting, so a waiting card
    /// arriving on top never moves Clear All under a pointer resting on
    /// Snooze), the other buttons where they were; not on the small card,
    /// not where the card is too narrow for it beside Previous and Next,
    /// not for one unpinned notification beside pinned or waiting cards;
    /// and the post-clear receipt keeps the card height with Undo clear of
    /// Clear All's old slot.
    #[test]
    fn clear_all_stands_left_of_snooze_where_a_stack_has_notifications() {
        use olo_protocol::notifications::InboxCard;
        let note = |id: u32, attention: bool, pinned: bool| InboxCard {
            id,
            app: "Rex".into(),
            title: "Agent finished".into(),
            attention,
            pinned,
            instance: if attention { 0 } else { u64::from(id) },
            ..InboxCard::default()
        };
        assert!(!inbox_clears_all(&[]));
        assert!(!inbox_clears_all(&[note(1, false, false)]), "one: its ×");
        assert!(!inbox_clears_all(&[note(1, true, false), note(2, false, false)]), "one notification and what waits in an app");
        assert!(!inbox_clears_all(&[note(1, false, true), note(2, false, false)]), "one pinned and one ordinary: only one clearable");
        assert!(!inbox_clears_all(&[note(1, false, true), note(2, false, true)]), "two pinned: none clearable");
        assert!(inbox_clears_all(&[note(1, false, false), note(2, false, false)]));
        assert!(inbox_clears_all(&[note(1, false, true), note(2, false, false), note(3, false, false)]), "pinned plus two ordinary");
        assert!(inbox_clears_all(&[note(1, true, false), note(2, false, false), note(3, false, false)]));
        // Older producer that omitted pin metadata (`instance == 0`): Clear All withheld (Astra 17).
        let legacy = |id: u32| InboxCard { id, app: "Rex".into(), title: "Old".into(), attention: false, pinned: false, instance: 0, ..InboxCard::default() };
        assert!(!inbox_clears_all(&[legacy(1), legacy(2)]), "older service without pin metadata must not offer Clear All");
        assert!(!inbox_clears_all(&[legacy(1), note(2, false, false), note(3, false, false)]), "any legacy card without pin metadata withholds Clear All");

        assert_eq!(inbox_clear_all_label(2, false), "Clear Both Notifications");
        assert_eq!(inbox_clear_all_label(3, false), "Clear All 3 Notifications");
        assert_eq!(inbox_clear_all_label(2, true), "Clear 2 Notifications");
        assert_eq!(inbox_clear_all_label(3, true), "Clear 3 Notifications");

        assert_eq!(inbox_kept_label(&[]), None);
        assert_eq!(inbox_kept_label(&[note(1, false, true)]).as_deref(), Some("1 pinned"));
        assert_eq!(inbox_kept_label(&[note(1, false, true), note(2, false, true)]).as_deref(), Some("2 pinned"));
        assert_eq!(inbox_kept_label(&[note(1, true, false)]).as_deref(), Some("1 waiting"));
        assert_eq!(inbox_kept_label(&[note(1, true, false), note(2, true, false)]).as_deref(), Some("2 waiting"));
        assert_eq!(inbox_kept_label(&[note(1, false, true), note(2, true, false)]).as_deref(), Some("2 kept"));

        let row = |width: f32| Bounds::new(point(px(0.0), px(0.0)), olo_interface::size(px(width), px(120.0)));
        let laid = |width: f32, cards: usize, size: Size, waiting: bool, all: bool| inbox_laid(row(width), cards, true, size, 12.0, waiting, all).unwrap().1;
        let at = |buttons: &[(InboxPart, Bounds<Pixels>)], part: InboxPart| buttons.iter().find(|(p, _)| *p == part).map(|(_, b)| *b);
        let (bw, _) = INBOX_BUTTON;

        // Without it: the buttons exactly where they were.
        let before = laid(172.0, 3, Size::Medium, false, false);
        assert_eq!(before.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [InboxPart::Previous, InboxPart::Next, InboxPart::Snooze, InboxPart::Dismiss]);
        // With it: the same four in the same places, and Clear All 12 pt left of Snooze, on their line.
        let with = laid(172.0, 3, Size::Medium, false, true);
        for (part, b) in &before {
            assert_eq!(at(&with, *part), Some(*b), "{part:?} moved");
        }
        let (all, snooze, dismiss, next) =
            (at(&with, InboxPart::ClearAll).unwrap(), at(&with, InboxPart::Snooze).unwrap(), at(&with, InboxPart::Dismiss).unwrap(), at(&with, InboxPart::Next).unwrap());
        assert_eq!(f32::from(snooze.left() - all.right()), 12.0);
        assert_eq!((all.top(), all.size), (snooze.top(), snooze.size));
        assert!(all.left() >= next.right(), "clear of Next");
        // Every button keeps the same distance from the card's right edge on
        // every kind of card: on a waiting card Dismiss's slot stays empty,
        // Snooze/Later keeps Snooze's exact slot, and Clear All stays 12 pt
        // left of Snooze.
        let (card_with, _) = inbox_laid(row(172.0), 3, true, Size::Medium, 12.0, false, true).unwrap();
        let (card_wait, waits) = inbox_laid(row(172.0), 3, true, Size::Medium, 12.0, true, true).unwrap();
        let (card_wait_solo, waits_solo) = inbox_laid(row(172.0), 1, true, Size::Medium, 12.0, true, false).unwrap();
        let (card_note_solo, note_solo) = inbox_laid(row(172.0), 1, true, Size::Medium, 12.0, false, false).unwrap();
        assert_eq!(waits.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [InboxPart::Previous, InboxPart::Next, InboxPart::ClearAll, InboxPart::Snooze]);
        let waits_all = at(&waits, InboxPart::ClearAll).unwrap();
        let waits_later = at(&waits, InboxPart::Snooze).unwrap();
        assert_eq!(card_wait.right() - waits_all.right(), card_with.right() - all.right(), "Clear All distance from right edge invariant");
        assert_eq!(card_wait.right() - waits_later.right(), card_with.right() - snooze.right(), "Snooze/Later distance from right edge invariant");
        assert_eq!(card_wait_solo.right() - at(&waits_solo, InboxPart::Snooze).unwrap().right(), card_with.right() - snooze.right());
        assert_eq!(card_note_solo.right() - at(&note_solo, InboxPart::Snooze).unwrap().right(), card_with.right() - snooze.right());
        assert_eq!(card_note_solo.right() - at(&note_solo, InboxPart::Dismiss).unwrap().right(), card_with.right() - dismiss.right());
        assert_eq!(f32::from(waits_later.left() - waits_all.right()), 12.0, "12 pt gap preserved on waiting cards");
        assert_eq!(at(&waits, InboxPart::Dismiss), None, "Dismiss slot empty on waiting cards");
        assert_eq!(inbox_vacant_dismiss(row(172.0), 3, true, Size::Medium, 12.0, true), Some(dismiss), "vacant Dismiss slot matches notification Dismiss rect");
        assert_eq!(inbox_vacant_dismiss(row(172.0), 3, true, Size::Medium, 12.0, false), None);
        assert_eq!(inbox_vacant_dismiss(row(172.0), 0, true, Size::Medium, 12.0, true), None);
        assert!(!waits_all.contains(&snooze.center()), "stationary pointer on Snooze must not land on Clear All");
        // Small card also keeps Snooze in its own slot on waiting cards.
        let (card_sm_n, sm_n) = inbox_laid(row(172.0), 2, true, Size::Small, 12.0, false, true).unwrap();
        let (card_sm_w, sm_w) = inbox_laid(row(172.0), 2, true, Size::Small, 12.0, true, true).unwrap();
        assert_eq!(card_sm_w.right() - at(&sm_w, InboxPart::Snooze).unwrap().right(), card_sm_n.right() - at(&sm_n, InboxPart::Snooze).unwrap().right());
        assert_eq!(inbox_vacant_dismiss(row(172.0), 2, true, Size::Small, 12.0, true), at(&sm_n, InboxPart::Dismiss));
        // Receipt card keeps exact card bounds & height, with Undo inset by
        // `INBOX_IN` on both right and bottom (matching header counter edge)
        // and clear of Clear All's old slot in English.
        let (receipt_card, undo) = inbox_receipt_laid(row(172.0), true, Size::Medium, 12.0);
        assert_eq!(receipt_card, card_with);
        assert_eq!(f32::from(receipt_card.right() - undo.right()), INBOX_IN, "Undo right edge aligned with content inset");
        assert_eq!(f32::from(receipt_card.bottom() - undo.bottom()), INBOX_IN, "Undo bottom edge aligned with content inset");
        assert!(undo.left() > all.right(), "a second click where Clear All stood must not hit Undo");
        let empty_inbox = Glance { body: Some(Body::Inbox { cards: vec![], snooze_minutes: 15, preview: true }), ..Default::default() };
        let full_inbox = Glance { body: Some(Body::Inbox { cards: vec![note(1, false, false)], snooze_minutes: 15, preview: true }), ..Default::default() };
        assert_eq!(
            height_for(Some(&empty_inbox), Size::Medium, Inbox { receipt: Some(InboxReceipt::Cleared { count: 3, undoing: false }), ..Default::default() }),
            height(Some(&full_inbox), Size::Medium)
        );
        assert_eq!(inbox_height_for(true, true, Size::Medium), height(Some(&full_inbox), Size::Medium));
        assert_eq!(inbox_height_for(false, true, Size::Medium), LINE + PAD);
        assert_eq!(height(Some(&empty_inbox), Size::Medium), LINE + PAD);
        // The small card is one line: never. A card too narrow for it beside Previous and Next: left out, the rest as they were.
        assert_eq!(at(&laid(172.0, 3, Size::Small, false, true), InboxPart::ClearAll), None);
        let narrow = 2.0 * 6.0 + 3.0 + bw * 3.0 + 3.0 + bw * 3.0 + INBOX_ALL_APART - 1.0;
        assert_eq!(laid(narrow, 3, Size::Medium, false, true).iter().map(|(p, _)| *p).collect::<Vec<_>>(), [InboxPart::Previous, InboxPart::Next, InboxPart::Snooze, InboxPart::Dismiss]);
        assert!(at(&laid(narrow + 1.0, 3, Size::Medium, false, true), InboxPart::ClearAll).is_some());
        // By name, as Deskbar's hits and agents have it.
        assert_eq!(InboxPart::named("Clear All"), Some(InboxPart::ClearAll));
        assert_eq!(InboxPart::named("Undo"), Some(InboxPart::Undo));
    }

    /// Undo fits its translated label without truncation in every catalog
    /// (Astra 25: Dutch "Ongedaan maken", German "Widerrufen"), and Russian
    /// preserves the "all" distinction for 21 (Astra 26). Reads catalog files
    /// directly without mutating the process-global language so parallel tests
    /// stay isolated.
    #[test]
    fn undo_fits_all_catalogs_and_russian_plural_keeps_all() {
        let row = Bounds::new(point(px(0.0), px(0.0)), olo_interface::size(px(172.0), px(120.0)));
        let plain = style::ui_font(FontWeight::NORMAL);
        let words_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/words");
        let lookup = |text: &str, key: &str| -> Option<String> {
            for block in text.split("\n\n") {
                let mut lines = block.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
                if lines.next() == Some(key) {
                    return lines.next().map(str::to_owned);
                }
            }
            None
        };
        for lang in &olo_kit_base::words::LANGUAGES {
            let label = if lang.code == "en" {
                "Undo".to_owned()
            } else {
                let text = std::fs::read_to_string(words_dir.join(format!("{}.txt", lang.code))).unwrap();
                lookup(&text, "Undo").unwrap_or_else(|| panic!("{}: missing Undo translation", lang.code))
            };
            let tw = style::text_width_app(&label, style::DETAIL_SIZE, &plain);
            for sz in [Size::Small, Size::Medium, Size::Large] {
                let (card, undo) = inbox_receipt_laid_for(row, true, sz, 12.0, &label);
                let inner_w = f32::from(undo.size.width) - 8.0;
                assert!(inner_w >= tw, "{}: {sz:?} Undo label {label:?} ({tw:.1} pt) truncated in {inner_w:.1} pt", lang.code);
                assert!(undo.left() >= card.left(), "{}: Undo escaped left edge of card", lang.code);
            }
        }
        // Russian 21 (Slavic `one` form) still distinguishes Clear All from partial Clear.
        let ru = std::fs::read_to_string(words_dir.join("ru.txt")).unwrap();
        let all_one = lookup(&ru, "Clear All {n} Notifications").unwrap();
        let some_one = lookup(&ru, "Clear {n} Notifications").unwrap();
        assert_ne!(all_one, some_one, "Russian `one` form (used for 21) must distinguish all vs partial: {all_one:?}");
        assert!(all_one.contains("все"), "Russian all `one` form should say все: {all_one:?}");
    }

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

    /// A provider makes its widgets when it is asked and when a path of
    /// its own changes, and writes one only when what it shows changed:
    /// not for anything else written in the folder (its own widgets,
    /// another provider's widgets and requests), nor for a file beside
    /// one it follows; and it ends when it is asked for nothing.
    #[test]
    fn a_provider_wakes_for_its_own_business_only() {
        use std::{
            os::unix::fs::MetadataExt as _,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            time::{Duration, Instant},
        };

        use olo_protocol::glance::{self as g, Entry, Request, Requests, Timeline};
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-glance-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("glance");
        std::fs::create_dir_all(&dir).unwrap();
        let (own, settings) = (root.join("own.json"), root.join("app_server.json"));
        std::fs::write(&own, "1").unwrap();
        std::fs::write(&settings, "{}").unwrap();
        let ask = |widgets: Vec<u32>| {
            let widgets = widgets.into_iter().map(|widget| Request { widget, kind: "k".into(), options: Default::default(), size: Default::default() }).collect();
            g::write_json(&g::requests_path(&dir, "test"), &Requests { widgets }).unwrap();
        };
        ask(vec![7]);
        let made = Arc::new(AtomicUsize::new(0));
        let served = {
            let (dir, settings, own, made) = (dir.clone(), settings.clone(), own.clone(), made.clone());
            std::thread::spawn(move || {
                serve_in(
                    &dir,
                    &settings,
                    "test",
                    move || vec![own.clone()],
                    move |asked| {
                        // (When it was made is another time at each making;
                        // what it shows is the same.)
                        let n = made.fetch_add(1, Ordering::SeqCst) as i64;
                        let entries = vec![Entry { from: 0, glance: Glance { value: "44°".into(), ..Glance::default() } }];
                        (asked.iter().map(|r| (r.widget, Timeline { made: n, entries: entries.clone() })).collect(), None)
                    },
                )
            })
        };
        let until = |what: &dyn Fn() -> bool| {
            let t = Instant::now();
            while !what() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            what()
        };
        let made_now = || made.load(Ordering::SeqCst);
        let file = g::content_path(&dir, 7);
        let written = || std::fs::metadata(&file).map(|m| m.ino()).ok();
        assert!(until(&|| made_now() == 1 && written().is_some()), "asked: made and written");
        let first = written();
        // Written in the folder, and beside a file it follows: none of
        // its business.
        for n in 0..20 {
            g::write_json(&g::content_path(&dir, 8), &Timeline { made: n, ..Default::default() }).unwrap();
            g::write_json(&g::requests_path(&dir, "other"), &Requests::default()).unwrap();
            std::fs::write(root.join("someone-elses.json"), n.to_string()).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(made_now(), 1, "woken for what isn't its business");
        // A path of its own changes: made again, once; what it shows is
        // the same, so its file is the one it was.
        std::fs::write(&own, "2").unwrap();
        assert!(until(&|| made_now() == 2));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!((made_now(), written()), (2, first), "made once more, and nothing written: it shows the same");
        // The desktop's settings change (its language may have).
        std::fs::write(&settings, "{ }").unwrap();
        assert!(until(&|| made_now() == 3));
        // Its file gone (a Deskbar that cleared the folder): there again
        // at the next making.
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&own, "3").unwrap();
        assert!(until(&|| made_now() == 4 && written().is_some()));
        // Asked for another widget: made for it.
        ask(vec![7, 9]);
        assert!(until(&|| g::content_path(&dir, 9).exists()));
        // Asked for nothing: it ends.
        ask(vec![]);
        assert!(until(&|| served.is_finished()), "asked for nothing, and still there");
        let _ = std::fs::remove_dir_all(&root);
    }
}
