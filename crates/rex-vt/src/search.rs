//! Incremental scrollback & viewport search over `GhosttySearch`.

use std::{ffi::c_void, ptr};

use crate::{
    error::{VtError, check},
    ffi,
    terminal::GhosttyTerminal,
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
}

unsafe impl Send for TerminalSearch {}

impl TerminalSearch {
    pub fn new(term: &GhosttyTerminal) -> Result<Self, VtError> {
        let mut raw: ffi::GhosttySearch = ptr::null_mut();
        check(unsafe { ffi::ghostty_search_new(ptr::null(), &mut raw, term.raw()) })?;
        Ok(Self { raw })
    }

    /// Set or clear the search needle (case-insensitive for ASCII letters).
    pub fn set_needle(&mut self, needle: Option<&str>) -> Result<(), VtError> {
        match needle {
            Some(s) if !s.is_empty() => {
                let gs = ffi::GhosttyString { ptr: s.as_ptr(), len: s.len() };
                check(unsafe { ffi::ghostty_search_set(self.raw, ffi::GHOSTTY_SEARCH_OPT_NEEDLE, (&gs as *const ffi::GhosttyString).cast::<c_void>()) })
            }
            _ => check(unsafe { ffi::ghostty_search_set(self.raw, ffi::GHOSTTY_SEARCH_OPT_NEEDLE, ptr::null()) }),
        }
    }

    /// Synchronize search with the live terminal (bounded work; call while
    /// holding the terminal lock briefly).
    pub fn feed(&mut self) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_search_feed(self.raw) })
    }

    /// Advance search on already-copied pages (safe without terminal access).
    pub fn tick(&mut self) -> Result<SearchStatus, VtError> {
        let mut st: ffi::GhosttySearchStatus = ffi::GHOSTTY_SEARCH_STATUS_COMPLETE;
        check(unsafe { ffi::ghostty_search_tick(self.raw, &mut st) })?;
        Ok(match st {
            ffi::GHOSTTY_SEARCH_STATUS_RUNNING => SearchStatus::Running,
            ffi::GHOSTTY_SEARCH_STATUS_FEED_REQUIRED => SearchStatus::FeedRequired,
            _ => SearchStatus::Complete,
        })
    }

    /// Run search to completion in one call.
    pub fn run(&mut self) -> Result<(), VtError> {
        check(unsafe { ffi::ghostty_search_run(self.raw) })
    }

    /// Select the next match (moving upward into scrollback) and scroll if needed.
    pub fn select_next(&mut self) -> Result<bool, VtError> {
        let res = unsafe { ffi::ghostty_search_select_next(self.raw) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        Ok(true)
    }

    /// Select the previous match (moving downward toward active prompt) and scroll if needed.
    pub fn select_prev(&mut self) -> Result<bool, VtError> {
        let res = unsafe { ffi::ghostty_search_select_prev(self.raw) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(res)?;
        Ok(true)
    }

    /// Total matches found so far on the active screen.
    pub fn total_matches(&self) -> usize {
        let mut count = 0usize;
        let _ = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_TOTAL_MATCHES, ptr::from_mut(&mut count).cast::<c_void>()) };
        count
    }

    /// Zero-based index of the currently selected match (`None` if none).
    pub fn selected_index(&self) -> Option<usize> {
        let mut idx = 0usize;
        let res = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_SELECTED_INDEX, ptr::from_mut(&mut idx).cast::<c_void>()) };
        (res == ffi::GHOSTTY_SUCCESS).then_some(idx)
    }

    /// Current viewport matches converted to viewport cell coordinates for rendering.
    pub fn viewport_matches(&self, term: &GhosttyTerminal) -> Vec<ViewportMatch> {
        let mut selected_sel = ffi::GhosttySelection::default();
        let has_selected = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_SELECTED_MATCH, ptr::from_mut(&mut selected_sel).cast::<c_void>()) } == ffi::GHOSTTY_SUCCESS;

        let mut query = ffi::GhosttySelectionBuffer { ptr: ptr::null_mut(), cap: 0, len: 0 };
        let res = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, ptr::from_mut(&mut query).cast::<c_void>()) };
        if res == ffi::GHOSTTY_SUCCESS && query.len == 0 {
            return Vec::new();
        }
        if res != ffi::GHOSTTY_OUT_OF_SPACE && res != ffi::GHOSTTY_SUCCESS {
            return Vec::new();
        }
        let mut raw_matches = vec![ffi::GhosttySelection::default(); query.len];
        let mut buf = ffi::GhosttySelectionBuffer { ptr: raw_matches.as_mut_ptr(), cap: raw_matches.len(), len: 0 };
        let res2 = unsafe { ffi::ghostty_search_get(self.raw, ffi::GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, ptr::from_mut(&mut buf).cast::<c_void>()) };
        if res2 != ffi::GHOSTTY_SUCCESS {
            return Vec::new();
        }
        raw_matches.truncate(buf.len);

        let (_, rows) = term.size();
        let mut out = Vec::with_capacity(raw_matches.len());
        for sel in raw_matches {
            let mut s_coord = ffi::GhosttyPointCoordinate::default();
            let mut e_coord = ffi::GhosttyPointCoordinate::default();
            let s_ok = unsafe { ffi::ghostty_terminal_point_from_grid_ref(term.raw(), &sel.start, ffi::GHOSTTY_POINT_TAG_VIEWPORT, &mut s_coord) } == ffi::GHOSTTY_SUCCESS;
            let e_ok = unsafe { ffi::ghostty_terminal_point_from_grid_ref(term.raw(), &sel.end, ffi::GHOSTTY_POINT_TAG_VIEWPORT, &mut e_coord) } == ffi::GHOSTTY_SUCCESS;
            if !s_ok || !e_ok {
                continue;
            }
            if s_coord.y >= rows as u32 && e_coord.y >= rows as u32 {
                continue;
            }
            let mut eq = false;
            let is_sel = has_selected && unsafe { ffi::ghostty_terminal_selection_equal(term.raw(), &sel, &selected_sel, &mut eq) } == ffi::GHOSTTY_SUCCESS && eq;
            out.push(ViewportMatch { start_col: s_coord.x, start_row: s_coord.y, end_col: e_coord.x, end_row: e_coord.y, selected: is_sel });
        }
        out
    }
}

impl Drop for TerminalSearch {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { ffi::ghostty_search_free(self.raw) };
        }
    }
}
