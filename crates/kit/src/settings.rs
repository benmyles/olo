//! app_server's settings (what Preferences changes) for apps that follow
//! some of them. Outside Olo (no `$OLO_SOCKET`) there are none.

use std::time::Duration;

use olo_protocol::{Client, Event, PowerStatus, Reply, Request, Settings};

/// The settings now, asked once (a local round trip; fine at startup).
pub fn current() -> Option<Settings> {
    olo_protocol::socket_path()?;
    match Client::connect().ok()?.request(&Request::GetSettings).ok()? {
        Reply::Settings { settings } => Some(settings),
        _ => None,
    }
}

/// Call `f` with the settings on a thread: now, after every change, and
/// again after app_server restarts.
pub fn watch(f: impl Fn(Settings) + Send + 'static) {
    if olo_protocol::socket_path().is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("settings-watch".into())
        .spawn(move || loop {
            if let Some(s) = current() {
                f(s);
            }
            if let Ok(client) = Client::connect()
                && let Ok(events) = client.into_events()
            {
                for event in events {
                    if let Ok(Event::SettingsChanged { settings }) = event {
                        f(settings);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        })
        .ok();
}

/// Call `f` on a thread with app_server's power status: now, after every
/// change (the screen locking, the displays sleeping, …), and again after
/// app_server restarts.
pub fn watch_power(f: impl Fn(PowerStatus) + Send + 'static) {
    if olo_protocol::socket_path().is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("power-watch".into())
        .spawn(move || loop {
            if let Ok(mut c) = Client::connect()
                && let Ok(Reply::Power { status }) = c.request(&Request::PowerStatus)
            {
                f(status);
            }
            if let Ok(client) = Client::connect()
                && let Ok(events) = client.into_events()
            {
                for event in events {
                    if let Ok(Event::PowerChanged { status }) = event {
                        f(status);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        })
        .ok();
}

/// Change the settings: `edit` is given the current ones and app_server
/// gets the result (and tells every app that follows them). On a thread,
/// so it never holds up the UI; false outside Olo.
/// Whether `app_id`'s windows are grouped (share one frame, a tab each:
/// the default) in `settings`.
pub fn groups_windows(settings: &Settings, app_id: &str) -> bool {
    !settings.tiling.ungrouped_apps.iter().any(|a| a == app_id)
}

/// Group `app_id`'s windows, or not (Deskbar's and Tracker's Group
/// Windows), off the UI thread as `update`.
pub fn set_groups_windows(app_id: &str, on: bool) -> bool {
    let app = app_id.to_owned();
    update(move |s| {
        s.tiling.ungrouped_apps.retain(|a| *a != app);
        if !on {
            s.tiling.ungrouped_apps.push(app);
        }
    })
}

pub fn update(edit: impl FnOnce(&mut Settings) + Send + 'static) -> bool {
    if olo_protocol::socket_path().is_none() {
        return false;
    }
    std::thread::Builder::new()
        .name("settings-update".into())
        .spawn(move || {
            if let Some(mut s) = current() {
                edit(&mut s);
                if let Ok(mut c) = Client::connect() {
                    let _ = c.request(&Request::SetSettings { settings: s });
                }
            }
        })
        .is_ok()
}
