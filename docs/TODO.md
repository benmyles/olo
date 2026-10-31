# Work list

What's left before Olo is complete. Keep it current: strike items
as they land (with the commit), add new ones as they come up.

## Done recently

- ~~**Keyboard profiles**~~ (Ben, 2026-10-29; done: HACKING, "Keyboard
  profiles"): Mac, Windows or BeOS keys for everything, true to each
  platform; every app's menu keys come from the profile; the shortcuts
  sheet (⇧⌘/); Tracker's ⌘↑ and ⌘⌫ work (they arrived as Ctrl+Home and
  a line deletion before).
- ~~**Every display has its own workspaces**~~ (Ben, 2026-10-29; done:
  HACKING, "Workspaces, one set per display"): each display switches
  among its own 1–4 (or however many) on its own; Deskbar is on every
  display (the setting is gone), each listing only its display's
  workspaces, headed "D0 Workspace 1" with more than one display, with
  the workspace's windowing mode at the heading's right (no longer in
  the tray) and a menu to show, hide, close or move all its windows to
  any workspace of any display; Twitcher's columns and the overview's
  panes are named the same way. Also fixed: with the external displays
  plugged in, the laptop's screen had no Deskbar (the first bar, opened
  before the displays were known, named none and followed the main
  display to the Studio Display, while Deskbar took it for the
  laptop's). The tray's icons share their rows evenly.

- ~~Terminal: ⌘- / ⌘= and Ctrl+_~~: ⌘- and ⌘= (Smaller, Bigger) did
  nothing, the shell's Ctrl+_ was out of reach, and the menus read ⇧⌘N
  for ⌘N. ⌘ now reaches terminals on the right Ctrl, which the kit marks
  as `command` ("Raw keys" in HACKING), and the menus read as pressed.

- ~~DiskUsage~~ (Ben): a new app for finding and reclaiming disk space,
  Haiku's ring chart beside a list by size, with DaisyDisk's Collector
  (drag in, then Move to Trash or Delete); volumes from mountinfo, a
  streaming worker scan that stays on one file system.

- ~~Touchpad drags, palms and stalls~~ (Ben): Olo's touch filter (a
  libinput plugin app_server loads; docs/TOUCHPAD.md) counts a touch
  only while it presses, ignores palms and makes the click from the
  fingers alone; Preferences ▸ Touchpad detects the pad, recommends a
  profile (MacBook, Magic Trackpad, Standard, Custom) and has every
  tunable: filter thresholds, click pressure, scrolling speed, glide,
  three-finger drag, drag lock, button maps, acceleration, typing
  timeouts. The driver-side fix is written up, not done (Ben's call).

- ~~⌘Tab across workspaces~~ (Ben): ⌘Tab (⌥Tab keeps to this
  workspace's apps, ⌃Tab is Next Tab; the other way round at first): in
  order of use whatever the workspace, so ⌘Tab flips between the last
  two apps, and Deskbar highlights the app chosen; a row of apps per workspace;
  ↑/↓ change rows (and workspaces), ←/→ walk a row, Esc puts everything
  back, and with keyboard hints on each cell has a letter to pick it by.
- ~~⌘Space finds an app's open windows first~~ (Ben): typing an app's
  name lists its windows before the app, so the palette switches windows
  too.
- ~~Group an app's windows~~ (Ben): an app's windows share one frame
  with a proper tab bar (side by side, never overlapping); Group Windows
  in Deskbar's team menu and Tracker's menu for an app turns it off or
  on per app, saved in `ungrouped_apps`.
- ~~⌘V doesn't paste in Terminal~~ (Ben): settings saved before the
  rename listed `tabula.terminal` as a terminal, so Olo's Terminal got
  ⌘V as Ctrl+V (bash's quoted-insert). Old app ids in saved settings are
  Olo's now (`adopt_old_app_ids`).
- ~~Deskbar's Olo menu arranges windows~~ (Ben): Window Arrangement,
  Workspaces (with Move Window To) and Show or Hide All Windows, acting
  on the window that had the focus.
- ~~Zoom All, Window to Workspace 1–9~~ (Ben): ⌃⌥⌘↩ zooms (or unzooms)
  every window on the display; ⌃⇧⌘1–9 move the focused window to a
  workspace.
- ~~⌘Tab keeps the stacking~~ (Ben): windows passed over go back where
  they were; only the chosen one is raised.
- ~~Deskbar shows the wordmark alone~~ (Ben), centred in its tile.
- ~~Rolled-up windows share one tile~~ (Ben): Tile All (and Tile App) gives
  the rolled-up windows one cell between them, their tabs fanned out along
  its top (a row lower when full), not a nearly empty cell each; tab
  offsets now survive a reload.
- ~~Cascade Others, Fan Out Others~~ (Ben): ⌃⇧⌘\ and ⌃⇧⌘/ arrange every
  window but the focused one, which is left where it is, in front.
- ~~Keyboard hints everywhere~~ (Ben; opt-in, Preferences ▸ Keyboard):
  pressing Control twice letters every control, menu item, list row, icon, window and
  Deskbar entry; a letter picks it or letters what's inside, as Vim's
  EasyMotion does. From the kit's trees (Tracker's icons and column rows
  are in them now, as are layer surfaces), so every kit app takes part;
  agents read the letters with `{"type":"hints"}`.
- ~~Resizing tiled windows is slow~~ (Ben): neighbours following a
  dragged edge got a configure per pointer motion and fell seconds
  behind, overlapping the dragged window by up to 100 px. They're paced
  like the dragged window and moved only when they draw (a 500 Hz drag:
  302 configures and 1.8 s to catch up before, about 110 and 0.03 s now;
  the gap stays even).
- ~~AI features use OpenAI's models~~ (Ben): Preferences ▸ AI offers
  Astra 6 (default), Sol 6.1, Terra 6.1 and Luna 6, through the Token
  Pool's Codex accounts; a Claude model chosen before reads as Astra.
  Every call retries 429/503/529 with backoff (up to 4 tries). The pool
  no longer benches an account for a 429 that isn't a spent meter (it
  was turning every account away with "out of quota" at 13% use) and
  passes the provider's own message back.
- ~~Tabula is now Olo Desktop~~ (Ben): plain `olo` in code (crates,
  programs, app ids `olo.X`, `OLO_*`, `~/.config/olo`…, D-Bus
  `com.olodesktop.*`), "Olo Desktop" in public; the brand (mark, wordmark,
  desktop icon) in Deskbar, About, the session entry and the README. A
  running Tabula session upgrades through the green arrow into Olo with
  every window, shell and setting kept (`olo_xdg::legacy`; HACKING
  "Renamed from Tabula"). The wallpaper pack (data/wallpapers) too.
- ~~Icons in the Deskbar menu~~ (Ben): every item in the Olo menu
  has its icon (applications, panels, .desktop entries, commands,
  categories; twelve new sunburst icons); Appearance ▸ Show icons in the
  Deskbar menu, on by default. Kit menus take `.icon(name)`.
- ~~Roll up windows~~ (Ben): ⌃⌘M, Roll Up in the tab's menu, the palette,
  or double-click the tab: only the tab stays; again rolls it down.
- ~~Longest tab title~~ (Ben): Preferences ▸ Appearance ▸ Longest window
  title (20–200 characters, 100 by default); longer titles end in "…".
- ~~Terminal: Rename Group and Ungroup did nothing~~ (Ben): no context
  menu of Terminal's worked (group, tab, text); the kit now sends a
  popup's choice that no widget takes to `App::command`.
- ~~AI preferences, and Smart Tiling~~ (Ben): Preferences ▸ AI uses the
  Token Pool by default (or a saved Anthropic key); Smart Tiling (on,
  Claude Opus 5.5 at medium; also Sonnet, Haiku, Fable and Astra 6)
  shows the model a screenshot, each window's app, title and widget
  outline, and applies its tiling (⌃⇧⌘↩, undoable, with progress and
  Cancel). tsession sessions now get their own D-Bus.
- ~~Resizing after Tile All moves the others~~ (Ben): Tile All and Tile
  App now tile each window into its cell (`Tile::Cell`), so dragging an
  edge resizes the neighbours on that line.
- ~~Token Pool follow-ups~~: a reset credit about to expire is spent at
  its best moment rather than lost (projected from each window's rate),
  and Claude Code's settings.json keeps its key order when edited.
- ~~Screenshot Window from the palette~~ (Ben): takes the window the
  palette was opened over (`--window ID`), not whatever is active then.
- ~~Token Pool heatmap marks~~ (Ben): a cell's "C"/"X" is centred on its
  capitals and kept inside the well.
- ~~DriveSetup: usage and free space~~ (Ben): a Free column with a bar of
  how full each mounted volume is (red past 90%), "N free of M" in the
  status line, `space` in `--list`, re-read every 5 s while open.
- ~~Fan Out and Undo Window Arrangement~~ (Ben): ⌃⌘/ stacks a display's
  windows with their tabs side by side, narrowing them to stay clear of
  the Deskbar (never below a quarter of the screen); ⌃⌘Z undoes the last
  arrangement (and again redoes). Also: Deskbar keeps its app list after
  an in-place reload (a layer surface came back at its first size).
- ~~Terminal: Show All Tabs, and tab groups that look right~~ (Ben):
  Show All Tabs (⌘⇧D) tiles the window's tabs as live panes, typing into
  the one clicked (it used to tile the group windows instead); the tab
  strip is taller with the group's band clear of the icons; a dialog's
  first field starts with its text selected.
- ~~Token Pool: a live heatmap of requests~~ (Ben): the requests open
  now as lit cells, each account's latest 25 as a strip coloured by how
  they ended, click for the details; the kit's new `CellGrid` (Pulse's
  LEDs). HACKING, "Token Pool" ▸ Activity, and Widgets.
- ~~Token Pool: reset credits~~: shown per account, spent when every
  account is out, or by hand.
- ~~A widget tree for Chrome~~ (Ben): the Olo extension's host
  answers Agent Screens' `tree` for Chrome, with each window's page as
  assistive technology sees it; the olo-screens skill says so.
  HACKING, "Chrome's widget tree, for agents".
- ~~Num Lock on by default, and a setting for it~~ (Ben): Preferences ▸
  Keyboard ▸ "Num Lock on when Olo starts".
- ~~Token pooling for Claude Code and Codex, natively~~ (Ben): Token
  Pool (`olo-pool`, crates/pool): a loopback gateway the clients are
  pointed at, subscriptions drained earliest-expiry-first, sign-ins
  sealed with a keyring key, usage counted without content. HACKING,
  "Token Pool".
- ~~NetPositive removed~~ (Ben): Olo's web browser is Chrome. Gone
  with it: apps/browser, the Servo and WebRender patches (and epaint's,
  needed only because of Servo), the kit's embedded surfaces, and
  passkeys in Passwords (only NetPositive asked for them).
- ~~1Password's `op` from Passwords~~ (Ben): `olo-op`, linked as `op`,
  answers `op inject`/`read`/`run`/`item get`/`whoami`/`account list`/`signin`
  from Passwords, asking once per terminal session; Preferences ▸
  Security turns it off. HACKING, "op: 1Password's command line".
- ~~Long menus scroll~~: kit popups shrink to the screen and scroll
  (arrows at the ends, wheel, keys).
- ~~Hide Window everywhere~~: ⌘M (Preferences ▸ Keyboard) and the
  palette's "Hide window", for any app's window.
- ~~Log Viewer~~ (70aea93).
- ~~Viewer: pop-out from Quick Look, PDF forms, text, ink, signatures,
  Save / Save As / Export Flattened~~ (903ba8b, f01bff9).
- ~~Kit FilePanel~~ (903ba8b).
- ~~Preferences app~~: launcher, Appearance, Backgrounds, Display, Keyboard
  (incl. the ⌘ / Caps Lock / tap-⌘ toggles), Touchpad, Workspaces, About;
  `Cx::activate` over xdg-activation.
- ~~Power management and caffeinated mode~~: app_server turns the display
  off and sleeps when idle (Preferences ▸ Power), honours zwp_idle_inhibit,
  handles the brightness and volume keys; caffeinated mode from the Deskbar
  cup or Preferences holds a logind inhibitor. Display ▸ Brightness. ~~On-screen
  level indicator~~. ~~Keyboard backlight~~
  (keys, Preferences ▸ Keyboard, off with the display). ~~Real DPMS off~~ (CRTCs
  cleared, rendering paused until input).
- ~~Preferences ▸ Sound~~: output/input device, level, mute, live from the
  sound server (`olo_system::audio::{devices, watch_devices, set_*}`).

- ~~Upgrades keep apps open~~ (only a new app_server reloads; Olo
  programs re-execute in place), ~~interactive resize keeps up~~,
  ~~focus not stolen by xdg_activation without input~~ (15c2b27).
- ~~Keyring prompts are Olo's~~: the polkit agent owns
  org.gnome.keyring.SystemPrompter (gcr's protocol and secret exchange,
  natively), so unlocking the login keyring (gh's git credentials hung
  on a GTK prompter that had lost its display) asks in an Olo alert.
- ~~Login keyring in a real session~~ (the session finishes starting the
  daemon PAM unlocked), ~~Passwords lists 1Password's accounts~~.

## Native rule (no wrapping system tools)

- ~~nmcli, wpctl, systemctl, gsettings, dbus-update-activation-environment~~
  → `olo-system` (NetworkManager, PulseAudio protocol with a change
  feed, logind, dconf, activation environment).

- ~~Keyboard mapping in app_server, replacing keyd~~: ⌘ as command,
  Caps Lock as Ctrl, ⌘Tab / ⌥Tab / ⌘\` switching, ⌘ arrows / ⌘⌫, tap ⌘ /
  ⌘Space for Find, terminals keep physical Ctrl (`keymap.rs`), Ctrl+⌘Q
  locks. keyd can now be disabled system-wide; while
  it runs it grabs the keyboards and app_server sees its output (handled:
  its ⌘ tap chord and Super+Tab are understood).
- ~~Kit opener instead of `xdg-open`~~: `olo_xdg::open` (shared-mime-info
  globs + sniffing, our apps first, mimeapps.list, Exec field codes).

## Apps

- **Agent Screens: Take Control is laggy** (Ben, 2026-10-31): "when using
  take control in agent screens it is very laggy -- this needs to feel as
  buttery smooth and be as performant as if we were using the desktop
  directly." Measure the path (input forwarded to the headless
  app_server, its frames brought back and shown) frame by frame and make
  it the same 120 fps, input-to-photon, as the desktop: frames shared as
  dmabufs rather than copied, input sent at once, no polling.
- **Agent Screens: Chrome opened from a screen's Deskbar lands on the
  person's display** (Ben, 2026-10-31): "i was just using the "Take
  Control" feature in an agent screen, and i used the deskpad to try open
  a new chrome, but it actually opened it on my user screen instead of in
  the agent screen". Anything launched on an agent screen (its Deskbar,
  its Tracker, an agent's `olo-screen run`) must open there. Likely cause
  (to confirm): Chrome's single-instance handoff, where the new process
  finds the person's running Chrome by its profile and asks it for a
  window, which that Chrome opens on its own Wayland connection; so a
  screen's Chrome needs its own instance (its own profile dir, or a
  Chrome running on that screen), and every launcher there passes the
  screen's `WAYLAND_DISPLAY`/`OLO_SOCKET`. Check the other single-instance
  apps (ours via D-Bus activation, Slack, GTK's GApplication) the same way.
- **Agent Screens: rename a screen** (Ben, 2026-10-31): "i should be able
  to rename an agent screen". A name the person gives (in the list, the
  screen's title and its menu, as Tracker renames: click the name or
  Rename…), kept with the screen's history and shown wherever the screen
  is (Deskbar's item, move-to menus, `olo-screen list`, the agent's tree).
- ~~**Deskbar still shows agent screens running after they've all
  stopped**~~ (Ben, 2026-10-31; fixed): "i have stopped all the agent
  screens but in the deskpad it still shows the icon that indicates
  agent screens are running". Cause: Deskbar's 30-second re-count (for a
  screen killed without a word) held the status lock while re-reading,
  which takes it again: a deadlock the first time it ran with a screen
  up, after which nothing in the tray changed (agent screens, the bell's
  count, Upgrade, battery, Wi-Fi, sharing); since fc8fd47 (2026-10-30).
  Seen in the running Deskbar:
  both status threads waiting on the lock. The lock is let go first now.
- **Agent Screens: archive and delete, one or many** (Ben, 2026-10-31):
  "i should be able to archive stopped agent screens so that they no
  longer show in the list but there should be a menu option to show
  archived to bring them back into the list but in the list the archived
  ones should have their own section. there should also be an option to
  delete. and for both archive/delete i should be able to multi-select
  agents in the sidebar list and right click to take bulk actions like
  archive/delete etc". So: Archive (stopped screens) hides a screen from
  the list; View ▸ Show Archived brings them back in their own section
  (with Unarchive); Delete removes a screen and its history, recordings
  and stats (to the Trash where they're files, asked once for many); the
  sidebar takes ⇧/⌘-click and ⌘A selection, and its context menu acts on
  every selected screen (Archive, Unarchive, Delete, Stop).
- **Passwords for Agent Screens** (Ben, 2026-10-31): "in the Agent
  Screens app i can choose items from passwords that i want to make
  available by default to all new and existing agent screen sessions and
  these items should be available to the agent by just opening the
  passwords app in the agent screen and they'll see it unlocked
  automatically and scoped to just the items i've chosen to make
  available. on any given agent screen session i should be able to
  further customize it in a new "Passwords" tab where i can optionally
  remove any grants already provided or add more. and for both places
  when defining grants it needs to be powerful: i should be able to
  search and filter to add/remove multiple matching items in bulk". So:
  default grants in Agent Screens (settings), per-screen grants in a
  Passwords tab of each screen (taking away defaults or adding more);
  Passwords opened on an agent screen opens unlocked, showing (and
  filling, `op`, Chrome) only the granted items; both editors with the
  kit's search language and filters, selecting many matches to grant or
  revoke at once.

- **Move windows to and from Agent Screens** (Ben, 2026-10-31):
  "everywhere we have menu options to move something to a
  display-workspace there should also be a section in that menu to move
  it to an agent-screen (new or existing) and when in the agent screens
  app next to take control there should be an option to move all windows
  in the agent screen to a user display-workspace". So: the window tab's
  menu, Deskbar's app menu, the workspace heading's menu, Tracker's and
  Terminal's tab menus (wherever Move to Workspace is) gain an Agent
  Screens section (each running screen, and New Agent Screen); Agent
  Screens gets, beside Take Control, a way to move every window on the
  screen onto one of the person's display-workspaces. The program keeps
  running as it moves (its connection handed between the two
  app_servers, or the app reopened there with its saved state).

- **Terminal: light mode hard to read** (Ben, 2026-10-31): "in light
  mode my terminal is really hard to read i'm not sure why or what's
  conflicting but can you figure out a way so that the terminal gets
  properly themed with colors that look beautiful and have high
  readability. make sure you test it with various terminal clis, tuis,
  claude code, and codex for every supported terminal theme light and
  dark and all terminal themes that come as part of the olo themes both
  the light and dark variants. every single one must look polished with
  excellent legibility and aesthetics." Seen in his screenshot (light):
  Codex's composer a mid-grey band with its placeholder and footer
  nearly invisible, its paths and dim text pale blue/grey on white, its
  added lines on a dark grey-green; Claude Code's diff on near-black
  green, its dim text and tips too faint. Likely causes to check: how
  programs learn the background (OSC 10/11/4 answers, `COLORFGBG`,
  `TERM`/`COLORTERM`), Codex and Claude Code picking dark styles
  (Claude Code's own theme setting), 256-colour greys and the bright
  colours chosen for dark backgrounds, faint (SGR 2) mixed too far
  towards the background, truecolor backgrounds a program blends from
  what it thinks the background is. So: answer the colour queries
  truly, a minimum contrast for text against its cell (as iTerm's and
  Ghostty's "minimum contrast"), faint kept legible, and each look's
  palettes tuned. Checked with ls, git diff/log, htop/btop, vim/nvim,
  less/man, tmux, fzf, lazygit, Claude Code and Codex in every terminal
  theme of every Olo look, light and dark, at 1× and 2×, as shots in
  the journal.
- ~~**olo-terminal uses far too much CPU**~~ (Ben, 2026-10-31; done:
  each row is compared with the cells it was drawn from and only rows
  that changed are rebuilt; a full-screen agent spinner frame 4.5 ms →
  1.2 ms whole, its grid 2.2 ms → 0.1 ms; the process 14% → under 4% of
  a core with ten windows and six spinners; hidden windows still draw
  once a second, under a millisecond): "same with
  olo-terminal: look at 4356 it is using wayu more cpu than it should".
  Measured (read-only, /proc and its `{"frames":true}`): 14% of a core on
  its UI thread, ~74 wakeups/s. Its window "olo — ◑ …" (a tab whose
  coding agent is working: the spinning title, the TUI's own spinner)
  drew 128 frames in 5 s, each ~4.5 ms (UI 2.5 ms, tessellating 1.4 ms,
  submitting 0.5 ms): the whole screen laid out and tessellated again for
  a spinner. A hidden window ("scraper") still draws once a second at
  5.5 ms. Wanted: single digits at most. Redraw only what changed (keep
  each row's shapes from frame to frame; repaint the title/tab spinner
  without the screen), draw a spinner no faster than it turns, and make
  a hidden window's once-a-second frame cheap or skip it.

- **Tooltips for text that doesn't show in full** (Ben, 2026-10-31):
  "when hovering over anything in olo or any olo app where the text has
  been truncated or occluded or otherwise is not being displayed in full
  then there should be a tooltip that appears showing the full text".
  Everywhere: kit widgets (labels, list cells cut with "…", tab and strip
  titles, menu items, text fields scrolled sideways, buttons, Deskbar's
  rows and tray, the chat list's previews), app_server's window tabs, and
  text painted by hand; a part hidden under something else counts too.
  Best done once in the kit (where text is cut to fit, remember the full
  text and its rect; show it in the kit's tip, `olo_kit::tip`, after the
  usual hover pause) so every app gets it, and agents read the full text
  as the element's name. Then (Ben, same day): "if the user presses the
  super key while hovering over it then it should show the tooltip
  immediately without a delay" (Super while the pointer is on cut-short
  text shows its tip at once; app_server's own text, window tabs, too).

- ~~**Bug: after an upgrade, an auto-tiled window under Deskbar**~~ (Ben,
  2026-10-30; fixed): "after an olo hot upgrade i sometimes see windows, e.g.
  terminal, in auto tile mode opens back up such that it extends further
  horizontally beyond the deskbar and is under it. if i click to another
  window then back it adjust it to fit properly." The tiles stop beside
  Deskbar's bar on their display, and after an upgrade that bar came
  late or on the wrong display: a display coming back wasn't told to
  Deskbar until the power tick happened to (up to 5 s), and Deskbar's
  bars, reopened by name before the kit knew the displays, all went on
  the main display. So the tiles were laid out without Deskbar there.
  Now app_server tells of a display as it comes, the kit opens a layer
  naming a display once that display is known, and app_server keeps a
  layer that comes before any display instead of closing it (Deskbar
  and the desktop quit over that in a session whose displays come
  late). Reproduced headless (three displays coming late, a Terminal and
  a Tracker group tiled: wide every upgrade, until 5 s on or for good);
  with the fixes every upgrade brings them back beside Deskbar.
- ~~**Bug: after an upgrade, ⌘Space's palette opens on the first display**~~
  (Ben, 2026-10-30; fixed): "every time after a hot upgrade the cmd+space
  pallette seems to open on d0". The resident palette was first started
  by ⌘Space as `olo-palette --toggle`, and an upgrade re-executes an app
  with its first arguments: it opened itself as it came back, before
  any window had the focus again, so on the first display. Restored
  now, it leaves its first launch's asks alone (`save_state`). Seen
  headless after an upgrade (palette on D0 by itself, the focused window
  on D1); with the fix nothing opens, and ⌘Space opens it on D1.
- ~~**Bug: an app at a whole core after an upgrade**~~ (Ben, 2026-10-30; fixed):
  "the olo supervisor has detected that olo-passwords looks stuck using
  98% of a core for over 5mins". Its window was back on a hidden
  workspace at another size: the kit's swapchain was to be made to fit
  1.5 s on, but only at a frame, and a hidden window's frame callback
  never came, so the loop was woken for it at once, again and again.
  Now that waits for the frame (`next_timeout`), and a window with
  nothing to draw into isn't asked again at once either. Reproduced
  headless (a window shrunk, then sent to another workspace: 99% before,
  0% after).
- ~~**Bug: scrolling a window that isn't active**~~ (Ben, 2026-10-30; fixed, to try after the upgrade):
  "some/most apps don't let me scroll their scrollable areas unless
  they're active. all scrollable areas should be scrollable even if
  their window isn't the active one". The wheel and the touchpad scroll
  whatever scrollable area is under the pointer, in any window, active
  or not (and without making it active), as macOS and BeOS do: Olo's
  kit apps, other Wayland programs (Chrome, GTK, Qt), app_server's own
  views. Find where it's lost (app_server's pointer axis routing, the
  kit taking axis events only with keyboard focus, egui's hover state
  in an unfocused window) and check it for every kit scroll view (lists,
  text views, cards, Tracker, Terminal, Activity Monitor, Preferences)
  at every scale.
  Ben, live: Activity Monitor scrolled, Agent Screens and Token Pool
  didn't, until a click on that display; StyledEdit didn't while Slack
  did; then an inactive Tracker on the active display didn't. Cause:
  windows that hadn't drawn lately (any display) were taken for not
  shown after each frame with nothing new (the DRM backend counted
  nothing as presented), so Smithay sent their frame callbacks once a
  second, and Olo's apps scroll in their frames; Activity Monitor
  redraws on its own and Chromium draws on its own clock. Now only a
  drawn frame changes which screen shows a window. Also: a motion gave
  the pointer to the surface under where it was before the motion.
  Headless sessions render every frame whole, so they couldn't show it:
  checked there that every kit view scrolls inactive (wheel and
  touchpad, on either display, mixed scales).
- ~~**Bug: Quick Look's full screen spans displays**~~ (Ben, 2026-10-30; fixed):
  "there's a bug with quick look when i have multiple displays i hit
  space and correctly see the quick look preview then i hit f and it
  went to a different display well actually it kinda spanned displays
  -- it should fullscreen it on only the one active display". With
  several displays, Quick Look (Space) shows on the right one, but F
  (full screen) moves it to another or stretches it across displays.
  It must fill only the display it's on (the active one, where the
  person is looking), and come back from full screen to where it was.
  Find the cause (what Quick Look asks for, and how app_server takes a
  full-screen or zoom request with more than one display) and test it
  with a headless two-display session at every scale.
  Cause: app_server took the size from the display the window touched
  first but filled the space's first display with it, and the other
  displays still drew the window where it was, now display-sized, over
  their edge. Now it fills the display the window is on (else the
  pointer's), and only that one draws it. Before and after journalled
  (two headless displays; 1×, 1.5×, 2×).
- ~~**Terminal: coding agents keep their tab's title current**~~ (Ben,
  2026-10-30; done: HACKING, Terminal ▸ Coding agents' tab titles): "terminal app should also make available to claude code
  and codex coding agents a tool to update the terminal title and inject
  instructions for them to keep the terminal title updated at all times
  to ensure it reflects the active work". A Claude Code or Codex started
  in a Terminal tab gets a tool to set that tab's title (the tab only,
  not the window's or another tab's; through the agents' own tool
  mechanisms, an MCP server of Terminal's given to the agent the way the
  group context is, not by editing the person's files beyond what that
  needs), and instructions in its context to keep the title saying what
  it is working on now, updated as the work changes. The title the agent
  sets is shown as a tab title the person set is (and the person's own
  rename wins until they clear it); readable and settable by agents
  outside the tab too (Terminal's widget tree and commands). A setting
  in Preferences ▸ Terminal, beside the group context's.
  Done so: `set_tab_title` from an MCP server of Terminal's
  (`olo-terminal --mcp`), named in Claude Code's `.claude.json` (and
  allowed in its settings) and Codex's `config.toml` while the setting
  is on; the instructions come from the same hook as the group context
  (at the start, and a reminder line with each prompt). There is no
  person's rename of a single tab to lose to it (the person names
  groups); outside the tab, `olo-terminal --set-tab-title SHELL TITLE`.
  Tested with Claude Code 2.1.314 (`claude mcp list`: connected) and
  Codex 0.166 (`codex mcp list`), and the tool driven from a tab.
- ~~**Terminal: "Tile All Tabs", a row per group**~~ (Ben, 2026-10-30):
  "change how the show all tabs option in terminal works. first, rename
  it to "Tile All Tabs" and then i want you to make it so that each group
  gets a row and then each tab within that group is a col within that
  row." The command is renamed Tile All Tabs (menu, keys, agents'
  command id kept working under its old name too); it lays the window's
  tabs out as a grid where each group is a row (in the strip's order,
  its name and colour heading it) and each of its tabs a column in it,
  ungrouped tabs a row of their own; rows share the height and each
  row's columns its width, every pane live and typed into as before,
  and the same command (or picking a pane) goes back to one tab. Done
  (HACKING, Terminal ▸ Tile All Tabs; the command id stays `tile-tabs`,
  ⌘⇧D); with no groups, the grid as before.
- ~~**Terminal: Tile All Tabs, choosing which groups by their headers**~~
  (Ben, 2026-10-30): "make it so that in terminal in tile all mode, if i
  left click on a specific group header it actually makes that group
  selected (similar visual style to a selected workspace in deskbar) and
  then when it is selected it only tiles the tabs for that group (using
  a full grid not just one row). if i left click it again then it should
  go back to showing all the groups. and if i left click another group it
  should add that group too as a second row and so then e.g. each group
  gets half of it (and if you were to select all groups then basically
  it'd just be like none were selected and you'd see them all). and if i
  right click on a group header then i should get the context menu that i
  currently get with a left click." In Tile All Tabs, a click on a group's
  header selects it (lit as Deskbar's showing workspace is, in the
  group's colour) and toggles it: with one selected only its tabs tile,
  as a full grid; with several, a row each, sharing the height; none (or
  all) selected, every group shows. The header's right button opens the
  menu its left button opens now. Agents read which are selected and
  choose through the headers. Done (the kit's `TabStrip::group_selection`,
  for the side strip too; the chosen groups kept with the window).
- ~~**Terminal: Tile All Tabs, rows' heights dragged**~~ (Ben, 2026-10-30; done: HACKING, Terminal ▸ Tile All Tabs):
  "in terminal tile mode with multiple groups let me drag the group
  divider/header up and down to give a group more or less vertical space
  relative to the other groups. if i right click the divider/group
  header then there should be an option to reset group heights (if they
  have been changed)". In Tile All Tabs with several groups' rows, the
  divider between two rows (its group's header) drags up and down, the
  rows on either side growing and shrinking (each down to a least height
  that still shows a line of text), the panes reflowing live within a
  frame; the shares are kept per group (by its name, so a row keeps its
  share when others come and go), saved with the window and restored
  after a reload. The header's context menu gets Reset Row Heights
  (enabled once they've been changed) to share the height evenly again.
  Agents can read the shares and set them (the strip's info, and a
  command). Done: each divider is a `Splitter` ("Row Height: GROUP",
  its share as its value) agents drag; Reset Row Heights is a command
  (`tile-reset-heights`). Found on the way and fixed: after any popup
  menu closed (Escape or an item chosen), the window didn't get the
  keyboard back (app_server couldn't find a gone popup's window, and
  smithay's grab ignored the focus change); and a click on Deskbar went
  through to the desktop under it, which took the keyboard from the
  window being typed into (Deskbar's menus too).
- ~~**Terminal: tell coding agents about the other repos in their tab
  group**~~ (Ben, 2026-10-30): "add a preferences -> terminal setting for
  injecting context to claude code and codex coding agents to tell them
  about worktrees/repos that are in the same terminal group. useful when
  using a group per feature and a feature spans repos b/c that way the
  coding agent knows about all the repos. it shouldn't make assumptions
  about what they are, just add a block to the coding agent context that
  it is part of a terminal group named '...' and inside that group are
  the following repos (including its own): ...". Preferences ▸ Terminal
  gets "Tell coding agents about the repositories in their tab group"
  (off by default). When on, a Claude Code or Codex started in a tab of
  a named group gets a block in its context saying only that: it is in
  Terminal group "NAME", and the group's tabs are in these repositories
  and worktrees (each one's path, with its branch; its own marked),
  nothing assumed about what they are for. Given through the agents'
  own context mechanisms without editing the person's files (Claude
  Code: `--append-system-prompt`, or a session-start hook in a settings
  layer of Olo's; Codex: its `-c` instructions or developer message),
  kept current as tabs join or leave the group (on the agent's next
  turn where it can, else its next start), and readable by agents
  (Terminal's widget tree says it's on). Done (HACKING, Terminal ▸
  Coding agents' context) through the agents' hooks, which both take a
  hook's output as context (`SessionStart`, and `UserPromptSubmit` when
  the group changed): the one place it can't avoid the person's files,
  so the hook is put in Claude Code's `settings.json` and Codex's
  `hooks.json` only while the setting is on, everything else in them
  left as it was, and taken out when it's off. Codex asks once to trust
  it (`/hooks`). Told only when the group's tabs are in more than one
  place. (Tested with a sandboxed Codex 0.166, which ran it and passed
  the session's folder, and with Terminal's own groups.)
- ~~**Deskbar: every display's workspaces on every Deskbar; the display
  in the tray**~~ (Ben, 2026-10-30): "the deskbar on all displays should
  show the display-workspaces for all displays not just the current
  display. and, let's also show the display id/number in the deskbar
  tray with like a little monitor icon and the display number in it, and
  clicking on it gives you the option to switch to a different display".
  This replaces the earlier rule (each Deskbar listing only its own
  display's workspaces): each Deskbar lists every display's workspaces
  and their apps, its own display's first ("D0 Workspace 1" …), the
  others' as they are on their displays (clicking one goes there, on
  that display). In the tray, a small monitor icon with the display's
  number in it (drawn at every scale, in the tray's style); clicking it
  lists the displays (their names and numbers, the pointer's marked) and
  choosing one moves the pointer and focus there (its front window, else
  its desktop). Agents read the display's number and choose through the
  menu. Done so (HACKING, Deskbar and multiple displays): `Request::FocusDisplay` goes to a
  display; the icon shows with more than one display, its menu listing
  each by number and name.
- ~~**Bug: upgrades still move windows; restore an exact snapshot**~~
  (Ben, 2026-10-30): "when I upgrade it is still messing up my windows
  and moving them around on the displays and the sizes and everything. i
  think we need a more reliable approach to ensuring that windows stay
  where they are on upgrades: we need to be able to save the current
  state/location/size/mode/etc of every display-workspace then do the
  upgrade then restore everything back to that exact state instead of
  trying to rely on the logic to place everything back where it was
  before." Before the upgrade, app_server writes one snapshot of the
  whole desktop: every display (its mode, scale, position) and each of
  its workspaces (its windowing mode, auto-tiling tree, what shows), and
  every window exactly (display, workspace, frame, content size, zoomed,
  rolled, minimized, tile and group with its tab order, stacking order,
  focus). After it, as each window comes back it is put exactly there
  and asked for exactly that size, with nothing else placing it
  (auto-tiling, cascades, groups' placing and fitting held off until all
  have come back or a deadline passes); whatever still differs once
  they're back is set from the snapshot again, and the result is
  compared with the snapshot and any difference logged (and checked by
  `tools/dev/upgrade-check` with several displays, every windowing
  mode, groups and auto-tiling). Done (HACKING, upgrade-check): the log
  of Ben's 09:18 upgrade showed the windows reconnecting before DRM had
  found the displays (0.4–0.9 s later); reproduced headless
  (`OLO_HEADLESS_LATE_MS`): windows on a display not found yet were put
  on another, and auto-tiled ones swapped tiles (the trees of displays
  not found yet were dropped and made afresh). Now a returning window
  waits for its display, those displays' trees are kept, and what came
  back is checked against what was saved and put back where it differs
  (logged). Two displays, both auto-tiled, a floating window and one on
  another workspace: 6 upgrades and 2 reloads unchanged (they swapped
  in 1 of 2 before), and an ordinary session's 3 unchanged.
- ~~**Move to Workspace: every display's workspaces, from a window's tab
  and from Deskbar's apps**~~ (Ben, 2026-10-30): "when right clicking a
  window title bar, under move to workspace it only shows the current
  display's workspaces. it should show all the workspaces for all the
  displays as targets to move to just like in the deskbar when you right
  click a workspace group title and get the option to move all windows
  to ->. and, speaking of the deskbar, when right clicking a running app
  in the deskbar all the move window options should be available in
  there too for that app." The tab's menu's Move to Workspace lists each
  display's workspaces ("D0 Workspace 1" …, as Deskbar names them; the
  window's own display first), moving the window to that display and
  workspace; an app's menu in Deskbar gets the same moves for all its
  windows (Move to Workspace ▸ every display's, Move to Display, and the
  window moves the tab's menu has), done with `MoveToWorkspace` and the
  display moves app_server already has. Done so (HACKING, "The window
  menu" and Workspaces): the tab's Move to Workspace lists every
  display's, left to right as Deskbar's heading menus do, its own
  display's with their keys and a check; `MoveToWorkspace` takes an
  `output` (to the same place there, tiled or zoomed as it was, hidden
  if that workspace isn't showing); an app's Deskbar menu has Move (All)
  to Workspace ▸ every display's and (All) to the Next / Previous
  Display. And a bug found doing it: after the tab's menu was put away
  with Escape, the next click (Deskbar's, say) went nowhere. The menu
  asked for its grab while the right button was still down, with no
  press of its own to name, and was refused, so it ran without one and
  its clear layer outlived it, taking that click; it now takes the
  pointer from the press, and a menu's window gets a frame as the menu
  goes (it draws on frames, which a still screen doesn't make).
- ~~**A group taller than its display, from a member's least size**~~
  (found by wm-fuzz, seed 3 steps 90 and 118, two displays 1600×1000
  and 1280×800 at 2×; also seed 1 step 229). Done: it wasn't a least
  size. wm-fuzz's step 90 sizes a Terminal group by hand 23 px taller
  than its 384-high display, which is the person's to do and stays. At
  step 118 a Tracker put in that group, asked for 247×407 but unable to
  be narrower than 250, drew 250×407; `keep_on_screen` took that as the
  app sizing itself and fitted the whole of it to the screen, height
  too, and the group followed. Now it fits only the sides the app chose
  (a side taken as asked is Olo's), so the group grows to 250 wide and
  stays as tall. wm-fuzz's Cmd+Tab and ⌘-drag-in checks also let a
  window coming into a group grow it to its least size, as group_window
  did (seed 3 step 244: a Tracker back from hidden into a Terminal group
  sized 151 high meanwhile; seed 2 step 50), and a tiled group may grow
  inside its tile area (seed 1 step 229: a right-hand tile grows
  leftwards). And the case the TODO was about, for tiles (seed 2 step
  134): a tiled Terminal grown to a Tracker's least size by grouping
  stayed that size after the Tracker was taken out, bigger than its
  tile, until a zoom round trip put it back. A tiled window or group now
  goes back to its tile's size as soon as the window that needed more
  leaves (`back_to_tile`); wm-fuzz lets it.
- ~~**Notifications: one of the same, not several**~~ (Ben, 2026-10-30):
  "add a preferences -> notifications setting, enabled by default, that
  dedupes notifications so that if we get >1 active notification from
  the same app with the same text/content then we just show the latest
  one and drop the earlier one". Preferences ▸ Notifications, on by
  default: a notification from an app arriving while one of the same app
  with the same title and body is still active (a banner showing, or
  unread in the history) replaces it: the new one shown (and kept), the
  earlier one taken away, as if it had been updated in place (its count
  in Deskbar's dot unchanged, its fade starting anew). Off: each kept,
  as now. Done (HACKING, Preferences ▸ Notifications): "Only the latest
  of the same notification", in the History box; pinned ones stay.
- ~~**Preferences ▸ Performance: a supervisor that keeps the desktop
  responsive**~~ (Ben, 2026-10-30; done: HACKING, The supervisor): "a new preferences -> performance app,
  and the first setting should be one to do smart process prioritization
  to ensure user responsiveness. the idea is that olo will run a
  supervisor/watchdog that monitors processes and ensures that stuff like
  compilers (e.g. rustc) or other background processes have low priotity
  and that higher priority is given to the active window app in the
  desktop, and absolute highest should be olo itself and the app server
  etc. so that responsiveness is always optimal. and the supervisor
  should watch for stuck processes that seem to be in a cpu spin loop for
  a long time, or zombie processes, or processes that appear to have
  memory leaks or are using such a large amount of memory that it is
  impacting overall system performance or stability. everything that
  this supervisor/watchdog does should be surfaced in activity monitor,
  and if there is an urgent thing like stuck processes or processes
  eating up all the memory then it should show the user a warning dialog
  with a button to kill the processes if they want and it should also
  have a button to go to the detail in activity monitor to see exactly
  what the process is doing and the evidence we have. there should also
  be an option to ignore a process so we don't keep getting warnings
  about it if it's not an issue. make sure in the preferences that any
  constants used in the hueristics/algorithm for detecting this stuff
  are tuneable, but also make sure that our detection algorithms are
  very smart/advanced don't just rely on one number/signal to detect
  these things. this feature should default to off for now, and there
  should be a tray icon to toggle it and right clicking the tray icon
  should have options to go to details in activity monitor."
  - **Where it runs**: a supervisor of olo-session's (a thread, or a
    small `olo-supervisor` it starts and keeps), off by default
    (Preferences ▸ Performance, a new panel, and a tray icon in Deskbar:
    a click turns it on or off; its menu: Show in Activity Monitor,
    what it has done lately, Performance Preferences…). Only the
    person's own processes unless Activity Monitor's full access is
    given; nothing it changes outlives it (priorities put back when
    it's turned off or exits).
  - **Priorities**: Olo itself highest (app_server, olo-session,
    wlproxy, Deskbar, the portal, input), then the active window's app
    and what it started (by its process tree and its scope, `scope.rs`),
    then the other apps on screen, then background work lowest:
    builds and compilers (rustc, cargo, cc, ld, clang, go, javac, node
    bundlers…), indexers and batch jobs, known by name, by their parent
    (a terminal's job not in front) and by behaviour (long CPU-bound
    runs with no window). Through CPU weight and `nice`/`ioprio`
    (`SCHED_IDLE`/`SCHED_BATCH` for builds), or cgroup `cpu.weight` and
    `io.weight` on the apps' scopes where the session's systemd allows;
    following focus changes within a frame of them, without blocking
    anything.
  - **Watchdog, from several signals each, not one number**: a stuck
    spin loop (high CPU for long with no progress: no syscalls or I/O,
    the same stack or instruction pointer sampled, no window updates or
    an unresponsive window that doesn't answer pings); zombies (and the
    parent not reaping them, for how long); memory leaks (resident and
    anonymous memory growing steadily over a long window by a trend fit,
    not one jump, against its own history, with swap and page-fault
    rates); memory that threatens the system (its share of RAM, the
    system's memory pressure, PSI stalls, swap thrash, how close to the
    OOM killer). Each finding carries its evidence (the samples, the
    trend, the pressure) and a confidence.
  - **Tunable**: every threshold and window in those heuristics in
    Preferences ▸ Performance (with sensible defaults and a reset), and
    an ignore list (processes, by name or command).
  - **Surfaced**: everything it does and finds in Activity Monitor (a
    Supervisor section and each process's inspector: its priority and
    why, its findings and their evidence, over time); an urgent finding
    (stuck, eating memory) asks in an Olo alert naming the process and
    why, with Stop Process, Show in Activity Monitor and Ignore This
    Process. Agents read and drive all of it (the panel, the tray, the
    alert, Activity Monitor).
  - **Done so**: `olo-supervisor`, a service of the real session, idle
    until turned on. Priorities by nice, `SCHED_BATCH` and idle I/O on
    every thread (unprivileged: the session's processes are all in one
    systemd scope, so no cgroup weights; `SCHED_IDLE` can't be undone
    without privileges, so batch); the active window's app's tree and
    Olo's own never lowered. The watchdog's signals as above, short of
    stack sampling (needs ptrace) and window pings; every threshold in
    the panel. Activity Monitor: a Supervisor section and an inspector
    tab. Agents: `olo-supervisor --status --json`, `--on`, `--off`.
    Found and fixed on the way: an alert of a program started again
    was put where the last one had been (on a screen of another size,
    off centre) rather than in the middle; the kit's single-instance
    socket (and other per-display files) broke when `WAYLAND_DISPLAY`
    named its socket by full path; alerts with long button labels ran
    off their left edge (`WindowOptions::alert_buttons`).
- ~~**New windows always become the active window; an optional glow**~~
  (Ben, 2026-10-30): "please make sure that any time a new window opens
  for any reason it becomes the active window with focus. also, add a
  setting in preferences -> appearance to give new windows a subtle glow
  for n seconds (default off, and default seconds when on should be 5s).
  the glow needs to work with the active theme and day/night mode and
  look really great and be fairly subtle but enough to draw attention to
  the window." Every new toplevel (Olo's apps, other programs, dialogs
  and alerts, windows opened in the background by a program or a
  portal) is raised and given the keyboard focus as it first shows, on
  the display and workspace it opens on (switching to them if it opens
  elsewhere), checked across every way windows open (restored after an
  upgrade excepted: those come back as they were). Preferences ▸
  Appearance gets "Glow new windows for [5] seconds" (off by default; a
  number of seconds when on, 5 by default): app_server draws a soft
  outer glow around the new window's frame in the look's accent (its
  own for each look, and for day and night), fading out over the last
  second, drawn at device pixels at every scale (1× to 2×, 1.25× and
  1.75× too), costing nothing once it has gone, and said in the
  window's info for agents. Done (HACKING, "New windows"): focus held in
  every case tried (other programs and background launches, another
  workspace's app, auto-tiling, tab groups; wm-fuzz checks it each
  step); the glow checked at 1× and 2×, light and dark.
- ~~**Bug: Empty Trash from the desktop's Trash icon didn't empty it**~~
  (Ben, 2026-10-30): "right clicking on trash on desktop and choosing
  empty trash didn't seem to work so i opened trash (still saw the
  folder in there to delete) then did empty trash again from the
  tracker menu and it worked", then "i think it was b/c there was a
  confirmation dialog it opened that i didn't see. make sure all dialogs
  like that get centered on the screen and on the top/front layer".
  Done (HACKING, alerts): an alert with no parent (asked from the
  desktop, a menu, Deskbar) opened in the next cascade slot, the top
  left of a display, unseen; now it opens in the middle of the display
  the pointer is on (kept off Deskbar's column), in front and focused,
  and stays over its app's windows until answered. One with a parent
  still opens over it.
- ~~**Deskbar: an app's notification dot drains with its notifications**~~
  (Ben, 2026-10-30): first "the blue notifications dot for an app in the
  deskbar should fade like the notifications in notifications app as
  that apps notifications fade out (based on the settings) -- make it so
  that like when all of that apps notifications are shown with 100%
  visibility still then the blue dot is 100% and then as each individual
  notification starts to fade that takes the relevant strength out ...
  based on the fraction of that notification ... make sure there's a
  min/floor ... where it still looks good like a dot", then instead:
  "instead of changing the opacity based on the notifications that are
  fading let's have the amount of the blue dot that's filled in change,
  filling from the left to the right so it drains as the apps
  notifications drain". The dot is filled from its left edge to a
  fraction of its width: the mean of the app's notifications' strengths
  (`NotificationSettings::strength`, from the fade settings: each 1/n of
  it), so it is full while none has begun to fade and its right side
  empties as each fades; the empty part is the dot's outline (or a faint
  well) in the same colour, so it always reads as a dot, never a mark,
  with a floor to the fill where it still reads as one (chosen by eye,
  in every look and at every scale, its edge snapped to device pixels).
  With fading off, full. Updated as the notifications age (no faster
  than the fill can show a change), and the fraction in the entry's
  status for agents. Done (HACKING, Deskbar): floor 0.3, the well at
  28% of the dot's colour; checked at 1×, 1.5× and 2×, light and dark.
- ~~**Notifications preferences: banners, sounds and history for all apps
  at once**~~ (Ben, 2026-10-30): "the notifications preferences already
  have the ability to turn on/off banners/sounds/history for each app
  but there should also be an option to do it for all apps". Above the
  apps table in Preferences ▸ Notifications, an "All Apps" row with the
  same three checkboxes: checking or unchecking one sets that column for
  every app; each shows mixed (–) while the apps differ. Applies to apps
  seen later too only if the person asks for it (the default for new
  apps stays as now). Readable and settable by agents through the
  panel's widget tree. Done (HACKING, Notifications; the kit's
  `CheckBox::mixed`); the panel is taller for it (and the Deskbar box).
  Also fixed: a scroll view squeezed below 18 points crashed its app
  (the knob's least length passed the track's).
- ~~**Deskbar: the notification count beside the dot, as an option**~~
  (Ben, 2026-10-30): "a preferences option in notifications to also
  include the notification count next to the blue dot. off by default
  but enableable. make sure you qa visually at all display scales to
  ensure it looks good and fits well". Preferences ▸ Notifications gets
  "Show the number of notifications in Deskbar" (off by default, in
  `NotificationSettings`); when on, the app's entry shows its count of
  notifications beside the dot, in the dot's colour and fading with it,
  in the kit's small type, fitting the entry at every width (a long
  name is cut before the count is), beside the dot as it drains,
  shaking with the dot when one
  arrives, and in the entry's status for agents. Checked by eye and with
  `tsession review` at 1×, 1.25×, 1.5×, 1.75× and 2×, in light, dark and
  night, with counts of 1, 9, 10 and 99+. Done (HACKING, Deskbar):
  Preferences ▸ Notifications ▸ Deskbar; checked with 2 and 99+ at 1×,
  1.5× and 2×, light and dark.
- ~~**Notifications: one click on a card acts**~~ (Ben, 2026-10-29; done:
  HACKING, Notifications' Cards): "in
  notifications app card view, i should only need a single click of a
  notification card to activate / go to the action".
- ~~**Instant updates when files change**~~ (Ben, 2026-10-29, bug; done:
  HACKING, rule 7 "Stay current without polling", `olo_kit::watch`): "currently
  when something changes on the desktop it doesn't reflect immediately,
  e.g. if i empty the trash we don't see the icon change for a few
  seconds. we should use event based notifications with optimized linux
  file watching or whatever so that this happens instantly. and that same
  principle should apply to everything in tracker and everywhere in olo:
  we want event based instant updates to any file changes so that olo
  always reflects the latest state. and it must all be done super
  efficiently so that it uses minimal resources." Every view of files
  (Tracker's windows and desktop, the Trash icon, Disk Usage, file
  panels, Viewer and StyledEdit's open documents, the palette's index)
  follows the kernel's change events (inotify / fanotify), at once,
  with no polling and little memory.
- ~~**Keyboard shortcut profiles**~~ (Ben, 2026-10-29; done: HACKING,
  "Keyboard profiles"): Mac (the default), Windows and BeOS profiles in
  Preferences ▸ Keyboard ▸ Shortcuts. Each has every shortcut,
  app_server's and the apps' menu keys, with the command key and its
  key behaviour. A change makes "Mac (Customized)", Save As… and Delete
  keep or drop named ones, and they go in Backup with the settings.
  ⇧⌘-arrows select text again (Mac tiling is Rectangle's ⌃⌥), and the
  shortcuts sheet comes up on ⇧⌘/ (Win+Shift+/, Alt+Shift+/).
  Not yet: keys for apps that aren't Olo's (Chrome's own).
- ~~**Twitcher: choose with the pointer**~~ (Ben, 2026-10-29; done:
  HACKING, Twitcher): while ⌘ (or ⌥) is still held, the pointer over a
  cell chooses it and a click there ends the switching on it.
- ~~**Terminal resizing itself**~~ (Ben, 2026-10-29; done): selecting
  text with Auto-Tiling on squeezed the Terminal (wl-copy's 1x1 window,
  made because the session's proxy predates data-control, took a tile;
  now kept out of sight); a window zoomed vertically keeps its height
  as its text size changes (app_server says which sides it holds, the
  kit keeps them); hidden windows no longer jump to a new text size
  when they come back, and covered ones take it at once.
- ~~**olo-wlproxy upgrades only at login**~~ (done: HACKING, "Upgrading
  the proxy"): an in-place upgrade kept the running proxy (restarting
  it would close every outside program's windows), so a protocol added
  to it (data-control was) reached programs only after logging in
  again. Now an upgrade starts the new proxy on a fresh socket put in
  the old one's place: new programs get it at once, and the old proxy
  keeps the programs it has until they close (then exits, from this
  version on). Carrying the running programs' state over into the new
  proxy (re-executing with every object, buffer and descriptor) was
  weighed and left: a mistake there closes every outside window.

- ~~**Open in Tracker, everywhere it makes sense**~~ (Ben, 2026-10-29;
  done: HACKING, Open in Tracker): `olo_kit::tracker` and Tracker's
  `--reveal`; Terminal's tabs and menus, StyledEdit, the Viewer,
  DiskUsage, DriveSetup, Preferences' backups, the palette (⌘↩ on a
  file), every window's yellow tab (`Cx::set_path`) and Tracker's Find
  results (Open Parent).
- ~~**Screenshots: the saved file's path on the clipboard**~~ (Ben,
  2026-10-29; done: HACKING, Screenshot and `olo_kit::clipboard`): Save,
  ⌘⇧3, Print Screen and ⌘⇧4 all put the file's path on the clipboard.
- ~~**Quick Look: Markdown rendered nicely**~~ (Ben, 2026-10-29; done:
  HACKING, `crates/preview`'s formatted documents, `rich.rs`): Quick
  Look's preview of Markdown files should be much more polished than it
  is now. Now a set page: a measure, a 20-point line, headings with room
  and rules, hanging indents, task boxes, alerts, code coloured by its
  language, tables lined up as written, the document's own pictures,
  links that open (and `#anchors` that scroll), laid out once and drawn
  only where seen.
- ~~**Notifications: pin a notification**~~ (Ben, 2026-10-29; done:
  HACKING, Notifications' Pin and Snooze): a pinned notification stays
  put: it doesn't fade out over time and isn't dismissed by dismissing
  all. Pinned, a banner stays on screen until closed (and after a
  restart), and the history keeps it at full strength.
- ~~**Notifications: snooze a notification**~~ (Ben, 2026-10-29; done:
  HACKING, Notifications' Pin and Snooze): snoozing puts it away and
  brings it back after a while (15 minutes, an hour, three hours or nine
  tomorrow morning; `--snooze ID MINUTES` for any time), as new.
- ~~**Always show the tab bar, even with one tab**~~ (Ben; done):
  Terminal's `TabStrip` shows with one tab. Window groups needed nothing:
  every window shows its yellow tab, as in BeOS, and a lone window's tab
  is its group's bar with one tab in it (decided rather than asked:
  another strip over a single window would be a second title).
- ~~**Quick Look: scroll, filter, search and sort tables**~~ (Ben,
  2026-10-29; done: HACKING, `crates/preview`'s spreadsheets, and the
  kit's `ColumnListView` scrolling sideways): "in quick look it shows a csv with rows and cols which is
  great but it doesn't let me scroll horizontally to see all the cols we
  need to support horizontal scroll and also add the ability to
  filter/search/sort in there too". CSV, TSV and spreadsheets: scroll
  sideways to every column; search across the cells; filter rows; sort
  by a column (header click, again to reverse), on a worker for big
  files.
- ~~**Window tabs: titles of unfocused windows update late**~~ (Ben,
  2026-10-29, bug; done: HACKING, `Cx::set_title`): "yellow tabs that are not focused don't have their
  titles (specifically i'm looking at agent status icons) updated very
  frequently it seems like they do update but not very often". A window's
  title (a Terminal's agent status icon in it) must show in its tab the
  moment it changes, focused or not. Cause: app_server lets a window
  whose content isn't shown (behind another in its tab group, covered)
  draw once a second, and Terminal set its window's title only while
  drawing. Now it sets it the moment a tab is retitled: a window behind
  another in its group showed 4 of 30 titles in 3 s, now all 30 (1×,
  1.5×, 2×).
- ~~**Terminal: ⌘-click opens any URL or file, reliably**~~ (Ben,
  2026-10-29, bug; done: HACKING, Terminal's Links): "in terminal we should be able to super+click any
  url and it should open in the default browser, or any local file and
  it should open in tracker. it should be able to figure out relative
  paths too based on the cwd in the terminal"; and, told it has them
  (`links.rs`: ⌘-hover underlines, ⌘-click opens; URLs by pattern, paths
  that exist relative to the foreground program's directory): "if it
  already has them then it doesn't reliably work". Find every way it
  misses (programs reporting the mouse, as Claude Code does; links
  wrapped over lines; paths in a TUI's boxes or quotes; the working
  directory of a program that changed its own; the key under each
  keyboard profile) and fix them; files open in Tracker, shown.
  Found and fixed: a ⌘-click on a Terminal that wasn't focused only
  focused it (the link was looked up on frames already knowing ⌘); a
  click used the previous frame's link; long URLs Claude Code wraps
  itself opened cut short; a Ctrl-click went to programs taking the
  mouse; files opened in their app. Checked with a real Claude Code
  reply: a URL, a long one it wrapped (from either piece), `path:line`
  and a plain path, at 1×, 1.5× and 2×.
- ~~**Terminal: several groups, tabs moved between them, groups
  reordered**~~ (Ben, 2026-10-29; done: HACKING, Terminal's Groups and
  the kit's `TabStrip`): "in terminal make sure that i can
  actually create multiple groups, move tabs between groups, reorder
  groups, and so on. because last time i tried (which was before your git
  worktree stuff) it didn't work." Today a group is a whole window (one
  name and colour, Tabs ▸ Group), and a tab can only go to a new window.
  Make groups runs of tabs in a window's strip, as a browser's: each led
  by its chip (name, colour); a tab's menu has Add to Group ▸ (New
  Group…, each group) and Remove from Group; dragging a tab into a run
  joins it, out of it leaves it; dragging a chip moves its whole group
  (reordering groups); a chip's menu renames, recolours, saves, ungroups,
  closes or moves the group to a new window; tabs and groups dragged
  onto another Terminal window's strip move there. In the kit's
  `TabStrip`, for other apps; repositories' own groups (repos.rs),
  saved groups, `save_state` and agents (the tree) follow. Done as
  written, except dragging onto another window's strip (next item);
  meanwhile a tab's Move to Window ▸ moves it to another window. Also
  done from the next item: a tab or group dragged out of the strip
  becomes a window under the pointer. Checked by drags in a test
  session, at 1×–2× (and 1.25×, 1.75×), light and dark, along the top
  and down the side, and through an upgrade.
- ~~**Drag a window into another's tab bar to make it a tab**~~ (Ben,
  2026-10-29; done: HACKING, the kit's `TabStrip` and app_server's window
  moves): "when there are apps with multiple windows and they also
  support tabs we should be able to drag the window back into the tab
  bar area to have it go back to being a tab instead of a window, and
  the opposite we should be able to drag a tab out into being a window",
  and "we should be able to drag a group out to make that whole group a
  new window with those tabs". Done: a tab or group out into a window
  (the kit's `TearOff`, `TearOffGroup`, `WindowOptions::under_pointer`).
  To do: a window (by its tab, which app_server moves) let go over
  another window of the same app's tab strip joins it there as a tab
  (or its tabs, its groups kept), and a tab dragged out of one window's
  strip onto another's goes into it: app_server tells the app where a
  window of its own was let go (over which of its windows, where), the
  kit says where its strips are, and the strip shows where it would go
  while the window is over it.
  Done so: a tab or group pulled out of the strip is a window under the
  pointer that moves with it (`Cx::begin_move`); a window moved over
  another of its app's goes see-through and that window's strip marks
  where it would go; let go there, its tabs (and groups) go in, and it
  closes. Checked in a test session: a tab out and back, a lone window
  dropped by its tab, a group out and back after another group.
- ~~**Terminal's tabs and groups look like Deskbar's list**~~ (Ben,
  2026-10-29): "can you make the terminal tabs styled more like the olo
  deskpad tabs that style looks much nicer than the almost childish look
  of the tabs/groups in terminal right now." Down the side: groups as
  Deskbar's workspace headings (bold, a band, no pill), tabs as its rows
  (icon, title, the one showing in its band), quiet; along the top, the
  same restraint.
  Done so: down the side, Deskbar's rows on the panel's ground with a
  hairline under each, the one showing recessed, groups under small bold
  headings with a dot of their colour; along the top, chips are a dot
  and a quiet bold name instead of a pill. `tsession review` clean at
  1×–2× in every layout, light, dark and night looked at. Then (Ben,
  with a picture: "the pr icons seem to be a little bit too big, can we
  make them a touch smaller and just generally polish up how that
  vertical tab looks"): a tab's pull request mark is as big as the text
  (12 pt, its lines whole pixels at every scale) in the close box's
  place, which takes it over under the pointer, and each group's
  colour bar stops short of the next's.
- ~~**Auto-tiling finds an arrangement where every window fits**~~ (Ben,
  2026-10-29): "can you work on the auto tiling algorithm it currently
  sometimes overlaps windows that would have all fit if the arrangement
  was different we are not finding the optimal solution right now".
  Done so (HACKING, auto-tiling): where the tiles can't give every
  window its least size, `autofit::refit` turns the fewest splits, else
  searches every slicing arrangement (shape curves: any grouping up to
  8 tiles, runs of their order beyond) for one where all fit, else
  takes the one that overlaps least. Five windows on 1440×900 that
  overlapped three ways now all fit.
- ~~**Show All Tabs: each pane's head in its group's colour**~~ (Ben,
  2026-10-29): "can you make the tile headers follow their groups color
  scheme so we can visually see whats what".
  Done so: a pane's head is tinted with its group's colour, a 2-point
  line of it across the top.
- ~~**Tracker: selection as in modern file managers**~~ (Ben, 2026-10-29):
  "in tracker i should be able to click a file/item, hold down shift then
  click above/below it to select all of them and any other common
  selection/movement/action behaviors and patterns in modern file
  managers". Shift-click ranges, ⌘-click to add or take one away,
  ⇧-arrows to extend, ⌘A, a rubber band in every view, type to select,
  and the rest Finder and Nautilus do.
  Done so: `ListState` has the selection every view shares (click,
  ⌘/⇧/⌘⇧-click, ⇧ with the arrows, Page keys, Home and End growing and
  shrinking from the anchor, bands that add with ⇧ and flip with ⌘ and
  scroll on), `widgets::type_select` for typing (Find, the file panel
  and every Tracker view); a plain click on one of several leaves it
  alone; Enter on several opens them all; Open Parent selects the folder
  it came from. Checked by driving each view in a test session.
- ~~**StyledEdit, much more featureful**~~ (done 2026-10-30: every feature below; HACKING, "StyledEdit") (Ben, 2026-10-29, after
  everything else): "please improve StyledEdit to be much more featureful
  -- it doesn't need all the bells and whistles of, say, modern Word or
  Excel, but maybe something like ClarisWorks / Gobe Productive". After
  Gobe Productive (BeOS's own office suite) and ClarisWorks: a word
  processor first (paragraph styles, rulers with tabs and indents,
  lists, tables, pictures, headers and footers, page view and printing
  to PDF, find and replace, spelling, word count), opening and saving
  the formats people have (RTF, Markdown, DOCX, ODT, HTML) natively.
  And (Ben): "as you build out new widgets/components/patterns etc for
  anything that is reusable they should go into the kit so we can use in
  other apps -- use it as an opportunity to enhance the kit too": rulers,
  style menus, tables, page views and the like are kit widgets,
  documented in HACKING.
  Done so far (2026-10-30): paragraph styles (Title, Heading 1–3,
  Quote, Code, Body), bulleted and numbered lists that nest and number
  themselves, indent levels and alignment per paragraph (the Paragraph
  menu; Tab, ⇧Tab, Return and Backspace in lists), a word count, all
  saved in the file's attributes; the kit's editor sets each paragraph's
  indents, spacing and marker (`editor::Paragraph`); File ▸ Import… for
  Markdown, Word and OpenDocument, and Export as any of those or a web
  page (`olo-richtext`, links kept; checked in LibreOffice); spelling as
  you type with suggestions (the kit's `spell`, Hunspell dictionaries);
  printing and PDF (`olo-print`: the Print panel, CUPS over IPP, PDF by
  krilla, pages laid out as the editor lays them out); RTF in and out
  (`olo-richtext`'s `rtf.rs`, checked both ways with LibreOffice);
  tables in `olo-richtext` (Word, OpenDocument and RTF, read and written,
  checked in LibreOffice), and Quick Look reading Word, OpenDocument and
  now RTF through it; tables in the editor (the kit editor's rows, the
  Table menu, in and out of every format, printed), column widths
  dragged by their rules; a ruler (the kit's `ruler`: indents and tab
  stops, left, centre, right and decimal, which epaint now lays out),
  kept in every format; headers and footers (Document ▸ Header &
  Footer…, left, centre and right parts with the page number and count,
  printed, and kept in Word, OpenDocument and RTF); pictures (characters
  in the text the kit's editor draws: Insert Picture…, pasted or
  dropped, sized from the Edit menu, kept beside the document, and in
  Word, OpenDocument, RTF, web pages, Markdown and print); Page View
  (sheets of the paper it prints on, breaking as printing does, headers
  and footers on them; the kit editor's `Options::pages`).
- ~~**Activity Monitor, revamped**~~ (Ben, 2026-10-29, at the end):
  "revamp activity monitor to include everything a nerd could ever want
  to know, with live views, drill downs, tuning settings and ability to
  take actions etc. it should look really slick / aesthetically
  pleasing, be extremely fast/performant/buttery smooth and extremely
  powerful. if needed it should just ask for elevated perms on startup to
  be able to show everything." Processes (tree, threads, open files and
  sockets, memory maps, environment, cgroups, I/O, priorities; signal,
  renice, pin to cores, limits), CPU (per core, frequencies, C-states,
  governors), memory and swap (zram, pressure, page cache), disks (per
  device and process I/O, latency, SMART), network (per interface and
  connection, per process), GPU, power and thermals (sensors, fans,
  battery), the kernel's pressure stall information and tunables; live
  graphs at 120 Hz, history, drill-down from any graph to who causes
  it. Read natively from /proc, /sys, netlink and the like (no ps, top
  or lsof); elevated access through Olo's polkit agent, asked once at
  start, for what needs it. Reusable graphs and inspectors go in the
  kit.
  Done (2026-10-30): the sections, the inspector, full access through
  pkexec (the sampler runs as root; only in the person's own session),
  idle states, NVMe health (read with full access: not yet seen on this
  machine as root), looking back from any graph; the kit's `Sidebar`,
  `dashboard` and `Graph`; graphs repaint a device pixel at a time (3.5%
  of a core for the Overview's six); every frame within 8.3 ms in the
  bench at 1×, 1.5× and 2× (the kit's lists got faster for it: one draw
  for a list's text, a per-frame budget for new cells); each process's
  network traffic (netlink `sock_diag` with TCP's byte counts, summed by
  the sockets' owners: Sent and Received columns, "Programs Using the
  Network"), and connections and interfaces read by netlink rather than
  /proc/net (done 2026-10-30, `netdiag.rs`).
- **Telegram, as an Olo app** (Ben, 2026-10-29): "please build an olo
  native telegram client that's an absolute pleasure to use and looks
  really great and supports all the core features of telegram". MTProto
  natively in Rust (a crate such as grammers; no Telegram Desktop or
  tdlib process): sign in with a phone code or QR and two-step
  password, the chat list with folders, pinned and archived chats,
  unread counts and search; messages with replies, forwards, edits,
  reactions, mentions, threads and topics; photos, video, voice notes,
  files and stickers (animated too); sending all of those, drafts and
  typing status; groups and channels; notifications through Olo's
  (with reply from the banner); contacts; secret storage of the session
  in Olo's keychain. Fast with thousands of chats (cached, streamed,
  never blocking), and Olo-looking: kit widgets, light, dark and night,
  every scale; agent-readable like every Olo app.
- **Screen sharing: an Olo portal backend** (Ben, 2026-10-29; the
  design researched by another agent, session 05de188d, kept here).
  Apps never capture the screen themselves on Wayland: Chrome, Slack,
  Discord, Firefox and OBS all call the xdg-desktop-portal ScreenCast
  API; a portal backend shows a picker, the compositor sends frames as a
  PipeWire stream, and the app reads it. Chrome and Electron apps share
  WebRTC's capture code, so this is a good portal backend and a good
  PipeWire stream, plus a few app quirks, nothing app-specific.
  - **Done** (2026-10-30, HACKING.md "Screen sharing"): olo-portal is
    the session's ScreenCast backend (olo-session runs it and routes the
    portal to it); the picker (screens, live thumbnails) opens over the
    asking window; streams are PipeWire nodes fed from dmabufs app_server
    draws on the GPU only when the screen changed (0.26 ms median of
    app_server's loop at 5K); shares survive an app_server reload and an
    upgrade waits for them; restore tokens and repeat requests answer
    without a picker. Checked end to end through xdg-desktop-portal with
    `olo-portal share-test`, and with Chrome 156's getDisplayMedia (a
    sandbox Chrome on a test session): Chrome's own dialog, Entire
    Screen, Olo's picker, Chrome's live preview, Share (the second
    session Chrome opens is answered by the restore token, no second
    picker), 5120×2880 at the 30 fps asked for, and Chrome's Stop
    sharing ends the stream. Deskbar's tray shows what is shared, with
    Stop Sharing (the app is told it closed). Windows as sources
    (2026-10-30): the picker's Windows, window frames at the client's
    size, renegotiated as it resizes, ended when it closes. App scopes
    (2026-10-30): programs started from desktop entries run in
    `app-olo-<id>-<pid>.scope`, so the portal knows which program asks
    (Chrome and Slack already put themselves in scopes of their own).
    Dmabufs passed through (2026-10-30): an app that takes them gets
    the GPU buffers app_server captures into, no CPU copy (checked with
    GStreamer's pipewiresrc and the portal's own reader, a screen and a
    window, resized, across an app_server reload); the rest get memory.
    Frames carry timestamps now, and a share outlives a reload (it used
    to end, taking app_server's stop as the window closing).
  - **Build** our own backend, `olo-portal`, started and supervised by
    olo-session: it owns `org.freedesktop.impl.portal.desktop.olo` over
    zbus (which we already use), shows the picker as a kit window,
    produces the PipeWire streams, and gets frames from app_server over
    ext-image-copy-capture with dmabufs (no CPU copy), on the direct
    socket so the protocol stays hidden from outside programs. Not
    inside the compositor (niri's, KWin's, Mutter's way): a stream in
    app_server drops every share in the middle of a call at each
    upgrade; out of process it outlives app_server (the portal
    reconnects and frames carry on), and libpipewire stays out of the
    compositor, which never waits on a client. COSMIC is the precedent
    (a Smithay compositor in Rust with exactly this split: pipewire-rs,
    gbm, ext-image-copy-capture). Not xdg-desktop-portal-wlr/-hyprland
    (not native, generic pickers), nor imitating Mutter's private D-Bus
    API so xdg-desktop-portal-gnome works, as niri does (GNOME's picker,
    startup races, someone else's private API). pipewire-rs links
    libpipewire in process, as Smithay links libinput: not shelling out;
    the pure-Rust `pipewire-native` can't stream video yet, so move to
    it later.
  - **For Chrome and Slack** (from WebRTC's capture code):
    - formats: offer BGRx and BGRA first (WebRTC swaps the bytes of RGB
      formats on the CPU); offer dmabuf with modifiers, and
      shared-memory (MemFd) buffers as a fallback (Chromium and some
      Electron apps showed black screens on niri until it added that);
      handle WebRTC renegotiating when a modifier fails;
    - stream metadata: WebRTC asks for the frame header, crop, cursor
      and damage metadata; send frames only when something changed;
      all three cursor modes (hidden, drawn in, sent as metadata);
    - window shares: renegotiate the stream size when a window resizes;
      draw the window whole even when it's behind another tab in a
      group (app_server already does this for agent captures);
    - Slack's repeated pickers: Chromium opens several portal sessions
      for one share (a Slack patch author counted three, so three
      pickers; Hyprland users use a wrapper script): answer a repeat
      request from the same app, a few seconds after a choice, with that
      choice;
    - which app is asking (done): the portal leaves the app id empty
      for unsandboxed apps unless they run in an `app-<id>-….scope`
      systemd unit. Chromium and Electron put themselves in one
      (`app-com.google.Chrome-<pid>.scope`, `app-slack-<pid>.scope`);
      Olo starts other programs from desktop entries in transient
      scopes (`StartTransientUnit` over D-Bus), which also gives the
      Inhibit, GlobalShortcuts and Notification portals their app ids;
    - restore tokens: save a choice for next time (portal interface
      version 4); OBS uses it, and Chrome the kind that lasts while the
      app runs;
    - Slack must run as a Wayland client (no Xwayland; Electron 38 and
      later do by default); its drawing overlay will still misbehave,
      but sharing works;
    - Zoom's native app offers Wayland sharing only when
      `XDG_CURRENT_DESKTOP=GNOME`, so it needs a launcher quirk for its
      process; Zoom in Chrome just works;
    - sharing a Chrome tab never reaches us, and Chrome can only share a
      tab's audio, not the whole system's: audio is out of scope.
  - **The Olo touches**: the picker, a kit dialog with Screens and
    Windows, live thumbnails and app icons, opened over the window that
    asked (agents read and drive it through the widget tree for free);
    while sharing, Deskbar shows what is shared and by which app, with
    Stop; a shared window gets an outline; Do Not Disturb turns on (the
    Notifications item); Passwords, the lock screen and the polkit
    prompt are blacked out of shares. Agent Screens as sources: the
    portal can connect to an agent screen's own app_server, to share an
    agent's screen into a Meet call; a working capture protocol also
    lets the Agent Screens viewer stop pulling raw-pixel screenshots
    eight times a second.
  - **Order**: 1. real capture in app_server, screens then windows
    (Smithay at our pinned rev already has window capture sources and
    the foreign-toplevel list; test headless with a small client);
    2. olo-portal with ScreenCast and the picker; 3. app scopes and
    answering repeat requests; 4. the Screenshot portal (cheap reuse of
    what we have); 5. RemoteDesktop over libei, for remote control
    (app_server already has a `libei` feature). Open question from the
    design: whether remote control (Zoom, Chrome Remote Desktop) matters
    soon, which decides how early the libei work comes.
  - Sources:
    [ScreenCast backend interface (v6)](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.ScreenCast.html);
    WebRTC's [screencast_portal.cc](https://chromium.googlesource.com/external/webrtc/+/master/modules/desktop_capture/linux/wayland/screencast_portal.cc)
    and [shared_screencast_stream.cc](https://chromium.googlesource.com/external/webrtc/+/master/modules/desktop_capture/linux/wayland/shared_screencast_stream.cc);
    [niri's Screencasting wiki](https://raw.githubusercontent.com/niri-wm/niri/main/docs/wiki/Screencasting.md)
    and [SHM-fallback notes](https://github.com/ljm-233/niri-portal-cast);
    [slack-on-wayland](https://github.com/hamolicious/slack-on-wayland),
    [Slack's Linux release notes](https://slack.com/release-notes/linux),
    [Electron's desktopCapturer](https://www.electronjs.org/docs/latest/api/desktop-capturer),
    [Electron PR #54550 (restore tokens)](https://github.com/electron/electron/pull/54550);
    [xdg-desktop-portal-cosmic](https://github.com/pop-os/xdg-desktop-portal-cosmic)
    ([Cargo.toml](https://raw.githubusercontent.com/pop-os/xdg-desktop-portal-cosmic/master/Cargo.toml));
    [pipewire-native](https://crates.io/crates/pipewire-native),
    [Collabora: PipeWire workshop 2025](https://www.collabora.com/news-and-blog/blog/2025/07/03/pipewire-workshop-2025-updates-video-transport-rust-bluetooth/);
    [Using portals with unsandboxed apps](https://blogs.gnome.org/ignapk/2025/06/04/using-portals-with-unsandboxed-apps/);
    [xdg-desktop-portal-wlr FAQ (Zoom)](https://github.com/emersion/xdg-desktop-portal-wlr/wiki/FAQ),
    [Zoom community thread](https://community.zoom.com/t5/Zoom-Meetings/Do-not-check-if-XDG-CURRENT-DESKTOP-is-gnome-for-wayland/m-p/108716).
- ~~**Token Pool: requests fail through it; make it truly transparent**~~
  (Ben, 2026-10-29): "please take a look at token pooler logs and figure
  out why requests started failing- i turned off token pooler for claude
  and then those same accounts worked fine going directly so something is
  broken with token pooler. please make sure that the proxy is truly
  transparent and acts exactly the same to claude code. actually what
  would be even better is if we can change it so that it doesn't put
  claude code into api mode with a custom endpoint but instead just makes
  the network data go through our local transparent proxy in a way where
  claude code just thinks it's using the normal subscription requests not
  a custom endpoint. same for codex." First the failures: the pool's
  logs (`olo-logs --print --program olo-pool`) against the same accounts
  used directly, request by request (headers, body, streaming, retries,
  token refresh), until the pool's requests are byte-for-byte what Claude
  Code and Codex send themselves. Then: no custom endpoint or API mode;
  Claude Code and Codex run as on a subscription, and their traffic to
  Anthropic's and OpenAI's hosts goes through a local transparent proxy
  (an `HTTPS_PROXY` they honour, or Olo's own network layer), which
  picks the account and passes everything else through untouched, with
  the logs and heatmap as now.
  Done (HACKING, "Token Pool"). The failures: account f919e666 on
  2026-10-29 13:05–13:13, requests getting no answer at all (status 0,
  28–116 s, no headers), while the quota reads hit 429 every minute: the
  pool read each account's usage report twice a minute, and was itself
  rate limited for it (now every five minutes, backing off on a 429).
  And the proxy: Claude Code and Codex on their own subscriptions with
  `HTTPS_PROXY` to the pool, which opens only their own connections to
  the two hosts (from /proc), serves the model requests from the pool
  and passes everything else through byte for byte; tested with the real
  clients (`tests/clients.rs`).
- ~~**Links from a shell: Chrome's "zygote" errors on the terminal**~~
  (Ben, 2026-10-30): `pulumi whoami`'s browser sign-in printed "Opening in
  existing browser session." then "Failed sending zygote boot message:
  Broken pipe" (it worked). xdg-open runs `olo-chrome URL`, which started
  a second Chrome only to hand the link to the running one, and Chrome
  prints those as that one goes. Now `olo-chrome` hands it over itself
  through Chrome's singleton socket (HACKING, "Google Chrome"): nothing
  printed, the page open in a millisecond, its window forward.
- ~~**Token Pool: Codex still not through the pool; its service's
  details and Restart; activity over time**~~ (Ben, 2026-10-30): "codex
  still doesn't seem to be going through token pooler, only claude is.
  you can look at the codex session i ahve running now and it's not going
  through the token pool. pls fix." · "you can just update the daemon it
  doesn't need to wait for non busy" · "also add a way to restart the
  daemons and see details about them in the token pool app itself too" ·
  "add a slick chart or something too that shows activity over time".
  The cause: the pool found Codex's daemon (started the day before, on
  the settings from before) but waited for it to be between turns, and
  Ben's Codex was in one agent turn for hours, all of it straight to
  OpenAI. Restarted his daemon then (Codex's own `daemon restart`, as
  the pool does; his window reconnected and carried on, now through the
  pool), and the pool now restarts a stale daemon at once. Done
  (HACKING, "Token Pool" ▸ The Clients window): Pool ▸ Clients (⌘L, or
  the Clients button) shows the activity over time (requests a minute
  by client over the last hour, six hours or day, kept a day), Codex's
  service (version, since, settings, where its connections go, its
  conversations) with Restart, and each Claude Code and Codex running
  and where its connections go; `olo-pool --clients [--json]` and
  `--restart-codex-service` for agents.
- ~~**Token Pool: restart Codex's service seamlessly, holding new requests**~~
  (Ben, 2026-10-30; done: HACKING, "Token Pool" ▸ Clients and Upgrades of
  the pool itself): "when token pool daemon thing restarts we recently
  changed it so that it doesn't wait for a quiet period in codex/claude.
  instead, let's do it smarter: make it so that when a restart is
  happening we still accept new requests but then we just have them wait
  as if they're waiting on the server and let the ones already in-flight
  finish, then seamlessly do the restart and then process the new ones
  that came in for real." A restart (the pool's settings changed under a
  running Codex service, or Restart in Clients) drains rather than cuts:
  - Requests already under way run to their end (their streams too).
  - Requests that come meanwhile are taken and held, the client seeing
    a request still being answered (no error, no retry storm), up to a
    limit of time after which they go ahead as before.
  - Once nothing is under way, the service restarts onto the pool's
    settings, and what was held goes on, in order, as if it had just
    been slow.
  Codex's windows talk to its service over its socket, and the service's
  own requests go out to OpenAI: holding at both ends (the pool's proxy
  for what reaches it; between Codex's windows and its service, a
  stand-in on its socket for the moment of the restart) so a turn going
  on is never cut and none is lost. Claude Code's requests, through the
  pool already, are held the same way while the pool itself restarts
  (an upgrade). Shown in Clients (Restarting: n under way, n waiting),
  for agents too.
  Done differently where that is better: a Codex service holds its
  turns' state, so restarting it in the middle of one can't be made
  seamless (the turn goes with it). Instead it no longer needs
  restarting: unchecking a client only pauses it, still pointed at the
  pool's proxy, which decides request by request whether the pool
  serves it or passes it through on the client's own sign-in, so
  turning the pool off or on for Claude Code or Codex takes effect from
  its next request with nothing restarted or cut (a restart remains
  only when Codex is first pointed at the proxy, or taken off it with
  `olo-pool --put-back`). And the pool's own restarts (an upgrade) are
  now exactly as asked: new connections wait in the queue, what is
  under way finishes, the new version takes the listening sockets over
  and answers what waited (`drain.rs`; `/pool/status`'s `drain`).
- ~~**Quick Look: TOML highlighted**~~ (Ben, 2026-10-30): "add support for
  toml syntax highlighting to quick look". The bundled grammars (syntect's
  defaults) have no TOML (or INI, which `.toml`, `.conf`, `.desktop` and
  `.service` were sent to, so they show plain): grammars of our own for
  them, built into the preview's syntax set, coloured as the others.
  Done so (`crates/preview/src/syntaxes`): TOML (every kind of value,
  arrays and inline tables over lines, Cargo.lock and the like too) and
  INI (systemd units, .desktop entries, git and editor configs).
- ~~**Deskbar: a dot for apps with notifications**~~ (Ben, 2026-10-29): "in
  the deskbar we should show a notification badge for each app that has
  active notifications. we don't need a count, just a dot on the right
  and it should be themed and not red/alert/problem colored but more
  notification colored i.e. not in your face but just letting you know
  it's there. and when a new notification arrives for that app have the
  dot bounce side to side subtly to indicate a new one arrived". A small
  dot at the right of the app's Deskbar entry while it has notifications
  in the history unread, in the palette's quiet accent (not failure red);
  a new one arriving shakes it gently side to side (a few frames, eased,
  at 120 Hz; not while Do Not Disturb hides banners, or quieter);
  agent-readable (the entry's status says "has notifications"), every
  scale and look.
  Done (HACKING, Deskbar): matched by the notes' desktop entry, else
  name; checked at 1× to 2× (1.25× and 1.75× too), light and dark, and
  under Do Not Disturb (no shake).
- ~~**Windows never move in an upgrade**~~ (Ben, 2026-10-29: "can you
  make sure when i do an olo upgrade my windows don't change positions or
  sizes or workspaces or displays or anything even if auto tiling is
  enabled? right now they still sometimes change"; done: HACKING, "The
  session and in-place reload", auto-tiling, and `tools/dev/upgrade-check`).
  Measured with the upgrade's own order (every app re-executing as
  app_server restarts), which the old reload-only tests missed: the
  auto-tiling trees are now kept across the restart (ratios as laid out,
  each tile naming its window's saved place), parked windows keep their
  display and minimized state, a hand's float choice is kept, kit apps no
  longer die in the race (texture deltas kept, unconfigured surfaces
  never drawn on, the handshake retried), GTK 4 windows tile, and
  displays changing scale keep their windows. 40+ upgrades exact at 1×,
  1.25×, 1.5× and 2× with two displays, hidden auto-tiled workspaces,
  minimized tiles, GTK windows in tiles and windows hanging over onto
  another display. The first upgrade from an app_server before this one
  can still move auto-tiled windows once (it keeps no trees).
- ~~**Kit apps survive the GPU device being lost**~~ (2026-10-29, from
  the upgrade checks; Ben: "never leave a bug like that as a todo not
  done"; done: HACKING, "The session and in-place reload"): once in ~60
  test upgrades on Asahi, a restart made Mesa's Vulkan WSI fail
  (`DRM_IOCTL_SYNCOBJ_TRANSFER failed`, then `VK_ERROR_DEVICE_LOST`), and
  Deskbar panicked in egui-wgpu (`egui_uniform_buffer` invalid). Now the
  device-lost callback, or a panic in GPU work on any thread, makes the
  app start itself anew with its saved state, as an upgrade does, and
  its windows come back exactly: place, size, tile, workspace, stacking,
  and which window has the keyboard (app_server no longer gives a window
  back on its own the focus it didn't have, or puts it on top; Deskbar
  keeps a gone app's place in its list). Pixel-identical at 1×, 1.5× and
  2× for StyledEdit (a tab group), Tracker and Deskbar; losing it again
  within 10 s ends the app instead of looping. Tested with the kit
  socket's debug-only `{"lose_gpu":true}`.
- ~~**Passwords: passkeys, and filling the right account in Chrome**~~
  (Ben, 2026-10-29; next, after Terminal's git work: "this is really
  really important"): "olo passwords needs to support passkeys with a
  scan qr code feature and also we need to do a better job at matching
  the correct accounts in chrome so that we look at all the available
  information (domain, current user profile/account, and text matches on
  the screen to an email/username we know about) and then in the right
  click menu there should be an "Autofill with Passwords (<account
  username/email>)" option and then the "Passwords" entry that shows all
  the other possible matches. we need to be able to reliably determine
  and then remember which e.g. google account to use b/c i have like 6
  google accounts and it'll be a different one for each chrome profile.
  and we need to do a better job at finding the correct inputs to fill.
  and it needs passkey integration so that it supplies the passkey if we
  have one and if not then it can advertise that it can do one and should
  integrate fully with everything so that it's just as seamless as
  possible and everything just works".
  - Passkeys: Passwords keeps passkeys (WebAuthn credentials, made and
    used natively: P-256 keys, attestation "none", sign counts) and
    syncs 1Password's; Chrome gets them through Olo's extension as the
    page's WebAuthn provider (`navigator.credentials.create`/`get`
    answered by Passwords, so a site offering passkeys offers Olo's,
    and one with a passkey there signs in with it); and "scan a QR
    code": a phone's passkey through caBLE / hybrid transport (the QR
    code a site shows, and Olo's own for a phone to use Olo's
    passkeys), over Bluetooth natively.
  - The right account: rank the candidates by everything known — the
    page's domain (and its related ones), the Chrome profile (its
    signed-in account, its name), emails and usernames on the page
    (an account chooser, a "signed in as"), what was filled there
    before in this profile — and remember the choice per profile and
    site, so each of six Google accounts goes with its own profile.
  - The context menu: "Autofill with Passwords (ben@…)" for the best
    match, then "Passwords ▸" with every other candidate.
  - The fields: find the username, email, password, new-password and
    one-time-code inputs reliably (autocomplete attributes, labels,
    names, types, forms split over steps, shadow DOM and iframes), and
    fill them as typing does, so sites' scripts see it.
  Done: the right account, the menu and the fields (HACKING,
  "Passwords in Chrome"): ranked by what the page shows, what was used
  there before in this profile (an autofill or a sign-in by hand), the
  profile's own account, then the site; Chrome puts the two items under
  Olo ▸ (an extension can't have two at the top). Checked in a test
  Chrome: the menu, a two-step sign-in filled through, a sign-in by
  hand as another account remembered for the profile. Passkeys
  (HACKING, "Passkeys"): Passwords is the page's passkey authenticator
  (sites see one, and offer passkeys); it makes them (an alert in
  front: Use Another Device… / Cancel / Save Passkey, into the
  account's login) and signs in with them (an alert, a choice of
  account when there are several), suggests them in a sign-in's field
  menu (Olo ▸ Sign In with Passkey), hands over to Chrome's own sheet,
  with its QR code for a phone's passkey and security keys, when it has
  none or the person chooses another device, takes its alert down when
  the page gives up, and asks for an unlock only when it has a passkey
  for the site. Checked in a test Chrome with a page verifying as a
  site does (client data, site hash, flags, attestation, the signature
  with WebCrypto): making two, signing in with each, Cancel, abort,
  locked both ways, the field menu, Chrome's QR sheet. Not done: Olo's
  passkeys used from another computer (Olo playing the phone, scanning
  that computer's QR code with a camera); a phone's passkeys in Olo
  work through Chrome's sheet.
- ~~**Passwords search and filters; rich search and filtering controls in
  the kit, for Logs too**~~ (Ben, 2026-10-29, while on Passwords): "while
  you're doing stuff with passwords can you also make the passwords
  search work better and add filtering. and add a full set of rich
  search and filtering reusable controls we can use in the log viewer
  too b/c right now the log viewer filtering/search is a mess".
  Done so far: `olo_kit::search` (HACKING, "Searching and filtering"):
  the query language, ranking, highlighting, `SearchField` with
  suggestions and `FilterBar`; Passwords uses them (every item's
  username and non-secret text searched, filters for tags, kinds,
  vaults, sites, users, one-time codes, attachments, weak and reused
  passwords, best matches first, matches marked). The Log Viewer too:
  one search with Program (several, or all, or left out), Level and
  Time as menus beside it, `pid:`, words marked in the messages, and
  `olo-logs --print --query` taking the same text. Review clean at 1×
  to 2× for both, wide and narrow.
- ~~**Terminal: git repos, worktrees and GitHub**~~ (Ben; done: HACKING,
  Terminal "Repositories"; Preferences ▸ Terminal), native (no `git`
  or `gh` subprocesses):
  - a tab in a git repo goes into a group named after the repo on its
    own;
  - the group's context menu manages the repo: its worktrees (every one
    found, a click opens it in a new tab in the group) and New
    Worktree…, which asks for a name, makes it and opens it in a new
    tab;
  - Preferences ▸ Terminal gets a worktrees location for new ones,
    `~/worktrees/[repo-name]/[worktree-name]` by default;
  - a tab in a worktree has Rename Worktree… and Remove Worktree… in
    its context menu;
  - GitHub: with an open PR for the branch, the tab's menu has an item
    to go to it, and the tab shows the PR's status with an icon.
- ~~**Terminal restores itself after Quit**~~ (Ben; done: HACKING,
  Terminal "Restoring"): the state is saved as
  it changes, so quitting and starting Terminal again brings back every
  window, group, tab and working directory. If olo-termd is running
  with the sessions, reattach them; if not, still restore the tabs,
  their directories and their scrollback.
- ~~**Terminal tabs, tab groups and coding agents**~~ (Ben; done
  2026-10-27): tabs (the kit's `TabStrip`; HACKING, Terminal "Tabs"),
  tab groups that can be saved and reopened (a window is a named,
  coloured group; "Groups"), Show All Tabs tiling a window's tabs,
  and native recognition of Claude Code and Codex in a tab, its state on
  the tab and a notification when one needs attention ("Agents").
- ~~**Passwords keeps its own store, synced with 1Password both ways**~~
  (Ben): going to 1Password for every look-up is slow and unreliable.
  Passwords should hold every item itself (encrypted at rest, unlocked
  with the account password or Touch ID) and answer the app, Chrome
  and the palette from it at once; 1Password is only for syncing, both
  ways (changes either side merge by item version, conflicts kept),
  in the background and when asked. Done: `vault.rs`, `sync.rs`; try it
  on the real account (Ben).
- ~~**Tracker opens in column view by default**~~ (Ben), set in
  Preferences ▸ Tracker; open windows keep their own view.
- ~~**Deskbar: a click on a running app with several windows offers them**~~
  (Ben): one window comes forward at once; with more, the left click
  opens the same menu as the right click (each window, Show All).
- ~~**Find: Space shows the selected result in Quick Look**~~ (Ben), as in
  a Tracker window; the arrow keys step through the results.
- ~~**Tracker: File ▸ Open in Terminal**~~ (Ben): the folder shown (the
  active column's), or a right-clicked folder, in a new Terminal window;
  also in the window's and the desktop's context menus, as a new tab in
  the Terminal window last in front.
- ~~**Video plays the wrong parts at the wrong speed**~~ (Ben): OpenH264
  was flushed after every packet, breaking B-frame reordering; now
  `Flush::NoFlush`, mp4 edit lists applied, and WebM seeks fixed
  (vendored matroska-demuxer).
- ~~**Confirm before quitting apps with state that would be lost**~~
  (Ben): Terminal asks before
  closing a window with a program running, and its Quit leaves the
  shells in olo-termd; StyledEdit asks about unsaved documents.
- ~~**Passwords: never change anything on sync without approval**~~
  (Ben): plan, Review Sync, apply (sync.rs); plus ~~File ▸ Reset
  Passwords…~~ (local only).
- ~~**Terminal: ⌥←/⌥→ in Claude Code**~~ (fixed: under the kitty keyboard
  protocol they're now `CSI 1;3 D/C`) (Ben): Option+Left/Right move by a
  word in plain bash but not in Claude Code. Check what Terminal sends for
  them (bash binds ESC b / ESC f; others expect CSI 1;3D / 1;3C) and
  whether Claude Code's input understands it; send what xterm does.
- ~~**Drag and drop everywhere, across apps**~~ (done) (Ben): everything that shows
  files or data can be dragged and dropped, within and between apps and
  to and from other Wayland clients (wl_data_device, text/uri-list and
  the data's own types): Tracker items to any app (e.g. an image into
  Terminal, so Claude Code gets its path), Tracker accepting drops from
  everything, StyledEdit text, Viewer
  images, Passwords fields, Deskbar launches…
  Study how BeOS did it first (the Be Book: BMessage drags with negotiated
  types, drop targets, Tracker clippings) and borrow what still fits.
  **Done:** the kit layer (`olo_kit::dnd`: negotiated types and
  actions, drag pictures with names, drop outlines); Tracker drags from
  every view and the desktop and takes drops on folders, windows, the
  Trash and the desktop (move/copy by file system, ⌥ copies, a
  secondary-button drag asks Move/Copy/Link, text becomes a clipping);
  desktop icons placed freely and kept (Clean Up puts them back on the
  grid); Terminal types dropped paths (shell-quoted) or text; Viewer
  and StyledEdit open dropped files. **Left:** ~~dragging text out of and
  into text fields and StyledEdit (move within, copy between)~~ (done:
  kit `text_drag`, tested headless), ~~Viewer
  images out (the file and image/png)~~ (done, with Quick Look's), ~~Passwords fields (a username or
  password onto another app's field)~~ (done), Deskbar (~~drop files on an app to
  open them with it~~ done; ~~drag an app out to make a link~~ done), dragging from
  ~~Tracker's Get Info~~ (done), ~~spring-loaded folders (hover on a
  folder while dragging to open it), scrolling a view when a drag nears
  its edge~~ (done).
- ~~**Tracker: restore from the Trash**~~ (File ▸ Restore and the item
  menu, `ops::Kind::Restore`; a clash gets a number instead of asking) (Ben): a Restore command (File
  menu and the context menu in the Trash) that puts the selected files
  back where each came from (the trash's `.trashinfo` original path),
  recreating missing folders and asking on a name clash.
- ~~**Passwords: sync every vault, Private included**~~ (archived items failed whole batches; now fetched active-only with a one-by-one fallback, and a Sync Problems window) (Ben): a sync on
  the real account doesn't bring in all vaults (Private missing). Every
  vault the account can read is synced, and errors or items that can't
  sync are listed clearly (by vault and item, with the reason), not as a
  wall of red text.
- ~~**Quick Look / Viewer: basic image editing**~~ (Ben): crop, resize,
  rotate and flip images, then Save / Save As (as the PDF tools do). Done:
  the Viewer's Image menu (rotate, flip, crop overlay, Adjust Size) with
  undo, Save / Save As / Revert; Quick Look rotates and saves in place.
- ~~**Multiplayer screen control for agents**~~ (Ben; done: Agent Screens,
  `apps/screen`, each screen a headless Olo session of its own, watched
  and taken over in the Agent Screens app; `olo-screen tree` from the
  kit's widget tree; the `olo-screens` skill put in Claude Code's and
  Codex's skill folders by a real session; HACKING, "Agent Screens"): an agent starts a
  Olo screen-control session and gets its own workspace and its own
  cursor and keyboard focus, so its input never disturbs the person, who
  can watch that workspace live (and step in). Complete control: pointer
  moves, clicks, drags and drops, scrolling, typing and shortcuts; visual
  updates (frames on change, sequences over time) and structured metadata
  about what is on screen (windows, widgets, text, roles, bounds, from
  the kit's own widget tree rather than OCR). An agent-friendly
  protocol/API (app_server IPC, documented, stable) plus agent skills
  injected automatically into Claude Code and Codex when they run under
  Olo. Agent-first from the ground up: everything designed to work
  seamlessly for agents and people alike.
- ~~**Agent Screens: bugs and improvements from an agent using it**~~
  Done 2026-10-27, all 16: popups in `tree` (Menu/MenuItem) and
  `windows` (`popups`); `scroll`, `drag` and `wheel` paced over time (a
  scroll no longer flings 20,000 px; drags move knobs and corners; the
  kit's wheel read only `discrete`, so every mouse notch was 15 px, now
  48 from `value120`); `menus`/`invoke` use the only window when none has
  the focus, and errors list the windows; rows were already clipped
  (a62e672) and now say `row N/M` and `selected`; `wait`, find by
  name/role/match/text for `click`, `set`, `select`, `scroll-to`; tree
  filters and one line per row; `focus`/`close`/`minimize`/`zoom`/
  `resize`/`place`; JSON from actions with the focused element after;
  `look` with no file, `--crop`; `type`/`set` warn when keys go to
  something that isn't a text field; skill doc updated. (Bug 5, focus
  lost after clicking a row, didn't happen here; the fallback covers it.)
  (Ben, 2026-10-27; feedback from an agent working with `olo-screen`).
  Bugs:
  1. *Popups are invisible:* a dropdown's menu (Log Viewer's level filter)
     shows on screen but is in neither `tree` nor `windows`. Popups belong
     in `tree` (under the window that owns them) and in `windows`.
  2. *`scroll` never ends the touchpad gesture:* scrolling past the top of
     a list leaves the overscroll gap (first row stuck at y≈240, knob
     wrong). Send the gesture end after the movement (`--no-end` to leave
     it open).
  3. *`drag` is too abrupt:* dragging a scrollbar knob or a window's
     corner does nothing, while `down`/`move`/`up` in steps works. Send
     intermediate moves with small delays (`--steps N`,
     `--duration MS`).
  4. *`wheel` stops short:* `wheel agent -30` didn't reach the top of a
     ~60-row list. Capped per call, or merged into one event? Not yet
     known.
  5. *Focus lost:* after a click on a list row, `windows` shows the only
     window unfocused and `invoke` fails ("no window has the focus").
     Find out why (perhaps the app). With one window, `invoke` and
     `menus` should just use it.
  6. *Rows take the header's names:* a table row partly under a sticky
     header gets the header's titles in its name, and hidden rows are
     reported under the header. Clip rows to what shows and mark them
     clipped or offscreen. (a62e672 clipped rows and cells; check against
     this report.)
  Improvements:
  7. *Waiting for the app:* `olo-screen wait agent --idle` (until the
     window stops redrawing), `wait --text "61 lines"` or `--role Row
     --name …`, or `--wait` on actions. Show in Context took 5–9 s and
     Find ~1 s; sleeps still read stale state.
  8. *Click by name:* `click agent --name "Errors Only"`, `--role Button
     --name "="`, `--match 'Row:panicked'`, returning what matched.
  9. *Filter the tree:* `--role`, `--name-contains`, `--visible-only`,
     `--depth`, `--max-len`. Big tables print each row three ways (Row,
     Cell × n, Unknown).
  10. *Set fields and choose items directly:* `set agent --name Find
      "PAM"`, `select agent --name Level "Errors Only"` (a dropdown opens
      with the current item under the pointer, so a second click lands on
      the wrong item).
  11. *Window commands:* `resize agent --window 1 1300x850`, `maximize`,
      `focus`, `close`, `move`; the tree could report resize handles.
  12. *Tables:* `scroll-to agent --name …`; the tree giving scroll range
      and row count (`rows=61 visible=13..25`), or every row with an
      offscreen flag.
  13. *JSON from every command:* `click`, `type` and `key` return the
      focused element or window afterwards.
  14. *Pictures:* `look --crop x,y,w,h`, saving to a scratch folder by
      default with the path printed, and hiding the pointer.
  15. *Clearer errors:* `type` says which element got the text (it went
      silently to a list); `invoke`'s focus error lists the windows.
  16. *Skill doc:* prefer `wheel` for lists (until 2 is fixed); dropdowns
      open relative to the current item; Find-style fields can be slow
      to update; `windows` doesn't list popups (until 1 is fixed).
- ~~**Deskbar: an Agent Screens item, and a tidier tray**~~ Done
  2026-10-27: the date and time have their own row (click: Date & Time);
  the well holds only icons, 16 px on a 22 px grid, centred, wrapping
  evenly; a monitor-and-pointer item (with a count) opens Agent Screens.
  (Ben, 2026-10-27):
  1. While any agent screen is running, the Deskbar shows an icon for it
     (in the tray), perhaps with a count; clicking it opens the Agent
     Screens app.
  2. The tray no longer fits all its icons. Move the date and time to a
     row of their own above the tray, so the tray holds only icons.
  3. Make the tray icons' size and spacing consistent and pixel-perfect
     (same size, same gaps, aligned to the pixel grid at 1× and 2×,
     wrapping to more rows evenly), so it looks neat rather than messy.
- ~~**Notifications history: denser, better looking, several views**~~
  Done 2026-10-27: two-line rows (16 px icon, bold title with app and
  time, one line of body; the selected one opens to its whole body and
  actions), a dot for what is new, day headings, search, the keyboard
  (arrows, Return, Delete, Escape, ⌘F), and List / By App / Cards /
  Conversations from the View menu (⌘1–⌘4), remembered.
  (Ben, 2026-10-27): each entry now wastes a lot of room (a 32 px icon beside a
  stack of app name, title, the sender's site on its own line, a blank
  line, then the body), so only three fit in a 380 px window. Make each
  take far less space and look much better: the app and time on one
  line with the title, the body right under it (one or two lines,
  ellipsized, the whole of it on selection or hover), the site folded
  into the app line, no blank line, a smaller icon, tighter rows, a
  clear unread mark. Then offer view styles (View menu, remembered):
  1. *List:* one compact row each, newest first.
  2. *By app:* grouped under each app (icon, name, count), groups
     collapsible, newest group first; Clear per group.
  3. *Cards:* a grid of small cards, as on a phone's lock screen.
  4. *Conversations:* entries from the same thread (same app and title,
     such as one chat channel) stacked into one, newest on top, with a
     count, expanding in place.
  Also worth trying: a timeline with day headers (Today, Yesterday),
  search/filter, and keyboard navigation (arrows, Delete, Enter to open).
  Keep it agent-readable (rows in the tree, named by their text).
- ~~**Resizing a tiled window resizes its tiled neighbours**~~ Done
  2026-10-27: dragging an edge moves the whole split (neighbours across
  the gap and in line), live, stopping at apps' minimum sizes; the parts
  are kept, also across a reload; only command-tiled windows take part.
  (Ben, 2026-10-27): e.g. one window tiled to the left half, one to the top
  right, one to the bottom right; dragging the bottom-right one's left
  edge to make it wider makes the left one narrower, and dragging the
  split between the right two moves both. Only windows that were tiled
  by a tiling command or shortcut take part (remember that per window,
  and forget it once the window is moved or sized away from its tile by
  hand); windows placed automatically or by hand are never pushed. It
  should work as you'd expect: shared edges move together, nothing
  overlaps or leaves a gap, neighbours keep a sensible minimum size.
- ~~**Chrome's tree: inside iframes**~~ (Ben; done: HACKING, "Chrome's
  widget tree, for agents", Iframes): an iframe is opaque to the
  tree agents get (`tree.js` runs in the top frame only, `frameIds:
  [0]` in `background.js`), so embedded logins, payment forms, consent
  dialogs and the like can't be read or driven by name. Run it in every
  frame (`allFrames`, cross-origin ones too), tell which iframe element
  each child frame is (the parent posts a token into each
  `contentWindow`; the child's content script keeps it and `tree.js`
  reports it), then stitch each frame's nodes under an `Iframe` node at
  the element's content box (its rect plus border and padding, the
  child's own scroll and zoom), clipped to it, nested frames in turn.
  `click`, `set` and `wait` then work inside them as they do on the
  page. HACKING, "Chrome's widget tree, for agents".
- ~~**Agent Screens: each screen's history, replays, videos and stats**~~
  (Ben): Agent Screens keeps, for each screen, what was done on it:
  every action (`click`, `type`, `key`, `invoke`, `drag`, `run`, … with
  what it matched and where, timed, from the requests app_server already
  gets) beside the frames the screen drew, only where they changed.
  1. *History:* a screen's actions as a list in the app (filter, search,
     select one to see the screen as it was then).
  2. *Replay:* plays it back, the frames in time with the actions, the
     pointer, clicks and keys shown; play/pause, speed, scrubbing,
     stepping action by action.
  3. *Export as MP4:* encoded in process in Rust (no ffmpeg), on a
     worker with progress and Cancel. Stretches where nothing happened
     are cut, and a card in the video says how long each was ("4 min
     12 s with nothing happening").
  4. *Stats:* per screen, when it was started and last used, how long
     it has run and been active, actions of each kind, the programs run
     in it, the windows open now; the screens list can sort by last use.
  Agents can read all of it too (`olo-screen history NAME [--json]`,
  `olo-screen stats [NAME]`). The history is kept within limits on
  size and age (Preferences), and outlives the screen until it is
  cleared.
  So far (2026-10-30): the actions' history and the stats, for agents
  (`olo-screen history` with `--kind`, `--search`, `--last`, `--clear`;
  `olo-screen stats`; HACKING, "Agent Screens"), kept under 8 MB, and
  the frames beside them (`record.rs`, changed tiles only;
  `history NAME --look N`), and History and Replay in the window
  (`replay.rs`), and MP4 export for agents (`history NAME --export`,
  `export.rs`), and Export Video… in the window (a worker with progress
  and Cancel), the stats in the window and the list by last use
  (stopped screens with a history too), and the limits in Preferences ▸
  AI (size and age).
- ~~**Agent Screens: a revamp, so it doesn't look like an afterthought**~~
  (Ben, 2026-10-30; done: HACKING, "Agent Screens"): "in the agent screens app the ux is all messed up
  and the scrollbar in history actions doesn't work properly, also the
  actions don't seem to update in real time, we should have filters, and
  the ux just generally needs revamp/polish so that it's not looking like
  an afterthought".
  1. *The History list's scroll bar* works as every other list's does
     (drag the knob, click the track, the wheel, kept in range as the
     list grows; following the newest action when at the bottom).
  2. *Live:* a screen's actions appear in History as they happen (and
     the stats with them), not only when the window is reopened or the
     screen chosen again.
  3. *Filters:* by kind (Pointer, Keys, Typing, Menus, Programs, Windows,
     Reads), by result (failed only), by time, with a search field, as
     Log Viewer filters; the count of shown and all.
  4. *A revamp:* the window laid out as a real app, live view, the
     screens' list, History, Replay and Stats each with its own place,
     sized and spaced from `metrics`, every state (no screens, a stopped
     screen, an empty history) designed, checked with `tsession review`
     at 1×, 1.5× and 2× in light, dark and night.
  Done: the scroll bar was pinned because the list was asked every frame
  to reveal the playhead's row, its context reaching past the list's
  end (the kit's `ColumnListView::reveal` now stops at the ends, and
  History reveals a row only when it is new); a reveal or scroll to rows
  just added drifted back, egui springing back from beyond last frame's
  bounds (the kit's `ScrollView` now goes there over two frames). The
  history is followed as it is written (`history::Reader`,
  `record::index_known`), with filters (kind, result, time, search;
  picked within 2 ms a frame), a sidebar of running and stopped screens,
  a header, Live, History and Stats tabs, and a detail of each action.
- **Other programs feel like Olo's own: every integration point**
  (Ben, 2026-10-29): "is there a way where we can make chrome (and any
  gtk / qt apps) better integrate with olo, e.g. replacing their file
  picker with olo tracker picker and e.g. in chrome something like show
  in folder should show in tracker. basically every integration point
  we can do (whether cleanly through some protocol or 'dirty' through
  some hack) we should do so that everything feels as seamless as
  possible". Clean ways first (portals, D-Bus services, settings), a
  hack where there is none. Each is for Chrome, Electron (Slack,
  Discord, VS Code), GTK 3/4 and Qt 5/6 alike unless noted:
  - **File pickers** (started 2026-10-30: olo-portal answers the
    FileChooser portal with the kit's file panel, open, save and folder,
    several files at once, the app's filters by glob and MIME type as a
    Show pop-up, the portal's `choices`; Chrome's uploads, GTK 4 and GTK 3
    tried. The toolkit switches are deliberately not set for everything:
    they also send GTK's link opening through OpenURI, which asks for an
    app until one was picked 3 times (HACKING, "Open and Save"); done
    2026-10-30: the panel's list as Tracker's, Name, Kind, Size and
    Modified, sorted by a header, Favorites and Recent Folders in its
    folder pop-up shared with Tracker's Go menu (`olo_kit::places`),
    and Space for a look at a file in the Viewer): an Olo
    portal backend
    (`org.freedesktop.impl.portal.FileChooser` in `olo-portal`, beside
    ScreenCast) showing the kit's file panel (Tracker's columns,
    favourites, Quick Look, recent places); `olo-portals.conf` sends
    FileChooser to it. Chrome and Electron use the portal with
    `--xdg-portal` / `GTK_USE_PORTAL=1`-style switches or by default;
    GTK 4 apps use it by default, GTK 3 with `GTK_USE_PORTAL=1`, Qt with
    `QT_QPA_PLATFORMTHEME=xdgdesktopportal`: set these in the session
    environment. Save dialogs too (name, folder, overwrite).
  - ~~**Show in folder**~~ (done 2026-10-30: the desktop's Tracker
    serves `org.freedesktop.FileManager1`, each call an Open in
    Tracker). The OpenURI portal's `OpenDirectory` needs nothing more:
    xdg-desktop-portal (1.22 here) answers it with FileManager1's
    `ShowItems`, which Tracker serves, and only without one opens
    `inode/directory`'s default, which olo-session makes Tracker too.
  - **Opening things** (started 2026-10-30: the AppChooser portal is
    Olo's Open With window, so OpenURI's "which app?" is Olo's; olo-session
    writes desktop entries and `olo-mimeapps.list` defaults so GIO,
    xdg-open and OpenURI open files in Olo's apps as Olo does, links
    staying with the default browser; Olo's Chrome, once installed, is
    offered as the default browser by `~/.local/share/applications/
    olo-mimeapps.list`, behind any browser the person chose and ahead
    of the system's): the
    OpenURI portal and `xdg-open`'s MIME
    defaults resolved by Olo's own (`olo_kit::open`), so links from any
    app open in Olo's choice, files in Olo's apps; Olo as the default
    file manager and browser handler (`mimeapps.list`, the `x-scheme-
    handler`s), Chrome as the default browser offered by Olo.
  - ~~**Downloads**: Chrome's land in Tracker's Downloads with Olo's own
    notification (open, show in Tracker) and Deskbar's progress; a
    download's file shows as being written.~~ (done 2026-10-30: the
    extension tells olo-chrome's host of Chrome's downloads; Deskbar's
    progress through LauncherEntry, an Olo notification with Open and
    Open in Tracker, and Tracker shows a `.crdownload` under its final
    name with a bar and "3.1 of 10 MB", from `user.olo.expected_size`.)
  - **Look** (mostly done: app_server's `toolkits.rs` publishes the
    colour scheme and accent through GSettings, which the GTK portal's
    Settings serves as `org.freedesktop.appearance`; GTK 3 and 4 themes
    from Olo's palette, Qt through its GTK 3 platform theme; Olo's fonts;
    2026-10-30: fonts hinted as the kit, slight and grayscale, and Olo's
    Haiku cursors as an XCursor theme for programs that draw their own
    pointer; Olo's icons in the look as the icon theme, for the
    freedesktop names it has pictures for). Left, both waiting on Olo
    itself: `contrast` (once Olo has a high-contrast look) and
    `text-scaling-factor` (once Olo has a text size of its own; today
    the kit's text is one size, `fonts::PLAIN_SIZE`, which GTK already
    gets as its font size, so there is nothing to scale to).
  - **Menus and shortcuts** (GlobalShortcuts done 2026-10-30: asked
    once per app, kept, held through upgrades): ~~the GlobalShortcuts portal~~ (Chrome's
    extensions' shortcuts, Discord's push-to-talk) bound through Olo's
    keyboard profiles; ~~app menus where a toolkit exports one
    (`org.gtk.Menus`, `com.canonical.dbusmenu`) shown in Olo's menu bar
    style if worthwhile~~ (not worthwhile, 2026-10-30: Olo's menus are
    in each window, as BeOS's were, not in a bar at the top of the
    screen, and GTK 4, Qt and Electron programs already show theirs in
    their windows; a second copy in Olo's style would only duplicate
    them. Tray items' menus are Olo's, which is where it helps).
  - **Notifications, tray and badges** (tray done 2026-10-30: Deskbar is
    the StatusNotifierWatcher and host, items in its tray with their
    menus as Olo menus; tried with Slack's): ~~StatusNotifierItem (Slack's,
    Discord's tray icons) in Deskbar's tray with their menus~~;
    ~~the Unity LauncherEntry count (unread badges) on Deskbar's entries;
    progress on Deskbar (LauncherEntry progress)~~ (done 2026-10-30: the
    row's dot, shaking when the count rises, and a progress bar).
  - **The rest of the portals**: ~~Inhibit~~ (done 2026-10-30: the
    Inhibit portal and `org.freedesktop.ScreenSaver` in olo-portal, held
    by app_server's `KeepAwake`), ~~Background~~ (done 2026-10-30, with
    olo-session starting login items), ~~Print~~ (done 2026-10-30: Olo's Print
    panel, CUPS over IPP, `olo-print`), ~~Email~~, ~~Account~~ (done 2026-10-30: asked in the permission
    dialog with your picture), ~~Wallpaper~~ (done 2026-10-30), Screenshot (with screen sharing),
    ~~Camera/Location as Olo asks permission~~ (xdg-desktop-portal asks
    them through the Access dialog, Olo's), Secret (Passwords as the
    Secret Service, so Chrome's and Electron's stored secrets live in
    Olo's keychain; **held for Ben's go-ahead**, 2026-10-30: Chrome keeps
    its "Chrome Safe Storage" key in gnome-keyring through libsecret, so
    taking over `org.freedesktop.secrets` without first moving every
    existing item across would lose every saved cookie and password in
    Chrome and Slack. It needs a migration that reads the real keyring,
    as an explicit step in Preferences ▸ Security, tried on a copy
    first), and ~~Trash~~ (xdg-desktop-portal trashes through
    GIO, in the freedesktop layout Tracker's Trash uses: nothing to add). Done 2026-10-30:
    Screenshot (Olo's Screenshot app, region when interactive; PickColor
    with a loupe) and Access (the permission dialog, as Olo's BAlert).
  - ~~**Drag and drop and the clipboard**: files dragged from Chrome,
    Slack and GTK into Tracker and back, with the right actions and
    formats (`text/uri-list`, images, `x-special/gnome-copied-files`).~~
    (done 2026-10-30, tried with Chrome 156: a picture dragged out of a
    page lands as its file, a link as a clipping named by its text, files
    from Tracker open in Chrome; Tracker copies files as file managers do,
    `x-special/gnome-copied-files` and all, and pastes files, pictures
    (Chrome's Copy Image) and text; `Cx::paste_data`,
    `olo_kit::clipboard::files`, `dnd::named_file`, `dnd::moz_url`.)
  - ~~**Agent-readable**: the AT-SPI trees of GTK and Qt windows (and
    Chrome's beyond the extension) folded into Olo's widget tree, so
    agents can drive outside programs as they do Olo's.~~ (done
    2026-10-30: olo-screen's `tree` reads a non-kit program's windows
    over AT-SPI, roles named as AccessKit's; tried with GNOME
    Calculator, driven by name; Qt asked to join with
    `QT_ACCESSIBILITY=1`. Chrome's pages stay with the extension, which
    sees more of them.)
  - Where there's no clean way, a hack is fine if it's reliable: an
    LD_PRELOAD shim, a launcher wrapper, a policy file or a patched
    config, documented in HACKING with how it's tested.
- **Google Chrome works great in Olo** (Ben; Olo's web browser):
  many sites really need Chrome. When stock Google Chrome is downloaded and run, Olo
  recognises it: Olo window chrome (server-side decorations, or
  whatever it takes: flags, policies, runtime patches), light/dark
  following the desktop, and Olo Passwords filling and saving logins
  in it seamlessly (e.g. a native-messaging host plus an extension
  installed by policy, speaking the keychain protocol), including the
  verification code (TOTP) from a field's context menu beside the
  username and password. Drag and drop, the clipboard, notifications,
  file pickers, downloads into Tracker, default browser, Deskbar entry:
  all of it as well as Olo can make it. Ugly hacks are fine as long
  as it is reliable.
  ~~**Downloads and the default browser**~~ (done 2026-10-30: Chrome's
  downloads in Deskbar, Olo's notifications and Tracker; Olo's Chrome
  offered as the default browser; see "Other programs feel like Olo's
  own").
  ~~**Olo's look**~~ (done: the portals now start in a real session,
  so it follows light/dark; each profile starts with Olo's title bar
  and frame instead of Chrome's own buttons and with the GTK theme's
  colours; HACKING, "Other toolkits' programs").
  ~~**Passwords in Chrome**~~ (done: the Olo Passwords extension,
  packed and signed per user, listed in Chrome's `extensions` folder,
  with `olo-chrome` as its native messaging host; a field's context
  menu offers each login's username, password, one-time code or the
  whole login, the toolbar button lists the page's logins, and a login
  sent with a form is offered for saving or updating; HACKING, "Google
  Chrome").
  ~~**Notifications**~~ (done: Chrome sends sites' and extensions'
  notifications to Olo's notifications app over
  `org.freedesktop.Notifications`; tested headless: shown with Chrome's
  name and icon, a click reaching the page's `onclick`, Chrome's Settings
  button opening its notification settings. Test buses need
  `tools/dev/busd-lenient`; HACKING, "Notifications").
  ~~**Installed on first use**~~ (done: `apps/chrome`, `olo-chrome`;
  tested headless end to end, the real 139 MB download into a fixture
  home, then Chrome running on Wayland in Olo; kept up to date: once
  a day a start fetches a newer version in the background for the next
  start, tested from a stand-in old version. Left: Chromium where Google
  has no build) (Ben): Google Chrome is always listed in
  Deskbar's apps; the first launch downloads it (Google ships arm64
  Linux packages now: dl.google.com/linux/chrome/rpm/stable/aarch64,
  checked against the repo's sha256), unpacks it in process (rpm cpio
  payload, natively) into `~/.local/share/olo/chrome/<version>` with
  a `current` link, with live progress, and keeps it updated from the
  same repo. Chromium if Google's isn't there for the machine. Ben's
  machine has 156.0.8078.47 there now (installed by hand the same way,
  with `~/.local/share/applications/google-chrome.desktop`); it runs on
  Wayland under Olo.
- ~~**Passwords: sync with several 1Password accounts**~~ (ticked in File ▸ 1Password Accounts…, one review, the sidebar grouped by account) (Ben): setup now
  makes you pick one account; Passwords should sync with every account
  the 1Password app has signed in (each account's vaults shown under it,
  items keeping which account and vault they belong to, per-account sync
  state and errors), with accounts added or removed later in its
  settings. The store and sync (`vault.rs`, `sync.rs`) key remote state
  by account as well as vault.
- **x86 machines, starting with Ben's ThinkPad P1 Gen 9** (Ben; it
  arrives within days): stock Fedora, `git clone` the repo, run a setup
  app, and everything works and is supported in Olo, not just on
  the arm64 M2 MacBook Pro (Aurora). The setup app checks and installs
  what is needed (packages, Rust toolchain, the session entry, PAM,
  udev rules, keyd-style key mapping where wanted) with live progress,
  and is safe to run again. The machine: Intel Core Ultra 7 356H; 64 GB
  LPDDR5X; 2 TB PCIe Gen5 SSD (Opal); 16" 3200×2000 tandem OLED, touch,
  HDR 600 (1500 nits peak), 100% DCI-P3, VRR 40–120 Hz, factory colour
  calibration; NVIDIA RTX PRO 2000 Blackwell 8 GB (hybrid with Intel
  graphics); 5 MP RGB+IR camera (face sign-in) with dual microphones;
  Intel Wi-Fi 7 BE211 and Bluetooth 5.4; fingerprint reader; haptic
  touchpad; backlit US keyboard. All of it should work: hybrid graphics
  (app_server on the Intel GPU, the NVIDIA one for apps that want it,
  external displays on either), HDR and wide colour and the panel's
  calibration, VRR, touch screen, the haptic touchpad's gestures and
  click settings, IR face sign-in alongside the fingerprint for unlock
  and Passwords, brightness, keyboard backlight, power profiles and
  battery, suspend, and an x86_64 Chrome. Build and test x86_64 in CI or
  a VM before it arrives.
  Done so far: `tools/setup.sh` (checks by capability, installs with dnf or
  layers with rpm-ostree, Rust through rustup if it's too old, builds and
  installs; `--check` reports only); `tools/dev/x86-check` (the whole
  workspace type-checks for x86_64 from this Mac, clean); the GPU Olo
  draws with is the boot GPU (Intel on a hybrid ThinkPad;
  `ANVIL_DRM_DEVICE` overrides); touch screens, keyboard backlights
  (`*kbd_backlight`) and laptop panel brightness are already generic.
  Waiting on the machine: NVIDIA for apps and external displays, HDR and
  wide colour, VRR, the haptic touchpad, IR face sign-in.
- ~~**Passwords settings in a new Security preferences panel**~~ (Preferences ▸ Security: sync automatically or only when asked, lock after idle, lock with the screen, clipboard clearing; Passwords follows them live; carried over from passwords.json; the panel and settings round trip tested headless, Passwords' side not yet run against them) (Ben):
  choose automatic sync with 1Password or sync only on demand, and how
  long Passwords waits before locking itself (or never). Preferences
  gets a Security panel for them (and a home for other security
  settings later); Passwords follows changes live.
- ~~**Other programs survive an Olo upgrade**~~ (done: `olo-wlproxy` holds their connections and replays their state on the new app_server; tested headless with GNOME Text Editor, kitty and Chrome through several reloads, a menu open during one, an olo-session re-exec and a proxy crash; HACKING, "The session and in-place reload". Deskbar's upgrade alert names only programs that won't survive (`WindowInfo::kept`), and windows put back after a restart stay where they were even when they hang off the screen. X11 is out: Olo is Wayland only, Xwayland support removed) (Ben): when app_server
  restarts for an upgrade, Chrome and other non-Olo Wayland apps
  keep running with their windows, as Olo's own apps do (e.g. hand
  the listening socket and client connections to the new app_server,
  or a small proxy that holds client connections across the restart
  and replays their state).
- ~~**Unzoom goes back to where it was**~~ (a tiled window unzooms back into its tile, one placed by Tile All back into its cell; dragging or resizing it by hand forgets the tile; Restore still goes back to its own frame; tested headless with the shortcut and the zoom box) (Ben): zooming a window that was
  tiled (e.g. to the right half) and then unzooming must put it back in
  that tile, not some older frame (PreZoom should be the frame just
  before the zoom, tiles included, and the tile state restored).
- ~~**Band selection everywhere**~~ (kit `ColumnListView`, so Tracker's list view and every list; Tracker's column view from below a column's items; icon view and desktop as before; scrolls on near the edges; tested headless) (Ben): dragging from empty space draws a
  selection band and selects every item it touches (⇧/⌘ add to the
  selection) in Tracker's list and column views as it already does in the
  icon view and on the desktop, so several things can be picked to move.
  Put it in the kit (ColumnListView and any list of items) so every app
  selects the same way; check how BeOS did it (Tracker's BPoseView,
  BListView, the Be Book's selection conventions) for one consistent
  behaviour across apps.
- ~~**Dialogs always come to the front**~~ (the window focused before an
  upgrade or reload kept a claim to focus for good, so a window reopened
  in its remembered place, like the Upgrade alert Deskbar closes and
  reopens, came up under it; now the claim lasts only while windows come
  back) (Ben): the Upgrade Olo dialog
  from the Deskbar tray sometimes opened behind every other window
  (after Tile All). A newly opened dialog or alert must be raised and
  focused.
- ~~**Disk manager**~~ (done: DriveSetup, apps/drive-setup, tested against
  `udisks-fake` on a private bus rather than loop devices) (Ben): a native Olo utility for disks and
  partitions, after BeOS's DriveSetup (and Haiku's): the disks as a tree
  with their partitions drawn to scale, and per partition: mount and
  unmount, initialize (format) with a file system, create/delete/resize
  partitions, set the name, show what is on it (size, used, file system,
  where mounted). Improve on it where we can: SMART health and
  temperature, encryption (LUKS) unlock and setup, USB sticks and SD cards
  (write an image, eject safely), a benchmark, and live progress for long
  jobs that can be cancelled. Native: talk to UDisks2 over D-Bus (zbus)
  rather than running parted or mkfs; every destructive step behind a
  clear alert naming the disk; polkit for privileges; agent-readable over
  IPC. Never test it on the real disks: loop devices in a test session.
- **Notifications, robust and everywhere** (Ben): after Haiku's
  notification_server and `BNotification`, better where we can. ~~Done~~:
  grouped by app, the four kinds (information, important, error,
  progress), body markup with links, `value` progress, replace-by-tag,
  inline reply (`NotificationReplied`), `transient`, `category`, sound
  hints read, `ActivationToken` before an action, a click bringing the
  app's window forward; sounds played natively (the freedesktop theme
  for important notes and errors); a history kept across a restart
  (History window, Deskbar's bell with the unread count); Do Not Disturb
  by hand, for a while or on a daily schedule; per-app banner, sound and
  history choices (Preferences ▸ Notifications); agent-readable
  (`com.olodesktop.Notifications1`, `olo-notifications --list --json`,
  `--invoke`, `--dismiss`, `--clear`, `--dnd`); GLib's
  `org.gtk.Notifications` (GApplication apps, their actions run on the
  app) and Flatpak apps through the GTK portal; a kit API for Olo's
  own apps (`olo_kit::notify`: Tracker's long
  copies); Do Not Disturb while sharing the screen (2026-10-30: from
  olo-portal's sharing state, errors held too, banners already up taken
  down; "While sharing the screen" in Preferences, on by default).
- ~~**Log viewer app**~~ (done: `apps/logs`, Log Viewer; the journal read
  natively, live; Find, program, level and time filters; syntax
  highlighting; Show in Context; `olo-logs --print … [--json]` for
  agents; HACKING, "Log Viewer"; 2026-10-30: the apps' own log files
  beside the journal too, the Agent Screens' listed in File, any other
  opened, `--print --file`) (Ben): an Olo app that shows every Olo
  event in one place (app_server, olo-session, Deskbar, Tracker and
  every app's log) so issues can be spotted and tracked down: live tail,
  filter by program, level and text, a time range, warnings and errors
  highlighted, and the lines around an error. Read the user's journal
  natively (the journal file format, in process, not `journalctl`) plus
  the apps' own log files; never block the UI (stream it, on a worker
  thread). Agent-first: the same query is available over IPC, so an agent
  can ask "what went wrong in the last ten minutes" without screenshots.
- ~~**Night light does nothing on the MacBook**~~ (confirmed: apple-drm has no gamma ramp, `gamma_length` 0, but every CRTC has `CTM`; app_server now sets a diagonal colour matrix there, in a non-blocking atomic commit retried at vblank; needs Ben's eyes on the real screen) (Ben): turning it on in
  Preferences changes nothing on screen. app_server gets the request and
  fades (log: `night light kelvin=Some(…)`), but no gamma warning is
  logged, so the Apple display driver most likely offers no legacy gamma
  ramp (`gamma_length` < 2) and `udev.rs` `set_gamma` returns quietly.
  Do it where it always works: use the CRTC's `GAMMA_LUT`/`CTM`
  properties through atomic modesetting when the driver has them, or
  else tint in the compositor's own render (a colour-matrix pass over
  the frame, which also covers screenshots and headless outputs). Show
  in Preferences when the screen can't do it; check external displays
  too.
- ~~**Scrolling feels the same everywhere**~~ (app_server `scroll.rs`: Chromium ×1/12 a touchpad, ×0.4 a wheel; GTK 4 ×0.4 a touchpad; GTK 3 by the window's height (it scrolls by its view's, ^⅔), 94–97% of Olo's distance; measured headless) (Ben): in Chrome scrolling is
  far too fast (maybe GTK apps too). A wheel notch or touchpad swipe
  should move Chrome and GTK apps as far as it moves an Olo app, with
  the same smooth scrolling (app_server's axis values, value120 and
  discrete steps, and what each toolkit multiplies them by).
- ~~**GNOME/GTK 4 apps look at home**~~ (done: the portals start
  in a real session (they never did, so GTK 4 had no settings at all),
  and GTK 3, GTK 4 and libadwaita take Olo's colours, fonts and
  close-left/zoom-right buttons in both modes; GTK windows without a
  headerbar get Olo's tab and frame through KDE's server-decoration
  protocol, tested headless with GTK 3 and 4; headerbar and libadwaita
  windows draw their own, by design, in Olo's colours; HACKING,
  "Other toolkits' programs") (Ben): a GTK 4 / libadwaita app
  opened in Olo follows light/dark mode properly (and switches live),
  and gets Olo's window chrome (yellow tab, BeOS borders) rather than
  its own client-side decorations: e.g. asking for server-side decorations
  via xdg-decoration / GTK's settings, and whatever libadwaita needs
  (portal `org.freedesktop.appearance color-scheme`, dconf) to agree.
- ~~**Settings & state backup: export/import in one file**~~ (Preferences ▸ Backup and `olo-preferences --export/--inspect/--import`; tested headless) (Ben): a native
  Olo utility that exports every Olo setting and piece of saved
  state, across all Olo apps (Preferences settings, Tracker views,
  Deskbar, Passwords' vault
  file, keymaps, window state, …), into a single compressed file, and
  imports one back (with a summary of what it holds and a confirm before
  replacing anything). In Rust, in process (e.g. tar + zstd crates);
  secrets stay encrypted as they are on disk.
- ~~**Notification banners: never, Deskbar-wide, compact**~~ (Ben; done:
  HACKING, Notifications main.rs): "Show banners for:" has Never (only
  errors show); banners are the Deskbar's width, flush with the right
  edge in its column, with tighter padding and no big icon unless the
  app sent a picture.
- ~~**Notifications: Dismiss, and Activate becomes Open**~~ (Ben; done:
  a selected note has Dismiss, which takes the whole conversation when
  it gathers several; the default action always reads Open; By App's
  per-app Clear became Dismiss too; HACKING, history.rs).
- ~~**New Window from a running app's context menu**~~ (Ben; done:
  HACKING, "Single instance"): Tracker offers New Window on a running
  Olo app's program or link (folders and desktop), and Deskbar in
  the app's menu; both forward
  `--new-window` to the running copy instead of launching a second one
  (Tracker, Terminal and StyledEdit; `apps::NEW_WINDOW`). Agents run
  `olo-x --new-window`.
- ~~**Tracker column view scrolls sideways**~~ (Ben): touchpad,
  Shift+wheel or the new scroll bar; it no longer snaps back to the
  active column, which it only follows when the selection moves.
- ~~**Command palette on ⌘Space**~~ (Ben): apps, the front app's menu
  commands, files, Passwords items, Preferences panels, appearance,
  windows and every desktop shortcut with its keys; a calculator; the web.
- ~~**Menus open on a touchpad tap**~~ (Ben): a tap's press and release
  came in one frame and the press was missed; a second press on the
  title closes the menu.
- ~~**Quick Look: F for fullscreen**~~ (Ben), F again for the window
  (`Cx::set_fullscreen`).
- ~~**Momentum scrolling and Mac touchpad behaviour**~~ (Ben): a flick
  glides on after the fingers lift and stops when they touch again
  (app_server); taps reach
  every kit control as a press then a release; two-finger tap is the
  secondary click, no middle-click emulation or drag lock.
- ~~**⌘A in a text field**~~ (Ben) selects the field's text, not the
  page or folder.

- ~~**Terminal sessions that outlive Terminal**~~ (Ben): done. Shells
  live in olo-termd (one per display); quitting, crashing or upgrading
  Terminal reattaches every window with its scrollback, and olo-termd
  upgrades in place too. Terminal ▸ Terminal Daemon… shows it and can
  restart it (keeping the shells) or start it over. Images come back
  too.
- Terminal: ~~find in scrollback~~ (Edit ▸ Find…, Find Again, Find
  Selection; regex over screen and history); ~~release-build memory
  check~~ (fonts mapped, wgpu MemoryUsage: every app ~100 MB smaller);
  ~~colour emoji~~ (olo-emoji, COLRv1 via skrifa + tiny-skia).
- ~~Activity Monitor~~: apps/activity (graphs of every /proc, /sys and
  hwmon source; processes with Quit / Force Quit).
- Preferences, remaining panels: ~~Network~~ (Wi-Fi list, join with
  password, disconnect, forget, interface details; ~~WPA-Enterprise~~: PEAP/TTLS/TLS
  sheet), ~~Bluetooth~~ (search, pair with passkey/PIN agent, connect,
  remove, battery), ~~Date & Time~~ (calendar, clock face, network
  time, time zone, Deskbar 24-hour/seconds/day), ~~Touch ID~~ (enrol with progress, remove); ~~Restart Olo~~; ~~a Choose… file panel for the
  wallpaper~~ (app_server now draws it); ~~Display mode switching and drag-to-arrange~~ (with keep/revert
  countdown; modes and positions restored at start-up).
- ~~**StyledEdit**~~: styled text in attributes beside plain text, Font /
  Document menus, Find / Replace windows, own undo, atomic saves, reload
  keeps unsaved work.
- ~~**DeskCalc**~~: R5's keypad, Haiku's scientific rows or none; typed
  or clicked expressions (own evaluator: functions, π/e, implicit ×,
  degrees/radians), history on ↑/↓, keys flash as you type.
- ~~**Screenshot**~~: Haiku's app (screen or active window, border,
  pointer, delay, five formats, folder choice, clipboard), ⌘⇧3/⌘⇧4/⌘⇧5
  (area) and Print Screen; every backend captures.
- Password manager: ~~Rust 1Password SDK~~ (desktop-app integration;
  fake server for tests), ~~Passwords app~~ (browse, copy, edit, create,
  archive/delete, generator, lock with ~~Touch ID through fprintd~~ or
  password), ~~browser integration~~ (the Olo Passwords extension in
  Chrome).
- Viewer: ~~image zoom/pan~~ (full resolution up to the texture limit,
  View menu, pinch/⌘-scroll/double-click, drag and scroll to pan; halved
  levels for smooth fitting); ~~video playback (frames + audio)~~ (H.264 in mp4/Matroska, AV1 in
  Matroska/WebM, synced to the sound, seeking, keeps the screen awake;
  ~~AV1-in-mp4~~ (2026-10-30; 600 pictures a second at 640×360 in
  release); VP9 and HEVC still show facts only), ~~AVIF/HEIC/Opus~~.

## app_server

- ~~**Agent rules: fast and light, measured**~~ (Ben, 2026-10-31; done:
  CLAUDE.md, HACKING "Fast and light", `tools/dev/cost`): once
  app_server's and Terminal's CPU are fixed, "add agent rules about this
  … so that moving forward everything is very carefully benchmarked and
  cpu cycles are never wasted and everything is ultra optimized. one of
  the core tenets of olo is that it is super fast and lightweight"
  (CLAUDE.md and HACKING.md: how to measure a change's CPU and frame
  cost, what budgets hold, and that a change isn't done until it's
  measured).
- ~~**app_server's idle CPU**~~ (Ben, 2026-10-31; done, measured
  headless; on the real displays to be seen after the upgrade: HACKING,
  rule 9's resting displays and "Fast and light"): every display looked
  for damage every frame forever (on a real one with a DRM test commit),
  and every app's kit threads took every event. Now displays rest after
  3 empty looks (every 100 ms, woken at once by commits on them, input,
  requests, window changes), the event loop's timeout is 100 ms, not
  16, and apps ask only for the events they use. Idle session: 10 looks
  a second instead of 61 (240 on two 120 Hz displays), 26 wakeups a
  second instead of 119, same scrolling latency. "i often see
  olo-app-server using 20%+ cpu. look at pid 3266 right now to see what
  it is doing and how it can be made much more efficient: it should
  never need to get above single digit cpu in a core." Measured
  (read-only, /proc): 21–26% of a core, all on the main thread, about
  half of it in the kernel (sys 13–14%), ~290–320 wakeups/s, with three
  displays (eDP-1 and DP-1 at 2×, DP-2 at 1×) and Terminal redrawing ~26
  frames/s for an agent's spinner (above) and Activity Monitor's live
  graphs open. So every client frame costs app_server several
  milliseconds. Find where (sampling its main thread), then: composite
  only the damaged region of only the display it's on, skip outputs
  with no damage, keep GPU work from re-uploading what didn't change,
  and see whether the keep-warm pixel (one frame a second per display
  while in use) and the GPU buffer ioctls (agx VM binds) are part of it.

- ~~**Auto-tiling follows the sizes I give**~~ (Ben, 2026-10-29; done:
  HACKING, auto-tiling; edges no longer snap when let go): "auto-tiling
  should let me resize windows however i want and then adapt around how i
  have sized them, instead of locking me to some sort of hidden grid like
  it does right now". A resize by hand keeps the size it was let go at
  (no snapping to quarters and thirds), and the neighbours take up the
  rest around it; new and closed windows keep the sizes given.
- ~~**Window menu on the title tab**~~ (Ben; done: HACKING, "The command
  palette", window menu): right-clicking a window's yellow tab opens a
  kit menu of Hide, Zoom, Back to Its Own Size, Center, Tile ▸, Move to
  Workspace ▸, the displays, Send to Back and Close, the shortcuts' own
  commands. Not yet: Always on Top (app_server has no such state) and
  "tile with…" (tiling beside a chosen window).
- ~~**Auto-tiling mode**~~ (Ben, 2026-10-28; done: HACKING, "Desktop
  shortcuts and tiling", shell/autotile.rs): Deskbar's tiles beside the
  cup, ⌃⌥⌘T, the palette and `set_auto_tiling`, per workspace (and per
  display with a Deskbar on each), remembered. A tree of splits per
  display (dwindle: a new window halves the focused tile along its longer
  side, a closed one's room goes to its neighbour, a window group is one
  tile); Zoom over the tiles and back, Zoom Vertically / Horizontally a
  full column or row, the tile commands set a window's part with the rest
  around it, edges dragged (snapped to quarters and thirds then; now they stay where let go), tabs drag to swap or go
  beside. Dialogs, alerts, fixed-size windows, the kit's
  `WindowOptions::floating()` (xdg-toplevel-tag) and portal and
  picture-in-picture windows float; Float Over the Tiles (window menu,
  ⌃⌥⌘F, `set_floating`) by hand. O(windows), only on a change, a new
  window opening at its tile's size, minimum sizes kept, ⌃⌘Z, through an
  in-place reload. Not yet: a master-and-stack layout (dwm, Amethyst),
  resizing every tile in one transaction (Sway), a snap-layout picker.
- ~~**⌘[ / ⌘] are Back / Forward everywhere**~~ (Ben, 2026-10-29; done:
  HACKING, BeOS idioms, keymap.rs): in every program that isn't Olo's
  (Slack too, not only browsers) they reach it as Alt+← / Alt+→. ⌘← / ⌘→
  never navigate: they are the line's start and end everywhere.
- ~~**Auto-tiling: a neighbour spills past Deskbar after a resize**~~
  (Ben, 2026-10-29; done: autotile.rs, tiling.rs, grabs.rs, tested by
  `tools/dev/auto-fuzz`): making Terminal wider resized Tracker beside
  it, but Tracker then reached past Deskbar's edge until the next layout.
  The edge drag's last size went to Terminal after the tiles were laid
  out for the one before it; a tile laid out while a size it was asked
  for was still on its way took that size late, unplaced; a neighbour
  stopped being followed once it answered, before it drew; and a new
  window's least size, learnt as it first drew, waited for the next
  layout.
- ~~**Opening an app makes a new window on this workspace**~~ (Ben,
  2026-10-29; done: HACKING, "Single instance", `Cx::on_this_workspace`):
  opening Tracker (or a folder), StyledEdit or a Terminal tab when the
  app's windows are all on another workspace opens a new window here
  instead of going there; each workspace has its own group of the app's
  windows. Single-window apps (Preferences, Activity, …) still go to
  their window's workspace.
- ~~**Windowing modes**~~ (Ben, 2026-10-29; done: HACKING, "Desktop
  shortcuts and tiling", shell/modes.rs): auto-tiling is one of three
  modes, per workspace (and per display with a Deskbar on each). Deskbar's
  tray icon shows the mode and opens Window Arrangement, which chooses
  it; Preferences ▸ Windows sets the default; ⌃⌥⌘T and ⌃⌥⌘O, the palette
  and `set_windowing_mode` too, each with its OSD. Standard (the default)
  leaves windows where they are put; Auto-Tiling as before; Focus shows
  only the active window, zoomed, its app's others grouped behind it as
  tabs, dialogs, alerts and floating windows over it. Activating another
  shows that one; the rest are covered (not drawn, no input, ready to
  show at once). Leaving Focus puts each window back; arranging commands
  leave it for Standard. Old auto-tiling settings are read as modes.
- ~~**Spotlight the chosen app while switching**~~ (Ben, 2026-10-28;
  done: HACKING, "Twitcher", spotlight.rs): while ⌘Tab (or ⌘\`) is
  held a moment, everything but the chosen app's windows (and their
  dialogs) dims, following the choice; it fades out as ⌘ is let go, and
  Esc or the lock takes it away at once. A quick ⌘Tab flips without it.
- ~~**An overview of everything, on ⌃Space**~~ (Ben, 2026-10-28; done:
  HACKING, "The overview", overview.rs): every display's workspaces as
  panes, their windows spread so none covers another, live (those on
  other workspaces too), lettered; a click or a letter makes one the
  active window on its workspace, the arrows and Return walk them, Esc,
  ⌃Space or a click on nothing leaves. Windows showing glide into place
  and back; the rest fade. Agents: `{"type":"overview"}`. Not yet:
  dragging a window to another workspace's pane, adding a workspace.

- ~~**Multiple monitors, managed like macOS Displays.**~~ (done: the last
  clause, the pointer crossing at matching scale, holds because displays
  are arranged in one logical space, each scale already in it, and the
  pointer crosses wherever screens touch, `clamp_to_screens` with tests;
  each display has its own workspaces and Deskbar too) Hotplug of external
  displays (USB-C/Thunderbolt on the M2), each with its own resolution,
  refresh rate, scale factor (2× default on HiDPI, 1×/1.5× etc. on
  others) and rotation; mirror or extend; choose the main display (where
  Deskbar lives). Preferences ▸ Displays shows the screens as proportional
  rectangles you drag to arrange and align (edges snap; the arrangement
  sets the global layout), with per-display settings beside them and an
  "Identify" overlay. Remembered per monitor (EDID make/model/serial) and
  restored on reconnect; a revert-in-15-seconds confirmation after mode
  changes. Windows move off a removed display; workspaces and Deskbar
  follow the main display; cursor crosses edges at matching scale.
  Done: per-output mode/refresh/scale, drag-to-arrange with snapping,
  revert countdown, saved per connector and restored at start-up,
  neighbours follow a resized screen, multi-screen headless tests.
  Rotation, the main display (Deskbar and desktop follow it, also on
  unplug) and Identify done too. ~~Settings keyed by EDID~~ (make,
  model, serial; connector entries migrate), ~~mirroring~~ (fit and centred,
  "Use as" in Preferences).

- ~~**Flicker, still, now and then**~~ (Ben; done: HACKING, "Finding
  flicker"): most of all on Quick Look (Space on an image in Tracker):
  the window flashed as it opened. Recorded frame by frame (the new
  `record` request): its first frame was in its cascade slot, the next
  over Tracker. Windows were placed in a post-commit hook, which runs
  before the buffer is taken in, so placement waited a commit; now it
  happens in the commit handler once the window knows its size, before
  it is drawn (every dialog and every returning window had the same
  frame elsewhere first).
- ~~**wm-fuzz seed 2, step 192**~~ (found 2026-10-29, also before today's
  changes): a tiled Tracker window drawn as the bottom-right quarter is
  refitted to the whole bottom half when its workspace is shown again
  (`tools/dev/wm-fuzz NAME --steps 192 --seed 2` on a fresh
  `2560x1600,1920x1080 --session --shell` session with a fresh home):
  its tile state and its frame part ways somewhere earlier (grouping,
  arranging, a display moved); find where.
  Done (seed 2 now passes 400 steps with `--tiles`, a new check that
  every tiled window is in its tile's part, which `list_windows` now
  gives as `part`): a hidden window passed over by ⌘` joined its old
  group for a moment and kept the group's tile when it went back to its
  own frame (its own tile is kept with its frame now); Tile All's
  window, still at its old size, was pushed inside the tiling area by
  that size and stayed there (`keep_in_tile_area` waits for the size
  asked); a group's back tab took a text-size change late, when the
  group came apart (`behind` in `WindowInfo`: the kit counts it not
  shown); and undoing an arrangement or unzooming pulled a window left
  hanging off an edge back in (`put_back_at`: back exactly while some
  of its top edge is on a display).
- ~~**wm-fuzz seeds 3 and 5**~~ (found 2026-10-30; seed 5's also on the
  app_server before the fixes above). Seeds reproduce only from a fresh,
  empty `--home` (a used one restores windows and changes the path).
  Fixed (3a8568a): a display back from mirroring kept its closed Deskbar
  bars, empty, in its layer map, which moved its tiling area over (seed
  5 step 213's window came back off the display); and a window its app
  keeps bigger than its tile jumped and came back each time it was put
  back. Then the rest (seed 3 passes 300 steps three runs in three, seed
  5 its 300):
  - seed 5 step 213: a window sent to another display that joins its
    app's group there by itself goes back as the round trip while it is
    still as it joined (the trip notes the join, `note_joined`), with
    the arrangement it left with (its tile was the group's).
  - seed 5 step 246 and seed 3 step 268: a window was a member of its
    group only if it was in that same group on that same display before
    (`was_member` by the group's number, `seen_on` by display), so one
    moved to another display joins a group there rather than leading it
    to its own frame.
  - seed 3 step 271: the checker allowed for the group growing to its new
    window's least size (`least` in `WindowInfo`, the smallest frame its
    app allows, tab and border included).
  - seed 3 step 243: a window straddling two displays (its middle on
    neither) groups on the display of the lowest-numbered window among it,
    its group and the window it was put with (`group_display`), so a
    group's windows aren't split by a few points of size.
  Run: `tsession start NAME 2560x1600,1920x1080 --session --shell --home
  EMPTY` (`TSESSION_PROFILE=release`), `wm-fuzz NAME --seed N --steps 300
  --keep-going`.
- ~~**app_server: a slow frame now and then in the bench's live resize**~~
  (found 2026-10-30, at 1.5× and 2×; done 2026-10-30): 15–48 ms once in
  a run, in `render_output`. Timed by part (app_server now logs a slow
  frame's parts, and which window or layer took the time): it was never
  drawing but gathering the elements, and two things in it: a new
  wallpaper's 30 MB texture uploaded whole in the first frame to show
  it (20–24 ms), and a window's tab redrawn by cosmic-text when its look
  changed (4–13 ms: a face's first use, glyphs at a new size, its lock).
  The wallpaper is now strips of about 4 MB, a new one uploaded a strip
  a frame with the old one up meanwhile; a changed tab is drawn on a
  worker, the old one shown till it's done. The bench (Activity Monitor
  and Log Viewer at 1.5× and 2×): 0 of 36 steps fail, where 2–3 did
  every run.
- ~~**wm-fuzz: failures now and then**~~ (found 2026-10-30, from a loop
  over seeds 3, 3, 3, 5, 1, 2, 4, 6 at 300 steps each; done 2026-10-30:
  seeds 1–7 pass all 300 steps, each from a fresh session). They were a
  checker that couldn't tell a slow client from a wrong frame, and five
  real bugs:
  - The checker waited a quarter-second of quiet; a Terminal busy for
    longer looked wrong. Now `WindowInfo::settling` says a window hasn't
    drawn the size it was asked, and wm-fuzz (and `olo-screen wait
    --idle`) wait for none to be; a failure prints every frame of the
    step (`--trail` for steps that pass).
  - Tiling a grouped Terminal moved it at once but kept its old size till
    it drew, so for a frame its middle was on the other display and
    per-display grouping took it out of its group; drawn, it rejoined
    and took the group's frame (seed 1 step 7). A window's display is
    now where it is heading (`heading_frame`).
  - Back from another workspace, each window of a group was placed by its
    own least size before the groups were made again, then pulled to the
    group's (seed 4's flickers): least sizes count the mates a window
    is coming back with (`mates_to_be`).
  - A zoomed window that joined a group on another display came back
    unzoomed, as `Arrangement` kept tile, part and axis but not zoom
    (seed 6 step 116); it keeps zoom and the frames Restore and unzoom go
    back to now.
  - A window that joined a group, then was zoomed into a frame that
    happened to be the group's, "restored" its old arrangement on its own
    again (seed 6 step 202): untouched means the group's frame and
    arrangement both (`Joined::untouched`).
  - Placing a window (`Request::Place`) from behind its group's front
    didn't take the group (it hadn't drawn at the group's size), and
    Restore afterwards took it back to its frame from before it was
    tiled (seed 1 step 210): a placed window leads (`Groups::lead`) and
    its placed frame is its own (`forget_own_frame`).

- ~~**Windows while rearranging displays**~~ (Ben): each window stays on
  its screen at the same place as screens are dragged around
  (`fixup_positions`); windows on a removed screen go to the main one,
  cascaded, and ones that no longer fit are fitted (`rehome_windows`).

- ~~Cursor shapes~~ (Haiku's cursors via olo-hvif, cursor-shape-v1);
  ~~night light~~ (CRTC gamma ramps, faded); ~~libinput settings~~ (touchpad tap, tap-drag, dwt, click
  method, natural scroll, speed; udev only); ~~centred alerts~~
  (`WindowOptions::alert`, set_parent, placed over the parent).

## Lock, screensaver, caffeinated

- ~~**Screen lock**~~: ext-session-lock in app_server, `olo-lock`
  (PAM + Touch ID), Ctrl+⌘Q / Deskbar / idle display-off / sleep / logind
  Lock; never fails open; survives reload (see HACKING "The lock screen").
  Lid close goes through logind's suspend, so it is covered when the lid
  sleeps.
- ~~**Screensaver**~~: the eleven R5 modules (olo-screensavers),
  olo-screensaver on every output, Preferences ▸ Screen Saver (Fade and
  Modules tabs, live preview, Test, hot corners), lock-after drawing over the
  lock screen; Flip and Lens see the desktop on every backend.
- ~~**Caffeinated mode**~~: Deskbar cup (click; right-click or Olo ▸
  Stay Awake for 1–8 hours or with the lid closed), Ctrl+⌘A, timed end,
  shown in the cup's tip and menu.
- ~~**Auto-approve for a while**~~ (Ben): Deskbar's key in the tray
  (click: 15 minutes; right-click or Olo ▸ Approve Without Asking: 5
  minutes to a day, Ask Again) turns it on with the account password
  (not Touch ID: Terminal answers sudo with it). While on, Terminal's
  sudo window and Passwords' unlock and `op` access go ahead without
  asking, each in a quiet notification and the log; the time left is in
  the key's tip and menu. It ends at its time, when the screen locks
  (sleeping locks first) and with app_server, and the password is wiped.

## Other

- ~~Deskbar 24-hour clock option~~ (Preferences ▸ Date & Time ▸ Clock).
- ~~**polkit authentication agent**~~: apps/polkit-agent (real sessions
  only; fprintd then password through polkit-agent-helper-1).
- ~~Icons for new file kinds~~: spreadsheet, presentation, word-document,
  pdf, font (audio, video, archive already had theirs).
- ~~Kit: asynchronous `read_clipboard`~~ (`paste` reads on a helper
  thread and delivers the Paste event through a loop channel).
