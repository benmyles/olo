//! What `olo agent`'s commands are asked, read from their arguments
//! (as typed, or as `rex do`'s structured form), and the command line
//! each kind of child is started with.

use serde_json::Value;

use super::model::{AgentCli, DesktopPolicy};
use super::state::SubagentLiveState;

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
    /// The agent's own automatic mode, where the parent says (`--auto`,
    /// `--no-auto`); unsaid, it is on unless the parent chose how leave
    /// is given another way ([`StartSpec::auto_for`]).
    pub auto: Option<bool>,
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
    /// `--command`: not an agent, the command after `--` as it is
    /// (`AgentCli::Command`; no model, no effort).
    pub command: bool,
    /// `--minder CLI:MODEL:EFFORT` or `--minder NAME`: a shared minder
    /// (`agents::minder`), with the parent's rules for this agent
    /// (`--rules FILE` or `--rules-text TEXT`) and what a newly started
    /// minder's environment has besides the child's (`--minder-env K=V`).
    pub minder: Option<super::minder::MinderChoice>,
    pub rules: Option<String>,
    pub rules_file: Option<String>,
    pub minder_env: Vec<(String, String)>,
    /// Set by the service alone, never asked for: pre-allocated handle
    /// (used when starting a shared minder whose `mind` script path
    /// names its own id).
    pub id: Option<String>,
    /// Set by the service alone, never asked for: the agent this one is
    /// the minder of (its id — older 1:1 kind).
    pub minds: Option<String>,
    /// Set by the service alone, never asked for: the shared minder
    /// that minds this agent (its id).
    pub minded_by: Option<String>,
    /// Set by the service alone (`start_shared_minder`), never asked for:
    /// marks this child as a service-owned shared minder.
    pub shared_minder: bool,
    /// `--approve-all-but FILE` (its text, read by the asking command):
    /// leave given by a list, by the service itself (`agentd/leave.rs`).
    pub approve_all_but: Option<String>,
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
        let role = str_opt("role");
        if role.as_deref().is_some_and(|r| r.eq_ignore_ascii_case("minder")) {
            return Err("`--role minder` is reserved for minders started by the service (`--minder claude:MODEL:EFFORT`)".into());
        }
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
            role,
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
            auto: v.get("auto").and_then(Value::as_bool),
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
            extra: str_vec("extra").into_iter().chain(str_vec("command")).collect(),
            command: cli == Some(AgentCli::Command) || matches!(v.get("command"), Some(Value::Array(_))),
            minder: match str_opt("minder") {
                Some(m) => Some(super::minder::MinderChoice::parse(&m)?),
                None => None,
            },
            rules: raw_str_opt("rules_text"),
            approve_all_but: raw_str_opt("approve_all_but"),
            rules_file: str_opt("rules"),
            minder_env: match v.get("minder_env").and_then(Value::as_object) {
                Some(obj) => obj.iter().map(|(k, val)| (k.clone(), val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string()))).collect(),
                None => Vec::new(),
            },
            id: None,
            minds: None,
            minded_by: None,
            shared_minder: false,
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
                "--role" => {
                    let r = next(&mut i, flag, inline)?;
                    if r.trim().eq_ignore_ascii_case("minder") {
                        return Err("`--role minder` is reserved for minders started by the service (`--minder claude:MODEL:EFFORT`)".into());
                    }
                    s.role = Some(r);
                }
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
                "--minder" => s.minder = Some(super::minder::MinderChoice::parse(&next(&mut i, flag, inline)?)?),
                "--rules" | "--rules-file" => s.rules_file = Some(next(&mut i, flag, inline)?),
                "--rules-text" => s.rules = Some(next(&mut i, flag, inline)?),
                "--approve-all-but-text" => s.approve_all_but = Some(next(&mut i, flag, inline)?),
                "--minder-env" => {
                    let kv = next(&mut i, flag, inline)?;
                    let Some((k, v)) = kv.split_once('=') else {
                        return Err(format!("--minder-env expects KEY=VALUE, got `{kv}`"));
                    };
                    validate_env_name("--minder-env", k)?;
                    s.minder_env.push((k.to_owned(), v.to_owned()));
                }
                "--command" => {
                    s.command = true;
                    s.cli = Some(AgentCli::Command);
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
                "--auto" => s.auto = Some(true),
                "--no-auto" => s.auto = Some(false),
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
                        s.command = parsed == AgentCli::Command;
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
    /// Whether a child is started in its agent's own automatic mode
    /// (Ben, 2026-11-27: "enable auto mode by default for the olo rex
    /// agents but provide a way for the parent/launching agent to
    /// disable it if they prefer"): Claude Code's `--permission-mode
    /// auto`, Antigravity's `--mode auto`, Codex's `--approve-for-me`
    /// (its approvals reviewed automatically, in its workspace
    /// sandbox). On unless the parent says otherwise: `--no-auto`, or
    /// any word of its own about how leave is given (a permission
    /// mode, a mode, an approval policy, a sandbox, skipping
    /// permissions), which is then all that is passed. `--auto` asks
    /// for it by name.
    pub fn auto_for(&self, cli: AgentCli) -> bool {
        if !cli.is_agent() {
            return false;
        }
        if let Some(said) = self.auto {
            return said;
        }
        let chosen = self.permission_mode.is_some() || self.mode.is_some() || self.dangerously_skip_permissions || self.allow_dangerously_skip_permissions;
        match cli {
            AgentCli::Codex => !(chosen || self.approval.is_some() || self.sandbox.is_some() || self.full_auto || self.approve_for_me),
            _ => !chosen,
        }
    }

    pub fn build_command(&self) -> Result<(AgentCli, String, String, Vec<String>), String> {
        let cli = self.cli.ok_or_else(|| {
            "missing agent CLI: specify `claude`, `codex`, or `agy` (e.g. `olo agent start claude --model ... --effort ...`), or `--command -- CMD…` for a command of your own".to_owned()
        })?;
        if cli == AgentCli::Command {
            return self.build_plain_command();
        }
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
                if let Some(pm) = self.permission_mode.as_ref().or(self.mode.as_ref()) {
                    argv.extend(["--permission-mode".into(), pm.clone()]);
                } else if self.auto_for(cli) {
                    argv.extend(["--permission-mode".into(), "auto".into()]);
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
                if let Some(n) = self.name.as_ref().or(self.title.as_ref()) {
                    argv.extend(["--name".into(), n.clone()]);
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
                if let Some(a) = self.approval.as_ref().or(self.permission_mode.as_ref()) {
                    argv.extend(["-a".into(), a.clone()]);
                } else if self.full_auto {
                    argv.extend(["-a".into(), "on-request".into()]);
                }
                if let Some(ref sb) = self.sandbox {
                    argv.extend(["-s".into(), sb.clone()]);
                } else if self.full_auto {
                    argv.extend(["-s".into(), "workspace-write".into()]);
                }
                if self.approve_for_me || self.auto_for(cli) {
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
                if let Some(m) = self.mode.as_ref().or(self.permission_mode.as_ref()) {
                    argv.extend(["--mode".into(), m.clone()]);
                } else if self.auto_for(cli) {
                    argv.extend(["--mode".into(), "auto".into()]);
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
            // (Answered above, before a model is asked for.)
            AgentCli::Command => return self.build_plain_command(),
        }
        Ok((cli, model, effort, argv))
    }

    /// `start --command -- CMD ARG…`: the command as given, nothing
    /// added. What only an agent takes is refused, so a typo for an
    /// agent's flag never starts a command that ignores it.
    fn build_plain_command(&self) -> Result<(AgentCli, String, String, Vec<String>), String> {
        if self.extra.is_empty() {
            return Err("`--command` runs the command after `--` (e.g. `olo agent start --command --name logs -- tail -f build.log`)".into());
        }
        let agents_only: [(&str, bool); 12] = [
            ("--model", self.model.is_some()),
            ("--effort", self.effort.is_some()),
            ("--resume", self.resume.is_some()),
            ("--continue", self.continue_last),
            ("--fork", self.fork),
            ("--session-id", self.session_id.is_some()),
            ("--prompt", self.prompt.is_some()),
            ("--trust", self.trust),
            ("--permission-mode", self.permission_mode.is_some()),
            ("--mode", self.mode.is_some()),
            ("--settings", self.settings.is_some()),
            ("--print/--stream", self.print_mode),
        ];
        if let Some((flag, _)) = agents_only.iter().find(|(_, given)| *given) {
            return Err(format!("`{flag}` is an agent's option; `--command` runs the command after `--` as it is"));
        }
        Ok((AgentCli::Command, String::new(), String::new(), self.extra.clone()))
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
        } else if cli == AgentCli::Command {
            self.extra.first().and_then(|c| c.rsplit('/').next()).filter(|c| !c.is_empty()).unwrap_or("command").to_owned()
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
    /// For each of `post_keys`: the key as the terminal's own encoder is
    /// asked to press it, where its bytes can't say it ([`pressed_key`]).
    pub post_presses: Vec<Option<Pressed>>,
    /// Raw `--key` names as given (for minder argument validation).
    pub raw_keys: Vec<String>,
    /// True if any `--ctrl` or control/Alt chord key was requested.
    pub has_ctrl: bool,
    /// True if `--wait` was requested (client waits for the turn; refused to minders).
    pub wait: bool,
    /// One of the keys types a character ([`key_types`]): into a
    /// question it is an answer, refused without `force` as text is.
    pub types: bool,
    pub force: bool,
    pub as_json: bool,
    pub verify_delivery: bool,
}

/// Whether `--key RAW` is one of the plain chooser navigation keys a
/// minder may send (no Ctrl, no Alt, no printable characters, no F-keys).
pub fn is_minder_chooser_key(raw: &str) -> bool {
    let given = raw.trim();
    if given.is_empty() || given.starts_with('^') {
        return false;
    }
    let (alt, ctrl, shift, key) = key_parts(given);
    if alt || ctrl {
        return false;
    }
    match key.to_ascii_lowercase().as_str() {
        "tab" => true,
        "shift-tab" | "btab" | "backtab" => true,
        "enter" | "return" | "cr" | "esc" | "escape" | "space" | "backspace" | "bs" | "up" | "down" | "left" | "right" => !shift,
        _ => false,
    }
}

/// Encodes text, `--paste`, `--file` (pre-read into `--text` + `--paste`), `--ctrl <KEY>`, `--key <NAME>`, `--force`, and `--enter`.
pub fn parse_send_payload(args: &[String], raw_obj: Option<&Value>) -> Result<SendPayload, String> {
    let mut target: Option<String> = None;
    let mut paste = false;
    let mut add_enter = false;
    let mut force = false;
    let mut wait = false;
    let mut as_json = false;
    let mut verify_delivery = false;
    let mut text_parts: Vec<String> = Vec::new();
    let mut post_keys: Vec<String> = Vec::new();
    let mut post_presses: Vec<Option<Pressed>> = Vec::new();
    let mut raw_keys: Vec<String> = Vec::new();
    let mut has_ctrl = false;
    let mut types = false;

    let key_is_ctrl_or_alt = |k: &str| {
        let g = k.trim();
        if g.starts_with('^') {
            return true;
        }
        let (alt, ctrl, _, _) = key_parts(g);
        alt || ctrl
    };

    if let Some(v) = raw_obj
        && v.get("args").is_none()
    {
        target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
        paste = v.get("paste").and_then(Value::as_bool).unwrap_or(false);
        add_enter = v.get("enter").and_then(Value::as_bool).unwrap_or(false);
        force = v.get("force").and_then(Value::as_bool).unwrap_or(false);
        wait = v.get("wait").and_then(Value::as_bool).unwrap_or(false);
        as_json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
        verify_delivery = v.get("verify").and_then(Value::as_bool).unwrap_or(paste && add_enter);
        if let Some(t) = v.get("text").and_then(Value::as_str) {
            text_parts.push(t.to_owned());
        }
        if let Some(c) = v.get("ctrl").and_then(Value::as_str) {
            has_ctrl = true;
            post_presses.push(None);
            post_keys.push(ctrl_byte(c)?);
        }
        if let Some(k) = v.get("key").and_then(Value::as_str) {
            types |= key_types(k);
            has_ctrl |= key_is_ctrl_or_alt(k);
            raw_keys.push(k.to_owned());
            post_presses.push(pressed_key(k));
            post_keys.push(special_key(k)?);
        }
        if let Some(ks) = v.get("keys").and_then(Value::as_array) {
            for k in ks {
                if let Some(ks_str) = k.as_str() {
                    types |= key_types(ks_str);
                    has_ctrl |= key_is_ctrl_or_alt(ks_str);
                    raw_keys.push(ks_str.to_owned());
                    post_presses.push(pressed_key(ks_str));
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
                "--wait" => wait = true,
                "--json" => as_json = true,
                "--ctrl" => {
                    i += 1;
                    let c = args.get(i).ok_or("--ctrl requires a key (e.g. c, d, z)")?;
                    has_ctrl = true;
                    post_presses.push(None);
                    post_keys.push(ctrl_byte(c)?);
                }
                "--key" => {
                    i += 1;
                    let k = args.get(i).ok_or("--key requires a key name (e.g. Enter, Escape, Tab, Up, Down)")?;
                    types |= key_types(k);
                    has_ctrl |= key_is_ctrl_or_alt(k);
                    raw_keys.push(k.clone());
                    post_presses.push(pressed_key(k));
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
                s if s.starts_with("--ctrl=") => {
                    has_ctrl = true;
                    post_presses.push(None);
                    post_keys.push(ctrl_byte(&s["--ctrl=".len()..])?);
                }
                s if s.starts_with("--key=") => {
                    let k = &s["--key=".len()..];
                    types |= key_types(k);
                    has_ctrl |= key_is_ctrl_or_alt(k);
                    raw_keys.push(k.to_owned());
                    post_presses.push(pressed_key(k));
                    post_keys.push(special_key(k)?);
                }
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

    let target = target.ok_or_else(|| {
        "usage: olo agent send <ID|NAME> [TEXT | --file PATH] [--enter] [--paste] [--force] [--ctrl c|d|z] [--key Enter|Escape|Tab|Up|Down|Left|Right] [--wait [--timeout SECS] [--out FILE]] | olo agent send --all-errored TEXT… [--enter] [--json] (every child of yours that stands at an error)"
            .to_owned()
    })?;
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
    Ok(SendPayload { target, body, paste, enter: add_enter, post_keys, post_presses, raw_keys, has_ctrl, wait, types, force, as_json, verify_delivery })
}

/// How a permission question is answered: by what a choice means,
/// never by where it stands in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Yes, this once (`ChoiceKind::Once`).
    Once,
    /// Yes, for this conversation or session, kept nowhere beyond it
    /// (`ChoiceKind::AllowSession`: Antigravity's "Yes, and always allow
    /// in this conversation"). Never one that writes to settings.
    Session,
    /// No, this once (`ChoiceKind::No`); never a no that is kept.
    No,
}

impl Answer {
    pub fn kind(self) -> olo_agent_screen::ChoiceKind {
        match self {
            Self::Once => olo_agent_screen::ChoiceKind::Once,
            Self::Session => olo_agent_screen::ChoiceKind::AllowSession,
            Self::No => olo_agent_screen::ChoiceKind::No,
        }
    }
}

const APPROVE_USAGE: &str = "usage: olo agent approve <ID|NAME> --once | --session | --no [--box KEY] [--reason TEXT] [--json]";

/// Parsed options for `olo agent approve <ID|NAME> --once | --session | --no [--reason TEXT] [--json]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApproveSpec {
    pub target: String,
    pub answer: Answer,
    /// The box this answers (`--box KEY`: `approval.box` of `status`
    /// or of the event it was judged from): refused where another
    /// stands by now.
    pub box_key: Option<String>,
    pub reason: Option<String>,
    pub json: bool,
}

impl ApproveSpec {
    pub fn parse(raw: Option<&Value>) -> Result<Self, String> {
        let Some(v) = raw else {
            return Err(APPROVE_USAGE.into());
        };
        let mut target: Option<String> = None;
        let (mut once, mut session, mut no) = (false, false, false);
        let mut reason: Option<String> = None;
        let mut box_key: Option<String> = None;
        let mut json = false;
        if let Some(arr) = v.get("args").and_then(Value::as_array) {
            let strs: Vec<String> = arr.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
            let mut i = 0;
            while i < strs.len() {
                match strs[i].as_str() {
                    "--once" | "--yes" => once = true,
                    "--session" | "--conversation" => session = true,
                    "--no" | "--deny" | "--cancel" => no = true,
                    "--json" => json = true,
                    "--reason" => {
                        i += 1;
                        reason = Some(strs.get(i).ok_or("--reason requires text")?.clone());
                    }
                    s if s.starts_with("--reason=") => reason = Some(s["--reason=".len()..].to_owned()),
                    "--box" => {
                        i += 1;
                        box_key = Some(strs.get(i).ok_or("--box requires the box's key (`approval.box` in `status --json`)")?.clone());
                    }
                    s if s.starts_with("--box=") => box_key = Some(s["--box=".len()..].to_owned()),
                    s if s.starts_with('-') => return Err(format!("unknown approve option `{s}` (use `--once`, `--session` or `--no`)")),
                    s if target.is_none() => target = Some(s.to_owned()),
                    s => return Err(format!("unexpected argument `{s}`")),
                }
                i += 1;
            }
        } else {
            target = v.get("target").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
            once = v.get("once").and_then(Value::as_bool).unwrap_or(false);
            session = v.get("session").and_then(Value::as_bool).unwrap_or(false);
            no = v.get("no").and_then(Value::as_bool).unwrap_or(false);
            reason = v.get("reason").and_then(Value::as_str).map(str::to_owned);
            box_key = v.get("box").and_then(Value::as_str).map(str::to_owned);
            json = v.get("json").and_then(Value::as_bool).unwrap_or(false);
        }
        let target = target.ok_or_else(|| APPROVE_USAGE.to_owned())?;
        let answer = match (once, session, no) {
            (true, false, false) => Answer::Once,
            (false, true, false) => Answer::Session,
            (false, false, true) => Answer::No,
            _ => return Err("specify exactly one of `--once`, `--session` or `--no` (never a choice that is kept in settings or switches mode)".into()),
        };
        Ok(Self { target, answer, box_key: box_key.filter(|k| !k.is_empty()), reason, json })
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

/// A key the terminal's own encoder presses: its name and modifiers as
/// the Rex server takes them (`block.event`, a key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pressed {
    pub key: &'static str,
    pub mods: String,
}

/// A key whose bytes ([`special_key`], xterm's table) can't say what was
/// asked: Enter with Ctrl or Shift (a bare Return there), Tab with Ctrl,
/// Backspace with Shift, Escape with either. Those are pressed through
/// the Rex server's encoder instead, which sends what its keyboard
/// would for what the program asked of its keys: `CSI 13;5u` for
/// Ctrl+Enter to a program on the kitty keyboard protocol (Claude Code),
/// xterm's `CSI 27;5;13~` to one that isn't. (Rex's own keyboard had the
/// same gap: Ben, 2026-11-26, "ctrl+enter doens't seem to work (at least
/// in claude code) in a rex terminal".) None: its bytes say it.
pub fn pressed_key(raw: &str) -> Option<Pressed> {
    let (alt, ctrl, shift, key) = key_parts(raw.trim());
    let key = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" | "cr" if ctrl || shift => "enter",
        "tab" if ctrl => "tab",
        "backspace" | "bs" if shift => "backspace",
        "esc" | "escape" if ctrl || shift => "escape",
        _ => return None,
    };
    let mods: Vec<&str> = [(ctrl, "ctrl"), (alt, "alt"), (shift, "shift")].into_iter().filter(|(held, _)| *held).map(|(_, name)| name).collect();
    Some(Pressed { key, mods: mods.join("+") })
}

/// The modifiers before a key's name (Alt, Ctrl, Shift) and the name.
/// (A lone `-` or `+` is the key itself; `C-`, `M-` and `S-` are
/// modifiers only with something after.)
fn key_parts(given: &str) -> (bool, bool, bool, &str) {
    let (mut alt, mut ctrl, mut shift) = (false, false, false);
    let mut key = given;
    while let Some(at) = key.char_indices().skip(1).find(|(_, c)| matches!(c, '+' | '-')).map(|(i, _)| i) {
        let (word, rest) = (&key[..at], &key[at + 1..]);
        if rest.is_empty() {
            break;
        }
        match word.to_ascii_lowercase().as_str() {
            "alt" | "meta" | "opt" | "option" | "m" | "a" => alt = true,
            "ctrl" | "control" | "c" => ctrl = true,
            "shift" | "s" => shift = true,
            _ => break,
        }
        key = rest;
    }
    (alt, ctrl, shift, key)
}

/// A key by its name, as a terminal sends it: `Enter`, `Escape`, `Tab`,
/// `Up`, `F5`, a single character; with any of Alt (`Alt+m`, `M-m`,
/// `Meta+`, `Option+`), Ctrl (`Ctrl+c`, `C-c`, `^C`) and Shift before it,
/// joined by `+` or `-` (`Ctrl+Shift+Up`, `Alt+Enter`). Alt is the
/// escape before the key (what tmux sends for `M-m`); a named key with
/// modifiers is xterm's form (`ESC [ 1 ; 5 A` for Ctrl+Up).
pub fn special_key(raw: &str) -> Result<String, String> {
    let given = raw.trim();
    if given.is_empty() {
        return Err("empty --key".into());
    }
    if let Some(rest) = given.strip_prefix('^').filter(|r| r.chars().count() == 1) {
        return ctrl_byte(rest);
    }
    let (alt, ctrl, shift, key) = key_parts(given);
    let name = key.to_ascii_lowercase();
    let unknown = || {
        format!(
            "unknown key `{raw}` (try Enter, Escape, Tab, Space, Backspace, Delete, Insert, Up, Down, Left, Right, Home, End, PageUp, PageDown, F1 to F12 or a single character, alone or with Alt+, Ctrl+ or Shift+ before it: 0, y, Alt+m, Ctrl+C, Shift+Tab, Ctrl+Shift+Up)"
        )
    };
    // xterm's number for the modifiers held: 1, and 1 for Shift, 2 for Alt, 4 for Ctrl.
    let held = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let esc = if alt { "\x1b" } else { "" };
    // Keys that are a byte: Alt is the escape before it.
    let byte = match name.as_str() {
        "enter" | "return" | "cr" => Some("\r"),
        "newline" | "lf" => Some("\n"),
        "esc" | "escape" => Some("\x1b"),
        "tab" if shift => return Ok(format!("{esc}\x1b[Z")),
        "shift-tab" | "btab" | "backtab" => return Ok(format!("{esc}\x1b[Z")),
        "tab" => Some("\t"),
        "space" if ctrl => Some("\0"),
        "space" => Some(" "),
        "backspace" | "bs" if ctrl => Some("\x08"),
        "backspace" | "bs" => Some("\x7f"),
        _ => None,
    };
    if let Some(byte) = byte {
        return Ok(format!("{esc}{byte}"));
    }
    // Keys that are a sequence: `ESC [ 1 ; held LETTER`, or `ESC [ NUMBER ; held ~`.
    let lettered = |letter: char| if held == 1 { format!("\x1b[{letter}") } else { format!("\x1b[1;{held}{letter}") };
    let numbered = |number: u8| if held == 1 { format!("\x1b[{number}~") } else { format!("\x1b[{number};{held}~") };
    let sequence = match name.as_str() {
        "up" => Some(lettered('A')),
        "down" => Some(lettered('B')),
        "right" => Some(lettered('C')),
        "left" => Some(lettered('D')),
        "home" => Some(lettered('H')),
        "end" => Some(lettered('F')),
        "insert" | "ins" => Some(numbered(2)),
        "delete" | "del" => Some(numbered(3)),
        "pageup" | "pgup" => Some(numbered(5)),
        "pagedown" | "pgdn" => Some(numbered(6)),
        _ => None,
    };
    if let Some(sequence) = sequence {
        return Ok(sequence);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
        return Ok(match n {
            1..=4 if held == 1 => format!("\x1bO{}", (b'P' + n - 1) as char),
            1..=4 => format!("\x1b[1;{held}{}", (b'P' + n - 1) as char),
            _ => numbered([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n) - 5]),
        });
    }
    // A single character: Ctrl makes it a control byte, Shift a capital.
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return Err(unknown()) };
    if ctrl {
        return Ok(format!("{esc}{}", ctrl_byte(&ch.to_string())?));
    }
    let ch = if shift { ch.to_uppercase().next().unwrap_or(ch) } else { ch };
    // By itself too: that one key pressed, and nothing after it (no
    // Return, no paste). It was refused ("a bare letter isn't a key's
    // name"), and a box that takes one key and must be given no text
    // could not be answered in a tab: Claude Code's "1 to review · 2 to
    // send · 0 to dismiss", whose only key anyone may press is its 0
    // (the ShiftUp lead's item 25, 2026-11-26). Into a question it is
    // an answer, and wants `--force` as text does ([`key_types`]).
    if ch.is_control() {
        return Err(unknown());
    }
    Ok(format!("{esc}{ch}"))
}

/// Whether `--key RAW` types a character of its own: one printable
/// character with nothing held, or Shift alone (`0`, `y`, `Shift+y`). A
/// named key, a control or an Alt chord is no character.
pub fn key_types(raw: &str) -> bool {
    let given = raw.trim();
    if given.starts_with('^') && given.chars().count() == 2 {
        return false;
    }
    let (alt, ctrl, _shift, key) = key_parts(given);
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(ch), None) if !alt && !ctrl && !ch.is_control())
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
            if !matches!(
                ev.as_str(),
                "any" | "question" | "waiting_question" | "approval" | "waiting_approval" | "waiting" | "idle" | "done" | "error" | "failed" | "exit" | "exited" | "closed" | "escalation" | "minding"
            ) {
                return Err(format!("unknown --on event `{ev}`; expected question|approval|waiting|idle|done|error|exit|escalation|minding|any"));
            }
        }
        if events.is_empty() && match_text.is_none() && regex_pat.is_none() {
            events = vec!["question".into(), "approval".into(), "idle".into(), "done".into(), "error".into(), "exit".into(), "escalation".into(), "minding".into()];
        }
        if timeout_secs == 0 || timeout_secs > 3600 {
            return Err(format!("--timeout must be between 1 and 3600 seconds (got {timeout_secs})"));
        }
        Ok(Self { targets, all, parent, since, events, match_text, regex_pat, timeout_secs, json })
    }

    /// Whether what a minder has for the parent is among what is waited
    /// for (`escalation`: so unless other events alone were named; with
    /// `any`, and with none named).
    pub fn wants_escalations(&self) -> bool {
        self.events.iter().any(|ev| matches!(ev.as_str(), "any" | "escalation"))
    }

    /// Whether minder roster/rules updates (`minding`) are among what
    /// is waited for (meaningful only for a shared minder's watch;
    /// ignored on every other watch).
    pub fn wants_minding(&self) -> bool {
        self.events.iter().any(|ev| matches!(ev.as_str(), "any" | "minding"))
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
    pub fn check_new_lines(&self, new_lines: &[String], regex_matches: Option<&dyn Fn(&str) -> bool>) -> Option<(String, Option<String>)> {
        if let Some(needle) = &self.match_text
            && !needle.is_empty()
            && let Some(line) = new_lines.iter().rev().find(|l| l.contains(needle.as_str()))
        {
            return Some(("match".into(), Some(line.trim().to_owned())));
        }
        if let Some(is_match) = regex_matches
            && let Some(line) = new_lines.iter().rev().find(|l| is_match(l))
        {
            return Some(("regex".into(), Some(line.trim().to_owned())));
        }
        None
    }
}

pub fn start_usage() -> String {
    "usage: olo agent start <claude|codex|agy> --model <MODEL> --effort <EFFORT> [--name NAME] [--trust] [--no-auto] [--desktop none|screens=NAME,…|inherit] [--unset-env K]... [--session-id UUID] [--resume ID | --continue] [--fork] [--title TITLE] [--role ROLE] [--cwd DIR] [--prompt TEXT | --prompt-file PATH] [--permission-mode MODE | --approval POLICY | --sandbox MODE | --mode MODE | --full-auto] [--settings JSON_OR_FILE] [--strict-mcp-config] [--disallowed-tools TOOLS...] [--add-dir DIR]... [--env K=V]... [--parent WIN] [--focus] [--close-on-exit] [--minder claude:MODEL:EFFORT|NAME [--rules FILE | --rules-text TEXT]] [--json] [-- EXTRA_CLI_ARGS...]\n\nA minder (--minder): an agent of its own under your tab that minds agents nested under it (one shared minder per CLI:MODEL:EFFORT up to 12 agents, or an existing minder by NAME), answering each agent by its own rules and escalating what they don't cover (`escalation` on your `olo agent watch`). For one that runs already: olo agent mind NAME --with claude:MODEL:EFFORT|NAME --rules FILE.\n\nNote: auto mode is on by default (`--no-auto` disables it). On Codex, `--approve-for-me` uses `workspace-write` and cannot be combined with `--sandbox`; pass `--sandbox workspace-write` or `--sandbox danger-full-access` with `--add-dir` when auto mode is off, or `--sandbox danger-full-access -c 'approvals_reviewer=\"auto_review\"'` to run unsandboxed with auto review. Default desktop policy for children is `--desktop none`.\n\nA command of your own in a child tab (no agent): olo agent start --command [--name NAME] [--title TITLE] [--cwd DIR] [--env K=V]... [--unset-env K]... [--desktop …] [--parent WIN] [--close-on-exit] [--focus] [--json] -- CMD [ARG…]".into()
}
