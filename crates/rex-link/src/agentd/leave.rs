//! Leave given by a list: the service itself says "yes, once" to what a
//! child asks leave for, at once, except where the command asked for
//! matches a pattern of the list, which goes to whoever answers for the
//! child (its minder if it has one, else its parent's watch). The
//! ShiftUp lead, 2026-11-27, with Ben's words: "they follow
//! instructions great honestly i don't care much about requiring
//! approvals from them at all only in the most extreme cases"; "A minder
//! answers in 6 to 16 s and costs a model turn each; most of our boxes
//! need no judgement. Please: `--approve-all-but FILE` … the service
//! itself answers 'yes, once' to any approval box of that child at once,
//! EXCEPT one whose whole command (every part of a chain, redirections
//! included) matches a pattern in FILE … Never a kept yes."
//!
//! The list is the parent's: a pattern a line (a regular expression,
//! looked for anywhere in what is asked), `#` for a remark. What is
//! asked is read as one text, [`asked`]: the tool and the whole command
//! as the agent shows it, chains and redirections and all (a pattern
//! sees `cd x && git push > log`, never only its first words, which is
//! all an agent's own allow rules judge). No model reads it, so nothing
//! is weighed: what no pattern names is allowed, once.

use regex::Regex;

/// A parent's list of what is *not* to be allowed without asking.
#[derive(Clone, Debug)]
pub struct LeaveList {
    patterns: Vec<(String, Regex)>,
}

impl LeaveList {
    /// The list from its text. A line that isn't a regular expression is
    /// said, with its number: a list that half holds allows what it was
    /// written to stop.
    pub fn parse(text: &str) -> Result<LeaveList, String> {
        let mut patterns = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match Regex::new(line) {
                Ok(re) => patterns.push((line.to_owned(), re)),
                Err(e) => return Err(format!("line {} of the list isn't a pattern (`{line}`): {}", i + 1, e.to_string().lines().last().unwrap_or_default().trim())),
            }
        }
        Ok(LeaveList { patterns })
    }

    /// Its patterns, as written.
    pub fn patterns(&self) -> Vec<&str> {
        self.patterns.iter().map(|(said, _)| said.as_str()).collect()
    }

    /// The pattern that names what is asked, if one does: then it is not
    /// allowed here, and goes to whoever answers for the child.
    pub fn excepts(&self, asked: &str) -> Option<&str> {
        self.patterns.iter().find(|(_, re)| re.is_match(asked)).map(|(said, _)| said.as_str())
    }
}

/// What a child asks leave for, as one text for the list's patterns:
/// `TOOL: COMMAND`, the command whole as the agent shows it (its lines
/// joined by a space); for what has no command (a file to read, a page
/// to fetch), the tool and the question's own words. None where neither
/// tool nor command could be read off the screen: what isn't understood
/// is never allowed by a list.
pub fn asked(tool: Option<&str>, command: Option<&str>, question: &str) -> Option<String> {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    match (tool.map(str::trim).filter(|t| !t.is_empty()), command.map(one_line).filter(|c| !c.is_empty())) {
        (Some(tool), Some(command)) => Some(format!("{tool}: {command}")),
        (None, Some(command)) => Some(command),
        (Some(tool), None) => Some(format!("{tool}: {}", one_line(question))),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_names_what_is_not_allowed_and_everything_else_is() {
        let list = LeaveList::parse(
            "# Nothing leaves this machine, nothing is destroyed.\n\
             \\bgit\\s+(push|commit)\\b\n\
             \n\
             \\brm\\s+-[a-zA-Z]*r\n\
             \\bsudo\\b\n\
             >\\s*/etc/\n\
             ^(Write|Edit): .*\\.env\\b\n",
        )
        .unwrap();
        assert_eq!(list.patterns().len(), 5);
        let text = |tool: &str, cmd: &str| asked(Some(tool), Some(cmd), "Do you want to proceed?").unwrap();
        // The whole command is read: a later part of a chain, a redirection, a second line.
        assert_eq!(list.excepts(&text("Bash", "cd /w && cargo build && git push origin main")), Some("\\bgit\\s+(push|commit)\\b"));
        assert_eq!(list.excepts(&text("Bash", "echo 'nameserver 1.1.1.1' > /etc/resolv.conf")), Some(">\\s*/etc/"));
        assert_eq!(list.excepts(&text("Bash", "cargo test\nrm -rf target")), Some("\\brm\\s+-[a-zA-Z]*r"));
        assert!(list.excepts(&text("Bash", "sudo systemctl restart x")).is_some());
        // What no pattern names is allowed: and a word inside another isn't the word.
        for allowed in ["cargo test -p olo-kit", "git status && git diff", "rm target/debug/x.o", "ls /etc/", "echo pseudo > out.txt", "cat .env.example | wc -l"] {
            assert_eq!(list.excepts(&text("Bash", allowed)), None, "{allowed}");
        }
        // What has no command is named by its tool and its question.
        let edit = asked(Some("Edit"), None, "Do you want to make this edit to .env?").unwrap();
        assert_eq!((edit.as_str(), list.excepts(&edit).is_some()), ("Edit: Do you want to make this edit to .env?", true));
        assert_eq!(list.excepts(&asked(Some("Read"), None, "Allow this read?").unwrap()), None);
        // What couldn't be read off the screen is nothing a list allows.
        assert_eq!(asked(None, None, "Proceed?"), None);
        assert_eq!(asked(None, Some("  make\n  install "), "").as_deref(), Some("make install"));

        // An empty list allows everything that is asked.
        assert_eq!(LeaveList::parse("# nothing\n\n").unwrap().excepts("Bash: git push"), None);
        // A line that is no pattern is said, by its number: never a list that half holds.
        let bad = LeaveList::parse("\\bsudo\\b\n(unclosed\n").unwrap_err();
        assert!(bad.starts_with("line 2 of the list isn't a pattern (`(unclosed`)"), "{bad}");
    }
}
