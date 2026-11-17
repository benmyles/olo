# Olo: rules for agents

Read [docs/HACKING.md](docs/HACKING.md) before changing anything. It is the
source of truth for layout, the kit, look-and-feel rules and testing; the
Olo Book ([docs/OLO-BOOK.md](docs/OLO-BOOK.md)) is how the kits work and
how to build with them; [docs/AGENT-NOTES.md](docs/AGENT-NOTES.md) is
what agents have learned about this computer and its tools. These rules
are universal and non-negotiable:

- **BeOS-inspired, not a BeOS replica.** For everything you design, first
  look at how BeOS did it (the Be Book, in
  [vendor/bebook](vendor/bebook/README.md); Tracker; the kits) and keep
  whatever still makes sense in 2026 for what we're building; where there
  is a better modern way, use that instead.
- **Agent-first, as much as people-first.** Everything should work
  seamlessly for agents as well as for people: what a person can see and
  do, an agent can read (structured, not just pixels) and drive through a
  documented API, without getting in the person's way.
- **Fast and light, measured.** Olo is super fast and lightweight: a core
  tenet, held as strictly as pixel-perfection. No CPU cycle is wasted:
  idle costs nothing (no polling, no frames or events when nothing
  changed), work follows what changed, not what's on screen, and work
  done once is kept. Every change to something that runs per frame, per
  event or while idle is measured on release builds before and after
  (`tools/dev/cost PID`, app_server's `timings`, `tools/dev/bench`,
  `scroll-check`), latency included, with the numbers in the commit
  message; it isn't done until it is. Judge it in absolute terms (a
  frame's work against its budget, 8.3 ms at 120 Hz; CPU at rest and
  while animating; latency), never against another toolkit. Count
  instructions with `perf stat --no-scale` (this machine's two kinds of
  core made scaled counts double at random). See "Fast and light" in
  HACKING.md.
- **Never block the UI.** Anything that can take more than a frame runs on a
  worker thread, shows live progress, streams results and can be cancelled.
  See "The responsiveness rule" in HACKING.md.
- **Pixel-perfect in light, dark and night modes at 1×, 1.5× and 2×.**
  Every time you check visuals, check all three scales
  (`tsession scale NAME 1`, `1.5`, `2`), not only 2×: it must look
  perfect in every one. Colours from the palette, spacing from `metrics`,
  text via `style`. Change the scale while the app runs, since that finds
  caches made for another scale. Where drawing depends on the scale
  (icons, cached sizes, app_server's decorator), look at 1.25× and 1.75×
  too. Nothing drawn from a bitmap is resampled: one texture pixel to a
  device pixel (`icons::paint`, `style::snap_centred`, app_server's
  `render::exact`). Anything measured in pixels is keyed by the scale.
  See "Fractional scaling" in HACKING.md.
- **Windows fit what they show, on every screen.** A window whose content
  sets its size (a form, a panel, a dialog, an alert) follows it: as tall
  as its content needs, never taller than the smallest screen (1280 × 800
  points) less room for its tab, where its Frame scrolls it
  (`WindowOptions::fits_content`; every `alert()` and `dialog()` has
  it). Never wrap a whole window's content in a scroll view of its own,
  and never compute a window's height by hand: a list in it shrinks and
  scrolls, nothing else does. What doesn't fit 1280 × 800 is laid out
  again (side by side, in tabs), not scrolled. The layout lint checks
  every window against that screen; run the live lint at your screen's
  size too (`tools/dev/ui-lint --screen WxH`). See "Windows fit what
  they show" in HACKING.md.
- **Measure, don't guess.** Run `tsession review` on each kit app at
  every scale, and fix what it finds. A finding at only one scale is a
  real bug until shown otherwise. The review doesn't see app_server's own
  drawing (frames, tabs, overlays), so zoom in and measure that, with the
  window at an odd x. Before changing something global (hinting, a kit
  size), compare before and after to show it changes what you meant.
  Before telling the person how something works (how to upgrade, what a
  command does), read the code that does it.
- **Leave the person's session alone.** Never reload, upgrade or install
  into it unless asked. To get work ready, run
  `cargo build --release --workspace`. The running session watches
  `target/release` and offers the new build in Deskbar's tray, whose
  Upgrade installs it and reloads in place. `tools/install-session.sh` is
  only for a first install, or when the login-screen entry or udev rules
  change.
- **Wayland only, no X11.** Olo runs Wayland programs and nothing
  else: no Xwayland, no X11 window management, no `DISPLAY`, no code or
  dependencies for X11-only programs. Programs that can speak either
  (Chrome, GTK, Qt) are run as Wayland clients.
- **Everything native.** Never wrap pre-existing system tools or apps
  (no shelling out to ffmpeg, pdftoppm, xdg-open, unzip, `op`, …). Read
  formats, talk protocols and do the work in Rust, in process (any
  Rust/cargo crate is fine), so the whole desktop is one cohesive,
  consistent experience.
- **Build on the shared kit.** If an app needs a widget or behaviour another
  app could use, add it to `crates/kit` and document it in HACKING.md.
- **GPUI is gone, for good.** Olo's kits (`crates/interface`,
  `crates/kit`) are its own. Never add GPUI back, as a dependency or as
  code, and never compare Olo's performance with it or with the build
  from before the kits (`~/.local/olo-kept/before-kits` was a login
  fallback, never a yardstick; Olo Doctor suggests removing it).
- **Survive in-place reload.** Apps reconnect when app_server restarts and
  re-exec after an upgrade; implement `View::save_state` (and
  `olo_kit::on_upgrade` for several windows) / `olo_kit::restored_state()`
  so windows come back as they were. Test with
  `tsession start NAME --session` + rebuild + `tsession reload NAME`.
- **Icons** follow [docs/ICONS.md](docs/ICONS.md) (sunburst at max quality,
  against the reference set).
- **Test headless** with `tools/dev/tsession`; never open windows on the
  user's display. Journal visible changes with `tools/journal.py`. Run
  both with `/usr/bin/python3`: the `python3` on PATH (linuxbrew) has no
  PIL, so shots and journal entries fail under it. Your shell has the
  person's `WAYLAND_DISPLAY`, so a GUI program you run from it opens on
  their screen: `olo-logs` prints only with `--print` as its first
  argument. A kit program asked `--help` or `--version` refuses without
  a window; any other argument, a wrong one too, is a launch: read the
  program's `main.rs` for its options, don't ask it.
- **Scratch on disk, in `/var/tmp`, never `/tmp`.** `/tmp` is RAM
  (tmpfs), so whatever goes there fills the person's memory: sandbox
  HOMEs, a `CARGO_TARGET_DIR`, copies of builds (a release binary is
  hundreds of MB), screenshots. Use a directory of your own under
  `/var/tmp` (copy builds with `cp --reflink=always`, free there) and
  remove it when you're done.
- **Notes in the repository, not in a memory.** What you learn that the
  code and the docs don't say (a pitfall, a fact about this computer, a
  tool's quirk) goes in docs/AGENT-NOTES.md, in the same commit as the
  work, and what's there is kept current (corrected or removed when it
  stops being true). Never store memories in an agent's own memory
  outside the repository.
- **No forks or subagents.** Do all the work yourself, in this session:
  never spawn a fork, subagent or background agent for any part of a task.
- **Root-cause every problem; never work around it.** Whenever something
  goes wrong (a hang, a crash, a slow step, a flaky test, a tool that
  misbehaves), find why it happened and fix that, so it can't happen
  again: in Olo, in its tools, or in the tool itself (fork it if need
  be). Restarting, retrying, skipping a hook or waiting it out is never
  the fix; if you must do one to keep going, say so and still fix the
  cause.
- **Waits that can't hang.** Never wait with `pgrep -f PATTERN` (or
  `pkill -f`) from a shell whose own command line holds PATTERN: it
  matches itself and the loop never ends (it hung a commit and a clone,
  2026-11-15). Wait on what you started: its PID (`while kill -0 PID`),
  its exit (run it with `run_in_background` and take the completion
  notice; `wait` in the same shell), or a file it writes when done. Every
  wait has a deadline well under the tool's timeout and says what it's
  waiting for when it gives up; a long job runs in the background and is
  looked at, not slept on.
