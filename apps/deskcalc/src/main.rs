//! DeskCalc: BeOS's desk calculator. An expression line over a keypad
//! that grows with the window: R5's basic keys, Haiku's scientific rows
//! above them, or no keypad at all (compact). Type or click; Enter or =
//! works it out (expr.rs); ↑ and ↓ step through earlier expressions. The
//! keypad's context menu holds the options, as in Haiku.

mod expr;

use std::time::{Duration, Instant};

use expr::Angle;
use serde::{Deserialize, Serialize};
use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Align, FontId, Id, Key, Rect, Sense, emath::GuiRounding, pos2, vec2},
    instance::{self, Launch},
    style::{self, c, shade},
    widgets,
};

const APP_ID: &str = "tabula.deskcalc";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Keypad {
    Compact,
    Basic,
    Scientific,
}

/// R5's keypad; Haiku's scientific rows go above it.
const BASIC: [[&str; 5]; 4] = [
    ["7", "8", "9", "(", ")"],
    ["4", "5", "6", "×", "÷"],
    ["1", "2", "3", "+", "−"],
    ["0", ".", "BS", "=", "C"],
];
const SCIENTIFIC: [[&str; 5]; 4] = [
    ["ln", "sin", "cos", "tan", "π"],
    ["log", "asin", "acos", "atan", "sqrt"],
    ["exp", "sinh", "cosh", "tanh", "cbrt"],
    ["!", "ceil", "floor", "E", "^"],
];

const WIDTH: f32 = 250.0;
const INSET: f32 = 8.0;
const GAP: f32 = 4.0;
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
    fn size(self) -> (u32, u32) {
        let rows = self.rows().len() as f32;
        let keys = if rows > 0.0 { INSET + rows * KEY_H + (rows - 1.0) * GAP } else { 0.0 };
        (WIDTH as u32, (INSET + DISPLAY_H + INSET + keys).round() as u32)
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
    tabula_kit::config_dir().join("deskcalc.json")
}

/// How many earlier expressions ↑ remembers.
const HISTORY: usize = 50;

struct DeskCalc {
    settings: Settings,
    window: Option<WindowId>,
    /// Why the last = failed, until the next edit.
    error: Option<String>,
    /// The display shows a result: a digit starts afresh, an operator
    /// carries on from it.
    shows_result: bool,
    /// Where ↑/↓ is in the history (None: the line being typed).
    recall: Option<usize>,
    /// A key drawn down for a moment because its character was typed.
    flash: Option<(&'static str, Instant)>,
    /// The cursor should go to the end (after a keypad press).
    cursor_to_end: bool,
}

impl DeskCalc {
    fn store(&self) {
        let text = serde_json::to_string_pretty(&self.settings).unwrap_or_default();
        std::thread::spawn(move || {
            let path = settings_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, text);
        });
    }

    fn open(&mut self, cx: &mut Cx) {
        if let Some(id) = self.window {
            cx.activate(id);
            return;
        }
        let (w, h) = self.settings.keypad.size();
        let min = Keypad::Compact.size();
        self.window = Some(cx.open_window(WindowOptions::new("DeskCalc", w, h).min_size(min.0 - 50, min.1)));
    }

    fn evaluate(&mut self) {
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
                self.settings.expression = expr::format(v);
                self.shows_result = true;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.recall = None;
        self.cursor_to_end = true;
        self.store();
    }

    /// A keypad key.
    fn press(&mut self, key: &str) {
        let e = &mut self.settings.expression;
        let starts_number = key.chars().next().is_some_and(|ch| ch.is_ascii_digit() || ch == '.' || ch == 'π' || ch == '(')
            || matches!(key, "ln" | "log" | "exp" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "sqrt" | "cbrt" | "ceil" | "floor");
        if self.shows_result && starts_number {
            e.clear();
        }
        self.shows_result = false;
        self.error = None;
        match key {
            "=" => return self.evaluate(),
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
        self.cursor_to_end = true;
    }

    /// ↑ / ↓ through the history.
    fn step_history(&mut self, back: bool) {
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
        self.settings.expression = next.map(|i| h[i].clone()).unwrap_or_default();
        self.recall = next;
        self.shows_result = false;
        self.error = None;
        self.cursor_to_end = true;
    }

    fn menu(&self) -> Menu {
        let s = &self.settings;
        Menu::new("")
            .item("Copy", "copy", None)
            .item("Paste", "paste", None)
            .separator()
            .item("Basic Keypad", "keypad-basic", None)
            .checked(s.keypad == Keypad::Basic)
            .item("Scientific Keypad", "keypad-scientific", None)
            .checked(s.keypad == Keypad::Scientific)
            .item("No Keypad", "keypad-compact", None)
            .checked(s.keypad == Keypad::Compact)
            .separator()
            .item("Radians", "radians", None)
            .checked(s.angle == Angle::Radians)
            .item("Degrees", "degrees", None)
            .checked(s.angle == Angle::Degrees)
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        let keypad = match command {
            "copy" => {
                cx.copy(self.settings.expression.clone());
                return;
            }
            "paste" => {
                cx.paste(window);
                return;
            }
            "keypad-basic" => Keypad::Basic,
            "keypad-scientific" => Keypad::Scientific,
            "keypad-compact" => Keypad::Compact,
            "radians" | "degrees" => {
                self.settings.angle = if command == "radians" { Angle::Radians } else { Angle::Degrees };
                self.store();
                return;
            }
            _ => return,
        };
        if keypad != self.settings.keypad {
            self.settings.keypad = keypad;
            let (w, h) = keypad.size();
            let width = cx.window_size(window).map_or(w, |s| s.0);
            cx.resize(window, (width, h));
            self.store();
        }
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

impl App for DeskCalc {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect();
        let rows = self.settings.keypad.rows();

        // The context menu, anywhere in the window.
        let back = ui.interact(full, Id::unique("deskcalc-background"), Sense::click());
        let menu = self.menu();
        if let Some(cmd) = widgets::context_menu(&back, || menu) {
            self.command(cx, window, &cmd);
        }

        // Keys the display doesn't handle itself.
        let (enter, up, down, escape, typed) = ui.input(|i| {
            let typed: Vec<char> = i
                .events
                .iter()
                .filter_map(|e| if let egui::Event::Text(t) = e { Some(t.chars()) } else { None })
                .flatten()
                .collect();
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Escape),
                typed,
            )
        });
        if let Some(&ch) = typed.last()
            && let Some(k) = key_for(ch)
        {
            self.flash = Some((k, Instant::now()));
        }
        if !typed.is_empty() {
            // Typing a digit over a result starts a new expression.
            if self.shows_result && typed.first().is_some_and(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == '(') {
                self.settings.expression.clear();
                self.cursor_to_end = true;
            }
            self.shows_result = false;
            self.error = None;
        }
        if escape {
            self.press("C");
        }
        if up || down {
            self.step_history(up);
        }

        // The display: a sunken well, the expression right-aligned in it.
        let display = Rect::from_min_size(full.min + vec2(INSET, INSET), vec2(full.width() - 2.0 * INSET, DISPLAY_H));
        ui.painter().rect_filled(display, 0.0, c(p.document_background));
        style::sunken(ui.painter(), display, &p, false);
        let size = (DISPLAY_H * 0.5).round();
        // TextEdit sets its line a little high in the well; nudge it down.
        let text_rect = display.shrink2(vec2(6.0, (DISPLAY_H - size * 1.3) / 2.0)).translate(vec2(0.0, 2.0));
        let edit_id = Id::unique("deskcalc-expression");
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect));
        let out = egui::TextEdit::singleline(&mut self.settings.expression)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .align(egui::Align2::RIGHT_CENTER)
            .font(FontId::proportional(size))
            .text_color(c(p.document_text))
            .desired_width(text_rect.width())
            .show(&mut child);
        widgets::edit_menu(ui, edit_id, &out.response, &mut self.settings.expression, true, false);
        if out.response.changed() {
            self.recall = None;
        }
        if self.cursor_to_end {
            self.cursor_to_end = false;
            let mut state = out.state.clone();
            let end = egui::text::CCursor::new(self.settings.expression.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), edit_id);
        }
        // The display always has the keyboard.
        if !out.response.has_focus() {
            out.response.request_focus();
        }
        if enter {
            self.evaluate();
            self.flash = Some(("=", Instant::now()));
        }
        if let Some(e) = &self.error {
            style::label(
                ui.painter(),
                display.left() + 6.0,
                display.top() + 9.0,
                Align::Min,
                e,
                FontId::proportional(10.0),
                c(p.failure),
            );
        }

        // The keypad fills the rest.
        if rows.is_empty() {
            return;
        }
        let pad = Rect::from_min_max(pos2(full.left() + INSET, display.bottom() + INSET), full.max - vec2(INSET, INSET));
        let n = rows.len() as f32;
        let kw = (pad.width() - 4.0 * GAP) / 5.0;
        let kh = (pad.height() - (n - 1.0) * GAP) / n;
        let font = FontId::proportional((kh * 0.42).clamp(10.0, 28.0).round());
        let flashing = self.flash.filter(|(_, t)| t.elapsed() < FLASH).map(|(k, _)| k);
        if flashing.is_some() {
            ui.ctx().request_repaint_after(FLASH);
        }
        let mut pressed = None;
        for (r, row) in rows.iter().enumerate() {
            for (col, &label) in row.iter().enumerate() {
                let rect = Rect::from_min_size(
                    pos2(pad.left() + col as f32 * (kw + GAP), pad.top() + r as f32 * (kh + GAP)),
                    vec2(kw, kh),
                )
                .round_to_pixels(ui.ctx().pixels_per_point());
                let resp = ui.interact(rect, Id::unique(("deskcalc-key", label)), Sense::click());
                let down = resp.is_pointer_button_down_on() && resp.hovered() || flashing == Some(label);
                key(ui, rect, label, &font, down, resp.hovered());
                if resp.clicked() {
                    pressed = Some(label);
                }
            }
        }
        if let Some(k) = pressed {
            self.press(k);
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, _window: WindowId) {
        self.store();
        cx.quit();
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if message.is::<Launch>() {
            self.open(cx);
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        // Everything worth keeping is in the settings.
        self.store();
        Some(String::new())
    }
}

/// One key: R5's raised bevel, pushed in while held.
fn key(ui: &egui::Ui, r: Rect, label: &str, font: &FontId, down: bool, hovered: bool) {
    let p = style::palette(ui.ctx());
    let painter = ui.painter();
    let base = p.control_background;
    let (top, bottom) = if down {
        (shade(&p, base, 1.2), shade(&p, base, 1.12))
    } else if hovered {
        (c(tabula_theme::tint(base, 0.7)), shade(&p, base, 1.02))
    } else {
        (c(tabula_theme::tint(base, 0.85)), shade(&p, base, 1.06))
    };
    style::vgradient(painter, r.shrink(1.0), top, bottom);
    style::frame_rect(painter, r, shade(&p, p.panel_background, 1.6));
    let inner = r.shrink(1.0);
    if down {
        style::bevel(painter, inner, shade(&p, base, 1.35), shade(&p, base, 1.1));
    } else {
        style::bevel(painter, inner, c(p.shine), shade(&p, base, 1.2));
    }
    let off = if down { 1.0 } else { 0.0 };
    let color = c(p.control_text);
    let galley = painter.layout_no_wrap(label.to_owned(), font.clone(), color);
    let pos = style::text_origin(ui.ctx(), &galley, r.center().x - galley.size().x / 2.0 + off, r.center().y + off, font.size);
    painter.galley(pos, galley, color);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // What was left showing last time counts as a result.
        let shows_result = !settings.expression.is_empty();
        let mut app = DeskCalc {
            settings,
            window: None,
            error: None,
            shows_result,
            recall: None,
            flash: None,
            cursor_to_end: true,
        };
        app.open(cx);
        app
    });
}
