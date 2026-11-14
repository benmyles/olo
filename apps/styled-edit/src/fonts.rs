//! The installed font families, and the faces a document's styles are
//! drawn in. The kit's text system finds any installed family by name; a
//! worker has it read the system's fonts once at the start (so the first
//! text in a face only the system has doesn't wait on a frame for them)
//! and lists the families for the Font menu.

use std::sync::Arc;

use olo_kit::{Font, FontStyle, FontWeight, TextSystem, style};

use crate::doc::{MONOSPACE, Style};

pub enum Event {
    Families(Vec<String>),
}

pub struct Fonts {
    pub families: Vec<String>,
    /// The Font menu's alphabetical groups of them, made once when the
    /// list comes (none checked): a window says its menus at every body.
    pub groups: Vec<olo_kit::Menu>,
}

impl Fonts {
    pub fn start(text: Arc<TextSystem>, send: impl Fn(Event) + Send + 'static) -> Fonts {
        let _ = std::thread::Builder::new().name("fonts".into()).spawn(move || {
            text.preload_system_fonts();
            let mut families: Vec<String> = text.all_font_names().into_iter().filter(|n| !n.starts_with('.')).collect();
            families.sort_by_key(|f| f.to_lowercase());
            families.dedup();
            send(Event::Families(families));
        });
        Fonts { families: Vec::new(), groups: Vec::new() }
    }
}

/// The face `s` is drawn in: the kit's monospaced one for code, the
/// system's (Olo's UI font) for none, else its family; bold and italic
/// from the family's faces (a variable face's weight from its axis).
pub fn font(s: &Style) -> Font {
    let weight = if s.bold { FontWeight::BOLD } else { FontWeight::NORMAL };
    let mut f = if s.family == MONOSPACE {
        style::mono_font(weight)
    } else if s.family.is_empty() {
        style::ui_font(weight)
    } else {
        Font { family: s.family.clone().into(), ..style::ui_font(weight) }
    };
    if s.italic {
        f.style = FontStyle::Italic;
    }
    f
}
