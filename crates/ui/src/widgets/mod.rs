//! Olo's widgets on GPUI (BeOS controls): see each module.

pub mod alert;
pub mod buttons;
pub mod control;
pub mod menu_field;
pub mod text_control;
pub mod text_field;
pub mod tooltip;
pub mod views;

pub use alert::{ALERT_ICON, ALERT_TEXT_X, ALERT_WIDTH, AlertFrame, Conversation, ConversationField, alert_title, button_row, paragraph, paragraph_height};
pub use buttons::{Button, CheckBox, RadioButton, button_width};
pub use control::{Control, ControlState, control};
pub use menu_field::MenuField;
pub use text_control::TextControl;
pub use text_field::{TextField, TextFieldEvent, text_field};
pub use tooltip::tooltip;
pub use views::{BBox, StringView};

/// The widgets' keys.
pub(crate) fn bind_keys(cx: &mut gpui::App) {
    text_field::bind_keys(cx);
    alert::bind_keys(cx);
}
