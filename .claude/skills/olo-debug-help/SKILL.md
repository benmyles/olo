---
name: olo-debug-help
description: Get a second investigator on a bug or a hard problem in Olo - start a headless Codex (gpt-6.2-astra at its highest reasoning effort, approving its own steps) with a written brief, let it dig in parallel while you go on, then check what it found. Use it freely, two brains are better than one - whenever you're unsure why something goes wrong (a hang, a crash, a wrong pixel, a flaky test, a tool that misbehaves), when a problem is complex (several programs, protocols or threads between cause and symptom), when your first theories didn't hold, or when a bug only happens on the person's real session. For finding causes and checking reasoning, not for doing the work itself.
---

# A second investigator: Codex, for a cause you can't find

Olo's rule is that every problem is root-caused, never worked around.
Two brains are better than one (Ben, 2026-11-18): whenever you're
unsure of a cause, or the problem is a complex one, put a second
investigator on it and keep working on your own line meanwhile. Don't
wait until you've circled for hours, and never settle for a guess. It
is one of the Codex helpers CLAUDE.md's "Subagents" allows (the others:
Codex subagents for narrow work items, `tools/dev/agents` and the
`olo-codex-agents` skill; and `codex-look` for pictures): the helper
investigates and reports; the work stays yours.

The first time it was used, it found in an hour what a day's reading
hadn't: a pointer that vanished in Chrome was the Wayland proxy
dropping cursor shapes after a restart, a program nobody had been
looking at, with a reproduction and a patch that checked out.

## When

- You're unsure why something goes wrong, and a quick first look (the
  logs, the code on the path: minutes, not an hour) hasn't shown it.
  Start it then, before forming your own theories, not once you're
  stuck (2026-11-19: "if you're stuck or having trouble ask astra 6.2
  for help", and CLAUDE.md, "A bug you can't see at once").
- The problem is complex: several programs, protocols or threads lie
  between the cause and what's seen; or your first theories didn't hold.
- It happens only on the person's real session (their hardware, their
  apps) and you need better theories, or a way to catch it in the act.
- You have a cause but aren't sure of it: have it argued with.
- Start it early and in parallel. Even a short brief is worth it, as
  long as it says what you've seen and ruled out (a helper told
  nothing does your first hour over again).
- Not to write the fix or do the task, and not for what a minute's
  reading would answer.

## How

1. **Write the brief** in a folder of your own under `/var/tmp` (never
   `/tmp`), as `brief.md`. The helper knows nothing you don't tell it,
   so the brief is the whole hand-over:
   - the symptom in the person's own words, and when it was seen;
   - what should happen instead;
   - the evidence, exact: log lines, crash reports, numbers, the
     commands that produced them, where the files are;
   - what you tried and what each attempt showed, so it isn't done
     again; what you ruled out and how;
   - the code on the path (`file:line`), and the docs that explain it
     (which part of `docs/HACKING.md`);
   - how to reproduce as far as you got (the `tsession` commands, test
     pages, scratch files it may use);
   - what you'd like back: the cause, or a way to catch it.
   Nothing private goes in it: not the person's name, email, accounts
   or files (the brief leaves this computer).

2. **Start it in the background** and go on with other work:

   ```sh
   tools/dev/codex-help /var/tmp/YOURS/brief.md
   ```

   Run it with `run_in_background` and take its completion notice; it
   can take an hour. It runs `codex exec -m gpt-6.2-astra -c
   model_reasoning_effort="max" --approve-for-me` in the repository,
   with rules in front of your brief: no changes to the working tree,
   no commits, the person's session left alone, headless tests under a
   `codex-` name, scratch in the brief's folder. It writes
   `answer.md` (its findings: its last message, which replaces any file
   of that name, so a brief never asks it to write `answer.md` itself),
   `log.txt` (what it did) and `notes.md` (each finding as it makes
   it) there. A run can end before its answer: the tool then says why
   (the log's last error) and the notes hold what it had found. Its
   service stops a run whose work reads like an attack on a system: a
   review asked to "attack" a root tool, to try "hostile" input,
   "escapes" and fuzzing was cut off nine minutes in (2026-11-20). Say
   what such work is in plain words (a reliability review of our own
   installer: what state each failure leaves), ask for reading and
   small deterministic checks, not tracing or fuzzing, and point a
   second run at the first's log and notes so nothing is done twice.
   One at a time, and not while the tree is mid-push if it will build.

3. **Check what comes back.** Its answer is a lead, in the same
   confident voice whether it's right or not. Read the code it points
   at, run what it ran, make the bug happen and stop happening
   yourself. Only then fix it, in your own commit, saying in the
   message what showed the cause.

4. **Write down what was learned** where it belongs (the commit,
   `docs/HACKING.md`, `docs/AGENT-NOTES.md`), and remove the scratch
   folder when you're done.

## If it doesn't start

`codex` must be on PATH and signed in (`codex login status`). The
script refuses a folder in `/tmp`, and says so. If the model or the
effort is refused, read `codex exec --help` and `~/.codex/config.toml`
before changing the script: fix the script, don't work round it.
