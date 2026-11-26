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

    /// Strict allow-list enforcing this policy against EVERY incoming Rex client action
    /// (including `olo.*`, `agent.*`, and catch-all UI actions).
    pub fn check_action(&self, action: &str, args: Option<&Value>) -> Result<(), String> {
        if matches!(self, Self::Inherit) {
            return Ok(());
        }
        // Orchestration (`olo.agent.*`) and bridge plumbing (`olo.wait`, `olo.file`, UUIDv4-addressed)
        // are allowed for every child; target ownership and grandchild policy caps are enforced by
        // `handle_agent_action`. Every other action (including `olo.passwords.*`, `agent.auto_approve_toggle`,
        // `olo.op`, `olo.open`, `olo.clipboard.*`, etc.) is denied unless explicitly permitted below!
        if action.starts_with("olo.agent.") || matches!(action, "olo.wait" | "olo.file") {
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

    /// Enforces monotonic privilege attenuation (`none ⊂ screens(subset) ⊂ inherit`) when a child
    /// launches a grandchild subagent: a restricted child may NEVER escalate a grandchild's policy.
    pub fn restrict_to(&self, requested: &Self) -> Result<Self, String> {
        match (self, requested) {
            (Self::Inherit, req) => Ok(req.clone()),
            (Self::None, Self::None) => Ok(Self::None),
            (Self::None, req) => Err(format!("cannot launch grandchild with `--desktop {}`: parent child runs with `--desktop none`", req.describe())),
            (Self::Screens { .. }, Self::None) => Ok(Self::None),
            (Self::Screens { names: allowed }, Self::Screens { names: req }) => {
                for n in req {
                    if !allowed.iter().any(|a| a == n) {
                        return Err(format!("cannot grant screen `{n}` to grandchild: parent child is restricted to screens [{}]", allowed.join(", ")));
                    }
                }
                Ok(Self::Screens { names: req.clone() })
            }
            (Self::Screens { names }, Self::Inherit) => Err(format!("cannot launch grandchild with `--desktop inherit`: parent child is restricted to `--desktop screens={}`", names.join(","))),
        }
    }
}

/// Extracts the target Agent Screen name from `olo screen <subcmd> [flags] <NAME> ...`.
/// Strictly rejects unknown subcommands, leading global flags before the subcommand, or
/// unknown value-taking flags so a restricted child can never shift the screen argument.
pub fn screen_target_name(args: &[String]) -> Option<&str> {
    let mut it = args.iter().map(String::as_str);
    let sub = it.next()?;
    if sub.starts_with('-')
        || !matches!(sub, "start" | "run" | "tree" | "click" | "type" | "set" | "key" | "wait" | "look" | "stop" | "scale" | "theme" | "size" | "close" | "scroll" | "hover" | "focus")
    {
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
        if matches!(tok, "--size" | "--scale" | "--theme" | "--timeout" | "--home" | "--owner" | "--role" | "--name" | "--text" | "--window" | "--out") {
            skip_next = true;
            continue;
        }
        if tok.starts_with("--") && tok.contains('=') {
            continue;
        }
        if tok.starts_with('-') {
            // Reject unknown bare flags before the screen name so an unknown value-taking flag cannot shift `NAME`.
            if !matches!(tok, "--json" | "--all" | "--wait" | "-q" | "--quiet" | "--dark" | "--light") {
                return None;
            }
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
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub last_pid: Option<i32>,
    /// Session view revision at which this child was created (prevents older concurrent views from falsely marking it exited).
    #[serde(default)]
    pub created_rev: u64,
    /// True once observed live in a session view (guards parallel `start` bursts against premature exit).
    #[serde(default)]
    pub seen_live: bool,
    /// True if `exited_at` was set by `sync_live_windows` (so it can heal if the window reappears in a newer view).
    #[serde(default)]
    pub exited_by_sync: bool,
    /// Confidence tag of `cli_session_id` discovery (`record`, `open`, `daemon`, `argv`, `written`, `known`, `guess`).
    #[serde(default)]
    pub session_confidence: Option<String>,
    /// Extra `--add-dir` paths explicitly passed at launch (used to bound `--trust` auto-confirm).
    #[serde(default)]
    pub add_dirs: Vec<String>,
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
    /// Monotonic event cursor persisted across in-place Olo Rex reloads (`H8`).
    #[serde(default)]
    pub event_seq: u64,
}

const MAX_EXITED_HISTORY: usize = 200;
const EXITED_TTL_MS: i64 = 7 * 86_400 * 1000;

/// Normalizes a Unix timestamp (legacy seconds or milliseconds) to Unix milliseconds.
pub fn to_unix_ms(ts: i64) -> i64 {
    if ts > 0 && ts < 10_000_000_000 { ts.saturating_mul(1000) } else { ts }
}

fn confidence_rank(c: Option<&str>) -> u8 {
    match c {
        Some("record") => 7,
        Some("open") => 6,
        Some("daemon") => 5,
        Some("argv") => 4,
        Some("written") => 3,
        Some("known") => 2,
        Some("guess") => 1,
        _ => 0,
    }
}

impl SubagentRegistry {
    pub fn state_path() -> PathBuf {
        olo_xdg::state_dir().join("rex-subagents.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::state_path())
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&raw) {
            Ok(reg) => reg,
            Err(e) => {
                let bad = path.with_extension("json.bad");
                let _ = std::fs::write(&bad, &raw);
                tracing::warn!("Corrupt subagent registry at {} ({e}); preserved backup at {}", path.display(), bad.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        self.save_to(&Self::state_path());
    }

    pub fn save_to(&self, path: &Path) {
        use std::{
            io::Write as _,
            sync::atomic::{AtomicU64, Ordering},
        };
        static SAVE_SEQ: AtomicU64 = AtomicU64::new(1);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let n = SAVE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("json.tmp.{}.{n}", std::process::id()));
        if let Ok(json) = serde_json::to_string_pretty(self)
            && let Ok(mut f) = std::fs::File::create(&tmp)
            && f.write_all(json.as_bytes()).is_ok()
        {
            let _ = f.sync_all();
            drop(f);
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

    /// Returns a currently live child already running `cli_session_id` (prevents two live CLIs appending to one transcript).
    pub fn live_using_session_id(&self, host_key: &str, target_sid: &str) -> Option<&NativeSubagent> {
        let q = target_sid.trim();
        if q.is_empty() {
            return None;
        }
        self.entries.iter().find(|e| e.host_key == host_key && e.exited_at.is_none() && e.effective_session_id().is_some_and(|s| s == q))
    }

    /// Marks `window_id` as exited (preserving its `cli_session_id` and metadata for
    /// `list --all` and `--resume`), reparenting any surviving children upward.
    pub fn mark_exited(&mut self, host_key: &str, session_id: &str, window_id: &str) -> bool {
        self.mark_exited_with_code(host_key, session_id, window_id, None)
    }

    pub fn mark_exited_with_code(&mut self, host_key: &str, session_id: &str, window_id: &str, exit_code: Option<i32>) -> bool {
        let parent = self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == window_id).map(|e| e.parent_window_id.clone());
        let mut changed = false;
        let now = now_unix_ms() as i64;
        for e in &mut self.entries {
            if e.host_key == host_key && e.session_id == session_id && e.window_id == window_id && e.exited_at.is_none() {
                e.exited_at = Some(now);
                e.exited_by_sync = false;
                if exit_code.is_some() {
                    e.exit_code = exit_code;
                }
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
    /// every exited child with its session ID ready for `--resume`).
    /// Guards against concurrent `start` bursts (`H1`) via `created_rev` and `seen_live`,
    /// and heals any entry that reappears in a newer view.
    pub fn sync_live_windows(&mut self, host_key: &str, session_id: &str, live_windows: &HashSet<String>, view_rev: u64) -> (bool, Vec<NativeSubagent>) {
        let parent_map: HashMap<String, String> =
            self.entries.iter().filter(|e| e.host_key == host_key && e.session_id == session_id).map(|e| (e.window_id.clone(), e.parent_window_id.clone())).collect();
        let mut changed = false;
        let mut newly_exited = Vec::new();
        let now = now_unix_ms() as i64;
        for e in &mut self.entries {
            if e.host_key != host_key || e.session_id != session_id {
                continue;
            }
            if live_windows.contains(&e.window_id) {
                if !e.seen_live {
                    e.seen_live = true;
                    changed = true;
                }
                if e.exited_at.is_some() && e.exited_by_sync {
                    e.exited_at = None;
                    e.exited_by_sync = false;
                    changed = true;
                }
            } else if e.exited_at.is_none() {
                // Never mark exited from a view older than the child's own creation view, or while brand-new and unseen.
                if view_rev > 0 && e.created_rev > 0 && view_rev < e.created_rev {
                    continue;
                }
                if !e.seen_live && now.saturating_sub(to_unix_ms(e.started_at)) < 5_000 {
                    continue;
                }
                e.exited_at = Some(now);
                e.exited_by_sync = true;
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
        let cutoff = (now_unix_ms() as i64) - EXITED_TTL_MS;
        let before = self.entries.len();
        self.entries.retain(|e| e.exited_at.is_none_or(|t| to_unix_ms(t) >= cutoff));
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

    pub fn update_discovered(&mut self, id: &str, cli_session_id: Option<&str>, confidence: Option<&str>, transcript_path: Option<&str>, pid: Option<i32>) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { return false };
        let mut changed = false;
        if let Some(sid) = cli_session_id.filter(|s| !s.is_empty()) {
            let new_rank = confidence_rank(confidence);
            let old_rank = confidence_rank(e.session_confidence.as_deref());
            // Allow surer evidence (e.g. Claude's PID record or open FD) to replace an earlier weaker guess (`H13`).
            let should_update = e.cli_session_id.is_none() || (e.cli_session_id.as_deref() != Some(sid) && new_rank > old_rank);
            if should_update {
                e.cli_session_id = Some(sid.to_owned());
                e.session_confidence = confidence.map(str::to_owned);
                changed = true;
            } else if e.cli_session_id.as_deref() == Some(sid) && new_rank > old_rank {
                e.session_confidence = confidence.map(str::to_owned);
                changed = true;
            }
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
            changed = true;
        }
        changed
    }

    pub fn by_window(&self, host_key: &str, session_id: &str, window_id: &str) -> Option<&NativeSubagent> {
        self.entries.iter().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == window_id && e.exited_at.is_none())
    }

    pub fn by_block(&self, host_key: &str, session_id: Option<&str>, block_id: &str) -> Option<&NativeSubagent> {
        self.entries
            .iter()
            .find(|e| e.host_key == host_key && session_id.is_none_or(|s| e.session_id == s) && e.block_id == block_id && e.exited_at.is_none())
            .or_else(|| self.entries.iter().find(|e| e.host_key == host_key && e.block_id == block_id && e.exited_at.is_none()))
    }

    /// Finds any child registered for `block_id` on `host_key` regardless of `exited_at` (used for fail-closed desktop isolation `C1`).
    pub fn by_block_any(&self, host_key: &str, session_id: Option<&str>, block_id: &str) -> Option<&NativeSubagent> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.host_key == host_key && session_id.is_none_or(|s| e.session_id == s) && e.block_id == block_id)
            .or_else(|| self.entries.iter().rev().find(|e| e.host_key == host_key && e.block_id == block_id))
    }

    /// True when `host_key` (optionally scoped to `session_id`) has ANY live restricted child (`desktop_policy != Inherit`).
    pub fn has_restricted_children(&self, host_key: &str, session_id: Option<&str>) -> bool {
        self.entries.iter().any(|e| e.host_key == host_key && session_id.is_none_or(|s| e.session_id == s) && e.exited_at.is_none() && !matches!(e.desktop_policy, DesktopPolicy::Inherit))
    }

    /// Returns the full ancestor chain `[parent_window_id, grandparent_window_id, ...]` for `window_id` (`M11`).
    pub fn ancestor_windows(&self, host_key: &str, session_id: &str, window_id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = window_id.to_owned();
        let mut seen = HashSet::new();
        seen.insert(cur.clone());
        while let Some(e) = self.entries.iter().rev().find(|e| e.host_key == host_key && e.session_id == session_id && e.window_id == cur) {
            if !seen.insert(e.parent_window_id.clone()) {
                break;
            }
            out.push(e.parent_window_id.clone());
            cur = e.parent_window_id.clone();
        }
        out
    }

    /// True if `target_window_id` is a strict descendant of `ancestor_window_id` (never self, never ancestor, never sibling!).
    pub fn is_strict_descendant(&self, host_key: &str, session_id: &str, ancestor_window_id: &str, target_window_id: &str) -> bool {
        if ancestor_window_id == target_window_id {
            return false;
        }
        self.ancestor_windows(host_key, session_id, target_window_id).iter().any(|w| w == ancestor_window_id)
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

    /// Resolve a target (`id`, `name`, `PARENT/NAME`, CLI `session_id`, `window_id`, `block_id`, or unique title/prefix)
    /// enforcing strict subtree authorization (`C3`) and disambiguation (`H16`):
    /// - When `caller_is_child` is true, ONLY strict descendants of `caller_win` may be targeted (never self, parent, sibling, or other session).
    /// - When top-level, prefers caller's own subtree first, then unambiguous match in `session_id`, erroring on ambiguity with candidate IDs.
    pub fn resolve_target_scoped(&self, host_key: &str, session_id: Option<&str>, caller_win: Option<&str>, caller_is_child: bool, target: &str) -> Result<NativeSubagent, String> {
        let q = target.trim();
        if q.is_empty() {
            return Err("empty subagent target".into());
        }
        // Optional `PARENT/NAME` syntax (`H16`).
        let (parent_hint, leaf) = match q.split_once('/') {
            Some((p, n)) if !p.is_empty() && !n.is_empty() => (Some(p), n),
            _ => (None, q),
        };
        let parent_hint_win: Option<String> = parent_hint.and_then(|ph| {
            self.entries
                .iter()
                .rev()
                .find(|e| e.host_key == host_key && session_id.is_none_or(|s| e.session_id == s) && (e.id == ph || e.window_id == ph || e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(ph))))
                .map(|e| e.window_id.clone())
                .or_else(|| Some(ph.to_owned()))
        });
        let ql = leaf.to_ascii_lowercase();

        let find_in = |pool: &[&NativeSubagent]| -> Result<Option<NativeSubagent>, String> {
            for live_only in [true, false] {
                let candidates: Vec<&NativeSubagent> =
                    pool.iter().copied().filter(|e| (!live_only || e.exited_at.is_none()) && parent_hint_win.as_deref().is_none_or(|pw| e.parent_window_id == pw)).collect();
                // 1. Globally unique IDs (`id`, `window_id`, `block_id`, `cli_session_id`).
                let id_hits: Vec<&NativeSubagent> = candidates
                    .iter()
                    .copied()
                    .filter(|e| e.id == leaf || e.window_id == leaf || e.block_id == leaf || e.cli_session_id.as_deref() == Some(leaf) || e.resume_id.as_deref() == Some(leaf))
                    .collect();
                if id_hits.len() == 1 {
                    return Ok(Some(id_hits[0].clone()));
                } else if id_hits.len() > 1 {
                    let list: Vec<String> = id_hits.iter().map(|e| format!("{} ({})", e.id, e.display_title())).collect();
                    return Err(format!("ambiguous target `{target}`; matches: {} (use exact ID)", list.join(", ")));
                }
                // 2. Name match (error on ambiguity across different parents!).
                let name_hits: Vec<&NativeSubagent> = candidates.iter().copied().filter(|e| e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(leaf))).collect();
                if name_hits.len() == 1 {
                    return Ok(Some(name_hits[0].clone()));
                } else if name_hits.len() > 1 {
                    let list: Vec<String> = name_hits.iter().map(|e| format!("{} (parent={})", e.id, e.parent_window_id)).collect();
                    return Err(format!("ambiguous name `{target}` matches multiple children: {} (use ID or PARENT/NAME)", list.join(", ")));
                }
                // 3. Unique exact title match.
                let title_hits: Vec<&NativeSubagent> = candidates.iter().copied().filter(|e| e.title.to_ascii_lowercase() == ql).collect();
                if title_hits.len() == 1 {
                    return Ok(Some(title_hits[0].clone()));
                }
                // 4. Unique prefix match.
                let pref: Vec<&NativeSubagent> =
                    candidates.iter().copied().filter(|e| e.id.starts_with(leaf) || e.window_id.starts_with(leaf) || e.cli_session_id.as_deref().is_some_and(|s| s.starts_with(leaf))).collect();
                if pref.len() == 1 {
                    return Ok(Some(pref[0].clone()));
                }
            }
            Ok(None)
        };

        // Strict descendant confinement for child subagent callers (`C3`).
        if caller_is_child {
            let (Some(sid), Some(cwin)) = (session_id, caller_win) else {
                return Err(format!("target `{target}` refused: child subagents may only address their own descendants"));
            };
            let descendants: Vec<&NativeSubagent> =
                self.entries.iter().filter(|e| e.host_key == host_key && e.session_id == sid && self.is_strict_descendant(host_key, sid, cwin, &e.window_id)).collect();
            if let Some(hit) = find_in(&descendants)? {
                return Ok(hit);
            }
            return Err(format!("subagent `{target}` is not a descendant of this child tab (children may not act on themselves, their parent, or siblings)"));
        }

        // Top-level orchestrator: prefer own subtree first, then session scope, then exited host history.
        if let (Some(sid), Some(cwin)) = (session_id, caller_win) {
            let own_tree: Vec<&NativeSubagent> =
                self.entries.iter().filter(|e| e.host_key == host_key && e.session_id == sid && self.is_strict_descendant(host_key, sid, cwin, &e.window_id)).collect();
            if let Some(hit) = find_in(&own_tree)? {
                return Ok(hit);
            }
        }
        if let Some(sid) = session_id {
            let in_sess: Vec<&NativeSubagent> = self.entries.iter().filter(|e| e.host_key == host_key && e.session_id == sid).collect();
            if let Some(hit) = find_in(&in_sess)? {
                return Ok(hit);
            }
        }
        // Fallback: exited entries from earlier Rex sessions on this host (or any session if caller has no session_id).
        let host_pool: Vec<&NativeSubagent> = self.entries.iter().filter(|e| e.host_key == host_key && (session_id.is_none() || e.exited_at.is_some())).collect();
        if let Some(hit) = find_in(&host_pool)? {
            return Ok(hit);
        }
        Err(format!("subagent `{target}` not found (run `olo agent list --all` to see active and exited child tabs)"))
    }

    /// Resolve a user/agent target by `id`, `name`, CLI `session_id`, `window_id`, `block_id`, or unique title/prefix.
    /// Live children are preferred over exited children when a name was reused.
    pub fn resolve_target(&self, host_key: &str, session_id: Option<&str>, target: &str) -> Option<NativeSubagent> {
        self.resolve_target_scoped(host_key, session_id, None, false, target).ok()
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
    /// Full ancestor window chain so `watch --all` still matches exited grandchildren (`M11`).
    #[serde(default)]
    pub ancestor_windows: Vec<String>,
    pub id: String,
    pub name: Option<String>,
    pub window_id: String,
    #[serde(default)]
    pub block_id: String,
    #[serde(default)]
    pub resume_id: Option<String>,
    pub cli_session_id: Option<String>,
    pub event: String,
    pub state: SubagentLiveState,
    pub summary: Option<String>,
    pub last_message: Option<String>,
    pub approval: Option<Value>,
    pub error: Option<String>,
    pub tail: Vec<String>,
    /// Internal key (`approval:<key>`, `question:<hash>`, `msg:<hash>`, `exited`, etc.) used to deduplicate
    /// identical consecutive pushes per child (`H5`/`H7`) without dropping A->B transitions.
    #[serde(skip)]
    pub dedupe_key: String,
}

const MAX_EVENT_LOG: usize = 2000;

/// Monotonic ring buffer of child subagent state transitions so back-to-back
/// `olo agent watch --all --since CURSOR` calls never lose an event.
#[derive(Debug, Default)]
pub struct SubagentEventLog {
    next_seq: u64,
    events: VecDeque<SubagentEvent>,
    /// Last `(state, dedupe_key, seq)` pushed per child `id` so duplicate pushes are dropped cleanly (`H7`).
    last_by_id: HashMap<String, (SubagentLiveState, String, u64)>,
}

impl SubagentEventLog {
    pub fn with_start_seq(start_seq: u64) -> Self {
        Self { next_seq: start_seq, events: VecDeque::new(), last_by_id: HashMap::new() }
    }

    pub fn latest_seq(&self) -> u64 {
        self.next_seq
    }

    /// Sequence number of the oldest event currently retained in the ring buffer (0 when empty).
    pub fn oldest_seq(&self) -> u64 {
        self.events.front().map_or(0, |e| e.seq)
    }

    /// True if `since` is older than the oldest buffered event (meaning events between `since` and
    /// `oldest_seq()` were evicted from the 2000-event ring buffer — `M12`).
    pub fn has_gap(&self, since: u64) -> bool {
        self.events.len() >= MAX_EVENT_LOG && since > 0 && self.oldest_seq() > since.saturating_add(1)
    }

    /// Clears per-child dedupe state (called on `send`/`approve` so the next transition is never swallowed — `H5`).
    pub fn reset_child_dedupe(&mut self, id: &str) {
        self.last_by_id.remove(id);
    }

    pub fn push(&mut self, mut ev: SubagentEvent) -> u64 {
        let key = if ev.dedupe_key.is_empty() { ev.event.clone() } else { ev.dedupe_key.clone() };
        if let Some((last_state, last_key, last_seq)) = self.last_by_id.get(&ev.id)
            && *last_state == ev.state
            && *last_key == key
        {
            return *last_seq;
        }
        self.next_seq += 1;
        ev.seq = self.next_seq;
        self.last_by_id.insert(ev.id.clone(), (ev.state, key, self.next_seq));
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
        // Preserve leading/trailing whitespace & formatting verbatim for prompt/settings (`M23`).
        let raw_str_opt = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
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
                validate_env_name("--env", k)?;
                env.push((k.clone(), val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string())));
            }
        }
        let unset_env = str_vec("unset_env");
        for k in &unset_env {
            validate_env_name("--unset-env", k)?;
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
        let stream = bool_val("stream");
        Ok(Self {
            cli,
            model: str_opt("model"),
            effort: str_opt("effort"),
            resume: str_opt("resume"),
            session_id: str_opt("session_id"),
            continue_last: bool_val("continue") || bool_val("last"),
            fork: bool_val("fork"),
            prompt: raw_str_opt("prompt"),
            title: str_opt("title"),
            role: str_opt("role"),
            cwd: str_opt("cwd"),
            parent_window: str_opt("parent").or_else(|| str_opt("parent_window")),
            env,
            unset_env,
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
            settings: raw_str_opt("settings"),
            strict_mcp_config: bool_val("strict_mcp_config"),
            system_prompt: raw_str_opt("system_prompt"),
            append_system_prompt: raw_str_opt("append_system_prompt"),
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
            print_mode: bool_val("print") || stream,
            input_format: str_opt("input_format").or_else(|| stream.then(|| "stream-json".into())),
            output_format: str_opt("output_format").or_else(|| stream.then(|| "stream-json".into())),
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
                    let k = k.trim();
                    validate_env_name("--unset-env", k)?;
                    s.unset_env.push(k.to_owned());
                }
                "-e" | "--env" => {
                    let kv = next(&mut i, flag, inline)?;
                    let Some((k, v)) = kv.split_once('=') else {
                        return Err(format!("--env expects KEY=VALUE, got `{kv}`"));
                    };
                    validate_env_name("--env", k)?;
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
                    } else if s.cli == Some(AgentCli::Codex) && args.get(i + 1).is_some_and(|n| matches!(n.as_str(), "read-only" | "workspace-write" | "danger-full-access")) {
                        i += 1;
                        s.sandbox = Some(args[i].clone());
                    } else if s.cli == Some(AgentCli::Codex) {
                        // Codex requires an explicit sandbox mode value.
                        s.sandbox = Some(next(&mut i, flag, inline)?);
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
                    // Optional worktree name is taken ONLY via `--worktree=NAME` so `-w "fix bug"` never eats the prompt (`M6`).
                    if let Some(v) = inline
                        && !v.trim().is_empty()
                    {
                        s.worktree = Some(Some(v.to_owned()));
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

    /// Environment variable names to unset inside the child process before exec (`H15`).
    pub fn env_vars_to_unset(&self) -> Vec<String> {
        let mut vars: Vec<String> = Vec::new();
        let mut push_unique = |k: &str| {
            if !vars.iter().any(|v| v == k) {
                vars.push(k.to_owned());
            }
        };
        match self.desktop {
            DesktopPolicy::None => {
                for k in ["DISPLAY", "WAYLAND_DISPLAY", "OLO_SOCKET", "OLO_SESSION", "REX_SESSION", "SSH_AUTH_SOCK", "DBUS_SESSION_BUS_ADDRESS"] {
                    push_unique(k);
                }
            }
            DesktopPolicy::Screens { .. } => {
                // Keep REX_SESSION only so `olo screen` can find the session; server enforces screen allowlist!
                for k in ["DISPLAY", "WAYLAND_DISPLAY", "OLO_SOCKET", "OLO_SESSION", "SSH_AUTH_SOCK", "DBUS_SESSION_BUS_ADDRESS"] {
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

    /// Validates mandatory fields (`cli`, `model`, `effort`) and per-CLI flag compatibility (`M7`/`M8`),
    /// then builds the exact CLI argv.
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
        if self.session_id.is_some() && cli != AgentCli::Claude {
            return Err(format!("`--session-id` is only supported by `claude` (not `{}`)", cli.binary()));
        }
        if self.sandbox.is_some() && cli == AgentCli::Claude {
            return Err("`--sandbox` is for `codex` / `agy`; on `claude` use `--permission-mode`".into());
        }
        if self.print_mode && cli == AgentCli::Codex {
            return Err("`--print` / `--stream` are supported by `claude` and `agy` (not `codex`)".into());
        }
        if self.fork && cli == AgentCli::Agy {
            return Err("`--fork` is supported by `claude` and `codex` (not `agy`)".into());
        }
        if self.worktree.is_some() && cli == AgentCli::Agy {
            return Err("`--worktree` is supported by `claude` and `codex` (not `agy`)".into());
        }
        if cli == AgentCli::Codex
            && let Some(ref pm) = self.permission_mode
            && !matches!(pm.as_str(), "untrusted" | "on-failure" | "on-request" | "never")
        {
            return Err(format!("`--permission-mode {pm}` is a Claude mode; for `codex` use `--approval untrusted|on-failure|on-request|never`"));
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
                    // Claude Code requires `--verbose` when `--print` is paired with `--output-format stream-json` (`M7`).
                    if self.print_mode && outf == "stream-json" && !self.extra.iter().any(|a| a == "--verbose") {
                        argv.push("--verbose".into());
                    }
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
                    if !self.extra.iter().any(|a| a == "--") {
                        argv.push("--".into());
                    }
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
                // Place variadic `-i/--image` BEFORE `-m` so it can NEVER swallow `[SESSION_ID]` or `[PROMPT]` (`H12`)!
                for img in &self.images {
                    argv.extend(["-i".into(), img.clone()]);
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
                if self.no_alt_screen {
                    argv.push("--no-alt-screen".into());
                }
                if self.no_daemon {
                    argv.push("--no-daemon".into());
                }
                if self.continue_last && self.resume.is_none() {
                    argv.push("--last".into());
                }
                argv.extend(self.extra.iter().cloned());
                // Separate positional `[SESSION_ID]` / `[PROMPT]` with `--` so prompts starting with `-`/`---` never fail clap (`H12`).
                if (self.resume.is_some() || self.prompt.is_some()) && !self.extra.iter().any(|a| a == "--") {
                    argv.push("--".into());
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
                let model_low = model.to_ascii_lowercase();
                let effort_low = effort.to_ascii_lowercase();
                let suffix = format!("-{effort_low}");
                // Reject conflicting effort suffixes (e.g. `--model gemini-4-neon-xhigh --effort high` — `M8`).
                for known in ["low", "medium", "high", "xhigh", "max"] {
                    let sfx = format!("-{known}");
                    if model_low.ends_with(&sfx) && known != effort_low {
                        return Err(format!("conflicting `agy` effort: `--model {model}` specifies `{known}` while `--effort` is `{effort}`"));
                    }
                }
                argv.extend(["--model".into(), model.clone()]);
                if !model_low.ends_with(&suffix) {
                    argv.extend(["--effort".into(), effort.clone()]);
                }
                if let Some(ref rid) = self.resume {
                    argv.extend(["--resume".into(), rid.clone()]);
                } else if self.continue_last {
                    argv.push("--continue".into());
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
                // Place agy's own `--print=<prompt>` / `-i <prompt>` BEFORE `self.extra` so positional args
                // in `extra` never stop Go's `flag` parser from seeing `-i` or `--print` (`M8`)!
                if self.print_mode {
                    argv.push(format!("--print={}", self.prompt.as_deref().unwrap_or("")));
                } else if let Some(ref p) = self.prompt {
                    argv.extend(["-i".into(), p.clone()]);
                }
                argv.extend(self.extra.iter().cloned());
            }
        }
        Ok((cli, model, effort, argv))
    }

    /// Wraps `cli_argv` with `env -u K1 ... KEY=VAL ...` when environment variables must be
    /// stripped or injected so desktop isolation and `--env K=V` work identically on every
    /// Rex server version (Rex's `session.new_window` ignores `options.env`).
    pub fn wrap_with_env_isolation(&self, cli_argv: Vec<String>) -> Vec<String> {
        let unset = self.env_vars_to_unset();
        if unset.is_empty() && self.env.is_empty() {
            return cli_argv;
        }
        let mut out = Vec::with_capacity(1 + unset.len() * 2 + self.env.len() + cli_argv.len());
        out.push("env".into());
        for k in unset {
            out.push("-u".into());
            out.push(k);
        }
        for (k, v) in &self.env {
            if !k.trim().is_empty() && !k.contains('=') {
                out.push(format!("{k}={v}"));
            }
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

fn validate_env_name(flag: &str, name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('=') || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("{flag} expects a valid variable name (letters, digits, `_`), got `{name}`"));
    }
    Ok(())
}

/// Strips bracketed-paste escape sequences (`\x1b[200~`, `\x1b[201~`), raw ESC, and C0 control
/// characters other than `\t` and `\n`, and normalizes CRLF/CR to LF so pasted text can never
/// break out of bracketed paste or inject fake keys (`H18`, `C4`).
pub fn sanitize_paste_text(raw: &str) -> String {
    let cleaned = raw.replace("\r\n", "\n").replace('\r', "\n").replace("\x1b[200~", "").replace("\x1b[201~", "");
    cleaned.chars().filter(|&c| c == '\t' || c == '\n' || (!c.is_control() && c != '\x1b')).collect()
}

/// Parsed send payload: separates pasted body from trailing Enter and individual post-keys (`M18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendPayload {
    pub target: String,
    pub body: String,
    pub paste: bool,
    pub enter: bool,
    pub post_keys: Vec<String>,
    pub force: bool,
    pub as_json: bool,
    pub verify_delivery: bool,
}

/// Encodes text, `--paste`, `--file` (pre-read into `--text` + `--paste`), `--ctrl <KEY>`, `--key <NAME>`, `--force`, and `--enter`.
pub fn parse_send_payload(args: &[String], raw_obj: Option<&Value>) -> Result<SendPayload, String> {
    let mut target: Option<String> = None;
    let mut paste = false;
    let mut add_enter = false;
    let mut force = false;
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
        force = v.get("force").and_then(Value::as_bool).unwrap_or(false);
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
                "--force" => force = true,
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

    let target = target.ok_or_else(|| "usage: olo agent send <ID|NAME> [TEXT | --file PATH] [--enter] [--paste] [--force] [--ctrl c|d|z] [--key Enter|Escape|Tab|Up|Down|Left|Right]".to_owned())?;
    let body = text_parts.join(" ");
    if body.is_empty() && !add_enter && post_keys.is_empty() {
        return Err("nothing to send: provide TEXT, --file PATH, --enter, --ctrl <KEY>, or --key <NAME>".into());
    }
    if body.contains('\n') {
        paste = true;
    }
    if paste && add_enter {
        verify_delivery = true;
    }
    Ok(SendPayload { target, body, paste, enter: add_enter, post_keys, force, as_json, verify_delivery })
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

    // 6. Fallback to Rex activity — an unrecognised Blocked dialog fails VISIBLE as WaitingQuestion
    //    (unless the screen is still completely blank at startup), never hanging forever as `working` (`H2`)!
    let (state, summary) = match activity {
        crate::agent::AgentActivity::Working => (SubagentLiveState::Working, last_nonempty()),
        crate::agent::AgentActivity::Blocked => {
            if screen_text.trim().is_empty() {
                (SubagentLiveState::Working, Some("starting".into()))
            } else {
                (SubagentLiveState::WaitingQuestion, Some("needs input (unrecognised dialog)".into()))
            }
        }
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

/// Shared state reconciliation across `status`, `list`, `watch`, and `record_subagent_transition` (`H10`, `M2`, `M3`, `M22`).
/// Returns `(effective_state, effective_error, aborted)`.
pub fn effective_state(cls: &ScreenClassification, tx_state: Option<&str>, tx_error: Option<&str>, alive: bool) -> (SubagentLiveState, Option<String>, bool) {
    if !alive || cls.state == SubagentLiveState::Exited {
        return (SubagentLiveState::Exited, None, false);
    }
    // Screen-level API error takes precedence (except over an active approval dialog).
    if let Some(ref err) = cls.api_error
        && cls.state != SubagentLiveState::WaitingApproval
    {
        return (SubagentLiveState::Error, Some(err.clone()), false);
    }
    // Transcript error is reported ONLY when the screen is NOT actively Working (retries!) or WaitingApproval (`M2`).
    if let Some(err) = tx_error
        && !matches!(cls.state, SubagentLiveState::Working | SubagentLiveState::WaitingApproval)
    {
        return (SubagentLiveState::Error, Some(err.to_owned()), false);
    }
    if cls.state == SubagentLiveState::WaitingApproval || cls.state == SubagentLiveState::WaitingQuestion {
        return (cls.state, None, false);
    }
    // Transcript fallback for interactive questions (`M3`).
    if tx_state == Some("waiting") && matches!(cls.state, SubagentLiveState::Idle | SubagentLiveState::Working) {
        return (SubagentLiveState::WaitingQuestion, None, false);
    }
    // Aborted turn reads as Idle with `aborted: true`, NOT `done` (`M3`).
    if tx_state == Some("aborted") && cls.state == SubagentLiveState::Idle {
        return (SubagentLiveState::Idle, None, true);
    }
    if matches!(tx_state, Some("done" | "end_turn")) && cls.state == SubagentLiveState::Idle {
        return (SubagentLiveState::Done, None, false);
    }
    (cls.state, None, false)
}

fn norm_cmd(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

/// Merges screen-parsed approval prompt with uncompleted transcript tool calls ONLY when a transcript
/// call actually agrees with what is shown on screen (`H14`). On parallel tool calls where another
/// uncompleted tool does not match the screen, keeps the screen's command/tool and exposes candidates.
pub fn approval_to_json(ap: &olo_agent_screen::ApprovalPrompt, pending_tools: &[Value]) -> Value {
    let screen_cmd_norm = ap.command.as_deref().map(norm_cmd).unwrap_or_default();
    let screen_tool_norm = ap.tool.as_deref().map(|s| s.to_ascii_lowercase());

    let matched: Option<&Value> = if pending_tools.is_empty() {
        None
    } else if !screen_cmd_norm.is_empty() {
        pending_tools.iter().find(|t| {
            t.get("command").and_then(Value::as_str).is_some_and(|c| {
                let cn = norm_cmd(c);
                !cn.is_empty()
                    && (cn.contains(&screen_cmd_norm) || screen_cmd_norm.contains(&cn) || {
                        let prefix_len = screen_cmd_norm.len().min(cn.len()).min(24);
                        prefix_len >= 6 && cn.as_bytes()[..prefix_len] == screen_cmd_norm.as_bytes()[..prefix_len]
                    })
            })
        })
    } else if pending_tools.len() == 1 {
        let only = &pending_tools[0];
        let tool_ok = screen_tool_norm.as_deref().is_none_or(|st| {
            only.get("tool").and_then(Value::as_str).is_some_and(|tt| {
                let ttl = tt.to_ascii_lowercase();
                ttl == st || (st == "bash" && matches!(ttl.as_str(), "bash" | "run_command" | "exec_command"))
            })
        });
        tool_ok.then_some(only)
    } else {
        None
    };

    let tx_tool = matched.and_then(|t| t.get("tool")).and_then(Value::as_str);
    let tx_cmd = matched.and_then(|t| t.get("command")).and_then(Value::as_str);
    let tx_args = matched.and_then(|t| t.get("arguments")).cloned();

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
    let mut out = json!({
        "question": ap.question,
        "tool": tool,
        "command": command,
        "screen_command": ap.command,
        "arguments": tx_args,
        "folded_lines": ap.folded_lines,
        "selected_index": ap.selected_index,
        "choices": choices,
    });
    if matched.is_none() && !pending_tools.is_empty() {
        out["transcript_candidates"] = Value::Array(pending_tools.to_vec());
    }
    out
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
            if let Some(sv) = v.get("since")
                && !sv.is_null()
            {
                since = Some(sv.as_u64().ok_or_else(|| format!("invalid `since` cursor `{sv}`; expected unsigned integer"))?);
            }
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
            if let Some(tv) = v.get("timeout")
                && !tv.is_null()
            {
                timeout_secs = tv.as_u64().ok_or_else(|| format!("invalid `timeout` `{tv}`; expected unsigned integer seconds"))?;
            }
            json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
        }

        if !all && targets.is_empty() {
            return Err("usage: olo agent watch [<ID|NAME>... | --all] [--since CURSOR] [--on question|approval|idle|done|error|exit|any] [--match TEXT] [--regex PAT] [--timeout SEC]".to_owned());
        }
        // Validate every `--on` event name so typos fail loudly instead of hanging forever (`L6`).
        for ev in &events {
            if !matches!(ev.as_str(), "any" | "question" | "waiting_question" | "approval" | "waiting_approval" | "waiting" | "idle" | "done" | "error" | "failed" | "exit" | "exited" | "closed") {
                return Err(format!("unknown --on event `{ev}`; expected question|approval|waiting|idle|done|error|exit|any"));
            }
        }
        if events.is_empty() && match_text.is_none() && regex_pat.is_none() {
            events = vec!["question".into(), "approval".into(), "idle".into(), "done".into(), "error".into(), "exit".into()];
        }
        if timeout_secs == 0 || timeout_secs > 3600 {
            return Err(format!("--timeout must be between 1 and 3600 seconds (got {timeout_secs})"));
        }
        Ok(Self { targets, all, parent, since, events, match_text, regex_pat, timeout_secs, json })
    }

    pub fn matches_state(&self, state: SubagentLiveState) -> bool {
        self.events.iter().any(|ev| match ev.as_str() {
            "any" => !matches!(state, SubagentLiveState::Working),
            "question" | "waiting_question" => state == SubagentLiveState::WaitingQuestion,
            "approval" | "waiting_approval" => state == SubagentLiveState::WaitingApproval,
            "waiting" => matches!(state, SubagentLiveState::WaitingQuestion | SubagentLiveState::WaitingApproval),
            // `idle` matches both `Idle` and `Done` so `--on idle` never misses normal completions (`H10`);
            // `--on done` matches strictly `Done`.
            "idle" => matches!(state, SubagentLiveState::Idle | SubagentLiveState::Done),
            "done" => state == SubagentLiveState::Done,
            "error" | "failed" => state == SubagentLiveState::Error,
            "exit" | "exited" | "closed" => state == SubagentLiveState::Exited,
            _ => false,
        })
    }

    /// Checks whether any newly arrived screen line matches `--match` or `--regex` (`H11`).
    pub fn check_new_lines(&self, new_lines: &[String], compiled_re: Option<&regex::Regex>) -> Option<(String, Option<String>)> {
        if let Some(needle) = &self.match_text
            && !needle.is_empty()
            && let Some(line) = new_lines.iter().rev().find(|l| l.contains(needle.as_str()))
        {
            return Some(("match".into(), Some(line.trim().to_owned())));
        }
        if let Some(re) = compiled_re
            && let Some(line) = new_lines.iter().rev().find(|l| re.is_match(l))
        {
            return Some(("regex".into(), Some(line.trim().to_owned())));
        }
        None
    }
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
        "started_at": to_unix_ms(entry.started_at),
        "exited_at": entry.exited_at.map(to_unix_ms),
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

        // Agy stream mode + accept-edits + settings (`--print=` string flag, before extra!)
        let agy = StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon", "--effort", "xhigh", "--mode", "accept-edits", "--settings", "{\"allow\":[]}", "--stream"])).unwrap();
        let (_, _, _, a_argv) = agy.build_command().unwrap();
        assert!(a_argv.contains(&"--print=".to_owned()), "agy argv: {a_argv:?}");
        assert!(a_argv.windows(2).any(|w| w == ["--input-format", "stream-json"]));
        assert!(a_argv.windows(2).any(|w| w == ["--output-format", "stream-json"]));
        assert!(a_argv.windows(2).any(|w| w == ["--mode", "accept-edits"]));
        assert!(a_argv.windows(2).any(|w| w == ["--settings", "{\"allow\":[]}"]));

        // Golden argv checks (`H12`, `M6`, `M7`, `M8`):
        // 1. Codex `-i` placed before `-m`, and `--` before prompt starting with `-` / `---`.
        let cx = StartSpec::parse_cli_args(&args(&[
            "codex",
            "--model",
            "gpt-5.4-codex",
            "--effort",
            "high",
            "--image",
            "shot.png",
            "--resume",
            "11111111-2222-3333-4444-555555555555",
            "--prompt",
            "---\nfront matter\n---",
        ]))
        .unwrap();
        let (_, _, _, cx_argv) = cx.build_command().unwrap();
        let img_pos = cx_argv.iter().position(|x| x == "-i").unwrap();
        let mod_pos = cx_argv.iter().position(|x| x == "-m").unwrap();
        let sep_pos = cx_argv.iter().position(|x| x == "--").unwrap();
        assert!(img_pos < mod_pos && mod_pos < sep_pos, "codex argv ordering wrong: {cx_argv:?}");
        assert_eq!(cx_argv[sep_pos + 1], "11111111-2222-3333-4444-555555555555");
        assert_eq!(cx_argv[sep_pos + 2], "---\nfront matter\n---");

        // 2. Bare `-w` does NOT eat the positional prompt (`M6`), while `--worktree=feat` sets name.
        let wt1 = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "high", "-w", "fix the parser"])).unwrap();
        assert_eq!(wt1.worktree, Some(None));
        assert_eq!(wt1.prompt.as_deref(), Some("fix the parser"));
        let wt2 = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "high", "--worktree=feat-1", "fix the parser"])).unwrap();
        assert_eq!(wt2.worktree, Some(Some("feat-1".into())));

        // 3. Per-CLI unsupported flags rejected (`M7`/`M8`) & Claude `--stream` adds `--verbose`.
        assert!(StartSpec::parse_cli_args(&args(&["codex", "--model", "m", "--effort", "high", "--session-id", "abc"])).unwrap().build_command().is_err());
        assert!(StartSpec::parse_cli_args(&args(&["codex", "--model", "m", "--effort", "high", "--permission-mode", "acceptEdits"])).unwrap().build_command().is_err());
        assert!(StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon-xhigh", "--effort", "high"])).unwrap().build_command().is_err());
        let cl_stream = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "high", "--stream", "--prompt", "hi"])).unwrap();
        let (_, _, _, cl_s_argv) = cl_stream.build_command().unwrap();
        assert!(cl_s_argv.contains(&"--verbose".to_owned()), "claude --stream must include --verbose: {cl_s_argv:?}");
    }

    #[test]
    fn desktop_policy_enforces_none_and_scoped_screens() {
        let none = DesktopPolicy::None;
        assert!(none.check_action("olo.op", None).is_err());
        assert!(none.check_action("olo.screen", Some(&json!({"args": ["start", "qa-1"]}))).is_err());
        assert!(none.check_action("olo.open", None).is_err());
        assert!(none.check_action("olo.clipboard.get", None).is_err());
        // Catch-all & non-allowlisted actions MUST be denied (`C1`)!
        assert!(none.check_action("agent.auto_approve_toggle", None).is_err());
        assert!(none.check_action("olo.passwords.fill", None).is_err());
        assert!(none.check_action("olo.install_remote_skill", None).is_err());
        assert!(none.check_action("olo.agent.list", None).is_ok());
        assert!(none.check_action("olo.wait", None).is_ok());
        assert!(none.check_action("olo.file", None).is_ok());
        // Privilege attenuation (`C2`): none cannot escalate to screens or inherit!
        assert!(none.restrict_to(&DesktopPolicy::Inherit).is_err());
        assert!(none.restrict_to(&DesktopPolicy::parse("screens=qa-1").unwrap()).is_err());
        assert!(none.restrict_to(&DesktopPolicy::None).is_ok());

        let scoped = DesktopPolicy::parse("screens=qa-1,qa-2").unwrap();
        assert!(scoped.check_action("olo.op", None).is_err());
        assert!(scoped.check_action("olo.open", None).is_err());
        assert!(scoped.check_action("agent.auto_approve_toggle", None).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["look", "qa-1", "/var/tmp/x.png"]}))).is_ok());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["tree", "qa-2"]}))).is_ok());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["tree", "other-screen"]}))).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["--owner", "qa-1", "look", "other"]}))).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["list"]}))).is_err());
        assert!(scoped.restrict_to(&DesktopPolicy::parse("screens=qa-1").unwrap()).is_ok());
        assert!(scoped.restrict_to(&DesktopPolicy::parse("screens=qa-1,qa-3").unwrap()).is_err());
        assert!(scoped.restrict_to(&DesktopPolicy::Inherit).is_err());
    }

    #[test]
    fn classifies_screens_without_false_approval_events() {
        // Bare AgentActivity::Blocked with empty screen stays Working (startup frame), while non-empty
        // unrecognised Blocked dialog fails visible as WaitingQuestion (`H2`), never WaitingApproval!
        let blank = classify_screen_rich(AgentCli::Claude, crate::agent::AgentActivity::Blocked, "", true);
        assert_eq!(blank.state, SubagentLiveState::Working);
        assert!(blank.approval.is_none());
        let unknown_dlg = classify_screen_rich(AgentCli::Claude, crate::agent::AgentActivity::Blocked, "Some brand-new prompt widget", true);
        assert_eq!(unknown_dlg.state, SubagentLiveState::WaitingQuestion);

        // Real Agy approval prompt emits WaitingApproval with structured choices
        let agy_ask = "Command\n────────────────────\nRequesting permission for:\n   rm -f /var/tmp/x\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation\n  3. No, cancel\n  ↑/↓ Navigate\n";
        let ap = classify_screen_rich(AgentCli::Agy, crate::agent::AgentActivity::Idle, agy_ask, true);
        assert_eq!(ap.state, SubagentLiveState::WaitingApproval);
        assert!(ap.approval.is_some());

        // H14: parallel pending tool calls MUST NOT overwrite screen command with a different longer tool!
        let ap_prompt = ap.approval.as_ref().unwrap();
        let pending = vec![json!({"tool": "Bash", "command": "git status --porcelain=v2 --untracked-files=all"}), json!({"tool": "Bash", "command": "rm -f /var/tmp/x"})];
        let ap_j = approval_to_json(ap_prompt, &pending);
        assert_eq!(ap_j["command"], json!("rm -f /var/tmp/x"));
        let mismatch_only = vec![json!({"tool": "Bash", "command": "git status --porcelain=v2 --untracked-files=all"})];
        let ap_m = approval_to_json(ap_prompt, &mismatch_only);
        assert_eq!(ap_m["command"], json!("rm -f /var/tmp/x"));
        assert!(ap_m["transcript_candidates"].as_array().is_some_and(|a| a.len() == 1));

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

        // H18: sanitize_paste_text strips bracketed paste escapes & raw ESC/C0 while keeping tabs/newlines!
        assert_eq!(sanitize_paste_text("hi\x1b[201~\r\n/exit\x1b[200~\t\x03ok"), "hi\n/exit\tok");
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
            started_at: now_unix_ms() as i64 - 10_000,
            exited_at: None,
            exit_code: None,
            last_pid: Some(999),
            created_rev: 10,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: Some("record".into()),
            add_dirs: Vec::new(),
        };
        reg.register(mk("sa-1", "builder-a", "w2", "w1", "11111111-2222-3333-4444-555555555555"));
        let live: HashSet<String> = ["w1", "w2"].into_iter().map(str::to_owned).collect();
        assert!(reg.is_live_name_taken("local", "s1", "w1", "builder-a", &live));
        assert_eq!(reg.resolve_target("local", Some("s1"), "builder-a").unwrap().id, "sa-1");
        assert_eq!(reg.resolve_target("local", Some("s1"), "11111111-2222-3333-4444-555555555555").unwrap().id, "sa-1");

        // C3: child `w2` CANNOT resolve itself!
        assert!(reg.resolve_target_scoped("local", Some("s1"), Some("w2"), true, "builder-a").is_err());
        assert!(reg.resolve_target_scoped("local", Some("s1"), Some("w2"), true, "sa-1").is_err());

        // H1: older view revision (9 < created_rev 10) MUST NOT mark `w2` exited!
        let empty: HashSet<String> = ["w1"].into_iter().map(str::to_owned).collect();
        let (changed_old, exited_old) = reg.sync_live_windows("local", "s1", &empty, 9);
        assert!(!changed_old && exited_old.is_empty());

        // Newer view revision (11 >= 10) marks it exited with session ID intact; reappearing heals it!
        let (changed, exited) = reg.sync_live_windows("local", "s1", &empty, 11);
        assert!(changed);
        assert_eq!(exited.len(), 1);
        assert!(!reg.is_live_name_taken("local", "s1", "w1", "builder-a", &empty));
        let found = reg.resolve_target("local", Some("s1"), "builder-a").unwrap();
        assert!(found.exited_at.is_some());
        assert_eq!(found.cli_session_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
        // Healing when window reappears in newer view (`H1`):
        let (healed, _) = reg.sync_live_windows("local", "s1", &live, 12);
        assert!(healed);
        assert!(reg.by_window("local", "s1", "w2").is_some());
    }

    #[test]
    fn event_log_cursors_and_watch_multi_parse() {
        let mut log = SubagentEventLog::default();
        let mk_ev = |id: &str, state: SubagentLiveState, dk: &str| SubagentEvent {
            seq: 0,
            timestamp_ms: 1000,
            host_key: "local".into(),
            session_id: "s1".into(),
            parent_window_id: "w1".into(),
            ancestor_windows: vec!["w1".into()],
            id: id.into(),
            name: Some(format!("n-{id}")),
            window_id: format!("win-{id}"),
            block_id: format!("blk-{id}"),
            resume_id: None,
            cli_session_id: None,
            event: state.as_str().into(),
            state,
            summary: None,
            last_message: Some("DONE".into()),
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key: dk.into(),
        };
        let c1 = log.push(mk_ev("sa-1", SubagentLiveState::Done, "msg:1"));
        // Duplicate push for same (id, state, key) is dropped (`H7`), while new key advances (`H5`)!
        assert_eq!(log.push(mk_ev("sa-1", SubagentLiveState::Done, "msg:1")), c1);
        let c2 = log.push(mk_ev("sa-2", SubagentLiveState::WaitingApproval, "ap:100"));
        let c3 = log.push(mk_ev("sa-2", SubagentLiveState::WaitingApproval, "ap:200"));
        assert_eq!((c1, c2, c3), (1, 2, 3));
        assert_eq!(log.find_since(0, |_| true).unwrap().id, "sa-1");
        assert_eq!(log.find_since(c1, |_| true).unwrap().id, "sa-2");
        assert_eq!(log.find_since(c2, |_| true).unwrap().seq, 3);
        assert!(log.find_since(c3, |_| true).is_none());

        let ws = WatchSpec::parse(Some(&json!({"args": ["--all", "--since", "42", "--on", "done,approval", "--json"]}))).unwrap();
        assert!(ws.all);
        assert_eq!(ws.since, Some(42));
        assert!(ws.matches_state(SubagentLiveState::Done));
        assert!(ws.matches_state(SubagentLiveState::WaitingApproval));
        assert!(!ws.matches_state(SubagentLiveState::Working));
        // H10: `--on idle` matches BOTH Idle and Done!
        let ws_idle = WatchSpec::parse(Some(&json!({"args": ["--all", "--on", "idle"]}))).unwrap();
        assert!(ws_idle.matches_state(SubagentLiveState::Idle));
        assert!(ws_idle.matches_state(SubagentLiveState::Done));
        // L6 / M24: unknown `--on` and non-numeric JSON `since`/`timeout` rejected!
        assert!(WatchSpec::parse(Some(&json!({"args": ["--all", "--on", "typo"]}))).is_err());
        assert!(WatchSpec::parse(Some(&json!({"all": true, "since": "42"}))).is_err());
        assert!(WatchSpec::parse(Some(&json!({"all": true, "timeout": "60"}))).is_err());

        let ap = ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--json"]}))).unwrap();
        assert_eq!((ap.target.as_str(), ap.once, ap.json), ("builder-a", true, true));
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a"]}))).is_err());
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--no"]}))).is_err());
    }
}
