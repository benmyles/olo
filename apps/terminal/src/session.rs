//! One shell in one window: the emulator state (alacritty_terminal's
//! `Term`), the PTY threads, and what the window needs to know.

use std::{
    os::fd::{AsRawFd, RawFd},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    sync::FairMutex,
    term::{Config, Term},
};
use tabula_kit::{Proxy, WindowId};

use crate::{io, pty::Pty};

/// Lines of scrollback kept per window.
pub const SCROLLBACK: usize = 10_000;

/// What the emulator reports to the app (via the kit's message channel).
pub enum TermEvent {
    /// New output to draw (at most one in flight per window).
    Wakeup,
    Title(Option<String>),
    Bell,
    Clipboard(String),
    Exited(Option<i32>),
}

pub struct Message(pub WindowId, pub TermEvent);

#[derive(Clone)]
pub struct Listener {
    win: WindowId,
    proxy: Proxy,
    writer: mpsc::Sender<Vec<u8>>,
    shared: Arc<io::Shared>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let e = match event {
            Event::Title(t) => TermEvent::Title(Some(t)),
            Event::ResetTitle => TermEvent::Title(None),
            Event::Bell => TermEvent::Bell,
            Event::ClipboardStore(_, text) => TermEvent::Clipboard(text),
            // Answers to the program go straight back, not via the UI.
            Event::PtyWrite(text) => {
                // Primary device attributes: say we do Sixel graphics (4)
                // as well as being a VT220-class terminal.
                let text = if text == "\x1b[?6c" { "\x1b[?62;4;22c".to_owned() } else { text };
                let _ = self.writer.send(text.into_bytes());
                return;
            }
            // "What colour is X?": light/dark detection asks for the
            // background (OSC 11).
            Event::ColorRequest(index, format) => {
                use alacritty_terminal::{term::color::Colors, vte::ansi::{Color, NamedColor}};
                let scheme = self.shared.scheme.lock().unwrap().clone();
                let none = Colors::default();
                let color = match index {
                    0..=255 => Color::Indexed(index as u8),
                    256 => Color::Named(NamedColor::Foreground),
                    257 => Color::Named(NamedColor::Background),
                    _ => Color::Named(NamedColor::Cursor),
                };
                let _ = self.writer.send(format(scheme.resolve(color, &none)).into_bytes());
                return;
            }
            Event::TextAreaSizeRequest(format) => {
                let cell = self.shared.cell.load(Ordering::Relaxed);
                let grid = self.shared.grid.load(Ordering::Relaxed);
                let ws = WindowSize {
                    num_cols: (grid >> 16) as u16,
                    num_lines: (grid & 0xffff) as u16,
                    cell_width: (cell >> 16) as u16,
                    cell_height: (cell & 0xffff) as u16,
                };
                let _ = self.writer.send(format(ws).into_bytes());
                return;
            }
            _ => return,
        };
        self.proxy.send(Message(self.win, e));
    }
}

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

pub struct Session {
    pub term: Arc<FairMutex<Term<Listener>>>,
    writer: mpsc::Sender<Vec<u8>>,
    pending: Arc<AtomicBool>,
    pub shared: Arc<io::Shared>,
    pub size: Size,
    pub pid: libc::pid_t,
    master: std::os::fd::OwnedFd,
    /// Title set by the program (OSC 0/2), if any.
    pub title: Option<String>,
    pub exited: Option<Option<i32>>,
}

impl Session {
    /// Start a session on a new or inherited PTY.
    pub fn start(
        win: WindowId,
        proxy: Proxy,
        size: Size,
        cell: (u16, u16),
        scheme: crate::colors::Scheme,
        pty: std::io::Result<Pty>,
    ) -> std::io::Result<Session> {
        let Pty { master, pid } = pty?;
        let fd = master.as_raw_fd();
        let shared = Arc::new(io::Shared {
            images: Default::default(),
            cell: AtomicU32::new(((cell.0 as u32) << 16) | cell.1 as u32),
            grid: AtomicU32::new(((size.cols as u32) << 16) | size.rows as u32),
            scheme: std::sync::Mutex::new(scheme),
        });
        let writer = io::spawn_writer(fd);
        let listener = Listener { win, proxy: proxy.clone(), writer: writer.clone(), shared: shared.clone() };
        let config = Config { scrolling_history: SCROLLBACK, kitty_keyboard: true, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        let pending = Arc::new(AtomicBool::new(false));
        let wake = {
            let (pending, proxy) = (pending.clone(), proxy.clone());
            move || {
                if !pending.swap(true, Ordering::AcqRel) {
                    proxy.send(Message(win, TermEvent::Wakeup));
                }
            }
        };
        let exited = move |code| proxy.send(Message(win, TermEvent::Exited(code)));
        io::spawn_reader(fd, pid, term.clone(), shared.clone(), wake, writer.clone(), exited);
        crate::pty::resize(fd, window_size(size, cell));
        Ok(Session { term, writer, pending, shared, size, pid, master, title: None, exited: None })
    }

    pub fn fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }

    /// Call when handling a Wakeup, before drawing.
    pub fn woke(&self) {
        self.pending.store(false, Ordering::Release);
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        if bytes.is_empty() || self.exited.is_some() {
            return;
        }
        let _ = self.writer.send(bytes);
    }

    /// Typing goes to the live screen: drop the selection and stop
    /// looking at the scrollback.
    pub fn typed(&self) {
        let mut t = self.term.lock();
        t.selection = None;
        t.scroll_display(Scroll::Bottom);
    }

    pub fn resize(&mut self, size: Size, cell: (u16, u16)) {
        self.shared.cell.store(((cell.0 as u32) << 16) | cell.1 as u32, Ordering::Relaxed);
        if size == self.size {
            return;
        }
        self.size = size;
        self.shared.grid.store(((size.cols as u32) << 16) | size.rows as u32, Ordering::Relaxed);
        self.term.lock().resize(size);
        crate::pty::resize(self.fd(), window_size(size, cell));
    }

    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.pid)).ok()
    }

    /// The foreground program's name, when it isn't the shell.
    pub fn foreground(&self) -> Option<String> {
        // SAFETY: tcgetpgrp on our open PTY master.
        let pgrp = unsafe { libc::tcgetpgrp(self.fd()) };
        if pgrp <= 0 || pgrp == self.pid {
            return None;
        }
        std::fs::read_to_string(format!("/proc/{pgrp}/comm")).ok().map(|s| s.trim().to_owned())
    }

    /// Whether closing the window would kill something the user is running.
    pub fn busy(&self) -> bool {
        self.exited.is_none() && self.foreground().is_some()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.exited.is_none() {
            // Hang up the shell, as closing a terminal window does.
            // SAFETY: signalling our own child's process group.
            unsafe { libc::kill(-self.pid, libc::SIGHUP) };
            unsafe { libc::kill(self.pid, libc::SIGHUP) };
        }
    }
}

pub fn window_size(size: Size, cell: (u16, u16)) -> WindowSize {
    WindowSize {
        num_lines: size.rows as u16,
        num_cols: size.cols as u16,
        cell_width: cell.0,
        cell_height: cell.1,
    }
}

/// The scrollback and screen as text with SGR attributes, to replay into a
/// fresh emulator after an upgrade. Ends with the cursor where it was.
pub fn serialize<T>(term: &Term<T>) -> String {
    use alacritty_terminal::{
        index::{Column, Line, Point},
        term::cell::Flags,
        vte::ansi::Color,
    };
    let grid = term.grid();
    let cursor = grid.cursor.point;
    let mut out = String::new();
    let mut last: Option<(Color, Color, Flags)> = None;
    let attrs = |fg: Color, bg: Color, flags: Flags| -> String {
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
    };
    let top = -(grid.history_size() as i32);
    let cols = grid.columns();
    for line in top..=cursor.line.0 {
        let row = &grid[Line(line)];
        // Trailing blanks aren't worth replaying (except up to the cursor).
        let mut end = (0..cols).rev().find(|&c| {
            let cell = &row[Column(c)];
            cell.c != ' ' || cell.bg != Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background)
        });
        if line == cursor.line.0 {
            end = end.max(cursor.column.0.checked_sub(1));
        }
        if let Some(end) = end {
            for c in 0..=end {
                let cell = &row[Column(c)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let key = (cell.fg, cell.bg, cell.flags & !(Flags::WRAPLINE | Flags::WIDE_CHAR | Flags::LEADING_WIDE_CHAR_SPACER));
                if last != Some(key) {
                    out.push_str(&attrs(key.0, key.1, key.2));
                    last = Some(key);
                }
                out.push(if cell.c == '\0' { ' ' } else { cell.c });
                if let Some(z) = cell.zerowidth() {
                    out.extend(z);
                }
            }
        }
        let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        if line < cursor.line.0 && !wrapped {
            out.push_str("\r\n");
        }
    }
    out.push_str("\x1b[0m");
    // Put the cursor back where it was on its line.
    out.push_str(&format!("\r\x1b[{}G", cursor.column.0 + 1));
    let _ = Point::new(Line(0), Column(0));
    out
}
