//! Becoming the new version after an upgrade. olo-session signals every
//! Olo program whose file it replaced (SIGURG, ignored by default); a
//! GPUI kit app whose binary changed asks the app for its state
//! ([`on_upgrade`]) and re-executes as the new binary, which finds it in
//! [`restored_state`]. app_server restarting in place is the same to
//! it ([`lost_compositor`]): it re-executes with its state saved and
//! connects to the app_server now there.

use std::{
    os::{fd::FromRawFd, unix::process::CommandExt as _},
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures::{StreamExt, channel::mpsc};
use gpui::{App, Global};

/// Where SIGURG lands.
static PIPE: AtomicI32 = AtomicI32::new(-1);
/// What came down the pipe.
const UPGRADE: u8 = 1;

extern "C" fn on_signal(_: libc::c_int) {
    let fd = PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe.
        unsafe { libc::write(fd, [UPGRADE].as_ptr().cast(), 1) };
    }
}

/// The compositor's connection is gone (app_server restarted in place,
/// or a protocol error): the app re-executes as it does for an upgrade,
/// with its state saved, and reconnects to the app_server now there;
/// at once, whatever it was busy with (no compositor, nothing to show
/// it on). Here, synchronously: GPUI calls this with nothing borrowed,
/// and its loop may be over.
///
/// The GPU device lost (a driver reset, `gpui::gpu_device_lost`) is the
/// same: what it made (pictures, pages, glyphs) went with it, and only
/// a fresh start makes them again. Lost again within [`GPU_AGAIN`] of
/// that start, the GPU is broken rather than reset, and the app ends
/// rather than go round for ever.
fn lost_compositor(cx: &mut App, binary: &Binary) {
    let gpu = gpui::gpu_device_lost();
    if gpu && gpu_lost_before().is_some_and(|at| at.elapsed().unwrap_or_default() < GPU_AGAIN) {
        tracing::error!("the GPU device is lost again within {} s of the last time: ending", GPU_AGAIN.as_secs());
        return;
    }
    if gpu {
        tracing::warn!("the GPU device is lost: re-executing to start anew, with the state saved");
    } else {
        tracing::warn!("the compositor is gone: re-executing to reconnect, with the state saved");
    }
    let state = saved_state(cx);
    // The person's app on an agent screen that has gone goes back to
    // their display.
    let env = (!gpu).then(going_home).flatten();
    binary.exec_with(state, gpu, env.as_ref());
}

/// What the app carries over ([`on_upgrade`]), asked now.
fn saved_state(cx: &mut App) -> Option<String> {
    let save = cx.try_global::<Saving>().is_some().then(|| cx.global_mut::<Saving>().0.take()).flatten();
    let state = save.as_ref().and_then(|save| save(cx));
    if let Some(save) = save {
        cx.global_mut::<Saving>().0 = Some(save);
    }
    state
}

/// The way back to the person's display, if this is their app on an
/// agent screen that has ended (its compositor lost: stopped, or its
/// app_server restarting, which keeps the screen's session and sockets;
/// a session ending takes a moment to remove them, so a little wait).
fn going_home() -> Option<olo_kit_base::moving::MoveTo> {
    use olo_kit_base::moving;
    let home = moving::home()?;
    let until = std::time::Instant::now() + HOME_WAIT;
    while !moving::screen_gone() {
        if std::time::Instant::now() > until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    tracing::warn!("the agent screen this app was moved to has gone: back to the person's display");
    Some(moving::env_for(&home, |k| std::env::var(k).ok()))
}

/// How long a lost compositor's screen has to be seen to end.
const HOME_WAIT: Duration = Duration::from_secs(2);

/// What a copy of the app on another display needs to open one of its
/// windows as it is here ([`on_move_window`]).
pub struct WindowMove {
    /// The copy's arguments.
    pub args: Vec<String>,
    /// The state it finds in [`restored_state`].
    pub state: Option<String>,
    /// Lets the window go here once the copy has started, the app's own
    /// way (without what closing it would end: a Terminal's shell).
    pub gone: Box<dyn FnOnce(&mut App)>,
}

type MoveWindow = Box<dyn Fn(gpui::AnyWindowHandle, &mut App) -> Result<WindowMove, String>>;

struct MovingWindows(Option<MoveWindow>);

impl Global for MovingWindows {}

/// How the app moves one of its windows to another display (an agent
/// screen's) while it keeps the others: what a copy of it there needs to
/// open that window ([`WindowMove`]), or why that one can't go. Without
/// it, only an app with one window can move (all of it).
pub fn on_move_window(cx: &mut App, f: impl Fn(gpui::AnyWindowHandle, &mut App) -> Result<WindowMove, String> + 'static) {
    cx.set_global(MovingWindows(Some(Box::new(f))));
}

type Refusing = Box<dyn Fn(&App) -> Option<String>>;

struct RefusingMoves(Refusing);

impl Global for RefusingMoves {}

/// While `why` says something, the app stays on its display, whole or a
/// window of it (Terminal started on an agent screen: its shells end with
/// the screen).
pub fn refuse_moves_while(cx: &mut App, why: impl Fn(&App) -> Option<String> + 'static) {
    cx.set_global(RefusingMoves(Box::new(why)));
}

/// Go to another display (`commands`' `move`): one window, by a copy of
/// the app there, or the whole app, re-executed there with its state.
pub(crate) fn move_to(cx: &mut App, to: olo_kit_base::moving::MoveTo, reply: std::sync::mpsc::Sender<olo_kit_base::moving::Moved>) {
    use olo_kit_base::moving::{self, Moved};
    let env = moving::env_for(&to, |k| std::env::var(k).ok());
    if let Some(why) = cx.try_global::<RefusingMoves>().and_then(|r| (r.0)(cx)) {
        let _ = reply.send(Moved::failed(why));
        return;
    }
    let window = to.surface.and_then(|s| crate::agent::by_surface(cx, s));
    if to.surface.is_some() && window.is_none() {
        let _ = reply.send(Moved::failed("it has no such window"));
        return;
    }
    let windows = cx.windows();
    let shell = windows.iter().any(|w| w.update(cx, |_, window, _| matches!(window.window_kind(), gpui::WindowKind::LayerShell | gpui::WindowKind::SessionLock)).unwrap_or(false));
    let others = windows.iter().filter(|w| Some(**w) != window).count();
    let stays = shell || crate::app::resident(cx);
    if let Some(w) = window
        && (others > 0 || stays)
    {
        // One of several: a copy there opens it.
        let hook = cx.try_global::<MovingWindows>().is_some().then(|| cx.global_mut::<MovingWindows>().0.take()).flatten();
        let made = match &hook {
            Some(f) => f(w, cx),
            None => Err("it can't move one of its windows by itself yet".into()),
        };
        if let Some(hook) = hook {
            cx.global_mut::<MovingWindows>().0 = Some(hook);
        }
        let made = match made {
            Ok(m) => m,
            Err(why) => {
                let _ = reply.send(Moved::failed(why));
                return;
            }
        };
        match moving::start_copy(&made.args, made.state.as_deref(), &env) {
            Ok(pid) => {
                (made.gone)(cx);
                let _ = reply.send(Moved { ok: true, error: None, pid: Some(pid) });
            }
            Err(e) => {
                let _ = reply.send(Moved::failed(format!("a copy of it couldn't be started there: {e}")));
            }
        }
        return;
    }
    if stays {
        let _ = reply.send(Moved::failed("it is part of the desktop here, so it stays whole: move its windows one by one"));
        return;
    }
    // The whole app: said first (it is under way), then re-executed there
    // with its state, once the answer has gone.
    let _ = reply.send(Moved { ok: true, error: None, pid: Some(std::process::id()) });
    tracing::info!("moving to another display");
    let binary = Binary::current();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(150)).await;
        let state = cx.update(saved_state).ok().flatten();
        binary.exec_with(state, false, Some(&env));
    })
    .detach();
}

/// How soon the GPU lost again means it's broken, not reset.
const GPU_AGAIN: Duration = Duration::from_secs(10);

/// Names when the GPU device was last lost, for the program re-executed
/// because of it (seconds since the epoch).
const GPU_LOST_ENV: &str = "OLO_GPU_LOST_AT";

/// When the previous self lost the GPU device, if that's why this one
/// started. Claimed (read and taken out of the environment) as the app
/// starts ([`claim`]), so the programs it starts don't inherit it.
fn gpu_lost_before() -> Option<SystemTime> {
    static AT: OnceLock<Option<SystemTime>> = OnceLock::new();
    *AT.get_or_init(|| {
        let at = std::env::var(GPU_LOST_ENV).ok()?;
        // SAFETY: claimed at start-up (`run`, first thing), before the
        // app starts threads that read the environment.
        unsafe { std::env::remove_var(GPU_LOST_ENV) };
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(at.parse().ok()?))
    })
}

/// Takes what the previous self left in the environment, at start-up.
pub(crate) fn claim() {
    gpu_lost_before();
}

type SaveState = Box<dyn Fn(&mut App) -> Option<String>>;

struct Saving(Option<SaveState>);

impl Global for Saving {}

/// What the app carries into the new version (its open documents, its
/// windows' places): called just before it becomes it.
pub fn on_upgrade(cx: &mut App, save: impl Fn(&mut App) -> Option<String> + 'static) {
    cx.set_global(Saving(Some(Box::new(save))));
}

type Due = Box<dyn Fn(&mut App)>;
type Busy = Box<dyn Fn(&App) -> bool>;

/// What the app does when its new version is installed, and what makes
/// becoming it wait.
#[derive(Default)]
struct Waiting {
    due: Option<Due>,
    busy: Option<Busy>,
}

impl Global for Waiting {}

/// A new version of the app's binary is installed and the app will
/// become it once it isn't busy ([`upgrade_waits_while`]): finish what is
/// under way and take on nothing new (a server stops accepting).
pub fn on_upgrade_due(cx: &mut App, f: impl Fn(&mut App) + 'static) {
    cx.default_global::<Waiting>().due = Some(Box::new(f));
}

/// While `busy` says so (sharing a screen, requests under way) an upgrade
/// of the app's binary waits; the app becomes the new version once it
/// says no (asked four times a second while one is due).
pub fn upgrade_waits_while(cx: &mut App, busy: impl Fn(&App) -> bool + 'static) {
    cx.default_global::<Waiting>().busy = Some(Box::new(busy));
}

/// The state the previous version left (once; None if it left none).
pub fn restored_state() -> Option<String> {
    olo_kit_base::restore::take()
}

/// Our own executable, to notice an upgrade replacing it.
struct Binary {
    path: PathBuf,
    id: Option<(u64, u64, SystemTime)>,
}

impl Binary {
    fn current() -> Binary {
        let path = std::env::current_exe().unwrap_or_default();
        let id = Self::identity(&path);
        Binary { path, id }
    }

    fn identity(path: &std::path::Path) -> Option<(u64, u64, SystemTime)> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino(), m.modified().ok()?))
    }

    fn changed(&self) -> bool {
        let now = Self::identity(&self.path);
        now.is_some() && now != self.id
    }

    /// Replaces this process with the binary now on disk, handing it
    /// `state` (and, when the GPU device was lost, when that was).
    fn exec(&self, state: Option<String>, gpu_lost: bool) {
        self.exec_with(state, gpu_lost, None);
    }

    /// [`Self::exec`], with the environment `env` changes (another
    /// display's: a move).
    fn exec_with(&self, state: Option<String>, gpu_lost: bool, env: Option<&olo_kit_base::moving::MoveTo>) {
        let mut cmd = std::process::Command::new(&self.path);
        cmd.args(std::env::args_os().skip(1));
        olo_xdg::upgrade_env(&mut cmd);
        if let Some(env) = env {
            olo_kit_base::moving::apply(env, &mut cmd);
        }
        olo_kit_base::restore::hand_over(&mut cmd, state.as_deref());
        if gpu_lost {
            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
            cmd.env(GPU_LOST_ENV, now.as_secs().to_string());
        }
        tracing::info!(path = %self.path.display(), "re-executing");
        // What the app saved is on disk before it's replaced.
        olo_kit_base::store::flush();
        let e = cmd.exec();
        tracing::warn!(?e, "could not start the upgraded binary; carrying on");
    }
}

/// Listens for olo-session's signal, in a session.
pub(crate) fn init(cx: &mut App) {
    if std::env::var_os("OLO_SESSION").is_none() {
        return;
    }
    let mut fds = [0; 2];
    // SAFETY: a pipe, and a process-wide handler that only writes to it.
    let read = unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        PIPE.store(fds[1], Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGURG, &action, std::ptr::null_mut());
        std::fs::File::from_raw_fd(fds[0])
    };
    cx.on_compositor_lost({
        let binary = Binary::current();
        move |cx| lost_compositor(cx, &binary)
    })
    .detach();
    let (tx, mut rx) = mpsc::unbounded::<u8>();
    // (A thread that sleeps in read(2) until a signal comes.)
    std::thread::Builder::new()
        .name("upgrade-watch".into())
        .spawn(move || {
            use std::io::Read as _;
            let mut read = read;
            let mut buf = [0u8; 16];
            loop {
                let n = match read.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };
                for &what in &buf[..n] {
                    if tx.unbounded_send(what).is_err() {
                        return;
                    }
                }
            }
        })
        .ok();
    let binary = Binary::current();
    cx.spawn(async move |cx| {
        while let Some(what) = rx.next().await {
            if what != UPGRADE || !binary.changed() {
                continue;
            }
            {
                // Told, then waited for while busy.
                let _ = cx.update(|cx| {
                    let due = cx.try_global::<Waiting>().is_some().then(|| cx.global_mut::<Waiting>().due.take()).flatten();
                    if let Some(due) = due {
                        due(cx);
                        cx.global_mut::<Waiting>().due = Some(due);
                    }
                });
                loop {
                    let busy = cx.update(|cx| cx.try_global::<Waiting>().and_then(|w| w.busy.as_ref()).is_some_and(|busy| busy(cx))).unwrap_or(false);
                    if !busy {
                        break;
                    }
                    cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                }
            }
            let state = cx
                .update(|cx| {
                    let save = cx.try_global::<Saving>().is_some().then(|| cx.global_mut::<Saving>().0.take()).flatten();
                    let state = save.as_ref().and_then(|save| save(cx));
                    if let Some(save) = save {
                        cx.global_mut::<Saving>().0 = Some(save);
                    }
                    state
                })
                .ok()
                .flatten();
            binary.exec(state, false);
        }
    })
    .detach();
}
