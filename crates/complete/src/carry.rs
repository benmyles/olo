//! A command a program printed for the person to run (a resume line),
//! given the options the person started that program with (Ben,
//! 2026-11-15: started as `codex --dangerously-bypass-approvals-and-
//! sandbox`, codex prints `codex resume 01a1ddfb-…` as it ends, "the ai
//! command autocomplete should be smart enough to know that what i want
//! is `codex --dangerously-bypass-approvals-and-sandbox resume 01a1ddfb-…`.
//! but don't like hardcode that rule for codex/claude or anything").
//!
//! Nothing here knows a program. What's learned, from every command the
//! person runs ([`Habits::add`], as it's run): which of a program's
//! options take a value (`-m o3`) and which stand alone (seen last, or
//! before another option), which the person nearly always gives it, and
//! whether they run what it prints as printed or with their options
//! carried ([`Habits::vote`], from what they run after it printed one).
//!
//! [`Habits::carry`]: the start's options the printed command hasn't (by
//! name: its own `--resume NEW` keeps over the start's `--resume OLD`),
//! each where it was in the start: before the first word that isn't an
//! option (the program's own options, as codex's), or after a word the
//! printed command has too (`git push -f`'s after `push`); one that
//! belonged to a word it hasn't (`cargo build --release`'s, for a printed
//! `cargo fix`) isn't carried, nor a value of more than one word (a
//! prompt, a message). With no start known, the options the person gives
//! the program in most runs.

use std::collections::HashMap;

/// A command's words as the shell splits them, each as it was written
/// (quotes and escapes kept): `a "b c" d\ e` is `a`, `"b c"`, `d\ e`.
pub fn words(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => escaped = true,
            (Some(_), _) => {}
            (None, '\\') => {
                escaped = true;
                start.get_or_insert(i);
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                start.get_or_insert(i);
            }
            (None, c) if c.is_whitespace() => {
                if let Some(s) = start.take() {
                    out.push(&line[s..i]);
                }
            }
            (None, _) => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

/// An option: `-x`, `--name`, `--name=value` (not `--`, which ends them,
/// nor `-` alone).
fn is_option(w: &str) -> bool {
    w.len() > 1 && w.starts_with('-') && w != "--"
}

/// An option's name: `--name=value`'s `--name`.
fn name(w: &str) -> &str {
    w.split_once('=').map_or(w, |(n, _)| n)
}

/// A program's name however it was run: `./codex`, `~/bin/codex`: `codex`.
fn program(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

/// What's known of one of a program's options.
#[derive(Debug, Clone, Default)]
struct Opt {
    /// Runs with it.
    runs: u32,
    /// Times a word not an option came after it (its value, or the
    /// program's next word), and times nothing or another option did.
    before_word: u32,
    alone: u32,
    /// Times it came before the program's first word that isn't one (the
    /// program's own option, not a subcommand's).
    first: u32,
}

/// What's known of one program's runs.
#[derive(Debug, Clone, Default)]
struct Program {
    runs: u32,
    options: HashMap<String, Opt>,
    /// Commands it printed run as printed, and run with the start's
    /// options carried.
    as_printed: u32,
    carried: u32,
}

/// An option of a start, as carried: its words (`-m o3`: two), and the
/// word it came after (none: the program's).
#[derive(Debug)]
struct Carried {
    words: Vec<String>,
    after: Option<String>,
}

/// What the person's runs show of each program's options.
#[derive(Debug, Clone, Default)]
pub struct Habits {
    programs: HashMap<String, Program>,
}

/// How a person runs what programs print: by program, as printed and
/// carried (kept by the daemon, as runs don't show it).
pub type Votes = HashMap<String, (u32, u32)>;

impl Habits {
    /// One more command run.
    pub fn add(&mut self, command: &str) {
        let ws = words(command);
        let Some((&first, rest)) = ws.split_first() else { return };
        let p = self.programs.entry(program(first).to_owned()).or_default();
        p.runs += 1;
        let mut seen: Vec<&str> = Vec::new();
        let mut positional = false;
        for (i, &w) in rest.iter().enumerate() {
            if w == "--" {
                break;
            }
            if !is_option(w) {
                positional = true;
                continue;
            }
            let n = name(w);
            let o = p.options.entry(n.to_owned()).or_default();
            if !seen.contains(&n) {
                o.runs += 1;
                seen.push(n);
            }
            if !positional {
                o.first += 1;
            }
            if w.contains('=') {
                continue;
            }
            match rest.get(i + 1) {
                Some(next) if !is_option(next) && *next != "--" => o.before_word += 1,
                _ => o.alone += 1,
            }
        }
        if p.options.len() > 256 {
            // (A program run with ever-new options: keep what's common.)
            p.options.retain(|_, o| o.runs > 1);
        }
    }

    /// Whether `option` of `program` takes the word after it: never seen
    /// without one.
    fn takes_value(&self, program: &str, option: &str) -> bool {
        self.programs.get(program).and_then(|p| p.options.get(option)).is_some_and(|o| o.before_word > 0 && o.alone == 0)
    }

    /// The person's runs of what `program` printed: as printed, or with
    /// the start's options carried.
    pub fn vote(&mut self, program_word: &str, carried: bool) {
        let p = self.programs.entry(program(program_word).to_owned()).or_default();
        if carried {
            p.carried += 1;
        } else {
            p.as_printed += 1;
        }
    }

    /// The votes, to keep; and kept ones put back.
    pub fn votes(&self) -> Votes {
        self.programs.iter().filter(|(_, p)| p.as_printed + p.carried > 0).map(|(n, p)| (n.clone(), (p.as_printed, p.carried))).collect()
    }

    pub fn set_votes(&mut self, votes: &Votes) {
        for (n, &(as_printed, carried)) in votes {
            let p = self.programs.entry(n.clone()).or_default();
            p.as_printed = as_printed;
            p.carried = carried;
        }
    }

    /// The options of `start` (how the program was started, if known) to
    /// carry into `printed`.
    fn options(&self, printed: &[&str], start: &[&str]) -> Vec<Carried> {
        let prog = program(printed[0]);
        let mut out = Vec::new();
        let mut after: Option<&str> = None;
        let mut i = 1;
        while i < start.len() {
            let w = start[i];
            i += 1;
            if w == "--" {
                break;
            }
            if !is_option(w) {
                after = Some(w);
                continue;
            }
            let mut ws = vec![w.to_owned()];
            if !w.contains('=')
                && let Some(&next) = start.get(i)
                && !is_option(next)
                && self.takes_value(prog, name(w))
                // (A word the printed command has is its own: a
                // subcommand, not this option's value.)
                && !printed.contains(&next)
            {
                ws.push(next.to_owned());
                i += 1;
            }
            out.push(Carried { words: ws, after: after.map(str::to_owned) });
        }
        out
    }

    /// The options the person gives `program` in most of at least three
    /// runs, before its first other word and without a value: carried
    /// when the start isn't known.
    fn habitual(&self, program_word: &str) -> Vec<Carried> {
        let Some(p) = self.programs.get(program(program_word)).filter(|p| p.runs >= 3) else { return Vec::new() };
        let mut found: Vec<(&String, &Opt)> = p.options.iter().filter(|(_, o)| o.runs * 2 > p.runs && o.first * 2 > o.runs && o.alone > 0).collect();
        found.sort_by(|a, b| b.1.runs.cmp(&a.1.runs).then(a.0.cmp(b.0)));
        found.into_iter().map(|(n, _)| Carried { words: vec![n.clone()], after: None }).collect()
    }

    /// `printed` (a command a program printed to run) with the options of
    /// `start` (the command that started it, the same program) it hasn't,
    /// each where it was; with no start, the person's habitual options for
    /// it. None when there's nothing to carry, the start is another
    /// program's, or the person runs what this program prints as printed.
    pub fn carry(&self, printed: &str, start: Option<&str>) -> Option<String> {
        let pw = words(printed);
        let prog = program(pw.first()?);
        if let Some(p) = self.programs.get(prog)
            && p.as_printed > p.carried + 1
        {
            return None;
        }
        let sw = start.map(words).unwrap_or_default();
        let options = match sw.first() {
            Some(&s) if program(s) == prog => self.options(&pw, &sw),
            Some(_) => return None,
            None => self.habitual(prog),
        };
        let has = |n: &str| pw.iter().any(|w| is_option(w) && name(w) == n);
        // (Text in a value, "a prompt": the person's words for that run.)
        let carried = options.iter().filter(|o| !has(name(&o.words[0])) && !o.words.iter().any(|w| words(&unquote(w)).len() > 1));
        // Where each goes: after the program, or after its word.
        let mut slots: Vec<Vec<&str>> = vec![Vec::new(); pw.len()];
        let mut any = false;
        for o in carried {
            let at = match &o.after {
                None => 0,
                Some(a) => match pw.iter().skip(1).position(|w| w == a) {
                    Some(j) => j + 1,
                    None => continue,
                },
            };
            slots[at].extend(o.words.iter().map(String::as_str));
            any = true;
        }
        if !any {
            return None;
        }
        let mut out: Vec<&str> = Vec::new();
        for (i, w) in pw.iter().enumerate() {
            out.push(w);
            out.extend(slots[i].iter().copied());
        }
        Some(out.join(" "))
    }
}

/// A word's text with its quotes taken off (`"a b"`: `a b`), to see
/// whether it's more than one word.
fn unquote(w: &str) -> String {
    let mut out = String::new();
    let mut quote = None;
    let mut escaped = false;
    for c in w.chars() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (_, '\\') if quote != Some('\'') => escaped = true,
            (None, '\'' | '"') => quote = Some(c),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn learned(runs: &[&str]) -> Habits {
        let mut h = Habits::default();
        for r in runs {
            h.add(r);
        }
        h
    }

    #[test]
    fn words_are_split_as_the_shell_does() {
        assert_eq!(words(r#"git commit -m "two words" a\ b 'c d'"#), ["git", "commit", "-m", r#""two words""#, r"a\ b", "'c d'"]);
        assert_eq!(words("  ls   -la "), ["ls", "-la"]);
        assert_eq!(unquote(r#""two words""#), "two words");
    }

    /// Ben's: codex started with its option, its resume line without.
    #[test]
    fn a_resume_line_keeps_how_the_program_was_started() {
        let h = learned(&["codex --dangerously-bypass-approvals-and-sandbox", "ls"]);
        let printed = "codex resume 01a1ddfb-20d0-7980-af3c-e082abab3e33";
        assert_eq!(
            h.carry(printed, Some("codex --dangerously-bypass-approvals-and-sandbox")).as_deref(),
            Some("codex --dangerously-bypass-approvals-and-sandbox resume 01a1ddfb-20d0-7980-af3c-e082abab3e33")
        );
        // Nothing to carry: none.
        assert_eq!(h.carry(printed, Some("codex")), None);
        // Another program's start: none.
        assert_eq!(h.carry(printed, Some("claude --dangerously-skip-permissions")), None);
    }

    /// Claude Code's resume line has the option it was started with, and
    /// its own `--resume`; the person's other option comes along, the old
    /// `--resume` doesn't.
    #[test]
    fn the_printed_commands_own_options_win() {
        let h = learned(&["claude --resume 0000 --dangerously-skip-permissions", "claude --allow-dangerously-skip-permissions --dangerously-skip-permissions --resume 1111"]);
        assert_eq!(
            h.carry("claude --dangerously-skip-permissions --resume 2222", Some("claude --allow-dangerously-skip-permissions --dangerously-skip-permissions --resume 1111")).as_deref(),
            Some("claude --allow-dangerously-skip-permissions --dangerously-skip-permissions --resume 2222")
        );
    }

    /// A value comes with its option when the person's runs show it takes
    /// one; a prompt doesn't come, nor a subcommand's option to a printed
    /// command without that subcommand.
    #[test]
    fn values_prompts_and_subcommands() {
        let h = learned(&["codex -m o3", "codex -m gpt-5 --full-auto", "codex --full-auto"]);
        assert_eq!(h.carry("codex resume 42", Some("codex -m o3 --full-auto")).as_deref(), Some("codex -m o3 --full-auto resume 42"));
        // A prompt in quotes is the person's words for that run.
        let h = learned(&["claude -p \"fix the tests\"", "claude -p \"and the docs\""]);
        assert_eq!(h.carry("claude --resume 9", Some("claude -p \"fix the tests\"")), None);
        // cargo build's --release is build's: not fix's.
        let h = learned(&["cargo build --release"]);
        assert_eq!(h.carry("cargo fix --lib -p olo", Some("cargo build --release")), None);
        // git push's -f goes after push.
        let h = learned(&["git push -f"]);
        assert_eq!(h.carry("git push --set-upstream origin topic", Some("git push -f")).as_deref(), Some("git push -f --set-upstream origin topic"));
        // A start whose option was always followed by its subcommand: the
        // printed command's subcommand isn't taken for its value.
        let h = learned(&["codex --yolo resume 1", "codex --yolo resume 2"]);
        assert_eq!(h.carry("codex resume 3", Some("codex --yolo resume 2")).as_deref(), Some("codex --yolo resume 3"));
    }

    /// With no start known, what the person gives the program in most
    /// runs; and a person who runs what it prints as printed isn't
    /// second-guessed.
    #[test]
    fn habits_and_votes() {
        let mut h = learned(&["codex --yolo", "codex --yolo", "codex --yolo exec hi", "codex"]);
        assert_eq!(h.carry("codex resume 7", None).as_deref(), Some("codex --yolo resume 7"));
        assert_eq!(learned(&["codex --yolo", "codex"]).carry("codex resume 7", None), None, "two runs: not yet a habit");
        h.vote("codex", false);
        assert!(h.carry("codex resume 7", None).is_some(), "once is no habit");
        h.vote("codex", false);
        assert_eq!(h.carry("codex resume 7", None), None);
        h.vote("codex", true);
        assert!(h.carry("codex resume 7", None).is_some());
        let mut again = Habits::default();
        again.set_votes(&h.votes());
        assert_eq!(again.votes(), h.votes());
    }
}
