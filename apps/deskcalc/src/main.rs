//! DeskCalc: BeOS's desk calculator. An expression line over a keypad
//! that grows with the window: R5's basic keys, Haiku's scientific rows
//! above them, or no keypad at all (compact). Type or click; Enter or =
//! works it out (expr.rs); ↑ and ↓ step through earlier expressions. The
//! keypad's context menu holds the options, as in Haiku. On Olo's GPUI
//! kit (olo-ui).

mod expr;

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use expr::Angle;
use olo_ui::{
    Menu, WindowOptions, agent,
    gpui::{
        self, App, AppContext as _, Bounds, Context, Entity, Focusable, FontWeight, InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        ParentElement as _, Pixels, Render, Styled as _, Subscription, Window, actions, canvas, div, point, px, size,
    },
    instance::{self, Launch},
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
const INSET: f32 = olo_ui::metrics::WINDOW_INSET;
const GAP: f32 = olo_ui::metrics::TIGHT;
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
    olo_ui::config_dir().join("deskcalc.json")
}

/// How many earlier expressions ↑ remembers.
const HISTORY: usize = 50;

actions!(deskcalc, [Evaluate, Back, Forward, Clear]);

/// Where each key was drawn, for the pointer.
type Keys = Rc<RefCell<Vec<(Bounds<Pixels>, &'static str)>>>;

struct DeskCalc {
    settings: Settings,
    field: Entity<TextField>,
    /// Why the last = failed, until the next edit.
    error: Option<String>,
    /// The display shows a result: a digit starts afresh, an operator
    /// carries on from it.
    shows_result: bool,
    /// Where ↑/↓ is in the history (None: the line being typed).
    recall: Option<usize>,
    /// A key drawn down for a moment because its character was typed.
    flash: Option<(&'static str, Instant)>,
    /// The key the pointer is on, and the one it went down on.
    hovered: Option<&'static str>,
    pressed: Option<&'static str>,
    keys: Keys,
    /// The field changes as the expression does.
    _field_events: Subscription,
}

impl DeskCalc {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> DeskCalc {
        let settings: Settings = olo_ui::store::load(&settings_path()).unwrap_or_default();
        let size = (DISPLAY_H * 0.5).round();
        let field = text_field(cx, |f| f.font_size(size).align(olo_ui::widgets::text_field::Align::Right).name("Expression"));
        field.update(cx, |f, cx| f.set_text(settings.expression.clone(), cx));
        let events = cx.subscribe_in(&field, window, Self::field_event);
        // The display always has the keyboard.
        window.focus(&field.focus_handle(cx));
        // What was left showing last time counts as a result.
        let shows_result = !settings.expression.is_empty();
        DeskCalc { settings, field, error: None, shows_result, recall: None, flash: None, hovered: None, pressed: None, keys: Rc::default(), _field_events: events }
    }

    fn store(&self) {
        olo_ui::store::save(settings_path(), &self.settings);
    }

    /// Shows `text` in the display, the caret at its end.
    fn show(&mut self, text: String, cx: &mut Context<Self>) {
        self.settings.expression = text.clone();
        self.field.update(cx, |f, cx| f.set_text(text, cx));
    }

    fn field_event(&mut self, field: &Entity<TextField>, event: &TextFieldEvent, _: &mut Window, cx: &mut Context<Self>) {
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
                    self.show(typed.to_string(), cx);
                }
                self.shows_result = false;
                self.error = None;
            }
            TextFieldEvent::Changed => {
                self.settings.expression = field.read(cx).text().to_owned();
                self.recall = None;
            }
            // (Return in the field: as = .)
            TextFieldEvent::Entered => {
                self.evaluate(cx);
                self.flash_key("=", cx);
            }
        }
        cx.notify();
    }

    fn flash_key(&mut self, key: &'static str, cx: &mut Context<Self>) {
        self.flash = Some((key, Instant::now()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |calc, cx| {
                if calc.flash.is_some_and(|(_, at)| at.elapsed() >= FLASH) {
                    calc.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn evaluate(&mut self, cx: &mut Context<Self>) {
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
                self.show(expr::format(v), cx);
                self.shows_result = true;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.recall = None;
        self.store();
        cx.notify();
    }

    /// A keypad key.
    fn press(&mut self, key: &str, cx: &mut Context<Self>) {
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
        self.show(e, cx);
        cx.notify();
    }

    /// ↑ / ↓ through the history.
    fn step_history(&mut self, back: bool, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    /// The keypad's context menu, as in Haiku.
    fn menu(&self) -> Menu {
        let s = &self.settings;
        Menu::new("DeskCalc")
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

    /// The menu commands (for the command palette and agents).
    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let keypad = match command {
            "copy" => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(self.settings.expression.clone()));
                return true;
            }
            "paste" => {
                // (Read off the UI thread: `paste_item`.)
                let (read, field) = (cx.paste_item(), self.field.clone());
                cx.spawn(async move |_, cx| {
                    if let Some(text) = read.await.and_then(|i| i.text()) {
                        let _ = field.update(cx, |f, cx| f.insert(&text, cx));
                    }
                })
                .detach();
                return true;
            }
            "keypad-basic" => Keypad::Basic,
            "keypad-scientific" => Keypad::Scientific,
            "keypad-compact" => Keypad::Compact,
            "radians" | "degrees" => {
                self.settings.angle = if command == "radians" { Angle::Radians } else { Angle::Degrees };
                self.store();
                return true;
            }
            _ => return false,
        };
        if keypad != self.settings.keypad {
            self.settings.keypad = keypad;
            let (_, h) = keypad.size();
            let width = window.viewport_size().width;
            window.resize(size(width, px(h)));
            self.store();
            cx.notify();
        }
        true
    }

    fn key_at(&self, p: gpui::Point<Pixels>) -> Option<&'static str> {
        self.keys.borrow().iter().find(|(b, _)| b.contains(&p)).map(|(_, k)| *k)
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pressed = self.key_at(e.position);
        cx.notify();
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pressed = self.pressed.take();
        if let Some(k) = pressed.filter(|k| self.key_at(e.position) == Some(*k)) {
            self.press(k, cx);
        }
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let over = self.key_at(e.position);
        if over != self.hovered {
            self.hovered = over;
            cx.notify();
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

impl Render for DeskCalc {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let rows = self.settings.keypad.rows();
        let flashing = self.flash.filter(|(_, t)| t.elapsed() < FLASH).map(|(k, _)| k);
        let (hovered, pressed) = (self.hovered, self.pressed);
        let error = self.error.clone();
        let keys = self.keys.clone();
        let palette = p.clone();
        // The display: a sunken well, the expression right-aligned in it.
        let well = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                style::fill_rect(window, b, style::c(palette.document_background));
                style::sunken(window, b, &palette, false);
                if let Some(e) = &error {
                    let r = style::label(window, cx, b.origin.x + px(6.0), b.origin.y + px(9.0), Align::Left, e.clone(), 10.0, style::ui_font(FontWeight::NORMAL), style::c(palette.failure));
                    agent::record(window, cx, agent::Node::label(e.clone(), r));
                }
            },
        )
        .absolute()
        .inset_0();
        let display = div()
            .absolute()
            .left(px(INSET))
            .top(px(INSET))
            .right(px(INSET))
            .h(px(DISPLAY_H))
            .child(well)
            .child(div().absolute().left(px(6.0)).right(px(6.0)).top(px(0.0)).bottom(px(0.0)).child(self.field.clone()));

        // The keypad fills the rest.
        let palette = p.clone();
        let keypad = canvas(
            |_, _, _| {},
            move |pad, _, window, cx| {
                let mut placed = keys.borrow_mut();
                placed.clear();
                if rows.is_empty() {
                    return;
                }
                let n = rows.len() as f32;
                let (pw, ph) = (f32::from(pad.size.width), f32::from(pad.size.height));
                let kw = (pw - 4.0 * GAP) / 5.0;
                let kh = (ph - (n - 1.0) * GAP) / n;
                let font_size = (kh * 0.42).clamp(10.0, 28.0).round();
                for (r, row) in rows.iter().enumerate() {
                    for (col, &label) in row.iter().enumerate() {
                        let b = Bounds { origin: point(pad.origin.x + px(col as f32 * (kw + GAP)), pad.origin.y + px(r as f32 * (kh + GAP))), size: size(px(kw), px(kh)) };
                        let b = style::snap(window, b);
                        let down = pressed == Some(label) && hovered == Some(label) || flashing == Some(label);
                        key(window, cx, &palette, b, label, font_size, down, hovered == Some(label));
                        // For agents: a button named by its key.
                        agent::record(window, cx, agent::Node::button(label, b));
                        placed.push((b, label));
                    }
                }
            },
        )
        .absolute()
        .left(px(INSET))
        .right(px(INSET))
        .top(px(INSET + DISPLAY_H + INSET))
        .bottom(px(INSET));

        div()
            .size_full()
            .relative()
            // (The field keeps the keyboard; these keys reach this
            // from it.)
            .key_context("DeskCalc")
            .on_action(cx.listener(|this, _: &Evaluate, _, cx| {
                this.evaluate(cx);
                this.flash_key("=", cx);
            }))
            .on_action(cx.listener(|this, _: &Back, _, cx| this.step_history(true, cx)))
            .on_action(cx.listener(|this, _: &Forward, _, cx| this.step_history(false, cx)))
            .on_action(cx.listener(|this, _: &Clear, _, cx| this.press("C", cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            // The context menu, anywhere in the window.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    let view = cx.entity();
                    olo_ui::menu::context_menu(window, cx, this.menu(), e.position, move |command, window, cx| {
                        view.update(cx, |calc, cx| {
                            calc.command(command, window, cx);
                        });
                    });
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .child(keypad)
            .child(display)
    }
}

/// One key: R5's raised bevel, pushed in while held.
#[allow(clippy::too_many_arguments)]
fn key(window: &mut Window, cx: &mut App, p: &olo_ui::palette::Palette, r: Bounds<Pixels>, label: &'static str, font_size: f32, down: bool, hovered: bool) {
    let base = p.control_background;
    let (top, bottom) = if down {
        (style::shade(p, base, 1.2), style::shade(p, base, 1.12))
    } else if hovered {
        (style::tint(base, 0.7), style::shade(p, base, 1.02))
    } else {
        (style::tint(base, 0.85), style::shade(p, base, 1.06))
    };
    let inner = style::inset(window, r, 1.0);
    style::vgradient(window, inner, top, bottom);
    style::frame_rect(window, r, style::shade(p, p.panel_background, 1.6));
    if down {
        style::bevel(window, inner, style::shade(p, base, 1.35), style::shade(p, base, 1.1));
    } else {
        style::bevel(window, inner, style::c(p.shine), style::shade(p, base, 1.2));
    }
    let off = if down { px(1.0) } else { px(0.0) };
    let c = r.center();
    style::label(window, cx, c.x + off, c.y + off, Align::Center, label, font_size, style::ui_font(FontWeight::NORMAL), style::c(p.control_text));
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_kit_base_adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, |cx| {
        cx.bind_keys([
            KeyBinding::new("enter", Evaluate, Some("DeskCalc")),
            KeyBinding::new("up", Back, Some("DeskCalc")),
            KeyBinding::new("down", Forward, Some("DeskCalc")),
            KeyBinding::new("escape", Clear, Some("DeskCalc")),
        ]);
        let settings: Settings = olo_ui::store::load(&settings_path()).unwrap_or_default();
        let (w, h) = settings.keypad.size();
        let (min_w, min_h) = Keypad::Compact.size();
        let calc: Rc<RefCell<Option<Entity<DeskCalc>>>> = Rc::default();
        let made = calc.clone();
        let handle = olo_ui::open_window(cx, WindowOptions::new("DeskCalc", w, h).min_size(min_w - 50.0, min_h).floating(), move |window, cx| {
            let view = cx.new(|cx| DeskCalc::new(window, cx));
            *made.borrow_mut() = Some(view.clone());
            view
        });
        instance::listen(APP_ID, olo_ui::proxy(cx));
        // Everything worth keeping is in the settings: stored before
        // becoming the new version.
        let calc_for_upgrade = calc.clone();
        olo_ui::on_upgrade(cx, move |cx| {
            if let Some(view) = calc_for_upgrade.borrow().as_ref() {
                let calc = view.read(cx);
                let text = serde_json::to_string_pretty(&calc.settings).unwrap_or_default();
                let _ = std::fs::write(settings_path(), text);
            }
            Some(String::new())
        });
        olo_ui::message_handler(cx, move |message, cx| {
            if message.is::<Launch>()
                && let Some(handle) = handle
            {
                olo_ui::raise(Some(handle), cx);
            }
        });
        // The palette's and agents' way to the menu's commands.
        let calc_for_menus = calc.clone();
        agent::set_menus(
            cx,
            move |cx| calc_for_menus.borrow().as_ref().map(|c| c.read(cx).menu().entries(APP_ID)).unwrap_or_default(),
            move |command, cx| {
                let Some(view) = calc.borrow().clone() else { return false };
                let Some(handle) = handle else { return false };
                handle.update(cx, |_, window, cx| view.update(cx, |calc, cx| calc.command(command, window, cx))).unwrap_or(false)
            },
        );
    });
}

/// Settings from before the rename (Tabula) carry on.
fn olo_kit_base_adopt() {
    olo_xdg::legacy::adopt();
}
