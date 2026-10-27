---
name: tabula-screens
description: See and operate graphical apps on the Tabula desktop through a screen of your own (Agent Screens) - start apps, look at windows, read their widgets and text as a tree, click, drag, scroll, type, press shortcuts and choose menu commands, without touching the person's pointer, keyboard or windows. Use whenever a task needs a GUI app (testing a desktop app, filling in a dialog, checking what a window shows) and TABULA_SESSION is set or `tabula-screen` is on PATH.
---
<!-- Kept up to date by Tabula; changes here are replaced. Delete this line to keep your own version. -->

# Tabula Agent Screens

Tabula gives agents screens of their own: each is a whole headless Tabula
desktop (its own Deskbar, windows, pointer and keyboard focus) running the
person's programs and files. Nothing you do there moves the person's
pointer or takes their focus, and they can watch it live in the Agent
Screens app and take control if they need to. **Never** drive the person's
own screen; always work on a screen you started.

All of it is the `tabula-screen` command (`tabula-screen --help`).
Coordinates are the screen's logical pixels from its top left. Cmd is ⌘
(apps see it as Ctrl).

## Start, run, stop

```sh
tabula-screen start                    # prints JSON; the name is "agent" (or agent-2, …)
tabula-screen start review --size 1280x800
tabula-screen run agent tabula-styled-edit notes.txt     # prints the pid
tabula-screen list
tabula-screen stop agent               # ends the screen and everything on it
```

`eval "$(tabula-screen env agent)"` sets `WAYLAND_DISPLAY` and
`TABULA_SOCKET` for your shell, so programs you start yourself (tests, a
binary you just built) open on that screen. Screens are silent unless
started with `--sound`. Stop your screens when you are done.

## Read what is on screen

Prefer the tree to pictures: it is exact, cheap and has coordinates ready
for clicking.

```sh
tabula-screen windows agent            # id, app_id, title, focused, frame, content (JSON)
tabula-screen tree agent               # every Tabula window's widgets, an outline
tabula-screen tree agent --window 3 --json
```

An outline line is `Role "name" value="…" at X,Y WxH` plus flags
(`disabled`, `focused`, `checked`, `selected`, `clickable`), indented by
nesting, in screen pixels. Roles are AccessKit's (`Button`, `CheckBox`,
`TextInput`, `MultilineTextInput`, `Label`, `Cell`, …); `Clickable` is an
area that takes clicks (an icon, a menu title, a list row), named by the
text inside it. To click something, click the middle of its rectangle.
Programs not built on Tabula's kit (Chrome, GTK, Qt apps) have no tree:
use a picture.

```sh
tabula-screen look agent /tmp/shot.png               # the whole screen, pointer drawn in
tabula-screen look agent /tmp/win.png --window 3     # one window
```

## Act

```sh
tabula-screen click agent 320 200 [--right|--middle] [--double]
tabula-screen move agent 320 200
tabula-screen drag agent 100 100 400 300
tabula-screen down agent 100 100 ; tabula-screen up agent
tabula-screen scroll agent 0 120       # pixels, as a touchpad (positive: down/right)
tabula-screen wheel agent 3            # mouse-wheel notches
tabula-screen type agent "Hello, world"
tabula-screen key agent Cmd+S          # also Ctrl+Shift+T, Enter, Esc, Tab, Down, F5, …
tabula-screen key agent Cmd+A Delete   # several chords in turn
```

Typing goes to the focused window; click into a field first. Menus are
easier through their commands than by clicking:

```sh
tabula-screen menus agent [--window 3]       # the menu bar: path, label, command, keys, enabled
tabula-screen invoke agent save [--window 3] # choose it, as from the menu
```

## Working well

- After each action, check the result (`tree`, or `look` for non-Tabula
  apps) before the next; windows take a frame or two to update.
- New windows open focused; `windows` shows which one is.
- Logs: a screen's session log is
  `$XDG_RUNTIME_DIR/tabula-screens/NAME.log`, and a program started with
  `run` logs to `NAME-PROGRAM.log` beside it. `tabula-logs --print --json`
  reads Tabula's journal.
- Don't sign in to the person's accounts, and don't open external URLs,
  unless the task needs it.
