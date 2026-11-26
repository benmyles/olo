//! Coding agents in a pane: what they are doing (the server's word, OSC
//! 7501, else read from the screen), and Keep It Going and Auto Approve as
//! Terminal has them, reading the screen as Terminal does
//! (`olo_agent_screen`).

use std::time::{Duration, Instant};

pub use olo_agent_screen::{AUTO_APPROVE_AFTER, Agent, KEEP_GOING_EVERY, KEEP_GOING_QUIET, State};
use olo_agent_screen::{approvable_for, at_prompt, classify, fingerprint};
use olo_rex_client::{ProgramPriority, ProgramStatusRecord};

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

/// What the person turned on for a pane: Keep It Going (Terminal's: a
/// coding agent told to go on, in its own prompt, once its turn has been
/// over and its screen quiet `KEEP_GOING_QUIET`, at most once an hour)
/// and Auto Approve (a question asking leave to go on, "1. Yes" chosen,
/// answered Yes `AUTO_APPROVE_AFTER` after it shows). Kept by the hub by
/// the pane's key, so a pane reattached has them still, and saved with
/// the app for an upgrade.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Help {
    #[serde(default)]
    pub keep_going: bool,
    #[serde(default)]
    pub auto_approve: bool,
    /// The question the person stopped (its key): still theirs when
    /// seen again.
    #[serde(default)]
    pub theirs: Option<u64>,
}

impl Help {
    pub fn any(&self) -> bool {
        self.keep_going || self.auto_approve
    }
}

/// Which coding agent a pane's program is: by its program (its name, its
/// file, or its arguments: an npm build runs under node, Claude Code's
/// own build is named by its version), else by the words its screen
/// shows.
pub fn which_agent(program: Option<&str>, exe: &str, args: &[String], screen: &str) -> Option<Agent> {
    if let Some(agent) = program.and_then(|name| olo_agent_screen::identify(name, exe, args)) {
        return Some(agent);
    }
    // (Only where the program could be one: a shell printing these words is none.)
    let shell = program.is_some_and(|p| matches!(p, "bash" | "zsh" | "sh" | "fish" | "dash" | "tmux" | "screen" | "vim" | "nvim" | "less" | "cat"));
    match () {
        _ if shell => None,
        _ if screen.contains("Claude Code") => Some(Agent::Claude),
        _ if screen.contains("OpenAI Codex") => Some(Agent::Codex),
        _ => None,
    }
}

/// The question Auto Approve will answer, while it waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// Which question (`olo_agent_screen::Ask::key`).
    pub key: u64,
    pub question: String,
    /// When it answers.
    pub due: Instant,
    /// The person typed in the pane, or pressed Stop: it's theirs.
    pub stopped: bool,
    /// Looks once due that didn't find it (a screen read as it's drawn).
    misses: u32,
    /// Its mark was moved down to its yes for this once (a question
    /// whose yes isn't the first answer): Return follows only once the
    /// mark is seen to stand there.
    moved: bool,
}

/// What a pane's agent is doing, as Keep It Going and Auto Approve see
/// it between one look at its screen and the next.
#[derive(Debug, Clone)]
pub struct Watch {
    pub agent: Option<Agent>,
    pub state: Option<State>,
    /// The screen, as `olo_agent_screen::fingerprint` tells a change.
    print: u64,
    /// When something last happened: the screen changed, or someone typed.
    since: Instant,
    /// When Keep It Going last looked and found nothing to do.
    looked: Option<Instant>,
    /// When Keep It Going last said its message (seconds since the epoch).
    said: Option<u64>,
    pub pending: Option<Pending>,
    /// How many questions it has answered.
    pub approved: u32,
}

/// What a pane is to be sent, by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Do {
    /// Return: Yes, the answer chosen.
    Return,
    /// The mark moved down this many answers, to the yes for this once
    /// (Claude Code's read outside the working directories: the fourth).
    /// The screen is looked at again before anything else is sent.
    Down(usize),
    /// Keep It Going's message, then Return a moment after.
    Say(String),
}

impl Watch {
    pub fn new(now: Instant) -> Watch {
        Watch { agent: None, state: None, print: 0, since: now, looked: None, said: None, pending: None, approved: 0 }
    }

    /// Its screen read again (`agent`: which agent runs there): what it
    /// is doing, whether anything changed on it, and the question to
    /// answer, if it asks one Auto Approve may answer. Whether the
    /// question waiting changed (for the countdown).
    pub fn looked(&mut self, agent: Option<Agent>, screen: &str, help: &Help, after: Duration, now: Instant) -> bool {
        self.agent = agent;
        self.state = agent.map(|a| classify(a, screen));
        let print = fingerprint(screen);
        if print != self.print {
            self.print = print;
            self.since = now;
        }
        let before = self.pending.as_ref().map(|p| (p.key, p.stopped));
        match (help.auto_approve, self.state, agent.and_then(|a| approvable_for(a, screen))) {
            (true, Some(State::Blocked), Some(ask)) => {
                if self.pending.as_ref().is_none_or(|p| p.key != ask.key) {
                    self.pending = Some(Pending { key: ask.key, question: ask.question, due: now + after, stopped: help.theirs == Some(ask.key), misses: 0, moved: false });
                }
            }
            // (Asking, but not yet read whole: the one waiting stays.)
            (true, Some(State::Blocked), None) => {}
            _ => self.pending = None,
        }
        before != self.pending.as_ref().map(|p| (p.key, p.stopped))
    }

    /// The person typed in the pane: what is on it is theirs (a question
    /// waiting, its key), and Keep It Going waits from now.
    pub fn typed(&mut self, now: Instant) -> Option<u64> {
        self.since = now;
        self.stop()
    }

    /// Stop pressed: the question waiting is the person's (its key).
    pub fn stop(&mut self) -> Option<u64> {
        let p = self.pending.as_mut().filter(|p| !p.stopped)?;
        p.stopped = true;
        Some(p.key)
    }

    /// The question it will answer and how long till it does, while it
    /// waits on its own.
    pub fn counting(&self, now: Instant) -> Option<(&str, Duration)> {
        self.pending.as_ref().filter(|p| !p.stopped).map(|p| (p.question.as_str(), p.due.saturating_duration_since(now)))
    }

    /// When it is next to look on its own: the question's answer, or the
    /// end of Keep It Going's quiet (and of its hour). None: nothing to
    /// wait for.
    pub fn next(&self, help: &Help, quiet: Duration, now: Instant, epoch: u64) -> Option<Instant> {
        let approve = self.pending.as_ref().filter(|p| help.auto_approve && !p.stopped).map(|p| p.due);
        let keep = help.keep_going.then(|| {
            let mut at = self.since + quiet;
            if let Some(looked) = self.looked {
                at = at.max(looked + quiet);
            }
            if let Some(said) = self.said {
                at = at.max(now + Duration::from_secs((said + KEEP_GOING_EVERY.as_secs()).saturating_sub(epoch)));
            }
            at
        });
        [approve, keep].into_iter().flatten().min()
    }

    /// Its time has come (`next`): what to send the pane, read from its
    /// screen as it is now. Auto Approve answers only the same question,
    /// still asked, its yes for this once under the mark (moved there
    /// first where it isn't the first answer, and Return only once it is
    /// seen there); Keep It Going speaks only into the
    /// agent's own prompt (a chooser would take its Return as a choice),
    /// its turn over and the screen quiet `quiet`, at most once an hour.
    pub fn due(&mut self, help: &Help, screen: &str, quiet: Duration, message: &str, now: Instant, epoch: u64) -> Option<Do> {
        if let Some(p) = self.pending.as_mut().filter(|p| help.auto_approve && !p.stopped && now >= p.due) {
            match self.agent.and_then(|a| approvable_for(a, screen)).filter(|a| a.key == p.key) {
                Some(ask) if ask.down == 0 => {
                    self.pending = None;
                    self.approved += 1;
                    return Some(Do::Return);
                }
                // Its yes is further down: the mark goes there, once,
                // and the screen is read again before Return.
                Some(ask) if !p.moved => {
                    p.moved = true;
                    p.due = now + Duration::from_millis(350);
                    return Some(Do::Down(ask.down));
                }
                // (Moved, and not seen there: read again, a few times.)
                _ => {}
            }
            p.misses += 1;
            if p.misses >= 3 {
                self.pending = None;
            } else {
                p.due = now + Duration::from_millis(200);
            }
            return None;
        }
        let quiet_enough = now.saturating_duration_since(self.since) >= quiet;
        let hour_over = self.said.is_none_or(|said| epoch.saturating_sub(said) >= KEEP_GOING_EVERY.as_secs());
        if help.keep_going && quiet_enough && hour_over {
            let at_prompt = self.agent.is_some_and(|a| classify(a, screen) == State::Idle && at_prompt(a, screen));
            if at_prompt && !message.trim().is_empty() {
                self.said = Some(epoch);
                self.since = now;
                self.looked = None;
                return Some(Do::Say(message.to_owned()));
            }
            self.looked = Some(now);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "────────────────────────────────────────";

    /// Claude Code asking leave to run a command, "1. Yes" chosen.
    fn claude_asks(what: &str) -> String {
        format!("● Bash({what})\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for this command\n   3. No, and tell Claude what to do differently (esc)\n")
    }

    /// Claude Code at its own prompt, its turn over.
    fn claude_at_rest(said: &str) -> String {
        format!("● {said}\n\n{RULE}\n❯ \n{RULE}\n  ? for shortcuts\n")
    }

    /// Claude Code's read outside the working directories (2.1.332): a
    /// kept yes first, under the mark, and the yes for this once fourth.
    fn claude_asks_to_read(marked: usize) -> String {
        let mark = |i: usize| if i == marked { "❯" } else { " " };
        format!(
            "● Read(/var/tmp/shots/a.png)\n{RULE}\n Read outside the working directories\n Read(/var/tmp/shots/a.png)\n\n Allow this read outside the working directories?\n {} 1. Yes, and keep allowing any reads outside the working directories\n {} 2. No, and block reads outside the working directories from now on\n {} 3. No, and ask again next time\n {} 4. Yes, but ask again next time\n\n Esc to cancel · Tab to amend\n",
            mark(0),
            mark(1),
            mark(2),
            mark(3)
        )
    }

    /// Every permission question counts down, whatever its shape (Ben,
    /// 2026-11-26: "when it's on it should auto approve everything after
    /// a 30s countdown"). One whose yes for this once isn't the first
    /// answer has the mark moved to it when its time comes, and Return
    /// only once the mark is seen there: never Return on a yes that is
    /// kept.
    #[test]
    fn a_question_whose_yes_is_further_down_is_answered_with_it() {
        let t0 = Instant::now();
        let help = Help { auto_approve: true, ..Help::default() };
        let after = Duration::from_secs(30);
        let mut w = Watch::new(t0);
        assert!(w.looked(Some(Agent::Claude), &claude_asks_to_read(0), &help, after, t0), "it counts down");
        assert_eq!(w.counting(t0).map(|(q, left)| (q.to_owned(), left.as_secs())), Some(("Allow this read outside the working directories?".into(), 30)));
        // Its time: the mark goes down three, and nothing else yet.
        assert_eq!(w.due(&help, &claude_asks_to_read(0), KEEP_GOING_QUIET, "", t0 + after, 1_000), Some(Do::Down(3)));
        assert_eq!(w.approved, 0);
        // (The screen read again as the mark moves: the same question, still waiting.)
        assert!(!w.looked(Some(Agent::Claude), &claude_asks_to_read(3), &help, after, t0 + after));
        let soon = t0 + after + Duration::from_millis(350);
        assert_eq!(w.next(&help, KEEP_GOING_QUIET, soon, 1_000), Some(soon));
        // Seen there: Return.
        assert_eq!(w.due(&help, &claude_asks_to_read(3), KEEP_GOING_QUIET, "", soon, 1_000), Some(Do::Return));
        assert_eq!((w.approved, w.pending.is_none()), (1, true));

        // The mark that doesn't come to stand there is never answered with Return.
        let mut w = Watch::new(t0);
        w.looked(Some(Agent::Claude), &claude_asks_to_read(0), &help, after, t0);
        assert_eq!(w.due(&help, &claude_asks_to_read(0), KEEP_GOING_QUIET, "", t0 + after, 1_000), Some(Do::Down(3)));
        let mut at = soon;
        for _ in 0..3 {
            assert_eq!(w.due(&help, &claude_asks_to_read(0), KEEP_GOING_QUIET, "", at, 1_000), None, "not moved twice, and no Return");
            at += Duration::from_millis(200);
        }
        assert!(w.pending.is_none(), "given up: the person's");
        assert_eq!(w.approved, 0);
        // A mark someone moved (onto a no) isn't a question Auto Approve answers.
        let mut w = Watch::new(t0);
        assert!(!w.looked(Some(Agent::Claude), &claude_asks_to_read(2), &help, after, t0));
        assert!(w.pending.is_none());
    }

    /// A folder to trust counts down and is answered with its yes (Ben,
    /// 2026-11-26: "i want folder trust auto approved"): Claude Code's
    /// mark starts on "No, exit", so it is moved down one first and
    /// Return sent once it is seen on the yes. A plan never counts down.
    #[test]
    fn a_folder_to_trust_is_answered_and_a_plan_is_not() {
        let trust = |on_yes: bool| {
            let (no, yes) = if on_yes { ("  ", "❯ ") } else { ("❯ ", "  ") };
            format!(
                "{}\n Accessing workspace:\n /var/tmp/new\n Quick safety check: Is this a project you created or one you trust?\n {no}No, exit\n {yes}Yes, I trust this folder\n Enter to confirm · Esc to cancel\n",
                "─".repeat(40)
            )
        };
        let t0 = Instant::now();
        let help = Help { auto_approve: true, ..Help::default() };
        let after = Duration::from_secs(30);
        let mut w = Watch::new(t0);
        assert!(w.looked(Some(Agent::Claude), &trust(false), &help, after, t0), "it counts down");
        assert_eq!(w.counting(t0).map(|(q, left)| (q.to_owned(), left.as_secs())), Some(("Trust /var/tmp/new?".into(), 30)));
        assert_eq!(w.due(&help, &trust(false), KEEP_GOING_QUIET, "", t0 + after - Duration::from_secs(1), 1_000), None, "not before its time");
        assert_eq!(w.due(&help, &trust(false), KEEP_GOING_QUIET, "", t0 + after, 1_000), Some(Do::Down(1)));
        assert!(!w.looked(Some(Agent::Claude), &trust(true), &help, after, t0 + after), "the same question with its mark moved");
        let soon = t0 + after + Duration::from_millis(350);
        assert_eq!(w.due(&help, &trust(true), KEEP_GOING_QUIET, "", soon, 1_000), Some(Do::Return));
        assert_eq!(w.approved, 1);
        // With Auto Approve off, nothing.
        let mut w = Watch::new(t0);
        assert!(!w.looked(Some(Agent::Claude), &trust(false), &Help::default(), after, t0));
        // A plan to choose a mode for is the person's, on or off.
        let plan = " Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n";
        let mut w = Watch::new(t0);
        assert!(!w.looked(Some(Agent::Claude), plan, &help, after, t0));
        assert!(w.pending.is_none());
        assert_eq!(w.due(&help, plan, KEEP_GOING_QUIET, "", t0 + after, 1_000), None);
    }

    /// A question Auto Approve may answer is answered Yes once its time
    /// has come, if it is still the one asked; not before.
    #[test]
    fn a_question_is_answered_when_its_time_comes() {
        let t0 = Instant::now();
        let help = Help { auto_approve: true, ..Help::default() };
        let after = Duration::from_secs(30);
        let mut w = Watch::new(t0);
        assert!(w.looked(Some(Agent::Claude), &claude_asks("touch x"), &help, after, t0));
        assert_eq!(w.next(&help, KEEP_GOING_QUIET, t0, 1_000), Some(t0 + after));
        assert_eq!(w.counting(t0 + Duration::from_secs(10)).map(|(q, left)| (q.to_owned(), left.as_secs())), Some(("Do you want to proceed?".into(), 20)));
        assert_eq!(w.due(&help, &claude_asks("touch x"), KEEP_GOING_QUIET, "go on", t0 + Duration::from_secs(10), 1_000), None, "not before");
        assert_eq!(w.due(&help, &claude_asks("touch x"), KEEP_GOING_QUIET, "go on", t0 + after, 1_000), Some(Do::Return));
        assert_eq!((w.approved, w.pending.is_none()), (1, true));
        // Another question, then another in its place before its time: the later one waits its whole time.
        w.looked(Some(Agent::Claude), &claude_asks("touch y"), &help, after, t0 + after);
        w.looked(Some(Agent::Claude), &claude_asks("rm -rf build"), &help, after, t0 + after + Duration::from_secs(5));
        assert_eq!(w.due(&help, &claude_asks("touch y"), KEEP_GOING_QUIET, "", t0 + after * 2, 1_000), None, "a question that went isn't answered");
        assert_eq!(w.due(&help, &claude_asks("rm -rf build"), KEEP_GOING_QUIET, "", t0 + after * 2 + Duration::from_secs(5), 1_000), Some(Do::Return));
        // Off: nothing waits.
        let off = Help::default();
        let mut w = Watch::new(t0);
        assert!(!w.looked(Some(Agent::Claude), &claude_asks("touch x"), &off, after, t0));
        assert_eq!(w.next(&off, KEEP_GOING_QUIET, t0, 1_000), None);
    }

    /// Typed into, or stopped, a question is the person's, and stays
    /// theirs when it shows again; one that goes isn't waited on.
    #[test]
    fn a_question_typed_into_or_stopped_is_the_persons() {
        let t0 = Instant::now();
        let mut help = Help { auto_approve: true, ..Help::default() };
        let after = Duration::from_secs(30);
        let mut w = Watch::new(t0);
        w.looked(Some(Agent::Claude), &claude_asks("touch x"), &help, after, t0);
        let key = w.typed(t0 + Duration::from_secs(3)).expect("its key");
        assert_eq!(w.counting(t0), None);
        assert_eq!(w.due(&help, &claude_asks("touch x"), KEEP_GOING_QUIET, "", t0 + after, 1_000), None);
        // (Kept as theirs: seen again after a restart, still theirs.)
        help.theirs = Some(key);
        let mut again = Watch::new(t0);
        again.looked(Some(Agent::Claude), &claude_asks("touch x"), &help, after, t0);
        assert_eq!(again.counting(t0), None);
        // Stop, the same.
        help.theirs = None;
        let mut w = Watch::new(t0);
        w.looked(Some(Agent::Claude), &claude_asks("touch z"), &help, after, t0);
        assert!(w.stop().is_some());
        assert_eq!(w.next(&help, KEEP_GOING_QUIET, t0, 1_000), None);
        // The question answered by the person: nothing waits.
        let mut w = Watch::new(t0);
        w.looked(Some(Agent::Claude), &claude_asks("touch q"), &help, after, t0);
        assert!(w.looked(Some(Agent::Claude), &claude_at_rest("Done."), &help, after, t0 + Duration::from_secs(2)));
        assert_eq!(w.pending, None);
    }

    /// Keep It Going speaks into the agent's own prompt once its screen
    /// has been quiet long enough, at most once an hour; never into a
    /// question (its Return would choose an answer).
    #[test]
    fn keep_it_going_speaks_only_into_its_prompt_when_quiet() {
        let t0 = Instant::now();
        let help = Help { keep_going: true, ..Help::default() };
        let quiet = Duration::from_secs(120);
        let rest = claude_at_rest("Done with the first part.");
        let mut w = Watch::new(t0);
        w.looked(Some(Agent::Claude), &rest, &help, AUTO_APPROVE_AFTER, t0);
        assert_eq!(w.next(&help, quiet, t0, 1_000), Some(t0 + quiet));
        assert_eq!(w.due(&help, &rest, quiet, "keep going", t0 + Duration::from_secs(60), 1_000), None, "not quiet long enough");
        assert_eq!(w.due(&help, &rest, quiet, "keep going", t0 + quiet, 1_000), Some(Do::Say("keep going".into())));
        // Within the hour: not again, however quiet.
        let later = t0 + quiet * 3;
        assert_eq!(w.due(&help, &rest, quiet, "keep going", later, 1_000 + 600), None);
        assert!(w.next(&help, quiet, later, 1_000 + 600).is_some_and(|at| at >= later + Duration::from_secs(3_000)));
        // Asking a question: not into it; looked again a quiet later, not at once.
        let mut w = Watch::new(t0);
        let asks = claude_asks("touch x");
        w.looked(Some(Agent::Claude), &asks, &help, AUTO_APPROVE_AFTER, t0);
        assert_eq!(w.due(&help, &asks, quiet, "keep going", t0 + quiet, 1_000), None);
        assert_eq!(w.next(&help, quiet, t0 + quiet, 1_000), Some(t0 + quiet * 2), "no looking again at once");
        // A change on the screen begins the quiet again.
        let mut w = Watch::new(t0);
        w.looked(Some(Agent::Claude), &rest, &help, AUTO_APPROVE_AFTER, t0);
        w.looked(Some(Agent::Claude), &claude_at_rest("Another line."), &help, AUTO_APPROVE_AFTER, t0 + Duration::from_secs(100));
        assert_eq!(w.due(&help, &claude_at_rest("Another line."), quiet, "go", t0 + quiet, 1_000), None);
        // No agent: nothing said.
        let mut w = Watch::new(t0);
        w.looked(None, &rest, &help, AUTO_APPROVE_AFTER, t0);
        assert_eq!(w.due(&help, &rest, quiet, "go", t0 + quiet, 1_000), None);
    }

    /// Which agent a pane's program is: by its name or its arguments,
    /// else by the words on its screen, but never a shell's.
    #[test]
    fn which_agent_a_program_is() {
        let none: &[String] = &[];
        assert_eq!(which_agent(Some("claude"), "", none, ""), Some(Agent::Claude));
        assert_eq!(which_agent(Some("2.1.4"), "/home/u/.local/share/claude/versions/2.1.4", none, ""), Some(Agent::Claude));
        assert_eq!(which_agent(Some("codex"), "", none, ""), Some(Agent::Codex));
        assert_eq!(which_agent(Some("agy"), "", none, ""), Some(Agent::Antigravity));
        assert_eq!(which_agent(Some("node"), "", &["node".into(), "/usr/lib/node_modules/@openai/codex/bin/codex.js".into()], ""), Some(Agent::Codex));
        assert_eq!(which_agent(Some("node"), "", none, "╭ Claude Code v2 ╮"), Some(Agent::Claude));
        assert_eq!(which_agent(Some("bash"), "", none, "$ echo Claude Code"), None);
        assert_eq!(which_agent(Some("vim"), "", none, "OpenAI Codex"), None);
    }

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
