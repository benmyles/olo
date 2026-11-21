//! High-performance dirty-tracked render state over `GhosttyRenderState`.
//!
//! Designed for Olo's 120 Hz GPU renderer:
//! - Two-phase `begin_update` / `end_update` keeps terminal lock hold time tiny.
//! - Stable `RowId([u64; 2])` + `DirtyState` let `RowCache` skip unchanged rows.
//! - Overscan support enables sub-row elastic scrolling without extra terminal queries.

use std::{ffi::c_void, mem, ptr};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RowId(pub [u64; 2]);

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
}

unsafe impl Send for RenderState {}

impl RenderState {
    pub fn new() -> Result<Self, VtError> {
        crate::sys::init();
        let mut raw: ffi::GhosttyRenderState = ptr::null_mut();
        check(unsafe { ffi::ghostty_render_state_new(ptr::null(), &mut raw) })?;

        let mut row_iter: ffi::GhosttyRenderStateRowIterator = ptr::null_mut();
        if let Err(e) = check(unsafe { ffi::ghostty_render_state_row_iterator_new(ptr::null(), &mut row_iter) }) {
            unsafe { ffi::ghostty_render_state_free(raw) };
            return Err(e);
        }

        let mut row_cells: ffi::GhosttyRenderStateRowCells = ptr::null_mut();
        if let Err(e) = check(unsafe { ffi::ghostty_render_state_row_cells_new(ptr::null(), &mut row_cells) }) {
            unsafe {
                ffi::ghostty_render_state_row_iterator_free(row_iter);
                ffi::ghostty_render_state_free(raw);
            }
            return Err(e);
        }

        let mut image_iter: ffi::GhosttyRenderStateImageIterator = ptr::null_mut();
        if let Err(e) = check(unsafe { ffi::ghostty_render_state_image_iterator_new(ptr::null(), &mut image_iter) }) {
            unsafe {
                ffi::ghostty_render_state_row_cells_free(row_cells);
                ffi::ghostty_render_state_row_iterator_free(row_iter);
                ffi::ghostty_render_state_free(raw);
            }
            return Err(e);
        }

        Ok(Self { raw, row_iter, row_cells, image_iter, utf8_scratch: vec![0u8; 64] })
    }

    /// Request overscan rows above and below the viewport for smooth scrolling.
    pub fn set_overscan(&mut self, above: u16, below: u16) -> Result<(), VtError> {
        let ov = ffi::GhosttyRenderStateOverscan { above, below };
        check(unsafe { ffi::ghostty_render_state_set(self.raw, ffi::GHOSTTY_RENDER_STATE_OPT_OVERSCAN, ptr::from_ref(&ov).cast::<c_void>()) })
    }

    /// Capture dirty terminal state into this render state in one call.
    pub fn update(&mut self, term: &GhosttyTerminal) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_render_state_update(self.raw, term.raw()) })
    }

    /// Phase 1 (while holding terminal lock): copy minimal terminal state.
    pub fn begin_update(&mut self, term: &GhosttyTerminal) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_render_state_begin_update(self.raw, term.raw()) })
    }

    /// Phase 2 (without terminal lock): finish deferred render state work.
    pub fn end_update(&mut self) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_render_state_end_update(self.raw) })
    }

    /// Mark all dirty flags consumed after painting a frame.
    pub fn clean(&mut self) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_render_state_clean(self.raw) })
    }

    /// Force the next frame to treat all rows as dirty (e.g. after theme/font change).
    pub fn mark_full_dirty(&mut self) -> Result<(), VtError> {
        let d = ffi::GHOSTTY_RENDER_STATE_DIRTY_FULL;
        check(unsafe { ffi::ghostty_render_state_set(self.raw, ffi::GHOSTTY_RENDER_STATE_OPT_DIRTY, ptr::from_ref(&d).cast::<c_void>()) })
    }

    pub fn size(&self) -> (u16, u16) {
        let mut cols = 0u16;
        let mut rows = 0u16;
        unsafe {
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_COLS, ptr::from_mut(&mut cols).cast::<c_void>());
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_ROWS, ptr::from_mut(&mut rows).cast::<c_void>());
        }
        (cols, rows)
    }

    pub fn dirty(&self) -> DirtyState {
        let mut raw_d = ffi::GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        unsafe {
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_DIRTY, ptr::from_mut(&mut raw_d).cast::<c_void>());
        }
        match raw_d {
            ffi::GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => DirtyState::Partial,
            ffi::GHOSTTY_RENDER_STATE_DIRTY_FULL => DirtyState::Full,
            _ => DirtyState::Clean,
        }
    }

    pub fn overscan_captured(&self) -> (u16, u16) {
        let mut ov = ffi::GhosttyRenderStateOverscan::default();
        unsafe {
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_OVERSCAN, ptr::from_mut(&mut ov).cast::<c_void>());
        }
        (ov.above, ov.below)
    }

    pub fn cursor(&self) -> CursorInfo {
        let mut c = ffi::GhosttyRenderStateCursor { size: mem::size_of::<ffi::GhosttyRenderStateCursor>(), ..Default::default() };
        unsafe {
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_CURSOR, ptr::from_mut(&mut c).cast::<c_void>());
        }
        CursorInfo {
            viewport_pos: c.viewport_has_value.then_some((c.viewport_x, c.viewport_y)),
            wide_tail: c.wide_tail,
            visible: c.visible,
            blinking: c.blinking,
            password_input: c.password_input,
            style: CursorStyle::from_ffi(c.visual_style),
        }
    }

    pub fn colors(&self) -> RenderColors {
        let mut c = ffi::GhosttyRenderStateColors { size: mem::size_of::<ffi::GhosttyRenderStateColors>(), ..Default::default() };
        unsafe {
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_COLORS, ptr::from_mut(&mut c).cast::<c_void>());
        }
        let palette = std::array::from_fn(|i| Rgb::from_ffi(c.palette[i]));
        RenderColors { background: Rgb::from_ffi(c.background), foreground: Rgb::from_ffi(c.foreground), cursor: c.cursor_has_value.then(|| Rgb::from_ffi(c.cursor)), palette }
    }

    pub fn image_placements_generation(&self) -> u64 {
        let mut generation = 0u64;
        unsafe {
            let _ = ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_IMAGE_PLACEMENTS_GENERATION, ptr::from_mut(&mut generation).cast::<c_void>());
        }
        generation
    }

    /// Extract visible rows (including any captured overscan rows).
    ///
    /// When `only_dirty` is true and global dirty is `Partial`, skips populating
    /// `cells` on clean rows (still returning row metadata so callers can map
    /// `RowId` -> `viewport_y` effortlessly!).
    pub fn extract_rows(&mut self, palette: &[Rgb; 256], only_dirty: bool) -> Result<Vec<RenderRow>, VtError> {
        let global_dirty = self.dirty();
        let (cols, rows) = self.size();
        let (ov_above, ov_below) = self.overscan_captured();
        let cap = rows as usize + ov_above as usize + ov_below as usize;

        check(unsafe { ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, ptr::from_mut(&mut self.row_iter).cast::<c_void>()) })?;

        let mut out = Vec::with_capacity(cap);
        while unsafe { ffi::ghostty_render_state_row_iterator_next(self.row_iter) } {
            let mut row_id = ffi::GhosttyRenderStateRowId::default();
            let mut viewport_y = 0i32;
            let mut row_dirty = false;
            let mut raw_row: ffi::GhosttyRow = 0;
            let mut sel = ffi::GhosttyRenderStateRowSelection { size: mem::size_of::<ffi::GhosttyRenderStateRowSelection>(), start_x: 0, end_x: 0 };
            unsafe {
                let _ = ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_ID, ptr::from_mut(&mut row_id).cast::<c_void>());
                let _ = ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y, ptr::from_mut(&mut viewport_y).cast::<c_void>());
                let _ = ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, ptr::from_mut(&mut row_dirty).cast::<c_void>());
                let _ = ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_RAW, ptr::from_mut(&mut raw_row).cast::<c_void>());
            }
            let selection =
                if unsafe { ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION, ptr::from_mut(&mut sel).cast::<c_void>()) } == ffi::GHOSTTY_SUCCESS {
                    Some((sel.start_x, sel.end_x))
                } else {
                    None
                };

            let mut wrap = false;
            let mut wrap_cont = false;
            unsafe {
                let _ = ffi::ghostty_row_get(raw_row, ffi::GHOSTTY_ROW_DATA_WRAP, ptr::from_mut(&mut wrap).cast::<c_void>());
                let _ = ffi::ghostty_row_get(raw_row, ffi::GHOSTTY_ROW_DATA_WRAP_CONTINUATION, ptr::from_mut(&mut wrap_cont).cast::<c_void>());
            }

            let effective_dirty = match global_dirty {
                DirtyState::Full => true,
                DirtyState::Partial => row_dirty,
                DirtyState::Clean => false,
            };

            let cells = if only_dirty && !effective_dirty { Vec::new() } else { self.extract_current_row_cells(cols as usize, palette)? };

            out.push(RenderRow { id: RowId(row_id.bits), viewport_y, dirty: effective_dirty, wrap, wrap_continuation: wrap_cont, selection, cells });
        }
        Ok(out)
    }

    fn extract_current_row_cells(&mut self, cols: usize, palette: &[Rgb; 256]) -> Result<Vec<RenderCell>, VtError> {
        check(unsafe { ffi::ghostty_render_state_row_get(self.row_iter, ffi::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, ptr::from_mut(&mut self.row_cells).cast::<c_void>()) })?;

        let mut cells = Vec::with_capacity(cols);
        while unsafe { ffi::ghostty_render_state_row_cells_next(self.row_cells) } {
            let mut raw_cell: ffi::GhosttyCell = 0;
            unsafe {
                let _ = ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, ptr::from_mut(&mut raw_cell).cast::<c_void>());
            }

            let mut content_tag = ffi::GHOSTTY_CELL_CONTENT_CODEPOINT;
            let mut wide_raw = ffi::GHOSTTY_CELL_WIDE_NARROW;
            let mut codepoint = 0u32;
            let mut has_styling = false;
            let mut hyperlink = false;
            unsafe {
                let _ = ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_CONTENT_TAG, ptr::from_mut(&mut content_tag).cast::<c_void>());
                let _ = ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_WIDE, ptr::from_mut(&mut wide_raw).cast::<c_void>());
                let _ = ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_CODEPOINT, ptr::from_mut(&mut codepoint).cast::<c_void>());
                let _ = ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_HAS_STYLING, ptr::from_mut(&mut has_styling).cast::<c_void>());
                let _ = ffi::ghostty_cell_get(raw_cell, ffi::GHOSTTY_CELL_DATA_HAS_HYPERLINK, ptr::from_mut(&mut hyperlink).cast::<c_void>());
            }

            let wide = match wide_raw {
                ffi::GHOSTTY_CELL_WIDE_WIDE => CellWide::Wide,
                ffi::GHOSTTY_CELL_WIDE_SPACER_TAIL => CellWide::SpacerTail,
                ffi::GHOSTTY_CELL_WIDE_SPACER_HEAD => CellWide::SpacerHead,
                _ => CellWide::Narrow,
            };

            let grapheme = if content_tag == ffi::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME { self.read_grapheme_utf8() } else { String::new() };

            let mut bg_raw = ffi::GhosttyColorRgb::default();
            let bg = if unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, ptr::from_mut(&mut bg_raw).cast::<c_void>()) }
                == ffi::GHOSTTY_SUCCESS
            {
                Some(Rgb::from_ffi(bg_raw))
            } else {
                None
            };

            let (fg, style) = if has_styling {
                let mut fg_raw = ffi::GhosttyColorRgb::default();
                let fg = if unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, ptr::from_mut(&mut fg_raw).cast::<c_void>()) }
                    == ffi::GHOSTTY_SUCCESS
                {
                    Some(Rgb::from_ffi(fg_raw))
                } else {
                    None
                };

                let mut st = ffi::GhosttyStyle::default();
                unsafe {
                    let _ = ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, ptr::from_mut(&mut st).cast::<c_void>());
                }

                let underline_color = resolve_style_color(st.underline_color, palette);
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

            cells.push(RenderCell { codepoint, grapheme, wide, fg, bg, style, hyperlink });
        }

        Ok(cells)
    }

    fn read_grapheme_utf8(&mut self) -> String {
        let mut buf = ffi::GhosttyBuffer { ptr: self.utf8_scratch.as_mut_ptr(), cap: self.utf8_scratch.len(), len: 0 };
        let res = unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, ptr::from_mut(&mut buf).cast::<c_void>()) };
        if res == ffi::GHOSTTY_OUT_OF_SPACE && buf.len > 0 {
            self.utf8_scratch.resize(buf.len, 0);
            buf.ptr = self.utf8_scratch.as_mut_ptr();
            buf.cap = self.utf8_scratch.len();
            buf.len = 0;
            let res2 = unsafe { ffi::ghostty_render_state_row_cells_get(self.row_cells, ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, ptr::from_mut(&mut buf).cast::<c_void>()) };
            if res2 != ffi::GHOSTTY_SUCCESS {
                return String::new();
            }
        } else if res != ffi::GHOSTTY_SUCCESS {
            return String::new();
        }
        String::from_utf8_lossy(&self.utf8_scratch[..buf.len]).into_owned()
    }

    /// Extract Kitty image placements sorted by `(layer, z)` ready to paint.
    pub fn extract_image_placements(&mut self) -> Result<Vec<ImagePlacement>, VtError> {
        check(unsafe { ffi::ghostty_render_state_get(self.raw, ffi::GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR, ptr::from_mut(&mut self.image_iter).cast::<c_void>()) })?;

        let mut out = Vec::new();
        while unsafe { ffi::ghostty_render_state_image_iterator_next(self.image_iter) } {
            let mut protocol = ffi::GHOSTTY_IMAGE_PROTOCOL_KITTY;
            unsafe {
                let _ = ffi::ghostty_render_state_image_get(self.image_iter, ffi::GHOSTTY_RENDER_STATE_IMAGE_DATA_PROTOCOL, ptr::from_mut(&mut protocol).cast::<c_void>());
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

            unsafe {
                macro_rules! get_img {
                    ($k:expr, $v:expr) => {
                        let _ = ffi::ghostty_render_state_image_get(self.image_iter, $k, ptr::from_mut(&mut $v).cast::<c_void>());
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

fn resolve_style_color(c: ffi::GhosttyStyleColor, palette: &[Rgb; 256]) -> Option<Rgb> {
    match c.tag as u32 {
        ffi::GHOSTTY_STYLE_COLOR_PALETTE => Some(palette[unsafe { c.value.palette } as usize]),
        ffi::GHOSTTY_STYLE_COLOR_RGB => Some(Rgb::from_ffi(unsafe { c.value.rgb })),
        _ => None,
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
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
