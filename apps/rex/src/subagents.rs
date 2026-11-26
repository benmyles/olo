//! Native nested subagent tabs in Olo Rex.
//!
//! Agents running inside a Rex terminal launch child subagents (`claude`,
//! `codex`, `agy`) as real interactive Rex terminal tabs linked to their
//! parent tab (`parent_window_id`). Only top-level parent tabs appear in the
//! horizontal tab bar; tabs with child tabs show a reusable Olo Kit tree icon
//! button to the left of `×` that opens a hierarchical tab switcher, and Rex's
//! Activity sidebar shows the same live parent/child/grandchild tree.
//!
//! Every launch (`olo agent start` / `olo.agent.start`) requires explicitly
//! specifying both `--model` and `--effort` so there are never surprises about
//! which model or reasoning effort a subagent runs with.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Supported first-class agent CLIs for native Rex child-tab subagents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentCli {
    Claude,
    Codex,
    Agy,
}

impl AgentCli {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" | "antigravity" => Some(Self::Agy),
            _ => None,
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Agy => "Antigravity",
        }
    }

    /// Lowercase product label for window chrome (sidebar, popover, header);
    /// uses `"antigravity"` instead of the CLI shorthand `"agy"`.
    pub fn chrome_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "antigravity",
        }
    }

    pub fn screen_agent(self) -> olo_agent_screen::Agent {
        match self {
            Self::Claude => olo_agent_screen::Agent::Claude,
            Self::Codex => olo_agent_screen::Agent::Codex,
            Self::Agy => olo_agent_screen::Agent::Antigravity,
        }
    }
}

/// Per-child desktop access policy enforced server-side by Olo Rex.
/// Default for every child tab is `DesktopPolicy::None` so builders never
/// reach the person's desktop (`op`, `olo screen`, `olo open`, clipboard, etc.).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DesktopPolicy {
    /// No desktop actions allowed (`op`, `screen`, `open`, `run`, `clipboard`, `send`, `sync`, `get`, `ls`, `forward`, `notify` all refused).
    #[default]
    None,
    /// Only `olo screen` commands targeting one of the listed Agent Screen names are allowed; `op` and all other desktop actions are refused.
    Screens { names: Vec<String> },
    /// Inherit the host's normal desktop bridge policy.
    Inherit,
}

impl DesktopPolicy {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        let low = s.to_ascii_lowercase();
        if low == "none" || low == "off" || low == "deny" {
            return Ok(Self::None);
        }
        if low == "inherit" || low == "host" || low == "allow" {
            return Ok(Self::Inherit);
        }
        if let Some(rest) = s.strip_prefix("screens=").or_else(|| s.strip_prefix("screen=")) {
            let names: Vec<String> = rest.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect();
            if names.is_empty() {
                return Err("--desktop screens=NAME,… requires at least one screen name".into());
            }
            return Ok(Self::Screens { names });
        }
        Err(format!("invalid --desktop policy `{raw}`; expected `none`, `screens=NAME,…`, or `inherit`"))
    }

    pub fn describe(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::Screens { names } => format!("screens={}", names.join(",")),
            Self::Inherit => "inherit".into(),
        }
    }

    /// Enforces this policy against an incoming desktop bridge action (`olo.*`).
    /// Note: `olo.agent.*` is handled separately (subagent orchestration), while
    /// all desktop-touching actions are strictly gated here.
    pub fn check_action(&self, action: &str, args: Option<&Value>) -> Result<(), String> {
        if matches!(self, Self::Inherit) {
            return Ok(());
        }
        // Only desktop actions are gated; agent orchestration & wait/file pieces are checked by their own ownership.
        let is_desktop_action = matches!(
            action,
            "olo.op"
                | "olo.screen"
                | "olo.open"
                | "olo.run"
                | "olo.run.poll"
                | "olo.run.cancel"
                | "olo.clipboard.get"
                | "olo.clipboard.set"
                | "olo.send"
                | "olo.sync"
                | "olo.get"
                | "olo.ls"
                | "olo.forward"
                | "olo.notify"
        );
        if !is_desktop_action {
            return Ok(());
        }
        match self {
            Self::Inherit => Ok(()),
            Self::None => Err(format!("action `{action}` refused: this child subagent runs with `--desktop none` (desktop access disabled)")),
            Self::Screens { names } => {
                if action != "olo.screen" {
                    return Err(format!("action `{action}` refused: this child subagent is restricted to `--desktop screens={}` (only `olo screen` on those screens is allowed)", names.join(",")));
                }
                let screen_args: Vec<String> = args.and_then(|v| v.get("args")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                let Some(target_screen) = screen_target_name(&screen_args) else {
                    return Err(format!("`olo screen` command refused: child is restricted to screens [{}] and may only target a named allowed screen", names.join(", ")));
                };
                if names.iter().any(|allowed| allowed == target_screen) {
                    Ok(())
                } else {
                    Err(format!("`olo screen` on `{target_screen}` refused: child is restricted to screens [{}]", names.join(", ")))
                }
            }
        }
    }
}

/// Extracts the target Agent Screen name from `olo screen <subcmd> [flags] <NAME> ...`.
/// Returns `None` for global commands like `list` or help without a screen name.
pub fn screen_target_name(args: &[String]) -> Option<&str> {
    let mut it = args.iter().map(String::as_str);
    let sub = it.next()?;
    if matches!(sub, "list" | "ls" | "-h" | "--help" | "--version") {
        return None;
    }
    let mut skip_next = false;
    for tok in it {
        if skip_next {
            skip_next = false;
            continue;
        }
        if tok == "--" {
            break;
        }
        if matches!(tok, "--size" | "--scale" | "--theme" | "--timeout" | "--home" | "--owner") {
            skip_next = true;
            continue;
        }
        if tok.starts_with('-') {
            continue;
        }
        return Some(tok);
    }
    None
}

/// One registered native Rex child-tab subagent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSubagent {
    /// Short unique handle (e.g. `sa-3f9a1c`) for CLI commands.
    pub id: String,
    /// Optional human-chosen unique name under the parent (`--name NAME`).
    #[serde(default)]
    pub name: Option<String>,
    pub host_key: String,
    pub session_id: String,
    /// The child tab's own Rex `window_id`.
    pub window_id: String,
    /// Primary terminal `block_id` inside `window_id`.
    pub block_id: String,
    /// Parent Rex `window_id` (may itself be another `NativeSubagent` for nested trees).
    pub parent_window_id: String,
    #[serde(default)]
    pub parent_block_id: Option<String>,
    pub cli: AgentCli,
    pub model: String,
    pub effort: String,
    pub title: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub resume_id: Option<String>,
    /// Real CLI session/conversation UUID discovered on the host (persisted for `--resume` after exit/restart).
    #[serde(default)]
    pub cli_session_id: Option<String>,
    /// On-disk transcript file path on the host (persisted for inspection after exit).
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Server-enforced desktop isolation policy (`none` by default).
    #[serde(default)]
    pub desktop_policy: DesktopPolicy,
    /// True if launched in non-interactive stream-json mode (e.g. `agy --print --output-format stream-json`).
    #[serde(default)]
    pub stream_mode: bool,
    #[serde(default)]
    pub started_at: i64,
    /// Set when the child tab closed or disappeared after a Rex server restart (kept for `list --all` & `--resume`).
    #[serde(default)]
    pub exited_at: Option<i64>,
    #[serde(default)]
    pub last_pid: Option<i32>,
}

impl NativeSubagent {
    /// Human-readable label shown in the tab-tree popover and Activity sidebar.
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            self.title.clone()
        } else if let Some(n) = self.name.as_deref().filter(|s| !s.trim().is_empty()) {
            n.to_owned()
        } else if let Some(role) = self.role.as_deref().filter(|r| !r.trim().is_empty()) {
            role.to_owned()
        } else {
            format!("{} ({})", self.cli.display_name(), self.id)
        }
    }

    /// Best known CLI session ID (`cli_session_id` if discovered, else `resume_id`).
    pub fn effective_session_id(&self) -> Option<&str> {
        self.cli_session_id.as_deref().filter(|s| !s.is_empty()).or_else(|| self.resume_id.as_deref().filter(|s| !s.is_empty()))
    }
}

/// Persistent parent/child registry of native Rex child-tab subagents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubagentRegistry {
    /// Ordered by launch time so sibling order is stable.
    pub entries: Vec<NativeSubagent>,
}

const MAX_EXITED_HISTORY: usize = 200;
const EXITED_TTL_SECS: i64 = 7 * 86_400;

impl SubagentRegistry {
    pub fn state_path() -> PathBuf {
        olo_xdg::state_dir().join("rex-subagents.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::state_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        self.save_to(&Self::state_path());
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(self)
            && std::fs::write(&tmp, json).is_ok()
        {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    pub fn register(&mut self, entry: NativeSubagent) {
        self.entries.retain(|e| !(e.host_key == entry.host_key && e.session_id == entry.session_id && e.window_id == entry.window_id));
        self.entries.push(entry);
    }

    /// True if an active (non-exited) child under the same parent already uses `name`.
    pub fn is_live_name_taken(&self, host_key: &str, session_id: &str, parent_window_id: &str, name: &str, live_windows: &HashSet<String>) -> bool {
        let q = name.trim();
        if q.is_empty() {
            return false;
        }
        self.entries.iter().any(|e| {
            e.host_key == host_key
                && e.session_id == session_id
                && e.parent_window_id == parent_window_id
                && e.exited_at.is_none()
                && live_windows.contains(&e.window_id)
                && e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(q))
        })
    }

    /// Marks `window_id` as exited (preserving its `cli_session_id` and metadata for
    /// `list --all` and `--resume`), reparenting any surviving children upward.
    pub fn mark_exited(&mut self, host_key: &str, session_id: &str, window_id: &str) -> bool {
        let parent = self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == window_id).map(|e| e.parent_window_id.clone());
        let mut changed = false;
        let now = now_unix();
        for e in &mut self.entries {
            if e.host_key == host_key && e.session_id == session_id && e.window_id == window_id && e.exited_at.is_none() {
                e.exited_at = Some(now);
                changed = true;
            }
        }
        if let Some(grandparent) = parent {
            for e in &mut self.entries {
                if e.host_key == host_key && e.session_id == session_id && e.exited_at.is_none() && e.parent_window_id == window_id {
                    e.parent_window_id = grandparent.clone();
                    changed = true;
                }
            }
        }
        self.trim_old_exited();
        changed
    }

    /// Marks entries whose Rex window no longer exists in `live_windows` as `exited`
    /// (preserving `cli_session_id` so after a Rex server restart `list --all` shows
    /// every exited child with its session ID ready for `--resume`). Returns the newly
    /// exited entries so callers can emit `exited` events.
    pub fn sync_live_windows(&mut self, host_key: &str, session_id: &str, live_windows: &HashSet<String>) -> (bool, Vec<NativeSubagent>) {
        let parent_map: HashMap<String, String> =
            self.entries.iter().filter(|e| e.host_key == host_key && e.session_id == session_id).map(|e| (e.window_id.clone(), e.parent_window_id.clone())).collect();
        let mut changed = false;
        let mut newly_exited = Vec::new();
        let now = now_unix();
        for e in &mut self.entries {
            if e.host_key == host_key && e.session_id == session_id && e.exited_at.is_none() && !live_windows.contains(&e.window_id) {
                e.exited_at = Some(now);
                newly_exited.push(e.clone());
                changed = true;
            }
        }
        // Reparent any remaining live child whose direct parent died to its nearest living ancestor.
        for e in &mut self.entries {
            if e.host_key != host_key || e.session_id != session_id || e.exited_at.is_some() {
                continue;
            }
            if !live_windows.contains(&e.parent_window_id) {
                let mut cur = e.parent_window_id.clone();
                let mut seen = HashSet::new();
                seen.insert(e.window_id.clone());
                while let Some(next) = parent_map.get(&cur) {
                    if !seen.insert(cur.clone()) {
                        break;
                    }
                    if live_windows.contains(next) {
                        e.parent_window_id = next.clone();
                        changed = true;
                        break;
                    }
                    cur = next.clone();
                }
            }
        }
        if self.trim_old_exited() {
            changed = true;
        }
        (changed, newly_exited)
    }

    fn trim_old_exited(&mut self) -> bool {
        let cutoff = now_unix() - EXITED_TTL_SECS;
        let before = self.entries.len();
        self.entries.retain(|e| e.exited_at.is_none_or(|t| t >= cutoff));
        let exited_count = self.entries.iter().filter(|e| e.exited_at.is_some()).count();
        if exited_count > MAX_EXITED_HISTORY {
            let mut to_drop = exited_count - MAX_EXITED_HISTORY;
            self.entries.retain(|e| {
                if to_drop > 0 && e.exited_at.is_some() {
                    to_drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
        self.entries.len() != before
    }

    pub fn prune_exited(&mut self, host_key: &str, session_id: Option<&str>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| !(e.host_key == host_key && session_id.is_none_or(|s| e.session_id == s) && e.exited_at.is_some()));
        before - self.entries.len()
    }

    pub fn update_discovered(&mut self, id: &str, cli_session_id: Option<&str>, transcript_path: Option<&str>, pid: Option<i32>) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { return false };
        let mut changed = false;
        if e.cli_session_id.is_none()
            && let Some(sid) = cli_session_id.filter(|s| !s.is_empty())
        {
            e.cli_session_id = Some(sid.to_owned());
            changed = true;
        }
        if let Some(tp) = transcript_path.filter(|s| !s.is_empty())
            && e.transcript_path.as_deref() != Some(tp)
        {
            e.transcript_path = Some(tp.to_owned());
            changed = true;
        }
        if let Some(p) = pid
            && e.last_pid != Some(p)
        {
            e.last_pid = Some(p);
        }
        changed
    }

    pub fn by_window(&self, host_key: &str, session_id: &str, window_id: &str) -> Option<&NativeSubagent> {
        self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == window_id && e.exited_at.is_none())
    }

    pub fn by_block(&self, host_key: &str, session_id: Option<&str>, block_id: &str) -> Option<&NativeSubagent> {
        // Check active entries first, then any entry matching block_id on this host (even if session_id was omitted/spoofed!).
        self.entries
            .iter()
            .find(|e| e.host_key == host_key && session_id.is_none_or(|s| e.session_id == s) && e.block_id == block_id && e.exited_at.is_none())
            .or_else(|| self.entries.iter().find(|e| e.host_key == host_key && e.block_id == block_id && e.exited_at.is_none()))
    }

    /// True when `window_id` is a registered live child tab whose ancestor chain still connects to a live parent window.
    pub fn is_child_window(&self, host_key: &str, session_id: &str, window_id: &str, live_windows: &HashSet<String>) -> bool {
        self.by_window(host_key, session_id, window_id).is_some_and(|e| e.parent_window_id != window_id && live_windows.contains(&e.parent_window_id))
    }

    /// Walks up `parent_window_id` to find the top-level parent tab `window_id` shown in `TabStrip`.
    pub fn root_ancestor(&self, host_key: &str, session_id: &str, window_id: &str, live_windows: &HashSet<String>) -> String {
        let mut cur = window_id.to_owned();
        let mut visited = HashSet::new();
        visited.insert(cur.clone());
        while let Some(entry) = self.by_window(host_key, session_id, &cur) {
            if !live_windows.contains(&entry.parent_window_id) || !visited.insert(entry.parent_window_id.clone()) {
                break;
            }
            cur = entry.parent_window_id.clone();
        }
        cur
    }

    /// Direct live children of `parent_window_id` in stable launch order.
    pub fn direct_children(&self, host_key: &str, session_id: &str, parent_window_id: &str, live_windows: &HashSet<String>) -> Vec<NativeSubagent> {
        self.entries
            .iter()
            .filter(|e| {
                e.host_key == host_key
                    && e.session_id == session_id
                    && e.exited_at.is_none()
                    && e.parent_window_id == parent_window_id
                    && e.window_id != parent_window_id
                    && live_windows.contains(&e.window_id)
            })
            .cloned()
            .collect()
    }

    /// Exited entries for `host_key` (optionally scoped to `session_id` / `parent_window_id`).
    pub fn exited_entries(&self, host_key: &str, session_id: Option<&str>, parent_window_id: Option<&str>) -> Vec<NativeSubagent> {
        self.entries
            .iter()
            .filter(|e| e.host_key == host_key && e.exited_at.is_some() && session_id.is_none_or(|sid| e.session_id == sid) && parent_window_id.is_none_or(|p| e.parent_window_id == p))
            .cloned()
            .collect()
    }

    /// All live descendants of `root_window_id` in DFS tree order with `(depth, is_last_sibling, ancestor_continuations, entry)`.
    /// `depth` starts at `1` for direct children of `root_window_id`.
    pub fn descendants_tree(&self, host_key: &str, session_id: &str, root_window_id: &str, live_windows: &HashSet<String>) -> Vec<TreeRow> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(root_window_id.to_owned());
        self.walk_tree(host_key, session_id, root_window_id, live_windows, 1, &[], &mut visited, &mut out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_tree(
        &self,
        host_key: &str,
        session_id: &str,
        parent_window_id: &str,
        live_windows: &HashSet<String>,
        depth: usize,
        guides: &[bool],
        visited: &mut HashSet<String>,
        out: &mut Vec<TreeRow>,
    ) {
        let kids = self.direct_children(host_key, session_id, parent_window_id, live_windows);
        let len = kids.len();
        for (idx, kid) in kids.into_iter().enumerate() {
            if !visited.insert(kid.window_id.clone()) {
                continue;
            }
            let is_last = idx + 1 == len;
            let wid = kid.window_id.clone();
            out.push(TreeRow { depth, is_last, guides: guides.to_vec(), entry: kid });
            let mut next_guides = guides.to_vec();
            next_guides.push(!is_last);
            self.walk_tree(host_key, session_id, &wid, live_windows, depth + 1, &next_guides, visited, out);
        }
    }

    /// Resolve a user/agent target by `id`, `name`, CLI `session_id`, `window_id`, `block_id`, or unique title/prefix.
    /// Live children are preferred over exited children when a name was reused.
    pub fn resolve_target(&self, host_key: &str, session_id: Option<&str>, target: &str) -> Option<NativeSubagent> {
        let q = target.trim();
        if q.is_empty() {
            return None;
        }
        let ql = q.to_ascii_lowercase();
        let matches_scope = |e: &&NativeSubagent, live_only: bool| e.host_key == host_key && session_id.is_none_or(|sid| e.session_id == sid) && (!live_only || e.exited_at.is_none());
        for live_only in [true, false] {
            if let Some(e) = self.entries.iter().rev().filter(|e| matches_scope(e, live_only)).find(|e| {
                e.id == q
                    || e.window_id == q
                    || e.block_id == q
                    || e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(q))
                    || e.cli_session_id.as_deref().is_some_and(|s| s == q)
                    || e.resume_id.as_deref().is_some_and(|s| s == q)
            }) {
                return Some(e.clone());
            }
            let exact_title: Vec<&NativeSubagent> = self.entries.iter().filter(|e| matches_scope(e, live_only)).filter(|e| e.title.to_ascii_lowercase() == ql).collect();
            if exact_title.len() == 1 {
                return Some(exact_title[0].clone());
            }
            let pref: Vec<&NativeSubagent> = self
                .entries
                .iter()
                .filter(|e| matches_scope(e, live_only))
                .filter(|e| e.id.starts_with(q) || e.window_id.starts_with(q) || e.cli_session_id.as_deref().is_some_and(|s| s.starts_with(q)))
                .collect();
            if pref.len() == 1 {
                return Some(pref[0].clone());
            }
        }
        None
    }
}

/// One row in a flattened DFS child-tab tree for popup menus and the Activity sidebar.
#[derive(Clone, Debug)]
pub struct TreeRow {
    pub depth: usize,
    pub is_last: bool,
    /// For each ancestor level `0..depth-1`, true if that ancestor has more siblings below.
    pub guides: Vec<bool>,
    pub entry: NativeSubagent,
}

impl TreeRow {
    /// Clean ASCII/box-drawing tree prefix for CLI list/table output (`├─ `, `└─ `, `│  ├─ `).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn menu_prefix(&self) -> String {
        let mut s = String::new();
        for &more in &self.guides {
            s.push_str(if more { "│  " } else { "   " });
        }
        s.push_str(if self.is_last { "└─ " } else { "├─ " });
        s
    }
}

/// One monotonic event recorded when a child subagent transitions state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentEvent {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub host_key: String,
    pub session_id: String,
    pub parent_window_id: String,
    pub id: String,
    pub name: Option<String>,
    pub window_id: String,
    pub cli_session_id: Option<String>,
    pub event: String,
    pub state: SubagentLiveState,
    pub summary: Option<String>,
    pub last_message: Option<String>,
    pub approval: Option<Value>,
    pub error: Option<String>,
    pub tail: Vec<String>,
}

const MAX_EVENT_LOG: usize = 2000;

/// Monotonic ring buffer of child subagent state transitions so back-to-back
/// `olo agent watch --all --since CURSOR` calls never lose an event.
#[derive(Debug, Default)]
pub struct SubagentEventLog {
    next_seq: u64,
    events: VecDeque<SubagentEvent>,
}

impl SubagentEventLog {
    pub fn latest_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn push(&mut self, mut ev: SubagentEvent) -> u64 {
        self.next_seq += 1;
        ev.seq = self.next_seq;
        self.events.push_back(ev);
        while self.events.len() > MAX_EVENT_LOG {
            self.events.pop_front();
        }
        self.next_seq
    }

    /// Finds the earliest event strictly after `since` matching `predicate`.
    pub fn find_since<F>(&self, since: u64, mut predicate: F) -> Option<SubagentEvent>
    where
        F: FnMut(&SubagentEvent) -> bool,
    {
        self.events.iter().find(|e| e.seq > since && predicate(e)).cloned()
    }
}

/// Parsed & validated request for `olo agent start` / `olo.agent.start`.
#[derive(Clone, Debug, Default)]
pub struct StartSpec {
    pub cli: Option<AgentCli>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub resume: Option<String>,
    pub session_id: Option<String>,
    pub continue_last: bool,
    pub fork: bool,
    pub prompt: Option<String>,
    pub title: Option<String>,
    pub role: Option<String>,
    pub cwd: Option<String>,
    pub parent_window: Option<String>,
    pub env: Vec<(String, String)>,
    pub unset_env: Vec<String>,
    pub desktop: DesktopPolicy,
    pub trust: bool,
    pub focus: bool,
    pub close_on_exit: bool,
    pub json: bool,
    // Permissions / approval / sandbox / mode options across claude, codex, agy
    pub permission_mode: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub mode: Option<String>,
    pub dangerously_skip_permissions: bool,
    pub allow_dangerously_skip_permissions: bool,
    pub full_auto: bool,
    pub approve_for_me: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub add_dirs: Vec<String>,
    pub worktree: Option<Option<String>>,
    pub agent: Option<String>,
    pub name: Option<String>,
    pub profile: Option<String>,
    pub configs: Vec<String>,
    pub settings: Option<String>,
    pub strict_mcp_config: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub mcp_configs: Vec<String>,
    pub project: Option<String>,
    pub new_project: bool,
    pub remote_agent: bool,
    pub remote_control: bool,
    pub search: bool,
    pub enables: Vec<String>,
    pub disables: Vec<String>,
    pub images: Vec<String>,
    pub no_alt_screen: bool,
    pub no_daemon: bool,
    pub print_mode: bool,
    pub input_format: Option<String>,
    pub output_format: Option<String>,
    /// Arbitrary passthrough flags/arguments after `--`.
    pub extra: Vec<String>,
}

fn looks_like_tool_token(tok: &str) -> bool {
    !tok.is_empty()
        && !tok.starts_with('-')
        && !tok.contains(char::is_whitespace)
        && tok.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '*')
        && tok.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '(' | ')' | '*' | '/' | '.' | ','))
}

impl StartSpec {
    /// Parses either structured JSON (`rex do olo.agent.start --args '{...}'`) or CLI args (`olo agent start …`).
    pub fn parse(raw: Option<&Value>) -> Result<Self, String> {
        let Some(v) = raw else {
            return Err(start_usage());
        };
        if let Some(arr) = v.get("args").and_then(Value::as_array) {
            let strs: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
            let mut spec = Self::parse_cli_args(&strs)?;
            if spec.cwd.is_none()
                && let Some(c) = v.get("cwd").and_then(Value::as_str).filter(|s| !s.is_empty())
            {
                spec.cwd = Some(c.to_owned());
            }
            return Ok(spec);
        }
        Self::parse_json_object(v)
    }

    pub fn parse_json_object(v: &Value) -> Result<Self, String> {
        let str_opt = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        let bool_val = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let str_vec = |k: &str| -> Vec<String> {
            match v.get(k) {
                Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
                _ => Vec::new(),
            }
        };
        let cli = match str_opt("cli").or_else(|| str_opt("agent_cli")) {
            Some(c) => Some(AgentCli::parse(&c).ok_or_else(|| format!("unsupported agent CLI `{c}`; expected `claude`, `codex`, or `agy`"))?),
            None => None,
        };
        let mut env = Vec::new();
        if let Some(obj) = v.get("env").and_then(Value::as_object) {
            for (k, val) in obj {
                env.push((k.clone(), val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string())));
            }
        }
        let desktop = match str_opt("desktop") {
            Some(raw) => DesktopPolicy::parse(&raw)?,
            None => DesktopPolicy::None,
        };
        let worktree = match v.get("worktree") {
            Some(Value::Bool(true)) => Some(None),
            Some(Value::String(s)) if !s.trim().is_empty() => Some(Some(s.clone())),
            _ => None,
        };
        let sandbox = match v.get("sandbox") {
            Some(Value::Bool(true)) => Some("true".to_owned()),
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            _ => None,
        };
        Ok(Self {
            cli,
            model: str_opt("model"),
            effort: str_opt("effort"),
            resume: str_opt("resume"),
            session_id: str_opt("session_id"),
            continue_last: bool_val("continue") || bool_val("last"),
            fork: bool_val("fork"),
            prompt: str_opt("prompt"),
            title: str_opt("title"),
            role: str_opt("role"),
            cwd: str_opt("cwd"),
            parent_window: str_opt("parent").or_else(|| str_opt("parent_window")),
            env,
            unset_env: str_vec("unset_env"),
            desktop,
            trust: bool_val("trust"),
            focus: bool_val("focus"),
            close_on_exit: bool_val("close_on_exit"),
            json: bool_val("json"),
            permission_mode: str_opt("permission_mode"),
            approval: str_opt("approval").or_else(|| str_opt("ask_for_approval")),
            sandbox,
            mode: str_opt("mode"),
            dangerously_skip_permissions: bool_val("dangerously_skip_permissions"),
            allow_dangerously_skip_permissions: bool_val("allow_dangerously_skip_permissions"),
            full_auto: bool_val("full_auto"),
            approve_for_me: bool_val("approve_for_me"),
            allowed_tools: str_vec("allowed_tools"),
            disallowed_tools: str_vec("disallowed_tools"),
            add_dirs: str_vec("add_dir").into_iter().chain(str_vec("add_dirs")).collect(),
            worktree,
            agent: str_opt("agent"),
            name: str_opt("name"),
            profile: str_opt("profile"),
            configs: str_vec("config").into_iter().chain(str_vec("configs")).collect(),
            settings: str_opt("settings"),
            strict_mcp_config: bool_val("strict_mcp_config"),
            system_prompt: str_opt("system_prompt"),
            append_system_prompt: str_opt("append_system_prompt"),
            mcp_configs: str_vec("mcp_config").into_iter().chain(str_vec("mcp_configs")).collect(),
            project: str_opt("project"),
            new_project: bool_val("new_project"),
            remote_agent: bool_val("remote_agent"),
            remote_control: bool_val("remote_control"),
            search: bool_val("search"),
            enables: str_vec("enable"),
            disables: str_vec("disable"),
            images: str_vec("image"),
            no_alt_screen: bool_val("no_alt_screen"),
            no_daemon: bool_val("no_daemon"),
            print_mode: bool_val("print") || bool_val("stream"),
            input_format: str_opt("input_format"),
            output_format: str_opt("output_format"),
            extra: str_vec("extra"),
        })
    }

    pub fn parse_cli_args(args: &[String]) -> Result<Self, String> {
        let mut s = Self::default();
        let mut i = 0;
        let next = |i: &mut usize, flag: &str, inline: Option<&str>| -> Result<String, String> {
            if let Some(v) = inline {
                return Ok(v.to_owned());
            }
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{flag} requires a value"))
        };
        while i < args.len() {
            let arg = args[i].as_str();
            if arg == "--" {
                s.extra.extend(args[i + 1..].iter().cloned());
                break;
            }
            if arg == "-h" || arg == "--help" {
                return Err(start_usage());
            }
            let (flag, inline) = if arg.starts_with("--") {
                match arg.split_once('=') {
                    Some((k, v)) => (k, Some(v)),
                    None => (arg, None),
                }
            } else {
                (arg, None)
            };
            match flag {
                "--cli" => {
                    let v = next(&mut i, flag, inline)?;
                    s.cli = Some(AgentCli::parse(&v).ok_or_else(|| format!("unsupported agent CLI `{v}`; expected `claude`, `codex`, or `agy`"))?);
                }
                "-m" | "--model" => s.model = Some(next(&mut i, flag, inline)?),
                "--effort" | "--reasoning-effort" => s.effort = Some(next(&mut i, flag, inline)?),
                "-r" | "--resume" | "--conversation" => s.resume = Some(next(&mut i, flag, inline)?),
                "--session-id" => s.session_id = Some(next(&mut i, flag, inline)?),
                "--continue" | "--last" => s.continue_last = true,
                "--fork" | "--fork-session" => s.fork = true,
                "-p" | "--prompt" => s.prompt = Some(next(&mut i, flag, inline)?),
                "--print" => s.print_mode = true,
                "--stream" => {
                    s.print_mode = true;
                    s.input_format = Some("stream-json".into());
                    s.output_format = Some("stream-json".into());
                }
                "--input-format" => s.input_format = Some(next(&mut i, flag, inline)?),
                "--output-format" => s.output_format = Some(next(&mut i, flag, inline)?),
                "--title" => s.title = Some(next(&mut i, flag, inline)?),
                "--role" => s.role = Some(next(&mut i, flag, inline)?),
                "-C" | "--cwd" | "--cd" => s.cwd = Some(next(&mut i, flag, inline)?),
                "--parent" => s.parent_window = Some(next(&mut i, flag, inline)?),
                "--trust" => s.trust = true,
                "--desktop" => {
                    let v = next(&mut i, flag, inline)?;
                    s.desktop = DesktopPolicy::parse(&v)?;
                }
                "--unset-env" => {
                    let k = next(&mut i, flag, inline)?;
                    if !k.trim().is_empty() {
                        s.unset_env.push(k.trim().to_owned());
                    }
                }
                "-e" | "--env" => {
                    let kv = next(&mut i, flag, inline)?;
                    let Some((k, v)) = kv.split_once('=') else {
                        return Err(format!("--env expects KEY=VALUE, got `{kv}`"));
                    };
                    s.env.push((k.to_owned(), v.to_owned()));
                }
                "--focus" => s.focus = true,
                "--close-on-exit" => s.close_on_exit = true,
                "--json" => s.json = true,
                "--permission-mode" => s.permission_mode = Some(next(&mut i, flag, inline)?),
                "-a" | "--approval" | "--ask-for-approval" => s.approval = Some(next(&mut i, flag, inline)?),
                "-s" | "--sandbox" => {
                    if let Some(v) = inline {
                        s.sandbox = Some(v.to_owned());
                    } else if args.get(i + 1).is_some_and(|n| !n.starts_with('-')) && s.cli != Some(AgentCli::Agy) {
                        i += 1;
                        s.sandbox = Some(args[i].clone());
                    } else {
                        s.sandbox = Some("true".to_owned());
                    }
                }
                "--mode" => s.mode = Some(next(&mut i, flag, inline)?),
                "--dangerously-skip-permissions" | "--dangerously-bypass-approvals-and-sandbox" | "--yolo" => s.dangerously_skip_permissions = true,
                "--allow-dangerously-skip-permissions" => s.allow_dangerously_skip_permissions = true,
                "--full-auto" => s.full_auto = true,
                "--approve-for-me" => s.approve_for_me = true,
                "--allowed-tools" | "--allowedTools" => {
                    let first = next(&mut i, flag, inline)?;
                    s.allowed_tools.push(first);
                    if inline.is_none() {
                        while let Some(peek) = args.get(i + 1)
                            && looks_like_tool_token(peek)
                        {
                            i += 1;
                            s.allowed_tools.push(args[i].clone());
                        }
                    }
                }
                "--disallowed-tools" | "--disallowedTools" => {
                    let first = next(&mut i, flag, inline)?;
                    s.disallowed_tools.push(first);
                    if inline.is_none() {
                        while let Some(peek) = args.get(i + 1)
                            && looks_like_tool_token(peek)
                        {
                            i += 1;
                            s.disallowed_tools.push(args[i].clone());
                        }
                    }
                }
                "--add-dir" => s.add_dirs.push(next(&mut i, flag, inline)?),
                "-w" | "--worktree" => {
                    if let Some(v) = inline {
                        s.worktree = Some(Some(v.to_owned()));
                    } else if args.get(i + 1).is_some_and(|n| !n.starts_with('-')) {
                        i += 1;
                        s.worktree = Some(Some(args[i].clone()));
                    } else {
                        s.worktree = Some(None);
                    }
                }
                "--agent" => s.agent = Some(next(&mut i, flag, inline)?),
                "-n" | "--name" => s.name = Some(next(&mut i, flag, inline)?),
                "--profile" => s.profile = Some(next(&mut i, flag, inline)?),
                "-c" | "--config" => s.configs.push(next(&mut i, flag, inline)?),
                "--settings" => s.settings = Some(next(&mut i, flag, inline)?),
                "--strict-mcp-config" => s.strict_mcp_config = true,
                "--system-prompt" => s.system_prompt = Some(next(&mut i, flag, inline)?),
                "--append-system-prompt" => s.append_system_prompt = Some(next(&mut i, flag, inline)?),
                "--mcp-config" => s.mcp_configs.push(next(&mut i, flag, inline)?),
                "--project" => s.project = Some(next(&mut i, flag, inline)?),
                "--new-project" => s.new_project = true,
                "--remote-agent" => s.remote_agent = true,
                "--remote-control" => s.remote_control = true,
                "--search" => s.search = true,
                "--enable" => s.enables.push(next(&mut i, flag, inline)?),
                "--disable" => s.disables.push(next(&mut i, flag, inline)?),
                "-i" | "--image" => s.images.push(next(&mut i, flag, inline)?),
                "--no-alt-screen" => s.no_alt_screen = true,
                "--no-daemon" => s.no_daemon = true,
                other if other.starts_with('-') => {
                    return Err(format!("unknown option `{other}`; pass CLI-specific flags after `--` (e.g. `-- {other}`)"));
                }
                positional => {
                    if s.cli.is_none()
                        && let Some(parsed) = AgentCli::parse(positional)
                    {
                        s.cli = Some(parsed);
                    } else if s.prompt.is_none() {
                        s.prompt = Some(positional.to_owned());
                    } else {
                        return Err(format!("unexpected argument `{positional}`"));
                    }
                }
            }
            i += 1;
        }
        Ok(s)
    }

    /// Environment variable names to unset inside the child process before exec.
    pub fn env_vars_to_unset(&self) -> Vec<String> {
        let mut vars: Vec<String> = Vec::new();
        let mut push_unique = |k: &str| {
            if !vars.iter().any(|v| v == k) {
                vars.push(k.to_owned());
            }
        };
        match self.desktop {
            DesktopPolicy::None => {
                for k in ["DISPLAY", "WAYLAND_DISPLAY", "OLO_SOCKET", "OLO_SESSION", "REX_SESSION"] {
                    push_unique(k);
                }
            }
            DesktopPolicy::Screens { .. } => {
                // Keep REX_SESSION only so `olo screen` can find the session; server enforces screen allowlist!
                for k in ["DISPLAY", "WAYLAND_DISPLAY", "OLO_SOCKET", "OLO_SESSION"] {
                    push_unique(k);
                }
            }
            DesktopPolicy::Inherit => {}
        }
        for k in &self.unset_env {
            push_unique(k);
        }
        vars
    }

    /// Validates mandatory fields (`cli`, `model`, `effort`) and builds the exact CLI argv.
    pub fn build_command(&self) -> Result<(AgentCli, String, String, Vec<String>), String> {
        let cli = self.cli.ok_or_else(|| "missing agent CLI: specify `claude`, `codex`, or `agy` (e.g. `olo agent start claude --model ... --effort ...`)".to_owned())?;
        let model = self
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("--model <MODEL> is required when launching `{}` (model must always be set explicitly so there are no surprises)", cli.binary()))?
            .to_owned();
        let effort = self
            .effort
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("--effort <EFFORT> is required when launching `{}` (reasoning effort must always be set explicitly so there are no surprises)", cli.binary()))?
            .to_owned();

        if self.approve_for_me && cli != AgentCli::Codex {
            return Err(format!("`--approve-for-me` is a Codex-only flag (routes approvals through Codex's automatic review) and cannot be passed to `{}`", cli.binary()));
        }

        let mut argv: Vec<String> = Vec::new();
        match cli {
            AgentCli::Claude => {
                argv.push("claude".into());
                argv.extend(["--model".into(), model.clone(), "--effort".into(), effort.clone()]);
                if let Some(ref sid) = self.session_id {
                    argv.extend(["--session-id".into(), sid.clone()]);
                }
                if let Some(ref rid) = self.resume {
                    argv.extend(["--resume".into(), rid.clone()]);
                } else if self.continue_last {
                    argv.push("--continue".into());
                }
                if self.fork {
                    argv.push("--fork-session".into());
                }
                if self.print_mode {
                    argv.push("--print".into());
                }
                if let Some(ref inf) = self.input_format {
                    argv.extend(["--input-format".into(), inf.clone()]);
                }
                if let Some(ref outf) = self.output_format {
                    argv.extend(["--output-format".into(), outf.clone()]);
                }
                if let Some(ref pm) = self.permission_mode.as_ref().or(self.mode.as_ref()) {
                    argv.extend(["--permission-mode".into(), (*pm).clone()]);
                }
                if self.dangerously_skip_permissions {
                    argv.push("--dangerously-skip-permissions".into());
                }
                if self.allow_dangerously_skip_permissions {
                    argv.push("--allow-dangerously-skip-permissions".into());
                }
                if !self.allowed_tools.is_empty() {
                    argv.push("--allowed-tools".into());
                    argv.extend(self.allowed_tools.iter().cloned());
                }
                if !self.disallowed_tools.is_empty() {
                    argv.push("--disallowed-tools".into());
                    argv.extend(self.disallowed_tools.iter().cloned());
                }
                for d in &self.add_dirs {
                    argv.extend(["--add-dir".into(), d.clone()]);
                }
                if let Some(ref wt) = self.worktree {
                    argv.push("--worktree".into());
                    if let Some(name) = wt {
                        argv.push(name.clone());
                    }
                }
                if let Some(ref a) = self.agent {
                    argv.extend(["--agent".into(), a.clone()]);
                }
                if let Some(ref n) = self.name.as_ref().or(self.title.as_ref()) {
                    argv.extend(["--name".into(), (*n).clone()]);
                }
                if let Some(ref st) = self.settings {
                    argv.extend(["--settings".into(), st.clone()]);
                }
                if self.strict_mcp_config {
                    argv.push("--strict-mcp-config".into());
                }
                if let Some(ref sp) = self.system_prompt {
                    argv.extend(["--system-prompt".into(), sp.clone()]);
                }
                if let Some(ref asp) = self.append_system_prompt {
                    argv.extend(["--append-system-prompt".into(), asp.clone()]);
                }
                for m in &self.mcp_configs {
                    argv.extend(["--mcp-config".into(), m.clone()]);
                }
                argv.extend(self.extra.iter().cloned());
                if let Some(ref p) = self.prompt {
                    argv.push(p.clone());
                }
            }
            AgentCli::Codex => {
                argv.push("codex".into());
                if self.fork {
                    argv.push("fork".into());
                } else if self.resume.is_some() || self.continue_last {
                    argv.push("resume".into());
                }
                argv.extend(["-m".into(), model.clone(), "-c".into(), format!("model_reasoning_effort=\"{}\"", effort.replace('"', "\\\""))]);
                if let Some(ref a) = self.approval.as_ref().or(self.permission_mode.as_ref()) {
                    argv.extend(["-a".into(), (*a).clone()]);
                } else if self.full_auto {
                    argv.extend(["-a".into(), "on-request".into()]);
                }
                if let Some(ref sb) = self.sandbox {
                    argv.extend(["-s".into(), sb.clone()]);
                } else if self.full_auto {
                    argv.extend(["-s".into(), "workspace-write".into()]);
                }
                if self.approve_for_me {
                    argv.push("--approve-for-me".into());
                }
                if self.dangerously_skip_permissions {
                    argv.push("--dangerously-bypass-approvals-and-sandbox".into());
                }
                for d in &self.add_dirs {
                    argv.extend(["--add-dir".into(), d.clone()]);
                }
                if self.worktree.is_some() {
                    argv.push("--worktree".into());
                }
                if let Some(ref p) = self.profile {
                    argv.extend(["-p".into(), p.clone()]);
                }
                for c in &self.configs {
                    argv.extend(["-c".into(), c.clone()]);
                }
                if self.search {
                    argv.push("--search".into());
                }
                for e in &self.enables {
                    argv.extend(["--enable".into(), e.clone()]);
                }
                for d in &self.disables {
                    argv.extend(["--disable".into(), d.clone()]);
                }
                for img in &self.images {
                    argv.extend(["-i".into(), img.clone()]);
                }
                if self.no_alt_screen {
                    argv.push("--no-alt-screen".into());
                }
                if self.no_daemon {
                    argv.push("--no-daemon".into());
                }
                argv.extend(self.extra.iter().cloned());
                if self.continue_last && self.resume.is_none() {
                    argv.push("--last".into());
                }
                if let Some(ref rid) = self.resume {
                    argv.push(rid.clone());
                }
                if let Some(ref p) = self.prompt {
                    argv.push(p.clone());
                }
            }
            AgentCli::Agy => {
                argv.push("agy".into());
                argv.extend(["--model".into(), model.clone(), "--effort".into(), effort.clone()]);
                if let Some(ref rid) = self.resume {
                    argv.extend(["--resume".into(), rid.clone()]);
                } else if self.continue_last {
                    argv.push("--continue".into());
                }
                if self.print_mode {
                    argv.push("--print".into());
                }
                if let Some(ref inf) = self.input_format {
                    argv.extend(["--input-format".into(), inf.clone()]);
                }
                if let Some(ref outf) = self.output_format {
                    argv.extend(["--output-format".into(), outf.clone()]);
                }
                if let Some(ref m) = self.mode.as_ref().or(self.permission_mode.as_ref()) {
                    argv.extend(["--mode".into(), (*m).clone()]);
                }
                if self.sandbox.as_deref().is_some_and(|v| !matches!(v.to_ascii_lowercase().as_str(), "false" | "0" | "off" | "none")) {
                    argv.push("--sandbox".into());
                }
                if self.dangerously_skip_permissions {
                    argv.push("--dangerously-skip-permissions".into());
                }
                for d in &self.add_dirs {
                    argv.extend(["--add-dir".into(), d.clone()]);
                }
                if let Some(ref a) = self.agent {
                    argv.extend(["--agent".into(), a.clone()]);
                }
                if let Some(ref pr) = self.project {
                    argv.extend(["--project".into(), pr.clone()]);
                }
                if self.new_project {
                    argv.push("--new-project".into());
                }
                if let Some(ref st) = self.settings {
                    argv.extend(["--settings".into(), st.clone()]);
                }
                if self.remote_agent {
                    argv.push("--remote-agent".into());
                }
                if self.remote_control {
                    argv.push("--remote-control".into());
                }
                argv.extend(self.extra.iter().cloned());
                if let Some(ref p) = self.prompt {
                    if self.print_mode {
                        argv.push(p.clone());
                    } else {
                        argv.extend(["-i".into(), p.clone()]);
                    }
                }
            }
        }
        Ok((cli, model, effort, argv))
    }

    /// Wraps `cli_argv` with `env -u K1 -u K2 ...` when environment variables must be
    /// stripped so desktop isolation works identically on every Rex server version.
    pub fn wrap_with_env_isolation(&self, cli_argv: Vec<String>) -> Vec<String> {
        let unset = self.env_vars_to_unset();
        if unset.is_empty() {
            return cli_argv;
        }
        let mut out = Vec::with_capacity(1 + unset.len() * 2 + cli_argv.len());
        out.push("env".into());
        for k in unset {
            out.push("-u".into());
            out.push(k);
        }
        out.extend(cli_argv);
        out
    }

    pub fn effective_title(&self, cli: AgentCli) -> String {
        if let Some(t) = self.title.as_deref().filter(|s| !s.trim().is_empty()) {
            t.trim().to_owned()
        } else if let Some(n) = self.name.as_deref().filter(|s| !s.trim().is_empty()) {
            n.trim().to_owned()
        } else if let Some(r) = self.role.as_deref().filter(|s| !s.trim().is_empty()) {
            r.trim().to_owned()
        } else {
            cli.display_name().to_owned()
        }
    }
}

/// Parsed send payload: separates pasted body from trailing Enter so TUIs never swallow `\r` into the paste buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendPayload {
    pub target: String,
    pub body: String,
    pub paste: bool,
    pub enter: bool,
    pub post_keys: String,
    pub as_json: bool,
    pub verify_delivery: bool,
}

/// Encodes text, `--paste`, `--file` (pre-read into `--text` + `--paste`), `--ctrl <KEY>`, `--key <NAME>`, and `--enter`.
pub fn parse_send_payload(args: &[String], raw_obj: Option<&Value>) -> Result<SendPayload, String> {
    let mut target: Option<String> = None;
    let mut paste = false;
    let mut add_enter = false;
    let mut as_json = false;
    let mut verify_delivery = false;
    let mut text_parts: Vec<String> = Vec::new();
    let mut post_keys: Vec<String> = Vec::new();

    if let Some(v) = raw_obj
        && v.get("args").is_none()
    {
        target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
        paste = v.get("paste").and_then(Value::as_bool).unwrap_or(false);
        add_enter = v.get("enter").and_then(Value::as_bool).unwrap_or(false);
        as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
        verify_delivery = v.get("verify").and_then(Value::as_bool).unwrap_or(paste && add_enter);
        if let Some(t) = v.get("text").and_then(Value::as_str) {
            text_parts.push(t.to_owned());
        }
        if let Some(c) = v.get("ctrl").and_then(Value::as_str) {
            post_keys.push(ctrl_byte(c)?);
        }
        if let Some(k) = v.get("key").and_then(Value::as_str) {
            post_keys.push(special_key(k)?);
        }
        if let Some(ks) = v.get("keys").and_then(Value::as_array) {
            for k in ks {
                if let Some(ks_str) = k.as_str() {
                    post_keys.push(special_key(ks_str)?);
                }
            }
        }
    } else {
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            match a {
                "--enter" | "--submit" => add_enter = true,
                "--paste" => paste = true,
                "--verify" => verify_delivery = true,
                "--json" => as_json = true,
                "--ctrl" => {
                    i += 1;
                    let c = args.get(i).ok_or("--ctrl requires a key (e.g. c, d, z)")?;
                    post_keys.push(ctrl_byte(c)?);
                }
                "--key" => {
                    i += 1;
                    let k = args.get(i).ok_or("--key requires a key name (e.g. Enter, Escape, Tab, Up, Down)")?;
                    post_keys.push(special_key(k)?);
                }
                "--text" => {
                    i += 1;
                    let t = args.get(i).ok_or("--text requires text")?;
                    text_parts.push(t.clone());
                }
                "--" => {
                    text_parts.extend(args[i + 1..].iter().cloned());
                    break;
                }
                s if s.starts_with("--ctrl=") => post_keys.push(ctrl_byte(&s["--ctrl=".len()..])?),
                s if s.starts_with("--key=") => post_keys.push(special_key(&s["--key=".len()..])?),
                s if s.starts_with("--text=") => text_parts.push(s["--text=".len()..].to_owned()),
                s if s.starts_with('-') => return Err(format!("unknown send option `{s}`")),
                s => {
                    if target.is_none() {
                        target = Some(s.to_owned());
                    } else {
                        text_parts.push(s.to_owned());
                    }
                }
            }
            i += 1;
        }
    }

    let target = target.ok_or_else(|| "usage: olo agent send <ID|NAME> [TEXT | --file PATH] [--enter] [--paste] [--ctrl c|d|z] [--key Enter|Escape|Tab|Up|Down|Left|Right]".to_owned())?;
    let body = text_parts.join(" ");
    let post = post_keys.join("");
    if body.is_empty() && !add_enter && post.is_empty() {
        return Err("nothing to send: provide TEXT, --file PATH, --enter, --ctrl <KEY>, or --key <NAME>".into());
    }
    if body.contains('\n') {
        paste = true;
    }
    if paste && add_enter {
        verify_delivery = true;
    }
    Ok(SendPayload { target, body, paste, enter: add_enter, post_keys: post, as_json, verify_delivery })
}

/// Parsed options for `olo agent approve <ID|NAME> --once | --no [--reason TEXT] [--json]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApproveSpec {
    pub target: String,
    /// `true` = approve once (`ChoiceKind::Once`), `false` = reject (`ChoiceKind::No`).
    pub once: bool,
    pub reason: Option<String>,
    pub json: bool,
}

impl ApproveSpec {
    pub fn parse(raw: Option<&Value>) -> Result<Self, String> {
        let Some(v) = raw else {
            return Err("usage: olo agent approve <ID|NAME> --once | --no [--reason TEXT] [--json]".into());
        };
        let mut target: Option<String> = None;
        let mut once = false;
        let mut no = false;
        let mut reason: Option<String> = None;
        let mut json = false;
        if let Some(arr) = v.get("args").and_then(Value::as_array) {
            let strs: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
            let mut i = 0;
            while i < strs.len() {
                match strs[i].as_str() {
                    "--once" | "--yes" => once = true,
                    "--no" | "--deny" | "--cancel" => no = true,
                    "--json" => json = true,
                    "--reason" => {
                        i += 1;
                        reason = Some(strs.get(i).ok_or("--reason requires text")?.clone());
                    }
                    s if s.starts_with("--reason=") => reason = Some(s["--reason=".len()..].to_owned()),
                    s if s.starts_with('-') => return Err(format!("unknown approve option `{s}` (use `--once` or `--no`)")),
                    s if target.is_none() => target = Some(s.to_owned()),
                    s => return Err(format!("unexpected argument `{s}`")),
                }
                i += 1;
            }
        } else {
            target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
            once = v.get("once").and_then(Value::as_bool).unwrap_or(false);
            no = v.get("no").and_then(Value::as_bool).unwrap_or(false);
            reason = v.get("reason").and_then(Value::as_str).map(str::to_owned);
            json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
        }
        let target = target.ok_or_else(|| "usage: olo agent approve <ID|NAME> --once | --no [--reason TEXT] [--json]".to_owned())?;
        if once == no {
            return Err("specify exactly one of `--once` or `--no` (never persistent/mode-switching choices)".into());
        }
        Ok(Self { target, once, reason, json })
    }
}

pub fn ctrl_byte(raw: &str) -> Result<String, String> {
    let s = raw.trim().to_ascii_lowercase();
    let s = s.strip_prefix("ctrl-").or_else(|| s.strip_prefix("ctrl+")).or_else(|| s.strip_prefix('^')).unwrap_or(&s);
    let mut chars = s.chars();
    let Some(ch) = chars.next() else {
        return Err("empty --ctrl key".into());
    };
    if chars.next().is_some() {
        return Err(format!("--ctrl expects a single character, got `{raw}`"));
    }
    let byte = match ch {
        '@' | ' ' => 0x00,
        'a'..='z' => (ch as u8) - b'a' + 1,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' => 0x1f,
        '?' => 0x7f,
        _ => return Err(format!("unsupported control key `{raw}`")),
    };
    Ok((byte as char).to_string())
}

pub fn special_key(raw: &str) -> Result<String, String> {
    let k = raw.trim().to_ascii_lowercase();
    if k.starts_with("ctrl-") || k.starts_with("ctrl+") || k.starts_with('^') {
        return ctrl_byte(&k);
    }
    Ok(match k.as_str() {
        "enter" | "return" | "cr" => "\r".into(),
        "newline" | "lf" => "\n".into(),
        "esc" | "escape" => "\x1b".into(),
        "tab" => "\t".into(),
        "shift-tab" | "btab" | "backtab" => "\x1b[Z".into(),
        "space" => " ".into(),
        "backspace" | "bs" => "\x7f".into(),
        "delete" | "del" => "\x1b[3~".into(),
        "up" => "\x1b[A".into(),
        "down" => "\x1b[B".into(),
        "right" => "\x1b[C".into(),
        "left" => "\x1b[D".into(),
        "home" => "\x1b[H".into(),
        "end" => "\x1b[F".into(),
        "pageup" | "pgup" => "\x1b[5~".into(),
        "pagedown" | "pgdn" => "\x1b[6~".into(),
        _ => return Err(format!("unknown special key `{raw}` (try Enter, Escape, Tab, Shift-Tab, Space, Backspace, Delete, Up, Down, Left, Right, Home, End, PageUp, PageDown, or Ctrl-C)")),
    })
}

/// Structured state of a native child-tab subagent derived from live screen + transcript + Rex activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentLiveState {
    Working,
    WaitingQuestion,
    WaitingApproval,
    Done,
    Error,
    Idle,
    Exited,
}

impl SubagentLiveState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::WaitingQuestion => "waiting_question",
            Self::WaitingApproval => "waiting_approval",
            Self::Done => "done",
            Self::Error => "error",
            Self::Idle => "idle",
            Self::Exited => "exited",
        }
    }
}

/// Rich screen classification without false positives from bare `"esc to cancel"` or empty repaint tails.
#[derive(Clone, Debug)]
pub struct ScreenClassification {
    pub state: SubagentLiveState,
    pub summary: Option<String>,
    pub approval: Option<olo_agent_screen::ApprovalPrompt>,
    pub folder_trust: Option<olo_agent_screen::FolderTrustPrompt>,
    pub api_error: Option<String>,
    pub usage: Option<olo_agent_screen::UsageInfo>,
}

/// Evaluates a child tab's current screen & activity against `olo_agent_screen`.
/// Never emits `WaitingApproval` unless a genuine structured approval prompt with choices is present.
pub fn classify_screen_rich(cli: AgentCli, activity: crate::agent::AgentActivity, screen_text: &str, alive: bool) -> ScreenClassification {
    if !alive {
        return ScreenClassification { state: SubagentLiveState::Exited, summary: Some("tab closed".into()), approval: None, folder_trust: None, api_error: None, usage: None };
    }
    let agent = cli.screen_agent();
    let usage = olo_agent_screen::parse_usage(screen_text);
    let last_nonempty = || screen_text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned);

    // 1. Folder-trust prompt (never misclassified as tool approval).
    if let Some(trust) = olo_agent_screen::parse_folder_trust(screen_text) {
        let sum = match &trust.folder {
            Some(f) => format!("Trust folder {f}?"),
            None => "Trust this folder?".into(),
        };
        return ScreenClassification { state: SubagentLiveState::WaitingQuestion, summary: Some(sum), approval: None, folder_trust: Some(trust), api_error: None, usage };
    }

    // 2. Structured tool approval prompt (requires real choices + live cursor mark).
    if let Some(ap) = olo_agent_screen::parse_approval(agent, screen_text) {
        let sum = ap.command.clone().or_else(|| Some(ap.question.clone()));
        return ScreenClassification { state: SubagentLiveState::WaitingApproval, summary: sum, approval: Some(ap), folder_trust: None, api_error: None, usage };
    }

    // 3. Genuine non-approval interactive question / chooser.
    if let Some(q) = olo_agent_screen::parse_question(agent, screen_text) {
        return ScreenClassification { state: SubagentLiveState::WaitingQuestion, summary: Some(q), approval: None, folder_trust: None, api_error: None, usage };
    }

    // 4. API error that cut a turn short without a normal Stop hook.
    if let Some(err) = olo_agent_screen::parse_api_error(agent, screen_text) {
        return ScreenClassification { state: SubagentLiveState::Error, summary: Some(err.clone()), approval: None, folder_trust: None, api_error: Some(err), usage };
    }

    // 5. Working on screen takes precedence over stale Idle/Done.
    if olo_agent_screen::classify(agent, screen_text) == olo_agent_screen::State::Working {
        return ScreenClassification { state: SubagentLiveState::Working, summary: last_nonempty(), approval: None, folder_trust: None, api_error: None, usage };
    }

    // 6. Fallback to Rex activity — NEVER turn bare `AgentActivity::Blocked` into a false `WaitingApproval`
    //    when no structured approval/question exists on screen!
    let (state, summary) = match activity {
        crate::agent::AgentActivity::Working | crate::agent::AgentActivity::Blocked => (SubagentLiveState::Working, last_nonempty()),
        crate::agent::AgentActivity::Error => (SubagentLiveState::Error, last_nonempty()),
        crate::agent::AgentActivity::Done => (SubagentLiveState::Done, last_nonempty()),
        crate::agent::AgentActivity::Idle => {
            if screen_text.trim().is_empty() {
                (SubagentLiveState::Working, Some("starting".into()))
            } else {
                let summary = if olo_agent_screen::at_prompt(agent, screen_text) { Some("at prompt".into()) } else { last_nonempty() };
                (SubagentLiveState::Idle, summary)
            }
        }
    };
    ScreenClassification { state, summary, approval: None, folder_trust: None, api_error: None, usage }
}

pub fn approval_to_json(ap: &olo_agent_screen::ApprovalPrompt, transcript_tool: Option<&Value>) -> Value {
    let tx_tool = transcript_tool.and_then(|t| t.get("tool")).and_then(Value::as_str);
    let tx_cmd = transcript_tool.and_then(|t| t.get("command")).and_then(Value::as_str);
    let tx_args = transcript_tool.and_then(|t| t.get("arguments")).cloned();
    // Prefer the untruncated transcript command whenever the screen folded lines or transcript has a longer command.
    let command = match (ap.command.as_deref(), tx_cmd) {
        (Some(_), Some(full)) if ap.folded_lines.is_some() || full.len() > ap.command.as_deref().map_or(0, str::len) => Some(full.to_owned()),
        (Some(s), _) => Some(s.to_owned()),
        (None, Some(full)) => Some(full.to_owned()),
        (None, None) => None,
    };
    let tool = tx_tool.map(str::to_owned).or_else(|| ap.tool.clone());
    let choices: Vec<Value> = ap
        .choices
        .iter()
        .map(|c| {
            json!({
                "index": c.index,
                "number": c.number,
                "label": c.label,
                "selected": c.selected,
                "kind": c.kind.as_str(),
            })
        })
        .collect();
    json!({
        "question": ap.question,
        "tool": tool,
        "command": command,
        "arguments": tx_args,
        "folded_lines": ap.folded_lines,
        "selected_index": ap.selected_index,
        "choices": choices,
    })
}

/// Parsed options for `olo agent watch [<ID|NAME>... | --all] [--since CURSOR] ...`.
#[derive(Clone, Debug)]
pub struct WatchSpec {
    pub targets: Vec<String>,
    pub all: bool,
    pub parent: Option<String>,
    pub since: Option<u64>,
    pub events: Vec<String>,
    pub match_text: Option<String>,
    pub regex_pat: Option<String>,
    pub timeout_secs: u64,
    pub json: bool,
}

impl WatchSpec {
    pub fn parse(raw: Option<&Value>) -> Result<Self, String> {
        let Some(v) = raw else {
            return Err(
                "usage: olo agent watch [<ID|NAME>... | --all] [--since CURSOR] [--parent WIN] [--on question|approval|idle|done|error|exit|any] [--match TEXT] [--regex PAT] [--timeout SEC] [--json]"
                    .into(),
            );
        };
        let mut targets: Vec<String> = Vec::new();
        let mut all = false;
        let mut parent: Option<String> = None;
        let mut since: Option<u64> = None;
        let mut events: Vec<String> = Vec::new();
        let mut match_text: Option<String> = None;
        let mut regex_pat: Option<String> = None;
        let mut timeout_secs: u64 = 600;
        let mut json = false;

        if let Some(arr) = v.get("args").and_then(Value::as_array) {
            let args: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
            let mut i = 0;
            while i < args.len() {
                let a = args[i].as_str();
                match a {
                    "--all" | "-a" => all = true,
                    "--parent" => {
                        i += 1;
                        parent = Some(args.get(i).ok_or("--parent requires a window or block ID")?.clone());
                    }
                    "--since" => {
                        i += 1;
                        let c = args.get(i).ok_or("--since requires a numeric cursor")?;
                        since = Some(c.parse::<u64>().map_err(|_| format!("invalid --since cursor `{c}`"))?);
                    }
                    "--on" | "--event" => {
                        i += 1;
                        let ev = args.get(i).ok_or("--on requires an event name")?;
                        events.extend(ev.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()));
                    }
                    "--match" | "--contains" => {
                        i += 1;
                        match_text = Some(args.get(i).ok_or("--match requires text")?.clone());
                    }
                    "--regex" => {
                        i += 1;
                        regex_pat = Some(args.get(i).ok_or("--regex requires a pattern")?.clone());
                    }
                    "--timeout" => {
                        i += 1;
                        let t = args.get(i).ok_or("--timeout requires seconds")?;
                        timeout_secs = t.trim_end_matches('s').parse::<u64>().map_err(|_| format!("invalid --timeout `{t}`"))?;
                    }
                    "--json" => json = true,
                    s if s.starts_with("--parent=") => parent = Some(s["--parent=".len()..].to_owned()),
                    s if s.starts_with("--since=") => {
                        let c = &s["--since=".len()..];
                        since = Some(c.parse::<u64>().map_err(|_| format!("invalid --since cursor `{c}`"))?);
                    }
                    s if s.starts_with("--on=") => events.extend(s["--on=".len()..].split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty())),
                    s if s.starts_with("--match=") => match_text = Some(s["--match=".len()..].to_owned()),
                    s if s.starts_with("--regex=") => regex_pat = Some(s["--regex=".len()..].to_owned()),
                    s if s.starts_with("--timeout=") => {
                        let t = &s["--timeout=".len()..];
                        timeout_secs = t.trim_end_matches('s').parse::<u64>().map_err(|_| format!("invalid --timeout `{t}`"))?;
                    }
                    s if s.starts_with('-') => return Err(format!("unknown watch option `{s}`")),
                    s => targets.push(s.to_owned()),
                }
                i += 1;
            }
        } else {
            if let Some(t) = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str) {
                targets.push(t.to_owned());
            }
            if let Some(ts) = v.get("targets").and_then(Value::as_array) {
                for x in ts {
                    if let Some(s) = x.as_str() {
                        targets.push(s.to_owned());
                    }
                }
            }
            all = v.get("all").and_then(Value::as_bool).unwrap_or(false);
            parent = v.get("parent").and_then(Value::as_str).map(str::to_owned);
            since = v.get("since").and_then(Value::as_u64);
            if let Some(ev) = v.get("on").and_then(Value::as_str) {
                events.extend(ev.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()));
            } else if let Some(arr) = v.get("on").and_then(Value::as_array) {
                for x in arr {
                    if let Some(s) = x.as_str() {
                        events.push(s.trim().to_ascii_lowercase());
                    }
                }
            }
            match_text = v.get("match").and_then(Value::as_str).map(str::to_owned);
            regex_pat = v.get("regex").and_then(Value::as_str).map(str::to_owned);
            timeout_secs = v.get("timeout").and_then(Value::as_u64).unwrap_or(600);
            json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
        }

        if !all && targets.is_empty() {
            return Err("usage: olo agent watch [<ID|NAME>... | --all] [--since CURSOR] [--on question|approval|idle|done|error|exit|any] [--match TEXT] [--regex PAT] [--timeout SEC]".to_owned());
        }
        if events.is_empty() && match_text.is_none() && regex_pat.is_none() {
            events = vec!["question".into(), "approval".into(), "idle".into(), "done".into(), "error".into(), "exit".into()];
        }
        Ok(Self { targets, all, parent, since, events, match_text, regex_pat, timeout_secs: timeout_secs.clamp(1, 3600), json })
    }

    pub fn matches_state(&self, state: SubagentLiveState) -> bool {
        self.events.iter().any(|ev| match ev.as_str() {
            "any" => !matches!(state, SubagentLiveState::Working),
            "question" | "waiting_question" => state == SubagentLiveState::WaitingQuestion,
            "approval" | "waiting_approval" => state == SubagentLiveState::WaitingApproval,
            "waiting" => matches!(state, SubagentLiveState::WaitingQuestion | SubagentLiveState::WaitingApproval),
            "idle" => state == SubagentLiveState::Idle,
            "done" => state == SubagentLiveState::Done,
            "error" | "failed" => state == SubagentLiveState::Error,
            "exit" | "exited" | "closed" => state == SubagentLiveState::Exited,
            _ => false,
        })
    }

    /// Checks whether the current state/screen satisfies this watch predicate.
    pub fn check(&self, state: SubagentLiveState, screen: &str, compiled_re: Option<&regex::Regex>) -> Option<(String, Option<String>)> {
        if let Some(needle) = &self.match_text
            && !needle.is_empty()
            && let Some(line) = screen.lines().rev().find(|l| l.contains(needle.as_str()))
        {
            return Some(("match".into(), Some(line.trim().to_owned())));
        }
        if let Some(re) = compiled_re
            && let Some(line) = screen.lines().rev().find(|l| re.is_match(l))
        {
            return Some(("regex".into(), Some(line.trim().to_owned())));
        }
        if self.matches_state(state) {
            return Some((state.as_str().to_owned(), None));
        }
        None
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn new_subagent_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("sa-{}", &raw[..6])
}

pub fn start_usage() -> String {
    "usage: olo agent start <claude|codex|agy> --model <MODEL> --effort <EFFORT> [--name NAME] [--trust] [--desktop none|screens=NAME,…|inherit] [--unset-env K]... [--session-id UUID] [--resume ID | --continue] [--fork] [--title TITLE] [--role ROLE] [--cwd DIR] [--prompt TEXT | --prompt-file PATH] [--permission-mode MODE | --approval POLICY | --sandbox MODE | --mode MODE | --full-auto] [--settings JSON_OR_FILE] [--strict-mcp-config] [--disallowed-tools TOOLS...] [--add-dir DIR]... [--env K=V]... [--parent WIN] [--focus] [--close-on-exit] [--json] [-- EXTRA_CLI_ARGS...]\n\nNote: --approve-for-me is Codex-only (off by default, never enabled automatically). Default desktop policy for children is `--desktop none`.".into()
}

pub fn summary_json(entry: &NativeSubagent, state: SubagentLiveState, summary: Option<&str>, depth: usize, children: usize) -> Value {
    json!({
        "id": entry.id,
        "name": entry.name,
        "window_id": entry.window_id,
        "block_id": entry.block_id,
        "parent_window_id": entry.parent_window_id,
        "cli": entry.cli.binary(),
        "model": entry.model,
        "effort": entry.effort,
        "title": entry.display_title(),
        "role": entry.role,
        "cwd": entry.cwd,
        "resume_id": entry.resume_id,
        "cli_session_id": entry.effective_session_id(),
        "transcript_path": entry.transcript_path,
        "desktop": entry.desktop_policy.describe(),
        "started_at": entry.started_at,
        "exited_at": entry.exited_at,
        "pid": entry.last_pid,
        "state": state.as_str(),
        "summary": summary,
        "depth": depth,
        "children": children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_always_requires_explicit_model_and_effort_and_rejects_non_codex_approve_for_me() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(StartSpec::parse_cli_args(&args(&["claude"])).unwrap().build_command().unwrap_err().contains("--model"));
        assert!(StartSpec::parse_cli_args(&args(&["claude", "--model", "claude-opus-5-5"])).unwrap().build_command().unwrap_err().contains("--effort"));
        assert!(StartSpec::parse_cli_args(&args(&["codex", "--resume", "abc123", "--model", "gpt-5.4-codex"])).unwrap().build_command().unwrap_err().contains("--effort"));
        assert!(StartSpec::parse_cli_args(&args(&["agy", "--continue", "--effort", "xhigh"])).unwrap().build_command().unwrap_err().contains("--model"));
        // --approve-for-me is Codex-only!
        assert!(StartSpec::parse_cli_args(&args(&["claude", "--model", "claude-opus-5-5", "--effort", "max", "--approve-for-me"])).unwrap().build_command().unwrap_err().contains("Codex-only"));
    }

    #[test]
    fn builds_commands_with_orchestration_flags_and_desktop_isolation() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let spec = StartSpec::parse_cli_args(&args(&[
            "claude",
            "--model",
            "claude-opus-5-5",
            "--effort",
            "max",
            "--name",
            "builder-1",
            "--trust",
            "--session-id",
            "11111111-2222-3333-4444-555555555555",
            "--strict-mcp-config",
            "--settings",
            "{\"hooks\":{}}",
            "--disallowed-tools",
            "Agent",
            "Workflow",
            "--add-dir",
            "/var/tmp/shared",
            "--permission-mode",
            "auto",
            "--unset-env",
            "CUSTOM_SECRET",
        ]))
        .unwrap();
        assert!(spec.trust);
        assert_eq!(spec.desktop, DesktopPolicy::None);
        assert_eq!(spec.session_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
        let (_, _, _, argv) = spec.build_command().unwrap();
        assert!(argv.windows(2).any(|w| w == ["--session-id", "11111111-2222-3333-4444-555555555555"]));
        assert!(argv.contains(&"--strict-mcp-config".to_owned()));
        assert!(argv.windows(3).any(|w| w == ["--disallowed-tools", "Agent", "Workflow"]));
        assert!(argv.windows(2).any(|w| w == ["--settings", "{\"hooks\":{}}"]));
        assert!(argv.windows(2).any(|w| w == ["--permission-mode", "auto"]));
        assert!(argv.windows(2).any(|w| w == ["--name", "builder-1"]));
        let wrapped = spec.wrap_with_env_isolation(argv);
        assert_eq!(wrapped[0], "env");
        assert!(wrapped.windows(2).any(|w| w == ["-u", "REX_SESSION"]));
        assert!(wrapped.windows(2).any(|w| w == ["-u", "DISPLAY"]));
        assert!(wrapped.windows(2).any(|w| w == ["-u", "CUSTOM_SECRET"]));

        // Agy stream mode + accept-edits + settings
        let agy = StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon", "--effort", "xhigh", "--mode", "accept-edits", "--settings", "{\"allow\":[]}", "--stream"])).unwrap();
        let (_, _, _, a_argv) = agy.build_command().unwrap();
        assert!(a_argv.contains(&"--print".to_owned()));
        assert!(a_argv.windows(2).any(|w| w == ["--input-format", "stream-json"]));
        assert!(a_argv.windows(2).any(|w| w == ["--output-format", "stream-json"]));
        assert!(a_argv.windows(2).any(|w| w == ["--mode", "accept-edits"]));
        assert!(a_argv.windows(2).any(|w| w == ["--settings", "{\"allow\":[]}"]));
    }

    #[test]
    fn desktop_policy_enforces_none_and_scoped_screens() {
        let none = DesktopPolicy::None;
        assert!(none.check_action("olo.op", None).is_err());
        assert!(none.check_action("olo.screen", Some(&json!({"args": ["start", "qa-1"]}))).is_err());
        assert!(none.check_action("olo.open", None).is_err());
        assert!(none.check_action("olo.clipboard.get", None).is_err());
        assert!(none.check_action("olo.agent.list", None).is_ok());

        let scoped = DesktopPolicy::parse("screens=qa-1,qa-2").unwrap();
        assert!(scoped.check_action("olo.op", None).is_err());
        assert!(scoped.check_action("olo.open", None).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["look", "qa-1", "/var/tmp/x.png"]}))).is_ok());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["tree", "qa-2"]}))).is_ok());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["tree", "other-screen"]}))).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["list"]}))).is_err());
    }

    #[test]
    fn classifies_screens_without_false_approval_events() {
        // Bare AgentActivity::Blocked with empty screen must NEVER emit false WaitingApproval!
        let blank = classify_screen_rich(AgentCli::Claude, crate::agent::AgentActivity::Blocked, "", true);
        assert_eq!(blank.state, SubagentLiveState::Working);
        assert!(blank.approval.is_none());

        // Real Agy approval prompt emits WaitingApproval with structured choices
        let agy_ask = "Command\n────────────────────\nRequesting permission for:\n   rm -f /var/tmp/x\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation\n  3. No, cancel\n  ↑/↓ Navigate\n";
        let ap = classify_screen_rich(AgentCli::Agy, crate::agent::AgentActivity::Idle, agy_ask, true);
        assert_eq!(ap.state, SubagentLiveState::WaitingApproval);
        assert!(ap.approval.is_some());

        // Claude folder-trust dialog is WaitingQuestion (never WaitingApproval)
        let trust = "────────────────────\n Accessing workspace:\n /var/tmp/wt\n Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n";
        let tr = classify_screen_rich(AgentCli::Claude, crate::agent::AgentActivity::Blocked, trust, true);
        assert_eq!(tr.state, SubagentLiveState::WaitingQuestion);
        assert!(tr.folder_trust.is_some());
        assert!(tr.approval.is_none());

        // API mid-response disconnect emits Error
        let err = "● Working\n  ⎿  API Error: Connection lost mid-response\n────────────────────\n❯ \n";
        let er = classify_screen_rich(AgentCli::Claude, crate::agent::AgentActivity::Idle, err, true);
        assert_eq!(er.state, SubagentLiveState::Error);
        assert!(er.api_error.as_deref().unwrap().contains("Connection lost mid-response"));
    }

    #[test]
    fn registry_preserves_exited_children_and_resolves_by_name_and_session_id() {
        let mut reg = SubagentRegistry::default();
        let mk = |id: &str, name: &str, wid: &str, pwid: &str, sid: &str| NativeSubagent {
            id: id.into(),
            name: Some(name.into()),
            host_key: "local".into(),
            session_id: "s1".into(),
            window_id: wid.into(),
            block_id: format!("b-{wid}"),
            parent_window_id: pwid.into(),
            parent_block_id: None,
            cli: AgentCli::Claude,
            model: "claude-opus-5-5".into(),
            effort: "max".into(),
            title: name.into(),
            role: None,
            cwd: None,
            resume_id: None,
            cli_session_id: Some(sid.into()),
            transcript_path: None,
            desktop_policy: DesktopPolicy::None,
            stream_mode: false,
            started_at: 100,
            exited_at: None,
            last_pid: Some(999),
        };
        reg.register(mk("sa-1", "builder-a", "w2", "w1", "11111111-2222-3333-4444-555555555555"));
        let live: HashSet<String> = ["w1", "w2"].into_iter().map(str::to_owned).collect();
        assert!(reg.is_live_name_taken("local", "s1", "w1", "builder-a", &live));
        assert_eq!(reg.resolve_target("local", Some("s1"), "builder-a").unwrap().id, "sa-1");
        assert_eq!(reg.resolve_target("local", Some("s1"), "11111111-2222-3333-4444-555555555555").unwrap().id, "sa-1");

        // After Rex server restart (empty live_windows), entry is marked exited with session ID intact!
        let empty: HashSet<String> = ["w1"].into_iter().map(str::to_owned).collect();
        let (changed, exited) = reg.sync_live_windows("local", "s1", &empty);
        assert!(changed);
        assert_eq!(exited.len(), 1);
        assert!(!reg.is_live_name_taken("local", "s1", "w1", "builder-a", &empty));
        let found = reg.resolve_target("local", Some("s1"), "builder-a").unwrap();
        assert!(found.exited_at.is_some());
        assert_eq!(found.cli_session_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
    }

    #[test]
    fn event_log_cursors_and_watch_multi_parse() {
        let mut log = SubagentEventLog::default();
        let mk_ev = |id: &str, state: SubagentLiveState| SubagentEvent {
            seq: 0,
            timestamp_ms: 1000,
            host_key: "local".into(),
            session_id: "s1".into(),
            parent_window_id: "w1".into(),
            id: id.into(),
            name: Some(format!("n-{id}")),
            window_id: format!("win-{id}"),
            cli_session_id: None,
            event: state.as_str().into(),
            state,
            summary: None,
            last_message: Some("DONE".into()),
            approval: None,
            error: None,
            tail: Vec::new(),
        };
        let c1 = log.push(mk_ev("sa-1", SubagentLiveState::Done));
        let c2 = log.push(mk_ev("sa-2", SubagentLiveState::WaitingApproval));
        assert_eq!((c1, c2), (1, 2));
        assert_eq!(log.find_since(0, |_| true).unwrap().id, "sa-1");
        assert_eq!(log.find_since(c1, |_| true).unwrap().id, "sa-2");
        assert!(log.find_since(c2, |_| true).is_none());

        let ws = WatchSpec::parse(Some(&json!({"args": ["--all", "--since", "42", "--on", "done,approval", "--json"]}))).unwrap();
        assert!(ws.all);
        assert_eq!(ws.since, Some(42));
        assert!(ws.matches_state(SubagentLiveState::Done));
        assert!(ws.matches_state(SubagentLiveState::WaitingApproval));
        assert!(!ws.matches_state(SubagentLiveState::Working));

        let ap = ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--json"]}))).unwrap();
        assert_eq!((ap.target.as_str(), ap.once, ap.json), ("builder-a", true, true));
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a"]}))).is_err());
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--no"]}))).is_err());
    }
}
