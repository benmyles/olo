<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="website/static/brand/olo-logo-on-dark.png">
    <img src="website/static/brand/olo-logo.png" alt="Olo Desktop" width="420">
  </picture>
</p>

<p align="center"><b>A desktop for Linux in the spirit of BeOS. Fast, light, native, and made for people and AI agents alike.</b></p>

<p align="center"><a href="https://olo-desktop.com">olo-desktop.com</a> · <a href="#ai-rex-and-integrations">AI &amp; Rex</a> · <a href="#the-desktop">The desktop</a> · <a href="#the-apps">The apps</a> · <a href="#install">Install</a> · <a href="docs/HACKING.md">Hacking</a></p>

**Olo** is a whole desktop for Linux, from the Wayland window server to
more than two dozen apps, written in Rust on one shared kit. It keeps
BeOS's yellow window tabs, Deskbar, Tracker's spatial folders and a menu
bar in every window, and adds what work in 2026 needs: **Rex** for
coding agents and the hosts they run on, **Agent Screens** an agent
works on and you can watch and take over, **Token Pool** for the
subscriptions they spend, your accounts under one roof in
**Integrations**, five looks and twenty-three colour schemes, seven
tiling layouts, and upgrades in place that keep every window open.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-desktop-dark.png">
  <img src="docs/img/olo-desktop.png" alt="Olo's desktop: Rex with an agent, its child agents and its transcript, Tracker showing a picture's preview, Calendar, and Deskbar with its live widgets at the top right">
</picture>

> **Olo is beta software.** It is developed and tested on a few
> computers (a Lenovo ThinkPad P1 Gen 9 above all), on Fedora 44 and 45.
> It installs beside GNOME, which stays untouched, so trying it costs
> nothing; on hardware Olo has not met yet, expect rough edges such as a
> special key that does nothing, a touchpad needing tuning, or a display
> mode not yet offered. [Your computer can be made one it knows](#your-hardware).

## What makes it Olo

- **Built for AI agents and remote hosts.** Rex manages local and remote
  agent sessions, nested child tabs and live transcripts; Agent Screens
  gives agents full headless desktops with structured widget trees; the
  Olo Desktop Bridge connects remote hosts back to your desktop under
  explicit per-host permissions; Token Pool shares subscriptions locally
  and on remote machines.
- **Fast and light, measured.** Idle costs nothing: no polling, no
  frames when nothing changed, and large resting windows release their
  GPU swapchains after thirty seconds. Work that takes more than a frame
  runs on a worker thread with live progress and cancellation. Every
  change to per-frame, per-event or idle work is measured on release
  builds before and after, with the numbers in its commit.
- **Native, all the way down.** The Wayland window server, the shared
  kit, the desktop services and Olo's native apps are written in Rust
  and speak formats and protocols in process: Bluetooth phone profiles,
  SSH, Telegram MTProto, CalDAV, iCalendar, PDFs, OCR, video decoding,
  archives and encrypted backups, so the whole desktop looks and behaves
  as one.
- **Pixel-perfect at every scale.** Light, dark and night modes at 1×,
  1.25×, 1.5×, 1.75× and 2×, checked at each; nothing drawn from a
  bitmap is resampled, one texture pixel to one device pixel.

## AI, Rex and integrations

Coding agents, the machines they run on and the accounts they and you
use are part of the desktop here, each with a place of its own.

### Rex: local and remote workspaces for agents

**Rex** is Olo's native client for local and remote Rex servers,
rendering with `libghostty-vt` over local sockets, native Rust SSH
(`russh`) or Tailscale. Sessions live on the server, so they survive
desktop reloads, laptop sleeps and network changes.

![Rex: this computer and a host in its sidebar, a shell, a coding agent and a dev server in three panes](docs/img/olo-rex.png)

- **Quick on a slow link.** Predictive type-ahead shows your
  keystrokes immediately (pausing automatically at password prompts and
  working cleanly inside full-screen agent TUIs), backed by Olo's static
  `olo-rex-link` helper deployed automatically to remote hosts with no
  root required, optional UDP/QUIC transport behind SSH, and adaptive
  stream catch-up.
- **Tabs, splits and remote convenience.** Top or side tabs, recursive
  pane splits, `⌘`-click links, Quick Look of remote files in local Olo
  apps, pasted desktop screenshots uploaded directly to remote panes,
  Command Palette search across every session and pane, a Deskbar Glance,
  and Tailscale QR pairing for Remote Access.
- **Host, account and repository setup.** Add Host can provision Rex on
  Ubuntu and Fedora hosts automatically. Each host's **Account** window
  installs Claude Code, Codex, Antigravity CLI, git identity and the Olo
  skill; carries your agent settings with a side-by-side diff, merge and
  remembered-choice reviewer; copies recent agent sessions across hosts
  with path rewriting; and clones your repositories with their setup
  commands.
- **Move to Another Host.** Migrate a working Rex account from one
  machine to another in two parallel `rsync` passes (bringing your home
  directory and `/var/tmp` while leaving the original intact), resuming
  live agent sessions on the new host, transferring Token Pool custody,
  and keeping **Go Back** if you decide to revert.

### Coding agents, child tabs and live transcripts

Rex and Terminal both recognise **Claude Code**, **Codex** and
**Antigravity (`agy`)** out of the box, showing whether each agent is
working, waiting for you, done or failed on its tab, in the **Activity**
sidebar, in Deskbar, and in notifications that jump straight to the right
pane (`⌘J` jumps to the next waiting agent).

![Rex's Activity: an agent with its child agents under it, and its transcript with a tool call opened](docs/img/olo-rex-agents.png)

- **Child tabs that run without the desktop.** Through `olo agent`, an
  agent starts subagents (`claude`, `codex`, `agy`) or plain commands as
  nested child tabs under its own tab. They run on the host's own agents
  service (`olo-rex-link agent serve`) with zero-polling cursor watches
  (`olo agent watch`) and semantic approval (`olo agent approve`), so
  multi-agent runs keep going even when your laptop is closed; reconnect
  and every running child appears immediately in Rex.
- **Live Transcript viewer and subagent trees.** Switch any agent pane
  between its raw terminal and a turn-grouped **Transcript** with
  collapsible tool calls, inline Find and Follow. Rex discovers nested
  subagents across Claude, Codex, Antigravity, child processes and tmux
  sessions.
- **Keep It Going and Auto Approve.** In both Rex and Terminal, **Keep It
  Going** nudges an idle agent after two quiet minutes at its prompt (at
  most once an hour, with your default or per-tab wording, never into a
  chooser). **Auto Approve** counts down thirty seconds, with a Stop
  button, before it answers a permission or folder-trust question; it
  never approves a plan, and it is yours to turn on: asked for by an
  agent, it asks you first, in an alert whose Return is Cancel.

### Olo Desktop Bridge and per-host permissions

Inside any Rex session, the `olo` command reaches back to your desktop,
and optionally installs transparent shims for `xdg-open`, `notify-send`,
`wl-copy`/`wl-paste` and `op`. Nothing is granted automatically to remote
hosts: all eight permissions start off and are toggled independently per
host in **Preferences ▸ Rex ▸ Hosts**.

![Rex's Hosts and Ports window beside a host's own window: its Rex server, the typing helper, Olo Integration and Files](docs/img/olo-rex-host.png)

- **Links, ports, files and live folder sync.** `olo open` opens URLs,
  auto-forwards remote `localhost` ports into your browser, and opens
  remote files in local Olo apps; `olo forward` manages ephemeral or
  persistent port forwards that survive upgrades; `olo send` transfers
  files to `Downloads ▸ Rex ▸ HOST`; `olo sync` runs zero-polling
  inotify sync (`--from-host`, `--to-host`, or `--two-way` three-way
  merge keeping conflict copies) via FastCDC, zstd and BLAKE3; and
  **Tracker** browses any host's folders directly at `rex://HOST/PATH`.
- **Passwords, SSH keys, clipboard and guarded `olo run`.** Remote hosts
  you trust can query `olo op` against your desktop Passwords vault,
  borrow SSH keys from Passwords with per-host approval, share the
  clipboard, drive Agent Screens (`olo screen`), or ask to run a command
  on your desktop (`olo run`). Every `olo run` request opens an explicit
  Deny-default approval sheet with a half-second guard and streams output
  back live.

### Agent Screens: headless desktops for agents

What you can see in Olo, an agent can read as structure and drive through
a documented CLI without touching your pointer or stealing your focus.
**Agent Screens** (`olo-screen`) starts full 120 Hz headless Wayland
desktops, each with its own D-Bus bus, Google Chrome profile and optional
sound.

```sh
olo-screen start                                   # a screen of the agent's own
olo-screen run agent olo-styled-edit notes.md
olo-screen tree agent --role Button                # what's on it, as structure
olo-screen click agent --role Button --name Save
```

![Agent Screens: an agent's own desktop shown live, with its history, and Take Control](docs/img/olo-agent-screens.png)

- **Watch live, scrub back, take over.** View any agent screen live via
  zero-copy GPU sharing, press `⌘K` to **Take Control** (locking out
  agent input until you hand it back), scrub back through the frame-tile
  **Recorder** and action **History**, export hardware-encoded MP4 video,
  or move windows between your desktop and an Agent Screen.
- **Structured UI across apps and browsers.** `olo-screen tree` reads Olo
  kit widgets, Google Chrome's DOM (including cross-origin iframes, tabs
  and covered-element checks), and GTK/Qt windows via AT-SPI. Every
  action settles and returns focused state, window list, visible tree and
  screenshot. Passwords and passkeys can be shared read-only per screen.

### Terminal built for coding work

**Terminal** keeps its shells alive across reloads and upgrades
(`olo-termd`), has a bash-compatible shell written in Rust (`brush`)
with command suggestions from a small model run on the computer
(`olo-complete`), and tells the coding agents in its tabs which
repositories and worktrees they are in.

![Terminal: a tab group for a feature, each tab titled by the coding agent working in it](docs/img/olo-terminal-agents.png)

<table>
  <tr>
    <td width="33%"><img src="docs/img/olo-terminal-agent-menu.png" alt="A tab's menu: Keep It Going with its message, and Auto Approve"></td>
    <td width="33%"><img src="docs/img/olo-terminal-auto-approve.png" alt="Auto Approve counting down before it answers a coding agent's question, with Stop beside it"></td>
    <td width="33%"><img src="docs/img/olo-terminal-worktree.png" alt="New Worktree: a name, and where it goes and from which branch"></td>
  </tr>
  <tr>
    <td align="center">Keep It Going</td>
    <td align="center">Auto Approve</td>
    <td align="center">A new worktree</td>
  </tr>
</table>

- **Tab groups, worktrees and pull requests.** Group tabs by repository
  or feature; create, rename and remove Git worktrees in process through
  `libgit2` (`New Worktree…`); and see each tab's GitHub pull request
  state and check status live without polling.
- **Agent titles, context and GUI sudo.** Agents receive an MCP tool to
  keep their tab title updated with their current task and startup context
  naming their group's worktrees (configurable in **Preferences ▸
  Terminal**). Headless agent commands that need elevation prompt you via
  `olo-sudo` in an Olo window.

### Token Pool: subscriptions shared locally and on remote hosts

**Token Pool** pools multiple Claude and ChatGPT subscriptions across
Claude Code and Codex in one unified window (**Accounts**, **Activity**,
**Machines**), scheduling earliest-expiring quotas first, tracking
per-model limits, sending client-identical requests inside a Landlock
sandbox, and handing over open streams across upgrades with no gap.

<table>
  <tr>
    <td width="50%"><img src="docs/img/olo-token-pool.png" alt="Token Pool's accounts: Claude and ChatGPT subscriptions, each with its five-hour and weekly quota"></td>
    <td width="50%"><img src="docs/img/olo-token-pool-machines.png" alt="Token Pool's machines: this computer's gateway and the hosts that share the accounts"></td>
  </tr>
  <tr>
    <td align="center">Accounts</td>
    <td align="center">Machines</td>
  </tr>
</table>

- **Standalone remote host pools.** Deploy `olo-pool-host` to your remote
  Rex hosts over SSH from the **Machines** tab. Remote pools keep serving
  agents on those hosts even when your desktop is powered off, with
  single-keeper refresh-token custody and automatic account sync.
- **Deskbar visibility.** Pin Token Pool quota bars or multi-account
  heatmaps straight into Deskbar.

### Integrations and desktop AI

**Preferences ▸ Integrations** brings external accounts into one place
across five provider tabs, while **Preferences ▸ AI** connects OpenAI
models (through Token Pool) and Claude Code defaults to Olo's own tools.

![Preferences ▸ Integrations: two Google accounts, each ticked for what it is used for](docs/img/olo-integrations.png)

- **Five provider tabs.** Connect multiple **Google** accounts with
  per-account ticks for Mail, Calendar and DiskBackup (adding one has
  Claude Code make the account's own Google Cloud app for you, on an
  Agent Screen you can follow), **Fastmail** (Mail and
  Calendar), **CalDAV** (Nextcloud, iCloud or any server), **GitHub**
  (Terminal pull request and check badges), and **Telegram** (API
  credentials and phone/QR sign-in).
- **AI in a few places, each one you can turn off.** Ask `/ask` in the Command
  Palette (aware of your active Terminal's directory and selection), get
  `⌘↩` emoji suggestions in the Emoji Picker, highlight weather windows
  with Weather's forecast assistant, or draft Calendar events that wait
  for your click before writing anything.

## The desktop

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-tracker-dark.png">
  <img src="docs/img/olo-tracker.png" alt="Tracker: the Pictures folder as icons and Quick Look on one of them">
</picture>

- **Deskbar, live widgets and App Glances.** Anchored top-right on every
  display: the Olo menu (including `~/AppImages` read in process), the
  clock showing upcoming Calendar events, a rich tray (upgrades, screen
  recording, DiskBackup, AirPlay, Agent Screens, Ethernet/Wi-Fi with
  captive-portal detection, battery, volume, notifications, Caffeinated,
  Approve Without Asking, Olo Doctor, and third-party tray icons), and
  per-workspace app lists. Fold Deskbar to a corner button (`⌃⌘D`) so
  tiled windows claim its space. Pin live system meters (CPU, memory,
  GPUs, disks, network, temperatures, battery power, Token Pool quotas)
  with hover cards showing the top five processes, plus **App Glances**
  for Notifications, Weather, Calendar, To-Do, Kache and Rex.
- **Tracker.** The desktop and spatial or single-window folders (Icon,
  List and Column views with per-folder column widths and sorts), Quick
  Look previews for images, PDFs, video, audio, documents, spreadsheets
  and archives, in-window browsing and creation of archives (`zip`,
  `tar`, `7z`, `RAR`, `ISO`, `UDF`, `cpio` with passwords), DiskBackup
  ticks and Older Versions, and remote Rex folders (`rex://HOST/PATH`).
- **Command Palette (`⌘Space`), Keyboard Hints and Emoji Picker.** Search
  apps, the front app's menu commands, desktop shortcuts, individual
  Preferences settings (opening and spotlighting the exact control), open
  windows, Rex sessions and panes, Passwords items, Calendar events,
  To-Do items (and `todo …` quick add), paths, calculator sums, `/ask`
  and `/ws` web search, ranked by an on-device SIMD neural model trained
  on your choices. **Keyboard hints** put a jump letter on every control
  in every window; the **Emoji Picker** (`⌃⌘Space`) covers Unicode 18
  with `⌘↩` AI suggestions.
- **Workspaces, Overview, Twitcher and seven auto-tiling layouts.**
  Per-display workspaces, an Overview of all of them, Twitcher (`⌘Tab`,
  `⌥Tab`, `⌘\``) with display columns and jump letters, tabbed window
  groups, and three per-workspace modes: **Standard**, **Focus** (one
  zoomed window at a time), and **Auto-Tiling** across **seven layouts**
  (**Main and Stack**, **Halves**, **Columns**, **Rows**, **Grid**,
  **Spiral**, and **Scrolling**), with synchronised drag resizes,
  Scratchpad, per-app tile pinning, and Undo/Redo Window Arrangement
  (`⌃⌘Z`).
- **Five classic looks, twenty-three colour schemes and six icon sets.**
  Choose from twenty-eight unified themes: **Classic** (BeOS-inspired),
  **Platinum** (Mac OS 9), **BeOS** (R5 palette and 8-bit icons),
  **AmigaOS** (Workbench 3 flat chrome and 16-pen icons) and **Aqua**
  (early Mac OS X gel blue and deep gloss), plus twenty-three popular
  schemes (Catppuccin, GitHub, One Dark Pro, Dracula, Tokyo Night,
  Solarized, Gruvbox, Nord, Rosé Pine, Kanagawa and more) and a **Flat**
  icon set. Edit any theme's colours, icon set or wallpaper live in
  **Customize…**, synced across GTK, Qt and Chrome. Light, dark and night
  modes; pixel-exact fractional scaling; HDR with gain maps; VRR; and
  multi-GPU output keeping the discrete GPU asleep when idle.
- **Your phone, natively over Bluetooth.** Pair an iPhone or Android
  phone with nothing installed on the phone and no cloud account between
  them: read and reply to messages, browse contacts, handle calls,
  control phone music, and route incoming SMS or email one-time codes to
  a waiting browser or agent tab.
- **Security, login, Rescue and upgrades in place.** Native login screen,
  Plymouth boot splash, start-up chime and screen saver; lock screen with
  password, Touch ID fingerprint and infrared face unlock; FIDO security
  keys and Touch ID keyboards for sudo and Passwords; Approve Without
  Asking; native polkit agent, keyring and Secret Service; **Olo Rescue**
  (`⌃⇧⌘Delete`) with safe display settings, isolated shell, process
  killer and all Preferences panels without quitting anything; and
  in-place upgrades from Deskbar that keep open windows, terminals, Rex
  forwards, Token Pool streams and DiskBackup runs alive.

<table>
  <tr>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-palette-dark.png"><img src="docs/img/olo-palette.png" alt="The command palette answering “term”: a window, an app, a Preferences panel and settings inside it"></picture></td>
    <td width="33%"><img src="docs/img/olo-overview.png" alt="The overview: a workspace's windows, each with a letter to jump to it"></td>
    <td width="33%"><img src="docs/img/olo-tiling.png" alt="Auto-tiling: Tracker beside Terminal and StyledEdit, DeskCalc floating over them"></td>
  </tr>
  <tr>
    <td align="center">The command palette</td>
    <td align="center">Workspace overview</td>
    <td align="center">Auto-tiling</td>
  </tr>
</table>

### Themes and looks

Classic, Platinum, BeOS, AmigaOS, Aqua and twenty-three colour schemes,
each with its own matching wallpaper and live **Customize…** editor.

![Six of Olo's themes: Classic, Platinum, BeOS, AmigaOS, Aqua and Tokyo Night](docs/img/olo-themes.png)

### Phone and Preferences

Your phone over Bluetooth, and twenty-nine **Preferences** panels: one
per area, from Appearance, Themes, Deskbar and Displays to Network,
Bluetooth, AirPods, Phone, Rex, Integrations, AI, Performance and
Security.

<table>
  <tr>
    <td width="50%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-phone-dark.png"><img src="docs/img/olo-phone.png" alt="Phone: a conversation, a bank's verification code in the list, and the phone's music"></picture></td>
    <td width="50%"><img src="docs/img/olo-preferences.png" alt="Preferences: Themes, and Backgrounds on Olo's own wallpapers"></td>
  </tr>
  <tr>
    <td align="center">Phone over Bluetooth</td>
    <td align="center">Preferences: Themes and Backgrounds</td>
  </tr>
</table>

## The apps

Twenty-four apps of Olo's own, **Preferences** (twenty-nine panels) and
**Notifications**, all in Rust on one shared kit; and Google Chrome and
Slack, set up by Olo to run as Wayland programs that follow its ways.

### AI and remote work

| App | What it does |
| --- | --- |
| **Rex** | Local and remote agent workspaces over SSH and Tailscale, predictive typing, nested child tabs (`olo agent`), live Transcripts, host/account setup, `olo sync` and Move to Another Host |
| **Terminal** | Persistent shells (`olo-termd`), repo tab groups, in-process `libgit2` worktrees, GitHub PR/check badges, coding-agent titles, Keep It Going, Auto Approve, `brush` shell and `olo-complete` suggestions |
| **Agent Screens** | Full 120 Hz headless Olo desktops for agents, Live GPU viewer, Take Control (`⌘K`), action History, frame-tile Recorder, MP4 export, structured widget/Chrome DOM trees and scoped Passwords sharing |
| **Token Pool** | Unified Accounts, Activity and Machines window sharing Claude and ChatGPT subscriptions across local agents and standalone remote host pools (`olo-pool-host`) with zero-gap upgrades |

### Files, writing and media

| App | What it does |
| --- | --- |
| **Tracker** | Desktop and spatial/single-window folders (Icon, List, Column), Quick Look, native archives (`zip`, `tar`, `7z`, `RAR`, `ISO`, `UDF`, `cpio`), DiskBackup ticks/Older Versions and `rex://` remote host folders |
| **StyledEdit** | Plain text with xattr styles, Markdown and syntax-highlighted code, tables, pictures, ruler, Page View, spelling, print, and import/export (`.docx`, `.odt`, `.rtf`, HTML, Markdown, PDF) |
| **Viewer** | Images (crop, rotate, flip, resize, on-device OCR text selection), PDFs (form filling, signing, search, Full Screen and Presentation modes), `.pptx` decks, GPU-decoded video (0.25×–2×) and audio |
| **DeskCalc** | BeOS's desk calculator, with scientific keys |

### Planning, communication and accounts

| App | What it does |
| --- | --- |
| **Calendar** | Unified Google, Fastmail and CalDAV accounts, Day/Week/Month/Agenda views, lossless iCalendar, drag to move/resize, invitations and RSVP, AI event drafter, reminders and Deskbar widgets |
| **To-Do** | `.ics` task lists, Today/Upcoming/All/Done and custom lists, List and Kanban Board views, natural-language quick entry, reminders, attachments, Deskbar widgets and Command Palette quick add |
| **Passwords** | Encrypted local vault (XChaCha20-Poly1305) with optional reviewed 1Password multi-account sync, passkeys, TOTP and QR setup scanning, SSH keys and session SSH agent, drop-in `op` CLI and Agent Screen sharing |
| **Phone** | Native Bluetooth MAP/PBAP/ANCS/HFP for iPhone and Android: messages, contacts, calls, phone music control and SMS/email one-time codes routed to waiting tabs with no phone app installed |
| **Telegram** | Pure-Rust MTProto client with folders, forums, Unknown Senders, persistent outbox, Ogg Opus voice recording, Lottie and WebM stickers, and inline notification replies |
| **Google Chrome** | Native Wayland launcher with rootless auto-updates, Olo extension (Passwords, passkeys, cards, QR TOTP, SMS/email OTP autofill), multi-window link picker, Deskbar/Tracker downloads and Agent Screen DOM trees |
| **Slack** | Isolated Chrome app profile with native Olo notifications and sender faces, Deskbar badge, external links in your default browser, Open in Viewer, palette channel jump and auto-status |
| **Weather** | Forty-four forecast layers via tomorrow.io, interactive timelines including cloud base and ceiling, an AI forecast assistant that highlights time windows on charts, and Deskbar widgets |

### Capture, disks, diagnostics and settings

| App | What it does |
| --- | --- |
| **Screenshot** | Screen, window and region captures, colour picker, and native GPU/CPU H.264 and AV1 MP4 screen recording capturing only changed frames |
| **Activity Monitor** | Overview, Processes, Supervisor, CPU, Memory/zram, Disks, Network, per-engine GPU, AI suggestions stats, Sensors/fans/battery, kernel tunables and clickable historical graphs |
| **Olo Doctor** | What is wrong with the computer and what could be better, by how much it matters, each with a button that fixes it |
| **OS Updates** | Native DNF5 daemon client for Fedora updates with advisories, CVEs, changelogs, offline updates, Copr/RPM Fusion repos, transaction Undo and Fedora version upgrades |
| **Log Viewer** | Native systemd journal and Agent Screen log reader with live tail, query language, syntax highlighting and Show in Context |
| **DriveSetup** and **DiskUsage** | Disk map, LUKS unlock, partition/format/resize, image write and benchmark via UDisks2; sunburst space chart with Collector and Trash undo |
| **DiskBackup** | Continuous, versioned, deduplicated (`FastCDC` + `zstd` + `XChaCha20-Poly1305`) backups to local folders or Google Drive with fast/full validation, speed limits and Tracker Older Versions |
| **Speed Test** | Cloudflare upload, download and loaded latency with kernel `TCP_INFO`/BBRv3 and server `cfL4` diagnostics |
| **Kache** | Native Rust build-cache dashboard with miss-cause analysis, hourly chart, slot/test manager, settings sliders and Deskbar widget |
| **Preferences** and **Notifications** | Twenty-nine Preferences panels covering every part of the desktop, plus Notification banners, Inbox history, Snooze and per-app rules |

<table>
  <tr>
    <td width="50%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-calendar-todo-dark.png"><img src="docs/img/olo-calendar-todo.png" alt="Calendar's week beside To-Do's Today list"></picture></td>
    <td width="50%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-viewer-telegram-dark.png"><img src="docs/img/olo-viewer-telegram.png" alt="Viewer on a picture beside Telegram in a group chat"></picture></td>
  </tr>
  <tr>
    <td align="center">Calendar and To-Do</td>
    <td align="center">Viewer and Telegram</td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/img/olo-styled-edit.png" alt="StyledEdit on a Markdown document with a table"></td>
    <td width="50%"><img src="docs/img/olo-doctor-kache.png" alt="Olo Doctor's findings, each with its fix, beside Kache's build cache"></td>
  </tr>
  <tr>
    <td align="center">StyledEdit</td>
    <td align="center">Olo Doctor and Kache</td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/img/olo-weather.png" alt="Weather: a place's forecast hour by hour, and its assistant"></td>
    <td width="50%"><img src="docs/img/olo-speedtest.png" alt="The Speed Test: download, upload, latency and the connection's own numbers"></td>
  </tr>
  <tr>
    <td align="center">Weather</td>
    <td align="center">Speed Test</td>
  </tr>
</table>

Underneath the apps, Olo ships its own native desktop services rather
than pulling in GNOME's: Wayland portals, keyring (unlocked by your login
password) and Secret Service, dconf for GTK programs, polkit agent,
AirPods management, AirPlay mirroring and casting, and voice processing
for every microphone, plus CLI tools (`pbcopy`/`pbpaste`, `op`, `olo-screen`,
`olo`). Programs from other toolkits (Chrome, GTK, Qt, Electron) run as
Wayland clients and follow Olo's colours.

## Install

On a stock **Fedora 44 or 45 Workstation** (x86_64), download the
Installer from the [latest release](https://github.com/benmyles/olo/releases/latest)
and run it (pass `--console` on minimal or headless Fedora without GNOME):

```sh
curl -LO https://github.com/benmyles/olo/releases/latest/download/olo-installer-x86_64
chmod +x olo-installer-x86_64 && ./olo-installer-x86_64
```

<p align="center"><img src="docs/img/olo-installer.png" alt="Olo's Installer: one full-screen program, after BeOS's own" width="640"></p>

The Installer is one full-screen program, after BeOS's own: agree to the
licence, then click Begin. It installs what Olo needs from Fedora,
downloads Olo and checks it, puts it in your home folder (`~/.local`)
and adds it to the login screen; if you like, it also installs Olo's own
login screen and boot splash, and RPM Fusion. It asks for your password
once, and Fedora's GNOME stays installed: you choose Olo or GNOME each
time you log in. Once inside Olo, Deskbar checks GitHub releases and
offers new versions right in its tray, or you can run the Installer
again, keeping what you chose and bringing everything up to date.

To build from source instead (Fedora 44 or 45, Workstation or an atomic
desktop such as Silverblue; x86_64 or aarch64):

```sh
git clone https://github.com/benmyles/olo && cd olo && tools/setup.sh
```

`tools/setup.sh` installs what's missing (compilers, libraries, Rust,
Git LFS, drivers), builds Olo into `~/.local/bin` and adds it to the
login screen. Run it again any time: each step only does what isn't done
yet. After a `git pull` and a build, Deskbar offers the new build in its
tray, and its Upgrade installs it and reloads the running desktop in
place.

### Your hardware

Olo is at its best on the computers it was developed on, where the
panel's HDR and refresh rates, both GPUs, the touchpad's palms, the
special keys, the fans, the battery's charge limit and the speakers were
each looked at, measured and made right. For any other computer there is
a skill for coding agents, `olo-hardware`, that does the same for yours:
it reads what your computer has, compares it with what Olo supports,
agrees with you what to do, and implements, tests and documents it, the
way the ThinkPad P1 Gen 9's support was made.

Running Olo, in a clone of this repository, start your coding agent and
ask for it:

```sh
git clone https://github.com/benmyles/olo && cd olo
claude --model claude-opus-5-5 --effort xhigh   # Claude Code; then: /olo-hardware
codex                                           # Codex; then: use the olo-hardware skill
```

Claude Opus 5.5 at extra-high effort, as that command starts it, is
highly recommended for the best results: the skill reads the whole of
your computer and Olo before it changes anything, and makes, tests and
measures what it adds.

Olo puts its skills where Claude Code and Codex look for them as you log
in. With another agent, or before you have logged in to Olo, say: *read
docs/skills/olo-hardware/SKILL.md and follow it*. It changes nothing on
your computer by itself: what needs your password is an Olo Doctor fix
you apply with a button, and what it builds you install with Deskbar's
Upgrade. If the result would help others with the same computer, it
offers to open a pull request.

## Wallpapers

Olo's own abstract collection in Olo blue and Olo yellow (Orbit, Halo,
Ribbons, Shapes, Glass and Dunes, most with dark versions and all with
HDR gain maps), beside the classic systems' own, redrawn for 4K: Mac OS
9 and X, AmigaOS, Windows 3.1 to 2000, OS/2 Warp and CDE, their patterns
tiled as they were. Every theme in Olo comes paired with a matching
wallpaper from the pack.

## The brand

The Olo mark is a glossy ring in Olo blue and Olo yellow; the wordmark
is set in Sora. [docs/BRAND.md](docs/BRAND.md) has the colours, the
typeface, the logos and how they're made; the brand kit is on the
website.

## Hacking

Start with [docs/HACKING.md](docs/HACKING.md): the layout, the kit, the
look-and-feel rules, and testing in headless sessions
(`tools/dev/tsession`). [The Olo Book](docs/OLO-BOOK.md) is how the kits
work and how to build with them; [docs/ICONS.md](docs/ICONS.md) the
icon and wallpaper guide; [docs/BRAND.md](docs/BRAND.md) the brand;
[docs/TODO.md](docs/TODO.md) the work list; and
[docs/journal](docs/journal) a picture of every visible change. The
pictures here are taken by `tools/dev/readme-shots`, in 4K on an Agent
Screen from a sample home folder. `website/` is
[olo-desktop.com](https://olo-desktop.com), a static site built by its
own small Rust program ([website/README.md](website/README.md)).
`tools/release.sh` makes a release, and `tools/dev/installer-check`
tries Setup on a fresh Fedora in a virtual machine.

Olo was called Tabula until October 2026; installs from then upgrade in
place, keeping everything.

## Licence

Olo is under the MIT License ([LICENSE](LICENSE)). It carries others'
work under their own licences: code that began in Zed's GPUI (Apache
2.0: the Interface Kit's renderer and its style macros, Olo's own now;
GPUI itself is gone and won't come back), Smithay (MIT), the Noto fonts
and Sora (SIL Open Font License 1.1), Haiku's cursors (MIT), and the
crates listed in its source.
