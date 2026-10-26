//! app_server side of the Tabula IPC (see the tabula-protocol crate):
//! newline-delimited JSON over a Unix socket, one reply per request, plus
//! events for subscribed connections.

use std::{
    cell::RefCell,
    io::{ErrorKind, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    rc::Rc,
};

use smithay::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction,
    generic::{Generic, NoIoDrop},
};
use tabula_protocol::{Event, Reply, Request};
use tracing::{debug, info, warn};

use crate::state::{AnvilState, Backend};

/// One client connection. Writes are blocking; replies are small.
pub struct Conn {
    stream: UnixStream,
    dead: RefCell<bool>,
}

impl Conn {
    pub fn send(&self, reply: &Reply) {
        if *self.dead.borrow() {
            return;
        }
        let mut line = serde_json::to_vec(reply).expect("serialize reply");
        line.push(b'\n');
        let mut s = &self.stream;
        if let Err(err) = s.write_all(&line) {
            debug!(?err, "ipc client went away");
            *self.dead.borrow_mut() = true;
        }
    }

    pub fn is_dead(&self) -> bool {
        *self.dead.borrow()
    }
}

#[derive(Default)]
pub struct IpcState {
    pub path: PathBuf,
    subscribers: Vec<Rc<Conn>>,
    /// Screenshot requests the backend fulfils after its next render.
    pub pending_screenshots: Vec<(PathBuf, Rc<Conn>)>,
}

impl std::fmt::Debug for IpcState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcState").field("path", &self.path).finish_non_exhaustive()
    }
}

impl IpcState {
    pub fn broadcast(&mut self, event: Event) {
        self.subscribers.retain(|c| !c.is_dead());
        let reply = Reply::Event { event };
        for c in &self.subscribers {
            c.send(&reply);
        }
    }
}

impl Drop for IpcState {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Open the socket and register it with the event loop. Sets
/// `$TABULA_SOCKET` for every process app_server starts afterwards.
pub fn init<B: Backend + 'static>(
    handle: &LoopHandle<'static, AnvilState<B>>,
    wayland_socket: &str,
) -> std::io::Result<IpcState> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("tabula-{wayland_socket}.sock"));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    // SAFETY: single-threaded; nothing else reads the environment concurrently.
    unsafe { std::env::set_var(tabula_protocol::SOCKET_ENV, &path) };
    info!(path = %path.display(), "IPC listening");

    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => accept(state, stream),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) => {
                            warn!(?e, "IPC accept failed");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    Ok(IpcState { path, subscribers: Vec::new(), pending_screenshots: Vec::new() })
}

fn accept<B: Backend + 'static>(state: &mut AnvilState<B>, stream: UnixStream) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let _ = read_half.set_nonblocking(true);
    let conn = Rc::new(Conn { stream, dead: RefCell::new(false) });
    let mut buf = Vec::<u8>::new();
    let res = state.handle.insert_source(
        Generic::new(read_half, Interest::READ, Mode::Level),
        move |_, stream: &mut NoIoDrop<UnixStream>, state| {
            let mut chunk = [0u8; 4096];
            loop {
                // SAFETY: we only read; the fd stays registered until we return Remove.
                let s = unsafe { stream.get_mut() };
                match s.read(&mut chunk) {
                    Ok(0) => {
                        *conn.dead.borrow_mut() = true;
                        return Ok(PostAction::Remove);
                    }
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        *conn.dead.borrow_mut() = true;
                        return Ok(PostAction::Remove);
                    }
                }
            }
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Request>(line) {
                    Ok(req) => {
                        if let Some(reply) = state.handle_ipc(req, &conn) {
                            conn.send(&reply);
                        }
                    }
                    Err(e) => conn.send(&Reply::Error { message: format!("bad request: {e}") }),
                }
            }
            Ok(if conn.is_dead() { PostAction::Remove } else { PostAction::Continue })
        },
    );
    if let Err(e) = res {
        warn!(?e, "could not register IPC client");
    }
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Handle one request. `None` means the reply is sent later (screenshots).
    pub fn handle_ipc(&mut self, req: Request, conn: &Rc<Conn>) -> Option<Reply> {
        let not_found = || Reply::Error { message: "no such window".into() };
        Some(match req {
            Request::Ping => Reply::Pong { version: env!("CARGO_PKG_VERSION").into() },
            Request::Subscribe => {
                self.ipc.subscribers.push(conn.clone());
                Reply::Ok
            }
            Request::ListWindows => Reply::Windows { windows: self.window_infos() },
            Request::Activate { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.restore_window(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::Minimize { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.minimize_window(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::Close { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.close_window(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::ToggleZoom { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.toggle_zoom(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::Workspaces => Reply::Workspaces {
                current: self.workspace,
                count: self.settings.workspace_count,
            },
            Request::SwitchWorkspace { index } => {
                self.switch_workspace(index);
                Reply::Ok
            }
            Request::MoveToWorkspace { id, index } => match self.window_by_id(id) {
                Some(w) => {
                    self.move_to_workspace(&w, index);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::Outputs => Reply::Outputs { outputs: self.output_infos() },
            Request::ConfigureOutput { name, config } => match self.configure_output(&name, config) {
                Ok(()) => Reply::Ok,
                Err(message) => Reply::Error { message },
            },
            Request::GetSettings => Reply::Settings { settings: self.settings.clone() },
            Request::SetSettings { settings } => {
                self.set_settings(settings);
                Reply::Ok
            }
            Request::Screenshot { path } => {
                self.ipc.pending_screenshots.push((PathBuf::from(path), conn.clone()));
                return None;
            }
            Request::Quit => {
                self.running.store(false, std::sync::atomic::Ordering::SeqCst);
                Reply::Ok
            }
        })
    }
}
