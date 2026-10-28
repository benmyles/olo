# Work list

What's left before Olo is complete. Keep it current: strike items
as they land (with the commit), add new ones as they come up.

## Done recently

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
  tapping ⌃⇧ letters every control, menu item, list row, icon, window and
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
  Caps Lock as Ctrl, ⌘Tab / ⌘\` switching, ⌘ arrows / ⌘⌫, tap ⌘ /
  ⌘Space for Find, terminals keep physical Ctrl (`keymap.rs`), Ctrl+⌘Q
  locks. keyd can now be disabled system-wide; while
  it runs it grabs the keyboards and app_server sees its output (handled:
  its ⌘ tap chord and Super+Tab are understood).
- ~~Kit opener instead of `xdg-open`~~: `olo_xdg::open` (shared-mime-info
  globs + sniffing, our apps first, mimeapps.list, Exec field codes).

## Apps

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

- ~~**Window menu on the title tab**~~ (Ben; done: HACKING, "The command
  palette", window menu): right-clicking a window's yellow tab opens a
  kit menu of Hide, Zoom, Back to Its Own Size, Center, Tile ▸, Move to
  Workspace ▸, the displays, Send to Back and Close, the shortcuts' own
  commands. Not yet: Always on Top (app_server has no such state) and
  "tile with…" (tiling beside a chosen window).
- **Multiple monitors, managed like macOS Displays.** Hotplug of external
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

## Other

- ~~Deskbar 24-hour clock option~~ (Preferences ▸ Date & Time ▸ Clock).
- ~~**polkit authentication agent**~~: apps/polkit-agent (real sessions
  only; fprintd then password through polkit-agent-helper-1).
- ~~Icons for new file kinds~~: spreadsheet, presentation, word-document,
  pdf, font (audio, video, archive already had theirs).
- ~~Kit: asynchronous `read_clipboard`~~ (`paste` reads on a helper
  thread and delivers the Paste event through a loop channel).
