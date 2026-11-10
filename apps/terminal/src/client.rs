//! Terminal's side of olo-termd (termd.rs), which runs the shells.
//!
//! One connection per Terminal, shared by its windows: a thread reads what
//! comes back and hands each shell's part to that shell's own thread
//! (session.rs), and a thread writes, so the UI never waits on the
//! socket. If the connection drops (olo-termd upgrading), it connects
//! again and re-attaches every shell.

use std::{
    collections::HashMap,
    io,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::wire::{self, Frame, Grid};

/// What a shell's thread gets.
pub enum Chunk {
    /// Attached (again): start over from `replay`.
    Attached {
        pid: i32,
        pty: Option<OwnedFd>,
        replay: Vec<u8>,
    },
    Output(Vec<u8>),
    Exited(Option<i32>),
    /// Another Terminal has it now.
    Detached,
}

struct Entry {
    tx: mpsc::Sender<Chunk>,
    grid: Grid,
}

enum Out {
    Frame(Frame),
    /// Answer once everything before has been written.
    Flush(mpsc::Sender<()>),
}

struct Inner {
    shells: Mutex<HashMap<u64, Entry>>,
    out: Mutex<mpsc::Sender<Out>>,
    /// The connection the writer writes to; None while reconnecting.
    stream: Mutex<Option<UnixStream>>,
    reconnected: Condvar,
    leaving: AtomicBool,
    /// olo-termd said the session is ending (`wire::ENDING`).
    ended: AtomicBool,
    /// …but we stopped it ourselves, to start over.
    starting_over: AtomicBool,
    next: AtomicU64,
}

/// The connection, for the windows.
#[derive(Clone)]
pub struct Link(Arc<Inner>);

/// What a shell writes with (input, and answers to its program).
#[derive(Clone)]
pub struct Writer {
    pub id: u64,
    link: Link,
}

impl Writer {
    pub fn send(&self, bytes: Vec<u8>) {
        self.link.input(self.id, bytes);
    }
}

fn hello() -> Frame {
    let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !matches!(k.as_str(), "XDG_ACTIVATION_TOKEN" | "DESKTOP_STARTUP_ID" | "OLO_RESTORE")).collect();
    crate::sudo::shell_env(&mut env);
    Frame::json(wire::HELLO, 0, &wire::Hello { pid: std::process::id() as i32, env })
}

/// Start olo-termd (it returns once it's listening).
fn start_daemon() {
    let exe = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("olo-termd"))).filter(|p| p.exists());
    let program = exe.unwrap_or_else(|| PathBuf::from("olo-termd"));
    if let Err(e) = std::process::Command::new(&program).stdin(std::process::Stdio::null()).status() {
        tracing::error!(?e, program = %program.display(), "could not start olo-termd");
    }
}

/// Connect, starting olo-termd if it isn't running; keep trying for
/// `patience`.
fn dial(patience: Duration) -> io::Result<UnixStream> {
    let deadline = Instant::now() + patience;
    let mut started = false;
    loop {
        match UnixStream::connect(wire::socket_path()) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) if !started => {
                start_daemon();
                started = true;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Connect, and find out what shells there are (before any window opens).
pub fn connect() -> io::Result<(Link, Vec<wire::ShellInfo>)> {
    let stream = dial(Duration::from_secs(5))?;
    wire::write(&stream, &hello())?;
    wire::write(&stream, &Frame::new(wire::LIST, 0, Vec::new()))?;
    let mut reader = wire::Reader::new(stream.try_clone()?);
    let shells = loop {
        match reader.next()? {
            Some(f) if f.kind == wire::SHELLS => break f.parse::<Vec<wire::ShellInfo>>().unwrap_or_default(),
            Some(_) => continue,
            None => return Err(io::ErrorKind::UnexpectedEof.into()),
        }
    };
    let (tx, rx) = mpsc::channel();
    let seed = (std::process::id() as u64) << 32 ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    let link = Link(Arc::new(Inner {
        shells: Mutex::new(HashMap::new()),
        out: Mutex::new(tx),
        stream: Mutex::new(Some(stream)),
        reconnected: Condvar::new(),
        leaving: AtomicBool::new(false),
        ended: AtomicBool::new(false),
        starting_over: AtomicBool::new(false),
        next: AtomicU64::new(seed),
    }));
    {
        let link = link.clone();
        std::thread::Builder::new().name("termd writer".into()).spawn(move || link.write_loop(rx)).expect("writer thread");
    }
    {
        let link = link.clone();
        std::thread::Builder::new().name("termd reader".into()).spawn(move || link.read_loop(reader)).expect("reader thread");
    }
    Ok((link, shells))
}

impl Link {
    fn send(&self, f: Frame) {
        let _ = self.0.out.lock().unwrap().send(Out::Frame(f));
    }

    /// A new shell's id.
    pub fn new_id(&self) -> u64 {
        // Spread out, so two Terminals' never meet.
        self.0.next.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1
    }

    pub fn writer(&self, id: u64) -> Writer {
        Writer { id, link: self.clone() }
    }

    fn register(&self, id: u64, tx: mpsc::Sender<Chunk>, grid: Grid) {
        self.0.shells.lock().unwrap().insert(id, Entry { tx, grid });
    }

    pub fn spawn(&self, id: u64, tx: mpsc::Sender<Chunk>, spawn: wire::Spawn) {
        self.register(id, tx, spawn.grid);
        self.send(Frame::json(wire::SPAWN, id, &spawn));
    }

    /// Hand olo-termd a shell we ran ourselves (before there was one).
    pub fn adopt(&self, id: u64, tx: mpsc::Sender<Chunk>, adopt: wire::Adopt, pty: OwnedFd) {
        self.register(id, tx, adopt.grid);
        let mut f = Frame::json(wire::ADOPT, id, &adopt);
        f.fd = Some(pty);
        self.send(f);
    }

    pub fn attach(&self, id: u64, tx: mpsc::Sender<Chunk>, grid: Grid) {
        self.register(id, tx, grid);
        self.send(Frame::json(wire::ATTACH, id, &grid));
    }

    pub fn input(&self, id: u64, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.send(Frame::new(wire::INPUT, id, bytes));
        }
    }

    pub fn resize(&self, id: u64, grid: Grid) {
        if let Some(e) = self.0.shells.lock().unwrap().get_mut(&id) {
            e.grid = grid;
        }
        self.send(Frame::json(wire::RESIZE, id, &grid));
    }

    pub fn clear(&self, id: u64) {
        self.send(Frame::new(wire::CLEAR, id, Vec::new()));
    }

    /// The window has closed: hang up its shell.
    pub fn close(&self, id: u64) {
        self.forget(id);
        self.send(Frame::new(wire::CLOSE, id, Vec::new()));
    }

    /// Stop showing a shell (it's gone, or we are).
    pub fn forget(&self, id: u64) {
        self.0.shells.lock().unwrap().remove(&id);
    }

    /// The session is ending: the windows stay as they were for next
    /// time, their shells going with it.
    pub fn ended(&self) -> bool {
        self.0.ended.load(Ordering::Acquire)
    }

    /// Terminal has said goodbye (`bye`: quitting), or given up on
    /// olo-termd: what it kept of its windows just before stays as it is.
    pub fn leaving(&self) -> bool {
        self.0.leaving.load(Ordering::Acquire)
    }

    /// Ending olo-termd to start it afresh: that isn't the session ending.
    pub fn starting_over(&self) {
        self.0.starting_over.store(true, Ordering::Release);
    }

    /// Quitting: leave the shells running, and say so, so olo-termd
    /// doesn't start Terminal again. Waits (briefly) for what's queued to
    /// go out first.
    pub fn bye(&self) {
        self.0.leaving.store(true, Ordering::Release);
        self.send(Frame::new(wire::BYE, 0, Vec::new()));
        let (tx, rx) = mpsc::channel();
        let _ = self.0.out.lock().unwrap().send(Out::Flush(tx));
        let _ = rx.recv_timeout(Duration::from_millis(500));
    }

    fn write_loop(&self, rx: mpsc::Receiver<Out>) {
        for out in rx {
            let f = match out {
                Out::Frame(f) => f,
                Out::Flush(done) => {
                    let _ = done.send(());
                    continue;
                }
            };
            let mut stream = self.0.stream.lock().unwrap();
            loop {
                if let Some(s) = stream.as_ref() {
                    if wire::write(s, &f).is_ok() {
                        break;
                    }
                    *stream = None;
                }
                // Wait for the reader to connect again (or give up).
                if self.0.leaving.load(Ordering::Acquire) {
                    break;
                }
                let (s, timeout) = self.0.reconnected.wait_timeout(stream, Duration::from_secs(10)).unwrap();
                stream = s;
                if timeout.timed_out() && stream.is_none() {
                    break;
                }
            }
        }
    }

    fn read_loop(&self, mut reader: wire::Reader) {
        loop {
            while let Ok(Some(f)) = reader.next() {
                self.dispatch(f);
            }
            if self.ended() {
                // Not connecting again (that would start olo-termd anew
                // as the session goes): the shells have gone.
                self.0.leaving.store(true, Ordering::Release);
                self.0.reconnected.notify_all();
                for (_, e) in self.0.shells.lock().unwrap().drain() {
                    let _ = e.tx.send(Chunk::Exited(None));
                }
                return;
            }
            if self.0.leaving.load(Ordering::Acquire) {
                return;
            }
            match self.reconnect() {
                Some(r) => reader = r,
                None => {
                    tracing::error!("lost olo-termd");
                    self.0.leaving.store(true, Ordering::Release);
                    self.0.reconnected.notify_all();
                    for (_, e) in self.0.shells.lock().unwrap().drain() {
                        let _ = e.tx.send(Chunk::Exited(None));
                    }
                    return;
                }
            }
        }
    }

    /// Connect again and pick up every shell where it is.
    fn reconnect(&self) -> Option<wire::Reader> {
        let mut stream = self.0.stream.lock().unwrap();
        *stream = None;
        let s = dial(Duration::from_secs(5)).ok()?;
        wire::write(&s, &hello()).ok()?;
        for (&id, e) in self.0.shells.lock().unwrap().iter() {
            wire::write(&s, &Frame::json(wire::ATTACH, id, &e.grid)).ok()?;
        }
        let reader = wire::Reader::new(s.try_clone().ok()?);
        *stream = Some(s);
        self.0.reconnected.notify_all();
        tracing::info!("reconnected to olo-termd");
        Some(reader)
    }

    fn dispatch(&self, mut f: Frame) {
        if f.kind == wire::ENDING {
            if !self.0.starting_over.swap(false, Ordering::AcqRel) {
                self.0.ended.store(true, Ordering::Release);
            }
            return;
        }
        let chunk = match f.kind {
            wire::ATTACHED if f.data.len() >= 4 => {
                let pid = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                f.data.drain(..4);
                Chunk::Attached { pid, pty: f.fd.take(), replay: f.data }
            }
            wire::OUTPUT => Chunk::Output(f.data),
            wire::EXITED => Chunk::Exited(f.parse::<Option<i32>>().flatten()),
            wire::DETACHED => Chunk::Detached,
            _ => return,
        };
        let mut shells = self.0.shells.lock().unwrap();
        let Some(e) = shells.get(&f.id) else { return };
        let last = matches!(chunk, Chunk::Exited(_) | Chunk::Detached);
        let _ = e.tx.send(chunk);
        if last {
            shells.remove(&f.id);
        }
    }
}

// ---- looking after olo-termd (Terminal ▸ Terminal Daemon…) -------------

/// A short connection of our own, to ask or tell olo-termd something
/// (not the shared one: this mustn't look like a Terminal attaching).
fn ask(kind: u8) -> Result<UnixStream, String> {
    let s = UnixStream::connect(wire::socket_path()).map_err(|_| "Not running.".to_owned())?;
    let limit = Some(Duration::from_secs(2));
    let _ = s.set_read_timeout(limit);
    let _ = s.set_write_timeout(limit);
    wire::write(&s, &Frame::new(kind, 0, Vec::new())).map_err(|_| "Not responding.".to_owned())?;
    Ok(s)
}

/// How olo-termd is. (Blocks for up to a couple of seconds: call it
/// off the UI thread.)
pub fn status() -> Result<wire::Status, String> {
    let s = ask(wire::STATUS)?;
    let mut reader = wire::Reader::new(s.try_clone().map_err(|e| e.to_string())?);
    let status = loop {
        match reader.next() {
            Ok(Some(f)) if f.kind == wire::REPORT => break f.parse::<wire::Status>().ok_or("Not responding.")?,
            Ok(Some(_)) => continue,
            _ => return Err("Not responding.".into()),
        }
    };
    let _ = wire::write(&s, &Frame::new(wire::BYE, 0, Vec::new()));
    Ok(status)
}

/// Have olo-termd re-execute itself; the shells carry on.
pub fn restart() -> Result<(), String> {
    let s = ask(wire::RESTART)?;
    let _ = wire::write(&s, &Frame::new(wire::BYE, 0, Vec::new()));
    Ok(())
}

/// Start olo-termd if it isn't running.
pub fn start() -> Result<(), String> {
    dial(Duration::from_secs(5)).map(drop).map_err(|e| format!("Couldn't start it: {e}."))
}

/// End olo-termd, however it is (and with it, the shells). Its pid
/// is in its lock file (or, from its last status, `known`).
/// Forget the shells olo-termd kept (when it was stopped to start over).
pub fn forget_kept() {
    if let Ok(dir) = std::fs::read_dir(wire::kept_dir()) {
        for e in dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("shell-")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub fn stop(known: Option<i32>) -> Result<(), String> {
    // (`tabula-termd` if it started before the rename and hasn't upgraded.)
    let ours = |pid: &i32| std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| matches!(c.trim(), "olo-termd" | "tabula-termd"));
    let Some(pid) = std::fs::read_to_string(wire::lock_path()).ok().and_then(|p| p.trim().parse::<i32>().ok()).filter(ours).or(known.filter(ours)) else {
        // Nothing to stop.
        return Ok(());
    };
    let gone = || std::fs::read_to_string(format!("/proc/{pid}/comm")).map_or(true, |c| !matches!(c.trim(), "olo-termd" | "tabula-termd"));
    // SAFETY: signalling our own olo-termd.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while !gone() {
        if Instant::now() >= deadline {
            // SAFETY: as above; it didn't listen.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
