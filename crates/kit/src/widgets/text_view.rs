//! A text to read in a scroll view, not editable: wrapped to the view,
//! in the plain or the monospaced face, coloured as its owner says
//! (runs: a log line's highlighting, what a search found marked behind),
//! with a note in the middle while it's empty ("Select a line to read it
//! whole."). Selected with the pointer (a drag, a double click a word),
//! ⌘A all; ⌘C or the context menu's Copy copies it. Agents read it as a
//! `Document` with its text.
//!
//! With [`TextView::links`], the web addresses, mail addresses and
//! numbers to dial in it are links ([`crate::links`]): in the look's
//! link colour, underlined under the pointer (as a document's are,
//! Preview's rich view), the pointer a hand over them. A click opens one
//! (a press and a release on it, not a drag: a drag from one selects);
//! the context menu on one has Open Link and Copy Link. Agents read each
//! as a `Link`, where it goes as its value, and click it.

use std::ops::Range;

use olo_interface::{
    Bounds, Child, ClipboardItem, Cx, Element, FocusHandle, Font, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, ScrollHandle, SharedString, Styled as _, TextLink, TextRun, View, agent, canvas, clipboard, div, point, px, styled_text,
};
use olo_kit_base::metrics as m;

use crate::{Menu, links, menu, style, theme, widgets::ScrollView};

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
    /// Its bar only while the text overflows (`auto_hide`).
    auto_hide: bool,
    /// Its links found ([`TextView::links`]), and those found: once for a
    /// text, as it's set.
    find_links: bool,
    links: Vec<links::Link>,
    /// The link under the pointer (underlined), by its place in `links`.
    hovered: Option<usize>,
    /// The link a press began on: it opens if the press ends on it.
    pressed: Option<usize>,
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
        build(TextView {
            focus: FocusHandle::new(),
            scroll: ScrollHandle::new(),
            text: SharedString::default(),
            runs: None,
            mono: false,
            empty: SharedString::default(),
            selected: 0..0,
            anchor: None,
            auto_hide: false,
            find_links: false,
            links: Vec::new(),
            hovered: None,
            pressed: None,
        })
    })
}

impl TextView {
    /// In the monospaced face.
    pub fn mono(mut self, on: bool) -> Self {
        self.mono = on;
        self
    }

    /// Its scroll bar only while the text is more than shows: a short
    /// text read beside others (Calendar's preview: a bar by "No notes."
    /// was heavy), as `ScrollView::auto_hide`.
    pub fn auto_hide(mut self, on: bool) -> Self {
        self.auto_hide = on;
        self
    }

    /// Its text's margins inside its frame, across and down.
    pub const MARGINS: (f32, f32) = (MARGIN_X, MARGIN_Y);

    /// Its lines' height in the plain face: a whole number of points (a
    /// paragraph's elsewhere isn't, `StringView::wrap`). With
    /// [`Self::MARGINS`], for text beside it to be set as it is
    /// (Calendar's preview: an event's facts in a box beside its notes,
    /// their lines on its lines at every scale).
    pub fn line_height() -> f32 {
        (style::PLAIN_SIZE * style::LINE_HEIGHT).ceil()
    }

    /// Its web addresses, mail addresses and numbers to dial as links
    /// (the module's doc): an event's notes, a message.
    pub fn links(mut self, on: bool) -> Self {
        self.find_links = on;
        self.found();
        self
    }

    /// Its links, found again for its text.
    fn found(&mut self) {
        self.links = if self.find_links { links::find(&self.text) } else { Vec::new() };
        (self.hovered, self.pressed) = (None, None);
    }

    /// The link the pointer at `at` (in the window) is on, if any.
    fn link_at(&self, at: olo_interface::Point<olo_interface::Pixels>, cx: &Cx<Self>) -> Option<usize> {
        if self.links.is_empty() {
            return None;
        }
        let byte = cx.text_hit_at(at)?;
        self.links.iter().position(|l| l.range.contains(&byte))
    }

    /// What it says, dim, in the middle, while there's no text.
    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = crate::words::of(text);
        self
    }

    /// The text it starts with (as [`TextView::set_text`] sets it).
    pub fn with_text(mut self, text: impl Into<SharedString>, runs: Option<Vec<TextRun>>) -> Self {
        self.text = text.into();
        self.runs = runs.filter(|r| r.iter().map(|r| r.len).sum::<usize>() == self.text.len());
        self.found();
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
        let changed = text != self.text;
        if changed {
            self.scroll.set_offset(cx, point(px(0.0), px(0.0)));
            self.selected = 0..0;
        }
        self.text = text;
        self.runs = runs.filter(|r| r.iter().map(|r| r.len).sum::<usize>() == self.text.len());
        if changed {
            self.found();
        }
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

    /// Its runs: its links in the link colour, the selection behind
    /// (see-through, as a field's).
    fn shown_runs(&self, focused: bool) -> Vec<TextRun> {
        let p = theme::palette();
        let runs = self
            .runs
            .clone()
            .unwrap_or_else(|| vec![TextRun { len: self.text.len(), font: self.font(), color: style::c(p.document_text), background_color: None, underline: None, strikethrough: None, size: None }]);
        let link = style::c(p.syntax(olo_theme::Syntax::Link));
        let ranges: Vec<Range<usize>> = self.links.iter().map(|l| l.range.clone()).collect();
        let runs = marked(runs, &ranges, |r| r.color = link);
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
        // (A link selected is in the text's colour: the link's on the
        // selection read poorly, Codex's look.)
        let text = style::c(p.document_text);
        marked(runs, std::slice::from_ref(&self.selected), |r| {
            r.background_color = Some(sel);
            if r.color == link {
                r.color = text;
            }
        })
    }

    /// Its links as the text element takes them: the one under the
    /// pointer underlined.
    fn text_links(&self) -> Vec<TextLink> {
        let link = style::c(theme::palette().syntax(olo_theme::Syntax::Link));
        self.links.iter().enumerate().map(|(i, l)| TextLink { range: l.range.clone(), target: l.target.clone().into(), underline: (self.hovered == Some(i)).then_some(link) }).collect()
    }

    /// Its context menu, on its link `on` (Open Link, Copy Link) or not.
    fn context_menu(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        let on = self.link_at(e.position, cx);
        let any = !self.selected.is_empty();
        let mut m = Menu::new("");
        if on.is_some() {
            m = m.item("Open Link", "open-link").item("Copy Link", "copy-link").separator();
        }
        let m = m.item("Copy", "copy").enabled(any).keys("⌘C").item("Select All", "select-all").keys("⌘A");
        let chosen = cx.listener(move |v: &mut Self, cmd: &str, cx| match cmd {
            "open-link" => {
                if let Some(l) = on.and_then(|i| v.links.get(i)) {
                    links::open(l);
                }
            }
            "copy-link" => {
                if let Some(l) = on.and_then(|i| v.links.get(i)) {
                    copy(l.copied(&v.text));
                }
            }
            "copy" => copy(v.selected_text().to_owned()),
            "select-all" => v.select_all(cx),
            _ => {}
        });
        menu::context_menu(cx, m, e.position, chosen);
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        let at = cx.text_index_at(e.position).unwrap_or(0);
        self.pressed = if e.click_count == 1 && !e.modifiers.shift { self.link_at(e.position, cx) } else { None };
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
            // (Only a text with links hears moves without a button.)
            let on = self.link_at(e.position, cx);
            if on != self.hovered {
                self.hovered = on;
                cx.notify();
            }
            return;
        }
        let (Some(from), Some(at)) = (self.anchor, cx.text_index_at(e.position)) else { return };
        let r = from.min(at)..from.max(at);
        if r != self.selected {
            self.selected = r;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        self.anchor = None;
        // A press and a release on the same link, nothing dragged: it
        // opens.
        if let Some(i) = self.pressed.take()
            && self.selected.is_empty()
            && self.link_at(e.position, cx) == Some(i)
        {
            links::open(&self.links[i]);
        }
    }
}

/// `runs` cut where `ranges` (in order, apart) start and end, `mark` done
/// to the pieces inside them: one pass over both.
fn marked(runs: Vec<TextRun>, ranges: &[Range<usize>], mark: impl Fn(&mut TextRun)) -> Vec<TextRun> {
    if ranges.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2 * ranges.len());
    let (mut at, mut next) = (0, 0);
    for r in runs {
        let end = at + r.len;
        while at < end {
            while ranges.get(next).is_some_and(|m| m.end <= at) {
                next += 1;
            }
            let (to, inside) = match ranges.get(next) {
                Some(m) if m.start <= at => (m.end.min(end), true),
                Some(m) => (m.start.min(end), false),
                None => (end, false),
            };
            let mut piece = TextRun { len: to - at, ..r.clone() };
            if inside {
                mark(&mut piece);
            }
            out.push(piece);
            at = to;
        }
    }
    out
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
        let styled = styled_text(self.text.clone(), self.shown_runs(focused)).unlabelled().links(self.text_links());
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
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            // (The pointer gone, from the view or the window: its link is
            // no longer under it.)
            .on_hover(cx.listener(|v: &mut Self, inside: &bool, cx| {
                if !*inside && v.hovered.take().is_some() {
                    cx.notify();
                }
            }));
        // (A selection dragged: only with the button held; a link under
        // the pointer, without.)
        let body = if self.links.is_empty() { body.moves_matter_while_pressed() } else { body };

        // The note while empty, and what agents read.
        let (empty, text, auto_hide) = (self.empty.clone(), self.text.clone(), self.auto_hide);
        let note = canvas(move |g, b| {
            let p = theme::palette();
            g.agent(agent::Node { role: "Document", value: Some(text.clone()), bounds: b, enabled: true, ..Default::default() });
            if text.is_empty() && !empty.is_empty() {
                // In the middle of where text goes: beside the bar, or all
                // of it where the bar is hidden (no text, nothing to
                // scroll: `auto_hide`; "No notes." sat left of the middle).
                let bar = if auto_hide { 0.0 } else { crate::widgets::SCROLL_BAR };
                let b = Bounds::from_corners(b.origin, point(b.right() - px(bar), b.bottom()));
                let dim = style::c(olo_theme::mix(p.document_text, p.document_background, 0.5));
                style::label_fit(g, b.center().x, b.center().y, style::Align::Center, empty.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, b.size.width - px(2.0 * m::SPACING));
            }
        })
        .paint_key(style::key((&self.empty, &self.text, self.auto_hide)))
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
            .on_mouse_down(MouseButton::Right, cx.listener(Self::context_menu))
            .child(ScrollView::new(&self.scroll).auto_hide(self.auto_hide).focused(focused).background(p.document_background).child(body).into_element())
            .child(note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_found() {
        let t = "error: quota_left 429 reached";
        assert_eq!(&t[super::word_at(t, 9)], "quota_left");
    }

    /// Links and a selection marked over a text's runs: cut where they
    /// start and end, nothing else changed.
    #[test]
    fn runs_marked() {
        let run = |len, color| TextRun { len, font: style::ui_font(FontWeight::NORMAL), color, background_color: None, underline: None, strikethrough: None, size: None };
        let (a, b, x) = (olo_interface::black(), olo_interface::white(), olo_interface::red());
        let runs = vec![run(10, a), run(10, b)];
        let out = marked(runs.clone(), &[2..4, 8..13, 19..20], |r| r.color = x);
        let got: Vec<(usize, bool)> = out.iter().map(|r| (r.len, r.color == x)).collect();
        assert_eq!(got, [(2, false), (2, true), (4, false), (2, true), (3, true), (6, false), (1, true)]);
        assert_eq!(out.iter().map(|r| r.len).sum::<usize>(), 20);
        assert_eq!(marked(runs.clone(), &[], |r| r.color = x), runs);
    }
}
