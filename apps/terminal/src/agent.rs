//! Coding agents in a tab (Claude Code, Codex, Antigravity): which one is in the
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
pub use olo_agent_screen::{Agent, Ask, State, approvable, at_prompt, classify, fingerprint};
use olo_agent_screen::{identify, shared_server};
use olo_kit_base::Proxy;

use crate::{
    io,
    session::{Listener, Nudge},
};

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

/// Whether process `pid` is an agent's server for all its sessions:
/// Codex's app server, one daemon that runs every Codex session's tools
/// and hooks, started by whichever window came first and so a child of
/// that one. What asks through it is not, for that, in the tab its
/// family leads to (`agent_context::Asker`).
pub fn serves_every_session(pid: i32) -> bool {
    let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return false };
    let args: Vec<String> = cmdline.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    shared_server(comm.trim(), &exe, &args)
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
                        let state = classify(agent, &text);
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
