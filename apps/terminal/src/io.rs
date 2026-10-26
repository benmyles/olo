//! The PTY reader and writer threads.
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
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    sync::{Arc, mpsc},
    time::Instant,
};

use alacritty_terminal::{
    event::EventListener,
    sync::FairMutex,
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
    Sixel { params: Vec<u8>, data: Vec<u8> },
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

/// Shared with the reader thread: where images go and the cell size.
pub struct Shared {
    pub images: graphics::Shared,
    /// Cell size in device pixels, packed (w << 16 | h).
    pub cell: std::sync::atomic::AtomicU32,
    /// Grid size, packed (cols << 16 | rows).
    pub grid: std::sync::atomic::AtomicU32,
    /// The colour scheme in use, for programs that ask (OSC 4/10/11).
    pub scheme: std::sync::Mutex<crate::colors::Scheme>,
}

/// Run the reader: parse output into `term` until the PTY closes, then
/// report the exit status. `wake` asks the window to redraw; `reply`
/// sends bytes back to the program.
pub fn spawn_reader<T: EventListener + Send + 'static>(
    fd: RawFd,
    pid: libc::pid_t,
    term: Arc<FairMutex<Term<T>>>,
    shared: Arc<Shared>,
    wake: impl Fn() + Send + 'static,
    reply: mpsc::Sender<Vec<u8>>,
    exited: impl FnOnce(Option<i32>) + Send + 'static,
) {
    // SAFETY: a duplicate of the session's PTY fd, owned by this thread.
    let mut file = unsafe { File::from_raw_fd(libc::dup(fd)) };
    std::thread::Builder::new()
        .name("pty reader".into())
        .spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut scanner = Scanner::default();
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                // Wait for output, or for a synchronized update to time out.
                let deadline = parser.sync_timeout().sync_timeout();
                let timeout_ms = deadline.map(|d| d.saturating_duration_since(Instant::now()).as_millis() as i32).unwrap_or(-1);
                let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: polling one valid fd.
                let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
                if r == 0 {
                    parser.stop_sync(&mut *term.lock());
                    wake();
                    continue;
                }
                if r < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                let n = match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    // EIO: the program and everything it started are gone.
                    Err(_) => break,
                };
                let mut pieces = Vec::new();
                scanner.feed(&buf[..n], &mut pieces);
                let mut out = Vec::new();
                {
                    let mut t = term.lock();
                    for piece in pieces {
                        match piece {
                            Piece::Text(text) => parser.advance(&mut *t, text),
                            Piece::Held(text) => parser.advance(&mut *t, &text),
                            image => {
                                // Apply any synchronized output first so the
                                // image lands after it.
                                if parser.sync_bytes_count() > 0 {
                                    parser.stop_sync(&mut *t);
                                }
                                let packed = shared.cell.load(std::sync::atomic::Ordering::Relaxed);
                                let cell = (packed >> 16, packed & 0xffff);
                                let bg = t.colors()[NamedColor::Background].map(|c| [c.r, c.g, c.b]).unwrap_or([0, 0, 0]);
                                let mut ctx = graphics::Context { store: &shared.images, cell, reply: &mut out };
                                match image {
                                    Piece::Kitty(body) => graphics::kitty(&mut *t, &mut ctx, &body),
                                    Piece::Sixel { params, data } => graphics::sixel(&mut *t, &mut ctx, &params, &data, bg),
                                    Piece::Iterm(body) => graphics::iterm(&mut *t, &mut ctx, &body),
                                    Piece::Text(_) | Piece::Held(_) => {}
                                }
                            }
                        }
                    }
                }
                if !out.is_empty() {
                    let _ = reply.send(out);
                }
                if parser.sync_bytes_count() < n {
                    wake();
                }
            }
            // Collect the exit status (the program closed the PTY; it has
            // exited or is about to).
            let mut status = 0;
            // SAFETY: waiting for our own child.
            let r = unsafe { libc::waitpid(pid, &mut status, 0) };
            let code = (r == pid).then(|| {
                if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) }
            });
            exited(code);
        })
        .expect("reader thread");
}

/// The writer: input and replies go out in order without ever blocking
/// the UI, even when the program isn't reading.
pub fn spawn_writer(fd: RawFd) -> mpsc::Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    // SAFETY: a duplicate of the PTY fd for this thread.
    let mut file = unsafe { File::from_raw_fd(libc::dup(fd)) };
    std::thread::Builder::new()
        .name("pty writer".into())
        .spawn(move || {
            for bytes in rx {
                if file.write_all(&bytes).is_err() {
                    break;
                }
            }
        })
        .expect("writer thread");
    tx
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
    fn split_across_reads() {
        assert_eq!(run(&[b"ab\x1b", b"_Ga=q;", b"xy\x1b", b"\\cd"]), ["T:ab", "K:a=q;xy", "T:cd"]);
        assert_eq!(run(&[b"ab\x1b", b"[31m"]), ["T:ab", "T:\x1b", "T:[31m"]);
    }
}
