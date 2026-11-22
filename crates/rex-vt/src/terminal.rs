//! Safe Rust wrapper around `GhosttyTerminal`, plus Kitty image pixel access,
//! selection helpers, OSC 133 prompt navigation, formatting, and the `VtEngine` trait.

use std::{ffi::c_void, mem, ptr, slice, sync::Arc};

use crate::limits::{self, Budget};
use olo_sync::{Mutex, MutexGuard};

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
        self.total.saturating_sub(self.offset.saturating_add(self.len))
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
    /// Physical grid-page bytes, including compressed pages.
    pub resident_bytes: u64,
    /// Address space reserved for pages, including spare pages.
    pub virtual_bytes: u64,
    /// Allocations charged to the pane heap (including images and attached searches).
    /// This overlaps image_bytes; it excludes grid page mappings and Rust-side render data.
    pub heap_bytes: usize,
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
    owner: Arc<OwnedTerminal>,
}

pub(crate) struct Shared {
    revision: Mutex<u64>,
    pub budget: Budget,
}
impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { revision: Mutex::new(0), budget: Budget::new() })
    }
    pub fn change(&self) -> MutexGuard<'_, u64> {
        let mut revision = self.lock();
        *revision = revision.wrapping_add(1);
        revision
    }
    pub fn lock(&self) -> MutexGuard<'_, u64> {
        self.revision.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct OwnedTerminal {
    raw: ffi::GhosttyTerminal,
    shared: Arc<Shared>,
}
// SAFETY: all wrapper access, including native destruction and dependent search
// or snapshot operations, takes shared.access. The allocator has no thread affinity.
unsafe impl Send for OwnedTerminal {}
// SAFETY: the immutable pointer is only used behind the same per-terminal lock.
unsafe impl Sync for OwnedTerminal {}

impl GhosttyTerminal {
    /// Create a grid of up to 262,144 cells with at most 4096 on either axis.
    pub fn new(cols: u16, rows: u16) -> Result<Self, VtError> {
        limits::grid(cols, rows)?;
        crate::sys::init()?;
        let shared = Shared::new();
        let mut raw = ptr::null_mut();
        // SAFETY: the grid is bounded, raw is writable, and shared keeps the
        // allocator alive until the last native terminal or decoder is freed.
        check(unsafe { ffi::ghostty_terminal_new(shared.budget.raw(), &mut raw, cols, rows) })?;
        Self::owned(raw, shared)
    }

    // Called only on successful native construction, with the originating allocator.
    pub(crate) fn owned(raw: ffi::GhosttyTerminal, shared: Arc<Shared>) -> Result<Self, VtError> {
        if raw.is_null() {
            return Err(VtError::InvalidValue);
        }
        Ok(Self { owner: Arc::new(OwnedTerminal { raw, shared }) })
    }

    /// Adopt a native terminal.
    ///
    /// # Safety
    /// The caller transfers exclusive ownership of a live non-null terminal.
    /// Any custom allocator or callbacks must outlive it, support calls on
    /// any thread, and already be registered. There must be no
    /// outstanding search, decoder or other unsynchronized user of the handle.
    pub unsafe fn from_raw(raw: ffi::GhosttyTerminal) -> Self {
        Self { owner: Arc::new(OwnedTerminal { raw, shared: Shared::new() }) }
    }

    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.owner.shared
    }
    pub(crate) fn lock(&self) -> MutexGuard<'_, u64> {
        self.shared().lock()
    }
    pub(crate) fn retain_for_decoder(&self) -> Self {
        Self { owner: self.owner.clone() }
    }
    /// Borrowed handle. Any external native call must satisfy Ghostty's
    /// synchronization and lifetime rules, including access by a bound search.
    pub fn raw(&self) -> ffi::GhosttyTerminal {
        self.owner.raw
    }

    /// Feed raw VT bytes into the terminal state machine.
    pub fn vt_write(&mut self, bytes: &[u8]) {
        let _access = self.shared().change();
        if !bytes.is_empty() {
            // SAFETY: The terminal is locked and bytes is readable for exactly the supplied length.
            unsafe { ffi::ghostty_terminal_vt_write(self.raw(), bytes.as_ptr(), bytes.len()) };
        }
    }

    /// Resize grid and pixel cell dimensions (triggers reflow on primary screen).
    pub fn resize(&mut self, cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32) -> Result<(), VtError> {
        let _access = self.shared().change();
        limits::grid(cols, rows)?;
        if cell_width_px.checked_mul(u32::from(cols)).is_none() || cell_height_px.checked_mul(u32::from(rows)).is_none() {
            return Err(VtError::LimitExceeded);
        }
        // SAFETY: The exclusive lock covers reflow; grid and pixel extents fit their native types.
        check(unsafe { ffi::ghostty_terminal_resize(self.raw(), cols, rows, cell_width_px, cell_height_px) })
    }

    /// Full RIS reset preserving dimensions and default colors.
    pub fn reset(&mut self) {
        let _access = self.shared().change();
        // SAFETY: The live terminal is exclusively locked for reset.
        unsafe { ffi::ghostty_terminal_reset(self.raw()) };
    }

    /// Current `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        let _access = self.lock();
        self.size_locked()
    }

    pub(crate) fn size_locked(&self) -> (u16, u16) {
        let mut cols = 0u16;
        let mut rows = 0u16;
        // SAFETY: The terminal lock spans both getters; COLS and ROWS each write uint16_t.
        unsafe {
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_COLS, ptr::from_mut(&mut cols).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return (0, 0);
            }
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_ROWS, ptr::from_mut(&mut rows).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return (0, 0);
            }
        }
        (cols, rows)
    }

    pub fn active_screen(&self) -> ScreenKind {
        let _access = self.lock();
        let mut s = ffi::GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        // SAFETY: ACTIVE_SCREEN writes one GhosttyTerminalScreen to the local output under the lock.
        unsafe {
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, ptr::from_mut(&mut s).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return ScreenKind::Primary;
            }
        }
        ScreenKind::from_ffi(s)
    }

    pub fn scrollbar(&self) -> ScrollbarState {
        let _access = self.lock();
        let mut sb = ffi::GhosttyTerminalScrollbar::default();
        // SAFETY: SCROLLBAR writes the fixed-layout GhosttyTerminalScrollbar to the local output.
        unsafe {
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_SCROLLBAR, ptr::from_mut(&mut sb).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return ScrollbarState::default();
            }
        }
        ScrollbarState { total: sb.total, offset: sb.offset, len: sb.len }
    }

    pub fn is_viewport_at_bottom(&self) -> bool {
        let _access = self.lock();
        let mut at_bottom = true;
        // SAFETY: The locked terminal writes one bool for this data key.
        unsafe {
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE, ptr::from_mut(&mut at_bottom).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return true;
            }
        }
        at_bottom
    }

    pub fn mouse_tracking(&self) -> bool {
        let _access = self.lock();
        let mut on = false;
        // SAFETY: The locked terminal writes one bool for this data key.
        unsafe {
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, ptr::from_mut(&mut on).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return false;
            }
        }
        on
    }

    pub fn kitty_keyboard_flags(&self) -> u8 {
        let _access = self.lock();
        let mut flags = 0u8;
        // SAFETY: KITTY_KEYBOARD_FLAGS takes a uint8_t output.
        unsafe {
            if ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, ptr::from_mut(&mut flags).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS {
                return 0;
            }
        }
        flags
    }

    /// Check whether a DEC private mode (e.g. `2004` bracketed paste, `1004` focus events) is set.
    pub fn dec_mode(&self, mode: u16) -> bool {
        let _access = self.lock();
        if mode > 0x7fff {
            return false;
        }
        let mut val = ffi::GhosttyTerminalModeConfig { mode, value: false };
        // SAFETY: MODE takes an initialized GhosttyTerminalModeConfig for both input and output.
        let res = unsafe { ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_MODE, ptr::from_mut(&mut val).cast::<c_void>()) };
        res == ffi::GHOSTTY_SUCCESS && val.value
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.dec_mode(2004)
    }

    pub fn focus_reporting_mode(&self) -> bool {
        self.dec_mode(1004)
    }

    pub fn set_default_cursor_style(&mut self, style: CursorStyle, blink: bool) -> Result<(), VtError> {
        let _access = self.shared().change();
        let st = style.to_ffi();
        // SAFETY: The setter copies the correctly typed cursor option while the terminal is locked.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE, ptr::from_ref(&st).cast::<c_void>()) })?;
        // SAFETY: The setter copies the correctly typed cursor option while the terminal is locked.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK, ptr::from_ref(&blink).cast::<c_void>()) })
    }

    /// Apply default foreground, background, cursor, and 256-color palette.
    pub fn apply_theme(&mut self, theme: &TerminalTheme) -> Result<(), VtError> {
        let _access = self.shared().change();
        let fg = theme.foreground.to_ffi();
        let bg = theme.background.to_ffi();
        let pal: [ffi::GhosttyColorRgb; 256] = theme.palette_256().map(|color| color.to_ffi());
        // SAFETY: All RGB values and the full 256-entry palette live through the setters; NULL resets cursor color.
        unsafe {
            check(ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, ptr::from_ref(&fg).cast::<c_void>()))?;
            check(ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, ptr::from_ref(&bg).cast::<c_void>()))?;
            if let Some(cur) = theme.cursor {
                let c = cur.to_ffi();
                check(ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, ptr::from_ref(&c).cast::<c_void>()))?;
            } else {
                check(ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, ptr::null()))?;
            }
            check(ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, pal.as_ptr().cast::<c_void>()))?;
        }
        Ok(())
    }

    pub fn scroll_delta(&mut self, delta: isize) -> Result<(), VtError> {
        let _access = self.shared().change();
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA;
        behavior.value.delta = delta;
        // SAFETY: The viewport tag selects the initialized union field (or no field) in this by-value argument.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw(), behavior) })
    }

    pub fn scroll_to_top(&mut self) -> Result<(), VtError> {
        let _access = self.shared().change();
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP;
        // SAFETY: The viewport tag selects the initialized union field (or no field) in this by-value argument.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw(), behavior) })
    }

    pub fn scroll_to_bottom(&mut self) -> Result<(), VtError> {
        let _access = self.shared().change();
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM;
        // SAFETY: The viewport tag selects the initialized union field (or no field) in this by-value argument.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw(), behavior) })
    }

    pub fn scroll_to_row(&mut self, row: usize) -> Result<(), VtError> {
        let _access = self.shared().change();
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW;
        behavior.value.row = row;
        // SAFETY: The viewport tag selects the initialized union field (or no field) in this by-value argument.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw(), behavior) })
    }

    /// Jump forward/backward by `delta_prompts` OSC 133 semantic prompts.
    pub fn scroll_prompt_delta(&mut self, delta_prompts: isize) -> Result<(), VtError> {
        let _access = self.shared().change();
        let mut behavior = ffi::GhosttyTerminalScrollViewport::default();
        behavior.tag = ffi::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA_PROMPT;
        behavior.value.delta_prompt = delta_prompts;
        // SAFETY: The viewport tag selects the initialized union field (or no field) in this by-value argument.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw(), behavior) })
    }

    /// Look up the OSC 8 hyperlink URI at viewport `(col, row)`, if any.
    pub fn hyperlink_at_viewport(&self, col: u16, row: u32) -> Option<String> {
        let _access = self.lock();
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
        if unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
            return None;
        }
        let mut needed = 0usize;
        // SAFETY: A live grid reference and NULL with zero capacity request only the required length.
        let res = unsafe { ffi::ghostty_grid_ref_hyperlink_uri(&gref, ptr::null_mut(), 0, &mut needed) };
        if res == ffi::GHOSTTY_SUCCESS && needed == 0 {
            return None;
        }
        if res != ffi::GHOSTTY_OUT_OF_SPACE || needed == 0 || needed > 1 << 20 {
            return None;
        }
        let mut buf = vec![0u8; needed];
        let mut written = 0usize;
        // SAFETY: The grid reference is still valid and the URI buffer is writable for its full length.
        if unsafe { ffi::ghostty_grid_ref_hyperlink_uri(&gref, buf.as_mut_ptr(), buf.len(), &mut written) } != ffi::GHOSTTY_SUCCESS {
            return None;
        }
        if written > buf.len() {
            return None;
        }
        buf.truncate(written);
        String::from_utf8(buf).ok()
    }

    /// Clear any active terminal selection.
    pub fn clear_selection(&mut self) -> Result<(), VtError> {
        let _access = self.shared().change();
        // SAFETY: NULL is the documented request to clear the selection on the locked terminal.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null()) })
    }

    /// Set a selection between two viewport cell coordinates (inclusive).
    pub fn set_viewport_selection(&mut self, start: (u16, u32), end: (u16, u32), rectangle: bool) -> Result<(), VtError> {
        let _access = self.shared().change();
        let mut s_pt = ffi::GhosttyPoint::default();
        s_pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        s_pt.value.coordinate = ffi::GhosttyPointCoordinate { x: start.0, y: start.1 };
        let mut e_pt = ffi::GhosttyPoint::default();
        e_pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        e_pt.value.coordinate = ffi::GhosttyPointCoordinate { x: end.0, y: end.1 };
        let mut sel = ffi::GhosttySelection { rectangle, ..Default::default() };
        // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
        check(unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), s_pt, &mut sel.start) })?;
        // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
        check(unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), e_pt, &mut sel.end) })?;
        // SAFETY: The selection contains endpoints obtained from this same terminal without an intervening mutation.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })
    }

    /// Select the word under viewport `(col, row)`.
    pub fn select_word_at_viewport(&mut self, col: u16, row: u32) -> Result<bool, VtError> {
        let _access = self.shared().change();
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
        let result = unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), pt, &mut gref) };
        if matches!(result, ffi::GHOSTTY_INVALID_VALUE | ffi::GHOSTTY_NO_VALUE) {
            return Ok(false);
        }
        check(result)?;
        let opts = ffi::GhosttyTerminalSelectWordOptions { size: mem::size_of::<ffi::GhosttyTerminalSelectWordOptions>(), ref_: gref, boundary_codepoints: ptr::null(), boundary_codepoints_len: 0 };
        let mut sel = ffi::GhosttySelection::default();
        // SAFETY: The sized options borrow a current grid reference; NULL/zero requests default boundaries.
        let res = unsafe { ffi::ghostty_terminal_select_word(self.raw(), &opts, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        // SAFETY: The selection contains endpoints obtained from this same terminal without an intervening mutation.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Select the logical line under viewport `(col, row)`.
    pub fn select_line_at_viewport(&mut self, col: u16, row: u32) -> Result<bool, VtError> {
        let _access = self.shared().change();
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
        let result = unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), pt, &mut gref) };
        if matches!(result, ffi::GHOSTTY_INVALID_VALUE | ffi::GHOSTTY_NO_VALUE) {
            return Ok(false);
        }
        check(result)?;
        let opts = ffi::GhosttyTerminalSelectLineOptions {
            size: mem::size_of::<ffi::GhosttyTerminalSelectLineOptions>(),
            ref_: gref,
            whitespace: ptr::null(),
            whitespace_len: 0,
            semantic_prompt_boundary: true,
        };
        let mut sel = ffi::GhosttySelection::default();
        // SAFETY: The sized options borrow a current grid reference; NULL/zero requests default boundaries.
        let res = unsafe { ffi::ghostty_terminal_select_line(self.raw(), &opts, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        // SAFETY: The selection contains endpoints obtained from this same terminal without an intervening mutation.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Select the semantic OSC 133 command output block containing viewport `(col, row)`.
    pub fn select_output_at_viewport(&mut self, col: u16, row: u32) -> Result<bool, VtError> {
        let _access = self.shared().change();
        let mut pt = ffi::GhosttyPoint::default();
        pt.tag = ffi::GHOSTTY_POINT_TAG_VIEWPORT;
        pt.value.coordinate = ffi::GhosttyPointCoordinate { x: col, y: row };
        let mut gref = ffi::GhosttyGridRef::default();
        // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
        let result = unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), pt, &mut gref) };
        if matches!(result, ffi::GHOSTTY_INVALID_VALUE | ffi::GHOSTTY_NO_VALUE) {
            return Ok(false);
        }
        check(result)?;
        let mut sel = ffi::GhosttySelection::default();
        // SAFETY: The grid reference belongs to the locked terminal and sel is a writable output.
        let res = unsafe { ffi::ghostty_terminal_select_output(self.raw(), &gref, &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        // SAFETY: The selection contains endpoints obtained from this same terminal without an intervening mutation.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Select all content on the active screen + scrollback.
    pub fn select_all(&mut self) -> Result<bool, VtError> {
        let _access = self.shared().change();
        let mut sel = ffi::GhosttySelection::default();
        // SAFETY: The live locked terminal supplies both endpoints in sel on success.
        let res = unsafe { ffi::ghostty_terminal_select_all(self.raw(), &mut sel) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        // SAFETY: The selection contains endpoints obtained from this same terminal without an intervening mutation.
        check(unsafe { ffi::ghostty_terminal_set(self.raw(), ffi::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::from_ref(&sel).cast::<c_void>()) })?;
        Ok(true)
    }

    /// Whether anything is selected, asked without reading it out (a
    /// menu's Copy is enabled by it at every key that might be its
    /// shortcut: the text itself can be all of the scrollback).
    pub fn has_selection(&self) -> bool {
        let _access = self.lock();
        let mut sel = ffi::GhosttySelection::default();
        // SAFETY: The output is a GhosttySelection of this locked terminal, read and dropped here.
        let res = unsafe { ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_SELECTION, ptr::from_mut(&mut sel).cast::<c_void>()) };
        res != ffi::GHOSTTY_NO_VALUE && check(res).is_ok()
    }

    /// Extract the currently selected text (unwrapped and trimmed like Ghostty).
    pub fn selection_text(&self) -> Result<Option<String>, VtError> {
        let _access = self.lock();
        let mut sel = ffi::GhosttySelection::default();
        // SAFETY: The output is a GhosttySelection; format_inner consumes its grid references before unlocking.
        let res = unsafe { ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_SELECTION, ptr::from_mut(&mut sel).cast::<c_void>()) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check(res)?;
        Ok(Some(self.format_inner(FormatKind::Plain, true, true, Some(&sel))?))
    }

    /// Format the active screen & scrollback as plain text, VT, or HTML.
    pub fn format(&self, kind: FormatKind, unwrap: bool, trim: bool) -> Result<String, VtError> {
        let _access = self.lock();
        self.format_inner(kind, unwrap, trim, None)
    }

    fn format_inner(&self, kind: FormatKind, unwrap: bool, trim: bool, selection: Option<&ffi::GhosttySelection>) -> Result<String, VtError> {
        let mut fmt: ffi::GhosttyFormatter = ptr::null_mut();
        // SAFETY: The allocator and locked terminal outlive the scoped formatter and its guard.
        check(unsafe { ffi::ghostty_formatter_terminal_new(self.shared().budget.raw(), &mut fmt, self.raw()) })?;
        if fmt.is_null() {
            return Err(VtError::InvalidValue);
        }
        struct Guard(ffi::GhosttyFormatter);
        impl Drop for Guard {
            fn drop(&mut self) {
                // SAFETY: This guard has sole ownership of the formatter and runs while its terminal is still locked.
                unsafe { ffi::ghostty_formatter_free(self.0) };
            }
        }
        let guard = Guard(fmt);

        let emit = match kind {
            FormatKind::Plain => ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN,
            FormatKind::Vt => ffi::GHOSTTY_FORMATTER_FORMAT_VT,
            FormatKind::Html => ffi::GHOSTTY_FORMATTER_FORMAT_HTML,
        };
        // SAFETY: The typed formatter options are copied, and any selected endpoints remain valid under the lock.
        unsafe {
            check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_EMIT, ptr::from_ref(&emit).cast::<c_void>()))?;
            check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_UNWRAP, ptr::from_ref(&unwrap).cast::<c_void>()))?;
            check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_TRIM, ptr::from_ref(&trim).cast::<c_void>()))?;
            if let Some(sel) = selection {
                check(ffi::ghostty_formatter_set(guard.0, ffi::GHOSTTY_FORMATTER_OPT_SELECTION, (sel as *const ffi::GhosttySelection).cast::<c_void>()))?;
            }
        }

        let bytes = collect_output(|writer| {
            // SAFETY: guard owns the formatter, its terminal remains locked,
            // and collect_output keeps the writer valid through the call.
            unsafe { ffi::ghostty_formatter_format(guard.0, writer) }
        })?;
        match String::from_utf8(bytes) {
            Ok(text) => Ok(text),
            Err(err) => Ok(String::from_utf8_lossy(err.as_bytes()).into_owned()),
        }
    }

    /// Encode this terminal to a `GHOSTSNP` binary snapshot (useful for testing and local state).
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, VtError> {
        let _access = self.lock();
        collect_output(|writer| {
            // SAFETY: the terminal remains locked and the writer is live until encode returns.
            unsafe { ffi::ghostty_snapshot_encode(self.raw(), writer) }
        })
    }

    /// Compress idle scrollback pages (`incremental = true` does bounded work per call).
    pub fn compress_scrollback(&mut self, incremental: bool) -> bool {
        let _access = self.shared().change();
        let mode = if incremental { ffi::GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL } else { ffi::GHOSTTY_TERMINAL_COMPRESSION_MODE_FULL };
        let mut result = ffi::GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE;
        // SAFETY: The exclusive lock covers page compression and result is the documented enum output.
        let code = unsafe { ffi::ghostty_terminal_compress(self.raw(), mode, &mut result) };
        code == ffi::GHOSTTY_SUCCESS && result == ffi::GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING
    }

    /// Query the grid pages, image storage and native heap without decompressing history.
    /// RenderState, Rust-owned pixel copies, graphics textures and allocator overhead are separate.
    pub fn memory_usage(&self) -> TerminalMemoryUsage {
        let _access = self.lock();
        let mut m = ffi::GhosttyTerminalMemoryUsage { size: mem::size_of::<ffi::GhosttyTerminalMemoryUsage>(), ..Default::default() };
        // SAFETY: The output has the size and layout required by terminal.h; it contains only copied statistics.
        let result = unsafe { ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_MEMORY_USAGE, ptr::from_mut(&mut m).cast::<c_void>()) };
        if result != ffi::GHOSTTY_SUCCESS {
            return TerminalMemoryUsage { heap_bytes: self.shared().budget.used(), ..Default::default() };
        }
        TerminalMemoryUsage {
            compression_supported: m.compression_supported,
            heap_bytes: self.shared().budget.used(),
            virtual_bytes: m.primary_virtual_bytes.saturating_add(m.alternate_virtual_bytes),
            resident_bytes: m.primary_resident_bytes.saturating_add(m.alternate_resident_bytes),
            compressed_bytes: m.primary_compressed_bytes.saturating_add(m.alternate_compressed_bytes),
            image_bytes: m.primary_image_bytes.saturating_add(m.alternate_image_bytes),
            pages: m.primary_pages.saturating_add(m.alternate_pages),
            compressed_pages: m.primary_compressed_pages.saturating_add(m.alternate_compressed_pages),
        }
    }

    /// Fetch decoded RGBA8 pixels for a Kitty graphics image by `image_id`.
    pub fn kitty_image_rgba(&self, image_id: u32) -> Option<DecodedKittyImage> {
        let _access = self.lock();
        let mut storage: ffi::GhosttyKittyGraphics = ptr::null_mut();
        // SAFETY: KITTY_GRAPHICS writes a borrowed handle; the lock covers its full use.
        if unsafe { ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, ptr::from_mut(&mut storage).cast::<c_void>()) } != ffi::GHOSTTY_SUCCESS || storage.is_null() {
            return None;
        }
        // SAFETY: The storage belongs to this locked terminal and the borrowed image is checked before use.
        let img = unsafe { ffi::ghostty_kitty_graphics_image(storage, image_id) };
        if img.is_null() {
            return None;
        }
        let mut width = 0u32;
        let mut height = 0u32;
        let mut generation = 0u64;
        let mut format = ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA;
        let mut pixels = ffi::GhosttyString::default();
        // SAFETY: The image stays live under the terminal lock; each output has the type required by kitty_graphics.h.
        unsafe {
            if ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS, ptr::from_mut(&mut pixels).cast::<c_void>()) != ffi::GHOSTTY_SUCCESS
                || pixels.ptr.is_null()
                || pixels.len == 0
            {
                return None;
            }
            check(ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH, ptr::from_mut(&mut width).cast::<c_void>())).ok()?;
            check(ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT, ptr::from_mut(&mut height).cast::<c_void>())).ok()?;
            check(ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_GENERATION, ptr::from_mut(&mut generation).cast::<c_void>())).ok()?;
            check(ffi::ghostty_kitty_graphics_image_get(img, ffi::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT, ptr::from_mut(&mut format).cast::<c_void>())).ok()?;
        }
        let len = limits::rgba_len(width, height)?;
        let channels = match format {
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => 4,
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGB => 3,
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => 2,
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => 1,
            _ => return None,
        };
        if pixels.len != len / 4 * channels {
            return None;
        }
        // SAFETY: PIXELS returned a non-null pointer and the checked length matches the entire bounded image.
        let raw = unsafe { slice::from_raw_parts(pixels.ptr, pixels.len) };
        let rgba = match format {
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => raw.to_vec(),
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_RGB => {
                let mut out = Vec::with_capacity(len);
                for ch in raw.chunks_exact(3) {
                    out.extend_from_slice(&[ch[0], ch[1], ch[2], 255]);
                }
                out
            }
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => {
                let mut out = Vec::with_capacity(len);
                for ch in raw.chunks_exact(2) {
                    out.extend_from_slice(&[ch[0], ch[0], ch[0], ch[1]]);
                }
                out
            }
            ffi::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => {
                let mut out = Vec::with_capacity(len);
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
        let _access = self.shared().change();
        let mut storage: ffi::GhosttyKittyGraphics = ptr::null_mut();
        // SAFETY: KITTY_GRAPHICS writes a borrowed handle; the lock covers its full use.
        if unsafe { ffi::ghostty_terminal_get(self.raw(), ffi::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, ptr::from_mut(&mut storage).cast::<c_void>()) } != ffi::GHOSTTY_SUCCESS || storage.is_null() {
            return None;
        }
        let mut next_ms = 0u64;
        // SAFETY: The borrowed storage is exclusively locked; next_ms receives a uint64_t.
        let res = unsafe { ffi::ghostty_kitty_graphics_animation_tick(storage, now_ms, &mut next_ms) };
        (res == ffi::GHOSTTY_SUCCESS).then_some(next_ms)
    }

    /// Extract visible plain text of viewport row `row` (0-indexed from top of viewport).
    pub fn viewport_row_text(&self, row: u32) -> String {
        let _access = self.lock();
        let (cols, rows) = self.size_locked();
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
            // SAFETY: The VIEWPORT tag selects coordinate; the output is used only on success under this lock.
            if unsafe { ffi::ghostty_terminal_grid_ref(self.raw(), pt, &mut gref) } != ffi::GHOSTTY_SUCCESS {
                out.push(' ');
                continue;
            }
            let mut written = 0usize;
            // SAFETY: The grid reference remains current; the destination has the supplied number of uint32_t entries.
            let res = unsafe { ffi::ghostty_grid_ref_graphemes(&gref, cp_buf.as_mut_ptr(), cp_buf.len(), &mut written) };
            if res == ffi::GHOSTTY_SUCCESS && written > 0 && written <= cp_buf.len() {
                for &cp in &cp_buf[..written] {
                    if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                    }
                }
            } else if res == ffi::GHOSTTY_OUT_OF_SPACE && written > 0 && written <= 1 << 18 {
                let mut dyn_cp = vec![0u32; written];
                let mut w2 = 0usize;
                // SAFETY: The grid reference remains current; the destination has the supplied number of uint32_t entries.
                if unsafe { ffi::ghostty_grid_ref_graphemes(&gref, dyn_cp.as_mut_ptr(), dyn_cp.len(), &mut w2) } == ffi::GHOSTTY_SUCCESS && w2 <= dyn_cp.len() {
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
        out.truncate(out.trim_end().len());
        out
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

impl Drop for OwnedTerminal {
    fn drop(&mut self) {
        let _access = self.shared.lock();
        // SAFETY: this is the last owner, the allocator is live, and search operations are excluded.
        unsafe { ffi::ghostty_terminal_free(self.raw) };
    }
}

/// Native writers stream into a fallible, bounded Rust buffer. The two APIs
/// call the writer synchronously and never retain it.
fn collect_output(write: impl FnOnce(ffi::GhosttyWriter) -> ffi::GhosttyResult) -> Result<Vec<u8>, VtError> {
    struct Output {
        bytes: Vec<u8>,
        error: Option<VtError>,
    }
    unsafe extern "C" fn append(ctx: *mut c_void, data: *const u8, len: usize) -> bool {
        // SAFETY: ctx points to our Output throughout write. io.h supplies a
        // separate initialized byte slice valid for this invocation.
        let out = unsafe { &mut *ctx.cast::<Output>() };
        if len > limits::SNAPSHOT_BYTES - out.bytes.len() {
            out.error = Some(VtError::LimitExceeded);
            return false;
        }
        if out.bytes.try_reserve(len).is_err() {
            out.error = Some(VtError::OutOfMemory);
            return false;
        }
        // SAFETY: native output is non-null, len is bounded and the space is reserved.
        out.bytes.extend_from_slice(unsafe { slice::from_raw_parts(data, len) });
        true
    }
    let mut out = Output { bytes: Vec::new(), error: None };
    let result = write(ffi::GhosttyWriter { write: Some(append), userdata: ptr::from_mut(&mut out).cast() });
    if let Some(err) = out.error {
        return Err(err);
    }
    check(result)?;
    Ok(out.bytes)
}

#[cfg(test)]
mod memory_tests {
    use super::*;
    use crate::{RenderState, SnapshotDecoder, TerminalSearch};

    #[test]
    fn native_heap_released_after_both_drop_orders_and_snapshot_cancellation() {
        for terminal_first in [true, false] {
            let mut t = GhosttyTerminal::new(80, 24).unwrap();
            let accounting = t.shared().clone();
            assert!(accounting.budget.used() > 0);
            let mut s = TerminalSearch::new(&t).unwrap();
            t.vt_write(b"search me\r\n");
            s.set_needle(Some("search")).unwrap();
            s.run().unwrap();
            t.vt_write(b"\x1b_Ga=t,f=24,s=1,v=1,i=10;ESIz\x1b\\");
            assert_eq!(t.memory_usage().image_bytes, 3);
            let mut r = RenderState::new().unwrap();
            r.update(&t).unwrap();
            let memory = t.memory_usage();
            assert!(memory.pages > 0 && memory.resident_bytes > 0);
            assert!(memory.virtual_bytes >= memory.resident_bytes && memory.resident_bytes >= memory.compressed_bytes);
            let mut decoder = SnapshotDecoder::new(t.encode_snapshot().unwrap(), true, None).unwrap();
            let restored = decoder.ready().unwrap();
            let restored_accounting = restored.shared().clone();
            if terminal_first {
                drop(t);
                drop(s);
                drop(restored);
                drop(decoder);
            } else {
                drop(s);
                drop(t);
                drop(decoder);
                drop(restored);
            }
            assert_eq!(accounting.budget.used(), 0);
            assert_eq!(restored_accounting.budget.used(), 0);
        }
    }
}
