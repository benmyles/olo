# Olo Desktop

A whole desktop for Linux, in the spirit of BeOS. From the Wayland window
server to two dozen apps, written in Rust on one shared kit. Fast and
light, measured. Made for people and their AI agents alike.

Beta, for Fedora 44 and 45 on x86_64. It installs beside GNOME, which stays
as it is. MIT licence. Source: <https://github.com/benmyles/olo>

New to Olo? The **User's Guide to Olo** walks through the whole desktop in
six chapters, a manual in the manner of the boxed software of the 1990s. On
the page it opens as a book (from the desktop's User's Guide icon, the hero,
or Deskbar; `#manual`, `#manual/p-7` and `#manual/ch-3` open it at a page or
a chapter). For agents it is Markdown: [assets/manual.md](assets/manual.md).

## The desktop

Olo keeps what BeOS got right (the window's yellow tab, Deskbar, Tracker's
spatial folders, a menu bar in every window) and adds what work in 2026
needs: a place for coding agents and the machines they run on, seven tiling
layouts, five looks, and upgrades in place that keep every window open.

- **The tab.** Every window wears its name on a tab, yellow on the one
  you're in. Fan Out (⌃⌘/) sets the tabs side by side.
- **A menu bar in every window**, as in BeOS.
- **Rex**: a coding agent, the agents it started under it, and its
  transcript, here or on a far host.
- **Deskbar**: the Olo menu, the clock with your next events, the tray,
  live meters, and this workspace's windows.
- **Tracker**: spatial folders, Quick Look for pictures, PDFs, video and
  archives, and any host's folders at `rex://`.
- **The command palette** (⌘Space): apps, menu commands, single settings,
  windows, Rex panes, passwords, events, sums; ranked by a small model
  trained on your choices, on the computer.
- **Seven tiling layouts**: Main and Stack, Halves, Columns, Rows, Grid,
  Spiral and Scrolling, per workspace, or floating windows.

## Two readers: everything you can see, an agent can read

Every Olo window is also a tree of named things, each with its place on the
screen. Agents read it and act on it by name, through a documented command,
on a screen of their own (Agent Screens): never by guessing at pixels, and
never with your pointer.

```
$ olo-screen tree agent
window 1 "DeskCalc" (olo.deskcalc) focused
  Window "DeskCalc" at 53,66 266x238
    Button "7" at 69,134 42x34
    …
    Button "=" at 213,254 42x34
    TextInput "Expression" value="1000/120" at 75,82 222x36 focused
$ olo-screen click agent --role Button --name "="
$ olo-screen tree agent --role TextInput
    TextInput "Expression" value="8.33333333333333" at 75,82 222x36
```

1000 ÷ 120 = 8.3: the budget every frame in Olo is measured against, in
milliseconds, at 120 Hz.

The tree reads Olo's widgets, a web page's text and fields, and GTK and Qt
windows alike. You watch any agent's screen live and take it over with ⌘K.
An agent on a remote machine can have a screen there, drawn in software
where there's no GPU, with no root; it costs no processor time at rest.

## Agents

Claude Code, Codex and Antigravity are part of the desktop: each one's state
on its tab, in Deskbar and in a notification that jumps to the right pane.

- **Rex**: local and remote workspaces over SSH or Tailscale. Sessions live
  on the server and outlast a reload, a closed lid and a change of network.
  An agent starts subagents as child tabs under its own; they run on the
  host's agents service and keep working with your laptop shut.

  ```
  $ olo agent start claude \
      --name api-audit \
      --model claude-opus-5-5 \
      --effort max \
      --prompt-file brief.md
  $ olo agent watch --all
  ```

- **Agent Screens**: a full desktop of the agent's own at 120 Hz, with its
  own session bus and Chrome profile. Watch live, scrub back, export an MP4,
  take control.
- **Token Pool**: Claude and ChatGPT subscriptions in one pool for Claude
  Code and Codex, the quota that runs out first spent first; hosts get pools
  of their own.
- **Terminal**: shells that outlive upgrades, tab groups and worktrees, pull
  request badges, Keep It Going and Auto Approve.
- **The bridge**: inside any Rex session, `olo` reaches back to your
  desktop (links, ports, files, folder sync, passwords). Every permission
  starts off, per host.
- **Integrations**: Google, Fastmail, CalDAV, GitHub and Telegram accounts
  in one place; adding a Google account has Claude Code make its Cloud app
  for you, on an Agent Screen you can follow.

## Measured: at rest, Olo does nothing at all

No polling, and no frame when nothing changed. Every change to what runs
each frame, each event or at rest is measured on release builds before and
after, and the numbers go in its commit.

| Before → after | What | Commit |
|---|---|---|
| 4 to 10 → 0 | Frames Calendar drew in ten seconds at rest (now 0.0% of a core, 0 wakeups a second; 0.71 ms a frame stepping through months) | [a5e17ca28](https://github.com/benmyles/olo/commit/a5e17ca28) |
| 1.9% → 0.1% | Of a core, for an idle Agent Screen (90 wakes a second to 8; click to first new frame 37 to 42 ms) | [281482885](https://github.com/benmyles/olo/commit/281482885) |
| 200 → 0 | Frames in ten seconds for a chart drawn again the same (8.60% of a processor to 0.90%) | [6069d2d5f](https://github.com/benmyles/olo/commit/6069d2d5f) |
| 244 → 44 MiB | GPU memory for a window filling a 5K display once it rests 30 s (back in 1.4 to 3.1 ms) | [965bdefd3](https://github.com/benmyles/olo/commit/965bdefd3) |
| 3.9% → ~0 | The command palette at rest, following 36,873 folders | [7c02d4d12](https://github.com/benmyles/olo/commit/7c02d4d12) |

## Apps

Twenty-four apps of its own, plus Preferences and Notifications, all on one
kit. On the page, each opens its Get Info window: its own window, pictured
at its own size on made-up data, and more about it.

- **Rex**: Local and remote workspaces for coding agents, with child tabs, live transcripts and predictive typing over SSH and Tailscale.
- **Terminal**: Shells that outlive upgrades, tab groups and worktrees, pull request badges, Keep It Going and Auto Approve.
- **Agent Screens**: Headless desktops for agents at 120 Hz: watch live, take control, scrub the history, export video.
- **Token Pool**: Claude and ChatGPT subscriptions pooled across Claude Code and Codex, here and on your hosts.
- **Tracker**: The desktop and spatial folders: Icon, List and Column views, Quick Look, archives, and rex:// hosts.
- **StyledEdit**: Styled text, Markdown and code, tables, pictures, spelling and print; .docx, .odt, .rtf and PDF out.
- **Viewer**: Pictures with OCR text you can select, PDFs you can fill and sign, slide decks, and video decoded on the GPU.
- **DeskCalc**: BeOS's desk calculator, with scientific keys. It knows the frame budget.
- **Calendar**: Google, Fastmail and CalDAV in one: day, week, month and agenda, invitations, and drafts that wait for your click.
- **To-Do**: Task lists as .ics: Today, Upcoming, Kanban boards, reminders, and quick entry from the palette.
- **Passwords**: An encrypted vault with passkeys, one-time codes and SSH keys, a drop-in op command, and sharing with agents' screens.
- **Phone**: Your iPhone or Android phone over Bluetooth, nothing installed on it: messages, contacts, calls and music.
- **Telegram**: A Telegram client written in Rust: folders, forums, voice notes, stickers and replies from a notification.
- **Weather**: Forty-four forecast layers, timelines down to the cloud base, and an assistant that marks the hours that suit you.
- **Screenshot**: The screen, a window or a region, a colour picker, and recordings in H.264 or AV1 that take only changed frames.
- **Activity Monitor**: Processes, the supervisor, every engine of every GPU, sensors, fans and battery, with graphs you can click back through.
- **Olo Doctor**: What is wrong with the computer and what could be better, by how much it matters, each with a button that fixes it.
- **OS Updates**: Fedora's updates through DNF5, with advisories and changelogs, offline updates, Undo and version upgrades.
- **Log Viewer**: The systemd journal and agents' screens' logs, live, with a query language and Show in Context.
- **DriveSetup**: Disks and partitions: LUKS, format, resize, write an image and benchmark.
- **DiskUsage**: Where the space went, as a sunburst, with a Collector and an undoable Trash.
- **DiskBackup**: Continuous, versioned, deduplicated and encrypted backups to a folder or Google Drive, with Older Versions in Tracker.
- **Speed Test**: Download, upload and latency under load, with the kernel's own view of the connection.
- **Kache**: A dashboard for Rust's build cache: why each miss missed, by the hour.

Google Chrome and Slack come set up to run as Wayland programs.

## Looks

Five classic looks (Classic, Platinum, BeOS, AmigaOS, Aqua) and twenty-three
colour schemes, in light, dark and night; pixel-exact at 1×, 1.25×, 1.5×,
1.75× and 2×.

Six icon sets, each look's own and one for any look: Classic (after BeOS:
glossy, bright and friendly), Platinum (after Mac OS 9: black outlines,
lilac greys), BeOS (as R5 drew them: flat faces in its 8-bit colours),
AmigaOS (as Workbench drew them: a screen's sixteen pens), Aqua (shinier and
bluer, with soft outlines) and Flat (each thing seen straight on, in plain
faces, no outlines). The colour schemes wear Classic's.

## Install

On a stock Fedora 44 or 45 Workstation (x86_64):

```sh
curl -LO https://github.com/benmyles/olo/releases/latest/download/olo-installer-x86_64
chmod +x olo-installer-x86_64 && ./olo-installer-x86_64
```

Or download the Installer itself:
<https://github.com/benmyles/olo/releases/latest/download/olo-installer-x86_64>.
Agree, then Begin; log out and choose Olo (GNOME stays installed); upgrade
from Deskbar's tray, in place. From source (Fedora 44 or 45, x86_64 or
aarch64):

```sh
git clone https://github.com/benmyles/olo && cd olo && tools/setup.sh
```

Olo is beta software, developed on a few computers (a Lenovo ThinkPad P1
Gen 9 above all). The `olo-hardware` skill has your coding agent make your
computer one Olo knows.

## The User's Guide

*User's Guide to Olo*, First Edition, November 2026: thirty printed pages
with pictures of Olo at their own size. The whole of it, as Markdown:
[assets/manual.md](assets/manual.md); as a page of its own (every page, one
after another): [assets/manual.html](assets/manual.html).

1. **First Steps on Your Olo Desktop** (page 6): the desktop at a glance;
   windows, tabs and menus; the command palette; workspaces, tiling and your
   keyboard.
2. **Files, Folders and Finding Things** (page 10): Tracker and Find; Quick
   Look, Viewer and archives; DiskUsage, DiskBackup and DriveSetup.
3. **Terminals, Remote Machines and Coding Agents** (page 14): Terminal and
   Rex; agents under your tab (`olo agent`, minders, Keep It Going, Auto
   Approve); the `olo` bridge; Agent Screens and Token Pool.
4. **Everyday Tools and Your Phone** (page 18): StyledEdit, Calendar and
   To-Do; Passwords, Telegram and Phone; Weather, Screenshot and the toolbox.
5. **Making Olo Yours** (page 22): looks, colour schemes and icon sets;
   light, dark and night; displays and sound; upgrades, Olo Doctor and Olo
   Rescue.
6. **Quick Reference** (page 26): two shortcut cards, two readers, the
   index and the colophon.

## More

- [The User's Guide to Olo](assets/manual.md): the whole desktop, chapter by chapter
- [The Olo Book](https://github.com/benmyles/olo/blob/main/docs/OLO-BOOK.md): how the kits work
- [Hacking](https://github.com/benmyles/olo/blob/main/docs/HACKING.md)
- [The brand](https://github.com/benmyles/olo/blob/main/docs/BRAND.md)
- [Releases](https://github.com/benmyles/olo/releases)
