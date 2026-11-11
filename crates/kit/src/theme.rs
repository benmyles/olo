//! Following app_server's look: the palette (theme, colour set, light or
//! dark) every window draws from, the keyboard profile (for menus'
//! keys, `olo_kit_base::keys`), and the person's choice of how text is
//! drawn on each display (Preferences ▸ Display,
//! `olo_protocol::panels`). A thread listens for app_server's events (it
//! sleeps until one comes); when the look changes, every window draws
//! itself again. Outside Olo (no `$OLO_SOCKET`), the default look, or
//! the one `$OLO_COLORS` and `$OLO_THEME` (`light`, `dark`) name.

use std::{sync::Arc, time::Duration};

use olo_protocol::{Appearance, Client, Event, EventKind, OutputInfo, Reply, Request};
use olo_sync::RwLock;
use olo_theme::{Palette, ThemeMode};

static PALETTE: RwLock<Option<Arc<Palette>>> = RwLock::new(None);

/// The palette windows draw in. From any thread: a looper reads it as it
/// draws, and is told to draw again when it changes.
pub fn palette() -> Arc<Palette> {
    if let Some(palette) = PALETTE.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return palette.clone();
    }
    // (Asked before the program has started following the look: the
    // look it starts with.)
    let palette = Arc::new(initial().palette());
    PALETTE.write().unwrap_or_else(|e| e.into_inner()).get_or_insert(palette).clone()
}

pub(crate) fn set_palette(palette: Arc<Palette>) -> bool {
    let mut slot = PALETTE.write().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_some_and(|p| **p == *palette) {
        return false;
    }
    *slot = Some(palette);
    true
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

/// Sets the look up and follows app_server's.
pub(crate) fn init() {
    let appearance = initial();
    olo_xdg::themes::set(&appearance.icon_theme);
    set_palette(Arc::new(appearance.palette()));
    if let Some(outputs) = olo_protocol::socket_path().and_then(|_| outputs()) {
        apply_outputs(&outputs);
    }
    if olo_protocol::socket_path().is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            loop {
                // (Re)connected, perhaps to a restarted app_server: catch
                // up, then follow.
                if let Some(s) = settings() {
                    look(s);
                }
                if let Some(o) = outputs()
                    && apply_outputs(&o)
                {
                    olo_interface::refresh_all_windows();
                }
                if let Ok(client) = Client::connect()
                    && let Ok(events) = client.into_events_of(&[EventKind::Settings, EventKind::Outputs])
                {
                    for event in events {
                        match event {
                            Ok(Event::SettingsChanged { settings }) => look(settings),
                            // (Displays come, go and move often: drawn
                            // again, everything afresh, only when text is
                            // to be drawn otherwise.)
                            Ok(Event::OutputsChanged { outputs }) => {
                                if apply_outputs(&outputs) {
                                    olo_interface::refresh_all_windows();
                                }
                            }
                            _ => continue,
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}

/// The settings as they are now: the windows draw again if the look
/// changed.
fn look(settings: olo_protocol::Settings) {
    olo_kit_base::keys::set(&settings.keyboard.profile);
    let a = settings.appearance;
    tracing::debug!("look: {:?}", a.effective_theme);
    if set_palette(Arc::new(a.palette())) {
        olo_xdg::themes::set(&a.icon_theme);
        olo_interface::refresh_all_windows();
    }
}

/// The person's choices of how text is drawn on each display, for the
/// windows on them: whether any changed.
fn apply_outputs(outputs: &[OutputInfo]) -> bool {
    use olo_interface::{SubpixelLayout, TextRendering};
    use olo_protocol::panels::{Source, Subpixel, TextMode};
    let mut changed = false;
    for o in outputs {
        let chosen = o.panel.as_ref().filter(|p| p.text_from == Source::Setting).and_then(|p| p.text);
        let rendering = chosen.map(|t| match t {
            TextMode::Unhinted => TextRendering::Unhinted,
            TextMode::Subpixel(Subpixel::Bgr) => TextRendering::Subpixel(SubpixelLayout::Bgr),
            TextMode::Subpixel(_) => TextRendering::Subpixel(SubpixelLayout::Rgb),
            TextMode::Hinted | TextMode::Auto => TextRendering::Hinted,
        });
        changed |= olo_interface::set_display_text_rendering(&o.name, rendering);
        // (Windows on it may draw HDR pictures as HDR.)
        olo_interface::set_display_hdr(&o.name, o.hdr == Some(true));
    }
    changed
}
