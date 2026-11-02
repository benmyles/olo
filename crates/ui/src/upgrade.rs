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
    sync::atomic::{AtomicI32, Ordering},
    time::SystemTime,
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
fn lost_compositor(cx: &mut App, binary: &Binary) {
    tracing::warn!("the compositor is gone: re-executing to reconnect, with the state saved");
    let save = cx.try_global::<Saving>().is_some().then(|| cx.global_mut::<Saving>().0.take()).flatten();
    let state = save.as_ref().and_then(|save| save(cx));
    if let Some(save) = save {
        cx.global_mut::<Saving>().0 = Some(save);
    }
    binary.exec(state);
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

    /// Replaces this process with the binary now on disk, handing it `state`.
    fn exec(&self, state: Option<String>) {
        let mut cmd = std::process::Command::new(&self.path);
        cmd.args(std::env::args_os().skip(1));
        olo_xdg::upgrade_env(&mut cmd);
        olo_kit_base::restore::hand_over(&mut cmd, state.as_deref());
        tracing::info!(path = %self.path.display(), "re-executing");
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
            binary.exec(state);
        }
    })
    .detach();
}
