//! Complete Rex client action catalog (~105 actions), server-synced keymap engine,
//! key sequences (`ctrl+b>c`), Which-Key sequence hints, key modes (`exclusive`/`once`),
//! and Olo `KeyDownEvent` → Rex rev-19 `KeyEvent` translation.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use olo_kit::{KeyDownEvent, Keystroke};
use olo_rex_client::{KeyEvent as RexKeyEvent, KeymapAction, KeymapBinding, KeymapDiagnostic, KeymapMode};
use serde_json::{Value, json};

/// Metadata for a single client-registerable Rex action.
#[derive(Debug, Clone)]
pub struct ActionSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub keywords: &'static [&'static str],
    pub args_schema: Option<Value>,
}

/// A parsed single key chord (`ctrl+shift+p`, `meta+k`, `space`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: String,
}

impl KeyChord {
    pub fn parse(raw: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut meta = false;
        let mut key = String::new();
        for part in raw.trim().split('+') {
            let p = part.trim().to_ascii_lowercase();
            match p.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "opt" | "option" => alt = true,
                "shift" => shift = true,
                "meta" | "cmd" | "super" | "command" => meta = true,
                other if !other.is_empty() => key = normalize_key_name(other),
                _ => {}
            }
        }
        if key.is_empty() {
            return None;
        }
        Some(Self { ctrl, alt, shift, meta, key })
    }

    /// The chord a keystroke is to the person's Rex configuration.
    /// `cmd`: it was pressed with a command key that is a key of its own
    /// ([`cmd_held`]), which the configuration writes as `cmd`; Control
    /// itself stays `ctrl` (with Windows' keys, where it is the command
    /// key too, a `ctrl+b` prefix is still Ctrl+B). A shifted symbol is
    /// its key with Shift (`ctrl+shift+period`, which arrives as ">").
    /// A `cmd` chord is matched without Shift: the one the command key
    /// brings to a terminal can't be told from the person's.
    pub fn from_keystroke(ks: &Keystroke, cmd: bool) -> Self {
        // (Alt is the terminal's Meta, both keys, as in Olo's Terminal.)
        let alt = ks.modifiers.alt;
        let lower = ks.key.to_ascii_lowercase();
        let (base, shifted) = olo_kit::keys::unshifted(&lower);
        Self { ctrl: ks.modifiers.control && !cmd, alt, shift: (ks.modifiers.shift || shifted) && !cmd, meta: cmd || ks.modifiers.platform, key: normalize_key_name(base) }
    }

    pub fn format_human(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push('⌃');
        }
        if self.alt {
            s.push('⌥');
        }
        if self.shift {
            s.push('⇧');
        }
        if self.meta {
            s.push('⌘');
        }
        let pretty = match self.key.as_str() {
            "enter" | "return" => "↩".to_owned(),
            "escape" => "Esc".to_owned(),
            "backspace" => "⌫".to_owned(),
            "tab" => "⇥".to_owned(),
            "space" => "Space".to_owned(),
            "up" | "arrow_up" => "↑".to_owned(),
            "down" | "arrow_down" => "↓".to_owned(),
            "left" | "arrow_left" => "←".to_owned(),
            "right" | "arrow_right" => "→".to_owned(),
            "comma" => ",".to_owned(),
            "period" => ".".to_owned(),
            "slash" => "/".to_owned(),
            "backslash" => "\\".to_owned(),
            "minus" => "-".to_owned(),
            "equal" => "=".to_owned(),
            "bracket_left" => "[".to_owned(),
            "bracket_right" => "]".to_owned(),
            k if k.len() == 1 => k.to_ascii_uppercase(),
            k => k.to_owned(),
        };
        s.push_str(&pretty);
        s
    }
}

/// Parsed multi-chord sequence (`ctrl+b>c` or `["ctrl+b", "c"]`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySequence {
    pub chords: Vec<KeyChord>,
}

impl KeySequence {
    pub fn parse(raw: &str) -> Option<Self> {
        let mut chords = Vec::new();
        for seg in raw.split('>') {
            let trimmed = seg.trim();
            if trimmed.is_empty() {
                continue;
            }
            chords.push(KeyChord::parse(trimmed)?);
        }
        if chords.is_empty() { None } else { Some(Self { chords }) }
    }

    pub fn from_wire_keys(keys: &[String]) -> Option<Self> {
        let mut chords = Vec::new();
        for raw in keys {
            if raw.contains('>') {
                if let Some(sub) = Self::parse(raw) {
                    chords.extend(sub.chords);
                } else {
                    return None;
                }
            } else {
                chords.push(KeyChord::parse(raw)?);
            }
        }
        if chords.is_empty() { None } else { Some(Self { chords }) }
    }
}

fn normalize_key_name(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "return" | "enter" => "enter".to_owned(),
        "esc" | "escape" => "escape".to_owned(),
        " " | "space" => "space".to_owned(),
        "arrowup" | "arrow_up" | "up" => "up".to_owned(),
        "arrowdown" | "arrow_down" | "down" => "down".to_owned(),
        "arrowleft" | "arrow_left" | "left" => "left".to_owned(),
        "arrowright" | "arrow_right" | "right" => "right".to_owned(),
        "pageup" | "page_up" => "page_up".to_owned(),
        "pagedown" | "page_down" => "page_down".to_owned(),
        "," | "comma" => "comma".to_owned(),
        "." | "period" => "period".to_owned(),
        "/" | "slash" => "slash".to_owned(),
        "\\" | "backslash" => "backslash".to_owned(),
        "-" | "minus" => "minus".to_owned(),
        "=" | "equal" | "plus" => "equal".to_owned(),
        "[" | "bracketleft" | "bracket_left" => "bracket_left".to_owned(),
        "]" | "bracketright" | "bracket_right" => "bracket_right".to_owned(),
        "`" | "backquote" | "grave" => "backquote".to_owned(),
        other => other.to_owned(),
    }
}

/// Whether a keystroke was pressed with the command key: ⌘ with a Mac's
/// keys and Alt with BeOS's (the right Control, as app_server sends
/// it), with Windows' keys Control itself. As Terminal tells it: what
/// may be one of Rex's own commands.
pub fn command_held(ks: &Keystroke) -> bool {
    ks.modifiers.control && (olo_kit::right_control_held() || !olo_kit_base::keys::command_key())
}

/// Whether it was pressed with a command key that is a key of its own (a
/// Mac's ⌘, BeOS's Alt), not Control: what the person's Rex
/// configuration calls `cmd`, and what brings a Shift of its own to a
/// terminal.
pub fn cmd_held(ks: &Keystroke) -> bool {
    ks.modifiers.control && olo_kit::right_control_held() && olo_kit_base::keys::command_key()
}

/// Convert an Olo [`KeyDownEvent`] into a Rex rev-19 [`RexKeyEvent`].
/// `cmd`: pressed with the command key ([`cmd_held`]) and no command of
/// Rex's: it goes to the program in the pane as Control with the key,
/// as in Terminal (⌘R searches a shell's history like Ctrl+R), without
/// the Shift the command key brings to a terminal.
pub fn to_rex_key_event(e: &KeyDownEvent, cmd: bool) -> RexKeyEvent {
    let plain;
    let ks = if cmd {
        let mut ks = e.keystroke.clone();
        let lower = ks.key.to_ascii_lowercase();
        ks.key = olo_kit::keys::unshifted(&lower).0.to_owned();
        ks.modifiers.shift = false;
        ks.key_char = None;
        plain = ks;
        &plain
    } else {
        &e.keystroke
    };
    let key = match ks.key.as_str() {
        "up" => "arrow_up".to_owned(),
        "down" => "arrow_down".to_owned(),
        "left" => "arrow_left".to_owned(),
        "right" => "arrow_right".to_owned(),
        "pageup" => "page_up".to_owned(),
        "pagedown" => "page_down".to_owned(),
        "enter" | "return" => "enter".to_owned(),
        "escape" => "escape".to_owned(),
        "backspace" => "backspace".to_owned(),
        "delete" => "delete".to_owned(),
        "tab" => "tab".to_owned(),
        "space" => "space".to_owned(),
        other => {
            if other.len() == 1 {
                let ch = other.chars().next().unwrap();
                if ch.is_ascii_alphabetic() {
                    format!("key_{}", ch.to_ascii_lowercase())
                } else if ch.is_ascii_digit() {
                    format!("digit_{ch}")
                } else {
                    normalize_key_name(other)
                }
            } else {
                normalize_key_name(other)
            }
        }
    };
    let alt = ks.modifiers.alt;
    let mut mods_parts = Vec::new();
    if ks.modifiers.control {
        mods_parts.push("ctrl");
    }
    if alt {
        mods_parts.push("alt");
    }
    if ks.modifiers.shift {
        mods_parts.push("shift");
    }
    if ks.modifiers.platform {
        mods_parts.push("super");
    }
    let mods = mods_parts.join("+");
    let text = ks.key_char.clone().unwrap_or_default();
    let unshifted_codepoint = if ks.key.chars().count() == 1 { ks.key.chars().next().unwrap() as u32 } else { 0 };
    RexKeyEvent { key, mods, consumed_mods: String::new(), text, unshifted_codepoint, composing: false, action: if e.is_held { "repeat".to_owned() } else { "press".to_owned() } }
}

/// Complete catalog of ~105 Olo Rex client actions registered via `client.keymap`
/// and searchable in the Command Palette (`⇧⌘P`).
pub fn action_catalog() -> Vec<ActionSpec> {
    vec![
        // ---- Application & Palettes ----
        ActionSpec { id: "app.view_activity", title: "View Activity", category: "Activity", keywords: &["agents", "blocked", "working", "status"], args_schema: None },
        ActionSpec { id: "app.toggle_sidebar", title: "Toggle Sidebar", category: "View", keywords: &["hosts", "sessions", "drawer"], args_schema: None },
        ActionSpec { id: "app.settings", title: "Rex Settings…", category: "General", keywords: &["preferences", "hosts", "permissions", "notifications"], args_schema: None },
        ActionSpec { id: "app.diagnostics", title: "Rex Diagnostics…", category: "Diagnostics", keywords: &["debug", "server", "status", "version"], args_schema: None },
        ActionSpec { id: "app.hosts", title: "Hosts and Ports…", category: "Network", keywords: &["ssh", "remote", "tailscale", "tunnels", "ports"], args_schema: None },
        ActionSpec {
            id: "host.account",
            title: "Set Up This Host’s Account…",
            category: "Network",
            keywords: &["account", "claude", "codex", "antigravity", "agy", "install", "agents", "git"],
            args_schema: None,
        },
        ActionSpec { id: "host.manage", title: "Manage Host…", category: "Network", keywords: &["host", "helper", "bridge", "skill", "install", "remove", "server"], args_schema: None },
        // (The account on the host in front moved whole to another of the
        // person's hosts: `host.move:KEY` for another's.)
        ActionSpec { id: "host.move", title: "Move to Another Host…", category: "Network", keywords: &["move", "migrate", "host", "account", "home", "copy", "server"], args_schema: None },
        ActionSpec {
            id: "host.permissions",
            title: "This Host’s Permissions…",
            category: "Network",
            keywords: &["permissions", "grants", "links", "passwords", "clipboard", "ssh", "screens"],
            args_schema: None,
        },
        ActionSpec { id: "app.pair_device", title: "Remote Access…", category: "Network", keywords: &["pair", "device", "phone", "qr", "tailscale"], args_schema: None },
        ActionSpec { id: "app.new_window", title: "New Window", category: "Window", keywords: &["window", "open"], args_schema: None },
        ActionSpec { id: "app.close_window", title: "Close Window", category: "Window", keywords: &["window", "close"], args_schema: None },
        ActionSpec { id: "app.quit", title: "Quit Rex", category: "General", keywords: &["exit", "quit"], args_schema: None },
        ActionSpec { id: "app.reload_config", title: "Reload Server Configuration", category: "General", keywords: &["lua", "config", "reload"], args_schema: None },
        // ---- Sessions ----
        ActionSpec { id: "app.add_host", title: "Add Host…", category: "App", keywords: &["remote", "ssh", "server", "connect"], args_schema: None },
        ActionSpec { id: "session.new", title: "New Session", category: "Session", keywords: &["create", "workspace", "session"], args_schema: None },
        ActionSpec { id: "session.close", title: "Close Session", category: "Session", keywords: &["kill", "delete"], args_schema: None },
        // ---- Windows / Tabs ----
        ActionSpec { id: "window.new", title: "New Tab", category: "Tab", keywords: &["window", "tab", "shell"], args_schema: None },
        ActionSpec { id: "window.close", title: "Close Tab", category: "Tab", keywords: &["close", "tab"], args_schema: None },
        ActionSpec { id: "window.rename", title: "Rename Tab…", category: "Tab", keywords: &["name", "title", "label"], args_schema: None },
        ActionSpec { id: "window.move_left", title: "Move Tab Left", category: "Tab", keywords: &["order", "reorder"], args_schema: None },
        ActionSpec { id: "window.move_right", title: "Move Tab Right", category: "Tab", keywords: &["order", "reorder"], args_schema: None },
        ActionSpec { id: "session.rename", title: "Rename Session…", category: "Session", keywords: &["name", "label"], args_schema: None },
        ActionSpec { id: "window.next", title: "Next Tab", category: "Tab", keywords: &["right", "forward"], args_schema: None },
        ActionSpec { id: "window.previous", title: "Previous Tab", category: "Tab", keywords: &["left", "back"], args_schema: None },
        ActionSpec { id: "window.select_1", title: "Select Tab 1", category: "Tab", keywords: &["1"], args_schema: None },
        ActionSpec { id: "window.select_2", title: "Select Tab 2", category: "Tab", keywords: &["2"], args_schema: None },
        ActionSpec { id: "window.select_3", title: "Select Tab 3", category: "Tab", keywords: &["3"], args_schema: None },
        ActionSpec { id: "window.select_4", title: "Select Tab 4", category: "Tab", keywords: &["4"], args_schema: None },
        ActionSpec { id: "window.select_5", title: "Select Tab 5", category: "Tab", keywords: &["5"], args_schema: None },
        ActionSpec { id: "window.select_6", title: "Select Tab 6", category: "Tab", keywords: &["6"], args_schema: None },
        ActionSpec { id: "window.select_7", title: "Select Tab 7", category: "Tab", keywords: &["7"], args_schema: None },
        ActionSpec { id: "window.select_8", title: "Select Tab 8", category: "Tab", keywords: &["8"], args_schema: None },
        ActionSpec { id: "window.select_9", title: "Select Tab 9", category: "Tab", keywords: &["9"], args_schema: None },
        // ---- Splits & Panes ----
        ActionSpec { id: "pane.split_right", title: "Split Right", category: "Pane", keywords: &["vertical", "divide", "horizontal"], args_schema: None },
        ActionSpec { id: "pane.split_down", title: "Split Down", category: "Pane", keywords: &["horizontal", "divide", "below"], args_schema: None },
        ActionSpec { id: "pane.split_best_fit", title: "Split Best Fit", category: "Pane", keywords: &["auto", "smart", "split"], args_schema: None },
        ActionSpec { id: "pane.close", title: "Close Pane", category: "Pane", keywords: &["close", "kill", "block"], args_schema: None },
        ActionSpec { id: "pane.focus_left", title: "Focus Pane Left", category: "Pane", keywords: &["navigate", "left"], args_schema: None },
        ActionSpec { id: "pane.focus_right", title: "Focus Pane Right", category: "Pane", keywords: &["navigate", "right"], args_schema: None },
        ActionSpec { id: "pane.focus_up", title: "Focus Pane Up", category: "Pane", keywords: &["navigate", "up"], args_schema: None },
        ActionSpec { id: "pane.focus_down", title: "Focus Pane Down", category: "Pane", keywords: &["navigate", "down"], args_schema: None },
        ActionSpec { id: "pane.zoom_toggle", title: "Zoom / Unzoom Pane", category: "Pane", keywords: &["maximize", "fullscreen", "focus"], args_schema: None },
        ActionSpec { id: "pane.broadcast_toggle", title: "Toggle Broadcast Input to Window", category: "Pane", keywords: &["sync", "all panes"], args_schema: None },
        // ---- Floating Layers ----
        ActionSpec { id: "layer.new_floating", title: "New Floating Terminal", category: "Layers", keywords: &["overlay", "popup", "float"], args_schema: None },
        // ---- Terminal, Clipboard, Find, Copy Mode, Navigation ----
        ActionSpec { id: "terminal.copy", title: "Copy", category: "Edit", keywords: &["clipboard", "yank"], args_schema: None },
        ActionSpec { id: "terminal.paste", title: "Paste", category: "Edit", keywords: &["clipboard"], args_schema: None },
        ActionSpec { id: "terminal.select_all", title: "Select All", category: "Edit", keywords: &["selection"], args_schema: None },
        ActionSpec { id: "terminal.clear_scrollback", title: "Clear Scrollback", category: "Terminal", keywords: &["cls", "clear"], args_schema: None },
        ActionSpec { id: "terminal.reset", title: "Reset Terminal", category: "Terminal", keywords: &["ris", "hard reset"], args_schema: None },
        ActionSpec { id: "terminal.find", title: "Find in Terminal…", category: "Edit", keywords: &["search", "grep"], args_schema: None },
        ActionSpec { id: "terminal.find_next", title: "Find Next", category: "Edit", keywords: &["search"], args_schema: None },
        ActionSpec { id: "terminal.find_previous", title: "Find Previous", category: "Edit", keywords: &["search"], args_schema: None },
        ActionSpec { id: "terminal.copy_mode", title: "Enter / Exit Copy Mode", category: "Terminal", keywords: &["vim", "tmux", "scroll", "select"], args_schema: None },
        ActionSpec { id: "terminal.scroll_page_up", title: "Scroll Page Up", category: "Terminal", keywords: &["scrollback"], args_schema: None },
        ActionSpec { id: "terminal.scroll_page_down", title: "Scroll Page Down", category: "Terminal", keywords: &["scrollback"], args_schema: None },
        ActionSpec { id: "terminal.scroll_top", title: "Scroll to Top", category: "Terminal", keywords: &["scrollback", "home"], args_schema: None },
        ActionSpec { id: "terminal.scroll_bottom", title: "Scroll to Bottom", category: "Terminal", keywords: &["scrollback", "end"], args_schema: None },
        ActionSpec { id: "terminal.prompt_prev", title: "Jump to Previous Prompt", category: "Terminal", keywords: &["osc133", "command"], args_schema: None },
        ActionSpec { id: "terminal.prompt_next", title: "Jump to Next Prompt", category: "Terminal", keywords: &["osc133", "command"], args_schema: None },
        ActionSpec { id: "terminal.select_output", title: "Select Last Command Output", category: "Terminal", keywords: &["osc133", "copy"], args_schema: None },
        ActionSpec { id: "terminal.font_increase", title: "Increase Font Size", category: "View", keywords: &["zoom", "bigger"], args_schema: None },
        ActionSpec { id: "terminal.font_decrease", title: "Decrease Font Size", category: "View", keywords: &["zoom", "smaller"], args_schema: None },
        ActionSpec { id: "terminal.font_reset", title: "Reset Font Size", category: "View", keywords: &["zoom", "actual"], args_schema: None },
        ActionSpec { id: "terminal.export_text", title: "Export Screen as Plain Text…", category: "Terminal", keywords: &["save", "dump"], args_schema: None },
        ActionSpec { id: "terminal.export_html", title: "Export Screen as HTML…", category: "Terminal", keywords: &["save", "html"], args_schema: None },
        // ---- Files, Quick Look ----
        ActionSpec { id: "files.quick_look", title: "Quick Look Path…", category: "Files", keywords: &["preview", "inspect"], args_schema: None },
        ActionSpec { id: "files.open_in_tracker", title: "Open in Tracker", category: "Files", keywords: &["finder", "folder", "tracker", "browse", "files"], args_schema: None },
        // ---- Coding Agents & Automation ----
        ActionSpec { id: "agent.keep_going_toggle", title: "Toggle Keep It Going", category: "Agents", keywords: &["nudge", "continue", "autonomous"], args_schema: None },
        ActionSpec { id: "agent.auto_approve_toggle", title: "Toggle Auto Approve", category: "Agents", keywords: &["approve", "yes", "permission", "agent", "proceed"], args_schema: None },
        ActionSpec { id: "agent.jump_next_blocked", title: "Jump to Next Blocked Agent", category: "Agents", keywords: &["attention", "prompt", "waiting"], args_schema: None },
        ActionSpec { id: "agent.mark_all_read", title: "Mark All Read", category: "Agents", keywords: &["clear", "unread", "badge"], args_schema: None },
        // ---- Key Modes ----
        ActionSpec {
            id: "client.mode.enter",
            title: "Enter Key Mode",
            category: "Keymap",
            keywords: &["mode", "vim", "resize"],
            args_schema: Some(json!({"type":"object","properties":{"mode":{"type":"string"},"once":{"type":"boolean"}},"required":["mode"]})),
        },
        ActionSpec { id: "client.mode.exit", title: "Exit Key Mode", category: "Keymap", keywords: &["mode", "leave"], args_schema: None },
        ActionSpec { id: "client.mode.exit_all", title: "Exit All Key Modes", category: "Keymap", keywords: &["mode", "reset"], args_schema: None },
        // ---- Olo Desktop Bridge Actions ----
        ActionSpec { id: "olo.passwords.fill", title: "Fill Password from Olo Passwords", category: "Olo Bridge", keywords: &["keychain", "op", "secret"], args_schema: None },
        ActionSpec { id: "olo.install_remote_skill", title: "Install Olo’s Skill for Agents", category: "Olo Bridge", keywords: &["skill", "claude", "codex", "agent"], args_schema: None },
    ]
}

/// Convert the catalog into the `client.keymap` wire actions payload.
/// What a program on a host may ask of this computer through the
/// server (`client.perform`: a link opened, a notice, the clipboard set,
/// an Agent Screen), carried out by the hub where the host's permissions
/// allow (`handle_client_perform`; Preferences ▸ Rex). Declared to the
/// server with Rex's commands, since it refuses to pass on what a
/// client hasn't said it offers; not commands of the person's, and in
/// no palette.
pub fn bridge_catalog() -> Vec<ActionSpec> {
    // (`args`: the command's own arguments, in order.)
    let command = || Some(json!({"type":"object","properties":{"args":{"type":"array","items":{"type":"string"}}},"required":["args"]}));
    vec![
        ActionSpec {
            id: "olo.open",
            title: "Open a Link or a File on the Olo Desktop",
            category: "Olo Bridge",
            keywords: &["browser", "chrome", "xdg-open"],
            args_schema: Some(json!({"type":"object","properties":{"target":{"type":"string"}},"required":["target"]})),
        },
        ActionSpec { id: "olo.op", title: "Ask Olo Passwords (1Password’s Command Line)", category: "Olo Bridge", keywords: &["op", "secret", "password"], args_schema: command() },
        ActionSpec {
            id: "olo.screen",
            title: "Use an Olo Agent Screen (olo-screen’s Command Line)",
            category: "Olo Bridge",
            keywords: &["agent screen", "headless", "olo-screen"],
            args_schema: command(),
        },
        ActionSpec {
            id: "olo.send", title: "Save Files of the Host’s on the Olo Desktop", category: "Olo Bridge", keywords: &["send", "download", "files", "folder", "copy"], args_schema: command()
        },
        ActionSpec {
            id: "olo.sync",
            title: "Keep a Folder of the Host’s in Step on the Olo Desktop",
            category: "Olo Bridge",
            keywords: &["sync", "keep", "folder", "download", "watch"],
            args_schema: command(),
        },
        ActionSpec {
            id: "olo.forward",
            title: "Reach a Server of the Host’s from the Olo Desktop",
            category: "Olo Bridge",
            keywords: &["forward", "port", "localhost", "tunnel"],
            args_schema: command(),
        },
        ActionSpec {
            id: "olo.file",
            title: "Fetch a Picture an Answer Named",
            category: "Olo Bridge",
            keywords: &["picture", "file"],
            args_schema: Some(json!({"type":"object","properties":{"id":{"type":"string"},"offset":{"type":"integer"}},"required":["id"]})),
        },
        ActionSpec {
            id: "olo.wait",
            title: "Wait for a Command Still Running on the Olo Desktop",
            category: "Olo Bridge",
            keywords: &["pending"],
            args_schema: Some(json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]})),
        },
        ActionSpec {
            id: "olo.notify",
            title: "Send Olo Desktop Notification",
            category: "Olo Bridge",
            keywords: &["notification", "alert"],
            args_schema: Some(json!({"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"}},"required":["title"]})),
        },
        ActionSpec {
            id: "olo.clipboard.set",
            title: "Set Olo Clipboard",
            category: "Olo Bridge",
            keywords: &["copy", "clipboard"],
            args_schema: Some(json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})),
        },
        ActionSpec { id: "olo.clipboard.get", title: "Read Olo Clipboard", category: "Olo Bridge", keywords: &["paste", "clipboard"], args_schema: Some(json!({"type":"object","properties":{}})) },
    ]
}

pub fn register_actions_payload() -> Vec<KeymapAction> {
    action_catalog()
        .into_iter()
        .chain(bridge_catalog())
        .map(|a| KeymapAction {
            name: a.id.to_owned(),
            kind: Some("client".to_owned()),
            title: Some(a.title.to_owned()),
            description: None,
            category: Some(a.category.to_owned()),
            keywords: Some(a.keywords.join(" ")),
            args: a.args_schema,
            repeats: None,
            source: Some("olo-rex".to_owned()),
        })
        .collect()
}

/// A resolved binding ready for matching.
#[derive(Debug, Clone)]
pub struct ResolvedBinding {
    pub sequence: KeySequence,
    pub action: String,
    pub args: Option<Value>,
    pub description: Option<String>,
    #[allow(dead_code)]
    pub source: String,
}

/// Candidate continuation shown in the Which-Key Sequence Hints HUD.
#[derive(Debug, Clone)]
pub struct SequenceHint {
    pub next_key: String,
    #[allow(dead_code)]
    pub action: String,
    pub label: String,
    pub is_prefix: bool,
}

/// Outcome of matching a keystroke against the active keymap & mode stack.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum KeyMatch {
    Execute { action: String, args: Option<Value> },
    Pending { prefix: Vec<KeyChord>, hints: Vec<SequenceHint> },
    Blocked,
    Passthrough,
}

#[derive(Debug, Clone)]
struct ActiveMode {
    name: String,
    once: bool,
}

/// Stateful client keymap matcher supporting server bindings, user overrides,
/// multi-chord sequences, Which-Key hints, and exclusive/once key modes.
pub struct KeymapEngine {
    bindings_by_mode: HashMap<String, Vec<ResolvedBinding>>,
    modes: HashMap<String, KeymapMode>,
    pub diagnostics: Vec<KeymapDiagnostic>,
    mode_stack: Vec<ActiveMode>,
    pending_prefix: Vec<KeyChord>,
    pending_since: Option<Instant>,
    titles: HashMap<String, String>,
}

impl Default for KeymapEngine {
    fn default() -> Self {
        Self::new(&[], &[], &[])
    }
}

impl KeymapEngine {
    /// The keys of the person's own Rex configuration, as the server
    /// resolves them (`client.keymap`: Rex sends it the commands it has
    /// and no keys of its own, so what comes back is the configuration's
    /// bindings alone): prefix keys, sequences, modes. Rex's own
    /// commands have their keys from Olo's keyboard profile
    /// (`olo_protocol::app_keys`, Preferences ▸ Keyboard) through the
    /// menu bar, as every app's. (It came with 69 keys of its own here,
    /// written for a Mac's command key as a modifier of its own, which
    /// is not how a key reaches an app in Olo: none of them could be
    /// pressed, and each went to the shell as a Control key.)
    pub fn new(server_bindings: &[KeymapBinding], server_modes: &[KeymapMode], diagnostics: &[KeymapDiagnostic]) -> Self {
        let titles: HashMap<String, String> = action_catalog().into_iter().map(|a| (a.id.to_owned(), a.title.to_owned())).collect();
        let mut engine = Self { bindings_by_mode: HashMap::new(), modes: HashMap::new(), diagnostics: Vec::new(), mode_stack: Vec::new(), pending_prefix: Vec::new(), pending_since: None, titles };
        engine.rebuild(server_bindings, server_modes, diagnostics);
        engine
    }

    pub fn rebuild(&mut self, server_bindings: &[KeymapBinding], server_modes: &[KeymapMode], diagnostics: &[KeymapDiagnostic]) {
        self.bindings_by_mode.clear();
        self.modes.clear();
        self.diagnostics = diagnostics.to_vec();

        let mut by_mode: HashMap<String, HashMap<KeySequence, ResolvedBinding>> = HashMap::new();
        by_mode.entry(String::new()).or_default();

        for m in server_modes {
            self.modes.insert(m.name.clone(), m.clone());
            by_mode.entry(m.name.clone()).or_default();
        }

        // Apply resolved bindings returned by `client.keymap`.
        for b in server_bindings {
            let mode_key = b.mode.clone().unwrap_or_default();
            if let Some(seq) = KeySequence::from_wire_keys(&b.keys) {
                let desc = self.titles.get(&b.action).cloned();
                by_mode.entry(mode_key).or_default().insert(
                    seq.clone(),
                    ResolvedBinding { sequence: seq, action: b.action.clone(), args: b.args.clone(), description: desc, source: b.source.clone().unwrap_or_else(|| "server".to_owned()) },
                );
            }
        }

        for (mode, map) in by_mode {
            let mut list: Vec<ResolvedBinding> = map.into_values().collect();
            list.sort_by(|a, b| a.action.cmp(&b.action));
            self.bindings_by_mode.insert(mode, list);
        }
    }

    pub fn active_mode_name(&self) -> Option<&str> {
        self.mode_stack.last().map(|m| m.name.as_str())
    }

    pub fn enter_mode(&mut self, name: String, once: bool) {
        self.clear_pending();
        self.mode_stack.push(ActiveMode { name, once });
    }

    pub fn exit_mode(&mut self) {
        self.clear_pending();
        self.mode_stack.pop();
    }

    pub fn exit_all_modes(&mut self) {
        self.clear_pending();
        self.mode_stack.clear();
    }

    pub fn clear_pending(&mut self) {
        self.pending_prefix.clear();
        self.pending_since = None;
    }

    pub fn pending_hints(&self) -> Option<(String, Vec<SequenceHint>)> {
        if self.pending_prefix.is_empty() {
            return None;
        }
        let prefix_str = self.pending_prefix.iter().map(KeyChord::format_human).collect::<Vec<_>>().join(" › ");
        let hints = self.collect_hints(&self.pending_prefix);
        Some((prefix_str, hints))
    }

    /// Whether a key is to be asked of the configuration's keys before
    /// Olo's own: it is in one of its modes, or part way through one of
    /// its sequences (one begun within the last two seconds: an older
    /// one is let go of here, before whose key it is gets decided. Left
    /// to be found stale only when the key was asked of it, ⌘D after a
    /// prefix key and a pause skipped Rex's own Split and went to the
    /// shell as Ctrl+D).
    pub fn engaged(&mut self) -> bool {
        if self.pending_since.is_some_and(|since| since.elapsed() > Duration::from_secs(2)) {
            self.clear_pending();
        }
        !self.mode_stack.is_empty() || !self.pending_prefix.is_empty()
    }

    /// Whether the configuration has any keys at all (with none, no key
    /// is asked of it).
    pub fn is_empty(&self) -> bool {
        self.bindings_by_mode.values().all(Vec::is_empty)
    }

    pub fn handle_key(&mut self, ks: &Keystroke, cmd: bool) -> KeyMatch {
        if let Some(since) = self.pending_since
            && since.elapsed() > Duration::from_secs(2)
        {
            self.clear_pending();
        }

        let chord = KeyChord::from_keystroke(ks, cmd);

        if chord.key == "escape" && !chord.ctrl && !chord.alt && !chord.meta {
            if !self.pending_prefix.is_empty() {
                self.clear_pending();
                return KeyMatch::Blocked;
            }
            if !self.mode_stack.is_empty() {
                self.mode_stack.pop();
                return KeyMatch::Blocked;
            }
        }

        let mode_name = self.active_mode_name().unwrap_or("").to_owned();
        let mut candidate = self.pending_prefix.clone();
        candidate.push(chord.clone());

        let exact = self.find_exact(&mode_name, &candidate);
        let hints = self.collect_hints_in(&mode_name, &candidate);

        if let Some(binding) = exact {
            self.clear_pending();
            if self.mode_stack.last().is_some_and(|m| m.once) {
                self.mode_stack.pop();
            }
            return KeyMatch::Execute { action: binding.action, args: binding.args };
        }

        if !hints.is_empty() {
            self.pending_prefix = candidate.clone();
            self.pending_since = Some(Instant::now());
            return KeyMatch::Pending { prefix: candidate, hints };
        }

        if !self.pending_prefix.is_empty() {
            self.clear_pending();
            let fresh = vec![chord];
            if let Some(binding) = self.find_exact(&mode_name, &fresh) {
                if self.mode_stack.last().is_some_and(|m| m.once) {
                    self.mode_stack.pop();
                }
                return KeyMatch::Execute { action: binding.action, args: binding.args };
            }
            let fresh_hints = self.collect_hints_in(&mode_name, &fresh);
            if !fresh_hints.is_empty() {
                self.pending_prefix = fresh.clone();
                self.pending_since = Some(Instant::now());
                return KeyMatch::Pending { prefix: fresh, hints: fresh_hints };
            }
        }

        if let Some(mode) = self.modes.get(&mode_name)
            && mode.exclusive
        {
            return KeyMatch::Blocked;
        }

        KeyMatch::Passthrough
    }

    fn find_exact(&self, mode: &str, seq: &[KeyChord]) -> Option<ResolvedBinding> {
        if let Some(list) = self.bindings_by_mode.get(mode)
            && let Some(hit) = list.iter().find(|b| b.sequence.chords == seq)
        {
            return Some(hit.clone());
        }
        if !mode.is_empty() && !self.modes.get(mode).is_some_and(|m| m.exclusive) {
            return self.bindings_by_mode.get("").and_then(|list| list.iter().find(|b| b.sequence.chords == seq)).cloned();
        }
        None
    }

    fn collect_hints(&self, prefix: &[KeyChord]) -> Vec<SequenceHint> {
        let mode = self.active_mode_name().unwrap_or("");
        self.collect_hints_in(mode, prefix)
    }

    fn collect_hints_in(&self, mode: &str, prefix: &[KeyChord]) -> Vec<SequenceHint> {
        let mut seen = HashSet::new();
        let mut hints = Vec::new();
        let mut scan = |list: &[ResolvedBinding]| {
            for b in list {
                if b.sequence.chords.len() > prefix.len() && b.sequence.chords[..prefix.len()] == *prefix {
                    let next = &b.sequence.chords[prefix.len()];
                    let next_label = next.format_human();
                    if seen.insert(next_label.clone()) {
                        let is_prefix = b.sequence.chords.len() > prefix.len() + 1;
                        let label = if is_prefix { "More…".to_owned() } else { b.description.clone().or_else(|| self.titles.get(&b.action).cloned()).unwrap_or_else(|| b.action.clone()) };
                        hints.push(SequenceHint { next_key: next_label, action: b.action.clone(), label, is_prefix });
                    }
                }
            }
        };
        if let Some(list) = self.bindings_by_mode.get(mode) {
            scan(list);
        }
        if !mode.is_empty()
            && !self.modes.get(mode).is_some_and(|m| m.exclusive)
            && let Some(list) = self.bindings_by_mode.get("")
        {
            scan(list);
        }
        hints.sort_by(|a, b| a.next_key.cmp(&b.next_key));
        hints
    }
}
