//! Locking the session (the lock screen, apps/lock): a window the app
//! makes on every output, and on any output added while locked, until
//! [`unlock_session`]. Over app_server's ext-session-lock.

use std::collections::HashSet;

use futures::{StreamExt as _, channel::mpsc};
use gpui::{App, Entity, Render, SessionLockEvent, Window};

use crate::WindowOptions;

/// Locks the session: `build` makes the view covering each output
/// (named); `on_event` hears `Locked` (once) and `Finished` (the lock was
/// refused or ended: quit).
pub fn lock_session<V: Render + 'static>(
    cx: &mut App,
    build: impl Fn(&str, &mut Window, &mut App) -> Entity<V> + 'static,
    on_event: impl Fn(SessionLockEvent, &mut App) + 'static,
) -> gpui::Result<()> {
    let (tx, mut rx) = mpsc::unbounded::<SessionLockEvent>();
    gpui::lock_session(move |e| {
        let _ = tx.unbounded_send(e);
    })?;
    cx.spawn(async move |cx| {
        let mut covered = HashSet::<String>::new();
        let mut told = false;
        while let Some(event) = rx.next().await {
            let _ = cx.update(|cx| {
                if event == SessionLockEvent::Finished {
                    on_event(event, cx);
                    return;
                }
                for display in cx.displays() {
                    let Some(name) = display.name() else { continue };
                    if !covered.insert(name.clone()) {
                        continue;
                    }
                    let options = WindowOptions { lock_output: Some(name.clone()), panel: false, ..WindowOptions::new("Locked", 1.0, 1.0) };
                    let build = &build;
                    crate::open_window(cx, options, |window, cx| build(&name, window, cx));
                }
                if event == SessionLockEvent::Locked && !told {
                    told = true;
                    on_event(event, cx);
                }
            });
        }
    })
    .detach();
    Ok(())
}

/// Ends the lock (the person was let in), at once.
pub fn unlock_session() {
    gpui::unlock_session();
}
