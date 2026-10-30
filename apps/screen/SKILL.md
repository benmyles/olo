---
name: olo-screens
description: See and operate graphical apps on the Olo desktop through a screen of your own (Agent Screens) - start apps, look at windows, read their widgets and text as a tree, click, drag, scroll, type, press shortcuts and choose menu commands, without touching the person's pointer, keyboard or windows. Use whenever a task needs a GUI app (testing a desktop app, filling in a dialog, checking what a window shows) and OLO_SESSION is set or `olo-screen` is on PATH.
---
<!-- Kept up to date by Olo; changes here are replaced. Delete this line to keep your own version. -->

# Olo Agent Screens

Olo gives agents screens of their own: each is a whole headless Olo
desktop (its own Deskbar, windows, pointer and keyboard focus) running the
person's programs and files. Nothing you do there moves the person's
pointer or takes their focus, and they can watch it live in the Agent
Screens app and take control if they need to. **Never** drive the person's
own screen; always work on a screen you started.

All of it is the `olo-screen` command (`olo-screen --help`).
Coordinates are the screen's logical pixels from its top left. Cmd is ⌘
(apps see it as Ctrl).

## Start, run, stop

```sh
olo-screen start                    # prints JSON; the name is "agent" (or agent-2, …)
olo-screen start review --size 1280x800
olo-screen run agent olo-styled-edit notes.txt     # prints the pid
olo-screen list
olo-screen stop agent               # ends the screen and everything on it
```

`eval "$(olo-screen env agent)"` sets `WAYLAND_DISPLAY` and
`OLO_SOCKET` for your shell, so programs you start yourself (tests, a
binary you just built) open on that screen. Screens are silent unless
started with `--sound`. Stop your screens when you are done.

## A web browser

Olo has Google Chrome (as a Wayland program): `olo-screen run agent
olo-chrome URL`. Chrome keeps one process per profile, so if the
person already has Chrome open, a launch with their profile opens its
window on *their* screen. Unless the task needs their signed-in sites,
give it a profile of its own:

```sh
olo-screen run agent olo-chrome --user-data-dir="$(mktemp -d)" https://example.com
```

Chrome's pages are in the tree too (a `Document` with the page's
title and address, then its headings, text, links, fields and buttons),
so `tree`, `click --name`, `set` and `wait` work on web pages. Chrome's
own tab strip and toolbar aren't: use `key` (`Cmd+L` for the address
bar, `Cmd+T`, `Cmd+W`) or a picture for those.

## Read what is on screen

Prefer the tree to pictures: it is exact, cheap and has coordinates ready
for clicking.

```sh
olo-screen windows agent            # id, app_id, title, focused, frame, content, popups (JSON)
olo-screen tree agent               # every Olo window's widgets, an outline
olo-screen tree agent --window 3 --json
olo-screen tree agent --role Row --max-len 60     # only rows, names cut short
olo-screen tree agent --contains "lines" --visible-only
```

An outline line is `Role "name" value="…" at X,Y WxH` plus flags
(`disabled`, `focused`, `checked`, `selected`, `clickable`), indented by
nesting, in screen pixels. Roles are AccessKit's (`Button`, `CheckBox`,
`TextInput`, `ComboBox`, `Row`, `Label`, …); `Clickable` is an area that
takes clicks (an icon, a menu title), named by the text inside it. A
table's rows say which they are, `row 13/61`; their cells show only with
`--cells`. An open menu or dropdown is a `Menu` with a `MenuItem` for each
item, under the window it opened from. Programs not built on Olo's kit
(GTK, Qt apps) have no tree: use a picture (their open menus are
still in `windows` as `popups`).

```sh
olo-screen look agent                             # the whole screen; prints the file's path
olo-screen look agent /tmp/win.png --window 3     # one window
olo-screen look agent --crop 100,80,400,300 --no-pointer
```

## Act

Find things by what they are rather than where: `--name` (exact, else
part of it; a field's value counts), `--role`, `--match ROLE:TEXT`,
`--text` (anywhere in name or value), `--index N` (from 0) and
`--window ID`. A miss says what there is of that role.

```sh
olo-screen click agent --role Button --name "="
olo-screen click agent --match "Row:panicked" --double
olo-screen set agent --name Find "PAM"                # click in, replace the text
olo-screen select agent --role ComboBox --index 1 "Errors Only"
olo-screen wait agent --text "61 lines"               # until it shows (--gone: until it goes)
olo-screen wait agent --idle                          # until the windows stop changing (and have drawn their sizes)
olo-screen scroll-to agent --match "Row:12:04:31"     # scroll the list until it shows
```

By position:

```sh
olo-screen click agent 320 200 [--right|--middle] [--double]
olo-screen move agent 320 200
olo-screen drag agent 100 100 400 300 [--steps 20] [--duration 300]
olo-screen down agent 100 100 ; olo-screen up agent
olo-screen scroll agent 0 120       # pixels, as a touchpad (positive: down/right)
olo-screen wheel agent 3            # mouse-wheel notches, 48 px each in Olo's lists
olo-screen type agent "Hello, world"
olo-screen key agent Cmd+S          # also Ctrl+Shift+T, Enter, Esc, Tab, Down, F5, …
olo-screen key agent Cmd+A Delete   # several chords in turn
```

Actions print JSON: what they matched and, under `after`, the focused
window and element. Typing goes to the focused element; `type` and `set`
warn when that isn't a text field (a list takes ⌘A as Select All).

Windows: `focus`, `close`, `minimize`, `zoom` (`--window ID`, else the
focused or only one), `resize agent 1200x800` (the content's size) and
`place agent X Y [WxH]`.

Menus are easier through their commands than by clicking:

```sh
olo-screen menus agent [--window 3]       # the menu bar: path, label, command, keys, enabled
olo-screen invoke agent save [--window 3] # choose it, as from the menu
```

Checking how an Olo app looks (one you are changing): `olo-screen
review agent [--window 3] [--json]` lists what its windows look to have
wrong as drawn now: errors (a blurry icon or line, a seam), then what
almost lines up, is almost one height, or sits off-centre, each with
where. Look closely at each, at every scale the screen is set to.

## What was done on a screen

Every action (`start`, `run`, `click`, `type`, `key`, `invoke`, `drag`,
…) is kept in the screen's history, with what it matched, how it went and
how long it took; it outlives the screen until cleared.

```sh
olo-screen history agent                      # one line each, oldest first
olo-screen history agent --kind click --last 5
olo-screen history agent --search Save --json
olo-screen stats [agent] [--json]             # started, last used, time running and in use, actions, programs
olo-screen history agent --look 12 /tmp/then.png   # the screen as it was when action 12 ended
olo-screen history agent --export /tmp/agent.mp4   # all of it as a video, quiet stretches cut
olo-screen history agent --clear
```

## Working well

- After each action, check the result (`tree`, or `look` for non-Olo
  apps) before the next; windows take a frame or two to update. Use
  `wait` rather than sleeps: some things take seconds (a search, Show in
  Context).
- A dropdown opens with its current item over the field, so the other
  items are above and below it; `select` finds them for you.
- New windows open focused; `windows` shows which one is.
- Logs: a screen's session log is
  `$XDG_RUNTIME_DIR/olo-screens/NAME.log`, and a program started with
  `run` logs to `NAME-PROGRAM.log` beside it. `olo-logs --print --json`
  reads Olo's journal.
- Don't sign in to the person's accounts, and don't open external URLs,
  unless the task needs it.
