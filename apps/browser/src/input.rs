//! egui's input, turned into Servo's. The content area has no input
//! surface of its own (the embed lets events through to the window), so
//! the window's events over it are forwarded to the tab's web view.

use servo::{
    ClipboardAction, Code, CompositionEvent, CompositionState, DevicePoint, ImeEvent, InputEvent, Key, KeyState,
    KeyboardEvent, Location, Modifiers, MouseButton, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, NamedKey, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use tabula_kit::egui::{self, Pos2, Rect};

/// Where the content sits in the window and how many device pixels a
/// logical one is.
#[derive(Clone, Copy)]
pub struct Viewport {
    pub rect: Rect,
    pub scale: f32,
}

impl Viewport {
    fn point(&self, pos: Pos2) -> WebViewPoint {
        let p = (pos - self.rect.min) * self.scale;
        WebViewPoint::Device(DevicePoint::new(p.x, p.y))
    }
}

/// What the window should do with an event besides forwarding it.
#[derive(Default)]
pub struct Forwarded {
    pub events: Vec<InputEvent>,
    /// The pointer went down in the content: it takes the keyboard.
    pub clicked: bool,
    /// Text to hand the page's paste.
    pub paste: Option<String>,
}

/// The pointer, as the content sees it.
#[derive(Default)]
pub struct Pointer {
    /// Over the content.
    inside: bool,
    /// Buttons pressed in the content and not yet released: until then
    /// the page gets the pointer wherever it goes (a drag-select).
    held: u8,
}

/// Pointer events over the content (always) and keyboard events (when
/// the content has the keyboard).
pub fn forward(events: &[egui::Event], view: Viewport, keyboard: bool, pointer: &mut Pointer) -> Forwarded {
    let mut out = Forwarded::default();
    for e in events {
        match e {
            egui::Event::PointerMoved(pos) => {
                if view.rect.contains(*pos) || pointer.held > 0 {
                    pointer.inside = view.rect.contains(*pos);
                    out.events.push(InputEvent::MouseMove(MouseMoveEvent::new(view.point(*pos))));
                } else if std::mem::take(&mut pointer.inside) {
                    out.events.push(InputEvent::MouseLeftViewport(MouseLeftViewportEvent::default()));
                }
            }
            egui::Event::PointerGone => {
                if std::mem::take(&mut pointer.inside) {
                    out.events.push(InputEvent::MouseLeftViewport(MouseLeftViewportEvent::default()));
                }
            }
            egui::Event::PointerButton { pos, button, pressed, .. } => {
                if *pressed {
                    if !view.rect.contains(*pos) {
                        continue;
                    }
                    pointer.held = pointer.held.saturating_add(1);
                } else if pointer.held > 0 {
                    pointer.held -= 1;
                } else {
                    continue;
                }
                let button = match button {
                    egui::PointerButton::Primary => MouseButton::Primary,
                    egui::PointerButton::Secondary => MouseButton::Secondary,
                    egui::PointerButton::Middle => MouseButton::Auxiliary,
                    egui::PointerButton::Extra1 => MouseButton::Back,
                    egui::PointerButton::Extra2 => MouseButton::Forward,
                };
                let action = if *pressed { MouseButtonAction::Down } else { MouseButtonAction::Up };
                if *pressed {
                    out.clicked = true;
                }
                out.events.push(InputEvent::MouseButton(MouseButtonEvent::new(action, button, view.point(*pos))));
            }
            egui::Event::MouseWheel { unit, delta, .. } if pointer.inside => {
                // Servo takes the delta as egui (and winit) give it, moving
                // the content, and scrolls by its negative (servo-paint's
                // webview_renderer.rs); negating it here scrolled backwards.
                let (d, mode) = match unit {
                    egui::MouseWheelUnit::Point => (*delta * view.scale, WheelMode::DeltaPixel),
                    egui::MouseWheelUnit::Line => (*delta, WheelMode::DeltaLine),
                    egui::MouseWheelUnit::Page => (*delta, WheelMode::DeltaPage),
                };
                let at = view.point(last_pointer(events).unwrap_or(view.rect.center()));
                out.events.push(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x: d.x as f64, y: d.y as f64, z: 0.0, mode },
                    at,
                )));
            }
            _ if !keyboard => {}
            egui::Event::Key { key, physical_key, pressed, repeat, modifiers } => {
                // Printable keys arrive as Text; only send those here when
                // a modifier makes them a shortcut for the page.
                let named = named_key(*key);
                if named.is_none() && !(modifiers.ctrl || modifiers.alt || modifiers.mac_cmd) {
                    continue;
                }
                let key_value = match named {
                    Some(n) => Key::Named(n),
                    None => Key::Character(key.symbol_or_name().to_lowercase()),
                };
                out.events.push(keyboard_event(
                    key_value,
                    code(physical_key.unwrap_or(*key)),
                    *pressed,
                    *repeat,
                    *modifiers,
                ));
            }
            egui::Event::Text(text) => {
                for ch in text.chars() {
                    let k = Key::Character(ch.to_string());
                    let c = char_code(ch);
                    out.events.push(keyboard_event(k.clone(), c, true, false, egui::Modifiers::NONE));
                    out.events.push(keyboard_event(k, c, false, false, egui::Modifiers::NONE));
                }
            }
            egui::Event::Copy => out.events.push(InputEvent::EditingAction(ClipboardAction::Copy)),
            egui::Event::Cut => out.events.push(InputEvent::EditingAction(ClipboardAction::Cut)),
            egui::Event::Paste(text) => {
                out.paste = Some(text.clone());
                out.events.push(InputEvent::EditingAction(ClipboardAction::Paste));
            }
            egui::Event::Ime(ime) => match ime {
                egui::ImeEvent::Preedit { text, .. } => out.events.push(InputEvent::Ime(ImeEvent::Composition(
                    CompositionEvent { state: CompositionState::Update, data: text.clone() },
                ))),
                egui::ImeEvent::Commit(text) => out.events.push(InputEvent::Ime(ImeEvent::Composition(
                    CompositionEvent { state: CompositionState::End, data: text.clone() },
                ))),
                _ => {}
            },
            _ => {}
        }
    }
    out
}

fn last_pointer(events: &[egui::Event]) -> Option<Pos2> {
    events.iter().rev().find_map(|e| match e {
        egui::Event::PointerMoved(p) => Some(*p),
        _ => None,
    })
}

fn keyboard_event(key: Key, code: Code, pressed: bool, repeat: bool, m: egui::Modifiers) -> InputEvent {
    let mut modifiers = Modifiers::empty();
    if m.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if m.ctrl {
        modifiers |= Modifiers::CONTROL;
    }
    if m.alt {
        modifiers |= Modifiers::ALT;
    }
    InputEvent::Keyboard(KeyboardEvent::new(keyboard_types::KeyboardEvent {
        state: if pressed { KeyState::Down } else { KeyState::Up },
        key,
        code,
        location: Location::Standard,
        modifiers,
        repeat,
        is_composing: false,
    }))
}

fn named_key(key: egui::Key) -> Option<NamedKey> {
    use egui::Key as K;
    Some(match key {
        K::ArrowDown => NamedKey::ArrowDown,
        K::ArrowLeft => NamedKey::ArrowLeft,
        K::ArrowRight => NamedKey::ArrowRight,
        K::ArrowUp => NamedKey::ArrowUp,
        K::Escape => NamedKey::Escape,
        K::Tab => NamedKey::Tab,
        K::Backspace => NamedKey::Backspace,
        K::Enter => NamedKey::Enter,
        K::Insert => NamedKey::Insert,
        K::Delete => NamedKey::Delete,
        K::Home => NamedKey::Home,
        K::End => NamedKey::End,
        K::PageUp => NamedKey::PageUp,
        K::PageDown => NamedKey::PageDown,
        K::F1 => NamedKey::F1,
        K::F2 => NamedKey::F2,
        K::F3 => NamedKey::F3,
        K::F4 => NamedKey::F4,
        K::F5 => NamedKey::F5,
        K::F6 => NamedKey::F6,
        K::F7 => NamedKey::F7,
        K::F8 => NamedKey::F8,
        K::F9 => NamedKey::F9,
        K::F10 => NamedKey::F10,
        K::F11 => NamedKey::F11,
        K::F12 => NamedKey::F12,
        _ => return None,
    })
}

fn code(key: egui::Key) -> Code {
    use egui::Key as K;
    match key {
        K::ArrowDown => Code::ArrowDown,
        K::ArrowLeft => Code::ArrowLeft,
        K::ArrowRight => Code::ArrowRight,
        K::ArrowUp => Code::ArrowUp,
        K::Escape => Code::Escape,
        K::Tab => Code::Tab,
        K::Backspace => Code::Backspace,
        K::Enter => Code::Enter,
        K::Space => Code::Space,
        K::Insert => Code::Insert,
        K::Delete => Code::Delete,
        K::Home => Code::Home,
        K::End => Code::End,
        K::PageUp => Code::PageUp,
        K::PageDown => Code::PageDown,
        K::Minus => Code::Minus,
        K::Equals | K::Plus => Code::Equal,
        K::Comma => Code::Comma,
        K::Period => Code::Period,
        K::Slash => Code::Slash,
        K::Backslash => Code::Backslash,
        K::Semicolon => Code::Semicolon,
        K::Quote => Code::Quote,
        K::Backtick => Code::Backquote,
        K::OpenBracket => Code::BracketLeft,
        K::CloseBracket => Code::BracketRight,
        K::Num0 => Code::Digit0,
        K::Num1 => Code::Digit1,
        K::Num2 => Code::Digit2,
        K::Num3 => Code::Digit3,
        K::Num4 => Code::Digit4,
        K::Num5 => Code::Digit5,
        K::Num6 => Code::Digit6,
        K::Num7 => Code::Digit7,
        K::Num8 => Code::Digit8,
        K::Num9 => Code::Digit9,
        K::A => Code::KeyA,
        K::B => Code::KeyB,
        K::C => Code::KeyC,
        K::D => Code::KeyD,
        K::E => Code::KeyE,
        K::F => Code::KeyF,
        K::G => Code::KeyG,
        K::H => Code::KeyH,
        K::I => Code::KeyI,
        K::J => Code::KeyJ,
        K::K => Code::KeyK,
        K::L => Code::KeyL,
        K::M => Code::KeyM,
        K::N => Code::KeyN,
        K::O => Code::KeyO,
        K::P => Code::KeyP,
        K::Q => Code::KeyQ,
        K::R => Code::KeyR,
        K::S => Code::KeyS,
        K::T => Code::KeyT,
        K::U => Code::KeyU,
        K::V => Code::KeyV,
        K::W => Code::KeyW,
        K::X => Code::KeyX,
        K::Y => Code::KeyY,
        K::Z => Code::KeyZ,
        K::F1 => Code::F1,
        K::F2 => Code::F2,
        K::F3 => Code::F3,
        K::F4 => Code::F4,
        K::F5 => Code::F5,
        K::F6 => Code::F6,
        K::F7 => Code::F7,
        K::F8 => Code::F8,
        K::F9 => Code::F9,
        K::F10 => Code::F10,
        K::F11 => Code::F11,
        K::F12 => Code::F12,
        _ => Code::Unidentified,
    }
}

/// The US-layout key a typed character most likely came from (pages
/// mostly look at `key`; `code` matters for games).
fn char_code(ch: char) -> Code {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        return egui::Key::from_name(&lower.to_ascii_uppercase().to_string()).map(code).unwrap_or(Code::Unidentified);
    }
    match ch {
        ' ' => Code::Space,
        '0' | ')' => Code::Digit0,
        '1' | '!' => Code::Digit1,
        '2' | '@' => Code::Digit2,
        '3' | '#' => Code::Digit3,
        '4' | '$' => Code::Digit4,
        '5' | '%' => Code::Digit5,
        '6' | '^' => Code::Digit6,
        '7' | '&' => Code::Digit7,
        '8' | '*' => Code::Digit8,
        '9' | '(' => Code::Digit9,
        '-' | '_' => Code::Minus,
        '=' | '+' => Code::Equal,
        '[' | '{' => Code::BracketLeft,
        ']' | '}' => Code::BracketRight,
        '\\' | '|' => Code::Backslash,
        ';' | ':' => Code::Semicolon,
        '\'' | '"' => Code::Quote,
        ',' | '<' => Code::Comma,
        '.' | '>' => Code::Period,
        '/' | '?' => Code::Slash,
        '`' | '~' => Code::Backquote,
        _ => Code::Unidentified,
    }
}
