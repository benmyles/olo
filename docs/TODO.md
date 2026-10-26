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
  cup or Preferences holds a logind inhibitor. Display ▸ Brightness. Still
  to do: an on-screen level indicator for the brightness / volume keys,
  keyboard backlight, real DPMS off instead of backlight 0.
- ~~Preferences ▸ Sound~~: output/input device, level, mute, live from the
  sound server (`tabula_system::audio::{devices, watch_devices, set_*}`).

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

- Terminal: find in scrollback; release-build memory check; colour emoji.
- Activity Monitor.
- Preferences, remaining panels: Network, Bluetooth, Date & Time (24-hour clock for Deskbar), Touch
  ID; Restart Tabula; a Choose… file panel for the
  wallpaper; Display mode switching (app_server ignores `mode` in
  ConfigureOutput so far) and drag-to-arrange.
- StyledEdit, DeskCalc.
- ~~**Screenshot**~~: Haiku's app (screen or active window, border,
  pointer, delay, five formats, folder choice, clipboard), ⌘⇧3/⌘⇧4/⌘⇧5
  and Print Screen; every backend captures. Region capture is still to do.
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

- Cursor shapes; night mode; libinput settings; centred alerts.

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
