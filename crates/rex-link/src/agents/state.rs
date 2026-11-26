//! How a child stands, read from its screen (`olo_agent_screen`), what
//! its program says of itself and its transcript: one reading for
//! `status`, `list`, `watch` and the events.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::{AgentCli, NativeSubagent, to_unix_ms};

/// What a program in a terminal says it is doing (OSC 7501, as the Rex
/// server keeps it: the record of the highest priority).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activity {
    #[default]
    Idle,
    Working,
    Blocked,
    Error,
    Done,
}

impl Activity {
    /// The state a program status record names.
    pub fn of(state: &str) -> Self {
        match state {
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "error" => Self::Error,
            "done" => Self::Done,
            _ => Self::Idle,
        }
    }

    /// Which of two a pane is in, by Rex's order: blocked, error,
    /// working, done, idle.
    pub fn rank(self) -> u8 {
        match self {
            Self::Blocked => 4,
            Self::Error => 3,
            Self::Working => 2,
            Self::Done => 1,
            Self::Idle => 0,
        }
    }
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
pub fn classify_screen_rich(cli: AgentCli, activity: Activity, screen_text: &str, alive: bool) -> ScreenClassification {
    if !alive {
        return ScreenClassification { state: SubagentLiveState::Exited, summary: Some("tab closed".into()), approval: None, folder_trust: None, api_error: None, usage: None };
    }
    let last_nonempty = || screen_text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned);
    // A plain command (`start --command`): running until it exits, and
    // nothing read into what it prints.
    let Some(agent) = cli.screen_agent() else {
        return ScreenClassification {
            state: SubagentLiveState::Working,
            summary: last_nonempty().or_else(|| Some("running".into())),
            approval: None,
            folder_trust: None,
            api_error: None,
            usage: None,
        };
    };
    let usage = olo_agent_screen::parse_usage(screen_text);

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
        Activity::Working => (SubagentLiveState::Working, last_nonempty()),
        Activity::Blocked => {
            if screen_text.trim().is_empty() {
                (SubagentLiveState::Working, Some("starting".into()))
            } else {
                (SubagentLiveState::WaitingQuestion, Some("needs input (unrecognised dialog)".into()))
            }
        }
        Activity::Error => (SubagentLiveState::Error, last_nonempty()),
        Activity::Done => (SubagentLiveState::Done, last_nonempty()),
        Activity::Idle => {
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
        // (What a command child runs; an agent's own command line isn't shown.)
        "command": (!entry.command.is_empty()).then_some(&entry.command),
    })
}
