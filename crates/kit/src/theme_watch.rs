//! Follow app_server's light/dark setting and the theme icons are drawn
//! in. Outside Olo (no `$OLO_SOCKET`) apps stay in the light palette and
//! the default (or `$OLO_ICON_THEME`) theme.

use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use olo_protocol::{Appearance, Client, Event, Reply, Request};
use olo_theme::{Palette, ThemeMode};

/// What the kit follows: the palette and the theme's id.
pub struct Look {
    pub palette: Palette,
    pub icon_theme: String,
}

impl Look {
    fn of(a: &Appearance) -> Look {
        Look { palette: Palette::for_mode(a.effective_theme), icon_theme: a.icon_theme.clone() }
    }
}

fn current() -> Option<Appearance> {
    let mut client = Client::connect().ok()?;
    match client.request(&Request::GetSettings).ok()? {
        Reply::Settings { settings } => Some(settings.appearance),
        _ => None,
    }
}

/// The palette to start in; also sets the icon theme from the settings.
pub fn initial_palette() -> Palette {
    let appearance = olo_protocol::socket_path().and_then(|_| current());
    if let Some(a) = &appearance {
        olo_xdg::themes::set(&a.icon_theme);
    }
    if let Ok(mode) = std::env::var("OLO_THEME") {
        return Palette::for_mode(if mode == "dark" { ThemeMode::Dark } else { ThemeMode::Light });
    }
    Palette::for_mode(appearance.map(|a| a.effective_theme).unwrap_or_default())
}

pub fn spawn(tx: Sender<Look>) {
    if olo_protocol::socket_path().is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            loop {
                // (Re)connected, perhaps to a restarted app_server: catch up
                // with the current theme, then follow changes.
                if let Some(a) = current()
                    && tx.send(Look::of(&a)).is_err()
                {
                    return;
                }
                if let Ok(client) = Client::connect()
                    && let Ok(events) = client.into_events()
                {
                    for event in events {
                        if let Ok(Event::SettingsChanged { settings }) = event
                            && tx.send(Look::of(&settings.appearance)).is_err()
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
