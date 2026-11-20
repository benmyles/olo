---
name: olo-codex-agents
description: Run Codex subagents on narrow, targeted Olo work items in parallel - each an interactive Codex (gpt-6.2-astra at xhigh) in a tmux session of its own, in this working tree, scoped to one crate no other agent is in, approving routine steps itself - and start, watch, steer, review and commit their work with tools/dev/agents. Use when a task splits into independent pieces in separate crates (one app each; a big feature such as the Mail app built in parallel; the same kind of change across several apps), at most five at once. Not for finding a bug's cause (olo-debug-help) or looking at pictures (codex-look).
---

# Codex subagents: narrow work, in parallel

CLAUDE.md, "Subagents": besides the debug helper and the picture look,
narrow, targeted work items that can go on in parallel may be handed to
Codex subagents (Ben, 2026-11-19). Each is an interactive Codex
(gpt-6.2-astra, reasoning at xhigh) in a tmux session of its own, so its
work can be read as it goes, its questions answered and its course
steered. All work in this same working tree, each in one crate that no
other agent, nor you, changes meanwhile: that is what keeps them from
clobbering each other. You own the result: the design, every visual
decision, the review, the commit.

## When

- The task splits into pieces in different crates that don't depend on
  each other's unfinished work: one app each, a crate under `crates/`
  each, the same change across several apps.
- Each piece can be briefed in a page: what to change, where, what done
  is, how to test it.
- Not for: a design that needs your eye at each step (visual design is
  yours: brief the look precisely, or do it yourself), a change across
  crates that must land together (do it yourself, or split it by crate
  and run the pieces in order), anything in a crate you are changing
  yourself, a bug's cause (`olo-debug-help`).
- At most five at once. Fewer, well briefed, beat more.

## The brief

`tools/dev/agents start` puts the rules in front of it (its crate only,
`cargo fmt -p` its package only, no git that changes anything, no new
dependencies, no agents of its own, nothing on the person's screen or
session, tsession named `codex-NAME`, scratch in its folder, ask when
unsure, `report.md` then DONE). Write the task itself, specific enough
that it needn't guess:

```markdown
# What, in a line

Crate: apps/foo (olo-foo), nothing outside it.

Why: the person's words if they asked, the problem it solves.

What to change: the files and functions (file:line), the kit APIs to
use (and how other apps use them: point at one), the behaviour wanted,
step by step. The look, exactly: which widgets, which metrics, where;
or "keep the look as it is".

Constraints: words through the words layer (`tr!`, literals the kit
looks up); nothing that blocks the UI or polls; no new dependencies.

Done when: the behaviour; `cargo test -p olo-foo` passes with new tests
for X; `cargo ui-lint` clean if it draws.

Report: anything new on screen described, what to look at closely.
```

## Running them

```sh
tools/dev/agents start NAME CRATE BRIEF.md   # NAME: lowercase, dashes
tools/dev/agents watch                       # in the background, always (below)
tools/dev/agents list                        # each: crate, state, since when
tools/dev/agents peek NAME [LINES]           # its screen
tools/dev/agents say NAME "…"                # answer, steer
tools/dev/agents approve NAME y|p|no         # an approval it asks you for
tools/dev/agents stop NAME                   # end it; its folder stays
tools/dev/agents forget NAME                 # remove its folder
tools/dev/agents attach NAME                 # what the person runs to watch it
```

- **Watch, always.** `agents watch` returns as soon as one needs you: it
  asks for approval, it finished a turn (DONE, or a question), it
  exited, or it showed nothing new for 15 minutes while working (a long
  build can; a loop or a hang shouldn't). Run it with
  `run_in_background` whenever any agent runs; when it returns, deal
  with every agent it names (`peek` first), then start it again. Look at
  `agents list` now and then besides, between your own steps, so nothing
  waits on you unseen.
- **Approvals.** Agents run with `--approve-for-me`: Codex's own
  reviewer approves routine steps (Ben: "the codex agents should run on
  auto approve for most things only the super risky stuff should come
  to you"). What it won't pass comes to you: read the command and the
  reason, and answer `y`, `p` (yes, and for commands that start the same
  from then on) or `no` (then `say` what to do instead). Never approve a
  push, a commit, anything on the person's session, deletion outside its
  crate, sudo, or what its rules forbid.
- **Steer** with `say`: a correction, an answer, "stop and report". It
  reads it at its next step.
- **Stay out of its crate** while it works; your own work goes on
  elsewhere. Each agent builds in its own target folder (its folder's
  `target/`, kache behind it), so its builds and yours never wait on one
  another.

## Review: its work becomes yours

When it says DONE:

1. Read `/var/tmp/olo-agents/NAME/report.md`, then the whole diff
   (`git diff -- CRATE`), and check nothing outside its crate changed
   (`git status`).
2. Read it as code you'd have written: the code's conventions and its
   comments' voice, tests for what it added, HACKING updated, words
   through the words layer, nothing blocking the UI or polling, measured
   where it runs per frame or event (CLAUDE.md, "Fast and light").
3. Build it (`cargo build --release --workspace`), run its tests and the
   lints (`cargo ui-lint`, `tools/dev/ui-lint`, `--tiles`), and picture
   every window and state it changed at 1×, 1.5× and 2×, light and dark,
   judging each closely as a designer. Codex is weak at visual design:
   expect spacing, alignment, hierarchy and words to need your hand.
   Then `codex-look`, as every change's pictures get.
4. Fix what isn't right yourself, or, while it still runs, `say` it back
   with exact instructions.
5. Commit it yourself, saying what the agent did and what you changed;
   then `stop` and `forget` it.

## How it works (codex-cli 0.170)

- One tmux server (`tmux -L olo-agents`), a session per agent, its pane
  kept when Codex ends; Codex's TUI inline (`--no-alt-screen`), so its
  screen and history read as text (`tmux capture-pane`).
- Started with `--approve-for-me` (which brings the workspace-write
  sandbox with it: Codex refuses `-s` beside it), `--disable
  multi_agent --disable multi_agent_v2`, `-C` the repository, `--add-dir`
  its folder; `OLO_SUBAGENT` and `CARGO_TARGET_DIR` set,
  `WAYLAND_DISPLAY` and `OLO_SOCKET` unset (nothing it runs can reach
  the person's screen). `tools/dev/agents`, `codex-help` and
  `codex-look` refuse to run with `OLO_SUBAGENT` set.
- What the watcher reads: "Working (…esc to interrupt)" while it works;
  "Would you like to …" and "Press enter to confirm or esc to cancel"
  when it asks; neither, after "Worked for …", when its turn is over.
- Text typed into it arrives as a paste: an Enter in the same burst is a
  new line, so `say` sends Enter on its own a moment later. A long
  message arrives as several pastes over a second or two, and an Enter
  sent among them is lost (the words sit in its field, the agent idle):
  `say` waits longer for a longer message and sends Enter again while
  the field still holds a paste, and says so if it still does. After a
  long `say`, `peek` to see it took it up.
- Its sandbox lets it write in the repository, `/tmp` and `/var/tmp`;
  the network and other places go through approval.
