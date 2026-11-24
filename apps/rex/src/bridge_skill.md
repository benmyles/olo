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
- **Files and folders of this host, saved on the desktop**: `olo send
  dist/app.tar.zst build/` puts each in the person's Downloads ▸ Rex ▸
  HOST (a folder as a folder of its name), where the person may look at
  or use it; it says where each went and how much was sent. Sending the
  same again sends only what changed (a rebuilt archive's changed
  parts); `--mirror` also takes away, within that folder there, what
  is no longer here. Where the person hasn't let this host send files
  (Preferences ▸ Rex), it says so: ask them.
- **A folder kept in step on the desktop**: `olo sync dist` keeps this
  host's folder in step in Downloads ▸ Rex ▸ HOST ▸ dist from then on
  (what changes here is brought there moments later, by itself: a
  build's output, logs); `olo sync wait dist` brings it in step now and
  answers when it is (after a build: `make && olo sync wait dist`);
  `olo sync ls` lists this host's kept folders and how each stands;
  `olo sync stop dist` lets it go (what was brought stays). `--mirror`
  takes away there what is no longer here. The same leave as `olo send`.
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
   sign-in callbacks name an address that works there too.
2. `olo open http://localhost:5173` for the person, or
   `olo screen run NAME olo-chrome http://localhost:5173` for yourself:
   the address goes through that forward (the page still says
   `localhost`).
3. Drive it on the screen: `olo-screen tree NAME`, `click`, `type`,
   `set`, `look`. What you type is sent as you wrote it, an address
   too: only what is opened (`run`'s and `start`'s arguments, `olo
   open`) is sent through a forward.
4. `olo forward close 5173` when you're done; it closes by itself when
   this Rex session ends.

Underneath it is Rex's own `rex do`, which you can use directly
(`rex actions --json` lists the actions with their arguments):
`rex do olo.open target=URL`, `rex do olo.op --args '{"args":[…]}'`,
`rex do olo.screen --args '{"args":[…]}'`, `rex do olo.forward --args
'{"args":["5173"]}'`, `rex do olo.send --args '{"args":["/whole/path"]}'`,
`rex do olo.sync --args '{"args":["/whole/path"]}'`,
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
