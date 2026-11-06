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

/// How many lines from the bottom of the screen are read.
const LOOK: usize = 30;

fn screen_text(term: &Term<Listener>) -> String {
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
                let settling = list.lock().unwrap().iter().any(|w| w.seen.is_some());
                nudge.wait(settling.then_some(LOOK_EVERY));
                // (At most every LOOK_EVERY, however much is printed.)
                std::thread::sleep((looked + LOOK_EVERY).saturating_duration_since(Instant::now()));
                looked = Instant::now();
                let mut list = list.lock().unwrap();
                list.retain(|w| w.term.strong_count() > 0);
                for w in list.iter_mut() {
                    let (Some(term), Some(shared)) = (w.term.upgrade(), w.shared.upgrade()) else { continue };
                    let mut print = None;
                    let now = shared.foreground().filter(|&p| Some(p) != shared.shell()).and_then(which).map(|agent| {
                        let text = screen_text(&term.lock());
                        print = Some(fingerprint(&text));
                        (agent, classify(&text))
                    });
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
            list.push(Watched { shell, term: Arc::downgrade(term), shared: Arc::downgrade(shared), reported: None, seen: None, print: 0 });
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
