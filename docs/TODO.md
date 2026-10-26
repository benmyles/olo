# Work list

What's left before Tabula is complete. Keep it current: strike items
as they land (with the commit), add new ones as they come up.

## Done recently

- ~~Viewer: pop-out from Quick Look, PDF forms, text, ink, signatures,
  Save / Save As / Export Flattened~~ (903ba8b, f01bff9).
- ~~Kit FilePanel~~ (903ba8b).

## Native rule (no wrapping system tools)

- ~~Keyboard mapping in app_server, replacing keyd~~: ⌘ as command,
  Caps Lock as Ctrl, ⌘Tab / ⌘` switching, ⌘ arrows / ⌘⌫, tap ⌘ /
  ⌘Space for Find, terminals keep physical Ctrl (`keymap.rs`). Still to
  do: the Twitcher overlay (switching works, nothing is drawn yet), a
  lock screen for Ctrl+⌘Q, and Preferences ▸ Keyboard toggles for the
  `KeyboardSettings` fields. keyd can then be disabled system-wide; while
  it runs it grabs the keyboards and app_server sees its output (handled:
  its ⌘ tap chord and Super+Tab are understood).
- Deskbar network: NetworkManager over D-Bus (zbus) instead of `nmcli`.
- Deskbar volume: PulseAudio/PipeWire protocol instead of `wpctl`.
- Kit opener (mimeapps.list + desktop entries) instead of `xdg-open`
  (kit platform, Terminal links, Tracker `fs::open`).
- app_server: settings without `gsettings`; session environment via
  zbus instead of `dbus-update-activation-environment`.

## Apps

- Terminal: find in scrollback; release-build memory check; colour emoji.
- Activity Monitor.
- Preferences: all M2 hardware panels (display, 2× default scale,
  keyboard, trackpad, sound, network, Bluetooth, power, Touch ID),
  Restart Tabula.
- StyledEdit, DeskCalc, Screenshot.
- Web browser (native).
- Password manager: Rust 1Password SDK, passkeys, Touch ID through
  fprintd natively, browser integration.
- Viewer: image zoom/pan, video playback (frames + audio), AVIF/HEIC/Opus.

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

- Cursor shapes; night mode; libinput settings; keybindings and
  Twitcher; centred alerts.

## Lock, screensaver, caffeinated

- **Screen lock** (ext-session-lock in app_server, a native lock client):
  password via PAM and Touch ID via fprintd over D-Bus, in Tabula's look;
  Ctrl+⌘Q, lock on lid close / sleep / idle timeout, and on screensaver
  exit after a configurable delay. Survives app_server reload and can
  never fail open (if the locker dies, the screen stays locked).
- **Screensaver**: idle detection in app_server (ext-idle-notify, and
  idle-inhibit honoured for video players), a screensaver host that runs
  modules full screen on every display, and Preferences ▸ Screen Saver
  (module list with live preview, start-after, lock-after, hot corners
  for "now" / "never"). Reproduce the stock BeOS R5 modules faithfully
  (verify the exact set and their look against R5 itself or period
  screenshots before building them; Blackness, Message, Spots and
  Lissart are believed to be among them).
- **Caffeinated mode**: one click (Deskbar replicant/tray toggle, plus a
  Deskbar menu item and a shortcut) keeps everything awake: no display
  sleep, system suspend, screensaver or idle lock (logind inhibitor
  locks over D-Bus, natively), optionally for a set time (1 h, until
  battery low, …). Visible state in Deskbar while on; lid-close still
  sleeps unless explicitly chosen.

## Other

- Deskbar 24-hour clock option.
- Icons: audio/system, spreadsheet, font and other new file kinds.
- Kit: asynchronous `read_clipboard`.
