//! The panels: names, icons, sizes, and the frame they share (content above,
//! Defaults and Revert along the bottom, as on BeOS).

use olo_kit::{
    egui::{self, Rect, pos2, vec2},
    metrics as m,
    widgets::Button,
};
use olo_protocol::Settings;

use crate::{Ctx, about, ai, appearance, backup, bluetooth, datetime, display, keyboard, network, notifications, power, screensaver, security, sound, touchid, touchpad, tracker, workspaces};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Ai,
    Appearance,
    Backgrounds,
    Backup,
    Bluetooth,
    DateTime,
    Display,
    Keyboard,
    Network,
    Notifications,
    Power,
    ScreenSaver,
    Security,
    Sound,
    TouchId,
    Touchpad,
    Tracker,
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
        Panel::Display,
        Panel::Keyboard,
        Panel::Network,
        Panel::Notifications,
        Panel::Power,
        Panel::ScreenSaver,
        Panel::Security,
        Panel::Sound,
        Panel::TouchId,
        Panel::Touchpad,
        Panel::Tracker,
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
            "wifi" | "wi-fi" | "networks" => Some(Panel::Network),
            "notification" | "dnd" | "do-not-disturb" => Some(Panel::Notifications),
            "time" | "date" | "clock" | "date-time" => Some(Panel::DateTime),
            "passwords" | "privacy" => Some(Panel::Security),
            "smart-tiling" | "models" => Some(Panel::Ai),
            _ => None,
        })
    }

    pub fn arg(self) -> &'static str {
        match self {
            Panel::Ai => "ai",
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Backup => "backup",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::Display => "display",
            Panel::Keyboard => "keyboard",
            Panel::Network => "network",
            Panel::Notifications => "notifications",
            Panel::Power => "power",
            Panel::ScreenSaver => "screensaver",
            Panel::Security => "security",
            Panel::Sound => "sound",
            Panel::TouchId => "touchid",
            Panel::Touchpad => "touchpad",
            Panel::Tracker => "tracker",
            Panel::Workspaces => "workspaces",
            Panel::About => "about",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Panel::Ai => "AI",
            Panel::Appearance => "Appearance",
            Panel::Backgrounds => "Backgrounds",
            Panel::Backup => "Backup",
            Panel::Bluetooth => "Bluetooth",
            Panel::DateTime => "Date & Time",
            Panel::Display => "Display",
            Panel::Keyboard => "Keyboard",
            Panel::Network => "Network",
            Panel::Notifications => "Notifications",
            Panel::Power => "Power",
            Panel::ScreenSaver => "Screen Saver",
            Panel::Security => "Security",
            Panel::Sound => "Sound",
            Panel::TouchId => "Touch ID",
            Panel::Touchpad => "Touchpad",
            Panel::Tracker => "Tracker",
            Panel::Workspaces => "Workspaces",
            Panel::About => "About Olo Desktop",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Panel::Ai => "ai",
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Backup => "backup",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::Display => "display",
            Panel::Keyboard => "keyboard",
            Panel::Network => "network-prefs",
            Panel::Notifications => "notifications",
            Panel::Power => "power",
            Panel::ScreenSaver => "screensaver",
            Panel::Security => "security",
            Panel::Sound => "sound",
            Panel::TouchId => "fingerprint",
            Panel::Touchpad => "touchpad",
            Panel::Tracker => "tracker",
            Panel::Workspaces => "workspaces",
            Panel::About => "about",
        }
    }

    /// Content size (logical).
    pub fn size(self) -> (u32, u32) {
        match self {
            Panel::Ai => (440, 300),
            Panel::Appearance => (420, 385),
            Panel::Backgrounds => (520, 448),
            Panel::Backup => (420, 400),
            Panel::Bluetooth => (440, 340),
            Panel::DateTime => (460, 370),
            Panel::Display => (480, 490),
            Panel::Keyboard => (480, 668),
            Panel::Network => (460, 360),
            Panel::Notifications => (440, 420),
            Panel::Power => (460, 362),
            Panel::ScreenSaver => (500, 400),
            Panel::Security => (420, 354),
            Panel::Sound => (440, 270),
            Panel::TouchId => (420, 330),
            Panel::Touchpad => (400, 382),
            Panel::Tracker => (300, 180),
            Panel::Workspaces => (360, 164),
            Panel::About => (420, 290),
        }
    }

    pub fn needs_server(self) -> bool {
        !matches!(self, Panel::About | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId | Panel::Notifications | Panel::Backup)
    }

    /// Whether the panel has Defaults / Revert.
    fn resettable(self) -> bool {
        !matches!(self, Panel::About | Panel::Display | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId | Panel::Notifications | Panel::Backup)
    }

    /// Put this panel's part of `settings` back to `from`.
    fn reset(self, settings: &mut Settings, from: &Settings) {
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
                settings.keyboard = from.keyboard.clone();
                settings.shortcuts = from.shortcuts.clone();
                settings.tiling = from.tiling.clone();
            }
            Panel::Touchpad => {
                settings.touchpad = from.touchpad.clone();
                settings.mouse_speed = from.mouse_speed;
                settings.focus_follows_mouse = from.focus_follows_mouse;
            }
            Panel::Tracker => settings.tracker = from.tracker.clone(),
            Panel::Workspaces => settings.workspace_count = from.workspace_count,
            Panel::DateTime => settings.clock = from.clock.clone(),
            Panel::Power => settings.power = from.power.clone(),
            Panel::ScreenSaver => settings.screen_saver = from.screen_saver.clone(),
            Panel::Security => settings.security = from.security.clone(),
            Panel::Ai => settings.ai = from.ai.clone(),
            Panel::Display | Panel::Backup | Panel::About | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId | Panel::Notifications => {}
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
    Network(Box<network::State>),
    Notifications(Box<notifications::State>),
    Backup(Box<backup::State>),
    Bluetooth(Box<bluetooth::State>),
    TouchId(Box<touchid::State>),
    Power(power::State),
    ScreenSaver(screensaver::State),
    About(about::State),
}

impl State {
    pub fn new(panel: Panel) -> State {
        match panel {
            Panel::Keyboard => State::Keyboard(Default::default()),
            Panel::Backgrounds => State::Backgrounds(Default::default()),
            Panel::Display => State::Display(Default::default()),
            Panel::Sound => State::Sound(Default::default()),
            Panel::DateTime => State::DateTime(Box::default()),
            Panel::Network => State::Network(Box::default()),
            Panel::Notifications => State::Notifications(Box::default()),
            Panel::Backup => State::Backup(Box::default()),
            Panel::Bluetooth => State::Bluetooth(Box::default()),
            Panel::TouchId => State::TouchId(Box::default()),
            Panel::Power => State::Power(Default::default()),
            Panel::ScreenSaver => State::ScreenSaver(Default::default()),
            Panel::About => State::About(about::State::new()),
            _ => State::None,
        }
    }
}

pub fn ui(panel: Panel, state: &mut State, ui: &mut egui::Ui, ctx: Ctx<'_>) {
    let full = ui.max_rect();
    let footer = if panel.resettable() { m::CONTROL_HEIGHT + m::WINDOW_INSET } else { 0.0 };
    let body = Rect::from_min_max(
        full.min + vec2(m::WINDOW_INSET, m::WINDOW_INSET),
        pos2(full.right() - m::WINDOW_INSET, full.bottom() - m::WINDOW_INSET - footer),
    );
    let Ctx { settings, outputs, power, opened, server } = ctx;
    ui.scope_builder(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
        ui.spacing_mut().item_spacing.y = m::SPACING;
        match (panel, state) {
            (Panel::Appearance, _) => appearance::ui(ui, settings),
            (Panel::Backgrounds, State::Backgrounds(s)) => workspaces::backgrounds_ui(ui, settings, s),
            (Panel::Display, State::Display(s)) => display::ui(ui, settings, outputs, power, server, s),
            (Panel::Keyboard, State::Keyboard(s)) => keyboard::ui(ui, settings, power, server, s),
            (Panel::Power, State::Power(s)) => power::ui(ui, settings, power, server, s),
            (Panel::ScreenSaver, State::ScreenSaver(s)) => screensaver::ui(ui, settings, server, s),
            (Panel::Sound, State::Sound(s)) => sound::ui(ui, s),
            (Panel::Security, _) => security::ui(ui, settings),
            (Panel::Ai, _) => ai::ui(ui, settings),
            (Panel::DateTime, State::DateTime(s)) => datetime::ui(ui, settings, s),
            (Panel::Network, State::Network(s)) => network::ui(ui, s),
            (Panel::Notifications, State::Notifications(s)) => notifications::ui(ui, &settings.clock, s),
            (Panel::Backup, State::Backup(s)) => backup::ui(ui, settings, s),
            (Panel::Bluetooth, State::Bluetooth(s)) => bluetooth::ui(ui, s),
            (Panel::TouchId, State::TouchId(s)) => touchid::ui(ui, s),
            (Panel::Touchpad, _) => touchpad::ui(ui, settings),
            (Panel::Tracker, _) => tracker::ui(ui, settings),
            (Panel::Workspaces, _) => workspaces::ui(ui, settings),
            (Panel::About, State::About(s)) => about::ui(ui, s),
            _ => {}
        }
    });
    if panel.resettable() {
        let y = full.bottom() - m::WINDOW_INSET - m::CONTROL_HEIGHT;
        let bw = m::BUTTON_MIN_WIDTH;
        let at = |x: f32| Rect::from_min_size(pos2(x, y), vec2(bw, m::CONTROL_HEIGHT));
        let defaults = Settings::default();
        let mut probe = settings.clone();
        panel.reset(&mut probe, &defaults);
        let is_default = probe == *settings;
        let mut probe = settings.clone();
        panel.reset(&mut probe, opened);
        let is_opened = probe == *settings;
        if ui
            .add_enabled_ui(!is_default, |ui| ui.put(at(full.left() + m::WINDOW_INSET), Button::new("Defaults")))
            .inner
            .clicked()
        {
            panel.reset(settings, &defaults);
        }
        if ui
            .add_enabled_ui(!is_opened, |ui| {
                ui.put(at(full.left() + m::WINDOW_INSET + bw + m::SPACING), Button::new("Revert"))
            })
            .inner
            .clicked()
        {
            panel.reset(settings, opened);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deskbar's menu and our panels agree on arguments, names and icons.
    #[test]
    fn deskbar_lists_every_panel() {
        let ours: Vec<(&str, &str, &str)> =
            Panel::ALL.iter().filter(|p| **p != Panel::About).map(|p| (p.arg(), p.title(), p.icon())).collect();
        assert_eq!(ours, olo_kit::apps::PREFERENCE_PANELS);
        for (arg, _, _) in olo_kit::apps::PREFERENCE_PANELS {
            assert!(Panel::from_arg(arg).is_some());
        }
    }
}
