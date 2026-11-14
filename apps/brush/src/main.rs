//! brush, the bash-compatible shell written in Rust
//! (github.com/reubeno/brush), built with Olo: installed with Olo's
//! programs and upgraded with them, and what Olo Doctor suggests as the
//! login shell (copied to /usr/local/bin/brush, listed in /etc/shells).
//!
//! Olo's own: with Preferences ▸ Terminal ▸ "Suggest commands with
//! Olo's model" on, the hint after what's typed is olo-complete's (the
//! model of the person's history: this folder, this shell's commands
//! before, the hour and the day), and each line entered is told to it to
//! learn from. Off, or with olo-complete not answering in 8 ms, brush's
//! own (the history's) — so the shell never waits on it.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use brush_interactive::hooks::{Hooks, InfoFn, ShellInfo};
use olo_complete::protocol::{self, Reply, Request, Settings};
use reedline::{DefaultHinter, Hinter, History};

/// How long a hint may take before the shell's own is shown.
const WAIT: Duration = Duration::from_millis(8);

/// Preferences' setting, read again when its file changes.
#[derive(Default)]
struct Setting {
    on: bool,
    stamp: Option<SystemTime>,
    checked: Option<Instant>,
}

impl Setting {
    fn on(&mut self) -> bool {
        // (A stat at most every quarter second: typing fast asks often.)
        if self.checked.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return self.on;
        }
        self.checked = Some(Instant::now());
        let stamp = std::fs::metadata(protocol::settings_path()).and_then(|m| m.modified()).ok();
        if stamp != self.stamp {
            self.stamp = stamp;
            self.on = Settings::load().enabled;
        }
        self.on
    }
}

/// What the hinter and the line hook share: the setting, the line a hint
/// taken made (whole), when olo-complete was started from here.
#[derive(Default)]
struct Shared {
    setting: Setting,
    taken: Option<String>,
    started: Option<Instant>,
}

struct Olo {
    shared: Arc<Mutex<Shared>>,
}

impl Olo {
    /// The shell's session for olo-complete: in Olo's Terminal its tab's
    /// shell id (`OLO_SHELL_ID`: olo-termd keeps its recent text under it,
    /// `protocol::tail_path`), else this process.
    fn session() -> u64 {
        IDS.get().and_then(|i| i.0).unwrap_or_else(|| u64::from(std::process::id()))
    }

    /// A shell standing in for one of an earlier session (a tab restored
    /// after a restart) says so once, before its first suggestion or
    /// command: its commands so far are the old one's.
    fn continues() {
        static TOLD: std::sync::Once = std::sync::Once::new();
        TOLD.call_once(|| {
            if let Some(from) = IDS.get().and_then(|i| i.1) {
                protocol::tell(&Request::Continues { session: Olo::session(), from });
            }
        });
    }
}

impl Hooks for Olo {
    fn hinter(&self, info: InfoFn) -> Option<Box<dyn Hinter>> {
        let style = nu_ansi_term::Style::new().italic().fg(nu_ansi_term::Color::DarkGray);
        Some(Box::new(ModelHinter { shared: self.shared.clone(), info, own: DefaultHinter::default().with_style(style), style, line: String::new(), hint: String::new(), mine: false }))
    }

    fn entered(&self, line: &str, info: &ShellInfo) {
        let mut s = self.shared.lock().unwrap();
        if !s.setting.on() {
            return;
        }
        let taken = s.taken.take();
        drop(s);
        Olo::continues();
        let session = Olo::session();
        let cwd = Some(info.working_dir.to_string_lossy().into_owned());
        // How the last command ended (known now, at the next prompt).
        protocol::tell(&Request::Ended { session, status: i32::from(info.last_status) });
        if taken.as_deref() == Some(line.trim_end()) {
            protocol::tell(&Request::Accepted { session, whole: true });
        }
        protocol::tell(&Request::Ran { command: line.to_owned(), cwd, session });
    }

    /// brush keeps its working directory for what it runs, not as its
    /// own process's: `/proc/PID/cwd` stayed where it started, and
    /// Terminal (a tab's folder, its repository and worktree, where a new
    /// tab opens) and anything else reading it saw that (Ben, 2026-11-04:
    /// a tab `cd`'d into a worktree wasn't seen as in it). The process
    /// goes where the shell is at each prompt, as other shells do on `cd`.
    fn prompt(&self, info: &ShellInfo) {
        if std::env::current_dir().ok().as_deref() != Some(info.working_dir.as_path()) {
            let _ = std::env::set_current_dir(&info.working_dir);
        }
    }
}

/// The hint: olo-complete's when it's on and answers, else brush's own.
struct ModelHinter {
    shared: Arc<Mutex<Shared>>,
    info: InfoFn,
    own: DefaultHinter,
    style: nu_ansi_term::Style,
    /// The line the hint is for, and the hint.
    line: String,
    hint: String,
    /// The hint shown is olo-complete's (else the history's).
    mine: bool,
}

impl ModelHinter {
    /// olo-complete's hint for `line`, if it's on and answers in time
    /// (started, once a minute at most, if it isn't running).
    fn ask(&mut self, line: &str) -> Option<String> {
        if !self.shared.lock().unwrap().setting.on() {
            return None;
        }
        let cwd = Some((self.info)().working_dir.to_string_lossy().into_owned());
        Olo::continues();
        match protocol::ask(&Request::Suggest { line: line.to_owned(), cwd, session: Olo::session() }, WAIT) {
            Some(Reply::Suggestion { rest, .. }) => Some(rest),
            Some(_) => None,
            None => {
                let mut s = self.shared.lock().unwrap();
                if s.started.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) && !protocol::running() {
                    s.started = Some(Instant::now());
                    protocol::start();
                }
                None
            }
        }
    }
}

impl Hinter for ModelHinter {
    fn handle(&mut self, line: &str, pos: usize, history: &dyn History, use_ansi_coloring: bool, cwd: &str) -> String {
        let at_end = pos == line.len();
        let rest = if at_end && !line.trim().is_empty() { self.ask(line) } else { None };
        match rest {
            Some(rest) => {
                self.mine = true;
                line.clone_into(&mut self.line);
                self.hint = rest;
                if use_ansi_coloring && !self.hint.is_empty() { self.style.paint(&self.hint).to_string() } else { self.hint.clone() }
            }
            None => {
                self.mine = false;
                self.own.handle(line, pos, history, use_ansi_coloring, cwd)
            }
        }
    }

    /// The hint, taken (reedline asks only then): what it makes is
    /// counted as accepted if it's what's entered.
    fn complete_hint(&self) -> String {
        if !self.mine {
            return self.own.complete_hint();
        }
        if !self.hint.is_empty() {
            self.shared.lock().unwrap().taken = Some(format!("{}{}", self.line, self.hint).trim_end().to_owned());
        }
        self.hint.clone()
    }

    fn next_hint_token(&self) -> String {
        if !self.mine {
            return self.own.next_hint_token();
        }
        // Its first word, with the space before it.
        let lead = self.hint.len() - self.hint.trim_start().len();
        let end = self.hint[lead..].find(' ').map_or(self.hint.len(), |i| lead + i);
        let word = self.hint[..end].to_owned();
        if !word.is_empty() {
            protocol::tell(&Request::Accepted { session: Olo::session(), whole: false });
        }
        word
    }
}

/// The shell's id and the one it stands in for (`Olo::session`), read as
/// it starts: brush takes the environment into its variables and the
/// process's own is gone after (a session by process id, every time).
static IDS: std::sync::OnceLock<(Option<u64>, Option<u64>)> = std::sync::OnceLock::new();

fn main() {
    olo_crash::install();
    let _ = IDS.set((protocol::shell_id(protocol::SHELL_ID_ENV), protocol::shell_id(protocol::SHELL_RESTORED_ENV)));
    // (Kept in the binary, for Preferences to find.)
    std::hint::black_box(protocol::HINTER_MARK);
    brush_interactive::hooks::set_hooks(Box::new(Olo { shared: Arc::default() }));
    brush_shell::entry::run();
}
