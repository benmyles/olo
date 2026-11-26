//! The children of a Rex tab: who they are, whose child each is, and
//! the tree they make. One definition for the host's agents service,
//! which owns them (`olo-rex-link agent`), and the Olo desktop, which
//! shows what the service says.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported first-class agent CLIs for native Rex child-tab subagents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentCli {
    Claude,
    Codex,
    Agy,
    /// Not an agent: a command of the caller's own in a child tab
    /// (`olo agent start --command -- CMD…`), running or exited and
    /// nothing else read of its screen.
    Command,
}

impl AgentCli {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" | "antigravity" => Some(Self::Agy),
            "command" | "cmd" => Some(Self::Command),
            _ => None,
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
            Self::Command => "command",
        }
    }

    /// Whether this child is one of the agents (its screen and
    /// transcript read), not a plain command.
    pub fn is_agent(self) -> bool {
        !matches!(self, Self::Command)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Agy => "Antigravity",
            Self::Command => "Command",
        }
    }

    /// Lowercase product label for window chrome (sidebar, popover, header);
    /// uses `"antigravity"` instead of the CLI shorthand `"agy"`.
    pub fn chrome_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "antigravity",
            Self::Command => "command",
        }
    }

    /// The agent whose screens `olo_agent_screen` reads for this child;
    /// none for a plain command.
    pub fn screen_agent(self) -> Option<olo_agent_screen::Agent> {
        match self {
            Self::Claude => Some(olo_agent_screen::Agent::Claude),
            Self::Codex => Some(olo_agent_screen::Agent::Codex),
            Self::Agy => Some(olo_agent_screen::Agent::Antigravity),
            Self::Command => None,
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
    /// What a command child runs (`start --command -- CMD…`): its
    /// program and arguments. Empty for an agent, whose own command
    /// line may hold a prompt and is nobody's to show.
    #[serde(default)]
    pub command: Vec<String>,
    /// The agent this one is the minder of, by its id (`agents::minder`):
    /// it reaches that agent and no other, and what that agent asks is
    /// its to answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minds: Option<String>,
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

    /// What it is, in a few words for the line under its name: its
    /// agent, model and effort ("claude · claude-opus-5-5 · max"); a
    /// command of the asker's own is "command" and what it runs
    /// ("command · tail -f build.log": it has neither).
    pub fn what(&self) -> String {
        if self.cli.is_agent() {
            format!("{} · {} · {}", self.cli.chrome_name(), self.model, self.effort)
        } else if self.command.is_empty() {
            self.cli.chrome_name().to_owned()
        } else {
            // (As it was typed, on one line.)
            format!("{} · {}", self.cli.chrome_name(), self.command.join(" ").split_whitespace().collect::<Vec<_>>().join(" "))
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
    /// What a service reads as it starts: the registry at `path`, an
    /// empty one where there is none yet, and an error where one is
    /// there and can't be read (damaged, or another version's): never
    /// an empty one in its place, which would say of every child that
    /// it isn't (ShiftUp's lead, 2026-11-26: "An empty list where there
    /// were children must not be an answer a parent can get").
    pub fn read_from(path: &Path) -> Result<Self, String> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("the registry at {} can't be read: {e}", path.display())),
        };
        serde_json::from_str::<Self>(&raw).map_err(|e| format!("the registry at {} can't be read: {e} (it is left as it is; nothing is listed from another)", path.display()))
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
                eprintln!("olo agents: the registry at {} couldn't be read ({e}); kept aside as {}", path.display(), bad.display());
                Self::default()
            }
        }
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

    /// The minder of the agent `id`, if it has one that runs.
    pub fn minder_of(&self, id: &str) -> Option<&NativeSubagent> {
        self.entries.iter().rev().find(|e| e.exited_at.is_none() && e.minds.as_deref() == Some(id))
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

pub fn now_unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A child's handle: `sa-` and six hex digits, of chance (the kernel's,
/// where it can be read; the clock's and this process's otherwise).
pub fn new_subagent_id() -> String {
    use std::io::Read as _;
    let mut raw = [0u8; 3];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut raw)).is_err() {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) as u64;
        let mixed = (t ^ (u64::from(std::process::id()) << 32)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        raw = [(mixed >> 40) as u8, (mixed >> 48) as u8, (mixed >> 56) as u8];
    }
    format!("sa-{:02x}{:02x}{:02x}", raw[0], raw[1], raw[2])
}
