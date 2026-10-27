//! Agent skills: a real session puts Tabula's skills where coding agents
//! look for them, so an agent run under Tabula knows how to use it (for
//! now Agent Screens, `apps/screen/SKILL.md`). Claude Code reads
//! `~/.claude/skills` (or `$CLAUDE_CONFIG_DIR/skills`); Codex reads
//! `~/.agents/skills`. A skill goes only to agents the person has (their
//! folder exists), and replaces only a skill Tabula wrote: one whose
//! marker line the person deleted is theirs and left alone. Kept up to
//! date at each start and after each upgrade.

use std::path::{Path, PathBuf};

/// (folder name, SKILL.md).
const SKILLS: &[(&str, &str)] = &[("tabula-screens", include_str!("../../../apps/screen/SKILL.md"))];

/// In every skill Tabula writes: without it, the file is the person's.
const MARKER: &str = "<!-- Kept up to date by Tabula;";

/// Install them on a thread (the home folder may be slow to answer).
pub fn start() {
    std::thread::spawn(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        for dir in skill_dirs(&home, claude) {
            for (name, text) in SKILLS {
                match install(&dir.join(name), text) {
                    Ok(true) => crate::log(format!("agent skill {name} put in {}", dir.display())),
                    Ok(false) => {}
                    Err(e) => crate::log(format!("could not put agent skill {name} in {}: {e}", dir.display())),
                }
            }
        }
    });
}

/// The skill folders of the agents the person has.
fn skill_dirs(home: &Path, claude_config: Option<PathBuf>) -> Vec<PathBuf> {
    if home.as_os_str().is_empty() {
        return Vec::new();
    }
    let claude = claude_config.unwrap_or_else(|| home.join(".claude"));
    let mut dirs = Vec::new();
    if claude.is_dir() {
        dirs.push(claude.join("skills"));
    }
    if home.join(".codex").is_dir() || home.join(".agents").is_dir() {
        dirs.push(home.join(".agents/skills"));
    }
    dirs
}

/// Write `dir/SKILL.md` unless it is already this or is the person's own;
/// whether it was written.
fn install(dir: &Path, text: &str) -> std::io::Result<bool> {
    let path = dir.join("SKILL.md");
    match std::fs::read_to_string(&path) {
        Ok(old) if old == text || !old.contains(MARKER) => return Ok(false),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".SKILL.md.tabula");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_carry_the_marker() {
        for (_, text) in SKILLS {
            assert!(text.starts_with("---\nname: "));
            assert!(text.contains(MARKER));
        }
    }

    #[test]
    fn installs_for_the_agents_there_are() {
        let home = std::env::temp_dir().join(format!("tabula-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        assert_eq!(skill_dirs(&home, None), [home.join(".claude/skills")]);
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        assert_eq!(skill_dirs(&home, None), [home.join(".claude/skills"), home.join(".agents/skills")]);
        assert_eq!(skill_dirs(&home, Some(home.join("elsewhere"))), [home.join(".agents/skills")]);

        let dir = home.join(".claude/skills/tabula-screens");
        let text = SKILLS[0].1;
        assert!(install(&dir, text).unwrap());
        assert!(!install(&dir, text).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), text);
        // An older version of ours is replaced.
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: old\n---\n{MARKER} … -->\n")).unwrap();
        assert!(install(&dir, text).unwrap());
        // The person's own (the marker gone) is kept.
        std::fs::write(dir.join("SKILL.md"), "---\nname: mine\n---\n").unwrap();
        assert!(!install(&dir, text).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "---\nname: mine\n---\n");
        let _ = std::fs::remove_dir_all(&home);
    }
}
