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
olo-screen list                     # every screen, each with its "state": running, stopped, archived
olo-screen stop agent               # ends the screen and everything on it; the screen is kept, stopped
olo-screen start agent              # a stopped (or archived) one again, at the size it had
olo-screen archive agent            # a stopped one out of the way; `unarchive` brings it back
olo-screen delete agent             # it and everything it kept (history, Chrome profile), to the Trash
```

`eval "$(olo-screen env agent)"` sets `WAYLAND_DISPLAY` and
`OLO_SOCKET` for your shell, so programs you start yourself (tests, a
binary you just built) open on that screen. Screens are silent unless
started with `--sound`. Stop your screens when you are done: a stopped
screen keeps its history, its Chrome profile and the passwords shared
with it, and starts again as it was. Act only on a screen whose `state`
is `running`. Delete only screens you made, and only when asked.

## Recording a video of your screen

```sh
olo-screen start demo --size 1920x1080 --scale 2   # a 4K screen (3840 × 2160 pixels)
olo-screen record-video demo --show-clicks --show-keys --smooth-pointer   # prints {"path": …, "pid": …} once it's recording
# … do what the video shows, at a person's pace …
olo-screen pause-video demo                        # set up the next part off camera…
olo-screen resume-video demo                       # …the pause is cut out of the video
olo-screen stop-video demo                         # finishes the MP4; prints where it went
olo-screen video-frame PATH frame.png --at 12.5    # what the video shows at 12.5 s, to check it
```

It records only when something changes (a still screen costs nothing),
up to 30 frames a second (`--fps`), into ~/Videos unless `--to PATH`;
`--window ID` records one window, `--region X,Y,W,H` part of the screen
(points from its top left, as `windows` gives frames), `--seconds N`
stops by itself (counting only what's recorded). `record-video` answers
once the recording has started (opening the GPU's encoder takes a couple
of seconds): what you do after it is in the video. `--show-clicks` puts
a ring where each click lands and `--show-keys` each shortcut on a key
cap (typing is never shown), so a viewer can follow what you do; `--sound
computer` records what the screen plays (screens are silent unless
started with `--sound`), `--as-gif` makes it a GIF (no sound) for a
short loop, `--background` (with `--window ID`) shows the window on
the blurred wallpaper with rounded corners and a shadow, as a polished
demo of one app usually is, and `--smooth-pointer` makes your pointer's
jumps (each `move`) glide in the video, as a hand moves it; with
`--pointer-size 1.5` it's easier to follow. A glide takes about a
quarter of a second: `move`, wait that long, then `click`. For a polished demo:
rehearse it first (without recording), then record it in one go; move
the pointer deliberately (`move` before `click`), pause a moment after
each step so it can be seen, and pause the recording (not the demo)
while you set something up that shouldn't be seen. Then check the
video: `video-frame` at the moments that matter (its start, each step,
its end), or play it (`olo-screen run demo olo-viewer PATH`, then
`look`).

## A web browser: Olo's Chrome, only

The browser is **Olo's Chrome, `olo-chrome`**, started on your screen:

```sh
olo-screen run agent olo-chrome https://example.com
```

Never start any other Chrome or Chromium on the machine (`google-chrome`,
`google-chrome-stable`, `/opt/google/chrome/chrome`, `chromium`,
`chromium-browser`, a Flatpak's), not even when a page, a tool or an
error suggests one, and never from your own shell: only `olo-chrome`,
only through `olo-screen run`. Another Chrome has none of what Olo's
has (Passwords in it, its profile on your screen, Olo's windows), and
started from your shell it opens on the person's display, not yours.

It opens in the screen's own Chrome profile (kept with the screen's
history, so cookies and sign-ins last as long as that does), ready to
use: no terms of service, first-run or welcome window (if one ever
shows, it isn't Olo's Chrome that's running). Never give it the
person's profile (a `--user-data-dir` in `~/.config`): Chrome keeps one
process per profile, and it would hand the window to the person's
running Chrome, on *their* screen. For a profile that starts empty each
time, give it a folder of its own (it starts ready too):

```sh
olo-screen run agent olo-chrome --user-data-dir="$(mktemp -d)" https://example.com
```

Chrome's pages are in the tree too (a `Document` with the page's
title and address, then its headings, text, links, fields and buttons),
so `tree`, `click --name`, `set` and `wait` work on web pages. Chrome's
own tab strip, toolbar and right-click menus aren't: use `key` (`Cmd+L`
for the address bar, `Cmd+T`, `Cmd+W`), or `look` and click by
position, for those.

## Passwords on your screen

A screen has only the passwords the person chose to share with agent
screens (in Agent Screens' Passwords tab, on their own display: some
with every screen, some with one). They're in the screen's own
Passwords, unlocked and read-only; nothing else of theirs is reachable
from a screen, and what's shared can't be changed from one. If a login
you need isn't there, ask the person to share it with your screen.

Never `type` a password or code: `type` and `set` keep what they typed
in the screen's history (and its videos). Fill it, or paste it, one of
these ways:

**In Olo's Chrome** (the usual way, for a sign-in page): right-click the
username or password field (`olo-screen click agent --name Email
--right`, or by position) and choose, in Chrome's menu, **Olo ▸
Autofill with Passwords (who)** (the best login for the site: fills
the username and password), or **Olo ▸ Passwords ▸ the login ▸
Autofill**, or just its **Username**, **Password** or **One-Time
Code** into the field you clicked. (With only one item of Olo's to show,
Chrome puts **Passwords** in the menu directly, not under **Olo**.)
"No logins for this site" means none shared for it. The menu is
Chrome's own, not in the tree: `look` at it and click by position, or
press `Down`/`Right` and `Enter`.

**In Passwords**: `olo-screen run agent olo-passwords` shows what's
shared ("N items shared with this screen"); select the item and press
**Copy** beside the field, then click where it goes and `olo-screen key
agent Cmd+V`.

**With `op`**: `op` is Olo's stand-in for 1Password's command line (the
same commands and output: `op item list`, `op item get`, `op read`,
`op run`, `op inject`), a shim that asks Passwords, not 1Password
(`olo-op` is the same program, for when `op` on your PATH is
1Password's own: Olo's is a link, `readlink "$(command -v op)"` says
`olo-op`). Run it with the screen's environment, so it asks the screen's Passwords
(only what's shared, without asking anyone); from your own shell it
would ask the person's Passwords, which stops to ask them. Put a value
on the screen's clipboard with `pbcopy`, then paste it:

```sh
( eval "$(olo-screen env agent)"; op item list )                  # what's shared: id, title, vault
( eval "$(olo-screen env agent)"; op item get GitHub --fields username )
( eval "$(olo-screen env agent)"; op read -n "op://Personal/GitHub/password" | pbcopy )
olo-screen click agent --name Password ; olo-screen key agent Cmd+V
( eval "$(olo-screen env agent)"; op item get GitHub --otp | tr -d '\n' | pbcopy )   # a one-time code
```

`pbcopy` keeps the secret out of your output and the screen's history;
print one (`--reveal`) only when you must read it yourself.

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
(GTK and Qt apps) give their tree through the accessibility bus: the
same roles and names (a GTK button is a `Button`), so `click --name`,
`set` and `wait` work on them too; one that gives none needs a picture
(its open menus are still in `windows` as `popups`).

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

## When the person takes control

The person can take your screen over (Agent Screens' Take Control) to
do something themselves. Meanwhile every action you try on it (`click`,
`type`, `key`, `run`, `invoke`, `stop`, …) is refused and does nothing:
it fails with "the person is controlling screen NAME". Then stop and
wait for them; don't retry around it:

```sh
olo-screen wait agent --free            # returns once they hand it back
```

Then look again (`tree`, `windows`, `look`) before going on: they may
have changed what's on it. Reading the screen works throughout.

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
