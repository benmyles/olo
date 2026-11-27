---
name: olo-debug-help
description: Get a second investigator on a bug or a hard problem in Olo - start an Antigravity CLI (agy, Gemini 4 Neon at xhigh) as a child tab of your own Rex tab (Rex's native subagents, through `olo agent`; a tmux session where you aren't in a Rex pane) with a written brief, let it dig in parallel while you go on (reading its screen, answering what it asks leave for, steering it), then check what it found. Use it freely, two brains are better than one - whenever you're unsure why something goes wrong (a hang, a crash, a wrong pixel, a flaky test, a tool that misbehaves), when a problem is complex (several programs, protocols or threads between cause and symptom), when your first theories didn't hold, or when a bug only happens on the person's real session. For finding causes and checking reasoning, not for doing the work itself.
---

# A second investigator, for a cause you can't find

(Where it runs: since 2026-11-26 the helper is a child tab of the tab
you are in, started and read through `olo agent` by `tools/dev/agy-help`
itself, and watched on `olo agent watch`; a tmux session only where you
aren't in a Rex pane. Nothing below changes with it: the brief, what it
may do unasked, peek, say and approve are the tool's. The `olo-agents`
skill has the whole of it. Keep its watch going for as long as it runs:
the front door's own wait, run in the background.)

Olo's rule is that every problem is root-caused, never worked around.
Two brains are better than one (Ben, 2026-11-18): whenever you're
unsure of a cause, or the problem is a complex one, put a second
investigator on it and keep working on your own line meanwhile. Don't
wait until you've circled for hours, and never settle for a guess. It
is one of the helpers CLAUDE.md's hierarchy allows: Gemini 4 Neon at
`xhigh` in the Antigravity CLI (`agy`), running as a native Rex child
tab under the minder (`agy` Neon 4 `high`, or tmux where there is no
Rex pane), never silent. The helper investigates and reports; the work
stays yours.

The first time one was used, it found in an hour what a day's reading
hadn't: a pointer that vanished in Chrome was the Wayland proxy
dropping cursor shapes after a restart, a program nobody had been
looking at, with a reproduction and a patch that checked out.

## When

- You're unsure why something goes wrong, and a quick first look (the
  logs, the code on the path: minutes, not an hour) hasn't shown it.
  Start it then, before forming your own theories, not once you're
  stuck (2026-11-19: "if you're stuck or having trouble ask astra 6.2
  for help", Gemini 4 Neon now; and CLAUDE.md, "A bug you can't see at
  once").
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
   Nothing private goes in it: not the person's name, email, accounts,
   hosts or files, nor what their windows show (the brief leaves this
   computer). And say in it that a test's processes are only the PIDs
   its launcher gave it: the person's own apps have the same names.

2. **Start it in the background** and go on with other work:

   ```sh
   tools/dev/agy-help /var/tmp/YOURS/brief.md
   ```

   Run it with `run_in_background`. It starts the helper, an `agy` in a
   tmux session named `help-YOURS` (Gemini 4 Neon at xhigh, in the
   repository), with rules in front of your brief: no changes to the
   working tree, no commits, the person's session left alone, headless
   tests under an `agy-` name, scratch in the brief's folder, no
   process looked up by its name. Then it waits, printing a line a
   minute of what the helper is doing, and returns:
   - with 0 when the helper has written `answer.md` (its findings) in
     the brief's folder and its turn is over; `notes.md` beside it
     holds each finding as it was made, and is all there is if the run
     ended early;
   - with 3 as soon as the helper needs you: it asks leave for a
     command that isn't routine (`tools/dev/agents approve help-YOURS
     y|p|no`), has a question, or has shown nothing new for 15 minutes.
     Deal with it, then run the same `agy-help` command again: it finds
     the helper it started and goes on waiting.

   It can take an hour. Meanwhile read its screen whenever you like
   (`tools/dev/agents peek help-YOURS`), and tell it what you learn on
   your own line (`tools/dev/agents say help-YOURS "…"`): a theory
   ruled out, a new log line. Reading, searching, building and testing
   it does without asking; what its rules forbid is refused outright
   (the `olo-agents` skill, "Running them", has the whole of it). When
   you've read the answer, ask what's unclear (`say`: it is still
   there), then `tools/dev/agents stop help-YOURS` and `forget`.

   Its service may stop a run whose work reads like an attack on a
   system. Say what such work is in plain words (a reliability review
   of our own installer: what state each failure leaves), ask for
   reading and small deterministic checks, not tracing or fuzzing, and
   point a second run at the first's notes so nothing is done twice.
   Not while the tree is mid-push if it will build.

3. **Check what comes back.** Its answer is a lead, in the same
   confident voice whether it's right or not. Read the code it points
   at, run what it ran, make the bug happen and stop happening
   yourself. Only then fix it, in your own commit, saying in the
   message what showed the cause.

4. **Write down what was learned** where it belongs (the commit,
   `docs/HACKING.md`, `docs/AGENT-NOTES.md`), and remove the scratch
   folder when you're done.

## If it doesn't start

`agy` must be on PATH and signed in (run `agy` once yourself to see).
The tool refuses a folder in `/tmp`, and says so. If the helper's
session ends at once, `tools/dev/agents peek help-YOURS` shows what its
program said (the pane is kept); if the model's name is refused, `agy
models` lists them: fix `tools/dev/agents`, don't work round it.
