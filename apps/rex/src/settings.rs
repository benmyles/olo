//! What Rex keeps of its own (`~/.config/olo/rex.json`): a window's
//! quick choices (its Settings and View menus: where the tabs are, pane
//! headers, the sidebar, the colours, the text's size) and its copy of
//! the hosts.
//!
//! Which hosts there are, what each may do on this computer and which
//! changes of an agent become notifications are the desktop's settings
//! (Preferences ▸ Rex), followed live (`shared`); the keys are
//! Preferences ▸ Keyboard. (As the app came, this file held some sixty
//! settings and a window of eight sections to change them; two thirds of
//! them had nothing behind them, or decided chrome that is the
//! desktop's to decide: densities, paddings, fades, a highlight colour,
//! cursor trails.)

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// Where a session's tabs are: along the top, or down the side (as
/// Terminal's). (There were four; below and on the right are read as
/// these two.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolbarLayout {
    #[default]
    #[serde(alias = "Below")]
    Above,
    #[serde(alias = "LeftSidebar", alias = "RightSidebar")]
    Side,
}

/// Whether split panes have their headers. (A third choice, "on hover",
/// was never built: it hid them.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BarBehavior {
    #[default]
    Always,
    #[serde(alias = "Hover")]
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarMode {
    #[default]
    Sessions,
    Activity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SchemeChoice {
    #[default]
    Auto,
    Light,
    Dark,
}

/// Which changes of an agent, in a pane that isn't in front, become a
/// notification: the desktop's (Preferences ▸ Rex), never kept here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notices {
    pub blocked: bool,
    pub done: bool,
    pub failed: bool,
    pub minded_subagents: bool,
}

impl Default for Notices {
    fn default() -> Self {
        Self { blocked: true, done: true, failed: true, minded_subagents: false }
    }
}

/// What a host's sessions may do on this computer: the desktop's
/// (Preferences ▸ Rex), never kept here; with none known, nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostBridgePolicy {
    pub allow_open_url: bool,
    pub allow_localhost_autoforward: bool,
    pub allow_agent_screens: bool,
    pub allow_passwords_op: bool,
    pub allow_remote_control: bool,
    /// Passwords' SSH agent in the host's terminals (`hub`, "The SSH
    /// agent on a host").
    pub allow_ssh_agent: bool,
    /// Files a program there sends (`olo send`) saved here, in Downloads
    /// ▸ Rex ▸ HOST.
    pub allow_send_files: bool,
    /// Run commands on this computer (`olo run`), asked first unless
    /// Approve Without Asking is on in the tray.
    pub allow_run_commands: bool,
}

impl HostBridgePolicy {
    pub fn local_default() -> Self {
        Self {
            allow_open_url: true,
            allow_localhost_autoforward: true,
            allow_agent_screens: true,
            allow_passwords_op: true,
            allow_remote_control: true,
            allow_ssh_agent: true,
            allow_send_files: true,
            allow_run_commands: true,
        }
    }

    pub fn remote_default() -> Self {
        Self::default()
    }

    /// Whether anything of the bridge's is granted: what its server
    /// here runs for, and what the Host window and the status bar's
    /// mark say of it. (Not the SSH keys: they go over SSH itself, with
    /// no bridge.)
    /// What of the bridge's is granted, each in a word or two, in the
    /// order Preferences lists them: for the Host window's line.
    pub fn granted(&self) -> Vec<&'static str> {
        [
            (self.allow_open_url, "Links"),
            (self.allow_send_files, "Files"),
            (self.allow_localhost_autoforward, "Local Servers"),
            (self.allow_passwords_op, "Passwords"),
            (self.allow_run_commands, "Desktop Commands"),
            // (Its keys, where the host has been given them: with them
            // unsaid, nothing in Rex showed whether a host had them when
            // git there was refused.)
            (self.allow_ssh_agent, "SSH keys"),
            // (In the order the Host window's description says them.)
            (self.allow_agent_screens, "Agent Screens"),
            (self.allow_remote_control, "Clipboard"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }

    pub fn any_enabled(&self) -> bool {
        self.allow_open_url || self.allow_localhost_autoforward || self.allow_agent_screens || self.allow_passwords_op || self.allow_remote_control || self.allow_send_files || self.allow_run_commands
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

/// The size the terminals' text comes at, in points.
pub const TEXT_SIZE: f32 = 10.5;
/// The least and the most the Settings menu takes it to.
pub const TEXT_SIZES: (f32, f32) = (8.0, 32.0);
/// A split pane's header, in points.
pub const PANE_HEADER: f32 = 22.0;
/// Around a terminal's text, in points (across, down).
pub const PADDING: (f32, f32) = (olo_kit_base::metrics::WINDOW_INSET, 6.0);
/// The least contrast a terminal's text is drawn with against what is
/// behind it.
pub const MINIMUM_CONTRAST: f32 = 4.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // ---- A window's quick choices (the Settings and View menus) ----
    pub toolbar_layout: ToolbarLayout,
    pub pane_header_behavior: BarBehavior,
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    pub sidebar_mode: SidebarMode,
    pub activity_include_idle: bool,
    pub scheme_choice: SchemeChoice,
    /// The colours for light and for dark, by their id among Olo's
    /// (`olo_theme::looks::terminal`); none: the look's own, followed.
    pub light_theme_id: Option<String>,
    pub dark_theme_id: Option<String>,
    pub font_size: f32,
    /// What is typed is drawn at once where the host's echo is slow to
    /// come back (`predict`).
    pub type_ahead: bool,
    /// Olo's helper is put on the hosts reached over SSH and used
    /// there (`link`, `olo_rex_client::helper`): what is typed then
    /// goes as it is typed, in order.
    pub host_helper: bool,
    /// What is known of the helper on each host, by the host's key:
    /// "yes" where a connection went through one, "no:BUILD" where
    /// this build of the app found it had none to give (`hub`,
    /// `helper_before_connecting`), "off" where it was taken off the
    /// host in the Host window (not put back by itself). A host with no
    /// word here is asked before its connection is made.
    pub helper_hosts: std::collections::BTreeMap<String, String>,
    /// What Keep It Going types to an agent that has stopped.
    pub keep_going_message: String,
    /// Typing over a slow link and Keep It Going's message were carried
    /// into the desktop's settings (Preferences ▸ Rex), once: this file's
    /// copy of them follows those since.
    #[serde(default)]
    pub carried_typing_to_preferences: bool,

    // ---- Hosts ----
    pub home_server_address: String,
    /// Rex's copy of the desktop's hosts (`shared::apply`), so that it
    /// knows them at once as it starts.
    pub saved_hosts: Vec<SavedHost>,
    pub ssh_agent_socket: Option<String>,
    /// The desktop's (`shared::apply`); not kept: with the desktop's
    /// settings unread, a remote host may do nothing here.
    #[serde(skip)]
    pub host_policies: HashMap<String, HostBridgePolicy>,
    #[serde(skip)]
    pub notices: Notices,
    /// What this file held of hosts' permissions and of notices before
    /// they became the desktop's, kept until it has been carried there
    /// (`shared::adopt`, once), whatever is saved meanwhile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_preferences: Option<crate::shared::Old>,
    pub carried_to_preferences: bool,
    /// Keys the app's own recorder left (it went with the Settings
    /// window): kept, and still sent to the server with Rex's commands,
    /// so what worked goes on working. They are changed in the person's
    /// Rex configuration now.
    pub keymap_overrides: Vec<olo_rex_client::KeymapLayer>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            toolbar_layout: ToolbarLayout::Above,
            pane_header_behavior: BarBehavior::Always,
            // (Wide enough for the sidebar's control to show each
            // name's mark beside it and Activity's count, with room:
            // as Ben drew it. It was 192, where the marks gave way.)
            sidebar_width: 244.0,
            sidebar_collapsed: false,
            sidebar_mode: SidebarMode::Sessions,
            activity_include_idle: false,
            scheme_choice: SchemeChoice::Auto,
            light_theme_id: None,
            dark_theme_id: None,
            font_size: TEXT_SIZE,
            type_ahead: true,
            host_helper: true,
            helper_hosts: std::collections::BTreeMap::new(),
            keep_going_message: olo_protocol::REX_KEEP_GOING_SAYS.to_owned(),
            carried_typing_to_preferences: false,
            home_server_address: "local".to_owned(),
            saved_hosts: Vec::new(),
            ssh_agent_socket: None,
            host_policies: HashMap::new(),
            notices: Notices::default(),
            before_preferences: None,
            carried_to_preferences: false,
            keymap_overrides: Vec::new(),
        }
    }
}

pub fn settings_path() -> PathBuf {
    olo_xdg::config_dir().join("rex.json")
}

impl Settings {
    pub fn load() -> Self {
        let text = std::fs::read_to_string(settings_path()).ok();
        Self::read(text.as_deref())
    }

    /// From the file's text: and, until it has been carried into the
    /// desktop's settings, what the file held of permissions and
    /// notices as the app wrote them before (read from the same text).
    pub fn read(text: Option<&str>) -> Self {
        let mut settings: Self = text.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        if !settings.carried_to_preferences && settings.before_preferences.is_none() {
            settings.before_preferences = Some(text.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default());
        }
        settings
    }

    pub fn save(&self) {
        olo_kit::store::save(settings_path(), self);
    }

    /// The three parts that follow the desktop's settings (`shared`), to
    /// tell whether following changed any of them.
    pub fn clone_followed(&self) -> (Vec<SavedHost>, Vec<(String, HostBridgePolicy)>, Notices) {
        let mut policies: Vec<(String, HostBridgePolicy)> = self.host_policies.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        policies.sort_by(|a, b| a.0.cmp(&b.0));
        (self.saved_hosts.clone(), policies, self.notices)
    }

    pub fn policy_for_host(&self, host_key: &str, is_local: bool) -> HostBridgePolicy {
        if let Some(p) = self.host_policies.get(host_key) {
            return p.clone();
        }
        if is_local { HostBridgePolicy::local_default() } else { HostBridgePolicy::remote_default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file the app wrote before: the choices that stay are read (the
    /// placements that went, as the two that stayed), what went is passed
    /// over, and the permissions and notices it held are kept aside until
    /// they are carried over, through a save and a load.
    #[test]
    fn an_older_file_is_read_and_what_it_held_is_kept_until_carried() {
        let text = r#"{"toolbar_layout":"RightSidebar","pane_header_behavior":"Hover","density":"Compact","cursor_trail":"Comet","font_size":13.0,
            "saved_hosts":[{"id":"h1","label":"Build box","endpoint":"ssh://dev@build.example","auto_connect":true}],
            "host_policies":{"h1":{"allow_open_url":true,"allow_localhost_autoforward":false,"allow_agent_screens":false,"allow_passwords_op":false,"allow_ssh_agent":true,"allow_remote_control":false,"allow_token_pool":true}},
            "notifications":{"done":{"tab_indicator":true,"desktop_notification":false,"badge":false,"sound":false,"volume":0.65}}}"#;
        let s = Settings::read(Some(text));
        assert_eq!((s.toolbar_layout, s.pane_header_behavior, s.font_size), (ToolbarLayout::Side, BarBehavior::Hidden, 13.0));
        assert!(s.host_policies.is_empty(), "permissions are never read from here");
        let old = s.before_preferences.clone().expect("kept aside");
        assert!(old.host_policies["h1"].allow_open_url && !old.notifications.done.desktop_notification && old.notifications.blocked.desktop_notification);
        // Saved and loaded again before it could be carried over: still there.
        let again = Settings::read(Some(&serde_json::to_string(&s).unwrap()));
        assert_eq!(again.before_preferences, Some(old));
        // Carried: gone from the file, and not looked for again.
        let mut done = again;
        done.before_preferences = None;
        done.carried_to_preferences = true;
        let text = serde_json::to_string(&done).unwrap();
        assert!(!text.contains("before_preferences") && !text.contains("host_policies") && !text.contains("notifications"));
        assert!(Settings::read(Some(&text)).before_preferences.is_none());
        // No file at all: nothing to carry, but the hosts of the desktop's are still merged in once.
        assert_eq!(Settings::read(None).before_preferences, Some(crate::shared::Old::default()));
    }
}
