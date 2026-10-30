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
- **Terminal's tabs and groups look like Deskbar's list** (Ben,
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
  1×–2× in every layout, light, dark and night looked at.
- **Show All Tabs: each pane's head in its group's colour** (Ben,
  2026-10-29): "can you make the tile headers follow their groups color
  scheme so we can visually see whats what".
  Done so: a pane's head is tinted with its group's colour, a 2-point
  line of it across the top.
- **Tracker: selection as in modern file managers** (Ben, 2026-10-29):
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
- **StyledEdit, much more featureful** (Ben, 2026-10-29, after
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
- **Activity Monitor, revamped** (Ben, 2026-10-29, at the end):
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
  - **Today** sharing fails for everyone: app_server still has Anvil's
    stub for ext-image-copy-capture and fails every frame
    (`crates/app_server/src/state.rs:846`); wlproxy hides that protocol
    from outside programs on purpose; `olo-portals.conf` sends every
    portal to the GTK backend, which has no ScreenCast.
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
    - which app is asking: the portal leaves the app id empty for
      unsandboxed apps unless they run in an `app-<id>-….scope` systemd
      unit, and Olo doesn't start apps in scopes, so it can't tell
      Chrome from Slack; start apps in transient scopes
      (`StartTransientUnit` over D-Bus), which also fixes app ids for
      the Inhibit, GlobalShortcuts and Notification portals;
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
- **Token Pool: requests fail through it; make it truly transparent**
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
- **Deskbar: a dot for apps with notifications** (Ben, 2026-10-29): "in
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
- **Agent Screens: each screen's history, replays, videos and stats**
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
  - **File pickers**: an Olo portal backend
    (`org.freedesktop.impl.portal.FileChooser` in `olo-portal`, beside
    ScreenCast) showing the kit's file panel (Tracker's columns,
    favourites, Quick Look, recent places); `olo-portals.conf` sends
    FileChooser to it. Chrome and Electron use the portal with
    `--xdg-portal` / `GTK_USE_PORTAL=1`-style switches or by default;
    GTK 4 apps use it by default, GTK 3 with `GTK_USE_PORTAL=1`, Qt with
    `QT_QPA_PLATFORMTHEME=xdgdesktopportal`: set these in the session
    environment. Save dialogs too (name, folder, overwrite).
  - **Show in folder / open the containing folder**:
    `org.freedesktop.FileManager1` (ShowFolders, ShowItems,
    ShowItemProperties) served by Tracker, so Chrome's "Show in
    folder", Electron's `shell.showItemInFolder`, GTK's and Qt's go to
    Tracker with the file selected; the OpenURI portal's
    `OpenDirectory` too.
  - **Opening things**: the OpenURI portal and `xdg-open`'s MIME
    defaults resolved by Olo's own (`olo_kit::open`), so links from any
    app open in Olo's choice, files in Olo's apps; Olo as the default
    file manager and browser handler (`mimeapps.list`, the `x-scheme-
    handler`s), Chrome as the default browser offered by Olo.
  - **Downloads**: Chrome's land in Tracker's Downloads with Olo's own
    notification (open, show in Tracker) and Deskbar's progress; a
    download's file shows as being written.
  - **Look**: light/dark and the accent from Olo's palette through the
    Settings portal (`org.freedesktop.appearance.color-scheme`,
    `accent-color`, contrast), GTK's and Qt's themes and fonts made from
    Olo's palette and `style` (generated gtk.css, a Qt palette/kvantum
    or qt6ct config), cursor theme and size, font hinting and scaling
    to match the kit, Olo's icons as the icon theme.
  - **Menus and shortcuts**: the GlobalShortcuts portal (Chrome's
    extensions' shortcuts, Discord's push-to-talk) bound through Olo's
    keyboard profiles; app menus where a toolkit exports one
    (`org.gtk.Menus`, `com.canonical.dbusmenu`) shown in Olo's menu bar
    style if worthwhile.
  - **Notifications, tray and badges**: StatusNotifierItem (Slack's,
    Discord's tray icons) in Deskbar's tray with their menus; the
    Unity LauncherEntry count (unread badges) on Deskbar's entries;
    progress on Deskbar (LauncherEntry progress).
  - **The rest of the portals**: Inhibit (no sleep during a call or a
    video), Background, Print (to Olo's print dialog, once there is
    one), Email, Account, Wallpaper, Screenshot (with screen sharing),
    Camera/Location as Olo asks permission, Secret (Passwords as the
    Secret Service, so Chrome's and Electron's stored secrets live in
    Olo's keychain), and Trash (to Tracker's Trash).
  - **Drag and drop and the clipboard**: files dragged from Chrome,
    Slack and GTK into Tracker and back, with the right actions and
    formats (`text/uri-list`, images, `x-special/gnome-copied-files`).
  - **Agent-readable**: the AT-SPI trees of GTK and Qt windows (and
    Chrome's beyond the extension) folded into Olo's widget tree, so
    agents can drive outside programs as they do Olo's.
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
  copies). Left: Do Not Disturb while sharing the screen, once there is
  screen sharing (app_server's image-copy capture fails every frame
  today; see multiplayer screen control).
- ~~**Log viewer app**~~ (done: `apps/logs`, Log Viewer; the journal read
  natively, live; Find, program, level and time filters; syntax
  highlighting; Show in Context; `olo-logs --print … [--json]` for
  agents; HACKING, "Log Viewer". Left: the apps' own log files, where
  there are any beside the journal) (Ben): an Olo app that shows every Olo
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
- ~~**Scrolling feels the same everywhere**~~ (app_server `scroll.rs`: Chromium ×1/12 a touchpad, ×0.4 a wheel; GTK 4 ×0.4 a touchpad; measured headless; GTK 3 not yet) (Ben): in Chrome scrolling is
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
  VP9, HEVC and AV1-in-mp4 still show facts only), ~~AVIF/HEIC/Opus~~.

## app_server

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
- **wm-fuzz seed 2, step 192** (found 2026-10-29, also before today's
  changes): a tiled Tracker window drawn as the bottom-right quarter is
  refitted to the whole bottom half when its workspace is shown again
  (`tools/dev/wm-fuzz NAME --steps 192 --seed 2` on a fresh
  `2560x1600,1920x1080 --session --shell` session with a fresh home):
  its tile state and its frame part ways somewhere earlier (grouping,
  arranging, a display moved); find where.

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
