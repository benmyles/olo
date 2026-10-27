# Screenshot journal

Tabula's visual evolution, newest first. Added with `tools/journal.py`
(images are stored with Git LFS; run `git lfs pull` after cloning).

## 2026-10-26

### Security preferences

*23:55* — Preferences ▸ Security holds how Passwords keeps its secrets: sync with 1Password automatically or only when asked, lock after idle, lock when the screen locks, and how soon copied secrets leave the clipboard. Passwords follows them live.

![Security preferences](2026-10-26/235516-security-preferences.png)

### Band selection in lists

*23:46* — Dragging from a row's other columns, or from below the rows, draws a selection band in Tracker's list view and in every kit list (⇧ or ⌘ adds; it scrolls on near the edges). The column view does the same from below a column's items.

![Band selection in lists](2026-10-26/234628-band-selection-in-lists.png)

### Drag and drop

*23:12* — Files drag between Tracker's views, the desktop, other apps and other Wayland programs, BeOS-style: a see-through picture with the name, folders light up to take them, move within a disk and copy across, a right-button drag asks Move/Copy/Link. Desktop icons stay where you put them. Drop a file on Terminal to type its path (an image into Claude Code).

![Drag and drop](2026-10-26/231257-drag-and-drop.png)

### Passwords: several 1Password accounts

*22:40* — Passwords now syncs with every 1Password account you tick (File ▸ 1Password Accounts…), in one review. The sidebar lists each account's vaults under its name, a vault is named with its account wherever it could be confused, and removing an account asks first and only removes its items from this computer.

![Passwords: several 1Password accounts](2026-10-26/224028-passwords-several-1password-accounts.png)

### Passwords: every vault, and sync problems you can read

*22:26* — 1Password won't let other apps read archived items, and one archived item failed a whole batch of fifty, so most of Personal and Private never arrived. Now only active items are fetched, an item archived there is archived here from its listing, and a failed batch is retried one item at a time. What still can't sync is listed in a Sync Problems window, grouped by vault, each with its reason, instead of a wall of red text.

![Passwords: every vault, and sync problems you can read](2026-10-26/222604-passwords-every-vault-and-sync-problems-you-can-.png)

### Restore from the Trash

*22:16* — Items in the Trash can go back where they came from: Restore in the File menu or an item's menu puts them back in their folders (made again if gone), and never over something new in their place.

![Restore from the Trash](2026-10-26/221633-restore-from-the-trash.png)

### Passwords asks before syncing anything

*22:08* — Syncing with 1Password now works out every change first, both ways (adding, changing, archiving, deleting, with the parts that differ), and makes none until you approve them in Review Sync. Automatic syncs only say how many changes are waiting.

![Passwords asks before syncing anything](2026-10-26/220845-passwords-asks-before-syncing-anything.png)

### NetPositive asks before quitting

*22:00* — Quitting NetPositive (or closing a window) with several tabs open, or downloads still running, now asks first, so a stray ⌘Q doesn't throw the tabs away.

![NetPositive asks before quitting](2026-10-26/220007-netpositive-asks-before-quitting.png)

### Videos with B-frames play properly

*21:54* — H.264 High videos with B-frames (most from screen recorders and x264) played with good sound but a picture that jumped and stalled: OpenH264 was asked to flush a picture after every packet, which broke reordering, so every B-frame failed. Now every picture decodes, in display order and in step with the sound (edit lists applied); seeking WebM/AV1 lands on the right frame too.

![Videos with B-frames play properly](2026-10-26/215448-videos-with-b-frames-play-properly.png)

### Passwords: changes wait while 1Password is away

*21:46* — With 1Password not running, changes are kept and the status line counts what's still to send; they go up on the next sync.

![Passwords: changes wait while 1Password is away](2026-10-26/214624-passwords-changes-wait-while-1password-is-away.png)

### Passwords: its own store, synced with 1Password

*21:46* — Items are read from the local encrypted store at once; 1Password is only for syncing, in the background. The status line shows the last sync.

![Passwords: its own store, synced with 1Password](2026-10-26/214623-passwords-its-own-store-synced-with-1password.png)

### Passwords: setup offers sync or local

*21:46* — Passwords now keeps its own encrypted store. The first time, it asks whether to sync with a 1Password account (both ways) or keep items only on this computer.

![Passwords: setup offers sync or local](2026-10-26/214623-passwords-setup-offers-sync-or-local.png)

### Tracker: Open in Terminal

*21:30* — File ▸ Open in Terminal, and the context menus, open a new Terminal window in the folder shown, or in the right-clicked folder (here alpha).

![Tracker: Open in Terminal](2026-10-26/213004-tracker-open-in-terminal.png)

### Find: Space for Quick Look

*21:27* — In Tracker's Find window, Space shows the selected result in Quick Look and the arrow keys step through the results, as in a folder window. A space typed into the Find field still goes into the query.

![Find: Space for Quick Look](2026-10-26/212739-find-space-for-quick-look.png)

### Deskbar: a click lists an app's windows

*21:27* — Clicking an application with more than one window in Deskbar now opens its window menu to pick one (or Show All); with one window it comes forward at once, as before.

![Deskbar: a click lists an app's windows](2026-10-26/212739-deskbar-a-click-lists-an-app-s-windows.png)

### Preferences ▸ Tracker

*21:22* — New Tracker windows open in column view by default; Preferences has a Tracker panel to choose the view new windows open in, and Tracker follows a change at once.

![Preferences ▸ Tracker](2026-10-26/212259-preferences-tracker.png)

### NetPositive learns login fields

*21:16* — The field menu now lists the page's logins at once (fetched when the page loads). Filling an odd field from it — here 'who are you' as the username — is remembered for that page, so the key button fills the same field next time. Hidden decoy fields, as on Google's sign-in, are skipped.

![NetPositive learns login fields](2026-10-26/211645-netpositive-learns-login-fields.png)

### Tracker: column view scrolls sideways

*20:50* — Columns wider than the window scroll sideways with the touchpad, Shift+wheel or a scroll bar along the bottom, and stay where they were put; the view only follows the keyboard when the selection moves to another column.

![Tracker: column view scrolls sideways](2026-10-26/205009-tracker-column-view-scrolls-sideways.png)

### Quick Look: F for fullscreen

*20:45* — In Quick Look, F fills the screen with the preview and F again puts it back in its window at the size it had (Cx::set_fullscreen, new in the kit).

![Quick Look: F for fullscreen](2026-10-26/204533-quick-look-f-for-fullscreen.png)

### NetPositive: dark pages, the Chrome way

*20:45* — Dark pages redone as Chrome's Auto Dark Mode for Web Contents: WebRender recolours what each thing is (light backgrounds and borders go dark, dark text light, by CIELAB lightness with hue kept), so photos, gradients and coloured buttons keep their true colours and only icons that Chrome's classifier would flip are inverted. Left: light desktop; right: dark.

![NetPositive: dark pages, the Chrome way](2026-10-26/204524-netpositive-dark-pages-the-chrome-way.png)

### Command palette (⌘Space)

*20:44* — ⌘Space opens a Raycast-style palette over everything: apps, the front app's menu commands, files, Passwords items, Preferences, windows, a calculator and every desktop shortcut with its keys, so typing "half" doubles as a reminder of ⇧⌘←.

![Command palette (⌘Space)](2026-10-26/204452-command-palette-space.png)

### NetPositive: dark pages

*19:47* — View ▸ Dark Pages (Automatic, the default, follows the desktop; Always; Never). Each frame is redrawn on the GPU through a shader that turns lightness around and keeps hues, so every page goes dark, blank ones included; pictures, video and canvases are pre-inverted with the CSS filter the shader undoes, and pages that are dark already are left alone. Left: Never; right: Always.

![NetPositive: dark pages](2026-10-26/194719-netpositive-dark-pages.png)

### NetPositive: web notifications

*19:16* — Sites can show notifications: the first time one asks, a bar over its tab offers Allow or Don’t Allow (remembered per site); allowed notifications appear in the desktop's notification centre, and a click brings the tab forward.

![NetPositive: web notifications](2026-10-26/191658-netpositive-web-notifications.png)

### Notifications

*19:00* — A native notification centre: apps' notifications (org.freedesktop.Notifications) appear as cards in the lower right with the app's icon, the summary, the body and their actions. A click is the default action; they go by themselves unless critical, and wait while the pointer is over them.

![Notifications](2026-10-26/190052-notifications.png)

### Keyboard shortcuts in Preferences

*18:19* — Keyboard ▸ Shortcuts lists the desktop's shortcuts, with the space around tiled windows. Click one and type new keys (here Left half became ⌃⌥L); keys taken from another shortcut are said, and Default Keys puts them all back.

![Keyboard shortcuts in Preferences](2026-10-26/181958-keyboard-shortcuts-in-preferences.png)

### Tile windows from the keyboard

*18:19* — ⇧⌘← and ⇧⌘→ tile the window into half the display, ⇧⌘↑/↓ the top or bottom; ⌃⌘↵ tiles every window on the display (here, three); quarters, thirds, centering and moving to the next display too. Space between them and at the edges is set in Preferences.

![Tile windows from the keyboard](2026-10-26/181957-tile-windows-from-the-keyboard.png)

### Brightness for every display

*17:49* — Preferences ▸ Display now has a brightness slider for each display whose brightness can be set, external monitors too (the Studio Display over USB). Every display starts at full brightness unless you've set a level for it.

![Brightness for every display](2026-10-26/174957-brightness-for-every-display.png)

### Right-click in text fields

*17:29* — Every text field now has Cut, Copy, Paste and Select All on right-click (keeping the selection), and double/triple-click select a word or the whole line, here in NetPositive's location field.

![Right-click in text fields](2026-10-26/172959-right-click-in-text-fields.png)

### Zoom leaves Deskbar alone

*17:18* — A zoomed window now stops 4 px short of Deskbar, as on BeOS, instead of running under it.

![Zoom leaves Deskbar alone](2026-10-26/171816-zoom-leaves-deskbar-alone.png)

### Images survive Terminal crashes

*16:48* — Terminal was killed after showing a kitty image; tabula-termd relaunched it and replayed the screen, image included. ⌘R also reaches the shell now (reverse history search).

![Images survive Terminal crashes](2026-10-26/164833-images-survive-terminal-crashes.png)

### Shells that outlive Terminal

*16:29* — Terminal's shells now run in tabula-termd, one per display, which keeps each screen too. Quit Terminal, or have it crash or upgrade, and the next one reopens a window on every shell as it was, scrollback, vim and all. Terminal ▸ Terminal Daemon… shows it and its shells, and can restart it in place (shells kept) or start it over.

![Shells that outlive Terminal](2026-10-26/162936-shells-that-outlive-terminal.png)

### Keyring prompts are Tabula's

*15:51* — gnome-keyring asks for the login keyring's password through a system prompter; the authentication agent now is that prompter (gcr's protocol and encrypted secret exchange, in Rust), so unlocking it for gh or a browser is a Tabula alert, not GTK's gcr-prompter. A wrong password comes back with the keyring's warning.

![Keyring prompts are Tabula's](2026-10-26/155116-keyring-prompts-are-tabula-s.png)

### Passwords: choose your 1Password account

*14:06* — Setting up Passwords now lists the accounts signed in to the 1Password app (read from its local account summaries), with email and sign-in address, instead of asking for a name or UUID. Another account still takes one by hand.

![Passwords: choose your 1Password account](2026-10-26/140631-passwords-choose-your-1password-account.png)

### NetPositive: the WebRTC samples' peer connection

*13:26* — The official webrtc.github.io 'Peer connection' sample, unmodified: the test camera (left) goes through two RTCPeerConnections and comes back decoded (right), connected in 281 ms.

![NetPositive: the WebRTC samples' peer connection](2026-10-26/132617-netpositive-the-webrtc-samples-peer-connection.png)

### Deskbar: a new build is ready

*13:17* — tabula-session watches the tree Tabula was built in. Once a build finishes with newer programs, a green arrow appears in Deskbar's tray; clicking it asks before installing them and reloading the session in place (windows stay open).

![Deskbar: a new build is ready](2026-10-26/131731-deskbar-a-new-build-is-ready.png)

### NetPositive: a call, end to end

*13:05* — Two RTCPeerConnections in one page, talking through NetPositive's own WebRTC (webrtc-rs, in process). The fake camera's test card goes out as H.264 (openh264) and comes back decoded on the right; the microphone goes as Opus and plays; a data channel carries text and bytes both ways; getStats() counts the pictures.

![NetPositive: a call, end to end](2026-10-26/130517-netpositive-a-call-end-to-end.png)

### NetPositive: a page of 120 big photos

*12:21* — 120 pictures of 2400×1600, 1.8 GB once decoded. The page's process now holds only their files (60 MB, was every bitmap), and the painter decodes a picture only while it's drawn, within 128 MB shared by every page, so the whole browser stays near 600 MB however long the page.

![NetPositive: a page of 120 big photos](2026-10-26/122157-netpositive-a-page-of-120-big-photos.png)

### NetPositive: a stuck page

*11:00* — Pages run in content processes, each a systemd unit with its own memory limit and CPU weight. A tab whose page stops answering for ten seconds gets this bar; Stop Page kills just that process and the tab shows the crash page.

![NetPositive: a stuck page](2026-10-26/110023-netpositive-a-stuck-page.png)

### NetPositive: a page over its memory

*10:58* — A page that runs away with memory meets its unit's MemoryMax (512 MiB here, for the test) and is OOM-killed alone; the bar says why, over the crash page.

![NetPositive: a page over its memory](2026-10-26/105845-netpositive-a-page-over-its-memory.png)

### NetPositive: audio and video

*09:58* — Video and audio elements in pages play with Tabula's own decoders (H.264/AV1 in MP4 and WebM, Opus, AAC, MP3 and more) through a native servo-media backend; sound goes to the sound server in process.

![NetPositive: audio and video](2026-10-26/095828-netpositive-audio-and-video.png)

### NetPositive: light page, dark desktop

*09:12* — A page with no colour scheme in dark mode: Servo would have drawn it with light text and dark controls on white; it now keeps the light scheme.

![NetPositive: light page, dark desktop](2026-10-26/091231-netpositive-light-page-dark-desktop.png)

### NetPositive: dark pages

*09:12* — In dark and night modes pages that support dark (color-scheme, prefers-color-scheme) go dark, canvas included; pages that don't stay black on white with light form controls and links.

![NetPositive: dark pages](2026-10-26/091225-netpositive-dark-pages.png)

### NetPositive: colour picker

*09:04* — <input type=color> opens a Choose a Colour sheet with the kit ColorControl; OK sets the page's colour and fires its input and change events (Servo 0.7 fires none).

![NetPositive: colour picker](2026-10-26/090431-netpositive-colour-picker.png)

### Preferences: Backgrounds' BColorControl

*09:04* — The desktop colour is now set with the kit's new ColorControl, BeOS's BColorControl: grey, red, green and blue ramps from black with draggable markers, and number fields.

![Preferences: Backgrounds' BColorControl](2026-10-26/090421-preferences-backgrounds-bcolorcontrol.png)

### NetPositive: tab icons

*08:56* — Tabs show the page's favicon (scaled on the CPU to a crisp 16-point icon), or a plain document for pages without one.

![NetPositive: tab icons](2026-10-26/085614-netpositive-tab-icons.png)

### NetPositive: scroll bars

*08:52* — Servo draws no scroll bars, so a script reports where the page is scrolled and NetPositive draws BeOS ones beside it (a new kit ScrollBar): the vertical bar always, the horizontal one when the page is wider than the window. Dragging, paging and the arrows scroll the page.

![NetPositive: scroll bars](2026-10-26/085226-netpositive-scroll-bars.png)

### NetPositive: Downloads

*08:44* — Downloads go straight to ~/Downloads on a worker thread: links the page can't show, download links (also blob: and data: ones made by script), and Download Linked File / Download Image in the context menu. The Downloads window shows live progress and rate, with Stop, Retry, Open and Show.

![NetPositive: Downloads](2026-10-26/084400-netpositive-downloads.png)

### NetPositive: Find

*08:19* — Edit ▸ Find… (⌘F) and Find Again (⌘G): a BeOS Find window; the match is selected and scrolled to the middle.

![NetPositive: Find](2026-10-26/081902-netpositive-find.png)

### Passwords: a passkey in a login

*08:14* — Passkeys live in a Passkey section of a 1Password login: credential ID, relying party, user, and the concealed P-256 key.

![Passwords: a passkey in a login](2026-10-26/081420-passwords-a-passkey-in-a-login.png)

### NetPositive: choosing a passkey

*08:13* — With several passkeys for the site, the bar has a pop-up to choose the account.

![NetPositive: choosing a passkey](2026-10-26/081358-netpositive-choosing-a-passkey.png)

### NetPositive: sign in with a passkey

*08:12* — navigator.credentials.get() finds the site's passkeys in Passwords and asks before Passwords signs.

![NetPositive: sign in with a passkey](2026-10-26/081251-netpositive-sign-in-with-a-passkey.png)

### NetPositive: save a passkey

*08:12* — A page's navigator.credentials.create() asks in a bar; Passwords makes and keeps the key.

![NetPositive: save a passkey](2026-10-26/081225-netpositive-save-a-passkey.png)

### NetPositive: offer to update a login

*07:54* — After a sign-in with a changed password, NetPositive offers to update the login in Passwords; a new one gets Save.

![NetPositive: offer to update a login](2026-10-26/075423-netpositive-offer-to-update-a-login.png)

### NetPositive: logins from Passwords

*07:52* — The key button lists the page's logins from Passwords (starting and unlocking it if need be) and fills the chosen one in.

![NetPositive: logins from Passwords](2026-10-26/075234-netpositive-logins-from-passwords.png)

### Passwords: locked

*07:25* — Opens locked and locks after idle; Touch ID (fprintd) or the account password (PAM) unlocks it.

![Passwords: locked](2026-10-26/072527-passwords-locked.png)

### Passwords in dark mode

*07:25*

![Passwords in dark mode](2026-10-26/072526-passwords-in-dark-mode.png)

### Passwords: a 1Password browser

*07:25* — Tabula's password manager, on its own Rust 1Password SDK over the desktop app's integration socket (shown against the test fake): vaults and categories, items, and the selected item's fields.

![Passwords: a 1Password browser](2026-10-26/072524-passwords-a-1password-browser.png)

### Passwords: Password Generator

*07:25* — Random, memorable and PIN recipes with a live strength meter; Use fills the field it was opened from.

![Passwords: Password Generator](2026-10-26/072505-passwords-password-generator.png)

### NetPositive error page

*06:38* — Network errors answer in haiku, as NetPositive did, picked at random from six new verses

![NetPositive error page](2026-10-26/063835-netpositive-error-page.png)

### NetPositive in dark mode

*06:38* — The new Servo based browser: tabs, toolbar and location field follow the theme while pages stay light

![NetPositive in dark mode](2026-10-26/063835-netpositive-in-dark-mode.png)

### Network: joining enterprise Wi-Fi

*05:50* — A sheet for 802.1X networks: PEAP or TTLS with username and password, or TLS with certificate and key, the CA and a domain; shown with made-up networks. Tab now moves between text fields in every kit form

![Network: joining enterprise Wi-Fi](2026-10-26/055031-network-joining-enterprise-wi-fi.png)

### Viewer: AV1 video

*05:42* — AV1 in WebM and Matroska plays through rav1d with Opus sound; the poster frame decodes too, and the YUV to RGB conversion moved to fixed point tables (1080p at 72 frames a second)

![Viewer: AV1 video](2026-10-26/054237-viewer-av1-video.png)

### Viewer: video playback

*05:38* — H.264 in mp4 and Matroska decoded with OpenH264 a few frames ahead and shown against a clock that follows the sound; play, pause, seek and replay, and the screen stays awake while it plays

![Viewer: video playback](2026-10-26/053831-viewer-video-playback.png)

### Viewer: AVIF

*04:43* — AVIF decoded natively with avif-parse and the BSD-licensed rav1d AV1 decoder, with our own YUV to RGB; HEIC through heifer and Ogg Opus playback through opus-decoder land in the same change

![Viewer: AVIF](2026-10-26/044300-viewer-avif.png)

### Viewer zoom and pan

*03:54* — Images load at full resolution in Viewer: Actual Size, pinch and command-scroll zoom about the pointer, drag to pan

![Viewer zoom and pan](2026-10-26/035434-viewer-zoom-and-pan.png)

### Icons for document kinds

*03:50* — Spreadsheet, presentation, word processing, PDF and font files get their own icons

![Icons for document kinds](2026-10-26/035008-icons-for-document-kinds.png)

### Colour emoji in Terminal

*03:42* — COLRv1 emoji painted natively: sequences, skin tones, flags, keycaps, on any background

![Colour emoji in Terminal](2026-10-26/034208-colour-emoji-in-terminal.png)

### Terminal found it

*03:33* — The match is selected and scrolled into view; Find Again steps on

![Terminal found it](2026-10-26/033359-terminal-found-it.png)

### Terminal Find window

*03:33* — Find in the scrollback: case, whole words, wrap-around, backwards

![Terminal Find window](2026-10-26/033359-terminal-find-window.png)

### Back to extended

*03:30* — Choosing Extended desktop puts the display back beside the other with its full settings

![Back to extended](2026-10-26/033025-back-to-extended.png)

### Display mirroring

*03:30* — A display set to mirror another leaves the arrangement; only resolution, refresh and Use as remain

![Display mirroring](2026-10-26/033024-display-mirroring.png)

### Keyboard backlight

*03:18* — Keyboard illumination keys raise the backlight with the level indicator; Preferences Keyboard has a Backlight slider that follows it. Turns off with the display.

![Keyboard backlight](2026-10-26/031818-keyboard-backlight.png)

### Deskbar Stay Awake menu

*03:15* — Right-click the cup (also in the Tabula menu): until turned off, 1 to 8 hours, even with the lid closed, Allow Sleep with the end time. The menu now flips to stay on screen.

![Deskbar Stay Awake menu](2026-10-26/031504-deskbar-stay-awake-menu.png)

### Level indicator

*03:15* — app_server panel for the brightness and volume keys and Ctrl+Cmd+A caffeinated, light and dark, 2x.

![Level indicator](2026-10-26/031504-level-indicator.png)

### Cursors in a session

*03:01* — Headless 2x session: StyledEdit asks for the text cursor over cursor-shape-v1, the desktop shows the hand, the window frame shows the resize arrow.

![Cursors in a session](2026-10-26/030140-cursors-in-a-session.png)

### Haiku system cursors

*03:01* — All 28 of Haiku app_server cursors drawn by the new tabula-hvif renderer at 1x, 1.5x and 2x on light and dark grounds, with the soft offset shadow. ResizeWest is mirrored from ResizeEast since the upstream data duplicates it.

![Haiku system cursors](2026-10-26/030140-haiku-system-cursors.png)

### Rotated main display

*02:48* — HEADLESS-2 rotated 90 degrees and made the main display; Deskbar and the desktop icons moved to it and re-laid out in portrait.

![Rotated main display](2026-10-26/024851-rotated-main-display.png)

### Display: Identify

*02:48* — Each screen shows its name, connector and mode in a raised panel for 4 s.

![Display: Identify](2026-10-26/024851-display-identify.png)

### Display: arranged screens, dark

*02:40* — Two headless screens stacked by dragging in the map; the lower one follows when the upper changes size.

![Display: arranged screens, dark](2026-10-26/024050-display-arranged-screens-dark.png)

### Display: resolution with revert countdown

*02:40* — HEADLESS-1 switched to 1920 by 1200; its right-hand neighbour moved with the edge; Keep or Revert, else it reverts after 15 s.

![Display: resolution with revert countdown](2026-10-26/024050-display-resolution-with-revert-countdown.png)

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
