//! Turning a program's output into the emulator's state, in Terminal and
//! in olo-termd alike.
//!
//! Output is fed to the emulator (alacritty_terminal through vte), except
//! for image sequences, which vte doesn't know: the scanner lifts kitty
//! graphics (APC `G`), Sixel (DCS `q`) and iTerm2 images (OSC 1337) out of
//! the stream and hands them to `graphics` with the terminal locked, so
//! they land exactly where the cursor is at that point in the output.
//!
//! Synchronized updates (DEC mode 2026, used by TUIs to avoid flicker) are
//! buffered by vte's processor; the window is only told to redraw once a
//! whole update is in.

use std::{
    os::fd::{AsRawFd, OwnedFd},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
    },
    time::Instant,
};

use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    term::Term,
    vte::ansi::{NamedColor, Processor},
};

use crate::graphics;

/// What the scanner found in a chunk of output.
#[derive(Debug, PartialEq)]
pub enum Piece<'a> {
    Text(&'a [u8]),
    /// Bytes held over from the previous read or a sequence that turned
    /// out not to be an image, passed through as text.
    Held(Vec<u8>),
    Kitty(Vec<u8>),
    Sixel {
        params: Vec<u8>,
        data: Vec<u8>,
    },
    Iterm(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Deciding: ESC _ seen; next byte says whether it's kitty's `G`.
    Apc,
    /// ESC P seen: reading parameters until the final byte.
    DcsHeader,
    /// ESC ] seen: checking for "1337;".
    OscHeader,
    Kitty,
    Sixel,
    Iterm,
}

/// Longest image sequence accepted (anything longer is dropped).
const MAX_SEQUENCE: usize = 128 * 1024 * 1024;

#[derive(Default)]
pub struct Scanner {
    /// A sequence being collected across reads.
    capture: Option<(Kind, Vec<u8>)>,
    /// The sixel's parameters (between ESC P and q).
    params: Vec<u8>,
    /// A lone ESC at the end of the last chunk.
    esc: bool,
    /// Inside a capture, the last byte was ESC (the terminator may follow).
    capture_esc: bool,
    /// The program turned light/dark reports (DEC mode 2031) on or off,
    /// last, in what was scanned.
    pub reports: Option<bool>,
    /// It asked whether the terminal is light or dark (`CSI ? 996 n`).
    pub asked: bool,
}

/// Light/dark reports (contour's DEC mode 2031, as Ghostty, kitty and
/// Neovim have it): turned on and off, and the question.
const REPORTS_ON: &[u8] = b"\x1b[?2031h";
const REPORTS_OFF: &[u8] = b"\x1b[?2031l";
const LIGHT_OR_DARK: &[u8] = b"\x1b[?996n";

/// The answer to [`LIGHT_OR_DARK`], and what's sent unasked when it
/// changes while reports are on: 1 dark, 2 light.
pub fn light_or_dark(dark: bool) -> &'static [u8] {
    if dark { b"\x1b[?997;1n" } else { b"\x1b[?997;2n" }
}

impl Scanner {
    /// Split `input` into text for vte and image sequences. Bytes that
    /// turn out to be ordinary sequences are passed through unchanged.
    pub fn feed<'a>(&mut self, input: &'a [u8], out: &mut Vec<Piece<'a>>) {
        let mut i = 0;
        let mut text_start = 0;
        if self.esc {
            self.esc = false;
            match input.first() {
                Some(b'_') => {
                    self.capture = Some((Kind::Apc, Vec::new()));
                    i = 1;
                }
                Some(b'P') => {
                    self.capture = Some((Kind::DcsHeader, Vec::new()));
                    self.params.clear();
                    i = 1;
                }
                Some(b']') => {
                    self.capture = Some((Kind::OscHeader, Vec::new()));
                    i = 1;
                }
                _ => out.push(Piece::Held(vec![0x1b])),
            }
            text_start = i;
        }
        while i < input.len() {
            if let Some((kind, buf)) = self.capture.as_mut() {
                let b = input[i];
                i += 1;
                match *kind {
                    Kind::Apc => {
                        if b == b'G' {
                            *kind = Kind::Kitty;
                        } else {
                            // Some other APC: let vte have it.
                            out.push(Piece::Held(vec![0x1b, b'_', b]));
                            self.capture = None;
                            text_start = i;
                        }
                    }
                    Kind::DcsHeader => match b {
                        b'0'..=b'9' | b';' => buf.push(b),
                        b'q' => {
                            self.params = std::mem::take(buf);
                            *kind = Kind::Sixel;
                        }
                        _ => {
                            let mut pass = vec![0x1b, b'P'];
                            pass.extend_from_slice(buf);
                            pass.push(b);
                            out.push(Piece::Held(pass));
                            self.capture = None;
                            text_start = i;
                        }
                    },
                    Kind::OscHeader => {
                        buf.push(b);
                        const PREFIX: &[u8] = b"1337;";
                        if !PREFIX.starts_with(buf) {
                            let mut pass = vec![0x1b, b']'];
                            pass.extend_from_slice(buf);
                            out.push(Piece::Held(pass));
                            self.capture = None;
                            text_start = i;
                        } else if buf.len() == PREFIX.len() {
                            buf.clear();
                            *kind = Kind::Iterm;
                        }
                    }
                    Kind::Kitty | Kind::Sixel | Kind::Iterm => {
                        let end = if self.capture_esc {
                            self.capture_esc = false;
                            if b == b'\\' {
                                true
                            } else {
                                buf.push(0x1b);
                                buf.push(b);
                                false
                            }
                        } else if b == 0x1b {
                            self.capture_esc = true;
                            false
                        } else if b == 0x07 && *kind == Kind::Iterm {
                            true
                        } else {
                            buf.push(b);
                            false
                        };
                        if end {
                            let (kind, data) = self.capture.take().unwrap();
                            if data.len() <= MAX_SEQUENCE {
                                out.push(match kind {
                                    Kind::Kitty => Piece::Kitty(data),
                                    Kind::Sixel => Piece::Sixel { params: std::mem::take(&mut self.params), data },
                                    _ => Piece::Iterm(data),
                                });
                            }
                            text_start = i;
                        } else if buf.len() > MAX_SEQUENCE + 16 {
                            // Runaway: stop growing, keep skipping to the end.
                            buf.truncate(MAX_SEQUENCE + 1);
                        }
                    }
                }
                continue;
            }
            // Plain output: look for the next ESC.
            match memchr(0x1b, &input[i..]) {
                None => {
                    i = input.len();
                }
                Some(off) => {
                    let at = i + off;
                    let next = input.get(at + 1).copied();
                    let start = match next {
                        Some(b'_') => Some(Kind::Apc),
                        Some(b'P') => Some(Kind::DcsHeader),
                        Some(b']') => Some(Kind::OscHeader),
                        // (A CSI vte keeps; light/dark reports noted on
                        // the way.)
                        Some(b'[') if input[at..].len() >= LIGHT_OR_DARK.len() && input[at + 2] == b'?' => {
                            let rest = &input[at..];
                            if rest.starts_with(REPORTS_ON) {
                                self.reports = Some(true);
                            } else if rest.starts_with(REPORTS_OFF) {
                                self.reports = Some(false);
                            } else if rest.starts_with(LIGHT_OR_DARK) {
                                self.asked = true;
                            }
                            None
                        }
                        None => {
                            // Can't tell yet: hold the ESC for the next read.
                            if at > text_start {
                                out.push(Piece::Text(&input[text_start..at]));
                            }
                            self.esc = true;
                            return;
                        }
                        _ => None,
                    };
                    match start {
                        Some(kind) => {
                            if at > text_start {
                                out.push(Piece::Text(&input[text_start..at]));
                            }
                            self.capture = Some((kind, Vec::new()));
                            if kind == Kind::DcsHeader {
                                self.params.clear();
                            }
                            i = at + 2;
                            text_start = i;
                        }
                        None => i = at + 1,
                    }
                }
            }
        }
        if self.capture.is_none() && text_start < input.len() {
            out.push(Piece::Text(&input[text_start..]));
        }
    }
}

fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == needle)
}

/// A grid size, for the emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Shared with the thread feeding the emulator: where images go, sizes,
/// the colour scheme, and the shell.
pub struct Shared {
    pub images: graphics::Shared,
    /// Cell size in device pixels, packed (w << 16 | h).
    pub cell: AtomicU32,
    /// Grid size, packed (cols << 16 | rows).
    pub grid: AtomicU32,
    /// The colour scheme in use, for programs that ask (OSC 4/10/11).
    pub scheme: Mutex<crate::colors::Scheme>,
    /// The shell's pid (0 until known).
    pub pid: AtomicI32,
    /// The PTY it runs in, to see what's in the foreground.
    pub pty: Mutex<Option<OwnedFd>>,
    /// The program wants to hear when the colours turn light or dark
    /// (DEC mode 2031).
    pub reports: AtomicBool,
}

impl Shared {
    pub fn new(cell: (u16, u16), size: Size, scheme: crate::colors::Scheme) -> Shared {
        Shared {
            images: Default::default(),
            cell: AtomicU32::new(((cell.0 as u32) << 16) | cell.1 as u32),
            grid: AtomicU32::new(((size.cols as u32) << 16) | size.rows as u32),
            scheme: Mutex::new(scheme),
            pid: AtomicI32::new(0),
            pty: Mutex::new(None),
            reports: AtomicBool::new(false),
        }
    }

    pub fn set_sizes(&self, size: Size, cell: (u16, u16)) {
        self.cell.store(((cell.0 as u32) << 16) | cell.1 as u32, Ordering::Relaxed);
        self.grid.store(((size.cols as u32) << 16) | size.rows as u32, Ordering::Relaxed);
    }

    pub fn shell(&self) -> Option<libc::pid_t> {
        Some(self.pid.load(Ordering::Relaxed)).filter(|&p| p > 0)
    }

    /// The process group in the foreground (the shell's, when it's idle).
    pub fn foreground(&self) -> Option<libc::pid_t> {
        let pty = self.pty.lock().unwrap();
        // SAFETY: tcgetpgrp on the PTY we hold.
        let pgrp = unsafe { libc::tcgetpgrp(pty.as_ref()?.as_raw_fd()) };
        (pgrp > 0).then_some(pgrp)
    }
}

/// Feeds output to an emulator.
pub struct Feeder {
    parser: Processor,
    scanner: Scanner,
}

impl Default for Feeder {
    fn default() -> Self {
        Feeder { parser: Processor::new(), scanner: Scanner::default() }
    }
}

impl Feeder {
    /// When a synchronized update has to be shown by, if one is pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// The update took too long: show what there is.
    pub fn flush<T: EventListener>(&mut self, term: &mut Term<T>) {
        self.parser.stop_sync(term);
    }

    /// Parse `bytes` into `term`; anything to tell the program (image
    /// replies) goes in `reply`. True when there's something new to draw.
    pub fn feed<T: EventListener>(&mut self, term: &mut Term<T>, shared: &Shared, bytes: &[u8], reply: &mut Vec<u8>) -> bool {
        let mut pieces = Vec::new();
        self.scanner.feed(bytes, &mut pieces);
        if let Some(on) = self.scanner.reports.take() {
            shared.reports.store(on, Ordering::Relaxed);
        }
        if std::mem::take(&mut self.scanner.asked) {
            reply.extend_from_slice(light_or_dark(shared.scheme.lock().unwrap().dark()));
        }
        for piece in pieces {
            match piece {
                Piece::Text(text) => self.parser.advance(term, text),
                Piece::Held(text) => self.parser.advance(term, &text),
                image => {
                    // Apply any synchronized output first so the image
                    // lands after it.
                    if self.parser.sync_bytes_count() > 0 {
                        self.parser.stop_sync(term);
                    }
                    let packed = shared.cell.load(Ordering::Relaxed);
                    let cell = (packed >> 16, packed & 0xffff);
                    let bg = term.colors()[NamedColor::Background].map(|c| [c.r, c.g, c.b]).unwrap_or([0, 0, 0]);
                    let mut ctx = graphics::Context { store: &shared.images, cell, reply };
                    match image {
                        Piece::Kitty(body) => graphics::kitty(term, &mut ctx, &body),
                        Piece::Sixel { params, data } => graphics::sixel(term, &mut ctx, &params, &data, bg),
                        Piece::Iterm(body) => graphics::iterm(term, &mut ctx, &body),
                        Piece::Text(_) | Piece::Held(_) => {}
                    }
                }
            }
        }
        self.parser.sync_bytes_count() < bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<String> {
        let mut s = Scanner::default();
        let mut out = Vec::new();
        for c in chunks {
            let mut pieces = Vec::new();
            s.feed(c, &mut pieces);
            for p in pieces {
                out.push(match p {
                    Piece::Text(t) => format!("T:{}", String::from_utf8_lossy(t)),
                    Piece::Held(t) => format!("T:{}", String::from_utf8_lossy(&t)),
                    Piece::Kitty(b) => format!("K:{}", String::from_utf8_lossy(&b)),
                    Piece::Sixel { params, data } => format!("S:{}|{}", String::from_utf8_lossy(&params), String::from_utf8_lossy(&data)),
                    Piece::Iterm(b) => format!("I:{}", String::from_utf8_lossy(&b)),
                });
            }
        }
        out
    }

    #[test]
    fn passes_text_and_lifts_images() {
        assert_eq!(run(&[b"hi\x1b[1mthere"]), ["T:hi\x1b[1mthere"]);
        assert_eq!(run(&[b"a\x1b_Ga=T;AAAA\x1b\\b"]), ["T:a", "K:a=T;AAAA", "T:b"]);
        assert_eq!(run(&[b"\x1bP0;1;0q#1~~\x1b\\"]), ["S:0;1;0|#1~~"]);
        assert_eq!(run(&[b"\x1b]1337;File=inline=1:QQ==\x07z"]), ["I:File=inline=1:QQ==", "T:z"]);
    }

    #[test]
    fn other_sequences_pass_through() {
        // OSC 0 (title) and a non-sixel DCS go to vte untouched.
        let r = run(&[b"\x1b]0;title\x07x"]);
        assert_eq!(r.concat(), "T:\x1b]0T:;title\x07x");
        let r = run(&[b"\x1bP$qm\x1b\\"]);
        assert_eq!(r.concat(), "T:\x1bP$T:qm\x1b\\");
    }

    #[test]
    fn light_and_dark_reports() {
        // Turned on, asked, turned off: noted, and the sequences still
        // go to vte.
        let mut s = Scanner::default();
        let mut pieces = Vec::new();
        s.feed(b"a\x1b[?2031hb\x1b[?996nc", &mut pieces);
        assert_eq!((s.reports, s.asked), (Some(true), true));
        assert_eq!(pieces, [Piece::Text(b"a\x1b[?2031hb\x1b[?996nc")]);
        s.reports = None;
        s.asked = false;
        s.feed(b"\x1b[?2031l\x1b[?2004h", &mut Vec::new());
        assert_eq!((s.reports, s.asked), (Some(false), false));
        // Answered by what the colours are, through the feeder.
        let shared = Shared::new((8, 16), Size { cols: 20, rows: 5 }, crate::colors::Scheme::light());
        let mut term = Term::new(alacritty_terminal::term::Config::default(), &Size { cols: 20, rows: 5 }, alacritty_terminal::event::VoidListener);
        let (mut feeder, mut reply) = (Feeder::default(), Vec::new());
        feeder.feed(&mut term, &shared, b"\x1b[?2031h\x1b[?996n", &mut reply);
        assert!(shared.reports.load(Ordering::Relaxed));
        assert_eq!(reply, b"\x1b[?997;2n");
        *shared.scheme.lock().unwrap() = crate::colors::Scheme::from(&olo_theme::looks::terminal("mocha").unwrap());
        reply.clear();
        feeder.feed(&mut term, &shared, b"\x1b[?996n\x1b[?2031l", &mut reply);
        assert_eq!(reply, b"\x1b[?997;1n");
        assert!(!shared.reports.load(Ordering::Relaxed));
    }

    /// What scanning costs on colourful output (a CSI every few bytes):
    /// `cargo test --release -p olo-terminal scan_cost -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn scan_cost() {
        let line = b"\x1b[38;5;244m drwxr-xr-x\x1b[0m \x1b[1;34mdocs\x1b[0m \x1b[?25l\x1b[2;3H\x1b[32m+added line\x1b[m\r\n";
        let text: Vec<u8> = line.iter().copied().cycle().take(64 << 20).collect();
        let mut s = Scanner::default();
        let start = std::time::Instant::now();
        for chunk in text.chunks(64 * 1024) {
            let mut pieces = Vec::new();
            s.feed(chunk, &mut pieces);
            std::hint::black_box(&pieces);
        }
        let secs = start.elapsed().as_secs_f64();
        println!("scanning {:.0} MB/s ({:.2} ms for 64 MiB)", 64.0 * 1.048576 / secs, secs * 1e3);
        // And all a shell's output goes through: into an emulator too.
        let size = Size { cols: 120, rows: 40 };
        let shared = Shared::new((8, 16), size, crate::colors::Scheme::light());
        let mut term = Term::new(alacritty_terminal::term::Config::default(), &size, alacritty_terminal::event::VoidListener);
        let mut feeder = Feeder::default();
        let start = std::time::Instant::now();
        for chunk in text.chunks(64 * 1024) {
            feeder.feed(&mut term, &shared, chunk, &mut Vec::new());
        }
        let secs = start.elapsed().as_secs_f64();
        println!("feeding {:.0} MB/s ({:.0} ms for 64 MiB)", 64.0 * 1.048576 / secs, secs * 1e3);
    }

    #[test]
    fn split_across_reads() {
        assert_eq!(run(&[b"ab\x1b", b"_Ga=q;", b"xy\x1b", b"\\cd"]), ["T:ab", "K:a=q;xy", "T:cd"]);
        assert_eq!(run(&[b"ab\x1b", b"[31m"]), ["T:ab", "T:\x1b", "T:[31m"]);
    }
}
