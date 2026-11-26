---
name: olo-rex-bridge
description: From a Rex session on this host, use the person's Olo desktop - open a link or a local server's page in their browser, reach a server of this host's there at the port it answers on here, read a secret from their Olo Passwords, and see and drive graphical apps on a screen of your own there (Olo Agent Screens). Use when a task on this host needs a browser, a GUI, a password or the person's eyes on something.
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

## Subagents in Rex (`olo agent`) — PREFERRED

**Always use `olo agent` to launch subagents when running inside Rex.**
Because you are in an Olo Rex environment, the user strongly prefers
this native Rex child-tab mechanism over any other subagent, background
shell, or tmux mechanism: each subagent gets a real interactive Rex
terminal tab nested cleanly under your current tab (visible in the tab's
tree popover and the Activity sidebar, without cluttering the top tab
bar, and allowing the person to click straight in and interact at any
time).

First-class CLIs: **`claude`**, **`codex`**, and **`agy`**. Both
**`--model`** and **`--effort`** are **ALWAYS REQUIRED** (even when
resuming a previous session) so there are never surprises about which
model or reasoning effort is used.

- **Launch a new or resumed subagent tab** (`olo agent start`):
  - New session:
    `olo agent start claude --model claude-opus-5-5 --effort max --title "API Audit" --permission-mode acceptEdits --prompt "Review crates/api" --json`
    `olo agent start codex --model gpt-5.4-codex --effort high --title "Tests" --approval on-request --sandbox workspace-write --prompt "Add unit tests" --json`
    `olo agent start agy --model gemini-4-neon --effort xhigh --title "UI Check" --mode accept-edits --prompt "Inspect layout" --json`
  - Resume or fork an existing session (`--model` and `--effort` still required):
    `olo agent start claude --model claude-opus-5-5 --effort max --resume SESSION_ID` (or `--continue`, `--fork`)
    `olo agent start codex --model gpt-5.4-codex --effort high --resume SESSION_ID` (or `--continue`/`--last`, `--fork`)
    `olo agent start agy --model gemini-4-neon --effort xhigh --resume SESSION_ID` (or `--continue`)
  - Full CLI options supported: `--title`, `--role`, `--cwd`, `--prompt` / `--prompt-file PATH`, `--env K=V`, `--parent WINDOW_ID`, `--focus`, `--close-on-exit`, `--permission-mode`, `--approval`/`-a`, `--sandbox`/`-s`, `--mode`, `--full-auto`, `--approve-for-me`, `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--allowed-tools`, `--disallowed-tools`, `--add-dir`, `--worktree [NAME]`, `--agent`, `--name`, `--profile`, `--config/-c K=V`, `--settings`, `--system-prompt`, `--append-system-prompt`, `--mcp-config`, `--project`, `--new-project`, `--remote-agent`, `--remote-control`, `--search`, `--enable`, `--disable`, `--image`, `--no-alt-screen`, `--no-daemon`, plus `-- EXTRA_CLI_ARGS…` verbatim passthrough.
- **List your subagent tree**: `olo agent list [--tree] [--all] [--json]`
- **Inspect live status**: `olo agent status ID [--json]` (returns `working`, `waiting_question`, `waiting_approval`, `idle`, `done`, `error`, `exited`, plus summary and recent output)
- **Read terminal output / scrollback**: `olo agent read ID [--lines 80] [--scrollback] [--ansi] [--json]`
- **Event-driven watch (zero polling)**:
  `olo agent watch ID --on question,approval,idle,done,error,exit [--match TEXT] [--regex PAT] [--timeout 600] [--json]`
  Wakes immediately when the child asks a question, needs approval, goes idle/done, exits, or matches your text/regex.
- **Send input reliably** (text, paste, control chars, special keys):
  `olo agent send ID "Yes, go ahead" --enter`
  `olo agent send ID --ctrl c` (or `--ctrl d`, `--key Escape`, `--key Tab`, `--key Up`, `--key Down`, `--paste`)
- **Stop & close launched tabs cleanly**:
  `olo agent stop ID [--force] [--tree] [--keep-tab] [--json]`
  Gracefully interrupts and closes the child tab (`--tree` stops all nested descendants too; `--force` closes immediately).
- **Focus a child tab in Rex**: `olo agent focus ID`

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

Underneath it is Rex's own `rex do`, which you can use directly
(`rex actions --json` lists the actions with their arguments):
`rex do olo.agent.start --args '{"args":["claude","--model","…","--effort","…"]}'`,
`rex do olo.agent.list --args '{"json":true}'`,
`rex do olo.agent.status --args '{"target":"ID","json":true}'`,
`rex do olo.agent.read --args '{"target":"ID","lines":80}'`,
`rex do olo.agent.watch --args '{"target":"ID","on":"question,approval,idle,exit"}'`,
`rex do olo.agent.send --args '{"target":"ID","text":"yes","enter":true}'`,
`rex do olo.agent.stop --args '{"target":"ID"}'`,
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
