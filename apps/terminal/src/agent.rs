//! Coding agents in a tab (Claude Code, Codex): which one is in the
//! foreground and what it's doing, read as a person would, from the
//! program running and what's on its screen. A thread looks at every tab
//! (shown or not) when a shell has printed, at most every 600 ms, and
//! reports changes to the app; with nothing printed it sleeps.

use std::{
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line},
    sync::FairMutex,
    term::Term,
};
use olo_kit_base::Proxy;

use crate::{
    io,
    session::{Listener, Nudge},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The agent in process group `pgrp`, if it is one: by its program
/// (Claude Code's native build runs from `…/claude/versions/<version>`)
/// or its arguments (the npm builds run under node).
pub fn which(pgrp: i32) -> Option<Agent> {
    let cmdline = std::fs::read(format!("/proc/{pgrp}/cmdline")).ok()?;
    let args: Vec<String> = cmdline.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    let exe = std::fs::read_link(format!("/proc/{pgrp}/exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let comm = std::fs::read_to_string(format!("/proc/{pgrp}/comm")).unwrap_or_default();
    identify(comm.trim(), &exe, &args)
}

fn identify(comm: &str, exe: &str, args: &[String]) -> Option<Agent> {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    let names: Vec<String> = [comm.to_owned(), base(exe)].into_iter().chain(args.iter().take(2).map(|a| base(a))).collect();
    if exe.contains("/claude/versions/") || names.iter().any(|n| n == "claude") || args.iter().any(|a| a.contains("@anthropic-ai/claude-code")) {
        return Some(Agent::Claude);
    }
    if names.iter().any(|n| n == "codex") || args.iter().any(|a| a.contains("@openai/codex")) {
        return Some(Agent::Codex);
    }
    None
}

/// What the agent is doing, from the bottom of its screen (where its
/// prompt, dialogs and status line are).
pub fn classify(screen: &str) -> State {
    let s = screen.to_lowercase();
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
/// there would have said yes for the person.
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
/// Not a plan waiting to be approved (Claude Code's "No, keep planning"):
/// that answer is the person's.
pub fn approvable(screen: &str) -> Option<Ask> {
    let lines: Vec<&str> = screen.lines().collect();
    let q = lines.iter().rposition(|l| {
        let t = l.trim().to_lowercase();
        (t.starts_with("do you want to ") || t.starts_with("would you like to ")) && t.ends_with('?')
    })?;
    let rest = &lines[q + 1..];
    // Its first answer (Codex shows the command between): "1. …", chosen.
    let at = rest.iter().position(|l| l.trim().trim_start_matches(['❯', '›']).trim_start().starts_with("1. "))?;
    // (Its own: no other question between; but for the reason Codex gives
    // for a command, which it's told to put as a question, "Reason: Do you
    // want to …?": Auto Approve never answered Codex.)
    if rest[..at].iter().map(|l| l.trim()).any(|l| l.ends_with('?') && !l.starts_with("Reason:")) {
        return None;
    }
    let first = rest[at].trim();
    let chosen = first.strip_prefix('❯').or_else(|| first.strip_prefix('›'))?.trim_start();
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

/// How many lines from the bottom of the screen are read.
const LOOK: usize = 30;

/// The bottom of `term`'s screen (where an agent's prompt, dialogs and
/// status line are), as text.
pub fn screen_text(term: &Term<Listener>) -> String {
    let grid = term.grid();
    let (rows, cols) = (term.screen_lines(), term.columns());
    let mut text = String::new();
    for l in rows.saturating_sub(LOOK)..rows {
        let row = &grid[Line(l as i32)];
        let line: String = (0..cols).map(|c| row[Column(c)].c).collect();
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

/// A change: shell `.0` now runs `.1` (or no agent).
pub struct Report(pub u64, pub Option<(Agent, State)>);

/// Shell `.0`'s agent's screen changed (Keep It Going: anything at all
/// is activity, but a clock that counts by itself).
pub struct Activity(pub u64);

/// Shell `.0`'s agent asks a question Auto Approve may answer (`approvable`),
/// another than before, or none now: sent as it changes.
pub struct Asking(pub u64, pub Option<Ask>);

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

struct Watched {
    shell: u64,
    term: Weak<FairMutex<Term<Listener>>>,
    shared: Weak<io::Shared>,
    reported: Option<(Agent, State)>,
    /// A different reading, and how many times running it's been seen.
    seen: Option<(Option<(Agent, State)>, u32)>,
    /// What its agent's screen last showed (`fingerprint`).
    print: u64,
    /// The question last told of (`Asking`), by its key.
    asked: Option<u64>,
    /// Another reading of it, and how many times running: told once read
    /// twice (a screen read halfway through being drawn isn't a change).
    ask_seen: Option<(Option<u64>, u32)>,
}

/// How often the tabs are looked at, at most (while shells print, and
/// while a reading settles).
const LOOK_EVERY: Duration = Duration::from_millis(600);

/// Looks at the tabs it's given until they go: when a shell has printed
/// (what it looks for shows on the screen), and while a reading settles;
/// otherwise it sleeps.
#[derive(Clone)]
pub struct Watcher {
    list: Arc<Mutex<Vec<Watched>>>,
    nudge: Nudge,
}

impl Watcher {
    pub fn start(proxy: Proxy) -> Watcher {
        let list: Arc<Mutex<Vec<Watched>>> = Arc::default();
        let nudge = Nudge::default();
        crate::session::on_output(nudge.clone());
        let w = Watcher { list: list.clone(), nudge: nudge.clone() };
        let _ = std::thread::Builder::new().name("agents".into()).spawn(move || {
            let mut looked = Instant::now();
            loop {
                let settling = list.lock().unwrap().iter().any(|w| w.seen.is_some() || w.ask_seen.is_some());
                nudge.wait(settling.then_some(LOOK_EVERY));
                // (At most every LOOK_EVERY, however much is printed.)
                std::thread::sleep((looked + LOOK_EVERY).saturating_duration_since(Instant::now()));
                looked = Instant::now();
                let mut list = list.lock().unwrap();
                list.retain(|w| w.term.strong_count() > 0);
                for w in list.iter_mut() {
                    let (Some(term), Some(shared)) = (w.term.upgrade(), w.shared.upgrade()) else { continue };
                    let (mut print, mut ask) = (None, None);
                    let now = shared.foreground().filter(|&p| Some(p) != shared.shell()).and_then(which).map(|agent| {
                        let text = screen_text(&term.lock());
                        print = Some(fingerprint(&text));
                        let state = classify(&text);
                        // (Only on a question: nothing more read otherwise.)
                        if state == State::Blocked {
                            ask = approvable(&text);
                        }
                        (agent, state)
                    });
                    let key = ask.as_ref().map(|a| a.key);
                    // (What each look found, for `RUST_LOG=olo_terminal::agent=debug`:
                    // a question that never settles shows as its key changing.)
                    tracing::debug!(shell = w.shell, ?now, ?key, "agents: looked");
                    if key == w.asked {
                        w.ask_seen = None;
                    } else {
                        match w.ask_seen {
                            Some((k, n)) if k == key && n >= 1 => {
                                w.asked = key;
                                w.ask_seen = None;
                                proxy.send(Asking(w.shell, ask));
                            }
                            Some((k, n)) if k == key => w.ask_seen = Some((k, n + 1)),
                            _ => w.ask_seen = Some((key, 1)),
                        }
                    }
                    if let Some(p) = print
                        && p != w.print
                    {
                        w.print = p;
                        proxy.send(Activity(w.shell));
                    }
                    if now == w.reported {
                        w.seen = None;
                        continue;
                    }
                    // Settle first: between steps the status line can go
                    // for a moment, which isn't the turn ending.
                    let count = match w.seen {
                        Some((s, n)) if s == now => n + 1,
                        _ => 1,
                    };
                    let need = match now {
                        Some((_, State::Idle)) | None => 3,
                        _ => 1,
                    };
                    if count >= need {
                        w.reported = now;
                        w.seen = None;
                        proxy.send(Report(w.shell, now));
                    } else {
                        w.seen = Some((now, count));
                    }
                }
            }
        });
        w
    }

    pub fn watch(&self, shell: u64, term: &Arc<FairMutex<Term<Listener>>>, shared: &Arc<io::Shared>) {
        let mut list = self.list.lock().unwrap();
        if !list.iter().any(|w| w.shell == shell) {
            list.push(Watched { shell, term: Arc::downgrade(term), shared: Arc::downgrade(shared), reported: None, seen: None, print: 0, asked: None, ask_seen: None });
            self.nudge.poke();
        }
    }
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
        assert_eq!(classify(codex), State::Blocked);
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
        assert_eq!(classify(working), State::Working);
        let asking = " Bash command\n   rm -rf build\n Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)\n";
        assert_eq!(classify(asking), State::Blocked);
        let idle = "● Done: the tests pass.\n\n╭────────╮\n│ >      │\n╰────────╯\n  ? for shortcuts\n";
        assert_eq!(classify(idle), State::Idle);
        let codex = "• Working (4s • esc to interrupt)\n▌ \n";
        assert_eq!(classify(codex), State::Working);
        let codex_ask = "Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n";
        assert_eq!(classify(codex_ask), State::Blocked);
    }
}
