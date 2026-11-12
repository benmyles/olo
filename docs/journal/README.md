# Screenshot journal

Olo's visual evolution, newest first. Added with `tools/journal.py`
(images are stored with Git LFS; run `git lfs pull` after cloning).

## 2026-11-12

### Deskbar: a film playing on an AirPlay receiver

*15:43* — While olo-airplay plays a film on a receiver, Deskbar's tray has a screen with a play mark on it (two bars while paused, a count when there are several). Its menu says what plays where, with Pause (or Resume) and Stop for each, and Stop All. The mark is placed in device pixels inside the screen's frame: centred at 1×, 1.25×, 1.5×, 1.75× and 2×. Tried against a receiver of the test's own, on the loopback.

![Deskbar: a film playing on an AirPlay receiver](2026-11-12/154356-deskbar-a-film-playing-on-an-airplay-receiver.png)

### Activity Monitor: an NVIDIA card's encoder and decoder

*13:56* — The card's video encoder and decoder are engines of their own, which its library counts apart from graphics: a screen recording encoding on it read as an idle GPU. Each is a series now (here a 4K encode: the encoder at half, graphics at a seventh), and the card, in Activity Monitor and Deskbar's widget, is as busy as its busiest engine. Intel's render and media engines the same.

![Activity Monitor: an NVIDIA card's encoder and decoder](2026-11-12/135642-activity-monitor-an-nvidia-card-s-encoder-and-de.png)

### Activity Monitor: a fan profile that isn't in force says so

*11:08* — The Fans card said "the fan service isn't running" of a service that was running and couldn't set the fans. It reads the service's state through olo_system::fans now: not in force and why (the driver, until the next start; the service stopped), or the level held.

![Activity Monitor: a fan profile that isn't in force says so](2026-11-12/110802-activity-monitor-a-fan-profile-that-isn-t-in-for.png)

### Olo Doctor: the Cool fan profile that isn't in force

*11:07* — Cool chosen and the fans' driver taking no levels (it had been told to by hand, which a restart undid): found, with the fix that tells it to at every start. Also found: the fan service stopped, and one an upgrade has left older.

![Olo Doctor: the Cool fan profile that isn't in force](2026-11-12/110751-olo-doctor-the-cool-fan-profile-that-isn-t-in-fo.png)

### Preferences ▸ Performance: the scheduler

*04:38* — A Scheduler tab (Ben: "add to olo the ability to pick a custom scheduler in preferences -> performance and the recommendation should be scx_lavd"): the kernel's own or LAVD (recommended), what each is for, and what is scheduling now as the kernel says it. Choosing one has Olo Doctor install and start it, or stop it (an administrator's password). Shown with a made-up kernel in a scratch folder.

![Preferences ▸ Performance: the scheduler](2026-11-12/043804-preferences-performance-the-scheduler.png)

### Preferences ▸ Power: the fan profile

*04:20* — Battery & Idle has a Fans box on a ThinkPad (Ben: "have in the power prefs the option for the default auto and this new cooler/louder auto profile"): Automatic, the firmware's own curve, or Cool, Olo's olo-fans running the fans ahead of the heat so the processor isn't throttled, with what the fans are doing now under it. Choosing one asks Olo Doctor to set it (an administrator's password). Shown with a made-up ThinkPad in a scratch folder.

![Preferences ▸ Power: the fan profile](2026-11-12/042022-preferences-power-the-fan-profile.png)

### The Installer

*03:59* — As it opens once the licence is agreed to: what will happen, where from and onto what, More Options closed, About, Quit and Begin. BeOS's Installer for Olo, in place of the Windows 95 wizard of five pages.

![The Installer](2026-11-12/035914-the-installer.png)

### The Installer, at work

*03:59* — Olo Setup redone after BeOS's Installer (Ben: "redo the installer to be reminiscent of the beos installer instead of windows 95"): one window on the BeOS desktop's blue, with Olo's mark, a box of words saying what is being done, Install from and Onto as pop-ups, More Options opening the optional parts under a latch, a status bar, and Begin, which is Stop while it works. Shown in its demo mode (a made-up computer; nothing is installed).

![The Installer, at work](2026-11-12/035913-the-installer-at-work.png)

## 2026-11-11

### Preferences on olo-kit

*22:34* — Preferences, every panel, on Olo's own kits: each panel's window a view on a looper of its own. The same to the pixel as it was but for two things meant: the launcher's cells are wider (the longest names ran into the cell beside them) and Notifications' From and to line up with the words above them. Typing in a field draws in a quarter of the time, a slider in half.

![Preferences on olo-kit](2026-11-11/223417-preferences-on-olo-kit.png)

### Terminal on olo-kit

*15:05* — Terminal on Olo's own kit: one Terminal behind a lock, each window a view on a looper of its own, its screens a view of their own drawn once a frame however much a shell prints. The same to the pixel as before at every scale, light and dark; a frame of typing 475 to 240 µs, of steady output 532 to 305 µs.

![Terminal on olo-kit](2026-11-11/150502-terminal-on-olo-kit.png)

### Passwords on olo-kit

*12:08* — Passwords moved from GPUI to Olo's own kits: its state is the app's, behind a lock, since it runs with no window as the SSH agent and the keychain; the main window, Review Sync, Sync Problems, the generator and a passkey's question are views that read it. The same to the pixel as before at 1×, 1.5× and 2×, light and dark, but for what was wrong: the item pane's scroll bar is there as an item opens (it came with the first scroll), Sync Problems' icons are level with their items at 1×, and “Synced a minute ago” counts the minutes by itself. A frame typing in the editor went from 1.9 ms to 0.3, the arrows down a list of 508 from 1.8 to 0.8.

![Passwords on olo-kit](2026-11-11/120840-passwords-on-olo-kit.png)

### Activity Monitor: the fans against what they cool

*04:00* — Sensors has a Fans card in place of a small graph a fan: each fan's speed, the processor's temperature and how often it was throttled for heat on one graph (each to its own scale, the fans to one), the fan profile in use and the level Olo's fan service holds, and the throttling counted since the computer started and since the window opened, so a fan curve can be seen to work. (Activity Monitor is on olo-kit as of today.)

![Activity Monitor: the fans against what they cool](2026-11-11/040027-activity-monitor-the-fans-against-what-they-cool.png)

### Screenshot: New screenshot shows the new picture

*02:45* — Screenshot moved to Olo's own kit. Its preview was kept while the picture's size was the same, so New screenshot went on showing the one before; a preview is now of one picture. The region and colour pickers show the screen a pixel to a device pixel at 1.5× and 1.75× too (the overlay's whole-point size was half a device pixel off the screen's, and the frozen picture was stretched by it).

![Screenshot: New screenshot shows the new picture](2026-11-11/024515-screenshot-new-screenshot-shows-the-new-picture.png)

## 2026-11-09

### Olo Doctor finds Token Pool running twice

*21:59* — A Token Pool started beside the one serving, unable to take its port, served nothing for two days while its window showed no requests. Olo Doctor now finds it and quits it; the session starts it again, and it keeps the one serving rather than running beside it.

![Olo Doctor finds Token Pool running twice](2026-11-09/215953-olo-doctor-finds-token-pool-running-twice.png)

### Terminal: each agent's own Keep It Going message

*20:21* — Keep It Going is a submenu in a tab's menu: on or off, the default message, or one of the tab's own (Custom Message…), kept with the tab. Keep It Going and Auto Approve now work with the real Claude Code and Codex: Codex's reasoned questions are answered, Claude Code's blinking marker no longer delays the answer, and nothing is typed into a chooser.

![Terminal: each agent's own Keep It Going message](2026-11-09/202116-terminal-each-agent-s-own-keep-it-going-message.png)

### Videos and sounds play at 0.25× to 2×

*19:29* — Quick Look and the Viewer have a speed button at the transport's right end: it says the speed and opens the speeds over itself, 0.25× to 2×; > and < step faster and slower, and the Viewer's View menu has them too. The sound keeps its pitch (time-stretched, as SoundTouch does it) and the picture follows it.

![Videos and sounds play at 0.25× to 2×](2026-11-09/192932-videos-and-sounds-play-at-0-25-to-2.png)

### AppImages show their own icons everywhere

*18:04* — An AppImage's icon, read out of its image, now shows for its file in Tracker (with or without the .AppImage extension), for its running windows in Deskbar and the Twitcher, and under its own name in the window menu. Each AppImage known gets a hidden desktop entry, so every app lookup finds it; one run from a folder nothing looked at is found by its process (here DBDelve, run from a folder of its own).

![AppImages show their own icons everywhere](2026-11-09/180432-appimages-show-their-own-icons-everywhere.png)

### Window tab menu: Close Others in a group

*16:01* — A window in a group: Close Others after Close closes every other window of its group, each as its close box would (one with unsaved work asks).

![Window tab menu: Close Others in a group](2026-11-09/160104-window-tab-menu-close-others-in-a-group.png)

### Terminal: Auto Approve in a tab's menu

*15:21* — Beside Keep It Going (and in Tabs). Turned on from the commands socket (an agent, the palette), it asks the person first.

![Terminal: Auto Approve in a tab's menu](2026-11-09/152147-terminal-auto-approve-in-a-tab-s-menu.png)

### Terminal: Auto Approve counts down to Yes

*15:21* — A coding agent's "Do you want to proceed?" (here a stand-in drawing Claude Code's dialog) answered Yes after 30 s (6 s in this test), unless the person types in the tab or presses Stop; the countdown in the pane's corner.

![Terminal: Auto Approve counts down to Yes](2026-11-09/152145-terminal-auto-approve-counts-down-to-yes.png)

### Telegram: folders behind More, a submenu each

*14:50* — A right-click on the More chip: each folder behind it with Mark All as Read, Mute All, Unmute All. Demo account.

![Telegram: folders behind More, a submenu each](2026-11-09/145016-telegram-folders-behind-more-a-submenu-each.png)

### Telegram: a right-click on the archive acts on all its chats

*14:50* — Mark All as Read, Mute All, Unmute All and Unarchive All… on the archive's row; folders' chips, Unknown Senders and the More chip (a submenu each) have the same; Chat ▸ Mark All as Read for the list shown. Demo account.

![Telegram: a right-click on the archive acts on all its chats](2026-11-09/145014-telegram-a-right-click-on-the-archive-acts-on-al.png)

### DiskBackup's title fits beside its buttons

*10:43* — Shorter titles ("Counting what to back up", "Backup didn't finish"), and a folder's path cut short with its whole in the tip. The layout review now finds text that runs over a control (text-over-control), and the lint draws DiskBackup's window in all 25 of its states.

![DiskBackup's title fits beside its buttons](2026-11-09/104331-diskbackup-s-title-fits-beside-its-buttons.png)

### DiskBackup's title over its buttons (before)

*10:43* — While a backup started, its title ("Getting ready: counting what to back up", or a folder named by its path) ran on over Restore… and Stop: the layout lint didn't look at DiskBackup, and had no check for text over a control.

![DiskBackup's title over its buttons (before)](2026-11-09/104330-diskbackup-s-title-over-its-buttons-before.png)

### DiskBackup: waiting for the network

*10:02* — No network mid-backup (a cable pulled, Wi-Fi left): the backup waits for one and says so, and goes on by itself, from where it got to, once it's back; a connection on a network that went is cut at once and the request asked again on the new one (olo-net). A sandboxed service in a network namespace of its own, its way out taken away.

![DiskBackup: waiting for the network](2026-11-09/100217-diskbackup-waiting-for-the-network.png)

## 2026-11-08

### Token Pool shows every quota an account has

*23:52* — The session (5-hour, “not started” before its first request), the week, and each model's own week (Weekly Fable, Weekly Sonnet), as Claude's usage report lists them; Codex's own limits too. Made-up accounts.

![Token Pool shows every quota an account has](2026-11-08/235228-token-pool-shows-every-quota-an-account-has.png)

### Terminal: what Keep It Going says, yours to choose

*23:16* — Preferences ▸ Terminal ▸ Coding Agents: Keep It Going says “Please continue” unless you change it; Use Default puts it back. Terminal follows it at once.

![Terminal: what Keep It Going says, yours to choose](2026-11-08/231638-terminal-what-keep-it-going-says-yours-to-choose.png)

### Stopping a backup, said as it goes

*22:56* — Stopping the moment Stop is clicked (or an upgrade asks): the pack being sent finished first, how much of it and how long, then what's done is kept for the next to go on from.

![Stopping a backup, said as it goes](2026-11-08/225647-stopping-a-backup-said-as-it-goes.png)

### DiskBackup says what it's doing, plainly

*22:56* — What it backs up (your home folder), how far through how much (more than the last backup's, when it is), files, time left, then what's new to the backup and what's sent, compressed and encrypted; Deskbar's menu says the same.

![DiskBackup says what it's doing, plainly](2026-11-08/225646-diskbackup-says-what-it-s-doing-plainly.png)

### Connecting Google Drive again: the sign-in from before filled in

*21:34* — After Disconnect, Connect Google Drive… fills in the client made before and folds the steps away (Show the Steps); with them shown the window now fits 1280 x 800 (708 points).

![Connecting Google Drive again: the sign-in from before filled in](2026-11-08/213421-connecting-google-drive-again-the-sign-in-from-b.png)

### DiskBackup validating: what's read back, of how much

*21:34* — Validate Fully at a 2 MB/s download limit: every pack the backups need read back and checked, as a restore would; Stop works throughout.

![DiskBackup validating: what's read back, of how much](2026-11-08/213421-diskbackup-validating-what-s-read-back-of-how-mu.png)

### DiskBackup's Destination: what it's connected to, and Disconnect

*21:34* — Whose Google Drive, its folder, Connected, what the backups take and the Drive's space (as Google says); Disconnect…, Use a Folder Instead…, Open in Google Drive. A folder says its disk's space.

![DiskBackup's Destination: what it's connected to, and Disconnect](2026-11-08/213420-diskbackup-s-destination-what-it-s-connected-to-.png)

### DiskBackup: unfinished backups, and each one validated

*21:34* — A backup that's stopped keeps what it got through and the next goes on from it (listed first, Unfinished, deletable to free what it sent). Each backup is validated fast once it's made (checksums compared with the store's, a sample read back); Validate does that for any, Validate Fully reads every byte back.

![DiskBackup: unfinished backups, and each one validated](2026-11-08/213419-diskbackup-unfinished-backups-and-each-one-valid.png)

### DiskBackup's Backups tab: every snapshot, delete some or erase all

*15:18* — Choose one or several backups and delete them (asked first; the space only they took is freed at once), or Erase All Backups to start fresh.

![DiskBackup's Backups tab: every snapshot, delete some or erase all](2026-11-08/151843-diskbackup-s-backups-tab-every-snapshot-delete-s.png)

### DiskBackup, redesigned: what it's doing now, its plan, every setting

*15:18* — Ben asked for an intuitive, full-featured backup app. A header says whether your files are safe; while a backup runs, the file it's at, a bar of bytes against the plan, files, time left, what's new and sent against the upload limit. Five tabs: Schedule (automatic or only when you click Back Up Now; what's kept, said plainly), Files, Backups, Speed & Resources (upload and download limits; the priority: idle, gently or full speed), Destination. Fits 1280×800 while backing up.

![DiskBackup, redesigned: what it's doing now, its plan, every setting](2026-11-08/151842-diskbackup-redesigned-what-it-s-doing-now-its-pl.png)

### Preferences ▸ AI: Claude Code's model

*13:40* — The model and thinking Olo starts Claude Code with for a task (DiskBackup's Agent Setup): Opus 5.5 on Medium by default.

![Preferences ▸ AI: Claude Code's model](2026-11-08/134047-preferences-ai-claude-code-s-model.png)

### DiskBackup: Agent Setup

*13:40* — Back Up to Google Drive ▸ Let an Agent Do It…: choose the Google account's login from Passwords; DiskBackup starts an agent screen, shares the login with it, and opens Claude Code in a Terminal window to make Google's sign-in for DiskBackup there, then hand it back (olo-diskbackup --agent-setup-done), with the consent page allowed on the agent's screen. Tried for real: the agent did the steps in 9 minutes.

![DiskBackup: Agent Setup](2026-11-08/134046-diskbackup-agent-setup.png)

### Move to Window, for every tab

*12:44* — A tab's menu, and a tab group's, has Move to Window ▸ with the app's other windows by title: the kit's Menu::move_to_window. Terminal's tabs and tab groups move there (a group stays a group), and the window they leave closes if nothing's left; a window's own tab (here StyledEdit's) joins the other window's group.

![Move to Window, for every tab](2026-11-08/124414-move-to-window-for-every-tab.png)

### Night Mode: shift gradually

*12:29* — Preferences ▸ Appearance ▸ Night Mode can warm gradually: scheduled, it shifts from neutral to its warmth over 1 minute to 3 hours from its start, in steps too small to see, and is back to neutral at its end as before (no shift out).

![Night Mode: shift gradually](2026-11-08/122912-night-mode-shift-gradually.png)

### Deskbar: Group All Windows, as the windows are

*12:05* — Deskbar's menu for an app says what its windows are, not a setting: Group All Windows when they aren't all in one group (here one was taken out by hand), Ungroup All Windows when they are. Group All puts every one back in; Ungroup All takes each out in a cascade, every tab showing. Before, Group Windows was ticked whenever the app's grouping was on, even with windows out of the group.

![Deskbar: Group All Windows, as the windows are](2026-11-08/120547-deskbar-group-all-windows-as-the-windows-are.png)

### Security ▸ Sign In: Touch ID, Face Unlock and Security Keys together

*11:50* — Preferences ▸ Security has two tabs. Sign In has every way in besides the password, each standing for it wherever it's asked, all at once. Touch ID, which was a panel of its own, is a box beside Face Unlock and Security Keys. The Passwords tab has Passwords' and programs' secrets settings. The old names (touchid, fingerprint, passwords) open the right tab.

![Security ▸ Sign In: Touch ID, Face Unlock and Security Keys together](2026-11-08/115020-security-sign-in-touch-id-face-unlock-and-securi.png)

### Every way in at once: the finger and the password

*11:50* — An administrator request (polkit) with Touch ID set up: the password, or a security key's touch, is taken while the fingerprint sensor listens, not after it gives up. Once polkit's PAM listens for a finger, a second conversation asks for the password; the first to succeed counts. sudo's window in Terminal does the same. Setting up Touch ID had made Ben's YubiKey wait behind the fingerprint.

![Every way in at once: the finger and the password](2026-11-08/115020-every-way-in-at-once-the-finger-and-the-password.png)

### Keyboard ▸ Power Button

*11:08* — Preferences ▸ Keyboard has a Power Button tab: what the computer's own power button does when pressed, and when held for a number of seconds, each from Nothing, Lock the Screen, Log Out, Restart (asking first, or not, or immediately) and Shut Down (the same three). By default a press does nothing, holding it 3 seconds asks to restart (Deskbar's Restart alert), and 10 seconds shuts down at once. Immediately needs Olo's small helper, which Install the Helper… (Olo Doctor's fix) puts in place. It is a tab of its own because the Keyboard tab with it no longer fit a 1280 × 800 screen.

![Keyboard ▸ Power Button](2026-11-08/110828-keyboard-power-button.png)

### Where Olo's login screen can't show, GDM's takes the seat

*09:33* — A GPU Olo can't draw on (the VM's plain virtio GPU): after Olo's splash, GDM's login screen, started by systemd through olo-login's OnFailure=. Before, SELinux kept olo-login from starting it and the screen stayed on the console.

![Where Olo's login screen can't show, GDM's takes the seat](2026-11-08/093318-where-olo-s-login-screen-can-t-show-gdm-s-takes-.png)

### Olo's boot splash on a Fedora 45 that Olo Setup set up

*09:33* — In the installer check's VM, booting as a stock install does (rhgb quiet): Plymouth shows Olo's theme, the wordmark, the four icons and Starting Olo…. Not seen in that VM before: its text install had left the serial console and no rhgb.

![Olo's boot splash on a Fedora 45 that Olo Setup set up](2026-11-08/093317-olo-s-boot-splash-on-a-fedora-45-that-olo-setup-.png)

### Olo after Setup on a fresh Fedora, in use

*08:41* — Tracker on Home, StyledEdit, Olo Doctor (two suggestions, no other desktop's daemons) and Preferences switched to dark.

![Olo after Setup on a fresh Fedora, in use](2026-11-08/084127-olo-after-setup-on-a-fresh-fedora-in-use.png)

### Olo's login screen after Setup, its steps now shared with Doctor

*08:41* — After the restart: the login-screen stage Setup and Olo Doctor share since 3a040276 (olo_doctor::login_screen), tried on a stock Fedora for the first time since that move.

![Olo's login screen after Setup, its steps now shared with Doctor](2026-11-08/084126-olo-s-login-screen-after-setup-its-steps-now-sha.png)

### Olo Setup: Olo Is Installed

*08:41* — Olo 2026.11.08-66af29d4 installed, with its dedication. Retaken, as above.

![Olo Setup: Olo Is Installed](2026-11-08/084126-olo-setup-olo-is-installed.png)

### Olo Setup: Installing

*08:41* — Step 3 of 8: the release downloaded (947 MB) and checked; the stages so far below. Retaken, as above.

![Olo Setup: Installing](2026-11-08/084125-olo-setup-installing.png)

### Olo Setup: Choose What to Install

*08:41* — On a fresh Fedora 45 in a VM, Olo's login screen and boot splash chosen. Retaken: the 2026-11-07 picture of this page had only its top left corner (a 1× shot cropped as if 2×).

![Olo Setup: Choose What to Install](2026-11-08/084124-olo-setup-choose-what-to-install.png)

### Auto-tiling: Main and Stack, and a layout picker

*05:01* — Auto-tiling has a second layout, Main and Stack (as dwm's): a new window is the main one on the left, the others stack on the right, each as tall. Chosen from Deskbar's windowing mode menu (Auto-Tiling Layout) or Preferences ▸ Windows. Every tile changing size now lands in the same frame (one round).

![Auto-tiling: Main and Stack, and a layout picker](2026-11-08/050141-auto-tiling-main-and-stack-and-a-layout-picker.png)

### Telegram: animated stickers play

*04:25* — Lottie stickers (Telegram's .tgs) now play: once as they come into view, and again while the pointer is over them, each frame drawn at the sticker's own device size on a worker (velato, vendored with its panics on real files fixed, drawn by vello_cpu). The demo's is Olo's own beating heart, caught mid-beat with its ring.

![Telegram: animated stickers play](2026-11-08/042517-telegram-animated-stickers-play.png)

### Telegram: voice messages recorded

*03:37* — Nothing to send, Telegram's send button is a microphone: hold it and speak (the time and live levels in the field), let go on it to send, away from it to cancel; a click records until Send, × or Escape. Opus in Ogg, coded in process (rusty-opus, kept to SILK). Shown at 2× with a test's fake microphone.

![Telegram: voice messages recorded](2026-11-08/033705-telegram-voice-messages-recorded.png)

### Passwords: a terminal's op or ssh allowed with a touch or a look

*03:14* — The Allow alert for a terminal session's op, or its first SSH signature, now says what else allows it (Touch ID, a security key set up, face unlock) and waits for it while open: a touch here took it away and op went on. Shown at 2× with a test's soft key.

![Passwords: a terminal's op or ssh allowed with a touch or a look](2026-11-08/031430-passwords-a-terminal-s-op-or-ssh-allowed-with-a-.png)

### The overview: windows dragged between workspaces, and New Workspace

*02:55* — In the overview (⌃Space) a window dragged onto another workspace's pane goes there, and onto New Workspace (the strip at the side) to an empty one, made if every one has windows; the overview stays out and lays itself out again. A click on New Workspace shows an empty one.

![The overview: windows dragged between workspaces, and New Workspace](2026-11-08/025528-the-overview-windows-dragged-between-workspaces-.png)

### Always on Top, from a window's menu

*02:42* — A window's tab menu has Always on Top: kept above the others (its dialogs over it still, picture in picture over it), across a reload; agents set it with set_on_top.

![Always on Top, from a window's menu](2026-11-08/024204-always-on-top-from-a-window-s-menu.png)

### Every app clean at 1.25× and 1.75×

*02:00* — The layout review at 1.25× and 1.75× had found 26 errors and 10 near-misses across the apps, all there before: Power's grid (here, even now), boxes' insets, right-aligned labels, text fields in rows, Screen Saver's monitor stand, Telegram's panes, check boxes beside menus, Network's columns, About's wordmark. Every app and panel is clean at 1, 1.25, 1.5, 1.75 and 2×.

![Every app clean at 1.25× and 1.75×](2026-11-08/020055-every-app-clean-at-1-25-and-1-75.png)

### Security Keys: a key with a PIN

*00:53* — A security key with a PIN set is set up with it (asked once, not kept): the key's PIN beside the account password, a wrong one said with the tries left. Its Remove works beside Face Unlock's (the two Remove buttons had one id and lost their clicks).

![Security Keys: a key with a PIN](2026-11-08/005344-security-keys-a-key-with-a-pin.png)

### Notifications: Undo Clear All and Forget

*00:33* — The History window's Edit ▸ Undo brings back what Clear All or Forget took, where it was (closed at its app by now); Redo forgets it again.

![Notifications: Undo Clear All and Forget](2026-11-08/003340-notifications-undo-clear-all-and-forget.png)

### DiskBackup: Edit ▸ Undo for its settings

*00:27* — Each settings change in DiskBackup's window is a step named for what it changed (Leave Out, Keep, Speed, Patterns typed as one run); Undo puts back only that, leaving what the service or a setup changed since.

![DiskBackup: Edit ▸ Undo for its settings](2026-11-08/002700-diskbackup-edit-undo-for-its-settings.png)

### Disk Usage: Undo Move to Trash

*00:20* — Edit ▸ Undo puts what Disk Usage moved to the Trash back where it was (its sizes back in the chart); Redo moves it again. Deleting for good still says it can't be undone.

![Disk Usage: Undo Move to Trash](2026-11-08/002025-disk-usage-undo-move-to-trash.png)

## 2026-11-07

### DiskBackup: Back Up to Google Drive, walked through

*22:34* — Google asks each person to make their own sign-in for an app like DiskBackup, so DiskBackup walks them through it: six steps in Google Cloud, each with Open for its page, then the client's file (or its ID and secret) and Sign In with Google. A client kept in Passwords skips the steps on another computer.

![DiskBackup: Back Up to Google Drive, walked through](2026-11-07/223440-diskbackup-back-up-to-google-drive-walked-throug.png)

### Tracker's list view: one edge at its foot

*21:54* — The count bar under the list view no longer draws its etched edge: the list's own sunken frame (its focus ring inside) edges it, and the two had stacked four lines in three points (tsession review: crowded lines, at 1× and 2×). Light and dark, 2× and 1×.

![Tracker's list view: one edge at its foot](2026-11-07/215408-tracker-s-list-view-one-edge-at-its-foot.png)

### DiskBackup's own icon, in every theme

*21:54* — A hard drive with a cloud before it, a green tick on the cloud (backed up, safe elsewhere), in place of Preferences ▸ Backup's drive and arrow (top row of each band: DiskBackup; under it, Backup's), at 64 and 16 px on the light, dark and night panels.

![DiskBackup's own icon, in every theme](2026-11-07/215408-diskbackup-s-own-icon-in-every-theme.png)

### Tracker: DiskBackup's ticks and Older Versions

*21:40* — Files whose current version is backed up wear a green tick, in every view and on the desktop (a changed file loses it until the next backup). A file's Older Versions… opens DiskBackup's Versions window: each version kept, the one there now said; Restore keeps both or replaces.

![Tracker: DiskBackup's ticks and Older Versions](2026-11-07/214008-tracker-diskbackup-s-ticks-and-older-versions.png)

### DiskBackup in Deskbar's tray

*21:10* — A drive in a circular arrow: idle, backing up (green), not backing up (dim: off, on battery), the last didn't finish (red dot), nothing for days (amber). Its menu, read as it opens: what it's doing, the last and next, Stop or Back Up Now, Pause, Restore Files…, DiskBackup…. Light and dark at 2×.

![DiskBackup in Deskbar's tray](2026-11-07/211021-diskbackup-in-deskbar-s-tray.png)

### DiskBackup: Keep, and tidying

*20:44* — How long snapshots are kept (hourly, daily, weekly, monthly), beside When; Speed moved beside Back Up To so the window still fits 1280 × 800. The status says what the backups take there and the last tidy (old snapshots let go of, space freed).

![DiskBackup: Keep, and tidying](2026-11-07/204457-diskbackup-keep-and-tidying.png)

### Preferences ▸ Deskbar

*13:16* — The new Deskbar panel: the widgets in order (Add, Remove, Move Up, Move Down) beside the chosen one's settings: what it shows and which, where, look, size, name and value, how often, how far back, colour. Each change shows in the Deskbar at once.

![Preferences ▸ Deskbar](2026-11-07/131634-preferences-deskbar.png)

### Deskbar: live widgets

*13:16* — Deskbar's new live widgets, as BeOS's Pulse and ProcessController put bars in its tray: a temperature and a CPU graph in the tray; under it a CPU graph, each core's bars, a memory-and-swap meter and the network's down and up; at the foot, the GPU and the system volume. Each is read on a worker only while shown, as often as it asks.

![Deskbar: live widgets](2026-11-07/131634-deskbar-live-widgets.png)

### Olo Doctor: ignored findings, quietly last

*10:49* — Every finding has Ignore beside its fix. What's ignored leaves the count, Fix All, the tray's bag and the notifications, and is listed last under Ignored, dim, with Stop Ignoring and its fix should the person change their mind. Doctor also offers Olo's own login screen where another's greets (not shown here: the sandbox's build has no olo-login).

![Olo Doctor: ignored findings, quietly last](2026-11-07/104951-olo-doctor-ignored-findings-quietly-last.png)

### Power ▸ Graphics says what each GPU is on now

*01:28* — Ben asked whether changing the power mode changes the NVIDIA GPU's clock. It does in Power Saver (its clock capped at 70%), but its box showed only its Custom slider, at the card's top clock whatever the mode, while Intel's sliders moved: now each GPU's line says what the mode has it on, in numbers (here Performance: Intel 1650–2450 MHz; NVIDIA as its driver has it, since a laptop's takes no power limit), and Custom's sliders show Custom's own settings, which don't move with the mode.

![Power ▸ Graphics says what each GPU is on now](2026-11-07/012824-power-graphics-says-what-each-gpu-is-on-now.png)

### Olo on a fresh Fedora 45, installed by Setup

*00:39* — Logged in through Olo's login screen; Home open in Tracker and Preferences switched to dark.

![Olo on a fresh Fedora 45, installed by Setup](2026-11-07/003932-olo-on-a-fresh-fedora-45-installed-by-setup.png)

### Olo's login screen after Setup, on a fresh Fedora 45

*00:39* — Restarted from Setup's last page: Olo's own login screen in GDM's place, Olo Desktop chosen.

![Olo's login screen after Setup, on a fresh Fedora 45](2026-11-07/003931-olo-s-login-screen-after-setup-on-a-fresh-fedora.png)

### Olo Setup: Welcome, full screen on a stock Fedora 45

*00:39* — Windows 95's Setup in BeOS's look: the blue backdrop and big serif title, a BeOS window for each page. Downloaded and run in a fresh Fedora 45 Workstation VM, 1x.

![Olo Setup: Welcome, full screen on a stock Fedora 45](2026-11-07/003929-olo-setup-welcome-full-screen-on-a-stock-fedora-.png)

## 2026-11-06

### Touchpad & Mouse: the wheel's scrolling speed

*09:49* — One speed for a wheel's notch in every program, Chrome included (app_server scales it before it's sent); at 1.5×

![Touchpad & Mouse: the wheel's scrolling speed](2026-11-06/094942-touchpad-mouse-the-wheel-s-scrolling-speed.png)

### Viewer: a video at Actual Size, Zoom to Fit and Zoom to Fill

*09:49* — View ▸ Actual Size (⌘0), Zoom to Fit (⌘9), Zoom to Fill (⇧⌘9), proportions kept; Enter Full Screen (F), the transport over the picture while the pointer moves

![Viewer: a video at Actual Size, Zoom to Fit and Zoom to Fill](2026-11-06/094941-viewer-a-video-at-actual-size-zoom-to-fit-and-zo.png)

### StyledEdit: Markdown table columns aligned

*04:14* — A Markdown table's columns align as its delimiter row says (:--- left, :---: centre, ---: right): the editor's rows take each column's own alignment (Row::aligns).

![StyledEdit: Markdown table columns aligned](2026-11-06/041458-stylededit-markdown-table-columns-aligned.png)

### StyledEdit: Markdown pictures

*02:25* — ![alt](file) shows as the picture it names (a file beside the document), off the caret's line.

![StyledEdit: Markdown pictures](2026-11-06/022520-stylededit-markdown-pictures.png)

### StyledEdit: Markdown tables as grids

*02:25* — A Markdown table shows as a ruled grid (the editor's table rows, its cells the text between the pipes, columns as wide as their longest cell, the header shaded, the |---| line hidden off the caret), typed in cell by cell: Tab to the next, Return adds a row.

![StyledEdit: Markdown tables as grids](2026-11-06/022519-stylededit-markdown-tables-as-grids.png)

### StyledEdit: source code coloured by syntax

*01:49* — A source file (by its name, extension or #! line) opens in the kit's code view: tree-sitter grammars for 43 languages, parsed on a worker, colours from the palette (dark mode here), line numbers in a gutter.

![StyledEdit: source code coloured by syntax](2026-11-06/014923-stylededit-source-code-coloured-by-syntax.png)

### StyledEdit: Markdown's source, ⌘/

*01:49* — Document ▸ Show Source (⌘/) shows the same text as a code editor: the monospaced face, line numbers, headings, links, code and marks coloured; the caret and view kept across the switch.

![StyledEdit: Markdown's source, ⌘/](2026-11-06/014922-stylededit-markdown-s-source.png)

### StyledEdit: Markdown's marks show on the caret's line

*01:49* — On the line being edited, the marks show dimmed, as typed (*emphasis*, **strong**, ~~struck~~, backticks, [link](address)); elsewhere they're hidden.

![StyledEdit: Markdown's marks show on the caret's line](2026-11-06/014922-stylededit-markdown-s-marks-show-on-the-caret-s-.png)

### StyledEdit: Markdown as it looks

*01:49* — A .md file opens as a Markdown document edited as it looks (Typora's way): headings, emphasis, inline code on its ground, links, bullets, numbers and task boxes hanging in their indents, a quotation's bar, a code block coloured by its fence's language, front matter; the marks that make them hidden off the caret's line.

![StyledEdit: Markdown as it looks](2026-11-06/014920-stylededit-markdown-as-it-looks.png)

### Terminal: Keep It Going for a coding agent that stopped

*00:43* — On a tab with Claude Code or Codex, its menu has Keep It Going: after two minutes with nothing at all changing on its screen (Codex's goal clock doesn't count) and its turn over, Terminal says "please continue" to it, at most once an hour. The tab's icon carries a blue disc with a go-on triangle while it's on.

![Terminal: Keep It Going for a coding agent that stopped](2026-11-06/004330-terminal-keep-it-going-for-a-coding-agent-that-s.png)

## 2026-11-05

### Screen recordings on the Intel GPU: AV1 through VA-API

*23:36* — Preferences ▸ GPUs ▸ Screen Recordings offers the Intel GPU where its VA-API driver encodes AV1 (Fedora's drivers make no H.264 or HEVC): recordings are AV1 at constant quality, a 2560 × 1600 screen in about 9 ms a frame, the NVIDIA GPU left asleep.

![Screen recordings on the Intel GPU: AV1 through VA-API](2026-11-05/233632-screen-recordings-on-the-intel-gpu-av1-through-v.png)

### Remote control in Deskbar, with Stop Remote Control

*23:16* — While a program controls the computer, Deskbar's sharing item says so (An app is controlling your keyboard and pointer) and Stop Remote Control ends the session: app_server closes its libei connection and lets go of keys left down.

![Remote control in Deskbar, with Stop Remote Control](2026-11-05/231637-remote-control-in-deskbar-with-stop-remote-contr.png)

### Remote control: the picker asks to share and control

*23:16* — The RemoteDesktop portal: a program that wants to see a screen and control the keyboard and pointer (Chrome Remote Desktop, Deskflow, a call's remote control) gets the screen picker, titled Share and Control Your Screen, saying what it will control, its button Allow. For control alone, the permission dialog asks. Never remembered: asked every time.

![Remote control: the picker asks to share and control](2026-11-05/231637-remote-control-the-picker-asks-to-share-and-cont.png)

### Quick Look scrolls without the pointer moving first

*22:52* — Space in Tracker opened Quick Look under a still pointer, and a touchpad scroll went nowhere until the pointer moved: Smithay only looks at what's under the pointer when it moves. app_server now enters what is under the pointer before a scroll or a click, and as a window comes on screen, forward or away; GPUI takes the place it's entered at as a move, so the cursor and hover are right at once. Here Quick Look, scrolled by the touchpad straight after Space.

![Quick Look scrolls without the pointer moving first](2026-11-05/225243-quick-look-scrolls-without-the-pointer-moving-fi.png)

### Preferences ▸ Touchpad on a ThinkPad: its own palm model

*21:26* — On the ThinkPad's pad: palms kept palms until they lift, Olo's own model for this kind of touchpad (made from Ben's hands), and no pressure controls it has no use for.

![Preferences ▸ Touchpad on a ThinkPad: its own palm model](2026-11-05/212614-preferences-touchpad-on-a-thinkpad-its-own-palm-.png)

### Palm trainer on the ThinkPad: a palm drawn as one

*21:26* — The ThinkPad's pad gives no contact size (its pressure is the click's force: a resting palm weighs 0), so palms were drawn as dots. Now a palm the touchpad marks is drawn palm-sized and a fingertip fingertip-sized; the trainer runs the ThinkPad's own model.

![Palm trainer on the ThinkPad: a palm drawn as one](2026-11-05/212613-palm-trainer-on-the-thinkpad-a-palm-drawn-as-one.png)

### olo-slack: Slack's notifications are Olo's

*20:31* — A Slack message as an Olo notification (Message style, Slack's edge); a click goes to the conversation. Shown here from a stand-in page.

![olo-slack: Slack's notifications are Olo's](2026-11-05/203111-olo-slack-slack-s-notifications-are-olo-s.png)

### olo-slack: Slack as an Olo app

*20:31* — Slack's web app in Olo's Chrome on a profile of its own; its window is olo.slack, so Deskbar names it Slack with Olo's icon.

![olo-slack: Slack as an Olo app](2026-11-05/203111-olo-slack-slack-as-an-olo-app.png)

### Starting Olo: the start-up screen

*19:47* — As a session starts, every display shows Olo's start-up screen (the upgrade veil's look, its own icons), lit as the displays, the desktop and Deskbar come up; Olo's start-up sound plays once a session.

![Starting Olo: the start-up screen](2026-11-05/194742-starting-olo-the-start-up-screen.png)

### Upgrading Olo: the veil

*19:35* — While a restarted app_server brings the windows back, every display shows Olo's wordmark and a row of icons lit stage by stage, after BeOS's boot screen; it lifts once the windows are back.

![Upgrading Olo: the veil](2026-11-05/193521-upgrading-olo-the-veil.png)

### A window's recording with the camera

*19:17* — Recording a window on a background with Show Camera: the camera (here the stand-in's pattern) drawn on the background's corner, ringed as the on-screen bubble is.

![A window's recording with the camera](2026-11-05/191758-a-window-s-recording-with-the-camera.png)

### A shared window, ringed

*18:52* — While an app shares a window, it has a red ring round its body on screen (never in what's shared), flush outside its border at every scale.

![A shared window, ringed](2026-11-05/185204-a-shared-window-ringed.png)

### Private windows kept out of shares

*18:52* — Left: the screen as the person sees it (and a screenshot). Right: the same moment as a screen share or recording gets it, Passwords blacked out; the polkit prompt, Terminal's sudo prompt and the lock screen likewise.

![Private windows kept out of shares](2026-11-05/185203-private-windows-kept-out-of-shares.png)

### A Terminal window moved to an agent screen

*18:04* — The same shell (its pid) on the screen, typed into there.

![A Terminal window moved to an agent screen](2026-11-05/180415-a-terminal-window-moved-to-an-agent-screen.png)

### Terminal: a tab to an agent screen

*18:04* — Terminal's tab menu sends one tab to an agent screen: a Terminal there takes its shell, which keeps running in the person's olo-termd.

![Terminal: a tab to an agent screen](2026-11-05/180415-terminal-a-tab-to-an-agent-screen.png)

### Window menu: Move to Agent Screen

*18:04* — A window's tab menu puts the window on an agent screen (or a new one), the program running on: an Olo app re-executed there, any other program replayed there by olo-wlproxy.

![Window menu: Move to Agent Screen](2026-11-05/180415-window-menu-move-to-agent-screen.png)

### Agent Screens: Move Windows To…

*18:04* — Beside Take Control, every window on the screen goes to one of the person's display-workspaces, its program running on (Control ▸ Move Windows To ▸ too).

![Agent Screens: Move Windows To…](2026-11-05/180414-agent-screens-move-windows-to.png)

### Preferences ▸ Security: Face Unlock

*16:48* — A look at the infrared camera can stand for the password: sudo, administrator requests, Passwords and the lock screen. Olo reads the IR camera over V4L2, lights its emitter through its UVC control, finds and recognises faces with two small models in process, and keeps the faces with a root service no app can read. Install… sets it up as root through Olo's polkit agent; Set Up… takes the password and five looks.

![Preferences ▸ Security: Face Unlock](2026-11-05/164804-preferences-security-face-unlock.png)

### Viewer: text and formatted documents selected, copied and found

*16:22* — Texts and formatted documents (Markdown, Word, OpenDocument) select as anywhere in Olo: a drag, a word with a double-click, a line with a triple, drawn behind the text in the kit's selection. Copy and Select All are in the Edit menu, and Find uses the kit's find bar, the same as PDFs': every match marked, even in a code block, with the one shown selected. Here Olo's README, looking for 'olo'.

![Viewer: text and formatted documents selected, copied and found](2026-11-05/162256-viewer-text-and-formatted-documents-selected-cop.png)

### Viewer: a PDF's text selected, copied and found

*16:05* — A PDF's text is read natively (hayro's interpreter with a device of Olo's own that keeps each glyph's Unicode and box), once a page, on the worker. It's selected by dragging (a word with a double-click, a line with a triple), copied with ⌘C or a right-click, Select All copies the whole document, and Find (⌘F) marks every match, selecting the one shown in the middle of the view. Here the third of 90 'compression's in bzip2's manual, the next one marked.

![Viewer: a PDF's text selected, copied and found](2026-11-05/160514-viewer-a-pdf-s-text-selected-copied-and-found.png)

### Preferences ▸ Network ▸ VPN, with WireGuard imported natively

*15:42* — The VPNs saved (WireGuard, and NetworkManager's plugins' OpenVPN, IPsec…) are each connected and disconnected here, and removed after asking. A WireGuard file (wg-quick's .conf) is imported natively: keys checked, addresses by family, and its DNS asked alone while it's up so a full tunnel's names don't leak. Here “Office” was imported and connected. Also new: a hotspot under Wi-Fi, and IPv6 and MTU on a wired link (nm-fake standing in for NetworkManager).

![Preferences ▸ Network ▸ VPN, with WireGuard imported natively](2026-11-05/154256-preferences-network-vpn-with-wireguard-imported-.png)

### Preferences ▸ Network ▸ Shaping, as a link conditioner

*15:25* — A link made slower, later and lossier than it is, each way, to try a program on a poor network: bandwidth, delay, jitter and loss, from a network to start from (Edge, 3G, LTE, DSL, Busy Wi-Fi, Very Bad Network, Offline). The kernel's netem does it, spoken to over rtnetlink by olo-net-admin as root (an ifb device carries what's received). Here 3G was chosen and is ready to turn on.

![Preferences ▸ Network ▸ Shaping, as a link conditioner](2026-11-05/152552-preferences-network-shaping-as-a-link-conditione.png)

### Preferences ▸ Network: Service Order, with Deskbar following

*15:24* — Network has tabs for Ethernet, Service Order, DNS, Hosts and Shaping now. Here Wi-Fi was put first. NetworkManager's per-kind route metrics, written as root by olo-net-admin, moved the default route without disconnecting anything, and Deskbar's tray drew Ethernet faint and Wi-Fi strong as it happened (nm-fake standing in for NetworkManager).

![Preferences ▸ Network: Service Order, with Deskbar following](2026-11-05/152457-preferences-network-service-order-with-deskbar-f.png)

### Deskbar: Ethernet beside Wi-Fi, with each link's details

*15:01* — An Ethernet port shows in the tray while a wired link is connected. With both connected, the link not carrying the traffic (NetworkManager's default route) is drawn faint. A right-click on either shows what it's joined to, whether it's in use, its signal and rate or speed, addresses, router, DNS servers, interface, hardware address and traffic, then Network Preferences…. Here Wi-Fi stands by while Ethernet carries the traffic (nm-fake, a stand-in NetworkManager).

![Deskbar: Ethernet beside Wi-Fi, with each link's details](2026-11-05/150134-deskbar-ethernet-beside-wi-fi-with-each-link-s-d.png)

### Passwords is the SSH agent

*14:02* — Preferences ▸ Security ▸ Be the SSH agent (on by default): ssh and git sign in with the SSH keys in Passwords, each terminal allowed the first time; where ~/.ssh/config sends ssh to another agent (1Password's app's), it says so and offers the switch once the session has Passwords' agent.

![Passwords is the SSH agent](2026-11-05/140222-passwords-is-the-ssh-agent.png)

### Olo Doctor: temporary files on disk

*12:26* — Doctor finds programs keeping their temporary files in memory (/tmp a tmpfs, TMPDIR unset) and offers to keep them in /var/tmp: an environment.d file of Olo's and the running session's environment.

![Olo Doctor: temporary files on disk](2026-11-05/122649-olo-doctor-temporary-files-on-disk.png)

### HDR wallpapers: what shines (tinted red)

*12:09* — The gain maps wallpaper-hdr made, shown as a red tint where each picture goes above SDR's white in HDR: the sun's glow and LEDs, the moon, glints, the neon grid, speculars; the large blue and yellow surfaces and the skies stay at SDR's white.

![HDR wallpapers: what shines (tinted red)](2026-11-05/120922-hdr-wallpapers-what-shines-tinted-red.png)

### Backgrounds: the pack's pictures have HDR versions

*12:09* — On a display in HDR the chosen pack picture shows in HDR, its lights above everything else's white (a gain map beside each picture, made from it); Backgrounds says so under the pictures.

![Backgrounds: the pack's pictures have HDR versions](2026-11-05/120921-backgrounds-the-pack-s-pictures-have-hdr-version.png)

### Display: Everything else's brightness up to 600 cd/m², on its track

*10:20* — The SDR-in-HDR slider went to 480 while app_server took 600 (and a built-in panel's now follows its brightness, near 600 on the ThinkPad's OLED), so its knob was drawn past the track's end (Ben: it 'runs off the side'). One range for both, in protocol; the kit's Slider draws a value outside its range at the end; the note under it level with the label. At 2×, 1.5× and 1×.

![Display: Everything else's brightness up to 600 cd/m², on its track](2026-11-05/102001-display-everything-else-s-brightness-up-to-600-c.png)

### Display: keep HDR or a new resolution in an alert

*10:20* — Turning HDR on (or choosing a resolution) in Preferences ▸ Display now asks in an alert over the window, counting down, Revert or Keep (the default), as BeOS's Screen preferences did; the line at the foot of the window was easy to miss, and HDR went off by itself 15 s later (Ben, 2026-11-05). The kit's widgets::open_timed_alert.

![Display: keep HDR or a new resolution in an alert](2026-11-05/102000-display-keep-hdr-or-a-new-resolution-in-an-alert.png)

### Captions and chapters in recordings

*08:01* — An agent narrating a demo puts captions in the video (olo-screen caption-video: drawn into the recording only, never on the screen) and marks its steps (mark-video): the marks become the video's chapters, which the Viewer shows as ticks on its timeline and lists in Go, playing from the one chosen.

![Captions and chapters in recordings](2026-11-05/080131-captions-and-chapters-in-recordings.png)

### Recordings in HEVC, and a third smaller in H.264

*07:20* — Preferences ▸ GPUs ▸ Format: H.264 (plays everywhere) or HEVC (about half the size), on NVIDIA's encoder; olo-screenshot --codec takes AV1 too. Each codec now keeps a constant, measured quality instead of a bitrate: the same screen typed into for 5 s was 262 KB in H.264 and is 171 now, 96 in HEVC, the text as sharp. Trimming, GIFs and the Viewer take all three.

![Recordings in HEVC, and a third smaller in H.264](2026-11-05/072017-recordings-in-hevc-and-a-third-smaller-in-h-264.png)

### The camera in a bubble while recording

*06:57* — Record… ▸ Show Camera (or --camera): the camera in a circle low in the screen's right corner while you record, so a recording of the screen has you in it; it's there before the countdown, so you can frame yourself, and drags anywhere. The camera is read natively (V4L2, in process); here a stand-in camera's pattern.

![The camera in a bubble while recording](2026-11-05/065745-the-camera-in-a-bubble-while-recording.png)

### Recordings zoom in on clicks

*06:45* — Record… ▸ Zoom on Clicks (or --zoom-follow): a recording of the screen glides in around each click (twice), pans to keep the pointer in view, holds while you click and type, and glides out three seconds after; a shortcut's key cap stays in its place, unzoomed. Agents zoom to a detail themselves (olo-screen zoom-video NAME X,Y,W,H, then out). The GPU draws the capture magnified, so it costs the recorder nothing; screenshots and the screen are as they are.

![Recordings zoom in on clicks](2026-11-05/064521-recordings-zoom-in-on-clicks.png)

### The pointer in a recording, larger and smoothed

*06:30* — Record… ▸ Larger Pointer and Smooth Pointer (or --pointer-size, --smooth-pointer): recordings draw the pointer bigger (drawn at that size, never enlarged) and gliding after the real one, so a hand's jitter is smoothed and an agent's jumps become moves. Here, the frames after one jump laid over each other, a thirtieth of a second apart. Only recordings have it; a window's recording now shows the pointer too.

![The pointer in a recording, larger and smoothed](2026-11-05/063045-the-pointer-in-a-recording-larger-and-smoothed.png)

### A window recorded on a background

*06:17* — Record… ▸ Window on a Background (or --background): a window's recording shows it on the wallpaper, blurred and a little dimmed, its corners rounded and a soft shadow under it, as a polished demo of one app usually looks. Made once as the recording starts; each frame copies only the window.

![A window recorded on a background](2026-11-05/061718-a-window-recorded-on-a-background.png)

### The Viewer's Trim…

*05:57* — Edit ▸ Trim… on a video puts a handle at each end of its timeline (what's cut dimmed, the picture where a handle is dragged shown); Trim copies the stretch kept into “NAME (trimmed).mp4” beside it, nothing encoded again, and shows it. olo-screenshot --trim does the same for agents.

![The Viewer's Trim…](2026-11-05/055739-the-viewer-s-trim.png)

### Clicks and shortcuts shown while recording

*05:36* — Record… ▸ Show Clicks and Show Shortcuts (or --show-clicks, --show-keys): a ring spreads where a button is pressed, and a shortcut shows on a key cap low on the screen, written as the keyboard profile writes it; typing never shows. Drawn by app_server, so recordings, screen sharing and agents' demos have them.

![Clicks and shortcuts shown while recording](2026-11-05/053603-clicks-and-shortcuts-shown-while-recording.png)

### The lock screen: touch your security key

*04:33* — Asked as the screen locks, again three times, then whenever someone is there.

![The lock screen: touch your security key](2026-11-05/043302-the-lock-screen-touch-your-security-key.png)

### Terminal's sudo window: touch your security key

*04:33* — With a key set up and plugged in, the window says a touch will do; the password the key keeps goes to sudo's prompt.

![Terminal's sudo window: touch your security key](2026-11-05/043302-terminal-s-sudo-window-touch-your-security-key.png)

### Preferences ▸ Security: Security Keys

*04:33* — A security key (a YubiKey) set up so a touch of it stands for the account password: the password checked, then two touches; a spare the same way; and three touches in a row to type its code where you are. Shown set up with the test key.

![Preferences ▸ Security: Security Keys](2026-11-05/043301-preferences-security-security-keys.png)

### Cube, light

*03:52* — The Cube theme: workstation greys, a black tab, deep blue selection, its airbrushed icons and studio wallpaper.

![Cube, light](2026-11-05/035249-cube-light.png)

### Platinum, light

*03:52* — The Platinum theme: platinum grey, a lavender tab, lilac-blue selection, its outlined icons and wallpaper.

![Platinum, light](2026-11-05/035247-platinum-light.png)

### Workbench, dark: deep blue and orange

*03:52* — The Workbench theme's dark colours (Workbench 1.3's blue, orange tab and selection, black outlines), its pixel-art icons and wallpaper.

![Workbench, dark: deep blue and orange](2026-11-05/035246-workbench-dark-deep-blue-and-orange.png)

### Preferences ▸ Themes: Cube, Platinum and Workbench

*03:52* — Three more themes, after NeXTSTEP, Mac OS 9 and AmigaOS: a full icon set each (135, drawn by gpt-image-2.5-sunburst against each theme's references), its wordmark, its wallpaper (light and dark) and its own colours. The panel lays the chosen theme's colour sets beside the ten rows, so it fits 1280 × 800 without scrolling.

![Preferences ▸ Themes: Cube, Platinum and Workbench](2026-11-05/035245-preferences-themes-cube-platinum-and-workbench.png)

### HDR video shown for an sRGB screen

*01:26* — HDR10 and HLG video (phones', streaming's: BT.2020, PQ or HLG) converted for an ordinary screen: its light counted in SDR's white (203 cd/m²), BT.2020 converted to sRGB, highlights rolled off up to the brightest it was mastered to. A test pattern, a PQ ramp to 1,000 cd/m² over BT.2020 colour bars at 100 and 600 cd/m²: above as it shows now, below as before (dull and grey, its samples taken as sRGB).

![HDR video shown for an sRGB screen](2026-11-05/012656-hdr-video-shown-for-an-srgb-screen.png)

## 2026-11-04

### Window menu: a dialog floats always

*23:57* — An alert or a dialog that names its window floats over it whatever is asked: in auto-tiling its window menu shows Float Over the Tiles checked and dimmed, and app_server leaves it floating when asked to tile it (an alert tiled by hand had stayed over its app's floating windows as their child: a tile over floats, found by the auto fuzzer).

![Window menu: a dialog floats always](2026-11-04/235741-window-menu-a-dialog-floats-always.png)

### Preferences > GPUs: the power mode sets them all

*20:59* — The power mode is the computer's one setting for speed against battery: Power Saver puts every GPU on Cooler and Quieter, Balanced on Balanced, Performance on Fastest, as it sets the processors, wherever it's chosen (a root service, olo-gpu-follow, installed by the first Apply). Its box says so and the mode now; Apply and Revert end the tuning's section, and Screen Recordings starts a section down (the layout review's new after-commit check found it 15 pt under Apply). Dark, 2x.

![Preferences > GPUs: the power mode sets them all](2026-11-04/205957-preferences-gpus-the-power-mode-sets-them-all.png)

### Screenshot: Record…

*20:59* — Screenshot's window records too: Record… offers Record Screen, Record Active Window (the one behind it) and Record Region… (dragged out on the frozen screen). Its window steps aside, the recorder starts on its own and Screenshot quits; with a recording going, the menu is Pause (or Resume) and Stop Recording. The window is as tall as its form now: a fixed height had let Save in: slip under the buttons. Dark, 2x.

![Screenshot: Record…](2026-11-04/205955-screenshot-record.png)

### Deskbar: a paused recording

*20:17* — A recording paused (olo-screenshot --pause, or Pause Recording in this menu) shows a pause sign in its record light's ring: two bars on device pixels, as far apart as they are wide, centred at 1x, 1.5x and 2x. Top: paused, light, 2x. Below: paused and recording, dark, 2x. The menu offers Resume Recording for Pause Recording; the pause is cut out of the video.

![Deskbar: a paused recording](2026-11-04/201749-deskbar-a-paused-recording.png)

### Preferences ▸ GPUs: what screen recordings encode on

*18:55* — The most powerful there is by default (here the NVIDIA RTX PRO 2000's encoder, NVENC), that GPU, or the processor (OpenH264); the recorder reads it unless told.

![Preferences ▸ GPUs: what screen recordings encode on](2026-11-04/185514-preferences-gpus-what-screen-recordings-encode-o.png)

### Screen recording from a shortcut: Deskbar's record light, then the notification

*18:46* — Shift-Cmd-6 records the screen; Deskbar's tray shows a red record light with Stop Recording while it goes; finished, a notification says it was saved, with Show in Tracker.

![Screen recording from a shortcut: Deskbar's record light, then the notification](2026-11-04/184621-screen-recording-from-a-shortcut-deskbar-s-recor.png)

### A screen recording, played back in the Viewer

*18:29* — olo-screenshot --record made it (frames only as the screen changed, H.264 into an MP4 written as it goes); Agent Screens record theirs with olo-screen record-video, at 4K too.

![A screen recording, played back in the Viewer](2026-11-04/182948-a-screen-recording-played-back-in-the-viewer.png)

### Preferences ▸ Keyboard knows the Magic Keyboard with Touch ID

*18:15* — The A2520 recognised by its name, in a box of its own with a picture (the keyboard icon for now), what it is, and why its Touch ID isn't used: its sensor is sealed to the Mac it's paired with.

![Preferences ▸ Keyboard knows the Magic Keyboard with Touch ID](2026-11-04/181502-preferences-keyboard-knows-the-magic-keyboard-wi.png)

### Agent Screens: a stopped screen stays, with Start Screen

*17:56* — Stopping a screen keeps it, listed under Stopped (even one no agent acted on, which used to vanish), with Start Screen to start it again at its size; archived screens under Archived, shown by default. The same from olo-screen: list (with each screen's state), start NAME, archive, unarchive, delete.

![Agent Screens: a stopped screen stays, with Start Screen](2026-11-04/175618-agent-screens-a-stopped-screen-stays-with-start-.png)

### Banner actions stack when they don't fit side by side

*15:47* — Two long actions, one above another at the card's width, at 1×, 1.5× and 2×; a label cut with … only when even a row of its own is too narrow.

![Banner actions stack when they don't fit side by side](2026-11-04/154740-banner-actions-stack-when-they-don-t-fit-side-by.png)

### A notification for each new suggestion, once

*15:47* — Olo Suggestions tells of a suggestion once ever; at 1×, 1.5× and 2×. Its action used to be cut off past the banner's edge: banner actions now fit, stacked when they must.

![A notification for each new suggestion, once](2026-11-04/154739-a-notification-for-each-new-suggestion-once.png)

### Olo Suggestions: just, uv, mise and worktrunk

*15:47* — Programs worth having, suggested while they're not installed: just and uv from Fedora's repositories, mise and worktrunk from their makers' GitHub releases (SHA-256 checked, native); and worktrunk set to make worktrees where Terminal does.

![Olo Suggestions: just, uv, mise and worktrunk](2026-11-04/154737-olo-suggestions-just-uv-mise-and-worktrunk.png)

### Backup chooses what goes in

*13:51* — Preferences ▸ Backup: a check box for each part of a backup (each app's settings with the passwords and sign-ins that go with them, and the Wi-Fi networks), exporting and importing: a backup of everything can bring back the Token Pool alone. The rest stays as it is.

![Backup chooses what goes in](2026-11-04/135106-backup-chooses-what-goes-in.png)

### Olo Rescue

*13:31* — Ctrl+Shift+Super+Delete, from anywhere: every window put away, the displays, keyboard and pointer on safe settings, and a screen of its own with a terminal (its own, in process), the processes to end, Preferences' panels (what's changed applies as Rescue ends), the displays' settings reset, Start Over, Log Out, Restart and Leave Rescue.

![Olo Rescue](2026-11-04/133132-olo-rescue.png)

### Display: Colour, with HDR

*11:01* — Preferences ▸ Display has a Colour tab beside the panel's facts: what the display shows (its gamut and light, from its EDID), whether sRGB's colours are shown accurately (converted into a wide display's gamut, the default) or vivid, and HDR on a display that takes it, with everything else's brightness and a band that reverts HDR unless kept. On a headless screen pretending to be a P3 HDR panel.

![Display: Colour, with HDR](2026-11-04/110136-display-colour-with-hdr.png)

### Preferences ▸ GPUs

*04:00* — A new panel: each GPU, what it drives, and its tuning. Presets (Cooler and Quieter, Balanced, Fastest) or the knobs by hand: Intel's lowest and highest clock and power-saving profile; NVIDIA's highest clock, power limit where it can be given one (not on a laptop) and persistence. Applied through polkit, kept for the computer and set again at every start. A sleeping NVIDIA GPU isn't asked (that would wake it): what it said last is shown.

![Preferences ▸ GPUs](2026-11-04/040036-preferences-gpus.png)

### Fitting windows open at their size

*03:17* — A window that fits what it shows (here Preferences ▸ Display, with its displays and GPUs, which come from app_server just after it opens) is no longer shown at its designed size and then grown: frames drawn at a size about to change aren't shown, and what it asks for as it opens holds its first showing for up to 150 ms. Display opens at 940×607 at once (before: 520, then 607). Its Graphics box's labels now level with their values.

![Fitting windows open at their size](2026-11-04/031731-fitting-windows-open-at-their-size.png)

### Deskbar: Olo Doctor's bag in the tray

*00:09* — Shown while Olo Doctor's last look (at login and every 15 minutes) found something to fix; a click opens it.

![Deskbar: Olo Doctor's bag in the tray](2026-11-04/000934-deskbar-olo-doctor-s-bag-in-the-tray.png)

### Performance in two tabs, the thresholds two by two

*00:09* — It was 1,220 points tall in a scroll view of its own; now Supervisor and Watchdog tabs, fitting a 1280 × 800 screen unscrolled.

![Performance in two tabs, the thresholds two by two](2026-11-04/000933-performance-in-two-tabs-the-thresholds-two-by-tw.png)

### Power: the power mode

*00:09* — Performance, Balanced or Power Saver, through the power profiles daemon (tuned-ppd here): the processors' energy preference, the ThinkPad's platform profile. Also in Deskbar's battery menu; Performance has a button here.

![Power: the power mode](2026-11-04/000933-power-the-power-mode.png)

### Display: settings beside the panel, the window as tall as both

*00:08* — At 200 % on the ThinkPad's 3200 × 2000 panel, Display was a 658-point window scrolling an 804-point panel, its scroll bar against the boxes. Now the display's settings and its panel's facts are side by side, in a window that follows what it shows (fits_content), on every screen down to 1280 × 800.

![Display: settings beside the panel, the window as tall as both](2026-11-04/000848-display-settings-beside-the-panel-the-window-as-.png)

### Activity Monitor ▸ AI: how the command model does

*00:00* — A new section, asked of olo-complete once a second only while it shows: suggestions offered and taken (whole or a word at a time), what it knows, latency from the question to the answer sent (typically, 95th percentile, slowest) with a graph, and its learning: fine-tunes, the last one's examples, steps and time, how often its first suggestion is right on the newest commands against the hand-written weights, and whether it was kept. Off, it says so, with Terminal Preferences… to turn it on.

![Activity Monitor ▸ AI: how the command model does](2026-11-04/000036-activity-monitor-ai-how-the-command-model-does.png)

## 2026-11-03

### Terminal ▸ Command Suggestions: brush's hints from a model of your own

*23:59* — Preferences ▸ Terminal turns on olo-complete, a tiny model of your own commands (the line, the folder, the commands before in that shell, the hour and the day) that gives brush its grey hint. Turning it on starts it and runs its first fine-tune from the history, then says how it did on your newest commands; it learns again a minute after you type. Unavailable while brush isn't the login shell, and a brush from before the model is told apart by a mark in its binary, each with a button to Olo Suggestions. The pane is 520×692 for its tallest state.

![Terminal ▸ Command Suggestions: brush's hints from a model of your own](2026-11-03/235938-terminal-command-suggestions-brush-s-hints-from-.png)

### Terminal: a slim scroll bar, there when wanted

*22:46* — The kit's SlimScrollBar over the scrollback's right edge: shown when the pointer comes near or the view scrolls, wider under the pointer, fading a moment after; its thumb drags and its track pages.

![Terminal: a slim scroll bar, there when wanted](2026-11-03/224620-terminal-a-slim-scroll-bar-there-when-wanted.png)

### Command palette: every text key, and Ctrl for files

*22:20* — Text editing's keys are commands of their own (Move to the end of the line, ⌘→), done in the window the palette opened over; files and folders come with a tap of Ctrl, as the key hint at the field's right says.

![Command palette: every text key, and Ctrl for files](2026-11-03/222010-command-palette-every-text-key-and-ctrl-for-file.png)

### Terminal: the side strip lined up

*21:51* — Down the side, the tabs' icons, the groups' names and New Tab's + sit under the menu bar's first title, the first chip clears the menu bar as it does along the top, and the pane heads beside it are as tall as the top strip.

![Terminal: the side strip lined up](2026-11-03/215104-terminal-the-side-strip-lined-up.png)

### Terminal: Hide When Tiled

*21:35* — A tab's menu hides it from Tile All Tabs: it stays in the strip with its icon and title faded, and the tiles share the window without it.

![Terminal: Hide When Tiled](2026-11-03/213544-terminal-hide-when-tiled.png)

### Terminal: tile mode's group headers selected while their groups show

*21:26* — With every group tiled (the default), each group's header is drawn selected; a click takes that group out of the tiles and another brings it back.

![Terminal: tile mode's group headers selected while their groups show](2026-11-03/212639-terminal-tile-mode-s-group-headers-selected-whil.png)

### Sound ▸ Voice Processing ▸ Try It

*20:46* — A few seconds of your voice, recorded and put through each setting worth comparing: what each adds (delay, its time a 10 ms frame, what noise suppression and levelling did) and what ran it; played as the original or the chosen setting's sound, switching at the same place, with a live spectrum of both (the kit's new Spectrum).

![Sound ▸ Voice Processing ▸ Try It](2026-11-03/204614-sound-voice-processing-try-it.png)

### Train for Your Palm: one thing a step, and a banner when it's done

*20:00* — Each of the twelve exercises asks one thing, with a line under the title saying what to do now (rest, hold, point, lift) from the hands on the pad; a finished step gets a green banner in the place of the status line (the kit's new Banner), and the key caps sit level with their labels at every scale.

![Train for Your Palm: one thing a step, and a banner when it's done](2026-11-03/200009-train-for-your-palm-one-thing-a-step-and-a-banne.png)

### Backup takes passwords to another computer

*18:09* — Preferences ▸ Backup seals passwords and sign-ins into the backup with a password, and moves it in on another computer: paths under the new home, the Token Pool paused, Telegram signed in again; what an import left behind is listed.

![Backup takes passwords to another computer](2026-11-03/180937-backup-takes-passwords-to-another-computer.png)

### StyledEdit's tool bars

*10:35* — The kit's new ToolBar: StyledEdit's file, editing and format tools along the top (wrapping to two rows in a narrow window, a » menu for what still doesn't fit), insert and view tools down the side. Every tool is a menu command and shows its state (Bold and Center pushed for the selected title); the pickers show the font, size and colour where the selection starts. 25 new icons, in every theme.

![StyledEdit's tool bars](2026-11-03/103549-stylededit-s-tool-bars.png)

## 2026-11-02

### Train for Your Palm, in a dialog of its own

*23:13* — The palm trainer moved out of the Touch tab (where it ran off the panel) into a 560×440 dialog opened from a button. The film is a laptop's deck, keys and the pad between the palm rests, with the hands resting and lifting; beside it the pad live, its own shape, each touch drawn as the model calls it: here one hand's palm half over the left edge, which the size rule misses (the red ring) and the model catches. A sixth exercise gathers exactly that case.

![Train for Your Palm, in a dialog of its own](2026-11-02/231348-train-for-your-palm-in-a-dialog-of-its-own.png)

### Preferences ▸ Touchpad ▸ Touch: Train for Your Palm

*22:41* — A guided run through five exercises on the touchpad, each shown as an Olo-style animation of hands on a pad (left) and mirrored live by what the pad sees (right: a palm in orange, a finger in blue), with a word on how it's going and how many more touches it needs. Then Olo's hand-written palm model is refined to the person's hands off the UI thread, kept in the settings, and tried live.

![Preferences ▸ Touchpad ▸ Touch: Train for Your Palm](2026-11-02/224115-preferences-touchpad-touch-train-for-your-palm.png)

### Text fields scroll to the caret, and show cut text in a tip

*22:12* — A one-line field used to always show its start: typing past its end hid the caret and what was typed (top, before). Now it scrolls sideways while edited, keeping the caret in view (middle). Unfocused, it shows its start again, and the whole text is in a tip on hover (bottom). Never a password's.

![Text fields scroll to the caret, and show cut text in a tip](2026-11-02/221250-text-fields-scroll-to-the-caret-and-show-cut-tex.png)

### Preferences ▸ Terminal: tell coding agents to use Agent Screens

*22:02* — A third Coding Agents setting: Claude Code and Codex are told as they start to test and use desktop apps and web pages on Agent Screens of their own, and how, Olo's Chrome included, so they never start a Chrome on the person's screen. The panel is taller to fit it.

![Preferences ▸ Terminal: tell coding agents to use Agent Screens](2026-11-02/220212-preferences-terminal-tell-coding-agents-to-use-a.png)

### Agent Screens: Live at the screen's own frame rate

*21:34* — Live now shows the agent screen's own frames, shared with the person's compositor as dmabufs (a subsurface of the window), at up to 120 frames a second; before, it took up to eight pictures a second and resampled each on the CPU (30% of a core while the screen moved, 0.6% now). The frame line round the picture stays flush at every scale.

![Agent Screens: Live at the screen's own frame rate](2026-11-02/213423-agent-screens-live-at-the-screen-s-own-frame-rat.png)

### Telegram: changes kept until Telegram has them

*20:30* — Archiving, reading, muting and pinning go into an outbox on disk and leave it only once Telegram has them, sent again on the next start if need be. A line under the chat list shows them going; quitting waits up to 30 s for them (Quit Now quits at once; what's left is kept). The demo account, its changes slowed down to show it.

![Telegram: changes kept until Telegram has them](2026-11-02/203039-telegram-changes-kept-until-telegram-has-them.png)

### Preferences ▸ AI: Emoji

*20:02* — The emoji picker's AI suggestions get their own switch, model and effort (low by default: they're asked while you wait), as Smart Tiling has; the panel grows to fit them.

![Preferences ▸ AI: Emoji](2026-11-02/200209-preferences-ai-emoji.png)

### Emoji picker: five from AI for a description

*19:56* — ⌃⌘Space opens the emoji picker over everything: search as you type, a tab a group, skin tones kept; ⌘↩ asks the model chosen in Preferences ▸ AI for five emoji for what was typed, each with its reason. Return puts the selected one into the field that had the keyboard, in any app (app_server's InsertText). (A fake model answered here; tests never call a real one.)

![Emoji picker: five from AI for a description](2026-11-02/195651-emoji-picker-five-from-ai-for-a-description.png)

### Agent Screens: several chosen, archived, deleted

*18:53* — ⌘- and ⇧-clicks or ⌘A choose screens together; their menu archives, unarchives, stops or deletes them all (asked once); View ▸ Show Archived lists the archived in their own section; Delete moves histories to the Trash.

![Agent Screens: several chosen, archived, deleted](2026-11-02/185350-agent-screens-several-chosen-archived-deleted.png)

### Agent Screens: a screen renamed

*18:45* — Screen ▸ Rename… or a click on its title: the name the person gives shows in the list and the header, kept with its history (olo-screen rename NAME TITLE; list says it).

![Agent Screens: a screen renamed](2026-11-02/184527-agent-screens-a-screen-renamed.png)

### Quick Look centred over the file panel

*17:45* — The panel exports its window (xdg-foreign, Window::export_handle) and the Viewer opens the look over it (--over=); app_server keeps a dialog that sizes itself after opening centred over its parent until it's moved.

![Quick Look centred over the file panel](2026-11-02/174556-quick-look-centred-over-the-file-panel.png)

### Quick Look from a file panel (before)

*17:45* — Before: the look, a process of its own with no parent, opened in a cascade slot at the asking app's corner, half over the panel.

![Quick Look from a file panel (before)](2026-11-02/174555-quick-look-from-a-file-panel-before.png)

### Deskbar's menus over banners

*17:26* — Layer surfaces' menus are drawn and hit-tested over the whole Top layer, so Deskbar's menus are over any banner (shell::layer_menus).

![Deskbar's menus over banners](2026-11-02/172612-deskbar-s-menus-over-banners.png)

### Banners over Deskbar's menu (before)

*17:26* — Before: a notification banner, a Top layer surface that came after Deskbar, drew over Deskbar's menus and took their clicks.

![Banners over Deskbar's menu (before)](2026-11-02/172611-banners-over-deskbar-s-menu-before.png)

### Menus taller than the screen scroll

*17:06* — A kit menu longer than its display stops short of its edges and scrolls: an arrow at each end there's more past scrolls it while the pointer rests there; the wheel and the keys too. Here Default Apps' every app, at the top and scrolled to the end.

![Menus taller than the screen scroll](2026-11-02/170629-menus-taller-than-the-screen-scroll.png)

### Olo Suggestions: applied, and undone

*16:52* — What's applied is listed with a button that undoes it (here: Use bash Again).

![Olo Suggestions: applied, and undone](2026-11-02/165249-olo-suggestions-applied-and-undone.png)

### Olo Suggestions

*16:52* — A new app: what Olo suggests, none of it needed, each applied with a button or all with Apply All. The first: brush, the bash-compatible shell in Rust, as the login shell.

![Olo Suggestions](2026-11-02/165248-olo-suggestions.png)

### Telegram: chats chosen together

*16:37* — ⌘-click, ⇧-click or Edit ▸ Select All Chats chooses several; their menu archives, unarchives, marks read, mutes or deletes them all (asking first).

![Telegram: chats chosen together](2026-11-02/163702-telegram-chats-chosen-together.png)

### Telegram: Unknown Senders

*16:37* — People who aren't contacts and were never written to get a list of their own under the archive: no notification, no unread count, until written back to. Avatars are 40 pt, from 46.

![Telegram: Unknown Senders](2026-11-02/163701-telegram-unknown-senders.png)

### File panel: the look floats

*16:06* — Space in a file panel (here the portal's, as Slack asks) looks at the file in a floating window over the panel, never a tile.

![File panel: the look floats](2026-11-02/160646-file-panel-the-look-floats.png)

### Focus: an alert from a covered app shows

*16:06* — In Focus mode an alert that names no window (here StyledEdit's 'Save changes?' while Tracker is in front; Passwords' passkey question over Chrome) shows over the window in front, with the keyboard, instead of staying hidden with its app.

![Focus: an alert from a covered app shows](2026-11-02/160645-focus-an-alert-from-a-covered-app-shows.png)

### Deskbar: voice processing in the speaker's menu

*15:31* — Right-click the speaker: Clean Up Microphone Sound, and in a submenu what it's tuned for, how much background noise to take out, echo and level (through olo-voice --set, as an agent would).

![Deskbar: voice processing in the speaker's menu](2026-11-02/153140-deskbar-voice-processing-in-the-speaker-s-menu.png)

### Sound: Voice Processing

*15:31* — Every microphone through Olo's voice processing (echo cancellation, DeepFilterNet noise suppression, level), and what it is doing now, in a line. Here against a sandboxed sound server's made-up devices.

![Sound: Voice Processing](2026-11-02/153111-sound-voice-processing.png)

### File panels: Open With

*15:29* — An item's menu in the Open and Save panels: Quick Look, Open With ▸ (the kit's olo_ui::open_with, as Tracker's) and Show in Tracker.

![File panels: Open With](2026-11-02/152903-file-panels-open-with.png)

### Default Apps: what Open With offers

*15:29* — The Open With tab: a kind's apps in Open With ▸, in order, moved, removed and added; Olo's Choice lets Olo choose again.

![Default Apps: what Open With offers](2026-11-02/152903-default-apps-what-open-with-offers.png)

### Default Apps: your own kinds

*15:29* — Preferences ▸ Default Apps ▸ Your Own Kinds: a kind named by its extensions (Game saves: .sav .sv2) opens in the app chosen; Olo knows files by it everywhere (olo_xdg::mime::own_kinds).

![Default Apps: your own kinds](2026-11-02/152902-default-apps-your-own-kinds.png)

### Tracker: Open With ▸ Other…

*14:48* — Any app can open a file: Other… lists every app that opens files, found by typing part of its name, with 'Always open files like … with it'.

![Tracker: Open With ▸ Other…](2026-11-02/144805-tracker-open-with-other.png)

### Preferences: Default Apps

*14:06* — A new panel: each kind of file and link with the app it opens in, changeable from the apps that suit it; a choice is the kind's for Olo and for other programs (Tracker's Always Open With chooses too). Its icon, a document with Olo's application cube, from the reference set.

![Preferences: Default Apps](2026-11-02/140643-preferences-default-apps.png)

### Tracker: Open With and Always Open With

*13:51* — A file's menu offers a few apps that suit its kind (its default marked, Olo's own, then those that say they open it), and Always Open With makes one the kind's default for Olo and for other programs.

![Tracker: Open With and Always Open With](2026-11-02/135142-tracker-open-with-and-always-open-with.png)

### Terminal: text kept legible on any background

*13:42* — Before (top) and after: colours a program picked for a dark terminal (pale greys and blues, bright white) or bands it drew (diff lines, a composer's grey) left text nearly invisible in light mode. Now text keeps WCAG's 4.5:1 against its cell, moved toward black or white just as far as it takes, its hue kept; box drawing and blocks as drawn; faint text still faint.

![Terminal: text kept legible on any background](2026-11-02/134227-terminal-text-kept-legible-on-any-background.png)

### A window tab's cut title shows whole

*13:33* — app_server shows the whole title of a window whose tab cuts it short on a tooltip's plate under the tab, once the pointer rests on the title (at once with ⌘), as Olo's apps do for their cut text.

![A window tab's cut title shows whole](2026-11-02/133301-a-window-tab-s-cut-title-shows-whole.png)

### Text cut short shows whole in a tip

*13:23* — Resting the pointer on text the kit cut with … (here a name in Tracker's column) shows all of it in the kit's tip; with ⌘ held, at once. Every kit window does it for every label it cuts or that runs past its clip.

![Text cut short shows whole in a tip](2026-11-02/132341-text-cut-short-shows-whole-in-a-tip.png)

### Command palette: Go to the path in the clipboard

*12:45* — With a path copied (absolute, ~/…, a file:// URI), the palette offers to go there before anything is typed, and a path typed whole is offered too: Tracker shows it, a folder in its window, a file selected in its folder's. Read off the UI thread once the palette has the keyboard, which app_server now gives an exclusive layer as it shows.

![Command palette: Go to the path in the clipboard](2026-11-02/124554-command-palette-go-to-the-path-in-the-clipboard.png)

### A setting found is shown, lit

*12:24* — Chosen in the palette, Preferences opens the setting's panel (on its tab, clicked if need be) with the control ringed in the keyboard-navigation colour for a moment: olo_ui::spotlight, which any app or agent can use to say 'here'. Two lines wide on device pixels; frames only while it comes and goes.

![A setting found is shown, lit](2026-11-02/122420-a-setting-found-is-shown-lit.png)

### Command palette: every setting in Preferences

*12:24* — Each control in every Preferences panel is a palette result, named with its panel, tab and box, and found by the words people use for it elsewhere ('night shift' finds Night Mode). The index is made from the panels as they're drawn, so a new setting is found without listing it.

![Command palette: every setting in Preferences](2026-11-02/122407-command-palette-every-setting-in-preferences.png)

### Deskbar: the speaker's menu

*12:06* — Right-click the speaker: every output and input, mute, and while AirPods play, their levels, noise control, Conversation Awareness, Personalized Volume, music or call quality, and both settings panels.

![Deskbar: the speaker's menu](2026-11-02/120605-deskbar-the-speaker-s-menu.png)

### Tracker: Go to File or Folder

*12:02* — Go ▸ Go to File or Folder… (⇧⌘G): a path copied is in the field already, selected, so Return goes; the names in its folder that what's typed could be are listed as it's typed, Tab completes, ↑↓ choose. A folder opens; a file shows selected in its folder's window.

![Tracker: Go to File or Folder](2026-11-02/120214-tracker-go-to-file-or-folder.png)

### Preferences on a short screen

*11:33* — A panel taller than the screen (AirPods, 752 points, on a 700-point display) opens as tall as fits, laid out at its own size and scrolled, with a scroll bar the window is widened for. Panels that fit are unchanged.

![Preferences on a short screen](2026-11-02/113343-preferences-on-a-short-screen.png)

### AirPods icons in every theme

*11:28* — airpods, airpods-case, airpod-left and airpod-right, generated against each theme's references (Classic, BeBox, Soft Be, Toybox, Papercraft, Enamel, Halftone), beside bluetooth and sound.

![AirPods icons in every theme](2026-11-02/112845-airpods-icons-in-every-theme.png)

### Deskbar: AirPods

*11:28* — Deskbar's tray shows AirPods while they're connected (amber or red when a bud runs low); its menu has their levels, noise control, Conversation Awareness and their settings.

![Deskbar: AirPods](2026-11-02/112844-deskbar-airpods.png)

### Sound: AirPods

*11:28* — Preferences ▸ Sound with AirPods as the output: their model, codec and whether they play at music or call quality, noise control, and where to record from while they're connected.

![Sound: AirPods](2026-11-02/112844-sound-airpods.png)

### AirPods panel, dark, 1×

*11:28* — The same at 1× in dark mode.

![AirPods panel, dark, 1×](2026-11-02/112843-airpods-panel-dark-1.png)

### AirPods panel

*11:27* — Preferences ▸ AirPods: levels with a picture of each bud and the case, noise control with Adaptive's strength, press and hold, listening features, the microphone, and what this computer does with them. Shown with a pretend pair (OLO_FAKE_AIRPODS).

![AirPods panel](2026-11-02/112750-airpods-panel.png)

### A video's picture in picture, Standard mode

*10:29* — Chrome's video picture in picture (no app id, titled "Picture in picture") in the corner, 16 px in, over the call page's window.

![A video's picture in picture, Standard mode](2026-11-02/102940-a-video-s-picture-in-picture-standard-mode.png)

### Picture in picture stays in its corner

*10:29* — A call popped out of Chrome (documentPictureInPicture, as Meet does) opens in the bottom right corner at its own size, over every window and on each workspace, out of Chrome's group, Focus's zoom and the window memory; the keyboard stays where it was. A link opened from another app (Terminal's Open Pull Request) now shows in Chrome, in front, with the call over it: before, the call joined Chrome's group and zoomed over the tab the link had opened in.

![Picture in picture stays in its corner](2026-11-02/102939-picture-in-picture-stays-in-its-corner.png)

### Terminal's Tile All Tabs heads

*01:07* — In Tile All Tabs each pane's head is on the strip's quiet band, with its group's chip at the right instead of a tint and a coloured line. The pane you type in has its title in full ink and medium weight, like the strip's selected tab. Icon, title and chip are centred on the band to the device pixel at every scale.

![Terminal's Tile All Tabs heads](2026-11-02/010722-terminal-s-tile-all-tabs-heads.png)

### Terminal's tabs in light mode

*01:07* — The same strip in light mode: a white plate on the quiet band, each group's capsule at 16% of its colour (22% in the dark).

![Terminal's tabs in light mode](2026-11-02/010720-terminal-s-tabs-in-light-mode.png)

### Terminal's tabs, redesigned

*01:07* — Terminal on GPUI with a new tab strip (olo-ui's TabStrip): everything sits inside the band. Tabs are their titles; the selected one is on a raised plate lit from above (a touch lighter at the top, its rim's top edge lit and tapering into the corners, a soft shadow), as BeOS lit its controls. A group is a capsule of its colour around its chip and tabs, with corners concentric with the plates inside. No lines, and nothing rises above the strip. Each tab is as wide as its title needs.

![Terminal's tabs, redesigned](2026-11-02/010713-terminal-s-tabs-redesigned.png)

## 2026-11-01

### Preferences on GPUI

*23:21* — Every panel of Preferences moved from egui to Olo's GPUI, drawn as before; Themes shows each theme's wordmark and icons, and its colour sets now share their row evenly.

![Preferences on GPUI](2026-11-01/232156-preferences-on-gpui.png)

### Pinch to zoom, off by default

*22:30* — Preferences ▸ Touchpad & Mouse ▸ Gestures: a pinch reaches the window under the pointer only when this is on. Off, two fingers drifting together while scrolling no longer zoom Chrome's pages.

![Pinch to zoom, off by default](2026-11-01/223036-pinch-to-zoom-off-by-default.png)

### Tracker on GPUI

*21:29* — Tracker moved from the egui kit to Olo's GPUI: folder windows (icons, mini icons, list, columns), the desktop, Get Info, Find, Tracker Status and Quick Look, with BeOS's drag rules through GPUI's own drops and Paste read off the UI thread.

![Tracker on GPUI](2026-11-01/212921-tracker-on-gpui.png)

### StyledEdit on GPUI

*20:14* — StyledEdit moves to the GPUI kit: its editor, ruler and spelling are olo-ui's now. Page View with a header, a table and a picture at 2x. Scrolls, types and resizes within a 120 Hz frame at every scale on a full 16-inch screen, as evenly as egui did; ends stretch and spring back.

![StyledEdit on GPUI](2026-11-01/201404-stylededit-on-gpui.png)

### Screen saver on GPUI

*17:14* — olo-screensaver draws on Olo's GPUI: Message, Icons, Lissart, Lens and Spots (rows) at 1, 1.25, 1.5, 1.75 and 2× (columns), over a test desktop. Lens magnifies the desktop through textured triangles, Lissart's hundreds of lines are painted a layer at a time.

![Screen saver on GPUI](2026-11-01/171459-screen-saver-on-gpui.png)

### Mice by make and model

*15:39* — Preferences ▸ Touchpad & Mouse ▸ Mouse names each mouse by maker and model (sensor, resolution, how it's connected), offers the profile recommended for it or settings of its own, says what is known about it, and points at Olo Doctor for what gets in its way (a key remapper grabbing it, a receiver allowed to sleep). The device list now comes by event, not every 3 s.

![Mice by make and model](2026-11-01/153934-mice-by-make-and-model.png)

### Olo Doctor

*15:17* — A new app that looks the computer over for what gets in Olo's way (an input remapper grabbing the mice, USB receivers allowed to sleep, Olo's udev rules missing, rtkit down, another desktop's daemons, programs on an old version) and fixes each with a button, or all at once with one password.

![Olo Doctor](2026-11-01/151718-olo-doctor.png)

### Activity Monitor ▸ Displays

*12:06* — Each display's frames a second, the ones dropped (a vblank or more late), what its frames cost app_server to draw and how app_server paces them (frames.rs: the draw's share of the frame set from its cost and from drops), whether the pointer is on a plane of its own; and the check box that has every drop logged with what went into it. Made-up displays here (OLO_ACTIVITY_DEMO_FRAMES); a headless session has none.

![Activity Monitor ▸ Displays](2026-11-01/120636-activity-monitor-displays.png)

### Passwords on GPUI

*10:49* — Passwords moves to the GPUI kit: its panes are cached parts of the window, so a one-time code counting down redraws the item pane alone, once a second; half the CPU of the egui version in every interaction and 90 MB against 124. Synthetic items from the test 1Password.

![Passwords on GPUI](2026-11-01/104952-passwords-on-gpui.png)

### Activity Monitor on GPUI

*09:50* — Activity Monitor moves from egui to Olo's GPUI kit, looking as it did. Its window is cached parts with the section's graphs on a layer of their own (the kit's GraphLayer): a graph moving a pixel redraws the graphs, not the cards around them. Graphs' fills are a new GPU primitive (paint_area) instead of paths filled through window-sized multisampled textures, and a window's graphs move together on one clock. On release at 2×: the UI thread at or under egui's in every section with graphs (CPU 3.9% of a core against 5.2), 93–96 MB against 119–123.

![Activity Monitor on GPUI](2026-11-01/095047-activity-monitor-on-gpui.png)

### Agent Screens on GPUI

*07:37* — Agent Screens moved to Olo's GPUI: the sidebar, header and Live/History/Stats tabs as before, now with the kit's GPUI Sidebar, TabView and dashboard cards. Live takes a picture only when the screen draws something new (app_server's new Painted event), each resampled to the exact device size on a worker; History plays back drawing only what changes.

![Agent Screens on GPUI](2026-11-01/073708-agent-screens-on-gpui.png)

### The share picker on GPUI

*06:15* — olo-portal's dialogs move to the GPUI kit: the share picker (live thumbnails in the kit's new ThumbnailPicker), permission, wallpaper, shortcuts and Open With questions, and the file and Print panels, each over the asking app's window by xdg-foreign. Its thumbnails no longer pile up in the GPU's atlas; a permission dialog opens in ~8 ms.

![The share picker on GPUI](2026-11-01/061527-the-share-picker-on-gpui.png)

### Notifications on GPUI

*05:22* — Notifications, banners and History, now on Olo's GPUI: the same cards, groups, stop signs, balloons and reply fields, with words kept on a row when they fit (spaces hang past the edge, as egui and typesetters wrap), the generic application icon for apps without one in banners and History alike, and an exact gap after an error's stop sign. 61 MB against egui's 153 with four banners; History scrolls 200 notes at 1.3–1.8 ms a frame.

![Notifications on GPUI](2026-11-01/052233-notifications-on-gpui.png)

### Viewer on GPUI

*03:57* — The Viewer moved to the GPUI kit: pictures fitted and resampled exactly to the device size on a worker, zoomed in tiles; the crop frame drawn over the picture, its thirds while it moves and its size under it; PDF forms, text boxes, ink and signatures; formatted documents, sheets, code, archives, fonts, sound and video from olo-preview's GPUI views. Windows open sized for the display they open on.

![Viewer on GPUI](2026-11-01/035745-viewer-on-gpui.png)

### Menus without stray separators

*01:41* — A menu built with items it shows only sometimes (Telegram's Chat menu with no chat open) ended in a separator with nothing under it; the GPUI kit now leaves out separators at either end of a menu and two together.

![Menus without stray separators](2026-11-01/014121-menus-without-stray-separators.png)

### Telegram on GPUI

*01:29* — Telegram moves to the GPUI kit: the same list, conversation and composer (a multi-line field: Return sends, Shift-Return a new line), its panes cached views of their own so a hover, a scroll or the caret redraws only its pane. Shown: the demo account's forum with topics, a reply, code and a photo, at 2x.

![Telegram on GPUI](2026-11-01/012944-telegram-on-gpui.png)

### Deskbar on GPUI

*00:00* — Deskbar moves to Olo's GPUI kit: the same tile, tray and workspaces, drawn on device pixels at every scale (the tray's glyphs as exact paths and GPU arcs), its menus' keys laid out as the egui kit's key combos, tips in popups of their own, applications dragged out as links with their icon and name. 57 MB against 141, and 32 frames for 30 pointer moves against 150.

![Deskbar on GPUI](2026-11-01/000024-deskbar-on-gpui.png)

## 2026-10-31

### Google Chrome's installer on GPUI

*23:18* — The offer to download Google Chrome (shown when it isn't installed) moves to the GPUI kit, now with its installer icon, which the egui window never found. 54 MB against 77.

![Google Chrome's installer on GPUI](2026-10-31/231841-google-chrome-s-installer-on-gpui.png)

### Log Viewer on GPUI

*23:12* — Log Viewer moves to the GPUI kit, with the kit's new search field (filters coloured, suggested as they're typed), filter bar and text view; the Message column now fits beside the scroll bar (no sideways scrolling). Loading 160,000 lines its slowest frame is 3 ms (egui's 34). A made-up log file, not a real journal.

![Log Viewer on GPUI](2026-10-31/231216-log-viewer-on-gpui.png)

### The command palette on GPUI

*22:46* — The command palette, its window menu and the shortcuts sheet move to the GPUI kit. Same look; key symbols (⇧ ⌘ ↵) set on the capitals' line by their outlines, the subtitle and kind on the title's baseline, the query lined up with the titles. It opens in 28 ms (egui: 47) and holds 40 MB less; small text's darkening is now an emboldened outline rather than a stroke (the same look, a third of the time).

![The command palette on GPUI](2026-10-31/224642-the-command-palette-on-gpui.png)

### DriveSetup on GPUI

*21:41* — DriveSetup moves to the GPUI kit: the disk map (segments to scale, hatched free space, the unlocked contents under a purple band) over the list of disks and volumes, the status line with progress and Stop, and its forms (Format, Create with a size slider, Resize, Rename, Unlock, New Partition Table) as alert windows that grow and shrink with their fields. Here on the test UDisks (udisks-fake). The kit gains BeOS keyboard navigation for it: Tab moves through fields, lists and controls (the focused control's label underlined in blue, Space presses it), and a default button answers Return from anywhere in its window.

![DriveSetup on GPUI](2026-10-31/214120-drivesetup-on-gpui.png)

### Screenshot on GPUI

*21:17* — Screenshot moves to the GPUI kit: the same form and buttons, the preview made at exactly its frame's size in device pixels (remade when the scale changes) and drawn one pixel to a pixel, Copy to clipboard as image/png through GPUI's clipboard (which now offers images and data, not only text), Save closing the window and holding the path on the clipboard until something else is copied. The ⌘⇧4 region picker and the colour picker are GPUI layer overlays showing each output's frozen picture 1:1.

![Screenshot on GPUI](2026-10-31/211747-screenshot-on-gpui.png)

### DiskUsage on GPUI

*20:57* — DiskUsage moves to the GPUI kit (olo-ui): the ring chart's slices are GPU sectors (paint_sector) with hairlines over them, the list beside it a ColumnListView whose rows drag into the Collector, Volumes, menus with the keyboard profile's keys, alerts and the folder panel all as before. The chart redraws only when the slice under the pointer changes, in about a millisecond; the list's name column now fits inside its scroll bar (egui's cut Files off), and the Collector's chips show each file's kind.

![DiskUsage on GPUI](2026-10-31/205741-diskusage-on-gpui.png)

### Token Pool on GPUI

*20:16* — Token Pool moves to olo-ui: the same window (gateway, now-grid, client switches, account cards with quota bars and request strips, Account menu, rename), drawn by GPUI. A test account in a fixture home; 73.6 MB against egui's 91.9 MB, idle with no wakeups.

![Token Pool on GPUI](2026-10-31/201636-token-pool-on-gpui.png)

### GPUI kit: the column list view

*19:39* — olo-ui gains BColumnListView (right), as the egui kit draws it (left, the demo's Lists tab): header sorting and dividers, the file-manager selection (shared now as olo_kit_base::list), bands, type-select, BeOS scroll bars; at 1.5x, where GPUI windows had been drawn at 2x and shrunk (fixed here).

![GPUI kit: the column list view](2026-10-31/193916-gpui-kit-the-column-list-view.png)

### Preferences ▸ Display: each display's panel

*17:19* — What the display is (make, size, density, refresh), what Olo works out about its panel (technology, colour stripes) and how it draws text on it, with why, each correctable (Automatic or a choice). Shown on a simulated 24-inch 1080p LCD (92 ppi): subpixel text.

![Preferences ▸ Display: each display's panel](2026-10-31/171914-preferences-display-each-display-s-panel.png)

### Telegram: polls and reactions on their baselines

*15:19* — A poll's shares stand on their answers' baseline; a reaction's count is centred in its chip by its figures with the emoji on their middle, and the emoji lines up with the message text above. Voice notes' waveform bars and the file glyph sit on device pixels at every scale. The layout review now judges an emoji inside a line of text with that line, not against the whole bubble.

![Telegram: polls and reactions on their baselines](2026-10-31/151915-telegram-polls-and-reactions-on-their-baselines.png)

## 2026-10-30

### StyledEdit: Page View, dark

*21:44* — The desk half way to black under dark sheets.

![StyledEdit: Page View, dark](2026-10-30/214407-stylededit-page-view-dark.png)

### StyledEdit: Page View

*21:44* — Document ▸ Page View sets the text on sheets of the paper it prints on, with its margins: paragraphs and rows break across pages as printed, headers and footers on each sheet; the kit editor's Options::pages.

![StyledEdit: Page View](2026-10-30/214406-stylededit-page-view.png)

### StyledEdit: pictures

*21:34* — Pictures in the text: Insert Picture…, pasted or dropped; drawn by the kit's editor where their character is, sized from the Edit menu, kept beside the document, and in Word, OpenDocument, RTF, web pages, Markdown and print.

![StyledEdit: pictures](2026-10-30/213451-stylededit-pictures.png)

### StyledEdit: Header & Footer

*20:42* — Document ▸ Header & Footer… sets a left, centre and right part above and below every printed page, {page} and {pages} standing for the page number and count. Kept in the file, and in Word, OpenDocument and RTF (checked in LibreOffice).

![StyledEdit: Header & Footer](2026-10-30/204225-stylededit-header-footer.png)

### StyledEdit: the ruler, indents and tab stops

*19:37* — A ruler over the text (the kit's ruler; Document ▸ Show Ruler): the caret's paragraph's first-line, left and right indents to drag, and tab stops: a click sets one, a drag moves it or takes it off, and its menu (or a double click) makes it left, centre, right or decimal. Tabs now go to stops (epaint's TabStops); here a left stop at 1½ in and a decimal one at 2¼ in line up a price list. Indents and stops are kept in Word, OpenDocument and RTF.

![StyledEdit: the ruler, indents and tab stops](2026-10-30/193751-stylededit-the-ruler-indents-and-tab-stops.png)

### StyledEdit: dragging a table's column rule

*19:15* — A table's inner rule can be dragged: a line shows where it will go, and let go, every row of the table takes the new widths (one step to undo). The widths are kept in StyledEdit's own files and in Word, OpenDocument, RTF and HTML.

![StyledEdit: dragging a table's column rule](2026-10-30/191513-stylededit-dragging-a-table-s-column-rule.png)

### File panels: Kind column, sorting, Favorites and Recent Folders

*19:15* — Every Open and Save panel lists Name, Kind, Size and Modified, sorted by any of them (folders first, names in natural order); the folder pop-up has the folders above this one, the standard places, Favorites (add or remove this folder) and Recent Folders, kept in ~/.config/olo/places.json and shared with Tracker's Go menu. Space looks at the selected file in the Viewer, as Quick Look.

![File panels: Kind column, sorting, Favorites and Recent Folders](2026-10-30/191512-file-panels-kind-column-sorting-favorites-and-re.png)

### StyledEdit: tables

*18:51* — A paragraph can be a table's row: its cells its text between tabs, laid out side by side in columns by the kit editor and ruled a device pixel wide (a header row shaded). Tab and ⇧Tab go from cell to cell, Tab in the last cell and Return add a row, and edits keep a table's cells and rows. The Table menu inserts and converts tables, adds and deletes rows and columns, and toggles the header; Word, OpenDocument, RTF and Markdown tables come in as tables and go out as them.

![StyledEdit: tables](2026-10-30/185129-stylededit-tables.png)

### Printing: the Print panel and PDF

*17:37* — StyledEdit's File > Print... (and Export > PDF): a printer from CUPS (spoken to natively over IPP) or a PDF, paper, copies and pages; pages laid out as the editor lays them out and made into a PDF in process (krilla), fonts embedded, text selectable.

![Printing: the Print panel and PDF](2026-10-30/173704-printing-the-print-panel-and-pdf.png)

### StyledEdit: spelling as you type

*17:24* — Misspelled words get a red wave under them (the kit editor's Options::spelling, Hunspell dictionaries read natively by spellbook); right-click one for suggestions, Ignore Spelling or Learn Spelling.

![StyledEdit: spelling as you type](2026-10-30/172433-stylededit-spelling-as-you-type.png)

### StyledEdit: paragraph styles and lists

*17:02* — Paragraphs have a style (Title, Heading 1-3, Quote, Code, Body), bulleted and numbered lists that nest and number themselves, indents and their own alignment; the kit's editor sets each paragraph's indents, spacing and list marker (Document::paragraph). Word count in the status line.

![StyledEdit: paragraph styles and lists](2026-10-30/170247-stylededit-paragraph-styles-and-lists.png)

### Quick Look full screen on two displays: after

*15:33* — F fills only the display the window is on; the other is untouched, and F again puts it back where it was. Same at 1.5× and 2×.

![Quick Look full screen on two displays: after](2026-10-30/153348-quick-look-full-screen-on-two-displays-after.png)

### Quick Look full screen on two displays: before

*15:33* — Quick Look on the right display, F: it filled the left display (the first one), and on the right it hung off its old place at full-screen size.

![Quick Look full screen on two displays: before](2026-10-30/153348-quick-look-full-screen-on-two-displays-before.png)

### Preferences ▸ Terminal: Let coding agents keep their tab's title current

*15:27* — Off by default; the panel is taller for it.

![Preferences ▸ Terminal: Let coding agents keep their tab's title current](2026-10-30/152752-preferences-terminal-let-coding-agents-keep-thei.png)

### Terminal: a coding agent's title for its tab

*15:27* — With Preferences ▸ Terminal's new setting, Claude Code and Codex get a set_tab_title tool (an MCP server of Terminal's) and are asked to keep their tab titled with what they're working on; here the tool was called from the tab.

![Terminal: a coding agent's title for its tab](2026-10-30/152751-terminal-a-coding-agent-s-title-for-its-tab.png)

### Deskbar: the supervisor's gauge at 1×, 1.25×, 1.5×, 1.75×, 2×

*15:08* — On (dial tinted, needle up, the findings counted, red if urgent) and off (dim, needle resting). A click turns it on or off; its menu lists the findings, Show in Activity Monitor and Performance Settings…

![Deskbar: the supervisor's gauge at 1×, 1.25×, 1.5×, 1.75×, 2×](2026-10-30/150853-deskbar-the-supervisor-s-gauge-at-1-1-25-1-5-1-7.png)

### Inspector: the Supervisor tab

*15:08* — A process's inspector has a Supervisor tab: its findings, how sure and why, and whether it was lowered and why. The alert's Show in Activity Monitor opens it there.

![Inspector: the Supervisor tab](2026-10-30/150853-inspector-the-supervisor-tab.png)

### Supervisor alert, sized for its buttons

*15:08* — An urgent finding asks in an alert naming the process, what was found and the evidence. The kit's WindowOptions::alert_buttons widens an alert to fit long button labels.

![Supervisor alert, sized for its buttons](2026-10-30/150852-supervisor-alert-sized-for-its-buttons.png)

### Performance supervisor, dark

*15:08* — The same in the dark look: the findings' confidence in the failure colour.

![Performance supervisor, dark](2026-10-30/150852-performance-supervisor-dark.png)

### Performance supervisor: Activity Monitor's Supervisor section and Preferences ▸ Performance

*15:08* — The supervisor (off by default) lowers builds and background work and watches for stuck processes, leaks, memory hogs and zombies, each judged from several signals with a confidence and evidence. Activity Monitor shows it as cards (Inspect, Stop Process, Ignore This Process); every threshold is tuned in Preferences ▸ Performance.

![Performance supervisor: Activity Monitor's Supervisor section and Preferences ▸ Performance](2026-10-30/150851-performance-supervisor-activity-monitor-s-superv.png)

### Terminal: Tile All Tabs rows' heights dragged

*14:03* — In Tile All Tabs, the line between two groups' rows (and the heads under it) drags their heights; the shares go by group and are kept with the window. The right button there gives the group's menu with Reset Row Heights.

![Terminal: Tile All Tabs rows' heights dragged](2026-10-30/140355-terminal-tile-all-tabs-rows-heights-dragged.png)

### Agent Screens: Stats

*13:51* — Stats as dashboard cards (dark, 1.5×): use, actions by kind, actions a minute over the last hour, programs run.

![Agent Screens: Stats](2026-10-30/135147-agent-screens-stats.png)

### Agent Screens: filters

*13:51* — History's filters, as Log Viewer's: Result ▸ Failed keeps the 12 of 249 actions that failed, each with why in red.

![Agent Screens: filters](2026-10-30/135147-agent-screens-filters.png)

### Agent Screens: History follows the agent live

*13:51* — Agent Screens revamped: a sidebar of running and stopped screens, a header saying how the chosen one is, and Live, History and Stats tabs. History now follows the history as it's written (each new action chosen and shown as it comes), its scroll bar works (the list was pinned to the playhead's row every frame), and a search with Kind, Result and Time filters narrows it; under the picture, what the action did.

![Agent Screens: History follows the agent live](2026-10-30/135146-agent-screens-history-follows-the-agent-live.png)

### Notifications: only the latest of the same

*13:16* — Preferences ▸ Notifications ▸ History: Only the latest of the same notification (on by default): the same again from the same app takes the earlier one's place while it's still showing or unread.

![Notifications: only the latest of the same](2026-10-30/131612-notifications-only-the-latest-of-the-same.png)

### Preferences ▸ Terminal: Coding Agents

*13:11* — Tell coding agents about the repositories in their tab group (off by default): Claude Code and Codex in a tab of a group spanning repositories are told the group's name and its repositories and worktrees, through their own hooks.

![Preferences ▸ Terminal: Coding Agents](2026-10-30/131141-preferences-terminal-coding-agents.png)

### Terminal: Tile All Tabs, a row per group

*12:57* — Tile All Tabs (was Show All Tabs) gives each group a row, its tabs side by side, and the ungrouped tabs a row of their own; clicking groups' chips chooses which tile (lit as Deskbar's workspace showing): here alpha and beta of three, a row each; one alone fills a grid; the right button gives the group's menu.

![Terminal: Tile All Tabs, a row per group](2026-10-30/125730-terminal-tile-all-tabs-a-row-per-group.png)

### Deskbar: every display's workspaces, and the display's number

*12:51* — Each Deskbar lists every display's workspaces and their apps, its own display's first (here D1's Deskbar, then D0's), and its tray shows its display's number in a small monitor; clicking it lists the displays and goes to the one chosen.

![Deskbar: every display's workspaces, and the display's number](2026-10-30/125139-deskbar-every-display-s-workspaces-and-the-displ.png)

### Deskbar: an app's windows to any workspace

*12:24* — An app's Deskbar menu moves its windows as a tab's menu moves one: Move to Workspace ▸ every display's workspaces (the one they're on checked), and To the Next / Previous Display.

![Deskbar: an app's windows to any workspace](2026-10-30/122417-deskbar-an-app-s-windows-to-any-workspace.png)

### Window menu: every display's workspaces

*12:24* — A window's tab menu ▸ Move to Workspace lists every display's workspaces, left to right as Deskbar names them, its own display's with their keys and a check; another display's moves it there, to the same place, tiled or zoomed as it was.

![Window menu: every display's workspaces](2026-10-30/122416-window-menu-every-display-s-workspaces.png)

### Token Pool: the Clients window

*12:09* — Pool ▸ Clients: the pool's activity over time (requests a minute by client, over the last hour, six hours or day, with totals), Codex's service with its version, settings, connections, conversations and Restart, and each Claude Code and Codex running and where its connections go (here a sandbox with a made-up day of activity).

![Token Pool: the Clients window](2026-10-30/120903-token-pool-the-clients-window.png)

### New windows glow a moment (Preferences ▸ Appearance)

*11:06* — With Glow new windows for N seconds on (off by default, 5 s), a new window has a soft halo in the look's focus blue round its frame and tab as one shape, fading out over its last second; left: just opened, right: after (at 1×, enlarged 2×).

![New windows glow a moment (Preferences ▸ Appearance)](2026-10-30/110649-new-windows-glow-a-moment-preferences-appearance.png)

### Notifications: All Apps, mixed while they differ

*10:52* — Preferences ▸ Notifications' Applications table has an All Apps row over the list: each column checked when every app's is, a dash while they differ (the kit's CheckBox::mixed); a click sets it for every app. With the Deskbar box above it, the panel is taller.

![Notifications: All Apps, mixed while they differ](2026-10-30/105255-notifications-all-apps-mixed-while-they-differ.png)

### Deskbar's notification dot drains as they fade

*10:46* — An app's dot in Deskbar is filled from the left to how fresh its notifications are together (each 1/n, as the history fades them), never below a third, the rest a faint well of its colour; with Preferences ▸ Notifications ▸ Show how many beside each app's dot, the count beside it (here 2, partway through a 2-minute fade, at 2×).

![Deskbar's notification dot drains as they fade](2026-10-30/104645-deskbar-s-notification-dot-drains-as-they-fade.png)

### Alerts open in the middle of the screen, in front

*10:26* — An alert asked for where no window of its app has the focus (Empty Trash from the desktop's Trash icon) opened in the next cascade slot, at the top left of some display, where it went unseen. Now it opens in the middle of the display the pointer is on, in front and focused, and stays over its app's windows until answered.

![Alerts open in the middle of the screen, in front](2026-10-30/102609-alerts-open-in-the-middle-of-the-screen-in-front.png)

### Activity Monitor: programs using the network

*09:41* — Each process's TCP traffic, from netlink's sock_diag (tcp_info's byte counts) summed by the sockets' owners: a card in the Network section and Sent and Received columns in the process list. Connections and interface counters come from netlink too.

![Activity Monitor: programs using the network](2026-10-30/094155-activity-monitor-programs-using-the-network.png)

### Downloads in Tracker, as they will be

*08:35* — A download Chrome is writing (NAME.crdownload) shows in Tracker under the name it will have, with its kind's icon, how far along it is (a bar under its icon; '3.1 of 10 MB' and 'Downloading, 31%' in the list and preview), from the size olo-chrome marks it with (user.olo.expected_size).

![Downloads in Tracker, as they will be](2026-10-30/083525-downloads-in-tracker-as-they-will-be.png)

### Activity Monitor: every process, as a list or a tree

*07:53* — Searched and filtered with the kit's search (user:, state:, kind:, pid:), sorted by any of twelve columns (names in natural order), as a tree of who started whom (kernel threads folded here). Double-click or ⌘I for its inspector: threads, open files, memory, environment, cores, limits, priority and signals.

![Activity Monitor: every process, as a list or a tree](2026-10-30/075336-activity-monitor-every-process-as-a-list-or-a-tr.png)

### Activity Monitor: the CPU, dark

*07:50* — Each core with its frequency, each cluster's governor, where idle time goes (WFI, powered down) and the load.

![Activity Monitor: the CPU, dark](2026-10-30/075051-activity-monitor-the-cpu-dark.png)

### Activity Monitor: sections, cards and live graphs

*07:50* — A sidebar of sections (Overview, Processes, CPU, Memory, Disks, Network, GPU, Sensors, System, Graphs), each figure live in the sidebar; the Overview's cards lead into each section, and clicking a graph shows what was busiest at that moment. Graphs scroll a device pixel at a time; built from the kit's new Sidebar, dashboard and Graph.

![Activity Monitor: sections, cards and live graphs](2026-10-30/075045-activity-monitor-sections-cards-and-live-graphs.png)

### Portal: other programs' global shortcuts

*05:40* — An app's keys that work whatever has the keyboard (push-to-talk) are asked for once, listing each and its keys; ones Olo already has show as in use. Allowed ones are held by app_server for the app, through upgrades.

![Portal: other programs' global shortcuts](2026-10-30/054012-portal-other-programs-global-shortcuts.png)

### Screenshot: pick a colour

*05:30* — PickColor (for other programs, through the Screenshot portal): the screen frozen, a loupe by the pointer with the pixels around it magnified, the one under it ringed, its swatch and hex.

![Screenshot: pick a colour](2026-10-30/053025-screenshot-pick-a-colour.png)

### Portal: Set as Desktop Background

*05:30* — Other programs' Set as Desktop Background (the Wallpaper portal) asks with the picture, as a BAlert, and sets it as Preferences would; the Screenshot portal's PickColor freezes the screen with a loupe; Send by Email opens a mailto: link.

![Portal: Set as Desktop Background](2026-10-30/053025-portal-set-as-desktop-background.png)

### Deskbar: programs' unread badges and progress

*05:20* — A program's launcher badge (Slack's unread count, Chrome's download progress) lights its row's quiet dot, shaking it when the count rises, and its progress runs as a thin bar along the row's foot.

![Deskbar: programs' unread badges and progress](2026-10-30/052030-deskbar-programs-unread-badges-and-progress.png)

### Deskbar: other programs' tray icons

*05:17* — Slack's, Discord's, Telegram's tray icons (StatusNotifierItem) now sit in Deskbar's tray with Olo's own: their pictures drawn a texel to a device pixel at every scale, their tips (named by the program when they give no title), and their menus as Olo menus, read ahead so they open at once.

![Deskbar: other programs' tray icons](2026-10-30/051713-deskbar-other-programs-tray-icons.png)

### Open With: other programs' app chooser

*04:58* — When an app opens a file or link through the OpenURI portal and the handler isn't settled, Olo's own Open With window asks (the AppChooser portal): what is opened, the apps that handle it with their icons and kinds, the last choice selected, the keyboard in the list.

![Open With: other programs' app chooser](2026-10-30/045800-open-with-other-programs-app-chooser.png)

### File panel: filters, several files, an app's choices

*04:50* — Other programs' Open dialogs (the FileChooser portal) now show the app's filters as a Show pop-up (by glob or MIME type), choose several files with ⇧ and ⌘ clicks, and carry the app's own choices above the buttons, the pop-ups lined up on one divider. Chrome's multi-file uploads come back whole.

![File panel: filters, several files, an app's choices](2026-10-30/045052-file-panel-filters-several-files-an-app-s-choice.png)

### Open and Save for other programs: Olo's file panel

*04:35* — olo-portal answers the FileChooser portal: another program's Open or Save is Olo's file panel, titled as it asks, in its folder, filtered to its file types.

![Open and Save for other programs: Olo's file panel](2026-10-30/043523-open-and-save-for-other-programs-olo-s-file-pane.png)

### Permission: the portals' dialog as an Olo alert

*04:32* — olo-portal answers the Access portal, which other portals ask permission through (a camera, a screenshot): Olo's BAlert with the portal's icon, its question and its choices.

![Permission: the portals' dialog as an Olo alert](2026-10-30/043234-permission-the-portals-dialog-as-an-olo-alert.png)

### Share Screen: screens and windows

*04:15* — The screen-sharing picker offers windows too (what the app asked for): Screens and Windows, each a live thumbnail; a window is shared as only what its client draws, at its own size.

![Share Screen: screens and windows](2026-10-30/041521-share-screen-screens-and-windows.png)

### Deskbar: what is being shared, with Stop Sharing

*03:55* — While an app shares a screen, Deskbar's tray shows a screen with a recording's red dot; its menu says what is shared with whom and stops it (the app is told the share closed).

![Deskbar: what is being shared, with Stop Sharing](2026-10-30/035525-deskbar-what-is-being-shared-with-stop-sharing.png)

### Share Screen: the screen-sharing picker

*03:36* — olo-portal's picker when an app asks to share the screen (xdg-desktop-portal ScreenCast): who asks, the screens as live thumbnails shaped like them, Cancel / Share. A fixed kit dialog opened over the asking window.

![Share Screen: the screen-sharing picker](2026-10-30/033635-share-screen-the-screen-sharing-picker.png)

### Agent Screens: History and Replay

*02:40* — Each agent screen keeps what was done on it and the frames it drew. History lists the actions (searchable), shows the screen as it was at the one chosen, and plays it back in time with them: clicks as rings, keys as captions, quiet stretches skipped. Export Video… makes it an MP4 in process. Stopped screens' histories stay listed, by last use; how much is kept is in Preferences ▸ AI.

![Agent Screens: History and Replay](2026-10-30/024031-agent-screens-history-and-replay.png)

### Deskbar: a dot for apps with notifications

*01:44* — An application with notifications unread has a quiet dot at the right of its Deskbar row, in the colour the history marks unread ones with (not an alarm's red); a new one arriving shakes it gently side to side, unless Do Not Disturb is on. Agents read the row's value, "has notifications".

![Deskbar: a dot for apps with notifications](2026-10-30/014422-deskbar-a-dot-for-apps-with-notifications.png)

### Tracker: Column View keeps its highlight

*01:14* — → into a folder highlights its first item again. A selection put back after an upgrade waited for items that never came, and wiped the active column's selection each time a listing landed; now a waiting selection belongs to its own folder only, and the window comes back with its columns. → before a listing lands selects once it does, and a selection made in code scrolls into view in every view.

![Tracker: Column View keeps its highlight](2026-10-30/011428-tracker-column-view-keeps-its-highlight.png)

### Quick Look: TOML (and INI) highlighted

*00:29* — TOML files (and Cargo.lock, uv.lock, poetry.lock) are coloured by a grammar of Quick Look's own: tables as headings, keys, strings of every kind, numbers, booleans and dates, arrays and inline tables; INI-style files (systemd units, .desktop entries, git config) by another. Light and dark, at 2×.

![Quick Look: TOML (and INI) highlighted](2026-10-30/002957-quick-look-toml-and-ini-highlighted.png)

## 2026-10-29

### Auto-tiling: every window fits where an arrangement lets it

*23:53* — Before (left): five windows opened in turn under auto-tiling on a 1440×900 display, the fifth (Passwords) halving a tile too small for it, overlapping three. After (right): the tree arranged again so each window has its least size: Disk Usage over Log Viewer, Passwords over StyledEdit and Activity Monitor.

![Auto-tiling: every window fits where an arrangement lets it](2026-10-29/235331-auto-tiling-every-window-fits-where-an-arrangeme.png)

### Passwords: passkeys in Chrome

*23:45* — Passwords is Chrome's passkey authenticator: it asks in an alert in front which account to sign in with (a choice when there are several, Use Another Device… for Chrome's own QR-code sheet), and a sign-in that suggests passkeys has Olo ▸ Sign In with Passkey in the field's menu. At 2×; reviewed at 1×, 1.5× and 2×.

![Passwords: passkeys in Chrome](2026-10-29/234533-passwords-passkeys-in-chrome.png)

### Terminal: smaller pull request marks in the tab strip

*23:28* — Before (left) and after at 2×. The pull request mark is drawn as big as the text (12 pt, from GitHub's 16-unit drawing, its lines whole device pixels at every scale) and sits in the close box's place, which takes it over only under the pointer, so titles get the room back and nothing moves; each group's colour bar stops short of the next group's.

![Terminal: smaller pull request marks in the tab strip](2026-10-29/232808-terminal-smaller-pull-request-marks-in-the-tab-s.png)

### Log Viewer: narrow, dark

*23:11* — In a narrow window the menus go under the search, their text over the list's first column.

![Log Viewer: narrow, dark](2026-10-29/231143-log-viewer-narrow-dark.png)

### Log Viewer: one search, filters as menus

*23:11* — The search takes words, phrases, -words and program:/level:/since:/pid: filters (several programs at once, all, or one left out), with Program, Level and Time menus beside it editing the same text; what was found is marked in the messages.

![Log Viewer: one search, filters as menus](2026-10-29/231143-log-viewer-one-search-filters-as-menus.png)

### Filter menus with counts

*23:04* — A filter's menu: Any, then each value with how many items have it, where a shortcut would be.

![Filter menus with counts](2026-10-29/230407-filter-menus-with-counts.png)

### Passwords: search, dark

*23:04* — A left-out filter (-has:reused) in red; matches marked.

![Passwords: search, dark](2026-10-29/230407-passwords-search-dark.png)

### Passwords: search with filters

*23:04* — The search field colours filters and suggests them as you type (has:we → Website, Weak Password, with counts); a filter bar over the list (Tag, Has) edits the same query. Words find usernames and non-secret fields of every item, best matches first, marked in the rows.

![Passwords: search with filters](2026-10-29/230406-passwords-search-with-filters.png)

### Icons: Integrations, GitHub, Chrome and Slack

*22:46* — Olo's own takes on GitHub's, Chrome's and Slack's marks (and Integrations' puzzle pieces), in every theme; Chrome's and Slack's replace the icons those programs ship wherever Olo shows them.

![Icons: Integrations, GitHub, Chrome and Slack](2026-10-29/224613-icons-integrations-github-chrome-and-slack.png)

### Preferences: Integrations, dark

*22:46*

![Preferences: Integrations, dark](2026-10-29/224613-preferences-integrations-dark.png)

### Preferences: Integrations

*22:46* — A panel for every service Olo works with, each with its own icon: GitHub shows whom Olo is connected as and from where (a token kept by Olo, GH_TOKEN, or the GitHub CLI's sign-in, now read from the login keyring too), and connects with a token. Terminal's pull requests use it.

![Preferences: Integrations](2026-10-29/224612-preferences-integrations.png)

### Terminal: quiet group chips along the top

*21:56* — Along the top, a group's chip is a dot of its colour and its name in quiet bold, no pill, with the line of its colour over its tabs; pane heads take the group's colour too. (1×, night.)

![Terminal: quiet group chips along the top](2026-10-29/215608-terminal-quiet-group-chips-along-the-top.png)

### Terminal: side tabs as Deskbar's list

*21:56* — Tabs down the side are Deskbar's rows now: the panel's ground, a hairline under each, the one showing recessed, groups under small bold headings with a dot of their colour and a bar of it down the left. Show All Tabs' pane heads are tinted with their group's colour, a line of it across the top. (1.5×, light.)

![Terminal: side tabs as Deskbar's list](2026-10-29/215607-terminal-side-tabs-as-deskbar-s-list.png)

### Terminal: tab groups down the side

*21:17* — Down the side, each group's chip heads its tabs with a bar of its colour beside them.

![Terminal: tab groups down the side](2026-10-29/211733-terminal-tab-groups-down-the-side.png)

### Terminal: tab groups in a window

*21:17* — Several groups per window, as a browser's: each run led by a chip with its name in its colour and a line of it over its tabs. Tabs join and leave by dragging; a chip drags its whole group; a tab or group dragged out of the strip becomes a window under the pointer.

![Terminal: tab groups in a window](2026-10-29/211731-terminal-tab-groups-in-a-window.png)

### Preferences ▸ Terminal (dark)

*18:06* — The same panel in dark mode.

![Preferences ▸ Terminal (dark)](2026-10-29/180617-preferences-terminal-dark.png)

### Preferences ▸ Terminal (light)

*18:06* — A new panel: where New Worktree… makes a repository's worktrees, ~/worktrees to start, each repository a folder of its name in there. Terminal follows it live.

![Preferences ▸ Terminal (light)](2026-10-29/180615-preferences-terminal-light.png)

### Terminal: New Worktree…

*18:06* — The group's New Worktree… (also Tabs ▸ Repository): a name that is a folder and a branch both, made in Preferences ▸ Terminal's location; the note says where it goes and on which branch (or, in red, why not), as you type. Files check out on a thread with progress and Stop.

![Terminal: New Worktree…](2026-10-29/180609-terminal-new-worktree.png)

### Terminal: repositories, worktrees and pull requests

*18:05* — A window whose tabs are all in one git repository becomes its group on its own; the group's menu lists the repository's worktrees and makes new ones; a tab on a branch with a pull request on GitHub wears GitHub's mark in the state's colour, its commit filled as its checks pass, fail or run (here: open, checks running).

![Terminal: repositories, worktrees and pull requests](2026-10-29/180540-terminal-repositories-worktrees-and-pull-request.png)

### Quick Look: search, sort and scroll tables

*17:29* — CSV and spreadsheets in Quick Look scroll sideways to every column, take the first row as the columns' names, line numbers up on the right, keep only the rows a search finds (in every column or one) and sort by a header click, numbers as numbers. Big sheets are searched and sorted on a worker.

![Quick Look: search, sort and scroll tables](2026-10-29/172908-quick-look-search-sort-and-scroll-tables.png)

### Quick Look: tables, pictures and links

*16:20* — Tables are as wide as what they hold and line up as the Markdown says; the document's own pictures show, crisp; links open, and a #heading link scrolls to it. Long documents lay out a few milliseconds a frame, so nothing waits.

![Quick Look: tables, pictures and links](2026-10-29/162021-quick-look-tables-pictures-and-links.png)

### Quick Look: Markdown as a set page

*16:20* — Markdown in Quick Look reads like a well-set page: a measure of at most 680 points, a 20-point line, headings with room above and rules under the first two, hanging indents with lined-up numbers and real task boxes, GitHub's alerts, and code coloured by its language.

![Quick Look: Markdown as a set page](2026-10-29/162020-quick-look-markdown-as-a-set-page.png)

### A pinned banner says so

*16:01* — The header of a banner whose notes are all pinned reads Pinned beside its close box; the history adds Pinned after a note's time.

![A pinned banner says so](2026-10-29/160148-a-pinned-banner-says-so.png)

### Notifications: pin and snooze

*16:01* — A right click on a note (in the History, or on a banner) pins it or snoozes it; Edit has the same for the selected note. Pinned notes never fade, survive Clear All and Close All, and their banners stay on screen until closed, even across a restart. Snoozed ones go away and come back as new after 15 minutes, an hour, three hours or at nine tomorrow morning.

![Notifications: pin and snooze](2026-10-29/160147-notifications-pin-and-snooze.png)

### Menus in the profile's keys (Windows)

*15:11* — Every app's menu keys come from the keyboard profile: with Windows' keys Tracker's File menu has Explorer's (Alt+↑, Alt+Enter, F2, Del, Ctrl+Shift+N); with a Mac's, Finder's (⌘↑, ⌘I, ⇧⌘N, ⌘⌫).

![Menus in the profile's keys (Windows)](2026-10-29/151108-menus-in-the-profile-s-keys-windows.png)

### Keyboard profiles in Preferences

*15:11* — Preferences ▸ Keyboard ▸ Shortcuts picks a keyboard profile (Mac, Windows, BeOS, or one saved) with its command key and key behaviour, then lists every shortcut in the platform's own key names (here Windows': Win+← snaps left); a change makes it “Windows (Customized)”.

![Keyboard profiles in Preferences](2026-10-29/151101-keyboard-profiles-in-preferences.png)

### Keyboard Shortcuts sheet (⇧⌘/)

*15:10* — Every shortcut of the keyboard profile on one sheet: the front app's menu keys first (Tracker's here), then text editing's and the desktop's, as cards in columns; ⇧⌘/ on a Mac, Win+Shift+/ on Windows, Alt+Shift+/ on BeOS.

![Keyboard Shortcuts sheet (⇧⌘/)](2026-10-29/151050-keyboard-shortcuts-sheet.png)

### Preferences: workspaces are per display

*13:43* — The Workspaces panel says each display has its own; Displays has no Deskbar-on-every-display setting any more.

![Preferences: workspaces are per display](2026-10-29/134336-preferences-workspaces-are-per-display.png)

### Deskbar: tray icons in even rows

*13:43* — More icons than fit go into as many rows as needed, shared out evenly (eight are two rows of four), at 1x, 1.5x and 2x, light and dark.

![Deskbar: tray icons in even rows](2026-10-29/134336-deskbar-tray-icons-in-even-rows.png)

### Twitcher: displays numbered

*13:43* — Columns are displays (D0, D1, D2 and their names), rows workspace numbers: a cell is that display's workspace.

![Twitcher: displays numbered](2026-10-29/134335-twitcher-displays-numbered.png)

### Deskbar: a workspace heading's menu

*13:43* — Right-clicking a heading: Show, Hide or Close All of its windows, move them to the same workspace of another display, or to any workspace of any display.

![Deskbar: a workspace heading's menu](2026-10-29/134335-deskbar-a-workspace-heading-s-menu.png)

### Deskbar: every display's own workspaces

*13:43* — Each display has its own workspaces and its own Deskbar, listing only its workspaces, headed D0/D1/D2 Workspace N; the windowing mode moved from the tray to each heading's right end (D2's is Auto-Tiling here).

![Deskbar: every display's own workspaces](2026-10-29/134334-deskbar-every-display-s-own-workspaces.png)

### Default windowing mode in Preferences

*11:05* — Preferences ▸ Windows chooses the mode a workspace starts in; Deskbar's tray chooses each workspace's own.

![Default windowing mode in Preferences](2026-10-29/110558-default-windowing-mode-in-preferences.png)

### Choose the windowing mode from the tray

*11:05* — Deskbar's tray icon is now the windowing mode (Standard, Auto-Tiling or Focus) of this workspace on this display: either click opens the modes, then the arranging commands. ⌃⌥⌘T and ⌃⌥⌘O toggle Auto-Tiling and Focus.

![Choose the windowing mode from the tray](2026-10-29/110558-choose-the-windowing-mode-from-the-tray.png)

### Focus mode

*11:05* — A new windowing mode: only the active window shows, zoomed, with its app's other windows as tabs along the top and its dialogs and panels over it. Activating another window (Deskbar, ⌘Tab, the overview) shows that one instead.

![Focus mode](2026-10-29/110557-focus-mode.png)

### Terminal's sudo window shows the whole command

*10:30* — A command too long for the window is elided in the field; resting on it shows the whole command in a kit tip that extends past the window (egui's own was cut at its edge).

![Terminal's sudo window shows the whole command](2026-10-29/103033-terminal-s-sudo-window-shows-the-whole-command.png)

### The key's tip, past Deskbar's edge

*10:30* — While on, the key is gold and its tip says how long is left. Tray tips now show in a popup of their own (olo_kit::tip), so they reach past Deskbar's narrow window instead of being cut.

![The key's tip, past Deskbar's edge](2026-10-29/103033-the-key-s-tip-past-deskbar-s-edge.png)

### Approve Without Asking

*10:30* — Deskbar's new key in the tray turns on approving without asking for a while: sudo in Terminal and Passwords' unlock and op go ahead without a window, each in a quiet notification. It takes the account password and ends at its time or when the screen locks.

![Approve Without Asking](2026-10-29/103031-approve-without-asking.png)

### Terminal comes back after the session ends

*09:28* — When the session ends, olo-termd keeps each shell's lines. Terminal remembers its windows and tabs, so the next login puts each tab back in its folder, with what it showed and a dim "Restored from" line.

![Terminal comes back after the session ends](2026-10-29/092808-terminal-comes-back-after-the-session-ends.png)

### Spotlight while switching

*09:01* — While ⌘Tab is held, every window but the chosen app's dims, so the one you're about to switch to stands out; it follows each step and fades back when you let go or cancel.

![Spotlight while switching](2026-10-29/090100-spotlight-while-switching.png)

### The overview, on ⌃Space

*08:59* — Every workspace at once, a pane each, its windows spread out live and lettered: click one or type its letter to go to it, the arrows and Return walk them, Esc leaves. Windows showing glide into place and back; the rest fade in.

![The overview, on ⌃Space](2026-10-29/085950-the-overview-on-space.png)

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
