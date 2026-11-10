//! One shell (a tab of a window): the emulator state (alacritty_terminal's
//! `Term`), fed from olo-termd (which runs the shell), and what the
//! window needs to know.

use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::Scroll,
    sync::FairMutex,
    term::{Config, Term},
};
use olo_kit_base::Proxy;

use crate::{
    client::{Chunk, Link, Writer},
    io, wire,
};

/// What the emulator reports to the app (via the kit's message channel).
pub enum TermEvent {
    /// New output to draw (at most one in flight per shell).
    Wakeup,
    Title(Option<String>),
    Bell,
    Clipboard(String),
    Exited(Option<i32>),
}

/// An event from shell `.0` (its id: tabs move between windows).
pub struct Message(pub u64, pub TermEvent);

/// A thread's wake-up call, coalesced: poked as often as anything
/// happens, it wakes once. What watches the tabs (agents, repositories)
/// sleeps on one until a shell prints, not on a timer.
#[derive(Clone, Default)]
pub struct Nudge(Arc<(olo_sync::Mutex<bool>, olo_sync::Condvar)>);

impl Nudge {
    pub fn poke(&self) {
        let (poked, woken) = &*self.0;
        let mut p = poked.lock().unwrap();
        if !*p {
            *p = true;
            woken.notify_all();
        }
    }

    /// Asleep until poked (or `timeout` has passed), the poke then taken.
    pub fn wait(&self, timeout: Option<Duration>) {
        let (poked, woken) = &*self.0;
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut p = poked.lock().unwrap();
        while !*p {
            p = match deadline {
                None => woken.wait(p).unwrap(),
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    woken.wait_timeout(p, left).unwrap().0
                }
            };
        }
        *p = false;
    }
}

/// Who's poked when any shell prints.
static ON_OUTPUT: OnceLock<olo_sync::Mutex<Vec<Nudge>>> = OnceLock::new();

/// Poke `nudge` whenever any shell prints (once per burst of output, as
/// it's coalesced) or is attached (taken back from termd after Terminal
/// starts again: where it is can be read only then).
pub fn on_output(nudge: Nudge) {
    ON_OUTPUT.get_or_init(Default::default).lock().unwrap().push(nudge);
}

/// A shell printed, or attached: wake what watches.
fn printed() {
    if let Some(all) = ON_OUTPUT.get() {
        for n in all.lock().unwrap().iter() {
            n.poke();
        }
    }
}

#[derive(Clone)]
pub struct Listener {
    shell: u64,
    proxy: Proxy,
    writer: Writer,
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
                self.writer.send(text.into_bytes());
                return;
            }
            // "What colour is X?": light/dark detection asks for the
            // background (OSC 11).
            Event::ColorRequest(index, format) => {
                use alacritty_terminal::{
                    term::color::Colors,
                    vte::ansi::{Color, NamedColor},
                };
                let scheme = self.shared.scheme.lock().unwrap().clone();
                let none = Colors::default();
                let color = match index {
                    0..=255 => Color::Indexed(index as u8),
                    256 => Color::Named(NamedColor::Foreground),
                    257 => Color::Named(NamedColor::Background),
                    _ => Color::Named(NamedColor::Cursor),
                };
                self.writer.send(format(scheme.resolve(color, &none)).into_bytes());
                return;
            }
            Event::TextAreaSizeRequest(format) => {
                let cell = self.shared.cell.load(Ordering::Relaxed);
                let grid = self.shared.grid.load(Ordering::Relaxed);
                let ws = WindowSize { num_cols: (grid >> 16) as u16, num_lines: (grid & 0xffff) as u16, cell_width: (cell >> 16) as u16, cell_height: (cell & 0xffff) as u16 };
                self.writer.send(format(ws).into_bytes());
                return;
            }
            _ => return,
        };
        self.proxy.send(Message(self.shell, e));
    }
}

pub use crate::io::Size;

/// How a window gets its shell.
pub enum Start {
    Spawn {
        command: Option<Vec<String>>,
        cwd: Option<PathBuf>,
    },
    /// A new shell in place of `from`, gone with the session it ran in:
    /// what it showed first (olo-termd kept it).
    Restore {
        cwd: Option<PathBuf>,
        from: u64,
    },
    /// One olo-termd already has.
    Attach(u64),
    /// One this process ran itself, before olo-termd: hand it over.
    Adopt {
        pty: std::os::fd::OwnedFd,
        pid: i32,
        title: Option<String>,
        replay: String,
    },
}

pub struct Session {
    pub id: u64,
    pub term: Arc<FairMutex<Term<Listener>>>,
    link: Link,
    pending: Arc<AtomicBool>,
    proxy: Proxy,
    reflow: Arc<Reflow>,
    pub shared: Arc<io::Shared>,
    pub size: Size,
    cell: (u16, u16),
    /// Title set by the program (OSC 0/2), if any.
    pub title: Option<String>,
    pub exited: Option<Option<i32>>,
}

/// A new size for the screen, made on a worker: a new width reflows the
/// whole scrollback (15 ms for 10,000 lines 400 wide), so while a window
/// is resized live the screen draws its last frame until it's done.
#[derive(Default)]
struct Reflow {
    /// The size to make next, and its grid for olo-termd.
    want: olo_sync::Mutex<Option<(Size, wire::Grid)>>,
    busy: AtomicBool,
}

fn grid(size: Size, cell: (u16, u16)) -> wire::Grid {
    wire::Grid { cols: size.cols as u16, rows: size.rows as u16, cell_w: cell.0, cell_h: cell.1 }
}

impl Session {
    /// Start a shell (or attach to one). Whether it worked arrives
    /// later: output, or `Exited`.
    pub fn start(proxy: Proxy, link: &Link, size: Size, cell: (u16, u16), scheme: crate::colors::Scheme, start: Start) -> Session {
        let id = match &start {
            Start::Spawn { .. } | Start::Restore { .. } | Start::Adopt { .. } => link.new_id(),
            Start::Attach(id) => *id,
        };
        let shared = Arc::new(io::Shared::new(cell, size, scheme));
        let writer = link.writer(id);
        let listener = Listener { shell: id, proxy: proxy.clone(), writer: writer.clone(), shared: shared.clone() };
        let config = Config { scrolling_history: wire::SCROLLBACK, kitty_keyboard: true, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        let pending = Arc::new(AtomicBool::new(false));
        let wake = {
            let (pending, proxy) = (pending.clone(), proxy.clone());
            move || {
                if !pending.swap(true, Ordering::AcqRel) {
                    proxy.send(Message(id, TermEvent::Wakeup));
                }
            }
        };
        let exited = {
            let proxy = proxy.clone();
            move |code| proxy.send(Message(id, TermEvent::Exited(code)))
        };
        let (tx, rx) = mpsc::channel();
        spawn_feeder(rx, term.clone(), shared.clone(), wake, writer, exited);
        match start {
            Start::Spawn { command, cwd } => link.spawn(id, tx, wire::Spawn { command, cwd, grid: grid(size, cell), restore: None }),
            Start::Restore { cwd, from } => link.spawn(id, tx, wire::Spawn { command: None, cwd: cwd.filter(|d| d.is_dir()), grid: grid(size, cell), restore: Some(from) }),
            Start::Attach(id) => link.attach(id, tx, grid(size, cell)),
            Start::Adopt { pty, pid, title, replay } => {
                link.adopt(id, tx, wire::Adopt { pid, grid: grid(size, cell), title, replay }, pty);
                // It's still our child: collect it when it exits.
                std::thread::spawn(move || {
                    let mut status = 0;
                    // SAFETY: waiting for our own child.
                    unsafe { libc::waitpid(pid, &mut status, 0) };
                });
            }
        }
        Session { id, term, link: link.clone(), pending, proxy, reflow: Arc::default(), shared, size, cell, title: None, exited: None }
    }

    /// What drawing the screen takes, to keep in a frame's paint.
    pub fn source(&self) -> Source {
        Source { term: self.term.clone(), shared: self.shared.clone(), reflow: self.reflow.clone() }
    }

    /// Call when handling a Wakeup, before drawing.
    pub fn woke(&self) {
        self.pending.store(false, Ordering::Release);
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        if self.exited.is_some() {
            return;
        }
        self.link.input(self.id, bytes.into());
    }

    /// Typing goes to the live screen: drop the selection and stop
    /// looking at the scrollback.
    pub fn typed(&self) {
        let mut t = self.term.lock();
        t.selection = None;
        t.scroll_display(Scroll::Bottom);
    }

    pub fn resize(&mut self, size: Size, cell: (u16, u16)) {
        if size == self.size && cell == self.cell {
            return;
        }
        self.shared.set_sizes(size, cell);
        let changed = size != self.size;
        self.size = size;
        self.cell = cell;
        if !changed {
            return self.link.resize(self.id, grid(size, cell));
        }
        // On the worker, which tells olo-termd once the screen has it, so
        // what the shell writes for the new size meets a screen of it.
        let mut want = self.reflow.want.lock().unwrap();
        *want = Some((size, grid(size, cell)));
        if self.reflow.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        drop(want);
        let (term, link, reflow, pending, proxy, id) = (self.term.clone(), self.link.clone(), self.reflow.clone(), self.pending.clone(), self.proxy.clone(), self.id);
        std::thread::Builder::new()
            .name("reflow".into())
            .spawn(move || {
                loop {
                    let next = {
                        let mut want = reflow.want.lock().unwrap();
                        let next = want.take();
                        if next.is_none() {
                            reflow.busy.store(false, Ordering::Release);
                        }
                        next
                    };
                    let Some((size, grid)) = next else { break };
                    term.lock().resize(size);
                    link.resize(id, grid);
                }
                // Draw it.
                if !pending.swap(true, Ordering::AcqRel) {
                    proxy.send(Message(id, TermEvent::Wakeup));
                }
            })
            .expect("thread");
    }

    /// Whether the screen is being reflowed to a new size on the worker:
    /// its lock may be held for milliseconds.
    pub fn reflowing(&self) -> bool {
        self.reflow.busy.load(Ordering::Acquire)
    }

    /// Whether some text is selected (false while it's being reflowed).
    pub fn has_selection(&self) -> bool {
        let term = if self.reflowing() { self.term.try_lock_unfair() } else { Some(self.term.lock()) };
        term.is_some_and(|t| t.selection.as_ref().is_some_and(|s| !s.is_empty()))
    }

    /// Forget the scrollback, here and in olo-termd.
    pub fn clear_history(&self) {
        let mut term = self.term.lock();
        term.grid_mut().clear_history();
        term.selection = None;
        drop(term);
        self.link.clear(self.id);
    }

    /// The window is closing: hang up the shell, as closing a terminal
    /// window does.
    pub fn hang_up(&self) {
        if self.exited.is_none() {
            self.link.close(self.id);
        }
    }

    /// The shell's working directory.
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.shared.shell()?)).ok()
    }

    /// The foreground program's name, when it isn't the shell.
    pub fn foreground(&self) -> Option<String> {
        let pgrp = self.shared.foreground()?;
        if Some(pgrp) == self.shared.shell() {
            return None;
        }
        std::fs::read_to_string(format!("/proc/{pgrp}/comm")).ok().map(|s| s.trim().to_owned())
    }

    /// Whether closing the window would kill something the user is running.
    pub fn busy(&self) -> bool {
        self.exited.is_none() && self.foreground().is_some()
    }
}

/// A screen to draw: the emulator and what goes with it (a frame's paint
/// has it, not the session).
#[derive(Clone)]
pub struct Source {
    pub term: Arc<FairMutex<Term<Listener>>>,
    pub shared: Arc<io::Shared>,
    reflow: Arc<Reflow>,
}

impl Source {
    /// See [`Session::reflowing`].
    pub fn reflowing(&self) -> bool {
        self.reflow.busy.load(Ordering::Acquire)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The shell stays with olo-termd unless `hang_up` said otherwise.
        self.link.forget(self.id);
    }
}

/// Feed what olo-termd sends for one shell into its emulator, until
/// the shell has gone.
fn spawn_feeder(
    rx: mpsc::Receiver<Chunk>,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<io::Shared>,
    wake: impl Fn() + Send + 'static,
    reply: Writer,
    exited: impl FnOnce(Option<i32>) + Send + 'static,
) {
    std::thread::Builder::new()
        .name("shell output".into())
        .spawn(move || {
            let mut feeder = io::Feeder::default();
            let code = loop {
                let chunk = match feeder.deadline() {
                    Some(d) => match rx.recv_timeout(d.saturating_duration_since(std::time::Instant::now())) {
                        Ok(c) => c,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            feeder.flush(&mut *term.lock());
                            wake();
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                    },
                    None => match rx.recv() {
                        Ok(c) => c,
                        Err(_) => break None,
                    },
                };
                match chunk {
                    Chunk::Attached { pid, pty, replay } => {
                        shared.pid.store(pid, Ordering::Relaxed);
                        *shared.pty.lock().unwrap() = pty;
                        *shared.images.lock().unwrap() = Default::default();
                        feeder = io::Feeder::default();
                        // Rebuilding the screen asks nothing of the program.
                        feeder.feed(&mut *term.lock(), &shared, &replay, &mut Vec::new());
                        printed();
                        wake();
                    }
                    Chunk::Output(bytes) => {
                        let mut out = Vec::new();
                        let draw = feeder.feed(&mut *term.lock(), &shared, &bytes, &mut out);
                        if !out.is_empty() {
                            reply.send(out);
                        }
                        printed();
                        if draw {
                            wake();
                        }
                    }
                    Chunk::Exited(code) => break code,
                    Chunk::Detached => break None,
                }
            };
            exited(code);
        })
        .expect("shell output thread");
}
