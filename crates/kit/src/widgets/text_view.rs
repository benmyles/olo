//! A text to read in a scroll view, not editable: wrapped to the view,
//! in the plain or the monospaced face, coloured as its owner says
//! (runs: a log line's highlighting, what a search found marked behind),
//! with a note in the middle while it's empty ("Select a line to read it
//! whole."). Selected with the pointer (a drag, a double click a word),
//! ⌘A all; ⌘C or the context menu's Copy copies it. Agents read it as a
//! `Document` with its text.

use std::ops::Range;

use olo_interface::{
    Bounds, Child, ClipboardItem, Cx, Element, FocusHandle, Font, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    ScrollHandle, SharedString, Styled as _, TextRun, View, agent, canvas, clipboard, div, point, px, styled_text,
};
use olo_kit_base::metrics as m;

use crate::{Menu, menu, style, theme, widgets::ScrollView};

/// The text's margin inside the view.
const MARGIN_X: f32 = 6.0;
const MARGIN_Y: f32 = 4.0;

pub struct TextView {
    focus: FocusHandle,
    scroll: ScrollHandle,
    text: SharedString,
    /// Its colours (runs covering it), else the document's text colour.
    runs: Option<Vec<TextRun>>,
    mono: bool,
    empty: SharedString,
    selected: Range<usize>,
    /// Where a drag started (a byte index), while the pointer is down.
    anchor: Option<usize>,
}

/// `text` to the clipboard, if there's any.
fn copy(text: String) {
    if !text.is_empty() {
        clipboard::write(ClipboardItem::new_string(text));
    }
}

/// A text view, owned by the view that makes it.
pub fn text_view<V: View>(cx: &mut Cx<V>, build: impl FnOnce(TextView) -> TextView) -> Child<TextView> {
    cx.new_view(|_| {
        build(TextView { focus: FocusHandle::new(), scroll: ScrollHandle::new(), text: SharedString::default(), runs: None, mono: false, empty: SharedString::default(), selected: 0..0, anchor: None })
    })
}

impl TextView {
    /// In the monospaced face.
    pub fn mono(mut self, on: bool) -> Self {
        self.mono = on;
        self
    }

    /// What it says, dim, in the middle, while there's no text.
    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = text.into();
        self
    }

    /// The text it starts with (as [`TextView::set_text`] sets it).
    pub fn with_text(mut self, text: impl Into<SharedString>, runs: Option<Vec<TextRun>>) -> Self {
        self.text = text.into();
        self.runs = runs.filter(|r| r.iter().map(|r| r.len).sum::<usize>() == self.text.len());
        self
    }

    /// The face its text is in (for runs made for it).
    pub fn font(&self) -> Font {
        if self.mono { style::mono_font(FontWeight::NORMAL) } else { style::ui_font(FontWeight::NORMAL) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// New text, coloured by `runs` (covering it, in [`TextView::font`]),
    /// or plain: scrolled to its top, nothing selected.
    pub fn set_text(&mut self, text: impl Into<SharedString>, runs: Option<Vec<TextRun>>, cx: &mut Cx<Self>) {
        let text = text.into();
        if text != self.text {
            self.scroll.set_offset(cx, point(px(0.0), px(0.0)));
            self.selected = 0..0;
        }
        self.text = text;
        self.runs = runs.filter(|r| r.iter().map(|r| r.len).sum::<usize>() == self.text.len());
        cx.notify();
    }

    pub fn selected_text(&self) -> &str {
        self.text.get(self.selected.clone()).unwrap_or("")
    }

    pub fn select_all(&mut self, cx: &mut Cx<Self>) {
        self.selected = 0..self.text.len();
        cx.notify();
    }

    /// The word around byte `at`.
    fn word_at(&self, at: usize) -> Range<usize> {
        word_at(&self.text, at)
    }

    /// Its runs, the selection behind (see-through, as a field's).
    fn shown_runs(&self, focused: bool) -> Vec<TextRun> {
        let p = theme::palette();
        let mut runs = self
            .runs
            .clone()
            .unwrap_or_else(|| vec![TextRun { len: self.text.len(), font: self.font(), color: style::c(p.document_text), background_color: None, underline: None, strikethrough: None, size: None }]);
        if self.selected.is_empty() {
            return runs;
        }
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let sel = style::c(p.control_highlight).opacity(if !focused {
            0.3
        } else if dark {
            0.75
        } else {
            0.55
        });
        // Split the runs where the selection starts and ends.
        let mut out = Vec::with_capacity(runs.len() + 2);
        let mut at = 0;
        for r in runs.drain(..) {
            let (a, b) = (at, at + r.len);
            at = b;
            let cuts = [a, self.selected.start.clamp(a, b), self.selected.end.clamp(a, b), b];
            for w in cuts.windows(2) {
                if w[1] > w[0] {
                    let inside = w[0] >= self.selected.start && w[1] <= self.selected.end;
                    out.push(TextRun { len: w[1] - w[0], background_color: if inside { Some(sel) } else { r.background_color }, ..r.clone() });
                }
            }
        }
        out
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        let at = cx.text_index_at(e.position).unwrap_or(0);
        self.focus.focus(cx);
        if e.click_count == 2 {
            self.selected = self.word_at(at);
            self.anchor = None;
        } else if e.click_count >= 3 {
            self.selected = 0..self.text.len();
            self.anchor = None;
        } else {
            let from = if e.modifiers.shift { self.anchor.unwrap_or(self.selected.start) } else { at };
            self.anchor = Some(from);
            self.selected = from.min(at)..from.max(at);
        }
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (Some(from), Some(at)) = (self.anchor, cx.text_index_at(e.position)) else { return };
        let r = from.min(at)..from.max(at);
        if r != self.selected {
            self.selected = r;
            cx.notify();
        }
    }
}

/// The word around byte `at` of `t`.
fn word_at(t: &str, at: usize) -> Range<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let at = at.min(t.len());
    let start = t[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = t[at..].char_indices().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, c)| at + i + c.len_utf8());
    start..end
}

impl View for TextView {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let focused = self.focus.is_focused(cx);
        let size = if self.mono { style::MONO_SIZE } else { style::PLAIN_SIZE };
        // (The view says what it is to agents: its text isn't a label
        // too.)
        let styled = styled_text(self.text.clone(), self.shown_runs(focused)).unlabelled();
        let body = div()
            .px(px(MARGIN_X))
            .py(px(MARGIN_Y))
            .w_full()
            .text_size(px(size))
            .line_height(px((size * style::LINE_HEIGHT).ceil()))
            .font(self.font())
            .cursor_text()
            .child(styled)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|v: &mut Self, _, _| v.anchor = None));

        // The note while empty, and what agents read.
        let (empty, text) = (self.empty.clone(), self.text.clone());
        let note = canvas(move |g, b| {
            let p = theme::palette();
            g.agent(agent::Node { role: "Document", value: Some(text.clone()), bounds: b, enabled: true, ..Default::default() });
            if text.is_empty() && !empty.is_empty() {
                // In the middle of where text goes (beside the bar).
                let b = Bounds::from_corners(b.origin, point(b.right() - px(crate::widgets::SCROLL_BAR), b.bottom()));
                let dim = style::c(olo_theme::mix(p.document_text, p.document_background, 0.5));
                style::label_fit(g, b.center().x, b.center().y, style::Align::Center, empty.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, b.size.width - px(2.0 * m::SPACING));
            }
        })
        .paint_key(style::key((&self.empty, &self.text)))
        .absolute()
        .inset_0();

        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .tab_stop()
            // (Its selection and its frame show whether it has the
            // keyboard.)
            .on_focus_changed(cx.listener(|_: &mut Self, _, cx| cx.notify()))
            .on_key_down(cx.listener(|v: &mut Self, e: &KeyDownEvent, cx| {
                let k = &e.keystroke;
                if !(k.modifiers.platform || k.modifiers.control) || k.modifiers.alt || k.modifiers.shift {
                    return;
                }
                match k.key.as_str() {
                    "c" => copy(v.selected_text().to_owned()),
                    "a" => v.select_all(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v: &mut Self, e: &MouseDownEvent, cx| {
                    let any = !v.selected.is_empty();
                    let m = Menu::new("").item("Copy", "copy").enabled(any).keys("⌘C").item("Select All", "select-all").keys("⌘A");
                    let chosen = cx.listener(|v: &mut Self, cmd: &str, cx| match cmd {
                        "copy" => copy(v.selected_text().to_owned()),
                        "select-all" => v.select_all(cx),
                        _ => {}
                    });
                    menu::context_menu(cx, m, e.position, chosen);
                }),
            )
            .child(ScrollView::new(&self.scroll).focused(focused).background(p.document_background).child(body).into_element())
            .child(note)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn words_are_found() {
        let t = "error: quota_left 429 reached";
        assert_eq!(&t[super::word_at(t, 9)], "quota_left");
    }
}
