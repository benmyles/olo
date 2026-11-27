//! A minder: an agent of its own that watches over agents of a parent
//! tab, answers what they ask at once by per-agent rules, and escalates
//! what those rules don't cover.
//!
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
//! Ben, 2026-11-27: "the way minders work right now is not very
//! efficient because they always are a 1:1 with what they're minding and
//! get nested under it. change it so that a minder can mind n other
//! agents and those agents get nested under the minder. that would be
//! way more efficient."
//!
//! A shared minder is a child tab of the tab that asked for it (the
//! lead's), and the agents it minds are child tabs of the minder:
//! `lead ▸ minder ▸ agents`. The lead keeps one minder per
//! `CLI:MODEL:EFFORT` up to [`DEFAULT_MINDS_MOST`] agents (overridden
//! via `OLO_REX_AGENTS_MINDS_MOST` for tests), so N minded agents cost
//! N + 1 tabs instead of 2N. Each minded agent keeps its own rules file
//! (`minders/AGENT_ID/rules.md`) and answers log. Older 1:1 minders
//! (`minds: Some(id)`, nested under their single agent) already running
//! when a new service takes over continue to work until they end.
//!
//! What a minder can do is the service's to enforce, not the minder's to
//! remember: it reaches only the agents it minds and nothing else, by
//! one small command of its own (`mind`, written for it:
//! [`multi_script`], or [`script`] for the older 1:1 kind). Its standing
//! instructions are the same for every minder ([`multi_brief`]); each
//! agent's rules are the parent's words, read from that agent's rules
//! file before answering it and re-read when told they changed.

use super::model::AgentCli;

/// The command a minder has, in its own folder: every one of the few
/// things it may do, for the agents it minds alone.
pub const COMMAND: &str = "mind";

/// The events of a minded agent that are its minder's to answer: a
/// parent's watch isn't woken by them while the minder is there.
pub const MINDERS_OWN: [&str; 2] = ["waiting_approval", "waiting_question"];

/// What a parent's watch is told when a minder has something for it.
pub const ESCALATION: &str = "escalation";

/// What a shared minder's watch is told when its roster or an agent's
/// rules change (`added`, `removed`, `rules`): names the agent and its
/// rules file, and wakes no other watch.
pub const MINDING: &str = "minding";

/// How many live agents one shared minder minds before a second minder
/// of the same `CLI:MODEL:EFFORT` is started under the asking tab
/// (overridden by `OLO_REX_AGENTS_MINDS_MOST` for tests).
pub const DEFAULT_MINDS_MOST: usize = 12;

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

/// What `--minder` (on `start`) or `--with` (on `mind`) names: either a
/// `CLI:MODEL:EFFORT` (reusing a running shared minder of that kind
/// under the asker's tab with room, or starting one) or `NAME` (no
/// colons: an existing running shared minder by name or id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinderChoice {
    Spec(MinderSpec),
    Named(String),
}

impl MinderChoice {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("`--minder` takes `claude:MODEL:EFFORT` or a running minder's NAME".into());
        }
        if trimmed.contains(':') { MinderSpec::parse(trimmed).map(Self::Spec) } else { Ok(Self::Named(trimmed.to_owned())) }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Spec(spec) => spec.describe(),
            Self::Named(name) => name.clone(),
        }
    }
}

/// `olo agent mind NAME …`: a minder attached to a running agent,
/// changed, given new rules, taken off, or asked about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MindSpec {
    pub target: String,
    pub with: Option<MinderChoice>,
    pub rules: Option<String>,
    pub rules_file: Option<String>,
    /// What the minder's own environment has besides the service's
    /// (`--env K=V`).
    pub env: Vec<(String, String)>,
    pub off: bool,
    pub json: bool,
    /// `--approve-all-but FILE` (its text): leave given by a list, by
    /// the service itself, from now; `--approve-none`: no longer.
    pub leave: Option<String>,
    pub leave_off: bool,
}

pub fn mind_usage() -> String {
    "usage: olo agent mind <ID|NAME> [--with claude:MODEL:EFFORT|NAME] [--rules FILE | --rules-text TEXT] [--env K=V]... [--off] [--approve-all-but FILE | --approve-none] [--json]\n  \
     --approve-all-but FILE   the service itself says yes, once, to what it asks leave for, except what a pattern of FILE names (a regular expression a line), which is asked as before\n  \
     --approve-none      no longer: everything it asks leave for is asked\n  \
     --with …            put it under your tab's shared minder for claude:MODEL:EFFORT (starting one if none has room) or under a running minder NAME\n  \
     --rules FILE        what the minder may approve, must refuse, how to steer, when to tell you (updates the running minder without restarting it)\n  \
     --off               take it off its minder (moves it back under the minder's parent tab; its asking comes to you again)\n  \
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
                "--with" | "--minder" => spec.with = Some(MinderChoice::parse(&value(&mut i, "--with")?)?),
                "--rules" | "--rules-file" => spec.rules_file = Some(value(&mut i, "--rules")?),
                "--rules-text" => spec.rules = Some(value(&mut i, "--rules-text")?),
                "-e" | "--env" => {
                    let kv = value(&mut i, "--env")?;
                    let Some((k, v)) = kv.split_once('=') else { return Err(format!("--env expects KEY=VALUE, got `{kv}`")) };
                    spec.env.push((k.to_owned(), v.to_owned()));
                }
                "--off" | "--stop" => spec.off = true,
                "--approve-all-but-text" => spec.leave = Some(value(&mut i, "--approve-all-but")?),
                "--approve-none" => spec.leave_off = true,
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
            return Err("`--off` takes it off its minder: it takes no minder and no rules".into());
        }
        if spec.rules.is_some() && spec.rules_file.is_some() {
            return Err("`--rules FILE` or `--rules-text TEXT`, not both".into());
        }
        if spec.leave.is_some() && spec.leave_off {
            return Err("`--approve-all-but FILE` or `--approve-none`, not both".into());
        }
        Ok(spec)
    }
}

/// What a shared minder's own command is, as a shell script: each thing
/// it may do for the agents it minds, asked of the service through
/// `helper` (Olo's helper on this host, by its whole path). The service
/// still knows who asks and holds it to its own minded agents alone;
/// one tool rule lets it work without leave.
pub fn multi_script(helper: &str) -> String {
    let helper = shell_word(helper);
    format!(
        r#"#!/bin/sh
# A shared minder's command: the few things it may do, for the agents it minds.
# Written by Olo's agents service; the service knows who asks, too.
helper={helper}
what=$1
[ $# -gt 0 ] && shift
case "$what" in
watch)
    case "$1" in
    "" | -*) exec "$helper" agent watch --all "$@" ;;
    *) exec "$helper" agent watch "$@" ;;
    esac
    ;;
list) exec "$helper" agent list "$@" ;;
status | read) exec "$helper" agent "$what" "$@" ;;
approve) exec "$helper" agent approve "$@" ;;
send) exec "$helper" agent mind-send "$@" ;;
escalate) exec "$helper" agent escalate "$@" ;;
*)
    echo "usage: {COMMAND} watch|list|status|read|approve|send|escalate …" >&2
    exit 2
    ;;
esac
"#
    )
}

/// What an older 1:1 minder's own command is, as a shell script: each
/// thing it may do, for the one agent `minded` (its id) alone. Kept for
/// compatibility with 1:1 minders already running across an upgrade.
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
send) exec "$helper" agent mind-send "$it" "$@" ;;
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
/// [`CLAUDE_TOOLS`]: in the shell, anything but this command is refused
/// at once, never asked about (it has no one to ask, and a minder
/// waiting for leave minds nothing).
pub fn allowed_tool(path: &str) -> String {
    format!("Bash({path} *)")
}

/// What a minder may do without asking: run its one command, and read.
/// It reads whatever it needs to judge a question (the script a command
/// would run, the brief its agent was given, the code it asks about):
/// reading changes nothing, and a minder that could read nothing handed
/// back every question about a file, which is most of them. Ben,
/// 2026-11-27, of one that could not read the script it was asked
/// about: "why don't we let a minder read files that seems silly i
/// would think minders should have full read only access by default".
pub fn allowed_tools(path: &str) -> Vec<String> {
    READS.iter().map(|tool| (*tool).to_owned()).chain([allowed_tool(path)]).collect()
}

/// Claude Code's tools that only read: a file, a search of files'
/// contents, a search of their names.
pub const READS: [&str; 3] = ["Read", "Grep", "Glob"];

/// Claude Code's permission mode for a minder: what its rules don't
/// allow is refused, not asked.
pub const CLAUDE_MODE: &str = "dontAsk";

/// Claude Code's arguments that leave a minder the shell (for its one
/// command) and the tools that only read, of all its tools: nothing
/// that writes, edits, fetches or starts anything.
pub const CLAUDE_TOOLS: [&str; 2] = ["--tools", "Bash,Read,Grep,Glob"];

/// What a shared minder is first told: to begin watching all of its
/// agents and roster/rules updates. (Its standing instructions,
/// [`multi_brief`], are its system's, so a long watch that is summed up
/// never loses how it works.)
pub fn multi_begin(path: &str, cursor: u64) -> String {
    format!("Begin minding now. Run: {path} watch --since {cursor} --on minding,question,approval,idle,error,exit --timeout 90 --json")
}

/// A shared minder's standing instructions: how it watches over the
/// agents nested under it, reads each agent's own rules file before
/// answering it and again when told its rules changed, answers each
/// agent strictly by that agent's rules, escalates what they don't
/// cover, and reports each agent's exit while continuing to mind the
/// rest.
pub fn multi_brief(path: &str, cursor: u64) -> String {
    format!(
        r#"You are a minder. Your job is to watch over the agents that run as child tabs under yours. The agent that started you (the parent) is busy with other work and leaves per-agent rules for each agent you mind. You answer what each agent asks at once, strictly by THAT agent's rules, so none sits waiting; what an agent's rules don't cover, you escalate to the parent. You do none of their work, none of the parent's, and nothing else at all.

Each agent you mind has its own rules file (`minders/AGENT_ID/rules.md`), named in its `minding` event and in `{path} list --json`. Always read an agent's rules file with the Read tool before answering it the first time, and re-read it whenever a `minding` event tells you its rules changed. Never apply one agent's rules to another agent. If an agent's rules file is empty, approve nothing and answer nothing for it: escalate every approval and question for it, word for word.

You have one command to act with, and you may read: any file, with the Read, Grep and Glob tools (each agent's rules file, the script a command would run, the brief an agent was given, the code it asks about). Read what a question is about before you answer it. Every other tool is closed to you, and in the shell everything but your command (nobody is there to give you leave, so never try anything else):

  {path} watch --since CURSOR --on minding,question,approval,idle,error,exit --timeout 90 --json
      Waits for a roster or rules update (`"event": "minding"`) or for any agent you mind to ask leave, ask a question, come to rest, fail or exit. It returns the moment one happens (exit code 0) with a "cursor", and `name` and `id` saying WHICH agent it is about; with nothing to say it returns code 124 after 90 seconds: run it again with the same cursor. Begin with CURSOR = {cursor}. Always pass the cursor of the last answer. For each agent you are told what stands, once: when an agent is added while already asking, you hear its `minding` event and then its standing question at once; if several things happened for an agent while you were busy, you hear the last, not each; what you have been told and left standing (a question you passed up) is not told to you again unless its rules change.
  {path} list --json
      Lists every agent you currently mind: its `id`, `name`, what it is, where it works (`cwd`), how it stands (`state`), and its `rules` file path.
  {path} status ID --json         how that agent stands now (what it asks, with the choices); NAME also works when unambiguous
  {path} read ID --lines 80       that agent's screen, when you need to see what led to a question
  {path} approve ID --once        yes, this once.   {path} approve ID --no --reason "WHY"    no, and why.
      (Never a yes that is kept: no "don't ask again", no "always allow". `approve` can't pick those, and you never try another way.)
      `approve` answers the one question you were last told of for ID (by `watch` or `status`) under its current rules. If it says the question or rules changed, nothing was answered: re-read its rules if they changed, run `status ID --json`, judge what stands asked now, then answer that.
  {path} send ID "TEXT" --enter   says TEXT to ID: an answer to its non-approval question, or a steer at rest (chooser nav keys `--key Up|Down|Tab|Space|Enter|Escape` also work on non-approval choosers). Never used for approval boxes.
  {path} escalate ID "TEXT"       tells the parent about ID (always pass the event's immutable `id`, e.g. `sa-…`, so a reused name can never be misattributed). It reaches the parent's own watch at once as that agent's `escalation`. Say which question, word for word, what you did or didn't do, and what you need.
  {path} escalate "TEXT"          tells the parent about yourself (if you hit a problem or need attention).

The loop you keep, for as long as you run:
1. `watch` (as above). On code 124, watch again. Do not think between watches when nothing happened.
2. A roster or rules update (`"event": "minding"`): inspect `minding.action` (`"added"`, `"rules"`, `"removed"`), `id`, `name`, and `minding.rules`. On `"added"` or `"rules"`, read `minding.rules` with Read (and note `minding.what` and `minding.cwd`). On `"removed"`, that agent is no longer yours to mind. Then watch again.
3. An approval (`"waiting_approval"`): make sure you have read THAT agent's current rules file (`{path} list --json` has its path if you forgot). Read what it asks leave for (`status ID --json` has it whole; `read ID` if you need what came before). If that agent's rules plainly allow it, `approve ID --once`. If they plainly refuse it, `approve ID --no --reason "…"` with the rule, and what to do instead if the rules say. If its rules don't say, or you are not sure, do NOT guess: `escalate ID "…"` word for word, and leave it waiting.
4. A question (`"waiting_question"`): answer with `send ID …` if that agent's rules give the answer; else `escalate ID "…"`.
5. At rest (`"idle"` or `"done"`): if that agent's rules say how to go on (a next step, a check to ask for), `send ID …`. If it has finished what it was asked, `escalate ID "…"` one line saying so, with where its result is if it said.
6. A failure (`"error"`): `escalate ID "…"` with what it says.
7. An agent exits (`"exited"`): `escalate ID "…"` (using the exited event's immutable `id`) with one short summary for that agent (what you approved, what you refused, what you passed up), then continue watching for your remaining agents. When no live agents remain, the service stops you once all pending exit reports are sent (or when the reporting grace deadline expires).

Never: start an agent or any other program; stop any agent; edit or write files; apply one agent's rules to another; approve what an agent's rules don't plainly allow; answer a question about the work itself from your own judgment where the rules are silent; type a secret, a password or a token into any agent."#
    )
}

/// What an older 1:1 minder is first told: to begin. (Its standing
/// instructions, [`brief`], are its system's, so that a long watch that
/// is summed up never loses the rules.)
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

You have one command to act with, and you may read: any file, with the Read, Grep and Glob tools (the script a command would run, the brief "{name}" was given, the code it asks about). Read what a question is about before you answer it. Every other tool is closed to you, and in the shell everything but your command (nobody is there to give you leave, so never try anything else):

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

Never: start an agent or any other program; stop "{name}" (unless the rules say exactly when); edit or write files; approve what the rules don't plainly allow; answer a question about the work itself from your own judgment where the rules are silent; type a secret, a password or a token into it.

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
            (attach.target.as_str(), attach.with.as_ref().map(MinderChoice::describe).as_deref(), attach.rules_file.as_deref(), attach.off, attach.json),
            ("api-audit", Some("claude:claude-fable-5-2:high"), Some("rules.md"), false, true)
        );
        let named = MindSpec::parse(&args(&["api-audit", "--with", "minder-2"])).unwrap();
        assert_eq!(named.with, Some(MinderChoice::Named("minder-2".into())));
        let rules = MindSpec::parse(&args(&["api-audit", "--rules-text", "Approve cargo test."])).unwrap();
        assert_eq!((rules.with, rules.rules.as_deref()), (None, Some("Approve cargo test.")));
        assert!(MindSpec::parse(&args(&["api-audit", "--off"])).unwrap().off);
        assert_eq!(MindSpec::parse(&args(&["api-audit"])).unwrap(), MindSpec { target: "api-audit".into(), ..Default::default() });
        for bad in [&[][..], &["--off"], &["a", "b"], &["a", "--off", "--rules", "r"], &["a", "--rules", "r", "--rules-text", "t"], &["a", "--with"], &["a", "--nope"]] {
            assert!(MindSpec::parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_minder_choice_is_either_a_spec_or_a_running_minders_name() {
        assert_eq!(MinderChoice::parse("claude:claude-opus-5-5:max").unwrap(), MinderChoice::Spec(MinderSpec { cli: AgentCli::Claude, model: "claude-opus-5-5".into(), effort: "max".into() }));
        assert_eq!(MinderChoice::parse("minder").unwrap(), MinderChoice::Named("minder".into()));
        assert_eq!(MinderChoice::parse(" minder-2 ").unwrap(), MinderChoice::Named("minder-2".into()));
        for bad in ["", "   ", "claude:opus", "claude::high", "codex:m:e", "claude:m:e:x"] {
            assert!(MinderChoice::parse(bad).is_err(), "{bad}");
        }
    }

    /// A shared minder's command (`multi_script`) names the agent where
    /// one is meant, watches all its agents by default, lists them, and
    /// escalates about one or about itself.
    #[test]
    fn the_shared_minders_command_routes_each_action() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-shared-minder-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("it's a helper");
        std::fs::write(&helper, "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done\n").unwrap();
        let mind = dir.join(COMMAND);
        std::fs::write(&mind, multi_script(&helper.to_string_lossy())).unwrap();
        for f in [&helper, &mind] {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let run = |args: &[&str]| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
            let out = loop {
                match std::process::Command::new(&mind).args(args).output() {
                    Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && std::time::Instant::now() < until => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    res => break res.unwrap(),
                }
            };
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        assert_eq!(run(&["watch", "--since", "7", "--json"]), (Some(0), "[agent][watch][--all][--since][7][--json]".into()));
        assert_eq!(run(&["watch"]), (Some(0), "[agent][watch][--all]".into()));
        assert_eq!(run(&["watch", "alpha", "--since", "7", "--json"]), (Some(0), "[agent][watch][alpha][--since][7][--json]".into()));
        assert_eq!(run(&["list", "--json"]).1, "[agent][list][--json]");
        assert_eq!(run(&["status", "alpha", "--json"]).1, "[agent][status][alpha][--json]");
        assert_eq!(run(&["read", "alpha", "--lines", "80"]).1, "[agent][read][alpha][--lines][80]");
        assert_eq!(run(&["approve", "alpha", "--no", "--reason", "not in rules"]).1, "[agent][approve][alpha][--no][--reason][not in rules]");
        assert_eq!(run(&["send", "alpha", "Run the tests first.", "--enter"]).1, "[agent][mind-send][alpha][Run the tests first.][--enter]");
        assert_eq!(run(&["escalate", "alpha", "It asks to push."]).1, "[agent][escalate][alpha][It asks to push.]");
        assert_eq!(run(&["escalate", "I need attention."]).1, "[agent][escalate][I need attention.]");
        for refused in [&["stop", "alpha"][..], &["start", "claude"], &["prune"], &[]] {
            assert_eq!(run(refused), (Some(2), String::new()), "{refused:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shared_minders_instructions_cover_multiple_agents_and_per_agent_rules() {
        let b = multi_brief("/state/minders/sa-m1/mind", 412);
        for must in [
            "/state/minders/sa-m1/mind watch --since CURSOR --on minding,question,approval,idle,error,exit --timeout 90 --json",
            "Begin with CURSOR = 412",
            "/state/minders/sa-m1/mind list --json",
            "/state/minders/sa-m1/mind status ID --json",
            "/state/minders/sa-m1/mind approve ID --once",
            "/state/minders/sa-m1/mind escalate ID \"TEXT\"",
            "/state/minders/sa-m1/mind escalate \"TEXT\"",
            "Never apply one agent's rules to another agent",
            "\"event\": \"minding\"",
            "do NOT guess",
        ] {
            assert!(b.contains(must), "missing: {must}");
        }
        assert_eq!(
            multi_begin("/state/minders/sa-m1/mind", 412),
            "Begin minding now. Run: /state/minders/sa-m1/mind watch --since 412 --on minding,question,approval,idle,error,exit --timeout 90 --json"
        );
    }

    /// The minder's command reaches the one agent, whatever is asked of
    /// it: run for real, with a stand-in helper that says what it was
    /// given.
    #[test]
    fn the_minders_command_names_its_agent_for_it() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-minder-script-{}", std::process::id()));
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
            let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
            let out = loop {
                match std::process::Command::new(&mind).args(args).output() {
                    Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && std::time::Instant::now() < until => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    res => break res.unwrap(),
                }
            };
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        assert_eq!(run(&["watch", "--since", "7", "--json"]), (Some(0), "[agent][watch][sa-1f2e3d][--since][7][--json]".into()));
        assert_eq!(run(&["status", "--json"]).1, "[agent][status][sa-1f2e3d][--json]");
        assert_eq!(run(&["read", "--lines", "80"]).1, "[agent][read][sa-1f2e3d][--lines][80]");
        assert_eq!(run(&["approve", "--no", "--reason", "not in the rules; it's a push"]).1, "[agent][approve][sa-1f2e3d][--no][--reason][not in the rules; it's a push]");
        assert_eq!(run(&["send", "Run the tests first.", "--enter"]).1, "[agent][mind-send][sa-1f2e3d][Run the tests first.][--enter]");
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
            "You have one command to act with, and you may read: any file, with the Read, Grep and Glob tools",
            "Every other tool is closed to you, and in the shell everything but your command",
            "edit or write files",
            "do NOT guess",
            "Never: start an agent",
        ] {
            assert!(b.contains(must), "missing: {must}");
        }
        // (No rules: everything goes up.)
        assert!(brief("a", "an agent", "/m", 0, " \n").contains("Approve nothing, answer nothing"));
        assert!(b.ends_with("\n----"), "the rules are its last word");
        assert_eq!(allowed_tool("/state/minders/sa-1/mind"), "Bash(/state/minders/sa-1/mind *)");
        // Its one command, and the tools that only read: nothing else is allowed, and nothing else is there.
        assert_eq!(allowed_tools("/state/minders/sa-1/mind"), ["Read", "Grep", "Glob", "Bash(/state/minders/sa-1/mind *)"]);
        assert_eq!(CLAUDE_TOOLS, ["--tools", "Bash,Read,Grep,Glob"]);
        assert_eq!(
            begin("api-audit", "/state/minders/sa-1/mind", 412),
            "Begin minding \"api-audit\" now. Run: /state/minders/sa-1/mind watch --since 412 --on question,approval,idle,error,exit --timeout 90 --json"
        );
    }
}
