//! The installed font families, and loading the faces a document names.
//! One worker thread owns the font database: it lists the families once,
//! then loads faces on request and registers them with the kit
//! (`style::add_document_font`) so every window can draw them.

use std::{
    collections::HashSet,
    sync::mpsc::{Sender, channel},
};

use olo_kit::{
    egui::{Context, FontFamily},
    style,
};

use crate::doc::Style;

pub enum Event {
    Families(Vec<String>),
    /// A face was registered (or couldn't be found): redraw.
    Loaded,
}

pub struct Fonts {
    pub families: Vec<String>,
    requests: Sender<(String, bool, bool)>,
    requested: HashSet<String>,
}

/// The kit family name a document face is registered under.
fn key(family: &str, bold: bool, italic: bool) -> String {
    let variant = match (bold, italic) {
        (false, false) => "R",
        (true, false) => "B",
        (false, true) => "I",
        (true, true) => "BI",
    };
    format!("se:{family}:{variant}")
}

impl Fonts {
    pub fn start(send: impl Fn(Event) + Send + 'static) -> Fonts {
        let (requests, rx) = channel::<(String, bool, bool)>();
        std::thread::spawn(move || {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            let mut families: Vec<String> = db
                .faces()
                .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
                .filter(|n| !n.starts_with('.'))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            families.sort_by_key(|f| f.to_lowercase());
            send(Event::Families(families));
            for (family, bold, italic) in rx {
                let names: Vec<&str> = if family.is_empty() { olo_theme::fonts::UI.to_vec() } else { vec![family.as_str()] };
                let face = names
                    .iter()
                    .find_map(|n| {
                        let families = [fontdb::Family::Name(n)];
                        db.query(&fontdb::Query {
                            families: &families,
                            weight: fontdb::Weight(if bold { 700 } else { 400 }),
                            stretch: fontdb::Stretch::Normal,
                            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                        })
                    })
                    .and_then(|id| db.with_face_data(id, |d, i| (d.to_vec(), i)));
                if let Some((data, index)) = face {
                    style::add_document_font(&key(&family, bold, italic), data, index);
                }
                send(Event::Loaded);
            }
        });
        Fonts { families: Vec::new(), requests, requested: HashSet::new() }
    }

    /// The family to draw `s` with in this frame: its own face once
    /// loaded (asking for it the first time), the UI font until then.
    pub fn family(&mut self, ctx: &Context, s: &Style) -> FontFamily {
        // The system font's regular, bold and italic are the kit's own.
        if s.family.is_empty() && !(s.bold && s.italic) {
            return match (s.bold, s.italic) {
                (true, _) => style::bold(),
                (_, true) => style::italic(),
                _ => FontFamily::Proportional,
            };
        }
        let k = key(&s.family, s.bold, s.italic);
        if style::family_ready(ctx, &k) {
            return FontFamily::Name(k.into());
        }
        if self.requested.insert(k) {
            let _ = self.requests.send((s.family.clone(), s.bold, s.italic));
        }
        if s.bold { style::bold() } else { FontFamily::Proportional }
    }
}
