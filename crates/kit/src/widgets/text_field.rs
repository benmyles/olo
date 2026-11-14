//! A text field, one line or several (`multiline`, wrapped to its width:
//! a chat's message): typing (with input methods), the caret and the
//! selection by keyboard and pointer, words, the clipboard and undo, and
//! an edit menu on the right button. No frame of its own: the caller
//! draws the well around it (a `style::sunken`, DeskCalc's display), so
//! it fits any of them. The caret is a line wide (a device pixel at 1×
//! and 1.5×), on device pixels, and blinks only while the field has the
//! keyboard in an active window. Agents read it as a `TextInput` with
//! its text.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    time::Duration,
};

use olo_interface::{
    Bounds, Child, ClipboardItem, ContentMask, CursorStyle, Cx, Element, Hsla, InteractiveElement as _, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter,
    ParentElement as _, Pixels, ShapedLine, SharedString, Styled as _, TextInput, TextRun, TimerId, UnderlineStyle, View, WrappedLine, clipboard, div, point, px, size,
};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::{style, theme};

/// A key as the field's table names it: "ctrl-shift-left" (the command
/// key reaches apps as Ctrl).
fn chord(e: &KeyDownEvent) -> String {
    let m = &e.keystroke.modifiers;
    let mut out = String::new();
    for (on, name) in [(m.control, "ctrl-"), (m.alt, "alt-"), (m.shift, "shift-")] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&e.keystroke.key);
    out
}

/// What happened to a field.
#[derive(Clone, Debug)]
pub enum TextFieldEvent {
    /// Its text changed (typed, pasted, cut, undone).
    Changed,
    /// Text typed into it (not pasted): the characters.
    Typed(SharedString),
    /// Return pressed in it.
    Entered,
}

/// Which side the text keeps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// How long the caret shows and hides.
const BLINK: Duration = Duration::from_millis(530);
/// How long it blinks after the last keystroke or click; then it stays,
/// and a window left with a field that has the keyboard draws nothing
/// (GTK's `gtk-cursor-blink-timeout` is the same 10 seconds).
const BLINK_FOR: Duration = Duration::from_secs(10);
/// How many steps undo keeps.
const UNDO: usize = 100;

pub struct TextField {
    /// It has the keyboard (as its `body` last found).
    focused: Cell<bool>,
    text: String,
    selected: Range<usize>,
    /// The selection grows from its end, back towards its start.
    reversed: bool,
    /// What an input method is composing.
    marked: Option<Range<usize>>,
    font_size: f32,
    /// Its face, if not the UI's (a PDF form's text).
    font: Option<olo_interface::Font>,
    /// The most characters it takes (a PDF field's limit).
    max_len: Option<usize>,
    color: Option<Hsla>,
    align: Align,
    name: SharedString,
    undo: Vec<(String, Range<usize>)>,
    redo: Vec<(String, Range<usize>)>,
    /// A run of typing (when, and where it got to): one undo step, as the
    /// kit's runs are (`undo::History::done_merging`).
    typed: Option<(std::time::Instant, usize)>,
    /// The line as last drawn, and where (its left, the line's top and
    /// height): what the pointer and the keys are worked out from.
    line: RefCell<Option<ShapedLine>>,
    drawn: Cell<Option<Bounds<Pixels>>>,
    selecting: bool,
    caret_on: bool,
    /// The caret's blinking: its timer, and how long it has blinked.
    blink: Option<(TimerId, Duration)>,
    /// Shown as bullets, never copied (a password).
    password: bool,
    /// Shown dim while it's empty ("Password").
    hint: SharedString,
    /// Can't be typed in now (a password while PAM checks the last).
    enabled: bool,
    /// Its text's colours, from its text (a search's filters).
    colors: Option<Colors>,
    /// Several lines, wrapped to its width; Return sends (`Entered`)
    /// when `enter_sends`, ⇧Return makes a new line.
    multiline: bool,
    enter_sends: bool,
    keeps_enter: bool,
    /// A multi-line field's paragraphs as drawn, the line height, and how
    /// far its text is scrolled up (it keeps the caret in view).
    paragraphs: RefCell<Vec<WrappedLine>>,
    line_height: Cell<Pixels>,
    scroll: Cell<Pixels>,
    /// How far a one-line field's text is scrolled sideways (to keep the
    /// caret in view while it's edited).
    scroll_x: Cell<Pixels>,
    /// Up and down keep to this column.
    goal_x: Option<Pixels>,
}

/// Colours for parts of a field's text (byte ranges, in order), in the
/// palette given; the rest in its colour.
pub type Colors = std::rc::Rc<dyn Fn(&str, &olo_theme::Palette) -> Vec<(Range<usize>, Hsla)>>;

/// What a password field shows for each character.
const BULLET: char = '•';

/// What a field is told besides what's typed into it.
pub enum FieldMessage {
    /// The clipboard's text, read: put in.
    Pasted(String),
    /// The caret's blink.
    Blink,
}

impl Default for TextField {
    fn default() -> Self {
        Self::new()
    }
}

impl TextField {
    pub fn new() -> TextField {
        TextField {
            focused: Cell::new(false),
            text: String::new(),
            selected: 0..0,
            reversed: false,
            marked: None,
            font_size: style::PLAIN_SIZE,
            font: None,
            max_len: None,
            color: None,
            align: Align::Left,
            name: SharedString::default(),
            undo: Vec::new(),
            typed: None,
            redo: Vec::new(),
            line: RefCell::new(None),
            drawn: Cell::new(None),
            selecting: false,
            caret_on: true,
            blink: None,
            password: false,
            enabled: true,
            hint: SharedString::default(),
            colors: None,
            multiline: false,
            enter_sends: false,
            keeps_enter: false,
            paragraphs: RefCell::new(Vec::new()),
            line_height: Cell::new(px(style::PLAIN_SIZE * style::LINE_HEIGHT)),
            scroll: Cell::new(px(0.0)),
            scroll_x: Cell::new(px(0.0)),
            goal_x: None,
        }
    }

    /// Several lines, wrapped to its width; Return sends (`Entered`)
    /// when `enter_sends`, else makes a new line (⇧Return always does).
    pub fn multiline(mut self, enter_sends: bool) -> Self {
        self.multiline = true;
        self.enter_sends = enter_sends;
        self
    }

    /// Return is its own (`Entered`), even in a window with a default
    /// button, which otherwise takes it (a first password field whose
    /// Return goes on to the second).
    pub fn keeps_enter(mut self) -> Self {
        self.keeps_enter = true;
        self
    }

    /// A multi-line field's line height at `font_size` on this window:
    /// whole device pixels, so every line's baseline is on one.
    pub fn line_height_for(scale: f32, font_size: f32) -> Pixels {
        let s = scale;
        px((font_size * style::LINE_HEIGHT * s).round() / s)
    }

    /// How many lines its text takes wrapped to `width` (a multi-line
    /// field's owner sizes it by this).
    pub fn rows_at(&self, width: Pixels) -> usize {
        if self.text.is_empty() {
            return 1;
        }
        let run = TextRun { len: self.text.len(), font: self.face(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None, size: None };
        olo_interface::thread_text_system()
            .shape_text(SharedString::from(self.text.clone()), px(self.font_size), &[run], Some(width), None)
            .map_or(1, |lines| lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum::<usize>().max(1))
    }

    /// What it shows, dim, while it's empty.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Changes the hint (a composer's "Message" or "Edit message").
    pub fn set_hint(&mut self, hint: impl Into<SharedString>, cx: &mut Cx<Self>) {
        let hint = hint.into();
        if hint != self.hint {
            self.hint = hint;
            cx.notify();
        }
    }

    /// Shows bullets for its characters and never copies them.
    pub fn password(mut self, on: bool) -> Self {
        self.password = on;
        self
    }

    /// Bullets or the text (PAM asks a password, or a name to echo).
    pub fn set_password(&mut self, on: bool, cx: &mut Cx<Self>) {
        if self.password != on {
            self.password = on;
            cx.notify();
        }
    }

    /// Whether it takes typing and the keyboard (drawn dim when not).
    pub fn set_enabled(&mut self, on: bool, cx: &mut Cx<Self>) {
        if self.enabled != on {
            // (Tab passes over a field that can't be used: its `body`
            // leaves it out of the tab order.)
            self.enabled = on;
            cx.notify();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Forgets the text, leaving no copy in freed memory (a password,
    /// once used): no undo of it either.
    pub fn clear_secret(&mut self, cx: &mut Cx<Self>) {
        // SAFETY: zero bytes keep the strings valid UTF-8.
        unsafe { self.text.as_bytes_mut().fill(0) };
        for (t, _) in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            unsafe { t.as_bytes_mut().fill(0) };
        }
        self.text.clear();
        self.undo.clear();
        self.redo.clear();
        self.selected = 0..0;
        self.marked = None;
        cx.notify();
    }

    /// What it shows: its text, or a bullet for each character.
    fn shown(&self) -> String {
        if self.password { std::iter::repeat_n(BULLET, self.text.chars().count()).collect() } else { self.text.clone() }
    }

    /// Where byte `i` of the text is in what's shown.
    fn to_shown(&self, i: usize) -> usize {
        if self.password { self.text[..i.min(self.text.len())].chars().count() * BULLET.len_utf8() } else { i }
    }

    /// Byte `i` of what's shown, in the text.
    fn from_shown(&self, i: usize) -> usize {
        if self.password { self.text.char_indices().nth(i / BULLET.len_utf8()).map_or(self.text.len(), |(b, _)| b) } else { i }
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Its text in `font` (else the UI's face).
    pub fn font(mut self, font: olo_interface::Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Takes at most `n` characters.
    pub fn max_len(mut self, n: usize) -> Self {
        self.max_len = Some(n);
        self
    }

    fn face(&self) -> olo_interface::Font {
        self.font.clone().unwrap_or_else(|| style::ui_font(olo_interface::FontWeight::NORMAL))
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// What agents call it.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    /// Its text to begin with (the caret after it).
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self.selected = self.text.len()..self.text.len();
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the text (from the app: no undo step), the caret at the
    /// end.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Cx<Self>) {
        let text = text.into();
        if text != self.text {
            self.text = text;
            self.marked = None;
        }
        self.move_to(self.text.len(), cx);
        cx.notify();
    }

    /// Replaces all the text as an undo step (the app's own edit of it: a
    /// calculator's key, Clear), the caret at the end.
    pub fn replace_all(&mut self, text: &str, cx: &mut Cx<Self>) {
        if text != self.text {
            self.typed = None;
            self.replace(0..self.text.len(), text, cx);
        }
    }

    /// Its own Undo and Redo, for an app's menus (the palette, agents):
    /// what ⌘Z and ⇧⌘Z do in it.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_step(&mut self, cx: &mut Cx<Self>) {
        self.undo(cx);
    }

    pub fn redo_step(&mut self, cx: &mut Cx<Self>) {
        self.redo(cx);
    }

    /// The caret to the end.
    pub fn caret_to_end(&mut self, cx: &mut Cx<Self>) {
        self.move_to(self.text.len(), cx);
    }

    /// Whether it has the keyboard (as its `body` last found: its owner
    /// asks `Child::is_focused` for now).
    pub fn is_focused(&self) -> bool {
        self.focused.get()
    }

    /// The selection (bytes; empty: the caret).
    pub fn selection(&self) -> Range<usize> {
        self.selected.clone()
    }

    /// Selects `range` (bytes, on characters' boundaries).
    pub fn set_selection(&mut self, range: Range<usize>, cx: &mut Cx<Self>) {
        let len = self.text.len();
        self.selected = range.start.min(len)..range.end.min(len);
        self.reversed = false;
        self.show_caret(cx);
        cx.notify();
    }

    /// Selects all its text (Find: what was looked for, to type over).
    pub fn select_all_text(&mut self, cx: &mut Cx<Self>) {
        self.set_selection(0..self.text.len(), cx);
    }

    /// Colours parts of its text ([`Colors`]; None: all in its colour).
    pub fn set_colors(&mut self, colors: Option<Colors>, cx: &mut Cx<Self>) {
        self.colors = colors;
        cx.notify();
    }

    fn caret(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Cx<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        self.goal_x = None;
        self.show_caret(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Cx<Self>) {
        self.goal_x = None;
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        self.show_caret(cx);
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text.grapheme_indices(true).rev().find_map(|(i, _)| (i < offset).then_some(i)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text.grapheme_indices(true).find_map(|(i, _)| (i > offset).then_some(i)).unwrap_or(self.text.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.text.unicode_word_indices().rev().find_map(|(i, _)| (i < offset).then_some(i)).unwrap_or(0)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.text.unicode_word_indices().find_map(|(i, w)| (i + w.len() > offset).then_some(i + w.len())).unwrap_or(self.text.len())
    }

    /// Replaces `range` with `new`, as an undo step, the caret after it.
    fn replace(&mut self, range: Range<usize>, new: &str, cx: &mut Cx<Self>) {
        if !self.enabled {
            return;
        }
        // Past its limit, what's typed is cut short.
        let new = match self.max_len {
            Some(n) => {
                let room = n.saturating_sub(self.text.chars().count() - self.text[range.clone()].chars().count());
                match new.char_indices().nth(room) {
                    Some((cut, _)) => &new[..cut],
                    None => new,
                }
            }
            None => new,
        };
        // A letter typed where the last one went, soon after: the same
        // step (a space starts the next, as words do).
        let typing = range.is_empty() && new.chars().count() == 1 && !new.chars().any(char::is_whitespace);
        let joins = typing && self.typed.is_some_and(|(t, at)| at == range.start && t.elapsed() < std::time::Duration::from_millis(1500));
        self.typed = typing.then(|| (std::time::Instant::now(), range.start + new.len()));
        // (A password keeps no copies to undo to.)
        if !self.password && !joins {
            self.undo.push((self.text.clone(), self.selected.clone()));
            if self.undo.len() > UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.text.replace_range(range.clone(), new);
        let at = range.start + new.len();
        self.selected = at..at;
        self.reversed = false;
        self.marked = None;
        self.show_caret(cx);
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn backspace(&mut self, cx: &mut Cx<Self>) {
        if self.selected.is_empty() {
            let from = self.previous_boundary(self.caret());
            self.select_to(from, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete(&mut self, cx: &mut Cx<Self>) {
        if self.selected.is_empty() {
            let to = self.next_boundary(self.caret());
            self.select_to(to, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete_word_left(&mut self, cx: &mut Cx<Self>) {
        if self.selected.is_empty() {
            // (⌘⌫: to the line's start.)
            let from = if crate::keys::mac_command() { self.line_edge(false) } else { self.previous_word(self.caret()) };
            self.select_to(from, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete_word_right(&mut self, cx: &mut Cx<Self>) {
        if self.selected.is_empty() {
            let to = self.next_word(self.caret());
            self.select_to(to, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn left(&mut self, cx: &mut Cx<Self>) {
        let to = if self.selected.is_empty() { self.previous_boundary(self.caret()) } else { self.selected.start };
        self.move_to(to, cx);
    }

    fn right(&mut self, cx: &mut Cx<Self>) {
        let to = if self.selected.is_empty() { self.next_boundary(self.caret()) } else { self.selected.end };
        self.move_to(to, cx);
    }

    /// A word back or on, or with ⌘ (`keys::mac_command`) the line's
    /// start or end.
    fn word_edge(&self, end: bool) -> usize {
        match (crate::keys::mac_command(), end) {
            (true, end) => self.line_edge(end),
            (false, false) => self.previous_word(self.caret()),
            (false, true) => self.next_word(self.caret()),
        }
    }

    fn word_left(&mut self, cx: &mut Cx<Self>) {
        self.move_to(self.word_edge(false), cx);
    }

    fn word_right(&mut self, cx: &mut Cx<Self>) {
        self.move_to(self.word_edge(true), cx);
    }

    fn select_left(&mut self, cx: &mut Cx<Self>) {
        self.select_to(self.previous_boundary(self.caret()), cx);
    }

    fn select_right(&mut self, cx: &mut Cx<Self>) {
        self.select_to(self.next_boundary(self.caret()), cx);
    }

    fn select_word_left(&mut self, cx: &mut Cx<Self>) {
        self.select_to(self.word_edge(false), cx);
    }

    fn select_word_right(&mut self, cx: &mut Cx<Self>) {
        self.select_to(self.word_edge(true), cx);
    }

    fn select_all(&mut self, cx: &mut Cx<Self>) {
        self.move_to(0, cx);
        self.select_to(self.text.len(), cx);
    }

    /// The start or end of the caret's line (a multi-line field's line as
    /// wrapped), else of the text.
    fn line_edge(&self, end: bool) -> usize {
        if !self.multiline {
            return if end { self.text.len() } else { 0 };
        }
        let Some(at) = self.position_of(self.to_shown(self.caret())) else { return 0 };
        let y = at.y + self.line_height.get() / 2.0;
        let i = self.from_shown(self.shown_index_at(point(if end { px(f32::MAX / 4.0) } else { px(0.0) }, y)));
        // A line's end where it wraps is the next line's start: before the
        // space it wrapped at, on the line.
        if end && i < self.text.len() && !self.text[i..].starts_with('\n') && self.position_of(self.to_shown(i)).is_some_and(|p| p.y > at.y) {
            return self.previous_boundary(i);
        }
        i
    }

    fn home(&mut self, cx: &mut Cx<Self>) {
        self.move_to(self.line_edge(false), cx);
    }

    fn end(&mut self, cx: &mut Cx<Self>) {
        self.move_to(self.line_edge(true), cx);
    }

    fn select_home(&mut self, cx: &mut Cx<Self>) {
        self.select_to(self.line_edge(false), cx);
    }

    fn select_end(&mut self, cx: &mut Cx<Self>) {
        self.select_to(self.line_edge(true), cx);
    }

    fn copy(&mut self) {
        if !self.selected.is_empty() && !self.password {
            clipboard::write(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
        }
    }

    fn cut(&mut self, cx: &mut Cx<Self>) {
        if !self.selected.is_empty() && !self.password {
            clipboard::write(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
            self.replace(self.selected.clone(), "", cx);
        }
    }

    fn paste(&mut self, cx: &mut Cx<Self>) {
        self.paste_text(cx);
    }

    /// The clipboard's text put in, once it's read (on a worker:
    /// `clipboard::paste_item`, which sends it back).
    /// Puts what's on the clipboard in, in place of the selection (the
    /// app's own Paste: a menu's, an agent's).
    pub fn paste_from_clipboard(&mut self, cx: &mut Cx<Self>) {
        self.paste_text(cx);
    }

    fn paste_text(&mut self, cx: &mut Cx<Self>) {
        let me = cx.messenger();
        clipboard::paste_item(move |item| {
            if let Some(text) = item.and_then(|item| item.text()) {
                me.send(FieldMessage::Pasted(text));
            }
        });
    }

    /// Pastes `text` as if from the clipboard (the app's Paste command):
    /// in one line, a line break becomes a space.
    pub fn insert(&mut self, text: &str, cx: &mut Cx<Self>) {
        let text = if self.multiline { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.replace(['\n', '\r'], " ") };
        self.replace(self.selected.clone(), &text, cx);
    }

    /// Return. Whether the field took it (not: the window's default
    /// button's).
    fn enter(&mut self, cx: &mut Cx<Self>) -> bool {
        if !self.enabled {
            return true;
        }
        if self.multiline && !self.enter_sends {
            self.replace(self.selected.clone(), "\n", cx);
        } else if !self.multiline && !self.keeps_enter && cx.has_default_button() {
            // As BeOS's: Return is the window's default button's, even
            // with a field to type in (which doesn't see it): the window
            // clicks it once this field has let the key go on.
            return false;
        } else {
            cx.emit(TextFieldEvent::Entered);
        }
        true
    }

    fn newline(&mut self, cx: &mut Cx<Self>) {
        self.replace(self.selected.clone(), "\n", cx);
    }

    /// Up or down a line from the caret (`dir` -1 or 1), keeping to the
    /// column it was in: where that is.
    fn line_step(&mut self, dir: f32) -> usize {
        let Some(at) = self.position_of(self.to_shown(self.caret())) else { return self.caret() };
        let x = *self.goal_x.get_or_insert(at.x);
        let y = at.y + self.line_height.get() * dir + self.line_height.get() / 2.0;
        if y < px(0.0) {
            return 0;
        }
        if y >= self.content_height() {
            return self.text.len();
        }
        self.from_shown(self.shown_index_at(point(x, y)))
    }

    fn up(&mut self, cx: &mut Cx<Self>) {
        let goal = self.goal_x;
        let to = if self.selected.is_empty() { self.line_step(-1.0) } else { self.selected.start };
        self.move_to(to, cx);
        self.goal_x = goal.or(self.goal_x);
    }

    fn down(&mut self, cx: &mut Cx<Self>) {
        let goal = self.goal_x;
        let to = if self.selected.is_empty() { self.line_step(1.0) } else { self.selected.end };
        self.move_to(to, cx);
        self.goal_x = goal.or(self.goal_x);
    }

    fn select_up(&mut self, cx: &mut Cx<Self>) {
        let goal = self.goal_x;
        let to = self.line_step(-1.0);
        self.select_to(to, cx);
        self.goal_x = goal.or(self.goal_x);
    }

    fn select_down(&mut self, cx: &mut Cx<Self>) {
        let goal = self.goal_x;
        let to = self.line_step(1.0);
        self.select_to(to, cx);
        self.goal_x = goal.or(self.goal_x);
    }

    fn doc_start(&mut self, cx: &mut Cx<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, cx: &mut Cx<Self>) {
        self.move_to(self.text.len(), cx);
    }

    /// Where each paragraph starts in what's shown, and its top.
    fn paragraph_tops(&self) -> Vec<(usize, Pixels)> {
        let mut out = Vec::with_capacity(self.paragraphs.borrow().len());
        let (mut start, mut y) = (0, px(0.0));
        for p in self.paragraphs.borrow().iter() {
            out.push((start, y));
            start += p.len() + 1;
            y += self.line_height.get() * (p.wrap_boundaries().len() + 1) as f32;
        }
        out
    }

    fn content_height(&self) -> Pixels {
        self.line_height.get() * self.paragraphs.borrow().iter().map(|p| p.wrap_boundaries().len() + 1).sum::<usize>().max(1) as f32
    }

    /// Where shown byte `i` is in a multi-line field's text (from its top
    /// left, unscrolled): the top of its line.
    fn position_of(&self, i: usize) -> Option<olo_interface::Point<Pixels>> {
        if self.paragraphs.borrow().is_empty() {
            return Some(point(px(0.0), px(0.0)));
        }
        let tops = self.paragraph_tops();
        let k = tops.iter().rposition(|(start, _)| *start <= i)?;
        let (start, top) = tops[k];
        let paragraphs = self.paragraphs.borrow();
        let local = (i - start).min(paragraphs[k].len());
        // (At a wrap, the start of the line after.)
        let p = &paragraphs[k];
        let rows = row_starts(p);
        let row = rows.iter().rposition(|&r| r <= local).unwrap_or(0);
        let x = p.unwrapped_layout.x_for_index(local) - p.unwrapped_layout.x_for_index(rows[row]);
        Some(point(x, top + self.line_height.get() * row as f32))
    }

    /// The shown byte nearest `at` (from the text's top left, unscrolled).
    fn shown_index_at(&self, at: olo_interface::Point<Pixels>) -> usize {
        let tops = self.paragraph_tops();
        let Some(k) = tops.iter().rposition(|(_, top)| *top <= at.y).or((!tops.is_empty()).then_some(0)) else { return 0 };
        let (start, top) = tops[k];
        let local = point(at.x.max(px(0.0)), (at.y - top).max(px(0.0)));
        let paragraphs = self.paragraphs.borrow();
        let i = match paragraphs[k].closest_index_for_position(local, self.line_height.get()) {
            Ok(i) | Err(i) => i,
        };
        start + i.min(paragraphs[k].len())
    }

    /// Cut, Copy, Paste and Select All, from the right button.
    fn edit_menu(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        cx.focus();
        let copyable = !self.selected.is_empty() && !self.password;
        let menu = crate::Menu::new("Edit")
            .item("Cut", "cut")
            .enabled(self.enabled && copyable)
            .keys("⌘X")
            .item("Copy", "copy")
            .enabled(copyable)
            .keys("⌘C")
            .item("Paste", "paste")
            .enabled(self.enabled && clipboard::has_text())
            .keys("⌘V")
            .separator()
            .item("Select All", "select-all")
            .enabled(!self.text.is_empty())
            .keys("⌘A");
        let chosen = cx.listener(|f: &mut Self, cmd: &str, cx| match cmd {
            "cut" if !f.selected.is_empty() && !f.password => {
                clipboard::write(ClipboardItem::new_string(f.text[f.selected.clone()].to_string()));
                f.replace(f.selected.clone(), "", cx);
            }
            "copy" if !f.selected.is_empty() && !f.password => clipboard::write(ClipboardItem::new_string(f.text[f.selected.clone()].to_string())),
            "paste" => f.paste_text(cx),
            "select-all" => f.set_selection(0..f.text.len(), cx),
            _ => {}
        });
        crate::menu::context_menu(cx, menu, e.position, chosen);
        cx.stop_propagation();
    }

    fn undo(&mut self, cx: &mut Cx<Self>) {
        self.typed = None;
        if let Some((text, selected)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.text, text), self.selected.clone()));
            self.selected = selected;
            cx.emit(TextFieldEvent::Changed);
            cx.notify();
        }
    }

    fn redo(&mut self, cx: &mut Cx<Self>) {
        if let Some((text, selected)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.text, text), self.selected.clone()));
            self.selected = selected;
            cx.emit(TextFieldEvent::Changed);
            cx.notify();
        }
    }

    fn index_at(&self, at: olo_interface::Point<Pixels>) -> usize {
        if self.multiline {
            return match self.drawn.get() {
                Some(drawn) => self.from_shown(self.shown_index_at(point(at.x - drawn.origin.x, at.y - drawn.origin.y + self.scroll.get()))),
                None => 0,
            };
        }
        match (self.line.borrow().as_ref(), self.drawn.get()) {
            (Some(line), Some(drawn)) => self.from_shown(line.closest_index_for_x(at.x - drawn.origin.x)),
            _ => 0,
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Cx<Self>) {
        // The click is the field's (the caret, a selection), not also what
        // it's drawn over: Tracker's view under a name being edited took
        // it as a click beside the item and ended the edit (Ben,
        // 2026-11-07). The edit menu's right-click stops here too.
        cx.stop_propagation();
        cx.focus();
        self.selecting = true;
        let at = self.index_at(event.position);
        if event.click_count >= 2 {
            // A word (or, a third time, everything).
            if event.click_count >= 3 {
                self.move_to(0, cx);
                self.select_to(self.text.len(), cx);
            } else {
                let start = self.previous_word(self.next_boundary(at).min(self.text.len()));
                let end = self.next_word(at);
                self.move_to(start, cx);
                self.select_to(end, cx);
            }
            return;
        }
        if event.modifiers.shift {
            self.select_to(at, cx);
        } else {
            self.move_to(at, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Cx<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Cx<Self>) {
        if self.selecting {
            self.select_to(self.index_at(event.position), cx);
        }
    }

    /// The caret shows (after a keystroke or click), and blinks from
    /// there while the field has the keyboard, for [`BLINK_FOR`].
    fn show_caret(&mut self, cx: &mut Cx<Self>) {
        self.caret_on = true;
        if let Some((timer, _)) = self.blink.take() {
            cx.looper().cancel(timer);
        }
        self.blink = Some((cx.every(BLINK, || FieldMessage::Blink), Duration::ZERO));
        // (The input method's window keeps beside the caret, as last
        // drawn.)
        if let Some(at) = self.caret_bounds() {
            let at = cx.to_window(at);
            cx.set_ime_position(at);
        }
    }

    /// Stops blinking (the field lost the keyboard or the window).
    fn stop_blinking(&mut self, cx: &mut Cx<Self>) {
        if let Some((timer, _)) = self.blink.take() {
            cx.looper().cancel(timer);
        }
        self.caret_on = true;
    }

    /// Where the caret is in the window, as last drawn.
    fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        let drawn = self.drawn.get()?;
        if self.multiline {
            let a = self.position_of(self.to_shown(self.caret()))?;
            let o = point(drawn.origin.x, drawn.origin.y - self.scroll.get());
            return Some(Bounds { origin: point(o.x + a.x, o.y + a.y), size: size(px(1.0), self.line_height.get()) });
        }
        let line = self.line.borrow();
        let x = drawn.origin.x + line.as_ref()?.x_for_index(self.to_shown(self.caret()));
        Some(Bounds { origin: point(x, drawn.top()), size: size(px(1.0), drawn.size.height) })
    }

    /// A key: the field's own, by its table.
    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let key = chord(e);
        let area = self.multiline;
        match key.as_str() {
            "backspace" | "shift-backspace" => self.backspace(cx),
            "delete" => self.delete(cx),
            "ctrl-backspace" | "alt-backspace" => self.delete_word_left(cx),
            "ctrl-delete" => self.delete_word_right(cx),
            "left" => self.left(cx),
            "right" => self.right(cx),
            "ctrl-left" | "alt-left" => self.word_left(cx),
            "ctrl-right" | "alt-right" => self.word_right(cx),
            "shift-left" => self.select_left(cx),
            "shift-right" => self.select_right(cx),
            "ctrl-shift-left" | "alt-shift-left" => self.select_word_left(cx),
            "ctrl-shift-right" | "alt-shift-right" => self.select_word_right(cx),
            "ctrl-a" => self.select_all(cx),
            "home" => self.home(cx),
            "end" => self.end(cx),
            "shift-home" => self.select_home(cx),
            "shift-end" => self.select_end(cx),
            "ctrl-c" => self.copy(),
            "ctrl-x" => self.cut(cx),
            "ctrl-v" => self.paste(cx),
            "ctrl-z" => self.undo(cx),
            "ctrl-shift-z" | "ctrl-y" => self.redo(cx),
            "enter" => {
                if !self.enter(cx) {
                    return;
                }
            }
            "up" if area => self.up(cx),
            "down" if area => self.down(cx),
            "shift-up" if area => self.select_up(cx),
            "shift-down" if area => self.select_down(cx),
            "shift-enter" | "alt-enter" if area => self.newline(cx),
            // (⌘↑ and ⌘↓ are left to the window's menu: a chat's composer
            // has Previous and Next Chat there.)
            "ctrl-home" if area => self.doc_start(cx),
            "ctrl-end" if area => self.doc_end(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl TextInput for TextField {
    fn insert_text(&mut self, text: &str, cx: &mut Cx<Self>) {
        let range = self.marked.clone().unwrap_or(self.selected.clone());
        self.replace(range, text, cx);
        if !text.is_empty() {
            cx.emit(TextFieldEvent::Typed(text.to_string().into()));
        }
    }

    fn set_marked_text(&mut self, text: &str, cx: &mut Cx<Self>) {
        let range = self.marked.clone().unwrap_or(self.selected.clone());
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = range.start + text.len()..range.start + text.len();
        cx.notify();
    }

    fn unmark_text(&mut self, _: &mut Cx<Self>) {
        self.marked = None;
    }

    fn delete_marked_text(&mut self, cx: &mut Cx<Self>) {
        if let Some(marked) = self.marked.clone() {
            self.replace(marked, "", cx);
        }
    }
}

impl View for TextField {
    type Message = FieldMessage;

    fn message(&mut self, message: FieldMessage, cx: &mut Cx<Self>) {
        match message {
            FieldMessage::Pasted(text) => self.insert(&text, cx),
            FieldMessage::Blink => {
                let Some((timer, shown)) = self.blink.as_mut() else { return };
                *shown += BLINK;
                let stop = *shown >= BLINK_FOR;
                let timer = *timer;
                // (Shown when it stops.)
                let on = stop || !self.caret_on;
                if self.caret_on != on {
                    self.caret_on = on;
                    cx.notify();
                }
                if stop {
                    cx.looper().cancel(timer);
                    // (Kept, spent: it starts again with the next key.)
                    self.blink = Some((timer, BLINK_FOR));
                }
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // Blink only while it has the keyboard in an active window.
        self.focused.set(cx.is_focused());
        let focused = self.focused.get() && cx.is_window_active();
        if focused && self.blink.is_none() {
            self.show_caret(cx);
        } else if !focused && self.blink.is_some() {
            self.stop_blinking(cx);
        }
        let mut root = div()
            .size_full()
            .cursor(CursorStyle::IBeam)
            .focusable()
            .on_key_down(cx.listener(Self::key_down))
            .on_focus_changed(cx.listener(|_: &mut Self, _, cx| cx.notify()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::edit_menu))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            // (A selection dragged out of the field goes on following
            // the pointer.)
            .on_window_mouse_move(cx.listener(Self::on_mouse_move))
            .agent("TextInput", self.name.clone())
            .agent_value(self.shown())
            .agent_clickable(true)
            .child(cx.canvas(Self::paint).size_full());
        if self.enabled {
            root = root.tab_stop().on_text_input(cx.text_input());
        }
        root
    }
}

impl TextField {
    /// Draws the text, the selection and the caret.
    fn paint(&self, g: &mut Painter, bounds: Bounds<Pixels>) {
        if self.multiline {
            return self.paint_multiline(g, bounds);
        }
        let palette = theme::palette();
        let field = self;
        let color = if field.enabled { field.color.unwrap_or(style::c(palette.document_text)) } else { style::disabled_text(&palette) };
        let text: SharedString = field.shown().into();
        let run = TextRun { len: text.len(), font: field.face(), color, background_color: None, underline: None, strikethrough: None, size: None };
        // Coloured parts (not while composing, or a password's).
        let spans = match (&field.colors, field.marked.is_none() && !field.password && field.enabled) {
            (Some(f), true) => f(&text, &palette),
            _ => Vec::new(),
        };
        let runs = match field.marked.clone().filter(|_| !field.password) {
            None if !spans.is_empty() => {
                let mut out = Vec::new();
                let mut at = 0;
                for (r, ink) in spans {
                    if r.start < at || r.end > text.len() || r.start >= r.end {
                        continue;
                    }
                    if r.start > at {
                        out.push(TextRun { len: r.start - at, ..run.clone() });
                    }
                    out.push(TextRun { len: r.end - r.start, color: ink, ..run.clone() });
                    at = r.end;
                }
                if at < text.len() {
                    out.push(TextRun { len: text.len() - at, ..run.clone() });
                }
                out
            }
            Some(m) => [
                TextRun { len: m.start, ..run.clone() },
                TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(color), thickness: style::hairline(g), wavy: false }), ..run.clone() },
                TextRun { len: text.len() - m.end, ..run.clone() },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            None => vec![run],
        };
        let size_px = field.font_size;
        let line = g.text_system().shape_line(text.clone(), px(size_px), &runs, None);
        let focused = field.focused.get();
        // Wider than the field, it scrolls sideways while it's edited, so
        // the caret stays in view (as BeOS's text views do); else its
        // start shows, and what's cut off its end is in a tip.
        let caret_w = style::hairline(g);
        let over = line.width + caret_w > bounds.size.width;
        let scroll_x = if over && focused {
            let caret_x = line.x_for_index(field.to_shown(field.caret()));
            let most = line.width + caret_w - bounds.size.width;
            let kept = field.scroll_x.get().min(most).max(px(0.0));
            if caret_x < kept {
                caret_x
            } else if caret_x + caret_w > kept + bounds.size.width {
                caret_x + caret_w - bounds.size.width
            } else {
                kept
            }
        } else {
            px(0.0)
        };
        // The line's capitals centred in the field, its baseline on a
        // device pixel; its left at the inset (less the scroll), or its
        // right.
        let left = match field.align {
            Align::Right if !over => bounds.right() - line.width,
            _ => bounds.origin.x - scroll_x,
        };
        let left = style::snap_coord(g, left);
        let baseline = style::snap_coord(g, bounds.center().y + px(size_px * style::CAP_HEIGHT / 2.0));
        let top = baseline - line.ascent;
        let height = line.ascent + line.descent;
        let drawn = Bounds { origin: point(left, top), size: size(line.width, height) };
        let selected = field.to_shown(field.selected.start)..field.to_shown(field.selected.end);
        let (caret, caret_on) = (field.to_shown(field.caret()), field.caret_on && field.enabled);
        let (hint, marked, right) = (field.hint.clone(), field.marked.clone(), field.align == Align::Right);
        let tip = over && !focused && !field.password;
        // (A line longer than the field scrolls across it: what the field
        // cuts is scrolled out of sight, not cut off.)
        crate::review::scroll_area(g, bounds);
        g.with_content_mask(Some(ContentMask { bounds }), |g| {
            if !selected.is_empty() {
                let x0 = left + line.x_for_index(selected.start);
                let x1 = left + line.x_for_index(selected.end);
                let sel = Bounds::from_corners(point(x0, top), point(x1, top + height));
                // The kit's selection: the highlight, see-through.
                style::fill_rect(g, sel, style::selection_behind(&palette, focused));
            }
            let _ = line.paint(point(left, top), height, g);
            crate::review::text(g, drawn, baseline, &text, size_px);
            if text.is_empty() && !hint.is_empty() && marked.is_none() {
                let ink = style::disabled_text(&palette);
                let align = if right { style::Align::Right } else { style::Align::Left };
                let x = if right { bounds.right() } else { bounds.origin.x };
                style::label(g, x, bounds.center().y, align, hint.clone(), size_px, style::ui_font(olo_interface::FontWeight::NORMAL), ink);
            }
            if focused && selected.is_empty() && caret_on {
                let x = style::snap_coord(g, left + line.x_for_index(caret));
                let w = style::hairline(g);
                style::fill_rect(g, Bounds { origin: point(x, top), size: size(w, height) }, style::c(palette.navigation_base));
            }
            // Cut off the field's end (never a password's): whole in a tip.
            if tip {
                g.note_cut(text.clone(), bounds);
            }
        });
        *self.line.borrow_mut() = Some(line);
        self.drawn.set(Some(drawn));
        self.scroll_x.set(scroll_x);
    }

    /// A multi-line field: its paragraphs wrapped to its width from its
    /// top, scrolled to keep the caret in view; the selection behind each
    /// line it covers; the caret a line wide.
    fn paint_multiline(&self, g: &mut Painter, bounds: Bounds<Pixels>) {
        let palette = theme::palette();
        let field = self;
        let color = if field.enabled { field.color.unwrap_or(style::c(palette.document_text)) } else { style::disabled_text(&palette) };
        let text: SharedString = field.shown().into();
        let font = field.face();
        let run = TextRun { len: text.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None, size: None };
        let runs = match field.marked.clone().filter(|_| !field.password) {
            Some(m) => [
                TextRun { len: m.start, ..run.clone() },
                TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(color), thickness: style::hairline(g), wavy: false }), ..run.clone() },
                TextRun { len: text.len() - m.end, ..run.clone() },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            None => vec![run],
        };
        let size_px = field.font_size;
        let lh = TextField::line_height_for(g.scale_factor(), size_px);
        let paragraphs: Vec<olo_interface::WrappedLine> = g.text_system().shape_text(text.clone(), px(size_px), &runs, Some(bounds.size.width), None).map(|l| l.into_vec()).unwrap_or_default();
        let focused = field.focused.get();
        let (caret, caret_on) = (field.to_shown(field.caret()), field.caret_on && field.enabled);
        let selected = field.to_shown(field.selected.start)..field.to_shown(field.selected.end);
        let (hint, marked, scroll) = (field.hint.clone(), field.marked.clone(), field.scroll.get());
        // Laid out: where the caret is, and the scroll that keeps it in view.
        *field.paragraphs.borrow_mut() = paragraphs.clone();
        field.line_height.set(lh);
        field.drawn.set(Some(bounds));
        let caret_at = field.position_of(caret).unwrap_or_default();
        let content = field.content_height();
        let view = bounds.size.height;
        let mut scroll = scroll.min((content - view).max(px(0.0)));
        if caret_at.y < scroll {
            scroll = caret_at.y;
        } else if caret_at.y + lh > scroll + view {
            scroll = caret_at.y + lh - view;
        }
        let s = g.scale_factor();
        let scroll = px((f32::from(scroll) * s).round() / s);
        let tops = field.paragraph_tops();
        // The selection: each line it covers, from where it starts on that
        // line to where it ends.
        let mut bands = Vec::new();
        if !selected.is_empty() {
            for (k, p) in paragraphs.iter().enumerate() {
                let (start, top) = tops[k];
                let rows = row_starts(p);
                for (r, &rs) in rows.iter().enumerate() {
                    let re = rows.get(r + 1).copied().unwrap_or(p.len());
                    let (a, b) = (start + rs, start + re);
                    // (A line's end counts its newline, so a selection across
                    // paragraphs shows the break.)
                    let b_nl = if r + 1 == rows.len() && k + 1 < paragraphs.len() { b + 1 } else { b };
                    let (lo, hi) = (selected.start.max(a), selected.end.min(b_nl));
                    if lo > hi || (lo == hi && !(lo == a && hi < b_nl)) {
                        continue;
                    }
                    let x = |i: usize| p.unwrapped_layout.x_for_index(i.min(start + p.len()) - start) - p.unwrapped_layout.x_for_index(rs);
                    let x1 = if hi > b { x(b) + px(4.0) } else { x(hi) };
                    let y = top + lh * r as f32;
                    bands.push(Bounds::from_corners(point(x(lo), y), point(x1, y + lh)));
                }
            }
        }
        let origin = point(bounds.origin.x, bounds.origin.y - scroll);
        g.with_content_mask(Some(ContentMask { bounds }), |g| {
            let ink = style::selection_behind(&palette, focused);
            for b in bands {
                style::fill_rect(g, Bounds { origin: point(origin.x + b.origin.x, origin.y + b.origin.y), size: b.size }, ink);
            }
            for (k, p) in paragraphs.iter().enumerate() {
                let _ = p.paint(point(origin.x, origin.y + tops[k].1), lh, olo_interface::TextAlign::Left, None, g);
            }
            if text.is_empty() && !hint.is_empty() && marked.is_none() {
                // Drawn as the first row's text is (a shaped line, in its
                // height), so the first letter typed lands where the hint
                // was: drawn as a label (its capitals centred) it sat a device
                // pixel higher at 1.5×, and its line stood out of the field.
                let ink = style::disabled_text(&palette);
                let hint = SharedString::from(hint.to_string());
                let run = TextRun { len: hint.len(), font: font.clone(), color: ink, background_color: None, underline: None, strikethrough: None, size: None };
                let line = g.text_system().shape_line(hint.clone(), px(size_px), &[run], None);
                let _ = line.paint(origin, lh, g);
                let h = line.ascent + line.descent;
                let top = origin.y + (lh - h) / 2.0;
                crate::review::text(g, Bounds { origin: point(origin.x, top), size: size(line.width, h) }, top + line.ascent, &hint, size_px);
            }
            if focused && selected.is_empty() && caret_on {
                let line = g.text_system().shape_line(
                    SharedString::from(" "),
                    px(size_px),
                    &[TextRun { len: 1, font: font.clone(), color, background_color: None, underline: None, strikethrough: None, size: None }],
                    None,
                );
                let h = line.ascent + line.descent;
                // (Spaces after a row's last word hang past its edge; the
                // caret among them stays at the edge.)
                let x = style::snap_coord(g, (origin.x + caret_at.x).min(bounds.right() - style::hairline(g)));
                let y = style::snap_coord(g, origin.y + caret_at.y + (lh - h) / 2.0);
                style::fill_rect(g, Bounds { origin: point(x, y), size: size(style::hairline(g), h) }, style::c(palette.navigation_base));
            }
        });
        self.scroll.set(scroll);
    }
}

/// Where each line of a wrapped paragraph starts (bytes in it).
fn row_starts(p: &WrappedLine) -> Vec<usize> {
    let layout = &p.unwrapped_layout;
    std::iter::once(0).chain(p.wrap_boundaries().iter().map(|b| layout.runs[b.run_ix].glyphs[b.glyph_ix].index)).collect()
}

/// A field with its `font_size` and so on set, made a view its maker
/// owns.
pub fn text_field<V: View>(cx: &mut Cx<V>, build: impl FnOnce(TextField) -> TextField) -> Child<TextField> {
    cx.new_view(|_| build(TextField::new()))
}
