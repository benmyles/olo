---
name: olo-rex-bridge
description: From a Rex session on this host: start, watch, answer and stop subagents as child tabs of your own tab (`olo agent`: Claude Code, Codex, Antigravity or a command of your own; it needs no desktop, and is the way to run any subagent, reviewer or helper here), and use the person's Olo desktop - open a link or a local server's page in their browser, reach a server of this host's there at the port it answers on here, read a secret from their Olo Passwords, and see and drive graphical apps on a screen of your own (Olo Agent Screens: on their desktop, or on this host itself with no desktop asked, Chrome included). Use when a task on this host needs a browser, a GUI, a password or the person's eyes on something.
---

# The Olo desktop, from this host

This host is used through **Rex** from a computer running the **Olo**
desktop. One command here asks that desktop for things, and answers as
if it had run here:

    ~/.claude/skills/olo-rex-bridge/olo COMMAND [ARGUMENT…]

It works in a Rex session that is open in Olo's Rex, and only for what
the person has allowed this host (Olo's Preferences ▸ Rex): a command
that isn't allowed fails and says so (status 69, as when no Olo desktop
is connected: then this host isn't open in Olo's Rex now, so say so).

**`olo agent` is the exception: it needs no desktop.** The children of
your tab are this host's own (below): started, read, answered and
stopped by this host, whether or not an Olo desktop is connected.

## Subagents in Rex (`olo agent`) — PREFERRED

**Always use `olo agent` to launch subagents when running inside Rex.**
Because you are in an Olo Rex environment, the user strongly prefers
this native Rex child-tab mechanism over any other subagent, background
shell, or tmux mechanism: each subagent gets a real interactive Rex
terminal tab nested cleanly under your current tab (visible in the tab's
tree popover and the Activity sidebar, without cluttering the top tab
bar, and allowing the person to click straight in and interact at any
time).

**While any child of yours runs, keep a watch going on them.** One
`olo agent watch --all --since CURSOR --on question,approval,escalation,idle,done,error,exit --timeout 90 --json`
as a background command of your harness (so its return wakes you),
from the moment you start the first child until the last has ended:

- it returns at once when a child asks leave, asks a question, comes
  to rest, fails or exits (exit code 0: read the event, act on it with
  `status`, `read`, `approve` or `send`, then watch again from the
  `cursor` it gave); with nothing to say it returns 124 when its time is
  up: watch again from the same cursor. Nothing is polled meanwhile.
- `CURSOR` is the `cursor` of your `start` (or of `list`): from there,
  nothing that happened while you looked away is missed, and nothing is
  said twice.
- Never start children and go on with other work without one: a child
  that asks waits on you unseen, for as long as you don't look. Never
  wait with `sleep` and `read` in a loop instead.
- Where a tool of the project starts the children for you (Olo's
  `tools/dev/agents` and its front doors), its own `watch` is this watch
  (it waits on `olo agent watch`): keep that one going instead.

**Give a child a minder when you will be busy** (`--minder`). A child
that asks leave or asks a question waits for as long as you are in the
middle of something else. A minder is an agent of its own whose whole
job is that one child: it waits on it, answers what it asks at once by
rules you write, steers it, and tells you what your rules don't cover.
It is a child tab of the child it minds (the person sees it there), and
you choose its model and effort: a stronger one than the child's where
its judgment matters.

    olo agent start agy --model gemini-4-neon --effort xhigh --name ui-check \
        --prompt-file brief.md \
        --minder claude:claude-fable-5-2:high --rules ui-check-rules.md --json

- **The rules are your words**, a file (`--rules FILE`) or a string
  (`--rules-text`): what to approve (name the commands or kinds: "cargo
  build and test, read-only git, reading files under the repository"),
  what to refuse and what to tell the child instead, how to steer it
  ("when it rests without having run the tests, tell it to run them";
  "when it has finished, ask for its summary in /var/tmp/x/answer.md"),
  and what you want to hear of. Give it the context it needs to judge:
  what the child was asked, what it may touch. Where the rules are
  silent a minder does not guess: it escalates, the question word for
  word, and leaves the child waiting for you.
- **What changes for your watch.** A minded child's `approval` and
  `question` no longer wake it: they are its minder's. `idle`, `done`,
  `error` and `exit` still do, and so does a new event,
  **`escalation`**: what the minder has for you (`"event":
  "escalation"`, `name` the child it is about, `summary` the minder's
  words). It is among the events a watch waits for by default; if you
  name them yourself, add it:
  `olo agent watch --all --since CURSOR --on escalation,idle,done,error,exit --timeout 90 --json`.
  You can still look at the child (`status`, `read`) and answer it
  yourself (`approve`, `send`) whenever you like.
- **For a child that runs already**, or to change what one has:
  `olo agent mind NAME --with claude:MODEL:EFFORT --rules FILE` attaches
  a minder (or puts another in its place); `olo agent mind NAME --rules
  FILE` gives the one it has new rules (the same minder, begun anew
  with them); `olo agent mind NAME --off` ends it, and the child's
  asking is yours again; `olo agent mind NAME [--json]` says who minds
  it, by what rules, and everything the minder answered (`answered`:
  each `approve`, `send` and `escalate`, with its time), to check its
  work.
- **What a minder can do is held by the service, not by its good
  sense.** It reaches the one child it minds and nothing else: it can
  watch it, read it, say yes once or no (never a yes that is kept), type
  to it, and escalate. It cannot start, stop or list anything, reach
  another child, or run any other program: the shell is its only tool
  and its one command the only thing allowed in it. It is told what
  stands, not each thing that passed while it thought, and its yes or
  no is for the question it was last told of: one that took a while
  never answers a question that came since. (A minder at a high effort
  answers a plain yes or no in some 3 to 7 seconds.)
- **How it ends.** When the child exits, its minder tells you once what
  it approved, refused and passed up (an `escalation`), and is stopped.
  `stop --tree` on the child stops both. If the minder itself dies
  while the child runs, you are told (an `escalation` saying so) and the
  child's asking comes to your watch again.
- A minder runs Claude Code (`claude:MODEL:EFFORT`) for now. It has the
  child's `--env`; `--minder-env K=V` gives it more (`mind … --env K=V`
  for one attached later).
- **A child can tell its parent something itself**:
  `olo agent escalate "TEXT"`, from inside any child's tab, is an
  `escalation` on the parent's watch at once. Say so in a child's brief
  where you want to hear of something without it stopping to ask.

**It works with no Olo desktop connected.** Every `olo agent` command
is answered by this host's own agents service (Olo's helper here, which
`olo` runs: `olo-rex-link agent`), through this host's Rex server. The
desktop is not asked and need not be there: children run, are watched,
answered and stopped while the person's computer sleeps or is away, and
the moment a desktop connects it shows every one of them, under the tab
that started it. The service starts by itself at the first command and
ends when nothing is left for it to do; if it is ever killed, the next
command starts another that knows every child (and `watch --since`
goes on from the same cursor). The same through an upgrade, when a
later build of the helper takes the service over: `list` has every live
child the moment after, with its name, its state and its conversation's
id, so nothing of yours needs a reading of a child's process to know it
is still there. After a restart of this host's Rex
server, `list --all` has each child as exited with its conversation's
id, to take up again with `start --resume`.

First-class CLIs: **`claude`**, **`codex`**, and **`agy`**. Both
**`--model`** and **`--effort`** are **ALWAYS REQUIRED** (even when
resuming a previous session) so there are never surprises about which
model or reasoning effort is used.

- **A command of your own in a child tab** (not an agent: a stream
  pipeline, a dev server, a test run): `olo agent start --command
  [--name NAME] [--title TITLE] [--cwd DIR] [--env K=V]… [--close-on-exit]
  [--json] -- CMD [ARG…]`. No `--model` or `--effort`. It is `working`
  while it runs and `exited` after (`status`: `running`, `exit_code`);
  `watch NAME --on exit` returns when it ends (a status other than 0 is
  its `error`); `read`, `send` and `stop` as for an agent. Nothing an
  agent's screen would mean is read into what it prints.

- **Launch a new or resumed subagent tab** (`olo agent start`):
  - New session:
    `olo agent start claude --model claude-opus-5-5 --effort max --name api-audit --title "API Audit" --trust --permission-mode acceptEdits --prompt "Review crates/api" --json`
    `olo agent start codex --model gpt-5.4-codex --effort high --name tests --title "Tests" --approval on-request --sandbox workspace-write --prompt "Add unit tests" --json`
    `olo agent start agy --model gemini-4-neon --effort xhigh --name ui-check --title "UI Check" --mode accept-edits --prompt "Inspect layout" --json`
  - Resume or fork an existing session (`--model` and `--effort` still required):
    `olo agent start claude --model claude-opus-5-5 --effort max --resume SESSION_ID` (or `--continue`, `--fork`, `--session-id UUID`)
    `olo agent start codex --model gpt-5.4-codex --effort high --resume SESSION_ID` (or `--continue`/`--last`, `--fork`)
    `olo agent start agy --model gemini-4-neon --effort xhigh --resume SESSION_ID` (or `--continue`, `--stream`)
  - Isolation & trust:
    - `--desktop none|screens=NAME,…|inherit` (default **`none`** for all children: server-side strict allow-list blocks `op`, `olo screen`, `olo open`, clipboard, catch-all actions, and privilege escalation by grandchildren; strips `DISPLAY`, `WAYLAND_DISPLAY`, `OLO_SOCKET`, `OLO_SESSION`, `REX_SESSION`, `SSH_AUTH_SOCK`, and `DBUS_SESSION_BUS_ADDRESS`; children may only act on their own strict descendants. Note: on the person's local desktop account, `--desktop none` is an environment/bridge guardrail, not a kernel sandbox. Use `--desktop screens=qa-1` for screen-only QA walkers).
    - `--trust`: for Claude Code (and Codex), atomically pre-trusts `--cwd` and every `--add-dir` in `~/.claude.json` (honouring `CLAUDE_CONFIG_DIR`) and auto-confirms matching startup folder-trust dialogs.
    - `--unset-env KEY` (repeatable): unsets additional environment variables before launch.
    - `--approve-for-me`: **Codex-only** (off by default; never enabled automatically; rejected on `claude` and `agy`). Passes Codex's own `--approve-for-me` flag to route Codex approvals through Codex's automated review.
  - Full CLI options supported: `--name` (unique among live children under your parent; target as `NAME` or `PARENT/NAME`), `--title`, `--role`, `--cwd`, `--prompt` / `--prompt-file PATH` (for prompts > 128 KiB, pass a file path in the prompt or paste via `send --file` after launch), `--env K=V`, `--unset-env K`, `--parent WINDOW_ID`, `--focus`, `--close-on-exit`, `--permission-mode`, `--approval`/`-a`, `--sandbox`/`-s`, `--mode`, `--full-auto`, `--approve-for-me` (Codex-only), `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--allowed-tools`, `--disallowed-tools` (variadic), `--add-dir`, `--worktree` / `--worktree=NAME`, `--agent`, `--session-id UUID` (Claude-only), `--profile`, `--config/-c K=V`, `--settings JSON_OR_FILE`, `--strict-mcp-config`, `--system-prompt`, `--append-system-prompt`, `--mcp-config`, `--project`, `--new-project`, `--remote-agent`, `--remote-control`, `--search`, `--enable`, `--disable`, `--image`, `--no-alt-screen`, `--no-daemon`, `--print`, `--stream`, `--input-format`, `--output-format`, plus `-- EXTRA_CLI_ARGS…` verbatim passthrough.
- **List your subagent tree**: `olo agent list [--all] [--parent WIN|NAME] [--fast] [--json]` (with `--all`, includes exited children and their persisted `cli_session_id` even after a Rex server restart; returns monotonic event `cursor`; timestamps are Unix milliseconds).
- **Inspect rich live status**: `olo agent status <ID|NAME> [--json]` (returns `state`, `running`, `cli_session_id`, `resume_id`, `transcript_path`, `pid`, `exit_code`, `aborted`, `survey` [a feedback survey that wouldn't be set aside is drawn over its screen], full verbatim `last_message`, structured `approval` with screen-verified command & typed choices, `error`, `started_at`, `exited_at`, `last_output_at`, `last_state_change_at` [all Unix ms], `quiet_secs`, `state_age_secs`, `background_shells`, `waiting_on`, `usage`, `cursor`, and recent `tail`).
  Two things to know of it. **`state` is the agent's turn, not its
  pane**: `working` means the model is in a turn. A turn that ended is
  `done` (or `waiting_question`, `idle`) and is told to `watch` at once,
  whatever the agent left running; a shell of its own still going (a dev
  server, a test run) is said beside it, in `waiting_on` (`own_run`) and
  `background_shells`, and whether that will wake the agent is yours to
  judge from its command. **`pid` is the `env` wrapper's for a moment
  after `start`** (a child is started as `env K=V … PROGRAM`), then the
  program's own, the same number: its environment read in that moment
  is `env`'s, without the variables. Read it once `status` shows the
  agent's screen.
- **Read terminal output / scrollback**: `olo agent read <ID|NAME> [--lines 80] [--scrollback] [--ansi] [--json]` (note: `--scrollback` still caps output to `--lines`, 80 by default; pass `--lines 2000` for deeper history).
- **Event-driven watch (zero polling, multi-agent & cursor-safe)**:
  `olo agent watch [<ID|NAME>… | --all] [--since CURSOR] [--parent WIN] [--on question,approval,escalation,idle,done,error,exit] [--match TEXT] [--regex PAT] [--timeout 90] [--json]`
  Always pass `--since CURSOR` (from `start`, `list`, `send`, `approve`, or the prior `watch`) and keep `--timeout` under your shell tool timeout (e.g. `90` seconds), looping on exit code **124** (`{"ok":false,"event":"timeout","cursor":...,"code":124}`). Exit codes: `0` event, `124` timeout, `2` usage/validation error, `69` it can't be done from here (no Rex server on this host; or a child asking what isn't its to ask), `75` the service ended while it did something that must not be done twice (`start`, `send`, `approve`, `stop`: look with `list` before asking again; `watch`, `list`, `status` and `read` simply ask the next service). `--on idle` matches both `idle` and `done`; `--on done` matches strictly `done`. If the event cursor resets (e.g. after a fresh start), `watch` returns `{"ok":true,"event":"reset","cursor":...}` immediately; if events were evicted from the 2000-event ring buffer, `gap:true` is set.
- **Approve or deny a pending tool approval safely**:
  `olo agent approve <ID|NAME> --once | --session | --no [--reason TEXT] [--json]`
  Picks the one choice of that kind by what it means, wherever it stands in the list (`approval.choices[].kind`: `once`, `allow_session`, `dont_ask_again`, `switch_mode`, `no`, `no_always`, `other`): `--once` the yes for this once ("Yes", "Yes, run command", "Yes, but ask again next time"), `--session` the yes kept for this conversation alone (Antigravity's "Yes, and always allow in this conversation"), `--no` the no for this once (never "No, and block reads"). Two of the kind asked for, or none, is refused with the choices named.
  Never selects persistent (`don't ask again` / `always allow` / kept in settings) or mode-switching options; re-verifies cursor selection before pressing Enter, confirms the specific dialog key dismissed without touching any next dialog, and pastes sanitized `--reason` only after dismissal.
- **Send input reliably** (text, file paste, control chars, special keys, delivery verification):
  `olo agent send <ID|NAME> "Please check tests/api.rs next" --enter --json`
  `olo agent send <ID|NAME> --file /var/tmp/prompt.md --enter --json`
  `olo agent send <ID|NAME> --ctrl c` (or `--ctrl d`, `--key Escape`, `--key Tab`, `--key Up`, `--key Down`, `--paste`)
  `--key` takes any key with any modifier a terminal can send, joined by `+` or `-`: `--key Alt+m` (what tmux sends for `M-m`), `--key Ctrl+Shift+Up`, `--key Alt+Enter`, `--key Shift+Tab`, `--key F5`, `--key C-M-c`; several `--key`s go one after another.
  A key that plain bytes can't say is pressed as the keyboard would press it, by what the agent asked of its keys: `--key Ctrl+Enter` and `--key Shift+Enter` (a new line in Claude Code's field, not a send), `--key Ctrl+Tab`, `--key Shift+Backspace`.
  A single character is a key too: `--key 0`, `--key y` press that one key and nothing else (no Return, no paste), for a box that takes one key and must be given no text (Claude Code's "1 to review · 2 to send · 0 to dismiss").
  Refuses to type text, or to press a character key, while an approval/question dialog is active: there a character is an answer by its number (use `olo agent approve` for approvals, or pass `--force`). Always strips bracketed-paste escapes and returns a pre-write `cursor`.
- **A minder for a child, and a word to the parent** (above):
  `olo agent start … --minder claude:MODEL:EFFORT [--rules FILE | --rules-text TEXT] [--minder-env K=V]…`
  `olo agent mind <ID|NAME> [--with claude:MODEL:EFFORT] [--rules FILE | --rules-text TEXT] [--env K=V]… [--off] [--json]`
  `olo agent escalate "TEXT" [--json]` (from a child's own tab)
- **Stop & archive launched tabs cleanly**:
  `olo agent stop <ID|NAME> [--save PATH] [--force] [--tree] [--keep-tab] [--json]`
  Gracefully interrupts (double Ctrl-C) and closes the child tab (`--save PATH` archives full scrollback to `PATH` before closing; `--tree` stops all nested descendants bottom-up; `--force` closes immediately; `--force` and `--keep-tab` cannot be combined; keeps `cli_session_id` in registry for `list --all` and `--resume`).
- **Prune exited child records**: `olo agent prune [--all] [--json]`
- **Focus a child tab in Rex**: `olo agent focus <ID|NAME>`
- **Feedback surveys are never yours to answer.** Claude Code's ("How is
  Claude doing this session? … 0: Dismiss") and Antigravity's ("How's
  the CLI experience so far? … [0] Skip") are set aside by the service
  itself the moment it sees one (it once hid a permission prompt for 25
  minutes), and what lies under it is what `status` and `watch` say.
  One that wouldn't go is said (`status`: `"survey": true`).

## Desktop integration commands

- **A link in the person's browser**: `olo open https://example.com`.
  A `http://localhost:PORT` of this host is reached through a forward
  Rex makes for it, where that is allowed (at a free port there, unless
  `olo forward` made one first). **A file of this host's**,
  by its whole path (`olo open /home/me/report.pdf`): copied to the
  desktop and opened in its app there (a picture in Olo's Viewer).
- **A server of this host's, reached from the desktop**: `olo forward
  5173` makes the desktop's localhost:5173 reach this host's
  localhost:5173 (also `olo forward HOST:PORT`, or a URL, which it
  prints at its address there). Make one before anything whose page
  must be at the same address there as here: a sign-in link, a cookie,
  the address an OAuth provider sends the browser back to. It serves
  the person's browser and your Agent Screens alike, and `olo open` and
  `olo screen` go through it. `--port N` for another port there,
  `--any-port` for a free one. `olo forward ls` lists this host's
  forwards there (`--json`), `olo forward close PORT` closes one made
  from here (the person's own are theirs). It lasts until closed or
  this Rex session ends; a port taken there is said, never quietly
  changed. (Rex's own `rex forward new`, `open` and `share`, run here,
  listen on this host, where `rex` runs, not on the desktop: the
  desktop's side is `olo forward`'s.)
- **Local desktop files (`rex://local/…`)**: When the person pastes a file
  or folder path from their Olo desktop into a remote Rex terminal, Rex
  rewrites it as a `rex://local/ABS/PATH` URI (for example
  `rex://local/home/user/Pictures/screenshot.png`). Those files live on the
  person's desktop, NOT on this host — fetch or inspect them with:
  - `olo get rex://local/PATH [DEST]` (alias `olo fetch`): downloads the
    desktop file onto this host (into `DEST` if given, or
    `/var/tmp/olo-rex-bridge/<filename>` by default, up to 32 MiB) and
    prints the saved local path on this host so you can read, view, or
    process it directly (`--json` for structured output). If given a folder,
    lists its entries and shows how to fetch files inside it.
  - `olo ls rex://local/PATH [--all] [--json]`: lists a folder on the
    person's desktop (returns `rex://local/…` URIs for each entry).
  - `olo open rex://local/PATH` (or `olo open rex://HOST/PATH`): opens or
    reveals that folder or file in Tracker on the person's desktop.
  Requires the host's file sharing permission in Preferences ▸ Rex (the same
  leave as `olo send` / `olo sync`). For files larger than 32 MiB or whole
  trees kept in step, use `olo sync --to-host`.
- **Files and folders of this host, saved on the desktop**: `olo send
  dist/app.tar.zst build/` puts each in the person's Downloads ▸ Rex ▸
  HOST (a folder as a folder of its name), where the person may look at
  or use it; it says where each went and how much was sent. Sending the
  same again sends only what changed (a rebuilt archive's changed
  parts); `--mirror` also takes away, within that folder there, what
  is no longer here. Where the person hasn't let this host send files
  (Preferences ▸ Rex), it says so: ask them.
- **A folder kept in step with the desktop**: `olo sync dist` keeps this
  host's folder in step in Downloads ▸ Rex ▸ HOST ▸ dist from then on
  (what changes here is brought there moments later, by itself: a
  build's output, logs). Direction flags: `--from-host` (default, host
  to desktop), `--to-host` (desktop to host), or `--two-way` (alias
  `--both`: edits, adds and deletes flow both ways, and when both sides
  edit the same file the newer copy stays in place while the other is
  preserved beside it as `name.conflict-SIDE-YYYYMMDD-HHMMSS.ext` on
  both sides so nothing is lost). Options: `--mirror` (in one-way sync,
  remove extras on the receiving side), `--shallow` (top-level files
  only), `--exclude PATTERN` (repeatable glob, e.g. `--exclude target
  --exclude '*.tmp'`). Other subcommands: `olo sync wait dist` brings it
  in step now and answers when it is (after a build: `make && olo sync
  wait dist`); `olo sync ls [--json]` lists this host's kept folders,
  directions, states and conflict counts; `olo sync conflicts [PATH]
  [--json]` lists any conflict copies to resolve; `olo sync stop dist`
  lets it go (files stay on both sides). The same leave as `olo send`.
- **A secret from Olo Passwords** (1Password's command line, answered
  on the desktop after the person approves there; no secret is kept on
  this host): `olo op read op://Vault/Item/field`. Changes too, as
  1Password's `op` makes them, each approved by the person:
  `olo op item create --category login --title NAME --vault VAULT
  username=… 'password=…'` (or a template: `--template FILE`, or `-` and
  JSON on standard input), `olo op item edit ITEM field=value`,
  `olo op item delete ITEM`. Files its flags name (`--template`, `-i`,
  `-o`) are this host's; `olo op run` isn't done (its command would run
  on the desktop): read the values with `olo op read` or `olo op inject`
  and run the command here.
- **A screen of your own on the desktop** (Olo Agent Screens: a
  headless desktop the person can watch), with `olo-screen`'s own
  command line (`olo-screen` itself works here too, with the desktop
  commands installed, as the olo-screens skill writes it):
  - `olo screen start NAME`
  - `olo screen run NAME olo-chrome https://example.com` (a web page)
  - `olo screen tree NAME`: what is on it, as text. Read this first,
    and for anything word for word: it has every paragraph whole.
  - `olo screen click NAME --role Button --name OK`, `type`, `key`,
    `wait`. Each answers with the screen as it is after it, in brief
    (text cut short): `tree` has the whole.
  - `olo screen look NAME [PATH]`: a picture, saved on this host, at
    PATH where you give one (beside your report), else under
    `/var/tmp/olo-rex-bridge`; the answer names the file here (`--json`:
    `{"path": …}`).
  - `olo screen stop NAME` when done; `olo screen --help` for the rest.
  In `olo screen run` (and `start`), a `http://localhost:PORT` of this
  host among the program's arguments (a browser's address) is reached
  through a forward, as for `olo open`; what you `type` or `set` into a
  page is sent as you wrote it, addresses too.
- **A screen of your own on this host itself**: the same Agent Screens
  with no desktop asked or needed: a headless Olo desktop of this
  host's own, drawn here (in software where the host has no GPU), with
  Deskbar, Tracker, Terminal, every Olo app and Chrome. The same
  commands, after `--on host`:
  - `olo screen status`: whether Olo is on this host (`installed`), what
    its programs lack here (`missing`: libraries to have installed;
    `older`: libraries too old for it, which only a newer system cures;
    `software_drawing`), Chrome's version, whether the host has a font
    with emoji (`emoji_font`: without one every emoji is a box, on a
    page too; `olo screen install` puts one), and where `olo screen`
    goes when it isn't said (`default`).
  - `olo screen install`: puts Olo here, under the account and nowhere
    else (`~/.local/share/olo/desktop`: about 600 MB downloaded from
    Olo's releases and checked, 1.9 GB on disk; nothing needs root),
    and Chrome (Google's own, unpacked under `~/.local/share/olo/chrome`),
    and, where the host has no font with emoji, Noto Color Emoji among
    the account's fonts (so a 🚀 is one, in Olo's windows and on a
    page, not a box). A line of JSON for each step, then what `status`
    says. Run again later, it brings Olo and Chrome up to the newest
    (and fetches nothing where the newest is here). It leaves Olo as
    it is while screens run here, yours or another agent's (each would
    reload a moment after its programs changed): it says which
    (`held`), and is run again once they are stopped.
  - `olo screen --on host start NAME`, then `run NAME olo-chrome
    http://localhost:5173` (a server of this host's needs no forward: it
    is the same machine), `tree NAME`, `click`, `type`, `key`, `wait`,
    `look NAME [PATH]`, `stop NAME`: as above.
  - `olo screen default host` makes the host's the ones `olo screen …`
    (and `olo-screen …`) means here when it isn't said, `--on desktop`
    the other; `OLO_SCREENS_ON=host` does it for one shell and what it
    starts. A child tab isn't started by your shell (the Rex server
    starts it), so say it as you start one: `olo agent start … --env
    OLO_SCREENS_ON=host`. A child needs no `--desktop` leave for the
    host's screens: they are the host's, as any program here is.
    `olo screen remove` takes Olo off the host again.
  - A screen at rest takes no processor time and about 250 MB of memory,
    more with what runs on it (Chrome most): stop it when you are done.
    One started from a Rex tab is stopped by itself when that tab's
    program ends (you, or the shell: what ran on it ends, the screen is
    kept as a stopped one and starts again as it was); `start NAME
    --keep` for one that must outlive the tab that started it (another
    agent's to go on with), which then stays until it is stopped.
    The person has all of this in Rex too (the Host window's Screens
    tab: Install, Update, Remove, what the host lacks, which screens
    `olo screen` means here by default, and a list of this host's
    screens, where they can look at one of yours as it is now, a
    picture, and stop it: give a screen a title that says what it is
    for, `olo screen --on host rename NAME "TITLE"`, since that list
    shows it).
    The person can't watch a host's screen live from their desktop yet,
    nor take it over: show
    what you did with a picture (`look`), or all of it as a video of
    its actions with the waits between them cut (`olo screen --on host
    history NAME --export OUT.mp4`, made in under a second with no GPU),
    which `olo send OUT.mp4` saves on their desktop.
  Use the host's when the work is the host's own (a server here, a page
  to test, a program to drive) or the desktop is away; the desktop's
  when the person should watch or take over, or the task needs what is
  theirs (their sign-ins, their Passwords).
- Also `olo notify TITLE [BODY]` (a notification on the desktop, which
  leads back to this pane), `olo clipboard TEXT` (copied there) and
  `olo paste` (what is copied there, as text).
- **A command run on the Olo desktop**: `olo run [--cwd DIR] -- COMMAND…`
  runs `COMMAND` on the person's desktop as their desktop account (in
  their desktop home unless `--cwd DIR` is given, with none of this
  host's environment). Requires the host's *Ask to run commands on this
  computer* permission in Preferences ▸ Rex (off by default). Each run opens an
  approval sheet on the desktop (showing the full command, this host and
  folder) unless Deskbar's *Approve Without Asking* is active, in which
  case it runs at once with a quiet desktop notification. Standard output
  and error stream back live as they arrive, the exit status is the
  command's own, and `^C` cancels the desktop command (or dismisses its
  approval sheet).
  - **When to use it:** driving the desktop's own programs, reading or
    modifying files that live only on the desktop, or accessing devices
    and services attached to the desktop (printers, audio, local hardware,
    desktop apps).
  - **When not to use it:** anything this host can do itself (builds,
    tests, git, file edits on this host), or tasks that already have a
    dedicated bridge command (`olo open`, `olo send`, `olo sync`,
    `olo forward`, `olo op`, `olo screen`).
- **The desktop commands**, where the person has put them on this host
  (Rex's Host window, "Desktop commands"): in a Rex session the usual
  commands already go to the desktop, so a program doesn't need to know
  of Olo: `xdg-open URL|FILE` (and `$BROWSER`), `notify-send`, `wl-copy`
  and `wl-paste`, `xclip`, `xsel`, `op`. Where the desktop can't be
  asked they are the host's own (`command -v wl-copy` shows which).

## A dev server here, seen on the desktop

A web app's dev server on this host (Vite at 5173, say), in the
person's browser or on your own screen there:

1. `olo forward 5173`: the desktop's localhost:5173 reaches this host's
   from then on, at the same port, so the app's own links, cookies and
   sign-in callbacks name an address that works there too. Add `--keep`
   for a lasting forward that Rex brings back automatically when Rex
   starts and when the host reconnects, even if this Rex session ends.
2. `olo open http://localhost:5173` for the person, or
   `olo screen run NAME olo-chrome http://localhost:5173` for yourself:
   the address goes through that forward (the page still says
   `localhost`).
3. Drive it on the screen: `olo-screen tree NAME`, `click`, `type`,
   `set`, `look`. What you type is sent as you wrote it, an address
   too: only what is opened (`run`'s and `start`'s arguments, `olo
   open`) is sent through a forward.
4. `olo forward close 5173` when you're done (also stops a `--keep`
   forward); without `--keep` it closes by itself when this Rex session
   ends.

Underneath the desktop's commands is Rex's own `rex do`, which you can
use directly (`rex actions --json` lists the actions with their
arguments). (`olo agent` is not underneath it: it asks this host's
service. The `olo.agent.*` actions are the same commands asked through
a connected desktop, which passes them to that service; use `olo
agent`, which needs none.)
`rex do olo.agent.start --args '{"args":["claude","--model","…","--effort","…"]}'`,
`rex do olo.agent.list --args '{"json":true}'`,
`rex do olo.agent.status --args '{"target":"ID","json":true}'`,
`rex do olo.agent.read --args '{"target":"ID","lines":80}'`,
`rex do olo.agent.watch --args '{"all":true,"since":42,"on":"question,approval,done,error,exit","timeout":90}'`,
`rex do olo.agent.send --args '{"target":"ID","text":"continue with tests","enter":true}'`,
`rex do olo.agent.approve --args '{"target":"ID","once":true}'`,
`rex do olo.agent.stop --args '{"target":"ID","save":"/var/tmp/out.txt"}'`,
`rex do olo.agent.prune --args '{"json":true}'`,
`rex do olo.agent.focus --args '{"target":"ID"}'`,
`rex do olo.open target=URL`, `rex do olo.get --args '{"args":["rex://local/PATH"]}'`,
`rex do olo.ls --args '{"args":["rex://local/PATH"]}'`, `rex do olo.op --args '{"args":[…]}'`,
`rex do olo.screen --args '{"args":[…]}'`, `rex do olo.forward --args
'{"args":["5173"]}'`, `rex do olo.send --args '{"args":["/whole/path"]}'`,
`rex do olo.sync --args '{"args":["/whole/path"]}'`,
`rex do olo.run --args '{"args":["--","uname","-a"]}'` (streamed with
`rex do olo.run.poll id=…` and cancelled with `rex do olo.run.cancel id=…`),
`rex do olo.notify title=…`,
`rex do olo.clipboard.set text=…`, `rex do olo.clipboard.get --args
'{}'`; an answer that says `pending` is
waited for with `rex do olo.wait id=…`, and a picture an answer lists
under `files` is fetched with `rex do olo.file id=… offset=…` (base64,
in pieces). The `olo` command does all of that: it picks the Olo
desktop among the server's clients (the connected one, passing over any
the server still lists as lost), waits while an answer is pending, and
saves pictures.

Rex itself is the `rex` command (`rex --help`; `rex skill setup`
installs its own skill).
