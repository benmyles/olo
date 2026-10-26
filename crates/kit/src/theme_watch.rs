//! Follow app_server's light/dark setting. Outside Tabula (no
//! `$TABULA_SOCKET`) apps stay in the light theme.

use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use tabula_protocol::{Client, Event, Reply, Request};
use tabula_theme::{Palette, ThemeMode};

fn current_mode() -> Option<ThemeMode> {
    let mut client = Client::connect().ok()?;
    match client.request(&Request::GetSettings).ok()? {
        Reply::Settings { settings } => Some(settings.appearance.effective_theme),
        _ => None,
    }
}

pub fn initial_palette() -> Palette {
    if let Ok(mode) = std::env::var("TABULA_THEME") {
        return Palette::for_mode(if mode == "dark" { ThemeMode::Dark } else { ThemeMode::Light });
    }
    Palette::for_mode(current_mode().unwrap_or_default())
}

pub fn spawn(tx: Sender<Palette>) {
    if tabula_protocol::socket_path().is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            loop {
                // (Re)connected, perhaps to a restarted app_server: catch up
                // with the current theme, then follow changes.
                if let Some(mode) = current_mode()
                    && tx.send(Palette::for_mode(mode)).is_err()
                {
                    return;
                }
                if let Ok(client) = Client::connect()
                    && let Ok(events) = client.into_events()
                {
                    for event in events {
                        if let Ok(Event::SettingsChanged { settings }) = event
                            && tx.send(Palette::for_mode(settings.appearance.effective_theme)).is_err()
                        {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}
