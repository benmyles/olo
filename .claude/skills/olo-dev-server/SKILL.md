---
name: olo-dev-server
description: Set up a Linux server (or VM) for Olo development and work on it - what to install, Jato and the pinned nightly, test desktops drawn in software with no GPU, using every processor for builds, tests and pictures (rigs and sessions at once), building and publishing a release from a Fedora container in the background, the helpers' first-time sign-ins, and reaching the person's own Olo desktop through Rex. Use when work moves to a new machine, when a dev server is missing something, or when you need to remember how builds, pictures and releases are done there.
---

# Olo on a dev server

Olo is developed on a Linux server the person reaches through Rex, as
well as on their own Olo computer (Ben, 2026-11-24: "just make sure
it's all documented in skills so that if you move to another server
you've got all that knowledge still"). This is what such a machine
needs and how the work is done there. What is learned about one
machine in particular goes in `docs/AGENT-NOTES.md` ("The dev server");
keep this skill and that section current together.

## What it needs

The first server was Ubuntu 26.04 in a VM: 32 processors, 125 GB, no
GPU (QEMU's display only); the second Fedora 44, 64 processors, 251 GB,
btrfs, no GPU either (on Fedora `tools/setup.sh` installs what's
needed, and `cp --reflink=always` of a build is free). `setup.sh` is Fedora's (dnf); on another
system install the same by its own packages:

- **Rust**: `rustup` (the tree's `rust-toolchain.toml` pins a nightly
  by date, with the musl targets; rustup fetches it on first build).
- **Jato** (every build and test goes through it, CLAUDE.md): its
  source is `~/src/jato`; `cargo install --locked --path .` there, or
  a copy of its binary in `~/.local/bin`. It is Olo's to fix where it
  gets in the way.
- **Graphics in software**: Mesa's Vulkan (`mesa-vulkan-drivers`:
  lavapipe) and GL (llvmpipe). `tsession` sets the rest
  (`OLO_HEADLESS_RENDERER=software`, `LIBGL_ALWAYS_SOFTWARE`,
  `GALLIUM_DRIVER=llvmpipe`): HACKING, "Test desktops draw in software".
- **Pictures and the journal**: `/usr/bin/python3` with PIL
  (`python3-pil`); Olo's fonts (`fonts-urw-base35`, `fonts-noto-core`,
  `-mono`, `-cjk`, `-extra`, `fonts-liberation`,
  `fonts-jetbrains-mono` on Ubuntu).
- **zig**, **tmux**, **gh** (the person signs it in), **docker** (for
  releases; without the docker group, `sudo docker`, which the publish
  tool does by itself).
- **The helpers' programs**: `claude`, `codex`, `agy`, each signed in
  by the person, never by you (below).

Then, in the clone: `tools/hooks/install` (the pre-commit layout lint
and the pre-push release build and lints, through Jato), and check a
test desktop draws: `/usr/bin/python3 tools/dev/tsession start
NAME`, `tsession run NAME …`, `tsession shot NAME out.png`, `tsession
stop NAME`.

Scratch goes in `/var/tmp` (a directory of your own), never `/tmp`:
`/tmp` is memory there too.

## Builds and tests

As anywhere (CLAUDE.md): `jato build -p PKG`, `jato test -p PKG` for
what you change, in dev; `jato build --release -p PKG` only to
measure; the workspace's release build is the pre-push hook's. The
hook builds and lints the pushed commit in a checkout of its own
(`/var/tmp/olo-prepush/tree`), so work goes on in the tree meanwhile;
its release build there is a ready "before" for measuring the next
change (`target/release` in it).

**Counting instructions**: where `kernel.perf_event_paranoid` is above
2 (Ubuntu's 4), `perf stat` counts only under sudo: `sudo -n perf stat
--no-scale -e instructions:u -p PID -- sleep 10`. Leave the setting as
it is.

## Use every processor

Ben, 2026-11-24: "try to reduce the dev loop time as much as possible
and take advantage of the hardware". Headless sessions are cheap here:

- **Pictures one session per look and scale, all at once.** A picture
  script takes its job from its arguments (`script.py win-dark-1.75`)
  and runs in a rig or session of its own; for the Rex rig,
  `tools/dev/rex-slow/at_once.py SCRIPT JOB…` gives each job its own
  `REX_SLOW_DIR` and `REX_SLOW_NAME` and prints each one's output.
  Fourteen pictures took 13 s so, seven minutes one after another.
  For other apps, one `tsession` per job, each its own name.
- **Crops, sheets and masks in a process pool**
  (`concurrent.futures.ProcessPoolExecutor`), not a loop.
- **Checks with several cases run them at once** (`try.py agents`: the
  three agents' stand-ins in 25 s, from about 90).
- `tools/dev/ui-lint --jobs N` and `tsession review` per scale run side
  by side; the test suites are already parallel.

## The README's and the site's pictures

`tools/dev/readme-shots [SCENE…]` takes them on an Agent Screen of its
own (a whole desktop at 3840 × 2160, drawn in software where there is
no GPU), from a release's own build rather than a second workspace
build:

    mkdir /var/tmp/YOURS-build
    (cd /var/tmp/olo-release/target/release && for f in *; do [ -f "$f" ] && [ -x "$f" ] && cp --reflink=always "$f" /var/tmp/YOURS-build/; done)
    cp target/debug/olo-WHAT-YOU-CHANGED /var/tmp/YOURS-build/     # dev builds laid over
    OLO_SHOTS_BUILD=/var/tmp/YOURS-build OLO_SHOTS_OUT=/var/tmp/YOURS-out /usr/bin/python3 tools/dev/readme-shots desktop

Look at each picture in `OLO_SHOTS_OUT` before it replaces one in
`docs/img`. The tool keeps the computer's name off the desktop
(`OLO_TRACKER_DEMO_VOLUME`) and the host's Rex server and agents
service out of reach; check each picture for anything else of the
machine's all the same (a shell prompt, a path, a process list).

## Releases

After each major piece of work (CLAUDE.md), from a commit on
origin/main, always in the background:

    tools/publish-release.sh NOTES.md            # --dry-run first if unsure

It builds the workspace in release in a Fedora 44 container
(`tools/release-image/Dockerfile`, its own target folder and Jato cache
under `/var/tmp/olo-release`), makes the five files
(`tools/release.sh`), unpacks them as Setup does, checks every
program's libraries on Fedora 44 and 45, names the release (vDATE,
vDATE.N for a second that day), publishes it at HEAD's full hash and
checks each file's size on GitHub. Its log is
`/var/tmp/olo-release/publish.log`; read its last line when it ends.
The notes follow the last release's ("What's new", "What's fixed",
"Files"; `VERSION` is put in). The person's session finds the release
on GitHub by itself and offers it in Deskbar's tray (two minutes after
it starts, then every six hours: `crates/session/src/upgrade.rs`).

## The helpers' sign-ins

- **Codex** (Astra 6.2's code review of every change,
  `tools/dev/astra-review`; it is also one of the agents Olo supports,
  and its live tests run it): `codex login status`. Not signed in, the
  review refuses at once and says so; ask the person to run `codex
  login --device-auth` (a `!` command in their prompt). Never sign in
  for them, never open a sign-in link. Which models it offers and the
  efforts each takes are in `~/.codex/models_cache.json` (Astra 6.2 is
  `gpt-6.2-astra`: low to max, and ultra).
- **Claude Code** (Opus's looks) asks once per computer whether to
  trust a folder; `agents watch` answers it for a look's own folder.
- **agy** (subagents, the second investigator): its front doors show
  its sign-in screen if it needs one; the same, the person's.
- agy's banner prints the account signed in: never capture an agent's
  screen by hand into anything kept or sent.

## The person's desktop, from here

The server is used through Rex from the person's Olo computer; the
`olo-rex-bridge` skill (installed for the account by Rex) reaches that
desktop for what they have allowed this host: a link opened in their
browser, a secret from their Passwords once they approve it, and an
Agent Screen of your own there (`olo screen start NAME`, `run`, `look`,
`tree`, `stop`). To see a file of theirs they point you at (a
screenshot), open it in Olo's Viewer on your own screen there and look
(`olo screen run NAME olo-viewer PATH`, `olo screen look NAME`); stop
the screen and delete the picture's copy here when done.

## A screen on the server itself

Since 2026-11-26 an agent on a Rex host can have an Agent Screen on the
host, with no desktop asked (HACKING, "Agent Screens of a host's own"):
`olo screen status` says whether Olo is on the server
(`~/.local/share/olo/desktop`), `olo screen install` puts it and Chrome
there (16 s on this server, no root), and `olo screen --on host start
NAME` is a whole Olo desktop drawn in software, Chrome included, driven
with the same commands. It is the released Olo, not the tree's: use it
for what an agent needs a browser or a GUI for (a page of a server here,
a program to drive). Olo's own changes are still tried in test sessions
(`tools/dev/tsession`: the tree's builds, fenced from the host's
services). To try the feature itself without touching the account,
AGENT-NOTES has the way ("A host's own screens, tried without touching
the host"). A screen's pictures name the machine: crop or mask them
before they leave it.

## Privacy, here as anywhere

The server's name, address and accounts never go into the repository,
a commit, a brief or a picture sent to a reviewer: grep the staged diff
and the message for them before each commit, and mask a shell prompt
(it names the machine) in pictures that leave it.
