//! Coding agents in a terminal (Claude Code, Codex, Antigravity), read as
//! a person reads them: which one a program is, what it is doing from the
//! bottom of its screen, whether its own prompt is showing, and whether
//! the question it asks may be answered Yes for the person. Text in, an
//! answer out: nothing here reads a process, a terminal or a clock.
//! Terminal reads its tabs' screens with it, and Rex its panes', on this
//! computer or a far host, so the two apps' Keep It Going and Auto
//! Approve see an agent the same way (Ben, 2026-11-24: "did we already
//! add the support for auto approve + keep it going for all the coding
//! harnesses in our olo rex?").

use std::time::Duration;

/// How long a question shows before Auto Approve answers it (Ben,
/// 2026-11-09: "have it auto approve / select yes after 30s").
pub const AUTO_APPROVE_AFTER: Duration = Duration::from_secs(30);
/// How long Keep It Going waits with nothing happening (Ben: "change it
/// from 60s to 120s without activity").
pub const KEEP_GOING_QUIET: Duration = Duration::from_secs(120);
/// At most once in this long ("limit it to sending up to one "please
/// continue" per hour").
pub const KEEP_GOING_EVERY: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
    /// Google's Antigravity CLI (`agy`). (Ben, 2026-11-23: "make sure
    /// that keep it going and auto approve and any and all other agent
    /// features for both terminal and olo rex work with antigravity tui
    /// (agy)". Its screens here are of agy 1.5.15.)
    Antigravity,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Antigravity => "Antigravity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Busy with its turn.
    Working,
    /// Stopped on a question: a permission to grant, a choice to make.
    Blocked,
    /// Its turn is over: waiting for the next message.
    Idle,
}

impl State {
    pub fn describe(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "needs your answer",
            State::Idle => "waiting for you",
        }
    }
}

pub fn identify(comm: &str, exe: &str, args: &[String]) -> Option<Agent> {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    let names: Vec<String> = [comm.to_owned(), base(exe)].into_iter().chain(args.iter().take(2).map(|a| base(a))).collect();
    if exe.contains("/claude/versions/") || names.iter().any(|n| n == "claude") || args.iter().any(|a| a.contains("@anthropic-ai/claude-code")) {
        return Some(Agent::Claude);
    }
    if names.iter().any(|n| n == "codex") || args.iter().any(|a| a.contains("@openai/codex")) {
        return Some(Agent::Codex);
    }
    // (One program, `agy`, wherever its installer put it.)
    if names.iter().any(|n| n == "agy" || n == "antigravity") {
        return Some(Agent::Antigravity);
    }
    None
}

/// Whether a program is an agent's server for all its sessions: Codex's
/// app server, one daemon that runs every Codex session's tools and hooks.
pub fn shared_server(comm: &str, exe: &str, args: &[String]) -> bool {
    identify(comm, exe, args) == Some(Agent::Codex) && args.iter().skip(1).take(3).any(|a| a == "app-server")
}

/// What the agent is doing, from the bottom of its screen (where its
/// prompt, dialogs and status line are). Antigravity's words are its
/// own: "esc to cancel" is under its field while it works (Claude
/// Code's is under a question), and a question of its has "↑/↓
/// Navigate" under its answers; working with a message queued, its last
/// line is "Press up to edit queued messages" and only its spinner (a
/// braille figure at a line's start, above its field) says it works.
pub fn classify(agent: Agent, screen: &str) -> State {
    let s = screen.to_lowercase();
    if agent == Agent::Antigravity {
        // (Its spinner in the last lines, not its banner's art above:
        // read from the end, nothing gathered.)
        // (Its last eight lines with words: a screen given untrimmed ends
        // in blank ones. U+2800 is braille's blank, a space, no spinner.)
        let spinning = || screen.lines().rev().filter(|l| !l.trim().is_empty()).take(8).any(|l| l.trim_start().chars().next().is_some_and(|c| ('\u{2801}'..='\u{28ff}').contains(&c)));
        return if s.contains("↑/↓ navigate") {
            State::Blocked
        } else if s.contains("esc to cancel") || s.contains("press up to edit queued messages") || spinning() {
            State::Working
        } else {
            State::Idle
        };
    }
    const ASKING: [&str; 9] = ["do you want to", "would you like to", "allow command?", "enter to select", "enter to confirm", "esc to cancel", "❯ 1. yes", "› 1. yes", "(y/n)"];
    if ASKING.iter().any(|a| s.contains(a)) {
        State::Blocked
    } else if s.contains("esc to interrupt") {
        State::Working
    } else {
        State::Idle
    }
}

/// `agent`'s own prompt is on `screen`, waiting for a message: what Keep
/// It Going types into, and nothing else (a chooser takes its words as
/// keys and its Return as a choice). Claude Code's "❯" in the box under
/// its rule (or its older "│ >" box), Codex's "›" on a line that isn't an
/// answer ("› 1. Yes"). Not Claude Code's "Make auto mode your default
/// permission mode?", whose "❯" is on an answer with no number: Return
/// there would have said yes for the person. Antigravity's ">" between
/// its field's two rules, with nothing being asked (a question's chosen
/// answer begins with ">" too) and nothing under way.
pub fn at_prompt(agent: Agent, screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let rule = |l: &str| {
        let t = l.trim();
        t.contains("────") && (t.starts_with('─') || t.ends_with('─'))
    };
    let numbered = |s: &str| s.split_once('.').is_some_and(|(n, rest)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' '));
    lines.iter().enumerate().any(|(i, l)| {
        let t = l.trim_start();
        match agent {
            Agent::Claude => (t.starts_with('❯') && lines[..i].iter().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| rule(l))) || t.starts_with("│ >") || t.starts_with("│ ❯"),
            Agent::Codex => t.strip_prefix('›').is_some_and(|rest| !numbered(rest.trim_start())),
            Agent::Antigravity => {
                let ruled = |l: Option<&&str>| l.is_some_and(|l| rule(l));
                classify(agent, screen) == State::Idle
                    && t.starts_with('>')
                    && ruled(lines[..i].iter().rev().find(|l| !l.trim().is_empty()))
                    && ruled(lines[i + 1..].iter().find(|l| !l.trim().is_empty()))
            }
        }
    })
}

/// A question its agent asks that Auto Approve may answer Yes to: one
/// asking leave to go on ("Do you want to proceed?", "Do you want to make
/// this edit to …?", Codex's "Would you like to run the following
/// command?") with its first answer, "1. Yes", chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// Which question it is: the dialog's text, its digits aside (Claude
    /// Code's "will automatically block this request in 1:50" counts down
    /// under it), so the same question isn't taken for a new one.
    pub key: u64,
    /// Its question ("Do you want to proceed?").
    pub question: String,
}

/// The question on `screen` that Auto Approve may answer, if one is: the
/// last line asking "Do you want to …?" or "Would you like to …?", its
/// first answer after it "1. Yes" with the chooser (❯, ›) on it.
/// Antigravity's too: a question ("Run this command?") with its first
/// answer, "> 1. Yes, run command", right under it and "↑/↓ Navigate"
/// under its answers. (Return there is yes for this once: never its
/// "always allow", which are answers 2 and 3.)
/// Not a plan waiting to be approved (Claude Code's "No, keep planning"):
/// that answer is the person's.
pub fn approvable(screen: &str) -> Option<Ask> {
    let lines: Vec<&str> = screen.lines().collect();
    // (Antigravity's chooser is a plain ">", which is also where a
    // message is typed: taken for a chooser only on a screen that has
    // its line of keys.)
    let navigated = screen.contains("↑/↓ Navigate");
    let marks: &[char] = if navigated { &['❯', '›', '>'] } else { &['❯', '›'] };
    let q = lines.iter().enumerate().rposition(|(i, l)| {
        let t = l.trim().to_lowercase();
        let first_under = || lines[i + 1..].iter().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim().strip_prefix('>').is_some_and(|rest| rest.trim_start().starts_with("1. ")));
        t.ends_with('?') && (t.starts_with("do you want to ") || t.starts_with("would you like to ") || (navigated && first_under()))
    })?;
    let rest = &lines[q + 1..];
    // Its first answer (Codex shows the command between): "1. …", chosen.
    let at = rest.iter().position(|l| l.trim().trim_start_matches(marks).trim_start().starts_with("1. "))?;
    // (Its own: no other question between; but for the reason Codex gives
    // for a command, which it's told to put as a question, "Reason: Do you
    // want to …?": Auto Approve never answered Codex.)
    if rest[..at].iter().map(|l| l.trim()).any(|l| l.ends_with('?') && !l.starts_with("Reason:")) {
        return None;
    }
    let first = rest[at].trim();
    let chosen = marks.iter().find_map(|mark| first.strip_prefix(*mark))?.trim_start();
    if !chosen.to_lowercase().starts_with("1. yes") || rest.iter().any(|l| l.to_lowercase().contains("keep planning")) {
        return None;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in &lines[q.saturating_sub(12)..] {
        // Only its words, the digits of a clock counting down aside: not
        // the marks a program animates as it waits (Claude Code's "●"
        // before the tool call it asks about blinks, so the question
        // never read the same twice: Auto Approve answered only once it
        // stopped, half a minute on).
        l.chars().filter(|c| c.is_alphanumeric()).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>().hash(&mut h);
    }
    Some(Ask { key: h.finish(), question: lines[q].trim().to_owned() })
}

/// What the screen shows, for telling a change: its text with the
/// clocks that count by themselves taken out (Codex's "Pursuing goal
/// (17h 52m)", whose time goes on every minute whatever it's doing: Ben,
/// 2026-11-06).
pub fn fingerprint(screen: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for line in screen.lines() {
        match line.find("Pursuing goal (") {
            Some(at) => {
                let rest = &line[at + "Pursuing goal (".len()..];
                let after = rest.find(')').map_or("", |e| &rest[e + 1..]);
                (&line[..at], after).hash(&mut h);
            }
            None => line.hash(&mut h),
        }
    }
    h.finish()
}

/// Semantic kind of a choice in a tool approval dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChoiceKind {
    /// Approve this single invocation only ("1. Yes", "1. Yes, proceed", "1. Yes, run command").
    Once,
    /// Allow for the rest of this session/conversation/project without persisting globally.
    AllowSession,
    /// Persist allowance / "don't ask again" across future turns or sessions.
    DontAskAgain,
    /// Switch the agent's execution/permission mode (e.g. "switch to auto mode").
    SwitchMode,
    /// Deny / cancel ("No", "No, cancel", "No, and tell ...").
    No,
    /// Any other choice.
    Other,
}

impl ChoiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::AllowSession => "allow_session",
            Self::DontAskAgain => "dont_ask_again",
            Self::SwitchMode => "switch_mode",
            Self::No => "no",
            Self::Other => "other",
        }
    }
}

/// One parsed option inside an approval chooser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalChoice {
    /// 0-based position in the displayed list.
    pub index: usize,
    /// Leading item number (`1`, `2`, …) if shown.
    pub number: Option<u32>,
    /// Cleaned label text (without leading cursor mark or item number).
    pub label: String,
    /// True if the cursor mark (`❯`, `›`, `>`) currently rests on this choice.
    pub selected: bool,
    /// Semantic kind of the choice so callers can pick `"once"` or `"no"` safely by meaning.
    pub kind: ChoiceKind,
}

/// Structured details of a live tool-approval dialog on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub key: u64,
    pub question: String,
    pub tool: Option<String>,
    pub command: Option<String>,
    /// Set when the terminal folded long output (e.g. agy's `"⋯ 28 lines hidden"`).
    pub folded_lines: Option<usize>,
    pub choices: Vec<ApprovalChoice>,
    pub selected_index: Option<usize>,
}

/// Claude Code's startup folder-trust safety dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTrustPrompt {
    pub folder: Option<String>,
    /// 0-based index of the currently highlighted choice (`❯`).
    pub selected_index: usize,
    /// 0-based index of `"Yes, I trust this folder"`.
    pub trust_index: usize,
    /// True when the cursor is already on `"Yes, I trust this folder"`.
    pub selected_trust: bool,
}

/// Usage or rate-limit notice parsed from an agent's screen footer/status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageInfo {
    pub line: String,
    pub percent: Option<u8>,
    pub limit_reached: bool,
}

fn classify_choice_kind(label: &str) -> ChoiceKind {
    let lower = label.trim().to_ascii_lowercase();
    if lower.contains("switch to auto") || lower.contains("auto-accept") || lower.contains("auto mode") {
        return ChoiceKind::SwitchMode;
    }
    if lower.contains("persist to settings") || lower.contains("settings.json") || lower.contains("don't ask again") || lower.contains("dont ask again") {
        return ChoiceKind::DontAskAgain;
    }
    if lower.contains("always allow") {
        if lower.contains("this conversation") || lower.contains("this session") {
            return ChoiceKind::AllowSession;
        }
        return ChoiceKind::DontAskAgain;
    }
    if lower.contains("allow all edits") || lower.contains("during this session") || lower.contains("in this session") {
        return ChoiceKind::AllowSession;
    }
    if lower == "no" || lower.starts_with("no,") || lower.starts_with("no ") || lower.starts_with("no(") || lower.contains("cancel") {
        return ChoiceKind::No;
    }
    if lower == "yes" || lower.starts_with("yes,") || lower.starts_with("yes ") || lower.starts_with("yes(") {
        return ChoiceKind::Once;
    }
    ChoiceKind::Other
}

/// Detects Claude Code's folder-trust dialog and reports exact cursor & trust indices.
pub fn parse_folder_trust(screen: &str) -> Option<FolderTrustPrompt> {
    let lower = screen.to_ascii_lowercase();
    if !lower.contains("trust this folder") && !lower.contains("is this a project you created or one you trust") {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let mut folder: Option<String> = None;
    for (i, l) in lines.iter().enumerate() {
        if l.to_ascii_lowercase().contains("accessing workspace") {
            for next in &lines[i + 1..] {
                let t = next.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with('/') || t.starts_with('~') {
                    folder = Some(t.to_owned());
                }
                break;
            }
        }
    }
    let marks = ['❯', '›', '>'];
    let mut choices: Vec<(bool, String)> = Vec::new();
    for l in &lines {
        let trimmed = l.trim();
        let (marked, rest) = match marks.iter().find_map(|m| trimmed.strip_prefix(*m)) {
            Some(r) => (true, r.trim_start()),
            None => (false, trimmed),
        };
        let rest_clean = rest.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).map(|(_, r)| r.trim_start()).unwrap_or(rest);
        let rl = rest_clean.to_ascii_lowercase();
        if rl.starts_with("yes, i trust") || rl.starts_with("yes, trust") || rl.starts_with("no, exit") || rl.starts_with("no, i don't trust") {
            choices.push((marked, rest_clean.to_owned()));
        }
    }
    let trust_index = choices.iter().position(|(_, c)| c.to_ascii_lowercase().starts_with("yes"))?;
    let selected_index = choices.iter().position(|(m, _)| *m).unwrap_or(0);
    Some(FolderTrustPrompt { folder, selected_index, trust_index, selected_trust: selected_index == trust_index })
}

/// Parses a structured tool-approval prompt across Claude Code, Codex, and Antigravity (`agy`).
/// Unlike `approvable()`, returns choices even when the cursor is not on choice 1, and labels
/// each choice semantically (`Once`, `AllowSession`, `DontAskAgain`, `SwitchMode`, `No`, `Other`).
pub fn parse_approval(agent: Agent, screen: &str) -> Option<ApprovalPrompt> {
    if parse_folder_trust(screen).is_some() {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let navigated = screen.contains("↑/↓ Navigate") || agent == Agent::Antigravity;
    let marks: &[char] = if navigated { &['❯', '›', '>'] } else { &['❯', '›'] };
    let parse_numbered = |raw: &str| -> Option<(bool, u32, String)> {
        let t = raw.trim();
        if t.is_empty() {
            return None;
        }
        let (selected, rest) = match marks.iter().find_map(|m| t.strip_prefix(*m)) {
            Some(r) => (true, r.trim_start()),
            None => (false, t),
        };
        let (num_s, label) = rest.split_once(". ")?;
        if num_s.is_empty() || !num_s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let num = num_s.parse::<u32>().ok()?;
        Some((selected, num, label.trim().to_owned()))
    };

    let q_idx = lines.iter().enumerate().rposition(|(i, l)| {
        let t = l.trim().to_ascii_lowercase();
        let first_under = || lines[i + 1..].iter().find(|nl| !nl.trim().is_empty()).and_then(|nl| parse_numbered(nl)).is_some_and(|(_, num, _)| num == 1);
        t.ends_with('?') && (t.starts_with("do you want to ") || t.starts_with("would you like to ") || t.starts_with("run this command") || t.starts_with("allow ") || (navigated && first_under()))
    })?;

    let rest = &lines[q_idx + 1..];
    let first_choice_rel = rest.iter().position(|l| parse_numbered(l).is_some_and(|(_, num, _)| num == 1))?;
    if rest[..first_choice_rel].iter().map(|l| l.trim()).any(|l| l.ends_with('?') && !l.starts_with("Reason:")) {
        return None;
    }
    if rest.iter().any(|l| l.to_ascii_lowercase().contains("keep planning")) {
        return None;
    }

    // Collect consecutive numbered choices (allowing wrapped continuation lines indented under a choice).
    let mut choices: Vec<ApprovalChoice> = Vec::new();
    let mut expected_num = 1u32;
    let footer_marker = |l: &str| {
        let tl = l.trim().to_ascii_lowercase();
        tl.starts_with("esc to cancel") || tl.starts_with("press enter to confirm") || tl.starts_with("↑/↓ navigate") || tl.starts_with("enter to select") || tl.starts_with("tab to amend")
    };
    for line in &rest[first_choice_rel..] {
        let t = line.trim();
        if t.is_empty() || footer_marker(t) {
            if !choices.is_empty() {
                break;
            }
            continue;
        }
        if let Some((selected, num, label)) = parse_numbered(line)
            && num == expected_num
        {
            let kind = classify_choice_kind(&label);
            let idx = choices.len();
            choices.push(ApprovalChoice { index: idx, number: Some(num), label, selected, kind });
            expected_num += 1;
        } else if let Some(last) = choices.last_mut()
            && (line.starts_with("    ") || line.starts_with('\t'))
        {
            last.label.push(' ');
            last.label.push_str(t);
            last.kind = classify_choice_kind(&last.label);
        } else if !choices.is_empty() {
            break;
        }
    }
    if choices.len() < 2 {
        return None;
    }
    // Must actually look like a yes/no approval dialog (at least one Once or No choice).
    if !choices.iter().any(|c| matches!(c.kind, ChoiceKind::Once | ChoiceKind::No)) {
        return None;
    }
    // Must have a live cursor mark on one of the choices (prevents stale scrollback false positives).
    let selected_index = choices.iter().position(|c| c.selected)?;

    // Extract tool name, full command/arguments, and folded-line indicator from the dialog body above `q_idx`
    // (and between `q_idx` and `first_choice_rel` for Codex).
    let dialog_start = lines[..q_idx]
        .iter()
        .rposition(|l| {
            let t = l.trim();
            t.starts_with("────────") && t.chars().all(|c| c == '─')
        })
        .unwrap_or_else(|| q_idx.saturating_sub(24));
    let header_slice = &lines[dialog_start..q_idx];
    let between_slice = &rest[..first_choice_rel];

    let mut tool: Option<String> = None;
    let mut cmd_lines: Vec<String> = Vec::new();
    let mut folded_lines: Option<usize> = None;

    let parse_folded = |t: &str| -> Option<usize> {
        let low = t.to_ascii_lowercase();
        if low.contains("lines hidden") { low.split_whitespace().find_map(|w| w.parse::<usize>().ok()) } else { None }
    };

    // Detect dashed command block (Claude Code 2.1.331: `╌╌╌╌` ... `╌╌╌╌`).
    let is_dash = |l: &str| {
        let t = l.trim();
        t.len() >= 12 && t.chars().all(|c| c == '╌' || c == '─')
    };
    let dash_positions: Vec<usize> = header_slice.iter().enumerate().filter_map(|(i, l)| is_dash(l).then_some(i)).collect();
    if dash_positions.len() >= 2 {
        let a = dash_positions[dash_positions.len() - 2];
        let b = dash_positions[dash_positions.len() - 1];
        for l in &header_slice[a + 1..b] {
            let t = l.trim().trim_start_matches('│').trim();
            if let Some(n) = parse_folded(t) {
                folded_lines = Some(n);
            } else if !t.is_empty() {
                cmd_lines.push(t.to_owned());
            }
        }
    }

    // Check `Requesting permission for:` (Antigravity) and tool headers (`Bash command`, `Edit file`, etc.).
    let mut in_req = false;
    for l in header_slice {
        let t = l.trim();
        if let Some(n) = parse_folded(t) {
            folded_lines = Some(n);
        }
        let tl = t.to_ascii_lowercase();
        if tool.is_none() {
            if tl == "bash command" || tl == "command" || tl.starts_with("bash(") {
                tool = Some("Bash".into());
            } else if tl.starts_with("edit file") || tl.starts_with("edit(") {
                tool = Some("Edit".into());
            } else if tl.starts_with("write file") || tl.starts_with("create file") || tl.starts_with("write(") {
                tool = Some("Write".into());
            } else if tl.starts_with("read file") || tl.starts_with("read(") {
                tool = Some("Read".into());
            } else if let Some(after) = t.strip_prefix("● ").and_then(|s| s.split('(').next()) {
                let name = after.trim();
                if !name.is_empty() && name.len() <= 40 && !name.contains(' ') {
                    tool = Some(name.to_owned());
                }
            }
        }
        if tl.starts_with("requesting permission for:") {
            in_req = true;
            let after = t["Requesting permission for:".len()..].trim();
            if !after.is_empty() && cmd_lines.is_empty() {
                cmd_lines.push(after.to_owned());
            }
            continue;
        }
        if in_req && cmd_lines.is_empty() && !t.is_empty() && !is_dash(l) {
            cmd_lines.push(t.to_owned());
        } else if in_req && !t.is_empty() && !is_dash(l) && parse_folded(t).is_none() {
            cmd_lines.push(t.to_owned());
        }
    }

    // Codex puts `$ command` between the question and `1. Yes`.
    for l in between_slice {
        let t = l.trim();
        if let Some(n) = parse_folded(t) {
            folded_lines = Some(n);
        }
        if let Some(c) = t.strip_prefix("$ ") {
            if tool.is_none() {
                tool = Some("Bash".into());
            }
            cmd_lines.push(c.trim().to_owned());
        } else if !cmd_lines.is_empty() && !t.is_empty() && !t.starts_with("Environment:") && !t.starts_with("Reason:") {
            cmd_lines.push(t.to_owned());
        }
    }

    // Claude Code older format (no dashed box, indented command directly under `Bash command`).
    if cmd_lines.is_empty() {
        let mut saw_tool_hdr = false;
        for l in header_slice {
            let t = l.trim();
            let tl = t.to_ascii_lowercase();
            if tl == "bash command" || tl == "edit file" || tl == "write file" {
                saw_tool_hdr = true;
                continue;
            }
            if saw_tool_hdr && !t.is_empty() && !tl.starts_with("tip:") && !t.starts_with('⚠') && !is_dash(l) {
                cmd_lines.push(t.to_owned());
                break;
            }
        }
    }

    if tool.is_none() {
        let ql = lines[q_idx].to_ascii_lowercase();
        if ql.contains("command") {
            tool = Some("Bash".into());
        } else if ql.contains("edit") {
            tool = Some("Edit".into());
        } else if ql.contains("write") || ql.contains("create") {
            tool = Some("Write".into());
        }
    }

    let command = (!cmd_lines.is_empty()).then(|| cmd_lines.join("\n"));

    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in &lines[q_idx.saturating_sub(12)..] {
        l.chars().filter(|c| c.is_alphanumeric()).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>().hash(&mut h);
    }

    Some(ApprovalPrompt { key: h.finish(), question: lines[q_idx].trim().to_owned(), tool, command, folded_lines, choices, selected_index: Some(selected_index) })
}

/// Detects whether a genuine interactive question / non-approval chooser is active on screen
/// (never fires on transient `"esc to cancel"` or blank repaint frames).
pub fn parse_question(agent: Agent, screen: &str) -> Option<String> {
    if let Some(trust) = parse_folder_trust(screen) {
        return Some(match trust.folder {
            Some(f) => format!("Trust folder {f}?"),
            None => "Trust this folder?".into(),
        });
    }
    if parse_approval(agent, screen).is_some() {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let navigated = screen.contains("↑/↓ Navigate");
    let has_chooser_footer = {
        let sl = screen.to_ascii_lowercase();
        navigated || sl.contains("enter to select") || sl.contains("enter to confirm")
    };
    if !has_chooser_footer {
        return None;
    }
    // Confirm there is a visible cursor mark on a non-prompt chooser line.
    let has_mark = lines.iter().any(|l| {
        let t = l.trim_start();
        (t.starts_with('❯') || t.starts_with('›') || (navigated && t.starts_with("> "))) && !at_prompt(agent, l)
    });
    if !has_mark {
        return None;
    }
    lines.iter().rev().map(|l| l.trim()).find(|l| l.ends_with('?')).map(str::to_owned).or_else(|| Some("waiting for choice".into()))
}

/// Detects an API / stream mid-response error that ended a turn without a normal Stop hook
/// (e.g. Claude Code's `"API Error: Connection lost mid-response"`).
pub fn parse_api_error(agent: Agent, screen: &str) -> Option<String> {
    if classify(agent, screen) == State::Working {
        return None;
    }
    let tail: Vec<&str> = screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(18).collect();
    for line in tail {
        let l = line.trim_start_matches(['⎿', '●', '•', '✗', '✕', '⚠', ' ']).trim();
        let low = l.to_ascii_lowercase();
        if l.starts_with('>') || l.starts_with('❯') || l.starts_with('›') || l.starts_with('+') || l.starts_with('-') {
            continue;
        }
        if low.starts_with("api error:")
            || low.contains("connection lost mid-response")
            || low.contains("stream disconnected")
            || low.contains("api error (")
            || low.contains("overloaded_error")
            || low.contains("request timed out")
        {
            return Some(l.to_owned());
        }
    }
    None
}

/// Parses usage percentage and rate-limit banners from an agent's live screen.
pub fn parse_usage(screen: &str) -> Option<UsageInfo> {
    let tail: Vec<&str> = screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(15).collect();
    for line in tail {
        let low = line.to_ascii_lowercase();
        let is_limit_hit = low.contains("you've hit your limit")
            || low.contains("hit your usage limit")
            || low.contains("usage limit reached")
            || low.contains("rate limit reached")
            || low.contains("weekly limit reached")
            || low.contains("session limit reached")
            || low.contains("out of extra usage");
        let is_usage_pct =
            (low.contains("you've used ") || low.contains("used ")) && low.contains('%') && (low.contains("limit") || low.contains("weekly") || low.contains("session") || low.contains("quota"));
        if is_limit_hit || is_usage_pct {
            let percent = line.split('%').next().and_then(|before| before.rsplit(|c: char| !c.is_ascii_digit()).next()).and_then(|d| d.parse::<u8>().ok()).filter(|&n| n <= 100);
            let limit_reached = is_limit_hit || percent == Some(100);
            return Some(UsageInfo { line: line.to_owned(), percent, limit_reached });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(identify("2.1.311", "/home/u/.local/share/claude/versions/2.1.311", &args(&["claude"])), Some(Agent::Claude));
        assert_eq!(identify("node", "/usr/bin/node", &args(&["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"])), Some(Agent::Claude));
        assert_eq!(identify("codex", "/usr/local/bin/codex", &args(&["codex"])), Some(Agent::Codex));
        assert_eq!(identify("node", "/usr/bin/node", &args(&["node", "/usr/local/bin/codex", "--full-auto"])), Some(Agent::Codex));
        assert_eq!(identify("vim", "/usr/bin/vim", &args(&["vim", "claude.md"])), None);
        // Codex's server for every session (as `ps` has it, codex-cli
        // 0.170), and what isn't one: a window's own Codex, a run of
        // `codex exec`, Claude Code.
        let packaged = "/home/u/.codex/packages/app-server-daemon/0.170.0/codex";
        assert!(shared_server("codex", packaged, &args(&[packaged, "app-server", "--listen", "unix://", "--analytics-default-enabled"])));
        assert!(shared_server("codex", packaged, &args(&[packaged, "app-server", "daemon", "pid-update-loop"])));
        assert!(!shared_server("codex", "/usr/local/bin/codex", &args(&["codex", "--full-auto"])));
        assert!(!shared_server("codex", "/usr/local/bin/codex", &args(&["codex", "exec", "-m", "x", "look at app-server"])));
        assert!(!shared_server("claude", "/home/u/.local/share/claude/versions/2.1.311", &args(&["claude", "app-server"])));
    }

    #[test]
    fn codex_goal_clock_isnt_a_change() {
        let at = |t: &str| format!("• Did the thing.\n\n  Pursuing goal ({t}) · ship the browser\n› \n");
        assert_eq!(fingerprint(&at("17h 52m")), fingerprint(&at("17h 53m")));
        assert_ne!(fingerprint(&at("17h 52m")), fingerprint("• Did the thing.\n\n  Pursuing goal (17h 52m) · ship the browser\n› more\n"));
        assert_ne!(fingerprint("✻ Pondering… (12s)"), fingerprint("✻ Pondering… (13s)"));
    }

    /// What Auto Approve answers: leave to go on, "1. Yes" chosen (Claude
    /// Code's, as Ben's screenshot of 2026-11-09 has it, and Codex's); the
    /// same question as it counts down; not a plan, nor another answer
    /// chosen, nor a question that isn't a dialog's.
    #[test]
    fn what_auto_approve_answers() {
        let claude = |left: &str| {
            format!(
                " Bash command\n   git commit -q -m \"$MSG\" 2>&1 | tail -15\n   Commit the mechanical rename\n Dangerous rm operation not ruled out\n ⚠ Claude Code will automatically block this request in {left}, to avoid holding up an unattended session\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n\n Esc to cancel · Tab to amend\n"
            )
        };
        let a = approvable(&claude("1:50")).expect("Claude Code's");
        assert_eq!(a.question, "Do you want to proceed?");
        assert_eq!(approvable(&claude("1:49")).map(|b| b.key), Some(a.key));
        // Claude Code 2.1.322's, as Terminal read it (the fake model's
        // command): the "●" before the tool call blinks while it asks, the
        // same question either way.
        let real = |bullet: &str| {
            format!(
                "❯ approve-test\n\n{bullet} Mark the test approved\n  ⎿  $ touch /var/tmp/kig/work/claude-approved\n\n────────────────────────────────\n Bash command\n Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" below\n Mark the test approved\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n touch /var/tmp/kig/work/claude-approved\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and always allow access to /var/tmp/kig/work from this project\n   3. Yes, and switch to auto mode · auto mode handles these prompts for you\n   4. No\n\n Esc to cancel · Tab to amend\n"
            )
        };
        let on = approvable(&real("●")).expect("Claude Code 2.1.322's");
        assert_eq!(approvable(&real(" ")).map(|b| b.key), Some(on.key));
        // Codex 0.167's, as Terminal read it: its reason for the command
        // put as a question, as Codex tells its model to.
        let codex = "• Running touch /var/tmp/kig/work/codex-approved\n\n\n  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: Do you want to mark the test approved?\n\n  $ touch /var/tmp/kig/work/codex-approved\n\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `touch /var/tmp/kig/\n     work/codex-approved` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        assert_eq!(approvable(codex).map(|a| a.question), Some("Would you like to run the following command?".into()));
        assert_eq!(classify(Agent::Codex, codex), State::Blocked);
        assert!(approvable(" Edit file\n src/main.rs\n Do you want to make this edit to main.rs?\n ❯ 1. Yes\n   2. Yes, allow all edits during this session (shift+tab)\n   3. No\n").is_some());
        assert!(approvable("Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n  2. No (esc)\n").is_some());
        assert_ne!(approvable(&claude("1:50").replace("tail -15", "tail -16x")).map(|b| b.key), Some(a.key));
        // The person moved to No: theirs.
        assert!(approvable(" Do you want to proceed?\n   1. Yes\n ❯ 2. No\n").is_none());
        // A plan: theirs.
        assert!(approvable(" Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n").is_none());
        // Words in the conversation, not a dialog.
        assert!(approvable("● Do you want to proceed?\n\n╭────╮\n│ >  │\n╰────╯\n").is_none());
        // A choice that isn't yes or no.
        assert!(approvable(" Which database?\n ❯ 1. Postgres\n   2. SQLite\n").is_none());
        // Another question's answers under words that ask.
        assert!(approvable("Do you want to keep the old API?\n\n Which one should go?\n ❯ 1. Yes, the old\n   2. The new\n").is_none());
    }

    /// Keep It Going types only at the agent's own prompt (Claude Code
    /// 2.1.322's and Codex 0.167's as Terminal read them): not into a
    /// chooser, whose Return would choose.
    #[test]
    fn at_its_prompt() {
        let claude = "● Ran it.\n\n✻ Cogitated for 0s · done 7:59 PM\n                     ◐ medium · /effort\n────────────────────────────────\n❯\u{a0}\n────────────────────────────────\n  ⏸ manual mode on · ? for shortcuts\n";
        assert!(at_prompt(Agent::Claude, claude));
        assert!(at_prompt(Agent::Claude, "● Done: the tests pass.\n\n╭────────╮\n│ >      │\n╰────────╯\n  ? for shortcuts\n"));
        let auto_mode = "────────────────────────────────\n Make auto mode your default permission mode?\n\n   Auto mode lets Claude handle permission prompts automatically.\n\n   ❯ Yes, set auto mode as my default permission mode\n     No, keep manual mode\n";
        assert!(!at_prompt(Agent::Claude, auto_mode));
        let codex = "• Ran it.\n\n  Worked for 7s • 8:09 PM\n\n\n› Ask Codex to do anything\n\n  gpt-test default · /var/tmp/kig/work\n  ← for agents · ? for shortcuts\n";
        assert!(at_prompt(Agent::Codex, codex));
        assert!(!at_prompt(Agent::Codex, "  Would you like to run the following command?\n\n  $ touch x\n\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\n"));
    }

    #[test]
    fn states() {
        let working = "● Reading files\n\n✻ Pondering… (12s · ↓ 300 tokens · esc to interrupt)\n\n╭────╮\n│ >  │\n╰────╯\n";
        assert_eq!(classify(Agent::Claude, working), State::Working);
        let asking = " Bash command\n   rm -rf build\n Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)\n";
        assert_eq!(classify(Agent::Claude, asking), State::Blocked);
        let idle = "● Done: the tests pass.\n\n╭────────╮\n│ >      │\n╰────────╯\n  ? for shortcuts\n";
        assert_eq!(classify(Agent::Claude, idle), State::Idle);
        let codex = "• Working (4s • esc to interrupt)\n▌ \n";
        assert_eq!(classify(Agent::Codex, codex), State::Working);
        let codex_ask = "Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n";
        assert_eq!(classify(Agent::Codex, codex_ask), State::Blocked);
    }

    /// Claude Code 2.1.331's screens (2026-11-24, from a run of it on a
    /// specimen task, its rules shortened; nothing of the account in them):
    /// asking leave for a command, its third answer one that would switch
    /// the person's mode (never Auto Approve's: Return is the first); at its
    /// prompt, its mode in the footer; at work, its field shown under what
    /// it does; and asking whether to trust a folder (the person's alone).
    #[test]
    fn claude_code_2_1_331() {
        let rule = "────────────────────────────────────────";
        let dashed = "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌";
        let asking = format!(
            "❯ Run exactly this one command and nothing else: cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n  Copying a.txt to b.txt\n  ⎿  $ cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n{rule}\n Bash command\n Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" below\n Copy a.txt to b.txt\n{dashed}\n │ cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n{dashed}\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and always allow access to /var/tmp/s from this project\n   3. Yes, and switch to auto mode · auto mode handles these prompts for you\n   4. No\n Esc to cancel · Tab to amend\n"
        );
        assert_eq!(classify(Agent::Claude, &asking), State::Blocked);
        assert_eq!(approvable(&asking).map(|a| a.question), Some("Do you want to proceed?".into()));
        let idle = format!("  Ran 1 shell command\n● DONE\n✻ Crunched for 2s · done 1:47 PM\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · ? for shortcuts\n");
        assert_eq!(classify(Agent::Claude, &idle), State::Idle);
        assert!(at_prompt(Agent::Claude, &idle));
        assert_eq!(approvable(&idle), None);
        let working = format!("❯ Run exactly this one command\n  Copying a.txt to b.txt\n✻ Copying… (3s)\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · esc to interrupt · ← for agents\n");
        assert_eq!(classify(Agent::Claude, &working), State::Working);
        let trust = format!(
            "{rule}\n Accessing workspace:\n /var/tmp/s\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team).\n Claude Code'll be able to read, edit, and execute files here.\n Security guide\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n"
        );
        assert_eq!(classify(Agent::Claude, &trust), State::Blocked);
        assert_eq!(approvable(&trust), None, "the person's");
        assert!(!at_prompt(Agent::Claude, &trust));
    }

    /// Screens captured from the agents themselves, read as Olo reads them:
    /// each file in `OLO_SCREENS` (`:` between), named for its agent
    /// (`agy-…`, `claude-…`, `codex-…`), its state, whether Keep It Going
    /// would type there and what Auto Approve would answer. For checking
    /// what an agent's update draws against this (`tools/dev/agents peek
    /// NAME 40 > agy-asking.txt` of a specimen asked to run a command it
    /// needs leave for): `OLO_SCREENS=… cargo test -p olo-agent-screen
    /// read_screens -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn read_screens() {
        let Some(files) = std::env::var_os("OLO_SCREENS") else { return };
        for file in std::env::split_paths(&files) {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let agent = if name.starts_with("agy") {
                Agent::Antigravity
            } else if name.starts_with("codex") {
                Agent::Codex
            } else {
                Agent::Claude
            };
            let screen = std::fs::read_to_string(&file).unwrap();
            println!("{name}: {:?}; Keep It Going would type: {}; Auto Approve would answer: {:?}", classify(agent, &screen), at_prompt(agent, &screen), approvable(&screen).map(|a| a.question));
        }
    }

    /// Antigravity's screens (agy 1.5.15, recorded from one run of it;
    /// its banner, which names an account, left out): at its prompt,
    /// working, and asking leave to run a command.
    const AGY_RULE: &str = "──────────────────────────────────────────────────────────────────────";
    fn agy(under: &str) -> String {
        format!("> Do what the brief says.\n● Read(/var/tmp/x/brief.md) (ctrl+o to expand)\n▸ Thought for 8s, 541 tokens\n● Bash(cd /var/tmp/x && touch one.txt)\n{under}")
    }
    fn agy_idle() -> String {
        agy(&format!("  DONE\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\n? for shortcuts\n"))
    }
    fn agy_working() -> String {
        agy(&format!("⣽  Generating...\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\nesc to cancel\n"))
    }
    fn agy_asking() -> String {
        agy(&format!(
            "● Bash(cd /var/tmp/x && rm one.txt) (ctrl+o to expand)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cd /var/tmp/x && rm one.txt\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with 'rm one.txt'\n  3. Yes, and always allow for commands that start with 'rm one.txt' (Persist to settings.json)\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel\n"
        ))
    }

    #[test]
    fn antigravity_is_told_by_its_program_and_read_by_its_own_words() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(identify("agy", "/home/u/.local/bin/agy", &args(&["agy"])), Some(Agent::Antigravity));
        assert_eq!(identify("agy", "/home/u/.local/bin/agy", &args(&["agy", "--model", "gemini-4-neon-high"])), Some(Agent::Antigravity));
        // Its words: "esc to cancel" under the field is its working (a
        // question's of Claude Code's), "↑/↓ Navigate" its asking.
        assert_eq!(classify(Agent::Antigravity, &agy_idle()), State::Idle);
        assert_eq!(classify(Agent::Antigravity, &agy_working()), State::Working);
        assert_eq!(classify(Agent::Antigravity, &agy_asking()), State::Blocked);
        // (Working with a message queued: no "esc to cancel", its spinner
        // and the queue's line say so; Keep It Going mustn't type then.)
        let queued = agy(&format!("⣯  Generating...\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\nPress up to edit queued messages\n"));
        assert_eq!(classify(Agent::Antigravity, &queued), State::Working);
        assert!(!at_prompt(Agent::Antigravity, &queued));
        let spinner_only = agy(&format!("⠙  Generating...\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\n? for shortcuts\n"));
        assert_eq!(classify(Agent::Antigravity, &spinner_only), State::Working);
        // (A screen given untrimmed, its blank lines after: still
        // working. Braille's blank, U+2800, is a space, no spinner.)
        assert_eq!(classify(Agent::Antigravity, &format!("{spinner_only}\n\n\n\n\n\n\n\n\n\n")), State::Working);
        let blank = agy(&format!("\u{2800}  notes\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\n? for shortcuts\n"));
        assert_eq!(classify(Agent::Antigravity, &blank), State::Idle);
        // (Braille art far above, in a banner, isn't a spinner.)
        let banner = format!("⣿⣿⣿ agy\n{}\n\n\n\n\n\n\n\n{}", "⣿⣿⣿⣿", agy_idle());
        assert_eq!(classify(Agent::Antigravity, &banner), State::Idle);
        // Keep It Going types at its prompt, and nowhere else: not into
        // a question (whose chosen answer begins with ">" too), nor
        // while it works.
        assert!(at_prompt(Agent::Antigravity, &agy_idle()));
        assert!(!at_prompt(Agent::Antigravity, &agy_working()));
        assert!(!at_prompt(Agent::Antigravity, &agy_asking()));
        // Auto Approve: its question, with "1. Yes, run command" chosen
        // (Return: this once). Not with another answer chosen; and the
        // same question read twice is one question.
        let ask = approvable(&agy_asking()).expect("Antigravity's");
        assert_eq!(ask.question, "Run this command?");
        assert_eq!(approvable(&agy_asking()).map(|a| a.key), Some(ask.key));
        let moved = agy_asking().replace("> 1. Yes, run command\n  2. Yes, and", "  1. Yes, run command\n> 2. Yes, and");
        assert_eq!(approvable(&moved), None, "the chooser on an answer that always allows");
        assert_eq!(approvable(&agy_idle()), None);
        assert_eq!(approvable(&agy_working()), None);
        // (A line a person typed that ends in a question isn't one of
        // its questions: no first answer under it.)
        assert_eq!(approvable(&agy(&format!("{AGY_RULE}\n> shall we go on?\n{AGY_RULE}\n? for shortcuts\n"))), None);
    }

    /// What reading agy's screen costs (`perf stat` around this, release,
    /// for a change to `classify`: Rex reads each agent's screen after
    /// every pause in its output).
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn classify_cost() {
        let screens = [agy_idle(), agy_working(), agy_asking()];
        let mut working = 0;
        for i in 0..200_000 {
            if classify(Agent::Antigravity, std::hint::black_box(&screens[i % 3])) == State::Working {
                working += 1;
            }
        }
        assert_eq!(working, 66_667);
    }

    #[test]
    fn parses_folder_trust_structured_approvals_api_errors_and_usage() {
        let rule = "────────────────────────────────────────";
        let dashed = "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌";
        // 1. Folder trust prompt (No, exit preselected)
        let trust = format!(
            "{rule}\n Accessing workspace:\n /var/tmp/worktree-1\n Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n"
        );
        let tp = parse_folder_trust(&trust).expect("folder trust detected");
        assert_eq!(tp.folder.as_deref(), Some("/var/tmp/worktree-1"));
        assert_eq!((tp.selected_index, tp.trust_index, tp.selected_trust), (0, 1, false));
        assert!(parse_approval(Agent::Claude, &trust).is_none(), "trust is never a tool approval");

        // 2. Claude Code structured approval with semantic choice kinds (even if cursor is on choice 2)
        let claude_ask = format!(
            "{rule}\n Bash command\n Copy a.txt\n{dashed}\n │ cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n{dashed}\n Do you want to proceed?\n   1. Yes\n ❯ 2. Yes, and always allow access to /var/tmp/s from this project\n   3. Yes, and switch to auto mode\n   4. No\n Esc to cancel · Tab to amend\n"
        );
        let ap = parse_approval(Agent::Claude, &claude_ask).expect("claude approval");
        assert_eq!(ap.tool.as_deref(), Some("Bash"));
        assert_eq!(ap.command.as_deref(), Some("cp /var/tmp/s/a.txt /var/tmp/s/b.txt"));
        assert_eq!(ap.selected_index, Some(1));
        assert_eq!(ap.choices.len(), 4);
        assert_eq!(ap.choices[0].kind, ChoiceKind::Once);
        assert_eq!(ap.choices[1].kind, ChoiceKind::DontAskAgain);
        assert_eq!(ap.choices[2].kind, ChoiceKind::SwitchMode);
        assert_eq!(ap.choices[3].kind, ChoiceKind::No);

        // 3. Antigravity approval with folded lines
        let agy_ask = agy(&format!(
            "● Bash(cargo test) (ctrl+o to expand)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cargo test --workspace\n   ⋯ 28 lines hidden\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with 'cargo'\n  3. Yes, and always allow for commands that start with 'cargo' (Persist to settings.json)\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel\n"
        ));
        let ap_agy = parse_approval(Agent::Antigravity, &agy_ask).expect("agy approval");
        assert_eq!(ap_agy.tool.as_deref(), Some("Bash"));
        assert_eq!(ap_agy.folded_lines, Some(28));
        assert_eq!(ap_agy.choices[0].kind, ChoiceKind::Once);
        assert_eq!(ap_agy.choices[1].kind, ChoiceKind::AllowSession);
        assert_eq!(ap_agy.choices[2].kind, ChoiceKind::DontAskAgain);
        assert_eq!(ap_agy.choices[3].kind, ChoiceKind::No);

        // 4. API Error mid-response detection
        let api_err = format!("● Working on patch\n  ⎿  API Error: Connection lost mid-response\n{rule}\n❯ \n{rule}\n");
        assert_eq!(parse_api_error(Agent::Claude, &api_err).as_deref(), Some("API Error: Connection lost mid-response"));
        // Not while actively generating
        let api_err_working = format!("API Error: old\n✻ Pondering… (esc to interrupt)\n");
        assert!(parse_api_error(Agent::Claude, &api_err_working).is_none());

        // 5. Usage percentage & limit reached
        let usage_scr = format!("{rule}\n❯ \n{rule}\n  You've used 87% of your weekly limit · resets Fri\n");
        let u = parse_usage(&usage_scr).expect("usage parsed");
        assert_eq!(u.percent, Some(87));
        assert!(!u.limit_reached);
        let hit_scr = format!("{rule}\n❯ \n{rule}\n  You've hit your limit · resets 5pm\n");
        let uh = parse_usage(&hit_scr).expect("limit hit parsed");
        assert!(uh.limit_reached);
    }
}
