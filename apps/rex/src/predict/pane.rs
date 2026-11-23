//! Between a pane and its predictions: what of a frame the model is
//! shown ([`screen_of`]), and the caret's row as it is painted with
//! what is drawn ahead ([`predicted_row`]).

use olo_rex_vt::{CellStyle, CellWide, CursorStyle, RenderCell, RenderColors, RenderRow, Rgb};

use super::{Cell, Like, OTHER, Overlay, Screen, WIDE_TAIL};
use crate::frame::Frame;

/// A cell as the model sees it.
fn cell_of(cell: &RenderCell, colors: &RenderColors) -> Cell {
    let ch = match cell.wide {
        CellWide::SpacerTail => WIDE_TAIL,
        _ if !cell.grapheme.is_empty() => OTHER,
        _ => char::from_u32(cell.codepoint).filter(|c| *c != '\0').unwrap_or(' '),
    };
    Cell { ch, dim: ch != ' ' && dim(cell, colors), ground: cell.bg.is_some() || cell.style.inverse }
}

/// Whether a cell is drawn dimmer than text: faint, in the palette's
/// grey, or in a colour much nearer the ground than text is (a
/// suggestion's and a placeholder's greys, which themes choose freely).
/// Never by its hue alone: a shell that colours what is typed colours
/// words differently, and they are the line's own.
fn dim(cell: &RenderCell, colors: &RenderColors) -> bool {
    if cell.style.inverse {
        return false;
    }
    if cell.style.faint || cell.style.fg_palette_index == Some(8) {
        return true;
    }
    let Some(fg) = cell.fg else { return false };
    let light = |c: Rgb| 0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b);
    let ground = light(cell.bg.unwrap_or(colors.background));
    (light(fg) - ground).abs() < 0.55 * (light(colors.foreground) - ground).abs()
}

/// What of a frame predictions are made on and checked against: the
/// caret's row. The terminal's cursor where it is shown; else the one
/// cell drawn in reverse video in view (a program's own caret: Claude
/// Code's, and others'). None where there is neither, or two such
/// cells (which is the caret isn't known). `alt`: the alternate
/// screen.
pub fn screen_of(frame: &Frame, alt: bool) -> Option<Screen> {
    let (colors, cursor) = frame.header.as_ref()?;
    let rows = || frame.order.iter().filter_map(|id| frame.rows.get(id)).map(|r| &r.row);
    let cells = |row: &RenderRow| row.cells.iter().map(|c| cell_of(c, colors)).collect();
    if cursor.visible {
        // (Out of view, scrolled back: nothing to type at is seen.)
        let (x, y) = cursor.viewport_pos?;
        let row = rows().find(|r| r.viewport_y == i32::from(y))?;
        let shape = match cursor.style {
            CursorStyle::Bar => 0,
            CursorStyle::Block => 1,
            CursorStyle::Underline => 2,
            CursorStyle::BlockHollow => 3,
        };
        return Some(Screen { row: y, col: x, cells: cells(row), drawn: false, shape, alt });
    }
    let mut found: Option<(&RenderRow, usize)> = None;
    for row in rows().filter(|r| r.viewport_y >= 0) {
        let reverse = |i: usize| row.cells.get(i).is_some_and(|c| c.style.inverse);
        for i in (0..row.cells.len()).filter(|&i| reverse(i) && (i == 0 || !reverse(i - 1))) {
            // One cell, or the two of a wide letter.
            let wide = row.cells[i].wide == CellWide::Wide;
            let len = (i..row.cells.len()).take_while(|&j| reverse(j)).count();
            if len != if wide { 2 } else { 1 } {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some((row, i));
        }
    }
    let (row, col) = found?;
    Some(Screen { row: u16::try_from(row.viewport_y).ok()?, col: col as u16, cells: cells(row), drawn: true, shape: u8::MAX, alt })
}

/// The caret's row as it is painted with `overlay` over it: each cell
/// drawn ahead looks as the cell it stands for (text moved along as it
/// was, a letter typed as the letter before it, the program's caret as
/// the program drew it).
pub fn predicted_row(row: &RenderRow, overlay: &Overlay) -> RenderRow {
    let was = overlay.was as usize;
    // An empty cell of the line: on the ground the caret stands on
    // (the next cell's, where the caret's own is the program's drawing).
    let ground = row.cells.get(if overlay.drawn { was + 1 } else { was }).and_then(|c| c.bg);
    let plain = RenderCell { codepoint: 0, grapheme: String::new(), wide: CellWide::Narrow, fg: None, bg: ground, style: CellStyle::default(), hyperlink: false };
    // Text typed: as the letter before the caret, where there is one
    // and it is text (not a prompt's last space, nor a hint).
    let typed = was.checked_sub(1).and_then(|i| row.cells.get(i)).filter(|c| c.codepoint > u32::from(' ') && !c.style.inverse && !c.style.faint).map_or_else(
        || plain.clone(),
        |c| RenderCell { fg: c.fg, bg: c.bg, style: CellStyle { bold: c.style.bold, italic: c.style.italic, fg_palette_index: c.style.fg_palette_index, ..CellStyle::default() }, ..plain.clone() },
    );
    let mut out = row.clone();
    // (Not the row the engine packed: never known again by its words.)
    out.raw = Default::default();
    for (i, (ch, like)) in overlay.cells.iter().enumerate() {
        let Some(slot) = out.cells.get_mut(overlay.col as usize + i) else { break };
        let mut cell = match like {
            Like::Cell(c) => row.cells.get(*c as usize).cloned().unwrap_or_else(|| plain.clone()),
            Like::Typed => typed.clone(),
            Like::Blank => plain.clone(),
            Like::Caret => row.cells.get(was).cloned().unwrap_or_else(|| plain.clone()),
        };
        (cell.codepoint, cell.wide) = match *ch {
            ' ' => (0, CellWide::Narrow),
            WIDE_TAIL => (0, CellWide::SpacerTail),
            c if olo_rex_vt::codepoint_width(c) == 2 => (c as u32, CellWide::Wide),
            c => (c as u32, CellWide::Narrow),
        };
        cell.grapheme.clear();
        cell.hyperlink = false;
        *slot = cell;
    }
    out
}

#[cfg(test)]
mod tests {
    use olo_rex_vt::{GhosttyTerminal, RenderState, ScreenKind};

    use super::*;
    use crate::predict::Predictor;

    /// A terminal of sixty columns fed `bytes`, as a pane holds it.
    struct Pane {
        term: GhosttyTerminal,
        state: RenderState,
        frame: Frame,
    }

    impl Pane {
        fn new() -> Pane {
            let mut term = GhosttyTerminal::new(60, 12).unwrap();
            term.resize(60, 12, 8, 16).unwrap();
            Pane { term, state: RenderState::new().unwrap(), frame: Frame::new() }
        }

        fn feed(&mut self, bytes: &str) -> Option<Screen> {
            self.term.vt_write(bytes.as_bytes());
            self.frame.capture(&mut self.state, &self.term).unwrap();
            screen_of(&self.frame, self.term.active_screen() == ScreenKind::Alternate)
        }

        fn text(&self, y: i32) -> String {
            let row = self.frame.order.iter().map(|id| &self.frame.rows[id].row).find(|r| r.viewport_y == y).unwrap();
            line(row)
        }
    }

    fn line(row: &RenderRow) -> String {
        row.cells.iter().map(|c| char::from_u32(c.codepoint).filter(|c| *c != '\0').unwrap_or(' ')).collect::<String>().trim_end().to_owned()
    }

    #[test]
    fn the_caret_is_the_cursor_where_it_is_shown() {
        let mut pane = Pane::new();
        let s = pane.feed("$ echo hi").unwrap();
        assert_eq!((s.row, s.col, s.drawn, s.alt), (0, 9, false, false));
        assert_eq!(s.cells.len(), 60);
        assert_eq!(s.cells[2..9].iter().map(|c| c.ch).collect::<String>(), "echo hi");
        // A suggestion in grey after it is dim; a word in a colour of its own isn't.
        let s = pane.feed("\x1b[90m there\x1b[0m\x1b[6D").unwrap();
        assert_eq!(s.col, 9);
        assert!(s.cells[10].dim && !s.cells[4].dim, "{:?}", &s.cells[2..16]);
        let s = pane.feed("\r\n$ \x1b[32mgit\x1b[0m \x1b[36mstatus\x1b[0m").unwrap();
        assert!(s.cells[2..12].iter().all(|c| !c.dim), "{:?}", &s.cells[..12]);
        // Hidden, and nothing drawn in its place: no caret.
        assert_eq!(pane.feed("\x1b[?25l"), None);
    }

    #[test]
    fn a_caret_a_program_draws_is_found_and_typed_at() {
        // An input box as an Ink program draws it: the cursor hidden
        // and left under the box, the caret a cell in reverse video.
        let mut pane = Pane::new();
        let boxed = |text: &str| format!("\x1b[?25l\x1b[H\x1b[J╭{0}╮\r\n│ > {text}\x1b[7m \x1b[27m{1}│\r\n╰{0}╯\r\n", "─".repeat(40), " ".repeat(36 - text.chars().count()));
        let s = pane.feed(&boxed("he")).unwrap();
        assert_eq!((s.row, s.col, s.drawn), (1, 6, true));
        assert!(s.cells[6].ground && !s.cells[5].ground);
        assert_eq!(s.cells[41].ch, '│');
        // Typed at: the letter where the caret was, the caret after it,
        // painted as the program paints them.
        let mut p = Predictor::new(true);
        let t = std::time::Instant::now();
        p.frame(Some(&s), t);
        p.typed(b"l", Some(&s), t);
        let s = pane.feed(&boxed("hel")).unwrap();
        p.frame(Some(&s), t + std::time::Duration::from_millis(300));
        assert!(p.typed(b"p", Some(&s), t + std::time::Duration::from_millis(400)).1);
        let overlay = p.overlay().unwrap();
        let row = pane.frame.order.iter().map(|id| &pane.frame.rows[id].row).find(|r| r.viewport_y == 1).unwrap();
        let painted = predicted_row(row, &overlay);
        assert_eq!(line(&painted), format!("│ > help{}│", " ".repeat(33)));
        assert!(!painted.cells[7].style.inverse && painted.cells[8].style.inverse && !painted.cells[9].style.inverse);
        assert_eq!(painted.cells[7].fg, row.cells[6].fg, "as the letter before it");
        // Two cells in reverse video (a menu's chosen line and a caret,
        // say): which is the caret isn't known.
        assert_eq!(pane.feed("\x1b[5;1H\x1b[7mx\x1b[27m"), None);
        assert_eq!(pane.text(1), format!("│ > hel{}│", " ".repeat(34)));
    }

    #[test]
    fn a_line_in_reverse_video_is_not_a_caret() {
        let mut pane = Pane::new();
        assert_eq!(pane.feed("\x1b[?25l\x1b[7m(END)\x1b[27m"), None);
        // The two cells of a wide letter under a caret are one caret.
        let s = pane.feed("\x1b[2J\x1b[H> \x1b[7m日\x1b[27m").unwrap();
        assert_eq!((s.col, s.drawn), (2, true));
    }
}
