# Work list

What's left before Tabula is complete. Keep it current: strike items
as they land (with the commit), add new ones as they come up.

## Done recently

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
  sound server (`tabula_system::audio::{devices, watch_devices, set_*}`).

- ~~Upgrades keep apps open~~ (only a new app_server reloads; Tabula
  programs re-execute in place), ~~interactive resize keeps up~~,
  ~~focus not stolen by xdg_activation without input~~ (15c2b27).
- ~~Keyring prompts are Tabula's~~: the polkit agent owns
  org.gnome.keyring.SystemPrompter (gcr's protocol and secret exchange,
  natively), so unlocking the login keyring (gh's git credentials hung
  on a GTK prompter that had lost its display) asks in a Tabula alert.
- ~~slack.com loads in NetPositive~~ (its 28 autoplaying videos made
  the page OOM): decoders budgeted per page, media downloads and `blob:`
  downloads on disk, compact frame queues, muted videos pause out of
  sight; pages end with NetPositive (they outlived it). Left: each video
  element keeps one full-size picture in Servo.
- ~~Login keyring in a real session~~ (the session finishes starting the
  daemon PAM unlocked), ~~Passwords lists 1Password's accounts~~.

## Native rule (no wrapping system tools)

- ~~nmcli, wpctl, systemctl, gsettings, dbus-update-activation-environment~~
  → `tabula-system` (NetworkManager, PulseAudio protocol with a change
  feed, logind, dconf, activation environment).

- ~~Keyboard mapping in app_server, replacing keyd~~: ⌘ as command,
  Caps Lock as Ctrl, ⌘Tab / ⌘\` switching, ⌘ arrows / ⌘⌫, tap ⌘ /
  ⌘Space for Find, terminals keep physical Ctrl (`keymap.rs`), Ctrl+⌘Q
  locks. keyd can now be disabled system-wide; while
  it runs it grabs the keyboards and app_server sees its output (handled:
  its ⌘ tap chord and Super+Tab are understood).
- ~~Kit opener instead of `xdg-open`~~: `tabula_xdg::open` (shared-mime-info
  globs + sniffing, our apps first, mimeapps.list, Exec field codes).

## Apps

- ~~**Passwords keeps its own store, synced with 1Password both ways**~~
  (Ben): going to 1Password for every look-up is slow and unreliable.
  Passwords should hold every item itself (encrypted at rest, unlocked
  with the account password or Touch ID) and answer the app, NetPositive
  and the palette from it at once; 1Password is only for syncing, both
  ways (changes either side merge by item version, conflicts kept),
  in the background and when asked. Done: `vault.rs`, `sync.rs`; try it
  on the real account (Ben).
- ~~**NetPositive fills Google's sign-in**~~ (Ben): two-step sign-ins
  fill the username alone, hidden decoy fields are skipped, the field
  menu has the logins at once (fetched when the page loads), and a field
  filled from the context menu is learnt for that page. Still to try on
  the real accounts.google.com (Ben).
- ~~**Tracker opens in column view by default**~~ (Ben), set in
  Preferences ▸ Tracker; open windows keep their own view.
- ~~**Deskbar: a click on a running app with several windows offers them**~~
  (Ben): one window comes forward at once; with more, the left click
  opens the same menu as the right click (each window, Show All).
- ~~**Find: Space shows the selected result in Quick Look**~~ (Ben), as in
  a Tracker window; the arrow keys step through the results.
- ~~**Tracker: File ▸ Open in Terminal**~~ (Ben): the folder shown (the
  active column's), or a right-clicked folder, in a new Terminal window;
  also in the window's and the desktop's context menus. When Terminal
  gets tabs, open a tab instead.
- ~~**Video plays the wrong parts at the wrong speed**~~ (Ben): OpenH264
  was flushed after every packet, breaking B-frame reordering; now
  `Flush::NoFlush`, mp4 edit lists applied, and WebM seeks fixed
  (vendored matroska-demuxer).
- ~~**Confirm before quitting apps with state that would be lost**~~
  (Ben): NetPositive asks before quitting or closing a window that would
  lose more than one tab or stop downloads; Terminal already asks before
  closing a window with a program running, and its Quit leaves the
  shells in tabula-termd; StyledEdit asks about unsaved documents.
- ~~**Passwords: never change anything on sync without approval**~~
  (Ben): plan, Review Sync, apply (sync.rs); plus ~~File ▸ Reset
  Passwords…~~ (local only).
- ~~**Terminal: ⌥←/⌥→ in Claude Code**~~ (fixed: under the kitty keyboard
  protocol they're now `CSI 1;3 D/C`) (Ben): Option+Left/Right move by a
  word in plain bash but not in Claude Code. Check what Terminal sends for
  them (bash binds ESC b / ESC f; others expect CSI 1;3D / 1;3C) and
  whether Claude Code's input understands it; send what xterm does.
- **Drag and drop everywhere, across apps** (Ben): everything that shows
  files or data can be dragged and dropped, within and between apps and
  to and from other Wayland clients (wl_data_device, text/uri-list and
  the data's own types): Tracker items to any app (e.g. an image into
  Terminal, so Claude Code gets its path), Tracker accepting drops from
  everything, NetPositive images/links/text, StyledEdit text, Viewer
  images, Passwords fields, Deskbar launches…
- ~~**Tracker: restore from the Trash**~~ (File ▸ Restore and the item
  menu, `ops::Kind::Restore`; a clash gets a number instead of asking) (Ben): a Restore command (File
  menu and the context menu in the Trash) that puts the selected files
  back where each came from (the trash's `.trashinfo` original path),
  recreating missing folders and asking on a name clash.
- ~~**NetPositive: logins for parent domains too**~~ (done: the whole site
  matched already; now nearest first, `tabula_keychain::closeness`) (Ben): on
  accounts.google.com, offer logins saved for any parent domain
  (*.google.com, google.com) as well as the exact host, sorted most
  specific first.
- **Passwords: sync with several 1Password accounts** (Ben): setup now
  makes you pick one account; Passwords should sync with every account
  the 1Password app has signed in (each account's vaults shown under it,
  items keeping which account and vault they belong to, per-account sync
  state and errors), with accounts added or removed later in its
  settings. The store and sync (`vault.rs`, `sync.rs`) key remote state
  by account as well as vault.
- **GNOME/GTK 4 apps look at home** (Ben): a GTK 4 / libadwaita app
  opened in Tabula follows light/dark mode properly (and switches live),
  and gets Tabula's window chrome (yellow tab, BeOS borders) rather than
  its own client-side decorations: e.g. asking for server-side decorations
  via xdg-decoration / GTK's settings, and whatever libadwaita needs
  (portal `org.freedesktop.appearance color-scheme`, dconf) to agree.
- **Settings & state backup: export/import in one file** (Ben): a native
  Tabula utility that exports every Tabula setting and piece of saved
  state, across all Tabula apps (Preferences settings, Tracker views,
  Deskbar, NetPositive bookmarks/history/learnt fields, Passwords' vault
  file, keymaps, window state, …), into a single compressed file, and
  imports one back (with a summary of what it holds and a confirm before
  replacing anything). In Rust, in process (e.g. tar + zstd crates);
  secrets stay encrypted as they are on disk.
- **Terminal tabs, tab groups and coding agents** (Ben; after everything
  else is done): tabs, tab groups that can be saved and reopened, and
  native recognition of coding agents (Claude Code, Codex) running in a
  tab, so tabs and groups show each agent's state (working, waiting for an
  answer, blocked) and notify when one needs attention. A tile-all mode
  shows every group at once, tiled, each group colour-coded. Design it
  properly (BeOS look, how tabs, groups and colours read).
- **Token pooling for Claude Code and Codex, natively** (Ben; after
  everything else is done): a Tabula version of ~/src/token-pooler's Local
  mode, without its team/cloud gateway: the local accounts' subscription
  credentials kept in Tabula (Passwords' store), a loopback gateway the
  coding clients are pointed at, accounts drained in a reset-aware order,
  usage recorded (never prompts or completions), and a Deskbar replicant
  or app showing the pool live in place of its macOS menu bar.
- **Rename Tabula to Olo, use the new brand, ship a wallpaper pack**
  (Ben; the very last thing, after everything else is done):
  - *Rename* everywhere (code, crates, binaries, app ids, environment
    variables, paths, the session entry, install tooling, UI text, docs);
    nothing still calls itself Tabula except history (the journal) and
    compatibility shims kept on purpose. Public places (README, About,
    session entry, website copy) say "Olo Desktop" (Olo Inc. holds "OLO"
    software trademarks); plain `olo` in code. Domain olodesktop.com.
    Existing installs carry on with nothing lost (settings, the browser
    profile, passwords and keychain access, saved window state), and a
    running session upgrades or reloads into the renamed build as it was:
    work out the migrations and test from a session started on the old
    build. Leave the repo directory and GitHub repo name (Ben renames them).
  - *Brand* from ~/olo-logos/ (overview: kit/brand-sheet.png): wordmark
    icons4/bounce-wordmark-2.png, mark icons5/yellow-sides-1.png, desktop
    icon kit/candidates/desktop-icon-2.png (through the docs/ICONS.md
    pipeline), and the finished kit/ (light/dark and single-colour
    wordmarks, flat and single-colour marks, app badges, favicons,
    lockups, social and GitHub images; each with its prompt .json). Use
    them for the app and desktop icon, Deskbar, About, README and
    wherever the old identity shows.
  - *Wallpapers* from ~/olo-logos/wallpapers/{4k (3840×2160), ultrawide
    (3840×1648)} (not v1/ or dropped/): dusk-gradient (default; light and
    -dark), boing (light and -dark), be-blue, aqua, clouds,
    workbench-pattern (one image each). Preferences picks from the pack
    with previews (thumbnails loaded in the background) and still takes
    a file of your own; light/dark versions follow the system mode live
    (Auto's schedule too); each screen gets the 4K or ultrawide version by
    its shape; a wallpaper already chosen is kept. The PNGs (about 93 MB)
    go in Git LFS. Never
    decode 4K on the UI thread; pixel-perfect light/dark/night at 2×;
    survive reload; shared widgets in the kit; tsession; journal.
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
  (app_server); a mouse wheel notch eases out in NetPositive; taps reach
  every kit control as a press then a release; two-finger tap is the
  secondary click, no middle-click emulation or drag lock.
- ~~**⌘A in a text field**~~ (Ben) selects the field's text, not the
  page or folder.
- ~~**NetPositive dark pages**~~ (Ben): View ▸ Dark Pages; per-item recolouring in WebRender as Chrome's Auto Dark Mode does it; pictures true, icons classified.

- **No GNOME or GTK underneath** (Ben): Tabula shouldn't depend on
  any. Today a real session leans on GDM (login), gnome-keyring-daemon
  (secrets, PAM unlock, ssh agent), xdg-desktop-portal-gnome / -gtk (file chooser, screenshot,
  settings, etc. for other apps: there's no Tabula portals.conf or
  backend), dconf (where app_server writes color-scheme / gtk-theme so
  GTK apps follow). ~~Wallpapers~~: none needed (the desktop is BeOS's
  own colours per workspace; Choose… starts in Pictures). Replace each
  with a native one: a Tabula login screen, secret service (Secret
  Service API over our own bus name, unlocked at login, keyring files
  read natively; the prompt is done), ssh agent, portal backend
  (`org.freedesktop.impl.portal.*` with Tabula's FilePanel and
  screenshot), settings portal. Needs Ben's go-ahead and order: these
  replace what his real session runs on. On Aurora, xdg-desktop-portal
  1.22 reads backends only from /usr/share/xdg-desktop-portal/portals
  (read-only) or `XDG_DESKTOP_PORTAL_DIR` (replacing it whole), and GTK
  apps outside a sandbox read the colour scheme from dconf, not the
  portal, so dconf stays until GTK apps are gone.
- ~~**Terminal sessions that outlive Terminal**~~ (Ben): done. Shells
  live in tabula-termd (one per display); quitting, crashing or upgrading
  Terminal reattaches every window with its scrollback, and tabula-termd
  upgrades in place too. Terminal ▸ Terminal Daemon… shows it and can
  restart it (keeping the shells) or start it over. Images come back
  too.
- Terminal: ~~find in scrollback~~ (Edit ▸ Find…, Find Again, Find
  Selection; regex over screen and history); ~~release-build memory
  check~~ (fonts mapped, wgpu MemoryUsage: every app ~100 MB smaller);
  ~~colour emoji~~ (tabula-emoji, COLRv1 via skrifa + tiny-skia).
- ~~Activity Monitor~~: apps/activity (graphs of every /proc, /sys and
  hwmon source; processes with Quit / Force Quit).
- Preferences, remaining panels: ~~Network~~ (Wi-Fi list, join with
  password, disconnect, forget, interface details; ~~WPA-Enterprise~~: PEAP/TTLS/TLS
  sheet), ~~Bluetooth~~ (search, pair with passkey/PIN agent, connect,
  remove, battery), ~~Date & Time~~ (calendar, clock face, network
  time, time zone, Deskbar 24-hour/seconds/day), ~~Touch ID~~ (enrol with progress, remove); ~~Restart Tabula~~; ~~a Choose… file panel for the
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
- ~~Web browser~~: **NetPositive** (apps/browser) on Servo, in process,
  drawn into a kit embedded subsurface: tabs, location field with search,
  back/forward/reload/stop/home, zoom, bookmarks and history menus, page
  context menus, `<select>`, dialogs, logins, file uploads, haiku error
  pages, in-place reload, ~~find in page~~ (a script, since Servo 0.7
  has no API), ~~downloads~~ (our own HTTP, as Servo 0.7 has no download
  API), ~~scroll bars~~ (Servo draws none; a script reports
  the scroll), ~~favicons in tabs~~. ~~dark pages~~ (a style per page, as
  Servo misapplies the theme), ~~audio/video in pages~~ (our own servo-media backend on Viewer's
  decoders), ~~a process and systemd unit per site~~ (memory limit, CPU
  weights, stuck-page bar; guard.rs), ~~Servo's unfinished-but-working web
  features on~~, ~~cameras, microphones and WebRTC calls~~ (rtc/: H.264 and
  Opus, data channels, stats; loopback tested). Still to do: trying Google Meet and Zoom themselves,
  permission prompts (Servo 0.7 implements none of the features behind
  them but ~~notifications~~, which ask per site and go to the notification centre), ~~a colour picker for `<input type=color>`~~,
  ~~password manager integration~~.
- Password manager: ~~Rust 1Password SDK~~ (desktop-app integration;
  fake server for tests), ~~Passwords app~~ (browse, copy, edit, create,
  archive/delete, generator, lock with ~~Touch ID through fprintd~~ or
  password), ~~browser integration~~ (NetPositive fills from the key
  button and offers to save logins used), ~~passkeys~~ (WebAuthn in
  NetPositive, keys made and kept by Passwords in 1Password items).
- Viewer: ~~image zoom/pan~~ (full resolution up to the texture limit,
  View menu, pinch/⌘-scroll/double-click, drag and scroll to pan; halved
  levels for smooth fitting); ~~video playback (frames + audio)~~ (H.264 in mp4/Matroska, AV1 in
  Matroska/WebM, synced to the sound, seeking, keeps the screen awake;
  VP9, HEVC and AV1-in-mp4 still show facts only), ~~AVIF/HEIC/Opus~~.

## app_server

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

- ~~Cursor shapes~~ (Haiku's cursors via tabula-hvif, cursor-shape-v1);
  ~~night light~~ (CRTC gamma ramps, faded); ~~libinput settings~~ (touchpad tap, tap-drag, dwt, click
  method, natural scroll, speed; udev only); ~~centred alerts~~
  (`WindowOptions::alert`, set_parent, placed over the parent).

## Lock, screensaver, caffeinated

- ~~**Screen lock**~~: ext-session-lock in app_server, `tabula-lock`
  (PAM + Touch ID), Ctrl+⌘Q / Deskbar / idle display-off / sleep / logind
  Lock; never fails open; survives reload (see HACKING "The lock screen").
  Lid close goes through logind's suspend, so it is covered when the lid
  sleeps.
- ~~**Screensaver**~~: the eleven R5 modules (tabula-screensavers),
  tabula-screensaver on every output, Preferences ▸ Screen Saver (Fade and
  Modules tabs, live preview, Test, hot corners), lock-after drawing over the
  lock screen; Flip and Lens see the desktop on every backend.
- ~~**Caffeinated mode**~~: Deskbar cup (click; right-click or Tabula ▸
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
