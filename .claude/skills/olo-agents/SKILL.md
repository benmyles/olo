---
name: olo-agents
description: "Run Olo's native Rex subagent hierarchy through `olo agent` and `tools/dev/agents`: Lead -> Minder (agy Gemini 4 Neon high) -> Builders (agy Gemini 4 Neon xhigh), Second Investigator (agy Neon 4 xhigh), Code Reviewers (Codex Astra 6.2 xhigh + agy Gemini 4 Neon xhigh, max 2 rounds each), and Visual Design Reviewer (Claude Opus 5.5 xhigh alongside agy-look and code reviewers whenever visual design work is involved). Use for parallel crate work, managing the minder, running code/visual reviews, and handling review escalations."
---

# Subagents: Lead ▸ Minder (`agy` Neon 4 `high`) ▸ Builders & Reviewers

Every helper runs as a native Rex child tab through `olo agent` (never the harness Agent tool, never headless print scripts; tmux only where there is no Rex pane).

## Hierarchy and Exact Models / Efforts

```text
Lead (autonomous coordinator)
└── minder  (ALWAYS agy, Gemini 4 Neon at high: gemini-4-neon-high)
    ├── builders       (agy, Gemini 4 Neon at xhigh: gemini-4-neon-xhigh)
    ├── investigator   (agy, Gemini 4 Neon at xhigh: tools/dev/agy-help)
    ├── code review 1  (codex, Astra 6.2 at xhigh: tools/dev/astra-review, max 2 rounds)
    ├── code review 2  (agy, Gemini 4 Neon at xhigh: tools/dev/agy-review, max 2 rounds)
    └── visual design  (changes with visual design work use ALL THREE reviewers:
                        agy-look [Neon 4 xhigh] + opus-look [Opus 5.5 xhigh]
                        + astra-review [Astra 6.2 xhigh] & agy-review [Neon 4 xhigh])
```

- **Minders:** ALWAYS Antigravity (`agy`) **Gemini 4 Neon on `high`** (`gemini-4-neon-high`, `--effort high`). Started directly under the lead with `tools/dev/agents minder start [BRIEF.md]`. All builders, investigators, and reviewers run as child tabs under the minder.
- **Builders:** ALWAYS Antigravity (`agy`) **Gemini 4 Neon on `xhigh`** (`gemini-4-neon-xhigh`, `--effort xhigh`). Up to five at once per checkout, each scoped to its own crate(s) so none clobbers another. Builders get `OLO_SUBAGENT=<name>` and cannot spawn subagents of their own.
- **Code Reviews (non-visual and visual changes, max 2 rounds each):**
  1. **Codex Astra 6.2 on `xhigh`** (`tools/dev/astra-review [--about "..."] [--tree DIR] [--no-wait] --working [PATH…] | FROM..TO [PATH…]`)
  2. **Antigravity Gemini 4 Neon on `xhigh`** (`tools/dev/agy-review [--about "..."] [--tree DIR] [--no-wait] --working [PATH…] | FROM..TO [PATH…]`)
  Non-visual changes use these two reviewers only.
- **Visual Design Reviews (any change with visual design work — all three reviewers):**
  1. **Antigravity Gemini 4 Neon on `xhigh`**: `tools/dev/agy-look` (pixel-level screenshot review) + `tools/dev/agy-review` (code review)
  2. **Codex Astra 6.2 on `xhigh`**: `tools/dev/astra-review` (code review)
  3. **Claude Opus 5.5 on `xhigh`**: `tools/dev/opus-look` (product/visual design review of screenshots)
- **Escalation after 2 rounds:** Each builder gets at most 2 review rounds per reviewer by default. If critical outstanding issues remain after 2 rounds, the minder escalates to the lead (`olo agent escalate "..."`) so the lead can decide whether the builder misaligned, whether the design needs rethinking, how to steer it, and whether to authorize additional review rounds.

## Monitoring the Minder & Active Watches (Mandatory)

- **Lead (monitor the minder for approvals AND stalls):** Keep a background `olo agent watch <minder-id> --since CURSOR --on approval,question,escalation,idle,done,error,exit --timeout 90 --json` running from the moment the minder starts until it stops, restarting it immediately after every return (advancing `--since` on exit `0`, reusing `CURSOR` on exit `124`).
  - If the minder hits `waiting_approval`, inspect and answer it immediately so the person never has to approve the minder.
  - On watch timeout (`124`) or if the minder sits `idle`/`done` while children are still running, inspect `olo agent status <minder-id> --json` and `olo agent read <minder-id> --lines 40` to verify the minder's own background `olo agent watch` is alive; if the minder stalled, dropped its watch, or wedged, steer or restart it immediately.
- **Minder:** keeps an active watch (`olo agent watch --all --since CURSOR --on question,approval,escalation,idle,done,error,exit --timeout 90 --json`) on all subagents nested under it at all times.
- Never run children (or a minder) without an active watch, and never double-watch the same child.

## Commands (`tools/dev/agents`)

```sh
# Lead: start and talk to the minder (agy Neon 4 high)
tools/dev/agents minder start [BRIEF.md] [--name minder]
tools/dev/agents minder peek|log|say|stop

# Minder: start / resume / steer / approve / stop builders (agy Neon 4 xhigh)
tools/dev/agents start [--tree DIR] NAME CRATE BRIEF.md
tools/dev/agents resume NAME [--result FILE] [--session ID] "TEXT…"
tools/dev/agents list
tools/dev/agents watch [--for MIN] [--only NAME[,NAME…]]
tools/dev/agents peek NAME [LINES]
tools/dev/agents log NAME
tools/dev/agents say NAME [--now] "…"
tools/dev/agents approve NAME y|p|no
tools/dev/agents answer NAME N
tools/dev/agents keys NAME KEY…
tools/dev/agents stop NAME
tools/dev/agents forget NAME

# Code reviews (max 2 rounds each)
tools/dev/astra-review [--about "…"] [--tree DIR] [--no-wait] --working [PATH…]
tools/dev/agy-review   [--about "…"] [--tree DIR] [--no-wait] --working [PATH…]
tools/dev/astra-review [--about "…"] [--tree DIR] [--no-wait] FROM..TO [PATH…]
tools/dev/agy-review   [--about "…"] [--tree DIR] [--no-wait] FROM..TO [PATH…]

# Visual design reviews (visual changes only; run alongside code reviews)
tools/dev/agy-look  [--about "…"] [--no-wait] PICTURE.png…
tools/dev/opus-look [--about "…"] [--no-wait] PICTURE.png…

# Collect a finished --no-wait review or look
tools/dev/agents collect NAME

# Subagent right-pane standup board (pre-allowed for minders, builders & investigators)
tools/dev/agent-board init --open --title NAME --role Builder --crate CRATE --goal "…" --now "…" --todo "…"
tools/dev/agent-board update --did "…" --now "…" --check-todo "…" --start-todo "…" --status testing
```

## Multi-Lead Isolation

`/var/tmp/olo-agents` may hold helpers from sibling checkouts (such as `olo.webv2`). `tools/dev/agents` records `root` and checks `olo agent list --all --fast --json` so `list`, `room`, `watch`, `tidy`, and `sweep_builds` operate strictly on this tab's subtree and checkout — never counting, modifying, or stopping another lead's helpers.

## Lead Final Verification and Landing

When the minder escalates that a builder's work has passed review and is ready:
1. Read the builder's report, the reviews under `/var/tmp/olo-reviews` (and screenshot reviews if visual), and inspect the diff (`git diff -- CRATE`).
2. Run the crate's tests and lints yourself (`jato test -p …`, `jato clippy`, and for visual changes `tsession review` / `ui-lint --tiles` and visually inspect the screenshots).
3. Commit cleanly (staging by path, never `-A`; recording what each reviewer found and what was done; checking no machine hostname or account leaks into the diff or message).
4. Have the minder stop (and once committed, `forget`) any finished builders no longer needed.
