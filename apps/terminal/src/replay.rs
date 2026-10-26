//! What tabula-termd sends a Terminal that attaches to a shell: output
//! that rebuilds the shell's screen and scrollback in a fresh emulator,
//! then the state programs set (the alternate screen and what's on it,
//! cursor keys, keypad, mouse reporting, bracketed paste, the kitty
//! keyboard flags, the cursor's shape) and the title. Images aren't
//! replayed; their cells come back blank.

use alacritty_terminal::{
    event::EventListener,
    grid::{Dimensions, Grid},
    index::{Column, Line},
    term::{Term, TermMode, cell::{Cell, Flags}},
    vte::ansi::{Color, CursorShape, NamedColor, Processor},
};

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
fn row(out: &mut String, grid: &Grid<Cell>, line: i32, end: Option<usize>, last: &mut Option<(Color, Color, Flags)>) {
    let Some(end) = end else { return };
    let row = &grid[Line(line)];
    for c in 0..=end {
        let cell = &row[Column(c)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
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
fn end(grid: &Grid<Cell>, line: i32) -> Option<usize> {
    let row = &grid[Line(line)];
    let cols = grid.columns();
    if row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
        // It continues on the next line: all of it, so it wraps there.
        return Some(cols - 1);
    }
    (0..cols).rev().find(|&c| {
        let cell = &row[Column(c)];
        (cell.c != ' ' && cell.c != '\0') || cell.bg != Color::Named(NamedColor::Background)
    })
}

/// The primary screen and its scrollback, as a stream of lines.
fn history(out: &mut String, grid: &Grid<Cell>) {
    let mut last = None;
    let top = -(grid.history_size() as i32);
    let bottom = grid.screen_lines() as i32 - 1;
    for line in top..=bottom {
        row(out, grid, line, end(grid, line), &mut last);
        let wrapped = grid[Line(line)][Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE);
        if line < bottom && !wrapped {
            out.push_str("\r\n");
        }
    }
    out.push_str("\x1b[0m");
    cursor(out, grid);
}

/// The alternate screen: each row in place.
fn screen(out: &mut String, grid: &Grid<Cell>) {
    let mut last = None;
    for line in 0..grid.screen_lines() as i32 {
        out.push_str(&format!("\x1b[{};1H", line + 1));
        row(out, grid, line, end(grid, line), &mut last);
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
pub fn snapshot<T: EventListener>(term: &mut Term<T>, title: Option<&str>) -> Vec<u8> {
    // Start from nothing (a re-attaching Terminal has a screen already).
    let mut out = String::from("\x1bc");
    if term.mode().contains(TermMode::ALT_SCREEN) {
        let mut alt = String::new();
        screen(&mut alt, term.grid());
        term.swap_alt();
        history(&mut out, term.grid());
        // Going back clears the alternate screen: draw it again.
        term.swap_alt();
        let mut parser: Processor = Processor::new();
        parser.advance(term, alt.as_bytes());
        out.push_str("\x1b[?1049h");
        out.push_str(&alt);
    } else {
        history(&mut out, term.grid());
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

#[cfg(test)]
mod tests {
    use alacritty_terminal::{event::VoidListener, term::Config};

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
            let replay = snapshot(t, Some("hi"));
            let mut b = term(20, 5);
            feed(&mut b, &replay);
            same(t, &b);
        }
        let replay = String::from_utf8(snapshot(&mut a, None)).unwrap();
        assert!(replay.contains("\x1b[0;31mred"));
    }

    #[test]
    fn rebuilds_the_alternate_screen_and_modes() {
        let mut a = term(20, 5);
        feed(&mut a, b"before\r\n$ vim\r\n");
        feed(&mut a, b"\x1b[?1049h\x1b[?1h\x1b=\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[>1u\x1b[?25l\x1b[5 q");
        feed(&mut a, b"\x1b[2;3Hhello\x1b[5;1H~ status\x1b[3;4H");
        let replay = snapshot(&mut a, Some("vim"));
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
}
