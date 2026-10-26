# Work list

What's left before Tabula is complete. Keep it current: strike items
as they land (with the commit), add new ones as they come up.

## Done recently

- ~~Viewer: pop-out from Quick Look, PDF forms, text, ink, signatures,
  Save / Save As / Export Flattened~~ (903ba8b, f01bff9).
- ~~Kit FilePanel~~ (903ba8b).

## Native rule (no wrapping system tools)

- **Keyboard mapping in app_server, replacing keyd.** ⌘ acts as the
  command key (Ctrl on the wire for apps), Caps Lock as Ctrl, ⌘Tab /
  ⌘` switch apps and windows (Twitcher), ⌘←/→/↑/↓ and ⌘⌫ text
  navigation, tap ⌘ / ⌘Space for search, Ctrl+⌘Q lock. Terminal keeps
  physical Ctrl for the tty. Configurable in Preferences ▸ Keyboard; the
  keyd service and its GNOME pieces are no longer needed in a Tabula
  session.
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

- Cursor shapes; night mode; libinput settings; keybindings and
  Twitcher; centred alerts.

## Other

- Deskbar 24-hour clock option.
- Icons: audio/system, spreadsheet, font and other new file kinds.
- Kit: asynchronous `read_clipboard`.
