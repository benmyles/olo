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
- **Design lessons baked in before Round 1 (don't repeat review mistakes).**
  Apply these up front on every UI you design or change so reviews don't
  spend rounds re-finding them (Ben, 2026-11-25):
  - *Colour role purity:* Never reuse an identity colour (provider,
    series, group) for unrelated semantic roles (status rings, quota
    bars, warnings). Neutral meters/progress bars use neutral slate
    (`#5E6E84` light / `#8C9DB3` dark); warning fills use true amber
    (`#E3A008`) paired with warning/error ink on the figure itself (never
    bar colour alone); in-progress status text uses primary/secondary
    ink (never green, which belongs to health dots); in-progress marks
    use a neutral ¾-arc spinner + left-anchored diagonal barber-pole bar
    (never a hollow "off" circle or full-width floating quota-bar block);
    small 14–16 pt list icons use neutral slate light/dark variants so
    multi-colour icons never collide with status or provider colours.
  - *Stable section order, problems first inside, stated once:* Keep
    named sections in fixed canonical order (never swap whole sections
    when one item breaks); float problem rows to the top *within* their
    section (or in a flat list) so broken items surface immediately.
    State each error once on its row (`Name · Status` in error red,
    secondary grey reason below, inline action button on the row); use
    BeOS alert construction (32 pt `stop`/`warning` icon, bold title,
    clear consequence, right-aligned action buttons, neutral lifted dark
    panel fill with 1 pt accent border — never khaki or pink web strips)
    for app-wide errors and inline confirmations.
  - *Quiet healthy rows, crisp hierarchy & uniform rhythm:* Healthy rows
    get zero permanent action buttons (actions live in menus, context
    menus, double-click, and footer). Use one consistent heading style
    across sections, ensure top-level section gaps unmistakably exceed
    internal item spacing (never use an inner divider that looks stronger
    than the section break), keep one uniform title-line height/pitch
    across rows with and without buttons, omit repetitive row icons when
    every row has the same icon, and align row sub-lines flush with the
    item name.
  - *Aligned columns, crisp charts, proper empty states:* Put primary
    numbers (`42%`) in a fixed right-aligned column in primary ink and
    left-align secondary text after them; cap right-aligned `Status`
    columns compactly so variable-width columns (`Model`, `Name`) absorb
    extra table width; frame tables with BeOS sunken wells matching
    charts and scroll lists. In `Graph`, use bare numeric Y-axis labels
    with tight headroom (`y_labels(true)`), keep X-axis clock ticks in an
    inside-well band culled by logical-point stride (`x_labels(…)`),
    auto-switch to a clean sparkline when plot height is too short, avoid
    muddy opaque fills, and put live counts right-aligned on the table
    heading instead of decorative swatch sections. Centre empty states at
    ~⅓ well height with a 48 pt icon, bold title, concise guidance,
    default primary footer button, and disabled inapplicable actions.
  - *Consistent footers & width-driven tiling:* Use the same button order
    and noun-qualified labels across tabs (`Add …`, `Edit/Remove …` …
    `Updated/Checked HH:MM`, global action), disabling `Remove …` while
    its inline confirmation is open. Use ≥20 pt gutters in two-column
    tiling layouts with aligned headings and well tops, drop table
    columns strictly by available width, drop whole trailing segments
    rather than cutting mid-word, and size labeled `MenuField` popups to
    fit their longest option without ellipsis.
  - *Complete, mathematically coherent review batches:* Test charts with
    organic non-periodic data whose last-hour totals, today totals,
    failure counts, and host statuses agree 100% across every tab, and
    include light/dark empty lists, error/stopped states across tabs,
    disabled controls, and dark-mode menus/confirmations in the batch.
- **Two more pairs of eyes on every picture: Gemini 4 Neon's and Opus
  5.5's.** Give all the pictures of a change to both for a design
  review before it's committed (Ben, 2026-11-19, of a new app's first
  pictures: "omg so much is ugly make sure you get visual design reviews
  from opus 5.5 and astra 6.2"; 2026-11-25: "everything should be
  either opus 5.5 max or neon 4 xhigh"):
  `tools/dev/agy-look --about "WHAT CHANGED" PICTURE.png…`
  (Gemini 4 Neon at xhigh, in the Antigravity CLI: what's wrong by a
  pixel) and `tools/dev/opus-look --about "WHAT IT IS, WHAT CHANGED"
  PICTURE.png…` (Claude Opus 5.5 at max: the design itself, what looks
  amateur or unfinished). Each looks in a tmux session of its own: run both in
  the background, read what each is doing as it goes (`tools/dev/agents
  peek`), and take each one's end: its review written, or a question
  for you. Whole windows and enlarged crops, each state, look and
  scale. Something new (an app, a window, a widget) goes to both early,
  before it's built out, and again when it's done. Check each thing
  they find against the picture yourself, fix what's real, and say in
  the commit what each found and what you did about it.
- **Three rounds of looks at most, each a whole batch.** (Ben,
  2026-11-23, after one button's change took six rounds: "all these
  reviewer rounds are getting kinda crazy and taking a very long time.
  i think you need to batch more stuff each time instead of giving
  them super granular things to review and also we need to cap the
  number of rounds at maybe 3 or something".) A change gets at most
  three rounds: an early one where it is new, one when it is built,
  and a third only to confirm what the second said must be fixed.
  Each round is everything at once: every state, both looks, every
  scale (1.25× and 1.75× too where anything is sized in rows or
  pixels), and several small changes together rather than one each.
  Before a round, do your own designer's pass and measure the
  pictures, so the reviewers aren't spent on what you could have
  seen. The brief says what changed and asks for what is wrong in
  that, most important first; what they find elsewhere in the window,
  there before the change, goes to docs/TODO.md with their words, not
  into this change (but for a real defect that is a line to fix). A
  fix is confirmed by measuring its picture yourself, not by another
  round. After the third round what is left goes to the TODO, and
  the change is committed.
- **Leave the person's session alone.** Never reload, upgrade or install
  into it unless asked. Work reaches it as a release (below): their
  session asks GitHub for Olo's releases and offers a newer one in
  Deskbar's tray, whose Upgrade installs it and reloads in place. (Where
  Olo is built on the person's own computer, its session also watches
  `target/release` and offers the build.) `tools/install-session.sh` is
  only for a first install, or when the login-screen entry or udev rules
  change.
- **Build with Jato.** Every build and test goes through `jato`
  (`jato build -p`, `jato test -p`, `jato build --release`): it keeps
  the builds it has made (Ben, 2026-11-24: "for builds moving forward
  please use jato"). Jato is experimental and ours: where it gets in
  the way, fix it in `~/src/jato` and build it again ("nobody else is
  working on it so it's yours to own too"), never work around it.
- **A release after each feature, published in the background.** When
  a major piece of work is in, make a GitHub release of it (Ben,
  2026-11-24: "after each major piece of functionality/feature
  implemented please make a new release on github"), with
  `tools/publish-release.sh NOTES.md`, always run in the background
  ("make sure you always run it in the background so you can move on
  with work instead of waiting for a slow upload"): it builds in a
  Fedora 44 container, checks and publishes (HACKING, "The
  Installer"). Its log says how it went; go on with the next piece
  meanwhile, and read its last line when it ends. GitHub keeps the
  latest release and the two before it: the script deletes the older
  ones once the new one is up (`tools/release-prune.sh`; Ben,
  2026-11-26: "for the releases in github let's just keep latest + 2
  previous ones").
- **Build what you change, in dev, while you work.** While editing,
  build and test only the packages you change, in the dev profile
  (optimized too): `jato build -p`, `jato test -p`, pictures from dev
  builds (`tsession`'s default), the live lint with `--dev` (with
  `jato build -p app_server -p olo-preview -p olo-fakesound` beside
  what it lints). The
  release build of the workspace is for work that's ready: build it
  once, when you finish, so the person's session offers the upgrade
  (the pre-push hook builds it too). Measure on release builds of the
  packages measured (`jato build --release -p`). Release builds were
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
- **A work list beside you.** At the start of every session, open a
  pane to the right of yours with the list of the work (the
  `olo-worklist` skill: `tools/dev/worklist open`), one short line an
  item, and keep it current as you go: an item started, checked off
  when done, added the moment the person asks for more, dropped or
  blocked (Ben, 2026-11-24: "keep in there for me an up to date todo
  list of the work (just a brief 1 line entry for each one and check
  them off as you go"; "a rule asking you to do it by default on new
  sessions"). Keep it text-only — never embed screenshots (`@img`) in
  the right-pane progress board (Ben, 2026-11-26: "remove the
  screenshots from the right pane progress board for me please they're
  too intrusive").
- **Helpers are Rex's own subagents: child tabs of yours, through `olo
  agent`.** (Ben, 2026-11-26: "when you use subagents for review/qa
  please use the new olo/rex native subagent support you built"; "it
  should all be available to you as skills if not you need to add
  them".) Every helper below (a worker, the second investigator, both
  reviewers) is started by `tools/dev/agents` and its front doors as a
  child tab of the tab you are in (`olo agent start`), read, answered
  and stopped through `olo agent` (the `olo-agents` and `olo-rex-bridge`
  skills): the person sees each under your tab in Rex, with what it is
  doing, and can read and answer it there. A tmux session only where
  you aren't in a Rex pane (no `REX_BLOCK`) or the host's `olo agent`
  doesn't answer; what the rules below say of tmux is of that case, and
  the rest of them (never a run that goes silent, the minder, what is
  never approved, stopping each when done) holds either way.
  **A watch is going whenever a child runs** (Ben, the same day: "make
  sure you have an olo agents watch going whenever you're running olo
  agents"): from the first child's start to the last one's end,
  `tools/dev/agents watch` (or the front door that started it, or the
  minder's) waits in the background and is started again each time it
  returns; for children you start with `olo agent start` yourself, `olo
  agent watch --all --since CURSOR --timeout 90` the same way. It
  returns the moment a child asks, comes to rest, fails or ends, and
  polls nothing. Never children running with no watch on them, and
  never two watches on the same ones.
- **Subagents: Antigravity, narrow, watched, reviewed.** Do
  the work yourself, in this session, but for three kinds of help, all
  Gemini 4 Neon at xhigh in the Antigravity CLI (`agy`) but Opus's
  design review (never Codex nor Astra: Ben, 2026-11-22, "stop using codex/astra for
  olo qa/dev tasks and everywhere we use codex/astra at the moment for
  olo dev/qa replace it with antigravity (`agy`) and gemini 4 neon on
  xhigh"; never a Claude fork, and the Agent tool only for the minder,
  below). That is for the
  help Olo's development gets: Olo's own support for Codex and Astra
  (Terminal, Rex, the pool, Preferences) is the product's and stays.
  - Narrow, targeted work items that can go on in parallel: subagents,
    each an interactive `agy` in a tmux session of its own, started,
    watched and steered with `tools/dev/agents` (the `olo-agents`
    skill). Up to five at once, all in this working tree, each in one
    crate that no other agent, nor you, is changing meanwhile, so none
    clobbers another. Each gets a clear brief: the task, its crate,
    what done is, how to test it. They never start agents themselves
    (the tooling refuses, and their brief says so). When one finishes,
    review its work as your own before anything is committed: read the
    whole diff, run its tests, picture every visual change and judge it
    closely (expect to redo some of what a subagent designs), then
    commit it yourself (2026-11-19).
  - A second investigator for debugging and root-causing, with the
    `olo-debug-help` skill (`tools/dev/agy-help BRIEF.md`: an `agy` in
    a tmux session, asking leave for what isn't routine), started as
    soon as a bug's cause isn't plain (below), while you keep working
    on your own line: two brains are better than one. It looks and
    reports; you check what it found and do the work.
  - `tools/dev/agy-look`, the second look at every change's pictures
    (above): Gemini 4 Neon in the Antigravity CLI, in a tmux session,
    on copies of the pictures; and, the one Claude among them,
    `tools/dev/opus-look`, Opus 5.5's design review of the same
    pictures (the Claude CLI in a tmux session, with the tools to read
    and write files in a folder of the pictures' copies and no other).
- **Every helper on a screen of its own, never a script that goes
  silent.** A subagent, the investigator and both reviewers each run
  their program's own interactive screen in a child tab of yours (a
  tmux session where there is no Rex pane: above)
  (`tools/dev/agents`; `agy-help`, `agy-look` and `opus-look` are its
  front doors), so what each is doing can be read at any moment
  (`agents peek NAME`), it can be told more (`agents say`), and what it
  asks leave for reaches you (`agents approve`). Never `agy --print`,
  `claude -p` or any other run that answers only at its end (Ben,
  2026-11-22: "always do it through tmux never a script that just goes
  silent … for all the subagents/reviewers etc … you can add tooling to
  launch and work with them efficiently but they should use tmux"; a
  headless look had sat eight minutes with nothing to show whether it
  was thinking or stuck). While any runs, something of yours waits on
  it in the background: `tools/dev/agents watch`, or the front door
  that started it; each returns the moment an agent asks, finishes or
  stalls, and prints a line a minute meanwhile. Start it again after
  each answer, and look at `agents list` now and then: answer
  questions promptly, steer what drifts, unstick what stalls. What is
  routine goes ahead without you (reading, searching, building,
  testing: `tools/dev/agents allowed`, given to that run alone, the
  person's own agy settings never touched); what an agent's rules
  forbid is refused outright; only the rest comes to you. Never
  approve a push, a commit, anything on the person's session, deletion
  outside its scope, or sudo.
- **A minder answers the helpers, so none waits on you.** Whenever a
  helper runs for more than a moment, start the minder with it: one
  subagent of the Agent tool (general-purpose, model `opus`, effort
  max), told to read `.claude/skills/olo-agents/minder.md` and
  given the commands to start and the agents to mind (Ben, 2026-11-22,
  when a helper's question had waited 66 seconds on a busy lead: "i
  think you're not getting the approval requests or not responding to
  them quick enough … launch a … subagent whose only job is to launch
  and manage the tmux sessions for these tmux subagents so that they
  get their responses quickly"; 2026-11-25: "NOTHING should still be
  using fable -- everything should be either opus 5.5 max or neon 4
  xhigh"). It starts the
  tmux agents, waits on `tools/dev/agents watch`, says yes or no at
  once to what its written lists cover, and hands back to you what
  they don't, every question about the work itself, and each result's
  path. It does none of the work and starts nothing else. It is the
  one use of the Agent tool here; its report is checked like any
  helper's (read the results yourself), and what it says went wrong
  with the tooling is fixed in the tooling.
- **Stop each helper when you have what you need of it.** Once an
  agent's result is read and nothing more is to be asked of it,
  `tools/dev/agents stop NAME` (its transcript is kept in its folder,
  and any test session it left is stopped), then `forget` it when its
  work is in; a look ends by itself when its review is written. Nothing
  of yours is left running or waiting at a prompt: before you finish a
  piece of work, `tools/dev/agents list` and `tools/dev/tsession list`
  show nothing of it (Ben, 2026-11-22: "make sure that the tmux agents
  / screens are getting properly stopped when they are no longer
  needed"). The tool tidies behind you all the same (an agent whose
  program exited, one done and untouched for half an hour, one idle
  for three hours: `agents tidy`, which `list`, `watch` and `start`
  run), but that is the net, not the way.
- **Opus 5.5 at max, Gemini 4 Neon at xhigh: nothing else.** Every
  helper is one of the two (Ben, 2026-11-25: "NOTHING should still be
  using fable -- everything should be either opus 5.5 max or neon 4
  xhigh"): wherever Gemini reasons for Olo (a subagent, the second
  investigator, the look at pictures) it is Gemini 4 Neon at xhigh
  (`gemini-4-neon-xhigh`); Opus 5.5's design review and the minder run
  at max. No Fable, no Astra, no other model, at no other effort.
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
  6.2 for help" (2026-11-19; the helper is Gemini 4 Neon now).
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
