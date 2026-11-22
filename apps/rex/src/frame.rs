//! A complete frame per terminal. Multiple workspace windows consume the same
//! dirty rows at different times, so each row carries its own paint revision.
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use olo_kit::RenderImage;
use olo_rex_vt::{CursorInfo, DirtyState, GhosttyTerminal, ImagePlacement, RenderColors, RenderRow, RenderState, RowId, VtError};

pub struct FrameRow {
    pub revision: u64,
    seen: u64,
    pub row: RenderRow,
}

#[derive(Default)]
pub struct Frame {
    pub owner: u64,
    pub revision: u64,
    pub order: Vec<RowId>,
    pub rows: HashMap<RowId, FrameRow>,
    pub header: Option<(RenderColors, CursorInfo)>,
    pub placements: Vec<ImagePlacement>,
    generation: Option<u64>,
    // None while the worker prepares this generation.
    pub textures: HashMap<(u32, u64), Option<Arc<RenderImage>>>,
    retry_full: bool,
}

impl Frame {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self { owner: NEXT.fetch_add(1, Ordering::Relaxed), ..Default::default() }
    }

    /// A failed capture leaves every part of the last good frame available.
    pub fn capture(&mut self, state: &mut RenderState, term: &GhosttyTerminal) -> Result<Vec<(u32, u64)>, VtError> {
        let retry = std::mem::replace(&mut self.retry_full, true);
        state.begin_update(term)?;
        state.end_update()?;
        let colors = state.colors();
        let cursor = state.cursor();
        let dirty = state.dirty();
        let full = retry || self.header.as_ref().is_none_or(|(old, _)| old != &colors) || dirty == DirtyState::Full;
        let changed = full || dirty != DirtyState::Clean;
        let mut extracted = if changed { state.extract_rows(&colors.palette, !full)? } else { Vec::new() };
        if !full && extracted.iter().any(|r| r.cells.is_empty() && !self.rows.contains_key(&r.id)) {
            extracted = state.extract_rows(&colors.palette, false)?;
        }
        let generation = state.image_placements_generation();
        let placements = if self.generation != Some(generation) { Some(state.extract_image_placements()?) } else { None };
        if changed || placements.is_some() {
            state.clean()?;
        }
        self.retry_full = false;
        self.header = Some((colors, cursor));
        if changed {
            self.revision += 1;
            self.order.clear();
            for mut row in extracted {
                self.order.push(row.id);
                match self.rows.entry(row.id) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let cached = entry.get_mut();
                        cached.seen = self.revision;
                        // Scrolling can mark the entire viewport dirty. Row IDs
                        // survive a scroll, and an unchanged row keeps its glyph run.
                        if row.selection != cached.row.selection || (!row.cells.is_empty() && row.cells != cached.row.cells) {
                            cached.revision = self.revision;
                        }
                        if row.cells.is_empty() {
                            row.cells = std::mem::take(&mut cached.row.cells);
                        }
                        cached.row = row;
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(FrameRow { row, revision: self.revision, seen: self.revision });
                    }
                }
            }
            self.rows.retain(|_, r| r.seen == self.revision);
        }
        let mut missing = Vec::new();
        if let Some(placements) = placements {
            let mut previous = std::mem::take(&mut self.textures);
            for p in &placements {
                let key = (p.image_id, p.image_generation);
                self.textures.entry(key).or_insert_with(|| {
                    previous.remove(&key).unwrap_or_else(|| {
                        missing.push(key);
                        None
                    })
                });
            }
            self.placements = placements;
            self.generation = Some(generation);
        }
        Ok(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleting_the_last_placement_releases_its_generation() {
        let mut f = Frame::new();
        let mut t = GhosttyTerminal::new(20, 6).unwrap();
        t.resize(20, 6, 8, 16).unwrap();
        let mut rs = RenderState::new().unwrap();
        t.vt_write(b"\x1b_Ga=T,f=24,s=1,v=1,i=1;ESIz\x1b\\");
        let pending = f.capture(&mut rs, &t).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(f.capture(&mut rs, &t).unwrap().is_empty());
        t.vt_write(b"\x1b_Ga=d,d=A\x1b\\");
        assert!(f.capture(&mut rs, &t).unwrap().is_empty());
        assert!(f.placements.is_empty());
        assert!(f.textures.is_empty());
    }
    #[test]
    fn only_changed_rows_advance_and_a_replacement_has_a_new_owner() {
        let mut f = Frame::new();
        let mut t = GhosttyTerminal::new(50, 12).unwrap();
        let mut s = RenderState::new().unwrap();
        f.capture(&mut s, &t).unwrap();
        let revision = f.revision;
        f.capture(&mut s, &t).unwrap();
        assert_eq!(revision, f.revision);
        t.vt_write(b"text");
        f.capture(&mut s, &t).unwrap();
        assert!(f.revision > revision);
        assert_eq!(f.rows[&f.order[0]].row.cells[0].codepoint, u32::from('t'));
        assert_eq!(f.rows[&f.order[1]].revision, revision);
        f.retry_full = true;
        f.capture(&mut s, &t).unwrap();
        assert_eq!(f.rows[&f.order[1]].revision, revision);
        let header = f.header.clone();
        s.set_overscan(256, 256).unwrap();
        t.resize(512, 512, 1, 1).unwrap();
        assert!(f.capture(&mut s, &t).is_err());
        assert_eq!(f.header, header);
        assert_ne!(f.owner, Frame::new().owner);
    }
}
