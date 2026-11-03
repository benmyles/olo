//! A text to read in a scroll view, not editable: wrapped to the view, in the plain or the monospaced face,
//! coloured as its owner says (runs: a log line's highlighting, what a
//! search found marked behind), with a note in the middle while it's
//! empty ("Select a line to read it whole."). Selected with the pointer
//! (a drag, a double click a word), ⌘A all; ⌘C or the context menu's
//! Copy copies it. Agents read it as a `Document` with its text.

use std::ops::Range;

use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, StyledText, TextRun, Window, canvas, div, px,
};
use olo_kit_base::metrics as m;

use crate::{Menu, agent, menu, style, theme, widgets::ScrollView};

/// The text's margin inside the view.
const MARGIN_X: f32 = 6.0;
const MARGIN_Y: f32 = 4.0;

pub struct TextView {
    id: SharedString,
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
fn copy(text: String, cx: &mut App) {
    if !text.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

/// A text view (`id` for its scroll view).
pub fn text_view(cx: &mut App, id: impl Into<SharedString>, build: impl FnOnce(TextView) -> TextView) -> Entity<TextView> {
    let id = id.into();
    cx.new(|cx| {
        build(TextView {
            id,
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            text: SharedString::default(),
            runs: None,
            mono: false,
            empty: SharedString::default(),
            selected: 0..0,
            anchor: None,
        })
    })
}

impl Focusable for TextView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
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

    /// The face its text is in (for runs made for it).
    pub fn font(&self) -> gpui::Font {
        if self.mono { style::mono_font(FontWeight::NORMAL) } else { style::ui_font(FontWeight::NORMAL) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// New text, coloured by `runs` (covering it, in [`TextView::font`]),
    /// or plain: scrolled to its top, nothing selected.
    pub fn set_text(&mut self, text: impl Into<SharedString>, runs: Option<Vec<TextRun>>, cx: &mut Context<Self>) {
        let text = text.into();
        if text != self.text {
            self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.selected = 0..0;
        }
        self.text = text;
        self.runs = runs.filter(|r| r.iter().map(|r| r.len).sum::<usize>() == self.text.len());
        cx.notify();
    }

    pub fn selected_text(&self) -> &str {
        self.text.get(self.selected.clone()).unwrap_or("")
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        cx.notify();
    }

    /// The word around byte `at`.
    fn word_at(&self, at: usize) -> Range<usize> {
        let t: &str = &self.text;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let at = at.min(t.len());
        let start = t[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
        let end = t[at..].char_indices().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, c)| at + i + c.len_utf8());
        start..end
    }

    /// Its runs, the selection behind (see-through, as a field's).
    fn shown_runs(&self, window: &Window, cx: &App) -> Vec<TextRun> {
        let p = theme::palette(cx);
        let mut runs = self
            .runs
            .clone()
            .unwrap_or_else(|| vec![TextRun { len: self.text.len(), font: self.font(), color: style::c(p.document_text), background_color: None, underline: None, strikethrough: None, size: None }]);
        if self.selected.is_empty() {
            return runs;
        }
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let focused = self.focus.is_focused(window);
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
}

impl Render for TextView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let p = theme::palette(cx);
        let focused = self.focus.is_focused(window);
        let size = if self.mono { style::MONO_SIZE } else { style::PLAIN_SIZE };
        let styled = StyledText::new(self.text.clone()).with_runs(self.shown_runs(window, cx));
        let layout = styled.layout().clone();
        let hit = move |p: gpui::Point<gpui::Pixels>| match layout.index_for_position(p) {
            Ok(i) | Err(i) => i,
        };
        let (down, moved) = (hit.clone(), hit);
        let (m_down, m_move, m_up) = (me.clone(), me.clone(), me.clone());
        let body = div()
            .id(SharedString::from(format!("{}-text", self.id)))
            .px(px(MARGIN_X))
            .py(px(MARGIN_Y))
            .w_full()
            .text_size(px(size))
            .line_height(px((size * style::LINE_HEIGHT).ceil()))
            .font(self.font())
            .cursor_text()
            .child(styled)
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
                let at = down(e.position);
                m_down.update(cx, |v, cx| {
                    window.focus(&v.focus);
                    if e.click_count == 2 {
                        v.selected = v.word_at(at);
                        v.anchor = None;
                    } else if e.click_count >= 3 {
                        v.selected = 0..v.text.len();
                        v.anchor = None;
                    } else {
                        let from = if e.modifiers.shift { v.anchor.unwrap_or(v.selected.start) } else { at };
                        v.anchor = Some(from);
                        v.selected = from.min(at)..from.max(at);
                    }
                    cx.notify();
                });
            })
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let at = moved(e.position);
                m_move.update(cx, |v, cx| {
                    if let Some(from) = v.anchor {
                        let r = from.min(at)..from.max(at);
                        if r != v.selected {
                            v.selected = r;
                            cx.notify();
                        }
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| m_up.update(cx, |v, _| v.anchor = None));

        // The note while empty, and what agents read.
        let (empty, text) = (self.empty.clone(), self.text.clone());
        let note = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                agent::record(window, cx, agent::Node { role: "Document", value: Some(text.clone()), bounds: b, enabled: true, ..Default::default() });
                if text.is_empty() && !empty.is_empty() {
                    // In the middle of where text goes (beside the bar).
                    let b = gpui::Bounds::from_corners(b.origin, gpui::point(b.right() - px(crate::widgets::SCROLL_BAR), b.bottom()));
                    let dim = style::c(olo_theme::mix(p.document_text, p.document_background, 0.5));
                    style::label_fit(
                        window,
                        cx,
                        b.center().x,
                        b.center().y,
                        style::Align::Center,
                        empty.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        dim,
                        b.size.width - px(2.0 * m::SPACING),
                    );
                }
            },
        )
        .absolute()
        .inset_0();

        let (keys, menu_me) = (me.clone(), me.clone());
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(move |e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if !(k.modifiers.platform || k.modifiers.control) || k.modifiers.alt || k.modifiers.shift {
                    return;
                }
                match k.key.as_str() {
                    "c" => {
                        let t = keys.read(cx).selected_text().to_owned();
                        copy(t, cx);
                    }
                    "a" => keys.update(cx, |v, cx| v.select_all(cx)),
                    _ => return,
                }
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, cx| {
                let any = !menu_me.read(cx).selected.is_empty();
                let me = menu_me.clone();
                let m = Menu::new("").item("Copy", "copy").enabled(any).keys("⌘C").item("Select All", "select-all").keys("⌘A");
                menu::context_menu(window, cx, m, e.position, move |cmd, _, cx| match cmd {
                    "copy" => {
                        let t = me.read(cx).selected_text().to_owned();
                        copy(t, cx);
                    }
                    "select-all" => me.update(cx, |v, cx| v.select_all(cx)),
                    _ => {}
                });
            })
            .child(ScrollView::new(self.id.clone(), &self.scroll).focused(focused).background(p.document_background).child(body))
            .child(note)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn words_are_found() {
        let t = "error: quota_left 429 reached";
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let at = 9;
        let start = t[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
        let end = t[at..].char_indices().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, c)| at + i + c.len_utf8());
        assert_eq!(&t[start..end], "quota_left");
    }
}
