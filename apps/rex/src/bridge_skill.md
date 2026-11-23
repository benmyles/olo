---
name: olo-rex-bridge
description: From a Rex session on this host, use the person's Olo desktop through Rex - open a link or a local server's page in their browser, read a secret from their Olo Passwords, and see and drive graphical apps on a screen of your own there (Olo Agent Screens). Use when a task on this host needs a browser, a GUI, a password or the person's eyes on something.
---

# The Olo desktop, through Rex

This host is used through **Rex** from a computer running the **Olo**
desktop. Olo's Rex app offers actions to this host's Rex server, and
you ask for them with Rex's own command, `rex do`. Nothing else is
installed here for it: there is no `olo-screen` or special `xdg-open`
on this host, and none is needed.

    rex do ACTION --json [KEY=VALUE…]          # or --args '{"key": …}'

`rex actions --json` lists every action with its arguments (the Olo
ones begin `olo.`). An answer is JSON: `ok`, and `stdout` and `stderr`
as the command wrote them on the desktop. Each action works only for
what the person has allowed this host (Olo's Preferences ▸ Rex); one
that isn't allowed fails and says so. If `rex do` says no app client
is connected, this host isn't open in Olo's Rex now: say so. If it
says it can't choose a client (more than one app is connected), name
one: `rex client ls`, then `rex -C ID do …`.

- **Open a link in the person's browser**:
  `rex do olo.open --json target=https://example.com`.
  A `http://localhost:PORT` of this host is reached through a forward
  Rex makes for it, where that is allowed.
- **A secret from Olo Passwords** (1Password's command line, answered
  on the desktop after the person approves there; no secret is kept on
  this host):
  `rex do olo.op --json --args '{"args":["read","op://Vault/Item/field"]}'`.
- **A screen of your own on the desktop** (Olo Agent Screens: a
  headless desktop the person can watch). `args` is `olo-screen`'s
  command line:
  - `rex do olo.screen --json --args '{"args":["start","NAME"]}'`
  - `… '{"args":["run","NAME","olo-chrome","https://example.com"]}'` (a web page)
  - `… '{"args":["tree","NAME"]}'` (what is on it, as text: read this first)
  - `… '{"args":["click","NAME","--role","Button","--name","OK"]}'`, `type`, `key`, `wait`
  - `… '{"args":["look","NAME"]}'` (a picture), `… '{"args":["stop","NAME"]}'` when done
  - `… '{"args":["--help"]}'` for the rest.
  A `http://localhost:PORT` of this host among the arguments is reached
  through a forward, as for `olo.open`.
- **A picture** an answer names is a file on the desktop, not here. The
  answer lists it under `files` with an `id`; fetch it with
  `rex do olo.file --json id=ID offset=0`, whose answer holds `base64`,
  `count` and `done` (ask again from `offset` + `count` until `done`):
  `rex do olo.file --json id=ID offset=0 | jq -r .base64 | base64 -d > /var/tmp/look.png`.
- **An answer that says `pending`** is of a command still running on
  the desktop (the person is being asked, a screen is starting): wait
  for it with `rex do olo.wait --json id=PENDING`, again while it says
  `pending`.
- Also: `rex do olo.notify title=… body=…` (a notification on the
  desktop, which leads back to this pane) and
  `rex do olo.clipboard.set text=…`.

Rex itself is the `rex` command (`rex --help`; `rex skill setup`
installs its own skill).
