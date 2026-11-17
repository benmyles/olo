//! Something unread in a window: a red dot on its tab (app_server draws
//! it, `Request::SetWindowUnread`), as Deskbar's red dot is for the app
//! (Ben, 2026-11-17: "when there's a red dot badge for an app in the
//! deskbar, that app's title window bar (the yellow tab) for the specific
//! window that has the alertable item should get a red dot too ... and
//! they should all automatically go away with a nice fade out animation
//! over about 5s after activating the target tab (or window if it's not
//! a tabbed app)").
//!
//! An app with one place for what's unread (Telegram, Phone) marks its
//! window as something comes in while it isn't looked at, with
//! `clear_on_focus`: the dot fades once the window is activated. An app
//! with tabs (Terminal) marks the window whose tab has it, and unmarks it
//! once that tab is looked at, its own tab's dot fading with it (the
//! kit's tab strip: `StripTab::dot`). Sent off the looper.

use olo_interface::WindowCx;
use olo_protocol::{Client, Request};

/// This window: something unread in it (`unread`), or no longer (its
/// dot fades). `clear_on_focus`: app_server fades it once the window is
/// activated.
pub fn mark(cx: &mut WindowCx, unread: bool, clear_on_focus: bool) {
    let Some(surface) = cx.surface_id() else { return };
    send(Request::SetWindowUnread { pid: Some(std::process::id()), surface: Some(surface), app_id: None, unread, clear_on_focus });
}

/// Every window of this program.
pub fn mark_program(unread: bool, clear_on_focus: bool) {
    send(Request::SetWindowUnread { pid: Some(std::process::id()), surface: None, app_id: None, unread, clear_on_focus });
}

/// Every window of the app `app_id` (a program that isn't the windows'
/// own: Slack's host for its Chrome).
pub fn mark_app(app_id: &str, unread: bool, clear_on_focus: bool) {
    send(Request::SetWindowUnread { pid: None, surface: None, app_id: Some(app_id.to_owned()), unread, clear_on_focus });
}

fn send(request: Request) {
    let _ = std::thread::Builder::new().name("unread".into()).spawn(move || {
        if let Err(e) = Client::connect().and_then(|mut c| c.request(&request)) {
            tracing::debug!("couldn't mark the window: {e}");
        }
    });
}
