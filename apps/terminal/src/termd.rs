//! olo-termd: Terminal's shells, kept apart from Terminal.
//!
//! One runs per display, started by the first Terminal that needs it. It
//! owns every shell's PTY and keeps its own copy of each screen (the same
//! emulator and image handling as Terminal's), so when Terminal quits,
//! crashes or upgrades, the shells carry on, and the next Terminal attaches
//! to them: it gets the screen and scrollback replayed (replay.rs), then the
//! live output. A Terminal that vanishes without saying goodbye is started
//! again. When the desktop session ends, so do the shells; when the last
//! shell has gone and no Terminal is attached, so does olo-termd. An
//! upgrade (SIGURG from olo-session) re-executes it in place, handing
//! the PTYs and screens to its new self. What each shell shows is kept
//! on disk (now and then, and when the session ends), so the next
//! session's Terminal can start a shell in its place with it
//! (`Spawn::restore`).

#[allow(dead_code)]
mod colors;
#[allow(dead_code)]
mod graphics;
#[allow(dead_code)]
mod io;
mod pty;
mod replay;
#[allow(dead_code)]
mod wire;

use olo_sync::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    sync::FairMutex,
    term::{Config, Term, TermMode},
};
use serde::{Deserialize, Serialize};

use io::Size;
use wire::{Frame, Grid, kept_dir, kept_path};

/// Where an upgraded olo-termd finds what its previous self handed on.
const RESTORE_ENV: &str = "OLO_TERMD_RESTORE";
/// Output queued for a Terminal before we stop reading a shell's.
const BACKLOG: usize = 8 << 20;
/// How long a Terminal that vanished has to come back on its own (an
/// upgrade re-executes it) before we start it again.
const RELAUNCH_AFTER: Duration = Duration::from_secs(2);
/// Starts allowed per minute, in case it can't stay up.
const RELAUNCHES: usize = 3;
/// How long to stay with nothing to do.
const LINGER: Duration = Duration::from_secs(3);
/// How often a shell that has printed since is kept on disk, so a
/// session that ends with no warning (the battery ran out) still has it.
const KEEP_EVERY: Duration = Duration::from_secs(60);
/// Kept shells no Terminal took back are forgotten after this.
const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// A shell has printed since shells were last kept: only then is there a
/// keep to wake for (idle, olo-termd sleeps until something happens).
static PRINTED: AtomicBool = AtomicBool::new(false);

fn log(message: impl std::fmt::Display) {
    eprintln!("olo-termd: {message}");
}

/// Frames for one Terminal, and how many bytes of them are still queued.
#[derive(Clone)]
struct Outbox {
    conn: u64,
    tx: mpsc::Sender<Frame>,
    backlog: Arc<AtomicUsize>,
}

impl Outbox {
    fn send(&self, f: Frame) {
        let n = f.data.len();
        self.backlog.fetch_add(n, Ordering::AcqRel);
        if self.tx.send(f).is_err() {
            self.backlog.fetch_sub(n, Ordering::AcqRel);
        }
    }
}

/// Which Terminal sees a shell's output.
type Attached = Arc<Mutex<Option<Outbox>>>;

/// What a shell's emulator reports.
#[derive(Clone)]
struct Listener {
    title: Arc<Mutex<Option<String>>>,
    attached: Attached,
    writer: mpsc::Sender<Vec<u8>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.lock().unwrap() = Some(t),
            Event::ResetTitle => *self.title.lock().unwrap() = None,
            // The attached Terminal answers the program's questions; we do
            // while none is.
            Event::PtyWrite(text) if self.attached.lock().unwrap().is_none() => {
                let text = if text == "\x1b[?6c" { "\x1b[?62;4;22c".to_owned() } else { text };
                let _ = self.writer.send(text.into_bytes());
            }
            _ => {}
        }
    }
}

struct Shell {
    /// Order of creation, to list them in.
    seq: u64,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<io::Shared>,
    master: OwnedFd,
    pid: libc::pid_t,
    grid: Grid,
    writer: mpsc::Sender<Vec<u8>>,
    attached: Attached,
    title: Arc<Mutex<Option<String>>>,
    /// The Terminal that had it, gone without a goodbye (re-executing
    /// for an upgrade or a lost compositor, or crashed), till another
    /// takes it: listed as its client still, so the other Terminals
    /// restarting with it leave it be while that process lives (each had
    /// opened a window for every other's shells, which closed again as
    /// their owners came back: twenty for five after an app_server
    /// reload), and a crashed one's are free to adopt.
    left_by: Mutex<Option<libc::pid_t>>,
    /// Bumped by each read of the shell's output…
    output: Arc<AtomicU64>,
    /// …and what it was when the shell was last kept on disk.
    kept: u64,
}

impl Shell {
    fn resize(&mut self, grid: Grid) {
        let size = Size { cols: grid.cols.max(1) as usize, rows: grid.rows.max(1) as usize };
        self.shared.set_sizes(size, (grid.cell_w, grid.cell_h));
        if grid != self.grid {
            self.grid = grid;
            self.term.lock().resize(size);
            pty::resize(self.master.as_raw_fd(), window_size(grid));
        }
    }

    /// Hang up, as closing a terminal window does.
    fn hang_up(&self) {
        // SAFETY: signalling our own child and its process group.
        unsafe {
            libc::kill(-self.pid, libc::SIGHUP);
            libc::kill(self.pid, libc::SIGHUP);
        }
    }
}

/// Keep what shell `id` shows on disk, for `Spawn::restore`.
fn keep(id: u64, term: &FairMutex<Term<Listener>>, shared: &io::Shared) {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let lines = replay::archive(&mut term.lock(), &shared.images.lock().unwrap());
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let Ok(packed) = z.write_all(&lines).and_then(|_| z.finish()) else { return };
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(kept_dir());
    let path = kept_path(id);
    let new = path.with_extension("z.new");
    let wrote = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new).and_then(|mut f| f.write_all(&packed));
    match wrote.and_then(|_| std::fs::rename(&new, &path)) {
        Ok(()) => {}
        Err(e) => log(format!("can't keep shell {id}: {e}")),
    }
}

/// What shell `id` of an earlier session showed, if it was kept, and
/// when (it's forgotten: it belongs to the new shell now).
fn take_kept(id: u64) -> Option<(Vec<u8>, SystemTime)> {
    let path = kept_path(id);
    let when = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let packed = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut lines = Vec::new();
    flate2::read::ZlibDecoder::new(&packed[..]).read_to_end(&mut lines).ok()?;
    Some((lines, when))
}

/// Forget kept shells older than `KEPT_FOR`, and any half written.
fn forget_old_kept() {
    let Ok(dir) = std::fs::read_dir(kept_dir()) else { return };
    let now = SystemTime::now();
    for e in dir.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).unwrap_or_default() > KEPT_FOR);
        if old || e.path().extension().is_some_and(|x| x == "new") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn window_size(g: Grid) -> WindowSize {
    WindowSize { num_lines: g.rows, num_cols: g.cols, cell_width: g.cell_w, cell_height: g.cell_h }
}

struct Client {
    outbox: Outbox,
    pid: i32,
    /// Said goodbye: it quit on purpose.
    bye: bool,
}

enum Ev {
    /// A shell's program has gone.
    Exited(u64),
    /// A Terminal disconnected (saying goodbye first, or not).
    Gone {
        bye: bool,
    },
    Upgrade,
    /// Asked to re-execute (whether or not there's a new version).
    Restart,
    /// The session ended, or we were told to stop.
    End,
    /// A shell printed, the first since shells were last kept.
    Printed,
    /// Our socket's folder changed (the socket deleted or replaced, the
    /// folder gone).
    Socket,
}

struct State {
    shells: HashMap<u64, Shell>,
    clients: HashMap<u64, Client>,
    seq: u64,
    /// The latest Terminal's environment: shells get it, and it's how to
    /// start Terminal again.
    env: Vec<(String, String)>,
    events: mpsc::Sender<Ev>,
    /// When we started (seconds since the epoch).
    started: u64,
}

type St = Arc<Mutex<State>>;

/// What an upgraded olo-termd gets from its previous self.
#[derive(Serialize, Deserialize)]
struct Carried {
    listener: RawFd,
    lock: RawFd,
    env: Vec<(String, String)>,
    shells: Vec<CarriedShell>,
}

#[derive(Serialize, Deserialize)]
struct CarriedShell {
    id: u64,
    fd: RawFd,
    pid: i32,
    grid: Grid,
    title: Option<String>,
    replay: Vec<u8>,
}

fn new_shell(st: &mut State, id: u64, pty: pty::Pty, grid: Grid, replay: Option<&[u8]>, title: Option<String>) {
    let pty::Pty { master, pid } = pty;
    let attached: Attached = Arc::new(Mutex::new(None));
    let title = Arc::new(Mutex::new(title));
    let writer = spawn_writer(&master);
    let output = Arc::new(AtomicU64::new(0));
    let listener = Listener { title: title.clone(), attached: attached.clone(), writer: writer.clone() };
    let size = Size { cols: grid.cols.max(1) as usize, rows: grid.rows.max(1) as usize };
    let config = Config { scrolling_history: wire::SCROLLBACK, kitty_keyboard: true, ..Config::default() };
    let mut term = Term::new(config, &size, listener);
    let shared = Arc::new(io::Shared::new((grid.cell_w, grid.cell_h), size, colors::Scheme::light()));
    if let Some(replay) = replay {
        // Images and all.
        io::Feeder::default().feed(&mut term, &shared, replay, &mut Vec::new());
        shared.images.lock().unwrap().pack();
    }
    let term = Arc::new(FairMutex::new(term));
    shared.pid.store(pid, Ordering::Relaxed);
    pty::resize(master.as_raw_fd(), window_size(grid));
    match master.try_clone() {
        Ok(fd) => spawn_reader(id, File::from(fd), pid, term.clone(), shared.clone(), attached.clone(), writer.clone(), output.clone(), st.events.clone()),
        Err(e) => log(format!("shell {id}: {e}")),
    }
    st.seq += 1;
    st.shells.insert(id, Shell { seq: st.seq, term, shared, master, pid, grid, writer, attached, title, left_by: Mutex::new(None), output, kept: 0 });
}

/// Read a shell's output into its emulator, and on to the Terminal
/// attached, until the program has gone.
#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    id: u64,
    mut file: File,
    pid: libc::pid_t,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<io::Shared>,
    attached: Attached,
    writer: mpsc::Sender<Vec<u8>>,
    output: Arc<AtomicU64>,
    events: mpsc::Sender<Ev>,
) {
    std::thread::Builder::new()
        .name("pty reader".into())
        .spawn(move || {
            let mut feeder = io::Feeder::default();
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let timeout_ms = feeder.deadline().map(|d| d.saturating_duration_since(Instant::now()).as_millis() as i32).unwrap_or(-1);
                let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: polling one valid fd.
                let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
                if r == 0 {
                    feeder.flush(&mut *term.lock());
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
                let mut replies = Vec::new();
                let backlog = {
                    let mut t = term.lock();
                    feeder.feed(&mut *t, &shared, &buf[..n], &mut replies);
                    // Still locked, so a Terminal attaching now gets this
                    // either in its replay or here, not both.
                    let out = attached.lock().unwrap();
                    match out.as_ref() {
                        Some(o) => {
                            o.send(Frame::new(wire::OUTPUT, id, buf[..n].to_vec()));
                            Some(o.backlog.clone())
                        }
                        None => {
                            if !replies.is_empty() {
                                let _ = writer.send(replies);
                            }
                            None
                        }
                    }
                };
                output.fetch_add(1, Ordering::Relaxed);
                if !PRINTED.swap(true, Ordering::AcqRel) {
                    let _ = events.send(Ev::Printed);
                }
                // Images are kept only to send on, so compressed (not
                // holding the screen up meanwhile).
                shared.images.lock().unwrap().pack();
                // Don't run ahead of a Terminal that's behind.
                if let Some(b) = backlog {
                    while b.load(Ordering::Acquire) > BACKLOG {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
            let mut status = 0;
            // SAFETY: waiting for our own child.
            let r = unsafe { libc::waitpid(pid, &mut status, 0) };
            let code = (r == pid).then(|| if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) });
            if let Some(o) = attached.lock().unwrap().as_ref() {
                o.send(Frame::json(wire::EXITED, id, &code));
            }
            let _ = events.send(Ev::Exited(id));
        })
        .expect("reader thread");
}

/// Input for a shell goes out in order without holding anyone up, even
/// when the program isn't reading.
fn spawn_writer(master: &OwnedFd) -> mpsc::Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let Ok(fd) = master.try_clone() else { return tx };
    let mut file = File::from(fd);
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

/// Give `conn` the shell `id`: its screen so far, then its output.
fn attach(st: &mut State, conn: u64, id: u64, grid: Grid) {
    let Some(client) = st.clients.get(&conn) else { return };
    let outbox = client.outbox.clone();
    let Some(shell) = st.shells.get_mut(&id) else {
        outbox.send(Frame::json(wire::EXITED, id, &None::<i32>));
        return;
    };
    shell.resize(grid);
    let mut term = shell.term.lock();
    let title = shell.title.lock().unwrap().clone();
    let replay = replay::snapshot(&mut term, title.as_deref(), &shell.shared.images.lock().unwrap(), shell.shared.reports.load(Ordering::Relaxed));
    let alt = term.mode().contains(TermMode::ALT_SCREEN);
    let mut data = shell.pid.to_le_bytes().to_vec();
    data.extend_from_slice(&replay);
    let mut frame = Frame::new(wire::ATTACHED, id, data);
    frame.fd = shell.master.try_clone().ok();
    let mut out = shell.attached.lock().unwrap();
    if let Some(old) = out.take()
        && old.conn != conn
    {
        old.send(Frame::new(wire::DETACHED, id, Vec::new()));
    }
    outbox.send(frame);
    *out = Some(outbox);
    drop(out);
    *shell.left_by.lock().unwrap() = None;
    drop(term);
    // A full-screen program draws itself again for the new window.
    if alt {
        // SAFETY: tcgetpgrp on our PTY; SIGWINCH to what runs in it.
        unsafe {
            let pgrp = libc::tcgetpgrp(shell.master.as_raw_fd());
            if pgrp > 0 {
                libc::kill(-pgrp, libc::SIGWINCH);
            }
        }
    }
}

/// The shells, oldest first.
fn shells(s: &State) -> Vec<wire::ShellInfo> {
    let pids: HashMap<u64, i32> = s.clients.iter().map(|(c, cl)| (*c, cl.pid)).collect();
    let mut shells: Vec<(u64, wire::ShellInfo)> = s
        .shells
        .iter()
        .map(|(&id, sh)| {
            let client = sh.attached.lock().unwrap().as_ref().and_then(|o| pids.get(&o.conn).copied()).or(*sh.left_by.lock().unwrap());
            let title = sh.title.lock().unwrap().clone();
            (sh.seq, wire::ShellInfo { id, pid: sh.pid, client, grid: sh.grid, title })
        })
        .collect();
    shells.sort_by_key(|(seq, _)| *seq);
    shells.into_iter().map(|(_, i)| i).collect()
}

fn handle(st: &St, conn: u64, f: Frame) {
    let mut s = st.lock().unwrap();
    match f.kind {
        wire::HELLO => {
            let Some(hello) = f.parse::<wire::Hello>() else { return };
            // The same Terminal again (it re-executed itself to upgrade):
            // its old connection is done with.
            let old: Vec<u64> = s.clients.iter().filter(|(c, cl)| **c != conn && cl.pid == hello.pid).map(|(c, _)| *c).collect();
            for shell in s.shells.values() {
                let mut a = shell.attached.lock().unwrap();
                if a.as_ref().is_some_and(|o| old.contains(&o.conn)) {
                    *a = None;
                    *shell.left_by.lock().unwrap() = Some(hello.pid);
                }
            }
            if let Some(c) = s.clients.get_mut(&conn) {
                c.pid = hello.pid;
            }
            s.env = hello.env;
        }
        wire::LIST => {
            if let Some(c) = s.clients.get(&conn) {
                c.outbox.send(Frame::json(wire::SHELLS, 0, &shells(&s)));
            }
        }
        wire::STATUS => {
            let status = wire::Status {
                pid: std::process::id() as i32,
                started: s.started,
                // Terminals (not those just asking, like this).
                clients: s.clients.values().filter(|c| c.pid != 0).count(),
                shells: shells(&s),
            };
            if let Some(c) = s.clients.get(&conn) {
                c.outbox.send(Frame::json(wire::REPORT, 0, &status));
            }
        }
        wire::RESTART => {
            log("asked to restart");
            let _ = s.events.send(Ev::Restart);
        }
        wire::SPAWN => {
            let Some(spawn) = f.parse::<wire::Spawn>() else { return };
            if s.shells.contains_key(&f.id) {
                if let Some(c) = s.clients.get(&conn) {
                    c.outbox.send(Frame::json(wire::EXITED, f.id, &None::<i32>));
                }
                return;
            }
            let home = s.env.iter().find(|(k, _)| k == "HOME").map(|(_, v)| PathBuf::from(v));
            let cwd = spawn.cwd.filter(|d| d.is_dir()).or(home);
            // The shell it stands in for: its lines, then when they're from.
            let before = spawn.restore.and_then(take_kept).map(|(mut lines, when)| {
                let when = olo_kit_base::files::format_time(Some(when));
                lines.extend_from_slice(format!("\x1b[2mRestored from {when}\x1b[0m\r\n").as_bytes());
                lines
            });
            match pty::Pty::spawn(spawn.command.as_deref(), cwd.as_deref(), window_size(spawn.grid), &s.env) {
                Ok(p) => {
                    new_shell(&mut s, f.id, p, spawn.grid, before.as_deref(), None);
                    attach(&mut s, conn, f.id, spawn.grid);
                }
                Err(e) => {
                    log(format!("could not start a shell: {e}"));
                    if let Some(c) = s.clients.get(&conn) {
                        c.outbox.send(Frame::json(wire::EXITED, f.id, &Some(127)));
                    }
                }
            }
        }
        wire::ADOPT => {
            let (Some(adopt), Some(fd)) = (f.parse::<wire::Adopt>(), f.fd) else { return };
            if s.shells.contains_key(&f.id) {
                return;
            }
            pty::set_cloexec(fd.as_raw_fd(), true);
            log(format!("took over shell {} from Terminal", adopt.pid));
            new_shell(&mut s, f.id, pty::Pty { master: fd, pid: adopt.pid }, adopt.grid, Some(adopt.replay.as_bytes()), adopt.title);
            attach(&mut s, conn, f.id, adopt.grid);
        }
        wire::ATTACH => {
            if let Some(grid) = f.parse::<Grid>() {
                attach(&mut s, conn, f.id, grid);
            }
        }
        wire::INPUT => {
            if let Some(sh) = s.shells.get(&f.id) {
                let _ = sh.writer.send(f.data);
            }
        }
        wire::RESIZE => {
            if let Some(grid) = f.parse::<Grid>()
                && let Some(sh) = s.shells.get_mut(&f.id)
                && sh.attached.lock().unwrap().as_ref().is_some_and(|o| o.conn == conn)
            {
                sh.resize(grid);
            }
        }
        wire::CLEAR => {
            if let Some(sh) = s.shells.get(&f.id) {
                sh.term.lock().grid_mut().clear_history();
            }
        }
        wire::CLOSE => {
            // Gone from the list at once, even if something keeps the PTY
            // open after the hang-up.
            if let Some(sh) = s.shells.remove(&f.id) {
                sh.hang_up();
            }
        }
        wire::BYE => {
            if let Some(c) = s.clients.get_mut(&conn) {
                c.bye = true;
            }
        }
        _ => {}
    }
}

/// Talk to one Terminal until it goes.
fn serve(st: St, stream: UnixStream, conn: u64) {
    let (tx, rx) = mpsc::channel::<Frame>();
    let backlog = Arc::new(AtomicUsize::new(0));
    let Ok(out) = stream.try_clone() else { return };
    {
        let backlog = backlog.clone();
        std::thread::Builder::new()
            .name("client writer".into())
            .spawn(move || {
                for f in rx {
                    let n = f.data.len();
                    if wire::write(&out, &f).is_err() {
                        break;
                    }
                    backlog.fetch_sub(n, Ordering::AcqRel);
                }
                // Nobody's reading: stop holding shells back for it.
                backlog.store(0, Ordering::Release);
                let _ = out.shutdown(std::net::Shutdown::Both);
            })
            .expect("client writer");
    }
    st.lock().unwrap().clients.insert(conn, Client { outbox: Outbox { conn, tx, backlog }, pid: 0, bye: false });
    let mut reader = wire::Reader::new(stream);
    while let Ok(Some(f)) = reader.next() {
        handle(&st, conn, f);
    }
    let mut s = st.lock().unwrap();
    let gone = s.clients.remove(&conn);
    let bye = gone.as_ref().is_some_and(|c| c.bye);
    // (Gone without a goodbye: still its, while it lives.)
    let left_by = gone.map(|c| c.pid).filter(|&p| p != 0 && !bye);
    for shell in s.shells.values() {
        let mut a = shell.attached.lock().unwrap();
        if a.as_ref().is_some_and(|o| o.conn == conn) {
            *a = None;
            *shell.left_by.lock().unwrap() = left_by;
        }
    }
    let _ = s.events.send(Ev::Gone { bye });
}

// ---- starting, upgrading, ending ---------------------------------------------

/// Where SIGURG and SIGTERM land.
static SIGNALS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNALS.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe.
        unsafe { libc::write(fd, [sig as u8].as_ptr().cast(), 1) };
    }
}

/// Turn signals and the end of the desktop session into events.
fn watch(events: mpsc::Sender<Ev>) {
    let mut fds = [0; 2];
    // SAFETY: a pipe, and handlers that only write to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        SIGNALS.store(fds[1], Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        for sig in [libc::SIGURG, libc::SIGTERM, libc::SIGINT] {
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    // SAFETY: the read end is ours alone.
    let signals = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // The session's pid file may not be there yet: look again until it is.
    let find_session = || {
        std::fs::read_to_string(wire::session_pid_path())
            .ok()
            .and_then(|p| p.trim().parse::<libc::pid_t>().ok())
            // SAFETY: pidfd_open makes a new descriptor we own.
            .and_then(|pid| {
                let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
                (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
            })
    };
    let mut session = find_session();
    // Not there yet (or no session, under a test's app_server): the pid
    // file coming wakes us through the pipe (a 0, no signal's number), to
    // look again.
    let pid_file = (session.is_none()).then(|| {
        olo_kit_base::watch::watch([wire::session_pid_path()], |_| {
            let fd = SIGNALS.load(Ordering::Relaxed);
            if fd >= 0 {
                // SAFETY: writing a byte to our own pipe.
                unsafe { libc::write(fd, [0u8].as_ptr().cast(), 1) };
            }
        })
    });
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            let mut pid_file = pid_file.flatten();
            loop {
                let mut pfds = vec![libc::pollfd { fd: signals.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
                if let Some(s) = &session {
                    pfds.push(libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                }
                // SAFETY: polling our own descriptors.
                let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as _, -1) };
                if n <= 0 {
                    continue;
                }
                if pfds.get(1).is_some_and(|p| p.revents != 0) {
                    log("the session has ended");
                    let _ = events.send(Ev::End);
                    return;
                }
                let mut sig = [0u8; 1];
                // SAFETY: reading the pipe we made.
                if unsafe { libc::read(signals.as_raw_fd(), sig.as_mut_ptr().cast(), 1) } == 1 {
                    if sig[0] == 0 {
                        if session.is_none() {
                            session = find_session();
                        }
                        if session.is_some() {
                            // (Found: no more to watch for.)
                            drop(pid_file.take());
                        }
                        continue;
                    }
                    let _ = events.send(if sig[0] as i32 == libc::SIGURG { Ev::Upgrade } else { Ev::End });
                }
            }
        })
        .expect("signal thread");
}

fn set_cloexec(fd: RawFd, on: bool) {
    pty::set_cloexec(fd, on);
}

/// Our own executable, to notice when an upgrade replaces it.
struct Binary {
    path: PathBuf,
    id: Option<(u64, u64, std::time::SystemTime)>,
}

impl Binary {
    fn current() -> Binary {
        let path = std::env::current_exe().unwrap_or_default();
        let id = Self::identity(&path);
        Binary { path, id }
    }

    fn identity(path: &std::path::Path) -> Option<(u64, u64, std::time::SystemTime)> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino(), m.modified().ok()?))
    }

    fn changed(&self) -> bool {
        let now = Self::identity(&self.path);
        now.is_some() && now != self.id
    }
}

/// Become the new version, handing it the shells.
fn upgrade(st: &St, binary: &Binary, listener: &UnixListener, lock: &File) {
    let s = st.lock().unwrap();
    let mut carried = Carried { listener: listener.as_raw_fd(), lock: lock.as_raw_fd(), env: s.env.clone(), shells: Vec::new() };
    for (&id, sh) in &s.shells {
        let mut term = sh.term.lock();
        let title = sh.title.lock().unwrap().clone();
        let replay = replay::snapshot(&mut term, title.as_deref(), &sh.shared.images.lock().unwrap(), sh.shared.reports.load(Ordering::Relaxed));
        carried.shells.push(CarriedShell { id, fd: sh.master.as_raw_fd(), pid: sh.pid, grid: sh.grid, title, replay });
    }
    let fds: Vec<RawFd> = carried.shells.iter().map(|c| c.fd).chain([carried.listener, carried.lock]).collect();
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = dir.join(format!("olo-termd-{}.json", std::process::id()));
    if let Err(e) = std::fs::write(&file, serde_json::to_vec(&carried).unwrap_or_default()) {
        log(format!("could not upgrade: {e}"));
        return;
    }
    for &fd in &fds {
        set_cloexec(fd, false);
    }
    log(format!("upgraded: re-executing with {} shells", carried.shells.len()));
    let mut cmd = std::process::Command::new(&binary.path);
    olo_xdg::upgrade_env(&mut cmd);
    let e = cmd.env(RESTORE_ENV, &file).exec();
    log(format!("could not re-execute: {e}"));
    for &fd in &fds {
        set_cloexec(fd, true);
    }
    let _ = std::fs::remove_file(file);
}

/// Start Terminal again, to show the shells it left behind.
fn relaunch(env: &[(String, String)]) {
    let exe = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("olo-terminal"))).filter(|p| p.exists());
    let mut cmd = std::process::Command::new(exe.unwrap_or_else(|| "olo-terminal".into()));
    cmd.env_clear().envs(env.iter().cloned()).stdin(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(mut child) => {
            log("Terminal went away; starting it again");
            std::thread::spawn(move || child.wait());
        }
        Err(e) => log(format!("could not start Terminal: {e}")),
    }
}

/// Is the session we belong to still there to show windows in?
fn session_alive() -> bool {
    let alive = match std::fs::read_to_string(wire::session_pid_path()).ok().and_then(|p| p.trim().parse::<i32>().ok()) {
        // SAFETY: signal 0 only checks the process exists.
        Some(pid) => unsafe { libc::kill(pid, 0) == 0 },
        None => true,
    };
    alive && wire::wayland_socket().exists()
}

/// Keep the shells that have printed since they last were, on a thread
/// of its own (`busy` while it does).
fn keep_changed(st: &St, busy: &Arc<AtomicBool>) {
    let mut s = st.lock().unwrap();
    // (Before looking: what prints from now on is for the next keep.)
    PRINTED.store(false, Ordering::Release);
    let changed: Vec<_> = s
        .shells
        .iter_mut()
        .filter_map(|(&id, sh)| {
            let now = sh.output.load(Ordering::Relaxed);
            (now != std::mem::replace(&mut sh.kept, now)).then(|| (id, sh.term.clone(), sh.shared.clone()))
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    busy.store(true, Ordering::Release);
    drop(s);
    let (st, busy) = (st.clone(), busy.clone());
    std::thread::Builder::new()
        .name("keep".into())
        .spawn(move || {
            for (id, term, shared) in changed {
                keep(id, &term, &shared);
                // It ended meanwhile.
                if !st.lock().unwrap().shells.contains_key(&id) {
                    let _ = std::fs::remove_file(kept_path(id));
                }
            }
            busy.store(false, Ordering::Release);
        })
        .expect("keep thread");
}

/// Take Terminals' connections on `listener`.
fn accept(st: St, listener: &UnixListener) {
    static CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let listener = listener.try_clone().expect("listener");
    std::thread::Builder::new()
        .name("accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (st, c) = (st.clone(), CONN.fetch_add(1, Ordering::Relaxed) + 1);
                std::thread::Builder::new().name("client".into()).spawn(move || serve(st, stream, c)).expect("client thread");
            }
        })
        .expect("accept thread");
}

fn run(st: St, events: mpsc::Receiver<Ev>, mut listener: UnixListener, lock: File) -> ! {
    let binary = Binary::current();
    accept(st.clone(), &listener);
    // The socket there should be the one we listen on.
    let ino = |p: &std::path::Path| std::fs::metadata(p).ok().map(|m| std::os::unix::fs::MetadataExt::ino(&m));
    let path = wire::socket_path();
    let mut ours = ino(&path);
    let mut relaunch_at: Option<Instant> = None;
    let mut relaunched: VecDeque<Instant> = VecDeque::new();
    let mut idle_since: Option<Instant> = None;
    let mut kept_at = Instant::now();
    let keeping = Arc::new(AtomicBool::new(false));
    forget_old_kept();
    // The socket deleted or replaced, or its folder gone: from the
    // kernel, not by looking on a timer.
    let socket_events = st.lock().unwrap().events.clone();
    let _watch = olo_kit_base::watch::watch([path.clone()], move |_| {
        let _ = socket_events.send(Ev::Socket);
    });
    loop {
        let printed = PRINTED.load(Ordering::Acquire);
        if printed && kept_at.elapsed() >= KEEP_EVERY {
            // (Still keeping the last time: next time round.)
            kept_at = Instant::now();
            if !keeping.load(Ordering::Acquire) {
                keep_changed(&st, &keeping);
            }
        }
        // Asleep until something happens or is due: a keep (once a shell
        // has printed), a Terminal to start again, the linger's end.
        let due = [PRINTED.load(Ordering::Acquire).then(|| kept_at + KEEP_EVERY), relaunch_at, idle_since.map(|t| t + LINGER)].into_iter().flatten().min();
        let event = match due {
            Some(t) => events.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => events.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match event {
            Ok(Ev::Exited(id)) => {
                st.lock().unwrap().shells.remove(&id);
                // It ended on its own: nothing to bring back.
                let _ = std::fs::remove_file(kept_path(id));
            }
            Ok(Ev::Gone { bye }) => {
                if !bye {
                    relaunch_at = Some(Instant::now() + RELAUNCH_AFTER);
                }
            }
            Ok(Ev::Upgrade) => {
                if binary.changed() {
                    upgrade(&st, &binary, &listener, &lock);
                }
            }
            Ok(Ev::Restart) => upgrade(&st, &binary, &listener, &lock),
            Ok(Ev::End) => {
                let deadline = Instant::now() + Duration::from_secs(1);
                while keeping.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let s = st.lock().unwrap();
                // Terminals keep their windows as they are for next time,
                // and each shell is kept to come back in them.
                for c in s.clients.values() {
                    // (A body, so the backlog says when it has gone out.)
                    c.outbox.send(Frame::new(wire::ENDING, 0, b"end".to_vec()));
                }
                std::thread::scope(|scope| {
                    for (&id, sh) in &s.shells {
                        let (term, shared) = (&sh.term, &sh.shared);
                        scope.spawn(move || keep(id, term, shared));
                    }
                });
                let deadline = Instant::now() + Duration::from_millis(200);
                while s.clients.values().any(|c| c.outbox.backlog.load(Ordering::Acquire) > 0) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                for sh in s.shells.values() {
                    sh.hang_up();
                }
                if ino(&path) == ours {
                    let _ = std::fs::remove_file(&path);
                }
                std::process::exit(0);
            }
            Ok(Ev::Printed | Ev::Socket) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => unreachable!("we hold a sender"),
        }
        if ours.is_none() || ino(&path) != ours {
            if ours.is_some() && path.exists() {
                log("another olo-termd has our socket; ending");
                let _ = st.lock().unwrap().events.send(Ev::End);
            } else if !path.parent().is_some_and(|d| d.exists()) {
                log("the session's runtime directory has gone; ending");
                let _ = st.lock().unwrap().events.send(Ev::End);
            } else {
                // Deleted from under us: listen again, so Terminal can
                // find its shells.
                match UnixListener::bind(&path) {
                    Ok(l) => {
                        log("our socket had gone; listening again");
                        accept(st.clone(), &l);
                        listener = l;
                        ours = ino(&path);
                    }
                    Err(e) => log(format!("can't listen again: {e}")),
                }
            }
        }
        let s = st.lock().unwrap();
        let now = Instant::now();
        if !s.clients.is_empty() {
            relaunch_at = None;
            idle_since = None;
            continue;
        }
        if s.shells.is_empty() {
            let since = *idle_since.get_or_insert(now);
            if now.duration_since(since) >= LINGER {
                if ino(&path) == ours {
                    let _ = std::fs::remove_file(&path);
                }
                std::process::exit(0);
            }
            continue;
        }
        idle_since = None;
        if relaunch_at.is_some_and(|t| now >= t) {
            relaunch_at = None;
            relaunched.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            if relaunched.len() < RELAUNCHES && session_alive() {
                relaunched.push_back(now);
                relaunch(&s.env);
            }
        }
    }
}

/// Wait (a little) for the lock that says we're the one running.
fn take_lock(file: &File) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // SAFETY: flock on our own descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Who we are, for Terminal to stop us if we stop answering.
fn say_pid(lock: &mut File) {
    use std::io::Seek;
    let _ = lock.set_len(0);
    let _ = lock.rewind();
    let _ = write!(lock, "{}", std::process::id());
}

/// Become a daemon: the parent waits for the child to say it's listening
/// (or that another one is), then exits.
fn daemonize() -> OwnedFd {
    let mut fds = [0; 2];
    // SAFETY: fork before any threads; the parent only reads and exits.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            panic!("pipe: {}", std::io::Error::last_os_error());
        }
        match libc::fork() {
            -1 => panic!("fork: {}", std::io::Error::last_os_error()),
            0 => {
                libc::close(fds[0]);
                libc::setsid();
                OwnedFd::from_raw_fd(fds[1])
            }
            _ => {
                libc::close(fds[1]);
                let mut b = [0u8; 1];
                let n = libc::read(fds[0], b.as_mut_ptr().cast(), 1);
                libc::_exit(if n == 1 { 0 } else { 1 });
            }
        }
    }
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let carried: Option<Carried> = std::env::var_os(RESTORE_ENV).and_then(|path| {
        let text = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);
        // SAFETY: nothing else reads the environment yet.
        unsafe { std::env::remove_var(RESTORE_ENV) };
        serde_json::from_slice(&text?).ok()
    });
    let (events_tx, events) = mpsc::channel();
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let st: St = Arc::new(Mutex::new(State { shells: HashMap::new(), clients: HashMap::new(), seq: 0, env: Vec::new(), events: events_tx.clone(), started }));
    // Shells start where Terminal says; we sit in the home directory.
    if let Some(home) = std::env::var_os("HOME") {
        let _ = std::env::set_current_dir(home);
    }
    let (listener, lock) = match carried {
        Some(c) => {
            set_cloexec(c.listener, true);
            set_cloexec(c.lock, true);
            // SAFETY: our previous self passed these to us alone.
            let (listener, mut lock) = unsafe { (UnixListener::from_raw_fd(c.listener), File::from_raw_fd(c.lock)) };
            say_pid(&mut lock);
            let mut s = st.lock().unwrap();
            s.env = c.env;
            for sh in c.shells {
                match pty::Pty::adopt(sh.fd, sh.pid) {
                    Ok(p) => new_shell(&mut s, sh.id, p, sh.grid, Some(&sh.replay), sh.title),
                    Err(e) => log(format!("lost shell {}: {e}", sh.id)),
                }
            }
            log(format!("upgraded, with {} shells", s.shells.len()));
            (listener, lock)
        }
        None => {
            let ready = daemonize();
            let mut lock = File::options().create(true).truncate(false).write(true).open(wire::lock_path()).expect("lock file");
            if !take_lock(&lock) {
                // Another one is running.
                let _ = File::from(ready).write_all(b"1");
                return;
            }
            say_pid(&mut lock);
            let path = wire::socket_path();
            let _ = std::fs::remove_file(&path);
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    log(format!("could not listen on {}: {e}", path.display()));
                    std::process::exit(1);
                }
            };
            let _ = File::from(ready).write_all(b"1");
            (listener, lock)
        }
    };
    // Keep stdin from holding anything open.
    if let Ok(null) = File::open("/dev/null") {
        // SAFETY: replacing our stdin with /dev/null.
        unsafe { libc::dup2(null.as_raw_fd(), 0) };
    }
    watch(events_tx);
    run(st, events, listener, lock);
}
