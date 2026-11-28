//! DeskCalc: BeOS's desk calculator. An expression line over a keypad
//! that grows with the window: R5's basic keys, Haiku's scientific rows
//! above them, or no keypad at all (compact). Type or click; Enter or =
//! works it out (expr.rs); ↑ and ↓ step through earlier expressions. The
//! keypad's context menu holds the options, as in Haiku. On Olo's kit
//! (olo-kit): each key is an element of its own, so the pointer coming
//! over one draws that key again and nothing else.

mod expr;

use std::time::{Duration, Instant};

use expr::Angle;
use olo_kit::{
    Bounds, Child, ClipboardItem, Cx, Element, FontWeight, InteractiveElement as _, KeyDownEvent, Menu, MouseButton, MouseDownEvent, Painter, ParentElement as _, Pixels, Styled as _, View, WindowCx,
    WindowOptions, agent, canvas, clipboard, control_canvas, div,
    instance::{self, Launch},
    px, size,
    style::{self, Align},
    theme,
    widgets::{TextField, TextFieldEvent, text_field},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.deskcalc";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Keypad {
    Compact,
    Basic,
    Scientific,
}

/// R5's keypad; Haiku's scientific rows go above it.
const BASIC: [[&str; 5]; 4] = [["7", "8", "9", "(", ")"], ["4", "5", "6", "×", "÷"], ["1", "2", "3", "+", "−"], ["0", ".", "⌫", "=", "C"]];
const SCIENTIFIC: [[&str; 5]; 4] = [["ln", "sin", "cos", "tan", "π"], ["log", "asin", "acos", "atan", "sqrt"], ["exp", "sinh", "cosh", "tanh", "cbrt"], ["!", "ceil", "floor", "E", "^"]];

const WIDTH: f32 = 266.0;
// (The kit's rhythm: the window's inset round it all, keys `TIGHT` apart.)
const INSET: f32 = olo_kit::metrics::WINDOW_INSET;
const GAP: f32 = olo_kit::metrics::TIGHT;
/// The display: a sunken well of two lines, each right-aligned, each
/// on its own baseline. The first, small and secondary, says what was
/// worked out ("12×3 =", or the expression that couldn't be); the
/// second, large, is the line being typed, or its result, or, in its
/// place, why it couldn't be worked out. (An error in the corner of one
/// line, the expression beside it, was the smallest text in the window,
/// on another baseline, a glyph cut where they met: Opus's look,
/// 2026-11-28.)
const DISPLAY_H: f32 = 52.0;
/// From the display's sides to what's in it.
const DISPLAY_PAD: f32 = 8.0;
/// The first line: its size, and its middle from the well's top.
const WORKED_SIZE: f32 = 12.0;
const WORKED_MID: f32 = 12.5;
/// The second line: the field's size, where it starts from the well's
/// top and how tall it is; an error there, its size.
const LINE_SIZE: f32 = 20.0;
const LINE_TOP: f32 = 21.5;
const LINE_H: f32 = 28.0;
const ERROR_SIZE: f32 = 15.0;

/// Whether a key moves in the line (to put an expression right): it
/// puts an error away, showing the line again.
fn dismisses_error(key: &str) -> bool {
    matches!(key, "left" | "right" | "home" | "end")
}

/// `text` as it fits `room` points across at `size` in `font`: whole, or
/// its end with "…" for the rest, cut between characters, never through
/// one (the end is what was typed last).
fn end_fit(text: &str, size: f32, font: &olo_kit::Font, room: f32) -> String {
    let wide = |t: &str| f32::from(style::text_width(t.to_owned(), size, font.clone()));
    if wide(text) <= room {
        return text.to_owned();
    }
    // The longest end that fits after "…" (by halves: wider the more is kept).
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
    let (mut lo, mut hi) = (0, starts.len() - 1);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if wide(&format!("…{}", &text[starts[mid]..])) <= room { hi = mid } else { lo = mid + 1 }
    }
    format!("…{}", &text[starts[lo]..])
}
const KEY_H: f32 = 34.0;
/// How long a key stays down when its character is typed.
const FLASH: Duration = Duration::from_millis(120);

impl Keypad {
    fn rows(self) -> Vec<[&'static str; 5]> {
        match self {
            Keypad::Compact => Vec::new(),
            Keypad::Basic => BASIC.to_vec(),
            Keypad::Scientific => SCIENTIFIC.iter().chain(BASIC.iter()).copied().collect(),
        }
    }

    /// The window size that shows it at its natural key size.
    fn size(self) -> (f32, f32) {
        let rows = self.rows().len() as f32;
        let keys = if rows > 0.0 { INSET + rows * KEY_H + (rows - 1.0) * GAP } else { 0.0 };
        (WIDTH, (INSET + DISPLAY_H + INSET + keys).round())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    keypad: Keypad,
    angle: Angle,
    expression: String,
    history: Vec<String>,
    /// What the result `expression` holds was worked out from, while it
    /// shows it ("12×3", said over it as "12×3 ="); none otherwise, and
    /// none from settings saved before (taken from the history, it put the
    /// last success over a failure: review, 2026-11-28).
    worked: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { keypad: Keypad::Basic, angle: Angle::Radians, expression: String::new(), history: Vec::new(), worked: None }
    }
}

fn settings_path() -> std::path::PathBuf {
    olo_kit::config_dir().join("deskcalc.json")
}

/// How many earlier expressions ↑ remembers.
const HISTORY: usize = 50;

struct DeskCalc {
    settings: Settings,
    field: Child<TextField>,
    /// Why the last = failed, until the next edit.
    error: Option<String>,
    /// The expression the result showing was worked out from (said over
    /// it, "12×3 =").
    worked: Option<String>,
    /// The display shows a result: a digit starts afresh, an operator
    /// carries on from it.
    shows_result: bool,
    /// Where ↑/↓ is in the history (None: the line being typed).
    recall: Option<usize>,
    /// A key drawn down for a moment because its character was typed.
    flash: Option<(&'static str, Instant)>,
}

#[derive(Clone)]
enum Msg {
    /// A keypad key clicked.
    Key(&'static str),
    /// A flashed key's moment is up.
    Flashed,
    /// A command chosen from the context menu.
    Command(String),
}

impl DeskCalc {
    fn new(cx: &mut Cx<Self>) -> DeskCalc {
        let settings: Settings = olo_kit::store::load(&settings_path()).unwrap_or_default();
        let field = text_field(cx, |f| f.font_size(LINE_SIZE).align(olo_kit::widgets::text_field::Align::Right).name("Expression").with_text(settings.expression.clone()));
        // (The field changes as the expression does.)
        cx.subscribe(&field, Self::field_event);
        // The display always has the keyboard.
        field.focus(cx);
        // What was left showing last time counts as a result.
        let shows_result = !settings.expression.is_empty();
        let worked = settings.worked.clone().filter(|_| shows_result);
        DeskCalc { settings, field, error: None, worked, shows_result, recall: None, flash: None }
    }

    fn store(&mut self) {
        // (What it was worked out from only while the result shows.)
        self.settings.worked = self.worked.clone().filter(|_| self.shows_result);
        olo_kit::store::save(settings_path(), &self.settings);
    }

    /// The line edited (by hand, a key, ↑): no longer a result, nor
    /// worked out from anything; no error.
    fn edited(&mut self) {
        self.shows_result = false;
        self.worked = None;
        self.error = None;
    }

    /// Shows `text` in the display, the caret at its end.
    fn show(&mut self, text: String, cx: &mut Cx<Self>) {
        self.settings.expression = text.clone();
        self.field.update(cx, |f, cx| f.set_text(text, cx));
    }

    /// The line made `text` as a step Undo takes back (a key pressed,
    /// Clear, a result), as typing in it is.
    fn put(&mut self, text: String, cx: &mut Cx<Self>) {
        self.settings.expression = text.clone();
        self.field.update(cx, |f, cx| f.replace_all(&text, cx));
    }

    fn field_event(&mut self, event: &TextFieldEvent, cx: &mut Cx<Self>) {
        match event {
            TextFieldEvent::Typed(typed) => {
                if let Some(ch) = typed.chars().last()
                    && let Some(k) = key_for(ch)
                {
                    self.flash_key(k, cx);
                }
                // Typing a digit over a result starts a new expression.
                let starts = typed.chars().next().is_some_and(|ch| ch.is_ascii_digit() || ch == '.' || ch == '(');
                if self.shows_result && starts {
                    self.put(typed.to_string(), cx);
                }
                self.shows_result = false;
                self.error = None;
            }
            TextFieldEvent::Changed => {
                let text = self.field.read(cx).text().to_owned();
                // (Changed by hand, a key deleting say: the error's gone
                // with what it was said of, and a result edited is one no
                // longer: "12×3 =" stayed over the 3 left of 36.)
                if text != self.settings.expression {
                    self.edited();
                }
                self.settings.expression = text;
                self.recall = None;
            }
            // (Return in the field: as = .)
            TextFieldEvent::Entered => {
                self.evaluate(cx);
                self.flash_key("=", cx);
            }
        }
        cx.invalidate();
    }

    fn flash_key(&mut self, key: &'static str, cx: &mut Cx<Self>) {
        self.flash = Some((key, Instant::now()));
        cx.after(FLASH, Msg::Flashed);
        cx.invalidate();
    }

    fn evaluate(&mut self, cx: &mut Cx<Self>) {
        let text = self.settings.expression.trim().to_owned();
        if text.is_empty() {
            return;
        }
        match expr::eval(&text, self.settings.angle) {
            Ok(v) => {
                self.settings.history.retain(|h| *h != text);
                self.settings.history.push(text.clone());
                let over = self.settings.history.len().saturating_sub(HISTORY);
                self.settings.history.drain(..over);
                self.worked = Some(text);
                self.put(expr::format(v), cx);
                self.shows_result = true;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
                self.worked = None;
            }
        }
        self.recall = None;
        self.store();
        cx.invalidate();
    }

    /// A keypad key.
    fn press(&mut self, key: &str, cx: &mut Cx<Self>) {
        let mut e = self.settings.expression.clone();
        let starts_number = key.chars().next().is_some_and(|ch| ch.is_ascii_digit() || ch == '.' || ch == 'π' || ch == '(')
            || matches!(key, "ln" | "log" | "exp" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "sqrt" | "cbrt" | "ceil" | "floor");
        if self.shows_result && starts_number {
            e.clear();
        }
        self.shows_result = false;
        self.error = None;
        match key {
            "=" => return self.evaluate(cx),
            "C" => e.clear(),
            "⌫" => {
                e.pop();
            }
            "E" => e.push('E'),
            k if k.len() > 1 && k.chars().all(char::is_alphabetic) => {
                e.push_str(k);
                e.push('(');
            }
            k => e.push_str(k),
        }
        self.recall = None;
        self.put(e, cx);
        cx.invalidate();
    }

    /// ↑ / ↓ through the history.
    fn step_history(&mut self, back: bool, cx: &mut Cx<Self>) {
        let h = &self.settings.history;
        if h.is_empty() {
            return;
        }
        let next = match (self.recall, back) {
            (None, true) => Some(h.len() - 1),
            (None, false) => None,
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < h.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        let text = next.map(|i| h[i].clone()).unwrap_or_default();
        self.show(text, cx);
        self.recall = next;
        self.shows_result = false;
        self.error = None;
        cx.invalidate();
    }

    /// The keypad's context menu, as in Haiku (Undo and Redo first: the
    /// line's, keys and results too).
    fn menu(&self, cx: &WindowCx) -> Menu {
        let s = &self.settings;
        let f = self.field.read(cx);
        Menu::new("DeskCalc")
            .item("Undo", "undo")
            .enabled(f.can_undo())
            .item("Redo", "redo")
            .enabled(f.can_redo())
            .separator()
            .item("Copy", "copy")
            .item("Paste", "paste")
            .separator()
            .item("Basic Keypad", "keypad-basic")
            .checked(s.keypad == Keypad::Basic)
            .item("Scientific Keypad", "keypad-scientific")
            .checked(s.keypad == Keypad::Scientific)
            .item("No Keypad", "keypad-compact")
            .checked(s.keypad == Keypad::Compact)
            .separator()
            .item("Radians", "radians")
            .checked(s.angle == Angle::Radians)
            .item("Degrees", "degrees")
            .checked(s.angle == Angle::Degrees)
    }
}

/// The key a typed character stands for, to flash it.
fn key_for(ch: char) -> Option<&'static str> {
    let s = match ch {
        '*' | '×' => "×",
        '/' | '÷' => "÷",
        '-' | '−' => "−",
        '^' => "^",
        '!' => "!",
        'E' => "E",
        _ => return BASIC.iter().flatten().find(|k| k.len() == 1 && k.starts_with(ch)).copied(),
    };
    Some(s)
}

impl View for DeskCalc {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Key(key) => self.press(key, cx),
            Msg::Flashed => {
                if self.flash.is_some_and(|(_, at)| at.elapsed() >= FLASH) {
                    self.flash = None;
                    cx.invalidate();
                }
            }
            Msg::Command(command) => {
                self.command(&command, cx);
            }
        }
    }

    /// The menu commands (the context menu's; the command palette's and
    /// agents').
    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        let keypad = match command {
            "undo" | "redo" => {
                self.field.update(cx, |f, cx| if command == "undo" { f.undo_step(cx) } else { f.redo_step(cx) });
                self.shows_result = false;
                return true;
            }
            "copy" => {
                clipboard::write(ClipboardItem::new_string(self.settings.expression.clone()));
                return true;
            }
            "paste" => {
                self.field.update(cx, |f, cx| f.paste_from_clipboard(cx));
                return true;
            }
            "keypad-basic" => Keypad::Basic,
            "keypad-scientific" => Keypad::Scientific,
            "keypad-compact" => Keypad::Compact,
            "radians" | "degrees" => {
                self.settings.angle = if command == "radians" { Angle::Radians } else { Angle::Degrees };
                self.store();
                cx.invalidate();
                return true;
            }
            _ => return false,
        };
        if keypad != self.settings.keypad {
            self.settings.keypad = keypad;
            let (_, h) = keypad.size();
            let width = cx.viewport_size().width;
            cx.resize_window(size(width, px(h)));
            self.store();
            cx.invalidate();
        }
        true
    }

    /// Everything worth keeping is in the settings: stored before
    /// becoming the new version.
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        let text = serde_json::to_string_pretty(&self.settings).unwrap_or_default();
        let _ = std::fs::write(settings_path(), text);
        None
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // (The command palette's and agents' way to the menu's commands.)
        olo_kit::menu::offer(self, cx, |v, cx| vec![v.menu(cx)]);
        let rows = self.settings.keypad.rows();
        let flashing = self.flash.filter(|(_, t)| t.elapsed() < FLASH).map(|(k, _)| k);
        // The display (`DISPLAY_H`): what was worked out over the line,
        // and the line, or why it couldn't be worked out in its place (the
        // field kept, with the keyboard, under it: an edit puts it back).
        let error = self.error.clone();
        let worked = match (&error, &self.worked) {
            (Some(_), _) => Some(self.settings.expression.clone()),
            (None, Some(w)) if self.shows_result => Some(format!("{w} =")),
            _ => None,
        };
        let well = canvas({
            let (error, worked) = (error.clone(), worked.clone());
            move |g, b| {
                let p = theme::palette();
                style::fill_rect(g, b, style::c(p.document_background));
                style::sunken(g, b, &p, false);
                let (font, right, room) = (style::ui_font(FontWeight::NORMAL), b.right() - px(DISPLAY_PAD), b.size.width - px(2.0 * DISPLAY_PAD));
                if let Some(w) = &worked {
                    let shown = end_fit(w, WORKED_SIZE, &font, f32::from(room));
                    let mid = style::snap_coord(g, b.origin.y + px(WORKED_MID));
                    let r = style::label(g, right, mid, Align::Right, shown, WORKED_SIZE, font.clone(), style::secondary_text(&p));
                    g.agent(agent::Node::label(w.clone(), r));
                }
                if let Some(e) = &error {
                    let mid = style::snap_coord(g, b.origin.y + px(LINE_TOP + LINE_H / 2.0));
                    let r = style::label_fit(g, right, mid, Align::Right, e.clone(), ERROR_SIZE, font.clone(), style::c(p.failure), room);
                    // (Whole for agents, though cut short on screen.)
                    g.agent(agent::Node::label(e.clone(), r));
                }
            }
        })
        .paint_key(style::key((&error, &worked)))
        .absolute()
        .inset_0();
        let line = div().absolute().left(px(DISPLAY_PAD)).right(px(DISPLAY_PAD)).top(px(LINE_TOP)).h(px(LINE_H)).child(&self.field);
        // (An error goes as soon as the line is gone back to, to be put
        // right: a click on the display, an arrow moving in it; the line,
        // hidden under it, was being edited unseen.)
        let display = div()
            .absolute()
            .left(px(INSET))
            .top(px(INSET))
            .right(px(INSET))
            .h(px(DISPLAY_H))
            .child(well)
            .child(if error.is_some() { line.opacity(0.0) } else { line })
            .on_mouse_down(
                olo_kit::MouseButton::Left,
                cx.listener(|c: &mut DeskCalc, _: &olo_kit::MouseDownEvent, cx| {
                    if c.error.take().is_some() {
                        cx.invalidate();
                    }
                }),
            )
            .capture_key_down(cx.listener(|c: &mut DeskCalc, e: &olo_kit::KeyDownEvent, cx| {
                if dismisses_error(&e.keystroke.key) && c.error.take().is_some() {
                    cx.invalidate();
                }
            }));

        // The keypad fills the rest: rows of keys, each as wide and as
        // tall as the others, `GAP` apart.
        let n = rows.len();
        let keypad = div().absolute().left(px(INSET)).right(px(INSET)).top(px(INSET + DISPLAY_H + INSET)).bottom(px(INSET)).flex().flex_col().gap(px(GAP)).children(rows.into_iter().map(|row| {
            div().flex_1().min_h_0().flex().gap(px(GAP)).children(row.into_iter().map(|label| {
                let flashed = flashing == Some(label);
                control_canvas(move |g, b, state| {
                    // (As big as the keys' height allows: all the same.)
                    let font_size = (f32::from(b.size.height) * 0.42).clamp(10.0, 28.0).round();
                    key(g, b, label, font_size, state.pressed || flashed, state.hovered);
                })
                .paint_key(style::key((label, flashed, n)))
                .id(label)
                .flex_1()
                .min_w_0()
                .h_full()
                // For agents: a button named by its key.
                .agent("Button", label)
                .on_click(cx.msg(Msg::Key(label)))
            }))
        }));

        div()
            .size_full()
            .relative()
            // (The field keeps the keyboard; these keys reach this from
            // it.)
            .on_key_down(cx.listener(|this: &mut Self, e: &KeyDownEvent, cx| {
                if e.keystroke.modifiers.modified() {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "up" => this.step_history(true, cx),
                    "down" => this.step_history(false, cx),
                    "escape" => this.press("C", cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            // The context menu, anywhere in the window.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this: &mut Self, e: &MouseDownEvent, cx| {
                    let menu = this.menu(cx);
                    let chosen = cx.msg_with(|command: &str| Msg::Command(command.to_string()));
                    olo_kit::menu::context_menu(cx, menu, e.position, chosen);
                }),
            )
            .child(keypad)
            .child(display)
    }
}

/// One key: R5's raised bevel, pushed in while held.
fn key(g: &mut Painter, r: Bounds<Pixels>, label: &'static str, font_size: f32, down: bool, hovered: bool) {
    let p = theme::palette();
    let p = &*p;
    let base = p.control_background;
    let (top, bottom) = if down {
        (style::shade(p, base, 1.2), style::shade(p, base, 1.12))
    } else if hovered {
        (style::tint(base, 0.7), style::shade(p, base, 1.02))
    } else {
        (style::tint(base, 0.85), style::shade(p, base, 1.06))
    };
    let inner = style::inset(g, r, 1.0);
    style::vgradient(g, inner, top, bottom);
    style::frame_rect(g, r, style::shade(p, p.panel_background, 1.6));
    if down {
        style::bevel(g, inner, style::shade(p, base, 1.35), style::shade(p, base, 1.1));
    } else {
        style::bevel(g, inner, style::c(p.shine), style::shade(p, base, 1.2));
    }
    let off = if down { px(1.0) } else { px(0.0) };
    let c = r.center();
    style::label(g, c.x + off, c.y + off, Align::Center, label, font_size, style::ui_font(FontWeight::NORMAL), style::c(p.control_text));
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_kit_base_adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |app| {
        let settings: Settings = olo_kit::store::load(&settings_path()).unwrap_or_default();
        let (w, h) = settings.keypad.size();
        let (min_w, min_h) = Keypad::Compact.size();
        let window = olo_kit::open_window(WindowOptions::new("DeskCalc", w, h).min_size(min_w - 50.0, min_h).floating(), DeskCalc::new);
        instance::listen(APP_ID, olo_kit::proxy());
        // (Launched again: the one there comes forward.)
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                olo_kit::raise(Some(&window));
            }
        });
    });
}

/// Settings from before the rename (Tabula) carry on.
fn olo_kit_base_adopt() {
    olo_xdg::legacy::adopt();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What was worked out fits its line: whole, or its end after "…",
    /// cut between characters, never wider than the room, at every width
    /// the window can be.
    #[test]
    fn what_was_worked_out_shows_its_end() {
        let font = style::ui_font(FontWeight::NORMAL);
        let wide = |t: &str| f32::from(style::text_width(t.to_owned(), WORKED_SIZE, font.clone()));
        assert_eq!(end_fit("12×3 =", WORKED_SIZE, &font, 200.0), "12×3 =");
        let long = "12345678901234567890123456789012345678901234567890/0";
        for room in [WIDTH - 2.0 * (INSET + DISPLAY_PAD), 120.0, 40.0] {
            let shown = end_fit(long, WORKED_SIZE, &font, room);
            assert!(shown.starts_with('…') && long.ends_with(&shown['…'.len_utf8()..]), "{room}: {shown}");
            assert!(wide(&shown) <= room, "{room}: “{shown}” {} across", wide(&shown));
            // As much as fits: one more character wouldn't.
            let kept = shown.chars().count() - 1;
            let more = format!("…{}", &long[long.len() - kept - 1..]);
            assert!(wide(&more) > room, "{room}: “{more}” would fit too");
        }
    }

    /// What a result was worked out from is saved with it and only while
    /// it shows; settings from before have none (not the history's last).
    #[test]
    fn what_was_worked_out_is_saved_with_its_result() {
        let old: Settings = serde_json::from_str(r#"{"expression":"3","history":["12×3"]}"#).unwrap();
        assert_eq!(old.worked, None);
        let saved: Settings = serde_json::from_str(r#"{"expression":"36","history":["12×3"],"worked":"12×3"}"#).unwrap();
        assert_eq!(saved.worked.as_deref(), Some("12×3"));
        assert!(dismisses_error("left") && dismisses_error("end") && !dismisses_error("5") && !dismisses_error("up"));
    }

    /// The display's lines are inside its well, its sides' padding kept,
    /// the first above the second, each tall enough for its text, the
    /// error the second's size or less.
    #[test]
    fn the_display_has_two_lines() {
        let line = |size: f32| size * style::LINE_HEIGHT;
        assert!(WORKED_MID - line(WORKED_SIZE) / 2.0 >= 2.0, "the first line under the well's top");
        assert!(WORKED_MID + line(WORKED_SIZE) / 2.0 <= LINE_TOP + 0.01, "the first line over the second");
        assert!(LINE_H >= line(LINE_SIZE) && LINE_TOP + LINE_H <= DISPLAY_H - 2.0, "the second line in the well");
        assert!(ERROR_SIZE < LINE_SIZE && line(ERROR_SIZE) <= LINE_H);
        assert_eq!(DISPLAY_PAD, 8.0);
        // "Can’t divide by zero" whole, at the window's own width.
        let error = f32::from(style::text_width("Can’t divide by zero".to_owned(), ERROR_SIZE, style::ui_font(FontWeight::NORMAL)));
        assert!(error <= WIDTH - 2.0 * (INSET + DISPLAY_PAD), "{error}");
    }
}
