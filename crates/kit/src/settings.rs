//! app_server's settings (what Preferences changes) for apps that follow
//! some of them. Outside Tabula (no `$TABULA_SOCKET`) there are none.

use std::time::Duration;

use tabula_protocol::{Client, Event, Reply, Request, Settings};

/// The settings now, asked once (a local round trip; fine at startup).
pub fn current() -> Option<Settings> {
    tabula_protocol::socket_path()?;
    match Client::connect().ok()?.request(&Request::GetSettings).ok()? {
        Reply::Settings { settings } => Some(settings),
        _ => None,
    }
}

/// Call `f` with the settings on a thread: now, after every change, and
/// again after app_server restarts.
pub fn watch(f: impl Fn(Settings) + Send + 'static) {
    if tabula_protocol::socket_path().is_none() {
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
