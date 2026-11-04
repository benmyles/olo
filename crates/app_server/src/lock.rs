//! The lock screen (ext-session-lock-v1), with app_server as the security
//! boundary:
//!
//! * Locking takes effect here first: from `lock_screen` on, outputs show
//!   only lock surfaces over an opaque background and no other client gets
//!   input, whether or not the lock screen has drawn anything yet.
//! * The lock screen (`olo-lock`) is started by app_server on a socket
//!   it creates, so app_server knows its client; the lock global is offered
//!   to that client only, so nothing else can lock or unlock.
//! * Only that client unlocking ends the lock. If it dies, the screen stays
//!   locked and it is started again.

use olo_sync::Mutex;
use std::{
    os::{fd::AsRawFd, unix::process::CommandExt},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::{
    backend::renderer::{
        ImportAll, Renderer,
        element::{Kind, surface::render_elements_from_surface_tree},
    },
    input::keyboard::KeyboardHandle,
    output::Output,
    reexports::wayland_server::{
        Client, DisplayHandle,
        backend::ClientId,
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Size},
    wayland::session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
};
use tracing::{info, warn};

use crate::{
    focus::KeyboardFocusTarget,
    render::CustomRenderElements,
    state::{AnvilState, Backend, ClientState},
};

/// Background while locked, behind (or instead of) the lock screen.
pub const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

const SLEEP_POLL: Duration = Duration::from_millis(50);
/// Longest we hold a suspend for the lock screen (logind's own limit is 5 s).
const SLEEP_WAIT: Duration = Duration::from_secs(3);

/// Present while the screen is locked, so an app_server that crashed or
/// reloaded in place comes back locked (see `relock`). The runtime dir is
/// the user's alone and goes away at logout.
/// Who the marker belongs to: olo-session outlives in-place reloads, so
/// its pid names the session (empty outside one: nothing to reload into).
fn session_id() -> String {
    std::env::var("OLO_SESSION_PID").unwrap_or_default()
}

fn marker() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(olo_xdg::legacy::runtime_file(std::path::Path::new(&dir), &format!("olo-locked-{display}")))
}

#[derive(Debug)]
pub struct Lock {
    state: SessionLockManagerState,
    /// The one client allowed to lock and unlock.
    allowed: Arc<Mutex<Option<ClientId>>>,
    locked: bool,
    surfaces: Vec<(LockSurface, Output)>,
    /// The running lock screen: its client and whether it has gone away.
    locker: Option<(ClientId, Arc<AtomicBool>)>,
    last_spawn: Option<Instant>,
    /// Keyboard focus to give back on unlock.
    focus_before: Option<KeyboardFocusTarget>,
    /// The screen saver's client, drawn over the lock screen while it runs
    /// (it was started before the lock and ends at the first input).
    pub over: Option<ClientId>,
    /// Holding off a suspend until the lock screen is up (see `watch_login`).
    sleep_guard: Option<(olo_system::power::Inhibitor, Instant)>,
}

impl Lock {
    pub fn new<B: Backend + 'static>(dh: &DisplayHandle) -> Lock {
        let allowed: Arc<Mutex<Option<ClientId>>> = Default::default();
        let filter = allowed.clone();
        let state = SessionLockManagerState::new::<AnvilState<B>, _>(dh, move |client: &Client| filter.lock().unwrap().as_ref() == Some(&client.id()));
        Lock { state, allowed, locked: false, surfaces: Vec::new(), locker: None, last_spawn: None, focus_before: None, over: None, sleep_guard: None }
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// The lock surface for `output`, if the lock screen has made one.
    pub fn surface_for(&self, output: &Output) -> Option<&WlSurface> {
        self.surfaces.iter().find(|(s, o)| o == output && s.alive()).map(|(s, _)| s.wl_surface())
    }

    pub fn surfaces(&self) -> impl Iterator<Item = &WlSurface> {
        self.surfaces.iter().filter(|(s, _)| s.alive()).map(|(s, _)| s.wl_surface())
    }
}

/// The lock surface for `output` as render elements (empty if none yet).
pub fn elements<R>(lock: &Lock, renderer: &mut R, output: &Output) -> Vec<CustomRenderElements<R>>
where
    R: Renderer + ImportAll + smithay::backend::renderer::ImportMem,
    R::TextureId: Clone + 'static,
{
    let scale = output.current_scale().fractional_scale();
    let mut out = Vec::new();
    // The screen saver first (topmost), then the lock screen under it.
    if let Some(saver) = &lock.over {
        let map = smithay::desktop::layer_map_for_output(output);
        for layer in map.layers() {
            if smithay::reexports::wayland_server::Resource::client(layer.wl_surface()).is_some_and(|c| c.id() == *saver)
                && let Some(geo) = map.layer_geometry(layer)
            {
                let at = geo.loc.to_physical_precise_round(scale);
                out.extend(render_elements_from_surface_tree(renderer, layer.wl_surface(), at, scale, 1.0, Kind::Unspecified));
            }
        }
    }
    if let Some(surface) = lock.surface_for(output) {
        out.extend(render_elements_from_surface_tree(renderer, surface, (0, 0), scale, 1.0, Kind::Unspecified));
    }
    out
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Lock now (Ctrl+⌘Q, idle, before sleep, logind's Lock). The screen is
    /// covered at once; the lock screen is started to offer unlocking.
    pub fn lock_screen(&mut self) {
        self.backend_data.wake(None);
        if !self.lock.locked {
            info!("locking");
            self.lock.locked = true;
            self.twitch = None;
            self.twitcher.forget();
            crate::spotlight::end(false);
            self.end_auto_approve("the screen locked");
            let keyboard = self.seat.get_keyboard().unwrap();
            self.lock.focus_before = keyboard.current_focus();
            keyboard.set_focus(self, None::<KeyboardFocusTarget>, SERIAL_COUNTER.next_serial());
            if let Some(p) = marker() {
                let _ = std::fs::write(p, session_id());
            }
            self.power_changed();
        }
        self.ensure_locker();
    }

    /// Lock again at startup if the previous app_server was locked.
    pub fn relock(&mut self) {
        // Only this session's lock: a marker left by another session that
        // once had the same display name doesn't count.
        let session = session_id();
        if !session.is_empty() && marker().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| s == session) {
            info!("the screen was locked before the restart");
            self.lock_screen();
        }
    }

    /// Lock before the machine sleeps (logind waits for us) and when logind
    /// asks (`loginctl lock-session`).
    pub fn watch_login(&mut self) {
        use olo_system::power::LoginEvent;
        use smithay::reexports::calloop::{
            channel::{self, Event as ChannelEvent},
            timer::{TimeoutAction, Timer},
        };
        let (tx, rx) = channel::channel();
        if let Err(e) = olo_system::power::watch_login(move |e| {
            let _ = tx.send(e);
        }) {
            warn!("not watching logind: {e}");
            return;
        }
        let _ = self.handle.insert_source(rx, |event, _, state| {
            let ChannelEvent::Msg(event) = event else { return };
            match event {
                LoginEvent::Sleeping(guard) => {
                    info!("going to sleep");
                    state.lock_screen();
                    if let Some(guard) = guard {
                        state.lock.sleep_guard = Some((guard, Instant::now()));
                        // Let the sleep go once every screen is covered.
                        let _ = state.handle.insert_source(Timer::from_duration(SLEEP_POLL), |_, _, state| {
                            if state.lock_covers_outputs() || state.lock.sleep_guard.as_ref().is_none_or(|(_, t)| t.elapsed() > SLEEP_WAIT) {
                                state.lock.sleep_guard = None;
                                return TimeoutAction::Drop;
                            }
                            TimeoutAction::ToDuration(SLEEP_POLL)
                        });
                    }
                }
                LoginEvent::Woke => {
                    info!("woke");
                    state.power_activity();
                }
                LoginEvent::Lock => state.lock_screen(),
            }
        });
    }

    /// A lock surface on every output.
    fn lock_covers_outputs(&self) -> bool {
        self.space.outputs().all(|o| self.lock.surface_for(o).is_some_and(|s| smithay::backend::renderer::utils::with_renderer_surface_state(s, |st| st.buffer().is_some()).unwrap_or(false)))
    }

    /// Keep a lock screen running while locked (start it, or again after a
    /// crash, at most every two seconds).
    pub fn ensure_locker(&mut self) {
        if !self.lock.locked {
            return;
        }
        if let Some((_, gone)) = &self.lock.locker {
            if !gone.load(Ordering::SeqCst) {
                return;
            }
            warn!("the lock screen went away while locked; starting it again");
            self.lock.locker = None;
            self.lock.surfaces.clear();
        }
        if self.lock.last_spawn.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.lock.last_spawn = Some(Instant::now());
        match self.spawn_locker() {
            Ok((id, gone)) => {
                *self.lock.allowed.lock().unwrap() = Some(id.clone());
                self.lock.locker = Some((id, gone));
            }
            Err(e) => warn!("cannot start the lock screen: {e}"),
        }
    }

    fn spawn_locker(&mut self) -> std::io::Result<(ClientId, Arc<AtomicBool>)> {
        let (id, gone) = self.spawn_private("olo-lock", &[])?;
        info!(client = ?id, "lock screen started");
        Ok((id, gone))
    }

    /// Start one of our own programs on a connection of its own
    /// (WAYLAND_SOCKET), so we know which client it is. Returns the client
    /// and a flag set when it disconnects.
    pub fn spawn_private(&mut self, name: &str, args: &[String]) -> std::io::Result<(ClientId, Arc<AtomicBool>)> {
        let program = olo_xdg::find_program(name).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{name} is not installed")))?;
        let (ours, theirs) = std::os::unix::net::UnixStream::pair()?;
        let gone = Arc::new(AtomicBool::new(false));
        let client_state = ClientState { gone: Some(gone.clone()), ..ClientState::default() };
        let client = self.display_handle.insert_client(ours, Arc::new(client_state)).map_err(|e| std::io::Error::other(e.to_string()))?;
        let fd = theirs.as_raw_fd();
        let mut cmd = std::process::Command::new(program);
        cmd.args(args).env("WAYLAND_SOCKET", fd.to_string()).env_remove("WAYLAND_DISPLAY");
        // SAFETY: only async-signal-safe fcntl calls between fork and exec,
        // to let the child inherit its end of the socket.
        unsafe {
            cmd.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        drop(theirs);
        // Reap it whenever it exits.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok((client.id(), gone))
    }

    /// While locked, keyboard input may only reach a lock surface.
    pub fn lock_guard_focus(&mut self) {
        if !self.lock.locked {
            return;
        }
        let keyboard: KeyboardHandle<Self> = self.seat.get_keyboard().unwrap();
        let ok = keyboard.current_focus().is_some_and(|f| match &f {
            KeyboardFocusTarget::Surface(s) => self.lock.surfaces().any(|l| l == s),
            _ => false,
        });
        if !ok {
            let target = self.lock_target(self.pointer.current_location());
            keyboard.set_focus(self, target.map(KeyboardFocusTarget::Surface), SERIAL_COUNTER.next_serial());
        }
    }

    /// The lock surface on the output under `pos` (or any).
    pub fn lock_target(&self, pos: Point<f64, Logical>) -> Option<WlSurface> {
        let output = self.space.output_under(pos).next().cloned();
        output.as_ref().and_then(|o| self.lock.surface_for(o)).or_else(|| self.lock.surfaces().next()).cloned()
    }

    /// The lock surface under `pos`, for the pointer.
    pub fn lock_surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(pos).next()?;
        let geo = self.space.output_geometry(output)?;
        Some((self.lock.surface_for(output)?.clone(), geo.loc.to_f64()))
    }

    fn lock_configure(&self, surface: &LockSurface, output: &Output) {
        let size = self.space.output_geometry(output).map(|g| g.size).unwrap_or_else(|| Size::from((1280, 800)));
        // Its output's scale, as windows are told it (else a lock screen
        // drew at 1× and was stretched): the surface is on that output,
        // at that fractional scale, buffers at the scale rounded up.
        let scale = output.current_scale();
        output.enter(surface.wl_surface());
        smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            smithay::wayland::fractional_scale::with_fractional_scale(states, |f| f.set_preferred_scale(scale.fractional_scale()));
            smithay::wayland::compositor::send_surface_state(surface.wl_surface(), states, scale.integer_scale(), output.current_transform());
        });
        surface.with_pending_state(|s| s.size = Some(Size::from((size.w as u32, size.h as u32))));
        surface.send_configure();
    }

    /// Outputs changed size or scale: tell the lock surfaces.
    pub fn lock_outputs_changed(&mut self) {
        for (surface, output) in self.lock.surfaces.clone() {
            self.lock_configure(&surface, &output);
        }
    }
}

impl<B: Backend + 'static> SessionLockHandler for AnvilState<B> {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.lock.state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // Our own lock screen asking (it is the only client that can): the
        // screen is already covered (see `lock_screen`), so confirm now.
        if !self.lock.locked {
            self.lock.locked = true;
            let keyboard = self.seat.get_keyboard().unwrap();
            self.lock.focus_before = keyboard.current_focus();
            keyboard.set_focus(self, None::<KeyboardFocusTarget>, SERIAL_COUNTER.next_serial());
            self.power_changed();
        }
        confirmation.lock();
    }

    fn unlock(&mut self) {
        info!("unlocked");
        self.lock.locked = false;
        self.lock.surfaces.clear();
        // (Its lock screen leaves now: the next lock starts another,
        // without taking this one's leaving for a crash.)
        self.lock.locker = None;
        let focus = self.lock.focus_before.take().filter(smithay::utils::IsAlive::alive);
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, focus, SERIAL_COUNTER.next_serial());
        self.power_activity();
        if let Some(p) = marker() {
            let _ = std::fs::remove_file(p);
        }
        self.power_changed();
        // Rescue asked for while locked: its window now.
        if self.rescue.is_some() {
            self.start_rescue_app();
        }
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let Some(output) = Output::from_resource(&output) else { return };
        self.lock_configure(&surface, &output);
        self.lock.surfaces.retain(|(s, o)| s.alive() && *o != output);
        self.lock.surfaces.push((surface, output));
        self.lock_guard_focus();
    }
}
