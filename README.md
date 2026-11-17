<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="website/static/brand/olo-logo-on-dark.png">
    <img src="website/static/brand/olo-logo.png" alt="Olo Desktop" width="420">
  </picture>
</p>

<p align="center"><b>A desktop for Linux in the spirit of BeOS. Fast, light, native, and made for people and AI agents alike.</b></p>

<p align="center"><a href="https://olo-desktop.com">olo-desktop.com</a> · <a href="#install">Install</a> · <a href="#the-apps">The apps</a> · <a href="docs/HACKING.md">Hacking</a></p>

**Olo** is a whole desktop for Linux, from the window server to the
apps, written in Rust for Wayland and built on one shared kit. BeOS's
yellow window tabs, Deskbar, Tracker's spatial folders and a menu bar in
every window, brought to 2026: crisp at every scale, light, dark and
night, HDR, tiling, a command palette, upgrades in place that keep every
window open, and screens of their own for the AI agents working beside
you.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/olo-desktop-dark.png">
  <img src="docs/img/olo-desktop.png" alt="Olo: Tracker showing a picture's preview, StyledEdit, Terminal and DeskCalc, with Deskbar's live widgets at the top right, on the Orbit wallpaper">
</picture>

> **Olo is beta software.** It is developed and tested on a few
> computers (a Lenovo ThinkPad P1 Gen 9 above all), on Fedora 44 and 45.
> It installs beside GNOME, which stays as it is, so trying it costs
> nothing; but on hardware it hasn't met, expect rough edges: a special
> key that does nothing, a touchpad that wants tuning, a display mode
> that isn't offered. [Your computer can be made one it knows](#your-hardware).

## What makes it Olo

- **Fast and light, measured.** Idle costs nothing: no polling, no
  frames when nothing changed. Anything that can take more than a frame
  runs on a worker thread with live progress and can be cancelled. Every
  change to what runs per frame, per event or at rest is measured on
  release builds before and after, and the numbers are in its commit.
- **Native, all the way down.** The window server, the kit and every
  app are Olo's own, in Rust. Formats and protocols are read and spoken
  in process: no wrappers around other programs, so the whole desktop
  looks and behaves as one.
- **Pixel-perfect at every scale.** Light, dark and night modes, at 1×,
  1.5×, 2× and between, checked at each; nothing drawn from a bitmap is
  resampled, one texture pixel to a device pixel.
- **Made for agents too.** What you can see, an agent can read as
  structure and drive through a documented API, on screens of its own
  that you can watch and take over.

## Install

On a stock **Fedora 44 or 45 Workstation** (x86_64), download the
Installer from the [latest release](https://github.com/benmyles/olo/releases/latest)
and run it:

```sh
curl -LO https://github.com/benmyles/olo/releases/latest/download/olo-installer-x86_64
chmod +x olo-installer-x86_64 && ./olo-installer-x86_64
```

<p align="center"><img src="docs/img/olo-installer.png" alt="Olo's Installer" width="640"></p>

The Installer is one full-screen program, after BeOS's own: agree to the
licence, then click Begin. It installs what Olo needs from Fedora,
downloads Olo and checks it, puts it in your home folder (`~/.local`)
and adds it to the login screen; if you like, it also installs Olo's own
login screen and boot splash, and RPM Fusion. It asks for your password
once, and Fedora's GNOME stays installed: you choose Olo or GNOME each
time you log in. To update, run it again: it keeps what you chose and
brings everything up to date.

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
claude        # Claude Code; then:   /olo-hardware
codex         # Codex; then:         use the olo-hardware skill
```

Olo puts its skills where Claude Code and Codex look for them as you log
in. With another agent, or before you have logged in to Olo, say: *read
docs/skills/olo-hardware/SKILL.md and follow it*. It changes nothing on
your computer by itself: what needs your password is an Olo Doctor fix
you apply with a button, and what it builds you install with Deskbar's
Upgrade. If the result would help others with the same computer, it
offers to open a pull request.

## The desktop

![Tracker: a folder of pictures as icons, and Quick Look on one of them](docs/img/olo-tracker.png)

- **Deskbar** in the top-right corner, as on BeOS: the Olo menu, the
  clock, a tray (sound, network, battery, Bluetooth, notifications) and
  each workspace's running apps. Live widgets show the processor,
  memory, GPUs, disks, network, temperatures and power as graphs, bars,
  meters or numbers.
- **Tracker**: the desktop, and spatial folder windows (icon, list and
  column views), with Quick Look previews of pictures, PDFs, video,
  audio, documents, spreadsheets and archives.
- **The command palette** (⌘Space): apps, the front app's menu commands,
  windows, files, settings and the web. **Keyboard hints** put a letter
  on every control in every window.
- **Workspaces** for each display, an overview of all of them, and
  tiling: arrange windows from the keyboard, auto-tiling, focus mode,
  window groups with tabs, and Smart Tiling by a model.
- **Two themes**, Classic (after BeOS) and Platinum (after Mac OS 9),
  each drawing every icon, window and control in its own style; light,
  dark and night modes; fractional scaling, HDR and wide-gamut colour,
  variable refresh rate, and each display on its own GPU.
- **Keyboard profiles** for Mac, Windows and BeOS keys, a touchpad tuned
  like a Mac's, touch screens, and an emoji picker.
- **Security**: a lock screen, fingerprint, face unlock (the infrared
  camera, in process), security keys and Touch ID keyboards, Approve
  Without Asking for a while, a polkit agent, and Olo's own keyring and
  Secret Service.
- **Olo's own login screen and boot splash**, the start-up sound and
  screen, and a screen saver.
- **Olo Rescue** (⌃⇧⌘Delete, from anywhere): a full-screen terminal,
  the processes to end and every Preferences panel, on safe display
  settings, without quitting anything.
- **Upgrades in place**: a new build replaces the running desktop
  without closing a window; apps come back as they were.

<table>
  <tr>
    <td width="33%"><img src="docs/img/olo-palette.png" alt="The command palette"></td>
    <td width="33%"><img src="docs/img/olo-overview.png" alt="The overview"></td>
    <td width="33%"><img src="docs/img/olo-tiling.png" alt="Auto-tiling"></td>
  </tr>
  <tr>
    <td align="center">The command palette</td>
    <td align="center">The overview</td>
    <td align="center">Auto-tiling</td>
  </tr>
</table>

### Two themes

Classic, light and dark, on top; Platinum under it.

![The same desktop in Classic and Platinum, light and dark](docs/img/olo-themes.png)

### Preferences

One panel per thing: appearance, themes, backgrounds, displays,
keyboard, touchpad, sound, network, Bluetooth, AirPods, power,
performance, security, backups, AI and more.

![Preferences: Themes, and Backgrounds with Olo's own wallpapers](docs/img/olo-preferences.png)

## The apps

| App | What it does |
| --- | --- |
| **Tracker** | Files and folders, the desktop, Quick Look, Open With and default apps; compress and extract archives |
| **Terminal** | Tabs and tab groups, Git branches and pull requests per tab, sudo in a window, command suggestions from a local model |
| **StyledEdit** | Rich text, Markdown and source code with syntax colouring, spelling, tables, pictures, find and print |
| **Viewer** | Pictures (edit, crop, rotate), PDFs (fill forms, sign), video decoded on the GPU (NVIDIA, Intel, AMD) and audio |
| **DeskCalc** | BeOS's desk calculator, with scientific keys |
| **Activity Monitor** | Processes, CPU, memory, disks, network, GPU, displays and sensors, live |
| **Passwords** | Logins, passkeys and secure notes, encrypted, synced with 1Password if you like; the SSH agent and `op` |
| **Weather** | Every detail of the forecast for your places, and an assistant given all of it |
| **Speed Test** | What the connection carries each way, and everything that can be read of it as it runs |
| **Screenshot** | Pictures and recordings of the screen, a window or an area |
| **DriveSetup** and **DiskUsage** | Disks, partitions and encryption; where the space went |
| **DiskBackup** | Continuous, encrypted, versioned backups of the disk to the cloud |
| **OS Updates** | Fedora's updates, in Olo |
| **Log Viewer** | Every program's log, searchable and live |
| **Olo Doctor** | Finds what gets in Olo's way and fixes it with a button |
| **Preferences** | One panel per area, from appearance to security |
| **Agent Screens** | Headless Olo screens for AI agents, which you can watch and take over |
| **Token Pool** | Several Claude and ChatGPT subscriptions shared by coding agents |
| **Kache** | The build cache, watched and run from Olo: its stats, charts and settings |
| **Telegram** and **Slack** | Native chat, and Slack's web app as an Olo app |

<table>
  <tr>
    <td width="50%"><img src="docs/img/olo-viewer-telegram.png" alt="Viewer and Telegram"></td>
    <td width="50%"><img src="docs/img/olo-terminal.png" alt="Terminal with tabs"></td>
  </tr>
  <tr>
    <td align="center">The Viewer and Telegram</td>
    <td align="center">Terminal</td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/img/olo-styled-edit.png" alt="StyledEdit"></td>
    <td width="50%"><img src="docs/img/olo-activity.png" alt="Activity Monitor"></td>
  </tr>
  <tr>
    <td align="center">StyledEdit</td>
    <td align="center">Activity Monitor</td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/img/olo-weather.png" alt="Weather"></td>
    <td width="50%"><img src="docs/img/olo-doctor.png" alt="Olo Doctor and the Speed Test"></td>
  </tr>
  <tr>
    <td align="center">Weather</td>
    <td align="center">Olo Doctor and the Speed Test</td>
  </tr>
</table>

Olo also replaces what a desktop needs from GNOME with its own: portals,
the keyring (opened by your login password) and Secret Service, dconf
for GTK programs, notifications, the polkit agent, AirPods and voice
processing for every microphone. Programs from other toolkits (Chrome,
GTK, Qt, Electron) run as Wayland clients and follow Olo's colours.

## Made for agents too

What you see, an agent can read: every window's widgets as structured
data, every menu command by name, and a documented API to drive them,
without moving your pointer or taking your focus. Agents get headless
screens of their own (`olo-screen`), whole Olo desktops that you can
watch live, scrub back through and take over, with what each did and
when. Coding agents in Terminal keep their tab's title current and know
which repository each tab group is for, and Token Pool shares several
Claude and ChatGPT subscriptions among them with no gap across upgrades.

```sh
olo-screen start                                   # a screen of the agent's own
olo-screen run agent olo-styled-edit notes.md
olo-screen tree agent --role Button                # what's on it, as structure
olo-screen click agent --role Button --name Save
```

![Agent Screens: an agent's own desktop, live, beside yours](docs/img/olo-agent-screens.png)

## Wallpapers

Olo's own abstract collection in Olo blue and Olo yellow (Orbit, Halo,
Ribbons, Shapes, Glass and Dunes, most with a version for dark mode),
beside the classic systems' own, redrawn for 4K: Mac OS 9 and X,
AmigaOS, Windows 3.1 to 2000, OS/2 Warp and CDE, their patterns tiled
as they were. Every one is lossless, without the bands and blotches
image models leave.

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
