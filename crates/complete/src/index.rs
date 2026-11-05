//! What's known of each command the person has run, kept as it's run:
//! how often, how lately, in which folders, at what hours and on which
//! days, what came before it, how it ended. From that, for what's typed
//! so far, the commands it could become ([`Index::candidates`]) and what
//! the model sees of each ([`Index::features`], [`F`] numbers).

use std::collections::{BTreeMap, HashMap};

use crate::history::{Entry, local_time};

/// How many numbers the model sees of each candidate.
pub const F: usize = 16;
/// How many of a shell's commands are kept for what comes next (Ben,
/// 2026-11-05: "the last n commands (maybe 10 ...)").
pub const SESSION_LAST: usize = 10;
/// How many candidates are scored for a line, at most.
pub const MAX_CANDIDATES: usize = 32;
/// What each feature is, for the person (Activity Monitor) and the tests.
pub const FEATURE_NAMES: [&str; F] = [
    "how often",
    "how lately (time)",
    "how lately (order)",
    "share in this folder",
    "uses in this folder",
    "after the last command",
    "two after",
    "same program as the last",
    "at this hour",
    "on this kind of day",
    "length left",
    "typed past the program",
    "uses in this shell",
    "at a word's end",
    "in the last ten",
    "ended well",
];

/// One command's history.
#[derive(Debug, Clone, Default)]
struct Stats {
    count: u32,
    last_at: i64,
    last_seq: u64,
    cwds: HashMap<u32, u32>,
    hours: [u16; 24],
    /// Uses on weekdays and at weekends, and how many had a time.
    days: [u32; 2],
    timed: u32,
    ok: u32,
    with_status: u32,
}

/// A shell's commands so far (most recent last), and how often each.
#[derive(Debug, Clone, Default)]
struct Session {
    last: Vec<u32>,
    counts: HashMap<u32, u32>,
}

/// Everything known, for scoring a line.
#[derive(Debug, Default)]
pub struct Index {
    ids: BTreeMap<String, u32>,
    commands: Vec<String>,
    stats: Vec<Stats>,
    cwds: HashMap<String, u32>,
    /// How often one command came right after another (`(before, after)`),
    /// and two after.
    follows: HashMap<(u32, u32), u32>,
    follows2: HashMap<(u32, u32), u32>,
    sessions: HashMap<u64, Session>,
    /// The programs run (each command's first word), to know a command
    /// named in a terminal's output (`output::commands`).
    programs: std::collections::HashSet<String>,
    /// The person's last commands, any shell (most recent last).
    recent: Vec<u32>,
    /// Commands seen, in all.
    seq: u64,
}

/// What a line is scored in: what's typed, where, in which shell, when.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    pub line: &'a str,
    pub cwd: Option<&'a str>,
    pub session: u64,
    pub at: i64,
}

impl Index {
    pub fn new() -> Index {
        Index::default()
    }

    /// Every entry, oldest first.
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Index {
        let mut ix = Index::new();
        for e in entries {
            ix.add(e);
        }
        ix
    }

    /// How many different commands it knows.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// How many commands it has seen, repeats too.
    pub fn seen(&self) -> u64 {
        self.seq
    }

    pub fn command(&self, id: u32) -> &str {
        &self.commands[id as usize]
    }

    pub fn id(&self, command: &str) -> Option<u32> {
        self.ids.get(command).copied()
    }

    /// One more command run.
    pub fn add(&mut self, e: &Entry) {
        let id = match self.ids.get(&e.command) {
            Some(&id) => id,
            None => {
                let id = self.commands.len() as u32;
                self.ids.insert(e.command.clone(), id);
                self.commands.push(e.command.clone());
                self.stats.push(Stats::default());
                if let Some(program) = e.command.split_whitespace().next() {
                    self.programs.insert(program.to_owned());
                }
                id
            }
        };
        self.seq += 1;
        let cwd = e.cwd.as_ref().map(|c| {
            let n = self.cwds.len() as u32;
            *self.cwds.entry(c.clone()).or_insert(n)
        });
        let s = &mut self.stats[id as usize];
        s.count += 1;
        s.last_seq = self.seq;
        if e.at > 0 {
            s.last_at = e.at;
            let (hour, day) = local_time(e.at);
            s.hours[(hour as usize).min(23)] += 1;
            s.days[usize::from(day == 0 || day == 6)] += 1;
            s.timed += 1;
        }
        if let Some(c) = cwd {
            *s.cwds.entry(c).or_insert(0) += 1;
        }
        if let Some(st) = e.status {
            s.with_status += 1;
            s.ok += u32::from(st == 0);
        }
        let session = self.sessions.entry(e.session).or_default();
        if let Some(&before) = session.last.last() {
            *self.follows.entry((before, id)).or_insert(0) += 1;
        }
        if session.last.len() >= 2 {
            let before2 = session.last[session.last.len() - 2];
            *self.follows2.entry((before2, id)).or_insert(0) += 1;
        }
        session.last.push(id);
        if session.last.len() > SESSION_LAST {
            session.last.remove(0);
        }
        *session.counts.entry(id).or_insert(0) += 1;
        self.recent.push(id);
        if self.recent.len() > 10 {
            self.recent.remove(0);
        }
    }

    /// Shell `session` carries on shell `from`: a Terminal tab restored
    /// after a restart (its shell new, `OLO_SHELL_RESTORED`). Its commands
    /// so far are the old one's, if it has none of its own yet.
    pub fn continue_session(&mut self, session: u64, from: u64) {
        if session == from || self.sessions.get(&session).is_some_and(|s| !s.last.is_empty()) {
            return;
        }
        if let Some(old) = self.sessions.get(&from).cloned() {
            self.sessions.insert(session, old);
        }
    }

    /// `word` is a program the person has run.
    pub fn knows_program(&self, word: &str) -> bool {
        self.programs.contains(word)
    }

    /// Shell `session`'s commands so far, oldest first (at most
    /// `SESSION_LAST`).
    pub fn session_commands(&self, session: u64) -> Vec<&str> {
        self.sessions.get(&session).map_or_else(Vec::new, |s| s.last.iter().map(|&id| self.command(id)).collect())
    }

    /// How the last command ended, told as the next prompt comes (it
    /// wasn't known when it was added).
    pub fn ended(&mut self, session: u64, status: i32) {
        let Some(&id) = self.sessions.get(&session).and_then(|s| s.last.last()) else { return };
        let s = &mut self.stats[id as usize];
        s.with_status += 1;
        s.ok += u32::from(status == 0);
    }

    /// The commands `line` could become: those that start with it (not
    /// it), the likeliest first by how often and how lately, at most
    /// [`MAX_CANDIDATES`].
    pub fn candidates(&self, line: &str) -> Vec<u32> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(f32, u32)> = self
            .ids
            .range(line.to_owned()..)
            .take_while(|(c, _)| c.starts_with(line))
            .filter(|(c, _)| c.len() > line.len())
            .map(|(_, &id)| {
                let s = &self.stats[id as usize];
                (((1 + s.count) as f32).ln() + 2.0 * (-((self.seq - s.last_seq) as f32) / 100.0).exp(), id)
            })
            .collect();
        if found.len() > MAX_CANDIDATES {
            found.select_nth_unstable_by(MAX_CANDIDATES - 1, |a, b| b.0.total_cmp(&a.0));
            found.truncate(MAX_CANDIDATES);
        }
        found.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// What the model sees of candidate `id` for `cx`: [`F`] numbers, each
    /// about 0 to 2 (`FEATURE_NAMES`).
    pub fn features(&self, cx: &Context, id: u32) -> [f32; F] {
        let s = &self.stats[id as usize];
        let cand = &self.commands[id as usize];
        let session = self.sessions.get(&cx.session);
        let prev = |n: usize| session.and_then(|s| s.last.len().checked_sub(n + 1).map(|i| s.last[i]));
        let ln = |n: u32| ((1 + n) as f32).ln();
        let cwd = cx.cwd.and_then(|c| self.cwds.get(c)).copied();
        let in_cwd = cwd.and_then(|c| s.cwds.get(&c)).copied().unwrap_or(0);
        let follow = |m: &HashMap<(u32, u32), u32>, before: Option<u32>| before.and_then(|b| m.get(&(b, id))).copied().unwrap_or(0);
        let first_word = |c: &str| c.split_whitespace().next().unwrap_or("").to_owned();
        let same_program = prev(0).is_some_and(|p| first_word(&self.commands[p as usize]) == first_word(cand));
        let (hour, day) = if cx.at > 0 { local_time(cx.at) } else { (0.0, 0) };
        let hour_share = if cx.at > 0 && s.timed > 0 {
            let h = (hour as usize).min(23);
            let near: u32 = [(h + 23) % 24, h, (h + 1) % 24].iter().map(|&i| u32::from(s.hours[i])).sum();
            near as f32 / s.timed as f32
        } else {
            0.0
        };
        let day_share = if cx.at > 0 && s.timed > 0 { s.days[usize::from(day == 0 || day == 6)] as f32 / s.timed as f32 } else { 0.5 };
        let left = cand.len().saturating_sub(cx.line.len());
        let rest = &cand[cx.line.len().min(cand.len())..];
        let at_word_end = rest.starts_with(' ') || cx.line.ends_with(' ');
        let in_session = session.and_then(|s| s.counts.get(&id)).copied().unwrap_or(0);
        [
            ln(s.count) / 4.0,
            if s.last_at > 0 && cx.at > 0 { (-((cx.at - s.last_at).max(0) as f32) / (3.0 * 86_400.0)).exp() } else { 0.0 },
            (-((self.seq - s.last_seq) as f32) / 100.0).exp(),
            if cwd.is_some() { in_cwd as f32 / s.count.max(1) as f32 } else { 0.0 },
            ln(in_cwd) / 3.0,
            ln(follow(&self.follows, prev(0))) / 2.0,
            ln(follow(&self.follows2, prev(1))) / 2.0,
            f32::from(u8::from(same_program)),
            hour_share,
            day_share,
            (left as f32 / 40.0).min(2.0),
            f32::from(u8::from(cx.line.contains(' '))),
            ln(in_session) / 2.0,
            f32::from(u8::from(at_word_end)),
            f32::from(u8::from(self.recent.contains(&id))),
            if s.with_status > 0 { s.ok as f32 / s.with_status as f32 } else { 1.0 },
        ]
    }

    /// Every candidate's features, one row each.
    pub fn rows(&self, cx: &Context, candidates: &[u32]) -> Vec<f32> {
        candidates.iter().flat_map(|&id| self.features(cx, id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(cmd: &str, cwd: &str, session: u64) -> Entry {
        Entry { cwd: Some(cwd.into()), session, ..Entry::new(cmd) }
    }

    #[test]
    fn candidates_start_with_the_line_and_the_likeliest_come_first() {
        let ix = Index::from_entries(&[e("git status", "/a", 1), e("git push", "/a", 1), e("git status", "/a", 1), e("ls", "/a", 1)]);
        let c: Vec<&str> = ix.candidates("git").iter().map(|&i| ix.command(i)).collect();
        assert_eq!(c, ["git status", "git push"]);
        // Not what's typed whole, nor nothing typed.
        assert!(ix.candidates("ls").is_empty() && ix.candidates("  ").is_empty());
    }

    #[test]
    fn a_restored_shell_carries_on_the_old_ones_commands() {
        let mut ix = Index::new();
        for (i, c) in ["cd src", "cargo build", "claude --resume 9ee4"].iter().enumerate() {
            ix.add(&Entry { at: i as i64 + 1, ..e(c, "/src", 7) });
        }
        ix.continue_session(8, 7);
        assert_eq!(ix.session_commands(8), ["cd src", "cargo build", "claude --resume 9ee4"]);
        // One with commands of its own already keeps them.
        ix.add(&e("ls", "/src", 9));
        ix.continue_session(9, 7);
        assert_eq!(ix.session_commands(9), ["ls"]);
        assert!(ix.knows_program("claude") && !ix.knows_program("--resume"));
        // Ten kept, the oldest let go.
        for i in 0..12 {
            ix.add(&e(&format!("echo {i}"), "/src", 10));
        }
        assert_eq!(ix.session_commands(10).len(), SESSION_LAST);
        assert_eq!(ix.session_commands(10)[0], "echo 2");
    }

    #[test]
    fn features_see_the_folder_the_order_and_the_shell() {
        let ix = Index::from_entries(&[e("cargo build", "/src", 1), e("cargo test", "/src", 1), e("cargo build", "/src", 1), e("cargo run", "/tmp", 2)]);
        let cx = Context { line: "cargo ", cwd: Some("/src"), session: 1, at: 0 };
        let test = ix.features(&cx, ix.id("cargo test").unwrap());
        let run = ix.features(&cx, ix.id("cargo run").unwrap());
        // "cargo test" came after "cargo build" here, in this folder.
        assert!(test[5] > 0.0 && run[5] == 0.0);
        assert_eq!((test[3], run[3]), (1.0, 0.0));
        assert_eq!((test[12] > 0.0, run[12]), (true, 0.0));
        // The same program as the last; typed past its name.
        assert_eq!((test[7], test[11]), (1.0, 1.0));
    }
}
