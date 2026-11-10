//! One command language in every interface. Plain text is a message; slash
//! commands are discoverable shortcuts, /api exposes every typed operation.
use crate::model::*;
use anyhow::{Context, Result, bail};

pub const HELP: &str = "Messages go to the agent. Enter sends; Escape stops.\n\n/new [workspace] · /sessions · /open ID · /status · /stop · /resume\n/model MODEL [EFFORT] · /provider codex|claude · /models\n/goal OBJECTIVE · /todos JSON · /memory TEXT · /attach PATH\n/spec on|off · /spec TEXT · /accept [REVISION]\n/keep on|off · /approval ask|auto|allow · /guidance TEXT\n/agents on|off · /agent JSON · /readonly on|off · /compact\n/config [JSON] · /defaults [JSON] · /set KEY JSON · /fork [TITLE] · /history [QUERY] · /undo EDIT · /checkpoints · /event SEQUENCE\n/git OP [JSON] · /github OP JSON · /tool NAME JSON\n/pin [on|off] · /rename TITLE · /tags WORDS · /archive · /restore\n/interrupt TEXT · /after-tool TEXT · /queue TEXT · /inbox\n/cancel-message ID · /retry-message ID\n/message [PEER/]SESSION TEXT · /peers · /connect NAME STORE · /disconnect NAME · /peer-sessions NAME\n/api JSON (every command) · /help · /quit\n\nChanges to model and configuration require a stopped turn.\nApproval: /approve CALL_ID or /deny CALL_ID.\nFull configuration includes budgets, timeouts, model provider fields,\nMCP servers, hooks, subagent limits and review model.\nAn execute or MCP tool has host access when approved.";
pub enum Input {
    Command(Command),
    Open(String),
    New(String),
    Help,
    Config,
    Quit,
}
pub fn parse(text: &str, s: &Session) -> Result<Input> {
    if !text.starts_with('/') {
        return Ok(Input::Command(Command::Send { session: s.id.clone(), text: text.into() }));
    }
    let (name, arg) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let arg = arg.trim();
    let session = s.id.clone();
    let mut config = s.config.clone();
    let command = match name {
        "/agent" => {
            let arguments: serde_json::Value = serde_json::from_str(arg).context("expected a JSON object with task and optional workspace/model settings")?;
            anyhow::ensure!(arguments.is_object(), "arguments must be a JSON object");
            Command::Tool { session, name: "spawn_agent".into(), arguments }
        }
        "/tool" => {
            let (name, args) = arg.split_once(char::is_whitespace).context("expected TOOL JSON_ARGUMENTS")?;
            Command::Tool { session, name: name.into(), arguments: serde_json::from_str(args)? }
        }
        "/git" | "/github" => {
            let (operation, args) = arg.split_once(char::is_whitespace).unwrap_or((if arg.is_empty() { "status" } else { arg }, "{}"));
            let mut arguments: serde_json::Value = serde_json::from_str(args)?;
            anyhow::ensure!(arguments.is_object(), "arguments must be a JSON object");
            arguments["operation"] = operation.into();
            let read = if name == "/git" {
                matches!(operation, "status" | "diff" | "log" | "branches" | "worktrees")
            } else {
                matches!(operation, "repository" | "pulls" | "pull" | "issues" | "issue" | "comments" | "files" | "reviews" | "checks")
            };
            if name == "/git" && read {
                Command::Repository { session, arguments }
            } else {
                Command::Tool { session, name: format!("{}_{}", if name == "/git" { "repository" } else { "github" }, if read { "read" } else { "write" }), arguments }
            }
        }
        "/checkpoints" => Command::Checkpoints { session },
        "/event" => Command::Events { session, after: arg.parse::<i64>()?.saturating_sub(1), limit: 1 },
        "/help" => return Ok(Input::Help),
        "/quit" => return Ok(Input::Quit),
        "/open" => return Ok(Input::Open(arg.into())),
        "/new" => return Ok(Input::New(arg.into())),
        "/sessions" => Command::Browse { filter: SessionFilter { query: arg.into(), ..Default::default() } },
        "/pin" => Command::Organize { session, title: None, pinned: Some(if arg.is_empty() { !s.pinned } else { on(arg)? }), archived: None, tags: None },
        "/archive" | "/restore" => Command::Organize { session, title: None, pinned: None, archived: Some(name == "/archive"), tags: None },
        "/rename" => Command::Organize { session, title: Some(arg.into()), pinned: None, archived: None, tags: None },
        "/tags" => Command::Organize { session, title: None, pinned: None, archived: None, tags: Some(arg.split_whitespace().map(str::to_owned).collect()) },
        "/interrupt" | "/queue" | "/after-tool" => Command::Steer {
            session,
            text: arg.into(),
            delivery: match name {
                "/interrupt" => Delivery::Interrupt,
                "/queue" => Delivery::Queue,
                _ => Delivery::AfterTool,
            },
            message_id: None,
        },
        "/inbox" => Command::Messages { session, after: 0, limit: 100 },
        "/cancel-message" => Command::CancelMessage { session, message: arg.into() },
        "/retry-message" => Command::RetryMessage { session, message: arg.into() },
        "/peers" => Command::Peers,
        "/connect" => {
            let (name, path) = arg.split_once(char::is_whitespace).context("expected NAME SERVICE_DIRECTORY")?;
            Command::LinkPeer { name: name.into(), directory: path.trim().into(), reciprocal: true }
        }
        "/disconnect" => Command::UnlinkPeer { name: arg.into() },
        "/peer-sessions" => Command::PeerSessions { peer: arg.into(), filter: Default::default() },
        "/message" => {
            let (target, text) = arg.split_once(char::is_whitespace).context("expected [PEER/]SESSION MESSAGE")?;
            let (peer, target) = target.split_once('/').map(|(p, s)| (Some(p.to_owned()), s)).unwrap_or((None, target));
            Command::SendMessage { session, target: target.into(), peer, text: text.into(), delivery: Delivery::AfterTool, wake: false, message_id: None }
        }
        "/status" => Command::Get { session },
        "/stop" => Command::Stop { session },
        "/resume" => Command::Resume { session },
        "/attach" => Command::Attach { session, path: arg.into() },
        "/goal" => Command::Goal { session, objective: arg.into() },
        "/memory" => Command::Memory { session, text: arg.into() },
        "/todos" => Command::Todos { session, todos: serde_json::from_str(arg)? },
        "/spec" => {
            if matches!(arg, "on" | "off") {
                Command::SpecMode { session, enabled: arg == "on" }
            } else {
                Command::Spec { session, text: arg.into(), criteria: vec![] }
            }
        }
        "/accept" => Command::AcceptSpec { session, revision: if arg.is_empty() { s.spec.revision } else { arg.parse()? } },
        "/approve" | "/deny" => {
            Command::Approve { session, call_id: if arg.is_empty() { s.pending.as_ref().context("no pending approval")?.call.id.clone() } else { arg.into() }, allow: name == "/approve" }
        }
        "/compact" => Command::Compact { session },
        "/fork" => Command::Fork { session, title: (!arg.is_empty()).then(|| arg.into()) },
        "/history" => {
            if arg.is_empty() {
                Command::Events { session, after: 0, limit: 100 }
            } else {
                Command::Search { session, query: arg.into() }
            }
        }
        "/undo" => Command::Undo { session, edit: arg.parse()? },
        "/models" => Command::Models { provider: s.config.model.provider },
        "/api" => serde_json::from_str(arg)?,
        "/defaults" => Command::Defaults { config: if arg.is_empty() { None } else { Some(serde_json::from_str(arg)?) } },
        "/set" => {
            let (key, value) = arg.split_once(char::is_whitespace).context("expected KEY JSON_VALUE")?;
            let mut v = serde_json::to_value(config)?;
            let keys = key.split('.').collect::<Vec<_>>();
            let mut at = &mut v;
            for k in &keys[..keys.len() - 1] {
                at = at.get_mut(*k).context("unknown configuration key")?;
            }
            let target = at.get_mut(*keys.last().unwrap()).context("unknown configuration key")?;
            *target = serde_json::from_str(value)?;
            Command::Configure { session, config: serde_json::from_value(v)? }
        }
        "/config" => {
            if arg.is_empty() {
                return Ok(Input::Config);
            }
            config = serde_json::from_str(arg)?;
            Command::Configure { session, config }
        }
        "/model" | "/provider" | "/keep" | "/agents" | "/readonly" | "/approval" | "/guidance" => {
            match name {
                "/model" => {
                    let mut p = arg.split_whitespace();
                    config.model.model = p.next().context("expected model id")?.into();
                    if let Some(e) = p.next() {
                        config.model.effort = e.into();
                    }
                }
                "/provider" => {
                    let provider = match arg {
                        "codex" => Provider::Codex,
                        "claude" => Provider::Claude,
                        _ => bail!("expected codex or claude"),
                    };
                    if config.model.provider != provider {
                        config.model = ModelConfig::for_provider(provider);
                    }
                }
                "/keep" => config.keep_going = on(arg)?,
                "/agents" => config.subagents = on(arg)?,
                "/readonly" => config.read_only = on(arg)?,
                "/guidance" => config.approval_guidance = arg.into(),
                "/approval" => {
                    config.approval = match arg {
                        "ask" => ApprovalMode::Ask,
                        "auto" => ApprovalMode::Auto,
                        "allow" => ApprovalMode::Allow,
                        _ => bail!("expected ask, auto or allow"),
                    }
                }
                _ => {}
            }
            Command::Configure { session, config }
        }
        _ => bail!("unknown command {name}; /help lists commands"),
    };
    Ok(Input::Command(command))
}
fn on(s: &str) -> Result<bool> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => bail!("expected on or off"),
    }
}
