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
    /// (The tests'.) Every row the engine marks dirty has its cells
    /// taken from it, as before rows known by their packed cells were
    /// passed over (rows it says are clean are left alone, then as
    /// now): what a frame that passes them over is held to.
    #[cfg(test)]
    every_row: bool,
    /// (The tests'.) How many rows the engine marked dirty were passed
    /// over, known by their packed cells.
    #[cfg(test)]
    passed_over: usize,
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
        let recoloured = self.header.as_ref().is_none_or(|(old, _)| old != &colors);
        let full = retry || recoloured || dirty == DirtyState::Full;
        let changed = full || dirty != DirtyState::Clean;
        // A scroll marks every row dirty, and all but the new line are
        // as they were: a row kept from the last frame whose packed
        // cells are the same isn't taken again (its glyph run is kept
        // too). Not after a capture that failed, nor when the colours
        // changed: a cell's colours are resolved through them.
        let rows = &self.rows;
        let known = (!retry && !recoloured && !self.every_row()).then_some(|id: &RowId| rows.get(id).map(|r| (&r.row.raw, r.row.cells.as_slice())).filter(|(raw, _)| !raw.cells.is_empty()));
        let mut extracted = if changed { state.extract_rows_known(&colors.palette, !full, known)? } else { Vec::new() };
        if extracted.iter().any(|r| r.cells.is_empty() && !self.rows.contains_key(&r.id)) {
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
                            #[cfg(test)]
                            {
                                self.passed_over += usize::from(row.dirty);
                            }
                            row.cells = std::mem::take(&mut cached.row.cells);
                            row.raw = std::mem::take(&mut cached.row.raw);
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

    fn every_row(&self) -> bool {
        #[cfg(test)]
        return self.every_row;
        #[cfg(not(test))]
        false
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

    /// A cell that is only a background carried in its word, and has a
    /// style besides (a snapshot can hold one; Astra's second review
    /// made this one, an 80 × 24 screen with such a cell before a B of
    /// the same style id), can't speak for the style: a line taken out
    /// of a two-column part of the screen moves another style under the
    /// same id, the first cell shows its own blue as before, and the B
    /// behind it has another ground. The row is taken.
    #[test]
    fn a_background_in_a_cells_word_doesnt_speak_for_its_style() {
        let snapshot = include_bytes!("testdata/an-inline-background-with-a-style.snap");
        let restored = || olo_rex_vt::try_decode_snapshot_prefix(snapshot, true, None).unwrap().unwrap().terminal;
        let (mut ta, mut tb) = (restored(), restored());
        let (mut sa, mut sb) = (RenderState::new().unwrap(), RenderState::new().unwrap());
        let (mut fa, mut fb) = (Frame::new(), Frame { every_row: true, ..Frame::new() });
        fa.capture(&mut sa, &ta).unwrap();
        fb.capture(&mut sb, &tb).unwrap();
        let y = fb.order.iter().position(|id| fb.rows[id].row.cells[1].codepoint == u32::from('B')).unwrap();
        let ground = |f: &Frame| {
            let row = &f.rows[&f.order[y]].row;
            (row.cells[0].bg, row.cells[1].bg)
        };
        let before = ground(&fb);
        assert_eq!(ground(&fa), before);
        // Left and right margins on, the first two columns; rows y and
        // the next the part that scrolls; a line taken out at its top.
        let bytes = format!("\x1b[?69h\x1b[1;2s\x1b[{};{}r\x1b[{};1H\x1b[M", y + 1, y + 2, y + 1);
        ta.vt_write(bytes.as_bytes());
        tb.vt_write(bytes.as_bytes());
        fa.capture(&mut sa, &ta).unwrap();
        fb.capture(&mut sb, &tb).unwrap();
        let after = ground(&fb);
        // (It is the case meant: the first cell's ground as it was, the B's another.)
        assert_eq!(after.0, before.0);
        assert_ne!(after.1, before.1);
        assert_eq!(ground(&fa), after);
        let cells = |f: &Frame| f.order.iter().map(|id| f.rows[id].row.cells.clone()).collect::<Vec<_>>();
        assert_eq!(cells(&fa), cells(&fb));
        assert_eq!(fa.rows[&fa.order[y]].revision, fb.rows[&fb.order[y]].revision);
    }

    /// A frame that passes over the rows it knows by their packed cells
    /// holds, after every capture, what a frame that takes the cells of
    /// every row the engine marks dirty holds (as it was done before),
    /// and counts the same rows changed:
    /// two terminals written the same bytes, whatever they are (lines
    /// that scroll the screen, colours, the cursor moved and things
    /// erased, lines put in and taken out, a part of the screen
    /// scrolled, wide characters, marks joined to a letter one write at
    /// a time, the other screen, another size, other colours, links,
    /// the history scrolled and compressed, a selection).
    #[test]
    fn rows_passed_over_are_the_rows_taken_again() {
        // (A small generator of its own: the same bytes every run.)
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut next = move |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 33) as u32 % n
        };
        let (mut ta, mut tb) = (GhosttyTerminal::new(40, 12).unwrap(), GhosttyTerminal::new(40, 12).unwrap());
        let (mut sa, mut sb) = (RenderState::new().unwrap(), RenderState::new().unwrap());
        let (mut fa, mut fb) = (Frame::new(), Frame { every_row: true, ..Frame::new() });
        // First, by name, what a row's packed cells don't say.
        {
            let mut look = |bytes: &[u8]| {
                ta.vt_write(bytes);
                tb.vt_write(bytes);
                fa.capture(&mut sa, &ta).unwrap();
                fb.capture(&mut sb, &tb).unwrap();
                let cells = |f: &Frame| f.order.iter().map(|id| f.rows[id].row.cells.clone()).collect::<Vec<_>>();
                assert_eq!(cells(&fa), cells(&fb), "after {:?}", String::from_utf8_lossy(bytes));
                cells(&fb)
            };
            // A mark joined to a letter that has one already: the cell's
            // word is the same, what it shows isn't. (The engine doesn't
            // say the row changed when a mark arrives in a write of its
            // own, so neither frame shows it yet: TODO, "A mark joined
            // later".) Then the screen scrolls a line, the engine makes
            // every row again, and the mark is there: a row with a
            // grapheme in it is taken, whatever its words.
            let rows = look("\x1b[6;1He\u{301}".as_bytes());
            assert_eq!(rows[5][0].grapheme, "e\u{301}");
            look("\u{302}".as_bytes());
            let rows = look(b"\x1b[12;1H\r\nfoot");
            assert_eq!(rows[4][0].grapheme, "e\u{301}\u{302}");
            look(b"\x1b[2J\x1b[H");
            // A red X rubbed out and a green one written where it was: the
            // same word (the style's id is free once no cell has it, and
            // the next style takes it), another colour. With the screen
            // as it was, with a line printed after, and bold for italic.
            let rows = look(b"\x1b[31mX\x1b[0m");
            let red = rows[0][0].fg;
            let rows = look(b"\r\x1b[2K\x1b[32mX\x1b[0m");
            assert!(red.is_some() && rows[0][0].fg.is_some());
            assert_ne!(rows[0][0].fg, red);
            look(b"\x1b[2J\x1b[2;1H\x1b[31mX\x1b[0m");
            let rows = look(b"\r\x1b[2K\x1b[32mX\x1b[0m\x1b[12;1H\r\n");
            assert_ne!(rows[0][0].fg, red);
            // The second style of a row, the first as it was: a red A
            // left alone, a green B rubbed out and a blue one written.
            look(b"\x1b[2J\x1b[H\x1b[31mA\x1b[32mB\x1b[0m");
            let rows = look(b"\r\x1b[C\x1b[K\x1b[34mB\x1b[0m");
            let (a, b) = (rows[0][0].fg, rows[0][1].fg);
            assert_eq!(a, red);
            assert!(b.is_some() && b != red);
            let rows = look(b"\r\x1b[C\x1b[K\x1b[35mB\x1b[0m");
            assert!(rows[0][1].fg.is_some() && rows[0][1].fg != b);
            look(b"\x1b[2J\x1b[H\x1b[1mX\x1b[0m");
            let rows = look(b"\r\x1b[2K\x1b[3mX\x1b[0m");
            assert!(rows[0][0].style.italic && !rows[0][0].style.bold);
            look(b"\x1b[2J\x1b[H");
            // A colour of the palette changed under text that has it,
            // with the screen as it was and with a line printed after.
            let rows = look(b"\r\n\x1b[31mred\x1b[0m");
            let red = rows[1][0].fg;
            let rows = look(b"\x1b]4;1;rgb:12/34/56\x07");
            assert_ne!(rows[1][0].fg, red);
            assert_eq!(rows[1][0].fg, Some(olo_rex_vt::Rgb { r: 0x12, g: 0x34, b: 0x56 }));
            let rows = look(b"\x1b]4;1;rgb:65/43/21\x07\r\nmore");
            assert_eq!(rows[1][0].fg, Some(olo_rex_vt::Rgb { r: 0x65, g: 0x43, b: 0x21 }));
            // The ground's colour, which a cell with none of its own takes
            // (said of the frame, not of the cell: the cells are the same).
            look(b"\x1b]11;#102030\x07");
            // A part of the screen scrolled: rows where other rows were.
            look(b"\x1b[2J\x1b[H1\r\n2\r\n3\r\n4\r\n5\r\n6");
            let rows = look(b"\x1b[2;4r\x1b[4;1H\x1bD");
            assert_eq!(rows.iter().take(6).map(|r| char::from_u32(r[0].codepoint).unwrap_or(' ')).collect::<String>(), "134\0 56".replace("\0 ", "\0"));
            look(b"\x1b[r\x1b[2J\x1b[H");
        }
        let (mut captures, mut rows_seen) = (0usize, 0usize);
        for step in 0..6000u32 {
            let mut bytes: Vec<u8> = Vec::new();
            let mut both = |f: &mut dyn FnMut(&mut GhosttyTerminal)| {
                f(&mut ta);
                f(&mut tb);
            };
            match next(31) {
                // A line of text, more often than anything: the screen scrolls.
                0..=9 => {
                    match next(6) {
                        0 => bytes.extend(format!("\x1b[3{}m", next(8)).bytes()),
                        1 => bytes.extend(format!("\x1b[38;5;{}m", next(256)).bytes()),
                        2 => bytes.extend(format!("\x1b[48;2;{};{};{}m", next(256), next(256), next(256)).bytes()),
                        3 => bytes.extend(b"\x1b[1;4m"),
                        4 => bytes.extend(b"\x1b[0m"),
                        _ => {}
                    }
                    for _ in 0..next(50) {
                        bytes.push(b' ' + next(95) as u8);
                    }
                    bytes.extend(b"\r\n");
                }
                // The cursor somewhere, and a word there.
                10 => bytes.extend(format!("\x1b[{};{}Hword", 1 + next(12), 1 + next(40)).bytes()),
                // Erased: to the end of the line, the line, below, the screen.
                11 => bytes.extend([&b"\x1b[K"[..], b"\x1b[2K", b"\x1b[J", b"\x1b[2J"][next(4) as usize]),
                // A part of the screen scrolls, up or down; or all of it again.
                12 => match next(4) {
                    0 => bytes.extend(format!("\x1b[{};{}r", 1 + next(5), 7 + next(6)).bytes()),
                    1 => bytes.extend(b"\x1bD"),
                    2 => bytes.extend(b"\x1bM"),
                    _ => bytes.extend(b"\x1b[r"),
                },
                // Lines put in and taken out; characters too.
                13 => bytes.extend(format!("\x1b[{}{}", 1 + next(3), ['L', 'M', '@', 'P', 'X'][next(5) as usize]).bytes()),
                // Wide characters.
                14 => bytes.extend("\u{65e5}\u{672c}\u{8a9e}".bytes()),
                // A letter, and marks joined to it, each in a write of its own.
                15 => bytes.extend(b"e"),
                16 => bytes.extend(["\u{301}", "\u{302}", "\u{308}"][next(3) as usize].bytes()),
                // People joined into one, piece by piece.
                17 => bytes.extend(["\u{1f468}", "\u{200d}", "\u{1f469}", "\u{200d}\u{1f467}"][next(4) as usize].bytes()),
                // The other screen, and back.
                18 => bytes.extend(if next(2) == 0 { &b"\x1b[?1049h"[..] } else { b"\x1b[?1049l" }),
                // Another size.
                19 => {
                    let (cols, rows) = (20 + next(40) as u16, 6 + next(14) as u16);
                    both(&mut |t| {
                        let _ = t.resize(cols, rows, 8, 16);
                    });
                }
                // Other colours: one of the palette's, the ground's, all turned about.
                20 => match next(4) {
                    0 => bytes.extend(format!("\x1b]4;{};rgb:{:02x}/40/80\x07", next(16), next(256)).bytes()),
                    1 => bytes.extend(format!("\x1b]11;#{:02x}2030\x07", next(256)).bytes()),
                    2 => bytes.extend(b"\x1b[?5h"),
                    _ => bytes.extend(b"\x1b[?5l"),
                },
                // A link.
                21 => bytes.extend(b"\x1b]8;;http://example.test/\x07link\x1b]8;;\x07"),
                // A tab, a step back, the line's start.
                22 => bytes.extend(b"\tx\x08y\r"),
                // The history gone, or everything.
                23 => bytes.extend(if next(8) == 0 { &b"\x1bc"[..] } else { b"\x1b[3J" }),
                // The history looked at, and left.
                24 => {
                    let by = next(9) as isize - 6;
                    both(&mut |t| {
                        let _ = t.scroll_delta(by);
                    });
                }
                25 => both(&mut |t| {
                    let _ = t.scroll_to_bottom();
                }),
                26 => both(&mut |t| {
                    t.compress_scrollback(true);
                }),
                // Something selected, or nothing.
                27 => {
                    let (from, to) = ((next(10) as u16, next(5)), (10 + next(10) as u16, next(5)));
                    both(&mut |t| {
                        // (Past a small screen's rows it is refused: either way, the same for both.)
                        let _ = t.set_viewport_selection(from, to, false);
                    });
                }
                28 => both(&mut |t| {
                    let _ = t.clear_selection();
                }),
                // A line rubbed out and written again, the same letters in
                // another style.
                29 => bytes.extend(format!("\r\x1b[2K\x1b[{}mXY\x1b[{}mZ\x1b[0m", [31, 32, 1, 3, 4, 7][next(6) as usize], [33, 34, 0, 9][next(4) as usize]).bytes()),
                // Nothing: a capture with nothing new.
                _ => {}
            }
            ta.vt_write(&bytes);
            tb.vt_write(&bytes);
            // (Now and then several writes before the next look.)
            if next(4) == 0 {
                continue;
            }
            let (a, b) = (fa.capture(&mut sa, &ta), fb.capture(&mut sb, &tb));
            assert_eq!(a.is_ok(), b.is_ok(), "step {step}");
            if a.is_err() {
                continue;
            }
            captures += 1;
            assert_eq!(fa.order.len(), fb.order.len(), "step {step}");
            assert_eq!(fa.revision, fb.revision, "step {step}");
            for (ia, ib) in fa.order.iter().zip(&fb.order) {
                let (ra, rb) = (&fa.rows[ia], &fb.rows[ib]);
                let at = ra.row.viewport_y;
                assert_eq!(ra.row.cells, rb.row.cells, "step {step}, row {at}");
                assert_eq!((at, ra.row.wrap, ra.row.wrap_continuation, ra.row.selection), (rb.row.viewport_y, rb.row.wrap, rb.row.wrap_continuation, rb.row.selection), "step {step}, row {at}");
                // (A row changed for one is changed for the other: a glyph run kept or made again alike.)
                assert_eq!(ra.revision, rb.revision, "step {step}, row {at}");
                rows_seen += 1;
            }
        }
        // (And the rows passed over are many: what is held to the old way is what runs.)
        assert_eq!(fb.passed_over, 0);
        assert!(captures > 3000 && fa.passed_over > 5000, "{captures} captures, {rows_seen} rows, {} passed over", fa.passed_over);
    }
}
