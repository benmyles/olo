//! Coding agent status detection (OSC 7501 primary + screen-reading fallback),
//! Keep It Going, and Auto Approve (with 30 s countdown & human-only confirmation).

use std::time::{Duration, Instant};

use olo_rex_client::{ProgramPriority, ProgramStatusRecord};

/// Delay before Auto Approve sends confirmation to a blocked agent prompt.

/// High-level agent state shown on tab icons, pane headers, Activity sidebar,
/// and Deskbar notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentActivity {
    Idle,
    Working,
    Blocked,
    Error,
    Done,
}

impl AgentActivity {
    pub fn from_records(records: &[ProgramStatusRecord]) -> Option<(Self, &ProgramStatusRecord)> {
        let top = records.iter().max_by_key(|r| r.priority())?;
        let act = match top.priority() {
            ProgramPriority::Blocked => Self::Blocked,
            ProgramPriority::Error => Self::Error,
            ProgramPriority::Working => Self::Working,
            ProgramPriority::Done => Self::Done,
            ProgramPriority::Idle => Self::Idle,
        };
        Some((act, top))
    }
}

/// Fallback screen-reading heuristic for coding agents that don't yet emit OSC 7501
/// (e.g. Claude Code, Codex, Aider, Goose, OpenCode over plain SSH shells).
pub fn detect_agent_from_screen(foreground_name: Option<&str>, last_lines: &[String]) -> Option<AgentActivity> {
    let fg = foreground_name.unwrap_or_default().to_ascii_lowercase();
    let is_known_agent = matches!(fg.as_str(), "claude" | "codex" | "agy" | "antigravity" | "aider" | "opencode" | "goose" | "gemini")
        || last_lines.iter().any(|l| l.contains("Claude Code") || l.contains("OpenAI Codex"));
    if !is_known_agent {
        return None;
    }

    let tail: String = last_lines.iter().rev().take(8).cloned().collect::<Vec<_>>().join("\n");
    let lower = tail.to_ascii_lowercase();

    // (Antigravity's question: its line of keys under the answers. Its
    // "esc to cancel" is under its field while it works, and under a
    // question too: the question is looked for first.)
    if lower.contains("↑/↓ navigate")
        || lower.contains("do you want to proceed?")
        || lower.contains("allow once")
        || lower.contains("yes, allow")
        || lower.contains("approve this")
        || lower.contains("[y/n]")
        || lower.contains("(y/n)")
        || lower.contains("press enter to continue")
    {
        return Some(AgentActivity::Blocked);
    }

    if tail.contains('⠋')
        || tail.contains('⠙')
        || tail.contains('⠹')
        || tail.contains('⠸')
        || tail.contains('⠼')
        || tail.contains('⠴')
        || tail.contains('⠦')
        || tail.contains('⠧')
        || tail.contains('⠇')
        || tail.contains('⠏')
        || lower.contains("thinking…")
        || lower.contains("working…")
        || lower.contains("esc to interrupt")
        // (Antigravity: any of braille's figures turning at a line's
        // start, and what is under its field while it works.)
        || tail.lines().any(|l| l.trim_start().chars().next().is_some_and(|c| ('\u{2800}'..='\u{28ff}').contains(&c)))
        || (matches!(fg.as_str(), "agy" | "antigravity") && lower.contains("esc to cancel"))
    {
        return Some(AgentActivity::Working);
    }

    Some(AgentActivity::Idle)
}

/// Per-pane Keep It Going state: when an agent finishes (`Done`/`Idle` after
/// working), nudges it with a custom message until paused or stopped.
#[derive(Debug, Clone)]
pub struct KeepGoingState {
    pub enabled: bool,
    pub message: String,
    pub last_nudged: Option<Instant>,
    pub saw_working: bool,
}

impl KeepGoingState {
    pub fn new(message: String) -> Self {
        Self { enabled: true, message, last_nudged: None, saw_working: false }
    }

    /// Observe current activity; returns `Some(&str)` to write into the pane when
    /// a nudge is due (debounced by at least 8 seconds).
    pub fn observe(&mut self, activity: AgentActivity) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        match activity {
            AgentActivity::Working => {
                self.saw_working = true;
                None
            }
            AgentActivity::Done | AgentActivity::Idle if self.saw_working => {
                if self.last_nudged.is_some_and(|t| t.elapsed() < Duration::from_secs(8)) {
                    return None;
                }
                self.saw_working = false;
                self.last_nudged = Some(Instant::now());
                Some(&self.message)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Antigravity's screens (agy 1.5.15: Terminal's `agent` has the
    /// same, recorded from one run): at rest, working, asking.
    #[test]
    fn antigravity_is_read_by_its_own_words() {
        let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        let rule = "────────────────────────────────────────";
        let at_rest = lines(&format!("  DONE\n{rule}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{rule}\n? for shortcuts"));
        let working = lines(&format!("⣽  Generating...\n{rule}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{rule}\nesc to cancel"));
        let asking =
            lines("Run this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel");
        assert_eq!(detect_agent_from_screen(Some("agy"), &at_rest), Some(AgentActivity::Idle));
        assert_eq!(detect_agent_from_screen(Some("agy"), &working), Some(AgentActivity::Working));
        assert_eq!(detect_agent_from_screen(Some("agy"), &asking), Some(AgentActivity::Blocked));
        // (Its spinner has gone by and only the line under its field says so.)
        assert_eq!(detect_agent_from_screen(Some("agy"), &lines(&format!("{rule}\n> \n{rule}\nesc to cancel"))), Some(AgentActivity::Working));
        // (A shell is no agent, whatever is on its screen.)
        assert_eq!(detect_agent_from_screen(Some("bash"), &lines("$ echo esc to cancel")), None);
    }
}
