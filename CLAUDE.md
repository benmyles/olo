# Olo: rules for agents

Read [docs/HACKING.md](docs/HACKING.md) before changing anything. It is the
source of truth for layout, the kit, look-and-feel rules and testing. These
rules are universal and non-negotiable:

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
  message; it isn't done until it is. See "Fast and light" in
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
  app could use, add it to `crates/ui` and document it in HACKING.md.
- **Survive in-place reload.** Apps reconnect when app_server restarts and
  re-exec after an upgrade; implement `olo_ui::on_upgrade` /
  `olo_ui::restored_state()` so windows come back as they were. Test with
  `tsession start NAME --session` + rebuild + `tsession reload NAME`.
- **Icons** follow [docs/ICONS.md](docs/ICONS.md) (sunburst at max quality,
  against the reference set).
- **Test headless** with `tools/dev/tsession`; never open windows on the
  user's display. Journal visible changes with `tools/journal.py`. Run
  both with `/usr/bin/python3`: the `python3` on PATH (linuxbrew) has no
  PIL, so shots and journal entries fail under it. Your shell has the
  person's `WAYLAND_DISPLAY`, so a GUI program you run from it opens on
  their screen: `olo-logs` prints only with `--print` as its first
  argument (else, even with `--help`, Logs opens).
- **Scratch on disk, in `/var/tmp`, never `/tmp`.** `/tmp` is RAM
  (tmpfs), so whatever goes there fills the person's memory: sandbox
  HOMEs, a `CARGO_TARGET_DIR`, copies of builds (a release binary is
  hundreds of MB), screenshots. Use a directory of your own under
  `/var/tmp` (copy builds with `cp --reflink=always`, free there) and
  remove it when you're done.
- **No forks or subagents.** Do all the work yourself, in this session:
  never spawn a fork, subagent or background agent for any part of a task.
