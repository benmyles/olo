//! Wayland input → egui events.

use egui::Key;
use smithay_client_toolkit::seat::keyboard::Keysym;

pub fn key_from_keysym(sym: Keysym) -> Option<Key> {
    use Key::*;
    Some(match sym {
        Keysym::Down => ArrowDown,
        Keysym::Left => ArrowLeft,
        Keysym::Right => ArrowRight,
        Keysym::Up => ArrowUp,
        Keysym::Escape => Escape,
        Keysym::Tab | Keysym::ISO_Left_Tab => Tab,
        Keysym::BackSpace => Backspace,
        Keysym::Return | Keysym::KP_Enter => Enter,
        Keysym::space => Space,
        Keysym::Insert => Insert,
        Keysym::Delete | Keysym::KP_Delete => Delete,
        Keysym::Home => Home,
        Keysym::End => End,
        Keysym::Page_Up => PageUp,
        Keysym::Page_Down => PageDown,
        Keysym::comma => Comma,
        Keysym::period => Period,
        Keysym::slash => Slash,
        Keysym::backslash => Backslash,
        Keysym::minus | Keysym::KP_Subtract => Minus,
        Keysym::plus | Keysym::KP_Add => Plus,
        Keysym::equal => Equals,
        Keysym::semicolon => Semicolon,
        Keysym::colon => Colon,
        Keysym::apostrophe => Quote,
        Keysym::grave => Backtick,
        Keysym::bracketleft => OpenBracket,
        Keysym::bracketright => CloseBracket,
        Keysym::question => Questionmark,
        Keysym::_0 | Keysym::KP_0 | Keysym::parenright => Num0,
        Keysym::_1 | Keysym::KP_1 | Keysym::exclam => Num1,
        Keysym::_2 | Keysym::KP_2 | Keysym::at => Num2,
        Keysym::_3 | Keysym::KP_3 | Keysym::numbersign => Num3,
        Keysym::_4 | Keysym::KP_4 | Keysym::dollar => Num4,
        Keysym::_5 | Keysym::KP_5 | Keysym::percent => Num5,
        Keysym::_6 | Keysym::KP_6 | Keysym::asciicircum => Num6,
        Keysym::_7 | Keysym::KP_7 | Keysym::ampersand => Num7,
        Keysym::_8 | Keysym::KP_8 | Keysym::asterisk => Num8,
        Keysym::_9 | Keysym::KP_9 | Keysym::parenleft => Num9,
        Keysym::F1 => F1,
        Keysym::F2 => F2,
        Keysym::F3 => F3,
        Keysym::F4 => F4,
        Keysym::F5 => F5,
        Keysym::F6 => F6,
        Keysym::F7 => F7,
        Keysym::F8 => F8,
        Keysym::F9 => F9,
        Keysym::F10 => F10,
        Keysym::F11 => F11,
        Keysym::F12 => F12,
        s => {
            // Letters, either case.
            let raw = s.raw();
            let lower = if (0x41..=0x5a).contains(&raw) { raw + 0x20 } else { raw };
            if (0x61..=0x7a).contains(&lower) {
                let name = ((lower as u8) as char).to_ascii_uppercase().to_string();
                return Key::from_name(&name);
            }
            return None;
        }
    })
}

/// Linux evdev button codes → egui buttons.
pub fn pointer_button(code: u32) -> Option<egui::PointerButton> {
    Some(match code {
        0x110 => egui::PointerButton::Primary,
        0x111 => egui::PointerButton::Secondary,
        0x112 => egui::PointerButton::Middle,
        0x113 => egui::PointerButton::Extra1,
        0x114 => egui::PointerButton::Extra2,
        _ => return None,
    })
}

pub fn cursor_icon(icon: egui::CursorIcon) -> smithay_client_toolkit::seat::pointer::CursorIcon {
    use egui::CursorIcon as E;
    use smithay_client_toolkit::seat::pointer::CursorIcon as W;
    match icon {
        E::Default => W::Default,
        E::None => W::Default,
        E::ContextMenu => W::ContextMenu,
        E::Help => W::Help,
        E::PointingHand => W::Pointer,
        E::Progress => W::Progress,
        E::Wait => W::Wait,
        E::Cell => W::Cell,
        E::Crosshair => W::Crosshair,
        E::Text => W::Text,
        E::VerticalText => W::VerticalText,
        E::Alias => W::Alias,
        E::Copy => W::Copy,
        E::Move => W::Move,
        E::NoDrop => W::NoDrop,
        E::NotAllowed => W::NotAllowed,
        E::Grab => W::Grab,
        E::Grabbing => W::Grabbing,
        E::AllScroll => W::AllScroll,
        E::ResizeHorizontal => W::EwResize,
        E::ResizeNeSw => W::NeswResize,
        E::ResizeNwSe => W::NwseResize,
        E::ResizeVertical => W::NsResize,
        E::ResizeEast => W::EResize,
        E::ResizeSouthEast => W::SeResize,
        E::ResizeSouth => W::SResize,
        E::ResizeSouthWest => W::SwResize,
        E::ResizeWest => W::WResize,
        E::ResizeNorthWest => W::NwResize,
        E::ResizeNorth => W::NResize,
        E::ResizeNorthEast => W::NeResize,
        E::ResizeColumn => W::ColResize,
        E::ResizeRow => W::RowResize,
        E::ZoomIn => W::ZoomIn,
        E::ZoomOut => W::ZoomOut,
    }
}

/// Split one frame's events so a button's press and its release never
/// arrive in the same frame: everything up to such a release now, the
/// release and what follows in the next frame. A touchpad tap delivers
/// both at once, and egui would then never see the button down (no
/// press-and-hold state for menus, scroll arrows or drags to begin).
pub fn split_taps(mut events: Vec<egui::Event>) -> (Vec<egui::Event>, Vec<egui::Event>) {
    let mut pressed: Vec<egui::PointerButton> = Vec::new();
    for (i, e) in events.iter().enumerate() {
        if let egui::Event::PointerButton { button, pressed: down, .. } = e {
            if *down {
                pressed.push(*button);
            } else if pressed.contains(button) {
                let later = events.split_off(i);
                return (events, later);
            }
        }
    }
    (events, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, pos2};

    fn button(pressed: bool) -> Event {
        Event::PointerButton { pos: pos2(1.0, 1.0), button: PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    #[test]
    fn taps_take_two_frames() {
        let moved = Event::PointerMoved(pos2(1.0, 1.0));
        let (now, later) = split_taps(vec![moved.clone(), button(true), button(false), moved.clone()]);
        assert_eq!(now, vec![moved.clone(), button(true)]);
        assert_eq!(later, vec![button(false), moved.clone()]);
        // A release on its own (the press came in an earlier frame) stays.
        let (now, later) = split_taps(vec![button(false), moved.clone()]);
        assert_eq!(now.len(), 2);
        assert!(later.is_empty());
        // Two taps in one frame: the second waits for the frame after.
        let (_, later) = split_taps(vec![button(true), button(false), button(true), button(false)]);
        let (now, rest) = split_taps(later);
        assert_eq!(now, vec![button(false), button(true)]);
        assert_eq!(rest, vec![button(false)]);
    }
}
