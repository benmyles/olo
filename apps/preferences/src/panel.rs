//! The panels: names, icons, sizes, and the frame they share (content above,
//! Defaults and Revert along the bottom, as on BeOS).

use olo_protocol::Settings;
use olo_ui::{
    gpui::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, Window, div, px},
    metrics as m, style, theme,
    widgets::{Button, ScrollView, StringView},
};

use crate::{PanelView, about, ai, airpods, appearance, backup, bluetooth, datetime, default_apps, display, form::Ui, integrations, keyboard, network, notifications, performance, power, screensaver, security, sound, terminal, themes, touchid, touchpad, tracker, windows, workspaces};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Ai,
    /// Opened from Bluetooth, Sound and Deskbar (not in the launcher's
    /// grid: it's about one device).
    AirPods,
    Appearance,
    Backgrounds,
    Backup,
    Bluetooth,
    DateTime,
    DefaultApps,
    Display,
    Integrations,
    Keyboard,
    Network,
    Notifications,
    Performance,
    Power,
    ScreenSaver,
    Security,
    Sound,
    Terminal,
    Themes,
    TouchId,
    Touchpad,
    Tracker,
    Windows,
    Workspaces,
    About,
}

impl Panel {
    pub const ALL: &[Panel] = &[
        Panel::Ai,
        Panel::Appearance,
        Panel::Backgrounds,
        Panel::Backup,
        Panel::Bluetooth,
        Panel::DateTime,
        Panel::DefaultApps,
        Panel::Display,
        Panel::Integrations,
        Panel::Keyboard,
        Panel::Network,
        Panel::Notifications,
        Panel::Performance,
        Panel::Power,
        Panel::ScreenSaver,
        Panel::Security,
        Panel::Sound,
        Panel::Terminal,
        Panel::Themes,
        Panel::TouchId,
        Panel::Touchpad,
        Panel::Tracker,
        Panel::Windows,
        Panel::Workspaces,
        Panel::About,
    ];

    pub fn from_arg(s: &str) -> Option<Panel> {
        Panel::ALL.iter().copied().find(|p| p.arg() == s).or(match s {
            "mouse" | "trackpad" => Some(Panel::Touchpad),
            "screen" | "displays" => Some(Panel::Display),
            "screen-saver" | "screensavers" => Some(Panel::ScreenSaver),
            "background" => Some(Panel::Backgrounds),
            "export" | "import" | "backups" => Some(Panel::Backup),
            "fingerprint" | "touch-id" | "fingerprints" => Some(Panel::TouchId),
            "bt" => Some(Panel::Bluetooth),
            "airpods" | "airpod" | "headphones" => Some(Panel::AirPods),
            "wifi" | "wi-fi" | "networks" => Some(Panel::Network),
            "notification" | "dnd" | "do-not-disturb" => Some(Panel::Notifications),
            "supervisor" | "responsiveness" | "watchdog" => Some(Panel::Performance),
            "time" | "date" | "clock" | "date-time" => Some(Panel::DateTime),
            "open-with" | "file-types" | "filetypes" | "default-applications" => Some(Panel::DefaultApps),
            "passwords" | "privacy" => Some(Panel::Security),
            "smart-tiling" | "models" => Some(Panel::Ai),
            "github" | "telegram" | "accounts" | "services" | "integration" => Some(Panel::Integrations),
            "theme" | "icons" | "icon-theme" => Some(Panel::Themes),
            "window" | "tiling" | "margins" | "gaps" | "windowing" | "windowing-mode" | "focus-mode" => Some(Panel::Windows),
            _ => None,
        })
    }

    pub fn arg(self) -> &'static str {
        match self {
            Panel::Ai => "ai",
            Panel::AirPods => "airpods",
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Backup => "backup",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::DefaultApps => "default-apps",
            Panel::Display => "display",
            Panel::Integrations => "integrations",
            Panel::Keyboard => "keyboard",
            Panel::Network => "network",
            Panel::Notifications => "notifications",
            Panel::Performance => "performance",
            Panel::Power => "power",
            Panel::ScreenSaver => "screensaver",
            Panel::Security => "security",
            Panel::Sound => "sound",
            Panel::Terminal => "terminal",
            Panel::Themes => "themes",
            Panel::TouchId => "touchid",
            Panel::Touchpad => "touchpad",
            Panel::Tracker => "tracker",
            Panel::Windows => "windows",
            Panel::Workspaces => "workspaces",
            Panel::About => "about",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Panel::Ai => "AI",
            Panel::AirPods => "AirPods",
            Panel::Appearance => "Appearance",
            Panel::Backgrounds => "Backgrounds",
            Panel::Backup => "Backup",
            Panel::Bluetooth => "Bluetooth",
            Panel::DateTime => "Date & Time",
            Panel::DefaultApps => "Default Apps",
            Panel::Display => "Display",
            Panel::Integrations => "Integrations",
            Panel::Keyboard => "Keyboard",
            Panel::Network => "Network",
            Panel::Notifications => "Notifications",
            Panel::Performance => "Performance",
            Panel::Power => "Power",
            Panel::ScreenSaver => "Screen Saver",
            Panel::Security => "Security",
            Panel::Sound => "Sound",
            Panel::Terminal => "Terminal",
            Panel::Themes => "Themes",
            Panel::TouchId => "Touch ID",
            Panel::Touchpad => "Touchpad & Mouse",
            Panel::Tracker => "Tracker",
            Panel::Windows => "Windows",
            Panel::Workspaces => "Workspaces",
            Panel::About => "About Olo Desktop",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Panel::Ai => "ai",
            Panel::AirPods => "airpods",
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Backup => "backup",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::DefaultApps => "default-apps",
            Panel::Display => "display",
            Panel::Integrations => "integrations",
            Panel::Keyboard => "keyboard",
            Panel::Network => "network-prefs",
            Panel::Notifications => "notifications",
            Panel::Performance => "performance",
            Panel::Power => "power",
            Panel::ScreenSaver => "screensaver",
            Panel::Security => "security",
            Panel::Sound => "sound",
            Panel::Terminal => "terminal",
            Panel::Themes => "themes",
            Panel::TouchId => "fingerprint",
            Panel::Touchpad => "touchpad",
            Panel::Tracker => "tracker",
            Panel::Windows => "windows",
            Panel::Workspaces => "workspaces",
            Panel::About => "about",
        }
    }

    /// Content size (logical), on the 2-point grid (whole device pixels
    /// at 1×, 1.5× and 2×).
    pub fn size(self) -> (u32, u32) {
        match self {
            Panel::Ai => (440, 430),
            Panel::AirPods => (520, 752),
            Panel::Appearance => (420, 418),
            Panel::Backgrounds => (640, 530),
            Panel::Backup => (420, 400),
            Panel::Bluetooth => (440, 340),
            Panel::DateTime => (460, 370),
            Panel::DefaultApps => (480, 494),
            // With its panel: what it is, how Olo draws for it and why.
            Panel::Display => (500, 658),
            Panel::Integrations => (460, 600),
            Panel::Keyboard => (520, 736),
            Panel::Network => (460, 360),
            Panel::Notifications => (440, 700),
            Panel::Performance => (480, 680),
            Panel::Power => (460, 366),
            Panel::ScreenSaver => (500, 400),
            Panel::Security => (420, 340),
            Panel::Sound => (460, 440),
            Panel::Terminal => (420, 390),
            Panel::Themes => (560, 672),
            Panel::TouchId => (420, 330),
            // Tall enough for Touch with both of its notes (a pad with no
            // pressure and the Apple driver's click thresholds).
            Panel::Touchpad => (460, 586),
            Panel::Tracker => (300, 182),
            Panel::Windows => (400, 548),
            Panel::Workspaces => (360, 188),
            Panel::About => (444, 290),
        }
    }

    pub fn needs_server(self) -> bool {
        !matches!(self, Panel::About | Panel::AirPods | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId | Panel::Notifications | Panel::Performance | Panel::Backup | Panel::Integrations | Panel::DefaultApps)
    }

    /// Whether the panel has Defaults / Revert.
    fn resettable(self) -> bool {
        !matches!(self, Panel::About | Panel::AirPods | Panel::Display | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId | Panel::Notifications | Panel::Performance | Panel::Backup | Panel::Integrations | Panel::DefaultApps)
    }

    /// Put this panel's part of `settings` back to `from`.
    pub fn reset(self, settings: &mut Settings, from: &Settings) {
        match self {
            Panel::Appearance => {
                settings.appearance.theme = from.appearance.theme;
                settings.appearance.night_light = from.appearance.night_light.clone();
                settings.tab_font_size = from.tab_font_size;
                settings.tab_title_chars = from.tab_title_chars;
                settings.appearance.menu_icons = from.appearance.menu_icons;
            }
            Panel::Backgrounds => {
                settings.workspace_colors = from.workspace_colors.clone();
                settings.wallpaper = from.wallpaper.clone();
            }
            Panel::Keyboard => {
                // Profiles saved by name stay (Defaults doesn't lose them).
                let mut saved = std::mem::take(&mut settings.keyboard.profiles);
                saved.retain(|p| !from.keyboard.profiles.iter().any(|f| f.name == p.name));
                settings.keyboard = from.keyboard.clone();
                settings.keyboard.profiles.extend(saved);
            }
            Panel::Windows => {
                settings.tiling.gap = from.tiling.gap;
                settings.tiling.margin = from.tiling.margin;
                settings.tiling.default_mode = from.tiling.default_mode;
            }
            Panel::Touchpad => {
                settings.touchpad = from.touchpad.clone();
                settings.mouse_speed = from.mouse_speed;
                settings.focus_follows_mouse = from.focus_follows_mouse;
            }
            Panel::Tracker => settings.tracker = from.tracker.clone(),
            Panel::Terminal => settings.terminal = from.terminal.clone(),
            Panel::Workspaces => settings.workspace_count = from.workspace_count,
            Panel::DateTime => settings.clock = from.clock.clone(),
            Panel::Power => settings.power = from.power.clone(),
            Panel::ScreenSaver => settings.screen_saver = from.screen_saver.clone(),
            Panel::Security => settings.security = from.security.clone(),
            Panel::Ai => settings.ai = from.ai.clone(),
            Panel::Themes => {
                settings.appearance.icon_theme = from.appearance.icon_theme.clone();
                settings.appearance.colors = from.appearance.colors.clone();
                settings.wallpaper = from.wallpaper.clone();
            }
            Panel::Display | Panel::Backup | Panel::About | Panel::AirPods | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId | Panel::Notifications | Panel::Performance | Panel::Integrations | Panel::DefaultApps => {}
        }
    }
}

/// Per-window state some panels keep (selections, text being typed).
pub enum State {
    None,
    Keyboard(keyboard::State),
    Backgrounds(workspaces::BackgroundsState),
    Display(display::State),
    Sound(sound::State),
    DateTime(Box<datetime::State>),
    DefaultApps(Box<default_apps::State>),
    Network(Box<network::State>),
    Notifications(Box<notifications::State>),
    Performance(Box<performance::State>),
    Backup(Box<backup::State>),
    Bluetooth(Box<bluetooth::State>),
    AirPods(Box<airpods::State>),
    TouchId(Box<touchid::State>),
    Touchpad(touchpad::State),
    Power(power::State),
    ScreenSaver(screensaver::State),
    About(about::State),
    Integrations(Box<integrations::State>),
}

impl State {
    pub fn new(panel: Panel, window: &mut Window, cx: &mut Context<PanelView>) -> State {
        match panel {
            Panel::Keyboard => State::Keyboard(keyboard::State::new(cx)),
            Panel::Backgrounds => State::Backgrounds(Default::default()),
            Panel::Display => State::Display(Default::default()),
            Panel::Sound => State::Sound(sound::State::new(cx)),
            Panel::DateTime => State::DateTime(Box::new(datetime::State::new(cx))),
            Panel::DefaultApps => State::DefaultApps(Box::new(default_apps::State::new(cx))),
            Panel::Network => State::Network(Box::new(network::State::new(cx))),
            Panel::Notifications => State::Notifications(Box::new(notifications::State::new(cx))),
            Panel::Performance => State::Performance(Box::new(performance::State::new(cx))),
            Panel::Backup => State::Backup(Box::new(backup::State::new(cx))),
            Panel::Bluetooth => State::Bluetooth(Box::new(bluetooth::State::new(cx))),
            Panel::AirPods => State::AirPods(Box::new(airpods::State::new(cx))),
            Panel::TouchId => State::TouchId(Box::new(touchid::State::new(cx))),
            Panel::Touchpad => State::Touchpad(touchpad::State::new(cx)),
            Panel::Power => State::Power(power::State::new(cx)),
            Panel::ScreenSaver => State::ScreenSaver(screensaver::State::new(window, cx)),
            Panel::About => State::About(about::State::new(cx)),
            Panel::Integrations => State::Integrations(Box::new(integrations::State::new(cx))),
            _ => State::None,
        }
    }
}

/// A panel's window: the panel's controls, and Defaults and Revert along
/// the bottom for those that change settings (each enabled only when it
/// would change something).
pub fn render(v: &mut PanelView, window: &mut Window, cx: &mut Context<PanelView>) -> AnyElement {
    let root = layout(v, window, cx);
    // On a screen shorter than the panel its window is too (see `fits`):
    // the panel is laid out at its own size all the same and scrolls,
    // the bar beside it.
    let (w, h) = v.panel.size();
    if f32::from(window.viewport_size().height) + 0.5 >= h as f32 {
        return root.into_any_element();
    }
    let sized = div().w(px(w as f32)).h(px(h as f32)).flex().flex_col().child(root);
    ScrollView::new("panel-scroll", &v.scroll).border(false).child(sized).into_any_element()
}

fn layout(v: &mut PanelView, window: &mut Window, cx: &mut Context<PanelView>) -> olo_ui::gpui::Div {
    let panel = v.panel;
    let (settings, outputs, power, server) = {
        let p = v.prefs.read(cx);
        (p.settings.clone(), p.outputs.clone(), p.power.clone(), p.server.clone())
    };
    let root = div().size_full().flex().flex_col();
    // Not connected (yet): panels that need app_server wait.
    if settings.is_none() && panel.needs_server() {
        let p = theme::palette(cx);
        return root.items_center().justify_center().child(StringView::new("Waiting for app_server…").color(style::disabled_text(&p)));
    }
    let s = settings.unwrap_or_default();
    let prefs = v.prefs.downgrade();
    let mut ui = Ui::new(window, cx, &s, prefs, &mut v.fields);
    let power = power.as_ref();
    match (panel, &mut v.state) {
        (Panel::Appearance, _) => appearance::ui(&mut ui),
        (Panel::Backgrounds, State::Backgrounds(st)) => workspaces::backgrounds_ui(&mut ui, st),
        (Panel::Display, State::Display(st)) => display::ui(&mut ui, &outputs, power, &server, st),
        (Panel::Keyboard, State::Keyboard(st)) => keyboard::ui(&mut ui, power, &server, st),
        (Panel::Power, State::Power(st)) => power::ui(&mut ui, power, &server, st),
        (Panel::ScreenSaver, State::ScreenSaver(st)) => screensaver::ui(&mut ui, &server, st),
        (Panel::Sound, State::Sound(st)) => sound::ui(&mut ui, st),
        (Panel::Security, _) => security::ui(&mut ui),
        (Panel::Ai, _) => ai::ui(&mut ui),
        (Panel::Themes, _) => themes::ui(&mut ui),
        (Panel::DateTime, State::DateTime(st)) => datetime::ui(&mut ui, st),
        (Panel::DefaultApps, State::DefaultApps(st)) => default_apps::ui(&mut ui, st),
        (Panel::Network, State::Network(st)) => network::ui(&mut ui, st),
        (Panel::Notifications, State::Notifications(st)) => notifications::ui(&mut ui, st),
        (Panel::Performance, State::Performance(st)) => performance::ui(&mut ui, st),
        (Panel::Backup, State::Backup(st)) => backup::ui(&mut ui, st),
        (Panel::Bluetooth, State::Bluetooth(st)) => bluetooth::ui(&mut ui, st),
        (Panel::AirPods, State::AirPods(st)) => airpods::ui(&mut ui, st),
        (Panel::TouchId, State::TouchId(st)) => touchid::ui(&mut ui, st),
        (Panel::Touchpad, State::Touchpad(st)) => touchpad::ui(&mut ui, st),
        (Panel::Tracker, _) => tracker::ui(&mut ui),
        (Panel::Terminal, _) => terminal::ui(&mut ui),
        (Panel::Windows, _) => windows::ui(&mut ui, &outputs),
        (Panel::Workspaces, _) => workspaces::ui(&mut ui),
        (Panel::About, State::About(st)) => about::ui(&mut ui, st),
        (Panel::Integrations, State::Integrations(st)) => integrations::ui(&mut ui, st),
        _ => {}
    }
    let items = ui.take();
    let body = div().flex_1().min_h_0().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).children(items);
    let root = root.child(body);
    if !panel.resettable() {
        return root;
    }
    let defaults = Settings::default();
    let differs = |from: &Settings| {
        let mut probe = s.clone();
        panel.reset(&mut probe, from);
        probe != s
    };
    let (to_defaults, to_opened) = (differs(&defaults), differs(&v.opened));
    let (d, r) = (v.prefs.downgrade(), v.prefs.downgrade());
    let opened = v.opened.clone();
    let footer = div()
        .flex()
        .flex_row()
        .gap(px(m::SPACING))
        .px(px(m::WINDOW_INSET))
        .pb(px(m::WINDOW_INSET))
        .child(Button::new("Defaults").min_width(m::BUTTON_MIN_WIDTH).enabled(to_defaults).on_click(move |_, cx| {
            let _ = d.update(cx, |p, cx| p.edit(cx, |s| panel.reset(s, &Settings::default())));
        }))
        .child(Button::new("Revert").min_width(m::BUTTON_MIN_WIDTH).enabled(to_opened).on_click(move |_, cx| {
            let _ = r.update(cx, |p, cx| p.edit(cx, |s| panel.reset(s, &opened)));
        }));
    root.child(footer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deskbar's menu and our panels agree on arguments, names and icons.
    #[test]
    fn deskbar_lists_every_panel() {
        let ours: Vec<(&str, &str, &str)> =
            Panel::ALL.iter().filter(|p| **p != Panel::About).map(|p| (p.arg(), p.title(), p.icon())).collect();
        assert_eq!(ours, olo_xdg::apps::PREFERENCE_PANELS);
        for (arg, _, _) in olo_xdg::apps::PREFERENCE_PANELS {
            assert!(Panel::from_arg(arg).is_some());
        }
    }
}
