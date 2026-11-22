//! Incremental scrollback & viewport search over `GhosttySearch`.

use std::{cell::Cell, ffi::c_void, ptr, sync::Arc};

use crate::{
    error::{VtError, check},
    ffi,
    terminal::{GhosttyTerminal, Shared},
};

/// Search progress status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStatus {
    Running,
    FeedRequired,
    Complete,
}

/// Viewport highlight match rectangle (in viewport cell coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportMatch {
    pub start_col: u16,
    pub start_row: u32,
    pub end_col: u16,
    pub end_row: u32,
    pub selected: bool,
}

/// Incremental terminal search bound to a [`GhosttyTerminal`].
pub struct TerminalSearch {
    raw: ffi::GhosttySearch,
    shared: Arc<Shared>,
    has_needle: bool,
    fed_revision: Cell<Option<u64>>,
}

// SAFETY: search is exclusively owned; all operations that can touch its
// terminal (including native detachment in either destructor) share one lock.
unsafe impl Send for TerminalSearch {}

impl TerminalSearch {
    pub fn new(term: &GhosttyTerminal) -> Result<Self, VtError> {
        let _access = term.lock();
        let mut raw: ffi::GhosttySearch = ptr::null_mut();
        // SAFETY: Creation registers this search under the terminal lock. Its allocator outlives the search.
        check(unsafe { ffi::ghostty_search_new(term.shared().budget.raw(), &mut raw, term.raw()) })?;
        if raw.is_null() {
            return Err(VtError::InvalidValue);
        }
        Ok(Self { raw, shared: term.shared().clone(), has_needle: false, fed_revision: Cell::new(None) })
    }

    /// Set or clear the search needle (case-insensitive for ASCII letters).
    pub fn set_needle(&mut self, needle: Option<&str>) -> Result<(), VtError> {
        let _access = self.shared.lock();
        match needle {
            Some(s) if !s.is_empty() => {
                if s.len() > 1 << 20 {
                    return Err(VtError::LimitExceeded);
                }
                let gs = ffi::GhosttyString { ptr: s.as_ptr(), len: s.len() };
                // SAFETY: NEEDLE copies this counted byte slice, and changes tracked state under the terminal lock.
                check(unsafe { ffi::ghostty_search_set(self.raw, ffi::GHOSTTY_SEARCH_OPT_NEEDLE, (&gs as *const ffi::GhosttyString).cast::<c_void>()) })
            }
            // SAFETY: NULL clears the needle under the same lock as terminal destruction.
            _ => check(unsafe { ffi::ghostty_search_set(self.raw, ffi::GHOSTTY_SEARCH_OPT_NEEDLE, ptr::null()) }),
        }?;
        self.has_needle = needle.is_some_and(|s| !s.is_empty());
        self.fed_revision.set(None);
        Ok(())
    }

    /// Synchronize search with the live terminal (bounded work; call while
    /// holding the terminal lock briefly).
    pub fn feed(&mut self) -> Result<(), VtError> {
        let _access = self.shared.lock();
        // SAFETY: The bound terminal and this search are serialized, including native detachment on close.
        check(unsafe { ffi::ghostty_search_feed(self.raw) })?;
        self.fed_revision.set(Some(*_access));
        Ok(())
    }

    /// Advance search on already-copied pages (safe without terminal access).
    pub fn tick(&mut self) -> Result<SearchStatus, VtError> {
        let _access = self.shared.lock();
        let mut st: ffi::GhosttySearchStatus = ffi::GHOSTTY_SEARCH_STATUS_COMPLETE;
        // SAFETY: The live search writes one GhosttySearchStatus. Detachment is serialized.
        check(unsafe { ffi::ghostty_search_tick(self.raw, &mut st) })?;
        Ok(match st {
            ffi::GHOSTTY_SEARCH_STATUS_RUNNING => SearchStatus::Running,
            ffi::GHOSTTY_SEARCH_STATUS_FEED_REQUIRED => SearchStatus::FeedRequired,
            _ => SearchStatus::Complete,
        })
    }

    /// Run search to completion in one call.
    pub fn run(&mut self) -> Result<(), VtError> {
        let _access = self.shared.lock();
        // SAFETY: The bound terminal and this search are serialized, including native detachment on close.
        check(unsafe { ffi::ghostty_search_run(self.raw) })?;
        self.fed_revision.set(Some(*_access));
        Ok(())
    }

    /// Select the next match (moving upward into scrollback) and scroll if needed.
    pub fn select_next(&mut self) -> Result<bool, VtError> {
        let _access = self.shared.change();
        // SAFETY: Moving the selection may scroll the terminal; its mutation lock covers this call.
        let res = unsafe { ffi::ghostty_search_select_next(self.raw) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        Ok(true)
    }

    /// Select the previous match (moving downward toward active prompt) and scroll if needed.
    pub fn select_prev(&mut self) -> Result<bool, VtError> {
        let _access = self.shared.change();
        // SAFETY: Moving the selection may scroll the terminal; its mutation lock covers this call.
        let res = unsafe { ffi::ghostty_search_select_prev(self.raw) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        Ok(true)
    }

    /// Total matches found so far on the active screen.
    pub fn total_matches(&self) -> usize {
        let _access = self.shared.lock();
        let mut count = 0usize;
        // SAFETY: The live search writes a size_t; it keeps no pointer to the output.
        let result = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_TOTAL_MATCHES, ptr::from_mut(&mut count).cast::<c_void>()) };
        if result == ffi::GHOSTTY_SUCCESS { count } else { 0 }
    }

    /// Zero-based index of the currently selected match (`None` if none).
    pub fn selected_index(&self) -> Option<usize> {
        let _access = self.shared.lock();
        let mut idx = 0usize;
        // SAFETY: The live search writes a size_t; it keeps no pointer to the output.
        let res = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_SELECTED_INDEX, ptr::from_mut(&mut idx).cast::<c_void>()) };
        (res == ffi::GHOSTTY_SUCCESS).then_some(idx)
    }

    /// Current viewport matches converted to viewport cell coordinates for rendering.
    pub fn viewport_matches(&self, term: &GhosttyTerminal) -> Vec<ViewportMatch> {
        let _access = self.shared.lock();
        if !self.has_needle || !Arc::ptr_eq(&self.shared, term.shared()) {
            return Vec::new();
        }
        // Viewport highlights contain untracked page references. Refresh them
        // once after a mutation, under the same lock as coordinate conversion.
        if self.fed_revision.get() != Some(*_access) {
            // SAFETY: the search is attached to this exact live, locked terminal.
            if unsafe { ffi::ghostty_search_feed(self.raw) } != ffi::GHOSTTY_SUCCESS {
                return Vec::new();
            }
            self.fed_revision.set(Some(*_access));
        }
        let mut selected_sel = ffi::GhosttySelection::default();
        // SAFETY: The selection output is correctly typed; the lock keeps its untracked references valid.
        let has_selected = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_SELECTED_MATCH, ptr::from_mut(&mut selected_sel).cast::<c_void>()) } == ffi::GHOSTTY_SUCCESS;

        let mut query = ffi::GhosttySelectionBuffer { ptr: ptr::null_mut(), cap: 0, len: 0 };
        // SAFETY: The empty selection buffer queries a count and does not write elements.
        let res = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, ptr::from_mut(&mut query).cast::<c_void>()) };
        if res == ffi::GHOSTTY_SUCCESS && query.len == 0 {
            return Vec::new();
        }
        if res != ffi::GHOSTTY_OUT_OF_SPACE && res != ffi::GHOSTTY_SUCCESS {
            return Vec::new();
        }
        if query.len > crate::limits::MAX_CELLS * 2 {
            return Vec::new();
        }
        let mut raw_matches = vec![ffi::GhosttySelection::default(); query.len];
        let mut buf = ffi::GhosttySelectionBuffer { ptr: raw_matches.as_mut_ptr(), cap: raw_matches.len(), len: 0 };
        // SAFETY: The buffer is backed by exactly cap writable GhosttySelection values.
        let res2 = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, ptr::from_mut(&mut buf).cast::<c_void>()) };
        if res2 != ffi::GHOSTTY_SUCCESS || buf.len > raw_matches.len() {
            return Vec::new();
        }
        raw_matches.truncate(buf.len);

        let (_, rows) = term.size_locked();
        let mut out = Vec::with_capacity(raw_matches.len());
        for sel in raw_matches {
            let mut s_coord = ffi::GhosttyPointCoordinate::default();
            let mut e_coord = ffi::GhosttyPointCoordinate::default();
            // SAFETY: The endpoints are fresh grid references from this terminal, which remains locked.
            let s_ok = unsafe { ffi::ghostty_terminal_point_from_grid_ref(term.raw(), &sel.start, ffi::GHOSTTY_POINT_TAG_VIEWPORT, &mut s_coord) } == ffi::GHOSTTY_SUCCESS;
            // SAFETY: The endpoints are fresh grid references from this terminal, which remains locked.
            let e_ok = unsafe { ffi::ghostty_terminal_point_from_grid_ref(term.raw(), &sel.end, ffi::GHOSTTY_POINT_TAG_VIEWPORT, &mut e_coord) } == ffi::GHOSTTY_SUCCESS;
            if !s_ok || !e_ok {
                continue;
            }
            if s_coord.y >= rows as u32 && e_coord.y >= rows as u32 {
                continue;
            }
            let mut eq = false;
            // SAFETY: Both selections refer to the same locked terminal; eq is a writable C bool.
            let is_sel = has_selected && unsafe { ffi::ghostty_terminal_selection_equal(term.raw(), &sel, &selected_sel, &mut eq) } == ffi::GHOSTTY_SUCCESS && eq;
            out.push(ViewportMatch { start_col: s_coord.x, start_row: s_coord.y, end_col: e_coord.x, end_row: e_coord.y, selected: is_sel });
        }
        out
    }
}

impl Drop for TerminalSearch {
    fn drop(&mut self) {
        let _access = self.shared.lock();
        if !self.raw.is_null() {
            // SAFETY: This is the unique search handle. Its allocator is live and native detachment is serialized.
            unsafe { ffi::ghostty_search_free(self.raw) };
        }
    }
}
