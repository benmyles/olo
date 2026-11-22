//! Complete Rex client action catalog (~105 actions), server-synced keymap engine,
//! key sequences (`ctrl+b>c`), Which-Key sequence hints, key modes (`exclusive`/`once`),
//! and Olo `KeyDownEvent` → Rex rev-19 `KeyEvent` translation.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use olo_kit::{KeyDownEvent, Keystroke};
use olo_rex_client::{KeyEvent as RexKeyEvent, KeymapAction, KeymapBinding, KeymapDiagnostic, KeymapLayer, KeymapMode};
use serde_json::{Value, json};

/// Metadata for a single client-registerable Rex action.
#[derive(Debug, Clone)]
pub struct ActionSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub keywords: &'static [&'static str],
    pub default_shortcut: Option<&'static str>,
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

    pub fn from_keystroke(ks: &Keystroke) -> Self {
        // (Alt is the terminal's Meta, both keys, as in Olo's Terminal.)
        let alt = ks.modifiers.alt;
        Self { ctrl: ks.modifiers.control, alt, shift: ks.modifiers.shift, meta: ks.modifiers.platform, key: normalize_key_name(&ks.key) }
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

    pub fn format_human(&self) -> String {
        self.chords.iter().map(KeyChord::format_human).collect::<Vec<_>>().join(" › ")
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

/// Convert an Olo [`KeyDownEvent`] into a Rex rev-19 [`RexKeyEvent`].
pub fn to_rex_key_event(e: &KeyDownEvent) -> RexKeyEvent {
    let ks = &e.keystroke;
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
    let dir_schema = Some(json!({
        "type": "object",
        "properties": { "direction": { "type": "string", "enum": ["left", "right", "up", "down"] } }
    }));
    let idx_schema = Some(json!({
        "type": "object",
        "properties": { "index": { "type": "integer", "minimum": 1 } }
    }));
    vec![
        // ---- Application & Palettes ----
        ActionSpec {
            id: "app.command_palette",
            title: "Command Palette…",
            category: "General",
            keywords: &["commands", "actions", "search"],
            default_shortcut: Some("meta+shift+p"),
            args_schema: None,
        },
        ActionSpec {
            id: "app.go_to_anything", title: "Go to Anything…", category: "Navigation", keywords: &["quick", "switch", "jump", "fuzzy"], default_shortcut: Some("meta+p"), args_schema: None
        },
        ActionSpec {
            id: "app.view_activity",
            title: "View Activity",
            category: "Activity",
            keywords: &["agents", "blocked", "working", "status"],
            default_shortcut: Some("meta+period"),
            args_schema: None,
        },
        ActionSpec { id: "app.toggle_sidebar", title: "Toggle Sidebar", category: "View", keywords: &["hosts", "sessions", "drawer"], default_shortcut: Some("alt+meta+s"), args_schema: None },
        ActionSpec {
            id: "app.settings",
            title: "Rex Settings…",
            category: "General",
            keywords: &["preferences", "hosts", "permissions", "notifications"],
            default_shortcut: Some("meta+comma"),
            args_schema: None,
        },
        ActionSpec {
            id: "app.diagnostics",
            title: "Rex Diagnostics…",
            category: "Diagnostics",
            keywords: &["debug", "server", "status", "version"],
            default_shortcut: Some("meta+shift+comma"),
            args_schema: None,
        },
        ActionSpec {
            id: "app.hosts",
            title: "Hosts & Port Forwards…",
            category: "Network",
            keywords: &["ssh", "remote", "tailscale", "tunnels", "ports"],
            default_shortcut: Some("meta+shift+h"),
            args_schema: None,
        },
        ActionSpec { id: "app.pair_device", title: "Remote Access…", category: "Network", keywords: &["pair", "device", "phone", "qr", "tailscale"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "app.new_window", title: "New Window", category: "Window", keywords: &["window", "open"], default_shortcut: Some("meta+n"), args_schema: None },
        ActionSpec { id: "app.close_window", title: "Close Window", category: "Window", keywords: &["window", "close"], default_shortcut: Some("meta+shift+w"), args_schema: None },
        ActionSpec { id: "app.quit", title: "Quit Rex", category: "General", keywords: &["exit", "quit"], default_shortcut: Some("meta+q"), args_schema: None },
        ActionSpec {
            id: "app.reload_config",
            title: "Reload Server Configuration",
            category: "General",
            keywords: &["lua", "config", "reload"],
            default_shortcut: Some("meta+shift+r"),
            args_schema: None,
        },
        // ---- Sessions ----
        ActionSpec { id: "session.new", title: "New Session", category: "Session", keywords: &["create", "workspace", "session"], default_shortcut: Some("alt+meta+n"), args_schema: None },
        ActionSpec { id: "session.rename", title: "Rename Session…", category: "Session", keywords: &["label", "title"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "session.close", title: "Close Session", category: "Session", keywords: &["kill", "delete"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "session.next", title: "Next Session", category: "Session", keywords: &["cycle", "switch"], default_shortcut: Some("ctrl+meta+down"), args_schema: None },
        ActionSpec { id: "session.previous", title: "Previous Session", category: "Session", keywords: &["cycle", "switch"], default_shortcut: Some("ctrl+meta+up"), args_schema: None },
        ActionSpec { id: "session.detach", title: "Detach Session", category: "Session", keywords: &["disconnect"], default_shortcut: None, args_schema: None },
        // ---- Windows / Tabs ----
        ActionSpec { id: "window.new", title: "New Tab", category: "Tab", keywords: &["window", "tab", "shell"], default_shortcut: Some("meta+t"), args_schema: None },
        ActionSpec { id: "window.close", title: "Close Tab", category: "Tab", keywords: &["close", "tab"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "window.rename", title: "Rename Tab…", category: "Tab", keywords: &["title", "label"], default_shortcut: Some("meta+shift+i"), args_schema: None },
        ActionSpec { id: "window.next", title: "Next Tab", category: "Tab", keywords: &["right", "forward"], default_shortcut: Some("meta+shift+bracket_right"), args_schema: None },
        ActionSpec { id: "window.previous", title: "Previous Tab", category: "Tab", keywords: &["left", "back"], default_shortcut: Some("meta+shift+bracket_left"), args_schema: None },
        ActionSpec { id: "window.select", title: "Select Tab by Index", category: "Tab", keywords: &["jump", "number"], default_shortcut: None, args_schema: idx_schema },
        ActionSpec { id: "window.select_1", title: "Select Tab 1", category: "Tab", keywords: &["1"], default_shortcut: Some("meta+1"), args_schema: None },
        ActionSpec { id: "window.select_2", title: "Select Tab 2", category: "Tab", keywords: &["2"], default_shortcut: Some("meta+2"), args_schema: None },
        ActionSpec { id: "window.select_3", title: "Select Tab 3", category: "Tab", keywords: &["3"], default_shortcut: Some("meta+3"), args_schema: None },
        ActionSpec { id: "window.select_4", title: "Select Tab 4", category: "Tab", keywords: &["4"], default_shortcut: Some("meta+4"), args_schema: None },
        ActionSpec { id: "window.select_5", title: "Select Tab 5", category: "Tab", keywords: &["5"], default_shortcut: Some("meta+5"), args_schema: None },
        ActionSpec { id: "window.select_6", title: "Select Tab 6", category: "Tab", keywords: &["6"], default_shortcut: Some("meta+6"), args_schema: None },
        ActionSpec { id: "window.select_7", title: "Select Tab 7", category: "Tab", keywords: &["7"], default_shortcut: Some("meta+7"), args_schema: None },
        ActionSpec { id: "window.select_8", title: "Select Tab 8", category: "Tab", keywords: &["8"], default_shortcut: Some("meta+8"), args_schema: None },
        ActionSpec { id: "window.select_9", title: "Select Tab 9", category: "Tab", keywords: &["9"], default_shortcut: Some("meta+9"), args_schema: None },
        ActionSpec { id: "window.move_left", title: "Move Tab Left", category: "Tab", keywords: &["reorder"], default_shortcut: Some("ctrl+shift+left"), args_schema: None },
        ActionSpec { id: "window.move_right", title: "Move Tab Right", category: "Tab", keywords: &["reorder"], default_shortcut: Some("ctrl+shift+right"), args_schema: None },
        ActionSpec { id: "window.toggle_mute", title: "Mute / Unmute Tab Notifications", category: "Tab", keywords: &["silence", "quiet"], default_shortcut: None, args_schema: None },
        // ---- Splits & Panes ----
        ActionSpec { id: "pane.split_right", title: "Split Right", category: "Pane", keywords: &["vertical", "divide", "horizontal"], default_shortcut: Some("meta+d"), args_schema: None },
        ActionSpec { id: "pane.split_down", title: "Split Down", category: "Pane", keywords: &["horizontal", "divide", "below"], default_shortcut: Some("meta+shift+d"), args_schema: None },
        ActionSpec { id: "pane.split_best_fit", title: "Split Best Fit", category: "Pane", keywords: &["auto", "smart", "split"], default_shortcut: Some("meta+enter"), args_schema: None },
        ActionSpec { id: "pane.close", title: "Close Pane", category: "Pane", keywords: &["close", "kill", "block"], default_shortcut: Some("meta+w"), args_schema: None },
        ActionSpec { id: "pane.focus_left", title: "Focus Pane Left", category: "Pane", keywords: &["navigate", "left"], default_shortcut: Some("alt+meta+left"), args_schema: None },
        ActionSpec { id: "pane.focus_right", title: "Focus Pane Right", category: "Pane", keywords: &["navigate", "right"], default_shortcut: Some("alt+meta+right"), args_schema: None },
        ActionSpec { id: "pane.focus_up", title: "Focus Pane Up", category: "Pane", keywords: &["navigate", "up"], default_shortcut: Some("alt+meta+up"), args_schema: None },
        ActionSpec { id: "pane.focus_down", title: "Focus Pane Down", category: "Pane", keywords: &["navigate", "down"], default_shortcut: Some("alt+meta+down"), args_schema: None },
        ActionSpec { id: "pane.focus_direction", title: "Focus Pane in Direction", category: "Pane", keywords: &["direction"], default_shortcut: None, args_schema: dir_schema },
        ActionSpec { id: "pane.focus_next", title: "Focus Next Pane", category: "Pane", keywords: &["cycle"], default_shortcut: Some("meta+bracket_right"), args_schema: None },
        ActionSpec { id: "pane.focus_previous", title: "Focus Previous Pane", category: "Pane", keywords: &["cycle"], default_shortcut: Some("meta+bracket_left"), args_schema: None },
        ActionSpec { id: "pane.swap_left", title: "Swap Pane Left", category: "Pane", keywords: &["move", "exchange"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "pane.swap_right", title: "Swap Pane Right", category: "Pane", keywords: &["move", "exchange"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "pane.swap_up", title: "Swap Pane Up", category: "Pane", keywords: &["move", "exchange"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "pane.swap_down", title: "Swap Pane Down", category: "Pane", keywords: &["move", "exchange"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "pane.resize_left", title: "Resize Pane Left", category: "Pane", keywords: &["shrink", "grow"], default_shortcut: Some("ctrl+meta+left"), args_schema: None },
        ActionSpec { id: "pane.resize_right", title: "Resize Pane Right", category: "Pane", keywords: &["shrink", "grow"], default_shortcut: Some("ctrl+meta+right"), args_schema: None },
        ActionSpec { id: "pane.resize_up", title: "Resize Pane Up", category: "Pane", keywords: &["shrink", "grow"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "pane.resize_down", title: "Resize Pane Down", category: "Pane", keywords: &["shrink", "grow"], default_shortcut: None, args_schema: None },
        ActionSpec {
            id: "pane.zoom_toggle",
            title: "Zoom / Unzoom Pane",
            category: "Pane",
            keywords: &["maximize", "fullscreen", "focus"],
            default_shortcut: Some("meta+shift+enter"),
            args_schema: None,
        },
        ActionSpec { id: "pane.balance", title: "Balance Splits", category: "Pane", keywords: &["equalize", "reset"], default_shortcut: Some("ctrl+meta+equal"), args_schema: None },
        ActionSpec { id: "pane.break_to_tab", title: "Break Pane into New Tab", category: "Pane", keywords: &["extract", "promote"], default_shortcut: None, args_schema: None },
        ActionSpec {
            id: "pane.broadcast_toggle",
            title: "Toggle Broadcast Input to Window",
            category: "Pane",
            keywords: &["sync", "all panes"],
            default_shortcut: Some("alt+meta+i"),
            args_schema: None,
        },
        // ---- Floating Layers ----
        ActionSpec { id: "layer.new_floating", title: "New Floating Terminal", category: "Layers", keywords: &["overlay", "popup", "float"], default_shortcut: Some("alt+meta+f"), args_schema: None },
        ActionSpec { id: "layer.toggle_float", title: "Float / Dock Focused Pane", category: "Layers", keywords: &["floating", "tile"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "layer.minimize", title: "Minimize Floating Layer", category: "Layers", keywords: &["capsule", "hide"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "layer.restore_all", title: "Restore All Floating Layers", category: "Layers", keywords: &["unminimize", "show"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "layer.raise", title: "Raise Floating Layer", category: "Layers", keywords: &["front", "z-order"], default_shortcut: None, args_schema: None },
        // ---- Terminal, Clipboard, Find, Copy Mode, Navigation ----
        ActionSpec { id: "terminal.copy", title: "Copy", category: "Edit", keywords: &["clipboard", "yank"], default_shortcut: Some("meta+c"), args_schema: None },
        ActionSpec { id: "terminal.paste", title: "Paste", category: "Edit", keywords: &["clipboard"], default_shortcut: Some("meta+v"), args_schema: None },
        ActionSpec { id: "terminal.select_all", title: "Select All", category: "Edit", keywords: &["selection"], default_shortcut: Some("meta+a"), args_schema: None },
        ActionSpec { id: "terminal.clear_scrollback", title: "Clear Scrollback", category: "Terminal", keywords: &["cls", "clear"], default_shortcut: Some("meta+k"), args_schema: None },
        ActionSpec { id: "terminal.reset", title: "Reset Terminal", category: "Terminal", keywords: &["ris", "hard reset"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "terminal.find", title: "Find in Terminal…", category: "Edit", keywords: &["search", "grep"], default_shortcut: Some("meta+f"), args_schema: None },
        ActionSpec { id: "terminal.find_next", title: "Find Next", category: "Edit", keywords: &["search"], default_shortcut: Some("meta+g"), args_schema: None },
        ActionSpec { id: "terminal.find_previous", title: "Find Previous", category: "Edit", keywords: &["search"], default_shortcut: Some("meta+shift+g"), args_schema: None },
        ActionSpec {
            id: "terminal.copy_mode",
            title: "Enter / Exit Copy Mode",
            category: "Terminal",
            keywords: &["vim", "tmux", "scroll", "select"],
            default_shortcut: Some("meta+shift+c"),
            args_schema: None,
        },
        ActionSpec { id: "terminal.scroll_page_up", title: "Scroll Page Up", category: "Terminal", keywords: &["scrollback"], default_shortcut: Some("shift+page_up"), args_schema: None },
        ActionSpec { id: "terminal.scroll_page_down", title: "Scroll Page Down", category: "Terminal", keywords: &["scrollback"], default_shortcut: Some("shift+page_down"), args_schema: None },
        ActionSpec { id: "terminal.scroll_top", title: "Scroll to Top", category: "Terminal", keywords: &["scrollback", "home"], default_shortcut: Some("meta+home"), args_schema: None },
        ActionSpec { id: "terminal.scroll_bottom", title: "Scroll to Bottom", category: "Terminal", keywords: &["scrollback", "end"], default_shortcut: Some("meta+end"), args_schema: None },
        ActionSpec { id: "terminal.prompt_prev", title: "Jump to Previous Prompt", category: "Terminal", keywords: &["osc133", "command"], default_shortcut: Some("meta+up"), args_schema: None },
        ActionSpec { id: "terminal.prompt_next", title: "Jump to Next Prompt", category: "Terminal", keywords: &["osc133", "command"], default_shortcut: Some("meta+down"), args_schema: None },
        ActionSpec {
            id: "terminal.select_output",
            title: "Select Last Command Output",
            category: "Terminal",
            keywords: &["osc133", "copy"],
            default_shortcut: Some("meta+shift+o"),
            args_schema: None,
        },
        ActionSpec { id: "terminal.font_increase", title: "Increase Font Size", category: "View", keywords: &["zoom", "bigger"], default_shortcut: Some("meta+equal"), args_schema: None },
        ActionSpec { id: "terminal.font_decrease", title: "Decrease Font Size", category: "View", keywords: &["zoom", "smaller"], default_shortcut: Some("meta+minus"), args_schema: None },
        ActionSpec { id: "terminal.font_reset", title: "Reset Font Size", category: "View", keywords: &["zoom", "actual"], default_shortcut: Some("meta+0"), args_schema: None },
        ActionSpec { id: "terminal.export_text", title: "Export Screen as Plain Text…", category: "Terminal", keywords: &["save", "dump"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "terminal.export_html", title: "Export Screen as HTML…", category: "Terminal", keywords: &["save", "html"], default_shortcut: None, args_schema: None },
        // ---- Files, Path Browser, Quick Look ----
        ActionSpec { id: "files.path_browser", title: "Path Browser…", category: "Files", keywords: &["insert path", "browse", "files"], default_shortcut: Some("ctrl+meta+i"), args_schema: None },
        ActionSpec { id: "files.go_to_dir", title: "Go to Directory…", category: "Files", keywords: &["cd", "folder", "mru"], default_shortcut: Some("ctrl+shift+g"), args_schema: None },
        ActionSpec { id: "files.quick_look", title: "Quick Look Path…", category: "Files", keywords: &["preview", "inspect"], default_shortcut: Some("meta+y"), args_schema: None },
        ActionSpec { id: "files.upload", title: "Upload File to Pane…", category: "Files", keywords: &["send", "drop", "remote"], default_shortcut: None, args_schema: None },
        ActionSpec {
            id: "files.open_in_tracker",
            title: "Open Working Directory in Tracker",
            category: "Files",
            keywords: &["finder", "folder", "tracker"],
            default_shortcut: None,
            args_schema: None,
        },
        // ---- Coding Agents & Automation ----
        ActionSpec { id: "agent.keep_going_toggle", title: "Toggle Keep It Going", category: "Agents", keywords: &["nudge", "continue", "autonomous"], default_shortcut: None, args_schema: None },
        ActionSpec {
            id: "agent.jump_next_blocked",
            title: "Jump to Next Blocked Agent",
            category: "Agents",
            keywords: &["attention", "prompt", "waiting"],
            default_shortcut: Some("meta+shift+period"),
            args_schema: None,
        },
        ActionSpec { id: "agent.mark_all_read", title: "Mark All Read", category: "Agents", keywords: &["clear", "unread", "badge"], default_shortcut: None, args_schema: None },
        // ---- Key Modes ----
        ActionSpec {
            id: "client.mode.enter",
            title: "Enter Key Mode",
            category: "Keymap",
            keywords: &["mode", "vim", "resize"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"mode":{"type":"string"},"once":{"type":"boolean"}},"required":["mode"]})),
        },
        ActionSpec { id: "client.mode.exit", title: "Exit Key Mode", category: "Keymap", keywords: &["mode", "leave"], default_shortcut: None, args_schema: None },
        ActionSpec { id: "client.mode.exit_all", title: "Exit All Key Modes", category: "Keymap", keywords: &["mode", "reset"], default_shortcut: None, args_schema: None },
        // ---- Olo Desktop Bridge Actions ----
        ActionSpec {
            id: "olo.open",
            title: "Open URL or File in Olo",
            category: "Olo Bridge",
            keywords: &["browser", "chrome", "xdg-open"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"target":{"type":"string"},"autoforward":{"type":"boolean"}},"required":["target"]})),
        },
        ActionSpec {
            id: "olo.notify",
            title: "Send Olo Desktop Notification",
            category: "Olo Bridge",
            keywords: &["notification", "alert"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"}},"required":["title"]})),
        },
        ActionSpec {
            id: "olo.clipboard.set",
            title: "Set Olo Clipboard",
            category: "Olo Bridge",
            keywords: &["copy", "clipboard"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})),
        },
        ActionSpec {
            id: "olo.screen.ensure",
            title: "Ensure Olo Agent Screen",
            category: "Olo Bridge",
            keywords: &["agent screen", "headless"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})),
        },
        ActionSpec {
            id: "olo.screen.watch",
            title: "Watch Olo Agent Screen",
            category: "Olo Bridge",
            keywords: &["agent screen", "watch"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})),
        },
        ActionSpec {
            id: "olo.screen.open_url",
            title: "Open URL on Olo Agent Screen",
            category: "Olo Bridge",
            keywords: &["agent screen", "chrome", "preview"],
            default_shortcut: None,
            args_schema: Some(json!({"type":"object","properties":{"name":{"type":"string"},"url":{"type":"string"}},"required":["name","url"]})),
        },
        ActionSpec {
            id: "olo.passwords.fill",
            title: "Fill Password from Olo Passwords",
            category: "Olo Bridge",
            keywords: &["keychain", "op", "secret"],
            default_shortcut: Some("alt+meta+backslash"),
            args_schema: None,
        },
        ActionSpec {
            id: "olo.install_remote_shim",
            title: "Install Olo Bridge Shim on Current Host…",
            category: "Olo Bridge",
            keywords: &["shim", "xdg-open", "op", "olo-screen", "remote"],
            default_shortcut: None,
            args_schema: None,
        },
        ActionSpec {
            id: "olo.install_remote_skill",
            title: "Install Rex & Olo Agent Skill on Current Host…",
            category: "Olo Bridge",
            keywords: &["skill", "claude", "codex", "agent"],
            default_shortcut: None,
            args_schema: None,
        },
        ActionSpec { id: "olo.theme.sync", title: "Sync Olo Theme to Server", category: "Olo Bridge", keywords: &["colors", "theme", "sync"], default_shortcut: None, args_schema: None },
    ]
}

/// Convert the catalog into the `client.keymap` wire actions payload.
pub fn register_actions_payload() -> Vec<KeymapAction> {
    action_catalog()
        .into_iter()
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

/// Default client keymap layer sent to `client.keymap`.
pub fn default_client_layer() -> KeymapLayer {
    let bindings = action_catalog()
        .into_iter()
        .filter_map(|a| {
            let raw = a.default_shortcut?;
            Some(KeymapBinding { keys: vec![raw.to_owned()], action: a.id.to_owned(), args: None, source: Some("default".to_owned()), mode: None })
        })
        .collect();
    KeymapLayer { name: "olo-rex-defaults".to_owned(), bindings, removals: Vec::new() }
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
        Self::new(&[], &[], &[], &[])
    }
}

impl KeymapEngine {
    pub fn new(user_overrides: &[KeymapLayer], server_bindings: &[KeymapBinding], server_modes: &[KeymapMode], diagnostics: &[KeymapDiagnostic]) -> Self {
        let titles: HashMap<String, String> = action_catalog().into_iter().map(|a| (a.id.to_owned(), a.title.to_owned())).collect();
        let mut engine = Self { bindings_by_mode: HashMap::new(), modes: HashMap::new(), diagnostics: Vec::new(), mode_stack: Vec::new(), pending_prefix: Vec::new(), pending_since: None, titles };
        engine.rebuild(user_overrides, server_bindings, server_modes, diagnostics);
        engine
    }

    pub fn rebuild(&mut self, user_overrides: &[KeymapLayer], server_bindings: &[KeymapBinding], server_modes: &[KeymapMode], diagnostics: &[KeymapDiagnostic]) {
        self.bindings_by_mode.clear();
        self.modes.clear();
        self.diagnostics = diagnostics.to_vec();

        let mut by_mode: HashMap<String, HashMap<KeySequence, ResolvedBinding>> = HashMap::new();
        let normal = by_mode.entry(String::new()).or_default();
        for spec in action_catalog() {
            if let Some(raw) = spec.default_shortcut
                && let Some(seq) = KeySequence::parse(raw)
            {
                normal.insert(seq.clone(), ResolvedBinding { sequence: seq, action: spec.id.to_owned(), args: None, description: Some(spec.title.to_owned()), source: "default".to_owned() });
            }
        }

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

        // Apply local Settings user override layers last.
        for layer in user_overrides {
            for rem in &layer.removals {
                let mode_key = rem.mode.clone().unwrap_or_default();
                if let Some(seq) = KeySequence::from_wire_keys(&rem.keys)
                    && let Some(table) = by_mode.get_mut(&mode_key)
                {
                    table.remove(&seq);
                }
            }
            for b in &layer.bindings {
                let mode_key = b.mode.clone().unwrap_or_default();
                if let Some(seq) = KeySequence::from_wire_keys(&b.keys) {
                    let desc = self.titles.get(&b.action).cloned();
                    by_mode.entry(mode_key).or_default().insert(
                        seq.clone(),
                        ResolvedBinding { sequence: seq, action: b.action.clone(), args: b.args.clone(), description: desc, source: b.source.clone().unwrap_or_else(|| layer.name.clone()) },
                    );
                }
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

    pub fn shortcut_for(&self, action_id: &str) -> Option<String> {
        self.bindings_by_mode.get("")?.iter().find(|b| b.action == action_id).map(|b| b.sequence.format_human())
    }

    pub fn handle_key(&mut self, ks: &Keystroke) -> KeyMatch {
        if let Some(since) = self.pending_since
            && since.elapsed() > Duration::from_secs(2)
        {
            self.clear_pending();
        }

        let chord = KeyChord::from_keystroke(ks);

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
