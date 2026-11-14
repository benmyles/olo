//! Locking the session (the lock screen, apps/lock): a window the app
//! makes on every output, and on any output added while locked, until
//! [`unlock_session`]. Over app_server's ext-session-lock.

use std::sync::Arc;

use olo_interface::{AppCx, Cx, SessionLockEvent, View, WindowHandle};

use crate::WindowOptions;

/// Locks the session: `build` makes the view covering each output
/// (named), in a window on a looper of its own; `on_event` hears, on the
/// app's looper, `Locked` (once), `Outputs` (each time the outputs
/// covered changed) and `Finished` (the lock was refused or ended:
/// quit), with the windows covering outputs now. A window whose output
/// went is forgotten, and the output covered again if it comes back.
pub fn lock_session<V: View>(
    app: &mut AppCx,
    build: impl Fn(&str, &mut Cx<V>) -> V + Send + Sync + 'static,
    mut on_event: impl FnMut(SessionLockEvent, &[(String, WindowHandle<V>)], &mut AppCx) + 'static,
) -> olo_interface::Result<()> {
    let build = Arc::new(build);
    let mut covered: Vec<(String, WindowHandle<V>)> = Vec::new();
    let mut told = false;
    olo_interface::lock_session(app, move |event, app| {
        if event == SessionLockEvent::Finished {
            return on_event(event, &covered, app);
        }
        let before = covered.len();
        covered.retain(|(_, window)| window.is_open());
        let mut changed = covered.len() != before;
        for name in olo_interface::display_names().into_iter().flatten() {
            if covered.iter().any(|(output, _)| *output == name) {
                continue;
            }
            let options = WindowOptions { lock_output: Some(name.clone()), panel: false, ..WindowOptions::new("Locked", 1.0, 1.0) };
            let (build, output) = (build.clone(), name.clone());
            covered.push((name, crate::open_window(options, move |cx| build(&output, cx))));
            changed = true;
        }
        if event == SessionLockEvent::Locked && !told {
            told = true;
            on_event(event, &covered, app);
        } else if changed {
            on_event(SessionLockEvent::Outputs, &covered, app);
        }
    })
}

/// Ends the lock (the person was let in): app_server has been told by
/// the time this returns.
pub fn unlock_session() {
    olo_interface::unlock_session();
}
