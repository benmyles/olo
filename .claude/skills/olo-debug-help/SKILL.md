---
name: olo-debug-help
description: Get a second investigator on a bug in Olo you can't root-cause - start a headless Codex (gpt-6.2-astra at its highest reasoning effort, approving its own steps) with a written brief, let it dig while you go on, then check what it found. Use when you have really tried to find why something goes wrong (a hang, a crash, a wrong pixel, a flaky test, a tool that misbehaves) and the cause still isn't shown, or when a bug only happens on the person's real session and you need more theories to test. Not for doing the work itself, and not before you've tried.
---

# A second investigator: Codex, for a cause you can't find

Olo's rule is that every problem is root-caused, never worked around.
When you've looked properly and the cause still isn't shown, don't
settle for a guess and don't keep circling: hand the question to a
second investigator and keep working. It is Ben's own suggestion
(2026-11-18), and the one exception to "no forks or subagents" in
CLAUDE.md: the helper investigates and reports; the work stays yours.

## When

- You've reproduced what you can, read the code on the path, and tried
  the likely causes, and it still isn't explained.
- It happens only on the person's real session (their hardware, their
  apps) and you need better theories, or a way to catch it in the act.
- Not for a first look, not to write the fix, not to review your diff.

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
   `answer.md` (its findings) and `log.txt` (what it did) there.
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
