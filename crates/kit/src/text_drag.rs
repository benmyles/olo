//! Dragging text out of a text field and dropping text into one, for any
//! `egui::TextEdit` (the kit's [`TextControl`](crate::widgets::TextControl)
//! and [`TextView`](crate::widgets::TextView) use it; StyledEdit too).
//!
//! As on BeOS: press on the selection and drag, and the text goes with
//! the pointer (to another field, another app, a Tracker folder as a
//! clipping); dropped in the field it came from, it moves. Text dragged
//! over a field shows where it will go with a caret, and is inserted
//! there, selected. Files are left to the window (StyledEdit opens them).
//!
//! ```ignore
//! let mut drag = TextDrag::new(ui, id, editable, password, multiline);
//! if let Some(d) = drag.take_drop() { let r = text_drag::apply(text, &d); text_drag::select(ui.ctx(), id, r); }
//! let out = TextEdit::multiline(text).id(id).interactive(editable && drag.interactive()).show(ui);
//! drag.finish(ui, &out);
//! ```

use std::{ops::Range, sync::Arc};

use egui::{
    Galley, Id, Pos2, Rect, Ui, pos2,
    text::{CCursor, CCursorRange, CharIndex},
    vec2,
};

use crate::{dnd, style};

/// Where a field's text was laid out last frame (to find the character
/// under a press or a drop before this frame's editor runs).
#[derive(Clone)]
struct Laid {
    galley: Arc<Galley>,
    /// Where the galley's origin is on the screen.
    origin: Pos2,
    /// The field, as far as it shows.
    clip: Rect,
}

impl Laid {
    fn char_at(&self, pos: Pos2) -> usize {
        self.galley.cursor_from_pos(pos - self.origin).index.into()
    }

    /// `pos` is on the selection `r` as it is drawn.
    fn on(&self, r: &Range<usize>, pos: Pos2) -> bool {
        if r.is_empty() || !self.clip.contains(pos) {
            return false;
        }
        let p = pos - self.origin;
        let lo = self.galley.pos_from_cursor(CCursor::new(r.start));
        let hi = self.galley.pos_from_cursor(CCursor::new(r.end));
        if p.y < lo.min.y || p.y > hi.max.y {
            return false;
        }
        !(p.y < lo.max.y && p.x < lo.min.x) && !(p.y > hi.min.y && p.x > hi.min.x)
    }

    /// The caret at character `i`, on the screen.
    fn caret(&self, i: usize) -> Rect {
        self.galley.pos_from_cursor(CCursor::new(i)).translate(self.origin.to_vec2())
    }
}

/// A press on the selection, not yet a drag or a click.
#[derive(Clone)]
struct Armed {
    press: Pos2,
    at: usize,
    range: Range<usize>,
}

/// The field a drag of text started in, and what it took (so a drop back
/// in it moves the text).
#[derive(Clone)]
struct Source {
    id: Id,
    range: Range<usize>,
}

/// What a drop does to the field's text, in characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextDrop {
    Insert { at: usize, text: String },
    /// Dropped in the field it came from: `from` goes to `to` (an index in
    /// the text before the move).
    Move { from: Range<usize>, to: usize, text: String },
}

/// One frame of a field's dragging; see the module's example.
pub struct TextDrag {
    id: Id,
    editable: bool,
    armed: Option<Armed>,
    dropped: Option<TextDrop>,
}

fn laid_id(id: Id) -> Id {
    id.with("tabula-text-drag-laid")
}

fn armed_id(id: Id) -> Id {
    id.with("tabula-text-drag-armed")
}

fn source_id() -> Id {
    Id::unique("tabula-text-drag-source")
}

fn target(id: Id) -> String {
    format!("{}text:{:016x}", dnd::WIDGET, id.value())
}

/// How far the pointer goes before a press on the selection is a drag.
const SLOP: f32 = 4.0;

impl TextDrag {
    /// Before the field's `TextEdit` is shown: takes a drop made on it and
    /// sees a press land on its selection.
    pub fn new(ui: &Ui, id: Id, editable: bool, password: bool, multiline: bool) -> TextDrag {
        let ctx = ui.ctx();
        let laid = ctx.data(|d| d.get_temp::<Laid>(laid_id(id)));
        let mut armed = ctx.data(|d| d.get_temp::<Option<Armed>>(armed_id(id))).flatten();
        let pressed = ui.input(|i| i.pointer.primary_pressed() && !i.modifiers.shift && !i.modifiers.alt);
        if pressed
            && !password
            && armed.is_none()
            && let (Some(laid), Some(pos)) = (&laid, ui.input(|i| i.pointer.press_origin()))
            && ctx.memory(|m| m.has_focus(id))
            && let Some(r) = egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range())
        {
            let range = sorted(r);
            if laid.on(&range, pos) {
                armed = Some(Armed { press: pos, at: laid.char_at(pos), range });
                ctx.data_mut(|d| d.insert_temp(armed_id(id), armed.clone()));
            }
        }
        let mut dropped = None;
        if let Some(drop) = dnd::take_widget_drop(ctx, &target(id))
            && editable
            && let Some(laid) = &laid
        {
            let mut text = drop.text().replace("\r\n", "\n");
            // One line: its lines joined with spaces.
            if !multiline && text.contains('\n') {
                text = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
            }
            let at = laid.char_at(drop.pos);
            let source = ctx.data_mut(|d| d.remove_temp::<Option<Source>>(source_id())).flatten();
            dropped = Some(match source {
                Some(s) if drop.from_here && s.id == id => TextDrop::Move { from: s.range, to: at, text },
                _ => TextDrop::Insert { at, text },
            });
            ctx.memory_mut(|m| m.request_focus(id));
        }
        TextDrag { id, editable, armed, dropped }
    }

    /// A drop to make on the text before showing it (with [`apply`], or
    /// the app's own editing, so its undo and styles follow).
    pub fn take_drop(&mut self) -> Option<TextDrop> {
        self.dropped.take()
    }

    /// Whether the editor may handle the pointer this frame: not while a
    /// press on the selection might become a drag (it would move the
    /// caret there).
    pub fn interactive(&self) -> bool {
        self.armed.is_none()
    }

    /// After the field's `TextEdit` is shown: starts a drag, or makes a
    /// press that wasn't one a click, and shows where text dragged over
    /// the field would go.
    pub fn finish(self, ui: &Ui, out: &egui::text_edit::TextEditOutput) {
        let ctx = ui.ctx();
        let laid = Laid { galley: out.galley.clone(), origin: out.galley_pos - vec2(out.galley.rect.left(), 0.0), clip: out.response.rect.intersect(ui.clip_rect()) };
        ctx.data_mut(|d| d.insert_temp(laid_id(self.id), laid.clone()));

        if let Some(a) = &self.armed {
            let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
            if !down {
                // A click on the selection: the caret goes there.
                ctx.data_mut(|d| d.insert_temp::<Option<Armed>>(armed_id(self.id), None));
                select(ctx, self.id, a.at..a.at);
            } else if pos.is_some_and(|p| p.distance(a.press) > SLOP) {
                ctx.data_mut(|d| {
                    d.insert_temp::<Option<Armed>>(armed_id(self.id), None);
                    d.insert_temp(source_id(), Some(Source { id: self.id, range: a.range.clone() }));
                });
                let dragged: String = out.galley.text().chars().skip(a.range.start).take(a.range.len()).collect();
                dnd::start(ctx, text_drag_data(ctx, &dragged));
            }
            ctx.request_repaint();
        }

        // Text dragged over the field.
        if self.editable
            && let Some(h) = dnd::hover(ctx)
            && !h.has_files()
            && laid.clip.contains(h.pos)
            && let Some(mime) = h.text_type()
        {
            dnd::accept(ctx, target(self.id), mime, dnd::Action::Copy);
            let caret = laid.caret(laid.char_at(h.pos));
            let p = style::palette(ctx);
            let x = caret.left().round() + 0.5;
            ui.painter().with_clip_rect(laid.clip.expand(1.0)).line_segment(
                [pos2(x, caret.top()), pos2(x, caret.bottom())],
                egui::Stroke::new(2.0, style::c(p.keyboard_navigation)),
            );
        }
    }
}

/// The drag of `text`: the text itself, with a clipping's icon and its
/// first words beside it.
fn text_drag_data(ctx: &egui::Context, text: &str) -> dnd::DragData {
    let data = dnd::DragData::text(text);
    let first: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut label: String = first.chars().take(30).collect();
    if first.chars().count() > 30 {
        label.push('…');
    }
    let icon = crate::files::Kind::Text.icon();
    // Below and right of the pointer, clear of the caret showing where
    // it will go.
    match crate::icons::drag_icon(ctx, icon, 32.0, 1, vec2(-6.0, -10.0), crate::icons::DragLabel::Right(&label)) {
        Some(i) => data.icon(i),
        None => data,
    }
}

fn sorted(r: CCursorRange) -> Range<usize> {
    let (a, b): (usize, usize) = (r.primary.index.into(), r.secondary.index.into());
    a.min(b)..a.max(b)
}

/// Makes `drop` on `text`; the characters it put there (to select).
pub fn apply(text: &mut dyn egui::TextBuffer, drop: &TextDrop) -> Range<usize> {
    match drop {
        TextDrop::Insert { at, text: t } => {
            let at = (*at).min(text.as_str().chars().count());
            let n = text.insert_text(t, CharIndex(at));
            at..at + n
        }
        TextDrop::Move { from, to, text: t } => {
            // Onto itself: nothing moves.
            if (from.start..=from.end).contains(to) {
                return from.clone();
            }
            text.delete_char_range(CharIndex(from.start)..CharIndex(from.end));
            let at = if *to > from.end { to - from.len() } else { *to };
            let n = text.insert_text(t, CharIndex(at));
            at..at + n
        }
    }
}

/// Selects `r` (characters) in the field `id` and gives it the keyboard.
pub fn select(ctx: &egui::Context, id: Id, r: Range<usize>) {
    let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    s.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(r.start), CCursor::new(r.end))));
    s.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_and_inserts() {
        let mut s = String::from("one two three");
        let r = apply(&mut s, &TextDrop::Move { from: 4..8, to: 13, text: "two ".into() });
        assert_eq!((s.as_str(), r), ("one threetwo ", 9..13));
        let mut s = String::from("one two three");
        let r = apply(&mut s, &TextDrop::Move { from: 8..13, to: 0, text: "three".into() });
        assert_eq!((s.as_str(), r), ("threeone two ", 0..5));
        let mut s = String::from("one two");
        assert_eq!(apply(&mut s, &TextDrop::Move { from: 0..3, to: 2, text: "one".into() }), 0..3);
        assert_eq!(s, "one two");
        let mut s = String::from("héllo");
        assert_eq!(apply(&mut s, &TextDrop::Insert { at: 2, text: "ü".into() }), 2..3);
        assert_eq!(s, "héüllo");
    }
}
