//! What olo-termd sends a Terminal that attaches to a shell: output
//! that rebuilds the shell's screen and scrollback in a fresh emulator,
//! then the state programs set (the alternate screen and what's on it,
//! cursor keys, keypad, mouse reporting, bracketed paste, the kitty
//! keyboard flags, the cursor's shape) and the title. Images come back
//! as kitty graphics: each sent once, then put row by row where its cells
//! are.

use std::collections::HashSet;

use alacritty_terminal::{
    event::EventListener,
    grid::{Dimensions, Grid},
    index::{Column, Line},
    term::{Term, TermMode, cell::{Cell, Flags}},
    vte::ansi::{Color, CursorShape, NamedColor},
};
use base64::Engine;

use crate::graphics::{self, Store};

/// The shell's images, and which of them the replay has sent so far.
struct Images<'a> {
    store: &'a Store,
    sent: HashSet<u32>,
}

impl Images<'_> {
    /// The placement row an image cell shows.
    fn at(&self, cell: &Cell) -> Option<(graphics::Placement, usize)> {
        let (uid, row) = graphics::parse_uri(cell.hyperlink()?.uri())?;
        let p = *self.store.placements.get(&uid)?;
        self.store.images.contains_key(&p.image).then_some((p, row))
    }

    /// Kitty graphics that put row `row` of placement `p` at the cursor,
    /// leaving the cursor after it (sending the image first if need be).
    fn put(&mut self, out: &mut String, p: &graphics::Placement, row: usize) {
        let image = &self.store.images[&p.image];
        // The program's own id, so it can still place or delete it.
        let id = self.store.kitty_id(p.image).unwrap_or(0x8000_0000 | p.image);
        if self.sent.insert(p.image) {
            let packed = if image.packed {
                image.rgba.to_vec()
            } else {
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                let _ = std::io::Write::write_all(&mut z, &image.rgba);
                z.finish().unwrap_or_default()
            };
            let data = base64::engine::general_purpose::STANDARD.encode(packed);
            // In chunks, as programs send them.
            let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
            for (i, chunk) in chunks.iter().enumerate() {
                let more = (i + 1 < chunks.len()) as u8;
                let chunk = std::str::from_utf8(chunk).unwrap_or_default();
                if i == 0 {
                    out.push_str(&format!("\x1b_Ga=t,f=32,s={},v={},i={id},o=z,q=2,m={more};{chunk}\x1b\\", image.width, image.height));
                } else {
                    out.push_str(&format!("\x1b_Gm={more};{chunk}\x1b\\"));
                }
            }
        }
        let [x, y, w, h] = p.src;
        let rows = p.rows.max(1) as u32;
        let (top, bottom) = (y + row as u32 * h / rows, y + (row as u32 + 1) * h / rows);
        out.push_str(&format!("\x1b_Ga=p,i={id},x={x},y={top},w={w},h={},c={},r=1,C=1,q=2\x1b\\", (bottom - top).max(1), p.cols));
        out.push_str(&format!("\x1b[{}C", p.cols));
    }
}

fn sgr(fg: Color, bg: Color, flags: Flags) -> String {
    let mut p: Vec<String> = vec!["0".into()];
    for (f, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if flags.contains(f) {
            p.push(code.into());
        }
    }
    let color = |c: Color, base: u16| -> Option<String> {
        Some(match c {
            Color::Named(n) => {
                let i = n as u16;
                match i {
                    0..8 => format!("{}", base + i),
                    8..16 => format!("{}", base + 60 + i - 8),
                    _ => return None,
                }
            }
            Color::Indexed(i) => format!("{};5;{i}", base + 8),
            Color::Spec(c) => format!("{};2;{};{};{}", base + 8, c.r, c.g, c.b),
        })
    };
    p.extend(color(fg, 30));
    p.extend(color(bg, 40));
    format!("\x1b[{}m", p.join(";"))
}

/// One row's cells from the first up to `end`, with their attributes.
fn row(out: &mut String, grid: &Grid<Cell>, line: i32, end: Option<usize>, last: &mut Option<(Color, Color, Flags)>, images: &mut Images) {
    let Some(end) = end else { return };
    let row = &grid[Line(line)];
    // Cells an image put covers.
    let mut skip = 0;
    for c in 0..=end {
        let cell = &row[Column(c)];
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if let Some((p, r)) = images.at(cell)
            && p.start_col == c
        {
            images.put(out, &p, r);
            skip = p.cols.min(grid.columns() - c) - 1;
            continue;
        }
        let key = (cell.fg, cell.bg, cell.flags & !(Flags::WRAPLINE | Flags::WIDE_CHAR | Flags::LEADING_WIDE_CHAR_SPACER));
        if *last != Some(key) {
            out.push_str(&sgr(key.0, key.1, key.2));
            *last = Some(key);
        }
        out.push(if cell.c == '\0' { ' ' } else { cell.c });
        if let Some(z) = cell.zerowidth() {
            out.extend(z);
        }
    }
}

/// The last column worth replaying on a line (trailing blanks aren't).
fn end(grid: &Grid<Cell>, line: i32, images: &Images) -> Option<usize> {
    let row = &grid[Line(line)];
    let cols = grid.columns();
    if row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
        // It continues on the next line: all of it, so it wraps there.
        return Some(cols - 1);
    }
    (0..cols).rev().find(|&c| {
        let cell = &row[Column(c)];
        (cell.c != ' ' && cell.c != '\0') || cell.bg != Color::Named(NamedColor::Background) || images.at(cell).is_some()
    })
}

/// The primary screen and its scrollback, as a stream of lines.
fn history(out: &mut String, grid: &Grid<Cell>, images: &mut Images) {
    let mut last = None;
    let top = -(grid.history_size() as i32);
    let bottom = grid.screen_lines() as i32 - 1;
    for line in top..=bottom {
        row(out, grid, line, end(grid, line, images), &mut last, images);
        let wrapped = grid[Line(line)][Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE);
        if line < bottom && !wrapped {
            out.push_str("\r\n");
        }
    }
    out.push_str("\x1b[0m");
    cursor(out, grid);
}

/// The alternate screen: each row in place.
fn screen(out: &mut String, grid: &Grid<Cell>, images: &mut Images) {
    let mut last = None;
    for line in 0..grid.screen_lines() as i32 {
        out.push_str(&format!("\x1b[{};1H", line + 1));
        row(out, grid, line, end(grid, line, images), &mut last, images);
    }
    out.push_str("\x1b[0m");
    cursor(out, grid);
}

fn cursor(out: &mut String, grid: &Grid<Cell>) {
    let p = grid.cursor.point;
    out.push_str(&format!("\x1b[{};{}H", p.line.0 + 1, p.column.0 + 1));
}

/// The modes that differ from a fresh terminal's.
fn modes(out: &mut String, mode: TermMode) {
    let private = [
        (TermMode::APP_CURSOR, 1),
        (TermMode::MOUSE_REPORT_CLICK, 1000),
        (TermMode::MOUSE_DRAG, 1002),
        (TermMode::MOUSE_MOTION, 1003),
        (TermMode::FOCUS_IN_OUT, 1004),
        (TermMode::UTF8_MOUSE, 1005),
        (TermMode::SGR_MOUSE, 1006),
        (TermMode::BRACKETED_PASTE, 2004),
    ];
    for (m, n) in private {
        if mode.contains(m) {
            out.push_str(&format!("\x1b[?{n}h"));
        }
    }
    for (m, n) in [(TermMode::SHOW_CURSOR, 25), (TermMode::LINE_WRAP, 7), (TermMode::ALTERNATE_SCROLL, 1007)] {
        if !mode.contains(m) {
            out.push_str(&format!("\x1b[?{n}l"));
        }
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        out.push_str("\x1b=");
    }
    if mode.contains(TermMode::INSERT) {
        out.push_str("\x1b[4h");
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.push_str("\x1b[20h");
    }
    let kitty = [
        TermMode::DISAMBIGUATE_ESC_CODES,
        TermMode::REPORT_EVENT_TYPES,
        TermMode::REPORT_ALTERNATE_KEYS,
        TermMode::REPORT_ALL_KEYS_AS_ESC,
        TermMode::REPORT_ASSOCIATED_TEXT,
    ]
    .iter()
    .enumerate()
    .filter(|(_, m)| mode.contains(**m))
    .fold(0, |bits, (i, _)| bits | 1 << i);
    if kitty != 0 {
        out.push_str(&format!("\x1b[>{kitty}u"));
    }
}

/// Output that brings a fresh emulator to where `term` is. (It swaps
/// `term`'s screens to read the one not showing, and puts them back.)
pub fn snapshot<T: EventListener>(term: &mut Term<T>, title: Option<&str>, store: &Store) -> Vec<u8> {
    let mut images = Images { store, sent: HashSet::new() };
    // Start from nothing (a re-attaching Terminal has a screen already).
    let mut out = String::from("\x1bc");
    if term.mode().contains(TermMode::ALT_SCREEN) {
        let alt = term.grid().clone();
        term.swap_alt();
        history(&mut out, term.grid(), &mut images);
        // Going back clears the alternate screen: put it back as it was.
        term.swap_alt();
        out.push_str("\x1b[?1049h");
        screen(&mut out, &alt, &mut images);
        *term.grid_mut() = alt;
    } else {
        history(&mut out, term.grid(), &mut images);
    }
    modes(&mut out, *term.mode());
    let style = term.cursor_style();
    // DECSCUSR (a steady block is where we start).
    let shape = match (style.shape, style.blinking) {
        (CursorShape::Block, true) => Some(1),
        (CursorShape::Underline, true) => Some(3),
        (CursorShape::Underline, false) => Some(4),
        (CursorShape::Beam, true) => Some(5),
        (CursorShape::Beam, false) => Some(6),
        _ => None,
    };
    if let Some(n) = shape {
        out.push_str(&format!("\x1b[{n} q"));
    }
    if let Some(t) = title {
        out.push_str(&format!("\x1b]2;{}\x07", t.replace(['\x07', '\x1b'], "")));
    }
    out.into_bytes()
}

/// What a shell showed, for a new shell in its place once it has gone
/// (olo-termd keeps it when the session ends): the normal screen and
/// its scrollback, down to the last line with anything on it, leaving
/// the cursor at the start of the next line. What programs set (modes,
/// the alternate screen and what's on it) doesn't come with it.
pub fn archive<T: EventListener>(term: &mut Term<T>, store: &Store) -> Vec<u8> {
    let mut images = Images { store, sent: HashSet::new() };
    let alt = term.mode().contains(TermMode::ALT_SCREEN).then(|| term.grid().clone());
    if alt.is_some() {
        term.swap_alt();
    }
    let mut out = String::new();
    let grid = term.grid();
    let top = -(grid.history_size() as i32);
    let bottom = grid.screen_lines() as i32 - 1;
    if let Some(last) = (top..=bottom).rev().find(|&l| end(grid, l, &images).is_some()) {
        let mut sgr = None;
        for line in top..=last {
            row(&mut out, grid, line, end(grid, line, &images), &mut sgr, &mut images);
            let wrapped = grid[Line(line)][Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE);
            if line == last || !wrapped {
                out.push_str("\r\n");
            }
        }
        out.push_str("\x1b[0m");
    }
    if let Some(alt) = alt {
        // As `snapshot` does: going back clears it, so put it back.
        term.swap_alt();
        *term.grid_mut() = alt;
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

use alacritty_terminal::{event::VoidListener, term::Config};

    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::io::Size;

    fn term(cols: usize, rows: usize) -> Term<VoidListener> {
        let config = Config { scrolling_history: 1000, kitty_keyboard: true, ..Config::default() };
        Term::new(config, &Size { cols, rows }, VoidListener)
    }

    fn feed(t: &mut Term<VoidListener>, bytes: &[u8]) {
        let mut parser: Processor = Processor::new();
        parser.advance(t, bytes);
    }

    fn text(grid: &Grid<Cell>) -> Vec<String> {
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .map(|l| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect::<String>().trim_end().to_owned())
            .collect()
    }

    fn same(a: &Term<VoidListener>, b: &Term<VoidListener>) {
        assert_eq!(text(a.grid()), text(b.grid()));
        assert_eq!(a.grid().cursor.point, b.grid().cursor.point);
        assert_eq!(a.mode(), b.mode());
    }

    #[test]
    fn rebuilds_scrollback_screen_and_cursor() {
        let mut a = term(20, 5);
        for i in 0..12 {
            feed(&mut a, format!("line {i}\r\n").as_bytes());
        }
        feed(&mut a, b"\x1b[31mred\x1b[0m and a line long enough to wrap twice over\r\n$ ");
        // `clear`: the cursor goes to the top with history behind it.
        let mut cleared = term(20, 5);
        feed(&mut cleared, b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\x1b[H\x1b[2J$ ");
        for t in [&mut a, &mut cleared] {
            let replay = snapshot(t, Some("hi"), &Store::default());
            let mut b = term(20, 5);
            feed(&mut b, &replay);
            same(t, &b);
        }
        let replay = String::from_utf8(snapshot(&mut a, None, &Store::default())).unwrap();
        assert!(replay.contains("\x1b[0;31mred"));
    }

    #[test]
    fn rebuilds_the_alternate_screen_and_modes() {
        let mut a = term(20, 5);
        feed(&mut a, b"before\r\n$ vim\r\n");
        feed(&mut a, b"\x1b[?1049h\x1b[?1h\x1b=\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[>1u\x1b[?25l\x1b[5 q");
        feed(&mut a, b"\x1b[2;3Hhello\x1b[5;1H~ status\x1b[3;4H");
        let replay = snapshot(&mut a, Some("vim"), &Store::default());
        // Reading the primary screen left the alternate one as it was.
        let mut again = term(20, 5);
        feed(&mut again, b"before\r\n$ vim\r\n\x1b[?1049h\x1b[?1h\x1b=\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[>1u\x1b[?25l\x1b[5 q\x1b[2;3Hhello\x1b[5;1H~ status\x1b[3;4H");
        same(&a, &again);
        let mut b = term(20, 5);
        feed(&mut b, &replay);
        same(&a, &b);
        assert_eq!(b.cursor_style().shape, CursorShape::Beam);
        // vim quits: the shell's screen is back.
        feed(&mut a, b"\x1b[?1049l");
        feed(&mut b, b"\x1b[?1049l");
        same(&a, &b);
        assert_eq!(text(b.grid())[0], "before");
    }

    #[test]
    fn archives_the_shells_lines_not_a_programs_screen() {
        let mut a = term(20, 5);
        for i in 0..8 {
            feed(&mut a, format!("line {i}\r\n").as_bytes());
        }
        feed(&mut a, b"a line long enough to wrap\r\n$ vim\r\n\x1b[?1049h\x1b[?2004h\x1b[2;3Hin vim");
        let kept = archive(&mut a, &Store::default());
        // Reading it left vim's screen as it was.
        assert!(a.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(text(a.grid())[1].trim(), "in vim");
        // A new shell's screen, wider: the long line unwraps, and its
        // prompt goes on the line after.
        let mut b = term(30, 5);
        feed(&mut b, &kept);
        feed(&mut b, b"$ ");
        let lines = text(b.grid());
        let at = lines.iter().position(|l| l == "line 0").unwrap();
        assert_eq!(lines[at + 8], "a line long enough to wrap");
        assert_eq!(lines[at + 9..], ["$ vim", "$"]);
        assert!(!b.mode().contains(TermMode::ALT_SCREEN) && !b.mode().contains(TermMode::BRACKETED_PASTE));
        assert!(archive(&mut term(20, 5), &Store::default()).is_empty());
    }

    #[test]
    fn rebuilds_images() {
        use base64::Engine;
        let shared = || crate::io::Shared::new((10, 20), Size { cols: 20, rows: 5 }, crate::colors::Scheme::light());
        let feed_images = |t: &mut Term<VoidListener>, sh: &crate::io::Shared, bytes: &[u8]| {
            let mut f = crate::io::Feeder::default();
            f.feed(t, sh, bytes, &mut Vec::new());
            f.flush(t);
        };
        // A 20×40 image (2×2 cells), with the program's id 7.
        let rgba: Vec<u8> = (0..20 * 40 * 4).map(|i| (i % 251) as u8).collect();
        let data = base64::engine::general_purpose::STANDARD.encode(&rgba);
        let (sa, sb) = (shared(), shared());
        let mut a = term(20, 5);
        feed_images(&mut a, &sa, format!("top\r\n\x1b_Ga=T,f=32,s=20,v=40,i=7,q=2;{data}\x1b\\after").as_bytes());
        // olo-termd keeps them compressed.
        sa.images.lock().unwrap().pack();
        let replay = snapshot(&mut a, None, &sa.images.lock().unwrap());
        let mut b = term(20, 5);
        feed_images(&mut b, &sb, &replay);
        same(&a, &b);
        let store = sb.images.lock().unwrap();
        let (&ours, image) = store.images.iter().next().unwrap();
        assert_eq!((store.images.len(), image.width, image.height, store.kitty_id(ours)), (1, 20, 40, Some(7)));
        assert_eq!(*image.rgba, rgba);
        // Both rows of it, in the same cells.
        let images = Images { store: &store, sent: HashSet::new() };
        for line in [1, 2] {
            let (p, row) = images.at(&b.grid()[Line(line)][Column(0)]).unwrap();
            assert_eq!((p.cols, p.src[3], row), (2, 20, 0));
            assert!(images.at(&b.grid()[Line(line)][Column(2)]).is_none());
        }
    }
}
