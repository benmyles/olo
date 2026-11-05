//! The spacing system. Every gap, inset and control height in Olo comes
//! from here, so all windows share one rhythm: a 2px grid, 12px between
//! controls, 10 between a box's rows, 16 from a window's edge to its
//! content. (Opened up on 2026-11-03, Ben: "everything is too cramped …
//! there should be more space between all the elements and it should
//! look polished, clean": it was 8, 4 and 12. The layout review holds
//! every window to it: `layout_review`'s `crowded`, `window-edge`.)

/// Between controls placed side by side or stacked.
pub const SPACING: f32 = 12.0;
/// Between the rows of a box or a form (a control and the next, a note
/// and what follows it).
pub const STACK: f32 = 10.0;
/// Between sections of a form: under a commit row (Apply, Revert: it
/// ends its group), what follows starts this far down, so the row reads
/// as the end of what's above it (the layout review's `after-commit`).
pub const SECTION: f32 = 2.0 * SPACING;
/// The vertical Deskbar's width; notification banners share its column.
pub const DESKBAR_WIDTH: f32 = 172.0;
/// Between closely related items (a note under the control it's about,
/// a toolbar's controls and its edges).
pub const TIGHT: f32 = 6.0;
/// A card's corners (`style::card`), and inside it, from its edge to
/// what it holds.
pub const CARD_RADIUS: f32 = 5.0;
pub const CARD_INSET: f32 = 12.0;
/// How far a selection plate (`style::selection_plate`) reaches beyond
/// the picture or swatch on it.
pub const SELECTION_PAD: f32 = 4.0;
/// From a window (or tab panel) edge to its content.
pub const WINDOW_INSET: f32 = 16.0;
/// From a BBox's frame to its content.
pub const BOX_INSET: f32 = 14.0;
/// Between a label and the field it names.
pub const LABEL_GAP: f32 = 10.0;
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
/// A toolbar under the menu bar: one row of controls, `TIGHT` above and
/// below. Its line goes under it (`style::hline` at its bottom) and what
/// follows starts a line lower (`style::hairline`): a line is a pixel at
/// 1.5×, not a point and a half, so the controls stay centred.
pub const TOOLBAR_HEIGHT: f32 = CONTROL_HEIGHT + 2.0 * TIGHT;
