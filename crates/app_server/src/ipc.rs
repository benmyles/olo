//! app_server side of the Olo IPC (see the olo-protocol crate):
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
use olo_protocol::{Event, Reply, Request};
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

    /// Whether the process at the other end is ours, running Olo's
    /// program `name` from beside app_server (the same user, by
    /// `SO_PEERCRED`, and its executable).
    pub fn peer_is_ours(&self, name: &str) -> bool {
        use std::os::fd::AsRawFd;
        // SAFETY: as in `peer`.
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let got = unsafe {
            libc::getsockopt(self.stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len)
        };
        // SAFETY: getuid can't fail.
        if got != 0 || cred.uid != unsafe { libc::getuid() } {
            return false;
        }
        let dir = |p: std::path::PathBuf| {
            let s = p.to_string_lossy().into_owned();
            std::path::PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s))
        };
        let (Ok(theirs), Ok(mine)) = (std::fs::read_link(format!("/proc/{}/exe", cred.pid)), std::env::current_exe()) else { return false };
        let (theirs, mine) = (dir(theirs), dir(mine));
        theirs.file_name().is_some_and(|n| n == name) && theirs.parent().is_some() && theirs.parent() == mine.parent()
    }

    /// The process at the other end, as "name (pid)", for the log.
    pub fn peer(&self) -> String {
        use std::os::fd::AsRawFd;
        // SAFETY: an all-zero ucred is valid, and getsockopt writes at
        // most `len` bytes into it.
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let got = unsafe {
            libc::getsockopt(
                self.stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &mut len,
            )
        };
        if got != 0 {
            return "unknown".into();
        }
        let name = std::fs::read_to_string(format!("/proc/{}/comm", cred.pid)).unwrap_or_default();
        format!("{} ({})", name.trim(), cred.pid)
    }
}

#[derive(Default)]
pub struct IpcState {
    pub path: PathBuf,
    /// The socket file, when we made it (not olo-session).
    _file: Option<crate::sockfile::Owned>,
    subscribers: Vec<Rc<Conn>>,
    /// Pictures to take with the next frame (see screenshot.rs).
    pub pending_screenshots: Vec<crate::screenshot::Shot>,
    /// Screenshots being written on threads, by number, and who to tell.
    saving: std::collections::HashMap<u64, Rc<Conn>>,
    next_save: u64,
    saved: Option<smithay::reexports::calloop::channel::Sender<(u64, Result<(), String>)>>,
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

/// Open the socket and register it with the event loop. Sets
/// `$OLO_SOCKET` for every process app_server starts afterwards.
pub fn init<B: Backend + 'static>(
    handle: &LoopHandle<'static, AnvilState<B>>,
    wayland_socket: &str,
) -> std::io::Result<IpcState> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let mut path = dir.join(format!("olo-{wayland_socket}.sock"));
    // olo-session may own the socket (so it survives our restarts), under
    // its old name if the session started on a build from before the
    // rename (Tabula).
    let (listener, owned) = match inherited_ipc_socket() {
        Some(l) => {
            path = olo_xdg::legacy::runtime_file(&dir, &format!("olo-{wayland_socket}.sock"));
            (l, false)
        }
        None => {
            let _ = std::fs::remove_file(&path);
            let l = UnixListener::bind(&path)?;
            (l, true)
        }
    };
    listener.set_nonblocking(true)?;
    // SAFETY: single-threaded; nothing else reads the environment concurrently.
    unsafe { std::env::set_var(olo_protocol::SOCKET_ENV, &path) };
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

    // An inherited socket's file belongs to olo-session: don't remove it.
    let file = owned.then(|| crate::sockfile::Owned::new(&path));
    let path = if owned { path } else { PathBuf::new() };
    let (saved, done) = smithay::reexports::calloop::channel::channel::<(u64, Result<(), String>)>();
    handle
        .insert_source(done, |event, _, state| {
            if let smithay::reexports::calloop::channel::Event::Msg((id, res)) = event
                && let Some(conn) = state.ipc.saving.remove(&id)
            {
                conn.send(&match res {
                    Ok(()) => Reply::Ok,
                    Err(message) => Reply::Error { message },
                });
            }
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(IpcState {
        path,
        _file: file,
        subscribers: Vec::new(),
        pending_screenshots: Vec::new(),
        saving: Default::default(),
        next_save: 0,
        saved: Some(saved),
    })
}

impl IpcState {
    /// Number a screenshot being saved, remembering who to answer; the
    /// saver sends `(number, result)` down the channel returned.
    #[allow(clippy::type_complexity)]
    pub fn saving(&mut self, conn: Option<Rc<Conn>>) -> (u64, Option<smithay::reexports::calloop::channel::Sender<(u64, Result<(), String>)>>) {
        let id = self.next_save;
        self.next_save += 1;
        if let Some(conn) = conn {
            self.saving.insert(id, conn);
        }
        (id, self.saved.clone())
    }
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
                        let start = std::time::Instant::now();
                        if let Some(reply) = state.handle_ipc(req, &conn) {
                            conn.send(&reply);
                        }
                        state.timings.handled(|| format!("request {}", serde_json::from_str::<serde_json::Value>(line).ok().and_then(|v| v["type"].as_str().map(str::to_owned)).unwrap_or_default()), start);
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
            Request::ListWindows => Reply::Windows { windows: self.window_infos_with_frames() },
            Request::ActivationToken => Reply::ActivationToken { token: self.issue_activation_token() },
            Request::Activate { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.restore_window(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::ActivateAll { ids } => {
                let mut windows: Vec<_> = ids.into_iter().filter_map(|id| self.window_by_id(id)).collect();
                // The one used last comes forward last, in front.
                windows.sort_by_key(|w| std::cmp::Reverse(self.recency(w.id())));
                for w in &windows {
                    self.restore_window(w);
                }
                if windows.is_empty() {
                    not_found()
                } else {
                    Reply::Ok
                }
            }
            Request::Minimize { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.minimize_window(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::Close { id } => match self.window_by_id(id) {
                Some(w) => {
                    // Apps may ask before closing; this says who wanted it.
                    info!(id, title = %w.title(), by = %conn.peer(), "asked to close a window");
                    self.close_window(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::ToggleZoom { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.front_of_group(&w);
                    self.toggle_zoom(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::SendToBack { id } => match self.window_by_id(id) {
                Some(w) => {
                    self.send_to_back(&w);
                    Reply::Ok
                }
                None => not_found(),
            },
            Request::GroupWindow { id, with, index } => match (self.window_by_id(id), self.window_by_id(with)) {
                (Some(w), Some(t)) if self.space.element_location(&w).is_some() && self.space.element_location(&t).is_some() => {
                    if self.put_in_group(&w, &t, index.map(|i| i as usize)) {
                        self.focus_window(&w);
                        Reply::Ok
                    } else {
                        Reply::Error { message: format!("window {id} can't be grouped with {with} (one is a dialog, a fixed-size panel or fullscreen, or they are the same window)") }
                    }
                }
                (Some(_), Some(_)) => Reply::Error { message: "both windows must be showing".into() },
                _ => not_found(),
            },
            Request::UngroupWindow { id } => match self.window_by_id(id) {
                Some(w) if self.ungroup_window(&w) => Reply::Ok,
                Some(_) => Reply::Error { message: format!("window {id} isn't in a group") },
                None => not_found(),
            },
            Request::MoveTab { id, index } => match self.window_by_id(id) {
                Some(w) if self.move_tab(&w, index as usize) => Reply::Ok,
                Some(_) => Reply::Error { message: format!("window {id} isn't in a group") },
                None => not_found(),
            },
            Request::Arrangement => match self.arrangement() {
                Some((output, area, windows)) => {
                    let infos = self.window_infos_with_frames();
                    let windows = windows.iter().filter_map(|w| infos.iter().find(|i| i.id == w.meta().id).cloned()).collect();
                    let area = olo_protocol::Frame { x: area.loc.x, y: area.loc.y, width: area.size.w, height: area.size.h };
                    Reply::Arrangement { output: output.name(), area, windows }
                }
                None => Reply::Error { message: "no display to arrange".into() },
            },
            Request::Arrange { windows } => match {
                self.arranging(None);
                self.arrange(&windows)
            } {
                0 => Reply::Error { message: "none of those windows are on the active display".into() },
                _ => {
                    self.readopt_active();
                    Reply::Ok
                }
            },
            Request::TileApp { app_id } => {
                self.arranging(None);
                if self.tile_app(&app_id) {
                    self.readopt_active();
                    Reply::Ok
                } else {
                    Reply::Error { message: format!("no windows of {app_id} to tile") }
                }
            }
            Request::Place { id, frame } => match self.window_by_id(id) {
                Some(w) if self.space.element_location(&w).is_some() => {
                    let size = (frame.width.max(1), frame.height.max(1));
                    // As if moved and sized by hand: its own place now,
                    // where it can be taken hold of again.
                    let to = self.reachable(smithay::utils::Rectangle::new((frame.x, frame.y).into(), size.into()));
                    self.front_of_group(&w);
                    // Auto-tiled: it floats where it is put.
                    if self.auto_tiled(&w) {
                        self.set_floating(&w, Some(true));
                    }
                    self.forget_tile(&w);
                    self.set_frame(&w, to, false);
                    Reply::Ok
                }
                Some(_) => Reply::Error { message: format!("window {id} isn't showing") },
                None => not_found(),
            },
            Request::WindowingMode { workspace } => {
                let ws = workspace.unwrap_or(self.workspace);
                Reply::WindowingMode { workspace: ws, displays: self.windowing_mode_infos(ws) }
            }
            Request::SetWindowingMode { mode, workspace, output } => {
                let o = match output {
                    Some(name) => match self.space.outputs().find(|o| o.name() == name).cloned() {
                        Some(o) => Some(o),
                        None => return Some(Reply::Error { message: format!("no display {name:?}") }),
                    },
                    None => None,
                };
                let ws = workspace.unwrap_or(self.workspace);
                self.set_windowing_mode(mode, Some(ws), o);
                Reply::WindowingMode { workspace: ws, displays: self.windowing_mode_infos(ws) }
            }
            Request::AutoTiling { workspace } => {
                let ws = workspace.unwrap_or(self.workspace);
                Reply::AutoTiling { workspace: ws, displays: self.auto_tiling_infos(ws) }
            }
            Request::SetAutoTiling { on, workspace, output } => {
                let o = match output {
                    Some(name) => match self.space.outputs().find(|o| o.name() == name).cloned() {
                        Some(o) => Some(o),
                        None => return Some(Reply::Error { message: format!("no display {name:?}") }),
                    },
                    None => None,
                };
                let ws = workspace.unwrap_or(self.workspace);
                self.set_auto_tiling(on, Some(ws), o);
                Reply::AutoTiling { workspace: ws, displays: self.auto_tiling_infos(ws) }
            }
            Request::SetFloating { id, floating } => match self.window_by_id(id) {
                Some(w) => {
                    self.set_floating(&w, floating);
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
            Request::InputDevices => Reply::InputDevices { devices: self.backend_data.input_devices() },
            Request::PowerStatus => Reply::Power { status: self.power_status() },
            Request::Lock => {
                self.lock_screen();
                Reply::Ok
            }
            Request::SetCaffeinated { on, minutes, lid } => {
                self.set_caffeinated(on, minutes, lid);
                Reply::Ok
            }
            Request::SetAutoApprove { minutes, password } => match self.set_auto_approve(minutes, password, conn.clone()) {
                Some(reply) => reply,
                None => return None,
            },
            Request::AutoApproveSecret => self.auto_approve_secret(&conn),
            Request::SetBrightness { level } => {
                self.set_brightness(level);
                Reply::Ok
            }
            Request::SetKeyboardBacklight { level } => {
                self.set_keyboard_backlight(level);
                Reply::Ok
            }
            Request::IdentifyOutputs => {
                for info in self.output_infos() {
                    if let Some(o) = self.space.outputs().find(|o| o.name() == info.name) {
                        let detail = match &info.current_mode {
                            Some(m) => format!("{} · {} × {}", info.name, m.width, m.height),
                            None => info.name.clone(),
                        };
                        crate::identify::show(o, &info.display_name(), &detail);
                    }
                }
                Reply::Ok
            }
            Request::ConfigureOutput { name, config } => match self.configure_output(&name, config) {
                Ok(()) => Reply::Ok,
                Err(message) => Reply::Error { message },
            },
            Request::RunShortcut { id, window } => match olo_protocol::shortcuts::by_id(&id) {
                Some(info) => {
                    let target = window.and_then(|w| self.window_by_id(w));
                    self.run_shortcut(info.action, target);
                    Reply::Ok
                }
                None => Reply::Error { message: format!("no shortcut {id:?}") },
            },
            Request::RecordShortcut { on } => {
                self.record_shortcut(on);
                Reply::Ok
            }
            Request::SetOutputBrightness { name, level } => match self.set_output_brightness(&name, level) {
                Ok(()) => Reply::Ok,
                Err(message) => Reply::Error { message },
            },
            Request::GetSettings => Reply::Settings { settings: self.settings.clone() },
            Request::Looks => Reply::Looks { themes: olo_protocol::LookInfo::all(), palette: (*crate::theme::palette()).clone() },
            Request::Palette { theme, colors, mode } => {
                let a = &self.settings.appearance;
                let palette = olo_theme::looks::palette(
                    theme.as_deref().unwrap_or(&a.icon_theme),
                    colors.as_deref().unwrap_or(&a.colors),
                    mode.unwrap_or(a.effective_theme),
                );
                Reply::Palette { palette }
            }
            Request::Hints => Reply::Hints { hints: self.hints.infos() },
            Request::Overview { show } => {
                match show {
                    Some(true) => self.open_overview(),
                    Some(false) => self.close_overview(None),
                    None => {}
                }
                Reply::Overview { overview: self.overview_info() }
            }
            Request::SetSettings { settings } => {
                self.set_settings(settings);
                Reply::Ok
            }
            Request::Synthetic { events } => {
                for e in events {
                    self.synthetic_input(e);
                }
                Reply::Ok
            }
            Request::StartScreenSaver => {
                self.start_screen_saver(true);
                Reply::Ok
            }
            Request::Screenshot { path, output, window, border, pointer, format } => {
                let shot = crate::screenshot::Shot {
                    path: PathBuf::from(path),
                    conn: Some(conn.clone()),
                    output,
                    window,
                    border,
                    pointer,
                    format,
                };
                match self.request_screenshot(shot) {
                    Ok(()) => return None,
                    Err(message) => Reply::Error { message },
                }
            }
            Request::Timings => Reply::Timings { timings: self.take_timings() },
            Request::Quit => {
                self.running.store(false, std::sync::atomic::Ordering::SeqCst);
                Reply::Ok
            }
        })
    }
}

fn inherited_ipc_socket() -> Option<UnixListener> {
    use std::os::fd::FromRawFd;
    let fd: i32 = std::env::var("OLO_IPC_FD").ok()?.parse().ok()?;
    // SAFETY: olo-session passes this fd to us alone.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    crate::set_cloexec(fd);
    // SAFETY: called during single-threaded start-up.
    unsafe { std::env::remove_var("OLO_IPC_FD") };
    Some(listener)
}
