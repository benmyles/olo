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
tabula-screen windows agent            # id, app_id, title, focused, frame, content, popups (JSON)
tabula-screen tree agent               # every Tabula window's widgets, an outline
tabula-screen tree agent --window 3 --json
tabula-screen tree agent --role Row --max-len 60     # only rows, names cut short
tabula-screen tree agent --contains "lines" --visible-only
```

An outline line is `Role "name" value="…" at X,Y WxH` plus flags
(`disabled`, `focused`, `checked`, `selected`, `clickable`), indented by
nesting, in screen pixels. Roles are AccessKit's (`Button`, `CheckBox`,
`TextInput`, `ComboBox`, `Row`, `Label`, …); `Clickable` is an area that
takes clicks (an icon, a menu title), named by the text inside it. A
table's rows say which they are, `row 13/61`; their cells show only with
`--cells`. An open menu or dropdown is a `Menu` with a `MenuItem` for each
item, under the window it opened from. Programs not built on Tabula's kit
(Chrome, GTK, Qt apps) have no tree: use a picture (their open menus are
still in `windows` as `popups`).

```sh
tabula-screen look agent                             # the whole screen; prints the file's path
tabula-screen look agent /tmp/win.png --window 3     # one window
tabula-screen look agent --crop 100,80,400,300 --no-pointer
```

## Act

Find things by what they are rather than where: `--name` (exact, else
part of it; a field's value counts), `--role`, `--match ROLE:TEXT`,
`--text` (anywhere in name or value), `--index N` (from 0) and
`--window ID`. A miss says what there is of that role.

```sh
tabula-screen click agent --role Button --name "="
tabula-screen click agent --match "Row:panicked" --double
tabula-screen set agent --name Find "PAM"                # click in, replace the text
tabula-screen select agent --role ComboBox --index 1 "Errors Only"
tabula-screen wait agent --text "61 lines"               # until it shows (--gone: until it goes)
tabula-screen wait agent --idle                          # until the windows stop changing
tabula-screen scroll-to agent --match "Row:12:04:31"     # scroll the list until it shows
```

By position:

```sh
tabula-screen click agent 320 200 [--right|--middle] [--double]
tabula-screen move agent 320 200
tabula-screen drag agent 100 100 400 300 [--steps 20] [--duration 300]
tabula-screen down agent 100 100 ; tabula-screen up agent
tabula-screen scroll agent 0 120       # pixels, as a touchpad (positive: down/right)
tabula-screen wheel agent 3            # mouse-wheel notches, 48 px each in Tabula's lists
tabula-screen type agent "Hello, world"
tabula-screen key agent Cmd+S          # also Ctrl+Shift+T, Enter, Esc, Tab, Down, F5, …
tabula-screen key agent Cmd+A Delete   # several chords in turn
```

Actions print JSON: what they matched and, under `after`, the focused
window and element. Typing goes to the focused element; `type` and `set`
warn when that isn't a text field (a list takes ⌘A as Select All).

Windows: `focus`, `close`, `minimize`, `zoom` (`--window ID`, else the
focused or only one), `resize agent 1200x800` (the content's size) and
`place agent X Y [WxH]`.

Menus are easier through their commands than by clicking:

```sh
tabula-screen menus agent [--window 3]       # the menu bar: path, label, command, keys, enabled
tabula-screen invoke agent save [--window 3] # choose it, as from the menu
```

## Working well

- After each action, check the result (`tree`, or `look` for non-Tabula
  apps) before the next; windows take a frame or two to update. Use
  `wait` rather than sleeps: some things take seconds (a search, Show in
  Context).
- A dropdown opens with its current item over the field, so the other
  items are above and below it; `select` finds them for you.
- New windows open focused; `windows` shows which one is.
- Logs: a screen's session log is
  `$XDG_RUNTIME_DIR/tabula-screens/NAME.log`, and a program started with
  `run` logs to `NAME-PROGRAM.log` beside it. `tabula-logs --print --json`
  reads Tabula's journal.
- Don't sign in to the person's accounts, and don't open external URLs,
  unless the task needs it.
