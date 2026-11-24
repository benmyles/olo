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
/// Navigate" under its answers.
pub fn classify(agent: Agent, screen: &str) -> State {
    let s = screen.to_lowercase();
    if agent == Agent::Antigravity {
        return if s.contains("↑/↓ navigate") {
            State::Blocked
        } else if s.contains("esc to cancel") {
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
}
