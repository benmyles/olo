//! High-performance dirty-tracked render state over `GhosttyRenderState`.
//!
//! Designed for Olo's 120 Hz GPU renderer:
//! - Two-phase `begin_update` / `end_update` keeps terminal lock hold time tiny.
//! - Stable `RowId([u64; 2])` + `DirtyState` let `RowCache` skip unchanged rows.
//! - Overscan support enables sub-row elastic scrolling without extra terminal queries.

use std::{cell::Cell, ffi::c_void, mem, ptr};

use crate::{
    color::Rgb,
    error::{VtError, check},
    ffi,
    terminal::GhosttyTerminal,
};

/// Global dirty state reported after updating a [`RenderState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyState {
    Clean,
    Partial,
    Full,
}

/// Stable opaque identity of a terminal row across frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowId(pub [u64; 2]);

/// Olo: its two words, each handed over whole (as an array it was a
/// length and then sixteen bytes, which a hasher takes a byte at a
/// time or by copying: a row's id is hashed some hundreds of times a
/// frame).
impl std::hash::Hash for RowId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.0[0]);
        state.write_u64(self.0[1]);
    }
}

/// Olo: a map keyed by rows' ids. An id is two words of the engine's
/// own making (a page's serial and a row's place in it), looked up for
/// every row of every pane at every frame: they are mixed, not hashed
/// against an adversary there isn't (the standard hasher was 4% of a
/// streaming frame).
pub type RowMap<V> = std::collections::HashMap<RowId, V, std::hash::BuildHasherDefault<RowIdHasher>>;

/// Olo: [`RowMap`]'s hasher: each word folded in with a rotation and a
/// multiplication (the rustc-hash crate's, as of its 1.x). For keys
/// that hand it words (`RowId` does): bytes are taken eight at a time,
/// a short last one padded with zeros.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowIdHasher(u64);

impl std::hash::Hasher for RowIdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write_u64(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, word: usize) {
        self.write_u64(word as u64);
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }
}

/// Cursor shape requested by the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

impl CursorStyle {
    pub fn from_ffi(v: ffi::GhosttyCursorStyle) -> Self {
        match v {
            ffi::GHOSTTY_CURSOR_STYLE_BAR => Self::Bar,
            ffi::GHOSTTY_CURSOR_STYLE_BLOCK => Self::Block,
            ffi::GHOSTTY_CURSOR_STYLE_UNDERLINE => Self::Underline,
            ffi::GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW => Self::BlockHollow,
            _ => Self::Block,
        }
    }

    pub fn to_ffi(self) -> ffi::GhosttyCursorStyle {
        match self {
            Self::Bar => ffi::GHOSTTY_CURSOR_STYLE_BAR,
            Self::Block => ffi::GHOSTTY_CURSOR_STYLE_BLOCK,
            Self::Underline => ffi::GHOSTTY_CURSOR_STYLE_UNDERLINE,
            Self::BlockHollow => ffi::GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW,
        }
    }
}

/// Snapshot of cursor geometry and visibility for the current render state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorInfo {
    pub viewport_pos: Option<(u16, u16)>,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub style: CursorStyle,
}

/// Snapshot of default colors & active 256-color palette for the current frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderColors {
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Option<Rgb>,
    pub palette: [Rgb; 256],
}

/// Underline rendering style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

impl UnderlineStyle {
    fn from_flags(flags: u16) -> Self {
        match (flags >> 8) & 0x7 {
            1 => Self::Single,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::None,
        }
    }
}

/// Cell width class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CellWide {
    #[default]
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

/// Extracted visual style of a single cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: UnderlineStyle,
    pub underline_color: Option<Rgb>,
    /// Set when the cell foreground was explicitly chosen from palette index 0..16
    /// (useful for optional bold-is-bright behavior).
    pub fg_palette_index: Option<u8>,
}

/// Extracted cell ready for Olo's `RowCache` and `GlyphRun` layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderCell {
    /// Primary Unicode scalar value (0 if empty).
    pub codepoint: u32,
    /// Full UTF-8 grapheme cluster when the cell has combining codepoints / ZWJ
    /// emoji (empty when single-codepoint or blank).
    pub grapheme: String,
    pub wide: CellWide,
    /// Resolved foreground RGB (`None` = use terminal default foreground).
    pub fg: Option<Rgb>,
    /// Resolved background RGB (`None` = use terminal default background).
    pub bg: Option<Rgb>,
    pub style: CellStyle,
    pub hyperlink: bool,
}

/// Extracted row from a [`RenderState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderRow {
    pub id: RowId,
    pub viewport_y: i32,
    pub dirty: bool,
    pub wrap: bool,
    pub wrap_continuation: bool,
    pub selection: Option<(u16, u16)>,
    pub cells: Vec<RenderCell>,
    /// The row as the engine packs it, for knowing it again
    /// ([`RenderState::extract_rows_known`]); empty where `cells` is.
    pub raw: RowRaw,
}

/// Olo: a row as the engine packs it: the row's own word and a word a
/// cell. A row with the same id whose words are all the same as at the
/// last update shows the same, if no cell of it is a grapheme (whose
/// further codepoints are kept beside the cell, not in its word), the
/// styles its cells name are the same styles, and the colours it is
/// resolved by are the same: what a caller keeps of a row, with its
/// cells, to have it passed over, its cells not taken again.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowRaw {
    pub row: u64,
    pub cells: Vec<u64>,
}

/// Image layer relative to backgrounds and text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImageLayer {
    BelowBackground = 0,
    BelowText = 1,
    AboveText = 2,
}

/// Extracted Kitty image placement from a [`RenderState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImagePlacement {
    pub image_id: u32,
    pub image_generation: u64,
    pub placement_id: u32,
    pub layer: ImageLayer,
    pub z: i32,
    pub viewport_col: i32,
    pub viewport_row: i32,
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub cell_offset_x: u32,
    pub cell_offset_y: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
}

/// Reusable render state holding pre-allocated row, cell, and image iterators.
pub struct RenderState {
    raw: ffi::GhosttyRenderState,
    row_iter: ffi::GhosttyRenderStateRowIterator,
    row_cells: ffi::GhosttyRenderStateRowCells,
    image_iter: ffi::GhosttyRenderStateImageIterator,
    utf8_scratch: Vec<u8>,
    pending: Cell<bool>,
    overscan: (u16, u16),
    budget: Box<crate::limits::Budget>,
}

// SAFETY: the owned snapshot and its iterators move together. Borrows are
// exhausted before methods return, and terminal access is locked during begin.
unsafe impl Send for RenderState {}

impl RenderState {
    pub fn new() -> Result<Self, VtError> {
        crate::sys::init()?;
        let budget = Box::new(crate::limits::Budget::new());
        let mut state = Self {
            raw: ptr::null_mut(),
            row_iter: ptr::null_mut(),
            row_cells: ptr::null_mut(),
            image_iter: ptr::null_mut(),
            utf8_scratch: vec![0; 64],
            pending: Cell::new(false),
            overscan: (0, 0),
            budget,
        };
        // SAFETY: each constructor receives a dedicated writable handle slot and
        // the stable allocator. Drop releases all initialized slots on any error.
        unsafe {
            check(ffi::ghostty_render_state_new(state.budget.raw(), &mut state.raw))?;
            check(ffi::ghostty_render_state_row_iterator_new(state.budget.raw(), &mut state.row_iter))?;
            check(ffi::ghostty_render_state_row_cells_new(state.budget.raw(), &mut state.row_cells))?;
            check(ffi::ghostty_render_state_image_iterator_new(state.budget.raw(), &mut state.image_iter))?;
        }
        if state.raw.is_null() || state.row_iter.is_null() || state.row_cells.is_null() || state.image_iter.is_null() {
            return Err(VtError::InvalidValue);
        }
        Ok(state)
    }

    /// Request overscan rows above and below the viewport for smooth scrolling.
    pub fn set_overscan(&mut self, above: u16, below: u16) -> Result<(), VtError> {
        if above > 256 || below > 256 {
            return Err(VtError::LimitExceeded);
        }
        self.finish_pending()?;
        let ov = ffi::GhosttyRenderStateOverscan { above, below };
        // SAFETY: OVERSCAN copies the two uint16_t values into the owned state.
        check(unsafe { ffi::ghostty_render_state_set(self.raw, ffi::GHOSTTY_RENDER_STATE_OPT_OVERSCAN, ptr::from_ref(&ov).cast::<c_void>()) })?;
        self.overscan = (above, below);
        Ok(())
    }

    /// Capture dirty terminal state into this render state in one call.
    pub fn update(&mut self, term: &GhosttyTerminal) -> Result<(), VtError> {
        self.begin_update(term)?;
        self.end_update()
    }

    /// Phase 1 (while holding terminal lock): copy minimal terminal state.
    pub fn begin_update(&mut self, term: &GhosttyTerminal) -> Result<(), VtError> {
        self.finish_pending()?;
        let _access = term.lock();
        let (cols, rows) = term.size_locked();
        let captured = usize::from(rows) + usize::from(self.overscan.0) + usize::from(self.overscan.1);
        if captured * usize::from(cols) > crate::limits::MAX_CELLS {
            return Err(VtError::LimitExceeded);
        }
        // SAFETY: Both handles are live; the terminal lock covers the copy and the render state is exclusive.
        check(unsafe { ffi::ghostty_render_state_begin_update(self.raw, term.raw()) })?;
        self.pending.set(true);
        Ok(())
    }

    /// Phase 2 (without terminal lock): finish deferred render state work.
    pub fn end_update(&mut self) -> Result<(), VtError> {
        self.finish_pending()
    }

    fn finish_pending(&self) -> Result<(), VtError> {
        if self.pending.replace(false) {
            // SAFETY: each successful begin is completed before any other access;
            // the deferred work uses only the uniquely owned render snapshot.
            check(unsafe { ffi::ghostty_render_state_end_update(self.raw) })?;
        }
        Ok(())
    }

    /// Mark all dirty flags consumed after painting a frame.
    pub fn clean(&mut self) -> Result<(), VtError> {
        self.finish_pending()?;
        // SAFETY: The render state is initialized, complete and exclusively owned.
        check(unsafe { ffi::ghostty_render_state_clean(self.raw) })
    }

    /// Force the next frame to treat all rows as dirty (e.g. after theme/font change).
    pub fn mark_full_dirty(&mut self) -> Result<(), VtError> {
        self.finish_pending()?;
        let d = ffi::GHOSTTY_RENDER_STATE_DIRTY_FULL;
        // SAFETY: DIRTY copies one GhosttyRenderStateDirty value into the owned state.
        check(unsafe { ffi::ghostty_render_state_set(self.raw, ffi::GHOSTTY_RENDER_STATE_OPT_DIRTY, ptr::from_ref(&d).cast::<c_void>()) })
    }

    pub fn size(&self) -> (u16, u16) {
        let _ = self.finish_pending();
        let mut cols = 0u16;
        let mut rows = 0u16;
        // SAFETY: The completed render state writes uint16_t dimensions to two separate outputs.
        unsafe {
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_COLS, ptr::from_mut(&mut cols).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return (0, 0);
            }
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_ROWS, ptr::from_mut(&mut rows).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return (0, 0);
            }
        }
        (cols, rows)
    }

    pub fn dirty(&self) -> DirtyState {
        let _ = self.finish_pending();
        let mut raw_d = ffi::GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        // SAFETY: DIRTY writes a GhosttyRenderStateDirty to the local output.
        unsafe {
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_DIRTY, ptr::from_mut(&mut raw_d).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return DirtyState::Full;
            }
        }
        match raw_d {
            ffi::GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => DirtyState::Partial,
            ffi::GHOSTTY_RENDER_STATE_DIRTY_FULL => DirtyState::Full,
            _ => DirtyState::Clean,
        }
    }

    pub fn overscan_captured(&self) -> (u16, u16) {
        let _ = self.finish_pending();
        let mut ov = ffi::GhosttyRenderStateOverscan::default();
        // SAFETY: OVERSCAN writes the fixed-layout pair of captured row counts.
        unsafe {
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_OVERSCAN, ptr::from_mut(&mut ov).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return (0, 0);
            }
        }
        (ov.above, ov.below)
    }

    pub fn cursor(&self) -> CursorInfo {
        let _ = self.finish_pending();
        let mut c = ffi::GhosttyRenderStateCursor { size: mem::size_of::<ffi::GhosttyRenderStateCursor>(), ..Default::default() };
        // SAFETY: The size-tagged local matches the aggregate output type required by render.h.
        unsafe {
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_CURSOR, ptr::from_mut(&mut c).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                c = Default::default();
            }
        }
        CursorInfo {
            // render.h leaves these fields undefined when the cursor is offscreen.
            viewport_pos: if c.viewport_has_value { Some((c.viewport_x, c.viewport_y)) } else { None },
            wide_tail: c.viewport_has_value && c.wide_tail,
            visible: c.visible,
            blinking: c.blinking,
            password_input: c.password_input,
            style: CursorStyle::from_ffi(c.visual_style),
        }
    }

    pub fn colors(&self) -> RenderColors {
        let _ = self.finish_pending();
        let mut c = ffi::GhosttyRenderStateColors { size: mem::size_of::<ffi::GhosttyRenderStateColors>(), ..Default::default() };
        // SAFETY: The size-tagged local matches the aggregate output type required by render.h.
        unsafe {
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_COLORS, ptr::from_mut(&mut c).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                c = Default::default();
            }
        }
        let palette = std::array::from_fn(|i| Rgb::from_ffi(c.palette[i]));
        RenderColors { background: Rgb::from_ffi(c.background), foreground: Rgb::from_ffi(c.foreground), cursor: c.cursor_has_value.then(|| Rgb::from_ffi(c.cursor)), palette }
    }

    pub fn image_placements_generation(&self) -> u64 {
        let _ = self.finish_pending();
        let mut generation = 0u64;
        // SAFETY: IMAGE_PLACEMENTS_GENERATION writes uint64_t to the local output.
        unsafe {
            if ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_IMAGE_PLACEMENTS_GENERATION, ptr::from_mut(&mut generation).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return 0;
            }
        }
        generation
    }

    /// Extract visible rows (including any captured overscan rows).
    ///
    /// When `only_dirty` is true and global dirty is `Partial`, skips populating
    /// `cells` on clean rows (still returning row metadata so callers can map
    /// `RowId` -> `viewport_y` effortlessly!).
    pub fn extract_rows(&mut self, palette: &[Rgb; 256], only_dirty: bool) -> Result<Vec<RenderRow>, VtError> {
        self.extract_rows_known(palette, only_dirty, None::<fn(&RowId) -> Option<(&'static RowRaw, &'static [RenderCell])>>)
    }

    /// Olo: [`RenderState::extract_rows`], passing over the rows the
    /// caller already has: `known` gives what it kept of a row
    /// ([`RenderRow::raw`]), and a row whose packed words are all the
    /// same comes back with no `cells` (and no `raw`), as a clean row
    /// does under `only_dirty`.
    ///
    /// For a scroll. The engine redraws everything when its viewport
    /// moves (a line printed at the foot of a full screen), and marks
    /// every row dirty: it says of none that it is as it was. But the
    /// rows keep their ids, and each is one read of its packed cells
    /// (`ROW_DATA_CELLS_RAW`) and a comparison away from being known
    /// the same, where taking its cells is calls a cell: a 63 × 40 pane
    /// printing a line took 2,520 cells to find 63 new.
    ///
    /// The packed words don't hold everything. A grapheme's further
    /// codepoints are beside the cell (a second combining mark changes
    /// no word), so a row with a grapheme in it is always taken. A
    /// cell's style is named by an id that another style can come to
    /// have, so a row with styles has each of them read and compared
    /// with the kept cells' (`styles_as_kept`: `known` gives those
    /// too). And colours are resolved through the palette and the
    /// defaults, so pass `known` only while those are as they were.
    pub fn extract_rows_known<'a>(&mut self, palette: &[Rgb; 256], only_dirty: bool, known: Option<impl Fn(&RowId) -> Option<(&'a RowRaw, &'a [RenderCell])>>) -> Result<Vec<RenderRow>, VtError> {
        let global_dirty = self.dirty();
        let (cols, rows) = self.size();
        let (ov_above, ov_below) = self.overscan_captured();
        let cap = rows as usize + ov_above as usize + ov_below as usize;

        // SAFETY: The handle slot contains our reusable iterator. Its render state remains live and stable during iteration.
        check(unsafe { ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, ptr::from_mut(&mut self.row_iter).cast::<c_void>()) })?;

        let mut out = Vec::with_capacity(cap);
        // SAFETY: The iterator was populated by this render state and has not been invalidated by an update.
        while unsafe { ffi::ghostty_render_state_row_iterator_next(self.row_iter) } {
            let mut row_id = ffi::GhosttyRenderStateRowId::default();
            let mut viewport_y = 0i32;
            let mut row_dirty = false;
            let mut raw_row: ffi::GhosttyRow = 0;
            let mut sel = ffi::GhosttyRenderStateRowSelection { size: mem::size_of::<ffi::GhosttyRenderStateRowSelection>(), start_x: 0, end_x: 0 };
            // SAFETY: The iterator is on a valid row. Outputs match the ID, signed Y, dirty bool and packed row fields.
            unsafe {
                check(ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_ID, ptr::from_mut(&mut row_id).cast::<c_void>()))?;
                check(ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y, ptr::from_mut(&mut viewport_y).cast::<c_void>()))?;
                check(ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, ptr::from_mut(&mut row_dirty).cast::<c_void>()))?;
                check(ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_RAW, ptr::from_mut(&mut raw_row).cast::<c_void>()))?;
            }
            // SAFETY: The iterator is positioned and the selection range output carries the required struct size.
            let selection = match unsafe { ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION, ptr::from_mut(&mut sel).cast::<c_void>()) } {
                ffi::GHOSTTY_SUCCESS => Some((sel.start_x, sel.end_x)),
                ffi::GHOSTTY_NO_VALUE => None,
                code => return Err(VtError::from_ffi(code)),
            };

            let mut wrap = false;
            let mut wrap_cont = false;
            // SAFETY: The packed row was returned by this snapshot; both wrap fields have bool outputs.
            unsafe {
                check(ffi::ghostty_row_get(raw_row, ffi::GHOSTTY_ROW_DATA_WRAP, ptr::from_mut(&mut wrap).cast::<c_void>()))?;
                check(ffi::ghostty_row_get(raw_row, ffi::GHOSTTY_ROW_DATA_WRAP_CONTINUATION, ptr::from_mut(&mut wrap_cont).cast::<c_void>()))?;
            }

            let effective_dirty = match global_dirty {
                DirtyState::Full => true,
                DirtyState::Partial => row_dirty,
                DirtyState::Clean => false,
            };

            let id = RowId(row_id.bits);
            let (cells, raw) = if only_dirty && !effective_dirty {
                (Vec::new(), RowRaw::default())
            } else {
                let mut view = ffi::GhosttyCellsView { ptr: ptr::null(), len: 0 };
                let (mut grapheme, mut styled) = (false, false);
                // SAFETY: The iterator is on a valid row: CELLS_RAW lends its packed cells until the next update, and the packed row has a bool for each of its flags.
                let packed: &[u64] = unsafe {
                    check(ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW, ptr::from_mut(&mut view).cast::<c_void>()))?;
                    check(ffi::ghostty_row_get(raw_row, ffi::GHOSTTY_ROW_DATA_GRAPHEME, ptr::from_mut(&mut grapheme).cast::<c_void>()))?;
                    check(ffi::ghostty_row_get(raw_row, ffi::GHOSTTY_ROW_DATA_STYLED, ptr::from_mut(&mut styled).cast::<c_void>()))?;
                    if view.ptr.is_null() { &[] } else { std::slice::from_raw_parts(view.ptr, view.len) }
                };
                let same = match known.as_ref().and_then(|known| known(&id)) {
                    // (The row's flag for styles is never clear where a cell has one.)
                    Some((kept, cells)) if !grapheme && kept.row == raw_row && kept.cells == packed && cells.len() == packed.len() => !styled || self.styles_as_kept(packed, cells, palette)?,
                    _ => false,
                };
                if same {
                    (Vec::new(), RowRaw::default())
                } else {
                    let raw = RowRaw { row: raw_row, cells: packed.to_vec() };
                    (self.extract_current_row_cells(cols as usize, palette)?, raw)
                }
            };

            out.push(RenderRow { id, viewport_y, dirty: effective_dirty, wrap, wrap_continuation: wrap_cont, selection, cells, raw });
        }
        Ok(out)
    }

    fn extract_current_row_cells(&mut self, cols: usize, palette: &[Rgb; 256]) -> Result<Vec<RenderCell>, VtError> {
        // SAFETY: The row is current and the slot holds our live reusable cell iterator.
        check(unsafe { ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, ptr::from_mut(&mut self.row_cells).cast::<c_void>()) })?;

        let mut cells: Vec<RenderCell> = Vec::with_capacity(cols);
        let mut previous_scalar = None;
        // SAFETY: The populated row iterator remains valid until the next render state update.
        while unsafe { ffi::ghostty_render_state_row_cells_next(self.row_cells) } {
            let mut raw_cell: ffi::GhosttyCell = 0;
            // SAFETY: RAW writes one packed GhosttyCell from the current cell.
            unsafe {
                check(ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, ptr::from_mut(&mut raw_cell).cast::<c_void>()))?;
            }

            // A scalar cell's visual attributes are entirely described by the
            // packed value within this row. Graphemes also have side-table data.
            if previous_scalar == Some(raw_cell)
                && let Some(previous) = cells.last()
            {
                cells.push(RenderCell { grapheme: String::new(), ..*previous });
                continue;
            }
            let (cell, tag) = self.cell_here(raw_cell, palette)?;
            previous_scalar = (tag != ffi::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME).then_some(raw_cell);
            cells.push(cell);
        }

        Ok(cells)
    }

    /// Olo: the cell the row's cell iterator is on (`raw_cell`, its
    /// packed word), as Rex draws it, and what its content is (text, a
    /// grapheme whose further codepoints are beside it, or only a
    /// background carried in the word).
    fn cell_here(&mut self, raw_cell: ffi::GhosttyCell, palette: &[Rgb; 256]) -> Result<(RenderCell, ffi::GhosttyCellContentTag), VtError> {
        let mut content_tag = ffi::GHOSTTY_CELL_CONTENT_CODEPOINT;
        let mut wide_raw = ffi::GHOSTTY_CELL_WIDE_NARROW;
        let mut codepoint = 0u32;
        let mut has_styling = false;
        let mut hyperlink = false;
        // SAFETY: The packed cell getters write the content and width enums, uint32_t scalar and two bool flags.
        unsafe {
            check(ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_CONTENT_TAG, ptr::from_mut(&mut content_tag).cast::<c_void>()))?;
            check(ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_WIDE, ptr::from_mut(&mut wide_raw).cast::<c_void>()))?;
            check(ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_CODEPOINT, ptr::from_mut(&mut codepoint).cast::<c_void>()))?;
            check(ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_HAS_STYLING, ptr::from_mut(&mut has_styling).cast::<c_void>()))?;
            check(ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_HAS_HYPERLINK, ptr::from_mut(&mut hyperlink).cast::<c_void>()))?;
        }

        let wide = match wide_raw {
            ffi::GHOSTTY_CELL_WIDE_WIDE => CellWide::Wide,
            ffi::GHOSTTY_CELL_WIDE_SPACER_TAIL => CellWide::SpacerTail,
            ffi::GHOSTTY_CELL_WIDE_SPACER_HEAD => CellWide::SpacerHead,
            _ => CellWide::Narrow,
        };

        let grapheme = if content_tag == ffi::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME { self.read_grapheme_utf8()? } else { String::new() };

        let mut bg_raw = ffi::GhosttyColorRgb::default();
        // SAFETY: The iterator is on a valid cell and the color output is a GhosttyColorRgb.
        let bg = match unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, ptr::from_mut(&mut bg_raw).cast::<c_void>()) } {
            ffi::GHOSTTY_SUCCESS => Some(Rgb::from_ffi(bg_raw)),
            ffi::GHOSTTY_NO_VALUE => None,
            code => return Err(VtError::from_ffi(code)),
        };

        let (fg, style) = if has_styling {
            let mut fg_raw = ffi::GhosttyColorRgb::default();
            // SAFETY: The iterator is on a valid cell and the color output is a GhosttyColorRgb.
            let fg = match unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, ptr::from_mut(&mut fg_raw).cast::<c_void>()) } {
                ffi::GHOSTTY_SUCCESS => Some(Rgb::from_ffi(fg_raw)),
                ffi::GHOSTTY_NO_VALUE => None,
                code => return Err(VtError::from_ffi(code)),
            };

            let mut st = ffi::GhosttyStyle::default();
            // SAFETY: STYLE writes a fixed-layout GhosttyStyle from this cell's page.
            unsafe {
                check(ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, ptr::from_mut(&mut st).cast::<c_void>()))?;
            }

            let underline_color = resolve_style_color(st.underline_color, palette);
            // SAFETY: The palette tag selects the one-byte palette union member (a valid 256-entry index).
            let fg_palette_index = if st.fg_color.tag as u32 == ffi::GHOSTTY_STYLE_COLOR_PALETTE { Some(unsafe { st.fg_color.value.palette }) } else { None };
            let flags = st.flags;
            let style = CellStyle {
                bold: (flags & (1 << 0)) != 0,
                italic: (flags & (1 << 1)) != 0,
                faint: (flags & (1 << 2)) != 0,
                blink: (flags & (1 << 3)) != 0,
                inverse: (flags & (1 << 4)) != 0,
                invisible: (flags & (1 << 5)) != 0,
                strikethrough: (flags & (1 << 6)) != 0,
                overline: (flags & (1 << 7)) != 0,
                underline: UnderlineStyle::from_flags(flags),
                underline_color,
                fg_palette_index,
            };
            (fg, style)
        } else {
            (None, CellStyle::default())
        };

        Ok((RenderCell { codepoint, grapheme, wide, fg, bg, style, hyperlink }, content_tag))
    }

    /// Olo: whether the styles of the current row's cells are what its
    /// cells as last taken (`kept`) have, the packed cells (`packed`)
    /// being what they were. A cell's word names its style by an id in
    /// its page, and an id is given to another style once no cell has
    /// it: a red X erased and a green X written where it was, between
    /// two looks, is the same word (Astra's review, 2026-11-22). So of
    /// each style id in the row, the first cell that has it is read
    /// again and compared with the kept one: an id names one style at a
    /// time, and a cell of text shows all of its style (its colours,
    /// its ground, its marks), so the rest that have the id are as that
    /// one is. (A row of more styles than a few is taken whole instead,
    /// and one where the first cell of a style isn't text.)
    fn styles_as_kept(&mut self, packed: &[u64], kept: &[RenderCell], palette: &[Rgb; 256]) -> Result<bool, VtError> {
        // SAFETY: The row is current and the slot holds our live reusable cell iterator.
        check(unsafe { ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, ptr::from_mut(&mut self.row_cells).cast::<c_void>()) })?;
        let mut seen = [0u16; 12];
        let (mut count, mut last) = (0, 0u16);
        let bits = style_id_bits();
        for (x, &cell) in packed.iter().enumerate() {
            let id = match bits {
                Some((lsb, mask)) => {
                    let id = ((cell >> lsb) & mask) as u16;
                    debug_assert_eq!(Ok(id), style_id_asked(cell), "the engine's manifest and its getter disagree on a cell's style id");
                    id
                }
                None => style_id_asked(cell)?,
            };
            // (None of its own; or the run's, or one seen before.)
            if id == 0 || id == last || seen[..count].contains(&id) {
                continue;
            }
            if count == seen.len() {
                return Ok(false);
            }
            (seen[count], count, last) = (id, count + 1, id);
            // SAFETY: `x` is a column of the current row, whose cells the iterator holds.
            check(unsafe { ffi::ghostty_render_state_row_cells_select(self.row_cells, x as u16) })?;
            let (now, tag) = self.cell_here(cell, palette)?;
            // (A cell that is only a background carried in its word shows
            // that and not its style's: it can't speak for the style's
            // background, which a letter with the same id shows. A
            // snapshot can hold such a cell with a style; Astra's second
            // review made one, and a B behind it kept its old ground.)
            if matches!(tag, ffi::GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE | ffi::GHOSTTY_CELL_CONTENT_BG_COLOR_RGB) || now != kept[x] {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn read_grapheme_utf8(&mut self) -> Result<String, VtError> {
        let mut buf = ffi::GhosttyBuffer { ptr: self.utf8_scratch.as_mut_ptr(), cap: self.utf8_scratch.len(), len: 0 };
        // SAFETY: The counted buffer points to writable scratch storage and this cell stays current during both reads.
        let res = unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, ptr::from_mut(&mut buf).cast::<c_void>()) };
        if res == ffi::GHOSTTY_OUT_OF_SPACE && buf.len > 0 {
            if buf.len > 1 << 20 {
                return Err(VtError::LimitExceeded);
            }
            self.utf8_scratch.resize(buf.len, 0);
            buf.ptr = self.utf8_scratch.as_mut_ptr();
            buf.cap = self.utf8_scratch.len();
            buf.len = 0;
            // SAFETY: The counted buffer points to writable scratch storage and this cell stays current during both reads.
            let res2 = unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, ptr::from_mut(&mut buf).cast::<c_void>()) };
            check(res2)?;
        } else if res != ffi::GHOSTTY_SUCCESS {
            return Err(VtError::from_ffi(res));
        }
        Ok(String::from_utf8_lossy(self.utf8_scratch.get(..buf.len).ok_or(VtError::InvalidValue)?).into_owned())
    }

    /// Extract Kitty image placements sorted by `(layer, z)` ready to paint.
    pub fn extract_image_placements(&mut self) -> Result<Vec<ImagePlacement>, VtError> {
        self.finish_pending()?;
        // SAFETY: The handle slot contains our reusable iterator. Its render state remains live and stable during iteration.
        check(unsafe { ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR, ptr::from_mut(&mut self.image_iter).cast::<c_void>()) })?;

        let mut out = Vec::new();
        // SAFETY: The iterator was populated by this render state and has not been invalidated by an update.
        while unsafe { ffi::ghostty_render_state_image_iterator_next(self.image_iter) } {
            if out.len() >= 65_536 {
                return Err(VtError::LimitExceeded);
            }
            let mut protocol = ffi::GHOSTTY_IMAGE_PROTOCOL_KITTY;
            // SAFETY: PROTOCOL writes the protocol enum for the current placement.
            unsafe {
                check(ffi::ghostty_render_state_image_get(self.image_iter, ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_PROTOCOL, ptr::from_mut(&mut protocol).cast::<c_void>()))?;
            }
            if protocol != ffi::GHOSTTY_IMAGE_PROTOCOL_KITTY {
                continue;
            }
            let mut image_id = 0u32;
            let mut image_generation = 0u64;
            let mut placement_id = 0u32;
            let mut raw_layer = ffi::GHOSTTY_IMAGE_LAYER_BELOW_TEXT;
            let mut z = 0i32;
            let mut viewport_col = 0i32;
            let mut viewport_row = 0i32;
            let mut grid_cols = 0u32;
            let mut grid_rows = 0u32;
            let mut cell_offset_x = 0u32;
            let mut cell_offset_y = 0u32;
            let mut pixel_width = 0u32;
            let mut pixel_height = 0u32;
            let mut source_x = 0u32;
            let mut source_y = 0u32;
            let mut source_width = 0u32;
            let mut source_height = 0u32;

            // SAFETY: Each image key below has the scalar output type documented in render.h. The placement stays current.
            unsafe {
                macro_rules! get_img {
                    ($k:expr, $v:expr) => {
                        check(ffi::ghostty_render_state_image_get(self.image_iter, $k, ptr::from_mut(&mut $v).cast::<c_void>()))?;
                    };
                }
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_ID, image_id);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_GENERATION, image_generation);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_KITTY_PLACEMENT_ID, placement_id);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_LAYER, raw_layer);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_Z, z);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_COL, viewport_col);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_ROW, viewport_row);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_GRID_COLS, grid_cols);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_GRID_ROWS, grid_rows);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_X, cell_offset_x);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_Y, cell_offset_y);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_WIDTH, pixel_width);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_HEIGHT, pixel_height);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_X, source_x);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_Y, source_y);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_WIDTH, source_width);
                get_img!(ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_HEIGHT, source_height);
            }

            let layer = match raw_layer {
                ffi::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND => ImageLayer::BelowBackground,
                ffi::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT => ImageLayer::AboveText,
                _ => ImageLayer::BelowText,
            };

            out.push(ImagePlacement {
                image_id,
                image_generation,
                placement_id,
                layer,
                z,
                viewport_col,
                viewport_row,
                grid_cols,
                grid_rows,
                cell_offset_x,
                cell_offset_y,
                pixel_width,
                pixel_height,
                source_x,
                source_y,
                source_width,
                source_height,
            });
        }

        out.sort_by_key(|p| (p.layer, p.z));
        Ok(out)
    }
}

/// Olo: a packed cell's style id, asked of the engine (a call a cell).
fn style_id_asked(cell: ffi::GhosttyCell) -> Result<u16, VtError> {
    let mut id = 0u16;
    // SAFETY: STYLE_ID writes a uint16_t read from the packed cell.
    check(unsafe { ffi::ghostty_cell_get(cell, ffi::GHOSTTY_CELL_DATA_STYLE_ID, ptr::from_mut(&mut id).cast::<c_void>()) })?;
    Ok(id)
}

/// Olo: where a packed cell's style id is in its word (its lowest bit,
/// and a mask of its width), as the engine says of itself: the layout
/// of a packed cell isn't part of its ABI, and `ghostty_type_json` is
/// how a caller is told to learn it. Read once. None if the manifest
/// doesn't say it as expected: the id is then asked of the engine,
/// cell by cell (`style_id_asked`: 2,520 calls a frame for a 63 × 40
/// pane of coloured text, a sixth of the frame).
fn style_id_bits() -> Option<(u32, u64)> {
    static BITS: std::sync::OnceLock<Option<(u32, u64)>> = std::sync::OnceLock::new();
    *BITS.get_or_init(|| {
        // SAFETY: The manifest is a static NUL-terminated string owned by the library.
        let json = unsafe { std::ffi::CStr::from_ptr(ffi::ghostty_type_json()) }.to_str().ok()?;
        style_id_bits_in(json)
    })
}

/// (The manifest: `{"schema":1,…,"types":{"GhosttyCell":{"kind":
/// "packed","underlying":"u64","bits":{…,"style_id":{"lsb":26,
/// "width":16,…}}}}}`. Read as JSON, the cell's own field and no
/// other; a schema this wasn't written for, or a field not as
/// expected, is not guessed at.)
fn style_id_bits_in(json: &str) -> Option<(u32, u64)> {
    let manifest: serde_json::Value = serde_json::from_str(json).ok()?;
    if manifest.get("schema")?.as_u64()? != 1 {
        return None;
    }
    let cell = manifest.get("types")?.get("GhosttyCell")?;
    if cell.get("kind")?.as_str()? != "packed" || cell.get("underlying")?.as_str()? != "u64" {
        return None;
    }
    let field = cell.get("bits")?.get("style_id")?;
    let (lsb, width) = (field.get("lsb")?.as_u64()?, field.get("width")?.as_u64()?);
    (width == 16 && lsb <= 48).then_some((lsb as u32, 0xffff))
}

fn resolve_style_color(c: ffi::GhosttyStyleColor, palette: &[Rgb; 256]) -> Option<Rgb> {
    match c.tag as u32 {
        // SAFETY: The palette tag selects the one-byte palette union member (a valid 256-entry index).
        ffi::GHOSTTY_STYLE_COLOR_PALETTE => Some(palette[unsafe { c.value.palette } as usize]),
        // SAFETY: The RGB tag selects the RGB union member.
        ffi::GHOSTTY_STYLE_COLOR_RGB => Some(Rgb::from_ffi(unsafe { c.value.rgb })),
        _ => None,
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: Each initialized handle is uniquely owned. Iterators are freed before their backing render state.
        unsafe {
            if !self.image_iter.is_null() {
                ffi::ghostty_render_state_image_iterator_free(self.image_iter);
            }
            if !self.row_cells.is_null() {
                ffi::ghostty_render_state_row_cells_free(self.row_cells);
            }
            if !self.row_iter.is_null() {
                ffi::ghostty_render_state_row_iterator_free(self.row_iter);
            }
            if !self.raw.is_null() {
                ffi::ghostty_render_state_free(self.raw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GhosttyTerminal;

    /// Rows' ids as the engine makes them (a page's serial, a row's
    /// place in it) land apart in a map of them: a screen and its
    /// history's worth, in a table of twice as many places, no more of
    /// them sharing one than thrown there at random.
    #[test]
    fn rows_ids_land_apart() {
        use std::hash::{BuildHasher, BuildHasherDefault};
        let hasher = BuildHasherDefault::<RowIdHasher>::default();
        let ids: Vec<RowId> = (0..8u64).flat_map(|serial| (0..512u64).map(move |y| RowId([serial, y + 1]))).collect();
        let places = (ids.len() * 2).next_power_of_two() as u64;
        // (A table takes a hash's low bits for where an entry goes, and
        // its top seven to tell entries there apart.)
        let mut taken = std::collections::HashSet::new();
        let (mut low, mut top) = (std::collections::HashMap::new(), std::collections::HashSet::new());
        for id in &ids {
            let hash = hasher.hash_one(id);
            assert!(taken.insert(hash), "two ids with one hash");
            *low.entry(hash & (places - 1)).or_insert(0usize) += 1;
            top.insert(hash >> 57);
        }
        // (Thrown at random, 4,096 ids into 8,192 places take 3,223 of them.)
        assert!(low.len() > 3200 && top.len() > 120, "{} places and {} marks for {} ids", low.len(), top.len(), ids.len());
        let peak = low.values().copied().max().unwrap();
        // (And no place with many: a hasher that put a page's rows in one
        // place took as many places as this asks, all but that page's.)
        assert!(peak <= 4, "{peak} rows in one place");
        let mut map: RowMap<u32> = RowMap::default();
        for (n, id) in ids.iter().enumerate() {
            map.insert(*id, n as u32);
        }
        assert!(ids.iter().enumerate().all(|(n, id)| map.get(id) == Some(&(n as u32))));
    }

    /// The engine's manifest says where a packed cell's style id is, and
    /// it is where it says: for every cell of a screen of text in many
    /// styles, the bits read there are what the engine's own getter
    /// gives. (What `styles_as_kept` walks a row by; a layout the
    /// manifest doesn't give as expected is asked for cell by cell.)
    #[test]
    fn a_cells_style_id_is_where_the_manifest_says() {
        let (lsb, mask) = style_id_bits().expect("the manifest gives the packed cell's style id");
        let with = |cell: &str| format!(r#"{{"schema":1,"types":{{"GhosttyCell":{cell}, "GhosttyX":{{"kind":"packed","underlying":"u64","bits":{{"style_id":{{"lsb":3,"width":16}}}}}}}}}}"#);
        assert_eq!(
            style_id_bits_in(&with(
                r#"{"kind":"packed","underlying":"u64","bits":{"content_tag":{"lsb":0,"width":2},"style_id":{"lsb":26,"width":16,"type":"GhosttyStyleId"},"wide":{"lsb":42,"width":2}}}"#
            )),
            Some((26, 0xffff))
        );
        // Not said, or not as expected: none (the engine is asked), and
        // never another field's or another type's (Astra's second
        // review: a search through the text took `wide`'s 42 for a
        // missing `lsb`, and the next type's 3 for a missing field).
        assert_eq!(style_id_bits_in(&with(r#"{"kind":"packed","underlying":"u64","bits":{"style_id":{"width":16},"wide":{"lsb":42,"width":2}}}"#)), None);
        assert_eq!(style_id_bits_in(&with(r#"{"kind":"packed","underlying":"u64","bits":{"wide":{"lsb":42,"width":2}}}"#)), None);
        assert_eq!(style_id_bits_in(&with(r#"{"kind":"packed","underlying":"u64","bits":{"style_id":{"lsb":26,"width":12}}}"#)), None);
        assert_eq!(style_id_bits_in(&with(r#"{"kind":"packed","underlying":"u32","bits":{"style_id":{"lsb":10,"width":16}}}"#)), None);
        assert_eq!(style_id_bits_in(&with(r#"{"kind":"struct","bits":{"style_id":{"lsb":26,"width":16}}}"#)), None);
        // A schema this wasn't written for.
        assert_eq!(style_id_bits_in(&with(r#"{"kind":"packed","underlying":"u64","bits":{"style_id":{"lsb":26,"width":16}}}"#).replace(r#""schema":1"#, r#""schema":2"#)), None);
        assert_eq!(style_id_bits_in("{}"), None);
        assert_eq!(style_id_bits_in("not json"), None);
        let mut t = GhosttyTerminal::new(40, 12).unwrap();
        for row in 0..12u32 {
            let line = (0..8).map(|i| format!("\x1b[{};{}mab\x1b[0m.", [1, 3, 4, 7][((row + i) % 4) as usize], 31 + (row * 3 + i) % 7)).collect::<String>();
            t.vt_write(format!("\x1b[{};1H{line}", row + 1).as_bytes());
        }
        let mut state = RenderState::new().unwrap();
        state.update(&t).unwrap();
        // SAFETY: The handle slot holds our reusable iterator over the state just updated.
        check(unsafe { ffi::ghostty_render_state_get(state.raw, ffi::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, ptr::from_mut(&mut state.row_iter).cast::<c_void>()) }).unwrap();
        let (mut cells, mut styled, mut ids) = (0, 0, std::collections::BTreeSet::new());
        // SAFETY: The iterator is on a row of that state; CELLS_RAW lends its packed cells until the next update.
        while unsafe { ffi::ghostty_render_state_row_iterator_next(state.row_iter) } {
            let mut view = ffi::GhosttyCellsView { ptr: ptr::null(), len: 0 };
            check(unsafe { ffi::ghostty_render_state_row_get(state.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW, ptr::from_mut(&mut view).cast::<c_void>()) }).unwrap();
            for &cell in unsafe { std::slice::from_raw_parts(view.ptr, view.len) } {
                let id = ((cell >> lsb) & mask) as u16;
                assert_eq!(Ok(id), style_id_asked(cell));
                cells += 1;
                styled += usize::from(id != 0);
                ids.insert(id);
            }
        }
        assert_eq!(cells, 40 * 12);
        assert!(styled >= 12 * 16 && ids.len() > 8, "{styled} cells with a style, {} styles", ids.len());
    }
}
