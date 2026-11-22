//! Complete persisted settings for Olo Rex (`~/.config/olo/rex.json`),
//! covering full Rex macOS parity plus Olo desktop integration policies.

use std::{collections::HashMap, path::PathBuf};

use olo_rex_client::KeymapLayer;
use olo_rex_vt::{CursorStyle, Rgb, TerminalTheme};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolbarLayout {
    #[default]
    Above,
    Below,
    LeftSidebar,
    RightSidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BarBehavior {
    #[default]
    Always,
    Hover,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarMode {
    #[default]
    Sessions,
    Activity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ActivityGroupBy {
    #[default]
    Priority,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SchemeChoice {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TabColorMode {
    #[default]
    Theme,
    Adaptive,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HighlightStyle {
    #[default]
    Fade,
    Move,
    Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AnimationStyle {
    #[default]
    Snappy,
    Natural,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorShapeSetting {
    #[default]
    Block,
    Bar,
    Underline,
    BlockHollow,
}

impl CursorShapeSetting {
    pub fn to_vt(self) -> CursorStyle {
        match self {
            Self::Block => CursorStyle::Block,
            Self::Bar => CursorStyle::Bar,
            Self::Underline => CursorStyle::Underline,
            Self::BlockHollow => CursorStyle::BlockHollow,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorTrail {
    #[default]
    Off,
    Warp,
    Comet,
    Blaze,
    Sweep,
    Tapered,
    Rainbow,
    Frozen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OptionAsAlt {
    #[default]
    Both,
    Left,
    Right,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CustomSchemePolicy {
    #[default]
    Ask,
    Always,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusNotificationRule {
    pub tab_indicator: bool,
    pub desktop_notification: bool,
    pub badge: bool,
    pub sound: bool,
    pub volume: f32,
}

impl StatusNotificationRule {
    pub fn working() -> Self {
        Self { tab_indicator: true, desktop_notification: false, badge: false, sound: false, volume: 0.7 }
    }
    pub fn blocked() -> Self {
        Self { tab_indicator: true, desktop_notification: true, badge: true, sound: true, volume: 0.85 }
    }
    pub fn error() -> Self {
        Self { tab_indicator: true, desktop_notification: true, badge: true, sound: true, volume: 0.85 }
    }
    pub fn done() -> Self {
        Self { tab_indicator: true, desktop_notification: true, badge: false, sound: false, volume: 0.65 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationMatrix {
    pub working: StatusNotificationRule,
    pub blocked: StatusNotificationRule,
    pub error: StatusNotificationRule,
    pub done: StatusNotificationRule,
}

impl Default for NotificationMatrix {
    fn default() -> Self {
        Self { working: StatusNotificationRule::working(), blocked: StatusNotificationRule::blocked(), error: StatusNotificationRule::error(), done: StatusNotificationRule::done() }
    }
}

/// Zero-trust per-host Olo desktop integration policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostBridgePolicy {
    pub allow_open_url: bool,
    pub allow_localhost_autoforward: bool,
    pub allow_agent_screens: bool,
    pub allow_passwords_op: bool,
    pub allow_ssh_agent: bool,
    pub allow_remote_control: bool,
    pub allow_token_pool: bool,
}

impl HostBridgePolicy {
    pub fn local_default() -> Self {
        Self { allow_open_url: true, allow_localhost_autoforward: true, allow_agent_screens: true, allow_passwords_op: true, allow_ssh_agent: true, allow_remote_control: true, allow_token_pool: true }
    }

    pub fn remote_default() -> Self {
        Self {
            allow_open_url: false,
            allow_localhost_autoforward: false,
            allow_agent_screens: false,
            allow_passwords_op: false,
            allow_ssh_agent: false,
            allow_remote_control: false,
            allow_token_pool: false,
        }
    }

    pub fn any_enabled(&self) -> bool {
        self.allow_open_url || self.allow_localhost_autoforward || self.allow_agent_screens || self.allow_passwords_op || self.allow_ssh_agent || self.allow_remote_control || self.allow_token_pool
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedHost {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default)]
    pub ssh_agent_socket: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // ---- Layout & Chrome ----
    pub toolbar_layout: ToolbarLayout,
    pub toolbar_behavior: BarBehavior,
    pub preserve_window_width: bool,
    pub tabs_fill_space: bool,
    pub tab_icon_count: u8,
    pub tab_color_mode: TabColorMode,
    pub pane_header_behavior: BarBehavior,
    pub density: Density,
    pub padding_x: f32,
    pub padding_y: f32,
    pub program_colors_padding: bool,
    pub program_colors_pane_header: bool,
    pub active_pane_highlight: bool,
    pub active_pane_highlight_color: Option<Rgb>,
    pub active_pane_highlight_opacity: f32,
    pub active_pane_highlight_style: HighlightStyle,
    pub fade_inactive_panes: bool,
    pub fade_inactive_windows: bool,
    pub focus_follows_mouse: bool,
    pub animation_style: AnimationStyle,
    pub autobalance_splits: bool,
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    pub sidebar_mode: SidebarMode,
    pub activity_group_by: ActivityGroupBy,
    pub activity_include_idle: bool,

    // ---- Theme & Colors ----
    pub scheme_choice: SchemeChoice,
    pub light_theme_id: Option<String>,
    pub dark_theme_id: Option<String>,
    pub custom_themes: Vec<TerminalTheme>,
    pub extended_palette: bool,
    pub minimum_contrast: f32,
    pub force_theme_palette: bool,
    pub tint_with_theme: bool,
    pub selection_fg: Option<Rgb>,
    pub selection_bg: Option<Rgb>,
    pub rounded_corners: bool,

    // ---- Typography ----
    pub font_family: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub char_spacing: f32,
    pub ligatures: bool,
    pub bold_is_bright: bool,
    pub fallback_fonts: Vec<String>,

    // ---- Cursor ----
    pub cursor_shape: CursorShapeSetting,
    pub force_cursor_style: bool,
    pub cursor_blink: bool,
    pub cursor_blink_interval_ms: u64,
    pub cursor_trail: CursorTrail,

    // ---- Behavior ----
    pub copy_on_selection: bool,
    pub shine_on_copy: bool,
    pub smooth_resizing: bool,
    pub anchor_bottom: bool,
    pub prefer_generated_titles: bool,
    pub startup_command: String,
    pub option_as_alt: OptionAsAlt,
    pub custom_scheme_policy: CustomSchemePolicy,
    pub trusted_links: Vec<String>,
    pub sudo_window: bool,
    pub keep_going_message: String,

    // ---- Notifications ----
    pub notifications: NotificationMatrix,

    // ---- Hosts, SSH & Olo Bridge ----
    pub home_server_address: String,
    pub saved_hosts: Vec<SavedHost>,
    pub ssh_agent_socket: Option<String>,
    pub host_policies: HashMap<String, HostBridgePolicy>,
    pub developer_mode: bool,

    // ---- Keymap Overrides ----
    pub keymap_overrides: Vec<KeymapLayer>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            toolbar_layout: ToolbarLayout::Above,
            toolbar_behavior: BarBehavior::Always,
            preserve_window_width: false,
            tabs_fill_space: false,
            tab_icon_count: 3,
            tab_color_mode: TabColorMode::Theme,
            pane_header_behavior: BarBehavior::Always,
            density: Density::Comfortable,
            padding_x: olo_kit_base::metrics::WINDOW_INSET,
            padding_y: 6.0,
            program_colors_padding: true,
            program_colors_pane_header: false,
            active_pane_highlight: true,
            active_pane_highlight_color: None,
            active_pane_highlight_opacity: 0.9,
            active_pane_highlight_style: HighlightStyle::Fade,
            fade_inactive_panes: false,
            fade_inactive_windows: false,
            focus_follows_mouse: false,
            animation_style: AnimationStyle::Snappy,
            autobalance_splits: true,
            sidebar_width: 192.0,
            sidebar_collapsed: false,
            sidebar_mode: SidebarMode::Sessions,
            activity_group_by: ActivityGroupBy::Priority,
            activity_include_idle: false,

            scheme_choice: SchemeChoice::Auto,
            light_theme_id: None,
            dark_theme_id: None,
            custom_themes: Vec::new(),
            extended_palette: false,
            minimum_contrast: 4.5,
            force_theme_palette: false,
            tint_with_theme: false,
            selection_fg: None,
            selection_bg: None,
            rounded_corners: true,

            font_family: None,
            font_size: 10.5,
            line_height: 1.0,
            char_spacing: 0.0,
            ligatures: true,
            bold_is_bright: false,
            fallback_fonts: Vec::new(),

            cursor_shape: CursorShapeSetting::Block,
            force_cursor_style: false,
            cursor_blink: true,
            cursor_blink_interval_ms: 530,
            cursor_trail: CursorTrail::Off,

            copy_on_selection: false,
            shine_on_copy: true,
            smooth_resizing: true,
            anchor_bottom: true,
            prefer_generated_titles: true,
            startup_command: String::new(),
            option_as_alt: OptionAsAlt::Both,
            custom_scheme_policy: CustomSchemePolicy::Ask,
            trusted_links: Vec::new(),
            sudo_window: true,
            keep_going_message: "Please keep going until everything is complete and verified.".to_owned(),

            notifications: NotificationMatrix::default(),

            home_server_address: "local".to_owned(),
            saved_hosts: Vec::new(),
            ssh_agent_socket: None,
            host_policies: HashMap::new(),
            developer_mode: false,
            keymap_overrides: Vec::new(),
        }
    }
}

pub fn settings_path() -> PathBuf {
    olo_xdg::config_dir().join("rex.json")
}

impl Settings {
    /// Reset preferences without forgetting connections or changing the trust granted to them.
    pub fn preference_defaults(&self) -> Self {
        Self {
            saved_hosts: self.saved_hosts.clone(),
            home_server_address: self.home_server_address.clone(),
            ssh_agent_socket: self.ssh_agent_socket.clone(),
            host_policies: self.host_policies.clone(),
            trusted_links: self.trusted_links.clone(),
            custom_scheme_policy: self.custom_scheme_policy,
            ..Self::default()
        }
    }

    pub fn load() -> Self {
        std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        olo_kit::store::save(settings_path(), self);
    }

    pub fn policy_for_host(&self, host_key: &str, is_local: bool) -> HostBridgePolicy {
        if let Some(p) = self.host_policies.get(host_key) {
            return p.clone();
        }
        if is_local { HostBridgePolicy::local_default() } else { HostBridgePolicy::remote_default() }
    }

    pub fn pane_header_height(&self) -> f32 {
        match self.density {
            Density::Comfortable => 22.0,
            Density::Compact => 20.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_connections_and_trust() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "saved_hosts": [{"id":"example","label":"Example","endpoint":"unix:///example","ssh_agent_socket":"/example-agent"}],
            "home_server_address":"unix:///example",
            "ssh_agent_socket":"/example-agent",
            "host_policies":{"local":HostBridgePolicy::remote_default(),"example":HostBridgePolicy::local_default()},
            "custom_scheme_policy":"Never","trusted_links":["example.invalid"],"font_size":21.0
        }))
        .unwrap();
        let defaults = settings.preference_defaults();
        assert_eq!(defaults.font_size, Settings::default().font_size);
        assert_eq!(defaults.saved_hosts, settings.saved_hosts);
        assert_eq!(defaults.home_server_address, settings.home_server_address);
        assert_eq!(defaults.ssh_agent_socket, settings.ssh_agent_socket);
        assert_eq!(defaults.host_policies, settings.host_policies);
        assert_eq!(defaults.custom_scheme_policy, settings.custom_scheme_policy);
        assert_eq!(defaults.trusted_links, settings.trusted_links);
    }
}
