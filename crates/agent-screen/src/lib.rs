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
///
/// Scoped to each CLI's bottom chrome / dialog region so ordinary output
/// prose (e.g. `"Do you want to keep the old API?"` or `"esc to cancel"`
/// in docs or test logs) is never misclassified as `Blocked` or `Working`.
pub fn classify(agent: Agent, screen: &str) -> State {
    let non_empty: Vec<&str> = screen.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail_joined = |n: usize| -> String {
        let start = non_empty.len().saturating_sub(n);
        non_empty[start..].join("\n").to_ascii_lowercase()
    };
    if agent == Agent::Antigravity {
        // Check only the bottom 8 non-empty chrome lines (banner art / scrollback prose ignored).
        let bottom8 = tail_joined(8);
        let spinning = || non_empty.iter().rev().take(8).any(|l| l.chars().next().is_some_and(|c| ('\u{2801}'..='\u{28ff}').contains(&c)));
        return if bottom8.contains("↑/↓ navigate") || parse_approval(agent, screen).is_some() {
            State::Blocked
        } else if bottom8.contains("esc to cancel") || bottom8.contains("press up to edit queued messages") || spinning() {
            State::Working
        } else {
            State::Idle
        };
    }
    if parse_folder_trust(screen).is_some() || parse_approval(agent, screen).is_some() {
        return State::Blocked;
    }
    let bottom6 = tail_joined(6);
    let bottom10 = tail_joined(10);
    // Genuine working indicator in Claude Code / Codex status footer.
    if bottom6.contains("esc to interrupt") {
        return State::Working;
    }
    // Check chooser/dialog chrome strictly within the bottom region so prose never triggers Blocked.
    let has_dialog_mark = bottom10.contains("❯ 1. ") || bottom10.contains("› 1. ") || bottom6.contains("❯ 1.") || bottom6.contains("› 1.");
    let has_footer_chrome = bottom6.contains("enter to select") || bottom6.contains("enter to confirm") || bottom6.contains("press enter to continue") || bottom6.contains("esc to cancel");
    let has_non_prompt_chooser_mark = {
        let lines: Vec<&str> = screen.lines().collect();
        let prompt_idxs = prompt_line_indices(agent, &lines);
        let start = lines.len().saturating_sub(14);
        lines.iter().enumerate().skip(start).any(|(i, l)| {
            !prompt_idxs.contains(&i) && {
                let t = l.trim_start();
                t.starts_with('❯') || t.starts_with('›')
            }
        })
    };
    if has_dialog_mark
        || (has_footer_chrome && has_non_prompt_chooser_mark)
        || bottom6.contains("allow command?")
        || non_empty.last().is_some_and(|l| {
            let low = l.to_ascii_lowercase();
            low.ends_with("(y/n)") || low.ends_with("[y/n]")
        })
    {
        State::Blocked
    } else {
        State::Idle
    }
}

/// Returns line indices in `lines` that correspond to the agent's own chat input prompt
/// (as opposed to chooser cursor lines like `❯ 1. Yes` or `❯ No, exit`).
fn prompt_line_indices(agent: Agent, lines: &[&str]) -> Vec<usize> {
    let rule = |l: &str| {
        let t = l.trim();
        t.contains("────") && (t.starts_with('─') || t.ends_with('─'))
    };
    let numbered = |s: &str| s.split_once('.').is_some_and(|(n, rest)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' '));
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let is_prompt = match agent {
            Agent::Claude => (t.starts_with('❯') && lines[..i].iter().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| rule(l))) || t.starts_with("│ >") || t.starts_with("│ ❯"),
            Agent::Codex => t.strip_prefix('›').is_some_and(|rest| !numbered(rest.trim_start())),
            Agent::Antigravity => {
                let ruled = |l: Option<&&str>| l.is_some_and(|l| rule(l));
                t.starts_with('>') && ruled(lines[..i].iter().rev().find(|l| !l.trim().is_empty())) && ruled(lines[i + 1..].iter().find(|l| !l.trim().is_empty()))
            }
        };
        if is_prompt {
            out.push(i);
        }
    }
    out
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
    /// How many answers below the mark its yes for this once stands: 0
    /// where Return says it, as in nearly every question. Claude Code's
    /// read outside the working directories has a kept yes first, under
    /// the mark, and "Yes, but ask again next time" fourth: Down three
    /// times, and Return only once the mark is seen to stand on it.
    pub down: usize,
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
        // ("Allow this read outside the working directories?": Claude Code, 2.1.33x.)
        t.ends_with('?') && (t.starts_with("do you want to ") || t.starts_with("would you like to ") || t.starts_with("allow this ") || (navigated && first_under()))
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
    if rest.iter().any(|l| l.to_lowercase().contains("keep planning")) {
        return None;
    }
    // Its answers, each with whether the mark stands on it: "N. …"
    // lines, a line under one that isn't numbered being the rest of it.
    let mut answers: Vec<(bool, String)> = Vec::new();
    for line in &rest[at..] {
        let t = line.trim();
        let (marked, unmarked) = match marks.iter().find_map(|mark| t.strip_prefix(*mark)) {
            Some(after) => (true, after.trim_start()),
            None => (false, t),
        };
        let numbered = unmarked.split_once(". ").or_else(|| unmarked.split_once(".\t")).filter(|(n, _)| !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()));
        match (numbered, answers.last_mut()) {
            (Some((_, label)), _) => answers.push((marked, label.trim().to_owned())),
            // (The line of keys under them, or the end of them.)
            (None, _) if t.is_empty() || t.contains("Esc to cancel") || t.contains("↑/↓") || t.to_lowercase().starts_with("press enter") => break,
            (None, Some(last)) => {
                last.1.push(' ');
                last.1.push_str(t);
            }
            (None, None) => break,
        }
    }
    // The mark stands on one of them: where the agent put it (the
    // first), or on the yes for this once.
    let mut marked = answers.iter().enumerate().filter(|(_, (m, _))| *m).map(|(i, _)| i);
    let (Some(at_mark), None) = (marked.next(), marked.next()) else { return None };
    let once: Vec<usize> = answers.iter().enumerate().filter(|(_, (_, label))| classify_choice_kind(label) == ChoiceKind::Once).map(|(i, _)| i).collect();
    let down = match once[..] {
        // (As ever: the first answer is the yes, and the mark is on it.)
        _ if at_mark == 0 && once.first() == Some(&0) => 0,
        // One yes for this once, further down: the mark where the agent
        // put it (to be moved there), or on it already. Anywhere else,
        // someone moved it: theirs.
        [only] if at_mark == only => 0,
        [only] if at_mark == 0 => only,
        _ => return None,
    };
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
    Some(Ask { key: h.finish(), question: lines[q].trim().to_owned(), down })
}

/// The permission question an agent asks, whatever its shape, as Auto
/// Approve may answer it (Ben, 2026-11-26, of a read outside the working
/// directories that never counted down: "when it's on it should auto
/// approve everything after a 30s countdown"): any question
/// [`parse_approval`] reads (each agent's, its answers by what they
/// mean), answered with the yes that keeps nothing: the one for this
/// once, wherever it stands; where a question has none, the one for this
/// conversation alone. Never a yes that is kept in settings or switches
/// the agent's mode, and never a plan to choose a mode for (Ben, the same
/// day, of the two left unanswered: "i want folder trust auto approved.
/// plans should not be though").
///
/// A folder to trust is answered too, with its yes ([`trust_ask`]): the
/// agent can't begin without it, and whoever turned Auto Approve on for
/// a pane has said yes to what it does there.
///
/// The mark must stand where the agent put it (the first answer) or on
/// that yes already: anywhere else, someone moved it and it is theirs.
/// [`Ask::down`] says how far below the mark the yes is.
pub fn approvable_for(agent: Agent, screen: &str) -> Option<Ask> {
    if let Some(trust) = parse_folder_trust(screen) {
        return trust_ask(&trust);
    }
    let asked = parse_approval(agent, screen)?;
    let of_kind = |kind: ChoiceKind| -> Vec<usize> { asked.choices.iter().filter(|c| c.kind == kind).map(|c| c.index).collect() };
    let (once, session) = (of_kind(ChoiceKind::Once), of_kind(ChoiceKind::AllowSession));
    let yes = match (&once[..], &session[..]) {
        ([only], _) => *only,
        // (More than one reads as a plain yes: the first answer, where it is one, as ever.)
        ([first, ..], _) if *first == 0 => 0,
        ([], [only]) => *only,
        _ => return None,
    };
    let at_mark = asked.selected_index?;
    let down = match at_mark {
        m if m == yes => 0,
        0 => yes,
        _ => return None,
    };
    Some(Ask { key: asked.key, question: asked.question, down })
}

/// A folder an agent asks to trust before it begins, as Auto Approve
/// answers it: with its yes ("Yes, I trust this folder"; Codex's "Yes,
/// continue"; "Yes, add it" for a folder added), wherever it stands.
/// Claude Code puts the mark on "No, exit" above it, Antigravity on the
/// yes (both read from the programs, 2026-11-26). As for any question:
/// a mark must be seen, on the first answer or on the yes; anywhere
/// else it was moved, and is theirs.
fn trust_ask(trust: &FolderTrustPrompt) -> Option<Ask> {
    use std::hash::{Hash, Hasher};
    if !trust.marked {
        return None;
    }
    let down = match trust.selected_index {
        at if at == trust.trust_index => 0,
        0 => trust.trust_index,
        _ => return None,
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("trust", &trust.folder, trust.is_add_dir).hash(&mut h);
    Some(Ask { key: h.finish(), question: trust.question.clone(), down })
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

/// A feedback survey an agent draws over its own last lines, which
/// takes a digit as its answer (Claude Code's "How is Claude doing this
/// session? … 1: Bad 2: Fine 3: Good 0: Dismiss"; Antigravity's "How's
/// the CLI experience so far? … [1] Good [2] Fine [3] Bad [0] Skip").
/// Never a question for whoever looks after the agent: it once hid a
/// permission prompt under it for 25 minutes, and a message beginning
/// with a digit would answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    pub question: String,
    /// The key that sets it aside without answering it.
    pub dismiss: &'static str,
}

/// The feedback survey showing on an agent's screen, if one is: its
/// question and its choices as the agent draws them, on two lines one
/// after the other at the screen's foot, each from its line's start. (A
/// message that quotes the survey in its prose isn't one.)
pub fn parse_survey(agent: Agent, screen: &str) -> Option<Survey> {
    let (asks, first, last) = match agent {
        Agent::Claude => ("How is Claude doing this session?", "1: Bad", "0: Dismiss"),
        Agent::Antigravity => ("How's the CLI experience so far?", "[1] Good", "[0] Skip"),
        Agent::Codex => return None,
    };
    let foot: Vec<&str> = screen.lines().rev().filter(|l| !l.trim().is_empty()).take(12).collect();
    foot.windows(2).find_map(|pair| {
        // (`pair`: a line and the one above it, the foot being read upwards.)
        let choices = pair[0].split_whitespace().collect::<Vec<_>>().join(" ");
        let question = pair[1].trim().trim_start_matches(['●', '•', '*']).trim();
        (choices.starts_with(first) && choices.ends_with(last) && question.starts_with(asks)).then(|| Survey { question: question.to_owned(), dismiss: "0" })
    })
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
    /// Deny / cancel this once ("No", "No, cancel", "No, and tell ...",
    /// "No, and ask again next time").
    No,
    /// A no that is kept ("No, and block reads", "No, and don't ask
    /// again"): it refuses the like from now on, as `DontAskAgain`
    /// allows it. Never what "no, this once" means.
    NoAlways,
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
            Self::NoAlways => "no_always",
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

/// Startup folder-trust safety dialog (Claude Code workspace/`--add-dir` trust or Codex directory trust).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTrustPrompt {
    pub folder: Option<String>,
    /// True when Claude Code is asking about a secondary `--add-dir` directory rather than the primary workspace.
    pub is_add_dir: bool,
    /// 0-based index of the currently highlighted choice (`❯` / `›`).
    pub selected_index: usize,
    /// 0-based index of `"Yes, I trust this folder"` / `"Yes, add it"` / `"Yes, continue"`.
    pub trust_index: usize,
    /// True when the cursor is already on the trust choice.
    pub selected_trust: bool,
    /// A mark was seen on one of its choices (without one,
    /// `selected_index` is the first by default and nothing is known of
    /// where Return would land).
    pub marked: bool,
    /// The question as it is said to the person where it counts down
    /// ("Trust /var/tmp/new?").
    pub question: String,
}

/// Usage or rate-limit notice parsed from an agent's screen footer/status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageInfo {
    pub line: String,
    pub percent: Option<u8>,
    pub limit_reached: bool,
}

/// Normalizes Unicode apostrophes/quotes, strips zero-width characters, and collapses whitespace
/// so choice matching handles typographic punctuation like Claude Code's `don’t ask again` (U+2019).
fn normalize_choice_text(label: &str) -> String {
    let replaced: String = label
        .chars()
        .filter(|c| !matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}'))
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' | '\u{2032}' | '`' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{00A0}' => ' ',
            other => other.to_ascii_lowercase(),
        })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strips a single trailing parenthetical shortcut hint such as `(y)`, `(1)`, `(enter)`.
fn strip_trailing_shortcut(s: &str) -> &str {
    let t = s.trim();
    if let Some(open) = t.rfind('(')
        && t.ends_with(')')
    {
        let inside = &t[open + 1..t.len() - 1];
        if !inside.is_empty() && inside.len() <= 10 && inside.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '+' | '-' | '/')) {
            return t[..open].trim_end();
        }
    }
    t
}

pub fn classify_choice_kind(label: &str) -> ChoiceKind {
    let lower = normalize_choice_text(label);
    if lower.is_empty() {
        return ChoiceKind::Other;
    }
    if lower.contains("switch to auto")
        || lower.contains("auto-accept")
        || lower.contains("auto mode")
        || lower.contains("shift+tab")
        || lower.contains("bypass permission")
        || lower.contains("full-auto")
    {
        return ChoiceKind::SwitchMode;
    }
    // A no that is kept, before anything else a "no" could be taken for.
    let is_no =
        lower == "no" || lower.starts_with("no,") || lower.starts_with("no ") || lower.starts_with("no(") || lower.starts_with("deny") || lower.starts_with("reject") || lower.starts_with("never");
    if is_no
        && (lower.contains("block")
            || lower.contains("don't ask")
            || lower.contains("dont ask")
            || lower.contains("do not ask")
            || lower.contains("always")
            || lower.contains("never")
            || lower.contains("remember"))
    {
        return ChoiceKind::NoAlways;
    }
    if lower.contains("persist to settings")
        || lower.contains("settings.json")
        || lower.contains("keep allowing")
        || lower.contains("don't ask")
        || lower.contains("dont ask")
        || lower.contains("do not ask")
        || lower.contains("remember")
        || lower.contains("all future")
        || lower.contains("for future")
    {
        return ChoiceKind::DontAskAgain;
    }
    if lower.contains("always allow") || lower.contains("always approve") || lower.contains("always run") {
        if lower.contains("this conversation") || lower.contains("this session") {
            return ChoiceKind::AllowSession;
        }
        return ChoiceKind::DontAskAgain;
    }
    if lower.contains("allow all")
        || lower.contains("allow reading")
        || lower.contains("allow edits")
        || lower.contains("during this session")
        || lower.contains("in this session")
        || lower.contains("for this session")
        || lower.contains("this conversation")
        || lower.contains("rest of this session")
        || lower.contains("for commands that start with")
        || lower.contains("for commands in")
    {
        return ChoiceKind::AllowSession;
    }
    if lower == "no" || lower.starts_with("no,") || lower.starts_with("no ") || lower.starts_with("no(") || lower.contains("cancel") || lower.starts_with("deny") || lower.starts_with("reject") {
        return ChoiceKind::No;
    }
    // Strict allowlist for single-use `Once`: NEVER classify unknown `"Yes, and ..."` or
    // standing-permission phrasing as `Once`.
    let clean = strip_trailing_shortcut(&lower).trim_end_matches('.');
    if matches!(
        clean,
        "yes"
            | "y"
            | "yes, proceed"
            | "yes, continue"
            | "yes, run"
            | "yes, run command"
            | "yes, run this command"
            | "yes, run tool"
            | "yes, run once"
            | "yes, once"
            | "yes, allow once"
            | "yes, allow this once"
            | "yes, approve once"
            | "yes, this time"
            // (Claude Code's read outside the working directories: a yes for this read alone.)
            | "yes, but ask again next time"
            | "yes, and ask again next time"
            | "yes, allow"
            | "yes, allow this"
            | "yes, allow this tool"
            | "yes, allow this command"
            | "allow once"
            | "approve once"
            | "run once"
            | "run command"
            | "proceed"
    ) {
        return ChoiceKind::Once;
    }
    ChoiceKind::Other
}

/// Detects Claude Code's startup folder-trust / `--add-dir` trust dialog or Codex's
/// directory-trust safety dialog and reports exact cursor & trust indices.
pub fn parse_folder_trust(screen: &str) -> Option<FolderTrustPrompt> {
    let lower = normalize_choice_text(screen);
    let is_add_dir = lower.contains("adding a new directory") && (lower.contains("yes, add") || lower.contains("is this a directory you created or one you trust"));
    let is_codex_trust = lower.contains("do you trust the contents of this directory") && lower.contains("yes, continue");
    let is_folder_trust = is_add_dir
        || is_codex_trust
        || lower.contains("trust this folder")
        || lower.contains("is this a project you created or one you trust")
        || lower.contains("is this a directory you created or one you trust");
    if !is_folder_trust {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let mut folder: Option<String> = None;
    for (i, l) in lines.iter().enumerate() {
        let lt = l.to_ascii_lowercase();
        if lt.contains("accessing workspace") || lt.contains("adding a new directory") {
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
        } else if is_codex_trust
            && folder.is_none()
            && let Some(pos) = lt.find("you are in ")
        {
            let rest = l[pos + "you are in ".len()..].trim().trim_end_matches('.');
            if rest.starts_with('/') || rest.starts_with('~') {
                folder = Some(rest.to_owned());
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
        let rl = normalize_choice_text(rest_clean);
        if rl.starts_with("yes, i trust")
            || rl.starts_with("yes, trust")
            || rl.starts_with("yes, add")
            || (is_codex_trust && rl.starts_with("yes, continue"))
            || rl.starts_with("no, exit")
            || rl.starts_with("no, quit")
            || rl.starts_with("no, i don't trust")
        {
            choices.push((marked, rest_clean.to_owned()));
        }
    }
    let trust_index = choices.iter().position(|(_, c)| c.to_ascii_lowercase().starts_with("yes"))?;
    let marked = choices.iter().any(|(m, _)| *m);
    let selected_index = choices.iter().position(|(m, _)| *m).unwrap_or(0);
    let question = match &folder {
        Some(folder) if is_add_dir => format!("Add and trust {folder}?"),
        Some(folder) => format!("Trust {folder}?"),
        None => "Trust this folder?".to_owned(),
    };
    Some(FolderTrustPrompt { folder, is_add_dir, selected_index, trust_index, selected_trust: selected_index == trust_index, marked, question })
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

    let is_known_question = |q: &str| -> bool {
        let t = q.trim().to_ascii_lowercase();
        t.ends_with('?') && (t.starts_with("do you want to ") || t.starts_with("would you like to ") || t.starts_with("run this command") || t.starts_with("allow "))
    };

    // Match single-line OR 2-line wrapped approval question headers (e.g. long edit file paths).
    let (q_start_idx, q_idx, question_text) = lines.iter().enumerate().rev().find_map(|(i, l)| {
        let trimmed = l.trim();
        if !trimmed.ends_with('?') {
            return None;
        }
        let first_under = || lines[i + 1..].iter().find(|nl| !nl.trim().is_empty()).and_then(|nl| parse_numbered(nl)).is_some_and(|(_, num, _)| num == 1);
        if is_known_question(trimmed) || (navigated && first_under()) {
            return Some((i, i, trimmed.to_owned()));
        }
        // Try joining with the preceding non-empty line for wrapped questions (`Do you want to make this edit to` / `path/to/file.rs?`).
        if let Some((pi, prev)) = lines[..i].iter().enumerate().rev().find(|(_, pl)| !pl.trim().is_empty()) {
            let joined = format!("{} {}", prev.trim(), trimmed);
            if is_known_question(&joined) {
                return Some((pi, i, joined));
            }
        }
        None
    })?;

    let rest = &lines[q_idx + 1..];
    // Find `1. ...` strictly belonging to the chooser (not a numbered line inside a command preview:
    // require it to be the last `1. ` block or followed by another choice + cursor mark).
    let first_choice_rel = rest.iter().enumerate().rev().find_map(|(idx, l)| {
        let (_, num, _) = parse_numbered(l)?;
        if num != 1 {
            return None;
        }
        // Confirm at least one choice in this trailing block has a live cursor mark.
        let has_cursor = rest[idx..].iter().take(12).any(|cl| parse_numbered(cl).is_some_and(|(sel, _, _)| sel));
        has_cursor.then_some(idx)
    })?;
    if rest[..first_choice_rel].iter().map(|l| l.trim()).any(|l| l.ends_with('?') && !l.starts_with("Reason:")) {
        return None;
    }
    if rest.iter().any(|l| l.to_ascii_lowercase().contains("keep planning")) {
        return None;
    }

    // Collect numbered choices in ascending order (supporting skipped numbers like agy's `1, 2, 4`
    // and both indented AND column-0 wrapped continuation lines, while NEVER absorbing another numbered line).
    let mut choices: Vec<ApprovalChoice> = Vec::new();
    let mut last_num = 0u32;
    let mut wrap_lines_for_last = 0u8;
    let footer_marker = |l: &str| {
        let tl = l.trim().to_ascii_lowercase();
        tl.starts_with("esc to cancel")
            || tl.starts_with("press enter to confirm")
            || tl.starts_with("press enter to continue")
            || tl.starts_with("↑/↓ navigate")
            || tl.starts_with("enter to select")
            || tl.starts_with("tab to amend")
            || tl.starts_with("ctrl+")
    };
    let is_rule_or_chrome =
        |t: &str| t.starts_with('─') || t.starts_with('╌') || t.starts_with('═') || t.starts_with('━') || t.starts_with('❯') || t.starts_with('›') || t.starts_with("> ") || t.starts_with("? for ");
    for line in &rest[first_choice_rel..] {
        let t = line.trim();
        if t.is_empty() || footer_marker(t) {
            if !choices.is_empty() {
                break;
            }
            continue;
        }
        if let Some((selected, num, label)) = parse_numbered(line) {
            if (choices.is_empty() && num == 1) || (!choices.is_empty() && num > last_num && num <= last_num + 3) {
                let kind = classify_choice_kind(&label);
                let idx = choices.len();
                choices.push(ApprovalChoice { index: idx, number: Some(num), label, selected, kind });
                last_num = num;
                wrap_lines_for_last = 0;
            } else if !choices.is_empty() {
                // Out-of-order numbered line: NEVER merge into previous choice!
                break;
            }
        } else if let Some(last) = choices.last_mut()
            && !is_rule_or_chrome(t)
            && wrap_lines_for_last < 3
        {
            last.label.push(' ');
            last.label.push_str(t);
            last.kind = classify_choice_kind(&last.label);
            wrap_lines_for_last += 1;
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

    // Extract tool name, full command/arguments, and folded-line indicator from the dialog body above `q_start_idx`
    // (and between `q_idx` and `first_choice_rel` for Codex).
    let dialog_start = lines[..q_start_idx]
        .iter()
        .rposition(|l| {
            let t = l.trim();
            t.starts_with("────────") && t.chars().all(|c| c == '─')
        })
        .unwrap_or_else(|| q_start_idx.saturating_sub(24));
    let header_slice = &lines[dialog_start..q_start_idx];
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
        let ql = question_text.to_ascii_lowercase();
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
    for l in &lines[q_start_idx.saturating_sub(12)..] {
        l.chars().filter(|c| c.is_alphanumeric()).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>().hash(&mut h);
    }

    Some(ApprovalPrompt { key: h.finish(), question: question_text, tool, command, folded_lines, choices, selected_index: Some(selected_index) })
}

/// Detects whether a genuine interactive question / non-approval chooser is active on screen
/// (never fires on transient `"esc to cancel"`, idle prompt lines, or prose mentioning `"enter to confirm"`).
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
    let non_empty: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let bottom8 = non_empty[non_empty.len().saturating_sub(8)..].join("\n").to_ascii_lowercase();
    let navigated = bottom8.contains("↑/↓ navigate");
    let has_chooser_footer = navigated || bottom8.contains("enter to select") || bottom8.contains("enter to confirm") || bottom8.contains("press enter to continue");
    if !has_chooser_footer {
        return None;
    }
    // Evaluate prompt lines against the WHOLE screen and exclude them so Claude's own `❯`
    // prompt never counts as a chooser cursor mark.
    let prompt_idxs = prompt_line_indices(agent, &lines);
    let dialog_top = lines.len().saturating_sub(18);
    let has_mark = lines.iter().enumerate().skip(dialog_top).any(|(i, l)| {
        if prompt_idxs.contains(&i) {
            return false;
        }
        let t = l.trim_start();
        t.starts_with('❯') || t.starts_with('›') || (navigated && t.starts_with("> "))
    });
    if !has_mark {
        return None;
    }
    lines[dialog_top..].iter().rev().map(|l| l.trim()).find(|l| l.ends_with('?')).map(str::to_owned).or_else(|| Some("waiting for choice".into()))
}

/// Detects an API / stream mid-response error that ended the CURRENT turn without a normal Stop hook
/// (e.g. Claude Code's `"API Error: Connection lost mid-response"` or Codex's `"■ ..."`).
/// Stops scanning at prior turn boundaries so a stale error from an earlier turn never poisons
/// a later healthy turn.
pub fn parse_api_error(agent: Agent, screen: &str) -> Option<String> {
    if classify(agent, screen) == State::Working {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let prompt_idxs = prompt_line_indices(agent, &lines);
    let mut seen_non_chrome = 0usize;
    for (idx, raw) in lines.iter().enumerate().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Skip horizontal rules & prompt input box / footer chrome.
        if line.chars().all(|c| matches!(c, '─' | '╌' | '═' | '━' | '╭' | '╮' | '╰' | '╯')) {
            continue;
        }
        // If we cross back to a non-empty user turn line (`› ...`, `❯ ...`, `> ...`) that is NOT
        // the live empty prompt box / placeholder, any error above it belonged to an earlier turn!
        let is_user_turn_line = (line.starts_with('❯') || line.starts_with('›') || line.starts_with("> ")) && {
            let body = line.trim_start_matches(['❯', '›', '>', ' ', '\u{a0}']).trim();
            let bl = body.to_ascii_lowercase();
            !body.is_empty() && !bl.starts_with("ask ") && !bl.starts_with("try \"") && !bl.starts_with("accept-edits mode") && !bl.starts_with("auto mode") && !bl.starts_with("plan mode")
        };
        if is_user_turn_line {
            break;
        }
        if prompt_idxs.contains(&idx) {
            continue;
        }
        let low_raw = line.to_ascii_lowercase();
        if low_raw.contains("? for shortcuts") || low_raw.contains("← for agents") || low_raw.contains("shift+tab to cycle") || low_raw.starts_with("gpt-") {
            continue;
        }
        // Codex's turn-completion summary or a non-error assistant reply means the current turn succeeded!
        if agent == Agent::Codex && (low_raw.contains("worked for ") || (line.starts_with("• ") && !low_raw.contains("error") && !low_raw.contains("failed") && !low_raw.contains("interrupted"))) {
            break;
        }
        seen_non_chrome += 1;
        if seen_non_chrome > 12 {
            break;
        }
        if let Some(after_sq) = line.strip_prefix("■ ").map(str::trim).filter(|s| !s.is_empty()) {
            // Skip Codex's expected user-denial notice ("Conversation interrupted - tell the model what to do differently")
            // if a turn isn't failed; wait: when an API/stream error actually happens, Codex prints "■ stream disconnected" / "■ unexpected status".
            // Keep real errors (including stream/API failures), and still report genuine unrecovered interrupts only when it's the current turn's tail!
            return Some(after_sq.to_owned());
        }
        let l = line.trim_start_matches(['⎿', '●', '•', '✗', '✕', '⚠', ' ']).trim();
        let low = l.to_ascii_lowercase();
        if l.starts_with('>') || l.starts_with('❯') || l.starts_with('›') || l.starts_with('+') || l.starts_with('-') {
            continue;
        }
        if low.starts_with("api error:")
            || low.starts_with("api error (")
            || low.contains("connection lost mid-response")
            || low.contains("stream disconnected")
            || low.contains("overloaded_error")
            || (low.starts_with("error:") && low.contains("timed out"))
        {
            return Some(l.to_owned());
        }
    }
    None
}

/// Parses usage percentage and rate-limit banners from an agent's live screen footer/status bar.
pub fn parse_usage(screen: &str) -> Option<UsageInfo> {
    let tail: Vec<&str> = screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(6).collect();
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

    #[test]
    fn fuzz_approval_choice_classification_never_mislabels_persistent_as_once() {
        // (A small generator of its own: the same cases every run.)
        struct Rng(u64);
        impl Rng {
            fn next_u64(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.0 = x;
                x
            }
            fn range(&mut self, n: usize) -> usize {
                (self.next_u64() as usize) % n.max(1)
            }
            fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
                &slice[self.range(slice.len())]
            }
        }
        let mut rng = Rng(0x010_5AFE_A990_0001);

        // Real-world traps reported in production + adversarial variants across all 3 harnesses.
        let persistent_or_session_traps = [
            "Yes, and don\u{2019}t ask again for: env",
            "Yes, and don't ask again for: cargo test",
            "Yes, and don\u{2018}t ask again for commands in /home/user/repo",
            "Yes, and dont ask again for `git status`",
            "Yes, and do not ask again for this command",
            "Yes, and always allow in this conversation for commands that start with 'env'",
            "Yes, and always allow for commands that start with 'env' (Persist to settings.json)",
            "Yes, allow reading from /workspace/project during this session",
            "Yes, allow all edits during this session (shift+tab)",
            "Yes, switch to auto mode",
            "Yes, and remember this choice for future turns",
            "Yes, and trust all future runs of this tool",
            "Yes, and auto-accept similar commands",
            "Yes, always run without asking",
            "Yes, allow for this session",
            "Yes, and something totally new and unknown that grants extra scope",
        ];
        for trap in persistent_or_session_traps {
            let k = classify_choice_kind(trap);
            assert_ne!(k, ChoiceKind::Once, "SECURITY VIOLATION: persistent/unknown choice `{trap}` was classified as `Once`!");
        }

        // Valid single-use choices across Claude Code, Codex, and Antigravity.
        let valid_once = ["Yes", "Yes (y)", "Yes, proceed (y)", "Yes, run command", "Yes, run this command", "Yes, allow once", "Allow once"];
        for ok in valid_once {
            assert_eq!(classify_choice_kind(ok), ChoiceKind::Once, "valid single-use choice `{ok}` should be `Once`");
        }

        // Wrapped multi-line Claude/Codex/Agy approval dialogs across 250 randomized layouts.
        let prefixes = ["❯ ", "› ", "> "];
        let indents = ["  ", "   ", "    ", "\t"];
        let apostrophes = ["'", "\u{2019}", "\u{2018}", "`"];
        let cmds = ["env", "cargo test -p api && echo 'ok'", "rm -rf build && make", "git status --short"];
        for i in 0..250 {
            let is_agy = i % 3 == 2;
            let agent = match i % 3 {
                0 => Agent::Claude,
                1 => Agent::Codex,
                _ => Agent::Antigravity,
            };
            let mark = if is_agy { "> " } else { *rng.pick(&prefixes[..2]) };
            let apos = *rng.pick(&apostrophes);
            let indent = *rng.pick(&indents);
            let cmd = *rng.pick(&cmds);
            let selected_idx = rng.range(3);
            let c1 = if is_agy {
                "Yes, run command"
            } else if i % 3 == 1 {
                "Yes, proceed (y)"
            } else {
                "Yes"
            };
            // Deliberately wrap choice 2 onto a continuation line with 2..4 spaces!
            let c2_a = "Yes, and";
            let c2_b = format!("don{apos}t ask again for: {cmd}");
            let c3 = if is_agy { "No, cancel" } else { "No, and tell agent what to do differently (esc)" };

            let m = |idx: usize| if idx == selected_idx { mark } else { "  " };
            let footer = if is_agy {
                "  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command"
            } else if i % 3 == 1 {
                "  Press enter to confirm or esc to cancel"
            } else {
                " Esc to cancel · Tab to amend"
            };
            let question = if is_agy {
                "Run this command?"
            } else if i % 3 == 1 {
                "Would you like to run the following command?"
            } else {
                "Do you want to proceed?"
            };
            let body = if is_agy {
                format!("Command\n────────────────────\nRequesting permission for:\n   {cmd}\n{question}\n")
            } else if i % 3 == 1 {
                format!("{question}\n\n  $ {cmd}\n\n")
            } else {
                format!("Bash command\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n  {cmd}\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n{question}\n")
            };
            let screen = format!("{body}{}1. {c1}\n{}2. {c2_a}\n{indent}{c2_b}\n{}3. {c3}\n{footer}\n", m(0), m(1), m(2));
            let ap = parse_approval(agent, &screen).unwrap_or_else(|| panic!("case {i} failed to parse:\n{screen}"));
            assert_eq!(ap.selected_index, Some(selected_idx), "case {i} wrong selected_index");
            assert_eq!(ap.choices.len(), 3, "case {i} wrong choice count: {:?}", ap.choices);
            assert_eq!(ap.choices[0].kind, ChoiceKind::Once, "case {i} choice 0");
            assert_ne!(ap.choices[1].kind, ChoiceKind::Once, "case {i} choice 1 MUST NOT be Once: {:?}", ap.choices[1]);
            assert_eq!(ap.choices[1].kind, ChoiceKind::DontAskAgain, "case {i} choice 1 should be DontAskAgain: {:?}", ap.choices[1]);
            assert_eq!(ap.choices[2].kind, ChoiceKind::No, "case {i} choice 2");
            assert!(ap.command.as_deref().is_some_and(|c| c.contains(cmd)), "case {i} command missing: {:?}", ap.command);
        }

        // Folder-trust AND `--add-dir` ("Adding a new directory:") prompts must NEVER be misclassified as tool approvals.
        let add_dir_screen = "────────────────────\n Adding a new directory:\n /var/tmp/extra-dir\n This session hasn't worked here before. Is this a directory you created or one you trust?\n ❯ No, exit\n   Yes, add it\n Enter to confirm · Esc to cancel\n";
        let trust = parse_folder_trust(add_dir_screen).expect("must detect --add-dir trust prompt");
        assert_eq!(trust.folder.as_deref(), Some("/var/tmp/extra-dir"));
        assert_eq!(trust.selected_index, 0);
        assert_eq!(trust.trust_index, 1);
        assert!(!trust.selected_trust);
        assert!(parse_approval(Agent::Claude, add_dir_screen).is_none());
    }

    /// A folder to trust is answered with its yes (Ben, 2026-11-26: "i
    /// want folder trust auto approved. plans should not be though"),
    /// as each agent draws the question (Claude Code 2.1.332 and
    /// Antigravity read from the programs that day, each started in a
    /// new folder; Codex's from its own test above).
    #[test]
    fn auto_approve_trusts_a_folder_and_never_approves_a_plan() {
        let rule = "─".repeat(60);
        // Claude Code: the mark starts on "No, exit"; the yes is one down.
        let claude = format!(
            "{rule}\n Accessing workspace:\n /var/tmp/olo-agents-try/trust/claude-new\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open\n source project, or work from your team). If not, take a moment to review what's in this folder first.\n Claude Code'll be able to read, edit, and execute files here.\n Security guide\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n"
        );
        let ask = approvable_for(Agent::Claude, &claude).expect("a folder to trust is answered");
        assert_eq!((ask.down, ask.question.as_str()), (1, "Trust /var/tmp/olo-agents-try/trust/claude-new?"));
        assert_eq!(classify(Agent::Claude, &claude), State::Blocked);
        // The mark moved down to the yes: Return is right, and it is the same question still.
        let on_yes = claude.replace(" ❯ No, exit\n   Yes, I trust this folder", "   No, exit\n ❯ Yes, I trust this folder");
        let there = approvable_for(Agent::Claude, &on_yes).unwrap();
        assert_eq!((there.down, there.key), (0, ask.key));
        // No mark seen (a screen half drawn): nothing is known of where Return would land.
        let unmarked = claude.replace(" ❯ No, exit", "   No, exit");
        assert!(approvable_for(Agent::Claude, &unmarked).is_none());
        // Antigravity: the mark is on the yes from the start.
        let agy = "Accessing workspace:\n/var/tmp/olo-agents-try/trust/agy-new\nDo you trust the contents of this project?\nAntigravity CLI requires permission to read, edit, and execute files here.\n> Yes, I trust this folder\n  No, exit\n  ↑/↓ Navigate · enter Confirm\n\n   auto · Gemini 4 Neon · xhigh\n";
        let ask = approvable_for(Agent::Antigravity, agy).expect("Antigravity's folder to trust");
        assert_eq!((ask.down, ask.question.as_str()), (0, "Trust /var/tmp/olo-agents-try/trust/agy-new?"));
        assert_eq!(classify(Agent::Antigravity, agy), State::Blocked);
        // Someone moved its mark to the no: theirs.
        let on_no = agy.replace("> Yes, I trust this folder\n  No, exit", "  Yes, I trust this folder\n> No, exit");
        assert!(approvable_for(Agent::Antigravity, &on_no).is_none());
        // Codex: "Yes, continue" under the mark.
        let codex = "> You are in /var/tmp/new\n\n  Do you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection.\n\n› 1. Yes, continue\n  2. No, quit\n\n  Press enter to continue\n";
        assert_eq!(approvable_for(Agent::Codex, codex).map(|a| (a.down, a.question)), Some((0, "Trust /var/tmp/new?".to_owned())));
        // A folder added to a running Claude Code.
        let added = format!("{rule}\n Adding a new directory:\n /var/tmp/other\n Is this a directory you created or one you trust?\n ❯ 1. Yes, add it\n   2. No\n Enter to confirm · Esc to cancel\n");
        assert_eq!(approvable_for(Agent::Claude, &added).map(|a| (a.down, a.question)), Some((0, "Add and trust /var/tmp/other?".to_owned())));
        // A plan waiting on a mode is never answered, whatever is on.
        let plan = " Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n";
        assert!(approvable_for(Agent::Claude, plan).is_none());
    }

    /// Claude Code's read outside the working directories, as 2.1.332
    /// draws it (Ben's picture of a pane where Auto Approve never counted
    /// down, 2026-11-26): Auto Approve may answer it, with its yes for
    /// this once, three below the mark; and Return is right only once
    /// the mark stands there.
    #[test]
    fn auto_approve_answers_a_read_outside_with_its_yes_for_this_once() {
        let dotted = "┆".repeat(40);
        let dialog = |marked: usize| {
            let mark = |i: usize| if i == marked { "❯" } else { " " };
            format!(
                "● Reading the picture\n{rule}\n Read outside the working directories\n{dotted}\n Read(/var/tmp/olo-agents-try/shots/light-2x.png)\n{dotted}\n Auto mode and the sandbox read outside the working directories without asking. Yes or No\n answers for this read; the rest is for later reads. Block: the file tools refuse reads outside\n the working directories in every project. To undo, remove\n permissions.blockReadsOutsideWorkingDirectories from your user settings; file tools follow\n next session, sandboxed commands at once.\n\n Allow this read outside the working directories?\n {} 1. Yes, and keep allowing any reads outside the working directories\n {} 2. No, and block reads outside the working directories from now on\n {} 3. No, and ask again next time\n {} 4. Yes, but ask again next time\n\n Esc to cancel · Tab to amend\n",
                mark(0),
                mark(1),
                mark(2),
                mark(3),
                rule = "─".repeat(40),
            )
        };
        let asked = approvable(&dialog(0)).expect("a question Auto Approve may answer");
        assert_eq!(asked.question, "Allow this read outside the working directories?");
        assert_eq!(asked.down, 3, "its yes for this once is the fourth: never Return on the first, which is kept");
        // The mark moved there: Return now, and it is the same question.
        let there = approvable(&dialog(3)).expect("the same question");
        assert_eq!((there.down, there.key), (0, asked.key));
        // The mark anywhere else is somebody's doing: theirs.
        assert!(approvable(&dialog(1)).is_none());
        assert!(approvable(&dialog(2)).is_none());
        // And as the service reads it: one of each kind.
        let parsed = parse_approval(Agent::Claude, &dialog(0)).expect("read as a permission question");
        assert_eq!(parsed.choices.iter().map(|c| c.kind).collect::<Vec<_>>(), [ChoiceKind::DontAskAgain, ChoiceKind::NoAlways, ChoiceKind::No, ChoiceKind::Once]);
        assert_eq!(classify(Agent::Claude, &dialog(0)), State::Blocked);
        // The questions it always answered are answered as before: Return.
        let plain = "● Bash(touch one.txt)\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for this command\n   3. No, and tell Claude what to do differently (esc)\n";
        assert_eq!(approvable(plain).expect("as before").down, 0);
        // (Its mark moved off the first: the person's.)
        assert!(approvable(&plain.replace(" ❯ 1. Yes", "   1. Yes").replace("   2. Yes, and", " ❯ 2. Yes, and")).is_none());

        // Every agent's question, by the one reader (`approvable_for`).
        let for_claude = approvable_for(Agent::Claude, &dialog(0)).expect("Claude Code's read outside");
        assert_eq!((for_claude.down, for_claude.key), (3, asked.key), "the same question, the same key");
        assert_eq!(approvable_for(Agent::Claude, &dialog(3)).map(|a| a.down), Some(0));
        assert!(approvable_for(Agent::Claude, &dialog(2)).is_none(), "the mark on a no: the person's");
        assert_eq!(approvable_for(Agent::Claude, plain).map(|a| a.down), Some(0));
        let agy = |marked: usize| {
            let mark = |i: usize| if i == marked { ">" } else { " " };
            format!(
                "Command\n────────────────────\nRequesting permission for:\n   rm -f /var/tmp/x\nRun this command?\n{} 1. Yes, run command\n{} 2. Yes, and always allow in this conversation\n{} 3. No, cancel\n  ↑/↓ Navigate\n",
                mark(0),
                mark(1),
                mark(2)
            )
        };
        assert_eq!(approvable_for(Agent::Antigravity, &agy(0)).map(|a| a.down), Some(0));
        assert!(approvable_for(Agent::Antigravity, &agy(1)).is_none(), "moved to the yes that is kept for the conversation: the person's");
        let codex = "  Would you like to run the following command?\n\n  Reason: Do you want to allow listing the folder?\n\n  $ ls -la\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `ls` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        assert_eq!(approvable_for(Agent::Codex, codex).map(|a| a.down), Some(0));
        // A question with no yes for this once: the yes for this conversation, never the kept one.
        let session_only = "Command\n────────────────────\nRequesting permission for:\n   curl example.com\nRun this command?\n> 1. Yes, and always allow for commands that start with 'curl' (Persist to settings.json)\n  2. Yes, and always allow in this conversation\n  3. No, cancel\n  ↑/↓ Navigate\n";
        assert_eq!(approvable_for(Agent::Antigravity, session_only).map(|a| a.down), Some(1));
        // A plan to choose a mode for is never answered (a folder to trust is: its own test).
        let trust = "────────────────────\n Accessing workspace:\n /var/tmp/new\n Quick safety check: Is this a project you created or one you trust?\n ❯ 1. Yes, I trust this folder\n   2. No, exit\n Enter to confirm · Esc to cancel\n";
        assert_eq!(approvable_for(Agent::Claude, trust).map(|a| a.down), Some(0));
        let plan = " Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n";
        assert!(approvable_for(Agent::Claude, plan).is_none());
    }

    /// Claude Code's question about a read outside the working
    /// directories (2.1.33x; ShiftUp's launcher met it live, 2026-11-25):
    /// two yeses and two noes, one of each kept and one for this once.
    #[test]
    fn a_read_outside_has_one_yes_and_one_no_for_this_once() {
        let screen = " Read(/var/tmp/other/notes.md)\n Allow this read?\n \u{276f} 1. Yes, and keep allowing any reads outside the working directories\n   2. No, and block reads\n   3. No, and ask again next time\n   4. Yes, but ask again next time\n Esc to cancel\n";
        let kinds: Vec<ChoiceKind> = ["Yes, and keep allowing any reads outside the working directories", "No, and block reads", "No, and ask again next time", "Yes, but ask again next time"]
            .into_iter()
            .map(classify_choice_kind)
            .collect();
        assert_eq!(kinds, [ChoiceKind::DontAskAgain, ChoiceKind::NoAlways, ChoiceKind::No, ChoiceKind::Once]);
        let asked = parse_approval(Agent::Claude, screen).expect("the question is read");
        assert_eq!(asked.choices.iter().map(|c| c.kind).collect::<Vec<_>>(), kinds);
        assert_eq!(asked.selected_index, Some(0), "the mark stands on the one that is kept: Return alone would keep it");
        assert_eq!(asked.choices.iter().filter(|c| c.kind == ChoiceKind::Once).count(), 1);
        assert_eq!(asked.choices.iter().filter(|c| c.kind == ChoiceKind::No).count(), 1);
        // A kept no in other words; and a plain no is still a no.
        assert_eq!(classify_choice_kind("No, and don\u{2019}t ask again"), ChoiceKind::NoAlways);
        assert_eq!(classify_choice_kind("No, and tell Claude what to do differently (esc)"), ChoiceKind::No);
        assert_eq!(classify_choice_kind("No, cancel"), ChoiceKind::No);
        // Antigravity's yes for this conversation is neither once nor kept in its settings.
        assert_eq!(classify_choice_kind("Yes, and always allow in this conversation"), ChoiceKind::AllowSession);
        assert_eq!(classify_choice_kind("Yes, and always allow for commands that start with 'env' (Persist to settings.json)"), ChoiceKind::DontAskAgain);
    }

    #[test]
    fn a_feedback_survey_is_seen_and_a_message_that_quotes_it_is_not() {
        let rule = "─".repeat(40);
        // Claude Code's, as it draws it over its prompt (2.1.33x).
        let claude = format!("● Done.\n{rule}\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n{rule}\n❯ \n{rule}");
        let seen = parse_survey(Agent::Claude, &claude).expect("Claude Code's survey");
        assert_eq!(seen.dismiss, "0");
        assert!(seen.question.starts_with("How is Claude doing"));
        // Antigravity's, drawn over a permission prompt's last lines.
        let agy =
            "● Bash(for i in 1 2 3; do cargo test …)\n How's the CLI experience so far? Help us improve:\n [1] Good  [2] Fine  [3] Bad  [0] Skip\nesc to cancel · accept-edits · Gemini 4 Neon · xhigh";
        assert_eq!(parse_survey(Agent::Antigravity, agy).expect("Antigravity's survey").dismiss, "0");
        // Each is its own agent's; Codex has none.
        assert!(parse_survey(Agent::Antigravity, &claude).is_none());
        assert!(parse_survey(Agent::Claude, agy).is_none());
        assert!(parse_survey(Agent::Codex, &claude).is_none());
        // A permission prompt isn't a survey, nor a working screen.
        assert!(parse_survey(Agent::Antigravity, "Run this command?\n> 1. Yes, run command\n  4. No, cancel\n  ↑/↓ Navigate").is_none());
        assert!(parse_survey(Agent::Claude, "✻ Working… (esc to interrupt)\n❯ ").is_none());
        // A message that quotes it in a sentence isn't one, nor one that
        // has only its choices, nor one scrolled far up the screen.
        assert!(parse_survey(Agent::Claude, "● The survey reads \"How is Claude doing this session?\" with 1: Bad … 0: Dismiss under it.\n❯ ").is_none());
        assert!(parse_survey(Agent::Claude, "● Ratings:\n  1: Bad    2: Fine   3: Good   0: Dismiss\n❯ ").is_none());
        let far = format!("{claude}\n{}", (0..14).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
        assert!(parse_survey(Agent::Claude, &far).is_none());
    }

    #[test]
    fn audit_hardening_parsers_edge_cases() {
        // 1. Zero-width characters & NBSP inside "don't ask again" must NEVER be `Once`.
        assert_eq!(classify_choice_kind("Yes, and don\u{200B}\u{2019}t\u{00A0}ask again for: env"), ChoiceKind::DontAskAgain);
        assert_eq!(classify_choice_kind("Yes, and something brand new"), ChoiceKind::Other);

        // 2. Column-0 wrapped choice continuations (real `agy` narrow terminal wrapping!) & skipped numbers `1, 2, 4`.
        let agy_col0_wrap = "Command\n────────────────────\nRequesting permission for:\n   echo DENIED_CMD_agy\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with\n'echo DENIED_CMD_agy'\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend\nesc to cancel\n";
        let ap = parse_approval(Agent::Antigravity, agy_col0_wrap).expect("col-0 wrapped agy approval");
        assert_eq!(ap.choices.len(), 3);
        assert_eq!(ap.choices[0].kind, ChoiceKind::Once);
        assert_eq!(ap.choices[1].kind, ChoiceKind::AllowSession);
        assert_eq!(ap.choices[2].kind, ChoiceKind::No);
        assert_eq!(ap.choices[2].number, Some(4));

        // 3. 2-line wrapped Claude edit question.
        let wrapped_q = "Edit file\n────────────────────\nDo you want to make this edit to\ncrates/rex-link/src/agent.rs?\n ❯ 1. Yes\n   2. Yes, allow all edits during this session (shift+tab)\n   3. No\n Esc to cancel\n";
        let ap_wq = parse_approval(Agent::Claude, wrapped_q).expect("2-line wrapped edit question");
        assert_eq!(ap_wq.question, "Do you want to make this edit to crates/rex-link/src/agent.rs?");
        assert_eq!(ap_wq.choices.len(), 3);

        // 4. Codex directory trust prompt detected as FolderTrustPrompt (not ApprovalPrompt, not Working).
        let codex_trust = "You are in /var/tmp/repo\n\nDo you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection.\n\n› 1. Yes, continue\n  2. No, quit\n\nPress enter to continue\n";
        let ct = parse_folder_trust(codex_trust).expect("codex trust");
        assert_eq!(ct.folder.as_deref(), Some("/var/tmp/repo"));
        assert!(!ct.is_add_dir);
        assert!(ct.selected_trust);
        assert!(parse_approval(Agent::Codex, codex_trust).is_none());
        assert_eq!(classify(Agent::Codex, codex_trust), State::Blocked);

        // 5. Ordinary prose containing "Do you want to", "esc to cancel", "enter to confirm", "(y/n)"
        //    must NEVER falsely classify as Blocked or Working or parse_question!
        let claude_prose =
            "● Do you want to keep the old API? Use (y/n) or press Enter to confirm; esc to cancel.\n\n────────────────────────────────\n❯ \n────────────────────────────────\n  ? for shortcuts\n";
        assert_eq!(classify(Agent::Claude, claude_prose), State::Idle);
        assert_eq!(parse_question(Agent::Claude, claude_prose), None);

        let agy_prose = "● Docs mention esc to cancel and ↑/↓ Navigate in old examples.\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n────────────────────────────────\n> Accept-edits mode\n────────────────────────────────\n? for shortcuts\n";
        assert_eq!(classify(Agent::Antigravity, agy_prose), State::Idle);

        // 6. Stale Codex `■ Conversation interrupted` from a prior turn must NEVER poison a later healthy turn!
        let codex_stale_err = "■ Conversation interrupted - tell the model what to do differently.\n\n› Follow-up task\n\n• TURN2_FOLLOWUP_codex_OK\n\n  Worked for 1s\n\n› Ask Codex to do anything\n  gpt-5.4-codex · /var/tmp/work\n";
        assert_eq!(parse_api_error(Agent::Codex, codex_stale_err), None);
        // Active error at tail of current turn IS still detected!
        let codex_live_err = "› Run broken request\n■ stream disconnected before completion: 529 overloaded\n\n› Ask Codex to do anything\n  gpt-5.4-codex · /var/tmp/work\n";
        assert_eq!(parse_api_error(Agent::Codex, codex_live_err).as_deref(), Some("stream disconnected before completion: 529 overloaded"));

        // 7. `parse_usage` ignores ordinary output lines more than 6 lines above the bottom.
        let fake_usage = "Test output: used 30% of the session quota\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n";
        assert!(parse_usage(fake_usage).is_none());
    }
}
