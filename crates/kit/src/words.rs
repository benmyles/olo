//! An app's words in the person's language: `olo_kit_base::words`, and
//! what the kit does with it. The words an app hands a widget as a
//! literal (`Button::new("Save")`, `Menu::item("Open…", "open")`, a
//! window's title, a field's hint, a tip) are in the language chosen
//! with nothing said ([`of`]); what's made as the app goes (a file's
//! name, a sentence with a number in it) is never looked up, and is
//! asked for with `tr!` where it's the app's own words. What an app
//! paints itself (`style::label`) is its own to ask for.

pub use olo_kit_base::words::*;

use crate::SharedString;

/// `text` as a widget shows it: a literal in the language chosen (the
/// same string, in English and where the language has no word for it:
/// nothing is made); anything else as it is.
pub fn of(text: impl Into<SharedString>) -> SharedString {
    let text = text.into();
    match text.as_static() {
        Some(written) => {
            let said = tr(written);
            // (A translation is no literal: it's never looked up again,
            // as a word of another language that is also an English one
            // would be.)
            if std::ptr::eq(said, written) { text } else { SharedString::new(said) }
        }
        None => text,
    }
}

/// Each of `texts` as a widget shows it (a tab view's tabs, a menu
/// field's choices): the same list in English.
pub fn of_all(texts: std::rc::Rc<[SharedString]>) -> std::rc::Rc<[SharedString]> {
    if current().code == "en" { texts } else { texts.iter().cloned().map(of).collect() }
}

/// Words that reach a widget as plain text but are the app's own, by
/// what they say (an alert's title, message and buttons, given as
/// `&str`): in the language chosen where it has them.
pub fn of_text(text: &str) -> SharedString {
    match lookup(text) {
        Some(said) => SharedString::new(said),
        None => SharedString::new(text.to_owned()),
    }
}
