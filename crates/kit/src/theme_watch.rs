//! Follow app_server's look: the palette (theme, colour set, light or
//! dark) and the theme icons are drawn in. Outside Olo (no
//! `$OLO_SOCKET`) apps draw in the default look, or the one
//! `$OLO_ICON_THEME`, `$OLO_COLORS` and `$OLO_THEME` (`light`, `dark`)
//! name. Also whether ⌘ is the command key, for terminal-style windows
//! ([`command_key`]), and where the app's windows are (`workspaces`).

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use olo_protocol::{Appearance, Client, Event, Reply, Request, Settings};
use olo_theme::{Palette, ThemeMode};

/// What the kit follows: the palette and the theme's id.
pub struct Look {
    pub palette: Palette,
    pub icon_theme: String,
}

impl Look {
    fn of(a: &Appearance) -> Look {
        Look { palette: a.palette(), icon_theme: a.icon_theme.clone() }
    }
}

static COMMAND_KEY: AtomicBool = AtomicBool::new(false);

/// ⌘ is the command key (`KeyboardSettings::command_key`), so app_server
/// sends it to terminals as its right Ctrl with Shift and keeps the left
/// Ctrl for the physical key: in a terminal-style window only the right
/// one is the command key. False outside Olo, where Ctrl is.
pub fn command_key() -> bool {
    COMMAND_KEY.load(Ordering::Relaxed)
}

fn current() -> Option<Settings> {
    let mut client = Client::connect().ok()?;
    match client.request(&Request::GetSettings).ok()? {
        Reply::Settings { settings } => {
            COMMAND_KEY.store(settings.keyboard.command_key, Ordering::Relaxed);
            Some(settings)
        }
        _ => None,
    }
}

/// The palette to start in; also sets the icon theme from the settings.
pub fn initial_palette() -> Palette {
    let appearance = olo_protocol::socket_path().and_then(|_| current()).map(|s| s.appearance);
    if let Some(a) = &appearance {
        olo_xdg::themes::set(&a.icon_theme);
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut a = appearance.unwrap_or_else(|| Appearance {
        icon_theme: env("OLO_ICON_THEME").unwrap_or_else(|| olo_xdg::themes::DEFAULT.into()),
        colors: env("OLO_COLORS").unwrap_or_default(),
        ..Appearance::default()
    });
    if let Some(mode) = env("OLO_THEME") {
        a.effective_theme = if mode == "dark" { ThemeMode::Dark } else { ThemeMode::Light };
    }
    a.palette()
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
                if let Some(s) = current()
                    && tx.send(Look::of(&s.appearance)).is_err()
                {
                    return;
                }
                if let Ok(client) = Client::connect()
                    && let Ok(events) = client.into_events()
                {
                    // Where our windows are, then as it changes.
                    crate::workspaces::catch_up();
                    for event in events {
                        match event {
                            Ok(Event::SettingsChanged { settings }) => {
                                COMMAND_KEY.store(settings.keyboard.command_key, Ordering::Relaxed);
                                if tx.send(Look::of(&settings.appearance)).is_err() {
                                    return;
                                }
                            }
                            Ok(Event::WindowsChanged { windows }) => crate::workspaces::windows_changed(&windows),
                            Ok(Event::WorkspaceChanged { current, displays, .. }) => crate::workspaces::workspace_changed(current, displays),
                            _ => {}
                        }
                    }
                    crate::workspaces::forget();
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}
