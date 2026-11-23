---
name: olo-rex-bridge
description: From a Rex session on this host, use the person's Olo desktop - open a link or a local server's page in their browser, read a secret from their Olo Passwords, and see and drive graphical apps on a screen of your own there (Olo Agent Screens). Use when a task on this host needs a browser, a GUI, a password or the person's eyes on something.
---

# The Olo desktop, from this host

This host is used through **Rex** from a computer running the **Olo**
desktop. One command here asks that desktop for things, and answers as
if it had run here:

    ~/.claude/skills/olo-rex-bridge/olo COMMAND [ARGUMENT…]

It works in a Rex session that is open in Olo's Rex, and only for what
the person has allowed this host (Olo's Preferences ▸ Rex): a command
that isn't allowed fails and says so. If it says no Olo desktop is
connected, this host isn't open in Olo's Rex now: say so.

- **A link in the person's browser**: `olo open https://example.com`.
  A `http://localhost:PORT` of this host is reached through a forward
  Rex makes for it, where that is allowed.
- **A secret from Olo Passwords** (1Password's command line, answered
  on the desktop after the person approves there; no secret is kept on
  this host): `olo op read op://Vault/Item/field`.
- **A screen of your own on the desktop** (Olo Agent Screens: a
  headless desktop the person can watch), with `olo-screen`'s own
  command line:
  - `olo screen start NAME`
  - `olo screen run NAME olo-chrome https://example.com` (a web page)
  - `olo screen tree NAME`: what is on it, as text. Read this first,
    and for anything word for word: it has every paragraph whole.
  - `olo screen click NAME --role Button --name OK`, `type`, `key`,
    `wait`. Each answers with the screen as it is after it, in brief
    (text cut short): `tree` has the whole.
  - `olo screen look NAME`: a picture, saved on this host; the answer
    names the file here (under `/var/tmp/olo-rex-bridge`).
  - `olo screen stop NAME` when done; `olo screen --help` for the rest.
  A `http://localhost:PORT` of this host among the arguments is reached
  through a forward, as for `olo open`.
- Also `olo notify TITLE [BODY]` (a notification on the desktop, which
  leads back to this pane) and `olo clipboard TEXT`.

Underneath it is Rex's own `rex do`, which you can use directly
(`rex actions --json` lists the actions with their arguments):
`rex do olo.open target=URL`, `rex do olo.op --args '{"args":[…]}'`,
`rex do olo.screen --args '{"args":[…]}'`, `rex do olo.notify title=…`,
`rex do olo.clipboard.set text=…`; an answer that says `pending` is
waited for with `rex do olo.wait id=…`, and a picture an answer lists
under `files` is fetched with `rex do olo.file id=… offset=…` (base64,
in pieces). The `olo` command does all of that: it picks the Olo
desktop among the server's clients (the connected one, passing over any
the server still lists as lost), waits while an answer is pending, and
saves pictures.

Rex itself is the `rex` command (`rex --help`; `rex skill setup`
installs its own skill).
