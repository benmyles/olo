//! Following app_server's look: the palette (theme, colour set, light or
//! dark), in a GPUI global every window draws from, and the person's
//! choice of how text is drawn on each display (Preferences ▸ Display,
//! `olo_protocol::panels`). A thread listens for app_server's events (it
//! sleeps until one comes) and hands them to the UI thread, which
//! changes the global and redraws the windows. Outside Olo (no
//! `$OLO_SOCKET`), the default look, or the one `$OLO_COLORS` and
//! `$OLO_THEME` (`light`, `dark`) name.

use std::{sync::Arc, time::Duration};

use futures::{StreamExt, channel::mpsc};
use gpui::{App, Global};
use olo_protocol::{Appearance, Client, Event, EventKind, OutputInfo, Reply, Request};
use olo_theme::{Palette, ThemeMode};

/// The look in effect.
pub struct Theme {
    pub palette: Arc<Palette>,
}

impl Global for Theme {}

/// The palette windows draw in.
pub fn palette(cx: &App) -> Arc<Palette> {
    cx.global::<Theme>().palette.clone()
}

enum Change {
    Look(Appearance),
    Outputs(Vec<OutputInfo>),
}

fn settings() -> Option<olo_protocol::Settings> {
    let mut client = Client::connect().ok()?;
    match client.request(&Request::GetSettings).ok()? {
        Reply::Settings { settings } => Some(settings),
        _ => None,
    }
}

fn outputs() -> Option<Vec<OutputInfo>> {
    let mut client = Client::connect().ok()?;
    match client.request(&Request::Outputs).ok()? {
        Reply::Outputs { outputs } => Some(outputs),
        _ => None,
    }
}

fn initial() -> Appearance {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut a = olo_protocol::socket_path().and_then(|_| settings()).map(|s| s.appearance).unwrap_or_else(|| Appearance {
        colors: env("OLO_COLORS").unwrap_or_default(),
        ..Appearance::default()
    });
    if let Some(mode) = env("OLO_THEME") {
        a.effective_theme = if mode == "dark" { ThemeMode::Dark } else { ThemeMode::Light };
    }
    a
}

/// Sets the theme up and follows app_server's.
pub(crate) fn init(cx: &mut App) {
    let appearance = initial();
    olo_xdg::themes::set(&appearance.icon_theme);
    cx.set_global(Theme { palette: Arc::new(appearance.palette()) });
    if let Some(outputs) = olo_protocol::socket_path().and_then(|_| outputs()) {
        apply_outputs(&outputs);
    }
    if olo_protocol::socket_path().is_none() {
        return;
    }
    let (tx, mut rx) = mpsc::unbounded::<Change>();
    std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            loop {
                // (Re)connected, perhaps to a restarted app_server: catch
                // up, then follow.
                if let Some(s) = settings()
                    && tx.unbounded_send(Change::Look(s.appearance)).is_err()
                {
                    return;
                }
                if let Some(o) = outputs()
                    && tx.unbounded_send(Change::Outputs(o)).is_err()
                {
                    return;
                }
                if let Ok(client) = Client::connect()
                    && let Ok(events) = client.into_events_of(&[EventKind::Settings, EventKind::Outputs])
                {
                    for event in events {
                        let change = match event {
                            Ok(Event::SettingsChanged { settings }) => Change::Look(settings.appearance),
                            Ok(Event::OutputsChanged { outputs }) => Change::Outputs(outputs),
                            _ => continue,
                        };
                        if tx.unbounded_send(change).is_err() {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
    cx.spawn(async move |cx| {
        while let Some(change) = rx.next().await {
            let _ = cx.update(|cx| {
                match change {
                    Change::Look(a) => {
                        let palette = Arc::new(a.palette());
                        if *palette == *cx.global::<Theme>().palette {
                            return;
                        }
                        olo_xdg::themes::set(&a.icon_theme);
                        cx.set_global(Theme { palette });
                    }
                    Change::Outputs(o) => apply_outputs(&o),
                }
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

/// The person's choices of how text is drawn on each display, for
/// GPUI's windows on them.
fn apply_outputs(outputs: &[OutputInfo]) {
    use olo_protocol::panels::{Source, Subpixel, TextMode};
    for o in outputs {
        let chosen = o.panel.as_ref().filter(|p| p.text_from == Source::Setting).and_then(|p| p.text);
        let rendering = chosen.map(|t| match t {
            TextMode::Unhinted => gpui::TextRendering::Unhinted,
            TextMode::Subpixel(Subpixel::Bgr) => gpui::TextRendering::Subpixel(gpui::SubpixelLayout::Bgr),
            TextMode::Subpixel(_) => gpui::TextRendering::Subpixel(gpui::SubpixelLayout::Rgb),
            TextMode::Hinted | TextMode::Auto => gpui::TextRendering::Hinted,
        });
        gpui::set_display_text_rendering(&o.name, rendering);
    }
}
