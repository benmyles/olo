# Screenshot journal

Olo's visual evolution, newest first. Added with `tools/journal.py`
(images are stored with Git LFS; run `git lfs pull` after cloning).

## 2026-10-29

### Deskbar's badge ring is one device pixel

*08:25* — The panel-coloured ring that cuts the unread badge out of the bell is a whole device pixel wide at 1.5× too, rather than a blurred pixel and a half.

![Deskbar's badge ring is one device pixel](2026-10-29/082532-deskbar-s-badge-ring-is-one-device-pixel.png)

### Disk Usage: crisp hairlines and bars at 1.5×

*08:25* — The lines between slices, round each ring and round the hub are one device pixel at every scale (style::hairline), and the bars' widths land on whole device pixels, so at 1.5× nothing is blurred across two.

![Disk Usage: crisp hairlines and bars at 1.5×](2026-10-29/082530-disk-usage-crisp-hairlines-and-bars-at-1-5.png)

### Screenshot's caption lines up with Save as

*08:25* — The caption under the preview sits level with the Save as: row beside it at 1×, 1.5× and 2×, in light, dark and night.

![Screenshot's caption lines up with Save as](2026-10-29/082530-screenshot-s-caption-lines-up-with-save-as.png)

### Terminal shows its tab strip with one tab

*08:25* — The tab strip no longer waits for a second tab: one tab shows with its icon, title, close box and +, so a new tab is always a click away (light and dark at 1×, 1.5× and 2×).

![Terminal shows its tab strip with one tab](2026-10-29/082529-terminal-shows-its-tab-strip-with-one-tab.png)

### Menu shortcuts: ⇧ stands with the capitals

*04:37* — Noto Sans Symbols puts ⇧ a device pixel below the capitals at every scale. key_combo now places a symbol about as tall as the capitals by its ink, so the feet of ⇧⌘Z line up at 1x, 1.25x, 1.5x, 1.75x and 2x.

![Menu shortcuts: ⇧ stands with the capitals](2026-10-29/043741-menu-shortcuts-stands-with-the-capitals.png)

### Themes: after, every card fits

*04:37* — Preferences panels are sized for their tallest content (Themes 560x634, Display 500x490, Keyboard 480x736, Touchpad 460x586, About 444x290), checked at 1x, 1.5x and 2x, light and dark. Power and Security's menus share widgets::divider_for, the widest label and the gap after it, so the wells line up and every label fits.

![Themes: after, every card fits](2026-10-29/043740-themes-after-every-card-fits.png)

### Themes: before, the colour schemes ran into the buttons

*04:37* — At 1x the panel's fixed height was too short for the colour-scheme cards: BeOS's description ran onto three lines and pushed under Defaults and Revert.

![Themes: before, the colour schemes ran into the buttons](2026-10-29/043740-themes-before-the-colour-schemes-ran-into-the-bu.png)

### Tracker's item count lines up with its list

*04:12* — The count bar's text starts in line with the list's icons above it (here at 1× and 2×), and the free space keeps the same inset from the right.

![Tracker's item count lines up with its list](2026-10-29/041208-tracker-s-item-count-lines-up-with-its-list.png)

### Notifications: times sit on the title's baseline

*04:12* — The app and time line up with the title's baseline rather than its box, and rows land on whole device pixels at 1.5× too.

![Notifications: times sit on the title's baseline](2026-10-29/041208-notifications-times-sit-on-the-title-s-baseline.png)

### Read-only documents grey out their Font menu

*04:12* — A file you can't write opens read-only: Font and Document's commands are disabled, and so are their submenus, rather than doing nothing when chosen.

![Read-only documents grey out their Font menu](2026-10-29/041208-read-only-documents-grey-out-their-font-menu.png)

### StyledEdit keeps its place through an upgrade

*04:12* — After an in-place reload StyledEdit comes back scrolled where it was, with its caret, its selection and any unsaved edits (here line 614, edited).

![StyledEdit keeps its place through an upgrade](2026-10-29/041207-stylededit-keeps-its-place-through-an-upgrade.png)

### StyledEdit on the kit's text editor

*04:12* — StyledEdit now draws with olo_kit::editor, one laid-out paragraph each, so a long document types and scrolls inside a frame. Left, centred, right and justified text keep their edges on the margin, trailing spaces and all.

![StyledEdit on the kit's text editor](2026-10-29/041207-stylededit-on-the-kit-s-text-editor.png)

### Deskbar lights the workspace showing

*01:48* — The whole workspace showing, its heading and its applications, is washed in the selection's colour across the bar with a hairline of the selection's edge above and below, like a BListView selection as big as the group; the other workspaces stay on the plain panel. Its heading takes the selection's text colour, and its rows shade that ground for focus, hover and separators, so it follows every theme and colour set, light, dark and night. Twitcher's choice inside it takes the menu's stronger selection so it still stands out. Light and dark at 2×.

![Deskbar lights the workspace showing](2026-10-29/014818-deskbar-lights-the-workspace-showing.png)

### ⌥Tab keeps to this workspace, ⌘Tab goes everywhere, and Deskbar follows

*01:10* — Reversing the earlier swap: ⌘Tab's Twitcher shows every workspace's applications, ⌥Tab only this one's. While it's up, Deskbar highlights the application Twitcher is on (style::selection_wash), here DeskCalc.

![⌥Tab keeps to this workspace, ⌘Tab goes everywhere, and Deskbar follows](2026-10-29/011006-tab-keeps-to-this-workspace-tab-goes-everywhere-.png)

### Clicking an application in Deskbar brings all its windows

*01:10* — A left click on StyledEdit (left) brings both its windows forward, the minimized one too, with the last one used in front and focused (right), as the Dock does; a right click still lists the windows to choose one.

![Clicking an application in Deskbar brings all its windows](2026-10-29/011005-clicking-an-application-in-deskbar-brings-all-it.png)

### Deskbar lists every workspace in order, the one showing in a pill

*01:10* — No more Other Workspaces: each workspace with windows gets a quiet caption over its applications, always in ascending order, and the one showing is marked by a pill in the selection's colours (style::selection_ink), so it follows every theme, light and dark. Soft Be, Toybox and Enamel, light and dark, at 2×.

![Deskbar lists every workspace in order, the one showing in a pill](2026-10-29/011004-deskbar-lists-every-workspace-in-order-the-one-s.png)

### Electron windows tile

*00:40* — Slack, and every Electron app, floated over the tiles: Electron sends i32::MAX as its largest size, meaning none, and any largest size floated a window. Now a window floats when it has one size only, or a largest size its display's tiles are bigger than.

![Electron windows tile](2026-10-29/004036-electron-windows-tile.png)

### Deskbar's wordmark follows the theme again

*00:29* — Choosing a theme in Preferences drew everything in BeBox: the theme was looked up by its address in THEMES, a const since it moved to olo-theme, so another crate's copy never matched and BeBox (the first) was taken. Now by id; Deskbar's wordmark, every app's icons and Twitcher's follow the theme again (Toybox, Enamel, Halftone, Papercraft, Soft Be, BeBox).

![Deskbar's wordmark follows the theme again](2026-10-29/002909-deskbar-s-wordmark-follows-the-theme-again.png)

### ⌘Tab keeps to this workspace, ⌥Tab goes everywhere

*00:19* — ⌘Tab (left) switches among this workspace's applications only; ⌥Tab (right) has a numbered row for every workspace with windows, as ⌘Tab had. ⌃Tab is Next Tab in the window.

![⌘Tab keeps to this workspace, ⌥Tab goes everywhere](2026-10-29/001957-tab-keeps-to-this-workspace-tab-goes-everywhere.png)

### Deskbar groups applications by workspace

*00:19* — With more than one workspace, Deskbar lists the applications on the workspace showing under its name, and folds the rest under Other Workspaces (BOutlineListView's latch); unfolded, each workspace's under its own heading. Clicking a heading goes there.

![Deskbar groups applications by workspace](2026-10-29/001957-deskbar-groups-applications-by-workspace.png)

### Auto-tiling

*00:19* — Deskbar's tiles beside the cup (or ⌃⌥⌘T) tile every window on the workspace: a new one halves the focused tile along its longer side, a closed one's room goes to its neighbour. Zoom still zooms and comes back; Zoom Vertically makes a column; dialogs and small utility windows such as DeskCalc float; ⌃⌘Z undoes.

![Auto-tiling](2026-10-29/001956-auto-tiling.png)

### Noto fonts, shipped with Olo

*00:19* — Before (Nimbus Sans, DejaVu for symbols) above, after (Noto Sans and Noto Sans Mono, with Noto Sans Symbols, Symbols 2 and Math for ⌘ ⌥ arrows and marks) below: one family everywhere, the same on every machine, open and clear at small sizes. app_server's tab titles, Twitcher and hint badges centre their capitals from the face's own metrics.

![Noto fonts, shipped with Olo](2026-10-29/001955-noto-fonts-shipped-with-olo.png)

## 2026-10-28

### Screen saver hides the pointer

*22:31* — app_server hides the pointer while the screen saver runs, from its first frame; the pointer comes back with the first move, which ends it.

![Screen saver hides the pointer](2026-10-28/223118-screen-saver-hides-the-pointer.png)

### Screen saver: pointer before

*22:31* — Before: started under a still pointer, the screen saver showed the pointer of the window underneath (here a text I-beam) until a move, and a move ends it.

![Screen saver: pointer before](2026-10-28/223117-screen-saver-pointer-before.png)

### Qt apps follow Olo: light

*21:51* — The same Ladybird switched back to light live, with no restart.

![Qt apps follow Olo: light](2026-10-28/215134-qt-apps-follow-olo-light.png)

### Qt apps follow Olo: dark

*21:51* — The session now says Olo:GNOME, so Qt takes its GTK 3 theme: Ladybird (from a toolbox) follows Olo's palette, font and light/dark live. 2x shown; checked at 1x, 1.5x and 2x.

![Qt apps follow Olo: dark](2026-10-28/215134-qt-apps-follow-olo-dark.png)

### Qt apps before: generic theme

*21:51* — Before: with XDG_CURRENT_DESKTOP=Olo, Qt didn't know the desktop and took its generic theme, so Ladybird stayed light in its own font while Olo was dark.

![Qt apps before: generic theme](2026-10-28/215133-qt-apps-before-generic-theme.png)

### Keyboard hints: instant on a busy screen

*21:51* — Control twice now asks only the surfaces showing, for only what can be picked, on worker threads, starting when Control is first let go: the letters show in under a millisecond after the second press (was about half a second, seconds on complex screens).

![Keyboard hints: instant on a busy screen](2026-10-28/215133-keyboard-hints-instant-on-a-busy-screen.png)

### Terminal: side tabs beside Show All Tabs

*21:51* — Pane heads beside a side strip are its first tab and the room round it tall (32), so the tab's middle is level with the heads'.

![Terminal: side tabs beside Show All Tabs](2026-10-28/215132-terminal-side-tabs-beside-show-all-tabs.png)

### Terminal: side tabs with even room

*21:51* — Side tabs are rounded rows with the same 4-point room on every side: from the window edge, the strip's edge, the menu bar and each other; the selected one is raised in the content's colours. 2x shown; checked at 1x, 1.5x and 2x.

![Terminal: side tabs with even room](2026-10-28/215132-terminal-side-tabs-with-even-room.png)

### Terminal: side tabs before

*21:51* — Before: the side strip's tabs ran into the content, the first hung from the menu bar with no room above it, and nothing balanced the left inset on the right.

![Terminal: side tabs before](2026-10-28/215132-terminal-side-tabs-before.png)

### Frames and tabs even at fractional scales

*20:48* — app_server draws the window frame out from where the content lands, in device pixels: outline a hairline, bevel light and shadow a point each rounded, the flat middle taking the rest, the same on all four sides (1, 2, 3, 2 pixels at 1.5x; it was 2, 1, 2, 1, 2 on one side and mirrored differently on the other). Tabs, the wallpaper, the switcher and other app_server pictures are drawn pixel for pixel instead of resampled a pixel off their size. 1.5x, 8x zoom.

![Frames and tabs even at fractional scales](2026-10-28/204818-frames-and-tabs-even-at-fractional-scales.png)

### Menu bar one row with the tabs

*20:48* — The kit's menu bar is 26 points tall, the same row as a tab strip and a pane head, instead of 21 (31.5 device pixels at 1.5x, cramped against the rest). Terminal at 1.5x, tabs down the side.

![Menu bar one row with the tabs](2026-10-28/204818-menu-bar-one-row-with-the-tabs.png)

### Text fields: the hint on the label's baseline

*19:50* — Found by the layout review. A TextControl's hint is drawn with the kit's text placement, so it lines up with its label and with typed text at 1x, 1.5x and 2x; it is the field's placeholder for agents too.

![Text fields: the hint on the label's baseline](2026-10-28/195041-text-fields-the-hint-on-the-label-s-baseline.png)

### Date & Time: one baseline for the tabs, the Time field ends with the calendar

*19:50* — Found by the layout review. Every TabView label now sits on one line, the selected tab's too; the Time field sits level with the calendar's sixth week so the two columns end together; the second hand is a device-pixel hairline, so it stays crisp at 1.5x.

![Date & Time: one baseline for the tabs, the Time field ends with the calendar](2026-10-28/195040-date-time-one-baseline-for-the-tabs-the-time-fie.png)

### Terminal: tabs on top, light, at 1.5x

*19:37* — The same grid with the tabs on top: each tab a pane head's height, standing on the strip's bottom edge, so the selected tab and the pane head under it are one height; icons on the menu's first-title column; one-device-pixel lines and no seams at 1.5x.

![Terminal: tabs on top, light, at 1.5x](2026-10-28/193705-terminal-tabs-on-top-light-at-1-5x.png)

### Terminal: chrome on one column and one row

*19:36* — One 12pt left column for the menu's first title, the tab icons, New Tab, the pane heads' icons and the terminal text; one 26pt row for side tabs, top tabs and pane heads, with the 21pt menu bar clearly its own thing; the first side tab hangs from the menu bar (its corner square, the bar's line its top edge). Tiles are laid out in device pixels, so no seams at 1.5x. Reviewed clean by layout_review in light and dark, tabs top and side, tiled and not, at 1x, 1.5x and 2x.

![Terminal: chrome on one column and one row](2026-10-28/193652-terminal-chrome-on-one-column-and-one-row.png)

### Terminal before: chrome on no common grid

*19:36* — Before: the menu's first title, the side tabs' icons, New Tab and the pane heads' icons each started at a different x; side tabs, top tabs and pane heads were three different heights; the side strip began with a gap under the menu bar, so its top line doubled the bar's.

![Terminal before: chrome on no common grid](2026-10-28/193651-terminal-before-chrome-on-no-common-grid.png)

### Password alerts sized to their text

*17:46* — The keyring and polkit password alerts are true fixed alerts now, named Authentication in Deskbar, and sized to their own text by the new style::plain_text_height (left: before, a window 18 points too tall; middle: after, dark; right: light, with the mismatch note in the row kept for it). app_server no longer gives a fixed-size window a remembered size from before, and the kit keeps a window within its maximum size.

![Password alerts sized to their text](2026-10-28/174643-password-alerts-sized-to-their-text.png)

### Twitcher: the chosen app on a plate

*17:31* — The app Twitcher has chosen sits on the same rounded selection plate the kit gives a chosen picture (Themes, icon grids), in place of a hard square of menu blue. Its edge colour is now Palette::selection_edge, shared by the kit and app_server.

![Twitcher: the chosen app on a plate](2026-10-28/173108-twitcher-the-chosen-app-on-a-plate.png)

### History: legible to the last

*17:27* — Notifications in the History window still fade as they age, but never below 30% ink: the oldest note (here 58 minutes old, two minutes from going) is faint but readable, in cards and in lists, instead of nearly invisible while still counted and clickable. Full again when selected or under the pointer.

![History: legible to the last](2026-10-28/172717-history-legible-to-the-last.png)

### Symbols on the baseline

*17:22* — Symbols Nimbus Sans lacks (the menu-path triangle, arrows, the Command key) come from DejaVu Sans, which egui centred in its own taller line, so they sat a few pixels below the words around them (above: before; below: now). The kit now shifts each fallback face onto the primary face's baseline, so they line up everywhere: Passwords' setup text, Palette's App > Menu paths, shortcuts and prose.

![Symbols on the baseline](2026-10-28/172254-symbols-on-the-baseline.png)

### Palette: kinds on the line

*17:12* — In Palette results, the kind on the right (Application, File, Web) used to float a few pixels above the name and description beside it. It now sits on the same line, placed with the kit's label_fit, and agents can read it in the tree. Before above, after below.

![Palette: kinds on the line](2026-10-28/171202-palette-kinds-on-the-line.png)

### Preferences: Security without the dead band

*17:09* — The Security panel ended in an empty band above Defaults and Revert; the window is now 336 tall (from 354), the least that fits its boxes, so the buttons sit a footer's gap under the last box as in every other panel. Before above, after below, light and dark.

![Preferences: Security without the dead band](2026-10-28/170916-preferences-security-without-the-dead-band.png)

### Preferences: designed empty states

*17:07* — Before (top) and after, light and dark. A panel with nothing it can show (no sound server, NetworkManager or bluetoothd not running, no fingerprint sensor, no displays) said so in one dim line in the corner of an empty window. The kit's new EmptyState puts the panel's icon and the message in the middle of the space, as lists already do with empty_text.

![Preferences: designed empty states](2026-10-28/170715-preferences-designed-empty-states.png)

### Log Viewer: the detail pane says what it's for

*16:56* — Until you pick a line, the pane under the list was a blank white box. It now says "Select a line to read it whole." in the list's dim empty-state text. This is TextView::empty_text in the kit, the TextView version of ColumnListView::empty_text.

![Log Viewer: the detail pane says what it's for](2026-10-28/165638-log-viewer-the-detail-pane-says-what-it-s-for.png)

### Terminal: ⌘ keys as you press them

*16:50* — The Terminal's menus showed ⇧⌘N, ⇧⌘T and so on, because ⌘ reaches a terminal as Ctrl+Shift; they now read ⌘N, ⌘T, as pressed, in the menu bar, context menus and the list agents get. ⌘- and ⌘= (Smaller, Bigger) work now; before, Shift turned - into _ and = into +, and neither matched. A physical Ctrl+Shift+- reaches the shell as Ctrl+_ (readline's undo) instead of being taken for ⌘-: app_server keeps the right Ctrl for ⌘, and the kit tells the two apart in terminal windows.

![Terminal: ⌘ keys as you press them](2026-10-28/165043-terminal-keys-as-you-press-them.png)

### Notification banners: close box, dark

*16:34* — The same banners in dark mode.

![Notification banners: close box, dark](2026-10-28/163416-notification-banners-close-box-dark.png)

### Notification banners: a quieter close box

*16:34* — One close box per banner, in its header beside the app's name, instead of one in the header and another on every notification. In a group of several, each notification shows its own only while the pointer is over it. Both are named buttons in the tree (Close, Close All from Builds), so agents can find them.

![Notification banners: a quieter close box](2026-10-28/163415-notification-banners-a-quieter-close-box.png)

### Terminal: the group colour you picked

*16:34* — The chosen colour in Name Group is a soft ring around the swatch, in light and dark, instead of a heavy outline.

![Terminal: the group colour you picked](2026-10-28/163415-terminal-the-group-colour-you-picked.png)

### Backgrounds: the chosen wallpaper on a plate

*16:34* — The wallpaper picker (the kit's ThumbnailPicker) shows the chosen picture on a rounded plate a few pixels larger than it, light and dark, rather than a thick border around the picture.

![Backgrounds: the chosen wallpaper on a plate](2026-10-28/163415-backgrounds-the-chosen-wallpaper-on-a-plate.png)

### Themes: colour sets selected, dark

*16:34* — The chosen colour set's plate in dark mode.

![Themes: colour sets selected, dark](2026-10-28/163414-themes-colour-sets-selected-dark.png)

### Themes: colour sets selected the same way

*16:34* — Preferences ▸ Themes marks the chosen colour set with the kit's selection plate, the same soft ground as a selected card, in place of a coloured ring.

![Themes: colour sets selected the same way](2026-10-28/163414-themes-colour-sets-selected-the-same-way.png)

### Selected cards in every look and colour set

*16:34* — The selected Builds card in each of the seven looks, with the BeOS and Catppuccin colour sets in bebox and papercraft, light and dark. The tint comes from each look's own selection colour, so it never fights the yellow window tab.

![Selected cards in every look and colour set](2026-10-28/163414-selected-cards-in-every-look-and-colour-set.png)

### Notifications: a selected card, not an outline

*16:34* — The selected card was drawn with a thick accent outline. Now it sits on a soft tint of the accent with a hairline edge, the way a selected row reads in a list; its text and buttons keep their contrast in light and dark.

![Notifications: a selected card, not an outline](2026-10-28/163413-notifications-a-selected-card-not-an-outline.png)

### Terminal: sudo cancelled, plainly

*15:55* — Cancel ends sudo with exit 1 and a line an agent can read: “sudo: not run: cancelled in Terminal's password window”.

![Terminal: sudo cancelled, plainly](2026-10-28/155502-terminal-sudo-cancelled-plainly.png)

### Terminal: the sudo window in dark mode

*15:55* — While the fingerprint reader waits, the password field waits too; the note under it is what PAM says.

![Terminal: the sudo window in dark mode](2026-10-28/155501-terminal-the-sudo-window-in-dark-mode.png)

### Terminal: sudo asks for its password in a window

*15:55* — When anything in a Terminal tab runs sudo (you at the prompt, or an agent like Claude Code with no terminal at all), Terminal asks in an alert of its own: who wants it, the command as typed, the fingerprint reader first if PAM uses it, then the password.

![Terminal: sudo asks for its password in a window](2026-10-28/155501-terminal-sudo-asks-for-its-password-in-a-window.png)

### Preferences: notification fade, dark

*15:32* — The same box in the dark theme.

![Preferences: notification fade, dark](2026-10-28/153241-preferences-notification-fade-dark.png)

### Preferences: fade notifications away over an hour

*15:32* — History box: on by default, over 1 hour; the strip shows the fade from now to gone.

![Preferences: fade notifications away over an hour](2026-10-28/153240-preferences-fade-notifications-away-over-an-hour.png)

### Notification list: a stop sign for errors

*15:30* — Rows lose their coloured kind edge; a failed note's title starts with the stop sign instead.

![Notification list: a stop sign for errors](2026-10-28/153049-notification-list-a-stop-sign-for-errors.png)

### Notification cards without the lines, dark

*15:30* — Cards are separated by space and an etched edge, not coloured bands; errors carry the stop sign.

![Notification cards without the lines, dark](2026-10-28/153048-notification-cards-without-the-lines-dark.png)

### Notification banners without the lines, dark

*15:30*

![Notification banners without the lines, dark](2026-10-28/153048-notification-banners-without-the-lines-dark.png)

### Notification banners without the lines

*15:30* — No accent band, top line or coloured edge. The app's colour is in its badge, tile and progress bar only; an error's title starts with a small crisp stop sign, level with the capitals, and there is more room around the icon.

![Notification banners without the lines](2026-10-28/153048-notification-banners-without-the-lines.png)

### Terminal tabs down the side, Show All Tabs, dark

*15:30* — Side tabs with the group's chip at the top and no band. In Show All Tabs the active pane's head is lit instead of edged.

![Terminal tabs down the side, Show All Tabs, dark](2026-10-28/153047-terminal-tabs-down-the-side-show-all-tabs-dark.png)

### Terminal tabs without the lines

*15:30* — The group shows as its chip alone: no coloured band under the strip, no separators beside the chip. The current tab is a raised page joined to the terminal below.

![Terminal tabs without the lines](2026-10-28/153047-terminal-tabs-without-the-lines.png)

### Notifications: unread first, fading with age

*15:15* — The History window puts unread notes first under an Unread heading (lit dot, bold title, Mark All Read); clicking one reads it, and it keeps its place until the window closes. With fading on (Preferences, 1 hour by default) each row fades as it ages, the newest strongest, and goes once it has faded away; it shows at full strength while selected or under the pointer.

![Notifications: unread first, fading with age](2026-10-28/151551-notifications-unread-first-fading-with-age.png)

### Notifications: banners specialised per app

*15:14* — Apps can say how a note looks (x-olo-* hints, olo_kit::notify's builders): an accent band, a badge and subtitle, a table of fields, a large picture, message or media styles. Websites through Chrome become their own apps (WhatsApp, Gmail) with their colours and monogram tiles; a message shows its sender round and the text in a balloon.

![Notifications: banners specialised per app](2026-10-28/151427-notifications-banners-specialised-per-app.png)

### Terminal: tabs down the side

*14:34* — Settings ▸ Tabs ▸ Down the Side stands the kit's TabStrip as a column (TabStrip::side): rows like Tracker's, the group's chip at the top and its band down each tab, the selected tab opening into the screen, and an edge to drag wider. The window keeps its size; the grid makes room.

![Terminal: tabs down the side](2026-10-28/143401-terminal-tabs-down-the-side.png)

### ⌘Tab groups windows by display

*14:26* — With a Deskbar on every display, Twitcher's rows split into a column per display, each in an etched BBox named across its top with a little screen, lit on the display this copy of the panel is on, so it's clear which windows are where. ↑/↓ keep to the chosen window's display.

![⌘Tab groups windows by display](2026-10-28/142612-tab-groups-windows-by-display.png)

### ⌘Space opens the palette on the display in use

*14:09* — With two displays, the palette now opens on the focused window's display (here the Terminal on the right-hand one), else the pointer's, rather than always on the main display.

![⌘Space opens the palette on the display in use](2026-10-28/140922-space-opens-the-palette-on-the-display-in-use.png)

### A new window joins its group at the group's size

*14:05* — A Terminal opened from its own menu (Terminal ▸ New Terminal) joined its group, then the whole group shrank to the new window's default size: the menu's pointer grab lasts until the pointer next moves, sizes waited for any grab, and the new window, in front, then led the group at its own size. Now sizes wait only for a resize grab, and a window that joins a group by itself doesn't lead it until it has drawn at the group's size (asked again if its app draws another size anyway).

![A new window joins its group at the group's size](2026-10-28/140550-a-new-window-joins-its-group-at-the-group-s-size.png)

### Window groups: ⌘-dragging a tab into another group

*10:57* — Any window can be grouped by hand: ⌘-drag its tab over another window's (here Tracker's over Terminal's); it folds into its tab where it would go and that group lights up in the focus blue (light, dark). Let go, and it joins. ⌘-dragging a tab away pops the window out; along the tabs, it reorders them.

![Window groups: ⌘-dragging a tab into another group](2026-10-28/105726-window-groups-dragging-a-tab-into-another-group.png)

### Display preferences: Deskbar on every display

*10:56* — The new check box under the display map, light and dark; the map draws a Deskbar on every display while it is on.

![Display preferences: Deskbar on every display](2026-10-28/105657-display-preferences-deskbar-on-every-display.png)

### Deskbar on every display

*10:56* — Preferences ▸ Display ▸ Show Deskbar on every display: each display gets its own Deskbar listing just the applications with windows there (main display left, second right, its only window just moved over).

![Deskbar on every display](2026-10-28/105657-deskbar-on-every-display.png)

### Terminal: Catppuccin colours

*10:56* — Settings ▸ Colors offers Catppuccin Latte and Frappé for light mode, Macchiato and Mocha for dark; Frappé (left, light mode) and Macchiato (right, dark mode) are the defaults.

![Terminal: Catppuccin colours](2026-10-28/105656-terminal-catppuccin-colours.png)

### Preferences ▸ Windows in dark mode

*01:56* — The Windows panel in BeBox dark: the miniature's windows take the document colour and the theme's tab colours.

![Preferences ▸ Windows in dark mode](2026-10-28/015604-preferences-windows-in-dark-mode.png)

### Preferences ▸ Windows: space around arranged windows

*01:56* — A new Windows panel sets the margin at the display's edges and the gap between windows, with a miniature of the main display that follows the sliders. Tiling, zooming, Tile All, Smart Tile, Center, cascading and fanning out all keep to it, and app_server refits tiled and zoomed windows live as the sliders move, without changing which window is in front. Here a margin of 26 and a gap of 12; Deskbar's Window Arrangement menu now ends with Windows Preferences….

![Preferences ▸ Windows: space around arranged windows](2026-10-28/015603-preferences-windows-space-around-arranged-window.png)

### Minimized windows in Deskbar's window list

*01:34* — An app's window list in Deskbar marks a minimized window as a Mac's Window menu does: a hollow diamond where the check mark goes and a lighter label. It can still be chosen to bring the window back; agents read minimized on the item.

![Minimized windows in Deskbar's window list](2026-10-28/013406-minimized-windows-in-deskbar-s-window-list.png)

### Olo menu shows its shortcuts

*01:34* — Deskbar's Olo menu shows the keys the person has for each desktop-wide command (Find, Window Arrangement, Show or Hide All Windows, Lock Screen), following changes in Keyboard preferences.

![Olo menu shows its shortcuts](2026-10-28/013404-olo-menu-shows-its-shortcuts.png)

### Context menus show their keys

*01:24* — Every context menu takes the keys its commands have in the window's menu bar (Menu::keys_from), so Tracker's item menu shows ⌘O, ⌘Y, ⌘I and so on, and the window tab menu shows the person's own desktop shortcuts.

![Context menus show their keys](2026-10-28/012428-context-menus-show-their-keys.png)

### Quick Look: a window of its own, over Tracker

*01:24* — Space in Tracker opens Quick Look as a dialog window (xdg_dialog_v1, parented to the Tracker window) instead of a tab: it is sized for the file, so it can be bigger than the folder window, opens centred over it before its first frame, and stays in front of it when the folder window is clicked.

![Quick Look: a window of its own, over Tracker](2026-10-28/012425-quick-look-a-window-of-its-own-over-tracker.png)

### Terminal polished (light)

*01:07* — The same strip and padding in the light theme: the selected tab opens into the screen's own background (TabStrip::opens_into).

![Terminal polished (light)](2026-10-28/010731-terminal-polished-light.png)

### Terminal polished (dark)

*01:07* — Rounded strip tabs that share the row and end long titles with '…', the group's name in a chip with a band of its colour on every tab, pane heads with icons, the pane being typed into marked in the group colour and the others dimmed, 8×6 padding around the screen. label_fit (elide with …) now used wherever a line can overflow: palette rows, Drive Setup, kit list messages.

![Terminal polished (dark)](2026-10-28/010730-terminal-polished-dark.png)

### Terminal before the polish

*01:07* — Show All Tabs before: square tabs cut off mid-letter, pane titles in plain panel grey, the screen's text right against the frame.

![Terminal before the polish](2026-10-28/010730-terminal-before-the-polish.png)

### Desktop menu over windows

*00:49* — The desktop's right-click menu opened over a window was drawn behind it, and clicks went to the window. app_server now draws the desktop layer's menus over the windows and the Deskbar (under only the Overlay layer) and finds them there for the pointer and keyboard.

![Desktop menu over windows](2026-10-28/004935-desktop-menu-over-windows.png)

### Themes switch the wallpaper too

*00:09* — Choosing a theme in Preferences ▸ Themes now also sets the wallpaper to that theme's own (here Toybox); Backgrounds can still pick another.

![Themes switch the wallpaper too](2026-10-28/000904-themes-switch-the-wallpaper-too.png)

### A wallpaper for every theme

*00:09* — Each theme now has its own wallpaper in the pack, light and dark, drawn in its material with the wordmark's o as the hero: BeBox, Soft Be, Toybox, Papercraft, Enamel, Halftone and Classic (rows, light then dark). BeBox's is the new default.

![A wallpaper for every theme](2026-10-28/000901-a-wallpaper-for-every-theme.png)

## 2026-10-27

### Touchpad: 576 tall, measured by the layout lint

*23:20* — The layout lint runs Touchpad with every pad and profile. Its worst case is a MacBook pad without pressure on Touch, which shows both notes. At 520 the While Typing box ran 56 px past the tab and over Defaults/Revert. At 564 it was still 12 px past at 2×, leaving 8 px instead of the box's 20 px inset. At 576 it fits. Without a pad (here) the notes aren't shown, so that room is left over.

![Touchpad: 576 tall, measured by the layout lint](2026-10-27/232039-touchpad-576-tall-measured-by-the-layout-lint.png)

### About: wide enough for long facts

*23:19* — The layout lint runs About with long real-world facts (e.g. “AMD Ryzen 9 7945HX with Radeon Graphics, 32 cores”); at 420 the processor was cut short with “…”, so About is now 440 wide.

![About: wide enough for long facts](2026-10-27/231957-about-wide-enough-for-long-facts.png)

### Tracker: after the layout lint

*23:19* — One pixel taller (300×181): the box ends WINDOW_INSET above Defaults/Revert.

![Tracker: after the layout lint](2026-10-27/231957-tracker-after-the-layout-lint.png)

### Tracker: before the layout lint

*23:19* — The layout lint found the New Windows Open In box 1 px past the panel's body at 2× (11 px to the buttons, not 12).

![Tracker: before the layout lint](2026-10-27/231957-tracker-before-the-layout-lint.png)

### Power: after the layout lint

*23:19* — One pixel taller (460×363) so the boxes clear the buttons by WINDOW_INSET, and the menu fields' divider moved out to 176 so the longest label has a LABEL_GAP before its field.

![Power: after the layout lint](2026-10-27/231956-power-after-the-layout-lint.png)

### Power: before the layout lint

*23:19* — Caught by the new layout lint (olo_kit::layout_lint): at 2× the Caffeinated box ran 1 px past the window's body, leaving 11 px above Defaults/Revert instead of WINDOW_INSET (12), and “Put the computer to sleep after:” stood 3 px from its field.

![Power: before the layout lint](2026-10-27/231956-power-before-the-layout-lint.png)

### Keyboard: Option+Tab switches tabs

*23:15* — Option+Tab and Option+Shift+Tab now move to the next and previous tab in any window that has tabs: Chrome, Terminal, Firefox, GTK and Qt apps and Olo's own tabbed windows. Like every shortcut, they can be changed in Keyboard. The new Zoom vertically and Zoom horizontally shortcuts are listed here too.

![Keyboard: Option+Tab switches tabs](2026-10-27/231518-keyboard-option-tab-switches-tabs.png)

### Zoom Vertically and Zoom Horizontally

*23:14* — A window's menu can now zoom it along one axis only: Zoom Vertically makes it as tall as the screen and keeps its width and place across; Zoom Horizontally does the same the other way. Here the Themes window has been zoomed vertically. Alt+Shift+Cmd+Up or Down and Alt+Shift+Cmd+Left or Right do the same, as does double-clicking the window's top or bottom (or left or right) edge.

![Zoom Vertically and Zoom Horizontally](2026-10-27/231457-zoom-vertically-and-zoom-horizontally.png)

### Touchpad: the Touch tab ran into the buttons

*23:14* — Before (left), the Touch tab ran past the bottom of the Touchpad window, so its last setting sat on top of the Defaults and Revert buttons. A first fix (right, 564 tall) clears the buttons but still squeezes the While Typing box at the bottom; the layout lint later measured it 12 px short, and the window is now 576 tall.

![Touchpad: the Touch tab ran into the buttons](2026-10-27/231413-touchpad-the-touch-tab-fits.png)

### Themes: the Deskbar and desktop in each theme

*23:14* — The same Deskbar menu, Applications submenu and desktop icons in each of the seven themes, left to right and top to bottom: Soft Be, BeBox, Toybox, Papercraft, Enamel, Halftone and Classic. The wordmark and every app's icon change together.

![Themes: the Deskbar and desktop in each theme](2026-10-27/231402-themes-the-deskbar-and-desktop-in-each-theme.png)

### Preferences: Themes

*23:12* — The Themes panel in light mode: each theme is shown by its wordmark and a row of its icons; Soft Be is the default.

![Preferences: Themes](2026-10-27/231216-preferences-themes.png)

### Preferences: Themes (dark)

*23:12* — A new Themes panel picks the look of every icon and the Olo wordmark, in every app, the Deskbar and the desktop: Soft Be (the new default), BeBox, Toybox, Papercraft, Enamel, Halftone and Classic (Olo's first set). It applies at once, everywhere.

![Preferences: Themes (dark)](2026-10-27/231201-preferences-themes-dark.png)

### A link opened from outside comes forward

*21:43* — A link opened from a shell or an agent (through olo-chrome) now takes you to the Chrome window it opened in, from whatever workspace: olo-chrome asks app_server for an activation token and Chrome uses it. Here the page was opened from workspace 3, and Olo moved to workspace 1, where the window is.

![A link opened from outside comes forward](2026-10-27/214350-a-link-opened-from-outside-comes-forward.png)

### Keyboard hints on a double Control

*21:35* — Keyboard hints now come up when Control is pressed twice quickly, in place of Control and Shift together. How quickly is Preferences ▸ Keyboard ▸ Double-press speed (400 ms by default).

![Keyboard hints on a double Control](2026-10-27/213534-keyboard-hints-on-a-double-control.png)

### The DiskUsage icon

*21:04* — A ring chart with a magnifier, generated against the reference set, beside DriveSetup and its neighbours on the light, dark and night panels.

![The DiskUsage icon](2026-10-27/210431-the-diskusage-icon.png)

### DiskUsage: where the space went

*21:04* — A new app: Haiku's DiskUsage ring chart (the folder in the middle, its contents in rings outwards; click to go in, the middle to go up) beside the folder's contents by size, with bars in the slices' colours, and DaisyDisk's Collector along the bottom: drag rows or slices in (or ⌘D), then Move to Trash or Delete. The scan runs on a worker and the chart grows as it goes; it stays on one file system and counts hard links once.

![DiskUsage: where the space went](2026-10-27/210430-diskusage-where-the-space-went.png)

### Touchpad preferences in dark

*20:42* — The Touch, Gestures and Mouse tabs in dark mode.

![Touchpad preferences in dark](2026-10-27/204217-touchpad-preferences-in-dark.png)

### Touchpad preferences: profiles and every tunable

*20:42* — Preferences ▸ Touchpad now names the pad app_server found and recommends a profile (MacBook, Magic Trackpad, Standard; hand tuning makes it Custom). Touch holds Olo's palm and hover filter, a libinput plugin that counts a touch only while it presses, ignores palms and makes the click from the fingers' pressure alone; Gestures has scrolling speed, momentum glide, three-finger drag, drag lock and acceleration.

![Touchpad preferences: profiles and every tunable](2026-10-27/204217-touchpad-preferences-profiles-and-every-tunable.png)

### Twitcher rows, dark

*20:07* — The same, in dark mode.

![Twitcher rows, dark](2026-10-27/200725-twitcher-rows-dark.png)

### Twitcher: a row per workspace

*20:07* — ⌘Tab shows a row of apps for each workspace with windows; ↑/↓ change rows and workspaces, ←/→ walk a row, Esc puts everything back. With keyboard hints on, each cell has a letter to pick it by.

![Twitcher: a row per workspace](2026-10-27/200725-twitcher-a-row-per-workspace.png)

### Palette: open windows first

*20:01* — Typing an app's name in ⌘Space lists its open windows before the app itself, the one already in front last.

![Palette: open windows first](2026-10-27/200134-palette-open-windows-first.png)

### Window groups, dark

*19:59* — The same group's tab bar in dark mode.

![Window groups, dark](2026-10-27/195943-window-groups-dark.png)

### Window groups

*19:59* — An app's windows share one frame, with a tab each side by side along the top; clicking a tab brings that window forward. Group Windows in Deskbar's team menu or Tracker turns it off per app.

![Window groups](2026-10-27/195942-window-groups.png)

### Fan Out Others from the menu

*19:43* — Chosen from Deskbar with README.md focused: the other windows fan out along the top and README.md stays where it was, in front.

![Fan Out Others from the menu](2026-10-27/194317-fan-out-others-from-the-menu.png)

### Deskbar: Workspaces

*19:43* — Go to a workspace, or move the window that had the focus to one.

![Deskbar: Workspaces](2026-10-27/194315-deskbar-workspaces.png)

### Deskbar: Window Arrangement

*19:43* — The Olo menu now has the desktop-wide commands: Window Arrangement (Tile All, Smart Tile, Zoom All, Cascade All, Fan Out, Cascade and Fan Out Others, Undo), Workspaces and Show or Hide All Windows. The Deskbar tile shows the olo wordmark alone, centred.

![Deskbar: Window Arrangement](2026-10-27/194314-deskbar-window-arrangement.png)

### Cascade Others

*19:31* — ⌃⇧⌘\ with notes.txt focused: every other window is cascaded from the top left, and notes.txt stays where it was, in front. Fan Out Others (⌃⇧⌘/) does the same with Fan Out.

![Cascade Others](2026-10-27/193159-cascade-others.png)

### Rolled-up windows share one tile (dark)

*19:31* — The same arrangement in dark mode.

![Rolled-up windows share one tile (dark)](2026-10-27/193156-rolled-up-windows-share-one-tile-dark.png)

### Rolled-up windows share one tile

*19:31* — Tile All with four of seven windows rolled up: the three open ones get a cell each, and the four rolled-up ones share the last cell, their tabs side by side along its top. Unrolling one fills that cell.

![Rolled-up windows share one tile](2026-10-27/193153-rolled-up-windows-share-one-tile.png)

### Keyboard hints in menus (dark)

*17:42* — A pick that opens a menu letters its items next, skipping disabled ones; a submenu's come first.

![Keyboard hints in menus (dark)](2026-10-27/174223-keyboard-hints-in-menus-dark.png)

### Keyboard hints

*17:42* — Tapping Control and Shift together (opt-in, Preferences ▸ Keyboard ▸ Keyboard Hints) puts a letter on everything that can be picked: the focused window's controls, other windows by their tabs, Deskbar and the desktop's icons. Typing one picks it.

![Keyboard hints](2026-10-27/174220-keyboard-hints.png)

### OpenAI's models for AI features

*16:59* — Preferences ▸ AI offers Astra 6 (the default), Sol 6.1, Terra 6.1 and Luna 6, reached through the Token Pool's Codex accounts. A Claude model chosen before reads as Astra; calls turned away for now are retried after a wait.

![OpenAI's models for AI features](2026-10-27/165918-openai-s-models-for-ai-features.png)

### Tabula is now Olo Desktop

*16:34* — The new brand in About and Deskbar, light and dark: the mark and the olo wordmark, drawn crisp at 2× from data/brand. A running Tabula session upgrades into Olo in place, every window and setting kept.

![Tabula is now Olo Desktop](2026-10-27/163441-tabula-is-now-olo-desktop.png)

### Deskbar menu icons in dark mode

*15:54* — The Applications submenu in dark mode. The kit's popup menus take an icon on any item (Menu::icon), with a column that sits after the check marks when a menu has both.

![Deskbar menu icons in dark mode](2026-10-27/155420-deskbar-menu-icons-in-dark-mode.png)

### Icons in the Deskbar menu

*15:54* — Every item in Deskbar's Tabula menu now has its icon: the commands, our applications, the preference panels and each category of other applications. New sunburst icons for Stay Awake, Lock Screen, Log Out, Restart, Shut Down and the categories. Appearance ▸ Show icons in the Deskbar menu turns them off; it's on by default.

![Icons in the Deskbar menu](2026-10-27/155418-icons-in-the-deskbar-menu.png)

### Roll up

*15:45* — Double-click a tab (or ⌃⌘M, or Roll Up in its menu) and only the tab stays; again brings the window back.

![Roll up](2026-10-27/154533-roll-up.png)

### Appearance: Longest window title

*15:40* — The new slider in Preferences ▸ Appearance.

![Appearance: Longest window title](2026-10-27/154028-appearance-longest-window-title.png)

### Longest window title

*15:40* — A tab shows at most this many characters of its title, then an ellipsis (here 20; 100 by default).

![Longest window title](2026-10-27/154028-longest-window-title.png)

### Preferences ▸ AI

*15:32* — AI features go through the Token Pool, or an Anthropic key; Smart Tiling picks its model and how hard it thinks.

![Preferences ▸ AI](2026-10-27/153223-preferences-ai.png)

### Smart Tiling

*15:32* — Smart Tile (⌃⇧⌘↩) shows a model the screen and what each window holds, then tiles them; the note says how and offers Undo. (Here a fake model answered.)

![Smart Tiling](2026-10-27/153223-smart-tiling.png)

### The wallpaper pack

*15:13* — Backgrounds offers the new wallpapers with previews; each follows light and dark mode live and comes in a 16:9 or ultrawide version for each screen.

![The wallpaper pack](2026-10-27/151308-the-wallpaper-pack.png)

### DriveSetup: free space

*14:54* — A Free column shows how full each mounted volume is, and the status line says how much is left; it stays current while the window is open.

![DriveSetup: free space](2026-10-27/145435-drivesetup-free-space.png)

### Fan Out

*14:49* — ⌃⌘/ stacks a display's windows with their tabs side by side, narrowing them to stay clear of the Deskbar; ⌃⌘Z undoes it.

![Fan Out](2026-10-27/144918-fan-out.png)

### Terminal: Show All Tabs

*14:33* — Show All Tabs (⌘⇧D) puts every tab of a window on screen at once, each a live pane with its title over it; click one to type into it, marked in the group's colour. The tab strip is taller now, with the group's band kept clear of the icons.

![Terminal: Show All Tabs](2026-10-27/143308-terminal-show-all-tabs.png)

### Token Pool: a heatmap of requests

*14:24* — The requests going through the pool now light up as cells under the header, like Pulse's LEDs, and each account keeps a strip of its latest 25: green completed, yellow rate limited, red failed, grey cancelled. Click any cell for its model, client, time and tokens. The grid is the kit's new CellGrid.

![Token Pool: a heatmap of requests](2026-10-27/142435-token-pool-a-heatmap-of-requests.png)

### Num Lock on at start

*14:00* — Num Lock now starts on, and Preferences ▸ Keyboard can turn that off; the Num Lock key still toggles it, and changing other keyboard settings keeps its state.

![Num Lock on at start](2026-10-27/140037-num-lock-on-at-start.png)

### Token Pool

*13:55* — Claude Code and Codex share several subscriptions through a loopback gateway: each request goes to the account whose quota resets soonest, out-of-quota accounts wait for their reset, sign-ins are refreshed and kept sealed, and usage is counted without keeping any content. Fixture accounts.

![Token Pool](2026-10-27/135505-token-pool.png)

### Banners in the Deskbar's column

*13:28* — Notification banners are now as wide as the Deskbar and flush with the screen's edge beneath it, where there is usually nothing else, and more compact: the app's icon in the group's title, the text taking the width, buttons sharing it. Preferences adds Never to Show banners for (errors still show).

![Banners in the Deskbar's column](2026-10-27/132840-banners-in-the-deskbar-s-column.png)

### New Window for a running app

*13:24* — Right-clicking a running Tabula app in Tracker (or opening its menu in Deskbar) offers New Window: the running copy opens another window, as BeOS's Deskbar did, instead of a second copy starting. Terminal, StyledEdit and Tracker answer it.

![New Window for a running app](2026-10-27/132434-new-window-for-a-running-app.png)

### A window menu on the tab

*13:18* — Right-clicking a window's yellow tab opens its menu: Hide, Zoom, Back to Its Own Size, Center, every tiling choice, Move to Workspace, Send to Back (what the right-click used to do) and Close, the same commands as the shortcuts and the palette.

![A window menu on the tab](2026-10-27/131810-a-window-menu-on-the-tab.png)

### Notifications: Open and Dismiss

*13:09* — A selected notification's default action now always reads Open (apps often call it Activate), and Dismiss sits beside it: on a conversation it dismisses every notification in it, on a single one just that one. Its buttons no longer overlap.

![Notifications: Open and Dismiss](2026-10-27/130921-notifications-open-and-dismiss.png)

### Terminal knows its coding agents

*13:03* — Claude Code or Codex in a tab is recognized natively: the tab wears its state (a turning arc while it works, an orange ! when it asks, a green dot when its turn is done) and, when you're looking elsewhere, a notification says it needs you; Show brings that tab forward.

![Terminal knows its coding agents](2026-10-27/130347-terminal-knows-its-coding-agents.png)

### Terminal tabs

*12:38* — Terminal windows hold tabs: ⌘T opens one in the same folder, ⌘[ and ⌘] or Ctrl+Tab move between them, and tabs drag to reorder. A bell in a hidden tab marks it with a dot; Move Tab to New Window splits one off. The window grows by the strip so the grid stays the same, and tabs come back after an upgrade.

![Terminal tabs](2026-10-27/123802-terminal-tabs.png)

### Preferences: op on or off

*12:15* — Preferences ▸ Security has a Command line box to turn op off; it's on by default.

![Preferences: op on or off](2026-10-27/121525-preferences-op-on-or-off.png)

### op asks Passwords

*12:15* — Tabula's op answers scripts that use 1Password's op command (op inject, read, run, item get, whoami). The first time a terminal asks, Passwords names the command, who ran it and where, and Allow covers that terminal until Passwords locks.

![op asks Passwords](2026-10-27/121525-op-asks-passwords.png)

### Notifications history: denser, with views

*11:13* — Two-line rows (icon, bold title with app and time, a line of body), a dot for what is new, day headings and search; List, By App and Cards shown, Conversations stacks a thread. The selected note opens to its whole body and actions.

![Notifications history: denser, with views](2026-10-27/111344-notifications-history-denser-with-views.png)

### Tiled windows move together when one is resized

*10:58* — Activity Monitor tiled to the left half, StyledEdit top right, Preferences bottom right; dragging Preferences' left edge leftwards moved the whole split: Activity Monitor narrower, both right-hand windows wider, the gap kept. Only windows tiled by a tiling command take part; one moved or sized by hand on its own is never pushed. Neighbours stop at their apps' minimum sizes, and the parts they end up with are kept (also across a reload).

![Tiled windows move together when one is resized](2026-10-27/105805-tiled-windows-move-together-when-one-is-resized.png)

### Deskbar: the date and time on their own row, an Agent Screens item

*10:21* — The tray's clock moves to a row of its own (date on the left, time on the right; click for Date & Time), so the well below holds only status icons, each 16 px on a 22 px grid, centred and wrapping to more rows when they don't fit. While agent screens run, a monitor with a pointer shows (with a count when there are several); clicking it opens Agent Screens.

![Deskbar: the date and time on their own row, an Agent Screens item](2026-10-27/102121-deskbar-the-date-and-time-on-their-own-row-an-ag.png)

### Other programs survive an app_server restart

*10:02* — GNOME Text Editor kept its window through tabula-wlproxy: the first line typed before app_server restarted, the second after, with no reconnect in the app.

![Other programs survive an app_server restart](2026-10-27/100230-other-programs-survive-an-app-server-restart.png)

### Chrome's notifications in Tabula

*08:59* — Google Chrome's notifications now come to Tabula's notifications app: a site's notification shows with Chrome's name and icon and the site as a link, a click goes back to the page, and Chrome's Settings button opens its notification settings.

![Chrome's notifications in Tabula](2026-10-27/085940-chrome-s-notifications-in-tabula.png)

### Agent Screens

*04:33* — Agents get screens of their own: each is a whole Tabula desktop with its own windows, pointer and keyboard focus, so an agent never moves your pointer or takes your focus. tabula-screen starts one and drives it (run, look, windows, click, type, key); the Agent Screens window shows it live, and Take Control lets you step in.

![Agent Screens](2026-10-27/043342-agent-screens.png)

### Passwords in Chrome

*04:14* — A field's context menu in Chrome now has Passwords: each login for the page, with its username, password, one-time code or the whole login. A Tabula extension, installed and kept up to date by tabula-chrome itself, talks to Passwords through tabula-chrome as its native messaging host; the toolbar button lists the page's logins, and a login sent with a form is offered for saving or updating.

![Passwords in Chrome](2026-10-27/041413-passwords-in-chrome.png)

### DriveSetup

*03:46* — A new app for disks, after BeOS's DriveSetup: a map of the chosen disk over every disk, partition and unlocked volume. Mount, unlock, format (optionally encrypted), create, resize, rename and delete partitions, new partition tables, write a disk image, eject, check health and benchmark, all through UDisks on worker threads with live progress and Stop. The disk holding the running system is never offered for anything destructive. Shown against udisks-fake, never real disks.

![DriveSetup](2026-10-27/034630-drivesetup.png)

### Drag a field out of Passwords

*03:07* — Drag a username, password, one-time code or website from an item onto another app's field; the drag shows the field's name, never the secret.

![Drag a field out of Passwords](2026-10-27/030729-drag-a-field-out-of-passwords.png)

### Drag text in and out of text fields

*03:03* — Press on selected text and drag it: within a StyledEdit document it moves (styles and all), into another field or app it's copied, onto a folder it becomes a clipping. A caret shows where it will land.

![Drag text in and out of text fields](2026-10-27/030344-drag-text-in-and-out-of-text-fields.png)

### Drag an application out of Deskbar

*02:50* — Dragging an application's row out of Deskbar leaves a link to it where it's dropped, named and drawn as the application; opening the link starts it.

![Drag an application out of Deskbar](2026-10-27/025059-drag-an-application-out-of-deskbar.png)

### GTK windows wear Tabula's chrome

*02:45* — Plain GTK 3 and GTK 4 windows (no headerbar) now get Tabula's tab and frame: app_server speaks KDE's server-decoration protocol, which is how GTK asks for server-side decorations.

![GTK windows wear Tabula's chrome](2026-10-27/024529-gtk-windows-wear-tabula-s-chrome.png)

### Views scroll under a drag

*02:37* — A drag held near the top or bottom of any scrolling view scrolls it on, faster nearer the edge, so a drop can reach what was out of view.

![Views scroll under a drag](2026-10-27/023759-views-scroll-under-a-drag.png)

### Spring-loaded folders

*02:37* — Rest a drag on a folder for a second and it opens, so the drag can go on into it (here a file dragged onto Dest, whose window opened under the pointer).

![Spring-loaded folders](2026-10-27/023707-spring-loaded-folders.png)

### Drop files on an app in Deskbar

*02:34* — Files dragged onto a running application in Deskbar highlight it and open with it when dropped.

![Drop files on an app in Deskbar](2026-10-27/023406-drop-files-on-an-app-in-deskbar.png)

### Drag a picture out of Viewer

*02:30* — A picture that fits Viewer's window drags out as its file (and, for PNG, JPEG, GIF and WebP, the picture itself for programs that take one). Quick Look does the same, from the picture or its footer's icon. Dropping it back into its own folder is refused.

![Drag a picture out of Viewer](2026-10-27/023043-drag-a-picture-out-of-viewer.png)

### Backups have their own icon

*02:23* — Tracker shows .tabula-backup files with the new backup icon; opening one shows it in Preferences ▸ Backup, ready to import.

![Backups have their own icon](2026-10-27/022303-backups-have-their-own-icon.png)

### Backup, dark

*02:23* — The Backup panel in the dark theme.

![Backup, dark](2026-10-27/022303-backup-dark.png)

### Preferences: Backup

*02:23* — Export saves every Tabula setting and piece of saved state in one .tabula-backup file. Import shows what a backup holds, app by app, and asks before replacing anything; the settings as they were are saved first.

![Preferences: Backup](2026-10-27/022302-preferences-backup.png)

### Quick Look rotates pictures

*02:12* — Quick Look's footer has Rotate Left and Right (⌘L and ⌘R), which turn the picture and save it at once, as on the Mac. Anything more opens in Viewer.

![Quick Look rotates pictures](2026-10-27/021250-quick-look-rotates-pictures.png)

### Viewer: Adjust Size

*02:12* — Image ▸ Adjust Size… resizes the picture, keeping its proportions unless asked not to. Rotate, flip, crop and resize can all be undone, and File ▸ Save writes the picture back in its own format (Save As for another).

![Viewer: Adjust Size](2026-10-27/021250-viewer-adjust-size.png)

### Viewer: crop a picture

*02:12* — Image ▸ Crop… puts a frame over the picture: drag one out, move it or pull its edges; the size shows below and the rest dims. Crop (or Return) keeps it, and the title says the picture is edited until it's saved.

![Viewer: crop a picture](2026-10-27/021250-viewer-crop-a-picture.png)

### Tracker says when a long copy is done

*01:58* — Through the kit's new notify API: a copy or move that ran 10 s or more posts “Finished copying …”; clicking it shows the folder.

![Tracker says when a long copy is done](2026-10-27/015834-tracker-says-when-a-long-copy-is-done.png)

### GApplication notifications

*01:58* — tabula-notifications now also serves GLib's org.gtk.Notifications: a GApplication's notification (here a test app's, with an app.reply button) shows like any other, and its actions run on the app over org.freedesktop.Application.

![GApplication notifications](2026-10-27/015833-gapplication-notifications.png)

### Preferences ▸ Notifications

*01:41* — Do Not Disturb by hand or every day at set times, how long banners stay, and per-app banners, sounds and history (apps appear once they have notified). Dark theme.

![Preferences ▸ Notifications](2026-10-27/014143-preferences-notifications.png)

### Deskbar's notification bell

*01:41* — The bell counts unread notifications; click for the history, right-click for Do Not Disturb (now, for an hour, until morning) and the settings. Struck through while quiet.

![Deskbar's notification bell](2026-10-27/014143-deskbar-s-notification-bell.png)

### Notification history

*01:41* — The History window (Deskbar's bell, or tabula-notifications --history): every notification newest first, kept across a restart. Click to act on it (or bring the app forward), × to forget one; Do Not Disturb and Clear All along the top.

![Notification history](2026-10-27/014143-notification-history.png)

### Notifications, grouped

*01:18* — Notifications now gather under their app as on Haiku, keep bold, italics and links, show progress, take a reply in place, and wear a blue edge when important or a red one for errors. A click hands the app an activation token so its window comes forward.

![Notifications, grouped](2026-10-27/011850-notifications-grouped.png)

### Long menus scroll

*01:07* — A menu taller than the screen fits it now, with BeOS scroll arrows at its ends: rest the pointer on one to scroll, or use the wheel or the arrow keys.

![Long menus scroll](2026-10-27/010732-long-menus-scroll.png)

### Hide window, anywhere

*00:54* — ⌘M hides the window in front, Tabula's or any other app's, and Hide window is a ⌘Space command (it finds "minimize" too). Change the keys in Preferences ▸ Keyboard.

![Hide window, anywhere](2026-10-27/005406-hide-window-anywhere.png)

### Log Viewer

*00:46* — A native reader for your journal: Tabula's own programs (or everything), by level and time, with Find, live tail, Show in Context and syntax highlighting on every line. Also a --print command line for agents.

![Log Viewer](2026-10-27/004659-log-viewer.png)

### GTK and Chrome in Tabula's colours

*00:28* — The desktop portals now start in a real session. GTK 3, GTK 4 and libadwaita apps take Tabula's palette, fonts and close-left/zoom-right buttons, in light and dark, and Chrome opens with Tabula's tab and frame instead of its own buttons.

![GTK and Chrome in Tabula's colours](2026-10-27/002846-gtk-and-chrome-in-tabula-s-colours.png)

### Google Chrome, installed on first use

*00:04* — Google Chrome is one of Tabula's applications. The first time it is opened, Tabula offers to download it from Google's own repository, checks it against the repository's checksum, unpacks it natively into the home folder and starts it, drawing with Wayland in a Tabula window.

![Google Chrome, installed on first use](2026-10-27/000453-google-chrome-installed-on-first-use.png)

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
