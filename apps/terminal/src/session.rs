//! One shell (a tab of a window): the emulator state (alacritty_terminal's
//! `Term`), fed from olo-termd (which runs the shell), and what the
//! window needs to know.

use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    /// New output to draw (at most one in flight per shell), of a shell
    /// whose window isn't known to be drawing it.
    Wakeup,
    /// A shell being drawn printed: the tabs' titles are looked at
    /// again soon (its window was told to draw it, `Session::drawn_in`).
    Printed,
    Title(Option<String>),
    Bell,
    Clipboard(String),
    Exited(Option<i32>),
}

/// An event from shell `.0` (its id: tabs move between windows).
pub struct Message(pub u64, pub TermEvent);

/// Terminal looks at the tabs' titles again soon (`retitle_soon`): a
/// shell printing needn't ask (12 times a second for an agent's spinner,
/// each a wake of Terminal's own thread for nothing).
pub static TITLES_DUE: AtomicBool = AtomicBool::new(false);

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
            Event::Title(t) => {
                self.shared.said_title(Some(&t));
                TermEvent::Title(Some(t))
            }
            Event::ResetTitle => {
                self.shared.said_title(None);
                TermEvent::Title(None)
            }
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
    /// One olo-termd already has, with its known shell PID and/or saved
    /// directory as fallback until `Chunk::Attached` arrives.
    Attach {
        id: u64,
        pid: Option<libc::pid_t>,
        cwd: Option<PathBuf>,
    },
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
    /// The window drawing it, as its screens were last drawn (its id
    /// and one; 0: none): its output tells that window itself, not
    /// Terminal's thread to tell it.
    drawn_in: Arc<AtomicU64>,
    proxy: Proxy,
    reflow: Arc<Reflow>,
    pub shared: Arc<io::Shared>,
    pub size: Size,
    cell: (u16, u16),
    /// Title set by the program (OSC 0/2), if any.
    pub title: Option<String>,
    pub exited: Option<Option<i32>>,
    /// Working directory requested when spawning/restoring this shell, kept as
    /// a fallback until `olo-termd` attaches and supplies a live PID/PTY.
    spawn_cwd: Option<PathBuf>,
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
            Start::Attach { id, .. } => *id,
        };
        let shared = Arc::new(io::Shared::new(cell, size, scheme));
        let writer = link.writer(id);
        let listener = Listener { shell: id, proxy: proxy.clone(), writer: writer.clone(), shared: shared.clone() };
        let config = Config { scrolling_history: wire::SCROLLBACK, kitty_keyboard: true, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        let pending = Arc::new(AtomicBool::new(false));
        let drawn_in = Arc::new(AtomicU64::new(0));
        let wake = {
            let (pending, proxy, drawn_in) = (pending.clone(), proxy.clone(), drawn_in.clone());
            move || {
                if pending.swap(true, Ordering::AcqRel) {
                    return;
                }
                match drawn_in.load(Ordering::Acquire) {
                    // Its window draws it again (from here: going through
                    // Terminal's thread was a wake of that thread an
                    // update, a twelfth of an agent's spinner's cost)...
                    w @ 1.. if crate::ui::screens_changed(crate::ui::WindowId(w - 1)) => {
                        // ...and the titles are looked at again soon.
                        if !TITLES_DUE.load(Ordering::Acquire) {
                            proxy.send(Message(id, TermEvent::Printed));
                        }
                    }
                    _ => proxy.send(Message(id, TermEvent::Wakeup)),
                }
            }
        };
        let exited = {
            let proxy = proxy.clone();
            move |code| proxy.send(Message(id, TermEvent::Exited(code)))
        };
        let (tx, rx) = mpsc::channel();
        spawn_feeder(rx, term.clone(), shared.clone(), wake, writer, exited);
        let spawn_cwd = match &start {
            Start::Spawn { cwd, .. } | Start::Restore { cwd, .. } | Start::Attach { cwd, .. } => cwd.clone(),
            Start::Adopt { .. } => None,
        };
        if let Start::Attach { pid: Some(pid), .. } = &start
            && *pid > 0
        {
            shared.pid.store(*pid, Ordering::Relaxed);
        }
        match start {
            Start::Spawn { command, cwd } => link.spawn(id, tx, wire::Spawn { command, cwd, grid: grid(size, cell), restore: None }),
            Start::Restore { cwd, from } => link.spawn(id, tx, wire::Spawn { command: None, cwd, grid: grid(size, cell), restore: Some(from) }),
            Start::Attach { id, .. } => link.attach(id, tx, grid(size, cell)),
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
        Session { id, term, link: link.clone(), pending, drawn_in, proxy, reflow: Arc::default(), shared, size, cell, title: None, exited: None, spawn_cwd }
    }

    /// What drawing the screen takes, to keep in a frame's paint.
    pub fn source(&self) -> Source {
        Source { term: self.term.clone(), shared: self.shared.clone(), reflow: self.reflow.clone() }
    }

    /// Call when handling a Wakeup, before drawing.
    pub fn woke(&self) {
        self.pending.store(false, Ordering::Release);
    }

    /// Which window draws its screen now (None: none does, a tab not
    /// shown), as that window's screens are drawn.
    pub fn drawn_in(&self, window: Option<crate::ui::WindowId>) {
        self.drawn_in.store(window.map_or(0, |w| w.0 + 1), Ordering::Release);
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

    /// The shell's working directory (used for layout persistence and Undo
    /// Close Tab): where it last said it is, where its process doesn't
    /// follow it (`said`: its word stands while a program of its runs,
    /// an editor during a save), else its `/proc/{shell}/cwd`, falling
    /// back to the requested spawn directory only until `olo-termd`
    /// attaches and supplies a live shell PID. No foreground probe: it's
    /// the shell's own.
    pub fn cwd(&self) -> Option<PathBuf> {
        let shell = self.shared.shell().filter(|&p| p > 0);
        // (Its process's folder read once, for both: a window of shells
        // is walked twice a burst of output, for its agents and its
        // layout.)
        let now = shell.and_then(|_| self.shared.shell_cwd());
        // (Only a folder that can be handed on and kept: `representable`.)
        self.said_cwd(now.as_deref()).or_else(|| resolve_shell_cwd(shell, self.exited.is_some(), self.spawn_cwd.as_deref(), |_| now.clone())).filter(|p| crate::said::representable(p))
    }

    /// The tab's active working directory (where Cmd+T and Cmd+N open):
    /// at the shell's prompt, where it says it is, where its process
    /// doesn't follow it (`said`: brush's stays where it started); else
    /// its foreground process group's `/proc/{fg}/cwd` when readable,
    /// falling back to its shell's `/proc/{shell}/cwd` (and to the
    /// requested spawn directory while waiting for `olo-termd` to
    /// attach). Never calls `stat(2)` on user mounts.
    pub fn foreground_cwd(&self) -> Option<PathBuf> {
        let (fg, shell) = (self.shared.foreground(), self.shared.shell().filter(|&p| p > 0));
        // (Read once each: the shell's folder is wanted by both.)
        let now = shell.and_then(|_| self.shared.shell_cwd());
        let at_prompt = shell.is_some() && fg == shell;
        if at_prompt && let Some(dir) = self.said_cwd(now.as_deref()) {
            return Some(dir);
        }
        resolve_foreground_cwd(fg, shell, self.exited.is_some(), self.spawn_cwd.as_deref(), |pid| if Some(pid) == shell { now.clone() } else { proc_cwd(pid) })
            .filter(|p| crate::said::representable(p))
    }

    /// Where the shell says it is (`said::Said`), where that's to be
    /// followed: its process read (`now`) where it was when it first said,
    /// never seen elsewhere. Only the shell's own word is kept, so it
    /// stands while a program of its runs (the shell can't move then).
    fn said_cwd(&self, now: Option<&std::path::Path>) -> Option<PathBuf> {
        if self.exited.is_some() {
            return None;
        }
        let mut said = self.shared.said.lock().unwrap();
        if !said.has_word() {
            return None;
        }
        said.followed(now)
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
            // (What the shell prints is read into the screen here before
            // any of it is drawn: on every key's way back.)
            crate::client::echo_path();
            let mut feeder = io::Feeder::default();
            let code = loop {
                let chunk = match feeder.deadline() {
                    Some(d) => match rx.recv_timeout(d.saturating_duration_since(std::time::Instant::now())) {
                        Ok(c) => c,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            feeder.flush(&mut *term.lock());
                            // (What was held is on the screen only now:
                            // what reads it looks again.)
                            printed();
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
                        // (What it says of where it is, said again by
                        // the replay's title and its next prompt.)
                        *shared.said.lock().unwrap() = Default::default();
                        feeder = io::Feeder::default();
                        // Rebuilding the screen asks nothing of the program.
                        feeder.feed_replay(&mut *term.lock(), &shared, &replay, &mut Vec::new());
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

fn is_live_proc_cwd(path: &std::path::Path) -> bool {
    !path.as_os_str().is_empty() && !path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)")
}

fn proc_cwd(pid: libc::pid_t) -> Option<PathBuf> {
    // Pure procfs lookup (never `stat(2)` on user mounts, which can stall the
    // UI thread on hung NFS/FUSE/SSHFS mounts). `olo-termd` validates `is_dir`
    // off the UI thread when spawning a shell.
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    is_live_proc_cwd(&path).then_some(path)
}

fn resolve_shell_cwd<F>(shell: Option<libc::pid_t>, exited: bool, spawn_cwd: Option<&std::path::Path>, mut read_dir: F) -> Option<PathBuf>
where
    F: FnMut(libc::pid_t) -> Option<PathBuf>,
{
    match shell.filter(|&p| p > 0) {
        Some(pid) => read_dir(pid),
        None if !exited => spawn_cwd.map(PathBuf::from),
        None => None,
    }
}

fn resolve_foreground_cwd<F>(foreground: Option<libc::pid_t>, shell: Option<libc::pid_t>, exited: bool, spawn_cwd: Option<&std::path::Path>, mut read_dir: F) -> Option<PathBuf>
where
    F: FnMut(libc::pid_t) -> Option<PathBuf>,
{
    let fg = foreground.filter(|&p| p > 0);
    let sh = shell.filter(|&p| p > 0);
    if fg.is_none() && sh.is_none() {
        return (!exited).then(|| spawn_cwd.map(PathBuf::from)).flatten();
    }
    if let Some(pid) = fg
        && let Some(dir) = read_dir(pid)
    {
        return Some(dir);
    }
    if let Some(pid) = sh
        && Some(pid) != fg
    {
        return read_dir(pid);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_cwd_prefers_foreground_and_falls_back_to_shell() {
        let mut probed = Vec::new();
        let dir = resolve_foreground_cwd(Some(200), Some(100), false, None, |pid| {
            probed.push(pid);
            match pid {
                200 => Some(PathBuf::from("/work/subshell")),
                100 => Some(PathBuf::from("/work")),
                _ => None,
            }
        });
        assert_eq!(dir, Some(PathBuf::from("/work/subshell")));
        assert_eq!(probed, [200]);

        // Unreadable or deleted foreground cwd falls back to shell cwd.
        let mut probed = Vec::new();
        let dir = resolve_foreground_cwd(Some(200), Some(100), false, None, |pid| {
            probed.push(pid);
            match pid {
                100 => Some(PathBuf::from("/work")),
                _ => None,
            }
        });
        assert_eq!(dir, Some(PathBuf::from("/work")));
        assert_eq!(probed, [200, 100]);

        // Same pid as foreground and shell is only probed once.
        let mut probed = Vec::new();
        let dir = resolve_foreground_cwd(Some(100), Some(100), false, None, |pid| {
            probed.push(pid);
            None
        });
        assert_eq!(dir, None);
        assert_eq!(probed, [100]);

        // Missing or non-positive pids with no spawn_cwd return None cleanly.
        assert_eq!(resolve_foreground_cwd(None, None, false, None, |_| Some(PathBuf::from("/x"))), None);
        assert_eq!(resolve_foreground_cwd(Some(0), Some(-1), false, None, |_| Some(PathBuf::from("/x"))), None);
    }

    #[test]
    fn shell_cwd_stays_on_shell_while_foreground_cwd_tracks_foreground() {
        let read = |pid: libc::pid_t| match pid {
            200 => Some(PathBuf::from("/scratch")),
            100 => Some(PathBuf::from("/work")),
            _ => None,
        };
        // Persistence / Undo uses shell cwd; Cmd+T / folder UI uses foreground-first cwd.
        assert_eq!(resolve_shell_cwd(Some(100), false, None, read), Some(PathBuf::from("/work")));
        assert_eq!(resolve_foreground_cwd(Some(200), Some(100), false, None, read), Some(PathBuf::from("/scratch")));
    }

    #[test]
    fn rapid_double_cmd_t_keeps_spawn_cwd_until_termd_attaches() {
        let shared = io::Shared::new((8, 16), Size { cols: 80, rows: 24 }, crate::colors::Scheme::light());
        let spawn = PathBuf::from("/work/project");
        // Before Chunk::Attached arrives, pid == 0 and pty == None: both accessors
        // return the requested spawn_cwd so a second rapid Cmd+T inherits it.
        assert_eq!(shared.shell(), None);
        assert_eq!(shared.foreground(), None);
        assert_eq!(resolve_foreground_cwd(shared.foreground(), shared.shell(), false, Some(&spawn), proc_cwd), Some(spawn.clone()));
        assert_eq!(resolve_shell_cwd(shared.shell(), false, Some(&spawn), proc_cwd), Some(spawn.clone()));
        // If the session already exited before attach, don't return stale spawn_cwd.
        assert_eq!(resolve_foreground_cwd(shared.foreground(), shared.shell(), true, Some(&spawn), proc_cwd), None);

        // Once termd attaches and supplies a live shell PID, live /proc wins
        // (and spawn_cwd is no longer used even if the live lookup later returns None).
        let me = std::process::id() as libc::pid_t;
        shared.pid.store(me, Ordering::Relaxed);
        let expected = std::env::current_dir().ok();
        assert_eq!(resolve_foreground_cwd(shared.foreground(), shared.shell(), false, Some(&spawn), proc_cwd), expected);
        assert_eq!(resolve_shell_cwd(shared.shell(), false, Some(&spawn), proc_cwd), expected);
        assert_eq!(resolve_foreground_cwd(shared.foreground(), shared.shell(), false, Some(&spawn), |_| None), None);
    }

    #[test]
    fn reattached_tab_preserves_saved_cwd_or_known_pid_before_chunk_attached() {
        let saved = PathBuf::from("/work/restored");
        // Case 1: reattached tab knows its saved cwd before Chunk::Attached (pid not yet stored).
        let shared = io::Shared::new((8, 16), Size { cols: 80, rows: 24 }, crate::colors::Scheme::light());
        assert_eq!(resolve_foreground_cwd(shared.foreground(), shared.shell(), false, Some(&saved), proc_cwd), Some(saved.clone()));
        assert_eq!(resolve_shell_cwd(shared.shell(), false, Some(&saved), proc_cwd), Some(saved.clone()));

        // Case 2: reattached tab was seeded with ShellInfo::pid before Chunk::Attached arrives.
        let me = std::process::id() as libc::pid_t;
        shared.pid.store(me, Ordering::Relaxed);
        let live = std::env::current_dir().ok();
        assert_eq!(resolve_foreground_cwd(shared.foreground(), shared.shell(), false, Some(&saved), proc_cwd), live);
        assert_eq!(resolve_shell_cwd(shared.shell(), false, Some(&saved), proc_cwd), live);
    }

    #[test]
    fn proc_cwd_reads_live_process_and_rejects_deleted_without_stat() {
        let me = std::process::id() as libc::pid_t;
        let expected = std::env::current_dir().ok();
        assert_eq!(proc_cwd(me), expected);
        // Missing /proc entry returns None without failing.
        assert_eq!(proc_cwd(i32::MAX), None);
        // Kernel-marked unlinked directories are rejected without calling stat(2).
        assert!(is_live_proc_cwd(std::path::Path::new("/var/tmp/nonexistent-without-stat")));
        assert!(!is_live_proc_cwd(std::path::Path::new("/var/tmp/gone (deleted)")));
        assert!(!is_live_proc_cwd(std::path::Path::new("")));
    }
}
