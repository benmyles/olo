//! A one-line text field: typing (with input methods), the caret and the
//! selection by keyboard and pointer, words, the clipboard and undo. No
//! frame of its own: the caller draws the well around it (a
//! `style::sunken`, DeskCalc's display), so it fits any of them. The
//! caret is a line wide (a device pixel at 1× and 1.5×), on device
//! pixels, and blinks only while the field has the keyboard in an
//! active window. Agents read it as a `TextInput` with its text.

use std::{ops::Range, time::Duration};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, ShapedLine, SharedString, Style, Styled as _, Task, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, point, px, relative, size,
};
use gpui::{AppContext as _, InteractiveElement as _};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::{agent, style, theme};

actions!(
    olo_text_field,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Enter,
    ]
);

const CONTEXT: &str = "OloTextField";

/// The field's keys (the command key reaches apps as Ctrl).
pub(crate) fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("ctrl-a", SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("ctrl-c", Copy, c),
        KeyBinding::new("ctrl-x", Cut, c),
        KeyBinding::new("ctrl-v", Paste, c),
        KeyBinding::new("ctrl-z", Undo, c),
        KeyBinding::new("ctrl-shift-z", Redo, c),
        KeyBinding::new("ctrl-y", Redo, c),
        KeyBinding::new("enter", Enter, c),
    ]);
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
/// How many steps undo keeps.
const UNDO: usize = 100;

pub struct TextField {
    focus: FocusHandle,
    text: String,
    selected: Range<usize>,
    /// The selection grows from its end, back towards its start.
    reversed: bool,
    /// What an input method is composing.
    marked: Option<Range<usize>>,
    font_size: f32,
    color: Option<Hsla>,
    align: Align,
    name: SharedString,
    undo: Vec<(String, Range<usize>)>,
    redo: Vec<(String, Range<usize>)>,
    line: Option<ShapedLine>,
    /// Where the text was drawn (its left, the line's top and height).
    drawn: Option<Bounds<Pixels>>,
    selecting: bool,
    caret_on: bool,
    blink: Option<Task<()>>,
    /// Shown as bullets, never copied (a password).
    password: bool,
    /// Shown dim while it's empty ("Password").
    hint: SharedString,
    /// Can't be typed in now (a password while PAM checks the last).
    enabled: bool,
    /// Its text's colours, from its text (a search's filters).
    colors: Option<Colors>,
}

/// Colours for parts of a field's text (byte ranges, in order), in the
/// palette given; the rest in its colour.
pub type Colors = std::rc::Rc<dyn Fn(&str, &olo_theme::Palette) -> Vec<(Range<usize>, Hsla)>>;

/// What a password field shows for each character.
const BULLET: char = '•';

impl EventEmitter<TextFieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextField {
    pub fn new(cx: &mut Context<Self>) -> TextField {
        TextField {
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
            selected: 0..0,
            reversed: false,
            marked: None,
            font_size: style::PLAIN_SIZE,
            color: None,
            align: Align::Left,
            name: SharedString::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            line: None,
            drawn: None,
            selecting: false,
            caret_on: true,
            blink: None,
            password: false,
            enabled: true,
            hint: SharedString::default(),
            colors: None,
        }
    }

    /// What it shows, dim, while it's empty.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Shows bullets for its characters and never copies them.
    pub fn password(mut self, on: bool) -> Self {
        self.password = on;
        self
    }

    /// Bullets or the text (PAM asks a password, or a name to echo).
    pub fn set_password(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.password != on {
            self.password = on;
            cx.notify();
        }
    }

    /// Whether it takes typing and the keyboard (drawn dim when not).
    pub fn set_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.enabled != on {
            self.enabled = on;
            // Tab passes over a field that can't be used.
            self.focus = self.focus.clone().tab_stop(on);
            cx.notify();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Forgets the text, leaving no copy in freed memory (a password,
    /// once used): no undo of it either.
    pub fn clear_secret(&mut self, cx: &mut Context<Self>) {
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

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the text (from the app: no undo step), the caret at the
    /// end.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text != self.text {
            self.text = text;
            self.marked = None;
        }
        self.move_to(self.text.len(), cx);
        cx.notify();
    }

    /// The caret to the end.
    pub fn caret_to_end(&mut self, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Gives it the keyboard.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// The selection (bytes; empty: the caret).
    pub fn selection(&self) -> Range<usize> {
        self.selected.clone()
    }

    /// Selects `range` (bytes, on characters' boundaries).
    pub fn set_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let len = self.text.len();
        self.selected = range.start.min(len)..range.end.min(len);
        self.reversed = false;
        self.show_caret(cx);
        cx.notify();
    }

    /// Selects all its text (Find: what was looked for, to type over).
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.set_selection(0..self.text.len(), cx);
    }

    /// Colours parts of its text ([`Colors`]; None: all in its colour).
    pub fn set_colors(&mut self, colors: Option<Colors>, cx: &mut Context<Self>) {
        self.colors = colors;
        cx.notify();
    }

    fn caret(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        self.show_caret(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
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
    fn replace(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        // (A password keeps no copies to undo to.)
        if !self.password {
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

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let from = self.previous_boundary(self.caret());
            self.select_to(from, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let to = self.next_boundary(self.caret());
            self.select_to(to, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let from = self.previous_word(self.caret());
            self.select_to(from, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let to = self.next_word(self.caret());
            self.select_to(to, cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected.is_empty() { self.previous_boundary(self.caret()) } else { self.selected.start };
        self.move_to(to, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected.is_empty() { self.next_boundary(self.caret()) } else { self.selected.end };
        self.move_to(to, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.caret()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.caret()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.caret()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.caret()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.caret()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.caret()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.text.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.text.len(), cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() && !self.password {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() && !self.password {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
            self.replace(self.selected.clone(), "", cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // One line: a pasted line break becomes a space.
            let text = text.replace(['\n', '\r'], " ");
            self.replace(self.selected.clone(), &text, cx);
        }
    }

    /// Pastes `text` as if from the clipboard (the app's Paste command).
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.replace(['\n', '\r'], " ");
        self.replace(self.selected.clone(), &text, cx);
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.enabled {
            cx.emit(TextFieldEvent::Entered);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, selected)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.text, text), self.selected.clone()));
            self.selected = selected;
            cx.emit(TextFieldEvent::Changed);
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, selected)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.text, text), self.selected.clone()));
            self.selected = selected;
            cx.emit(TextFieldEvent::Changed);
            cx.notify();
        }
    }

    fn index_at(&self, x: Pixels) -> usize {
        match (&self.line, &self.drawn) {
            (Some(line), Some(drawn)) => self.from_shown(line.closest_index_for_x(x - drawn.origin.x)),
            _ => 0,
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.selecting = true;
        let at = self.index_at(event.position.x);
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

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_at(event.position.x), cx);
        }
    }

    /// The caret shows (after a keystroke or click), and blinks from
    /// there while the field has the keyboard.
    fn show_caret(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let keep = this
                    .update(cx, |field, cx| {
                        field.caret_on = !field.caret_on;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    /// Stops blinking (the field lost the keyboard or the window).
    fn stop_blinking(&mut self) {
        self.blink = None;
        self.caret_on = true;
    }

    fn utf16_to_offset(&self, utf16: usize) -> usize {
        let mut seen = 0;
        for (i, ch) in self.text.char_indices() {
            if seen >= utf16 {
                return i;
            }
            seen += ch.len_utf16();
        }
        self.text.len()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())].chars().map(char::len_utf16).sum()
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_offset(r.start)..self.utf16_to_offset(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let range = self.range_from_utf16(&range);
        *actual = Some(self.range_to_utf16(&range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = range.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.replace(range, text, cx);
        if !text.is_empty() {
            cx.emit(TextFieldEvent::Typed(text.to_string().into()));
        }
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let range = range.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = selected
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.start)
            .unwrap_or_else(|| range.start + text.len()..range.start + text.len());
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let (line, drawn) = (self.line.as_ref()?, self.drawn?);
        let range = self.range_from_utf16(&range);
        let x = |i: usize| drawn.origin.x + line.x_for_index(self.to_shown(i));
        Some(Bounds::from_corners(point(x(range.start), drawn.top()), point(x(range.end), drawn.bottom())))
    }

    fn character_index_for_point(&mut self, p: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let at = self.index_at(p.x);
        Some(self.offset_to_utf16(at))
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // Blink only while it has the keyboard in an active window.
        let focused = self.focus.is_focused(window) && window.is_window_active();
        if focused && self.blink.is_none() {
            self.show_caret(cx);
        } else if !focused && self.blink.is_some() {
            self.stop_blinking();
        }
        div()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(FieldElement { field: cx.entity() })
    }
}

/// Draws the text, the selection and the caret, and takes typing.
struct FieldElement {
    field: Entity<TextField>,
}

impl IntoElement for FieldElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut Window, _: &mut App) {}

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        let palette = theme::palette(cx);
        let field = self.field.read(cx);
        let color = if field.enabled { field.color.unwrap_or(style::c(palette.document_text)) } else { style::disabled_text(&palette) };
        let text: SharedString = field.shown().into();
        let run = TextRun { len: text.len(), font: style::ui_font(gpui::FontWeight::NORMAL), color, background_color: None, underline: None, strikethrough: None };
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
                TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(color), thickness: style::hairline(window), wavy: false }), ..run.clone() },
                TextRun { len: text.len() - m.end, ..run.clone() },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            None => vec![run],
        };
        let size_px = field.font_size;
        let line = window.text_system().shape_line(text.clone(), px(size_px), &runs, None);
        // The line's capitals centred in the field, its baseline on a
        // device pixel; its left at the inset, or its right.
        let left = match field.align {
            Align::Left => bounds.origin.x,
            Align::Right => bounds.right() - line.width,
        };
        let left = style::snap_coord(window, left);
        let baseline = style::snap_coord(window, bounds.center().y + px(size_px * style::CAP_HEIGHT / 2.0));
        let top = baseline - line.ascent;
        let height = line.ascent + line.descent;
        let drawn = Bounds { origin: point(left, top), size: size(line.width, height) };
        let focused = field.focus.is_focused(window);
        let selected = field.to_shown(field.selected.start)..field.to_shown(field.selected.end);
        let (caret, caret_on) = (field.to_shown(field.caret()), field.caret_on && field.enabled);
        let name = field.name.clone();
        let (hint, marked, right) = (field.hint.clone(), field.marked.clone(), field.align == Align::Right);
        if field.enabled {
            window.handle_input(&field.focus.clone(), ElementInputHandler::new(bounds, self.field.clone()), cx);
        }
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if !selected.is_empty() {
                let x0 = left + line.x_for_index(selected.start);
                let x1 = left + line.x_for_index(selected.end);
                let sel = Bounds::from_corners(point(x0, top), point(x1, top + height));
                // The kit's selection: the highlight, see-through.
                let dark = palette.mode == olo_theme::ThemeMode::Dark;
                let ink = style::c(palette.control_highlight).opacity(if !focused { 0.3 } else if dark { 0.75 } else { 0.55 });
                style::fill_rect(window, sel, ink);
            }
            let _ = line.paint(point(left, top), height, window, cx);
            crate::review::text(window, drawn, baseline, &text, size_px);
            if text.is_empty() && !hint.is_empty() && marked.is_none() {
                let ink = style::disabled_text(&palette);
                let align = if right { style::Align::Right } else { style::Align::Left };
                let x = if right { bounds.right() } else { bounds.origin.x };
                style::label(window, cx, x, bounds.center().y, align, hint.clone(), size_px, style::ui_font(gpui::FontWeight::NORMAL), ink);
            }
            if focused && selected.is_empty() && caret_on {
                let x = style::snap_coord(window, left + line.x_for_index(caret));
                let w = style::hairline(window);
                style::fill_rect(window, Bounds { origin: point(x, top), size: size(w, height) }, style::c(palette.navigation_base));
            }
        });
        agent::record(window, cx, agent::Node { role: "TextInput", name: (!name.is_empty()).then_some(name), value: Some(text), bounds, enabled: true, clickable: true, focused, ..Default::default() });
        self.field.update(cx, |f, _| {
            f.line = Some(line);
            f.drawn = Some(drawn);
        });
    }
}

/// A field with its `font_size` and so on set, made into an entity.
pub fn text_field(cx: &mut App, build: impl FnOnce(TextField) -> TextField) -> Entity<TextField> {
    cx.new(|cx| build(TextField::new(cx)))
}
