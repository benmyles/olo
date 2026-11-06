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
/// argument, or what's between backquotes when that is.
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
    if out.is_empty() && !t.contains('`') && is_command(t, known) {
        out.push(t.to_owned());
    }
    out
}

fn is_command(text: &str, known: &dyn Fn(&str) -> bool) -> bool {
    let mut words = text.split_whitespace();
    let Some(first) = words.next() else { return false };
    // A sentence ("git is a version control system.") isn't a command.
    known(first) && words.next().is_some() && !text.trim_end().ends_with(['.', '!', '?']) && text.len() <= 1000
}

/// The commands `text` (a terminal's last lines) names, oldest first: each
/// line's command after a prompt, or printed for the person to run; the
/// last prompt's (being typed now) left out.
pub fn commands(text: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let lines = &lines[lines.len().saturating_sub(LINES)..];
    // The prompt being typed at: the last line that is one.
    let end = lines.iter().rposition(|l| after_prompt(l).is_some()).unwrap_or(lines.len());
    let mut out: Vec<String> = Vec::new();
    for line in &lines[..end] {
        let found = match after_prompt(line) {
            Some(c) if !c.is_empty() => vec![c.to_owned()],
            Some(_) => Vec::new(),
            None => printed(line, known),
        };
        for c in found {
            // Each once, at its latest.
            out.retain(|o| *o != c);
            out.push(c);
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

    /// Ben's tab (2026-11-05), restored after a restart: Claude Code
    /// printed how to resume, he ran it with more flags, the machine
    /// restarted, he went back to the folder and typed `cl`.
    const BENS_TAB: &str = "\
Generated rules for gemini at /home/ben/src/notes/GEMINI.md
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

    #[test]
    fn printed_commands_and_prompts() {
        let text = "\
me@box:~/x$ git push
fatal: The current branch topic has no upstream branch.
To push the current branch and set the remote as upstream, use

    git push --set-upstream origin topic

warning: run `cargo fix --lib -p olo` to apply 2 suggestions
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
