//! Following app_server's look: the palette (theme, colour set, light or
//! dark), in a GPUI global every window draws from, the keyboard profile
//! (for menus' keys, `olo_kit_base::keys`), and the person's
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
    Look(Box<olo_protocol::Settings>),
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
    let settings = olo_protocol::socket_path().and_then(|_| settings());
    if let Some(s) = &settings {
        olo_kit_base::keys::set(&s.keyboard.profile);
    }
    let mut a = settings.map(|s| s.appearance).unwrap_or_else(|| Appearance { colors: env("OLO_COLORS").unwrap_or_default(), ..Appearance::default() });
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
                    && tx.unbounded_send(Change::Look(Box::new(s))).is_err()
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
                            Ok(Event::SettingsChanged { settings }) => Change::Look(Box::new(settings)),
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
                    Change::Look(settings) => {
                        olo_kit_base::keys::set(&settings.keyboard.profile);
                        let a = settings.appearance;
                        tracing::debug!("look: {:?}", a.effective_theme);
                        let palette = Arc::new(a.palette());
                        if *palette == *cx.global::<Theme>().palette {
                            return;
                        }
                        olo_xdg::themes::set(&a.icon_theme);
                        cx.set_global(Theme { palette });
                    }
                    // (Displays come, go and move often: drawn again,
                    // everything afresh, only when text is to be drawn
                    // otherwise.)
                    Change::Outputs(o) if !apply_outputs(&o) => return,
                    Change::Outputs(_) => {}
                }
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

/// The person's choices of how text is drawn on each display, for
/// GPUI's windows on them: whether any changed.
fn apply_outputs(outputs: &[OutputInfo]) -> bool {
    use olo_protocol::panels::{Source, Subpixel, TextMode};
    let mut changed = false;
    for o in outputs {
        let chosen = o.panel.as_ref().filter(|p| p.text_from == Source::Setting).and_then(|p| p.text);
        let rendering = chosen.map(|t| match t {
            TextMode::Unhinted => gpui::TextRendering::Unhinted,
            TextMode::Subpixel(Subpixel::Bgr) => gpui::TextRendering::Subpixel(gpui::SubpixelLayout::Bgr),
            TextMode::Subpixel(_) => gpui::TextRendering::Subpixel(gpui::SubpixelLayout::Rgb),
            TextMode::Hinted | TextMode::Auto => gpui::TextRendering::Hinted,
        });
        changed |= gpui::set_display_text_rendering(&o.name, rendering);
    }
    changed
}
