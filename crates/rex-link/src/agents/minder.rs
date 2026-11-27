//! A minder: an agent of its own whose whole job is one other agent.
//! Ben, 2026-11-26: "often when subagents need an approval or have a
//! question it takes a long time for parent agents to notice and respond
//! because they are busy with something else … allow parent agents, when
//! starting an olo rex agent (also with the ability to add/change it for
//! a running agent), to attach an approval/minder agent specifically for
//! each agent where the parent agent can choose to use a more powerful
//! model … and give it context and instructions on what to approve/deny
//! and how to steer that specific agent it is minding … and also giving
//! … the minder the ability to message the parent when something needs
//! escalation."
//!
//! A minder is a child tab of the agent it minds (so the person sees it
//! under that agent, and stopping the agent's tree stops it too). It
//! waits on that agent, answers what it asks at once by the parent's
//! written rules, and tells the parent what the rules don't cover: an
//! `escalation`, which is an event on the parent's own watch, never
//! something typed into the parent's pane.
//!
//! What a minder can do is the service's to enforce, not the minder's to
//! remember: it reaches the one agent it minds and nothing else, by one
//! small command of its own (`mind`, written for it: [`script`]) that
//! names that agent for it. Its rules are the parent's words, set inside
//! standing instructions that are the same for every minder ([`brief`]).

use super::model::AgentCli;

/// The command a minder has, in its own folder: every one of the few
/// things it may do, each for the one agent it minds.
pub const COMMAND: &str = "mind";

/// The events of a minded agent that are its minder's to answer: a
/// parent's watch isn't woken by them while the minder is there.
pub const MINDERS_OWN: [&str; 2] = ["waiting_approval", "waiting_question"];

/// What a parent's watch is told when a minder has something for it.
pub const ESCALATION: &str = "escalation";

/// The agent a minder runs as: its program, model and effort, as the
/// parent chose them (`--minder claude:claude-fable-5-2:high`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinderSpec {
    pub cli: AgentCli,
    pub model: String,
    pub effort: String,
}

impl MinderSpec {
    /// `CLI:MODEL:EFFORT`. A minder runs Claude Code for now: its tools
    /// can be held to the one command it needs, so that it never waits
    /// for a leave nobody is there to give.
    pub fn parse(raw: &str) -> Result<MinderSpec, String> {
        let usage = || format!("`--minder` takes CLI:MODEL:EFFORT (as `claude:claude-fable-5-2:high`), not `{raw}`");
        let mut parts = raw.trim().splitn(3, ':');
        let (Some(cli), Some(model), Some(effort)) = (parts.next(), parts.next(), parts.next()) else { return Err(usage()) };
        let (model, effort) = (model.trim(), effort.trim());
        if model.is_empty() || effort.is_empty() || effort.contains(':') {
            return Err(usage());
        }
        match AgentCli::parse(cli.trim()) {
            Some(AgentCli::Claude) => Ok(MinderSpec { cli: AgentCli::Claude, model: model.to_owned(), effort: effort.to_owned() }),
            Some(other) => Err(format!("a minder runs Claude Code for now (`claude:MODEL:EFFORT`), not {}", other.display_name())),
            None => Err(usage()),
        }
    }

    pub fn describe(&self) -> String {
        format!("{}:{}:{}", self.cli.binary(), self.model, self.effort)
    }
}

/// `olo agent mind NAME …`: a minder attached to a running agent,
/// replaced, given new rules, ended, or asked about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MindSpec {
    pub target: String,
    pub with: Option<MinderSpec>,
    pub rules: Option<String>,
    pub rules_file: Option<String>,
    /// What the minder's own environment has besides the service's
    /// (`--env K=V`).
    pub env: Vec<(String, String)>,
    pub off: bool,
    pub json: bool,
}

pub fn mind_usage() -> String {
    "usage: olo agent mind <ID|NAME> [--with claude:MODEL:EFFORT] [--rules FILE | --rules-text TEXT] [--env K=V]... [--off] [--json]\n  \
     --with …            attach a minder to it, or replace the one it has\n  \
     --rules FILE        what the minder may approve, must refuse, how to steer, when to tell you (your own words)\n  \
     --off               end its minder\n  \
     (nothing more)      how it is minded, and what its minder has answered\n"
        .to_owned()
}

impl MindSpec {
    pub fn parse(args: &[String]) -> Result<MindSpec, String> {
        let mut spec = MindSpec::default();
        let mut i = 0;
        let value = |i: &mut usize, flag: &str| -> Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("`{flag}` needs a value\n{}", mind_usage()))
        };
        while i < args.len() {
            match args[i].as_str() {
                "--with" | "--minder" => spec.with = Some(MinderSpec::parse(&value(&mut i, "--with")?)?),
                "--rules" | "--rules-file" => spec.rules_file = Some(value(&mut i, "--rules")?),
                "--rules-text" => spec.rules = Some(value(&mut i, "--rules-text")?),
                "-e" | "--env" => {
                    let kv = value(&mut i, "--env")?;
                    let Some((k, v)) = kv.split_once('=') else { return Err(format!("--env expects KEY=VALUE, got `{kv}`")) };
                    spec.env.push((k.to_owned(), v.to_owned()));
                }
                "--off" | "--stop" => spec.off = true,
                "--json" => spec.json = true,
                flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`\n{}", mind_usage())),
                target if spec.target.is_empty() => spec.target = target.to_owned(),
                extra => return Err(format!("one agent at a time: `{extra}` is a second\n{}", mind_usage())),
            }
            i += 1;
        }
        if spec.target.is_empty() {
            return Err(mind_usage());
        }
        if spec.off && (spec.with.is_some() || spec.rules.is_some() || spec.rules_file.is_some() || !spec.env.is_empty()) {
            return Err("`--off` ends its minder: it takes no minder and no rules".into());
        }
        if spec.rules.is_some() && spec.rules_file.is_some() {
            return Err("`--rules FILE` or `--rules-text TEXT`, not both".into());
        }
        Ok(spec)
    }
}

/// What the minder's own command is, as a shell script: each thing it
/// may do, for the agent `minded` (its id) alone, asked of the service
/// through `helper` (Olo's helper on this host, by its whole path: the
/// minder's tab need have nothing on its PATH). The service still knows
/// who asks and refuses anything else; this is so that the minder never
/// has to name the agent, and so that one tool rule lets it work.
pub fn script(helper: &str, minded: &str) -> String {
    let (helper, minded) = (shell_word(helper), shell_word(minded));
    format!(
        r#"#!/bin/sh
# A minder's command: the few things it may do, for the one agent it minds.
# Written by Olo's agents service; the service knows who asks, too.
helper={helper}
it={minded}
what=$1
[ $# -gt 0 ] && shift
case "$what" in
watch) exec "$helper" agent watch "$it" "$@" ;;
status | read) exec "$helper" agent "$what" "$it" "$@" ;;
approve) exec "$helper" agent approve "$it" "$@" ;;
send) exec "$helper" agent send "$it" "$@" ;;
escalate) exec "$helper" agent escalate "$@" ;;
*)
    echo "usage: {COMMAND} watch|status|read|approve|send|escalate …" >&2
    exit 2
    ;;
esac
"#
    )
}

/// One argument as a POSIX shell takes it whole.
fn shell_word(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The tool rule that lets a Claude Code minder run its command, at
/// `path`, without asking leave. With it go [`CLAUDE_MODE`] and
/// [`CLAUDE_TOOLS`]: the shell is the one tool it has, and anything in
/// it but this command is refused at once, never asked about (it has no
/// one to ask, and a minder waiting for leave minds nothing).
pub fn allowed_tool(path: &str) -> String {
    format!("Bash({path} *)")
}

/// Claude Code's permission mode for a minder: what its rules don't
/// allow is refused, not asked.
pub const CLAUDE_MODE: &str = "dontAsk";

/// Claude Code's arguments that leave a minder the shell alone, of all
/// its tools.
pub const CLAUDE_TOOLS: [&str; 2] = ["--tools", "Bash"];

/// What a minder is first told: to begin. (Its standing instructions,
/// [`brief`], are its system's, so that a long watch that is summed up
/// never loses the rules.)
pub fn begin(name: &str, path: &str, cursor: u64) -> String {
    format!("Begin minding \"{name}\" now. Run: {path} watch --since {cursor} --on question,approval,idle,error,exit --timeout 90 --json")
}

/// A minder's standing instructions with the parent's `rules` set in
/// them: who it minds (`name`, what that agent is and was asked), the
/// command it has (`mind`, at `path`), the loop it keeps, what it never
/// does, and how it ends. `cursor`: where the minded agent's events
/// begin for it. The same for every minder; only the rules are the
/// parent's.
pub fn brief(name: &str, what: &str, path: &str, cursor: u64, rules: &str) -> String {
    let rules = rules.trim();
    let rules = if rules.is_empty() { "(No rules were given. Approve nothing, answer nothing: escalate every approval and every question, word for word.)" } else { rules };
    format!(
        r#"You are a minder. Your whole job is one other agent, "{name}" ({what}), which runs in the tab above yours. The agent that started it (its parent) is busy with other work and has left you its rules. You answer what "{name}" asks at once, by those rules, so that it never sits waiting; what the rules don't cover, you tell the parent. You do none of "{name}"'s work, none of the parent's, and nothing else at all.

You have one command, and every tool but it is closed to you (nobody is there to give you leave, so never try anything else):

  {path} watch --since CURSOR --on question,approval,idle,error,exit --timeout 90 --json
      Waits for "{name}" to ask leave, ask a question, come to rest, fail or exit. It returns the moment one happens (exit code 0) with a "cursor"; with nothing to say it returns code 124 after 90 seconds: run it again with the same cursor. Begin with CURSOR = {cursor}. Always pass the cursor of the last answer. You are told what stands, once: at your very first watch, how "{name}" stands at that moment if that is something to act on (a question it was already asking before you began is told to you at once); and if several things happened while you were busy, you hear the last, not each. What you have been told and left standing (a question you passed up) is not told to you again.
  {path} status --json            how it stands now (what it asks, with the choices)
  {path} read --lines 80          its screen, when you need to see what led to a question
  {path} approve --once           yes, this once.   {path} approve --no --reason "WHY"    no, and why.
      (Never a yes that is kept: no "don't ask again", no "always allow". `approve` can't pick those, and you never try another way.)
      `approve` answers the one question you were last told of (by `watch` or `status`). If it says the question has changed, nothing was answered: run `status --json`, judge what stands asked now, then answer that.
  {path} send "TEXT" --enter      says TEXT to it: an answer to its question, or a steer. Short and plain.
  {path} escalate "TEXT"          tells the parent. It reaches the parent's own watch at once. Say which question, word for word, what you did or didn't do, and what you need.

The loop you keep, for as long as "{name}" runs:
1. `watch` (as above). On code 124, watch again. Do not think between watches when nothing happened.
2. An approval ("approval"): read what it asks leave for (`status --json` has it whole; `read` if you need what came before). If the rules plainly allow it, `approve --once`. If they plainly refuse it, `approve --no --reason "…"` with the rule, and what to do instead if the rules say. If the rules don't say, or you are not sure, do NOT guess: `escalate` it, word for word, and leave it waiting.
3. A question ("question"): answer with `send` if the rules give the answer; else `escalate`.
4. At rest ("idle" or "done"): if the rules say how to go on (a next step, a check to ask for), `send` that. If it has finished what it was asked, `escalate` one line saying so, with where its result is if it said.
5. A failure ("error"): `escalate` what it says.
6. It exits ("exit"): `escalate` one short summary (what you approved, what you refused, what you passed up), then stop: your work is done. Say nothing more and run nothing more.

Never: start an agent or any other program; stop "{name}" (unless the rules say exactly when); edit, write or read files; approve what the rules don't plainly allow; answer a question about the work itself from your own judgment where the rules are silent; type a secret, a password or a token into it.

The parent's rules for "{name}":
----
{rules}
----"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minder_is_named_by_its_program_model_and_effort() {
        let m = MinderSpec::parse("claude:claude-fable-5-2:high").unwrap();
        assert_eq!((m.cli, m.model.as_str(), m.effort.as_str(), m.describe().as_str()), (AgentCli::Claude, "claude-fable-5-2", "high", "claude:claude-fable-5-2:high"));
        assert_eq!(MinderSpec::parse(" claude : claude-opus-5-5 : max ").unwrap().describe(), "claude:claude-opus-5-5:max");
        for bad in ["claude", "claude:opus", "claude::high", "claude:opus:", "nobody:m:e", "claude:m:e:x"] {
            assert!(MinderSpec::parse(bad).is_err(), "{bad}");
        }
        // (Claude Code alone for now: said, not guessed at.)
        let other = MinderSpec::parse("codex:gpt-5.4-codex:high").unwrap_err();
        assert!(other.contains("Claude Code for now"), "{other}");
    }

    #[test]
    fn what_is_asked_of_a_running_agents_minder() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let attach = MindSpec::parse(&args(&["api-audit", "--with", "claude:claude-fable-5-2:high", "--rules", "rules.md", "--json"])).unwrap();
        assert_eq!(
            (attach.target.as_str(), attach.with.as_ref().map(MinderSpec::describe).as_deref(), attach.rules_file.as_deref(), attach.off, attach.json),
            ("api-audit", Some("claude:claude-fable-5-2:high"), Some("rules.md"), false, true)
        );
        let rules = MindSpec::parse(&args(&["api-audit", "--rules-text", "Approve cargo test."])).unwrap();
        assert_eq!((rules.with, rules.rules.as_deref()), (None, Some("Approve cargo test.")));
        assert!(MindSpec::parse(&args(&["api-audit", "--off"])).unwrap().off);
        assert_eq!(MindSpec::parse(&args(&["api-audit"])).unwrap(), MindSpec { target: "api-audit".into(), ..Default::default() });
        for bad in [&[][..], &["--off"], &["a", "b"], &["a", "--off", "--rules", "r"], &["a", "--rules", "r", "--rules-text", "t"], &["a", "--with"], &["a", "--nope"]] {
            assert!(MindSpec::parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    /// The minder's command reaches the one agent, whatever is asked of
    /// it: run for real, with a stand-in helper that says what it was
    /// given.
    #[test]
    fn the_minders_command_names_its_agent_for_it() {
        let dir = std::env::temp_dir().join(format!("olo-minder-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("it's a helper");
        std::fs::write(&helper, "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done\n").unwrap();
        let mind = dir.join(COMMAND);
        std::fs::write(&mind, script(&helper.to_string_lossy(), "sa-1f2e3d")).unwrap();
        for f in [&helper, &mind] {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let run = |args: &[&str]| {
            let out = std::process::Command::new(&mind).args(args).output().unwrap();
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        assert_eq!(run(&["watch", "--since", "7", "--json"]), (Some(0), "[agent][watch][sa-1f2e3d][--since][7][--json]".into()));
        assert_eq!(run(&["status", "--json"]).1, "[agent][status][sa-1f2e3d][--json]");
        assert_eq!(run(&["read", "--lines", "80"]).1, "[agent][read][sa-1f2e3d][--lines][80]");
        assert_eq!(run(&["approve", "--no", "--reason", "not in the rules; it's a push"]).1, "[agent][approve][sa-1f2e3d][--no][--reason][not in the rules; it's a push]");
        assert_eq!(run(&["send", "Run the tests first.", "--enter"]).1, "[agent][send][sa-1f2e3d][Run the tests first.][--enter]");
        assert_eq!(run(&["escalate", "It asks to push."]).1, "[agent][escalate][It asks to push.]");
        // (Nothing else: not another command of the service's, not another agent.)
        for refused in [&["stop"][..], &["start", "claude"], &["list"], &[]] {
            assert_eq!(run(refused), (Some(2), String::new()), "{refused:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_minders_instructions_hold_the_parents_rules_and_its_one_command() {
        let b = brief("api-audit", "Claude Code, claude-opus-5-5", "/state/minders/sa-1/mind", 412, "  Approve `cargo test` and `cargo build`.\nRefuse any push.  ");
        for must in [
            "\"api-audit\" (Claude Code, claude-opus-5-5)",
            "/state/minders/sa-1/mind watch --since CURSOR",
            "Begin with CURSOR = 412",
            "/state/minders/sa-1/mind approve --once",
            "/state/minders/sa-1/mind escalate \"TEXT\"",
            "----\nApprove `cargo test` and `cargo build`.\nRefuse any push.\n----",
            "every tool but it is closed to you",
            "do NOT guess",
            "Never: start an agent",
        ] {
            assert!(b.contains(must), "missing: {must}");
        }
        // (No rules: everything goes up.)
        assert!(brief("a", "an agent", "/m", 0, " \n").contains("Approve nothing, answer nothing"));
        assert!(b.ends_with("\n----"), "the rules are its last word");
        assert_eq!(allowed_tool("/state/minders/sa-1/mind"), "Bash(/state/minders/sa-1/mind *)");
        assert_eq!(
            begin("api-audit", "/state/minders/sa-1/mind", 412),
            "Begin minding \"api-audit\" now. Run: /state/minders/sa-1/mind watch --since 412 --on question,approval,idle,error,exit --timeout 90 --json"
        );
    }
}
