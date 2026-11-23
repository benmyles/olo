//! The panels: names, icons, sizes, and the frame they share (content above,
//! Defaults and Revert along the bottom, as on BeOS).

use olo_kit::{
    Cx, Element, ParentElement as _, Styled as _, div, metrics as m, px, style, theme,
    widgets::{Button, StringView},
};
use olo_protocol::Settings;

use crate::{
    PanelView, about, ai, airplay, airpods, appearance, backup, bluetooth, datetime, default_apps, deskbar, display, form::Ui, integrations, keyboard, network, notifications, performance, phone,
    power, prefs, rex, screensaver, security, sound, terminal, themes, touchpad, tracker, windows, workspaces,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Ai,
    /// The receivers: pairing, checking, forgetting, showing the display.
    AirPlay,
    /// Opened from Bluetooth, Sound and Deskbar (not in the launcher's
    /// grid: it's about one device).
    AirPods,
    Appearance,
    Backgrounds,
    Backup,
    Bluetooth,
    DateTime,
    DefaultApps,
    /// Deskbar's live widgets.
    Deskbar,
    Display,
    /// Each GPU's tuning.
    Integrations,
    Keyboard,
    /// Olo's language (and, later, the formats).
    Language,
    Network,
    Notifications,
    Performance,
    Phone,
    Power,
    Rex,
    ScreenSaver,
    Security,
    Sound,
    Terminal,
    Themes,
    Touchpad,
    Tracker,
    Windows,
    Workspaces,
    About,
}

impl Panel {
    pub const ALL: &[Panel] = &[
        Panel::Ai,
        Panel::AirPlay,
        Panel::Appearance,
        Panel::Backgrounds,
        Panel::Backup,
        Panel::Bluetooth,
        Panel::DateTime,
        Panel::DefaultApps,
        Panel::Deskbar,
        Panel::Display,
        Panel::Integrations,
        Panel::Keyboard,
        Panel::Language,
        Panel::Network,
        Panel::Notifications,
        Panel::Performance,
        Panel::Phone,
        Panel::Power,
        Panel::Rex,
        Panel::ScreenSaver,
        Panel::Security,
        Panel::Sound,
        Panel::Terminal,
        Panel::Themes,
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
            // (GPUs were a panel of their own: Power's Graphics now.)
            "gpus" | "gpu" | "graphics" | "nvidia" => Some(Panel::Power),
            "screen-saver" | "screensavers" => Some(Panel::ScreenSaver),
            "background" => Some(Panel::Backgrounds),
            "export" | "import" | "backups" => Some(Panel::Backup),
            // (Touch ID was a panel of its own: Security's Sign In now,
            // with the face and security keys.)
            "touchid" | "fingerprint" | "touch-id" | "fingerprints" | "face" | "face-unlock" | "security-keys" | "yubikey" | "sign-in" => Some(Panel::Security),
            "bt" => Some(Panel::Bluetooth),
            "airpods" | "airpod" | "headphones" => Some(Panel::AirPods),
            "iphone" | "messages" | "sms" | "codes" | "one-time-codes" | "blueferry" => Some(Panel::Phone),
            "air-play" | "mirroring" | "screen-mirroring" | "apple-tv" | "tv" => Some(Panel::AirPlay),
            "wifi" | "wi-fi" | "networks" => Some(Panel::Network),
            "notification" | "dnd" | "do-not-disturb" => Some(Panel::Notifications),
            "supervisor" | "responsiveness" | "watchdog" | "pointer-thread" | "input-thread" | "kernel" | "kernel-tuning" | "writeback" | "dirty-writeback" | "nvme" | "nvme-scheduler" => {
                Some(Panel::Performance)
            }
            "time" | "date" | "clock" | "date-time" => Some(Panel::DateTime),
            "languages" | "locale" | "region" | "translation" | "translations" | "language-region" => Some(Panel::Language),
            "open-with" | "file-types" | "filetypes" | "default-applications" => Some(Panel::DefaultApps),
            "passwords" | "privacy" => Some(Panel::Security),
            "smart-tiling" | "models" => Some(Panel::Ai),
            "github" | "telegram" | "fastmail" | "caldav" | "accounts" | "services" | "integration" => Some(Panel::Integrations),
            "theme" | "icons" | "icon-theme" => Some(Panel::Themes),
            "window" | "tiling" | "margins" | "gaps" | "windowing" | "windowing-mode" | "focus-mode" => Some(Panel::Windows),
            _ => None,
        })
    }

    pub fn arg(self) -> &'static str {
        match self {
            Panel::Ai => "ai",
            Panel::AirPlay => "airplay",
            Panel::AirPods => "airpods",
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Backup => "backup",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::DefaultApps => "default-apps",
            Panel::Deskbar => "deskbar",
            Panel::Display => "display",
            Panel::Integrations => "integrations",
            Panel::Keyboard => "keyboard",
            Panel::Language => "language",
            Panel::Network => "network",
            Panel::Notifications => "notifications",
            Panel::Performance => "performance",
            Panel::Phone => "phone",
            Panel::Power => "power",
            Panel::Rex => "rex",
            Panel::ScreenSaver => "screensaver",
            Panel::Security => "security",
            Panel::Sound => "sound",
            Panel::Terminal => "terminal",
            Panel::Themes => "themes",
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
            Panel::AirPlay => "AirPlay",
            Panel::AirPods => "AirPods",
            Panel::Appearance => "Appearance",
            Panel::Backgrounds => "Backgrounds",
            Panel::Backup => "Backup",
            Panel::Bluetooth => "Bluetooth",
            Panel::DateTime => "Date & Time",
            Panel::DefaultApps => "Default Apps",
            Panel::Deskbar => "Deskbar",
            Panel::Display => "Display",
            Panel::Integrations => "Integrations",
            Panel::Keyboard => "Keyboard",
            Panel::Language => "Language & Region",
            Panel::Network => "Network",
            Panel::Notifications => "Notifications",
            Panel::Performance => "Performance",
            Panel::Phone => "Phone",
            Panel::Power => "Power",
            Panel::Rex => "Rex",
            Panel::ScreenSaver => "Screen Saver",
            Panel::Security => "Security",
            Panel::Sound => "Sound",
            Panel::Terminal => "Terminal",
            Panel::Themes => "Themes",
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
            Panel::AirPlay => "airplay",
            Panel::AirPods => "airpods",
            Panel::Appearance => "appearance",
            Panel::Backgrounds => "backgrounds",
            Panel::Backup => "backup",
            Panel::Bluetooth => "bluetooth",
            Panel::DateTime => "datetime",
            Panel::DefaultApps => "default-apps",
            // (Its own icon to come: generated against the reference set,
            // which needs the image model's key.)
            Panel::Deskbar => "deskbar",
            Panel::Display => "display",
            Panel::Integrations => "integrations",
            Panel::Keyboard => "keyboard",
            Panel::Language => "language",
            Panel::Network => "network-prefs",
            Panel::Notifications => "notifications",
            Panel::Performance => "performance",
            Panel::Phone => "phone",
            Panel::Power => "power",
            Panel::Rex => "rex",
            Panel::ScreenSaver => "screensaver",
            Panel::Security => "security",
            Panel::Sound => "sound",
            Panel::Terminal => "terminal",
            Panel::Themes => "themes",
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
            Panel::Ai => (1100, 408),
            // The receivers (their names, kinds and how each is), pairing's
            // code beneath while a receiver shows one.
            Panel::AirPlay => (500, 360),
            Panel::AirPods => (960, 622),
            Panel::Appearance => (420, 482),
            Panel::Backgrounds => (760, 720),
            // Export and Import side by side, each with its list of what
            // goes in (or comes back) and what an import left behind.
            Panel::Backup => (920, 400),
            Panel::Bluetooth => (560, 340),
            Panel::DateTime => (460, 370),
            Panel::DefaultApps => (520, 662),
            // The widgets beside the chosen one's settings.
            Panel::Deskbar => (760, 470),
            // With its panel beside it: what it is, how Olo draws for it
            // and why. As tall as the display's rows (`Fit`).
            Panel::Display => (940, 520),
            // GitHub and Mail beside Telegram.
            Panel::Integrations => (900, 480),
            Panel::Keyboard => (520, 736),
            Panel::Language => (460, 240),
            // Eight tabs, each name whole in the language shown (the
            // window wider for them, `network::least_width`); Hosts' four
            // columns; Shaping's two ways side by side.
            Panel::Network => (crate::network::least_width().max(640), 420),
            Panel::Notifications => (440, 700),
            // Its Watchdog tab's thresholds two by two, each a pair of sliders.
            Panel::Performance => (900, 560),
            // How the phone stands, then what's told of beside where
            // codes go.
            Panel::Phone => (820, 464),
            // Power follows its two forms and their disclosures.
            Panel::Power => (900, 200),
            // The measured two-column form, including both inline messages.
            // The stacked layout grows to fit on a narrower display.
            Panel::Rex => (816, 584),
            Panel::ScreenSaver => (500, 400),
            // Passwords' settings, security keys beside them.
            Panel::Security => (880, 440),
            // As tall as its devices need, which is a machine's own (no
            // list in it shrinks): the height is only its least. The
            // lint's test devices, the most a machine shows, need 466; a
            // machine with fewer kept that, empty below (Ben's).
            Panel::Sound => (800, 160),
            // Coding Agents beside Git Worktrees and Command Suggestions.
            Panel::Terminal => (900, 600),
            // The themes, the chosen one's colour sets beside them.
            Panel::Themes => (824, 600),
            // Tall enough for Touch with both of its notes (a pad with no
            // pressure and the Apple driver's click thresholds).
            Panel::Touchpad => (784, 600),
            Panel::Tracker => (300, 222),
            Panel::Windows => (760, 420),
            Panel::Workspaces => (360, 230),
            Panel::About => (444, 392),
        }
    }

    pub fn needs_server(self) -> bool {
        !matches!(
            self,
            Panel::About
                | Panel::AirPlay
                | Panel::AirPods
                | Panel::Sound
                | Panel::Network
                | Panel::Bluetooth
                | Panel::Notifications
                | Panel::Performance
                | Panel::Phone
                | Panel::Backup
                | Panel::Integrations
                | Panel::DefaultApps
        )
    }

    /// Whether the panel has Defaults / Revert.
    pub fn resettable(self) -> bool {
        !matches!(
            self,
            Panel::About
                | Panel::AirPlay
                | Panel::AirPods
                | Panel::Display
                | Panel::Sound
                | Panel::Network
                | Panel::Bluetooth
                | Panel::Notifications
                | Panel::Performance
                | Panel::Phone
                | Panel::Backup
                | Panel::Integrations
                | Panel::DefaultApps
        )
    }

    /// Defaults or Revert pressed (`what`): this panel's settings put
    /// back to `from`'s, and what a change of them tells others told
    /// (the language, to the programs Olo starts).
    pub fn reset_pressed(self, what: &'static str, from: &Settings) {
        prefs().edit_as("", what, |s| self.reset(s, from));
        if self == Panel::Language {
            crate::language::tell_others(&from.language);
        }
    }

    /// Put this panel's part of `settings` back to `from`, and nothing
    /// else: it's also how the panel asks whether that would change
    /// anything (at every drawing).
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
                settings.tiling.layout = from.tiling.layout;
                settings.tiling.floating_apps = from.tiling.floating_apps.clone();
            }
            Panel::Touchpad => {
                settings.touchpad = from.touchpad.clone();
                settings.mouse_speed = from.mouse_speed;
                settings.focus_follows_mouse = from.focus_follows_mouse;
            }
            Panel::Rex => settings.rex = from.rex.clone(),
            Panel::Tracker => settings.tracker = from.tracker.clone(),
            Panel::Terminal => settings.terminal = from.terminal.clone(),
            Panel::Workspaces => settings.workspace_count = from.workspace_count,
            Panel::DateTime => settings.clock = from.clock.clone(),
            // (Only the setting: `reset` is also how the panel asks, at
            // every drawing, whether Defaults and Revert would change
            // anything, and it told other programs the language each
            // time, the default's then the opened one's, on two threads:
            // looking at Language & Region could leave locale.conf saying
            // English. Said where they're pressed: `reset_pressed`.)
            Panel::Language => settings.language = from.language.clone(),
            Panel::Deskbar => settings.deskbar = from.deskbar.clone(),
            Panel::Power => settings.power = from.power.clone(),
            Panel::ScreenSaver => settings.screen_saver = from.screen_saver.clone(),
            Panel::Security => settings.security = from.security.clone(),
            Panel::Ai => settings.ai = from.ai.clone(),
            Panel::Themes => {
                settings.appearance.look = from.appearance.look.clone();
                settings.appearance.custom = from.appearance.custom.clone();
                settings.wallpaper = from.wallpaper.clone();
            }
            Panel::Display
            | Panel::Backup
            | Panel::About
            | Panel::AirPlay
            | Panel::AirPods
            | Panel::Sound
            | Panel::Network
            | Panel::Bluetooth
            | Panel::Notifications
            | Panel::Performance
            | Panel::Phone
            | Panel::Integrations
            | Panel::DefaultApps => {}
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
    Deskbar(Box<deskbar::State>),
    Network(Box<network::State>),
    Notifications(Box<notifications::State>),
    Performance(Box<performance::State>),
    Phone(phone::State),
    Backup(Box<backup::State>),
    Bluetooth(Box<bluetooth::State>),
    AirPods(Box<airpods::State>),
    AirPlay(Box<airplay::State>),
    Touchpad(touchpad::State),
    Power(power::State),
    Rex(rex::State),
    ScreenSaver(screensaver::State),
    About(about::State),
    Integrations(Box<integrations::State>),
    Security(Box<security::State>),
    Themes(Box<themes::State>),
}

impl State {
    pub fn new(panel: Panel, cx: &mut Cx<PanelView>) -> State {
        match panel {
            Panel::Keyboard => State::Keyboard(keyboard::State::new(cx)),
            Panel::Backgrounds => State::Backgrounds(Default::default()),
            Panel::Display => State::Display(Default::default()),
            Panel::Sound => State::Sound(sound::State::new(cx)),
            Panel::DateTime => State::DateTime(Box::new(datetime::State::new(cx))),
            Panel::DefaultApps => State::DefaultApps(Box::new(default_apps::State::new(cx))),
            Panel::Deskbar => State::Deskbar(Box::new(deskbar::State::new(cx))),
            Panel::Themes => State::Themes(Box::new(themes::State::new())),
            Panel::Network => State::Network(Box::new(network::State::new(cx))),
            Panel::Notifications => State::Notifications(Box::new(notifications::State::new(cx))),
            Panel::Performance => State::Performance(Box::new(performance::State::new(cx))),
            Panel::Phone => State::Phone(phone::State::new()),
            Panel::Backup => State::Backup(Box::new(backup::State::new(cx))),
            Panel::Bluetooth => State::Bluetooth(Box::new(bluetooth::State::new(cx))),
            Panel::AirPods => State::AirPods(Box::new(airpods::State::new(cx))),
            Panel::AirPlay => State::AirPlay(Box::new(airplay::State::new(cx))),
            Panel::Touchpad => State::Touchpad(touchpad::State::new(cx)),
            Panel::Rex => State::Rex(rex::State::default()),
            Panel::Power => State::Power(power::State::new(cx)),
            Panel::ScreenSaver => State::ScreenSaver(screensaver::State::new(cx)),
            Panel::About => State::About(about::State::new(cx)),
            Panel::Integrations => State::Integrations(Box::new(integrations::State::new(cx))),
            Panel::Security => State::Security(Box::new(security::State::new(cx))),
            _ => State::None,
        }
    }
}

/// A panel's window: the panel's controls, and Defaults and Revert along
/// the bottom for those that change settings (each enabled only when it
/// would change something).
pub fn render(v: &mut PanelView, cx: &mut Cx<PanelView>) -> Element {
    // (Its window follows what it shows: `WindowOptions::fits_content`.)
    let panel = v.panel;
    let (settings, outputs, outputs_in, power, server) = {
        let p = prefs();
        (p.settings.clone(), p.outputs.clone(), p.outputs_in, p.power.clone(), p.server.clone())
    };
    // (Not shown until what it shows from app_server is in, briefly: else
    // it opens waiting, or without its displays, and grows.)
    if ((settings.is_none() && panel.needs_server()) || (panel == Panel::Display && !outputs_in))
        && let Some(hold) = olo_kit::hold_first_show(cx)
    {
        let mut p = prefs();
        // (In since it was looked at: nothing to wait for.)
        if !(p.settings.is_some() && p.outputs_in) {
            p.holds.push(hold);
        }
    }
    let root = div().size_full().flex().flex_col();
    // Not connected (yet): panels that need app_server wait.
    if settings.is_none() && panel.needs_server() {
        let p = theme::palette();
        return root.items_center().justify_center().child(StringView::new("Waiting for app_server…").color(style::disabled_text(&p)));
    }
    let s = settings.unwrap_or_default();
    // Power edits are immediate. Per-setting undo is available from Edit.
    let resettable = panel.resettable() && !matches!(&v.state, State::Power(_));
    let items = {
        let mut ui = Ui::new(cx, &s, &mut v.fields);
        ui.resets(resettable);
        let power = power.as_ref();
        match (panel, &mut v.state) {
            (Panel::Appearance, _) => appearance::ui(&mut ui),
            (Panel::Backgrounds, State::Backgrounds(st)) => workspaces::backgrounds_ui(&mut ui, st),
            (Panel::Display, State::Display(st)) => display::ui(&mut ui, &outputs, power, &server, st),
            (Panel::Keyboard, State::Keyboard(st)) => keyboard::ui(&mut ui, power, &server, st),
            (Panel::Power, State::Power(st)) => power::ui(&mut ui, power, &server, st),
            (Panel::ScreenSaver, State::ScreenSaver(st)) => screensaver::ui(&mut ui, &server, st, power.is_some_and(|p| p.policy.on_battery)),
            (Panel::Sound, State::Sound(st)) => sound::ui(&mut ui, st),
            (Panel::Security, State::Security(st)) => security::ui(&mut ui, st),
            (Panel::Ai, _) => ai::ui(&mut ui),
            (Panel::Language, _) => crate::language::ui(&mut ui),
            (Panel::Themes, State::Themes(st)) => themes::ui(&mut ui, st),
            (Panel::DateTime, State::DateTime(st)) => datetime::ui(&mut ui, st),
            (Panel::DefaultApps, State::DefaultApps(st)) => default_apps::ui(&mut ui, st),
            (Panel::Deskbar, State::Deskbar(st)) => deskbar::ui(&mut ui, st),
            (Panel::Network, State::Network(st)) => network::ui(&mut ui, st),
            (Panel::Notifications, State::Notifications(st)) => notifications::ui(&mut ui, st),
            (Panel::Performance, State::Performance(st)) => performance::ui(&mut ui, st),
            (Panel::Phone, State::Phone(st)) => phone::ui(&mut ui, st),
            (Panel::Backup, State::Backup(st)) => backup::ui(&mut ui, st),
            (Panel::Bluetooth, State::Bluetooth(st)) => bluetooth::ui(&mut ui, st),
            (Panel::AirPods, State::AirPods(st)) => airpods::ui(&mut ui, st),
            (Panel::AirPlay, State::AirPlay(st)) => airplay::ui(&mut ui, st),
            (Panel::Touchpad, State::Touchpad(st)) => touchpad::ui(&mut ui, st),
            (Panel::Rex, State::Rex(st)) => rex::ui(&mut ui, st),
            (Panel::Tracker, _) => tracker::ui(&mut ui),
            (Panel::Terminal, _) => terminal::ui(&mut ui),
            (Panel::Windows, _) => windows::ui(&mut ui, &outputs),
            (Panel::Workspaces, _) => workspaces::ui(&mut ui),
            (Panel::About, State::About(st)) => about::ui(&mut ui, st),
            (Panel::Integrations, State::Integrations(st)) => integrations::ui(&mut ui, st),
            _ => {}
        }
        ui.take()
    };
    // (Never shorter than what's in it: the window grows to it, and
    // too much for the screen pushes Defaults and Revert down, where the
    // window scrolls and the layout lint sees it. A list in it shrinks and
    // scrolls.)
    let body = div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).children(items);
    let root = root.child(body);
    if !resettable {
        return root;
    }

    let defaults = Settings::default();
    let differs = |from: &Settings| {
        let mut probe = s.clone();
        panel.reset(&mut probe, from);
        probe != s
    };
    // (While a shortcut's keys are typed, only Esc is the way out: they'd
    // throw the change away under the hand typing it.)
    let typing = matches!(&v.state, State::Keyboard(k) if k.shortcuts.recording());
    let (to_defaults, to_opened) = (differs(&defaults) && !typing, differs(&v.opened) && !typing);
    let opened = v.opened.clone();
    let footer = div()
        .flex()
        .flex_row()
        .gap(px(m::SPACING))
        .px(px(m::WINDOW_INSET))
        .pb(px(m::WINDOW_INSET))
        .child(Button::new("Defaults").min_width(m::BUTTON_MIN_WIDTH).enabled(to_defaults).on_click(move |_, _| panel.reset_pressed("Defaults", &Settings::default())))
        .child(Button::new("Revert").min_width(m::BUTTON_MIN_WIDTH).enabled(to_opened).on_click(move |_, _| panel.reset_pressed("Revert", &opened)));
    root.child(footer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deskbar's menu and our panels agree on arguments, names and icons.
    #[test]
    fn deskbar_lists_every_panel() {
        let ours: Vec<(&str, &str, &str)> = Panel::ALL.iter().filter(|p| **p != Panel::About).map(|p| (p.arg(), p.title(), p.icon())).collect();
        assert_eq!(ours, olo_xdg::apps::PREFERENCE_PANELS);
        for (arg, _, _) in olo_xdg::apps::PREFERENCE_PANELS {
            assert!(Panel::from_arg(arg).is_some());
        }
    }
}
