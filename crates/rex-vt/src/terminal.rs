//! Safe Rust wrapper around `GhosttyTerminal`, plus Kitty image pixel access,
//! selection helpers, OSC 133 prompt navigation, formatting, and the `VtEngine` trait.

use std::{ffi::c_void, mem, ptr, slice};

use crate::{
    color::TerminalTheme,
    error::{VtError, check},
    ffi,
    render::CursorStyle,
};

/// Active screen buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKind {
    Primary,
    Alternate,
}

impl ScreenKind {
    pub fn from_ffi(v: ffi::GhosttyTerminalScreen) -> Self {
        match v {
            ffi::GHOSTTY_TERMINAL_SCREEN_ALTERNATE => Self::Alternate,
            _ => Self::Primary,
        }
    }
}

/// Scrollbar geometry reported by the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollbarState {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

impl ScrollbarState {
    /// Rows scrolled up from the live bottom (`0` = at the bottom).
    pub fn display_offset_from_bottom(&self) -> u64 {
        self.total.saturating_sub(self.offset + self.len)
    }

    /// Total scrollback history rows above the active screen.
    pub fn history_rows(&self) -> u64 {
        self.total.saturating_sub(self.len)
    }
}

/// Formatted screen output kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Plain,
    Vt,
    Html,
}

/// Decoded RGBA8 Kitty graphics image pixels ready for GPU texture upload.
#[derive(Debug, Clone)]
pub struct DecodedKittyImage {
    pub id: u32,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
}

/// Memory usage breakdown reported by `libghostty-vt`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalMemoryUsage {
    pub compression_supported: bool,
    pub resident_bytes: u64,
    pub compressed_bytes: u64,
    pub image_bytes: u64,
    pub pages: u64,
    pub compressed_pages: u64,
}

/// Clean trait boundary abstracting the terminal engine.
pub trait VtEngine: Send {
    fn vt_write(&mut self, bytes: &[u8]);
    fn resize(&mut self, cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32) -> Result<(), VtError>;
    fn reset(&mut self);
    fn size(&self) -> (u16, u16);
    fn active_screen(&self) -> ScreenKind;
    fn scrollbar(&self) -> ScrollbarState;
    fn scroll_delta(&mut self, delta: isize) -> Result<(), VtError>;
    fn scroll_to_top(&mut self) -> Result<(), VtError>;
    fn scroll_to_bottom(&mut self) -> Result<(), VtError>;
    fn scroll_to_row(&mut self, row: usize) -> Result<(), VtError>;
    fn scroll_prompt_delta(&mut self, delta_prompts: isize) -> Result<(), VtError>;
    fn apply_theme(&mut self, theme: &TerminalTheme) -> Result<(), VtError>;
}

/// Safe owned wrapper around `GhosttyTerminal`.
pub struct GhosttyTerminal {
    raw: ffi::GhosttyTerminal,
}

unsafe impl Send for GhosttyTerminal {}

impl GhosttyTerminal {
    /// Create a fresh terminal with `cols` × `rows` dimensions.
    pub fn new(cols: u16, rows: u16) -> Result<Self, VtError> {
        crate::sys::init();
        let mut raw: ffi::GhosttyTerminal = ptr::null_mut();
        check(unsafe { ffi::ghostty_terminal_new(ptr::null(), &mut raw, cols.max(1), rows.max(1)) })?;
        Ok(Self { raw })
    }

    /// Take ownership of a raw `GhosttyTerminal` returned by `SnapshotDecoder::ready`.
    ///
    /// # Safety
    /// `raw` must be a valid non-null handle owned by the caller.
    pub unsafe fn from_raw(raw: ffi::GhosttyTerminal) -> Self {
        Self { raw }
    }

    #[inline]
    pub fn raw(&self) -> ffi::GhosttyTerminal {
        self.raw
    }

    /// Feed raw VT bytes into the terminal state machine.
    pub fn vt_write(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            unsafe { ffi::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) };
        }
    }

    /// Resize grid and pixel cell dimensions (triggers reflow on primary screen).
    pub fn resize(&mut self, cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_terminal_resize(self.raw, cols.max(1), rows.max(1), cell_width_px, cell_height_px) })
    }

    /// Full RIS reset preserving dimensions and default colors.
    pub fn reset(&mut self) {
        unsafe { ffi::ghostty_terminal_reset(self.raw) };
    }

    /// Current `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        let mut cols = 0u16;
        let mut rows = 0u16;
        unsafe {
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_COLS, ptr::from_mut(&mut cols).cast::<c_void>());
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_ROWS, ptr::from_mut(&mut rows).cast::<c_void>());
        }
        (cols, rows)
    }

    pub fn active_screen(&self) -> ScreenKind {
        let mut s = ffi::GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, ptr::from_mut(&mut s).cast::<c_void>());
        }
        ScreenKind::from_ffi(s)
    }

    pub fn scrollbar(&self) -> ScrollbarState {
        let mut sb = ffi::GhosttyTerminalScrollbar::default();
        unsafe {
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_SCROLLBAR, ptr::from_mut(&mut sb).cast::<c_void>());
        }
        ScrollbarState { total: sb.total, offset: sb.offset, len: sb.len }
    }

    pub fn is_viewport_at_bottom(&self) -> bool {
        let mut at_bottom = true;
        unsafe {
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE, ptr::from_mut(&mut at_bottom).cast::<c_void>());
        }
        at_bottom
    }

    pub fn mouse_tracking(&self) -> bool {
        let mut on = false;
        unsafe {
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, ptr::from_mut(&mut on).cast::<c_void>());
        }
        on
    }

    pub fn kitty_keyboard_flags(&self) -> u8 {
        let mut flags = 0u8;
        unsafe {
            let _ = ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, ptr::from_mut(&mut flags).cast::<c_void>());
        }
        flags
    }

    /// Check whether a DEC private mode (e.g. `2004` bracketed paste, `1004` focus events) is set.
    pub fn dec_mode(&self, mode: u16) -> bool {
        let mut val = ffi::GhosttyTerminalModeConfig { mode: mode & 0x7FFF, value: false };
        let res = unsafe { ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_MODE, ptr::from_mut(&mut val).cast::<c_void>()) };
        res == ffi::GHOSTTY_SUCCESS && val.value
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.dec_mode(2004)
    }

    pub fn focus_reporting_mode(&self) -> bool {
        self.dec_mode(1004)
    }

    pub fn set_default_cursor_style(&mut self, style: CursorStyle, blink: bool) -> Result<(), VtError> {
        let st = style.to_ffi();
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE, ptr::from_ref(&st).cast::<c_void>()) })?;
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK, ptr::from_ref(&blink).cast::<c_void>()) })
    }

    /// Apply default foreground, background, cursor, and 256-color palette.
    pub fn apply_theme(&mut self, theme: &TerminalTheme) -> Result<(), VtError> {
        let fg = theme.foreground.to_ffi();
        let bg = theme.background.to_ffi();
        let pal: [ffi::GhosttyColorRgb; 256] = std::array::from_fn(|i| theme.palette_256()[i].to_ffi());
        unsafe {
            check(ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, ptr::from_ref(&fg).cast::<c_void>()))?;
            check(ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, ptr::from_ref(&bg).cast::<c_void>()))?;
            if let Some(cur) = theme.cursor {
                let c = cur.to_ffi();
                check(ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, ptr::from_ref(&c).cast::<c_void>()))?;
            } else {
                check(ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, ptr::null()))?;
            }
            check(ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, pal.as_ptr().cast::<c_void>()))?;
        }
        Ok(())
    }

    pub fn scroll_delta(&mut self, delta: isize) -> Result<(), VtError> {
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA;
        behavior.value.delta = delta;
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw, behavior) })
    }

    pub fn scroll_to_top(&mut self) -> Result<(), VtError> {
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP;
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw, behavior) })
    }

    pub fn scroll_to_bottom(&mut self) -> Result<(), VtError> {
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM;
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw, behavior) })
    }

    pub fn scroll_to_row(&mut self, row: usize) -> Result<(), VtError> {
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW;
        behavior.value.row = row;
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw, behavior) })
    }

    /// Jump forward/backward by `delta_prompts` OSC 133 semantic prompts.
    pub fn scroll_prompt_delta(&mut self, delta_prompts: isize) -> Result<(), VtError> {
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA_PROMPT;
        behavior.value.delta_prompt = delta_prompts;
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw, behavior) })
    }

    /// Look up the OSC 8 hyperlink URI at viewport `(col, row)`, if any.
    pub fn hyperlink_at_viewport(&self, col: u16, row: u32) -> Option<String> {
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        if unsafe { ffi::ghostty_terminal_grid_ref(self.raw, pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
            return None;
        }
        let mut needed = 0usize;
        let res = unsafe { ffi::ghostty_grid_ref_hyperlink_uri(&gref, ptr::null_mut(), 0, &mut needed) };
        if res == ffi::GHOSTTY_SUCCESS && needed == 0 {
            return None;
        }
        if res != ffi::GHOSTTY_OUT_OF_SPACE || needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed];
        let mut written = 0usize;
        if unsafe { ffi::ghostty_grid_ref_hyperlink_uri(&gref, buf.as_mut_ptr(), buf.len(), &mut written) } != ffi::GHOSTTY_SUCCESS {
            return None;
        }
        buf.truncate(written);
        String::from_utf8(buf).ok()
    }

    /// Clear any active terminal selection.
    pub fn clear_selection(&mut self) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null()) })
    }

    /// Set a selection between two viewport cell coordinates (inclusive).
    pub fn set_viewport_selection(&mut self, start: (u16, u32), end: (u16, u32), rectangle: bool) -> Result<(), VtError> {
        let mut s_pt = ffi::GhosttyPoint::default();
        s_pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        s_pt.value.coordinate = ffi::GhosttyPointCoordinate { x: start.0, y: start.1 };
        let mut e_pt = ffi::GhosttyPoint::default();
        e_pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        e_pt.value.coordinate = ffi::GhosttyPointCoordinate { x: end.0, y: end.1 };
        let mut sel = ffi::GhosttySelection { rectangle, ..Default::default() };
        check(unsafe { ffi::ghostty_terminal_grid_ref(self.raw, s_pt, &mut sel.start) })?;
        check(unsafe { ffi::ghostty_terminal_grid_ref(self.raw, e_pt, &mut sel.end) })?;
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })
    }

    /// Select the word under viewport `(col, row)`.
    pub fn select_word_at_viewport(&mut self, col: u16, row: u32) -> Result<bool, VtError> {
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        if unsafe { ffi::ghostty_terminal_grid_ref(self.raw, pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
            return Ok(false);
        }
        let opts = ffi::GhosttyTerminalSelectWordOptions { size: mem::size_of::<ffi::GhosttyTerminalSelectWordOptions>(), ref_: gref, boundary_codepoints: ptr::null(), boundary_codepoints_len: 0 };
        let mut sel = ffi::GhosttySelection::default();
        let res = unsafe { ffi::ghostty_terminal_select_word(self.raw, &opts, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Select the logical line under viewport `(col, row)`.
    pub fn select_line_at_viewport(&mut self, col: u16, row: u32) -> Result<bool, VtError> {
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        if unsafe { ffi::ghostty_terminal_grid_ref(self.raw, pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
            return Ok(false);
        }
        let opts = ffi::GhosttyTerminalSelectLineOptions {
            size: mem::size_of::<ffi::GhosttyTerminalSelectLineOptions>(),
            ref_: gref,
            whitespace: ptr::null(),
            whitespace_len: 0,
            semantic_prompt_boundary: true,
        };
        let mut sel = ffi::GhosttySelection::default();
        let res = unsafe { ffi::ghostty_terminal_select_line(self.raw, &opts, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Select the semantic OSC 133 command output block containing viewport `(col, row)`.
    pub fn select_output_at_viewport(&mut self, col: u16, row: u32) -> Result<bool, VtError> {
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        if unsafe { ffi::ghostty_terminal_grid_ref(self.raw, pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
            return Ok(false);
        }
        let mut sel = ffi::GhosttySelection::default();
        let res = unsafe { ffi::ghostty_terminal_select_output(self.raw, &gref, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Select all content on the active screen + scrollback.
    pub fn select_all(&mut self) -> Result<bool, VtError> {
        let mut sel = ffi::GhosttySelection::default();
        let res = unsafe { ffi::ghostty_terminal_select_all(self.raw, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        check(unsafe { ffi::ghostty_terminal_set(self.raw, ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Extract the currently selected text (unwrapped and trimmed like Ghostty).
    pub fn selection_text(&self) -> Result<Option<String>, VtError> {
        let mut sel = ffi::GhosttySelection::default();
        let res = unsafe { ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_SELECTION, ptr::from_mut(&mut sel).cast::<c_void>()) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check(res)?;
        Ok(Some(self.format_inner(FormatKind::Plain, true, true, Some(&sel))?))
    }

    /// Format the active screen & scrollback as plain text, VT, or HTML.
    pub fn format(&self, kind: FormatKind, unwrap: bool, trim: bool) -> Result<String, VtError> {
        self.format_inner(kind, unwrap, trim, None)
    }

    fn format_inner(&self, kind: FormatKind, unwrap: bool, trim: bool, selection: Option<&ffi::GhosttySelection>) -> Result<String, VtError> {
        let mut fmt: ffi::GhosttyFormatter = ptr::null_mut();
        check(unsafe { ffi::ghostty_formatter_terminal_new(ptr::null(), &mut fmt, self.raw) })?;
        struct Guard(ffi::GhosttyFormatter);
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe { ffi::ghostty_formatter_free(self.0) };
            }
        }
        let guard = Guard(fmt);

        let emit = match kind {
            FormatKind::Plain => ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN,
            FormatKind::Vt => ffi::GHOSTTY_FORMATTER_FORMAT_VT,
            FormatKind::Html => ffi::GHOSTTY_FORMATTER_FORMAT_HTML,
        };
        unsafe {
            check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_EMIT, ptr::from_ref(&emit).cast::<c_void>()))?;
            check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_UNWRAP, ptr::from_ref(&unwrap).cast::<c_void>()))?;
            check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_TRIM, ptr::from_ref(&trim).cast::<c_void>()))?;
            if let Some(sel) = selection {
                check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_SELECTION, (sel as *const ffi::GhosttySelection).cast::<c_void>()))?;
            }
        }

        let mut out_ptr: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        check(unsafe { ffi::ghostty_formatter_format_alloc(guard.0, ptr::null(), &mut out_ptr, &mut out_len) })?;
        let text = if !out_ptr.is_null() && out_len > 0 {
            let bytes = unsafe { slice::from_raw_parts(out_ptr, out_len) };
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            String::new()
        };
        if !out_ptr.is_null() {
            unsafe { ffi::ghostty_free(ptr::null(), out_ptr, out_len) };
        }
        Ok(text)
    }

    /// Encode this terminal to a `GHOSTSNP` binary snapshot (useful for testing and local state).
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, VtError> {
        let mut ptr_out: *mut u8 = ptr::null_mut();
        let mut len_out: usize = 0;
        check(unsafe { ffi::ghostty_snapshot_encode_alloc(self.raw, ptr::null(), &mut ptr_out, &mut len_out) })?;
        let bytes = if !ptr_out.is_null() && len_out > 0 { unsafe { slice::from_raw_parts(ptr_out, len_out) }.to_vec() } else { Vec::new() };
        if !ptr_out.is_null() {
            unsafe { ffi::ghostty_free(ptr::null(), ptr_out, len_out) };
        }
        Ok(bytes)
    }

    /// Compress idle scrollback pages (`incremental = true` does bounded work per call).
    pub fn compress_scrollback(&mut self, incremental: bool) -> bool {
        let mode = if incremental { ffi::GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL } else { ffi::GHOSTTY_TERMINAL_COMPRESSION_MODE_FULL };
        let mut result = ffi::GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE;
        let _ = unsafe { ffi::ghostty_terminal_compress(self.raw, mode, &mut result) };
        result == ffi::GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING
    }

    /// Query memory held by this terminal without decompressing scrollback.
    pub fn memory_usage(&self) -> TerminalMemoryUsage {
        let mut m = ffi::GhosttyTerminalMemoryUsage { size: mem::size_of::<ffi::GhosttyTerminalMemoryUsage>(), ..Default::default() };
        let _ = unsafe { ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_MEMORY_USAGE, ptr::from_mut(&mut m).cast::<c_void>()) };
        TerminalMemoryUsage {
            compression_supported: m.compression_supported,
            resident_bytes: m.primary_resident_bytes + m.alternate_resident_bytes,
            compressed_bytes: m.primary_compressed_bytes + m.alternate_compressed_bytes,
            image_bytes: m.primary_image_bytes + m.alternate_image_bytes,
            pages: m.primary_pages + m.alternate_pages,
            compressed_pages: m.primary_compressed_pages + m.alternate_compressed_pages,
        }
    }

    /// Fetch decoded RGBA8 pixels for a Kitty graphics image by `image_id`.
    pub fn kitty_image_rgba(&self, image_id: u32) -> Option<DecodedKittyImage> {
        let mut storage: ffi::GhosttyKittyGraphics = ptr::null_mut();
        if unsafe { ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, ptr::from_mut(&mut storage).cast::<c_void>()) } != ffi::GHOSTTY_SUCCESS || storage.is_null() {
            return None;
        }
        let img = unsafe { ffi::ghostty_kitty_graphics_image(storage, image_id) };
        if img.is_null() {
            return None;
        }
        let mut width = 0u32;
        let mut height = 0u32;
        let mut generation = 0u64;
        let mut format = ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA;
        let mut pixels = ffi::GhosttyString::default();
        unsafe {
            if ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS, ptr::from_mut(&mut pixels).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS
                || pixels.ptr.is_null()
                || pixels.len == 0
            {
                return None;
            }
            let _ = ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH, ptr::from_mut(&mut width).cast::<c_void>());
            let _ = ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT, ptr::from_mut(&mut height).cast::<c_void>());
            let _ = ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_GENERATION, ptr::from_mut(&mut generation).cast::<c_void>());
            let _ = ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT, ptr::from_mut(&mut format).cast::<c_void>());
        }
        let raw = unsafe { slice::from_raw_parts(pixels.ptr, pixels.len) };
        let rgba = match format {
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => raw.to_vec(),
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGB => {
                let mut out = Vec::with_capacity((raw.len() / 3) * 4);
                for ch in raw.chunks_exact(3) {
                    out.extend_from_slice(&[ch[0], ch[1], ch[2], 255]);
                }
                out
            }
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => {
                let mut out = Vec::with_capacity((raw.len() / 2) * 4);
                for ch in raw.chunks_exact(2) {
                    out.extend_from_slice(&[ch[0], ch[0], ch[0], ch[1]]);
                }
                out
            }
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => {
                let mut out = Vec::with_capacity(raw.len() * 4);
                for &v in raw {
                    out.extend_from_slice(&[v, v, v, 255]);
                }
                out
            }
            _ => return None,
        };
        Some(DecodedKittyImage { id: image_id, generation, width, height, rgba })
    }

    /// Advance playing Kitty graphics animations at monotonic clock `now_ms`.
    /// Returns `Some(next_ms)` when another animation frame is scheduled.
    pub fn kitty_animation_tick(&mut self, now_ms: u64) -> Option<u64> {
        let mut storage: ffi::GhosttyKittyGraphics = ptr::null_mut();
        if unsafe { ffi::ghostty_terminal_get(self.raw, ffi::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, ptr::from_mut(&mut storage).cast::<c_void>()) } != ffi::GHOSTTY_SUCCESS || storage.is_null() {
            return None;
        }
        let mut next_ms = 0u64;
        let res = unsafe { ffi::ghostty_kitty_graphics_animation_tick(storage, now_ms, &mut next_ms) };
        (res == ffi::GHOSTTY_SUCCESS).then_some(next_ms)
    }

    /// Extract visible plain text of viewport row `row` (0-indexed from top of viewport).
    pub fn viewport_row_text(&self, row: u32) -> String {
        let (cols, rows) = self.size();
        if row >= rows as u32 {
            return String::new();
        }
        let mut out = String::with_capacity(cols as usize);
        let mut cp_buf = [0u32; 8];
        for col in 0..cols {
            let mut pt = ffi::GhosttyPoint::default();
            pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
            pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
            let mut gref = ffi::GhosttyGridRef::default();
            if unsafe { ffi::ghostty_terminal_grid_ref(self.raw, pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
                out.push(' ');
                continue;
            }
            let mut written = 0usize;
            let res = unsafe { ffi::ghostty_grid_ref_graphemes(&gref, cp_buf.as_mut_ptr(), cp_buf.len(), &mut written) };
            if res == ffi::GHOSTTY_SUCCESS && written > 0 {
                for &cp in &cp_buf[..written] {
                    if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                    }
                }
            } else if res == ffi::GHOSTTY_OUT_OF_SPACE && written > 0 {
                let mut dyn_cp = vec![0u32; written];
                let mut w2 = 0usize;
                if unsafe { ffi::ghostty_grid_ref_graphemes(&gref, dyn_cp.as_mut_ptr(), dyn_cp.len(), &mut w2) } == ffi::GHOSTTY_SUCCESS {
                    for &cp in &dyn_cp[..w2] {
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        }
                    }
                }
            } else {
                out.push(' ');
            }
        }
        out.trim_end().to_owned()
    }
}

impl VtEngine for GhosttyTerminal {
    fn vt_write(&mut self, bytes: &[u8]) {
        GhosttyTerminal::vt_write(self, bytes);
    }
    fn resize(&mut self, cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32) -> Result<(), VtError> {
        GhosttyTerminal::resize(self, cols, rows, cell_width_px, cell_height_px)
    }
    fn reset(&mut self) {
        GhosttyTerminal::reset(self);
    }
    fn size(&self) -> (u16, u16) {
        GhosttyTerminal::size(self)
    }
    fn active_screen(&self) -> ScreenKind {
        GhosttyTerminal::active_screen(self)
    }
    fn scrollbar(&self) -> ScrollbarState {
        GhosttyTerminal::scrollbar(self)
    }
    fn scroll_delta(&mut self, delta: isize) -> Result<(), VtError> {
        GhosttyTerminal::scroll_delta(self, delta)
    }
    fn scroll_to_top(&mut self) -> Result<(), VtError> {
        GhosttyTerminal::scroll_to_top(self)
    }
    fn scroll_to_bottom(&mut self) -> Result<(), VtError> {
        GhosttyTerminal::scroll_to_bottom(self)
    }
    fn scroll_to_row(&mut self, row: usize) -> Result<(), VtError> {
        GhosttyTerminal::scroll_to_row(self, row)
    }
    fn scroll_prompt_delta(&mut self, delta_prompts: isize) -> Result<(), VtError> {
        GhosttyTerminal::scroll_prompt_delta(self, delta_prompts)
    }
    fn apply_theme(&mut self, theme: &TerminalTheme) -> Result<(), VtError> {
        GhosttyTerminal::apply_theme(self, theme)
    }
}

impl Drop for GhosttyTerminal {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { ffi::ghostty_terminal_free(self.raw) };
        }
    }
}
