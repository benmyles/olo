//! The screen saver: after `screen_saver.run_minutes` idle (or at once, from
//! Preferences' Test or the "Fade now" corner) app_server starts
//! `olo-screensaver` on a connection of its own; it covers every output
//! with an overlay layer surface and draws the chosen module. Any input
//! ends it (app_server disconnects it). If the screen locks while it runs
//! (`screen_saver.lock`), it keeps drawing over the lock screen until then.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_protocol::Corner;
use smithay::{
    input::pointer::CursorImageStatus,
    reexports::wayland_server::backend::{ClientId, DisconnectReason},
    utils::{Logical, Point},
};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

/// Modules that show the desktop (olo-screensavers' `uses_desktop`):
/// app_server grabs a picture of it for them.
const DESKTOP_MODULES: &[&str] = &["Flip", "Lens"];

/// Input right after a deliberate start (Test, the corner) doesn't end it.
const GRACE: Duration = Duration::from_millis(1500);

/// How close to a corner counts, in points.
const CORNER: f64 = 3.0;

#[derive(Debug, Default)]
pub struct Saver {
    client: Option<(ClientId, Arc<AtomicBool>)>,
    grace_until: Option<Instant>,
    /// The pointer is in the "Fade now" corner (start on entering only).
    in_corner: bool,
}

impl Saver {
    pub fn running(&self) -> bool {
        self.client.as_ref().is_some_and(|(_, gone)| !gone.load(Ordering::SeqCst))
    }
}

fn desktop_picture() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(std::path::PathBuf::from(dir).join(format!("olo-saver-desktop-{display}.png")))
}

/// The pointer as it is shown, for `status` (`AnvilState::cursor_status`):
/// hidden while the screen saver runs. (The saver hides it too, but only
/// once the pointer is on its surface, which takes a move, and a move
/// ends it.)
pub fn shown_cursor(saver: &Saver, status: &CursorImageStatus) -> CursorImageStatus {
    if saver.running() { CursorImageStatus::Hidden } else { status.clone() }
}

impl<B: Backend + 'static> AnvilState<B> {
    pub fn start_screen_saver(&mut self, deliberate: bool) {
        if self.saver.running() || self.power.display_asleep() {
            return;
        }
        let ss = self.settings.screen_saver.clone();
        let options = ss.options.get(&ss.module).cloned().unwrap_or_default();
        let mut args = vec!["--module".to_owned(), ss.module.clone(), "--options".to_owned(), serde_json::to_string(&options).unwrap()];
        if DESKTOP_MODULES.contains(&ss.module.as_str())
            && !self.lock.locked()
            && let Some(path) = desktop_picture()
        {
            // Taken with the next frame, before the saver has drawn.
            let _ = std::fs::remove_file(&path);
            let _ = self.request_screenshot(crate::screenshot::Shot::screen(path.clone()));
            args.extend(["--desktop".to_owned(), path.display().to_string()]);
        }
        match self.spawn_private("olo-screensaver", &args) {
            Ok((id, gone)) => {
                info!(module = ss.module, "screen saver started");
                self.lock.over = Some(id.clone());
                self.saver.client = Some((id, gone));
                self.saver.grace_until = deliberate.then(|| Instant::now() + GRACE);
                self.power_changed();
            }
            Err(e) => warn!("cannot start the screen saver: {e}"),
        }
    }

    pub fn stop_screen_saver(&mut self) {
        let Some((id, _)) = self.saver.client.take() else { return };
        info!("screen saver stopped");
        self.display_handle.backend_handle().kill_client(id, DisconnectReason::ConnectionClosed);
        self.lock.over = None;
        self.saver.grace_until = None;
        self.power_changed();
    }

    /// Input: end the saver (unless it was just started on purpose), and
    /// start it if the pointer has just reached the "Fade now" corner.
    pub fn saver_activity(&mut self) {
        if self.saver.client.is_some() && self.saver.grace_until.is_none_or(|t| Instant::now() >= t) {
            self.stop_screen_saver();
        }
        // Activity is noted before the event moves the pointer: look at the
        // corner once it has been handled.
        if self.settings.screen_saver.fade_now.is_some() {
            self.handle.insert_idle(|state| state.saver_corner());
        }
    }

    fn saver_corner(&mut self) {
        let ss = &self.settings.screen_saver;
        let in_corner = ss.enabled && ss.fade_now.is_some() && self.pointer_corner() == ss.fade_now;
        if in_corner && !self.saver.in_corner {
            self.start_screen_saver(true);
        }
        self.saver.in_corner = in_corner;
    }

    /// Every power tick: forget a saver that went away by itself.
    pub fn saver_tick(&mut self) {
        if self.saver.client.as_ref().is_some_and(|(_, gone)| gone.load(Ordering::SeqCst)) {
            self.saver.client = None;
            self.lock.over = None;
            self.power_changed();
        }
    }

    /// The screen corner the pointer is in, if any.
    pub fn pointer_corner(&self) -> Option<Corner> {
        let p: Point<f64, Logical> = self.pointer.current_location();
        self.space.outputs().find_map(|o| {
            let g = self.space.output_geometry(o)?.to_f64();
            let (left, top) = (p.x - g.loc.x <= CORNER, p.y - g.loc.y <= CORNER);
            let (right, bottom) = (g.loc.x + g.size.w - p.x <= CORNER + 1.0, g.loc.y + g.size.h - p.y <= CORNER + 1.0);
            if !g.contains(p) {
                return None;
            }
            match (left, right, top, bottom) {
                (true, _, true, _) => Some(Corner::TopLeft),
                (_, true, true, _) => Some(Corner::TopRight),
                (true, _, _, true) => Some(Corner::BottomLeft),
                (_, true, _, true) => Some(Corner::BottomRight),
                _ => None,
            }
        })
    }
}
