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
- **Every tiling layout, out of the box.** Every window that tiles (all
  but the floating-only: alerts, dialogs, a window with a parent, one
  of a fixed size or tagged floating) adapts and renders right zoomed,
  in a vertical half and a horizontal half, a quarter, a vertical third
  and a horizontal third, on the smallest screen and on the person's:
  nothing overlapping, cut or crowded, the essentials there (a side
  panel gives way, a toolbar folds), and its least size fits each of
  them. Review each app you change in every layout
  (`tools/dev/ui-lint --tiles`, which tiles each window into each and
  reviews it there; picture the ones it changes), and improve the kit
  where an app needs help to adapt (Ben, 2026-11-19: Calendar broke in a
  quarter). See "Tiling layouts" in HACKING.md.
- **Measure, don't guess.** Run `tsession review` on each kit app at
  every scale, and fix what it finds. A finding at only one scale is a
  real bug until shown otherwise. The review doesn't see app_server's own
  drawing (frames, tabs, overlays), so zoom in and measure that, with the
  window at an odd x. Before changing something global (hinting, a kit
  size), compare before and after to show it changes what you meant.
  Before telling the person how something works (how to upgrade, what a
  command does), read the code that does it.
- **Look at everything you change, as a designer would, before it's
  done.** Picture every window and state a change touches (headless,
  `tsession shot`; in light and dark, at 1× and 2× at least) and judge
  each picture closely, zoomed in: spacing (nothing set right against
  what's above or beside it, gaps that match where they should),
  alignment (edges, baselines, columns), text cut or crowded, every
  state (empty, full, an error, a long name), and how it sits with the
  rest of the window. The lint and `tsession review` find overlaps and
  overflows, not this: a status line set right under its paragraph
  passed both on every Integrations tab and reached the person
  (2026-11-18: "you should be visually reviewing everything you do").
  Check the picture shows what you meant to show (the right tab, the
  window in front) before judging it.
- **Two more pairs of eyes on every picture: Astra 6.2's and Opus
  5.5's.** Give all the pictures of a change to both for a design
  review before it's committed (Ben, 2026-11-19, of a new app's first
  pictures: "omg so much is ugly make sure you get visual design reviews
  from opus 5.5 and astra 6.2"):
  `tools/dev/codex-look --about "WHAT CHANGED" PICTURE.png…`
  (gpt-6.2-astra at medium effort, looking only: what's wrong by a
  pixel) and `tools/dev/opus-look --about "WHAT IT IS, WHAT CHANGED"
  PICTURE.png…` (Claude Opus 5.5, looking only: the design itself, what
  looks amateur or unfinished). Whole windows and enlarged crops, each
  state, look and scale. Something new (an app, a window, a widget)
  goes to both early, before it's built out, and again when it's
  done. Check each thing they find against the picture yourself, fix
  what's real, and say in the commit what each found and what you did
  about it.
- **Leave the person's session alone.** Never reload, upgrade or install
  into it unless asked. To get work ready, run
  `cargo build --release --workspace`. The running session watches
  `target/release` and offers the new build in Deskbar's tray, whose
  Upgrade installs it and reloads in place. `tools/install-session.sh` is
  only for a first install, or when the login-screen entry or udev rules
  change.
- **Build what you change, in dev, while you work.** While editing,
  build and test only the packages you change, in the dev profile
  (optimized too): `cargo build -p`, `cargo test -p`, pictures from dev
  builds (`tsession`'s default), the live lint with `--dev` (with
  `cargo build -p app_server -p olo-preview -p olo-fakesound` beside
  what it lints). The
  release build of the workspace is for work that's ready: build it
  once, when you finish, so the person's session offers the upgrade
  (the pre-push hook builds it too). Measure on release builds of the
  packages measured (`cargo build --release -p`). Release builds were
  49.1 of the 52 compiler-hours kache spent on misses (measured,
  2026-11-19): a change to olo-kit-base rebuilds every app. (A word in
  a catalog no longer does: the catalogs are data, `data/words`,
  2026-11-20.)
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
- **Subagents: Codex, narrow, watched, reviewed.** Do the work yourself,
  in this session, but for three kinds of help, all Codex but the design
  review (never a Claude fork, nor the Agent tool):
  - Narrow, targeted work items that can go on in parallel: Codex
    subagents (gpt-6.2-astra at xhigh), each interactive in a tmux
    session of its own, started, watched and steered with
    `tools/dev/agents` (the `olo-codex-agents` skill). Up to five at
    once, all in this working tree, each in one crate that no other
    agent, nor you, is changing meanwhile, so none clobbers another.
    Each gets a clear brief: the task, its crate, what done is, how to
    test it. They never start agents themselves (the tooling refuses,
    and their brief says so). While any runs, keep `tools/dev/agents
    watch` going in the background, started again after each time it
    returns, and look at `agents list` now and then: answer approvals
    and questions promptly, steer what drifts, unstick what stalls. When
    one finishes, review its work as your own before anything is
    committed: read the whole diff, run its tests, picture every visual
    change and judge it closely (Codex is weak at visual design: expect
    to redo some), then commit it yourself (2026-11-19).
  - A second investigator for debugging and root-causing, with the
    `olo-debug-help` skill (`tools/dev/codex-help BRIEF.md`: a headless
    Codex at xhigh, approving its own steps), started as
    soon as a bug's cause isn't plain (below), while you keep working
    on your own line: two brains are better than one. It looks and
    reports; you check what it found and do the work.
  - `tools/dev/codex-look`, the second look at every change's pictures
    (above); and, the one Claude among them, `tools/dev/opus-look`, Opus
    5.5's design review of the same pictures (the Claude CLI headless,
    reading pictures and nothing else).
- **Astra 6.2 at xhigh, never max.** Wherever Astra reasons for Olo (a
  subagent, the second investigator, a review of a design or a build),
  its effort is xhigh (Ben, 2026-11-20: "whenever you use astra 6.2 it
  should be on xhigh not max"; where his earlier words here say "astra
  6.2 max", read xhigh). The picture look stays at medium: it only
  looks.
- **A bug you can't see at once gets a second investigator at once.**
  If a quick first look (the logs, the code on its path: minutes, not
  an hour) hasn't shown you the cause, start the `olo-debug-help`
  helper right then, before your own theories: don't wait to be stuck,
  to have circled, or to have ruled things out first (say in the brief
  what you've seen so far, however little). Then keep digging on your
  own line while it works, and check what it brings back. A flaky
  test, a hang, a race, anything spanning programs or only on the
  person's session: always. It found in an hour what a day of reading
  hadn't (2026-11-18); "if you're stuck or having trouble ask astra
  6.2 for help" (2026-11-19).
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
