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
    collections::{HashMap, HashSet},
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
}

/// One registered native Rex child-tab subagent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSubagent {
    /// Short unique handle (e.g. `sa-3f9a1c`) for CLI commands.
    pub id: String,
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
    #[serde(default)]
    pub started_at: i64,
}

impl NativeSubagent {
    /// Human-readable label shown in the tab-tree popover and Activity sidebar.
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            self.title.clone()
        } else if let Some(role) = self.role.as_deref().filter(|r| !r.trim().is_empty()) {
            role.to_owned()
        } else {
            format!("{} ({})", self.cli.display_name(), self.id)
        }
    }
}

/// Persistent parent/child registry of native Rex child-tab subagents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubagentRegistry {
    /// Ordered by launch time so sibling order is stable.
    pub entries: Vec<NativeSubagent>,
}

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

    pub fn remove_window(&mut self, host_key: &str, session_id: &str, window_id: &str) -> bool {
        let removed_parent = self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == window_id).map(|e| e.parent_window_id.clone());
        let before = self.entries.len();
        self.entries.retain(|e| !(e.host_key == host_key && e.session_id == session_id && e.window_id == window_id));
        // If a middle node closed, reparent its children to its own parent (or keep them promoted).
        if let Some(grandparent) = removed_parent {
            for e in &mut self.entries {
                if e.host_key == host_key && e.session_id == session_id && e.parent_window_id == window_id {
                    e.parent_window_id = grandparent.clone();
                }
            }
        }
        self.entries.len() != before
    }

    /// Prunes entries whose Rex window no longer exists in `live_windows` for `(host_key, session_id)`.
    pub fn prune_session(&mut self, host_key: &str, session_id: &str, live_windows: &HashSet<String>) -> bool {
        // Build parent map before removing dead middle nodes so grandchildren reparent upward cleanly.
        let parent_map: HashMap<String, String> =
            self.entries.iter().filter(|e| e.host_key == host_key && e.session_id == session_id).map(|e| (e.window_id.clone(), e.parent_window_id.clone())).collect();
        let mut changed = false;
        self.entries.retain(|e| {
            if e.host_key == host_key && e.session_id == session_id && !live_windows.contains(&e.window_id) {
                changed = true;
                false
            } else {
                true
            }
        });
        // Reparent any remaining child whose direct parent died to its nearest living ancestor.
        for e in &mut self.entries {
            if e.host_key != host_key || e.session_id != session_id {
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
        changed
    }

    pub fn by_window(&self, host_key: &str, session_id: &str, window_id: &str) -> Option<&NativeSubagent> {
        self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == window_id)
    }

    pub fn by_block(&self, host_key: &str, session_id: &str, block_id: &str) -> Option<&NativeSubagent> {
        self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.block_id == block_id)
    }

    /// True when `window_id` is a registered child tab whose ancestor chain still connects to a live parent window.
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

    /// Direct children of `parent_window_id` in stable launch order.
    pub fn direct_children(&self, host_key: &str, session_id: &str, parent_window_id: &str, live_windows: &HashSet<String>) -> Vec<NativeSubagent> {
        self.entries
            .iter()
            .filter(|e| e.host_key == host_key && e.session_id == session_id && e.parent_window_id == parent_window_id && e.window_id != parent_window_id && live_windows.contains(&e.window_id))
            .cloned()
            .collect()
    }

    /// All descendants of `root_window_id` in DFS tree order with `(depth, is_last_sibling, ancestor_continuations, entry)`.
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

    /// Resolve a user/agent target by `id`, `window_id`, `block_id`, or unique title/prefix.
    pub fn resolve_target(&self, host_key: &str, session_id: Option<&str>, target: &str) -> Option<NativeSubagent> {
        let q = target.trim();
        if q.is_empty() {
            return None;
        }
        let matches_scope = |e: &&NativeSubagent| e.host_key == host_key && session_id.is_none_or(|sid| e.session_id == sid);
        if let Some(e) = self.entries.iter().filter(matches_scope).find(|e| e.id == q || e.window_id == q || e.block_id == q) {
            return Some(e.clone());
        }
        let ql = q.to_ascii_lowercase();
        let exact_title: Vec<&NativeSubagent> = self.entries.iter().filter(matches_scope).filter(|e| e.title.to_ascii_lowercase() == ql).collect();
        if exact_title.len() == 1 {
            return Some(exact_title[0].clone());
        }
        let pref: Vec<&NativeSubagent> = self.entries.iter().filter(matches_scope).filter(|e| e.id.starts_with(q) || e.window_id.starts_with(q)).collect();
        if pref.len() == 1 {
            return Some(pref[0].clone());
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

/// Parsed & validated request for `olo agent start` / `olo.agent.start`.
#[derive(Clone, Debug, Default)]
pub struct StartSpec {
    pub cli: Option<AgentCli>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub resume: Option<String>,
    pub continue_last: bool,
    pub fork: bool,
    pub prompt: Option<String>,
    pub title: Option<String>,
    pub role: Option<String>,
    pub cwd: Option<String>,
    pub parent_window: Option<String>,
    pub env: Vec<(String, String)>,
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
    /// Arbitrary passthrough flags/arguments after `--`.
    pub extra: Vec<String>,
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
            continue_last: bool_val("continue") || bool_val("last"),
            fork: bool_val("fork"),
            prompt: str_opt("prompt"),
            title: str_opt("title"),
            role: str_opt("role"),
            cwd: str_opt("cwd"),
            parent_window: str_opt("parent").or_else(|| str_opt("parent_window")),
            env,
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
                "-r" | "--resume" => s.resume = Some(next(&mut i, flag, inline)?),
                "--continue" | "--last" => s.continue_last = true,
                "--fork" | "--fork-session" => s.fork = true,
                "-p" | "--prompt" => s.prompt = Some(next(&mut i, flag, inline)?),
                "--title" => s.title = Some(next(&mut i, flag, inline)?),
                "--role" => s.role = Some(next(&mut i, flag, inline)?),
                "-C" | "--cwd" | "--cd" => s.cwd = Some(next(&mut i, flag, inline)?),
                "--parent" => s.parent_window = Some(next(&mut i, flag, inline)?),
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
                "--allowed-tools" | "--allowedTools" => s.allowed_tools.push(next(&mut i, flag, inline)?),
                "--disallowed-tools" | "--disallowedTools" => s.disallowed_tools.push(next(&mut i, flag, inline)?),
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

        let mut argv: Vec<String> = Vec::new();
        match cli {
            AgentCli::Claude => {
                argv.push("claude".into());
                argv.extend(["--model".into(), model.clone(), "--effort".into(), effort.clone()]);
                if let Some(ref rid) = self.resume {
                    argv.extend(["--resume".into(), rid.clone()]);
                } else if self.continue_last {
                    argv.push("--continue".into());
                }
                if self.fork {
                    argv.push("--fork-session".into());
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
                for t in &self.allowed_tools {
                    argv.extend(["--allowed-tools".into(), t.clone()]);
                }
                for t in &self.disallowed_tools {
                    argv.extend(["--disallowed-tools".into(), t.clone()]);
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
                    argv.extend(["-i".into(), p.clone()]);
                }
            }
        }
        Ok((cli, model, effort, argv))
    }

    pub fn effective_title(&self, cli: AgentCli) -> String {
        if let Some(t) = self.title.as_deref().filter(|s| !s.trim().is_empty()) {
            t.trim().to_owned()
        } else if let Some(r) = self.role.as_deref().filter(|s| !s.trim().is_empty()) {
            r.trim().to_owned()
        } else if let Some(n) = self.name.as_deref().filter(|s| !s.trim().is_empty()) {
            n.trim().to_owned()
        } else {
            cli.display_name().to_owned()
        }
    }
}

/// Encodes text, `--paste`, `--ctrl <KEY>`, `--key <NAME>`, and `--enter` into terminal bytes.
pub fn encode_send_payload(args: &[String], raw_obj: Option<&Value>) -> Result<(String, String, bool), String> {
    let mut target: Option<String> = None;
    let mut out = String::new();
    let mut paste = false;
    let mut add_enter = false;
    let mut as_json = false;
    let mut text_parts: Vec<String> = Vec::new();
    let mut post_keys: Vec<String> = Vec::new();

    if let Some(v) = raw_obj
        && v.get("args").is_none()
    {
        target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
        paste = v.get("paste").and_then(Value::as_bool).unwrap_or(false);
        add_enter = v.get("enter").and_then(Value::as_bool).unwrap_or(false);
        as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
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

    let target = target.ok_or_else(|| "usage: olo agent send <ID> [TEXT] [--enter] [--paste] [--ctrl c|d|z] [--key Enter|Escape|Tab|Up|Down|Left|Right]".to_owned())?;
    let body = text_parts.join(" ");
    if !body.is_empty() {
        if paste {
            out.push_str("\x1b[200~");
            out.push_str(&body);
            out.push_str("\x1b[201~");
        } else {
            out.push_str(&body);
        }
    }
    if add_enter {
        out.push('\r');
    }
    for k in post_keys {
        out.push_str(&k);
    }
    if out.is_empty() {
        return Err("nothing to send: provide TEXT, --enter, --ctrl <KEY>, or --key <NAME>".into());
    }
    Ok((target, out, as_json))
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

/// Structured state of a native child-tab subagent derived from live screen + Rex activity.
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

/// Evaluates a child tab's current screen & activity against `olo_agent_screen`.
pub fn classify_screen(cli: AgentCli, activity: crate::agent::AgentActivity, screen_text: &str, alive: bool) -> (SubagentLiveState, Option<String>) {
    if !alive {
        return (SubagentLiveState::Exited, Some("tab closed".into()));
    }
    let agent = match cli {
        AgentCli::Claude => olo_agent_screen::Agent::Claude,
        AgentCli::Codex => olo_agent_screen::Agent::Codex,
        AgentCli::Agy => olo_agent_screen::Agent::Antigravity,
    };
    if let Some(ask) = olo_agent_screen::approvable(screen_text) {
        return (SubagentLiveState::WaitingApproval, Some(ask.question));
    }
    let last_nonempty = || screen_text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned);
    match olo_agent_screen::classify(agent, screen_text) {
        olo_agent_screen::State::Blocked => {
            let question = screen_text.lines().rev().map(str::trim).find(|l| l.ends_with('?')).map(str::to_owned).or_else(last_nonempty);
            let lower = question.as_deref().unwrap_or("").to_ascii_lowercase();
            let is_approval =
                lower.contains("approve") || lower.contains("allow") || lower.contains("permission") || lower.contains("run command") || lower.contains("yes/no") || lower.contains("(y/n)");
            let state = if is_approval { SubagentLiveState::WaitingApproval } else { SubagentLiveState::WaitingQuestion };
            return (state, question);
        }
        olo_agent_screen::State::Working => {
            return (SubagentLiveState::Working, last_nonempty());
        }
        olo_agent_screen::State::Idle => {}
    }
    match activity {
        crate::agent::AgentActivity::Blocked => (SubagentLiveState::WaitingApproval, last_nonempty().or_else(|| Some("waiting for approval".into()))),
        crate::agent::AgentActivity::Working => (SubagentLiveState::Working, last_nonempty()),
        crate::agent::AgentActivity::Error => (SubagentLiveState::Error, last_nonempty()),
        crate::agent::AgentActivity::Done => (SubagentLiveState::Done, last_nonempty()),
        crate::agent::AgentActivity::Idle => {
            let summary = if olo_agent_screen::at_prompt(agent, screen_text) { Some("at prompt".into()) } else { last_nonempty() };
            (SubagentLiveState::Idle, summary)
        }
    }
}

/// Parsed options for `olo agent watch <TARGET>`.
#[derive(Clone, Debug)]
pub struct WatchSpec {
    pub target: String,
    pub events: Vec<String>,
    pub match_text: Option<String>,
    pub regex_pat: Option<String>,
    pub timeout_secs: u64,
    pub json: bool,
}

impl WatchSpec {
    pub fn parse(raw: Option<&Value>) -> Result<Self, String> {
        let Some(v) = raw else {
            return Err("usage: olo agent watch <ID> [--on question|approval|idle|done|error|exit|any] [--match TEXT] [--regex PAT] [--timeout SEC] [--json]".into());
        };
        let mut target: Option<String> = None;
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
                    s if s.starts_with("--on=") => events.extend(s["--on=".len()..].split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty())),
                    s if s.starts_with("--match=") => match_text = Some(s["--match=".len()..].to_owned()),
                    s if s.starts_with("--regex=") => regex_pat = Some(s["--regex=".len()..].to_owned()),
                    s if s.starts_with("--timeout=") => {
                        let t = &s["--timeout=".len()..];
                        timeout_secs = t.trim_end_matches('s').parse::<u64>().map_err(|_| format!("invalid --timeout `{t}`"))?;
                    }
                    s if s.starts_with('-') => return Err(format!("unknown watch option `{s}`")),
                    s if target.is_none() => target = Some(s.to_owned()),
                    s => return Err(format!("unexpected argument `{s}`")),
                }
                i += 1;
            }
        } else {
            target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
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

        let target = target.ok_or_else(|| "usage: olo agent watch <ID> [--on question|approval|idle|done|error|exit|any] [--match TEXT] [--regex PAT] [--timeout SEC]".to_owned())?;
        if events.is_empty() && match_text.is_none() && regex_pat.is_none() {
            events = vec!["question".into(), "approval".into(), "idle".into(), "done".into(), "error".into(), "exit".into()];
        }
        Ok(Self { target, events, match_text, regex_pat, timeout_secs: timeout_secs.clamp(1, 3600), json })
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
        let state_hit = self.events.iter().any(|ev| match ev.as_str() {
            "any" => !matches!(state, SubagentLiveState::Working),
            "question" | "waiting_question" => state == SubagentLiveState::WaitingQuestion,
            "approval" | "waiting_approval" => state == SubagentLiveState::WaitingApproval,
            "waiting" => matches!(state, SubagentLiveState::WaitingQuestion | SubagentLiveState::WaitingApproval),
            "idle" => state == SubagentLiveState::Idle,
            "done" => state == SubagentLiveState::Done,
            "error" | "failed" => state == SubagentLiveState::Error,
            "exit" | "exited" | "closed" => state == SubagentLiveState::Exited,
            _ => false,
        });
        if state_hit {
            return Some((state.as_str().to_owned(), None));
        }
        None
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn new_subagent_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("sa-{}", &raw[..6])
}

pub fn start_usage() -> String {
    "usage: olo agent start <claude|codex|agy> --model <MODEL> --effort <EFFORT> [--resume ID | --continue] [--fork] [--title TITLE] [--role ROLE] [--cwd DIR] [--prompt TEXT] [--permission-mode MODE | --approval POLICY | --sandbox MODE | --mode MODE | --full-auto | --dangerously-skip-permissions] [--add-dir DIR]... [--env K=V]... [--focus] [--close-on-exit] [--json] [-- EXTRA_CLI_ARGS...]".into()
}

pub fn summary_json(entry: &NativeSubagent, state: SubagentLiveState, summary: Option<&str>, depth: usize, children: usize) -> Value {
    json!({
        "id": entry.id,
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
        "started_at": entry.started_at,
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
    fn start_always_requires_explicit_model_and_effort() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // Missing both
        let e = StartSpec::parse_cli_args(&args(&["claude"])).unwrap().build_command().unwrap_err();
        assert!(e.contains("--model"), "{e}");
        // Missing effort
        let e = StartSpec::parse_cli_args(&args(&["claude", "--model", "claude-opus-5-5"])).unwrap().build_command().unwrap_err();
        assert!(e.contains("--effort"), "{e}");
        // Missing effort even on --resume!
        let e = StartSpec::parse_cli_args(&args(&["codex", "--resume", "abc123", "--model", "gpt-5.4-codex"])).unwrap().build_command().unwrap_err();
        assert!(e.contains("--effort"), "{e}");
        // Missing model even on --continue!
        let e = StartSpec::parse_cli_args(&args(&["agy", "--continue", "--effort", "xhigh"])).unwrap().build_command().unwrap_err();
        assert!(e.contains("--model"), "{e}");
    }

    #[test]
    fn builds_commands_for_claude_codex_and_agy_new_and_resume() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

        // Claude new + options + passthrough
        let (cli, m, e, argv) = StartSpec::parse_cli_args(&args(&[
            "claude",
            "--model",
            "claude-opus-5-5",
            "--effort",
            "max",
            "--permission-mode",
            "acceptEdits",
            "--add-dir",
            "/tmp/x",
            "--allowed-tools",
            "Bash(git*)",
            "--title",
            "Reviewer",
            "--prompt",
            "Audit diff",
            "--",
            "--verbose",
        ]))
        .unwrap()
        .build_command()
        .unwrap();
        assert_eq!((cli, m.as_str(), e.as_str()), (AgentCli::Claude, "claude-opus-5-5", "max"));
        assert_eq!(
            argv,
            vec![
                "claude",
                "--model",
                "claude-opus-5-5",
                "--effort",
                "max",
                "--permission-mode",
                "acceptEdits",
                "--allowed-tools",
                "Bash(git*)",
                "--add-dir",
                "/tmp/x",
                "--name",
                "Reviewer",
                "--verbose",
                "Audit diff",
            ]
        );

        // Codex resume with explicit model + effort + approval/sandbox
        let (cli, _, _, argv) = StartSpec::parse_cli_args(&args(&[
            "codex",
            "--model",
            "gpt-5.4-codex",
            "--effort",
            "high",
            "--resume",
            "sess-42",
            "--approval",
            "on-request",
            "--sandbox",
            "workspace-write",
            "--prompt",
            "Continue",
        ]))
        .unwrap()
        .build_command()
        .unwrap();
        assert_eq!(cli, AgentCli::Codex);
        assert_eq!(argv, vec!["codex", "resume", "-m", "gpt-5.4-codex", "-c", "model_reasoning_effort=\"high\"", "-a", "on-request", "-s", "workspace-write", "sess-42", "Continue",]);

        // Agy new with mode + sandbox + interactive prompt
        let (cli, _, _, argv) = StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon", "--effort", "xhigh", "--mode", "accept-edits", "--sandbox", "--prompt", "Inspect UI"]))
            .unwrap()
            .build_command()
            .unwrap();
        assert_eq!(cli, AgentCli::Agy);
        assert_eq!(argv, vec!["agy", "--model", "gemini-4-neon", "--effort", "xhigh", "--mode", "accept-edits", "--sandbox", "-i", "Inspect UI",]);
    }

    #[test]
    fn encodes_text_paste_ctrl_and_special_keys() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let (t, bytes, _) = encode_send_payload(&args(&["sa-1", "hello", "world", "--enter"]), None).unwrap();
        assert_eq!((t.as_str(), bytes.as_str()), ("sa-1", "hello world\r"));
        let (_, bytes, _) = encode_send_payload(&args(&["sa-1", "--ctrl", "c"]), None).unwrap();
        assert_eq!(bytes, "\x03");
        let (_, bytes, _) = encode_send_payload(&args(&["sa-1", "--key", "Escape", "--key", "Up"]), None).unwrap();
        assert_eq!(bytes, "\x1b\x1b[A");
        let (_, bytes, _) = encode_send_payload(&args(&["sa-1", "line1\nline2", "--paste", "--enter"]), None).unwrap();
        assert_eq!(bytes, "\x1b[200~line1\nline2\x1b[201~\r");
    }

    #[test]
    fn nested_tree_hierarchy_and_pruning() {
        let mut reg = SubagentRegistry::default();
        let mk = |id: &str, wid: &str, pwid: &str, title: &str| NativeSubagent {
            id: id.into(),
            host_key: "local".into(),
            session_id: "s1".into(),
            window_id: wid.into(),
            block_id: format!("b-{wid}"),
            parent_window_id: pwid.into(),
            parent_block_id: None,
            cli: AgentCli::Claude,
            model: "claude-opus-5-5".into(),
            effort: "max".into(),
            title: title.into(),
            role: None,
            cwd: None,
            resume_id: None,
            started_at: 100,
        };
        reg.register(mk("sa-1", "w2", "w1", "Worker A"));
        reg.register(mk("sa-2", "w3", "w2", "Subworker A1"));
        reg.register(mk("sa-3", "w4", "w1", "Worker B"));
        let live: HashSet<String> = ["w1", "w2", "w3", "w4"].into_iter().map(str::to_owned).collect();

        assert!(!reg.is_child_window("local", "s1", "w1", &live));
        assert!(reg.is_child_window("local", "s1", "w2", &live));
        assert!(reg.is_child_window("local", "s1", "w3", &live));
        assert_eq!(reg.root_ancestor("local", "s1", "w3", &live), "w1");

        let rows = reg.descendants_tree("local", "s1", "w1", &live);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].depth, rows[0].menu_prefix().as_str(), rows[0].entry.id.as_str()), (1, "├─ ", "sa-1"));
        assert_eq!((rows[1].depth, rows[1].menu_prefix().as_str(), rows[1].entry.id.as_str()), (2, "│  └─ ", "sa-2"));
        assert_eq!((rows[2].depth, rows[2].menu_prefix().as_str(), rows[2].entry.id.as_str()), (1, "└─ ", "sa-3"));

        // Removing middle node w2 reparents grandchild w3 cleanly to w1!
        let live2: HashSet<String> = ["w1", "w3", "w4"].into_iter().map(str::to_owned).collect();
        assert!(reg.prune_session("local", "s1", &live2));
        let rows2 = reg.descendants_tree("local", "s1", "w1", &live2);
        assert_eq!(rows2.len(), 2);
        assert_eq!(rows2[0].depth, 1);
        assert_eq!(rows2[0].entry.id, "sa-2");
    }

    #[test]
    fn builds_valid_commands_for_claude_codex_and_agy_and_classifies_screens() {
        let args = |xs: &[&str]| xs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let codex = StartSpec::parse_cli_args(&args(&["codex", "--model", "gpt-5.4-codex", "--effort", "high", "--resume", "uuid-1", "--full-auto", "--approve-for-me", "Fix tests"])).unwrap();
        let (_, _, _, argv) = codex.build_command().unwrap();
        assert_eq!(&argv[..2], ["codex", "resume"]);
        assert!(argv.windows(2).any(|w| w == ["-a", "on-request"]));
        assert!(argv.windows(2).any(|w| w == ["-s", "workspace-write"]));
        assert!(argv.contains(&"--approve-for-me".to_owned()));
        assert!(!argv.contains(&"--full-auto".to_owned()));
        assert_eq!(&argv[argv.len() - 2..], ["uuid-1", "Fix tests"]);

        let agy = StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon", "--effort", "xhigh", "--continue", "--mode", "auto", "--sandbox", "Review PR"])).unwrap();
        let (_, _, _, argv) = agy.build_command().unwrap();
        assert_eq!(argv[0], "agy");
        assert!(argv.contains(&"--continue".to_owned()));
        assert!(argv.windows(2).any(|w| w == ["--mode", "auto"]));
        assert!(argv.contains(&"--sandbox".to_owned()));
        assert!(argv.windows(2).any(|w| w == ["-i", "Review PR"]));

        let (st, _) = classify_screen(AgentCli::Agy, crate::agent::AgentActivity::Idle, "Allow command?\n> 1. Yes, run command\n↑/↓ Navigate", true);
        assert_eq!(st, SubagentLiveState::WaitingApproval);
        let (st_done, _) = classify_screen(AgentCli::Claude, crate::agent::AgentActivity::Done, "All done", true);
        assert_eq!(st_done, SubagentLiveState::Done);
        let (st_exit, _) = classify_screen(AgentCli::Codex, crate::agent::AgentActivity::Idle, "", false);
        assert_eq!(st_exit, SubagentLiveState::Exited);
    }
}
