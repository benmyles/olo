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
- **Reviewers on every change: Codex Astra 6.2 (`xhigh`), Antigravity
  Gemini 4 Neon (`xhigh`), and Claude Opus 5.5 (`xhigh`) for visual
  design work.** (Ben, 2026-11-27: builders get code reviews from a
  Codex Astra 6.2 on `xhigh` (max 2 rounds) as well as another
  Antigravity Gemini 4 Neon on `xhigh` (max 2 rounds); any change with
  visual design work is also reviewed by Claude Opus 5.5 on `xhigh`, so
  visual changes use all three reviewers while non-visual changes use
  the Astra and Neon reviewers.)
  - **Code review (every change, max 2 rounds each):** run both
    `tools/dev/astra-review --about "WHAT IT IS, WHAT CHANGED" --working
    [PATH…]` (Codex Astra 6.2 at `xhigh`) and `tools/dev/agy-review
    --about "WHAT IT IS, WHAT CHANGED" --working [PATH…]` (Antigravity
    Gemini 4 Neon at `xhigh`), or pass `FROM..TO` for commits. Each
    reads the diff and the tree, runs nothing, changes nothing in the
    repo, and keeps its review with the diff under
    `/var/tmp/olo-reviews`. Check each finding against the code (a
    finding is a claim until reproduced), fix what is real with a test
    that fails before the fix, and note in the commit what each reviewer
    found and what was done. Nothing is released with an unreviewed
    commit.
  - **Visual design review (any change with visual design work, all
    three reviewers):** give all screenshots (whole windows and enlarged
    crops, every state, light and dark, every scale) to
    `tools/dev/agy-look --about "WHAT CHANGED" PICTURE.png…` (Gemini 4
    Neon at `xhigh`: pixel-level spacing, alignment, edges) and
    `tools/dev/opus-look --about "WHAT IT IS, WHAT CHANGED"
    PICTURE.png…` (Claude Opus 5.5 at `xhigh`: design quality,
    hierarchy, rhythm, colour, native feel), alongside the `astra-review`
    and `agy-review` code reviews. Something new (an app, a window, a
    widget) goes to visual review early, before it is built out, and
    again when built. Check each finding against the pictures yourself,
    fix what is real, and record in the commit what each found and what
    was done.
- **Two review rounds max by default; escalate to the lead if critical
  issues persist.** Each builder gets at most two rounds of review from
  each required reviewer, batched whole (every state, every scale
  including 1.25× and 1.75× where anything is sized in rows or pixels,
  and related changes together rather than piecemeal). Before a round,
  do a designer's/engineer's pass and measure the pictures so reviewers
  are not spent on obvious mistakes. What reviewers find elsewhere in a
  window (pre-existing outside the change) goes to `docs/TODO.md` in
  their words. If critical outstanding issues remain after 2 rounds of
  review, the minder escalates to the lead to decide whether the builder
  has become misaligned, whether the design needs rethinking, how to
  steer it, and whether to grant additional review rounds (the lead is
  authorized to grant extra rounds when warranted). Fixes confirmed by
  measurement or tests do not need another round; after the final round
  any non-critical remainder goes to `docs/TODO.md` and the change is
  committed.
- **Leave the person's session alone.** Never reload, upgrade or install
  into it unless asked. Work reaches it as a release (below): their
  session asks GitHub for Olo's releases and offers a newer one in
  Deskbar's tray, whose Upgrade installs it and reloads in place. (Where
  Olo is built on the person's own computer, its session also watches
  `target/release` and offers the build.) `tools/install-session.sh` is
  only for a first install, or when the login-screen entry or udev rules
  change.
- **Leave the host's agents alone too.** Other agents work on this
  host through its agents service (`olo agent`: the ShiftUp lead's
  builders, hours into their work). Nothing built from the tree speaks
  to the live service: tests and pictures have a Rex server and an
  agents folder of their own (every `tsession` is given both), and a
  dialog in a script is put away by its Cancel, never by a key whose
  meaning you haven't read. The same of the host's Token Pool
  (`olo-pool-host`, 127.0.0.1:17891 and 17892, which their requests and
  yours go through): every `tsession` has port 0 for it, and
  `olo-pool`'s demo listens on ports of its own. The service here is replaced only by a
  release's own step (`tools/dev/host-upgrade`), and only after a note
  in their file with the time (`/var/tmp/shiftup/
  olo-agent-needs-answers.md`); `~/.local/state/olo/host-upgrade.hold`
  holds it, theirs to put there or yours. After any replacement, check
  and write there: the service's build, their children, the cursor.
  (2026-11-26: a build of the tree served the live socket with an empty
  registry for 2 min 21 s; their lead: "We cannot watch or reach them.
  Please put it back now … Tell us in this file whenever the service
  here is about to be replaced, before it is." AGENT-NOTES has what
  met and what closed it.)
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
  previous ones"). And it upgrades this computer as a Rex host: the
  release's helper (`olo agent`, the agents service), skill and `olo`
  command are put here as its last step (`tools/dev/host-upgrade`; Ben,
  the same day: "as you build the new releases please upgrade the
  services, skills etc on this rex host automatically for me so that
  the agents working on stuff relying on olo agent fixes etc get it as
  soon as they're ready"). That is this host's own pieces only: the
  desktop's session is still never touched.
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
- **A live standup board beside every subagent (`tools/dev/agent-board`).**
  (Ben, 2026-11-27: every builder, investigator, and minder gets a Rex
  split pane to its right automatically opened by `tools/dev/agents` on
  `start`/`resume`/`investigate`/`minder start` using the absolute
  `tools/dev/agent-board` path so it works in every worktree, and keeps
  it up to date incrementally via `agent-board update …`, supplying only
  what changed.) The board displays a plain-English 1–2 paragraph
  version of the agent's goal/brief, a standup (Recently Did, Doing Now,
  Up Next), a color progress bar with ASCII status art, and its todo
  checklist — driven by `inotify` (0% CPU at rest) and pre-allowed in
  `tools/dev/agents` so subagents update it unasked.
- **Agent hierarchy through `olo agent`: Lead ▸ Minder (`agy` Neon 4
  `high`) ▸ Builders & Reviewers.** (Ben, 2026-11-26 & 2026-11-27: all
  helpers are native Rex subagents via `olo agent` — never the harness
  Agent tool, never headless print scripts, and tmux only when not in a
  Rex pane.)
  - **Lead** (autonomous coordinator): keeps everyone aligned, makes
    architectural decisions, steers workers, reviews and integrates
    finished work, handles escalations from the minder, and authorizes
    additional review rounds when warranted.
  - **Minder** (ALWAYS **`agy` Gemini 4 Neon on `high`**,
    `gemini-4-neon-high` / `--effort high`): a single native `olo agent`
    child tab directly under the lead (`tools/dev/agents minder start`,
    following `.claude/skills/olo-agents/minder.md` and
    `minder-rules.md`). All builders, investigators, and reviewers run
    as native `olo agent` child tabs nested under the minder (`Lead ▸
    minder ▸ subagents`), so the person sees the entire tree cleanly in
    Rex. The minder watches all subagents continuously via `olo agent
    watch`, answers routine approvals and brief-covered questions at
    once, coordinates the 2-round code/visual reviews, and escalates to
    the lead via `olo agent escalate` whenever a builder finishes and
    is ready to land, whenever critical issues remain after 2 review
    rounds, or whenever architectural alignment, risky approvals, or
    blockers need the lead's call. The minder does none of the coding
    or committing itself.
  - **Monitor the minder continuously (never leave approvals or stalls
    to the person).** (Ben, 2026-11-27: "make sure you also monitor the
    minder in case it needs approvals or gets stuck (make this a rule
    too)".) The lead keeps an active background `olo agent watch` on the
    minder from its start to its end (with `--on
    approval,question,escalation,idle,done,error,exit`), restarting it
    immediately after every return. When the minder asks for an
    approval, inspect and answer it at once (approve safe routine minder
    actions; deny anything dangerous or out of scope) so the person
    never has to approve the minder. When a watch times out (`124`) or
    the minder sits `idle`/`done` while children are still active, check
    its status and scrollback (`olo agent status` / `olo agent read`) to
    verify its own background `olo agent watch` is running and prefer
    steering/re-prompting the existing minder tab if it stalled. If a
    minder truly wedges and must be restarted, note that Rex promotes
    its surviving children to the lead's tab on exit (so a fresh minder
    tab cannot watch those siblings): the lead must watch and mind any
    promoted surviving children directly (or resume stopped ones under
    the new minder) until ownership is restored. The minder in turn
    keeps an active watch on all subagents under it at all times. Never
    run children (or a minder) without an active watch, and never
    double-watch the same child.
- **Builders, investigators, and reviewers: exact models and efforts.**
  Every helper uses one of these exact models and efforts — nothing
  else (no Fable, no Claude Opus minders, no `max` effort for
  reviewers/minders):
  - **Minders:** Antigravity (`agy`), **Gemini 4 Neon on `high`**
    (`gemini-4-neon-high`, `--effort high`). Always `agy` Neon 4 `high`.
  - **Builders:** Antigravity (`agy`), **Gemini 4 Neon on `high`**
    (`gemini-4-neon-high`, `--effort high`; Ben, 2026-11-27: Neon 4
    builders run on `high`, while reviewers and the lead run on `xhigh`).
    Started, watched, and steered with `tools/dev/agents` (the
    `olo-agents` skill), **at most four active builders at once** (across
    worktrees), each scoped to its own crate(s) so none clobbers another.
    Builders never start subagents of their own (the tooling refuses via
    `OLO_SUBAGENT`, and their brief forbids it); they may run
    `claude`/`codex`/`agy` only when their task is about testing those
    CLIs. When a builder finishes and clears review, the lead reads the
    whole diff, runs its tests, inspects any visual changes, and commits
    it.
  - **Second investigator** (`tools/dev/agy-help BRIEF.md`, the
    `olo-debug-help` skill): Antigravity (`agy`), **Gemini 4 Neon on
    `high`**, for root-causing hard bugs in parallel.
  - **Code reviewers (every change, max 2 rounds each):**
    1. **Codex Astra 6.2 on `xhigh`** (`gpt-6.2-astra`,
       `model_reasoning_effort="xhigh"`) via `tools/dev/astra-review`.
       (Codex/Astra is used ONLY for code review — never for dev/QA
       building. Olo's own product support for Codex/Astra in Terminal,
       Rex, Token Pool, and Preferences stays.)
    2. **Antigravity Gemini 4 Neon on `xhigh`** (`gemini-4-neon-xhigh`)
       via `tools/dev/agy-review`.
  - **Visual design reviewers (any change involving visual design work —
    ALL THREE reviewers):**
    1. **Antigravity Gemini 4 Neon on `xhigh`** (`tools/dev/agy-look`
       for screenshots + `tools/dev/agy-review` for code).
    2. **Codex Astra 6.2 on `xhigh`** (`tools/dev/astra-review` for
       code).
    3. **Claude Opus 5.5 on `xhigh`** (`claude-opus-5-5`,
       `--effort xhigh`) via `tools/dev/opus-look` for visual design
       review of screenshots.
    Changes with no visual design work use only the `agy` (`xhigh`) and
    `codex` (`xhigh`) reviewers.
- **Reliably sending & submitting messages to subagents.**
  (Ben, 2026-11-27: bare `olo agent send ID "TEXT"` without `--enter`
  only fills the agent's prompt box and leaves it unsubmitted!) ALWAYS
  send steering/follow-up messages with `tools/dev/agents say NAME|ID
  "TEXT"` (or `tools/dev/agents minder say "TEXT"`), which passes
  `--enter --paste --verify --text` and verifies that the input field
  actually cleared (retrying a safe non-`--force` Enter once if needed
  and failing loudly if text stays stuck). If calling `olo agent send`
  directly, ALWAYS pass `--enter --paste --verify` (and single-quote
  text containing backticks so the shell never expands them), never bare
  `olo agent send ID "TEXT"`.
- **Every helper on an interactive screen of its own, never silent.**
  Every minder, builder, investigator, and reviewer runs interactively
  in its own Rex child tab (`olo agent`, or tmux only where there is no
  Rex pane) via `tools/dev/agents` and its front doors (`agy-help`,
  `agy-review`, `astra-review`, `agy-look`, `opus-look`), so what each
  is doing can be read (`agents peek NAME`), steered (`agents say`),
  and approved (`agents approve`). Never use `agy --print`, `claude -p`,
  or any script that goes silent until completion. What is routine goes
  ahead unasked (`tools/dev/agents allowed`, session-only `--settings`,
  never touching the person's own settings); what is dangerous is
  refused outright; for the rest, **give a helper whatever its task
  reasonably needs** — prefer `--session` ("always allow for this
  session") whenever the prompt offers a safe, scoped prefix so routine
  commands stop generating approval noise (never persist to
  `settings.json` or session-allow broad/destructive prefixes), and deny
  only what is dangerous, out of line with the task, or done safer
  another way (and say which way). Never approve a push, a commit,
  anything on the person's session, deletion outside scope, `olo agent
  prune`, or `sudo`.
- **Stop each helper when done.** Once an agent's result is read and
  nothing more is needed of it, stop it (`tools/dev/agents stop NAME`,
  which keeps its transcript and stops any `agy-NAME…` test session it
  left), then `forget` it when its work is committed; reviewers end
  themselves once their review is saved. Before finishing a piece of
  work, `tools/dev/agents list` and `tools/dev/tsession list` show
  nothing of yours left running. (`agents tidy` cleans up exited/stale
  own-subtree helpers as a safety net, never touching other leads'
  agents.)
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
