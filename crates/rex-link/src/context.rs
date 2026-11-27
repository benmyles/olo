//! `olo-rex-link context install|remove|status [--json]`: what every
//! coding agent on this host is told before it begins.
//!
//! Ben, 2026-11-27, of an agent on a Rex host that spent a long while
//! finding out where it ran and how to open a link in his browser: "can
//! we have it so that all agents on a rex host get a section added to
//! their system instructions or the closest to that each harness allows
//! that explains where they're running and an overview of the relevant
//! skills and capabilities of each. should work for all of claude, codex
//! and agy".
//!
//! Two things, both by `olo_agent_context` and this file, in Rust, on
//! the host itself:
//!
//! - **The section**: a block of Olo's own in Claude Code's `CLAUDE.md`
//!   and Codex's `AGENTS.md` (the person's text there kept to the byte),
//!   and a rule file of Olo's own for Antigravity; only for an agent
//!   whose own folder is on the machine (`olo_agent_context`).
//! - **The bridge's skill where the others look**: Rex puts
//!   `olo-rex-bridge` in `~/.claude/skills`, which only Claude Code
//!   reads. Codex and Antigravity read `~/.agents/skills`: where the
//!   person has either, the skill's `SKILL.md` is put there too, marked
//!   as Olo's (a stamp beside it), kept the same as Claude Code's, and
//!   never written over one that isn't Olo's.
//!
//! Run by Rex when it puts its pieces on a host, and by
//! `tools/dev/host-upgrade`; `remove` when they are taken off. Each is
//! safe to run again: nothing is written that is already as it should be.

use std::path::{Path, PathBuf};

use olo_agent_context::{Env, Harness, Outcome, Status};

const USAGE: &str = "usage: olo-rex-link context install|remove|status [--json]";

/// The bridge's skill, as Rex puts it for Claude Code.
const SKILL: &str = ".claude/skills/olo-rex-bridge/SKILL.md";
/// Where Codex and Antigravity look for skills, and Olo's stamp there.
const SHARED_SKILL_DIR: &str = ".agents/skills/olo-rex-bridge";
const STAMP: &str = ".olo-skill";

pub fn run(args: &[String]) -> i32 {
    let as_json = args.iter().any(|a| a == "--json");
    let what: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--json").collect();
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from) else {
        eprintln!("olo-rex-link context: no HOME: nowhere to look for an agent's own folder");
        return 1;
    };
    let env = Env::current();
    let (lines, failed) = match what.as_slice() {
        ["install"] => {
            let mut said = outcomes(olo_agent_context::install_all(&home, &env), "written");
            said.0.push(skill_line(shared_skill(&home, &env, true)));
            said
        }
        ["remove"] => {
            let mut said = outcomes(olo_agent_context::remove_all(&home, &env), "removed");
            said.0.push(skill_line(shared_skill(&home, &env, false)));
            said
        }
        ["status"] => statuses(olo_agent_context::status_all(&home, &env)),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    if as_json {
        println!("{}", serde_json::Value::Array(lines.iter().map(|(who, state, path)| serde_json::json!({ "for": who, "state": state, "path": path })).collect()));
    } else {
        for (who, state, path) in &lines {
            println!("{who}: {state} ({path})");
        }
    }
    i32::from(failed)
}

/// At a connection: the section and the shared skill brought to what
/// this helper has, where Olo's skill for agents is on this host (the
/// person had it put there: its stamp), and not otherwise. Says nothing
/// and fails nothing: a file that can't be written is as it was.
///
/// Only a helper that is this host's own does it: one run from where
/// Rex puts a host's helper (`~/.local/share/olo/rex-link`). A build run
/// from anywhere else, a test's or a developer's, writes nothing into
/// the home it happens to run in.
pub fn keep() {
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from) else { return };
    let its_own = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).is_some_and(|dir| dir == home.join(".local/share/olo/rex-link"));
    if !its_own || !home.join(SKILL).with_file_name(STAMP).exists() {
        return;
    }
    let env = Env::current();
    let _ = olo_agent_context::install_all(&home, &env);
    let _ = shared_skill(&home, &env, true);
}

type Line = (String, String, String);

fn outcomes(reports: Vec<olo_agent_context::HarnessReport<std::io::Result<Outcome>>>, changed: &str) -> (Vec<Line>, bool) {
    let mut failed = false;
    let lines = reports
        .into_iter()
        .map(|r| {
            let state = match r.result {
                Ok(Outcome::NotInstalled) => "not on this machine".to_owned(),
                Ok(Outcome::Unchanged) => "as it was".to_owned(),
                Ok(Outcome::Changed) => changed.to_owned(),
                Err(e) => {
                    failed = true;
                    format!("failed: {e}")
                }
            };
            (r.harness.name().to_owned(), state, r.path.display().to_string())
        })
        .collect();
    (lines, failed)
}

fn statuses(reports: Vec<olo_agent_context::HarnessReport<std::io::Result<Status>>>) -> (Vec<Line>, bool) {
    let mut failed = false;
    let lines = reports
        .into_iter()
        .map(|r| {
            let state = match r.result {
                Ok(Status::NotInstalled) => "not on this machine".to_owned(),
                Ok(Status::Absent) => "not there".to_owned(),
                Ok(Status::Current) => "there".to_owned(),
                Ok(Status::Stale) => "there, of an earlier Olo or changed".to_owned(),
                Err(e) => {
                    failed = true;
                    format!("failed: {e}")
                }
            };
            (r.harness.name().to_owned(), state, r.path.display().to_string())
        })
        .collect();
    (lines, failed)
}

fn skill_line((state, path): (String, PathBuf)) -> Line {
    ("the bridge's skill for codex and agy".to_owned(), state, path.display().to_string())
}

/// The bridge's skill where Codex and Antigravity look (`put`), or
/// taken away from there (`!put`): what was done, and the file.
fn shared_skill(home: &Path, env: &Env, put: bool) -> (String, PathBuf) {
    let dir = home.join(SHARED_SKILL_DIR);
    let file = dir.join("SKILL.md");
    let ours = dir.join(STAMP).exists();
    if !put {
        if !ours {
            return ("not Olo's: left".to_owned(), file);
        }
        return match std::fs::remove_dir_all(&dir) {
            Ok(()) => ("removed".to_owned(), file),
            Err(e) => (format!("failed: {e}"), file),
        };
    }
    if !(Harness::Codex.is_present(home, env) || Harness::Antigravity.is_present(home, env)) {
        return ("neither is on this machine".to_owned(), file);
    }
    let Ok(skill) = std::fs::read(home.join(SKILL)) else {
        return ("the bridge's skill isn't on this host yet".to_owned(), file);
    };
    if file.exists() && !ours {
        return ("one that isn't Olo's is there: left".to_owned(), file);
    }
    if std::fs::read(&file).is_ok_and(|there| there == skill) {
        return ("as it was".to_owned(), file);
    }
    let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(STAMP), b"")).and_then(|()| {
        let tmp = dir.join(format!(".SKILL.md.olo-tmp.{}", std::process::id()));
        std::fs::write(&tmp, &skill).and_then(|()| std::fs::rename(&tmp, &file))
    });
    match written {
        Ok(()) => ("written".to_owned(), file),
        Err(e) => (format!("failed: {e}"), file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-context-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The bridge's skill goes where Codex and Antigravity look only
    /// where the person has one of them, is kept the same as Claude
    /// Code's, is never written over one that isn't Olo's, and is taken
    /// away only where it is Olo's.
    #[test]
    fn the_bridges_skill_is_put_where_the_others_look() {
        let home = scratch("skill");
        let env = Env::default();
        std::fs::create_dir_all(home.join(".claude/skills/olo-rex-bridge")).unwrap();
        std::fs::write(home.join(SKILL), "the skill, first\n").unwrap();
        // Neither Codex nor Antigravity here: nothing made.
        assert_eq!(shared_skill(&home, &env, true).0, "neither is on this machine");
        assert!(!home.join(".agents").exists());
        // Codex is: written, with Olo's stamp; again, as it was; changed for Claude Code, written again.
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let file = home.join(SHARED_SKILL_DIR).join("SKILL.md");
        assert_eq!(shared_skill(&home, &env, true), ("written".to_owned(), file.clone()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the skill, first\n");
        assert!(home.join(SHARED_SKILL_DIR).join(STAMP).exists());
        assert_eq!(shared_skill(&home, &env, true).0, "as it was");
        std::fs::write(home.join(SKILL), "the skill, second\n").unwrap();
        assert_eq!(shared_skill(&home, &env, true).0, "written");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the skill, second\n");
        // Taken away, it is gone; one the person put there is left, both ways.
        assert_eq!(shared_skill(&home, &env, false).0, "removed");
        assert!(!home.join(SHARED_SKILL_DIR).exists());
        std::fs::create_dir_all(home.join(SHARED_SKILL_DIR)).unwrap();
        std::fs::write(&file, "the person's own\n").unwrap();
        assert_eq!(shared_skill(&home, &env, true).0, "one that isn't Olo's is there: left");
        assert_eq!(shared_skill(&home, &env, false).0, "not Olo's: left");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the person's own\n");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The command itself, in a scratch home: each agent's line, by
    /// whether it is on the machine; and nothing written twice.
    #[test]
    fn the_command_says_each_agents_line() {
        let home = scratch("lines");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let env = Env::default();
        let (lines, failed) = outcomes(olo_agent_context::install_all(&home, &env), "written");
        assert!(!failed);
        let said: Vec<(&str, &str)> = lines.iter().map(|(who, state, _)| (who.as_str(), state.as_str())).collect();
        assert_eq!(said, [("claude", "written"), ("codex", "not on this machine"), ("agy", "not on this machine")]);
        let (again, _) = outcomes(olo_agent_context::install_all(&home, &env), "written");
        assert_eq!(again[0].1, "as it was");
        let (there, _) = statuses(olo_agent_context::status_all(&home, &env));
        assert_eq!(there[0].1, "there");
        assert!(std::fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap().contains("olo open"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
