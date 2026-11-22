---
name: olo-agents
description: Run subagents on narrow, targeted Olo work items in parallel - each an interactive Antigravity CLI (agy, Gemini 4 Neon at high) in a tmux session of its own, in this working tree, scoped to one crate no other agent is in, running routine commands without asking and asking leave for the rest - and start, watch, steer, approve, review and commit their work with tools/dev/agents. Also how every helper here is run and watched in tmux (the second investigator, Gemini's look at pictures, Opus 5.5's design review). Use when a task splits into independent pieces in separate crates (one app each; a big feature built in parallel; the same kind of change across several apps), at most five at once, and whenever a helper is running and needs reading, answering or steering. Not for finding a bug's cause (olo-debug-help).
---

# Subagents: narrow work, in parallel, each in tmux

CLAUDE.md, "Subagents": besides the debug helper and the looks at
pictures, narrow, targeted work items that can go on in parallel may be
handed to subagents (Ben, 2026-11-19). Each is an interactive
Antigravity CLI (`agy`: Gemini 4 Neon, reasoning at high; Ben,
2026-11-22: "stop using codex/astra for olo qa/dev tasks and … replace
it with antigravity (`agy`) and gemini 4 neon on xhigh", then "switch
the reasoning level to high") in a tmux
session of its own, so its work can be read as it goes, its questions
answered and its course steered. All work in this same working tree,
each in one crate that no other agent, nor you, changes meanwhile: that
is what keeps them from clobbering each other. You own the result: the
design, every visual decision, the review, the commit.

Every helper is run this way, never as a script that answers only at
its end (Ben, 2026-11-22: "always do it through tmux never a script
that just goes silent … for all the subagents/reviewers etc"): the
second investigator (`tools/dev/agy-help`, the `olo-debug-help` skill),
Gemini's look at a change's pictures (`tools/dev/agy-look`) and Opus
5.5's design review of them (`tools/dev/opus-look`) are front doors to
the same `tools/dev/agents`, and everything under "Running them" below
(peek, say, approve, the watching) holds for them too.

## The minder

Start a minder with the helpers, so what they ask is answered in
seconds and not when you next look up (Ben, 2026-11-22: "launch a fable
5.2 medium subagent whose only job is to launch and manage the tmux
sessions for these tmux subagents so that they get their responses
quickly"). It is one subagent of the Agent tool, the only use of that
tool here:

```
Agent(subagent_type: "general-purpose", model: "fable", effort: "medium",
      description: "Mind the tmux agents",
      prompt: "You are the minder … read REPO/.claude/skills/olo-agents/minder.md
               first, in full, and follow it exactly. The repository: REPO.
               Start these, each exactly as written: … (or: start nothing,
               these are running: …). For each: what it is, its brief, its
               scratch folder, where its result will be, what it should and
               shouldn't need.")
```

Write the briefs yourself first (they are the work's hand-over, and
yours); give the minder the commands, whole paths, and for each agent
what it should need (so a request outside that stands out). `minder.md`
has its lists: what it says yes to (reading; cargo's build, check, test
and `fmt -p`; Olo's test tools; files in the agent's own scratch folder
or crate), what it refuses and explains (git that changes anything,
sudo, the person's session, files elsewhere, processes by name, the
network, more agents), and that everything else, every question about
the work, and anything stuck or exited comes back to you. It ends when
every agent has its result or something is yours to decide; its report
names each result's path and every answer it gave. Read the results
yourself, answer what it handed back (`approve`, `say`), and start a
minder again if agents still run. Stop and forget the agents yourself
once their results are read.

Without a minder (one quick look, say), the waiting is yours: below.

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
session, tsession named `agy-NAME`, scratch in its folder, never a
process looked up by its name, nothing of the person's sent outside,
what goes ahead without asking, ask when unsure, `report.md` then
DONE). Write the task itself, specific enough that it needn't guess:

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

Nothing private goes in a brief (it leaves this computer): not the
person's name, email, accounts, hosts, files, nor what their windows
show.

## Running them

```sh
tools/dev/agents start NAME CRATE BRIEF.md   # NAME: lowercase, dashes
tools/dev/agents watch [--for MIN]           # in the background, always (below)
tools/dev/agents list                        # each: what it's on, its state, since when
                                             #   (working, asking, idle, done, exited, gone)
tools/dev/agents peek NAME [LINES]           # its screen
tools/dev/agents log NAME                    # its whole transcript
tools/dev/agents say NAME [--now] "…"        # answer, steer (--now: at once, mid-step)
tools/dev/agents approve NAME y|p|no         # what it asks leave for
tools/dev/agents answer NAME N               # the Nth choice of another question
tools/dev/agents keys NAME down enter        # keys, where choices have no numbers; esc stops its step
tools/dev/agents stop NAME                   # end it; its folder stays
tools/dev/agents forget NAME                 # remove its folder
tools/dev/agents attach NAME                 # what the person runs to watch it
tools/dev/agents allowed                     # what goes ahead unasked, what is refused
```

- **Watch, always.** `agents watch` returns as soon as one needs you: it
  asks leave, it finished a turn (DONE, or a question), it exited, or
  it showed nothing new for 15 minutes while working (a long build can;
  a loop or a hang shouldn't). Meanwhile it prints a line a minute for
  each agent (its state, and the last line of its screen), so its
  output reads as a running account. Run it with `run_in_background`
  whenever any agent runs; when it returns, deal with every agent it
  names (`peek` first), then start it again. Look at `agents list` now
  and then besides, between your own steps, so nothing waits on you
  unseen. (`agy-help`, `agy-look` and `opus-look` do this waiting
  themselves for the one they started: run those in the background the
  same way, and run the same command again after answering.)
- **What goes ahead, what is refused, what comes to you.** An agent
  runs in agy's accept-edits mode, with lists given to that run alone
  (`agy --settings`; the person's own agy settings are never read for
  them nor written). Its file edits go ahead. So does a command that
  starts as one on the allowed list does (reading and searching,
  `cargo build/check/test/clippy/fmt -p`, git's reading commands,
  `tools/dev/tsession`, `ui-lint`, `cost`, `bench`; several piped or
  joined go ahead when each does), and so does output sent to a file
  inside its own scratch folder. A command on the refused list (any
  git that changes something, `sudo`, another agent, `pkill`,
  `systemctl`) is refused without asking. Everything else stops and
  asks: read the command, and answer `y` (this once), `p` (this, and
  commands that start the same, for the rest of its conversation) or
  `no` (then `say` what to do instead). Never approve a push, a
  commit, anything on the person's session, deletion outside its
  crate, sudo, or what its rules forbid. The tool never takes agy's
  choice that would save a rule in the person's settings.
- **Steer** with `say`: a correction, an answer, "stop and report". It
  is pasted into its field and sent. While the agent works, agy queues
  it for the end of the step it is on, and a long think is one step:
  `say NAME --now "…"` delivers it at once. `keys NAME esc` stops the
  step it is on.
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
3. Build it (`cargo build -p` what it changed), run its tests and the
   lints (`cargo ui-lint`, `tools/dev/ui-lint`, `--tiles`), and picture
   every window and state it changed at 1×, 1.5× and 2×, light and dark,
   judging each closely as a designer. Expect spacing, alignment,
   hierarchy and words to need your hand. Then `agy-look` and
   `opus-look`, as every change's pictures get.
4. Fix what isn't right yourself, or, while it still runs, `say` it back
   with exact instructions.
5. Commit it yourself, saying what the agent did and what you changed;
   then `stop` and `forget` it.

## How it works (agy 1.5.14, Claude Code 2.1.330)

- One tmux server (`tmux -L olo-agents`), a session per agent, its pane
  kept when its program ends; both programs draw inline, so a session's
  screen and history read as text (`tmux capture-pane`). The program's
  banner, which names the account it is signed in to, is left out of
  everything the tool prints.
- An agy agent is `agy --model gemini-4-neon-high --mode accept-edits
  --settings ITS/settings.json --add-dir ITS -i "Read ITS/brief.md …"`,
  started in the repository (a look: in its own folder, on copies of
  the pictures, with no command allowed unasked). The settings hold
  its folders as trusted (agy asks at its start otherwise, and waits)
  and `permissions.allow` / `permissions.deny`, lists of
  `command(PREFIX)`. `OLO_SUBAGENT` and `CARGO_TARGET_DIR` are set,
  `WAYLAND_DISPLAY` and `OLO_SOCKET` unset (nothing it runs can reach
  the person's screen). `tools/dev/agents` and its front doors refuse
  to run with `OLO_SUBAGENT` set.
- Never agy's `auto` mode (what it starts in when no mode is named): it
  judges for itself what is safe, and runs a command outside its
  sandbox when the sandbox refuses it. A `sudo -n true` went through
  unasked when tried (2026-11-22).
- What agy asks about though its command is allowed: output sent to a
  file with `>` (the rules tell an agent to pipe, and to write files
  with its file tool).
- What the watcher reads, agy: its last line begins "esc to cancel"
  while it works and "? for shortcuts" when its turn is over; a
  question has "↑/↓ Navigate" under its numbered choices ("Requesting
  permission for: … Run this command? 1. Yes, run command 2. Yes, and
  always allow in this conversation for commands that start with '…'
  3. … (Persist to settings.json) 4. No, cancel"); a digit takes a
  choice.
- Opus's design review is `claude --model claude-opus-5-5 --effort high
  --restricted --strict-mcp-config --tools Read,Write --permission-mode
  acceptEdits --disable-slash-commands BRIEF`, in its own folder on
  copies of the pictures: no tool that runs anything, the file tools
  kept to that folder, none of the person's settings, hooks or MCP
  servers. The brief goes last, after an option that takes no value
  (`--tools` and `--add-dir` take every word that follows them). Its
  last line holds "esc to interrupt" while it works; a question has
  "Esc to cancel" under its choices.
- Text for an agent is pasted (`tmux paste-buffer -p`), so a new line
  in it is a new line and not a send; Enter goes by itself a moment
  later, and again if the field still holds the words.
