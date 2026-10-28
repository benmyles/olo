//! The spacing system. Every gap, inset and control height in Olo comes
//! from here, so all windows share one rhythm: a 2px grid, 8px between
//! controls, 12px from a window's edge to its content.

/// Between controls placed side by side or stacked.
pub const SPACING: f32 = 8.0;
/// The vertical Deskbar's width; notification banners share its column.
pub const DESKBAR_WIDTH: f32 = 172.0;
/// Between closely related items (a stack of check boxes).
pub const TIGHT: f32 = 4.0;
/// From a window (or tab panel) edge to its content.
pub const WINDOW_INSET: f32 = 12.0;
/// From a BBox's frame to its content.
pub const BOX_INSET: f32 = 10.0;
/// Between a label and the field it names.
pub const LABEL_GAP: f32 = 8.0;
/// Between a check box / radio mark and its label.
pub const MARK_GAP: f32 = 6.0;
/// A check box's mark: a note under its label lines up after it and
/// `MARK_GAP`.
pub const MARK_SIZE: f32 = 12.0;

/// Push buttons, menu fields and text controls share one height so rows
/// of mixed controls line up.
pub const CONTROL_HEIGHT: f32 = 24.0;
/// Check boxes and radio buttons.
pub const MARK_ROW: f32 = 20.0;
/// Height of a line of plain text (labels in sliders, status bars).
pub const LABEL_ROW: f32 = 16.0;
/// Horizontal padding inside buttons and fields.
pub const CONTROL_PAD: f32 = 12.0;
pub const BUTTON_MIN_WIDTH: f32 = 80.0;
/// Text inset inside a text well.
pub const TEXT_INSET: f32 = 6.0;
/// Rows in list views.
pub const LIST_ROW: f32 = 20.0;
/// Cell text inset in list views.
pub const CELL_INSET: f32 = 6.0;
/// A toolbar under the menu bar: one row of controls.
pub const TOOLBAR_HEIGHT: f32 = CONTROL_HEIGHT + 2.0 * TIGHT + 1.0;
