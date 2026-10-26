//! The panels: names, icons, sizes, and the frame they share (content above,
//! Defaults and Revert along the bottom, as on BeOS).

use tabula_kit::{
    egui::{self, Rect, pos2, vec2},
    metrics as m,
    widgets::Button,
};
use tabula_protocol::Settings;

use crate::{Ctx, about, appearance, bluetooth, datetime, display, keyboard, network, power, screensaver, sound, touchid, touchpad, workspaces};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Appearance,
    Backgrounds,
    Bluetooth,
    DateTime,
    Display,
    Keyboard,
    Network,
    Power,
    ScreenSaver,
    Sound,
    TouchId,
    Touchpad,
    Workspaces,
    About,
}

impl Panel {
    pub const ALL: &[Panel] = &[
        Panel::Appearance,
        Panel::Backgrounds,
        Panel::Bluetooth,
        Panel::DateTime,
        Panel::Display,
        Panel::Keyboard,
        Panel::Network,
        Panel::Power,
        Panel::ScreenSaver,
        Panel::Sound,
        Panel::TouchId,
        Panel::Touchpad,
        Panel::Workspaces,
        Panel::About,
    ];

    pub fn from_arg(s: &str) -> Option<Panel> {
        Panel::ALL.iter().copied().find(|p| p.arg() == s).or(match s {
            "mouse" | "trackpad" => Some(Panel::Touchpad),
            "screen" | "displays" => Some(Panel::Display),
            "screen-saver" | "screensavers" => Some(Panel::ScreenSaver),
            "background" => Some(Panel::Backgrounds),
            "fingerprint" | "touch-id" | "fingerprints" => Some(Panel::TouchId),
            "bt" => Some(Panel::Bluetooth),
            "wifi" | "wi-fi" | "networks" => Some(Panel::Network),
            "time" | "date" | "clock" | "date-time" => Some(Panel::DateTime),
            _ => None,
        })
    }

    pub fn arg(self) -> &'static str {
        match self {
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::Display => "display",
            Panel::Keyboard => "keyboard",
            Panel::Network => "network",
            Panel::Power => "power",
            Panel::ScreenSaver => "screensaver",
            Panel::Sound => "sound",
            Panel::TouchId => "touchid",
            Panel::Touchpad => "touchpad",
            Panel::Workspaces => "workspaces",
            Panel::About => "about",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Panel::Appearance => "Appearance",
            Panel::Backgrounds => "Backgrounds",
            Panel::Bluetooth => "Bluetooth",
            Panel::DateTime => "Date & Time",
            Panel::Display => "Display",
            Panel::Keyboard => "Keyboard",
            Panel::Network => "Network",
            Panel::Power => "Power",
            Panel::ScreenSaver => "Screen Saver",
            Panel::Sound => "Sound",
            Panel::TouchId => "Touch ID",
            Panel::Touchpad => "Touchpad",
            Panel::Workspaces => "Workspaces",
            Panel::About => "About Tabula",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::Display => "display",
            Panel::Keyboard => "keyboard",
            Panel::Network => "network-prefs",
            Panel::Power => "power",
            Panel::ScreenSaver => "screensaver",
            Panel::Sound => "sound",
            Panel::TouchId => "fingerprint",
            Panel::Touchpad => "touchpad",
            Panel::Workspaces => "workspaces",
            Panel::About => "about",
        }
    }

    /// Content size (logical).
    pub fn size(self) -> (u32, u32) {
        match self {
            Panel::Appearance => (420, 300),
            Panel::Backgrounds => (440, 308),
            Panel::Bluetooth => (440, 340),
            Panel::DateTime => (460, 370),
            Panel::Display => (480, 490),
            Panel::Keyboard => (440, 470),
            Panel::Network => (460, 360),
            Panel::Power => (460, 362),
            Panel::ScreenSaver => (500, 400),
            Panel::Sound => (440, 270),
            Panel::TouchId => (420, 330),
            Panel::Touchpad => (400, 382),
            Panel::Workspaces => (360, 164),
            Panel::About => (420, 290),
        }
    }

    pub fn needs_server(self) -> bool {
        !matches!(self, Panel::About | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId)
    }

    /// Whether the panel has Defaults / Revert.
    fn resettable(self) -> bool {
        !matches!(self, Panel::About | Panel::Display | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId)
    }

    /// Put this panel's part of `settings` back to `from`.
    fn reset(self, settings: &mut Settings, from: &Settings) {
        match self {
            Panel::Appearance => {
                settings.appearance.theme = from.appearance.theme;
                settings.appearance.night_light = from.appearance.night_light.clone();
                settings.tab_font_size = from.tab_font_size;
            }
            Panel::Backgrounds => {
                settings.workspace_colors = from.workspace_colors.clone();
                settings.wallpaper = from.wallpaper.clone();
            }
            Panel::Keyboard => settings.keyboard = from.keyboard.clone(),
            Panel::Touchpad => {
                settings.touchpad = from.touchpad.clone();
                settings.mouse_speed = from.mouse_speed;
                settings.focus_follows_mouse = from.focus_follows_mouse;
            }
            Panel::Workspaces => settings.workspace_count = from.workspace_count,
            Panel::DateTime => settings.clock = from.clock.clone(),
            Panel::Power => settings.power = from.power.clone(),
            Panel::ScreenSaver => settings.screen_saver = from.screen_saver.clone(),
            Panel::Display | Panel::About | Panel::Sound | Panel::Network | Panel::Bluetooth | Panel::TouchId => {}
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
            (Panel::DateTime, State::DateTime(s)) => datetime::ui(ui, settings, s),
            (Panel::Network, State::Network(s)) => network::ui(ui, s),
            (Panel::Bluetooth, State::Bluetooth(s)) => bluetooth::ui(ui, s),
            (Panel::TouchId, State::TouchId(s)) => touchid::ui(ui, s),
            (Panel::Touchpad, _) => touchpad::ui(ui, settings),
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
        assert_eq!(ours, tabula_kit::apps::PREFERENCE_PANELS);
        for (arg, _, _) in tabula_kit::apps::PREFERENCE_PANELS {
            assert!(Panel::from_arg(arg).is_some());
        }
    }
}
