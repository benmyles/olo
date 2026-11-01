//! Olo's widgets on GPUI (BeOS controls): see each module.

pub mod alert;
pub mod buttons;
pub mod charts;
pub mod control;
pub mod dashboard;
pub mod image_view;
pub mod list;
pub mod menu_field;
pub mod scroll;
pub mod sidebar;
pub mod slider;
pub mod status;
pub mod tabs;
pub mod text_control;
pub mod text_field;
pub mod text_view;
pub mod thumbs;
pub mod tooltip;
pub mod views;

pub use alert::{ALERT_ICON, ALERT_TEXT_X, ALERT_WIDTH, AlertFrame, Conversation, ConversationField, alert_height, alert_title, alert_width, button_row, open_alert, paragraph, paragraph_height};
pub use scroll::{SCROLL_BAR, ScrollBar, ScrollView};
pub use slider::Slider;
pub use status::{StatusBar, level_bar};
pub use buttons::{Button, CheckBox, RadioButton, button_width, check_box_look};
pub use control::{Control, ControlState, control};
pub use image_view::{ImageView, image_view};
pub use list::{Cell, Column, ColumnListView, HEADER_HEIGHT, ListModel};
pub use menu_field::MenuField;
pub use text_control::TextControl;
pub use text_field::{TextField, TextFieldEvent, text_field};
pub use text_view::{TextView, text_view};
pub use sidebar::Sidebar;
pub use tabs::{TAB_HEIGHT, TabView};
pub use thumbs::{Thumbnail, ThumbnailPicker};
pub use tooltip::tooltip;
pub use charts::{CellGrid, Graph, GraphLayer, GraphShelf, GridCell, LegendItem, Series, legend, legend_height, nice_ceiling};
pub use views::{BBox, EmptyState, Separator, StringView, heading, separator};

/// The widgets' keys.
pub(crate) fn bind_keys(cx: &mut gpui::App) {
    text_field::bind_keys(cx);
    alert::bind_keys(cx);
    list::bind_keys(cx);
}
