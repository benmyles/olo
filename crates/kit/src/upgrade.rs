//! Becoming the new version after an upgrade. olo-session signals every
//! Olo program whose file it replaced (SIGURG, ignored by default); a
//! kit app whose binary changed asks its windows for their state (each
//! window's own view: `View::save_state`), has the app make one state
//! of them ([`on_upgrade`]) and re-executes as the new binary, which
//! finds it in [`restored_state`]. app_server restarting in place is
//! the same to it: it re-executes with its state saved and connects to
//! the app_server now there.
//!
//! All of it happens on a thread of its own (`upgrade-watch`), which
//! may wait for the windows' loopers: no looper waits for another.

use std::{
    os::{fd::FromRawFd, unix::process::CommandExt as _},
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, SystemTime},
};

use olo_sync::Mutex;

/// Where SIGURG lands.
static PIPE: AtomicI32 = AtomicI32::new(-1);
/// What comes down the pipe: the binary may have been replaced, or the
/// compositor is gone.
const UPGRADE: u8 = 1;
const LOST: u8 = 2;

extern "C" fn on_signal(_: libc::c_int) {
    let fd = PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe.
        unsafe { libc::write(fd, [UPGRADE].as_ptr().cast(), 1) };
    }
}

type Save = Arc<dyn Fn(Vec<String>) -> Option<String> + Send + Sync>;
type Due = Arc<dyn Fn() + Send + Sync>;
type Busy = Arc<dyn Fn() -> bool + Send + Sync>;

/// What the app said it does about becoming another version.
#[derive(Default)]
struct Hooks {
    save: Option<Save>,
    due: Option<Due>,
    busy: Option<Busy>,
}

fn hooks() -> &'static Mutex<Hooks> {
    static HOOKS: OnceLock<Mutex<Hooks>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

/// What the app carries into the new version (its open documents, its
/// windows' places): `save` is handed what each of its windows keeps
/// (its own view's `View::save_state`, those that keep something) and
/// makes the one state the new version finds in [`restored_state`].
/// Called just before it becomes it, on a thread of its own. Without
/// it, an app with one window carries that window's state as it is.
pub fn on_upgrade(save: impl Fn(Vec<String>) -> Option<String> + Send + Sync + 'static) {
    hooks().lock().unwrap_or_else(|e| e.into_inner()).save = Some(Arc::new(save));
}

/// A new version of the app's binary is installed and the app will
/// become it once it isn't busy ([`upgrade_waits_while`]): finish what is
/// under way and take on nothing new (a server stops accepting).
pub fn on_upgrade_due(f: impl Fn() + Send + Sync + 'static) {
    hooks().lock().unwrap_or_else(|e| e.into_inner()).due = Some(Arc::new(f));
}

/// While `busy` says so (sharing a screen, requests under way) an upgrade
/// of the app's binary waits; the app becomes the new version once it
/// says no (asked four times a second while one is due).
pub fn upgrade_waits_while(busy: impl Fn() -> bool + Send + Sync + 'static) {
    hooks().lock().unwrap_or_else(|e| e.into_inner()).busy = Some(Arc::new(busy));
}

/// The state the previous version left (once; None if it left none).
pub fn restored_state() -> Option<String> {
    olo_kit_base::restore::take()
}

/// How long the windows have to say what they keep.
const STATE_WAIT: Duration = Duration::from_secs(2);

/// What the app carries over, asked now: of its windows, then of it.
/// (Not from a looper: it waits for them.)
fn saved_state() -> Option<String> {
    let states = olo_interface::ask_windows(STATE_WAIT, |cx| cx.save_state());
    let save = hooks().lock().unwrap_or_else(|e| e.into_inner()).save.clone();
    match save {
        Some(save) => save(states),
        None => (states.len() == 1).then(|| states.into_iter().next()).flatten(),
    }
}

/// The app is told its new version is due, and waited for while it's
/// busy.
fn wait_until_free() {
    let (due, busy) = {
        let hooks = hooks().lock().unwrap_or_else(|e| e.into_inner());
        (hooks.due.clone(), hooks.busy.clone())
    };
    if let Some(due) = due {
        due();
    }
    let Some(busy) = busy else { return };
    let mut marked = false;
    while busy() {
        // (Said, so olo-session doesn't stop it for being old.)
        if !marked {
            olo_kit_base::restore::mark_waiting(true);
            marked = true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if marked {
        olo_kit_base::restore::mark_waiting(false);
    }
}

/// The compositor's connection is gone (app_server restarted in place,
/// or a protocol error): the app re-executes as it does for an upgrade,
/// with its state saved, and reconnects to the app_server now there; at
/// once, unless it hands its work over (`on_upgrade_due`), which it does
/// first, waiting while `upgrade_waits_while` says it's busy.
///
/// The GPU device lost (a driver reset, `olo_interface::gpu_device_lost`)
/// is the same: what it made (pictures, pages, glyphs) went with it, and
/// only a fresh start makes them again. Lost again within [`GPU_AGAIN`]
/// of that start, the GPU is broken rather than reset, and the app ends
/// rather than go round for ever.
fn lost_compositor(binary: &Binary) {
    if handed_connection() {
        tracing::warn!("the connection the compositor handed this program is gone: ending");
        olo_interface::quit();
        return;
    }
    let gpu = olo_interface::gpu_device_lost();
    if gpu && gpu_lost_before().is_some_and(|at| at.elapsed().unwrap_or_default() < GPU_AGAIN) {
        tracing::error!("the GPU device is lost again within {} s of the last time: ending", GPU_AGAIN.as_secs());
        olo_interface::quit();
        return;
    }
    if gpu {
        tracing::warn!("the GPU device is lost: re-executing to start anew, with the state saved");
    } else {
        tracing::warn!("the compositor is gone: re-executing to reconnect, with the state saved");
        // An app that hands its work over (`on_upgrade_due`: Token Pool's
        // gateway, whose connections aren't the compositor's) does so now
        // as for an upgrade, and what is under way finishes first.
        wait_until_free();
    }
    let state = saved_state();
    // The person's app on an agent screen that has gone goes back to
    // their display.
    let env = (!gpu).then(going_home).flatten();
    binary.exec_with(state, gpu, env.as_ref());
    // (It couldn't: nothing to show on, nothing to become.)
    olo_interface::quit();
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
    handed_connection();
}

/// Whether the compositor handed this program its connection
/// (`WAYLAND_SOCKET`: app_server starts the lock screen and the screen
/// saver so, to know which client each is). Read at start-up, before
/// the connection is made (which takes it out of the environment). Such
/// a connection can't be made again, by this program or a new one: where
/// another program re-executes (its binary upgraded, the compositor gone)
/// this one ends, and whoever started it starts it again if it is still
/// wanted (app_server, the lock screen while locked).
fn handed_connection() -> bool {
    static HANDED: OnceLock<bool> = OnceLock::new();
    *HANDED.get_or_init(|| std::env::var_os("WAYLAND_SOCKET").is_some_and(|s| !s.is_empty()))
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
    /// `state` (and, when the GPU device was lost, when that was), with
    /// the environment `env` changes (another display's: a move).
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
        // What the app saved, and how its windows were left, is on disk
        // before it's replaced.
        crate::kept::write();
        olo_kit_base::store::flush();
        let e = cmd.exec();
        tracing::warn!(?e, "could not start the upgraded binary; carrying on");
    }
}

/// Listens for olo-session's signal and for the compositor going, in a
/// session.
pub(crate) fn init() {
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
    // The compositor gone: told down the same pipe, so what's done about
    // it is done on the watching thread, which may wait for the windows.
    olo_interface::on_compositor_lost(|| {
        let fd = PIPE.load(Ordering::Relaxed);
        if fd >= 0 {
            // SAFETY: a byte down our own pipe.
            unsafe { libc::write(fd, [LOST].as_ptr().cast(), 1) };
        }
    });
    let binary = Binary::current();
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
                if buf[..n].contains(&LOST) {
                    lost_compositor(&binary);
                    return;
                }
                if !binary.changed() {
                    continue;
                }
                if handed_connection() {
                    tracing::info!("upgraded: ending, to be started again by the compositor that handed this program its connection");
                    olo_interface::quit();
                    return;
                }
                wait_until_free();
                binary.exec_with(saved_state(), false, None);
            }
        })
        .ok();
}
