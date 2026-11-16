//! Commands a terminal's recent text names (Ben, 2026-11-05: the
//! suggestions should "also receive ... the recent output so that it can
//! take all that into account"): what was run at a prompt (a tab restored
//! after a restart shows its old commands so), and what a program printed
//! for the person to run ("Resume this session with:" and the command on
//! the next line, "run `cargo fix`", `git push --set-upstream origin
//! branch`). olo-termd keeps each shell's last lines as text
//! (`protocol::tail_path`); olo-complete suggests the latest that fits
//! what's typed, ahead of the history's.

/// How many of the text's last lines are looked at.
pub const LINES: usize = 80;

/// The command after a prompt in `line`, if it has one: `user@host:dir$
/// command` (brush's, bash's), `dir % command`, `# command`; the prompt
/// one word (no spaces), ending in `$`, `%`, `#` or `❯` and a space.
pub fn after_prompt(line: &str) -> Option<&str> {
    for mark in ["$ ", "% ", "# ", "❯ "] {
        let Some(at) = line.find(mark) else { continue };
        let prompt = &line[..at];
        let looks = !prompt.is_empty() && !prompt.contains(char::is_whitespace) && prompt.contains(['@', ':', '~', '/']);
        if looks || at == 0 {
            return Some(line[at + mark.len()..].trim());
        }
    }
    None
}

/// A command printed in `line` (not after a prompt): the line itself when
/// its first word is a program the person runs (`known`) and it has an
/// argument, what's between backquotes when that is, or what follows the
/// line's last colon after words saying to run it ("To continue this
/// session, run: codex resume …"; not "error: cargo build failed").
fn printed(line: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let t = line.trim();
    let mut out = Vec::new();
    let mut parts = t.split('`');
    parts.next();
    // Odd pieces are inside backquotes.
    for (i, piece) in parts.enumerate() {
        if i % 2 == 0 && is_command(piece, known) {
            out.push(piece.trim().to_owned());
        }
    }
    if out.is_empty() && !t.contains('`') {
        if is_command(t, known) {
            out.push(t.to_owned());
        } else if let Some((before, after)) = t.rsplit_once(": ")
            && before.split_whitespace().last().is_some_and(|w| RUN_CUES.contains(&w.to_lowercase().as_str()))
            && is_command(after, known)
        {
            out.push(after.trim().to_owned());
        }
    }
    out
}

/// The word before a colon that says what follows is to be run.
const RUN_CUES: &[&str] = &["run", "with", "use", "try", "type", "execute", "enter", "command", "again", "resume", "continue", "rerun"];

fn is_command(text: &str, known: &dyn Fn(&str) -> bool) -> bool {
    let mut words = text.split_whitespace();
    let Some(first) = words.next() else { return false };
    // A sentence ("git is a version control system.") isn't a command.
    known(first) && words.next().is_some() && !text.trim_end().ends_with(['.', '!', '?']) && text.len() <= 1000
}

/// A command a terminal's text names, and, for one a program printed, the
/// command at the prompt above it that started that program (the same
/// program), if it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub command: String,
    /// Printed by a program, not run at a prompt.
    pub printed: bool,
    pub start: Option<String>,
}

/// The commands `text` (a terminal's last lines) names, oldest first: each
/// line's command after a prompt, or printed for the person to run; the
/// last prompt's (being typed now) left out.
pub fn commands(text: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    named(text, known).into_iter().map(|n| n.command).collect()
}

/// [`commands`], each printed one with its start ([`Named::start`]).
pub fn named(text: &str, known: &dyn Fn(&str) -> bool) -> Vec<Named> {
    let lines: Vec<&str> = text.lines().collect();
    let lines = &lines[lines.len().saturating_sub(LINES)..];
    // The prompt being typed at: the last line that is one.
    let end = lines.iter().rposition(|l| after_prompt(l).is_some()).unwrap_or(lines.len());
    let program = |c: &str| c.split_whitespace().next().map(|w| w.rsplit('/').next().unwrap_or(w).to_owned());
    let mut out: Vec<Named> = Vec::new();
    let mut started: Option<String> = None;
    for line in &lines[..end] {
        let found: Vec<Named> = match after_prompt(line) {
            Some(c) if !c.is_empty() => {
                started = Some(c.to_owned());
                vec![Named { command: c.to_owned(), printed: false, start: None }]
            }
            Some(_) => {
                started = None;
                Vec::new()
            }
            None => printed(line, known)
                .into_iter()
                .map(|c| {
                    let start = started.clone().filter(|s| program(s) == program(&c) && *s != c);
                    Named { command: c, printed: true, start }
                })
                .collect(),
        };
        for n in found {
            // Each once, at its latest.
            out.retain(|o| o.command != n.command);
            out.push(n);
        }
    }
    out
}

/// The latest command `text` names that `typed` grows into, if any.
pub fn latest_for<'a>(found: &'a [String], typed: &str) -> Option<&'a str> {
    if typed.trim().is_empty() {
        return None;
    }
    found.iter().rev().find(|c| c.starts_with(typed) && c.len() > typed.len()).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tab restored after a restart (as Ben's was, 2026-11-05; names
    /// made up): Claude Code printed how to resume, it was run with more
    /// flags, the machine restarted, then back in the folder `cl` typed.
    const BENS_TAB: &str = "\
Generated rules for gemini at /home/sam/src/notes/GEMINI.md
✅ marshal rules generated
sam@laptop:~/src/notes$ claude
sam@laptop:~/src/notes$ claude --help|grep danger
  --allow-dangerously-skip-permissions  Enable bypassing all permission checks
  --dangerously-skip-permissions        Bypass all permission checks.
sam@laptop:~/src/notes$ claude --dangerously-skip-permissions

Resume this session with:
claude --dangerously-skip-permissions --resume 00000000-1111-4222-8333-444444444444
sam@laptop:~/src/notes$ claude --help|grep allow
  --add-dir <directories...>            Additional directories to allow tool
sam@laptop:~/src/notes$ claude --allow-dangerously-skip-permissions --dangerously-skip-permissions --resume 00000000-1111-4222-8333-444444444444
Restored from Nov 5, 2026, 9:15 AM
sam@laptop:~$ cd ~/src/notes
sam@laptop:~/src/notes$ cl";

    fn known(w: &str) -> bool {
        ["claude", "cd", "git", "cargo"].contains(&w)
    }

    #[test]
    fn bens_restored_tab_suggests_the_resume() {
        let found = commands(BENS_TAB, &known);
        assert_eq!(latest_for(&found, "cl"), Some("claude --allow-dangerously-skip-permissions --dangerously-skip-permissions --resume 00000000-1111-4222-8333-444444444444"));
        // What Claude Code printed is one too.
        assert!(found.iter().any(|c| c == "claude --dangerously-skip-permissions --resume 00000000-1111-4222-8333-444444444444"));
        // The help's lines aren't commands, nor the line being typed.
        assert!(!found.iter().any(|c| c.contains("Enable bypassing") || c == "cl"));
        assert_eq!(latest_for(&found, "cd "), Some("cd ~/src/notes"));
    }

    /// Ben's (2026-11-15): codex started with an option, its resume line
    /// printed as it ended: named with that start.
    #[test]
    fn a_printed_command_knows_its_start() {
        let text = "\
me@box:~/x$ codex --dangerously-bypass-approvals-and-sandbox
Token usage: total=12,345 input=10,000 output=2,345
To continue this session, run: codex resume 01a1ddfb-20d0-7980-af3c-e082abab3e33
me@box:~/x$ ";
        let known = |w: &str| w == "codex";
        let found = named(text, &known);
        assert_eq!(found.len(), 2, "{found:?}");
        let resume = &found[1];
        assert_eq!(resume.command, "codex resume 01a1ddfb-20d0-7980-af3c-e082abab3e33");
        assert_eq!(resume.start.as_deref(), Some("codex --dangerously-bypass-approvals-and-sandbox"));
        assert!(found[1].printed && !found[0].printed);
        assert_eq!(found[0].start, None, "a run has none");
        // The command on a line of its own after the colon: the same.
        let text = text.replace("run: codex", "run:\ncodex");
        assert_eq!(named(&text, &known)[1], resume.clone());
        // Not after a prompt of that program: no start.
        let text = text.replace("$ codex --dangerously-bypass-approvals-and-sandbox", "$ ls");
        assert_eq!(named(&text, &|w| w == "codex" || w == "ls").last().unwrap().start, None);
    }

    #[test]
    fn printed_commands_and_prompts() {
        let text = "\
me@box:~/x$ git push
fatal: The current branch topic has no upstream branch.
To push the current branch and set the remote as upstream, use

    git push --set-upstream origin topic

warning: run `cargo fix --lib -p olo` to apply 2 suggestions
error: cargo build failed
git is a version control system.
me@box:~/x$ ";
        let found = commands(text, &known);
        assert_eq!(found, ["git push", "git push --set-upstream origin topic", "cargo fix --lib -p olo"]);
        assert_eq!(latest_for(&found, "git p"), Some("git push --set-upstream origin topic"));
        assert_eq!(latest_for(&found, "ls"), None);
        assert_eq!(after_prompt("sam@laptop:~/src/olo$ ls -la"), Some("ls -la"));
        assert_eq!(after_prompt("costs $ 5 each"), None);
    }
}
