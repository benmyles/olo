//! The BeOS controls and what else every app shows the same way.

pub mod alert;
pub mod banner;
pub mod buttons;
pub mod charts;
pub mod color;
pub mod control;
pub mod dashboard;
pub mod find_bar;
pub mod image_view;
pub mod key_hint;
pub mod list;
pub mod menu_field;
pub mod ruler;
pub mod scroll;
pub mod side_pane;
pub mod sidebar;
pub mod slider;
pub mod slim_scroll;
pub mod status;
pub mod strip;
pub mod tabs;
pub mod text_control;
pub mod text_field;
pub mod text_view;
pub mod thumbs;
pub mod toolbar;
pub mod views;

pub use alert::{
    ALERT_ICON, ALERT_TEXT_X, ALERT_WIDTH, Alert, AlertFrame, Conversation, ConversationField, Timeout, Unsaved, WAYS_IN, alert_height, alert_title, alert_width, ask_to_save, button_row, open_alert,
    open_app_alert, open_timed_alert, paragraph, paragraph_height, ways_in,
};
pub use banner::{Banner, Tone, banner_back, banner_height, verdict_mark};
pub use buttons::{Button, CheckBox, LinkButton, PaneSwitch, RadioButton, SMALL_BUTTON_HEIGHT, button_width, check_box_look, link_button_width};
pub use charts::{CellGrid, Graph, GridCell, LegendItem, SPECTRUM_LABELS, Series, Spectrum, SpectrumSeries, legend, legend_height, nice_ceiling};
pub use color::{ColorControl, ColorWell, color_control_height};
pub use control::{Control, ControlState, DISABLED_OPACITY, control};
pub use find_bar::{find_bar, find_said};
pub use image_view::{ImageView, image_view};
pub use key_hint::KeyHint;
pub use list::{Cell, Column, ColumnListView, HEADER_HEIGHT, ListModel};
pub use menu_field::{MenuField, NONE_CHOSEN};
pub use scroll::{SCROLL_BAR, ScrollBar, ScrollView};
pub use side_pane::{SidePane, side_divider};
pub use sidebar::Sidebar;
pub use slider::Slider;
pub use slim_scroll::{SLIM_BAR, SLIM_NEAR, SlimScrollBar};
pub use status::{StatusBar, level_bar};
pub use strip::{STRIP_FADED, STRIP_HEIGHT, STRIP_ROW, STRIP_SIDE_HEAD, STRIP_SIDE_MAX, STRIP_SIDE_MIN, STRIP_SIDE_WIDTH, StripAction, StripGroup, StripTab, TabStrip};
pub use tabs::{TAB_HEIGHT, TabView};
pub use text_control::TextControl;
pub use text_field::{FieldMessage, TextField, TextFieldEvent, text_field};
pub use text_view::{TextView, text_view};
pub use thumbs::{Thumbnail, ThumbnailPicker};
pub use toolbar::{TOOL, ToolBar};
pub use views::{BBox, EmptyState, Separator, StringView, Tag, heading, separator};
