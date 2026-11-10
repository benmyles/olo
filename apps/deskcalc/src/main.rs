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
const BASIC: [[&str; 5]; 4] = [["7", "8", "9", "(", ")"], ["4", "5", "6", "×", "÷"], ["1", "2", "3", "+", "−"], ["0", ".", "BS", "=", "C"]];
const SCIENTIFIC: [[&str; 5]; 4] = [["ln", "sin", "cos", "tan", "π"], ["log", "asin", "acos", "atan", "sqrt"], ["exp", "sinh", "cosh", "tanh", "cbrt"], ["!", "ceil", "floor", "E", "^"]];

const WIDTH: f32 = 266.0;
// (The kit's rhythm: the window's inset round it all, keys `TIGHT` apart.)
const INSET: f32 = olo_kit::metrics::WINDOW_INSET;
const GAP: f32 = olo_kit::metrics::TIGHT;
const DISPLAY_H: f32 = 36.0;
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { keypad: Keypad::Basic, angle: Angle::Radians, expression: String::new(), history: Vec::new() }
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
        let size = (DISPLAY_H * 0.5).round();
        let field = text_field(cx, |f| f.font_size(size).align(olo_kit::widgets::text_field::Align::Right).name("Expression").with_text(settings.expression.clone()));
        // (The field changes as the expression does.)
        cx.subscribe(&field, Self::field_event);
        // The display always has the keyboard.
        field.focus(cx);
        // What was left showing last time counts as a result.
        let shows_result = !settings.expression.is_empty();
        DeskCalc { settings, field, error: None, shows_result, recall: None, flash: None }
    }

    fn store(&self) {
        olo_kit::store::save(settings_path(), &self.settings);
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
                self.settings.expression = self.field.read(cx).text().to_owned();
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
                self.settings.history.push(text);
                let over = self.settings.history.len().saturating_sub(HISTORY);
                self.settings.history.drain(..over);
                self.put(expr::format(v), cx);
                self.shows_result = true;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
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
            "BS" => {
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
        let menu = self.menu(cx);
        olo_kit::menu::set_menus(cx, vec![menu]);
        let rows = self.settings.keypad.rows();
        let flashing = self.flash.filter(|(_, t)| t.elapsed() < FLASH).map(|(k, _)| k);
        // The display: a sunken well, the expression right-aligned in it.
        let error = self.error.clone();
        let well = canvas({
            let error = error.clone();
            move |g, b| {
                let p = theme::palette();
                style::fill_rect(g, b, style::c(p.document_background));
                style::sunken(g, b, &p, false);
                if let Some(e) = &error {
                    let r = style::label(g, b.origin.x + px(6.0), b.origin.y + px(9.0), Align::Left, e.clone(), 10.0, style::ui_font(FontWeight::NORMAL), style::c(p.failure));
                    g.agent(agent::Node::label(e.clone(), r));
                }
            }
        })
        .paint_key(style::key(&error))
        .absolute()
        .inset_0();
        let display = div()
            .absolute()
            .left(px(INSET))
            .top(px(INSET))
            .right(px(INSET))
            .h(px(DISPLAY_H))
            .child(well)
            .child(div().absolute().left(px(6.0)).right(px(6.0)).top(px(0.0)).bottom(px(0.0)).child(&self.field));

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
