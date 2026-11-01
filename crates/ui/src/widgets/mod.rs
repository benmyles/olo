//! Olo's widgets on GPUI (BeOS controls): see each module.

pub mod text_field;

pub use text_field::{TextField, TextFieldEvent, text_field};

/// The widgets' keys.
pub(crate) fn bind_keys(cx: &mut gpui::App) {
    text_field::bind_keys(cx);
}
