# Screenshot journal

Tabula's visual evolution, newest first. Added with `tools/journal.py`
(images are stored with Git LFS; run `git lfs pull` after cloning).

## 2026-10-26

### Backgrounds in dark mode

*02:31* — The chosen wallpaper survives an in-place reload via About Tabula, Restart Tabula.

![Backgrounds in dark mode](2026-10-26/023141-backgrounds-in-dark-mode.png)

### Choose a Wallpaper panel

*02:31* — The kit FilePanel as a child of Backgrounds, limited to picture types, kept on screen when wider than its parent.

![Choose a Wallpaper panel](2026-10-26/023141-choose-a-wallpaper-panel.png)

### Wallpaper drawn by app_server

*02:31* — Backgrounds sets a JPEG XL wallpaper; app_server decodes and scales it to cover the output on a worker thread and draws it under every surface.

![Wallpaper drawn by app_server](2026-10-26/023137-wallpaper-drawn-by-app-server.png)

### Touch ID: enrolling (dark)

*02:19* — Progress per touch with the sensor guidance; Cancel stops the sensor.

![Touch ID: enrolling (dark)](2026-10-26/021930-touch-id-enrolling-dark.png)

### Preferences: Touch ID

*02:19* — Enrolled fingers, add one (likeliest finger first), remove with confirmation (fake sensor).

![Preferences: Touch ID](2026-10-26/021930-preferences-touch-id.png)

### Preferences: Bluetooth

*02:12* — Power, search (30 s), named and paired devices with type, status and battery; pairing questions appear in the panel.

![Preferences: Bluetooth](2026-10-26/021218-preferences-bluetooth.png)

### Preferences: Network

*02:06* — Wi-Fi networks in range with security and signal; the one in use ticked, with Disconnect and Forget.

![Preferences: Network](2026-10-26/020623-preferences-network.png)

### polkit agent: password

*02:02* — After the fingerprint times out PAM asks for the password; the field takes focus.

![polkit agent: password](2026-10-26/020241-polkit-agent-password.png)

### polkit agent

*02:02* — Authentication Required: polkit message, Touch ID through PAM first (fprintd), new padlock icon.

![polkit agent](2026-10-26/020241-polkit-agent.png)

### Date & Time (dark)

*01:54* — Calendar and clock face; disabled while network time is on.

![Date & Time (dark)](2026-10-26/015406-date-time-dark.png)

### Date & Time: time zones

*01:54* — Every timedated zone with its offset and local time, searchable; opens on the current zone.

![Date & Time: time zones](2026-10-26/015406-date-time-time-zones.png)

### Date & Time: Deskbar clock

*01:54* — 24-hour clock, seconds and weekday options; Deskbar follows the setting live.

![Date & Time: Deskbar clock](2026-10-26/015406-date-time-deskbar-clock.png)

### Alerts open over their window

*01:44* — Alerts are children of the focused window; app_server centres them across it a quarter of the way down, as BAlert did.

![Alerts open over their window](2026-10-26/014403-alerts-open-over-their-window.png)

### Activity Monitor: dark mode

*01:41* — Graphs and legend in dark mode.

![Activity Monitor: dark mode](2026-10-26/014117-activity-monitor-dark-mode.png)

### Activity Monitor: permission alert

*01:41* — Force Quit of another user's process shows a stop alert; nothing is signalled.

![Activity Monitor: permission alert](2026-10-26/014117-activity-monitor-permission-alert.png)

### Activity Monitor: processes

*01:41* — Sortable process list with search by name or ID and an owner filter.

![Activity Monitor: processes](2026-10-26/014117-activity-monitor-processes.png)

### Activity Monitor: graphs

*01:41* — CPU, memory and cache with Total System Power from hwmon; network and disk rates below.

![Activity Monitor: graphs](2026-10-26/014116-activity-monitor-graphs.png)

### Alerts get their icons

*01:31* — The warning, stop, info and idea alert icons were missing from the set (every alert drew none); generated against the reference set.

![Alerts get their icons](2026-10-26/013135-alerts-get-their-icons.png)

### StyledEdit in dark mode after a reload

*01:31* — Unsaved text and styles came back after app_server reloaded and the app re-executed; default text follows the theme, explicit colours stay.

![StyledEdit in dark mode after a reload](2026-10-26/013135-stylededit-in-dark-mode-after-a-reload.png)

### StyledEdit: selection keeps its colours

*01:31* — egui paints selected text in one colour; StyledEdit draws the selected glyphs again over the highlight in their own colours.

![StyledEdit: selection keeps its colours](2026-10-26/013135-stylededit-selection-keeps-its-colours.png)

### StyledEdit: fonts, sizes and colours

*01:31* — StyledEdit with a bold red word and a 24 pt Comfortaa word in the system font's line; styles live in the file's user.tabula.styles attribute, the text stays plain.

![StyledEdit: fonts, sizes and colours](2026-10-26/013135-stylededit-fonts-sizes-and-colours.png)

### DeskCalc scientific (dark)

*01:02* — Haiku's scientific rows above R5's keys; options in the context menu.

![DeskCalc scientific (dark)](2026-10-26/010211-deskcalc-scientific-dark.png)

### DeskCalc (light)

*01:02* — R5's keypad under an expression line; 2*(3+4) worked out.

![DeskCalc (light)](2026-10-26/010211-deskcalc-light.png)

### Screenshot: active window (dark)

*00:53* — Active-window capture with the tab's shape kept; JPEG into a folder chosen with the new folder panel.

![Screenshot: active window (dark)](2026-10-26/005304-screenshot-active-window-dark.png)

### Screenshot (light)

*00:53* — Haiku-style Screenshot app: preview, capture options, name/format/folder; restored after an in-place reload.

![Screenshot (light)](2026-10-26/005304-screenshot-light.png)

### Screen saver over the lock screen

*00:31* — Locks underneath after the delay; the first input ends the saver and shows the lock.

![Screen saver over the lock screen](2026-10-26/003101-screen-saver-over-the-lock-screen.png)

### Preferences: Screen Saver, Modules

*00:31* — Live preview in the monitor, module list, settings and Test (dark).

![Preferences: Screen Saver, Modules](2026-10-26/003101-preferences-screen-saver-modules.png)

### Preferences: Screen Saver, Fade

*00:31* — R5's Fade tab: run after, turn off screen, password lock, Fade now / Don't fade corners.

![Preferences: Screen Saver, Fade](2026-10-26/003100-preferences-screen-saver-fade.png)

### Screen saver modules

*00:31* — BeOS R5's modules redrawn: BuyNow, Flip, Icons, Lens, Lissart, Message, Spiral, Spots, StringTheme, SuperString (plus Blackness).

![Screen saver modules](2026-10-26/003100-screen-saver-modules.png)

### Power: lock when the display turns off

*00:07* — New option, on by default.

![Power: lock when the display turns off](2026-10-26/000701-power-lock-when-the-display-turns-off.png)

### Deskbar: Lock Screen

*00:07* — Lock Screen in the Tabula menu; the off cup in the tray is now visible in dark mode.

![Deskbar: Lock Screen](2026-10-26/000701-deskbar-lock-screen.png)

### Lock screen: wrong password

*00:07* — A wrong password shakes the panel and says so (light); PAM runs on a thread.

![Lock screen: wrong password](2026-10-26/000701-lock-screen-wrong-password.png)

### Lock screen

*00:07* — tabula-lock over ext-session-lock: clock, date, and a BeOS panel asking for the password or Touch ID (dark).

![Lock screen](2026-10-26/000701-lock-screen.png)

## 2026-10-25

### Deskbar caffeinated cup

*23:43* — The tray cup: empty when sleep is allowed, full and steaming while caffeinated.

![Deskbar caffeinated cup](2026-10-25/234329-deskbar-caffeinated-cup.png)

### Display ▸ Brightness

*23:43* — The built-in display's brightness slider (simulated backlight in a headless session).

![Display ▸ Brightness](2026-10-25/234328-display-brightness.png)

### Preferences ▸ Power

*23:43* — Battery health and state, idle display-off and sleep times, caffeinated mode (light and dark).

![Preferences ▸ Power](2026-10-25/234328-preferences-power.png)

### Preferences ▸ Sound

*23:30* — Output and input device, level and mute, read live from the sound server (light and dark).

![Preferences ▸ Sound](2026-10-25/233013-preferences-sound.png)

### Preferences launcher

*23:20* — tabula-preferences with no argument: a grid of every panel.

![Preferences launcher](2026-10-25/232029-preferences-launcher.png)

### Preferences panels (dark)

*23:20* — Appearance, Keyboard, Touchpad, Backgrounds, Display and About in dark mode: one fixed-size window per panel with Defaults/Revert.

![Preferences panels (dark)](2026-10-25/232029-preferences-panels-dark.png)

### Preferences panels (light)

*23:20* — Appearance, Keyboard, Touchpad, Backgrounds, Display and About in light mode: one fixed-size window per panel with Defaults/Revert.

![Preferences panels (light)](2026-10-25/232028-preferences-panels-light.png)

### Twitcher (dark)

*22:51* — The same in dark mode.

![Twitcher (dark)](2026-10-25/225144-twitcher-dark.png)

### Twitcher (light)

*22:51* — ⌘Tab held: app_server draws the BeOS Twitcher strip with each running app's icon and the chosen app's name.

![Twitcher (light)](2026-10-25/225144-twitcher-light.png)

### Quick Look pop-out to Viewer

*22:34* — Popping a Quick Look out of Tracker opens the file in a standalone Viewer window (dark).

![Quick Look pop-out to Viewer](2026-10-25/223446-quick-look-pop-out-to-viewer.png)

### Kit FilePanel: Export Flattened PDF

*22:34* — BFilePanel-style Save panel: path popup, Home, Name/Size/Modified list, Save as field.

![Kit FilePanel: Export Flattened PDF](2026-10-25/223446-kit-filepanel-export-flattened-pdf.png)

### Viewer: PDF form in dark mode

*22:34* — The same form in dark: Draw tool active, ink mark and Text tool date beside the signature.

![Viewer: PDF form in dark mode](2026-10-25/223446-viewer-pdf-form-in-dark-mode.png)

### Viewer: PDF form filling (light)

*22:34* — Text, comb, combo, list box, radio and check fields filled in place, with a placed signature.

![Viewer: PDF form filling (light)](2026-10-25/223446-viewer-pdf-form-filling-light.png)

### Quick Look: every format (dark)

*22:00* — Contact sheet: MP3/OGG with tags and playback, Markdown, JPEG, tar.gz, SVG, PDF (hayro), CSV, PNG, Word, zip, Rust. All decoded in process, with no helper tools.

![Quick Look: every format (dark)](2026-10-25/220050-quick-look-every-format-dark.png)

### Quick Look: source code (dark)

*22:00* — Text and code coloured by syntax with the shared kit syntax palette (the same hues as Terminal), line numbers, virtualised rows.

![Quick Look: source code (dark)](2026-10-25/220049-quick-look-source-code-dark.png)

### Quick Look: images

*22:00* — Space on a selected item opens Quick Look; the window fits the screen (app_server now keeps newly sized windows on screen) and follows the selection.

![Quick Look: images](2026-10-25/220049-quick-look-images.png)

### Terminal (dark)

*21:36* — The same session in Tabula Dark (graphite, soft pastels). The scheme follows the desktop theme, and OSC 10/11 colour queries answer from it, so TUIs like Claude Code choose the matching theme.

![Terminal (dark)](2026-10-25/213633-terminal-dark.png)

### Terminal (light)

*21:36* — Tabula Light scheme: 16 ANSI colours tuned for contrast on paper white, SGR attributes and truecolor, CJK/Greek/symbol fallback fonts, box drawing drawn as geometry (rounded and heavy), block elements, a kitty-protocol inline image, and a compiler-style path link.

![Terminal (light)](2026-10-25/213633-terminal-light.png)

### Tracker: live updates

*21:35* — Folder windows, columns and the desktop now follow the disk as it changes via inotify, with no polling. Shot 120 ms after a shell wrote download.iso, renamed notes.md, deleted photo-02.jpg and made Archive. Selection and sort are kept; a deleted folder says so.

![Tracker: live updates](2026-10-25/213524-tracker-live-updates.png)

### In-place reload: after an upgrade

*21:01* — After tabula-session reload with new tabula-session, app_server and Tracker binaries: the session re-executed itself, restarted app_server, and Tracker re-executed with its saved state. Pixel-identical to the screenshot before.

![In-place reload: after an upgrade](2026-10-25/210132-in-place-reload-after-an-upgrade.png)

### In-place reload: before

*21:01* — Tracker (desktop + Home window, Home icon selected) and Deskbar under tabula-session, before rebuilding and running tabula-session reload.

![In-place reload: before](2026-10-25/210132-in-place-reload-before.png)

### Tracker Status: background file operations

*20:43* — Copy, move, duplicate, Trash and Empty Trash run on worker threads (copy_file_range in chunks). Jobs longer than 0.4 s appear here with bytes, items, current file and time left; Stop cancels and removes the partial copy.

![Tracker Status: background file operations](2026-10-25/204337-tracker-status-background-file-operations.png)

### Tracker: Find searches contents with ripgrep

*20:38* — Find is built on ripgrep's crates (ignore + grep-searcher): parallel walk, smart case, .gitignore-aware, binary files skipped. Contents matches show the line number and text; live progress, Stop/Escape cancel.

![Tracker: Find searches contents with ripgrep](2026-10-25/203834-tracker-find-searches-contents-with-ripgrep.png)

### Menus: shortcut glyphs on the baseline

*20:29* — ⌘, ⇧ and arrows come from a fallback font; each is now laid out separately so it shares the item's baseline. The Trash is titled Trash.

![Menus: shortcut glyphs on the baseline](2026-10-25/202952-menus-shortcut-glyphs-on-the-baseline.png)

### Tracker: Find

*20:29* — Find takes focus on open, closes with ⌘W, shows home-relative locations and a baseline-aligned status.

![Tracker: Find](2026-10-25/202952-tracker-find.png)

### Tracker: column view, dark

*20:20*

![Tracker: column view, dark](2026-10-25/202049-tracker-column-view-dark.png)

### Tracker: column view

*20:20* — macOS-style column view (⌘4): one column per folder level, auto-hiding scroll bars, and a preview pane with an async thumbnail and file facts.

![Tracker: column view](2026-10-25/202047-tracker-column-view.png)

### Tracker: editing a name in place

*20:14* — BeOS-style inline rename replaces the Rename dialog: the field sits on the label's own baseline and lines up with the selection highlight; the name without its extension starts selected.

![Tracker: editing a name in place](2026-10-25/201411-tracker-editing-a-name-in-place.png)

### Tracker mini icon view

*19:58*

![Tracker mini icon view](2026-10-25/195831-tracker-mini-icon-view.png)

### Tracker list view

*19:58* — Sortable ColumnListView with 16 px icons; count and free space in the bar beside the scroll bar.

![Tracker list view](2026-10-25/195830-tracker-list-view.png)

### Tracker desktop, folder window and Get Info (light)

*19:58* — Selected labels inverted as on BeOS, highlight balanced on cap height and baseline; links in italics; executable scripts get the executable icon.

![Tracker desktop, folder window and Get Info (light)](2026-10-25/195830-tracker-desktop-folder-window-and-get-info-light.png)

### Tracker and Get Info in dark mode

*19:58* — Get Info rows on an exact LABEL_ROW + TIGHT grid, window sized to its content.

![Tracker and Get Info in dark mode](2026-10-25/195829-tracker-and-get-info-in-dark-mode.png)

### First Tracker desktop

*19:58* — Volumes and Home beside the Deskbar, Trash in the corner. Labels were off-centre (centred galleys double-offset) and ~/Desktop items were missing.

![First Tracker desktop](2026-10-25/195828-first-tracker-desktop.png)

### Icon set after the style guide

*19:58* — Generated against the reference set (docs/ICONS.md): the outliers are now three-quarter view like the rest.

![Icon set after the style guide](2026-10-25/195828-icon-set-after-the-style-guide.png)

### Icon set before the style guide

*19:58* — Text-only prompts: about, bluetooth, screenshot, display and trash drift to flat or straight-on views.

![Icon set before the style guide](2026-10-25/195828-icon-set-before-the-style-guide.png)
